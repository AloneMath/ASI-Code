use crate::audit;
use crate::config::{default_model, resolve_model_alias};
use crate::cost;
use crate::meta::DEFAULT_SYSTEM_PROMPT;
use crate::permissions;
use crate::plugin;
use crate::provider::{
    tool_call_to_legacy_args, ChatMessage, ChatProvider, CompletionResult, ProviderClient,
    StreamingResult, ToolCallSpec,
};
use crate::orchestrator::types::{classify_tool_call, ToolCallKind};
use crate::sandbox::{SandboxPreflightContext, SandboxStrategy};
use crate::security;
use crate::tools::{DefaultToolRunner, ToolResult, ToolRunner};
use serde_json::json;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const LENGTH_CONTINUATION_PROMPT: &str =
    "Continue from exactly where you stopped due to length. Do not repeat prior text. Finish the remaining response.";

const EMPTY_LENGTH_HINT: &str = "[no answer] The model spent its entire output budget on hidden reasoning before writing a reply (stop_reason=length). Retry with a narrower question, raise the limit (e.g. ASI_MAX_TOKENS_DEEPSEEK=65536), or lower thinking effort (ASI_DEEPSEEK_REASONING_EFFORT=high).";

#[derive(Debug, Clone)]
pub struct TurnResult {
    pub text: String,
    pub stop_reason: String,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub is_tool_result: bool,
    pub turn_cost_usd: f64,
    pub total_cost_usd: f64,
    pub thinking: Option<String>,
    /// Native tool calls returned by the API (if any).
    pub native_tool_calls: Vec<NativeToolResult>,
}

/// Result of executing a single native tool call.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct NativeToolResult {
    pub tool_call_id: String,
    pub tool_name: String,
    pub tool_args_display: String,
    pub result_text: String,
    pub ok: bool,
}

#[derive(Debug)]
struct ConcurrentToolOutcome {
    index: usize,
    command: String,
    tool_name: String,
    tool_args: String,
    text: String,
    stop_reason: String,
    is_tool_result: bool,
    audit_allowed: Option<bool>,
    audit_reason: String,
}

#[derive(Debug, Clone)]
struct ConcurrentToolExecShared {
    provider: String,
    model: String,
    permission_mode: String,
    auto_review_mode: String,
    auto_review_severity_threshold: String,
    safe_shell_mode: bool,
    disable_web_tools: bool,
    disable_bash_tool: bool,
    allow_rules: Vec<String>,
    deny_rules: Vec<String>,
    path_restriction_enabled: bool,
    workspace_root: PathBuf,
    additional_directories: Vec<PathBuf>,
    sandbox_strategy: SandboxStrategy,
    tool_runner: Arc<dyn ToolRunner>,
    hooks: HookConfig,
}

#[derive(Debug, Clone, Default)]
struct HookConfig {
    pre_tool_use: Option<String>,
    permission_request: Option<String>,
    post_tool_use: Option<String>,
    session_start: Option<String>,
    user_prompt_submit: Option<String>,
    stop: Option<String>,
    subagent_stop: Option<String>,
    pre_compact: Option<String>,
    post_compact: Option<String>,
    timeout_secs: u64,
    json_protocol: bool,
    failure_policy: HookFailurePolicy,
    handlers: Vec<HookHandlerConfig>,
}

#[derive(Debug, Clone)]
struct HookHandlerConfig {
    event: String,
    script: String,
    timeout_secs: Option<u64>,
    json_protocol: Option<bool>,
    tool_prefix: Option<String>,
    permission_mode: Option<String>,
    failure_policy: Option<HookFailurePolicy>,
}

#[derive(Debug, Clone)]
struct HookOutcome {
    allow: bool,
    reason: String,
    is_error: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HookDiagnostic {
    pub source: String,
    pub event: String,
    pub allow: bool,
    pub is_error: bool,
    pub reason: String,
}

#[derive(Debug, Clone)]
struct PluginHookContext {
    names_csv: String,
    count: usize,
}

#[derive(Debug, Clone)]
struct PluginHookExecution {
    script: String,
    timeout_secs: Option<u64>,
    json_protocol: Option<bool>,
    tool_prefix: Option<String>,
    permission_mode: Option<String>,
    failure_policy: Option<HookFailurePolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookFailurePolicy {
    FailClosed,
    FailOpen,
}

impl Default for HookFailurePolicy {
    fn default() -> Self {
        Self::FailClosed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelCapabilities {
    pub native_tool_calling_default: bool,
    pub native_tool_calling_supported: bool,
    pub recommended_max_output_tokens: u64,
    /// When native tool calling is active, only accept canonical
    /// line-leading `/toolcall` text as a fallback (no fenced-block
    /// promotion or pseudo-XML heuristics). Enabled for providers whose
    /// native tool_use is reliable, so prose or code samples in a final
    /// answer are never mistaken for actions.
    pub strict_text_toolcall_fallback: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderErrorKind {
    NativeToolUnsupported,
    ModelNotFound,
    Decode,
    RetryableNetwork,
    Auth,
    Quota,
    Unknown,
}

pub struct Runtime {
    pub provider: String,
    pub model: String,
    pub permission_mode: String,
    pub auto_review_mode: String,
    pub auto_review_severity_threshold: String,
    pub max_turns: usize,
    pub client: Box<dyn ChatProvider>,
    pub tool_runner: Arc<dyn ToolRunner>,
    pub messages: Vec<ChatMessage>,
    pub cumulative_input_tokens: usize,
    pub cumulative_output_tokens: usize,
    pub cumulative_cost_usd: f64,
    pub extended_thinking: bool,
    pub disable_web_tools: bool,
    pub disable_bash_tool: bool,
    pub safe_shell_mode: bool,
    pub permission_allow_rules: Vec<String>,
    pub permission_deny_rules: Vec<String>,
    pub path_restriction_enabled: bool,
    pub workspace_root: PathBuf,
    pub additional_directories: Vec<PathBuf>,
    pub session_permission_allow_rules: Vec<String>,
    pub session_permission_deny_rules: Vec<String>,
    pub session_additional_directories: Vec<PathBuf>,
    pub next_permission_allow_rules: Vec<String>,
    pub next_additional_directories: Vec<PathBuf>,
    pub interactive_approval_allow_rules: Vec<String>,
    pub interactive_approval_deny_rules: Vec<String>,
    /// Enable native tool calling via API (tool_use / function calling).
    pub native_tool_calling: bool,
    /// Project context loaded from CLAUDE.md, README.md, git status, etc.
    #[allow(dead_code)]
    pub project_context: String,
    pub project_context_sources: Vec<String>,
    pub sandbox_strategy: SandboxStrategy,
    provider_decode_retry_count: usize,
    provider_decode_final_fail_count: usize,
    native_tool_auto_disabled_reason: Option<String>,
    last_provider_error_kind: Option<ProviderErrorKind>,
    last_provider_error_message: Option<String>,
    last_stop_reason_raw: Option<String>,
    last_stop_reason_alias: Option<String>,
}

#[derive(Debug, Clone)]
struct ProjectContextBundle {
    text: String,
    sources: Vec<String>,
}

impl Runtime {
    fn read_doc_context(path: &Path, label: &str) -> Option<String> {
        let content = std::fs::read_to_string(path).ok()?;
        if content.trim().is_empty() {
            return None;
        }
        Some(format!("--- {} ---\n{}", label, content))
    }

    /// Load project context from CLAUDE.md, README.md, AGENTS.md, git status, etc.
    fn load_project_context(workspace_root: &PathBuf) -> ProjectContextBundle {
        let claude_single_mode = Self::claude_single_mode_enabled();
        Self::load_project_context_with_mode(workspace_root, claude_single_mode)
    }

    fn claude_single_mode_from_env(raw: Option<&str>) -> bool {
        raw.and_then(Self::parse_boolish).unwrap_or(true)
    }

    fn claude_single_mode_enabled() -> bool {
        let raw = std::env::var("ASI_CLAUDE_SINGLE").ok();
        Self::claude_single_mode_from_env(raw.as_deref())
    }

    fn load_project_context_with_mode(
        workspace_root: &PathBuf,
        claude_single_mode: bool,
    ) -> ProjectContextBundle {
        use std::process::Command;

        let mut context_parts = Vec::new();
        let mut sources = Vec::new();

        let layered_mode = std::env::var("ASI_PROJECT_INSTRUCTIONS_LAYERED")
            .ok()
            .and_then(|v| Self::parse_boolish(&v))
            .unwrap_or(true);

        let doc_roots = if layered_mode {
            Self::workspace_ancestor_chain(workspace_root)
        } else {
            vec![workspace_root.clone()]
        };

        if claude_single_mode {
            let mut loaded_any_claude = false;
            for root in &doc_roots {
                for filename in Self::claude_instruction_candidates() {
                    let claude_path = root.join(filename);
                    if let Some(part) = Self::read_doc_context(&claude_path, filename) {
                        context_parts.push(part);
                        sources.push(Self::normalize_path_for_source(
                            &claude_path,
                            workspace_root,
                        ));
                        loaded_any_claude = true;
                    }
                }
            }

            if !loaded_any_claude {
                for root in &doc_roots {
                    for filename in Self::fallback_instruction_candidates() {
                        let path = root.join(filename);
                        if let Some(part) = Self::read_doc_context(&path, filename) {
                            context_parts.push(part);
                            sources.push(Self::normalize_path_for_source(&path, workspace_root));
                        }
                    }
                }
            }
        } else {
            for root in &doc_roots {
                for filename in Self::all_instruction_candidates() {
                    let path = root.join(filename);
                    if let Some(part) = Self::read_doc_context(&path, filename) {
                        context_parts.push(part);
                        sources.push(Self::normalize_path_for_source(&path, workspace_root));
                    }
                }
            }
        }

        // Get git status if available
        if let Ok(output) = Command::new("git")
            .arg("status")
            .arg("--short")
            .current_dir(workspace_root)
            .output()
        {
            if output.status.success() {
                let status = String::from_utf8_lossy(&output.stdout);
                if !status.trim().is_empty() {
                    context_parts.push(format!("--- Git Status ---\n{}", status));
                    sources.push("git:status".to_string());
                }
            }
        }

        // Get recent git commits
        if let Ok(output) = Command::new("git")
            .arg("log")
            .arg("--oneline")
            .arg("-5")
            .current_dir(workspace_root)
            .output()
        {
            if output.status.success() {
                let log = String::from_utf8_lossy(&output.stdout);
                if !log.trim().is_empty() {
                    context_parts.push(format!("--- Recent Git Commits (last 5) ---\n{}", log));
                    sources.push("git:log".to_string());
                }
            }
        }

        let text = if context_parts.is_empty() {
            String::new()
        } else {
            format!("\n\n## Project Context\n\n{}", context_parts.join("\n\n"))
        };

        ProjectContextBundle { text, sources }
    }

    fn claude_instruction_candidates() -> &'static [&'static str] {
        &["CLAUDE.override.md", "CLAUDE.local.md", "CLAUDE.md"]
    }

    fn fallback_instruction_candidates() -> &'static [&'static str] {
        &[
            "AGENTS.override.md",
            "AGENTS.local.md",
            "AGENTS.md",
            "README.override.md",
            "README.local.md",
            "README.md",
        ]
    }

    fn all_instruction_candidates() -> &'static [&'static str] {
        &[
            "CLAUDE.override.md",
            "CLAUDE.local.md",
            "CLAUDE.md",
            "AGENTS.override.md",
            "AGENTS.local.md",
            "AGENTS.md",
            "README.override.md",
            "README.local.md",
            "README.md",
        ]
    }

    fn workspace_ancestor_chain(workspace_root: &PathBuf) -> Vec<PathBuf> {
        let max_levels = std::env::var("ASI_PROJECT_INSTRUCTIONS_MAX_LEVELS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(4)
            .clamp(1, 16);
        let mut stack = Vec::new();
        let mut cur = Some(workspace_root.as_path());
        while let Some(p) = cur {
            stack.push(p.to_path_buf());
            if stack.len() >= max_levels {
                break;
            }
            cur = p.parent();
        }
        stack.reverse();
        stack
    }

    fn normalize_path_for_source(path: &Path, workspace_root: &Path) -> String {
        if let Ok(rel) = path.strip_prefix(workspace_root) {
            let s = rel.to_string_lossy().replace('\\', "/");
            if s.is_empty() {
                ".".to_string()
            } else {
                s
            }
        } else {
            path.to_string_lossy().replace('\\', "/")
        }
    }

    pub fn new(provider: String, model: String, permission_mode: String, max_turns: usize) -> Self {
        let client: Box<dyn ChatProvider> =
            Box::new(ProviderClient::new(provider.clone(), model.clone()));
        let native = Self::default_native_tool_calling(&provider, &model);
        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let project_context_bundle = Self::load_project_context(&workspace_root);
        let sandbox_strategy = SandboxStrategy::from_env();
        let tool_runner: Arc<dyn ToolRunner> = Arc::new(DefaultToolRunner);

        // Combine system prompt with project context if available
        let system_prompt = if project_context_bundle.text.is_empty() {
            DEFAULT_SYSTEM_PROMPT.to_string()
        } else {
            format!(
                "{}\n\n{}",
                DEFAULT_SYSTEM_PROMPT, project_context_bundle.text
            )
        };

        let rt = Self {
            provider,
            model,
            permission_mode,
            auto_review_mode: "off".to_string(),
            auto_review_severity_threshold: "high".to_string(),
            max_turns,
            client,
            tool_runner,
            messages: vec![ChatMessage::text("system".to_string(), system_prompt)],
            cumulative_input_tokens: 0,
            cumulative_output_tokens: 0,
            cumulative_cost_usd: 0.0,
            extended_thinking: false,
            disable_web_tools: false,
            disable_bash_tool: false,
            safe_shell_mode: true,
            permission_allow_rules: Vec::new(),
            permission_deny_rules: Vec::new(),
            path_restriction_enabled: true,
            workspace_root,
            additional_directories: Vec::new(),
            session_permission_allow_rules: Vec::new(),
            session_permission_deny_rules: Vec::new(),
            session_additional_directories: Vec::new(),
            next_permission_allow_rules: Vec::new(),
            next_additional_directories: Vec::new(),
            interactive_approval_allow_rules: Vec::new(),
            interactive_approval_deny_rules: Vec::new(),
            native_tool_calling: native,
            project_context: project_context_bundle.text,
            project_context_sources: project_context_bundle.sources,
            sandbox_strategy,
            provider_decode_retry_count: 0,
            provider_decode_final_fail_count: 0,
            native_tool_auto_disabled_reason: None,
            last_provider_error_kind: None,
            last_provider_error_message: None,
            last_stop_reason_raw: None,
            last_stop_reason_alias: None,
        };
        let hooks = Self::load_hook_config();
        let _ = run_event_hooks(
            &hooks,
            "SessionStart",
            "runtime",
            "runtime_initialized",
            &rt.permission_mode,
        );
        rt
    }

    pub fn default_native_tool_calling(provider: &str, model: &str) -> bool {
        Self::capabilities_for(provider, model).native_tool_calling_default
    }

    pub fn capabilities_for(provider: &str, model: &str) -> ModelCapabilities {
        let provider = provider.trim().to_ascii_lowercase();
        let model = model.trim().to_ascii_lowercase();
        let model_is_deepseek_reasoner = model.contains("reasoner") && model.contains("deepseek");

        if model_is_deepseek_reasoner {
            return ModelCapabilities {
                native_tool_calling_default: false,
                native_tool_calling_supported: false,
                // Thinking model: matches provider::max_tokens_for_provider.
                recommended_max_output_tokens: 32_768,
                strict_text_toolcall_fallback: false,
            };
        }

        if provider == "deepseek" {
            let native = model.contains("deepseek-v4");
            return ModelCapabilities {
                native_tool_calling_default: native,
                native_tool_calling_supported: native,
                // v4-pro runs with reasoning enabled; thinking needs headroom.
                recommended_max_output_tokens: if model.contains("v4-pro") { 32_768 } else { 4096 },
                // DeepSeek often emits text-form calls even in native mode.
                strict_text_toolcall_fallback: false,
            };
        }

        if provider == "claude" {
            return ModelCapabilities {
                native_tool_calling_default: true,
                native_tool_calling_supported: true,
                recommended_max_output_tokens: 32_768,
                strict_text_toolcall_fallback: true,
            };
        }

        if provider == "openai" {
            return ModelCapabilities {
                native_tool_calling_default: true,
                native_tool_calling_supported: true,
                recommended_max_output_tokens: 16_384,
                strict_text_toolcall_fallback: true,
            };
        }

        ModelCapabilities {
            native_tool_calling_default: false,
            native_tool_calling_supported: false,
            recommended_max_output_tokens: 8192,
            strict_text_toolcall_fallback: false,
        }
    }

    /// True when text-form toolcalls should be parsed strictly: native tool
    /// calling is active and the provider's native protocol is reliable.
    pub fn strict_text_toolcall_parsing(&self) -> bool {
        self.native_tool_calling
            && Self::capabilities_for(&self.provider, &self.model).strict_text_toolcall_fallback
    }

    pub fn run_turn(&mut self, user_text: &str) -> TurnResult {
        self.run_turn_streaming(user_text, |_| {})
    }

    pub fn run_turn_streaming<F>(&mut self, user_text: &str, mut on_delta: F) -> TurnResult
    where
        F: FnMut(&str),
    {
        let hooks = Self::load_hook_config();
        for outcome in run_event_hooks(
            &hooks,
            "UserPromptSubmit",
            "runtime",
            user_text,
            &self.permission_mode,
        ) {
            if !outcome.allow {
                let reason = format!("hook UserPromptSubmit denied: {}", outcome.reason);
                self.record_stop_reason("permission_denied");
                return TurnResult {
                    text: format!("Permission denied: {}", reason),
                    stop_reason: "permission_denied".to_string(),
                    input_tokens: 0,
                    output_tokens: 0,
                    total_input_tokens: self.cumulative_input_tokens,
                    total_output_tokens: self.cumulative_output_tokens,
                    is_tool_result: false,
                    turn_cost_usd: 0.0,
                    total_cost_usd: self.cumulative_cost_usd,
                    thinking: None,
                    native_tool_calls: Vec::new(),
                };
            }
        }

        let turns = self.turn_count();
        if turns >= self.max_turns {
            self.record_stop_reason("max_turns_reached");
            return TurnResult {
                text: "Max turns reached".to_string(),
                stop_reason: "max_turns_reached".to_string(),
                input_tokens: 0,
                output_tokens: 0,
                total_input_tokens: self.cumulative_input_tokens,
                total_output_tokens: self.cumulative_output_tokens,
                is_tool_result: false,
                turn_cost_usd: 0.0,
                total_cost_usd: self.cumulative_cost_usd,
                thinking: None,
                native_tool_calls: Vec::new(),
            };
        }

        // Handle manual /toolcall commands
        if let Some(rest) = user_text.strip_prefix("/toolcall ") {
            return self.handle_manual_toolcall(rest, user_text);
        }

        let input_tokens = estimate_tokens(user_text);
        self.messages
            .push(ChatMessage::text("user".to_string(), user_text.to_string()));

        let thinking = if self.extended_thinking {
            Some(generate_thinking(user_text))
        } else {
            None
        };

        // Try native tool calling first
        if self.native_tool_calling {
            return self.run_native_tool_turn(input_tokens, thinking, &mut on_delta);
        }

        // Fallback: legacy text-based streaming
        self.run_legacy_text_turn(input_tokens, thinking, &mut on_delta)
    }

    /// Whether the last history entry is a role="tool" result, meaning the
    /// native protocol expects the assistant to be re-invoked with no new
    /// user message (the model continues from its own tool results).
    pub fn last_message_is_tool_result(&self) -> bool {
        self.messages
            .last()
            .map(|m| m.role == "tool")
            .unwrap_or(false)
    }

    /// Continue the conversation after native tool results WITHOUT pushing a
    /// synthetic user message. This is how tool-trained models expect the
    /// loop to run: assistant(tool_calls) -> tool results -> assistant
    /// continues. Injecting "please continue" user prompts between rounds
    /// (the old behavior) both wasted tokens and pushed the model
    /// off-distribution.
    pub fn run_continuation_streaming<F>(&mut self, mut on_delta: F) -> TurnResult
    where
        F: FnMut(&str),
    {
        let turns = self.turn_count();
        if turns >= self.max_turns {
            self.record_stop_reason("max_turns_reached");
            return TurnResult {
                text: "Max turns reached".to_string(),
                stop_reason: "max_turns_reached".to_string(),
                input_tokens: 0,
                output_tokens: 0,
                total_input_tokens: self.cumulative_input_tokens,
                total_output_tokens: self.cumulative_output_tokens,
                is_tool_result: false,
                turn_cost_usd: 0.0,
                total_cost_usd: self.cumulative_cost_usd,
                thinking: None,
                native_tool_calls: Vec::new(),
            };
        }
        self.run_native_tool_turn(0, None, &mut on_delta)
    }

    fn provider_retry_limit() -> usize {
        std::env::var("ASI_PROVIDER_MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
            .clamp(0, 3)
    }

    fn provider_retry_backoff() -> Duration {
        let ms = std::env::var("ASI_PROVIDER_RETRY_BACKOFF_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(800)
            .clamp(0, 10_000);
        Duration::from_millis(ms)
    }

    fn model_auto_fallback_enabled() -> bool {
        std::env::var("ASI_MODEL_AUTO_FALLBACK")
            .ok()
            .and_then(|v| Self::parse_boolish(&v))
            .unwrap_or(true)
    }

    fn load_hook_config() -> HookConfig {
        let explicit = std::env::var("ASI_HOOKS_ENABLED")
            .ok()
            .and_then(|v| Self::parse_boolish(&v));
        // Hooks installed at user level (`asi skills install`) are on without
        // any environment setup; ASI_HOOKS_ENABLED=0 still turns them off.
        // Project-level `.asi/hooks.json` is deliberately NOT auto-loaded: a
        // cloned repository must never be able to run code just by being
        // opened.
        let user_hooks = user_hooks_config_path().filter(|p| p.is_file());
        let hook_enabled = explicit.unwrap_or(user_hooks.is_some());
        if !hook_enabled {
            return HookConfig::default();
        }

        let read = |key: &str| {
            std::env::var(key)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };

        let timeout_secs = std::env::var("ASI_HOOK_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(15)
            .clamp(1, 60);
        let json_protocol = std::env::var("ASI_HOOK_JSON")
            .ok()
            .and_then(|v| Self::parse_boolish(&v))
            .unwrap_or(true);
        let failure_policy = std::env::var("ASI_HOOK_FAILURE_POLICY")
            .ok()
            .and_then(|v| parse_hook_failure_policy(&v))
            .unwrap_or(HookFailurePolicy::FailClosed);

        let mut cfg = HookConfig {
            pre_tool_use: read("ASI_HOOK_PRE_TOOL_USE"),
            permission_request: read("ASI_HOOK_PERMISSION_REQUEST"),
            post_tool_use: read("ASI_HOOK_POST_TOOL_USE"),
            session_start: read("ASI_HOOK_SESSION_START"),
            user_prompt_submit: read("ASI_HOOK_USER_PROMPT_SUBMIT"),
            stop: read("ASI_HOOK_STOP"),
            subagent_stop: read("ASI_HOOK_SUBAGENT_STOP"),
            pre_compact: read("ASI_HOOK_PRE_COMPACT"),
            post_compact: read("ASI_HOOK_POST_COMPACT"),
            timeout_secs,
            json_protocol,
            failure_policy,
            handlers: Vec::new(),
        };

        if let Some(path) = read("ASI_HOOK_CONFIG_PATH") {
            if let Ok(handlers) = Self::load_hook_handlers_from_file(&path) {
                cfg.handlers = handlers;
            }
        } else if let Some(path) = user_hooks {
            if let Ok(handlers) = Self::load_hook_handlers_from_file(&path.to_string_lossy()) {
                cfg.handlers = handlers;
            }
        }
        cfg
    }

    fn load_hook_handlers_from_file(path: &str) -> Result<Vec<HookHandlerConfig>, String> {
        #[derive(Debug, serde::Deserialize)]
        struct HookFile {
            handlers: Option<Vec<HookFileHandler>>,
        }

        #[derive(Debug, serde::Deserialize)]
        struct HookFileHandler {
            event: Option<String>,
            script: Option<String>,
            timeout_secs: Option<u64>,
            json_protocol: Option<bool>,
            tool_prefix: Option<String>,
            permission_mode: Option<String>,
            failure_policy: Option<String>,
        }

        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let parsed = serde_json::from_str::<HookFile>(&text).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in parsed.handlers.unwrap_or_default() {
            let event = row.event.unwrap_or_default().trim().to_string();
            let script = row.script.unwrap_or_default().trim().to_string();
            if event.is_empty() || script.is_empty() {
                continue;
            }
            out.push(HookHandlerConfig {
                event,
                script,
                timeout_secs: row.timeout_secs,
                json_protocol: row.json_protocol,
                tool_prefix: row.tool_prefix.and_then(|v| {
                    let t = v.trim().to_string();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t)
                    }
                }),
                permission_mode: row.permission_mode.and_then(|v| {
                    let t = v.trim().to_string();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t)
                    }
                }),
                failure_policy: row
                    .failure_policy
                    .as_deref()
                    .and_then(parse_hook_failure_policy),
            });
        }
        Ok(out)
    }

