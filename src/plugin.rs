use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRecord {
    pub name: String,
    pub path: String,
    pub enabled: bool,
    pub trusted: bool,
    #[serde(default)]
    pub trust_policy: String,
    #[serde(default)]
    pub trust_hash: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    pub config: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct PluginHookInvocation {
    pub plugin_name: String,
    pub event: String,
    pub script: String,
    pub timeout_secs: Option<u64>,
    pub json_protocol: Option<bool>,
    pub tool_prefix: Option<String>,
    pub permission_mode: Option<String>,
    pub failure_policy: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PluginManifest {
    #[serde(default)]
    hooks: PluginManifestHooks,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PluginManifestHooks {
    #[serde(default, alias = "preToolUse")]
    pre_tool_use: Option<String>,
    #[serde(default, alias = "permissionRequest")]
    permission_request: Option<String>,
    #[serde(default, alias = "postToolUse")]
    post_tool_use: Option<String>,
    #[serde(default, alias = "sessionStart")]
    session_start: Option<String>,
    #[serde(default, alias = "userPromptSubmit")]
    user_prompt_submit: Option<String>,
    #[serde(default)]
    stop: Option<String>,
    #[serde(default, alias = "subagentStop")]
    subagent_stop: Option<String>,
    #[serde(default, alias = "preCompact")]
    pre_compact: Option<String>,
    #[serde(default, alias = "postCompact")]
    post_compact: Option<String>,
    #[serde(default)]
    handlers: Vec<PluginManifestHookHandler>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct PluginManifestHookHandler {
    event: Option<String>,
    script: Option<String>,
    timeout_secs: Option<u64>,
    json_protocol: Option<bool>,
    tool_prefix: Option<String>,
    permission_mode: Option<String>,
    failure_policy: Option<String>,
}

fn state_path() -> PathBuf {
    if let Some(path) = std::env::var("ASI_PLUGIN_STATE_PATH")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        return PathBuf::from(path);
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".asi_plugins.json")
}

#[cfg(test)]
pub(crate) fn plugin_state_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn load_state() -> Vec<PluginRecord> {
    let path = state_path();
    let text = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_state(items: &[PluginRecord]) -> Result<(), String> {
    let path = state_path();
    let body = serde_json::to_string_pretty(items).map_err(|e| e.to_string())?;
    fs::write(path, body).map_err(|e| e.to_string())
}

fn normalize_plugin_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("plugin name cannot be empty".to_string());
    }
    if n.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(n.to_string())
    } else {
        Err("plugin name must use [a-zA-Z0-9_-]".to_string())
    }
}

fn normalize_plugin_path(path: &str) -> Result<String, String> {
    let p = path.trim();
    if p.is_empty() {
        return Err("plugin path cannot be empty".to_string());
    }
    let candidate = PathBuf::from(p);
    let abs = if candidate.is_absolute() {
        candidate
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(candidate)
    };
    if !abs.exists() {
        return Err(format!("plugin path not found: {}", abs.display()));
    }
    if !abs.is_dir() {
        return Err(format!("plugin path must be a directory: {}", abs.display()));
    }
    let manifest = abs.join(".codex-plugin").join("plugin.json");
    if !manifest.exists() {
        return Err(format!(
            "missing plugin manifest: {}",
            manifest.display()
        ));
    }
    Ok(abs.to_string_lossy().to_string())
}

fn find_plugin_mut<'a>(items: &'a mut [PluginRecord], name: &str) -> Option<&'a mut PluginRecord> {
    items.iter_mut().find(|x| x.name == name)
}

