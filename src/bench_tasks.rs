//! SWE-style private task suite (`asi bench --suite tasks`).
//!
//! Each task is a tiny self-contained repository (Python, stdlib only) with
//! a natural-language request. The agent works on a fresh copy; afterwards
//! the harness writes hidden tests the agent never saw and runs them. Pass
//! or fail is decided by the verifier's exit code, never by the model's own
//! claims. Reports can be diffed against a baseline to catch regressions
//! whenever prompts or the agent loop change.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct TaskFile {
    path: &'static str,
    content: &'static str,
    /// Write with CRLF line endings (exercises edit reliability).
    crlf: bool,
}

const fn lf(path: &'static str, content: &'static str) -> TaskFile {
    TaskFile { path, content, crlf: false }
}

const fn crlf(path: &'static str, content: &'static str) -> TaskFile {
    TaskFile { path, content, crlf: true }
}

type ExtraCheck = fn(&Path) -> Result<(), String>;

struct TaskDef {
    id: &'static str,
    category: &'static str,
    prompt: &'static str,
    files: &'static [TaskFile],
    hidden_test: &'static str,
    /// Files that make the task pass; used only by self-tests.
    #[cfg_attr(not(test), allow(dead_code))]
    reference: &'static [TaskFile],
    extra_check: Option<ExtraCheck>,
}

const HIDDEN_TEST_FILE: &str = "test_asi_hidden.py";

