//! Git worktree management as a first-class slash command.
//!
//! Mirrors Claude Code's `EnterWorktree` / `ExitWorktree` UX. Worktrees are
//! created under `<project>/.asi/worktrees/<branch>/` so they stay scoped
//! to the current project and never pollute siblings of the repo root.
//!
//! Slash usage:
//!
//!   /worktree create <branch> [base]
//!   /worktree list
//!   /worktree remove <branch> [--force]
//!   /worktree enter <branch>
//!   /worktree exit [keep|remove]   (default: keep)
//!
//! `enter` swaps the REPL's session cwd to the worktree directory and
//! remembers the original project root so a later `exit keep` can return.
//! `exit remove` deletes the worktree (refusing if there are uncommitted
//! changes unless `--force` is also passed via `--force`).

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub branch: String,
    pub path: PathBuf,
    pub head: String,
    pub bare: bool,
    pub locked: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WorktreeSession {
    /// Original project root the REPL was launched in. Set by `enter` and
    /// cleared by `exit` so a later `exit keep` can restore the cwd.
    pub original_root: Option<PathBuf>,
    /// The branch name the session is currently inside, if any.
    pub current_branch: Option<String>,
}

impl WorktreeSession {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_inside(&self) -> bool {
        self.original_root.is_some()
    }
}

