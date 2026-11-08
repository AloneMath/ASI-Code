#![allow(dead_code)]
use regex::Regex;
use std::fs;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub fn guard_bash_command(
    permission_mode: &str,
    command: &str,
    safe_shell_mode: bool,
) -> Result<(), String> {
    if permission_mode == "read-only" {
        return Err("bash is blocked in read-only mode".to_string());
    }

    if !safe_shell_mode {
        return Ok(());
    }

    if let Some(pattern) = find_dangerous_command_pattern(command) {
        return Err(format!(
            "blocked by safe shell mode: detected dangerous pattern `{}`",
            pattern
        ));
    }

    Ok(())
}

/// Replace credential-looking literals with `[REDACTED]`. Used before
/// anything is persisted (audit log) so a blocked secret is not stored
/// on disk where a later search could surface it to the model.
pub fn redact_secrets(text: &str) -> String {
    static PATTERNS: std::sync::OnceLock<Vec<Regex>> = std::sync::OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?(-----END [A-Z ]*PRIVATE KEY-----|$)",
            r"\bAKIA[0-9A-Z]{16}\b",
            r"\bgh[pousr]_[A-Za-z0-9]{36,}\b",
            r"\bsk-(ant-)?[A-Za-z0-9_-]{24,}\b",
        ]
        .iter()
        .map(|p| Regex::new(p).expect("valid redaction regex"))
        .collect()
    });
    let mut out = text.to_string();
    for re in patterns {
        out = re.replace_all(&out, "[REDACTED]").into_owned();
    }
    out
}

/// True when `path` names a credential file: `.env` / `.env.*`, key and
/// certificate files, or anything under `.ssh`, `.aws`, `.gnupg`.
/// Templates such as `.env.example` are not secrets.
pub fn is_secret_file_path(path: &str) -> bool {
    let normalized = path.trim().replace('\\', "/").to_ascii_lowercase();
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let Some(name) = parts.last() else {
        return false;
    };
    if [".example", ".sample", ".template", ".dist"].iter().any(|s| name.ends_with(s)) {
        return false;
    }
    if parts[..parts.len() - 1].iter().any(|p| matches!(*p, ".ssh" | ".aws" | ".gnupg")) {
        return true;
    }
    if *name == ".env" || name.starts_with(".env.") {
        return true;
    }
    if matches!(*name, ".npmrc" | ".pypirc" | ".netrc" | "id_rsa" | "id_ed25519" | "id_ecdsa" | "id_dsa") {
        return true;
    }
    [".pem", ".key", ".pfx", ".p12"].iter().any(|s| name.ends_with(s))
}

/// Target path of a file tool call, from either the legacy text form
/// (`path <<<CONTENT ...`, `"quoted path" ...`) or a raw JSON object.
fn file_tool_target_path(args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return v
                .get("path")
                .or_else(|| v.get("file_path"))
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string();
        }
    }
    let head = trimmed.split("<<<").next().unwrap_or("");
    let (path, _) = split_first_arg(head);
    strip_surrounding_quotes(path).to_string()
}

pub fn secret_files_allowed_by_env() -> bool {
    matches!(
        std::env::var("ASI_ALLOW_SECRET_FILES")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "on" | "yes"
    )
}

/// Built-in credential protection, on by default. Tool output is sent to
/// the model provider, so reading a secret file leaks it just as surely as
/// writing one corrupts it. Opt out with ASI_ALLOW_SECRET_FILES=1.
pub fn guard_secret_file_access(tool: &str, args: &str) -> Result<(), String> {
    if secret_files_allowed_by_env() {
        return Ok(());
    }
    let path = match tool {
        "read_file" | "write_file" | "edit_file" => file_tool_target_path(args),
        "grep_search" => strip_surrounding_quotes(&parse_grep_base_path(args.trim())).to_string(),
        _ => return Ok(()),
    };
    if is_secret_file_path(&path) {
        return Err(format!(
            "credential protection: `{}` looks like a secret file and its contents would be sent to the model provider; ask the user to handle it (set ASI_ALLOW_SECRET_FILES=1 to disable this check)",
            path
        ));
    }
    Ok(())
}