const TASKS: &[TaskDef] = &[
    TaskDef {
        id: "fix_median_even",
        category: "bugfix",
        prompt: "The median() function in stats.py returns the wrong value for lists with an even number of elements (median([1, 2, 3, 4]) should be 2.5). Fix it. Do not change mean().",
        files: &[lf(
            "stats.py",
            "def mean(values):\n    return sum(values) / len(values)\n\n\ndef median(values):\n    ordered = sorted(values)\n    mid = len(ordered) // 2\n    return ordered[mid]\n",
        )],
        hidden_test: "import unittest\nfrom stats import mean, median\n\n\nclass T(unittest.TestCase):\n    def test_odd(self):\n        self.assertEqual(median([3, 1, 2]), 2)\n\n    def test_even(self):\n        self.assertEqual(median([4, 1, 3, 2]), 2.5)\n        self.assertEqual(median([10, 20]), 15)\n\n    def test_mean_unchanged(self):\n        self.assertEqual(mean([1, 2, 3]), 2)\n",
        reference: &[lf(
            "stats.py",
            "def mean(values):\n    return sum(values) / len(values)\n\n\ndef median(values):\n    ordered = sorted(values)\n    mid = len(ordered) // 2\n    if len(ordered) % 2 == 0:\n        return (ordered[mid - 1] + ordered[mid]) / 2\n    return ordered[mid]\n",
        )],
        extra_check: None,
    },
    TaskDef {
        id: "crlf_config_edit",
        category: "edit-reliability",
        prompt: "In settings.py change the default timeout to 60 and add a new default \"backoff\": 1.5 to DEFAULTS. Keep everything else unchanged.",
        files: &[crlf(
            "settings.py",
            "DEFAULTS = {\n    \"timeout\": 30,\n    \"retries\": 3,\n}\n\n\ndef get(key):\n    return DEFAULTS[key]\n",
        )],
        hidden_test: "import unittest\nfrom settings import get\n\n\nclass T(unittest.TestCase):\n    def test_values(self):\n        self.assertEqual(get(\"timeout\"), 60)\n        self.assertEqual(get(\"retries\"), 3)\n        self.assertEqual(get(\"backoff\"), 1.5)\n",
        reference: &[crlf(
            "settings.py",
            "DEFAULTS = {\n    \"timeout\": 60,\n    \"retries\": 3,\n    \"backoff\": 1.5,\n}\n\n\ndef get(key):\n    return DEFAULTS[key]\n",
        )],
        extra_check: Some(check_settings_still_crlf),
    },
    TaskDef {
        id: "rename_across_files",
        category: "multi-file-refactor",
        prompt: "Rename the function calc_total to compute_total everywhere in this project (its definition and every call site and import). Behavior must not change.",
        files: &[
            lf("pricing.py", "def calc_total(items):\n    return sum(price * qty for price, qty in items)\n"),
            lf("cart.py", "from pricing import calc_total\n\n\ndef cart_total(cart):\n    return calc_total(list(cart.values()))\n"),
            lf("report.py", "from pricing import calc_total\n\n\ndef report(items):\n    return f\"Total: {calc_total(items):.2f}\"\n"),
        ],
        hidden_test: "import unittest\nfrom pricing import compute_total\nfrom cart import cart_total\nfrom report import report\n\n\nclass T(unittest.TestCase):\n    def test_behavior(self):\n        self.assertEqual(compute_total([(2.0, 3), (1.5, 2)]), 9.0)\n        self.assertEqual(cart_total({\"a\": (2.0, 1), \"b\": (3.0, 2)}), 8.0)\n        self.assertEqual(report([(1.0, 1)]), \"Total: 1.00\")\n",
        reference: &[
            lf("pricing.py", "def compute_total(items):\n    return sum(price * qty for price, qty in items)\n"),
            lf("cart.py", "from pricing import compute_total\n\n\ndef cart_total(cart):\n    return compute_total(list(cart.values()))\n"),
            lf("report.py", "from pricing import compute_total\n\n\ndef report(items):\n    return f\"Total: {compute_total(items):.2f}\"\n"),
        ],
        extra_check: Some(check_no_calc_total_left),
    },
    TaskDef {
        id: "implement_slugify",
        category: "feature",
        prompt: "Implement slugify() in textutil.py according to its docstring.",
        files: &[lf(
            "textutil.py",
            "def slugify(text):\n    \"\"\"Convert text to a URL slug.\n\n    Rules:\n    - lowercase\n    - every run of characters that are not a-z or 0-9 becomes a single \"-\"\n    - no leading or trailing \"-\"\n    \"\"\"\n    raise NotImplementedError\n",
        )],
        hidden_test: "import unittest\nfrom textutil import slugify\n\n\nclass T(unittest.TestCase):\n    def test_rules(self):\n        self.assertEqual(slugify(\"Hello, World!\"), \"hello-world\")\n        self.assertEqual(slugify(\"  --Rust  2026--\"), \"rust-2026\")\n        self.assertEqual(slugify(\"a__b\"), \"a-b\")\n        self.assertEqual(slugify(\"\"), \"\")\n",
        reference: &[lf(
            "textutil.py",
            "import re\n\n\ndef slugify(text):\n    return re.sub(r\"[^a-z0-9]+\", \"-\", text.lower()).strip(\"-\")\n",
        )],
        extra_check: None,
    },
    TaskDef {
        id: "ambiguous_duplicate_edit",
        category: "edit-precision",
        prompt: "In shapes.py, make area_rectangle return twice its current value by changing its scale to 2. area_square must keep returning side * side.",
        files: &[lf(
            "shapes.py",
            "def area_square(side):\n    scale = 1\n    return side * side * scale\n\n\ndef area_rectangle(w, h):\n    scale = 1\n    return w * h * scale\n",
        )],
        hidden_test: "import unittest\nfrom shapes import area_rectangle, area_square\n\n\nclass T(unittest.TestCase):\n    def test_values(self):\n        self.assertEqual(area_square(3), 9)\n        self.assertEqual(area_rectangle(2, 3), 12)\n",
        reference: &[lf(
            "shapes.py",
            "def area_square(side):\n    scale = 1\n    return side * side * scale\n\n\ndef area_rectangle(w, h):\n    scale = 2\n    return w * h * scale\n",
        )],
        extra_check: None,
    },
    TaskDef {
        id: "cross_module_bug",
        category: "diagnosis",
        prompt: "python run.py prints TOTAL 12 for orders.csv, but the correct total is 17. Find the root cause and fix it. Do not hardcode the answer.",
        files: &[
            lf("orders.csv", "apple, 5\npear, 10\n# restock later\nplum, 2\n"),
            lf("pipeline/__init__.py", ""),
            lf("pipeline/load.py", "def read_lines(path):\n    with open(path, encoding=\"utf-8\") as f:\n        lines = f.read().splitlines()\n    return lines[1:]\n"),
            lf("pipeline/parse.py", "def parse_line(line):\n    name, qty = line.split(\",\")\n    return name.strip(), int(qty)\n"),
            lf("pipeline/summarize.py", "from pipeline.load import read_lines\nfrom pipeline.parse import parse_line\n\n\ndef total_for(path):\n    total = 0\n    for line in read_lines(path):\n        if not line.strip() or line.startswith(\"#\"):\n            continue\n        total += parse_line(line)[1]\n    return total\n"),
            lf("run.py", "from pipeline.summarize import total_for\n\nprint(\"TOTAL\", total_for(\"orders.csv\"))\n"),
        ],
        hidden_test: "import os\nimport tempfile\nimport unittest\nfrom pipeline.summarize import total_for\n\n\nclass T(unittest.TestCase):\n    def test_fixture(self):\n        self.assertEqual(total_for(\"orders.csv\"), 17)\n\n    def test_fresh_file(self):\n        fd, path = tempfile.mkstemp(suffix=\".csv\")\n        with os.fdopen(fd, \"w\", encoding=\"utf-8\") as f:\n            f.write(\"x, 1\\ny, 2\\nz, 4\\n\")\n        try:\n            self.assertEqual(total_for(path), 7)\n        finally:\n            os.remove(path)\n",
        reference: &[lf(
            "pipeline/load.py",
            "def read_lines(path):\n    with open(path, encoding=\"utf-8\") as f:\n        return f.read().splitlines()\n",
        )],
        extra_check: None,
    },
];

