use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct InitOptions {
    pub root: PathBuf,
}

pub struct IngestOptions {
    pub root: PathBuf,
    pub source: PathBuf,
    pub title: Option<String>,
    pub no_copy: bool,
}

pub struct QueryOptions {
    pub root: PathBuf,
    pub question: String,
    pub top: usize,
    pub save: bool,
}

pub struct LintOptions {
    pub root: PathBuf,
    pub write_report: bool,
}

#[derive(Debug, Clone)]
struct QueryHit {
    link: String,
    path: PathBuf,
    score: usize,
    snippets: Vec<String>,
}

const RAW_DIR: &str = "raw";
const RAW_SOURCES_DIR: &str = "raw/sources";
const RAW_ASSETS_DIR: &str = "raw/assets";
const WIKI_DIR: &str = "wiki";
const WIKI_SOURCES_DIR: &str = "wiki/sources";
const WIKI_ENTITIES_DIR: &str = "wiki/entities";
const WIKI_CONCEPTS_DIR: &str = "wiki/concepts";
const WIKI_ANALYSES_DIR: &str = "wiki/analyses";
const WIKI_REPORTS_DIR: &str = "wiki/reports";
const SCHEMA_FILE: &str = "AGENTS.md";
const INDEX_FILE: &str = "index.md";
const LOG_FILE: &str = "log.md";

pub fn init(opts: InitOptions) -> Result<String, String> {
    let root = normalize_root(&opts.root)?;
    fs::create_dir_all(&root)
        .map_err(|e| format!("failed to create root {}: {}", root.display(), e))?;

    let mut created_dirs = 0usize;
    for rel in [
        RAW_DIR,
        RAW_SOURCES_DIR,
        RAW_ASSETS_DIR,
        WIKI_DIR,
        WIKI_SOURCES_DIR,
        WIKI_ENTITIES_DIR,
        WIKI_CONCEPTS_DIR,
        WIKI_ANALYSES_DIR,
        WIKI_REPORTS_DIR,
    ] {
        let dir = root.join(rel);
        if !dir.exists() {
            fs::create_dir_all(&dir)
                .map_err(|e| format!("failed to create {}: {}", dir.display(), e))?;
            created_dirs += 1;
        }
    }

    let mut created_files = 0usize;
    if ensure_file_if_missing(&root.join(SCHEMA_FILE), schema_template())? {
        created_files += 1;
    }
    if ensure_file_if_missing(&root.join(INDEX_FILE), index_template())? {
        created_files += 1;
    }
    if ensure_file_if_missing(&root.join(LOG_FILE), log_template())? {
        created_files += 1;
    }
    if ensure_file_if_missing(
        &root.join(WIKI_DIR).join("overview.md"),
        overview_template(),
    )? {
        created_files += 1;
    }

    let log_details = vec![
        format!("- root=`{}`", root.display()),
        format!("- created_dirs={}", created_dirs),
        format!("- created_files={}", created_files),
    ];
    append_log_entry(&root, &format!("init | {}", root.display()), &log_details)?;

    Ok(format!(
        "wiki_init=ok\nroot={}\ncreated_dirs={}\ncreated_files={}\nnext=asi wiki ingest --root \"{}\" --source <path>",
        root.display(),
        created_dirs,
        created_files,
        root.display()
    ))
}

