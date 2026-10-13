use crate::{
    compiler,
    model::{CellKind, CellResult, ConnectionInput, Notebook, RunResult, now_ms},
    storage::Storage,
};
use futures_util::TryStreamExt;
use postgres_native_tls::MakeTlsConnector;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio_postgres::{Client, Config, SimpleQueryMessage, config::SslMode};

pub const ROW_LIMIT: usize = 500;
const RESULT_BYTES_LIMIT: usize = 2_000_000;

pub struct ActiveRun {
    pub run_id: String,
    pub cancelled: Arc<AtomicBool>,
    pub token: Option<tokio_postgres::CancelToken>,
}

#[derive(Default)]
pub struct Database {
    pub connection: Mutex<Option<Config>>,
    pub active: Mutex<Option<ActiveRun>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    pub notebook: Notebook,
    pub cell_id: Option<String>,
    pub bulk: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub run_id: String,
    pub cell_id: String,
    pub index: usize,
    pub total: usize,
}

fn tls() -> Result<MakeTlsConnector, String> {
    native_tls::TlsConnector::new()
        .map(MakeTlsConnector::new)
        .map_err(|e| e.to_string())
}

pub fn configuration(input: &ConnectionInput) -> Config {
    let mut config = Config::new();
    config
        .host(&input.host)
        .port(input.port)
        .dbname(&input.database)
        .user(&input.user)
        .password(&input.password)
        .ssl_mode(if input.tls {
            SslMode::Require
        } else {
            SslMode::Disable
        });
    config
}

pub async fn connect(input: &ConnectionInput) -> Result<Client, String> {
    connect_config(&configuration(input)).await
}

async fn connect_config(input: &Config) -> Result<Client, String> {
    let mut config = input.clone();
    if config.get_connect_timeout().is_none() {
        config.connect_timeout(Duration::from_secs(10));
    }
    if config.get_application_name().is_none() {
        config.application_name("JupSQL");
    }
    let (client, connection) =
        tokio::time::timeout(Duration::from_secs(12), config.connect(tls()?))
            .await
            .map_err(|_| "Connection timed out after 12 seconds.".to_string())?
            .map_err(|e| error_message(&e))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(client)
}

fn error_message(error: &tokio_postgres::Error) -> String {
    match error.as_db_error() {
        Some(db) => format!("{} [{}]", db.message(), db.code().code()),
        None => error.to_string(),
    }
}

pub async fn test_connection(input: &ConnectionInput) -> Result<String, String> {
    test_config(&configuration(input)).await
}

pub async fn test_config(config: &Config) -> Result<String, String> {
    let client = connect_config(config).await?;
    let row = client
        .query_one("SELECT current_database(), current_user", &[])
        .await
        .map_err(|e| error_message(&e))?;
    Ok(format!(
        "{} · {}",
        row.get::<_, String>(0),
        row.get::<_, String>(1)
    ))
}

struct RunGuard<'a>(&'a Database);
impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.0.active.lock() {
            *active = None;
        }
    }
}

pub async fn cancel(database: &Database) -> Result<(), String> {
    let token = {
        let active = database.active.lock().map_err(|e| e.to_string())?;
        let Some(active) = active.as_ref() else {
            return Ok(());
        };
        active.cancelled.store(true, Ordering::SeqCst);
        active.token.clone()
    };
    if let Some(token) = token {
        tokio::time::timeout(Duration::from_secs(5), token.cancel_query(tls()?))
            .await
            .map_err(|_| {
                "Cancellation request timed out; the statement timeout remains active.".to_string()
            })?
            .map_err(|e| error_message(&e))?;
    }
    Ok(())
}

pub async fn execute(
    database: &Database,
    storage: &Storage,
    request: RunRequest,
    progress: impl Fn(Progress),
) -> Result<RunResult, String> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let mut active = database.active.lock().map_err(|e| e.to_string())?;
        if active.is_some() {
            return Err("A run is already in progress.".into());
        }
        *active = Some(ActiveRun {
            run_id: run_id.clone(),
            cancelled: cancelled.clone(),
            token: None,
        });
    }
    let _guard = RunGuard(database);
    let result = execute_inner(database, storage, &request, &run_id, cancelled, progress).await;
    let logging_error = storage.record("run_finished", json!({ "runId": run_id, "error": result.as_ref().err(), "cancelled": result.as_ref().map(|r| r.cancelled).unwrap_or(false) })).err();
    match result {
        Ok(mut result) => {
            result.logging_error = logging_error;
            Ok(result)
        }
        Err(error) => Err(match logging_error {
            Some(log) => format!("{error}. Logging also failed: {log}"),
            None => error,
        }),
    }
}

