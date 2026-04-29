// Extracted from main.rs to reduce main-file size and isolate auto-loop orchestration logic.

#[derive(Clone, Copy)]
enum LoopMode {
    ReplVerbose,
    PromptCompact,
}

fn auto_loop_stop_reason_at_loop_head(
    started: Instant,
    limits: AutoLoopLimits,
    no_progress_rounds: usize,
    steps: usize,
    consecutive_failures: usize,
    engine: &OrchestratorEngine,
) -> Option<String> {
    if started.elapsed() >= limits.max_duration {
        return Some("exceeded max auto-loop duration".to_string());
    }
    if no_progress_rounds >= limits.max_no_progress_rounds {
        return Some("no progress across too many rounds".to_string());
    }
    if let Some(reason) = engine.stop_reason(steps, consecutive_failures) {
        return Some(reason.to_string());
    }
    None
}

fn record_tool_result_observation(
    cfg: &AppConfig,
    rt: &Runtime,
    command: &str,
    tool_result: &runtime::TurnResult,
    file_synopsis_cache: &mut FileSynopsisCache,
    canonical_observations: &mut Vec<String>,
    failure_memory: &mut Vec<FailureMemoryEntry>,
    consecutive_failures: &mut usize,
    last_failure_category: &mut Option<ToolFailureCategory>,
    consecutive_constraint_blocks: &mut usize,
    max_consecutive_constraint_blocks: usize,
) -> Option<String> {
    cache_read_file_from_command(file_synopsis_cache, command);
    log_interaction_event(cfg, rt, command, tool_result);
    cache_read_file_from_tool_result(file_synopsis_cache, tool_result);

    if let Some((line, memory_entry)) = canonical_tool_result_line(command, tool_result) {
        push_canonical_tool_observation(canonical_observations, line, TOOL_OBSERVATION_WINDOW);
        if let Some(entry) = memory_entry {
            push_failure_memory(failure_memory, entry, FAILURE_MEMORY_WINDOW);
        }
    }

    if is_tool_failure(tool_result) {
        *consecutive_failures += 1;
        *last_failure_category = Some(classify_tool_failure_result(tool_result));
        if is_constraint_block_text(&tool_result.text) {
            *consecutive_constraint_blocks += 1;
        }
    } else {
        *consecutive_failures = 0;
        *last_failure_category = None;
        *consecutive_constraint_blocks = 0;
    }

    constraint_block_stop_reason_if_needed(
        *consecutive_constraint_blocks,
        max_consecutive_constraint_blocks,
        Some(&tool_result.text),
    )
}

fn run_parallel_batch_tool_results(
    rt: &mut Runtime,
    constraints: ToolExecutionConstraints,
    commands: &[String],
) -> Vec<runtime::TurnResult> {
    let mut tool_results = Vec::with_capacity(commands.len());
    for command in commands {
        if let Some(reason) = toolcall_is_blocked_by_user_constraints(command, constraints) {
            let blocked = constrained_tool_result(rt, reason);
            tool_results.push(blocked);
        } else {
            let mut one = rt.run_manual_toolcall_batch_concurrent(&[command.clone()]);
            tool_results.push(one.pop().unwrap_or(constrained_tool_result(
                rt,
                "blocked: empty tool result".to_string(),
            )));
        }
    }
    tool_results
}

fn run_sequential_tool_call_with_observation(
    rt: &mut Runtime,
    cfg: &AppConfig,
    command: &str,
    constraints: ToolExecutionConstraints,
    file_synopsis_cache: &mut FileSynopsisCache,
    canonical_observations: &mut Vec<String>,
    failure_memory: &mut Vec<FailureMemoryEntry>,
    consecutive_failures: &mut usize,
    last_failure_category: &mut Option<ToolFailureCategory>,
    consecutive_constraint_blocks: &mut usize,
    max_consecutive_constraint_blocks: usize,
) -> (runtime::TurnResult, Option<String>) {
    let tool_result = if let Some(reason) =
        toolcall_is_blocked_by_user_constraints(command, constraints)
    {
        constrained_tool_result(rt, reason)
    } else {
        rt.run_turn(command)
    };
    let stop_reason = record_tool_result_observation(
        cfg,
        rt,
        command,
        &tool_result,
        file_synopsis_cache,
        canonical_observations,
        failure_memory,
        consecutive_failures,
        last_failure_category,
        consecutive_constraint_blocks,
        max_consecutive_constraint_blocks,
    );
    (tool_result, stop_reason)
}

fn build_followup_prompt_for_loop(
    engine: &OrchestratorEngine,
    strict_mode: bool,
    anchored_context: &str,
    last_failure_category: Option<ToolFailureCategory>,
    failure_memory: &[FailureMemoryEntry],
    canonical_observations: &[String],
    file_synopsis_cache: &FileSynopsisCache,
) -> String {
    let followup_base = engine.followup_prompt_for(strict_mode);
    let followup_body = append_failure_memory_to_followup(
        followup_base,
        strict_mode,
        last_failure_category,
        failure_memory,
        canonical_observations,
    );
    let followup_with_synopsis =
        append_file_synopsis_to_followup(followup_body, file_synopsis_cache, strict_mode);
    format!("{}\n\n{}", anchored_context, followup_with_synopsis)
}