    fn is_native_tool_calling_unsupported_error(error: &str) -> bool {
        Self::classify_provider_error(error) == ProviderErrorKind::NativeToolUnsupported
    }

    fn parse_boolish(raw: &str) -> Option<bool> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" | "enabled" => Some(true),
            "0" | "false" | "off" | "no" | "disabled" => Some(false),
            _ => None,
        }
    }

    fn is_retryable_provider_error(error: &str) -> bool {
        matches!(
            Self::classify_provider_error(error),
            ProviderErrorKind::RetryableNetwork | ProviderErrorKind::Decode
        )
    }

    fn is_decode_provider_error(error: &str) -> bool {
        Self::classify_provider_error(error) == ProviderErrorKind::Decode
    }

    fn is_model_not_found_error(error: &str) -> bool {
        Self::classify_provider_error(error) == ProviderErrorKind::ModelNotFound
    }

    fn classify_provider_error(error: &str) -> ProviderErrorKind {
        let lower = error.to_ascii_lowercase();
        if [
            "unsupported parameter",
            "unknown parameter",
            "invalid parameter",
            "parameter tools",
            "does not support tools",
            "tool calls are not supported",
            "tool call is not supported",
            "function calling is not supported",
            "function_call is not supported",
            "tool_choice",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            return ProviderErrorKind::NativeToolUnsupported;
        }

        if [
            "model not exist",
            "model_not_found",
            "unknown model",
            "invalid model",
            "no such model",
            "does not exist",
            "unsupported model",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            return ProviderErrorKind::ModelNotFound;
        }

        if [
            "error decoding response body",
            "error decoding",
            "decode error",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            return ProviderErrorKind::Decode;
        }

        if [
            "401",
            "403",
            "unauthorized",
            "invalid api key",
            "invalid token",
            "forbidden",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            return ProviderErrorKind::Auth;
        }

        if [
            "quota",
            "insufficient_quota",
            "billing",
            "exceeded your current quota",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            return ProviderErrorKind::Quota;
        }

        if [
            "timeout",
            "timed out",
            "connection reset",
            "connection refused",
            "temporarily unavailable",
            "temporary failure",
            "dns",
            "tls",
            "rate limit",
            "too many requests",
            "deadline exceeded",
            "http error: 408",
            "http error: 409",
            "http error: 425",
            "http error: 429",
            "http error: 500",
            "http error: 502",
            "http error: 503",
            "http error: 504",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            return ProviderErrorKind::RetryableNetwork;
        }

        ProviderErrorKind::Unknown
    }

