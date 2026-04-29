use crate::security::sanitize_undercover_text;
use std::process::Command;

fn run_git(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if text.trim().is_empty() {
        text = format!("exit={}", output.status.code().unwrap_or(-1));
    }
    if output.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

fn git_commit_message(message: &str) -> Result<String, String> {
    run_git(&["commit", "-m", message])
}

pub fn handle_git_command(args: &str, undercover_mode: bool) -> Result<String, String> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "status" {
        return run_git(&["status", "--short", "--branch"]);
    }
    if trimmed == "diff" {
        return run_git(&["diff"]);
    }
    if trimmed == "branch" {
        return run_git(&["branch", "--show-current"]);
    }
    if trimmed == "log" {
        return run_git(&["log", "--oneline", "-n", "12"]);
    }

    if let Some(message) = trimmed.strip_prefix("commit-msg ") {
        let msg = if undercover_mode {
            sanitize_undercover_text(message.trim())
        } else {
            message.trim().to_string()
        };
        return git_commit_message(&msg);
    }

    if trimmed.starts_with("commit ") && undercover_mode {
        return Err(
            "Undercover mode is on. Use: /git commit-msg <message> (message will be sanitized)"
                .to_string(),
        );
    }

    run_git(&trimmed.split_whitespace().collect::<Vec<_>>())
}