fn apply_followup_turn_updates(
    next: &runtime::TurnResult,
    extra_cost: &mut f64,
    changed_files: &mut Vec<String>,
    last_progress_count: &mut usize,
    no_progress_rounds: &mut usize,
    last_failure_category: &mut Option<ToolFailureCategory>,
    consecutive_constraint_blocks: &mut usize,
    max_consecutive_constraint_blocks: usize,
) -> Option<String> {
    *extra_cost += next.turn_cost_usd;
    collect_native_changed_files(&next.native_tool_calls, changed_files);

    if changed_files.len() > *last_progress_count {
        *no_progress_rounds = 0;
        *last_progress_count = changed_files.len();
    } else {
        *no_progress_rounds += 1;
    }

    if is_tool_failure(next) {
        *last_failure_category = Some(classify_tool_failure_result(next));
        if is_constraint_block_text(&next.text) {
            *consecutive_constraint_blocks += 1;
        }
    } else {
        *last_failure_category = None;
        *consecutive_constraint_blocks = 0;
    }

    constraint_block_stop_reason_if_needed(
        *consecutive_constraint_blocks,
        max_consecutive_constraint_blocks,
        Some(&next.text),
    )
}

fn observe_assistant_followup_turn(
    file_synopsis_cache: &mut FileSynopsisCache,
    canonical_observations: &mut Vec<String>,
    failure_memory: &mut Vec<FailureMemoryEntry>,
    next: &runtime::TurnResult,
) {
    cache_read_file_from_tool_result(file_synopsis_cache, next);
    if let Some((line, memory_entry)) = canonical_tool_result_line("/toolcall tool <assistant>", next) {
        push_canonical_tool_observation(canonical_observations, line, TOOL_OBSERVATION_WINDOW);
        if let Some(entry) = memory_entry {
            push_failure_memory(failure_memory, entry, FAILURE_MEMORY_WINDOW);
        }
    }
}

fn post_batch_stop_reason(
    engine: &OrchestratorEngine,
    steps: usize,
    consecutive_failures: usize,
    consecutive_constraint_blocks: usize,
    max_consecutive_constraint_blocks: usize,
) -> Option<String> {
    if let Some(reason) = engine.stop_reason(steps, consecutive_failures) {
        return Some(reason.to_string());
    }
    constraint_block_stop_reason_if_needed(
        consecutive_constraint_blocks,
        max_consecutive_constraint_blocks,
        None,
    )
}

fn record_changed_path_for_mode(
    mode: LoopMode,
    path: &str,
    changed_files: &mut Vec<String>,
    change_events: Option<&mut Vec<String>>,
    ui: Option<&Ui>,
    event_kind: &'static str,
) {
    push_unique_changed_file(changed_files, path);
    if let Some(events) = change_events {
        push_change_event(events, event_kind, path);
    }
    if matches!(mode, LoopMode::ReplVerbose) {
        if let Some(panel) = ui {
            println!("{}", panel.info(&format!("changed_file={}", path)));
        }
    }
}

fn record_changed_files_from_tool_result(
    mode: LoopMode,
    command: &str,
    tool_result: &runtime::TurnResult,
    changed_files: &mut Vec<String>,
    mut change_events: Option<&mut Vec<String>>,
    ui: Option<&Ui>,
) {
    if let Some(path) = extract_changed_file(command, tool_result) {
        record_changed_path_for_mode(
            mode,
            &path,
            changed_files,
            change_events.as_deref_mut(),
            ui,
            "auto",
        );
    }
    for path in collect_native_changed_paths(&tool_result.native_tool_calls) {
        record_changed_path_for_mode(
            mode,
            &path,
            changed_files,
            change_events.as_deref_mut(),
            ui,
            "auto_native",
        );
    }
}

fn loop_info(mode: LoopMode, ui: Option<&Ui>, message: &str) {
    if matches!(mode, LoopMode::ReplVerbose) {
        if let Some(panel) = ui {
            println!("{}", panel.info(message));
        }
    }
}

fn run_turn_for_mode(
    rt: &mut Runtime,
    cfg: &AppConfig,
    mode: LoopMode,
    ui: Option<&Ui>,
    prompt: &str,
) -> (runtime::TurnResult, bool) {
    let mut streamed = false;
    let next = match mode {
        LoopMode::ReplVerbose => {
            if let Some(panel) = ui {
                rt.run_turn_streaming(prompt, |delta| {
                    if !streamed {
                        streamed = true;
                        println!();
                        print!("{} • ", panel.assistant_label());
                    }
                    print!("{}", delta);
                    let _ = io::stdout().flush();
                })
            } else {
                rt.run_turn(prompt)
            }
        }
        LoopMode::PromptCompact => rt.run_turn(prompt),
    };
    log_interaction_event(cfg, rt, prompt, &next);
    if streamed {
        println!();
    }
    (next, streamed)
}

