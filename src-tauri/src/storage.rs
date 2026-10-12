use crate::model::{Notebook, now_ms};
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::Mutex,
};

pub struct Storage {
    pub directory: PathBuf,
    pub session_id: String,
    pub log_path: PathBuf,
    log: Mutex<File>,
    save_lock: Mutex<()>,
    _instance_lock: File,
}

fn private_file(path: &std::path::Path, append: bool) -> std::io::Result<File> {
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

impl Storage {
    pub fn new(directory: PathBuf) -> Result<Self, String> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let logs = directory.join("sessions");
        fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
            fs::set_permissions(&logs, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let instance_lock =
            private_file(&directory.join("notebook.lock"), true).map_err(|e| e.to_string())?;
        instance_lock.try_lock().map_err(|e| format!("This notebook is already open in another JupSQL process, or its lock could not be acquired: {e}"))?;
        let log_path = logs.join(format!("{}-{session_id}.jsonl", now_ms()));
        let log = private_file(&log_path, true).map_err(|e| e.to_string())?;
        let store = Self {
            directory,
            session_id,
            log_path,
            log: Mutex::new(log),
            save_lock: Mutex::new(()),
            _instance_lock: instance_lock,
        };
        store.record(
            "session_started",
            json!({ "appVersion": env!("CARGO_PKG_VERSION") }),
        )?;
        Ok(store)
    }

    pub fn record(&self, event: &str, data: Value) -> Result<(), String> {
        let entry = json!({ "version": 1, "sessionId": self.session_id, "at": now_ms(), "event": event, "data": data });
        let mut file = self.log.lock().map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut *file, &entry).map_err(|e| e.to_string())?;
        file.write_all(b"\n")
            .and_then(|_| file.flush())
            .map_err(|e| format!("Session log could not be written: {e}"))
    }

    pub fn load(&self) -> Result<Option<Notebook>, String> {
        let path = self.directory.join("notebook.json");
        if !path.exists() {
            return Ok(None);
        }
        let data = fs::read(&path).map_err(|e| e.to_string())?;
        let book = serde_json::from_slice(&data).map_err(|e| {
            format!(
                "Could not read {}: {e}. The original file has been preserved.",
                path.display()
            )
        })?;
        self.record(
            "notebook_opened",
            serde_json::to_value(&book).map_err(|e| e.to_string())?,
        )?;
        Ok(Some(book))
    }

    pub fn save(&self, notebook: &Notebook) -> Result<(), String> {
        if notebook.version != 1 || notebook.cells.len() > 100 {
            return Err("Unsupported notebook format or more than 100 cells.".into());
        }
        let data = serde_json::to_vec_pretty(notebook).map_err(|e| e.to_string())?;
        if data.len() > 2_000_000 {
            return Err("Notebook exceeds the 2 MB prototype limit.".into());
        }
        let _lock = self.save_lock.lock().map_err(|e| e.to_string())?;
        let temporary = self.directory.join("notebook.next");
        let mut file = private_file(&temporary, false).map_err(|e| e.to_string())?;
        file.write_all(&data)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(temporary, self.directory.join("notebook.json")).map_err(|e| e.to_string())?;
        self.record(
            "notebook_saved",
            serde_json::to_value(notebook).map_err(|e| e.to_string())?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saves_drafts_and_writes_readable_session_events() {
        let path = std::env::temp_dir().join(format!("jupsql-storage-{}", uuid::Uuid::new_v4()));
        let store = Storage::new(path.clone()).unwrap();
        assert!(
            Storage::new(path.clone()).is_err(),
            "A second process must not overwrite this notebook"
        );
        let notebook = Notebook {
            version: 1,
            title: "My investigation".into(),
            cells: vec![],
        };
        store.save(&notebook).unwrap();
        assert_eq!(store.load().unwrap().unwrap().title, notebook.title);
        let lines = fs::read_to_string(&store.log_path).unwrap();
        let events: Vec<Value> = lines
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events[0]["event"], "session_started");
        assert_eq!(events[1]["event"], "notebook_saved");
        fs::remove_dir_all(path).unwrap();
    }
}
