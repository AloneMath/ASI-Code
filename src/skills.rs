//! User-definable Skills loaded from disk.
//!
//! A Skill is a markdown file with a YAML-style frontmatter header. Skills
//! live in two locations and project-level entries shadow user-level ones
//! with the same fully-qualified name:
//!
//!   - User level:    `~/.asi/skills/<name>.md`
//!   - Project level: `<project>/.asi/skills/<name>.md`
//!
//! At REPL start the registry scans both directories and builds an index by
//! `namespace:name`. Skills can be invoked with either:
//!
//!   `/<name> [args...]`               (when name is unambiguous)
//!   `/<namespace>:<name> [args...]`   (always works)
//!
//! Slash command dispatch in `main.rs` consults the registry **after** the
//! built-in slash table, so a user-supplied skill can never shadow a
//! built-in (we also reject loading skills whose name collides with a
//! builtin during scan).
//!
//! Frontmatter schema (all optional except `name`):
//!
//! ```yaml
//! ---
//! name: refactor-api
//! description: Refactor REST endpoints with consistent error handling
//! namespace: api
//! allowed_tools: [bash, edit_file, read_file]
//! model: sonnet
//! ---
//! ```
//!
//! Body supports `{{args}}`, `{{cwd}}`, `{{date}}` substitutions.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const BUILTIN_SLASH_NAMES: &[&str] = &[
    "help",
    "exit",
    "quit",
    "clear",
    "compact",
    "status",
    "cost",
    "changes",
    "checkpoint",
    "save",
    "sessions",
    "theme",
    "setup",
    "project",
    "import",
    "voice",
    "privacy",
    "flags",
    "policy",
    "permissions",
    "runtime-profile",
    "model",
    "provider",
    "speed",
    "profile",
    "think",
    "markdown",
    "auto",
    "workmode",
    "native",
    "run",
    "scan",
    "review",
    "secure",
    "code",
    "work",
    "agent",
    "toolcall",
    "tools",
    "memory",
    "todo",
    "mcp",
    "plugin",
    "hooks",
    "audit",
    "autoresearch",
    "wiki",
    "tokenizer",
    "oauth",
    "api",
    "api-page",
    "index",
    "git",
    "skills",
    "skill",
    "worktree",
    "cron",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub namespace: Option<String>,
    pub description: String,
    pub allowed_tools: Vec<String>,
    pub model: Option<String>,
    pub body: String,
    pub source_path: PathBuf,
    pub source_kind: SkillSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    User,
    Project,
}

impl Skill {
    /// Fully qualified name for index lookups: `namespace:name` or just `name`.
    pub fn fqname(&self) -> String {
        match &self.namespace {
            Some(ns) if !ns.is_empty() => format!("{}:{}", ns, self.name),
            _ => self.name.clone(),
        }
    }

    /// Render the skill body with `{{args}}`, `{{cwd}}`, `{{date}}`
    /// substituted. Unknown `{{tokens}}` are left untouched.
    pub fn render(&self, args: &str, cwd: &Path, today_iso: &str) -> String {
        let cwd_str = cwd.display().to_string();
        self.body
            .replace("{{args}}", args)
            .replace("{{cwd}}", &cwd_str)
            .replace("{{date}}", today_iso)
    }
}

/// Errors surfaced during skill registry scans. Kept as plain strings so
/// they can be rendered by the existing UI layer without a dedicated type.
#[derive(Debug, Clone)]
pub struct SkillLoadError {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct SkillRegistry {
    /// Index keyed by fully qualified name (`namespace:name` or `name`).
    by_fqname: BTreeMap<String, Skill>,
    /// Bare-name index for unambiguous shorthand dispatch (`/name` instead
    /// of `/namespace:name`). Only populated when no other skill in any
    /// namespace shares the same bare name.
    by_bare: BTreeMap<String, String>,
    pub load_errors: Vec<SkillLoadError>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load both user-level and project-level skill directories. Project
    /// entries override user entries when their fqname collides.
    pub fn load(user_dir: Option<&Path>, project_dir: Option<&Path>) -> Self {
        let mut reg = Self::new();
        if let Some(dir) = user_dir {
            reg.scan_dir(dir, SkillSource::User);
        }
        if let Some(dir) = project_dir {
            reg.scan_dir(dir, SkillSource::Project);
        }
        reg.rebuild_bare_index();
        reg
    }

