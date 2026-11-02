use crate::{
    connection::ConnectionIdentity,
    model::{Notebook, now_ms},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub struct Storage {
    pub directory: PathBuf,
    pub session_id: String,
    pub log_path: PathBuf,
    root: PathBuf,
    notebook_id: String,
    connection: Mutex<Option<ConnectionIdentity>>,
    log: Mutex<File>,
    save_lock: Mutex<()>,
    _instance_lock: File,
}

enum Selection<'a> {
    Recent,
    Open(&'a str),
    New,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecentNotebook {
    notebook_id: String,
    connection: ConnectionIdentity,
    opened_at: u128,
}

#[derive(Deserialize, Serialize)]
struct Catalog {
    version: u32,
    recent: Vec<RecentNotebook>,
    #[serde(default)]
    notebooks: Vec<NotebookEntry>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct NotebookEntry {
    id: String,
    title: String,
    last_activity: u128,
    modified_at: u128,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookSummary {
    pub id: String,
    pub title: String,
    pub last_activity: u128,
    pub is_open: bool,
    pub connection: Option<String>,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            version: 1,
            recent: vec![],
            notebooks: vec![],
        }
    }
}

impl Catalog {
    fn activity(&mut self, id: &str, title: &str, modified_at: u128) {
        let last_activity = now_ms().max(
            self.notebooks
                .iter()
                .map(|entry| entry.last_activity)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
        self.notebooks.retain(|entry| entry.id != id);
        self.notebooks.push(NotebookEntry {
            id: id.into(),
            title: title.into(),
            last_activity,
            modified_at,
        });
    }

    fn connection(&self, id: &str) -> Option<ConnectionIdentity> {
        self.recent
            .iter()
            .filter(|entry| entry.notebook_id == id)
            .max_by_key(|entry| entry.opened_at)
            .map(|entry| entry.connection.clone())
    }

    fn remember(&mut self, notebook_id: &str, connection: &ConnectionIdentity) {
        // Serialize recency even when several processes start in the same millisecond.
        let opened_at = now_ms().max(
            self.recent
                .iter()
                .map(|entry| entry.opened_at)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        );
        self.recent
            .retain(|entry| entry.notebook_id != notebook_id || entry.connection != *connection);
        self.recent.push(RecentNotebook {
            notebook_id: notebook_id.into(),
            connection: connection.clone(),
            opened_at,
        });
    }
}

fn private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn private_file(path: &Path, append: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    write_atomic(path, &data)
}

fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("next");
    let mut file = private_file(&temporary, false).map_err(|error| error.to_string())?;
    file.write_all(data)
        .and_then(|_| file.sync_all())
        .map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

fn lock_catalog(root: &Path) -> Result<File, String> {
    let lock = private_file(&root.join("catalog.lock"), true).map_err(|error| error.to_string())?;
    lock.lock().map_err(|error| error.to_string())?;
    Ok(lock)
}

fn read_catalog(root: &Path) -> Result<Catalog, String> {
    let path = root.join("catalog.json");
    let data = match fs::read(&path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Catalog::default()),
        Err(error) => return Err(error.to_string()),
    };
    let catalog: Catalog = serde_json::from_slice(&data).map_err(|error| {
        format!(
            "Could not read {}: {error}. The original file has been preserved.",
            path.display()
        )
    })?;
    if catalog.version != 1
        || catalog
            .notebooks
            .iter()
            .any(|entry| uuid::Uuid::parse_str(&entry.id).is_err())
        || catalog
            .recent
            .iter()
            .any(|entry| uuid::Uuid::parse_str(&entry.notebook_id).is_err())
    {
        return Err(
            "Unsupported or invalid notebook catalog. The original file has been preserved.".into(),
        );
    }
    Ok(catalog)
}

fn modified_at(path: &Path) -> Result<u128, String> {
    Ok(fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_err(|error| error.to_string())?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos())
}

fn read_notebook(directory: &Path) -> Result<Notebook, String> {
    let path = directory.join("notebook.json");
    let data = fs::read(&path).map_err(|error| error.to_string())?;
    let notebook: Notebook = serde_json::from_slice(&data).map_err(|error| {
        format!(
            "Could not read {}: {error}. The original file has been preserved.",
            path.display()
        )
    })?;
    if notebook.version != 1 || notebook.cells.len() > 100 {
        return Err("Unsupported notebook format or more than 100 cells. The original file has been preserved.".into());
    }
    Ok(notebook)
}

impl Storage {
    pub fn new(root: PathBuf, connection: Option<ConnectionIdentity>) -> Result<Self, String> {
        Self::select(root, connection, Selection::Recent)
    }

    pub fn create(root: PathBuf, connection: Option<ConnectionIdentity>) -> Result<Self, String> {
        Self::select(root, connection, Selection::New)
    }

    pub fn open(root: PathBuf, id: &str) -> Result<Self, String> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err("Invalid notebook ID.".into());
        }
        Self::select(root, None, Selection::Open(id))
    }

    fn select(
        root: PathBuf,
        connection: Option<ConnectionIdentity>,
        selection: Selection<'_>,
    ) -> Result<Self, String> {
        private_directory(&root)?;
        private_directory(&root.join("notebooks"))?;
        private_directory(&root.join("sessions"))?;
        // Hold this only while choosing and reserving a notebook, not for the window's lifetime.
        let catalog_lock = lock_catalog(&root)?;
        let mut catalog = read_catalog(&root)?;
        let requested = match selection {
            Selection::Open(id) => Some(id),
            _ => None,
        };
        let connection = requested.map_or(connection, |id| catalog.connection(id));
        let recent = connection.as_ref().and_then(|connection| {
            catalog
                .recent
                .iter()
                .filter(|entry| entry.connection == *connection)
                .max_by_key(|entry| entry.opened_at)
        });
        let mut selected = None;
        let selected_id = match selection {
            Selection::Open(id) => Some(id.to_owned()),
            Selection::Recent => recent.map(|entry| entry.notebook_id.clone()),
            Selection::New => None,
        };
        if let Some(id) = selected_id {
            let directory = root.join("notebooks").join(&id);
            match fs::metadata(directory.join("notebook.json")) {
                Ok(_) => {
                    let lock = private_file(&directory.join("notebook.lock"), true)
                        .map_err(|error| error.to_string())?;
                    match lock.try_lock() {
                        Ok(()) => {
                            read_notebook(&directory)?;
                            selected = Some((id, directory, lock));
                        }
                        Err(TryLockError::WouldBlock) if requested.is_some() => {
                            return Err("This notebook is already open in another window.".into());
                        }
                        Err(TryLockError::WouldBlock) => {}
                        Err(TryLockError::Error(error)) => return Err(error.to_string()),
                    }
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound && requested.is_none() => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        let resumed = selected.is_some();
        let (notebook_id, directory, instance_lock) = match selected {
            Some(selected) => selected,
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                let directory = root.join("notebooks").join(&id);
                private_directory(&directory)?;
                let lock = private_file(&directory.join("notebook.lock"), true)
                    .map_err(|error| error.to_string())?;
                lock.lock().map_err(|error| error.to_string())?;
                // Persist immediately, before another process can observe the catalog entry.
                write_json(&directory.join("notebook.json"), &Notebook::empty())?;
                (id, directory, lock)
            }
        };
        if let Some(connection) = &connection {
            catalog.remember(&notebook_id, connection);
        }
        let notebook = read_notebook(&directory)?;
        let modified = modified_at(&directory.join("notebook.json"))?;
        if !resumed {
            catalog.activity(&notebook_id, &notebook.title, modified);
        } else if !catalog
            .notebooks
            .iter()
            .any(|entry| entry.id == notebook_id)
        {
            catalog.notebooks.push(NotebookEntry {
                id: notebook_id.clone(),
                title: notebook.title,
                last_activity: modified / 1_000_000,
                modified_at: modified,
            });
        }
        write_json(&root.join("catalog.json"), &catalog)?;
        let session_id = uuid::Uuid::new_v4().to_string();
        let log_path = root
            .join("sessions")
            .join(format!("{}-{session_id}.jsonl", now_ms()));
        let log = private_file(&log_path, true).map_err(|error| error.to_string())?;
        let store = Self {
            directory,
            root,
            notebook_id,
            connection: Mutex::new(connection),
            session_id,
            log_path,
            log: Mutex::new(log),
            save_lock: Mutex::new(()),
            _instance_lock: instance_lock,
        };
        store.record("session_started", json!({ "appVersion": env!("CARGO_PKG_VERSION"), "notebookId": store.notebook_id, "resumed": resumed }))?;
        drop(catalog_lock);
        Ok(store)
    }

    pub fn id(&self) -> &str {
        &self.notebook_id
    }

    pub fn connection_identity(&self) -> Result<Option<ConnectionIdentity>, String> {
        self.connection
            .lock()
            .map(|value| value.clone())
            .map_err(|error| error.to_string())
    }

    pub fn list(root: &Path) -> Result<Vec<NotebookSummary>, String> {
        let _catalog_lock = lock_catalog(root)?;
        let mut catalog = read_catalog(root)?;
        let mut changed = false;
        let mut notebooks = vec![];
        // Discover unassociated notebooks and older catalogs, caching titles until files change.
        for directory in fs::read_dir(root.join("notebooks")).map_err(|error| error.to_string())? {
            let directory = directory.map_err(|error| error.to_string())?;
            let id = directory.file_name().to_string_lossy().into_owned();
            if uuid::Uuid::parse_str(&id).is_err() {
                continue;
            }
            let path = directory.path();
            if !path.join("notebook.json").is_file() {
                continue;
            }
            let modified = modified_at(&path.join("notebook.json"))?;
            let entry = catalog.notebooks.iter().find(|entry| entry.id == id);
            if entry.is_none_or(|entry| entry.modified_at != modified) {
                let book = read_notebook(&path)?;
                let last_activity = entry
                    .map_or(0, |entry| entry.last_activity)
                    .max(modified / 1_000_000);
                catalog.notebooks.retain(|entry| entry.id != id);
                catalog.notebooks.push(NotebookEntry {
                    id: id.clone(),
                    title: book.title,
                    last_activity,
                    modified_at: modified,
                });
                changed = true;
            }
            let lock = private_file(&path.join("notebook.lock"), true)
                .map_err(|error| error.to_string())?;
            let is_open = match lock.try_lock() {
                Ok(()) => false,
                Err(TryLockError::WouldBlock) => true,
                Err(TryLockError::Error(error)) => return Err(error.to_string()),
            };
            let entry = catalog
                .notebooks
                .iter()
                .find(|entry| entry.id == id)
                .unwrap();
            notebooks.push(NotebookSummary {
                id: id.clone(),
                title: entry.title.clone(),
                last_activity: entry.last_activity,
                is_open,
                connection: catalog.connection(&id).map(|identity| identity.label()),
            });
        }
        if changed {
            write_json(&root.join("catalog.json"), &catalog)?;
        }
        notebooks.sort_by(|a, b| {
            b.last_activity
                .cmp(&a.last_activity)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(notebooks)
    }

    pub fn mark_activity(&self) -> Result<(), String> {
        let _catalog_lock = lock_catalog(&self.root)?;
        let mut catalog = read_catalog(&self.root)?;
        let book = read_notebook(&self.directory)?;
        catalog.activity(
            &self.notebook_id,
            &book.title,
            modified_at(&self.directory.join("notebook.json"))?,
        );
        write_json(&self.root.join("catalog.json"), &catalog)
    }

    pub fn associate_connection(&self, connection: ConnectionIdentity) -> Result<(), String> {
        let mut current = self.connection.lock().map_err(|error| error.to_string())?;
        if current.as_ref() == Some(&connection) {
            return Ok(());
        }
        let _catalog_lock = lock_catalog(&self.root)?;
        let mut catalog = read_catalog(&self.root)?;
        catalog.remember(&self.notebook_id, &connection);
        write_json(&self.root.join("catalog.json"), &catalog)?;
        *current = Some(connection);
        Ok(())
    }

    pub fn record(&self, event: &str, data: Value) -> Result<(), String> {
        let entry = json!({ "version": 1, "sessionId": self.session_id, "at": now_ms(), "event": event, "data": data });
        let mut file = self.log.lock().map_err(|error| error.to_string())?;
        serde_json::to_writer(&mut *file, &entry).map_err(|error| error.to_string())?;
        file.write_all(b"\n")
            .and_then(|_| file.flush())
            .map_err(|error| format!("Session log could not be written: {error}"))
    }

    pub fn load(&self) -> Result<Option<Notebook>, String> {
        let book = read_notebook(&self.directory)?;
        self.record(
            "notebook_opened",
            serde_json::to_value(&book).map_err(|error| error.to_string())?,
        )?;
        Ok(Some(book))
    }

    pub fn save(&self, notebook: &Notebook) -> Result<(), String> {
        if notebook.version != 1 || notebook.cells.len() > 100 {
            return Err("Unsupported notebook format or more than 100 cells.".into());
        }
        let data = serde_json::to_vec_pretty(notebook).map_err(|error| error.to_string())?;
        if data.len() > 2_000_000 {
            return Err("Notebook exceeds the 2 MB prototype limit.".into());
        }
        let _lock = self.save_lock.lock().map_err(|error| error.to_string())?;
        let path = self.directory.join("notebook.json");
        if fs::read(&path).map_err(|error| error.to_string())? != data {
            write_atomic(&path, &data)?;
            self.mark_activity()?;
        }
        self.record(
            "notebook_saved",
            serde_json::to_value(notebook).map_err(|error| error.to_string())?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("jupsql-storage-{}", uuid::Uuid::new_v4()))
    }

    fn connection(database: &str) -> ConnectionIdentity {
        let mut config = tokio_postgres::Config::new();
        config
            .host("localhost")
            .port(5432)
            .user("test_user")
            .dbname(database)
            .password("never-store-this-password");
        ConnectionIdentity::from_config(&config)
    }

    fn save_title(store: &Storage, title: &str) {
        store
            .save(&Notebook {
                title: title.into(),
                ..Notebook::empty()
            })
            .unwrap();
    }

    #[test]
    fn adding_and_deleting_queries_count_as_activity_but_reading_does_not() {
        let root = root();
        let store = Storage::new(root.clone(), None).unwrap();
        let mut notebook = store.load().unwrap().unwrap();
        let initial = Storage::list(&root).unwrap()[0].last_activity;
        notebook.cells.push(crate::model::Cell {
            id: uuid::Uuid::new_v4().to_string(),
            name: "query_1".into(),
            kind: crate::model::CellKind::Query,
            sql: String::new(),
            notes: String::new(),
        });
        store.save(&notebook).unwrap();
        let added = Storage::list(&root).unwrap()[0].last_activity;
        assert!(added > initial, "adding even an empty query is activity");
        notebook.cells.clear();
        store.save(&notebook).unwrap();
        let deleted = Storage::list(&root).unwrap()[0].last_activity;
        assert!(deleted > added, "deleting the last query is activity");
        let unchanged = store.load().unwrap().unwrap();
        store.save(&unchanged).unwrap();
        assert_eq!(Storage::list(&root).unwrap()[0].last_activity, deleted);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lists_all_notebooks_by_activity_without_changing_startup_recency() {
        let root = root();
        let first = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        save_title(&first, "First");
        let second = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        save_title(&second, "Second");
        let second_id = second.id().to_owned();
        let listed = Storage::list(&root).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].title, "Second");
        assert!(listed.iter().all(|entry| entry.is_open));
        first.save(&first.load().unwrap().unwrap()).unwrap();
        assert_eq!(
            Storage::list(&root).unwrap()[0].id,
            second_id,
            "unchanged autosaves must not reorder notebooks"
        );
        first.mark_activity().unwrap();
        assert_eq!(Storage::list(&root).unwrap()[0].title, "First");
        save_title(&first, "Renamed first");
        assert_eq!(Storage::list(&root).unwrap()[0].title, "Renamed first");
        let activity_before = Storage::list(&root).unwrap()[1].last_activity;
        drop(second);
        let reopened = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        assert_eq!(
            reopened.id(),
            second_id,
            "startup uses last opened, not last activity"
        );
        let listed = Storage::list(&root).unwrap();
        assert_eq!(
            listed[0].title, "Renamed first",
            "opening must not reorder notebooks"
        );
        assert_eq!(listed[1].last_activity, activity_before);
        drop(first);
        let opened = Storage::open(root.clone(), &listed[0].id).unwrap();
        assert_eq!(
            Storage::list(&root).unwrap()[0].last_activity,
            listed[0].last_activity
        );
        drop((opened, reopened));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovers_unassociated_notebooks_from_older_catalogs() {
        let root = root();
        let store = Storage::new(root.clone(), None).unwrap();
        save_title(&store, "Recovered history");
        let id = store.id().to_owned();
        drop(store);
        fs::write(root.join("catalog.json"), r#"{"version":1,"recent":[]}"#).unwrap();
        let listed = Storage::list(&root).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert_eq!(listed[0].title, "Recovered history");
        assert!(!listed[0].is_open);
        assert!(listed[0].connection.is_none());
        assert_eq!(read_catalog(&root).unwrap().notebooks.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_open_uses_the_selected_notebook_and_rejects_busy_or_invalid_ids() {
        let root = root();
        let first = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        save_title(&first, "Selected");
        let second = Storage::new(root.clone(), Some(connection("two"))).unwrap();
        let id = first.id().to_owned();
        assert!(Storage::open(root.clone(), &id).is_err());
        assert!(Storage::open(root.clone(), "../notebook").is_err());
        assert!(Storage::open(root.clone(), &uuid::Uuid::new_v4().to_string()).is_err());
        assert_eq!(Storage::list(&root).unwrap().len(), 2);
        drop(first);
        let selected = Storage::open(root.clone(), &id).unwrap();
        assert_eq!(selected.load().unwrap().unwrap().title, "Selected");
        assert_eq!(
            selected.connection_identity().unwrap(),
            Some(connection("one"))
        );
        drop((selected, second));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reopens_the_most_recent_notebook_for_a_connection() {
        let root = root();
        let first = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        save_title(&first, "Saved investigation");
        let directory = first.directory.clone();
        let lines = fs::read_to_string(&first.log_path).unwrap();
        let events: Vec<Value> = lines
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events[0]["event"], "session_started");
        assert_eq!(events[1]["event"], "notebook_saved");
        drop(first);
        let reopened = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        assert_eq!(reopened.directory, directory);
        assert_eq!(
            reopened.load().unwrap().unwrap().title,
            "Saved investigation"
        );
        let catalog = fs::read_to_string(root.join("catalog.json")).unwrap();
        assert!(!catalog.contains("never-store-this-password"));
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn busy_most_recent_creates_empty_instead_of_opening_an_older_notebook() {
        let root = root();
        let older = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        save_title(&older, "Older");
        let recent = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        save_title(&recent, "Most recent");
        let recent_directory = recent.directory.clone();
        let older_directory = older.directory.clone();
        drop(older);
        drop(recent);
        let resumed = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        assert_eq!(resumed.directory, recent_directory);
        let fresh = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        assert_ne!(fresh.directory, recent_directory);
        assert_ne!(fresh.directory, older_directory);
        assert!(fresh.load().unwrap().unwrap().cells.is_empty());
        assert_eq!(fresh.load().unwrap().unwrap().title, "Untitled notebook");
        assert_eq!(resumed.load().unwrap().unwrap().title, "Most recent");
        drop((fresh, resumed));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn connection_history_is_separate_and_saving_does_not_change_open_recency() {
        let root = root();
        let first = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        let recent = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        let recent_directory = recent.directory.clone();
        save_title(&first, "Saved after the newer notebook opened");
        first.associate_connection(connection("one")).unwrap();
        let other = Storage::new(root.clone(), Some(connection("two"))).unwrap();
        save_title(&other, "Other database");
        let other_directory = other.directory.clone();
        drop((first, recent, other));
        let one = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        let two = Storage::new(root.clone(), Some(connection("two"))).unwrap();
        assert_eq!(one.directory, recent_directory);
        assert_eq!(two.directory, other_directory);
        assert_eq!(two.load().unwrap().unwrap().title, "Other database");
        drop((one, two));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_startup_settings_create_fresh_and_manual_connections_register_history() {
        let root = root();
        let first = Storage::new(root.clone(), None).unwrap();
        save_title(&first, "Connected manually");
        first.associate_connection(connection("one")).unwrap();
        first.associate_connection(connection("two")).unwrap();
        let directory = first.directory.clone();
        drop(first);
        let unassociated = Storage::new(root.clone(), None).unwrap();
        assert_ne!(unassociated.directory, directory);
        drop(unassociated);
        let one = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        assert_eq!(one.directory, directory);
        drop(one);
        let two = Storage::new(root.clone(), Some(connection("two"))).unwrap();
        assert_eq!(two.directory, directory);
        drop(two);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_shared_notebooks_and_their_locks_are_preserved() {
        let root = root();
        private_directory(&root).unwrap();
        let legacy = root.join("notebook.json");
        fs::write(&legacy, "old notebook that has no connection metadata").unwrap();
        let lock = private_file(&root.join("notebook.lock"), true).unwrap();
        lock.lock().unwrap();
        let fresh = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        assert!(fresh.load().unwrap().unwrap().cells.is_empty());
        assert_eq!(
            fs::read_to_string(legacy).unwrap(),
            "old notebook that has no connection metadata"
        );
        drop((fresh, lock));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_starts_reserve_distinct_notebooks_without_lost_history() {
        let root = root();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let root = root.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let store = Storage::new(root, Some(connection("one"))).unwrap();
                    barrier.wait();
                    store
                })
            })
            .collect();
        let stores: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        let directories: std::collections::HashSet<_> =
            stores.iter().map(|store| &store.directory).collect();
        assert_eq!(directories.len(), 4);
        assert_eq!(read_catalog(&root).unwrap().recent.len(), 4);
        drop(stores);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_catalog_and_notebooks_are_not_silently_replaced() {
        let root = root();
        let store = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        let notebook = store.directory.join("notebook.json");
        drop(store);
        fs::write(&notebook, "broken notebook").unwrap();
        assert!(Storage::new(root.clone(), Some(connection("one"))).is_err());
        assert_eq!(fs::read_to_string(notebook).unwrap(), "broken notebook");
        fs::write(root.join("catalog.json"), "broken catalog").unwrap();
        assert!(Storage::new(root.clone(), Some(connection("two"))).is_err());
        assert_eq!(
            fs::read_to_string(root.join("catalog.json")).unwrap(),
            "broken catalog"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn new_catalogs_notebooks_and_logs_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = root();
        let store = Storage::new(root.clone(), Some(connection("one"))).unwrap();
        for directory in [
            &root,
            &root.join("notebooks"),
            &root.join("sessions"),
            &store.directory,
        ] {
            assert_eq!(
                fs::metadata(directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        for file in [
            root.join("catalog.json"),
            root.join("catalog.lock"),
            store.directory.join("notebook.json"),
            store.directory.join("notebook.lock"),
            store.log_path.clone(),
        ] {
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