fn normalize_optional_text(raw: Option<&str>) -> Option<String> {
    raw.map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn normalize_signature(raw: &str) -> Result<String, String> {
    let v = raw.trim();
    if v.is_empty() {
        return Err("signature cannot be empty".to_string());
    }
    if !v.starts_with("sha256:") && !v.starts_with("sig:") {
        return Err("signature must start with sha256: or sig:".to_string());
    }
    Ok(v.to_string())
}

fn normalize_source(raw: &str) -> Result<String, String> {
    let v = raw.trim();
    if v.is_empty() {
        return Err("source cannot be empty".to_string());
    }
    if v.starts_with("https://")
        || v.starts_with("http://")
        || v.starts_with("git+")
        || v.starts_with("file://")
        || v.starts_with("local://")
    {
        return Ok(v.to_string());
    }
    Err("source must start with https://, http://, git+, file://, or local://".to_string())
}

fn validate_plugin_publish_meta(item: &PluginRecord) -> Result<(), String> {
    if let Some(version) = item.version.as_ref() {
        if version.trim().is_empty() {
            return Err("version cannot be empty".to_string());
        }
    }
    if let Some(source) = item.source.as_ref() {
        normalize_source(source)?;
    }
    if let Some(signature) = item.signature.as_ref() {
        normalize_signature(signature)?;
    }
    Ok(())
}

pub fn list_plugins() -> Vec<PluginRecord> {
    load_state()
}

pub fn get_plugin(name: &str) -> Option<PluginRecord> {
    load_state().into_iter().find(|x| x.name == name)
}

pub fn add_plugin(name: &str, path: &str) -> Result<PluginRecord, String> {
    let mut state = load_state();
    let name = normalize_plugin_name(name)?;
    if state.iter().any(|x| x.name == name) {
        return Err(format!("plugin `{}` already exists", name));
    }
    let path = normalize_plugin_path(path)?;
    let rec = PluginRecord {
        name,
        path,
        enabled: true,
        trusted: false,
        trust_policy: "manual".to_string(),
        trust_hash: None,
        source: None,
        version: None,
        signature: None,
        config: HashMap::new(),
    };
    state.push(rec.clone());
    save_state(&state)?;
    Ok(rec)
}

pub fn remove_plugin(name: &str) -> Result<bool, String> {
    let mut state = load_state();
    let before = state.len();
    state.retain(|x| x.name != name);
    let changed = before != state.len();
    if changed {
        save_state(&state)?;
    }
    Ok(changed)
}

pub fn set_plugin_enabled(name: &str, enabled: bool) -> Result<(), String> {
    let mut state = load_state();
    let Some(item) = find_plugin_mut(&mut state, name) else {
        return Err(format!("plugin `{}` not found", name));
    };
    item.enabled = enabled;
    save_state(&state)
}

pub fn set_plugin_trust_manual(name: &str, trusted: bool) -> Result<(), String> {
    let mut state = load_state();
    let Some(item) = find_plugin_mut(&mut state, name) else {
        return Err(format!("plugin `{}` not found", name));
    };
    item.trust_policy = "manual".to_string();
    item.trusted = trusted;
    item.trust_hash = None;
    validate_plugin_publish_meta(item)?;
    save_state(&state)
}

pub fn set_plugin_trust_hash(name: &str, expected_hash: Option<&str>) -> Result<String, String> {
    let mut state = load_state();
    let Some(item) = find_plugin_mut(&mut state, name) else {
        return Err(format!("plugin `{}` not found", name));
    };
    let root_path = PathBuf::from(&item.path);
    let hash = hash_directory_sha256(&root_path)?;
    if let Some(expected) = expected_hash {
        let normalized = normalize_hash(expected)?;
        if normalized != hash {
            return Err(format!(
                "plugin hash mismatch expected={} actual={}",
                normalized, hash
            ));
        }
    }
    item.trust_policy = "hash".to_string();
    item.trusted = true;
    item.trust_hash = Some(hash.clone());
    validate_plugin_publish_meta(item)?;
    save_state(&state)?;
    Ok(hash)
}

pub fn verify_plugin_trust(name: &str) -> Result<bool, String> {
    let mut state = load_state();
    let Some(item) = find_plugin_mut(&mut state, name) else {
        return Err(format!("plugin `{}` not found", name));
    };
    if !item.enabled {
        return Ok(false);
    }
    if item.trust_policy == "hash" {
        let Some(stored_hash) = item.trust_hash.clone() else {
            item.trusted = false;
            let _ = save_state(&state);
            return Ok(false);
        };
        let root_path = PathBuf::from(&item.path);
        let now_hash = hash_directory_sha256(&root_path)?;
        let ok = stored_hash.eq_ignore_ascii_case(&now_hash);
        item.trusted = ok;
        let _ = validate_plugin_publish_meta(item);
        let _ = save_state(&state);
        return Ok(ok);
    }
    validate_plugin_publish_meta(item)?;
    Ok(item.trusted)
}

pub fn verify_enabled_plugins() -> Vec<PluginRecord> {
    let mut state = load_state();
    let mut changed = false;
    for item in state.iter_mut() {
        if !item.enabled {
            continue;
        }
        if item.trust_policy != "hash" {
            let _ = validate_plugin_publish_meta(item);
            continue;
        }
        let Some(stored_hash) = item.trust_hash.clone() else {
            if item.trusted {
                item.trusted = false;
                changed = true;
            }
            continue;
        };
        let root_path = PathBuf::from(&item.path);
        let ok = hash_directory_sha256(&root_path)
            .map(|h| h.eq_ignore_ascii_case(&stored_hash))
            .unwrap_or(false);
        if item.trusted != ok {
            item.trusted = ok;
            changed = true;
        }
        let _ = validate_plugin_publish_meta(item);
    }
    if changed {
        let _ = save_state(&state);
    }
    state
        .into_iter()
        .filter(|x| x.enabled && x.trusted)
        .collect::<Vec<_>>()
}

fn normalize_hash(raw: &str) -> Result<String, String> {
    let v = raw.trim().to_ascii_lowercase();
    if v.len() != 64 || !v.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("hash must be 64-char hex sha256".to_string());
    }
    Ok(v)
}

