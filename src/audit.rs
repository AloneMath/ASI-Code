use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default)]
pub struct AuditSummary {
    pub total: usize,
    pub allow: usize,
    pub deny: usize,
    pub last_ts_ms: Option<u128>,
}

#[derive(Debug, Clone, Default)]
pub struct ToolAuditSummary {
    pub tool: String,
    pub total: usize,
    pub allow: usize,
    pub deny: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ReasonAuditSummary {
    pub reason: String,
    pub total: usize,
    pub allow: usize,
    pub deny: usize,
}

pub fn audit_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".asi_audit.jsonl")
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn log_permission_decision(
    provider: &str,
    model: &str,
    permission_mode: &str,
    tool: &str,
    args: &str,
    allowed: bool,
    reason: &str,
) -> Result<(), String> {
    let line = json!({
        "ts_ms": now_millis(),
        "event": "permission_decision",
        "provider": provider,
        "model": model,
        "permission_mode": permission_mode,
        "tool": tool,
        "args_preview": truncate(args, 200),
        "allowed": allowed,
        "reason": reason,
    });

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(audit_path())
        .map_err(|e| e.to_string())?;
    let payload = serde_json::to_string(&line).map_err(|e| e.to_string())?;
    file.write_all(payload.as_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())
}

pub fn log_auto_review_decision(
    provider: &str,
    model: &str,
    permission_mode: &str,
    auto_review_mode: &str,
    auto_review_threshold: &str,
    tool: &str,
    args: &str,
    severity: &str,
    blocked: bool,
    reason: &str,
) -> Result<(), String> {
    let line = json!({
        "ts_ms": now_millis(),
        "event": "auto_review_decision",
        "provider": provider,
        "model": model,
        "permission_mode": permission_mode,
        "auto_review_mode": auto_review_mode,
        "auto_review_threshold": auto_review_threshold,
        "tool": tool,
        "args_preview": truncate(args, 200),
        "severity": severity,
        "blocked": blocked,
        "reason": reason,
    });

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(audit_path())
        .map_err(|e| e.to_string())?;
    let payload = serde_json::to_string(&line).map_err(|e| e.to_string())?;
    file.write_all(payload.as_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())
}

pub fn log_voice_event(
    provider: &str,
    model: &str,
    permission_mode: &str,
    action: &str,
    ok: bool,
    detail: &str,
) -> Result<(), String> {
    let line = json!({
        "ts_ms": now_millis(),
        "event": "voice_event",
        "provider": provider,
        "model": model,
        "permission_mode": permission_mode,
        "action": action,
        "ok": ok,
        "detail": truncate(detail, 300),
    });

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(audit_path())
        .map_err(|e| e.to_string())?;
    let payload = serde_json::to_string(&line).map_err(|e| e.to_string())?;
    file.write_all(payload.as_bytes())
        .map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())
}
pub fn read_tail(limit: usize) -> Result<String, String> {
    let lines = list_recent_lines(limit.clamp(1, 500))?;
    if lines.is_empty() {
        return Ok("audit log is empty".to_string());
    }
    Ok(lines.join("\n"))
}

pub fn list_recent_lines(limit: usize) -> Result<Vec<String>, String> {
    read_recent_lines(limit.clamp(1, 5000))
}

pub fn summarize_recent(limit: usize) -> Result<AuditSummary, String> {
    let lines = read_recent_lines(limit.clamp(1, 5000))?;
    if lines.is_empty() {
        return Ok(AuditSummary::default());
    }

    let mut out = AuditSummary::default();

    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        if v.get("event").and_then(|x| x.as_str()) != Some("permission_decision") {
            continue;
        }

        out.total += 1;
        if v.get("allowed").and_then(|x| x.as_bool()).unwrap_or(false) {
            out.allow += 1;
        } else {
            out.deny += 1;
        }

        if let Some(ts) = v.get("ts_ms").and_then(|x| x.as_u64()) {
            out.last_ts_ms = Some(ts as u128);
        }
    }

    Ok(out)
}

pub fn summarize_recent_by_tool(limit: usize) -> Result<Vec<ToolAuditSummary>, String> {
    let lines = read_recent_lines(limit.clamp(1, 5000))?;
    if lines.is_empty() {
        return Ok(Vec::new());
    }

    let mut map: BTreeMap<String, ToolAuditSummary> = BTreeMap::new();

    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        if v.get("event").and_then(|x| x.as_str()) != Some("permission_decision") {
            continue;
        }

        let tool = v
            .get("tool")
            .and_then(|x| x.as_str())
            .unwrap_or("unknown")
            .to_string();
        let allowed = v.get("allowed").and_then(|x| x.as_bool()).unwrap_or(false);

        let row = map.entry(tool.clone()).or_insert_with(|| ToolAuditSummary {
            tool,
            total: 0,
            allow: 0,
            deny: 0,
        });

        row.total += 1;
        if allowed {
            row.allow += 1;
        } else {
            row.deny += 1;
        }
    }

    let mut rows = map.into_values().collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.deny
            .cmp(&a.deny)
            .then(b.total.cmp(&a.total))
            .then(a.tool.cmp(&b.tool))
    });

    Ok(rows)
}