pub fn ingest(opts: IngestOptions) -> Result<String, String> {
    let root = normalize_root(&opts.root)?;
    ensure_layout(&root)?;

    let source = normalize_input_path(&opts.source)?;
    if !source.exists() {
        return Err(format!("source not found: {}", source.display()));
    }
    if !source.is_file() {
        return Err(format!("source is not a file: {}", source.display()));
    }

    let now = now_unix_secs();
    let title = opts
        .title
        .clone()
        .unwrap_or_else(|| infer_title_from_path(&source));

    let mut source_record_path = source.clone();
    let mut copied = false;
    if !opts.no_copy {
        let raw_dir = root.join(RAW_SOURCES_DIR);
        let file_name = source
            .file_name()
            .ok_or_else(|| format!("invalid source path: {}", source.display()))?;
        let dest = unique_path_with_suffix(&raw_dir.join(file_name));
        fs::copy(&source, &dest)
            .map_err(|e| format!("failed to copy source to {}: {}", dest.display(), e))?;
        source_record_path = dest;
        copied = true;
    }

    let slug_base = slugify(&title);
    let slug = unique_page_slug(&root.join(WIKI_SOURCES_DIR), &slug_base);
    let source_page = root.join(WIKI_SOURCES_DIR).join(format!("{}.md", slug));
    let source_record_display = display_relative_or_absolute(&source_record_path, &root);
    let source_abs_display = source.display();

    let page = format!(
        "# Source: {}\n\n- title: `{}`\n- ingested_at_unix: `{}`\n- source_original: `{}`\n- source_record: `{}`\n- status: `needs_synthesis`\n\n## Summary\n- TODO: summarize this source in 5-10 bullet points.\n\n## Key Facts\n- TODO\n\n## Contradictions / Open Questions\n- TODO\n\n## Links\n- [[overview]]\n",
        title,
        escape_backticks(&title),
        now,
        escape_backticks(&source_abs_display.to_string()),
        escape_backticks(&source_record_display),
    );
    fs::write(&source_page, page)
        .map_err(|e| format!("failed to write {}: {}", source_page.display(), e))?;

    let link = format!("sources/{}", slug);
    let entry = format!("- [[{}]] - {} (ingest_unix={})", link, title, now);
    let _ = insert_line_under_section(&root.join(INDEX_FILE), "## Sources", &entry)?;

    let log_details = vec![
        format!("- source_original=`{}`", source.display()),
        format!("- source_record=`{}`", source_record_path.display()),
        format!("- copied={}", copied),
        format!("- page=`{}`", source_page.display()),
    ];
    append_log_entry(&root, &format!("ingest | {}", title), &log_details)?;

    Ok(format!(
        "wiki_ingest=ok\nroot={}\nsource_original={}\nsource_record={}\ncopied={}\npage={}\nindex_entry=[[{}]]",
        root.display(),
        source.display(),
        source_record_path.display(),
        copied,
        source_page.display(),
        link
    ))
}

pub fn query(opts: QueryOptions) -> Result<String, String> {
    let root = normalize_root(&opts.root)?;
    ensure_layout(&root)?;

    let question = opts.question.trim();
    if question.is_empty() {
        return Err("question cannot be empty".to_string());
    }

    let top = opts.top.clamp(1, 20);
    let terms = tokenize_query(question);
    let wiki_root = root.join(WIKI_DIR);

    let mut pages = Vec::new();
    collect_markdown_files(&wiki_root, &mut pages)?;
    if pages.is_empty() {
        return Ok(format!(
            "wiki_query=ok\nquestion={}\nterms={}\nhits=0\ninfo=no wiki pages found under {}",
            question,
            terms.join(","),
            wiki_root.display()
        ));
    }

    let mut hits = Vec::new();
    for page in pages {
        let content = match fs::read_to_string(&page) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let link = page_to_wikilink(&root, &page)?;
        let score = score_text(&content, &link, &terms);
        if score == 0 {
            continue;
        }
        let snippets = pick_snippets(&content, &terms, 2, 180);
        hits.push(QueryHit {
            link,
            path: page,
            score,
            snippets,
        });
    }

    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.link.cmp(&b.link)));
    let selected: Vec<QueryHit> = hits.into_iter().take(top).collect();

    let mut out = Vec::new();
    out.push("wiki_query=ok".to_string());
    out.push(format!("root={}", root.display()));
    out.push(format!("question={}", question));
    out.push(format!("terms={}", terms.join(",")));
    out.push(format!("hits={}", selected.len()));

    for (idx, hit) in selected.iter().enumerate() {
        out.push(format!(
            "{}. score={} page={} path={}",
            idx + 1,
            hit.score,
            hit.link,
            hit.path.display()
        ));
        for s in &hit.snippets {
            out.push(format!("   - {}", s));
        }
    }

    if opts.save {
        let ts = now_unix_secs();
        let name = format!("query-{}.md", ts);
        let path = root.join(WIKI_ANALYSES_DIR).join(&name);
        let link = format!("analyses/query-{}", ts);
        let mut body = String::new();
        body.push_str(&format!("# Query: {}\n\n", question));
        body.push_str(&format!("- created_at_unix: `{}`\n", ts));
        body.push_str("- mode: `lexical-search-draft`\n\n");
        body.push_str("## Candidate Pages\n");
        if selected.is_empty() {
            body.push_str("- none\n");
        } else {
            for hit in &selected {
                body.push_str(&format!("- [[{}]] (score={})\n", hit.link, hit.score));
            }
        }
        body.push_str("\n## Draft Answer\n");
        body.push_str("- TODO: synthesize the candidate pages into a final answer.\n");
        body.push_str("- TODO: cite exact sections and reconcile contradictions.\n");

        fs::write(&path, body)
            .map_err(|e| format!("failed to write query page {}: {}", path.display(), e))?;

        let idx_entry = format!(
            "- [[{}]] - Query: {} (created_unix={})",
            link,
            clip_chars(question, 80),
            ts
        );
        let _ = insert_line_under_section(&root.join(INDEX_FILE), "## Analyses", &idx_entry)?;

        let details = vec![
            format!("- question=`{}`", escape_backticks(question)),
            format!("- hits={}", selected.len()),
            format!("- page=`{}`", path.display()),
        ];
        append_log_entry(
            &root,
            &format!("query | {}", clip_chars(question, 80)),
            &details,
        )?;

        out.push(format!("saved_page={}", path.display()));
        out.push(format!("saved_link=[[{}]]", link));
    }

    Ok(out.join("\n"))
}