    fn provider_error_kind_label(kind: ProviderErrorKind) -> &'static str {
        match kind {
            ProviderErrorKind::NativeToolUnsupported => "native_tool_unsupported",
            ProviderErrorKind::ModelNotFound => "model_not_found",
            ProviderErrorKind::Decode => "decode",
            ProviderErrorKind::RetryableNetwork => "retryable_network",
            ProviderErrorKind::Auth => "auth",
            ProviderErrorKind::Quota => "quota",
            ProviderErrorKind::Unknown => "unknown",
        }
    }

    fn fallback_model_for_provider(provider: &str) -> String {
        resolve_model_alias(default_model(provider))
    }

    fn record_provider_error(&mut self, error: &str) {
        self.last_provider_error_kind = Some(Self::classify_provider_error(error));
        self.last_provider_error_message = Some(error.to_string());
    }

    fn record_stop_reason(&mut self, stop_reason: &str) {
        self.last_stop_reason_raw = Some(stop_reason.to_string());
        self.last_stop_reason_alias = Some(Self::stop_reason_alias(stop_reason).to_string());
        let hooks = Self::load_hook_config();
        let _ = run_event_hooks(
            &hooks,
            "Stop",
            "runtime",
            stop_reason,
            &self.permission_mode,
        );
    }

    pub fn emit_hook_event_with_mode(
        permission_mode: &str,
        event: &str,
        tool: &str,
        args: &str,
    ) -> Vec<(bool, String)> {
        let hooks = Self::load_hook_config();
        run_event_hooks(&hooks, event, tool, args, permission_mode)
            .into_iter()
            .map(|x| (x.allow, x.reason))
            .collect()
    }

    fn complete_streaming_with_resilience<F>(
        &mut self,
        on_delta: &mut F,
    ) -> Result<(StreamingResult, Option<String>), String>
    where
        F: FnMut(&str),
    {
        let retry_limit = Self::provider_retry_limit();
        let backoff = Self::provider_retry_backoff();
        let mut retry_attempts = 0usize;
        let mut fallback_note: Option<String> = None;

        loop {
            match self.client.complete_streaming_dyn(&self.messages, on_delta) {
                Ok(streaming) => return Ok((streaming, fallback_note)),
                Err(e) => {
                    if retry_attempts < retry_limit && Self::is_retryable_provider_error(&e) {
                        if Self::is_decode_provider_error(&e) {
                            self.provider_decode_retry_count += 1;
                        }
                        retry_attempts += 1;
                        if !backoff.is_zero() {
                            thread::sleep(backoff);
                        }
                        continue;
                    }

                    if Self::model_auto_fallback_enabled() && Self::is_model_not_found_error(&e) {
                        let fallback_model = Self::fallback_model_for_provider(&self.provider);
                        if !fallback_model.is_empty() && fallback_model != self.model {
                            let previous = self.model.clone();
                            self.model = fallback_model.clone();
                            self.client = Box::new(ProviderClient::new(
                                self.provider.clone(),
                                self.model.clone(),
                            ));
                            fallback_note = Some(format!(
                                "model auto-fallback applied after provider error: {} -> {}",
                                previous, fallback_model
                            ));
                            retry_attempts = 0;
                            continue;
                        }
                    }

                    let msg = if let Some(note) = &fallback_note {
                        format!("{} | {}", e, note)
                    } else {
                        e
                    };
                    self.record_provider_error(&msg);
                    if Self::is_decode_provider_error(&msg) {
                        self.provider_decode_final_fail_count += 1;
                    }
                    return Err(msg);
                }
            }
        }
    }

    fn complete_with_tools_with_resilience<F>(
        &mut self,
        on_delta: &mut F,
    ) -> Result<(CompletionResult, Option<String>), String>
    where
        F: FnMut(&str),
    {
        let retry_limit = Self::provider_retry_limit();
        let backoff = Self::provider_retry_backoff();
        let mut retry_attempts = 0usize;
        let mut fallback_note: Option<String> = None;

        loop {
            match self
                .client
                .complete_with_tools_dyn(&self.messages, &[], on_delta)
            {
                Ok(completion) => return Ok((completion, fallback_note)),
                Err(e) => {
                    if retry_attempts < retry_limit && Self::is_retryable_provider_error(&e) {
                        if Self::is_decode_provider_error(&e) {
                            self.provider_decode_retry_count += 1;
                        }
                        retry_attempts += 1;
                        if !backoff.is_zero() {
                            thread::sleep(backoff);
                        }
                        continue;
                    }

                    if Self::model_auto_fallback_enabled() && Self::is_model_not_found_error(&e) {
                        let fallback_model = Self::fallback_model_for_provider(&self.provider);
                        if !fallback_model.is_empty() && fallback_model != self.model {
                            let previous = self.model.clone();
                            self.model = fallback_model.clone();
                            self.client = Box::new(ProviderClient::new(
                                self.provider.clone(),
                                self.model.clone(),
                            ));
                            fallback_note = Some(format!(
                                "model auto-fallback applied after provider error: {} -> {}",
                                previous, fallback_model
                            ));
                            retry_attempts = 0;
                            continue;
                        }
                    }

                    let msg = if let Some(note) = &fallback_note {
                        format!("{} | {}", e, note)
                    } else {
                        e
                    };
                    self.record_provider_error(&msg);
                    if Self::is_decode_provider_error(&msg) {
                        self.provider_decode_final_fail_count += 1;
                    }
                    return Err(msg);
                }
            }
        }
    }

    fn length_continuation_max_rounds() -> usize {
        std::env::var("ASI_LENGTH_CONTINUATION_MAX_ROUNDS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(4)
            .clamp(0, 12)
    }

    /// Native-path counterpart of `complete_streaming_with_length_continuation`:
    /// when a text-only answer was cut off by the output limit, ask the model
    /// to continue and stitch the parts together. The scaffolding messages
    /// used to request continuation are removed afterwards so history keeps
    /// a single assistant turn with the full text.
    fn continue_native_after_length<F>(
        &mut self,
        first: CompletionResult,
        on_delta: &mut F,
    ) -> CompletionResult
    where
        F: FnMut(&str),
    {
        let needs_continuation = |c: &CompletionResult| {
            c.tool_calls.is_empty()
                && !c.text.trim().is_empty()
                && Self::stop_reason_alias(&c.stop_reason) == "length"
        };
        if !needs_continuation(&first) {
            return first;
        }
        let history_len = self.messages.len();
        let max_rounds = Self::length_continuation_max_rounds();
        let mut combined = first;
        let mut rounds = 0usize;
        while needs_continuation(&combined) && rounds < max_rounds {
            self.messages
                .push(ChatMessage::text("assistant", combined.text.clone()));
            self.messages.push(ChatMessage::text(
                "user".to_string(),
                LENGTH_CONTINUATION_PROMPT.to_string(),
            ));
            let next = match self.complete_with_tools_with_resilience(on_delta) {
                Ok((next, _)) => next,
                Err(_) => break,
            };
            combined.text.push_str(&next.text);
            combined.stop_reason = next.stop_reason;
            combined.tool_calls = next.tool_calls;
            rounds += 1;
        }
        self.messages.truncate(history_len);
        combined
    }

    fn complete_streaming_with_length_continuation<F>(
        &mut self,
        on_delta: &mut F,
    ) -> Result<(StreamingResult, Option<String>), String>
    where
        F: FnMut(&str),
    {
        let (first, note) = self.complete_streaming_with_resilience(on_delta)?;
        if first.stop_reason != "length" {
            return Ok((first, note));
        }

        let mut combined = first.text;
        let mut stop_reason = first.stop_reason;
        let mut rounds = 0usize;
        let max_rounds = Self::length_continuation_max_rounds();

        while stop_reason == "length" && rounds < max_rounds {
            self.messages
                .push(ChatMessage::text("assistant".to_string(), combined.clone()));
            self.messages.push(ChatMessage::text(
                "user".to_string(),
                LENGTH_CONTINUATION_PROMPT.to_string(),
            ));
            let (next, _) = self.complete_streaming_with_resilience(on_delta)?;
            if !next.text.is_empty() {
                combined.push_str(&next.text);
            }
            stop_reason = next.stop_reason;
            rounds += 1;
        }

        Ok((
            StreamingResult {
                text: combined,
                stop_reason,
            },
            note,
        ))
    }

    /// Native tool calling flow: send message with tool definitions, execute any
    /// tool calls returned by the API, send results back.
    fn run_native_tool_turn<F>(
        &mut self,
        input_tokens: usize,
        thinking: Option<String>,
        on_delta: &mut F,
    ) -> TurnResult
    where
        F: FnMut(&str),
    {
        let result = self.complete_with_tools_with_resilience(on_delta);
        let result = match result {
            Ok((completion, note)) => Ok((self.continue_native_after_length(completion, on_delta), note)),
            Err(e) => Err(e),
        };

        match result {
            Ok((completion, fallback_note)) => {
                let completion_text = completion.text.clone();
                let output_tokens = estimate_tokens(&completion_text);
                self.cumulative_input_tokens += input_tokens;
                self.cumulative_output_tokens += output_tokens;
                let turn_cost = cost::estimate_cost_usd(&self.model, input_tokens, output_tokens);
                self.cumulative_cost_usd += turn_cost;

                if completion_text.trim().is_empty()
                    && completion.tool_calls.is_empty()
                    && Self::stop_reason_alias(&completion.stop_reason) == "length"
                {
                    // The whole output budget went to hidden reasoning. Say so
                    // instead of printing nothing, and record an assistant turn
                    // so history never holds back-to-back user messages (which
                    // makes the next prompt fail the same way).
                    let text = EMPTY_LENGTH_HINT.to_string();
                    self.messages.push(ChatMessage::text("assistant", text.clone()));
                    self.record_stop_reason(&completion.stop_reason);
                    return TurnResult {
                        text,
                        stop_reason: completion.stop_reason,
                        input_tokens,
                        output_tokens,
                        total_input_tokens: self.cumulative_input_tokens,
                        total_output_tokens: self.cumulative_output_tokens,
                        is_tool_result: false,
                        turn_cost_usd: turn_cost,
                        total_cost_usd: self.cumulative_cost_usd,
                        thinking,
                        native_tool_calls: Vec::new(),
                    };
                }

                if !completion_text.is_empty() && completion.tool_calls.is_empty() {
                    self.messages
                        .push(ChatMessage::text("assistant", completion_text.clone()));
                }

                if completion.tool_calls.is_empty() {
                    let mut text = completion_text;
                    if let Some(note) = fallback_note {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(&format!("[{}]", note));
                    }
                    self.record_stop_reason(&completion.stop_reason);
                    return TurnResult {
                        text,
                        stop_reason: completion.stop_reason,
                        input_tokens,
                        output_tokens,
                        total_input_tokens: self.cumulative_input_tokens,
                        total_output_tokens: self.cumulative_output_tokens,
                        is_tool_result: false,
                        turn_cost_usd: turn_cost,
                        total_cost_usd: self.cumulative_cost_usd,
                        thinking,
                        native_tool_calls: Vec::new(),
                    };
                }

                let mut native_results = Vec::new();
                let mut pending_exec: Vec<(usize, String, String)> = Vec::new();
                for tc in &completion.tool_calls {
                    let legacy_args = tool_call_to_legacy_args(&tc.name, &tc.arguments);
                    let normalized_args = normalize_tool_args(&tc.name, &legacy_args);
                    log_normalized_toolcall(&tc.name, &normalized_args);
                    let args_display = format!(
                        "{} {}",
                        tc.name,
                        &normalized_args.chars().take(200).collect::<String>()
                    );

                    let allow_rules = self.effective_allow_rules();
                    let deny_rules = self.effective_deny_rules();
                    let additional_dirs = self.effective_additional_directories();
                    let hooks = Self::load_hook_config();
                    let mut pre_hook_denied_reason: Option<String> = None;
                    for outcome in run_event_hooks(
                        &hooks,
                        "PreToolUse",
                        &tc.name,
                        &normalized_args,
                        &self.permission_mode,
                    ) {
                        if !outcome.allow {
                            pre_hook_denied_reason =
                                Some(format!("hook PreToolUse denied: {}", outcome.reason));
                            break;
                        }
                    }
                    if let Some(reason) = pre_hook_denied_reason {
                        let _ = audit::log_permission_decision(
                            &self.provider,
                            &self.model,
                            &self.permission_mode,
                            &tc.name,
                            &normalized_args,
                            false,
                            &reason,
                        );
                        native_results.push(NativeToolResult {
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            tool_args_display: args_display.clone(),
                            result_text: format!("Permission denied: {}", reason),
                            ok: false,
                        });
                        continue;
                    }

                    if let Some(reason) = deny_reason(
                        &self.permission_mode,
                        &tc.name,
                        &normalized_args,
                        self.safe_shell_mode,
                        self.disable_web_tools,
                        self.disable_bash_tool,
                        &allow_rules,
                        &deny_rules,
                        self.path_restriction_enabled,
                        &self.workspace_root,
                        &additional_dirs,
                        &self.sandbox_strategy,
                    ) {
                        let _ = audit::log_permission_decision(
                            &self.provider,
                            &self.model,
                            &self.permission_mode,
                            &tc.name,
                            &normalized_args,
                            false,
                            &reason,
                        );
                        native_results.push(NativeToolResult {
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            tool_args_display: args_display,
                            result_text: format!("Permission denied: {}", reason),
                            ok: false,
                        });
                        continue;
                    }

                    let approval_reason = match request_tool_approval(
                        &self.permission_mode,
                        &tc.name,
                        &normalized_args,
                        &mut self.interactive_approval_allow_rules,
                        &mut self.interactive_approval_deny_rules,
                    ) {
                        Ok(ToolApprovalDecision::Allow) => None,
                        Ok(ToolApprovalDecision::Deny(reason)) => Some(reason),
                        Err(e) => Some(format!("[approval] {}", e)),
                    };
                    if let Some(reason) = approval_reason {
                        let _ = audit::log_permission_decision(
                            &self.provider,
                            &self.model,
                            &self.permission_mode,
                            &tc.name,
                            &normalized_args,
                            false,
                            &reason,
                        );
                        native_results.push(NativeToolResult {
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            tool_args_display: args_display,
                            result_text: format!("Permission denied: {}", reason),
                            ok: false,
                        });
                        continue;
                    }
                    let mut permission_hook_denied_reason: Option<String> = None;
                    for outcome in run_event_hooks(
                        &hooks,
                        "PermissionRequest",
                        &tc.name,
                        &normalized_args,
                        &self.permission_mode,
                    ) {
                        if !outcome.allow {
                            permission_hook_denied_reason =
                                Some(format!("hook PermissionRequest denied: {}", outcome.reason));
                            break;
                        }
                    }
                    if let Some(reason) = permission_hook_denied_reason {
                        let _ = audit::log_permission_decision(
                            &self.provider,
                            &self.model,
                            &self.permission_mode,
                            &tc.name,
                            &normalized_args,
                            false,
                            &reason,
                        );
                        native_results.push(NativeToolResult {
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            tool_args_display: args_display.clone(),
                            result_text: format!("Permission denied: {}", reason),
                            ok: false,
                        });
                        continue;
                    }

                    let (auto_blocked, auto_severity, auto_reason) = auto_review_decision(
                        &self.auto_review_mode,
                        &self.auto_review_severity_threshold,
                        &tc.name,
                        &normalized_args,
                    );
                    let _ = audit::log_auto_review_decision(
                        &self.provider,
                        &self.model,
                        &self.permission_mode,
                        &self.auto_review_mode,
                        &self.auto_review_severity_threshold,
                        &tc.name,
                        &normalized_args,
                        auto_severity.as_str(),
                        auto_blocked,
                        &auto_reason,
                    );
                    maybe_emit_auto_review_warning(
                        &self.auto_review_mode,
                        &self.auto_review_severity_threshold,
                        &tc.name,
                        auto_severity,
                        &auto_reason,
                    );
                    if auto_blocked {
                        let reason = format!(
                            "[auto-review] blocked severity={} threshold={} reason={}",
                            auto_severity.as_str(),
                            parse_auto_review_threshold(&self.auto_review_severity_threshold)
                                .as_str(),
                            auto_reason
                        );
                        let _ = audit::log_permission_decision(
                            &self.provider,
                            &self.model,
                            &self.permission_mode,
                            &tc.name,
                            &normalized_args,
                            false,
                            &reason,
                        );
                        native_results.push(NativeToolResult {
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            tool_args_display: args_display.clone(),
                            result_text: format!("Permission denied: {}", reason),
                            ok: false,
                        });
                        continue;
                    }

                    let _ = audit::log_permission_decision(
                        &self.provider,
                        &self.model,
                        &self.permission_mode,
                        &tc.name,
                        &normalized_args,
                        true,
                        "allowed",
                    );

                    // Defer execution: all permission checks run first (in
                    // order, since approval can be interactive), then the
                    // approved calls execute together below.
                    pending_exec.push((native_results.len(), tc.name.clone(), normalized_args));
                    native_results.push(NativeToolResult {
                        tool_call_id: tc.id.clone(),
                        tool_name: tc.name.clone(),
                        tool_args_display: args_display,
                        result_text: String::new(),
                        ok: false,
                    });
                }

                let tool_outputs = run_native_tools_maybe_parallel(&self.tool_runner, &pending_exec);
                let post_hooks = Self::load_hook_config();
                for ((slot, name, args), tool_result) in pending_exec.iter().zip(tool_outputs) {
                    let _ = run_event_hooks(&post_hooks, "PostToolUse", name, args, &self.permission_mode);
                    native_results[*slot].result_text = tool_result.output;
                    native_results[*slot].ok = tool_result.ok;
                }

                self.consume_next_permissions();

                // Record history in the native tool-calling protocol shape:
                // one assistant message carrying the original tool_calls
                // array, then one role="tool" message per result, paired by
                // tool_call_id. Models are trained on exactly this exchange;
                // the previous flattening (fake assistant text + results as
                // a user message) pushed them off-distribution and caused
                // erratic early stops and invented pseudo-XML call formats.
                self.messages.push(ChatMessage::assistant_with_tool_calls_and_reasoning(
                    completion_text.clone(),
                    completion
                        .tool_calls
                        .iter()
                        .map(ToolCallSpec::from_call)
                        .collect(),
                    completion.reasoning_content.clone(),
                ));
                for nr in &native_results {
                    self.messages.push(ChatMessage::tool_result(
                        nr.tool_call_id.clone(),
                        compact_tool_result_for_history(&nr.tool_name, &nr.result_text),
                    ));
                }

                let mut display_text = completion_text;
                for nr in &native_results {
                    display_text.push_str(&format!(
                        "\n[tool:{}:{}]\n{}",
                        nr.tool_name,
                        if nr.ok { "ok" } else { "error" },
                        nr.result_text
                    ));
                }
                if let Some(note) = fallback_note {
                    if !display_text.is_empty() {
                        display_text.push('\n');
                    }
                    display_text.push_str(&format!("[{}]", note));
                }

                let stop_reason = if native_results.is_empty() {
                    "completed".to_string()
                } else {
                    "tool_use".to_string()
                };
                self.record_stop_reason(&stop_reason);
                TurnResult {
                    text: display_text,
                    stop_reason,
                    input_tokens,
                    output_tokens,
                    total_input_tokens: self.cumulative_input_tokens,
                    total_output_tokens: self.cumulative_output_tokens,
                    is_tool_result: !native_results.is_empty(),
                    turn_cost_usd: turn_cost,
                    total_cost_usd: self.cumulative_cost_usd,
                    thinking,
                    native_tool_calls: native_results,
                }
            }
            Err(e) => {
                if Self::is_native_tool_calling_unsupported_error(&e) {
                    self.native_tool_calling = false;
                    let note = format!(
                        "native tool calling unsupported by current endpoint/model, auto-disabled for this session: {}",
                        e
                    );
                    self.native_tool_auto_disabled_reason = Some(note.clone());
                    self.record_provider_error(&e);
                    return self
                        .run_legacy_text_turn(input_tokens, thinking, on_delta)
                        .with_fallback_note(&note);
                }

                self.record_provider_error(&e);
                self.run_legacy_text_turn(input_tokens, thinking, on_delta)
                    .with_fallback_note(&e)
            }
        }
    }

    /// Legacy text-based streaming (no native tool calling).
    fn run_legacy_text_turn<F>(
        &mut self,
        input_tokens: usize,
        thinking: Option<String>,
        on_delta: &mut F,
    ) -> TurnResult
    where
        F: FnMut(&str),
    {
        match self.complete_streaming_with_length_continuation(on_delta) {
            Ok((streaming, fallback_note)) => {
                let text = streaming.text;
                let output_tokens = estimate_tokens(&text);
                self.cumulative_input_tokens += input_tokens;
                self.cumulative_output_tokens += output_tokens;
                let turn_cost_usd =
                    cost::estimate_cost_usd(&self.model, input_tokens, output_tokens);
                self.cumulative_cost_usd += turn_cost_usd;
                self.messages
                    .push(ChatMessage::text("assistant".to_string(), text.clone()));

                let mut display_text = text;
                if let Some(note) = fallback_note {
                    if !display_text.is_empty() {
                        display_text.push('\n');
                    }
                    display_text.push_str(&format!("[{}]", note));
                }
                self.record_stop_reason(&streaming.stop_reason);

                TurnResult {
                    text: display_text,
                    stop_reason: streaming.stop_reason,
                    input_tokens,
                    output_tokens,
                    total_input_tokens: self.cumulative_input_tokens,
                    total_output_tokens: self.cumulative_output_tokens,
                    is_tool_result: false,
                    turn_cost_usd,
                    total_cost_usd: self.cumulative_cost_usd,
                    thinking,
                    native_tool_calls: Vec::new(),
                }
            }
            Err(e) => {
                self.record_provider_error(&e);
                self.record_stop_reason("provider_error");
                TurnResult {
                    text: format!("Provider error: {}", e),
                    stop_reason: "provider_error".to_string(),
                    input_tokens: 0,
                    output_tokens: 0,
                    total_input_tokens: self.cumulative_input_tokens,
                    total_output_tokens: self.cumulative_output_tokens,
                    is_tool_result: false,
                    turn_cost_usd: 0.0,
                    total_cost_usd: self.cumulative_cost_usd,
                    thinking,
                    native_tool_calls: Vec::new(),
                }
            }
        }
    }

    /// Handle manual /toolcall command.
    fn handle_manual_toolcall(&mut self, rest: &str, user_text: &str) -> TurnResult {
        let (name, args_raw) = split_tool(rest);
        if !is_known_tool(&name) {
            self.consume_next_permissions();
            self.record_stop_reason("unknown_tool");
            return TurnResult {
                text: format!("Unknown tool: {}. Use /tools to see available tools.", name),
                stop_reason: "unknown_tool".to_string(),
                input_tokens: 0,
                output_tokens: 0,
                total_input_tokens: self.cumulative_input_tokens,
                total_output_tokens: self.cumulative_output_tokens,
                is_tool_result: true,
                turn_cost_usd: 0.0,
                total_cost_usd: self.cumulative_cost_usd,
                thinking: None,
                native_tool_calls: Vec::new(),
            };
        }

        let args = normalize_tool_args(&name, &args_raw);
        log_normalized_toolcall(&name, &args);

        let allow_rules = self.effective_allow_rules();
        let deny_rules = self.effective_deny_rules();
        let additional_dirs = self.effective_additional_directories();
        let hooks = Self::load_hook_config();
        for outcome in run_event_hooks(&hooks, "PreToolUse", &name, &args, &self.permission_mode) {
            if !outcome.allow {
                let reason = format!("hook PreToolUse denied: {}", outcome.reason);
                let _ = audit::log_permission_decision(
                    &self.provider,
                    &self.model,
                    &self.permission_mode,
                    &name,
                    &args,
                    false,
                    &reason,
                );
                self.consume_next_permissions();
                self.record_stop_reason("permission_denied");
                return TurnResult {
                    text: format!("Permission denied: {}", reason),
                    stop_reason: "permission_denied".to_string(),
                    input_tokens: 0,
                    output_tokens: 0,
                    total_input_tokens: self.cumulative_input_tokens,
                    total_output_tokens: self.cumulative_output_tokens,
                    is_tool_result: true,
                    turn_cost_usd: 0.0,
                    total_cost_usd: self.cumulative_cost_usd,
                    thinking: None,
                    native_tool_calls: Vec::new(),
                };
            }
        }

        if let Some(reason) = deny_reason(
            &self.permission_mode,
            &name,
            &args,
            self.safe_shell_mode,
            self.disable_web_tools,
            self.disable_bash_tool,
            &allow_rules,
            &deny_rules,
            self.path_restriction_enabled,
            &self.workspace_root,
            &additional_dirs,
            &self.sandbox_strategy,
        ) {
            let _ = audit::log_permission_decision(
                &self.provider,
                &self.model,
                &self.permission_mode,
                &name,
                &args,
                false,
                &reason,
            );
            self.consume_next_permissions();
            self.record_stop_reason("permission_denied");
            return TurnResult {
                text: format!("Permission denied: {}", reason),
                stop_reason: "permission_denied".to_string(),
                input_tokens: 0,
                output_tokens: 0,
                total_input_tokens: self.cumulative_input_tokens,
                total_output_tokens: self.cumulative_output_tokens,
                is_tool_result: true,
                turn_cost_usd: 0.0,
                total_cost_usd: self.cumulative_cost_usd,
                thinking: None,
                native_tool_calls: Vec::new(),
            };
        }

        let approval_reason = match request_tool_approval(
            &self.permission_mode,
            &name,
            &args,
            &mut self.interactive_approval_allow_rules,
            &mut self.interactive_approval_deny_rules,
        ) {
            Ok(ToolApprovalDecision::Allow) => None,
            Ok(ToolApprovalDecision::Deny(reason)) => Some(reason),
            Err(e) => Some(format!("[approval] {}", e)),
        };
        if let Some(reason) = approval_reason {
            let _ = audit::log_permission_decision(
                &self.provider,
                &self.model,
                &self.permission_mode,
                &name,
                &args,
                false,
                &reason,
            );
            self.consume_next_permissions();
            self.record_stop_reason("permission_denied");
            return TurnResult {
                text: format!("Permission denied: {}", reason),
                stop_reason: "permission_denied".to_string(),
                input_tokens: 0,
                output_tokens: 0,
                total_input_tokens: self.cumulative_input_tokens,
                total_output_tokens: self.cumulative_output_tokens,
                is_tool_result: true,
                turn_cost_usd: 0.0,
                total_cost_usd: self.cumulative_cost_usd,
                thinking: None,
                native_tool_calls: Vec::new(),
            };
        }
        for outcome in run_event_hooks(
            &hooks,
            "PermissionRequest",
            &name,
            &args,
            &self.permission_mode,
        ) {
            if !outcome.allow {
                let reason = format!("hook PermissionRequest denied: {}", outcome.reason);
                let _ = audit::log_permission_decision(
                    &self.provider,
                    &self.model,
                    &self.permission_mode,
                    &name,
                    &args,
                    false,
                    &reason,
                );
                self.consume_next_permissions();
                self.record_stop_reason("permission_denied");
                return TurnResult {
                    text: format!("Permission denied: {}", reason),
                    stop_reason: "permission_denied".to_string(),
                    input_tokens: 0,
                    output_tokens: 0,
                    total_input_tokens: self.cumulative_input_tokens,
                    total_output_tokens: self.cumulative_output_tokens,
                    is_tool_result: true,
                    turn_cost_usd: 0.0,
                    total_cost_usd: self.cumulative_cost_usd,
                    thinking: None,
                    native_tool_calls: Vec::new(),
                };
            }
        }

        let (auto_blocked, auto_severity, auto_reason) = auto_review_decision(
            &self.auto_review_mode,
            &self.auto_review_severity_threshold,
            &name,
            &args,
        );
        let _ = audit::log_auto_review_decision(
            &self.provider,
            &self.model,
            &self.permission_mode,
            &self.auto_review_mode,
            &self.auto_review_severity_threshold,
            &name,
            &args,
            auto_severity.as_str(),
            auto_blocked,
            &auto_reason,
        );
        maybe_emit_auto_review_warning(
            &self.auto_review_mode,
            &self.auto_review_severity_threshold,
            &name,
            auto_severity,
            &auto_reason,
        );
        if auto_blocked {
            let reason = format!(
                "[auto-review] blocked severity={} threshold={} reason={}",
                auto_severity.as_str(),
                parse_auto_review_threshold(&self.auto_review_severity_threshold).as_str(),
                auto_reason
            );
            let _ = audit::log_permission_decision(
                &self.provider,
                &self.model,
                &self.permission_mode,
                &name,
                &args,
                false,
                &reason,
            );
            self.consume_next_permissions();
            self.record_stop_reason("permission_denied");
            return TurnResult {
                text: format!("Permission denied: {}", reason),
                stop_reason: "permission_denied".to_string(),
                input_tokens: 0,
                output_tokens: 0,
                total_input_tokens: self.cumulative_input_tokens,
                total_output_tokens: self.cumulative_output_tokens,
                is_tool_result: true,
                turn_cost_usd: 0.0,
                total_cost_usd: self.cumulative_cost_usd,
                thinking: None,
                native_tool_calls: Vec::new(),
            };
        }

        let _ = audit::log_permission_decision(
            &self.provider,
            &self.model,
            &self.permission_mode,
            &name,
            &args,
            true,
            "allowed",
        );
        self.consume_next_permissions();

        let r = self.tool_runner.run(&name, &args);
        let _ = run_event_hooks(&hooks, "PostToolUse", &name, &args, &self.permission_mode);
        let txt = format!(
            "[tool:{}:{}]\n{}",
            name,
            if r.ok { "ok" } else { "error" },
            r.output
        );
        self.messages
            .push(ChatMessage::text("user".to_string(), user_text.to_string()));
        self.messages.push(ChatMessage::text(
            "assistant".to_string(),
            compact_tool_result_for_history(&name, &txt),
        ));
        self.record_stop_reason("tool_result");
        TurnResult {
            text: txt,
            stop_reason: "tool_result".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            total_input_tokens: self.cumulative_input_tokens,
            total_output_tokens: self.cumulative_output_tokens,
            is_tool_result: true,
            turn_cost_usd: 0.0,
            total_cost_usd: self.cumulative_cost_usd,
            thinking: None,
            native_tool_calls: Vec::new(),
        }
    }

    pub fn run_manual_toolcall_batch_concurrent(&mut self, commands: &[String]) -> Vec<TurnResult> {
        if commands.is_empty() {
            return Vec::new();
        }

        let shared = ConcurrentToolExecShared {
            provider: self.provider.clone(),
            model: self.model.clone(),
            permission_mode: self.permission_mode.clone(),
            auto_review_mode: self.auto_review_mode.clone(),
            auto_review_severity_threshold: self.auto_review_severity_threshold.clone(),
            safe_shell_mode: self.safe_shell_mode,
            disable_web_tools: self.disable_web_tools,
            disable_bash_tool: self.disable_bash_tool,
            allow_rules: self.effective_allow_rules(),
            deny_rules: self.effective_deny_rules(),
            path_restriction_enabled: self.path_restriction_enabled,
            workspace_root: self.workspace_root.clone(),
            additional_directories: self.effective_additional_directories(),
            sandbox_strategy: self.sandbox_strategy.clone(),
            tool_runner: self.tool_runner.clone(),
            hooks: Self::load_hook_config(),
        };

        let mut handles = Vec::with_capacity(commands.len());
        for (index, command) in commands.iter().enumerate() {
            let shared_ctx = shared.clone();
            let command_text = command.clone();
            handles.push((
                index,
                command_text.clone(),
                thread::spawn(move || {
                    execute_toolcall_command_concurrently(index, command_text, shared_ctx)
                }),
            ));
        }

        let mut outcomes = Vec::with_capacity(commands.len());
        for (index, command, handle) in handles {
            match handle.join() {
                Ok(outcome) => outcomes.push(outcome),
                Err(_) => outcomes.push(ConcurrentToolOutcome {
                    index,
                    command,
                    tool_name: "tool".to_string(),
                    tool_args: String::new(),
                    text: "Tool worker panicked".to_string(),
                    stop_reason: "tool_error".to_string(),
                    is_tool_result: true,
                    audit_allowed: None,
                    audit_reason: String::new(),
                }),
            }
        }

        outcomes.sort_by_key(|x| x.index);

        let mut results = Vec::with_capacity(outcomes.len());
        for outcome in outcomes {
            if let Some(allowed) = outcome.audit_allowed {
                let _ = audit::log_permission_decision(
                    &self.provider,
                    &self.model,
                    &self.permission_mode,
                    &outcome.tool_name,
                    &outcome.tool_args,
                    allowed,
                    &outcome.audit_reason,
                );
            }

            if outcome.stop_reason == "tool_result" {
                self.messages.push(ChatMessage::text(
                    "user".to_string(),
                    outcome.command.clone(),
                ));
                self.messages.push(ChatMessage::text(
                    "assistant".to_string(),
                    compact_tool_result_for_history(&outcome.tool_name, &outcome.text),
                ));
            }

            results.push(TurnResult {
                text: outcome.text,
                stop_reason: outcome.stop_reason,
                input_tokens: 0,
                output_tokens: 0,
                total_input_tokens: self.cumulative_input_tokens,
                total_output_tokens: self.cumulative_output_tokens,
                is_tool_result: outcome.is_tool_result,
                turn_cost_usd: 0.0,
                total_cost_usd: self.cumulative_cost_usd,
                thinking: None,
                native_tool_calls: Vec::new(),
            });
        }

        if let Some(last) = results.last() {
            self.record_stop_reason(&last.stop_reason);
        }

        self.consume_next_permissions();
        results
    }
    fn effective_allow_rules(&self) -> Vec<String> {
        let base = merge_unique_strings(
            &self.permission_allow_rules,
            &self.session_permission_allow_rules,
        );
        merge_unique_strings(&base, &self.next_permission_allow_rules)
    }

    fn effective_deny_rules(&self) -> Vec<String> {
        merge_unique_strings(
            &self.permission_deny_rules,
            &self.session_permission_deny_rules,
        )
    }

    fn effective_additional_directories(&self) -> Vec<PathBuf> {
        let base = merge_unique_paths(
            &self.additional_directories,
            &self.session_additional_directories,
        );
        merge_unique_paths(&base, &self.next_additional_directories)
    }

    fn consume_next_permissions(&mut self) {
        self.next_permission_allow_rules.clear();
        self.next_additional_directories.clear();
    }

    /// Lossy compaction: keep system + last `keep_last` user/assistant
    /// messages, drop everything else without summarizing. Use when you
    /// know the dropped history isn't needed — e.g. you just finished a
    /// self-contained subtask. For long-running tasks where context
    /// matters, prefer `compact_with_summary`.
    pub fn compact(&mut self, keep_last: usize) -> String {
        let hooks = Self::load_hook_config();
        for outcome in run_event_hooks(
            &hooks,
            "PreCompact",
            "runtime",
            "runtime_compact",
            &self.permission_mode,
        ) {
            if !outcome.allow {
                return format!("Compact denied: hook PreCompact denied: {}", outcome.reason);
            }
        }
        let head = self
            .messages
            .first()
            .cloned()
            .into_iter()
            .collect::<Vec<_>>();
        let mut tail = self
            .messages
            .iter()
            .skip(1)
            .filter(|m| m.role == "user" || m.role == "assistant")
            .cloned()
            .collect::<Vec<_>>();
        if tail.len() > keep_last {
            tail = tail[tail.len() - keep_last..].to_vec();
        }
        let removed = self.messages.len().saturating_sub(head.len() + tail.len());
        self.messages = [head, tail].concat();
        let _ = run_event_hooks(
            &hooks,
            "PostCompact",
            "runtime",
            "runtime_compact",
            &self.permission_mode,
        );
        format!("Compacted conversation; removed {} messages", removed)
    }

    /// Smart compaction for long-running tasks (the Claude-Code-style
    /// "30 hours overnight" use case).
    ///
    /// Instead of just dropping early turns, this asks the current provider
    /// to write a **structured summary** of everything between the system
    /// prompt and the last `keep_last` turns. The summary replaces the
    /// dropped history as a synthetic assistant message, so:
    ///   - original task goal,
    ///   - files touched + how,
    ///   - decisions / failed approaches,
    ///   - outstanding TODOs,
    /// all survive into the post-compaction conversation. The recent
    /// `keep_last` user/assistant messages stay verbatim so the model
    /// retains short-term context.
    ///
    /// Returns a human-readable result string. On provider error we fall
    /// back to lossy `compact` so the operation never leaves the runtime
    /// in a half-state.
    pub fn compact_with_summary(&mut self, keep_last: usize) -> String {
        self.compact_with_summary_for_task(keep_last, None)
    }

    /// Produce a final answer after the agent loop stopped on a budget
    /// (steps, time, no progress) while the last turn was still a tool
    /// result. Uses the text-only completion path, which sends no tool
    /// definitions, so the model cannot start another round of tool calls.
    /// Returns None if the provider fails or returns nothing.
    pub fn wrap_up_without_tools(&mut self, task: &str, stop_reason: &str) -> Option<TurnResult> {
        let body: Vec<ChatMessage> = self.messages.iter().skip(1).cloned().collect();
        let evidence = build_wrap_up_evidence(&body, 60_000);
        let request = vec![
            ChatMessage::text("system".to_string(), WRAP_UP_SYSTEM_PROMPT.to_string()),
            ChatMessage::text(
                "user".to_string(),
                format!(
                    "TASK:\n{}\n\nThe agent stopped before answering ({}). Evidence gathered so far:\n\n{}\n\nWrite the final answer to the TASK now.",
                    clip_with_marker(task, 4000),
                    stop_reason,
                    evidence
                ),
            ),
        ];
        let mut streamed = String::new();
        let result = self
            .client
            .complete_streaming_dyn(&request, &mut |delta| streamed.push_str(delta));
        let text = match result {
            Ok(r) if !streamed.trim().is_empty() => streamed,
            Ok(r) if !r.text.trim().is_empty() => r.text,
            _ => return None,
        };
        let input_tokens = request.iter().map(|m| estimate_tokens(&m.content)).sum::<usize>();
        let output_tokens = estimate_tokens(&text);
        self.cumulative_input_tokens += input_tokens;
        self.cumulative_output_tokens += output_tokens;
        let turn_cost = cost::estimate_cost_usd(&self.model, input_tokens, output_tokens);
        self.cumulative_cost_usd += turn_cost;
        // An assistant text turn after tool results is a valid continuation
        // of the native protocol, so the history stays replayable.
        self.messages.push(ChatMessage::text("assistant", text.clone()));
        self.record_stop_reason("completed");
        Some(TurnResult {
            text,
            stop_reason: "completed".to_string(),
            input_tokens,
            output_tokens,
            total_input_tokens: self.cumulative_input_tokens,
            total_output_tokens: self.cumulative_output_tokens,
            is_tool_result: false,
            turn_cost_usd: turn_cost,
            total_cost_usd: self.cumulative_cost_usd,
            thinking: None,
            native_tool_calls: Vec::new(),
        })
    }

    /// Structured compaction: summarize older turns (decisions, work done,
    /// failed approaches) while pinning the task text verbatim and keeping
    /// the recent tail intact.
    pub fn compact_with_summary_for_task(&mut self, keep_last: usize, task: Option<&str>) -> String {
        let hooks = Self::load_hook_config();
        for outcome in run_event_hooks(
            &hooks,
            "PreCompact",
            "runtime",
            "runtime_compact",
            &self.permission_mode,
        ) {
            if !outcome.allow {
                return format!("Compact denied: hook PreCompact denied: {}", outcome.reason);
            }
        }

        // Separate history into [system, ...drop_region, ...keep_tail]. The
        // tail is counted in user/assistant messages, then widened so it never
        // starts with role="tool" results whose assistant tool_calls message
        // would otherwise be dropped (an invalid native-protocol history).
        let system_msg = self.messages.first().cloned();
        let body: Vec<ChatMessage> = self.messages.iter().skip(1).cloned().collect();
        let split = compaction_split_index(&body, keep_last);
        let keep_tail: Vec<ChatMessage> = body[split..].to_vec();
        let drop_region: Vec<ChatMessage> = body[..split].to_vec();
        let kept_count = keep_tail.len();

        // Nothing to compact (an earlier summary alone does not count).
        if drop_region.iter().all(is_compacted_summary_message) {
            return "Compact skipped: nothing to summarize.".to_string();
        }

        // The task is pinned verbatim so repeated compactions never
        // paraphrase it away: explicit task > task pinned by an earlier
        // compaction > first user message.
        let pinned_task = task
            .map(|t| t.to_string())
            .or_else(|| drop_region.iter().find_map(pinned_task_from_summary))
            .or_else(|| {
                drop_region
                    .iter()
                    .find(|m| m.role == "user")
                    .map(|m| m.content.clone())
            })
            .map(|t| clip_with_marker(&t, 4000));

        let transcript = build_compaction_transcript(&drop_region);

        let summary_instructions = "You are summarizing an in-progress long-running coding session so it can be \
continued without losing critical state. Output ONLY the summary, no preamble. Use these sections, in this order, each at most a few sentences:\n\
\n\
1. ORIGINAL TASK: the user's overall goal (one sentence).\n\
2. WORK DONE: bullet list of concrete actions completed (which files were read/written/edited, which commands ran, which tests passed). Cite paths verbatim.\n\
3. KEY FACTS: anything the next turn needs to know — important file contents, decisions made, library versions, environment specifics.\n\
4. FAILED APPROACHES: things tried that did NOT work, so we don't repeat them.\n\
5. OUTSTANDING WORK: bullet list of TODOs that remain.\n\
6. RECENT CONTEXT: one-line note on what the very last turn was about, since the most recent messages are kept verbatim after this summary.\n\
\n\
Keep the whole summary under 800 tokens. Be specific, not generic.";

        let summary_msgs = vec![
            ChatMessage::text("system".to_string(), summary_instructions.to_string()),
            ChatMessage::text("user".to_string(), format!(
                    "Summarize the following session transcript (the head is the oldest, the tail is the most recent).\n\n{}",
                    transcript
                )),
        ];

        let mut summary_text = String::new();
        let summary_call = self
            .client
            .complete_streaming_dyn(&summary_msgs, &mut |delta| {
                summary_text.push_str(delta);
            });

        let removed = drop_region.len();
        let task_block = pinned_task
            .as_deref()
            .map(|t| format!("{}\n{}\n{}\n", PINNED_TASK_OPEN, t, PINNED_TASK_CLOSE))
            .unwrap_or_default();
        // Some gateways ignore streaming and only return the final text.
        if let Ok(res) = &summary_call {
            if summary_text.trim().is_empty() {
                summary_text = res.text.clone();
            }
        }
        let result_msg = match summary_call {
            Ok(_) if !summary_text.trim().is_empty() => {
                // Inject the summary as a synthetic assistant message so the
                // model treats it as authoritative history. Wrap with a clear
                // boundary header so downstream prompts and reviewers can see
                // exactly where the compaction kicked in.
                let synthesized = ChatMessage::text("assistant".to_string(), format!(
                        "{}\n{}{}\n[END OF COMPACTED SUMMARY]",
                        COMPACTED_SUMMARY_HEADER,
                        task_block,
                        summary_text.trim()
                    ));
                let mut new_msgs: Vec<ChatMessage> = Vec::with_capacity(2 + keep_tail.len());
                if let Some(sys) = system_msg {
                    new_msgs.push(sys);
                }
                new_msgs.push(synthesized);
                new_msgs.extend(keep_tail);
                self.messages = new_msgs;
                format!(
                    "Compacted conversation with structured summary; removed {} messages, kept {} recent",
                    removed, kept_count
                )
            }
            _ => {
                // Provider error or empty summary: don't leave a half-broken
                // state. Fall back to plain lossy compact so the user at
                // least gets the context cap they asked for, but keep the
                // pinned task so the agent does not lose its goal.
                let mut new_msgs: Vec<ChatMessage> = system_msg.into_iter().collect();
                if !task_block.is_empty() {
                    new_msgs.push(ChatMessage::text(
                        "assistant".to_string(),
                        format!(
                            "{}\n{}(summary unavailable; older turns were dropped)\n[END OF COMPACTED SUMMARY]",
                            COMPACTED_SUMMARY_HEADER, task_block
                        ),
                    ));
                }
                new_msgs.extend(keep_tail);
                self.messages = new_msgs;
                format!(
                    "Smart compact failed (provider returned empty); fell back to lossy compact. Removed {} messages.",
                    removed
                )
            }
        };

        let _ = run_event_hooks(
            &hooks,
            "PostCompact",
            "runtime",
            "runtime_compact",
            &self.permission_mode,
        );
        result_msg
    }

    /// Threshold check: returns Some(reason) when auto-compaction should
    /// trigger because cumulative tokens are nearing the configured cap.
    /// Used by long-running agent loops to keep context fresh without
    /// requiring the user to manually `/compact`.
    pub fn auto_compact_recommendation(&self, threshold_input_tokens: usize) -> Option<String> {
        if threshold_input_tokens == 0 {
            return None;
        }
        // Measure the live context that will be sent next turn. The
        // cumulative counter never shrinks after compaction, so using it
        // would re-trigger a compaction on every later iteration.
        let context_tokens = self.context_tokens_estimate();
        if context_tokens >= threshold_input_tokens {
            Some(format!(
                "context tokens {} >= threshold {}",
                context_tokens, threshold_input_tokens
            ))
        } else {
            None
        }
    }

    /// Estimated token size of the current message history, including
    /// native tool-call arguments.
    pub fn context_tokens_estimate(&self) -> usize {
        self.messages
            .iter()
            .map(|m| {
                estimate_tokens(&m.content)
                    + m.tool_calls
                        .iter()
                        .map(|tc| estimate_tokens(&tc.function.arguments))
                        .sum::<usize>()
            })
            .sum()
    }

    /// Turn-count threshold check: returns Some(reason) when the live
    /// `turn_count()` has reached `ratio * max_turns`. Use this to
    /// auto-compact BEFORE the runtime hits max_turns_reached so the
    /// next user input doesn't fail immediately. `ratio` is clamped
    /// to (0.0, 1.0]; values outside that range disable the check.
    pub fn auto_compact_recommendation_by_turns(&self, ratio: f64) -> Option<String> {
        if self.max_turns == 0 || !(ratio > 0.0 && ratio <= 1.0) {
            return None;
        }
        let threshold = ((self.max_turns as f64) * ratio).ceil() as usize;
        let threshold = threshold.max(1);
        let turns = self.turn_count();
        if turns >= threshold {
            Some(format!(
                "turns {} >= {} ({}% of max_turns {})",
                turns,
                threshold,
                (ratio * 100.0).round() as usize,
                self.max_turns
            ))
        } else {
            None
        }
    }

    /// Append a synthetic user-role message to history. Used by the
    /// auto-tool-loop to inject auto-test failure feedback so the next
    /// model turn sees it in context. Does not touch token counters —
    /// those are updated on the next `run_turn_streaming` call.
    pub fn push_user_message(&mut self, content: &str) {
        self.messages
            .push(ChatMessage::text("user".to_string(), content.to_string()));
    }

    pub fn clear(&mut self) {
        self.messages = vec![ChatMessage::text(
            "system".to_string(),
            DEFAULT_SYSTEM_PROMPT.to_string(),
        )];
        self.cumulative_input_tokens = 0;
        self.cumulative_output_tokens = 0;
        self.cumulative_cost_usd = 0.0;
        self.native_tool_auto_disabled_reason = None;
        self.last_provider_error_kind = None;
        self.last_provider_error_message = None;
        self.last_stop_reason_raw = None;
        self.last_stop_reason_alias = None;
    }

    pub fn turn_count(&self) -> usize {
        self.messages.iter().filter(|m| m.role == "user").count()
    }

    pub fn load_session_messages(
        &mut self,
        provider: String,
        model: String,
        loaded: Vec<serde_json::Value>,
    ) {
        self.provider = provider.clone();
        self.model = model.clone();
        self.client = Box::new(ProviderClient::new(provider, model));
        // A resumed session may have been created with a different model or
        // provider. Recompute the native-tool capability instead of carrying
        // the previous session's boolean into the new protocol.
        self.native_tool_calling = Self::default_native_tool_calling(&self.provider, &self.model);
        self.native_tool_auto_disabled_reason = None;
        self.last_provider_error_kind = None;
        self.last_provider_error_message = None;
        self.last_stop_reason_raw = None;
        self.last_stop_reason_alias = None;
        self.messages = vec![ChatMessage::text(
            "system".to_string(),
            DEFAULT_SYSTEM_PROMPT.to_string(),
        )];
        for v in loaded {
            // Prefer full-fidelity deserialization so native tool_calls /
            // tool role messages saved by newer builds round-trip intact.
            if let Ok(msg) = serde_json::from_value::<ChatMessage>(v.clone()) {
                let keep = match msg.role.as_str() {
                    "user" | "assistant" => !msg.content.is_empty() || !msg.tool_calls.is_empty(),
                    "tool" => msg.tool_call_id.is_some(),
                    _ => false,
                };
                if keep {
                    self.messages.push(msg);
                }
                continue;
            }
            let role = v
                .get("role")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let content = v
                .get("content")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            if (role == "user" || role == "assistant") && !content.is_empty() {
                self.messages.push(ChatMessage::text(role, content));
            }
        }
        self.cumulative_input_tokens = 0;
        self.cumulative_output_tokens = 0;
        self.cumulative_cost_usd = 0.0;
    }

    pub fn sandbox_name(&self) -> &'static str {
        self.sandbox_strategy.name()
    }

    pub fn as_json_messages(&self) -> Vec<serde_json::Value> {
        self.messages
            .iter()
            .map(|m| {
                serde_json::to_value(m)
                    .unwrap_or_else(|_| json!({"role": m.role, "content": m.content}))
            })
            .collect()
    }

    pub fn provider_decode_retry_count(&self) -> usize {
        self.provider_decode_retry_count
    }

    pub fn provider_decode_final_fail_count(&self) -> usize {
        self.provider_decode_final_fail_count
    }

    pub fn provider_decode_stats_line(&self) -> String {
        format!(
            "provider_decode_errors retries={} final_failures={}",
            self.provider_decode_retry_count(),
            self.provider_decode_final_fail_count()
        )
    }

    pub fn stop_reason_alias(stop_reason: &str) -> &'static str {
        match stop_reason.trim().to_ascii_lowercase().as_str() {
            "tool_use" | "tool_calls" | "function_call" => "tool_use",
            "end_turn" | "stop" | "completed" => "completed",
            "max_tokens" | "length" => "length",
            "content_filter" | "safety" => "content_filter",
            "provider_error" => "provider_error",
            _ => "other",
        }
    }

    pub fn status_provider_runtime_line(&self) -> String {
        let caps = Self::capabilities_for(&self.provider, &self.model);
        let native_reason = self
            .native_tool_auto_disabled_reason
            .as_deref()
            .unwrap_or("none");
        format!(
            "provider_runtime native={} default={} supported={} recommended_max_output_tokens={} auto_disabled_reason={}",
            self.native_tool_calling,
            caps.native_tool_calling_default,
            caps.native_tool_calling_supported,
            caps.recommended_max_output_tokens,
            native_reason
        )
    }

    pub fn status_provider_error_line(&self) -> String {
        let kind = self
            .last_provider_error_kind
            .map(Self::provider_error_kind_label)
            .unwrap_or("none");
        let message = self
            .last_provider_error_message
            .as_deref()
            .map(|m| m.chars().take(240).collect::<String>())
            .unwrap_or_else(|| "none".to_string());
        format!("provider_last_error kind={} message={}", kind, message)
    }

    pub fn status_stop_reason_line(&self) -> String {
        let raw = self.last_stop_reason_raw.as_deref().unwrap_or("none");
        let alias = self.last_stop_reason_alias.as_deref().unwrap_or("none");
        format!("stop_reason_last raw={} alias={}", raw, alias)
    }

    pub fn status_project_context_line(&self) -> String {
        let count = self.project_context_sources.len();
        if count == 0 {
            return "project_context sources=0".to_string();
        }
        let preview = self
            .project_context_sources
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        format!("project_context sources={} [{}]", count, preview)
    }

    pub fn last_stop_reason_raw(&self) -> &str {
        self.last_stop_reason_raw.as_deref().unwrap_or("none")
    }

    pub fn last_stop_reason_alias(&self) -> &str {
        self.last_stop_reason_alias.as_deref().unwrap_or("none")
    }

    pub fn rebind_provider_model(&mut self, provider: String, model: String) {
        self.provider = provider.clone();
        self.model = model.clone();
        self.client = Box::new(ProviderClient::new(provider, model));
        self.native_tool_calling = Self::default_native_tool_calling(&self.provider, &self.model);
        self.native_tool_auto_disabled_reason = None;
        self.last_provider_error_kind = None;
        self.last_provider_error_message = None;
        self.last_stop_reason_raw = None;
        self.last_stop_reason_alias = None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolApprovalDecision {
    Allow,
    Deny(String),
}

fn is_interactive_permission_mode(mode: &str) -> bool {
    matches!(
        mode.trim().to_ascii_lowercase().as_str(),
        "ask" | "on-request" | "on_request" | "interactive"
    )
}

pub fn collect_hook_diagnostics(
    event: &str,
    tool: &str,
    args: &str,
    mode: &str,
) -> Vec<HookDiagnostic> {
    let hooks = Runtime::load_hook_config();
    let (_, diagnostics) = run_event_hooks_with_diagnostics(&hooks, event, tool, args, mode);
    diagnostics
}

fn tool_requires_user_confirmation(tool: &str) -> bool {
    matches!(
        tool,
        "bash"
            | "write_file"
            | "edit_file"
            | "click"
            | "mouse_click"
            | "click_text"
            | "type"
            | "type_text"
            | "ue5_bridge"
            | "ue5_scene_probe"
            | "blender_bridge"
            | "blender_scene_probe"
            | "unity_bridge"
            | "unity_scene_probe"
            | "video_capture"
    )
}

fn tool_args_preview(args: &str, max_chars: usize) -> String {
    let flattened = args.replace('\r', "\\r").replace('\n', "\\n");
    if flattened.chars().count() <= max_chars {
        return flattened;
    }
    let mut out = flattened.chars().take(max_chars).collect::<String>();
    out.push_str("...");
    out
}

fn add_unique_string(list: &mut Vec<String>, value: String) {
    if !list.iter().any(|v| v == &value) {
        list.push(value);
    }
}

fn interactive_approval_rule(tool: &str, args: &str) -> String {
    match tool {
        "bash" => {
            let prefix = args
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ");
            if prefix.is_empty() {
                "bash".to_string()
            } else {
                format!("bash:{}", prefix)
            }
        }
        "write_file" | "edit_file" => {
            let (first, _) = split_first_arg(args);
            let path = strip_surrounding_quotes(first).trim();
            if path.is_empty() {
                tool.to_string()
            } else {
                format!("{}:{}", tool, path)
            }
        }
        _ => tool.to_string(),
    }
}

fn check_interactive_approval_rules(
    tool: &str,
    args: &str,
    allow_rules: &[String],
    deny_rules: &[String],
) -> Option<ToolApprovalDecision> {
    if let Some(rule) = deny_rules
        .iter()
        .find(|rule| permissions::rule_matches(rule.as_str(), tool, args))
    {
        return Some(ToolApprovalDecision::Deny(format!(
            "[user-rule] blocked by interactive rule `{}`",
            rule
        )));
    }

    if allow_rules
        .iter()
        .any(|rule| permissions::rule_matches(rule.as_str(), tool, args))
    {
        return Some(ToolApprovalDecision::Allow);
    }

    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AutoReviewSeverity {
    Low = 1,
    Medium = 2,
    High = 3,
    Critical = 4,
}

impl AutoReviewSeverity {
    fn as_str(self) -> &'static str {
        match self {
            AutoReviewSeverity::Low => "low",
            AutoReviewSeverity::Medium => "medium",
            AutoReviewSeverity::High => "high",
            AutoReviewSeverity::Critical => "critical",
        }
    }
}

fn parse_auto_review_mode(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "disabled" | "disable" | "none" => "off",
        "warn" | "warning" => "warn",
        "block" | "strict" | "enforce" => "block",
        _ => "off",
    }
}