pub fn guard_tool_path_access(
    tool: &str,
    args: &str,
    path_restriction_enabled: bool,
    workspace_root: &Path,
    additional_directories: &[PathBuf],
) -> Result<(), String> {
    if !path_restriction_enabled {
        return Ok(());
    }

    let mut candidates = Vec::new();
    match tool {
        "read_file" | "write_file" | "edit_file" => {
            let (path, _) = split_first_arg(args);
            let path = strip_surrounding_quotes(path);
            if !path.is_empty() {
                candidates.push(path.to_string());
            }
        }
        "type"
        | "type_text"
        | "click"
        | "mouse_click"
        | "click_text"
        | "screenshot"
        | "find_window"
        | "read_screen_text"
        | "ue5_bridge"
        | "ue5_scene_probe"
        | "blender_bridge"
        | "blender_scene_probe"
        | "unity_bridge"
        | "unity_scene_probe"
        | "video_capture"
        | "video_keyframes"
        | "probe_diff" => {
            return Ok(());
        }
        "glob_search" => {
            let pattern = strip_surrounding_quotes(args.trim());
            let base = glob_base(pattern);
            candidates.push(base);
        }
        "grep_search" => {
            let base = strip_surrounding_quotes(&parse_grep_base_path(args.trim())).to_string();
            candidates.push(base);
        }
        _ => return Ok(()),
    }

    for raw in candidates {
        let candidate = resolve_candidate_path(&raw, workspace_root);
        if !is_allowed_path(&candidate, workspace_root, additional_directories) {
            return Err(format!(
                "path access denied for `{}` (outside workspace and additional directories)",
                raw
            ));
        }
    }

    Ok(())
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

fn find_dangerous_pattern(command: &str) -> Option<&'static str> {
    let lower = command.to_ascii_lowercase();
    let patterns = [
        "rm -rf /",
        "rm -rf \\",
        "rm -rf *",
        "sudo rm -rf",
        "del /f /s /q",
        "rd /s /q",
        "format ",
        "mkfs",
        "shutdown",
        "reboot",
        "halt",
        "poweroff",
        "diskpart",
        "cipher /w",
    ];

    patterns.iter().copied().find(|p| lower.contains(*p))
}

/// Extract the base_path from grep_search args, supporting quoted patterns.
fn parse_grep_base_path(args: &str) -> String {
    if args.is_empty() {
        return ".".to_string();
    }
    // Quoted pattern: "pattern with spaces" base_path
    if args.starts_with('"') {
        if let Some(end) = args[1..].find('"') {
            let rest = args[end + 2..].trim();
            if rest.is_empty() {
                return ".".to_string();
            }
            return rest.to_string();
        }
    }
    // Unquoted: pattern base_path
    let mut it = args.splitn(2, ' ');
    let _pattern = it.next().unwrap_or("");
    it.next().unwrap_or(".").trim().to_string()
}

fn glob_base(pattern: &str) -> String {
    if pattern.is_empty() {
        return ".".to_string();
    }

    let mut cut = pattern.len();
    for (idx, ch) in pattern.char_indices() {
        if matches!(ch, '*' | '?' | '[' | '{') {
            cut = idx;
            break;
        }
    }

    let prefix = pattern[..cut].trim();
    if prefix.is_empty() {
        return ".".to_string();
    }

    prefix
        .trim_end_matches(['/', '\\'])
        .to_string()
        .if_empty_then_dot()
}

trait EmptyFallback {
    fn if_empty_then_dot(self) -> String;
}

impl EmptyFallback for String {
    fn if_empty_then_dot(self) -> String {
        if self.is_empty() {
            ".".to_string()
        } else {
            self
        }
    }
}

fn resolve_candidate_path(raw: &str, workspace_root: &Path) -> PathBuf {
    let input = PathBuf::from(raw);
    let abs = if input.is_absolute() {
        input
    } else {
        workspace_root.join(input)
    };

    normalize_path_lexical(&normalize_path(&abs))
}

fn normalize_path(path: &Path) -> PathBuf {
    let lexical = normalize_path_lexical(path);

    if let Ok(c) = fs::canonicalize(&lexical) {
        return c;
    }

    let mut tail: Vec<OsString> = Vec::new();
    let mut cursor = lexical.as_path();

    loop {
        if let Ok(base) = fs::canonicalize(cursor) {
            let mut rebuilt = base;
            for seg in tail.iter().rev() {
                rebuilt.push(seg);
            }
            return rebuilt;
        }

        let Some(name) = cursor.file_name() else {
            break;
        };
        tail.push(name.to_os_string());

        let Some(parent) = cursor.parent() else {
            break;
        };
        cursor = parent;
        if cursor.as_os_str().is_empty() {
            break;
        }
    }

    lexical
}

