use glob::glob;
use regex::Regex;
use reqwest::blocking::Client;
use serde::Serialize;
use std::fs;
use std::io::{BufRead, BufReader, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct ToolResult {
    pub ok: bool,
    pub output: String,
}

pub trait ToolRunner: std::fmt::Debug + Send + Sync {
    fn run(&self, name: &str, args: &str) -> ToolResult;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultToolRunner;

impl ToolRunner for DefaultToolRunner {
    fn run(&self, name: &str, args: &str) -> ToolResult {
        run_tool(name, args)
    }
}

impl ToolResult {
    fn ok(output: impl Into<String>) -> Self {
        Self {
            ok: true,
            output: output.into(),
        }
    }
    fn err(output: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: output.into(),
        }
    }
}

pub fn run_tool(name: &str, args: &str) -> ToolResult {
    match name {
        "read_file" => {
            let trimmed = args.trim();
            // Accept both snake_case and camelCase aliases. DeepSeek v4 Pro
            // sometimes hallucinates `filePath`/`fileName` in tool-call JSON
            // even though our schema declares `path`; we forgive that here
            // rather than failing the call.
            let path = extract_json_string_arg(trimmed, &["path", "file_path", "filePath", "fileName", "filename"])
                .or_else(|| extract_key_value_string_arg(trimmed, &["path", "file_path", "filePath", "fileName", "filename"]));
            if let Some(path) = path {
                let start_line = extract_json_usize_arg(trimmed, &["start_line"])
                    .or_else(|| extract_key_value_usize_arg(trimmed, &["start_line"]));
                let max_lines = extract_json_usize_arg(trimmed, &["max_lines"])
                    .or_else(|| extract_key_value_usize_arg(trimmed, &["max_lines"]));
                if let (Some(start_line), Some(max_lines)) = (start_line, max_lines) {
                    read_file_range(&path, start_line, max_lines.clamp(1, MAX_READ_FILE_LINES))
                } else {
                    read_file(&path)
                }
            } else {
                let (path, range) = parse_read_file_args(trimmed);
                if let Some((start_line, max_lines)) = range {
                    read_file_range(path, start_line, max_lines)
                } else {
                    read_file(path)
                }
            }
        }
        "write_file" => {
            let trimmed = args.trim();
            let named_shape = looks_like_named_args(
                trimmed,
                &["path", "file_path", "filePath", "fileName", "filename", "content"],
            );
            let path = extract_json_string_arg(trimmed, &["path", "file_path", "filePath", "fileName", "filename"])
                .or_else(|| extract_key_value_string_arg(trimmed, &["path", "file_path", "filePath", "fileName", "filename"]));
            let content = extract_json_string_arg(trimmed, &["content", "fileContent", "text", "body"])
                .or_else(|| {
                    extract_key_value_string_arg(trimmed, &["content", "fileContent", "text", "body"])
                        .map(|v| decode_key_value_text_value(&v))
                });
            if let (Some(path), Some(content)) = (path, content) {
                write_file(&path, &content)
            } else if named_shape {
                if let Some((path, content)) = parse_named_write_args_lossy(trimmed) {
                    write_file(&path, &content)
                } else {
                    ToolResult::err(
                        "write_file requires both path and content (named-arg format detected)"
                            .to_string(),
                    )
                }
            } else {
                let (path, content) = parse_delimited_write(args);
                write_file(&path, &content)
            }
        }
        "edit_file" => {
            let trimmed = args.trim();
            let named_shape = looks_like_named_args(
                trimmed,
                &[
                    "path", "file_path", "filePath", "fileName", "filename",
                    "old_text", "new_text", "oldText", "newText",
                    "old_string", "new_string", "oldString", "newString",
                ],
            );
            let path = extract_json_string_arg(trimmed, &["path", "file_path", "filePath", "fileName", "filename"])
                .or_else(|| extract_key_value_string_arg(trimmed, &["path", "file_path", "filePath", "fileName", "filename"]));
            let old_text = extract_json_string_arg(trimmed, &["old_text", "oldText", "old_string", "oldString"])
                .or_else(|| {
                    extract_key_value_string_arg(trimmed, &["old_text", "oldText", "old_string", "oldString"])
                        .map(|v| decode_key_value_text_value(&v))
                });
            let new_text = extract_json_string_arg(trimmed, &["new_text", "newText", "new_string", "newString"])
                .or_else(|| {
                    extract_key_value_string_arg(trimmed, &["new_text", "newText", "new_string", "newString"])
                        .map(|v| decode_key_value_text_value(&v))
                });
            if let (Some(path), Some(old_text), Some(new_text)) = (path, old_text, new_text) {
                edit_file(&path, &old_text, &new_text)
            } else if named_shape {
                if let Some((path, old_text, new_text)) = parse_named_edit_args_lossy(trimmed) {
                    edit_file(&path, &old_text, &new_text)
                } else {
                    ToolResult::err(
                        "edit_file requires path, old_text, and new_text (named-arg format detected)"
                            .to_string(),
                    )
                }
            } else {
                let parsed = parse_delimited_edit(args);
                edit_file(&parsed.0, &parsed.1, &parsed.2)
            }
        }
        "glob_search" => {
            let pattern = extract_json_string_arg(args.trim(), &["pattern"])
                .or_else(|| extract_key_value_string_arg(args.trim(), &["pattern"]))
                .unwrap_or_else(|| strip_surrounding_quotes(args.trim()).to_string());
            glob_search(&pattern)
        }
        "grep_search" => {
            let pattern = extract_json_string_arg(args.trim(), &["regex", "pattern"])
                .or_else(|| extract_key_value_string_arg(args.trim(), &["regex", "pattern"]));
            if let Some(pattern) = pattern {
                let base = extract_json_string_arg(args.trim(), &["base_path", "path"])
                    .or_else(|| extract_key_value_string_arg(args.trim(), &["base_path", "path"]))
                    .unwrap_or_else(|| ".".to_string());
                grep_search(&pattern, &base)
            } else {
                let normalized = normalize_grep_search_args(args);
                let (pattern, base) = split_two_default(&normalized, ".");
                grep_search(pattern, base)
            }
        }
        "web_search" => {
            let query = extract_json_string_arg(args.trim(), &["query", "q"])
                .or_else(|| extract_key_value_string_arg(args.trim(), &["query", "q"]))
                .unwrap_or_else(|| strip_surrounding_quotes(args.trim()).to_string());
            web_search(&query)
        }
        "web_fetch" => {
            let url = extract_json_string_arg(args.trim(), &["url"])
                .or_else(|| extract_key_value_string_arg(args.trim(), &["url"]))
                .unwrap_or_else(|| strip_surrounding_quotes(args.trim()).to_string());
            web_fetch(&url)
        }
        "bash" => {
            let cmd = extract_json_string_arg(args.trim(), &["command", "cmd", "shell", "script"])
                .or_else(|| extract_key_value_string_arg(args.trim(), &["command", "cmd", "shell", "script"]))
                .unwrap_or_else(|| normalize_bash_args(args));
            bash(&cmd)
        }
        _ => ToolResult::err(format!("Unknown tool: {}", name)),
    }
}

fn extract_json_string_arg(raw: &str, keys: &[&str]) -> Option<String> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let obj = parsed.as_object()?;
    for key in keys {
        if let Some(v) = obj.get(*key) {
            if let Some(s) = v.as_str() {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn extract_json_usize_arg(raw: &str, keys: &[&str]) -> Option<usize> {
    let trimmed = raw.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let obj = parsed.as_object()?;
    for key in keys {
        if let Some(v) = obj.get(*key) {
            if let Some(n) = v.as_u64() {
                return Some(n as usize);
            }
            if let Some(s) = v.as_str() {
                if let Ok(parsed) = s.parse::<usize>() {
                    return Some(parsed);
                }
            }
        }
    }
    None
}

pub(crate) fn extract_key_value_string_arg(raw: &str, keys: &[&str]) -> Option<String> {
    let pairs = parse_key_value_args(raw)?;
    for key in keys {
        if let Some((_, value)) = pairs
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        {
            return Some(value.clone());
        }
    }
    None
}

fn extract_key_value_usize_arg(raw: &str, keys: &[&str]) -> Option<usize> {
    extract_key_value_string_arg(raw, keys)?
        .trim()
        .parse::<usize>()
        .ok()
}

fn parse_key_value_args(raw: &str) -> Option<Vec<(String, String)>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('{') || !trimmed.contains('=') {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut idx = 0usize;
    while idx < bytes.len() {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() {
            break;
        }

        let key_start = idx;
        while idx < bytes.len()
            && (bytes[idx].is_ascii_alphanumeric() || bytes[idx] == b'_' || bytes[idx] == b'-')
        {
            idx += 1;
        }
        if key_start == idx {
            return None;
        }
        let key_end = idx;

        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() || bytes[idx] != b'=' {
            return None;
        }
        idx += 1;
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }

        let (value, next_idx) = parse_key_value_value(trimmed, idx)?;
        out.push((trimmed[key_start..key_end].to_ascii_lowercase(), value));
        idx = next_idx;
    }

    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn parse_key_value_value(raw: &str, start_idx: usize) -> Option<(String, usize)> {
    if start_idx >= raw.len() {
        return Some((String::new(), start_idx));
    }
    let rest = &raw[start_idx..];
    let Some(first) = rest.chars().next() else {
        return Some((String::new(), start_idx));
    };

    if first == '"' || first == '\'' {
        let quote = first;
        let mut value = String::new();
        let mut escaped = false;
        for (off, ch) in rest.char_indices().skip(1) {
            if escaped {
                match ch {
                    '\\' => value.push('\\'),
                    '"' if quote == '"' => value.push('"'),
                    '\'' if quote == '\'' => value.push('\''),
                    _ => {
                        value.push('\\');
                        value.push(ch);
                    }
                }
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote {
                let next_idx = start_idx + off + ch.len_utf8();
                return Some((value, next_idx));
            }
            value.push(ch);
        }
        if escaped {
            value.push('\\');
        }
        return Some((value, raw.len()));
    }

    let mut end_idx = raw.len();
    for (off, ch) in rest.char_indices() {
        if ch.is_whitespace() {
            end_idx = start_idx + off;
            break;
        }
    }
    Some((rest[..end_idx - start_idx].to_string(), end_idx))
}

fn decode_key_value_text_value(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut escaped = false;
    for ch in raw.chars() {
        if escaped {
            match ch {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                _ => {
                    out.push('\\');
                    out.push(ch);
                }
            }
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else {
            out.push(ch);
        }
    }
    if escaped {
        out.push('\\');
    }
    out
}

fn looks_like_named_args(raw: &str, keys: &[&str]) -> bool {
    let lower = raw.to_ascii_lowercase();
    keys.iter().any(|key| {
        lower.contains(&format!("{}=", key)) || lower.contains(&format!("{} =", key))
    })
}

#[derive(Clone, Debug)]
struct NamedArgPos {
    key_start: usize,
    value_start: usize,
}

fn find_named_arg_pos(raw: &str, keys: &[&str], start_idx: usize) -> Option<NamedArgPos> {
    if start_idx >= raw.len() {
        return None;
    }
    let bytes = raw.as_bytes();
    let mut idx = start_idx;

    while idx < bytes.len() {
        if idx > 0 && !bytes[idx - 1].is_ascii_whitespace() {
            idx += 1;
            continue;
        }
        if !(bytes[idx].is_ascii_alphanumeric() || bytes[idx] == b'_' || bytes[idx] == b'-') {
            idx += 1;
            continue;
        }

        let key_start = idx;
        while idx < bytes.len()
            && (bytes[idx].is_ascii_alphanumeric() || bytes[idx] == b'_' || bytes[idx] == b'-')
        {
            idx += 1;
        }
        let key_end = idx;
        let key = raw[key_start..key_end].to_ascii_lowercase();
        let is_target = keys.iter().any(|candidate| key.eq_ignore_ascii_case(candidate));

        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx >= bytes.len() || bytes[idx] != b'=' {
            idx = key_end.saturating_add(1);
            continue;
        }
        idx += 1;
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }

        if is_target {
            return Some(NamedArgPos {
                key_start,
                value_start: idx,
            });
        }
    }

    None
}

fn strip_one_outer_quote(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.len() >= 2 {
        let bytes = trimmed.as_bytes();
        let first = bytes[0];
        let last = bytes[trimmed.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

fn parse_named_path_value_lossy(raw: &str, keys: &[&str], value_end: usize) -> Option<String> {
    let pos = find_named_arg_pos(raw, keys, 0)?;
    let end = value_end.max(pos.value_start).min(raw.len());
    let value = raw[pos.value_start..end].trim();
    if value.is_empty() {
        return None;
    }
    Some(strip_one_outer_quote(value).to_string())
}

fn parse_named_write_args_lossy(raw: &str) -> Option<(String, String)> {
    let content_pos = find_named_arg_pos(raw, &["content"], 0)?;
    let path = parse_named_path_value_lossy(raw, &["path", "file_path"], content_pos.key_start)?;
    let content_raw = raw[content_pos.value_start..].trim();
    let content = decode_key_value_text_value(strip_one_outer_quote(content_raw));
    Some((path, content))
}

fn parse_named_edit_args_lossy(raw: &str) -> Option<(String, String, String)> {
    let old_pos = find_named_arg_pos(raw, &["old_text"], 0)?;
    let new_pos = find_named_arg_pos(raw, &["new_text"], old_pos.value_start)?;
    let path = parse_named_path_value_lossy(raw, &["path", "file_path"], old_pos.key_start)?;
    let old_raw = raw[old_pos.value_start..new_pos.key_start].trim();
    let new_raw = raw[new_pos.value_start..].trim();
    let old_text = decode_key_value_text_value(strip_one_outer_quote(old_raw));
    let new_text = decode_key_value_text_value(strip_one_outer_quote(new_raw));
    Some((path, old_text, new_text))
}

fn normalize_grep_search_args(args: &str) -> String {
    let trimmed = args.trim();
    if !trimmed.starts_with('{') || !trimmed.ends_with('}') {
        return trimmed.to_string();
    }
    let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return trimmed.to_string(),
    };
    let Some(obj) = parsed.as_object() else {
        return trimmed.to_string();
    };
    let pattern = obj
        .get("regex")
        .or_else(|| obj.get("pattern"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let base = obj
        .get("base_path")
        .or_else(|| obj.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    format!("{} {}", pattern, base)
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

fn normalize_bash_args(args: &str) -> String {
    let mut out = normalize_wrapped_bash_command(args.trim());
    // Some models emit JSON-escaped shell fragments such as:
    //   \"D:\\path\" or \"import sys\"
    // Unescape these common forms before handing off to PowerShell.
    if out.contains("\\\"") || out.contains("\\'") || out.contains("\\\\") {
        out = out.replace("\\\"", "\"");
        out = out.replace("\\'", "'");
        out = out.replace("\\\\", "\\");
    }
    out
}

fn normalize_wrapped_bash_command(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let first = match trimmed.chars().next() {
        Some(c @ '"') | Some(c @ '\'') => c,
        _ => return strip_surrounding_quotes(trimmed).trim().to_string(),
    };
    let mut escaped = false;
    for (idx, ch) in trimmed.char_indices().skip(1) {
        if first == '"' && ch == '\\' && !escaped {
            escaped = true;
            continue;
        }
        if ch == first && !escaped {
            let inner = &trimmed[1..idx];
            let tail = trimmed[idx + ch.len_utf8()..].trim();
            if tail.is_empty() {
                return inner.trim().to_string();
            }
            let lower_tail = tail.to_ascii_lowercase();
            if lower_tail.starts_with("2>&1")
                || lower_tail.starts_with("|")
                || lower_tail.starts_with(";")
            {
                return format!("{} {}", inner.trim(), tail);
            }
            break;
        }
        escaped = false;
    }
    strip_surrounding_quotes(trimmed).trim().to_string()
}

fn rewrite_python_herestring_for_powershell(command: &str) -> String {
    // Convert bash-style here-string, but only for python invocations that
    // directly contain `<<< ...` in the same command segment. Prefix capture
    // ensures we bind to the closest python after start/; /&& /||.
    let re = match Regex::new(
        r#"(?i)(^|(?:&&|\|\||;)\s*)(python(?:3|\.exe)?(?:\s+(?:"[^"]*"|'[^']*'|[^|;&\s]+))*)\s*<<<\s*('(?:[^']*)'|"(?:[^"]*)")"#,
    ) {
        Ok(v) => v,
        Err(_) => return command.to_string(),
    };
    re.replace_all(command, |caps: &regex::Captures| {
        let prefix = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let lhs = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        let rhs = caps.get(3).map(|m| m.as_str().trim()).unwrap_or("");
        format!("{}{} | {}", prefix, rhs, lhs)
    })
    .to_string()
}

fn rewrite_python_dash_c_single_quoted_script(command: &str) -> String {
    // PowerShell can mangle nested escaped quotes in:
    //   python -c '...\"...\"...'
    // Rewrite to double-quoted -c payload with PowerShell-safe escaping.
    let re = match Regex::new(r#"(?i)\b(python(?:3|\.exe)?)\s+-c\s+'([^']*)'"#) {
        Ok(v) => v,
        Err(_) => return command.to_string(),
    };
    re.replace_all(command, |caps: &regex::Captures| {
        let py = caps.get(1).map(|m| m.as_str()).unwrap_or("python");
        let script_raw = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        let script = script_raw.replace("\\\"", "\"");
        let escaped = escape_powershell_double_quoted(&script);
        format!(r#"{py} -c "{}""#, escaped)
    })
    .to_string()
}

fn rewrite_bash_group_pipe_python_for_powershell(command: &str) -> String {
    // Convert Bash group input pattern:
    //   { echo +; echo 10; echo 5; echo q; } | python3 main.py
    // into PowerShell-native:
    //   @('+', '10', '5', 'q') | python3 main.py
    let re = match Regex::new(
        r#"(?i)\{\s*((?:echo\s+[^;{}|]+;\s*)+)\}\s*\|\s*(python(?:3|\.exe)?(?:\s+(?:"[^"]*"|'[^']*'|[^|;&\s]+))*)"#,
    ) {
        Ok(v) => v,
        Err(_) => return command.to_string(),
    };

    re.replace_all(command, |caps: &regex::Captures| {
        let echoes = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let py_cmd = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("python");
        let item_re = match Regex::new(r#"(?i)echo\s+([^;{}|]+)\s*;"#) {
            Ok(v) => v,
            Err(_) => return caps.get(0).map(|m| m.as_str()).unwrap_or("").to_string(),
        };
        let mut items = Vec::new();
        for cap in item_re.captures_iter(echoes) {
            let raw = cap.get(1).map(|m| m.as_str()).unwrap_or("").trim();
            if raw.is_empty() {
                continue;
            }
            let token = strip_surrounding_quotes(raw);
            let escaped = token.replace('\'', "''");
            items.push(format!("'{}'", escaped));
        }

        if items.is_empty() {
            caps.get(0).map(|m| m.as_str()).unwrap_or("").to_string()
        } else {
            format!("@({}) | {}", items.join(", "), py_cmd)
        }
    })
    .to_string()
}

fn split_shell_words(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escape = false;

    for ch in input.chars() {
        if let Some(q) = quote {
            cur.push(ch);
            if q == '"' && ch == '\\' && !escape {
                escape = true;
                continue;
            }
            if ch == q && !escape {
                quote = None;
            } else {
                escape = false;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                cur.push(ch);
            }
            c if c.is_whitespace() => {
                let token = cur.trim();
                if !token.is_empty() {
                    out.push(token.to_string());
                }
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }

    let token = cur.trim();
    if !token.is_empty() {
        out.push(token.to_string());
    }

    out
}

fn extract_parenthesized_pattern(input: &str) -> Option<(String, usize)> {
    let bytes = input.as_bytes();
    if bytes.first().copied()? != b'(' {
        return None;
    }
    let mut depth = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escape = false;

    for (idx, ch) in input.char_indices() {
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            continue;
        }
        if in_double {
            if escape {
                escape = false;
                continue;
            }
            if ch == '\\' {
                escape = true;
                continue;
            }
            if ch == '"' {
                in_double = false;
            }
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let body = input[1..idx].trim().to_string();
                    return Some((body, idx + 1));
                }
            }
            _ => {}
        }
    }

    None
}

fn rewrite_grep_pipe_for_powershell(command: &str) -> String {
    let re = match Regex::new(r"(?i)\|\s*grep(?:\s+(-E|-e))?\s+") {
        Ok(v) => v,
        Err(_) => return command.to_string(),
    };

    let mut out = String::with_capacity(command.len() + 32);
    let mut cursor = 0usize;

    while let Some(mat) = re.find(&command[cursor..]) {
        let start = cursor + mat.start();
        let end = cursor + mat.end();
        out.push_str(&command[cursor..start]);

        let rem = command[end..].trim_start();
        let mut consumed = command[end..].len() - rem.len();

        let (pattern, used) = if let Some((p, used)) = extract_parenthesized_pattern(rem) {
            (p, used)
        } else if rem.starts_with('"') || rem.starts_with('\'') {
            let quote = rem.chars().next().unwrap_or('"');
            let mut esc = false;
            let mut close_idx = None;
            for (idx, ch) in rem.char_indices().skip(1) {
                if quote == '"' && ch == '\\' && !esc {
                    esc = true;
                    continue;
                }
                if ch == quote && !esc {
                    close_idx = Some(idx);
                    break;
                }
                esc = false;
            }
            if let Some(idx) = close_idx {
                (rem[1..idx].to_string(), idx + 1)
            } else {
                (rem.to_string(), rem.len())
            }
        } else {
            let mut token_end = rem.len();
            for sep in [" |", ";", "&&", "||"] {
                if let Some(idx) = rem.find(sep) {
                    token_end = token_end.min(idx);
                }
            }
            (rem[..token_end].trim().to_string(), token_end)
        };

        out.push_str("| Select-String -Pattern ");
        out.push('\'');
        out.push_str(&pattern.replace('\'', "''"));
        out.push('\'');

        consumed += used;
        cursor = end + consumed;
    }

    out.push_str(&command[cursor..]);
    out
}

fn normalize_windows_shell_command(command: &str) -> String {
    let mut cmd = command.trim().to_string();
    if cmd.is_empty() {
        return cmd;
    }

    cmd = rewrite_python_herestring_for_powershell(&cmd);
    cmd = rewrite_python_dash_c_single_quoted_script(&cmd);
    cmd = rewrite_bash_group_pipe_python_for_powershell(&cmd);
    cmd = rewrite_grep_pipe_for_powershell(&cmd);

    // PowerShell 5.1 doesn't support `&&`.
    cmd = cmd.replace("&&", ";");
    // Keep behavior predictable across PowerShell versions: treat `||` as
    // sequential fallback separator (best-effort Unix->PowerShell rewrite).
    cmd = cmd.replace("||", ";");

    if let Ok(re) = Regex::new(r#"(?i)\becho\s+-e\s+('([^']*)'|"((?:[^"\\]|\\.)*)")"#) {
        cmd = re
            .replace_all(&cmd, |caps: &regex::Captures| {
                let raw = caps
                    .get(2)
                    .or_else(|| caps.get(3))
                    .map(|m| m.as_str())
                    .unwrap_or("");
                let decoded = decode_echo_dash_e_escapes(raw);
                let escaped = escape_powershell_double_quoted(&decoded);
                format!("Write-Output \"{}\"", escaped)
            })
            .to_string();
    }
    if let Ok(re) = Regex::new(r"(?i)\becho\s+-e\b") {
        // PowerShell `echo` alias does not support `-e`.
        cmd = re.replace_all(&cmd, "echo").to_string();
    }
    if let Ok(re) = Regex::new(r"(?i)(^|;)\s*mkdir\s+-p\s+([^;|]+)") {
        cmd = re
            .replace_all(&cmd, |caps: &regex::Captures| {
                let prefix = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let path_expr = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
                let paths = split_shell_words(path_expr);
                let normalized_path_expr = if paths.len() <= 1 {
                    path_expr.to_string()
                } else {
                    paths.join(", ")
                };
                let spacer = if prefix.is_empty() { "" } else { " " };
                format!(
                    "{}{}New-Item -ItemType Directory -Force -Path {} | Out-Null",
                    prefix, spacer, normalized_path_expr
                )
            })
            .to_string();
    }

    // Common Unix aliases/patterns produced by LLMs.
    if let Ok(re) = Regex::new(r"(?i)\bls\s+-la\b|\bls\s+-al\b") {
        cmd = re.replace_all(&cmd, "Get-ChildItem -Force").to_string();
    }
    if let Ok(re) = Regex::new(r"(?i)\|\s*head\s*-\s*(\d+)") {
        cmd = re
            .replace_all(&cmd, "| Select-Object -First $1")
            .to_string();
    }
    if let Ok(re) = Regex::new(r"(?i)\|\s*head\b") {
        cmd = re
            .replace_all(&cmd, "| Select-Object -First 10")
            .to_string();
    }
    if let Ok(re) = Regex::new(r"(?i)\|\s*tail\s*-\s*(\d+)") {
        cmd = re
            .replace_all(&cmd, "| Select-Object -Last $1")
            .to_string();
    }
    if let Ok(re) = Regex::new(r"(?i)\|\s*tail\b") {
        cmd = re
            .replace_all(&cmd, "| Select-Object -Last 10")
            .to_string();
    }
    if let Ok(re) = Regex::new(r"(?i)\bcurl\s+-s\b") {
        // Force native curl binary instead of PowerShell alias.
        cmd = re.replace_all(&cmd, "curl.exe -s").to_string();
    }
    if let Ok(re) = Regex::new(r"(?i)\bnpm(?:\.cmd)?\b") {
        // Avoid npm.ps1 execution-policy failures in PowerShell.
        cmd = re.replace_all(&cmd, "npm.cmd").to_string();
    }
    if let Ok(re) = Regex::new(r"(?i)\bnpx(?:\.cmd)?\b") {
        cmd = re.replace_all(&cmd, "npx.cmd").to_string();
    }
    if let Ok(re) = Regex::new(r"(?i)\bpnpm(?:\.cmd)?\b") {
        cmd = re.replace_all(&cmd, "pnpm.cmd").to_string();
    }
    if let Ok(re) = Regex::new(r"(?i)\s+2>&1\b") {
        // This tool already captures stdout/stderr separately on Windows, so
        // explicit `2>&1` is usually redundant and can trigger PowerShell
        // parser/runtime quirks (for example after if/else blocks).
        cmd = re.replace_all(&cmd, "").to_string();
    }

    cmd
}

fn with_windows_utf8_preamble(command: &str) -> String {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    format!(
        "$OutputEncoding = [System.Text.UTF8Encoding]::new($false); [Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); {}",
        trimmed
    )
}

fn decode_echo_dash_e_escapes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn escape_powershell_double_quoted(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '`' => out.push_str("``"),
            '"' => out.push_str("`\""),
            '$' => out.push_str("`$"),
            _ => out.push(ch),
        }
    }
    out
}

fn windows_missing_yarn_error(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    (lower.contains("yarn") && lower.contains("commandnotfoundexception"))
        || lower.contains("yarn : the term 'yarn' is not recognized as the name of a cmdlet")
        || lower.contains("'yarn' is not recognized as an internal or external command")
}

fn windows_shell_error_hint(output: &str) -> Option<&'static str> {
    if windows_missing_yarn_error(output) {
        return Some(
            "[hint] Detected missing `yarn`. Install manually with `npm.cmd install -g yarn` (or `corepack enable`) and retry. To allow automatic install for this CLI, set `ASI_AUTO_INSTALL_YARN=1`.",
        );
    }
    if windows_connect_timeout_error(output) {
        return Some(
            "[hint] Detected outbound network timeout while downloading remote assets. This is usually a network/proxy/firewall issue (not a script-syntax issue). Check connectivity/proxy (HTTP_PROXY/HTTPS_PROXY), then retry.",
        );
    }

    None
}

fn auto_install_yarn_enabled() -> bool {
    std::env::var("ASI_AUTO_INSTALL_YARN")
        .ok()
        .map(|v| {
            let s = v.trim().to_ascii_lowercase();
            !matches!(s.as_str(), "0" | "false" | "no" | "off")
        })
        .unwrap_or(false)
}

fn should_auto_recover_yarn(shell_command: &str, output: &str) -> bool {
    if !windows_missing_yarn_error(output) {
        return false;
    }
    let cmd = shell_command.to_ascii_lowercase();
    if cmd.contains("npm run") || cmd.contains("npm.cmd run") {
        return true;
    }
    if cmd.contains("yarn") {
        if cmd.contains("yarn --version")
            || cmd.contains("yarn -v")
            || cmd.contains("yarn --help")
        {
            return false;
        }
        return true;
    }
    false
}

fn append_block(out: &mut String, title: &str, body: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(title);
    if !body.trim().is_empty() {
        out.push('\n');
        out.push_str(body.trim());
    }
}

const YARN_AUTOFIX_GUARD_ENV: &str = "ASI_BASH_YARN_AUTOFIX_ACTIVE";
const PYTHON_ALIAS_AUTOFIX_GUARD_ENV: &str = "ASI_BASH_PY_ALIAS_AUTOFIX_ACTIVE";
const CACHE_DIR_AUTOFIX_GUARD_ENV: &str = "ASI_BASH_CACHE_DIR_AUTOFIX_ACTIVE";
fn ensure_windows_npm_cache_dir() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("npm_config_cache") {
        let path = PathBuf::from(raw);
        if fs::create_dir_all(&path).is_ok() {
            return Some(path);
        }
    }

    let cwd = std::env::current_dir().ok()?;
    let fallback = cwd.join(".asi").join("npm-cache");
    if fs::create_dir_all(&fallback).is_ok() {
        Some(fallback)
    } else {
        None
    }
}
fn windows_permission_error(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("eperm")
        || lower.contains("operation not permitted")
        || lower.contains("access is denied")
        || lower.contains("permission denied")
}

fn windows_connect_timeout_error(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("und_err_connect_timeout")
        || lower.contains("connecttimeouterror")
        || lower.contains("etimedout")
        || (lower.contains("fetch failed") && lower.contains("timeout"))
}

fn windows_file_exists_path(output: &str) -> Option<String> {
    let re = Regex::new(r"(?i)fileexistserror:\s*\[winerror\s*183\][^\r\n]*?:\s*'([^']+)'").ok()?;
    let caps = re.captures(output)?;
    Some(caps.get(1)?.as_str().to_string())
}

fn windows_missing_path_error_path(output: &str) -> Option<String> {
    let re1 = Regex::new(r"(?i)could not find a part of the path '([^']+)'").ok()?;
    if let Some(caps) = re1.captures(output) {
        if let Some(m) = caps.get(1) {
            return Some(m.as_str().to_string());
        }
    }
    let re2 = Regex::new(r"(?i)cannot find path '([^']+)'").ok()?;
    let caps = re2.captures(output)?;
    Some(caps.get(1)?.as_str().to_string())
}

fn is_autoresearch_cache_path(path: &str) -> bool {
    let mut normalized = String::with_capacity(path.len());
    let mut in_sep = false;
    for ch in path.chars() {
        if ch == '\\' || ch == '/' {
            if !in_sep {
                normalized.push('\\');
                in_sep = true;
            }
        } else {
            in_sep = false;
            normalized.push(ch);
        }
    }
    let lower = normalized.to_ascii_lowercase();
    lower.contains("\\.cache\\autoresearch\\")
}

fn should_auto_recover_windows_cache_dir(shell_command: &str, output: &str) -> Option<String> {
    let lower_cmd = shell_command.to_ascii_lowercase();
    if lower_cmd.contains("prepare.py")
        || lower_cmd.contains("train.py")
        || lower_cmd.contains("python ")
        || lower_cmd.contains("python3 ")
    {
        let path = windows_file_exists_path(output)?;
        if is_autoresearch_cache_path(&path) {
            return Some(path);
        }
    }
    if lower_cmd.contains("copy ")
        || lower_cmd.contains("copy-item")
        || lower_cmd.contains("cp ")
        || lower_cmd.contains("mkdir ")
    {
        let path = windows_missing_path_error_path(output)?;
        let candidate = Path::new(&path);
        let target = if candidate.extension().is_some() {
            candidate
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or(path)
        } else {
            path
        };
        if is_autoresearch_cache_path(&target) {
            return Some(target);
        }
    }
    None
}

fn escape_powershell_single_quoted(input: &str) -> String {
    input.replace('\'', "''")
}

fn build_windows_cache_dir_repair_command(path: &str) -> String {
    let escaped = escape_powershell_single_quoted(path);
    format!(
        "$p='{escaped}'; \
if (Test-Path -LiteralPath $p) {{ \
  $item = Get-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue; \
  if ($item -and ((-not $item.PSIsContainer) -or (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0))) {{ \
    Remove-Item -LiteralPath $p -Force -Recurse -ErrorAction SilentlyContinue; \
  }} \
}}; \
New-Item -ItemType Directory -Force -Path $p | Out-Null; \
Write-Output ('[auto-fix] ensured directory: ' + $p)"
    )
}

fn windows_python_alias_error(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    let alias_path = lower.contains("windowsapps") && lower.contains("pythonsoftwarefoundation.python");
    let invalid_executable = lower.contains("oserror: [errno 22] invalid argument");
    let launcher_failed = lower.contains("unable to create process using");
    (alias_path && invalid_executable) || launcher_failed
}

fn should_auto_recover_python_alias(shell_command: &str, output: &str) -> bool {
    if !windows_python_alias_error(output) {
        return false;
    }
    let has_python = Regex::new(r"(?i)\bpython\b")
        .map(|re| re.is_match(shell_command))
        .unwrap_or(false);
    let has_python3 = Regex::new(r"(?i)\bpython3\b")
        .map(|re| re.is_match(shell_command))
        .unwrap_or(false);
    let lower = shell_command.to_ascii_lowercase();
    let has_explicit_venv_python =
        lower.contains(".venv\\scripts\\python.exe") || lower.contains(".venv/scripts/python.exe");
    (has_python || has_python3) && !has_explicit_venv_python
}

fn rewrite_python_command_to_python3(shell_command: &str) -> Option<String> {
    let re = Regex::new(r"(?i)\bpython\b").ok()?;
    if !re.is_match(shell_command) {
        return None;
    }
    Some(re.replace_all(shell_command, "python3").to_string())
}

fn rewrite_python_command_to_py_launcher(shell_command: &str) -> Option<String> {
    let re = Regex::new(r"(?i)\bpython\b").ok()?;
    if !re.is_match(shell_command) {
        return None;
    }
    Some(re.replace_all(shell_command, "py -3").to_string())
}

fn rewrite_python_command_to_venv_python(shell_command: &str) -> Option<String> {
    let re = Regex::new(r"(?i)\bpython\b").ok()?;
    if !re.is_match(shell_command) {
        return None;
    }
    Some(
        re.replace_all(shell_command, r".\.venv\Scripts\python.exe")
            .to_string(),
    )
}

fn rewrite_python3_command_to_venv_python(shell_command: &str) -> Option<String> {
    let re = Regex::new(r"(?i)\bpython3\b").ok()?;
    if !re.is_match(shell_command) {
        return None;
    }
    Some(
        re.replace_all(shell_command, r".\.venv\Scripts\python.exe")
            .to_string(),
    )
}

fn parse_delimited_write(args: &str) -> (String, String) {
    let trimmed = args.trim();

    // Generic heredoc support:
    // /toolcall write_file <path> <<<EOF\n...\nEOF
    // /toolcall write_file <path> <<<CONTENT\n...\n<<<END
    if let Some(idx) = trimmed.find("<<<") {
        let path = strip_surrounding_quotes(trimmed[..idx].trim()).to_string();
        let after = &trimmed[idx + 3..];
        let mut parts = after.splitn(2, '\n');
        let marker = parts.next().unwrap_or("").trim();
        let body = parts.next().unwrap_or("");

        let mut lines = Vec::new();
        for line in body.lines() {
            let t = line.trim_end_matches('\r');
            if t == "<<<END" || (!marker.is_empty() && t == marker) {
                break;
            }
            lines.push(line);
        }

        return (path, lines.join("\n"));
    }

    // Fallback: first token is path, rest is content. Decode common backslash
    // escapes (\n, \t, \", ...) so positional `write_file <path> "<content>"`
    // calls with embedded escapes produce real newlines on disk instead of the
    // literal two characters `\` and `n`. Without this, models that emit the
    // shape `write_file solution.py "def f():\n    return 1"` write a single-
    // line file with a literal backslash-n in it, which then fails py_compile.
    let (path, content) = split_two(trimmed);
    (
        decode_key_value_text_value(strip_surrounding_quotes(path)),
        decode_key_value_text_value(strip_surrounding_quotes(content)),
    )
}

/// Parse edit_file args supporting both:
///   /toolcall edit_file <path> <<<OLD\n...\n<<<NEW\n...\n<<<END
///   /toolcall edit_file <path> <old> <new>  (legacy single-line)
fn parse_delimited_edit(args: &str) -> (String, String, String) {
    let trimmed = args.trim();

    // Check for <<<OLD delimiter
    if let Some(old_idx) = trimmed.find("<<<OLD") {
        let path = strip_surrounding_quotes(trimmed[..old_idx].trim()).to_string();
        let rest = &trimmed[old_idx + "<<<OLD".len()..];
        let rest = rest.strip_prefix('\n').unwrap_or(rest);

        if let Some(new_idx) = rest.find("<<<NEW") {
            let old_text = rest[..new_idx].trim_end_matches('\n').to_string();
            let after_new = &rest[new_idx + "<<<NEW".len()..];
            let after_new = after_new.strip_prefix('\n').unwrap_or(after_new);
            let new_text = if let Some(end_idx) = after_new.rfind("<<<END") {
                after_new[..end_idx].trim_end_matches('\n').to_string()
            } else {
                after_new.to_string()
            };
            return (path, old_text, new_text);
        }

        // Only <<<OLD without <<<NEW - treat rest as old, new is empty
        let old_text = if let Some(end_idx) = rest.rfind("<<<END") {
            rest[..end_idx].trim_end_matches('\n').to_string()
        } else {
            rest.to_string()
        };
        return (path, old_text, String::new());
    }

    // Legacy fallback: path old new (space-separated). Decode common
    // backslash escapes so positional edit_file calls with embedded `\n`
    // produce real newlines.
    let (path, rest) = split_two(trimmed);
    let (old, new) = split_two(rest);
    (
        decode_key_value_text_value(strip_surrounding_quotes(path)),
        decode_key_value_text_value(strip_surrounding_quotes(old)),
        decode_key_value_text_value(strip_surrounding_quotes(new)),
    )
}

fn split_two(s: &str) -> (&str, &str) {
    let trimmed = s.trim();
    // Support quoted first argument with either quote type.
    if let Some(quote) = trimmed.chars().next().filter(|c| *c == '"' || *c == '\'') {
        if let Some(end) = trimmed[1..].find(quote) {
            let first = &trimmed[1..end + 1];
            let rest = trimmed[end + 2..].trim();
            return (first, rest);
        }
    }
    let mut it = trimmed.splitn(2, ' ');
    let first = it.next().unwrap_or("");
    let second = it.next().unwrap_or("");
    (first, second)
}

fn split_two_default<'a>(s: &'a str, default_second: &'a str) -> (&'a str, &'a str) {
    let trimmed = s.trim();
    // Support quoted first argument with either quote type.
    if let Some(quote) = trimmed.chars().next().filter(|c| *c == '"' || *c == '\'') {
        if let Some(end) = trimmed[1..].find(quote) {
            let first = &trimmed[1..end + 1];
            let rest = trimmed[end + 2..].trim();
            if rest.is_empty() {
                return (first, default_second);
            }
            return (first, rest);
        }
    }
    let mut it = trimmed.splitn(2, ' ');
    let first = it.next().unwrap_or("");
    let second = it.next().unwrap_or(default_second);
    (first, second)
}

const DEFAULT_READ_FILE_LINES: usize = 300;
const MAX_READ_FILE_LINES: usize = 2000;
const MEDIUM_FILE_FULL_READ_MAX_LINES: usize = 1200;

fn parse_read_file_args(args: &str) -> (&str, Option<(usize, usize)>) {
    let (path, rest) = split_two(args.trim());
    let mut it = rest.split_whitespace();
    let start = it.next().and_then(|v| v.parse::<usize>().ok());
    let count = it.next().and_then(|v| v.parse::<usize>().ok());
    match (start, count) {
        (Some(s), Some(c)) => (path, Some((s, c.clamp(1, MAX_READ_FILE_LINES)))),
        _ => (path, None),
    }
}

pub fn read_file(path: &str) -> ToolResult {
    // For plain `read_file <path>` calls, auto-expand medium files to full
    // content to reduce follow-up paging churn.
    read_file_range_with_policy(path, 1, DEFAULT_READ_FILE_LINES, true)
}

pub fn read_file_range(path: &str, start_line: usize, max_lines: usize) -> ToolResult {
    // For explicit ranged reads, always honor requested pagination.
    read_file_range_with_policy(path, start_line, max_lines, false)
}

fn read_file_range_with_policy(
    path: &str,
    start_line: usize,
    max_lines: usize,
    allow_medium_file_auto_expand: bool,
) -> ToolResult {
    if path.is_empty() {
        return ToolResult::err("path is empty");
    }

    let text = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(e) => return ToolResult::err(e.to_string()),
    };

    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return ToolResult::ok(format!("[read_file:{} lines 0-0 of 0]\n", path));
    }

    let start_idx = if start_line > 0 { start_line - 1 } else { 0 };
    if start_idx >= lines.len() {
        return ToolResult::ok(format!(
            "[read_file:{} lines {}-{} of {}]\n",
            path,
            lines.len(),
            lines.len(),
            lines.len()
        ));
    }

    // Only auto-return full content for non-ranged reads.
    let safe_count = if allow_medium_file_auto_expand
        && lines.len() <= MEDIUM_FILE_FULL_READ_MAX_LINES
        && start_idx == 0
    {
        lines.len()
    } else {
        max_lines.clamp(1, MAX_READ_FILE_LINES)
    };

    let end_idx = (start_idx + safe_count).min(lines.len());
    let body = lines[start_idx..end_idx].join("\n");

    let mut out = format!(
        "[read_file:{} lines {}-{} of {}]\n{}",
        path,
        start_idx + 1,
        end_idx,
        lines.len(),
        body
    );

    if end_idx < lines.len() {
        out.push_str(&format!(
            "\n\n[truncated: next call should use start_line={} to continue reading; batch multiple read_file calls in one response when possible]",
            end_idx + 1
        ));
    }

    ToolResult::ok(out)
}

pub fn write_file(path: &str, content: &str) -> ToolResult {
    let p = Path::new(path);
    if let Some(parent) = p.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            return ToolResult::err(e.to_string());
        }
    }
    match fs::write(path, content) {
        Ok(_) => {
            let mut output = format!("Wrote {} chars to {}", content.len(), path);
            if let Ok(preview) = render_file_preview(path, 120, 5000) {
                output.push_str("\n\n[preview]\n");
                output.push_str(&preview);
            }
            ToolResult::ok(output)
        }
        Err(e) => ToolResult::err(e.to_string()),
    }
}

/// Generate a unified diff showing the change with context lines.
fn generate_unified_diff(file_content: &str, old_text: &str, new_text: &str) -> String {
    // Find the position of old_text in the file
    if let Some(start_byte) = file_content.find(old_text) {
        // Calculate line numbers
        let lines_before = file_content[..start_byte].lines().count();
        let line_start = lines_before + 1; // 1-based line number

        // Count lines in old and new text
        let old_lines_count = old_text.lines().count();
        let new_lines_count = new_text.lines().count();

        // Get context lines before and after the change
        let context_lines = 2; // Show 2 lines before and after

        // Build the diff header
        let mut diff = format!(
            "@@ -{},{} +{},{} @@\n",
            line_start, old_lines_count, line_start, new_lines_count
        );

        // Add context before
        let lines: Vec<&str> = file_content.lines().collect();
        let context_start = if line_start > context_lines {
            line_start - context_lines - 1
        } else {
            0
        };
        for i in context_start..(line_start - 1).min(lines.len()) {
            diff.push_str(&format!("  {}\n", lines[i]));
        }

        // Add removed lines (old text)
        for line in old_text.lines() {
            diff.push_str(&format!("- {}\n", line));
        }

        // Add added lines (new text)
        for line in new_text.lines() {
            diff.push_str(&format!("+ {}\n", line));
        }

        // Add context after
        let after_start = line_start - 1 + old_lines_count;
        let after_end = (after_start + context_lines).min(lines.len());
        for i in after_start..after_end {
            diff.push_str(&format!("  {}\n", lines[i]));
        }

        diff
    } else {
        // Fallback to simple diff if old text not found (should not happen)
        let mut diff = String::from("--- old\n+++ new\n");
        for line in old_text.lines() {
            diff.push_str(&format!("- {}\n", line));
        }
        for line in new_text.lines() {
            diff.push_str(&format!("+ {}\n", line));
        }
        diff
    }
}

pub fn edit_file(path: &str, old: &str, new: &str) -> ToolResult {
    let text = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(e) => return ToolResult::err(e.to_string()),
    };
    if old.is_empty() {
        return ToolResult::err("old text is empty — read the file first to get exact text");
    }

    let file_uses_crlf = text.contains("\r\n");
    let mut old_for_diff = old.to_string();
    let mut new_for_diff = new.to_string();

    let updated = {
        let count = text.matches(old).count();
        if count == 1 {
            text.replacen(old, new, 1)
        } else if count > 1 {
            return ToolResult::err(format!(
                "old text found {} times — provide more surrounding context to make it unique",
                count
            ));
        } else {
            // Fallback for Windows line-ending mismatches (\r\n vs \n).
            let text_lf = text.replace("\r\n", "\n");
            let old_lf = old.replace("\r\n", "\n");
            let new_lf = new.replace("\r\n", "\n");
            let lf_count = text_lf.matches(&old_lf).count();
            if lf_count == 1 {
                let updated_lf = text_lf.replacen(&old_lf, &new_lf, 1);
                old_for_diff = if file_uses_crlf {
                    old_lf.replace('\n', "\r\n")
                } else {
                    old_lf.clone()
                };
                new_for_diff = if file_uses_crlf {
                    new_lf.replace('\n', "\r\n")
                } else {
                    new_lf.clone()
                };
                if file_uses_crlf {
                    updated_lf.replace('\n', "\r\n")
                } else {
                    updated_lf
                }
            } else if lf_count > 1 {
                return ToolResult::err(format!(
                    "old text matched {} times after newline normalization — provide more surrounding context",
                    lf_count
                ));
            } else {
                return ToolResult::err(
                    "old text not found in file — make sure it matches exactly (including whitespace/newlines)",
                );
            }
        }
    };

    match fs::write(path, &updated) {
        Ok(_) => {
            let mut output = format!("Edited {}", path);
            // Show unified diff with context
            output.push_str("\n\n[unified diff]\n");
            output.push_str(&generate_unified_diff(&text, &old_for_diff, &new_for_diff));
            if let Ok(preview) = render_file_preview(path, 60, 3000) {
                output.push_str("\n[preview]\n");
                output.push_str(&preview);
            }
            ToolResult::ok(output)
        }
        Err(e) => ToolResult::err(e.to_string()),
    }
}

fn render_file_preview(path: &str, max_lines: usize, max_chars: usize) -> Result<String, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    if text.is_empty() {
        return Ok("(empty file)\n".to_string());
    }

    let mut out = String::new();
    for (idx, line) in text.lines().take(max_lines).enumerate() {
        out.push_str(&format!("{:>4} | {}\n", idx + 1, line));
        if out.len() >= max_chars {
            out.push_str("... (truncated)\n");
            return Ok(out);
        }
    }

    if text.lines().count() > max_lines {
        out.push_str("... (truncated)\n");
    }
    Ok(out)
}

