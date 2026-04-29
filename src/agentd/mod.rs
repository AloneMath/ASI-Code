pub mod checkpoint;
pub mod device_identity;
pub mod executor;
pub mod health;
pub mod job_queue;
pub mod notify;
pub mod remote_link;
pub mod result_store;
pub mod safety_gate;
pub mod service;
pub mod types;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use checkpoint::CheckpointStore;
use device_identity::DeviceIdentity;
use health::AgentHealth;
use safety_gate::SafetyGate;
use service::AgentdService;
use types::{JobRecord, JobSpec, JobState};

pub fn daemon_install() -> Result<String, String> {
    ensure_layout()?;
    let identity = DeviceIdentity::default().ensure_registered()?;
    Ok(format!(
        "agentd layout ready root={} device_id={}",
        agentd_root()?.display(),
        identity.device_id
    ))
}

pub fn daemon_start() -> Result<String, String> {
    daemon_run(None, None, false, false)
}

pub fn daemon_run(
    max_loops: Option<usize>,
    interval_ms: Option<u64>,
    no_stop_when_idle: bool,
    once: bool,
) -> Result<String, String> {
    ensure_layout()?;
    let mut service = AgentdService::default();
    if let Some(v) = max_loops {
        service.max_loops = v.clamp(1, 200_000);
    }
    if let Some(v) = interval_ms {
        service.interval_ms = v.clamp(10, 60_000);
    }
    service.stop_when_idle = !no_stop_when_idle;

    if once {
        service.max_jobs = Some(1);
        service.stop_when_idle = true;
    }

    let report = service.run()?;
    Ok(format!(
        "agentd run complete max_loops={} interval_ms={} stop_when_idle={} max_jobs={} processed={} succeeded={} failed={} pulled_remote_jobs={} pulled_remote_duplicates={} pulled_remote_invalid={} pushed_outbox_sent={} pushed_outbox_failed={} pushed_outbox_skipped={} completion_events={} idle_loops={}",
        service.max_loops,
        service.interval_ms,
        service.stop_when_idle,
        service.max_jobs.map(|v| v.to_string()).unwrap_or_else(|| "none".to_string()),
        report.processed,
        report.succeeded,
        report.failed,
        report.pulled_remote_jobs,
        report.pulled_remote_duplicates,
        report.pulled_remote_invalid,
        report.pushed_outbox_sent,
        report.pushed_outbox_failed,
        report.pushed_outbox_skipped,
        report.completion_events,
        report.idle_loops
    ))
}

pub fn daemon_stop() -> Result<String, String> {
    ensure_layout()?;
    let path = write_stop_signal()?;
    Ok(format!("agentd stop requested via {}", path.display()))
}

pub fn daemon_status() -> Result<String, String> {
    ensure_layout()?;
    let snap = build_status_snapshot()?;

    let queued = snap.get("queued").and_then(|v| v.as_u64()).unwrap_or(0);
    let running = snap.get("running").and_then(|v| v.as_u64()).unwrap_or(0);
    let succeeded = snap.get("succeeded").and_then(|v| v.as_u64()).unwrap_or(0);
    let failed = snap.get("failed").and_then(|v| v.as_u64()).unwrap_or(0);
    let canceled = snap.get("canceled").and_then(|v| v.as_u64()).unwrap_or(0);

    let heartbeat = heartbeat_summary_from_value(snap.get("heartbeat"))
        .unwrap_or_else(|| "heartbeat=none".to_string());

    Ok(format!(
        "agentd status queued={} running={} succeeded={} failed={} canceled={} {}",
        queued, running, succeeded, failed, canceled, heartbeat
    ))
}

pub fn daemon_status_json() -> Result<String, String> {
    ensure_layout()?;
    let snap = build_status_snapshot()?;
    serde_json::to_string_pretty(&snap).map_err(|e| e.to_string())
}

pub fn daemon_heartbeat() -> Result<String, String> {
    ensure_layout()?;
    match read_heartbeat_value()? {
        Some(v) => serde_json::to_string_pretty(&v).map_err(|e| e.to_string()),
        None => Ok("heartbeat=none".to_string()),
    }
}

pub fn daemon_checkpoint() -> Result<String, String> {
    ensure_layout()?;
    match read_checkpoint_value()? {
        Some(v) => serde_json::to_string_pretty(&v).map_err(|e| e.to_string()),
        None => Ok("checkpoint=none".to_string()),
    }
}