fn normalize_path_lexical(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    let mut anchored = false;

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                normalized.push(prefix.as_os_str());
                anchored = true;
            }
            Component::RootDir => {
                normalized.push(component.as_os_str());
                anchored = true;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !anchored {
                    normalized.push("..");
                }
            }
            Component::Normal(name) => normalized.push(name),
        }
    }

    normalized
}

fn is_allowed_path(path: &Path, workspace_root: &Path, additional_directories: &[PathBuf]) -> bool {
    let workspace = normalize_path_lexical(&normalize_path(workspace_root));
    if path.starts_with(&workspace) {
        return true;
    }

    additional_directories.iter().any(|d| {
        let dir = normalize_path_lexical(&normalize_path(d));
        path.starts_with(&dir)
    })
}

pub fn sanitize_undercover_text(input: &str) -> String {
    let mut lines = Vec::new();
    for line in input.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("co-authored-by") {
            continue;
        }
        lines.push(line.to_string());
    }

    let mut out = lines.join("\n");
    let replacements = [
        r"(?i)\bclaude code\b",
        r"(?i)\basi code\b",
        r"(?i)\banthropic\b",
        r"(?i)\bcapybara\b",
        r"(?i)\btengu\b",
        r"(?i)\bfennec\b",
        r"(?i)\bnumbat\b",
        r"(?i)\bgenerated with\b",
        r"(?i)\bai[- ]?generated\b",
    ];

    for pat in replacements {
        if let Ok(re) = Regex::new(pat) {
            out = re.replace_all(&out, "assistant").to_string();
        }
    }

    let compact = out
        .lines()
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    if compact.is_empty() {
        "Update project files".to_string()
    } else {
        compact
    }
}

/// Enhanced path normalization that prevents path traversal attacks
/// This is a more robust version of normalize_path that properly handles
/// path components and symlinks
pub fn normalize_path_secure(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    use std::path::{Component, PathBuf};

    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                // On Windows, preserve the prefix
                normalized.push(prefix.as_os_str());
            }
            Component::RootDir => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {
                // Current directory, do nothing
            }
            Component::ParentDir => {
                // Attempt to go up a directory
                if !normalized.pop() {
                    return Err(format!("Path traversal attempt: {}", path.display()));
                }
            }
            Component::Normal(name) => {
                // Normal path component
                normalized.push(name);
            }
        }
    }

    // Try to canonicalize if the path exists
    if normalized.exists() {
        if let Ok(canonical) = std::fs::canonicalize(&normalized) {
            return Ok(canonical);
        }
    }

    Ok(normalized)
}

/// Check if a command is dangerous and should be blocked
/// Returns Some(pattern) if dangerous, None if safe
pub fn find_dangerous_command_pattern(command: &str) -> Option<&'static str> {
    let lower = command.to_ascii_lowercase();

    // Expanded list of dangerous patterns
    let patterns = [
        // File deletion patterns
        "rm -rf /",
        "rm -rf \\",
        "rm -rf *",
        "sudo rm -rf",
        "del /f /s /q",
        "rd /s /q",
        "format ",
        "mkfs",
        // System shutdown/reboot
        "shutdown",
        "reboot",
        "halt",
        "poweroff",
        // Disk manipulation
        "diskpart",
        "cipher /w",
        "dd if=",
        // Network dangerous commands
        "iptables -f",
        "route delete",
        "netsh firewall",
        // Process killing
        "kill -9",
        "taskkill /f /im",
        // Registry manipulation (Windows)
        "reg delete",
        "reg add hkcr",
        // Environment manipulation
        "setx ",
        // File system corruption
        "chmod 000",
        "chown root:root",
        // Cryptocurrency mining (often malicious)
        "xmrig",
        "minerd",
        // Reverse shells and network tools
        "nc -e",
        "ncat -e",
        "socat ",
        // Password extraction
        "mimikatz",
        "procdump",
    ];

    patterns.iter().copied().find(|p| lower.contains(*p))
}

/// Validate that a path is within allowed directories
/// Returns Ok(()) if allowed, Err(message) if denied
pub fn validate_path_access(
    path: &std::path::Path,
    workspace_root: &std::path::Path,
    additional_directories: &[std::path::PathBuf],
) -> Result<(), String> {
    let normalized_path = normalize_path_secure(path).map_err(|e| e.to_string())?;
    let normalized_workspace = normalize_path_secure(workspace_root).map_err(|e| e.to_string())?;

    // Check if path is within workspace
    if normalized_path.starts_with(&normalized_workspace) {
        return Ok(());
    }

    // Check if path is within additional allowed directories
    for dir in additional_directories {
        let normalized_dir = normalize_path_secure(dir).map_err(|e| e.to_string())?;
        if normalized_path.starts_with(&normalized_dir) {
            return Ok(());
        }
    }

    Err(format!(
        "Access denied: path '{}' is outside allowed directories",
        path.display()
    ))
}