pub fn lint(opts: LintOptions) -> Result<String, String> {
    let root = normalize_root(&opts.root)?;
    ensure_layout(&root)?;

    let wiki_root = root.join(WIKI_DIR);
    let mut pages = Vec::new();
    collect_markdown_files(&wiki_root, &mut pages)?;

    let mut page_links = BTreeSet::new();
    let mut page_contents = BTreeMap::new();
    for page in &pages {
        let link = page_to_wikilink(&root, page)?;
        page_links.insert(link.clone());
        let content = fs::read_to_string(page)
            .map_err(|e| format!("failed to read {}: {}", page.display(), e))?;
        page_contents.insert(link, content);
    }

    let mut inbound: BTreeMap<String, usize> = BTreeMap::new();
    for p in &page_links {
        inbound.insert(p.clone(), 0);
    }

    let mut broken_links = Vec::new();
    for (from, content) in &page_contents {
        for target in extract_wikilinks(content) {
            if page_links.contains(&target) {
                if let Some(v) = inbound.get_mut(&target) {
                    *v += 1;
                }
            } else {
                broken_links.push(format!("{} -> {}", from, target));
            }
        }
    }

    let mut orphans = Vec::new();
    for (page, count) in &inbound {
        if *count == 0 {
            orphans.push(page.clone());
        }
    }

    let mut todo_pages = Vec::new();
    for (page, content) in &page_contents {
        if content.contains("TODO") || content.contains("todo") {
            todo_pages.push(page.clone());
        }
    }

    let index_text = fs::read_to_string(root.join(INDEX_FILE)).unwrap_or_default();
    let indexed: BTreeSet<String> = extract_wikilinks(&index_text).into_iter().collect();

    let mut missing_in_index = Vec::new();
    for page in &page_links {
        if !indexed.contains(page) {
            missing_in_index.push(page.clone());
        }
    }

    let mut out = Vec::new();
    out.push("wiki_lint=ok".to_string());
    out.push(format!("root={}", root.display()));
    out.push(format!("pages_total={}", page_links.len()));
    out.push(format!("broken_links={}", broken_links.len()));
    out.push(format!("orphans={}", orphans.len()));
    out.push(format!("todo_pages={}", todo_pages.len()));
    out.push(format!("missing_in_index={}", missing_in_index.len()));

    if !broken_links.is_empty() {
        out.push("broken_links_samples:".to_string());
        for item in broken_links.iter().take(10) {
            out.push(format!("- {}", item));
        }
    }
    if !orphans.is_empty() {
        out.push("orphans_samples:".to_string());
        for item in orphans.iter().take(10) {
            out.push(format!("- {}", item));
        }
    }
    if !missing_in_index.is_empty() {
        out.push("missing_in_index_samples:".to_string());
        for item in missing_in_index.iter().take(10) {
            out.push(format!("- {}", item));
        }
    }

    if opts.write_report {
        let ts = now_unix_secs();
        let report_path = root.join(WIKI_REPORTS_DIR).join(format!("lint-{}.md", ts));
        let mut md = String::new();
        md.push_str("# Wiki Lint Report\n\n");
        md.push_str(&format!("- generated_at_unix: `{}`\n", ts));
        md.push_str(&format!("- root: `{}`\n", root.display()));
        md.push_str(&format!("- pages_total: `{}`\n", page_links.len()));
        md.push_str(&format!("- broken_links: `{}`\n", broken_links.len()));
        md.push_str(&format!("- orphans: `{}`\n", orphans.len()));
        md.push_str(&format!("- todo_pages: `{}`\n", todo_pages.len()));
        md.push_str(&format!(
            "- missing_in_index: `{}`\n",
            missing_in_index.len()
        ));

        if !broken_links.is_empty() {
            md.push_str("\n## Broken Links\n");
            for item in &broken_links {
                md.push_str(&format!("- {}\n", item));
            }
        }
        if !orphans.is_empty() {
            md.push_str("\n## Orphan Pages\n");
            for item in &orphans {
                md.push_str(&format!("- {}\n", item));
            }
        }
        if !missing_in_index.is_empty() {
            md.push_str("\n## Missing in Index\n");
            for item in &missing_in_index {
                md.push_str(&format!("- {}\n", item));
            }
        }

        fs::write(&report_path, md).map_err(|e| {
            format!(
                "failed to write lint report {}: {}",
                report_path.display(),
                e
            )
        })?;
        out.push(format!("report={}", report_path.display()));

        let details = vec![
            format!("- pages_total={}", page_links.len()),
            format!("- broken_links={}", broken_links.len()),
            format!("- orphans={}", orphans.len()),
            format!("- missing_in_index={}", missing_in_index.len()),
            format!("- report=`{}`", report_path.display()),
        ];
        append_log_entry(
            &root,
            &format!("lint | pages={}", page_links.len()),
            &details,
        )?;
    }

    Ok(out.join("\n"))
}

