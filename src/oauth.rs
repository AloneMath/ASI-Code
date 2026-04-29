use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokenRecord {
    pub provider: String,
    pub access_token: String,
    pub saved_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Session,
    Project,
    Global,
}

pub fn normalize_scope_value(raw: &str) -> Option<&'static str> {
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

fn scope_name(scope: ScopeKind) -> &'static str {
    match scope {
        ScopeKind::Session => "session",
        ScopeKind::Project => "project",
        ScopeKind::Global => "global",
    }
}

fn read_oauth_path_override() -> Option<PathBuf> {
    std::env::var("ASI_OAUTH_PATH")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn using_legacy_single_path_mode() -> bool {
    read_oauth_path_override().is_some()
}

fn project_oauth_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".asi_oauth.json")
}

fn session_oauth_path() -> PathBuf {
    let pid = std::process::id();
    std::env::temp_dir().join(format!("asi_oauth_session_{}.json", pid))
}

fn global_oauth_path() -> PathBuf {
    if let Ok(root) = std::env::var("ASI_GLOBAL_CONFIG_DIR") {
        let trimmed = root.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("oauth_global.json");
        }
    }

    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".asi_oauth.json")
}

fn path_for_scope(scope: ScopeKind) -> PathBuf {
    if let Some(path) = read_oauth_path_override() {
        return path;
    }
    match scope {
        ScopeKind::Session => session_oauth_path(),
        ScopeKind::Project => project_oauth_path(),
        ScopeKind::Global => global_oauth_path(),
    }
}

fn oauth_candidates() -> Vec<PathBuf> {
    if let Some(path) = read_oauth_path_override() {
        return vec![path];
    }
    vec![global_oauth_path(), project_oauth_path()]
}

fn oauth_file() -> PathBuf {
    oauth_candidates()
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from(".").join(".asi_oauth.json"))
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn read_items(path: &Path) -> Vec<OAuthTokenRecord> {
    let text = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn write_items(path: &Path, items: &[OAuthTokenRecord]) -> Result<(), String> {
    ensure_parent(path)?;
    let body = serde_json::to_string_pretty(items).map_err(|e| e.to_string())?;
    fs::write(path, body).map_err(|e| format!("{}: {}", path.display(), e))
}

fn load_all_flat() -> Vec<OAuthTokenRecord> {
    for path in oauth_candidates() {
        let text = match fs::read_to_string(&path) {
            Ok(v) => v,
            Err(_) => continue,
        };
        return serde_json::from_str(&text).unwrap_or_default();
    }
    Vec::new()
}

fn save_all_flat(items: &[OAuthTokenRecord]) -> Result<(), String> {
    let body = serde_json::to_string_pretty(items).map_err(|e| e.to_string())?;
    let mut last_err: Option<String> = None;
    for path in oauth_candidates() {
        let _ = ensure_parent(&path);
        match fs::write(&path, &body) {
            Ok(_) => return Ok(()),
            Err(e) => {
                last_err = Some(format!("{}: {}", path.display(), e));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| "unable to persist oauth token file".to_string()))
}

fn now_ts() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())
        .map(|v| v.as_secs())
}

fn upsert_token(items: &mut Vec<OAuthTokenRecord>, provider: &str, token: &str, now: u64) {
    if let Some(item) = items.iter_mut().find(|x| x.provider == provider) {
        item.access_token = token.to_string();
        item.saved_at = now;
    } else {
        items.push(OAuthTokenRecord {
            provider: provider.to_string(),
            access_token: token.to_string(),
            saved_at: now,
        });
    }
}

fn remove_token(items: &mut Vec<OAuthTokenRecord>, provider: &str) -> bool {
    let before = items.len();
    items.retain(|x| x.provider != provider);
    items.len() != before
}

fn mutate_scope_items<F, T>(scope: ScopeKind, f: F) -> Result<T, String>
where
    F: FnOnce(&mut Vec<OAuthTokenRecord>) -> Result<T, String>,
{
    let path = path_for_scope(scope);
    let mut items = read_items(&path);
    let out = f(&mut items)?;
    write_items(&path, &items)?;
    Ok(out)
}

pub fn save_token(provider: &str, token: &str) -> Result<(), String> {
    let mut items = load_all_flat();
    let now = now_ts()?;
    upsert_token(&mut items, provider, token, now);
    save_all_flat(&items)
}

pub fn load_token(provider: &str) -> Option<String> {
    load_all_flat()
        .into_iter()
        .find(|x| x.provider == provider)
        .map(|x| x.access_token)
}