pub fn glob_search(pattern: &str) -> ToolResult {
    let mut lines = Vec::new();
    if let Ok(entries) = glob(pattern) {
        for (idx, e) in entries.flatten().enumerate() {
            if idx >= 300 {
                break;
            }
            lines.push(e.display().to_string());
        }
    }
    if lines.is_empty() {
        ToolResult::ok("No matches")
    } else {
        ToolResult::ok(lines.join("\n"))
    }
}

/// Maximum number of matching lines collected per grep invocation.
const GREP_MAX_MATCHES: usize = 300;
/// Maximum number of files visited per grep invocation. Stops a runaway
/// walk when the model passes a base_path that is far too broad (e.g. "/"
/// or a drive root).
const GREP_MAX_FILES_VISITED: usize = 5_000;
/// Maximum file size (in bytes) the grep tool will read. Larger files are
/// skipped to keep memory and latency bounded.
const GREP_MAX_FILE_BYTES: u64 = 1_048_576;
/// Directory names that almost never contain user-meaningful matches and
/// commonly hide tens of thousands of generated files. Skipping them is
/// the difference between "instant" and "scans all of D:\" for a regex.
const GREP_PRUNE_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".venv",
    "venv",
    "env",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
    ".idea",
    ".vscode",
    ".next",
    ".nuxt",
    ".cache",
    "coverage",
    ".asi",
    ".codex",
    ".claude",
    "sessions",
    "bench_reports",
];