/// Resource limits for tool execution
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    pub max_memory_mb: Option<u64>,
    pub max_cpu_percent: Option<f32>,
    pub max_execution_time_sec: Option<u64>,
    pub max_file_size_mb: Option<u64>,
    pub max_process_count: Option<u32>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_memory_mb: Some(1024),        // 1GB
            max_cpu_percent: Some(50.0),      // 50% CPU
            max_execution_time_sec: Some(30), // 30 seconds
            max_file_size_mb: Some(10),       // 10MB max file size
            max_process_count: Some(10),      // Max 10 child processes
        }
    }
}

/// Check if current resource usage is within limits
/// This is a basic implementation - in production you'd want to use
/// proper process monitoring libraries
pub fn check_resource_limits(_limits: &ResourceLimits) -> Result<(), String> {
    // Basic implementation - in a real system you would:
    // 1. Monitor process memory usage
    // 2. Check CPU time
    // 3. Track file sizes
    // 4. Count child processes

    // For now, just return success
    // TODO: Implement proper resource monitoring
    Ok(())
}

#[cfg(test)]
mod secret_file_tests {
    use super::{guard_secret_file_access, is_secret_file_path, redact_secrets};

    #[test]
    fn redacts_credential_literals_but_keeps_context() {
        let out = redact_secrets("notes.txt \"API_KEY = sk-PLACEHOLDERPLACEHOLDERPLACEHOLDER00\" AKIAABCDEFGHIJKLMNOP");
        assert_eq!(out, "notes.txt \"API_KEY = [REDACTED]\" [REDACTED]");
        assert_eq!(redact_secrets("cargo test --release"), "cargo test --release");
    }

    #[test]
    fn classifies_secret_and_template_paths() {
        for p in [".env", "app/.env.local", r"C:\Users\x\.ssh\config", "~/.aws/credentials", "certs/server.PEM", "id_ed25519"] {
            assert!(is_secret_file_path(p), "{} should be secret", p);
        }
        for p in [".env.example", "src/env.rs", "environment.py", "docs/keys.md", "README.md", ""] {
            assert!(!is_secret_file_path(p), "{} should not be secret", p);
        }
    }

    #[test]
    fn guards_every_file_tool_arg_shape() {
        assert!(guard_secret_file_access("read_file", ".env").is_err());
        assert!(guard_secret_file_access("read_file", "\"my app/.env\" 1 50").is_err());
        assert!(guard_secret_file_access("write_file", "server.key <<<CONTENT\nx\n<<<END").is_err());
        assert!(guard_secret_file_access("edit_file", r#"{"path":".env.prod","old_text":"a","new_text":"b"}"#).is_err());
        assert!(guard_secret_file_access("grep_search", "TOKEN .env").is_err());
        assert!(guard_secret_file_access("read_file", "src/main.rs 1 200").is_ok());
        assert!(guard_secret_file_access("bash", "cat .env").is_ok(), "bash is the guard hook's job");
        let err = guard_secret_file_access("read_file", ".env").unwrap_err();
        assert!(err.contains("ASI_ALLOW_SECRET_FILES"), "{}", err);
    }
}

#[cfg(test)]
mod tests {
    use super::{guard_tool_path_access, normalize_path_lexical};
    use std::path::PathBuf;

    #[test]
    fn normalize_path_lexical_cleans_curdir_and_parent_segments() {
        let p = PathBuf::from(r"D:\test_code\.\templates\..\static\index.html");
        let got = normalize_path_lexical(&p);
        assert_eq!(got, PathBuf::from(r"D:\test_code\static\index.html"));
    }

    #[test]
    fn guard_tool_path_access_allows_workspace_relative_write_path() {
        let workspace = PathBuf::from(r"D:\test_code");
        let res = guard_tool_path_access(
            "write_file",
            r#""templates/index.html" "<html></html>""#,
            true,
            &workspace,
            &[],
        );
        assert!(res.is_ok(), "expected allowed path, got {:?}", res);
    }
}
