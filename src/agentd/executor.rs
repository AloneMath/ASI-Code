use std::env;
use std::path::{Path, PathBuf};

use crate::config::AppConfig;
use crate::orchestrator::engine::OrchestratorEngine;
use crate::runtime::Runtime;
use crate::tools::run_tool;

use super::types::{JobRecord, JobResult};

#[derive(Debug, Default)]
pub struct Executor;

impl Executor {
    pub fn execute(&self, job: &JobRecord) -> Result<JobResult, String> {
        let project = Path::new(&job.spec.project_path);
        if !project.exists() {
            return Err(format!(
                "project path does not exist: {}",
                job.spec.project_path
            ));
        }

        if uses_runtime_turn(job) {
            return self.execute_runtime_turn(job);
        }

        let mut logs = Vec::new();
        logs.push("mode=scaffold".to_string());
        logs.push(format!("goal={}", job.spec.goal));
        logs.push(format!("project={}", job.spec.project_path));
        if !job.spec.constraints.is_empty() {
            logs.push(format!("constraints={}", job.spec.constraints.join(",")));
        }

        let summary = format!(
            "Job {} executed in scaffold mode. Add constraint runtime_turn to run a model-driven multi-step loop.",
            job.id
        );

        Ok(JobResult {
            summary,
            artifacts: Vec::new(),
            changed_files: Vec::new(),
            logs,
            cost_usd: 0.0,
        })
    }

    fn execute_runtime_turn(&self, job: &JobRecord) -> Result<JobResult, String> {
        let old_cwd = env::current_dir().map_err(|e| format!("current_dir: {}", e))?;
        env::set_current_dir(&job.spec.project_path)
            .map_err(|e| format!("set_current_dir {}: {}", job.spec.project_path, e))?;

        let run_result = (|| -> Result<JobResult, String> {
            let cfg = AppConfig::load();
            let mut rt = Runtime::new(
                cfg.provider.clone(),
                cfg.model.clone(),
                cfg.permission_mode.clone(),
                cfg.max_turns,
            );
            apply_runtime_flags_from_cfg(&mut rt, &cfg);
            rt.extended_thinking = cfg.extended_thinking;

            let max_steps = constraint_usize(job, "max_steps=")
                .unwrap_or(8)
                .clamp(1, 64);
            let max_failures = constraint_usize(job, "max_failures=")
                .unwrap_or(2)
                .clamp(1, 10);
            let engine = OrchestratorEngine::new(max_steps, max_failures);

            let mut logs = Vec::new();
            logs.push("mode=runtime_turn".to_string());
            logs.push(format!("provider={}", cfg.provider));
            logs.push(format!("model={}", cfg.model));
            logs.push(format!("max_steps={}", max_steps));
            logs.push(format!("max_failures={}", max_failures));

            let constraints = if job.spec.constraints.is_empty() {
                "<none>".to_string()
            } else {
                job.spec.constraints.join(",")
            };

            let mut prompt = format!(
                "You are executing an agentd background job.\nProject: {}\nGoal: {}\nConstraints: {}\n\nRules:\n1) If local actions are needed, output /toolcall lines only.\n2) Batch tool calls when possible.\n3) When done, provide the final summary.",
                job.spec.project_path, job.spec.goal, constraints
            );

            let mut step_count = 0usize;
            let mut consecutive_failures = 0usize;
            let mut changed_files = Vec::new();
            let mut final_answer = String::new();

            'outer: loop {
                if let Some(reason) = engine.stop_reason(step_count, consecutive_failures) {
                    logs.push(format!("stop={}", reason));
                    break;
                }

                let turn = rt.run_turn(&prompt);
                logs.push(format!("turn_stop_reason={}", turn.stop_reason));
                logs.push(format!("turn_input_tokens={}", turn.input_tokens));
                logs.push(format!("turn_output_tokens={}", turn.output_tokens));

                if turn.stop_reason == "provider_error" {
                    return Err(format!("provider_error: {}", turn.text));
                }

                final_answer = turn.text.clone();
                let batches = engine.parse_and_plan(&turn.text);
                if batches.is_empty() {
                    break;
                }

                let mut executed = 0usize;
                let mut tool_outputs = Vec::new();

                for batch in batches {
                    for call in batch.calls {
                        if let Some(reason) = engine.stop_reason(step_count, consecutive_failures) {
                            logs.push(format!("stop={}", reason));
                            break 'outer;
                        }

                        step_count += 1;
                        executed += 1;

                        let result = run_tool(&call.name, &call.args);
                        if result.ok {
                            consecutive_failures = 0;
                        } else {
                            consecutive_failures += 1;
                        }

                        collect_changed_files(&result.output, &mut changed_files);
                        logs.push(format!(
                            "tool={} ok={} step={}",
                            call.name, result.ok, step_count
                        ));

                        let output = truncate_text(&result.output, 5000);
                        tool_outputs.push(format!(
                            "COMMAND: {}\nSTATUS: {}\n{}",
                            call.to_command(),
                            if result.ok { "ok" } else { "error" },
                            output
                        ));
                    }
                }

                if executed == 0 {
                    break;
                }

                prompt = format!(
                    "{}\n\nTool outputs:\n{}",
                    engine.followup_prompt(),
                    tool_outputs.join("\n\n---\n\n")
                );
            }

            if final_answer.trim().is_empty() {
                final_answer = "Job finished without assistant summary.".to_string();
            }

            Ok(JobResult {
                summary: final_answer,
                artifacts: Vec::new(),
                changed_files,
                logs,
                cost_usd: rt.cumulative_cost_usd,
            })
        })();