async fn execute_inner(
    database: &Database,
    storage: &Storage,
    request: &RunRequest,
    run_id: &str,
    cancelled: Arc<AtomicBool>,
    progress: impl Fn(Progress),
) -> Result<RunResult, String> {
    compiler::validate_notebook(&request.notebook)?;
    let (targets, skipped) = if request.bulk {
        compiler::refresh_targets(&request.notebook, request.cell_id.as_deref())?
    } else {
        (
            vec![request.cell_id.clone().ok_or("Choose a cell to run.")?],
            vec![],
        )
    };
    if targets.is_empty() {
        return Err("There are no query cells to refresh.".into());
    }
    let input = database
        .connection
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or("Connect to PostgreSQL first.")?;
    let readonly = request.bulk
        || request
            .notebook
            .cells
            .iter()
            .find(|c| Some(&c.id) == request.cell_id.as_ref())
            .is_none_or(|c| c.kind == CellKind::Query);
    let started_at = now_ms();
    storage.record("run_started", json!({ "runId": run_id, "bulk": request.bulk, "readOnly": readonly, "targets": targets, "skipped": skipped, "notebook": request.notebook }))?;
    let client = connect_config(&input).await?;
    {
        let mut active = database.active.lock().map_err(|e| e.to_string())?;
        if let Some(active) = active.as_mut() {
            active.token = Some(client.cancel_token());
        }
    }
    if cancelled.load(Ordering::SeqCst) {
        return Err("Run cancelled before execution.".into());
    }
    client.batch_execute(if readonly {
        "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; SET LOCAL statement_timeout = '30s'; SET LOCAL lock_timeout = '3s'; SET LOCAL idle_in_transaction_session_timeout = '60s';"
    } else {
        "BEGIN; SET LOCAL statement_timeout = '30s'; SET LOCAL lock_timeout = '3s'; SET LOCAL idle_in_transaction_session_timeout = '60s';"
    }).await.map_err(|e| error_message(&e))?;
    let snapshot: String = client
        .query_one("SELECT pg_current_snapshot()::text", &[])
        .await
        .map_err(|e| error_message(&e))?
        .get(0);
    storage.record(
        "snapshot_acquired",
        json!({ "runId": run_id, "snapshot": snapshot }),
    )?;
    let mut results = vec![];
    for (index, id) in targets.iter().enumerate() {
        if cancelled.load(Ordering::SeqCst) {
            break;
        }
        let cell = request
            .notebook
            .cells
            .iter()
            .find(|c| &c.id == id)
            .ok_or("Cell does not exist.")?;
        progress(Progress {
            run_id: run_id.into(),
            cell_id: id.clone(),
            index,
            total: targets.len(),
        });
        let started = Instant::now();
        let mut result = CellResult {
            cell_id: id.clone(),
            compiled_sql: String::new(),
            columns: vec![],
            rows: vec![],
            affected_rows: 0,
            truncated: false,
            elapsed_ms: 0,
            error: None,
        };
        match compiler::compile(&request.notebook, id) {
            Err(error) => result.error = Some(error),
            Ok(sql) => {
                result.compiled_sql = sql.clone();
                storage.record("cell_started", json!({ "runId": run_id, "cellId": id, "name": cell.name, "kind": cell.kind, "sql": cell.sql, "compiledSql": sql }))?;
                client
                    .batch_execute("SAVEPOINT jupsql_cell")
                    .await
                    .map_err(|e| error_message(&e))?;
                // Limit only the displayed query, never a referenced definition.
                let executed_sql = if readonly {
                    format!(
                        "SELECT * FROM ({sql}) AS jupsql_preview LIMIT {}",
                        ROW_LIMIT + 1
                    )
                } else {
                    sql
                };
                match collect(&client, &executed_sql, &mut result).await {
                    Ok(()) => {
                        client
                            .batch_execute("RELEASE SAVEPOINT jupsql_cell")
                            .await
                            .map_err(|e| error_message(&e))?;
                    }
                    Err(error) => {
                        result.error = Some(error);
                        client
                            .batch_execute(
                                "ROLLBACK TO SAVEPOINT jupsql_cell; RELEASE SAVEPOINT jupsql_cell",
                            )
                            .await
                            .map_err(|e| error_message(&e))?;
                    }
                }
            }
        }
        result.elapsed_ms = started.elapsed().as_millis();
        storage.record("cell_finished", json!({ "runId": run_id, "cellId": id, "elapsedMs": result.elapsed_ms, "displayedRows": result.rows.len(), "affectedRows": result.affected_rows, "truncated": result.truncated, "error": result.error }))?;
        results.push(result);
    }
    let was_cancelled = cancelled.load(Ordering::SeqCst);
    client.batch_execute(if was_cancelled { "ROLLBACK" } else { "COMMIT" }).await
        .map_err(|e| format!("Could not confirm the transaction outcome: {}. Check the database before retrying a mutation.", error_message(&e)))?;
    if was_cancelled && !readonly {
        for result in &mut results {
            result.error = Some("Mutation rolled back because the run was cancelled.".into());
        }
    }
    Ok(RunResult {
        run_id: run_id.into(),
        snapshot,
        started_at,
        results,
        skipped,
        cancelled: was_cancelled,
        logging_error: None,
    })
}

async fn collect(client: &Client, sql: &str, result: &mut CellResult) -> Result<(), String> {
    let stream = client
        .simple_query_raw(sql)
        .await
        .map_err(|e| error_message(&e))?;
    futures_util::pin_mut!(stream);
    let mut bytes = 0;
    while let Some(message) = stream.try_next().await.map_err(|e| error_message(&e))? {
        match message {
            SimpleQueryMessage::RowDescription(columns) => {
                result.columns = columns.iter().map(|c| c.name().to_string()).collect()
            }
            SimpleQueryMessage::Row(row) => {
                let values: Vec<_> = (0..row.len())
                    .map(|i| row.get(i).map(str::to_string))
                    .collect();
                bytes += values.iter().flatten().map(String::len).sum::<usize>();
                if result.rows.len() < ROW_LIMIT && bytes <= RESULT_BYTES_LIMIT {
                    result.rows.push(values);
                } else {
                    result.truncated = true;
                }
            }
            SimpleQueryMessage::CommandComplete(count) => result.affected_rows = count,
            _ => {}
        }
    }
    Ok(())
}
