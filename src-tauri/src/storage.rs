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
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            version: 1,
            recent: vec![],
        }
    }
}

impl Catalog {
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
    let temporary = path.with_extension("next");
    let mut file = private_file(&temporary, false).map_err(|error| error.to_string())?;
    file.write_all(&data)
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
        private_directory(&root)?;
        private_directory(&root.join("notebooks"))?;
        private_directory(&root.join("sessions"))?;
        // Hold this only while choosing and reserving a notebook, not for the window's lifetime.
        let catalog_lock = lock_catalog(&root)?;
        let mut catalog = read_catalog(&root)?;
        let recent = connection.as_ref().and_then(|connection| {
            catalog
                .recent
                .iter()
                .filter(|entry| entry.connection == *connection)
                .max_by_key(|entry| entry.opened_at)
        });
        let mut selected = None;
        if let Some(recent) = recent {
            let directory = root.join("notebooks").join(&recent.notebook_id);
            match fs::metadata(directory.join("notebook.json")) {
                Ok(_) => {
                    let lock = private_file(&directory.join("notebook.lock"), true)
                        .map_err(|error| error.to_string())?;
                    match lock.try_lock() {
                        Ok(()) => {
                            read_notebook(&directory)?;
                            selected = Some((recent.notebook_id.clone(), directory, lock));
                        }
                        Err(TryLockError::WouldBlock) => {}
                        Err(TryLockError::Error(error)) => return Err(error.to_string()),
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
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
            write_json(&root.join("catalog.json"), &catalog)?;
        }
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
        write_json(&self.directory.join("notebook.json"), notebook)?;
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
