use std::env;

use super::adapter::{SandboxAdapter, SandboxPreflightContext};

/// Local sandbox preflight policy.
///
/// Historical default was "block all network unless ASI_SANDBOX_ALLOW_NETWORK=1".
/// That was the right default for `bash curl` (data-exfil risk via shell), but
/// wrong for `web_search` / `web_fetch` — those tools' entire purpose is HTTP,
/// and blocking them by default produced a useless "search doesn't work until
/// you remember an env var" UX.
///
/// New defaults:
///   - `web_search`, `web_fetch`: ALLOWED by default (they are explicit network
///     tools that the operator chose to expose to the model). Set
///     `ASI_SANDBOX_BLOCK_WEB_TOOLS=1` to opt back into the old strict policy.
///   - `bash` with networked subcommands (curl/wget/IRM/...): BLOCKED by
///     default; `ASI_SANDBOX_ALLOW_NETWORK=1` lifts the block, same as before.
#[derive(Debug, Clone)]
pub struct LocalSandbox {
    allow_network: bool,
    block_web_tools: bool,
}

impl LocalSandbox {
    pub fn from_env() -> Self {
        let allow_network = matches!(
            env::var("ASI_SANDBOX_ALLOW_NETWORK")
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str(),
            "1" | "true" | "on"
        );
        let block_web_tools = matches!(
            env::var("ASI_SANDBOX_BLOCK_WEB_TOOLS")
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str(),
            "1" | "true" | "on"
        );

        Self {
            allow_network,
            block_web_tools,
        }
    }

    fn command_looks_networked(command: &str) -> bool {
        let lower = command.to_ascii_lowercase();
        [
            "curl ",
            "wget ",
            "invoke-webrequest",
            "irm ",
            "http://",
            "https://",
            "nc ",
            "ncat ",
            "ping ",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }
}

impl SandboxAdapter for LocalSandbox {
    fn name(&self) -> &'static str {
        "local"
    }

    fn preflight(&self, ctx: &SandboxPreflightContext<'_>) -> Result<(), String> {
        let _ = ctx.permission_mode;

        if self.allow_network {
            return Ok(());
        }

        if matches!(ctx.tool, "web_search" | "web_fetch") {
            // Allow by default. Only block when the operator opts into the
            // strict legacy policy via ASI_SANDBOX_BLOCK_WEB_TOOLS=1.
            if self.block_web_tools {
                return Err("sandbox(local): web tools are blocked (unset ASI_SANDBOX_BLOCK_WEB_TOOLS or set ASI_SANDBOX_ALLOW_NETWORK=1 to allow)".to_string());
            }
            return Ok(());
        }

        if ctx.tool == "bash" && Self::command_looks_networked(ctx.args) {
            return Err("sandbox(local): networked shell command is blocked (set ASI_SANDBOX_ALLOW_NETWORK=1 to allow)".to_string());
        }

        Ok(())
    }
}