fn normalize_root(root: &Path) -> Result<PathBuf, String> {
    let p = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("failed to get current dir: {}", e))?
            .join(root)
    };
    Ok(p)
}

fn normalize_input_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .map_err(|e| format!("failed to get current dir: {}", e))?
            .join(path))
    }
}

fn ensure_layout(root: &Path) -> Result<(), String> {
    for rel in [RAW_DIR, RAW_SOURCES_DIR, WIKI_DIR, WIKI_SOURCES_DIR] {
        let p = root.join(rel);
        if !p.exists() {
            return Err(format!(
                "wiki layout missing: {} (run `asi wiki init --root \"{}\"` first)",
                p.display(),
                root.display()
            ));
        }
    }
    for rel in [SCHEMA_FILE, INDEX_FILE, LOG_FILE] {
        let p = root.join(rel);
        if !p.exists() {
            return Err(format!(
                "wiki file missing: {} (run `asi wiki init --root \"{}\"` first)",
                p.display(),
                root.display()
            ));
        }
    }
    Ok(())
}

fn ensure_file_if_missing(path: &Path, content: String) -> Result<bool, String> {
    if path.exists() {
        return Ok(false);
    }
    fs::write(path, content).map_err(|e| format!("failed to write {}: {}", path.display(), e))?;
    Ok(true)
}

fn append_log_entry(root: &Path, title: &str, details: &[String]) -> Result<(), String> {
    let path = root.join(LOG_FILE);
    let mut current = fs::read_to_string(&path).unwrap_or_default();
    if !current.ends_with('\n') {
        current.push('\n');
    }
    let now = now_unix_secs();
    current.push_str(&format!("\n## [unix:{}] {}\n", now, title));
    for d in details {
        current.push_str(d);
        current.push('\n');
    }
    fs::write(&path, current).map_err(|e| format!("failed to append log {}: {}", path.display(), e))
}

