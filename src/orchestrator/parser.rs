use std::collections::HashSet;

use serde_json::Value;

use crate::provider::tool_call_to_legacy_args;

use super::types::ToolCallRequest;

#[allow(dead_code)]
pub fn parse_toolcall_line(line: &str) -> Option<ToolCallRequest> {
    let trimmed = line.trim();
    let idx = trimmed.find("/toolcall ")?;
    let raw = trimmed[idx..].trim().trim_matches('`').trim();
    parse_toolcall_text(raw)
}

pub fn extract_tool_calls(text: &str) -> Vec<ToolCallRequest> {
    let mut normalized_toolcall_lines: Vec<(String, usize)> = Vec::new();
    let mut previous_tool_end_line: Option<usize> = None;

    let pseudo_xml_normalized = normalize_pseudo_xml_toolcall_markers(text);
    let inline_calling_normalized = normalize_inline_angle_calling_blocks(&pseudo_xml_normalized);
    let calling_normalized = normalize_calling_markdown_blocks(&inline_calling_normalized);
    let dsml_normalized = normalize_dsml_fenced_tags(&calling_normalized);
    let normalized = strip_markdown_fences(&dsml_normalized);
    let with_tool_argument_pairs = normalize_tool_argument_pairs(&normalized);
    let preexpanded = normalize_tag_style_toolcalls(&with_tool_argument_pairs);
    let lines: Vec<&str> = preexpanded.lines().collect();
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i].trim();
        let Some(idx) = line.find("/toolcall ") else {
            i += 1;
            continue;
        };

        let mut raw = line[idx..].trim().trim_matches('`').trim().to_string();
        let mut consumed = 0usize;

        if let Some(rest) = raw.strip_prefix("/toolcall ") {
            if let Some(name) = parse_tool_name_for_capture(rest) {
                if needs_multiline_capture(&name, &raw)
                    || (starts_with_named_arg_fragment(rest) && !is_double_quote_balanced(&raw))
                {
                    let terminator = heredoc_terminator(&name, &raw);

                    while i + consumed + 1 < lines.len() {
                        let next = lines[i + consumed + 1];
                        raw.push('\n');
                        raw.push_str(next);
                        consumed += 1;

                        if let Some(term) = &terminator {
                            let t = next.trim();
                            if t == "<<<END" || t == term {
                                break;
                            }
                        } else if is_double_quote_balanced(&raw) {
                            break;
                        }
                    }
                } else if is_known_tool_name(&name)
                    && rest
                        .split_once(char::is_whitespace)
                        .map(|(_, after)| after.trim())
                        .unwrap_or("")
                        .is_empty()
                {
                    // `/toolcall TOOL` with the args on the immediately following
                    // line(s). Some models — DeepSeek v4 Pro in particular —
                    // emit shell commands this way when the command is long
                    // enough to feel "block-like". Without this branch, the
                    // tool name dispatches with empty args and the audit log
                    // records "empty shell command; ambiguous intent".
                    while i + consumed + 1 < lines.len() {
                        let next = lines[i + consumed + 1];
                        let nt = next.trim();
                        if nt.is_empty() {
                            consumed += 1;
                            continue;
                        }
                        if nt.starts_with("/toolcall ")
                            || nt.starts_with('<')
                            || nt.starts_with("```")
                        {
                            break;
                        }
                        raw.push(' ');
                        raw.push_str(nt);
                        consumed += 1;
                        break;
                    }
                }
            }
        }

        let end_line = i + consumed;
        let rest = raw
            .strip_prefix("/toolcall ")
            .map(str::trim)
            .unwrap_or_default();

        let should_merge_named_fragment = starts_with_named_arg_fragment(rest)
            && previous_tool_end_line
                .map(|prev_end| prev_end + 1 == i)
                .unwrap_or(false);

        if should_merge_named_fragment {
            if let Some((prev_raw, prev_end)) = normalized_toolcall_lines.last_mut() {
                if can_absorb_named_fragment(prev_raw) {
                    let prev_ends_with_whitespace = prev_raw
                        .chars()
                        .last()
                        .map(|c| c.is_whitespace())
                        .unwrap_or(false);
                    if !prev_ends_with_whitespace {
                        prev_raw.push(' ');
                    }
                    prev_raw.push_str(rest);
                    *prev_end = end_line;
                    previous_tool_end_line = Some(end_line);
                    i += consumed + 1;
                    continue;
                }
            }
        }

        normalized_toolcall_lines.push((raw, end_line));
        previous_tool_end_line = Some(end_line);
        i += consumed + 1;
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (raw, _) in normalized_toolcall_lines {
        for call in parse_toolcall_text_many(&raw) {
            if seen.insert(call.raw.clone()) {
                out.push(call);
            }
        }
    }

    out
}

/// Recognize the "**Calling:** `tool` \n ```{json}``` " markdown shape that
/// DeepSeek v4 Pro (and a few other reasoning-trained models) emit instead of
/// using the API's native `tool_calls` field. We rewrite each such block into
/// a canonical `/toolcall TOOL {json}` line so the rest of the pipeline picks
/// it up. Without this, the runtime sees `completion.tool_calls` empty, the
/// agent loop terminates after one turn, and the user never gets a result —
/// only a wall of "**Calling:** ..." text.
/// Recognize the pseudo-XML toolcall markers DeepSeek v4 Pro emits when it
/// thinks `/toolcall` is an XML-style element. Patterns observed:
///
///     < /toolcall >
///     glob_search "**/*.html"
///     < /toolcall >
///     read_file "hello.py" 1 50
///     < /toolcall >
///
/// Markers with a space-before-slash (`< /toolcall >`) are unambiguously
/// pseudo-XML separators and are always dropped. No-space markers
/// (`<toolcall>` / `</toolcall>`) may also be real wrapper open/close
/// lines, so we only convert when the next non-empty line looks like a
/// bare tool-call (starts with a known tool name and not with `<`).
fn normalize_pseudo_xml_toolcall_markers(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0usize;

    while i < lines.len() {
        let line = lines[i];

        if is_space_slash_pseudo_toolcall_marker(line) {
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }
            if let Some((promoted, advance_to)) = promote_bare_tool_call_line(&lines, i, j) {
                out.extend(promoted);
                i = advance_to;
            } else {
                i += 1;
            }
            continue;
        }

        if is_no_space_pseudo_toolcall_marker(line) {
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }
            if j < lines.len() && !lines[j].trim().starts_with('<') {
                if let Some((promoted, advance_to)) =
                    promote_bare_tool_call_line(&lines, i, j)
                {
                    out.extend(promoted);
                    i = advance_to;
                    continue;
                }
            }
            out.push(line.to_string());
            i += 1;
            continue;
        }

        out.push(line.to_string());
        i += 1;
    }

    out.join("\n")
}

fn promote_bare_tool_call_line(
    lines: &[&str],
    i: usize,
    j: usize,
) -> Option<(Vec<String>, usize)> {
    if j >= lines.len() {
        return None;
    }
    let next = lines[j].trim();
    if next.is_empty() || next.starts_with('<') {
        return None;
    }
    let first_token = next.split_whitespace().next()?;
    let normalized = normalize_tag_tool_name(&first_token.to_ascii_lowercase());
    if !is_known_tool_name(&normalized) {
        return None;
    }

    let mut emitted: Vec<String> = Vec::new();
    for k in (i + 1)..j {
        emitted.push(lines[k].to_string());
    }
    let rest = next.strip_prefix(first_token).unwrap_or("").trim_start();
    if rest.is_empty() {
        emitted.push(format!("/toolcall {}", normalized));
    } else {
        emitted.push(format!("/toolcall {} {}", normalized, rest));
    }

    // Don't consume trailing markers; in the alternating
    // marker/content pattern the closing marker is also the next opener.
    Some((emitted, j + 1))
}

fn is_space_slash_pseudo_toolcall_marker(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "</toolcall >"
            | "< /toolcall>"
            | "< /toolcall >"
            | "< toolcall >"
            | "</tool_call >"
            | "< /tool_call>"
            | "< /tool_call >"
            | "< tool_call >"
    )
}

fn is_no_space_pseudo_toolcall_marker(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "</toolcall>" | "<toolcall>" | "</tool_call>" | "<tool_call>"
    )
}

/// Recognize the inline angle-bracketed Calling block emitted by DeepSeek v4 Pro:
///
///     <**Calling:** `glob_search` `{ "pattern": "**/*" }`>
///     </**Calling**>
///
/// The opening line is a single `<...>` block containing a bolded
/// `**Calling:**` marker, then the tool name in backticks, then either no
/// args or a single backtick-wrapped JSON / quoted-args string. The closing
/// line `</**Calling**>` is pure noise. We rewrite the opener to a
/// canonical `/toolcall TOOL ARGS` line and drop the closer.
fn normalize_inline_angle_calling_blocks(text: &str) -> String {
    let mut out: Vec<String> = Vec::with_capacity(text.lines().count());
    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        // Drop the closing marker line entirely.
        if matches!(
            lower.as_str(),
            "</**calling**>" | "</**calling:**>" | "</calling>" | "</calling:>"
        ) {
            continue;
        }
        if let Some((tool, args)) = parse_inline_angle_calling_line(trimmed) {
            if args.is_empty() {
                out.push(format!("/toolcall {}", tool));
            } else {
                out.push(format!("/toolcall {} {}", tool, args));
            }
            continue;
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

fn parse_inline_angle_calling_line(line: &str) -> Option<(String, String)> {
    if line.len() < 4 || !line.starts_with('<') || !line.ends_with('>') {
        return None;
    }
    let inner = line[1..line.len() - 1].trim();
    let lower = inner.to_ascii_lowercase();

    // Find a "calling:" or "calling" marker; everything before it must be
    // bold-markdown markers or whitespace, so we don't accidentally match
    // arbitrary `<some text>` lines.
    let calling_pos = lower.find("calling:").or_else(|| lower.find("calling "))?;
    let prefix = &inner[..calling_pos];
    if !prefix.chars().all(|c| c == '*' || c.is_whitespace()) {
        return None;
    }
    let calling_tok_len = if lower[calling_pos..].starts_with("calling:") {
        "calling:".len()
    } else {
        "calling ".len()
    };
    let after = inner[calling_pos + calling_tok_len..]
        .trim_start_matches('*')
        .trim();
    let after = after.trim_start_matches('*').trim();

    // First backtick group = tool name.
    let after = after.strip_prefix('`')?;
    let tool_end = after.find('`')?;
    let tool_name = after[..tool_end].trim();
    let after_tool = after[tool_end + 1..].trim();
    let after_tool = after_tool.trim_end_matches('*').trim_end();

    let normalized = normalize_tag_tool_name(&tool_name.to_ascii_lowercase());
    if !is_known_tool_name(&normalized) {
        return None;
    }

    if after_tool.is_empty() {
        return Some((normalized, String::new()));
    }

    // Optional second backtick group = args.
    let args_block = after_tool.strip_prefix('`')?;
    let args_end = args_block.rfind('`')?;
    let args = args_block[..args_end].trim();

    Some((normalized, args.to_string()))
}

fn normalize_calling_markdown_blocks(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        if let Some(tool) = parse_calling_marker_line(line) {
            // Look ahead for an optional fenced code block carrying the args.
            // Skip blank lines between the marker and the fence.
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }
            if j < lines.len() {
                let fence = lines[j].trim();
                if fence.starts_with("```") {
                    // Collect body until closing ``` .
                    let mut body: Vec<&str> = Vec::new();
                    let mut k = j + 1;
                    let mut closed = false;
                    while k < lines.len() {
                        if lines[k].trim() == "```" {
                            closed = true;
                            break;
                        }
                        body.push(lines[k]);
                        k += 1;
                    }
                    if closed {
                        let joined = body.join("\n").trim().to_string();
                        if joined.is_empty() {
                            out.push(format!("/toolcall {}", tool));
                        } else {
                            out.push(format!("/toolcall {} {}", tool, joined));
                        }
                        i = k + 1;
                        continue;
                    }
                }
            }
            // No fence found — treat the marker line as a bare call.
            out.push(format!("/toolcall {}", tool));
            i += 1;
            continue;
        }
        out.push(line.to_string());
        i += 1;
    }
    out.join("\n")
}

