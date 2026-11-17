use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

const HISTORY_LIMIT: usize = 100;

pub struct GraphHistory {
    present: String,
    undo: VecDeque<String>,
    redo: Vec<String>,
    group: Option<u64>,
}

impl GraphHistory {
    pub fn new(snapshot: String) -> Self {
        Self {
            present: snapshot,
            undo: VecDeque::new(),
            redo: Vec::new(),
            group: None,
        }
    }

    pub fn record(&mut self, snapshot: String, group: Option<u64>) -> bool {
        let changed = snapshot != self.present;
        if changed {
            if group.is_none() || group != self.group {
                self.undo.push_back(self.present.clone());
                if self.undo.len() > HISTORY_LIMIT {
                    self.undo.pop_front();
                }
            }
            self.present = snapshot;
            self.redo.clear();
        }
        self.group = group;
        changed
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> Option<String> {
        let previous = self.undo.pop_back()?;
        self.redo
            .push(std::mem::replace(&mut self.present, previous));
        self.group = None;
        Some(self.present.clone())
    }

    pub fn redo(&mut self) -> Option<String> {
        let next = self.redo.pop()?;
        self.undo
            .push_back(std::mem::replace(&mut self.present, next));
        self.group = None;
        Some(self.present.clone())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Recovery {
    pub graph: String,
    pub file: Option<PathBuf>,
}

pub struct Workspace {
    directory: PathBuf,
    pub recent: Vec<PathBuf>,
    _lock: std::fs::File,
}

impl Workspace {
    pub fn open() -> Result<Self, String> {
        let dirs = directories::ProjectDirs::from("", "", "ScreenAutomation")
            .ok_or("Application data directory is unavailable")?;
        Self::at(dirs.data_local_dir().to_path_buf())
    }

    pub fn at(directory: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("workspace.lock"))
            .map_err(|error| format!("Could not open workspace lock: {error}"))?;
        lock.try_lock().map_err(|error| {
            format!("Recovery storage is in use by another instance or cannot be locked: {error}")
        })?;
        Ok(Self {
            directory,
            recent: Vec::new(),
            _lock: lock,
        })
    }

    pub fn load_recent(&mut self) -> Result<(), String> {
        let recent = match std::fs::read_to_string(self.directory.join("recent.json")) {
            Ok(contents) => serde_json::from_str(&contents)
                .map_err(|error| format!("Invalid recent-file list: {error}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(format!("Could not read recent files: {error}")),
        };
        self.recent = recent;
        Ok(())
    }

    fn write(&self, name: &str, contents: &str) -> Result<(), String> {
        let path = self.directory.join(name);
        let temporary = self.directory.join(format!("{name}.tmp"));
        std::fs::write(&temporary, contents)
            .map_err(|error| format!("Could not write {}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, &path)
            .map_err(|error| format!("Could not replace {}: {error}", path.display()))
    }

    pub fn remember(&mut self, path: &Path) -> Result<(), String> {
        let path = std::fs::canonicalize(path)
            .map_err(|error| format!("Could not resolve {}: {error}", path.display()))?;
        self.recent.retain(|entry| entry != &path);
        self.recent.insert(0, path);
        self.recent.truncate(10);
        self.write(
            "recent.json",
            &serde_json::to_string(&self.recent).map_err(|error| error.to_string())?,
        )
    }

    pub fn recovery(&self) -> Result<Option<Recovery>, String> {
        match std::fs::read_to_string(self.directory.join("recovery.json")) {
            Ok(contents) => serde_json::from_str(&contents)
                .map(Some)
                .map_err(|error| format!("Invalid recovery copy: {error}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("Could not read recovery copy: {error}")),
        }
    }

    pub fn save_recovery(&self, recovery: &Recovery) -> Result<(), String> {
        self.write(
            "recovery.json",
            &serde_json::to_string(recovery).map_err(|error| error.to_string())?,
        )
    }

    pub fn clear_recovery(&self) -> Result<(), String> {
        match std::fs::remove_file(self.directory.join("recovery.json")) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Could not remove recovery copy: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_groups_gestures_and_discards_redo_after_new_edits() {
        let mut history = GraphHistory::new("initial".into());
        history.record("drag-start".into(), Some(1));
        history.record("drag-middle".into(), Some(1));
        history.record("drag-end".into(), Some(1));
        history.record("drag-end".into(), None);
        history.record("typed".into(), Some(2));
        assert_eq!(history.undo().as_deref(), Some("drag-end"));
        assert_eq!(history.undo().as_deref(), Some("initial"));
        assert_eq!(history.redo().as_deref(), Some("drag-end"));
        history.record("new edit".into(), None);
        assert!(!history.can_redo());
        assert_eq!(history.undo().as_deref(), Some("drag-end"));
    }

    #[test]
    fn recovery_and_recents_survive_reopening_without_touching_workflow() {
        let directory = std::env::temp_dir().join(format!("sa-workspace-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let workflow = directory.join("workflow.json");
        std::fs::write(&workflow, "manually saved").unwrap();
        let mut workspace = Workspace::at(directory.clone()).unwrap();
        workspace.remember(&workflow).unwrap();
        workspace.remember(&workflow).unwrap();
        workspace
            .save_recovery(&Recovery {
                graph: "unsaved recovery".into(),
                file: Some(workflow.clone()),
            })
            .unwrap();
        workspace
            .save_recovery(&Recovery {
                graph: "latest recovery".into(),
                file: Some(workflow.clone()),
            })
            .unwrap();
        assert!(Workspace::at(directory.clone()).is_err());
        drop(workspace);
        let mut restored = Workspace::at(directory.clone()).unwrap();
        restored.load_recent().unwrap();
        assert_eq!(restored.recent.len(), 1);
        assert_eq!(
            restored.recovery().unwrap().unwrap().graph,
            "latest recovery"
        );
        assert_eq!(
            std::fs::read_to_string(&workflow).unwrap(),
            "manually saved"
        );
        std::fs::write(directory.join("recent.json"), "invalid JSON").unwrap();
        assert!(restored.load_recent().is_err());
        assert!(restored.recovery().unwrap().is_some());
        restored.clear_recovery().unwrap();
        assert!(restored.recovery().unwrap().is_none());
        drop(restored);
        for name in ["recent.json", "workflow.json", "workspace.lock"] {
            std::fs::remove_file(directory.join(name)).unwrap();
        }
        std::fs::remove_dir(directory).unwrap();
    }
}