pub fn submit_job_from_spec_file(spec_path: &Path) -> Result<JobRecord, String> {
    ensure_layout()?;
    let text = fs::read_to_string(spec_path)
        .map_err(|e| format!("failed to read {}: {}", spec_path.display(), e))?;
    let content = text.trim_start_matches('\u{feff}');
    let mut spec: JobSpec = serde_json::from_str(content)
        .map_err(|e| format!("invalid JobSpec json {}: {}", spec_path.display(), e))?;

    if spec.id.trim().is_empty() {
        spec.id = format!("job-{}", epoch_seconds());
    }
    if spec.timeout_seconds == 0 {
        spec.timeout_seconds = 1800;
    }

    let decision = SafetyGate::default().evaluate_job_spec(&spec)?;
    if !decision.allowed {
        return Err(format!(
            "job blocked by safety gate: {}",
            decision.reasons.join("; ")
        ));
    }
    for warning in decision.warnings {
        eprintln!("WARN safety_gate: {}", warning);
    }

    let mut record = JobRecord::new(spec, epoch_seconds());
    record.state = JobState::Queued;
    save_job_record(&record)?;
    Ok(record)
}

pub fn list_jobs(limit: usize) -> Result<Vec<JobRecord>, String> {
    ensure_layout()?;
    let mut jobs = Vec::new();
    let dir = jobs_dir()?;

    for entry in fs::read_dir(&dir).map_err(|e| format!("read_dir {}: {}", dir.display(), e))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let text = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
        let record: JobRecord = serde_json::from_str(&text)
            .map_err(|e| format!("invalid job json {}: {}", path.display(), e))?;
        jobs.push(record);
    }

    jobs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    if jobs.len() > limit {
        jobs.truncate(limit);
    }
    Ok(jobs)
}

pub fn get_job(job_id: &str) -> Result<JobRecord, String> {
    ensure_layout()?;
    let path = job_file(job_id)?;
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
    serde_json::from_str(&text).map_err(|e| format!("invalid job json {}: {}", path.display(), e))
}

pub fn cancel_job(job_id: &str) -> Result<JobRecord, String> {
    let mut record = get_job(job_id)?;
    record.state = JobState::Canceled;
    record.updated_at = epoch_seconds();
    record.error = Some("canceled_by_user".to_string());
    save_job_record(&record)?;
    Ok(record)
}

pub fn retry_job(job_id: &str) -> Result<JobRecord, String> {
    let mut record = get_job(job_id)?;
    record.state = JobState::Queued;
    record.attempts = record.attempts.saturating_add(1);
    record.updated_at = epoch_seconds();
    record.error = None;
    save_job_record(&record)?;
    Ok(record)
}

pub(super) fn save_job_record(record: &JobRecord) -> Result<(), String> {
    let path = job_file(&record.id)?;
    let json = serde_json::to_string_pretty(record)
        .map_err(|e| format!("serialize job {}: {}", record.id, e))?;
    fs::write(&path, json).map_err(|e| format!("write {}: {}", path.display(), e))
}

pub(super) fn ensure_layout() -> Result<(), String> {
    fs::create_dir_all(agentd_root()?).map_err(|e| e.to_string())?;
    fs::create_dir_all(jobs_dir()?).map_err(|e| e.to_string())?;
    fs::create_dir_all(results_dir()?).map_err(|e| e.to_string())?;
    fs::create_dir_all(locks_dir()?).map_err(|e| e.to_string())?;
    fs::create_dir_all(inbox_dir()?).map_err(|e| e.to_string())?;
    fs::create_dir_all(remote_seen_dir()?).map_err(|e| e.to_string())?;
    fs::create_dir_all(outbox_acks_dir()?).map_err(|e| e.to_string())?;
    fs::create_dir_all(outbox_completions_dir()?).map_err(|e| e.to_string())?;
    Ok(())
}

fn build_status_snapshot() -> Result<serde_json::Value, String> {
    let jobs = list_jobs(500)?;

    let mut queued = 0u64;
    let mut running = 0u64;
    let mut succeeded = 0u64;
    let mut failed = 0u64;
    let mut canceled = 0u64;

    for j in jobs {
        match j.state {
            JobState::Queued => queued += 1,
            JobState::Running => running += 1,
            JobState::Succeeded => succeeded += 1,
            JobState::Failed => failed += 1,
            JobState::Canceled => canceled += 1,
        }
    }

    let heartbeat = read_heartbeat_value()?.unwrap_or(serde_json::Value::Null);
    let checkpoint = read_checkpoint_value()?.unwrap_or(serde_json::Value::Null);
    let health = match AgentHealth::default().snapshot() {
        Ok(v) => serde_json::to_value(v).map_err(|e| e.to_string())?,
        Err(e) => serde_json::json!({"status": "error", "error": e}),
    };
    let device_identity = match DeviceIdentity::default().summary() {
        Ok(v) => serde_json::to_value(v).map_err(|e| e.to_string())?,
        Err(e) => serde_json::json!({"status": "error", "error": e}),
    };

    Ok(serde_json::json!({
        "queued": queued,
        "running": running,
        "succeeded": succeeded,
        "failed": failed,
        "canceled": canceled,
        "heartbeat": heartbeat,
        "checkpoint": checkpoint,
        "health": health,
        "device_identity": device_identity,
        "updated_at": epoch_seconds(),
    }))
}

