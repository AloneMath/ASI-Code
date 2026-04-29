use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RESULTS_HEADER: &str = "commit\tval_bpb\tmemory_gb\tstatus\tdescription\n";

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub repo: PathBuf,
    pub iterations: usize,
    pub timeout_secs: u64,
    pub log_path: Option<PathBuf>,
    pub description: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug)]
struct ParsedRunMetrics {
    val_bpb: Option<f64>,
    peak_vram_mb: Option<f64>,
    training_seconds: Option<f64>,
}

#[derive(Debug)]
struct SingleRunResult {
    log_path: PathBuf,
    status: String,
    val_bpb: f64,
    peak_vram_mb: f64,
    training_seconds: Option<f64>,
    timed_out: bool,
    exit_code: Option<i32>,
}

pub fn init_repo(repo_input: &Path) -> Result<String, String> {
    let repo = normalize_repo_path(repo_input)?;
    validate_repo_layout(&repo)?;
    let results_path = ensure_results_tsv(&repo)?;
    let results_dir = repo.join("results");
    if !results_dir.exists() {
        fs::create_dir_all(&results_dir).map_err(|e| e.to_string())?;
    }
    Ok(format!(
        "autoresearch init complete\nrepo={}\nresults_tsv={}\nresults_dir={}",
        repo.display(),
        results_path.display(),
        results_dir.display()
    ))
}

pub fn doctor(repo_input: &Path) -> Result<String, String> {
    let repo = normalize_repo_path(repo_input)?;
    validate_repo_layout(&repo)?;
    let mut lines = Vec::new();
    lines.push(format!("repo={}", repo.display()));

    let mut required = Vec::new();
    for file in [
        "README.md",
        "program.md",
        "prepare.py",
        "train.py",
        "pyproject.toml",
    ] {
        let path = repo.join(file);
        required.push(format!(
            "{}={}",
            file,
            if path.exists() { "ok" } else { "missing" }
        ));
    }
    lines.push(format!("files: {}", required.join(", ")));

    let uv_ok = ProcessCommand::new("uv")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    lines.push(format!("uv={}", if uv_ok { "ok" } else { "missing" }));

    let gpu_info = ProcessCommand::new("nvidia-smi")
        .arg("--query-gpu=name,memory.total,driver_version")
        .arg("--format=csv,noheader")
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "not detected".to_string());
    lines.push(format!("gpu={}", gpu_info));

    let cache_dir = default_cache_dir();
    let cache_text = cache_dir
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    lines.push(format!("cache_dir={}", cache_text));

    if let Some(cache) = cache_dir {
        let data_ready = cache.join("data").exists();
        let tokenizer_ready = cache.join("tokenizer").join("tokenizer.pkl").exists()
            && cache.join("tokenizer").join("token_bytes.pt").exists();
        lines.push(format!(
            "cache_data={}",
            if data_ready { "ok" } else { "missing" }
        ));
        lines.push(format!(
            "cache_tokenizer={}",
            if tokenizer_ready { "ok" } else { "missing" }
        ));
        if !data_ready || !tokenizer_ready {
            lines.push("hint=run `uv run prepare.py` in autoresearch repo first".to_string());
        }
    }

    let results_path = repo.join("results.tsv");
    lines.push(format!(
        "results_tsv={}",
        if results_path.exists() {
            "present"
        } else {
            "missing"
        }
    ));
    Ok(lines.join("\n"))
}

