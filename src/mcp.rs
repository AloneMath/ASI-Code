use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerRecord {
    pub name: String,
    pub command: String,
    pub pid: Option<u32>,
    pub status: String,
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default = "default_trusted")]
    pub trusted: bool,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub signature: Option<String>,
    pub auth_type: String,
    pub auth_value: Option<String>,
    pub config: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ScopedStateFile {
    #[serde(default)]
    session: Vec<McpServerRecord>,
    #[serde(default)]
    project: Vec<McpServerRecord>,
    #[serde(default)]
    global: Vec<McpServerRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Session,
    Project,
    Global,
}

fn default_scope() -> String {
    "session".to_string()
}

fn default_trusted() -> bool {
    false
}

fn parse_boolish(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn normalize_scope_value(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "session" => Some("session"),
        "project" => Some("project"),
        "global" => Some("global"),
        _ => None,
    }
}

fn parse_scope_kind(raw: &str) -> Result<ScopeKind, String> {
    match normalize_scope_value(raw) {
        Some("session") => Ok(ScopeKind::Session),
        Some("project") => Ok(ScopeKind::Project),
        Some("global") => Ok(ScopeKind::Global),
        _ => Err("scope must be one of: session|project|global".to_string()),
    }
}

fn allow_untrusted_start() -> bool {
    std::env::var("ASI_MCP_ALLOW_UNTRUSTED_START")
        .ok()
        .and_then(|v| parse_boolish(&v))
        .unwrap_or(false)
}

fn read_state_path_override() -> Option<PathBuf> {
    std::env::var("ASI_MCP_STATE_PATH")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn using_legacy_single_path_mode() -> bool {
    read_state_path_override().is_some()
}

fn project_state_path() -> PathBuf {
    if let Some(path) = read_state_path_override() {
        return path;
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".asi_mcp_state.json")
}

fn session_state_path() -> PathBuf {
    if using_legacy_single_path_mode() {
        return project_state_path();
    }
    let ts = std::process::id();
    std::env::temp_dir().join(format!("asi_mcp_session_{}.json", ts))
}

fn global_state_path() -> PathBuf {
    if using_legacy_single_path_mode() {
        return project_state_path();
    }
    let root = std::env::var("ASI_GLOBAL_CONFIG_DIR")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var("USERPROFILE")
                .ok()
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::temp_dir())
                .join(".asi")
        });
    root.join("mcp_state_global.json")
}

#[cfg(test)]
pub(crate) fn mcp_state_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn active_scope_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn load_state_file(path: &Path) -> Vec<McpServerRecord> {
    let text = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    if let Ok(v1) = serde_json::from_str::<Vec<McpServerRecord>>(&text) {
        return v1;
    }
    if let Ok(scoped) = serde_json::from_str::<ScopedStateFile>(&text) {
        return merge_prefer_newer(scoped.session, scoped.project, scoped.global);
    }
    Vec::new()
}

fn save_state_file(path: &Path, items: &[McpServerRecord]) -> Result<(), String> {
    ensure_parent(path)?;
    let body = serde_json::to_string_pretty(items).map_err(|e| e.to_string())?;
    fs::write(path, body).map_err(|e| e.to_string())
}

fn merge_prefer_newer(
    session_items: Vec<McpServerRecord>,
    project_items: Vec<McpServerRecord>,
    global_items: Vec<McpServerRecord>,
) -> Vec<McpServerRecord> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for mut item in session_items.into_iter().chain(project_items).chain(global_items) {
        if seen.insert(item.name.clone()) {
            if normalize_scope_value(&item.scope).is_none() {
                item.scope = default_scope();
            }
            out.push(item);
        }
    }
    out
}

fn merge_visible_state() -> Vec<McpServerRecord> {
    if using_legacy_single_path_mode() {
        return load_state_file(&project_state_path());
    }
    let session = load_state_file(&session_state_path());
    let project = load_state_file(&project_state_path());
    let global = load_state_file(&global_state_path());
    merge_prefer_newer(session, project, global)
}