    fn scan_dir(&mut self, dir: &Path, kind: SkillSource) {
        if !dir.exists() {
            return;
        }
        let walker = match fs::read_dir(dir) {
            Ok(w) => w,
            Err(e) => {
                self.load_errors.push(SkillLoadError {
                    path: dir.to_path_buf(),
                    message: format!("read_dir failed: {}", e),
                });
                return;
            }
        };
        for entry in walker.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // One level of subdirectories acts as a namespace (e.g.
                // `~/.asi/skills/api/refactor.md` -> namespace=api).
                let ns = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(str::to_string);
                self.scan_namespace_dir(&path, ns, kind);
                continue;
            }
            self.try_register_file(&path, None, kind);
        }
    }

    fn scan_namespace_dir(&mut self, dir: &Path, namespace: Option<String>, kind: SkillSource) {
        let walker = match fs::read_dir(dir) {
            Ok(w) => w,
            Err(e) => {
                self.load_errors.push(SkillLoadError {
                    path: dir.to_path_buf(),
                    message: format!("read_dir failed: {}", e),
                });
                return;
            }
        };
        for entry in walker.flatten() {
            let path = entry.path();
            if path.is_file() {
                self.try_register_file(&path, namespace.clone(), kind);
            }
        }
    }

    fn try_register_file(
        &mut self,
        path: &Path,
        ns_from_dir: Option<String>,
        kind: SkillSource,
    ) {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase);
        if ext.as_deref() != Some("md") {
            return;
        }
        let raw = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                self.load_errors.push(SkillLoadError {
                    path: path.to_path_buf(),
                    message: format!("read failed: {}", e),
                });
                return;
            }
        };
        match parse_skill_file(&raw, path, ns_from_dir, kind) {
            Ok(skill) => {
                if BUILTIN_SLASH_NAMES.contains(&skill.name.as_str()) {
                    self.load_errors.push(SkillLoadError {
                        path: path.to_path_buf(),
                        message: format!(
                            "skill name '{}' collides with a built-in slash command; rename it",
                            skill.name
                        ),
                    });
                    return;
                }
                self.by_fqname.insert(skill.fqname(), skill);
            }
            Err(message) => {
                self.load_errors.push(SkillLoadError {
                    path: path.to_path_buf(),
                    message,
                });
            }
        }
    }

    fn rebuild_bare_index(&mut self) {
        self.by_bare.clear();
        let mut bare_counts: BTreeMap<String, usize> = BTreeMap::new();
        for skill in self.by_fqname.values() {
            *bare_counts.entry(skill.name.clone()).or_default() += 1;
        }
        for skill in self.by_fqname.values() {
            if bare_counts.get(&skill.name).copied().unwrap_or(0) == 1 {
                self.by_bare.insert(skill.name.clone(), skill.fqname());
            }
        }
    }

    pub fn list(&self) -> Vec<&Skill> {
        self.by_fqname.values().collect()
    }

    pub fn lookup(&self, query: &str) -> Option<&Skill> {
        if let Some(skill) = self.by_fqname.get(query) {
            return Some(skill);
        }
        let fq = self.by_bare.get(query)?;
        self.by_fqname.get(fq)
    }

    /// True if `query` is ambiguous (multiple skills share the bare name and
    /// the user did not qualify with a namespace). Public so the future
    /// scheduler / dispatcher can surface a "did you mean ns:name?" hint;
    /// not yet called from the REPL fast path.
    #[allow(dead_code)]
    pub fn is_ambiguous(&self, query: &str) -> bool {
        if query.contains(':') {
            return false;
        }
        if self.by_bare.contains_key(query) {
            return false;
        }
        self.by_fqname
            .values()
            .filter(|s| s.name == query)
            .count()
            > 1
    }

    /// Try to resolve a slash command line against the registry. Returns
    /// `(skill_clone, args_string)` so the caller can render and dispatch.
    pub fn try_dispatch(&self, line: &str) -> Option<(Skill, String)> {
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix('/')?;
        let mut parts = rest.splitn(2, char::is_whitespace);
        let head = parts.next().unwrap_or("").trim();
        let args = parts.next().unwrap_or("").trim().to_string();
        if head.is_empty() {
            return None;
        }
        let skill = self.lookup(head)?;
        Some((skill.clone(), args))
    }
}

