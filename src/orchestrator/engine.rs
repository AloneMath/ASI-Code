use super::parser;
use super::scheduler;
use super::types::ExecutionBatch;

pub struct OrchestratorEngine {
    max_steps: usize,
    max_consecutive_failures: usize,
}

impl OrchestratorEngine {
    pub fn new(max_steps: usize, max_consecutive_failures: usize) -> Self {
        Self {
            max_steps,
            max_consecutive_failures,
        }
    }

    pub fn parse_and_plan(&self, text: &str) -> Vec<ExecutionBatch> {
        let calls = parser::extract_tool_calls(text);
        scheduler::partition_tool_calls(calls)
    }

    pub fn stop_reason(&self, steps: usize, consecutive_failures: usize) -> Option<&'static str> {
        if steps >= self.max_steps {
            return Some("reached max tool steps");
        }

        if consecutive_failures >= self.max_consecutive_failures {
            return Some("repeated tool failures");
        }

        None
    }

    pub fn followup_prompt(&self) -> &'static str {
        "Continue the task using latest tool results. If more local actions are required, output ALL needed /toolcall lines in one response (batch them, one line per call). For read_file paging, use the exact next start_line from the previous tool header; do not overlap ranges. If no more actions are needed, provide the final answer."
    }

    pub fn followup_prompt_strict(&self) -> &'static str {
        "Continue using latest tool results under STRICT mode. If actions are needed, output ONLY plain /toolcall lines (no prose/code fences). Include concrete validation toolcalls after edits; if validation fails, emit corrective /toolcall lines and retry. For read_file paging, use exact next start_line without overlap. If no more actions are needed, provide final answer with changed files and validation summary."
    }

    pub fn followup_prompt_for(&self, strict_mode: bool) -> &'static str {
        if strict_mode {
            self.followup_prompt_strict()
        } else {
            self.followup_prompt()
        }
    }
}