fn state_path_for_scope(scope: ScopeKind) -> PathBuf {
    match scope {
        ScopeKind::Session => session_state_path(),
        ScopeKind::Project => project_state_path(),
        ScopeKind::Global => global_state_path(),
    }
}

fn mutate_scope_state<F, T>(scope: ScopeKind, f: F) -> Result<T, String>
where
    F: FnOnce(&mut Vec<McpServerRecord>) -> Result<T, String>,
{
    let path = state_path_for_scope(scope);
    let mut items = load_state_file(&path);
    let result = f(&mut items)?;
    save_state_file(&path, &items)?;
    Ok(result)
}

fn find_server_mut<'a>(items: &'a mut [McpServerRecord], name: &str) -> Option<&'a mut McpServerRecord> {
    items.iter_mut().find(|x| x.name == name)
}

fn default_server(name: &str, command: &str, scope: ScopeKind) -> McpServerRecord {
    let scope_str = match scope {
        ScopeKind::Session => "session",
        ScopeKind::Project => "project",
        ScopeKind::Global => "global",
    };
    McpServerRecord {
        name: name.to_string(),
        command: command.to_string(),
        pid: None,
        status: "stopped".to_string(),
        scope: scope_str.to_string(),
        trusted: default_trusted(),
        source: None,
        version: None,
        signature: None,
        auth_type: "none".to_string(),
        auth_value: None,
        config: HashMap::new(),
    }
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

fn validate_publish_meta(item: &McpServerRecord) -> Result<(), String> {
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

pub fn list_servers() -> Vec<McpServerRecord> {
    let _guard = active_scope_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    merge_visible_state()
}

pub fn get_server(name: &str) -> Option<McpServerRecord> {
    let _guard = active_scope_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    merge_visible_state().into_iter().find(|x| x.name == name)
}

#[cfg(test)]
pub fn add_server(name: &str, command: &str) -> Result<McpServerRecord, String> {
    add_server_with_scope(name, command, "session")
}

pub fn add_server_with_scope(name: &str, command: &str, scope: &str) -> Result<McpServerRecord, String> {
    let _guard = active_scope_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let scope_kind = parse_scope_kind(scope)?;
    if merge_visible_state().iter().any(|x| x.name == name) {
        return Err(format!("MCP server `{}` already exists", name));
    }
    mutate_scope_state(scope_kind, |items| {
        if items.iter().any(|x| x.name == name) {
            return Err(format!("MCP server `{}` already exists in scope", name));
        }
        let item = default_server(name, command, scope_kind);
        items.push(item.clone());
        Ok(item)
    })
}

pub fn remove_server(name: &str) -> Result<bool, String> {
    let _guard = active_scope_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if using_legacy_single_path_mode() {
        return mutate_scope_state(ScopeKind::Project, |items| {
            let before = items.len();
            items.retain(|x| x.name != name);
            Ok(items.len() != before)
        });
    }
    let mut changed = false;
    for scope in [ScopeKind::Session, ScopeKind::Project, ScopeKind::Global] {
        let local_changed = mutate_scope_state(scope, |items| {
            let before = items.len();
            items.retain(|x| x.name != name);
            Ok(items.len() != before)
        })?;
        changed = changed || local_changed;
    }
    Ok(changed)
}

fn mutate_visible_server<F>(name: &str, f: F) -> Result<(), String>
where
    F: FnOnce(&mut McpServerRecord) -> Result<(), String>,
{
    let _guard = active_scope_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let merged = merge_visible_state();
    let Some(current) = merged.into_iter().find(|x| x.name == name) else {
        return Err(format!("MCP server `{}` not found", name));
    };
    let scope_kind = parse_scope_kind(&current.scope)?;
    mutate_scope_state(scope_kind, |items| {
        let Some(item) = find_server_mut(items, name) else {
            return Err(format!("MCP server `{}` not found", name));
        };
        f(item)
    })
}

pub fn set_server_auth(name: &str, auth_type: &str, auth_value: Option<&str>) -> Result<(), String> {
    mutate_visible_server(name, |item| {
        item.auth_type = auth_type.trim().to_ascii_lowercase();
        item.auth_value = auth_value
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        Ok(())
    })
}

pub fn set_server_config(name: &str, key: &str, value: &str) -> Result<(), String> {
    mutate_visible_server(name, |item| {
        let key_norm = key.trim();
        if key_norm.eq_ignore_ascii_case("source") {
            item.source = match normalize_optional_text(Some(value)) {
                Some(v) => Some(normalize_source(&v)?),
                None => None,
            };
            return validate_publish_meta(item);
        }
        if key_norm.eq_ignore_ascii_case("version") {
            item.version = normalize_optional_text(Some(value));
            return validate_publish_meta(item);
        }
        if key_norm.eq_ignore_ascii_case("signature") {
            item.signature = Some(normalize_signature(value)?);
            return validate_publish_meta(item);
        }
        item.config
            .insert(key.trim().to_string(), value.trim().to_string());
        Ok(())
    })
}

pub fn set_server_scope(name: &str, scope: &str) -> Result<(), String> {
    let _guard = active_scope_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let target_scope = parse_scope_kind(scope)?;
    let merged = merge_visible_state();
    let Some(current) = merged.into_iter().find(|x| x.name == name) else {
        return Err(format!("MCP server `{}` not found", name));
    };
    let current_scope = parse_scope_kind(&current.scope)?;
    if current_scope == target_scope {
        return Ok(());
    }
    mutate_scope_state(current_scope, |items| {
        items.retain(|x| x.name != name);
        Ok(())
    })?;
    mutate_scope_state(target_scope, |items| {
        let mut moved = current.clone();
        moved.scope = match target_scope {
            ScopeKind::Session => "session".to_string(),
            ScopeKind::Project => "project".to_string(),
            ScopeKind::Global => "global".to_string(),
        };
        items.retain(|x| x.name != name);
        items.push(moved);
        Ok(())
    })?;
    Ok(())
}

pub fn set_server_trusted(name: &str, trusted: bool) -> Result<(), String> {
    mutate_visible_server(name, |item| {
        item.trusted = trusted;
        validate_publish_meta(item)
    })
}

pub fn remove_server_config(name: &str, key: &str) -> Result<bool, String> {
    let mut changed = false;
    mutate_visible_server(name, |item| {
        if key.eq_ignore_ascii_case("source") {
            changed = item.source.take().is_some();
            return validate_publish_meta(item);
        }
        if key.eq_ignore_ascii_case("version") {
            changed = item.version.take().is_some();
            return validate_publish_meta(item);
        }
        if key.eq_ignore_ascii_case("signature") {
            changed = item.signature.take().is_some();
            return validate_publish_meta(item);
        }
        changed = item.config.remove(key).is_some();
        Ok(())
    })?;
    Ok(changed)
}

pub fn start_server(name: &str, command_override: Option<&str>) -> Result<McpServerRecord, String> {
    let _guard = active_scope_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let merged = merge_visible_state();
    let existing = merged.into_iter().find(|x| x.name == name);
    let (scope_kind, mut record) = if let Some(v) = existing {
        (parse_scope_kind(&v.scope)?, v)
    } else if let Some(cmd) = command_override {
        let created = add_server_with_scope(name, cmd.trim(), "session")?;
        (ScopeKind::Session, created)
    } else {
        return Err(format!(
            "MCP server `{}` not found. Use /mcp add <name> <command> first.",
            name
        ));
    };

    if record.status == "running" {
        return Err(format!("MCP server `{}` is already running", name));
    }
    if let Some(cmd) = command_override {
        let cmd_trim = cmd.trim();
        if !cmd_trim.is_empty() {
            record.command = cmd_trim.to_string();
        }
    }
    if record.command.trim().is_empty() {
        return Err(format!("MCP server `{}` has empty command", name));
    }
    if !record.trusted && !allow_untrusted_start() {
        return Err(format!(
            "MCP server `{}` is untrusted. Run /mcp config trust {} on before start, or set ASI_MCP_ALLOW_UNTRUSTED_START=true to bypass.",
            name, name
        ));
    }

    let child = if cfg!(target_os = "windows") {
        Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(&record.command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?
    } else {
        Command::new("sh")
            .arg("-lc")
            .arg(&record.command)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?
    };

    record.pid = Some(child.id());
    record.status = "running".to_string();
    mutate_scope_state(scope_kind, |items| {
        let Some(item) = find_server_mut(items, name) else {
            return Err(format!("MCP server `{}` not found", name));
        };
        *item = record.clone();
        Ok(())
    })?;
    Ok(record)
}

pub fn stop_server(name: &str) -> Result<bool, String> {
    let _guard = active_scope_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let merged = merge_visible_state();
    let Some(record) = merged.into_iter().find(|x| x.name == name) else {
        return Ok(false);
    };
    let scope_kind = parse_scope_kind(&record.scope)?;
    let mut stopped = false;
    mutate_scope_state(scope_kind, |items| {
        let Some(item) = find_server_mut(items, name) else {
            return Err(format!("MCP server `{}` not found", name));
        };
        if item.status == "running" {
            if let Some(pid) = item.pid {
                if cfg!(target_os = "windows") {
                    let _ = Command::new("taskkill")
                        .args(["/PID", &pid.to_string(), "/T", "/F"])
                        .output();
                } else {
                    let _ = Command::new("kill").arg(pid.to_string()).output();
                }
            }
            item.status = "stopped".to_string();
            item.pid = None;
            stopped = true;
        }
        Ok(())
    })?;
    Ok(stopped)
}

pub fn export_state(path: &str) -> Result<String, String> {
    let _guard = active_scope_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if using_legacy_single_path_mode() {
        let body = serde_json::to_string_pretty(&load_state_file(&project_state_path()))
            .map_err(|e| e.to_string())?;
        fs::write(path, body).map_err(|e| e.to_string())?;
        return Ok(path.to_string());
    }
    let body = serde_json::to_string_pretty(&ScopedStateFile {
        session: load_state_file(&session_state_path()),
        project: load_state_file(&project_state_path()),
        global: load_state_file(&global_state_path()),
    })
    .map_err(|e| e.to_string())?;
    fs::write(path, body).map_err(|e| e.to_string())?;
    Ok(path.to_string())
}

pub fn import_state(path: &str, merge: bool) -> Result<usize, String> {
    let _guard = active_scope_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;

    if using_legacy_single_path_mode() {
        let incoming_v1: Vec<McpServerRecord> = if let Ok(v2) = serde_json::from_str::<ScopedStateFile>(&text) {
            merge_prefer_newer(v2.session, v2.project, v2.global)
        } else {
            serde_json::from_str(&text).map_err(|e| e.to_string())?
        };
        if !merge {
            save_state_file(&project_state_path(), &incoming_v1)?;
            return Ok(incoming_v1.len());
        }
        let mut existing = load_state_file(&project_state_path());
        for rec in incoming_v1 {
            if let Some(pos) = existing.iter().position(|x| x.name == rec.name) {
                existing[pos] = rec;
            } else {
                existing.push(rec);
            }
        }
        let count = existing.len();
        save_state_file(&project_state_path(), &existing)?;
        return Ok(count);
    }

    let incoming = if let Ok(v2) = serde_json::from_str::<ScopedStateFile>(&text) {
        v2
    } else {
        let v1: Vec<McpServerRecord> = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let mut scoped = ScopedStateFile::default();
        for mut item in v1 {
            let kind = parse_scope_kind(&item.scope).unwrap_or(ScopeKind::Session);
            item.scope = match kind {
                ScopeKind::Session => "session".to_string(),
                ScopeKind::Project => "project".to_string(),
                ScopeKind::Global => "global".to_string(),
            };
            match kind {
                ScopeKind::Session => scoped.session.push(item),
                ScopeKind::Project => scoped.project.push(item),
                ScopeKind::Global => scoped.global.push(item),
            }
        }
        scoped
    };

    let apply_scope = |scope: ScopeKind, mut records: Vec<McpServerRecord>| -> Result<(), String> {
        let path = state_path_for_scope(scope);
        if !merge {
            for rec in &mut records {
                rec.scope = match scope {
                    ScopeKind::Session => "session".to_string(),
                    ScopeKind::Project => "project".to_string(),
                    ScopeKind::Global => "global".to_string(),
                };
            }
            return save_state_file(&path, &records);
        }
        let mut existing = load_state_file(&path);
        for mut rec in records {
            rec.scope = match scope {
                ScopeKind::Session => "session".to_string(),
                ScopeKind::Project => "project".to_string(),
                ScopeKind::Global => "global".to_string(),
            };
            if let Some(pos) = existing.iter().position(|x| x.name == rec.name) {
                existing[pos] = rec;
            } else {
                existing.push(rec);
            }
        }
        save_state_file(&path, &existing)
    };

    apply_scope(ScopeKind::Session, incoming.session)?;
    apply_scope(ScopeKind::Project, incoming.project)?;
    apply_scope(ScopeKind::Global, incoming.global)?;

    Ok(merge_visible_state().len())
}

#[cfg(test)]
mod tests {
    use super::{
        add_server, add_server_with_scope, export_state, import_state, list_servers, remove_server,
        set_server_auth, set_server_config, set_server_scope, start_server,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn cwd_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{}_{}", prefix, ts));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn with_temp_cwd<F>(prefix: &str, f: F)
    where
        F: FnOnce(&PathBuf),
    {
        let _env_guard = super::mcp_state_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _guard = cwd_lock().lock().unwrap_or_else(|e| e.into_inner());
        let old = std::env::current_dir().unwrap();
        let old_state_path = std::env::var("ASI_MCP_STATE_PATH").ok();
        let old_global_dir = std::env::var("ASI_GLOBAL_CONFIG_DIR").ok();
        let dir = temp_dir(prefix);
        std::env::set_var("ASI_GLOBAL_CONFIG_DIR", dir.join(".global").display().to_string());
        std::env::set_var("ASI_MCP_STATE_PATH", dir.join("mcp_state_project.json").display().to_string());
        std::env::set_current_dir(&dir).unwrap();
        f(&dir);
        std::env::set_current_dir(old).unwrap();
        if let Some(v) = old_state_path {
            std::env::set_var("ASI_MCP_STATE_PATH", v);
        } else {
            std::env::remove_var("ASI_MCP_STATE_PATH");
        }
        if let Some(v) = old_global_dir {
            std::env::set_var("ASI_GLOBAL_CONFIG_DIR", v);
        } else {
            std::env::remove_var("ASI_GLOBAL_CONFIG_DIR");
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn import_state_replace_overwrites_existing_state() {
        with_temp_cwd("asi_mcp_replace", |dir| {
            add_server("old", "echo old").unwrap();
            let import_path = dir.join("import_replace.json");
            fs::write(
                &import_path,
                r#"{
  "session": [
    {
      "name": "new",
      "command": "echo new",
      "pid": null,
      "status": "stopped",
      "scope": "session",
      "trusted": false,
      "auth_type": "none",
      "auth_value": null,
      "config": {}
    }
  ],
  "project": [],
  "global": []
}"#,
            )
            .unwrap();

            let total = import_state(&import_path.display().to_string(), false).unwrap();
            assert_eq!(total, 1);

            let state = list_servers();
            assert_eq!(state.len(), 1);
            assert_eq!(state[0].name, "new");
        });
    }

    #[test]
    fn import_state_merge_updates_and_preserves_records() {
        with_temp_cwd("asi_mcp_merge", |dir| {
            add_server("a", "echo a").unwrap();
            add_server_with_scope("b", "echo b", "project").unwrap();
            set_server_auth("a", "bearer", Some("token-a")).unwrap();
            set_server_config("a", "k1", "v1").unwrap();

            let import_path = dir.join("import_merge.json");
            fs::write(
                &import_path,
                r#"{
  "session": [
    {
      "name": "a",
      "command": "echo a2",
      "pid": null,
      "status": "stopped",
      "scope": "session",
      "trusted": false,
      "auth_type": "api-key",
      "auth_value": "token-a2",
      "config": { "k2": "v2" }
    }
  ],
  "project": [],
  "global": [
    {
      "name": "c",
      "command": "echo c",
      "pid": null,
      "status": "stopped",
      "scope": "global",
      "trusted": false,
      "auth_type": "none",
      "auth_value": null,
      "config": {}
    }
  ]
}"#,
            )
            .unwrap();

            let total = import_state(&import_path.display().to_string(), true).unwrap();
            assert_eq!(total, 3);

            let state = list_servers();
            assert_eq!(state.len(), 3);
            let a = state.iter().find(|s| s.name == "a").unwrap();
            assert_eq!(a.command, "echo a2");
            assert_eq!(a.auth_type, "api-key");
            assert_eq!(a.auth_value.as_deref(), Some("token-a2"));
            assert_eq!(a.config.get("k2").map(String::as_str), Some("v2"));
            assert!(state.iter().any(|s| s.name == "b"));
            assert!(state.iter().any(|s| s.name == "c"));
        });
    }

    #[test]
    fn export_state_roundtrips_current_records() {
        with_temp_cwd("asi_mcp_export", |dir| {
            add_server("x", "echo x").unwrap();
            add_server_with_scope("y", "echo y", "project").unwrap();
            set_server_config("x", "env", "dev").unwrap();
            let out = dir.join("mcp_export.json");
            let saved = export_state(&out.display().to_string()).unwrap();
            assert_eq!(saved, out.display().to_string());
            let txt = fs::read_to_string(out).unwrap();
            assert!(txt.contains("\"session\""));
            assert!(txt.contains("\"project\""));
            assert!(txt.contains("\"name\": \"x\""));
            assert!(txt.contains("\"name\": \"y\""));
            let _ = remove_server("x");
            let _ = remove_server("y");
        });
    }

    #[test]
    fn scope_layers_are_visible_and_prefer_narrow_scope() {
        with_temp_cwd("asi_mcp_scopes_visible", |_dir| {
            add_server_with_scope("dup", "echo session", "session").unwrap();
            add_server_with_scope("proj", "echo project", "project").unwrap();
            add_server_with_scope("glob", "echo global", "global").unwrap();
            let items = list_servers();
            assert_eq!(items.len(), 3);
            let dup = items.iter().find(|x| x.name == "dup").unwrap();
            assert_eq!(dup.scope, "session");
            let proj = items.iter().find(|x| x.name == "proj").unwrap();
            assert_eq!(proj.scope, "project");
            let glob = items.iter().find(|x| x.name == "glob").unwrap();
            assert_eq!(glob.scope, "global");
        });
    }

    #[test]
    fn move_server_scope_relocates_storage() {
        with_temp_cwd("asi_mcp_scope_move", |_dir| {
            add_server("srv", "echo s").unwrap();
            set_server_scope("srv", "project").unwrap();
            let items = list_servers();
            let srv = items.iter().find(|x| x.name == "srv").unwrap();
            assert_eq!(srv.scope, "project");
        });
    }

    #[test]
    fn start_server_rejects_untrusted_server_by_default() {
        with_temp_cwd("asi_mcp_start_untrusted", |_dir| {
            add_server("srv", "echo hi").unwrap();
            let err = start_server("srv", None).expect_err("untrusted server must be blocked");
            assert!(err.to_ascii_lowercase().contains("untrusted"));
            assert!(err.contains("/mcp config trust srv on"));
        });
    }
}