pub fn grep_search(pattern: &str, base: &str) -> ToolResult {
    let regex = match Regex::new(pattern) {
        Ok(v) => v,
        Err(e) => return ToolResult::err(e.to_string()),
    };
    let base_path = Path::new(base);
    let mut out = Vec::new();
    let mut files_visited: usize = 0;
    if base_path.is_file() {
        search_file(base_path, &regex, &mut out, &mut files_visited);
    } else {
        walk(base_path, &regex, &mut out, &mut files_visited);
    }
    let truncated = files_visited >= GREP_MAX_FILES_VISITED && out.len() < GREP_MAX_MATCHES;
    if out.is_empty() {
        let msg = if truncated {
            format!(
                "No matches (search truncated after visiting {} files)",
                GREP_MAX_FILES_VISITED
            )
        } else {
            "No matches".to_string()
        };
        ToolResult::ok(msg)
    } else if truncated {
        let mut joined = out.join("\n");
        joined.push_str(&format!(
            "\n[grep_search: search truncated after visiting {} files]",
            GREP_MAX_FILES_VISITED
        ));
        ToolResult::ok(joined)
    } else {
        ToolResult::ok(out.join("\n"))
    }
}

fn looks_like_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|b| *b == 0)
}

fn search_file(path: &Path, regex: &Regex, out: &mut Vec<String>, files_visited: &mut usize) {
    if out.len() >= GREP_MAX_MATCHES || !path.is_file() {
        return;
    }
    *files_visited += 1;
    if let Ok(meta) = fs::metadata(path) {
        if meta.len() > GREP_MAX_FILE_BYTES {
            return;
        }
    }
    let bytes = match fs::read(path) {
        Ok(v) => v,
        Err(_) => return,
    };
    if looks_like_binary(&bytes) {
        return;
    }
    let text = match String::from_utf8(bytes) {
        Ok(v) => v,
        Err(_) => return,
    };
    for (i, line) in text.lines().enumerate() {
        if regex.is_match(line) {
            out.push(format!("{}:{}:{}", path.display(), i + 1, line));
            if out.len() >= GREP_MAX_MATCHES {
                return;
            }
        }
    }
}