fn should_skip_hash_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|name| {
            matches!(
                name,
                ".git"
                    | ".svn"
                    | ".hg"
                    | ".next"
                    | "node_modules"
                    | "__pycache__"
                    | ".pytest_cache"
                    | ".cache"
                    | ".DS_Store"
            )
        })
        .unwrap_or(false)
}

fn collect_files_for_hash(root: &Path, current: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in fs::read_dir(current).map_err(|e| format!("failed to read {}: {}", current.display(), e))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if should_skip_hash_path(&path) {
            continue;
        }
        if path.is_dir() {
            dirs.push(path);
        } else if path.is_file() {
            files.push(path);
        }
    }

    dirs.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    files.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

    for file in files {
        let rel = file
            .strip_prefix(root)
            .map_err(|e| format!("failed to relativize path {}: {}", file.display(), e))?
            .to_path_buf();
        out.push(rel);
    }
    for dir in dirs {
        collect_files_for_hash(root, &dir, out)?;
    }
    Ok(())
}

fn hash_directory_sha256(root: &Path) -> Result<String, String> {
    if !root.exists() {
        return Err(format!("plugin path not found: {}", root.display()));
    }
    if !root.is_dir() {
        return Err(format!("plugin path must be a directory: {}", root.display()));
    }
    let mut files = Vec::new();
    collect_files_for_hash(root, root, &mut files)?;

    let mut hasher = Sha256::new();
    for rel in files {
        let rel_norm = rel.to_string_lossy().replace('\\', "/");
        hasher.update(rel_norm.as_bytes());
        hasher.update([0]);
        let content = fs::read(root.join(&rel))
            .map_err(|e| format!("failed to read {}: {}", root.join(&rel).display(), e))?;
        hasher.update(content);
        hasher.update([0xff]);
    }
    let out = hasher.finalize();
    Ok(format!("{:x}", out))
}

