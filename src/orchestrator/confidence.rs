use crate::clip_chars;
use crate::orchestrator::types::ExecutionBatch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfidenceLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConfidenceDeclaration {
    pub(crate) level: ConfidenceLevel,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ConfidenceGateStats {
    pub(crate) checks: usize,
    pub(crate) declaration_missing: usize,
    pub(crate) declaration_low: usize,
    pub(crate) blocked_risky_toolcalls: usize,
    pub(crate) retries_exhausted: usize,
}

impl ConfidenceGateStats {
    pub(crate) fn stats_line(&self) -> String {
        format!(
            "confidence_gate checks={} missing_declaration={} low_declaration={} blocked_risky_toolcalls={} retries_exhausted={}",
            self.checks,
            self.declaration_missing,
            self.declaration_low,
            self.blocked_risky_toolcalls,
            self.retries_exhausted
        )
    }

    pub(crate) fn checks(&self) -> usize {
        self.checks
    }

    pub(crate) fn declaration_missing(&self) -> usize {
        self.declaration_missing
    }

    pub(crate) fn declaration_low(&self) -> usize {
        self.declaration_low
    }

    pub(crate) fn blocked_risky_toolcalls(&self) -> usize {
        self.blocked_risky_toolcalls
    }

    pub(crate) fn retries_exhausted(&self) -> usize {
        self.retries_exhausted
    }
}

pub(crate) const CONFIDENCE_GATE_MAX_RETRIES: usize = 2;

pub(crate) fn parse_confidence_level(raw: &str) -> Option<ConfidenceLevel> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "low" => Some(ConfidenceLevel::Low),
        "medium" | "med" => Some(ConfidenceLevel::Medium),
        "high" => Some(ConfidenceLevel::High),
        _ => None,
    }
}

pub(crate) fn parse_confidence_declaration(text: &str) -> Option<ConfidenceDeclaration> {
    let mut saw_declaration = false;
    let mut level: Option<ConfidenceLevel> = None;
    let mut reason: Option<String> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower == "confidence declaration:" {
            saw_declaration = true;
            continue;
        }
        if !saw_declaration {
            continue;
        }
        if lower.starts_with("confidence_level:") {
            let raw = line.split_once(':').map(|(_, v)| v.trim()).unwrap_or("");
            level = parse_confidence_level(raw);
            continue;
        }
        if lower.starts_with("reason:") {
            let raw = line
                .split_once(':')
                .map(|(_, v)| v.trim())
                .unwrap_or("")
                .to_string();
            if !raw.is_empty() {
                reason = Some(raw);
            }
            continue;
        }
        if line.starts_with("/toolcall ") {
            break;
        }
    }

    match (level, reason) {
        (Some(level), Some(reason)) => Some(ConfidenceDeclaration { level, reason }),
        _ => None,
    }
}

pub(crate) fn is_risky_tool_name(name: &str) -> bool {
    matches!(name, "write_file" | "edit_file" | "bash")
}

pub(crate) fn toolcall_count_by_risk(batches: &[ExecutionBatch]) -> (usize, usize) {
    let mut risky = 0usize;
    let mut safe = 0usize;
    for batch in batches {
        for call in &batch.calls {
            if is_risky_tool_name(call.name.as_str()) {
                risky += 1;
            } else {
                safe += 1;
            }
        }
    }
    (risky, safe)
}

pub(crate) fn build_confidence_gate_prompt(
    context_contract: &str,
    original_text: &str,
    risky_calls: usize,
    safe_calls: usize,
) -> String {
    let assistant_excerpt = clip_chars(original_text.trim(), 600);
    format!(
        "{context_contract}\n\nConfidence Gate: detected {risky_calls} risky toolcall(s) (write_file/edit_file/bash) and {safe_calls} non-risky toolcall(s) in your previous response.\nBefore any risky toolcall execution, output exactly this block first:\nConfidence Declaration:\nconfidence_level: <low|medium|high>\nreason: <one concise sentence>\nThen output toolcalls.\nIf confidence_level is low, output only read_file/glob_search/grep_search/web_search/web_fetch toolcalls and avoid write_file/edit_file/bash for this turn.\n\nPrevious assistant output excerpt:\n{assistant_excerpt}"
    )
}

pub(crate) fn confidence_low_block_reason(
    declaration: Option<&ConfidenceDeclaration>,
) -> String {
    let reason = declaration
        .map(|d| d.reason.as_str())
        .unwrap_or("insufficient confidence declaration");
    format!(
        "blocked by user constraint: blocked by strict confidence gate due to low confidence for risky toolcalls; choose read-first actions only (reason: {})",
        clip_chars(reason, 120)
    )
}
