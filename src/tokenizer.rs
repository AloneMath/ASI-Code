use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_BUILD_TIMEOUT_SECS: u64 = 900;
pub const DEFAULT_TRAIN_TIMEOUT_SECS: u64 = 1800;

#[derive(Debug, Clone)]
pub struct TrainOptions {
    pub repo: PathBuf,
    pub input: PathBuf,
    pub vocab_size: u32,
    pub output: Option<PathBuf>,
    pub pattern: Option<String>,
    pub name: String,
    pub python_cmd: Option<String>,
    pub auto_build: bool,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
struct CommandSpec {
    exe: String,
    args: Vec<String>,
    display: String,
}

const TRAIN_SCRIPT: &str = r#"import argparse
import json
import pathlib
import sys


def iter_lines(path: pathlib.Path):
    with path.open("r", encoding="utf-8", errors="ignore") as f:
        for line in f:
            line = line.rstrip("\n")
            if line:
                yield line


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--input", required=True)
    parser.add_argument("--vocab-size", required=True, type=int)
    parser.add_argument("--output", required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--pattern", default="")
    args = parser.parse_args()

    repo = pathlib.Path(args.repo).resolve()
    if str(repo) not in sys.path:
        sys.path.insert(0, str(repo))

    try:
        import rustbpe
    except Exception as e:
        print(f"ERROR import rustbpe failed: {e}", file=sys.stderr)
        print("HINT run: maturin develop --release (inside rustbpe repo) or pip install rustbpe", file=sys.stderr)
        sys.exit(2)

    tokenizer = rustbpe.Tokenizer()
    pattern = args.pattern if args.pattern else None
    tokenizer.train_from_iterator(
        iter_lines(pathlib.Path(args.input)),
        vocab_size=int(args.vocab_size),
        pattern=pattern,
    )
    ranks = tokenizer.get_mergeable_ranks()
    payload = {
        "name": args.name,
        "vocab_size": int(tokenizer.vocab_size),
        "pattern": tokenizer.get_pattern(),
        "mergeable_ranks_hex": [[bytes(k).hex(), int(v)] for (k, v) in ranks],
    }
    out = pathlib.Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")
    print(f"trained_vocab_size={tokenizer.vocab_size}")
    print(f"pattern={tokenizer.get_pattern()}")
    print(f"mergeable_ranks={len(ranks)}")
    print(f"output={out}")


if __name__ == "__main__":
    main()
"#;

pub fn doctor(repo_input: &Path) -> Result<String, String> {
    let repo = normalize_repo_path(repo_input)?;
    validate_repo_layout(&repo)?;

    let mut lines = Vec::new();
    lines.push(format!("repo={}", repo.display()));

    for req in [
        "Cargo.toml",
        "pyproject.toml",
        "src/lib.rs",
        "tests/python/test_tokenizer.py",
    ] {
        let path = repo.join(req);
        lines.push(format!(
            "file_{}={}",
            req.replace('/', "_"),
            if path.exists() { "ok" } else { "missing" }
        ));
    }

    let cargo = probe_command(
        &CommandSpec {
            exe: "cargo".to_string(),
            args: vec!["--version".to_string()],
            display: "cargo --version".to_string(),
        },
        None,
    );
    lines.push(format!(
        "cargo={}",
        match cargo {
            Ok(v) => format!("ok ({})", v),
            Err(e) => format!("missing ({})", e),
        }
    ));

    let uv = probe_command(
        &CommandSpec {
            exe: "uv".to_string(),
            args: vec!["--version".to_string()],
            display: "uv --version".to_string(),
        },
        None,
    );
    lines.push(format!(
        "uv={}",
        match uv {
            Ok(v) => format!("ok ({})", v),
            Err(e) => format!("missing ({})", e),
        }
    ));

    let python = detect_python_command();
    match python {
        Some(spec) => {
            let py_version = probe_command(
                &CommandSpec {
                    exe: spec.exe.clone(),
                    args: {
                        let mut v = spec.args.clone();
                        v.push("--version".to_string());
                        v
                    },
                    display: format!("{} --version", spec.display),
                },
                None,
            );
            lines.push(format!("python_cmd={}", spec.display));
            lines.push(format!(
                "python={}",
                match py_version {
                    Ok(v) => format!("ok ({})", v),
                    Err(e) => format!("error ({})", e),
                }
            ));
        }
        None => {
            lines.push("python=missing".to_string());
            lines.push(
                "hint=install Python 3.9+ and ensure `python` or `py -3` is available".to_string(),
            );
        }
    }

    let maturin_candidates = build_maturin_candidates(vec!["--version".to_string()]);
    let mut maturin_ok: Option<String> = None;
    let mut maturin_errs = Vec::new();
    for spec in maturin_candidates {
        match probe_command(&spec, Some(&repo)) {
            Ok(v) => {
                maturin_ok = Some(format!("{} ({})", spec.display, v));
                break;
            }
            Err(e) => maturin_errs.push(format!("{} => {}", spec.display, e)),
        }
    }
    if let Some(v) = maturin_ok {
        lines.push(format!("maturin=ok ({})", v));
    } else {
        lines.push(format!("maturin=missing ({})", maturin_errs.join(" | ")));
        lines.push(
            "hint=install with `pip install maturin` or `uv tool install maturin`".to_string(),
        );
    }

    lines.push(
        "next=asi tokenizer build --repo <rustbpe_repo> && asi tokenizer train --repo <rustbpe_repo> --input <corpus.txt>"
            .to_string(),
    );
    Ok(lines.join("\n"))
}

pub fn build_repo(repo_input: &Path, release: bool, timeout_secs: u64) -> Result<String, String> {
    if timeout_secs == 0 {
        return Err("timeout_secs must be >= 1".to_string());
    }

    let repo = normalize_repo_path(repo_input)?;
    validate_repo_layout(&repo)?;

    let mut errors = Vec::new();
    let mut build_args = vec!["develop".to_string()];
    if release {
        build_args.push("--release".to_string());
    }

    for spec in build_maturin_candidates(build_args.clone()) {
        match run_command_with_timeout(&spec, Some(&repo), timeout_secs) {
            Ok(out) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let mut lines = vec![
                    format!("repo={}", repo.display()),
                    format!("build_command={}", render_command(&spec.exe, &spec.args)),
                    format!("mode={}", if release { "release" } else { "debug" }),
                    format!("timeout_secs={}", timeout_secs),
                ];
                if !stdout.is_empty() {
                    lines.push(format!("stdout={}", clip_chars(&stdout, 800)));
                }
                if !stderr.is_empty() {
                    lines.push(format!("stderr={}", clip_chars(&stderr, 800)));
                }
                lines.push("status=ok".to_string());
                return Ok(lines.join("\n"));
            }
            Ok(out) => {
                let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
                errors.push(format!(
                    "{} => {}",
                    spec.display,
                    if msg.is_empty() {
                        format!("exit={}", out.status)
                    } else {
                        clip_chars(&msg, 400)
                    }
                ));
            }
            Err(e) => errors.push(format!("{} => {}", spec.display, e)),
        }
    }

    Err(format!(
        "failed to build rustbpe module\nrepo={}\nerrors={}",
        repo.display(),
        errors.join(" | ")
    ))
}

pub fn train_from_file(opts: TrainOptions) -> Result<String, String> {
    if opts.vocab_size < 256 {
        return Err("vocab_size must be >= 256".to_string());
    }
    if opts.name.trim().is_empty() {
        return Err("name cannot be empty".to_string());
    }
    if opts.timeout_secs == 0 {
        return Err("timeout_secs must be >= 1".to_string());
    }

    let repo = normalize_repo_path(&opts.repo)?;
    validate_repo_layout(&repo)?;

    let input = normalize_file_path(&opts.input)?;
    if !input.is_file() {
        return Err(format!("input is not a file: {}", input.display()));
    }

    if opts.auto_build {
        let _ = build_repo(&repo, true, opts.timeout_secs)?;
    }

    let output = resolve_output_path(&repo, opts.output.as_ref())?;
    if let Some(parent) = output.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }

    let python = select_python_command(opts.python_cmd.as_deref())?;
    let script_path =
        std::env::temp_dir().join(format!("asi_rustbpe_train_{}.py", now_unix_secs()));
    fs::write(&script_path, TRAIN_SCRIPT).map_err(|e| e.to_string())?;

    let mut args = python.args.clone();
    args.push(script_path.display().to_string());
    args.push("--repo".to_string());
    args.push(repo.display().to_string());
    args.push("--input".to_string());
    args.push(input.display().to_string());
    args.push("--vocab-size".to_string());
    args.push(opts.vocab_size.to_string());
    args.push("--output".to_string());
    args.push(output.display().to_string());
    args.push("--name".to_string());
    args.push(opts.name.clone());
    if let Some(p) = opts.pattern.as_deref() {
        args.push("--pattern".to_string());
        args.push(p.to_string());
    }

    let run_spec = CommandSpec {
        exe: python.exe,
        args,
        display: python.display,
    };

    let result = run_command_with_timeout(&run_spec, Some(&repo), opts.timeout_secs);
    let _ = fs::remove_file(&script_path);
    let out = result?;

    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !out.status.success() {
        return Err(format!(
            "tokenizer training failed\nrepo={}\ncommand={}\nstdout={}\nstderr={}\nhint=run `asi tokenizer build --repo {}` first, then retry",
            repo.display(),
            render_command(&run_spec.exe, &run_spec.args),
            clip_chars(&stdout, 1200),
            clip_chars(&stderr, 1200),
            repo.display()
        ));
    }