pub fn summarize_recent_by_reason(limit: usize) -> Result<Vec<ReasonAuditSummary>, String> {
    let lines = read_recent_lines(limit.clamp(1, 5000))?;
    if lines.is_empty() {
        return Ok(Vec::new());
    }

    let mut map: BTreeMap<String, ReasonAuditSummary> = BTreeMap::new();

    for line in lines {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        if v.get("event").and_then(|x| x.as_str()) != Some("permission_decision") {
            continue;
        }

        let reason = v
            .get("reason")
            .and_then(|x| x.as_str())
            .unwrap_or("unknown")
            .to_string();
        let allowed = v.get("allowed").and_then(|x| x.as_bool()).unwrap_or(false);

        let row = map
            .entry(reason.clone())
            .or_insert_with(|| ReasonAuditSummary {
                reason,
                total: 0,
                allow: 0,
                deny: 0,
            });

        row.total += 1;
        if allowed {
            row.allow += 1;
        } else {
            row.deny += 1;
        }
    }

    let mut rows = map.into_values().collect::<Vec<_>>();
    rows.sort_by(|a, b| {
        b.deny
            .cmp(&a.deny)
            .then(b.total.cmp(&a.total))
            .then(a.reason.cmp(&b.reason))
    });

    Ok(rows)
}

pub fn export_report(path: &Path, format: &str, window: usize) -> Result<(), String> {
    let n = window.clamp(1, 5000);
    let summary = summarize_recent(n)?;
    let tools = summarize_recent_by_tool(n)?;
    let reasons = summarize_recent_by_reason(n)?;
    let lines = list_recent_lines(n)?;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }

    match format {
        "json" => {
            let entries = lines
                .iter()
                .map(|line| {
                    serde_json::from_str::<Value>(line).unwrap_or_else(|_| json!({"raw": line}))
                })
                .collect::<Vec<_>>();

            let payload = json!({
                "generated_at_ms": now_millis(),
                "window": n,
                "summary": {
                    "total": summary.total,
                    "allow": summary.allow,
                    "deny": summary.deny,
                    "last_ts_ms": summary.last_ts_ms,
                },
                "tools": tools.iter().map(|x| json!({
                    "tool": x.tool,
                    "total": x.total,
                    "allow": x.allow,
                    "deny": x.deny,
                })).collect::<Vec<_>>(),
                "reasons": reasons.iter().map(|x| json!({
                    "reason": x.reason,
                    "total": x.total,
                    "allow": x.allow,
                    "deny": x.deny,
                })).collect::<Vec<_>>(),
                "entries": entries,
            });
            let body = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
            fs::write(path, body).map_err(|e| e.to_string())
        }
        "md" => {
            let body = render_markdown_report(n, &summary, &tools, &reasons, &lines);
            fs::write(path, body).map_err(|e| e.to_string())
        }
        _ => Err("unsupported export format; use md or json".to_string()),
    }
}

fn render_markdown_report(
    window: usize,
    summary: &AuditSummary,
    tools: &[ToolAuditSummary],
    reasons: &[ReasonAuditSummary],
    lines: &[String],
) -> String {
    let mut out = String::new();
    out.push_str("# ASI Code Audit Report\n\n");
    out.push_str(&format!("GeneratedAtMs: {}\n\n", now_millis()));
    out.push_str(&format!("Window: {}\n\n", window));

    out.push_str("## Summary\n\n");
    out.push_str(&format!(
        "- Total: {}\n- Allow: {}\n- Deny: {}\n- LastTsMs: {}\n\n",
        summary.total,
        summary.allow,
        summary.deny,
        summary
            .last_ts_ms
            .map(|x| x.to_string())
            .unwrap_or_else(|| "none".to_string())
    ));

    out.push_str("## Tools\n\n");
    if tools.is_empty() {
        out.push_str("- none\n\n");
    } else {
        for row in tools.iter().take(50) {
            out.push_str(&format!(
                "- {} total={} allow={} deny={}\n",
                row.tool, row.total, row.allow, row.deny
            ));
        }
        out.push('\n');
    }

    out.push_str("## Reasons\n\n");
    if reasons.is_empty() {
        out.push_str("- none\n\n");
    } else {
        for row in reasons.iter().take(50) {
            out.push_str(&format!(
                "- {} total={} allow={} deny={}\n",
                row.reason, row.total, row.allow, row.deny
            ));
        }
        out.push('\n');
    }

    out.push_str("## Entries (JSONL)\n\n");
    out.push_str("```json\n");
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("```\n");

    out
}

fn read_recent_lines(limit: usize) -> Result<Vec<String>, String> {
    let path = audit_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut lines = content
        .lines()
        .filter(|x| !x.trim().is_empty())
        .map(|x| x.to_string())
        .collect::<Vec<_>>();

    if lines.len() > limit {
        lines = lines[lines.len() - limit..].to_vec();
    }

    Ok(lines)
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = text.chars().take(max).collect::<String>();
    out.push_str("...");
    out
}