/// Match the marker line of a "Calling:" block. Returns the tool name if the
/// line is one of the known shapes:
///   `**Calling:** \`tool\``        (DeepSeek v4 Pro, with bold)
///   `Calling: tool`                 (plain prose form)
///   `Calling \`tool\``              (no colon)
/// We deliberately reject any tool name that isn't in our canonical list so
/// arbitrary prose containing the word "Calling" doesn't get misparsed.
fn parse_calling_marker_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    // Strip optional surrounding markdown bold/italic so `**Calling:**` and
    // `*Calling:*` both work.
    let stripped = trimmed
        .trim_start_matches('*')
        .trim_end_matches('*')
        .trim();
    let lower = stripped.to_ascii_lowercase();
    let after = if let Some(rest) = lower.strip_prefix("calling:") {
        &stripped[stripped.len() - rest.len()..]
    } else if let Some(rest) = lower.strip_prefix("calling tool:") {
        &stripped[stripped.len() - rest.len()..]
    } else if let Some(rest) = lower.strip_prefix("calling ") {
        &stripped[stripped.len() - rest.len() - 1..]
    } else {
        return None;
    };
    let after = after.trim().trim_start_matches('*').trim();
    // Pull the tool name: it is either the first backtick-quoted token or
    // the first whitespace-delimited token.
    let candidate = if let Some(rest) = after.strip_prefix('`') {
        let end = rest.find('`')?;
        rest[..end].trim().to_string()
    } else {
        after
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches('`')
            .to_string()
    };
    if candidate.is_empty() {
        return None;
    }
    let normalized = normalize_tag_tool_name(&candidate.to_ascii_lowercase());
    if is_known_tool_name(&normalized) {
        Some(normalized)
    } else {
        None
    }
}

fn normalize_tool_argument_pairs(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i].trim();
        let Some(tool_name) = parse_tool_header_line(line) else {
            out.push(lines[i].to_string());
            i += 1;
            continue;
        };

        let mut args: Vec<String> = Vec::new();
        let mut consumed = 0usize;
        while i + consumed + 1 < lines.len() {
            let next = lines[i + consumed + 1].trim();
            if next.is_empty() {
                consumed += 1;
                continue;
            }
            if parse_tool_header_line(next).is_some() {
                break;
            }
            if let Some(arg) = parse_arguments_line(next) {
                if !arg.is_empty() {
                    args.push(arg);
                }
                consumed += 1;
                continue;
            }
            break;
        }

        if args.is_empty() {
            out.push(lines[i].to_string());
            i += 1;
            continue;
        }

        let merged = if args.len() == 1 {
            args.remove(0)
        } else {
            args.join(" ")
        };
        if merged.is_empty() {
            out.push(format!("/toolcall {}", tool_name));
        } else {
            out.push(format!("/toolcall {} {}", tool_name, merged));
        }
        i += consumed + 1;
    }
    out.join("\n")
}

fn parse_tool_header_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("tool:") {
        return None;
    }
    let name = trimmed[5..].trim();
    if name.is_empty() {
        return None;
    }
    let normalized = normalize_tag_tool_name(&name.to_ascii_lowercase());
    if is_known_tool_name(&normalized) {
        Some(normalized)
    } else {
        None
    }
}

fn parse_arguments_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("arguments:") {
        return None;
    }
    let raw = trimmed[10..].trim();
    if raw.is_empty() {
        return Some(String::new());
    }
    let value = crate::json_toolcall::parse_relaxed_json_value(raw)?;
    let obj = value.as_object()?;
    let mut parts = Vec::new();
    for (k, v) in obj {
        match v {
            Value::String(s) => {
                let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
                parts.push(format!(r#"{}="{}""#, k, escaped));
            }
            _ => {
                let escaped = v.to_string().replace('\\', "\\\\").replace('"', "\\\"");
                parts.push(format!(r#"{}="{}""#, k, escaped));
            }
        }
    }
    Some(parts.join(" "))
}

/// DeepSeek-v4-Pro and a few other reasoning models occasionally emit a
/// pseudo-XML wire format using the fullwidth pipe `｜` and a `DSML`
/// literal as their tag delimiters, e.g.
///   `<｜DSML｜tool_calls>`
///   `<｜DSML｜invoke name="glob_search">`
///   `<｜DSML｜parameter name="pattern" string="true">**/*.py</｜DSML｜parameter>`
///
/// Strip the `｜DSML｜` and the trailing-fence variant (`</｜DSML｜...>`)
/// so the line becomes ordinary `<tool_calls>` / `<invoke>` /
/// `<parameter ...>` markup. Then alias `<tool_calls>` to the
/// `<function_calls>` wrapper the existing pipeline already knows about.
fn normalize_dsml_fenced_tags(text: &str) -> String {
    if !text.contains('\u{FF5C}') && !text.contains("<tool_calls") && !text.contains("</tool_calls") {
        return text.to_string();
    }
    let opener = "\u{FF5C}DSML\u{FF5C}";
    let mut out = text.replace(opener, "");
    // After stripping the sentinel, alias the outer `tool_calls` wrapper
    // (an underscore variant DeepSeek uses) to `function_calls` so the
    // existing block parser fires.
    out = out
        .replace("<tool_calls>", "<function_calls>")
        .replace("</tool_calls>", "</function_calls>");
    out
}

fn strip_markdown_fences(text: &str) -> String {
    // Promote fenced shell blocks to `/toolcall bash <body>` invocations and
    // promote fenced file-write blocks (with a `# path/to/file` header) to
    // `/toolcall write_file <path> <body>`. Other fence languages are simply
    // stripped so any inline `/toolcall` lines inside still get parsed.
    let promoted = promote_fenced_blocks_to_toolcalls(text);
    promoted
        .replace("```bash", "\n")
        .replace("```shell", "\n")
        .replace("```sh", "\n")
        .replace("```powershell", "\n")
        .replace("```ps1", "\n")
        .replace("```", "\n")
}

/// Convert markdown fenced shell blocks into `/toolcall bash` heredoc
/// invocations and recognized file-write fenced blocks (whose first line is
/// a `# path/to/file` header) into `/toolcall write_file` invocations.
/// Returns the rewritten text. Unrelated fences are left untouched here and
/// stripped by `strip_markdown_fences`.
///
/// We skip any fence that appears inside an open XML quoted attribute value
/// (e.g. `<write_file content="```bash ... ```" />`) because in that case
/// the fence is just literal text inside an argument, not an actionable
/// shell block.
fn promote_fenced_blocks_to_toolcalls(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0usize;
    let mut inside_quoted_attr = false;
    while i < lines.len() {
        let line = lines[i];

        if inside_quoted_attr {
            // Update quote state for this line and emit verbatim.
            inside_quoted_attr = update_quoted_attr_state(inside_quoted_attr, line);
            out.push(line.to_string());
            i += 1;
            continue;
        }

        let trimmed = line.trim_start();
        if let Some(lang) = parse_fence_open_lang(trimmed) {
            // Find the matching closing fence (a line whose trimmed start is
            // exactly "```").
            let mut consumed = 0usize;
            let mut found_close = false;
            let mut body: Vec<&str> = Vec::new();
            while i + consumed + 1 < lines.len() {
                let next = lines[i + consumed + 1];
                consumed += 1;
                if next.trim_start().starts_with("```") {
                    found_close = true;
                    break;
                }
                body.push(next);
            }
            if found_close {
                if let Some(toolcall) = fenced_block_to_toolcall(&lang, &body) {
                    out.push(toolcall);
                    i += consumed + 1;
                    continue;
                }
            }
            out.push(line.to_string());
            i += 1;
            continue;
        }

        // Track whether this line opens a quoted attribute value that
        // continues across newlines (e.g. `<write_file content="...`).
        inside_quoted_attr = update_quoted_attr_state(inside_quoted_attr, line);

        out.push(line.to_string());
        i += 1;
    }
    out.join("\n")
}

/// Returns true if, after walking `line`, the parser is inside an unclosed
/// double-quoted attribute value. Tracks the previous state so the loop
/// can carry the flag across line boundaries.
fn update_quoted_attr_state(prev_inside: bool, line: &str) -> bool {
    let mut inside = prev_inside;
    let mut escape = false;
    for ch in line.chars() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if ch == '"' {
            inside = !inside;
        }
    }
    inside
}

fn parse_fence_open_lang(trimmed_start: &str) -> Option<String> {
    if !trimmed_start.starts_with("```") {
        return None;
    }
    let after = trimmed_start.trim_start_matches('`').trim();
    if after.is_empty() {
        return None;
    }
    // Take the first whitespace-delimited token as the language hint.
    Some(
        after
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase(),
    )
}

fn fenced_block_to_toolcall(lang: &str, body: &[&str]) -> Option<String> {
    let body_text = body.join("\n");
    let trimmed_body = body_text.trim_matches(['\r', '\n']);
    if trimmed_body.is_empty() {
        return None;
    }

    // If the body itself already contains explicit `/toolcall ...` lines we
    // must not wrap it; the fence is just framing for human readability.
    if trimmed_body
        .lines()
        .any(|l| l.trim_start().starts_with("/toolcall "))
    {
        return None;
    }

    match lang {
        "bash" | "shell" | "sh" | "powershell" | "ps1" | "pwsh" | "cmd" | "console" => {
            if !looks_like_shell_command(trimmed_body) {
                return None;
            }
            // Use the named `command="..."` shape so the executor extracts a
            // single argument even when the body spans multiple lines. Escape
            // embedded double quotes and backslashes.
            let escaped = trimmed_body
                .replace('\\', "\\\\")
                .replace('"', "\\\"");
            Some(format!("/toolcall bash command=\"{}\"", escaped))
        }
        "python" | "py" | "javascript" | "js" | "typescript" | "ts" | "rust" | "rs"
        | "json" | "yaml" | "yml" | "toml" | "go" | "java" | "ruby" | "rb" | "html" | "css"
        | "markdown" | "md" | "text" | "txt" => {
            let mut iter = trimmed_body.lines();
            let first = iter.next()?.trim();
            let path = parse_file_header(first)?;
            let rest: String = iter.collect::<Vec<_>>().join("\n");
            if rest.trim().is_empty() {
                return None;
            }
            Some(format!(
                "/toolcall write_file \"{}\" <<<CONTENT\n{}\n<<<END",
                path, rest
            ))
        }
        _ => None,
    }
}

fn looks_like_shell_command(body: &str) -> bool {
    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        // Any non-empty non-comment line counts as a shell command for our
        // purposes; the executor will reject malformed input anyway.
        return true;
    }
    false
}

fn parse_file_header(line: &str) -> Option<String> {
    // Accept `# path`, `// path`, `-- path`, `<!-- path -->` style headers.
    let t = line.trim();
    let stripped = if let Some(rest) = t.strip_prefix("#") {
        rest.trim()
    } else if let Some(rest) = t.strip_prefix("//") {
        rest.trim()
    } else if let Some(rest) = t.strip_prefix("--") {
        rest.trim()
    } else if let Some(rest) = t.strip_prefix("<!--") {
        rest.trim_end_matches("-->").trim()
    } else {
        return None;
    };
    if stripped.is_empty() {
        return None;
    }
    // Only accept if it looks like a path (contains a slash/backslash and
    // looks like a filename).
    if !(stripped.contains('/') || stripped.contains('\\')) {
        return None;
    }
    if stripped.contains(' ') {
        return None;
    }
    Some(stripped.to_string())
}

fn normalize_tag_style_toolcalls(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if is_function_calls_open(trimmed) {
            let mut block_lines: Vec<&str> = Vec::new();
            let mut consumed = 0usize;
            let mut found_close = false;
            while i + consumed + 1 < lines.len() {
                let next = lines[i + consumed + 1];
                consumed += 1;
                if is_function_calls_close(next.trim()) {
                    found_close = true;
                    break;
                }
                block_lines.push(next);
            }
            if found_close {
                let parsed = parse_function_calls_block(&block_lines);
                for (name, args) in parsed {
                    if args.is_empty() {
                        out.push(format!("/toolcall {}", name));
                    } else {
                        out.push(format!("/toolcall {} {}", name, args));
                    }
                }
                i += consumed + 1;
                continue;
            }
        }

        if is_tool_call_wrapper_open(trimmed) {
            let mut body_lines: Vec<&str> = Vec::new();
            let mut consumed = 0usize;
            let mut found_close = false;

            while i + consumed + 1 < lines.len() {
                let next = lines[i + consumed + 1];
                consumed += 1;
                if is_tool_call_wrapper_close(next.trim()) {
                    found_close = true;
                    break;
                }
                body_lines.push(next);
            }

            if found_close {
                if let Some((name, args)) = parse_tool_call_wrapper(&body_lines, Some(trimmed)) {
                    if args.is_empty() {
                        out.push(format!("/toolcall {}", name));
                    } else if args.contains('\n') && needs_multiline_capture(&name, &args) {
                        // Inline the first line with the /toolcall prefix and
                        // keep the heredoc body on subsequent lines so the
                        // line-based extractor in extract_tool_calls can
                        // pick up the heredoc opener after the path.
                        let mut iter = args.lines();
                        let head = iter.next().unwrap_or("");
                        let second = iter.next().unwrap_or("");
                        // If the next line begins a heredoc, glue it onto the
                        // same /toolcall line so the heredoc opener is on the
                        // anchor line; this satisfies needs_multiline_capture.
                        if second.trim_start().starts_with("<<<") {
                            out.push(format!("/toolcall {} {} {}", name, head, second));
                            for tail in iter {
                                out.push(tail.to_string());
                            }
                        } else {
                            out.push(format!("/toolcall {} {}", name, head));
                            out.push(second.to_string());
                            for tail in iter {
                                out.push(tail.to_string());
                            }
                        }
                    } else {
                        out.push(format!("/toolcall {} {}", name, args));
                    }
                    i += consumed + 1;
                    continue;
                }
            }
        }

        // Unknown wrapper tag whose body is a sequence of named param tags
        // (e.g. `<file-creation-attempt><path>...</path><content>...</content></file-creation-attempt>`).
        // Reasoning-trained models occasionally invent such semantic wrappers
        // around what is structurally a tool call. We accept them as long as
        // the inner params unambiguously identify a canonical tool via
        // `infer_tool_name_from_legacy_args`.
        if let Some(unknown_name) = parse_unknown_wrapper_open_tag(trimmed) {
            let close_tag = format!("</{}>", unknown_name);
            let mut body_lines: Vec<&str> = Vec::new();
            let mut consumed = 0usize;
            let mut found_close = false;
            while i + consumed + 1 < lines.len() {
                let next = lines[i + consumed + 1];
                consumed += 1;
                if next.trim().eq_ignore_ascii_case(&close_tag) {
                    found_close = true;
                    break;
                }
                body_lines.push(next);
            }
            if found_close && body_starts_with_named_param_tag(&body_lines) {
                if let Some((parsed_name, parsed_args)) =
                    parse_tool_call_wrapper(&body_lines, None)
                {
                    if is_known_tool_name(&parsed_name) {
                        if parsed_args.is_empty() {
                            out.push(format!("/toolcall {}", parsed_name));
                        } else {
                            out.push(format!("/toolcall {} {}", parsed_name, parsed_args));
                        }
                        i += consumed + 1;
                        continue;
                    }
                }
            }
        }

        if let Some(open) = parse_open_tag(trimmed) {
            if open.self_closing {
                if open.attrs.is_empty() {
                    out.push(format!("/toolcall {}", open.name));
                } else {
                    out.push(format!("/toolcall {} {}", open.name, open.attrs));
                }
                i += 1;
                continue;
            }

            let mut body_lines: Vec<&str> = Vec::new();
            let mut consumed = 0usize;
            let close_tag_primary = format!("</{}>", open.close_name);
            let close_tag_alt = format!("</{}>", open.name);
            let mut found_close = false;

            while i + consumed + 1 < lines.len() {
                let next = lines[i + consumed + 1];
                consumed += 1;
                let next_trimmed = next.trim();
                if next_trimmed.eq_ignore_ascii_case(&close_tag_primary)
                    || next_trimmed.eq_ignore_ascii_case(&close_tag_alt)
                {
                    found_close = true;
                    break;
                }
                body_lines.push(next);
            }

            if found_close {
                // If the body looks like a sequence of named param tags
                // (e.g. <path>...</path>, <content>...</content>), parse it
                // through the same wrapper logic so each child becomes a
                // canonical `key="value"` argument instead of a positional
                // heredoc body. This is the form most reasoning-trained
                // models default to when they hallucinate XML tool wire
                // formats. We always prefer this path over the heredoc fall-
                // back when the children look named, even if the open tag
                // already carries (typically meaningless) attributes like
                // `tool_name="bash"`.
                if body_starts_with_named_param_tag(&body_lines) {
                    if let Some((parsed_name, parsed_args)) =
                        parse_tool_call_wrapper(&body_lines, None)
                            .or_else(|| Some((open.name.clone(), {
                                let coalesced = coalesce_param_blocks(&body_lines);
                                let mut out_args: Vec<String> = Vec::new();
                                for line in coalesced {
                                    if let Some(arg) = extract_parameter_as_legacy_arg(line.trim())
                                        .or_else(|| extract_named_attr_tag_as_legacy_arg(line.trim()))
                                    {
                                        out_args.push(arg);
                                    }
                                }
                                out_args.join(" ")
                            })))
                    {
                        let final_name = if is_known_tool_name(&parsed_name) {
                            parsed_name
                        } else {
                            open.name.clone()
                        };
                        if parsed_args.is_empty() {
                            out.push(format!("/toolcall {}", final_name));
                        } else {
                            out.push(format!("/toolcall {} {}", final_name, parsed_args));
                        }
                        i += consumed + 1;
                        continue;
                    }
                }

                let mut args = open.attrs;
                let body_text = body_lines.join("\n");
                if !body_text.trim().is_empty() {
                    let wrapped = if body_text.trim_start().starts_with("<<<") {
                        body_text.trim_matches('\n').replace('\r', "")
                    } else {
                        wrap_body_as_heredoc(&body_text)
                    };
                    if args.is_empty() {
                        args = wrapped;
                    } else {
                        args = format!("{} {}", args, wrapped);
                    }
                }
                if args.is_empty() {
                    out.push(format!("/toolcall {}", open.name));
                } else {
                    out.push(format!("/toolcall {} {}", open.name, args));
                }
                i += consumed + 1;
                continue;
            }
        }

        // Multi-line `<TOOL attr="..." attr="..." />` shape some providers
        // emit when an attribute value (such as `content="..."`) spans many
        // lines. Stitch lines until quotes balance and a `>` appears, then
        // re-parse the joined string.
        if let Some((joined_line, lines_consumed)) =
            coalesce_multiline_open_tag(&lines, i)
        {
            if let Some(open) = parse_open_tag(joined_line.trim()) {
                if open.self_closing {
                    if open.attrs.is_empty() {
                        out.push(format!("/toolcall {}", open.name));
                    } else {
                        out.push(format!("/toolcall {} {}", open.name, open.attrs));
                    }
                    i += lines_consumed;
                    continue;
                }

                let mut body_lines: Vec<&str> = Vec::new();
                let mut consumed = 0usize;
                let close_tag_primary = format!("</{}>", open.close_name);
                let close_tag_alt = format!("</{}>", open.name);
                let mut found_close = false;

                while i + lines_consumed - 1 + consumed + 1 < lines.len() {
                    let next = lines[i + lines_consumed - 1 + consumed + 1];
                    consumed += 1;
                    let next_trimmed = next.trim();
                    if next_trimmed.eq_ignore_ascii_case(&close_tag_primary)
                        || next_trimmed.eq_ignore_ascii_case(&close_tag_alt)
                    {
                        found_close = true;
                        break;
                    }
                    body_lines.push(next);
                }

                if found_close {
                    let mut args = open.attrs;
                    let body_text = body_lines.join("\n");
                    if !body_text.trim().is_empty() {
                        let wrapped = if body_text.trim_start().starts_with("<<<") {
                            body_text.trim_matches('\n').replace('\r', "")
                        } else {
                            wrap_body_as_heredoc(&body_text)
                        };
                        if args.is_empty() {
                            args = wrapped;
                        } else {
                            args = format!("{} {}", args, wrapped);
                        }
                    }
                    if args.is_empty() {
                        out.push(format!("/toolcall {}", open.name));
                    } else {
                        out.push(format!("/toolcall {} {}", open.name, args));
                    }
                    i += lines_consumed + consumed;
                    continue;
                }

                // No matching close tag: emit as self-closing-style.
                if open.attrs.is_empty() {
                    out.push(format!("/toolcall {}", open.name));
                } else {
                    out.push(format!("/toolcall {} {}", open.name, open.attrs));
                }
                i += lines_consumed;
                continue;
            }
        }

        out.push(lines[i].to_string());
        i += 1;
    }

    out.join("\n")
}