    let mut lines = vec![
        format!("repo={}", repo.display()),
        format!("input={}", input.display()),
        format!("output={}", output.display()),
        format!("vocab_size={}", opts.vocab_size),
        format!("python_cmd={}", run_spec.display),
        format!("auto_build={}", opts.auto_build),
        format!("timeout_secs={}", opts.timeout_secs),
        "status=ok".to_string(),
    ];
    if !stdout.is_empty() {
        lines.push(format!("stdout={}", clip_chars(&stdout, 1200)));
    }
    if !stderr.is_empty() {
        lines.push(format!("stderr={}", clip_chars(&stderr, 1200)));
    }
    Ok(lines.join("\n"))
}

pub fn repl_usage() -> &'static str {
    "Usage: /tokenizer help | /tokenizer doctor [--repo <path>|<path>] | /tokenizer build [--repo <path>|<path>] [--debug] [--timeout-secs <sec>] | /tokenizer train [--repo <path>] --input <corpus.txt> [--vocab-size <n>] [--output <path>] [--pattern <regex>] [--name <name>] [--python-cmd <cmd>] [--timeout-secs <sec>] [--auto-build|--no-auto-build]"
}

pub fn handle_repl_command(args: &str) -> Result<String, String> {
    let tokens = parse_cli_tokens(args)?;
    if tokens.is_empty() {
        return Ok(repl_usage().to_string());
    }

    let sub = tokens[0].as_str();
    let rest = &tokens[1..];

    match sub {
        "help" => Ok(repl_usage().to_string()),
        "doctor" => {
            let repo = parse_repo_arg(rest, "doctor")?;
            doctor(&repo)
        }
        "build" => {
            let (repo, release, timeout_secs) = parse_build_args(rest)?;
            build_repo(&repo, release, timeout_secs)
        }
        "train" => {
            let opts = parse_train_args(rest)?;
            train_from_file(opts)
        }
        _ => Ok(repl_usage().to_string()),
    }
}

