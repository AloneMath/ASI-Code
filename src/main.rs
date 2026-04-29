mod app;
mod agentd;
mod audit;
mod autoresearch;
mod bench;
mod config;
mod cost;
mod cron;
#[allow(dead_code)]
mod error;
mod git_tools;
mod gateway;
mod json_toolcall;
mod markdown;
mod mcp;
mod memory;
mod meta;
mod notebook;
mod oauth;
mod plugin;
mod orchestrator;
mod permissions;
mod policy;
mod provider;
mod research;
mod runtime;
mod sandbox;
mod security;
mod self_update;
mod session;
mod skills;
mod telemetry;
mod todo;
mod tokenizer;
mod tools;
mod ui;
mod voice;
mod wiki;
mod worktree;

use clap::{Parser, Subcommand, ValueEnum};
use config::{
    api_env_status, apply_api_key_env, default_model, is_model_compatible_with_provider,
    normalize_execution_speed, normalize_provider_name, reconcile_model_for_provider,
    resolve_model_alias, AppConfig,
};
use glob::glob;
use meta::APP_NAME;
pub(crate) use orchestrator::confidence::{
    build_confidence_gate_prompt, confidence_low_block_reason, parse_confidence_declaration,
    toolcall_count_by_risk, ConfidenceGateStats,
    ConfidenceLevel, CONFIDENCE_GATE_MAX_RETRIES,
};
use orchestrator::engine::OrchestratorEngine;
use provider::ChatMessage;
use runtime::Runtime;
use session::SessionStore;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tools::tool_index;
use ui::Ui;

const AGENT_JSON_SCHEMA_VERSION: &str = "1";
const MCP_JSON_SCHEMA_VERSION: &str = "1";
const HOOKS_JSON_SCHEMA_VERSION: &str = "1";
const REVIEW_JSON_SCHEMA_VERSION: &str = "1";
const HELP_JSON_SCHEMA_VERSION: &str = "1";
const SAFE_PROFILE_PERMISSION_MODE: &str = "on-request";
const SAFE_PROFILE_SANDBOX_MODE: &str = "local";
const FAST_PROFILE_PERMISSION_MODE: &str = "workspace-write";
const FAST_PROFILE_SANDBOX_MODE: &str = "disabled";

#[derive(Debug, Parser)]
#[command(name = "asi", about = "ASI Code terminal agent")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Gateway {
        #[arg(long, default_value = "127.0.0.1:8787")]
        listen: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        permission_mode: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    Repl {
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        permission_mode: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value_t = false)]
        no_setup: bool,
        #[arg(long, default_value_t = false)]
        voice: bool,
        #[arg(long, value_enum, default_value_t = PromptProfile::Standard)]
        profile: PromptProfile,
        #[arg(long, value_enum)]
        speed: Option<ExecutionSpeed>,
    },
    Prompt {
        text: Option<String>,
        #[arg(long, default_value_t = false)]
        stdin: bool,
        #[arg(long)]
        text_file: Option<PathBuf>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        permission_mode: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value_t = false)]
        agent: bool,
        #[arg(long, default_value_t = false)]
        secure: bool,
        #[arg(long, default_value_t = 50, help = "0 means unlimited")]
        agent_max_steps: usize,
        #[arg(long, value_enum, default_value_t = PromptProfile::Standard)]
        profile: PromptProfile,
        #[arg(long, value_enum)]
        speed: Option<ExecutionSpeed>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        output_format: OutputFormat,
        #[arg(long = "prompt-auto-tools", help = "Enable or disable prompt-mode auto tool continuation: on|off")]
        prompt_auto_tools: Option<String>,
    },
    Config {
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        permission_mode: Option<String>,
        #[arg(long)]
        auto_review_mode: Option<String>,
        #[arg(long)]
        auto_review_severity_threshold: Option<String>,
        #[arg(long)]
        execution_speed: Option<ExecutionSpeed>,
        #[arg(long)]
        max_turns: Option<usize>,
        #[arg(long)]
        extended_thinking: Option<bool>,
        #[arg(long)]
        markdown_render: Option<bool>,
        #[arg(long)]
        theme: Option<String>,
        #[arg(long)]
        telemetry_enabled: Option<bool>,
        #[arg(long)]
        telemetry_log_tool_details: Option<bool>,
        #[arg(long)]
        undercover_mode: Option<bool>,
        #[arg(long)]
        safe_shell_mode: Option<bool>,
        #[arg(long)]
        remote_policy_enabled: Option<bool>,
        #[arg(long)]
        remote_policy_url: Option<String>,
        #[arg(long)]
        disable_web_tools: Option<bool>,
        #[arg(long)]
        disable_bash_tool: Option<bool>,
        #[arg(long)]
        disable_subagent: Option<bool>,
        #[arg(long)]
        disable_research: Option<bool>,
        #[arg(long = "allow-tool-rule")]
        allow_tool_rule: Vec<String>,
        #[arg(long = "deny-tool-rule")]
        deny_tool_rule: Vec<String>,
        #[arg(long)]
        clear_tool_rules: bool,
        #[arg(long)]
        path_restriction_enabled: Option<bool>,
        #[arg(long = "additional-dir")]
        additional_dir: Vec<String>,
        #[arg(long)]
        clear_additional_dirs: bool,
    },
    ApiPage,
    Setup,
    Mcp {
        #[command(subcommand)]
        action: McpCliCommand,
    },
    Plugin {
        #[command(subcommand)]
        action: PluginCliCommand,
    },
    Hooks {
        #[command(subcommand)]
        action: HooksCliCommand,
    },
    Theme {
        #[arg(long)]
        select: Option<usize>,
    },
    Scan {
        #[arg(long)]
        deep: bool,
        #[arg(long = "deep-limit")]
        deep_limit: Option<usize>,
        patterns: Vec<String>,
    },
    Login {
        #[arg(long, default_value = "claude")]
        provider: String,
        #[arg(long)]
        token: Option<String>,
    },
    Logout {
        #[arg(long, default_value = "claude")]
        provider: String,
    },
    Research {
        #[arg(long)]
        topic: String,
        #[arg(long, default_value_t = 3)]
        rounds: usize,
    },
    Sessions {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = false)]
        agent_enabled: bool,
        #[arg(long, default_value_t = false)]
        blocked_only: bool,
    },
    Resume {
        session_id: String,
    },
    Daemon {
        #[command(subcommand)]
        action: DaemonCommand,
    },
    Job {
        #[command(subcommand)]
        action: JobCommand,
    },
    Autoresearch {
        #[command(subcommand)]
        action: AutoResearchCommand,
    },
    Tokenizer {
        #[command(subcommand)]
        action: TokenizerCommand,
    },
    Wiki {
        #[command(subcommand)]
        action: WikiCommand,
    },
    SelfUpdate {
        #[arg(long)]
        source: String,
        #[arg(long)]
        sha256: Option<String>,
        #[arg(long, default_value_t = false)]
        restart: bool,
    },
    Bench {
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        permission_mode: Option<String>,
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value_t = 8)]
        agent_max_steps: usize,
        #[arg(long, default_value_t = 1)]
        repeat: usize,
        #[arg(long, value_enum, default_value_t = BenchSuite::Core)]
        suite: BenchSuite,
        #[arg(long, default_value = "bench_reports")]
        out_dir: PathBuf,
    },
    Version,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
    Jsonl,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum PromptProfile {
    Standard,
    Strict,
}

impl PromptProfile {
    fn as_str(self) -> &'static str {
        match self {
            PromptProfile::Standard => "standard",
            PromptProfile::Strict => "strict",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
enum ExecutionSpeed {
    Sprint,
    Deep,
}

impl ExecutionSpeed {
    fn as_str(self) -> &'static str {
        match self {
            ExecutionSpeed::Sprint => "sprint",
            ExecutionSpeed::Deep => "deep",
        }
    }
}

impl Default for ExecutionSpeed {
    fn default() -> Self {
        ExecutionSpeed::Deep
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BenchSuite {
    Core,
    Gpt53Proxy6d,
}

impl BenchSuite {
    fn as_str(self) -> &'static str {
        match self {
            BenchSuite::Core => "core",
            BenchSuite::Gpt53Proxy6d => "gpt53-proxy-6d",
        }
    }
}

#[derive(Debug)]
struct ScopedEnvVar {
    key: &'static str,
    prev: Option<String>,
}

impl ScopedEnvVar {
    fn set_default_if_unset(key: &'static str, value: &str) -> Option<Self> {
        if std::env::var(key).is_ok() {
            return None;
        }
        std::env::set_var(key, value);
        Some(Self { key, prev: None })
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        match &self.prev {
            Some(prev) => std::env::set_var(self.key, prev),
            None => std::env::remove_var(self.key),
        }
    }
}

#[derive(Debug, Clone)]
struct SubagentTask {
    id: String,
    provider: String,
    model: String,
    permission_mode: String,
    allow_rules: Vec<String>,
    deny_rules: Vec<String>,
    rules_source: String,
    profile_name: Option<String>,
    default_skills: Vec<String>,
    background: bool,
    task: String,
    started_at_ms: u128,
}

#[derive(Debug, Clone)]
struct SubagentOutcome {
    task: SubagentTask,
    finished_at_ms: u128,
    result: Result<String, String>,
}

#[derive(Debug, Clone)]
struct SubagentEvent {
    at_ms: u128,
    event: String,
    message: String,
}

#[derive(Debug, Clone)]
struct SubagentState {
    task: SubagentTask,
    status: String,
    finished_at_ms: Option<u128>,
    output_preview: Option<String>,
    submitted_turns: usize,
    interrupted_count: usize,
    last_interrupted_at_ms: Option<u128>,
    history: Vec<ChatMessage>,
}

#[derive(Debug, Clone)]
struct SubagentRuntimeSnapshot {
    extended_thinking: bool,
    disable_web_tools: bool,
    disable_bash_tool: bool,
    safe_shell_mode: bool,
    permission_allow_rules: Vec<String>,
    permission_deny_rules: Vec<String>,
    path_restriction_enabled: bool,
    additional_directories: Vec<PathBuf>,
    session_permission_allow_rules: Vec<String>,
    session_permission_deny_rules: Vec<String>,
    session_additional_directories: Vec<PathBuf>,
    next_permission_allow_rules: Vec<String>,
    next_additional_directories: Vec<PathBuf>,
    interactive_approval_allow_rules: Vec<String>,
    interactive_approval_deny_rules: Vec<String>,
    native_tool_calling: bool,
}

#[derive(Debug, Clone)]
struct SubagentRunConfig {
    provider: String,
    model: String,
    permission_mode: String,
    rules_source: String,
    profile_name: Option<String>,
    default_skills: Vec<String>,
    max_turns: usize,
    app_cfg: AppConfig,
    runtime_snapshot: SubagentRuntimeSnapshot,
    tool_loop_enabled: bool,
    strict_mode: bool,
    speed: ExecutionSpeed,
    limits: AutoLoopLimits,
    constraints: ToolExecutionConstraints,
}

struct SubagentManager {
    seq: u64,
    active: HashMap<String, mpsc::Receiver<Result<String, String>>>,
    states: HashMap<String, SubagentState>,
    logs: HashMap<String, Vec<SubagentEvent>>,
}

impl SubagentManager {
    fn new() -> Self {
        Self {
            seq: 0,
            active: HashMap::new(),
            states: HashMap::new(),
            logs: HashMap::new(),
        }
    }

    fn push_event(&mut self, id: &str, event: &str, message: &str) {
        let rows = self.logs.entry(id.to_string()).or_default();
        rows.push(SubagentEvent {
            at_ms: now_timestamp_ms(),
            event: event.to_string(),
            message: clip_chars(message, 600),
        });
        if rows.len() > 200 {
            let trim = rows.len().saturating_sub(200);
            rows.drain(0..trim);
        }
    }

    fn read_events(
        &mut self,
        id: &str,
        tail: Option<usize>,
    ) -> Result<(SubagentState, Vec<SubagentEvent>, usize), String> {
        self.reap_finished();
        let state = self
            .states
            .get(id)
            .cloned()
            .ok_or_else(|| format!("unknown subagent id={}", id))?;
        let all = self.logs.get(id).cloned().unwrap_or_default();
        let total = all.len();
        let shown = if let Some(n) = tail {
            let keep = n.max(1);
            all.into_iter().rev().take(keep).collect::<Vec<_>>()
        } else {
            all.into_iter().rev().take(20).collect::<Vec<_>>()
        };
        let mut rows = shown;
        rows.reverse();
        Ok((state, rows, total))
    }

    fn launch_run(
        run_cfg: SubagentRunConfig,
        messages: Vec<ChatMessage>,
    ) -> mpsc::Receiver<Result<String, String>> {
        let (tx, rx) = mpsc::channel::<Result<String, String>>();
        thread::spawn(move || {
            let out = run_subagent_with_messages(&run_cfg, &messages);
            let _ = tx.send(out);
        });
        rx
    }

    fn fresh_history(task: &str) -> Vec<ChatMessage> {
        vec![
            ChatMessage {
                role: "system".to_string(),
                content: subagent_system_prompt().to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: task.to_string(),
            },
        ]
    }

    fn spawn(&mut self, run_cfg: &SubagentRunConfig, task: &str, background: bool) -> String {
        self.seq = self.seq.saturating_add(1);
        let id = format!("sa-{}", self.seq);
        let task_trimmed = task.trim().to_string();
        let (allow_rules, deny_rules) = subagent_effective_rule_summary(run_cfg);
        let task_obj = SubagentTask {
            id: id.clone(),
            provider: run_cfg.provider.clone(),
            model: run_cfg.model.clone(),
            permission_mode: run_cfg.permission_mode.clone(),
            allow_rules,
            deny_rules,
            rules_source: run_cfg.rules_source.clone(),
            profile_name: run_cfg.profile_name.clone(),
            default_skills: run_cfg.default_skills.clone(),
            background,
            task: task_trimmed.clone(),
            started_at_ms: now_timestamp_ms(),
        };

        let history = Self::fresh_history(&task_trimmed);
        let rx = Self::launch_run(run_cfg.clone(), history.clone());
        self.push_event(
            &id,
            "spawn",
            &format!(
                "spawned run_mode={} provider={} model={} task={}",
                if background {
                    "background"
                } else {
                    "foreground"
                },
                run_cfg.provider,
                run_cfg.model,
                clip_chars(&task_trimmed, 120)
            ),
        );

        self.states.insert(
            id.clone(),
            SubagentState {
                task: task_obj,
                status: "running".to_string(),
                finished_at_ms: None,
                output_preview: None,
                submitted_turns: 1,
                interrupted_count: 0,
                last_interrupted_at_ms: None,
                history,
            },
        );
        self.active.insert(id.clone(), rx);
        id
    }

    fn send(
        &mut self,
        run_cfg: &SubagentRunConfig,
        id: &str,
        task: &str,
        interrupt: bool,
        no_context: bool,
    ) -> Result<String, String> {
        self.reap_finished();
        let task_trimmed = task.trim();
        if task_trimmed.is_empty() {
            return Err(
                "Usage: /agent send [--json|--jsonl] [--interrupt] [--no-context] <id> <task>"
                    .to_string(),
            );
        }
        let mut interrupted = false;
        if self.active.contains_key(id) {
            if !interrupt {
                return Err(format!(
                    "subagent id={} is still running; wait or use /agent send --interrupt <id> <task>",
                    id
                ));
            }
            self.active.remove(id);
            interrupted = true;
        }
        let state = self
            .states
            .get_mut(id)
            .ok_or_else(|| format!("unknown subagent id={}", id))?;
        if state.status == "closed" {
            return Err(format!("subagent id={} is closed", id));
        }

        state.task.provider = run_cfg.provider.clone();
        state.task.model = run_cfg.model.clone();
        state.task.permission_mode = run_cfg.permission_mode.clone();
        let (allow_rules, deny_rules) = subagent_effective_rule_summary(run_cfg);
        state.task.allow_rules = allow_rules;
        state.task.deny_rules = deny_rules;
        state.task.rules_source = run_cfg.rules_source.clone();
        state.task.profile_name = run_cfg.profile_name.clone();
        state.task.default_skills = run_cfg.default_skills.clone();
        state.task.task = task_trimmed.to_string();
        state.status = "running".to_string();
        state.finished_at_ms = None;
        state.output_preview = None;
        state.submitted_turns = state.submitted_turns.saturating_add(1);
        if interrupted {
            state.interrupted_count = state.interrupted_count.saturating_add(1);
            state.last_interrupted_at_ms = Some(now_timestamp_ms());
        }
        if no_context {
            state.history = Self::fresh_history(task_trimmed);
        } else {
            state.history.push(ChatMessage {
                role: "user".to_string(),
                content: task_trimmed.to_string(),
            });
        }

        let rx = Self::launch_run(run_cfg.clone(), state.history.clone());
        self.active.insert(id.to_string(), rx);
        let msg = if interrupted {
            format!(
                "subagent id={} interrupted previous run; queued turn={} context={} task={}",
                id,
                state.submitted_turns,
                if no_context { "reset" } else { "append" },
                clip_chars(task_trimmed, 120)
            )
        } else {
            format!(
                "subagent id={} queued turn={} context={} task={}",
                id,
                state.submitted_turns,
                if no_context { "reset" } else { "append" },
                clip_chars(task_trimmed, 120)
            )
        };
        self.push_event(
            id,
            if interrupted { "send_interrupt" } else { "send" },
            &msg,
        );
        Ok(msg)
    }

    fn cancel(&mut self, id: &str) -> Result<String, String> {
        self.reap_finished();
        if self.active.remove(id).is_some() {
            if let Some(state) = self.states.get_mut(id) {
                state.status = "cancelled".to_string();
                state.finished_at_ms = Some(now_timestamp_ms());
                if state.output_preview.is_none() {
                    state.output_preview = Some("cancelled by user".to_string());
                }
            }
            let msg = format!("cancelled subagent id={}", id);
            self.push_event(id, "cancel", &msg);
            return Ok(msg);
        }
        if let Some(state) = self.states.get(id) {
            if state.status == "cancelled" {
                let msg = format!("subagent already cancelled id={}", id);
                self.push_event(id, "cancel_noop", &msg);
                return Ok(msg);
            }
            if state.status == "closed" {
                let msg = format!("subagent id={} is closed", id);
                self.push_event(id, "cancel_noop", &msg);
                return Ok(msg);
            }
            let msg = format!(
                "subagent id={} is not running (status={})",
                id, state.status
            );
            self.push_event(id, "cancel_noop", &msg);
            return Ok(msg);
        }
        Err(format!("unknown subagent id={}", id))
    }

    fn retry(&mut self, run_cfg: &SubagentRunConfig, id: &str) -> Result<String, String> {
        self.reap_finished();
        if self.active.contains_key(id) {
            return Err(format!(
                "subagent id={} is still running; wait or use /agent send --interrupt <id> <task>",
                id
            ));
        }
        let state = self
            .states
            .get_mut(id)
            .ok_or_else(|| format!("unknown subagent id={}", id))?;
        if state.status == "closed" {
            return Err(format!("subagent id={} is closed", id));
        }

        let retry_task = state.task.task.clone();
        state.task.provider = run_cfg.provider.clone();
        state.task.model = run_cfg.model.clone();
        state.task.permission_mode = run_cfg.permission_mode.clone();
        let (allow_rules, deny_rules) = subagent_effective_rule_summary(run_cfg);
        state.task.allow_rules = allow_rules;
        state.task.deny_rules = deny_rules;
        state.task.rules_source = run_cfg.rules_source.clone();
        state.task.profile_name = run_cfg.profile_name.clone();
        state.task.default_skills = run_cfg.default_skills.clone();
        state.status = "running".to_string();
        state.finished_at_ms = None;
        state.output_preview = None;
        state.submitted_turns = state.submitted_turns.saturating_add(1);
        state.history.push(ChatMessage {
            role: "user".to_string(),
            content: format!("Retry previous task: {}", retry_task),
        });
        let rx = Self::launch_run(run_cfg.clone(), state.history.clone());
        self.active.insert(id.to_string(), rx);
        let msg = format!(
            "subagent id={} retried turn={} task={}",
            id,
            state.submitted_turns,
            clip_chars(&retry_task, 120)
        );
        self.push_event(id, "retry", &msg);
        Ok(msg)
    }

    fn has_active(&mut self) -> bool {
        self.reap_finished();
        !self.active.is_empty()
    }

    fn reap_finished(&mut self) {
        let mut finished: Vec<SubagentOutcome> = Vec::new();
        let ids: Vec<String> = self.active.keys().cloned().collect();
        for id in ids {
            let maybe_result = if let Some(rx) = self.active.get(&id) {
                match rx.try_recv() {
                    Ok(res) => Some(res),
                    Err(TryRecvError::Disconnected) => {
                        Some(Err("subagent channel disconnected".to_string()))
                    }
                    Err(TryRecvError::Empty) => None,
                }
            } else {
                None
            };
            if let Some(result) = maybe_result {
                self.active.remove(&id);
                if let Some(state) = self.states.get(&id).cloned() {
                    finished.push(SubagentOutcome {
                        task: state.task,
                        finished_at_ms: now_timestamp_ms(),
                        result,
                    });
                }
            }
        }
        for done in finished {
            self.mark_finished(done);
        }
    }

    fn wait_any(&mut self, timeout_secs: u64) -> Option<SubagentOutcome> {
        self.reap_finished();
        if self.active.is_empty() {
            return None;
        }

        let timeout = Duration::from_secs(timeout_secs.max(1));
        let ids: Vec<String> = self.active.keys().cloned().collect();
        for id in ids {
            let recv_result = {
                let Some(rx) = self.active.get(&id) else {
                    continue;
                };
                rx.recv_timeout(timeout)
            };
            match recv_result {
                Ok(result) => {
                    self.active.remove(&id);
                    if let Some(state) = self.states.get(&id).cloned() {
                        let done = SubagentOutcome {
                            task: state.task,
                            finished_at_ms: now_timestamp_ms(),
                            result,
                        };
                        self.mark_finished(done.clone());
                        return Some(done);
                    }
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    self.active.remove(&id);
                    if let Some(state) = self.states.get(&id).cloned() {
                        let done = SubagentOutcome {
                            task: state.task,
                            finished_at_ms: now_timestamp_ms(),
                            result: Err("subagent channel disconnected".to_string()),
                        };
                        self.mark_finished(done.clone());
                        return Some(done);
                    }
                }
            }
        }
        None
    }

    fn wait_id(&mut self, id: &str, timeout_secs: u64) -> Result<Option<SubagentOutcome>, String> {
        self.reap_finished();
        if !self.states.contains_key(id) {
            return Err(format!("unknown subagent id={}", id));
        }
        let Some(rx) = self.active.get(id) else {
            return Ok(None);
        };
        let timeout = Duration::from_secs(timeout_secs.max(1));
        let recv_result = rx.recv_timeout(timeout);
        match recv_result {
            Ok(result) => {
                self.active.remove(id);
                if let Some(state) = self.states.get(id).cloned() {
                    let done = SubagentOutcome {
                        task: state.task,
                        finished_at_ms: now_timestamp_ms(),
                        result,
                    };
                    self.mark_finished(done.clone());
                    Ok(Some(done))
                } else {
                    Ok(None)
                }
            }
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                self.active.remove(id);
                if let Some(state) = self.states.get(id).cloned() {
                    let done = SubagentOutcome {
                        task: state.task,
                        finished_at_ms: now_timestamp_ms(),
                        result: Err("subagent channel disconnected".to_string()),
                    };
                    self.mark_finished(done.clone());
                    Ok(Some(done))
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn close(&mut self, id: &str) -> Result<String, String> {
        self.reap_finished();
        if self.active.remove(id).is_some() {
            if let Some(state) = self.states.get_mut(id) {
                state.status = "closed".to_string();
                state.finished_at_ms = Some(now_timestamp_ms());
                if state.output_preview.is_none() {
                    state.output_preview = Some("closed by user".to_string());
                }
            }
            let msg = format!("closed subagent id={}", id);
            self.push_event(id, "close", &msg);
            return Ok(msg);
        }

        if let Some(state) = self.states.get_mut(id) {
            if state.status == "closed" {
                let msg = format!("subagent already closed id={}", id);
                self.push_event(id, "close_noop", &msg);
                return Ok(msg);
            }
            state.status = "closed".to_string();
            if state.finished_at_ms.is_none() {
                state.finished_at_ms = Some(now_timestamp_ms());
            }
            if state.output_preview.is_none() {
                state.output_preview = Some("closed by user".to_string());
            }
            let msg = format!("closed subagent id={}", id);
            self.push_event(id, "close", &msg);
            return Ok(msg);
        }

        Err(format!("unknown subagent id={}", id))
    }

    fn list(&mut self) -> Vec<SubagentState> {
        self.reap_finished();
        let mut rows: Vec<SubagentState> = self.states.values().cloned().collect();
        rows.sort_by(|a, b| a.task.id.cmp(&b.task.id));
        rows
    }

    fn state_of(&mut self, id: &str) -> Option<SubagentState> {
        self.reap_finished();
        self.states.get(id).cloned()
    }

    fn mark_finished(&mut self, done: SubagentOutcome) {
        let done_id = done.task.id.clone();
        let done_status = if done.result.is_ok() {
            "completed"
        } else {
            "failed"
        };
        let done_msg = match &done.result {
            Ok(text) => format!("run completed: {}", clip_chars(text.trim(), 200)),
            Err(err) => format!("run failed: {}", clip_chars(err.trim(), 200)),
        };
        if let Some(state) = self.states.get_mut(&done.task.id) {
            state.status = if done.result.is_ok() {
                "completed".to_string()
            } else {
                "failed".to_string()
            };
            state.finished_at_ms = Some(done.finished_at_ms);
            state.output_preview = Some(match &done.result {
                Ok(text) => {
                    state.history.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: text.clone(),
                    });
                    clip_chars(text.trim(), 240)
                }
                Err(err) => clip_chars(err.trim(), 240),
            });
        }
        self.push_event(&done_id, done_status, &done_msg);
        let status = if done.result.is_ok() { "done" } else { "error" };
        let hook_args = format!("id={} status={}", done.task.id, status);
        let _ = Runtime::emit_hook_event_with_mode(
            &done.task.permission_mode,
            "SubagentStop",
            "subagent",
            &hook_args,
        );
    }
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    Install,
    Start,
    Run {
        #[arg(long)]
        loops: Option<usize>,
        #[arg(long)]
        interval_ms: Option<u64>,
        #[arg(long, default_value_t = false)]
        no_stop_when_idle: bool,
        #[arg(long, default_value_t = false)]
        once: bool,
    },
    Stop,
    Status {
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Heartbeat,
    Checkpoint,
}

#[derive(Debug, Subcommand)]
enum JobCommand {
    Submit {
        #[arg(long)]
        spec: PathBuf,
    },
    List {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    Show {
        id: String,
    },
    Cancel {
        id: String,
    },
    Retry {
        id: String,
    },
}

#[derive(Debug, Subcommand)]
enum AutoResearchCommand {
    Doctor {
        #[arg(long)]
        repo: PathBuf,
    },
    Init {
        #[arg(long)]
        repo: PathBuf,
    },
    Run {
        #[arg(long)]
        repo: PathBuf,
        #[arg(long, default_value_t = 1)]
        iterations: usize,
        #[arg(long, default_value_t = 600)]
        timeout_secs: u64,
        #[arg(long)]
        log: Option<PathBuf>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        status: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum TokenizerCommand {
    Doctor {
        #[arg(long)]
        repo: PathBuf,
    },
    Build {
        #[arg(long)]
        repo: PathBuf,
        #[arg(long, default_value_t = false)]
        debug: bool,
        #[arg(long, default_value_t = 900)]
        timeout_secs: u64,
    },
    Train {
        #[arg(long)]
        repo: PathBuf,
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value_t = 4096)]
        vocab_size: u32,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        pattern: Option<String>,
        #[arg(long, default_value = "rustbpe_custom")]
        name: String,
        #[arg(long)]
        python_cmd: Option<String>,
        #[arg(long, default_value_t = false)]
        auto_build: bool,
        #[arg(long, default_value_t = 1800)]
        timeout_secs: u64,
    },
}

#[derive(Debug, Subcommand)]
enum WikiCommand {
    Init {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    Ingest {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, default_value_t = false)]
        no_copy: bool,
    },
    Query {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        question: String,
        #[arg(long, default_value_t = 5)]
        top: usize,
        #[arg(long, default_value_t = false)]
        save: bool,
    },
    Lint {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value_t = false)]
        write_report: bool,
    },
}
fn main() {
    if let Err(e) = run() {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}
fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let command = match cli.command {
        Some(c) => c,
        None => return run_default_terminal_app(),
    };
    app::commands::dispatch(command)
}

fn handle_tokenizer_command(action: TokenizerCommand) -> Result<(), String> {
    let msg = match action {
        TokenizerCommand::Doctor { repo } => tokenizer::doctor(&repo)?,
        TokenizerCommand::Build {
            repo,
            debug,
            timeout_secs,
        } => tokenizer::build_repo(&repo, !debug, timeout_secs)?,
        TokenizerCommand::Train {
            repo,
            input,
            vocab_size,
            output,
            pattern,
            name,
            python_cmd,
            auto_build,
            timeout_secs,
        } => tokenizer::train_from_file(tokenizer::TrainOptions {
            repo,
            input,
            vocab_size,
            output,
            pattern,
            name,
            python_cmd,
            auto_build,
            timeout_secs,
        })?,
    };
    println!("{}", msg);
    Ok(())
}
fn handle_autoresearch_command(action: AutoResearchCommand) -> Result<(), String> {
    let msg = match action {
        AutoResearchCommand::Doctor { repo } => autoresearch::doctor(&repo)?,
        AutoResearchCommand::Init { repo } => autoresearch::init_repo(&repo)?,
        AutoResearchCommand::Run {
            repo,
            iterations,
            timeout_secs,
            log,
            description,
            status,
        } => autoresearch::run_experiments(autoresearch::RunOptions {
            repo,
            iterations,
            timeout_secs,
            log_path: log,
            description,
            status,
        })?,
    };
    println!("{}", msg);
    Ok(())
}

fn handle_wiki_command(action: WikiCommand) -> Result<(), String> {
    let msg = match action {
        WikiCommand::Init { root } => wiki::init(wiki::InitOptions { root })?,
        WikiCommand::Ingest {
            root,
            source,
            title,
            no_copy,
        } => wiki::ingest(wiki::IngestOptions {
            root,
            source,
            title,
            no_copy,
        })?,
        WikiCommand::Query {
            root,
            question,
            top,
            save,
        } => wiki::query(wiki::QueryOptions {
            root,
            question,
            top,
            save,
        })?,
        WikiCommand::Lint { root, write_report } => {
            wiki::lint(wiki::LintOptions { root, write_report })?
        }
    };
    println!("{}", msg);
    Ok(())
}
fn handle_daemon_command(action: DaemonCommand) -> Result<(), String> {
    let msg = match action {
        DaemonCommand::Install => agentd::daemon_install()?,
        DaemonCommand::Start => agentd::daemon_start()?,
        DaemonCommand::Run {
            loops,
            interval_ms,
            no_stop_when_idle,
            once,
        } => agentd::daemon_run(loops, interval_ms, no_stop_when_idle, once)?,
        DaemonCommand::Stop => agentd::daemon_stop()?,
        DaemonCommand::Status { json } => {
            if json {
                agentd::daemon_status_json()?
            } else {
                agentd::daemon_status()?
            }
        }
        DaemonCommand::Heartbeat => agentd::daemon_heartbeat()?,
        DaemonCommand::Checkpoint => agentd::daemon_checkpoint()?,
    };
    println!("{}", msg);
    Ok(())
}

fn handle_job_command(action: JobCommand) -> Result<(), String> {
    match action {
        JobCommand::Submit { spec } => {
            let record = agentd::submit_job_from_spec_file(&spec)?;
            println!(
                "queued id={} state={} project={} goal={}",
                record.id,
                record.state.as_str(),
                record.spec.project_path,
                record.spec.goal
            );
            Ok(())
        }
        JobCommand::List { limit } => {
            let jobs = agentd::list_jobs(limit)?;
            if jobs.is_empty() {
                println!("no jobs");
                return Ok(());
            }
            for j in jobs {
                println!(
                    "{}\t{}\tattempts={}\tupdated={}\tgoal={}",
                    j.id,
                    j.state.as_str(),
                    j.attempts,
                    j.updated_at,
                    j.spec.goal
                );
            }
            Ok(())
        }
        JobCommand::Show { id } => {
            let job = agentd::get_job(&id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&job).map_err(|e| e.to_string())?
            );
            Ok(())
        }
        JobCommand::Cancel { id } => {
            let job = agentd::cancel_job(&id)?;
            println!("canceled id={} state={}", job.id, job.state.as_str());
            Ok(())
        }
        JobCommand::Retry { id } => {
            let job = agentd::retry_job(&id)?;
            println!(
                "retried id={} state={} attempts={}",
                job.id,
                job.state.as_str(),
                job.attempts
            );
            Ok(())
        }
    }
}
fn normalize_prompt_text(mut content: String) -> String {
    if let Some(stripped) = content.strip_prefix('\u{feff}') {
        content = stripped.to_string();
    }
    content
}

fn resolve_prompt_text(
    text: Option<String>,
    read_stdin: bool,
    text_file: Option<PathBuf>,
) -> Result<String, String> {
    let sources = text.is_some() as u8 + read_stdin as u8 + text_file.is_some() as u8;
    if sources == 0 {
        return Err(
            "missing prompt text: provide positional <text>, or --stdin, or --text-file <path>"
                .to_string(),
        );
    }
    if sources > 1 {
        return Err(
            "conflicting prompt text sources: use only one of <text>, --stdin, --text-file"
                .to_string(),
        );
    }

    if let Some(path) = text_file {
        let content = fs::read_to_string(&path)
            .map_err(|e| format!("failed to read text file {}: {}", path.display(), e))?;
        let content = normalize_prompt_text(content);
        if content.trim().is_empty() {
            return Err(format!("text file is empty: {}", path.display()));
        }
        return Ok(content);
    }

    if read_stdin {
        let mut content = String::new();
        io::stdin()
            .read_to_string(&mut content)
            .map_err(|e| format!("failed to read --stdin: {}", e))?;
        let content = normalize_prompt_text(content);
        if content.trim().is_empty() {
            return Err("stdin is empty".to_string());
        }
        return Ok(content);
    }

    match text {
        Some(content) => {
            let content = normalize_prompt_text(content);
            if content.trim().is_empty() {
                Err("prompt text is empty".to_string())
            } else {
                Ok(content)
            }
        }
        None => Err("missing prompt text".to_string()),
    }
}

fn looks_like_model_name(raw: &str) -> bool {
    let v = raw.trim().to_ascii_lowercase();
    v.starts_with("deepseek-")
        || v.starts_with("claude-")
        || v.starts_with("gpt-")
        || v.starts_with("o1")
        || v.starts_with("o3")
        || v.starts_with("o4")
}

pub(crate) fn resolve_cfg(
    provider: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
) -> AppConfig {
    let mut cfg = AppConfig::load();
    cfg.provider = normalize_provider_name(&cfg.provider);

    let mut provider_changed = false;
    let mut hinted_model_from_provider: Option<String> = None;
    if let Some(p) = provider {
        let provider_input = p.trim().to_string();
        let normalized = normalize_provider_name(&provider_input);
        provider_changed = normalized != cfg.provider;
        cfg.provider = normalized;

        if model.is_none() && looks_like_model_name(&provider_input) {
            let hinted = resolve_model_alias(&provider_input);
            if is_model_compatible_with_provider(&cfg.provider, &hinted) {
                hinted_model_from_provider = Some(hinted);
            }
        }
    }

    if let Some(m) = model {
        cfg.model = resolve_model_alias(&m);
    } else if let Some(hinted) = hinted_model_from_provider {
        cfg.model = hinted;
    } else if provider_changed {
        cfg.model = resolve_model_alias(default_model(&cfg.provider));
    }

    if let Some(pm) = permission_mode {
        cfg.permission_mode = pm;
    }
    cfg.permission_mode = normalize_permission_mode(&cfg.permission_mode);
    cfg.auto_review_mode = normalize_auto_review_mode(&cfg.auto_review_mode);
    cfg.auto_review_severity_threshold =
        normalize_auto_review_severity_threshold(&cfg.auto_review_severity_threshold);

    if cfg.model.is_empty() {
        cfg.model = resolve_model_alias(default_model(&cfg.provider));
    }

    let requested_model = cfg.model.clone();
    let (reconciled_model, fallback) =
        reconcile_model_for_provider(&cfg.provider, &requested_model);
    if fallback {
        eprintln!(
            "WARN model {} incompatible with provider {}, fallback={}",
            requested_model, cfg.provider, reconciled_model
        );
    }
    cfg.model = reconciled_model;

    apply_api_key_env(&cfg);
    cfg
}

pub(crate) fn resolve_execution_speed(
    speed: Option<ExecutionSpeed>,
    cfg: &AppConfig,
) -> ExecutionSpeed {
    if let Some(v) = speed {
        return v;
    }
    parse_execution_speed(&cfg.execution_speed).unwrap_or(ExecutionSpeed::Deep)
}

fn normalize_permission_mode(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ask" | "on-request" | "on_request" | "interactive" => "on-request".to_string(),
        "workspace-write" | "workspace_write" => "workspace-write".to_string(),
        "danger-full-access" | "danger_full_access" | "bypass-permissions" => {
            "danger-full-access".to_string()
        }
        "read-only" | "read_only" => "read-only".to_string(),
        _ => SAFE_PROFILE_PERMISSION_MODE.to_string(),
    }
}

pub(crate) fn normalize_auto_review_mode(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" | "disabled" | "disable" | "none" => "off".to_string(),
        "warn" | "warning" => "warn".to_string(),
        "block" | "strict" | "enforce" => "block".to_string(),
        _ => "off".to_string(),
    }
}

pub(crate) fn normalize_auto_review_severity_threshold(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "critical" | "crit" | "p0" | "c4" => "critical".to_string(),
        "high" | "p1" | "h3" => "high".to_string(),
        "medium" | "med" | "p2" | "m2" => "medium".to_string(),
        "low" | "p3" | "l1" => "low".to_string(),
        _ => "high".to_string(),
    }
}

fn render_api_page(cfg: &AppConfig) {
    println!("# {} API Support", APP_NAME);
    println!();
    println!("Active provider: {}", cfg.provider);
    println!("Active model: {}", cfg.model);
    println!("Permission mode: {}", cfg.permission_mode);
    println!("Extended thinking: {}", cfg.extended_thinking);
    println!("Markdown render: {}", cfg.markdown_render);
    println!("Theme: {}", cfg.theme);
    println!("Telemetry enabled: {}", cfg.telemetry_enabled);
    println!("Undercover mode: {}", cfg.undercover_mode);
    println!("Safe shell mode: {}", cfg.safe_shell_mode);
    println!("Feature flags: {}", feature_status(cfg));
    println!("Permission rules: {}", permissions_status(cfg));
    println!();
    println!("Supported providers:");
    println!("- OpenAI: /chat/completions (+stream)");
    println!("- DeepSeek: OpenAI-compatible /chat/completions (+stream)");
    println!("- Claude: /v1/messages (+stream), API key or OAuth token");
    println!("- Model aliases: opus / sonnet / haiku");
    println!();
    println!("Environment:");
    for (k, v) in api_env_status() {
        println!("- {}: {}", k, v);
    }
    println!("- OAuth file: {}", oauth::oauth_path().display());
}

fn run_default_terminal_app() -> Result<(), String> {
    println!("ASI Code Terminal App");
    println!("Mode: repl --provider <openai|deepseek|claude> --project <path> --no-setup");
    println!();

    println!("Select provider:");
    println!("1) OpenAI");
    println!("2) DeepSeek");
    println!("3) Claude");

    let provider_input = prompt_input("Provider [1-3|name, Enter deepseek]")?;
    let selected_provider = provider_from_choice(&provider_input, "deepseek");

    let key_env = provider_api_key_env(&selected_provider);
    let has_key = std::env::var(key_env)
        .ok()
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    println!("{}: {}", key_env, if has_key { "set" } else { "not set" });

    let key_input =
        prompt_secret_input(&format!("Input {} (optional, Enter keep current)", key_env))?;
    if !key_input.trim().is_empty() {
        apply_session_api_key(&selected_provider, key_input.trim());
    }

    let default_project = if Path::new("D:\\Code").exists() {
        "D:\\Code".to_string()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .display()
            .to_string()
    };

    let project_input = prompt_input(&format!("Project path (Enter use {})", default_project))?;
    let project = if project_input.trim().is_empty() {
        default_project
    } else {
        trim_cli_path_input(&project_input)
    };

    let cfg = resolve_cfg(Some(selected_provider), None, None);
    run_repl(
        cfg,
        Some(project),
        true,
        false,
        PromptProfile::Standard,
        None,
    )
}

fn trim_cli_path_input(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 {
        let b = trimmed.as_bytes();
        let quoted = (b[0] == b'"' && b[trimmed.len() - 1] == b'"')
            || (b[0] == b'\'' && b[trimmed.len() - 1] == b'\'');
        if quoted {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

fn run_repl(
    mut cfg: AppConfig,
    project: Option<String>,
    no_setup: bool,
    start_voice_mode: bool,
    profile: PromptProfile,
    speed: Option<ExecutionSpeed>,
) -> Result<(), String> {
    let _sandbox_default_guard =
        ScopedEnvVar::set_default_if_unset("ASI_SANDBOX", SAFE_PROFILE_SANDBOX_MODE);
    if let Some(path) = project.as_deref() {
        let switched = set_project_dir(path)?;
        println!("project={}", switched);

        let project_cfg = AppConfig::load();
        cfg.voice_enabled = project_cfg.voice_enabled;
        cfg.voice_engine = project_cfg.voice_engine;
        cfg.voice_openai_voice = project_cfg.voice_openai_voice;
        cfg.voice_timeout_secs = project_cfg.voice_timeout_secs;
        cfg.voice_mute = project_cfg.voice_mute;
        cfg.voice_openai_fallback_local = project_cfg.voice_openai_fallback_local;
        cfg.voice_local_soft_fail = project_cfg.voice_local_soft_fail;
        cfg.voice_ptt = project_cfg.voice_ptt;
        cfg.voice_ptt_trigger = project_cfg.voice_ptt_trigger;
        cfg.voice_ptt_hotkey = project_cfg.voice_ptt_hotkey;
        cfg.execution_speed = project_cfg.execution_speed;
    }
    let mut ui = Ui::new(&cfg.theme);
    let mut rt = Runtime::new(
        cfg.provider.clone(),
        cfg.model.clone(),
        cfg.permission_mode.clone(),
        cfg.max_turns,
    );
    rt.extended_thinking = cfg.extended_thinking;
    apply_runtime_flags_from_cfg(&mut rt, &cfg);

    if no_setup {
        println!(
            "{}",
            ui.info("setup skipped (--no-setup); using current config/env")
        );
    } else {
        let startup_msg = run_startup_provider_wizard(&mut cfg, &mut rt, &mut ui)?;
        println!("{}", ui.info(&startup_msg));
    }
    println!();

    let mut markdown_render = cfg.markdown_render;
    let mut auto_agent_enabled = true;
    let mut auto_work_mode_enabled = false;
    let mut strict_profile_enabled = profile == PromptProfile::Strict;
    let mut execution_speed = resolve_execution_speed(speed, &cfg);
    cfg.execution_speed = normalize_execution_speed(execution_speed.as_str());
    let mut repl_auto_loop_limits = parse_auto_loop_limits_from_env(50);
    let mut previous_auto_loop_stop_reason: Option<String> = None;
    let mut file_synopsis_cache = FileSynopsisCache::default();
    let mut confidence_gate_stats = ConfidenceGateStats::default();
    let mut subagent_manager = SubagentManager::new();
    let mut auto_checkpoint_enabled = parse_bool_env("ASI_AUTO_CHECKPOINT", true);
    let mut auto_checkpoint_error_reported = false;
    let mut voice_mode = start_voice_mode || cfg.voice_enabled;
    let mut voice_timeout_secs = std::env::var("ASI_VOICE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(cfg.voice_timeout_secs)
        .clamp(1, 120);
    let mut voice_engine = std::env::var("ASI_VOICE_ENGINE")
        .ok()
        .and_then(|v| {
            if v.trim().eq_ignore_ascii_case("auto") {
                Some(voice::auto_engine())
            } else {
                voice::parse_engine(&v)
            }
        })
        .or_else(|| voice::parse_engine(&cfg.voice_engine))
        .unwrap_or_else(voice::default_engine);
    let mut voice_openai_voice = std::env::var("ASI_VOICE_OPENAI_VOICE")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            let v = cfg.voice_openai_voice.trim().to_string();
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        })
        .unwrap_or_else(|| "alloy".to_string());
    let mut voice_mute = parse_bool_env("ASI_VOICE_MUTE", cfg.voice_mute);
    let mut voice_ptt = parse_bool_env("ASI_VOICE_PTT", cfg.voice_ptt);
    let mut voice_ptt_trigger = std::env::var("ASI_VOICE_PTT_TRIGGER")
        .ok()
        .map(|v| normalize_ptt_trigger(&v))
        .unwrap_or_else(|| normalize_ptt_trigger(&cfg.voice_ptt_trigger));
    let mut voice_ptt_hotkey = std::env::var("ASI_VOICE_PTT_HOTKEY")
        .ok()
        .and_then(|v| voice::parse_hotkey_name(&v))
        .or_else(|| voice::parse_hotkey_name(&cfg.voice_ptt_hotkey))
        .unwrap_or_else(|| "F8".to_string());
    let mut voice_stats = VoiceRuntimeStats::default();

    if std::env::var("ASI_VOICE_OPENAI_FALLBACK_LOCAL").is_err() {
        std::env::set_var(
            "ASI_VOICE_OPENAI_FALLBACK_LOCAL",
            if cfg.voice_openai_fallback_local {
                "true"
            } else {
                "false"
            },
        );
    }
    if std::env::var("ASI_VOICE_LOCAL_SOFT_FAIL").is_err() {
        std::env::set_var(
            "ASI_VOICE_LOCAL_SOFT_FAIL",
            if cfg.voice_local_soft_fail {
                "true"
            } else {
                "false"
            },
        );
    }
    if std::env::var("ASI_VOICE_PTT").is_err() {
        std::env::set_var(
            "ASI_VOICE_PTT",
            if cfg.voice_ptt { "true" } else { "false" },
        );
    }
    if std::env::var("ASI_VOICE_PTT_TRIGGER").is_err() {
        std::env::set_var("ASI_VOICE_PTT_TRIGGER", &voice_ptt_trigger);
    }
    if std::env::var("ASI_VOICE_PTT_HOTKEY").is_err() {
        std::env::set_var("ASI_VOICE_PTT_HOTKEY", &voice_ptt_hotkey);
    }
    let mut changed_files_session: Vec<String> = Vec::new();
    let mut change_events_session: Vec<String> = Vec::new();
    let mut skill_registry = load_skill_registry_for_repl(&cfg, &ui);
    let mut worktree_session = worktree::WorktreeSession::new();

    let cwd = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .display()
        .to_string();

    // Runtime::new already injects project context into the system prompt.
    // Do not add synthetic user/assistant turns here, otherwise the first
    // real user message can be biased by fake dialogue history.
    if !rt.project_context.is_empty() {
        println!(
            "{}",
            ui.info(&format!(
                "project context loaded ({} chars)",
                rt.project_context.len()
            ))
        );
    }
    println!(
        "{}",
        ui.info("auto-agent default=on (no need to run /auto on for normal coding/chat)")
    );
    println!("{}", ui.info(&format!("sandbox={}", rt.sandbox_name())));
    println!(
        "{}",
        ui.info(&format!(
            "profile={}",
            if strict_profile_enabled {
                "strict"
            } else {
                "standard"
            }
        ))
    );
    println!("{}", ui.info(&format!("speed={}", execution_speed.as_str())));

    println!("{}", ui.welcome_card(&cwd));
    println!();
    println!("{}", ui.tips_section());
    if voice_mode {
        if voice::is_supported() {
            println!(
                "{}",
                ui.info(&format!(
                    "voice_mode=on engine={} openai_voice={} timeout={}s ptt={} trigger={} hotkey={} (blank line {} trigger)",
                    voice::engine_name(voice_engine),
                    voice_openai_voice,
                    voice_timeout_secs,
                    voice_ptt,
                    voice_ptt_trigger,
                    voice_ptt_hotkey,
                    if voice_ptt { "does not" } else { "does" }
                ))
            );
        } else {
            voice_mode = false;
            println!(
                "{}",
                ui.error("voice_mode requested but unsupported on this OS (Windows required)")
            );
        }
    }
    println!();

    let store = SessionStore::default()?;

    loop {
        print!("{}", ui.input_prompt());
        io::stdout().flush().map_err(|e| e.to_string())?;

        let mut raw_line = String::new();
        let n = io::stdin()
            .read_line(&mut raw_line)
            .map_err(|e| e.to_string())?;

        if n == 0 {
            break;
        }
        let mut line_buf = normalize_prompt_text(raw_line.trim().to_string());
        if voice_mode && voice_ptt && line_buf == voice_ptt_trigger {
            println!("{}", ui.info("voice listening..."));
            voice_stats.listen_attempts += 1;
            match voice::recognize_once(voice_timeout_secs) {
                Ok(Some(spoken)) => {
                    line_buf = spoken;
                    voice_stats.listen_ok += 1;
                    log_voice_event_runtime(&rt, "listen_ptt_trigger", true, "recognized");
                    println!("{}", ui.info(&format!("voice_input={}", line_buf)));
                }
                Ok(None) => {
                    voice_stats.listen_empty += 1;
                    log_voice_event_runtime(&rt, "listen_ptt_trigger", true, "empty");
                    println!("{}", ui.info("voice_input=none"));
                    continue;
                }
                Err(e) => {
                    voice_stats.listen_failed += 1;
                    voice_stats.last_error = Some(e.clone());
                    log_voice_event_runtime(&rt, "listen_ptt_trigger", false, &e);
                    println!("{}", ui.error(&format!("voice listen failed: {}", e)));
                    continue;
                }
            }
        }
        if line_buf.is_empty() {
            if voice_mode && !voice_ptt {
                println!("{}", ui.info("voice listening..."));
                voice_stats.listen_attempts += 1;
                match voice::recognize_once(voice_timeout_secs) {
                    Ok(Some(spoken)) => {
                        line_buf = spoken;
                        voice_stats.listen_ok += 1;
                        log_voice_event_runtime(&rt, "listen_blank", true, "recognized");
                        println!("{}", ui.info(&format!("voice_input={}", line_buf)));
                    }
                    Ok(None) => {
                        voice_stats.listen_empty += 1;
                        log_voice_event_runtime(&rt, "listen_blank", true, "empty");
                        println!("{}", ui.info("voice_input=none"));
                        continue;
                    }
                    Err(e) => {
                        voice_stats.listen_failed += 1;
                        voice_stats.last_error = Some(e.clone());
                        log_voice_event_runtime(&rt, "listen_blank", false, &e);
                        println!("{}", ui.error(&format!("voice listen failed: {}", e)));
                        continue;
                    }
                }
            } else {
                continue;
            }
        }

        if line_buf == "/exit" || line_buf == "/quit" {
            break;
        }
        let change_events_before_turn = change_events_session.len();
        let mut line = line_buf.as_str();
        if line == "/help" {
            println!("{}", build_repl_help_text_short());
            continue;
        }
        if let Some(rest) = line.strip_prefix("/help ") {
            let topic = rest.trim();
            let (topic_for_lookup, help_mode) = parse_help_output_mode(topic);
            if topic_for_lookup.trim().is_empty() && matches!(help_mode, HelpOutputMode::Text) {
                println!("{}", build_repl_help_text_short());
                continue;
            }
            let normalized = normalize_help_topic(&topic_for_lookup);
            if matches!(help_mode, HelpOutputMode::Json) {
                let resolved = resolve_help_topic_for_json(&topic_for_lookup);
                println!("{}", build_repl_help_topic_json_text(&resolved));
                continue;
            }
            if matches!(help_mode, HelpOutputMode::Jsonl) {
                let resolved = resolve_help_topic_for_json(&topic_for_lookup);
                println!("{}", build_repl_help_topic_jsonl_text(&resolved));
                continue;
            }
            if matches!(help_mode, HelpOutputMode::Markdown) {
                let resolved = resolve_help_topic_for_text(&topic_for_lookup);
                let rendered_md = match normalize_help_topic(&resolved).as_str() {
                    "short" => Some(render_help_markdown(&build_repl_help_text_short())),
                    "full" => Some(render_help_markdown(&build_repl_help_text())),
                    "topics" | "index" => Some(render_help_markdown(&build_repl_help_topics_text())),
                    key if key.starts_with("search ") => Some(render_help_markdown(
                        &build_repl_help_search_text(key.trim_start_matches("search ")),
                    )),
                    "search" => Some(render_help_markdown(&build_repl_help_search_text(""))),
                    _ => build_repl_help_topic(&resolved).map(|s| render_help_markdown(&s)),
                };
                match rendered_md {
                    Some(text) => println!("{}", text),
                    None => println!(
                        "{}",
                        ui.info(&format!(
                            "warning: unknown help topic: '{}'. Try /help short, /help full, or /help /mcp.",
                            topic
                        ))
                    ),
                }
                continue;
            }
            let rendered = match normalized.as_str() {
                "short" => Some(build_repl_help_text_short()),
                "full" => Some(build_repl_help_text()),
                "topics" | "index" => Some(build_repl_help_topics_text()),
                _ if normalized.starts_with("search ") => {
                    Some(build_repl_help_search_text(normalized.trim_start_matches("search ")))
                }
                "search" => Some(build_repl_help_search_text("")),
                _ => build_repl_help_topic(topic),
            };
            match rendered {
                Some(text) => println!("{}", text),
                None => println!(
                    "{}",
                    ui.info(&format!(
                        "warning: unknown help topic: '{}'. Try /help short, /help full, or /help /mcp.",
                        topic
                    ))
                ),
            }
            continue;
        }

        if line == "/changes" {
            println!(
                "{}",
                ui.info(&format_changed_files(
                    &changed_files_session,
                    &change_events_session
                ))
            );
            continue;
        }
        if let Some(rest) = line.strip_prefix("/changes ") {
            let msg = handle_changes_command(
                rest,
                &mut changed_files_session,
                &mut change_events_session,
            )?;
            println!("{}", ui.info(&msg));
            continue;
        }

        if line == "/theme" {
            println!("{}", ui.theme_menu(&cfg.theme));
            let value = prompt_input("Select style [1-6], Enter to keep")?;
            if !value.trim().is_empty() {
                if let Ok(idx) = value.trim().parse::<usize>() {
                    if let Some(key) = theme_from_index(idx) {
                        cfg.theme = key.to_string();
                        ui.set_theme(&cfg.theme);
                        let _ = cfg.save();
                        println!("{}", ui.info(&format!("theme={}", cfg.theme)));
                    }
                }
            }
            continue;
        }

        if line == "/setup" {
            let msg = run_setup_wizard(&mut cfg, &mut rt, &mut ui)?;
            println!("{}", ui.info(&msg));
            continue;
        }

        if let Some(rest) = line.strip_prefix("/scan") {
            let panel = render_pattern_scan_panel(rest.trim());
            println!("{}", panel);
            continue;
        }

        if line == "/project" {
            let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
            println!("{}", ui.info(&format!("project={}", cwd.display())));
            continue;
        }

        if let Some(path) = line.strip_prefix("/project ") {
            let switched = set_project_dir(path.trim())?;
            apply_runtime_flags_from_cfg(&mut rt, &cfg);
            println!("{}", ui.info(&format!("project={}", switched)));
            continue;
        }

        if let Some(path) = line.strip_prefix("/import ") {
            let switched = set_project_dir(path.trim())?;
            apply_runtime_flags_from_cfg(&mut rt, &cfg);
            println!("{}", ui.info(&format!("project={}", switched)));
            continue;
        }
        if line == "/status" {
            println!(
                "{}",
                ui.status_line(
                    &rt.provider,
                    &rt.model,
                    &rt.permission_mode,
                    rt.turn_count(),
                    rt.cumulative_input_tokens,
                    rt.cumulative_output_tokens,
                    rt.cumulative_cost_usd,
                )
            );
            println!("{}", ui.info(&rt.status_provider_runtime_line()));
            println!("{}", ui.info(&rt.status_provider_error_line()));
            println!("{}", ui.info(&rt.status_stop_reason_line()));
            println!("{}", ui.info(&rt.status_project_context_line()));
            println!("{}", ui.info(&rt.provider_decode_stats_line()));
            println!("{}", ui.info(&privacy_status(&cfg)));
            println!("{}", ui.info(&feature_status(&cfg)));
            println!("{}", ui.info(&permissions_status(&cfg)));
            println!(
                "{}",
                ui.info(&format!(
                    "auto_review mode={} threshold={}",
                    rt.auto_review_mode, rt.auto_review_severity_threshold
                ))
            );
            println!("{}", ui.info(&session_permissions_status(&rt)));
            println!("{}", ui.info(&format!("auto_agent={}", auto_agent_enabled)));
            println!("{}", ui.info(&format!("speed={}", execution_speed.as_str())));
            println!(
                "{}",
                ui.info(&format!("work_mode={}", auto_work_mode_enabled))
            );
            println!(
                "{}",
                ui.info(&format!(
                    "checkpoint_auto={} checkpoint_exists={}",
                    auto_checkpoint_enabled,
                    store.checkpoint_exists()
                ))
            );
            println!(
                "{}",
                ui.info(&format!(
                    "voice_mode={} engine={} openai_voice={} timeout_secs={} mute={} fallback_local={} local_soft_fail={} ptt={} ptt_trigger={} ptt_hotkey={}",
                    voice_mode,
                    voice::engine_name(voice_engine),
                    voice_openai_voice,
                    voice_timeout_secs,
                    voice_mute,
                    voice::openai_fallback_local_enabled(),
                    voice::local_soft_fail_enabled(),
                    voice_ptt,
                voice_ptt_trigger,
                voice_ptt_hotkey
                ))
            );
            println!("{}", ui.info(&format_voice_stats(&voice_stats)));
            let hooks_status = hooks_status_payload();
            let hooks_enabled = hooks_status
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let hooks_diag_count = hooks_status
                .get("diagnostics_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let hooks_diag_denied = hooks_status
                .get("diagnostics_denied")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let hooks_diag_errors = hooks_status
                .get("diagnostics_errors")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let hooks_env_count = hooks_status
                .get("env_event_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let hooks_file_count = hooks_status
                .get("configured_file_handlers")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let hooks_plugin_count = hooks_status
                .get("plugin_hook_estimated")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let hooks_config_path = hooks_status
                .get("config_path")
                .and_then(|v| v.as_str())
                .unwrap_or("<unset>");
            println!(
                "{}",
                ui.info(&format!(
                    "hooks enabled={} diagnostics={} denied={} errors={} source_env={} source_file={} source_plugin={} config_path={}",
                    hooks_enabled,
                    hooks_diag_count,
                    hooks_diag_denied,
                    hooks_diag_errors,
                    hooks_env_count,
                    hooks_file_count,
                    hooks_plugin_count,
                    hooks_config_path
                ))
            );
            if let Some(by_event) = hooks_status
                .get("configured_file_handlers_by_event")
                .and_then(|v| v.as_object())
            {
                if !by_event.is_empty() {
                    let mut pairs = Vec::new();
                    for (k, v) in by_event {
                        pairs.push(format!("{}={}", k, v.as_u64().unwrap_or(0)));
                    }
                    println!(
                        "{}",
                        ui.info(&format!("hooks file_handlers_by_event={}", pairs.join(", ")))
                    );
                }
            }
            if let Some(rows) = hooks_status
                .get("diagnostics")
                .and_then(|v| v.as_array())
            {
                for row in rows.iter().take(5) {
                    println!(
                        "{}",
                        ui.info(&format!(
                            "hook source={} event={} allow={} is_error={} reason={}",
                            row.get("source").and_then(|v| v.as_str()).unwrap_or("-"),
                            row.get("event").and_then(|v| v.as_str()).unwrap_or("-"),
                            row.get("allow").and_then(|v| v.as_bool()).unwrap_or(true),
                            row.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false),
                            row.get("reason").and_then(|v| v.as_str()).unwrap_or("")
                        ))
                    );
                }
            }
            if let Ok(summary) = audit::summarize_recent(50) {
                println!("{}", ui.info(&format_audit_summary(&summary, 50)));
            }
            if let Ok(rows) = audit::summarize_recent_by_tool(50) {
                println!("{}", ui.info(&format_audit_tools_brief(&rows, 50)));
            }
            if let Ok(reasons) = audit::summarize_recent_by_reason(50) {
                println!("{}", ui.info(&format_audit_reasons_brief(&reasons, 50)));
            }
            continue;
        }
        if line == "/voice" || line == "/voice status" {
            let listen_hint = if voice_ptt {
                format!("type '{}' then Enter to trigger listen, or use /voice hold-listen ({}) / hotkey-listen", voice_ptt_trigger, voice_ptt_hotkey)
            } else {
                "blank line triggers listen".to_string()
            };
            println!(
                "{}",
                ui.info(&format!(
                    "voice_mode={} supported={} engine={} openai_voice={} openai_key_present={} timeout_secs={} mute={} fallback_local={} local_soft_fail={} ptt={} ptt_trigger={} ptt_hotkey={} ({})",
                    voice_mode,
                    voice::is_supported(),
                    voice::engine_name(voice_engine),
                    voice_openai_voice,
                    voice::openai_key_present(),
                    voice_timeout_secs,
                    voice_mute,
                    voice::openai_fallback_local_enabled(),
                    voice::local_soft_fail_enabled(),
                    voice_ptt,
                    voice_ptt_trigger,
                    voice_ptt_hotkey,
                    listen_hint,
                ))
            );
            println!("{}", ui.info(&format_voice_stats(&voice_stats)));
            continue;
        }
        if let Some(rest) = line.strip_prefix("/voice ") {
            let arg = rest.trim();
            if arg == "stats" {
                println!("{}", ui.info(&format_voice_stats(&voice_stats)));
                continue;
            }
            if arg == "on" {
                if !voice::is_supported() {
                    println!("{}", ui.error("voice mode is supported on Windows only"));
                } else {
                    voice_mode = true;
                    sync_voice_config_fields(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    );
                    println!("{}", ui.info("voice_mode=true"));
                }
                continue;
            }
            if arg == "off" {
                voice_mode = false;
                sync_voice_config_fields(
                    &mut cfg,
                    voice_mode,
                    voice_engine,
                    &voice_openai_voice,
                    voice_timeout_secs,
                    voice_mute,
                    voice_ptt,
                    &voice_ptt_trigger,
                );
                println!("{}", ui.info("voice_mode=false"));
                continue;
            }
            if arg == "config" || arg == "config show" {
                sync_voice_config_fields(
                    &mut cfg,
                    voice_mode,
                    voice_engine,
                    &voice_openai_voice,
                    voice_timeout_secs,
                    voice_mute,
                    voice_ptt,
                    &voice_ptt_trigger,
                );
                println!(
                    "{}",
                    ui.info(&format!(
                        "voice_config persisted: enabled={} engine={} openai_voice={} timeout_secs={} mute={} fallback_local={} local_soft_fail={} ptt={} ptt_trigger={} ptt_hotkey={}",
                        cfg.voice_enabled,
                        cfg.voice_engine,
                        cfg.voice_openai_voice,
                        cfg.voice_timeout_secs,
                        cfg.voice_mute,
                        cfg.voice_openai_fallback_local,
                        cfg.voice_local_soft_fail,
                        cfg.voice_ptt,
                    cfg.voice_ptt_trigger,
                    cfg.voice_ptt_hotkey,
                    ))
                );
                continue;
            }
            if arg == "config save" {
                let msg = save_voice_config(
                    &mut cfg,
                    voice_mode,
                    voice_engine,
                    &voice_openai_voice,
                    voice_timeout_secs,
                    voice_mute,
                    voice_ptt,
                    &voice_ptt_trigger,
                )?;
                println!("{}", ui.info(&msg));
                continue;
            }
            if let Some(v) = arg.strip_prefix("config fallback-local ") {
                if let Some(flag) = parse_on_off(v) {
                    std::env::set_var(
                        "ASI_VOICE_OPENAI_FALLBACK_LOCAL",
                        if flag { "true" } else { "false" },
                    );
                    let msg = save_voice_config(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    )?;
                    println!(
                        "{}",
                        ui.info(&format!("voice_fallback_local={} {}", flag, msg))
                    );
                } else {
                    println!(
                        "{}",
                        ui.error("Usage: /voice config fallback-local <on|off>")
                    );
                }
                continue;
            }
            if let Some(v) = arg.strip_prefix("config local-soft-fail ") {
                if let Some(flag) = parse_on_off(v) {
                    std::env::set_var(
                        "ASI_VOICE_LOCAL_SOFT_FAIL",
                        if flag { "true" } else { "false" },
                    );
                    let msg = save_voice_config(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    )?;
                    println!(
                        "{}",
                        ui.info(&format!("voice_local_soft_fail={} {}", flag, msg))
                    );
                } else {
                    println!(
                        "{}",
                        ui.error("Usage: /voice config local-soft-fail <on|off>")
                    );
                }
                continue;
            }
            if let Some(v) = arg.strip_prefix("config ptt-trigger ") {
                let token = normalize_ptt_trigger(v);
                if token.is_empty() {
                    println!("{}", ui.error("Usage: /voice config ptt-trigger <token>"));
                } else {
                    voice_ptt_trigger = token;
                    std::env::set_var("ASI_VOICE_PTT_TRIGGER", &voice_ptt_trigger);
                    let msg = save_voice_config(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    )?;
                    println!(
                        "{}",
                        ui.info(&format!("voice_ptt_trigger={} {}", voice_ptt_trigger, msg))
                    );
                }
                continue;
            }
            if let Some(v) = arg.strip_prefix("config ptt-hotkey ") {
                if let Some(key) = voice::parse_hotkey_name(v) {
                    voice_ptt_hotkey = key;
                    std::env::set_var("ASI_VOICE_PTT_HOTKEY", &voice_ptt_hotkey);
                    let msg = save_voice_config(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    )?;
                    println!(
                        "{}",
                        ui.info(&format!("voice_ptt_hotkey={} {}", voice_ptt_hotkey, msg))
                    );
                } else {
                    println!("{}", ui.error("Usage: /voice config ptt-hotkey <key>"));
                }
                continue;
            }
            if let Some(v) = arg.strip_prefix("config ptt ") {
                if let Some(flag) = parse_on_off(v) {
                    voice_ptt = flag;
                    std::env::set_var("ASI_VOICE_PTT", if flag { "true" } else { "false" });
                    let msg = save_voice_config(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    )?;
                    println!("{}", ui.info(&format!("voice_ptt={} {}", flag, msg)));
                } else {
                    println!("{}", ui.error("Usage: /voice config ptt <on|off>"));
                }
                continue;
            }
            if let Some(v) = arg.strip_prefix("ptt-trigger ") {
                let token = normalize_ptt_trigger(v);
                if token.is_empty() {
                    println!("{}", ui.error("Usage: /voice ptt-trigger <token>"));
                } else {
                    voice_ptt_trigger = token;
                    sync_voice_config_fields(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    );
                    println!(
                        "{}",
                        ui.info(&format!("voice_ptt_trigger={}", voice_ptt_trigger))
                    );
                }
                continue;
            }
            if let Some(v) = arg.strip_prefix("ptt-hotkey ") {
                if let Some(key) = voice::parse_hotkey_name(v) {
                    voice_ptt_hotkey = key;
                    std::env::set_var("ASI_VOICE_PTT_HOTKEY", &voice_ptt_hotkey);
                    sync_voice_config_fields(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    );
                    println!(
                        "{}",
                        ui.info(&format!("voice_ptt_hotkey={}", voice_ptt_hotkey))
                    );
                } else {
                    println!("{}", ui.error("Usage: /voice ptt-hotkey <key>"));
                }
                continue;
            }
            if let Some(v) = arg.strip_prefix("ptt ") {
                let raw = v.trim().to_ascii_lowercase();
                if matches!(raw.as_str(), "on" | "true" | "1") {
                    voice_ptt = true;
                    sync_voice_config_fields(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    );
                    println!("{}", ui.info("voice_ptt=true"));
                } else if matches!(raw.as_str(), "off" | "false" | "0") {
                    voice_ptt = false;
                    sync_voice_config_fields(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    );
                    println!("{}", ui.info("voice_ptt=false"));
                } else {
                    println!("{}", ui.error("Usage: /voice ptt <on|off>"));
                }
                continue;
            }
            if let Some(v) = arg.strip_prefix("mute ") {
                let raw = v.trim().to_ascii_lowercase();
                if matches!(raw.as_str(), "on" | "true" | "1") {
                    voice_mute = true;
                    sync_voice_config_fields(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    );
                    println!("{}", ui.info("voice_mute=true"));
                } else if matches!(raw.as_str(), "off" | "false" | "0") {
                    voice_mute = false;
                    sync_voice_config_fields(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    );
                    println!("{}", ui.info("voice_mute=false"));
                } else {
                    println!("{}", ui.error("Usage: /voice mute <on|off>"));
                }
                continue;
            }
            if arg == "hold-listen" || arg.starts_with("hold-listen ") {
                if !voice::is_supported() {
                    println!("{}", ui.error("voice mode is supported on Windows only"));
                    continue;
                }
                if !voice_mode {
                    println!("{}", ui.error("voice_mode=false; run /voice on first"));
                    continue;
                }
                let wait_secs = if let Some(v) = arg.strip_prefix("hold-listen ") {
                    match v.trim().parse::<u64>() {
                        Ok(n) => n.clamp(1, 300),
                        Err(_) => {
                            println!("{}", ui.error("Usage: /voice hold-listen [wait_secs]"));
                            continue;
                        }
                    }
                } else {
                    30
                };
                println!(
                    "{}",
                    ui.info(&format!(
                        "hold {} to speak (wait {}s, max_hold {}s)",
                        voice_ptt_hotkey, wait_secs, voice_timeout_secs
                    ))
                );
                voice_stats.listen_attempts += 1;
                match voice::recognize_while_hotkey_held(
                    &voice_ptt_hotkey,
                    wait_secs,
                    voice_timeout_secs,
                ) {
                    Ok(voice::HotkeyHoldOutcome::WaitTimedOut) => {
                        log_voice_event_runtime(&rt, "listen_hotkey_hold", true, "wait_timeout");
                        println!(
                            "{}",
                            ui.info("voice hold-listen timeout waiting for hotkey")
                        );
                        continue;
                    }
                    Ok(voice::HotkeyHoldOutcome::Captured(Some(spoken))) => {
                        line_buf = spoken;
                        line = line_buf.as_str();
                        voice_stats.listen_ok += 1;
                        log_voice_event_runtime(&rt, "listen_hotkey_hold", true, "recognized");
                        println!("{}", ui.info(&format!("voice_input={}", line_buf)));
                    }
                    Ok(voice::HotkeyHoldOutcome::Captured(None)) => {
                        voice_stats.listen_empty += 1;
                        log_voice_event_runtime(&rt, "listen_hotkey_hold", true, "empty");
                        println!("{}", ui.info("voice_input=none"));
                        continue;
                    }
                    Err(e) => {
                        voice_stats.listen_failed += 1;
                        voice_stats.last_error = Some(e.clone());
                        log_voice_event_runtime(&rt, "listen_hotkey_hold", false, &e);
                        println!("{}", ui.error(&format!("voice hold-listen failed: {}", e)));
                        continue;
                    }
                }
            } else if arg == "hotkey-listen" {
                if !voice::is_supported() {
                    println!("{}", ui.error("voice mode is supported on Windows only"));
                    continue;
                }
                if !voice_mode {
                    println!("{}", ui.error("voice_mode=false; run /voice on first"));
                    continue;
                }
                println!(
                    "{}",
                    ui.info(&format!(
                        "press {} now (Esc cancels, timeout 30s)",
                        voice_ptt_hotkey
                    ))
                );
                match voice::wait_for_hotkey_once(&voice_ptt_hotkey, 30) {
                    Ok(voice::HotkeyWaitOutcome::Triggered) => {
                        println!("{}", ui.info("voice listening..."));
                        voice_stats.listen_attempts += 1;
                        match voice::recognize_once(voice_timeout_secs) {
                            Ok(Some(spoken)) => {
                                line_buf = spoken;
                                line = line_buf.as_str();
                                voice_stats.listen_ok += 1;
                                log_voice_event_runtime(&rt, "listen_hotkey", true, "recognized");
                                println!("{}", ui.info(&format!("voice_input={}", line_buf)));
                            }
                            Ok(None) => {
                                voice_stats.listen_empty += 1;
                                log_voice_event_runtime(&rt, "listen_hotkey", true, "empty");
                                println!("{}", ui.info("voice_input=none"));
                                continue;
                            }
                            Err(e) => {
                                voice_stats.listen_failed += 1;
                                voice_stats.last_error = Some(e.clone());
                                log_voice_event_runtime(&rt, "listen_hotkey", false, &e);
                                println!("{}", ui.error(&format!("voice listen failed: {}", e)));
                                continue;
                            }
                        }
                    }
                    Ok(voice::HotkeyWaitOutcome::Escaped) => {
                        println!("{}", ui.info("voice hotkey canceled"));
                        continue;
                    }
                    Ok(voice::HotkeyWaitOutcome::TimedOut) => {
                        println!("{}", ui.info("voice hotkey timeout"));
                        continue;
                    }
                    Err(e) => {
                        println!("{}", ui.error(&format!("voice hotkey failed: {}", e)));
                        continue;
                    }
                }
            } else if arg == "listen" {
                if !voice::is_supported() {
                    println!("{}", ui.error("voice mode is supported on Windows only"));
                    continue;
                }
                println!("{}", ui.info("voice listening..."));
                voice_stats.listen_attempts += 1;
                match voice::recognize_once(voice_timeout_secs) {
                    Ok(Some(spoken)) => {
                        line_buf = spoken;
                        line = line_buf.as_str();
                        voice_stats.listen_ok += 1;
                        log_voice_event_runtime(&rt, "listen_manual", true, "recognized");
                        println!("{}", ui.info(&format!("voice_input={}", line_buf)));
                    }
                    Ok(None) => {
                        voice_stats.listen_empty += 1;
                        log_voice_event_runtime(&rt, "listen_manual", true, "empty");
                        println!("{}", ui.info("voice_input=none"));
                        continue;
                    }
                    Err(e) => {
                        voice_stats.listen_failed += 1;
                        voice_stats.last_error = Some(e.clone());
                        log_voice_event_runtime(&rt, "listen_manual", false, &e);
                        println!("{}", ui.error(&format!("voice listen failed: {}", e)));
                        continue;
                    }
                }
            } else if let Some(v) = arg.strip_prefix("engine ") {
                let engine_arg = v.trim();
                if engine_arg.eq_ignore_ascii_case("auto") {
                    voice_engine = voice::auto_engine();
                    sync_voice_config_fields(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    );
                    println!(
                        "{}",
                        ui.info(&format!(
                            "voice_engine={} (auto)",
                            voice::engine_name(voice_engine)
                        ))
                    );
                } else if let Some(parsed) = voice::parse_engine(engine_arg) {
                    voice_engine = parsed;
                    sync_voice_config_fields(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    );
                    println!(
                        "{}",
                        ui.info(&format!(
                            "voice_engine={}",
                            voice::engine_name(voice_engine)
                        ))
                    );
                    if matches!(voice_engine, voice::VoiceEngine::OpenAi)
                        && !voice::openai_key_present()
                    {
                        println!(
                            "{}",
                            ui.error(
                                "OPENAI_API_KEY missing; openai engine will fail until key is set"
                            )
                        );
                    }
                } else {
                    println!("{}", ui.error("Usage: /voice engine <local|openai|auto>"));
                }
                continue;
            } else if let Some(v) = arg.strip_prefix("test ") {
                if !voice::is_supported() {
                    println!("{}", ui.error("voice mode is supported on Windows only"));
                    continue;
                }
                let text = v.trim();
                if text.is_empty() {
                    println!("{}", ui.error("Usage: /voice test <text>"));
                    continue;
                }
                if voice_mute {
                    voice_stats.tts_skipped_mute += 1;
                    log_voice_event_runtime(&rt, "test_tts", true, "skipped_mute");
                    println!("{}", ui.info("voice_test_skipped (mute=true)"));
                    continue;
                }
                voice_stats.tts_attempts += 1;
                match voice::speak_text_with_engine(text, voice_engine, &voice_openai_voice) {
                    Ok(()) => {
                        voice_stats.tts_ok += 1;
                        log_voice_event_runtime(&rt, "test_tts", true, "ok");
                        println!("{}", ui.info("voice_test_ok"));
                    }
                    Err(e) => {
                        voice_stats.tts_failed += 1;
                        voice_stats.last_error = Some(e.clone());
                        log_voice_event_runtime(&rt, "test_tts", false, &e);
                        println!("{}", ui.error(&format!("voice_test_failed: {}", e)));
                    }
                }
                continue;
            } else if arg == "test" {
                if !voice::is_supported() {
                    println!("{}", ui.error("voice mode is supported on Windows only"));
                    continue;
                }
                if voice_mute {
                    voice_stats.tts_skipped_mute += 1;
                    log_voice_event_runtime(&rt, "test_tts", true, "skipped_mute");
                    println!("{}", ui.info("voice_test_skipped (mute=true)"));
                    continue;
                }
                let text = "Voice test from ASI Code.";
                voice_stats.tts_attempts += 1;
                match voice::speak_text_with_engine(text, voice_engine, &voice_openai_voice) {
                    Ok(()) => {
                        voice_stats.tts_ok += 1;
                        log_voice_event_runtime(&rt, "test_tts", true, "ok");
                        println!("{}", ui.info("voice_test_ok"));
                    }
                    Err(e) => {
                        voice_stats.tts_failed += 1;
                        voice_stats.last_error = Some(e.clone());
                        log_voice_event_runtime(&rt, "test_tts", false, &e);
                        println!("{}", ui.error(&format!("voice_test_failed: {}", e)));
                    }
                }
                continue;
            } else if let Some(v) = arg.strip_prefix("openai-voice ") {
                let name = v.trim();
                if name.is_empty() {
                    println!("{}", ui.error("Usage: /voice openai-voice <name>"));
                } else {
                    voice_openai_voice = name.to_string();
                    sync_voice_config_fields(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    );
                    println!(
                        "{}",
                        ui.info(&format!("voice_openai_voice={}", voice_openai_voice))
                    );
                }
                continue;
            } else if let Some(v) = arg.strip_prefix("timeout ") {
                if let Ok(secs) = v.trim().parse::<u64>() {
                    voice_timeout_secs = secs.clamp(1, 120);
                    sync_voice_config_fields(
                        &mut cfg,
                        voice_mode,
                        voice_engine,
                        &voice_openai_voice,
                        voice_timeout_secs,
                        voice_mute,
                        voice_ptt,
                        &voice_ptt_trigger,
                    );
                    println!(
                        "{}",
                        ui.info(&format!("voice_timeout_secs={}", voice_timeout_secs))
                    );
                } else {
                    println!("{}", ui.error("Usage: /voice timeout <1..120>"));
                }
                continue;
            } else {
                println!(
                    "{}",
                    ui.error("Usage: /voice [on|off|listen|timeout <sec>|engine <local|openai|auto>|openai-voice <name>|mute <on|off>|ptt <on|off>|ptt-trigger <token>|ptt-hotkey <key>|hotkey-listen|hold-listen [wait_secs]|test [text]|config [show|save|fallback-local <on|off>|local-soft-fail <on|off>|ptt <on|off>|ptt-trigger <token>|ptt-hotkey <key>]|stats|status]")
                );
                continue;
            }
        }
        if line == "/privacy" || line == "/privacy status" {
            println!("{}", ui.info(&privacy_status(&cfg)));
            continue;
        }
        if let Some(rest) = line.strip_prefix("/privacy ") {
            let msg = handle_privacy_command(&mut cfg, rest, &mut rt)?;
            println!("{}", ui.info(&msg));
            continue;
        }
        if line == "/flags" || line == "/flags list" {
            println!("{}", ui.info(&feature_status(&cfg)));
            println!("{}", ui.info(&permissions_status(&cfg)));
            println!("{}", ui.info(&session_permissions_status(&rt)));
            println!("{}", ui.info(&format!("auto_agent={}", auto_agent_enabled)));
            println!("{}", ui.info(&format!("speed={}", execution_speed.as_str())));
            println!(
                "{}",
                ui.info(&format!("work_mode={}", auto_work_mode_enabled))
            );
            println!(
                "{}",
                ui.info(&format!(
                    "checkpoint_auto={} checkpoint_exists={}",
                    auto_checkpoint_enabled,
                    store.checkpoint_exists()
                ))
            );
            println!(
                "{}",
                ui.info(&format!(
                    "voice_mode={} engine={} openai_voice={} timeout_secs={} mute={} fallback_local={} local_soft_fail={} ptt={} ptt_trigger={} ptt_hotkey={}",
                    voice_mode,
                    voice::engine_name(voice_engine),
                    voice_openai_voice,
                    voice_timeout_secs,
                    voice_mute,
                    voice::openai_fallback_local_enabled(),
                    voice::local_soft_fail_enabled(),
                    voice_ptt,
                voice_ptt_trigger,
                voice_ptt_hotkey
                ))
            );
            continue;
        }
        if let Some(rest) = line.strip_prefix("/flags ") {
            let msg = handle_flags_command(&mut cfg, rest)?;
            apply_runtime_flags_from_cfg(&mut rt, &cfg);
            println!("{}", ui.info(&msg));
            continue;
        }
        if line == "/policy sync" {
            let msg = policy::sync_remote_policy(&mut cfg)?;
            let provider = cfg.provider.clone();
            let model = cfg.model.clone();
            rt.permission_mode = cfg.permission_mode.clone();
            rt.rebind_provider_model(provider, model);
            apply_runtime_flags_from_cfg(&mut rt, &cfg);
            println!("{}", ui.info(&msg));
            continue;
        }
        if line == "/cost" {
            println!(
                "{}",
                ui.info(&format!(
                    "total_cost={} input_tokens={} output_tokens={} wallet={}",
                    cost::format_usd(rt.cumulative_cost_usd),
                    rt.cumulative_input_tokens,
                    rt.cumulative_output_tokens,
                    cost::format_usd(cfg.wallet_usd)
                ))
            );
            continue;
        }
        if line == "/compact" {
            println!("{}", ui.info(&rt.compact(8)));
            continue;
        }
        if line == "/clear" {
            rt.clear();
            println!("{}", ui.info("conversation cleared"));
            continue;
        }
        if line == "/permissions" || line == "/permissions list" {
            println!("{}", ui.info(&permissions_status(&cfg)));
            println!("{}", ui.info(&session_permissions_status(&rt)));
            println!("{}", ui.info(&format!("auto_agent={}", auto_agent_enabled)));
            println!("{}", ui.info(&format!("speed={}", execution_speed.as_str())));
            println!(
                "{}",
                ui.info(&format!("work_mode={}", auto_work_mode_enabled))
            );
            println!(
                "{}",
                ui.info(&format!(
                    "checkpoint_auto={} checkpoint_exists={}",
                    auto_checkpoint_enabled,
                    store.checkpoint_exists()
                ))
            );
            println!(
                "{}",
                ui.info(&format!(
                    "voice_mode={} engine={} openai_voice={} timeout_secs={} mute={} fallback_local={} local_soft_fail={} ptt={} ptt_trigger={} ptt_hotkey={}",
                    voice_mode,
                    voice::engine_name(voice_engine),
                    voice_openai_voice,
                    voice_timeout_secs,
                    voice_mute,
                    voice::openai_fallback_local_enabled(),
                    voice::local_soft_fail_enabled(),
                    voice_ptt,
                voice_ptt_trigger,
                voice_ptt_hotkey
                ))
            );
            continue;
        }
        if line == "/runtime-profile" || line == "/runtime-profile status" {
            println!(
                "{}",
                ui.info(&format!(
                    "runtime_profile permission_mode={} sandbox={} (safe={}+{}, fast={}+{})",
                    cfg.permission_mode,
                    rt.sandbox_name(),
                    SAFE_PROFILE_PERMISSION_MODE,
                    SAFE_PROFILE_SANDBOX_MODE,
                    FAST_PROFILE_PERMISSION_MODE,
                    FAST_PROFILE_SANDBOX_MODE
                ))
            );
            continue;
        }
        if let Some(rest) = line.strip_prefix("/runtime-profile ") {
            let arg = rest.trim().to_ascii_lowercase();
            if arg == "safe" {
                cfg.permission_mode = SAFE_PROFILE_PERMISSION_MODE.to_string();
                rt.permission_mode = cfg.permission_mode.clone();
                std::env::set_var("ASI_SANDBOX", SAFE_PROFILE_SANDBOX_MODE);
                rt.sandbox_strategy = sandbox::SandboxStrategy::from_env();
                let _ = cfg.save();
                println!(
                    "{}",
                    ui.info(&format!(
                        "runtime_profile=safe permission_mode={} sandbox={}",
                        cfg.permission_mode,
                        rt.sandbox_name()
                    ))
                );
                continue;
            }
            if arg == "fast" {
                cfg.permission_mode = FAST_PROFILE_PERMISSION_MODE.to_string();
                rt.permission_mode = cfg.permission_mode.clone();
                std::env::set_var("ASI_SANDBOX", FAST_PROFILE_SANDBOX_MODE);
                rt.sandbox_strategy = sandbox::SandboxStrategy::from_env();
                let _ = cfg.save();
                println!(
                    "{}",
                    ui.info(&format!(
                        "runtime_profile=fast permission_mode={} sandbox={}",
                        cfg.permission_mode,
                        rt.sandbox_name()
                    ))
                );
                continue;
            }
            println!(
                "{}",
                ui.error("Usage: /runtime-profile [safe|fast|status]")
            );
            continue;
        }
        if let Some(rest) = line.strip_prefix("/permissions ") {
            let msg = handle_permissions_command(&mut cfg, &mut rt, rest)?;
            println!("{}", ui.info(&msg));
            continue;
        }
        if let Some(v) = line.strip_prefix("/model ") {
            let requested_model = resolve_model_alias(v.trim());
            let (reconciled_model, fallback) =
                reconcile_model_for_provider(&rt.provider, &requested_model);
            rt.rebind_provider_model(rt.provider.clone(), reconciled_model);
            cfg.model = rt.model.clone();
            apply_runtime_flags_from_cfg(&mut rt, &cfg);
            let _ = cfg.save();
            if fallback {
                println!(
                    "{}",
                    ui.info(&format!(
                        "model {} incompatible with provider {}, fallback={}",
                        requested_model, rt.provider, rt.model
                    ))
                );
            }
            println!("{}", ui.info(&format!("model={}", rt.model)));
            continue;
        }
        if let Some(v) = line.strip_prefix("/provider ") {
            let provider_input = v.trim();
            let next_provider = normalize_provider_name(provider_input);
            cfg.provider = next_provider.clone();

            let requested_model = if looks_like_model_name(provider_input) {
                let hinted = resolve_model_alias(provider_input);
                if is_model_compatible_with_provider(&next_provider, &hinted) {
                    hinted
                } else {
                    cfg.model.clone()
                }
            } else {
                cfg.model.clone()
            };

            let (reconciled_model, fallback) =
                reconcile_model_for_provider(&next_provider, &requested_model);
            rt.rebind_provider_model(next_provider, reconciled_model);
            cfg.model = rt.model.clone();
            apply_runtime_flags_from_cfg(&mut rt, &cfg);
            let _ = cfg.save();
            if fallback {
                println!(
                    "{}",
                    ui.info(&format!(
                        "model {} incompatible with provider {}, fallback={}",
                        requested_model, rt.provider, rt.model
                    ))
                );
            }
            println!(
                "{}",
                ui.info(&format!("provider={} model={}", rt.provider, rt.model))
            );
            continue;
        }
        if line == "/profile" {
            println!(
                "{}",
                ui.info(&format!(
                    "profile={}",
                    if strict_profile_enabled {
                        "strict"
                    } else {
                        "standard"
                    }
                ))
            );
            continue;
        }
        if line == "/speed" {
            println!("{}", ui.info(&format!("speed={}", execution_speed.as_str())));
            continue;
        }
        if let Some(v) = line.strip_prefix("/speed ") {
            if let Some(next_speed) = parse_execution_speed(v.trim()) {
                execution_speed = next_speed;
                cfg.execution_speed = normalize_execution_speed(next_speed.as_str());
                let _ = cfg.save();
                println!("{}", ui.info(&format!("speed={}", next_speed.as_str())));
            } else {
                println!("{}", ui.error("Usage: /speed <sprint|deep>"));
            }
            continue;
        }
        if let Some(v) = line.strip_prefix("/profile ") {
            let raw = v.trim();
            if let Some(next_profile) = parse_prompt_profile(raw) {
                strict_profile_enabled = next_profile == PromptProfile::Strict;
                println!(
                    "{}",
                    ui.info(&format!("profile={}", next_profile.as_str()))
                );
            } else {
                println!("{}", ui.error("Usage: /profile <standard|strict>"));
            }
            continue;
        }
        if let Some(v) = line.strip_prefix("/think ") {
            rt.extended_thinking = matches!(v.trim(), "on" | "true" | "1");
            cfg.extended_thinking = rt.extended_thinking;
            let _ = cfg.save();
            println!(
                "{}",
                ui.info(&format!("extended_thinking={}", rt.extended_thinking))
            );
            continue;
        }
        if let Some(v) = line.strip_prefix("/markdown ") {
            markdown_render = matches!(v.trim(), "on" | "true" | "1");
            cfg.markdown_render = markdown_render;
            let _ = cfg.save();
            println!(
                "{}",
                ui.info(&format!("markdown_render={}", markdown_render))
            );
            continue;
        }
        if line == "/auto" {
            println!("{}", ui.info(&format!("auto_agent={}", auto_agent_enabled)));
            println!("{}", ui.info(&format!("speed={}", execution_speed.as_str())));
            let steps_desc = repl_auto_loop_limits
                .max_steps
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unlimited".to_string());
            println!(
                "{}",
                ui.info(&format!(
                    "auto_limits steps={} duration_secs={} no_progress_rounds={} constraint_blocks={}",
                    steps_desc,
                    repl_auto_loop_limits.max_duration.as_secs(),
                    repl_auto_loop_limits.max_no_progress_rounds,
                    repl_auto_loop_limits.max_consecutive_constraint_blocks
                ))
            );
            println!("{}", ui.info(&file_synopsis_cache.stats_line()));
            println!("{}", ui.info(&confidence_gate_stats.stats_line()));
            println!("{}", ui.info(&rt.provider_decode_stats_line()));
            println!(
                "{}",
                ui.info(&format!(
                    "confidence_gate strict={} max_retries={}",
                    strict_profile_enabled, CONFIDENCE_GATE_MAX_RETRIES
                ))
            );
            continue;
        }
        if let Some(v) = line.strip_prefix("/auto ") {
            let rest = v.trim();
            if rest == "status" {
                let steps_desc = repl_auto_loop_limits
                    .max_steps
                    .map(|vv| vv.to_string())
                    .unwrap_or_else(|| "unlimited".to_string());
                println!("{}", ui.info(&format!("auto_agent={}", auto_agent_enabled)));
                println!("{}", ui.info(&format!("speed={}", execution_speed.as_str())));
                println!(
                    "{}",
                    ui.info(&format!(
                        "auto_limits steps={} duration_secs={} no_progress_rounds={} constraint_blocks={}",
                        steps_desc,
                        repl_auto_loop_limits.max_duration.as_secs(),
                        repl_auto_loop_limits.max_no_progress_rounds,
                        repl_auto_loop_limits.max_consecutive_constraint_blocks
                    ))
                );
                println!("{}", ui.info(&file_synopsis_cache.stats_line()));
                println!("{}", ui.info(&confidence_gate_stats.stats_line()));
                println!("{}", ui.info(&rt.provider_decode_stats_line()));
                println!(
                    "{}",
                    ui.info(&format!(
                        "confidence_gate strict={} max_retries={}",
                        strict_profile_enabled, CONFIDENCE_GATE_MAX_RETRIES
                    ))
                );
                continue;
            }
            if let Some(raw_steps) = rest.strip_prefix("steps ") {
                if let Some(steps) = parse_auto_steps_value(raw_steps) {
                    repl_auto_loop_limits.max_steps = if steps == 0 { None } else { Some(steps) };
                    let steps_desc = repl_auto_loop_limits
                        .max_steps
                        .map(|vv| vv.to_string())
                        .unwrap_or_else(|| "unlimited".to_string());
                    println!("{}", ui.info(&format!("auto_steps={}", steps_desc)));
                } else {
                    println!(
                        "{}",
                        ui.error("Usage: /auto steps <n|0|unlimited>")
                    );
                }
            } else if let Some(raw_secs) = rest.strip_prefix("duration ") {
                if let Some(d) = parse_auto_limit_duration(raw_secs) {
                    repl_auto_loop_limits.max_duration = d;
                    println!(
                        "{}",
                        ui.info(&format!("auto_duration_secs={}", d.as_secs()))
                    );
                } else {
                    println!(
                        "{}",
                        ui.error("Usage: /auto duration <seconds|0|unlimited>")
                    );
                }
            } else if let Some(raw_rounds) = rest.strip_prefix("no-progress ") {
                if let Ok(vv) = raw_rounds.trim().parse::<usize>() {
                    repl_auto_loop_limits.max_no_progress_rounds = vv.clamp(1, 200);
                    println!(
                        "{}",
                        ui.info(&format!(
                            "auto_no_progress_rounds={}",
                            repl_auto_loop_limits.max_no_progress_rounds
                        ))
                    );
                } else {
                    println!("{}", ui.error("Usage: /auto no-progress <n>"));
                }
            } else if let Some(raw_blocks) = rest.strip_prefix("constraint-blocks ") {
                if let Ok(vv) = raw_blocks.trim().parse::<usize>() {
                    repl_auto_loop_limits.max_consecutive_constraint_blocks = vv.clamp(1, 50);
                    println!(
                        "{}",
                        ui.info(&format!(
                            "auto_constraint_blocks={}",
                            repl_auto_loop_limits.max_consecutive_constraint_blocks
                        ))
                    );
                } else {
                    println!("{}", ui.error("Usage: /auto constraint-blocks <n>"));
                }
            } else {
                auto_agent_enabled = matches!(rest, "on" | "true" | "1");
                println!("{}", ui.info(&format!("auto_agent={}", auto_agent_enabled)));
            }
            continue;
        }
        if line == "/workmode" {
            println!(
                "{}",
                ui.info(&format!("work_mode={}", auto_work_mode_enabled))
            );
            println!(
                "{}",
                ui.info(&format!(
                    "checkpoint_auto={} checkpoint_exists={}",
                    auto_checkpoint_enabled,
                    store.checkpoint_exists()
                ))
            );
            println!(
                "{}",
                ui.info(&format!(
                    "voice_mode={} engine={} openai_voice={} timeout_secs={} mute={} fallback_local={} local_soft_fail={} ptt={} ptt_trigger={} ptt_hotkey={}",
                    voice_mode,
                    voice::engine_name(voice_engine),
                    voice_openai_voice,
                    voice_timeout_secs,
                    voice_mute,
                    voice::openai_fallback_local_enabled(),
                    voice::local_soft_fail_enabled(),
                    voice_ptt,
                voice_ptt_trigger,
                voice_ptt_hotkey
                ))
            );
            continue;
        }
        if let Some(v) = line.strip_prefix("/workmode ") {
            auto_work_mode_enabled = matches!(v.trim(), "on" | "true" | "1");
            println!(
                "{}",
                ui.info(&format!("work_mode={}", auto_work_mode_enabled))
            );
            println!(
                "{}",
                ui.info(&format!(
                    "checkpoint_auto={} checkpoint_exists={}",
                    auto_checkpoint_enabled,
                    store.checkpoint_exists()
                ))
            );
            println!(
                "{}",
                ui.info(&format!(
                    "voice_mode={} engine={} openai_voice={} timeout_secs={} mute={} fallback_local={} local_soft_fail={} ptt={} ptt_trigger={} ptt_hotkey={}",
                    voice_mode,
                    voice::engine_name(voice_engine),
                    voice_openai_voice,
                    voice_timeout_secs,
                    voice_mute,
                    voice::openai_fallback_local_enabled(),
                    voice::local_soft_fail_enabled(),
                    voice_ptt,
                voice_ptt_trigger,
                voice_ptt_hotkey
                ))
            );
            continue;
        }
        if line == "/native" {
            println!(
                "{}",
                ui.info(&format!("native_tool_calling={}", rt.native_tool_calling))
            );
            println!("{}", ui.info(&rt.status_provider_runtime_line()));
            continue;
        }
        if let Some(v) = line.strip_prefix("/native ") {
            rt.native_tool_calling = matches!(v.trim(), "on" | "true" | "1");
            println!(
                "{}",
                ui.info(&format!("native_tool_calling={}", rt.native_tool_calling))
            );
            continue;
        }
        if line == "/api" || line == "/api-page" {
            render_api_page(&cfg);
            continue;
        }
        if let Some(cmd) = line.strip_prefix("/run ") {
            let cmd = cmd.trim();
            if cmd.is_empty() {
                println!("{}", ui.error("Usage: /run <command>"));
                continue;
            }
            println!("{}", ui.info(&format!("running: {}", cmd)));
            let code = run_live_command(cmd)?;
            println!("{}", ui.info(&format!("command exit_code={}", code)));
            continue;
        }
        if line == "/tools" {
            println!("{}", tool_index());
            continue;
        }
        if line == "/audit" || line == "/audit tail" {
            println!("{}", ui.info(&audit::read_tail(20)?));
            continue;
        }
        if let Some(rest) = line.strip_prefix("/audit ") {
            println!("{}", ui.info(&handle_audit_command(rest)?));
            continue;
        }
        if let Some(rest) = line.strip_prefix("/sessions") {
            let tokens: Vec<&str> = rest.split_whitespace().collect();
            let mut parsed_limit = 10usize;
            let mut filter_agent_enabled = false;
            let mut filter_blocked_only = false;
            if let Some(first) = tokens.first() {
                if let Ok(v) = first.parse::<usize>() {
                    parsed_limit = v;
                }
            }
            for token in &tokens {
                let t = token.to_ascii_lowercase();
                if t == "agent-enabled" || t == "agent_enabled" || t == "agent" {
                    filter_agent_enabled = true;
                }
                if t == "blocked-only" || t == "blocked_only" || t == "blocked" {
                    filter_blocked_only = true;
                }
            }
            for sid in store.list_sessions(parsed_limit)? {
                match store.load(&sid) {
                    Ok(s) => {
                        if !session_matches_filters(
                            &s,
                            filter_agent_enabled,
                            filter_blocked_only,
                        ) {
                            continue;
                        }
                        if let Some(meta) = s.meta {
                            println!(
                                "{} provider={} model={} source={} agent_enabled={} loop_stop_reason={} cg_checks={} cg_blocked={}",
                                sid,
                                s.provider,
                                s.model,
                                meta.source,
                                meta.agent_enabled,
                                meta.auto_loop_stop_reason.as_deref().unwrap_or("none"),
                                meta.confidence_gate.checks,
                                meta.confidence_gate.blocked_risky_toolcalls
                            );
                        } else {
                            println!("{} provider={} model={}", sid, s.provider, s.model);
                        }
                    }
                    Err(_) => println!("{}", sid),
                }
            }
            continue;
        }
        if let Some(id) = line.strip_prefix("/resume ") {
            let s = store.load(id.trim())?;
            rt.load_session_messages(s.provider.clone(), s.model.clone(), s.messages.clone());
            cfg.provider = s.provider;
            cfg.model = s.model;
            apply_runtime_flags_from_cfg(&mut rt, &cfg);
            let meta_line = if let Some(meta) = &s.meta {
                format!(
                    " source={} agent_enabled={} loop_stop_reason={} cg_checks={} cg_blocked={}",
                    meta.source,
                    meta.agent_enabled,
                    meta.auto_loop_stop_reason.as_deref().unwrap_or("none"),
                    meta.confidence_gate.checks,
                    meta.confidence_gate.blocked_risky_toolcalls
                )
            } else {
                String::new()
            };
            println!(
                "{}",
                ui.info(&format!(
                    "loaded session={} provider={} model={} messages={}{}",
                    s.session_id,
                    cfg.provider,
                    cfg.model,
                    s.messages.len(),
                    meta_line
                ))
            );
            continue;
        }
        if line == "/save" {
            let meta = session::SessionMeta {
                source: "repl_manual_save".to_string(),
                agent_enabled: auto_agent_enabled,
                auto_loop_stop_reason: previous_auto_loop_stop_reason.clone(),
                confidence_gate: session::ConfidenceGateSessionStats {
                    checks: confidence_gate_stats.checks(),
                    missing_declaration: confidence_gate_stats.declaration_missing(),
                    low_declaration: confidence_gate_stats.declaration_low(),
                    blocked_risky_toolcalls: confidence_gate_stats.blocked_risky_toolcalls(),
                    retries_exhausted: confidence_gate_stats.retries_exhausted(),
                },
            };
            let path = store.save_with_meta(
                &rt.provider,
                &rt.model,
                rt.as_json_messages(),
                Some(meta),
            )?;
            println!("{}", ui.info(&format!("saved={}", path.display())));
            continue;
        }
        if line == "/checkpoint" || line == "/checkpoint status" {
            println!(
                "{}",
                ui.info(&format!(
                    "checkpoint_auto={} checkpoint_exists={}",
                    auto_checkpoint_enabled,
                    store.checkpoint_exists()
                ))
            );
            continue;
        }
        if let Some(rest) = line.strip_prefix("/checkpoint ") {
            let arg = rest.trim();
            if arg == "save" {
                let meta = session::SessionMeta {
                    source: "repl_checkpoint_manual".to_string(),
                    agent_enabled: auto_agent_enabled,
                    auto_loop_stop_reason: previous_auto_loop_stop_reason.clone(),
                    confidence_gate: session::ConfidenceGateSessionStats {
                        checks: confidence_gate_stats.checks(),
                        missing_declaration: confidence_gate_stats.declaration_missing(),
                        low_declaration: confidence_gate_stats.declaration_low(),
                        blocked_risky_toolcalls: confidence_gate_stats.blocked_risky_toolcalls(),
                        retries_exhausted: confidence_gate_stats.retries_exhausted(),
                    },
                };
                match store.save_checkpoint_with_meta(
                    &rt.provider,
                    &rt.model,
                    rt.as_json_messages(),
                    Some(meta),
                ) {
                    Ok(path) => println!(
                        "{}",
                        ui.info(&format!("checkpoint_saved={}", path.display()))
                    ),
                    Err(e) => println!("{}", ui.error(&format!("checkpoint_save_failed: {}", e))),
                }
            } else if arg == "load" {
                match store.load_checkpoint() {
                    Ok(s) => {
                        rt.load_session_messages(
                            s.provider.clone(),
                            s.model.clone(),
                            s.messages.clone(),
                        );
                        cfg.provider = s.provider;
                        cfg.model = s.model;
                        apply_runtime_flags_from_cfg(&mut rt, &cfg);
                        let meta_line = if let Some(meta) = &s.meta {
                            format!(
                                " source={} agent_enabled={} loop_stop_reason={} cg_checks={} cg_blocked={}",
                                meta.source,
                                meta.agent_enabled,
                                meta.auto_loop_stop_reason.as_deref().unwrap_or("none"),
                                meta.confidence_gate.checks,
                                meta.confidence_gate.blocked_risky_toolcalls
                            )
                        } else {
                            String::new()
                        };
                        println!(
                            "{}",
                            ui.info(&format!(
                                "checkpoint_loaded={} provider={} model={} messages={}{}",
                                s.session_id,
                                cfg.provider,
                                cfg.model,
                                s.messages.len(),
                                meta_line
                            ))
                        );
                    }
                    Err(e) => println!("{}", ui.error(&format!("checkpoint_load_failed: {}", e))),
                }
            } else if arg == "clear" {
                match store.clear_checkpoint() {
                    Ok(()) => println!("{}", ui.info("checkpoint_cleared=true")),
                    Err(e) => println!("{}", ui.error(&format!("checkpoint_clear_failed: {}", e))),
                }
            } else if let Some(flag) = parse_on_off(arg) {
                auto_checkpoint_enabled = flag;
                println!(
                    "{}",
                    ui.info(&format!("checkpoint_auto={}", auto_checkpoint_enabled))
                );
            } else {
                println!(
                    "{}",
                    ui.error("Usage: /checkpoint [status|on|off|save|load|clear]")
                );
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("/oauth ") {
            println!("{}", ui.info(&handle_oauth_command(rest)?));
            continue;
        }
        if let Some(rest) = line.strip_prefix("/todo ") {
            println!("{}", handle_todo_command(rest)?);
            continue;
        }
        if let Some(rest) = line.strip_prefix("/memory") {
            println!("{}", handle_memory_command(rest)?);
            continue;
        }
        if let Some(rest) = line.strip_prefix("/git") {
            println!("{}", handle_git_command(rest, cfg.undercover_mode)?);
            continue;
        }
        if let Some(rest) = line.strip_prefix("/mcp") {
            println!("{}", handle_mcp_command(rest)?);
            continue;
        }
        if let Some(rest) = line.strip_prefix("/plugin") {
            println!("{}", handle_plugin_command(rest)?);
            continue;
        }
        if let Some(rest) = line.strip_prefix("/hooks") {
            println!("{}", handle_hooks_command(rest)?);
            continue;
        }
        if let Some(rest) = line.strip_prefix("/notebook ") {
            println!("{}", handle_notebook_command(rest)?);
            continue;
        }
        if line == "/autoresearch" || line == "/autoresearch help" {
            println!("{}", ui.info(autoresearch_repl_usage()));
            continue;
        }
        if let Some(rest) = line.strip_prefix("/autoresearch ") {
            println!("{}", ui.info(&handle_autoresearch_repl_command(rest)?));
            continue;
        }
        if line == "/tokenizer" || line == "/tokenizer help" {
            println!("{}", ui.info(tokenizer::repl_usage()));
            continue;
        }
        if let Some(rest) = line.strip_prefix("/tokenizer ") {
            println!("{}", ui.info(&tokenizer::handle_repl_command(rest)?));
            continue;
        }
        if line == "/agent" || line == "/agent help" {
            println!("{}", ui.info(&subagent_usage()));
            continue;
        }
        if let Some(task) = line.strip_prefix("/agent ") {
            if cfg.is_feature_disabled("subagent") {
                println!("{}", ui.error("subagent is disabled by feature killswitch"));
                continue;
            }
            let subagent_run_cfg = build_subagent_run_config(
                &cfg,
                &rt,
                execution_speed,
                strict_profile_enabled,
            );
            let agent_cmd = task.trim();
            if agent_cmd == "view"
                || agent_cmd.starts_with("view ")
                || agent_cmd == "front"
                || agent_cmd.starts_with("front ")
                || agent_cmd == "back"
                || agent_cmd.starts_with("back ")
            {
                let (scope, filter_raw) = if agent_cmd.starts_with("front") {
                    (
                        AgentViewScope::Foreground,
                        agent_cmd.strip_prefix("front").unwrap_or("").trim(),
                    )
                } else if agent_cmd.starts_with("back") {
                    (
                        AgentViewScope::Background,
                        agent_cmd.strip_prefix("back").unwrap_or("").trim(),
                    )
                } else {
                    let token = agent_cmd
                        .strip_prefix("view")
                        .map(|v| v.trim())
                        .unwrap_or("");
                    let mut local_scope = AgentViewScope::All;
                    let mut rest = token;
                    if let Some(first) = token.split_whitespace().next() {
                        if let Some(parsed_scope) = AgentViewScope::from_opt(Some(first)) {
                            local_scope = parsed_scope;
                            rest = token
                                .strip_prefix(first)
                                .map(|v| v.trim())
                                .unwrap_or("");
                        }
                    }
                    (local_scope, rest)
                };
                let (profile_filter, skill_filter) =
                    match parse_agent_view_filters(&format!("x {}", filter_raw)) {
                        Ok(v) => v,
                        Err(e) => {
                            println!("{}", ui.error(&e));
                            continue;
                        }
                    };
                let rows = subagent_manager.list();
                let filtered = filter_subagent_rows(
                    rows,
                    scope,
                    profile_filter.as_deref(),
                    skill_filter.as_deref(),
                );
                println!(
                    "{}",
                    ui.info(&format_subagent_list_with_filters(
                        &filtered,
                        scope,
                        profile_filter.as_deref(),
                        skill_filter.as_deref(),
                    ))
                );
                continue;
            }
            if agent_cmd.starts_with("list") {
                let opts = match parse_agent_list_options(agent_cmd) {
                    Ok(v) => v,
                    Err(e) => {
                        println!("{}", ui.error(&e));
                        continue;
                    }
                };
                let rows = subagent_manager.list();
                let filtered = filter_subagent_rows(
                    rows,
                    opts.scope,
                    opts.profile.as_deref(),
                    opts.skill.as_deref(),
                );
                if !matches!(opts.output_mode, AgentOutputMode::Text) {
                    print_agent_machine_output(
                        opts.output_mode.clone(),
                        "list",
                        build_agent_list_payload(
                            &filtered,
                            opts.scope,
                            opts.profile.as_deref(),
                            opts.skill.as_deref(),
                        ),
                    );
                } else {
                    println!(
                        "{}",
                        ui.info(&format_subagent_list_with_filters(
                            &filtered,
                            opts.scope,
                            opts.profile.as_deref(),
                            opts.skill.as_deref(),
                        ))
                    );
                }
                continue;
            }
            if agent_cmd.starts_with("close") {
                let opts = match parse_agent_close_options(agent_cmd) {
                    Ok(v) => v,
                    Err(e) => {
                        println!("{}", ui.error(&e));
                        continue;
                    }
                };
                let msg = subagent_manager.close(&opts.id)?;
                if !matches!(opts.output_mode, AgentOutputMode::Text) {
                    print_agent_machine_output(
                        opts.output_mode.clone(),
                        "close",
                        build_agent_close_payload(&opts.id, &msg),
                    );
                } else {
                    println!("{}", ui.info(&msg));
                }
                continue;
            }
            if agent_cmd.starts_with("cancel") {
                let opts = match parse_agent_cancel_options(agent_cmd) {
                    Ok(v) => v,
                    Err(e) => {
                        println!("{}", ui.error(&e));
                        continue;
                    }
                };
                let msg = subagent_manager.cancel(&opts.id)?;
                if !matches!(opts.output_mode, AgentOutputMode::Text) {
                    let state = subagent_manager.state_of(&opts.id);
                    print_agent_machine_output(
                        opts.output_mode.clone(),
                        "cancel",
                        build_agent_cancel_payload(&opts.id, state.as_ref(), &msg),
                    );
                } else {
                    println!("{}", ui.info(&msg));
                }
                continue;
            }
            if agent_cmd.starts_with("retry") {
                let opts = match parse_agent_retry_options(agent_cmd) {
                    Ok(v) => v,
                    Err(e) => {
                        println!("{}", ui.error(&e));
                        continue;
                    }
                };
                let msg = subagent_manager.retry(&subagent_run_cfg, &opts.id)?;
                if !matches!(opts.output_mode, AgentOutputMode::Text) {
                    let state = subagent_manager.state_of(&opts.id);
                    print_agent_machine_output(
                        opts.output_mode.clone(),
                        "retry",
                        build_agent_retry_payload(&opts.id, state.as_ref(), &msg),
                    );
                } else {
                    println!("{}", ui.info(&msg));
                }
                continue;
            }
            if agent_cmd.starts_with("log") {
                let opts = match parse_agent_log_options(agent_cmd) {
                    Ok(v) => v,
                    Err(e) => {
                        println!("{}", ui.error(&e));
                        continue;
                    }
                };
                let (state, events, total_events) =
                    subagent_manager.read_events(&opts.id, opts.tail)?;
                if !matches!(opts.output_mode, AgentOutputMode::Text) {
                    print_agent_machine_output(
                        opts.output_mode.clone(),
                        "log",
                        build_agent_log_payload(&state, &events, total_events, opts.tail),
                    );
                } else {
                    println!(
                        "{}",
                        ui.info(&format_subagent_log_rows(&state, &events, total_events))
                    );
                }
                continue;
            }
            if agent_cmd.starts_with("status") {
                let opts = match parse_agent_status_options(agent_cmd) {
                    Ok(v) => v,
                    Err(e) => {
                        println!("{}", ui.error(&e));
                        continue;
                    }
                };
                let state = subagent_manager.state_of(&opts.id);
                if !matches!(opts.output_mode, AgentOutputMode::Text) {
                    print_agent_machine_output(
                        opts.output_mode.clone(),
                        "status",
                        build_agent_status_payload(state.as_ref(), &opts.id),
                    );
                } else if let Some(state) = state {
                    let finished = state
                        .finished_at_ms
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    let last_interrupted = state
                        .last_interrupted_at_ms
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    let allow_rules = if state.task.allow_rules.is_empty() {
                        "-".to_string()
                    } else {
                        state.task.allow_rules.join(",")
                    };
                    let deny_rules = if state.task.deny_rules.is_empty() {
                        "-".to_string()
                    } else {
                        state.task.deny_rules.join(",")
                    };
                    println!(
                        "{}",
                        ui.info(&format!(
                            "subagent id={} status={} provider={} model={} turns={} interrupted_count={} last_interrupted_at_ms={} started_at_ms={} finished_at_ms={} allow_rules=[{}] deny_rules=[{}]",
                            state.task.id,
                            state.status,
                            state.task.provider,
                            state.task.model,
                            state.submitted_turns,
                            state.interrupted_count,
                            last_interrupted,
                            state.task.started_at_ms,
                            finished,
                            allow_rules,
                            deny_rules
                        ))
                    );
                    println!(
                        "{}",
                        ui.info(&format!("subagent rules_source={}", state.task.rules_source))
                    );
                } else {
                    println!(
                        "{}",
                        ui.info(&format!("subagent id={} status=unknown", opts.id))
                    );
                }
                continue;
            }
            if let Some(rest) = agent_cmd.strip_prefix("send ") {
                let opts = match parse_agent_send_options(rest) {
                    Ok(v) => v,
                    Err(e) => {
                        println!("{}", ui.error(&e));
                        continue;
                    }
                };
                let msg = subagent_manager.send(
                    &subagent_run_cfg,
                    &opts.id,
                    &opts.task,
                    opts.interrupt,
                    opts.no_context,
                )?;
                if !matches!(opts.output_mode, AgentOutputMode::Text) {
                    let state = subagent_manager.state_of(&opts.id);
                    print_agent_machine_output(
                        opts.output_mode.clone(),
                        "send",
                        build_agent_send_payload(
                            state.as_ref(),
                            &opts.id,
                            opts.no_context,
                            opts.interrupt,
                            &msg,
                        ),
                    );
                } else {
                    println!("{}", ui.info(&msg));
                }
                continue;
            }
            if agent_cmd.starts_with("wait") {
                let opts = match parse_agent_wait_options(agent_cmd) {
                    Ok(v) => v,
                    Err(e) => {
                        println!("{}", ui.error(&e));
                        continue;
                    }
                };
                let outcome = if let Some(id) = opts.target_id.as_deref() {
                    subagent_manager.wait_id(id, opts.timeout_secs)?
                } else {
                    subagent_manager.wait_any(opts.timeout_secs)
                };
                match outcome {
                    Some(done) => {
                        let status = if done.result.is_ok() { "done" } else { "error" };
                        let body = done.result.clone().unwrap_or_else(|e| e);
                        if !matches!(opts.output_mode, AgentOutputMode::Text) {
                            print_agent_machine_output(
                                opts.output_mode.clone(),
                                "wait",
                                build_agent_wait_done_payload(&done),
                            );
                        } else {
                            let title = format!("agent {}", done.task.id);
                            println!("{}", ui.tool_panel(&title, status, &body));
                        }
                    }
                    None => {
                        if !matches!(opts.output_mode, AgentOutputMode::Text) {
                            let payload = if subagent_manager.has_active() {
                                build_agent_wait_timeout_payload(opts.timeout_secs)
                            } else {
                                build_agent_wait_idle_payload()
                            };
                            print_agent_machine_output(opts.output_mode.clone(), "wait", payload);
                            continue;
                        }
                        if subagent_manager.has_active() {
                            println!(
                                "{}",
                                ui.info(&format!("no subagent finished within {}s", opts.timeout_secs))
                            );
                        } else {
                            println!("{}", ui.info("no running subagents"));
                        }
                    }
                }
                continue;
            }

            let spawn_opts = match parse_agent_spawn_options(agent_cmd) {
                Ok(v) => v,
                Err(e) => {
                    println!("{}", ui.error(&e));
                    continue;
                }
            };
            let mut spawn_cfg = subagent_run_cfg.clone();
            if let Some(profile_name) = spawn_opts.profile.as_deref() {
                match load_agent_profile(profile_name) {
                    Ok(profile) => {
                        spawn_cfg.profile_name = Some(profile_name.trim().to_string());
                        apply_agent_profile_to_config(&mut spawn_cfg, &profile);
                    }
                    Err(e) => {
                        println!("{}", ui.error(&e));
                        continue;
                    }
                }
            }
            let id =
                subagent_manager.spawn(&spawn_cfg, &spawn_opts.task, spawn_opts.background);
            if !matches!(spawn_opts.output_mode, AgentOutputMode::Text) {
                print_agent_machine_output(
                    spawn_opts.output_mode.clone(),
                    "spawn",
                    build_agent_spawn_payload(
                        &id,
                        &spawn_cfg.provider,
                        &spawn_cfg.model,
                        spawn_cfg.profile_name.as_deref(),
                        &spawn_cfg.default_skills,
                        spawn_opts.background,
                        &spawn_opts.task,
                    ),
                );
            } else {
                println!(
                    "{}",
                    ui.info(&format!(
                        "subagent spawned id={} run_mode={} provider={} model={}",
                        id,
                        if spawn_opts.background {
                            "background"
                        } else {
                            "foreground"
                        },
                        spawn_cfg.provider,
                        spawn_cfg.model
                    ))
                );
            }
            continue;
        }

        // Skills: `/skills` lists / reloads, `/<name> [args]` dispatches.
        if line == "/skills" || line == "/skills list" {
            print_skill_list(&skill_registry, &ui);
            continue;
        }
        if line == "/skills reload" {
            skill_registry = load_skill_registry_for_repl(&cfg, &ui);
            println!("{}", ui.info("skills reloaded"));
            continue;
        }
        // Worktree slash command. We dispatch BEFORE the skill resolver so
        // a skill named "worktree" cannot accidentally shadow it (the
        // registry already rejects builtin names, but this is belt and
        // suspenders).
        if line == "/worktree" || line == "/worktree list" {
            handle_worktree_list(&ui);
            continue;
        }
        if let Some(rest) = line.strip_prefix("/worktree ") {
            let trimmed = rest.trim();
            handle_worktree_command(trimmed, &mut worktree_session, &ui);
            continue;
        }
        if line == "/cron" || line == "/cron list" {
            handle_cron_list(&ui);
            continue;
        }
        if let Some(rest) = line.strip_prefix("/cron ") {
            let trimmed = rest.trim();
            handle_cron_command(trimmed, &ui);
            continue;
        }
        // Resolve a skill match into a `/code <rendered_body>` line and let
        // the existing `/code` branch handle the workspace snapshot, prompt
        // building, auto loop, etc. This keeps the skill path consistent
        // with everything else without duplicating the runtime plumbing.
        let mut skill_dispatched_line: Option<String> = None;
        if let Some((skill, skill_args)) = skill_registry.try_dispatch(line) {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let today = current_date_iso();
            let rendered = skill.render(&skill_args, &cwd, &today);
            println!(
                "{}",
                ui.info(&format!(
                    "skill: {} ({})",
                    skill.fqname(),
                    match skill.source_kind {
                        skills::SkillSource::User => "user",
                        skills::SkillSource::Project => "project",
                    }
                ))
            );
            if !skill.allowed_tools.is_empty() {
                println!(
                    "{}",
                    ui.info(&format!(
                        "skill allowed_tools: {}",
                        skill.allowed_tools.join(", ")
                    ))
                );
            }
            skill_dispatched_line = Some(format!("/code {}", rendered));
        }
        let line_owned = skill_dispatched_line.unwrap_or_else(|| line.to_string());
        let line: &str = &line_owned;

        let mut model_input = line.to_string();
        if let Some(task) = line.strip_prefix("/work ") {
            let trimmed_task = task.trim();
            if trimmed_task.is_empty() {
                println!("{}", ui.error("Usage: /work <task>"));
                continue;
            }
            let root = std::env::current_dir().map_err(|e| e.to_string())?;
            let snapshot = build_workspace_snapshot(&root);
            model_input = build_work_prompt(trimmed_task, &snapshot, strict_profile_enabled);
            println!("{}", ui.info("work mode: workspace snapshot prepared"));
        }
        if let Some(task) = line.strip_prefix("/code ") {
            let trimmed_task = task.trim();
            if trimmed_task.is_empty() {
                println!("{}", ui.error("Usage: /code <task>"));
                continue;
            }
            let root = std::env::current_dir().map_err(|e| e.to_string())?;
            let snapshot = build_workspace_snapshot(&root);
            model_input = build_work_prompt(trimmed_task, &snapshot, strict_profile_enabled);
            println!("{}", ui.info("code mode: workspace snapshot prepared"));
        }
        if let Some(task) = line.strip_prefix("/secure ") {
            let trimmed_task = task.trim();
            if trimmed_task.is_empty() {
                println!("{}", ui.error("Usage: /secure <task>"));
                continue;
            }
            let root = std::env::current_dir().map_err(|e| e.to_string())?;
            let snapshot = build_workspace_snapshot(&root);
            model_input =
                build_security_work_prompt(trimmed_task, &snapshot, strict_profile_enabled);
            println!("{}", ui.info("security mode: workspace snapshot prepared"));
        }
        let review_task_and_json_only = parse_review_task_json_only(line);
        let mut review_json_only_mode = false;
        if let Some((trimmed_task, json_only)) = review_task_and_json_only {
            review_json_only_mode = json_only;
            if trimmed_task.is_empty() {
                println!("{}", ui.error("Usage: /review <task>"));
                continue;
            }
            let root = std::env::current_dir().map_err(|e| e.to_string())?;
            let snapshot = build_workspace_snapshot(&root);
            model_input = build_review_prompt(&trimmed_task, &snapshot, strict_profile_enabled);
            if review_json_only_mode {
                println!(
                    "{}",
                    ui.info("review mode: workspace snapshot prepared (json-only)")
                );
            } else {
                println!("{}", ui.info("review mode: workspace snapshot prepared"));
            }
        }
        if !line.starts_with('/') && (auto_work_mode_enabled || is_coding_intent(line)) {
            let root = std::env::current_dir().map_err(|e| e.to_string())?;
            let snapshot = build_workspace_snapshot(&root);
            model_input = build_work_prompt(line, &snapshot, strict_profile_enabled);
            if auto_work_mode_enabled {
                println!(
                    "{}",
                    ui.info("work mode: auto-injected workspace context for coding task")
                );
            } else {
                println!(
                    "{}",
                    ui.info("code intent detected: auto-injected workspace context")
                );
            }
        }

        let is_tool_call = model_input.starts_with("/toolcall ");
        let start = Instant::now();
        let mut streamed = false;
        let mut stream_tokens = 0usize;
        let constraints_for_turn = derive_tool_execution_constraints(line, strict_profile_enabled);
        let complexity_for_turn = estimate_task_complexity(line, strict_profile_enabled);
        let adaptive_limits_for_turn = apply_adaptive_budgets(
            repl_auto_loop_limits,
            complexity_for_turn,
            strict_profile_enabled,
            execution_speed,
        );
        let auto_tooling_enabled_for_turn =
            auto_agent_enabled && should_enable_auto_tooling_for_turn(line, auto_work_mode_enabled);
        let model_input_for_runtime = if !is_tool_call && auto_tooling_enabled_for_turn {
            let header = format_context_contract_header(
                &rt,
                strict_profile_enabled,
                execution_speed,
                constraints_for_turn,
                adaptive_limits_for_turn,
                previous_auto_loop_stop_reason.as_deref(),
            );
            format!("{}\n\n{}", header, model_input)
        } else if !is_tool_call && auto_agent_enabled {
            // Auto-agent is on, but this turn is smalltalk/non-coding.
            // Keep plain chat behavior while preventing fabricated pseudo-execution output.
            let chat_guard = build_chat_no_fabrication_guard(&rt.permission_mode);
            format!("{}\n\n{}", chat_guard, model_input)
        } else if !is_tool_call {
            let manual_guard = build_manual_chat_guard(&rt.permission_mode);
            format!("{}\n\n{}", manual_guard, model_input)
        } else {
            model_input.clone()
        };

        if !is_tool_call {
            println!("{}", ui.spinning_line(0, 0));
        }

        let mut result = rt.run_turn_streaming(&model_input_for_runtime, |delta| {
            if !streamed {
                streamed = true;
                println!();
                print!("{} • ", ui.assistant_label());
            }
            stream_tokens += estimate_tokens(delta);
            print!("{}", delta);
            let _ = io::stdout().flush();
        });
        log_interaction_event(&cfg, &rt, &model_input_for_runtime, &result);

        if let Some(thinking) = &result.thinking {
            println!("{}", ui.thinking_block(thinking));
        }

        if streamed {
            println!();
        }

        if review_json_only_mode {
            let review_payload = build_review_json_payload(line, &result.text);
            println!("{}", build_review_json_only_repl_output(&review_payload));
        } else if result.stop_reason == "provider_error" {
            println!("{}", ui.error(&result.text));
        } else if !result.native_tool_calls.is_empty() {
            // Display native tool call results
            for nr in &result.native_tool_calls {
                let status = if nr.ok { "ok" } else { "error" };
                println!(
                    "{}",
                    ui.tool_panel(
                        &nr.tool_name,
                        status,
                        &compact_tool_panel_body(&nr.tool_name, status, &nr.result_text)
                    )
                );
            }
        } else if result.is_tool_result {
            if let Some((name, status, body)) = parse_tool_payload(&result.text) {
                println!(
                    "{}",
                    ui.tool_panel(
                        &name,
                        &status,
                        &compact_tool_panel_body(&name, &status, &body)
                    )
                );
            } else {
                println!("{}", ui.tool_panel("tool", "result", &result.text));
            }
        } else if !streamed {
            let text = if markdown_render {
                markdown::render_markdown_ansi(&result.text)
            } else {
                result.text.clone()
            };
            println!("{}", ui.assistant(&text));
        }

        let mut turn_cost_sum = result.turn_cost_usd;
        let mut auto_loop_stop_reason: Option<String> = None;
        // Auto-agent loop: triggered by completed text or native tool_use results
        if auto_tooling_enabled_for_turn
            && !is_tool_call
            && is_auto_loop_continuable_stop_reason(&result.stop_reason)
        {
            let limits = adaptive_limits_for_turn;
            println!("{}", ui.info(&format_adaptive_budget_note(complexity_for_turn, repl_auto_loop_limits, limits)));
            let (loop_result, extra_cost, loop_stop_reason) = run_auto_tool_loop(
                &mut rt,
                &cfg,
                &ui,
                line,
                markdown_render,
                &result,
                50,
                &mut changed_files_session,
                &mut change_events_session,
                strict_profile_enabled,
                execution_speed,
                constraints_for_turn,
                limits,
                previous_auto_loop_stop_reason.as_deref(),
                &mut file_synopsis_cache,
                &mut confidence_gate_stats,
            );
            result = loop_result;
            turn_cost_sum += extra_cost;
            auto_loop_stop_reason = loop_stop_reason;
            previous_auto_loop_stop_reason = auto_loop_stop_reason.clone();
        }
        if auto_tooling_enabled_for_turn && !is_tool_call && result.stop_reason == "tool_use" {
            println!(
                "{}",
                ui.info(
                    "auto-agent ended with stop_reason=tool_use; likely hit tool-step/failure limit before final prose"
                )
            );
        }
        if let Some(reason) = auto_loop_stop_reason.as_deref() {
            println!("{}", ui.info(&format!("auto_loop_stop_reason={}", reason)));
        }
        if auto_tooling_enabled_for_turn && !is_tool_call {
            let _ = telemetry::log_auto_loop_summary(
                &cfg,
                &rt.provider,
                &rt.model,
                "repl",
                auto_loop_stop_reason.as_deref(),
                confidence_gate_stats.checks(),
                confidence_gate_stats.declaration_missing(),
                confidence_gate_stats.declaration_low(),
                confidence_gate_stats.blocked_risky_toolcalls(),
                confidence_gate_stats.retries_exhausted(),
            );
        }

        if voice_mode
            && !is_tool_call
            && result.stop_reason != "provider_error"
            && !result.is_tool_result
            && !voice_mute
        {
            let spoken = clip_chars(result.text.trim(), 1200);
            if !spoken.trim().is_empty() {
                voice_stats.tts_attempts += 1;
                match voice::speak_text_with_engine(&spoken, voice_engine, &voice_openai_voice) {
                    Ok(()) => {
                        voice_stats.tts_ok += 1;
                        log_voice_event_runtime(&rt, "auto_tts", true, "ok");
                    }
                    Err(e) => {
                        voice_stats.tts_failed += 1;
                        voice_stats.last_error = Some(e.clone());
                        log_voice_event_runtime(&rt, "auto_tts", false, &e);
                        println!("{}", ui.error(&format!("voice speak failed: {}", e)));
                    }
                }
            }
        }
        if let Some(path) = extract_changed_file(&model_input_for_runtime, &result) {
            push_unique_changed_file(&mut changed_files_session, &path);
            let change_src = if is_tool_call { "manual" } else { "work" };
            push_change_event(&mut change_events_session, change_src, &path);
            println!("{}", ui.info(&format!("changed_file={}", path)));
        }
        // Also track native tool call changes
        for path in collect_native_changed_paths(&result.native_tool_calls) {
            push_unique_changed_file(&mut changed_files_session, &path);
            push_change_event(&mut change_events_session, "native", &path);
            println!("{}", ui.info(&format!("changed_file={}", path)));
        }
        let changed_this_turn =
            collect_changed_paths_since_events(&change_events_session, change_events_before_turn);
        let auto_validation_results = run_auto_validation_guards(&changed_this_turn);
        if !auto_validation_results.is_empty() {
            println!(
                "{}",
                ui.info(&format_auto_validation_summary(&auto_validation_results))
            );
            for vr in &auto_validation_results {
                let status = if vr.ok { "ok" } else { "error" };
                let body = format!("command: {}\n{}", vr.command, vr.output);
                println!("{}", ui.tool_panel("auto_validate", status, &body));
            }
        }
        if auto_checkpoint_enabled {
            let meta = session::SessionMeta {
                source: "repl_auto_checkpoint".to_string(),
                agent_enabled: auto_agent_enabled,
                auto_loop_stop_reason: previous_auto_loop_stop_reason.clone(),
                confidence_gate: session::ConfidenceGateSessionStats {
                    checks: confidence_gate_stats.checks(),
                    missing_declaration: confidence_gate_stats.declaration_missing(),
                    low_declaration: confidence_gate_stats.declaration_low(),
                    blocked_risky_toolcalls: confidence_gate_stats.blocked_risky_toolcalls(),
                    retries_exhausted: confidence_gate_stats.retries_exhausted(),
                },
            };
            if let Err(e) = store.save_checkpoint_with_meta(
                &rt.provider,
                &rt.model,
                rt.as_json_messages(),
                Some(meta),
            ) {
                if !auto_checkpoint_error_reported {
                    println!(
                        "{}",
                        ui.error(&format!(
                            "checkpoint_auto_save_failed: {} ({})",
                            e,
                            checkpoint_auto_save_hint(&e)
                        ))
                    );
                    auto_checkpoint_error_reported = true;
                }
            } else {
                auto_checkpoint_error_reported = false;
            }
        }
        if !is_tool_call {
            let elapsed = start.elapsed().as_secs();
            let token_count = stream_tokens.max(result.output_tokens);
            println!("{}", ui.done_line(elapsed, token_count));
        }

        cfg.wallet_usd = (cfg.wallet_usd - turn_cost_sum).max(0.0);

        println!(
            "{}",
            ui.status_line(
                &rt.provider,
                &rt.model,
                &rt.permission_mode,
                rt.turn_count(),
                result.total_input_tokens,
                result.total_output_tokens,
                result.total_cost_usd,
            )
        );
        println!("{}", ui.cost_line(turn_cost_sum, result.total_cost_usd));

        if !is_auto_loop_continuable_stop_reason(&result.stop_reason)
            && result.stop_reason != "tool_result"
        {
            println!(
                "{}",
                ui.info(&format!("stop_reason={}", result.stop_reason))
            );
        }
    }

    sync_voice_config_fields(
        &mut cfg,
        voice_mode,
        voice_engine,
        &voice_openai_voice,
        voice_timeout_secs,
        voice_mute,
        voice_ptt,
        &voice_ptt_trigger,
    );
    let final_meta = session::SessionMeta {
        source: "repl_exit_save".to_string(),
        agent_enabled: auto_agent_enabled,
        auto_loop_stop_reason: previous_auto_loop_stop_reason.clone(),
        confidence_gate: session::ConfidenceGateSessionStats {
            checks: confidence_gate_stats.checks(),
            missing_declaration: confidence_gate_stats.declaration_missing(),
            low_declaration: confidence_gate_stats.declaration_low(),
            blocked_risky_toolcalls: confidence_gate_stats.blocked_risky_toolcalls(),
            retries_exhausted: confidence_gate_stats.retries_exhausted(),
        },
    };
    let saved_path = match store.save_with_meta(
        &rt.provider,
        &rt.model,
        rt.as_json_messages(),
        Some(final_meta),
    ) {
        Ok(path) => Some(path),
        Err(e) => {
            println!("{}", ui.error(&format!("session_save_failed: {}", e)));
            None
        }
    };
    let _ = cfg.save();
    if let Some(path) = saved_path {
        println!("{}", ui.info(&format!("saved={}", path.display())));
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
struct VoiceRuntimeStats {
    listen_attempts: usize,
    listen_ok: usize,
    listen_empty: usize,
    listen_failed: usize,
    tts_attempts: usize,
    tts_ok: usize,
    tts_failed: usize,
    tts_skipped_mute: usize,
    last_error: Option<String>,
}

fn format_voice_stats(stats: &VoiceRuntimeStats) -> String {
    let last = stats
        .last_error
        .as_deref()
        .map(|v| clip_chars(v, 160))
        .unwrap_or_else(|| "none".to_string());
    format!(
        "voice_stats listen(attempts={} ok={} empty={} failed={}) tts(attempts={} ok={} failed={} skipped_mute={}) last_error={}",
        stats.listen_attempts,
        stats.listen_ok,
        stats.listen_empty,
        stats.listen_failed,
        stats.tts_attempts,
        stats.tts_ok,
        stats.tts_failed,
        stats.tts_skipped_mute,
        last
    )
}

fn log_voice_event_runtime(rt: &Runtime, action: &str, ok: bool, detail: &str) {
    let _ = audit::log_voice_event(
        &rt.provider,
        &rt.model,
        &rt.permission_mode,
        action,
        ok,
        detail,
    );
}

fn build_manual_chat_guard(permission_mode: &str) -> String {
    format!(
        "Manual Chat Contract:\nauto_agent=off\npermission_mode={}\nDo not claim tools were executed unless the user explicitly sends /toolcall.\nDo not output pseudo-execution snippets like bash(\"...\") or tool_call = {{...}}.\nFor read/run requests, either:\n1) ask the user to enable /auto on, or\n2) provide exact /toolcall commands for the user to run.",
        permission_mode
    )
}

fn build_chat_no_fabrication_guard(permission_mode: &str) -> String {
    format!(
        "Chat Contract:\nauto_agent=on\npermission_mode={}\nYou are in normal chat mode for this turn.\nDo not fabricate tool execution logs, fake shell output, markdown code blocks that pretend execution, or pseudo-results like [tool:*].\nIf the user asks for coding/file actions, respond briefly and ask for a concrete task; do not emit /toolcall lines unless you actually intend tool execution in this turn.",
        permission_mode
    )
}

fn parse_bool_env(key: &str, default_value: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => true,
            "0" | "false" | "off" | "no" => false,
            _ => default_value,
        },
        Err(_) => default_value,
    }
}

fn parse_usize_env(key: &str, default_value: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default_value)
}

fn normalize_ptt_trigger(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return ".".to_string();
    }
    let out = trimmed.chars().take(32).collect::<String>();
    if out.is_empty() {
        ".".to_string()
    } else {
        out
    }
}
fn sync_voice_config_fields(
    cfg: &mut AppConfig,
    voice_mode: bool,
    voice_engine: voice::VoiceEngine,
    voice_openai_voice: &str,
    voice_timeout_secs: u64,
    voice_mute: bool,
    voice_ptt: bool,
    voice_ptt_trigger: &str,
) {
    cfg.voice_enabled = voice_mode;
    cfg.voice_engine = voice::engine_name(voice_engine).to_string();
    cfg.voice_openai_voice = if voice_openai_voice.trim().is_empty() {
        "alloy".to_string()
    } else {
        voice_openai_voice.trim().to_string()
    };
    cfg.voice_timeout_secs = voice_timeout_secs.clamp(1, 120);
    cfg.voice_mute = voice_mute;
    cfg.voice_ptt = voice_ptt;
    cfg.voice_ptt_trigger = normalize_ptt_trigger(voice_ptt_trigger);
    let hotkey_raw =
        std::env::var("ASI_VOICE_PTT_HOTKEY").unwrap_or_else(|_| cfg.voice_ptt_hotkey.clone());
    cfg.voice_ptt_hotkey = voice::normalize_hotkey_name(&hotkey_raw);
    cfg.voice_openai_fallback_local = voice::openai_fallback_local_enabled();
    cfg.voice_local_soft_fail = voice::local_soft_fail_enabled();
}

fn save_voice_config(
    cfg: &mut AppConfig,
    voice_mode: bool,
    voice_engine: voice::VoiceEngine,
    voice_openai_voice: &str,
    voice_timeout_secs: u64,
    voice_mute: bool,
    voice_ptt: bool,
    voice_ptt_trigger: &str,
) -> Result<String, String> {
    sync_voice_config_fields(
        cfg,
        voice_mode,
        voice_engine,
        voice_openai_voice,
        voice_timeout_secs,
        voice_mute,
        voice_ptt,
        voice_ptt_trigger,
    );
    let path = cfg.save()?;
    Ok(format!("voice_config_saved={}", path.display()))
}

fn provider_from_choice(raw: &str, current: &str) -> String {
    let current = normalize_provider_name(current);
    match raw.trim().to_ascii_lowercase().as_str() {
        "" => current,
        "1" | "openai" => "openai".to_string(),
        "2" | "deepseek" => "deepseek".to_string(),
        "3" | "claude" | "claude-code" | "claude code" => "claude".to_string(),
        _ => current,
    }
}

fn apply_session_api_key(provider: &str, key: &str) {
    match provider {
        "openai" => std::env::set_var("OPENAI_API_KEY", key),
        "deepseek" => std::env::set_var("DEEPSEEK_API_KEY", key),
        "claude" => std::env::set_var("ANTHROPIC_API_KEY", key),
        _ => {}
    }
}

fn is_yes(input: &str) -> bool {
    matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes" | "true" | "1" | "on"
    )
}

fn handle_api_key_input(cfg: &mut AppConfig, api_key: &str) -> Result<(), String> {
    apply_session_api_key(&cfg.provider, api_key);

    let persist = prompt_input("Persist API key to config.json? (yes/no, Enter no)")?;
    if is_yes(&persist) {
        cfg.api_key_provider = Some(cfg.provider.clone());
        cfg.api_key = Some(api_key.to_string());
        apply_api_key_env(cfg);
    } else {
        cfg.api_key_provider = None;
        cfg.api_key = None;
    }

    Ok(())
}
fn provider_api_key_env(provider: &str) -> &'static str {
    match provider {
        "deepseek" => "DEEPSEEK_API_KEY",
        "claude" => "ANTHROPIC_API_KEY",
        _ => "OPENAI_API_KEY",
    }
}

fn run_startup_provider_wizard(
    cfg: &mut AppConfig,
    rt: &mut Runtime,
    _ui: &mut Ui,
) -> Result<String, String> {
    println!("Configure chat provider");
    println!("Select API provider:");
    println!("1) OpenAI");
    println!("2) DeepSeek");
    println!("3) Claude");

    let provider_input =
        prompt_input(&format!("Provider [1-3|name, Enter keep {}]", cfg.provider))?;
    let selected_provider = provider_from_choice(&provider_input, &cfg.provider);
    let provider_changed = selected_provider != cfg.provider;
    cfg.provider = selected_provider;

    if provider_changed {
        cfg.model = resolve_model_alias(default_model(&cfg.provider));
    }

    println!("API key is optional. Leave blank to keep env/current key.");
    let key_env = provider_api_key_env(&cfg.provider);
    let api_key = prompt_secret_input(&format!("Enter {} (optional)", key_env))?;
    if !api_key.is_empty() {
        handle_api_key_input(cfg, &api_key)?;
    }

    let model_input = prompt_input(&format!(
        "Input the default model (Enter keep {})",
        cfg.model
    ))?;
    if !model_input.is_empty() {
        cfg.model = resolve_model_alias(&model_input);
    }

    cfg.provider = normalize_provider_name(&cfg.provider);
    if cfg.model.is_empty() {
        cfg.model = resolve_model_alias(default_model(&cfg.provider));
    }
    let requested_model = cfg.model.clone();
    let (reconciled_model, fallback) =
        reconcile_model_for_provider(&cfg.provider, &requested_model);
    if fallback {
        println!(
            "INFO model {} incompatible with provider {}, fallback={}",
            requested_model, cfg.provider, reconciled_model
        );
    }
    cfg.model = reconciled_model;

    rt.rebind_provider_model(cfg.provider.clone(), cfg.model.clone());
    apply_runtime_flags_from_cfg(rt, cfg);
    let path = cfg.save()?;

    Ok(format!(
        "startup config saved at {} (provider={}, model={})",
        path.display(),
        cfg.provider,
        cfg.model
    ))
}

fn run_setup_wizard(cfg: &mut AppConfig, rt: &mut Runtime, _ui: &mut Ui) -> Result<String, String> {
    println!("Configure custom interface");

    if cfg.wallet_usd <= 0.0 {
        println!("[wallet] No money, no honey. Such poor. Very sad.");
    } else {
        println!("[wallet] {}", cost::format_usd(cfg.wallet_usd));
    }

    println!("Select API provider:");
    println!("1) OpenAI");
    println!("2) DeepSeek");
    println!("3) Claude");
    let provider_input =
        prompt_input(&format!("Provider [1-3|name, Enter keep {}]", cfg.provider))?;
    let selected_provider = provider_from_choice(&provider_input, &cfg.provider);
    let provider_changed = selected_provider != cfg.provider;
    cfg.provider = selected_provider;
    if provider_changed {
        cfg.model = resolve_model_alias(default_model(&cfg.provider));
    }

    let key_env = provider_api_key_env(&cfg.provider);
    let api_key = prompt_secret_input(&format!("Enter {} (optional)", key_env))?;
    if !api_key.is_empty() {
        handle_api_key_input(cfg, &api_key)?;
    }
    println!("Press Enter to save the current item and continue.");

    let default_model_input = prompt_input(&format!(
        "Input the default model (Enter keep {})",
        cfg.model
    ))?;
    if !default_model_input.is_empty() {
        cfg.model = resolve_model_alias(&default_model_input);
    }

    cfg.provider = normalize_provider_name(&cfg.provider);
    if cfg.model.is_empty() {
        cfg.model = resolve_model_alias(default_model(&cfg.provider));
    }
    let requested_model = cfg.model.clone();
    let (reconciled_model, fallback) =
        reconcile_model_for_provider(&cfg.provider, &requested_model);
    if fallback {
        println!(
            "INFO model {} incompatible with provider {}, fallback={}",
            requested_model, cfg.provider, reconciled_model
        );
    }
    cfg.model = reconciled_model;

    rt.rebind_provider_model(cfg.provider.clone(), cfg.model.clone());

    let wallet_input = prompt_input("Input wallet USD (optional)")?;
    if !wallet_input.trim().is_empty() {
        if let Ok(v) = wallet_input.trim().parse::<f64>() {
            cfg.wallet_usd = v.max(0.0);
        }
    }

    let telemetry = prompt_input("Enable telemetry? (on/off, Enter keep)")?;
    if let Some(v) = parse_on_off(&telemetry) {
        cfg.telemetry_enabled = v;
    }

    let undercover =
        prompt_input("Enable undercover mode for git commit messages? (on/off, Enter keep)")?;
    if let Some(v) = parse_on_off(&undercover) {
        cfg.undercover_mode = v;
    }

    apply_runtime_flags_from_cfg(rt, cfg);
    let path = cfg.save()?;
    Ok(format!(
        "setup saved at {} (provider={}, model={})",
        path.display(),
        cfg.provider,
        cfg.model
    ))
}

fn theme_from_index(index: usize) -> Option<&'static str> {
    match index {
        1 => Some("dark"),
        2 => Some("light"),
        3 => Some("dark-colorblind"),
        4 => Some("light-colorblind"),
        5 => Some("dark-ansi"),
        6 => Some("light-ansi"),
        _ => None,
    }
}

fn render_pattern_scan_panel(raw: &str) -> String {
    let project_kind = detect_project_kind();
    let (mut patterns, deep_mode, deep_limit) = parse_scan_request(raw);
    if patterns.is_empty() {
        patterns = default_scan_patterns(project_kind);
    }

    let start = Instant::now();
    let mut lines = Vec::new();
    lines.push("• I will inspect repository entry points first.".to_string());
    lines.push(String::new());
    lines.push(format!("Project profile: {}", project_kind));
    lines.push(format!(
        "Scan mode: {}",
        if deep_mode {
            format!("deep (up to {} matches/pattern)", deep_limit)
        } else {
            "normal (first match per pattern)".to_string()
        }
    ));
    lines.push(format!(
        "Searching for {} patterns… (ctrl+o to expand)",
        patterns.len()
    ));

    let mut found: Vec<(String, Vec<String>)> = Vec::new();
    let mut missing = Vec::new();

    for p in &patterns {
        lines.push(format!("└ \"{}\"", p));

        let mut matched_paths = Vec::new();
        if let Ok(entries) = glob(p) {
            for path in entries.flatten().take(deep_limit) {
                matched_paths.push(path.display().to_string());
            }
        }

        if matched_paths.is_empty() {
            missing.push(p.clone());
        } else {
            found.push((p.clone(), matched_paths));
        }
    }

    let elapsed = start.elapsed().as_secs().max(1);
    lines.push(String::new());
    lines.push(format!(
        "✓ Found {} pattern(s), missing {} (thought for {}s)",
        found.len(),
        missing.len(),
        elapsed
    ));

    if !found.is_empty() {
        lines.push("· Found…".to_string());
        for (pattern, paths) in found.into_iter().take(8) {
            if deep_mode {
                lines.push(format!("  ■ {} ({} match(es))", pattern, paths.len()));
                for path in paths {
                    lines.push(format!("    - {}", path));
                }
            } else if let Some(first) = paths.first() {
                lines.push(format!("  ■ {} -> {}", pattern, first));
            }
        }
    }

    if !missing.is_empty() {
        lines.push("· Missing…".to_string());
        for p in missing {
            lines.push(format!("  □ Inspect repository entry points for {}", p));
        }
    }

    lines.join("\n")
}

fn parse_scan_request(raw: &str) -> (Vec<String>, bool, usize) {
    let mut patterns = Vec::new();
    let mut deep_mode = false;
    let mut deep_limit = 10usize;

    for token in raw.split_whitespace() {
        if token == "--deep" {
            deep_mode = true;
            continue;
        }

        if let Some(v) = token.strip_prefix("--deep=") {
            if let Ok(n) = v.parse::<usize>() {
                deep_mode = true;
                deep_limit = n.clamp(1, 50);
                continue;
            }
        }

        patterns.push(token.to_string());
    }

    if !deep_mode {
        deep_limit = 1;
    }

    (patterns, deep_mode, deep_limit)
}
fn detect_project_kind() -> &'static str {
    let Ok(cwd) = std::env::current_dir() else {
        return "generic";
    };

    if cwd.join("Cargo.toml").exists() {
        return "rust";
    }
    if cwd.join("package.json").exists() {
        return "javascript";
    }
    if cwd.join("pyproject.toml").exists()
        || cwd.join("requirements.txt").exists()
        || cwd.join("setup.py").exists()
    {
        return "python";
    }
    "generic"
}

fn default_scan_patterns(project_kind: &str) -> Vec<String> {
    match project_kind {
        "rust" => vec![
            "**/Cargo.toml".to_string(),
            "**/src/main.rs".to_string(),
            "**/src/lib.rs".to_string(),
            "**/README.md".to_string(),
        ],
        "javascript" => vec![
            "**/package.json".to_string(),
            "**/tsconfig.json".to_string(),
            "**/src/**/*.ts".to_string(),
            "**/src/**/*.js".to_string(),
            "**/README.md".to_string(),
        ],
        "python" => vec![
            "**/pyproject.toml".to_string(),
            "**/requirements.txt".to_string(),
            "**/setup.py".to_string(),
            "**/*.py".to_string(),
            "**/README.md".to_string(),
        ],
        _ => vec![
            "**/README.md".to_string(),
            "**/package.json".to_string(),
            "**/pyproject.toml".to_string(),
            "**/Cargo.toml".to_string(),
        ],
    }
}

fn handle_oauth_command(args: &str) -> Result<String, String> {
    let trimmed = args.trim();
    if let Some(token) = trimmed.strip_prefix("login ") {
        oauth::save_token("claude", token.trim())?;
        return Ok("OAuth token saved for claude".to_string());
    }
    if trimmed == "logout" {
        let removed = oauth::clear_token("claude")?;
        return Ok(format!("OAuth token removed={}", removed));
    }
    Ok("Usage: /oauth login <token> | /oauth logout".to_string())
}

fn handle_todo_command(args: &str) -> Result<String, String> {
    let trimmed = args.trim();
    if let Some(text) = trimmed.strip_prefix("add ") {
        let item = todo::add(text)?;
        return Ok(format!("todo added #{} {}", item.id, item.text));
    }
    if let Some(id) = trimmed.strip_prefix("done ") {
        let n = id.trim().parse::<u64>().map_err(|e| e.to_string())?;
        let ok = todo::mark_done(n)?;
        return Ok(format!("todo done #{} changed={}", n, ok));
    }
    if let Some(id) = trimmed.strip_prefix("rm ") {
        let n = id.trim().parse::<u64>().map_err(|e| e.to_string())?;
        let ok = todo::remove(n)?;
        return Ok(format!("todo removed #{} changed={}", n, ok));
    }

    let items = todo::list();
    if items.is_empty() {
        return Ok("No todos".to_string());
    }
    let lines = items
        .into_iter()
        .map(|x| {
            format!(
                "- [{}] #{} {}",
                if x.done { "x" } else { " " },
                x.id,
                x.text
            )
        })
        .collect::<Vec<_>>();
    Ok(lines.join("\n"))
}

fn handle_memory_command(args: &str) -> Result<String, String> {
    let trimmed = args.trim();
    if let Some(note) = trimmed.strip_prefix("add ") {
        let path = memory::append_memory(note)?;
        return Ok(format!("memory updated: {}", path.display()));
    }
    memory::read_memory()
}

fn handle_git_command(args: &str, undercover_mode: bool) -> Result<String, String> {
    git_tools::handle_git_command(args.trim_start(), undercover_mode)
}

fn normalize_publish_meta_in_config_args(raw: &str) -> String {
    let tokens = parse_cli_tokens(raw).unwrap_or_else(|_| {
        raw.split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    });
    if tokens.is_empty() {
        return raw.to_string();
    }
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < tokens.len() {
        let token = tokens[i].clone();
        out.push(token.clone());
        if token.eq_ignore_ascii_case("config") {
            let name = tokens.get(i + 1).cloned();
            let key = tokens.get(i + 2).cloned();
            if let (Some(n), Some(k)) = (name, key) {
                let key_norm = k.trim().to_ascii_lowercase();
                if key_norm == "source" || key_norm == "version" || key_norm == "signature" {
                    out.push(n);
                    out.push(k.clone());
                    let mut j = i + 3;
                    let mut value_parts = Vec::new();
                    while j < tokens.len() {
                        if tokens[j].starts_with("--") {
                            break;
                        }
                        value_parts.push(tokens[j].clone());
                        j += 1;
                    }
                    let value = value_parts.join(" ");
                    if !value.is_empty() {
                        out.push(value);
                    }
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }
    out.join(" ")
}

fn parse_mcp_json_mode(raw: &str) -> (String, bool) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (String::new(), false);
    }

    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.is_empty() {
        return (String::new(), false);
    }

    if tokens.len() >= 2 && tokens[1].eq_ignore_ascii_case("--json") {
        if tokens.len() == 2 {
            return (normalize_publish_meta_in_config_args(tokens[0]), true);
        }
        let body = format!("{} {}", tokens[0], tokens[2..].join(" "));
        return (
            normalize_publish_meta_in_config_args(&body),
            true,
        );
    }

    if tokens[tokens.len() - 1].eq_ignore_ascii_case("--json") {
        if tokens.len() == 1 {
            return (normalize_publish_meta_in_config_args(tokens[0]), true);
        }
        let body = format!("{} {}", tokens[0], tokens[1..tokens.len() - 1].join(" "));
        return (
            normalize_publish_meta_in_config_args(&body),
            true,
        );
    }

    if tokens[0].eq_ignore_ascii_case("list") {
        if tokens.len() == 2 && tokens[1].eq_ignore_ascii_case("--json") {
            return ("list".to_string(), true);
        }
        return (normalize_publish_meta_in_config_args(trimmed), false);
    }

    if tokens[0].eq_ignore_ascii_case("show") {
        if tokens.len() >= 2 && tokens[1].eq_ignore_ascii_case("--json") {
            if tokens.len() == 2 {
                return ("show".to_string(), true);
            }
            return (format!("show {}", tokens[2..].join(" ")), true);
        }
        if tokens.len() >= 2
            && tokens[tokens.len() - 1].eq_ignore_ascii_case("--json")
        {
            if tokens.len() == 2 {
                return ("show".to_string(), true);
            }
            return (
                normalize_publish_meta_in_config_args(&format!(
                    "show {}",
                    tokens[1..tokens.len() - 1].join(" ")
                )),
                true,
            );
        }
        return (normalize_publish_meta_in_config_args(trimmed), false);
    }

    (normalize_publish_meta_in_config_args(trimmed), false)
}

fn mcp_record_json(mut item: mcp::McpServerRecord) -> serde_json::Value {
    let mut config_pairs: Vec<(String, String)> = item.config.drain().collect();
    config_pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let config = config_pairs
        .into_iter()
        .map(|(k, v)| serde_json::json!({ "key": k, "value": v }))
        .collect::<Vec<_>>();
    serde_json::json!({
        "name": item.name,
        "status": item.status,
        "pid": item.pid,
        "command": item.command,
        "scope": item.scope,
        "trusted": item.trusted,
        "source": item.source,
        "version": item.version,
        "signature": item.signature,
        "auth_type": item.auth_type,
        "auth_value": item.auth_value,
        "config": config,
    })
}

fn mcp_json_response(command: &str, mcp: serde_json::Value) -> String {
    serde_json::json!({
        "schema_version": MCP_JSON_SCHEMA_VERSION,
        "command": command,
        "mcp": mcp,
    })
    .to_string()
}

fn parse_mcp_oauth_provider(raw: &str) -> String {
    let p = raw.trim().to_ascii_lowercase();
    if p.is_empty() {
        "generic".to_string()
    } else {
        p
    }
}

fn parse_mcp_scope_value(raw: &str) -> Option<String> {
    let scope = raw.trim().to_ascii_lowercase();
    if matches!(scope.as_str(), "session" | "project" | "global") {
        Some(scope)
    } else {
        None
    }
}

fn parse_mcp_oauth_scope_flag(tokens: &mut Vec<&str>) -> Result<Option<String>, String> {
    let mut scope: Option<String> = None;
    let mut idx = 0usize;
    while idx < tokens.len() {
        if tokens[idx].eq_ignore_ascii_case("--scope") {
            let value = tokens
                .get(idx + 1)
                .ok_or_else(|| "scope must be one of: session|project|global".to_string())?;
            let parsed = parse_mcp_scope_value(value)
                .ok_or_else(|| "scope must be one of: session|project|global".to_string())?;
            scope = Some(parsed);
            tokens.drain(idx..=idx + 1);
            continue;
        }
        idx += 1;
    }
    Ok(scope)
}

fn resolve_mcp_oauth_scope_for_server(name: &str, explicit_scope: Option<&str>) -> String {
    if let Some(s) = explicit_scope {
        if let Some(scope) = parse_mcp_scope_value(s) {
            return scope;
        }
    }
    if let Some(server) = mcp::get_server(name) {
        if let Some(scope) = parse_mcp_scope_value(&server.scope) {
            return scope;
        }
    }
    "project".to_string()
}

fn mcp_oauth_json_status_payload(
    name: &str,
    provider: &str,
    scope_hint: Option<&str>,
) -> serde_json::Value {
    let request_scope = scope_hint
        .and_then(parse_mcp_scope_value)
        .unwrap_or_else(|| "auto".to_string());
    let (token_present, token_scope) = if request_scope == "auto" {
        let scoped = oauth::load_mcp_token_with_scope(name, provider);
        (
            scoped.is_some(),
            scoped
                .as_ref()
                .map(|(_, scope)| scope.to_string())
                .unwrap_or_else(|| "none".to_string()),
        )
    } else {
        let present = oauth::load_mcp_token_scoped(name, provider, &request_scope).is_some();
        (
            present,
            if present {
                request_scope.clone()
            } else {
                "none".to_string()
            },
        )
    };
    let providers = if request_scope == "auto" {
        oauth::list_mcp_token_providers(name)
    } else {
        oauth::list_mcp_token_providers_scoped(name, &request_scope).unwrap_or_default()
    };
    serde_json::json!({
        "name": name,
        "provider": provider,
        "token_present": token_present,
        "token_scope": token_scope,
        "request_scope": request_scope,
        "stored_providers": providers,
    })
}

pub(crate) fn handle_mcp_command(args: &str) -> Result<String, String> {
    let (trimmed, json_mode) = parse_mcp_json_mode(&normalize_publish_meta_in_config_args(args.trim()));
    let trimmed = trimmed.as_str();
    if trimmed == "list" || trimmed.is_empty() {
        let list = mcp::list_servers();
        if json_mode {
            let items = list.into_iter().map(mcp_record_json).collect::<Vec<_>>();
            let trusted_count = items
                .iter()
                .filter(|v| v.get("trusted").and_then(|x| x.as_bool()).unwrap_or(false))
                .count();
            let untrusted_count = items.len().saturating_sub(trusted_count);
            let mut scope_session = 0usize;
            let mut scope_project = 0usize;
            let mut scope_global = 0usize;
            let mut scope_other = 0usize;
            for item in &items {
                match item
                    .get("scope")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "session" => scope_session += 1,
                    "project" => scope_project += 1,
                    "global" => scope_global += 1,
                    _ => scope_other += 1,
                }
            }
            return Ok(mcp_json_response(
                "mcp_list",
                serde_json::json!({
                    "count": items.len(),
                    "diagnostics": {
                        "trusted_count": trusted_count,
                        "untrusted_count": untrusted_count,
                        "scope_counts": {
                            "session": scope_session,
                            "project": scope_project,
                            "global": scope_global,
                            "other": scope_other,
                        },
                    },
                    "items": items,
                }),
            ));
        }
        if list.is_empty() {
            return Ok("No MCP servers".to_string());
        }
        let lines = list
            .into_iter()
            .map(|x| {
                format!(
                    "- {} status={} pid={:?} scope={} trusted={} auth={} cmd={} config_keys={}",
                    x.name,
                    x.status,
                    x.pid,
                    x.scope,
                    x.trusted,
                    x.auth_type,
                    x.command,
                    x.config.len()
                )
            })
            .collect::<Vec<_>>();
        return Ok(lines.join("\n"));
    }
    if let Some(rest) = trimmed.strip_prefix("add ") {
        let mut tokens: Vec<&str> = rest.split_whitespace().collect();
        let mut scope = "session".to_string();
        let mut idx = 0usize;
        while idx < tokens.len() {
            if tokens[idx].eq_ignore_ascii_case("--scope") {
                if idx + 1 >= tokens.len() {
                    return Ok("Usage: /mcp add <name> <command> [--scope <session|project|global>]".to_string());
                }
                scope = tokens[idx + 1].trim().to_ascii_lowercase();
                tokens.drain(idx..=idx + 1);
                continue;
            }
            idx += 1;
        }
        if !matches!(scope.as_str(), "session" | "project" | "global") {
            return Ok("Usage: /mcp add <name> <command> [--scope <session|project|global>]".to_string());
        }
        if tokens.len() < 2 {
            return Ok("Usage: /mcp add <name> <command> [--scope <session|project|global>]".to_string());
        }
        let name = tokens[0].trim();
        let cmd = tokens[1..].join(" ");
        let cmd = cmd.trim();
        if name.is_empty() || cmd.is_empty() {
            return Ok("Usage: /mcp add <name> <command> [--scope <session|project|global>]".to_string());
        }
        let item = mcp::add_server_with_scope(name, cmd, &scope)?;
        if json_mode {
            return Ok(mcp_json_response(
                "mcp_add",
                serde_json::json!({
                    "added": true,
                    "scope": scope,
                    "server": mcp_record_json(item),
                }),
            ));
        }
        return Ok(format!(
            "added {} scope={} cmd={}",
            item.name, item.scope, item.command
        ));
    }
    if let Some(path) = trimmed.strip_prefix("export ") {
        let out = path.trim();
        if out.is_empty() {
            return Ok("Usage: /mcp export <path.json>".to_string());
        }
        let saved = mcp::export_state(out)?;
        if json_mode {
            return Ok(mcp_json_response(
                "mcp_export",
                serde_json::json!({
                    "path": saved,
                }),
            ));
        }
        return Ok(format!("mcp exported path={}", saved));
    }
    if let Some(rest) = trimmed.strip_prefix("import ") {
        let mut it = rest.split_whitespace();
        let path = it.next().unwrap_or("").trim();
        let mode = it.next().unwrap_or("replace").trim().to_ascii_lowercase();
        if path.is_empty() {
            return Ok("Usage: /mcp import <path.json> [merge|replace]".to_string());
        }
        let merge = matches!(mode.as_str(), "merge");
        let total = mcp::import_state(path, merge)?;
        if json_mode {
            return Ok(mcp_json_response(
                "mcp_import",
                serde_json::json!({
                    "path": path,
                    "mode": if merge { "merge" } else { "replace" },
                    "total_servers": total,
                }),
            ));
        }
        return Ok(format!(
            "mcp imported path={} mode={} total_servers={}",
            path,
            if merge { "merge" } else { "replace" },
            total
        ));
    }
    if let Some(name) = trimmed.strip_prefix("rm ") {
        let id = name.trim();
        if id.is_empty() {
            return Ok("Usage: /mcp rm <name>".to_string());
        }
        let changed = mcp::remove_server(id)?;
        if json_mode {
            return Ok(mcp_json_response(
                "mcp_rm",
                serde_json::json!({
                    "name": id,
                    "changed": changed,
                }),
            ));
        }
        return Ok(format!("removed {} changed={}", id, changed));
    }
    if let Some(name) = trimmed.strip_prefix("show ") {
        let id = name.trim();
        if id.is_empty() {
            return Ok("Usage: /mcp show <name>".to_string());
        }
        if let Some(item) = mcp::get_server(id) {
            if json_mode {
                let provider = item
                    .config
                    .get("oauth_provider")
                    .map(|v| v.trim().to_ascii_lowercase())
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "generic".to_string());
                let oauth_token_present = oauth::load_mcp_token(&item.name, &provider).is_some();
                let mut payload = mcp_record_json(item.clone());
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert(
                        "diagnostics".to_string(),
                        serde_json::json!({
                            "has_auth_value": item.auth_value.as_ref().map(|v| !v.trim().is_empty()).unwrap_or(false),
                            "config_key_count": item.config.len(),
                            "scope": item.scope,
                            "trusted": item.trusted,
                            "oauth_provider": provider,
                            "oauth_token_present": oauth_token_present,
                        }),
                    );
                }
                return Ok(mcp_json_response("mcp_show", payload));
            }
            let mut lines = vec![
                format!("name={}", item.name),
                format!("status={}", item.status),
                format!("pid={:?}", item.pid),
                format!("command={}", item.command),
                format!("scope={}", item.scope),
                format!("trusted={}", item.trusted),
                format!("auth_type={}", item.auth_type),
                format!(
                    "auth_value={}",
                    item.auth_value.unwrap_or_else(|| "-".to_string())
                ),
            ];
            if item.config.is_empty() {
                lines.push("config: <empty>".to_string());
            } else {
                lines.push("config:".to_string());
                let mut pairs: Vec<(String, String)> = item.config.into_iter().collect();
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
                for (k, v) in pairs {
                    lines.push(format!("- {}={}", k, v));
                }
            }
            return Ok(lines.join("\n"));
        }
        if json_mode {
            return Ok(mcp_json_response(
                "mcp_show",
                serde_json::json!({
                    "name": id,
                    "status": "unknown",
                }),
            ));
        }
        return Ok(format!("MCP server `{}` not found", id));
    }
    if let Some(rest) = trimmed.strip_prefix("auth ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() < 2 {
            return Ok("Usage: /mcp auth <name> <none|bearer|api-key|basic> [value]".to_string());
        }
        let name = parts[0].trim();
        let auth_type = parts[1].trim();
        let auth_value = if parts.len() >= 3 {
            Some(parts[2..].join(" "))
        } else {
            None
        };
        mcp::set_server_auth(name, auth_type, auth_value.as_deref())?;
        if json_mode {
            return Ok(mcp_json_response(
                "mcp_auth",
                serde_json::json!({
                    "name": name,
                    "auth_type": auth_type,
                    "value_set": auth_value.is_some(),
                }),
            ));
        }
        return Ok(format!(
            "mcp auth updated name={} type={} value_set={}",
            name,
            auth_type,
            auth_value.is_some()
        ));
    }
    if let Some(rest) = trimmed.strip_prefix("oauth ") {
        let mut tokens: Vec<&str> = rest.split_whitespace().collect();
        if tokens.is_empty() {
            return Ok("Usage: /mcp oauth login <name> <provider> <token> [--scope <session|project|global>] [--link-auth] [--json] | /mcp oauth status <name> [provider] [--scope <session|project|global>] [--json] | /mcp oauth logout <name> [provider] [--scope <session|project|global>] [--json]".to_string());
        }
        let action = tokens[0].trim().to_ascii_lowercase();
        let scope = match parse_mcp_oauth_scope_flag(&mut tokens) {
            Ok(v) => v,
            Err(_) => {
                return Ok("Usage: /mcp oauth login <name> <provider> <token> [--scope <session|project|global>] [--link-auth] [--json] | /mcp oauth status <name> [provider] [--scope <session|project|global>] [--json] | /mcp oauth logout <name> [provider] [--scope <session|project|global>] [--json]".to_string());
            }
        };
        if action == "login" {
            if tokens.len() < 4 {
                return Ok("Usage: /mcp oauth login <name> <provider> <token> [--scope <session|project|global>] [--link-auth]".to_string());
            }
            let name = tokens[1].trim();
            let provider = parse_mcp_oauth_provider(tokens[2]);
            let token = tokens[3].trim();
            let link_auth = tokens[4..]
                .iter()
                .any(|t| t.trim().eq_ignore_ascii_case("--link-auth"));
            if name.is_empty() || token.is_empty() {
                return Ok("Usage: /mcp oauth login <name> <provider> <token> [--scope <session|project|global>] [--link-auth]".to_string());
            }
            let resolved_scope = resolve_mcp_oauth_scope_for_server(name, scope.as_deref());
            if resolved_scope == "project" {
                oauth::save_mcp_token(name, &provider, token)?;
            } else {
                oauth::save_mcp_token_scoped(name, &provider, token, &resolved_scope)?;
            }
            if link_auth {
                mcp::set_server_auth(name, "bearer", Some(token))?;
            }
            if json_mode {
                return Ok(mcp_json_response(
                    "mcp_oauth_login",
                    serde_json::json!({
                        "name": name,
                        "provider": provider,
                        "scope": resolved_scope,
                        "saved": true,
                        "linked_auth": link_auth,
                    }),
                ));
            }
            return Ok(format!(
                "mcp oauth saved name={} provider={} scope={} linked_auth={}",
                name, provider, resolved_scope, link_auth
            ));
        }
        if action == "status" {
            if tokens.len() < 2 {
                return Ok("Usage: /mcp oauth status <name> [provider] [--scope <session|project|global>]".to_string());
            }
            let name = tokens[1].trim();
            if name.is_empty() {
                return Ok("Usage: /mcp oauth status <name> [provider] [--scope <session|project|global>]".to_string());
            }
            let provider = if tokens.len() >= 3 {
                parse_mcp_oauth_provider(tokens[2])
            } else {
                "generic".to_string()
            };
            let payload = mcp_oauth_json_status_payload(name, &provider, scope.as_deref());
            if json_mode {
                return Ok(mcp_json_response("mcp_oauth_status", payload));
            }
            let present = payload
                .get("token_present")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let token_scope = payload
                .get("token_scope")
                .and_then(|v| v.as_str())
                .unwrap_or("none");
            let providers = payload
                .get("stored_providers")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            return Ok(format!(
                "mcp oauth status name={} provider={} token_present={} token_scope={} stored_providers={}",
                name,
                provider,
                present,
                token_scope,
                if providers.is_empty() {
                    "-".to_string()
                } else {
                    providers.join(",")
                }
            ));
        }
        if action == "logout" {
            if tokens.len() < 2 {
                return Ok("Usage: /mcp oauth logout <name> [provider] [--scope <session|project|global>]".to_string());
            }
            let name = tokens[1].trim();
            if name.is_empty() {
                return Ok("Usage: /mcp oauth logout <name> [provider] [--scope <session|project|global>]".to_string());
            }
            let provider = if tokens.len() >= 3 {
                parse_mcp_oauth_provider(tokens[2])
            } else {
                "generic".to_string()
            };
            let removed = if let Some(scope_value) = scope.as_deref() {
                let resolved_scope = resolve_mcp_oauth_scope_for_server(name, Some(scope_value));
                oauth::clear_mcp_token_scoped(name, &provider, &resolved_scope)?
            } else {
                oauth::clear_mcp_token(name, &provider)?
            };
            if json_mode {
                return Ok(mcp_json_response(
                    "mcp_oauth_logout",
                    serde_json::json!({
                        "name": name,
                        "provider": provider,
                        "scope": scope.unwrap_or_else(|| "all".to_string()),
                        "removed": removed,
                    }),
                ));
            }
            return Ok(format!(
                "mcp oauth removed name={} provider={} scope={} removed={}",
                name,
                provider,
                scope.unwrap_or_else(|| "all".to_string()),
                removed
            ));
        }
        return Ok("Usage: /mcp oauth login <name> <provider> <token> [--scope <session|project|global>] [--link-auth] [--json] | /mcp oauth status <name> [provider] [--scope <session|project|global>] [--json] | /mcp oauth logout <name> [provider] [--scope <session|project|global>] [--json]".to_string());
    }
    if let Some(rest) = trimmed.strip_prefix("config ") {
        if let Some(tail) = rest.strip_prefix("scope ") {
            let mut it = tail.split_whitespace();
            let name = it.next().unwrap_or("").trim();
            let scope = it.next().unwrap_or("").trim();
            if name.is_empty() || scope.is_empty() {
                return Ok("Usage: /mcp config scope <name> <session|project|global>".to_string());
            }
            mcp::set_server_scope(name, scope)?;
            if json_mode {
                return Ok(mcp_json_response(
                    "mcp_config_scope",
                    serde_json::json!({
                        "name": name,
                        "scope": scope.to_ascii_lowercase(),
                    }),
                ));
            }
            return Ok(format!("mcp config scope updated name={} scope={}", name, scope));
        }
        if let Some(tail) = rest.strip_prefix("trust ") {
            let mut it = tail.split_whitespace();
            let name = it.next().unwrap_or("").trim();
            let value = it.next().unwrap_or("").trim().to_ascii_lowercase();
            if name.is_empty() || value.is_empty() {
                return Ok("Usage: /mcp config trust <name> <on|off>".to_string());
            }
            let trusted = parse_on_off(&value)
                .ok_or_else(|| "Usage: /mcp config trust <name> <on|off>".to_string())?;
            mcp::set_server_trusted(name, trusted)?;
            if json_mode {
                return Ok(mcp_json_response(
                    "mcp_config_trust",
                    serde_json::json!({
                        "name": name,
                        "trusted": trusted,
                    }),
                ));
            }
            return Ok(format!("mcp config trust updated name={} trusted={}", name, trusted));
        }
        if let Some(tail) = rest.strip_prefix("rm ") {
            let mut it = tail.splitn(2, ' ');
            let name = it.next().unwrap_or("").trim();
            let key = it.next().unwrap_or("").trim();
            if name.is_empty() || key.is_empty() {
                return Ok("Usage: /mcp config rm <name> <key>".to_string());
            }
            let changed = mcp::remove_server_config(name, key)?;
            if json_mode {
                return Ok(mcp_json_response(
                    "mcp_config_rm",
                    serde_json::json!({
                        "name": name,
                        "key": key,
                        "changed": changed,
                    }),
                ));
            }
            return Ok(format!("mcp config removed name={} key={} changed={}", name, key, changed));
        }
        let mut parts = rest.split_whitespace();
        let name = parts.next().unwrap_or("").trim();
        let key = parts.next().unwrap_or("").trim();
        let value = parts.collect::<Vec<_>>().join(" ");
        if name.is_empty() || key.is_empty() || value.trim().is_empty() {
            return Ok("Usage: /mcp config <name> <key> <value> | /mcp config rm <name> <key> | /mcp config scope <name> <session|project|global> | /mcp config trust <name> <on|off>".to_string());
        }
        mcp::set_server_config(name, key, value.trim())?;
        if json_mode {
            return Ok(mcp_json_response(
                "mcp_config_set",
                serde_json::json!({
                    "name": name,
                    "key": key,
                }),
            ));
        }
        return Ok(format!("mcp config updated name={} key={}", name, key));
    }
    if let Some(rest) = trimmed.strip_prefix("start ") {
        let mut it = rest.splitn(2, ' ');
        let name = it.next().unwrap_or("").trim();
        let cmd = it.next().unwrap_or("").trim();
        if name.is_empty() {
            return Ok("Usage: /mcp start <name> [command]".to_string());
        }
        let start = mcp::start_server(name, if cmd.is_empty() { None } else { Some(cmd) });
        let item = match start {
            Ok(v) => v,
            Err(e) => {
                if json_mode {
                    return Ok(mcp_json_response(
                        "mcp_start",
                        serde_json::json!({
                            "started": false,
                            "name": name,
                            "error": e,
                        }),
                    ));
                }
                return Err(e);
            }
        };
        if json_mode {
            return Ok(mcp_json_response(
                "mcp_start",
                serde_json::json!({
                    "started": true,
                    "server": mcp_record_json(item),
                }),
            ));
        }
        return Ok(format!("started {} pid={:?}", item.name, item.pid));
    }
    if let Some(name) = trimmed.strip_prefix("stop ") {
        let ok = mcp::stop_server(name.trim())?;
        if json_mode {
            return Ok(mcp_json_response(
                "mcp_stop",
                serde_json::json!({
                    "name": name.trim(),
                    "changed": ok,
                }),
            ));
        }
        return Ok(format!("stopped {} changed={}", name.trim(), ok));
    }
    Ok("Usage: /mcp list [--json] | /mcp add <name> <command> [--scope <session|project|global>] [--json] | /mcp rm <name> [--json] | /mcp show <name> [--json] | /mcp auth <name> <none|bearer|api-key|basic> [value] [--json] | /mcp oauth login <name> <provider> <token> [--scope <session|project|global>] [--link-auth] [--json] | /mcp oauth status <name> [provider] [--scope <session|project|global>] [--json] | /mcp oauth logout <name> [provider] [--scope <session|project|global>] [--json] | /mcp config <name> <key> <value> [--json] | /mcp config rm <name> <key> [--json] | /mcp config scope <name> <session|project|global> [--json] | /mcp config trust <name> <on|off> [--json] | /mcp export <path.json> [--json] | /mcp import <path.json> [merge|replace] [--json] | /mcp start <name> [command] [--json] | /mcp stop <name> [--json]".to_string())
}

fn handle_notebook_command(args: &str) -> Result<String, String> {
    let trimmed = args.trim();
    if let Some(rest) = trimmed.strip_prefix("add ") {
        let mut it = rest.splitn(2, ' ');
        let path = it.next().unwrap_or("").trim();
        let text = it.next().unwrap_or("").trim();
        if path.is_empty() || text.is_empty() {
            return Ok("Usage: /notebook add <path.ipynb> <markdown text>".to_string());
        }
        return notebook::add_markdown_cell(path, text);
    }
    if let Some(path) = trimmed.strip_prefix("list ") {
        return notebook::list_cells(path.trim());
    }
    Ok(
        "Usage: /notebook list <path.ipynb> | /notebook add <path.ipynb> <markdown text>"
            .to_string(),
    )
}

fn autoresearch_repl_usage() -> &'static str {
    "Usage: /autoresearch help | /autoresearch doctor [--repo <path>|<path>] | /autoresearch init [--repo <path>|<path>] | /autoresearch run [--repo <path>|<path>] [--iterations <n>] [--timeout-secs <sec>] [--log <path>] [--description <text>] [--status <keep|discard|crash>] (repo defaults to current project)"
}

fn handle_autoresearch_repl_command(args: &str) -> Result<String, String> {
    let tokens = parse_cli_tokens(args)?;
    if tokens.is_empty() {
        return Ok(autoresearch_repl_usage().to_string());
    }

    let sub = tokens[0].as_str();
    let rest = &tokens[1..];
    match sub {
        "help" => Ok(autoresearch_repl_usage().to_string()),
        "doctor" => {
            let repo = parse_autoresearch_repo_arg(rest, "doctor")?;
            autoresearch::doctor(&repo)
        }
        "init" => {
            let repo = parse_autoresearch_repo_arg(rest, "init")?;
            autoresearch::init_repo(&repo)
        }
        "run" => {
            let opts = parse_autoresearch_run_opts(rest)?;
            autoresearch::run_experiments(opts)
        }
        _ => Ok(autoresearch_repl_usage().to_string()),
    }
}

fn parse_autoresearch_repo_arg(tokens: &[String], subcommand: &str) -> Result<PathBuf, String> {
    let mut repo: Option<PathBuf> = None;
    let mut i = 0usize;
    while i < tokens.len() {
        let token = tokens[i].as_str();
        if token == "--repo" {
            i += 1;
            let value = tokens.get(i).ok_or_else(|| {
                format!("missing value for --repo\n{}", autoresearch_repl_usage())
            })?;
            repo = Some(PathBuf::from(value));
            i += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix("--repo=") {
            repo = Some(PathBuf::from(value));
            i += 1;
            continue;
        }
        if token.starts_with('-') {
            return Err(format!(
                "unknown flag for {}: {}\n{}",
                subcommand,
                token,
                autoresearch_repl_usage()
            ));
        }
        if repo.is_some() {
            return Err(format!(
                "unexpected token for {}: {}\n{}",
                subcommand,
                token,
                autoresearch_repl_usage()
            ));
        }
        repo = Some(PathBuf::from(token));
        i += 1;
    }

    if let Some(path) = repo {
        Ok(path)
    } else {
        std::env::current_dir().map_err(|e| e.to_string())
    }
}

fn parse_autoresearch_run_opts(tokens: &[String]) -> Result<autoresearch::RunOptions, String> {
    let mut repo: Option<PathBuf> = None;
    let mut iterations = 1usize;
    let mut timeout_secs = 600u64;
    let mut log_path: Option<PathBuf> = None;
    let mut description: Option<String> = None;
    let mut status: Option<String> = None;

    let mut i = 0usize;
    while i < tokens.len() {
        let token = tokens[i].as_str();
        match token {
            "--repo" => {
                i += 1;
                let value = tokens.get(i).ok_or_else(|| {
                    format!("missing value for --repo\n{}", autoresearch_repl_usage())
                })?;
                repo = Some(PathBuf::from(value));
                i += 1;
                continue;
            }
            "--iterations" => {
                i += 1;
                let value = tokens.get(i).ok_or_else(|| {
                    format!(
                        "missing value for --iterations\n{}",
                        autoresearch_repl_usage()
                    )
                })?;
                iterations = value.parse::<usize>().map_err(|_| {
                    format!(
                        "invalid --iterations value: {}\n{}",
                        value,
                        autoresearch_repl_usage()
                    )
                })?;
                i += 1;
                continue;
            }
            "--timeout-secs" => {
                i += 1;
                let value = tokens.get(i).ok_or_else(|| {
                    format!(
                        "missing value for --timeout-secs\n{}",
                        autoresearch_repl_usage()
                    )
                })?;
                timeout_secs = value.parse::<u64>().map_err(|_| {
                    format!(
                        "invalid --timeout-secs value: {}\n{}",
                        value,
                        autoresearch_repl_usage()
                    )
                })?;
                i += 1;
                continue;
            }
            "--log" => {
                i += 1;
                let value = tokens.get(i).ok_or_else(|| {
                    format!("missing value for --log\n{}", autoresearch_repl_usage())
                })?;
                log_path = Some(PathBuf::from(value));
                i += 1;
                continue;
            }
            "--description" => {
                i += 1;
                let value = tokens.get(i).ok_or_else(|| {
                    format!(
                        "missing value for --description\n{}",
                        autoresearch_repl_usage()
                    )
                })?;
                description = Some(value.to_string());
                i += 1;
                continue;
            }
            "--status" => {
                i += 1;
                let value = tokens.get(i).ok_or_else(|| {
                    format!("missing value for --status\n{}", autoresearch_repl_usage())
                })?;
                status = Some(value.to_string());
                i += 1;
                continue;
            }
            _ => {}
        }

        if let Some(value) = token.strip_prefix("--repo=") {
            repo = Some(PathBuf::from(value));
            i += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix("--iterations=") {
            iterations = value.parse::<usize>().map_err(|_| {
                format!(
                    "invalid --iterations value: {}\n{}",
                    value,
                    autoresearch_repl_usage()
                )
            })?;
            i += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix("--timeout-secs=") {
            timeout_secs = value.parse::<u64>().map_err(|_| {
                format!(
                    "invalid --timeout-secs value: {}\n{}",
                    value,
                    autoresearch_repl_usage()
                )
            })?;
            i += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix("--log=") {
            log_path = Some(PathBuf::from(value));
            i += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix("--description=") {
            description = Some(value.to_string());
            i += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix("--status=") {
            status = Some(value.to_string());
            i += 1;
            continue;
        }

        if token.starts_with('-') {
            return Err(format!(
                "unknown flag for run: {}\n{}",
                token,
                autoresearch_repl_usage()
            ));
        }

        if repo.is_none() {
            repo = Some(PathBuf::from(token));
            i += 1;
            continue;
        }

        return Err(format!(
            "unexpected token for run: {}\n{}",
            token,
            autoresearch_repl_usage()
        ));
    }

    let repo = if let Some(path) = repo {
        path
    } else {
        std::env::current_dir().map_err(|e| e.to_string())?
    };

    Ok(autoresearch::RunOptions {
        repo,
        iterations,
        timeout_secs,
        log_path,
        description,
        status,
    })
}

fn parse_cli_tokens(input: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escape_in_double = false;
    let mut chars = input.trim().chars().peekable();

    while let Some(ch) = chars.next() {
        if let Some(q) = quote {
            if q == '"' {
                if escape_in_double {
                    current.push(ch);
                    escape_in_double = false;
                    continue;
                }
                if ch == '\\' {
                    match chars.peek().copied() {
                        Some('"') | Some('\\') => {
                            escape_in_double = true;
                            continue;
                        }
                        _ => {
                            current.push('\\');
                            continue;
                        }
                    }
                }
                if ch == '"' {
                    quote = None;
                } else {
                    current.push(ch);
                }
                continue;
            }

            if ch == q {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }

        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                out.push(current.clone());
                current.clear();
            }
            continue;
        }
        current.push(ch);
    }

    if escape_in_double {
        return Err("unterminated escape in quoted argument".to_string());
    }
    if quote.is_some() {
        return Err("unterminated quoted argument".to_string());
    }
    if !current.is_empty() {
        out.push(current);
    }
    Ok(out)
}
fn handle_audit_command(args: &str) -> Result<String, String> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "tail" {
        return audit::read_tail(20);
    }

    if let Some(v) = trimmed.strip_prefix("tail ") {
        let n = if v.trim().is_empty() {
            20
        } else {
            v.trim()
                .parse::<usize>()
                .map_err(|_| audit_usage().to_string())?
        };
        return audit::read_tail(n);
    }

    if trimmed == "stats" {
        let summary = audit::summarize_recent(50)?;
        return Ok(format_audit_summary(&summary, 50));
    }
    if let Some(v) = trimmed.strip_prefix("stats ") {
        let n = if v.trim().is_empty() {
            50
        } else {
            v.trim()
                .parse::<usize>()
                .map_err(|_| audit_usage().to_string())?
        };
        let summary = audit::summarize_recent(n)?;
        return Ok(format_audit_summary(&summary, n));
    }

    if trimmed == "tools" {
        let rows = audit::summarize_recent_by_tool(50)?;
        return Ok(format_audit_tools(&rows, 50, 20));
    }
    if let Some(v) = trimmed.strip_prefix("tools ") {
        let n = if v.trim().is_empty() {
            50
        } else {
            v.trim()
                .parse::<usize>()
                .map_err(|_| audit_usage().to_string())?
        };
        let rows = audit::summarize_recent_by_tool(n)?;
        return Ok(format_audit_tools(&rows, n, 20));
    }

    if trimmed == "reasons" {
        let rows = audit::summarize_recent_by_reason(50)?;
        return Ok(format_audit_reasons(&rows, 50, 20));
    }
    if let Some(v) = trimmed.strip_prefix("reasons ") {
        let n = if v.trim().is_empty() {
            50
        } else {
            v.trim()
                .parse::<usize>()
                .map_err(|_| audit_usage().to_string())?
        };
        let rows = audit::summarize_recent_by_reason(n)?;
        return Ok(format_audit_reasons(&rows, n, 20));
    }

    if trimmed == "export-last" {
        return export_audit_report_last(".", "both", 200);
    }
    if let Some(rest) = trimmed.strip_prefix("export-last ") {
        let (dir, format, window) = parse_audit_export_last_args(rest)?;
        return export_audit_report_last(&dir, &format, window);
    }

    if let Some(rest) = trimmed.strip_prefix("export ") {
        let (path, format, window) = parse_audit_export_args(rest)?;
        return export_audit_report(&path, &format, window);
    }

    Ok(audit_usage().to_string())
}

fn audit_usage() -> &'static str {
    "Usage: /audit tail [n] | /audit stats [n] | /audit tools [n] | /audit reasons [n] | /audit export <path> [md|json] [n] | /audit export-last [dir] [md|json|both] [n]"
}

fn parse_audit_export_args(input: &str) -> Result<(String, String, usize), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(audit_usage().to_string());
    }

    let (path, rest) = split_first_cli_token(trimmed).ok_or_else(|| audit_usage().to_string())?;
    let mut format = if path.to_ascii_lowercase().ends_with(".json") {
        "json".to_string()
    } else {
        "md".to_string()
    };
    let mut window = 200usize;

    for token in rest.split_whitespace() {
        if token.eq_ignore_ascii_case("md") || token.eq_ignore_ascii_case("json") {
            format = token.to_ascii_lowercase();
            continue;
        }
        if let Ok(n) = token.parse::<usize>() {
            window = n.clamp(1, 5000);
            continue;
        }
        return Err(audit_usage().to_string());
    }

    Ok((path, format, window))
}

fn parse_audit_export_last_args(input: &str) -> Result<(String, String, usize), String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok((".".to_string(), "both".to_string(), 200));
    }

    let mut dir = ".".to_string();
    let mut token_stream = trimmed;

    if let Some((first, rest)) = split_first_cli_token(trimmed) {
        let lower = first.to_ascii_lowercase();
        let is_format = lower == "md" || lower == "json" || lower == "both";
        let is_number = first.parse::<usize>().is_ok();
        if !is_format && !is_number {
            dir = first;
            token_stream = rest;
        }
    }

    let mut format = "both".to_string();
    let mut window = 200usize;

    for token in token_stream.split_whitespace() {
        if token.eq_ignore_ascii_case("md")
            || token.eq_ignore_ascii_case("json")
            || token.eq_ignore_ascii_case("both")
        {
            format = token.to_ascii_lowercase();
            continue;
        }
        if let Ok(n) = token.parse::<usize>() {
            window = n.clamp(1, 5000);
            continue;
        }
        return Err(audit_usage().to_string());
    }

    Ok((dir, format, window))
}

fn split_first_cli_token(input: &str) -> Option<(String, &str)> {
    let trimmed = input.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(rest) = trimmed.strip_prefix('"') {
        let end = rest.find('"')?;
        let token = rest[..end].to_string();
        let tail = rest[end + 1..].trim_start();
        return Some((token, tail));
    }

    if let Some((first, tail)) = trimmed.split_once(char::is_whitespace) {
        return Some((first.to_string(), tail.trim_start()));
    }

    Some((trimmed.to_string(), ""))
}

fn export_audit_report(path: &str, format: &str, window: usize) -> Result<String, String> {
    let target = PathBuf::from(path);
    audit::export_report(&target, format, window)?;
    Ok(format!(
        "audit report exported: {} format={} window={}",
        target.display(),
        format,
        window.clamp(1, 5000)
    ))
}

fn export_audit_report_last(dir: &str, format: &str, window: usize) -> Result<String, String> {
    let base_dir = PathBuf::from(dir);
    fs::create_dir_all(&base_dir).map_err(|e| e.to_string())?;

    let n = window.clamp(1, 5000);
    let ts = now_timestamp_ms();
    let mode = format.to_ascii_lowercase();

    match mode.as_str() {
        "md" => {
            let out = base_dir.join(format!("asi_audit_report_{}.md", ts));
            audit::export_report(&out, "md", n)?;
            Ok(format!(
                "audit report exported: {} format=md window={}",
                out.display(),
                n
            ))
        }
        "json" => {
            let out = base_dir.join(format!("asi_audit_report_{}.json", ts));
            audit::export_report(&out, "json", n)?;
            Ok(format!(
                "audit report exported: {} format=json window={}",
                out.display(),
                n
            ))
        }
        "both" => {
            let out_md = base_dir.join(format!("asi_audit_report_{}.md", ts));
            let out_json = base_dir.join(format!("asi_audit_report_{}.json", ts));
            audit::export_report(&out_md, "md", n)?;
            audit::export_report(&out_json, "json", n)?;
            Ok(format!(
                "audit reports exported: md={} json={} window={}",
                out_md.display(),
                out_json.display(),
                n
            ))
        }
        _ => Err(audit_usage().to_string()),
    }
}

fn now_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn format_audit_summary(summary: &audit::AuditSummary, window: usize) -> String {
    let last = summary
        .last_ts_ms
        .map(|x| x.to_string())
        .unwrap_or_else(|| "none".to_string());
    format!(
        "audit_recent(window={}): total={} allow={} deny={} last_ts_ms={}",
        window, summary.total, summary.allow, summary.deny, last
    )
}

fn format_audit_tools(rows: &[audit::ToolAuditSummary], window: usize, max_rows: usize) -> String {
    if rows.is_empty() {
        return format!("audit_tools(window={}): none", window);
    }

    let shown = rows.len().min(max_rows.max(1));
    let mut lines = vec![format!(
        "audit_tools(window={}): tool_count={} showing={}",
        window,
        rows.len(),
        shown
    )];

    for row in rows.iter().take(shown) {
        lines.push(format!(
            "- {} total={} allow={} deny={}",
            row.tool, row.total, row.allow, row.deny
        ));
    }

    lines.join("\n")
}

fn format_audit_tools_brief(rows: &[audit::ToolAuditSummary], window: usize) -> String {
    if rows.is_empty() {
        return format!("audit_tools_top(window={}): none", window);
    }

    let top = rows
        .iter()
        .take(3)
        .map(|row| format!("{}(a{} d{} t{})", row.tool, row.allow, row.deny, row.total))
        .collect::<Vec<_>>()
        .join("; ");

    format!("audit_tools_top(window={}): {}", window, top)
}

fn format_audit_reasons(
    rows: &[audit::ReasonAuditSummary],
    window: usize,
    max_rows: usize,
) -> String {
    if rows.is_empty() {
        return format!("audit_reasons(window={}): none", window);
    }

    let shown = rows.len().min(max_rows.max(1));
    let mut lines = vec![format!(
        "audit_reasons(window={}): reason_count={} showing={}",
        window,
        rows.len(),
        shown
    )];

    for row in rows.iter().take(shown) {
        lines.push(format!(
            "- {} total={} allow={} deny={}",
            row.reason, row.total, row.allow, row.deny
        ));
    }

    lines.join("\n")
}

fn format_audit_reasons_brief(rows: &[audit::ReasonAuditSummary], window: usize) -> String {
    if rows.is_empty() {
        return format!("audit_reasons_top(window={}): none", window);
    }

    let top = rows
        .iter()
        .take(3)
        .map(|row| {
            format!(
                "{}(a{} d{} t{})",
                row.reason, row.allow, row.deny, row.total
            )
        })
        .collect::<Vec<_>>()
        .join("; ");

    format!("audit_reasons_top(window={}): {}", window, top)
}

pub(crate) fn apply_runtime_flags_from_cfg(rt: &mut Runtime, cfg: &AppConfig) {
    rt.disable_web_tools = cfg.is_feature_disabled("web_tools");
    rt.disable_bash_tool = cfg.is_feature_disabled("bash_tool");
    rt.safe_shell_mode = cfg.safe_shell_mode;
    rt.permission_allow_rules = cfg.permission_allow_rules.clone();
    rt.permission_deny_rules = cfg.permission_deny_rules.clone();
    rt.path_restriction_enabled = cfg.path_restriction_enabled;
    rt.workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    rt.additional_directories = cfg
        .additional_directories
        .iter()
        .map(PathBuf::from)
        .collect();
    rt.auto_review_mode = cfg.auto_review_mode.clone();
    rt.auto_review_severity_threshold = cfg.auto_review_severity_threshold.clone();

    let provider_default = Runtime::default_native_tool_calling(&rt.provider, &rt.model);
    rt.native_tool_calling = match std::env::var("ASI_NATIVE_TOOL_CALLING") {
        Ok(v) => match v.trim().to_ascii_lowercase().as_str() {
            "on" | "true" | "1" | "enable" | "enabled" => true,
            "off" | "false" | "0" | "disable" | "disabled" => false,
            _ => provider_default,
        },
        Err(_) => provider_default,
    };
}
fn feature_status(cfg: &AppConfig) -> String {
    let names = ["web_tools", "bash_tool", "subagent", "research"];
    names
        .iter()
        .map(|name| {
            format!(
                "{}={}",
                name,
                if cfg.is_feature_disabled(name) {
                    "disabled"
                } else {
                    "enabled"
                }
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn privacy_status(cfg: &AppConfig) -> String {
    format!(
        "telemetry={} tool_details={} undercover={} safe_shell={} remote_policy_enabled={} remote_policy_url={}",
        cfg.telemetry_enabled,
        cfg.telemetry_log_tool_details,
        cfg.undercover_mode,
        cfg.safe_shell_mode,
        cfg.remote_policy_enabled,
        cfg.remote_policy_url
            .clone()
            .unwrap_or_else(|| "<unset>".to_string())
    )
}

fn parse_on_off(value: &str) -> Option<bool> {
    match value.trim() {
        "on" | "true" | "1" | "enable" | "enabled" => Some(true),
        "off" | "false" | "0" | "disable" | "disabled" => Some(false),
        _ => None,
    }
}

fn parse_prompt_profile(value: &str) -> Option<PromptProfile> {
    match value.trim().to_ascii_lowercase().as_str() {
        "standard" | "default" | "normal" => Some(PromptProfile::Standard),
        "strict" => Some(PromptProfile::Strict),
        _ => None,
    }
}

fn parse_execution_speed(value: &str) -> Option<ExecutionSpeed> {
    match value.trim().to_ascii_lowercase().as_str() {
        "sprint" | "fast" => Some(ExecutionSpeed::Sprint),
        "deep" | "thorough" => Some(ExecutionSpeed::Deep),
        _ => None,
    }
}

fn normalize_rule(rule: &str) -> Result<String, String> {
    let value = rule.trim();
    if value.is_empty() {
        return Err("rule is empty".to_string());
    }
    if value.contains(' ') && !value.contains(':') {
        return Err("rule with spaces must use <tool>:<prefix> format".to_string());
    }
    Ok(value.to_string())
}

fn normalize_directory(path: &str) -> Result<String, String> {
    let input = path.trim();
    if input.is_empty() {
        return Err("directory path is empty".to_string());
    }

    let p = PathBuf::from(input);
    let abs = if p.is_absolute() {
        p
    } else {
        std::env::current_dir().map_err(|e| e.to_string())?.join(p)
    };

    if !abs.exists() {
        return Err(format!("directory does not exist: {}", abs.display()));
    }
    if !abs.is_dir() {
        return Err(format!("not a directory: {}", abs.display()));
    }

    let canonical = std::fs::canonicalize(&abs).map_err(|e| e.to_string())?;
    Ok(canonical.display().to_string())
}

fn add_unique_rule(vec: &mut Vec<String>, rule: String) {
    if !vec.iter().any(|x| x == &rule) {
        vec.push(rule);
    }
}

#[derive(Debug, Subcommand)]
enum McpCliCommand {
    List {
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Add {
        name: String,
        command: String,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Rm {
        name: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Show {
        name: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Auth {
        name: String,
        auth_type: String,
        value: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Oauth {
        #[command(subcommand)]
        action: McpCliOauthCommand,
    },
    Config {
        #[command(subcommand)]
        action: McpCliConfigCommand,
    },
    Export {
        path: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Import {
        path: String,
        mode: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Start {
        name: String,
        command: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Stop {
        name: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum McpCliOauthCommand {
    Login {
        name: String,
        provider: String,
        token: String,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value_t = false)]
        link_auth: bool,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Status {
        name: String,
        provider: Option<String>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Logout {
        name: String,
        provider: Option<String>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum McpCliConfigCommand {
    Set {
        name: String,
        key: String,
        value: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Rm {
        name: String,
        key: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Scope {
        name: String,
        scope: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Trust {
        name: String,
        trusted: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

pub(crate) fn mcp_cli_to_repl_args(cmd: McpCliCommand) -> String {
    match cmd {
        McpCliCommand::List { json } => append_json_flag("list".to_string(), json),
        McpCliCommand::Add {
            name,
            command,
            scope,
            json,
        } => {
            let mut s = format!("add {} {}", name, command);
            if let Some(v) = scope {
                s.push_str(" --scope ");
                s.push_str(&v);
            }
            append_json_flag(s, json)
        }
        McpCliCommand::Rm { name, json } => append_json_flag(format!("rm {}", name), json),
        McpCliCommand::Show { name, json } => append_json_flag(format!("show {}", name), json),
        McpCliCommand::Auth {
            name,
            auth_type,
            value,
            json,
        } => {
            let mut s = format!("auth {} {}", name, auth_type);
            if let Some(v) = value {
                s.push(' ');
                s.push_str(&v);
            }
            append_json_flag(s, json)
        }
        McpCliCommand::Oauth { action } => match action {
            McpCliOauthCommand::Login {
                name,
                provider,
                token,
                scope,
                link_auth,
                json,
            } => {
                let mut s = format!("oauth login {} {} {}", name, provider, token);
                if let Some(v) = scope {
                    s.push_str(" --scope ");
                    s.push_str(&v);
                }
                if link_auth {
                    s.push_str(" --link-auth");
                }
                append_json_flag(s, json)
            }
            McpCliOauthCommand::Status {
                name,
                provider,
                scope,
                json,
            } => {
                let mut s = format!("oauth status {}", name);
                if let Some(p) = provider {
                    s.push(' ');
                    s.push_str(&p);
                }
                if let Some(v) = scope {
                    s.push_str(" --scope ");
                    s.push_str(&v);
                }
                append_json_flag(s, json)
            }
            McpCliOauthCommand::Logout {
                name,
                provider,
                scope,
                json,
            } => {
                let mut s = format!("oauth logout {}", name);
                if let Some(p) = provider {
                    s.push(' ');
                    s.push_str(&p);
                }
                if let Some(v) = scope {
                    s.push_str(" --scope ");
                    s.push_str(&v);
                }
                append_json_flag(s, json)
            }
        },
        McpCliCommand::Config { action } => match action {
            McpCliConfigCommand::Set {
                name,
                key,
                value,
                json,
            } => append_json_flag(format!("config {} {} {}", name, key, value), json),
            McpCliConfigCommand::Rm { name, key, json } => {
                append_json_flag(format!("config rm {} {}", name, key), json)
            }
            McpCliConfigCommand::Scope { name, scope, json } => {
                append_json_flag(format!("config scope {} {}", name, scope), json)
            }
            McpCliConfigCommand::Trust {
                name,
                trusted,
                json,
            } => append_json_flag(format!("config trust {} {}", name, trusted), json),
        },
        McpCliCommand::Export { path, json, .. } => {
            append_json_flag(format!("export {}", path), json)
        }
        McpCliCommand::Import { path, mode, json } => {
            let mut s = format!("import {}", path);
            if let Some(m) = mode {
                s.push(' ');
                s.push_str(&m);
            }
            append_json_flag(s, json)
        }
        McpCliCommand::Start {
            name,
            command,
            json,
        } => {
            let mut s = format!("start {}", name);
            if let Some(cmd) = command {
                s.push(' ');
                s.push_str(&cmd);
            }
            append_json_flag(s, json)
        }
        McpCliCommand::Stop { name, json } => append_json_flag(format!("stop {}", name), json),
    }
}

fn append_json_flag(mut base: String, json: bool) -> String {
    if json {
        if !base.is_empty() {
            base.push(' ');
        }
        base.push_str("--json");
    }
    base
}

#[derive(Debug, Subcommand)]
enum PluginCliCommand {
    List {
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Add {
        name: String,
        path: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Rm {
        name: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Show {
        name: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Enable {
        name: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Disable {
        name: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Trust {
        name: String,
        mode: String,
        hash: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Verify {
        name: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Config {
        #[command(subcommand)]
        action: PluginCliConfigCommand,
    },
    Export {
        path: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Import {
        path: String,
        mode: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Market {
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum PluginCliConfigCommand {
    Set {
        name: String,
        key: String,
        value: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Rm {
        name: String,
        key: String,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

fn plugin_record_json(mut item: plugin::PluginRecord) -> serde_json::Value {
    let mut config_pairs: Vec<(String, String)> = item.config.drain().collect();
    config_pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let config = config_pairs
        .into_iter()
        .map(|(k, v)| serde_json::json!({ "key": k, "value": v }))
        .collect::<Vec<_>>();
    serde_json::json!({
        "name": item.name,
        "path": item.path,
        "enabled": item.enabled,
        "trusted": item.trusted,
        "trust_policy": item.trust_policy,
        "trust_hash": item.trust_hash,
        "source": item.source,
        "version": item.version,
        "signature": item.signature,
        "config": config,
    })
}

fn plugin_json_response(command: &str, plugin_payload: serde_json::Value) -> String {
    serde_json::json!({
        "schema_version": "1",
        "command": command,
        "plugin": plugin_payload
    })
    .to_string()
}

fn handle_plugin_command(args: &str) -> Result<String, String> {
    let normalized_args = normalize_publish_meta_in_config_args(args);
    let tokens = parse_cli_tokens(&normalized_args)?;
    if tokens.is_empty() || tokens[0].eq_ignore_ascii_case("list") {
        let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
        let list = plugin::list_plugins();
        if json_mode {
            let items = list.into_iter().map(plugin_record_json).collect::<Vec<_>>();
            return Ok(plugin_json_response(
                "plugin_list",
                serde_json::json!({
                    "count": items.len(),
                    "items": items
                }),
            ));
        }
        if list.is_empty() {
            return Ok("No plugins".to_string());
        }
        let lines = list
            .into_iter()
            .map(|x| {
                format!(
                    "- {} enabled={} trusted={} path={} config_keys={}",
                    x.name,
                    x.enabled,
                    x.trusted,
                    x.path,
                    x.config.len()
                )
            })
            .collect::<Vec<_>>();
        return Ok(lines.join("\n"));
    }

    if tokens[0].eq_ignore_ascii_case("add") {
        if tokens.len() < 3 {
            return Ok("Usage: /plugin add <name> <path> [--json]".to_string());
        }
        let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
        let rec = plugin::add_plugin(&tokens[1], &tokens[2])?;
        if json_mode {
            return Ok(plugin_json_response(
                "plugin_add",
                serde_json::json!({
                    "added": true,
                    "plugin": plugin_record_json(rec)
                }),
            ));
        }
        return Ok(format!("plugin added name={} path={}", rec.name, rec.path));
    }

    if tokens[0].eq_ignore_ascii_case("rm") {
        if tokens.len() < 2 {
            return Ok("Usage: /plugin rm <name> [--json]".to_string());
        }
        let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
        let changed = plugin::remove_plugin(&tokens[1])?;
        if json_mode {
            return Ok(plugin_json_response(
                "plugin_rm",
                serde_json::json!({
                    "name": tokens[1],
                    "changed": changed
                }),
            ));
        }
        return Ok(format!("plugin removed name={} changed={}", tokens[1], changed));
    }

    if tokens[0].eq_ignore_ascii_case("show") {
        if tokens.len() < 2 {
            return Ok("Usage: /plugin show <name> [--json]".to_string());
        }
        let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
        if let Some(rec) = plugin::get_plugin(&tokens[1]) {
            if json_mode {
                return Ok(plugin_json_response("plugin_show", plugin_record_json(rec)));
            }
            let mut lines = vec![
                format!("name={}", rec.name),
                format!("path={}", rec.path),
                format!("enabled={}", rec.enabled),
                format!("trusted={}", rec.trusted),
            ];
            if rec.config.is_empty() {
                lines.push("config: <empty>".to_string());
            } else {
                lines.push("config:".to_string());
                let mut pairs = rec.config.into_iter().collect::<Vec<_>>();
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
                for (k, v) in pairs {
                    lines.push(format!("- {}={}", k, v));
                }
            }
            return Ok(lines.join("\n"));
        }
        if json_mode {
            return Ok(plugin_json_response(
                "plugin_show",
                serde_json::json!({
                    "name": tokens[1],
                    "status": "unknown"
                }),
            ));
        }
        return Ok(format!("plugin `{}` not found", tokens[1]));
    }

    if tokens[0].eq_ignore_ascii_case("enable") {
        if tokens.len() < 2 {
            return Ok("Usage: /plugin enable <name> [--json]".to_string());
        }
        let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
        plugin::set_plugin_enabled(&tokens[1], true)?;
        if json_mode {
            return Ok(plugin_json_response(
                "plugin_enable",
                serde_json::json!({
                    "name": tokens[1],
                    "enabled": true
                }),
            ));
        }
        return Ok(format!("plugin enabled name={}", tokens[1]));
    }

    if tokens[0].eq_ignore_ascii_case("disable") {
        if tokens.len() < 2 {
            return Ok("Usage: /plugin disable <name> [--json]".to_string());
        }
        let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
        plugin::set_plugin_enabled(&tokens[1], false)?;
        if json_mode {
            return Ok(plugin_json_response(
                "plugin_disable",
                serde_json::json!({
                    "name": tokens[1],
                    "enabled": false
                }),
            ));
        }
        return Ok(format!("plugin disabled name={}", tokens[1]));
    }

    if tokens[0].eq_ignore_ascii_case("trust") {
        if tokens.len() < 3 {
            return Ok("Usage: /plugin trust <name> <manual|hash> [hash] [--json]".to_string());
        }
        let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
        let name = tokens[1].as_str();
        let mode = tokens[2].trim().to_ascii_lowercase();
        if mode == "manual" {
            plugin::set_plugin_trust_manual(name, true)?;
            if json_mode {
                return Ok(plugin_json_response(
                    "plugin_trust",
                    serde_json::json!({
                        "name": name,
                        "trusted": true,
                        "trust_policy": "manual",
                        "trust_hash": serde_json::Value::Null
                    }),
                ));
            }
            return Ok(format!(
                "plugin trust updated name={} trust_policy=manual trusted=true",
                name
            ));
        }
        if mode == "hash" {
            let expected = tokens
                .iter()
                .skip(3)
                .find(|t| !t.eq_ignore_ascii_case("--json"))
                .map(|s| s.as_str());
            let actual = plugin::set_plugin_trust_hash(name, expected)?;
            if json_mode {
                return Ok(plugin_json_response(
                    "plugin_trust",
                    serde_json::json!({
                        "name": name,
                        "trusted": true,
                        "trust_policy": "hash",
                        "trust_hash": actual
                    }),
                ));
            }
            return Ok(format!(
                "plugin trust updated name={} trust_policy=hash trusted=true trust_hash={}",
                name, actual
            ));
        }
        return Ok("Usage: /plugin trust <name> <manual|hash> [hash] [--json]".to_string());
    }

    if tokens[0].eq_ignore_ascii_case("verify") {
        if tokens.len() < 2 {
            return Ok("Usage: /plugin verify <name> [--json]".to_string());
        }
        let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
        let name = tokens[1].as_str();
        let ok = plugin::verify_plugin_trust(name)?;
        if json_mode {
            return Ok(plugin_json_response(
                "plugin_verify",
                serde_json::json!({
                    "name": name,
                    "verified": ok
                }),
            ));
        }
        return Ok(format!("plugin verify name={} verified={}", name, ok));
    }

    if tokens[0].eq_ignore_ascii_case("config") {
        if tokens.len() < 2 {
            return Ok("Usage: /plugin config set <name> <key> <value> [--json] | /plugin config rm <name> <key> [--json]".to_string());
        }
        let action = tokens[1].to_ascii_lowercase();
        let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
        if action == "set" {
            if tokens.len() < 5 {
                return Ok("Usage: /plugin config set <name> <key> <value> [--json]".to_string());
            }
            plugin::set_plugin_config(&tokens[2], &tokens[3], &tokens[4])?;
            if json_mode {
                return Ok(plugin_json_response(
                    "plugin_config_set",
                    serde_json::json!({
                        "name": tokens[2],
                        "key": tokens[3]
                    }),
                ));
            }
            return Ok(format!(
                "plugin config updated name={} key={}",
                tokens[2], tokens[3]
            ));
        }
        if action == "rm" {
            if tokens.len() < 4 {
                return Ok("Usage: /plugin config rm <name> <key> [--json]".to_string());
            }
            let changed = plugin::remove_plugin_config(&tokens[2], &tokens[3])?;
            if json_mode {
                return Ok(plugin_json_response(
                    "plugin_config_rm",
                    serde_json::json!({
                        "name": tokens[2],
                        "key": tokens[3],
                        "changed": changed
                    }),
                ));
            }
            return Ok(format!(
                "plugin config removed name={} key={} changed={}",
                tokens[2], tokens[3], changed
            ));
        }
        return Ok("Usage: /plugin config set <name> <key> <value> [--json] | /plugin config rm <name> <key> [--json]".to_string());
    }

    if tokens[0].eq_ignore_ascii_case("export") {
        if tokens.len() < 2 {
            return Ok("Usage: /plugin export <path.json> [--json]".to_string());
        }
        let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
        let saved = plugin::export_state(&tokens[1])?;
        if json_mode {
            return Ok(plugin_json_response(
                "plugin_export",
                serde_json::json!({
                    "path": saved
                }),
            ));
        }
        return Ok(format!("plugin exported path={}", saved));
    }

    if tokens[0].eq_ignore_ascii_case("import") {
        if tokens.len() < 2 {
            return Ok("Usage: /plugin import <path.json> [merge|replace] [--json]".to_string());
        }
        let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
        let mode = tokens
            .get(2)
            .map(|v| v.to_ascii_lowercase())
            .unwrap_or_else(|| "replace".to_string());
        let merge = mode == "merge";
        let total = plugin::import_state(&tokens[1], merge)?;
        if json_mode {
            return Ok(plugin_json_response(
                "plugin_import",
                serde_json::json!({
                    "path": tokens[1],
                    "mode": if merge { "merge" } else { "replace" },
                    "total_plugins": total
                }),
            ));
        }
        return Ok(format!(
            "plugin imported path={} mode={} total_plugins={}",
            tokens[1],
            if merge { "merge" } else { "replace" },
            total
        ));
    }

    if tokens[0].eq_ignore_ascii_case("market") {
        let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
        let market_path = std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(".agents")
            .join("plugins")
            .join("marketplace.json");
        let exists = market_path.exists();
        let mut item_count = 0usize;
        let mut names: Vec<String> = Vec::new();
        let mut parse_error: Option<String> = None;
        if exists {
            match std::fs::read_to_string(&market_path) {
                Ok(text) => {
                    match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(v) => {
                            if let Some(items) = v
                                .get("items")
                                .and_then(|x| x.as_array())
                            {
                                item_count = items.len();
                                for item in items {
                                    if let Some(name) = item
                                        .get("name")
                                        .and_then(|x| x.as_str())
                                        .map(|s| s.to_string())
                                    {
                                        names.push(name);
                                    }
                                }
                            } else if let Some(items) = v.as_array() {
                                item_count = items.len();
                                for item in items {
                                    if let Some(name) = item
                                        .get("name")
                                        .and_then(|x| x.as_str())
                                        .map(|s| s.to_string())
                                    {
                                        names.push(name);
                                    }
                                }
                            }
                            names.sort();
                        }
                        Err(e) => {
                            parse_error = Some(e.to_string());
                        }
                    }
                }
                Err(e) => {
                    parse_error = Some(e.to_string());
                }
            }
        }

        if json_mode {
            return Ok(plugin_json_response(
                "plugin_market",
                serde_json::json!({
                    "path": market_path.display().to_string(),
                    "exists": exists,
                    "items_count": item_count,
                    "items_preview": names.into_iter().take(20).collect::<Vec<_>>(),
                    "parse_error": parse_error,
                }),
            ));
        }

        if !exists {
            return Ok(format!(
                "plugin market path={} status=missing",
                market_path.display()
            ));
        }
        if let Some(err) = parse_error {
            return Ok(format!(
                "plugin market path={} status=parse_error error={}",
                market_path.display(),
                err
            ));
        }
        let preview = if names.is_empty() {
            "<empty>".to_string()
        } else {
            names.into_iter().take(10).collect::<Vec<_>>().join(", ")
        };
        return Ok(format!(
            "plugin market path={} status=ok items_count={} items_preview=[{}]",
            market_path.display(),
            item_count,
            preview
        ));
    }

    Ok("Usage: /plugin list [--json] | /plugin add <name> <path> [--json] | /plugin rm <name> [--json] | /plugin show <name> [--json] | /plugin enable <name> [--json] | /plugin disable <name> [--json] | /plugin trust <name> <manual|hash> [hash] [--json] | /plugin verify <name> [--json] | /plugin config set <name> <key> <value> [--json] | /plugin config rm <name> <key> [--json] | /plugin export <path.json> [--json] | /plugin import <path.json> [merge|replace] [--json] | /plugin market [--json]".to_string())
}

#[derive(Debug, Subcommand)]
enum HooksCliCommand {
    Status {
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Test {
        event: String,
        #[arg(long)]
        tool: Option<String>,
        #[arg(long)]
        args: Option<String>,
        #[arg(long)]
        mode: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Config {
        #[command(subcommand)]
        action: HooksCliConfigCommand,
    },
}

#[derive(Debug, Subcommand)]
enum HooksCliConfigCommand {
    Show {
        #[arg(long)]
        path: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    ListHandlers {
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        event: Option<String>,
        #[arg(long)]
        tool_prefix: Option<String>,
        #[arg(long)]
        permission_mode: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Export {
        path: String,
        #[arg(long = "path")]
        source_path: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Import {
        path: String,
        mode: Option<String>,
        #[arg(long)]
        target: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Set {
        event: String,
        script: String,
        #[arg(long)]
        path: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    SetHandler {
        event: String,
        script: String,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        timeout_secs: Option<String>,
        #[arg(long)]
        json_protocol: Option<String>,
        #[arg(long)]
        tool_prefix: Option<String>,
        #[arg(long)]
        permission_mode: Option<String>,
        #[arg(long)]
        failure_policy: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    EditHandler {
        event: String,
        script: String,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        new_script: Option<String>,
        #[arg(long)]
        timeout_secs: Option<String>,
        #[arg(long)]
        json_protocol: Option<String>,
        #[arg(long)]
        tool_prefix: Option<String>,
        #[arg(long)]
        permission_mode: Option<String>,
        #[arg(long)]
        failure_policy: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Rm {
        event: String,
        #[arg(long)]
        path: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    RmHandler {
        event: String,
        script: String,
        #[arg(long)]
        path: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    Validate {
        #[arg(long)]
        path: Option<String>,
        #[arg(long, default_value_t = false)]
        strict: bool,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

fn hooks_json_response(command: &str, hooks_payload: serde_json::Value) -> String {
    serde_json::json!({
        "schema_version": HOOKS_JSON_SCHEMA_VERSION,
        "command": command,
        "hooks": hooks_payload,
    })
    .to_string()
}

fn normalize_hooks_event(raw: &str) -> Option<String> {
    let lower = raw.trim().to_ascii_lowercase();
    match lower.as_str() {
        "pretooluse" | "pre_tool_use" | "pre-tool-use" => Some("PreToolUse".to_string()),
        "permissionrequest" | "permission_request" | "permission-request" => {
            Some("PermissionRequest".to_string())
        }
        "posttooluse" | "post_tool_use" | "post-tool-use" => Some("PostToolUse".to_string()),
        "sessionstart" | "session_start" | "session-start" => Some("SessionStart".to_string()),
        "userpromptsubmit" | "user_prompt_submit" | "user-prompt-submit" => {
            Some("UserPromptSubmit".to_string())
        }
        "stop" => Some("Stop".to_string()),
        "subagentstop" | "subagent_stop" | "subagent-stop" => Some("SubagentStop".to_string()),
        "precompact" | "pre_compact" | "pre-compact" => Some("PreCompact".to_string()),
        "postcompact" | "post_compact" | "post-compact" => Some("PostCompact".to_string()),
        _ => None,
    }
}

fn hooks_allowed_events_line() -> String {
    "allowed events: PreToolUse|PermissionRequest|PostToolUse|SessionStart|UserPromptSubmit|Stop|SubagentStop|PreCompact|PostCompact".to_string()
}

fn hooks_config_usage_line() -> &'static str {
    "Usage: /hooks config show [--path <file.json>] [--json] | /hooks config list-handlers [--path <file.json>] [--event <event>] [--tool-prefix <tool>] [--permission-mode <mode>] [--json] | /hooks config export <path.json> [--path <source.json>] [--json] | /hooks config import <path.json> [merge|replace] [--target <file.json>] [--json] | /hooks config set <event> <script> [--path <file.json>] [--json] | /hooks config set-handler <event> <script> [--path <file.json>] [--timeout-secs <n>] [--json-protocol <on|off>] [--tool-prefix <tool>] [--permission-mode <read-only|workspace-write|on-request|danger-full-access>] [--failure-policy <fail-open|fail-closed>] [--json] | /hooks config edit-handler <event> <script> [--path <file.json>] [--new-script <script>] [--timeout-secs <n|none>] [--json-protocol <on|off|none>] [--tool-prefix <tool|none>] [--permission-mode <read-only|workspace-write|on-request|danger-full-access|none>] [--failure-policy <fail-open|fail-closed|none>] [--json] | /hooks config rm <event> [--path <file.json>] [--json] | /hooks config rm-handler <event> <script> [--path <file.json>] [--json] | /hooks config validate [--path <file.json>] [--strict] [--json]"
}

fn hooks_config_list_handlers_usage_line() -> &'static str {
    "Usage: /hooks config list-handlers [--path <file.json>] [--event <event>] [--tool-prefix <tool>] [--permission-mode <mode>] [--json]"
}

fn hooks_config_set_handler_usage_line() -> &'static str {
    "Usage: /hooks config set-handler <event> <script> [--path <file.json>] [--timeout-secs <n>] [--json-protocol <on|off>] [--tool-prefix <tool>] [--permission-mode <read-only|workspace-write|on-request|danger-full-access>] [--failure-policy <fail-open|fail-closed>] [--json]"
}

fn hooks_config_edit_handler_usage_line() -> &'static str {
    "Usage: /hooks config edit-handler <event> <script> [--path <file.json>] [--new-script <script>] [--timeout-secs <n|none>] [--json-protocol <on|off|none>] [--tool-prefix <tool|none>] [--permission-mode <read-only|workspace-write|on-request|danger-full-access|none>] [--failure-policy <fail-open|fail-closed|none>] [--json]"
}

fn hooks_config_validate_usage_line() -> &'static str {
    "Usage: /hooks config validate [--path <file.json>] [--strict] [--json]"
}

fn hooks_env_key_for_event(event: &str) -> Option<&'static str> {
    match event {
        "PreToolUse" => Some("ASI_HOOK_PRE_TOOL_USE"),
        "PermissionRequest" => Some("ASI_HOOK_PERMISSION_REQUEST"),
        "PostToolUse" => Some("ASI_HOOK_POST_TOOL_USE"),
        "SessionStart" => Some("ASI_HOOK_SESSION_START"),
        "UserPromptSubmit" => Some("ASI_HOOK_USER_PROMPT_SUBMIT"),
        "Stop" => Some("ASI_HOOK_STOP"),
        "SubagentStop" => Some("ASI_HOOK_SUBAGENT_STOP"),
        "PreCompact" => Some("ASI_HOOK_PRE_COMPACT"),
        "PostCompact" => Some("ASI_HOOK_POST_COMPACT"),
        _ => None,
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Default)]
struct HooksConfigFile {
    #[serde(default)]
    handlers: Vec<HooksConfigRow>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct HooksConfigRow {
    event: String,
    script: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    json_protocol: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_policy: Option<String>,
}

fn resolve_hooks_config_path(explicit_path: Option<&str>) -> Result<PathBuf, String> {
    if let Some(path) = explicit_path {
        let t = path.trim();
        if t.is_empty() {
            return Err("hooks config path is empty".to_string());
        }
        return Ok(PathBuf::from(t));
    }
    if let Ok(path) = std::env::var("ASI_HOOK_CONFIG_PATH") {
        let t = path.trim();
        if !t.is_empty() {
            return Ok(PathBuf::from(t));
        }
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok(cwd.join(".asi").join("hooks.json"))
}

fn load_hooks_config_file(path: &Path) -> Result<HooksConfigFile, String> {
    if !path.exists() {
        return Ok(HooksConfigFile::default());
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Ok(HooksConfigFile::default());
    }
    serde_json::from_str::<HooksConfigFile>(&text).map_err(|e| e.to_string())
}

fn save_hooks_config_file(path: &Path, config: &HooksConfigFile) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

fn parse_hooks_path_flag(tokens: &mut Vec<String>) -> Result<Option<String>, String> {
    let mut path: Option<String> = None;
    let mut idx = 0usize;
    while idx < tokens.len() {
        if tokens[idx].eq_ignore_ascii_case("--path") {
            let value = tokens
                .get(idx + 1)
                .ok_or_else(|| hooks_config_usage_line().to_string())?
                .trim()
                .to_string();
            if value.is_empty() {
                return Err(hooks_config_usage_line().to_string());
            }
            path = Some(value);
            tokens.drain(idx..=idx + 1);
            continue;
        }
        idx += 1;
    }
    Ok(path)
}

fn hooks_config_show(path_hint: Option<&str>, json_mode: bool) -> Result<String, String> {
    let path = resolve_hooks_config_path(path_hint)?;
    let cfg = load_hooks_config_file(&path)?;
    if json_mode {
        return Ok(hooks_json_response(
            "hooks_config_show",
            serde_json::json!({
                "path": path.display().to_string(),
                "handlers_count": cfg.handlers.len(),
                "handlers": cfg.handlers,
            }),
        ));
    }
    if cfg.handlers.is_empty() {
        return Ok(format!(
            "hooks config path={} handlers=0",
            path.display()
        ));
    }
    let mut lines = vec![format!(
        "hooks config path={} handlers={}",
        path.display(),
        cfg.handlers.len()
    )];
    for row in &cfg.handlers {
        lines.push(format!("- event={} script={}", row.event, row.script));
    }
    Ok(lines.join("\n"))
}

#[derive(Debug, Clone, Default)]
struct HooksListFilter {
    event: Option<String>,
    tool_prefix: Option<String>,
    permission_mode: Option<String>,
}

fn hooks_row_matches_filter(row: &HooksConfigRow, filter: &HooksListFilter) -> bool {
    if let Some(event) = filter.event.as_deref() {
        if row.event != event {
            return false;
        }
    }
    if let Some(tool_prefix) = filter.tool_prefix.as_deref() {
        if row.tool_prefix.as_deref() != Some(tool_prefix) {
            return false;
        }
    }
    if let Some(mode) = filter.permission_mode.as_deref() {
        if row.permission_mode.as_deref() != Some(mode) {
            return false;
        }
    }
    true
}

fn hooks_config_list_handlers(
    path_hint: Option<&str>,
    filter: HooksListFilter,
    json_mode: bool,
) -> Result<String, String> {
    let path = resolve_hooks_config_path(path_hint)?;
    let cfg = load_hooks_config_file(&path)?;
    let items = cfg
        .handlers
        .into_iter()
        .filter(|row| hooks_row_matches_filter(row, &filter))
        .collect::<Vec<_>>();
    if json_mode {
        return Ok(hooks_json_response(
            "hooks_config_list_handlers",
            serde_json::json!({
                "path": path.display().to_string(),
                "count": items.len(),
                "filter": {
                    "event": filter.event,
                    "tool_prefix": filter.tool_prefix,
                    "permission_mode": filter.permission_mode,
                },
                "handlers": items,
            }),
        ));
    }
    if items.is_empty() {
        return Ok(format!(
            "hooks config list-handlers path={} count=0",
            path.display()
        ));
    }
    let mut lines = vec![format!(
        "hooks config list-handlers path={} count={}",
        path.display(),
        items.len()
    )];
    for row in &items {
        lines.push(format!(
            "- event={} script={} timeout_secs={:?} json_protocol={:?} tool_prefix={:?} permission_mode={:?} failure_policy={:?}",
            row.event,
            row.script,
            row.timeout_secs,
            row.json_protocol,
            row.tool_prefix,
            row.permission_mode,
            row.failure_policy
        ));
    }
    Ok(lines.join("\n"))
}

fn hooks_config_export(output_path: &str, source_path_hint: Option<&str>, json_mode: bool) -> Result<String, String> {
    let out = output_path.trim();
    if out.is_empty() {
        return Err("export path is empty".to_string());
    }
    let source_path = resolve_hooks_config_path(source_path_hint)?;
    let cfg = load_hooks_config_file(&source_path)?;
    let out_path = PathBuf::from(out);
    save_hooks_config_file(&out_path, &cfg)?;
    if json_mode {
        return Ok(hooks_json_response(
            "hooks_config_export",
            serde_json::json!({
                "source_path": source_path.display().to_string(),
                "path": out_path.display().to_string(),
                "handlers_count": cfg.handlers.len(),
            }),
        ));
    }
    Ok(format!(
        "hooks config export source_path={} path={} handlers={}",
        source_path.display(),
        out_path.display(),
        cfg.handlers.len()
    ))
}

fn hooks_config_import(
    import_path: &str,
    mode_raw: Option<&str>,
    target_path_hint: Option<&str>,
    json_mode: bool,
) -> Result<String, String> {
    let in_path = import_path.trim();
    if in_path.is_empty() {
        return Err("import path is empty".to_string());
    }
    let source_path = PathBuf::from(in_path);
    let incoming = load_hooks_config_file(&source_path)?;
    let mode = mode_raw.unwrap_or("replace").trim().to_ascii_lowercase();
    let merge = match mode.as_str() {
        "merge" => true,
        "replace" => false,
        _ => return Err("mode must be merge|replace".to_string()),
    };
    let target_path = resolve_hooks_config_path(target_path_hint)?;
    let final_cfg = if merge {
        let mut current = load_hooks_config_file(&target_path)?;
        for incoming_row in incoming.handlers {
            let mut replaced = false;
            for existing in &mut current.handlers {
                if existing.event == incoming_row.event {
                    *existing = incoming_row.clone();
                    replaced = true;
                    break;
                }
            }
            if !replaced {
                current.handlers.push(incoming_row);
            }
        }
        current
    } else {
        incoming
    };
    save_hooks_config_file(&target_path, &final_cfg)?;
    if json_mode {
        return Ok(hooks_json_response(
            "hooks_config_import",
            serde_json::json!({
                "path": source_path.display().to_string(),
                "target_path": target_path.display().to_string(),
                "mode": if merge { "merge" } else { "replace" },
                "handlers_count": final_cfg.handlers.len(),
            }),
        ));
    }
    Ok(format!(
        "hooks config import path={} target_path={} mode={} handlers={}",
        source_path.display(),
        target_path.display(),
        if merge { "merge" } else { "replace" },
        final_cfg.handlers.len()
    ))
}

fn hooks_config_set(
    event_raw: &str,
    script: &str,
    path_hint: Option<&str>,
    json_mode: bool,
) -> Result<String, String> {
    let event = normalize_hooks_event(event_raw).ok_or_else(|| {
        format!(
            "invalid event `{}`; {}",
            event_raw,
            hooks_allowed_events_line()
        )
    })?;
    let script_trimmed = script.trim();
    if script_trimmed.is_empty() {
        return Err("script is empty".to_string());
    }
    let path = resolve_hooks_config_path(path_hint)?;
    let mut cfg = load_hooks_config_file(&path)?;
    let mut replaced = false;
    for row in &mut cfg.handlers {
        if row.event == event {
            row.script = script_trimmed.to_string();
            replaced = true;
        }
    }
    if !replaced {
        cfg.handlers.push(HooksConfigRow {
            event: event.clone(),
            script: script_trimmed.to_string(),
            timeout_secs: None,
            json_protocol: None,
            tool_prefix: None,
            permission_mode: None,
            failure_policy: None,
        });
    }
    save_hooks_config_file(&path, &cfg)?;
    let env_key = hooks_env_key_for_event(&event).unwrap_or("-");
    if json_mode {
        return Ok(hooks_json_response(
            "hooks_config_set",
            serde_json::json!({
                "path": path.display().to_string(),
                "event": event,
                "env_key": env_key,
                "script": script_trimmed,
                "replaced": replaced,
                "handlers_count": cfg.handlers.len(),
            }),
        ));
    }
    Ok(format!(
        "hooks config set path={} event={} env_key={} replaced={} handlers={}",
        path.display(),
        event,
        env_key,
        replaced,
        cfg.handlers.len()
    ))
}

#[derive(Debug, Clone, Default)]
struct HooksHandlerOverrides {
    timeout_secs: Option<Option<u64>>,
    json_protocol: Option<Option<bool>>,
    tool_prefix: Option<Option<String>>,
    permission_mode: Option<Option<String>>,
    failure_policy: Option<Option<String>>,
}

fn validate_hooks_failure_policy(raw: &str) -> Option<String> {
    let v = raw.trim().to_ascii_lowercase();
    match v.as_str() {
        "fail-open" | "open" => Some("fail-open".to_string()),
        "fail-closed" | "closed" => Some("fail-closed".to_string()),
        _ => None,
    }
}

fn validate_hooks_json_protocol(raw: &str) -> Option<bool> {
    parse_on_off(raw)
}

fn parse_hooks_optional_value(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(t.to_string())
    }
}

fn hooks_config_set_handler(
    event_raw: &str,
    script: &str,
    path_hint: Option<&str>,
    overrides: HooksHandlerOverrides,
    json_mode: bool,
) -> Result<String, String> {
    let event = normalize_hooks_event(event_raw).ok_or_else(|| {
        format!(
            "invalid event `{}`; {}",
            event_raw,
            hooks_allowed_events_line()
        )
    })?;
    let script_trimmed = script.trim();
    if script_trimmed.is_empty() {
        return Err("script is empty".to_string());
    }
    if let Some(Some(mode)) = overrides.permission_mode.as_ref() {
        let _ = validate_hooks_permission_mode(mode).ok_or_else(|| {
            "permission_mode must be one of: read-only|workspace-write|on-request|danger-full-access".to_string()
        })?;
    }
    if let Some(Some(fp)) = overrides.failure_policy.as_ref() {
        let _ = validate_hooks_failure_policy(fp)
            .ok_or_else(|| "failure_policy must be fail-open|fail-closed".to_string())?;
    }
    let path = resolve_hooks_config_path(path_hint)?;
    let mut cfg = load_hooks_config_file(&path)?;
    let mut replaced = false;
    for row in &mut cfg.handlers {
        if row.event == event {
            row.script = script_trimmed.to_string();
            if let Some(v) = overrides.timeout_secs {
                row.timeout_secs = v;
            }
            if let Some(v) = overrides.json_protocol {
                row.json_protocol = v;
            }
            if let Some(v) = overrides.tool_prefix.clone() {
                row.tool_prefix = v;
            }
            if let Some(v) = overrides.permission_mode.clone() {
                row.permission_mode = v;
            }
            if let Some(v) = overrides.failure_policy.clone() {
                row.failure_policy = v;
            }
            replaced = true;
        }
    }
    if !replaced {
        cfg.handlers.push(HooksConfigRow {
            event: event.clone(),
            script: script_trimmed.to_string(),
            timeout_secs: overrides.timeout_secs.unwrap_or(None),
            json_protocol: overrides.json_protocol.unwrap_or(None),
            tool_prefix: overrides.tool_prefix.clone().unwrap_or(None),
            permission_mode: overrides.permission_mode.clone().unwrap_or(None),
            failure_policy: overrides.failure_policy.clone().unwrap_or(None),
        });
    }
    save_hooks_config_file(&path, &cfg)?;
    let env_key = hooks_env_key_for_event(&event).unwrap_or("-");
    if json_mode {
        return Ok(hooks_json_response(
            "hooks_config_set_handler",
            serde_json::json!({
                "path": path.display().to_string(),
                "event": event,
                "env_key": env_key,
                "script": script_trimmed,
                "replaced": replaced,
                "handlers_count": cfg.handlers.len(),
                "timeout_secs": overrides.timeout_secs.unwrap_or(None),
                "json_protocol": overrides.json_protocol.unwrap_or(None),
                "tool_prefix": overrides.tool_prefix.unwrap_or(None),
                "permission_mode": overrides.permission_mode.unwrap_or(None),
                "failure_policy": overrides.failure_policy.unwrap_or(None),
            }),
        ));
    }
    Ok(format!(
        "hooks config set-handler path={} event={} env_key={} replaced={} handlers={} timeout_secs={:?} json_protocol={:?} tool_prefix={:?} permission_mode={:?} failure_policy={:?}",
        path.display(),
        event,
        env_key,
        replaced,
        cfg.handlers.len(),
        overrides.timeout_secs.unwrap_or(None),
        overrides.json_protocol.unwrap_or(None),
        overrides.tool_prefix.unwrap_or(None),
        overrides.permission_mode.unwrap_or(None),
        overrides.failure_policy.unwrap_or(None)
    ))
}

fn hooks_config_edit_handler(
    event_raw: &str,
    script_raw: &str,
    path_hint: Option<&str>,
    new_script: Option<String>,
    overrides: HooksHandlerOverrides,
    json_mode: bool,
) -> Result<String, String> {
    let event = normalize_hooks_event(event_raw).ok_or_else(|| {
        format!(
            "invalid event `{}`; {}",
            event_raw,
            hooks_allowed_events_line()
        )
    })?;
    let script = script_raw.trim().to_string();
    if script.is_empty() {
        return Err("script is empty".to_string());
    }
    let new_script_trimmed = new_script.as_ref().map(|v| v.trim().to_string());
    if let Some(v) = new_script_trimmed.as_ref() {
        if v.is_empty() {
            return Err("new script is empty".to_string());
        }
    }
    if let Some(Some(mode)) = overrides.permission_mode.as_ref() {
        let _ = validate_hooks_permission_mode(mode).ok_or_else(|| {
            "permission_mode must be one of: read-only|workspace-write|on-request|danger-full-access".to_string()
        })?;
    }
    if let Some(Some(fp)) = overrides.failure_policy.as_ref() {
        let _ = validate_hooks_failure_policy(fp)
            .ok_or_else(|| "failure_policy must be fail-open|fail-closed".to_string())?;
    }
    let path = resolve_hooks_config_path(path_hint)?;
    let mut cfg = load_hooks_config_file(&path)?;
    let target_idx = cfg
        .handlers
        .iter()
        .position(|row| row.event == event && row.script.trim() == script);
    let Some(idx) = target_idx else {
        return Err(format!(
            "handler not found for event={} script={}",
            event, script
        ));
    };
    let (changed, updated_script, timeout_secs, json_protocol, tool_prefix, permission_mode, failure_policy) = {
        let row = &mut cfg.handlers[idx];
        let mut changed = false;
        let mut updated_script = script.clone();
        if let Some(ns) = new_script_trimmed.as_ref() {
            if row.script != *ns {
                row.script = ns.clone();
                updated_script = ns.clone();
                changed = true;
            }
        }
        if let Some(v) = overrides.timeout_secs {
            if row.timeout_secs != v {
                row.timeout_secs = v;
                changed = true;
            }
        }
        if let Some(v) = overrides.json_protocol {
            if row.json_protocol != v {
                row.json_protocol = v;
                changed = true;
            }
        }
        if let Some(v) = overrides.tool_prefix.as_ref() {
            if row.tool_prefix != *v {
                row.tool_prefix = v.clone();
                changed = true;
            }
        }
        if let Some(v) = overrides.permission_mode.as_ref() {
            if row.permission_mode != *v {
                row.permission_mode = v.clone();
                changed = true;
            }
        }
        if let Some(v) = overrides.failure_policy.as_ref() {
            if row.failure_policy != *v {
                row.failure_policy = v.clone();
                changed = true;
            }
        }
        (
            changed,
            updated_script,
            row.timeout_secs,
            row.json_protocol,
            row.tool_prefix.clone(),
            row.permission_mode.clone(),
            row.failure_policy.clone(),
        )
    };
    save_hooks_config_file(&path, &cfg)?;
    if json_mode {
        return Ok(hooks_json_response(
            "hooks_config_edit_handler",
            serde_json::json!({
                "path": path.display().to_string(),
                "event": event,
                "script": script,
                "updated_script": updated_script,
                "changed": changed,
                "handlers_count": cfg.handlers.len(),
                "timeout_secs": timeout_secs,
                "json_protocol": json_protocol,
                "tool_prefix": tool_prefix,
                "permission_mode": permission_mode,
                "failure_policy": failure_policy,
            }),
        ));
    }
    return Ok(format!(
        "hooks config edit-handler path={} event={} script={} updated_script={} changed={} handlers={} timeout_secs={:?} json_protocol={:?} tool_prefix={:?} permission_mode={:?} failure_policy={:?}",
        path.display(),
        event,
        script,
        updated_script,
        changed,
        cfg.handlers.len(),
        timeout_secs,
        json_protocol,
        tool_prefix,
        permission_mode,
        failure_policy
    ));
}

#[derive(Debug, Clone, serde::Serialize)]
struct HooksConfigValidationIssue {
    level: String,
    path: String,
    message: String,
}

fn hooks_config_validate(path_hint: Option<&str>, strict_mode: bool, json_mode: bool) -> Result<String, String> {
    let path = resolve_hooks_config_path(path_hint)?;
    let cfg = load_hooks_config_file(&path)?;
    let mut issues: Vec<HooksConfigValidationIssue> = Vec::new();
    let mut seen = std::collections::HashSet::<(String, String)>::new();
    for (idx, row) in cfg.handlers.iter().enumerate() {
        let row_path = format!("handlers[{}]", idx);
        if normalize_hooks_event(&row.event).is_none() {
            issues.push(HooksConfigValidationIssue {
                level: "error".to_string(),
                path: format!("{}.event", row_path),
                message: format!(
                    "invalid event `{}`; {}",
                    row.event,
                    hooks_allowed_events_line()
                ),
            });
        }
        let script_trimmed = row.script.trim();
        if script_trimmed.is_empty() {
            issues.push(HooksConfigValidationIssue {
                level: "error".to_string(),
                path: format!("{}.script", row_path),
                message: "script is empty".to_string(),
            });
        }
        if let Some(mode) = row.permission_mode.as_deref() {
            if validate_hooks_permission_mode(mode).is_none() {
                issues.push(HooksConfigValidationIssue {
                    level: "error".to_string(),
                    path: format!("{}.permission_mode", row_path),
                    message: "permission_mode must be one of: read-only|workspace-write|on-request|danger-full-access".to_string(),
                });
            }
        }
        if let Some(policy) = row.failure_policy.as_deref() {
            if validate_hooks_failure_policy(policy).is_none() {
                issues.push(HooksConfigValidationIssue {
                    level: "error".to_string(),
                    path: format!("{}.failure_policy", row_path),
                    message: "failure_policy must be fail-open|fail-closed".to_string(),
                });
            }
        }
        if let Some(timeout) = row.timeout_secs {
            if timeout == 0 || timeout > 60 {
                issues.push(HooksConfigValidationIssue {
                    level: "error".to_string(),
                    path: format!("{}.timeout_secs", row_path),
                    message: "timeout_secs must be in range 1..=60".to_string(),
                });
            }
        }
        let key = (row.event.clone(), script_trimmed.to_string());
        if !script_trimmed.is_empty() && !seen.insert(key.clone()) {
            issues.push(HooksConfigValidationIssue {
                level: "warning".to_string(),
                path: row_path,
                message: format!(
                    "duplicate handler for event={} script={}",
                    key.0, key.1
                ),
            });
        }
    }
    let errors = issues.iter().filter(|x| x.level == "error").count();
    let warnings = issues.iter().filter(|x| x.level == "warning").count();
    let valid = if strict_mode { errors == 0 && warnings == 0 } else { errors == 0 };
    if json_mode {
        return Ok(hooks_json_response(
            "hooks_config_validate",
            serde_json::json!({
                "path": path.display().to_string(),
                "valid": valid,
                "strict": strict_mode,
                "handlers_count": cfg.handlers.len(),
                "error_count": errors,
                "warning_count": warnings,
                "issues": issues,
            }),
        ));
    }
    let mut lines = vec![format!(
        "hooks config validate path={} valid={} strict={} handlers={} errors={} warnings={}",
        path.display(),
        valid,
        strict_mode,
        cfg.handlers.len(),
        errors,
        warnings
    )];
    for issue in issues {
        lines.push(format!(
            "- level={} path={} message={}",
            issue.level, issue.path, issue.message
        ));
    }
    Ok(lines.join("\n"))
}

fn hooks_config_rm(event_raw: &str, path_hint: Option<&str>, json_mode: bool) -> Result<String, String> {
    let event = normalize_hooks_event(event_raw).ok_or_else(|| {
        format!(
            "invalid event `{}`; {}",
            event_raw,
            hooks_allowed_events_line()
        )
    })?;
    let path = resolve_hooks_config_path(path_hint)?;
    let mut cfg = load_hooks_config_file(&path)?;
    let before = cfg.handlers.len();
    cfg.handlers.retain(|row| row.event != event);
    let changed = cfg.handlers.len() != before;
    save_hooks_config_file(&path, &cfg)?;
    let env_key = hooks_env_key_for_event(&event).unwrap_or("-");
    if json_mode {
        return Ok(hooks_json_response(
            "hooks_config_rm",
            serde_json::json!({
                "path": path.display().to_string(),
                "event": event,
                "env_key": env_key,
                "changed": changed,
                "handlers_count": cfg.handlers.len(),
            }),
        ));
    }
    Ok(format!(
        "hooks config rm path={} event={} env_key={} changed={} handlers={}",
        path.display(),
        event,
        env_key,
        changed,
        cfg.handlers.len()
    ))
}

fn hooks_config_rm_handler(
    event_raw: &str,
    script_raw: &str,
    path_hint: Option<&str>,
    json_mode: bool,
) -> Result<String, String> {
    let event = normalize_hooks_event(event_raw).ok_or_else(|| {
        format!(
            "invalid event `{}`; {}",
            event_raw,
            hooks_allowed_events_line()
        )
    })?;
    let script = script_raw.trim();
    if script.is_empty() {
        return Err("script is empty".to_string());
    }
    let path = resolve_hooks_config_path(path_hint)?;
    let mut cfg = load_hooks_config_file(&path)?;
    let before = cfg.handlers.len();
    cfg.handlers
        .retain(|row| !(row.event == event && row.script.trim() == script));
    let removed = before.saturating_sub(cfg.handlers.len());
    let changed = removed > 0;
    save_hooks_config_file(&path, &cfg)?;
    if json_mode {
        return Ok(hooks_json_response(
            "hooks_config_rm_handler",
            serde_json::json!({
                "path": path.display().to_string(),
                "event": event,
                "script": script,
                "changed": changed,
                "removed": removed,
                "handlers_count": cfg.handlers.len(),
            }),
        ));
    }
    Ok(format!(
        "hooks config rm-handler path={} event={} script={} changed={} removed={} handlers={}",
        path.display(),
        event,
        script,
        changed,
        removed,
        cfg.handlers.len()
    ))
}

fn handle_hooks_config_command(args: &str, json_mode: bool) -> Result<String, String> {
    let mut tokens = parse_cli_tokens(args)?;
    if tokens.is_empty() {
        return Ok(hooks_config_usage_line().to_string());
    }
    let path_hint = parse_hooks_path_flag(&mut tokens)?;
    if tokens.is_empty() {
        return Ok(hooks_config_usage_line().to_string());
    }
    let action = tokens[0].trim().to_ascii_lowercase();
    if action == "show" {
        if tokens.len() != 1 {
            return Ok("Usage: /hooks config show [--path <file.json>] [--json]".to_string());
        }
        return hooks_config_show(path_hint.as_deref(), json_mode);
    }
    if action == "list-handlers" {
        let mut filter = HooksListFilter::default();
        let mut idx = 1usize;
        while idx < tokens.len() {
            let flag = tokens[idx].to_ascii_lowercase();
            if flag == "--event" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_list_handlers_usage_line().to_string())?;
                filter.event = Some(
                    normalize_hooks_event(value).ok_or_else(|| {
                        format!("invalid event `{}`; {}", value, hooks_allowed_events_line())
                    })?,
                );
                idx += 2;
                continue;
            }
            if flag == "--tool-prefix" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_list_handlers_usage_line().to_string())?;
                let v = value.trim().to_string();
                if v.is_empty() {
                    return Ok(hooks_config_list_handlers_usage_line().to_string());
                }
                filter.tool_prefix = Some(v);
                idx += 2;
                continue;
            }
            if flag == "--permission-mode" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_list_handlers_usage_line().to_string())?;
                filter.permission_mode = Some(
                    validate_hooks_permission_mode(value).ok_or_else(|| {
                        "permission_mode must be one of: read-only|workspace-write|on-request|danger-full-access".to_string()
                    })?,
                );
                idx += 2;
                continue;
            }
            return Ok(hooks_config_list_handlers_usage_line().to_string());
        }
        return hooks_config_list_handlers(path_hint.as_deref(), filter, json_mode);
    }
    if action == "export" {
        if tokens.len() != 2 {
            return Ok("Usage: /hooks config export <path.json> [--path <source.json>] [--json]".to_string());
        }
        return hooks_config_export(&tokens[1], path_hint.as_deref(), json_mode);
    }
    if action == "import" {
        if tokens.len() < 2 {
            return Ok("Usage: /hooks config import <path.json> [merge|replace] [--target <file.json>] [--json]".to_string());
        }
        let mut mode: Option<String> = None;
        let mut target: Option<String> = None;
        let mut idx = 2usize;
        while idx < tokens.len() {
            if tokens[idx].eq_ignore_ascii_case("--target") {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| "Usage: /hooks config import <path.json> [merge|replace] [--target <file.json>] [--json]".to_string())?
                    .trim()
                    .to_string();
                if value.is_empty() {
                    return Ok("Usage: /hooks config import <path.json> [merge|replace] [--target <file.json>] [--json]".to_string());
                }
                target = Some(value);
                idx += 2;
                continue;
            }
            let token = tokens[idx].trim().to_ascii_lowercase();
            if token == "merge" || token == "replace" {
                mode = Some(token);
                idx += 1;
                continue;
            }
            return Ok("Usage: /hooks config import <path.json> [merge|replace] [--target <file.json>] [--json]".to_string());
        }
        return hooks_config_import(&tokens[1], mode.as_deref(), target.as_deref().or(path_hint.as_deref()), json_mode);
    }
    if action == "set" {
        if tokens.len() != 3 {
            return Ok("Usage: /hooks config set <event> <script> [--path <file.json>] [--json]".to_string());
        }
        return hooks_config_set(&tokens[1], &tokens[2], path_hint.as_deref(), json_mode);
    }
    if action == "set-handler" {
        if tokens.len() < 3 {
            return Ok(hooks_config_set_handler_usage_line().to_string());
        }
        let mut overrides = HooksHandlerOverrides::default();
        let mut idx = 3usize;
        while idx < tokens.len() {
            let flag = tokens[idx].to_ascii_lowercase();
            if flag == "--timeout-secs" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_set_handler_usage_line().to_string())?;
                if value.trim().eq_ignore_ascii_case("none") {
                    overrides.timeout_secs = Some(None);
                    idx += 2;
                    continue;
                }
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| "timeout_secs must be a positive integer".to_string())?;
                overrides.timeout_secs = Some(Some(parsed.clamp(1, 60)));
                idx += 2;
                continue;
            }
            if flag == "--json-protocol" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_set_handler_usage_line().to_string())?;
                if value.trim().eq_ignore_ascii_case("none") {
                    overrides.json_protocol = Some(None);
                    idx += 2;
                    continue;
                }
                overrides.json_protocol = Some(
                    Some(validate_hooks_json_protocol(value)
                        .ok_or_else(|| "json_protocol must be on|off".to_string())?,
                    ),
                );
                idx += 2;
                continue;
            }
            if flag == "--tool-prefix" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_set_handler_usage_line().to_string())?;
                let parsed = parse_hooks_optional_value(value);
                if parsed.as_deref() == Some("") {
                    return Err("tool_prefix cannot be empty".to_string());
                }
                overrides.tool_prefix = Some(parsed);
                idx += 2;
                continue;
            }
            if flag == "--permission-mode" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_set_handler_usage_line().to_string())?;
                let parsed = parse_hooks_optional_value(value);
                if parsed.as_deref() == Some("") {
                    return Err("permission_mode cannot be empty".to_string());
                }
                if let Some(v) = parsed.as_ref() {
                    let mode = validate_hooks_permission_mode(v).ok_or_else(|| {
                        "permission_mode must be one of: read-only|workspace-write|on-request|danger-full-access".to_string()
                    })?;
                    overrides.permission_mode = Some(Some(mode));
                } else {
                    overrides.permission_mode = Some(None);
                }
                idx += 2;
                continue;
            }
            if flag == "--failure-policy" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_set_handler_usage_line().to_string())?;
                let parsed = parse_hooks_optional_value(value);
                if parsed.as_deref() == Some("") {
                    return Err("failure_policy cannot be empty".to_string());
                }
                if let Some(v) = parsed.as_ref() {
                    let policy = validate_hooks_failure_policy(v)
                        .ok_or_else(|| "failure_policy must be fail-open|fail-closed".to_string())?;
                    overrides.failure_policy = Some(Some(policy));
                } else {
                    overrides.failure_policy = Some(None);
                }
                idx += 2;
                continue;
            }
            return Ok(hooks_config_set_handler_usage_line().to_string());
        }
        return hooks_config_set_handler(&tokens[1], &tokens[2], path_hint.as_deref(), overrides, json_mode);
    }
    if action == "edit-handler" {
        if tokens.len() < 3 {
            return Ok(hooks_config_edit_handler_usage_line().to_string());
        }
        let mut overrides = HooksHandlerOverrides::default();
        let mut new_script: Option<String> = None;
        let mut idx = 3usize;
        while idx < tokens.len() {
            let flag = tokens[idx].to_ascii_lowercase();
            if flag == "--new-script" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_edit_handler_usage_line().to_string())?;
                let v = value.trim().to_string();
                if v.is_empty() {
                    return Err("new script is empty".to_string());
                }
                new_script = Some(v);
                idx += 2;
                continue;
            }
            if flag == "--timeout-secs" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_edit_handler_usage_line().to_string())?;
                if value.trim().eq_ignore_ascii_case("none") {
                    overrides.timeout_secs = Some(None);
                    idx += 2;
                    continue;
                }
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| "timeout_secs must be a positive integer".to_string())?;
                overrides.timeout_secs = Some(Some(parsed.clamp(1, 60)));
                idx += 2;
                continue;
            }
            if flag == "--json-protocol" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_edit_handler_usage_line().to_string())?;
                if value.trim().eq_ignore_ascii_case("none") {
                    overrides.json_protocol = Some(None);
                    idx += 2;
                    continue;
                }
                overrides.json_protocol = Some(
                    Some(validate_hooks_json_protocol(value)
                        .ok_or_else(|| "json_protocol must be on|off".to_string())?,
                    ),
                );
                idx += 2;
                continue;
            }
            if flag == "--tool-prefix" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_edit_handler_usage_line().to_string())?;
                let parsed = parse_hooks_optional_value(value);
                if parsed.as_deref() == Some("") {
                    return Err("tool_prefix cannot be empty".to_string());
                }
                overrides.tool_prefix = Some(parsed);
                idx += 2;
                continue;
            }
            if flag == "--permission-mode" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_edit_handler_usage_line().to_string())?;
                let parsed = parse_hooks_optional_value(value);
                if parsed.as_deref() == Some("") {
                    return Err("permission_mode cannot be empty".to_string());
                }
                if let Some(v) = parsed.as_ref() {
                    let mode = validate_hooks_permission_mode(v).ok_or_else(|| {
                        "permission_mode must be one of: read-only|workspace-write|on-request|danger-full-access".to_string()
                    })?;
                    overrides.permission_mode = Some(Some(mode));
                } else {
                    overrides.permission_mode = Some(None);
                }
                idx += 2;
                continue;
            }
            if flag == "--failure-policy" {
                let value = tokens
                    .get(idx + 1)
                    .ok_or_else(|| hooks_config_edit_handler_usage_line().to_string())?;
                let parsed = parse_hooks_optional_value(value);
                if parsed.as_deref() == Some("") {
                    return Err("failure_policy cannot be empty".to_string());
                }
                if let Some(v) = parsed.as_ref() {
                    let policy = validate_hooks_failure_policy(v)
                        .ok_or_else(|| "failure_policy must be fail-open|fail-closed".to_string())?;
                    overrides.failure_policy = Some(Some(policy));
                } else {
                    overrides.failure_policy = Some(None);
                }
                idx += 2;
                continue;
            }
            return Ok(hooks_config_edit_handler_usage_line().to_string());
        }
        return hooks_config_edit_handler(
            &tokens[1],
            &tokens[2],
            path_hint.as_deref(),
            new_script,
            overrides,
            json_mode,
        );
    }
    if action == "rm" {
        if tokens.len() != 2 {
            return Ok("Usage: /hooks config rm <event> [--path <file.json>] [--json]".to_string());
        }
        return hooks_config_rm(&tokens[1], path_hint.as_deref(), json_mode);
    }
    if action == "rm-handler" {
        if tokens.len() != 3 {
            return Ok("Usage: /hooks config rm-handler <event> <script> [--path <file.json>] [--json]".to_string());
        }
        return hooks_config_rm_handler(&tokens[1], &tokens[2], path_hint.as_deref(), json_mode);
    }
    if action == "validate" {
        let mut strict_mode = false;
        let mut idx = 1usize;
        while idx < tokens.len() {
            let flag = tokens[idx].to_ascii_lowercase();
            if flag == "--strict" {
                strict_mode = true;
                idx += 1;
                continue;
            }
            return Ok(hooks_config_validate_usage_line().to_string());
        }
        return hooks_config_validate(path_hint.as_deref(), strict_mode, json_mode);
    }
    Ok(hooks_config_usage_line().to_string())
}

fn hooks_status_payload() -> serde_json::Value {
    let enabled = std::env::var("ASI_HOOKS_ENABLED")
        .ok()
        .and_then(|v| parse_on_off(&v))
        .unwrap_or(false);
    let timeout_secs = std::env::var("ASI_HOOK_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(15)
        .clamp(1, 60);
    let json_protocol = std::env::var("ASI_HOOK_JSON")
        .ok()
        .and_then(|v| parse_on_off(&v))
        .unwrap_or(true);
    let failure_policy = std::env::var("ASI_HOOK_FAILURE_POLICY")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| v == "fail-open" || v == "open" || v == "fail-closed" || v == "closed")
        .map(|v| if v == "open" { "fail-open".to_string() } else if v == "closed" { "fail-closed".to_string() } else { v })
        .unwrap_or_else(|| "fail-closed".to_string());
    let config_path = std::env::var("ASI_HOOK_CONFIG_PATH")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());

    let mut env_events = Vec::new();
    let env_pairs = [
        ("PreToolUse", "ASI_HOOK_PRE_TOOL_USE"),
        ("PermissionRequest", "ASI_HOOK_PERMISSION_REQUEST"),
        ("PostToolUse", "ASI_HOOK_POST_TOOL_USE"),
        ("SessionStart", "ASI_HOOK_SESSION_START"),
        ("UserPromptSubmit", "ASI_HOOK_USER_PROMPT_SUBMIT"),
        ("Stop", "ASI_HOOK_STOP"),
        ("SubagentStop", "ASI_HOOK_SUBAGENT_STOP"),
        ("PreCompact", "ASI_HOOK_PRE_COMPACT"),
        ("PostCompact", "ASI_HOOK_POST_COMPACT"),
    ];
    for (event, key) in env_pairs {
        if std::env::var(key)
            .ok()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
        {
            env_events.push(event.to_string());
        }
    }
    let configured_file_handlers = if let Ok(path) = resolve_hooks_config_path(None) {
        load_hooks_config_file(&path)
            .map(|cfg| cfg.handlers.len())
            .unwrap_or(0)
    } else {
        0
    };
    let configured_file_handlers_by_event = if let Ok(path) = resolve_hooks_config_path(None) {
        if let Ok(cfg) = load_hooks_config_file(&path) {
            let mut map = std::collections::BTreeMap::<String, usize>::new();
            for row in cfg.handlers {
                let key = row.event.trim().to_string();
                if key.is_empty() {
                    continue;
                }
                *map.entry(key).or_insert(0) += 1;
            }
            map
        } else {
            std::collections::BTreeMap::new()
        }
    } else {
        std::collections::BTreeMap::new()
    };
    let plugin_enabled_trusted_count = plugin::verify_enabled_plugins().len();
    let plugin_hook_total = {
        let events = [
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "SessionStart",
            "UserPromptSubmit",
            "Stop",
            "SubagentStop",
            "PreCompact",
            "PostCompact",
        ];
        events
            .iter()
            .map(|e| runtime::collect_hook_diagnostics(e, "runtime", "status_probe", "on-request").len())
            .sum::<usize>()
    };
    let plugin_hook_estimated = plugin_hook_total.saturating_sub(configured_file_handlers);

    let diagnostics =
        runtime::collect_hook_diagnostics("SessionStart", "runtime", "hooks_status_probe", "on-request");
    let diagnostics_denied = diagnostics.iter().filter(|d| !d.allow).count();
    let diagnostics_errors = diagnostics.iter().filter(|d| d.is_error).count();

    serde_json::json!({
        "enabled": enabled,
        "timeout_secs": timeout_secs,
        "json_protocol": json_protocol,
        "failure_policy": failure_policy,
        "config_path": config_path,
        "env_event_count": env_events.len(),
        "env_events": env_events,
        "configured_file_handlers": configured_file_handlers,
        "configured_file_handlers_by_event": configured_file_handlers_by_event,
        "plugin_enabled_trusted_count": plugin_enabled_trusted_count,
        "plugin_hook_estimated": plugin_hook_estimated,
        "diagnostics_count": diagnostics.len(),
        "diagnostics_denied": diagnostics_denied,
        "diagnostics_errors": diagnostics_errors,
        "diagnostics": diagnostics,
    })
}

fn validate_hooks_permission_mode(raw: &str) -> Option<String> {
    let mode = normalize_permission_mode(raw);
    if matches!(
        mode.as_str(),
        "read-only" | "workspace-write" | "on-request" | "danger-full-access"
    ) {
        Some(mode)
    } else {
        None
    }
}

fn parse_hooks_json_mode(raw: &str) -> (String, bool) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return (String::new(), false);
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.is_empty() {
        return (String::new(), false);
    }
    let json_mode = tokens.iter().any(|t| t.eq_ignore_ascii_case("--json"));
    if !json_mode {
        return (trimmed.to_string(), false);
    }
    let filtered = tokens
        .into_iter()
        .filter(|t| !t.eq_ignore_ascii_case("--json"))
        .collect::<Vec<_>>();
    (filtered.join(" "), true)
}

fn parse_hooks_test_options(tokens: &[String]) -> Result<(String, String, String), String> {
    let mut tool = "runtime".to_string();
    let mut args = "hooks_test_probe".to_string();
    let mut mode = "on-request".to_string();
    let mut idx = 0usize;
    while idx < tokens.len() {
        let token = tokens[idx].as_str();
        if token.eq_ignore_ascii_case("--tool") {
            let value = tokens
                .get(idx + 1)
                .ok_or_else(|| {
                    "Usage: /hooks test <event> [--tool <name>] [--args <text>] [--mode <read-only|workspace-write|on-request|danger-full-access>] [--json]".to_string()
                })?
                .trim();
            if value.is_empty() {
                return Err(
                    "Usage: /hooks test <event> [--tool <name>] [--args <text>] [--mode <read-only|workspace-write|on-request|danger-full-access>] [--json]".to_string(),
                );
            }
            tool = value.to_string();
            idx += 2;
            continue;
        }
        if token.eq_ignore_ascii_case("--args") {
            let value = tokens
                .get(idx + 1)
                .ok_or_else(|| {
                    "Usage: /hooks test <event> [--tool <name>] [--args <text>] [--mode <read-only|workspace-write|on-request|danger-full-access>] [--json]".to_string()
                })?;
            args = value.to_string();
            idx += 2;
            continue;
        }
        if token.eq_ignore_ascii_case("--mode") {
            let value = tokens
                .get(idx + 1)
                .ok_or_else(|| {
                    "Usage: /hooks test <event> [--tool <name>] [--args <text>] [--mode <read-only|workspace-write|on-request|danger-full-access>] [--json]".to_string()
                })?;
            mode = validate_hooks_permission_mode(value).ok_or_else(|| {
                "mode must be one of: read-only|workspace-write|on-request|danger-full-access"
                    .to_string()
            })?;
            idx += 2;
            continue;
        }
        return Err(
            "Usage: /hooks test <event> [--tool <name>] [--args <text>] [--mode <read-only|workspace-write|on-request|danger-full-access>] [--json]".to_string(),
        );
    }
    Ok((tool, args, mode))
}

pub(crate) fn handle_hooks_command(args: &str) -> Result<String, String> {
    let (trimmed, json_mode) = parse_hooks_json_mode(args.trim());
    if let Some(rest) = trimmed.strip_prefix("config ") {
        return handle_hooks_config_command(rest, json_mode);
    }
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("status") {
        let payload = hooks_status_payload();
        if json_mode {
            return Ok(hooks_json_response("hooks_status", payload));
        }
        let enabled = payload
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let timeout_secs = payload
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(15);
        let json_protocol = payload
            .get("json_protocol")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let failure_policy = payload
            .get("failure_policy")
            .and_then(|v| v.as_str())
            .unwrap_or("fail-closed");
        let env_event_count = payload
            .get("env_event_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let configured_file_handlers = payload
            .get("configured_file_handlers")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let plugin_enabled_trusted_count = payload
            .get("plugin_enabled_trusted_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let plugin_hook_estimated = payload
            .get("plugin_hook_estimated")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let diagnostics_count = payload
            .get("diagnostics_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let diagnostics_denied = payload
            .get("diagnostics_denied")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let diagnostics_errors = payload
            .get("diagnostics_errors")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let config_path = payload
            .get("config_path")
            .and_then(|v| v.as_str())
            .unwrap_or("<unset>");
        let env_events = payload
            .get("env_events")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let mut lines = vec![
            format!(
                "hooks enabled={} timeout_secs={} json_protocol={} failure_policy={} config_path={}",
                enabled, timeout_secs, json_protocol, failure_policy, config_path
            ),
            format!(
                "hooks env_event_count={} configured_file_handlers={} plugin_enabled_trusted_count={} plugin_hook_estimated={} diagnostics_count={} diagnostics_denied={} diagnostics_errors={}",
                env_event_count, configured_file_handlers, plugin_enabled_trusted_count, plugin_hook_estimated, diagnostics_count, diagnostics_denied, diagnostics_errors
            ),
        ];
        if !env_events.is_empty() {
            lines.push(format!("hooks env_events={}", env_events));
        }
        if let Some(by_event) = payload
            .get("configured_file_handlers_by_event")
            .and_then(|v| v.as_object())
        {
            if !by_event.is_empty() {
                let mut pairs = Vec::new();
                for (k, v) in by_event {
                    pairs.push(format!("{}={}", k, v.as_u64().unwrap_or(0)));
                }
                lines.push(format!("hooks file_handlers_by_event={}", pairs.join(", ")));
            }
        }
        if let Some(rows) = payload.get("diagnostics").and_then(|v| v.as_array()) {
            for row in rows.iter().take(5) {
                lines.push(format!(
                    "hook source={} event={} allow={} is_error={} reason={}",
                    row.get("source").and_then(|v| v.as_str()).unwrap_or("-"),
                    row.get("event").and_then(|v| v.as_str()).unwrap_or("-"),
                    row.get("allow").and_then(|v| v.as_bool()).unwrap_or(true),
                    row.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false),
                    row.get("reason").and_then(|v| v.as_str()).unwrap_or("")
                ));
            }
        }
        return Ok(lines.join("\n"));
    }

    if let Some(rest) = trimmed.strip_prefix("test ") {
        let tokens = parse_cli_tokens(rest)?;
        if tokens.is_empty() {
            return Ok(format!(
                "Usage: /hooks test <event> [--tool <name>] [--args <text>] [--mode <read-only|workspace-write|on-request|danger-full-access>] [--json]\n{}",
                hooks_allowed_events_line()
            ));
        }
        let event = normalize_hooks_event(&tokens[0]).ok_or_else(|| {
            format!(
                "invalid event `{}`; {}",
                tokens[0],
                hooks_allowed_events_line()
            )
        })?;
        let (tool, args_value, mode) = parse_hooks_test_options(&tokens[1..])?;
        let diagnostics = runtime::collect_hook_diagnostics(&event, &tool, &args_value, &mode);
        let denied = diagnostics.iter().filter(|d| !d.allow).count();
        let errors = diagnostics.iter().filter(|d| d.is_error).count();
        let payload = serde_json::json!({
            "event": event,
            "tool": tool,
            "args": args_value,
            "mode": mode,
            "diagnostics_count": diagnostics.len(),
            "diagnostics_denied": denied,
            "diagnostics_errors": errors,
            "diagnostics": diagnostics,
        });
        if json_mode {
            return Ok(hooks_json_response("hooks_test", payload));
        }
        let mut lines = vec![format!(
            "hooks test event={} tool={} mode={} diagnostics_count={} diagnostics_denied={} diagnostics_errors={}",
            payload.get("event").and_then(|v| v.as_str()).unwrap_or("-"),
            payload.get("tool").and_then(|v| v.as_str()).unwrap_or("-"),
            payload.get("mode").and_then(|v| v.as_str()).unwrap_or("-"),
            payload
                .get("diagnostics_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            payload
                .get("diagnostics_denied")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            payload
                .get("diagnostics_errors")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
        )];
        if let Some(rows) = payload.get("diagnostics").and_then(|v| v.as_array()) {
            for row in rows.iter().take(8) {
                lines.push(format!(
                    "hook source={} event={} allow={} is_error={} reason={}",
                    row.get("source").and_then(|v| v.as_str()).unwrap_or("-"),
                    row.get("event").and_then(|v| v.as_str()).unwrap_or("-"),
                    row.get("allow").and_then(|v| v.as_bool()).unwrap_or(true),
                    row.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false),
                    row.get("reason").and_then(|v| v.as_str()).unwrap_or("")
                ));
            }
        }
        return Ok(lines.join("\n"));
    }

    Ok(format!(
        "Usage: /hooks status [--json] | /hooks test <event> [--tool <name>] [--args <text>] [--mode <read-only|workspace-write|on-request|danger-full-access>] [--json] | /hooks config show [--path <file.json>] [--json] | /hooks config list-handlers [--path <file.json>] [--event <event>] [--tool-prefix <tool>] [--permission-mode <mode>] [--json] | /hooks config export <path.json> [--path <source.json>] [--json] | /hooks config import <path.json> [merge|replace] [--target <file.json>] [--json] | /hooks config set <event> <script> [--path <file.json>] [--json] | /hooks config set-handler <event> <script> [--path <file.json>] [--timeout-secs <n|none>] [--json-protocol <on|off|none>] [--tool-prefix <tool|none>] [--permission-mode <read-only|workspace-write|on-request|danger-full-access|none>] [--failure-policy <fail-open|fail-closed|none>] [--json] | /hooks config edit-handler <event> <script> [--path <file.json>] [--new-script <script>] [--timeout-secs <n|none>] [--json-protocol <on|off|none>] [--tool-prefix <tool|none>] [--permission-mode <read-only|workspace-write|on-request|danger-full-access|none>] [--failure-policy <fail-open|fail-closed|none>] [--json] | /hooks config rm <event> [--path <file.json>] [--json] | /hooks config rm-handler <event> <script> [--path <file.json>] [--json] | /hooks config validate [--path <file.json>] [--strict] [--json]\n{}",
        hooks_allowed_events_line()
    ))
}

pub(crate) fn hooks_cli_to_repl_args(cmd: HooksCliCommand) -> String {
    match cmd {
        HooksCliCommand::Status { json } => append_json_flag("status".to_string(), json),
        HooksCliCommand::Test {
            event,
            tool,
            args,
            mode,
            json,
        } => {
            let mut s = format!("test {}", event);
            if let Some(v) = tool {
                s.push_str(" --tool ");
                s.push_str(&v);
            }
            if let Some(v) = args {
                s.push_str(" --args ");
                if v.contains(' ') {
                    s.push('"');
                    s.push_str(&v.replace('"', "\\\""));
                    s.push('"');
                } else {
                    s.push_str(&v);
                }
            }
            if let Some(v) = mode {
                s.push_str(" --mode ");
                s.push_str(&v);
            }
            append_json_flag(s, json)
        }
        HooksCliCommand::Config { action } => match action {
            HooksCliConfigCommand::Show { path, json } => {
                let mut s = "config show".to_string();
                if let Some(v) = path {
                    s.push_str(" --path ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                append_json_flag(s, json)
            }
            HooksCliConfigCommand::ListHandlers {
                path,
                event,
                tool_prefix,
                permission_mode,
                json,
            } => {
                let mut s = "config list-handlers".to_string();
                if let Some(v) = path {
                    s.push_str(" --path ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                if let Some(v) = event {
                    s.push_str(" --event ");
                    s.push_str(&v);
                }
                if let Some(v) = tool_prefix {
                    s.push_str(" --tool-prefix ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                if let Some(v) = permission_mode {
                    s.push_str(" --permission-mode ");
                    s.push_str(&v);
                }
                append_json_flag(s, json)
            }
            HooksCliConfigCommand::Export {
                path,
                source_path,
                json,
            } => {
                let mut s = format!("config export {}", path);
                if let Some(v) = source_path {
                    s.push_str(" --path ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                append_json_flag(s, json)
            }
            HooksCliConfigCommand::Import {
                path,
                mode,
                target,
                json,
            } => {
                let mut s = format!("config import {}", path);
                if let Some(m) = mode {
                    s.push(' ');
                    s.push_str(&m);
                }
                if let Some(v) = target {
                    s.push_str(" --target ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                append_json_flag(s, json)
            }
            HooksCliConfigCommand::Set {
                event,
                script,
                path,
                json,
            } => {
                let mut s = format!("config set {} ", event);
                if script.contains(' ') {
                    s.push('"');
                    s.push_str(&script.replace('"', "\\\""));
                    s.push('"');
                } else {
                    s.push_str(&script);
                }
                if let Some(v) = path {
                    s.push_str(" --path ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                append_json_flag(s, json)
            }
            HooksCliConfigCommand::SetHandler {
                event,
                script,
                path,
                timeout_secs,
                json_protocol,
                tool_prefix,
                permission_mode,
                failure_policy,
                json,
            } => {
                let mut s = format!("config set-handler {} ", event);
                if script.contains(' ') {
                    s.push('"');
                    s.push_str(&script.replace('"', "\\\""));
                    s.push('"');
                } else {
                    s.push_str(&script);
                }
                if let Some(v) = path {
                    s.push_str(" --path ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                if let Some(v) = timeout_secs {
                    s.push_str(" --timeout-secs ");
                    s.push_str(&v);
                }
                if let Some(v) = json_protocol {
                    s.push_str(" --json-protocol ");
                    s.push_str(&v);
                }
                if let Some(v) = tool_prefix {
                    s.push_str(" --tool-prefix ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                if let Some(v) = permission_mode {
                    s.push_str(" --permission-mode ");
                    s.push_str(&v);
                }
                if let Some(v) = failure_policy {
                    s.push_str(" --failure-policy ");
                    s.push_str(&v);
                }
                append_json_flag(s, json)
            }
            HooksCliConfigCommand::EditHandler {
                event,
                script,
                path,
                new_script,
                timeout_secs,
                json_protocol,
                tool_prefix,
                permission_mode,
                failure_policy,
                json,
            } => {
                let mut s = format!("config edit-handler {} ", event);
                if script.contains(' ') {
                    s.push('"');
                    s.push_str(&script.replace('"', "\\\""));
                    s.push('"');
                } else {
                    s.push_str(&script);
                }
                if let Some(v) = path {
                    s.push_str(" --path ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                if let Some(v) = new_script {
                    s.push_str(" --new-script ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                if let Some(v) = timeout_secs {
                    s.push_str(" --timeout-secs ");
                    s.push_str(&v);
                }
                if let Some(v) = json_protocol {
                    s.push_str(" --json-protocol ");
                    s.push_str(&v);
                }
                if let Some(v) = tool_prefix {
                    s.push_str(" --tool-prefix ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                if let Some(v) = permission_mode {
                    s.push_str(" --permission-mode ");
                    s.push_str(&v);
                }
                if let Some(v) = failure_policy {
                    s.push_str(" --failure-policy ");
                    s.push_str(&v);
                }
                append_json_flag(s, json)
            }
            HooksCliConfigCommand::Rm { event, path, json } => {
                let mut s = format!("config rm {}", event);
                if let Some(v) = path {
                    s.push_str(" --path ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                append_json_flag(s, json)
            }
            HooksCliConfigCommand::RmHandler {
                event,
                script,
                path,
                json,
            } => {
                let mut s = format!("config rm-handler {} ", event);
                if script.contains(' ') {
                    s.push('"');
                    s.push_str(&script.replace('"', "\\\""));
                    s.push('"');
                } else {
                    s.push_str(&script);
                }
                if let Some(v) = path {
                    s.push_str(" --path ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                append_json_flag(s, json)
            }
            HooksCliConfigCommand::Validate { path, strict, json } => {
                let mut s = "config validate".to_string();
                if let Some(v) = path {
                    s.push_str(" --path ");
                    if v.contains(' ') {
                        s.push('"');
                        s.push_str(&v.replace('"', "\\\""));
                        s.push('"');
                    } else {
                        s.push_str(&v);
                    }
                }
                if strict {
                    s.push_str(" --strict");
                }
                append_json_flag(s, json)
            }
        },
    }
}

pub(crate) fn plugin_cli_to_repl_args(cmd: PluginCliCommand) -> String {
    match cmd {
        PluginCliCommand::List { json } => append_json_flag("list".to_string(), json),
        PluginCliCommand::Add { name, path, json } => {
            append_json_flag(format!("add {} {}", name, path), json)
        }
        PluginCliCommand::Rm { name, json } => append_json_flag(format!("rm {}", name), json),
        PluginCliCommand::Show { name, json } => append_json_flag(format!("show {}", name), json),
        PluginCliCommand::Enable { name, json } => {
            append_json_flag(format!("enable {}", name), json)
        }
        PluginCliCommand::Disable { name, json } => {
            append_json_flag(format!("disable {}", name), json)
        }
        PluginCliCommand::Trust {
            name,
            mode,
            hash,
            json,
        } => {
            let mut s = format!("trust {} {}", name, mode);
            if let Some(h) = hash {
                s.push(' ');
                s.push_str(&h);
            }
            append_json_flag(s, json)
        }
        PluginCliCommand::Verify { name, json } => {
            append_json_flag(format!("verify {}", name), json)
        }
        PluginCliCommand::Config { action } => match action {
            PluginCliConfigCommand::Set {
                name,
                key,
                value,
                json,
            } => append_json_flag(format!("config set {} {} {}", name, key, value), json),
            PluginCliConfigCommand::Rm { name, key, json } => {
                append_json_flag(format!("config rm {} {}", name, key), json)
            }
        },
        PluginCliCommand::Export { path, json, .. } => {
            append_json_flag(format!("export {}", path), json)
        }
        PluginCliCommand::Import { path, mode, json } => {
            let mut s = format!("import {}", path);
            if let Some(m) = mode {
                s.push(' ');
                s.push_str(&m);
            }
            append_json_flag(s, json)
        }
        PluginCliCommand::Market { json } => append_json_flag("market".to_string(), json),
    }
}

fn remove_rule(vec: &mut Vec<String>, rule: &str) -> bool {
    let before = vec.len();
    vec.retain(|x| x != rule);
    vec.len() != before
}

fn add_unique_dir(vec: &mut Vec<PathBuf>, dir: PathBuf) {
    if !vec.iter().any(|x| x == &dir) {
        vec.push(dir);
    }
}

fn remove_dir(vec: &mut Vec<PathBuf>, dir: &PathBuf) -> bool {
    let before = vec.len();
    vec.retain(|x| x != dir);
    vec.len() != before
}

fn permissions_status(cfg: &AppConfig) -> String {
    let allow = if cfg.permission_allow_rules.is_empty() {
        "<empty>".to_string()
    } else {
        cfg.permission_allow_rules.join(", ")
    };
    let deny = if cfg.permission_deny_rules.is_empty() {
        "<empty>".to_string()
    } else {
        cfg.permission_deny_rules.join(", ")
    };
    let dirs = if cfg.additional_directories.is_empty() {
        "<empty>".to_string()
    } else {
        cfg.additional_directories.join(", ")
    };
    format!(
        "persistent mode={} path_restriction={} auto_review_mode={} auto_review_threshold={} allow_rules=[{}] deny_rules=[{}] additional_dirs=[{}]",
        cfg.permission_mode,
        cfg.path_restriction_enabled,
        cfg.auto_review_mode,
        cfg.auto_review_severity_threshold,
        allow,
        deny,
        dirs
    )
}

fn session_permissions_status(rt: &Runtime) -> String {
    let allow = if rt.session_permission_allow_rules.is_empty() {
        "<empty>".to_string()
    } else {
        rt.session_permission_allow_rules.join(", ")
    };
    let deny = if rt.session_permission_deny_rules.is_empty() {
        "<empty>".to_string()
    } else {
        rt.session_permission_deny_rules.join(", ")
    };
    let dirs = if rt.session_additional_directories.is_empty() {
        "<empty>".to_string()
    } else {
        rt.session_additional_directories
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let once_allow = if rt.next_permission_allow_rules.is_empty() {
        "<empty>".to_string()
    } else {
        rt.next_permission_allow_rules.join(", ")
    };
    let once_dirs = if rt.next_additional_directories.is_empty() {
        "<empty>".to_string()
    } else {
        rt.next_additional_directories
            .iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "session allow_rules=[{}] deny_rules=[{}] additional_dirs=[{}] once_allow=[{}] once_dirs=[{}]",
        allow, deny, dirs, once_allow, once_dirs
    )
}
fn handle_permissions_command(
    cfg: &mut AppConfig,
    rt: &mut Runtime,
    rest: &str,
) -> Result<String, String> {
    let trimmed = rest.trim();
    if trimmed.is_empty() || trimmed == "list" {
        return Ok(format!(
            "{}\n{}",
            permissions_status(cfg),
            session_permissions_status(rt)
        ));
    }

    if let Some(mode) = trimmed.strip_prefix("mode ") {
        cfg.permission_mode = mode.trim().to_string();
        rt.permission_mode = cfg.permission_mode.clone();
        let _ = cfg.save();
        return Ok(format!("permission_mode={}", cfg.permission_mode));
    }

    if let Some(mode) = trimmed.strip_prefix("auto-review ") {
        cfg.auto_review_mode = normalize_auto_review_mode(mode);
        apply_runtime_flags_from_cfg(rt, cfg);
        let _ = cfg.save();
        return Ok(format!("auto_review_mode={}", cfg.auto_review_mode));
    }

    if let Some(level) = trimmed.strip_prefix("auto-review-threshold ") {
        cfg.auto_review_severity_threshold = normalize_auto_review_severity_threshold(level);
        apply_runtime_flags_from_cfg(rt, cfg);
        let _ = cfg.save();
        return Ok(format!(
            "auto_review_severity_threshold={}",
            cfg.auto_review_severity_threshold
        ));
    }

    if let Some(v) = trimmed.strip_prefix("path-restriction ") {
        let enabled = parse_on_off(v)
            .ok_or_else(|| "Usage: /permissions path-restriction on|off".to_string())?;
        cfg.path_restriction_enabled = enabled;
        apply_runtime_flags_from_cfg(rt, cfg);
        let _ = cfg.save();
        return Ok(format!(
            "path_restriction_enabled={}",
            cfg.path_restriction_enabled
        ));
    }

    if trimmed == "dirs" {
        if cfg.additional_directories.is_empty() {
            return Ok("persistent additional_dirs=<empty>".to_string());
        }
        return Ok(cfg
            .additional_directories
            .iter()
            .map(|d| format!("- {}", d))
            .collect::<Vec<_>>()
            .join("\n"));
    }

    if let Some(path) = trimmed.strip_prefix("add-dir ") {
        let dir = normalize_directory(path)?;
        add_unique_rule(&mut cfg.additional_directories, dir.clone());
        apply_runtime_flags_from_cfg(rt, cfg);
        let _ = cfg.save();
        return Ok(format!("persistent additional directory added: {}", dir));
    }

    if let Some(path) = trimmed.strip_prefix("rm-dir ") {
        let dir = normalize_directory(path)?;
        let changed = remove_rule(&mut cfg.additional_directories, &dir);
        apply_runtime_flags_from_cfg(rt, cfg);
        let _ = cfg.save();
        return Ok(format!(
            "persistent additional directory removed={} path={}",
            changed, dir
        ));
    }

    if trimmed == "clear-dirs" {
        cfg.additional_directories.clear();
        apply_runtime_flags_from_cfg(rt, cfg);
        let _ = cfg.save();
        return Ok("persistent additional directories cleared".to_string());
    }

    if trimmed == "clear-rules" {
        cfg.permission_allow_rules.clear();
        cfg.permission_deny_rules.clear();
        apply_runtime_flags_from_cfg(rt, cfg);
        let _ = cfg.save();
        return Ok("persistent permission rules cleared".to_string());
    }

    if let Some(rule) = trimmed.strip_prefix("allow ") {
        let rule = normalize_rule(rule)?;
        add_unique_rule(&mut cfg.permission_allow_rules, rule.clone());
        apply_runtime_flags_from_cfg(rt, cfg);
        let _ = cfg.save();
        return Ok(format!("persistent allow rule added: {}", rule));
    }

    if let Some(rule) = trimmed.strip_prefix("deny ") {
        let rule = normalize_rule(rule)?;
        add_unique_rule(&mut cfg.permission_deny_rules, rule.clone());
        apply_runtime_flags_from_cfg(rt, cfg);
        let _ = cfg.save();
        return Ok(format!("persistent deny rule added: {}", rule));
    }

    if let Some(rule) = trimmed.strip_prefix("rm-allow ") {
        let rule = normalize_rule(rule)?;
        let changed = remove_rule(&mut cfg.permission_allow_rules, &rule);
        apply_runtime_flags_from_cfg(rt, cfg);
        let _ = cfg.save();
        return Ok(format!(
            "persistent allow rule removed={} rule={}",
            changed, rule
        ));
    }

    if let Some(rule) = trimmed.strip_prefix("rm-deny ") {
        let rule = normalize_rule(rule)?;
        let changed = remove_rule(&mut cfg.permission_deny_rules, &rule);
        apply_runtime_flags_from_cfg(rt, cfg);
        let _ = cfg.save();
        return Ok(format!(
            "persistent deny rule removed={} rule={}",
            changed, rule
        ));
    }

    if trimmed == "temp-list" {
        return Ok(session_permissions_status(rt));
    }

    if trimmed == "temp-next-clear" {
        rt.next_permission_allow_rules.clear();
        rt.next_additional_directories.clear();
        return Ok("next-only session permissions cleared".to_string());
    }

    if let Some(rule) = trimmed.strip_prefix("temp-next-allow ") {
        let rule = normalize_rule(rule)?;
        add_unique_rule(&mut rt.next_permission_allow_rules, rule.clone());
        return Ok(format!("next-only allow rule added: {}", rule));
    }

    if let Some(path) = trimmed.strip_prefix("temp-next-add-dir ") {
        let dir = PathBuf::from(normalize_directory(path)?);
        add_unique_dir(&mut rt.next_additional_directories, dir.clone());
        return Ok(format!(
            "next-only additional directory added: {}",
            dir.display()
        ));
    }

    if trimmed == "temp-clear" {
        rt.session_permission_allow_rules.clear();
        rt.session_permission_deny_rules.clear();
        return Ok("session permission rules cleared".to_string());
    }

    if let Some(rule) = trimmed.strip_prefix("temp-allow ") {
        let rule = normalize_rule(rule)?;
        add_unique_rule(&mut rt.session_permission_allow_rules, rule.clone());
        return Ok(format!("session allow rule added: {}", rule));
    }

    if let Some(rule) = trimmed.strip_prefix("temp-deny ") {
        let rule = normalize_rule(rule)?;
        add_unique_rule(&mut rt.session_permission_deny_rules, rule.clone());
        return Ok(format!("session deny rule added: {}", rule));
    }

    if let Some(rule) = trimmed.strip_prefix("temp-rm-allow ") {
        let rule = normalize_rule(rule)?;
        let changed = remove_rule(&mut rt.session_permission_allow_rules, &rule);
        return Ok(format!(
            "session allow rule removed={} rule={}",
            changed, rule
        ));
    }

    if let Some(rule) = trimmed.strip_prefix("temp-rm-deny ") {
        let rule = normalize_rule(rule)?;
        let changed = remove_rule(&mut rt.session_permission_deny_rules, &rule);
        return Ok(format!(
            "session deny rule removed={} rule={}",
            changed, rule
        ));
    }

    if trimmed == "temp-clear-dirs" {
        rt.session_additional_directories.clear();
        return Ok("session additional directories cleared".to_string());
    }

    if let Some(path) = trimmed.strip_prefix("temp-add-dir ") {
        let dir = PathBuf::from(normalize_directory(path)?);
        add_unique_dir(&mut rt.session_additional_directories, dir.clone());
        return Ok(format!(
            "session additional directory added: {}",
            dir.display()
        ));
    }

    if let Some(path) = trimmed.strip_prefix("temp-rm-dir ") {
        let dir = PathBuf::from(normalize_directory(path)?);
        let changed = remove_dir(&mut rt.session_additional_directories, &dir);
        return Ok(format!(
            "session additional directory removed={} path={}",
            changed,
            dir.display()
        ));
    }

    if trimmed == "temp-dirs" {
        if rt.session_additional_directories.is_empty() {
            return Ok("session additional_dirs=<empty>".to_string());
        }
        return Ok(rt
            .session_additional_directories
            .iter()
            .map(|d| format!("- {}", d.display()))
            .collect::<Vec<_>>()
            .join("\n"));
    }

    let mode = trimmed;
    cfg.permission_mode = mode.to_string();
    rt.permission_mode = cfg.permission_mode.clone();
    let _ = cfg.save();
    Ok(format!("permission_mode={}", cfg.permission_mode))
}
fn handle_privacy_command(
    cfg: &mut AppConfig,
    rest: &str,
    rt: &mut Runtime,
) -> Result<String, String> {
    let trimmed = rest.trim();
    if let Some(v) = trimmed.strip_prefix("telemetry ") {
        let on = parse_on_off(v).ok_or_else(|| "Usage: /privacy telemetry on|off".to_string())?;
        cfg.telemetry_enabled = on;
        let _ = cfg.save();
        return Ok(format!("telemetry_enabled={}", cfg.telemetry_enabled));
    }
    if let Some(v) = trimmed.strip_prefix("tool-details ") {
        let on =
            parse_on_off(v).ok_or_else(|| "Usage: /privacy tool-details on|off".to_string())?;
        cfg.telemetry_log_tool_details = on;
        let _ = cfg.save();
        return Ok(format!(
            "telemetry_log_tool_details={}",
            cfg.telemetry_log_tool_details
        ));
    }
    if let Some(v) = trimmed.strip_prefix("undercover ") {
        let on = parse_on_off(v).ok_or_else(|| "Usage: /privacy undercover on|off".to_string())?;
        cfg.undercover_mode = on;
        let _ = cfg.save();
        return Ok(format!("undercover_mode={}", cfg.undercover_mode));
    }
    if let Some(v) = trimmed.strip_prefix("safe-shell ") {
        let on = parse_on_off(v).ok_or_else(|| "Usage: /privacy safe-shell on|off".to_string())?;
        cfg.safe_shell_mode = on;
        apply_runtime_flags_from_cfg(rt, cfg);
        let _ = cfg.save();
        return Ok(format!("safe_shell_mode={}", cfg.safe_shell_mode));
    }
    if trimmed == "status" || trimmed.is_empty() {
        return Ok(privacy_status(cfg));
    }

    Ok("Usage: /privacy status | /privacy telemetry on|off | /privacy tool-details on|off | /privacy undercover on|off | /privacy safe-shell on|off".to_string())
}

fn handle_flags_command(cfg: &mut AppConfig, rest: &str) -> Result<String, String> {
    let trimmed = rest.trim();
    if trimmed == "list" || trimmed.is_empty() {
        return Ok(feature_status(cfg));
    }

    let mut parts = trimmed.split_whitespace();
    if parts.next() != Some("set") {
        return Ok(
            "Usage: /flags list | /flags set <web_tools|bash_tool|subagent|research> on|off"
                .to_string(),
        );
    }
    let name = parts
        .next()
        .ok_or_else(|| "Usage: /flags set <name> on|off".to_string())?;
    let enabled_token = parts
        .next()
        .ok_or_else(|| "Usage: /flags set <name> on|off".to_string())?;

    if !matches!(name, "web_tools" | "bash_tool" | "subagent" | "research") {
        return Err("Unsupported flag name".to_string());
    }

    let on = parse_on_off(enabled_token).ok_or_else(|| "expected on|off".to_string())?;
    cfg.set_feature_disabled(name, !on);
    let _ = cfg.save();
    Ok(format!(
        "{}={}",
        name,
        if on { "enabled" } else { "disabled" }
    ))
}

fn parse_toolcall_line(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("/toolcall ")?;
    let mut trimmed = rest.trim();
    if trimmed.is_empty() {
        return None;
    }
    if (trimmed.starts_with('(') && trimmed.ends_with(')'))
        || (trimmed.starts_with('<') && trimmed.ends_with('>'))
    {
        let inner = trimmed[1..trimmed.len() - 1].trim();
        if !inner.is_empty() {
            trimmed = inner;
        }
    }
    if trimmed.is_empty() {
        return None;
    }

    if let Some((name, args)) = parse_json_toolcall_wrapper(trimmed) {
        return Some((name, args));
    }

    let (name, args) = runtime::split_tool_public(trimmed);
    if name.is_empty() {
        return None;
    }
    Some((name, args))
}

fn parse_json_toolcall_wrapper(input: &str) -> Option<(String, String)> {
    let value = crate::json_toolcall::parse_relaxed_json_value(input)?;
    let candidate = crate::json_toolcall::first_json_tool_candidate(&value)?;
    let name = crate::json_toolcall::extract_json_tool_name(candidate)?;
    let args = extract_json_tool_args_for_main(candidate, &name);
    Some((name, args))
}

fn extract_json_tool_args_for_main(candidate: &serde_json::Value, name: &str) -> String {
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
            provider::tool_call_to_legacy_args(name, &normalized)
        }
        other => provider::tool_call_to_legacy_args(name, &other.to_string()),
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ToolExecutionConstraints {
    block_uv: bool,
    block_git_branching: bool,
    block_mutating_tools: bool,
}

#[derive(Debug, Clone, Copy)]
struct AutoLoopLimits {
    max_steps: Option<usize>,
    max_duration: Duration,
    max_no_progress_rounds: usize,
    max_consecutive_constraint_blocks: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskComplexity {
    Simple,
    Medium,
    Complex,
}

fn parse_auto_steps_value(raw: &str) -> Option<usize> {
    let v = raw.trim().to_ascii_lowercase();
    if v.is_empty() {
        return None;
    }
    if v == "unlimited" || v == "infinite" || v == "inf" {
        return Some(0);
    }
    v.parse::<usize>().ok()
}

fn parse_auto_limit_duration(raw: &str) -> Option<Duration> {
    let v = raw.trim().to_ascii_lowercase();
    if v.is_empty() {
        return None;
    }
    if v == "0" || v == "unlimited" || v == "infinite" || v == "inf" {
        return Some(Duration::from_secs(u64::MAX / 4));
    }
    if let Ok(sec) = v.parse::<u64>() {
        return Some(Duration::from_secs(sec.clamp(30, 24 * 60 * 60)));
    }
    None
}

fn parse_auto_loop_limits_from_env(default_max_steps: usize) -> AutoLoopLimits {
    let env_steps = parse_usize_env("ASI_AUTO_AGENT_MAX_STEPS", default_max_steps);
    let max_steps = if env_steps == 0 { None } else { Some(env_steps) };

    let max_duration_secs = parse_usize_env("ASI_AUTO_AGENT_MAX_DURATION_SECS", 3600);
    let max_duration =
        Duration::from_secs((max_duration_secs as u64).clamp(30, 24 * 60 * 60));

    let max_no_progress_rounds = parse_usize_env("ASI_AUTO_AGENT_MAX_NO_PROGRESS_ROUNDS", 12)
        .clamp(1, 200);
    let max_consecutive_constraint_blocks =
        parse_usize_env("ASI_AUTO_AGENT_MAX_CONSECUTIVE_CONSTRAINT_BLOCKS", 3).clamp(1, 50);

    AutoLoopLimits {
        max_steps,
        max_duration,
        max_no_progress_rounds,
        max_consecutive_constraint_blocks,
    }
}

pub(crate) fn estimate_task_complexity(input: &str, strict_mode: bool) -> TaskComplexity {
    let s = input.to_ascii_lowercase();
    let compact_len = input.chars().filter(|c| !c.is_whitespace()).count();
    let has_cjk = contains_cjk_script(input);
    let mut score = 0usize;

    if strict_mode {
        score += 1;
    }
    if s.contains("/secure ") || s.contains("security") || s.contains("vulnerability") {
        score += 2;
    }
    if s.contains("/work ")
        || s.contains("/code ")
        || s.contains("/agent ")
        || s.contains("/review ")
    {
        score += 2;
    }
    if s.contains("refactor")
        || s.contains("migrate")
        || s.contains("architecture")
        || s.contains("benchmark")
        || s.contains("gateway")
    {
        score += 2;
    }
    if s.contains("and ")
        || s.contains(" then ")
        || s.contains("afterwards")
        || s.contains("as well as")
        || s.contains("at the same time")
    {
        score += 1;
    }
    if has_cjk && compact_len >= 12 {
        score += 2;
    }
    if s.contains("run-only") || s.contains("don't modify") || s.contains("do not modify") {
        score += 1;
    }
    if input.chars().count() > 220 {
        score += 1;
    }

    if score <= 1 {
        TaskComplexity::Simple
    } else if score <= 3 {
        TaskComplexity::Medium
    } else {
        TaskComplexity::Complex
    }
}

pub(crate) fn apply_adaptive_budgets(
    base: AutoLoopLimits,
    complexity: TaskComplexity,
    strict_mode: bool,
    speed: ExecutionSpeed,
) -> AutoLoopLimits {
    let mut out = base;
    let strict_boost = if strict_mode { 1.15 } else { 1.0 };
    let speed_boost = match speed {
        ExecutionSpeed::Sprint => 0.8,
        ExecutionSpeed::Deep => 1.2,
    };
    match complexity {
        TaskComplexity::Simple => {
            out.max_steps = out
                .max_steps
                .map(|v| ((v as f64) * (0.65 * speed_boost)).round() as usize);
            out.max_duration = Duration::from_secs(
                ((out.max_duration.as_secs_f64() * (0.7 * speed_boost)) as u64)
                    .clamp(30, 24 * 60 * 60),
            );
            out.max_no_progress_rounds = ((out.max_no_progress_rounds as f64) * (0.65 * speed_boost))
                .round()
                .clamp(2.0, 200.0) as usize;
        }
        TaskComplexity::Medium => {
            out.max_steps = out
                .max_steps
                .map(|v| ((v as f64) * (0.95 * strict_boost * speed_boost)).round() as usize);
            out.max_duration = Duration::from_secs(
                ((out.max_duration.as_secs_f64() * (0.95 * strict_boost * speed_boost)) as u64)
                    .clamp(30, 24 * 60 * 60),
            );
            out.max_no_progress_rounds = ((out.max_no_progress_rounds as f64)
                * (0.95 * strict_boost * speed_boost))
                .round()
                .clamp(2.0, 200.0) as usize;
        }
        TaskComplexity::Complex => {
            out.max_steps = out
                .max_steps
                .map(|v| ((v as f64) * (1.35 * strict_boost * speed_boost)).round() as usize);
            out.max_duration = Duration::from_secs(
                ((out.max_duration.as_secs_f64() * (1.35 * strict_boost * speed_boost)) as u64)
                    .clamp(30, 24 * 60 * 60),
            );
            out.max_no_progress_rounds = ((out.max_no_progress_rounds as f64)
                * (1.35 * strict_boost * speed_boost))
                .round()
                .clamp(2.0, 200.0) as usize;
            out.max_consecutive_constraint_blocks = match speed {
                ExecutionSpeed::Sprint => out.max_consecutive_constraint_blocks.clamp(1, 50),
                ExecutionSpeed::Deep => (out.max_consecutive_constraint_blocks + 1).clamp(1, 50),
            };
        }
    }

    if let Some(steps) = out.max_steps {
        out.max_steps = Some(steps.clamp(1, 10000));
    }
    out
}

fn format_adaptive_budget_note(
    complexity: TaskComplexity,
    base: AutoLoopLimits,
    adaptive: AutoLoopLimits,
) -> String {
    let complexity_name = match complexity {
        TaskComplexity::Simple => "simple",
        TaskComplexity::Medium => "medium",
        TaskComplexity::Complex => "complex",
    };
    let base_steps = base
        .max_steps
        .map(|v| v.to_string())
        .unwrap_or_else(|| "unlimited".to_string());
    let adaptive_steps = adaptive
        .max_steps
        .map(|v| v.to_string())
        .unwrap_or_else(|| "unlimited".to_string());
    format!(
        "adaptive_budget complexity={} steps:{}->{} duration_secs:{}->{} no_progress:{}->{} constraint_blocks:{}->{}",
        complexity_name,
        base_steps,
        adaptive_steps,
        base.max_duration.as_secs(),
        adaptive.max_duration.as_secs(),
        base.max_no_progress_rounds,
        adaptive.max_no_progress_rounds,
        base.max_consecutive_constraint_blocks,
        adaptive.max_consecutive_constraint_blocks
    )
}

fn user_disallows_uv(input: &str) -> bool {
    let s = input.to_ascii_lowercase();
    (s.contains("do not use uv")
        || s.contains("don't use uv")
        || s.contains("do not use uv")
        || s.contains("without uv"))
        && !s.contains("you may use uv")
        && !s.contains("you can use uv")
        && !s.contains("uv is allowed")
}

fn user_requests_run_only(input: &str) -> bool {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return false;
    }

    let edit_hints = [
        "edit",
        "write",
        "modify",
        "fix",
        "implement",
        "refactor",
    ];
    let negative_modify_hints = [
        "don't modify",
        "do not modify",
        "no modify",
    ];
    let has_negative_modify = negative_modify_hints.iter().any(|k| s.contains(k));
    if edit_hints.iter().any(|k| s.contains(k)) && !has_negative_modify {
        return false;
    }

    let run_hints = ["run", "execute", "program.md", "according to"];
    if !run_hints.iter().any(|k| s.contains(k)) {
        return false;
    }

    if (s.contains("run code") || s.contains("run the code") || s.contains("execute code"))
        || (s.contains("program.md") && (s.contains("run") || s.contains("execute")))
        || ((s.contains("don't modify")
            || s.contains("do not modify")
            || s.contains("no file edits"))
            && (s.contains("run") || s.contains("execute")))
    {
        return true;
    }

    false
}

fn user_explicitly_requests_git_branch_ops(input: &str) -> bool {
    let s = input.to_ascii_lowercase();
    let hints = [
        "git checkout",
        "git switch",
        "git reset",
        "checkout -b",
        "switch branch",
        "create branch",
        "reset to",
        "roll back to",
    ];
    hints.iter().any(|k| s.contains(k))
}

fn derive_tool_execution_constraints(input: &str, strict_mode: bool) -> ToolExecutionConstraints {
    let allow_git_branching = user_explicitly_requests_git_branch_ops(input);
    ToolExecutionConstraints {
        block_uv: user_disallows_uv(input),
        block_git_branching: strict_mode && !allow_git_branching,
        block_mutating_tools: user_requests_run_only(input),
    }
}

fn bash_uses_uv(args: &str) -> bool {
    let lower = args
        .to_ascii_lowercase()
        .replace('\n', " ")
        .replace('\r', " ")
        .replace('\t', " ");
    lower.contains(" uv ")
        || lower.contains(" uv;")
        || lower.contains("; uv ")
        || lower.contains("| uv ")
        || lower.contains("&& uv ")
        || lower.starts_with("uv ")
        || lower.starts_with("uv\n")
        || lower.starts_with("uv\t")
        || lower.contains("uv sync")
        || lower.contains("uv run")
        || lower.contains("uv pip")
}

fn bash_uses_forbidden_git_branching(args: &str) -> bool {
    let lower = args
        .to_ascii_lowercase()
        .replace('\n', " ")
        .replace('\r', " ")
        .replace('\t', " ");
    lower.contains("git checkout") || lower.contains("git switch") || lower.contains("git reset")
}

fn toolcall_is_blocked_by_user_constraints(
    command: &str,
    constraints: ToolExecutionConstraints,
) -> Option<String> {
    let (name, args) = parse_toolcall_line(command)?;
    if constraints.block_mutating_tools && matches!(name.as_str(), "write_file" | "edit_file") {
        return Some(
            "blocked by user constraint: request is run-only and does not allow file edits"
                .to_string(),
        );
    }

    if name != "bash" {
        return None;
    }

    if constraints.block_uv && bash_uses_uv(&args) {
        return Some(
            "blocked by user constraint: request explicitly said not to use uv".to_string(),
        );
    }

    if constraints.block_git_branching && bash_uses_forbidden_git_branching(&args) {
        return Some(
            "blocked by strict safety rule: git branch/history-changing commands require explicit user request".to_string(),
        );
    }

    None
}

fn constrained_tool_result(rt: &Runtime, reason: String) -> runtime::TurnResult {
    runtime::TurnResult {
        text: reason,
        stop_reason: "tool_result".to_string(),
        input_tokens: 0,
        output_tokens: 0,
        total_input_tokens: rt.cumulative_input_tokens,
        total_output_tokens: rt.cumulative_output_tokens,
        is_tool_result: true,
        turn_cost_usd: 0.0,
        total_cost_usd: rt.cumulative_cost_usd,
        native_tool_calls: Vec::new(),
        thinking: None,
    }
}

fn log_interaction_event(cfg: &AppConfig, rt: &Runtime, line: &str, result: &runtime::TurnResult) {
    if let Some((name, args)) = parse_toolcall_line(line) {
        let _ = telemetry::log_tool_call(
            cfg,
            &rt.provider,
            &rt.model,
            &name,
            &args,
            &result.stop_reason,
            &result.text,
        );
    } else {
        let _ = telemetry::log_turn(
            cfg,
            &rt.provider,
            &rt.model,
            &result.stop_reason,
            result.input_tokens,
            result.output_tokens,
            result.turn_cost_usd,
        );
    }
}
fn subagent_system_prompt() -> &'static str {
    "You are a focused sub-agent. Return concise actionable output."
}

fn runtime_subagent_snapshot(rt: &Runtime) -> SubagentRuntimeSnapshot {
    SubagentRuntimeSnapshot {
        extended_thinking: rt.extended_thinking,
        disable_web_tools: rt.disable_web_tools,
        disable_bash_tool: rt.disable_bash_tool,
        safe_shell_mode: rt.safe_shell_mode,
        permission_allow_rules: rt.permission_allow_rules.clone(),
        permission_deny_rules: rt.permission_deny_rules.clone(),
        path_restriction_enabled: rt.path_restriction_enabled,
        additional_directories: rt.additional_directories.clone(),
        session_permission_allow_rules: rt.session_permission_allow_rules.clone(),
        session_permission_deny_rules: rt.session_permission_deny_rules.clone(),
        session_additional_directories: rt.session_additional_directories.clone(),
        next_permission_allow_rules: rt.next_permission_allow_rules.clone(),
        next_additional_directories: rt.next_additional_directories.clone(),
        interactive_approval_allow_rules: rt.interactive_approval_allow_rules.clone(),
        interactive_approval_deny_rules: rt.interactive_approval_deny_rules.clone(),
        native_tool_calling: rt.native_tool_calling,
    }
}

fn apply_subagent_snapshot_to_runtime(rt: &mut Runtime, snap: &SubagentRuntimeSnapshot) {
    rt.extended_thinking = snap.extended_thinking;
    rt.disable_web_tools = snap.disable_web_tools;
    rt.disable_bash_tool = snap.disable_bash_tool;
    rt.safe_shell_mode = snap.safe_shell_mode;
    rt.permission_allow_rules = snap.permission_allow_rules.clone();
    rt.permission_deny_rules = snap.permission_deny_rules.clone();
    rt.path_restriction_enabled = snap.path_restriction_enabled;
    rt.additional_directories = snap.additional_directories.clone();
    rt.session_permission_allow_rules = snap.session_permission_allow_rules.clone();
    rt.session_permission_deny_rules = snap.session_permission_deny_rules.clone();
    rt.session_additional_directories = snap.session_additional_directories.clone();
    rt.next_permission_allow_rules = snap.next_permission_allow_rules.clone();
    rt.next_additional_directories = snap.next_additional_directories.clone();
    rt.interactive_approval_allow_rules = snap.interactive_approval_allow_rules.clone();
    rt.interactive_approval_deny_rules = snap.interactive_approval_deny_rules.clone();
    rt.native_tool_calling = snap.native_tool_calling;
}

fn merge_unique_owned(base: &[String], extra: &[String]) -> Vec<String> {
    let mut out = base.to_vec();
    for item in extra {
        if !out.iter().any(|x| x == item) {
            out.push(item.clone());
        }
    }
    out
}

fn subagent_effective_rule_summary(run_cfg: &SubagentRunConfig) -> (Vec<String>, Vec<String>) {
    let snap = &run_cfg.runtime_snapshot;
    let allow = {
        let merged = merge_unique_owned(
            &snap.permission_allow_rules,
            &snap.session_permission_allow_rules,
        );
        merge_unique_owned(&merged, &snap.next_permission_allow_rules)
    };
    let deny = merge_unique_owned(
        &snap.permission_deny_rules,
        &snap.session_permission_deny_rules,
    );
    (allow, deny)
}

fn snapshot_has_allow_rules(snap: &SubagentRuntimeSnapshot) -> bool {
    !snap.permission_allow_rules.is_empty()
        || !snap.session_permission_allow_rules.is_empty()
        || !snap.next_permission_allow_rules.is_empty()
}

fn snapshot_has_deny_rules(snap: &SubagentRuntimeSnapshot) -> bool {
    !snap.permission_deny_rules.is_empty() || !snap.session_permission_deny_rules.is_empty()
}

fn build_subagent_run_config(
    cfg: &AppConfig,
    rt: &Runtime,
    speed: ExecutionSpeed,
    strict_mode: bool,
) -> SubagentRunConfig {
    let limits = parse_auto_loop_limits_from_env(16);
    SubagentRunConfig {
        provider: rt.provider.clone(),
        model: rt.model.clone(),
        permission_mode: rt.permission_mode.clone(),
        rules_source: "runtime".to_string(),
        profile_name: None,
        default_skills: Vec::new(),
        max_turns: rt.max_turns,
        app_cfg: cfg.clone(),
        runtime_snapshot: runtime_subagent_snapshot(rt),
        tool_loop_enabled: parse_bool_env("ASI_SUBAGENT_TOOL_LOOP", true),
        strict_mode,
        speed,
        limits,
        constraints: ToolExecutionConstraints {
            block_uv: false,
            block_git_branching: strict_mode,
            block_mutating_tools: false,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentViewScope {
    Foreground,
    Background,
    All,
}

impl AgentViewScope {
    fn from_opt(raw: Option<&str>) -> Option<Self> {
        let Some(v) = raw else {
            return None;
        };
        match v.trim().to_ascii_lowercase().as_str() {
            "fg" | "foreground" => Some(Self::Foreground),
            "bg" | "background" => Some(Self::Background),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct AgentProfile {
    provider: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    allowed_tools: Option<Vec<String>>,
    denied_tools: Option<Vec<String>>,
    default_skills: Option<Vec<String>>,
    disable_web_tools: Option<bool>,
    disable_bash_tool: Option<bool>,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
struct AgentProfileFile {
    profiles: Option<HashMap<String, AgentProfile>>,
}

fn load_agent_profile(name: &str) -> Result<AgentProfile, String> {
    let path = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .join("agent_profiles.json");
    load_agent_profile_from_path(&path, name)
}

fn load_agent_profile_from_path(path: &Path, name: &str) -> Result<AgentProfile, String> {
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
    let parsed: AgentProfileFile =
        serde_json::from_str(&text).map_err(|e| format!("invalid {}: {}", path.display(), e))?;
    let key = name.trim().to_string();
    let profile = parsed
        .profiles
        .unwrap_or_default()
        .remove(&key)
        .ok_or_else(|| format!("agent profile not found: {}", name))?;
    Ok(profile)
}

fn apply_agent_profile_to_config(run_cfg: &mut SubagentRunConfig, profile: &AgentProfile) {
    if let Some(p) = profile.provider.as_deref() {
        run_cfg.provider = normalize_provider_name(p);
    }
    if let Some(m) = profile.model.as_deref() {
        run_cfg.model = resolve_model_alias(m);
    }
    if let Some(pm) = profile.permission_mode.as_deref() {
        run_cfg.permission_mode = pm.trim().to_string();
    }
    let mut touched_profile_rules = false;
    let mut mixed_with_runtime_rules = false;
    if let Some(allowed_tools) = profile.allowed_tools.as_ref() {
        let had_runtime_allow = snapshot_has_allow_rules(&run_cfg.runtime_snapshot);
        let mut allow_rules = Vec::new();
        for tool in allowed_tools {
            let t = tool.trim();
            if t.is_empty() {
                continue;
            }
            add_unique_rule(&mut allow_rules, t.to_string());
        }
        let has_profile_allow = !allow_rules.is_empty();
        if had_runtime_allow && has_profile_allow {
            mixed_with_runtime_rules = true;
            let merged = merge_unique_owned(&run_cfg.runtime_snapshot.permission_allow_rules, &allow_rules);
            run_cfg.runtime_snapshot.permission_allow_rules = merged;
        } else {
            run_cfg.runtime_snapshot.permission_allow_rules = allow_rules;
            run_cfg.runtime_snapshot.session_permission_allow_rules.clear();
            run_cfg.runtime_snapshot.next_permission_allow_rules.clear();
        }
        touched_profile_rules = true;
    }
    if let Some(denied_tools) = profile.denied_tools.as_ref() {
        let had_runtime_deny = snapshot_has_deny_rules(&run_cfg.runtime_snapshot);
        let mut deny_rules = Vec::new();
        for tool in denied_tools {
            let t = tool.trim();
            if t.is_empty() {
                continue;
            }
            add_unique_rule(&mut deny_rules, t.to_string());
        }
        let has_profile_deny = !deny_rules.is_empty();
        if had_runtime_deny && has_profile_deny {
            mixed_with_runtime_rules = true;
            let merged = merge_unique_owned(&run_cfg.runtime_snapshot.permission_deny_rules, &deny_rules);
            run_cfg.runtime_snapshot.permission_deny_rules = merged;
        } else {
            run_cfg.runtime_snapshot.permission_deny_rules = deny_rules;
            run_cfg.runtime_snapshot.session_permission_deny_rules.clear();
        }
        touched_profile_rules = true;
    }
    if touched_profile_rules {
        run_cfg.rules_source = if mixed_with_runtime_rules {
            "mixed".to_string()
        } else {
            "profile".to_string()
        };
    }
    if let Some(skills) = profile.default_skills.as_ref() {
        run_cfg.default_skills = skills
            .iter()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>();
    }
    if let Some(v) = profile.disable_web_tools {
        run_cfg.app_cfg.set_feature_disabled("web_tools", v);
    }
    if let Some(v) = profile.disable_bash_tool {
        run_cfg.app_cfg.set_feature_disabled("bash_tool", v);
    }
}

fn run_subagent_with_messages(
    run_cfg: &SubagentRunConfig,
    messages: &[ChatMessage],
) -> Result<String, String> {
    let mut rt = Runtime::new(
        run_cfg.provider.clone(),
        run_cfg.model.clone(),
        run_cfg.permission_mode.clone(),
        run_cfg.max_turns,
    );
    apply_subagent_snapshot_to_runtime(&mut rt, &run_cfg.runtime_snapshot);
    rt.messages = messages.to_vec();
    let Some(last_user) = messages
        .iter()
        .rev()
        .find(|m| m.role.eq_ignore_ascii_case("user"))
        .map(|m| m.content.as_str())
    else {
        return Err("subagent has no user task in history".to_string());
    };
    if !run_cfg.default_skills.is_empty() {
        eprintln!(
            "INFO subagent profile skills={}",
            run_cfg.default_skills.join(",")
        );
    }

    let mut result = rt.run_turn(last_user);
    if run_cfg.tool_loop_enabled && is_auto_loop_continuable_stop_reason(&result.stop_reason) {
        let mut changed_files = Vec::new();
        let mut file_synopsis_cache = FileSynopsisCache::default();
        let mut confidence_gate_stats = ConfidenceGateStats::default();
        let (loop_result, _extra_cost, _steps, _loop_stop_reason) = run_prompt_auto_tool_loop(
            &mut rt,
            &run_cfg.app_cfg,
            last_user,
            &result,
            &mut changed_files,
            run_cfg.strict_mode,
            run_cfg.speed,
            run_cfg.constraints,
            run_cfg.limits,
            None,
            &mut file_synopsis_cache,
            &mut confidence_gate_stats,
        );
        result = loop_result;
    }

    if result.stop_reason == "provider_error" {
        Err(result.text)
    } else {
        Ok(result.text)
    }
}

fn auto_tool_repair_prompt() -> &'static str {
    "Your previous response appears to have malformed tool-call formatting. Re-output ONLY valid /toolcall lines (one per line), with no prose and no code fences. If no tool calls are needed, reply exactly NO_TOOLCALLS."
}

fn auto_tool_repair_prompt_strict() -> &'static str {
    "STRICT mode: previous output had malformed action format. Re-output ONLY valid plain /toolcall lines (one line per call), with exact tool names and arguments. No prose. No code fences. If no actions are needed, reply exactly NO_TOOLCALLS."
}

fn looks_like_malformed_toolcall_output(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let known_tools = [
        "read_file",
        "write_file",
        "edit_file",
        "glob_search",
        "grep_search",
        "bash",
        "web_search",
        "web_fetch",
    ];
    let trimmed = lower.trim_start();
    if trimmed.starts_with("/toolcall ") {
        return false;
    }
    if lower.contains("/toolcall") {
        return true;
    }

    let has_function_like_tool = known_tools
        .iter()
        .any(|needle| lower.contains(&format!("{}(", needle)));
    if has_function_like_tool {
        return true;
    }
    let has_xml_tool_tag = known_tools.iter().any(|needle| {
        lower.contains(&format!("<{} ", needle))
            || lower.contains(&format!("<{}>", needle))
            || lower.contains(&format!("</{}>", needle))
            || lower.contains(&format!("<{}_write", needle))
    });
    let has_tool_call_wrapper = lower.contains("<tool_call>") || lower.contains("</tool_call>");
    let has_function_calls_wrapper =
        lower.contains("<function-calls>") || lower.contains("</function-calls>");
    let has_tool_arguments_pairs =
        lower.contains("tool:") && lower.contains("arguments:");
    if has_xml_tool_tag
        || has_tool_call_wrapper
        || has_function_calls_wrapper
        || has_tool_arguments_pairs
        || lower.contains("<file_write")
        || lower.contains("</file_write>")
        || lower.contains("<file_edit")
        || lower.contains("</file_edit>")
    {
        return true;
    }

    let has_wrapper_call = lower.contains("tool_call(") || lower.contains("toolcall(");
    let has_quoted_tool_name = known_tools.iter().any(|needle| {
        lower.contains(&format!("\"{}\"", needle)) || lower.contains(&format!("'{}'", needle))
    });
    if has_wrapper_call && has_quoted_tool_name {
        return true;
    }

    let mentions_toolcall =
        lower.contains("toolcall") || lower.contains("tool call") || lower.contains("tool_call");
    let mentions_known_tool = known_tools.iter().any(|needle| lower.contains(needle));
    if mentions_toolcall && mentions_known_tool {
        return true;
    }

    let has_json_tool_envelope = (lower.contains("\"tool_calls\"")
        || lower.contains("\"function\"")
        || lower.contains("\"arguments\"")
        || lower.contains("\"name\""))
        && has_quoted_tool_name;
    if has_json_tool_envelope {
        return true;
    }

    false
}

fn is_auto_loop_continuable_stop_reason(stop_reason: &str) -> bool {
    matches!(
        Runtime::stop_reason_alias(stop_reason),
        "completed" | "tool_use"
    ) || stop_reason.trim().eq_ignore_ascii_case("tool_result")
}

fn looks_like_mixed_toolcall_and_result_output(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if !lower.contains("/toolcall") {
        return false;
    }

    lower.contains("[tool:")
        || lower.contains("\"exit_code\"")
        || lower.contains("\"stdout\"")
        || lower.contains("\"stderr\"")
        || lower.contains("\"command\"")
        || lower.contains("```")
        || lower.contains("\n---")
}

fn looks_like_fabricated_tool_result_output(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if !lower.contains("[tool:") {
        return false;
    }

    let has_status_suffix =
        lower.contains(":ok]") || lower.contains(":error]") || lower.contains(":result]");
    if !has_status_suffix {
        return false;
    }

    lower.contains("[read_file:")
        || lower.contains("[glob_search:")
        || lower.contains("[grep_search:")
        || lower.contains("[web_search:")
        || lower.contains("[web_fetch:")
        || lower.contains("\"stdout\"")
        || lower.contains("\"stderr\"")
        || lower.contains("\"exit_code\"")
}

fn should_attempt_auto_output_repair(repair_attempted: bool, text: &str) -> bool {
    !repair_attempted
        && (looks_like_mixed_toolcall_and_result_output(text)
            || looks_like_fabricated_tool_result_output(text))
}

const AUTO_OUTPUT_REPAIR_MAX_ATTEMPTS: usize = 3;

fn is_brief_non_executing_response(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    let line_count = trimmed.lines().take(6).count();
    let char_count = trimmed.chars().count();
    line_count <= 4 && char_count <= 260
}

fn looks_like_plan_preface_without_actions(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.contains("/toolcall ") {
        return false;
    }
    let hints = [
        "i will",
        "i'll",
        "let me",
        "start by",
        "first",
        "next",
        "then",
        "plan",
        "going to",
    ];
    hints.iter().any(|k| lower.contains(k))
}

fn looks_like_execution_ready_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("/toolcall ")
        || lower.starts_with("<toolcall")
        || lower.starts_with("<tool_call")
        || lower.starts_with("</tool_call")
        || lower.starts_with("<function-calls")
        || lower.starts_with("</function-calls")
        || lower.starts_with("<function-call")
        || lower.starts_with("</function-call")
        || lower.starts_with("<invoke ")
        || lower.starts_with("<parameter ")
        || lower.starts_with("<file_")
        || lower.starts_with("</file_")
        || lower.starts_with("<glob_search")
        || lower.starts_with("<grep_search")
        || lower.starts_with("<read_file")
        || lower.starts_with("<write_file")
        || lower.starts_with("<edit_file")
        || lower.starts_with("<web_search")
        || lower.starts_with("<web_fetch")
        || lower.starts_with("<bash")
        || lower.contains("tool_call")
        || lower.contains("toolcall")
    {
        return true;
    }
    let prefixes = [
        "glob_search ",
        "grep_search ",
        "read_file ",
        "write_file ",
        "edit_file ",
        "bash ",
        "web_search ",
        "web_fetch ",
    ];
    prefixes.iter().any(|p| lower.starts_with(p))
}

fn looks_like_execution_ready_text(text: &str) -> bool {
    text.lines().any(looks_like_execution_ready_line)
}

fn should_attempt_zero_toolcall_nudge(
    nudge_attempted: bool,
    steps: usize,
    result: &runtime::TurnResult,
) -> bool {
    if nudge_attempted || steps > 1 || result.is_tool_result {
        return false;
    }
    if !is_auto_loop_continuable_stop_reason(&result.stop_reason) {
        return false;
    }
    if looks_like_execution_ready_text(&result.text) {
        return true;
    }
    is_brief_non_executing_response(&result.text)
        || looks_like_plan_preface_without_actions(&result.text)
        || looks_like_toolcall_required_intent(&result.text)
}

fn looks_like_toolcall_required_intent(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("use tool calls")
        || lower.contains("tool calls")
        || lower.contains("output only /toolcall")
        || lower.contains("output only toolcall")
        || lower.contains("continue using tool results")
}

fn task_anchor_line(task: &str) -> String {
    let compact = task
        .trim()
        .replace('\n', " ")
        .replace('\r', " ")
        .replace('\t', " ");
    format!("Primary task (do not drift): {}", clip_chars(&compact, 320))
}

fn context_with_task_anchor(context_contract: &str, task: &str) -> String {
    format!("{}\n{}", context_contract, task_anchor_line(task))
}

fn build_zero_toolcall_nudge_prompt(
    context_contract: &str,
    task: &str,
    strict_mode: bool,
) -> String {
    let profile_tail = if strict_mode {
        "Keep strict profile requirements unchanged."
    } else {
        "Follow the existing execution rules."
    };
    let anchor = task_anchor_line(task);
    format!(
        "{}\n{}\n\nYour previous response did not execute the task yet.\nContinue now.\n\nIMPORTANT: Markdown code fences (```bash ... ``` or ```python ... ```) are NOT executed. They are just inert text. To actually run a command or write a file you MUST emit one of these formats on its own line, with no fences and no other prose on the same line:\n\n  /toolcall bash <command>\n  /toolcall write_file <path> <<<CONTENT\n  ...file body...\n  <<<END\n  /toolcall edit_file <path> <<<OLD\n  ...old text...\n  <<<NEW\n  ...new text...\n  <<<END\n  /toolcall read_file <path> <start_line> <max_lines>\n  /toolcall glob_search <pattern>\n  /toolcall grep_search <pattern>\n\nDo not invent alternative wire formats (no `<toolcall>`, no `<write_file path=...>`, no `<function_calls>`). Use the exact `/toolcall NAME ARGS` shape above.\n\nIf tools are needed, output /toolcall lines only and execute immediately.\nIf tools are not needed, provide the final user-facing answer now.\n{}",
        context_contract, anchor, profile_tail
    )
}

const BILINGUAL_MD_ENFORCEMENT_MAX_RETRIES: usize = 2;
const REVIEW_SCHEMA_ENFORCEMENT_MAX_RETRIES: usize = 2;

fn looks_like_task_encoding_loss(task: &str) -> bool {
    let total = task.chars().count();
    if total == 0 {
        return false;
    }
    let question_marks = task.chars().filter(|ch| *ch == '?').count();
    // Heuristic for lossy console encoding where non-ASCII text degrades to '?'.
    question_marks >= 6 && question_marks * 4 >= total
}

fn requires_bilingual_markdown_reports(task: &str) -> bool {
    let lower = task.to_ascii_lowercase();
    let asks_markdown = lower.contains(".md") || lower.contains("markdown");
    let asks_chinese = lower.contains("chinese")
        || task.contains("\u{4e2d}\u{6587}")
        || lower.contains("zh");
    let asks_english = lower.contains("english")
        || task.contains("\u{82f1}\u{6587}")
        || lower.contains("en");
    let likely_encoding_loss = asks_markdown && looks_like_task_encoding_loss(task);
    asks_markdown && ((asks_chinese && asks_english) || likely_encoding_loss)
}

fn markdown_language_tag(path: &str) -> Option<&'static str> {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    if !normalized.ends_with(".md") {
        return None;
    }
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    if file_name.contains("_zh")
        || file_name.contains("-zh")
        || file_name.contains("_cn")
        || file_name.contains("-cn")
        || file_name.contains("chinese")
    {
        return Some("zh");
    }
    if file_name.contains("_en")
        || file_name.contains("-en")
        || file_name.contains("english")
    {
        return Some("en");
    }
    None
}

fn has_bilingual_markdown_reports(changed_files: &[String]) -> bool {
    let mut has_zh = false;
    let mut has_en = false;
    for path in changed_files {
        match markdown_language_tag(path) {
            Some("zh") => has_zh = true,
            Some("en") => has_en = true,
            _ => {}
        }
    }
    has_zh && has_en
}

fn should_enforce_bilingual_markdown_reports(task: &str, changed_files: &[String]) -> bool {
    requires_bilingual_markdown_reports(task) && !has_bilingual_markdown_reports(changed_files)
}

pub(crate) fn is_review_task_input(task: &str) -> bool {
    let trimmed = task.trim();
    if trimmed == "/review"
        || trimmed
            .to_ascii_lowercase()
            .starts_with("/review ")
    {
        return true;
    }

    // Generated review prompts are plain-text prefixed and no longer start
    // with "/review". Keep schema enforcement active for those prompts.
    trimmed
        .to_ascii_lowercase()
        .starts_with("code review task in the current local project.")
}

pub(crate) fn parse_review_task_json_only(raw: &str) -> Option<(String, bool)> {
    let trimmed = raw.trim();
    let review_prefix = "/review ";
    if !trimmed.to_ascii_lowercase().starts_with(review_prefix) {
        return None;
    }
    let rest = trimmed[review_prefix.len()..].trim();
    if rest.is_empty() {
        return None;
    }
    let json_only_suffix = "--json-only";
    if rest.eq_ignore_ascii_case(json_only_suffix) {
        return None;
    }
    if !rest.to_ascii_lowercase().ends_with(json_only_suffix) {
        return Some((rest.to_string(), false));
    }
    let idx = rest.len() - json_only_suffix.len();
    let before = rest[..idx].trim();
    if before.is_empty() {
        return None;
    }
    Some((before.to_string(), true))
}

fn find_section_header_index(lines: &[&str], header: &str) -> Option<usize> {
    lines.iter().position(|line| {
        line.trim()
            .to_ascii_lowercase()
            .eq(header)
    })
}

fn section_has_list_or_none(lines: &[&str], start: usize, end: usize) -> bool {
    if start + 1 >= end {
        return false;
    }
    lines[start + 1..end]
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .any(|line| {
            line.eq_ignore_ascii_case("none")
                || line.starts_with("- ")
                || line.starts_with("* ")
                || line
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_digit())
                    && line.contains('.')
        })
}

pub(crate) fn review_output_schema_error(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Some("response is empty".to_string());
    }

    let section_specs = [
        ("Findings", "findings:"),
        ("Missing Tests", "missing tests:"),
        ("Open Questions", "open questions:"),
        ("Summary", "summary:"),
    ];

    let mut section_positions: Vec<(&str, usize)> = Vec::new();
    let mut missing = Vec::new();
    for (label, normalized_header) in section_specs {
        if let Some(idx) = find_section_header_index(&lines, normalized_header) {
            section_positions.push((label, idx));
        } else {
            missing.push(label);
        }
    }
    if !missing.is_empty() {
        return Some(format!("missing sections: {}", missing.join(", ")));
    }

    let mut last = 0usize;
    for (pos, (_, idx)) in section_positions.iter().enumerate() {
        if pos > 0 && *idx <= last {
            return Some("section order must be Findings -> Missing Tests -> Open Questions -> Summary"
                .to_string());
        }
        last = *idx;
    }

    for i in 0..section_positions.len() {
        let (label, start) = section_positions[i];
        let end = if i + 1 < section_positions.len() {
            section_positions[i + 1].1
        } else {
            lines.len()
        };
        if !section_has_list_or_none(&lines, start, end) {
            return Some(format!(
                "section `{}` must contain a list item or `None`",
                label
            ));
        }
    }

    None
}

fn should_enforce_review_output_schema(task: &str, response_text: &str) -> bool {
    is_review_task_input(task) && review_output_schema_error(response_text).is_some()
}

fn collect_review_section_lines(
    lines: &[&str],
    starts: &[usize],
    index_in_starts: usize,
) -> Vec<String> {
    let start = starts[index_in_starts];
    let next = starts
        .iter()
        .copied()
        .filter(|v| *v > start)
        .min()
        .unwrap_or(lines.len());
    if start + 1 >= next {
        return Vec::new();
    }
    lines[start + 1..next]
        .iter()
        .map(|line| line.trim_end())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect()
}

fn is_review_list_item_line(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("- ") || t.starts_with("* ") {
        return true;
    }
    let mut chars = t.chars().peekable();
    let mut saw_digit = false;
    while let Some(ch) = chars.peek() {
        if ch.is_ascii_digit() {
            saw_digit = true;
            chars.next();
        } else {
            break;
        }
    }
    if !saw_digit {
        return false;
    }
    matches!(chars.next(), Some('.')) && matches!(chars.next(), Some(' '))
}

fn strip_review_list_prefix(line: &str) -> &str {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix("- ") {
        return rest.trim();
    }
    if let Some(rest) = t.strip_prefix("* ") {
        return rest.trim();
    }
    let mut idx = 0usize;
    let bytes = t.as_bytes();
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx > 0 && idx + 1 < bytes.len() && bytes[idx] == b'.' && bytes[idx + 1] == b' ' {
        return t[idx + 2..].trim();
    }
    t.trim()
}

fn parse_review_severity_prefix(text: &str) -> (Option<String>, String) {
    let trimmed = text.trim();
    if !trimmed.starts_with('[') {
        return (None, trimmed.to_string());
    }
    if let Some(close) = trimmed.find(']') {
        let sev = trimmed[1..close].trim();
        let rest = trimmed[close + 1..].trim();
        if sev.is_empty() {
            return (None, rest.to_string());
        }
        return (Some(sev.to_string()), rest.to_string());
    }
    (None, trimmed.to_string())
}

fn parse_review_path_and_line(location: &str) -> (Option<String>, Option<u64>) {
    let loc = location.trim();
    if loc.is_empty() {
        return (None, None);
    }
    if let Some((path, line_raw)) = loc.rsplit_once(':') {
        let candidate = line_raw.trim();
        if !candidate.is_empty() && candidate.chars().all(|ch| ch.is_ascii_digit()) {
            let line = candidate.parse::<u64>().ok();
            let path_norm = path.trim();
            let path_value = if path_norm.is_empty() {
                None
            } else {
                Some(path_norm.to_string())
            };
            return (path_value, line);
        }
    }
    (Some(loc.to_string()), None)
}

fn parse_labeled_review_value(line: &str, label: &str) -> Option<String> {
    let (k, v) = line.split_once(':')?;
    if !k.trim().eq_ignore_ascii_case(label) {
        return None;
    }
    let value = v.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn normalize_review_severity(raw: Option<&str>) -> Option<String> {
    let sev = raw?.trim().to_ascii_lowercase();
    if sev.is_empty() {
        return None;
    }
    let normalized = if sev.contains("critical") || sev == "cr" || sev == "p0" {
        "critical"
    } else if sev.contains("high") || sev == "h" || sev == "p1" {
        "high"
    } else if sev.contains("medium") || sev == "med" || sev == "m" || sev == "p2" {
        "medium"
    } else if sev.contains("low") || sev == "l" || sev == "p3" {
        "low"
    } else {
        return None;
    };
    Some(normalized.to_string())
}

fn compose_review_location(path: Option<&str>, line: Option<u64>) -> Option<String> {
    match (path, line) {
        (Some(p), Some(ln)) if !p.trim().is_empty() => Some(format!("{}:{}", p.trim(), ln)),
        (Some(p), None) if !p.trim().is_empty() => Some(p.trim().to_string()),
        _ => None,
    }
}

fn review_severity_rank(raw: Option<&str>) -> u8 {
    match raw {
        Some("critical") => 0,
        Some("high") => 1,
        Some("medium") => 2,
        Some("low") => 3,
        _ => 4,
    }
}

#[derive(Debug, Clone, Copy)]
struct ReviewRiskWeights {
    critical: u64,
    high: u64,
    medium: u64,
    low: u64,
    unknown: u64,
}

impl Default for ReviewRiskWeights {
    fn default() -> Self {
        Self {
            critical: 8,
            high: 4,
            medium: 2,
            low: 1,
            unknown: 1,
        }
    }
}

fn parse_review_risk_weights_from_env() -> ReviewRiskWeights {
    ReviewRiskWeights {
        critical: parse_usize_env("ASI_REVIEW_RISK_WEIGHT_CRITICAL", 8) as u64,
        high: parse_usize_env("ASI_REVIEW_RISK_WEIGHT_HIGH", 4) as u64,
        medium: parse_usize_env("ASI_REVIEW_RISK_WEIGHT_MEDIUM", 2) as u64,
        low: parse_usize_env("ASI_REVIEW_RISK_WEIGHT_LOW", 1) as u64,
        unknown: parse_usize_env("ASI_REVIEW_RISK_WEIGHT_UNKNOWN", 1) as u64,
    }
}

fn review_severity_weight(raw: &str, weights: ReviewRiskWeights) -> u64 {
    match raw {
        "critical" => weights.critical,
        "high" => weights.high,
        "medium" => weights.medium,
        "low" => weights.low,
        _ => weights.unknown,
    }
}

#[derive(Default)]
struct ReviewRiskPathStat {
    score: u64,
    findings: u64,
    critical: u64,
    high: u64,
    medium: u64,
    low: u64,
    unknown: u64,
}

fn review_stats_value_with_weights(
    sections: Option<&serde_json::Value>,
    weights: ReviewRiskWeights,
) -> serde_json::Value {
    let findings = sections
        .and_then(|s| s.get("findings"))
        .and_then(|v| v.as_array());
    let missing_tests_count = sections
        .and_then(|s| s.get("missing_tests"))
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(strip_review_list_prefix)
                .filter(|line| !line.eq_ignore_ascii_case("none"))
                .count() as u64
        })
        .unwrap_or(0);
    let mut counts = serde_json::json!({
        "critical": 0u64,
        "high": 0u64,
        "medium": 0u64,
        "low": 0u64,
        "unknown": 0u64
    });
    let mut top_risk_map: HashMap<String, ReviewRiskPathStat> = HashMap::new();
    let mut total = 0u64;
    let mut risk_score_total = 0u64;
    if let Some(items) = findings {
        for item in items {
            total += 1;
            let sev = item
                .get("normalized_severity")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let key = match sev {
                "critical" => "critical",
                "high" => "high",
                "medium" => "medium",
                "low" => "low",
                _ => "unknown",
            };
            if let Some(slot) = counts.get_mut(key) {
                let n = slot.as_u64().unwrap_or(0) + 1;
                *slot = serde_json::Value::from(n);
            }
            let weight = review_severity_weight(key, weights);
            risk_score_total += weight;
            let path = item
                .get("path")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty());
            if let Some(path) = path {
                let stat = top_risk_map.entry(path.to_string()).or_default();
                stat.findings += 1;
                stat.score += weight;
                match key {
                    "critical" => stat.critical += 1,
                    "high" => stat.high += 1,
                    "medium" => stat.medium += 1,
                    "low" => stat.low += 1,
                    _ => stat.unknown += 1,
                }
            }
        }
    }

    let mut top_risk_paths = top_risk_map.into_iter().collect::<Vec<_>>();
    top_risk_paths.sort_by(|(path_a, stat_a), (path_b, stat_b)| {
        stat_b
            .score
            .cmp(&stat_a.score)
            .then_with(|| stat_b.findings.cmp(&stat_a.findings))
            .then_with(|| path_a.cmp(path_b))
    });
    let top_risk_paths = top_risk_paths
        .into_iter()
        .take(10)
        .map(|(path, stat)| {
            serde_json::json!({
                "path": path,
                "risk_score": stat.score,
                "findings": stat.findings,
                "severity_counts": {
                    "critical": stat.critical,
                    "high": stat.high,
                    "medium": stat.medium,
                    "low": stat.low,
                    "unknown": stat.unknown
                }
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "has_findings": total > 0,
        "total_findings": total,
        "severity_counts": counts,
        "risk_score_total": risk_score_total,
        "missing_tests_count": missing_tests_count,
        "top_risk_paths": top_risk_paths
    })
}

fn review_stats_value(sections: Option<&serde_json::Value>) -> serde_json::Value {
    let weights = parse_review_risk_weights_from_env();
    review_stats_value_with_weights(sections, weights)
}

fn parse_structured_review_finding(block: &[String]) -> serde_json::Value {
    if block.is_empty() {
        return serde_json::json!({
            "severity": serde_json::Value::Null,
            "path": serde_json::Value::Null,
            "line": serde_json::Value::Null,
            "summary": "",
            "evidence": serde_json::Value::Null,
            "risk": serde_json::Value::Null,
            "raw": ""
        });
    }

    let header_raw = block[0].trim();
    let header = strip_review_list_prefix(header_raw);
    let (severity, rest) = parse_review_severity_prefix(header);

    let mut path: Option<String> = None;
    let mut line: Option<u64> = None;
    let summary: String;
    if let Some((location, rhs)) = rest.split_once(" - ") {
        let (p, ln) = parse_review_path_and_line(location);
        path = p;
        line = ln;
        summary = rhs.trim().to_string();
    } else {
        summary = rest.trim().to_string();
    }

    let mut evidence: Option<String> = None;
    let mut risk: Option<String> = None;
    for extra in block.iter().skip(1) {
        let t = extra.trim();
        if evidence.is_none() {
            evidence = parse_labeled_review_value(t, "Evidence");
        }
        if risk.is_none() {
            risk = parse_labeled_review_value(t, "Risk");
        }
    }
    let normalized_severity = normalize_review_severity(severity.as_deref());
    let location = compose_review_location(path.as_deref(), line);

    serde_json::json!({
        "severity": severity,
        "normalized_severity": normalized_severity,
        "path": path,
        "line": line,
        "location": location,
        "summary": summary,
        "evidence": evidence,
        "risk": risk,
        "raw": block
    })
}

fn parse_structured_review_findings(lines: &[String]) -> Vec<serde_json::Value> {
    if lines.len() == 1 && lines[0].trim().eq_ignore_ascii_case("none") {
        return Vec::new();
    }

    let mut blocks: Vec<Vec<String>> = Vec::new();
    for line in lines {
        if is_review_list_item_line(line) {
            blocks.push(vec![line.trim().to_string()]);
        } else if let Some(last) = blocks.last_mut() {
            last.push(line.trim().to_string());
        } else {
            blocks.push(vec![line.trim().to_string()]);
        }
    }
    blocks
        .into_iter()
        .filter(|b| !b.is_empty())
        .map(|b| parse_structured_review_finding(&b))
        .collect()
}

fn sorted_review_findings_for_display(findings: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let mut items = findings.to_vec();
    items.sort_by(|a, b| {
        let a_sev = a
            .get("normalized_severity")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .or_else(|| {
                normalize_review_severity(a.get("severity").and_then(|v| v.as_str()))
            });
        let b_sev = b
            .get("normalized_severity")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
            .or_else(|| {
                normalize_review_severity(b.get("severity").and_then(|v| v.as_str()))
            });
        let a_sev = review_severity_rank(a_sev.as_deref());
        let b_sev = review_severity_rank(b_sev.as_deref());
        a_sev
            .cmp(&b_sev)
            .then_with(|| {
                let a_path = a.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let b_path = b.get("path").and_then(|v| v.as_str()).unwrap_or("");
                a_path.cmp(b_path)
            })
            .then_with(|| {
                let a_line = a.get("line").and_then(|v| v.as_u64()).unwrap_or(u64::MAX);
                let b_line = b.get("line").and_then(|v| v.as_u64()).unwrap_or(u64::MAX);
                a_line.cmp(&b_line)
            })
            .then_with(|| {
                let a_summary = a.get("summary").and_then(|v| v.as_str()).unwrap_or("");
                let b_summary = b.get("summary").and_then(|v| v.as_str()).unwrap_or("");
                a_summary.cmp(b_summary)
            })
    });
    items
}

fn review_sections_value(text: &str) -> Option<serde_json::Value> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return None;
    }

    let idx_findings = find_section_header_index(&lines, "findings:")?;
    let idx_missing_tests = find_section_header_index(&lines, "missing tests:")?;
    let idx_open_questions = find_section_header_index(&lines, "open questions:")?;
    let idx_summary = find_section_header_index(&lines, "summary:")?;
    let starts = [idx_findings, idx_missing_tests, idx_open_questions, idx_summary];

    let findings_raw = collect_review_section_lines(&lines, &starts, 0);
    let findings = parse_structured_review_findings(&findings_raw);
    let findings_sorted = sorted_review_findings_for_display(&findings);
    let missing_tests = collect_review_section_lines(&lines, &starts, 1);
    let open_questions = collect_review_section_lines(&lines, &starts, 2);
    let summary = collect_review_section_lines(&lines, &starts, 3);

    Some(serde_json::json!({
        "findings": findings,
        "findings_sorted": findings_sorted,
        "findings_raw": findings_raw,
        "missing_tests": missing_tests,
        "open_questions": open_questions,
        "summary": summary
    }))
}

pub(crate) fn build_review_json_payload(
    task_input: &str,
    response_text: &str,
) -> serde_json::Value {
    let is_review = is_review_task_input(task_input);
    if !is_review {
        return serde_json::json!({
            "schema_version": REVIEW_JSON_SCHEMA_VERSION,
            "is_review_task": false,
            "schema_valid": false,
            "schema_error": serde_json::Value::Null,
            "sections": serde_json::Value::Null,
            "stats": serde_json::Value::Null
        });
    }

    let schema_error = review_output_schema_error(response_text);
    let sections = review_sections_value(response_text);
    let stats = review_stats_value(sections.as_ref());
    serde_json::json!({
        "schema_version": REVIEW_JSON_SCHEMA_VERSION,
        "is_review_task": true,
        "schema_valid": schema_error.is_none(),
        "schema_error": schema_error,
        "sections": sections,
        "stats": stats
    })
}

fn review_json_only_schema_error(
    review_payload: &serde_json::Value,
    strict_schema: bool,
) -> Option<String> {
    if !strict_schema {
        return None;
    }
    let is_review_task = review_payload
        .get("is_review_task")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !is_review_task {
        return None;
    }
    let schema_valid = review_payload
        .get("schema_valid")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if schema_valid {
        return None;
    }
    let schema_error = review_payload
        .get("schema_error")
        .and_then(|v| v.as_str())
        .unwrap_or("review schema validation failed");
    Some(schema_error.to_string())
}

fn build_review_schema_invalid_envelope(
    review_payload: &serde_json::Value,
    schema_error: &str,
) -> serde_json::Value {
    review_json_response(
        "error",
        serde_json::json!({
            "error": "review_schema_invalid",
            "schema_error": schema_error,
            "review": review_payload
        }),
    )
}

pub(crate) fn is_review_json_only_schema_invalid_envelope(value: &serde_json::Value) -> bool {
    value.get("status").and_then(|v| v.as_str()) == Some("error")
        && value.get("error").and_then(|v| v.as_str()) == Some("review_schema_invalid")
}

pub(crate) fn build_review_json_only_repl_output_with_strict(
    review_payload: &serde_json::Value,
    strict_schema: bool,
) -> serde_json::Value {
    if let Some(schema_error) = review_json_only_schema_error(review_payload, strict_schema) {
        return build_review_schema_invalid_envelope(review_payload, &schema_error);
    }
    review_json_response("ok", serde_json::json!({ "review": review_payload }))
}

pub(crate) fn build_review_json_only_repl_output(
    review_payload: &serde_json::Value,
) -> serde_json::Value {
    let strict_schema = parse_bool_env("ASI_REVIEW_JSON_ONLY_STRICT_SCHEMA", false);
    build_review_json_only_repl_output_with_strict(review_payload, strict_schema)
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn build_review_json_only_prompt_output_with_strict(
    review_payload: &serde_json::Value,
    strict_schema: bool,
) -> serde_json::Value {
    build_review_json_only_prompt_output_with_options(review_payload, strict_schema, false)
}

pub(crate) fn build_review_json_only_prompt_output_with_options(
    review_payload: &serde_json::Value,
    strict_schema: bool,
    wrap_success_envelope: bool,
) -> serde_json::Value {
    if let Some(schema_error) = review_json_only_schema_error(review_payload, strict_schema) {
        return build_review_schema_invalid_envelope(review_payload, &schema_error);
    }
    if wrap_success_envelope {
        review_json_response("ok", serde_json::json!({ "review": review_payload }))
    } else {
        review_payload.clone()
    }
}

fn review_json_response(status: &str, review: serde_json::Value) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "schema_version".to_string(),
        serde_json::Value::String(REVIEW_JSON_SCHEMA_VERSION.to_string()),
    );
    map.insert(
        "status".to_string(),
        serde_json::Value::String(status.to_string()),
    );
    if let Some(obj) = review.as_object() {
        for (k, v) in obj {
            map.insert(k.clone(), v.clone());
        }
    }
    serde_json::Value::Object(map)
}

pub(crate) fn build_review_json_only_prompt_output(
    review_payload: &serde_json::Value,
) -> serde_json::Value {
    let strict_schema = parse_bool_env("ASI_REVIEW_JSON_ONLY_STRICT_SCHEMA", false);
    let wrap_success_envelope =
        parse_bool_env("ASI_REVIEW_JSON_ONLY_PROMPT_ENVELOPE", false);
    if wrap_success_envelope {
        build_review_json_only_prompt_output_with_options(review_payload, strict_schema, true)
    } else {
        build_review_json_only_prompt_output_with_strict(review_payload, strict_schema)
    }
}

pub(crate) fn should_fail_review_json_only_prompt_output_with_strict_exit(
    review_json_only_output: &serde_json::Value,
    fail_on_schema_invalid: bool,
) -> bool {
    fail_on_schema_invalid && is_review_json_only_schema_invalid_envelope(review_json_only_output)
}

pub(crate) fn should_fail_review_json_only_prompt_output(
    review_json_only_output: &serde_json::Value,
) -> bool {
    let fail_on_schema_invalid = parse_bool_env("ASI_REVIEW_JSON_ONLY_STRICT_FAIL_EXIT", false);
    should_fail_review_json_only_prompt_output_with_strict_exit(
        review_json_only_output,
        fail_on_schema_invalid,
    )
}

fn build_review_schema_repair_prompt(
    context_contract: &str,
    task: &str,
    strict_mode: bool,
    schema_error: &str,
) -> String {
    let profile_tail = if strict_mode {
        "Keep strict review profile requirements unchanged."
    } else {
        "Keep review focus on bugs, regressions, risks, and missing tests."
    };
    let anchor = task_anchor_line(task);
    format!(
        "{}\n{}\n\nReview output schema check failed: {}.\nRe-emit the final response now using EXACT section headers:\nFindings:\n- [SEV] path:line - summary\n  Evidence: <short evidence>\n  Risk: <impact>\nMissing Tests:\n- <test gap or 'None'>\nOpen Questions:\n- <question or 'None'>\nSummary:\n- <1-3 bullets>\nDo not call additional tools unless absolutely necessary.\n{}",
        context_contract, anchor, schema_error, profile_tail
    )
}

fn build_bilingual_markdown_artifact_prompt(
    context_contract: &str,
    task: &str,
    strict_mode: bool,
) -> String {
    let profile_tail = if strict_mode {
        "Keep strict profile requirements unchanged."
    } else {
        "Follow the existing execution rules."
    };
    let anchor = task_anchor_line(task);
    format!(
        "{}\n{}\n\nTask completion check: required bilingual Markdown reports are still missing.\nCreate BOTH files now via /toolcall write_file or /toolcall edit_file:\n1) analysis_zh.md (Chinese report)\n2) analysis_en.md (English report)\nOutput only /toolcall lines while actions are pending. After both files are created, provide the final answer with exact file paths.\n{}",
        context_contract, anchor, profile_tail
    )
}

fn planned_toolcall_count(text: &str) -> usize {
    let engine = OrchestratorEngine::new(200, 4);
    engine
        .parse_and_plan(text)
        .iter()
        .map(|b| b.calls.len())
        .sum::<usize>()
}

fn run_repair_pass_streaming(
    rt: &mut Runtime,
    cfg: &AppConfig,
    ui: &Ui,
    strict_mode: bool,
) -> runtime::TurnResult {
    let repair_prompt = if strict_mode {
        auto_tool_repair_prompt_strict()
    } else {
        auto_tool_repair_prompt()
    };
    let mut repair_streamed = false;
    let repaired = rt.run_turn_streaming(repair_prompt, |delta| {
        if !repair_streamed {
            repair_streamed = true;
            println!();
            print!("{} • ", ui.assistant_label());
        }
        print!("{}", delta);
        let _ = io::stdout().flush();
    });
    log_interaction_event(cfg, rt, repair_prompt, &repaired);
    if repair_streamed {
        println!();
    }
    repaired
}

fn run_repair_pass(rt: &mut Runtime, cfg: &AppConfig, strict_mode: bool) -> runtime::TurnResult {
    let repair_prompt = if strict_mode {
        auto_tool_repair_prompt_strict()
    } else {
        auto_tool_repair_prompt()
    };
    let repaired = rt.run_turn(repair_prompt);
    log_interaction_event(cfg, rt, repair_prompt, &repaired);
    repaired
}

fn should_force_final_response_after_tool_result(result: &runtime::TurnResult) -> bool {
    result.is_tool_result
        || result.stop_reason.trim().eq_ignore_ascii_case("tool_result")
        || Runtime::stop_reason_alias(&result.stop_reason) == "tool_use"
}

fn run_confidence_gate_pass_streaming(
    rt: &mut Runtime,
    cfg: &AppConfig,
    ui: &Ui,
    prompt: &str,
) -> runtime::TurnResult {
    let mut streamed = false;
    let turn = rt.run_turn_streaming(prompt, |delta| {
        if !streamed {
            streamed = true;
            println!();
            print!("{} • ", ui.assistant_label());
        }
        print!("{}", delta);
        let _ = io::stdout().flush();
    });
    log_interaction_event(cfg, rt, prompt, &turn);
    if streamed {
        println!();
    }
    turn
}

fn run_confidence_gate_pass(
    rt: &mut Runtime,
    cfg: &AppConfig,
    prompt: &str,
) -> runtime::TurnResult {
    let turn = rt.run_turn(prompt);
    log_interaction_event(cfg, rt, prompt, &turn);
    turn
}

include!("orchestrator/loop.rs");

fn prepare_prompt_agent_input(
    text: &str,
    force_agent: bool,
    force_secure: bool,
    profile: PromptProfile,
) -> Result<(String, bool, bool), String> {
    let trimmed = text.trim();

    if trimmed.starts_with("/toolcall ") {
        return Ok((text.to_string(), false, false));
    }

    if let Some(task) = trimmed.strip_prefix("/work ") {
        let task = task.trim();
        if task.is_empty() {
            return Err("Usage in prompt text: /work <task>".to_string());
        }
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        let snapshot = build_workspace_snapshot(&root);
        return Ok((
            build_work_prompt(task, &snapshot, profile == PromptProfile::Strict),
            true,
            false,
        ));
    }

    if let Some(task) = trimmed.strip_prefix("/code ") {
        let task = task.trim();
        if task.is_empty() {
            return Err("Usage in prompt text: /code <task>".to_string());
        }
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        let snapshot = build_workspace_snapshot(&root);
        return Ok((
            build_work_prompt(task, &snapshot, profile == PromptProfile::Strict),
            true,
            false,
        ));
    }

    if let Some(task) = trimmed.strip_prefix("/secure ") {
        let task = task.trim();
        if task.is_empty() {
            return Err("Usage in prompt text: /secure <task>".to_string());
        }
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        let snapshot = build_workspace_snapshot(&root);
        return Ok((
            build_security_work_prompt(task, &snapshot, profile == PromptProfile::Strict),
            true,
            true,
        ));
    }

    if let Some((task, _json_only)) = parse_review_task_json_only(trimmed) {
        if task.is_empty() {
            return Err("Usage in prompt text: /review <task>".to_string());
        }
        let root = std::env::current_dir().map_err(|e| e.to_string())?;
        let snapshot = build_workspace_snapshot(&root);
        return Ok((
            build_review_prompt(&task, &snapshot, profile == PromptProfile::Strict),
            true,
            false,
        ));
    }

    let should_agent = force_agent || force_secure || is_coding_intent(trimmed);
    if !should_agent {
        return Ok((text.to_string(), false, false));
    }

    let root = std::env::current_dir().map_err(|e| e.to_string())?;
    let snapshot = build_workspace_snapshot(&root);
    if force_secure {
        Ok((
            build_security_work_prompt(trimmed, &snapshot, profile == PromptProfile::Strict),
            true,
            true,
        ))
    } else {
        Ok((
            build_work_prompt(trimmed, &snapshot, profile == PromptProfile::Strict),
            true,
            false,
        ))
    }
}

fn collect_native_changed_paths(native_tool_calls: &[runtime::NativeToolResult]) -> Vec<String> {
    let mut out = Vec::new();
    for nr in native_tool_calls {
        if !nr.ok {
            continue;
        }
        if !matches!(nr.tool_name.as_str(), "write_file" | "edit_file") {
            continue;
        }

        if let Some(path) = parse_file_path_arg(&nr.tool_args_display) {
            push_unique_changed_file(&mut out, &path);
        }
    }
    out
}

fn collect_native_changed_files(
    native_tool_calls: &[runtime::NativeToolResult],
    changed_files: &mut Vec<String>,
) {
    for path in collect_native_changed_paths(native_tool_calls) {
        push_unique_changed_file(changed_files, &path);
    }
}

fn should_skip_workspace_entry(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | "target"
            | "node_modules"
            | ".idea"
            | ".vscode"
            | "__pycache__"
            | ".venv"
            | "venv"
            | ".pytest_cache"
            | ".mypy_cache"
    )
}

fn clip_chars(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in text.chars().enumerate() {
        if i >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn collect_workspace_entries(
    root: &Path,
    dir: &Path,
    depth: usize,
    max_depth: usize,
    limit: usize,
    out: &mut Vec<String>,
) {
    if out.len() >= limit || depth > max_depth {
        return;
    }

    let mut entries: Vec<_> = match fs::read_dir(dir) {
        Ok(v) => v.filter_map(Result::ok).collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name().to_string_lossy().to_ascii_lowercase());

    for entry in entries {
        if out.len() >= limit {
            return;
        }

        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if should_skip_workspace_entry(&name) {
            continue;
        }

        let rel = path.strip_prefix(root).unwrap_or(&path);
        let rel_display = rel.to_string_lossy().replace('\\', "/");
        if rel_display.is_empty() {
            continue;
        }

        if path.is_dir() {
            out.push(format!("- {}/", rel_display));
            if depth < max_depth {
                collect_workspace_entries(root, &path, depth + 1, max_depth, limit, out);
            }
        } else {
            out.push(format!("- {}", rel_display));
        }
    }
}

fn read_brief_file(path: &Path, max_chars: usize) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let joined = content.lines().take(40).collect::<Vec<_>>().join("\n");
    Some(clip_chars(joined.trim(), max_chars))
}

fn build_workspace_snapshot(root: &Path) -> String {
    let mut lines = Vec::new();
    lines.push(format!("workspace_root={}", root.display()));

    // Git info for workspace context
    let git_info = collect_git_info(root);
    if !git_info.is_empty() {
        lines.push(format!("git_info:\n{}", git_info));
    }

    let mut entries = Vec::new();
    collect_workspace_entries(root, root, 0, 2, 80, &mut entries);
    if entries.is_empty() {
        lines.push("workspace_entries=(empty)".to_string());
    } else {
        lines.push("workspace_entries:".to_string());
        lines.extend(entries);
    }

    let key_files = [
        "CLAUDE.md",
        "AGENTS.md",
        "README.md",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
    ];

    let mut snippet_added = false;
    for name in key_files {
        let p = root.join(name);
        if p.exists() {
            if let Some(snippet) = read_brief_file(&p, 700) {
                if !snippet_added {
                    lines.push("key_file_snippets:".to_string());
                    snippet_added = true;
                }
                lines.push(format!("[{}]", name));
                lines.push(snippet);
            }
        }
    }

    lines.join("\n")
}

/// Load the user-level and project-level skill registries for a REPL
/// session. Surface load errors via the UI but never fail startup.
fn load_skill_registry_for_repl(_cfg: &AppConfig, ui: &Ui) -> skills::SkillRegistry {
    let user_dir = skills::default_user_skill_dir();
    let project_root = std::env::current_dir().ok();
    let project_dir = project_root.as_deref().map(skills::default_project_skill_dir);
    let reg = skills::SkillRegistry::load(user_dir.as_deref(), project_dir.as_deref());
    let count = reg.list().len();
    if count > 0 {
        println!("{}", ui.info(&format!("loaded {} skill(s)", count)));
    }
    for err in &reg.load_errors {
        println!(
            "{}",
            ui.error(&format!(
                "skill load error at {}: {}",
                err.path.display(),
                err.message
            ))
        );
    }
    reg
}

/// Render the registered skill body and return a workspace task prompt
/// suitable for `/code` mode. Prints a short banner and any out-of-scope
/// `allowed_tools` warning. Currently unused by the REPL fast path
/// (which inlines the skill render) but kept available for callers that
/// want a single-shot helper.
#[allow(dead_code)]
fn build_skill_task_input(
    skill: &skills::Skill,
    args: &str,
    strict_mode: bool,
    ui: &Ui,
) -> String {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let today = current_date_iso();
    let rendered = skill.render(args, &cwd, &today);
    let snapshot = build_workspace_snapshot(&cwd);
    println!(
        "{}",
        ui.info(&format!(
            "skill: {} ({})",
            skill.fqname(),
            match skill.source_kind {
                skills::SkillSource::User => "user",
                skills::SkillSource::Project => "project",
            }
        ))
    );
    if !skill.allowed_tools.is_empty() {
        println!(
            "{}",
            ui.info(&format!(
                "skill allowed_tools: {}",
                skill.allowed_tools.join(", ")
            ))
        );
    }
    build_work_prompt(&rendered, &snapshot, strict_mode)
}

/// Pretty-print the skill registry. Groups by namespace and shows source
/// (user vs project) plus the one-line description.
fn print_skill_list(reg: &skills::SkillRegistry, ui: &Ui) {
    let skills_vec = reg.list();
    if skills_vec.is_empty() {
        println!(
            "{}",
            ui.info(
                "no skills loaded; drop *.md files into ~/.asi/skills/ or .asi/skills/ to add some"
            )
        );
        return;
    }
    println!("{}", ui.info(&format!("skills ({}):", skills_vec.len())));
    for skill in skills_vec {
        let src = match skill.source_kind {
            skills::SkillSource::User => "user",
            skills::SkillSource::Project => "project",
        };
        let ns = skill
            .namespace
            .as_deref()
            .map(|n| format!("{}:", n))
            .unwrap_or_default();
        let desc = if skill.description.is_empty() {
            "(no description)"
        } else {
            skill.description.as_str()
        };
        println!("  /{}{} [{}] {}", ns, skill.name, src, desc);
    }
    if !reg.load_errors.is_empty() {
        for err in &reg.load_errors {
            println!(
                "{}",
                ui.error(&format!(
                    "load error at {}: {}",
                    err.path.display(),
                    err.message
                ))
            );
        }
    }
}

/// Render the result of `/worktree list` to stdout.
fn handle_worktree_list(ui: &Ui) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    match worktree::list(&cwd) {
        Ok(entries) => {
            if entries.is_empty() {
                println!("{}", ui.info("no worktrees"));
                return;
            }
            println!("{}", ui.info(&format!("worktrees ({}):", entries.len())));
            for w in entries {
                let lock = if w.locked { " (locked)" } else { "" };
                let bare = if w.bare { " (bare)" } else { "" };
                println!("  {} -> {}{}{}", w.branch, w.path.display(), lock, bare);
            }
        }
        Err(e) => println!("{}", ui.error(&format!("worktree list: {}", e))),
    }
}

/// Dispatch `/worktree <subcommand> ...` lines.
fn handle_worktree_command(rest: &str, session: &mut worktree::WorktreeSession, ui: &Ui) {
    let mut parts = rest.split_whitespace();
    let sub = parts.next().unwrap_or("");
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    match sub {
        "list" => handle_worktree_list(ui),
        "create" => {
            let branch = match parts.next() {
                Some(b) => b,
                None => {
                    println!(
                        "{}",
                        ui.error("Usage: /worktree create <branch> [base-ref]")
                    );
                    return;
                }
            };
            let base = parts.next();
            match worktree::create(&cwd, branch, base) {
                Ok(info) => println!(
                    "{}",
                    ui.info(&format!(
                        "created worktree {} at {}",
                        info.branch,
                        info.path.display()
                    ))
                ),
                Err(e) => println!("{}", ui.error(&format!("worktree create: {}", e))),
            }
        }
        "remove" => {
            let mut force = false;
            let mut branch: Option<&str> = None;
            for p in parts {
                if p == "--force" || p == "-f" {
                    force = true;
                } else if branch.is_none() {
                    branch = Some(p);
                }
            }
            let branch = match branch {
                Some(b) => b,
                None => {
                    println!(
                        "{}",
                        ui.error("Usage: /worktree remove <branch> [--force]")
                    );
                    return;
                }
            };
            match worktree::remove(&cwd, branch, force) {
                Ok(()) => println!(
                    "{}",
                    ui.info(&format!("removed worktree for branch '{}'", branch))
                ),
                Err(e) => println!("{}", ui.error(&format!("worktree remove: {}", e))),
            }
        }
        "enter" => {
            let branch = match parts.next() {
                Some(b) => b,
                None => {
                    println!("{}", ui.error("Usage: /worktree enter <branch>"));
                    return;
                }
            };
            match worktree::enter(&cwd, branch, session) {
                Ok(target) => {
                    if let Err(e) = std::env::set_current_dir(&target) {
                        println!(
                            "{}",
                            ui.error(&format!(
                                "cd into {} failed: {}",
                                target.display(),
                                e
                            ))
                        );
                        // Roll back the session record so /worktree exit
                        // does not pretend we're inside.
                        session.original_root = None;
                        session.current_branch = None;
                        return;
                    }
                    println!(
                        "{}",
                        ui.info(&format!(
                            "entered worktree '{}' at {}",
                            branch,
                            target.display()
                        ))
                    );
                }
                Err(e) => println!("{}", ui.error(&format!("worktree enter: {}", e))),
            }
        }
        "exit" => {
            let mut action = worktree::ExitAction::Keep;
            let mut force = false;
            for p in parts {
                match p {
                    "keep" => action = worktree::ExitAction::Keep,
                    "remove" => action = worktree::ExitAction::Remove { force },
                    "--force" | "-f" => {
                        force = true;
                        if matches!(action, worktree::ExitAction::Remove { .. }) {
                            action = worktree::ExitAction::Remove { force: true };
                        }
                    }
                    _ => {}
                }
            }
            match worktree::exit(session, action) {
                Ok(outcome) => match outcome {
                    worktree::ExitOutcome::Kept { original_root, branch } => {
                        if let Err(e) = std::env::set_current_dir(&original_root) {
                            println!(
                                "{}",
                                ui.error(&format!(
                                    "cd back to {} failed: {}",
                                    original_root.display(),
                                    e
                                ))
                            );
                            return;
                        }
                        let label = branch.as_deref().unwrap_or("(unknown)");
                        println!(
                            "{}",
                            ui.info(&format!(
                                "exited worktree '{}' (kept on disk)",
                                label
                            ))
                        );
                    }
                    worktree::ExitOutcome::Removed { original_root, branch } => {
                        if let Err(e) = std::env::set_current_dir(&original_root) {
                            println!(
                                "{}",
                                ui.error(&format!(
                                    "cd back to {} failed: {}",
                                    original_root.display(),
                                    e
                                ))
                            );
                            return;
                        }
                        println!(
                            "{}",
                            ui.info(&format!(
                                "exited and removed worktree '{}'",
                                branch
                            ))
                        );
                    }
                },
                Err(e) => println!("{}", ui.error(&format!("worktree exit: {}", e))),
            }
        }
        other => {
            println!(
                "{}",
                ui.error(&format!(
                    "unknown /worktree subcommand '{}': expected create|list|remove|enter|exit",
                    other
                ))
            );
        }
    }
}

/// Render `/cron list` to stdout. Loads from `~/.asi/cron.json` each call
/// so external edits are reflected.
fn handle_cron_list(ui: &Ui) {
    let path = match cron::default_store_path() {
        Some(p) => p,
        None => {
            println!(
                "{}",
                ui.error("cannot resolve home directory; set HOME or USERPROFILE")
            );
            return;
        }
    };
    let store = cron::CronStore::load(&path);
    if store.jobs.is_empty() {
        println!("{}", ui.info("no cron jobs"));
        return;
    }
    println!("{}", ui.info(&format!("cron jobs ({}):", store.jobs.len())));
    for j in &store.jobs {
        let kind = if j.recurring { "recurring" } else { "one-shot" };
        let preview: String = j.prompt.chars().take(60).collect();
        println!(
            "  {} [{}] '{}' -> {}",
            j.id, kind, j.cron, preview
        );
    }
}

/// Dispatch `/cron <subcommand> ...` lines.
fn handle_cron_command(rest: &str, ui: &Ui) {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let sub = parts.next().unwrap_or("");
    let rest_args = parts.next().unwrap_or("").trim();
    let path = match cron::default_store_path() {
        Some(p) => p,
        None => {
            println!(
                "{}",
                ui.error("cannot resolve home directory; set HOME or USERPROFILE")
            );
            return;
        }
    };
    let mut store = cron::CronStore::load(&path);
    match sub {
        "list" => {
            handle_cron_list(ui);
        }
        "create" | "create-recurring" | "create-once" => {
            // Format: <CRON_EXPR with 5 fields> <prompt...>
            // We split off the first 5 whitespace-delimited fields as the
            // cron expression and treat the rest as the prompt.
            let mut iter = rest_args.split_whitespace();
            let mut cron_fields: Vec<&str> = Vec::with_capacity(5);
            for _ in 0..5 {
                match iter.next() {
                    Some(v) => cron_fields.push(v),
                    None => {
                        println!(
                            "{}",
                            ui.error(
                                "Usage: /cron create <M H DoM Mon DoW> <prompt...>"
                            )
                        );
                        return;
                    }
                }
            }
            let prompt: String = iter.collect::<Vec<_>>().join(" ");
            if prompt.trim().is_empty() {
                println!(
                    "{}",
                    ui.error("Usage: /cron create <M H DoM Mon DoW> <prompt...>")
                );
                return;
            }
            let cron_expr = cron_fields.join(" ");
            let recurring = sub != "create-once";
            match store.add(cron_expr.clone(), prompt.clone(), recurring) {
                Ok(job) => {
                    let id = job.id.clone();
                    if let Err(e) = store.save(&path) {
                        println!("{}", ui.error(&format!("save cron store: {}", e)));
                        return;
                    }
                    println!(
                        "{}",
                        ui.info(&format!(
                            "created {} '{}' [{}] -> {}",
                            id,
                            cron_expr,
                            if recurring { "recurring" } else { "one-shot" },
                            prompt.chars().take(80).collect::<String>()
                        ))
                    );
                }
                Err(e) => println!("{}", ui.error(&format!("cron create: {}", e))),
            }
        }
        "delete" | "remove" => {
            let id = rest_args.trim();
            if id.is_empty() {
                println!("{}", ui.error("Usage: /cron delete <id>"));
                return;
            }
            match store.remove(id) {
                Ok(()) => {
                    if let Err(e) = store.save(&path) {
                        println!("{}", ui.error(&format!("save cron store: {}", e)));
                        return;
                    }
                    println!("{}", ui.info(&format!("deleted {}", id)));
                }
                Err(e) => println!("{}", ui.error(&format!("cron delete: {}", e))),
            }
        }
        "run-once" => {
            let id = rest_args.trim();
            if id.is_empty() {
                println!("{}", ui.error("Usage: /cron run-once <id>"));
                return;
            }
            let Some(job) = store.get(id) else {
                println!("{}", ui.error(&format!("no cron job with id '{}'", id)));
                return;
            };
            // Print the job's prompt to stdout so the user can copy it
            // straight into the next REPL turn. The full scheduler that
            // dispatches into the agent loop lives in src/agentd and is
            // not invoked here to avoid coupling /cron run-once to job
            // queue behaviour during tests.
            println!(
                "{}",
                ui.info(&format!(
                    "{}: prompt to run -> {}",
                    job.id, job.prompt
                ))
            );
        }
        other => {
            println!(
                "{}",
                ui.error(&format!(
                    "unknown /cron subcommand '{}': expected create|create-once|list|delete|run-once",
                    other
                ))
            );
        }
    }
}

fn current_date_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Lightweight YYYY-MM-DD without a chrono dependency. Accurate to the
    // day in UTC, which matches how Claude Code stamps `{{date}}`.
    let days = (secs / 86_400) as i64;
    let (year, month, day) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

/// Convert "days since 1970-01-01 (UTC)" to (year, month, day).
fn days_to_ymd(days_since_epoch: i64) -> (i32, u32, u32) {
    // Algorithm from Howard Hinnant; works for any reasonable date.
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32)
}

fn build_work_prompt(task: &str, snapshot: &str, strict_mode: bool) -> String {
    if strict_mode {
        return format!(
            "Work in the current local project using tools.\\n\\nWorkspace snapshot:\\n{}\\n\\nTask:\\n{}\\n\\nStrict execution profile:\\n1) Inspect relevant files first with /toolcall glob_search, grep_search, and chunked /toolcall read_file <path> <start_line> <max_lines> (max 240 lines each call).\\n2) Actions must be plain /toolcall lines only (no prose when actions are pending).\\n3) For shell commands on Windows, use PowerShell syntax (`;` separators, not `&&`). Do not use plain `tail`; use PowerShell equivalents.\\n4) Do not run git branch-switching/history-rewrite commands (for example `git checkout`, `git switch`, `git reset`) unless the user explicitly asks.\\n5) If the user says not to use a tool/package manager (for example uv), do not use it. Choose alternatives that follow the user's constraint.\\n6) After edits, run concrete validation commands. If validation fails, emit corrective /toolcall lines and retry until pass or blocked.\\n7) If the user explicitly asks for output artifacts (for example reports or docs), create those files directly via write_file/edit_file in this run and report the exact paths.\\n8) Do not ask clarification questions when required paths/filenames are already explicit in the task. Execute first, then summarize results.",
            snapshot, task
        );
    }
    format!(
        "Work in the current local project using tools.\\n\\nWorkspace snapshot:\\n{}\\n\\nTask:\\n{}\\n\\nExecution rules:\\n1) Inspect relevant files first with /toolcall glob_search, grep_search, and chunked /toolcall read_file <path> <start_line> <max_lines> (max 240 lines each call).\\n2) Make focused edits with /toolcall write_file or edit_file.\\n3) Run checks with /toolcall bash when useful. On Windows use PowerShell syntax (`;` separators, not `&&`).\\n4) If the user explicitly asks for output artifacts (for example reports or docs), create those files directly via write_file/edit_file in this run and report the exact paths.\\n5) Do not ask clarification questions when required paths/filenames are already explicit in the task. Execute first, then summarize results.\\n6) If more actions are needed, output /toolcall lines only. Otherwise provide the final answer.",
        snapshot, task
    )
}

fn build_security_work_prompt(task: &str, snapshot: &str, strict_mode: bool) -> String {
    if strict_mode {
        return format!(
            "Security hardening task in the current local project.\\n\\nWorkspace snapshot:\\n{}\\n\\nTask:\\n{}\\n\\nStrict security profile:\\n1) Inspect risk areas first with /toolcall glob_search, grep_search, and read_file (inputs, shell commands, file paths, auth, secrets).\\n2) Emit only /toolcall lines while actions are pending; keep each step minimal and verifiable.\\n3) Validate with /toolcall bash (tests/build/lint/security checks). If a guard fails, emit corrective calls and retry.\\n4) Never claim success unless tool output confirms it.\\n5) End with concise summary including changed files and verification outcomes.",
            snapshot, task
        );
    }
    format!(
        "Security hardening task in the current local project.\\n\\nWorkspace snapshot:\\n{}\\n\\nTask:\\n{}\\n\\nSecurity rules:\\n1) Inspect risk areas first with /toolcall glob_search, grep_search, and read_file (inputs, shell commands, file paths, auth, secrets).\\n2) Apply minimal safe fixes with /toolcall write_file or edit_file.\\n3) Validate with /toolcall bash (tests/build/lint/security checks) when useful. On Windows use PowerShell syntax (`;` separators, not `&&`).\\n4) Never claim success unless tool output confirms it.\\n5) If more actions are needed, output /toolcall lines only; otherwise return a concise summary with changed files.",
        snapshot, task
    )
}

fn build_review_prompt(task: &str, snapshot: &str, strict_mode: bool) -> String {
    if strict_mode {
        return format!(
            "Code review task in the current local project.\\n\\nWorkspace snapshot:\\n{}\\n\\nTask:\\n{}\\n\\nStrict review profile:\\n1) Prioritize findings only: bugs, behavior regressions, security risks, missing tests, and broken assumptions.\\n2) Inspect relevant files first with /toolcall glob_search, grep_search, and chunked read_file calls.\\n3) Validate critical findings with concrete evidence (line refs and/or tool outputs).\\n4) Do not spend tokens on style-only feedback unless style causes correctness risk.\\n5) Output EXACTLY in this format:\\nFindings:\\n- [SEV] path:line - summary\\n  Evidence: <short evidence>\\n  Risk: <impact>\\nMissing Tests:\\n- <test gap or 'None'>\\nOpen Questions:\\n- <question or 'None'>\\nSummary:\\n- <1-3 bullets>",
            snapshot, task
        );
    }
    format!(
        "Code review task in the current local project.\\n\\nWorkspace snapshot:\\n{}\\n\\nTask:\\n{}\\n\\nReview rules:\\n1) Focus on bugs, regressions, risks, and missing tests.\\n2) Inspect files first with glob_search/grep_search/read_file before conclusions.\\n3) Cite file paths and line numbers for each finding.\\n4) Keep style commentary minimal unless it affects correctness.\\n5) Output sections: Findings / Missing Tests / Open Questions / Summary.",
        snapshot, task
    )
}

#[derive(Debug, Clone)]
enum AgentOutputMode {
    Text,
    Json,
    Jsonl,
}

#[derive(Debug, Clone)]
struct AgentSendOptions {
    output_mode: AgentOutputMode,
    interrupt: bool,
    no_context: bool,
    id: String,
    task: String,
}

#[derive(Debug, Clone)]
struct AgentWaitOptions {
    output_mode: AgentOutputMode,
    target_id: Option<String>,
    timeout_secs: u64,
}

#[derive(Debug, Clone)]
struct AgentListOptions {
    output_mode: AgentOutputMode,
    scope: AgentViewScope,
    profile: Option<String>,
    skill: Option<String>,
}

#[derive(Debug, Clone)]
struct AgentStatusOptions {
    output_mode: AgentOutputMode,
    id: String,
}

#[derive(Debug, Clone)]
struct AgentCloseOptions {
    output_mode: AgentOutputMode,
    id: String,
}

#[derive(Debug, Clone)]
struct AgentLogOptions {
    output_mode: AgentOutputMode,
    id: String,
    tail: Option<usize>,
}

#[derive(Debug, Clone)]
struct AgentRetryOptions {
    output_mode: AgentOutputMode,
    id: String,
}

#[derive(Debug, Clone)]
struct AgentCancelOptions {
    output_mode: AgentOutputMode,
    id: String,
}

#[derive(Debug, Clone)]
struct AgentSpawnOptions {
    output_mode: AgentOutputMode,
    profile: Option<String>,
    background: bool,
    task: String,
}

fn parse_agent_send_options(raw: &str) -> Result<AgentSendOptions, String> {
    let trimmed = raw.trim();
    let usage = agent_usage_includes_jsonl("Usage: /agent send [--json] [--interrupt] [--no-context] <id> <task>");
    if trimmed.is_empty() {
        return Err(usage.to_string());
    }

    let mut output_mode = AgentOutputMode::Text;
    let mut interrupt = false;
    let mut no_context = false;
    let mut rest = trimmed;
    loop {
        if let Some(stripped) = rest.strip_prefix("--json ") {
            output_mode = AgentOutputMode::Json;
            rest = stripped.trim();
            continue;
        }
        if let Some(stripped) = rest.strip_prefix("--jsonl ") {
            output_mode = AgentOutputMode::Jsonl;
            rest = stripped.trim();
            continue;
        }
        if let Some(stripped) = rest.strip_prefix("--interrupt ") {
            interrupt = true;
            rest = stripped.trim();
            continue;
        }
        if let Some(stripped) = rest.strip_prefix("--no-context ") {
            no_context = true;
            rest = stripped.trim();
            continue;
        }
        break;
    }

    let mut it = rest.splitn(2, ' ');
    let id = it.next().unwrap_or("").trim();
    let task = it.next().unwrap_or("").trim();
    if id.is_empty() || task.is_empty() || id.starts_with("--") {
        return Err(usage.to_string());
    }
    Ok(AgentSendOptions {
        output_mode,
        interrupt,
        no_context,
        id: id.to_string(),
        task: task.to_string(),
    })
}

fn parse_agent_wait_options(raw: &str) -> Result<AgentWaitOptions, String> {
    let usage = agent_usage_includes_jsonl("Usage: /agent wait [--json] [id] [timeout_secs]");
    let mut output_mode = AgentOutputMode::Text;
    let mut positional: Vec<&str> = Vec::new();

    for token in raw.split_whitespace().skip(1) {
        if token.eq_ignore_ascii_case("--json") {
            output_mode = AgentOutputMode::Json;
            continue;
        }
        if token.eq_ignore_ascii_case("--jsonl") {
            output_mode = AgentOutputMode::Jsonl;
            continue;
        }
        if token.starts_with("--") {
            return Err(usage.to_string());
        }
        positional.push(token);
    }

    let mut target_id: Option<String> = None;
    let mut timeout_secs: u64 = 60;
    match positional.len() {
        0 => {}
        1 => {
            if let Ok(v) = positional[0].parse::<u64>() {
                timeout_secs = v;
            } else {
                target_id = Some(positional[0].to_string());
            }
        }
        2 => {
            if positional[0].parse::<u64>().is_ok() {
                timeout_secs = positional[1]
                    .parse::<u64>()
                    .map_err(|_| usage.to_string())?;
            } else {
                target_id = Some(positional[0].to_string());
                timeout_secs = positional[1]
                    .parse::<u64>()
                    .map_err(|_| usage.to_string())?;
            }
        }
        _ => return Err(usage.to_string()),
    }

    Ok(AgentWaitOptions {
        output_mode,
        target_id,
        timeout_secs,
    })
}

fn parse_agent_list_options(raw: &str) -> Result<AgentListOptions, String> {
    let usage = agent_usage_includes_jsonl(
        "Usage: /agent list [--json] [--scope <foreground|background|all>] [--profile <name>] [--skill <name>]"
    );
    let mut output_mode = AgentOutputMode::Text;
    let mut scope = AgentViewScope::All;
    let mut profile: Option<String> = None;
    let mut skill: Option<String> = None;

    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let mut i = 1usize;
    while i < tokens.len() {
        let token = tokens[i];
        if token.eq_ignore_ascii_case("--json") {
            output_mode = AgentOutputMode::Json;
            i += 1;
            continue;
        }
        if token.eq_ignore_ascii_case("--jsonl") {
            output_mode = AgentOutputMode::Jsonl;
            i += 1;
            continue;
        }
        if token.eq_ignore_ascii_case("--scope") {
            let value = tokens.get(i + 1).ok_or_else(|| usage.to_string())?;
            scope = AgentViewScope::from_opt(Some(value)).ok_or_else(|| usage.to_string())?;
            i += 2;
            continue;
        }
        if token.eq_ignore_ascii_case("--profile") {
            let value = tokens.get(i + 1).ok_or_else(|| usage.to_string())?;
            let v = value.trim();
            if v.is_empty() || v.starts_with("--") {
                return Err(usage.to_string());
            }
            profile = Some(v.to_string());
            i += 2;
            continue;
        }
        if token.eq_ignore_ascii_case("--skill") {
            let value = tokens.get(i + 1).ok_or_else(|| usage.to_string())?;
            let v = value.trim();
            if v.is_empty() || v.starts_with("--") {
                return Err(usage.to_string());
            }
            skill = Some(v.to_string());
            i += 2;
            continue;
        }
        return Err(usage.to_string());
    }

    Ok(AgentListOptions {
        output_mode,
        scope,
        profile,
        skill,
    })
}

fn parse_agent_status_options(raw: &str) -> Result<AgentStatusOptions, String> {
    let usage = agent_usage_includes_jsonl("Usage: /agent status [--json] <id>");
    let mut output_mode = AgentOutputMode::Text;
    let mut id: Option<String> = None;
    for token in raw.split_whitespace().skip(1) {
        if token.eq_ignore_ascii_case("--json") {
            output_mode = AgentOutputMode::Json;
            continue;
        }
        if token.eq_ignore_ascii_case("--jsonl") {
            output_mode = AgentOutputMode::Jsonl;
            continue;
        }
        if token.starts_with("--") {
            return Err(usage.to_string());
        }
        if id.is_none() {
            id = Some(token.to_string());
        } else {
            return Err(usage.to_string());
        }
    }
    let id = id.ok_or_else(|| usage.to_string())?;
    Ok(AgentStatusOptions { output_mode, id })
}

fn parse_agent_close_options(raw: &str) -> Result<AgentCloseOptions, String> {
    let usage = agent_usage_includes_jsonl("Usage: /agent close [--json] <id>");
    let mut output_mode = AgentOutputMode::Text;
    let mut id: Option<String> = None;
    for token in raw.split_whitespace().skip(1) {
        if token.eq_ignore_ascii_case("--json") {
            output_mode = AgentOutputMode::Json;
            continue;
        }
        if token.eq_ignore_ascii_case("--jsonl") {
            output_mode = AgentOutputMode::Jsonl;
            continue;
        }
        if token.starts_with("--") {
            return Err(usage.to_string());
        }
        if id.is_none() {
            id = Some(token.to_string());
        } else {
            return Err(usage.to_string());
        }
    }
    let id = id.ok_or_else(|| usage.to_string())?;
    Ok(AgentCloseOptions { output_mode, id })
}

fn parse_agent_log_options(raw: &str) -> Result<AgentLogOptions, String> {
    let usage = agent_usage_includes_jsonl("Usage: /agent log [--json] <id> [--tail <n>]");
    let mut output_mode = AgentOutputMode::Text;
    let mut id: Option<String> = None;
    let mut tail: Option<usize> = None;
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let mut i = 1usize;
    while i < tokens.len() {
        let token = tokens[i];
        if token.eq_ignore_ascii_case("--json") {
            output_mode = AgentOutputMode::Json;
            i += 1;
            continue;
        }
        if token.eq_ignore_ascii_case("--jsonl") {
            output_mode = AgentOutputMode::Jsonl;
            i += 1;
            continue;
        }
        if token.eq_ignore_ascii_case("--tail") {
            let value = tokens.get(i + 1).ok_or_else(|| usage.to_string())?;
            let n = value.parse::<usize>().map_err(|_| usage.to_string())?;
            if n == 0 {
                return Err(usage.to_string());
            }
            tail = Some(n);
            i += 2;
            continue;
        }
        if token.starts_with("--") {
            return Err(usage.to_string());
        }
        if id.is_none() {
            id = Some(token.to_string());
            i += 1;
            continue;
        }
        return Err(usage.to_string());
    }
    let id = id.ok_or_else(|| usage.to_string())?;
    Ok(AgentLogOptions {
        output_mode,
        id,
        tail,
    })
}

fn parse_agent_retry_options(raw: &str) -> Result<AgentRetryOptions, String> {
    let usage = agent_usage_includes_jsonl("Usage: /agent retry [--json] <id>");
    let mut output_mode = AgentOutputMode::Text;
    let mut id: Option<String> = None;
    for token in raw.split_whitespace().skip(1) {
        if token.eq_ignore_ascii_case("--json") {
            output_mode = AgentOutputMode::Json;
            continue;
        }
        if token.eq_ignore_ascii_case("--jsonl") {
            output_mode = AgentOutputMode::Jsonl;
            continue;
        }
        if token.starts_with("--") {
            return Err(usage.to_string());
        }
        if id.is_none() {
            id = Some(token.to_string());
        } else {
            return Err(usage.to_string());
        }
    }
    let id = id.ok_or_else(|| usage.to_string())?;
    Ok(AgentRetryOptions { output_mode, id })
}

fn parse_agent_cancel_options(raw: &str) -> Result<AgentCancelOptions, String> {
    let usage = agent_usage_includes_jsonl("Usage: /agent cancel [--json] <id>");
    let mut output_mode = AgentOutputMode::Text;
    let mut id: Option<String> = None;
    for token in raw.split_whitespace().skip(1) {
        if token.eq_ignore_ascii_case("--json") {
            output_mode = AgentOutputMode::Json;
            continue;
        }
        if token.eq_ignore_ascii_case("--jsonl") {
            output_mode = AgentOutputMode::Jsonl;
            continue;
        }
        if token.starts_with("--") {
            return Err(usage.to_string());
        }
        if id.is_none() {
            id = Some(token.to_string());
        } else {
            return Err(usage.to_string());
        }
    }
    let id = id.ok_or_else(|| usage.to_string())?;
    Ok(AgentCancelOptions { output_mode, id })
}

fn parse_agent_spawn_options(raw: &str) -> Result<AgentSpawnOptions, String> {
    let usage = agent_usage_includes_jsonl(
        "Usage: /agent spawn [--json] [--profile <name>] [--background|--foreground] <task>"
    );
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(usage.to_string());
    }

    let body = if let Some(rest) = trimmed.strip_prefix("spawn ") {
        rest.trim()
    } else {
        trimmed
    };

    let mut output_mode = AgentOutputMode::Text;
    let mut profile: Option<String> = None;
    let mut background = false;
    let mut rest = body;
    loop {
        if let Some(stripped) = rest.strip_prefix("--json ") {
            output_mode = AgentOutputMode::Json;
            rest = stripped.trim();
            continue;
        }
        if rest.eq_ignore_ascii_case("--json") {
            return Err(usage.to_string());
        }
        if let Some(stripped) = rest.strip_prefix("--jsonl ") {
            output_mode = AgentOutputMode::Jsonl;
            rest = stripped.trim();
            continue;
        }
        if rest.eq_ignore_ascii_case("--jsonl") {
            return Err(usage.to_string());
        }
        if let Some(stripped) = rest.strip_prefix("--background ") {
            background = true;
            rest = stripped.trim();
            continue;
        }
        if rest.eq_ignore_ascii_case("--background") {
            return Err(usage.to_string());
        }
        if let Some(stripped) = rest.strip_prefix("--foreground ") {
            background = false;
            rest = stripped.trim();
            continue;
        }
        if rest.eq_ignore_ascii_case("--foreground") {
            return Err(usage.to_string());
        }
        if let Some(stripped) = rest.strip_prefix("--profile ") {
            let after = stripped.trim();
            let mut it = after.splitn(2, ' ');
            let name = it.next().unwrap_or("").trim();
            if name.is_empty() || name.starts_with("--") {
                return Err(usage.to_string());
            }
            profile = Some(name.to_string());
            rest = it.next().unwrap_or("").trim();
            continue;
        }
        break;
    }

    if rest.is_empty() {
        return Err(usage.to_string());
    }
    if rest.starts_with("--") {
        return Err(usage.to_string());
    }

    Ok(AgentSpawnOptions {
        output_mode,
        profile,
        background,
        task: rest.to_string(),
    })
}

fn parse_agent_view_filters(raw: &str) -> Result<(Option<String>, Option<String>), String> {
    let usage = "Usage: /agent view [foreground|background|all] [--profile <name>] [--skill <name>] | /agent front [--profile <name>] [--skill <name>] | /agent back [--profile <name>] [--skill <name>]";
    let mut profile: Option<String> = None;
    let mut skill: Option<String> = None;
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let mut i = 1usize;
    while i < tokens.len() {
        let token = tokens[i];
        if token.eq_ignore_ascii_case("--profile") {
            let value = tokens.get(i + 1).ok_or_else(|| usage.to_string())?;
            let v = value.trim();
            if v.is_empty() || v.starts_with("--") {
                return Err(usage.to_string());
            }
            profile = Some(v.to_string());
            i += 2;
            continue;
        }
        if token.eq_ignore_ascii_case("--skill") {
            let value = tokens.get(i + 1).ok_or_else(|| usage.to_string())?;
            let v = value.trim();
            if v.is_empty() || v.starts_with("--") {
                return Err(usage.to_string());
            }
            skill = Some(v.to_string());
            i += 2;
            continue;
        }
        return Err(usage.to_string());
    }
    Ok((profile, skill))
}

fn agent_json_response(command: &str, agent: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "schema_version": AGENT_JSON_SCHEMA_VERSION,
        "command": command,
        "agent": agent
    })
}

fn agent_jsonl_response(command: &str, agent: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "schema_version": AGENT_JSON_SCHEMA_VERSION,
        "event": format!("agent.{}", command),
        "data": {
            "command": command,
            "agent": agent,
        }
    })
}

fn print_agent_machine_output(mode: AgentOutputMode, command: &str, agent: serde_json::Value) {
    match mode {
        AgentOutputMode::Json => println!("{}", agent_json_response(command, agent)),
        AgentOutputMode::Jsonl => println!("{}", agent_jsonl_response(command, agent)),
        AgentOutputMode::Text => {}
    }
}

fn build_agent_spawn_payload(
    id: &str,
    provider: &str,
    model: &str,
    profile_name: Option<&str>,
    default_skills: &[String],
    background: bool,
    task: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "status": "running",
        "provider": provider,
        "model": model,
        "profile": profile_name.unwrap_or(""),
        "default_skills": default_skills,
        "run_mode": if background { "background" } else { "foreground" },
        "task": task,
    })
}

fn build_agent_list_payload(
    rows: &[SubagentState],
    scope: AgentViewScope,
    profile: Option<&str>,
    skill: Option<&str>,
) -> serde_json::Value {
    let now_ms = now_timestamp_ms();
    let running_count = rows.iter().filter(|r| r.status == "running").count();
    let completed_count = rows.iter().filter(|r| r.status == "completed").count();
    let failed_count = rows.iter().filter(|r| r.status == "failed").count();
    let closed_count = rows.iter().filter(|r| r.status == "closed").count();
    let unknown_count = rows
        .len()
        .saturating_sub(running_count + completed_count + failed_count + closed_count);
    let foreground_count = rows.iter().filter(|r| !r.task.background).count();
    let background_count = rows.len().saturating_sub(foreground_count);
    let mut age_ms_min: Option<u128> = None;
    let mut age_ms_max: Option<u128> = None;
    let mut completed_duration_ms_min: Option<u128> = None;
    let mut completed_duration_ms_max: Option<u128> = None;
    for row in rows {
        let age_ms = now_ms.saturating_sub(row.task.started_at_ms);
        age_ms_min = Some(age_ms_min.map_or(age_ms, |v| v.min(age_ms)));
        age_ms_max = Some(age_ms_max.map_or(age_ms, |v| v.max(age_ms)));
        if let Some(finished_at_ms) = row.finished_at_ms {
            let duration_ms = finished_at_ms.saturating_sub(row.task.started_at_ms);
            completed_duration_ms_min = Some(
                completed_duration_ms_min.map_or(duration_ms, |v| v.min(duration_ms)),
            );
            completed_duration_ms_max = Some(
                completed_duration_ms_max.map_or(duration_ms, |v| v.max(duration_ms)),
            );
        }
    }
    let items = rows
        .iter()
        .map(|row| {
            let age_ms = now_ms.saturating_sub(row.task.started_at_ms);
            let duration_ms = row
                .finished_at_ms
                .map(|finished_at_ms| finished_at_ms.saturating_sub(row.task.started_at_ms));
            serde_json::json!({
                "id": row.task.id,
                "status": row.status,
                "provider": row.task.provider,
                "model": row.task.model,
                "allow_rules": row.task.allow_rules,
                "deny_rules": row.task.deny_rules,
                "rules_source": row.task.rules_source,
                "profile": row.task.profile_name,
                "default_skills": row.task.default_skills,
                "run_mode": if row.task.background { "background" } else { "foreground" },
                "turns": row.submitted_turns,
                "interrupted_count": row.interrupted_count,
                "last_interrupted_at_ms": row.last_interrupted_at_ms,
                "started_at_ms": row.task.started_at_ms,
                "finished_at_ms": row.finished_at_ms,
                "age_ms": age_ms,
                "duration_ms": duration_ms,
                "task": row.task.task,
                "preview": row.output_preview,
            })
        })
        .collect::<Vec<_>>();
    let scope_value = scope.as_str();
    let profile_value = profile.unwrap_or("");
    let skill_value = skill.unwrap_or("");
    serde_json::json!({
        "count": rows.len(),
        "filters_applied": {
            "scope": scope_value,
            "profile": profile_value,
            "skill": skill_value,
        },
        "diagnostics": {
            "filters": {
                "scope": scope_value,
                "profile": profile_value,
                "skill": skill_value,
            },
            "total_items": rows.len(),
            "counts": {
                "running": running_count,
                "completed": completed_count,
                "failed": failed_count,
                "closed": closed_count,
                "unknown": unknown_count,
            },
            "run_modes": {
                "foreground": foreground_count,
                "background": background_count,
            },
            "timings": {
                "now_ms": now_ms,
                "age_ms_min": age_ms_min,
                "age_ms_max": age_ms_max,
                "completed_duration_ms_min": completed_duration_ms_min,
                "completed_duration_ms_max": completed_duration_ms_max,
            },
        },
        "items": items,
    })
}

fn build_agent_status_payload(state: Option<&SubagentState>, id: &str) -> serde_json::Value {
    if let Some(state) = state {
        let allow_rule_count = state.task.allow_rules.len();
        let deny_rule_count = state.task.deny_rules.len();
        let now_ms = now_timestamp_ms();
        let age_ms = now_ms.saturating_sub(state.task.started_at_ms);
        let duration_ms = state
            .finished_at_ms
            .map(|finished_at_ms| finished_at_ms.saturating_sub(state.task.started_at_ms));
        serde_json::json!({
            "id": state.task.id,
            "status": state.status,
            "provider": state.task.provider,
            "model": state.task.model,
            "allow_rules": state.task.allow_rules,
            "deny_rules": state.task.deny_rules,
            "rules_source": state.task.rules_source,
            "diagnostics": {
                "rules_source": state.task.rules_source,
                "allow_rule_count": allow_rule_count,
                "deny_rule_count": deny_rule_count,
            },
            "profile": state.task.profile_name,
            "default_skills": state.task.default_skills,
            "run_mode": if state.task.background { "background" } else { "foreground" },
            "turns": state.submitted_turns,
            "interrupted_count": state.interrupted_count,
            "last_interrupted_at_ms": state.last_interrupted_at_ms,
            "started_at_ms": state.task.started_at_ms,
            "finished_at_ms": state.finished_at_ms,
            "age_ms": age_ms,
            "duration_ms": duration_ms,
            "task": state.task.task,
            "preview": state.output_preview,
        })
    } else {
        serde_json::json!({
            "id": id,
            "status": "unknown",
            "diagnostics": {
                "rules_source": "unknown",
                "allow_rule_count": 0,
                "deny_rule_count": 0,
            },
        })
    }
}

fn build_agent_send_payload(
    state: Option<&SubagentState>,
    id: &str,
    no_context: bool,
    interrupt: bool,
    message: &str,
) -> serde_json::Value {
    if let Some(state) = state {
        serde_json::json!({
            "id": state.task.id,
            "status": state.status,
            "provider": state.task.provider,
            "model": state.task.model,
            "allow_rules": state.task.allow_rules,
            "deny_rules": state.task.deny_rules,
            "rules_source": state.task.rules_source,
            "profile": state.task.profile_name,
            "default_skills": state.task.default_skills,
            "run_mode": if state.task.background { "background" } else { "foreground" },
            "turns": state.submitted_turns,
            "interrupted_count": state.interrupted_count,
            "last_interrupted_at_ms": state.last_interrupted_at_ms,
            "started_at_ms": state.task.started_at_ms,
            "finished_at_ms": state.finished_at_ms,
            "task": state.task.task,
            "context": if no_context { "reset" } else { "append" },
            "interrupt": interrupt,
            "message": message,
        })
    } else {
        serde_json::json!({
            "id": id,
            "status": "unknown",
            "context": if no_context { "reset" } else { "append" },
            "interrupt": interrupt,
            "message": message,
        })
    }
}

fn build_agent_wait_done_payload(done: &SubagentOutcome) -> serde_json::Value {
    let status = if done.result.is_ok() { "done" } else { "error" };
    let body = done.result.clone().unwrap_or_else(|e| e);
    serde_json::json!({
        "id": done.task.id,
        "provider": done.task.provider,
        "model": done.task.model,
        "allow_rules": done.task.allow_rules,
        "deny_rules": done.task.deny_rules,
        "rules_source": done.task.rules_source,
        "profile": done.task.profile_name,
        "default_skills": done.task.default_skills,
        "run_mode": if done.task.background { "background" } else { "foreground" },
        "task": done.task.task,
        "started_at_ms": done.task.started_at_ms,
        "finished_at_ms": done.finished_at_ms,
        "status": status,
        "ok": done.result.is_ok(),
        "result": body,
    })
}

fn build_agent_wait_timeout_payload(timeout_secs: u64) -> serde_json::Value {
    serde_json::json!({
        "status": "timeout",
        "timeout_secs": timeout_secs,
        "message": format!("no subagent finished within {}s", timeout_secs),
    })
}

fn build_agent_wait_idle_payload() -> serde_json::Value {
    serde_json::json!({
        "status": "idle",
        "message": "no running subagents",
    })
}

fn build_agent_close_payload(id: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "status": "closed",
        "message": message,
    })
}

fn build_agent_cancel_payload(id: &str, state: Option<&SubagentState>, message: &str) -> serde_json::Value {
    let status = state
        .map(|s| s.status.clone())
        .unwrap_or_else(|| "unknown".to_string());
    serde_json::json!({
        "id": id,
        "status": status,
        "message": message,
    })
}

fn build_agent_retry_payload(id: &str, state: Option<&SubagentState>, message: &str) -> serde_json::Value {
    if let Some(s) = state {
        serde_json::json!({
            "id": id,
            "status": s.status,
            "provider": s.task.provider,
            "model": s.task.model,
            "turns": s.submitted_turns,
            "run_mode": if s.task.background { "background" } else { "foreground" },
            "task": s.task.task,
            "message": message,
        })
    } else {
        serde_json::json!({
            "id": id,
            "status": "unknown",
            "message": message,
        })
    }
}

fn build_agent_log_payload(
    state: &SubagentState,
    events: &[SubagentEvent],
    total_events: usize,
    tail: Option<usize>,
) -> serde_json::Value {
    let items = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "at_ms": e.at_ms,
                "event": e.event,
                "message": e.message,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "id": state.task.id,
        "status": state.status,
        "provider": state.task.provider,
        "model": state.task.model,
        "run_mode": if state.task.background { "background" } else { "foreground" },
        "turns": state.submitted_turns,
        "events_total": total_events,
        "tail": tail.unwrap_or(20),
        "items": items,
    })
}

fn agent_usage_includes_jsonl(raw: &str) -> String {
    raw.replace("[--json]", "[--json|--jsonl]")
}

fn subagent_usage() -> String {
    agent_usage_includes_jsonl(
        "Usage: /agent spawn [--json] [--profile <name>] [--background|--foreground] <task> | /agent send [--json] [--interrupt] [--no-context] <id> <task> | /agent wait [--json] [id] [timeout_secs] | /agent list [--json] [--scope <foreground|background|all>] [--profile <name>] [--skill <name>] | /agent status [--json] <id> | /agent log [--json] <id> [--tail <n>] | /agent retry [--json] <id> | /agent cancel [--json] <id> | /agent close [--json] <id> | /agent view [foreground|background|all] [--profile <name>] [--skill <name>] | /agent front [--profile <name>] [--skill <name>] | /agent back [--profile <name>] [--skill <name>] (legacy: /agent <task>)"
    )
}

fn format_subagent_log_rows(
    state: &SubagentState,
    events: &[SubagentEvent],
    total_events: usize,
) -> String {
    let mut rows = Vec::new();
    rows.push(format!(
        "subagent log id={} status={} provider={} model={} total_events={} shown={}",
        state.task.id,
        state.status,
        state.task.provider,
        state.task.model,
        total_events,
        events.len()
    ));
    if events.is_empty() {
        rows.push("- <empty>".to_string());
    } else {
        for row in events {
            rows.push(format!(
                "- at_ms={} event={} message={}",
                row.at_ms,
                row.event,
                clip_chars(&row.message, 180)
            ));
        }
    }
    rows.join("\n")
}

fn filter_subagent_rows(
    rows: Vec<SubagentState>,
    scope: AgentViewScope,
    profile: Option<&str>,
    skill: Option<&str>,
) -> Vec<SubagentState> {
    let profile_norm = profile
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty());
    let skill_norm = skill
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty());

    rows.into_iter()
        .filter(|row| match scope {
            AgentViewScope::All => true,
            AgentViewScope::Foreground => !row.task.background,
            AgentViewScope::Background => row.task.background,
        })
        .filter(|row| {
            if let Some(p) = profile_norm.as_deref() {
                row.task
                    .profile_name
                    .as_deref()
                    .map(|v| v.trim().eq_ignore_ascii_case(p))
                    .unwrap_or(false)
            } else {
                true
            }
        })
        .filter(|row| {
            if let Some(s) = skill_norm.as_deref() {
                row.task
                    .default_skills
                    .iter()
                    .any(|x| x.trim().eq_ignore_ascii_case(s))
            } else {
                true
            }
        })
        .collect::<Vec<_>>()
}

fn format_subagent_list(rows: &[SubagentState]) -> String {
    if rows.is_empty() {
        return "subagents: none".to_string();
    }
    let mut out = vec![format!("subagents: count={}", rows.len())];
    for row in rows {
        let finished = row
            .finished_at_ms
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string());
        let preview = row
            .output_preview
            .as_deref()
            .map(|s| clip_chars(s, 100))
            .unwrap_or_else(|| "-".to_string());
        let last_interrupted = row
            .last_interrupted_at_ms
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string());
        out.push(format!(
            "- id={} status={} run_mode={} provider={} model={} turns={} interrupted_count={} last_interrupted_at_ms={} started_at_ms={} finished_at_ms={} task={} preview={}",
            row.task.id,
            row.status,
            if row.task.background {
                "background"
            } else {
                "foreground"
            },
            row.task.provider,
            row.task.model,
            row.submitted_turns,
            row.interrupted_count,
            last_interrupted,
            row.task.started_at_ms,
            finished,
            clip_chars(&row.task.task, 120),
            preview
        ));
    }
    out.join("\n")
}

fn format_subagent_list_with_filters(
    rows: &[SubagentState],
    scope: AgentViewScope,
    profile: Option<&str>,
    skill: Option<&str>,
) -> String {
    let profile_label = profile
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "-".to_string());
    let skill_label = skill
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "-".to_string());

    if rows.is_empty() {
        return format!(
            "subagents: none scope={} profile={} skill={}",
            scope.as_str(),
            profile_label,
            skill_label
        );
    }
    let running_count = rows.iter().filter(|r| r.status == "running").count();
    let completed_count = rows.iter().filter(|r| r.status == "completed").count();
    let failed_count = rows.iter().filter(|r| r.status == "failed").count();
    let closed_count = rows.iter().filter(|r| r.status == "closed").count();
    let unknown_count = rows
        .len()
        .saturating_sub(running_count + completed_count + failed_count + closed_count);
    let foreground_count = rows.iter().filter(|r| !r.task.background).count();
    let background_count = rows.len().saturating_sub(foreground_count);
    let now_ms = now_timestamp_ms();
    let max_age_ms = rows
        .iter()
        .map(|row| now_ms.saturating_sub(row.task.started_at_ms))
        .max()
        .unwrap_or(0);
    let max_finished_duration_ms = rows
        .iter()
        .filter_map(|row| {
            row.finished_at_ms
                .map(|finished_at_ms| finished_at_ms.saturating_sub(row.task.started_at_ms))
        })
        .max();
    let max_finished_duration_label = max_finished_duration_ms
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string());

    let mut out = format_subagent_list(rows)
        .lines()
        .map(|v| v.to_string())
        .collect::<Vec<_>>();
    if let Some(first) = out.first_mut() {
        *first = format!(
            "{} scope={} profile={} skill={}",
            first,
            scope.as_str(),
            profile_label,
            skill_label
        );
    }
    out.insert(
        1,
        format!(
            "summary running={} completed={} failed={} closed={} unknown={} foreground={} background={} max_age_ms={} max_finished_duration_ms={}",
            running_count,
            completed_count,
            failed_count,
            closed_count,
            unknown_count,
            foreground_count,
            background_count,
            max_age_ms,
            max_finished_duration_label
        ),
    );
    out.join("\n")
}

pub(crate) fn should_enable_auto_tooling_for_turn(
    input: &str,
    auto_work_mode_enabled: bool,
) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.starts_with("/work ") || trimmed.starts_with("/code ") || trimmed.starts_with("/secure ")
    {
        return true;
    }
    if parse_review_task_json_only(trimmed).is_some() {
        return true;
    }

    if trimmed.starts_with('/') {
        return false;
    }

    auto_work_mode_enabled || is_coding_intent(trimmed)
}

fn is_coding_intent(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }

    let compact_len = trimmed.chars().filter(|c| !c.is_whitespace()).count();
    if compact_len <= 20 && contains_cjk_script(trimmed) {
        let tiny_non_task = [
            "你好", "您好", "你在吗", "在吗", "嗨", "哈喽", "hello", "hi", "hey", "thanks", "谢谢",
            "你能做什么", "你会什么",
        ];
        if tiny_non_task
            .iter()
            .any(|p| trimmed.eq_ignore_ascii_case(p) || trimmed.contains(p))
        {
            return false;
        }
    }
    if compact_len >= 6 && contains_cjk_script(trimmed) {
        return true;
    }

    let s = trimmed.to_lowercase();
    if matches!(
        s.as_str(),
        "hi"
            | "hello"
            | "hey"
            | "thanks"
            | "thank you"
            | "ok"
            | "okay"
    ) {
        return false;
    }

    let hints = [
        "code",
        "coding",
        "implement",
        "fix",
        "bug",
        "refactor",
        "write",
        "create",
        "modify",
        "project",
        "build",
        "compile",
        "test",
        "security",
        "vulnerability",
        "python",
        "rust",
        "javascript",
        "typescript",
        "java",
        "go",
        "cargo ",
        "npm ",
        "pip ",
        "error",
        "exception",
        "stack trace",
        "source code",
        "bugfix",
        "exploit",
        "rework",
        "feature",
        "build error",
        "unit test",
        "repository",
        "file",
        "runtime",
        "failure",
        "codebase",
        "workspace",
        "directory",
        "folder",
        "filepath",
        "analyze",
        "inspect",
        "verify",
        "validate",
        "execute",
        "debug",
        "failure",
        "issue",
        "fixup",
        "enhancement",
        "implementation",
        "revision",
        "restructure",
        "build",
        "dependency",
    ];
    if hints.iter().any(|k| s.contains(k)) {
        return true;
    }

    let file_hints = [
        ".rs", ".py", ".js", ".ts", ".tsx", ".jsx", ".java", ".go", ".cpp", ".c", ".cs", ".toml",
        ".json", ".yaml", ".yml", ".md",
    ];
    file_hints.iter().any(|ext| s.contains(ext))
}

fn contains_cjk_script(input: &str) -> bool {
    input.chars().any(|ch| {
        let cp = ch as u32;
        matches!(
            cp,
            0x3400..=0x4DBF
                | 0x4E00..=0x9FFF
                | 0xF900..=0xFAFF
                | 0x3040..=0x30FF
                | 0xAC00..=0xD7AF
        )
    })
}

fn extract_changed_file(input_line: &str, result: &runtime::TurnResult) -> Option<String> {
    let rest = input_line.strip_prefix("/toolcall ")?;
    let mut parts = rest.splitn(2, ' ');
    let tool = parts.next().unwrap_or("");
    let args = parts.next().unwrap_or("");

    if !result.is_tool_result || result.stop_reason != "tool_result" {
        return None;
    }

    if !matches!(tool, "write_file" | "edit_file") {
        return None;
    }

    let (name, status, _) = parse_tool_payload(&result.text)?;
    if name != tool || status != "ok" {
        return None;
    }

    parse_file_path_arg(args)
}

fn parse_file_path_arg(args: &str) -> Option<String> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(path) =
        tools::extract_key_value_string_arg(trimmed, &["path", "file_path"])
    {
        return normalize_file_path_token(&path);
    }

    if let Some(idx) = trimmed.find("<<<") {
        return normalize_file_path_token(&trimmed[..idx]);
    }
    if let Some(idx) = trimmed.find("<<") {
        return normalize_file_path_token(&trimmed[..idx]);
    }

    if let Some(first) = trimmed.chars().next() {
        if first == '"' || first == '\'' {
            let mut escaped = false;
            for (idx, ch) in trimmed.char_indices().skip(1) {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                    continue;
                }
                if ch == first {
                    return Some(trimmed[1..idx].to_string());
                }
            }

            return normalize_file_path_token(trimmed);
        }
    }

    let token = trimmed.split_whitespace().next().unwrap_or("");
    normalize_file_path_token(token)
}

fn normalize_file_path_token(token: &str) -> Option<String> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return None;
    }

    let path = if trimmed.len() >= 2 {
        let b = trimmed.as_bytes();
        let quoted = (b[0] == b'"' && b[trimmed.len() - 1] == b'"')
            || (b[0] == b'\'' && b[trimmed.len() - 1] == b'\'');
        if quoted {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    let path = path.trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}
fn push_unique_changed_file(changed_files: &mut Vec<String>, path: &str) {
    if !changed_files.iter().any(|p| p == path) {
        changed_files.push(path.to_string());
    }
}

#[derive(Debug, Clone)]
struct AutoValidationResult {
    command: String,
    ok: bool,
    output: String,
}

fn collect_changed_paths_since_events(change_events: &[String], from_index: usize) -> Vec<String> {
    let mut out = Vec::new();
    if from_index >= change_events.len() {
        return out;
    }

    for ev in &change_events[from_index..] {
        let (_, path) = parse_change_event(ev);
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        push_unique_changed_file(&mut out, path);
    }

    out
}

fn shell_quote_for_bash_arg(raw: &str) -> String {
    if cfg!(target_os = "windows") {
        // PowerShell single-quote escaping: ' becomes ''
        format!("'{}'", raw.replace('\'', "''"))
    } else {
        // POSIX shell single-quote escaping
        format!("'{}'", raw.replace('\'', "'\"'\"'"))
    }
}

fn build_auto_validation_commands(changed_files: &[String]) -> Vec<String> {
    let mut commands = Vec::new();
    if changed_files.is_empty() {
        return commands;
    }

    let mut rust_related = false;
    let mut py_files: Vec<String> = Vec::new();

    for path in changed_files {
        let lower = path.trim().to_ascii_lowercase();
        if lower.is_empty() {
            continue;
        }

        if lower.ends_with(".rs") || lower.ends_with("cargo.toml") || lower.ends_with("cargo.lock")
        {
            rust_related = true;
        }

        if lower.ends_with(".py") {
            push_unique_changed_file(&mut py_files, path.trim());
        }
    }

    if rust_related && Path::new("Cargo.toml").exists() {
        commands.push("cargo check --offline".to_string());
    }

    for py in py_files.into_iter().take(20) {
        commands.push(format!(
            "python -m py_compile {}",
            shell_quote_for_bash_arg(&py)
        ));
    }

    commands
}

fn run_auto_validation_guards(changed_files: &[String]) -> Vec<AutoValidationResult> {
    let commands = build_auto_validation_commands(changed_files);
    let mut results = Vec::new();

    for command in commands {
        let run = tools::run_tool("bash", &command);
        let mut output = run.output.trim().to_string();
        if output.is_empty() {
            output = if run.ok {
                "ok".to_string()
            } else {
                "failed (no output)".to_string()
            };
        }

        results.push(AutoValidationResult {
            command,
            ok: run.ok,
            output: clip_chars(&output, 2400),
        });
    }

    results
}

fn format_auto_validation_summary(results: &[AutoValidationResult]) -> String {
    let passed = results.iter().filter(|x| x.ok).count();
    let total = results.len();
    let failed = total.saturating_sub(passed);
    format!(
        "auto_validation={} passed={} failed={}",
        total, passed, failed
    )
}
fn parse_change_event(ev: &str) -> (&str, &str) {
    match ev.split_once(':') {
        Some((src, path)) => (src, path),
        None => ("unknown", ev),
    }
}

fn handle_changes_command(
    rest: &str,
    changed_files: &mut Vec<String>,
    change_events: &mut Vec<String>,
) -> Result<String, String> {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return Ok(format_changed_files(changed_files, change_events));
    }

    if trimmed == "clear" {
        changed_files.clear();
        change_events.clear();
        return Ok("changed_files cleared".to_string());
    }

    if trimmed == "tail" || trimmed.starts_with("tail ") {
        let n = trimmed
            .strip_prefix("tail")
            .unwrap_or("")
            .trim()
            .parse::<usize>()
            .ok()
            .unwrap_or(10)
            .clamp(1, 200);

        if change_events.is_empty() {
            return Ok("change_events=none".to_string());
        }

        let start = change_events.len().saturating_sub(n);
        let mut lines = vec![format!(
            "change_events_tail={}",
            change_events.len() - start
        )];
        for ev in &change_events[start..] {
            lines.push(format!("- {}", ev));
        }
        return Ok(lines.join("\n"));
    }

    if let Some(pattern) = trimmed.strip_prefix("file ") {
        let p = pattern.trim();
        if p.is_empty() {
            return Err("Usage: /changes file <pattern>".to_string());
        }
        let mut lines = vec![];
        for ev in change_events.iter() {
            let (_src, path) = parse_change_event(ev);
            if path.contains(p) {
                lines.push(format!("- {}", ev));
            }
        }
        if lines.is_empty() {
            return Ok(format!("change_events file={} none", p));
        }
        lines.insert(0, format!("change_events file={} count={}", p, lines.len()));
        return Ok(lines.join("\n"));
    }

    if let Some(args) = trimmed.strip_prefix("export ") {
        let mut parts = args.split_whitespace();
        let out_path = parts
            .next()
            .ok_or_else(|| "Usage: /changes export <path> [md|json] [n]".to_string())?;
        let mut fmt = "md".to_string();
        let mut limit = 200usize;

        if let Some(v) = parts.next() {
            if v == "md" || v == "json" {
                fmt = v.to_string();
            } else if let Ok(n) = v.parse::<usize>() {
                limit = n.clamp(1, 2000);
            }
        }
        if let Some(v) = parts.next() {
            if let Ok(n) = v.parse::<usize>() {
                limit = n.clamp(1, 2000);
            }
        }

        let total = change_events.len();
        let start = total.saturating_sub(limit);
        let selected = &change_events[start..];

        let payload = if fmt == "json" {
            serde_json::to_string_pretty(&serde_json::json!({
                "changed_files": changed_files,
                "change_events": selected,
                "total_events": total,
                "exported_events": selected.len()
            }))
            .map_err(|e| e.to_string())?
        } else {
            let mut out = String::new();
            out.push_str("# ASI Code Change Report\n\n");
            out.push_str(&format!("changed_files={}\\n", changed_files.len()));
            for f in changed_files.iter() {
                out.push_str(&format!("- {}\\n", f));
            }
            out.push_str("\n## Change Events\n");
            out.push_str(&format!("total={} exported={}\\n", total, selected.len()));
            for ev in selected.iter() {
                out.push_str(&format!("- {}\\n", ev));
            }
            out
        };

        fs::write(out_path, payload).map_err(|e| e.to_string())?;
        return Ok(format!(
            "changes exported: path={} format={} events={}",
            out_path,
            fmt,
            selected.len()
        ));
    }

    Ok("Usage: /changes [clear|tail [n]|file <pattern>|export <path> [md|json] [n]]".to_string())
}
fn push_change_event(change_events: &mut Vec<String>, source: &str, path: &str) {
    change_events.push(format!("{}:{}", source, path));
}

fn format_changed_files(changed_files: &[String], change_events: &[String]) -> String {
    if changed_files.is_empty() {
        return "changed_files=none".to_string();
    }

    let mut lines = vec![format!("changed_files={} (session)", changed_files.len())];
    for p in changed_files {
        lines.push(format!("- {}", p));
    }

    if !change_events.is_empty() {
        lines.push(String::new());
        lines.push(format!("change_events={} (session)", change_events.len()));
        for ev in change_events {
            lines.push(format!("- {}", ev));
        }
    }

    lines.join("\n")
}

fn session_matches_filters(
    session: &session::Session,
    require_agent_enabled: bool,
    blocked_only: bool,
) -> bool {
    if !require_agent_enabled && !blocked_only {
        return true;
    }
    let Some(meta) = &session.meta else {
        return false;
    };
    if require_agent_enabled && !meta.agent_enabled {
        return false;
    }
    if blocked_only && meta.confidence_gate.blocked_risky_toolcalls == 0 {
        return false;
    }
    true
}
fn is_interactive_approval_mode(mode: &str) -> bool {
    matches!(
        mode.trim().to_ascii_lowercase().as_str(),
        "ask" | "on-request" | "on_request" | "interactive"
    )
}
fn is_recoverable_bash_tool_error(text: &str) -> bool {
    if !text.starts_with("[tool:bash:error]") {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    lower.contains("commandnotfoundexception")
        || lower.contains("is not recognized as the name of a cmdlet")
        || lower.contains("is not recognized as an internal or external command")
        || lower.contains(": not found")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolFailureCategory {
    ConstraintBlock,
    Permission,
    CommandMissing,
    ShellSyntax,
    Network,
    Timeout,
    Unknown,
}

#[derive(Debug, Clone)]
struct FailureMemoryEntry {
    tool: String,
    category: ToolFailureCategory,
    summary: String,
    hint: &'static str,
    next_action: &'static str,
}

#[derive(Debug, Clone)]
pub(crate) struct FileSynopsisEntry {
    path: String,
    line_range: String,
    total_lines: usize,
    summary: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FileSynopsisCache {
    entries: HashMap<String, FileSynopsisEntry>,
    order: VecDeque<String>,
    hits: usize,
    misses: usize,
    inserts: usize,
}

const FILE_SYNOPSIS_CACHE_MAX_ENTRIES: usize = 12;

impl FileSynopsisCache {
    fn upsert(&mut self, path: String, entry: FileSynopsisEntry) {
        if self.entries.contains_key(&path) {
            self.order.retain(|k| k != &path);
        } else {
            self.inserts += 1;
        }
        self.order.push_back(path.clone());
        self.entries.insert(path, entry);
        while self.order.len() > FILE_SYNOPSIS_CACHE_MAX_ENTRIES {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }

    fn try_get(&mut self, path: &str) -> Option<&FileSynopsisEntry> {
        if self.entries.contains_key(path) {
            self.hits += 1;
            self.order.retain(|k| k != path);
            self.order.push_back(path.to_string());
            return self.entries.get(path);
        }
        self.misses += 1;
        None
    }

    fn render_recent_hints(&self, limit: usize) -> String {
        if self.order.is_empty() {
            return "none".to_string();
        }
        let mut lines = Vec::new();
        for key in self.order.iter().rev().take(limit) {
            if let Some(entry) = self.entries.get(key) {
                lines.push(format!(
                    "- path={} range={} total_lines={} summary={}",
                    entry.path, entry.line_range, entry.total_lines, entry.summary
                ));
            }
        }
        if lines.is_empty() {
            "none".to_string()
        } else {
            lines.join("\n")
        }
    }

    fn stats_line(&self) -> String {
        format!(
            "file_synopsis_cache entries={} hits={} misses={} inserts={}",
            self.entries.len(),
            self.hits,
            self.misses,
            self.inserts
        )
    }
}

fn parse_read_file_header(header: &str) -> Option<(String, String, usize)> {
    let trimmed = header.trim();
    if !trimmed.starts_with("[read_file:") || !trimmed.ends_with(']') {
        return None;
    }
    let inner = trimmed
        .trim_start_matches("[read_file:")
        .trim_end_matches(']');
    let marker = " lines ";
    let idx = inner.rfind(marker)?;
    let path = inner[..idx].trim().to_string();
    let rest = inner[idx + marker.len()..].trim();
    let of_marker = " of ";
    let of_idx = rest.rfind(of_marker)?;
    let range = rest[..of_idx].trim().to_string();
    let total_lines = rest[of_idx + of_marker.len()..].trim().parse::<usize>().ok()?;
    Some((path, range, total_lines))
}

fn summarize_read_file_body(body: &str) -> String {
    let mut out = Vec::new();
    let mut non_empty = 0usize;
    for line in body.lines() {
        let l = line.trim();
        if l.is_empty() {
            continue;
        }
        if l.starts_with('[')
            || l.starts_with("```")
            || l.starts_with("(read content hidden in UI")
            || l.starts_with("(truncated")
        {
            continue;
        }
        non_empty += 1;
        if l.starts_with("fn ")
            || l.starts_with("class ")
            || l.starts_with("def ")
            || l.starts_with("pub ")
            || l.starts_with("struct ")
            || l.starts_with("impl ")
            || l.starts_with("const ")
            || l.starts_with("use ")
        {
            out.push(clip_chars(l, 80));
        }
        if out.len() >= 3 {
            break;
        }
    }
    if out.is_empty() {
        return format!("non_empty_lines={}", non_empty);
    }
    format!("{}; non_empty_lines={}", out.join(" | "), non_empty)
}

fn cache_read_file_from_tool_result(cache: &mut FileSynopsisCache, result: &runtime::TurnResult) {
    if !result.is_tool_result {
        return;
    }
    let Some((name, status, body)) = parse_tool_payload(&result.text) else {
        return;
    };
    if name != "read_file" || status != "ok" {
        return;
    }
    let mut lines = body.lines();
    let Some(header) = lines.next() else {
        return;
    };
    let Some((path, range, total_lines)) = parse_read_file_header(header) else {
        return;
    };
    let summary = summarize_read_file_body(body.trim());
    let entry = FileSynopsisEntry {
        path: path.clone(),
        line_range: range,
        total_lines,
        summary,
    };
    cache.upsert(path, entry);
}

fn cache_read_file_from_command(cache: &mut FileSynopsisCache, command: &str) {
    let Some((tool, args)) = parse_toolcall_line(command) else {
        return;
    };
    if tool != "read_file" {
        return;
    }
    let Some(path) = parse_file_path_arg(&args) else {
        return;
    };
    let _ = cache.try_get(&path);
}

fn append_file_synopsis_to_followup(
    mut followup: String,
    cache: &FileSynopsisCache,
    strict_mode: bool,
) -> String {
    if cache.entries.is_empty() {
        return followup;
    }
    let max_items = if strict_mode { 4 } else { 2 };
    followup.push_str("\n\nFile Synopsis Cache (recent read_file summaries):\n");
    followup.push_str(&cache.render_recent_hints(max_items));
    followup
}

const FAILURE_MEMORY_WINDOW: usize = 4;
const TOOL_OBSERVATION_WINDOW: usize = 8;
const PLAN_HANDSHAKE_MAX_LINES: usize = 7;

fn is_constraint_block_text(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("blocked by user constraint:")
        || lower.contains("blocked by strict safety rule:")
}

fn classify_constraint_block_detail(text: Option<&str>) -> &'static str {
    let lower = text.unwrap_or("").to_ascii_lowercase();
    if lower.contains("run-only") || lower.contains("does not allow file edits") {
        return "run-only/edit conflict";
    }
    if lower.contains("not to use uv") || lower.contains("said not to use uv") {
        return "uv disallowed by user request";
    }
    if lower.contains("git branch/history-changing") {
        return "strict git-branch safety rule";
    }
    "tool policy conflict"
}

fn constraint_block_stop_reason_if_needed(
    streak: usize,
    max_streak: usize,
    last_text: Option<&str>,
) -> Option<String> {
    if streak < max_streak {
        return None;
    }
    Some(format!(
        "repeated user-constraint blocks ({}, {} consecutive; threshold={})",
        classify_constraint_block_detail(last_text),
        streak,
        max_streak
    ))
}

fn classify_tool_failure_result(result: &runtime::TurnResult) -> ToolFailureCategory {
    classify_tool_failure_text(&result.text, &result.stop_reason)
}

fn classify_tool_failure_text(text: &str, stop_reason: &str) -> ToolFailureCategory {
    let lower = text.to_ascii_lowercase();
    let stop = stop_reason.to_ascii_lowercase();

    if is_constraint_block_text(text) {
        return ToolFailureCategory::ConstraintBlock;
    }
    if stop.contains("permission") || lower.contains("permission denied") {
        return ToolFailureCategory::Permission;
    }
    if lower.contains("not recognized as the name of a cmdlet")
        || lower.contains("not recognized as an internal or external command")
        || lower.contains("commandnotfoundexception")
        || lower.contains(": not found")
        || lower.contains("unknown tool")
    {
        return ToolFailureCategory::CommandMissing;
    }
    if lower.contains("parsererror")
        || lower.contains("invalidendofline")
        || lower.contains("missing terminator")
        || lower.contains("syntax error")
        || lower.contains("unexpected token")
    {
        return ToolFailureCategory::ShellSyntax;
    }
    if lower.contains("connecttimeouterror")
        || lower.contains("und_err_connect_timeout")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("etimedout")
    {
        return ToolFailureCategory::Timeout;
    }
    if lower.contains("dns")
        || lower.contains("name resolution")
        || lower.contains("network")
        || lower.contains("econnrefused")
        || lower.contains("econnreset")
        || lower.contains("unable to resolve")
    {
        return ToolFailureCategory::Network;
    }
    ToolFailureCategory::Unknown
}

fn strict_failure_hint(category: ToolFailureCategory) -> &'static str {
    match category {
        ToolFailureCategory::ConstraintBlock => {
            "Last tool was blocked by task constraints. Stop retrying blocked edits/commands and choose actions compatible with user limits."
        }
        ToolFailureCategory::Permission => {
            "Last tool failed due to permission policy. Use safer read-only probes first, or adjust permissions explicitly before retry."
        }
        ToolFailureCategory::CommandMissing => {
            "Last tool failed because a command or alias was missing. On Windows prefer PowerShell-native commands and .cmd executables when needed."
        }
        ToolFailureCategory::ShellSyntax => {
            "Last tool failed due to shell syntax. Rewrite command in PowerShell-compatible syntax; avoid Unix-specific separators or utilities."
        }
        ToolFailureCategory::Network => {
            "Last tool failed due to network/connectivity issue. Run local diagnostics first, then retry remote operations."
        }
        ToolFailureCategory::Timeout => {
            "Last tool timed out. Reduce scope, split command, or increase timeout via safer staged retries."
        }
        ToolFailureCategory::Unknown => {
            "Last tool failed for an unknown reason. Inspect error output directly and apply a minimal corrective command."
        }
    }
}

fn failure_next_action(category: ToolFailureCategory) -> &'static str {
    match category {
        ToolFailureCategory::ConstraintBlock => "Avoid blocked tools and choose a compliant action.",
        ToolFailureCategory::Permission => "Use a safer read probe or request permission change.",
        ToolFailureCategory::CommandMissing => "Switch to available command(s) for this OS/shell.",
        ToolFailureCategory::ShellSyntax => "Rewrite command with PowerShell-compatible syntax.",
        ToolFailureCategory::Network => "Run local diagnostics, then retry smaller remote calls.",
        ToolFailureCategory::Timeout => "Split scope or lower workload before retrying.",
        ToolFailureCategory::Unknown => "Inspect latest error text and apply minimal corrective step.",
    }
}

fn tool_result_category_for_ok(result: &runtime::TurnResult) -> &'static str {
    if !result.is_tool_result {
        return "none";
    }
    if let Some((_, status, _)) = parse_tool_payload(&result.text) {
        if status == "ok" {
            return "ok";
        }
    }
    "ok"
}

fn canonical_tool_result_line(
    command: &str,
    result: &runtime::TurnResult,
) -> Option<(String, Option<FailureMemoryEntry>)> {
    if !result.is_tool_result {
        return None;
    }
    let (tool_name, tool_args) =
        parse_toolcall_line(command).unwrap_or(("tool".to_string(), String::new()));
    if !is_tool_failure(result) {
        // Detect bash invocations whose command body looks like a file write
        // performed via PowerShell (Out-File / Set-Content / Add-Content /
        // New-Item -ItemType File / Tee-Object / [IO.File]::WriteAllText).
        // These commonly return exit=0 even when the heredoc body never
        // reached the file. We flag them so the model reconsiders and uses
        // the write_file tool instead, which actually lands content on disk.
        if tool_name == "bash" && bash_args_look_like_pseudo_file_write(&tool_args) {
            let line = format!(
                "status=ok category={} tool=bash hint=bash_pseudo_write next_action=use_write_file_tool",
                tool_result_category_for_ok(result)
            );
            let memory = FailureMemoryEntry {
                tool: tool_name.to_string(),
                category: ToolFailureCategory::Unknown,
                summary: "bash command looks like a file write via PowerShell heredoc; exit=0 does not prove the file was created".to_string(),
                hint: "bash_pseudo_write",
                next_action: "use_write_file_tool",
            };
            return Some((line, Some(memory)));
        }
        let line = format!(
            "status=ok category={} tool={} hint=- next_action=continue",
            tool_result_category_for_ok(result),
            tool_name
        );
        return Some((line, None));
    }

    let category = classify_tool_failure_result(result);
    let category_name = match category {
        ToolFailureCategory::ConstraintBlock => "constraint_block",
        ToolFailureCategory::Permission => "permission",
        ToolFailureCategory::CommandMissing => "command_missing",
        ToolFailureCategory::ShellSyntax => "shell_syntax",
        ToolFailureCategory::Network => "network",
        ToolFailureCategory::Timeout => "timeout",
        ToolFailureCategory::Unknown => "unknown",
    };
    let hint = strict_failure_hint(category);
    let next_action = failure_next_action(category);
    let summary = clip_chars(result.text.trim(), 200).replace('\n', " ");
    let line = format!(
        "status=error category={} tool={} hint={} next_action={}",
        category_name, tool_name, hint, next_action
    );
    let memory = FailureMemoryEntry {
        tool: tool_name.to_string(),
        category,
        summary,
        hint,
        next_action,
    };
    Some((line, Some(memory)))
}

/// Heuristic for detecting bash commands that try to write files via
/// PowerShell idioms (which silently no-op on heredoc/encoding edge cases).
/// We only consider exact PowerShell cmdlets and .NET file-write idioms;
/// shell here-doc redirection (`>`, `>>`, `tee`) is left alone because
/// posix-style redirection is reliable.
fn bash_args_look_like_pseudo_file_write(args: &str) -> bool {
    let lower = args.to_ascii_lowercase();
    const FILE_WRITE_PATTERNS: &[&str] = &[
        "out-file",
        "set-content",
        "add-content",
        "new-item -itemtype file",
        "new-item -type file",
        "tee-object",
        "[io.file]::writealltext",
        "[system.io.file]::writealltext",
    ];
    FILE_WRITE_PATTERNS.iter().any(|p| lower.contains(p))
}

fn push_failure_memory(
    memory: &mut Vec<FailureMemoryEntry>,
    entry: FailureMemoryEntry,
    max_len: usize,
) {
    if memory.len() >= max_len {
        let extra = memory.len() + 1 - max_len;
        memory.drain(0..extra);
    }
    memory.push(entry);
}

fn push_canonical_tool_observation(observations: &mut Vec<String>, line: String, max_len: usize) {
    if observations.len() >= max_len {
        let extra = observations.len() + 1 - max_len;
        observations.drain(0..extra);
    }
    observations.push(line);
}

pub(crate) fn format_context_contract_header(
    rt: &Runtime,
    strict_mode: bool,
    speed: ExecutionSpeed,
    constraints: ToolExecutionConstraints,
    limits: AutoLoopLimits,
    previous_loop_stop_reason: Option<&str>,
) -> String {
    let mode = if strict_mode { "strict" } else { "standard" };
    let steps = limits
        .max_steps
        .map(|v| v.to_string())
        .unwrap_or_else(|| "unlimited".to_string());
    let mut constraints_tokens = Vec::new();
    if constraints.block_mutating_tools {
        constraints_tokens.push("run-only");
    }
    if constraints.block_uv {
        constraints_tokens.push("no-uv");
    }
    if constraints.block_git_branching {
        constraints_tokens.push("no-branch-ops");
    }
    if constraints_tokens.is_empty() {
        constraints_tokens.push("none");
    }
    let previous = previous_loop_stop_reason
        .map(|s| s.to_string())
        .unwrap_or_else(|| "none".to_string());

    format!(
        "Context Contract:\nmode={} speed={} permission_mode={} tools=[read_file,write_file,edit_file,glob_search,grep_search,web_search,web_fetch,bash]\nconstraints={} budget={{steps:{},duration_secs:{},no_progress:{},constraint_blocks:{}}}\nprevious_loop_stop_reason={}",
        mode,
        speed.as_str(),
        rt.permission_mode,
        constraints_tokens.join(","),
        steps,
        limits.max_duration.as_secs(),
        limits.max_no_progress_rounds,
        limits.max_consecutive_constraint_blocks,
        previous,
    )
}

fn append_failure_memory_to_followup(
    followup_base: &str,
    strict_mode: bool,
    last_failure: Option<ToolFailureCategory>,
    failure_memory: &[FailureMemoryEntry],
    canonical_observations: &[String],
) -> String {
    let mut out = if strict_mode {
        strict_followup_prompt_with_hint(followup_base, last_failure)
    } else {
        followup_base.to_string()
    };

    if !canonical_observations.is_empty() {
        out.push_str("\n\nRecent Tool Results (canonical):");
        for line in canonical_observations {
            out.push_str("\n- ");
            out.push_str(line);
        }
    }

    if !failure_memory.is_empty() {
        out.push_str("\n\nFailure Memory Window:");
        for (idx, item) in failure_memory.iter().enumerate() {
            out.push_str(&format!(
                "\n{}. tool={} category={:?} summary={} hint={} next_action={}",
                idx + 1,
                item.tool,
                item.category,
                item.summary,
                item.hint,
                item.next_action
            ));
        }
    }
    out
}

fn is_plan_line(line: &str) -> bool {
    let s = line.trim();
    if s.is_empty() {
        return false;
    }
    if s.starts_with("/toolcall ") {
        return false;
    }
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_digit() && bytes[1] == b'.' {
        return true;
    }
    s.starts_with("- ") || s.starts_with("* ")
}

fn collect_plan_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| is_plan_line(line))
        .take(PLAN_HANDSHAKE_MAX_LINES)
        .map(|line| line.trim().to_string())
        .collect()
}

fn should_require_plan_handshake(
    strict_mode: bool,
    current_text: &str,
    toolcall_count: usize,
    steps: usize,
) -> bool {
    if !strict_mode {
        return false;
    }
    if steps > 0 {
        return false;
    }
    if toolcall_count == 0 {
        return false;
    }
    !current_text.contains("Action plan:")
}

fn build_plan_handshake_prompt(context_contract: &str) -> String {
    format!(
        "{}\n\nPlan Handshake required before tool execution. Produce ONLY a concise 3-7 line action plan (numbered or bullet list). Do not emit /toolcall lines in this response.",
        context_contract
    )
}

fn strict_followup_prompt_with_hint(base: &str, last_failure: Option<ToolFailureCategory>) -> String {
    if let Some(category) = last_failure {
        return format!(
            "{}\nFailure category: {:?}\nRecovery hint: {}",
            base,
            category,
            strict_failure_hint(category)
        );
    }
    base.to_string()
}

fn is_tool_failure(result: &runtime::TurnResult) -> bool {
    if !result.is_tool_result {
        return false;
    }
    if is_constraint_block_text(&result.text) {
        return true;
    }
    if is_recoverable_bash_tool_error(&result.text) {
        return false;
    }
    if matches!(
        result.stop_reason.as_str(),
        "permission_denied" | "unknown_tool" | "tool_error"
    ) {
        return true;
    }
    if result.text.contains("Permission denied:") || result.text.contains("Tool error:") {
        return true;
    }
    result.text.starts_with("[tool:") && result.text.contains(":error]")
}

fn compact_tool_panel_body(name: &str, status: &str, body: &str) -> String {
    if name == "read_file" && status == "ok" {
        let header = body.lines().next().unwrap_or("[read_file]").trim();
        return format!(
            "{}\n(read content hidden in UI to reduce token-heavy output; use /toolcall read_file <path> <start_line> <max_lines> for exact ranges)",
            header
        );
    }
    body.to_string()
}
fn parse_tool_payload(text: &str) -> Option<(String, String, String)> {
    if !text.starts_with("[tool:") {
        return None;
    }
    let split_idx = text.find(']')?;
    let head = &text[6..split_idx];
    let mut parts = head.splitn(2, ':');
    let name = parts.next()?.to_string();
    let status = parts.next().unwrap_or("result").to_string();
    let body = text
        .get(split_idx + 1..)
        .unwrap_or("")
        .trim_start_matches('\n')
        .to_string();
    Some((name, status, body))
}

fn estimate_tokens(text: &str) -> usize {
    (text.len() / 4).max(1)
}

fn prompt_input(label: &str) -> Result<String, String> {
    print!("{}: ", label);
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

fn prompt_secret_input(label: &str) -> Result<String, String> {
    print!("{}: ", label);
    io::stdout().flush().map_err(|e| e.to_string())?;

    if io::stdin().is_terminal() {
        let secret = rpassword::read_password().map_err(|e| e.to_string())?;
        Ok(secret.trim().to_string())
    } else {
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        Ok(line.trim().to_string())
    }
}
fn run_live_command(command: &str) -> Result<i32, String> {
    let mut child = if cfg!(target_os = "windows") {
        ProcessCommand::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?
    } else {
        ProcessCommand::new("sh")
            .arg("-lc")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let out_handle = thread::spawn(move || {
        if let Some(out) = stdout {
            let reader = BufReader::new(out);
            for line in reader.lines().map_while(Result::ok) {
                println!("{}", line);
            }
        }
    });

    let err_handle = thread::spawn(move || {
        if let Some(err) = stderr {
            let reader = BufReader::new(err);
            for line in reader.lines().map_while(Result::ok) {
                eprintln!("{}", line);
            }
        }
    });

    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = out_handle.join();
    let _ = err_handle.join();

    Ok(status.code().unwrap_or(-1))
}

fn normalize_help_topic(input: &str) -> String {
    input
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<String>>()
        .join(" ")
}

fn format_help_command_line(command: &str, description: &str) -> String {
    let width = 58usize;
    if command.len() >= width {
        format!("{} - {}", command, description)
    } else {
        format!("{:<width$} - {}", command, description, width = width)
    }
}

fn render_help_markdown(text: &str) -> String {
    let mut lines = Vec::new();
    let mut seen_title = false;
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            lines.push(String::new());
            continue;
        }
        if !seen_title {
            lines.push(format!("# {}", trimmed));
            seen_title = true;
            continue;
        }
        if trimmed == "Examples"
            || trimmed == "Usage"
            || trimmed == "Purpose"
            || trimmed == "Arguments"
            || trimmed == "Notes"
            || trimmed == "Modes"
            || trimmed == "Tools"
            || trimmed == "Profiles"
            || trimmed == "Lifecycle"
            || trimmed == "Views"
            || trimmed == "Core Commands"
            || trimmed == "Project And Workflow"
            || trimmed == "Model And Runtime"
            || trimmed == "Sessions, State, And Logs"
            || trimmed == "Safety And Permissions"
            || trimmed == "Integrations"
            || trimmed == "Subagents"
            || trimmed == "Manual Toolcalls"
            || trimmed == "Permission Modes"
            || trimmed == "Overview Modes"
            || trimmed == "Primary Topics"
            || trimmed == "Subtopics"
        {
            lines.push(format!("## {}", trimmed));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Help Topic: ") {
            lines.push(format!("## {}", rest));
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Tip: ") {
            lines.push(format!("> Tip: {}", rest));
            continue;
        }
        if trimmed.starts_with('/') {
            if let Some((cmd, desc)) = trimmed.split_once(" - ") {
                lines.push(format!("- `{}` - {}", cmd.trim(), desc.trim()));
            } else {
                lines.push(format!("- `{}`", trimmed));
            }
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("- ") {
            if rest.starts_with('/') {
                lines.push(format!("- `{}`", rest.trim()));
            } else if let Some((cmd, desc)) = rest.split_once(": ") {
                lines.push(format!("- `{}`: {}", cmd.trim(), desc.trim()));
            } else {
                lines.push(format!("- {}", rest));
            }
            continue;
        }
        lines.push(trimmed.to_string());
    }
    lines.join("\n")
}

fn help_topic_examples_from_text(text: &str, max_items: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for raw in text.lines() {
        if out.len() >= max_items {
            break;
        }
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let candidate = if let Some(rest) = trimmed.strip_prefix("Example: ") {
            if rest.starts_with('/') {
                Some(rest.trim().to_string())
            } else {
                None
            }
        } else if trimmed.starts_with('/') {
            Some(trimmed.to_string())
        } else if let Some(rest) = trimmed.strip_prefix("- ") {
            if rest.starts_with('/') {
                Some(rest.trim().to_string())
            } else {
                None
            }
        } else {
            None
        };
        if let Some(example) = candidate {
            if seen.insert(example.clone()) {
                out.push(example);
            }
        }
    }
    out
}

fn checkpoint_auto_save_hint(error: &str) -> &'static str {
    let lower = error.to_ascii_lowercase();
    if lower.contains("os error 3")
        || lower.contains("not found")
        || lower.contains("找不到")
    {
        "hint: checkpoint directory was missing; ensure project path exists and is writable."
    } else if lower.contains("os error 5")
        || lower.contains("permission")
        || lower.contains("拒绝访问")
    {
        "hint: checkpoint write permission denied; check directory ACL/locks."
    } else {
        "hint: run /checkpoint save manually for diagnostics or disable auto checkpoints via /checkpoint off."
    }
}

fn build_repl_help_text_short() -> String {
    let lines = vec![
        "ASI Code Help (Short)".to_string(),
        "".to_string(),
        "Core Commands".to_string(),
        format_help_command_line(
            "/help [short|full|topics|<topic>] [--format text|markdown|json|jsonl]",
            "Show help overview or one topic.",
        ),
        format_help_command_line("/status", "Show runtime, model, and tool status."),
        format_help_command_line("/work <task>", "Run workspace-aware coding mode."),
        format_help_command_line("/code <task>", "Run code-focused mode."),
        format_help_command_line("/secure <task>", "Run security-focused mode."),
        format_help_command_line("/review <task>", "Run bug/regression review mode."),
        format_help_command_line("/run <command>", "Execute a local shell command."),
        format_help_command_line("/project [path]", "Show or switch active project path."),
        format_help_command_line(
            "/model <name> | /provider <name>",
            "Switch active model or provider.",
        ),
        format_help_command_line(
            "/profile [standard|strict] | /speed [sprint|deep]",
            "Adjust runtime behavior and depth.",
        ),
        format_help_command_line("/permissions ...", "Manage permission modes and rules."),
        format_help_command_line(
            "/mcp ... | /plugin ... | /hooks ...",
            "Manage integrations and runtime automation.",
        ),
        format_help_command_line("/agent ...", "Manage subagent lifecycle and views."),
        format_help_command_line("/exit", "Exit the REPL."),
        "".to_string(),
        "Manual Toolcalls".to_string(),
        format_help_command_line("/help toolcall", "Overview of all manual tools."),
        format_help_command_line(
            "/help toolcall read_file",
            "Detailed guide for read_file tool.",
        ),
        format_help_command_line(
            "/help toolcall bash",
            "Detailed guide for bash tool execution.",
        ),
        "".to_string(),
        "Tip: Use /help topics for all topics, /help search <keyword> [--format markdown|json|jsonl] for structured output, or /help /mcp for focused docs.".to_string(),
    ];
    lines.join("\n")
}

fn build_repl_help_topics_text() -> String {
    let lines = vec![
        "ASI Code Help Topics".to_string(),
        "".to_string(),
        "Overview Modes".to_string(),
        format_help_command_line("/help short", "Short command index."),
        format_help_command_line("/help full", "Complete grouped command reference."),
        format_help_command_line("/help topics", "List all help topics and subtopics."),
        format_help_command_line(
        "/help search <keyword> [--format markdown|json|jsonl]",
            "Search topics by keyword.",
        ),
        "".to_string(),
        "Primary Topics".to_string(),
        format_help_command_line("/help toolcall", "Manual tool invocation reference."),
        format_help_command_line("/help permissions", "Permission modes and allow/deny rules."),
        format_help_command_line("/help agent", "Subagent lifecycle and task views."),
        format_help_command_line("/help mcp", "MCP server management and auth/config."),
        format_help_command_line("/help plugin", "Plugin management and trust/verify."),
        format_help_command_line("/help hooks", "Hook handlers and validation."),
        format_help_command_line("/help voice", "Voice mode and PTT settings."),
        format_help_command_line("/help runtime-profile", "Runtime preset profiles."),
        format_help_command_line("/help work", "Work/code/secure/review mode prompts."),
        format_help_command_line("/help model", "Model/provider/profile/speed controls."),
        format_help_command_line("/help status", "Status, sessions, and observability."),
        format_help_command_line("/help project", "Project navigation commands."),
        format_help_command_line("/help privacy", "Privacy flags and remote policy."),
        format_help_command_line("/help oauth", "Additional command groups."),
        "".to_string(),
        "Subtopics".to_string(),
        format_help_command_line("/help mcp oauth", "MCP OAuth login/status/logout."),
        format_help_command_line("/help mcp add", "Register an MCP server command."),
        format_help_command_line("/help mcp auth", "Configure MCP auth modes."),
        format_help_command_line("/help mcp config", "Set MCP config/scope/trust."),
        format_help_command_line("/help mcp start", "Start or stop MCP server process."),
        format_help_command_line("/help agent spawn", "Create subagent tasks."),
        format_help_command_line("/help agent send", "Send follow-up task to subagent."),
        format_help_command_line("/help agent wait", "Wait for subagent completion."),
        format_help_command_line("/help agent list", "List/status/log/retry/cancel/close."),
        format_help_command_line("/help agent view", "Foreground/background task views."),
        format_help_command_line("/help toolcall read_file", "Detailed read_file tool guide."),
        format_help_command_line("/help toolcall write_file", "Detailed write_file tool guide."),
        format_help_command_line("/help toolcall edit_file", "Detailed edit_file tool guide."),
        format_help_command_line("/help toolcall glob_search", "Detailed glob_search tool guide."),
        format_help_command_line("/help toolcall grep_search", "Detailed grep_search tool guide."),
        format_help_command_line("/help toolcall web_search", "Detailed web_search tool guide."),
        format_help_command_line("/help toolcall web_fetch", "Detailed web_fetch tool guide."),
        format_help_command_line("/help toolcall bash", "Detailed bash tool guide."),
        "".to_string(),
        "Examples".to_string(),
        "/help /mcp oauth".to_string(),
        "/help /agent spawn".to_string(),
        "/help toolcall read_file".to_string(),
    ];
    lines.join("\n")
}

fn repl_help_search_index() -> &'static [(&'static str, &'static str)] {
    &[
        ("short", "Concise overview of common commands"),
        ("full", "Complete grouped help with command descriptions"),
        ("topics", "Topic index for discoverability"),
        ("toolcall", "Manual tool invocation reference"),
        ("toolcall read_file", "Read a file slice safely with line bounds"),
        ("toolcall write_file", "Create or fully replace a file"),
        ("toolcall edit_file", "Patch an existing file by exact string replacement"),
        ("toolcall glob_search", "Find files by glob pattern"),
        ("toolcall grep_search", "Search text patterns across files"),
        ("toolcall web_search", "Search the web and return snippets"),
        ("toolcall web_fetch", "Fetch and parse webpage content"),
        ("toolcall bash", "Run shell commands in the project context"),
        ("permissions", "Permission modes and allow/deny rules"),
        ("agent", "Subagent lifecycle and task views"),
        ("mcp", "MCP server management and auth/config"),
        ("plugin", "Plugin install/trust/config lifecycle"),
        ("hooks", "Runtime hook handlers and validation"),
        ("voice", "Voice mode, PTT, engine, and diagnostics"),
        ("runtime-profile", "Bundled runtime presets"),
        ("work", "Workspace-aware execution mode"),
        ("code", "Code-focused execution mode"),
        ("secure", "Security-focused execution mode"),
        ("review", "Regression/bug-focused review mode"),
        ("model", "Model selection and switching"),
        ("provider", "Provider selection and switching"),
        ("profile", "Prompt profile behavior"),
        ("speed", "Execution depth/speed strategy"),
        ("status", "Runtime/provider status output"),
        ("cost", "Token and cost reporting"),
        ("sessions", "Session list and resume flows"),
        ("checkpoint", "Checkpoint lifecycle"),
        ("changes", "Changed file/event tracking"),
        ("audit", "Audit logs and exports"),
        ("project", "Project context switch and path info"),
        ("scan", "Workspace scan helpers"),
        ("run", "Direct shell command execution"),
        ("privacy", "Privacy and telemetry toggles"),
        ("flags", "Feature-flag toggles"),
        ("policy", "Remote policy synchronization"),
        ("oauth", "OAuth shortcut commands"),
        ("notebook", "Notebook-related command group"),
        ("tokenizer", "Tokenizer doctor/build/train"),
        ("autoresearch", "Autoresearch workflows"),
        ("todo", "Todo list command group"),
        ("memory", "Memory command group"),
        ("git", "Git helper command group"),
        ("mcp oauth", "MCP OAuth login/status/logout details"),
        ("mcp add", "Register a new MCP server"),
        ("mcp auth", "Configure non-OAuth MCP auth"),
        ("mcp config", "MCP config/scope/trust controls"),
        ("mcp start", "Start a configured MCP server"),
        ("mcp stop", "Stop a running MCP server"),
        ("agent spawn", "Create foreground/background subagent"),
        ("agent send", "Send follow-up task to subagent"),
        ("agent wait", "Wait for subagent completion"),
        ("agent list", "List subagents"),
        ("agent status", "Show one subagent status"),
        ("agent log", "Tail subagent log"),
        ("agent retry", "Retry subagent task"),
        ("agent cancel", "Cancel active subagent"),
        ("agent close", "Close subagent and release resources"),
        ("agent view", "Filter task views by scope/profile/skill"),
        ("agent front", "Quick foreground task view"),
        ("agent back", "Quick background task view"),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelpOutputMode {
    Text,
    Markdown,
    Json,
    Jsonl,
}

fn parse_help_output_mode(raw: &str) -> (String, HelpOutputMode) {
    let tokens = parse_cli_tokens(raw).unwrap_or_else(|_| {
        raw.split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<String>>()
    });
    if tokens.is_empty() {
        return (String::new(), HelpOutputMode::Text);
    }

    let mut mode = HelpOutputMode::Text;
    let mut kept: Vec<String> = Vec::new();
    let mut idx = 0usize;
    while idx < tokens.len() {
        let token = tokens[idx].clone();
        if token.eq_ignore_ascii_case("--json") {
            mode = HelpOutputMode::Json;
            idx += 1;
            continue;
        }
        if token.eq_ignore_ascii_case("--jsonl") {
            mode = HelpOutputMode::Jsonl;
            idx += 1;
            continue;
        }
        if token.eq_ignore_ascii_case("--format") {
            if let Some(next) = tokens.get(idx + 1) {
                if next.eq_ignore_ascii_case("json") {
                    mode = HelpOutputMode::Json;
                    idx += 2;
                    continue;
                }
                if next.eq_ignore_ascii_case("jsonl") {
                    mode = HelpOutputMode::Jsonl;
                    idx += 2;
                    continue;
                }
                if next.eq_ignore_ascii_case("text") {
                    mode = HelpOutputMode::Text;
                    idx += 2;
                    continue;
                }
                if next.eq_ignore_ascii_case("markdown") || next.eq_ignore_ascii_case("md") {
                    mode = HelpOutputMode::Markdown;
                    idx += 2;
                    continue;
                }
            }
            kept.push(token);
            idx += 1;
            continue;
        }
        kept.push(token);
        idx += 1;
    }
    (kept.join(" "), mode)
}

fn build_repl_help_search_text(query: &str) -> String {
    let normalized_query = normalize_help_topic(query);
    if normalized_query.is_empty() {
        return "Help Search\n\nUsage: /help search <keyword>\nExample: /help search mcp".to_string();
    }

    let mut lines = vec![
        format!("Help Search: {}", normalized_query),
        "".to_string(),
    ];
    let terms: Vec<&str> = normalized_query.split_whitespace().collect();
    let mut hits: Vec<(&str, &str)> = repl_help_search_index()
        .iter()
        .copied()
        .filter(|(topic, desc)| {
            let topic_l = topic.to_ascii_lowercase();
            let desc_l = desc.to_ascii_lowercase();
            terms
                .iter()
                .all(|term| topic_l.contains(term) || desc_l.contains(term))
        })
        .collect();

    hits.sort_by(|a, b| a.0.cmp(b.0));
    hits.dedup_by(|a, b| a.0 == b.0);

    if hits.is_empty() {
        lines.push("No matching help topics.".to_string());
        lines.push("Try /help topics to browse all topics.".to_string());
        return lines.join("\n");
    }

    lines.push("Matches".to_string());
    for (topic, desc) in hits {
        lines.push(format!("/help {} - {}", topic, desc));
    }
    lines.push("".to_string());
    lines.push("Tip: Use any matched /help topic directly.".to_string());
    lines.join("\n")
}

fn resolve_help_topic_for_json(topic_for_lookup: &str) -> String {
    if normalize_help_topic(topic_for_lookup).is_empty() {
        "full".to_string()
    } else {
        topic_for_lookup.trim().to_string()
    }
}

fn resolve_help_topic_for_text(topic_for_lookup: &str) -> String {
    if normalize_help_topic(topic_for_lookup).is_empty() {
        "short".to_string()
    } else {
        topic_for_lookup.trim().to_string()
    }
}

fn build_repl_help_search_json_text(query: &str) -> String {
    let normalized_query = normalize_help_topic(query);
    if normalized_query.is_empty() {
        return serde_json::json!({
            "schema_version": HELP_JSON_SCHEMA_VERSION,
            "command": "help_search",
            "help": {
                "ok": false,
                "query": "",
                "error": "missing_query",
                "usage": "/help search <keyword> [--format json|jsonl|markdown|text]",
            }
        })
        .to_string();
    }

    let terms: Vec<&str> = normalized_query.split_whitespace().collect();
    let mut hits: Vec<serde_json::Value> = repl_help_search_index()
        .iter()
        .copied()
        .filter(|(topic, desc)| {
            let topic_l = topic.to_ascii_lowercase();
            let desc_l = desc.to_ascii_lowercase();
            terms
                .iter()
                .all(|term| topic_l.contains(term) || desc_l.contains(term))
        })
        .map(|(topic, desc)| {
            serde_json::json!({
                "topic": topic,
                "description": desc,
                "command": format!("/help {}", topic),
            })
        })
        .collect();

    hits.sort_by(|a, b| {
        let ta = a.get("topic").and_then(|v| v.as_str()).unwrap_or("");
        let tb = b.get("topic").and_then(|v| v.as_str()).unwrap_or("");
        ta.cmp(tb)
    });
    hits.dedup_by(|a, b| {
        a.get("topic").and_then(|v| v.as_str()) == b.get("topic").and_then(|v| v.as_str())
    });

    let matched = !hits.is_empty();
    serde_json::json!({
        "schema_version": HELP_JSON_SCHEMA_VERSION,
        "command": "help_search",
        "help": {
            "ok": true,
            "query": normalized_query,
            "matched": matched,
            "count": hits.len(),
            "items": hits,
        }
    })
    .to_string()
}

fn build_repl_help_topic_jsonl_text(topic: &str) -> String {
    let topic_norm = normalize_help_topic(topic);
    let topic_label = if topic_norm.is_empty() {
        "full".to_string()
    } else {
        topic_norm
    };

    let payload_text = build_repl_help_topic_json_text(topic);
    let payload: serde_json::Value = serde_json::from_str(&payload_text).unwrap_or_else(|_| {
        serde_json::json!({
            "schema_version": HELP_JSON_SCHEMA_VERSION,
            "command": "help_topic",
            "help": {
                "ok": false,
                "topic": topic_label,
                "error": "json_decode_failed",
            }
        })
    });
    let command = payload
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("help_topic");
    let ok = payload
        .get("help")
        .and_then(|v| v.get("ok"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let start = serde_json::json!({
        "schema_version": HELP_JSON_SCHEMA_VERSION,
        "event": "help.start",
        "data": {
            "topic": topic_label,
            "command": command,
        }
    });
    let body = serde_json::json!({
        "schema_version": HELP_JSON_SCHEMA_VERSION,
        "event": format!("help.{}", command),
        "data": payload,
    });
    let end = serde_json::json!({
        "schema_version": HELP_JSON_SCHEMA_VERSION,
        "event": "help.end",
        "data": {
            "topic": topic_label,
            "command": command,
            "ok": ok,
        }
    });
    [start.to_string(), body.to_string(), end.to_string()].join("\n")
}

fn build_repl_help_topics_json_text() -> String {
    let mut items: Vec<serde_json::Value> = repl_help_search_index()
        .iter()
        .copied()
        .map(|(topic, desc)| {
            serde_json::json!({
                "topic": topic,
                "description": desc,
                "command": format!("/help {}", topic),
            })
        })
        .collect();
    items.sort_by(|a, b| {
        let ta = a.get("topic").and_then(|v| v.as_str()).unwrap_or("");
        let tb = b.get("topic").and_then(|v| v.as_str()).unwrap_or("");
        ta.cmp(tb)
    });
    serde_json::json!({
        "schema_version": HELP_JSON_SCHEMA_VERSION,
        "command": "help_topics",
        "help": {
            "ok": true,
            "count": items.len(),
            "items": items,
        }
    })
    .to_string()
}

fn build_repl_help_topic_json_text(topic: &str) -> String {
    let normalized = normalize_help_topic(topic);
    if normalized.is_empty() {
        return serde_json::json!({
            "schema_version": HELP_JSON_SCHEMA_VERSION,
            "command": "help_topic",
            "help": {
                "ok": false,
                "topic": "",
                "error": "missing_topic",
                "usage": "/help <topic> [--format text|markdown|json|jsonl]",
            }
        })
        .to_string();
    }

    if normalized == "short" {
        let text = build_repl_help_text_short();
        return serde_json::json!({
            "schema_version": HELP_JSON_SCHEMA_VERSION,
            "command": "help_short",
            "help": {
                "ok": true,
                "topic": "short",
                "text": text,
                "examples": help_topic_examples_from_text(&text, 8),
            }
        })
        .to_string();
    }
    if normalized == "full" {
        let text = build_repl_help_text();
        return serde_json::json!({
            "schema_version": HELP_JSON_SCHEMA_VERSION,
            "command": "help_full",
            "help": {
                "ok": true,
                "topic": "full",
                "text": text,
                "examples": help_topic_examples_from_text(&text, 12),
            }
        })
        .to_string();
    }
    if normalized == "topics" || normalized == "index" {
        return build_repl_help_topics_json_text();
    }
    if normalized == "search" {
        return build_repl_help_search_json_text("");
    }
    if let Some(query) = normalized.strip_prefix("search ") {
        return build_repl_help_search_json_text(query);
    }
    if let Some(text) = build_repl_help_topic(&normalized) {
        let examples = help_topic_examples_from_text(&text, 12);
        return serde_json::json!({
            "schema_version": HELP_JSON_SCHEMA_VERSION,
            "command": "help_topic",
            "help": {
                "ok": true,
                "topic": normalized,
                "text": text,
                "examples": examples,
            }
        })
        .to_string();
    }

    serde_json::json!({
        "schema_version": HELP_JSON_SCHEMA_VERSION,
        "command": "help_topic",
        "help": {
            "ok": false,
            "topic": normalized,
            "error": "unknown_topic",
            "hint": "Try /help topics or /help search <keyword> [--format json|jsonl|markdown|text].",
        }
    })
    .to_string()
}

fn build_repl_help_topic(topic: &str) -> Option<String> {
    let key = normalize_help_topic(topic);
    if key == "topics" || key == "index" {
        return Some(build_repl_help_topics_text());
    }
    if let Some(query) = key.strip_prefix("search ") {
        return Some(build_repl_help_search_text(query));
    }
    if key == "search" {
        return Some(build_repl_help_search_text(""));
    }
    if let Some(subtopic) = build_repl_help_subtopic(&key) {
        return Some(subtopic.to_string());
    }
    let body = match key.as_str() {
        "toolcall" | "tools" => {
            "Help Topic: /toolcall

Usage:
/toolcall <tool_name> <args>

Tools:
- read_file: Read a file slice safely without loading huge files.
  Example: /toolcall read_file \"src/main.rs\" 1 200
- write_file: Create or fully replace a file.
  Example: /toolcall write_file \"notes.txt\" \"hello\"
- edit_file: Replace exact text chunk in an existing file.
  Example: /toolcall edit_file \"a.py\" \"old\" \"new\"
- glob_search: Find files by glob pattern.
  Example: /toolcall glob_search \"**/*.rs\"
- grep_search: Search text pattern inside files.
  Example: /toolcall grep_search \"TODO\" \"src\"
- web_search: Search the web and return result snippets.
  Example: /toolcall web_search \"rust clap derive examples\"
- web_fetch: Fetch and parse webpage content.
  Example: /toolcall web_fetch \"https://example.com\"
- bash: Execute shell command in current project.
  Example: /toolcall bash \"cargo test --release\""
        }
        "permissions" => {
            "Help Topic: /permissions

Usage:
/permissions list
/permissions mode <read-only|workspace-write|on-request|ask|danger-full-access>
/permissions allow <rule> | deny <rule>
/permissions rm-allow <rule> | rm-deny <rule>
/permissions clear-rules
/permissions path-restriction on|off
/permissions dirs|add-dir <path>|rm-dir <path>|clear-dirs
/permissions temp-list|temp-allow <rule>|temp-deny <rule>|temp-rm-allow <rule>|temp-rm-deny <rule>|temp-clear
/permissions temp-dirs|temp-add-dir <path>|temp-rm-dir <path>|temp-clear-dirs
/permissions temp-next-allow <rule>|temp-next-add-dir <path>|temp-next-clear
/permissions auto-review <off|warn|block>
/permissions auto-review-threshold <critical|high|medium|low>

Modes:
- read-only: read/search tools only.
- workspace-write: file edits allowed inside workspace.
- on-request/ask: request approval for restricted actions.
- danger-full-access: full access, highest risk."
        }
        "agent" => {
            "Help Topic: /agent

Lifecycle:
/agent spawn [--json|--jsonl] [--profile <name>] [--background|--foreground] <task>
/agent send [--json|--jsonl] [--interrupt] [--no-context] <id> <task>
/agent wait [--json|--jsonl] [id] [timeout_secs]
/agent list [--json|--jsonl]
/agent status [--json|--jsonl] <id>
/agent log [--json|--jsonl] <id> [--tail <n>]
/agent retry [--json|--jsonl] <id>
/agent cancel [--json|--jsonl] <id>
/agent close [--json|--jsonl] <id>

Views:
/agent view [foreground|background|all] [--profile <name>] [--skill <name>]
/agent front [--profile <name>] [--skill <name>]
/agent back [--profile <name>] [--skill <name>]

Legacy:
/agent <task>"
        }
        "mcp" => {
            "Help Topic: /mcp

Usage:
/mcp list [--json]
/mcp add <name> <command> [--scope <session|project|global>] [--json]
/mcp rm <name> [--json]
/mcp show <name> [--json]
/mcp auth <name> <none|bearer|api-key|basic> [value] [--json]
/mcp oauth login <name> <provider> <token> [--scope <session|project|global>] [--link-auth] [--json]
/mcp oauth status <name> [provider] [--scope <session|project|global>] [--json]
/mcp oauth logout <name> [provider] [--scope <session|project|global>] [--json]
/mcp config <name> <key> <value> [--json]
/mcp config rm <name> <key> [--json]
/mcp config scope <name> <session|project|global> [--json]
/mcp config trust <name> <on|off> [--json]
/mcp export <path.json> [--json]
/mcp import <path.json> [merge|replace] [--json]
/mcp start <name> [command] [--json]
/mcp stop <name> [--json]"
        }
        "plugin" | "plugins" => {
            "Help Topic: /plugin

Usage:
/plugin list [--json]
/plugin add <name> <path> [--json]
/plugin rm <name> [--json]
/plugin show <name> [--json]
/plugin enable <name> [--json]
/plugin disable <name> [--json]
/plugin trust <name> <manual|hash> [hash] [--json]
/plugin verify <name> [--json]
/plugin config set <name> <key> <value> [--json]
/plugin config rm <name> <key> [--json]
/plugin export <path.json> [--json]
/plugin import <path.json> [merge|replace] [--json]
/plugin market [--json]"
        }
        "hooks" => {
            "Help Topic: /hooks

Usage:
/hooks status [--json]
/hooks test <event> [--tool <name>] [--args <text>] [--mode <read-only|workspace-write|on-request|danger-full-access>] [--json]
/hooks config show [--path <file.json>] [--json]
/hooks config list-handlers [--path <file.json>] [--event <event>] [--tool-prefix <tool>] [--permission-mode <mode>] [--json]
/hooks config export <path.json> [--path <source.json>] [--json]
/hooks config import <path.json> [merge|replace] [--target <file.json>] [--json]
/hooks config set <event> <script> [--path <file.json>] [--json]
/hooks config set-handler <event> <script> [--path <file.json>] [--timeout-secs <n|none>] [--json-protocol <on|off|none>] [--tool-prefix <tool|none>] [--permission-mode <...>] [--failure-policy <...>] [--json]
/hooks config edit-handler <event> <script> [--path <file.json>] [--new-script <script>] [--timeout-secs <n|none>] [--json-protocol <on|off|none>] [--tool-prefix <tool|none>] [--permission-mode <...>] [--failure-policy <...>] [--json]
/hooks config rm <event> [--path <file.json>] [--json]
/hooks config rm-handler <event> <script> [--path <file.json>] [--json]
/hooks config validate [--path <file.json>] [--strict] [--json]"
        }
        "voice" => {
            "Help Topic: /voice

Usage:
/voice [on|off|listen|timeout <sec>|engine <local|openai|auto>|openai-voice <name>|mute <on|off>|ptt <on|off>|ptt-trigger <token>|ptt-hotkey <key>|hotkey-listen|hold-listen [wait_secs]|test [text]|config [show|save|fallback-local <on|off>|local-soft-fail <on|off>|ptt <on|off>|ptt-trigger <token>|ptt-hotkey <key>]|stats]"
        }
        "runtime-profile" | "runtime" => {
            "Help Topic: /runtime-profile

Usage:
/runtime-profile [safe|fast|status]

Profiles:
- safe: on-request permissions + local sandbox.
- fast: workspace-write permissions + disabled sandbox.
- status: print currently active profile-related config."
        }
        "work" | "code" | "secure" | "review" => {
            "Help Topic: Work Modes

Usage:
/work <task>   - Workspace-aware coding instructions.
/code <task>   - Code-focused task framing.
/secure <task> - Security-focused task framing.
/review <task> - Review framing with bug/regression emphasis."
        }
        "model" | "provider" | "profile" | "speed" => {
            "Help Topic: Model/Provider Control

Usage:
/provider <name>              - Switch provider.
/model <name>                 - Switch model.
/profile [standard|strict]    - Prompt profile behavior.
/speed [sprint|deep]          - Runtime depth/speed preference."
        }
        "status" | "cost" | "sessions" | "resume" | "save" | "checkpoint" | "changes" | "audit" => {
            "Help Topic: Session And Observability

/status - Runtime/provider and safety status.
/cost - Token/cost summary.
/sessions [limit] [agent-enabled] [blocked-only] - Session list.
/resume <id> - Resume prior session.
/save - Persist current session.
/checkpoint [status|on|off|save|load|clear] - Checkpoint lifecycle.
/changes ... - Changed files and events.
/audit ... - Audit log inspection/export."
        }
        "project" | "scan" | "run" | "import" => {
            "Help Topic: Project Navigation

/project [path] - Show or switch project directory.
/import <path> - Alias to switch project directory.
/scan [patterns...] [--deep|--deep=N] - Workspace scan.
/run <command> - Execute shell command directly."
        }
        "privacy" | "flags" | "policy" => {
            "Help Topic: Safety Flags And Policy

/privacy status|telemetry on|off|tool-details on|off|undercover on|off|safe-shell on|off
/flags list|set <web_tools|bash_tool|subagent|research> on|off
/policy sync"
        }
        "oauth" | "notebook" | "tokenizer" | "autoresearch" | "todo" | "memory" | "git" => {
            "Help Topic: Additional Commands

/oauth login <token>|logout
/notebook ...
/tokenizer ...
/autoresearch ...
/todo ...
/memory ...
/git ...

Run each command without required arguments to see its detailed Usage."
        }
        _ => return None,
    };
    Some(body.to_string())
}

fn build_repl_help_subtopic(key: &str) -> Option<&'static str> {
    match key {
        "toolcall read_file" => Some(
            "Help Topic: /toolcall read_file

Purpose:
Read a bounded slice of a file without loading the whole file.

Usage:
/toolcall read_file <path> <start_line> <max_lines>

Arguments:
- path: Relative or absolute file path.
- start_line: 1-based line number to start reading.
- max_lines: Maximum number of lines to return.

Examples:
/toolcall read_file \"src/main.rs\" 1 120
/toolcall read_file \"README.md\" 40 80

Notes:
- Preferred for large files to keep context compact.
- Use with glob_search/grep_search to locate files first.",
        ),
        "toolcall write_file" => Some(
            "Help Topic: /toolcall write_file

Purpose:
Create a new file or fully replace an existing file.

Usage:
/toolcall write_file <path> <content>

Arguments:
- path: Target file path.
- content: Full file content to write.

Examples:
/toolcall write_file \"notes.txt\" \"hello\"
/toolcall write_file \"src/config.json\" \"{\\\"mode\\\":\\\"safe\\\"}\"

Notes:
- This is a full overwrite operation.
- Prefer edit_file for targeted in-place changes.",
        ),
        "toolcall edit_file" => Some(
            "Help Topic: /toolcall edit_file

Purpose:
Edit an existing file by exact text replacement.

Usage:
/toolcall edit_file <path> <old_text> <new_text>

Arguments:
- path: Target file path.
- old_text: Exact text fragment to replace.
- new_text: Replacement text.

Example:
/toolcall edit_file \"src/main.rs\" \"old_value\" \"new_value\"

Notes:
- Replacement requires exact old_text match.
- When patching large files, inspect with read_file first.",
        ),
        "toolcall glob_search" => Some(
            "Help Topic: /toolcall glob_search

Purpose:
Find files by pattern in the current project.

Usage:
/toolcall glob_search <pattern>

Examples:
/toolcall glob_search \"**/*.rs\"
/toolcall glob_search \"**/Cargo.toml\"

Notes:
- Use this to discover candidate files before reading/editing.",
        ),
        "toolcall grep_search" => Some(
            "Help Topic: /toolcall grep_search

Purpose:
Search file contents by keyword or pattern.

Usage:
/toolcall grep_search <pattern> [base_path]

Examples:
/toolcall grep_search \"TODO\" \"src\"
/toolcall grep_search \"fn build_repl_help\" \".\"

Notes:
- Use with read_file to inspect exact matches in context.",
        ),
        "toolcall web_search" => Some(
            "Help Topic: /toolcall web_search

Purpose:
Search the web for references and return result snippets.

Usage:
/toolcall web_search <query>

Example:
/toolcall web_search \"rust clap value_enum example\"

Notes:
- Best for discovery.
- Use web_fetch when you need the actual page content.",
        ),
        "toolcall web_fetch" => Some(
            "Help Topic: /toolcall web_fetch

Purpose:
Fetch and parse a target webpage.

Usage:
/toolcall web_fetch <url>

Example:
/toolcall web_fetch \"https://example.com/docs\"

Notes:
- Use after web_search to inspect a specific source URL.",
        ),
        "toolcall bash" => Some(
            "Help Topic: /toolcall bash

Purpose:
Run shell commands in the current project context.

Usage:
/toolcall bash <command>

Examples:
/toolcall bash \"cargo test --release\"
/toolcall bash \"rg -n \\\"TODO\\\" src\"

Notes:
- Command behavior follows current permission mode.
- In stricter modes, risky commands can be blocked.",
        ),
        "mcp oauth" | "mcp oauth login" | "mcp oauth status" | "mcp oauth logout" => Some(
            "Help Topic: /mcp oauth

Purpose:
Manage MCP OAuth tokens by server/provider and scope (session, project, global).

Usage:
/mcp oauth login <name> <provider> <token> [--scope <session|project|global>] [--link-auth] [--json]
/mcp oauth status <name> [provider] [--scope <session|project|global>] [--json]
/mcp oauth logout <name> [provider] [--scope <session|project|global>] [--json]

Examples:
/mcp oauth login github-mcp github ghp_xxx --scope project --link-auth
/mcp oauth status github-mcp github --scope project
/mcp oauth logout github-mcp github --scope project",
        ),
        "mcp add" => Some(
            "Help Topic: /mcp add

Purpose:
Register a new MCP server command.

Usage:
/mcp add <name> <command> [--scope <session|project|global>] [--json]

Example:
/mcp add local-fs \"npx -y @modelcontextprotocol/server-filesystem D:/test-cli\" --scope project",
        ),
        "mcp auth" => Some(
            "Help Topic: /mcp auth

Purpose:
Set non-OAuth auth mode for an MCP server.

Usage:
/mcp auth <name> <none|bearer|api-key|basic> [value] [--json]

Examples:
/mcp auth web-search bearer sk_xxx
/mcp auth data-api api-key key_abc
/mcp auth local none",
        ),
        "mcp config" | "mcp config scope" | "mcp config trust" => Some(
            "Help Topic: /mcp config

Purpose:
Manage per-server key/value config, scope, and trust policy.

Usage:
/mcp config <name> <key> <value> [--json]
/mcp config rm <name> <key> [--json]
/mcp config scope <name> <session|project|global> [--json]
/mcp config trust <name> <on|off> [--json]

Examples:
/mcp config my-mcp endpoint https://api.example.com
/mcp config scope my-mcp project
/mcp config trust my-mcp on",
        ),
        "mcp start" | "mcp stop" => Some(
            "Help Topic: /mcp start|stop

Purpose:
Start or stop a configured MCP server process.

Usage:
/mcp start <name> [command] [--json]
/mcp stop <name> [--json]

Examples:
/mcp start local-fs
/mcp stop local-fs",
        ),
        "agent spawn" => Some(
            "Help Topic: /agent spawn

Purpose:
Create a foreground/background subagent task.

Usage:
/agent spawn [--json|--jsonl] [--profile <name>] [--background|--foreground] <task>

Examples:
/agent spawn --background \"scan src and summarize risks\"
/agent spawn --profile reviewer --foreground \"review changed files for regressions\"",
        ),
        "agent send" => Some(
            "Help Topic: /agent send

Purpose:
Send follow-up input to an existing subagent.

Usage:
/agent send [--json|--jsonl] [--interrupt] [--no-context] <id> <task>

Examples:
/agent send sa-123 \"continue and focus on test coverage\"
/agent send --interrupt sa-123 \"stop current work and only fix failing tests\"",
        ),
        "agent wait" => Some(
            "Help Topic: /agent wait

Purpose:
Wait for one subagent (or current foreground one) to complete.

Usage:
/agent wait [--json|--jsonl] [id] [timeout_secs]

Examples:
/agent wait sa-123 300
/agent wait --jsonl sa-123 120",
        ),
        "agent list" | "agent status" | "agent log" | "agent retry" | "agent cancel"
        | "agent close" => Some(
            "Help Topic: /agent lifecycle

Usage:
/agent list [--json|--jsonl]
/agent status [--json|--jsonl] <id>
/agent log [--json|--jsonl] <id> [--tail <n>]
/agent retry [--json|--jsonl] <id>
/agent cancel [--json|--jsonl] <id>
/agent close [--json|--jsonl] <id>

Examples:
/agent list
/agent log sa-123 --tail 100
/agent retry sa-123",
        ),
        "agent view" | "agent front" | "agent back" => Some(
            "Help Topic: /agent views

Purpose:
Filter agent task views by foreground/background/profile/skill.

Usage:
/agent view [foreground|background|all] [--profile <name>] [--skill <name>]
/agent front [--profile <name>] [--skill <name>]
/agent back [--profile <name>] [--skill <name>]",
        ),
        _ => None,
    }
}

fn build_repl_help_text() -> String {
    let lines = vec![
        "ASI Code Help".to_string(),
        "".to_string(),
        "Core Commands".to_string(),
        format_help_command_line(
            "/help [short|full|topics|<topic>] [--format text|markdown|json|jsonl]",
            "Show help overview, index, or one topic.",
        ),
        format_help_command_line(
            "/help search <keyword> [--format text|markdown|json|jsonl]",
            "Search help topics and subtopics.",
        ),
        format_help_command_line("/status", "Show runtime, provider, and tool status."),
        format_help_command_line("/exit", "Exit the REPL."),
        format_help_command_line("/clear", "Clear conversation history."),
        format_help_command_line(
            "/compact",
            "Compact conversation while keeping a summary.",
        ),
        format_help_command_line("/cost", "Show current session token/cost summary."),
        "".to_string(),
        "Project And Workflow".to_string(),
        format_help_command_line(
            "/project [path]",
            "Show or switch current project directory.",
        ),
        format_help_command_line(
            "/scan [patterns...] [--deep|--deep=N]",
            "Scan workspace files.",
        ),
        format_help_command_line("/run <command>", "Execute a local shell command."),
        format_help_command_line("/work <task>", "Run workspace-aware coding mode."),
        format_help_command_line("/code <task>", "Run code-focused mode."),
        format_help_command_line("/secure <task>", "Run security-focused mode."),
        format_help_command_line("/review <task>", "Run bug/regression review mode."),
        format_help_command_line(
            "/workmode on|off",
            "Toggle workspace snapshot injection for coding intents.",
        ),
        "".to_string(),
        "Model And Runtime".to_string(),
        format_help_command_line("/provider <name>", "Set active provider."),
        format_help_command_line("/model <name>", "Set active model."),
        format_help_command_line(
            "/profile [standard|strict]",
            "Switch prompt profile behavior.",
        ),
        format_help_command_line(
            "/speed [sprint|deep]",
            "Control response depth/speed policy.",
        ),
        format_help_command_line(
            "/runtime-profile [safe|fast|status]",
            "Apply bundled runtime defaults.",
        ),
        format_help_command_line("/api", "Show API env/status hints."),
        format_help_command_line("/native on|off", "Toggle native tool-calling path."),
        format_help_command_line("/auto on|off", "Toggle auto-loop orchestration."),
        format_help_command_line("/think on|off", "Toggle extended thinking metadata."),
        format_help_command_line("/markdown on|off", "Toggle markdown rendering."),
        "".to_string(),
        "Sessions, State, And Logs".to_string(),
        format_help_command_line("/save", "Save current session snapshot."),
        format_help_command_line(
            "/sessions [limit] [agent-enabled] [blocked-only]",
            "List saved sessions.",
        ),
        format_help_command_line("/resume <id>", "Resume a saved session."),
        format_help_command_line(
            "/checkpoint [status|on|off|save|load|clear]",
            "Manage checkpoint lifecycle.",
        ),
        format_help_command_line(
            "/changes [clear|tail [n]|file <pattern>|export <path> [md|json] [n]]",
            "Track changed files and events.",
        ),
        format_help_command_line(
            "/audit tail|stats|tools|reasons|export|export-last",
            "Inspect and export audit logs.",
        ),
        "".to_string(),
        "Safety And Permissions".to_string(),
        format_help_command_line(
            "/permissions ...",
            "Manage allow/deny rules, temp rules, and path restrictions.",
        ),
        format_help_command_line(
            "/permissions auto-review <off|warn|block>",
            "Set automatic risky tool handling.",
        ),
        format_help_command_line(
            "/permissions auto-review-threshold <critical|high|medium|low>",
            "Set risk threshold level.",
        ),
        format_help_command_line(
            "/privacy ...",
            "Manage telemetry/tool detail/privacy flags.",
        ),
        format_help_command_line(
            "/flags ...",
            "Toggle web/bash/subagent/research feature flags.",
        ),
        format_help_command_line("/policy sync", "Sync remote policy configuration."),
        "".to_string(),
        "Integrations".to_string(),
        format_help_command_line(
            "/mcp ...",
            "Manage MCP servers, auth, config, trust, and process state.",
        ),
        format_help_command_line(
            "/plugin ...",
            "Manage local plugins, trust/verify, config, import/export.",
        ),
        format_help_command_line(
            "/hooks ...",
            "Manage runtime hook handlers and validation.",
        ),
        format_help_command_line(
            "/oauth login <token>|logout",
            "Use OAuth token shortcut commands.",
        ),
        format_help_command_line(
            "/voice ...",
            "Voice control, listen/PTT, and engine options.",
        ),
        format_help_command_line("/notebook ...", "Notebook list/add helpers."),
        format_help_command_line("/tokenizer ...", "Tokenizer doctor/build/train commands."),
        format_help_command_line(
            "/autoresearch ...",
            "Autoresearch doctor/init/run workflow.",
        ),
        format_help_command_line("/todo ...", "Manage TODO items."),
        format_help_command_line("/memory ...", "Manage memory records."),
        format_help_command_line("/git ...", "Git helper command group."),
        "".to_string(),
        "Subagents".to_string(),
        format_help_command_line(
            "/agent spawn|send|wait|list|status|log|retry|cancel|close ...",
            "Manage subagent lifecycle.",
        ),
        format_help_command_line(
            "/agent view [foreground|background|all] [--profile <name>] [--skill <name>]",
            "Filter agent task view.",
        ),
        format_help_command_line(
            "/agent front|back [--profile <name>] [--skill <name>]",
            "Quick foreground/background views.",
        ),
        "Legacy: /agent <task> still works.".to_string(),
        "".to_string(),
        "Manual Toolcalls".to_string(),
        format_help_command_line(
            "/toolcall read_file <path> <start_line> <max_lines>",
            "Read a file slice safely.",
        ),
        format_help_command_line(
            "/toolcall write_file <path> <content>",
            "Create or fully replace a file.",
        ),
        format_help_command_line(
            "/toolcall edit_file <path> <old> <new>",
            "Patch file content by exact replace.",
        ),
        format_help_command_line("/toolcall glob_search <pattern>", "Find files by glob."),
        format_help_command_line(
            "/toolcall grep_search <pattern> [base_path]",
            "Search text in files.",
        ),
        format_help_command_line(
            "/toolcall web_search <query>",
            "Search the web and return snippets.",
        ),
        format_help_command_line(
            "/toolcall web_fetch <url>",
            "Fetch and parse webpage content.",
        ),
        format_help_command_line(
            "/toolcall bash <command>",
            "Execute shell command in current project.",
        ),
        format_help_command_line(
            "/help toolcall <read_file|write_file|edit_file|glob_search|grep_search|web_search|web_fetch|bash>",
            "Show detailed guide for one tool.",
        ),
        "".to_string(),
        "Permission Modes".to_string(),
        format_help_command_line("read-only", "Read/search tools only."),
        format_help_command_line("workspace-write", "Read + edit inside workspace."),
        format_help_command_line(
            "on-request (alias: ask)",
            "Request approval for restricted actions.",
        ),
        format_help_command_line("danger-full-access", "Full access; highest risk."),
        "".to_string(),
        "Tip: For nested commands, run the command without required args to see its Usage.".to_string(),
    ];
    lines.join("\n")
}

pub(crate) fn set_project_dir(path: &str) -> Result<String, String> {
    if path.trim().is_empty() {
        return Err("project path is empty".to_string());
    }

    let target = std::path::PathBuf::from(path);
    if !target.exists() {
        return Err(format!("project path not found: {}", path));
    }
    if !target.is_dir() {
        return Err(format!("project path is not a directory: {}", path));
    }

    std::env::set_current_dir(&target).map_err(|e| e.to_string())?;
    let resolved = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok(resolved.display().to_string())
}

/// Collect git branch, status, and recent log.
fn collect_git_info(cwd: &Path) -> String {
    // Check if this is a git repo
    if !cwd.join(".git").exists() {
        return String::new();
    }

    let mut lines = Vec::new();

    // Current branch
    if let Ok(output) = ProcessCommand::new("git")
        .args(["branch", "--show-current"])
        .current_dir(cwd)
        .output()
    {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            lines.push(format!("branch: {}", branch));
        }
    }

    // Short status
    if let Ok(output) = ProcessCommand::new("git")
        .args(["status", "--short"])
        .current_dir(cwd)
        .output()
    {
        let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !status.is_empty() {
            let status_lines: Vec<&str> = status.lines().take(15).collect();
            lines.push(format!("modified files:\n{}", status_lines.join("\n")));
        } else {
            lines.push("working tree clean".to_string());
        }
    }

    // Recent commits (last 5)
    if let Ok(output) = ProcessCommand::new("git")
        .args(["log", "--oneline", "-5"])
        .current_dir(cwd)
        .output()
    {
        let log = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !log.is_empty() {
            lines.push(format!("recent commits:\n{}", log));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod prompt_and_validation_tests {
    use super::{
        apply_adaptive_budgets, estimate_task_complexity,
        parse_auto_limit_duration, parse_auto_loop_limits_from_env, parse_auto_steps_value,
        bash_uses_forbidden_git_branching, bash_uses_uv, build_auto_validation_commands,
        build_chat_no_fabrication_guard, build_manual_chat_guard,
        build_confidence_gate_prompt, cache_read_file_from_tool_result,
        confidence_low_block_reason,
        derive_tool_execution_constraints, normalize_prompt_text, parse_prompt_profile,
        parse_confidence_declaration,
        normalize_permission_mode, parse_execution_speed, resolve_execution_speed,
        parse_toolcall_line,
        prepare_prompt_agent_input, resolve_prompt_text, toolcall_is_blocked_by_user_constraints,
        is_auto_loop_continuable_stop_reason, should_enable_auto_tooling_for_turn, user_disallows_uv,
        user_requests_run_only,
        build_work_prompt,
        AutoLoopLimits, ConfidenceGateStats,
        ConfidenceLevel, ExecutionSpeed, FileSynopsisCache, PromptProfile, TaskComplexity,
    };
    use crate::config::AppConfig;
    use crate::runtime::TurnResult;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn resolve_prompt_text_accepts_positional_text() {
        let got = resolve_prompt_text(Some("hello".to_string()), false, None).unwrap();
        assert_eq!(got, "hello");
    }

    #[test]
    fn normalize_prompt_text_strips_utf8_bom() {
        let got = normalize_prompt_text("\u{feff}/toolcall bash \"echo hi\"".to_string());
        assert_eq!(got, "/toolcall bash \"echo hi\"");
    }

    #[test]
    fn resolve_prompt_text_rejects_conflicting_sources() {
        let err = resolve_prompt_text(
            Some("hello".to_string()),
            true,
            Some(PathBuf::from("prompt.txt")),
        )
        .unwrap_err();
        assert!(err.contains("conflicting prompt text sources"));
    }

    #[test]
    fn resolve_prompt_text_reads_text_file() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("asi_prompt_test_{}.txt", ts));
        fs::write(&path, "from-file").unwrap();

        let got = resolve_prompt_text(None, false, Some(path.clone())).unwrap();
        assert_eq!(got, "from-file");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn repl_help_text_is_structured_and_contains_tool_descriptions() {
        let help = super::build_repl_help_text();
        assert!(help.contains("ASI Code Help"));
        assert!(help.contains("Core Commands"));
        assert!(help.contains("Manual Toolcalls"));
        assert!(help.contains("/toolcall read_file"));
        assert!(help.contains("Read a file slice safely"));
        assert!(help.contains("/toolcall bash"));
        assert!(help.contains("Execute shell command in current project"));
        assert!(help.contains("Permission Modes"));
        assert!(help.contains("on-request (alias: ask)"));
    }

    #[test]
    fn normalize_help_topic_trims_slash_and_lowercases() {
        assert_eq!(super::normalize_help_topic("/MCP"), "mcp");
        assert_eq!(super::normalize_help_topic("  /TOOLCALL  "), "toolcall");
        assert_eq!(super::normalize_help_topic("  /MCP   OAuth "), "mcp oauth");
    }

    #[test]
    fn repl_help_short_contains_summary_and_tip() {
        let help = super::build_repl_help_text_short();
        assert!(help.contains("ASI Code Help (Short)"));
        assert!(help.contains("/help [short|full|topics|<topic>] [--format text|markdown|json|jsonl]"));
        assert!(help.contains("/agent ..."));
        assert!(help.contains("Tip: Use /help topics"));
        assert!(help.contains("/help search <keyword> [--format markdown|json|jsonl]"));
    }

    #[test]
    fn repl_help_topic_returns_mcp_and_toolcall_docs() {
        let mcp = super::build_repl_help_topic("mcp").expect("mcp help must exist");
        assert!(mcp.contains("Help Topic: /mcp"));
        assert!(mcp.contains("/mcp oauth login"));

        let tool = super::build_repl_help_topic("/toolcall").expect("toolcall help must exist");
        assert!(tool.contains("Help Topic: /toolcall"));
        assert!(tool.contains("read_file"));
        assert!(tool.contains("bash: Execute shell command"));
    }

    #[test]
    fn repl_help_topic_returns_subtopic_docs_for_mcp_oauth_and_agent_spawn() {
        let mcp_oauth = super::build_repl_help_topic("/mcp oauth").expect("mcp oauth help");
        assert!(mcp_oauth.contains("Help Topic: /mcp oauth"));
        assert!(mcp_oauth.contains("/mcp oauth login"));
        assert!(mcp_oauth.contains("scope (session, project, global)"));

        let agent_spawn =
            super::build_repl_help_topic("agent spawn").expect("agent spawn help");
        assert!(agent_spawn.contains("Help Topic: /agent spawn"));
        assert!(agent_spawn.contains("--background|--foreground"));
    }

    #[test]
    fn repl_help_topic_unknown_returns_none() {
        assert!(super::build_repl_help_topic("unknown-topic").is_none());
    }

    #[test]
    fn repl_help_topics_index_contains_primary_and_subtopics() {
        let topics = super::build_repl_help_topics_text();
        assert!(topics.contains("ASI Code Help Topics"));
        assert!(topics.contains("/help mcp"));
        assert!(topics.contains("/help mcp oauth"));
        assert!(topics.contains("/help agent spawn"));
        assert!(topics.contains("/help search <keyword>"));
    }

    #[test]
    fn repl_help_topic_topics_alias_returns_topics_index() {
        let topics = super::build_repl_help_topic("topics").expect("topics help must exist");
        assert!(topics.contains("ASI Code Help Topics"));
        let index_alias = super::build_repl_help_topic("/index").expect("index help must exist");
        assert!(index_alias.contains("/help agent send"));
    }

    #[test]
    fn repl_help_search_returns_matches_for_mcp_keyword() {
        let search = super::build_repl_help_search_text("mcp");
        assert!(search.contains("Help Search: mcp"));
        assert!(search.contains("/help mcp -"));
        assert!(search.contains("/help mcp oauth -"));
    }

    #[test]
    fn repl_help_search_handles_empty_and_no_match_queries() {
        let empty = super::build_repl_help_search_text("");
        assert!(empty.contains("Usage: /help search <keyword>"));

        let none = super::build_repl_help_search_text("zzzz-not-found");
        assert!(none.contains("No matching help topics."));
        assert!(none.contains("/help topics"));
    }

    #[test]
    fn repl_help_topic_search_alias_works() {
        let via_topic = super::build_repl_help_topic("search agent spawn")
            .expect("search topic should produce text");
        assert!(via_topic.contains("Help Search: search agent spawn") || via_topic.contains("Help Search: agent spawn"));
        assert!(via_topic.contains("/help agent spawn -"));
    }

    #[test]
    fn parse_help_output_mode_supports_json_and_jsonl() {
        let a = super::parse_help_output_mode("search mcp --json");
        assert_eq!(a.0, "search mcp");
        assert!(matches!(a.1, super::HelpOutputMode::Json));

        let b = super::parse_help_output_mode("--jsonl topics");
        assert_eq!(b.0, "topics");
        assert!(matches!(b.1, super::HelpOutputMode::Jsonl));

        let c = super::parse_help_output_mode("full");
        assert_eq!(c.0, "full");
        assert!(matches!(c.1, super::HelpOutputMode::Text));

        let d = super::parse_help_output_mode("topics --format jsonl");
        assert_eq!(d.0, "topics");
        assert!(matches!(d.1, super::HelpOutputMode::Jsonl));

        let e = super::parse_help_output_mode("--format json search mcp");
        assert_eq!(e.0, "search mcp");
        assert!(matches!(e.1, super::HelpOutputMode::Json));

        let f = super::parse_help_output_mode("--format text search mcp --jsonl");
        assert_eq!(f.0, "search mcp");
        assert!(matches!(f.1, super::HelpOutputMode::Jsonl));

        let g = super::parse_help_output_mode("topics --format markdown");
        assert_eq!(g.0, "topics");
        assert!(matches!(g.1, super::HelpOutputMode::Markdown));

        let h = super::parse_help_output_mode("--format md search mcp");
        assert_eq!(h.0, "search mcp");
        assert!(matches!(h.1, super::HelpOutputMode::Markdown));
    }

    #[test]
    fn repl_help_search_json_shape_is_machine_readable() {
        let out = super::build_repl_help_search_json_text("mcp");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some(super::HELP_JSON_SCHEMA_VERSION)
        );
        assert_eq!(parsed.get("command").and_then(|v| v.as_str()), Some("help_search"));
        let help = parsed.get("help").expect("help object");
        assert_eq!(help.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(help.get("query").and_then(|v| v.as_str()), Some("mcp"));
        assert_eq!(help.get("matched").and_then(|v| v.as_bool()), Some(true));
        assert!(
            help.get("items")
                .and_then(|v| v.as_array())
                .map(|arr| !arr.is_empty())
                .unwrap_or(false)
        );
    }

    #[test]
    fn repl_help_search_json_reports_missing_query_usage() {
        let out = super::build_repl_help_search_json_text("");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some(super::HELP_JSON_SCHEMA_VERSION)
        );
        let help = parsed.get("help").expect("help object");
        assert_eq!(help.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(
            help.get("error").and_then(|v| v.as_str()),
            Some("missing_query")
        );
        assert_eq!(
            help.get("usage").and_then(|v| v.as_str()),
            Some("/help search <keyword> [--format json|jsonl|markdown|text]")
        );
    }

    #[test]
    fn repl_help_topics_json_shape_is_machine_readable() {
        let out = super::build_repl_help_topics_json_text();
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some(super::HELP_JSON_SCHEMA_VERSION)
        );
        assert_eq!(parsed.get("command").and_then(|v| v.as_str()), Some("help_topics"));
        let help = parsed.get("help").expect("help object");
        assert_eq!(help.get("ok").and_then(|v| v.as_bool()), Some(true));
        let items = help
            .get("items")
            .and_then(|v| v.as_array())
            .expect("items array");
        assert!(!items.is_empty());
    }

    #[test]
    fn repl_help_topic_json_returns_topic_text_and_unknown_shape() {
        let known = super::build_repl_help_topic_json_text("mcp");
        let known_parsed: serde_json::Value = serde_json::from_str(&known).expect("valid json");
        assert_eq!(
            known_parsed.get("schema_version").and_then(|v| v.as_str()),
            Some(super::HELP_JSON_SCHEMA_VERSION)
        );
        let known_help = known_parsed.get("help").expect("help object");
        assert_eq!(known_help.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(known_help.get("topic").and_then(|v| v.as_str()), Some("mcp"));
        assert!(
            known_help
                .get("text")
                .and_then(|v| v.as_str())
                .map(|t| t.contains("Help Topic: /mcp"))
                .unwrap_or(false)
        );

        let unknown = super::build_repl_help_topic_json_text("not-a-topic");
        let unknown_parsed: serde_json::Value = serde_json::from_str(&unknown).expect("valid json");
        let unknown_help = unknown_parsed.get("help").expect("help object");
        assert_eq!(unknown_help.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(
            unknown_help.get("error").and_then(|v| v.as_str()),
            Some("unknown_topic")
        );
        assert_eq!(
            unknown_help.get("hint").and_then(|v| v.as_str()),
            Some("Try /help topics or /help search <keyword> [--format json|jsonl|markdown|text].")
        );
    }

    #[test]
    fn repl_help_topic_json_includes_examples_array() {
        let known = super::build_repl_help_topic_json_text("toolcall read_file");
        let known_parsed: serde_json::Value = serde_json::from_str(&known).expect("valid json");
        let help = known_parsed.get("help").expect("help object");
        let examples = help
            .get("examples")
            .and_then(|v| v.as_array())
            .expect("examples array");
        assert!(!examples.is_empty());
        assert!(examples
            .iter()
            .filter_map(|v| v.as_str())
            .any(|s| s.starts_with("/toolcall read_file")));
    }

    #[test]
    fn render_help_markdown_outputs_headings_and_bullets() {
        let md = super::render_help_markdown(&super::build_repl_help_text_short());
        assert!(md.contains("# ASI Code Help (Short)"));
        assert!(md.contains("## Core Commands"));
        assert!(md.contains("`/status`"));
    }

    #[test]
    fn checkpoint_auto_save_hint_classifies_common_errors() {
        let missing = super::checkpoint_auto_save_hint("系统找不到指定的路径。 (os error 3)");
        assert!(missing.contains("missing"));

        let denied = super::checkpoint_auto_save_hint("拒绝访问。 (os error 5)");
        assert!(denied.contains("permission"));

        let generic = super::checkpoint_auto_save_hint("unexpected write failure");
        assert!(generic.contains("/checkpoint save"));
    }

    #[test]
    fn repl_help_topic_json_supports_search_and_topics_aliases() {
        let search = super::build_repl_help_topic_json_text("search agent");
        let search_parsed: serde_json::Value = serde_json::from_str(&search).expect("valid json");
        assert_eq!(
            search_parsed.get("command").and_then(|v| v.as_str()),
            Some("help_search")
        );

        let topics = super::build_repl_help_topic_json_text("topics");
        let topics_parsed: serde_json::Value = serde_json::from_str(&topics).expect("valid json");
        assert_eq!(
            topics_parsed.get("command").and_then(|v| v.as_str()),
            Some("help_topics")
        );
    }

    #[test]
    fn resolve_help_topic_for_json_maps_empty_to_full() {
        assert_eq!(super::resolve_help_topic_for_json(""), "full");
        assert_eq!(super::resolve_help_topic_for_json("   "), "full");
        assert_eq!(super::resolve_help_topic_for_json("mcp"), "mcp");
    }

    #[test]
    fn repl_help_topic_json_supports_full_alias_for_empty_help_json() {
        let out = super::build_repl_help_topic_json_text(&super::resolve_help_topic_for_json(""));
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid json");
        assert_eq!(parsed.get("command").and_then(|v| v.as_str()), Some("help_full"));
        let help = parsed.get("help").expect("help object");
        assert_eq!(help.get("topic").and_then(|v| v.as_str()), Some("full"));
        assert_eq!(help.get("ok").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn repl_help_topic_jsonl_emits_three_events_with_payload() {
        let out = super::build_repl_help_topic_jsonl_text("mcp");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);

        let start: serde_json::Value = serde_json::from_str(lines[0]).expect("jsonl start");
        let body: serde_json::Value = serde_json::from_str(lines[1]).expect("jsonl body");
        let end: serde_json::Value = serde_json::from_str(lines[2]).expect("jsonl end");

        assert_eq!(
            start.get("event").and_then(|v| v.as_str()),
            Some("help.start")
        );
        assert_eq!(
            body.get("event").and_then(|v| v.as_str()),
            Some("help.help_topic")
        );
        assert_eq!(
            end.get("event").and_then(|v| v.as_str()),
            Some("help.end")
        );
        assert_eq!(
            end.get("data")
                .and_then(|v| v.get("ok"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn build_auto_validation_commands_includes_python_and_dedups() {
        let changed = vec![
            "a.py".to_string(),
            "a.py".to_string(),
            "nested/b.py".to_string(),
            "src/main.rs".to_string(),
        ];
        let cmds = build_auto_validation_commands(&changed);

        let py_cmds: Vec<&String> = cmds
            .iter()
            .filter(|c| c.starts_with("python -m py_compile "))
            .collect();
        assert_eq!(py_cmds.len(), 2);
        assert!(cmds.iter().any(|c| c.contains("'a.py'")));

        if Path::new("Cargo.toml").exists() {
            assert!(cmds.iter().any(|c| c == "cargo check --offline"));
        }
    }

    #[test]
    fn strict_prompt_profile_injects_strict_execution_rules() {
        let (prompt, agent, secure) = prepare_prompt_agent_input(
            "/work fix failing test",
            false,
            false,
            PromptProfile::Strict,
        )
        .unwrap();
        assert!(agent);
        assert!(!secure);
        assert!(prompt.contains("Strict execution profile"));
        assert!(prompt.contains("Do not run git branch-switching/history-rewrite commands"));
        assert!(prompt.contains("If the user says not to use a tool/package manager"));
    }

    #[test]
    fn review_prompt_mode_transforms_review_command() {
        let (prompt, agent, secure) = prepare_prompt_agent_input(
            "/review inspect parser regressions",
            false,
            false,
            PromptProfile::Standard,
        )
        .unwrap();
        assert!(agent);
        assert!(!secure);
        assert!(prompt.contains("Code review task in the current local project."));
        assert!(prompt.contains("Output sections: Findings / Missing Tests / Open Questions / Summary."));
    }

    #[test]
    fn parse_review_task_json_only_extracts_task_and_flag() {
        let parsed = super::parse_review_task_json_only("/review inspect parser --json-only")
            .expect("must parse");
        assert_eq!(parsed.0, "inspect parser");
        assert!(parsed.1);

        let parsed2 = super::parse_review_task_json_only("/review inspect parser")
            .expect("must parse");
        assert_eq!(parsed2.0, "inspect parser");
        assert!(!parsed2.1);

        assert!(super::parse_review_task_json_only("/review").is_none());
        assert!(super::parse_review_task_json_only("/review --json-only").is_none());
    }

    #[test]
    fn subagent_usage_mentions_send_interrupt() {
        let usage = super::subagent_usage();
        assert!(usage.contains("send [--json|--jsonl] [--interrupt] [--no-context] <id> <task>"));
        assert!(usage.contains("wait [--json|--jsonl] [id] [timeout_secs]"));
        assert!(usage.contains("log [--json|--jsonl] <id> [--tail <n>]"));
        assert!(usage.contains("retry [--json|--jsonl] <id>"));
        assert!(usage.contains("cancel [--json|--jsonl] <id>"));
    }

    #[test]
    fn agent_usage_includes_jsonl_rewrites_json_flags() {
        let usage = super::agent_usage_includes_jsonl("Usage: /agent list [--json]");
        assert_eq!(usage, "Usage: /agent list [--json|--jsonl]");
    }

    #[test]
    fn parse_agent_send_options_supports_flags_and_order() {
        let parsed = super::parse_agent_send_options("--interrupt --no-context sa-1 do work")
            .expect("must parse send options");
        assert!(matches!(parsed.output_mode, super::AgentOutputMode::Text));
        assert!(parsed.interrupt);
        assert!(parsed.no_context);
        assert_eq!(parsed.id, "sa-1");
        assert_eq!(parsed.task, "do work");

        let parsed2 = super::parse_agent_send_options("--no-context --interrupt sa-2 run")
            .expect("must parse send options");
        assert!(matches!(parsed2.output_mode, super::AgentOutputMode::Text));
        assert!(parsed2.interrupt);
        assert!(parsed2.no_context);
        assert_eq!(parsed2.id, "sa-2");
        assert_eq!(parsed2.task, "run");

        let parsed3 = super::parse_agent_send_options("sa-3 continue")
            .expect("must parse send options");
        assert!(matches!(parsed3.output_mode, super::AgentOutputMode::Text));
        assert!(!parsed3.interrupt);
        assert!(!parsed3.no_context);
        assert_eq!(parsed3.id, "sa-3");
        assert_eq!(parsed3.task, "continue");

        let parsed4 = super::parse_agent_send_options("--json --interrupt sa-4 rerun")
            .expect("must parse send options with --json");
        assert!(matches!(parsed4.output_mode, super::AgentOutputMode::Json));
        assert!(parsed4.interrupt);
        assert!(!parsed4.no_context);
        assert_eq!(parsed4.id, "sa-4");
        assert_eq!(parsed4.task, "rerun");

        let parsed5 = super::parse_agent_send_options("--jsonl sa-5 run")
            .expect("must parse send options with --jsonl");
        assert!(matches!(parsed5.output_mode, super::AgentOutputMode::Jsonl));
        assert!(!parsed5.interrupt);
        assert!(!parsed5.no_context);
        assert_eq!(parsed5.id, "sa-5");
        assert_eq!(parsed5.task, "run");
    }

    #[test]
    fn parse_agent_send_options_rejects_missing_args() {
        let err = super::parse_agent_send_options("--interrupt sa-1")
            .expect_err("missing task must fail");
        assert!(err.contains("Usage: /agent send"));

        let err2 = super::parse_agent_send_options("--no-context")
            .expect_err("missing id/task must fail");
        assert!(err2.contains("Usage: /agent send"));

        let err3 = super::parse_agent_send_options("--json --bad sa-1 do-work")
            .expect_err("unknown flag must fail");
        assert!(err3.contains("Usage: /agent send"));
    }

    #[test]
    fn parse_mcp_json_mode_supports_list_and_show_variants() {
        let a = super::parse_mcp_json_mode("list --json");
        assert_eq!(a.0, "list");
        assert!(a.1);

        let b = super::parse_mcp_json_mode("show my-srv --json");
        assert_eq!(b.0, "show my-srv");
        assert!(b.1);

        let c = super::parse_mcp_json_mode("show --json my-srv");
        assert_eq!(c.0, "show my-srv");
        assert!(c.1);

        let d = super::parse_mcp_json_mode("show --json");
        assert_eq!(d.0, "show");
        assert!(d.1);

        let e = super::parse_mcp_json_mode("list");
        assert_eq!(e.0, "list");
        assert!(!e.1);

        let f = super::parse_mcp_json_mode("add srv echo hi --json");
        assert_eq!(f.0, "add srv echo hi");
        assert!(f.1);

        let g = super::parse_mcp_json_mode("config rm srv key --json");
        assert_eq!(g.0, "config rm srv key");
        assert!(g.1);

        let h = super::parse_mcp_json_mode("oauth status srv deepseek --json");
        assert_eq!(h.0, "oauth status srv deepseek");
        assert!(h.1);

        let i = super::parse_mcp_json_mode("oauth logout srv --json");
        assert_eq!(i.0, "oauth logout srv");
        assert!(i.1);

        let j = super::parse_mcp_json_mode(
            "config srv source \"https://example.com/org/repo\" --json",
        );
        assert_eq!(j.0, "config srv source https://example.com/org/repo");
        assert!(j.1);
    }

    #[test]
    fn normalize_publish_meta_in_config_args_collapses_quoted_values() {
        let a = super::normalize_publish_meta_in_config_args(
            "config srv source \"https://example.com/org/repo\" --json",
        );
        assert_eq!(a, "config srv source https://example.com/org/repo --json");

        let b =
            super::normalize_publish_meta_in_config_args("config set demo signature \"sha256:abcd\"");
        assert_eq!(b, "config set demo signature sha256:abcd");

        let c = super::normalize_publish_meta_in_config_args(
            "config set demo version \"v4 pro build\" --json",
        );
        assert_eq!(c, "config set demo version v4 pro build --json");
    }

    #[test]
    fn parse_hooks_json_mode_supports_flag_positions() {
        let a = super::parse_hooks_json_mode("status --json");
        assert_eq!(a.0, "status");
        assert!(a.1);

        let b = super::parse_hooks_json_mode("--json status");
        assert_eq!(b.0, "status");
        assert!(b.1);

        let c = super::parse_hooks_json_mode("test SessionStart --tool runtime");
        assert_eq!(c.0, "test SessionStart --tool runtime");
        assert!(!c.1);
    }

    #[test]
    fn normalize_hooks_event_accepts_aliases() {
        assert_eq!(
            super::normalize_hooks_event("session_start").as_deref(),
            Some("SessionStart")
        );
        assert_eq!(
            super::normalize_hooks_event("pre-tool-use").as_deref(),
            Some("PreToolUse")
        );
        assert_eq!(
            super::normalize_hooks_event("postcompact").as_deref(),
            Some("PostCompact")
        );
        assert!(super::normalize_hooks_event("unknown-event").is_none());
    }

    #[test]
    fn hooks_cli_to_repl_args_formats_status_and_test() {
        let status = super::hooks_cli_to_repl_args(super::HooksCliCommand::Status { json: true });
        assert_eq!(status, "status --json");

        let test = super::hooks_cli_to_repl_args(super::HooksCliCommand::Test {
            event: "SessionStart".to_string(),
            tool: Some("runtime".to_string()),
            args: Some("hello world".to_string()),
            mode: Some("on-request".to_string()),
            json: true,
        });
        assert!(test.starts_with("test SessionStart --tool runtime --args \"hello world\""));
        assert!(test.contains("--mode on-request"));
        assert!(test.ends_with("--json"));
    }

    #[test]
    fn hooks_status_json_shape_is_machine_readable() {
        let out = super::handle_hooks_command("status --json").expect("hooks status json");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse hooks status");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some(super::HOOKS_JSON_SCHEMA_VERSION)
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("hooks_status")
        );
        assert!(parsed
            .get("hooks")
            .and_then(|v| v.get("enabled"))
            .and_then(|v| v.as_bool())
            .is_some());
        assert!(parsed
            .get("hooks")
            .and_then(|v| v.get("diagnostics"))
            .and_then(|v| v.as_array())
            .is_some());
    }

    #[test]
    fn hooks_test_json_shape_is_machine_readable() {
        let out = super::handle_hooks_command(
            "test SessionStart --tool runtime --args probe --mode on-request --json",
        )
        .expect("hooks test json");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse hooks test");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some(super::HOOKS_JSON_SCHEMA_VERSION)
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("hooks_test")
        );
        assert_eq!(
            parsed
                .get("hooks")
                .and_then(|v| v.get("event"))
                .and_then(|v| v.as_str()),
            Some("SessionStart")
        );
        assert_eq!(
            parsed
                .get("hooks")
                .and_then(|v| v.get("mode"))
                .and_then(|v| v.as_str()),
            Some("on-request")
        );
    }

    #[test]
    fn hooks_test_rejects_invalid_event() {
        let err = super::handle_hooks_command("test bad_event --json").expect_err("must fail");
        assert!(err.contains("invalid event"));
        assert!(err.contains("allowed events:"));
    }

    #[test]
    fn hooks_config_cli_to_repl_args_formats_commands() {
        let show = super::hooks_cli_to_repl_args(super::HooksCliCommand::Config {
            action: super::HooksCliConfigCommand::Show {
                path: Some("tmp/hooks.json".to_string()),
                json: true,
            },
        });
        assert_eq!(show, "config show --path tmp/hooks.json --json");

        let list_handlers = super::hooks_cli_to_repl_args(super::HooksCliCommand::Config {
            action: super::HooksCliConfigCommand::ListHandlers {
                path: Some("tmp/hooks.json".to_string()),
                event: Some("SessionStart".to_string()),
                tool_prefix: Some("bash".to_string()),
                permission_mode: Some("on-request".to_string()),
                json: true,
            },
        });
        assert!(list_handlers.starts_with("config list-handlers --path tmp/hooks.json"));
        assert!(list_handlers.contains("--event SessionStart"));
        assert!(list_handlers.contains("--tool-prefix bash"));
        assert!(list_handlers.contains("--permission-mode on-request"));
        assert!(list_handlers.ends_with("--json"));

        let set = super::hooks_cli_to_repl_args(super::HooksCliCommand::Config {
            action: super::HooksCliConfigCommand::Set {
                event: "SessionStart".to_string(),
                script: "python hook.py".to_string(),
                path: Some("tmp/hooks.json".to_string()),
                json: false,
            },
        });
        assert!(set.starts_with("config set SessionStart \"python hook.py\""));
        assert!(set.contains("--path tmp/hooks.json"));

        let set_handler = super::hooks_cli_to_repl_args(super::HooksCliCommand::Config {
            action: super::HooksCliConfigCommand::SetHandler {
                event: "SessionStart".to_string(),
                script: "python hook.py".to_string(),
                path: Some("tmp/hooks.json".to_string()),
                timeout_secs: Some("10".to_string()),
                json_protocol: Some("on".to_string()),
                tool_prefix: Some("bash".to_string()),
                permission_mode: Some("on-request".to_string()),
                failure_policy: Some("fail-open".to_string()),
                json: true,
            },
        });
        assert!(set_handler.contains("config set-handler SessionStart"));
        assert!(set_handler.contains("--timeout-secs 10"));
        assert!(set_handler.contains("--json-protocol on"));
        assert!(set_handler.contains("--tool-prefix bash"));
        assert!(set_handler.contains("--permission-mode on-request"));
        assert!(set_handler.contains("--failure-policy fail-open"));
        assert!(set_handler.ends_with("--json"));

        let edit_handler = super::hooks_cli_to_repl_args(super::HooksCliCommand::Config {
            action: super::HooksCliConfigCommand::EditHandler {
                event: "SessionStart".to_string(),
                script: "python hook.py".to_string(),
                path: Some("tmp/hooks.json".to_string()),
                new_script: Some("python hook2.py".to_string()),
                timeout_secs: Some("11".to_string()),
                json_protocol: Some("off".to_string()),
                tool_prefix: Some("read_file".to_string()),
                permission_mode: Some("workspace-write".to_string()),
                failure_policy: Some("fail-closed".to_string()),
                json: true,
            },
        });
        assert!(edit_handler.contains("config edit-handler SessionStart"));
        assert!(edit_handler.contains("--new-script \"python hook2.py\""));
        assert!(edit_handler.contains("--timeout-secs 11"));
        assert!(edit_handler.contains("--json-protocol off"));
        assert!(edit_handler.contains("--tool-prefix read_file"));
        assert!(edit_handler.contains("--permission-mode workspace-write"));
        assert!(edit_handler.contains("--failure-policy fail-closed"));
        assert!(edit_handler.ends_with("--json"));

        let rm = super::hooks_cli_to_repl_args(super::HooksCliCommand::Config {
            action: super::HooksCliConfigCommand::Rm {
                event: "SessionStart".to_string(),
                path: Some("tmp/hooks.json".to_string()),
                json: true,
            },
        });
        assert_eq!(rm, "config rm SessionStart --path tmp/hooks.json --json");

        let rm_handler = super::hooks_cli_to_repl_args(super::HooksCliCommand::Config {
            action: super::HooksCliConfigCommand::RmHandler {
                event: "SessionStart".to_string(),
                script: "python hook.py".to_string(),
                path: Some("tmp/hooks.json".to_string()),
                json: true,
            },
        });
        assert_eq!(
            rm_handler,
            "config rm-handler SessionStart \"python hook.py\" --path tmp/hooks.json --json"
        );

        let export = super::hooks_cli_to_repl_args(super::HooksCliCommand::Config {
            action: super::HooksCliConfigCommand::Export {
                path: "out/hooks.json".to_string(),
                source_path: Some("src/hooks.json".to_string()),
                json: true,
            },
        });
        assert_eq!(
            export,
            "config export out/hooks.json --path src/hooks.json --json"
        );

        let import = super::hooks_cli_to_repl_args(super::HooksCliCommand::Config {
            action: super::HooksCliConfigCommand::Import {
                path: "in/hooks.json".to_string(),
                mode: Some("merge".to_string()),
                target: Some("out/hooks.json".to_string()),
                json: true,
            },
        });
        assert_eq!(
            import,
            "config import in/hooks.json merge --target out/hooks.json --json"
        );

        let validate = super::hooks_cli_to_repl_args(super::HooksCliCommand::Config {
            action: super::HooksCliConfigCommand::Validate {
                path: Some("tmp/hooks.json".to_string()),
                strict: true,
                json: true,
            },
        });
        assert_eq!(validate, "config validate --path tmp/hooks.json --strict --json");
    }

    #[test]
    fn hooks_config_set_show_rm_roundtrip_json() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_hooks_cfg_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("hooks.json");
        let path_s = path.display().to_string();

        let set_out = super::handle_hooks_command(&format!(
            "config set SessionStart \"python hook.py\" --path \"{}\" --json",
            path_s
        ))
        .expect("hooks config set");
        let set_v: serde_json::Value =
            serde_json::from_str(&set_out).expect("parse hooks config set");
        assert_eq!(
            set_v.get("command").and_then(|v| v.as_str()),
            Some("hooks_config_set")
        );
        assert_eq!(
            set_v
                .get("hooks")
                .and_then(|v| v.get("event"))
                .and_then(|v| v.as_str()),
            Some("SessionStart")
        );

        let show_out = super::handle_hooks_command(&format!(
            "config show --path \"{}\" --json",
            path_s
        ))
        .expect("hooks config show");
        let show_v: serde_json::Value =
            serde_json::from_str(&show_out).expect("parse hooks config show");
        assert_eq!(
            show_v.get("command").and_then(|v| v.as_str()),
            Some("hooks_config_show")
        );
        assert_eq!(
            show_v
                .get("hooks")
                .and_then(|v| v.get("handlers_count"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );

        let rm_out = super::handle_hooks_command(&format!(
            "config rm SessionStart --path \"{}\" --json",
            path_s
        ))
        .expect("hooks config rm");
        let rm_v: serde_json::Value =
            serde_json::from_str(&rm_out).expect("parse hooks config rm");
        assert_eq!(
            rm_v.get("command").and_then(|v| v.as_str()),
            Some("hooks_config_rm")
        );
        assert_eq!(
            rm_v
                .get("hooks")
                .and_then(|v| v.get("changed"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            rm_v
                .get("hooks")
                .and_then(|v| v.get("handlers_count"))
                .and_then(|v| v.as_u64()),
            Some(0)
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hooks_config_export_import_roundtrip_json() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_hooks_export_import_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("source.json");
        let exported = dir.join("exported.json");
        let target = dir.join("target.json");

        let source_s = source.display().to_string();
        let exported_s = exported.display().to_string();
        let target_s = target.display().to_string();

        super::handle_hooks_command(&format!(
            "config set SessionStart \"python a.py\" --path \"{}\" --json",
            source_s
        ))
        .expect("seed source");

        let export_out = super::handle_hooks_command(&format!(
            "config export \"{}\" --path \"{}\" --json",
            exported_s, source_s
        ))
        .expect("export hooks");
        let export_v: serde_json::Value =
            serde_json::from_str(&export_out).expect("parse export json");
        assert_eq!(
            export_v.get("command").and_then(|v| v.as_str()),
            Some("hooks_config_export")
        );
        assert_eq!(
            export_v
                .get("hooks")
                .and_then(|v| v.get("handlers_count"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );

        let import_out = super::handle_hooks_command(&format!(
            "config import \"{}\" replace --target \"{}\" --json",
            exported_s, target_s
        ))
        .expect("import hooks");
        let import_v: serde_json::Value =
            serde_json::from_str(&import_out).expect("parse import json");
        assert_eq!(
            import_v.get("command").and_then(|v| v.as_str()),
            Some("hooks_config_import")
        );
        assert_eq!(
            import_v
                .get("hooks")
                .and_then(|v| v.get("mode"))
                .and_then(|v| v.as_str()),
            Some("replace")
        );
        assert_eq!(
            import_v
                .get("hooks")
                .and_then(|v| v.get("handlers_count"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );

        let show_target = super::handle_hooks_command(&format!(
            "config show --path \"{}\" --json",
            target_s
        ))
        .expect("show target");
        let show_v: serde_json::Value =
            serde_json::from_str(&show_target).expect("parse show target");
        assert_eq!(
            show_v
                .get("hooks")
                .and_then(|v| v.get("handlers_count"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hooks_config_import_merge_replaces_same_event_and_keeps_others() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_hooks_import_merge_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("source.json");
        let target = dir.join("target.json");
        let source_s = source.display().to_string();
        let target_s = target.display().to_string();

        super::handle_hooks_command(&format!(
            "config set SessionStart \"python new.py\" --path \"{}\" --json",
            source_s
        ))
        .expect("seed source");

        super::handle_hooks_command(&format!(
            "config set SessionStart \"python old.py\" --path \"{}\" --json",
            target_s
        ))
        .expect("seed target old");
        super::handle_hooks_command(&format!(
            "config set Stop \"python keep.py\" --path \"{}\" --json",
            target_s
        ))
        .expect("seed target keep");

        let out = super::handle_hooks_command(&format!(
            "config import \"{}\" merge --target \"{}\" --json",
            source_s, target_s
        ))
        .expect("merge import");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse import merge");
        assert_eq!(
            parsed
                .get("hooks")
                .and_then(|v| v.get("mode"))
                .and_then(|v| v.as_str()),
            Some("merge")
        );
        assert_eq!(
            parsed
                .get("hooks")
                .and_then(|v| v.get("handlers_count"))
                .and_then(|v| v.as_u64()),
            Some(2)
        );

        let show = super::handle_hooks_command(&format!(
            "config show --path \"{}\" --json",
            target_s
        ))
        .expect("show merged");
        let show_v: serde_json::Value = serde_json::from_str(&show).expect("parse show merged");
        let handlers = show_v
            .get("hooks")
            .and_then(|v| v.get("handlers"))
            .and_then(|v| v.as_array())
            .expect("handlers array");
        let mut session_script = None::<String>;
        let mut stop_script = None::<String>;
        for row in handlers {
            let event = row.get("event").and_then(|v| v.as_str()).unwrap_or("");
            let script = row
                .get("script")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if event == "SessionStart" {
                session_script = Some(script);
            } else if event == "Stop" {
                stop_script = Some(script);
            }
        }
        assert_eq!(session_script.as_deref(), Some("python new.py"));
        assert_eq!(stop_script.as_deref(), Some("python keep.py"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hooks_config_set_handler_persists_extended_fields() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_hooks_set_handler_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("hooks.json");
        let path_s = path.display().to_string();

        let out = super::handle_hooks_command(&format!(
            "config set-handler SessionStart \"python hook.py\" --path \"{}\" --timeout-secs 12 --json-protocol on --tool-prefix bash --permission-mode on-request --failure-policy fail-open --json",
            path_s
        ))
        .expect("set-handler");
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("parse set-handler json");
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("hooks_config_set_handler")
        );
        assert_eq!(
            parsed
                .get("hooks")
                .and_then(|v| v.get("timeout_secs"))
                .and_then(|v| v.as_u64()),
            Some(12)
        );
        assert_eq!(
            parsed
                .get("hooks")
                .and_then(|v| v.get("json_protocol"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            parsed
                .get("hooks")
                .and_then(|v| v.get("tool_prefix"))
                .and_then(|v| v.as_str()),
            Some("bash")
        );
        assert_eq!(
            parsed
                .get("hooks")
                .and_then(|v| v.get("permission_mode"))
                .and_then(|v| v.as_str()),
            Some("on-request")
        );
        assert_eq!(
            parsed
                .get("hooks")
                .and_then(|v| v.get("failure_policy"))
                .and_then(|v| v.as_str()),
            Some("fail-open")
        );

        let show = super::handle_hooks_command(&format!(
            "config show --path \"{}\" --json",
            path_s
        ))
        .expect("show handler cfg");
        let show_v: serde_json::Value = serde_json::from_str(&show).expect("parse show json");
        let handler = show_v
            .get("hooks")
            .and_then(|v| v.get("handlers"))
            .and_then(|v| v.as_array())
            .and_then(|v| v.first())
            .expect("one handler");
        assert_eq!(
            handler.get("timeout_secs").and_then(|v| v.as_u64()),
            Some(12)
        );
        assert_eq!(
            handler.get("json_protocol").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            handler.get("tool_prefix").and_then(|v| v.as_str()),
            Some("bash")
        );
        assert_eq!(
            handler
                .get("permission_mode")
                .and_then(|v| v.as_str()),
            Some("on-request")
        );
        assert_eq!(
            handler
                .get("failure_policy")
                .and_then(|v| v.as_str()),
            Some("fail-open")
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hooks_config_list_handlers_filters_and_rm_handler_works() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_hooks_list_rm_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("hooks.json");
        let path_s = path.display().to_string();

        super::handle_hooks_command(&format!(
            "config set-handler SessionStart \"python a.py\" --path \"{}\" --tool-prefix bash --permission-mode on-request --json",
            path_s
        ))
        .expect("set handler a");
        super::handle_hooks_command(&format!(
            "config set-handler Stop \"python b.py\" --path \"{}\" --tool-prefix read_file --permission-mode read-only --json",
            path_s
        ))
        .expect("set handler b");

        let list = super::handle_hooks_command(&format!(
            "config list-handlers --path \"{}\" --event SessionStart --tool-prefix bash --permission-mode on-request --json",
            path_s
        ))
        .expect("list filtered");
        let list_v: serde_json::Value = serde_json::from_str(&list).expect("parse list");
        assert_eq!(
            list_v.get("command").and_then(|v| v.as_str()),
            Some("hooks_config_list_handlers")
        );
        assert_eq!(
            list_v
                .get("hooks")
                .and_then(|v| v.get("count"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );

        let rmh = super::handle_hooks_command(&format!(
            "config rm-handler SessionStart \"python a.py\" --path \"{}\" --json",
            path_s
        ))
        .expect("rm-handler");
        let rmh_v: serde_json::Value = serde_json::from_str(&rmh).expect("parse rmh");
        assert_eq!(
            rmh_v.get("command").and_then(|v| v.as_str()),
            Some("hooks_config_rm_handler")
        );
        assert_eq!(
            rmh_v
                .get("hooks")
                .and_then(|v| v.get("removed"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );

        let show = super::handle_hooks_command(&format!(
            "config show --path \"{}\" --json",
            path_s
        ))
        .expect("show");
        let show_v: serde_json::Value = serde_json::from_str(&show).expect("parse show");
        assert_eq!(
            show_v
                .get("hooks")
                .and_then(|v| v.get("handlers_count"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hooks_config_edit_handler_and_validate_work() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_hooks_edit_validate_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("hooks.json");
        let path_s = path.display().to_string();

        super::handle_hooks_command(&format!(
            "config set-handler SessionStart \"python old.py\" --path \"{}\" --timeout-secs 10 --json-protocol on --tool-prefix bash --permission-mode on-request --failure-policy fail-open --json",
            path_s
        ))
        .expect("seed handler");

        let edit = super::handle_hooks_command(&format!(
            "config edit-handler SessionStart \"python old.py\" --path \"{}\" --new-script \"python new.py\" --timeout-secs 9 --json-protocol off --tool-prefix read_file --permission-mode workspace-write --failure-policy fail-closed --json",
            path_s
        ))
        .expect("edit handler");
        let edit_v: serde_json::Value = serde_json::from_str(&edit).expect("parse edit");
        assert_eq!(
            edit_v.get("command").and_then(|v| v.as_str()),
            Some("hooks_config_edit_handler")
        );
        assert_eq!(
            edit_v
                .get("hooks")
                .and_then(|v| v.get("updated_script"))
                .and_then(|v| v.as_str()),
            Some("python new.py")
        );
        assert_eq!(
            edit_v
                .get("hooks")
                .and_then(|v| v.get("timeout_secs"))
                .and_then(|v| v.as_u64()),
            Some(9)
        );
        assert_eq!(
            edit_v
                .get("hooks")
                .and_then(|v| v.get("json_protocol"))
                .and_then(|v| v.as_bool()),
            Some(false)
        );

        let validate_ok = super::handle_hooks_command(&format!(
            "config validate --path \"{}\" --json",
            path_s
        ))
        .expect("validate ok");
        let validate_ok_v: serde_json::Value =
            serde_json::from_str(&validate_ok).expect("parse validate ok");
        assert_eq!(
            validate_ok_v.get("command").and_then(|v| v.as_str()),
            Some("hooks_config_validate")
        );
        assert_eq!(
            validate_ok_v
                .get("hooks")
                .and_then(|v| v.get("valid"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            validate_ok_v
                .get("hooks")
                .and_then(|v| v.get("strict"))
                .and_then(|v| v.as_bool()),
            Some(false)
        );

        std::fs::write(
            &path,
            r#"{
  "handlers": [
    {
      "event": "SessionStart",
      "script": "python a.py",
      "timeout_secs": 0,
      "permission_mode": "invalid"
    }
  ]
}"#,
        )
        .expect("write invalid hooks config");
        let validate_bad = super::handle_hooks_command(&format!(
            "config validate --path \"{}\" --json",
            path_s
        ))
        .expect("validate bad");
        let validate_bad_v: serde_json::Value =
            serde_json::from_str(&validate_bad).expect("parse validate bad");
        assert_eq!(
            validate_bad_v
                .get("hooks")
                .and_then(|v| v.get("valid"))
                .and_then(|v| v.as_bool()),
            Some(false)
        );
        assert!(
            validate_bad_v
                .get("hooks")
                .and_then(|v| v.get("error_count"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                >= 1
        );

        std::fs::write(
            &path,
            r#"{
  "handlers": [
    {
      "event": "SessionStart",
      "script": "python new.py"
    },
    {
      "event": "SessionStart",
      "script": "python new.py"
    }
  ]
}"#,
        )
        .expect("write duplicate hooks config");
        let validate_non_strict = super::handle_hooks_command(&format!(
            "config validate --path \"{}\" --json",
            path_s
        ))
        .expect("validate non-strict duplicates");
        let validate_non_strict_v: serde_json::Value =
            serde_json::from_str(&validate_non_strict).expect("parse validate non-strict");
        assert_eq!(
            validate_non_strict_v
                .get("hooks")
                .and_then(|v| v.get("valid"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            validate_non_strict_v
                .get("hooks")
                .and_then(|v| v.get("warning_count"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );

        let validate_strict = super::handle_hooks_command(&format!(
            "config validate --path \"{}\" --strict --json",
            path_s
        ))
        .expect("validate strict duplicates");
        let validate_strict_v: serde_json::Value =
            serde_json::from_str(&validate_strict).expect("parse validate strict");
        assert_eq!(
            validate_strict_v
                .get("hooks")
                .and_then(|v| v.get("strict"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            validate_strict_v
                .get("hooks")
                .and_then(|v| v.get("valid"))
                .and_then(|v| v.as_bool()),
            Some(false)
        );

        let clear = super::handle_hooks_command(&format!(
            "config edit-handler SessionStart \"python new.py\" --path \"{}\" --timeout-secs none --json-protocol none --tool-prefix none --permission-mode none --failure-policy none --json",
            path_s
        ))
        .expect("clear optional fields");
        let clear_v: serde_json::Value = serde_json::from_str(&clear).expect("parse clear");
        assert_eq!(
            clear_v
                .get("hooks")
                .and_then(|v| v.get("timeout_secs"))
                .cloned(),
            Some(serde_json::Value::Null)
        );
        assert_eq!(
            clear_v
                .get("hooks")
                .and_then(|v| v.get("json_protocol"))
                .cloned(),
            Some(serde_json::Value::Null)
        );
        assert_eq!(
            clear_v
                .get("hooks")
                .and_then(|v| v.get("tool_prefix"))
                .cloned(),
            Some(serde_json::Value::Null)
        );
        assert_eq!(
            clear_v
                .get("hooks")
                .and_then(|v| v.get("permission_mode"))
                .cloned(),
            Some(serde_json::Value::Null)
        );
        assert_eq!(
            clear_v
                .get("hooks")
                .and_then(|v| v.get("failure_policy"))
                .cloned(),
            Some(serde_json::Value::Null)
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mcp_record_json_sorts_config_keys() {
        let mut config = std::collections::HashMap::new();
        config.insert("z".to_string(), "3".to_string());
        config.insert("a".to_string(), "1".to_string());
        let record = crate::mcp::McpServerRecord {
            name: "srv".to_string(),
            command: "echo ok".to_string(),
            pid: Some(1),
            status: "running".to_string(),
            scope: "session".to_string(),
            trusted: false,
            source: None,
            version: None,
            signature: None,
            auth_type: "none".to_string(),
            auth_value: None,
            config,
        };
        let value = super::mcp_record_json(record);
        assert_eq!(value.get("scope").and_then(|v| v.as_str()), Some("session"));
        assert_eq!(value.get("trusted").and_then(|v| v.as_bool()), Some(false));
        let keys: Vec<String> = value
            .get("config")
            .and_then(|v| v.as_array())
            .unwrap()
            .iter()
            .filter_map(|x| x.get("key").and_then(|k| k.as_str()).map(|s| s.to_string()))
            .collect();
        assert_eq!(keys, vec!["a".to_string(), "z".to_string()]);
    }

    #[test]
    fn mcp_add_parses_scope_flag_and_sets_scope_in_json() {
        let lock = crate::mcp::mcp_state_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_mcp_add_scope_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let state_path = dir.join("mcp_state.json");
        let old_state_path = std::env::var("ASI_MCP_STATE_PATH").ok();
        std::env::set_var("ASI_MCP_STATE_PATH", state_path.display().to_string());

        let out = super::handle_mcp_command("add scoped-srv echo hi --scope project --json")
            .expect("mcp add json");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp add");
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_add")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("scope"))
                .and_then(|v| v.as_str()),
            Some("project")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("server"))
                .and_then(|v| v.get("scope"))
                .and_then(|v| v.as_str()),
            Some("project")
        );

        if let Some(v) = old_state_path {
            std::env::set_var("ASI_MCP_STATE_PATH", v);
        } else {
            std::env::remove_var("ASI_MCP_STATE_PATH");
        }
        let _ = std::fs::remove_dir_all(dir);
        drop(lock);
    }

    #[test]
    fn mcp_oauth_status_json_reports_scope_details() {
        let mcp_lock = crate::mcp::mcp_state_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let oauth_lock = crate::oauth::oauth_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_mcp_oauth_scope_status_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let old_cwd = std::env::current_dir().ok();
        let old_state_path = std::env::var("ASI_MCP_STATE_PATH").ok();
        let old_oauth_path = std::env::var("ASI_OAUTH_PATH").ok();
        let old_global_dir = std::env::var("ASI_GLOBAL_CONFIG_DIR").ok();

        std::env::remove_var("ASI_MCP_STATE_PATH");
        std::env::remove_var("ASI_OAUTH_PATH");
        std::env::set_var("ASI_GLOBAL_CONFIG_DIR", dir.join(".global").display().to_string());
        std::env::set_current_dir(&dir).expect("set cwd");

        crate::mcp::add_server_with_scope("scoped-srv", "echo hi", "project").expect("add server");
        crate::oauth::save_mcp_token_scoped("scoped-srv", "deepseek", "tok-p", "project")
            .expect("save token");

        let out = super::handle_mcp_command("oauth status scoped-srv deepseek --json")
            .expect("mcp oauth status json");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp oauth status");
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_oauth_status")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("token_present"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("token_scope"))
                .and_then(|v| v.as_str()),
            Some("project")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("request_scope"))
                .and_then(|v| v.as_str()),
            Some("auto")
        );

        let _ = crate::mcp::remove_server("scoped-srv");
        if let Some(cwd) = old_cwd {
            let _ = std::env::set_current_dir(cwd);
        }
        if let Some(v) = old_state_path {
            std::env::set_var("ASI_MCP_STATE_PATH", v);
        } else {
            std::env::remove_var("ASI_MCP_STATE_PATH");
        }
        if let Some(v) = old_oauth_path {
            std::env::set_var("ASI_OAUTH_PATH", v);
        } else {
            std::env::remove_var("ASI_OAUTH_PATH");
        }
        if let Some(v) = old_global_dir {
            std::env::set_var("ASI_GLOBAL_CONFIG_DIR", v);
        } else {
            std::env::remove_var("ASI_GLOBAL_CONFIG_DIR");
        }
        let _ = std::fs::remove_dir_all(dir);
        drop(oauth_lock);
        drop(mcp_lock);
    }

    #[test]
    fn mcp_config_publish_meta_roundtrip_json() {
        let lock = crate::mcp::mcp_state_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_mcp_publish_meta_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let state_path = dir.join("mcp_state.json");
        let old_state_path = std::env::var("ASI_MCP_STATE_PATH").ok();
        std::env::set_var("ASI_MCP_STATE_PATH", state_path.display().to_string());

        super::handle_mcp_command("add meta-srv echo hi --json").expect("mcp add");
        super::handle_mcp_command(
            "config meta-srv source \"https://example.com/org/mcp\" --json",
        )
        .expect("mcp config source");
        super::handle_mcp_command("config meta-srv version \"1.2.3\" --json")
            .expect("mcp config version");
        super::handle_mcp_command("config meta-srv signature \"sha256:abcd\" --json")
            .expect("mcp config signature");

        let show = super::handle_mcp_command("show meta-srv --json").expect("mcp show");
        let show_v: serde_json::Value = serde_json::from_str(&show).expect("parse show");
        assert_eq!(
            show_v
                .get("mcp")
                .and_then(|v| v.get("source"))
                .and_then(|v| v.as_str()),
            Some("https://example.com/org/mcp")
        );
        assert_eq!(
            show_v
                .get("mcp")
                .and_then(|v| v.get("version"))
                .and_then(|v| v.as_str()),
            Some("1.2.3")
        );
        assert_eq!(
            show_v
                .get("mcp")
                .and_then(|v| v.get("signature"))
                .and_then(|v| v.as_str()),
            Some("sha256:abcd")
        );

        if let Some(v) = old_state_path {
            std::env::set_var("ASI_MCP_STATE_PATH", v);
        } else {
            std::env::remove_var("ASI_MCP_STATE_PATH");
        }
        let _ = std::fs::remove_dir_all(&dir);
        drop(lock);
    }

    #[test]
    fn mcp_config_publish_meta_rejects_invalid_source() {
        let lock = crate::mcp::mcp_state_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_mcp_publish_meta_bad_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let state_path = dir.join("mcp_state.json");
        let old_state_path = std::env::var("ASI_MCP_STATE_PATH").ok();
        std::env::set_var("ASI_MCP_STATE_PATH", state_path.display().to_string());

        super::handle_mcp_command("add meta-srv echo hi --json").expect("mcp add");
        let err = super::handle_mcp_command("config meta-srv source bad-source --json")
            .expect_err("invalid source must fail");
        assert!(err.contains("source must start with"));

        if let Some(v) = old_state_path {
            std::env::set_var("ASI_MCP_STATE_PATH", v);
        } else {
            std::env::remove_var("ASI_MCP_STATE_PATH");
        }
        let _ = std::fs::remove_dir_all(&dir);
        drop(lock);
    }

    #[test]
    fn plugin_config_publish_meta_roundtrip_json() {
        let lock = crate::plugin::plugin_state_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_plugin_publish_meta_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let state_path = dir.join("plugins_state.json");
        let old_state_path = std::env::var("ASI_PLUGIN_STATE_PATH").ok();
        std::env::set_var("ASI_PLUGIN_STATE_PATH", state_path.display().to_string());

        let plugin_dir = dir.join("demo-plugin");
        let manifest_dir = plugin_dir.join(".codex-plugin");
        std::fs::create_dir_all(&manifest_dir).expect("create manifest dir");
        std::fs::write(
            manifest_dir.join("plugin.json"),
            r#"{"name":"demo-plugin","version":"1.0.0"}"#,
        )
        .expect("write manifest");

        super::handle_plugin_command(&format!(
            "add demo {} --json",
            plugin_dir.display()
        ))
        .expect("plugin add");
        super::handle_plugin_command(
            "config set demo source \"https://example.com/org/plugin\" --json",
        )
        .expect("plugin source");
        super::handle_plugin_command("config set demo version \"1.2.3\" --json")
            .expect("plugin version");
        super::handle_plugin_command("config set demo signature \"sha256:abcd\" --json")
            .expect("plugin signature");
        let show = super::handle_plugin_command("show demo --json").expect("plugin show");
        let show_v: serde_json::Value = serde_json::from_str(&show).expect("parse show");
        assert_eq!(
            show_v
                .get("plugin")
                .and_then(|v| v.get("source"))
                .and_then(|v| v.as_str()),
            Some("https://example.com/org/plugin")
        );
        assert_eq!(
            show_v
                .get("plugin")
                .and_then(|v| v.get("version"))
                .and_then(|v| v.as_str()),
            Some("1.2.3")
        );
        assert_eq!(
            show_v
                .get("plugin")
                .and_then(|v| v.get("signature"))
                .and_then(|v| v.as_str()),
            Some("sha256:abcd")
        );

        if let Some(v) = old_state_path {
            std::env::set_var("ASI_PLUGIN_STATE_PATH", v);
        } else {
            std::env::remove_var("ASI_PLUGIN_STATE_PATH");
        }
        let _ = std::fs::remove_dir_all(&dir);
        drop(lock);
    }

    #[test]
    fn plugin_config_publish_meta_rejects_invalid_signature() {
        let lock = crate::plugin::plugin_state_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_plugin_publish_meta_bad_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let state_path = dir.join("plugins_state.json");
        let old_state_path = std::env::var("ASI_PLUGIN_STATE_PATH").ok();
        std::env::set_var("ASI_PLUGIN_STATE_PATH", state_path.display().to_string());

        let plugin_dir = dir.join("demo-plugin");
        let manifest_dir = plugin_dir.join(".codex-plugin");
        std::fs::create_dir_all(&manifest_dir).expect("create manifest dir");
        std::fs::write(
            manifest_dir.join("plugin.json"),
            r#"{"name":"demo-plugin","version":"1.0.0"}"#,
        )
        .expect("write manifest");
        super::handle_plugin_command(&format!(
            "add demo {} --json",
            plugin_dir.display()
        ))
        .expect("plugin add");

        let err = super::handle_plugin_command("config set demo signature badsig --json")
            .expect_err("invalid signature should fail");
        assert!(err.contains("signature must start with"));

        if let Some(v) = old_state_path {
            std::env::set_var("ASI_PLUGIN_STATE_PATH", v);
        } else {
            std::env::remove_var("ASI_PLUGIN_STATE_PATH");
        }
        let _ = std::fs::remove_dir_all(&dir);
        drop(lock);
    }

    #[test]
    fn mcp_list_json_shape_is_machine_readable() {
        let out = super::handle_mcp_command("list --json").expect("mcp list json");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp list json");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_list")
        );
        assert!(parsed
            .get("mcp")
            .and_then(|v| v.get("count"))
            .and_then(|v| v.as_u64())
            .is_some());
        assert!(parsed
            .get("mcp")
            .and_then(|v| v.get("items"))
            .and_then(|v| v.as_array())
            .is_some());
        assert!(parsed
            .get("mcp")
            .and_then(|v| v.get("diagnostics"))
            .and_then(|v| v.get("trusted_count"))
            .and_then(|v| v.as_u64())
            .is_some());
        assert!(parsed
            .get("mcp")
            .and_then(|v| v.get("diagnostics"))
            .and_then(|v| v.get("scope_counts"))
            .and_then(|v| v.get("session"))
            .and_then(|v| v.as_u64())
            .is_some());
    }

    #[test]
    fn mcp_show_json_includes_diagnostics_when_present() {
        let lock = crate::mcp::mcp_state_env_lock().lock().expect("mcp env lock");
        let dir = std::env::temp_dir().join("asi_mcp_show_diag_test");
        let _ = std::fs::create_dir_all(&dir);
        let state_path = dir.join("mcp_state.json");
        std::env::set_var("ASI_MCP_STATE_PATH", state_path.display().to_string());
        let _ = crate::mcp::remove_server("diag-srv");
        crate::mcp::add_server("diag-srv", "echo ok").expect("add server");
        crate::mcp::set_server_scope("diag-srv", "project").expect("set scope");
        crate::mcp::set_server_trusted("diag-srv", true).expect("set trust");
        crate::mcp::set_server_auth("diag-srv", "bearer", Some("token-x")).expect("set auth");
        crate::mcp::set_server_config("diag-srv", "oauth_provider", "deepseek")
            .expect("set provider");

        let out = super::handle_mcp_command("show diag-srv --json").expect("mcp show json");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp show json");
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_show")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("diagnostics"))
                .and_then(|v| v.get("scope"))
                .and_then(|v| v.as_str()),
            Some("project")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("diagnostics"))
                .and_then(|v| v.get("trusted"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("diagnostics"))
                .and_then(|v| v.get("has_auth_value"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("diagnostics"))
                .and_then(|v| v.get("oauth_provider"))
                .and_then(|v| v.as_str()),
            Some("deepseek")
        );
        drop(lock);
    }

    #[test]
    fn mcp_show_json_unknown_shape_is_machine_readable() {
        let out = super::handle_mcp_command("show definitely-missing-server --json")
            .expect("mcp show json");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp show json");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_show")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str()),
            Some("definitely-missing-server")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("status"))
                .and_then(|v| v.as_str()),
            Some("unknown")
        );
    }

    #[test]
    fn mcp_start_json_returns_machine_readable_error_for_untrusted_server() {
        let lock = crate::mcp::mcp_state_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("asi_mcp_start_untrusted_{}", ts));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let state_path = dir.join("mcp_state.json");
        let old_state_path = std::env::var("ASI_MCP_STATE_PATH").ok();
        let old_allow_untrusted = std::env::var("ASI_MCP_ALLOW_UNTRUSTED_START").ok();
        std::env::set_var("ASI_MCP_STATE_PATH", state_path.display().to_string());
        std::env::set_var("ASI_MCP_ALLOW_UNTRUSTED_START", "false");

        let _ = crate::mcp::remove_server("untrusted");
        crate::mcp::add_server("untrusted", "echo hi").expect("add server");

        let out = super::handle_mcp_command("start untrusted --json").expect("mcp start json");
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp start json");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_start")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("started"))
                .and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str()),
            Some("untrusted")
        );
        let err = parsed
            .get("mcp")
            .and_then(|v| v.get("error"))
            .and_then(|v| v.as_str())
            .expect("mcp.error string");
        assert!(err.to_ascii_lowercase().contains("untrusted"));

        if let Some(v) = old_state_path {
            std::env::set_var("ASI_MCP_STATE_PATH", v);
        } else {
            std::env::remove_var("ASI_MCP_STATE_PATH");
        }
        if let Some(v) = old_allow_untrusted {
            std::env::set_var("ASI_MCP_ALLOW_UNTRUSTED_START", v);
        } else {
            std::env::remove_var("ASI_MCP_ALLOW_UNTRUSTED_START");
        }
        let _ = std::fs::remove_dir_all(dir);
        drop(lock);
    }

    #[test]
    fn mcp_json_response_envelope_is_stable() {
        let out = super::mcp_json_response(
            "mcp_list",
            serde_json::json!({
                "count": 0,
                "items": []
            }),
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp envelope");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some(super::MCP_JSON_SCHEMA_VERSION)
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_list")
        );
        assert!(parsed.get("mcp").is_some());
    }

    #[test]
    fn mcp_add_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_add",
            serde_json::json!({
                "added": true,
                "server": {
                    "name": "srv",
                    "status": "stopped",
                    "pid": serde_json::Value::Null,
                    "command": "echo hi",
                    "auth_type": "none",
                    "auth_value": serde_json::Value::Null,
                    "config": [],
                }
            }),
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp add");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_add")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("added"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("server"))
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str()),
            Some("srv")
        );
    }

    #[test]
    fn mcp_rm_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_rm",
            serde_json::json!({
                "name": "srv",
                "changed": true,
            }),
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp rm");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_rm")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str()),
            Some("srv")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("changed"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn mcp_start_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_start",
            serde_json::json!({
                "started": true,
                "server": {
                    "name": "srv",
                    "status": "running",
                    "pid": 1234,
                    "command": "echo hi",
                    "auth_type": "none",
                    "auth_value": serde_json::Value::Null,
                    "config": [],
                }
            }),
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp start");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_start")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("started"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("server"))
                .and_then(|v| v.get("status"))
                .and_then(|v| v.as_str()),
            Some("running")
        );
    }

    #[test]
    fn mcp_stop_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_stop",
            serde_json::json!({
                "name": "srv",
                "changed": true,
            }),
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp stop");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_stop")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str()),
            Some("srv")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("changed"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn mcp_auth_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_auth",
            serde_json::json!({
                "name": "srv",
                "auth_type": "bearer",
                "value_set": true,
            }),
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp auth");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_auth")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str()),
            Some("srv")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("auth_type"))
                .and_then(|v| v.as_str()),
            Some("bearer")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("value_set"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn mcp_oauth_login_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_oauth_login",
            serde_json::json!({
                "name": "srv",
                "provider": "deepseek",
                "scope": "project",
                "saved": true,
                "linked_auth": true,
            }),
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("parse mcp oauth login");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_oauth_login")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str()),
            Some("srv")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("provider"))
                .and_then(|v| v.as_str()),
            Some("deepseek")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("scope"))
                .and_then(|v| v.as_str()),
            Some("project")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("saved"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("linked_auth"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn mcp_oauth_status_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_oauth_status",
            serde_json::json!({
                "name": "srv",
                "provider": "deepseek",
                "token_present": true,
                "token_scope": "project",
                "request_scope": "auto",
                "stored_providers": ["deepseek", "openai"],
            }),
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("parse mcp oauth status");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_oauth_status")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("token_present"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("token_scope"))
                .and_then(|v| v.as_str()),
            Some("project")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("request_scope"))
                .and_then(|v| v.as_str()),
            Some("auto")
        );
        let providers = parsed
            .get("mcp")
            .and_then(|v| v.get("stored_providers"))
            .and_then(|v| v.as_array())
            .expect("stored_providers array");
        assert_eq!(providers.len(), 2);
    }

    #[test]
    fn mcp_oauth_logout_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_oauth_logout",
            serde_json::json!({
                "name": "srv",
                "provider": "deepseek",
                "scope": "all",
                "removed": true,
            }),
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("parse mcp oauth logout");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_oauth_logout")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("removed"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("scope"))
                .and_then(|v| v.as_str()),
            Some("all")
        );
    }

    #[test]
    fn mcp_config_set_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_config_set",
            serde_json::json!({
                "name": "srv",
                "key": "region",
            }),
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("parse mcp config set");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_config_set")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str()),
            Some("srv")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("key"))
                .and_then(|v| v.as_str()),
            Some("region")
        );
    }

    #[test]
    fn mcp_config_rm_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_config_rm",
            serde_json::json!({
                "name": "srv",
                "key": "region",
                "changed": true,
            }),
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("parse mcp config rm");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_config_rm")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str()),
            Some("srv")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("key"))
                .and_then(|v| v.as_str()),
            Some("region")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("changed"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn mcp_config_scope_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_config_scope",
            serde_json::json!({
                "name": "srv",
                "scope": "project",
            }),
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("parse mcp config scope");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_config_scope")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("scope"))
                .and_then(|v| v.as_str()),
            Some("project")
        );
    }

    #[test]
    fn mcp_config_trust_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_config_trust",
            serde_json::json!({
                "name": "srv",
                "trusted": true,
            }),
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("parse mcp config trust");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_config_trust")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("trusted"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn mcp_export_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_export",
            serde_json::json!({
                "path": "D:\\\\tmp\\\\mcp.json",
            }),
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp export");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_export")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("path"))
                .and_then(|v| v.as_str()),
            Some("D:\\\\tmp\\\\mcp.json")
        );
    }

    #[test]
    fn mcp_import_json_shape_snapshot_v1() {
        let out = super::mcp_json_response(
            "mcp_import",
            serde_json::json!({
                "path": "D:\\\\tmp\\\\mcp.json",
                "mode": "merge",
                "total_servers": 3,
            }),
        );
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("parse mcp import");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("mcp_import")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("path"))
                .and_then(|v| v.as_str()),
            Some("D:\\\\tmp\\\\mcp.json")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("mode"))
                .and_then(|v| v.as_str()),
            Some("merge")
        );
        assert_eq!(
            parsed
                .get("mcp")
                .and_then(|v| v.get("total_servers"))
                .and_then(|v| v.as_u64()),
            Some(3)
        );
    }

    #[test]
    fn parse_agent_wait_options_supports_json_and_legacy_forms() {
        let a = super::parse_agent_wait_options("wait --json sa-1 12").expect("parse wait");
        assert!(matches!(a.output_mode, super::AgentOutputMode::Json));
        assert_eq!(a.target_id.as_deref(), Some("sa-1"));
        assert_eq!(a.timeout_secs, 12);

        let b = super::parse_agent_wait_options("wait 15").expect("parse wait timeout");
        assert!(matches!(b.output_mode, super::AgentOutputMode::Text));
        assert!(b.target_id.is_none());
        assert_eq!(b.timeout_secs, 15);

        let c = super::parse_agent_wait_options("wait sa-2 9").expect("parse wait id timeout");
        assert!(matches!(c.output_mode, super::AgentOutputMode::Text));
        assert_eq!(c.target_id.as_deref(), Some("sa-2"));
        assert_eq!(c.timeout_secs, 9);

        let d = super::parse_agent_wait_options("wait --json").expect("parse wait json only");
        assert!(matches!(d.output_mode, super::AgentOutputMode::Json));
        assert!(d.target_id.is_none());
        assert_eq!(d.timeout_secs, 60);

        let e = super::parse_agent_wait_options("wait --jsonl sa-9 8").expect("parse wait jsonl");
        assert!(matches!(e.output_mode, super::AgentOutputMode::Jsonl));
        assert_eq!(e.target_id.as_deref(), Some("sa-9"));
        assert_eq!(e.timeout_secs, 8);
    }

    #[test]
    fn parse_agent_wait_options_rejects_invalid_flags_and_shapes() {
        let err = super::parse_agent_wait_options("wait --bad sa-1")
            .expect_err("invalid flag must fail");
        assert!(err.contains("Usage: /agent wait"));

        let err2 = super::parse_agent_wait_options("wait sa-1 notnum")
            .expect_err("invalid timeout must fail");
        assert!(err2.contains("Usage: /agent wait"));

        let err3 = super::parse_agent_wait_options("wait a b c")
            .expect_err("too many args must fail");
        assert!(err3.contains("Usage: /agent wait"));
    }

    #[test]
    fn parse_agent_list_options_supports_json_flag() {
        let a = super::parse_agent_list_options("list").expect("parse list");
        assert!(matches!(a.output_mode, super::AgentOutputMode::Text));
        assert_eq!(a.scope, super::AgentViewScope::All);
        assert!(a.profile.is_none());
        assert!(a.skill.is_none());

        let b = super::parse_agent_list_options("list --json").expect("parse list --json");
        assert!(matches!(b.output_mode, super::AgentOutputMode::Json));
        assert_eq!(b.scope, super::AgentViewScope::All);
        assert!(b.profile.is_none());
        assert!(b.skill.is_none());

        let c = super::parse_agent_list_options(
            "list --json --scope background --profile safe-review --skill review",
        )
        .expect("parse list with filters");
        assert!(matches!(c.output_mode, super::AgentOutputMode::Json));
        assert_eq!(c.scope, super::AgentViewScope::Background);
        assert_eq!(c.profile.as_deref(), Some("safe-review"));
        assert_eq!(c.skill.as_deref(), Some("review"));

        let d = super::parse_agent_list_options("list --jsonl --scope foreground")
            .expect("parse list --jsonl");
        assert!(matches!(d.output_mode, super::AgentOutputMode::Jsonl));
        assert_eq!(d.scope, super::AgentViewScope::Foreground);
    }

    #[test]
    fn parse_agent_list_options_rejects_invalid_shapes() {
        let err = super::parse_agent_list_options("list --bad")
            .expect_err("invalid flag must fail");
        assert!(err.contains("Usage: /agent list"));

        let err2 = super::parse_agent_list_options("list a b")
            .expect_err("extra args must fail");
        assert!(err2.contains("Usage: /agent list"));

        let err3 = super::parse_agent_list_options("list --scope invalid")
            .expect_err("invalid scope must fail");
        assert!(err3.contains("Usage: /agent list"));
    }

    #[test]
    fn parse_agent_status_options_supports_json_flag() {
        let a = super::parse_agent_status_options("status sa-1").expect("parse status");
        assert!(matches!(a.output_mode, super::AgentOutputMode::Text));
        assert_eq!(a.id, "sa-1");

        let b = super::parse_agent_status_options("status --json sa-2")
            .expect("parse status --json");
        assert!(matches!(b.output_mode, super::AgentOutputMode::Json));
        assert_eq!(b.id, "sa-2");

        let c = super::parse_agent_status_options("status --jsonl sa-3")
            .expect("parse status --jsonl");
        assert!(matches!(c.output_mode, super::AgentOutputMode::Jsonl));
        assert_eq!(c.id, "sa-3");
    }

    #[test]
    fn parse_agent_status_options_rejects_invalid_shapes() {
        let err = super::parse_agent_status_options("status")
            .expect_err("missing id must fail");
        assert!(err.contains("Usage: /agent status"));

        let err2 = super::parse_agent_status_options("status --bad sa-1")
            .expect_err("invalid flag must fail");
        assert!(err2.contains("Usage: /agent status"));

        let err3 = super::parse_agent_status_options("status a b")
            .expect_err("too many args must fail");
        assert!(err3.contains("Usage: /agent status"));
    }

    #[test]
    fn parse_agent_close_options_supports_json_flag() {
        let a = super::parse_agent_close_options("close sa-1").expect("parse close");
        assert!(matches!(a.output_mode, super::AgentOutputMode::Text));
        assert_eq!(a.id, "sa-1");

        let b = super::parse_agent_close_options("close --json sa-2")
            .expect("parse close --json");
        assert!(matches!(b.output_mode, super::AgentOutputMode::Json));
        assert_eq!(b.id, "sa-2");

        let c = super::parse_agent_close_options("close --jsonl sa-3")
            .expect("parse close --jsonl");
        assert!(matches!(c.output_mode, super::AgentOutputMode::Jsonl));
        assert_eq!(c.id, "sa-3");
    }

    #[test]
    fn parse_agent_close_options_rejects_invalid_shapes() {
        let err = super::parse_agent_close_options("close")
            .expect_err("missing id must fail");
        assert!(err.contains("Usage: /agent close"));

        let err2 = super::parse_agent_close_options("close --bad sa-1")
            .expect_err("invalid flag must fail");
        assert!(err2.contains("Usage: /agent close"));

        let err3 = super::parse_agent_close_options("close a b")
            .expect_err("too many args must fail");
        assert!(err3.contains("Usage: /agent close"));
    }

    #[test]
    fn parse_agent_log_options_supports_tail_and_json_flags() {
        let a = super::parse_agent_log_options("log sa-1").expect("parse log");
        assert!(matches!(a.output_mode, super::AgentOutputMode::Text));
        assert_eq!(a.id, "sa-1");
        assert_eq!(a.tail, None);

        let b = super::parse_agent_log_options("log --json sa-2 --tail 15")
            .expect("parse log --json");
        assert!(matches!(b.output_mode, super::AgentOutputMode::Json));
        assert_eq!(b.id, "sa-2");
        assert_eq!(b.tail, Some(15));

        let c = super::parse_agent_log_options("log --jsonl --tail 3 sa-3")
            .expect("parse log --jsonl");
        assert!(matches!(c.output_mode, super::AgentOutputMode::Jsonl));
        assert_eq!(c.id, "sa-3");
        assert_eq!(c.tail, Some(3));
    }

    #[test]
    fn parse_agent_log_options_rejects_invalid_shapes() {
        let err = super::parse_agent_log_options("log")
            .expect_err("missing id must fail");
        assert!(err.contains("Usage: /agent log"));

        let err2 = super::parse_agent_log_options("log sa-1 --tail 0")
            .expect_err("invalid tail must fail");
        assert!(err2.contains("Usage: /agent log"));

        let err3 = super::parse_agent_log_options("log --bad sa-1")
            .expect_err("invalid flag must fail");
        assert!(err3.contains("Usage: /agent log"));
    }

    #[test]
    fn parse_agent_retry_options_supports_json_flags() {
        let a = super::parse_agent_retry_options("retry sa-1").expect("parse retry");
        assert!(matches!(a.output_mode, super::AgentOutputMode::Text));
        assert_eq!(a.id, "sa-1");

        let b = super::parse_agent_retry_options("retry --json sa-2")
            .expect("parse retry --json");
        assert!(matches!(b.output_mode, super::AgentOutputMode::Json));
        assert_eq!(b.id, "sa-2");

        let c = super::parse_agent_retry_options("retry --jsonl sa-3")
            .expect("parse retry --jsonl");
        assert!(matches!(c.output_mode, super::AgentOutputMode::Jsonl));
        assert_eq!(c.id, "sa-3");
    }

    #[test]
    fn parse_agent_cancel_options_supports_json_flags() {
        let a = super::parse_agent_cancel_options("cancel sa-1").expect("parse cancel");
        assert!(matches!(a.output_mode, super::AgentOutputMode::Text));
        assert_eq!(a.id, "sa-1");

        let b = super::parse_agent_cancel_options("cancel --json sa-2")
            .expect("parse cancel --json");
        assert!(matches!(b.output_mode, super::AgentOutputMode::Json));
        assert_eq!(b.id, "sa-2");

        let c = super::parse_agent_cancel_options("cancel --jsonl sa-3")
            .expect("parse cancel --jsonl");
        assert!(matches!(c.output_mode, super::AgentOutputMode::Jsonl));
        assert_eq!(c.id, "sa-3");
    }

    #[test]
    fn parse_agent_retry_cancel_options_reject_invalid_shapes() {
        let err = super::parse_agent_retry_options("retry")
            .expect_err("missing id must fail");
        assert!(err.contains("Usage: /agent retry"));

        let err2 = super::parse_agent_retry_options("retry --bad sa-1")
            .expect_err("bad flag must fail");
        assert!(err2.contains("Usage: /agent retry"));

        let err3 = super::parse_agent_cancel_options("cancel")
            .expect_err("missing id must fail");
        assert!(err3.contains("Usage: /agent cancel"));

        let err4 = super::parse_agent_cancel_options("cancel a b")
            .expect_err("too many args must fail");
        assert!(err4.contains("Usage: /agent cancel"));
    }

    #[test]
    fn parse_agent_spawn_options_supports_json_and_legacy_forms() {
        let a = super::parse_agent_spawn_options("spawn do work").expect("parse spawn");
        assert!(matches!(a.output_mode, super::AgentOutputMode::Text));
        assert!(a.profile.is_none());
        assert!(!a.background);
        assert_eq!(a.task, "do work");

        let b = super::parse_agent_spawn_options("spawn --json do work")
            .expect("parse spawn --json");
        assert!(matches!(b.output_mode, super::AgentOutputMode::Json));
        assert!(b.profile.is_none());
        assert!(!b.background);
        assert_eq!(b.task, "do work");

        let c = super::parse_agent_spawn_options("plain legacy task")
            .expect("parse legacy /agent <task>");
        assert!(matches!(c.output_mode, super::AgentOutputMode::Text));
        assert!(c.profile.is_none());
        assert!(!c.background);
        assert_eq!(c.task, "plain legacy task");

        let d = super::parse_agent_spawn_options("--json from legacy")
            .expect("parse legacy json");
        assert!(matches!(d.output_mode, super::AgentOutputMode::Json));
        assert!(d.profile.is_none());
        assert!(!d.background);
        assert_eq!(d.task, "from legacy");

        let e = super::parse_agent_spawn_options("spawn --profile review run checks")
            .expect("parse profile");
        assert!(matches!(e.output_mode, super::AgentOutputMode::Text));
        assert_eq!(e.profile.as_deref(), Some("review"));
        assert!(!e.background);
        assert_eq!(e.task, "run checks");

        let f = super::parse_agent_spawn_options("spawn --json --profile deep task")
            .expect("parse json profile");
        assert!(matches!(f.output_mode, super::AgentOutputMode::Json));
        assert_eq!(f.profile.as_deref(), Some("deep"));
        assert!(!f.background);
        assert_eq!(f.task, "task");

        let g = super::parse_agent_spawn_options("spawn --background run in back")
            .expect("parse background");
        assert!(g.background);
        assert_eq!(g.task, "run in back");

        let h = super::parse_agent_spawn_options("spawn --foreground run in front")
            .expect("parse foreground");
        assert!(!h.background);
        assert_eq!(h.task, "run in front");

        let i = super::parse_agent_spawn_options("spawn --background --json --profile deep run")
            .expect("parse background with json and profile");
        assert!(i.background);
        assert!(matches!(i.output_mode, super::AgentOutputMode::Json));
        assert_eq!(i.profile.as_deref(), Some("deep"));
        assert_eq!(i.task, "run");

        let j = super::parse_agent_spawn_options("spawn --jsonl run quick")
            .expect("parse jsonl spawn");
        assert!(matches!(j.output_mode, super::AgentOutputMode::Jsonl));
        assert_eq!(j.task, "run quick");
    }

    #[test]
    fn parse_agent_spawn_options_rejects_missing_task() {
        let err = super::parse_agent_spawn_options("")
            .expect_err("empty must fail");
        assert!(err.contains("Usage: /agent spawn"));

        let err2 = super::parse_agent_spawn_options("spawn --json")
            .expect_err("missing task after --json must fail");
        assert!(err2.contains("Usage: /agent spawn"));

        let err3 = super::parse_agent_spawn_options("--json")
            .expect_err("legacy json without task must fail");
        assert!(err3.contains("Usage: /agent spawn"));

        let err4 = super::parse_agent_spawn_options("spawn --profile")
            .expect_err("missing profile name must fail");
        assert!(err4.contains("Usage: /agent spawn"));
    }

    #[test]
    fn load_agent_profile_reads_project_file() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("asi_agent_profile_{}", ts));
        std::fs::create_dir_all(&root).expect("create temp root");
        let profile_path = root.join("agent_profiles.json");
        std::fs::write(
            &profile_path,
            r#"{
  "profiles": {
    "deep-review": {
      "provider": "deepseek",
      "model": "deepseek-v4-pro",
      "permission_mode": "on-request",
      "allowed_tools": ["read_file", "glob_search", "grep_search"],
      "denied_tools": ["bash"],
      "default_skills": ["review", "security"],
      "disable_web_tools": true,
      "disable_bash_tool": false
    }
  }
}"#,
        )
        .expect("write profile file");

        let profile =
            super::load_agent_profile_from_path(&profile_path, "deep-review").expect("load profile");
        assert_eq!(profile.provider.as_deref(), Some("deepseek"));
        assert_eq!(profile.model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(profile.permission_mode.as_deref(), Some("on-request"));
        assert_eq!(
            profile.allowed_tools,
            Some(vec![
                "read_file".to_string(),
                "glob_search".to_string(),
                "grep_search".to_string()
            ])
        );
        assert_eq!(profile.denied_tools, Some(vec!["bash".to_string()]));
        assert_eq!(
            profile.default_skills,
            Some(vec!["review".to_string(), "security".to_string()])
        );
        assert_eq!(profile.disable_web_tools, Some(true));
        assert_eq!(profile.disable_bash_tool, Some(false));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_agent_profile_reports_missing_profile_name() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("asi_agent_profile_missing_{}", ts));
        std::fs::create_dir_all(&root).expect("create temp root");
        let profile_path = root.join("agent_profiles.json");
        std::fs::write(
            &profile_path,
            r#"{
  "profiles": {
    "safe": {
      "permission_mode": "on-request"
    }
  }
}"#,
        )
        .expect("write profile file");

        let err = super::load_agent_profile_from_path(&profile_path, "unknown")
            .expect_err("missing profile should error");
        assert!(err.contains("agent profile not found"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn apply_agent_profile_to_config_overrides_run_config() {
        let mut cfg = super::SubagentRunConfig {
            provider: "openai".to_string(),
            model: "gpt-5.3-codex".to_string(),
            permission_mode: "workspace-write".to_string(),
            rules_source: "runtime".to_string(),
            profile_name: None,
            default_skills: Vec::new(),
            max_turns: 8,
            app_cfg: AppConfig::default(),
            runtime_snapshot: super::SubagentRuntimeSnapshot {
                extended_thinking: false,
                disable_web_tools: false,
                disable_bash_tool: false,
                safe_shell_mode: true,
                permission_allow_rules: Vec::new(),
                permission_deny_rules: Vec::new(),
                path_restriction_enabled: false,
                additional_directories: Vec::new(),
                session_permission_allow_rules: Vec::new(),
                session_permission_deny_rules: Vec::new(),
                session_additional_directories: Vec::new(),
                next_permission_allow_rules: Vec::new(),
                next_additional_directories: Vec::new(),
                interactive_approval_allow_rules: Vec::new(),
                interactive_approval_deny_rules: Vec::new(),
                native_tool_calling: false,
            },
            tool_loop_enabled: true,
            strict_mode: false,
            speed: super::ExecutionSpeed::Deep,
            limits: super::AutoLoopLimits {
                max_steps: Some(16),
                max_duration: Duration::from_secs(300),
                max_no_progress_rounds: 5,
                max_consecutive_constraint_blocks: 3,
            },
            constraints: super::ToolExecutionConstraints::default(),
        };
        let profile = super::AgentProfile {
            provider: Some("deepseek".to_string()),
            model: Some("deepseek-v4-pro".to_string()),
            permission_mode: Some("on-request".to_string()),
            allowed_tools: Some(vec!["read_file".to_string(), "glob_search".to_string()]),
            denied_tools: Some(vec!["bash".to_string(), "edit_file".to_string()]),
            default_skills: Some(vec!["review".to_string(), "security".to_string()]),
            disable_web_tools: Some(true),
            disable_bash_tool: Some(true),
        };

        super::apply_agent_profile_to_config(&mut cfg, &profile);
        assert_eq!(cfg.provider, "deepseek");
        assert_eq!(cfg.model, "deepseek-v4-pro");
        assert_eq!(cfg.permission_mode, "on-request");
        assert!(cfg.app_cfg.is_feature_disabled("web_tools"));
        assert!(cfg.app_cfg.is_feature_disabled("bash_tool"));
        assert_eq!(
            cfg.runtime_snapshot.permission_allow_rules,
            vec!["read_file".to_string(), "glob_search".to_string()]
        );
        assert_eq!(
            cfg.runtime_snapshot.permission_deny_rules,
            vec!["bash".to_string(), "edit_file".to_string()]
        );
        assert_eq!(cfg.rules_source, "profile");
        assert_eq!(
            cfg.default_skills,
            vec!["review".to_string(), "security".to_string()]
        );
    }

    #[test]
    fn apply_agent_profile_to_config_marks_mixed_when_runtime_rules_exist() {
        let mut cfg = super::SubagentRunConfig {
            provider: "openai".to_string(),
            model: "gpt-5.3-codex".to_string(),
            permission_mode: "workspace-write".to_string(),
            rules_source: "runtime".to_string(),
            profile_name: None,
            default_skills: Vec::new(),
            max_turns: 8,
            app_cfg: AppConfig::default(),
            runtime_snapshot: super::SubagentRuntimeSnapshot {
                extended_thinking: false,
                disable_web_tools: false,
                disable_bash_tool: false,
                safe_shell_mode: true,
                permission_allow_rules: vec!["read_file".to_string()],
                permission_deny_rules: vec!["bash".to_string()],
                path_restriction_enabled: false,
                additional_directories: Vec::new(),
                session_permission_allow_rules: Vec::new(),
                session_permission_deny_rules: Vec::new(),
                session_additional_directories: Vec::new(),
                next_permission_allow_rules: Vec::new(),
                next_additional_directories: Vec::new(),
                interactive_approval_allow_rules: Vec::new(),
                interactive_approval_deny_rules: Vec::new(),
                native_tool_calling: false,
            },
            tool_loop_enabled: true,
            strict_mode: false,
            speed: super::ExecutionSpeed::Deep,
            limits: super::AutoLoopLimits {
                max_steps: Some(16),
                max_duration: Duration::from_secs(300),
                max_no_progress_rounds: 5,
                max_consecutive_constraint_blocks: 3,
            },
            constraints: super::ToolExecutionConstraints::default(),
        };
        let profile = super::AgentProfile {
            provider: None,
            model: None,
            permission_mode: None,
            allowed_tools: Some(vec!["glob_search".to_string()]),
            denied_tools: Some(vec!["edit_file".to_string()]),
            default_skills: None,
            disable_web_tools: None,
            disable_bash_tool: None,
        };
        super::apply_agent_profile_to_config(&mut cfg, &profile);
        assert_eq!(cfg.rules_source, "mixed");
        assert!(cfg
            .runtime_snapshot
            .permission_allow_rules
            .contains(&"read_file".to_string()));
        assert!(cfg
            .runtime_snapshot
            .permission_allow_rules
            .contains(&"glob_search".to_string()));
        assert!(cfg
            .runtime_snapshot
            .permission_deny_rules
            .contains(&"bash".to_string()));
        assert!(cfg
            .runtime_snapshot
            .permission_deny_rules
            .contains(&"edit_file".to_string()));
    }

    #[test]
    fn agent_json_response_includes_schema_version_and_agent_payload() {
        let payload = super::agent_json_response("status", serde_json::json!({
            "status": "running",
            "id": "sa-1"
        }));
        assert_eq!(
            payload
                .get("schema_version")
                .and_then(|v| v.as_str()),
            Some(super::AGENT_JSON_SCHEMA_VERSION)
        );
        assert_eq!(
            payload
                .get("command")
                .and_then(|v| v.as_str()),
            Some("status")
        );
        assert_eq!(
            payload
                .get("agent")
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str()),
            Some("sa-1")
        );
    }

    #[test]
    fn agent_jsonl_response_includes_event_and_data_envelope() {
        let payload = super::agent_jsonl_response(
            "status",
            serde_json::json!({
                "status": "running",
                "id": "sa-1"
            }),
        );
        assert_eq!(
            payload
                .get("schema_version")
                .and_then(|v| v.as_str()),
            Some(super::AGENT_JSON_SCHEMA_VERSION)
        );
        assert_eq!(
            payload
                .get("event")
                .and_then(|v| v.as_str()),
            Some("agent.status")
        );
        assert_eq!(
            payload
                .get("data")
                .and_then(|v| v.get("command"))
                .and_then(|v| v.as_str()),
            Some("status")
        );
        assert_eq!(
            payload
                .get("data")
                .and_then(|v| v.get("agent"))
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str()),
            Some("sa-1")
        );
    }

    #[test]
    fn agent_json_spawn_shape_snapshot_v1() {
        let payload = super::agent_json_response(
            "spawn",
            serde_json::json!({
                "id": "sa-1",
                "status": "running",
                "provider": "openai",
                "model": "gpt-5.3-codex",
                "run_mode": "foreground",
                "task": "inspect src/main.rs",
            }),
        );
        assert_eq!(
            payload,
            serde_json::json!({
                "schema_version": "1",
                "command": "spawn",
                "agent": {
                    "id": "sa-1",
                    "status": "running",
                    "provider": "openai",
                    "model": "gpt-5.3-codex",
                    "run_mode": "foreground",
                    "task": "inspect src/main.rs",
                }
            })
        );
    }

    #[test]
    fn agent_json_send_shape_snapshot_v1() {
        let payload = super::agent_json_response(
            "send",
            serde_json::json!({
                "id": "sa-1",
                "status": "running",
                "provider": "deepseek",
                "model": "deepseek-v4-pro",
                "run_mode": "background",
                "turns": 2,
                "interrupted_count": 1,
                "last_interrupted_at_ms": 123456u128,
                "started_at_ms": 1000u128,
                "finished_at_ms": serde_json::Value::Null,
                "task": "continue",
                "context": "append",
                "interrupt": true,
                "message": "subagent id=sa-1 interrupted previous run; queued turn=2 context=append task=continue",
            }),
        );
        assert_eq!(
            payload,
            serde_json::json!({
                "schema_version": "1",
                "command": "send",
                "agent": {
                    "id": "sa-1",
                    "status": "running",
                    "provider": "deepseek",
                    "model": "deepseek-v4-pro",
                    "run_mode": "background",
                    "turns": 2,
                    "interrupted_count": 1,
                    "last_interrupted_at_ms": 123456u128,
                    "started_at_ms": 1000u128,
                    "finished_at_ms": serde_json::Value::Null,
                    "task": "continue",
                    "context": "append",
                    "interrupt": true,
                    "message": "subagent id=sa-1 interrupted previous run; queued turn=2 context=append task=continue",
                }
            })
        );
    }

    #[test]
    fn agent_json_list_shape_snapshot_v1() {
        let payload = super::agent_json_response(
            "list",
            serde_json::json!({
                "count": 1,
                "filters_applied": {
                    "scope": "all",
                    "profile": "",
                    "skill": "",
                },
                "diagnostics": {
                    "filters": {
                        "scope": "all",
                        "profile": "",
                        "skill": "",
                    },
                    "total_items": 1,
                    "counts": {
                        "running": 0,
                        "completed": 1,
                        "failed": 0,
                        "closed": 0,
                        "unknown": 0,
                    },
                    "run_modes": {
                        "foreground": 1,
                        "background": 0,
                    },
                    "timings": {
                        "now_ms": 3000u128,
                        "age_ms_min": 2000u128,
                        "age_ms_max": 2000u128,
                        "completed_duration_ms_min": 1000u128,
                        "completed_duration_ms_max": 1000u128,
                    },
                },
                "items": [{
                    "id": "sa-1",
                    "status": "completed",
                    "provider": "openai",
                    "model": "gpt-5.3-codex",
                    "allow_rules": ["read_file"],
                    "deny_rules": [],
                    "rules_source": "runtime",
                    "run_mode": "foreground",
                    "turns": 1,
                    "interrupted_count": 0,
                    "last_interrupted_at_ms": serde_json::Value::Null,
                    "started_at_ms": 1000u128,
                    "finished_at_ms": 2000u128,
                    "age_ms": 2000u128,
                    "duration_ms": 1000u128,
                    "task": "inspect parser",
                    "preview": "ok",
                }]
            }),
        );
        assert_eq!(
            payload,
            serde_json::json!({
                "schema_version": "1",
                "command": "list",
                "agent": {
                    "count": 1,
                    "filters_applied": {
                        "scope": "all",
                        "profile": "",
                        "skill": "",
                    },
                    "diagnostics": {
                        "filters": {
                            "scope": "all",
                            "profile": "",
                            "skill": "",
                        },
                        "total_items": 1,
                        "counts": {
                            "running": 0,
                            "completed": 1,
                            "failed": 0,
                            "closed": 0,
                            "unknown": 0,
                        },
                        "run_modes": {
                            "foreground": 1,
                            "background": 0,
                        },
                        "timings": {
                            "now_ms": 3000u128,
                            "age_ms_min": 2000u128,
                            "age_ms_max": 2000u128,
                            "completed_duration_ms_min": 1000u128,
                            "completed_duration_ms_max": 1000u128,
                        },
                    },
                    "items": [{
                        "id": "sa-1",
                        "status": "completed",
                        "provider": "openai",
                        "model": "gpt-5.3-codex",
                        "allow_rules": ["read_file"],
                        "deny_rules": [],
                        "rules_source": "runtime",
                        "run_mode": "foreground",
                        "turns": 1,
                        "interrupted_count": 0,
                        "last_interrupted_at_ms": serde_json::Value::Null,
                        "started_at_ms": 1000u128,
                        "finished_at_ms": 2000u128,
                        "age_ms": 2000u128,
                        "duration_ms": 1000u128,
                        "task": "inspect parser",
                        "preview": "ok",
                    }]
                }
            })
        );
    }

    #[test]
    fn agent_json_status_shape_snapshot_v1() {
        let payload = super::agent_json_response(
            "status",
            serde_json::json!({
                "id": "sa-2",
                "status": "running",
                "provider": "anthropic",
                "model": "claude-sonnet",
                "allow_rules": ["read_file", "glob_search"],
                "deny_rules": ["bash"],
                "rules_source": "profile",
                "diagnostics": {
                    "rules_source": "profile",
                    "allow_rule_count": 2,
                    "deny_rule_count": 1,
                },
                "run_mode": "background",
                "turns": 3,
                "interrupted_count": 0,
                "last_interrupted_at_ms": serde_json::Value::Null,
                "started_at_ms": 1000u128,
                "finished_at_ms": serde_json::Value::Null,
                "task": "run review",
                "preview": serde_json::Value::Null,
            }),
        );
        assert_eq!(
            payload,
            serde_json::json!({
                "schema_version": "1",
                "command": "status",
                "agent": {
                    "id": "sa-2",
                    "status": "running",
                    "provider": "anthropic",
                    "model": "claude-sonnet",
                    "allow_rules": ["read_file", "glob_search"],
                    "deny_rules": ["bash"],
                    "rules_source": "profile",
                    "diagnostics": {
                        "rules_source": "profile",
                        "allow_rule_count": 2,
                        "deny_rule_count": 1,
                    },
                    "run_mode": "background",
                    "turns": 3,
                    "interrupted_count": 0,
                    "last_interrupted_at_ms": serde_json::Value::Null,
                    "started_at_ms": 1000u128,
                    "finished_at_ms": serde_json::Value::Null,
                    "task": "run review",
                    "preview": serde_json::Value::Null,
                }
            })
        );
    }

    #[test]
    fn agent_json_wait_done_shape_snapshot_v1() {
        let payload = super::agent_json_response(
            "wait",
            serde_json::json!({
                "id": "sa-3",
                "provider": "openai",
                "model": "gpt-5.3-codex",
                "allow_rules": ["read_file"],
                "deny_rules": ["bash"],
                "rules_source": "mixed",
                "run_mode": "foreground",
                "task": "summarize repo",
                "started_at_ms": 1000u128,
                "finished_at_ms": 3000u128,
                "status": "done",
                "ok": true,
                "result": "done",
            }),
        );
        assert_eq!(
            payload,
            serde_json::json!({
                "schema_version": "1",
                "command": "wait",
                "agent": {
                    "id": "sa-3",
                    "provider": "openai",
                    "model": "gpt-5.3-codex",
                    "allow_rules": ["read_file"],
                    "deny_rules": ["bash"],
                    "rules_source": "mixed",
                    "run_mode": "foreground",
                    "task": "summarize repo",
                    "started_at_ms": 1000u128,
                    "finished_at_ms": 3000u128,
                    "status": "done",
                    "ok": true,
                    "result": "done",
                }
            })
        );
    }

    #[test]
    fn agent_json_wait_timeout_shape_snapshot_v1() {
        let payload = super::agent_json_response(
            "wait",
            serde_json::json!({
                "status": "timeout",
                "timeout_secs": 30,
                "message": "no subagent finished within 30s",
            }),
        );
        assert_eq!(
            payload,
            serde_json::json!({
                "schema_version": "1",
                "command": "wait",
                "agent": {
                    "status": "timeout",
                    "timeout_secs": 30,
                    "message": "no subagent finished within 30s",
                }
            })
        );
    }

    #[test]
    fn agent_json_wait_idle_shape_snapshot_v1() {
        let payload = super::agent_json_response(
            "wait",
            serde_json::json!({
                "status": "idle",
                "message": "no running subagents",
            }),
        );
        assert_eq!(
            payload,
            serde_json::json!({
                "schema_version": "1",
                "command": "wait",
                "agent": {
                    "status": "idle",
                    "message": "no running subagents",
                }
            })
        );
    }

    #[test]
    fn agent_json_close_shape_snapshot_v1() {
        let payload = super::agent_json_response(
            "close",
            serde_json::json!({
                "id": "sa-1",
                "status": "closed",
                "message": "closed subagent id=sa-1",
            }),
        );
        assert_eq!(
            payload,
            serde_json::json!({
                "schema_version": "1",
                "command": "close",
                "agent": {
                    "id": "sa-1",
                    "status": "closed",
                    "message": "closed subagent id=sa-1",
                }
            })
        );
    }

    #[test]
    fn agent_json_log_shape_snapshot_v1() {
        let payload = super::agent_json_response(
            "log",
            serde_json::json!({
                "id": "sa-1",
                "status": "running",
                "provider": "deepseek",
                "model": "deepseek-v4-pro",
                "run_mode": "background",
                "turns": 3,
                "events_total": 4,
                "tail": 2,
                "items": [
                    { "at_ms": 1000u128, "event": "spawn", "message": "spawned" },
                    { "at_ms": 1200u128, "event": "send", "message": "queued" }
                ]
            }),
        );
        assert_eq!(
            payload,
            serde_json::json!({
                "schema_version": "1",
                "command": "log",
                "agent": {
                    "id": "sa-1",
                    "status": "running",
                    "provider": "deepseek",
                    "model": "deepseek-v4-pro",
                    "run_mode": "background",
                    "turns": 3,
                    "events_total": 4,
                    "tail": 2,
                    "items": [
                        { "at_ms": 1000u128, "event": "spawn", "message": "spawned" },
                        { "at_ms": 1200u128, "event": "send", "message": "queued" }
                    ]
                }
            })
        );
    }

    #[test]
    fn agent_json_retry_shape_snapshot_v1() {
        let payload = super::agent_json_response(
            "retry",
            serde_json::json!({
                "id": "sa-2",
                "status": "running",
                "provider": "openai",
                "model": "gpt-5.3-codex",
                "turns": 2,
                "run_mode": "foreground",
                "task": "inspect",
                "message": "subagent id=sa-2 retried turn=2 task=inspect"
            }),
        );
        assert_eq!(
            payload,
            serde_json::json!({
                "schema_version": "1",
                "command": "retry",
                "agent": {
                    "id": "sa-2",
                    "status": "running",
                    "provider": "openai",
                    "model": "gpt-5.3-codex",
                    "turns": 2,
                    "run_mode": "foreground",
                    "task": "inspect",
                    "message": "subagent id=sa-2 retried turn=2 task=inspect"
                }
            })
        );
    }

    #[test]
    fn agent_json_cancel_shape_snapshot_v1() {
        let payload = super::agent_json_response(
            "cancel",
            serde_json::json!({
                "id": "sa-3",
                "status": "cancelled",
                "message": "cancelled subagent id=sa-3"
            }),
        );
        assert_eq!(
            payload,
            serde_json::json!({
                "schema_version": "1",
                "command": "cancel",
                "agent": {
                    "id": "sa-3",
                    "status": "cancelled",
                    "message": "cancelled subagent id=sa-3"
                }
            })
        );
    }

    #[test]
    fn agent_json_string_roundtrip_parses_as_expected_envelope() {
        let payload = super::agent_json_response(
            "spawn",
            super::build_agent_spawn_payload(
                "sa-42",
                "openai",
                "gpt-5.3-codex",
                Some("safe-review"),
                &vec!["review".to_string()],
                false,
                "inspect src/main.rs",
            ),
        );
        let printed = format!("{}", payload);
        let parsed: serde_json::Value =
            serde_json::from_str(&printed).expect("must parse printed json");
        assert_eq!(
            parsed
                .get("schema_version")
                .and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            parsed.get("command").and_then(|v| v.as_str()),
            Some("spawn")
        );
        assert_eq!(
            parsed
                .get("agent")
                .and_then(|v| v.get("id"))
                .and_then(|v| v.as_str()),
            Some("sa-42")
        );
        assert_eq!(
            parsed
                .get("agent")
                .and_then(|v| v.get("profile"))
                .and_then(|v| v.as_str()),
            Some("safe-review")
        );
        assert_eq!(
            parsed
                .get("agent")
                .and_then(|v| v.get("default_skills"))
                .and_then(|v| v.as_array())
                .map(|v| v.len()),
            Some(1)
        );
    }

    #[test]
    fn agent_json_builders_roundtrip_through_string_for_send_list_wait() {
        let state = super::SubagentState {
            task: super::SubagentTask {
                id: "sa-7".to_string(),
                provider: "deepseek".to_string(),
                model: "deepseek-v4-pro".to_string(),
                permission_mode: "on-request".to_string(),
                allow_rules: vec!["read_file".to_string(), "glob_search".to_string()],
                deny_rules: vec!["bash".to_string()],
                rules_source: "profile".to_string(),
                profile_name: Some("safe-review".to_string()),
                default_skills: vec!["review".to_string()],
                background: true,
                task: "continue".to_string(),
                started_at_ms: 1000,
            },
            status: "running".to_string(),
            finished_at_ms: None,
            output_preview: Some("pending".to_string()),
            submitted_turns: 2,
            interrupted_count: 1,
            last_interrupted_at_ms: Some(1234),
            history: Vec::new(),
        };
        let send_payload = super::build_agent_send_payload(
            Some(&state),
            "sa-7",
            false,
            true,
            "queued",
        );
        let send_json = super::agent_json_response("send", send_payload);
        let send_parsed: serde_json::Value =
            serde_json::from_str(&send_json.to_string()).expect("parse send json");
        assert_eq!(
            send_parsed
                .get("agent")
                .and_then(|v| v.get("context"))
                .and_then(|v| v.as_str()),
            Some("append")
        );
        assert_eq!(
            send_parsed
                .get("agent")
                .and_then(|v| v.get("run_mode"))
                .and_then(|v| v.as_str()),
            Some("background")
        );
        assert_eq!(
            send_parsed
                .get("agent")
                .and_then(|v| v.get("interrupt"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            send_parsed
                .get("agent")
                .and_then(|v| v.get("profile"))
                .and_then(|v| v.as_str()),
            Some("safe-review")
        );
        assert_eq!(
            send_parsed
                .get("agent")
                .and_then(|v| v.get("allow_rules"))
                .and_then(|v| v.as_array())
                .map(|v| v.len()),
            Some(2)
        );
        assert_eq!(
            send_parsed
                .get("agent")
                .and_then(|v| v.get("rules_source"))
                .and_then(|v| v.as_str()),
            Some("profile")
        );

        let list_payload = super::build_agent_list_payload(
            &[state.clone()],
            super::AgentViewScope::All,
            Some("safe-review"),
            Some("review"),
        );
        let list_json = super::agent_json_response("list", list_payload);
        let list_parsed: serde_json::Value =
            serde_json::from_str(&list_json.to_string()).expect("parse list json");
        assert_eq!(
            list_parsed
                .get("agent")
                .and_then(|v| v.get("count"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );
        assert_eq!(
            list_parsed
                .get("agent")
                .and_then(|v| v.get("filters_applied"))
                .and_then(|v| v.get("scope"))
                .and_then(|v| v.as_str()),
            Some("all")
        );
        assert_eq!(
            list_parsed
                .get("agent")
                .and_then(|v| v.get("filters_applied"))
                .and_then(|v| v.get("profile"))
                .and_then(|v| v.as_str()),
            Some("safe-review")
        );
        assert_eq!(
            list_parsed
                .get("agent")
                .and_then(|v| v.get("diagnostics"))
                .and_then(|v| v.get("filters"))
                .and_then(|v| v.get("scope"))
                .and_then(|v| v.as_str()),
            Some("all")
        );
        assert_eq!(
            list_parsed
                .get("agent")
                .and_then(|v| v.get("diagnostics"))
                .and_then(|v| v.get("total_items"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );
        assert_eq!(
            list_parsed
                .get("agent")
                .and_then(|v| v.get("diagnostics"))
                .and_then(|v| v.get("counts"))
                .and_then(|v| v.get("running"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );
        assert_eq!(
            list_parsed
                .get("agent")
                .and_then(|v| v.get("diagnostics"))
                .and_then(|v| v.get("run_modes"))
                .and_then(|v| v.get("background"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );
        let now_ms = list_parsed
            .get("agent")
            .and_then(|v| v.get("diagnostics"))
            .and_then(|v| v.get("timings"))
            .and_then(|v| v.get("now_ms"))
            .and_then(|v| v.as_u64())
            .expect("timings.now_ms must exist");
        let age_ms_max = list_parsed
            .get("agent")
            .and_then(|v| v.get("diagnostics"))
            .and_then(|v| v.get("timings"))
            .and_then(|v| v.get("age_ms_max"))
            .and_then(|v| v.as_u64())
            .expect("timings.age_ms_max must exist");
        assert_eq!(age_ms_max, now_ms.saturating_sub(1000));
        assert_eq!(
            list_parsed
                .get("agent")
                .and_then(|v| v.get("items"))
                .and_then(|v| v.as_array())
                .map(|v| v.len()),
            Some(1)
        );
        assert_eq!(
            list_parsed
                .get("agent")
                .and_then(|v| v.get("items"))
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.get("default_skills"))
                .and_then(|v| v.as_array())
                .map(|v| v.len()),
            Some(1)
        );
        assert_eq!(
            list_parsed
                .get("agent")
                .and_then(|v| v.get("items"))
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.get("rules_source"))
                .and_then(|v| v.as_str()),
            Some("profile")
        );
        assert_eq!(
            list_parsed
                .get("agent")
                .and_then(|v| v.get("items"))
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.get("age_ms"))
                .and_then(|v| v.as_u64()),
            Some(now_ms.saturating_sub(1000))
        );
        assert_eq!(
            list_parsed
                .get("agent")
                .and_then(|v| v.get("items"))
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.get("duration_ms"))
                .and_then(|v| v.as_u64()),
            None
        );
        let status_payload = super::build_agent_status_payload(Some(&state), "sa-7");
        let status_json = super::agent_json_response("status", status_payload);
        let status_parsed: serde_json::Value =
            serde_json::from_str(&status_json.to_string()).expect("parse status json");
        assert_eq!(
            status_parsed
                .get("agent")
                .and_then(|v| v.get("diagnostics"))
                .and_then(|v| v.get("rules_source"))
                .and_then(|v| v.as_str()),
            Some("profile")
        );
        assert_eq!(
            status_parsed
                .get("agent")
                .and_then(|v| v.get("diagnostics"))
                .and_then(|v| v.get("allow_rule_count"))
                .and_then(|v| v.as_u64()),
            Some(2)
        );
        assert_eq!(
            status_parsed
                .get("agent")
                .and_then(|v| v.get("diagnostics"))
                .and_then(|v| v.get("deny_rule_count"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );

        let done = super::SubagentOutcome {
            task: state.task.clone(),
            finished_at_ms: 2000,
            result: Ok("ok".to_string()),
        };
        let wait_payload = super::build_agent_wait_done_payload(&done);
        let wait_json = super::agent_json_response("wait", wait_payload);
        let wait_parsed: serde_json::Value =
            serde_json::from_str(&wait_json.to_string()).expect("parse wait json");
        assert_eq!(
            wait_parsed
                .get("agent")
                .and_then(|v| v.get("status"))
                .and_then(|v| v.as_str()),
            Some("done")
        );
        assert_eq!(
            wait_parsed
                .get("agent")
                .and_then(|v| v.get("ok"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            wait_parsed
                .get("agent")
                .and_then(|v| v.get("rules_source"))
                .and_then(|v| v.as_str()),
            Some("profile")
        );
    }

    #[test]
    fn format_subagent_list_includes_interrupt_observability_fields() {
        let row = super::SubagentState {
            task: super::SubagentTask {
                id: "sa-1".to_string(),
                provider: "deepseek".to_string(),
                model: "deepseek-v4-pro".to_string(),
                permission_mode: "workspace-write".to_string(),
                allow_rules: vec!["read_file".to_string()],
                deny_rules: vec!["bash".to_string()],
                rules_source: "runtime".to_string(),
                profile_name: None,
                default_skills: Vec::new(),
                background: true,
                task: "check parser".to_string(),
                started_at_ms: 1000,
            },
            status: "running".to_string(),
            finished_at_ms: None,
            output_preview: Some("pending".to_string()),
            submitted_turns: 3,
            interrupted_count: 2,
            last_interrupted_at_ms: Some(1234),
            history: Vec::new(),
        };
        let text = super::format_subagent_list(&[row]);
        assert!(text.contains("interrupted_count=2"));
        assert!(text.contains("last_interrupted_at_ms=1234"));
        assert!(text.contains("turns=3"));
        assert!(text.contains("run_mode=background"));
    }

    #[test]
    fn subagent_status_text_format_includes_rule_summaries() {
        let state = super::SubagentState {
            task: super::SubagentTask {
                id: "sa-9".to_string(),
                provider: "openai".to_string(),
                model: "gpt-5.3-codex".to_string(),
                permission_mode: "on-request".to_string(),
                allow_rules: vec!["read_file".to_string(), "glob_search".to_string()],
                deny_rules: vec!["bash".to_string()],
                rules_source: "mixed".to_string(),
                profile_name: Some("safe-review".to_string()),
                default_skills: vec!["review".to_string()],
                background: false,
                task: "inspect".to_string(),
                started_at_ms: 100,
            },
            status: "running".to_string(),
            finished_at_ms: None,
            output_preview: None,
            submitted_turns: 1,
            interrupted_count: 0,
            last_interrupted_at_ms: None,
            history: Vec::new(),
        };
        let finished = state
            .finished_at_ms
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string());
        let last_interrupted = state
            .last_interrupted_at_ms
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string());
        let allow_rules = if state.task.allow_rules.is_empty() {
            "-".to_string()
        } else {
            state.task.allow_rules.join(",")
        };
        let deny_rules = if state.task.deny_rules.is_empty() {
            "-".to_string()
        } else {
            state.task.deny_rules.join(",")
        };
        let line = format!(
            "subagent id={} status={} provider={} model={} turns={} interrupted_count={} last_interrupted_at_ms={} started_at_ms={} finished_at_ms={} allow_rules=[{}] deny_rules=[{}]",
            state.task.id,
            state.status,
            state.task.provider,
            state.task.model,
            state.submitted_turns,
            state.interrupted_count,
            last_interrupted,
            state.task.started_at_ms,
            finished,
            allow_rules,
            deny_rules
        );
        assert!(line.contains("allow_rules=[read_file,glob_search]"));
        assert!(line.contains("deny_rules=[bash]"));
        assert_eq!(state.task.rules_source, "mixed");
    }

    #[test]
    fn filter_subagent_rows_applies_scope_profile_and_skill() {
        let mk = |id: &str, bg: bool, profile: Option<&str>, skills: &[&str]| super::SubagentState {
            task: super::SubagentTask {
                id: id.to_string(),
                provider: "deepseek".to_string(),
                model: "deepseek-v4-pro".to_string(),
                permission_mode: "on-request".to_string(),
                allow_rules: vec!["read_file".to_string()],
                deny_rules: Vec::new(),
                rules_source: "runtime".to_string(),
                profile_name: profile.map(|v| v.to_string()),
                default_skills: skills.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
                background: bg,
                task: "task".to_string(),
                started_at_ms: 1,
            },
            status: "running".to_string(),
            finished_at_ms: None,
            output_preview: None,
            submitted_turns: 1,
            interrupted_count: 0,
            last_interrupted_at_ms: None,
            history: Vec::new(),
        };

        let rows = vec![
            mk("sa-1", true, Some("safe-review"), &["review"]),
            mk("sa-2", false, Some("fast-code"), &["coding"]),
            mk("sa-3", true, Some("safe-review"), &["security"]),
        ];
        let by_scope = super::filter_subagent_rows(
            rows.clone(),
            super::AgentViewScope::Background,
            None,
            None,
        );
        assert_eq!(by_scope.len(), 2);

        let by_profile = super::filter_subagent_rows(
            rows.clone(),
            super::AgentViewScope::All,
            Some("safe-review"),
            None,
        );
        assert_eq!(by_profile.len(), 2);

        let by_skill = super::filter_subagent_rows(
            rows.clone(),
            super::AgentViewScope::All,
            None,
            Some("security"),
        );
        assert_eq!(by_skill.len(), 1);
        assert_eq!(by_skill[0].task.id, "sa-3");

        let combo = super::filter_subagent_rows(
            rows,
            super::AgentViewScope::Background,
            Some("safe-review"),
            Some("review"),
        );
        assert_eq!(combo.len(), 1);
        assert_eq!(combo[0].task.id, "sa-1");
    }

    #[test]
    fn format_subagent_list_with_filters_includes_filter_summary() {
        let row = super::SubagentState {
            task: super::SubagentTask {
                id: "sa-1".to_string(),
                provider: "deepseek".to_string(),
                model: "deepseek-v4-pro".to_string(),
                permission_mode: "on-request".to_string(),
                allow_rules: vec!["read_file".to_string()],
                deny_rules: Vec::new(),
                rules_source: "runtime".to_string(),
                profile_name: Some("safe-review".to_string()),
                default_skills: vec!["review".to_string()],
                background: true,
                task: "inspect".to_string(),
                started_at_ms: 1,
            },
            status: "running".to_string(),
            finished_at_ms: None,
            output_preview: None,
            submitted_turns: 1,
            interrupted_count: 0,
            last_interrupted_at_ms: None,
            history: Vec::new(),
        };
        let text = super::format_subagent_list_with_filters(
            &[row],
            super::AgentViewScope::Background,
            Some("safe-review"),
            Some("review"),
        );
        assert!(text.contains("scope=background"));
        assert!(text.contains("profile=safe-review"));
        assert!(text.contains("skill=review"));
        assert!(text.contains("summary running=1 completed=0 failed=0 closed=0 unknown=0"));
        assert!(text.contains("foreground=0 background=1"));
        assert!(text.contains("max_age_ms="));
        assert!(text.contains("max_finished_duration_ms=-"));
    }

    #[test]
    fn format_subagent_list_with_filters_empty_includes_filter_summary() {
        let text = super::format_subagent_list_with_filters(
            &[],
            super::AgentViewScope::All,
            None,
            None,
        );
        assert_eq!(text, "subagents: none scope=all profile=- skill=-");
    }

    #[test]
    fn format_subagent_log_rows_formats_events() {
        let state = super::SubagentState {
            task: super::SubagentTask {
                id: "sa-1".to_string(),
                provider: "deepseek".to_string(),
                model: "deepseek-v4-pro".to_string(),
                permission_mode: "on-request".to_string(),
                allow_rules: vec!["read_file".to_string()],
                deny_rules: vec![],
                rules_source: "runtime".to_string(),
                profile_name: None,
                default_skills: vec![],
                background: true,
                task: "inspect".to_string(),
                started_at_ms: 1,
            },
            status: "running".to_string(),
            finished_at_ms: None,
            output_preview: None,
            submitted_turns: 2,
            interrupted_count: 0,
            last_interrupted_at_ms: None,
            history: Vec::new(),
        };
        let events = vec![
            super::SubagentEvent {
                at_ms: 1000,
                event: "spawn".to_string(),
                message: "spawned".to_string(),
            },
            super::SubagentEvent {
                at_ms: 1200,
                event: "send".to_string(),
                message: "queued".to_string(),
            },
        ];
        let text = super::format_subagent_log_rows(&state, &events, 2);
        assert!(text.contains("subagent log id=sa-1"));
        assert!(text.contains("event=spawn"));
        assert!(text.contains("event=send"));
    }

    #[test]
    fn user_disallows_uv_detects_multilingual_constraints() {
        assert!(user_disallows_uv("do not use uv to run"));
        assert!(user_disallows_uv("do not use uv for this task"));
        assert!(!user_disallows_uv("you can use uv"));
    }

    #[test]
    fn run_only_constraint_detects_execute_intent_without_edit_intent() {
        assert!(user_requests_run_only(
            "run the code according to program.md, dataset is ready, do not modify files"
        ));
        assert!(user_requests_run_only(
            "run the code according to program.md and do not modify source files"
        ));
        assert!(!user_requests_run_only("run and fix bugs in train.py"));
    }

    #[test]
    fn strict_constraints_block_git_branch_and_mutating_tools() {
        let c = derive_tool_execution_constraints(
            "run code according to program.md and do not use uv",
            true,
        );
        let git_block = toolcall_is_blocked_by_user_constraints(
            "/toolcall bash \"git checkout -b tmp\"",
            c,
        );
        assert!(git_block
            .unwrap_or_default()
            .contains("git branch/history-changing commands"));

        let edit_block =
            toolcall_is_blocked_by_user_constraints("/toolcall edit_file \"a.py\" \"x\" \"y\"", c);
        assert!(edit_block
            .unwrap_or_default()
            .contains("run-only and does not allow file edits"));

        let uv_block =
            toolcall_is_blocked_by_user_constraints("/toolcall bash \"uv run train.py\"", c);
        assert!(uv_block
            .unwrap_or_default()
            .contains("not to use uv"));
    }

    #[test]
    fn explicit_git_request_disables_git_branch_block() {
        let c = derive_tool_execution_constraints("please git checkout -b feature/new", true);
        let blocked = toolcall_is_blocked_by_user_constraints(
            "/toolcall bash \"git checkout -b feature/new\"",
            c,
        );
        assert!(blocked.is_none());
    }

    #[test]
    fn bash_constraint_detectors_cover_common_forms() {
        assert!(bash_uses_uv("cd repo ; uv run train.py"));
        assert!(bash_uses_uv("uv sync"));
        assert!(bash_uses_forbidden_git_branching("git switch feat/x"));
        assert!(bash_uses_forbidden_git_branching("git reset --hard HEAD~1"));
    }

    #[test]
    fn auto_agent_tool_use_stop_reason_message_is_human_readable() {
        let msg =
            "auto-agent ended with stop_reason=tool_use; likely hit tool-step/failure limit before final prose";
        assert!(msg.contains("stop_reason=tool_use"));
        assert!(msg.contains("tool-step/failure limit"));
    }

    #[test]
    fn auto_loop_continuable_stop_reason_accepts_stop_aliases() {
        assert!(is_auto_loop_continuable_stop_reason("completed"));
        assert!(is_auto_loop_continuable_stop_reason("tool_use"));
        assert!(is_auto_loop_continuable_stop_reason("tool_result"));
        assert!(is_auto_loop_continuable_stop_reason("stop"));
        assert!(is_auto_loop_continuable_stop_reason("end_turn"));
        assert!(!is_auto_loop_continuable_stop_reason("provider_error"));
    }

    #[test]
    fn parse_auto_steps_value_supports_unlimited_forms() {
        assert_eq!(parse_auto_steps_value("0"), Some(0));
        assert_eq!(parse_auto_steps_value("unlimited"), Some(0));
        assert_eq!(parse_auto_steps_value("inf"), Some(0));
        assert_eq!(parse_auto_steps_value("120"), Some(120));
        assert_eq!(parse_auto_steps_value("bad"), None);
    }

    #[test]
    fn parse_auto_limit_duration_supports_seconds_and_unlimited() {
        assert_eq!(parse_auto_limit_duration("0").unwrap().as_secs(), u64::MAX / 4);
        assert_eq!(
            parse_auto_limit_duration("unlimited").unwrap().as_secs(),
            u64::MAX / 4
        );
        assert_eq!(parse_auto_limit_duration("90").unwrap().as_secs(), 90);
        assert!(parse_auto_limit_duration("bad").is_none());
    }

    #[test]
    fn parse_auto_loop_limits_from_env_reads_constraint_block_limit() {
        std::env::set_var("ASI_AUTO_AGENT_MAX_STEPS", "0");
        std::env::set_var("ASI_AUTO_AGENT_MAX_DURATION_SECS", "120");
        std::env::set_var("ASI_AUTO_AGENT_MAX_NO_PROGRESS_ROUNDS", "7");
        std::env::set_var("ASI_AUTO_AGENT_MAX_CONSECUTIVE_CONSTRAINT_BLOCKS", "5");

        let limits = parse_auto_loop_limits_from_env(50);
        assert_eq!(limits.max_steps, None);
        assert_eq!(limits.max_duration.as_secs(), 120);
        assert_eq!(limits.max_no_progress_rounds, 7);
        assert_eq!(limits.max_consecutive_constraint_blocks, 5);

        std::env::remove_var("ASI_AUTO_AGENT_MAX_STEPS");
        std::env::remove_var("ASI_AUTO_AGENT_MAX_DURATION_SECS");
        std::env::remove_var("ASI_AUTO_AGENT_MAX_NO_PROGRESS_ROUNDS");
        std::env::remove_var("ASI_AUTO_AGENT_MAX_CONSECUTIVE_CONSTRAINT_BLOCKS");
    }

    #[test]
    fn parse_prompt_profile_accepts_standard_and_strict() {
        assert_eq!(
            parse_prompt_profile("standard"),
            Some(PromptProfile::Standard)
        );
        assert_eq!(parse_prompt_profile("strict"), Some(PromptProfile::Strict));
        assert_eq!(parse_prompt_profile("unknown"), None);
    }

    #[test]
    fn estimate_task_complexity_classifies_simple_medium_complex() {
        assert_eq!(
            estimate_task_complexity("hello", false),
            TaskComplexity::Simple
        );
        assert_eq!(
            estimate_task_complexity("/work fix one bug", false),
            TaskComplexity::Medium
        );
        assert_eq!(
            estimate_task_complexity(
                "/secure refactor architecture and run benchmark then validate",
                true,
            ),
            TaskComplexity::Complex
        );
    }

    #[test]
    fn estimate_task_complexity_boosts_long_cjk_requests() {
        assert_eq!(
            estimate_task_complexity("\u{30b3}\u{30fc}\u{30c9}\u{3092}\u{306a}\u{304a}\u{3057}\u{3066}", false),
            TaskComplexity::Simple
        );
        assert_eq!(
            estimate_task_complexity(
                "\u{30d7}\u{30ed}\u{30b8}\u{30a7}\u{30af}\u{30c8}\u{5168}\u{4f53}\u{3092}\u{5206}\u{6790}\u{3057}\u{3066}\u{30ec}\u{30dd}\u{30fc}\u{30c8}\u{3082}\u{4f5c}\u{6210}\u{3057}\u{3066}\u{30c6}\u{30b9}\u{30c8}\u{3057}\u{3066}\u{4fee}\u{6b63}\u{3057}\u{3066}\u{518d}\u{78ba}\u{8a8d}\u{3057}\u{3066}",
                false
            ),
            TaskComplexity::Medium
        );
    }

    #[test]
    fn auto_tooling_gate_skips_smalltalk_but_keeps_coding_intent() {
        assert!(!should_enable_auto_tooling_for_turn("hello", false));
        assert!(!should_enable_auto_tooling_for_turn("thanks", false));
        assert!(!should_enable_auto_tooling_for_turn("yo", false));
        assert!(!should_enable_auto_tooling_for_turn("ok", false));
        assert!(should_enable_auto_tooling_for_turn("fix this bug", false));
        assert!(should_enable_auto_tooling_for_turn(
            "\u{30b3}\u{30fc}\u{30c9}\u{3092}\u{306a}\u{304a}\u{3057}\u{3066}",
            false
        ));
        assert!(should_enable_auto_tooling_for_turn(
            "\u{d14c}\u{c2a4}\u{d2b8}\u{d558}\u{ace0}\u{20}\u{ace0}\u{ccd0}\u{c918}",
            false
        ));
        assert!(should_enable_auto_tooling_for_turn(
            "write english and chinese .md report",
            false
        ));
        assert!(should_enable_auto_tooling_for_turn(
            "analyze current directory",
            false
        ));
        assert!(should_enable_auto_tooling_for_turn("inspect project files", false));
        assert!(should_enable_auto_tooling_for_turn("/work inspect repo", false));
        assert!(should_enable_auto_tooling_for_turn("hello", true));
    }

    #[test]
    fn non_agent_prompt_auto_tools_only_runs_for_coding_intent() {
        assert!(!should_enable_auto_tooling_for_turn("hello", false));
        assert!(should_enable_auto_tooling_for_turn("inspect project files", false));
    }

    #[test]
    fn work_prompt_contains_context_contract_header() {
        let snapshot = "workspace_root=D:/Code/Rust";
        let prompt = build_work_prompt("inspect project files", snapshot, false);
        assert!(prompt.contains("Execution rules:"));
        assert!(prompt.contains("Workspace snapshot:"));
    }

    #[test]
    fn adaptive_budgets_shrink_simple_and_expand_complex() {
        let base = AutoLoopLimits {
            max_steps: Some(50),
            max_duration: Duration::from_secs(3600),
            max_no_progress_rounds: 12,
            max_consecutive_constraint_blocks: 3,
        };

        let simple = apply_adaptive_budgets(
            base,
            TaskComplexity::Simple,
            false,
            ExecutionSpeed::Deep,
        );
        assert!(simple.max_steps.unwrap_or(0) < 50);
        assert!(simple.max_duration.as_secs() < 3600);
        assert!(simple.max_no_progress_rounds < 12);

        let complex = apply_adaptive_budgets(
            base,
            TaskComplexity::Complex,
            true,
            ExecutionSpeed::Deep,
        );
        assert!(complex.max_steps.unwrap_or(0) > 50);
        assert!(complex.max_duration.as_secs() > 3600);
        assert!(complex.max_no_progress_rounds > 12);
        assert!(complex.max_consecutive_constraint_blocks >= 4);
    }

    #[test]
    fn adaptive_budgets_respect_sprint_vs_deep_speed() {
        let base = AutoLoopLimits {
            max_steps: Some(50),
            max_duration: Duration::from_secs(3600),
            max_no_progress_rounds: 12,
            max_consecutive_constraint_blocks: 3,
        };
        let sprint = apply_adaptive_budgets(base, TaskComplexity::Medium, true, ExecutionSpeed::Sprint);
        let deep = apply_adaptive_budgets(base, TaskComplexity::Medium, true, ExecutionSpeed::Deep);
        assert!(sprint.max_steps.unwrap_or(0) < deep.max_steps.unwrap_or(0));
        assert!(sprint.max_duration.as_secs() < deep.max_duration.as_secs());
        assert!(sprint.max_no_progress_rounds <= deep.max_no_progress_rounds);
    }

    #[test]
    fn manual_chat_guard_disallows_pseudo_execution_and_recommends_toolcall() {
        let text = build_manual_chat_guard("workspace-write");
        assert!(text.contains("auto_agent=off"));
        assert!(text.contains("Do not claim tools were executed"));
        assert!(text.contains("bash(\"...\")"));
        assert!(text.contains("provide exact /toolcall commands"));
    }

    #[test]
    fn chat_guard_blocks_fabricated_tool_output_in_non_coding_turns() {
        let text = build_chat_no_fabrication_guard("workspace-write");
        assert!(text.contains("Chat Contract:"));
        assert!(text.contains("normal chat mode"));
        assert!(text.contains("Do not fabricate tool execution logs"));
        assert!(text.contains("pseudo-results like [tool:*]"));
    }

    #[test]
    fn parse_execution_speed_accepts_sprint_and_deep_aliases() {
        assert_eq!(parse_execution_speed("sprint"), Some(ExecutionSpeed::Sprint));
        assert_eq!(parse_execution_speed("fast"), Some(ExecutionSpeed::Sprint));
        assert_eq!(parse_execution_speed("deep"), Some(ExecutionSpeed::Deep));
        assert_eq!(parse_execution_speed("thorough"), Some(ExecutionSpeed::Deep));
        assert_eq!(parse_execution_speed("unknown"), None);
    }

    #[test]
    fn resolve_execution_speed_prefers_cli_and_falls_back_to_config() {
        let mut cfg = AppConfig::default();
        cfg.execution_speed = "sprint".to_string();
        assert_eq!(
            resolve_execution_speed(None, &cfg),
            ExecutionSpeed::Sprint
        );
        assert_eq!(
            resolve_execution_speed(Some(ExecutionSpeed::Deep), &cfg),
            ExecutionSpeed::Deep
        );
    }

    #[test]
    fn resolve_execution_speed_invalid_config_falls_back_to_deep() {
        let mut cfg = AppConfig::default();
        cfg.execution_speed = "invalid".to_string();
        assert_eq!(resolve_execution_speed(None, &cfg), ExecutionSpeed::Deep);
    }

    #[test]
    fn normalize_permission_mode_maps_aliases_and_defaults() {
        assert_eq!(normalize_permission_mode("ask"), "on-request");
        assert_eq!(normalize_permission_mode("on_request"), "on-request");
        assert_eq!(normalize_permission_mode("interactive"), "on-request");
        assert_eq!(
            normalize_permission_mode("workspace_write"),
            "workspace-write"
        );
        assert_eq!(
            normalize_permission_mode("danger_full_access"),
            "danger-full-access"
        );
        assert_eq!(normalize_permission_mode("read_only"), "read-only");
        assert_eq!(
            normalize_permission_mode("unexpected-mode"),
            "on-request"
        );
    }

    #[test]
    fn file_synopsis_cache_tracks_read_file_hits_and_entries() {
        let mut cache = FileSynopsisCache::default();
        let ok_read = TurnResult {
            text: "[tool:read_file:ok]\n[read_file:src/main.rs lines 1-4 of 100]\nuse std::fs;\nfn run() {}\n".to_string(),
            stop_reason: "tool_result".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            is_tool_result: true,
            turn_cost_usd: 0.0,
            total_cost_usd: 0.0,
            thinking: None,
            native_tool_calls: vec![],
        };
        cache_read_file_from_tool_result(&mut cache, &ok_read);
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.inserts, 1);
        let _ = cache.try_get("src/main.rs");
        assert_eq!(cache.hits, 1);
    }

    #[test]
    fn parse_confidence_level_accepts_standard_tokens() {
        assert_eq!(
            crate::orchestrator::confidence::parse_confidence_level("low"),
            Some(ConfidenceLevel::Low)
        );
        assert_eq!(
            crate::orchestrator::confidence::parse_confidence_level("medium"),
            Some(ConfidenceLevel::Medium)
        );
        assert_eq!(
            crate::orchestrator::confidence::parse_confidence_level("med"),
            Some(ConfidenceLevel::Medium)
        );
        assert_eq!(
            crate::orchestrator::confidence::parse_confidence_level("high"),
            Some(ConfidenceLevel::High)
        );
        assert_eq!(
            crate::orchestrator::confidence::parse_confidence_level("unknown"),
            None
        );
    }

    #[test]
    fn parse_confidence_declaration_reads_block() {
        let text = "Action plan:\n1. inspect\nConfidence Declaration:\nconfidence_level: low\nreason: model uncertainty after tool failures\n/toolcall read_file \"src/main.rs\" 1 40";
        let parsed = parse_confidence_declaration(text).unwrap();
        assert_eq!(parsed.level, ConfidenceLevel::Low);
        assert!(parsed.reason.contains("uncertainty"));
    }

    #[test]
    fn parse_confidence_declaration_requires_reason_and_level() {
        let missing_reason =
            "Confidence Declaration:\nconfidence_level: high\n/toolcall read_file \"src/main.rs\" 1 40";
        assert!(parse_confidence_declaration(missing_reason).is_none());
        let missing_level =
            "Confidence Declaration:\nreason: enough evidence\n/toolcall read_file \"src/main.rs\" 1 40";
        assert!(parse_confidence_declaration(missing_level).is_none());
    }

    #[test]
    fn confidence_gate_prompt_includes_expected_instructions() {
        let prompt = build_confidence_gate_prompt(
            "Context Contract:\nmode=strict",
            "/toolcall edit_file \"a.py\" \"x\" \"y\"",
            1,
            0,
        );
        assert!(prompt.contains("Confidence Gate"));
        assert!(prompt.contains("Confidence Declaration:"));
        assert!(prompt.contains("confidence_level: <low|medium|high>"));
        assert!(prompt.contains("read_file/glob_search/grep_search/web_search/web_fetch"));
    }

    #[test]
    fn confidence_low_block_reason_contains_policy_marker() {
        let reason = confidence_low_block_reason(None);
        assert!(reason.contains("blocked by user constraint"));
        assert!(reason.contains("strict confidence gate"));
        assert!(reason.contains("read-first actions"));
    }

    #[test]
    fn confidence_gate_stats_line_includes_all_counters() {
        let mut stats = ConfidenceGateStats::default();
        stats.checks = 3;
        stats.declaration_missing = 2;
        stats.declaration_low = 1;
        stats.blocked_risky_toolcalls = 4;
        stats.retries_exhausted = 1;
        let line = stats.stats_line();
        assert!(line.contains("checks=3"));
        assert!(line.contains("missing_declaration=2"));
        assert!(line.contains("low_declaration=1"));
        assert!(line.contains("blocked_risky_toolcalls=4"));
        assert!(line.contains("retries_exhausted=1"));
    }

    #[test]
    fn parse_toolcall_line_supports_json_function_wrapper() {
        let line = r#"/toolcall [{"type":"function","function":{"name":"glob_search","arguments":"{\"pattern\":\"*.py\"}"}}]"#;
        let parsed = parse_toolcall_line(line).expect("json wrapper should parse");
        assert_eq!(parsed.0, "glob_search");
        assert_eq!(parsed.1, "*.py");
    }

    #[test]
    fn parse_toolcall_line_supports_escaped_json_function_wrapper() {
        let line = r#"/toolcall [{\"type\":\"function\",\"function\":{\"name\":\"glob_search\",\"arguments\":\"{\\\"pattern\\\":\\\"*.py\\\"}\"}}]"#;
        let parsed = parse_toolcall_line(line).expect("escaped json wrapper should parse");
        assert_eq!(parsed.0, "glob_search");
        assert_eq!(parsed.1, "*.py");
    }

    #[test]
    fn parse_toolcall_line_supports_function_style_named_args() {
        let line =
            r#"/toolcall bash(command="Write-Output 'ok'; python --version 2>&1")"#;
        let parsed = parse_toolcall_line(line).expect("function style toolcall should parse");
        assert_eq!(parsed.0, "bash");
        assert!(parsed.1.contains("command=\""));
    }

    #[test]
    fn parse_file_path_arg_supports_key_value_file_path() {
        let got = super::parse_file_path_arg(
            r#"file_path="D:\test-cli\src\calcplus\core.py" content="x""#,
        )
        .expect("path");
        assert_eq!(got, r"D:\test-cli\src\calcplus\core.py");
    }

    #[test]
    fn zero_toolcall_nudge_detects_execution_ready_tool_lines() {
        let result = TurnResult {
            text: "/toolcall glob_search \"*.py\"".to_string(),
            stop_reason: "stop".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            is_tool_result: false,
            turn_cost_usd: 0.0,
            total_cost_usd: 0.0,
            thinking: None,
            native_tool_calls: vec![],
        };
        assert!(super::should_attempt_zero_toolcall_nudge(false, 0, &result));
    }

    #[test]
    fn zero_toolcall_nudge_allows_after_non_tool_followup_step() {
        let result = TurnResult {
            text: "1. Run /toolcall bash to print cwd\n2. Then finalize".to_string(),
            stop_reason: "stop".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            is_tool_result: false,
            turn_cost_usd: 0.0,
            total_cost_usd: 0.0,
            thinking: None,
            native_tool_calls: vec![],
        };
        assert!(super::should_attempt_zero_toolcall_nudge(false, 1, &result));
    }

    #[test]
    fn zero_toolcall_nudge_detects_toolcall_required_intent() {
        let result = TurnResult {
            text: "Use tool calls to finish this task.".to_string(),
            stop_reason: "stop".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            is_tool_result: false,
            turn_cost_usd: 0.0,
            total_cost_usd: 0.0,
            thinking: None,
            native_tool_calls: vec![],
        };
        assert!(super::should_attempt_zero_toolcall_nudge(false, 0, &result));
    }

    #[test]
    fn plan_preface_detector_ignores_toolcall_lines() {
        let text = "/toolcall bash \"echo hi\"";
        assert!(!super::looks_like_plan_preface_without_actions(text));
    }

}

#[cfg(test)]
mod autoresearch_repl_parse_tests {
    use super::{parse_autoresearch_repo_arg, parse_autoresearch_run_opts, parse_cli_tokens};
    use std::path::PathBuf;

    #[test]
    fn parse_cli_tokens_keeps_windows_path_in_quotes() {
        let tokens = parse_cli_tokens(
            "run --repo \"D:\\Code\\autoresearch\" --description \"baseline run\"",
        )
        .unwrap();
        assert_eq!(tokens[0], "run");
        assert_eq!(tokens[2], "D:\\Code\\autoresearch");
        assert_eq!(tokens[4], "baseline run");
    }

    #[test]
    fn parse_autoresearch_repo_arg_supports_positional_path() {
        let tokens = vec!["D:\\Code\\autoresearch".to_string()];
        let repo = parse_autoresearch_repo_arg(&tokens, "doctor").unwrap();
        assert_eq!(repo, PathBuf::from("D:\\Code\\autoresearch"));
    }

    #[test]
    fn parse_autoresearch_run_opts_parses_flags() {
        let tokens = vec![
            "--repo".to_string(),
            "D:\\Code\\autoresearch".to_string(),
            "--iterations".to_string(),
            "2".to_string(),
            "--timeout-secs=900".to_string(),
            "--log".to_string(),
            "results/run.log".to_string(),
            "--description".to_string(),
            "baseline run".to_string(),
            "--status".to_string(),
            "keep".to_string(),
        ];

        let opts = parse_autoresearch_run_opts(&tokens).unwrap();
        assert_eq!(opts.repo, PathBuf::from("D:\\Code\\autoresearch"));
        assert_eq!(opts.iterations, 2);
        assert_eq!(opts.timeout_secs, 900);
        assert_eq!(opts.log_path, Some(PathBuf::from("results/run.log")));
        assert_eq!(opts.description.as_deref(), Some("baseline run"));
        assert_eq!(opts.status.as_deref(), Some("keep"));
    }
}

#[cfg(test)]
mod tool_failure_tests {
    use super::{
        build_zero_toolcall_nudge_prompt, task_anchor_line,
        has_bilingual_markdown_reports, markdown_language_tag, requires_bilingual_markdown_reports,
        should_enforce_bilingual_markdown_reports, looks_like_task_encoding_loss,
        collect_plan_lines,
        build_confidence_gate_prompt, confidence_low_block_reason,
        classify_tool_failure_text, is_constraint_block_text, is_tool_failure,
        looks_like_fabricated_tool_result_output, looks_like_malformed_toolcall_output,
        looks_like_mixed_toolcall_and_result_output, planned_toolcall_count,
        build_review_json_only_prompt_output_with_options, build_review_json_only_prompt_output_with_strict, build_review_json_only_repl_output_with_strict,
        is_review_json_only_schema_invalid_envelope, should_fail_review_json_only_prompt_output_with_strict_exit,
        review_output_schema_error, should_enforce_review_output_schema,
        should_attempt_auto_output_repair, should_force_final_response_after_tool_result,
        should_require_plan_handshake,
        strict_followup_prompt_with_hint, tool_result_category_for_ok,
        ToolFailureCategory,
    };
    use crate::OrchestratorEngine;
    use crate::runtime::TurnResult;

    fn mk_tool_result(text: &str, stop_reason: &str) -> TurnResult {
        TurnResult {
            text: text.to_string(),
            stop_reason: stop_reason.to_string(),
            input_tokens: 0,
            output_tokens: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            is_tool_result: true,
            turn_cost_usd: 0.0,
            total_cost_usd: 0.0,
            thinking: None,
            native_tool_calls: vec![],
        }
    }

    #[test]
    fn command_not_found_bash_errors_are_recoverable() {
        let msg = "[tool:bash:error]\nyarn : The term 'yarn' is not recognized as the name of a cmdlet\nFullyQualifiedErrorId : CommandNotFoundException";
        let result = mk_tool_result(msg, "tool_error");
        assert!(!is_tool_failure(&result));
    }

    #[test]
    fn generic_tool_error_still_counts_as_failure() {
        let result = mk_tool_result("[tool:bash:error]\nexit code 1", "tool_error");
        assert!(is_tool_failure(&result));
    }

    #[test]
    fn permission_denied_still_counts_as_failure() {
        let result = mk_tool_result("Permission denied: bash", "permission_denied");
        assert!(is_tool_failure(&result));
    }

    #[test]
    fn classifier_detects_missing_command_errors() {
        let text = "[tool:bash:error]\nyarn : The term 'yarn' is not recognized as the name of a cmdlet";
        let kind = classify_tool_failure_text(text, "tool_error");
        assert_eq!(kind, ToolFailureCategory::CommandMissing);
    }

    #[test]
    fn classifier_detects_shell_syntax_errors() {
        let text = "[tool:bash:error]\nParserError: Unexpected token '||' in expression or statement";
        let kind = classify_tool_failure_text(text, "tool_error");
        assert_eq!(kind, ToolFailureCategory::ShellSyntax);
    }

    #[test]
    fn strict_followup_prompt_appends_failure_hint() {
        let prompt = strict_followup_prompt_with_hint(
            "base prompt",
            Some(ToolFailureCategory::Timeout),
        );
        assert!(prompt.contains("base prompt"));
        assert!(prompt.contains("Failure category"));
        assert!(prompt.contains("timeout"));
    }

    #[test]
    fn classifier_detects_user_constraint_blocks() {
        let text = "blocked by user constraint: request is run-only and does not allow file edits";
        let kind = classify_tool_failure_text(text, "tool_result");
        assert_eq!(kind, ToolFailureCategory::ConstraintBlock);
    }

    #[test]
    fn constraint_block_stop_reason_triggers_after_threshold() {
        let reason = super::constraint_block_stop_reason_if_needed(
            3,
            3,
            Some("blocked by user constraint: request is run-only and does not allow file edits"),
        );
        assert!(reason.is_some());
        let txt = reason.unwrap();
        assert!(txt.contains("repeated user-constraint blocks"));
        assert!(txt.contains("run-only/edit conflict"));
        assert!(txt.contains("threshold=3"));
    }

    #[test]
    fn constraint_block_streak_survives_non_constraint_failures() {
        let mut streak = 0usize;

        // blocked edit
        if is_constraint_block_text(
            "blocked by user constraint: request is run-only and does not allow file edits",
        ) {
            streak += 1;
        }

        // ordinary bash failure should not reset streak in current policy
        let non_constraint_failure = "[tool:bash:error]\nTraceback ... NameError";
        assert!(!is_constraint_block_text(non_constraint_failure));

        // blocked edit again -> reaches threshold 2
        if is_constraint_block_text(
            "blocked by user constraint: request is run-only and does not allow file edits",
        ) {
            streak += 1;
        }

        let reason = super::constraint_block_stop_reason_if_needed(
            streak,
            2,
            Some("blocked by user constraint: request is run-only and does not allow file edits"),
        );
        assert!(reason.is_some());
    }

    #[test]
    fn malformed_detector_handles_tool_call_wrapper_codeblock() {
        let text = "I'll start by exploring the project.\n```python\ntool_call(\"glob_search\", {\"pattern\": \"**/*\"})\n```";
        assert!(looks_like_malformed_toolcall_output(text));
    }

    #[test]
    fn malformed_detector_handles_xml_style_tool_tags() {
        let text = "<file_write path=\"D:\\\\test-cli\\\\x.py\">\nprint('x')\n</file_write>";
        assert!(looks_like_malformed_toolcall_output(text));
    }

    #[test]
    fn malformed_detector_handles_tool_call_xml_wrapper() {
        let text = "<tool_call>\n<tool_name>glob_search</tool_name>\n<tool_arg>**/Cargo.toml</tool_arg>\n</tool_call>";
        assert!(looks_like_malformed_toolcall_output(text));
    }

    #[test]
    fn malformed_detector_handles_function_calls_wrapper() {
        let text = "<function-calls>\n<function-call>\n<invoke name=\"glob_search\">\n<parameter name=\"pattern\">**/*.rs</parameter>\n</invoke>\n</function-call>\n</function-calls>";
        assert!(looks_like_malformed_toolcall_output(text));
    }

    #[test]
    fn malformed_detector_does_not_fire_on_plain_valid_toolcall_line() {
        let text = "/toolcall glob_search \"**/*\"";
        assert!(!looks_like_malformed_toolcall_output(text));
    }

    #[test]
    fn malformed_detector_does_not_fire_on_plain_tool_name_mention() {
        let text = "If needed, I can use glob_search later.";
        assert!(!looks_like_malformed_toolcall_output(text));
    }

    #[test]
    fn execution_ready_detector_handles_xml_style_lines() {
        assert!(super::looks_like_execution_ready_line(
            "<glob_search pattern=\"**/*.py\">"
        ));
        assert!(super::looks_like_execution_ready_line(
            "<file_write path=\"a.py\">"
        ));
        assert!(super::looks_like_execution_ready_line("<tool_call>"));
        assert!(super::looks_like_execution_ready_line("<function-calls>"));
        assert!(super::looks_like_execution_ready_line(
            "<invoke name=\"bash\">"
        ));
    }

    #[test]
    fn mixed_toolcall_and_result_detector_flags_replayed_blocks() {
        let text = "/toolcall bash \"python --version\"\n---\n[tool:bash:ok]\n{\"stdout\":\"Python 3.13.0\"}\n---";
        assert!(looks_like_mixed_toolcall_and_result_output(text));
    }

    #[test]
    fn mixed_toolcall_and_result_detector_ignores_plain_toolcalls() {
        let text = "/toolcall glob_search \"**/*.py\"\n/toolcall read_file \"main.py\" 1 240";
        assert!(!looks_like_mixed_toolcall_and_result_output(text));
    }

    #[test]
    fn fabricated_tool_result_detector_flags_replayed_blocks() {
        let text =
            "[tool:read_file:ok]\n[read_file:main.py lines 1-20 of 74]\ndef add(a, b):\n    return a + b";
        assert!(looks_like_fabricated_tool_result_output(text));
    }

    #[test]
    fn fabricated_tool_result_detector_ignores_plain_explanations() {
        let text = "I can use read_file if you want, but I have not executed any tool yet.";
        assert!(!looks_like_fabricated_tool_result_output(text));
    }

    #[test]
    fn auto_output_repair_gate_fires_for_mixed_or_fabricated_output() {
        let mixed = "/toolcall bash \"python --version\"\n[tool:bash:ok]\n{\"stdout\":\"Python 3.13.0\"}";
        assert!(should_attempt_auto_output_repair(false, mixed));

        let fabricated = "[tool:read_file:ok]\n[read_file:main.py lines 1-4 of 100]\nhello";
        assert!(should_attempt_auto_output_repair(false, fabricated));

        let clean = "/toolcall glob_search \"*.py\"";
        assert!(!should_attempt_auto_output_repair(false, clean));
        assert!(!should_attempt_auto_output_repair(true, mixed));
    }

    #[test]
    fn auto_output_repair_max_attempts_constant_is_three() {
        assert_eq!(super::AUTO_OUTPUT_REPAIR_MAX_ATTEMPTS, 3);
    }

    #[test]
    fn zero_toolcall_nudge_prompt_includes_task_anchor() {
        let prompt = build_zero_toolcall_nudge_prompt(
            "Context Contract:\nmode=standard",
            "analyze workspace and write chinese+english report files",
            false,
        );
        assert!(prompt.contains("Primary task (do not drift):"));
        assert!(prompt.contains("analyze workspace"));
    }

    #[test]
    fn task_anchor_line_flattens_whitespace() {
        let line = task_anchor_line("analyze\nworkspace\tand report");
        assert!(line.contains("Primary task (do not drift):"));
        assert!(!line.contains('\n'));
        assert!(!line.contains('\t'));
    }

    #[test]
    fn markdown_language_tag_detects_expected_suffixes() {
        assert_eq!(markdown_language_tag("analysis_zh.md"), Some("zh"));
        assert_eq!(markdown_language_tag("analysis-en.md"), Some("en"));
        assert_eq!(markdown_language_tag("report.md"), None);
        assert_eq!(markdown_language_tag("analysis_zh.txt"), None);
    }

    #[test]
    fn bilingual_markdown_helpers_enforce_until_both_files_exist() {
        let task = "Analyze current directory and write Chinese and English .md reports";
        assert!(requires_bilingual_markdown_reports(task));

        let none: Vec<String> = Vec::new();
        assert!(should_enforce_bilingual_markdown_reports(task, &none));

        let only_zh = vec!["analysis_zh.md".to_string()];
        assert!(!has_bilingual_markdown_reports(&only_zh));
        assert!(should_enforce_bilingual_markdown_reports(task, &only_zh));

        let both = vec!["analysis_zh.md".to_string(), "analysis_en.md".to_string()];
        assert!(has_bilingual_markdown_reports(&both));
        assert!(!should_enforce_bilingual_markdown_reports(task, &both));
    }

    #[test]
    fn requires_bilingual_markdown_reports_handles_encoding_loss_markers() {
        let lossy = "???????????????????.md???";
        assert!(looks_like_task_encoding_loss(lossy));
        assert!(requires_bilingual_markdown_reports(lossy));
    }

    #[test]
    fn planned_toolcall_count_detects_toolcalls_in_mixed_text() {
        let text = "/toolcall glob_search \"*.py\"\n---\n{\"results\":[]}\n/toolcall read_file \"main.py\" 1 20";
        assert_eq!(planned_toolcall_count(text), 2);
    }

    #[test]
    fn tool_result_ok_category_defaults_to_ok() {
        let result = mk_tool_result("[tool:read_file:ok]\nhello", "tool_result");
        assert_eq!(tool_result_category_for_ok(&result), "ok");
    }

    #[test]
    fn review_schema_validator_accepts_expected_structure() {
        let ok = "Findings:\n- [HIGH] src/main.rs:12 - bug summary\n  Evidence: trace\n  Risk: crash\nMissing Tests:\n- add regression test for parser\nOpen Questions:\n- None\nSummary:\n- fixed root cause";
        assert!(review_output_schema_error(ok).is_none());
        assert!(!should_enforce_review_output_schema("/work inspect", ok));
        assert!(!should_enforce_review_output_schema("/review parser", ok));

        let generated_review_prompt =
            "Code review task in the current local project.\n\nWorkspace snapshot:\n...\n\nTask:\ninspect parser regressions";
        let bad = "Findings:\n- item only";
        assert!(should_enforce_review_output_schema(generated_review_prompt, bad));
    }

    #[test]
    fn review_schema_validator_reports_missing_sections() {
        let bad = "Findings:\n- item\nSummary:\n- done";
        let err = review_output_schema_error(bad).expect("must fail");
        assert!(err.contains("missing sections"));
        assert!(should_enforce_review_output_schema("/review parser", bad));
    }

    #[test]
    fn review_schema_validator_requires_order_and_non_empty_sections() {
        let bad_order =
            "Summary:\n- done\nFindings:\n- issue\nMissing Tests:\n- None\nOpen Questions:\n- None";
        let err = review_output_schema_error(bad_order).expect("must fail");
        assert!(err.contains("section order"));

        let empty_section =
            "Findings:\n- issue\nMissing Tests:\n\nOpen Questions:\n- None\nSummary:\n- done";
        let err2 = review_output_schema_error(empty_section).expect("must fail");
        assert!(err2.contains("Missing Tests"));
    }

    #[test]
    fn build_review_json_payload_marks_non_review_tasks() {
        let payload = super::build_review_json_payload("/work fix parser", "hello");
        assert_eq!(
            payload
                .get("schema_version")
                .and_then(|v| v.as_str()),
            Some(super::REVIEW_JSON_SCHEMA_VERSION)
        );
        assert_eq!(
            payload
                .get("is_review_task")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            false
        );
        assert_eq!(
            payload
                .get("schema_valid")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            false
        );
        assert!(payload.get("sections").is_some());
        assert!(payload.get("sections").unwrap().is_null());
        assert!(payload.get("stats").is_some());
        assert!(payload.get("stats").unwrap().is_null());
    }

    #[test]
    fn build_review_json_payload_extracts_structured_sections_when_valid() {
        let text = "Findings:\n- [HIGH] src/main.rs:12 - bug summary\n  Evidence: trace\n  Risk: crash\nMissing Tests:\n- add regression test\nOpen Questions:\n- None\nSummary:\n- fixed root cause";
        let payload = super::build_review_json_payload("/review parser", text);
        assert_eq!(
            payload
                .get("schema_version")
                .and_then(|v| v.as_str()),
            Some(super::REVIEW_JSON_SCHEMA_VERSION)
        );
        assert_eq!(
            payload
                .get("is_review_task")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            true
        );
        assert_eq!(
            payload
                .get("schema_valid")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            true
        );
        let sections = payload.get("sections").unwrap();
        let findings = sections
            .get("findings")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(!findings.is_empty());
        let findings_sorted = sections
            .get("findings_sorted")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(findings_sorted.len(), findings.len());
        let first = findings[0].as_object().expect("structured finding object");
        assert_eq!(
            first.get("severity").and_then(|v| v.as_str()),
            Some("HIGH")
        );
        assert_eq!(
            first.get("normalized_severity").and_then(|v| v.as_str()),
            Some("high")
        );
        assert_eq!(
            first.get("path").and_then(|v| v.as_str()),
            Some("src/main.rs")
        );
        assert_eq!(first.get("line").and_then(|v| v.as_u64()), Some(12));
        assert_eq!(
            first.get("location").and_then(|v| v.as_str()),
            Some("src/main.rs:12")
        );
        assert_eq!(
            first.get("summary").and_then(|v| v.as_str()),
            Some("bug summary")
        );
        assert_eq!(
            first.get("evidence").and_then(|v| v.as_str()),
            Some("trace")
        );
        assert_eq!(first.get("risk").and_then(|v| v.as_str()), Some("crash"));
        let findings_raw = sections
            .get("findings_raw")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(!findings_raw.is_empty());
        let summary = sections
            .get("summary")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(summary.len(), 1);
    }

    #[test]
    fn structured_findings_parser_supports_multiple_finding_blocks() {
        let text = "Findings:\n- [MEDIUM] src/lib.rs:7 - first issue\n  Evidence: repro A\n  Risk: medium impact\n- [LOW] src/mod.rs - second issue\n  Evidence: repro B\nMissing Tests:\n- add tests\nOpen Questions:\n- None\nSummary:\n- done";
        let payload = super::build_review_json_payload("/review multi", text);
        let findings = payload
            .get("sections")
            .and_then(|v| v.get("findings"))
            .and_then(|v| v.as_array())
            .expect("findings array");
        assert_eq!(findings.len(), 2);

        let f1 = findings[0].as_object().unwrap();
        assert_eq!(
            f1.get("severity").and_then(|v| v.as_str()),
            Some("MEDIUM")
        );
        assert_eq!(
            f1.get("normalized_severity").and_then(|v| v.as_str()),
            Some("medium")
        );
        assert_eq!(
            f1.get("path").and_then(|v| v.as_str()),
            Some("src/lib.rs")
        );
        assert_eq!(f1.get("line").and_then(|v| v.as_u64()), Some(7));
        assert_eq!(
            f1.get("location").and_then(|v| v.as_str()),
            Some("src/lib.rs:7")
        );

        let f2 = findings[1].as_object().unwrap();
        assert_eq!(f2.get("line").and_then(|v| v.as_u64()), None);
        assert_eq!(
            f2.get("path").and_then(|v| v.as_str()),
            Some("src/mod.rs")
        );
        assert_eq!(
            f2.get("location").and_then(|v| v.as_str()),
            Some("src/mod.rs")
        );

        let sorted = payload
            .get("sections")
            .and_then(|v| v.get("findings_sorted"))
            .and_then(|v| v.as_array())
            .expect("sorted findings");
        assert_eq!(sorted.len(), 2);
        assert_eq!(
            sorted[0]
                .get("normalized_severity")
                .and_then(|v| v.as_str()),
            Some("medium")
        );
        assert_eq!(
            sorted[1]
                .get("normalized_severity")
                .and_then(|v| v.as_str()),
            Some("low")
        );
    }

    #[test]
    fn review_stats_value_counts_severity_buckets() {
        let text = "Findings:\n- [CRITICAL] src/a.rs:1 - crash\n  Evidence: e1\n  Risk: r1\n- [HIGH] src/a.rs:3 - wrong\n  Evidence: e2\n  Risk: r2\n- [MEDIUM] src/c.rs:3 - edge\n  Evidence: e3\n  Risk: r3\n- [LOW] src/d.rs - docs\n  Evidence: e4\n- [UNKNOWN] src/e.rs - odd\nMissing Tests:\n- add tests\n- None\nOpen Questions:\n- None\nSummary:\n- done";
        let payload = super::build_review_json_payload("/review stats", text);
        let stats = payload.get("stats").expect("stats");
        assert_eq!(stats.get("has_findings").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(stats.get("total_findings").and_then(|v| v.as_u64()), Some(5));
        let sev = stats.get("severity_counts").expect("severity_counts");
        assert_eq!(sev.get("critical").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(sev.get("high").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(sev.get("medium").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(sev.get("low").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(sev.get("unknown").and_then(|v| v.as_u64()), Some(1));
        assert_eq!(stats.get("risk_score_total").and_then(|v| v.as_u64()), Some(16));
        assert_eq!(
            stats.get("missing_tests_count").and_then(|v| v.as_u64()),
            Some(1)
        );
        let top = stats
            .get("top_risk_paths")
            .and_then(|v| v.as_array())
            .expect("top risk paths");
        assert!(!top.is_empty());
        assert_eq!(top[0].get("path").and_then(|v| v.as_str()), Some("src/a.rs"));
        assert_eq!(top[0].get("risk_score").and_then(|v| v.as_u64()), Some(12));
        assert_eq!(top[0].get("findings").and_then(|v| v.as_u64()), Some(2));
    }

    #[test]
    fn review_stats_value_uses_configurable_risk_weights() {
        let sections = serde_json::json!({
            "findings": [
                {"normalized_severity":"critical","path":"src/a.rs"},
                {"normalized_severity":"high","path":"src/a.rs"},
                {"normalized_severity":"high","path":"src/b.rs"}
            ],
            "missing_tests": ["- add regression"]
        });
        let stats = super::review_stats_value_with_weights(
            Some(&sections),
            super::ReviewRiskWeights {
                critical: 100,
                high: 10,
                medium: 2,
                low: 1,
                unknown: 1,
            },
        );
        let top = stats
            .get("top_risk_paths")
            .and_then(|v| v.as_array())
            .expect("top risk paths");
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].get("path").and_then(|v| v.as_str()), Some("src/a.rs"));
        assert_eq!(top[0].get("risk_score").and_then(|v| v.as_u64()), Some(110));
        assert_eq!(stats.get("risk_score_total").and_then(|v| v.as_u64()), Some(120));
        assert_eq!(
            stats.get("missing_tests_count").and_then(|v| v.as_u64()),
            Some(1)
        );
    }

    #[test]
    fn review_json_only_repl_output_strict_schema_gates_invalid_payload() {
        let review_payload = super::build_review_json_payload(
            "/review parser",
            "Findings:\n- only one section",
        );
        let non_strict = build_review_json_only_repl_output_with_strict(&review_payload, false);
        assert!(non_strict.get("review").is_some());
        assert_eq!(
            non_strict
                .get("schema_version")
                .and_then(|v| v.as_str()),
            Some(super::REVIEW_JSON_SCHEMA_VERSION)
        );
        assert_eq!(non_strict.get("status").and_then(|v| v.as_str()), Some("ok"));

        let strict = build_review_json_only_repl_output_with_strict(&review_payload, true);
        assert_eq!(
            strict
                .get("schema_version")
                .and_then(|v| v.as_str()),
            Some(super::REVIEW_JSON_SCHEMA_VERSION)
        );
        assert_eq!(strict.get("status").and_then(|v| v.as_str()), Some("error"));
        assert_eq!(
            strict.get("error").and_then(|v| v.as_str()),
            Some("review_schema_invalid")
        );
        let schema_error = strict
            .get("schema_error")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(schema_error.contains("missing sections"));
        assert_eq!(
            strict
                .get("review")
                .and_then(|v| v.get("schema_valid"))
                .and_then(|v| v.as_bool()),
            Some(false)
        );
        assert!(strict.get("review").is_some());
    }

    #[test]
    fn review_json_only_prompt_output_strict_schema_gates_invalid_payload() {
        let review_payload = super::build_review_json_payload(
            "/review parser",
            "Findings:\n- only one section",
        );
        let non_strict = build_review_json_only_prompt_output_with_strict(&review_payload, false);
        assert_eq!(
            non_strict
                .get("schema_valid")
                .and_then(|v| v.as_bool()),
            Some(false)
        );
        assert!(non_strict.get("review").is_none());

        let strict = build_review_json_only_prompt_output_with_strict(&review_payload, true);
        assert_eq!(
            strict
                .get("schema_version")
                .and_then(|v| v.as_str()),
            Some(super::REVIEW_JSON_SCHEMA_VERSION)
        );
        assert_eq!(strict.get("status").and_then(|v| v.as_str()), Some("error"));
        assert_eq!(
            strict.get("error").and_then(|v| v.as_str()),
            Some("review_schema_invalid")
        );
        let schema_error = strict
            .get("schema_error")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(schema_error.contains("missing sections"));
        assert_eq!(
            strict
                .get("review")
                .and_then(|v| v.get("schema_valid"))
                .and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn review_json_only_prompt_output_can_wrap_success_envelope() {
        let review_payload = super::build_review_json_payload(
            "/review parser",
            "Findings:\n- [LOW] src/a.rs:1 - ok\nMissing Tests:\n- None\nOpen Questions:\n- None\nSummary:\n- done",
        );
        let wrapped =
            build_review_json_only_prompt_output_with_options(&review_payload, false, true);
        assert_eq!(
            wrapped.get("schema_version").and_then(|v| v.as_str()),
            Some(super::REVIEW_JSON_SCHEMA_VERSION)
        );
        assert_eq!(wrapped.get("status").and_then(|v| v.as_str()), Some("ok"));
        assert!(wrapped.get("review").is_some());

        let unwrapped =
            build_review_json_only_prompt_output_with_options(&review_payload, false, false);
        assert!(unwrapped.get("review").is_none());
        assert_eq!(
            unwrapped
                .get("schema_version")
                .and_then(|v| v.as_str()),
            Some(super::REVIEW_JSON_SCHEMA_VERSION)
        );
    }

    #[test]
    fn review_json_only_schema_invalid_envelope_detector_works() {
        let review_payload = super::build_review_json_payload(
            "/review parser",
            "Findings:\n- only one section",
        );
        let strict = build_review_json_only_repl_output_with_strict(&review_payload, true);
        let non_strict = build_review_json_only_repl_output_with_strict(&review_payload, false);
        assert!(is_review_json_only_schema_invalid_envelope(&strict));
        assert!(!is_review_json_only_schema_invalid_envelope(&non_strict));
    }

    #[test]
    fn review_json_response_keeps_schema_and_status_with_payload_fields() {
        let out = super::review_json_response(
            "ok",
            serde_json::json!({
                "review": {
                    "schema_version": super::REVIEW_JSON_SCHEMA_VERSION
                },
                "extra": 1
            }),
        );
        assert_eq!(
            out.get("schema_version").and_then(|v| v.as_str()),
            Some(super::REVIEW_JSON_SCHEMA_VERSION)
        );
        assert_eq!(out.get("status").and_then(|v| v.as_str()), Some("ok"));
        assert!(out.get("review").is_some());
        assert_eq!(out.get("extra").and_then(|v| v.as_i64()), Some(1));
    }

    #[test]
    fn review_json_only_prompt_fail_exit_toggle_works() {
        let review_payload = super::build_review_json_payload(
            "/review parser",
            "Findings:\n- only one section",
        );
        let strict = build_review_json_only_prompt_output_with_strict(&review_payload, true);
        let non_strict = build_review_json_only_prompt_output_with_strict(&review_payload, false);
        assert!(should_fail_review_json_only_prompt_output_with_strict_exit(
            &strict, true
        ));
        assert!(!should_fail_review_json_only_prompt_output_with_strict_exit(
            &strict, false
        ));
        assert!(!should_fail_review_json_only_prompt_output_with_strict_exit(
            &non_strict,
            true
        ));
    }

    #[test]
    fn findings_sorted_orders_by_severity_then_location() {
        let text = "Findings:\n- [LOW] src/z.rs:7 - late\n- [CRITICAL] src/a.rs:2 - first\n- [HIGH] src/b.rs:1 - second\nMissing Tests:\n- None\nOpen Questions:\n- None\nSummary:\n- done";
        let payload = super::build_review_json_payload("/review sort", text);
        let sorted = payload
            .get("sections")
            .and_then(|v| v.get("findings_sorted"))
            .and_then(|v| v.as_array())
            .expect("sorted findings");
        assert_eq!(sorted.len(), 3);
        assert_eq!(
            sorted[0]
                .get("normalized_severity")
                .and_then(|v| v.as_str()),
            Some("critical")
        );
        assert_eq!(
            sorted[1]
                .get("normalized_severity")
                .and_then(|v| v.as_str()),
            Some("high")
        );
        assert_eq!(
            sorted[2]
                .get("normalized_severity")
                .and_then(|v| v.as_str()),
            Some("low")
        );
    }

    #[test]
    fn normalize_review_severity_supports_common_aliases() {
        assert_eq!(
            super::normalize_review_severity(Some("CRITICAL")),
            Some("critical".to_string())
        );
        assert_eq!(
            super::normalize_review_severity(Some("p1")),
            Some("high".to_string())
        );
        assert_eq!(
            super::normalize_review_severity(Some("med")),
            Some("medium".to_string())
        );
        assert_eq!(
            super::normalize_review_severity(Some("L")),
            Some("low".to_string())
        );
        assert_eq!(super::normalize_review_severity(Some("unknown")), None);
    }

    #[test]
    fn build_review_json_payload_sets_schema_error_when_invalid() {
        let payload = super::build_review_json_payload(
            "/review parser",
            "Findings:\n- only one section",
        );
        assert_eq!(
            payload
                .get("schema_valid")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            false
        );
        let err = payload
            .get("schema_error")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(err.contains("missing sections"));
    }

    #[test]
    fn finalize_gate_triggers_for_tool_result_and_tool_use_alias() {
        let mut tool_result = mk_tool_result("[tool:glob_search:ok]\n...", "tool_result");
        assert!(should_force_final_response_after_tool_result(&tool_result));

        tool_result.stop_reason = "tool_use".to_string();
        assert!(should_force_final_response_after_tool_result(&tool_result));

        tool_result.stop_reason = "stop".to_string();
        tool_result.is_tool_result = false;
        assert!(!should_force_final_response_after_tool_result(&tool_result));
    }

    #[test]
    fn collect_plan_lines_extracts_numbered_and_bulleted_items() {
        let text = "Action plan:\n1. inspect files\n2. run checks\n- patch code\n/toolcall read_file \"a\" 1 20";
        let got = collect_plan_lines(text);
        assert_eq!(got.len(), 3);
        assert!(got[0].starts_with("1."));
        assert!(got[2].starts_with("- "));
    }

    #[test]
    fn collect_plan_lines_extracts_plain_numbered_steps() {
        let text = "1. run grep scan\n2. inspect results\n3. output marker";
        let got = collect_plan_lines(text);
        assert_eq!(got.len(), 3);
        assert!(got.iter().all(|line| line.starts_with(char::is_numeric)));
    }

    #[test]
    fn plan_handshake_gate_only_triggers_for_first_strict_complex_step() {
        assert!(should_require_plan_handshake(true, "/toolcall a\n/toolcall b", 2, 0));
        assert!(should_require_plan_handshake(true, "/toolcall a", 1, 0));
        assert!(!should_require_plan_handshake(false, "/toolcall a\n/toolcall b", 2, 0));
        assert!(!should_require_plan_handshake(true, "no tools", 0, 0));
        assert!(!should_require_plan_handshake(true, "/toolcall a\n/toolcall b", 2, 1));
        assert!(!should_require_plan_handshake(
            true,
            "Action plan:\n1. x\n/toolcall a\n/toolcall b",
            2,
            0
        ));
    }

    #[test]
    fn confidence_gate_prompt_mentions_detected_risky_calls() {
        let prompt = build_confidence_gate_prompt(
            "Context Contract:\nmode=strict",
            "/toolcall bash \"python train.py\"",
            1,
            2,
        );
        assert!(prompt.contains("detected 1 risky toolcall(s)"));
        assert!(prompt.contains("2 non-risky toolcall(s)"));
    }

    #[test]
    fn confidence_low_block_reason_is_constraint_text() {
        let reason = confidence_low_block_reason(None);
        assert!(is_constraint_block_text(&reason));
    }

    #[test]
    fn non_strict_auto_loop_uses_higher_failure_tolerance() {
        let step_budget = 128usize;
        let strict_engine = OrchestratorEngine::new(step_budget, 4);
        let non_strict_engine = OrchestratorEngine::new(step_budget, 4);
        assert_eq!(strict_engine.stop_reason(0, 3), None);
        assert_eq!(non_strict_engine.stop_reason(0, 3), None);
        assert_eq!(
            strict_engine.stop_reason(0, 4),
            Some("repeated tool failures")
        );
        assert_eq!(
            non_strict_engine.stop_reason(0, 4),
            Some("repeated tool failures")
        );
    }
}