fn check_settings_still_crlf(dir: &Path) -> Result<(), String> {
    let text = fs::read_to_string(dir.join("settings.py")).map_err(|e| e.to_string())?;
    let bare_lf = text.matches('\n').count() - text.matches("\r\n").count();
    if bare_lf > 0 {
        return Err(format!("settings.py now has {} bare LF line ending(s); expected CRLF only", bare_lf));
    }
    Ok(())
}

fn check_no_calc_total_left(dir: &Path) -> Result<(), String> {
    for name in ["pricing.py", "cart.py", "report.py"] {
        let text = fs::read_to_string(dir.join(name)).map_err(|e| e.to_string())?;
        if text.contains("calc_total") {
            return Err(format!("{} still references calc_total", name));
        }
    }
    Ok(())
}

fn write_task_file(dir: &Path, f: &TaskFile) -> Result<(), String> {
    let path = dir.join(f.path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let content = if f.crlf {
        f.content.replace('\n', "\r\n")
    } else {
        f.content.to_string()
    };
    fs::write(&path, content).map_err(|e| format!("write {}: {}", path.display(), e))
}

fn materialize(task: &TaskDef, dir: &Path) -> Result<(), String> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for f in task.files {
        write_task_file(dir, f)?;
    }
    Ok(())
}

fn python_bin() -> String {
    std::env::var("ASI_BENCH_PYTHON")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "python".to_string())
}

/// Write the hidden test (overwriting anything the agent created with the
/// same name) and run it. Returns (passed, verifier output tail).
fn verify(task: &TaskDef, dir: &Path) -> (bool, String) {
    if let Err(e) = write_task_file(dir, &lf(HIDDEN_TEST_FILE, task.hidden_test)) {
        return (false, e);
    }
    let module = HIDDEN_TEST_FILE.trim_end_matches(".py");
    let spawned = Command::new(python_bin())
        .args(["-m", "unittest", "-q", module])
        .current_dir(dir)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return (false, format!("failed to start verifier ({}): {}", python_bin(), e)),
    };
    let started = Instant::now();
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) if started.elapsed() > Duration::from_secs(120) => {
                let _ = child.kill();
                break true;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break false,
        }
    };
    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return (false, e.to_string()),
    };
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let _ = fs::remove_file(dir.join(HIDDEN_TEST_FILE));
    if timed_out {
        return (false, format!("verifier timed out\n{}", tail(&text, 1500)));
    }
    let mut passed = output.status.success();
    if passed {
        if let Some(check) = task.extra_check {
            if let Err(e) = check(dir) {
                passed = false;
                text.push_str(&format!("\nextra check failed: {}", e));
            }
        }
    }
    (passed, tail(&text, 1500))
}

fn tail(s: &str, max_chars: usize) -> String {
    let n = s.chars().count();
    if n <= max_chars {
        s.trim().to_string()
    } else {
        format!("...{}", s.chars().skip(n - max_chars).collect::<String>().trim())
    }
}

#[derive(Debug, Serialize, Clone)]
struct TaskRunResult {
    id: String,
    category: String,
    round: usize,
    pass: bool,
    verifier_output: String,
    agent_steps: usize,
    stop_reason: String,
    agent_loop_stop_reason: Option<String>,
    changed_files: Vec<String>,
    duration_ms: u64,
    cost_usd: f64,
    workdir: String,
    final_answer_preview: String,
}

#[derive(Debug, Serialize, Clone)]
struct TaskSummary {
    id: String,
    category: String,
    runs: usize,
    passes: usize,
    pass_rate: f64,
    avg_steps: f64,
    avg_duration_ms: u64,
}

