use crate::config::AppConfig;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
struct RemotePolicyPatch {
    provider: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    max_turns: Option<usize>,
    extended_thinking: Option<bool>,
    markdown_render: Option<bool>,
    telemetry_enabled: Option<bool>,
    telemetry_log_tool_details: Option<bool>,
    undercover_mode: Option<bool>,
    safe_shell_mode: Option<bool>,
    feature_killswitches: Option<HashMap<String, bool>>,
    permission_allow_rules: Option<Vec<String>>,
    permission_deny_rules: Option<Vec<String>>,
    path_restriction_enabled: Option<bool>,
    additional_directories: Option<Vec<String>>,
}

pub fn sync_remote_policy(cfg: &mut AppConfig) -> Result<String, String> {
    if !cfg.remote_policy_enabled {
        return Err("remote policy is disabled (set remote_policy_enabled=true)".to_string());
    }
    let url = cfg
        .remote_policy_url
        .as_deref()
        .ok_or_else(|| "remote_policy_url is not set".to_string())?;

    let patch = Client::new()
        .get(url)
        .send()
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| format!("remote policy HTTP error: {}", e))?
        .json::<RemotePolicyPatch>()
        .map_err(|e| format!("remote policy parse error: {}", e))?;

    let mut applied = Vec::new();

    if let Some(v) = patch.provider {
        cfg.provider = v;
        applied.push("provider");
    }
    if let Some(v) = patch.model {
        cfg.model = v;
        applied.push("model");
    }
    if let Some(v) = patch.permission_mode {
        if v == "danger-full-access" || v == "bypass-permissions" {
            applied.push("permission_mode(ignored_unsafe)");
        } else {
            cfg.permission_mode = v;
            applied.push("permission_mode");
        }
    }
    if let Some(v) = patch.max_turns {
        cfg.max_turns = v.clamp(1, 200);
        applied.push("max_turns");
    }
    if let Some(v) = patch.extended_thinking {
        cfg.extended_thinking = v;
        applied.push("extended_thinking");
    }
    if let Some(v) = patch.markdown_render {
        cfg.markdown_render = v;
        applied.push("markdown_render");
    }
    if let Some(v) = patch.telemetry_enabled {
        cfg.telemetry_enabled = v;
        applied.push("telemetry_enabled");
    }
    if let Some(v) = patch.telemetry_log_tool_details {
        cfg.telemetry_log_tool_details = v;
        applied.push("telemetry_log_tool_details");
    }
    if let Some(v) = patch.undercover_mode {
        cfg.undercover_mode = v;
        applied.push("undercover_mode");
    }
    if let Some(v) = patch.safe_shell_mode {
        cfg.safe_shell_mode = v;
        applied.push("safe_shell_mode");
    }
    if let Some(v) = patch.feature_killswitches {
        for (k, disabled) in v {
            cfg.feature_killswitches.insert(k, disabled);
        }
        applied.push("feature_killswitches");
    }
    if let Some(v) = patch.permission_allow_rules {
        cfg.permission_allow_rules = v;
        applied.push("permission_allow_rules");
    }
    if let Some(v) = patch.permission_deny_rules {
        cfg.permission_deny_rules = v;
        applied.push("permission_deny_rules");
    }
    if let Some(v) = patch.path_restriction_enabled {
        cfg.path_restriction_enabled = v;
        applied.push("path_restriction_enabled");
    }
    if let Some(v) = patch.additional_directories {
        cfg.additional_directories = v;
        applied.push("additional_directories");
    }

    let _ = cfg.save();
    Ok(format!(
        "remote policy synced from {} (applied: {})",
        url,
        if applied.is_empty() {
            "none".to_string()
        } else {
            applied.join(", ")
        }
    ))
}