fn walk(dir: &Path, regex: &Regex, out: &mut Vec<String>, files_visited: &mut usize) {
    if out.len() >= GREP_MAX_MATCHES || *files_visited >= GREP_MAX_FILES_VISITED {
        return;
    }
    let rd = match fs::read_dir(dir) {
        Ok(v) => v,
        Err(_) => return,
    };
    for e in rd.flatten() {
        if out.len() >= GREP_MAX_MATCHES || *files_visited >= GREP_MAX_FILES_VISITED {
            return;
        }
        let path = e.path();
        if path.is_dir() {
            let dir_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if GREP_PRUNE_DIRS.iter().any(|p| *p == dir_name) {
                continue;
            }
            walk(&path, regex, out, files_visited);
            continue;
        }
        if !path.is_file() {
            continue;
        }
        search_file(&path, regex, out, files_visited);
    }
}

/// Default browser-style User-Agent used by web_search backends. Some search
/// pages (Bing especially) serve a stripped or captcha-only layout to bare
/// HTTP clients with no UA, so we always send something realistic.
const WEB_SEARCH_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub fn web_search(query: &str) -> ToolResult {
    if query.is_empty() {
        return ToolResult::err("query is empty");
    }
    // Default backend is Bing because it is reachable from mainland China,
    // where DuckDuckGo is blocked. Override with ASI_SEARCH_BACKEND=duckduckgo
    // (or `ddg`) to restore the previous behavior.
    let backend = std::env::var("ASI_SEARCH_BACKEND")
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "bing".to_string());
    match backend.as_str() {
        "bing" => web_search_bing(query),
        "ddg" | "duckduckgo" => web_search_ddg(query),
        other => ToolResult::err(format!(
            "unknown ASI_SEARCH_BACKEND {:?} (expected: bing | duckduckgo)",
            other
        )),
    }
}

