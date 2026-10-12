use jupsql_lib::{
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
    let setup = database::connect(&connection).await.unwrap();
    setup.batch_execute("CREATE TABLE orders (id int, amount numeric); INSERT INTO orders VALUES (1, 10), (2, 20); CREATE FUNCTION hidden_write() RETURNS int LANGUAGE plpgsql AS $$ BEGIN INSERT INTO orders VALUES (99, 99); RETURN 99; END $$;").await.unwrap();
    let database = Database::default();
    *database.connection.lock().unwrap() = Some(connection.clone());
    let directory =
        std::env::temp_dir().join(format!("jupsql-integration-{}", uuid::Uuid::new_v4()));
    let storage = Storage::new(directory.clone()).unwrap();

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
    assert_eq!(result.results[1].rows[0][0].as_deref(), Some("900"));

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
    assert!(result.results[0].elapsed_ms < 3000);
    assert!(database.active.lock().unwrap().is_none());

    let log = std::fs::read_to_string(&storage.log_path).unwrap();
    assert!(log.contains("compiledSql"));
    assert!(log.contains("snapshot_acquired"));
    assert!(!log.contains("never-log-this-password"));
    assert!(!log.contains("\"rows\":"));
    std::fs::remove_dir_all(directory).unwrap();
}