pub fn run_experiments(opts: RunOptions) -> Result<String, String> {
    if opts.iterations == 0 {
        return Err("iterations must be >= 1".to_string());
    }
    if opts.timeout_secs < 60 {
        return Err("timeout_secs must be >= 60".to_string());
    }
    if opts.log_path.is_some() && opts.iterations > 1 {
        return Err("custom --log can only be used when iterations=1".to_string());
    }

    let repo = normalize_repo_path(&opts.repo)?;
    validate_repo_layout(&repo)?;
    let status = normalize_status(opts.status.as_deref())?;
    let results_path = ensure_results_tsv(&repo)?;

    let mut output_lines = Vec::new();
    output_lines.push(format!("repo={}", repo.display()));
    output_lines.push(format!("results_tsv={}", results_path.display()));
    output_lines.push(format!("iterations={}", opts.iterations));
    output_lines.push(format!("timeout_secs={}", opts.timeout_secs));

    let base_desc = sanitize_description(opts.description.as_deref().unwrap_or("baseline run"));
    for i in 0..opts.iterations {
        let log_path = match (&opts.log_path, opts.iterations) {
            (Some(p), 1) => {
                if p.is_absolute() {
                    p.clone()
                } else {
                    repo.join(p)
                }
            }
            _ => {
                let runs_dir = repo.join("results");
                if !runs_dir.exists() {
                    fs::create_dir_all(&runs_dir).map_err(|e| e.to_string())?;
                }
                runs_dir.join(format!("run-{}-{:03}.log", now_unix_secs(), i + 1))
            }
        };
        let desc = if opts.iterations > 1 {
            format!("{} [run {}]", base_desc, i + 1)
        } else {
            base_desc.clone()
        };

        let result = run_single(&repo, &log_path, opts.timeout_secs, &status, &desc)?;
        output_lines.push(format!(
            "run={} status={} val_bpb={:.6} peak_vram_mb={:.1} training_seconds={} timeout={} exit_code={} log={}",
            i + 1,
            result.status,
            result.val_bpb,
            result.peak_vram_mb,
            result
                .training_seconds
                .map(|v| format!("{:.1}", v))
                .unwrap_or_else(|| "n/a".to_string()),
            result.timed_out,
            result
                .exit_code
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none".to_string()),
            result.log_path.display()
        ));
    }

    Ok(output_lines.join("\n"))
}

fn run_single(
    repo: &Path,
    log_path: &Path,
    timeout_secs: u64,
    status_if_success: &str,
    description: &str,
) -> Result<SingleRunResult, String> {
    if let Some(parent) = log_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    let log_file = File::create(log_path).map_err(|e| e.to_string())?;
    let log_file_err = log_file.try_clone().map_err(|e| e.to_string())?;

    let mut child = ProcessCommand::new("uv")
        .arg("run")
        .arg("train.py")
        .current_dir(repo)
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(log_file_err))
        .spawn()
        .map_err(|e| format!("failed to start `uv run train.py`: {}", e))?;

    let start = Instant::now();
    let mut timed_out = false;
    let exit_code = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status.code(),
            None => {
                if start.elapsed() >= Duration::from_secs(timeout_secs) {
                    timed_out = true;
                    let _ = child.kill();
                    let waited = child.wait().ok();
                    break waited.and_then(|s| s.code());
                }
                thread::sleep(Duration::from_millis(250));
            }
        }
    };

    let log_text = fs::read_to_string(log_path).map_err(|e| e.to_string())?;
    let parsed = parse_metrics_from_log(&log_text);
    let commit = git_short_commit(repo).unwrap_or_else(|| "nogit".to_string());

    let crashed = timed_out || exit_code.unwrap_or(-1) != 0;
    let status = if crashed {
        "crash".to_string()
    } else {
        status_if_success.to_string()
    };
    let val_bpb = if crashed {
        0.0
    } else {
        parsed.val_bpb.unwrap_or(0.0)
    };
    let peak_vram_mb = if crashed {
        0.0
    } else {
        parsed.peak_vram_mb.unwrap_or(0.0)
    };
    let memory_gb = peak_vram_mb / 1024.0;

    let mut results = OpenOptions::new()
        .append(true)
        .open(repo.join("results.tsv"))
        .map_err(|e| e.to_string())?;
    writeln!(
        results,
        "{}\t{:.6}\t{:.1}\t{}\t{}",
        commit, val_bpb, memory_gb, status, description
    )
    .map_err(|e| e.to_string())?;

    Ok(SingleRunResult {
        log_path: log_path.to_path_buf(),
        status,
        val_bpb,
        peak_vram_mb,
        training_seconds: parsed.training_seconds,
        timed_out,
        exit_code,
    })
}