        let restore = env::set_current_dir(&old_cwd)
            .map_err(|e| format!("restore cwd {}: {}", old_cwd.display(), e));

        match (run_result, restore) {
            (Ok(v), Ok(())) => Ok(v),
            (Err(e), Ok(())) => Err(e),
            (Ok(_), Err(e)) => Err(e),
            (Err(run_err), Err(restore_err)) => Err(format!(
                "{}; additionally failed to restore cwd: {}",
                run_err, restore_err
            )),
        }
    }
}

fn uses_runtime_turn(job: &JobRecord) -> bool {
    job.spec
        .constraints
        .iter()
        .map(|c| c.trim().to_ascii_lowercase())
        .any(|c| c == "runtime_turn" || c == "runtime")
}

fn constraint_usize(job: &JobRecord, prefix: &str) -> Option<usize> {
    let prefix_l = prefix.to_ascii_lowercase();
    for raw in &job.spec.constraints {
        let v = raw.trim().to_ascii_lowercase();
        if let Some(rest) = v.strip_prefix(&prefix_l) {
            if let Ok(n) = rest.parse::<usize>() {
                return Some(n);
            }
        }
    }
    None
}

fn apply_runtime_flags_from_cfg(rt: &mut Runtime, cfg: &AppConfig) {
    rt.disable_web_tools = cfg.is_feature_disabled("web_tools");
    rt.disable_bash_tool = cfg.is_feature_disabled("bash_tool");
    rt.safe_shell_mode = cfg.safe_shell_mode;
    rt.permission_allow_rules = cfg.permission_allow_rules.clone();
    rt.permission_deny_rules = cfg.permission_deny_rules.clone();
    rt.path_restriction_enabled = cfg.path_restriction_enabled;
    rt.workspace_root = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    rt.additional_directories = cfg
        .additional_directories
        .iter()
        .map(PathBuf::from)
        .collect();
    rt.native_tool_calling = matches!(rt.provider.as_str(), "claude");
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let mut out = text[..max_chars].to_string();
    out.push_str("\n... [truncated]");
    out
}

fn collect_changed_files(output: &str, changed_files: &mut Vec<String>) {
    for line in output.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("changed_file=") {
            push_unique_changed(changed_files, v.trim_matches('"').trim());
            continue;
        }
        if let Some(v) = t.strip_prefix("Edited ") {
            push_unique_changed(changed_files, v.trim());
            continue;
        }
        if let Some(idx) = t.find(" chars to ") {
            if t.starts_with("Wrote ") {
                let path = &t[idx + " chars to ".len()..];
                push_unique_changed(changed_files, path.trim());
            }
        }
    }
}

fn push_unique_changed(changed_files: &mut Vec<String>, value: &str) {
    if value.is_empty() {
        return;
    }
    if changed_files.iter().any(|v| v == value) {
        return;
    }
    changed_files.push(value.to_string());
}