pub fn clear_token(provider: &str) -> Result<bool, String> {
    let mut items = load_all_flat();
    let removed = remove_token(&mut items, provider);
    save_all_flat(&items)?;
    Ok(removed)
}

pub fn oauth_path() -> PathBuf {
    oauth_file()
}

pub fn mcp_scope_key(server: &str, provider: &str) -> String {
    let s = server.trim().to_ascii_lowercase();
    let p = provider.trim().to_ascii_lowercase();
    format!("mcp:{}:{}", s, p)
}

pub fn save_mcp_token_scoped(
    server: &str,
    provider: &str,
    token: &str,
    scope: &str,
) -> Result<(), String> {
    let key = mcp_scope_key(server, provider);
    if using_legacy_single_path_mode() {
        return save_token(&key, token);
    }
    let scope_kind = parse_scope_kind(scope)?;
    let now = now_ts()?;
    mutate_scope_items(scope_kind, |items| {
        upsert_token(items, &key, token, now);
        Ok(())
    })
}

pub fn load_mcp_token_scoped(server: &str, provider: &str, scope: &str) -> Option<String> {
    let key = mcp_scope_key(server, provider);
    if using_legacy_single_path_mode() {
        return load_token(&key);
    }
    let scope_kind = parse_scope_kind(scope).ok()?;
    read_items(&path_for_scope(scope_kind))
        .into_iter()
        .find(|x| x.provider == key)
        .map(|x| x.access_token)
}

pub fn load_mcp_token_with_scope(server: &str, provider: &str) -> Option<(String, String)> {
    let key = mcp_scope_key(server, provider);
    if using_legacy_single_path_mode() {
        return load_token(&key).map(|token| (token, "legacy".to_string()));
    }
    for scope in [ScopeKind::Session, ScopeKind::Project, ScopeKind::Global] {
        if let Some(token) = read_items(&path_for_scope(scope))
            .into_iter()
            .find(|x| x.provider == key)
            .map(|x| x.access_token)
        {
            return Some((token, scope_name(scope).to_string()));
        }
    }
    None
}

pub fn save_mcp_token(server: &str, provider: &str, token: &str) -> Result<(), String> {
    save_mcp_token_scoped(server, provider, token, "project")
}

pub fn load_mcp_token(server: &str, provider: &str) -> Option<String> {
    load_mcp_token_with_scope(server, provider).map(|v| v.0)
}

pub fn clear_mcp_token_scoped(server: &str, provider: &str, scope: &str) -> Result<bool, String> {
    let key = mcp_scope_key(server, provider);
    if using_legacy_single_path_mode() {
        return clear_token(&key);
    }
    let scope_kind = parse_scope_kind(scope)?;
    mutate_scope_items(scope_kind, |items| Ok(remove_token(items, &key)))
}

pub fn clear_mcp_token(server: &str, provider: &str) -> Result<bool, String> {
    let key = mcp_scope_key(server, provider);
    if using_legacy_single_path_mode() {
        return clear_token(&key);
    }
    let mut changed = false;
    for scope in [ScopeKind::Session, ScopeKind::Project, ScopeKind::Global] {
        let local = mutate_scope_items(scope, |items| Ok(remove_token(items, &key)))?;
        changed = changed || local;
    }
    Ok(changed)
}

pub fn list_mcp_token_providers_scoped(server: &str, scope: &str) -> Result<Vec<String>, String> {
    let server = server.trim().to_ascii_lowercase();
    let prefix = format!("mcp:{}:", server);
    if using_legacy_single_path_mode() {
        let mut providers = load_all_flat()
            .into_iter()
            .filter_map(|item| {
                let key = item.provider.trim().to_ascii_lowercase();
                key.strip_prefix(&prefix).map(|v| v.to_string())
            })
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>();
        providers.sort();
        providers.dedup();
        return Ok(providers);
    }
    let scope_kind = parse_scope_kind(scope)?;
    let mut providers = read_items(&path_for_scope(scope_kind))
        .into_iter()
        .filter_map(|item| {
            let key = item.provider.trim().to_ascii_lowercase();
            key.strip_prefix(&prefix).map(|v| v.to_string())
        })
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>();
    providers.sort();
    providers.dedup();
    Ok(providers)
}

