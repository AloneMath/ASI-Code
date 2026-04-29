use crate::config::AppConfig;
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn telemetry_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".asi_telemetry.jsonl")
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn log_turn(
    cfg: &AppConfig,
    provider: &str,
    model: &str,
    stop_reason: &str,
    input_tokens: usize,
    output_tokens: usize,
    turn_cost_usd: f64,
) -> Result<(), String> {
    if !cfg.telemetry_enabled {
        return Ok(());
    }

    let line = json!({
        "ts_ms": now_millis(),
        "event": "turn",
        "provider": provider,
        "model": model,
        "permission_mode": cfg.permission_mode,
        "stop_reason": stop_reason,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "turn_cost_usd": turn_cost_usd
        }
    });

    append_json_line(&line)
}

pub fn log_tool_call(
    cfg: &AppConfig,
    provider: &str,
    model: &str,
    tool_name: &str,
    raw_args: &str,
    stop_reason: &str,
    output_preview: &str,
) -> Result<(), String> {
    if !cfg.telemetry_enabled {
        return Ok(());
    }

    let input = if cfg.telemetry_log_tool_details {
        raw_args.to_string()
    } else {
        truncate(raw_args, 128)
    };

    let line = json!({
        "ts_ms": now_millis(),
        "event": "tool_call",
        "provider": provider,
        "model": model,
        "permission_mode": cfg.permission_mode,
        "tool_name": tool_name,
        "tool_input": input,
        "stop_reason": stop_reason,
        "output_preview": truncate(output_preview, 256)
    });

    append_json_line(&line)
}

pub fn log_auto_loop_summary(
    cfg: &AppConfig,
    provider: &str,
    model: &str,
    mode: &str,
    loop_stop_reason: Option<&str>,
    confidence_gate_checks: usize,
    confidence_gate_missing_declaration: usize,
    confidence_gate_low_declaration: usize,
    confidence_gate_blocked_risky_toolcalls: usize,
    confidence_gate_retries_exhausted: usize,
) -> Result<(), String> {
    if !cfg.telemetry_enabled {
        return Ok(());
    }

    let line = json!({
        "ts_ms": now_millis(),
        "event": "auto_loop_summary",
        "provider": provider,
        "model": model,
        "permission_mode": cfg.permission_mode,
        "mode": mode,
        "loop_stop_reason": loop_stop_reason.unwrap_or("none"),
        "confidence_gate": {
            "checks": confidence_gate_checks,
            "missing_declaration": confidence_gate_missing_declaration,
            "low_declaration": confidence_gate_low_declaration,
            "blocked_risky_toolcalls": confidence_gate_blocked_risky_toolcalls,
            "retries_exhausted": confidence_gate_retries_exhausted
        }
    });

    append_json_line(&line)
}

fn append_json_line(value: &serde_json::Value) -> Result<(), String> {
    let path = telemetry_path();
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let line = serde_json::to_string(value).map_err(|e| e.to_string())?;
    file.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = text.chars().take(max).collect::<String>();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use std::fs;

    #[test]
    fn auto_loop_summary_respects_telemetry_toggle() {
        let path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".asi_telemetry.jsonl");
        let _ = fs::remove_file(&path);

        let mut cfg = AppConfig::default();
        cfg.telemetry_enabled = false;
        let _ = log_auto_loop_summary(
            &cfg,
            "provider",
            "model",
            "prompt",
            Some("none"),
            1,
            1,
            0,
            0,
            0,
        );

        assert!(!path.exists());
    }
}
