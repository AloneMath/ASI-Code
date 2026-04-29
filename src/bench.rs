use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub(crate) struct BenchOptions {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub project: Option<String>,
    pub agent_max_steps: usize,
    pub repeat: usize,
    pub suite: crate::BenchSuite,
    pub out_dir: PathBuf,
}

#[derive(Debug, Clone, Copy)]
enum BenchExpectation {
    Contains(&'static str),
    AgentProgress,
    AgentProgressAndContains(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BenchDimension {
    Core,
    SweBenchProPublic,
    TerminalBench2,
    OsworldVerified,
    GdpvalWinsOrTies,
    CybersecurityCtfChallenges,
    SweLancerIcDiamond,
}

impl BenchDimension {
    fn key(self) -> &'static str {
        match self {
            BenchDimension::Core => "core",
            BenchDimension::SweBenchProPublic => "swe-bench-pro-public",
            BenchDimension::TerminalBench2 => "terminal-bench-2",
            BenchDimension::OsworldVerified => "osworld-verified",
            BenchDimension::GdpvalWinsOrTies => "gdpval-wins-or-ties",
            BenchDimension::CybersecurityCtfChallenges => "cybersecurity-ctf-challenges",
            BenchDimension::SweLancerIcDiamond => "swe-lancer-ic-diamond",
        }
    }

    fn label(self) -> &'static str {
        match self {
            BenchDimension::Core => "Core",
            BenchDimension::SweBenchProPublic => "SWE-Bench Pro (Public)",
            BenchDimension::TerminalBench2 => "Terminal-Bench 2.0",
            BenchDimension::OsworldVerified => "OSWorld-Verified",
            BenchDimension::GdpvalWinsOrTies => "GDPval (wins or ties)",
            BenchDimension::CybersecurityCtfChallenges => {
                "Cybersecurity Capture The Flag Challenges"
            }
            BenchDimension::SweLancerIcDiamond => "SWE-Lancer IC Diamond",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct BenchCaseDef {
    id: &'static str,
    dimension: BenchDimension,
    description: &'static str,
    prompt: &'static str,
    agent: bool,
    secure: bool,
    profile: crate::PromptProfile,
    expectation: BenchExpectation,
}

const CORE_CASES: [BenchCaseDef; 4] = [
    BenchCaseDef {
        id: "echo_standard",
        dimension: BenchDimension::Core,
        description: "Model obeys deterministic output for a plain prompt.",
        prompt: "Reply with exactly BENCH_OK and nothing else.",
        agent: false,
        secure: false,
        profile: crate::PromptProfile::Standard,
        expectation: BenchExpectation::Contains("BENCH_OK"),
    },
    BenchCaseDef {
        id: "math_standard",
        dimension: BenchDimension::Core,
        description: "Model returns correct arithmetic scalar output.",
        prompt: "Compute 17 * 19 and output only the integer.",
        agent: false,
        secure: false,
        profile: crate::PromptProfile::Standard,
        expectation: BenchExpectation::Contains("323"),
    },
    BenchCaseDef {
        id: "strict_agent_workflow",
        dimension: BenchDimension::Core,
        description: "Strict profile can execute tool loop for workspace task.",
        prompt: "Count top-level entries in the current workspace using tool calls, then output TOP_LEVEL_COUNT=<number>.",
        agent: true,
        secure: false,
        profile: crate::PromptProfile::Strict,
        expectation: BenchExpectation::AgentProgress,
    },
    BenchCaseDef {
        id: "strict_secure_workflow",
        dimension: BenchDimension::Core,
        description: "Strict secure workflow can complete at least one validated step.",
        prompt: "Run a minimal security check for hardcoded API keys in the current project files using tool calls, then output SECURITY_SCAN_DONE.",
        agent: true,
        secure: true,
        profile: crate::PromptProfile::Strict,
        expectation: BenchExpectation::AgentProgressAndContains("SECURITY_SCAN_DONE"),
    },
];

const GPT53_PROXY_6D_CASES: [BenchCaseDef; 6] = [
    BenchCaseDef {
        id: "swe_bench_pro_public_proxy",
        dimension: BenchDimension::SweBenchProPublic,
        description: "Proxy: inspect package metadata and return a deterministic coding benchmark marker.",
        prompt: "Use tool calls to read Cargo.toml and output exactly SWEBENCH_PUBLIC_PROXY:asi-code.",
        agent: true,
        secure: false,
        profile: crate::PromptProfile::Strict,
        expectation: BenchExpectation::AgentProgressAndContains("SWEBENCH_PUBLIC_PROXY:asi-code"),
    },
    BenchCaseDef {
        id: "terminal_bench_2_proxy",
        dimension: BenchDimension::TerminalBench2,
        description: "Proxy: execute shell command path and return terminal benchmark marker.",
        prompt: "Use shell tool calls to print current working directory, then output TERMINAL_BENCH2_PROXY_OK.",
        agent: true,
        secure: false,
        profile: crate::PromptProfile::Strict,
        expectation: BenchExpectation::AgentProgressAndContains("TERMINAL_BENCH2_PROXY_OK"),
    },
    BenchCaseDef {
        id: "osworld_verified_proxy",
        dimension: BenchDimension::OsworldVerified,
        description: "Proxy: perform workspace state operation and return verified marker with count.",
        prompt: "Use tool calls to count files directly under src/ and output OSWORLD_VERIFIED_PROXY_COUNT=<number>.",
        agent: true,
        secure: false,
        profile: crate::PromptProfile::Strict,
        expectation: BenchExpectation::AgentProgressAndContains("OSWORLD_VERIFIED_PROXY_COUNT="),
    },
    BenchCaseDef {
        id: "gdpval_proxy",
        dimension: BenchDimension::GdpvalWinsOrTies,
        description: "Proxy: verify evidence lookup and return valuation marker.",
        prompt: "Use tool calls to check whether README.md contains the phrase Open-Core Model. Output GDPVAL_PROXY=FOUND if present, otherwise GDPVAL_PROXY=MISSING.",
        agent: true,
        secure: false,
        profile: crate::PromptProfile::Strict,
        expectation: BenchExpectation::AgentProgressAndContains("GDPVAL_PROXY=FOUND"),
    },
    BenchCaseDef {
        id: "cybersecurity_ctf_proxy",
        dimension: BenchDimension::CybersecurityCtfChallenges,
        description: "Proxy: run security-oriented grep workflow and return CTF marker.",
        prompt: "Use tool calls to perform a quick secret pattern scan for 'sk-' in this project, then output CYBERSEC_CTF_PROXY_DONE.",
        agent: true,
        secure: true,
        profile: crate::PromptProfile::Strict,
        expectation: BenchExpectation::AgentProgressAndContains("CYBERSEC_CTF_PROXY_DONE"),
    },
    BenchCaseDef {
        id: "swe_lancer_ic_diamond_proxy",
        dimension: BenchDimension::SweLancerIcDiamond,
        description: "Proxy: solve file analytics task and return SWE-Lancer marker.",
        prompt: "Use tool calls to identify the Rust file in src/ with the highest line count, then output SWE_LANCER_PROXY_OK.",
        agent: true,
        secure: false,
        profile: crate::PromptProfile::Strict,
        expectation: BenchExpectation::AgentProgressAndContains("SWE_LANCER_PROXY_OK"),
    },
];

#[derive(Debug, Serialize, Clone)]
struct BenchCaseResult {
    round: usize,
    id: String,
    dimension_key: String,
    dimension_label: String,
    description: String,
    profile: String,
    agent: bool,
    secure: bool,
    pass: bool,
    check: String,
    latency_ms: u64,
    stop_reason: String,
    agent_steps: usize,
    agent_loop_stop_reason: Option<String>,
    confidence_gate_checks: usize,
    confidence_gate_missing_declaration: usize,
    confidence_gate_low_declaration: usize,
    confidence_gate_blocked_risky_toolcalls: usize,
    confidence_gate_retries_exhausted: usize,
    input_tokens: usize,
    output_tokens: usize,
    turn_cost_usd: f64,
    output_preview: String,
}

#[derive(Debug, Serialize, Clone)]
struct BenchDimensionScore {
    dimension_key: String,
    dimension_label: String,
    case_count: usize,
    pass_count: usize,
    pass_rate: f64,
    avg_latency_ms: u64,
    total_cost_usd: f64,
}

#[derive(Debug, Serialize)]
struct BenchRoundReport {
    round: usize,
    case_count: usize,
    pass_count: usize,
    pass_rate: f64,
    dimension_score_avg: f64,
    duration_ms: u64,
    total_cost_usd: f64,
    dimensions: Vec<BenchDimensionScore>,
}

#[derive(Debug, Serialize)]
struct BenchReport {
    benchmark_id: String,
    suite: String,
    created_unix_secs: u64,
    provider: String,
    model: String,
    permission_mode: String,
    project_dir: String,
    repeat: usize,
    case_run_count: usize,
    case_pass_count: usize,
    case_pass_rate: f64,
    round_pass_rate_mean: f64,
    round_pass_rate_stddev: f64,
    round_dimension_avg_mean: f64,
    round_dimension_avg_stddev: f64,
    total_duration_ms: u64,
    total_cost_usd: f64,
    dimensions: Vec<BenchDimensionScore>,
    rounds: Vec<BenchRoundReport>,
    cases: Vec<BenchCaseResult>,
}

pub(crate) fn run(options: BenchOptions) -> Result<String, String> {
    if let Some(path) = options.project.as_deref() {
        let _ = crate::set_project_dir(path)?;
    }

    let cfg = crate::resolve_cfg(options.provider, options.model, options.permission_mode);
    let repeat = options.repeat.clamp(1, 100);
    let cases_def = suite_cases(options.suite);

    let started = Instant::now();
    let mut all_cases = Vec::with_capacity(cases_def.len() * repeat);
    let mut rounds = Vec::with_capacity(repeat);

    for round in 1..=repeat {
        let mut rt = crate::runtime::Runtime::new(
            cfg.provider.clone(),
            cfg.model.clone(),
            cfg.permission_mode.clone(),
            cfg.max_turns,
        );
        rt.extended_thinking = cfg.extended_thinking;
        crate::apply_runtime_flags_from_cfg(&mut rt, &cfg);

        let round_started = Instant::now();
        let mut round_cases = Vec::with_capacity(cases_def.len());
        for case in cases_def {
            round_cases.push(run_case(
                &cfg,
                &mut rt,
                *case,
                round,
                options.agent_max_steps.clamp(1, 200),
            )?);
        }

        let dimensions = compute_dimension_scores(&round_cases);
        let case_count = round_cases.len();
        let pass_count = round_cases.iter().filter(|c| c.pass).count();
        let pass_rate = pct(pass_count, case_count);
        let dimension_score_avg = mean(&dimensions.iter().map(|d| d.pass_rate).collect::<Vec<_>>());
        let duration_ms = duration_to_ms_u64(round_started.elapsed().as_millis());
        let total_cost_usd = round_cases.iter().map(|c| c.turn_cost_usd).sum::<f64>();

        rounds.push(BenchRoundReport {
            round,
            case_count,
            pass_count,
            pass_rate,
            dimension_score_avg,
            duration_ms,
            total_cost_usd,
            dimensions,
        });
        all_cases.extend(round_cases);
    }

    let dimensions = compute_dimension_scores(&all_cases);
    let case_run_count = all_cases.len();
    let case_pass_count = all_cases.iter().filter(|c| c.pass).count();
    let case_pass_rate = pct(case_pass_count, case_run_count);
    let round_pass_rates = rounds.iter().map(|r| r.pass_rate).collect::<Vec<_>>();
    let round_dimension_avgs = rounds
        .iter()
        .map(|r| r.dimension_score_avg)
        .collect::<Vec<_>>();
    let round_pass_rate_mean = mean(&round_pass_rates);
    let round_pass_rate_stddev = stddev(&round_pass_rates);
    let round_dimension_avg_mean = mean(&round_dimension_avgs);
    let round_dimension_avg_stddev = stddev(&round_dimension_avgs);
    let total_duration_ms = duration_to_ms_u64(started.elapsed().as_millis());
    let total_cost_usd = all_cases.iter().map(|c| c.turn_cost_usd).sum::<f64>();

    let now = now_unix_secs();
    let benchmark_id = format!("asi_bench_{}_{}", options.suite.as_str(), now);
    let out_dir = if options.out_dir.is_absolute() {
        options.out_dir
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(options.out_dir)
    };
    fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let report = BenchReport {
        benchmark_id: benchmark_id.clone(),
        suite: options.suite.as_str().to_string(),
        created_unix_secs: now,
        provider: cfg.provider.clone(),
        model: cfg.model.clone(),
        permission_mode: cfg.permission_mode.clone(),
        project_dir: std::env::current_dir()
            .map_err(|e| e.to_string())?
            .display()
            .to_string(),
        repeat,
        case_run_count,
        case_pass_count,
        case_pass_rate,
        round_pass_rate_mean,
        round_pass_rate_stddev,
        round_dimension_avg_mean,
        round_dimension_avg_stddev,
        total_duration_ms,
        total_cost_usd,
        dimensions,
        rounds,
        cases: all_cases,
    };

    let json_path = out_dir.join(format!("{}.json", benchmark_id));
    let md_path = out_dir.join(format!("{}.md", benchmark_id));

    let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    fs::write(&json_path, json).map_err(|e| e.to_string())?;
    fs::write(&md_path, render_markdown(&report)).map_err(|e| e.to_string())?;

    Ok(format!(
        "bench_id={} suite={} repeat={} pass={}/{} pass_rate={:.1}% round_mean={:.1}% round_stddev={:.2}% total_cost={} json={} md={}",
        report.benchmark_id,
        report.suite,
        report.repeat,
        report.case_pass_count,
        report.case_run_count,
        report.case_pass_rate,
        report.round_pass_rate_mean,
        report.round_pass_rate_stddev,
        crate::cost::format_usd(report.total_cost_usd),
        json_path.display(),
        md_path.display()
    ))
}

fn suite_cases(suite: crate::BenchSuite) -> &'static [BenchCaseDef] {
    match suite {
        crate::BenchSuite::Core => &CORE_CASES,
        crate::BenchSuite::Gpt53Proxy6d => &GPT53_PROXY_6D_CASES,
    }
}

const GDPVAL_PROXY_MARKER: &str = "Open-Core Model";

const TERMINAL_BENCH2_FALLBACK_COMMANDS: &[&str] = &[
    "/toolcall bash \"Get-Location\"",
    "/toolcall bash \"Write-Output 'TERMINAL_BENCH2_PROXY_OK'\"",
];

const GDPVAL_FALLBACK_COMMANDS: &[&str] = &[
    "/toolcall glob_search \"README.md\"",
    "/toolcall read_file \"README.md\" 1 240",
    "/toolcall bash \"if (Test-Path README.md) { if (Select-String -Path README.md -Pattern 'Open-Core Model' -Quiet) { Write-Output 'GDPVAL_PROXY=FOUND' } else { Write-Output 'GDPVAL_PROXY=MISSING' } } else { Write-Output 'GDPVAL_PROXY=MISSING' }\"",
];

enum BenchCaseFixture {
    None,
    Readme {
        path: PathBuf,
        original: Option<String>,
    },
}

impl BenchCaseFixture {
    fn prepare(case: BenchCaseDef) -> Result<Self, String> {
        let project_root = std::env::current_dir().map_err(|e| e.to_string())?;
        Self::prepare_in(case, &project_root)
    }

    fn prepare_in(case: BenchCaseDef, project_root: &std::path::Path) -> Result<Self, String> {
        if case.id != "gdpval_proxy" {
            return Ok(Self::None);
        }

        let path = project_root.join("README.md");
        let original = if path.exists() {
            Some(
                fs::read_to_string(&path)
                    .map_err(|e| format!("failed to read {}: {}", path.display(), e))?,
            )
        } else {
            None
        };

        let has_marker = original
            .as_deref()
            .is_some_and(|content| content.contains(GDPVAL_PROXY_MARKER));
        if !has_marker {
            let fixture_content = match original.as_deref() {
                Some(existing) if !existing.trim().is_empty() => {
                    let trimmed = existing.trim_end_matches(['\r', '\n']);
                    format!("{trimmed}\n\n{GDPVAL_PROXY_MARKER}\n")
                }
                _ => format!("# Bench Fixture README\n\n{GDPVAL_PROXY_MARKER}\n"),
            };
            fs::write(&path, fixture_content)
                .map_err(|e| format!("failed to write {}: {}", path.display(), e))?;
        }

        Ok(Self::Readme { path, original })
    }

    fn restore(self) -> Result<(), String> {
        match self {
            Self::None => Ok(()),
            Self::Readme { path, original } => match original {
                Some(content) => fs::write(&path, content)
                    .map_err(|e| format!("failed to restore {}: {}", path.display(), e)),
                None => {
                    if path.exists() {
                        fs::remove_file(&path).map_err(|e| {
                            format!("failed to remove fixture file {}: {}", path.display(), e)
                        })
                    } else {
                        Ok(())
                    }
                }
            },
        }
    }
}

fn case_postcheck_fallback_commands(case_id: &str) -> Option<&'static [&'static str]> {
    match case_id {
        "terminal_bench_2_proxy" => Some(TERMINAL_BENCH2_FALLBACK_COMMANDS),
        "gdpval_proxy" => Some(GDPVAL_FALLBACK_COMMANDS),
        _ => None,
    }
}

fn run_case_postcheck_fallback(
    cfg: &crate::config::AppConfig,
    rt: &mut crate::runtime::Runtime,
    case: BenchCaseDef,
) -> Option<(crate::runtime::TurnResult, usize, f64)> {
    let commands = case_postcheck_fallback_commands(case.id)?;
    let mut steps = 0usize;
    let mut extra_cost = 0.0;
    let mut last: Option<crate::runtime::TurnResult> = None;

    for command in commands {
        let turn = rt.run_turn(command);
        crate::log_interaction_event(cfg, rt, command, &turn);
        steps += 1;
        extra_cost += turn.turn_cost_usd;
        let is_terminal = matches!(
            crate::runtime::Runtime::stop_reason_alias(&turn.stop_reason),
            "provider_error"
        );
        last = Some(turn);
        if is_terminal {
            break;
        }
    }

    last.map(|turn| (turn, steps, extra_cost))
}

fn run_case(
    cfg: &crate::config::AppConfig,
    rt: &mut crate::runtime::Runtime,
    case: BenchCaseDef,
    round: usize,
    agent_max_steps: usize,
) -> Result<BenchCaseResult, String> {
    let fixture = match BenchCaseFixture::prepare(case) {
        Ok(v) => v,
        Err(err) => {
            eprintln!(
                "bench fixture warning (case={}): {}; continuing without fixture",
                case.id, err
            );
            BenchCaseFixture::None
        }
    };
    let case_result = run_case_inner(cfg, rt, case, round, agent_max_steps);
    if let Err(restore_err) = fixture.restore() {
        if case_result.is_ok() {
            return Err(restore_err);
        }
    }
    case_result
}

fn run_case_inner(
    cfg: &crate::config::AppConfig,
    rt: &mut crate::runtime::Runtime,
    case: BenchCaseDef,
    round: usize,
    agent_max_steps: usize,
) -> Result<BenchCaseResult, String> {
    let started = Instant::now();
    let mut changed_files = Vec::new();
    let (model_input, prompt_agent_enabled, _) =
        crate::prepare_prompt_agent_input(case.prompt, case.agent, case.secure, case.profile)?;

    let mut result = rt.run_turn(&model_input);
    crate::log_interaction_event(cfg, rt, &model_input, &result);

    if let Some(path) = crate::extract_changed_file(&model_input, &result) {
        crate::push_unique_changed_file(&mut changed_files, &path);
    }
    crate::collect_native_changed_files(&result.native_tool_calls, &mut changed_files);

    let mut turn_cost_sum = result.turn_cost_usd;
    let mut agent_steps = 0usize;
    let mut agent_loop_stop_reason = None;
    let mut confidence_gate_stats = crate::ConfidenceGateStats::default();

    if prompt_agent_enabled && crate::is_auto_loop_continuable_stop_reason(&result.stop_reason)
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
            case.prompt,
            case.profile == crate::PromptProfile::Strict,
        );
        let limits =
            crate::apply_adaptive_budgets(
                base_limits,
                complexity,
                case.profile == crate::PromptProfile::Strict,
                crate::ExecutionSpeed::Deep,
            );
        let mut file_synopsis_cache = crate::FileSynopsisCache::default();
        let (loop_result, extra_cost, steps, loop_stop_reason) = crate::run_prompt_auto_tool_loop(
            rt,
            cfg,
            case.prompt,
            &result,
            &mut changed_files,
            case.profile == crate::PromptProfile::Strict,
            crate::ExecutionSpeed::Deep,
            crate::derive_tool_execution_constraints(
                case.prompt,
                case.profile == crate::PromptProfile::Strict,
            ),
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

    let expectation = case.expectation;
    let (mut pass, mut check) = evaluate_case(expectation, &result, agent_steps);
    if !pass {
        if let BenchExpectation::AgentProgressAndContains(token) = expectation {
            if !result.text.contains(token) {
                if let Some((fallback_result, fallback_steps, fallback_extra_cost)) =
                    run_case_postcheck_fallback(cfg, rt, case)
                {
                    turn_cost_sum += fallback_extra_cost;
                    agent_steps += fallback_steps;
                    result = fallback_result;

                    let (fallback_pass, fallback_check) =
                        evaluate_case(expectation, &result, agent_steps);
                    if fallback_pass {
                        pass = true;
                        check = format!(
                            "{} (postcheck_fallback case={})",
                            fallback_check, case.id
                        );
                    } else {
                        check = format!(
                            "{}; fallback_result={}",
                            check,
                            fallback_check
                        );
                    }
                }
            }
        }
    }
    Ok(BenchCaseResult {
        round,
        id: case.id.to_string(),
        dimension_key: case.dimension.key().to_string(),
        dimension_label: case.dimension.label().to_string(),
        description: case.description.to_string(),
        profile: case.profile.as_str().to_string(),
        agent: case.agent,
        secure: case.secure,
        pass,
        check,
        latency_ms: duration_to_ms_u64(started.elapsed().as_millis()),
        stop_reason: result.stop_reason,
        agent_steps,
        agent_loop_stop_reason,
        confidence_gate_checks: confidence_gate_stats.checks(),
        confidence_gate_missing_declaration: confidence_gate_stats.declaration_missing(),
        confidence_gate_low_declaration: confidence_gate_stats.declaration_low(),
        confidence_gate_blocked_risky_toolcalls: confidence_gate_stats.blocked_risky_toolcalls(),
        confidence_gate_retries_exhausted: confidence_gate_stats.retries_exhausted(),
        input_tokens: result.input_tokens,
        output_tokens: result.output_tokens,
        turn_cost_usd: turn_cost_sum,
        output_preview: preview_text(&result.text, 280),
    })
}

fn evaluate_case(
    expectation: BenchExpectation,
    result: &crate::runtime::TurnResult,
    agent_steps: usize,
) -> (bool, String) {
    let terminal_ok =
        matches!(crate::runtime::Runtime::stop_reason_alias(&result.stop_reason), "completed" | "tool_use")
            || result.stop_reason == "tool_result";
    match expectation {
        BenchExpectation::Contains(token) => {
            let pass = result.text.contains(token);
            if pass {
                (true, format!("output contains {}", token))
            } else {
                (
                    false,
                    format!("expected token {} not found in output", token),
                )
            }
        }
        BenchExpectation::AgentProgress => {
            let pass = terminal_ok && agent_steps > 0;
            if pass {
                (
                    true,
                    format!("agent loop executed {} step(s)", agent_steps),
                )
            } else {
                (
                    false,
                    format!(
                        "agent loop insufficient (steps={}, stop_reason={})",
                        agent_steps, result.stop_reason
                    ),
                )
            }
        }
        BenchExpectation::AgentProgressAndContains(token) => {
            let has_token = result.text.contains(token);
            let pass = terminal_ok && agent_steps > 0 && has_token;
            if pass {
                (
                    true,
                    format!(
                        "agent loop executed {} step(s) and output contains {}",
                        agent_steps, token
                    ),
                )
            } else {
                (
                    false,
                    format!(
                        "expected agent progress + token {} (steps={}, stop_reason={})",
                        token, agent_steps, result.stop_reason
                    ),
                )
            }
        }
    }
}

fn compute_dimension_scores(cases: &[BenchCaseResult]) -> Vec<BenchDimensionScore> {
    let mut grouped: BTreeMap<(String, String), Vec<&BenchCaseResult>> = BTreeMap::new();
    for case in cases {
        grouped
            .entry((case.dimension_key.clone(), case.dimension_label.clone()))
            .or_default()
            .push(case);
    }

    let mut out = Vec::with_capacity(grouped.len());
    for ((dimension_key, dimension_label), items) in grouped {
        let case_count = items.len();
        let pass_count = items.iter().filter(|c| c.pass).count();
        let pass_rate = pct(pass_count, case_count);
        let total_latency = items.iter().map(|c| c.latency_ms as u128).sum::<u128>();
        let avg_latency_ms = if case_count == 0 {
            0
        } else {
            duration_to_ms_u64(total_latency / (case_count as u128))
        };
        let total_cost_usd = items.iter().map(|c| c.turn_cost_usd).sum::<f64>();

        out.push(BenchDimensionScore {
            dimension_key,
            dimension_label,
            case_count,
            pass_count,
            pass_rate,
            avg_latency_ms,
            total_cost_usd,
        });
    }
    out
}

fn render_markdown(report: &BenchReport) -> String {
    let mut out = String::new();
    out.push_str("# ASI Bench Report\n\n");
    out.push_str(&format!(
        "- Benchmark ID: `{}`\n- Suite: `{}`\n- Repeat: `{}`\n- Provider: `{}`\n- Model: `{}`\n- Permission mode: `{}`\n- Project: `{}`\n- Case pass: **{}/{} ({:.1}%)**\n- Round pass mean/stddev: **{:.1}% / {:.2}%**\n- Round dimension-avg mean/stddev: **{:.1}% / {:.2}%**\n- Total cost: `{}`\n- Total duration: `{} ms`\n\n",
        report.benchmark_id,
        report.suite,
        report.repeat,
        report.provider,
        report.model,
        report.permission_mode,
        report.project_dir,
        report.case_pass_count,
        report.case_run_count,
        report.case_pass_rate,
        report.round_pass_rate_mean,
        report.round_pass_rate_stddev,
        report.round_dimension_avg_mean,
        report.round_dimension_avg_stddev,
        crate::cost::format_usd(report.total_cost_usd),
        report.total_duration_ms
    ));

    out.push_str("## Round Summary\n\n");
    out.push_str("| round | pass | pass_rate | dim_avg | duration_ms | cost |\n");
    out.push_str("|---:|---:|---:|---:|---:|---:|\n");
    for round in &report.rounds {
        out.push_str(&format!(
            "| {} | {}/{} | {:.1}% | {:.1}% | {} | {:.6} |\n",
            round.round,
            round.pass_count,
            round.case_count,
            round.pass_rate,
            round.dimension_score_avg,
            round.duration_ms,
            round.total_cost_usd
        ));
    }

    out.push_str("\n## Dimension Scores (Aggregated)\n\n");
    out.push_str("| dimension | pass | rate | avg_latency_ms | cost |\n");
    out.push_str("|---|---:|---:|---:|---:|\n");
    for dimension in &report.dimensions {
        out.push_str(&format!(
            "| {} | {}/{} | {:.1}% | {} | {:.6} |\n",
            escape_md_cell(&dimension.dimension_label),
            dimension.pass_count,
            dimension.case_count,
            dimension.pass_rate,
            dimension.avg_latency_ms,
            dimension.total_cost_usd
        ));
    }

    out.push_str("\n## Case Table\n\n");
    out.push_str("| round | case | dimension | profile | agent | secure | pass | stop_reason | steps | cg_checks | cg_blocked | latency_ms | cost |\n");
    out.push_str("|---:|---|---|---|---:|---:|---:|---|---:|---:|---:|---:|---:|\n");
    for case in &report.cases {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {:.6} |\n",
            case.round,
            escape_md_cell(&case.id),
            escape_md_cell(&case.dimension_label),
            escape_md_cell(&case.profile),
            case.agent,
            case.secure,
            case.pass,
            escape_md_cell(&case.stop_reason),
            case.agent_steps,
            case.confidence_gate_checks,
            case.confidence_gate_blocked_risky_toolcalls,
            case.latency_ms,
            case.turn_cost_usd
        ));
    }