fn render_turn_output_for_mode(
    mode: LoopMode,
    ui: Option<&Ui>,
    markdown_render: bool,
    turn: &runtime::TurnResult,
    streamed: bool,
    show_thinking: bool,
) {
    if !matches!(mode, LoopMode::ReplVerbose) {
        return;
    }
    let Some(panel) = ui else {
        return;
    };

    if show_thinking {
        if let Some(thinking) = &turn.thinking {
            println!("{}", panel.thinking_block(thinking));
        }
    }
    if turn.stop_reason == "provider_error" {
        println!("{}", panel.error(&turn.text));
    } else if turn.is_tool_result {
        if let Some((name, status, body)) = parse_tool_payload(&turn.text) {
            println!(
                "{}",
                panel.tool_panel(
                    &name,
                    &status,
                    &compact_tool_panel_body(&name, &status, &body)
                )
            );
        } else {
            println!("{}", panel.tool_panel("tool", "result", &turn.text));
        }
    } else if !streamed {
        let text = if markdown_render {
            markdown::render_markdown_ansi(&turn.text)
        } else {
            turn.text.clone()
        };
        println!("{}", panel.assistant(&text));
    }
}

fn run_repair_pass_for_mode(
    rt: &mut Runtime,
    cfg: &AppConfig,
    mode: LoopMode,
    ui: Option<&Ui>,
    strict_mode: bool,
) -> runtime::TurnResult {
    match mode {
        LoopMode::ReplVerbose => {
            if let Some(panel) = ui {
                run_repair_pass_streaming(rt, cfg, panel, strict_mode)
            } else {
                run_repair_pass(rt, cfg, strict_mode)
            }
        }
        LoopMode::PromptCompact => run_repair_pass(rt, cfg, strict_mode),
    }
}

fn run_confidence_gate_pass_for_mode(
    rt: &mut Runtime,
    cfg: &AppConfig,
    mode: LoopMode,
    ui: Option<&Ui>,
    prompt: &str,
) -> runtime::TurnResult {
    match mode {
        LoopMode::ReplVerbose => {
            if let Some(panel) = ui {
                run_confidence_gate_pass_streaming(rt, cfg, panel, prompt)
            } else {
                run_confidence_gate_pass(rt, cfg, prompt)
            }
        }
        LoopMode::PromptCompact => run_confidence_gate_pass(rt, cfg, prompt),
    }
}

fn record_native_changed_files_for_mode(
    mode: LoopMode,
    native_tool_calls: &[runtime::NativeToolResult],
    changed_files: &mut Vec<String>,
    mut change_events: Option<&mut Vec<String>>,
    ui: Option<&Ui>,
) {
    for path in collect_native_changed_paths(native_tool_calls) {
        record_changed_path_for_mode(
            mode,
            &path,
            changed_files,
            change_events.as_deref_mut(),
            ui,
            "auto_native",
        );
    }
}

fn prompt_mode_short_circuits_provider_error(mode: LoopMode) -> bool {
    matches!(mode, LoopMode::PromptCompact)
}