fn parse_build_args(tokens: &[String]) -> Result<(PathBuf, bool, u64), String> {
    let mut repo: Option<PathBuf> = None;
    let mut release = true;
    let mut timeout_secs = DEFAULT_BUILD_TIMEOUT_SECS;
    let mut i = 0usize;

    while i < tokens.len() {
        let token = tokens[i].as_str();
        if token == "--debug" {
            release = false;
            i += 1;
            continue;
        }
        if token == "--release" {
            release = true;
            i += 1;
            continue;
        }
        if token == "--timeout-secs" {
            i += 1;
            let value = tokens
                .get(i)
                .ok_or_else(|| format!("missing value for --timeout-secs\n{}", repl_usage()))?;
            timeout_secs = value
                .parse::<u64>()
                .map_err(|_| format!("invalid --timeout-secs: {}\n{}", value, repl_usage()))?;
            if timeout_secs == 0 {
                return Err(format!("--timeout-secs must be >= 1\n{}", repl_usage()));
            }
            i += 1;
            continue;
        }
        if token == "--repo" {
            i += 1;
            let value = tokens
                .get(i)
                .ok_or_else(|| format!("missing value for --repo\n{}", repl_usage()))?;
            repo = Some(PathBuf::from(value));
            i += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix("--repo=") {
            repo = Some(PathBuf::from(value));
            i += 1;
            continue;
        }
        if let Some(value) = token.strip_prefix("--timeout-secs=") {
            timeout_secs = value
                .parse::<u64>()
                .map_err(|_| format!("invalid --timeout-secs: {}\n{}", value, repl_usage()))?;
            if timeout_secs == 0 {
                return Err(format!("--timeout-secs must be >= 1\n{}", repl_usage()));
            }
            i += 1;
            continue;
        }
        if token.starts_with('-') {
            return Err(format!("unknown build flag: {}\n{}", token, repl_usage()));
        }
        if repo.is_none() {
            repo = Some(PathBuf::from(token));
            i += 1;
            continue;
        }
        return Err(format!(
            "unexpected build token: {}\n{}",
            token,
            repl_usage()
        ));
    }

    let repo = if let Some(path) = repo {
        path
    } else {
        std::env::current_dir().map_err(|e| e.to_string())?
    };
    Ok((repo, release, timeout_secs))
}

fn parse_train_args(tokens: &[String]) -> Result<TrainOptions, String> {
    let mut repo: Option<PathBuf> = None;
    let mut input: Option<PathBuf> = None;
    let mut vocab_size = 4096_u32;
    let mut output: Option<PathBuf> = None;
    let mut pattern: Option<String> = None;
    let mut name = "rustbpe_custom".to_string();
    let mut python_cmd: Option<String> = None;
    let mut auto_build = false;
    let mut timeout_secs = DEFAULT_TRAIN_TIMEOUT_SECS;

    let mut i = 0usize;
    while i < tokens.len() {
        let token = tokens[i].as_str();
        match token {
            "--repo" => {
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| format!("missing value for --repo\n{}", repl_usage()))?;
                repo = Some(PathBuf::from(value));
                i += 1;
                continue;
            }
            "--input" => {
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| format!("missing value for --input\n{}", repl_usage()))?;
                input = Some(PathBuf::from(value));
                i += 1;
                continue;
            }
            "--vocab-size" => {
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| format!("missing value for --vocab-size\n{}", repl_usage()))?;
                vocab_size = value
                    .parse::<u32>()
                    .map_err(|_| format!("invalid --vocab-size: {}\n{}", value, repl_usage()))?;
                i += 1;
                continue;
            }
            "--output" => {
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| format!("missing value for --output\n{}", repl_usage()))?;
                output = Some(PathBuf::from(value));
                i += 1;
                continue;
            }
            "--pattern" => {
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| format!("missing value for --pattern\n{}", repl_usage()))?;
                pattern = Some(value.to_string());
                i += 1;
                continue;
            }
            "--name" => {
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| format!("missing value for --name\n{}", repl_usage()))?;
                name = value.to_string();
                i += 1;
                continue;
            }
            "--python-cmd" => {
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| format!("missing value for --python-cmd\n{}", repl_usage()))?;
                python_cmd = Some(value.to_string());
                i += 1;
                continue;
            }
            "--auto-build" => {
                auto_build = true;
                i += 1;
                continue;
            }
            "--no-auto-build" => {
                auto_build = false;
                i += 1;
                continue;
            }
            "--timeout-secs" => {
                i += 1;
                let value = tokens
                    .get(i)
                    .ok_or_else(|| format!("missing value for --timeout-secs\n{}", repl_usage()))?;
                timeout_secs = value
                    .parse::<u64>()
                    .map_err(|_| format!("invalid --timeout-secs: {}\n{}", value, repl_usage()))?;
                if timeout_secs == 0 {
                    return Err(format!("--timeout-secs must be >= 1\n{}", repl_usage()));
                }
                i += 1;
                continue;
            }
            _ => {}
        }

        if let Some(v) = token.strip_prefix("--repo=") {
            repo = Some(PathBuf::from(v));
            i += 1;
            continue;
        }
        if let Some(v) = token.strip_prefix("--input=") {
            input = Some(PathBuf::from(v));
            i += 1;
            continue;
        }
        if let Some(v) = token.strip_prefix("--vocab-size=") {
            vocab_size = v
                .parse::<u32>()
                .map_err(|_| format!("invalid --vocab-size: {}\n{}", v, repl_usage()))?;
            i += 1;
            continue;
        }
        if let Some(v) = token.strip_prefix("--output=") {
            output = Some(PathBuf::from(v));
            i += 1;
            continue;
        }
        if let Some(v) = token.strip_prefix("--pattern=") {
            pattern = Some(v.to_string());
            i += 1;
            continue;
        }
        if let Some(v) = token.strip_prefix("--name=") {
            name = v.to_string();
            i += 1;
            continue;
        }
        if let Some(v) = token.strip_prefix("--python-cmd=") {
            python_cmd = Some(v.to_string());
            i += 1;
            continue;
        }
        if let Some(v) = token.strip_prefix("--auto-build=") {
            auto_build = parse_bool_flag(v)
                .ok_or_else(|| format!("invalid --auto-build value: {}\n{}", v, repl_usage()))?;
            i += 1;
            continue;
        }
        if let Some(v) = token.strip_prefix("--timeout-secs=") {
            timeout_secs = v
                .parse::<u64>()
                .map_err(|_| format!("invalid --timeout-secs: {}\n{}", v, repl_usage()))?;
            if timeout_secs == 0 {
                return Err(format!("--timeout-secs must be >= 1\n{}", repl_usage()));
            }
            i += 1;
            continue;
        }

        if token.starts_with('-') {
            return Err(format!("unknown train flag: {}\n{}", token, repl_usage()));
        }
        if input.is_none() {
            input = Some(PathBuf::from(token));
            i += 1;
            continue;
        }
        return Err(format!(
            "unexpected train token: {}\n{}",
            token,
            repl_usage()
        ));
    }

    let repo = if let Some(path) = repo {
        path
    } else {
        std::env::current_dir().map_err(|e| e.to_string())?
    };
    let input = input.ok_or_else(|| format!("missing --input\n{}", repl_usage()))?;

    Ok(TrainOptions {
        repo,
        input,
        vocab_size,
        output,
        pattern,
        name,
        python_cmd,
        auto_build,
        timeout_secs,
    })
}