fn heartbeat_summary_from_value(v: Option<&serde_json::Value>) -> Option<String> {
    let hb = v?;
    if hb.is_null() {
        return Some("heartbeat=none".to_string());
    }

    let state = hb
        .get("state")
        .and_then(|x| x.as_str())
        .unwrap_or("unknown");
    let pid = hb.get("pid").and_then(|x| x.as_u64()).unwrap_or(0);
    let updated_at = hb.get("updated_at").and_then(|x| x.as_u64()).unwrap_or(0);
    let processed = hb.get("processed").and_then(|x| x.as_u64()).unwrap_or(0);
    let succeeded = hb.get("succeeded").and_then(|x| x.as_u64()).unwrap_or(0);
    let failed = hb.get("failed").and_then(|x| x.as_u64()).unwrap_or(0);

    Some(format!(
        "heartbeat=state:{} pid:{} updated_at:{} processed:{} succeeded:{} failed:{}",
        state, pid, updated_at, processed, succeeded, failed
    ))
}

fn read_heartbeat_value() -> Result<Option<serde_json::Value>, String> {
    let path = heartbeat_file()?;
    if !path.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("invalid heartbeat {}: {}", path.display(), e))?;

    Ok(Some(value))
}

fn read_checkpoint_value() -> Result<Option<serde_json::Value>, String> {
    let store = CheckpointStore;
    let Some(snapshot) = store.load()? else {
        return Ok(None);
    };
    let value = serde_json::to_value(snapshot).map_err(|e| e.to_string())?;
    Ok(Some(value))
}

pub(super) fn agentd_root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok(cwd.join(".asi").join("agentd"))
}

pub(super) fn jobs_dir() -> Result<PathBuf, String> {
    Ok(agentd_root()?.join("jobs"))
}

pub(super) fn results_dir() -> Result<PathBuf, String> {
    Ok(agentd_root()?.join("results"))
}

pub(super) fn locks_dir() -> Result<PathBuf, String> {
    Ok(agentd_root()?.join("locks"))
}

pub(super) fn heartbeat_file() -> Result<PathBuf, String> {
    Ok(agentd_root()?.join("heartbeat.json"))
}

fn inbox_dir() -> Result<PathBuf, String> {
    Ok(agentd_root()?.join("inbox"))
}

fn remote_seen_dir() -> Result<PathBuf, String> {
    Ok(agentd_root()?.join("remote").join("seen"))
}

fn outbox_acks_dir() -> Result<PathBuf, String> {
    Ok(agentd_root()?.join("outbox").join("acks"))
}

fn outbox_completions_dir() -> Result<PathBuf, String> {
    Ok(agentd_root()?.join("outbox").join("completions"))
}

pub(super) fn stop_signal_file() -> Result<PathBuf, String> {
    Ok(agentd_root()?.join("stop.signal"))
}

pub(super) fn is_stop_signal_present() -> Result<bool, String> {
    Ok(stop_signal_file()?.exists())
}

pub(super) fn clear_stop_signal() -> Result<bool, String> {
    let path = stop_signal_file()?;
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path).map_err(|e| format!("remove {}: {}", path.display(), e))?;
    Ok(true)
}

fn write_stop_signal() -> Result<PathBuf, String> {
    let path = stop_signal_file()?;
    let payload = serde_json::json!({
        "requested_at": epoch_seconds(),
        "pid_hint": read_heartbeat_value()?
            .as_ref()
            .and_then(|v| v.get("pid"))
            .and_then(|v| v.as_u64()),
    });
    let body = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    fs::write(&path, body).map_err(|e| format!("write {}: {}", path.display(), e))?;
    Ok(path)
}

fn job_file(job_id: &str) -> Result<PathBuf, String> {
    let safe = sanitize_job_id(job_id);
    Ok(jobs_dir()?.join(format!("{}.json", safe)))
}

fn sanitize_job_id(job_id: &str) -> String {
    let mut out = String::new();
    for ch in job_id.chars() {
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

pub(super) fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