/// Detect a tool open tag whose `>` is on a later line because an attribute
/// value contains newlines. Returns the joined line (with `\n` replaced by
/// literal whitespace within quoted values preserved as `\n`) and how many
/// source lines it consumed. Only triggers if the first line starts with
/// `<KNOWN_TOOL_NAME` and the `>` cannot be located while quotes balance.
fn coalesce_multiline_open_tag(lines: &[&str], start: usize) -> Option<(String, usize)> {
    let first = lines.get(start)?;
    let trimmed = first.trim_start();
    if !trimmed.starts_with('<') || trimmed.starts_with("</") {
        return None;
    }
    let after_lt = &trimmed[1..];
    let name_end = after_lt
        .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
        .unwrap_or(after_lt.len());
    let raw_name = &after_lt[..name_end];
    let name_lower = raw_name.to_ascii_lowercase();
    let normalized = normalize_tag_tool_name(&name_lower);
    if !is_known_tool_name(&normalized) {
        return None;
    }

    // If `>` already appears on the first line with balanced quotes, the
    // existing single-line parser handles it.
    if has_unquoted_close_bracket(first) {
        return None;
    }

    let mut joined = String::from(*first);
    let mut consumed = 1usize;
    while start + consumed < lines.len() {
        joined.push('\n');
        joined.push_str(lines[start + consumed]);
        consumed += 1;
        if has_unquoted_close_bracket(&joined) {
            return Some((joined, consumed));
        }
        // Bound the lookahead so we never scan the whole document.
        if consumed > 4096 {
            return None;
        }
    }
    None
}

/// True if `s` contains a `>` that is not inside double or single quotes,
/// taking backslash escapes into account.
fn has_unquoted_close_bracket(s: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut escape = false;
    for ch in s.chars() {
        if escape {
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }
        if ch == '>' {
            return true;
        }
    }
    false
}

struct OpenTagCall {
    name: String,
    close_name: String,
    attrs: String,
    self_closing: bool,
}

fn parse_open_tag(line: &str) -> Option<OpenTagCall> {
    let trimmed = line.trim();
    if !trimmed.starts_with('<') || !trimmed.ends_with('>') || trimmed.starts_with("</") {
        return None;
    }
    if trimmed.len() < 3 {
        return None;
    }

    let self_closing = trimmed.ends_with("/>");
    let inner = if self_closing {
        &trimmed[1..trimmed.len() - 2]
    } else {
        &trimmed[1..trimmed.len() - 1]
    };
    let mut parts = inner.splitn(2, char::is_whitespace);
    let raw_name = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let name = normalize_tag_tool_name(&raw_name);
    if !is_known_tool_name(&name) {
        return None;
    }
    let attrs_raw = parts.next().unwrap_or("").trim();
    let attrs = normalize_xml_attrs(attrs_raw)?;
    Some(OpenTagCall {
        name,
        close_name: raw_name,
        attrs,
        self_closing,
    })
}

fn normalize_tag_tool_name(name: &str) -> String {
    match name {
        "file_write" => "write_file".to_string(),
        "file_edit" => "edit_file".to_string(),
        other => other.to_string(),
    }
}

fn is_tool_call_wrapper_open(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower == "<tool_call>"
        || lower.starts_with("<tool_call ")
        || lower == "<toolcall>"
        || lower.starts_with("<toolcall ")
}