fn normalize_event_name(raw: &str) -> Option<String> {
    let lower = raw.trim().to_ascii_lowercase();
    match lower.as_str() {
        "pretooluse" | "pre_tool_use" | "pre-tool-use" => Some("PreToolUse".to_string()),
        "permissionrequest" | "permission_request" | "permission-request" => {
            Some("PermissionRequest".to_string())
        }
        "posttooluse" | "post_tool_use" | "post-tool-use" => Some("PostToolUse".to_string()),
        "sessionstart" | "session_start" | "session-start" => Some("SessionStart".to_string()),
        "userpromptsubmit" | "user_prompt_submit" | "user-prompt-submit" => {
            Some("UserPromptSubmit".to_string())
        }
        "stop" => Some("Stop".to_string()),
        "subagentstop" | "subagent_stop" | "subagent-stop" => Some("SubagentStop".to_string()),
        "precompact" | "pre_compact" | "pre-compact" => Some("PreCompact".to_string()),
        "postcompact" | "post_compact" | "post-compact" => Some("PostCompact".to_string()),
        "*" => Some("*".to_string()),
        _ => None,
    }
}

fn normalize_optional_nonempty(raw: Option<String>) -> Option<String> {
    raw.and_then(|v| {
        let s = v.trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    })
}

fn manifest_path_for_plugin(plugin_path: &Path) -> PathBuf {
    plugin_path.join(".codex-plugin").join("plugin.json")
}

fn read_manifest(plugin_path: &Path) -> Result<PluginManifest, String> {
    let manifest_path = manifest_path_for_plugin(plugin_path);
    let text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("failed to read {}: {}", manifest_path.display(), e))?;
    serde_json::from_str::<PluginManifest>(&text)
        .map_err(|e| format!("invalid plugin manifest {}: {}", manifest_path.display(), e))
}

fn push_event_hook_if_present(
    out: &mut Vec<PluginHookInvocation>,
    plugin_name: &str,
    event: &str,
    script: Option<String>,
) {
    let Some(script_value) = normalize_optional_nonempty(script) else {
        return;
    };
    out.push(PluginHookInvocation {
        plugin_name: plugin_name.to_string(),
        event: event.to_string(),
        script: script_value,
        timeout_secs: None,
        json_protocol: None,
        tool_prefix: None,
        permission_mode: None,
        failure_policy: None,
    });
}