pub fn list_mcp_token_providers(server: &str) -> Vec<String> {
    let server = server.trim().to_ascii_lowercase();
    let prefix = format!("mcp:{}:", server);

    let mut providers = if using_legacy_single_path_mode() {
        load_all_flat()
            .into_iter()
            .filter_map(|item| {
                let key = item.provider.trim().to_ascii_lowercase();
                key.strip_prefix(&prefix).map(|v| v.to_string())
            })
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
    } else {
        let mut out = Vec::new();
        for scope in [ScopeKind::Session, ScopeKind::Project, ScopeKind::Global] {
            out.extend(
                read_items(&path_for_scope(scope))
                    .into_iter()
                    .filter_map(|item| {
                        let key = item.provider.trim().to_ascii_lowercase();
                        key.strip_prefix(&prefix).map(|v| v.to_string())
                    })
                    .filter(|v| !v.is_empty())
                    .collect::<Vec<_>>(),
            );
        }
        out
    };

    providers.sort();
    providers.dedup();
    providers
}

#[cfg(test)]
pub(crate) fn oauth_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::{
        clear_mcp_token, clear_mcp_token_scoped, list_mcp_token_providers,
        list_mcp_token_providers_scoped, load_mcp_token, load_mcp_token_scoped,
        load_mcp_token_with_scope, mcp_scope_key, normalize_scope_value, oauth_path,
        save_mcp_token, save_mcp_token_scoped,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_file(prefix: &str) -> PathBuf {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{}_{}.json", prefix, ts))
    }

    fn with_temp_oauth_path<F>(f: F)
    where
        F: FnOnce(),
    {
        let _mcp_guard = crate::mcp::mcp_state_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _guard = super::oauth_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old_path = std::env::var("ASI_OAUTH_PATH").ok();
        let old_global = std::env::var("ASI_GLOBAL_CONFIG_DIR").ok();
        let old_cwd = std::env::current_dir().ok();

        let file = temp_file("asi_oauth_test");
        std::env::set_var("ASI_OAUTH_PATH", file.display().to_string());
        std::env::remove_var("ASI_GLOBAL_CONFIG_DIR");
        let _ = fs::remove_file(&file);

        f();

        let _ = fs::remove_file(&file);
        match old_path {
            Some(v) => std::env::set_var("ASI_OAUTH_PATH", v),
            None => std::env::remove_var("ASI_OAUTH_PATH"),
        }
        match old_global {
            Some(v) => std::env::set_var("ASI_GLOBAL_CONFIG_DIR", v),
            None => std::env::remove_var("ASI_GLOBAL_CONFIG_DIR"),
        }
        if let Some(cwd) = old_cwd {
            let _ = std::env::set_current_dir(cwd);
        }
    }

    fn with_temp_oauth_scopes<F>(prefix: &str, f: F)
    where
        F: FnOnce(&PathBuf),
    {
        let _mcp_guard = crate::mcp::mcp_state_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _guard = super::oauth_env_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old_path = std::env::var("ASI_OAUTH_PATH").ok();
        let old_global = std::env::var("ASI_GLOBAL_CONFIG_DIR").ok();
        let old_cwd = std::env::current_dir().ok();

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{}_{}", prefix, ts));
        let global_dir = dir.join(".global");
        let session_file =
            std::env::temp_dir().join(format!("asi_oauth_session_{}.json", std::process::id()));

        let _ = fs::create_dir_all(&dir);
        let _ = fs::create_dir_all(&global_dir);
        std::env::remove_var("ASI_OAUTH_PATH");
        std::env::set_var("ASI_GLOBAL_CONFIG_DIR", global_dir.display().to_string());
        let _ = std::env::set_current_dir(&dir);
        let _ = fs::remove_file(&session_file);

        f(&dir);

        let _ = fs::remove_file(&session_file);
        let _ = fs::remove_dir_all(&dir);
        match old_path {
            Some(v) => std::env::set_var("ASI_OAUTH_PATH", v),
            None => std::env::remove_var("ASI_OAUTH_PATH"),
        }
        match old_global {
            Some(v) => std::env::set_var("ASI_GLOBAL_CONFIG_DIR", v),
            None => std::env::remove_var("ASI_GLOBAL_CONFIG_DIR"),
        }
        if let Some(cwd) = old_cwd {
            let _ = std::env::set_current_dir(cwd);
        }
    }

    #[test]
    fn normalize_scope_value_accepts_expected_values() {
        assert_eq!(normalize_scope_value("session"), Some("session"));
        assert_eq!(normalize_scope_value("PROJECT"), Some("project"));
        assert_eq!(normalize_scope_value(" Global "), Some("global"));
        assert_eq!(normalize_scope_value("bad"), None);
    }

    #[test]
    fn mcp_scope_key_normalizes_values() {
        let key = mcp_scope_key("MySrv", "DeepSeek");
        assert_eq!(key, "mcp:mysrv:deepseek");
    }

    #[test]
    fn mcp_token_roundtrip_and_list_providers() {
        with_temp_oauth_path(|| {
            save_mcp_token("srv1", "deepseek", "token-a").expect("save deepseek");
            save_mcp_token("srv1", "openai", "token-b").expect("save openai");
            save_mcp_token("srv2", "deepseek", "token-c").expect("save srv2");

            assert_eq!(
                load_mcp_token("srv1", "deepseek").as_deref(),
                Some("token-a")
            );
            assert_eq!(
                load_mcp_token("srv1", "openai").as_deref(),
                Some("token-b")
            );
            assert_eq!(
                load_mcp_token("srv2", "deepseek").as_deref(),
                Some("token-c")
            );

            let providers = list_mcp_token_providers("srv1");
            assert_eq!(providers, vec!["deepseek".to_string(), "openai".to_string()]);

            let removed = clear_mcp_token("srv1", "deepseek").expect("clear");
            assert!(removed);
            assert!(load_mcp_token("srv1", "deepseek").is_none());
            assert_eq!(
                load_mcp_token("srv1", "openai").as_deref(),
                Some("token-b")
            );
        });
    }

    #[test]
    fn scoped_mcp_tokens_prefer_session_then_project_then_global() {
        with_temp_oauth_scopes("asi_oauth_scope_pref", |_dir| {
            save_mcp_token_scoped("srv", "deepseek", "global-token", "global")
                .expect("save global");
            let first = load_mcp_token_with_scope("srv", "deepseek").expect("load first");
            assert_eq!(first.0, "global-token");
            assert_eq!(first.1, "global");

            save_mcp_token_scoped("srv", "deepseek", "project-token", "project")
                .expect("save project");
            let second = load_mcp_token_with_scope("srv", "deepseek").expect("load second");
            assert_eq!(second.0, "project-token");
            assert_eq!(second.1, "project");

            save_mcp_token_scoped("srv", "deepseek", "session-token", "session")
                .expect("save session");
            let third = load_mcp_token_with_scope("srv", "deepseek").expect("load third");
            assert_eq!(third.0, "session-token");
            assert_eq!(third.1, "session");
        });
    }

    #[test]
    fn clear_mcp_token_scoped_only_affects_selected_scope() {
        with_temp_oauth_scopes("asi_oauth_scope_clear", |_dir| {
            save_mcp_token_scoped("srv", "openai", "global-token", "global")
                .expect("save global");
            save_mcp_token_scoped("srv", "openai", "project-token", "project")
                .expect("save project");

            let removed =
                clear_mcp_token_scoped("srv", "openai", "project").expect("clear project");
            assert!(removed);
            assert_eq!(
                load_mcp_token_scoped("srv", "openai", "project").as_deref(),
                None
            );
            assert_eq!(
                load_mcp_token_scoped("srv", "openai", "global").as_deref(),
                Some("global-token")
            );
            assert_eq!(load_mcp_token("srv", "openai").as_deref(), Some("global-token"));
        });
    }

    #[test]
    fn list_mcp_token_providers_scoped_limits_to_scope() {
        with_temp_oauth_scopes("asi_oauth_scope_list", |_dir| {
            save_mcp_token_scoped("srv", "deepseek", "t1", "project").expect("save p1");
            save_mcp_token_scoped("srv", "openai", "t2", "project").expect("save p2");
            save_mcp_token_scoped("srv", "claude", "t3", "global").expect("save g1");

            let project =
                list_mcp_token_providers_scoped("srv", "project").expect("project list");
            assert_eq!(project, vec!["deepseek".to_string(), "openai".to_string()]);

            let all = list_mcp_token_providers("srv");
            assert_eq!(
                all,
                vec![
                    "claude".to_string(),
                    "deepseek".to_string(),
                    "openai".to_string()
                ]
            );
        });
    }

    #[test]
    fn oauth_path_uses_override_env() {
        with_temp_oauth_path(|| {
            let path = oauth_path();
            let expected = std::env::var("ASI_OAUTH_PATH").expect("env set");
            assert_eq!(path.display().to_string(), expected);
        });
    }
}