/// Recognize a no-attribute open tag whose name is NOT a canonical tool name
/// and is NOT one of our reserved structural wrappers, so the caller can
/// attempt body-driven tool inference. We deliberately reject anything with
/// attributes or self-closing forms so we don't compete with `parse_open_tag`.
fn parse_unknown_wrapper_open_tag(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('<')
        || !trimmed.ends_with('>')
        || trimmed.starts_with("</")
        || trimmed.ends_with("/>")
        || trimmed.len() < 3
    {
        return None;
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    if inner.contains(char::is_whitespace) {
        return None;
    }
    let name = inner.to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    if is_known_tool_name(&normalize_tag_tool_name(&name)) {
        return None;
    }
    if matches!(
        name.as_str(),
        "tool_call"
            | "toolcall"
            | "function-calls"
            | "function_calls"
            | "function-call"
            | "function_call"
            | "invoke"
            | "parameter"
            | "tool_calls"
            | "tool_use"
            | "tool_uses"
    ) {
        return None;
    }
    Some(name)
}

fn is_tool_call_wrapper_close(line: &str) -> bool {
    line.eq_ignore_ascii_case("</tool_call>") || line.eq_ignore_ascii_case("</toolcall>")
}

fn is_function_calls_open(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower == "<function-calls>"
        || lower.starts_with("<function-calls ")
        || lower == "<function_calls>"
        || lower.starts_with("<function_calls ")
        // DeepSeek v4 Pro and other reasoning-trained models often emit the
        // Anthropic-style `<tool_calls> ... <tool_call name="X"> ...` shape
        // (with a plural-`s` outer wrapper). Treat it the same as
        // <function_calls> so the inner <tool_call name="..."> children get
        // parsed via parse_function_calls_block.
        || lower == "<tool_calls>"
        || lower.starts_with("<tool_calls ")
}

fn is_function_calls_close(line: &str) -> bool {
    line.eq_ignore_ascii_case("</function-calls>")
        || line.eq_ignore_ascii_case("</function_calls>")
        || line.eq_ignore_ascii_case("</tool_calls>")
}

fn parse_function_calls_block(lines: &[&str]) -> Vec<(String, String)> {
    let coalesced = coalesce_param_blocks(lines);
    let mut out = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current_args: Vec<String> = Vec::new();
    let mut in_call = false;

    for raw in coalesced.iter().map(String::as_str) {
        let line = raw.trim();
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("<function-call") || lower.starts_with("<function_call") {
            in_call = true;
            current_name = None;
            current_args.clear();
            continue;
        }
        if lower == "</function-call>" || lower == "</function_call>" {
            if let Some(name) = current_name.clone() {
                out.push((name, current_args.join(" ")));
            }
            in_call = false;
            current_name = None;
            current_args.clear();
            continue;
        }
        // Some providers emit <invoke> directly inside <function_calls> with no
        // <function-call> wrapper. Treat each <invoke>...</invoke> pair as a
        // complete call so those still get executed.
        if lower.starts_with("<invoke") && !in_call {
            in_call = true;
            current_name = None;
            current_args.clear();
        }
        if lower == "</invoke>" && in_call && current_name.is_some() {
            if let Some(name) = current_name.clone() {
                out.push((name, current_args.join(" ")));
            }
            in_call = false;
            current_name = None;
            current_args.clear();
            continue;
        }
        // Anthropic-style child shape: `<tool_call name="TOOL_NAME">` ...
        // `</tool_call>`. DeepSeek v4 Pro emits this whenever it tries to
        // imitate a structured tool format. Treat `<tool_call ...>` exactly
        // like `<invoke ...>` so the inner `<parameter name="...">` children
        // get harvested.
        if lower.starts_with("<tool_call") && !lower.starts_with("<tool_calls") && !in_call {
            in_call = true;
            current_name = None;
            current_args.clear();
            if let Some(name) = extract_attr_tag_value(line, "tool_call", "name") {
                let normalized = normalize_tag_tool_name(&name.to_ascii_lowercase());
                if is_known_tool_name(&normalized) {
                    current_name = Some(normalized);
                }
            }
            continue;
        }
        if lower == "</tool_call>" && in_call {
            if let Some(name) = current_name.clone() {
                out.push((name, current_args.join(" ")));
            }
            in_call = false;
            current_name = None;
            current_args.clear();
            continue;
        }
        if !in_call {
            // Self-closing tool-name-as-tag child shape:
            //   <tool_calls>
            //     <read_file path="..." start_line="1" max_lines="80"/>
            //     <glob_search pattern="*.py" base_path="..."/>
            //   </tool_calls>
            // DeepSeek v4 Pro emits this when it imitates "function calling
            // XML" without the `<tool_call name="...">` wrapper around each
            // child. parse_open_tag already validates that the tag name is
            // a known tool, so unrelated `<some_other_tag/>` lines are
            // ignored.
            if let Some(open) = parse_open_tag(line) {
                if open.self_closing {
                    out.push((open.name, open.attrs));
                    continue;
                }
            }
            continue;
        }

        if let Some(name) = extract_invoke_name(line) {
            let normalized = normalize_tag_tool_name(&name.to_ascii_lowercase());
            if is_known_tool_name(&normalized) {
                current_name = Some(normalized);
            }
            continue;
        }

        if let Some(arg) = extract_parameter_as_legacy_arg(line) {
            current_args.push(arg);
            continue;
        }
    }

    out
}

fn parse_tool_call_wrapper(
    body_lines: &[&str],
    wrapper_open_line: Option<&str>,
) -> Option<(String, String)> {
    let mut name: Option<String> = wrapper_open_line
        .and_then(extract_tool_call_wrapper_name)
        .map(|v| normalize_tag_tool_name(&v.to_ascii_lowercase()))
        .filter(|n| is_known_tool_name(n));
    let mut args: Vec<String> = Vec::new();

    // Some providers emit <toolcall> wrappers whose first child is a
    // pseudo-XML tag of the form <TOOL_NAME:VALUE> or <TOOL_NAME>VALUE
    // (with the rest of the body being a heredoc). Detect and rewrite that
    // shape into a normal positional invocation so the existing pipeline
    // handles it.
    if name.is_none() {
        if let Some((tool, head_arg, body_start)) = peel_pseudo_tool_first_child(body_lines) {
            let mut combined = String::new();
            if !head_arg.is_empty() {
                combined.push_str(&head_arg);
            }
            for line in &body_lines[body_start..] {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(line);
            }
            return Some((tool, combined.trim().to_string()));
        }
    }

    let coalesced = coalesce_param_blocks(body_lines);
    for raw in coalesced.iter().map(String::as_str) {
        let line = raw.trim();
        if let Some(value) = extract_inline_xml_tag_value(line, "tool_name")
            .or_else(|| extract_invoke_name(line))
        {
            let normalized = normalize_tag_tool_name(&value.to_ascii_lowercase());
            if is_known_tool_name(&normalized) {
                name = Some(normalized);
            }
            continue;
        }
        if let Some(value) = extract_parameter_as_legacy_arg(line)
            .or_else(|| extract_named_attr_tag_as_legacy_arg(line))
            .or_else(|| {
            extract_inline_xml_tag_value(line, "tool_arg")
            .or_else(|| extract_inline_xml_tag_value(line, "tool_args"))
            .or_else(|| extract_inline_xml_tag_value(line, "arguments"))
            .or_else(|| extract_parameter_value(line))
        }) {
            let val = value.trim().to_string();
            if !val.is_empty() {
                args.push(val);
            }
        }
    }

    let name = name
        .or_else(|| infer_tool_name_from_legacy_args(&args).map(str::to_string))?;
    let args = args.join(" ");
    Some((name, args))
}

/// Infer a canonical tool name from a set of already-extracted
/// `key="value"` legacy arg strings. Used when the outer wrapper tag is a
/// non-canonical name (e.g. `<file-creation-attempt>`, `<tool>`) but the
/// inner params unambiguously identify the intended tool. Order matters:
/// more specific patterns must be checked before less specific ones.
fn infer_tool_name_from_legacy_args(args: &[String]) -> Option<&'static str> {
    let has_key = |k: &str| -> bool {
        let needle_q = format!("{}=\"", k);
        let needle_p = format!("{}=", k);
        args.iter().any(|a| {
            let lower = a.trim_start().to_ascii_lowercase();
            lower.starts_with(&needle_q) || lower.starts_with(&needle_p)
        })
    };

    let has_path = has_key("path") || has_key("file_path") || has_key("filepath");
    let has_content = has_key("content");
    let has_old = has_key("old_string") || has_key("old_text");
    let has_new = has_key("new_string") || has_key("new_text");
    let has_command = has_key("command");
    let has_pattern = has_key("pattern");
    let has_query = has_key("query");
    let has_url = has_key("url");

    if has_command {
        return Some("bash");
    }
    if has_url {
        return Some("web_fetch");
    }
    if has_path && (has_old || has_new) {
        return Some("edit_file");
    }
    if has_path && has_content {
        return Some("write_file");
    }
    if has_pattern && has_path {
        return Some("grep_search");
    }
    if has_query && !has_pattern {
        return Some("web_search");
    }
    if has_pattern {
        return Some("glob_search");
    }
    if has_path {
        return Some("read_file");
    }
    None
}

/// Names of bare XML tags that some providers emit as direct children of
/// `<tool_call>` / `<invoke>` instead of wrapping each value in
/// `<parameter name="...">`. We rewrite them into the canonical
/// `<parameter name="X">VALUE</parameter>` form so downstream extractors
/// treat them as real arguments.
const BARE_PARAM_TAGS: &[&str] = &[
    "content",
    "path",
    "file_path",
    "filepath",
    "command",
    "pattern",
    "query",
    "url",
    "old_text",
    "new_text",
    "old_string",
    "new_string",
    "start_line",
    "max_lines",
];

/// Coalesce multi-line `<parameter ...>...</parameter>` blocks and bare
/// `<NAME>...</NAME>` blocks (for known parameter names) into single logical
/// strings so the existing line-based extractors still work when the body
/// spans many lines (e.g. multi-line file content). Bare named tags are
/// rewritten to canonical `<parameter name="NAME">BODY</parameter>` form.
fn coalesce_param_blocks(lines: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();

        // Multi-line <parameter ...> ... </parameter>.
        if lower.starts_with("<parameter") {
            // If close is on the same line, leave it alone.
            if let Some(gt_idx) = trimmed.find('>') {
                let after_open = &trimmed[gt_idx + 1..];
                if after_open.to_ascii_lowercase().contains("</parameter>") {
                    out.push(line.to_string());
                    i += 1;
                    continue;
                }
            }
            let mut buf = trimmed.to_string();
            let mut consumed = 0usize;
            let mut found_close = false;
            while i + consumed + 1 < lines.len() {
                let next = lines[i + consumed + 1];
                consumed += 1;
                buf.push('\n');
                buf.push_str(next);
                if next.to_ascii_lowercase().contains("</parameter>") {
                    found_close = true;
                    break;
                }
            }
            if found_close {
                out.push(buf);
                i += consumed + 1;
                continue;
            }
            out.push(line.to_string());
            i += 1;
            continue;
        }

        // Multi-line <TAG name="..."> ... </TAG> for any non-tool tag with a
        // name="..." attribute (e.g. <param name="x">, <arg name="x">,
        // <path_param name="x">). Coalesce the body so the line-based
        // extractors see the full inner text.
        if let Some(tag) = named_attr_tag_open_for_coalesce(trimmed) {
            // Same-line close?
            let close = format!("</{}>", tag);
            if let Some(gt_idx) = trimmed.find('>') {
                let after_open = &trimmed[gt_idx + 1..];
                if after_open.to_ascii_lowercase().contains(&close) {
                    out.push(line.to_string());
                    i += 1;
                    continue;
                }
            }
            let mut buf = trimmed.to_string();
            let mut consumed = 0usize;
            let mut found_close = false;
            while i + consumed + 1 < lines.len() {
                let next = lines[i + consumed + 1];
                consumed += 1;
                buf.push('\n');
                buf.push_str(next);
                if next.to_ascii_lowercase().contains(&close) {
                    found_close = true;
                    break;
                }
            }
            if found_close {
                out.push(buf);
                i += consumed + 1;
                continue;
            }
            out.push(line.to_string());
            i += 1;
            continue;
        }

        // Bare named param: <NAME>...</NAME>, possibly multi-line.
        if let Some(name) = parse_bare_param_open(trimmed) {
            let open_tag = format!("<{}>", name);
            let close_tag = format!("</{}>", name);
            // Strip the open tag prefix to find the body start.
            let after_open = &trimmed[open_tag.len()..];
            // Same-line close.
            if let Some(close_rel) = after_open.to_ascii_lowercase().find(&close_tag) {
                let body = &after_open[..close_rel];
                out.push(rewrite_bare_param_as_parameter(&name, body));
                i += 1;
                continue;
            }
            // Multi-line: keep collecting until close.
            let mut body = String::from(after_open);
            let mut consumed = 0usize;
            let mut found_close = false;
            while i + consumed + 1 < lines.len() {
                let next = lines[i + consumed + 1];
                consumed += 1;
                let next_lower = next.to_ascii_lowercase();
                if let Some(close_rel) = next_lower.find(&close_tag) {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str(&next[..close_rel]);
                    found_close = true;
                    break;
                }
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(next);
            }
            if found_close {
                out.push(rewrite_bare_param_as_parameter(&name, &body));
                i += consumed + 1;
                continue;
            }
            // Fallback: emit as-is if no close was found.
            out.push(line.to_string());
            i += 1;
            continue;
        }

        out.push(line.to_string());
        i += 1;
    }
    out
}

