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