fn parse_auto_review_threshold(raw: &str) -> AutoReviewSeverity {
    match raw.trim().to_ascii_lowercase().as_str() {
        "critical" | "crit" | "p0" | "c4" => AutoReviewSeverity::Critical,
        "high" | "p1" | "h3" => AutoReviewSeverity::High,
        "medium" | "med" | "p2" | "m2" => AutoReviewSeverity::Medium,
        "low" | "p3" | "l1" => AutoReviewSeverity::Low,
        _ => AutoReviewSeverity::High,
    }
}

fn command_contains_any_ascii_case_insensitive(text: &str, needles: &[&str]) -> bool {
    let lower = text.to_ascii_lowercase();
    needles.iter().any(|n| lower.contains(n))
}

fn score_auto_review_risk(tool: &str, args: &str) -> (AutoReviewSeverity, String) {
    let trimmed = args.trim();
    match tool {
        "bash" => {
            if trimmed.is_empty() {
                return (
                    AutoReviewSeverity::Medium,
                    "empty shell command; ambiguous intent".to_string(),
                );
            }

            if command_contains_any_ascii_case_insensitive(
                trimmed,
                &[
                    "rm -rf",
                    "remove-item -recurse -force",
                    "del /f /q",
                    "format ",
                    "shutdown",
                    "reboot",
                    "set-executionpolicy",
                    "net user",
                    "net localgroup administrators",
                ],
            ) {
                return (
                    AutoReviewSeverity::Critical,
                    "destructive or system-level shell pattern".to_string(),
                );
            }

            if command_contains_any_ascii_case_insensitive(
                trimmed,
                &[
                    "curl ",
                    "wget ",
                    "invoke-webrequest",
                    "invoke-restmethod",
                    "pip install",
                    "npm install -g",
                    "cargo install",
                    "winget install",
                    "choco install",
                ],
            ) {
                return (
                    AutoReviewSeverity::High,
                    "network/package installation shell pattern".to_string(),
                );
            }

            if command_contains_any_ascii_case_insensitive(
                trimmed,
                &["move-item", "copy-item", "rename-item", "git reset --hard"],
            ) {
                return (
                    AutoReviewSeverity::High,
                    "state-mutating shell operation".to_string(),
                );
            }

            (
                AutoReviewSeverity::Medium,
                "shell execution may mutate workspace state".to_string(),
            )
        }
        "write_file" | "edit_file" => (
            AutoReviewSeverity::Medium,
            "file mutation requested".to_string(),
        ),
        "click"
        | "mouse_click"
        | "click_text"
        | "type"
        | "type_text"
        | "ue5_bridge"
        | "ue5_scene_probe"
        | "blender_bridge"
        | "blender_scene_probe"
        | "unity_bridge"
        | "unity_scene_probe"
        | "video_capture" => (
            AutoReviewSeverity::High,
            "desktop input control requested".to_string(),
        ),
        "web_fetch" => (
            AutoReviewSeverity::Low,
            "external fetch request".to_string(),
        ),
        "web_search" => (
            AutoReviewSeverity::Low,
            "external search request".to_string(),
        ),
        _ => (
            AutoReviewSeverity::Low,
            "read-only or low-risk tool".to_string(),
        ),
    }
}