/// Parse a single skill markdown file. Returns `Err(message)` for malformed
/// input. The frontmatter delimiter is `---` on its own line at the very
/// top; a missing frontmatter is treated as the whole file being the body
/// with `name` derived from the filename stem.
fn parse_skill_file(
    raw: &str,
    path: &Path,
    ns_from_dir: Option<String>,
    kind: SkillSource,
) -> Result<Skill, String> {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .ok_or_else(|| "skill file has no readable stem".to_string())?;
    let (front, body) = split_frontmatter(raw);
    let mut name: Option<String> = None;
    let mut namespace: Option<String> = ns_from_dir;
    let mut description = String::new();
    let mut allowed_tools: Vec<String> = Vec::new();
    let mut model: Option<String> = None;

    if let Some(front) = front {
        for (key, value) in parse_frontmatter_kv(&front)? {
            match key.as_str() {
                "name" => name = Some(value),
                "namespace" => {
                    if !value.is_empty() {
                        namespace = Some(value);
                    }
                }
                "description" => description = value,
                "allowed_tools" => allowed_tools = parse_inline_list(&value),
                "model" => {
                    if !value.is_empty() {
                        model = Some(value);
                    }
                }
                _ => {
                    // Unknown frontmatter keys are ignored; this keeps the
                    // schema forward-compatible.
                }
            }
        }
    }

    let name = name
        .filter(|s| !s.is_empty())
        .unwrap_or(stem)
        .to_ascii_lowercase();
    if !is_valid_slash_name(&name) {
        return Err(format!(
            "invalid skill name '{}'; allowed chars are a-z, 0-9, '-', '_'",
            name
        ));
    }
    if let Some(ns) = &namespace {
        if !is_valid_slash_name(ns) {
            return Err(format!(
                "invalid skill namespace '{}'; allowed chars are a-z, 0-9, '-', '_'",
                ns
            ));
        }
    }

    Ok(Skill {
        name,
        namespace,
        description,
        allowed_tools,
        model,
        body: body.to_string(),
        source_path: path.to_path_buf(),
        source_kind: kind,
    })
}

fn split_frontmatter(raw: &str) -> (Option<String>, &str) {
    let trimmed_start = raw.trim_start_matches(['\u{feff}']);
    if !trimmed_start.starts_with("---") {
        return (None, raw);
    }
    let after_first = &trimmed_start[3..];
    // Require the opener line to be exactly `---` (allowing trailing CR/LF).
    let after_first = match after_first.strip_prefix('\r') {
        Some(r) => r,
        None => after_first,
    };
    let after_first = match after_first.strip_prefix('\n') {
        Some(r) => r,
        None => return (None, raw),
    };
    if let Some(end_idx) = find_closing_fence(after_first) {
        let front = &after_first[..end_idx];
        let after_close = &after_first[end_idx..];
        // Skip the closing `---` line plus any trailing newline.
        let body_start = after_close
            .find('\n')
            .map(|n| n + 1)
            .unwrap_or(after_close.len());
        let body = &after_close[body_start..];
        return (Some(front.to_string()), body);
    }
    (None, raw)
}

fn find_closing_fence(after_first: &str) -> Option<usize> {
    let mut idx = 0usize;
    for line in after_first.split_inclusive('\n') {
        let stripped = line.trim_end_matches(['\r', '\n']);
        if stripped == "---" {
            return Some(idx);
        }
        idx += line.len();
    }
    None
}

fn parse_frontmatter_kv(front: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (lineno, raw_line) in front.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| format!("frontmatter line {}: missing ':' in '{}'", lineno + 1, line))?;
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().trim_matches('"').trim_matches('\'').to_string();
        out.push((key, value));
    }
    Ok(out)
}

