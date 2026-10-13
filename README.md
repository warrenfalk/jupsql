# JupSQL

A local Tauri desktop notebook for PostgreSQL. Write SQL, see results inline, and reuse named query definitions in other cells.

## Run

```sh
nix develop
pnpm install --frozen-lockfile
pnpm dev
```

Or run `direnv allow` once and use the same pnpm commands. The flake supplies Node 24, pnpm, Rust, PostgreSQL, and Linux WebKit/GTK dependencies. It selects a matching Mesa runtime so the app does not load an incompatible host graphics library.

JupSQL automatically connects using a nonempty `DATABASE_URL` first. If it is absent or empty, it uses the individual PG variables with these defaults:

```text
PGUSER=postgres
PGHOST=localhost
PGPORT=5432
PGPASSWORD=postgres
PGDATABASE=<effective PGUSER>
```

With no variables set, this is equivalent to `postgres://postgres:postgres@localhost:5432/postgres`. An explicitly empty password stays empty. Saved profiles do not affect startup; manual connection changes last for the current session.

```sh
DATABASE_URL='postgres://myuser:password@localhost:5432/mydb' pnpm dev
# When DATABASE_URL is absent:
PGHOST=localhost PGPORT=5432 PGDATABASE=mydb PGUSER=myuser pnpm dev
# A socket directory or full .s.PGSQL.<port> socket path needs no PGPORT:
PGHOST=/path/to/postgres/socket PGDATABASE=mydb PGUSER=myuser pnpm dev
```

`DATABASE_URL` accepts `postgres://` and `postgresql://`, percent-encoded credentials, and connection options supported by tokio-postgres (for example `sslmode=require` and `application_name`). It takes precedence over all PG variables, including when some URL fields are omitted; omitted fields use the defaults above. An invalid URL reports an error rather than falling back to another connection. URL options are retained for query connections; using the manual connection dialog instead applies its visible fields. The app's 12-second overall connection timeout and query limits still apply.

For a `PGHOST` Unix socket, JupSQL detects the port from the socket filename and ignores `PGPORT`, including invalid values. A directory must contain exactly one PostgreSQL socket; if it contains several, specify the full socket path. A socket directory in `DATABASE_URL` uses the URL's port (or `5432`), so it can select a particular socket in a shared directory.

An unsuccessful automatic connection shows an error and leaves the settings editable. Automatic TCP connections prefer TLS when the server supports it unless the URL specifies another mode; the manual dialog can require TLS with certificate verification. Credentials stay in memory. Raw URLs and passwords are excluded from saved files and session logs. Launch the app from the shell exporting the variables, or define them in `.env.local` and load it with direnv.

The example notebook uses `VALUES`, so it works on any PostgreSQL database without creating tables. An optional local demo is available:

```sh
pnpm db:demo
DATABASE_URL='postgres://jupsql:@127.0.0.1:55432/jupsql_demo?sslmode=disable' pnpm dev
pg_ctl -D .cache/demo-postgres/data stop
```

The demo listens only on loopback at port `55432`, uses a project-local cluster, and remains running until stopped.

`pnpm dev:web` is only a browser preview; database execution, filesystem persistence, and session logs require Tauri.

## Notebook behavior

Give a query cell a unique lowercase name, such as `paid_orders`. Reference it as a relation:

```sql
SELECT customer, SUM(amount) AS total
FROM notebook.paid_orders AS orders
GROUP BY customer;
```