fn parse_bare_param_open(trimmed: &str) -> Option<String> {
    if !trimmed.starts_with('<') {
        return None;
    }
    let gt = trimmed.find('>')?;
    let inside = &trimmed[1..gt];
    // Reject self-closing or attributed open tags here; those go through other
    // paths.
    if inside.ends_with('/') || inside.contains(char::is_whitespace) {
        return None;
    }
    let name = inside.to_ascii_lowercase();
    if BARE_PARAM_TAGS.iter().any(|t| *t == name) {
        Some(name)
    } else {
        None
    }
}

/// If `trimmed` opens a non-tool tag with a `name="..."` attribute (e.g.
/// `<param name="content">`, `<arg name="path">`, `<path_param name="x">`),
/// return the lowercased tag name so it can be paired with its closer for
/// multi-line coalescing.
fn named_attr_tag_open_for_coalesce(trimmed: &str) -> Option<String> {
    if !trimmed.starts_with('<') || trimmed.starts_with("</") {
        return None;
    }
    let tag = tag_name_of(trimmed);
    if tag.is_empty() || tag == "parameter" || is_known_tool_name(&tag) {
        return None;
    }
    // Reserved structural tags must not be coalesced as parameters.
    if matches!(
        tag.as_str(),
        "invoke"
            | "function-call"
            | "function_call"
            | "function-calls"
            | "function_calls"
            | "tool_call"
            | "toolcall"
            | "tool_use"
            | "tool"
    ) {
        return None;
    }
    let name_attr = extract_attr_tag_value(trimmed, &tag, "name")?;
    if name_attr.is_empty() {
        return None;
    }
    Some(tag)
}

fn rewrite_bare_param_as_parameter(name: &str, body: &str) -> String {
    let trimmed_body = body.trim_matches(['\r', '\n']);
    format!(
        "<parameter name=\"{}\">{}</parameter>",
        name, trimmed_body
    )
}

/// True when the first non-empty line of a block body looks like one of the
/// canonical bare named param tags (e.g. `<path>`, `<content>`,
/// `<command>`). Used to switch the open-tag-block parser between
/// "treat body as heredoc" and "treat body as named children" modes.
fn body_starts_with_named_param_tag(body_lines: &[&str]) -> bool {
    for raw in body_lines {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if parse_bare_param_open(trimmed).is_some() {
            return true;
        }
        let lower = trimmed.to_ascii_lowercase();
        // Canonical <parameter ...>... shape.
        if lower.starts_with("<parameter") {
            return true;
        }
        // Provider variants such as <param name="...">, <arg name="...">,
        // <path_param name="..."> — but only via the named-attr-tag helper
        // so we skip structural tags like <invoke>, <function_call>, etc.
        if named_attr_tag_open_for_coalesce(trimmed).is_some() {
            return true;
        }
        return false;
    }
    false
}