fn parse_repo_arg(tokens: &[String], subcommand: &str) -> Result<PathBuf, String> {
    let mut repo: Option<PathBuf> = None;
    let mut i = 0usize;
    while i < tokens.len() {
        let token = tokens[i].as_str();
        if token == "--repo" {
            i += 1;
            let value = tokens
                .get(i)
                .ok_or_else(|| format!("missing value for --repo\n{}", repl_usage()))?;
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
                repl_usage()
            ));
        }
        if repo.is_some() {
            return Err(format!(
                "unexpected token for {}: {}\n{}",
                subcommand,
                token,
                repl_usage()
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

fn parse_bool_flag(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
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

fn normalize_repo_path(repo_input: &Path) -> Result<PathBuf, String> {
    if !repo_input.exists() {
        return Err(format!("repo path not found: {}", repo_input.display()));
    }
    fs::canonicalize(repo_input).map_err(|e| e.to_string())
}

fn normalize_file_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok(cwd.join(path))
}

fn validate_repo_layout(repo: &Path) -> Result<(), String> {
    for name in ["Cargo.toml", "pyproject.toml", "src/lib.rs"] {
        let path = repo.join(name);
        if !path.exists() {
            return Err(format!(
                "invalid rustbpe repo: missing {} at {}",
                name,
                path.display()
            ));
        }
    }
    Ok(())
}

fn resolve_output_path(repo: &Path, output: Option<&PathBuf>) -> Result<PathBuf, String> {
    if let Some(p) = output {
        if p.is_absolute() {
            return Ok(p.clone());
        }
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        return Ok(cwd.join(p));
    }
    Ok(repo
        .join("artifacts")
        .join(format!("rustbpe-tokenizer-{}.json", now_unix_secs())))
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn detect_python_command() -> Option<CommandSpec> {
    let candidates = vec![
        CommandSpec {
            exe: "python".to_string(),
            args: vec![],
            display: "python".to_string(),
        },
        CommandSpec {
            exe: "py".to_string(),
            args: vec!["-3".to_string()],
            display: "py -3".to_string(),
        },
        CommandSpec {
            exe: "py".to_string(),
            args: vec![],
            display: "py".to_string(),
        },
    ];
    for spec in candidates {
        let mut check = spec.args.clone();
        check.push("--version".to_string());
        if run_command(
            &CommandSpec {
                exe: spec.exe.clone(),
                args: check,
                display: spec.display.clone(),
            },
            None,
        )
        .map(|o| o.status.success())
        .unwrap_or(false)
        {
            return Some(spec);
        }
    }
    None
}

fn select_python_command(raw_cmd: Option<&str>) -> Result<CommandSpec, String> {
    if let Some(raw) = raw_cmd {
        let tokens = parse_cli_tokens(raw)?;
        if tokens.is_empty() {
            return Err("python command is empty".to_string());
        }
        let exe = tokens[0].to_string();
        let args = tokens[1..].to_vec();
        return Ok(CommandSpec {
            exe,
            args,
            display: raw.to_string(),
        });
    }
    detect_python_command().ok_or_else(|| {
        "python not found: install Python and ensure `python` or `py -3` is in PATH".to_string()
    })
}

fn build_maturin_candidates(extra_args: Vec<String>) -> Vec<CommandSpec> {
    let mut out = Vec::new();

    let mut direct = vec![];
    direct.extend(extra_args.iter().cloned());
    out.push(CommandSpec {
        exe: "maturin".to_string(),
        args: direct,
        display: "maturin".to_string(),
    });

    let mut uv_run = vec!["run".to_string(), "maturin".to_string()];
    uv_run.extend(extra_args);
    out.push(CommandSpec {
        exe: "uv".to_string(),
        args: uv_run,
        display: "uv run maturin".to_string(),
    });

    out
}

fn probe_command(spec: &CommandSpec, cwd: Option<&Path>) -> Result<String, String> {
    let out = run_command(spec, cwd)?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if out.status.success() {
        let msg = if !stdout.is_empty() { stdout } else { stderr };
        if msg.is_empty() {
            Ok("ok".to_string())
        } else {
            Ok(clip_chars(&msg, 200))
        }
    } else {
        let msg = if !stderr.is_empty() { stderr } else { stdout };
        if msg.is_empty() {
            Err(format!("exit={}", out.status))
        } else {
            Err(clip_chars(&msg, 200))
        }
    }
}

fn run_command(spec: &CommandSpec, cwd: Option<&Path>) -> Result<Output, String> {
    let mut cmd = ProcessCommand::new(&spec.exe);
    cmd.args(&spec.args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.output().map_err(|e| e.to_string())
}

fn run_command_with_timeout(
    spec: &CommandSpec,
    cwd: Option<&Path>,
    timeout_secs: u64,
) -> Result<Output, String> {
    if timeout_secs == 0 {
        return Err("timeout_secs must be >= 1".to_string());
    }

    let mut cmd = ProcessCommand::new(&spec.exe);
    cmd.args(&spec.args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut out_reader = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture stdout".to_string())?;
    let mut err_reader = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture stderr".to_string())?;

    let out_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_reader.read_to_end(&mut buf);
        buf
    });
    let err_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_reader.read_to_end(&mut buf);
        buf
    });

    let timeout = Duration::from_secs(timeout_secs);
    let start = Instant::now();
    let mut timed_out = false;

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    timed_out = true;
                    let _ = child.kill();
                    break child.wait().map_err(|e| e.to_string())?;
                }
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(e.to_string());
            }
        }
    };

    let stdout = out_handle
        .join()
        .map_err(|_| "stdout reader thread panicked".to_string())?;
    let stderr = err_handle
        .join()
        .map_err(|_| "stderr reader thread panicked".to_string())?;

    if timed_out {
        let stdout_s = String::from_utf8_lossy(&stdout).trim().to_string();
        let stderr_s = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(format!(
            "command timed out after {}s\ncommand={}\nstdout={}\nstderr={}\nhint=increase --timeout-secs and retry",
            timeout_secs,
            render_command(&spec.exe, &spec.args),
            clip_chars(&stdout_s, 800),
            clip_chars(&stderr_s, 800)
        ));
    }

    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn render_command(exe: &str, args: &[String]) -> String {
    if args.is_empty() {
        return exe.to_string();
    }
    format!("{} {}", exe, args.join(" "))
}