fn run_tool_loop_core(
    rt: &mut Runtime,
    cfg: &AppConfig,
    task_input: &str,
    markdown_render: bool,
    initial_result: &runtime::TurnResult,
    changed_files: &mut Vec<String>,
    mut change_events: Option<&mut Vec<String>>,
    strict_mode: bool,
    speed: ExecutionSpeed,
    constraints: ToolExecutionConstraints,
    limits: AutoLoopLimits,
    previous_loop_stop_reason: Option<&str>,
    file_synopsis_cache: &mut FileSynopsisCache,
    confidence_gate_stats: &mut ConfidenceGateStats,
    mode: LoopMode,
    ui: Option<&Ui>,
) -> (runtime::TurnResult, f64, usize, Option<String>) {
    let step_budget = limits.max_steps.unwrap_or(usize::MAX / 4);
    let engine = OrchestratorEngine::new(step_budget, if strict_mode { 4 } else { 4 });
    let mut current = initial_result.clone();
    let mut extra_cost = 0.0;
    let mut steps = 0usize;
    let mut consecutive_failures = 0usize;
    let mut last_failure_category: Option<ToolFailureCategory> = None;
    let mut mixed_output_repair_attempts = 0usize;
    let mut malformed_repair_attempted = false;
    let mut no_progress_rounds = 0usize;
    let mut last_progress_count = changed_files.len();
    let mut consecutive_constraint_blocks = 0usize;
    let mut loop_stop_reason: Option<String> = None;
    let mut failure_memory: Vec<FailureMemoryEntry> = Vec::new();
    let mut canonical_observations: Vec<String> = Vec::new();
    let mut plan_handshake_done = false;
    let mut confidence_gate_retries = 0usize;
    let mut zero_toolcall_nudge_attempted = false;
    let mut strict_forced_execution_attempts = 0usize;
    let mut bilingual_md_retries = 0usize;
    let mut review_schema_retries = 0usize;
    let started = Instant::now();

    loop {
        if let Some(reason) = auto_loop_stop_reason_at_loop_head(
            started,
            limits,
            no_progress_rounds,
            steps,
            consecutive_failures,
            &engine,
        ) {
            loop_info(mode, ui, &format!("auto-agent stopped: {}", reason));
            loop_stop_reason = Some(reason);
            break;
        }

        if !current.is_tool_result
            && should_attempt_auto_output_repair(
                mixed_output_repair_attempts >= AUTO_OUTPUT_REPAIR_MAX_ATTEMPTS,
                &current.text,
            )
        {
            let replayable_toolcalls = planned_toolcall_count(&current.text);
            mixed_output_repair_attempts += 1;
            if matches!(mode, LoopMode::ReplVerbose) {
                if looks_like_mixed_toolcall_and_result_output(&current.text) {
                    loop_info(
                        mode,
                        ui,
                        "auto-agent: mixed toolcall+result output detected; forcing repair",
                    );
                } else {
                    loop_info(
                        mode,
                        ui,
                        "auto-agent: fabricated tool-result text detected; forcing repair",
                    );
                }
            }
            let repaired = run_repair_pass_for_mode(rt, cfg, mode, ui, strict_mode);
            if repaired.stop_reason == "provider_error" {
                render_turn_output_for_mode(mode, ui, markdown_render, &repaired, false, false);
                current = repaired;
                break;
            }

            extra_cost += repaired.turn_cost_usd;
            if matches!(mode, LoopMode::ReplVerbose) {
                record_native_changed_files_for_mode(
                    mode,
                    &repaired.native_tool_calls,
                    changed_files,
                    change_events.as_deref_mut(),
                    ui,
                );
            } else {
                collect_native_changed_files(&repaired.native_tool_calls, changed_files);
            }

            if repaired.text.trim() == "NO_TOOLCALLS" {
                if replayable_toolcalls > 0 {
                    loop_info(
                        mode,
                        ui,
                        &format!(
                            "auto-agent: repair returned NO_TOOLCALLS; replaying {} parsed toolcall(s) from previous mixed output",
                            replayable_toolcalls
                        ),
                    );
                    continue;
                }
                if should_enforce_bilingual_markdown_reports(task_input, changed_files)
                    && bilingual_md_retries < BILINGUAL_MD_ENFORCEMENT_MAX_RETRIES
                {
                    bilingual_md_retries += 1;
                    let enforce_prompt = build_bilingual_markdown_artifact_prompt(
                        &context_with_task_anchor(
                            &format_context_contract_header(
                                rt,
                                strict_mode,
                                speed,
                                constraints,
                                limits,
                                previous_loop_stop_reason,
                            ),
                            task_input,
                        ),
                        task_input,
                        strict_mode,
                    );
                    let (next, _streamed) = run_turn_for_mode(rt, cfg, mode, ui, &enforce_prompt);
                    cache_read_file_from_tool_result(file_synopsis_cache, &next);
                    if prompt_mode_short_circuits_provider_error(mode)
                        && next.stop_reason == "provider_error"
                    {
                        current = next;
                        break;
                    }
                    extra_cost += next.turn_cost_usd;
                    collect_native_changed_files(&next.native_tool_calls, changed_files);
                    steps += 1;
                    current = next;
                    if !is_auto_loop_continuable_stop_reason(&current.stop_reason) {
                        break;
                    }
                    continue;
                }
                current = repaired;
                break;
            }

            current = repaired;
            if !is_auto_loop_continuable_stop_reason(&current.stop_reason) {
                break;
            }
            continue;
        }

        let context_contract = format_context_contract_header(
            rt,
            strict_mode,
            speed,
            constraints,
            limits,
            previous_loop_stop_reason,
        );
        let anchored_context = context_with_task_anchor(&context_contract, task_input);
        if should_enforce_review_output_schema(task_input, &current.text)
            && review_schema_retries < REVIEW_SCHEMA_ENFORCEMENT_MAX_RETRIES
        {
            review_schema_retries += 1;
            let schema_error = review_output_schema_error(&current.text)
                .unwrap_or_else(|| "unknown schema mismatch".to_string());
            loop_info(
                mode,
                ui,
                &format!(
                    "auto-agent: review schema mismatch detected; retrying ({}/{})",
                    review_schema_retries, REVIEW_SCHEMA_ENFORCEMENT_MAX_RETRIES
                ),
            );
            let repair_prompt = build_review_schema_repair_prompt(
                &context_contract,
                task_input,
                strict_mode,
                &schema_error,
            );
            let (next, streamed) = run_turn_for_mode(rt, cfg, mode, ui, &repair_prompt);
            cache_read_file_from_tool_result(file_synopsis_cache, &next);
            render_turn_output_for_mode(mode, ui, markdown_render, &next, streamed, true);
            if prompt_mode_short_circuits_provider_error(mode)
                && next.stop_reason == "provider_error"
            {
                current = next;
                break;
            }
            extra_cost += next.turn_cost_usd;
            collect_native_changed_files(&next.native_tool_calls, changed_files);
            steps += 1;
            current = next;
            if !is_auto_loop_continuable_stop_reason(&current.stop_reason) {
                break;
            }
            continue;
        }
        if should_force_final_response_after_tool_result(&current)
            && is_auto_loop_continuable_stop_reason(&current.stop_reason)
        {
            if should_enforce_bilingual_markdown_reports(task_input, changed_files)
                && bilingual_md_retries < BILINGUAL_MD_ENFORCEMENT_MAX_RETRIES
            {
                bilingual_md_retries += 1;
                let enforce_prompt = build_bilingual_markdown_artifact_prompt(
                    &anchored_context,
                    task_input,
                    strict_mode,
                );
                let (next, _streamed) = run_turn_for_mode(rt, cfg, mode, ui, &enforce_prompt);
                cache_read_file_from_tool_result(file_synopsis_cache, &next);
                if prompt_mode_short_circuits_provider_error(mode)
                    && next.stop_reason == "provider_error"
                {
                    current = next;
                    break;
                }
                extra_cost += next.turn_cost_usd;
                collect_native_changed_files(&next.native_tool_calls, changed_files);
                steps += 1;
                current = next;
                if !is_auto_loop_continuable_stop_reason(&current.stop_reason) {
                    break;
                }
                continue;
            }
            let finalize_prompt = format!(
                "{}\n\nTool execution is complete. Provide the final user-facing answer now using existing tool results. Do not call additional tools unless absolutely necessary.",
                anchored_context
            );
            let (next, streamed) = run_turn_for_mode(rt, cfg, mode, ui, &finalize_prompt);
            cache_read_file_from_tool_result(file_synopsis_cache, &next);
            render_turn_output_for_mode(mode, ui, markdown_render, &next, streamed, true);
            if next.stop_reason == "provider_error" {
                current = next;
                break;
            }
            extra_cost += next.turn_cost_usd;
            collect_native_changed_files(&next.native_tool_calls, changed_files);
            steps += 1;
            current = next;
            if !is_auto_loop_continuable_stop_reason(&current.stop_reason) {
                break;
            }
            continue;
        }

        let batches = engine.parse_and_plan(&current.text);
        let toolcall_count = batches.iter().map(|b| b.calls.len()).sum::<usize>();
        let (risky_toolcall_count, safe_toolcall_count) = toolcall_count_by_risk(&batches);

        if strict_mode && risky_toolcall_count > 0 {
            confidence_gate_stats.checks += 1;
            let declaration = parse_confidence_declaration(&current.text);
            if declaration.is_none() {
                confidence_gate_stats.declaration_missing += 1;
                if confidence_gate_retries >= CONFIDENCE_GATE_MAX_RETRIES {
                    confidence_gate_stats.retries_exhausted += 1;
                    let reason = format!(
                        "confidence gate retries exhausted (missing declaration, threshold={})",
                        CONFIDENCE_GATE_MAX_RETRIES
                    );
                    loop_info(mode, ui, &format!("auto-agent stopped: {}", reason));
                    loop_stop_reason = Some(reason);
                    break;
                }
                confidence_gate_retries += 1;
                let gate_prompt = build_confidence_gate_prompt(
                    &anchored_context,
                    &current.text,
                    risky_toolcall_count,
                    safe_toolcall_count,
                );
                let gate_turn =
                    run_confidence_gate_pass_for_mode(rt, cfg, mode, ui, &gate_prompt);
                if gate_turn.stop_reason == "provider_error" {
                    render_turn_output_for_mode(mode, ui, markdown_render, &gate_turn, false, false);
                    current = gate_turn;
                    break;
                }
                extra_cost += gate_turn.turn_cost_usd;
                cache_read_file_from_tool_result(file_synopsis_cache, &gate_turn);
                current = gate_turn;
                continue;
            }
            if declaration
                .as_ref()
                .is_some_and(|d| d.level == ConfidenceLevel::Low)
            {
                confidence_gate_stats.declaration_low += 1;
                let mut blocked_any = false;
                for batch in &batches {
                    for call in &batch.calls {
                        if crate::orchestrator::confidence::is_risky_tool_name(call.name.as_str()) {
                            blocked_any = true;
                            confidence_gate_stats.blocked_risky_toolcalls += 1;
                            let reason = confidence_low_block_reason(declaration.as_ref());
                            let blocked = constrained_tool_result(rt, reason);
                            if is_tool_failure(&blocked) {
                                consecutive_failures += 1;
                                last_failure_category =
                                    Some(classify_tool_failure_result(&blocked));
                            }
                            consecutive_constraint_blocks += 1;
                            log_interaction_event(cfg, rt, &call.to_command(), &blocked);
                            render_turn_output_for_mode(
                                mode,
                                ui,
                                markdown_render,
                                &blocked,
                                false,
                                false,
                            );
                        }
                    }
                }
                if blocked_any {
                    if let Some(reason) = constraint_block_stop_reason_if_needed(
                        consecutive_constraint_blocks,
                        limits.max_consecutive_constraint_blocks,
                        Some(
                            "blocked by strict confidence gate: low confidence for risky toolcalls",
                        ),
                    ) {
                        loop_info(mode, ui, &format!("auto-agent stopped: {}", reason));
                        loop_stop_reason = Some(reason);
                        break;
                    }
                    let followup_base = engine.followup_prompt_for(strict_mode);
                    let followup_body = append_failure_memory_to_followup(
                        followup_base,
                        strict_mode,
                        last_failure_category,
                        &failure_memory,
                        &canonical_observations,
                    );
                    let followup_with_synopsis = append_file_synopsis_to_followup(
                        followup_body,
                        file_synopsis_cache,
                        strict_mode,
                    );
                    let followup = format!(
                        "{}\n\n{}\n\nConfidence Gate result: low confidence blocked risky tools. Next response must use only read/search/web tools to gather evidence.",
                        anchored_context, followup_with_synopsis
                    );
                    let (next, _streamed) = run_turn_for_mode(rt, cfg, mode, ui, &followup);
                    cache_read_file_from_tool_result(file_synopsis_cache, &next);
                    if prompt_mode_short_circuits_provider_error(mode)
                        && next.stop_reason == "provider_error"
                    {
                        current = next;
                        break;
                    }
                    extra_cost += next.turn_cost_usd;
                    current = next;
                    continue;
                }
            }
            confidence_gate_retries = 0;
        }

        if should_require_plan_handshake(strict_mode, &current.text, toolcall_count, steps)
            && !plan_handshake_done
        {
            let handshake_prompt = build_plan_handshake_prompt(&anchored_context);
            let (plan_turn, _streamed) = run_turn_for_mode(rt, cfg, mode, ui, &handshake_prompt);
            if plan_turn.stop_reason == "provider_error" {
                render_turn_output_for_mode(mode, ui, markdown_render, &plan_turn, false, false);
                current = plan_turn;
                break;
            }
            let plan_lines = collect_plan_lines(&plan_turn.text);
            if plan_lines.len() < 3 {
                let retry_prompt = format!(
                    "{}\n\nPlan Handshake validation failed (need 3-7 concise lines). Re-emit only the plan, no /toolcall lines.",
                    anchored_context
                );
                let retry = rt.run_turn(&retry_prompt);
                log_interaction_event(cfg, rt, &retry_prompt, &retry);
                let retry_lines = collect_plan_lines(&retry.text);
                if retry_lines.len() < 3 {
                    current = retry;
                    break;
                }
                current = retry;
            } else {
                current = plan_turn;
            }
            plan_handshake_done = true;
            continue;
        }

        if toolcall_count == 0
            && strict_mode
            && strict_forced_execution_attempts < 2
            && collect_plan_lines(&current.text).len() >= 3
            && !looks_like_execution_ready_text(&current.text)
        {
            let forced_execution_prompt = if strict_forced_execution_attempts == 0 {
                format!(
                    "{}\n\nPlan received. Execute now. Output ONLY /toolcall lines in this response. Do not restate the plan.",
                    anchored_context
                )
            } else {
                format!(
                    "{}\n\nExecution still missing. Output exactly ONE /toolcall bash line now (no prose), run it, then continue tool execution.",
                    anchored_context
                )
            };
            let (next, streamed) = run_turn_for_mode(rt, cfg, mode, ui, &forced_execution_prompt);
            cache_read_file_from_tool_result(file_synopsis_cache, &next);
            render_turn_output_for_mode(mode, ui, markdown_render, &next, streamed, true);
            if prompt_mode_short_circuits_provider_error(mode)
                && next.stop_reason == "provider_error"
            {
                current = next;
                break;
            }
            extra_cost += next.turn_cost_usd;
            collect_native_changed_files(&next.native_tool_calls, changed_files);
            steps += 1;
            strict_forced_execution_attempts += 1;
            current = next;
            if !is_auto_loop_continuable_stop_reason(&current.stop_reason) {
                break;
            }
            continue;
        }

        if toolcall_count == 0 {
            if should_enforce_bilingual_markdown_reports(task_input, changed_files)
                && bilingual_md_retries < BILINGUAL_MD_ENFORCEMENT_MAX_RETRIES
            {
                bilingual_md_retries += 1;
                let enforce_prompt = build_bilingual_markdown_artifact_prompt(
                    &anchored_context,
                    task_input,
                    strict_mode,
                );
                let (next, _streamed) = run_turn_for_mode(rt, cfg, mode, ui, &enforce_prompt);
                cache_read_file_from_tool_result(file_synopsis_cache, &next);
                if prompt_mode_short_circuits_provider_error(mode)
                    && next.stop_reason == "provider_error"
                {
                    current = next;
                    break;
                }
                extra_cost += next.turn_cost_usd;
                collect_native_changed_files(&next.native_tool_calls, changed_files);
                steps += 1;
                current = next;
                if !is_auto_loop_continuable_stop_reason(&current.stop_reason) {
                    break;
                }
                continue;
            }
            if should_attempt_zero_toolcall_nudge(zero_toolcall_nudge_attempted, steps, &current) {
                zero_toolcall_nudge_attempted = true;
                let nudge_prompt =
                    build_zero_toolcall_nudge_prompt(&anchored_context, task_input, strict_mode);
                let (nudged, streamed) = run_turn_for_mode(rt, cfg, mode, ui, &nudge_prompt);
                cache_read_file_from_tool_result(file_synopsis_cache, &nudged);
                render_turn_output_for_mode(mode, ui, markdown_render, &nudged, streamed, true);
                if nudged.stop_reason == "provider_error" {
                    current = nudged;
                    break;
                }
                extra_cost += nudged.turn_cost_usd;
                if matches!(mode, LoopMode::ReplVerbose) {
                    record_native_changed_files_for_mode(
                        mode,
                        &nudged.native_tool_calls,
                        changed_files,
                        change_events.as_deref_mut(),
                        ui,
                    );
                } else {
                    collect_native_changed_files(&nudged.native_tool_calls, changed_files);
                }
                steps += 1;
                current = nudged;
                if !is_auto_loop_continuable_stop_reason(&current.stop_reason) {
                    break;
                }
                continue;
            }
            if !malformed_repair_attempted && looks_like_malformed_toolcall_output(&current.text) {
                malformed_repair_attempted = true;
                loop_info(
                    mode,
                    ui,
                    "auto-agent: attempting one strict toolcall-format repair pass",
                );
                let repaired = run_repair_pass_for_mode(rt, cfg, mode, ui, strict_mode);
                if repaired.stop_reason == "provider_error" {
                    render_turn_output_for_mode(mode, ui, markdown_render, &repaired, false, false);
                    current = repaired;
                    break;
                }

                extra_cost += repaired.turn_cost_usd;
                if matches!(mode, LoopMode::ReplVerbose) {
                    record_native_changed_files_for_mode(
                        mode,
                        &repaired.native_tool_calls,
                        changed_files,
                        change_events.as_deref_mut(),
                        ui,
                    );
                } else {
                    collect_native_changed_files(&repaired.native_tool_calls, changed_files);
                }

                if repaired.text.trim() == "NO_TOOLCALLS" {
                    current = repaired;
                    break;
                }

                current = repaired;
                if !is_auto_loop_continuable_stop_reason(&current.stop_reason) {
                    break;
                }
                continue;
            }
            break;
        }

        loop_info(
            mode,
            ui,
            &format!("auto-agent: executing {} tool call(s)", toolcall_count),
        );

        for batch in batches {
            let can_run_parallel = batch.concurrency_safe
                && batch.calls.len() > 1
                && rt.next_permission_allow_rules.is_empty()
                && rt.next_additional_directories.is_empty()
                && !is_interactive_approval_mode(&rt.permission_mode);

            if can_run_parallel {
                let mut commands = Vec::new();
                for call in &batch.calls {
                    if let Some(reason) = engine.stop_reason(steps, consecutive_failures) {
                        loop_info(mode, ui, &format!("auto-agent stopped: {}", reason));
                        loop_stop_reason = Some(reason.to_string());
                        break;
                    }
                    commands.push(call.to_command());
                    steps += 1;
                }

                if !commands.is_empty() {
                    loop_info(
                        mode,
                        ui,
                        &format!(
                            "auto-agent: executing concurrent-safe batch in parallel size={}",
                            commands.len()
                        ),
                    );
                    for command in &commands {
                        loop_info(mode, ui, &format!("auto> {}", command));
                    }

                    let tool_results = run_parallel_batch_tool_results(rt, constraints, &commands);
                    for (idx, tool_result) in tool_results.into_iter().enumerate() {
                        let command = commands
                            .get(idx)
                            .cloned()
                            .unwrap_or_else(|| "/toolcall <unknown>".to_string());
                        if let Some(reason) = record_tool_result_observation(
                            cfg,
                            rt,
                            &command,
                            &tool_result,
                            file_synopsis_cache,
                            &mut canonical_observations,
                            &mut failure_memory,
                            &mut consecutive_failures,
                            &mut last_failure_category,
                            &mut consecutive_constraint_blocks,
                            limits.max_consecutive_constraint_blocks,
                        ) {
                            loop_info(mode, ui, &format!("auto-agent stopped: {}", reason));
                            loop_stop_reason = Some(reason);
                            break;
                        }

                        record_changed_files_from_tool_result(
                            mode,
                            &command,
                            &tool_result,
                            changed_files,
                            change_events.as_deref_mut(),
                            ui,
                        );
                        render_turn_output_for_mode(
                            mode,
                            ui,
                            markdown_render,
                            &tool_result,
                            false,
                            false,
                        );
                    }
                }
            } else {
                if batch.concurrency_safe && batch.calls.len() > 1 {
                    loop_info(
                        mode,
                        ui,
                        &format!(
                            "auto-agent: planned concurrent-safe batch size={} (executing sequentially due next-only permissions)",
                            batch.calls.len()
                        ),
                    );
                }

                for call in batch.calls {
                    if let Some(reason) = engine.stop_reason(steps, consecutive_failures) {
                        loop_info(mode, ui, &format!("auto-agent stopped: {}", reason));
                        loop_stop_reason = Some(reason.to_string());
                        break;
                    }

                    let command = call.to_command();
                    cache_read_file_from_command(file_synopsis_cache, &command);
                    steps += 1;
                    loop_info(mode, ui, &format!("auto> {}", command));

                    let (tool_result, stop_reason) = run_sequential_tool_call_with_observation(
                        rt,
                        cfg,
                        &command,
                        constraints,
                        file_synopsis_cache,
                        &mut canonical_observations,
                        &mut failure_memory,
                        &mut consecutive_failures,
                        &mut last_failure_category,
                        &mut consecutive_constraint_blocks,
                        limits.max_consecutive_constraint_blocks,
                    );
                    if let Some(reason) = stop_reason {
                        loop_info(mode, ui, &format!("auto-agent stopped: {}", reason));
                        loop_stop_reason = Some(reason);
                        break;
                    }

                    record_changed_files_from_tool_result(
                        mode,
                        &command,
                        &tool_result,
                        changed_files,
                        change_events.as_deref_mut(),
                        ui,
                    );
                    render_turn_output_for_mode(
                        mode,
                        ui,
                        markdown_render,
                        &tool_result,
                        false,
                        false,
                    );
                }
            }

            if let Some(reason) = engine.stop_reason(steps, consecutive_failures) {
                loop_info(mode, ui, &format!("auto-agent stopped: {}", reason));
                loop_stop_reason = Some(reason.to_string());
                break;
            }
        }

        if let Some(reason) = post_batch_stop_reason(
            &engine,
            steps,
            consecutive_failures,
            consecutive_constraint_blocks,
            limits.max_consecutive_constraint_blocks,
        ) {
            loop_info(mode, ui, &format!("auto-agent stopped: {}", reason));
            loop_stop_reason = Some(reason);
            break;
        }

        let followup = build_followup_prompt_for_loop(
            &engine,
            strict_mode,
            &anchored_context,
            last_failure_category,
            &failure_memory,
            &canonical_observations,
            file_synopsis_cache,
        );
        let (next, streamed) = run_turn_for_mode(rt, cfg, mode, ui, &followup);
        observe_assistant_followup_turn(
            file_synopsis_cache,
            &mut canonical_observations,
            &mut failure_memory,
            &next,
        );
        render_turn_output_for_mode(mode, ui, markdown_render, &next, streamed, true);

        if matches!(mode, LoopMode::ReplVerbose) {
            record_native_changed_files_for_mode(
                mode,
                &next.native_tool_calls,
                changed_files,
                change_events.as_deref_mut(),
                ui,
            );
        }
        if prompt_mode_short_circuits_provider_error(mode) && next.stop_reason == "provider_error" {
            current = next;
            break;
        }

        if let Some(reason) = apply_followup_turn_updates(
            &next,
            &mut extra_cost,
            changed_files,
            &mut last_progress_count,
            &mut no_progress_rounds,
            &mut last_failure_category,
            &mut consecutive_constraint_blocks,
            limits.max_consecutive_constraint_blocks,
        ) {
            loop_info(mode, ui, &format!("auto-agent stopped: {}", reason));
            loop_stop_reason = Some(reason);
            current = next;
            break;
        }
        current = next;

        if !is_auto_loop_continuable_stop_reason(&current.stop_reason) {
            break;
        }
    }

    (current, extra_cost, steps, loop_stop_reason)
}

