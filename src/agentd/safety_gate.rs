use serde::Serialize;

use super::types::JobSpec;

#[derive(Debug, Clone, Serialize)]
pub struct SafetyDecision {
    pub allowed: bool,
    pub risk_level: String,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Default)]
pub struct SafetyGate;

impl SafetyGate {
    pub fn evaluate_job_spec(&self, spec: &JobSpec) -> Result<SafetyDecision, String> {
        let mut reasons = Vec::new();
        let mut warnings = Vec::new();

        if spec.project_path.trim().is_empty() {
            reasons.push("project_path cannot be empty".to_string());
        }
        if spec.goal.trim().is_empty() {
            reasons.push("goal cannot be empty".to_string());
        }
        if spec.constraints.len() > 128 {
            reasons.push("too many constraints (max 128)".to_string());
        }

        if spec.timeout_seconds > 86_400 {
            warnings.push(format!(
                "timeout_seconds={} is very high; consider <=86400",
                spec.timeout_seconds
            ));
        }

        if let Some(max_steps) = parse_usize_constraint(&spec.constraints, "max_steps=") {
            if max_steps > 256 {
                reasons.push("max_steps too large (max 256)".to_string());
            } else if max_steps > 64 {
                warnings.push(format!(
                    "max_steps={} may increase run-away risk",
                    max_steps
                ));
            }
        }

        if let Some(max_failures) = parse_usize_constraint(&spec.constraints, "max_failures=") {
            if max_failures > 32 {
                reasons.push("max_failures too large (max 32)".to_string());
            } else if max_failures > 8 {
                warnings.push(format!(
                    "max_failures={} may hide unstable execution",
                    max_failures
                ));
            }
        }

        let corpus = format!(
            "{} {}",
            spec.goal.to_ascii_lowercase(),
            spec.constraints.join(" ").to_ascii_lowercase()
        );

        for marker in [
            "rm -rf",
            "del /f /s /q",
            "format c:",
            "shutdown /s",
            "credential dump",
            "token exfil",
            "disable security",
        ] {
            if corpus.contains(marker) {
                warnings.push(format!("high-risk marker detected: `{}`", marker));
            }
        }

        let allowed = reasons.is_empty();
        let risk_level = if !allowed {
            "blocked"
        } else if warnings.len() >= 2 {
            "elevated"
        } else {
            "normal"
        };

        Ok(SafetyDecision {
            allowed,
            risk_level: risk_level.to_string(),
            reasons,
            warnings,
        })
    }
}

fn parse_usize_constraint(constraints: &[String], prefix: &str) -> Option<usize> {
    let p = prefix.to_ascii_lowercase();
    for raw in constraints {
        let value = raw.trim().to_ascii_lowercase();
        if let Some(rest) = value.strip_prefix(&p) {
            if let Ok(n) = rest.parse::<usize>() {
                return Some(n);
            }
        }
    }
    None
}