fn insert_line_under_section(path: &Path, section: &str, line: &str) -> Result<bool, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;

    if content.lines().any(|l| l.trim() == line.trim()) {
        return Ok(false);
    }

    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    if lines.is_empty() {
        lines.push("# Index".to_string());
    }

    let section_idx = lines.iter().position(|l| l.trim() == section);
    match section_idx {
        Some(start) => {
            let mut insert_at = lines.len();
            for (idx, l) in lines.iter().enumerate().skip(start + 1) {
                if l.trim_start().starts_with("## ") {
                    insert_at = idx;
                    break;
                }
            }
            lines.insert(insert_at, line.to_string());
        }
        None => {
            if !lines.last().map(|v| v.is_empty()).unwrap_or(false) {
                lines.push(String::new());
            }
            lines.push(section.to_string());
            lines.push(line.to_string());
        }
    }

    let mut out = lines.join("\n");
    out.push('\n');
    fs::write(path, out).map_err(|e| format!("failed to write {}: {}", path.display(), e))?;
    Ok(true)
}

fn collect_markdown_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in
        fs::read_dir(dir).map_err(|e| format!("failed to list {}: {}", dir.display(), e))?
    {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_markdown_files(&path, out)?;
        } else if path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case("md"))
            .unwrap_or(false)
        {
            out.push(path);
        }
    }
    Ok(())
}

fn page_to_wikilink(root: &Path, page: &Path) -> Result<String, String> {
    let wiki_root = root.join(WIKI_DIR);
    let rel = page
        .strip_prefix(&wiki_root)
        .map_err(|_| format!("page is outside wiki root: {}", page.display()))?;
    let mut s = rel.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = s.strip_suffix(".md") {
        s = stripped.to_string();
    }
    Ok(s)
}

fn extract_wikilinks(content: &str) -> Vec<String> {
    let re = Regex::new(r"\[\[([^\]]+)\]\]").expect("valid wikilink regex");
    let mut out = Vec::new();
    for cap in re.captures_iter(content) {
        let raw = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
        let mut item = raw.split('|').next().unwrap_or_default().trim().to_string();
        if item.is_empty() {
            continue;
        }
        if let Some((left, _)) = item.split_once('#') {
            item = left.trim().to_string();
        }
        item = item.replace('\\', "/").trim_matches('/').to_string();
        if let Some(stripped) = item.strip_suffix(".md") {
            item = stripped.to_string();
        }
        if item.is_empty() {
            continue;
        }
        out.push(item);
    }
    out
}

fn tokenize_query(input: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for raw in input
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let token = raw.to_lowercase();
        if token.len() >= 2 {
            terms.push(token);
        }
    }
    if terms.is_empty() {
        terms.push(input.to_lowercase());
    }
    terms.sort();
    terms.dedup();
    terms
}

fn score_text(content: &str, link: &str, terms: &[String]) -> usize {
    let text = content.to_lowercase();
    let link_lc = link.to_lowercase();
    let mut score = 0usize;
    for term in terms {
        if term.is_empty() {
            continue;
        }
        score += text.match_indices(term).count();
        if link_lc.contains(term) {
            score += 2;
        }
    }
    score
}

fn pick_snippets(
    content: &str,
    terms: &[String],
    max_snippets: usize,
    max_len: usize,
) -> Vec<String> {
    let mut out = Vec::new();
    for line in content.lines() {
        let line_trim = line.trim();
        if line_trim.is_empty() {
            continue;
        }
        let line_lc = line_trim.to_lowercase();
        if terms.iter().any(|t| !t.is_empty() && line_lc.contains(t)) {
            out.push(clip_chars(line_trim, max_len));
            if out.len() >= max_snippets {
                break;
            }
        }
    }
    out
}