#[derive(Debug, Serialize)]
struct BaselineDiff {
    baseline_path: String,
    baseline_pass_rate: f64,
    regressions: Vec<String>,
    improvements: Vec<String>,
}

#[derive(Debug, Serialize)]
struct TasksReport {
    benchmark_id: String,
    suite: String,
    provider: String,
    model: String,
    repeat: usize,
    task_count: usize,
    run_count: usize,
    pass_count: usize,
    pass_rate: f64,
    total_duration_ms: u64,
    total_cost_usd: f64,
    tasks: Vec<TaskSummary>,
    runs: Vec<TaskRunResult>,
    baseline: Option<BaselineDiff>,
}

fn selected_tasks(only: Option<&str>) -> Result<Vec<&'static TaskDef>, String> {
    let Some(list) = only.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(TASKS.iter().collect());
    };
    let mut out = Vec::new();
    for id in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match TASKS.iter().find(|t| t.id == id) {
            Some(t) => out.push(t),
            None => {
                let known = TASKS.iter().map(|t| t.id).collect::<Vec<_>>().join(", ");
                return Err(format!("unknown task id `{}` (known: {})", id, known));
            }
        }
    }
    Ok(out)
}

/// Per-task pass rates from a previous report JSON.
fn load_baseline(path: &Path) -> Result<(f64, BTreeMap<String, f64>), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read baseline {}: {}", path.display(), e))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("parse baseline: {}", e))?;
    let overall = v.get("pass_rate").and_then(Value::as_f64).unwrap_or(0.0);
    let mut per_task = BTreeMap::new();
    for t in v.get("tasks").and_then(Value::as_array).into_iter().flatten() {
        if let (Some(id), Some(rate)) = (
            t.get("id").and_then(Value::as_str),
            t.get("pass_rate").and_then(Value::as_f64),
        ) {
            per_task.insert(id.to_string(), rate);
        }
    }
    Ok((overall, per_task))
}

fn diff_against_baseline(
    path: &Path,
    tasks: &[TaskSummary],
) -> Result<BaselineDiff, String> {
    let (overall, before) = load_baseline(path)?;
    let mut regressions = Vec::new();
    let mut improvements = Vec::new();
    for t in tasks {
        if let Some(prev) = before.get(&t.id) {
            if t.pass_rate + 1e-9 < *prev {
                regressions.push(format!("{}: {:.0}% -> {:.0}%", t.id, prev, t.pass_rate));
            } else if t.pass_rate > *prev + 1e-9 {
                improvements.push(format!("{}: {:.0}% -> {:.0}%", t.id, prev, t.pass_rate));
            }
        }
    }
    Ok(BaselineDiff {
        baseline_path: path.display().to_string(),
        baseline_pass_rate: overall,
        regressions,
        improvements,
    })
}

fn summarize(runs: &[TaskRunResult], tasks: &[&TaskDef]) -> Vec<TaskSummary> {
    tasks
        .iter()
        .map(|t| {
            let mine: Vec<&TaskRunResult> = runs.iter().filter(|r| r.id == t.id).collect();
            let n = mine.len().max(1);
            let passes = mine.iter().filter(|r| r.pass).count();
            TaskSummary {
                id: t.id.to_string(),
                category: t.category.to_string(),
                runs: mine.len(),
                passes,
                pass_rate: passes as f64 * 100.0 / n as f64,
                avg_steps: mine.iter().map(|r| r.agent_steps as f64).sum::<f64>() / n as f64,
                avg_duration_ms: mine.iter().map(|r| r.duration_ms).sum::<u64>() / n as u64,
            }
        })
        .collect()
}