fn auto_review_decision(
    mode_raw: &str,
    threshold_raw: &str,
    tool: &str,
    args: &str,
) -> (bool, AutoReviewSeverity, String) {
    let mode = parse_auto_review_mode(mode_raw);
    let threshold = parse_auto_review_threshold(threshold_raw);
    let (severity, reason) = score_auto_review_risk(tool, args);
    let blocked = mode == "block" && severity >= threshold;
    (blocked, severity, reason)
}

fn maybe_emit_auto_review_warning(
    mode_raw: &str,
    threshold_raw: &str,
    tool: &str,
    severity: AutoReviewSeverity,
    reason: &str,
) {
    let mode = parse_auto_review_mode(mode_raw);
    let threshold = parse_auto_review_threshold(threshold_raw);
    if mode == "warn" && severity >= threshold {
        eprintln!(
            "WARN auto-review tool={} severity={} threshold={} reason={}",
            tool,
            severity.as_str(),
            threshold.as_str(),
            reason
        );
    }
}

fn request_tool_approval(
    mode: &str,
    tool: &str,
    args: &str,
    interactive_allow_rules: &mut Vec<String>,
    interactive_deny_rules: &mut Vec<String>,
) -> Result<ToolApprovalDecision, String> {
    if !is_interactive_permission_mode(mode) || !tool_requires_user_confirmation(tool) {
        return Ok(ToolApprovalDecision::Allow);
    }

    if let Some(decision) = check_interactive_approval_rules(
        tool,
        args,
        interactive_allow_rules,
        interactive_deny_rules,
    ) {
        return Ok(decision);
    }

    if !io::stdin().is_terminal() {
        return Err("interactive approval required but stdin is not a terminal".to_string());
    }

    let preview = tool_args_preview(args, 180);
    let suggested_rule = interactive_approval_rule(tool, args);
    eprintln!();
    eprintln!("APPROVAL required (permission_mode={}):", mode);
    if preview.is_empty() {
        eprintln!("  /toolcall {}", tool);
    } else {
        eprintln!("  /toolcall {} {}", tool, preview);
    }
    eprintln!("  choices: y=yes, n=no, a=always allow (session), d=always deny (session)");
    eprintln!("  suggested rule: {}", suggested_rule);

    loop {
        eprint!("Allow this tool call? [y/N/a/d]: ");
        let _ = io::stderr().flush();

        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("failed to read approval input: {}", e))?;
        let answer = line.trim().to_ascii_lowercase();

        if answer.is_empty() || answer == "n" || answer == "no" {
            return Ok(ToolApprovalDecision::Deny(
                "[user] denied interactive approval".to_string(),
            ));
        }
        if answer == "y" || answer == "yes" {
            return Ok(ToolApprovalDecision::Allow);
        }
        if answer == "a" {
            add_unique_string(interactive_allow_rules, suggested_rule.clone());
            eprintln!("INFO interactive approval rule added: {}", suggested_rule);
            return Ok(ToolApprovalDecision::Allow);
        }
        if answer == "d" {
            add_unique_string(interactive_deny_rules, suggested_rule.clone());
            eprintln!("INFO interactive denial rule added: {}", suggested_rule);
            return Ok(ToolApprovalDecision::Deny(format!(
                "[user-rule] blocked by interactive rule `{}`",
                suggested_rule
            )));
        }
        eprintln!("Please input y, n, a, or d.");
    }
}
impl TurnResult {
    fn with_fallback_note(mut self, error: &str) -> Self {
        if !self.text.is_empty() {
            self.text.push_str(&format!(
                "\n[native tool calling failed: {}, used legacy mode]",
                error
            ));
        }
        self
    }
}

/// User-level hook config: `ASI_USER_HOOKS_PATH`, else `~/.asi/hooks.json`.
/// Unit tests only honor the explicit variable so a developer's installed
/// hooks never change test outcomes.
pub fn user_hooks_config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ASI_USER_HOOKS_PATH").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    if cfg!(test) {
        return None;
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".asi").join("hooks.json"))
}

const COMPACTED_SUMMARY_HEADER: &str = "[COMPACTED CONTEXT — summary of earlier turns]";
const PINNED_TASK_OPEN: &str = "[ORIGINAL TASK — verbatim]";
const PINNED_TASK_CLOSE: &str = "[END ORIGINAL TASK]";

fn is_compacted_summary_message(m: &ChatMessage) -> bool {
    m.role == "assistant" && m.content.starts_with(COMPACTED_SUMMARY_HEADER)
}

fn pinned_task_from_summary(m: &ChatMessage) -> Option<String> {
    if !is_compacted_summary_message(m) {
        return None;
    }
    let start = m.content.find(PINNED_TASK_OPEN)? + PINNED_TASK_OPEN.len();
    let end = m.content[start..].find(PINNED_TASK_CLOSE)? + start;
    Some(m.content[start..end].trim().to_string())
}

/// Index in `body` (history without the system message) where the kept tail
/// starts: the `keep_last`-th user/assistant message from the end, moved
/// back so the tail never begins with orphaned role="tool" results.
fn compaction_split_index(body: &[ChatMessage], keep_last: usize) -> usize {
    let mut split = body.len();
    let mut counted = 0usize;
    while split > 0 && counted < keep_last {
        split -= 1;
        if body[split].role == "user" || body[split].role == "assistant" {
            counted += 1;
        }
    }
    while split > 0 && split < body.len() && body[split].role == "tool" {
        split -= 1;
    }
    split
}

fn clip_with_marker(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars).collect();
    format!("{}…[truncated {} chars]", head, total - max_chars)
}

fn tool_result_looks_failed(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    ["error", "failed", "denied", "not found", "panicked", "exit code", "traceback"]
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Render the region being summarized so the summarizer sees intent,
/// decisions and failures, not bulk file contents: earlier summaries are
/// kept whole, tool calls keep their arguments, successful tool results
/// shrink to their first line, and failures keep enough text to explain why.
fn build_compaction_transcript(drop_region: &[ChatMessage]) -> String {
    let mut transcript = String::new();
    for (i, m) in drop_region.iter().enumerate() {
        let body = if is_compacted_summary_message(m) {
            m.content.clone()
        } else if m.role == "tool" {
            if tool_result_looks_failed(&m.content) {
                format!("FAILED RESULT: {}", clip_with_marker(&m.content, 800))
            } else {
                let first = m.content.lines().next().unwrap_or("");
                format!("ok: {} [body omitted]", clip_with_marker(first, 160))
            }
        } else {
            let mut text = clip_with_marker(&m.content, 600);
            for tc in &m.tool_calls {
                text.push_str(&format!(
                    "\n-> call {}({})",
                    tc.function.name,
                    clip_with_marker(&tc.function.arguments, 200)
                ));
            }
            text
        };
        transcript.push_str(&format!("---\n#{} [{}]:\n{}\n", i + 1, m.role, body));
    }
    transcript
}

/// Evidence gathered during an agent run, rendered for a tools-disabled
/// wrap-up call. Unlike the compaction transcript, tool result bodies are
/// kept (clipped), because the answer usually depends on what was read.
/// Walks newest-first so the most recent evidence survives the budget.
fn build_wrap_up_evidence(body: &[ChatMessage], max_total_chars: usize) -> String {
    let mut blocks: Vec<String> = Vec::new();
    let mut used = 0usize;
    for m in body.iter().rev() {
        let block = if m.role == "tool" {
            format!("[tool result]\n{}", clip_with_marker(&m.content, 6000))
        } else if m.role == "assistant" {
            let mut text = clip_with_marker(&m.content, 1500);
            for tc in &m.tool_calls {
                text.push_str(&format!(
                    "\n-> call {}({})",
                    tc.function.name,
                    clip_with_marker(&tc.function.arguments, 200)
                ));
            }
            if text.trim().is_empty() {
                continue;
            }
            format!("[assistant]\n{}", text)
        } else {
            continue;
        };
        if used + block.len() > max_total_chars {
            blocks.push("[older evidence omitted]".to_string());
            break;
        }
        used += block.len();
        blocks.push(block);
    }
    blocks.reverse();
    blocks.join("\n---\n")
}

const WRAP_UP_SYSTEM_PROMPT: &str = "You are finishing an agent run whose tool budget is exhausted. Tools are disabled: do not request or describe tool calls. Answer the user's task directly from the evidence provided. Be specific and cite file paths. If something could not be checked, say so briefly instead of guessing.";

/// Execute already-approved native tool calls. When every call is a
/// read/search/web tool they run on scoped threads (results keep call
/// order); otherwise they run sequentially so writes and shell commands
/// observe each other's effects.
fn run_native_tools_maybe_parallel(
    runner: &Arc<dyn ToolRunner>,
    pending: &[(usize, String, String)],
) -> Vec<ToolResult> {
    let all_concurrency_safe = pending.iter().all(|(_, name, _)| {
        matches!(
            classify_tool_call(name),
            ToolCallKind::Read | ToolCallKind::Search | ToolCallKind::Web
        )
    });
    if pending.len() < 2 || !all_concurrency_safe {
        return pending
            .iter()
            .map(|(_, name, args)| runner.run(name, args))
            .collect();
    }
    thread::scope(|scope| {
        let handles: Vec<_> = pending
            .iter()
            .map(|(_, name, args)| scope.spawn(move || runner.run(name, args)))
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join().unwrap_or_else(|_| ToolResult {
                    ok: false,
                    output: "Tool worker panicked".to_string(),
                })
            })
            .collect()
    })
}

fn execute_toolcall_command_concurrently(
    index: usize,
    command: String,
    shared: ConcurrentToolExecShared,
) -> ConcurrentToolOutcome {
    let trimmed = command.trim().to_string();
    let rest_owned = trimmed
        .strip_prefix("/toolcall ")
        .unwrap_or(trimmed.as_str())
        .to_string();
    let (name, args_raw) = split_tool(&rest_owned);
    let args = normalize_tool_args(&name, &args_raw);
    log_normalized_toolcall(&name, &args);

    if !is_known_tool(&name) {
        return ConcurrentToolOutcome {
            index,
            command,
            tool_name: name.clone(),
            tool_args: args.clone(),
            text: format!("Unknown tool: {}. Use /tools to see available tools.", name),
            stop_reason: "unknown_tool".to_string(),
            is_tool_result: true,
            audit_allowed: None,
            audit_reason: String::new(),
        };
    }

    for outcome in run_event_hooks(
        &shared.hooks,
        "PreToolUse",
        &name,
        &args,
        &shared.permission_mode,
    ) {
        if !outcome.allow {
            let reason = format!("hook PreToolUse denied: {}", outcome.reason);
            return ConcurrentToolOutcome {
                index,
                command,
                tool_name: name,
                tool_args: args,
                text: format!("Permission denied: {}", reason),
                stop_reason: "permission_denied".to_string(),
                is_tool_result: true,
                audit_allowed: Some(false),
                audit_reason: reason,
            };
        }
    }

    if let Some(reason) = deny_reason(
        &shared.permission_mode,
        &name,
        &args,
        shared.safe_shell_mode,
        shared.disable_web_tools,
        shared.disable_bash_tool,
        &shared.allow_rules,
        &shared.deny_rules,
        shared.path_restriction_enabled,
        &shared.workspace_root,
        &shared.additional_directories,
        &shared.sandbox_strategy,
    ) {
        return ConcurrentToolOutcome {
            index,
            command,
            tool_name: name,
            tool_args: args,
            text: format!("Permission denied: {}", reason),
            stop_reason: "permission_denied".to_string(),
            is_tool_result: true,
            audit_allowed: Some(false),
            audit_reason: reason,
        };
    }

    if is_interactive_permission_mode(&shared.permission_mode)
        && tool_requires_user_confirmation(&name)
    {
        let reason =
            "[approval] interactive approval mode requires sequential execution".to_string();
        return ConcurrentToolOutcome {
            index,
            command,
            tool_name: name,
            tool_args: args,
            text: format!("Permission denied: {}", reason),
            stop_reason: "permission_denied".to_string(),
            is_tool_result: true,
            audit_allowed: Some(false),
            audit_reason: reason,
        };
    }

    for outcome in run_event_hooks(
        &shared.hooks,
        "PermissionRequest",
        &name,
        &args,
        &shared.permission_mode,
    ) {
        if !outcome.allow {
            let reason = format!("hook PermissionRequest denied: {}", outcome.reason);
            return ConcurrentToolOutcome {
                index,
                command,
                tool_name: name,
                tool_args: args,
                text: format!("Permission denied: {}", reason),
                stop_reason: "permission_denied".to_string(),
                is_tool_result: true,
                audit_allowed: Some(false),
                audit_reason: reason,
            };
        }
    }

    let (auto_blocked, auto_severity, auto_reason) = auto_review_decision(
        &shared.auto_review_mode,
        &shared.auto_review_severity_threshold,
        &name,
        &args,
    );
    let _ = audit::log_auto_review_decision(
        &shared.provider,
        &shared.model,
        &shared.permission_mode,
        &shared.auto_review_mode,
        &shared.auto_review_severity_threshold,
        &name,
        &args,
        auto_severity.as_str(),
        auto_blocked,
        &auto_reason,
    );
    maybe_emit_auto_review_warning(
        &shared.auto_review_mode,
        &shared.auto_review_severity_threshold,
        &name,
        auto_severity,
        &auto_reason,
    );
    if auto_blocked {
        let reason = format!(
            "[auto-review] blocked severity={} threshold={} reason={}",
            auto_severity.as_str(),
            parse_auto_review_threshold(&shared.auto_review_severity_threshold).as_str(),
            auto_reason
        );
        return ConcurrentToolOutcome {
            index,
            command,
            tool_name: name,
            tool_args: args,
            text: format!("Permission denied: {}", reason),
            stop_reason: "permission_denied".to_string(),
            is_tool_result: true,
            audit_allowed: Some(false),
            audit_reason: reason,
        };
    }

    let run = shared.tool_runner.run(&name, &args);
    let _ = run_event_hooks(
        &shared.hooks,
        "PostToolUse",
        &name,
        &args,
        &shared.permission_mode,
    );
    let output = format!(
        "[tool:{}:{}]\n{}",
        name,
        if run.ok { "ok" } else { "error" },
        run.output
    );

    ConcurrentToolOutcome {
        index,
        command,
        tool_name: name,
        tool_args: args,
        text: output,
        stop_reason: "tool_result".to_string(),
        is_tool_result: true,
        audit_allowed: Some(true),
        audit_reason: "allowed".to_string(),
    }
}
fn is_known_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "write_file"
            | "edit_file"
            | "glob_search"
            | "grep_search"
            | "web_search"
            | "web_fetch"
            | "bash"
            | "screenshot"
            | "find_window"
            | "click"
            | "mouse_click"
            | "click_text"
            | "type"
            | "type_text"
            | "read_screen_text"
            | "ue5_bridge"
            | "ue5_scene_probe"
            | "blender_bridge"
            | "blender_scene_probe"
            | "unity_bridge"
            | "unity_scene_probe"
            | "probe_diff"
            | "video_capture"
            | "video_keyframes"
    )
}
fn compact_tool_result_for_history(tool: &str, text: &str) -> String {
    // History retention limits per tool. read_file was previously clipped
    // at 2,600 chars (~60-80 lines of code) — far too aggressive: the model
    // would edit files it had only "seen" the top of, fail repeatedly, and
    // trip the failure circuit breaker. Claude Code keeps ~2,000 lines in
    // context; 40,000 chars (~1,000 lines) is a sane middle ground now that
    // auto-compaction handles long-session token pressure.
    let default_limit = match tool {
        "read_file" => 40_000,
        "web_fetch" => 20_000,
        _ => 16_000,
    };
    let limit = std::env::var("ASI_HISTORY_TOOL_RESULT_MAX_CHARS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v >= 1_000)
        .unwrap_or(default_limit);

    if text.chars().count() <= limit {
        return text.to_string();
    }

    let mut clipped = text.chars().take(limit).collect::<String>();
    clipped.push_str("\n[history-truncated to reduce token usage]");
    clipped
}
fn split_tool(input: &str) -> (String, String) {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return (String::new(), String::new());
    }

    if let Some((name, args)) = parse_json_tool_invocation(trimmed) {
        return (name, args);
    }

    if let Some((name, args)) = parse_function_style_toolcall(trimmed) {
        return (name, args);
    }

    let mut it = trimmed.splitn(2, ' ');
    let name = it.next().unwrap_or("").trim().to_string();
    let args = it.next().unwrap_or("").trim().to_string();
    (name, args)
}

pub(crate) fn split_tool_public(input: &str) -> (String, String) {
    split_tool(input)
}

fn parse_json_tool_invocation(input: &str) -> Option<(String, String)> {
    let trimmed = input.trim();
    if !(trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with('"')
        || trimmed.starts_with('\'')
        || trimmed.starts_with('`'))
    {
        return None;
    }
    let value = crate::json_toolcall::parse_relaxed_json_value(trimmed)?;
    let candidate = crate::json_toolcall::first_json_tool_candidate(&value)?;
    let name = crate::json_toolcall::extract_json_tool_name(candidate)?;
    let args = extract_json_tool_args(candidate, &name);
    Some((name, args))
}

fn extract_json_tool_args(candidate: &serde_json::Value, name: &str) -> String {
    let Some(obj) = candidate.as_object() else {
        return String::new();
    };
    let args_value = obj
        .get("function")
        .and_then(serde_json::Value::as_object)
        .and_then(|f| f.get("arguments"))
        .or_else(|| obj.get("arguments"));
    let Some(args_raw) = args_value else {
        return String::new();
    };
    match args_raw {
        serde_json::Value::String(s) => {
            let normalized = crate::json_toolcall::normalize_json_string_argument(s);
            tool_call_to_legacy_args(name, &normalized)
        }
        other => tool_call_to_legacy_args(name, &other.to_string()),
    }
}

fn parse_function_style_toolcall(input: &str) -> Option<(String, String)> {
    let open = input.find('(')?;
    if !input.ends_with(')') {
        return None;
    }

    let name = input[..open].trim();
    if name.is_empty() {
        return None;
    }
    if !is_known_tool(name) {
        return None;
    }

    let inner = &input[open + 1..input.len() - 1];
    let args = if let Some(kv_args) = parse_named_function_args(inner) {
        kv_args
    } else {
        split_function_args(inner)
            .iter()
            .map(|x| normalize_arg_token(x))
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };

    Some((name.to_string(), args))
}

fn parse_named_function_args(inner: &str) -> Option<String> {
    let tokens = split_function_args(inner);
    if tokens.is_empty() {
        return Some(String::new());
    }
    let mut normalized = Vec::with_capacity(tokens.len());
    for token in tokens {
        let trimmed = token.trim();
        let eq = trimmed.find('=')?;
        let key = trimmed[..eq].trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            return None;
        }
        let value_raw = trimmed[eq + 1..].trim();
        let value = normalize_arg_token(value_raw)
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        normalized.push(format!(r#"{}="{}""#, key, value));
    }
    Some(normalized.join(" "))
}

fn split_function_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escape = false;
    let mut depth = 0usize;

    for ch in s.chars() {
        if let Some(q) = quote {
            cur.push(ch);
            if escape {
                escape = false;
                continue;
            }
            if ch == '\\' {
                escape = true;
                continue;
            }
            if ch == q {
                quote = None;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                cur.push(ch);
            }
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(ch);
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                cur.push(ch);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }

    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }

    out
}

fn normalize_arg_token(token: &str) -> String {
    let t = token.trim();
    if t.len() >= 2 {
        let b = t.as_bytes();
        let quoted =
            (b[0] == b'"' && b[t.len() - 1] == b'"') || (b[0] == b'\'' && b[t.len() - 1] == b'\'');
        if quoted {
            return t[1..t.len() - 1].replace("\\\"", "\"").replace("\\'", "'");
        }
    }
    t.to_string()
}

fn normalize_tool_args(name: &str, args: &str) -> String {
    let trimmed = args.trim();
    match name {
        "read_file" | "write_file" | "edit_file" => {
            let (first, rest) = split_first_arg(trimmed);
            if first.is_empty() {
                return String::new();
            }
            let first = strip_surrounding_quotes(first).to_string();
            if rest.trim().is_empty() {
                first
            } else {
                format!("{} {}", first, rest.trim())
            }
        }
        "glob_search"
        | "web_search"
        | "web_fetch"
        | "bash"
        | "find_window"
        | "click_text"
        | "type"
        | "type_text"
        | "ue5_bridge"
        | "ue5_scene_probe"
        | "blender_bridge"
        | "blender_scene_probe"
        | "unity_bridge"
        | "unity_scene_probe"
        | "probe_diff"
        | "video_capture"
        | "video_keyframes" => {
            let sanitized = strip_accidental_trailing_closing_parens(trimmed);
            strip_surrounding_quotes(&sanitized).to_string()
        }
        _ => trimmed.to_string(),
    }
}

fn strip_accidental_trailing_closing_parens(input: &str) -> String {
    let mut current = input.trim().to_string();

    loop {
        let trimmed = current.trim_end();
        if !trimmed.ends_with(')') {
            return trimmed.to_string();
        }

        if !has_unmatched_closing_paren_outside_quotes(trimmed) {
            return trimmed.to_string();
        }

        current = trimmed[..trimmed.len() - 1].trim_end().to_string();
        if current.is_empty() {
            return current;
        }
    }
}

