use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::checkpoint::CheckpointStore;
use super::types::JobState;
use super::{heartbeat_file, list_jobs, locks_dir, stop_signal_file};

const STALE_LOCK_TTL_SECS: u64 = 1800;

#[derive(Debug, Clone, Serialize)]
pub struct HealthSnapshot {
    pub status: String,
    pub queue_depth: u64,
    pub running_jobs: u64,
    pub failed_jobs: u64,
    pub lock_files: u64,
    pub stale_locks: u64,
    pub heartbeat_age_secs: Option<u64>,
    pub checkpoint_age_secs: Option<u64>,
    pub stop_requested: bool,
    pub updated_at: u64,
}

#[derive(Debug, Default)]
pub struct AgentHealth;

impl AgentHealth {
    pub fn snapshot(&self) -> Result<HealthSnapshot, String> {
        let jobs = list_jobs(500)?;

        let mut queue_depth = 0u64;
        let mut running_jobs = 0u64;
        let mut failed_jobs = 0u64;
        for j in jobs {
            match j.state {
                JobState::Queued => queue_depth += 1,
                JobState::Running => running_jobs += 1,
                JobState::Failed => failed_jobs += 1,
                _ => {}
            }
        }

        let mut lock_files = 0u64;
        let mut stale_locks = 0u64;
        let lock_dir = locks_dir()?;
        for entry in fs::read_dir(&lock_dir)
            .map_err(|e| format!("read_dir {}: {}", lock_dir.display(), e))?
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("lock") {
                continue;
            }

            lock_files += 1;
            let meta =
                fs::metadata(&path).map_err(|e| format!("metadata {}: {}", path.display(), e))?;
            let modified = meta
                .modified()
                .map_err(|e| format!("modified {}: {}", path.display(), e))?;
            let age = SystemTime::now()
                .duration_since(modified)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if age >= STALE_LOCK_TTL_SECS {
                stale_locks += 1;
            }
        }

        let heartbeat_age_secs = read_age_from_json_file(heartbeat_file()?, "updated_at")?;
        let checkpoint_age_secs = read_checkpoint_age()?;
        let stop_requested = stop_signal_file()?.exists();

        let status = if stale_locks > 0 || failed_jobs > 0 {
            "degraded"
        } else {
            "ok"
        };

        Ok(HealthSnapshot {
            status: status.to_string(),
            queue_depth,
            running_jobs,
            failed_jobs,
            lock_files,
            stale_locks,
            heartbeat_age_secs,
            checkpoint_age_secs,
            stop_requested,
            updated_at: now_secs(),
        })
    }
}

fn read_age_from_json_file(path: std::path::PathBuf, field: &str) -> Result<Option<u64>, String> {
    if !path.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("invalid json {}: {}", path.display(), e))?;
    let ts = value.get(field).and_then(|v| v.as_u64());
    Ok(ts.map(|v| now_secs().saturating_sub(v)))
}

fn read_checkpoint_age() -> Result<Option<u64>, String> {
    let store = CheckpointStore;
    let Some(snapshot) = store.load()? else {
        return Ok(None);
    };
    Ok(Some(now_secs().saturating_sub(snapshot.updated_at)))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