fn manifest_hooks_for_plugin(plugin_name: &str, plugin_path: &Path) -> Vec<PluginHookInvocation> {
    let Ok(manifest) = read_manifest(plugin_path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    push_event_hook_if_present(
        &mut out,
        plugin_name,
        "PreToolUse",
        manifest.hooks.pre_tool_use.clone(),
    );
    push_event_hook_if_present(
        &mut out,
        plugin_name,
        "PermissionRequest",
        manifest.hooks.permission_request.clone(),
    );
    push_event_hook_if_present(
        &mut out,
        plugin_name,
        "PostToolUse",
        manifest.hooks.post_tool_use.clone(),
    );
    push_event_hook_if_present(
        &mut out,
        plugin_name,
        "SessionStart",
        manifest.hooks.session_start.clone(),
    );
    push_event_hook_if_present(
        &mut out,
        plugin_name,
        "UserPromptSubmit",
        manifest.hooks.user_prompt_submit.clone(),
    );
    push_event_hook_if_present(&mut out, plugin_name, "Stop", manifest.hooks.stop.clone());
    push_event_hook_if_present(
        &mut out,
        plugin_name,
        "SubagentStop",
        manifest.hooks.subagent_stop.clone(),
    );
    push_event_hook_if_present(
        &mut out,
        plugin_name,
        "PreCompact",
        manifest.hooks.pre_compact.clone(),
    );
    push_event_hook_if_present(
        &mut out,
        plugin_name,
        "PostCompact",
        manifest.hooks.post_compact.clone(),
    );

    for row in manifest.hooks.handlers {
        let Some(event_raw) = normalize_optional_nonempty(row.event.clone()) else {
            continue;
        };
        let Some(script) = normalize_optional_nonempty(row.script.clone()) else {
            continue;
        };
        let Some(event) = normalize_event_name(&event_raw) else {
            continue;
        };
        out.push(PluginHookInvocation {
            plugin_name: plugin_name.to_string(),
            event,
            script,
            timeout_secs: row.timeout_secs,
            json_protocol: row.json_protocol,
            tool_prefix: normalize_optional_nonempty(row.tool_prefix.clone()),
            permission_mode: normalize_optional_nonempty(row.permission_mode.clone()),
            failure_policy: normalize_optional_nonempty(row.failure_policy.clone()),
        });
    }

    out
}

pub fn list_enabled_trusted_plugin_hooks() -> Vec<PluginHookInvocation> {
    let mut hooks = Vec::new();
    for plugin in verify_enabled_plugins() {
        let plugin_path = PathBuf::from(&plugin.path);
        hooks.extend(manifest_hooks_for_plugin(&plugin.name, &plugin_path));
    }
    hooks.sort_by(|a, b| a.plugin_name.cmp(&b.plugin_name).then(a.event.cmp(&b.event)));
    hooks
}

pub fn set_plugin_config(name: &str, key: &str, value: &str) -> Result<(), String> {
    let mut state = load_state();
    let Some(item) = find_plugin_mut(&mut state, name) else {
        return Err(format!("plugin `{}` not found", name));
    };
    let k = key.trim();
    if k.is_empty() {
        return Err("plugin config key cannot be empty".to_string());
    }
    if k.eq_ignore_ascii_case("source") {
        item.source = match normalize_optional_text(Some(value)) {
            Some(v) => Some(normalize_source(&v)?),
            None => None,
        };
        validate_plugin_publish_meta(item)?;
        return save_state(&state);
    }
    if k.eq_ignore_ascii_case("version") {
        item.version = normalize_optional_text(Some(value));
        validate_plugin_publish_meta(item)?;
        return save_state(&state);
    }
    if k.eq_ignore_ascii_case("signature") {
        item.signature = Some(normalize_signature(value)?);
        validate_plugin_publish_meta(item)?;
        return save_state(&state);
    }
    item.config.insert(k.to_string(), value.trim().to_string());
    save_state(&state)
}

pub fn remove_plugin_config(name: &str, key: &str) -> Result<bool, String> {
    let mut state = load_state();
    let Some(item) = find_plugin_mut(&mut state, name) else {
        return Err(format!("plugin `{}` not found", name));
    };
    if key.eq_ignore_ascii_case("source") {
        let changed = item.source.take().is_some();
        validate_plugin_publish_meta(item)?;
        save_state(&state)?;
        return Ok(changed);
    }
    if key.eq_ignore_ascii_case("version") {
        let changed = item.version.take().is_some();
        validate_plugin_publish_meta(item)?;
        save_state(&state)?;
        return Ok(changed);
    }
    if key.eq_ignore_ascii_case("signature") {
        let changed = item.signature.take().is_some();
        validate_plugin_publish_meta(item)?;
        save_state(&state)?;
        return Ok(changed);
    }
    let changed = item.config.remove(key).is_some();
    save_state(&state)?;
    Ok(changed)
}

pub fn export_state(path: &str) -> Result<String, String> {
    let target = Path::new(path);
    let body = serde_json::to_string_pretty(&load_state()).map_err(|e| e.to_string())?;
    fs::write(target, body).map_err(|e| e.to_string())?;
    Ok(target.to_string_lossy().to_string())
}

pub fn import_state(path: &str, merge: bool) -> Result<usize, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let incoming: Vec<PluginRecord> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if !merge {
        save_state(&incoming)?;
        return Ok(incoming.len());
    }
    let mut state = load_state();
    for item in incoming {
        if let Some(pos) = state.iter().position(|x| x.name == item.name) {
            state[pos] = item;
        } else {
            state.push(item);
        }
    }
    let total = state.len();
    save_state(&state)?;
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct EnvGuard {
        key: &'static str,
        old: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let old = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(v) = &self.old {
                std::env::set_var(self.key, v);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{}_{}", prefix, ts));
        fs::create_dir_all(&dir).expect("create temp");
        dir
    }

    fn write_text(path: &Path, content: &str) {
        fs::write(path, content).expect("write file");
    }

    fn create_plugin_fixture(root: &Path, hook_script: &str) -> PathBuf {
        let plugin_dir = root.join("demo-plugin");
        let manifest_dir = plugin_dir.join(".codex-plugin");
        fs::create_dir_all(&manifest_dir).expect("create manifest dir");
        let manifest = format!(
            r#"{{
  "name": "demo-plugin",
  "version": "1.0.0",
  "hooks": {{
    "pre_tool_use": "{hook_script}",
    "handlers": [
      {{
        "event": "PostToolUse",
        "script": "{hook_script}",
        "tool_prefix": "bash",
        "permission_mode": "workspace-write"
      }},
      {{
        "event": "BadEvent",
        "script": "{hook_script}"
      }}
    ]
  }}
}}"#
        );
        write_text(&manifest_dir.join("plugin.json"), &manifest);
        plugin_dir
    }

    #[test]
    fn list_enabled_trusted_plugin_hooks_reads_manifest_hooks() {
        let _lock = plugin_state_env_lock().lock().expect("plugin env lock");
        let root = make_temp_dir("asi_plugin_hooks_manifest");
        let state_path = root.join("plugins_state.json");
        let _state_guard = EnvGuard::set(
            "ASI_PLUGIN_STATE_PATH",
            state_path.to_string_lossy().as_ref(),
        );

        let plugin_dir = create_plugin_fixture(&root, "echo plugin-hook");
        let plugin_dir_str = plugin_dir.to_string_lossy().to_string();
        add_plugin("demo_plugin", &plugin_dir_str).expect("add plugin");
        set_plugin_trust_manual("demo_plugin", true).expect("trust plugin");

        let hooks = list_enabled_trusted_plugin_hooks();
        assert!(hooks.iter().any(|h| {
            h.plugin_name == "demo_plugin"
                && h.event == "PreToolUse"
                && h.script == "echo plugin-hook"
        }));
        assert!(hooks.iter().any(|h| {
            h.plugin_name == "demo_plugin"
                && h.event == "PostToolUse"
                && h.script == "echo plugin-hook"
                && h.tool_prefix.as_deref() == Some("bash")
        }));
        assert!(!hooks.iter().any(|h| h.event == "BadEvent"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verify_enabled_plugins_marks_hash_trust_false_on_manifest_change() {
        let _lock = plugin_state_env_lock().lock().expect("plugin env lock");
        let root = make_temp_dir("asi_plugin_hash_verify");
        let state_path = root.join("plugins_state.json");
        let _state_guard = EnvGuard::set(
            "ASI_PLUGIN_STATE_PATH",
            state_path.to_string_lossy().as_ref(),
        );

        let plugin_dir = create_plugin_fixture(&root, "echo stable");
        let plugin_dir_str = plugin_dir.to_string_lossy().to_string();
        add_plugin("hash_plugin", &plugin_dir_str).expect("add plugin");
        let _hash = set_plugin_trust_hash("hash_plugin", None).expect("set hash trust");

        let first = verify_enabled_plugins();
        assert!(first.iter().any(|p| p.name == "hash_plugin"));

        let manifest_path = plugin_dir.join(".codex-plugin").join("plugin.json");
        write_text(
            &manifest_path,
            r#"{"name":"demo-plugin","version":"2.0.0","hooks":{"pre_tool_use":"echo changed"}}"#,
        );

        let second = verify_enabled_plugins();
        assert!(!second.iter().any(|p| p.name == "hash_plugin"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn hash_directory_sha256_changes_when_non_manifest_file_changes() {
        let root = make_temp_dir("asi_plugin_hash_dir_change");
        let plugin_dir = create_plugin_fixture(&root, "echo stable");
        let extra_path = plugin_dir.join("scripts");
        fs::create_dir_all(&extra_path).expect("create scripts dir");
        let worker = extra_path.join("worker.ps1");
        write_text(&worker, "Write-Output 'v1'");

        let first = hash_directory_sha256(&plugin_dir).expect("hash directory first");
        write_text(&worker, "Write-Output 'v2'");
        let second = hash_directory_sha256(&plugin_dir).expect("hash directory second");

        assert_ne!(first, second);
        let _ = fs::remove_dir_all(root);
    }
}
