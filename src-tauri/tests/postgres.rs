use jupsql_lib::{
    connection,
    database::{self, Database, RunRequest},
    model::{Cell, CellKind, ConnectionInput, Notebook},
    storage::Storage,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

fn input() -> ConnectionInput {
    ConnectionInput {
        host: std::env::var("JUPSQL_TEST_HOST").expect("Use pnpm test:postgres"),
        port: 55433,
        database: "postgres".into(),
        user: "jupsql".into(),
        password: "never-log-this-password".into(),
        tls: false,
    }
}

fn cell(name: &str, sql: &str, kind: CellKind) -> Cell {
    Cell {
        id: name.into(),
        name: name.into(),
        sql: sql.into(),
        kind,
        notes: String::new(),
    }
}

fn notebook(cells: Vec<Cell>) -> Notebook {
    Notebook {
        version: 1,
        title: "Integration".into(),
        cells,
    }
}

#[tokio::test]
#[ignore = "Starts a disposable PostgreSQL via scripts/test-postgres.sh"]
async fn real_postgres_execution_contract() {
    let connection = input();
    // An environment socket selects the nonstandard server port without PGPORT.
    // A full socket path also works, even with a conflicting/invalid PGPORT.
    for host in [
        connection.host.clone(),
        format!("{}/.s.PGSQL.55433", connection.host),
    ] {
        let defaults = connection::environment_defaults(|key| match key {
            "PGHOST" => Some(host.clone()),
            "PGPORT" => Some("ignored-for-sockets".into()),
            "PGUSER" => Some("jupsql".into()),
            "PGDATABASE" => Some("postgres".into()),
            _ => None,
        });
        assert!(defaults.error.is_none());
        let resolved = connection::resolve(defaults.input).unwrap();
        assert_eq!(resolved.port, 55433);
        assert_eq!(
            database::test_connection(&resolved).await.unwrap(),
            "postgres · jupsql"
        );
    }
    let setup = database::connect(&connection).await.unwrap();
    setup.batch_execute("CREATE TABLE orders (id int, amount numeric); INSERT INTO orders VALUES (1, 10), (2, 20); CREATE FUNCTION hidden_write() RETURNS int LANGUAGE plpgsql AS $$ BEGIN INSERT INTO orders VALUES (99, 99); RETURN 99; END $$;").await.unwrap();
    let database = Database::default();
    *database.connection.lock().unwrap() = Some(database::configuration(&connection));
    let directory =
        std::env::temp_dir().join(format!("jupsql-integration-{}", uuid::Uuid::new_v4()));
    let storage = Storage::new(directory.clone(), None).unwrap();

    // DATABASE_URL must win over PG variables and retain options on every query connection.
    let url = format!(
        "postgres://jupsql:url%40password-never-log@{}:55433/postgres?sslmode=disable&application_name=url-client&options=-c%20search_path%3Dpg_catalog",
        connection.host.replace('/', "%2F")
    );
    let from_url = connection::environment_defaults(|key| match key {
        "DATABASE_URL" => Some(url.clone()),
        "PGHOST" => Some("ignored.example".into()),
        "PGPORT" => Some("invalid".into()),
        _ => None,
    });
    assert!(from_url.error.is_none());
    let url_database = Database::default();
    *url_database.connection.lock().unwrap() = from_url.config;
    let result = database::execute(
        &url_database,
        &storage,
        RunRequest {
            notebook: notebook(vec![cell(
                "settings",
                "SELECT current_setting('application_name'), current_setting('search_path')",
                CellKind::Query,
            )]),
            cell_id: Some("settings".into()),
            bulk: false,
        },
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(
        result.results[0].rows[0],
        [Some("url-client".into()), Some("pg_catalog".into())]
    );
    let log = std::fs::read_to_string(&storage.log_path).unwrap();
    assert!(!log.contains("url@password-never-log"));
    assert!(!log.contains(&url));

    let book = notebook(vec![
        cell(
            "base",
            "SELECT * FROM orders WHERE amount >= 20",
            CellKind::Query,
        ),
        cell(
            "summary",
            "SELECT sum(amount) AS total FROM notebook.base",
            CellKind::Query,
        ),
        cell(
            "write",
            "DELETE FROM orders WHERE id IN (SELECT id FROM notebook.base)",
            CellKind::Mutation,
        ),
    ]);
    let result = database::execute(
        &database,
        &storage,
        RunRequest {
            notebook: book.clone(),
            cell_id: Some("base".into()),
            bulk: true,
        },
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(result.results.len(), 2);
    assert_eq!(result.results[1].rows[0][0].as_deref(), Some("20"));
    assert_eq!(result.skipped, ["write"]);
    assert_eq!(
        setup
            .query_one("SELECT count(*) FROM orders", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        2
    );

    // A hidden function write must fail on the server, and the next cell must still run.
    let guarded = notebook(vec![
        cell("bad", "SELECT hidden_write()", CellKind::Query),
        cell("good", "SELECT count(*) FROM orders", CellKind::Query),
    ]);
    let result = database::execute(
        &database,
        &storage,
        RunRequest {
            notebook: guarded,
            cell_id: None,
            bulk: true,
        },
        |_| {},
    )
    .await
    .unwrap();
    assert!(
        result.results[0]
            .error
            .as_ref()
            .unwrap()
            .contains("read-only")
    );
    assert_eq!(result.results[1].rows[0][0].as_deref(), Some("2"));

    // The preview limit must not leak into reference expansion.
    let unlimited = notebook(vec![
        cell(
            "all_numbers",
            "SELECT generate_series(1, 900) AS n",
            CellKind::Query,
        ),
        cell(
            "total",
            "SELECT count(*) FROM notebook.all_numbers",
            CellKind::Query,
        ),
    ]);
    let result = database::execute(
        &database,
        &storage,
        RunRequest {
            notebook: unlimited,
            cell_id: None,
            bulk: true,
        },
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(result.results[0].rows.len(), 500);
    assert!(result.results[0].truncated);
    assert_eq!(result.results[0].returned_rows, Some(900));
    assert!(!result.results[0].row_count_limited);
    assert_eq!(result.results[1].rows[0][0].as_deref(), Some("900"));

    // Count the returned result, independently of the retained display preview.
    for count in [0, 1, 500, 501, 49_999, 50_000, 50_001, 75_000] {
        let result = database::execute(
            &database,
            &storage,
            RunRequest {
                notebook: notebook(vec![cell(
                    "numbers",
                    &format!("SELECT generate_series(1, {count}) AS n"),
                    CellKind::Query,
                )]),
                cell_id: Some("numbers".into()),
                bulk: false,
            },
            |_| {},
        )
        .await
        .unwrap();
        let result = &result.results[0];
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.returned_rows, Some(count.min(50_000)));
        assert_eq!(result.row_count_limited, count > 50_000);
        assert_eq!(result.rows.len() as u64, count.min(500));
        assert_eq!(result.truncated, count > 500);
    }
    for (sql, count, preview, limited) in [
        (
            "SELECT generate_series(1, 75000) LIMIT 42",
            Some(42),
            42,
            false,
        ),
        (
            "SELECT repeat('x', 600000) FROM generate_series(1, 10)",
            Some(10),
            3,
            false,
        ),
        (
            "SELECT repeat('x', 2000001) FROM generate_series(1, 2)",
            Some(2),
            0,
            false,
        ),
        (
            "SELECT CASE WHEN n = 900 THEN 1 / (n - 900) ELSE n END FROM generate_series(1, 1000) AS n",
            None,
            0,
            false,
        ),
    ] {
        let result = database::execute(
            &database,
            &storage,
            RunRequest {
                notebook: notebook(vec![cell("counted", sql, CellKind::Query)]),
                cell_id: Some("counted".into()),
                bulk: false,
            },
            |_| {},
        )
        .await
        .unwrap();
        let result = &result.results[0];
        assert_eq!(result.returned_rows, count);
        assert_eq!(result.row_count_limited, limited);
        if count.is_some() {
            assert!(result.error.is_none(), "{:?}", result.error);
            assert_eq!(result.rows.len(), preview);
        } else {
            assert!(result.error.is_some());
        }
    }
    let result = database::execute(
        &database,
        &storage,
        RunRequest {
            notebook: notebook(vec![
                cell(
                    "many",
                    "SELECT generate_series(1, 75000) AS n",
                    CellKind::Query,
                ),
                cell(
                    "full_count",
                    "SELECT count(*) FROM notebook.many",
                    CellKind::Query,
                ),
            ]),
            cell_id: None,
            bulk: true,
        },
        |_| {},
    )
    .await
    .unwrap();
    assert!(result.results[0].row_count_limited);
    assert_eq!(result.results[1].rows[0][0].as_deref(), Some("75000"));

    setup
        .batch_execute("CREATE TABLE returned_rows (n int)")
        .await
        .unwrap();
    let result = database::execute(
        &database,
        &storage,
        RunRequest {
            notebook: notebook(vec![cell(
                "insert_rows",
                "INSERT INTO returned_rows SELECT generate_series(1, 600) RETURNING n",
                CellKind::Mutation,
            )]),
            cell_id: Some("insert_rows".into()),
            bulk: false,
        },
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(result.results[0].returned_rows, Some(600));
    assert_eq!(result.results[0].affected_rows, 600);
    assert_eq!(result.results[0].rows.len(), 500);
    assert!(result.results[0].truncated);

    // Insert on a second connection after the batch has acquired its snapshot.
    let started = Arc::new(AtomicBool::new(false));
    let trigger = started.clone();
    let writer_input = connection.clone();
    let writer = tokio::spawn(async move {
        while !trigger.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let client = database::connect(&writer_input).await.unwrap();
        client
            .batch_execute("INSERT INTO orders VALUES (3, 30)")
            .await
            .unwrap();
    });
    let same_snapshot = notebook(vec![
        cell(
            "first",
            "SELECT count(*), pg_sleep(0.3) FROM orders",
            CellKind::Query,
        ),
        cell("second", "SELECT count(*) FROM orders", CellKind::Query),
    ]);
    let result = database::execute(
        &database,
        &storage,
        RunRequest {
            notebook: same_snapshot,
            cell_id: None,
            bulk: true,
        },
        |_| {
            started.store(true, Ordering::SeqCst);
        },
    )
    .await
    .unwrap();
    writer.await.unwrap();
    assert_eq!(result.results[0].rows[0][0].as_deref(), Some("2"));
    assert_eq!(result.results[1].rows[0][0].as_deref(), Some("2"));
    assert_eq!(
        setup
            .query_one("SELECT count(*) FROM orders", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        3
    );

    // Explicit mutations can consume query definitions and do commit.
    let result = database::execute(
        &database,
        &storage,
        RunRequest {
            notebook: book,
            cell_id: Some("write".into()),
            bulk: false,
        },
        |_| {},
    )
    .await
    .unwrap();
    assert!(result.results[0].error.is_none());
    assert_eq!(result.results[0].affected_rows, 2);
    assert_eq!(
        setup
            .query_one("SELECT count(*) FROM orders", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );

    // Cancellation reaches PostgreSQL, and the run slot is reusable afterward.
    let slow = notebook(vec![cell("slow", "SELECT pg_sleep(20)", CellKind::Query)]);
    let execution = database::execute(
        &database,
        &storage,
        RunRequest {
            notebook: slow,
            cell_id: None,
            bulk: true,
        },
        |_| {},
    );
    let cancellation = async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        database::cancel(&database).await.unwrap();
    };
    let (result, ()) = tokio::join!(execution, cancellation);
    let result = result.unwrap();
    assert!(result.cancelled);
    assert_eq!(result.results[0].returned_rows, None);
    assert!(result.results[0].elapsed_ms < 3000);
    assert!(database.active.lock().unwrap().is_none());

    let log = std::fs::read_to_string(&storage.log_path).unwrap();
    assert!(log.contains("compiledSql"));
    assert!(log.contains("snapshot_acquired"));
    assert!(log.contains("\"returnedRows\":50000"));
    assert!(log.contains("\"rowCountLimited\":true"));
    assert!(!log.contains("never-log-this-password"));
    assert!(!log.contains("\"rows\":"));
    std::fs::remove_dir_all(directory).unwrap();
}