fn tag_name_of(line: &str) -> String {
    let trimmed = line.trim();
    if !trimmed.starts_with('<') {
        return String::new();
    }
    let after = &trimmed[1..];
    let end = after
        .find(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/')
        .unwrap_or(after.len());
    after[..end].to_ascii_lowercase()
}

/// Recognize the malformed pattern `<TOOL_NAME:HEAD_ARG>` or `<TOOL_NAME>HEAD`
/// as the first non-empty child of a `<toolcall>` block. Returns
/// `(tool, head_arg, body_start_index)` so the caller can stitch the
/// remaining body lines back as a heredoc-style positional argument.
fn peel_pseudo_tool_first_child(body_lines: &[&str]) -> Option<(String, String, usize)> {
    let mut idx = 0usize;
    while idx < body_lines.len() {
        let trimmed = body_lines[idx].trim();
        if trimmed.is_empty() {
            idx += 1;
            continue;
        }
        if !trimmed.starts_with('<') {
            return None;
        }
        // Find the closing '>' of the open tag (it may not be at end of line).
        let gt = trimmed.find('>')?;
        let inside = &trimmed[1..gt];
        if inside.starts_with('/') {
            return None;
        }
        let after_gt = trimmed[gt + 1..].trim().to_string();
        // <TOOL:VALUE>[trailing]
        if let Some((head, value)) = inside.split_once(':') {
            let candidate = head.trim().to_ascii_lowercase();
            let normalized = normalize_tag_tool_name(&candidate);
            if is_known_tool_name(&normalized) {
                let mut head_arg = value.trim().to_string();
                if !after_gt.is_empty() {
                    if !head_arg.is_empty() {
                        head_arg.push(' ');
                    }
                    head_arg.push_str(&after_gt);
                }
                return Some((normalized, head_arg, idx + 1));
            }
        }
        // <TOOL>[trailing]
        let candidate = inside.trim().to_ascii_lowercase();
        let normalized = normalize_tag_tool_name(&candidate);
        if is_known_tool_name(&normalized) {
            return Some((normalized, after_gt, idx + 1));
        }
        return None;
    }
    None
}

fn extract_tool_call_wrapper_name(line: &str) -> Option<String> {
    extract_attr_tag_value(line, "tool_call", "name")
        .or_else(|| extract_attr_tag_value(line, "toolcall", "name"))
}

fn extract_inline_xml_tag_value(line: &str, tag: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let open = format!("<{}>", tag.to_ascii_lowercase());
    let close = format!("</{}>", tag.to_ascii_lowercase());
    if lower.starts_with(&open) && lower.ends_with(&close) && line.len() >= open.len() + close.len()
    {
        let start = open.len();
        let end = line.len() - close.len();
        return Some(line[start..end].trim().to_string());
    }
    None
}

fn extract_invoke_name(line: &str) -> Option<String> {
    extract_attr_tag_value(line, "invoke", "name")
}

fn extract_parameter_value(line: &str) -> Option<String> {
    if let Some(v) = extract_attr_tag_inner_text(line, "parameter") {
        return Some(v);
    }
    extract_attr_tag_value(line, "parameter", "value")
}

fn extract_parameter_as_legacy_arg(line: &str) -> Option<String> {
    let key = extract_attr_tag_value(line, "parameter", "name");
    let value = extract_parameter_value(line);
    match (key, value) {
        (Some(k), Some(v)) => {
            let escaped = v.replace('\\', "\\\\").replace('"', "\\\"");
            Some(format!(r#"{}="{}""#, k, escaped))
        }
        (None, Some(v)) => Some(v),
        _ => None,
    }
}

/// Generalized version of `extract_parameter_as_legacy_arg` that accepts any
/// XML tag with a `name="..."` attribute (e.g. `<param name="x">`,
/// `<arg name="x">`, `<path_param name="x">`). The tag itself must NOT be a
/// known tool name (to avoid mis-parsing `<bash name="...">` as a param).
fn extract_named_attr_tag_as_legacy_arg(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let tag = tag_name_of(trimmed);
    if tag.is_empty() || tag == "parameter" || is_known_tool_name(&tag) {
        return None;
    }
    let key = extract_attr_tag_value(trimmed, &tag, "name")?;
    if key.is_empty() {
        return None;
    }
    // Inner text between `>` and `</TAG>`.
    let value = extract_attr_tag_inner_text(trimmed, &tag).unwrap_or_default();
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    Some(format!(r#"{}="{}""#, key, escaped))
}

fn extract_attr_tag_value(line: &str, tag: &str, attr: &str) -> Option<String> {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    let open = format!("<{}", tag.to_ascii_lowercase());
    if !lower.starts_with(&open) {
        return None;
    }
    let attr_pat = format!(r#"{}=""#, attr.to_ascii_lowercase());
    let idx = lower.find(&attr_pat)?;
    let val_start = idx + attr_pat.len();
    let rest = &trimmed[val_start..];
    let end_rel = rest.find('"')?;
    Some(rest[..end_rel].to_string())
}

fn extract_attr_tag_inner_text(line: &str, tag: &str) -> Option<String> {
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();
    let open = format!("<{}", tag.to_ascii_lowercase());
    if !lower.starts_with(&open) {
        return None;
    }
    let gt = trimmed.find('>')?;
    let close = format!("</{}>", tag.to_ascii_lowercase());
    let close_idx = lower.rfind(&close)?;
    if close_idx <= gt {
        return None;
    }
    Some(trimmed[gt + 1..close_idx].trim().to_string())
}

fn normalize_xml_attrs(attrs_raw: &str) -> Option<String> {
    if attrs_raw.is_empty() {
        return Some(String::new());
    }

    let bytes = attrs_raw.as_bytes();
    let mut idx = 0usize;
    let mut out: Vec<String> = Vec::new();
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
        let key = &attrs_raw[key_start..idx];

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
        if idx >= bytes.len() {
            return None;
        }

        let quote = bytes[idx];
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        idx += 1;
        let mut value = String::new();
        let mut closed = false;
        while idx < bytes.len() {
            let b = bytes[idx];
            idx += 1;
            if b == quote {
                closed = true;
                break;
            }
            value.push(char::from(b));
        }
        if !closed {
            return None;
        }

        let escaped = value.replace('"', "\\\"");
        out.push(format!(r#"{}="{}""#, key, escaped));
    }

    Some(out.join(" "))
}

fn wrap_body_as_heredoc(body: &str) -> String {
    let body_clean = body.trim_matches('\n').replace('\r', "");
    format!("<<<CONTENT\n{}\n<<<END", body_clean)
}

fn parse_toolcall_text(raw: &str) -> Option<ToolCallRequest> {
    parse_toolcall_text_many(raw).into_iter().next()
}

fn parse_toolcall_text_many(raw: &str) -> Vec<ToolCallRequest> {
    let Some(rest) = raw.strip_prefix("/toolcall ").map(str::trim) else {
        return Vec::new();
    };
    if let Some(invocations) = parse_json_tool_invocations(rest) {
        if !invocations.is_empty() {
            return invocations
                .into_iter()
                .map(|(name, args)| {
                    let call_raw = format_toolcall_raw(&name, &args);
                    ToolCallRequest::new(call_raw, name, args)
                })
                .collect();
        }
    }
    if let Some((name, args)) = parse_tool_invocation(rest) {
        return vec![ToolCallRequest::new(raw.to_string(), name, args)];
    }
    Vec::new()
}

fn format_toolcall_raw(name: &str, args: &str) -> String {
    if args.is_empty() {
        format!("/toolcall {}", name)
    } else {
        format!("/toolcall {} {}", name, args)
    }
}

fn parse_tool_invocation(rest: &str) -> Option<(String, String)> {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some((name, args)) = parse_function_style(trimmed) {
        return Some((name, args));
    }

    if let Some((name, args)) = parse_json_tool_invocation(trimmed) {
        return Some((name, args));
    }

    let mut it = trimmed.splitn(2, ' ');
    let name = it.next().unwrap_or("").trim();
    let args = it.next().unwrap_or("").trim();
    if name.is_empty() {
        return None;
    }
    // Heredoc-style payload can contain arbitrary quotes in the body, so
    // quote-balance check is only meaningful for non-heredoc invocations.
    if !has_heredoc_marker(args) && has_unclosed_quote(args) {
        return None;
    }

    Some((name.to_string(), args.to_string()))
}

fn parse_json_tool_invocation(raw: &str) -> Option<(String, String)> {
    parse_json_tool_invocations(raw)
        .and_then(|mut items| {
            if items.is_empty() {
                None
            } else {
                Some(items.remove(0))
            }
        })
}

fn parse_json_tool_invocations(raw: &str) -> Option<Vec<(String, String)>> {
    let trimmed = raw.trim();
    if !(trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with('"')
        || trimmed.starts_with('\'')
        || trimmed.starts_with('`'))
    {
        return None;
    }
    let value = crate::json_toolcall::parse_relaxed_json_value(trimmed)?;
    let mut candidates = Vec::new();
    crate::json_toolcall::collect_json_tool_candidates(&value, &mut candidates);
    if candidates.is_empty() {
        return None;
    }

    let mut out = Vec::new();
    for candidate in candidates {
        if let Some(name) = crate::json_toolcall::extract_json_tool_name(candidate) {
            let args = extract_json_tool_args(candidate, &name);
            out.push((name, args));
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn extract_json_tool_args(candidate: &Value, name: &str) -> String {
    let Some(obj) = candidate.as_object() else {
        return String::new();
    };
    let args_value = obj
        .get("function")
        .and_then(Value::as_object)
        .and_then(|f| f.get("arguments"))
        .or_else(|| obj.get("arguments"));
    let Some(args_raw) = args_value else {
        return String::new();
    };
    match args_raw {
        Value::String(s) => {
            let normalized = crate::json_toolcall::normalize_json_string_argument(s);
            tool_call_to_legacy_args(name, &normalized)
        }
        other => tool_call_to_legacy_args(name, &other.to_string()),
    }
}

fn has_heredoc_marker(s: &str) -> bool {
    s.contains("<<<")
}

fn starts_with_named_arg_fragment(rest: &str) -> bool {
    let trimmed = rest.trim_start();
    let mut key = String::new();

    for ch in trimmed.chars() {
        if ch == '=' {
            break;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            key.push(ch);
        } else {
            return false;
        }
    }

    if key.is_empty() {
        return false;
    }

    let has_eq = trimmed.get(key.len()..).is_some_and(|tail| tail.starts_with('='));
    if !has_eq {
        return false;
    }

    matches!(
        key.as_str(),
        "path"
            | "file_path"
            | "content"
            | "old_text"
            | "new_text"
            | "command"
            | "pattern"
            | "query"
            | "url"
            | "start_line"
            | "max_lines"
    )
}

fn can_absorb_named_fragment(previous_raw: &str) -> bool {
    let rest = previous_raw
        .strip_prefix("/toolcall ")
        .map(str::trim)
        .unwrap_or_default();
    let Some(name) = parse_tool_name_for_capture(rest) else {
        return false;
    };
    if !is_known_tool_name(&name) || previous_raw.contains("<<<") {
        return false;
    }
    true
}

fn is_known_tool_name(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "write_file"
            | "edit_file"
            | "glob_search"
            | "grep_search"
            | "web_search"
            | "web_fetch"
            | "bash"
    )
}

fn parse_tool_name_for_capture(rest: &str) -> Option<String> {
    let trimmed = rest.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some((name, _)) = parse_function_style(trimmed) {
        return Some(name);
    }

    let name = trimmed.split_whitespace().next().unwrap_or("").trim();
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

fn needs_multiline_capture(name: &str, raw: &str) -> bool {
    if !matches!(name, "write_file" | "edit_file") {
        return false;
    }

    raw.contains("<<<") || !is_double_quote_balanced(raw)
}

fn heredoc_terminator(name: &str, raw: &str) -> Option<String> {
    if !raw.contains("<<<") {
        return None;
    }

    if name == "edit_file" || raw.contains("<<<OLD") || raw.contains("<<<NEW") {
        return Some("<<<END".to_string());
    }

    let marker = parse_heredoc_marker(raw)?;
    Some(marker)
}

fn parse_heredoc_marker(raw: &str) -> Option<String> {
    let idx = raw.find("<<<")?;
    let after = &raw[idx + 3..];
    let marker = after.lines().next()?.trim();
    if marker.is_empty() {
        return None;
    }
    Some(marker.to_string())
}

fn is_double_quote_balanced(s: &str) -> bool {
    let mut in_escape = false;
    let mut count = 0usize;

    for ch in s.chars() {
        if in_escape {
            in_escape = false;
            continue;
        }

        if ch == '\\' {
            in_escape = true;
            continue;
        }

        if ch == '"' {
            count += 1;
        }
    }

    count % 2 == 0
}

fn has_unclosed_quote(s: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut escape = false;

    for ch in s.chars() {
        if let Some(q) = quote {
            if escape {
                escape = false;
                continue;
            }
            if ch == '\\' {
                escape = true;
                continue;
            }
            if ch == q {
                quote = None;
            }
            continue;
        }

        if ch == '"' || ch == '\'' {
            quote = Some(ch);
        }
    }

    quote.is_some()
}

fn parse_function_style(rest: &str) -> Option<(String, String)> {
    let open = rest.find('(')?;
    if !rest.ends_with(')') {
        return None;
    }

    let name = rest[..open].trim();
    if name.is_empty() {
        return None;
    }
    if !matches!(
        name,
        "read_file"
            | "write_file"
            | "edit_file"
            | "glob_search"
            | "grep_search"
            | "web_search"
            | "web_fetch"
            | "bash"
    ) {
        return None;
    }

    let inner = &rest[open + 1..rest.len() - 1];
    let args = if let Some(kv_args) = parse_named_function_args(inner) {
        kv_args
    } else {
        let parts = split_function_args(inner);
        parts
            .iter()
            .map(|p| normalize_arg_token(p))
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };

    Some((name.to_string(), args))
}

fn parse_named_function_args(inner: &str) -> Option<String> {
    let parts = split_function_args(inner);
    if parts.is_empty() {
        return Some(String::new());
    }
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        let trimmed = part.trim();
        let eq = trimmed.find('=')?;
        let key = trimmed[..eq].trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            return None;
        }
        let value = normalize_arg_token(trimmed[eq + 1..].trim())
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        out.push(format!(r#"{}="{}""#, key, value));
    }
    Some(out.join(" "))
}

fn split_function_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut escape = false;
    let mut depth = 0usize;

    for ch in s.chars() {
        if let Some(q) = quote {
            cur.push(ch);
            if escape {
                escape = false;
                continue;
            }
            if ch == '\\' {
                escape = true;
                continue;
            }
            if ch == q {
                quote = None;
            }
            continue;
        }

        match ch {
            '"' | '\'' => {
                quote = Some(ch);
                cur.push(ch);
            }
            '(' | '[' | '{' => {
                depth += 1;
                cur.push(ch);
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                cur.push(ch);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }

    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }

    out
}

fn normalize_arg_token(token: &str) -> String {
    let t = token.trim();
    if t.len() >= 2 {
        let b = t.as_bytes();
        let quoted =
            (b[0] == b'"' && b[t.len() - 1] == b'"') || (b[0] == b'\'' && b[t.len() - 1] == b'\'');
        if quoted {
            return t[1..t.len() - 1].replace("\\\"", "\"").replace("\\'", "'");
        }
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::{extract_tool_calls, parse_toolcall_line};

    #[test]
    fn parses_function_style_toolcall() {
        let call = parse_toolcall_line("/toolcall glob_search(\"src/**/*.ts\")").unwrap();
        assert_eq!(call.name, "glob_search");
        assert_eq!(call.args, "src/**/*.ts");
    }

    #[test]
    fn extracts_multiline_write_file_heredoc() {
        let text = "\n/toolcall write_file \"a.md\" <<<EOF\nline1\nline2\nEOF\n";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("<<<EOF"));
        assert!(calls[0].args.contains("line1"));
        assert!(calls[0].args.contains("line2"));
    }

    #[test]
    fn extracts_multiline_write_file_quoted_content() {
        let text = "/toolcall write_file \"a.md\" \"title\nbody\"\n";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("title"));
        assert!(calls[0].args.contains("body"));
    }

    #[test]
    fn extracts_multiline_write_file_content_marker() {
        let text = "/toolcall write_file \"a.md\" <<<CONTENT\nline1\nline2\n<<<END\n";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("<<<CONTENT"));
        assert!(calls[0].args.contains("line1"));
        assert!(calls[0].args.contains("<<<END"));
    }

    #[test]
    fn extracts_multiline_write_file_content_with_quotes() {
        let text = "/toolcall write_file \"a.py\" <<<CONTENT\nprint(\"hello\")\nname='asi'\n<<<END\n";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("print(\"hello\")"));
        assert!(calls[0].args.contains("name='asi'"));
        assert!(calls[0].args.contains("<<<END"));
    }

    #[test]
    fn content_marker_terminates_at_content_line() {
        let text = "/toolcall write_file \"a.py\" <<<CONTENT\nprint(\"hello\")\nCONTENT\n/toolcall glob_search \"**/*.py\"\n";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("print(\"hello\")"));
        assert!(!calls[0].args.contains("glob_search"));
        assert_eq!(calls[1].name, "glob_search");
    }

    #[test]
    fn extracts_multiline_edit_file_blocks() {
        let text = "/toolcall edit_file \"a.md\" <<<OLD\nold line\n<<<NEW\nnew line\n<<<END\n";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "edit_file");
        assert!(calls[0].args.contains("<<<OLD"));
        assert!(calls[0].args.contains("old line"));
        assert!(calls[0].args.contains("<<<NEW"));
        assert!(calls[0].args.contains("new line"));
        assert!(calls[0].args.contains("<<<END"));
    }

    #[test]
    fn skips_incomplete_toolcall_with_unclosed_quote() {
        let text = "/toolcall write_file \"requirements_";
        let calls = extract_tool_calls(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn parses_json_function_wrapper_toolcall() {
        let text = "/toolcall [{\"type\":\"function\",\"function\":{\"name\":\"glob_search\",\"arguments\":\"{\\\"pattern\\\":\\\"*.py\\\"}\"}}]";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "glob_search");
        assert_eq!(calls[0].args, "*.py");
    }

    #[test]
    fn parses_escaped_json_function_wrapper_toolcall() {
        let text = r#"/toolcall [{\"type\":\"function\",\"function\":{\"name\":\"glob_search\",\"arguments\":\"{\\\"pattern\\\":\\\"*.py\\\"}\"}}]"#;
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "glob_search");
        assert_eq!(calls[0].args, "*.py");
    }

    #[test]
    fn parses_json_wrapper_array_with_multiple_tool_calls() {
        let text = r#"/toolcall [{"type":"function","function":{"name":"glob_search","arguments":{"pattern":"*.py"}}},{"type":"function","function":{"name":"read_file","arguments":{"path":"main.py"}}}]"#;
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "glob_search");
        assert_eq!(calls[0].args, "*.py");
        assert_eq!(calls[1].name, "read_file");
        assert_eq!(calls[1].args, "main.py");
    }

    #[test]
    fn extracts_toolcalls_wrapped_in_markdown_fences() {
        let text = "I will inspect first.\n```bash\n/toolcall glob_search \"*.py\"\n```\n```bash\n/toolcall read_file \"main.py\" 1 120\n```";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "glob_search");
        assert_eq!(calls[0].args, "\"*.py\"");
        assert_eq!(calls[1].name, "read_file");
        assert_eq!(calls[1].args, "\"main.py\" 1 120");
    }

    #[test]
    fn parses_function_style_named_args_for_known_tool() {
        let text = r#"/toolcall bash(command="Write-Output 'ok'")"#;
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].args.contains("command=\""));
    }

    #[test]
    fn ignores_non_tool_function_style_lines() {
        let text = "Confidence Declaration: high";
        let calls = extract_tool_calls(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn merges_fragmented_write_file_named_arg_lines() {
        let text = "/toolcall write_file\n/toolcall path=\"analysis_en.md\"\n/toolcall content=\"# Report\\nAll good\\n\"";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("path=\"analysis_en.md\""));
        assert!(calls[0].args.contains("content=\"# Report\\nAll good\\n\""));
    }

    #[test]
    fn merges_fragment_into_existing_named_arg_call() {
        let text =
            "/toolcall write_file path=\"analysis_zh.md\"\n/toolcall content=\"# Report\\nLooks good\\n\"";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("path=\"analysis_zh.md\""));
        assert!(calls[0].args.contains("content=\"# Report\\nLooks good\\n\""));
    }

    #[test]
    fn parses_tag_style_file_write_to_toolcall() {
        let text = "<file_write path=\"D:\\\\test-cli\\\\calcplus\\\\core.py\">\n<<<CONTENT\ndef add(a, b):\n    return a + b\n<<<END\n</file_write>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("path=\"D:"));
        assert!(calls[0].args.contains("calcplus"));
        assert!(calls[0].args.contains("core.py\""));
        assert!(calls[0].args.contains("<<<CONTENT"));
        assert!(calls[0].args.contains("def add(a, b):"));
        assert!(calls[0].args.contains("<<<END"));
    }

    #[test]
    fn parses_tag_style_file_edit_to_toolcall() {
        let text = "<file_edit path=\"D:\\\\test-cli\\\\calcplus\\\\core.py\" old_text=\"a\" new_text=\"b\">\n</file_edit>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "edit_file");
        assert!(calls[0].args.contains("path=\"D:"));
        assert!(calls[0].args.contains("calcplus"));
        assert!(calls[0].args.contains("core.py\""));
        assert!(calls[0].args.contains("old_text=\"a\""));
        assert!(calls[0].args.contains("new_text=\"b\""));
    }

    #[test]
    fn parses_tag_style_bash_to_toolcall() {
        let text = "<bash command=\"python --version 2>&1\">\n</bash>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].args.contains("command=\"python --version 2>&1\""));
    }

    #[test]
    fn parses_tag_style_glob_search_with_body_as_ignored_if_empty() {
        let text = "<glob_search pattern=\"src/**/*.py\">\n</glob_search>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "glob_search");
        assert_eq!(calls[0].args, "pattern=\"src/**/*.py\"");
    }

    #[test]
    fn parses_tool_call_wrapper_xml() {
        let text = "<tool_call>\n<tool_name>glob_search</tool_name>\n<tool_arg>**/Cargo.toml</tool_arg>\n</tool_call>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "glob_search");
        assert_eq!(calls[0].args, "**/Cargo.toml");
    }

    #[test]
    fn parses_toolcall_wrapper_with_name_and_parameter_tags() {
        let text = "<toolcall name=\"read_file\">\n<parameter name=\"path\">D:\\\\test-cli\\\\sessions\\\\_checkpoint_latest.json</parameter>\n<parameter name=\"max_lines\">60</parameter>\n</toolcall>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert!(calls[0].args.contains("path=\"D:"));
        assert!(calls[0].args.contains("test-cli"));
        assert!(calls[0].args.contains("_checkpoint_latest.json\""));
        assert!(calls[0].args.contains("max_lines=\"60\""));
    }

    #[test]
    fn parses_tool_call_wrapper_with_name_and_parameter_tags() {
        let text = "<tool_call name=\"glob_search\">\n<parameter name=\"pattern\">**/*</parameter>\n</tool_call>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "glob_search");
        assert_eq!(calls[0].args, "pattern=\"**/*\"");
    }

    #[test]
    fn parses_function_calls_invoke_parameter_xml() {
        let text = "<function-calls>\n<function-call>\n<invoke name=\"glob_search\">\n<parameter name=\"pattern\" string=\"true\">**/Cargo.toml</parameter>\n</invoke>\n</function-call>\n</function-calls>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "glob_search");
        assert_eq!(calls[0].args, "pattern=\"**/Cargo.toml\"");
    }

    #[test]
    fn parses_function_calls_with_bash_command_parameter() {
        let text = "<function-calls>\n<function-call>\n<invoke name=\"bash\">\n<parameter name=\"command\" string=\"true\">pwd; Write-Output \"TERMINAL_BENCH2_PROXY_OK\"</parameter>\n</invoke>\n</function-call>\n</function-calls>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].args.contains("command=\"pwd; Write-Output"));
    }

    #[test]
    fn parses_function_calls_with_multiple_invokes() {
        let text = "<function-calls>\n<function-call>\n<invoke name=\"glob_search\">\n<parameter name=\"pattern\">**/*.rs</parameter>\n</invoke>\n</function-call>\n<function-call>\n<invoke name=\"read_file\">\n<parameter name=\"path\">Cargo.toml</parameter>\n<parameter name=\"start_line\">1</parameter>\n<parameter name=\"max_lines\">120</parameter>\n</invoke>\n</function-call>\n</function-calls>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "glob_search");
        assert_eq!(calls[0].args, "pattern=\"**/*.rs\"");
        assert_eq!(calls[1].name, "read_file");
        assert!(calls[1].args.contains("path=\"Cargo.toml\""));
        assert!(calls[1].args.contains("start_line=\"1\""));
        assert!(calls[1].args.contains("max_lines=\"120\""));
    }

    #[test]
    fn parses_self_closing_xml_tool_tags() {
        let text =
            "<read_file path=\"D:\\\\test-cli\\\\config.json\" start_line=\"1\" max_lines=\"50\"/>\n<glob_search pattern=\"**/*.py\"/>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "read_file");
        assert!(calls[0].args.contains("path=\"D:"));
        assert!(calls[0].args.contains("test-cli"));
        assert!(calls[0].args.contains("config.json\""));
        assert!(calls[0].args.contains("start_line=\"1\""));
        assert!(calls[0].args.contains("max_lines=\"50\""));
        assert_eq!(calls[1].name, "glob_search");
        assert_eq!(calls[1].args, "pattern=\"**/*.py\"");
    }

    #[test]
    fn parses_tool_arguments_pair_style_lines() {
        let text = "Tool: read_file\nArguments: {\"path\": \"D:\\\\test-cli\\\\sessions\\\\_checkpoint_latest.json\", \"max_lines\": 60}\nTool: glob_search\nArguments: {\"pattern\": \"**/*\", \"path\": \"D:\\\\test-cli\"}";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "read_file");
        assert!(calls[0].args.contains("path=\"D:"));
        assert!(calls[0].args.contains("test-cli"));
        assert!(calls[0].args.contains("_checkpoint_latest.json\""));
        assert!(calls[0].args.contains("max_lines=\"60\""));
        assert_eq!(calls[1].name, "glob_search");
        assert!(calls[1].args.contains("pattern=\"**/*\""));
    }

    // Regression: deepseek-style <function_calls> (underscore) wrapping a
    // single <invoke> with no <function-call> inner wrapper.
    #[test]
    fn parses_underscore_function_calls_invoke_without_function_call_wrapper() {
        let text = "<function_calls>\n<invoke name=\"glob_search\">\n<parameter name=\"pattern\" string=\"true\">**/*</parameter>\n</invoke>\n</function_calls>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "glob_search");
        assert!(calls[0].args.contains("pattern=\"**/*\""));
    }

    #[test]
    fn parses_underscore_function_calls_with_multiple_invokes() {
        let text = "<function_calls>\n<invoke name=\"glob_search\">\n<parameter name=\"pattern\">**/*.rs</parameter>\n</invoke>\n<invoke name=\"read_file\">\n<parameter name=\"path\">Cargo.toml</parameter>\n</invoke>\n</function_calls>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "glob_search");
        assert!(calls[0].args.contains("pattern=\"**/*.rs\""));
        assert_eq!(calls[1].name, "read_file");
        assert!(calls[1].args.contains("path=\"Cargo.toml\""));
    }

    // Regression: <tool_call name="X"> with bare child param tags such as
    // <content>, <path>, <command> instead of <parameter name="...">.
    #[test]
    fn parses_tool_call_wrapper_with_bare_named_param_tags() {
        let text = "<tool_call name=\"write_file\">\n<content># top</content>\n<path>src/x.py</path>\n</tool_call>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("content=\"# top\""));
        assert!(calls[0].args.contains("path=\"src/x.py\""));
    }

    #[test]
    fn parses_tool_call_wrapper_with_multiline_content_block() {
        let text = "<tool_call name=\"write_file\">\n<content>line1\nline2\nline3</content>\n<path>a.py</path>\n</tool_call>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("line1"));
        assert!(calls[0].args.contains("line2"));
        assert!(calls[0].args.contains("line3"));
        assert!(calls[0].args.contains("path=\"a.py\""));
    }

    #[test]
    fn parses_multiple_consecutive_tool_call_blocks_with_bare_params() {
        let text = "<tool_call name=\"write_file\">\n<content>A</content>\n<path>a.py</path>\n</tool_call>\n<tool_call name=\"write_file\">\n<content>B</content>\n<path>b.py</path>\n</tool_call>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "write_file");
        assert_eq!(calls[1].name, "write_file");
        assert!(calls[0].args.contains("path=\"a.py\""));
        assert!(calls[1].args.contains("path=\"b.py\""));
    }

    #[test]
    fn parses_tool_call_wrapper_with_bare_command_for_bash() {
        let text = "<tool_call name=\"bash\">\n<command>python --version</command>\n</tool_call>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].args.contains("command=\"python --version\""));
    }

    #[test]
    fn parses_multiline_parameter_block_inside_invoke() {
        let text = "<function_calls>\n<invoke name=\"write_file\">\n<parameter name=\"content\">line1\nline2</parameter>\n<parameter name=\"path\">x.py</parameter>\n</invoke>\n</function_calls>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("line1"));
        assert!(calls[0].args.contains("line2"));
        assert!(calls[0].args.contains("path=\"x.py\""));
    }

    // Regression: <toolcall><TOOL:VALUE>...</toolcall> pseudo-shape some
    // providers emit when they hallucinate a wire format.
    #[test]
    fn parses_toolcall_wrapper_with_pseudo_tool_value_child() {
        let text = "<toolcall>\n<bash:mkdir -p src tests>\n</toolcall>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].args.contains("mkdir -p src tests"));
    }

    #[test]
    fn parses_toolcall_wrapper_with_write_file_pseudo_value_and_heredoc_body() {
        let text = "<toolcall>\n<write_file:src/x.py>\n<<<CONTENT\nprint('hi')\n<<<END\n</toolcall>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("src/x.py"));
        assert!(calls[0].args.contains("<<<CONTENT"));
        assert!(calls[0].args.contains("print('hi')"));
        assert!(calls[0].args.contains("<<<END"));
    }

    #[test]
    fn parses_toolcall_wrapper_with_bare_tool_tag_and_heredoc_body() {
        let text = "<toolcall>\n<write_file>src/x.py\n<<<CONTENT\nbody\n<<<END\n</toolcall>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("src/x.py"));
        assert!(calls[0].args.contains("body"));
    }

    // Regression: multi-line `<TOOL attr="..." />` open tag whose `>`
    // appears on a later line because an attribute value contains newlines.
    #[test]
    fn parses_multiline_self_closing_write_file_with_content_attribute() {
        let text = "<write_file path=\"a.py\" content=\"line1\nline2\nline3\" />";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("path=\"a.py\""));
        assert!(calls[0].args.contains("line1"));
        assert!(calls[0].args.contains("line2"));
        assert!(calls[0].args.contains("line3"));
    }

    #[test]
    fn parses_multiline_write_file_open_close_tags_with_long_attribute() {
        let text = "<write_file path=\"a.py\" content=\"x\ny\">\n</write_file>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("path=\"a.py\""));
        assert!(calls[0].args.contains("x"));
        assert!(calls[0].args.contains("y"));
    }

    #[test]
    fn parses_multiple_consecutive_multiline_write_file_self_closing_tags() {
        let text = "<write_file path=\"a.py\" content=\"alpha\nbeta\" />\n<write_file path=\"b.py\" content=\"gamma\" />";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("path=\"a.py\""));
        assert!(calls[0].args.contains("alpha"));
        assert_eq!(calls[1].name, "write_file");
        assert!(calls[1].args.contains("path=\"b.py\""));
        assert!(calls[1].args.contains("gamma"));
    }

    // Regression: <write_file><path>...</path><content>...</content></write_file>
    // shape that reasoning models default to. Body children must be parsed as
    // named params, not as a positional heredoc body.
    #[test]
    fn parses_open_close_write_file_with_bare_named_param_children() {
        let text = "<write_file>\n<path>D:\\test\\a.py</path>\n<content>print('hi')\nname='asi'</content>\n</write_file>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("path=\"D:"));
        assert!(calls[0].args.contains("a.py\""));
        assert!(calls[0].args.contains("print('hi')"));
        assert!(calls[0].args.contains("name='asi'"));
    }

    #[test]
    fn parses_open_close_bash_with_bare_command_child() {
        let text = "<bash>\n<command>python -V 2>&1</command>\n</bash>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].args.contains("command=\"python -V 2>&1\""));
    }

    #[test]
    fn parses_two_consecutive_write_file_blocks_with_named_param_children() {
        let text = "<write_file>\n<path>a.py</path>\n<content>A</content>\n</write_file>\n<write_file>\n<path>b.py</path>\n<content>B</content>\n</write_file>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "write_file");
        assert_eq!(calls[1].name, "write_file");
        assert!(calls[0].args.contains("path=\"a.py\""));
        assert!(calls[1].args.contains("path=\"b.py\""));
    }

    // Regression: <bash tool_name="bash"><path_param name="command">...</path_param></bash>
    // — provider hallucinates a tool_name attribute on the open tag and uses
    // an unusual <path_param name="command"> child instead of <parameter>.
    #[test]
    fn parses_bash_with_path_param_named_command_child() {
        let text = "<bash tool_name=\"bash\">\n<path_param name=\"command\">ls -la 2>&1</path_param>\n</bash>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].args.contains("command=\"ls -la 2>&1\""));
    }

    #[test]
    fn parses_write_file_with_path_param_named_path_and_content_children() {
        let text = "<write_file tool_name=\"write_file\">\n<path_param name=\"path\">a.py</path_param>\n<path_param name=\"content\">print('x')\nprint('y')</path_param>\n</write_file>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("path=\"a.py\""));
        assert!(calls[0].args.contains("print('x')"));
        assert!(calls[0].args.contains("print('y')"));
    }

    // Regression: bare ```bash / ```shell fenced blocks should be promoted
    // to /toolcall bash so reasoning models that hallucinate fences instead
    // of /toolcall lines still execute.
    #[test]
    fn promotes_bare_bash_fence_block_to_toolcall_bash() {
        let text = "Let me check the workspace.\n\n```bash\nls -la /tmp/test 2>&1\n```\n";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].args.contains("ls -la /tmp/test 2>&1"));
    }

    #[test]
    fn promotes_bare_powershell_fence_block_to_toolcall_bash() {
        let text = "```powershell\nNew-Item -ItemType Directory -Force -Path \"D:\\x\" 2>&1\n```";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].args.contains("New-Item"));
    }

    #[test]
    fn promotes_python_fence_with_path_header_to_toolcall_write_file() {
        let text = "```python\n# D:/test/a.py\nprint('hello')\n```";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].args.contains("D:/test/a.py"));
        assert!(calls[0].args.contains("print('hello')"));
    }

    #[test]
    fn does_not_promote_python_fence_without_path_header() {
        // Without a path header these are documentation samples, not file
        // writes — leave them as inert prose.
        let text = "Sample:\n\n```python\nprint('hi')\n```";
        let calls = extract_tool_calls(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn does_not_promote_empty_fence_block() {
        let text = "```bash\n```";
        let calls = extract_tool_calls(text);
        assert!(calls.is_empty());
    }

    // Regression: a write_file open tag whose `content="..."` value contains
    // a markdown fence (e.g. README content with ```bash usage examples)
    // must NOT have those inner fences promoted to /toolcall bash, because
    // they're literal data inside an unclosed attribute, not actionable
    // shell blocks.
    // Regression: DeepSeek v4 Pro occasionally wraps tool calls in fullwidth
    // `｜DSML｜` sentinels (e.g. `<｜DSML｜invoke name="...">`). Strip the
    // sentinel so the call is recognized as a normal `<invoke>` tag.
    #[test]
    fn parses_dsml_fenced_function_calls_block() {
        let text = "<\u{FF5C}DSML\u{FF5C}tool_calls>\n<\u{FF5C}DSML\u{FF5C}invoke name=\"glob_search\">\n<\u{FF5C}DSML\u{FF5C}parameter name=\"pattern\" string=\"true\">**/*.py</\u{FF5C}DSML\u{FF5C}parameter>\n</\u{FF5C}DSML\u{FF5C}invoke>\n</\u{FF5C}DSML\u{FF5C}tool_calls>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "glob_search");
        assert!(calls[0].args.contains("pattern=\"**/*.py\""));
    }

    // Regression: DeepSeek v4 Pro (and similar reasoning-trained models)
    // sometimes describe tool calls in markdown prose instead of using the
    // native tool_calls field. The shape is:
    //   **Calling:** `tool_name`
    //   ```
    //   {json args}
    //   ```
    // Without this normalizer the runtime sees `completion.tool_calls` empty,
    // exits after one turn, and the user only sees the prose with no result.
    #[test]
    fn promotes_deepseek_calling_markdown_to_toolcall() {
        let text = "I'll search the web.\n**Calling:** `web_search`\n```\n{\"query\": \"OpenAI GPT-5.5 release 2025\"}\n```\nthat's it";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "web_search");
        assert!(
            calls[0].args.contains("OpenAI GPT-5.5"),
            "args should carry the query: {}",
            calls[0].args
        );
    }

    #[test]
    fn promotes_deepseek_calling_markdown_with_camelcase_filepath() {
        // Model hallucinates `filePath` instead of `path` — the parser must
        // still emit a real toolcall; the tool dispatcher then forgives the
        // alias.
        let text = "**Calling:** `read_file`\n```\n{\"filePath\": \"D:/test/foo.py\"}\n```";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert!(calls[0].args.contains("filePath") || calls[0].args.contains("D:/test/foo.py"));
    }

    #[test]
    fn ignores_calling_marker_for_unknown_tool_name() {
        // "Calling Alice" in prose must not become a toolcall.
        let text = "I was calling Alice on the phone.";
        let calls = extract_tool_calls(text);
        assert!(calls.is_empty());
    }

    // Regression: model emits `/toolcall bash` on one line and the actual
    // command on the next line. Without next-line absorption this dispatches
    // bash with empty args and silently logs "ambiguous intent".
    #[test]
    fn absorbs_next_line_as_args_for_bare_toolcall_bash() {
        let text = "/toolcall bash\nGet-ChildItem D:\\test-cli -Name 2>&1";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(
            calls[0].args.contains("Get-ChildItem"),
            "args should carry the next-line command: {}",
            calls[0].args
        );
    }

    #[test]
    fn does_not_absorb_next_toolcall_line_as_args() {
        let text = "/toolcall bash\n/toolcall read_file foo.py";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "bash");
        assert!(
            calls[0].args.is_empty(),
            "first bash must remain empty when next line is another toolcall: got {:?}",
            calls[0].args
        );
        assert_eq!(calls[1].name, "read_file");
    }

    // Regression: when the model wraps tool args in a non-canonical wrapper
    // tag like `<file-creation-attempt>` but the inner params are
    // unambiguous (`<path>`, `<content>`), we must still dispatch to
    // `write_file` rather than emitting an unknown-tool toolcall.
    #[test]
    fn infers_write_file_from_unknown_wrapper_with_path_and_content() {
        let text = "<file-creation-attempt>\n<path>hello.py</path>\n<content>\nprint('Hello from ASI Code')\na, b = 0, 1\nfor i in range(10):\n    print(a, end=' ')\n    a, b = b, a + b\nprint()\n</content>\n</file-creation-attempt>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "write_file");
        assert!(
            calls[0].args.contains("path=\"hello.py\""),
            "args should carry the path: {}",
            calls[0].args
        );
        assert!(
            calls[0].args.contains("Hello from ASI Code"),
            "args should carry the content body: {}",
            calls[0].args
        );
    }

    // Regression: DeepSeek v4 Pro emits `<tool_calls> <tool_call name="X">
    // Regression: `< /toolcall >\nTOOL ARGS\n< /toolcall >` triplets,
    // emitted by DeepSeek v4 Pro on some sessions. Without normalization
    // the parser sees `/toolcall ` inside the marker and dispatches `>`
    // as the tool name, hitting `Unknown tool: >` repeatedly.
    #[test]
    fn promotes_pseudo_xml_toolcall_markers_with_space_slash() {
        let text = "< /toolcall >\nglob_search \"**/*.html\"\n< /toolcall >\nread_file \"hello.py\" 1 50\n< /toolcall >";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2, "expected 2 calls, got {:?}", calls);
        assert_eq!(calls[0].name, "glob_search");
        assert!(calls[0].args.contains("**/*.html"));
        assert_eq!(calls[1].name, "read_file");
        assert!(calls[1].args.contains("hello.py"));
    }

    #[test]
    fn pseudo_xml_marker_does_not_swallow_prose_lines() {
        let text = "< /toolcall >\nI was about to call a tool but changed my mind.\n< /toolcall >";
        let calls = extract_tool_calls(text);
        assert!(calls.is_empty(), "expected zero calls, got {:?}", calls);
    }

    #[test]
    fn pseudo_xml_marker_handles_underscored_tool_call_form() {
        let text = "<tool_call>\nbash \"ls -la\"\n</tool_call>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].args.contains("ls -la"));
    }

    // Regression: DeepSeek v4 Pro single-line inline form
    //   <**Calling:** `glob_search` `{ "pattern": "**/*" }`>
    //   </**Calling**>
    // Without normalization the runtime never sees a tool call and the
    // user gets "agent searched but never produced an answer".
    #[test]
    fn promotes_inline_angle_calling_block_to_toolcall() {
        let text = "<**Calling:** `glob_search` `{ \"pattern\": \"**/*\" }`>\n</**Calling**>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1, "expected 1 call, got {:?}", calls);
        assert_eq!(calls[0].name, "glob_search");
        assert!(
            calls[0].args.contains("**/*"),
            "args should carry the JSON: {}",
            calls[0].args
        );
    }

    #[test]
    fn promotes_inline_angle_calling_block_with_complex_json() {
        let text = "<**Calling:** `read_file` `{ \"path\": \"D:\\\\test-cli\\\\config.json\", \"start_line\": \"1\", \"max_lines\": \"50\" }`>\n</**Calling**>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
        assert!(calls[0].args.contains("config.json"));
    }

    #[test]
    fn promotes_inline_angle_calling_bash_with_shell_metacharacters() {
        let text = "<**Calling:** `bash` `{ \"command\": \"python --version 2>&1; pip list 2>&1 | head -20\" }`>\n</**Calling**>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "bash");
        assert!(calls[0].args.contains("python --version"));
    }

    #[test]
    fn ignores_inline_angle_block_with_unknown_tool() {
        let text = "<**Calling:** `not_a_real_tool` `{}`>\n</**Calling**>";
        let calls = extract_tool_calls(text);
        assert!(calls.is_empty());
    }

    #[test]
    fn parses_anthropic_style_tool_calls_plural_wrapper() {
        let text = "<tool_calls>\n<tool_call name=\"web_search\">\n<parameter name=\"query\" string=\"true\">OpenAI GPT-5.5 release 2025</parameter>\n</tool_call>\n</tool_calls>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "web_search");
        assert!(
            calls[0].args.contains("OpenAI GPT-5.5"),
            "args should carry the query: {}",
            calls[0].args
        );
    }

    #[test]
    fn parses_anthropic_style_tool_calls_plural_wrapper_with_multiple_calls() {
        let text = "<tool_calls>\n<tool_call name=\"web_search\">\n<parameter name=\"query\" string=\"true\">first query</parameter>\n</tool_call>\n<tool_call name=\"web_search\">\n<parameter name=\"query\" string=\"true\">second query</parameter>\n</tool_call>\n</tool_calls>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "web_search");
        assert!(calls[0].args.contains("first query"));
        assert_eq!(calls[1].name, "web_search");
        assert!(calls[1].args.contains("second query"));
    }

    // Regression: DeepSeek v4 Pro emits self-closing tool-name-as-tag
    // children inside a `<tool_calls>` plural wrapper:
    //
    //   <tool_calls>
    //     <read_file path="..." start_line="1" max_lines="80"/>
    //     <glob_search pattern="*.py" base_path="..."/>
    //   </tool_calls>
    //
    // Each child uses the canonical tool name as the tag and embeds args as
    // XML attributes. parse_function_calls_block must promote each into a
    // standalone tool call.
    #[test]
    fn parses_tool_calls_wrapper_with_self_closing_tool_tags() {
        let text = "<tool_calls>\n<read_file path=\"D:\\\\test-cli\\\\config.json\" max_lines=\"80\" start_line=\"1\"/>\n<read_file path=\"D:\\\\test-cli\\\\hello.py\" max_lines=\"30\" start_line=\"1\"/>\n<glob_search pattern=\"*.md\" base_path=\"D:\\\\test-cli\"/>\n<glob_search pattern=\"*.py\" base_path=\"D:\\\\test-cli\"/>\n</tool_calls>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 4, "expected 4 calls, got {:?}", calls);
        assert_eq!(calls[0].name, "read_file");
        assert!(calls[0].args.contains("config.json"));
        assert_eq!(calls[1].name, "read_file");
        assert!(calls[1].args.contains("hello.py"));
        assert_eq!(calls[2].name, "glob_search");
        assert!(calls[2].args.contains("*.md"));
        assert_eq!(calls[3].name, "glob_search");
        assert!(calls[3].args.contains("*.py"));
    }

    #[test]
    fn ignores_unknown_self_closing_child_inside_tool_calls_wrapper() {
        // Unknown tool name must NOT be promoted (parse_open_tag rejects it).
        let text = "<tool_calls>\n<some_random_tag attr=\"x\"/>\n<read_file path=\"a.py\"/>\n</tool_calls>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "read_file");
    }

    #[test]
    fn parses_dsml_fenced_multiple_invokes() {
        let text = "<\u{FF5C}DSML\u{FF5C}tool_calls>\n<\u{FF5C}DSML\u{FF5C}invoke name=\"read_file\">\n<\u{FF5C}DSML\u{FF5C}parameter name=\"path\" string=\"true\">requirements.txt</\u{FF5C}DSML\u{FF5C}parameter>\n</\u{FF5C}DSML\u{FF5C}invoke>\n<\u{FF5C}DSML\u{FF5C}invoke name=\"read_file\">\n<\u{FF5C}DSML\u{FF5C}parameter name=\"path\" string=\"true\">src/proxy/__init__.py</\u{FF5C}DSML\u{FF5C}parameter>\n</\u{FF5C}DSML\u{FF5C}invoke>\n</\u{FF5C}DSML\u{FF5C}tool_calls>";
        let calls = extract_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "read_file");
        assert!(calls[0].args.contains("path=\"requirements.txt\""));
        assert_eq!(calls[1].name, "read_file");
        assert!(calls[1].args.contains("src/proxy/__init__.py"));
    }
}
