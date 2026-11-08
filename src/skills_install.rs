//! `asi skills install|uninstall|status`: manage the asi-skills safety guard
//! as a user-level PreToolUse hook in `~/.asi/hooks.json`.
//!
//! Install runs the guard once against a known-bad and a known-good call
//! before writing anything. A hook that fails to start denies every tool
//! call (hooks fail closed), so an unverified install could lock the agent.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Marks handlers owned by this installer so reinstall/uninstall never
/// touch hooks the user wrote by hand.
const OWNER_TAG: &str = "asi-skills";

fn hook_script_path(skills_dir: &Path) -> PathBuf {
    skills_dir.join("asi_skills").join("hook.py")
}

fn hook_command(python: &str, script: &Path) -> String {
    let script = script.display();
    if cfg!(windows) {
        // ASI runs hook commands through PowerShell, which needs the call
        // operator to execute a quoted path.
        format!("& \"{}\" \"{}\"", python, script)
    } else {
        format!("\"{}\" \"{}\"", python, script)
    }
}

/// Run the guard directly with a payload and return its `allow` decision.
fn probe_guard(python: &str, script: &Path, tool: &str, args: &str) -> Result<bool, String> {
    let payload = json!({"tool": tool, "args": args}).to_string();
    let out = Command::new(python)
        .arg(script)
        .env("ASI_HOOK_INPUT_JSON", payload)
        .output()
        .map_err(|e| format!("failed to start `{}`: {}", python, e))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().last().unwrap_or("").trim();
    let v: Value = serde_json::from_str(line).map_err(|_| {
        format!(
            "guard did not print a JSON decision (stdout: {:?}, stderr: {:?})",
            line,
            String::from_utf8_lossy(&out.stderr).trim()
        )
    })?;
    v.get("allow")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("guard output has no boolean `allow`: {}", line))
}

fn verify_guard(python: &str, script: &Path) -> Result<(), String> {
    if !script.is_file() {
        return Err(format!(
            "{} not found; pass the asi-skills repository directory",
            script.display()
        ));
    }
    if probe_guard(python, script, "read_file", ".env")? {
        return Err("self-test failed: guard allowed reading .env".to_string());
    }
    if !probe_guard(python, script, "bash", "cargo test")? {
        return Err("self-test failed: guard blocked `cargo test`".to_string());
    }
    Ok(())
}

fn load_config(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(json!({"handlers": []}));
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Ok(json!({"handlers": []}));
    }
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not valid JSON ({}); fix or remove it first", path.display(), e))?;
    if !v.is_object() {
        return Err(format!("{} must contain a JSON object", path.display()));
    }
    Ok(v)
}

fn without_owned_handlers(config: &mut Value) -> usize {
    let Some(obj) = config.as_object_mut() else {
        return 0;
    };
    let handlers = obj.entry("handlers").or_insert_with(|| json!([]));
    let Some(list) = handlers.as_array_mut() else {
        *handlers = json!([]);
        return 0;
    };
    let before = list.len();
    list.retain(|h| h.get("owner").and_then(Value::as_str) != Some(OWNER_TAG));
    before - list.len()
}

fn write_config(path: &Path, config: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| e.to_string())
}

fn config_path() -> Result<PathBuf, String> {
    crate::runtime::user_hooks_config_path()
        .ok_or_else(|| "cannot locate home directory (set ASI_USER_HOOKS_PATH)".to_string())
}

pub fn install(skills_dir: &Path, python: &str) -> Result<String, String> {
    let skills_dir = fs::canonicalize(skills_dir)
        .map_err(|e| format!("{}: {}", skills_dir.display(), e))?;
    let skills_dir = PathBuf::from(
        skills_dir.to_string_lossy().trim_start_matches(r"\\?\").to_string(),
    );
    let script = hook_script_path(&skills_dir);
    verify_guard(python, &script)?;

    let path = config_path()?;
    let mut config = load_config(&path)?;
    let replaced = without_owned_handlers(&mut config);
    config["handlers"].as_array_mut().expect("handlers array").push(json!({
        "owner": OWNER_TAG,
        "event": "PreToolUse",
        "script": hook_command(python, &script),
        "json_protocol": true,
        "timeout_secs": 10,
        "failure_policy": "fail-closed",
    }));
    write_config(&path, &config)?;
    Ok(format!(
        "installed asi-skills guard ({}) -> {}\nself-test passed: blocks reading .env, allows `cargo test`\nactive for new ASI Code sessions; disable with ASI_HOOKS_ENABLED=0 or `asi skills uninstall`",
        if replaced > 0 { "updated" } else { "new" },
        path.display()
    ))
}

pub fn uninstall() -> Result<String, String> {
    let path = config_path()?;
    if !path.exists() {
        return Ok("asi-skills guard is not installed".to_string());
    }
    let mut config = load_config(&path)?;
    let removed = without_owned_handlers(&mut config);
    if removed == 0 {
        return Ok("asi-skills guard is not installed".to_string());
    }
    let remaining = config["handlers"].as_array().map(Vec::len).unwrap_or(0);
    if remaining == 0 && config.as_object().map(|o| o.len() == 1).unwrap_or(false) {
        fs::remove_file(&path).map_err(|e| e.to_string())?;
    } else {
        write_config(&path, &config)?;
    }
    Ok(format!("removed asi-skills guard from {}", path.display()))
}

pub fn status() -> Result<String, String> {
    let path = config_path()?;
    let config = load_config(&path)?;
    let owned = config["handlers"]
        .as_array()
        .map(|l| {
            l.iter()
                .filter(|h| h.get("owner").and_then(Value::as_str) == Some(OWNER_TAG))
                .count()
        })
        .unwrap_or(0);
    let disabled = std::env::var("ASI_HOOKS_ENABLED")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(false);
    Ok(format!(
        "user_hooks={} asi_skills_guard={} hooks_disabled_by_env={} credential_protection={}",
        path.display(),
        if owned > 0 { "installed" } else { "not-installed" },
        disabled,
        if crate::security::secret_files_allowed_by_env() { "off" } else { "on" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reinstall_replaces_only_owned_handlers() {
        let mut config = json!({"handlers": [
            {"event": "PreToolUse", "script": "user-hook"},
            {"owner": "asi-skills", "event": "PreToolUse", "script": "old"}
        ]});
        assert_eq!(without_owned_handlers(&mut config), 1);
        let list = config["handlers"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["script"], "user-hook");
    }

    #[test]
    fn hook_command_quotes_paths_for_the_host_shell() {
        let cmd = hook_command(r"C:\Program Files\Python\python.exe", Path::new(r"D:\s\asi_skills\hook.py"));
        if cfg!(windows) {
            assert!(cmd.starts_with("& \""), "{}", cmd);
        }
        assert!(cmd.contains("hook.py\""), "{}", cmd);
    }

    #[test]
    fn verify_rejects_missing_guard() {
        let err = verify_guard("python", Path::new("Z:/definitely/missing/hook.py")).unwrap_err();
        assert!(err.contains("not found"), "{}", err);
    }
}
