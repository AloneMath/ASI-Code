pub(crate) fn dispatch(command: crate::Command) -> Result<(), String> {
    match command {
        crate::Command::Gateway {
            listen,
            provider,
            model,
            permission_mode,
            project,
        } => {
            crate::gateway::serve(listen, provider, model, permission_mode, project)
        }
        crate::Command::Version => {
            println!("{} {}", crate::meta::APP_NAME, crate::meta::APP_VERSION);
            Ok(())
        }
        crate::Command::SelfUpdate {
            source,
            sha256,
            restart,
        } => {
            let msg = crate::self_update::run_self_update(&source, sha256.as_deref(), restart)?;
            println!("{}", msg);
            Ok(())
        }
        crate::Command::Bench {
            provider,
            model,
            permission_mode,
            project,
            agent_max_steps,
            repeat,
            suite,
            out_dir,
        } => {
            let msg = crate::bench::run(crate::bench::BenchOptions {
                provider,
                model,
                permission_mode,
                project,
                agent_max_steps,
                repeat,
                suite,
                out_dir,
            })?;
            println!("{}", msg);
            Ok(())
        }
        crate::Command::Config {
            provider,
            model,
            permission_mode,
            auto_review_mode,
            auto_review_severity_threshold,
            execution_speed,
            max_turns,
            extended_thinking,
            markdown_render,
            theme,
            telemetry_enabled,
            telemetry_log_tool_details,
            undercover_mode,
            safe_shell_mode,
            remote_policy_enabled,
            remote_policy_url,
            disable_web_tools,
            disable_bash_tool,
            disable_subagent,
            disable_research,
            allow_tool_rule,
            deny_tool_rule,
            clear_tool_rules,
            path_restriction_enabled,
            additional_dir,
            clear_additional_dirs,
        } => {
            let mut cfg = crate::config::AppConfig::load();
            if let Some(p) = provider {
                cfg.provider = p;
            }
            if let Some(m) = model {
                cfg.model = crate::config::resolve_model_alias(&m);
            }
            if let Some(pm) = permission_mode {
                cfg.permission_mode = pm;
            }
            if let Some(v) = auto_review_mode {
                cfg.auto_review_mode = crate::normalize_auto_review_mode(&v);
            }
            if let Some(v) = auto_review_severity_threshold {
                cfg.auto_review_severity_threshold =
                    crate::normalize_auto_review_severity_threshold(&v);
            }
            if let Some(v) = execution_speed {
                cfg.execution_speed = v.as_str().to_string();
            }
            if let Some(mt) = max_turns {
                cfg.max_turns = mt.clamp(1, 200);
            }
            if let Some(v) = extended_thinking {
                cfg.extended_thinking = v;
            }
            if let Some(v) = markdown_render {
                cfg.markdown_render = v;
            }
            if let Some(v) = theme {
                cfg.theme = v;
            }
            if let Some(v) = telemetry_enabled {
                cfg.telemetry_enabled = v;
            }
            if let Some(v) = telemetry_log_tool_details {
                cfg.telemetry_log_tool_details = v;
            }
            if let Some(v) = undercover_mode {
                cfg.undercover_mode = v;
            }
            if let Some(v) = safe_shell_mode {
                cfg.safe_shell_mode = v;
            }
            if let Some(v) = remote_policy_enabled {
                cfg.remote_policy_enabled = v;
            }
            if let Some(v) = remote_policy_url {
                cfg.remote_policy_url = Some(v);
            }
            if let Some(v) = disable_web_tools {
                cfg.set_feature_disabled("web_tools", v);
            }
            if let Some(v) = disable_bash_tool {
                cfg.set_feature_disabled("bash_tool", v);
            }
            if let Some(v) = disable_subagent {
                cfg.set_feature_disabled("subagent", v);
            }
            if let Some(v) = disable_research {
                cfg.set_feature_disabled("research", v);
            }
            if clear_tool_rules {
                cfg.permission_allow_rules.clear();
                cfg.permission_deny_rules.clear();
            }
            if let Some(v) = path_restriction_enabled {
                cfg.path_restriction_enabled = v;
            }
            if clear_additional_dirs {
                cfg.additional_directories.clear();
            }
            for dir in additional_dir {
                let normalized = crate::normalize_directory(&dir)?;
                crate::add_unique_rule(&mut cfg.additional_directories, normalized);
            }
            for rule in allow_tool_rule {
                crate::add_unique_rule(&mut cfg.permission_allow_rules, crate::normalize_rule(&rule)?);
            }
            for rule in deny_tool_rule {
                crate::add_unique_rule(&mut cfg.permission_deny_rules, crate::normalize_rule(&rule)?);
            }
            cfg.provider = crate::config::normalize_provider_name(&cfg.provider);
            if cfg.model.is_empty() {
                cfg.model = crate::config::resolve_model_alias(crate::config::default_model(&cfg.provider));
            }
            let requested_model = cfg.model.clone();
            let (reconciled_model, fallback) =
                crate::config::reconcile_model_for_provider(&cfg.provider, &requested_model);
            if fallback {
                eprintln!(
                    "WARN model {} incompatible with provider {}, fallback={}",
                    requested_model, cfg.provider, reconciled_model
                );
            }
            cfg.model = reconciled_model;
            crate::config::apply_api_key_env(&cfg);
            let path = cfg.save()?;
            println!("{}", path.display());
            Ok(())
        }
        crate::Command::ApiPage => {
            crate::render_api_page(&crate::config::AppConfig::load());
            Ok(())
        }
        crate::Command::Setup => {
            let mut cfg = crate::config::AppConfig::load();
            let mut rt = crate::runtime::Runtime::new(
                cfg.provider.clone(),
                cfg.model.clone(),
                cfg.permission_mode.clone(),
                cfg.max_turns,
            );
            crate::apply_runtime_flags_from_cfg(&mut rt, &cfg);
            let mut ui = crate::ui::Ui::new(&cfg.theme);
            let msg = crate::run_setup_wizard(&mut cfg, &mut rt, &mut ui)?;
            println!("{}", msg);
            Ok(())
        }
        crate::Command::Mcp { action } => {
            let line = crate::mcp_cli_to_repl_args(action);
            println!("{}", crate::handle_mcp_command(&line)?);
            Ok(())
        }
        crate::Command::Plugin { action } => {
            let line = crate::plugin_cli_to_repl_args(action);
            println!("{}", crate::handle_plugin_command(&line)?);
            Ok(())
        }
        crate::Command::Hooks { action } => {
            let line = crate::hooks_cli_to_repl_args(action);
            println!("{}", crate::handle_hooks_command(&line)?);
            Ok(())
        }
        crate::Command::Theme { select } => {
            let mut cfg = crate::config::AppConfig::load();
            let mut ui = crate::ui::Ui::new(&cfg.theme);
            if let Some(i) = select {
                if let Some(key) = crate::theme_from_index(i) {
                    cfg.theme = key.to_string();
                    ui.set_theme(&cfg.theme);
                    let _ = cfg.save();
                    println!("theme={}", cfg.theme);
                }
            }
            println!("{}", ui.theme_menu(&cfg.theme));
            Ok(())
        }
        crate::Command::Scan {
            deep,
            deep_limit,
            patterns,
        } => {
            let mut tokens = patterns;
            if let Some(n) = deep_limit {
                tokens.push(format!("--deep={}", n));
            } else if deep {
                tokens.push("--deep".to_string());
            }
            let raw = tokens.join(" ");
            println!("{}", crate::render_pattern_scan_panel(&raw));
            Ok(())
        }
        crate::Command::Login { provider, token } => {
            let t = if let Some(v) = token {
                v
            } else {
                crate::prompt_input("OAuth token")?
            };
            crate::oauth::save_token(&provider, &t)?;
            println!(
                "OAuth token saved for provider={} at {}",
                provider,
                crate::oauth::oauth_path().display()
            );
            Ok(())
        }
        crate::Command::Logout { provider } => {
            let removed = crate::oauth::clear_token(&provider)?;
            println!("provider={} removed={}", provider, removed);
            Ok(())
        }
        crate::Command::Prompt {
            text,
            stdin,
            text_file,
            provider,
            model,
            permission_mode,
            project,
            agent,
            secure,
            agent_max_steps,
            profile,
            speed,
            output_format,
            prompt_auto_tools,
        } => {
            if let Some(path) = project.as_deref() {
                let _ = crate::set_project_dir(path)?;
            }
            let cfg = crate::resolve_cfg(provider, model, permission_mode);
            let speed = crate::resolve_execution_speed(speed, &cfg);
            let mut rt = crate::runtime::Runtime::new(
                cfg.provider.clone(),
                cfg.model.clone(),
                cfg.permission_mode.clone(),
                cfg.max_turns,
            );
            rt.extended_thinking = cfg.extended_thinking;
            crate::apply_runtime_flags_from_cfg(&mut rt, &cfg);

            let prompt_text = crate::resolve_prompt_text(text, stdin, text_file)?;
            let prompt_review_json_only = crate::parse_review_task_json_only(&prompt_text)
                .map(|(_, json_only)| json_only)
                .unwrap_or(false);
            let output_json_mode = matches!(
                output_format,
                crate::OutputFormat::Json | crate::OutputFormat::Jsonl
            ) || prompt_review_json_only;
            let output_jsonl_mode = matches!(output_format, crate::OutputFormat::Jsonl);
            let _json_bash_stream_guard = if output_json_mode {
                crate::ScopedEnvVar::set_default_if_unset("ASI_BASH_STREAM_OUTPUT", "false")
            } else {
                None
            };

            let (model_input, prompt_agent_enabled, prompt_secure_mode) =
                crate::prepare_prompt_agent_input(&prompt_text, agent, secure, profile)?;
            let is_tool_call = model_input.starts_with("/toolcall ");
            let constraints_for_prompt = crate::derive_tool_execution_constraints(
                &prompt_text,
                profile == crate::PromptProfile::Strict,
            );
            let model_input_for_runtime = if prompt_agent_enabled && !is_tool_call {
                let base_limits = crate::AutoLoopLimits {
                    max_steps: if agent_max_steps == 0 {
                        None
                    } else {
                        Some(agent_max_steps.min(10000))
                    },
                    max_duration: std::time::Duration::from_secs(
                        crate::parse_usize_env("ASI_AUTO_AGENT_MAX_DURATION_SECS", 3600)
                            .clamp(30, 24 * 60 * 60) as u64,
                    ),
                    max_no_progress_rounds: crate::parse_usize_env(
                        "ASI_AUTO_AGENT_MAX_NO_PROGRESS_ROUNDS",
                        12,
                    )
                    .clamp(1, 200),
                    max_consecutive_constraint_blocks: crate::parse_usize_env(
                        "ASI_AUTO_AGENT_MAX_CONSECUTIVE_CONSTRAINT_BLOCKS",
                        3,
                    )
                    .clamp(1, 50),
                };
                let complexity = crate::estimate_task_complexity(
                    &prompt_text,
                    profile == crate::PromptProfile::Strict,
                );
                let limits = crate::apply_adaptive_budgets(
                    base_limits,
                    complexity,
                    profile == crate::PromptProfile::Strict,
                    speed,
                );
                let header = crate::format_context_contract_header(
                    &rt,
                    profile == crate::PromptProfile::Strict,
                    speed,
                    constraints_for_prompt,
                    limits,
                    None,
                );
                format!("{}\n\n{}", header, model_input)
            } else {
                model_input.clone()
            };
            let mut changed_files = Vec::new();
            let mut file_synopsis_cache = crate::FileSynopsisCache::default();
            let mut confidence_gate_stats = crate::ConfidenceGateStats::default();

            let mut result = rt.run_turn(&model_input_for_runtime);
            crate::log_interaction_event(&cfg, &rt, &model_input_for_runtime, &result);

            if let Some(path) = crate::extract_changed_file(&model_input_for_runtime, &result) {
                crate::push_unique_changed_file(&mut changed_files, &path);
            }
            crate::collect_native_changed_files(&result.native_tool_calls, &mut changed_files);

            let mut turn_cost_sum = result.turn_cost_usd;
            let mut agent_steps = 0usize;
            let mut agent_loop_stop_reason: Option<String> = None;
            let prompt_auto_tools_without_agent = if let Some(value) = prompt_auto_tools {
                crate::parse_on_off(&value).ok_or_else(|| {
                    format!("invalid --prompt-auto-tools value: {} (expected on|off)", value)
                })?
            } else {
                std::env::var("ASI_PROMPT_AUTO_TOOLS")
                    .ok()
                    .and_then(|v| {
                        crate::parse_on_off(&v)
                            .or_else(|| match v.trim().to_ascii_lowercase().as_str() {
                                "auto" | "enabled" => Some(true),
                                _ => None,
                            })
                    })
                    .unwrap_or(true)
            };
            let prompt_auto_tools_active_for_task = crate::should_enable_auto_tooling_for_turn(
                &prompt_text,
                false,
            );
            let should_run_prompt_auto_loop =
                !is_tool_call
                    && (prompt_agent_enabled
                        || (prompt_auto_tools_without_agent && prompt_auto_tools_active_for_task));
            if should_run_prompt_auto_loop
                && crate::is_auto_loop_continuable_stop_reason(&result.stop_reason)
            {
                let base_limits = crate::AutoLoopLimits {
                    max_steps: if agent_max_steps == 0 {
                        None
                    } else {
                        Some(agent_max_steps.min(10000))
                    },
                    max_duration: std::time::Duration::from_secs(
                        crate::parse_usize_env("ASI_AUTO_AGENT_MAX_DURATION_SECS", 3600)
                            .clamp(30, 24 * 60 * 60) as u64,
                    ),
                    max_no_progress_rounds: crate::parse_usize_env(
                        "ASI_AUTO_AGENT_MAX_NO_PROGRESS_ROUNDS",
                        12,
                    )
                    .clamp(1, 200),
                    max_consecutive_constraint_blocks: crate::parse_usize_env(
                        "ASI_AUTO_AGENT_MAX_CONSECUTIVE_CONSTRAINT_BLOCKS",
                        3,
                    )
                    .clamp(1, 50),
                };
                let complexity = crate::estimate_task_complexity(
                    &prompt_text,
                    profile == crate::PromptProfile::Strict,
                );
                let limits = crate::apply_adaptive_budgets(
                    base_limits,
                    complexity,
                    profile == crate::PromptProfile::Strict,
                    speed,
                );
                let prompt_loop_input = if prompt_agent_enabled {
                    prompt_text.clone()
                } else {
                    let snapshot = crate::build_workspace_snapshot(&std::env::current_dir().map_err(|e| e.to_string())?);
                    if let Some((task, _json_only)) = crate::parse_review_task_json_only(&prompt_text)
                    {
                        crate::build_review_prompt(
                            &task,
                            &snapshot,
                            profile == crate::PromptProfile::Strict,
                        )
                    } else {
                        crate::build_work_prompt(
                            &prompt_text,
                            &snapshot,
                            profile == crate::PromptProfile::Strict,
                        )
                    }
                };
                let (loop_result, extra_cost, steps, loop_stop_reason) =
                    crate::run_prompt_auto_tool_loop(
                        &mut rt,
                        &cfg,
                        &prompt_loop_input,
                        &result,
                        &mut changed_files,
                        profile == crate::PromptProfile::Strict,
                        speed,
                        constraints_for_prompt,
                        limits,
                        None,
                        &mut file_synopsis_cache,
                        &mut confidence_gate_stats,
                    );
                result = loop_result;
                turn_cost_sum += extra_cost;
                agent_steps = steps;
                agent_loop_stop_reason = loop_stop_reason;
            }
            if should_run_prompt_auto_loop {
                let _ = crate::telemetry::log_auto_loop_summary(
                    &cfg,
                    &rt.provider,
                    &rt.model,
                    "prompt",
                    agent_loop_stop_reason.as_deref(),
                    confidence_gate_stats.checks(),
                    confidence_gate_stats.declaration_missing(),
                    confidence_gate_stats.declaration_low(),
                    confidence_gate_stats.blocked_risky_toolcalls(),
                    confidence_gate_stats.retries_exhausted(),
                );
            }

            // For prompt-mode /review --json-only, run a bounded schema-repair loop
            // even when no tool loop was entered, so strict JSON smoke can recover.
            if prompt_review_json_only
                && !prompt_agent_enabled
                && !should_run_prompt_auto_loop
                && crate::is_auto_loop_continuable_stop_reason(&result.stop_reason)
                && crate::should_enforce_review_output_schema(&prompt_text, &result.text)
            {
                let strict_mode = profile == crate::PromptProfile::Strict;
                let limits = crate::AutoLoopLimits {
                    max_steps: Some(2),
                    max_duration: std::time::Duration::from_secs(90),
                    max_no_progress_rounds: 2,
                    max_consecutive_constraint_blocks: 2,
                };
                let context_contract = crate::format_context_contract_header(
                    &rt,
                    strict_mode,
                    speed,
                    constraints_for_prompt,
                    limits,
                    None,
                );

                let mut repair_round = 0usize;
                while repair_round < 2
                    && crate::is_auto_loop_continuable_stop_reason(&result.stop_reason)
                {
                    if let Some(schema_error) =
                        crate::review_output_schema_error(&result.text)
                    {
                        let repair_prompt = crate::build_review_schema_repair_prompt(
                            &context_contract,
                            &prompt_text,
                            strict_mode,
                            &schema_error,
                        );
                        let next = rt.run_turn(&repair_prompt);
                        crate::log_interaction_event(&cfg, &rt, &repair_prompt, &next);
                        turn_cost_sum += next.turn_cost_usd;
                        crate::collect_native_changed_files(
                            &next.native_tool_calls,
                            &mut changed_files,
                        );
                        result = next;
                        repair_round += 1;
                        continue;
                    }
                    break;
                }
            }
            let auto_validation_results = crate::run_auto_validation_guards(&changed_files);

            let rendered = if cfg.markdown_render && !result.is_tool_result {
                crate::markdown::render_markdown_ansi(&result.text)
            } else {
                result.text.clone()
            };
            let review_payload = crate::build_review_json_payload(&prompt_text, &result.text);
            let store = crate::session::SessionStore::default()?;
            let meta = crate::session::SessionMeta {
                source: "prompt".to_string(),
                agent_enabled: prompt_agent_enabled,
                auto_loop_stop_reason: agent_loop_stop_reason.clone(),
                confidence_gate: crate::session::ConfidenceGateSessionStats {
                    checks: confidence_gate_stats.checks(),
                    missing_declaration: confidence_gate_stats.declaration_missing(),
                    low_declaration: confidence_gate_stats.declaration_low(),
                    blocked_risky_toolcalls: confidence_gate_stats.blocked_risky_toolcalls(),
                    retries_exhausted: confidence_gate_stats.retries_exhausted(),
                },
            };
            let session_id = match store.save_with_meta(
                &cfg.provider,
                &cfg.model,
                rt.as_json_messages(),
                Some(meta),
            ) {
                Ok(path) => path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string(),
                Err(e) => {
                    eprintln!("WARN session_save_failed: {}", e);
                    String::new()
                }
            };
            if !output_json_mode {
                    if let Some(t) = &result.thinking {
                        println!("{}", t);
                    }
                    println!("{}", rendered);
                    if !changed_files.is_empty() {
                        println!("changed_files={}", changed_files.join(", "));
                    }
                    if !auto_validation_results.is_empty() {
                        println!(
                            "{}",
                            crate::format_auto_validation_summary(&auto_validation_results)
                        );
                        for vr in &auto_validation_results {
                            println!(
                                "[auto_validate:{}] {}",
                                if vr.ok { "ok" } else { "error" },
                                vr.command
                            );
                            if !vr.output.trim().is_empty() {
                                println!("{}", vr.output);
                            }
                        }
                    }
                    println!(
                        "session={} stop_reason={} stop_reason_alias={} cost={} agent_mode={} secure_mode={} profile={} speed={} agent_steps={} agent_loop_stop_reason={} cg_checks={} cg_missing_declaration={} cg_low_declaration={} cg_blocked_risky_toolcalls={} cg_retries_exhausted={}",
                        session_id,
                        result.stop_reason,
                        crate::runtime::Runtime::stop_reason_alias(&result.stop_reason),
                        crate::cost::format_usd(turn_cost_sum),
                        prompt_agent_enabled,
                        prompt_secure_mode,
                        profile.as_str(),
                        speed.as_str(),
                        agent_steps,
                        agent_loop_stop_reason.as_deref().unwrap_or("none"),
                        confidence_gate_stats.checks(),
                        confidence_gate_stats.declaration_missing(),
                        confidence_gate_stats.declaration_low(),
                        confidence_gate_stats.blocked_risky_toolcalls(),
                        confidence_gate_stats.retries_exhausted(),
                    );
            } else {
                let common_payload = prompt_json_output(
                    &rt,
                    &result,
                    &review_payload,
                    &session_id,
                    &changed_files,
                    &auto_validation_results,
                    turn_cost_sum,
                    prompt_agent_enabled,
                    prompt_secure_mode,
                    profile,
                    speed,
                    agent_steps,
                    agent_loop_stop_reason.clone(),
                    &confidence_gate_stats,
                );

                if output_jsonl_mode {
                    println!(
                        "{}",
                        prompt_jsonl_event(
                            "prompt.result",
                            serde_json::json!({
                                "message": result.text,
                                "stop_reason": result.stop_reason,
                                "stop_reason_alias": crate::runtime::Runtime::stop_reason_alias(&result.stop_reason),
                                "session": session_id,
                                "usage": {
                                    "input_tokens": result.input_tokens,
                                    "output_tokens": result.output_tokens,
                                    "total_input_tokens": result.total_input_tokens,
                                    "total_output_tokens": result.total_output_tokens,
                                    "turn_cost_usd": turn_cost_sum,
                                    "total_cost_usd": result.total_cost_usd
                                },
                                "agent": {
                                    "enabled": prompt_agent_enabled,
                                    "secure_mode": prompt_secure_mode,
                                    "profile": profile.as_str(),
                                    "speed": speed.as_str(),
                                    "steps": agent_steps,
                                    "loop_stop_reason": agent_loop_stop_reason,
                                    "confidence_gate": {
                                        "checks": confidence_gate_stats.checks(),
                                        "missing_declaration": confidence_gate_stats.declaration_missing(),
                                        "low_declaration": confidence_gate_stats.declaration_low(),
                                        "blocked_risky_toolcalls": confidence_gate_stats.blocked_risky_toolcalls(),
                                        "retries_exhausted": confidence_gate_stats.retries_exhausted()
                                    }
                                }
                            })
                        )
                    );
                    println!("{}", prompt_jsonl_event(
                        "prompt.changed_files",
                        common_payload
                            .get("changed_files")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!([]))
                    ));
                    println!(
                        "{}",
                        prompt_jsonl_event(
                            "prompt.native_tool_calls",
                            prompt_native_tool_calls_json(&result.native_tool_calls)
                        )
                    );
                    println!(
                        "{}",
                        prompt_jsonl_event(
                            "prompt.auto_validation",
                            common_payload
                                .get("auto_validation")
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!([]))
                        )
                    );
                    println!(
                        "{}",
                        prompt_jsonl_event(
                            "prompt.review",
                            common_payload
                                .get("review")
                                .cloned()
                                .unwrap_or_else(|| serde_json::json!({}))
                        )
                    );
                    println!(
                        "{}",
                        prompt_jsonl_event(
                            "prompt.runtime",
                            serde_json::json!({
                                "runtime": common_payload
                                    .get("runtime")
                                    .cloned()
                                    .unwrap_or_else(|| serde_json::json!({})),
                                "thinking": common_payload
                                    .get("thinking")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null)
                            })
                        )
                    );
                } else if prompt_review_json_only {
                    let review_json_only_output = enrich_review_json_only_output(
                        crate::build_review_json_only_prompt_output(&review_payload),
                        &result,
                    );
                    println!("{}", review_json_only_output);
                    if crate::should_fail_review_json_only_prompt_output(&review_json_only_output) {
                        return Err(
                            "review json-only schema invalid (strict fail-exit enabled)"
                                .to_string(),
                        );
                    }
                } else {
                    println!("{}", common_payload);
                }
            }
            Ok(())
        }
        crate::Command::Repl {
            provider,
            model,
            permission_mode,
            project,
            no_setup,
            voice,
            profile,
            speed,
        } => {
            let cfg = crate::resolve_cfg(provider, model, permission_mode);
            crate::run_repl(cfg, project, no_setup, voice, profile, speed)
        }
        crate::Command::Research { topic, rounds } => {
            let cfg = crate::config::AppConfig::load();
            if cfg.is_feature_disabled("research") {
                return Err("research is disabled by feature killswitch".to_string());
            }
            let report = crate::research::run_research(&topic, rounds);
            let out = std::env::current_dir()
                .map_err(|e| e.to_string())?
                .join("research_report.md");
            std::fs::write(&out, &report).map_err(|e| e.to_string())?;
            println!("{}", report);
            println!("saved={}", out.display());
            Ok(())
        }
        crate::Command::Sessions {
            limit,
            agent_enabled,
            blocked_only,
        } => {
            let store = crate::session::SessionStore::default()?;
            for sid in store.list_sessions(limit)? {
                match store.load(&sid) {
                    Ok(s) => {
                        if !crate::session_matches_filters(&s, agent_enabled, blocked_only) {
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
            Ok(())
        }
        crate::Command::Resume { session_id } => {
            let store = crate::session::SessionStore::default()?;
            let s = store.load(&session_id)?;
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
                "session={} provider={} model={} messages={}{}",
                s.session_id,
                s.provider,
                s.model,
                s.messages.len(),
                meta_line
            );
            Ok(())
        }
        crate::Command::Autoresearch { action } => crate::handle_autoresearch_command(action),
        crate::Command::Tokenizer { action } => crate::handle_tokenizer_command(action),
        crate::Command::Wiki { action } => crate::handle_wiki_command(action),
        crate::Command::Daemon { action } => crate::handle_daemon_command(action),
        crate::Command::Job { action } => crate::handle_job_command(action),
    }
}

fn enrich_review_json_only_output(
    mut value: serde_json::Value,
    result: &crate::runtime::TurnResult,
) -> serde_json::Value {
    let stop_reason_alias = crate::runtime::Runtime::stop_reason_alias(&result.stop_reason);
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "stop_reason".to_string(),
            serde_json::Value::String(result.stop_reason.clone()),
        );
        obj.insert(
            "stop_reason_alias".to_string(),
            serde_json::Value::String(stop_reason_alias.to_string()),
        );
        let is_provider_error = stop_reason_alias.eq_ignore_ascii_case("provider_error")
            || result.stop_reason.trim().eq_ignore_ascii_case("provider_error");
        obj.insert(
            "provider_error".to_string(),
            serde_json::Value::Bool(is_provider_error),
        );
        if is_provider_error && !result.text.trim().is_empty() {
            obj.insert(
                "provider_error_message".to_string(),
                serde_json::Value::String(result.text.clone()),
            );
        }
    }
    value
}