- References expand the **current SQL definition**, including edits that have not been executed. Results from previous runs are not inputs.
- References work anywhere in the notebook, irrespective of visual order. Explicit aliases and ordinary SQL CTEs are supported. Missing references, cycles, duplicate names, and attempts to reference mutations produce errors.
- Editing SQL or cell types marks the affected cells and their transitive dependents stale. Renaming a cell marks its dependents stale but preserves its own result when the compiled SQL is unchanged; renaming does not clear an already stale result. Notes and cell order do not change query results. A result that arrives after its executable definition was edited remains stale.
- **Run** executes one cell. Ctrl/Cmd+Enter runs the focused cell.
- **Rerun dependents** refreshes the selected source cell and all its downstream query cells. **Run all queries** refreshes every query cell. Both skip mutation cells.
- Every bulk run uses one `REPEATABLE READ READ ONLY` transaction and a fixed copy of the submitted notebook. Each cell runs as its own composed statement; upstream SQL can execute again in each dependent statement. Savepoints let independent cells finish even when another cell fails.
- All query cells run inside read-only transactions, including individual runs. The compiler rejects obvious writes, modifying CTEs, `SELECT INTO`, and row locks; PostgreSQL rejects hidden database writes from functions. This is not a sandbox for external side effects implemented by database extensions.
- Mutation cells run only when explicitly executed. They support one `INSERT`, `UPDATE`, `DELETE`, or `WITH`/`SELECT` statement, can use query definitions as inputs, and commit individually. Successful mutations mark other results stale because the app does not attempt to infer their table-level effects. Transaction-control statements and schema changes are excluded from the prototype.
- Results are bounded to 500 rows and 2 MB, with 50-row display pages. The cap applies only to the displayed query, never to referenced definitions. CSV exports contain displayed rows. NULL is displayed distinctly from empty text; values retain PostgreSQL's textual representation.
- Queries time out after 30 seconds; lock waits time out after 3 seconds. Cancel requests reach PostgreSQL, and cancelled mutations roll back. No query is automatically retried.

## Saving and session logs

In the development shell, data lives in `.local-data/` (ignored by Git):

```text
.local-data/notebook.json           current notebook, saved atomically
.local-data/sessions/<time>-<id>.jsonl
```

Outside that shell, the default is Tauri's OS application-data directory for `dev.warren.jupsql`. `JUPSQL_DATA_DIR` can choose another directory. SQL and notes autosave after a short debounce, and closing the window flushes the latest draft. Results are kept in memory only. Import/export uses `.jupsql.json` files; importing replaces the current notebook. Export the current notebook first if you want to keep it separately.

Use **Session log** to inspect the current log, copy its path, or export it. To investigate a session, start at `run_started` for the submitted notebook and targets, follow `snapshot_acquired`, then inspect each `cell_started`/`cell_finished` and the final `run_finished`. Match events using `sessionId`, `runId`, and `cellId`. `notebook_saved` records debounced draft snapshots; focus/add/delete/reorder/import events record navigation and editing context.

Logs include original and expanded SQL, snapshots, errors, durations, and row counts. They do not include result sets or connection passwords. **Literal values in SQL and notes are recorded as written.** Files are local, with owner-only permissions on Unix, and have no remote telemetry or automatic retention/deletion. Saving and logging failures appear in the UI.

## Checks

```sh
pnpm check
pnpm test
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
pnpm test:postgres
pnpm test:desktop  # Linux: actual Tauri/WebKit window and a disposable Postgres
```

The PostgreSQL integration test creates and removes a disposable cluster. It checks SQL composition, mutation exclusion, server-enforced read-only behavior, preview bounds, concurrent-write snapshot consistency, explicit mutations, cancellation, and logging.

The native desktop smoke test uses WebKitWebDriver, an isolated Xvfb display, and separate notebook directories. It checks manual connections, automatic TCP/socket connections, recovery after automatic connection failures, password exclusion, running, editing, transitive staleness, composing current SQL, reference errors, saving, and session logs. Its screenshot and session log are written under `.cache/validation/`.

Build a desktop executable with `pnpm desktop:build` (or add `--debug` for a faster local build). Installer packaging, schema browsing, multiple simultaneous notebooks/connections, charts, and references to cached results are outside this prototype. The SQL parser supports a broad PostgreSQL subset; unsupported syntax is reported rather than sent unchanged.

Implementation: React/Vite/TypeScript with CodeMirror and Tailwind in `src/`; SQL analysis/expansion in `src-tauri/src/compiler.rs`; database transactions and cancellation in `database.rs`; local persistence and JSONL recording in `storage.rs`.

Technical references: [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/), [Tauri dialogs](https://v2.tauri.app/plugin/dialog/), [PostgreSQL transaction modes](https://www.postgresql.org/docs/current/sql-set-transaction.html), [PostgreSQL CTEs](https://www.postgresql.org/docs/current/queries-with.html).
