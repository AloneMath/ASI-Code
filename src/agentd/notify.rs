use reqwest::blocking::Client;
use serde_json::json;

use super::types::JobRecord;

#[derive(Debug)]
pub struct Notifier {
    client: Client,
}

impl Default for Notifier {
    fn default() -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { client }
    }
}

impl Notifier {
    pub fn send(&self, job: &JobRecord) -> Result<(), String> {
        if job.spec.notify_targets.is_empty() {
            return Ok(());
        }

        let mut errs = Vec::new();
        for target in &job.spec.notify_targets {
            let kind = target.kind.trim().to_ascii_lowercase();
            let result = match kind.as_str() {
                "webhook" => self.send_webhook(&target.target, job),
                "stdout" => {
                    println!(
                        "notify stdout: id={} state={} goal={}",
                        job.id,
                        job.state.as_str(),
                        job.spec.goal
                    );
                    Ok(())
                }
                _ => Err(format!("unsupported notify target kind: {}", target.kind)),
            };
            if let Err(e) = result {
                errs.push(format!("{} -> {}", target.target, e));
            }
        }

        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs.join("; "))
        }
    }

    fn send_webhook(&self, url: &str, job: &JobRecord) -> Result<(), String> {
        let payload = json!({
            "job_id": job.id,
            "state": job.state.as_str(),
            "goal": job.spec.goal,
            "project_path": job.spec.project_path,
            "updated_at": job.updated_at,
            "error": job.error,
            "summary": job.result.as_ref().map(|r| r.summary.as_str()).unwrap_or(""),
        });

        let resp = self
            .client
            .post(url)
            .json(&payload)
            .send()
            .map_err(|e| format!("webhook request failed: {}", e))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("webhook status {}", resp.status()))
        }
    }
}
