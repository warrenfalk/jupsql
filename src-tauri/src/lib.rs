pub mod compiler;
pub mod database;
pub mod model;
pub mod storage;

use model::{Analysis, ConnectionInput, Notebook, RunResult};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::DialogExt;

struct AppState {
    storage: storage::Storage,
    database: database::Database,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    notebook: Option<Notebook>,
    session_id: String,
    log_path: String,
    data_directory: String,
}

#[tauri::command]
fn bootstrap(state: State<AppState>) -> Result<Bootstrap, String> {
    Ok(Bootstrap {
        notebook: state.storage.load()?,
        session_id: state.storage.session_id.clone(),
        log_path: state.storage.log_path.display().to_string(),
        data_directory: state.storage.directory.display().to_string(),
    })
}

#[tauri::command]
fn analyze_notebook(notebook: Notebook) -> Vec<Analysis> {
    compiler::analyze(&notebook)
}

#[tauri::command]
fn save_notebook(state: State<AppState>, notebook: Notebook) -> Result<(), String> {
    state.storage.save(&notebook)
}

#[tauri::command]
async fn connect_database(
    state: State<'_, AppState>,
    input: ConnectionInput,
) -> Result<String, String> {
    if state
        .database
        .active
        .lock()
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err("Wait for the current run before changing connections.".into());
    }
    let result = database::test_connection(&input).await;
    state.storage.record("connection_attempt", json!({ "host": input.host, "port": input.port, "database": input.database, "user": input.user, "tls": input.tls, "error": result.as_ref().err() }))?;
    let label = result?;
    *state
        .database
        .connection
        .lock()
        .map_err(|e| e.to_string())? = Some(input);
    Ok(label)
}

#[tauri::command]
async fn run_cells(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: database::RunRequest,
) -> Result<RunResult, String> {
    database::execute(&state.database, &state.storage, request, |progress| {
        let _ = app.emit("run-progress", progress);
    })
    .await
}

#[tauri::command]
async fn cancel_run(state: State<'_, AppState>) -> Result<(), String> {
    state.storage.record("cancel_requested", json!({}))?;
    database::cancel(&state.database).await
}

#[tauri::command]
fn record_event(state: State<AppState>, event: String, data: Value) -> Result<(), String> {
    state.storage.record(&format!("ui.{event}"), data)
}

#[tauri::command]
fn read_session_log(state: State<AppState>) -> Result<String, String> {
    std::fs::read_to_string(&state.storage.log_path).map_err(|e| e.to_string())
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
            analyze_notebook,
            save_notebook,
            connect_database,
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
            let storage = storage::Storage::new(directory).map_err(std::io::Error::other)?;
            app.manage(AppState {
                storage,
                database: database::Database::default(),
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
                let _ = state.storage.record("window_closed", json!({}));
            }
        })
        .run(tauri::generate_context!())
        .expect("Could not start JupSQL");
}
