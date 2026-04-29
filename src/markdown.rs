use regex::Regex;

pub fn render_markdown_ansi(text: &str) -> String {
    let mut out = text.to_string();

    let heading_re =
        Regex::new(r"(?m)^#{1,6}\s+(.+)$").unwrap_or_else(|_| Regex::new("$").expect("regex"));
    out = heading_re
        .replace_all(&out, "\x1b[1;36m$1\x1b[0m")
        .to_string();

    let bold_re = Regex::new(r"\*\*(.+?)\*\*").unwrap_or_else(|_| Regex::new("$").expect("regex"));
    out = bold_re.replace_all(&out, "\x1b[1m$1\x1b[0m").to_string();

    let code_re = Regex::new(r"`([^`]+)`").unwrap_or_else(|_| Regex::new("$").expect("regex"));
    out = code_re.replace_all(&out, "\x1b[33m$1\x1b[0m").to_string();

    out
}
