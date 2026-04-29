use serde_json::Value;

pub fn parse_relaxed_json_value(input: &str) -> Option<Value> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut candidates: Vec<String> = Vec::new();
    push_relaxed_json_candidate(&mut candidates, trimmed);

    if let Some(stripped) = trimmed
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .map(str::trim)
    {
        push_relaxed_json_candidate(&mut candidates, stripped);
    }

    let tick_trimmed = trimmed.trim_matches('`').trim();
    if tick_trimmed != trimmed {
        push_relaxed_json_candidate(&mut candidates, tick_trimmed);
    }

    if let Some(fenced_body) = strip_markdown_fence_body(trimmed) {
        push_relaxed_json_candidate(&mut candidates, &fenced_body);
    }

    for candidate in candidates {
        if let Some(value) = parse_json_value_with_nested_string(&candidate) {
            return Some(value);
        }
        if let Some(decoded) = decode_shell_escaped_json(&candidate) {
            if let Some(value) = parse_json_value_with_nested_string(&decoded) {
                return Some(value);
            }
        }
    }

    None
}

pub fn collect_json_tool_candidates<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_json_tool_candidates(item, out);
            }
        }
        Value::Object(obj) => {
            if obj
                .get("function")
                .and_then(Value::as_object)
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
                .is_some()
                || obj.get("name").and_then(Value::as_str).is_some()
            {
                out.push(value);
            }
            if let Some(tool_calls) = obj.get("tool_calls") {
                collect_json_tool_candidates(tool_calls, out);
            }
        }
        _ => {}
    }
}

pub fn extract_json_tool_name(candidate: &Value) -> Option<String> {
    let obj = candidate.as_object()?;
    if let Some(name) = obj
        .get("function")
        .and_then(Value::as_object)
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
    {
        return Some(name.to_string());
    }
    obj.get("name")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

pub fn first_json_tool_candidate<'a>(value: &'a Value) -> Option<&'a Value> {
    let mut candidates = Vec::new();
    collect_json_tool_candidates(value, &mut candidates);
    candidates.into_iter().next()
}

pub fn normalize_json_string_argument(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if serde_json::from_str::<Value>(trimmed).is_ok() {
        return trimmed.to_string();
    }
    if (trimmed.starts_with('{') || trimmed.starts_with('[')) && trimmed.contains("\\\"") {
        let decoded = trimmed.replace("\\\"", "\"");
        if serde_json::from_str::<Value>(&decoded).is_ok() {
            return decoded;
        }
    }
    trimmed.to_string()
}

fn push_relaxed_json_candidate(candidates: &mut Vec<String>, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }
    if candidates.iter().any(|x| x == trimmed) {
        return;
    }
    candidates.push(trimmed.to_string());
}

fn strip_markdown_fence_body(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if !trimmed.starts_with("```") {
        return None;
    }
    let mut lines = trimmed.lines();
    let first = lines.next()?;
    if !first.trim_start().starts_with("```") {
        return None;
    }
    let mut body: Vec<&str> = lines.collect();
    if body.last().is_some_and(|line| line.trim() == "```") {
        body.pop();
    }
    let joined = body.join("\n");
    let joined = joined.trim();
    if joined.is_empty() {
        None
    } else {
        Some(joined.to_string())
    }
}

fn parse_json_value_with_nested_string(input: &str) -> Option<Value> {
    let mut value: Value = serde_json::from_str(input).ok()?;
    for _ in 0..2 {
        let Some(next) = value
            .as_str()
            .map(str::trim)
            .filter(|inner| inner.starts_with('{') || inner.starts_with('['))
            .and_then(|inner| serde_json::from_str::<Value>(inner).ok())
        else {
            break;
        };
        value = next;
    }
    Some(value)
}

fn decode_shell_escaped_json(input: &str) -> Option<String> {
    if !input.contains("\\\"") {
        return None;
    }
    // Preserve inner JSON escapes (\\") first, then unescape outer shell-level (\").
    const MARKER: &str = "\u{001f}ASI_ESC_QUOTE_SHARED\u{001f}";
    let protected = input.replace("\\\\\"", MARKER);
    let decoded = protected.replace("\\\"", "\"");
    let restored = decoded.replace(MARKER, "\\\\\"");
    if restored == input {
        None
    } else {
        Some(restored)
    }
}
