use std::fs;
use std::thread;
use std::time::Duration;

use serde_json::json;

use super::checkpoint::{CheckpointSnapshot, CheckpointStore};
use super::executor::Executor;
use super::job_queue::JobQueue;
use super::notify::Notifier;
use super::remote_link::RemoteLink;
use super::result_store::ResultStore;
use super::{clear_stop_signal, epoch_seconds, heartbeat_file, is_stop_signal_present};

#[derive(Debug, Clone)]
pub struct AgentdService {
    pub max_loops: usize,
    pub interval_ms: u64,
    pub stop_when_idle: bool,
    pub max_jobs: Option<usize>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ServiceReport {
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

impl Default for AgentdService {
    fn default() -> Self {
        Self {
            max_loops: 100,
            interval_ms: 200,
            stop_when_idle: true,
            max_jobs: None,
        }
    }
}

impl AgentdService {
    pub fn run(&self) -> Result<ServiceReport, String> {
        let queue = JobQueue;
        let executor = Executor;
        let result_store = ResultStore;
        let notifier = Notifier::default();
        let remote = RemoteLink;
        let checkpoint = CheckpointStore;

        let mut report = ServiceReport::default();
        let mut loops = 0usize;
        let pid = std::process::id();

        if let Err(e) = clear_stop_signal() {
            eprintln!("WARN clear stop signal failed: {}", e);
        }

        if let Err(e) = write_runtime_state(&checkpoint, self, pid, "starting", loops, &report) {
            eprintln!("WARN runtime state write failed: {}", e);
        }

        while loops < self.max_loops {
            loops += 1;

            if let Err(e) = write_runtime_state(&checkpoint, self, pid, "running", loops, &report) {
                eprintln!("WARN runtime state write failed: {}", e);
            }

            match check_stop_requested(&checkpoint, self, pid, loops, &report) {
                Ok(true) => break,
                Ok(false) => {}
                Err(e) => eprintln!("WARN stop signal check failed: {}", e),
            }

            match remote.pull_once() {
                Ok(pull) => {
                    report.pulled_remote_jobs += pull.accepted;
                    report.pulled_remote_duplicates += pull.duplicates;
                    report.pulled_remote_invalid += pull.invalid;
                }
                Err(e) => eprintln!("WARN remote pull failed: {}", e),
            }

            match remote.push_outbox_once() {
                Ok(push) => {
                    report.pushed_outbox_sent += push.sent;
                    report.pushed_outbox_failed += push.failed;
                    report.pushed_outbox_skipped += push.skipped;
                }
                Err(e) => eprintln!("WARN outbox push failed: {}", e),
            }

            let Some(job) = queue.claim_next()? else {
                report.idle_loops += 1;
                if self.stop_when_idle {
                    if let Err(e) =
                        write_runtime_state(&checkpoint, self, pid, "idle", loops, &report)
                    {
                        eprintln!("WARN runtime state write failed: {}", e);
                    }
                    break;
                }
                thread::sleep(Duration::from_millis(self.interval_ms));
                continue;
            };

            report.processed += 1;
            match executor.execute(&job) {
                Ok(result) => {
                    let _ = result_store.persist(&job.id, &result)?;
                    let finished = queue.finish_success(job, result)?;
                    match remote.record_completion(&finished) {
                        Ok(_) => report.completion_events += 1,
                        Err(e) => eprintln!(
                            "WARN completion outbox write failed for {}: {}",
                            finished.id, e
                        ),
                    }
                    if let Err(e) = notifier.send(&finished) {
                        eprintln!("WARN notify failed for {}: {}", finished.id, e);
                    }
                    report.succeeded += 1;
                }
                Err(e) => {
                    let failed = queue.finish_failed(job, e)?;
                    match remote.record_completion(&failed) {
                        Ok(_) => report.completion_events += 1,
                        Err(err) => eprintln!(
                            "WARN completion outbox write failed for {}: {}",
                            failed.id, err
                        ),
                    }
                    if let Err(err) = notifier.send(&failed) {
                        eprintln!("WARN notify failed for {}: {}", failed.id, err);
                    }
                    report.failed += 1;
                }
            }

            if let Some(max_jobs) = self.max_jobs {
                if report.processed >= max_jobs {
                    if let Err(e) = write_runtime_state(
                        &checkpoint,
                        self,
                        pid,
                        "max_jobs_reached",
                        loops,
                        &report,
                    ) {
                        eprintln!("WARN runtime state write failed: {}", e);
                    }
                    break;
                }
            }
        }

        if let Err(e) = write_runtime_state(&checkpoint, self, pid, "stopped", loops, &report) {
            eprintln!("WARN runtime state write failed: {}", e);
        }

        Ok(report)
    }
}

fn check_stop_requested(
    checkpoint: &CheckpointStore,
    svc: &AgentdService,
    pid: u32,
    loops: usize,
    report: &ServiceReport,
) -> Result<bool, String> {
    if !is_stop_signal_present()? {
        return Ok(false);
    }

    write_runtime_state(checkpoint, svc, pid, "stop_requested", loops, report)?;
    clear_stop_signal()?;
    Ok(true)
}

fn write_runtime_state(
    checkpoint: &CheckpointStore,
    svc: &AgentdService,
    pid: u32,
    state: &str,
    loops: usize,
    report: &ServiceReport,
) -> Result<(), String> {
    write_heartbeat(pid, state, loops, report)?;

    let snapshot = CheckpointSnapshot {
        pid,
        state: state.to_string(),
        updated_at: epoch_seconds(),
        loops,
        max_loops: svc.max_loops,
        interval_ms: svc.interval_ms,
        stop_when_idle: svc.stop_when_idle,
        max_jobs: svc.max_jobs,
        processed: report.processed,
        succeeded: report.succeeded,
        failed: report.failed,
        pulled_remote_jobs: report.pulled_remote_jobs,
        pulled_remote_duplicates: report.pulled_remote_duplicates,
        pulled_remote_invalid: report.pulled_remote_invalid,
        pushed_outbox_sent: report.pushed_outbox_sent,
        pushed_outbox_failed: report.pushed_outbox_failed,
        pushed_outbox_skipped: report.pushed_outbox_skipped,
        completion_events: report.completion_events,
        idle_loops: report.idle_loops,
    };

    checkpoint.save(&snapshot)
}

fn write_heartbeat(
    pid: u32,
    state: &str,
    loops: usize,
    report: &ServiceReport,
) -> Result<(), String> {
    let path = heartbeat_file()?;
    let payload = json!({
        "pid": pid,
        "state": state,
        "updated_at": epoch_seconds(),
        "loops": loops,
        "processed": report.processed,
        "succeeded": report.succeeded,
        "failed": report.failed,
        "pulled_remote_jobs": report.pulled_remote_jobs,
        "pulled_remote_duplicates": report.pulled_remote_duplicates,
        "pulled_remote_invalid": report.pulled_remote_invalid,
        "pushed_outbox_sent": report.pushed_outbox_sent,
        "pushed_outbox_failed": report.pushed_outbox_failed,
        "pushed_outbox_skipped": report.pushed_outbox_skipped,
        "completion_events": report.completion_events,
        "idle_loops": report.idle_loops,
    });

    let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("write {}: {}", path.display(), e))
}
