pub mod compiler;
pub mod connection;
pub mod database;
pub mod model;
pub mod storage;

use connection::{ConnectionDefaults, ConnectionIdentity};
use model::{Analysis, ConnectionInput, Notebook, RunResult};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

#[derive(Clone)]
struct Workspace {
    storage: Arc<storage::Storage>,
    defaults: ConnectionDefaults,
}

impl Workspace {
    fn bootstrap(&self) -> Result<Bootstrap, String> {
        Ok(Bootstrap {
            notebook: self.storage.load()?,
            notebook_id: self.storage.id().into(),
            session_id: self.storage.session_id.clone(),
            log_path: self.storage.log_path.display().to_string(),
            data_directory: self.storage.directory.display().to_string(),
            connection_defaults: self.defaults.clone(),
        })
    }
}

struct AppState {
    root: PathBuf,
    workspace: Mutex<Workspace>,
    database: database::Database,
    navigation: tokio::sync::Mutex<()>,
    known_connections: Mutex<Vec<(ConnectionIdentity, tokio_postgres::Config)>>,
}

impl AppState {
    fn workspace(&self) -> Result<Workspace, String> {
        self.workspace
            .lock()
            .map(|workspace| workspace.clone())
            .map_err(|error| error.to_string())
    }

    fn notebook(&self, id: &str) -> Result<Workspace, String> {
        let workspace = self.workspace()?;
        if workspace.storage.id() != id {
            return Err(
                "The notebook changed. Please retry the action in the current notebook.".into(),
            );
        }
        Ok(workspace)
    }

    async fn open(&self, current_id: &str, id: &str) -> Result<Bootstrap, String> {
        self.navigate(current_id, Some(id)).await
    }