fn unique_page_slug(dir: &Path, base: &str) -> String {
    let mut idx = 0usize;
    loop {
        let slug = if idx == 0 {
            base.to_string()
        } else {
            format!("{}-{}", base, idx + 1)
        };
        let path = dir.join(format!("{}.md", slug));
        if !path.exists() {
            return slug;
        }
        idx += 1;
    }
}

fn unique_path_with_suffix(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));

    for i in 2..10_000 {
        let name = if ext.is_empty() {
            format!("{}-{}", stem, i)
        } else {
            format!("{}-{}.{}", stem, i, ext)
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

fn infer_title_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.replace('_', " ").replace('-', " "))
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "untitled source".to_string())
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if ch.is_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        format!("source-{}", now_unix_secs())
    } else {
        trimmed
    }
}

fn display_relative_or_absolute(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => path.display().to_string(),
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn clip_chars(input: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in input.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn escape_backticks(input: &str) -> String {
    input.replace('`', "'")
}

fn schema_template() -> String {
    r#"# AGENTS.md

This file defines wiki maintenance behavior for LLM agents.

## Goals
- Keep the wiki updated as a persistent knowledge artifact.
- Prefer editing existing pages over creating duplicates.
- Preserve source traceability to raw files.

## Ingest Workflow
1. Read source from `raw/sources/`.
2. Update the source page under `wiki/sources/`.
3. Propagate facts into concept/entity pages.
4. Update `index.md` and append an entry in `log.md`.

## Query Workflow
1. Start from `index.md` to locate relevant pages.
2. Read candidate pages and answer with citations.
3. If answer is reusable, save into `wiki/analyses/` and index it.

## Lint Workflow
- Detect broken wikilinks, orphan pages, stale TODO placeholders, and missing index coverage.
- Propose concrete fix actions.
"#
    .to_string()
}

fn index_template() -> String {
    "# Wiki Index\n\n## Overview\n- [[overview]] - Global synthesis page.\n\n## Sources\n\n## Entities\n\n## Concepts\n\n## Analyses\n".to_string()
}

fn log_template() -> String {
    "# Wiki Log\n\nAppend-only operational log for init, ingest, query, and lint actions.\n"
        .to_string()
}

fn overview_template() -> String {
    "# Overview\n\n## Current Thesis\n- TODO\n\n## Important Entities\n- TODO\n\n## Important Concepts\n- TODO\n\n## Open Questions\n- TODO\n".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_root(prefix: &str) -> PathBuf {
        let ts = now_unix_secs();
        std::env::temp_dir().join(format!("{}_{}", prefix, ts))
    }

    #[test]
    fn init_creates_layout_files() {
        let root = test_root("asi_wiki_init_test");
        let msg = init(InitOptions { root: root.clone() }).unwrap();
        assert!(msg.contains("wiki_init=ok"));
        assert!(root.join(SCHEMA_FILE).exists());
        assert!(root.join(INDEX_FILE).exists());
        assert!(root.join(LOG_FILE).exists());
        assert!(root.join(WIKI_DIR).join("overview.md").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ingest_updates_sources_and_index() {
        let root = test_root("asi_wiki_ingest_test");
        init(InitOptions { root: root.clone() }).unwrap();

        let src = root.join("sample.txt");
        fs::write(&src, "alpha beta gamma").unwrap();

        let out = ingest(IngestOptions {
            root: root.clone(),
            source: src,
            title: Some("Alpha Source".to_string()),
            no_copy: false,
        })
        .unwrap();

        assert!(out.contains("wiki_ingest=ok"));
        let index = fs::read_to_string(root.join(INDEX_FILE)).unwrap();
        assert!(index.contains("[[sources/alpha-source"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn lint_detects_broken_links() {
        let root = test_root("asi_wiki_lint_test");
        init(InitOptions { root: root.clone() }).unwrap();

        let page = root.join(WIKI_CONCEPTS_DIR).join("broken.md");
        fs::write(&page, "# Broken\n\nSee [[missing-page]].\n").unwrap();

        let report = lint(LintOptions {
            root: root.clone(),
            write_report: false,
        })
        .unwrap();

        assert!(report.contains("broken_links=1"));
        let _ = fs::remove_dir_all(root);
    }
}