fn render_markdown(r: &TasksReport) -> String {
    let mut s = format!(
        "# ASI Code task bench `{}`\n\n- provider/model: {} / {}\n- pass: {}/{} ({:.1}%)\n- duration: {:.1}s, cost: {}\n\n",
        r.benchmark_id,
        r.provider,
        r.model,
        r.pass_count,
        r.run_count,
        r.pass_rate,
        r.total_duration_ms as f64 / 1000.0,
        crate::cost::format_usd(r.total_cost_usd)
    );
    if let Some(b) = &r.baseline {
        s.push_str(&format!(
            "## Baseline\n\n- baseline: `{}` ({:.1}%)\n- regressions: {}\n- improvements: {}\n\n",
            b.baseline_path,
            b.baseline_pass_rate,
            if b.regressions.is_empty() { "none".to_string() } else { b.regressions.join("; ") },
            if b.improvements.is_empty() { "none".to_string() } else { b.improvements.join("; ") },
        ));
    }
    s.push_str("| task | category | pass | avg steps | avg time (s) |\n|---|---|---|---|---|\n");
    for t in &r.tasks {
        s.push_str(&format!(
            "| {} | {} | {}/{} | {:.1} | {:.1} |\n",
            t.id,
            t.category,
            t.passes,
            t.runs,
            t.avg_steps,
            t.avg_duration_ms as f64 / 1000.0
        ));
    }
    s.push_str("\n## Failures\n\n");
    let failures: Vec<&TaskRunResult> = r.runs.iter().filter(|x| !x.pass).collect();
    if failures.is_empty() {
        s.push_str("none\n");
    }
    for f in failures {
        s.push_str(&format!(
            "### {} (round {})\n\nworkdir: `{}`\nstop: {} / {}\n\n```\n{}\n```\n\n",
            f.id,
            f.round,
            f.workdir,
            f.stop_reason,
            f.agent_loop_stop_reason.clone().unwrap_or_default(),
            f.verifier_output
        ));
    }
    s
}

pub(crate) struct TasksOptions {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub agent_max_steps: usize,
    pub repeat: usize,
    pub out_dir: PathBuf,
    pub only: Option<String>,
    pub baseline: Option<PathBuf>,
}

pub(crate) fn run(options: TasksOptions) -> Result<String, String> {
    let original_cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let out_dir = if options.out_dir.is_absolute() {
        options.out_dir.clone()
    } else {
        original_cwd.join(&options.out_dir)
    };
    let result = run_in_workdirs(&options, &out_dir);
    let _ = std::env::set_current_dir(&original_cwd);
    result
}