/// Bing HTML scraper. Targets `cn.bing.com` because:
///   - it is reachable on Chinese networks where `duckduckgo.com` is blocked;
///   - it serves the same `<li class="b_algo">` result layout as bing.com.
/// To use the international layout instead, set `ASI_BING_HOST=bing.com`.
fn web_search_bing(query: &str) -> ToolResult {
    let host = std::env::var("ASI_BING_HOST")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "cn.bing.com".to_string());
    let encoded = simple_url_encode(query);
    let url = format!("https://{}/search?q={}", host, encoded);

    let client = match Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ToolResult::err(format!("http client build: {}", e)),
    };
    let body = match client
        .get(&url)
        .header("User-Agent", WEB_SEARCH_USER_AGENT)
        .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.6")
        .header("Accept", "text/html,application/xhtml+xml")
        .send()
    {
        Ok(resp) => match resp.text() {
            Ok(v) => v,
            Err(e) => return ToolResult::err(e.to_string()),
        },
        Err(e) => return ToolResult::err(e.to_string()),
    };

    // Bing wraps each organic result in `<li class="b_algo">`. Within each
    // result, the title link sits in `<h2><a href="URL">TITLE</a></h2>` and
    // the snippet is usually `<p class="b_lineclamp..."> ... </p>` (sometimes
    // nested inside `<div class="b_caption">`).
    let item_re = match Regex::new(r#"<li class="b_algo"[^>]*>([\s\S]*?)</li>"#) {
        Ok(v) => v,
        Err(e) => return ToolResult::err(e.to_string()),
    };
    let title_re = match Regex::new(
        r#"<h2[^>]*>\s*<a[^>]*href="([^"]+)"[^>]*>([\s\S]*?)</a>\s*</h2>"#,
    ) {
        Ok(v) => v,
        Err(e) => return ToolResult::err(e.to_string()),
    };
    let snippet_re = match Regex::new(
        r#"<p[^>]*class="[^"]*b_lineclamp[^"]*"[^>]*>([\s\S]*?)</p>"#,
    ) {
        Ok(v) => v,
        Err(e) => return ToolResult::err(e.to_string()),
    };
    let snippet_alt_re = match Regex::new(r#"<div class="b_caption"[^>]*>[\s\S]*?<p[^>]*>([\s\S]*?)</p>"#) {
        Ok(v) => v,
        Err(e) => return ToolResult::err(e.to_string()),
    };
    let tag_re = Regex::new(r"<[^>]+>").unwrap_or_else(|_| Regex::new("$").expect("regex"));

    let mut lines = Vec::new();
    for cap in item_re.captures_iter(&body).take(8) {
        let block = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let Some(title_cap) = title_re.captures(block) else {
            continue;
        };
        let href = title_cap.get(1).map(|m| m.as_str()).unwrap_or("").trim();
        if href.is_empty() {
            continue;
        }
        let title_html = title_cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let title = decode_html_entities_minimal(&tag_re.replace_all(title_html, "").to_string())
            .trim()
            .to_string();
        let snippet = snippet_re
            .captures(block)
            .or_else(|| snippet_alt_re.captures(block))
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .map(|s| {
                decode_html_entities_minimal(&tag_re.replace_all(&s, "").to_string())
                    .trim()
                    .to_string()
            })
            .unwrap_or_default();
        if snippet.is_empty() {
            lines.push(format!("- {}\n  {}", title, href));
        } else {
            lines.push(format!("- {}\n  {}\n  {}", title, snippet, href));
        }
    }

    if lines.is_empty() {
        // Either Bing returned a captcha/empty layout, or the regex needs to
        // be widened. Surface a hint so the agent can decide what to do.
        ToolResult::ok(format!(
            "No Bing search results parsed for {:?}. The response may be a captcha or a layout change. Try ASI_BING_HOST=bing.com or ASI_SEARCH_BACKEND=duckduckgo.",
            query
        ))
    } else {
        ToolResult::ok(lines.join("\n"))
    }
}