fn parse_metrics_from_log(text: &str) -> ParsedRunMetrics {
    fn parse_line_value(line: &str, prefix: &str) -> Option<f64> {
        let idx = line.find(prefix)?;
        let v = line.get(idx + prefix.len()..)?.trim();
        v.parse::<f64>().ok()
    }

    let mut parsed = ParsedRunMetrics {
        val_bpb: None,
        peak_vram_mb: None,
        training_seconds: None,
    };
    for line in text.lines() {
        if parsed.val_bpb.is_none() {
            parsed.val_bpb = parse_line_value(line, "val_bpb:");
        }
        if parsed.peak_vram_mb.is_none() {
            parsed.peak_vram_mb = parse_line_value(line, "peak_vram_mb:");
        }
        if parsed.training_seconds.is_none() {
            parsed.training_seconds = parse_line_value(line, "training_seconds:");
        }
    }
    parsed
}

fn normalize_repo_path(repo_input: &Path) -> Result<PathBuf, String> {
    if !repo_input.exists() {
        return Err(format!("repo path not found: {}", repo_input.display()));
    }
    fs::canonicalize(repo_input).map_err(|e| e.to_string())
}

fn validate_repo_layout(repo: &Path) -> Result<(), String> {
    for name in ["prepare.py", "train.py", "program.md", "pyproject.toml"] {
        let path = repo.join(name);
        if !path.exists() {
            return Err(format!(
                "invalid autoresearch repo: missing {} at {}",
                name,
                path.display()
            ));
        }
    }
    Ok(())
}

fn ensure_results_tsv(repo: &Path) -> Result<PathBuf, String> {
    let path = repo.join("results.tsv");
    if !path.exists() {
        fs::write(&path, RESULTS_HEADER).map_err(|e| e.to_string())?;
        return Ok(path);
    }
    let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    if content.trim().is_empty() {
        fs::write(&path, RESULTS_HEADER).map_err(|e| e.to_string())?;
        return Ok(path);
    }
    let first = content.lines().next().unwrap_or("");
    if first != RESULTS_HEADER.trim_end() {
        let mut merged = String::from(RESULTS_HEADER);
        merged.push_str(&content);
        fs::write(&path, merged).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn default_cache_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .map(|p| p.join(".cache").join("autoresearch"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        env::var_os("HOME")
            .map(PathBuf::from)
            .map(|p| p.join(".cache").join("autoresearch"))
    }
}

fn git_short_commit(repo: &Path) -> Option<String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(repo)
        .arg("rev-parse")
        .arg("--short")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn sanitize_description(raw: &str) -> String {
    raw.replace('\t', " ")
        .replace('\r', " ")
        .replace('\n', " ")
        .trim()
        .to_string()
}

fn normalize_status(raw: Option<&str>) -> Result<String, String> {
    let normalized = raw.unwrap_or("keep").trim().to_lowercase();
    match normalized.as_str() {
        "keep" | "discard" | "crash" => Ok(normalized),
        _ => Err("status must be one of: keep, discard, crash".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_metrics_from_log;

    #[test]
    fn parse_metrics_reads_expected_lines() {
        let log = r#"
---
val_bpb:          0.997900
training_seconds: 300.1
peak_vram_mb:     45060.2
"#;
        let parsed = parse_metrics_from_log(log);
        assert_eq!(parsed.val_bpb, Some(0.997900));
        assert_eq!(parsed.training_seconds, Some(300.1));
        assert_eq!(parsed.peak_vram_mb, Some(45060.2));
    }
}
