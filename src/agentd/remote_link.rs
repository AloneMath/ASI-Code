use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::types::JobRecord;
use super::{epoch_seconds, submit_job_from_spec_file};

const DEFAULT_RETRY_MAX_ATTEMPTS: u32 = 5;
const DEFAULT_RETRY_BASE_DELAY_SECS: u64 = 5;
const DEFAULT_RETRY_MAX_DELAY_SECS: u64 = 300;

#[derive(Debug, Default, Clone, Copy)]
pub struct PullReport {
    pub accepted: usize,
    pub duplicates: usize,
    pub invalid: usize,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct OutboxPushReport {
    pub sent: usize,
    pub failed: usize,
    pub skipped: usize,
}

#[derive(Debug, Default)]
pub struct RemoteLink;

#[derive(Debug, Clone, Copy)]
struct RetryConfig {
    max_attempts: u32,
    base_delay_secs: u64,
    max_delay_secs: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct RetryState {
    attempts: u32,
    next_attempt_at: u64,
    last_attempt_at: u64,
    last_error: String,
}

impl RetryConfig {
    fn from_env() -> Self {
        let max_attempts = env_u32(
            "ASI_AGENTD_OUTBOX_RETRY_MAX_ATTEMPTS",
            DEFAULT_RETRY_MAX_ATTEMPTS,
        )
        .clamp(1, 20);
        let base_delay_secs = env_u64(
            "ASI_AGENTD_OUTBOX_RETRY_BASE_DELAY_SECS",
            DEFAULT_RETRY_BASE_DELAY_SECS,
        )
        .clamp(1, 3600);
        let max_delay_secs = env_u64(
            "ASI_AGENTD_OUTBOX_RETRY_MAX_DELAY_SECS",
            DEFAULT_RETRY_MAX_DELAY_SECS,
        )
        .clamp(base_delay_secs, 86_400);

        Self {
            max_attempts,
            base_delay_secs,
            max_delay_secs,
        }
    }
}

impl RemoteLink {
    pub fn pull_once(&self) -> Result<PullReport, String> {
        let inbox = inbox_dir()?;
        let processed = inbox.join("processed");
        let failed = inbox.join("failed");

        fs::create_dir_all(&inbox).map_err(|e| e.to_string())?;
        fs::create_dir_all(&processed).map_err(|e| e.to_string())?;
        fs::create_dir_all(&failed).map_err(|e| e.to_string())?;
        fs::create_dir_all(seen_dir()?).map_err(|e| e.to_string())?;
        fs::create_dir_all(acks_dir()?).map_err(|e| e.to_string())?;

        let mut report = PullReport::default();

        for entry in
            fs::read_dir(&inbox).map_err(|e| format!("read_dir {}: {}", inbox.display(), e))?
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("inbox_job");

            let raw = match fs::read_to_string(&path) {
                Ok(v) => v,
                Err(err) => {
                    report.invalid += 1;
                    let msg = format!("failed to read {}: {}", path.display(), err);
                    write_ack(stem, "invalid", None, Some(&msg))?;
                    let err_file = failed.join(format!("{}.error.txt", sanitize(stem)));
                    fs::write(&err_file, msg)
                        .map_err(|e| format!("write {}: {}", err_file.display(), e))?;
                    let dest = failed.join(format!("{}.bad.json", sanitize(stem)));
                    move_file(&path, &dest)?;
                    continue;
                }
            };

            let content = raw.trim_start_matches('\u{feff}');
            let digest = digest_hex(content);
            if let Some(existing_job_id) = seen_job_id(&digest)? {
                report.duplicates += 1;
                write_ack(
                    stem,
                    "duplicate",
                    Some(&existing_job_id),
                    Some("duplicate payload hash"),
                )?;
                let dest = processed.join(format!("{}__duplicate.json", sanitize(stem)));
                move_file(&path, &dest)?;
                continue;
            }

            match submit_job_from_spec_file(&path) {
                Ok(job) => {
                    report.accepted += 1;
                    mark_seen(&digest, &job.id)?;
                    write_ack(stem, "accepted", Some(&job.id), None)?;
                    let dest = processed.join(format!("{}__{}.json", sanitize(stem), job.id));
                    move_file(&path, &dest)?;
                }
                Err(err) => {
                    report.invalid += 1;
                    write_ack(stem, "invalid", None, Some(&err))?;
                    let err_file = failed.join(format!("{}.error.txt", sanitize(stem)));
                    fs::write(&err_file, &err)
                        .map_err(|e| format!("write {}: {}", err_file.display(), e))?;
                    let dest = failed.join(format!("{}.bad.json", sanitize(stem)));
                    move_file(&path, &dest)?;
                }
            }
        }

        Ok(report)
    }

