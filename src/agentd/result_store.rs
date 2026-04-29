use std::fs;
use std::path::PathBuf;

use super::types::JobResult;
use super::{ensure_layout, results_dir};

#[derive(Debug, Default)]
pub struct ResultStore;

impl ResultStore {
    pub fn persist(&self, job_id: &str, result: &JobResult) -> Result<PathBuf, String> {
        ensure_layout()?;
        let file = results_dir()?.join(format!("{}.result.json", sanitize(job_id)));
        let body = serde_json::to_string_pretty(result)
            .map_err(|e| format!("serialize result for {}: {}", job_id, e))?;
        fs::write(&file, body).map_err(|e| format!("write {}: {}", file.display(), e))?;
        Ok(file)
    }
}

fn sanitize(job_id: &str) -> String {
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