fn clip_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut out = String::new();
    for (idx, ch) in input.chars().enumerate() {
        if idx >= max_chars {
            break;
        }
        out.push(ch);
    }
    out.push_str("…");
    out
}

#[cfg(test)]
mod tests {
    use super::{parse_build_args, parse_cli_tokens, parse_train_args, DEFAULT_TRAIN_TIMEOUT_SECS};
    use std::path::PathBuf;

    #[test]
    fn parse_tokens_supports_quotes() {
        let tokens =
            parse_cli_tokens(r#"train --input "D:\Code\corpus.txt" --name "my tok""#).unwrap();
        assert_eq!(tokens[0], "train");
        assert_eq!(tokens[2], "D:\\Code\\corpus.txt");
        assert_eq!(tokens[4], "my tok");
    }

    #[test]
    fn parse_train_args_supports_positional_input() {
        let tokens = vec![
            "corpus.txt".to_string(),
            "--vocab-size".to_string(),
            "4096".to_string(),
            "--auto-build".to_string(),
        ];
        let opts = parse_train_args(&tokens).unwrap();
        assert_eq!(opts.input, PathBuf::from("corpus.txt"));
        assert_eq!(opts.vocab_size, 4096);
        assert!(opts.auto_build);
        assert_eq!(opts.timeout_secs, DEFAULT_TRAIN_TIMEOUT_SECS);
    }

    #[test]
    fn parse_train_args_supports_flag_values() {
        let tokens = vec![
            "--repo=D:\\Code\\rustbpe".to_string(),
            "--input".to_string(),
            "data.txt".to_string(),
            "--python-cmd".to_string(),
            "py -3".to_string(),
            "--timeout-secs=777".to_string(),
            "--auto-build=false".to_string(),
        ];
        let opts = parse_train_args(&tokens).unwrap();
        assert_eq!(opts.repo, PathBuf::from("D:\\Code\\rustbpe"));
        assert_eq!(opts.input, PathBuf::from("data.txt"));
        assert_eq!(opts.python_cmd.as_deref(), Some("py -3"));
        assert_eq!(opts.timeout_secs, 777);
        assert!(!opts.auto_build);
    }

    #[test]
    fn parse_build_args_supports_timeout() {
        let tokens = vec![
            "--repo".to_string(),
            "D:\\Code\\rustbpe".to_string(),
            "--debug".to_string(),
            "--timeout-secs".to_string(),
            "321".to_string(),
        ];
        let (repo, release, timeout_secs) = parse_build_args(&tokens).unwrap();
        assert_eq!(repo, PathBuf::from("D:\\Code\\rustbpe"));
        assert!(!release);
        assert_eq!(timeout_secs, 321);
    }
}