    pub fn push_outbox_once(&self) -> Result<OutboxPushReport, String> {
        let endpoint = std::env::var("ASI_AGENTD_REMOTE_OUTBOX_URL").unwrap_or_default();

        let pending = list_outbox_items()?;
        if endpoint.trim().is_empty() {
            return Ok(OutboxPushReport {
                sent: 0,
                failed: 0,
                skipped: pending.len(),
            });
        }

        let cfg = RetryConfig::from_env();

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .map_err(|e| format!("build outbox client: {}", e))?;

        let mut report = OutboxPushReport::default();
        let now = epoch_seconds();
        for (kind, path) in pending {
            let filename = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("event.json")
                .to_string();

            if should_defer_retry(&kind, &filename, now)? {
                report.skipped += 1;
                continue;
            }

            let body = match fs::read_to_string(&path) {
                Ok(v) => v,
                Err(e) => {
                    report.failed += 1;
                    register_retry_failure(
                        &cfg,
                        &kind,
                        &path,
                        &filename,
                        &format!("read failed: {}", e),
                    )?;
                    continue;
                }
            };

            let resp = client
                .post(&endpoint)
                .header("content-type", "application/json")
                .header("x-asi-outbox-kind", &kind)
                .header("x-asi-outbox-file", &filename)
                .body(body)
                .send();

            match resp {
                Ok(r) if r.status().is_success() => {
                    report.sent += 1;
                    clear_retry_state(&kind, &filename)?;
                    let dest = sent_dir(&kind)?.join(&filename);
                    move_file(&path, &dest)?;
                }
                Ok(r) => {
                    report.failed += 1;
                    register_retry_failure(
                        &cfg,
                        &kind,
                        &path,
                        &filename,
                        &format!("http status {}", r.status()),
                    )?;
                }
                Err(e) => {
                    report.failed += 1;
                    register_retry_failure(
                        &cfg,
                        &kind,
                        &path,
                        &filename,
                        &format!("request failed: {}", e),
                    )?;
                }
            }
        }

        Ok(report)
    }

    pub fn record_completion(&self, job: &JobRecord) -> Result<PathBuf, String> {
        fs::create_dir_all(completions_dir()?).map_err(|e| e.to_string())?;

        let path = completions_dir()?.join(format!("{}.json", sanitize(&job.id)));
        let payload = json!({
            "job_id": job.id,
            "state": job.state.as_str(),
            "project_path": job.spec.project_path,
            "goal": job.spec.goal,
            "updated_at": job.updated_at,
            "attempts": job.attempts,
            "error": job.error,
            "summary": job.result.as_ref().map(|r| r.summary.clone()).unwrap_or_default(),
            "cost_usd": job.result.as_ref().map(|r| r.cost_usd).unwrap_or(0.0),
            "changed_files": job.result.as_ref().map(|r| r.changed_files.clone()).unwrap_or_default(),
        });
        let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
        fs::write(&path, text).map_err(|e| format!("write {}: {}", path.display(), e))?;
        Ok(path)
    }
}

fn inbox_dir() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok(cwd.join(".asi").join("agentd").join("inbox"))
}

fn remote_root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok(cwd.join(".asi").join("agentd").join("remote"))
}

fn seen_dir() -> Result<PathBuf, String> {
    Ok(remote_root()?.join("seen"))
}

fn outbox_root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok(cwd.join(".asi").join("agentd").join("outbox"))
}

fn acks_dir() -> Result<PathBuf, String> {
    Ok(outbox_root()?.join("acks"))
}

fn completions_dir() -> Result<PathBuf, String> {
    Ok(outbox_root()?.join("completions"))
}

fn sent_dir(kind: &str) -> Result<PathBuf, String> {
    Ok(outbox_root()?.join("sent").join(kind))
}

fn retry_dir(kind: &str) -> Result<PathBuf, String> {
    Ok(outbox_root()?.join("retry").join(kind))
}

fn dead_dir(kind: &str) -> Result<PathBuf, String> {
    Ok(outbox_root()?.join("dead").join(kind))
}

fn list_outbox_items() -> Result<Vec<(String, PathBuf)>, String> {
    let mut out = Vec::new();

    for (kind, dir) in [("acks", acks_dir()?), ("completions", completions_dir()?)] {
        fs::create_dir_all(&dir).map_err(|e| format!("create_dir_all {}: {}", dir.display(), e))?;
        for entry in fs::read_dir(&dir).map_err(|e| format!("read_dir {}: {}", dir.display(), e))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
                out.push((kind.to_string(), path));
            }
        }
    }

    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}

fn retry_state_path(kind: &str, filename: &str) -> Result<PathBuf, String> {
    Ok(retry_dir(kind)?.join(format!("{}.retry.json", sanitize(filename))))
}

fn read_retry_state(kind: &str, filename: &str) -> Result<Option<RetryState>, String> {
    let path = retry_state_path(kind, filename)?;
    if !path.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    let state: RetryState = serde_json::from_str(&text)
        .map_err(|e| format!("invalid retry state {}: {}", path.display(), e))?;
    Ok(Some(state))
}

fn write_retry_state(kind: &str, filename: &str, state: &RetryState) -> Result<(), String> {
    let path = retry_state_path(kind, filename)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create_dir_all {}: {}", parent.display(), e))?;
    }
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("write {}: {}", path.display(), e))
}