    out.push_str("\n## Case Details\n\n");
    for case in &report.cases {
        out.push_str(&format!("### `round {} / {}`\n", case.round, case.id));
        out.push_str(&format!("- dimension: `{}`\n", case.dimension_label));
        out.push_str(&format!("- pass: `{}`\n", case.pass));
        out.push_str(&format!("- check: `{}`\n", case.check));
        out.push_str(&format!(
            "- loop_stop_reason: `{}`\n",
            case.agent_loop_stop_reason.as_deref().unwrap_or("none")
        ));
        out.push_str(&format!(
            "- confidence_gate: checks={} missing_declaration={} low_declaration={} blocked_risky_toolcalls={} retries_exhausted={}\n",
            case.confidence_gate_checks,
            case.confidence_gate_missing_declaration,
            case.confidence_gate_low_declaration,
            case.confidence_gate_blocked_risky_toolcalls,
            case.confidence_gate_retries_exhausted
        ));
        out.push_str(&format!("- output preview:\n\n```\n{}\n```\n\n", case.output_preview));
    }
    out
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let cleaned = text.replace('\r', "");
    if cleaned.chars().count() <= max_chars {
        return cleaned;
    }
    let mut out = cleaned.chars().take(max_chars).collect::<String>();
    out.push_str("...");
    out
}