    async fn navigate(&self, current_id: &str, id: Option<&str>) -> Result<Bootstrap, String> {
        let _navigation = self.navigation.try_lock().map_err(|_| {
            "Wait for the current query or connection attempt before switching notebooks."
                .to_string()
        })?;
        let current = self.notebook(current_id)?;
        if Some(current_id) == id {
            return current.bootstrap();
        }
        current.storage.record(
            if id.is_some() {
                "notebook_switch_requested"
            } else {
                "notebook_create_requested"
            },
            json!({"notebookId": id}),
        )?;
        let storage = Arc::new(match id {
            Some(id) => storage::Storage::open(self.root.clone(), id)?,
            None => {
                storage::Storage::create(self.root.clone(), current.storage.connection_identity()?)?
            }
        });
        let identity = storage.connection_identity()?;
        let cached = self
            .known_connections
            .lock()
            .map_err(|error| error.to_string())?
            .iter()
            .find(|(known, _)| Some(known) == identity.as_ref())
            .map(|(_, config)| config.clone());
        let defaults = if let Some(config) = cached {
            ConnectionDefaults {
                input: connection::profile(&config)?,
                auto_connect: true,
                error: None,
                config: Some(config),
            }
        } else {
            let input =
                identity
                    .as_ref()
                    .map(ConnectionIdentity::input)
                    .unwrap_or(ConnectionInput {
                        host: "localhost".into(),
                        port: 5432,
                        database: "postgres".into(),
                        user: "postgres".into(),
                        password: String::new(),
                        tls: false,
                    });
            ConnectionDefaults {
                input,
                auto_connect: false,
                error: None,
                config: None,
            }
        };
        let next = Workspace { storage, defaults };
        let bootstrap = next.bootstrap()?;
        *self
            .database
            .connection
            .lock()
            .map_err(|error| error.to_string())? = None;
        *self.workspace.lock().map_err(|error| error.to_string())? = next;
        Ok(bootstrap)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    notebook: Option<Notebook>,
    notebook_id: String,
    session_id: String,
    log_path: String,
    data_directory: String,
    connection_defaults: ConnectionDefaults,
}

#[tauri::command]
fn bootstrap(state: State<AppState>) -> Result<Bootstrap, String> {
    state.workspace()?.bootstrap()
}

#[tauri::command]
fn list_notebooks(state: State<AppState>) -> Result<Vec<storage::NotebookSummary>, String> {
    storage::Storage::list(&state.root)
}

#[tauri::command]
async fn open_notebook(
    state: State<'_, AppState>,
    notebook_id: String,
    id: String,
) -> Result<Bootstrap, String> {
    state.open(&notebook_id, &id).await
}

#[tauri::command]
async fn create_notebook(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<Bootstrap, String> {
    state.navigate(&notebook_id, None).await
}

#[tauri::command]
fn analyze_notebook(notebook: Notebook) -> Vec<Analysis> {
    compiler::analyze(&notebook)
}

#[tauri::command]
fn save_notebook(
    state: State<AppState>,
    notebook_id: String,
    notebook: Notebook,
) -> Result<(), String> {
    state.notebook(&notebook_id)?.storage.save(&notebook)
}

#[tauri::command]
async fn connect_database(
    state: State<'_, AppState>,
    notebook_id: String,
    input: ConnectionInput,
) -> Result<String, String> {
    let _navigation = state
        .navigation
        .try_lock()
        .map_err(|_| "Wait for the current operation before changing connections.".to_string())?;
    let workspace = state.notebook(&notebook_id)?;
    let config =
        connection::resolve(input.clone()).map(|resolved| database::configuration(&resolved));
    establish_connection(&state, &workspace, &input, config).await
}

#[tauri::command]
async fn connect_environment(
    state: State<'_, AppState>,
    notebook_id: String,
) -> Result<String, String> {
    let _navigation = state
        .navigation
        .try_lock()
        .map_err(|_| "Wait for the current operation before changing connections.".to_string())?;
    let workspace = state.notebook(&notebook_id)?;
    let defaults = workspace.defaults.clone();
    let config = defaults.config.ok_or_else(|| {
        defaults
            .error
            .unwrap_or_else(|| "Could not read connection settings.".into())
    });
    establish_connection(&state, &workspace, &defaults.input, config).await
}

async fn establish_connection(
    state: &AppState,
    workspace: &Workspace,
    input: &ConnectionInput,
    config: Result<tokio_postgres::Config, String>,
) -> Result<String, String> {
    let result = match &config {
        Ok(config) => database::test_config(config).await,
        Err(error) => Err(error.clone()),
    };
    workspace.storage.record("connection_attempt", json!({ "host": input.host, "port": input.port, "database": input.database, "user": input.user, "tls": input.tls, "error": result.as_ref().err() }))?;
    let label = result?;
    let config = config?;
    let identity = ConnectionIdentity::from_config(&config);
    let previous = workspace.storage.connection_identity()?;
    workspace.storage.associate_connection(identity.clone())?;
    if previous.as_ref() == Some(&identity) {
        workspace.storage.mark_activity()?;
    }
    let defaults = ConnectionDefaults {
        input: connection::profile(&config)?,
        auto_connect: true,
        error: None,
        config: Some(config.clone()),
    };
    let mut known = state
        .known_connections
        .lock()
        .map_err(|error| error.to_string())?;
    known.retain(|(key, _)| key != &identity);
    known.push((identity, config.clone()));
    *state
        .database
        .connection
        .lock()
        .map_err(|error| error.to_string())? = Some(config);
    state
        .workspace
        .lock()
        .map_err(|error| error.to_string())?
        .defaults = defaults;
    Ok(label)
}

#[tauri::command]
async fn run_cells(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    notebook_id: String,
    request: database::RunRequest,
) -> Result<RunResult, String> {
    let _navigation = state
        .navigation
        .try_lock()
        .map_err(|_| "Wait for the current operation before running queries.".to_string())?;
    let workspace = state.notebook(&notebook_id)?;
    let mut result = database::execute(&state.database, &workspace.storage, request, |progress| {
        let _ = app.emit("run-progress", progress);
    })
    .await?;
    if let Err(error) = workspace.storage.mark_activity() {
        result.logging_error = Some(error);
    }
    Ok(result)
}

#[tauri::command]
async fn cancel_run(state: State<'_, AppState>, notebook_id: String) -> Result<(), String> {
    state
        .notebook(&notebook_id)?
        .storage
        .record("cancel_requested", json!({}))?;
    database::cancel(&state.database).await
}

#[tauri::command]
fn record_event(
    state: State<AppState>,
    notebook_id: String,
    event: String,
    data: Value,
) -> Result<(), String> {
    state
        .notebook(&notebook_id)?
        .storage
        .record(&format!("ui.{event}"), data)
}

#[tauri::command]
fn read_session_log(state: State<AppState>, notebook_id: String) -> Result<String, String> {
    std::fs::read_to_string(&state.notebook(&notebook_id)?.storage.log_path)
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn export_file(
    app: tauri::AppHandle,
    name: String,
    contents: String,
) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let filename = std::path::Path::new(&name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("jupsql-export.json");
        let Some(path) = app
            .dialog()
            .file()
            .set_file_name(filename)
            .blocking_save_file()
        else {
            return Ok(false);
        };
        let path = path.into_path().map_err(|e| e.to_string())?;
        std::fs::write(&path, contents).map_err(|e| e.to_string())?;
        Ok(true)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(target_os = "linux")]
fn prepare_display() {
    use gtk::prelude::*;
    if let Some(screen) = gtk::gdk::Screen::default()
        && screen.resolution() <= 0.0
    {
        let dpi = gtk::Settings::default()
            .map(|settings| f64::from(settings.gtk_xft_dpi()) / 1024.0)
            .filter(|dpi| *dpi > 0.0)
            .unwrap_or(96.0);
        screen.set_resolution(dpi);
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            list_notebooks,
            open_notebook,
            create_notebook,
            analyze_notebook,
            save_notebook,
            connect_database,
            connect_environment,
            run_cells,
            cancel_run,
            record_event,
            read_session_log,
            export_file
        ])
        .setup(|app| {
            let directory = std::env::var_os("JUPSQL_DATA_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or(app.path().app_data_dir()?);
            let defaults = connection::environment_defaults(|name| std::env::var(name).ok());
            let identity = defaults
                .config
                .as_ref()
                .map(connection::ConnectionIdentity::from_config);
            let storage = Arc::new(storage::Storage::new(directory.clone(), identity).map_err(std::io::Error::other)?);
            let input = &defaults.input;
            storage.record("connection_environment", json!({ "host": input.host, "port": input.port, "database": input.database, "user": input.user, "tls": input.tls, "error": defaults.error })).map_err(std::io::Error::other)?;
            let known_connections = defaults.config.as_ref().map(|config| (ConnectionIdentity::from_config(config), config.clone())).into_iter().collect();
            app.manage(AppState {
                root: directory,
                workspace: Mutex::new(Workspace { storage, defaults }),
                database: database::Database::default(),
                navigation: tokio::sync::Mutex::new(()),
                known_connections: Mutex::new(known_connections),
            });
            #[cfg(target_os = "linux")]
            prepare_display();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("JupSQL")
                .inner_size(1380.0, 960.0)
                .min_inner_size(900.0, 600.0)
                .on_navigation(|url| {
                    matches!(url.scheme(), "tauri" | "http" | "https")
                        && matches!(
                            url.host_str(),
                            Some("localhost" | "127.0.0.1" | "tauri.localhost")
                        )
                })
                .build()?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                let state = window.state::<AppState>();
                if let Ok(workspace) = state.workspace() { let _ = workspace.storage.record("window_closed", json!({})); }
            }
        })
        .run(tauri::generate_context!())
        .expect("Could not start JupSQL");
}

#[cfg(test)]
mod workspace_tests {
    use super::*;

    fn defaults(database: &str) -> ConnectionDefaults {
        connection::environment_defaults(|name| match name {
            "PGDATABASE" => Some(database.into()),
            "PGUSER" => Some("test_user".into()),
            "PGPASSWORD" => Some("session-only-password".into()),
            _ => None,
        })
    }

    fn state(root: PathBuf) -> AppState {
        let defaults = defaults("one");
        let config = defaults.config.as_ref().unwrap().clone();
        let identity = ConnectionIdentity::from_config(&config);
        let storage =
            Arc::new(storage::Storage::new(root.clone(), Some(identity.clone())).unwrap());
        AppState {
            root,
            workspace: Mutex::new(Workspace { storage, defaults }),
            database: database::Database::default(),
            navigation: tokio::sync::Mutex::new(()),
            known_connections: Mutex::new(vec![(identity, config)]),
        }
    }

    #[tokio::test]
    async fn creating_is_empty_preserves_history_and_reuses_current_connection() {
        let root = std::env::temp_dir().join(format!("jupsql-create-{}", uuid::Uuid::new_v4()));
        let state = state(root.clone());
        let current = state.workspace().unwrap();
        let original = current.storage.id().to_owned();
        current
            .storage
            .save(&Notebook {
                title: "Keep me".into(),
                ..Notebook::empty()
            })
            .unwrap();
        drop(current);
        let operation = state.navigation.lock().await;
        assert!(state.navigate(&original, None).await.is_err());
        drop(operation);
        let created = state.navigate(&original, None).await.unwrap();
        assert_ne!(created.notebook_id, original);
        assert!(created.notebook.unwrap().cells.is_empty());
        assert!(created.connection_defaults.auto_connect);
        assert_eq!(created.connection_defaults.input.database, "one");
        assert_eq!(
            created.connection_defaults.input.password,
            "session-only-password"
        );
        assert!(state.navigate(&original, None).await.is_err());
        assert_eq!(storage::Storage::list(&root).unwrap().len(), 2);
        let restored = state.open(&created.notebook_id, &original).await.unwrap();
        assert_eq!(restored.notebook.unwrap().title, "Keep me");
        drop(state);
        // Explicit creation must not resume the now-unlocked recent notebook.
        let identity = ConnectionIdentity::from_config(&defaults("one").config.unwrap());
        let fresh = storage::Storage::create(root.clone(), Some(identity.clone())).unwrap();
        let fresh_id = fresh.id().to_owned();
        assert_ne!(fresh_id, original);
        assert!(fresh.load().unwrap().unwrap().cells.is_empty());
        drop(fresh);
        let resumed = storage::Storage::new(root.clone(), Some(identity)).unwrap();
        assert_eq!(resumed.id(), fresh_id);
        drop(resumed);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn switching_scopes_requests_and_only_reuses_matching_credentials() {
        let root = std::env::temp_dir().join(format!("jupsql-workspace-{}", uuid::Uuid::new_v4()));
        let state = state(root.clone());
        let current = state.workspace().unwrap();
        let original_id = current.storage.id().to_owned();
        current
            .storage
            .save(&Notebook {
                title: "Original".into(),
                ..Notebook::empty()
            })
            .unwrap();
        drop(current);
        let other_config = defaults("two").config.unwrap();
        let other = storage::Storage::new(
            root.clone(),
            Some(ConnectionIdentity::from_config(&other_config)),
        )
        .unwrap();
        let other_id = other.id().to_owned();
        other
            .save(&Notebook {
                title: "Other".into(),
                ..Notebook::empty()
            })
            .unwrap();
        drop(other);
        *state.database.connection.lock().unwrap() = state.workspace().unwrap().defaults.config;

        let selected = state.open(&original_id, &other_id).await.unwrap();
        assert_eq!(selected.notebook.unwrap().title, "Other");
        assert!(
            state.notebook(&original_id).is_err(),
            "late callbacks must not address the new notebook"
        );
        assert!(!selected.connection_defaults.auto_connect);
        assert_eq!(selected.connection_defaults.input.database, "two");
        assert!(selected.connection_defaults.input.password.is_empty());
        assert!(state.database.connection.lock().unwrap().is_none());

        let original = state.open(&other_id, &original_id).await.unwrap();
        assert_eq!(original.notebook.unwrap().title, "Original");
        assert!(original.connection_defaults.auto_connect);
        assert_eq!(
            original.connection_defaults.input.password,
            "session-only-password"
        );
        assert!(
            !std::fs::read_to_string(root.join("catalog.json"))
                .unwrap()
                .contains("session-only-password")
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn busy_notebooks_and_running_operations_leave_the_current_workspace_intact() {
        let root = std::env::temp_dir().join(format!("jupsql-workspace-{}", uuid::Uuid::new_v4()));
        let state = state(root.clone());
        let original = state.workspace().unwrap().storage.id().to_owned();
        let other = storage::Storage::new(root.clone(), None).unwrap();
        let other_id = other.id().to_owned();
        assert!(
            state
                .open(&original, &other_id)
                .await
                .err()
                .unwrap()
                .contains("already open")
        );
        assert_eq!(state.workspace().unwrap().storage.id(), original);
        drop(other);
        let operation = state.navigation.lock().await;
        assert!(
            state
                .open(&original, &other_id)
                .await
                .err()
                .unwrap()
                .contains("current query")
        );
        assert_eq!(state.workspace().unwrap().storage.id(), original);
        drop(operation);
        assert_eq!(
            state.open(&original, &other_id).await.unwrap().notebook_id,
            other_id
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}