fn clear_retry_state(kind: &str, filename: &str) -> Result<(), String> {
    let path = retry_state_path(kind, filename)?;
    if !path.exists() {
        return Ok(());
    }
    fs::remove_file(&path).map_err(|e| format!("remove {}: {}", path.display(), e))
}

fn should_defer_retry(kind: &str, filename: &str, now: u64) -> Result<bool, String> {
    let Some(state) = read_retry_state(kind, filename)? else {
        return Ok(false);
    };
    Ok(state.next_attempt_at > now)
}

fn register_retry_failure(
    cfg: &RetryConfig,
    kind: &str,
    path: &PathBuf,
    filename: &str,
    message: &str,
) -> Result<(), String> {
    let now = epoch_seconds();
    let mut state = read_retry_state(kind, filename)?.unwrap_or(RetryState {
        attempts: 0,
        next_attempt_at: now,
        last_attempt_at: 0,
        last_error: String::new(),
    });

    state.attempts = state.attempts.saturating_add(1);
    state.last_attempt_at = now;
    state.last_error = message.to_string();

    if state.attempts >= cfg.max_attempts {
        let final_msg = format!(
            "max attempts reached attempts={} error={}",
            state.attempts, message
        );
        write_retry_error(kind, filename, &final_msg)?;
        let dest = dead_dir(kind)?.join(filename);
        move_file(path, &dest)?;
        clear_retry_state(kind, filename)?;
        return Ok(());
    }

    let backoff = compute_backoff_secs(cfg, state.attempts);
    state.next_attempt_at = now.saturating_add(backoff);
    write_retry_state(kind, filename, &state)?;

    let msg = format!(
        "attempt={} next_retry_at={} backoff_secs={} error={}",
        state.attempts, state.next_attempt_at, backoff, message
    );
    write_retry_error(kind, filename, &msg)
}

fn compute_backoff_secs(cfg: &RetryConfig, attempts: u32) -> u64 {
    let shift = attempts.saturating_sub(1).min(20);
    let factor = 1u64 << shift;
    let delay = cfg.base_delay_secs.saturating_mul(factor);
    delay.clamp(1, cfg.max_delay_secs)
}

fn write_retry_error(kind: &str, filename: &str, message: &str) -> Result<(), String> {
    let dir = retry_dir(kind)?;
    fs::create_dir_all(&dir).map_err(|e| format!("create_dir_all {}: {}", dir.display(), e))?;
    let path = dir.join(format!("{}.error.txt", sanitize(filename)));
    fs::write(&path, message).map_err(|e| format!("write {}: {}", path.display(), e))
}

fn digest_hex(text: &str) -> String {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn seen_job_id(digest: &str) -> Result<Option<String>, String> {
    let path = seen_dir()?.join(format!("{}.json", sanitize(digest)));
    if !path.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("invalid seen file {}: {}", path.display(), e))?;

    Ok(value
        .get("job_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string()))
}

fn mark_seen(digest: &str, job_id: &str) -> Result<(), String> {
    fs::create_dir_all(seen_dir()?).map_err(|e| e.to_string())?;
    let path = seen_dir()?.join(format!("{}.json", sanitize(digest)));
    let payload = json!({
        "digest": digest,
        "job_id": job_id,
        "created_at": epoch_seconds(),
    });
    let body = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    fs::write(&path, body).map_err(|e| format!("write {}: {}", path.display(), e))
}

fn write_ack(
    stem: &str,
    status: &str,
    job_id: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    fs::create_dir_all(acks_dir()?).map_err(|e| e.to_string())?;
    let path = acks_dir()?.join(format!("{}__{}.json", sanitize(stem), epoch_seconds()));

    let payload = json!({
        "source": stem,
        "status": status,
        "job_id": job_id,
        "error": error,
        "created_at": epoch_seconds(),
    });

    let body = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    fs::write(&path, body).map_err(|e| format!("write {}: {}", path.display(), e))
}

fn sanitize(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        }
    }
    if out.is_empty() {
        "inbox_job".to_string()
    } else {
        out
    }
}

fn move_file(src: &PathBuf, dest: &PathBuf) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create_dir_all {}: {}", parent.display(), e))?;
    }

    match fs::rename(src, dest) {
        Ok(_) => Ok(()),
        Err(_) => {
            fs::copy(src, dest)
                .map_err(|e| format!("copy {} -> {}: {}", src.display(), dest.display(), e))?;
            fs::remove_file(src).map_err(|e| format!("remove {}: {}", src.display(), e))
        }
    }
}

fn env_u32(key: &str, default_value: u32) -> u32 {
    match std::env::var(key) {
        Ok(v) => v.trim().parse::<u32>().unwrap_or(default_value),
        Err(_) => default_value,
    }
}

fn env_u64(key: &str, default_value: u64) -> u64 {
    match std::env::var(key) {
        Ok(v) => v.trim().parse::<u64>().unwrap_or(default_value),
        Err(_) => default_value,
    }
}