/// DuckDuckGo HTML scraper (legacy default; opt-in via ASI_SEARCH_BACKEND).
fn web_search_ddg(query: &str) -> ToolResult {
    let encoded = simple_url_encode(query);
    let url = format!("https://duckduckgo.com/html/?q={}", encoded);
    let client = match Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ToolResult::err(format!("http client build: {}", e)),
    };
    let body = match client
        .get(url)
        .header("User-Agent", WEB_SEARCH_USER_AGENT)
        .send()
    {
        Ok(resp) => match resp.text() {
            Ok(v) => v,
            Err(e) => return ToolResult::err(e.to_string()),
        },
        Err(e) => return ToolResult::err(e.to_string()),
    };

    let link_re =
        match Regex::new(r#"<a[^>]*class=\"result__a\"[^>]*href=\"([^\"]+)\"[^>]*>(.*?)</a>"#) {
            Ok(v) => v,
            Err(e) => return ToolResult::err(e.to_string()),
        };

    let tag_re = Regex::new(r"<[^>]+>").unwrap_or_else(|_| Regex::new("$").expect("regex"));

    let mut lines = Vec::new();
    for cap in link_re.captures_iter(&body).take(8) {
        let href = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let title_html = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let title = tag_re.replace_all(title_html, "").to_string();
        lines.push(format!("- {}\n  {}", title, href));
    }

    if lines.is_empty() {
        ToolResult::ok("No search results parsed")
    } else {
        ToolResult::ok(lines.join("\n"))
    }
}

/// Decode the small set of HTML entities that show up most often in search
/// result titles and snippets. Anything else is left as-is.
fn decode_html_entities_minimal(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

pub fn web_fetch(url: &str) -> ToolResult {
    if url.is_empty() {
        return ToolResult::err("url is empty");
    }
    let text = match Client::new().get(url).send() {
        Ok(resp) => match resp.text() {
            Ok(v) => v,
            Err(e) => return ToolResult::err(e.to_string()),
        },
        Err(e) => return ToolResult::err(e.to_string()),
    };

    let mut out = text;
    if out.len() > 12000 {
        out.truncate(12000);
    }
    ToolResult::ok(out)
}

fn simple_url_encode(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(ch),
            ' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", ch as u32)),
        }
    }
    out
}