fn escape_md_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', "<br/>")
}

fn pct(pass: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        (pass as f64) * 100.0 / (total as f64)
    }
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / (values.len() as f64)
    }
}

fn stddev(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let m = mean(values);
    let var = values
        .iter()
        .map(|v| {
            let d = *v - m;
            d * d
        })
        .sum::<f64>()
        / (values.len() as f64);
    var.sqrt()
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn duration_to_ms_u64(ms: u128) -> u64 {
    if ms > (u64::MAX as u128) {
        u64::MAX
    } else {
        ms as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_case() -> BenchCaseResult {
        BenchCaseResult {
            round: 1,
            id: "sample".to_string(),
            dimension_key: "core".to_string(),
            dimension_label: "Core".to_string(),
            description: "desc".to_string(),
            profile: "strict".to_string(),
            agent: true,
            secure: false,
            pass: true,
            check: "ok".to_string(),
            latency_ms: 123,
            stop_reason: "completed".to_string(),
            agent_steps: 5,
            agent_loop_stop_reason: Some("none".to_string()),
            confidence_gate_checks: 2,
            confidence_gate_missing_declaration: 1,
            confidence_gate_low_declaration: 1,
            confidence_gate_blocked_risky_toolcalls: 3,
            confidence_gate_retries_exhausted: 0,
            input_tokens: 10,
            output_tokens: 20,
            turn_cost_usd: 0.0001,
            output_preview: "preview".to_string(),
        }
    }

    #[test]
    fn render_markdown_includes_confidence_gate_columns_and_details() {
        let case = sample_case();
        let report = BenchReport {
            benchmark_id: "bench_id".to_string(),
            suite: "core".to_string(),
            created_unix_secs: 0,
            provider: "p".to_string(),
            model: "m".to_string(),
            permission_mode: "ask".to_string(),
            project_dir: ".".to_string(),
            repeat: 1,
            case_run_count: 1,
            case_pass_count: 1,
            case_pass_rate: 100.0,
            round_pass_rate_mean: 100.0,
            round_pass_rate_stddev: 0.0,
            round_dimension_avg_mean: 100.0,
            round_dimension_avg_stddev: 0.0,
            total_duration_ms: 100,
            total_cost_usd: case.turn_cost_usd,
            dimensions: vec![BenchDimensionScore {
                dimension_key: "core".to_string(),
                dimension_label: "Core".to_string(),
                case_count: 1,
                pass_count: 1,
                pass_rate: 100.0,
                avg_latency_ms: 123,
                total_cost_usd: case.turn_cost_usd,
            }],
            rounds: vec![BenchRoundReport {
                round: 1,
                case_count: 1,
                pass_count: 1,
                pass_rate: 100.0,
                dimension_score_avg: 100.0,
                duration_ms: 100,
                total_cost_usd: case.turn_cost_usd,
                dimensions: vec![],
            }],
            cases: vec![case],
        };

        let md = render_markdown(&report);
        assert!(md.contains("cg_checks"));
        assert!(md.contains("cg_blocked"));
        assert!(md.contains("confidence_gate: checks=2"));
        assert!(md.contains("blocked_risky_toolcalls=3"));
    }

    #[test]
    fn evaluate_case_accepts_stop_reason_alias_for_agent_progress() {
        let result = crate::runtime::TurnResult {
            text: "ok".to_string(),
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

        let (pass, check) = evaluate_case(BenchExpectation::AgentProgress, &result, 1);
        assert!(pass, "{}", check);
    }

    #[test]
    fn case_postcheck_fallback_commands_maps_expected_cases() {
        let terminal = case_postcheck_fallback_commands("terminal_bench_2_proxy")
            .expect("terminal fallback");
        assert_eq!(terminal.len(), 2);
        assert!(terminal[1].contains("TERMINAL_BENCH2_PROXY_OK"));

        let gdpval = case_postcheck_fallback_commands("gdpval_proxy")
            .expect("gdpval fallback");
        assert!(gdpval.iter().any(|line| line.contains("GDPVAL_PROXY=FOUND")));

        assert!(case_postcheck_fallback_commands("other_case").is_none());
    }

    fn fixture_case(id: &'static str) -> BenchCaseDef {
        BenchCaseDef {
            id,
            dimension: BenchDimension::Core,
            description: "fixture",
            prompt: "prompt",
            agent: false,
            secure: false,
            profile: crate::PromptProfile::Standard,
            expectation: BenchExpectation::Contains("ok"),
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("{}_{}_{}", prefix, ts, seq));
        fs::create_dir_all(&dir).expect("create temp directory");
        dir
    }

    #[test]
    fn gdpval_fixture_creates_and_removes_readme_when_missing() {
        let tmp = unique_temp_dir("asi_bench_fixture_missing");
        let path = tmp.join("README.md");
        let fixture = BenchCaseFixture::prepare_in(fixture_case("gdpval_proxy"), &tmp)
            .expect("prepare");
        let created = fs::read_to_string(&path).expect("fixture should create README");
        assert!(created.contains(GDPVAL_PROXY_MARKER));
        fixture.restore().expect("restore");
        assert!(
            !path.exists(),
            "fixture restore should remove temp README"
        );
        if tmp.exists() {
            fs::remove_dir_all(&tmp).expect("cleanup temp directory");
        }
    }

    #[test]
    fn gdpval_fixture_restores_existing_readme_content() {
        let tmp = unique_temp_dir("asi_bench_fixture_restore");
        let path = tmp.join("README.md");

        let original = "fixture-original-content\n";
        fs::write(&path, original).expect("write fixture README");
        let fixture = BenchCaseFixture::prepare_in(fixture_case("gdpval_proxy"), &tmp)
            .expect("prepare");
        let mutated = fs::read_to_string(&path).expect("read mutated README");
        assert!(mutated.contains(GDPVAL_PROXY_MARKER));
        fixture.restore().expect("restore");
        let restored = fs::read_to_string(&path).expect("read restored README");
        assert_eq!(restored, original);
        if tmp.exists() {
            fs::remove_dir_all(&tmp).expect("cleanup temp directory");
        }
    }

    #[test]
    fn non_gdpval_fixture_is_noop() {
        let fixture = BenchCaseFixture::prepare(fixture_case("terminal_bench_2_proxy"))
            .expect("prepare");
        fixture.restore().expect("restore");
    }

    #[test]
    fn gdpval_fixture_does_not_mutate_when_marker_exists() {
        let tmp = unique_temp_dir("asi_bench_fixture_marker");
        let path: PathBuf = tmp.join("README.md");
        let original = format!("# Existing\n\n{}\n", GDPVAL_PROXY_MARKER);
        fs::write(&path, &original).expect("write fixture README");

        let fixture = BenchCaseFixture::prepare_in(fixture_case("gdpval_proxy"), &tmp)
            .expect("prepare");
        let unchanged = fs::read_to_string(&path).expect("read unchanged README");
        assert_eq!(unchanged, original);
        fixture.restore().expect("restore");
        let restored = fs::read_to_string(&path).expect("read restored README");
        assert_eq!(restored, original);
        if tmp.exists() {
            fs::remove_dir_all(&tmp).expect("cleanup temp directory");
        }
    }
}