fn run_auto_tool_loop(
    rt: &mut Runtime,
    cfg: &AppConfig,
    ui: &Ui,
    task_input: &str,
    markdown_render: bool,
    initial_result: &runtime::TurnResult,
    _max_steps: usize,
    changed_files_session: &mut Vec<String>,
    change_events_session: &mut Vec<String>,
    strict_mode: bool,
    speed: ExecutionSpeed,
    constraints: ToolExecutionConstraints,
    limits: AutoLoopLimits,
    previous_loop_stop_reason: Option<&str>,
    file_synopsis_cache: &mut FileSynopsisCache,
    confidence_gate_stats: &mut ConfidenceGateStats,
) -> (runtime::TurnResult, f64, Option<String>) {
    let (current, extra_cost, _steps, loop_stop_reason) = run_tool_loop_core(
        rt,
        cfg,
        task_input,
        markdown_render,
        initial_result,
        changed_files_session,
        Some(change_events_session),
        strict_mode,
        speed,
        constraints,
        limits,
        previous_loop_stop_reason,
        file_synopsis_cache,
        confidence_gate_stats,
        LoopMode::ReplVerbose,
        Some(ui),
    );
    (current, extra_cost, loop_stop_reason)
}

fn run_prompt_auto_tool_loop(
    rt: &mut Runtime,
    cfg: &AppConfig,
    task_input: &str,
    initial_result: &runtime::TurnResult,
    changed_files: &mut Vec<String>,
    strict_mode: bool,
    speed: ExecutionSpeed,
    constraints: ToolExecutionConstraints,
    limits: AutoLoopLimits,
    previous_loop_stop_reason: Option<&str>,
    file_synopsis_cache: &mut FileSynopsisCache,
    confidence_gate_stats: &mut ConfidenceGateStats,
) -> (runtime::TurnResult, f64, usize, Option<String>) {
    run_tool_loop_core(
        rt,
        cfg,
        task_input,
        false,
        initial_result,
        changed_files,
        None,
        strict_mode,
        speed,
        constraints,
        limits,
        previous_loop_stop_reason,
        file_synopsis_cache,
        confidence_gate_stats,
        LoopMode::PromptCompact,
        None,
    )
}