fn parse_inline_list(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    let stripped = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    stripped
        .split(',')
        .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn is_valid_slash_name(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Default user-level skill directory: `~/.asi/skills`.
pub fn default_user_skill_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    Some(home.join(".asi").join("skills"))
}

/// Default project-level skill directory: `<project>/.asi/skills`.
pub fn default_project_skill_dir(project_root: &Path) -> PathBuf {
    project_root.join(".asi").join("skills")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Lightweight temp dir helper that does not pull a new crate into the
    /// build graph. Cleaned on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("asi-skills-test-{}-{}-{}", pid, nanos, id));
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_file(dir: &Path, rel: &str, body: &str) -> PathBuf {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let raw = "---\nname: refactor-api\ndescription: Refactor endpoints\nallowed_tools: [bash, edit_file]\nmodel: sonnet\n---\nBody content {{args}}\n";
        let path = PathBuf::from("/tmp/refactor-api.md");
        let skill = parse_skill_file(raw, &path, None, SkillSource::User).unwrap();
        assert_eq!(skill.name, "refactor-api");
        assert_eq!(skill.description, "Refactor endpoints");
        assert_eq!(skill.allowed_tools, vec!["bash", "edit_file"]);
        assert_eq!(skill.model.as_deref(), Some("sonnet"));
        assert!(skill.body.contains("Body content"));
    }

    #[test]
    fn missing_frontmatter_falls_back_to_filename_stem() {
        let raw = "Just a body.";
        let path = PathBuf::from("/tmp/cleanup.md");
        let skill = parse_skill_file(raw, &path, None, SkillSource::User).unwrap();
        assert_eq!(skill.name, "cleanup");
        assert_eq!(skill.body.trim(), "Just a body.");
    }

    #[test]
    fn rejects_invalid_skill_name() {
        let raw = "---\nname: bad name with spaces\n---\nbody";
        let path = PathBuf::from("/tmp/x.md");
        let err = parse_skill_file(raw, &path, None, SkillSource::User).unwrap_err();
        assert!(err.contains("invalid skill name"));
    }

    #[test]
    fn project_dir_overrides_user_dir() {
        let user = TempDir::new();
        let project = TempDir::new();
        write_file(
            user.path(),
            "shared.md",
            "---\nname: shared\ndescription: user version\n---\nUSER",
        );
        write_file(
            project.path(),
            "shared.md",
            "---\nname: shared\ndescription: project version\n---\nPROJECT",
        );
        let reg = SkillRegistry::load(Some(user.path()), Some(project.path()));
        let skill = reg.lookup("shared").expect("shared skill");
        assert_eq!(skill.description, "project version");
        assert!(skill.body.contains("PROJECT"));
    }

    #[test]
    fn rejects_skill_with_builtin_slash_name() {
        let dir = TempDir::new();
        write_file(dir.path(), "help.md", "---\nname: help\n---\nbody");
        let reg = SkillRegistry::load(Some(dir.path()), None);
        assert!(reg.lookup("help").is_none());
        assert!(!reg.load_errors.is_empty());
    }

    #[test]
    fn render_substitutes_args_cwd_date() {
        let raw = "---\nname: greet\n---\nHello {{args}} from {{cwd}} on {{date}}.";
        let skill =
            parse_skill_file(raw, &PathBuf::from("/tmp/greet.md"), None, SkillSource::User)
                .unwrap();
        let rendered = skill.render("world", &PathBuf::from("/projects/foo"), "2026-04-25");
        assert!(rendered.contains("Hello world"));
        assert!(rendered.contains("/projects/foo"));
        assert!(rendered.contains("2026-04-25"));
    }

    #[test]
    fn dispatch_resolves_bare_and_qualified_names() {
        let dir = TempDir::new();
        write_file(
            dir.path(),
            "api/refactor.md",
            "---\nname: refactor\n---\nBody",
        );
        let reg = SkillRegistry::load(Some(dir.path()), None);
        let (skill, args) = reg.try_dispatch("/refactor target.py").expect("bare dispatch");
        assert_eq!(skill.name, "refactor");
        assert_eq!(skill.namespace.as_deref(), Some("api"));
        assert_eq!(args, "target.py");

        let (skill2, _) = reg
            .try_dispatch("/api:refactor")
            .expect("qualified dispatch");
        assert_eq!(skill2.name, "refactor");
    }

    #[test]
    fn ambiguous_bare_name_requires_qualification() {
        let dir = TempDir::new();
        write_file(dir.path(), "api/lint.md", "---\nname: lint\n---\nA");
        write_file(dir.path(), "ops/lint.md", "---\nname: lint\n---\nB");
        let reg = SkillRegistry::load(Some(dir.path()), None);
        assert!(reg.lookup("lint").is_none());
        assert!(reg.is_ambiguous("lint"));
        assert!(reg.lookup("api:lint").is_some());
        assert!(reg.lookup("ops:lint").is_some());
    }
}
