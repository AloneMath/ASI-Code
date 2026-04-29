use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSnapshot {
    pub pid: u32,
    pub state: String,
    pub updated_at: u64,
    pub loops: usize,
    pub max_loops: usize,
    pub interval_ms: u64,
    pub stop_when_idle: bool,
    pub max_jobs: Option<usize>,
    pub processed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub pulled_remote_jobs: usize,
    pub pulled_remote_duplicates: usize,
    pub pulled_remote_invalid: usize,
    pub pushed_outbox_sent: usize,
    pub pushed_outbox_failed: usize,
    pub pushed_outbox_skipped: usize,
    pub completion_events: usize,
    pub idle_loops: usize,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CheckpointStore;

impl CheckpointStore {
    pub fn save(&self, snapshot: &CheckpointSnapshot) -> Result<(), String> {
        let path = checkpoint_file()?;
        let body = serde_json::to_string_pretty(snapshot).map_err(|e| e.to_string())?;
        fs::write(&path, body).map_err(|e| format!("write {}: {}", path.display(), e))
    }

    pub fn load(&self) -> Result<Option<CheckpointSnapshot>, String> {
        let path = checkpoint_file()?;
        if !path.exists() {
            return Ok(None);
        }

        let text =
            fs::read_to_string(&path).map_err(|e| format!("read {}: {}", path.display(), e))?;
        let snapshot: CheckpointSnapshot = serde_json::from_str(&text)
            .map_err(|e| format!("invalid checkpoint {}: {}", path.display(), e))?;
        Ok(Some(snapshot))
    }
}

fn checkpoint_file() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let root = cwd.join(".asi").join("agentd");
    fs::create_dir_all(&root).map_err(|e| format!("create_dir_all {}: {}", root.display(), e))?;
    Ok(root.join("checkpoint.json"))
}
