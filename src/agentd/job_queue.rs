use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

use serde_json::json;

use super::types::{JobRecord, JobResult, JobState};
use super::{epoch_seconds, get_job, list_jobs, locks_dir, save_job_record};

const LOCK_TTL_SECS: u64 = 1800;

#[derive(Debug, Default)]
pub struct JobQueue;

impl JobQueue {
    pub fn claim_next(&self) -> Result<Option<JobRecord>, String> {
        let mut jobs = list_jobs(500)?;
        jobs.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        for candidate in jobs {
            if !matches!(candidate.state, JobState::Queued) {
                continue;
            }

            if !try_acquire_lock(&candidate.id)? {
                continue;
            }

            let mut latest = match get_job(&candidate.id) {
                Ok(v) => v,
                Err(_) => {
                    let _ = release_lock(&candidate.id);
                    continue;
                }
            };

            if !matches!(latest.state, JobState::Queued) {
                let _ = release_lock(&candidate.id);
                continue;
            }

            latest.state = JobState::Running;
            latest.updated_at = epoch_seconds();
            latest.error = None;
            save_job_record(&latest)?;
            return Ok(Some(latest));
        }

        Ok(None)
    }

    pub fn finish_success(
        &self,
        mut job: JobRecord,
        result: JobResult,
    ) -> Result<JobRecord, String> {
        job.state = JobState::Succeeded;
        job.updated_at = epoch_seconds();
        job.result = Some(result);
        job.error = None;
        save_job_record(&job)?;
        let _ = release_lock(&job.id);
        Ok(job)
    }

    pub fn finish_failed(
        &self,
        mut job: JobRecord,
        err: impl Into<String>,
    ) -> Result<JobRecord, String> {
        job.state = JobState::Failed;
        job.updated_at = epoch_seconds();
        job.error = Some(err.into());
        save_job_record(&job)?;
        let _ = release_lock(&job.id);
        Ok(job)
    }
}

fn try_acquire_lock(job_id: &str) -> Result<bool, String> {
    let lock_path = lock_file(job_id)?;

    if lock_path.exists() && is_stale_lock(&lock_path)? {
        let _ = fs::remove_file(&lock_path);
    }

    let mut file = match OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&lock_path)
    {
        Ok(f) => f,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                return Ok(false);
            }
            return Err(format!("acquire lock {}: {}", lock_path.display(), e));
        }
    };

    let owner = std::process::id();
    let now = epoch_seconds();
    let payload = json!({"job_id": job_id, "pid": owner, "claimed_at": now});
    let text = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("write lock {}: {}", lock_path.display(), e))?;

    Ok(true)
}

fn release_lock(job_id: &str) -> Result<(), String> {
    let lock_path = lock_file(job_id)?;
    if !lock_path.exists() {
        return Ok(());
    }
    fs::remove_file(&lock_path).map_err(|e| format!("remove lock {}: {}", lock_path.display(), e))
}

fn is_stale_lock(path: &PathBuf) -> Result<bool, String> {
    let meta =
        fs::metadata(path).map_err(|e| format!("lock metadata {}: {}", path.display(), e))?;
    let modified = meta
        .modified()
        .map_err(|e| format!("lock modified {}: {}", path.display(), e))?;
    let age = SystemTime::now()
        .duration_since(modified)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(age >= LOCK_TTL_SECS)
}

fn lock_file(job_id: &str) -> Result<PathBuf, String> {
    Ok(locks_dir()?.join(format!("{}.lock", sanitize(job_id))))
}

fn sanitize(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        }
    }
    if out.is_empty() {
        "job".to_string()
    } else {
        out
    }
}