pub fn bash(command: &str) -> ToolResult {
    let timeout_secs = std::env::var("ASI_BASH_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(|v| v.clamp(1, 3600))
        .unwrap_or(900);
    let stream_output = std::env::var("ASI_BASH_STREAM_OUTPUT")
        .ok()
        .map(|v| {
            let s = v.trim().to_ascii_lowercase();
            matches!(s.as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or_else(|| std::io::stderr().is_terminal());
    let max_capture_lines = std::env::var("ASI_BASH_CAPTURE_MAX_LINES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map(|v| v.clamp(100, 50000))
        .unwrap_or(4000);

    let shell_command = if cfg!(target_os = "windows") {
        normalize_windows_shell_command(command)
    } else {
        command.to_string()
    };
    let exec_shell_command = if cfg!(target_os = "windows") {
        with_windows_utf8_preamble(&shell_command)
    } else {
        shell_command.clone()
    };

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = Command::new("powershell");
        c.arg("-NoProfile").arg("-Command").arg(&exec_shell_command);
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-lc").arg(&exec_shell_command);
        c
    };

    if cfg!(target_os = "windows") {
        if let Some(cache_dir) = ensure_windows_npm_cache_dir() {
            cmd.env("npm_config_cache", cache_dir);
        }
    }

    let spawn_res = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
    let mut child = match spawn_res {
        Ok(c) => c,
        Err(e) => return ToolResult::err(e.to_string()),
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (tx, rx) = mpsc::channel::<String>();

    let out_tx = tx.clone();
    let out_handle = thread::spawn(move || {
        if let Some(out) = stdout {
            let reader = BufReader::new(out);
            for line in reader.lines().map_while(Result::ok) {
                let _ = out_tx.send(line);
            }
        }
    });

    let err_tx = tx.clone();
    let err_handle = thread::spawn(move || {
        if let Some(err) = stderr {
            let reader = BufReader::new(err);
            for line in reader.lines().map_while(Result::ok) {
                let _ = err_tx.send(line);
            }
        }
    });

    drop(tx);

    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let mut next_heartbeat = Duration::from_secs(10);
    let mut status_code = -1;
    let mut timed_out = false;
    let mut lines = Vec::new();
    let mut truncated_lines = 0usize;

    loop {
        while let Ok(line) = rx.try_recv() {
            if stream_output {
                eprintln!("BASH | {}", line);
            }
            if lines.len() < max_capture_lines {
                lines.push(line);
            } else {
                truncated_lines += 1;
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                status_code = status.code().unwrap_or(-1);
                break;
            }
            Ok(None) => {
                let elapsed = start.elapsed();
                if stream_output && elapsed >= next_heartbeat {
                    eprintln!(
                        "INFO bash still running ({}s elapsed): {}",
                        elapsed.as_secs(),
                        shell_command
                    );
                    next_heartbeat += Duration::from_secs(10);
                }
                if elapsed >= timeout {
                    timed_out = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                thread::sleep(Duration::from_millis(120));
            }
            Err(e) => return ToolResult::err(e.to_string()),
        }
    }

    let _ = out_handle.join();
    let _ = err_handle.join();

    while let Ok(line) = rx.try_recv() {
        if stream_output {
            eprintln!("BASH | {}", line);
        }
        if lines.len() < max_capture_lines {
            lines.push(line);
        } else {
            truncated_lines += 1;
        }
    }
    let mut text = lines.join("\n");
    if truncated_lines > 0 {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!(
            "[bash-output-truncated] dropped {} line(s); set ASI_BASH_CAPTURE_MAX_LINES to keep more output.",
            truncated_lines
        ));
    }

    if timed_out {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!(
            "[bash-timeout] command exceeded {}s; set ASI_BASH_TIMEOUT_SECS=1800 to allow longer runs.",
            timeout_secs
        ));
        return ToolResult {
            ok: false,
            output: text,
        };
    }

    if cfg!(target_os = "windows") && status_code != 0 {
        let auto_fix_guarded = std::env::var_os(YARN_AUTOFIX_GUARD_ENV).is_some();
        if !auto_fix_guarded
            && auto_install_yarn_enabled()
            && should_auto_recover_yarn(&shell_command, &text)
        {
            std::env::set_var(YARN_AUTOFIX_GUARD_ENV, "1");
            let global_install_result = bash("npm.cmd install -g yarn");
            let mut install_ok = global_install_result.ok;
            let mut merged = String::new();
            append_block(&mut merged, "[auto-fix] original command output", &text);
            append_block(
                &mut merged,
                "[auto-fix] running: npm.cmd install -g yarn",
                &global_install_result.output,
            );

            if !install_ok && windows_permission_error(&global_install_result.output) {
                let local_install_result = bash("npm.cmd install --save-dev yarn");
                append_block(
                    &mut merged,
                    "[auto-fix] fallback: npm.cmd install --save-dev yarn",
                    &local_install_result.output,
                );
                install_ok = local_install_result.ok;
            }

            let retry_result = if install_ok {
                Some(bash(&shell_command))
            } else {
                None
            };
            std::env::remove_var(YARN_AUTOFIX_GUARD_ENV);

            if let Some(retry) = retry_result {
                append_block(
                    &mut merged,
                    "[auto-fix] retrying original command",
                    &retry.output,
                );
                if !retry.ok {
                    if let Some(hint) = windows_shell_error_hint(&retry.output) {
                        append_block(&mut merged, "[auto-fix] hint", hint);
                    }
                }
                return ToolResult {
                    ok: retry.ok,
                    output: merged,
                };
            }

            if let Some(hint) = windows_shell_error_hint(&text) {
                append_block(&mut merged, "[auto-fix] hint", hint);
            }
            return ToolResult {
                ok: false,
                output: merged,
            };
        }

        let python_alias_fix_guarded = std::env::var_os(PYTHON_ALIAS_AUTOFIX_GUARD_ENV).is_some();
        if !python_alias_fix_guarded && should_auto_recover_python_alias(&shell_command, &text) {
            std::env::set_var(PYTHON_ALIAS_AUTOFIX_GUARD_ENV, "1");
            let mut merged = String::new();
            append_block(&mut merged, "[auto-fix] original command output", &text);

            let mut attempts: Vec<String> = Vec::new();
            if let Some(cmd) = rewrite_python_command_to_python3(&shell_command) {
                attempts.push(cmd);
            }
            if let Some(cmd) = rewrite_python_command_to_py_launcher(&shell_command) {
                if !attempts.iter().any(|x| x.eq_ignore_ascii_case(&cmd)) {
                    attempts.push(cmd);
                }
            }
            if let Some(cmd) = rewrite_python_command_to_venv_python(&shell_command) {
                if !attempts.iter().any(|x| x.eq_ignore_ascii_case(&cmd)) {
                    attempts.push(cmd);
                }
            }
            if let Some(cmd) = rewrite_python3_command_to_venv_python(&shell_command) {
                if !attempts.iter().any(|x| x.eq_ignore_ascii_case(&cmd)) {
                    attempts.push(cmd);
                }
            }

            for retry_command in attempts {
                let retry = bash(&retry_command);
                append_block(
                    &mut merged,
                    &format!("[auto-fix] retrying with python fallback: {}", retry_command),
                    &retry.output,
                );
                if retry.ok {
                    std::env::remove_var(PYTHON_ALIAS_AUTOFIX_GUARD_ENV);
                    return ToolResult {
                        ok: true,
                        output: merged,
                    };
                }
                if !windows_python_alias_error(&retry.output) {
                    std::env::remove_var(PYTHON_ALIAS_AUTOFIX_GUARD_ENV);
                    if let Some(hint) = windows_shell_error_hint(&retry.output) {
                        append_block(&mut merged, "[auto-fix] hint", hint);
                    }
                    return ToolResult {
                        ok: retry.ok,
                        output: merged,
                    };
                }
            }
            std::env::remove_var(PYTHON_ALIAS_AUTOFIX_GUARD_ENV);

            if let Some(hint) = windows_shell_error_hint(&text) {
                append_block(&mut merged, "[auto-fix] hint", hint);
            }
            return ToolResult {
                ok: false,
                output: merged,
            };
        }

        let cache_dir_fix_guarded = std::env::var_os(CACHE_DIR_AUTOFIX_GUARD_ENV).is_some();
        if !cache_dir_fix_guarded {
            if let Some(cache_path) = should_auto_recover_windows_cache_dir(&shell_command, &text) {
                std::env::set_var(CACHE_DIR_AUTOFIX_GUARD_ENV, "1");
                let repair_command = build_windows_cache_dir_repair_command(&cache_path);
                let repair = bash(&repair_command);
                let retry = if repair.ok {
                    Some(bash(&shell_command))
                } else {
                    None
                };
                std::env::remove_var(CACHE_DIR_AUTOFIX_GUARD_ENV);

                let mut merged = String::new();
                append_block(&mut merged, "[auto-fix] original command output", &text);
                append_block(
                    &mut merged,
                    "[auto-fix] repairing broken cache directory",
                    &repair.output,
                );
                if let Some(retry_result) = retry {
                    append_block(
                        &mut merged,
                        "[auto-fix] retrying original command",
                        &retry_result.output,
                    );
                    if !retry_result.ok {
                        if let Some(hint) = windows_shell_error_hint(&retry_result.output) {
                            append_block(&mut merged, "[auto-fix] hint", hint);
                        }
                    }
                    return ToolResult {
                        ok: retry_result.ok,
                        output: merged,
                    };
                }
                if let Some(hint) = windows_shell_error_hint(&text) {
                    append_block(&mut merged, "[auto-fix] hint", hint);
                }
                return ToolResult {
                    ok: false,
                    output: merged,
                };
            }
        }

        if let Some(hint) = windows_shell_error_hint(&text) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(hint);
        }
    }

    if text.is_empty() {
        text = format!("exit={}", status_code);
    }

    ToolResult {
        ok: status_code == 0,
        output: text,
    }
}
pub fn tool_index() -> String {
    [
        "- read_file <path> [start_line] [max_lines]",
        "- write_file <path> <content>",
        "- edit_file <path> <old> <new>",
        "- glob_search <pattern>",
        "- grep_search <regex> [base_path]",
        "- web_search <query>",
        "- web_fetch <url>",
        "- bash <command>",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        build_windows_cache_dir_repair_command, edit_file, is_autoresearch_cache_path,
        grep_search,
        decode_key_value_text_value,
        normalize_bash_args, normalize_grep_search_args, normalize_windows_shell_command,
        extract_key_value_string_arg,
        rewrite_python_dash_c_single_quoted_script, rewrite_python_herestring_for_powershell,
        rewrite_bash_group_pipe_python_for_powershell,
        run_tool,
        parse_delimited_edit,
        parse_delimited_write, rewrite_python3_command_to_venv_python,
        read_file_range,
        rewrite_python_command_to_py_launcher, rewrite_python_command_to_python3,
        rewrite_python_command_to_venv_python, should_auto_recover_python_alias,
        should_auto_recover_windows_cache_dir, should_auto_recover_yarn,
        windows_connect_timeout_error, windows_file_exists_path, windows_missing_path_error_path,
        windows_missing_yarn_error, windows_permission_error, extract_json_string_arg,
        windows_python_alias_error, windows_shell_error_hint,
    };
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parse_delimited_write_supports_eof_marker() {
        let args = "\"a.md\" <<<EOF\nline1\nline2\nEOF\n";
        let (path, content) = parse_delimited_write(args);
        assert_eq!(path, "a.md");
        assert_eq!(content, "line1\nline2");
    }

    #[test]
    fn parse_delimited_write_supports_content_end_marker() {
        let args = "\"a.md\" <<<CONTENT\nline1\nline2\n<<<END\n";
        let (path, content) = parse_delimited_write(args);
        assert_eq!(path, "a.md");
        assert_eq!(content, "line1\nline2");
    }

    #[test]
    fn parse_delimited_write_legacy_fallback_strips_wrapping_quotes() {
        // Legacy positional fallback now decodes common backslash escapes
        // (\t, \n, ...) so models that emit `write_file <path> "<body>"`
        // with embedded escapes produce real whitespace on disk.
        let args = "\"results.tsv\" \"a\\tb\\tc\"";
        let (path, content) = parse_delimited_write(args);
        assert_eq!(path, "results.tsv");
        assert_eq!(content, "a\tb\tc");
    }

    #[test]
    fn parse_delimited_write_legacy_fallback_decodes_newlines() {
        // Regression: when a model emits the shape
        //   write_file solution.py "def f():\n    return 1"
        // the positional fallback used to strip the outer quotes but leave
        // `\n` as the literal two characters `\` `n`, producing a single-
        // line file that fails py_compile. After the fix it produces a real
        // newline.
        let args = "\"solution.py\" \"def f():\\n    return 1\\n\"";
        let (path, content) = parse_delimited_write(args);
        assert_eq!(path, "solution.py");
        assert_eq!(content, "def f():\n    return 1\n");
    }

    #[test]
    fn parse_delimited_edit_supports_old_new_end_blocks() {
        let args = "\"a.md\" <<<OLD\nold text\n<<<NEW\nnew text\n<<<END\n";
        let (path, old_text, new_text) = parse_delimited_edit(args);
        assert_eq!(path, "a.md");
        assert_eq!(old_text, "old text");
        assert_eq!(new_text, "new text");
    }

    #[test]
    fn parse_delimited_edit_strips_quotes_in_legacy_fallback() {
        let args = "\"requirements.txt\" \"old line\" \"new line\"";
        let (path, old_text, new_text) = parse_delimited_edit(args);
        assert_eq!(path, "requirements.txt");
        assert_eq!(old_text, "old line");
        assert_eq!(new_text, "new line");
    }

    #[test]
    fn normalize_bash_args_strips_wrapping_quotes() {
        assert_eq!(normalize_bash_args("\"pwd 2>&1\""), "pwd 2>&1");
        assert_eq!(normalize_bash_args("'pwd 2>&1'"), "pwd 2>&1");
        assert_eq!(normalize_bash_args("pwd 2>&1"), "pwd 2>&1");
    }

    #[test]
    fn normalize_bash_args_unescapes_json_style_windows_snippets() {
        let raw = r#"cd \"D:\\test_code\" ; python -c \"import sys; print('ok')\""#;
        let got = normalize_bash_args(raw);
        assert_eq!(got, "cd \"D:\\test_code\" ; python -c \"import sys; print('ok')\"");
    }

    #[test]
    fn normalize_bash_args_keeps_inner_quotes_and_trailing_redirect_after_outer_quote() {
        let raw =
            "\"cd D:\\test_code\\autoresearch ; python -c \\\"import os; print(os.getcwd())\\\"\" 2>&1";
        let got = normalize_bash_args(raw);
        assert_eq!(
            got,
            "cd D:\\test_code\\autoresearch ; python -c \"import os; print(os.getcwd())\" 2>&1"
        );
    }

    #[test]
    fn normalize_bash_args_keeps_single_quoted_wrapper_with_trailing_redirect() {
        let raw =
            "'cd D:\\test_code\\autoresearch ; python -c \"import sys; print(sys.version)\"' 2>&1";
        let got = normalize_bash_args(raw);
        assert_eq!(
            got,
            "cd D:\\test_code\\autoresearch ; python -c \"import sys; print(sys.version)\" 2>&1"
        );
    }

    #[test]
    fn extract_json_string_arg_reads_expected_key() {
        let raw = r#"{"command":"dir","other":1}"#;
        let got = extract_json_string_arg(raw, &["command"]).expect("command key");
        assert_eq!(got, "dir");
        assert!(extract_json_string_arg(raw, &["missing"]).is_none());
    }

    #[test]
    fn extract_key_value_string_arg_supports_quoted_payloads() {
        let raw = r#"file_path="D:\test-cli\demo.txt" content="line1\nline2""#;
        let path = extract_key_value_string_arg(raw, &["file_path"]).expect("path");
        let content = extract_key_value_string_arg(raw, &["content"]).expect("content");
        assert_eq!(path, r"D:\test-cli\demo.txt");
        assert_eq!(content, r"line1\nline2");
    }

    #[test]
    fn decode_key_value_text_value_decodes_common_escapes() {
        let got = decode_key_value_text_value(r#"line1\nline2\tok\\done"#);
        assert_eq!(got, "line1\nline2\tok\\done");
    }

    #[test]
    fn run_tool_write_file_supports_file_path_key_value_shape() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!("asi_kv_write_{}.txt", ts));
        let path_str = path.to_string_lossy().replace('\\', "\\\\");
        let args = format!(r#"file_path="{}" content="hello\nworld""#, path_str);

        let result = run_tool("write_file", &args);
        assert!(result.ok, "{}", result.output);
        let text = fs::read_to_string(&path).expect("read written file");
        assert_eq!(text, "hello\nworld");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn run_tool_bash_supports_command_key_value_shape() {
        let result = run_tool("bash", r#"command="Write-Output KV_OK""#);
        assert!(result.ok, "{}", result.output);
        assert!(result.output.contains("KV_OK"), "{}", result.output);
    }

    #[test]
    fn run_tool_edit_file_supports_key_value_shape() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!("asi_kv_edit_{}.txt", ts));
        fs::write(&path, "alpha\nbeta\n").expect("write fixture");

        let path_str = path.to_string_lossy().replace('\\', "\\\\");
        let args = format!(
            r#"path="{}" old_text="alpha" new_text="gamma""#,
            path_str
        );
        let result = run_tool("edit_file", &args);
        assert!(result.ok, "{}", result.output);
        let text = fs::read_to_string(&path).expect("read edited file");
        assert!(text.contains("gamma"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn run_tool_write_file_named_shape_with_unescaped_inner_quotes_does_not_fall_back_to_path_literal() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!("asi_kv_write_lossy_{}.py", ts));
        let path_str = path.to_string_lossy().replace('\\', "\\\\");
        let args = format!(
            r#"path="{}" content="print("hello")
name='asi'
""#,
            path_str
        );

        let result = run_tool("write_file", &args);
        assert!(result.ok, "{}", result.output);
        let text = fs::read_to_string(&path).expect("read written file");
        assert!(text.contains("print(\"hello\")"), "{}", text);
        assert!(text.contains("name='asi'"), "{}", text);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn normalize_grep_search_args_supports_json_shape() {
        let raw = r#"{"regex":"TODO","base_path":"src"}"#;
        let got = normalize_grep_search_args(raw);
        assert_eq!(got, "TODO src");
    }

    #[test]
    fn read_file_range_honors_explicit_range_for_medium_file() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!("asi_read_range_{}.txt", ts));

        let mut content = String::new();
        for i in 1..=600 {
            content.push_str(&format!("line {}\n", i));
        }
        fs::write(&path, content).unwrap();

        let got = read_file_range(path.to_str().unwrap(), 1, 10);
        assert!(got.ok);
        assert!(got.output.contains("lines 1-10 of 600"));
        assert!(!got.output.contains("line 600"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn grep_search_supports_single_file_base_path() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let mut dir = std::env::temp_dir();
        dir.push(format!("asi_grep_file_{}", ts));
        fs::create_dir_all(&dir).unwrap();

        let mut file: PathBuf = dir.clone();
        file.push("demo.txt");
        fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();

        let got = grep_search("beta", file.to_str().unwrap());
        assert!(got.ok);
        assert!(got.output.contains("demo.txt:2:beta"));

        let _ = fs::remove_file(file);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn normalize_windows_shell_command_rewrites_common_unix_style() {
        let cmd =
            r#"cd D:\repo && ls -la && curl -s http://localhost:3000 | head -20 && npm run dev"#;
        let got = normalize_windows_shell_command(cmd);
        assert!(got.contains("cd D:\\repo ; Get-ChildItem -Force ;"));
        assert!(got.contains("curl.exe -s http://localhost:3000 | Select-Object -First 20"));
        assert!(got.contains("npm.cmd run dev"));
    }

    #[test]
    fn normalize_windows_shell_command_rewrites_or_tail_and_if_redirection() {
        let cmd =
            "if (Test-Path ~/.cache/autoresearch/data) { echo 'ok' } else { echo 'missing' } 2>&1 || echo 'fallback' | tail -20";
        let got = normalize_windows_shell_command(cmd);
        assert!(!got.contains("||"));
        assert!(!got.contains("} 2>&1"));
        assert!(got.contains("; echo 'fallback' | Select-Object -Last 20"));
    }

    #[test]
    fn normalize_windows_shell_command_rewrites_plain_tail_pipe() {
        let cmd = "Get-Content log.txt | tail";
        let got = normalize_windows_shell_command(cmd);
        assert!(got.contains("| Select-Object -Last 10"));
    }

    #[test]
    fn normalize_windows_shell_command_strips_redundant_stderr_redirect() {
        let cmd = "git status 2>&1 ; echo done 2>&1";
        let got = normalize_windows_shell_command(cmd);
        assert!(!got.contains("2>&1"));
        assert!(got.contains("git status"));
        assert!(got.contains("echo done"));
    }

    #[test]
    fn normalize_windows_shell_command_rewrites_echo_dash_e_with_tabs() {
        let cmd = r#"echo -e 'run_tag\tval_bpb\ttimestamp' > results.tsv"#;
        let got = normalize_windows_shell_command(cmd);
        assert!(got.contains("Write-Output"));
        assert!(!got.contains("echo -e"));
        assert!(got.contains("run_tag\tval_bpb\ttimestamp"));
        assert!(got.contains("> results.tsv"));
    }

    #[test]
    fn normalize_windows_shell_command_rewrites_mkdir_dash_p() {
        let cmd = "cd D:\\repo ; mkdir -p ~/.cache/autoresearch/data ; python prepare.py";
        let got = normalize_windows_shell_command(cmd);
        assert!(!got.contains("mkdir -p"));
        assert!(
            got.contains(
                "New-Item -ItemType Directory -Force -Path ~/.cache/autoresearch/data | Out-Null"
            )
        );
    }

    #[test]
    fn normalize_windows_shell_command_rewrites_mkdir_dash_p_multiple_paths() {
        let cmd = "mkdir -p templates static";
        let got = normalize_windows_shell_command(cmd);
        assert!(!got.contains("mkdir -p"));
        assert!(got.contains("New-Item -ItemType Directory -Force -Path templates, static | Out-Null"));
    }

    #[test]
    fn rewrite_python_herestring_for_powershell_converts_bash_syntax() {
        let cmd = "python main.py <<< 'q'";
        let got = rewrite_python_herestring_for_powershell(cmd);
        assert_eq!(got, "'q' | python main.py");
    }

    #[test]
    fn rewrite_python_dash_c_single_quoted_script_unescapes_inner_double_quotes() {
        let cmd = r#"python -c 'from main import add; print(\"ok\")'"#;
        let got = rewrite_python_dash_c_single_quoted_script(cmd);
        assert_eq!(got, r#"python -c "from main import add; print(`"ok`")""#);
    }

    #[test]
    fn normalize_windows_shell_command_rewrites_python_herestring_and_dash_c_quote_form() {
        let cmd = r#"python -c 'print(\"ok\")' && python main.py <<< 'q'"#;
        let got = normalize_windows_shell_command(cmd);
        assert!(got.contains(r#"python -c "print(`"ok`")""#));
        assert!(got.contains("; 'q' | python main.py"));
        assert!(!got.contains("<<<"));
        assert!(!got.contains("&&"));
    }

    #[test]
    fn rewrite_bash_group_pipe_python_for_powershell_converts_echo_group() {
        let cmd = "{ echo +; echo 10; echo 5; echo q; } | python3 main.py";
        let got = rewrite_bash_group_pipe_python_for_powershell(cmd);
        assert_eq!(got, "@('+', '10', '5', 'q') | python3 main.py");
    }

    #[test]
    fn normalize_windows_shell_command_rewrites_bash_group_pipe_python_pattern() {
        let cmd = "{ echo +; echo 10; echo 5; echo q; } | python3 main.py 2>&1";
        let got = normalize_windows_shell_command(cmd);
        assert_eq!(got, "@('+', '10', '5', 'q') | python3 main.py");
    }

    #[test]
    fn normalize_windows_shell_command_rewrites_grep_pipe_simple() {
        let cmd = "pip list 2>&1 | grep -E '(torch|numpy)' 2>&1";
        let got = normalize_windows_shell_command(cmd);
        assert!(!got.contains("grep -E"));
        assert!(got.contains("| Select-String -Pattern '(torch|numpy)'"));
    }

    #[test]
    fn windows_missing_yarn_error_detects_known_signatures() {
        let msg = "yarn : The term 'yarn' is not recognized as the name of a cmdlet. FullyQualifiedErrorId : CommandNotFoundException";
        assert!(windows_missing_yarn_error(msg));
    }

    #[test]
    fn should_auto_recover_yarn_requires_relevant_command() {
        let msg = "'yarn' is not recognized as an internal or external command";
        assert!(should_auto_recover_yarn("npm run dev", msg));
        assert!(should_auto_recover_yarn("npm.cmd run dev", msg));
        assert!(should_auto_recover_yarn("yarn dev", msg));
        assert!(!should_auto_recover_yarn("yarn --version", msg));
        assert!(!should_auto_recover_yarn("python app.py", msg));
    }

    #[test]
    fn windows_permission_error_detects_eperm_and_denied() {
        assert!(windows_permission_error("npm ERR! code EPERM"));
        assert!(windows_permission_error("Access is denied"));
        assert!(!windows_permission_error("network timeout"));
    }
    #[test]
    fn windows_shell_error_hint_detects_missing_yarn() {
        let msg = "yarn : The term 'yarn' is not recognized as the name of a cmdlet. FullyQualifiedErrorId : CommandNotFoundException";
        let hint = windows_shell_error_hint(msg);
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("npm.cmd install -g yarn"));
    }

    #[test]
    fn windows_shell_error_hint_returns_none_for_other_errors() {
        let msg = "npm ERR! code E401";
        assert!(windows_shell_error_hint(msg).is_none());
    }

    #[test]
    fn windows_connect_timeout_error_detects_signatures() {
        assert!(windows_connect_timeout_error("UND_ERR_CONNECT_TIMEOUT"));
        assert!(windows_connect_timeout_error(
            "ConnectTimeoutError: Connect Timeout Error"
        ));
        assert!(windows_connect_timeout_error(
            "TypeError: fetch failed ... timeout"
        ));
        assert!(!windows_connect_timeout_error("npm ERR! code E401"));
    }

    #[test]
    fn windows_shell_error_hint_detects_network_timeout() {
        let msg =
            "TypeError: fetch failed; ConnectTimeoutError: Connect Timeout Error; code: UND_ERR_CONNECT_TIMEOUT";
        let hint = windows_shell_error_hint(msg);
        assert!(hint.is_some());
        assert!(hint.unwrap().contains("network/proxy/firewall"));
    }

    #[test]
    fn windows_python_alias_error_detects_known_signatures() {
        let msg = "OSError: [Errno 22] Invalid argument: 'C:\\Users\\u\\AppData\\Local\\Microsoft\\WindowsApps\\PythonSoftwareFoundation.Python.3.13_qbz5n2kfra8p0\\python.exe'";
        assert!(windows_python_alias_error(msg));
        assert!(windows_python_alias_error(
            "Unable to create process using '...python3.13.exe' --version"
        ));
        assert!(!windows_python_alias_error("python: can't open file"));
    }

    #[test]
    fn should_auto_recover_python_alias_requires_plain_python_command() {
        let msg = "Unable to create process using '...python3.13.exe' --version";
        assert!(should_auto_recover_python_alias("python train.py", msg));
        assert!(should_auto_recover_python_alias("cd D:\\repo ; python -m pip list", msg));
        assert!(should_auto_recover_python_alias("python3 train.py", msg));
        assert!(!should_auto_recover_python_alias(
            ".\\.venv\\Scripts\\python.exe train.py",
            msg
        ));
        assert!(!should_auto_recover_python_alias("Get-ChildItem", msg));
    }

    #[test]
    fn rewrite_python_command_to_python3_rewrites_whole_word_only() {
        let got = rewrite_python_command_to_python3("python train.py").expect("expected rewrite");
        assert_eq!(got, "python3 train.py");
        let got = rewrite_python_command_to_python3("cd D:\\repo ; python -m pip list")
            .expect("expected rewrite");
        assert_eq!(got, "cd D:\\repo ; python3 -m pip list");
        assert!(rewrite_python_command_to_python3("python3 train.py").is_none());
    }

    #[test]
    fn rewrite_python_fallback_commands_cover_py_and_venv() {
        let py = rewrite_python_command_to_py_launcher("python train.py").expect("py fallback");
        assert_eq!(py, "py -3 train.py");

        let venv = rewrite_python_command_to_venv_python("python train.py").expect("venv fallback");
        assert_eq!(venv, ".\\.venv\\Scripts\\python.exe train.py");

        let venv_from_python3 =
            rewrite_python3_command_to_venv_python("python3 train.py").expect("venv fallback");
        assert_eq!(venv_from_python3, ".\\.venv\\Scripts\\python.exe train.py");
    }

    #[test]
    fn windows_file_exists_path_extracts_path() {
        let msg = "FileExistsError: [WinError 183] Cannot create a file when that file already exists.: 'C:\\\\Users\\\\me\\\\.cache\\\\autoresearch\\\\data'";
        let got = windows_file_exists_path(msg).expect("expected cache path");
        assert_eq!(got, r"C:\\Users\\me\\.cache\\autoresearch\\data");
    }

    #[test]
    fn windows_missing_path_error_path_extracts_path() {
        let msg = "copy : Could not find a part of the path 'C:\\\\Users\\\\me\\\\.cache\\\\autoresearch\\\\data\\\\shard_00000.parquet'.";
        let got = windows_missing_path_error_path(msg).expect("expected missing path");
        assert_eq!(
            got,
            r"C:\\Users\\me\\.cache\\autoresearch\\data\\shard_00000.parquet"
        );
    }

    #[test]
    fn is_autoresearch_cache_path_matches_expected_variants() {
        assert!(is_autoresearch_cache_path(
            r"C:\\Users\\me\\.cache\\autoresearch\\data"
        ));
        assert!(is_autoresearch_cache_path(
            "C:/Users/me/.cache/autoresearch/tokenizer"
        ));
        assert!(!is_autoresearch_cache_path(r"D:\\repo\\data"));
    }

    #[test]
    fn should_auto_recover_windows_cache_dir_returns_path_for_prepare_py() {
        let cmd = "python prepare.py";
        let out = "FileExistsError: [WinError 183] ...: 'C:\\\\Users\\\\me\\\\.cache\\\\autoresearch\\\\data'";
        let got = should_auto_recover_windows_cache_dir(cmd, out).expect("expected recovery");
        assert_eq!(got, r"C:\\Users\\me\\.cache\\autoresearch\\data");
    }

    #[test]
    fn should_auto_recover_windows_cache_dir_for_copy_missing_target_parent() {
        let cmd = "copy base_data\\*.parquet ~/.cache/autoresearch/data\\";
        let out = "copy : Could not find a part of the path 'C:\\\\Users\\\\me\\\\.cache\\\\autoresearch\\\\data\\\\shard_00000.parquet'.";
        let got = should_auto_recover_windows_cache_dir(cmd, out).expect("expected recovery");
        assert_eq!(got, r"C:\\Users\\me\\.cache\\autoresearch\\data");
    }

    #[test]
    fn build_windows_cache_dir_repair_command_embeds_literal_path() {
        let path = r"C:\\Users\\me\\.cache\\autoresearch\\data";
        let cmd = build_windows_cache_dir_repair_command(path);
        assert!(cmd.contains("$p='C:\\\\Users\\\\me\\\\.cache\\\\autoresearch\\\\data'"));
        assert!(cmd.contains("New-Item -ItemType Directory -Force -Path $p"));
    }

    #[test]
    fn edit_file_allows_lf_old_against_crlf_file() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock issue")
            .as_nanos();
        let path = env::temp_dir().join(format!("asi_tools_edit_{}.txt", nonce));
        fs::write(&path, "a\r\nb\r\nc\r\n").expect("write fixture failed");

        let res = edit_file(path.to_string_lossy().as_ref(), "b\nc", "x\ny");
        assert!(res.ok, "{}", res.output);

        let updated = fs::read_to_string(&path).expect("read updated failed");
        assert!(updated.contains("a\r\nx\r\ny\r\n"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn edit_file_not_found_message_mentions_newlines() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock issue")
            .as_nanos();
        let path = env::temp_dir().join(format!("asi_tools_edit_missing_{}.txt", nonce));
        fs::write(&path, "hello\nworld\n").expect("write fixture failed");

        let res = edit_file(path.to_string_lossy().as_ref(), "not-there", "x");
        assert!(!res.ok);
        assert!(res.output.contains("whitespace/newlines"));

        let _ = fs::remove_file(path);
    }
}