fn has_unmatched_closing_paren_outside_quotes(input: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;
    let mut depth = 0i32;

    for ch in input.chars() {
        if escape {
            escape = false;
            continue;
        }

        if in_single {
            if ch == '\\' {
                escape = true;
            } else if ch == '\'' {
                in_single = false;
            }
            continue;
        }

        if in_double {
            if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_double = false;
            }
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '(' => depth += 1,
            ')' => {
                if depth > 0 {
                    depth -= 1;
                } else {
                    return true;
                }
            }
            _ => {}
        }
    }

    false
}
fn split_first_arg(input: &str) -> (&str, &str) {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return ("", "");
    }

    if let Some(quote) = trimmed.chars().next().filter(|c| *c == '"' || *c == '\'') {
        if let Some(end) = trimmed[1..].find(quote) {
            let first = &trimmed[..end + 2];
            let rest = trimmed[end + 2..].trim();
            return (first, rest);
        }
    }

    let mut it = trimmed.splitn(2, ' ');
    let first = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("");
    (first, rest)
}

fn strip_surrounding_quotes(s: &str) -> &str {
    let trimmed = s.trim();
    if trimmed.len() >= 4 {
        if trimmed.starts_with("\\\"") && trimmed.ends_with("\\\"") {
            return &trimmed[2..trimmed.len() - 2];
        }
        if trimmed.starts_with("\\'") && trimmed.ends_with("\\'") {
            return &trimmed[2..trimmed.len() - 2];
        }
    }

    if trimmed.len() >= 2 {
        let b = trimmed.as_bytes();
        let quoted = (b[0] == b'"' && b[trimmed.len() - 1] == b'"')
            || (b[0] == b'\'' && b[trimmed.len() - 1] == b'\'');
        if quoted {
            return &trimmed[1..trimmed.len() - 1];
        }
    }

    trimmed
}

fn log_normalized_toolcall(name: &str, args: &str) {
    let enabled = std::env::var("ASI_DEBUG_TOOLCALL")
        .ok()
        .map(|v| {
            let lv = v.trim().to_ascii_lowercase();
            lv == "1" || lv == "true" || lv == "yes" || lv == "on"
        })
        .unwrap_or(false);
    if !enabled {
        return;
    }

    let preview = args_log_preview(args, 240);
    if preview.is_empty() {
        eprintln!("INFO toolcall(normalized)> /toolcall {}", name);
    } else {
        eprintln!("INFO toolcall(normalized)> /toolcall {} {}", name, preview);
    }
}

fn args_log_preview(args: &str, max_chars: usize) -> String {
    let flattened = args.replace('\r', "\\r").replace('\n', "\\n");
    let total = flattened.chars().count();
    if total <= max_chars {
        return flattened;
    }

    let mut out = flattened.chars().take(max_chars).collect::<String>();
    out.push_str(&format!("... [truncated {} chars]", total - max_chars));
    out
}

fn deny_reason(
    mode: &str,
    tool: &str,
    args: &str,
    safe_shell_mode: bool,
    disable_web_tools: bool,
    disable_bash_tool: bool,
    allow_rules: &[String],
    deny_rules: &[String],
    path_restriction_enabled: bool,
    workspace_root: &PathBuf,
    additional_directories: &[PathBuf],
    sandbox_strategy: &SandboxStrategy,
) -> Option<String> {
    if disable_web_tools && matches!(tool, "web_search" | "web_fetch") {
        return Some("web tools are disabled by feature killswitch".to_string());
    }
    if disable_bash_tool && tool == "bash" {
        return Some("bash tool is disabled by feature killswitch".to_string());
    }

    match permissions::evaluate_rule_permissions(tool, args, allow_rules, deny_rules) {
        permissions::PermissionDecision::Allow { .. } => {}
        permissions::PermissionDecision::Deny { source, reason } => {
            return Some(format!("[{}] {}", source, reason));
        }
    }

    if let Err(e) = security::guard_secret_file_access(tool, args) {
        return Some(e);
    }

    if let Err(e) = security::guard_tool_path_access(
        tool,
        args,
        path_restriction_enabled,
        workspace_root,
        additional_directories,
    ) {
        return Some(e);
    }

    let sandbox_ctx = SandboxPreflightContext {
        tool,
        args,
        permission_mode: mode,
    };
    if let Err(e) = sandbox_strategy.preflight(&sandbox_ctx) {
        return Some(e);
    }

    let lvl = level(mode);
    let required = match tool {
        "read_file" | "glob_search" | "grep_search" | "web_search" | "web_fetch" | "screenshot"
        | "find_window" | "read_screen_text" | "probe_diff" | "video_keyframes" => 0,
        "write_file"
        | "edit_file"
        | "bash"
        | "click"
        | "mouse_click"
        | "click_text"
        | "type"
        | "type_text"
        | "ue5_bridge"
        | "ue5_scene_probe"
        | "blender_bridge"
        | "blender_scene_probe"
        | "unity_bridge"
        | "unity_scene_probe"
        | "video_capture" => 1,
        _ => 2,
    };
    if lvl < required {
        return Some(format!("tool `{}` requires higher permission", tool));
    }

    if tool == "bash" {
        if let Err(e) = security::guard_bash_command(mode, args, safe_shell_mode) {
            return Some(e);
        }
    }

    None
}

fn run_hook(
    script: &str,
    event: &str,
    tool: &str,
    args: &str,
    mode: &str,
    timeout_secs: u64,
    json_protocol: bool,
    plugin_ctx: Option<&PluginHookContext>,
) -> HookOutcome {
    let timeout = timeout_secs.clamp(1, 30);
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("powershell");
        c.arg("-NoProfile").arg("-Command").arg(script);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.arg("-lc").arg(script);
        c
    };

    cmd.env("ASI_HOOK_EVENT", event)
        .env("ASI_HOOK_TOOL", tool)
        .env("ASI_HOOK_ARGS", args)
        .env("ASI_HOOK_PERMISSION_MODE", mode)
        .env(
            "ASI_HOOK_PLUGIN_COUNT",
            plugin_ctx
                .map(|p| p.count.to_string())
                .unwrap_or_else(|| "0".to_string()),
        )
        .env(
            "ASI_HOOK_PLUGIN_NAMES",
            plugin_ctx.map(|p| p.names_csv.as_str()).unwrap_or(""),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if json_protocol {
        let plugin_names = plugin_ctx
            .map(|p| {
                p.names_csv
                    .split(',')
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let args_details = build_hook_args_details(event, tool, args);
        let payload = json!({
            "schema_version": "1",
            "event_version": "1",
            "event": event,
            "tool": tool,
            "args": args,
            "args_details": args_details,
            "permission_mode": mode,
            "plugins": {
                "count": plugin_ctx.map(|p| p.count).unwrap_or(0),
                "names": plugin_names,
            },
        });
        cmd.env("ASI_HOOK_INPUT_JSON", payload.to_string());
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return HookOutcome {
                allow: false,
                reason: format!("hook spawn failed: {}", e),
                is_error: true,
            }
        }
    };

    let start = std::time::Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let output = child.wait_with_output();
            return match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    if json_protocol && !stdout.is_empty() {
                        if let Some(outcome) = parse_hook_json_outcome(&stdout) {
                            return outcome;
                        }
                    }
                    let mut reason = if !stdout.is_empty() { stdout } else { stderr };
                    if reason.is_empty() {
                        reason = format!("hook exit code {:?}", status.code());
                    }
                    HookOutcome {
                        allow: status.success(),
                        reason,
                        is_error: !status.success(),
                    }
                }
                Err(e) => HookOutcome {
                    allow: false,
                    reason: format!("hook read output failed: {}", e),
                    is_error: true,
                },
            };
        }
        if start.elapsed() > Duration::from_secs(timeout) {
            let _ = child.kill();
            return HookOutcome {
                allow: false,
                reason: format!("hook timeout after {}s", timeout),
                is_error: true,
            };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn parse_hook_json_outcome(stdout: &str) -> Option<HookOutcome> {
    let v = serde_json::from_str::<serde_json::Value>(stdout).ok()?;
    let allow = v.get("allow").and_then(|x| x.as_bool())?;
    let reason = v
        .get("reason")
        .and_then(|x| x.as_str())
        .unwrap_or_else(|| if allow { "ok" } else { "denied" })
        .to_string();
    let is_error = v
        .get("is_error")
        .and_then(|x| x.as_bool())
        .unwrap_or(!allow);
    Some(HookOutcome {
        allow,
        reason,
        is_error,
    })
}

fn parse_hook_failure_policy(raw: &str) -> Option<HookFailurePolicy> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "fail-open" | "open" => Some(HookFailurePolicy::FailOpen),
        "fail-closed" | "closed" => Some(HookFailurePolicy::FailClosed),
        _ => None,
    }
}

fn apply_hook_failure_policy(
    mut outcome: HookOutcome,
    policy: HookFailurePolicy,
    source: &str,
) -> HookOutcome {
    if outcome.allow || !outcome.is_error {
        return outcome;
    }
    if matches!(policy, HookFailurePolicy::FailOpen) {
        outcome.allow = true;
        outcome.reason = format!("{} fail-open: {}", source, outcome.reason);
    }
    outcome
}

fn hook_event_matches(candidate: &str, event: &str) -> bool {
    let c = candidate.trim().to_ascii_lowercase();
    let e = event.trim().to_ascii_lowercase();
    c == "*" || c == e
}

fn hook_handler_matches(handler: &HookHandlerConfig, event: &str, tool: &str, mode: &str) -> bool {
    if !hook_event_matches(&handler.event, event) {
        return false;
    }
    if let Some(prefix) = handler.tool_prefix.as_deref() {
        let p = prefix.trim();
        if !p.is_empty() && !tool.starts_with(p) {
            return false;
        }
    }
    if let Some(pm) = handler.permission_mode.as_deref() {
        let expected = pm.trim();
        if !expected.is_empty() && !mode.eq_ignore_ascii_case(expected) {
            return false;
        }
    }
    true
}

fn plugin_hook_matches(hook: &PluginHookExecution, tool: &str, mode: &str) -> bool {
    if let Some(prefix) = hook.tool_prefix.as_deref() {
        let p = prefix.trim();
        if !p.is_empty() && !tool.starts_with(p) {
            return false;
        }
    }
    if let Some(pm) = hook.permission_mode.as_deref() {
        let expected = pm.trim();
        if !expected.is_empty() && !mode.eq_ignore_ascii_case(expected) {
            return false;
        }
    }
    true
}

fn build_plugin_event_hooks(event: &str) -> Vec<PluginHookExecution> {
    let mut rows = Vec::new();
    for hook in plugin::list_enabled_trusted_plugin_hooks() {
        if !hook_event_matches(&hook.event, event) {
            continue;
        }
        rows.push(PluginHookExecution {
            script: hook.script,
            timeout_secs: hook.timeout_secs,
            json_protocol: hook.json_protocol,
            tool_prefix: hook.tool_prefix,
            permission_mode: hook.permission_mode,
            failure_policy: hook
                .failure_policy
                .as_deref()
                .and_then(parse_hook_failure_policy),
        });
    }
    rows
}

fn run_event_hooks(
    hooks: &HookConfig,
    event: &str,
    tool: &str,
    args: &str,
    mode: &str,
) -> Vec<HookOutcome> {
    let plugin_ctx = build_plugin_hook_context();
    let mut outcomes = Vec::new();

    let env_script = match event {
        "PreToolUse" => hooks.pre_tool_use.as_deref(),
        "PermissionRequest" => hooks.permission_request.as_deref(),
        "PostToolUse" => hooks.post_tool_use.as_deref(),
        "SessionStart" => hooks.session_start.as_deref(),
        "UserPromptSubmit" => hooks.user_prompt_submit.as_deref(),
        "Stop" => hooks.stop.as_deref(),
        "SubagentStop" => hooks.subagent_stop.as_deref(),
        "PreCompact" => hooks.pre_compact.as_deref(),
        "PostCompact" => hooks.post_compact.as_deref(),
        _ => None,
    };
    if let Some(script) = env_script {
        let outcome = run_hook(
            script,
            event,
            tool,
            args,
            mode,
            hooks.timeout_secs,
            hooks.json_protocol,
            plugin_ctx.as_ref(),
        );
        outcomes.push(apply_hook_failure_policy(
            outcome,
            hooks.failure_policy,
            "env-hook",
        ));
    }

    for handler in &hooks.handlers {
        if !hook_handler_matches(handler, event, tool, mode) {
            continue;
        }
        let outcome = run_hook(
            &handler.script,
            event,
            tool,
            args,
            mode,
            handler.timeout_secs.unwrap_or(hooks.timeout_secs),
            handler.json_protocol.unwrap_or(hooks.json_protocol),
            plugin_ctx.as_ref(),
        );
        outcomes.push(apply_hook_failure_policy(
            outcome,
            handler.failure_policy.unwrap_or(hooks.failure_policy),
            "config-hook",
        ));
    }

    for plugin_hook in build_plugin_event_hooks(event) {
        if !plugin_hook_matches(&plugin_hook, tool, mode) {
            continue;
        }
        let outcome = run_hook(
            &plugin_hook.script,
            event,
            tool,
            args,
            mode,
            plugin_hook.timeout_secs.unwrap_or(hooks.timeout_secs),
            plugin_hook.json_protocol.unwrap_or(hooks.json_protocol),
            plugin_ctx.as_ref(),
        );
        outcomes.push(apply_hook_failure_policy(
            outcome,
            plugin_hook.failure_policy.unwrap_or(hooks.failure_policy),
            "plugin-hook",
        ));
    }

    outcomes
}

fn run_event_hooks_with_diagnostics(
    hooks: &HookConfig,
    event: &str,
    tool: &str,
    args: &str,
    mode: &str,
) -> (Vec<HookOutcome>, Vec<HookDiagnostic>) {
    let plugin_ctx = build_plugin_hook_context();
    let mut outcomes = Vec::new();
    let mut diagnostics = Vec::new();

    let env_script = match event {
        "PreToolUse" => hooks.pre_tool_use.as_deref(),
        "PermissionRequest" => hooks.permission_request.as_deref(),
        "PostToolUse" => hooks.post_tool_use.as_deref(),
        "SessionStart" => hooks.session_start.as_deref(),
        "UserPromptSubmit" => hooks.user_prompt_submit.as_deref(),
        "Stop" => hooks.stop.as_deref(),
        "SubagentStop" => hooks.subagent_stop.as_deref(),
        "PreCompact" => hooks.pre_compact.as_deref(),
        "PostCompact" => hooks.post_compact.as_deref(),
        _ => None,
    };
    if let Some(script) = env_script {
        let base = run_hook(
            script,
            event,
            tool,
            args,
            mode,
            hooks.timeout_secs,
            hooks.json_protocol,
            plugin_ctx.as_ref(),
        );
        let final_outcome = apply_hook_failure_policy(base, hooks.failure_policy, "env-hook");
        diagnostics.push(HookDiagnostic {
            source: "env-hook".to_string(),
            event: event.to_string(),
            allow: final_outcome.allow,
            is_error: final_outcome.is_error,
            reason: final_outcome.reason.clone(),
        });
        outcomes.push(final_outcome);
    }

    for handler in &hooks.handlers {
        if !hook_handler_matches(handler, event, tool, mode) {
            continue;
        }
        let base = run_hook(
            &handler.script,
            event,
            tool,
            args,
            mode,
            handler.timeout_secs.unwrap_or(hooks.timeout_secs),
            handler.json_protocol.unwrap_or(hooks.json_protocol),
            plugin_ctx.as_ref(),
        );
        let final_outcome = apply_hook_failure_policy(
            base,
            handler.failure_policy.unwrap_or(hooks.failure_policy),
            "config-hook",
        );
        diagnostics.push(HookDiagnostic {
            source: "config-hook".to_string(),
            event: event.to_string(),
            allow: final_outcome.allow,
            is_error: final_outcome.is_error,
            reason: final_outcome.reason.clone(),
        });
        outcomes.push(final_outcome);
    }

    for plugin_hook in build_plugin_event_hooks(event) {
        if !plugin_hook_matches(&plugin_hook, tool, mode) {
            continue;
        }
        let base = run_hook(
            &plugin_hook.script,
            event,
            tool,
            args,
            mode,
            plugin_hook.timeout_secs.unwrap_or(hooks.timeout_secs),
            plugin_hook.json_protocol.unwrap_or(hooks.json_protocol),
            plugin_ctx.as_ref(),
        );
        let final_outcome = apply_hook_failure_policy(
            base,
            plugin_hook.failure_policy.unwrap_or(hooks.failure_policy),
            "plugin-hook",
        );
        diagnostics.push(HookDiagnostic {
            source: "plugin-hook".to_string(),
            event: event.to_string(),
            allow: final_outcome.allow,
            is_error: final_outcome.is_error,
            reason: final_outcome.reason.clone(),
        });
        outcomes.push(final_outcome);
    }

    (outcomes, diagnostics)
}

fn build_plugin_hook_context() -> Option<PluginHookContext> {
    let mut names = plugin::verify_enabled_plugins()
        .into_iter()
        .map(|p| p.name)
        .collect::<Vec<_>>();
    if names.is_empty() {
        return None;
    }
    names.sort();
    Some(PluginHookContext {
        names_csv: names.join(","),
        count: names.len(),
    })
}

fn level(mode: &str) -> i32 {
    match mode {
        "read-only" => 0,
        "workspace-write" => 1,
        "danger-full-access" | "bypass-permissions" => 2,
        _ => 2,
    }
}

fn merge_unique_strings(base: &[String], extra: &[String]) -> Vec<String> {
    let mut out = base.to_vec();
    for item in extra {
        if !out.iter().any(|x| x == item) {
            out.push(item.clone());
        }
    }
    out
}

fn merge_unique_paths(base: &[PathBuf], extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = base.to_vec();
    for item in extra {
        if !out.iter().any(|x| x == item) {
            out.push(item.clone());
        }
    }
    out
}

fn parse_kv_pairs(raw: &str) -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    for token in raw.split_whitespace() {
        let Some((k, v)) = token.split_once('=') else {
            continue;
        };
        if k.trim().is_empty() {
            continue;
        }
        out.insert(
            k.trim().to_string(),
            serde_json::Value::String(v.trim().to_string()),
        );
    }
    out
}

fn build_hook_args_details(event: &str, tool: &str, args: &str) -> serde_json::Value {
    match event {
        "SessionStart" => json!({
            "kind": "session_start",
            "action": args,
        }),
        "UserPromptSubmit" => json!({
            "kind": "user_prompt_submit",
            "prompt": args,
            "prompt_len": args.chars().count(),
        }),
        "Stop" => json!({
            "kind": "stop",
            "stop_reason": args,
        }),
        "SubagentStop" => {
            let parsed = parse_kv_pairs(args);
            json!({
                "kind": "subagent_stop",
                "tool": tool,
                "fields": parsed,
            })
        }
        "PreCompact" | "PostCompact" => json!({
            "kind": if event == "PreCompact" { "pre_compact" } else { "post_compact" },
            "action": args,
        }),
        "PreToolUse" | "PermissionRequest" | "PostToolUse" => json!({
            "kind": "tool_event",
            "tool": tool,
            "tool_args": args,
        }),
        _ => json!({
            "kind": "generic",
            "tool": tool,
            "raw_args": args,
        }),
    }
}

fn estimate_tokens(text: &str) -> usize {
    (text.len() / 4).max(1)
}

fn generate_thinking(user_text: &str) -> String {
    let brief = user_text.chars().take(72).collect::<String>();
    format!(
        "Thinking block:\n- Understand intent: {}\n- Plan minimal safe steps\n- Execute and verify output",
        brief
    )
}

#[cfg(test)]
mod native_parallel_tests {
    use super::run_native_tools_maybe_parallel;
    use crate::tools::{ToolResult, ToolRunner};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[derive(Debug)]
    struct SleepyRunner;

    impl ToolRunner for SleepyRunner {
        fn run(&self, name: &str, args: &str) -> ToolResult {
            std::thread::sleep(Duration::from_millis(200));
            ToolResult {
                ok: true,
                output: format!("{}:{}", name, args),
            }
        }
    }

    fn pending(calls: &[(&str, &str)]) -> Vec<(usize, String, String)> {
        calls
            .iter()
            .enumerate()
            .map(|(i, (n, a))| (i, n.to_string(), a.to_string()))
            .collect()
    }

    #[test]
    fn native_read_only_calls_run_in_parallel_and_keep_order() {
        let runner: Arc<dyn ToolRunner> = Arc::new(SleepyRunner);
        let calls = pending(&[("read_file", "a.rs"), ("grep_search", "foo"), ("glob_search", "*.rs")]);
        let started = Instant::now();
        let out = run_native_tools_maybe_parallel(&runner, &calls);
        assert!(started.elapsed() < Duration::from_millis(500), "expected parallel execution");
        let texts: Vec<_> = out.iter().map(|r| r.output.as_str()).collect();
        assert_eq!(texts, vec!["read_file:a.rs", "grep_search:foo", "glob_search:*.rs"]);
    }