fn prompt_jsonl_event(event: &str, payload: serde_json::Value) -> String {
    serde_json::json!({
        "schema_version": "1",
        "event": event,
        "data": payload
    })
    .to_string()
}

fn prompt_native_tool_calls_json(
    native_tool_calls: &[crate::runtime::NativeToolResult],
) -> serde_json::Value {
    let items: Vec<serde_json::Value> = native_tool_calls
        .iter()
        .map(|call| {
            let preview = crate::clip_chars(call.result_text.trim(), 600);
            let preview_clipped = preview != call.result_text.trim();
            serde_json::json!({
                "tool_call_id": call.tool_call_id,
                "tool_name": call.tool_name,
                "tool_args_display": call.tool_args_display,
                "ok": call.ok,
                "result_text_preview": preview,
                "result_text_preview_clipped": preview_clipped,
                "result_text_length_chars": call.result_text.chars().count()
            })
        })
        .collect();

    serde_json::json!({
        "count": items.len(),
        "items": items
    })
}

fn prompt_json_output(
    rt: &crate::runtime::Runtime,
    result: &crate::runtime::TurnResult,
    review_payload: &serde_json::Value,
    session_id: &str,
    changed_files: &[String],
    auto_validation_results: &[crate::AutoValidationResult],
    turn_cost_sum: f64,
    prompt_agent_enabled: bool,
    prompt_secure_mode: bool,
    profile: crate::PromptProfile,
    speed: crate::ExecutionSpeed,
    agent_steps: usize,
    agent_loop_stop_reason: Option<String>,
    confidence_gate_stats: &crate::ConfidenceGateStats,
) -> serde_json::Value {
    serde_json::json!({
        "message": result.text,
        "stop_reason": result.stop_reason,
        "stop_reason_alias": crate::runtime::Runtime::stop_reason_alias(&result.stop_reason),
        "runtime": {
            "stop_reason_last": {
                "raw": rt.last_stop_reason_raw(),
                "alias": rt.last_stop_reason_alias(),
            },
            "provider": {
                "runtime_line": rt.status_provider_runtime_line(),
                "error_line": rt.status_provider_error_line(),
                "decode_stats_line": rt.provider_decode_stats_line(),
            }
        },
        "session": session_id,
        "usage": {
            "input_tokens": result.input_tokens,
            "output_tokens": result.output_tokens,
            "total_input_tokens": result.total_input_tokens,
            "total_output_tokens": result.total_output_tokens,
            "turn_cost_usd": turn_cost_sum,
            "total_cost_usd": result.total_cost_usd
        },
        "thinking": result.thinking,
        "changed_files": changed_files,
        "auto_validation": auto_validation_results.iter().map(|vr| serde_json::json!({
            "command": vr.command,
            "ok": vr.ok,
            "output": vr.output
        })).collect::<Vec<_>>(),
        "review": review_payload,
        "agent": {
            "enabled": prompt_agent_enabled,
            "secure_mode": prompt_secure_mode,
            "profile": profile.as_str(),
            "speed": speed.as_str(),
            "steps": agent_steps,
            "input_transformed": prompt_agent_enabled,
            "loop_stop_reason": agent_loop_stop_reason,
            "confidence_gate": {
                "checks": confidence_gate_stats.checks(),
                "missing_declaration": confidence_gate_stats.declaration_missing(),
                "low_declaration": confidence_gate_stats.declaration_low(),
                "blocked_risky_toolcalls": confidence_gate_stats.blocked_risky_toolcalls(),
                "retries_exhausted": confidence_gate_stats.retries_exhausted()
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn prompt_json_agent_payload_includes_confidence_gate_fields() {
        let payload = json!({
            "agent": {
                "enabled": true,
                "secure_mode": false,
                "profile": "strict",
                "speed": "deep",
                "steps": 3,
                "input_transformed": true,
                "loop_stop_reason": "none",
                "confidence_gate": {
                    "checks": 2,
                    "missing_declaration": 1,
                    "low_declaration": 1,
                    "blocked_risky_toolcalls": 4,
                    "retries_exhausted": 0
                }
            }
        });

        let agent = payload.get("agent").unwrap();
        let gate = agent.get("confidence_gate").unwrap();
        assert_eq!(gate.get("checks").and_then(|v| v.as_u64()), Some(2));
        assert_eq!(
            gate.get("missing_declaration").and_then(|v| v.as_u64()),
            Some(1)
        );
        assert_eq!(
            gate.get("low_declaration").and_then(|v| v.as_u64()),
            Some(1)
        );
        assert_eq!(
            gate.get("blocked_risky_toolcalls").and_then(|v| v.as_u64()),
            Some(4)
        );
        assert_eq!(
            gate.get("retries_exhausted").and_then(|v| v.as_u64()),
            Some(0)
        );
    }

    #[test]
    fn prompt_json_includes_stop_reason_alias_and_runtime_stop_reason() {
        let payload = json!({
            "stop_reason": "end_turn",
            "stop_reason_alias": "completed",
            "runtime": {
                "stop_reason_last": {
                    "raw": "end_turn",
                    "alias": "completed"
                }
            }
        });

        assert_eq!(
            payload.get("stop_reason_alias").and_then(|v| v.as_str()),
            Some("completed")
        );
        assert_eq!(
            payload
                .get("runtime")
                .and_then(|v| v.get("stop_reason_last"))
                .and_then(|v| v.get("raw"))
                .and_then(|v| v.as_str()),
            Some("end_turn")
        );
        assert_eq!(
            payload
                .get("runtime")
                .and_then(|v| v.get("stop_reason_last"))
                .and_then(|v| v.get("alias"))
                .and_then(|v| v.as_str()),
            Some("completed")
        );
    }

    #[test]
    fn prompt_json_review_payload_includes_extended_stats_and_sorted_findings() {
        let task = "/review inspect parser";
        let review_text = "Findings:\n- [LOW] src/z.rs:7 - low issue\n- [CRITICAL] src/a.rs:2 - critical issue\nMissing Tests:\n- add parser regression test\nOpen Questions:\n- None\nSummary:\n- done";
        let review = crate::build_review_json_payload(task, review_text);
        let payload = json!({
            "message": review_text,
            "review": review
        });

        assert_eq!(
            payload
                .get("review")
                .and_then(|v| v.get("schema_version"))
                .and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(
            payload
                .get("review")
                .and_then(|v| v.get("is_review_task"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            payload
                .get("review")
                .and_then(|v| v.get("schema_valid"))
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            payload
                .get("review")
                .and_then(|v| v.get("stats"))
                .and_then(|v| v.get("risk_score_total"))
                .and_then(|v| v.as_u64()),
            Some(9)
        );
        assert_eq!(
            payload
                .get("review")
                .and_then(|v| v.get("stats"))
                .and_then(|v| v.get("missing_tests_count"))
                .and_then(|v| v.as_u64()),
            Some(1)
        );
        let top_path = payload
            .get("review")
            .and_then(|v| v.get("stats"))
            .and_then(|v| v.get("top_risk_paths"))
            .and_then(|v| v.as_array())
            .and_then(|v| v.first())
            .and_then(|v| v.get("path"))
            .and_then(|v| v.as_str());
        assert_eq!(top_path, Some("src/a.rs"));

        let sorted = payload
            .get("review")
            .and_then(|v| v.get("sections"))
            .and_then(|v| v.get("findings_sorted"))
            .and_then(|v| v.as_array())
            .expect("findings_sorted");
        assert_eq!(sorted.len(), 2);
        assert_eq!(
            sorted[0]
                .get("normalized_severity")
                .and_then(|v| v.as_str()),
            Some("critical")
        );
    }

    #[test]
    fn prompt_review_json_only_detection_works() {
        let parsed = crate::parse_review_task_json_only("/review inspect parser --json-only")
            .expect("review parse");
        assert_eq!(parsed.0, "inspect parser");
        assert!(parsed.1);

        let parsed2 =
            crate::parse_review_task_json_only("/review inspect parser").expect("review parse");
        assert_eq!(parsed2.0, "inspect parser");
        assert!(!parsed2.1);
    }

    #[test]
    fn prompt_review_json_only_strict_wrapper_is_machine_readable() {
        let review_payload =
            crate::build_review_json_payload("/review parser", "Findings:\n- invalid only");
        let wrapped = crate::build_review_json_only_prompt_output(&review_payload);
        let strict = crate::build_review_json_only_prompt_output_with_strict(&review_payload, true);
        let non_strict =
            crate::build_review_json_only_prompt_output_with_strict(&review_payload, false);

        assert!(wrapped.is_object());
        assert_eq!(wrapped.get("schema_version").and_then(|v| v.as_str()), Some("1"));
        assert_eq!(wrapped.get("status"), None);
        assert_eq!(strict.get("status").and_then(|v| v.as_str()), Some("error"));
        assert_eq!(strict.get("schema_version").and_then(|v| v.as_str()), Some("1"));
        assert_eq!(
            strict.get("error").and_then(|v| v.as_str()),
            Some("review_schema_invalid")
        );
        assert_eq!(
            non_strict
                .get("schema_valid")
                .and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[test]
    fn prompt_review_json_only_strict_fail_exit_toggle_detects_invalid_envelope() {
        let review_payload =
            crate::build_review_json_payload("/review parser", "Findings:\n- invalid only");
        let strict = crate::build_review_json_only_prompt_output_with_strict(&review_payload, true);
        let non_strict =
            crate::build_review_json_only_prompt_output_with_strict(&review_payload, false);

        assert!(crate::should_fail_review_json_only_prompt_output_with_strict_exit(
            &strict, true
        ));
        assert!(!crate::should_fail_review_json_only_prompt_output_with_strict_exit(
            &strict, false
        ));
        assert!(!crate::should_fail_review_json_only_prompt_output_with_strict_exit(
            &non_strict,
            true
        ));
    }

    #[test]
    fn prompt_review_json_only_success_envelope_option_works() {
        let review_payload = crate::build_review_json_payload(
            "/review parser",
            "Findings:\n- [LOW] src/a.rs:1 - ok\nMissing Tests:\n- None\nOpen Questions:\n- None\nSummary:\n- done",
        );
        let wrapped = crate::build_review_json_only_prompt_output_with_options(
            &review_payload,
            false,
            true,
        );
        assert_eq!(wrapped.get("status").and_then(|v| v.as_str()), Some("ok"));
        assert_eq!(wrapped.get("schema_version").and_then(|v| v.as_str()), Some("1"));
        assert!(wrapped.get("review").is_some());

        let unwrapped = crate::build_review_json_only_prompt_output_with_options(
            &review_payload,
            false,
            false,
        );
        assert!(unwrapped.get("status").is_none());
        assert_eq!(unwrapped.get("schema_version").and_then(|v| v.as_str()), Some("1"));
    }

    #[test]
    fn enrich_review_json_only_output_sets_provider_error_fields() {
        let turn = crate::runtime::TurnResult {
            text: "error sending request for url".to_string(),
            stop_reason: "provider_error".to_string(),
            input_tokens: 0,
            output_tokens: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            is_tool_result: false,
            turn_cost_usd: 0.0,
            total_cost_usd: 0.0,
            thinking: None,
            native_tool_calls: Vec::new(),
        };
        let payload = crate::build_review_json_payload("/review parser", "Findings:\n- invalid only");
        let json_only = crate::build_review_json_only_prompt_output_with_strict(&payload, true);
        let enriched = super::enrich_review_json_only_output(json_only, &turn);
        assert_eq!(
            enriched.get("stop_reason_alias").and_then(|v| v.as_str()),
            Some("provider_error")
        );
        assert_eq!(
            enriched.get("provider_error").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert!(
            enriched
                .get("provider_error_message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .contains("error sending request")
        );
    }

    #[test]
    fn prompt_jsonl_event_envelope_is_stable() {
        let row = super::prompt_jsonl_event(
            "prompt.result",
            json!({
                "session": "123",
                "stop_reason": "end_turn"
            }),
        );
        let parsed: serde_json::Value = serde_json::from_str(&row).expect("parse jsonl row");
        assert_eq!(
            parsed.get("schema_version").and_then(|v| v.as_str()),
            Some("1")
        );
        assert_eq!(parsed.get("event").and_then(|v| v.as_str()), Some("prompt.result"));
        assert_eq!(
            parsed
                .get("data")
                .and_then(|v| v.get("session"))
                .and_then(|v| v.as_str()),
            Some("123")
        );
    }

    #[test]
    fn prompt_native_tool_calls_json_shape_is_stable() {
        let payload = super::prompt_native_tool_calls_json(&[crate::runtime::NativeToolResult {
            tool_call_id: "call_1".to_string(),
            tool_name: "read_file".to_string(),
            tool_args_display: "README.md 1 20".to_string(),
            result_text: "ok".to_string(),
            ok: true,
        }]);

        assert_eq!(payload.get("count").and_then(|v| v.as_u64()), Some(1));
        let first = payload
            .get("items")
            .and_then(|v| v.as_array())
            .and_then(|v| v.first())
            .expect("first item");
        assert_eq!(
            first.get("tool_call_id").and_then(|v| v.as_str()),
            Some("call_1")
        );
        assert_eq!(
            first.get("tool_name").and_then(|v| v.as_str()),
            Some("read_file")
        );
        assert_eq!(first.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            first.get("result_text_preview").and_then(|v| v.as_str()),
            Some("ok")
        );
        assert_eq!(
            first
                .get("result_text_preview_clipped")
                .and_then(|v| v.as_bool()),
            Some(false)
        );
    }
}