fn run_in_workdirs(options: &TasksOptions, out_dir: &Path) -> Result<String, String> {
    let tasks = selected_tasks(options.only.as_deref())?;
    let cfg = crate::resolve_cfg(
        options.provider.clone(),
        options.model.clone(),
        options.permission_mode.clone(),
    );
    let repeat = options.repeat.clamp(1, 20);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let benchmark_id = format!("asi_bench_tasks_{}", now);
    let work_root = std::env::temp_dir().join("asi_bench_tasks").join(&benchmark_id);
    let started = Instant::now();
    let mut runs = Vec::new();

    for round in 1..=repeat {
        for task in &tasks {
            let dir = work_root.join(format!("{}_r{}", task.id, round));
            materialize(task, &dir)?;
            crate::set_project_dir(&dir.to_string_lossy())?;
            eprintln!("[bench tasks] round {} task {} -> {}", round, task.id, dir.display());

            // Fresh runtime per task: no history leaks between tasks.
            let mut rt = crate::runtime::Runtime::new(
                cfg.provider.clone(),
                cfg.model.clone(),
                cfg.permission_mode.clone(),
                cfg.max_turns,
            );
            rt.extended_thinking = cfg.extended_thinking;
            crate::apply_runtime_flags_from_cfg(&mut rt, &cfg);

            let task_started = Instant::now();
            let prompt = format!(
                "{}\n\nWork directly in the current directory. Make the change with tools; do not ask questions.",
                task.prompt
            );
            let run = crate::bench::run_agent_prompt(
                &cfg,
                &mut rt,
                &prompt,
                true,
                false,
                crate::PromptProfile::Standard,
                options.agent_max_steps.clamp(1, 200),
            );
            let (pass, verifier_output) = verify(task, &dir);
            let (steps, stop, loop_stop, changed, cost, preview) = match run {
                Ok(r) => (
                    r.agent_steps,
                    r.result.stop_reason.clone(),
                    r.agent_loop_stop_reason.clone(),
                    r.changed_files.clone(),
                    r.turn_cost_usd,
                    tail(&r.result.text, 400),
                ),
                Err(e) => (0, "agent_error".to_string(), Some(e), Vec::new(), 0.0, String::new()),
            };
            eprintln!("[bench tasks] {} -> {}", task.id, if pass { "PASS" } else { "FAIL" });
            runs.push(TaskRunResult {
                id: task.id.to_string(),
                category: task.category.to_string(),
                round,
                pass,
                verifier_output,
                agent_steps: steps,
                stop_reason: stop,
                agent_loop_stop_reason: loop_stop,
                changed_files: changed,
                duration_ms: task_started.elapsed().as_millis() as u64,
                cost_usd: cost,
                workdir: dir.display().to_string(),
                final_answer_preview: preview,
            });
        }
    }

    let summaries = summarize(&runs, &tasks);
    let pass_count = runs.iter().filter(|r| r.pass).count();
    let baseline = match &options.baseline {
        Some(p) => Some(diff_against_baseline(p, &summaries)?),
        None => None,
    };
    let report = TasksReport {
        benchmark_id: benchmark_id.clone(),
        suite: "tasks".to_string(),
        provider: cfg.provider.clone(),
        model: cfg.model.clone(),
        repeat,
        task_count: tasks.len(),
        run_count: runs.len(),
        pass_count,
        pass_rate: pass_count as f64 * 100.0 / runs.len().max(1) as f64,
        total_duration_ms: started.elapsed().as_millis() as u64,
        total_cost_usd: runs.iter().map(|r| r.cost_usd).sum(),
        tasks: summaries,
        runs,
        baseline,
    };

    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let json_path = out_dir.join(format!("{}.json", benchmark_id));
    let md_path = out_dir.join(format!("{}.md", benchmark_id));
    fs::write(&json_path, serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    fs::write(&md_path, render_markdown(&report)).map_err(|e| e.to_string())?;

    let mut line = format!(
        "bench_id={} suite=tasks pass={}/{} pass_rate={:.1}% total_cost={} json={} md={}",
        report.benchmark_id,
        report.pass_count,
        report.run_count,
        report.pass_rate,
        crate::cost::format_usd(report.total_cost_usd),
        json_path.display(),
        md_path.display()
    );
    if let Some(b) = &report.baseline {
        line.push_str(&format!(
            " baseline={:.1}% regressions={} improvements={}",
            b.baseline_pass_rate,
            b.regressions.len(),
            b.improvements.len()
        ));
        if !b.regressions.is_empty() {
            line.push_str(&format!("\nREGRESSIONS: {}", b.regressions.join("; ")));
        }
    }
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python_available() -> bool {
        Command::new(python_bin())
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn scratch(tag: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("asi_tasks_selftest_{}_{}", tag, nonce))
    }

    #[test]
    fn task_ids_are_unique() {
        let mut ids: Vec<_> = TASKS.iter().map(|t| t.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), TASKS.len());
    }

    #[test]
    fn every_task_fails_unsolved_and_passes_with_reference() {
        if !python_available() {
            eprintln!("skipping: python not available");
            return;
        }
        for task in TASKS {
            let dir = scratch(task.id);
            materialize(task, &dir).unwrap();
            let (pass, out) = verify(task, &dir);
            assert!(!pass, "{} must fail before any fix:\n{}", task.id, out);
            for f in task.reference {
                write_task_file(&dir, f).unwrap();
            }
            let (pass, out) = verify(task, &dir);
            assert!(pass, "{} reference solution must pass:\n{}", task.id, out);
            assert!(!dir.join(HIDDEN_TEST_FILE).exists(), "hidden test must be removed");
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn crlf_task_rejects_lf_rewrite() {
        if !python_available() {
            return;
        }
        let task = TASKS.iter().find(|t| t.id == "crlf_config_edit").unwrap();
        let dir = scratch("crlf_lf");
        materialize(task, &dir).unwrap();
        // Correct values, but line endings converted to LF.
        write_task_file(&dir, &lf("settings.py", task.reference[0].content)).unwrap();
        let (pass, out) = verify(task, &dir);
        assert!(!pass);
        assert!(out.contains("bare LF"), "{}", out);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn baseline_diff_flags_regressions_and_improvements() {
        let path = scratch("baseline").with_extension("json");
        fs::write(
            &path,
            r#"{"pass_rate": 50.0, "tasks": [{"id": "a", "pass_rate": 100.0}, {"id": "b", "pass_rate": 0.0}]}"#,
        )
        .unwrap();
        let mk = |id: &str, rate: f64| TaskSummary {
            id: id.to_string(),
            category: String::new(),
            runs: 1,
            passes: 0,
            pass_rate: rate,
            avg_steps: 0.0,
            avg_duration_ms: 0,
        };
        let diff = diff_against_baseline(&path, &[mk("a", 0.0), mk("b", 100.0)]).unwrap();
        assert_eq!(diff.regressions.len(), 1);
        assert!(diff.regressions[0].starts_with("a:"));
        assert_eq!(diff.improvements.len(), 1);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn unknown_task_filter_is_an_error() {
        assert!(selected_tasks(Some("nope")).is_err());
        assert_eq!(selected_tasks(Some("fix_median_even")).unwrap().len(), 1);
    }
}