/// Returns Ok(stdout) on success, Err(stderr_or_message) on failure. Always
/// runs in the given working directory.
fn run_git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("failed to spawn git: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let msg = if stderr.is_empty() { stdout } else { stderr };
        return Err(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            msg
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn ensure_inside_git_repo(cwd: &Path) -> Result<(), String> {
    run_git(cwd, &["rev-parse", "--is-inside-work-tree"])
        .map_err(|e| format!("not inside a git work tree: {}", e))?;
    Ok(())
}

/// Resolve where worktrees should live. Defaults to
/// `<repo_top>/.asi/worktrees/<sanitized_branch>`. The branch name is
/// sanitized so a forward-slash branch name becomes a nested directory.
pub fn worktree_path_for(repo_top: &Path, branch: &str) -> PathBuf {
    let mut p = repo_top.join(".asi").join("worktrees");
    for segment in branch.split('/') {
        let seg = sanitize_segment(segment);
        if !seg.is_empty() {
            p.push(seg);
        }
    }
    p
}

fn sanitize_segment(seg: &str) -> String {
    seg.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Locate the top-level repo root by asking git. Returns `Err` if not in
/// a git repo at all.
pub fn repo_top(cwd: &Path) -> Result<PathBuf, String> {
    let out = run_git(cwd, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(out.trim()))
}

pub fn create(
    cwd: &Path,
    branch: &str,
    base: Option<&str>,
) -> Result<WorktreeInfo, String> {
    if branch.trim().is_empty() {
        return Err("branch name is required".to_string());
    }
    ensure_inside_git_repo(cwd)?;
    let top = repo_top(cwd)?;
    let target = worktree_path_for(&top, branch);
    if target.exists() {
        return Err(format!(
            "worktree path already exists: {}",
            target.display()
        ));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create_dir_all {}: {}", parent.display(), e))?;
    }

    // Use `-b` so the new branch is created from `base` (or HEAD) and
    // checked out into the worktree atomically. Falls back to plain
    // checkout if the branch already exists locally.
    let target_str = target.to_string_lossy().to_string();
    let branch_exists = run_git(cwd, &["show-ref", "--verify", "--quiet",
        &format!("refs/heads/{}", branch)]).is_ok();
    if branch_exists {
        run_git(cwd, &["worktree", "add", &target_str, branch])?;
    } else {
        let base_ref = base.unwrap_or("HEAD");
        run_git(
            cwd,
            &["worktree", "add", "-b", branch, &target_str, base_ref],
        )?;
    }

    list(cwd)?
        .into_iter()
        .find(|w| w.path == target)
        .ok_or_else(|| "created worktree but could not find it in `git worktree list`".to_string())
}

pub fn list(cwd: &Path) -> Result<Vec<WorktreeInfo>, String> {
    ensure_inside_git_repo(cwd)?;
    let porcelain = run_git(cwd, &["worktree", "list", "--porcelain"])?;
    Ok(parse_porcelain(&porcelain))
}

pub fn remove(cwd: &Path, branch: &str, force: bool) -> Result<(), String> {
    if branch.trim().is_empty() {
        return Err("branch name is required".to_string());
    }
    ensure_inside_git_repo(cwd)?;
    let top = repo_top(cwd)?;
    let target = worktree_path_for(&top, branch);
    if !target.exists() {
        return Err(format!("worktree path not found: {}", target.display()));
    }
    let target_str = target.to_string_lossy().to_string();
    let mut args = vec!["worktree", "remove", &target_str];
    if force {
        args.push("--force");
    }
    run_git(cwd, &args)?;
    Ok(())
}

pub fn enter(
    cwd: &Path,
    branch: &str,
    session: &mut WorktreeSession,
) -> Result<PathBuf, String> {
    if session.is_inside() {
        return Err("already inside a worktree session; exit it first".to_string());
    }
    let entries = list(cwd)?;
    let entry = entries
        .iter()
        .find(|w| w.branch == format!("refs/heads/{}", branch) || w.branch == branch)
        .ok_or_else(|| format!("no worktree registered for branch '{}'", branch))?;
    if !entry.path.exists() {
        return Err(format!(
            "worktree path is gone on disk: {}",
            entry.path.display()
        ));
    }
    session.original_root = Some(cwd.to_path_buf());
    session.current_branch = Some(branch.to_string());
    Ok(entry.path.clone())
}

pub fn exit(
    session: &mut WorktreeSession,
    action: ExitAction,
) -> Result<ExitOutcome, String> {
    let original = session
        .original_root
        .take()
        .ok_or_else(|| "no worktree session active".to_string())?;
    let branch = session.current_branch.take();
    let outcome = match action {
        ExitAction::Keep => ExitOutcome::Kept {
            original_root: original.clone(),
            branch: branch.clone(),
        },
        ExitAction::Remove { force } => {
            let branch_name = branch.clone().ok_or_else(|| {
                "session has no branch on record; cannot remove worktree".to_string()
            })?;
            remove(&original, &branch_name, force)?;
            ExitOutcome::Removed {
                original_root: original.clone(),
                branch: branch_name,
            }
        }
    };
    Ok(outcome)
}

#[derive(Debug, Clone, Copy)]
pub enum ExitAction {
    Keep,
    Remove { force: bool },
}

#[derive(Debug, Clone)]
pub enum ExitOutcome {
    Kept {
        original_root: PathBuf,
        branch: Option<String>,
    },
    Removed {
        original_root: PathBuf,
        branch: String,
    },
}

fn parse_porcelain(porcelain: &str) -> Vec<WorktreeInfo> {
    let mut out = Vec::new();
    let mut current: Option<WorktreeBuilder> = None;
    for line in porcelain.lines() {
        if line.is_empty() {
            if let Some(b) = current.take() {
                if let Some(info) = b.build() {
                    out.push(info);
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(b) = current.take() {
                if let Some(info) = b.build() {
                    out.push(info);
                }
            }
            current = Some(WorktreeBuilder::new(rest.to_string()));
            continue;
        }
        let Some(b) = current.as_mut() else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("branch ") {
            b.branch = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            b.head = Some(rest.to_string());
        } else if line == "bare" {
            b.bare = true;
        } else if line == "locked" || line.starts_with("locked ") {
            b.locked = true;
        } else if line == "detached" {
            b.branch = Some("(detached)".to_string());
        }
    }
    if let Some(b) = current.take() {
        if let Some(info) = b.build() {
            out.push(info);
        }
    }
    out
}

struct WorktreeBuilder {
    path: String,
    branch: Option<String>,
    head: Option<String>,
    bare: bool,
    locked: bool,
}

impl WorktreeBuilder {
    fn new(path: String) -> Self {
        Self {
            path,
            branch: None,
            head: None,
            bare: false,
            locked: false,
        }
    }
    fn build(self) -> Option<WorktreeInfo> {
        Some(WorktreeInfo {
            branch: self.branch.unwrap_or_else(|| "(unknown)".to_string()),
            path: PathBuf::from(self.path),
            head: self.head.unwrap_or_default(),
            bare: self.bare,
            locked: self.locked,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_porcelain_output() {
        let porcelain = "worktree /repo/main\nHEAD abcd\nbranch refs/heads/main\n\nworktree /repo/.asi/worktrees/feature_x\nHEAD efgh\nbranch refs/heads/feature_x\n";
        let entries = parse_porcelain(porcelain);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, PathBuf::from("/repo/main"));
        assert_eq!(entries[0].branch, "refs/heads/main");
        assert_eq!(entries[1].path, PathBuf::from("/repo/.asi/worktrees/feature_x"));
        assert_eq!(entries[1].branch, "refs/heads/feature_x");
    }

    #[test]
    fn parses_detached_and_bare_entries() {
        let porcelain =
            "worktree /repo/main\nHEAD aaaa\nbranch refs/heads/main\n\nworktree /repo/bare\nbare\n\nworktree /repo/det\nHEAD bbbb\ndetached\n";
        let entries = parse_porcelain(porcelain);
        assert_eq!(entries.len(), 3);
        assert!(entries[1].bare);
        assert_eq!(entries[2].branch, "(detached)");
    }

    #[test]
    fn worktree_path_sanitizes_branch_segments() {
        let p = worktree_path_for(Path::new("/repo"), "feature/some name!");
        // Spaces and `!` are sanitized to `_`; `/` becomes a directory boundary.
        let s = p.to_string_lossy();
        assert!(s.ends_with("/repo/.asi/worktrees/feature/some_name_") || s.replace('\\', "/").ends_with("/repo/.asi/worktrees/feature/some_name_"));
    }

    #[test]
    fn enter_then_exit_keep_restores_state() {
        let mut session = WorktreeSession::new();
        // Manually populate state because we don't want to spin up a real
        // repo here; the public `enter` path is exercised by the integration
        // test in `cli_smoke` if desired.
        session.original_root = Some(PathBuf::from("/orig"));
        session.current_branch = Some("feat".to_string());
        let outcome = exit(&mut session, ExitAction::Keep).unwrap();
        match outcome {
            ExitOutcome::Kept { original_root, branch } => {
                assert_eq!(original_root, PathBuf::from("/orig"));
                assert_eq!(branch.as_deref(), Some("feat"));
            }
            _ => panic!("expected Kept"),
        }
        assert!(!session.is_inside());
    }

    #[test]
    fn exit_without_active_session_errors() {
        let mut session = WorktreeSession::new();
        let err = exit(&mut session, ExitAction::Keep).unwrap_err();
        assert!(err.contains("no worktree session"));
    }

    #[test]
    fn enter_rejects_double_entry() {
        // We don't need real git for this — populate the session and try.
        let mut session = WorktreeSession::new();
        session.original_root = Some(PathBuf::from("/orig"));
        let err = enter(Path::new("."), "x", &mut session).unwrap_err();
        assert!(err.contains("already inside"));
    }
}