    #[test]
    fn native_calls_with_a_write_run_sequentially() {
        let runner: Arc<dyn ToolRunner> = Arc::new(SleepyRunner);
        let calls = pending(&[("read_file", "a.rs"), ("write_file", "b.rs"), ("read_file", "b.rs")]);
        let started = Instant::now();
        let out = run_native_tools_maybe_parallel(&runner, &calls);
        assert!(started.elapsed() >= Duration::from_millis(600), "writes must not run concurrently");
        assert_eq!(out[1].output, "write_file:b.rs");
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_hook_failure_policy, auto_review_decision, build_hook_args_details,
        build_plugin_event_hooks, check_interactive_approval_rules, hook_event_matches,
        hook_handler_matches, interactive_approval_rule, is_interactive_permission_mode,
        normalize_tool_args, parse_auto_review_mode, parse_auto_review_threshold,
        parse_hook_failure_policy, parse_hook_json_outcome, parse_json_tool_invocation,
        plugin_hook_matches, score_auto_review_risk, tool_requires_user_confirmation,
        HookFailurePolicy, HookHandlerConfig, PluginHookExecution, ProviderErrorKind,
        Runtime as RuntimeType, ToolApprovalDecision,
    };
    use crate::provider::{ChatMessage, ChatProvider, CompletionResult, StreamingResult};
    use crate::runtime::Runtime;
    use std::collections::VecDeque;
    use std::fs;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct EnvGuard {
        key: &'static str,
        old: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let old = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(v) = &self.old {
                std::env::set_var(self.key, v);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[derive(Debug)]
    struct MockProvider {
        streaming: Arc<Mutex<VecDeque<Result<StreamingResult, String>>>>,
        with_tools: Arc<Mutex<VecDeque<Result<CompletionResult, String>>>>,
    }

    impl MockProvider {
        fn with_tools_responses(responses: Vec<Result<CompletionResult, String>>) -> Self {
            Self {
                streaming: Arc::new(Mutex::new(VecDeque::new())),
                with_tools: Arc::new(Mutex::new(responses.into())),
            }
        }

        fn with_streaming(responses: Vec<Result<StreamingResult, String>>) -> Self {
            Self {
                streaming: Arc::new(Mutex::new(responses.into())),
                with_tools: Arc::new(Mutex::new(VecDeque::new())),
            }
        }
    }

    impl ChatProvider for MockProvider {
        fn complete_streaming_dyn(
            &self,
            _messages: &[ChatMessage],
            _on_delta: &mut dyn FnMut(&str),
        ) -> Result<StreamingResult, String> {
            let mut q = self.streaming.lock().unwrap();
            q.pop_front().unwrap_or_else(|| {
                Ok(StreamingResult {
                    text: "mock-default-streaming".to_string(),
                    stop_reason: "completed".to_string(),
                })
            })
        }

        fn complete_with_tools_dyn(
            &self,
            _messages: &[ChatMessage],
            _tool_results: &[(String, String)],
            _on_delta: &mut dyn FnMut(&str),
        ) -> Result<CompletionResult, String> {
            let mut q = self.with_tools.lock().unwrap();
            q.pop_front().unwrap_or_else(|| {
                Ok(CompletionResult {
                    text: "mock-default-with-tools".to_string(),
                    tool_calls: Vec::new(),
                    stop_reason: "completed".to_string(),
                    reasoning_content: String::new(),
                })
            })
        }
    }

    fn make_temp_dir(prefix: &str) -> std::path::PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{}_{}", prefix, ts));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_text(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
    }

    #[test]
    fn normalize_tool_args_strips_accidental_trailing_paren_for_bash() {
        let args = normalize_tool_args("bash", "\"python --version\")");
        assert_eq!(args, "python --version");
    }

    #[test]
    fn normalize_tool_args_strips_accidental_trailing_paren_for_glob_search() {
        let args = normalize_tool_args("glob_search", "\"**/*\")");
        assert_eq!(args, "**/*");
    }

    #[test]
    fn normalize_tool_args_preserves_balanced_parens() {
        let args = normalize_tool_args("bash", "(Get-Item .)");
        assert_eq!(args, "(Get-Item .)");
    }

    #[test]
    fn parse_function_style_toolcall_supports_named_arguments() {
        let line = r#"bash(command="Write-Output 'ok'; python --version 2>&1")"#;
        let parsed = super::parse_function_style_toolcall(line).expect("named call");
        assert_eq!(parsed.0, "bash");
        assert!(parsed.1.contains(r#"command=""#));
        assert!(parsed.1.contains("Write-Output"));
    }

    #[test]
    fn parse_function_style_toolcall_ignores_non_tool_headings() {
        let line = "Confidence Declaration: high";
        assert!(super::parse_function_style_toolcall(line).is_none());
    }

    #[test]
    fn parse_json_tool_invocation_supports_function_wrapper_array() {
        let raw = r#"[{"type":"function","function":{"name":"glob_search","arguments":"{\"pattern\":\"*.py\"}"}}]"#;
        let parsed = parse_json_tool_invocation(raw).expect("should parse");
        assert_eq!(parsed.0, "glob_search");
        assert_eq!(parsed.1, "*.py");
    }

    #[test]
    fn parse_json_tool_invocation_supports_escaped_wrapper_text() {
        let raw = r#"[{\"type\":\"function\",\"function\":{\"name\":\"glob_search\",\"arguments\":\"{\\\"pattern\\\":\\\"*.py\\\"}\"}}]"#;
        let parsed = parse_json_tool_invocation(raw).expect("should parse escaped wrapper");
        assert_eq!(parsed.0, "glob_search");
        assert_eq!(parsed.1, "*.py");
    }

    #[test]
    fn interactive_permission_mode_aliases_are_supported() {
        assert!(is_interactive_permission_mode("ask"));
        assert!(is_interactive_permission_mode("on-request"));
        assert!(is_interactive_permission_mode("on_request"));
        assert!(!is_interactive_permission_mode("workspace-write"));
    }

    #[test]
    fn tool_confirmation_scope_is_mutating_tools() {
        assert!(tool_requires_user_confirmation("bash"));
        assert!(tool_requires_user_confirmation("write_file"));
        assert!(tool_requires_user_confirmation("edit_file"));
        assert!(tool_requires_user_confirmation("click"));
        assert!(tool_requires_user_confirmation("type_text"));
        assert!(tool_requires_user_confirmation("blender_scene_probe"));
        assert!(tool_requires_user_confirmation("ue5_scene_probe"));
        assert!(tool_requires_user_confirmation("unity_scene_probe"));
        assert!(tool_requires_user_confirmation("video_capture"));
        assert!(!tool_requires_user_confirmation("read_file"));
        assert!(!tool_requires_user_confirmation("screenshot"));
        assert!(!tool_requires_user_confirmation("video_keyframes"));
    }

    #[test]
    fn interactive_approval_rule_uses_command_prefix_for_bash() {
        let rule = interactive_approval_rule(
            "bash",
            "npm.cmd install -g yarn --registry https://registry.npmjs.org",
        );
        assert_eq!(rule, "bash:npm.cmd install -g");
    }

    #[test]
    fn interactive_approval_rule_uses_path_for_write_edit() {
        let rule = interactive_approval_rule("write_file", "\"src/main.rs\" hello");
        assert_eq!(rule, "write_file:src/main.rs");
    }

    #[test]
    fn interactive_rule_check_prioritizes_deny_over_allow() {
        let allow = vec!["bash:npm.cmd install -g".to_string()];
        let deny = vec!["bash:npm.cmd install -g".to_string()];
        let decision =
            check_interactive_approval_rules("bash", "npm.cmd install -g yarn", &allow, &deny);
        match decision {
            Some(ToolApprovalDecision::Deny(reason)) => {
                assert!(reason.contains("interactive rule"));
            }
            _ => panic!("expected deny decision"),
        }
    }

    #[test]
    fn auto_review_mode_parser_accepts_warn_alias() {
        assert_eq!(parse_auto_review_mode("warn"), "warn");
        assert_eq!(parse_auto_review_mode("warning"), "warn");
        assert_eq!(parse_auto_review_mode("BLOCK"), "block");
        assert_eq!(parse_auto_review_mode("unknown"), "off");
    }

    #[test]
    fn auto_review_threshold_parser_supports_aliases() {
        assert_eq!(parse_auto_review_threshold("critical").as_str(), "critical");
        assert_eq!(parse_auto_review_threshold("p1").as_str(), "high");
        assert_eq!(parse_auto_review_threshold("med").as_str(), "medium");
        assert_eq!(parse_auto_review_threshold("l1").as_str(), "low");
        assert_eq!(parse_auto_review_threshold("unknown").as_str(), "high");
    }

    #[test]
    fn auto_review_risk_score_marks_destructive_bash_as_critical() {
        let (severity, reason) = score_auto_review_risk("bash", "rm -rf ./tmp");
        assert_eq!(severity.as_str(), "critical");
        assert!(reason.contains("destructive") || reason.contains("system-level"));
    }

    #[test]
    fn auto_review_warn_mode_never_blocks_by_itself() {
        let (blocked, severity, reason) =
            auto_review_decision("warn", "low", "bash", "rm -rf ./tmp");
        assert!(!blocked);
        assert_eq!(severity.as_str(), "critical");
        assert!(!reason.trim().is_empty());
    }

    #[test]
    fn auto_review_block_mode_blocks_when_severity_meets_threshold() {
        let (blocked, severity, _) =
            auto_review_decision("block", "high", "bash", "pip install requests");
        assert!(blocked);
        assert!(severity.as_str() == "high" || severity.as_str() == "critical");
    }

    #[test]
    fn auto_review_block_mode_allows_when_below_threshold() {
        let (blocked, severity, _) =
            auto_review_decision("block", "critical", "write_file", "src/main.rs hello");
        assert!(!blocked);
        assert_eq!(severity.as_str(), "medium");
    }

    #[test]
    fn load_project_context_prefers_single_claude_md_when_present() {
        let dir = make_temp_dir("asi_runtime_ctx_claude");
        write_text(&dir.join("CLAUDE.md"), "# single policy\nrule=claude-only");
        write_text(&dir.join("README.md"), "# readme\nshould_not_be_loaded");
        write_text(&dir.join("AGENTS.md"), "# agents\nshould_not_be_loaded");

        let ctx = Runtime::load_project_context_with_mode(&dir, true);
        assert!(ctx.text.contains("CLAUDE.md"));
        assert!(ctx.text.contains("rule=claude-only"));
        assert!(!ctx.text.contains("should_not_be_loaded"));
        assert!(ctx.sources.iter().any(|s| s.ends_with("CLAUDE.md")));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_project_context_falls_back_to_readme_and_agents_without_claude() {
        let dir = make_temp_dir("asi_runtime_ctx_fallback");
        write_text(&dir.join("README.md"), "# readme\nfallback-readme");
        write_text(&dir.join("AGENTS.md"), "# agents\nfallback-agents");

        let ctx = Runtime::load_project_context_with_mode(&dir, true);
        assert!(ctx.text.contains("README.md"));
        assert!(ctx.text.contains("AGENTS.md"));
        assert!(ctx.text.contains("fallback-readme"));
        assert!(ctx.text.contains("fallback-agents"));
        assert!(ctx.sources.iter().any(|s| s.ends_with("README.md")));
        assert!(ctx.sources.iter().any(|s| s.ends_with("AGENTS.md")));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_project_context_fallback_prefers_override_then_local_then_base() {
        let dir = make_temp_dir("asi_runtime_ctx_fallback_override");
        write_text(&dir.join("README.md"), "# readme\nbase-readme");
        write_text(&dir.join("README.local.md"), "# readme-local\nlocal-readme");
        write_text(
            &dir.join("README.override.md"),
            "# readme-override\noverride-readme",
        );
        write_text(&dir.join("AGENTS.md"), "# agents\nbase-agents");
        write_text(&dir.join("AGENTS.local.md"), "# agents-local\nlocal-agents");
        write_text(
            &dir.join("AGENTS.override.md"),
            "# agents-override\noverride-agents",
        );

        let ctx = Runtime::load_project_context_with_mode(&dir, true);
        assert!(ctx.text.contains("README.override.md"));
        assert!(ctx.text.contains("README.local.md"));
        assert!(ctx.text.contains("README.md"));
        assert!(ctx.text.contains("AGENTS.override.md"));
        assert!(ctx.text.contains("AGENTS.local.md"));
        assert!(ctx.text.contains("AGENTS.md"));
        let override_readme_idx = ctx
            .text
            .find("README.override.md")
            .expect("override readme present");
        let local_readme_idx = ctx
            .text
            .find("README.local.md")
            .expect("local readme present");
        let base_readme_idx = ctx.text.find("README.md").expect("base readme present");
        assert!(override_readme_idx < local_readme_idx);
        assert!(local_readme_idx < base_readme_idx);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_project_context_single_claude_mode_loads_override_and_local() {
        let dir = make_temp_dir("asi_runtime_ctx_claude_override");
        write_text(&dir.join("CLAUDE.md"), "# claude\nbase-claude");
        write_text(&dir.join("CLAUDE.local.md"), "# claude-local\nlocal-claude");
        write_text(
            &dir.join("CLAUDE.override.md"),
            "# claude-override\noverride-claude",
        );
        write_text(&dir.join("AGENTS.md"), "# agents\nfallback-agents");

        let ctx = Runtime::load_project_context_with_mode(&dir, true);
        assert!(ctx.text.contains("CLAUDE.override.md"));
        assert!(ctx.text.contains("CLAUDE.local.md"));
        assert!(ctx.text.contains("CLAUDE.md"));
        assert!(ctx.text.contains("override-claude"));
        assert!(!ctx.text.contains("fallback-agents"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_project_context_includes_all_docs_when_single_mode_disabled() {
        let dir = make_temp_dir("asi_runtime_ctx_multi");
        write_text(&dir.join("CLAUDE.md"), "# claude\nfrom-claude");
        write_text(&dir.join("README.md"), "# readme\nfrom-readme");
        write_text(&dir.join("AGENTS.md"), "# agents\nfrom-agents");

        let ctx = Runtime::load_project_context_with_mode(&dir, false);
        assert!(ctx.text.contains("CLAUDE.md"));
        assert!(ctx.text.contains("README.md"));
        assert!(ctx.text.contains("AGENTS.md"));
        assert!(ctx.text.contains("from-claude"));
        assert!(ctx.text.contains("from-readme"));
        assert!(ctx.text.contains("from-agents"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn layered_project_context_loads_ancestor_agents() {
        let root = make_temp_dir("asi_runtime_ctx_layered_root");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("create child");

        write_text(&root.join("AGENTS.md"), "# agents\nroot-agents");
        write_text(&child.join("README.md"), "# readme\nchild-readme");

        let old = std::env::var("ASI_PROJECT_INSTRUCTIONS_LAYERED").ok();
        std::env::set_var("ASI_PROJECT_INSTRUCTIONS_LAYERED", "1");

        let ctx = Runtime::load_project_context_with_mode(&child, false);
        assert!(ctx.text.contains("root-agents"));
        assert!(ctx.text.contains("child-readme"));
        assert!(ctx.sources.iter().any(|s| s.ends_with("AGENTS.md")));
        assert!(ctx.sources.iter().any(|s| s.ends_with("README.md")));

        match old {
            Some(v) => std::env::set_var("ASI_PROJECT_INSTRUCTIONS_LAYERED", v),
            None => std::env::remove_var("ASI_PROJECT_INSTRUCTIONS_LAYERED"),
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn layered_project_context_loads_ancestor_override_agents() {
        let root = make_temp_dir("asi_runtime_ctx_layered_override_root");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("create child");

        write_text(
            &root.join("AGENTS.override.md"),
            "# agents-override\nroot-agents-override",
        );
        write_text(&child.join("AGENTS.md"), "# agents\nchild-agents");

        let old = std::env::var("ASI_PROJECT_INSTRUCTIONS_LAYERED").ok();
        std::env::set_var("ASI_PROJECT_INSTRUCTIONS_LAYERED", "1");

        let ctx = Runtime::load_project_context_with_mode(&child, false);
        assert!(ctx.text.contains("root-agents-override"));
        assert!(ctx.text.contains("child-agents"));
        assert!(ctx
            .sources
            .iter()
            .any(|s| s.ends_with("AGENTS.override.md")));
        assert!(ctx.sources.iter().any(|s| s.ends_with("AGENTS.md")));

        match old {
            Some(v) => std::env::set_var("ASI_PROJECT_INSTRUCTIONS_LAYERED", v),
            None => std::env::remove_var("ASI_PROJECT_INSTRUCTIONS_LAYERED"),
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_single_mode_parser_supports_true_and_false_values() {
        assert!(!Runtime::claude_single_mode_from_env(Some("0")));
        assert!(!Runtime::claude_single_mode_from_env(Some("false")));
        assert!(!Runtime::claude_single_mode_from_env(Some("off")));
        assert!(Runtime::claude_single_mode_from_env(Some("1")));
        assert!(Runtime::claude_single_mode_from_env(Some("true")));
        assert!(Runtime::claude_single_mode_from_env(Some("on")));
        assert!(Runtime::claude_single_mode_from_env(None));
    }

    #[test]
    fn retryable_provider_error_includes_decoding_failures() {
        assert!(RuntimeType::is_retryable_provider_error(
            "error decoding response body"
        ));
        assert!(RuntimeType::is_retryable_provider_error(
            "Provider error: decode error"
        ));
    }

    #[test]
    fn decode_error_retry_counter_increments_on_successful_retry() {
        let mut rt = Runtime::new(
            "deepseek".to_string(),
            "deepseek-v4-pro".to_string(),
            "workspace-write".to_string(),
            8,
        );
        rt.client = Box::new(MockProvider::with_streaming(vec![
            Err("error decoding response body".to_string()),
            Ok(StreamingResult {
                text: "API_OK".to_string(),
                stop_reason: "completed".to_string(),
            }),
        ]));
        rt.native_tool_calling = false;

        let _env_guard = EnvGuard::set("ASI_PROVIDER_MAX_RETRIES", "1");
        let _turn = rt.run_turn("ping");

        assert_eq!(rt.provider_decode_retry_count(), 1);
        assert_eq!(rt.provider_decode_final_fail_count(), 0);
    }

    fn tool_call_spec(id: &str, name: &str, args: &str) -> crate::provider::ToolCallSpec {
        crate::provider::ToolCallSpec {
            id: id.to_string(),
            call_type: "function".to_string(),
            function: crate::provider::ToolCallFunction {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    fn mock_summary(text: &str) -> Result<StreamingResult, String> {
        Ok(StreamingResult {
            text: text.to_string(),
            stop_reason: "completed".to_string(),
        })
    }

    /// History: task, then `rounds` native tool rounds (assistant tool_calls
    /// followed by a tool result) each with a large file body.
    fn runtime_with_tool_history(rounds: usize) -> Runtime {
        let mut rt = Runtime::new(
            "openai".to_string(),
            "gpt-test".to_string(),
            "workspace-write".to_string(),
            200,
        );
        rt.messages.push(ChatMessage::text("user", "Fix the parser bug in src/lib.rs".to_string()));
        for i in 0..rounds {
            let id = format!("call_{}", i);
            rt.messages.push(ChatMessage::assistant_with_tool_calls_and_reasoning(
                String::new(),
                vec![tool_call_spec(&id, "read_file", "{\"path\":\"src/lib.rs\"}")],
                String::new(),
            ));
            rt.messages.push(ChatMessage::tool_result(id, format!("src/lib.rs lines 1-400\n{}", "x".repeat(4000))));
        }
        rt
    }

    fn assert_tool_protocol_is_paired(messages: &[ChatMessage]) {
        for (i, m) in messages.iter().enumerate() {
            if m.role == "tool" {
                let id = m.tool_call_id.clone().unwrap_or_default();
                let has_owner = messages[..i]
                    .iter()
                    .any(|p| p.tool_calls.iter().any(|tc| tc.id == id));
                assert!(has_owner, "orphaned tool result {} at index {}", id, i);
            }
        }
    }

    #[test]
    fn compaction_keeps_native_tool_protocol_paired() {
        let mut rt = runtime_with_tool_history(10);
        rt.client = Box::new(MockProvider::with_streaming(vec![mock_summary("WORK DONE: read lib.rs")]));
        let msg = rt.compact_with_summary(3);
        assert!(msg.contains("structured summary"), "{}", msg);
        assert!(rt.messages.iter().any(|m| m.role == "tool"), "recent tool results must be kept");
        assert_tool_protocol_is_paired(&rt.messages);
    }

    #[test]
    fn compaction_pins_task_verbatim_across_repeated_rounds() {
        let mut rt = runtime_with_tool_history(6);
        rt.client = Box::new(MockProvider::with_streaming(vec![
            mock_summary("first summary"),
            mock_summary("second summary"),
        ]));
        rt.compact_with_summary(2);
        for i in 0..6 {
            let id = format!("later_{}", i);
            rt.messages.push(ChatMessage::assistant_with_tool_calls_and_reasoning(
                String::new(),
                vec![tool_call_spec(&id, "grep_search", "{\"pattern\":\"fn\"}")],
                String::new(),
            ));
            rt.messages.push(ChatMessage::tool_result(id, "ok".to_string()));
        }
        rt.compact_with_summary(2);
        let summary = rt.messages[1].content.clone();
        assert!(summary.contains("[ORIGINAL TASK — verbatim]\nFix the parser bug in src/lib.rs\n"), "{}", summary);
        assert!(summary.contains("second summary"));
        assert_tool_protocol_is_paired(&rt.messages);
    }

    #[test]
    fn compaction_lossy_fallback_still_pins_task() {
        let mut rt = runtime_with_tool_history(6);
        rt.client = Box::new(MockProvider::with_streaming(vec![Err("boom".to_string())]));
        let msg = rt.compact_with_summary_for_task(2, Some("Ship feature X"));
        assert!(msg.contains("lossy"), "{}", msg);
        assert!(rt.messages[1].content.contains("Ship feature X"));
        assert_tool_protocol_is_paired(&rt.messages);
    }

    #[test]
    fn auto_compact_trigger_tracks_live_context_not_cumulative_tokens() {
        let mut rt = runtime_with_tool_history(10);
        rt.cumulative_input_tokens = 1_000_000;
        let threshold = 5_000;
        assert!(rt.auto_compact_recommendation(threshold).is_some());
        rt.client = Box::new(MockProvider::with_streaming(vec![mock_summary("short summary")]));
        rt.compact_with_summary(1);
        assert!(
            rt.auto_compact_recommendation(threshold).is_none(),
            "after compaction the live context is small; must not re-trigger (context={})",
            rt.context_tokens_estimate()
        );
    }

    #[test]
    fn wrap_up_turns_tool_evidence_into_final_answer() {
        let mut rt = runtime_with_tool_history(3);
        rt.client = Box::new(MockProvider::with_streaming(vec![mock_summary(
            "Bug: median() ignores even-length lists (src/lib.rs).",
        )]));
        let answer = rt
            .wrap_up_without_tools("Find the bugs", "reached max tool steps")
            .expect("wrap-up answer");
        assert!(answer.text.contains("median()"));
        assert!(!answer.is_tool_result);
        assert_eq!(rt.messages.last().unwrap().role, "assistant");
        assert!(!rt.last_message_is_tool_result());
        assert_tool_protocol_is_paired(&rt.messages);
    }

    #[test]
    fn wrap_up_failure_leaves_history_untouched() {
        let mut rt = runtime_with_tool_history(2);
        let before = rt.messages.len();
        rt.client = Box::new(MockProvider::with_streaming(vec![Err("boom".to_string())]));
        assert!(rt.wrap_up_without_tools("Find the bugs", "reached max tool steps").is_none());
        assert_eq!(rt.messages.len(), before);
    }

    #[test]
    fn wrap_up_evidence_keeps_file_bodies_and_prefers_newest() {
        let mut body = Vec::new();
        for i in 0..30 {
            body.push(ChatMessage::tool_result(
                format!("c{}", i),
                format!("FILE_{} {}", i, "y".repeat(5000)),
            ));
        }
        let evidence = super::build_wrap_up_evidence(&body, 20_000);
        assert!(evidence.contains("FILE_29"), "newest evidence must be kept");
        assert!(!evidence.contains("FILE_0 "), "oldest evidence should be dropped under budget");
        assert!(evidence.starts_with("[older evidence omitted]"));
    }

    #[test]
    fn compaction_transcript_drops_file_bodies_but_keeps_failures() {
        let region = vec![
            ChatMessage::assistant_with_tool_calls_and_reasoning(
                "Reading".to_string(),
                vec![tool_call_spec("a", "read_file", "{\"path\":\"big.rs\"}")],
                String::new(),
            ),
            ChatMessage::tool_result("a".to_string(), format!("big.rs lines 1-900\n{}", "BODYTEXT ".repeat(500))),
            ChatMessage::tool_result("b".to_string(), "error[E0308]: mismatched types in src/lib.rs:42".to_string()),
        ];
        let transcript = super::build_compaction_transcript(&region);
        assert!(transcript.contains("-> call read_file"));
        assert!(transcript.contains("ok: big.rs lines 1-900 [body omitted]"));
        assert!(!transcript.contains("BODYTEXT"));
        assert!(transcript.contains("FAILED RESULT: error[E0308]"));
    }

    #[test]
    fn decode_error_final_fail_counter_increments_when_retry_exhausted() {
        let mut rt = Runtime::new(
            "deepseek".to_string(),
            "deepseek-v4-pro".to_string(),
            "workspace-write".to_string(),
            8,
        );
        rt.client = Box::new(MockProvider::with_streaming(vec![
            Err("error decoding response body".to_string()),
            Err("error decoding response body".to_string()),
            Err("error decoding response body".to_string()),
            Err("error decoding response body".to_string()),
            Err("error decoding response body".to_string()),
        ]));
        rt.native_tool_calling = false;

        let turn = rt.run_turn("ping");

        assert_eq!(turn.stop_reason, "provider_error");
        assert_eq!(rt.provider_decode_final_fail_count(), 1);
    }

    #[test]
    fn provider_decode_stats_line_includes_retry_and_final_fail_counters() {
        let mut rt = Runtime::new(
            "deepseek".to_string(),
            "deepseek-v4-pro".to_string(),
            "workspace-write".to_string(),
            8,
        );
        rt.client = Box::new(MockProvider::with_streaming(vec![
            Err("error decoding response body".to_string()),
            Err("error decoding response body".to_string()),
            Err("error decoding response body".to_string()),
            Err("error decoding response body".to_string()),
            Err("error decoding response body".to_string()),
        ]));
        rt.native_tool_calling = false;
        let _ = rt.run_turn("ping");

        let line = rt.provider_decode_stats_line();
        assert!(line.contains("provider_decode_errors"));
        assert!(line.contains("retries="));
        assert!(line.contains("final_failures=1"));
    }

    #[test]
    fn default_native_tool_calling_disables_deepseek_reasoner_family() {
        assert!(RuntimeType::default_native_tool_calling(
            "deepseek",
            "deepseek-v4-pro"
        ));
        assert!(!RuntimeType::default_native_tool_calling(
            "deepseek",
            "deepseek-reasoner"
        ));
        assert!(RuntimeType::default_native_tool_calling(
            "openai",
            "gpt-5.3-codex"
        ));
    }

    #[test]
    fn native_tool_unsupported_detector_matches_common_signatures() {
        assert!(RuntimeType::is_native_tool_calling_unsupported_error(
            "Unsupported parameter: tools"
        ));
        assert!(RuntimeType::is_native_tool_calling_unsupported_error(
            "function calling is not supported for this model"
        ));
        assert!(!RuntimeType::is_native_tool_calling_unsupported_error(
            "timeout while reading response"
        ));
    }

    fn completion(text: &str, stop: &str) -> Result<CompletionResult, String> {
        Ok(CompletionResult {
            text: text.to_string(),
            tool_calls: Vec::new(),
            stop_reason: stop.to_string(),
            reasoning_content: String::new(),
        })
    }

    fn native_deepseek_runtime(responses: Vec<Result<CompletionResult, String>>) -> Runtime {
        let mut rt = Runtime::new(
            "deepseek".to_string(),
            "deepseek-v4-pro".to_string(),
            "workspace-write".to_string(),
            50,
        );
        rt.native_tool_calling = true;
        rt.client = Box::new(MockProvider::with_tools_responses(responses));
        rt
    }

    fn assert_no_back_to_back_user_messages(messages: &[ChatMessage]) {
        for pair in messages.windows(2) {
            assert!(
                !(pair[0].role == "user" && pair[1].role == "user"),
                "history has consecutive user messages: {:?}",
                messages.iter().map(|m| m.role.as_str()).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn native_empty_length_reply_explains_and_keeps_history_valid() {
        // Reproduces the observed failure: reasoning consumed the whole
        // budget, so content is empty and finish_reason=length, twice.
        let mut rt = native_deepseek_runtime(vec![completion("", "length"), completion("", "length")]);
        let first = rt.run_turn("Which bugs did you find in this project?");
        assert!(first.text.contains("output budget"), "{}", first.text);
        let second = rt.run_turn("?");
        assert!(second.text.contains("output budget"), "{}", second.text);
        assert_no_back_to_back_user_messages(&rt.messages);
    }

    #[test]
    fn native_truncated_text_reply_is_continued_and_stitched() {
        let mut rt = native_deepseek_runtime(vec![
            completion("Bug 1: off-by-one. Bug 2: ", "length"),
            completion("missing None check.", "stop"),
        ]);
        let turn = rt.run_turn("List the bugs.");
        assert_eq!(turn.text, "Bug 1: off-by-one. Bug 2: missing None check.");
        let roles: Vec<_> = rt.messages.iter().skip(1).map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant"], "continuation scaffolding must not stay in history");
        assert!(!rt.messages.iter().any(|m| m.content.contains("Continue from exactly")));
    }

    #[test]
    fn deepseek_thinking_models_get_room_for_reasoning() {
        let caps = RuntimeType::capabilities_for("deepseek", "deepseek-v4-pro");
        assert!(caps.recommended_max_output_tokens >= 32_768);
    }

    #[test]
    fn user_level_hooks_load_without_env_and_env_can_disable() {
        let dir = std::env::temp_dir().join(format!(
            "asi_user_hooks_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("hooks.json");
        std::fs::write(
            &path,
            r#"{"handlers":[{"owner":"asi-skills","event":"PreToolUse","script":"guard","json_protocol":true}]}"#,
        )
        .unwrap();
        let _p = EnvGuard::set("ASI_USER_HOOKS_PATH", &path.to_string_lossy());
        std::env::remove_var("ASI_HOOKS_ENABLED");
        std::env::remove_var("ASI_HOOK_CONFIG_PATH");
        let cfg = Runtime::load_hook_config();
        assert_eq!(cfg.handlers.len(), 1, "user hooks must load with no env setup");
        assert_eq!(cfg.handlers[0].script, "guard");
        {
            let _off = EnvGuard::set("ASI_HOOKS_ENABLED", "0");
            assert!(Runtime::load_hook_config().handlers.is_empty());
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deny_reason_blocks_secret_files_by_default() {
        let ws = std::env::temp_dir();
        let reason = super::deny_reason(
            "workspace-write",
            "read_file",
            ".env",
            false,
            false,
            false,
            &[],
            &[],
            false,
            &ws,
            &[],
            &crate::sandbox::SandboxStrategy::from_env(),
        );
        assert!(reason.unwrap_or_default().contains("credential protection"));
    }

    #[test]
    fn capabilities_for_provider_model_matches_expected_defaults() {
        let deepseek = RuntimeType::capabilities_for("deepseek", "deepseek-v4-pro");
        assert!(deepseek.native_tool_calling_default);
        assert!(deepseek.native_tool_calling_supported);
        assert_eq!(deepseek.recommended_max_output_tokens, 32_768);
        let deepseek_chat = RuntimeType::capabilities_for("deepseek", "deepseek-chat");
        assert_eq!(deepseek_chat.recommended_max_output_tokens, 4096);

        let openai = RuntimeType::capabilities_for("openai", "gpt-5.3-codex");
        assert!(openai.native_tool_calling_default);
        assert!(openai.native_tool_calling_supported);
        assert_eq!(openai.recommended_max_output_tokens, 16_384);
    }

    #[test]
    fn status_line_reports_runtime_capabilities() {
        let rt = Runtime::new(
            "deepseek".to_string(),
            "deepseek-v4-pro".to_string(),
            "workspace-write".to_string(),
            8,
        );
        let line = rt.status_provider_runtime_line();
        assert!(line.contains("native=true"));
        assert!(line.contains("supported=true"));
        assert!(line.contains("recommended_max_output_tokens=32768"));
    }

    #[test]
    fn auto_compact_recommendation_by_turns_fires_above_ratio() {
        let mut rt = Runtime::new(
            "deepseek".to_string(),
            "deepseek-v4-pro".to_string(),
            "workspace-write".to_string(),
            10,
        );
        for i in 0..8usize {
            rt.messages
                .push(ChatMessage::text("user".to_string(), format!("msg {}", i)));
        }
        // 8 user turns of 10 = 80%, threshold at 0.80 -> ceil(8.0) = 8 -> fires
        assert!(rt.auto_compact_recommendation_by_turns(0.80).is_some());
        // ratio 0.95 -> threshold ceil(9.5)=10, 8 < 10 -> no fire
        assert!(rt.auto_compact_recommendation_by_turns(0.95).is_none());
        // ratio 0 / >1 -> disabled
        assert!(rt.auto_compact_recommendation_by_turns(0.0).is_none());
        assert!(rt.auto_compact_recommendation_by_turns(1.5).is_none());
    }

    #[test]
    fn auto_compact_recommendation_by_turns_disabled_when_max_turns_zero() {
        let mut rt = Runtime::new(
            "deepseek".to_string(),
            "deepseek-v4-pro".to_string(),
            "workspace-write".to_string(),
            0,
        );
        rt.messages
            .push(ChatMessage::text("user".to_string(), "x".to_string()));
        assert!(rt.auto_compact_recommendation_by_turns(0.5).is_none());
    }

    #[test]
    fn classify_provider_error_detects_auth_and_quota() {
        assert_eq!(
            RuntimeType::classify_provider_error("HTTP error: 401 unauthorized"),
            ProviderErrorKind::Auth
        );
        assert_eq!(
            RuntimeType::classify_provider_error("Invalid Token"),
            ProviderErrorKind::Auth
        );
        assert_eq!(
            RuntimeType::classify_provider_error("insufficient_quota for this key"),
            ProviderErrorKind::Quota
        );
    }

    #[test]
    fn stop_reason_alias_maps_common_variants() {
        assert_eq!(RuntimeType::stop_reason_alias("end_turn"), "completed");
        assert_eq!(RuntimeType::stop_reason_alias("stop"), "completed");
        assert_eq!(RuntimeType::stop_reason_alias("function_call"), "tool_use");
        assert_eq!(RuntimeType::stop_reason_alias("max_tokens"), "length");
    }

    #[test]
    fn parse_hook_json_outcome_reads_allow_and_reason() {
        let parsed =
            parse_hook_json_outcome(r#"{"allow":true,"reason":"ok"}"#).expect("valid hook json");
        assert!(parsed.allow);
        assert_eq!(parsed.reason, "ok");
        assert!(!parsed.is_error);
    }

    #[test]
    fn parse_hook_json_outcome_defaults_reason_when_missing() {
        let parsed =
            parse_hook_json_outcome(r#"{"allow":false}"#).expect("valid hook json without reason");
        assert!(!parsed.allow);
        assert_eq!(parsed.reason, "denied");
        assert!(parsed.is_error);
    }

    #[test]
    fn parse_hook_json_outcome_rejects_invalid_payload() {
        assert!(parse_hook_json_outcome(r#"{"reason":"ok"}"#).is_none());
        assert!(parse_hook_json_outcome("not-json").is_none());
    }

    #[test]
    fn parse_hook_failure_policy_accepts_known_values() {
        assert_eq!(
            parse_hook_failure_policy("fail-open"),
            Some(HookFailurePolicy::FailOpen)
        );
        assert_eq!(
            parse_hook_failure_policy("open"),
            Some(HookFailurePolicy::FailOpen)
        );
        assert_eq!(
            parse_hook_failure_policy("fail-closed"),
            Some(HookFailurePolicy::FailClosed)
        );
        assert_eq!(
            parse_hook_failure_policy("closed"),
            Some(HookFailurePolicy::FailClosed)
        );
        assert_eq!(parse_hook_failure_policy("unknown"), None);
    }

    #[test]
    fn apply_hook_failure_policy_fail_open_allows_runtime_errors() {
        let outcome = super::HookOutcome {
            allow: false,
            reason: "hook timeout after 10s".to_string(),
            is_error: true,
        };
        let final_outcome =
            apply_hook_failure_policy(outcome, HookFailurePolicy::FailOpen, "config-hook");
        assert!(final_outcome.allow);
        assert!(final_outcome.reason.contains("fail-open"));
    }

    #[test]
    fn apply_hook_failure_policy_keeps_explicit_denies_blocking() {
        let outcome = super::HookOutcome {
            allow: false,
            reason: "denied by policy".to_string(),
            is_error: false,
        };
        let final_outcome =
            apply_hook_failure_policy(outcome, HookFailurePolicy::FailOpen, "config-hook");
        assert!(!final_outcome.allow);
        assert_eq!(final_outcome.reason, "denied by policy");
    }

    #[test]
    fn hook_event_match_supports_case_insensitive_and_wildcard() {
        assert!(hook_event_matches("PreToolUse", "pretooluse"));
        assert!(hook_event_matches("SessionStart", "sessionstart"));
        assert!(hook_event_matches("*", "PermissionRequest"));
        assert!(!hook_event_matches("PostToolUse", "PermissionRequest"));
    }

    #[test]
    fn hook_handler_match_respects_event_prefix_and_permission_mode() {
        let handler = HookHandlerConfig {
            event: "SessionStart".to_string(),
            script: "dummy.ps1".to_string(),
            timeout_secs: None,
            json_protocol: None,
            tool_prefix: Some("runtime".to_string()),
            permission_mode: Some("workspace-write".to_string()),
            failure_policy: None,
        };
        assert!(hook_handler_matches(
            &handler,
            "SessionStart",
            "runtime",
            "workspace-write"
        ));
        assert!(!hook_handler_matches(
            &handler,
            "SessionStart",
            "read_file",
            "workspace-write"
        ));
        assert!(!hook_handler_matches(
            &handler,
            "SessionStart",
            "runtime",
            "read-only"
        ));
        assert!(!hook_handler_matches(
            &handler,
            "PostToolUse",
            "runtime",
            "workspace-write"
        ));
    }

    #[test]
    fn plugin_hook_match_respects_tool_prefix_and_permission_mode() {
        let row = PluginHookExecution {
            script: "echo ok".to_string(),
            timeout_secs: Some(5),
            json_protocol: Some(true),
            tool_prefix: Some("bash".to_string()),
            permission_mode: Some("workspace-write".to_string()),
            failure_policy: None,
        };
        assert!(plugin_hook_matches(&row, "bash", "workspace-write"));
        assert!(!plugin_hook_matches(&row, "write_file", "workspace-write"));
        assert!(!plugin_hook_matches(&row, "bash", "on-request"));
    }

    #[test]
    fn build_plugin_event_hooks_returns_empty_without_plugins() {
        let _lock = crate::plugin::plugin_state_env_lock()
            .lock()
            .expect("plugin env lock");
        let dir = make_temp_dir("asi_runtime_plugin_hook_empty");
        let state_path = dir.join("plugins_state.json");
        let _guard = EnvGuard::set(
            "ASI_PLUGIN_STATE_PATH",
            state_path.to_string_lossy().as_ref(),
        );

        let rows = build_plugin_event_hooks("PreToolUse");
        assert!(rows.is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_hook_handlers_from_file_parses_valid_rows_and_skips_invalid() {
        let dir = make_temp_dir("asi_runtime_hook_cfg");
        let path = dir.join("hooks.json");
        write_text(
            &path,
            r#"{
  "handlers": [
    {
      "event": "SessionStart",
      "script": "scripts/pre.ps1",
      "timeout_secs": 12,
      "json_protocol": true,
      "tool_prefix": "runtime",
      "permission_mode": "workspace-write",
      "failure_policy": "fail-open"
    },
    {
      "event": "",
      "script": "scripts/invalid.ps1"
    },
    {
      "event": "PostToolUse",
      "script": "scripts/post.ps1"
    }
  ]
}"#,
        );

        let handlers =
            Runtime::load_hook_handlers_from_file(path.to_str().expect("utf8 path for test"))
                .expect("parse hook handlers");

        assert_eq!(handlers.len(), 2);
        assert_eq!(handlers[0].event, "SessionStart");
        assert_eq!(handlers[0].script, "scripts/pre.ps1");
        assert_eq!(handlers[0].timeout_secs, Some(12));
        assert_eq!(handlers[0].json_protocol, Some(true));
        assert_eq!(handlers[0].tool_prefix.as_deref(), Some("runtime"));
        assert_eq!(
            handlers[0].permission_mode.as_deref(),
            Some("workspace-write")
        );
        assert_eq!(
            handlers[0].failure_policy,
            Some(HookFailurePolicy::FailOpen)
        );
        assert_eq!(handlers[1].event, "PostToolUse");
        assert_eq!(handlers[1].script, "scripts/post.ps1");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn load_hook_handlers_from_file_errors_for_invalid_json() {
        let dir = make_temp_dir("asi_runtime_hook_cfg_invalid");
        let path = dir.join("hooks.json");
        write_text(&path, "{invalid-json");

        let err = Runtime::load_hook_handlers_from_file(path.to_str().expect("utf8 path for test"))
            .err()
            .expect("expected parse error");
        assert!(!err.trim().is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn hook_timeout_error_message_shape_is_stable() {
        let timeout = 10u64;
        let msg = format!("hook timeout after {}s", timeout.clamp(1, 30));
        assert_eq!(msg, "hook timeout after 10s");
    }

    #[test]
    fn build_hook_args_details_structures_stop_and_subagent_stop() {
        let stop = build_hook_args_details("Stop", "runtime", "max_turns_reached");
        assert_eq!(stop.get("kind").and_then(|v| v.as_str()), Some("stop"));
        assert_eq!(
            stop.get("stop_reason").and_then(|v| v.as_str()),
            Some("max_turns_reached")
        );

        let sub = build_hook_args_details("SubagentStop", "subagent", "id=sa-9 status=done");
        assert_eq!(
            sub.get("kind").and_then(|v| v.as_str()),
            Some("subagent_stop")
        );
        assert_eq!(sub.get("tool").and_then(|v| v.as_str()), Some("subagent"));
        assert_eq!(
            sub.get("fields")
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str()),
            Some("sa-9")
        );
        assert_eq!(
            sub.get("fields")
                .and_then(|v| v.get("status"))
                .and_then(|v| v.as_str()),
            Some("done")
        );
    }

    #[test]
    fn build_hook_args_details_structures_compact_and_prompt_submit() {
        let pre = build_hook_args_details("PreCompact", "runtime", "runtime_compact");
        assert_eq!(
            pre.get("kind").and_then(|v| v.as_str()),
            Some("pre_compact")
        );
        assert_eq!(
            pre.get("action").and_then(|v| v.as_str()),
            Some("runtime_compact")
        );

        let prompt = build_hook_args_details("UserPromptSubmit", "runtime", "hello world");
        assert_eq!(
            prompt.get("kind").and_then(|v| v.as_str()),
            Some("user_prompt_submit")
        );
        assert_eq!(
            prompt.get("prompt").and_then(|v| v.as_str()),
            Some("hello world")
        );
        assert_eq!(prompt.get("prompt_len").and_then(|v| v.as_u64()), Some(11));
    }

    #[test]
    fn collect_hook_diagnostics_returns_empty_when_hooks_disabled() {
        let old = std::env::var("ASI_HOOKS_ENABLED").ok();
        std::env::set_var("ASI_HOOKS_ENABLED", "false");
        let rows =
            super::collect_hook_diagnostics("SessionStart", "runtime", "probe", "on-request");
        assert!(rows.is_empty());
        if let Some(v) = old {
            std::env::set_var("ASI_HOOKS_ENABLED", v);
        } else {
            std::env::remove_var("ASI_HOOKS_ENABLED");
        }
    }
}
