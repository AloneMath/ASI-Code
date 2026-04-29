use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyTarget {
    pub kind: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    #[serde(default)]
    pub id: String,
    pub project_path: String,
    pub goal: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default)]
    pub notify_targets: Vec<NotifyTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobResult {
    pub summary: String,
    #[serde(default)]
    pub artifacts: Vec<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub spec: JobSpec,
    pub state: JobState,
    pub attempts: u32,
    pub created_at: u64,
    pub updated_at: u64,
    pub result: Option<JobResult>,
    pub error: Option<String>,
}

impl JobRecord {
    pub fn new(spec: JobSpec, now: u64) -> Self {
        let id = spec.id.clone();
        Self {
            id,
            spec,
            state: JobState::Queued,
            attempts: 0,
            created_at: now,
            updated_at: now,
            result: None,
            error: None,
        }
    }
}

impl JobState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
}

fn default_timeout_seconds() -> u64 {
    1800
}
