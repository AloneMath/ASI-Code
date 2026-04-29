# ASI Code

**A modern terminal coding agent in Rust.** Single binary. No Node.js runtime required, and no npm install step. Runs against **DeepSeek**, **OpenAI**, or **Claude** with the same workflow.

Inspired by modern coding-agent CLIs, with a similar workflow and UX: REPL with slash commands, streaming output, auto-tool loop, work / code / secure / review modes, sub-agents, MCP server support, plugin system, agent skills, cron jobs, git worktrees, sandbox + audit log, and a 60+ command surface.

```powershell
# 30-second quickstart (Windows / PowerShell)
$env:DEEPSEEK_API_KEY = "sk-..."
cargo run --release -- repl --provider deepseek --model deepseek-v4-pro --project D:\your-project --no-setup
```

---

ASI Code is a Rust-built terminal coding agent with a workflow similar to modern coding-agent CLIs.

This project is independent and is not affiliated with, endorsed by, or sponsored by Anthropic, OpenAI, Anysphere, or any other model/tool provider.

## Implemented Features
- Interactive REPL with slash commands and streaming output
- REPL startup provider wizard (choose provider, API key, model before chat)
- API key safety: wizard keys are session-only by default unless explicitly persisted
- Provider support: OpenAI, DeepSeek, Claude (API key + Claude OAuth token)
- Tool system: `bash`, `read_file`, `write_file`, `edit_file`, `glob_search`, `grep_search`
- Compact read display: `read_file` defaults to chunked ranges and UI shows a concise read summary instead of dumping full file content
- Web tools: `web_search`, `web_fetch`
- Auto tool loop: parses assistant `/toolcall ...` lines and executes them (toggle with `/auto on|off`)
- Work mode: `/work <task>` or `/code <task>` injects workspace snapshot and drives tool-first project editing
- Intent-aware coding mode: non-command prompts that look like coding requests auto-enter workspace coding flow (simple greetings do not)
- Security fix mode: `/secure <task>` focuses on vulnerability and hardening fixes
- Review mode: `/review <task>` focuses on bugs, regressions, risks, and missing tests
- Session persistence: save, list, resume
- Change tracking: per-turn `changed_file=...` feedback and `/changes` for session summary (`/changes clear` to reset)
- Todo tracking: add/list/done/remove
- Project memory notes and notebook markdown cells
- Git integration command wrapper
- MCP server management (add/rm/auth/oauth/config/show/start/list/stop)
- Subagent tool-loop closure: `/agent` runs in an isolated runtime and can continue through tool-call loops (toggle: `ASI_SUBAGENT_TOOL_LOOP`)
- Cost tracking and status line (tokens + USD estimate)
- Markdown ANSI rendering toggle
- Model aliases: `opus`, `sonnet`, `haiku`
- Privacy and safety controls:
  - local telemetry JSONL (disabled by default)
  - tool-input telemetry redaction by default
  - safe shell guard for dangerous commands
  - undercover mode for sanitized commit messages
  - auto-review guard layer: `off|warn|block` with severity threshold `critical|high|medium|low`
  - feature killswitches (`web_tools`, `bash_tool`, `subagent`, `research`)
  - explicit remote policy sync (`/policy sync`) with safe-mode guardrails
- Theme setup flow:
  - `theme` / `/theme` text style chooser with preview
  - `setup` / `/setup` API key + model + wallet setup wizard
  - `scan` / `/scan` repository pattern scan panel (auto-detects project profile: Rust/JavaScript/Python when no patterns are provided, supports deep mode: `--deep` or `--deep=N`)

## Quick Start

```powershell
cd "D:\Code\Rust"
cargo run --offline -- repl --project "D:\Code\YourProject" --provider deepseek --model deepseek-v4-pro
```

## Standalone Windows Install (No npm/pnpm)

Install ASI Code as a standalone app (`asi.exe`) with one command:

```powershell
cd "D:\Code\Rust"
powershell -ExecutionPolicy Bypass -File .\scripts\windows\install_asi.ps1 -BuildRelease
```

Package a distributable release for users (zip + one-click installer exe):

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\windows\package_release.ps1
```

Generated artifacts:
- `dist\asi-code-installer-<version>.exe` (one-click installer, creates Start Menu entries)
- `dist\asi-code-installer-<version>.exe.sha256.txt`
- `dist\asi-code-windows-x64-<version>.zip`
- `dist\asi-code-windows-x64-<version>.zip.sha256.txt`

Inside the ZIP root:
- `asi.exe` (portable launcher, double-click entry)
- `ASI Launcher.exe` (same launcher name for clarity)
- `start_asi.cmd` (script-based launcher)
- `bin\asi.exe` (CLI engine binary)

Detailed guide: INSTALL_WINDOWS.md

## VS Code PowerShell Quick Launch (Windows)

```powershell
cd "D:\Code\Rust"
.\start_asi.ps1
```

What `start_asi.ps1` does:
- Prompts provider: OpenAI / DeepSeek / Claude
- Prompts matching API key env (`OPENAI_API_KEY` / `DEEPSEEK_API_KEY` / `ANTHROPIC_API_KEY`)
- Prompts project path
- Runs: `cargo run --release -- repl --provider <provider> --project <path> --no-setup`

Optional flags:

```powershell
.\start_asi.ps1 -Provider openai
.\start_asi.ps1 -Provider claude -ProjectPath "D:\Code\YourProject"
.\start_asi.ps1 -WithSetup
```

Double-click launcher:

```powershell
.\start_asi.cmd
```

## Ubuntu/WSL Quick Launch

```bash
cd /mnt/d/Code/Rust
chmod +x ./start_asi.sh
./start_asi.sh /mnt/d/Code/Rust
```

Skip startup setup wizard:

```bash
./start_asi.sh /mnt/d/Code/Rust --no-setup
```

Optional API keys before launch:

```bash
export OPENAI_API_KEY="..."
export DEEPSEEK_API_KEY="..."
export ANTHROPIC_API_KEY="..."
export ASI_AUTO_REVIEW_MODE="warn"
export ASI_AUTO_REVIEW_SEVERITY_THRESHOLD="medium"
```


## VS Code Workspace Behavior

- Run ASI Code from your project folder (or pass `--project`) and it will read/write directly in that workspace.
- Agent file edits are written to the same files you see in VS Code Explorer.
- Agent command execution happens in terminal via `/run <command>` or `/toolcall bash <command>`.

## Useful Commands

```powershell
cargo run --offline -- version
cargo run --offline -- api-page
cargo run --offline -- theme
cargo run --offline -- theme --select 1
cargo run --offline -- setup
cargo run --offline -- scan --deep
cargo run --offline -- scan --deep-limit 20 "**/*.py"
cargo run --offline -- tokenizer doctor --repo "D:\Code\rustbpe"
cargo run --offline -- tokenizer build --repo "D:\Code\rustbpe" --timeout-secs 900
cargo run --offline -- tokenizer train --repo "D:\Code\rustbpe" --input "D:\data\corpus.txt" --vocab-size 8192 --timeout-secs 1800 --auto-build
cargo run --offline -- wiki init --root "D:\Code\my-wiki"
cargo run --offline -- wiki ingest --root "D:\Code\my-wiki" --source "D:\Code\my-wiki\raw-input\paper.md" --title "Paper Notes"
cargo run --offline -- wiki query --root "D:\Code\my-wiki" --question "what changed in method section" --top 5 --save
cargo run --offline -- wiki lint --root "D:\Code\my-wiki" --write-report
cargo run --offline -- prompt "review this repo" --project "D:\Code\YourProject" --output-format text
cargo run --offline -- prompt "review this repo" --output-format json
cargo run --offline -- prompt "review this repo" --output-format jsonl
cargo run --offline -- repl --speed sprint
cargo run --offline -- prompt "review this repo" --speed deep --output-format json
cargo run --offline -- bench --provider deepseek --model deepseek-reasoner --project "D:\Code\Rust" --suite core --agent-max-steps 8 --repeat 1 --out-dir bench_reports
cargo run --release -- bench --provider deepseek-reasoner --model deepseek-reasoner --project "D:\Code\Rust" --suite gpt53-proxy6d --agent-max-steps 4 --repeat 3 --out-dir bench_reports
cargo run --offline -- sessions --limit 20
cargo run --offline -- self-update --source "D:\Code\Rust\dist\asi-code-windows-x64-0.3.0.zip"
cargo run --offline -- self-update --source "https://example.com/asi-code-windows-x64-0.3.0.zip" --sha256 "<sha256>"
cargo run --offline -- config --telemetry-enabled true --safe-shell-mode true --undercover-mode true
cargo run --offline -- config --disable-web-tools true --disable-bash-tool false
cargo run --offline -- config --auto-review-mode warn --auto-review-severity-threshold medium
cargo run --offline -- config --allow-tool-rule read_file --allow-tool-rule "bash:git "
cargo run --offline -- config --deny-tool-rule "bash:rm -rf" --clear-tool-rules
cargo run --offline -- config --path-restriction-enabled true --additional-dir "D:\Code\shared"
cargo run --offline -- config --execution-speed sprint
cargo run --offline -- mcp list --json
cargo run --offline -- mcp add local-agent "python -m mcp_server" --json
cargo run --offline -- mcp config scope local-agent project --json
cargo run --offline -- mcp config trust local-agent on --json
cargo run --offline -- plugin list --json
cargo run --offline -- plugin add demo-plugin "D:\Code\my-plugin" --json
cargo run --offline -- plugin trust demo-plugin manual --json
cargo run --offline -- plugin trust demo-plugin hash --json
cargo run --offline -- plugin verify demo-plugin --json
powershell -ExecutionPolicy Bypass -File .\scripts\windows\benchmark_regression.ps1 -Project "D:\Code" -Agent
```

## REPL Slash Commands
- `/help`, `/status`, `/project [path]`, `/cost`, `/compact`, `/clear`, `/changes`, `/changes clear`, `/changes tail [n]`, `/changes file <pattern>`, `/changes export <path> [md|json] [n]`, `/exit`
- `/provider <name>`, `/model <name>`, `/profile [standard|strict]`, `/speed [sprint|deep]`, `/permissions list|mode <mode>|allow <rule>|deny <rule>|rm-allow <rule>|rm-deny <rule>|clear-rules|path-restriction on|off|dirs|add-dir <path>|rm-dir <path>|clear-dirs|temp-list|temp-allow <rule>|temp-deny <rule>|temp-rm-allow <rule>|temp-rm-deny <rule>|temp-clear|temp-dirs|temp-add-dir <path>|temp-rm-dir <path>|temp-clear-dirs|temp-next-allow <rule>|temp-next-add-dir <path>|temp-next-clear|auto-review <off|warn|block>|auto-review-threshold <critical|high|medium|low>`
- `/theme`, `/setup`, `/scan [patterns...] [--deep|--deep=N]`, `/project <path>`, `/import <path>`, `/run <command>`, `/work <task>`, `/code <task>`, `/secure <task>`, `/workmode on|off`, `/auto on|off`, `/autoresearch help|doctor [--repo <path>]|init [--repo <path>]|run [--repo <path>] [--iterations <n>] [--timeout-secs <sec>] [--log <path>] [--description <text>] [--status <keep|discard|crash>]`, `/tokenizer help|doctor [--repo <path>]|build [--repo <path>] [--debug] [--timeout-secs <sec>]|train --input <corpus.txt> [--repo <path>] [--vocab-size <n>] [--output <path>] [--pattern <regex>] [--name <name>] [--python-cmd <cmd>] [--timeout-secs <sec>] [--auto-build|--no-auto-build]`
- `/runtime-profile [safe|fast|status]` (one-command runtime safety preset switch)
- `/markdown on|off`, `/think on|off`
- `/privacy status|telemetry on|off|tool-details on|off|undercover on|off|safe-shell on|off`
- `/flags list|set <web_tools|bash_tool|subagent|research> on|off`
- `/policy sync`
- `/tools`, `/toolcall <tool> <args>`
- `/audit tail [n] | /audit stats [n] | /audit tools [n] | /audit reasons [n] | /audit export <path> [md|json] [n] | /audit export-last [dir] [md|json|both] [n]` (tail, summaries, and export report)
- `/sessions [limit]`, `/resume <id>`, `/save`, `/checkpoint [status|on|off|save|load|clear]`
- `/oauth login <token>`, `/oauth logout`
- `/todo ...`, `/memory ...`, `/git ...`, `/mcp [list [--json]|add <name> <command> [--scope <session|project|global>] [--json]|rm <name> [--json]|show <name> [--json]|auth <name> <none|bearer|api-key|basic> [value] [--json]|oauth login <name> <provider> <token> [--scope <session|project|global>] [--link-auth] [--json]|oauth status <name> [provider] [--scope <session|project|global>] [--json]|oauth logout <name> [provider] [--scope <session|project|global>] [--json]|config <name> <key> <value> [--json]|config rm <name> <key> [--json]|config scope <name> <session|project|global> [--json]|config trust <name> <on|off> [--json]|export <path.json> [--json]|import <path.json> [merge|replace] [--json]|start <name> [command] [--json]|stop <name> [--json]]`, `/plugin [list [--json]|add <name> <path> [--json]|rm <name> [--json]|show <name> [--json]|enable <name> [--json]|disable <name> [--json]|trust <name> <manual|hash> [hash] [--json]|verify <name> [--json]|config set <name> <key> <value> [--json]|config rm <name> <key> [--json]|export <path.json> [--json]|import <path.json> [merge|replace] [--json]|market [--json]]`, `/notebook ...`, `/agent [spawn [--json|--jsonl] <task>|send [--json|--jsonl] [--interrupt] [--no-context] <id> <task>|wait [--json|--jsonl] [id] [timeout_secs]|list [--json|--jsonl]|status [--json|--jsonl] <id>|log [--json|--jsonl] <id> [--tail <n>]|retry [--json|--jsonl] <id>|cancel [--json|--jsonl] <id>|close [--json|--jsonl] <id>|view [foreground|background|all]|front|back]`
- Undercover commit helper: `/git commit-msg <message>`

## Prompt Output Formats

- `--output-format text`: human-readable output
- `--output-format json`: single JSON object (stable machine payload)
- `--output-format jsonl`: line-delimited JSON events (`schema_version=1`) for CI/automation pipelines
  - `prompt.result`
  - `prompt.changed_files`
  - `prompt.native_tool_calls`
  - `prompt.auto_validation`
  - `prompt.review`
  - `prompt.runtime`

## Agent JSON Schema (Stable v1)

When using JSON-enabled `/agent` commands, output uses a stable envelope:

```json
{
  "schema_version": "1",
  "command": "<spawn|send|list|status|wait|log|retry|cancel|close>",
  "agent": { "...": "..." }
}
```

Compatibility policy:
- `schema_version=1` remains stable for backward-compatible additions.
- Any breaking field rename/removal or shape change requires a schema version bump.

## Agent JSONL Event Stream (Stable v1)

When using `/agent ... --jsonl`, each command emits one JSONL event row:

```json
{
  "schema_version": "1",
  "event": "agent.<spawn|send|list|status|wait|log|retry|cancel|close>",
  "data": {
    "command": "<spawn|send|list|status|wait|log|retry|cancel|close>",
    "agent": { "...": "same payload shape as --json agent field" }
  }
}
```

### Commands and Shapes

- `/agent spawn --json <task>` (or `--jsonl`)
  - optional profile/run mode: `/agent spawn --json --profile <name> [--background|--foreground] <task>`
  - `agent`: `id`, `status` (`running`), `provider`, `model`, `run_mode` (`foreground|background`), `task`
- `/agent list --json` (or `--jsonl`)
- `/agent list --json [--scope <foreground|background|all>] [--profile <name>] [--skill <name>]`
  - `agent`: `count`, `filters_applied` (`scope`, `profile`, `skill`), `diagnostics` (`filters`, `total_items`, `counts`, `run_modes`, `timings`), `items[]`
  - each item: `id`, `status`, `provider`, `model`, `run_mode`, `turns`, `interrupted_count`, `last_interrupted_at_ms`, `started_at_ms`, `finished_at_ms`, `task`, `preview`
- `/agent status --json <id>` (or `--jsonl`)
  - `agent`: `id`, `status`, `provider`, `model`, `allow_rules`, `deny_rules`, `rules_source`, `diagnostics` (`rules_source`, `allow_rule_count`, `deny_rule_count`), `run_mode`, `turns`, `interrupted_count`, `last_interrupted_at_ms`, `started_at_ms`, `finished_at_ms`, `age_ms`, `duration_ms`, `task`, `preview`
  - unknown id: `agent.id`, `agent.status=unknown`
- `/agent wait --json [id] [timeout_secs]` (or `--jsonl`)
  - finished: `agent.id`, `provider`, `model`, `run_mode`, `task`, `started_at_ms`, `finished_at_ms`, `status` (`done|error`), `ok`, `result`
  - timeout: `agent.status=timeout`, `timeout_secs`, `message`
  - idle: `agent.status=idle`, `message`
- `/agent close --json <id>` (or `--jsonl`)
  - `agent`: `id`, `status` (`closed`), `message`
- `/agent cancel --json <id>` (or `--jsonl`)
  - `agent`: `id`, `status` (`cancelled|closed|completed|failed|unknown`), `message`
- `/agent retry --json <id>` (or `--jsonl`)
  - `agent`: `id`, `status`, `provider`, `model`, `turns`, `run_mode`, `task`, `message`
- `/agent log --json <id> [--tail <n>]` (or `--jsonl`)
  - `agent`: `id`, `status`, `provider`, `model`, `run_mode`, `turns`, `events_total`, `tail`, `items[]`
  - each log item: `at_ms`, `event`, `message`
- `/agent send --json [--interrupt] [--no-context] <id> <task>` (or `--jsonl`)
  - `agent`: `id`, `status`, `provider`, `model`, `run_mode`, `turns`, `interrupted_count`, `last_interrupted_at_ms`, `started_at_ms`, `finished_at_ms`, `task`, `context` (`append|reset`), `interrupt`, `message`

### Quick Examples

```powershell
# Spawn
/agent spawn --json "inspect src/main.rs"

# Spawn with agent profile (loaded from ./agent_profiles.json)
/agent spawn --json --profile safe-review "inspect src/main.rs"

# Spawn as background task (for /agent view background)
/agent spawn --json --background "inspect src/main.rs"

# List
/agent list --json
/agent list --json --scope background --profile safe-review --skill review

# Send
/agent send --json sa-1 "continue with tests"

# Status
/agent status --json sa-1

# Wait
/agent wait --json sa-1 30

# Close
/agent close --json sa-1

# Cancel running task
/agent cancel --json sa-1

# Retry last task for one subagent
/agent retry --json sa-1

# Show latest subagent events
/agent log --json sa-1 --tail 20
```

## MCP JSON Schema (Stable v1)

When using JSON-enabled `/mcp` commands, output uses a stable envelope:

```json
{
  "schema_version": "1",
  "command": "<mcp_list|mcp_show|mcp_add|mcp_rm|mcp_auth|mcp_config_set|mcp_config_rm|mcp_export|mcp_import|mcp_start|mcp_stop>",
  "mcp": { "...": "..." }
}
```

Compatibility policy:
- `schema_version=1` remains stable for backward-compatible additions.
- Any breaking field rename/removal or shape change requires a schema version bump.

### Agent Profiles (`agent_profiles.json`)

`/agent spawn --profile <name> <task>` can apply project-local defaults from `./agent_profiles.json`.

Supported fields:
- `provider`
- `model`
- `permission_mode`
- `allowed_tools` (tool whitelist; maps to allow rules in subagent runtime)
- `denied_tools` (tool deny list; maps to deny rules in subagent runtime)
- `default_skills` (skill labels attached to subagent metadata)
- `disable_web_tools`
- `disable_bash_tool`

Example:

```json
{
  "profiles": {
    "safe-review": {
      "provider": "deepseek",
      "model": "deepseek-reasoner",
      "permission_mode": "on-request",
      "allowed_tools": ["read_file", "glob_search", "grep_search"],
      "denied_tools": ["bash"],
      "default_skills": ["review", "security"],
      "disable_web_tools": true,
      "disable_bash_tool": false
    },
    "fast-code": {
      "provider": "openai",
      "model": "gpt-5.3-codex",
      "permission_mode": "workspace-write"
    }
  }
}
```

Notes:
- `allowed_tools` is enforced as allow-rules in the subagent runtime. Tools outside the list are blocked.
- `denied_tools` is enforced as deny-rules in the subagent runtime. Matching tools are blocked even if broadly allowed.
- `default_skills` is surfaced in agent JSON outputs (`spawn/list/status/send/wait`) and logged for traceability.

### Commands and Shapes

- `/mcp list --json`
  - `mcp`: `count`, `diagnostics` (`trusted_count`, `untrusted_count`, `scope_counts`), `items[]`
- each item: `name`, `status`, `pid`, `command`, `scope`, `trusted`, `auth_type`, `auth_value`, `config[]` (`key`, `value`)
- `/mcp show <name> --json`
  - found: `mcp` server record shape (same fields as list item) plus `diagnostics` (`has_auth_value`, `config_key_count`, `scope`, `trusted`, `oauth_provider`, `oauth_token_present`)
  - missing: `mcp.name`, `mcp.status=unknown`
- `/mcp add <name> <command> [--scope <session|project|global>] --json`
  - `mcp`: `added` (bool), `scope`, `server` (server record)
- `/mcp rm <name> --json`
  - `mcp`: `name`, `changed` (bool)
- `/mcp auth <name> <none|bearer|api-key|basic> [value] --json`
  - `mcp`: `name`, `auth_type`, `value_set` (bool)
- `/mcp oauth login <name> <provider> <token> [--scope <session|project|global>] [--link-auth] --json`
  - `mcp`: `name`, `provider`, `scope`, `saved` (bool), `linked_auth` (bool)
- `/mcp oauth status <name> [provider] [--scope <session|project|global>] --json`
  - `mcp`: `name`, `provider`, `token_present` (bool), `token_scope`, `request_scope`, `stored_providers` (string array)
- `/mcp oauth logout <name> [provider] [--scope <session|project|global>] --json`
  - `mcp`: `name`, `provider`, `scope`, `removed` (bool)
- `/mcp config <name> <key> <value> --json`
  - `mcp`: `name`, `key`
- `/mcp config rm <name> <key> --json`
  - `mcp`: `name`, `key`, `changed` (bool)
- `/mcp config scope <name> <session|project|global> --json`
  - `mcp`: `name`, `scope`
- `/mcp config trust <name> <on|off> --json`
  - `mcp`: `name`, `trusted` (bool)
- `/mcp export <path.json> --json`
  - `mcp`: `path`
- `/mcp import <path.json> [merge|replace] --json`
  - `mcp`: `path`, `mode`, `total_servers`
- `/mcp start <name> [command] --json`
  - success: `mcp`: `started` (bool), `server` (server record)
  - blocked/error: `mcp`: `started=false`, `name`, `error`
- `/mcp stop <name> --json`
  - `mcp`: `name`, `changed` (bool)

### Quick Examples

```powershell
# List
/mcp list --json

# Show
/mcp show local-agent --json

# Add
/mcp add local-agent "python -m mcp_server" --json

# OAuth (stored token for MCP server scope)
/mcp oauth login local-agent deepseek sk-example --scope project --link-auth --json
/mcp oauth status local-agent deepseek --scope project --json
/mcp oauth logout local-agent deepseek --scope project --json

# Start / Stop
/mcp start local-agent --json
/mcp stop local-agent --json
```

Equivalent top-level CLI subcommands:

```powershell
asi mcp list --json
asi mcp add local-agent "python -m mcp_server" --json
asi mcp config scope local-agent project --json
asi mcp config trust local-agent on --json
asi mcp oauth status local-agent deepseek --scope project --json
```

### MCP Trust and Publish Metadata Policy

Server records support optional publish metadata via `/mcp config`:
- `source`
- `version`
- `signature`

Accepted formats:
- `source` must start with one of: `https://`, `http://`, `git+`, `file://`, `local://`
- `signature` must start with `sha256:` or `sig:`
- `version` must be non-empty when set

Examples:

```powershell
/mcp config local-agent source "https://example.com/org/mcp-server" --json
/mcp config local-agent version "1.2.3" --json
/mcp config local-agent signature "sha256:abcd1234" --json

asi mcp config local-agent source "https://example.com/org/mcp-server" --json
asi mcp config local-agent version "1.2.3" --json
asi mcp config local-agent signature "sig:publisher-v1" --json
```

## Plugin JSON Schema (Stable v1)

When using JSON-enabled `/plugin` commands (or `asi plugin ... --json`), output uses:

```json
{
  "schema_version": "1",
  "command": "<plugin_list|plugin_add|plugin_rm|plugin_show|plugin_enable|plugin_disable|plugin_trust|plugin_config_set|plugin_config_rm|plugin_export|plugin_import>",
  "plugin": { "...": "..." }
}
```

Quick examples:

```powershell
/plugin list --json
/plugin show demo-plugin --json
/plugin trust demo-plugin manual --json
/plugin trust demo-plugin hash --json
/plugin verify demo-plugin --json

asi plugin list --json
asi plugin add demo-plugin "D:\Code\my-plugin" --json
asi plugin trust demo-plugin manual --json
asi plugin trust demo-plugin hash --json
asi plugin verify demo-plugin --json
```

`trust ... hash` stores a deterministic SHA-256 of the plugin directory contents (not only manifest),
so changing plugin code/config invalidates trust until re-trusted.

### Plugin Trust and Publish Metadata Policy

Plugin records support optional publish metadata via `/plugin config set`:
- `source`
- `version`
- `signature`

Accepted formats:
- `source` must start with one of: `https://`, `http://`, `git+`, `file://`, `local://`
- `signature` must start with `sha256:` or `sig:`
- `version` must be non-empty when set

Examples:

```powershell
/plugin config set demo-plugin source "https://example.com/org/demo-plugin" --json
/plugin config set demo-plugin version "0.9.0" --json
/plugin config set demo-plugin signature "sha256:abcd1234" --json

asi plugin config set demo-plugin source "https://example.com/org/demo-plugin" --json
asi plugin config set demo-plugin version "0.9.0" --json
asi plugin config set demo-plugin signature "sig:publisher-v1" --json
```

## Environment Variables
- OpenAI: `OPENAI_API_KEY`, `OPENAI_BASE_URL`
- DeepSeek: `DEEPSEEK_API_KEY`, `DEEPSEEK_BASE_URL`
- Claude: `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL`
- ASI toggles: `ASI_TELEMETRY`, `ASI_UNDERCOVER`, `ASI_AUTO_CHECKPOINT`, `ASI_NATIVE_TOOL_CALLING`, `ASI_PROVIDER_MAX_RETRIES`, `ASI_PROVIDER_RETRY_BACKOFF_MS`, `ASI_MODEL_AUTO_FALLBACK`
- Project instruction loading: `ASI_PROJECT_INSTRUCTIONS_LAYERED` (default `true`), `ASI_PROJECT_INSTRUCTIONS_MAX_LEVELS` (default `4`), `ASI_CLAUDE_SINGLE` (default `true`)
  - instruction candidates (high to low priority per directory):
    - Claude single mode: `CLAUDE.override.md`, `CLAUDE.local.md`, `CLAUDE.md`
    - fallback mode: `AGENTS.override.md`, `AGENTS.local.md`, `AGENTS.md`, `README.override.md`, `README.local.md`, `README.md`
    - non-single mode loads all above in order
- Hooks (disabled by default): `ASI_HOOKS_ENABLED`, `ASI_HOOK_PRE_TOOL_USE`, `ASI_HOOK_PERMISSION_REQUEST`, `ASI_HOOK_POST_TOOL_USE`, `ASI_HOOK_SESSION_START`, `ASI_HOOK_USER_PROMPT_SUBMIT`, `ASI_HOOK_STOP`, `ASI_HOOK_SUBAGENT_STOP`, `ASI_HOOK_PRE_COMPACT`, `ASI_HOOK_POST_COMPACT`, `ASI_HOOK_TIMEOUT_SECS` (default `15`), `ASI_HOOK_JSON` (default `true`, provides `ASI_HOOK_INPUT_JSON`), `ASI_HOOK_CONFIG_PATH` (optional JSON handler matrix file), `ASI_HOOK_FAILURE_POLICY` (`fail-closed` default, supports `fail-open`)
  - Hook plugin context envs: `ASI_HOOK_PLUGIN_COUNT`, `ASI_HOOK_PLUGIN_NAMES` (trusted+enabled plugin names CSV)
  - Windows lifecycle hook template: `scripts/windows/hooks/lifecycle_event.ps1` (logs SessionStart/UserPromptSubmit/Stop/SubagentStop/PreCompact/PostCompact into `.asi/hooks.lifecycle.log`)
  - Trusted + enabled plugins can also provide hook scripts in `.codex-plugin/plugin.json` under `hooks`:
    - single-event keys: `pre_tool_use`, `permission_request`, `post_tool_use`, `session_start`, `user_prompt_submit`, `stop`, `subagent_stop`, `pre_compact`, `post_compact`
    - optional `handlers` array with fields: `event`, `script`, `timeout_secs`, `json_protocol`, `tool_prefix`, `permission_mode`, `failure_policy`
    - runtime executes order: env hook -> config handlers -> plugin hooks (deterministic by plugin name)
- Subagent auto tool-loop toggle: `ASI_SUBAGENT_TOOL_LOOP` (default `true`)
- Review risk scoring weights: `ASI_REVIEW_RISK_WEIGHT_CRITICAL` (default `8`), `ASI_REVIEW_RISK_WEIGHT_HIGH` (default `4`), `ASI_REVIEW_RISK_WEIGHT_MEDIUM` (default `2`), `ASI_REVIEW_RISK_WEIGHT_LOW` (default `1`), `ASI_REVIEW_RISK_WEIGHT_UNKNOWN` (default `1`)
- Review JSON-only strict gate: `ASI_REVIEW_JSON_ONLY_STRICT_SCHEMA` (default `false`; when `true`, invalid `/review --json-only` output is wrapped in an explicit error envelope)
- Review JSON-only strict fail exit: `ASI_REVIEW_JSON_ONLY_STRICT_FAIL_EXIT` (default `false`; when `true`, prompt mode exits non-zero if strict JSON-only schema validation fails)
- Review JSON-only prompt envelope: `ASI_REVIEW_JSON_ONLY_PROMPT_ENVELOPE` (default `false`; when `true`, prompt `/review --json-only` success output uses the same `{schema_version,status,review}` envelope as REPL)
- Security defaults:
  - Default `permission_mode` is `on-request` (interactive approvals for mutating tools)
  - Default sandbox mode is `local` (startup sets `ASI_SANDBOX=local` when unset; explicit env keeps priority)
  - MCP start trust gate: untrusted MCP servers are blocked by default; trust explicitly with `/mcp config trust <name> on` (override for controlled environments: `ASI_MCP_ALLOW_UNTRUSTED_START=true`)
  - Use `/runtime-profile safe` to enforce `on-request + local`
  - Use `/runtime-profile fast` to switch to `workspace-write + disabled`

`/status` includes `project_context sources=<n> [...]` so you can verify which instruction files were loaded.
When hooks are enabled, `/status` also reports hook diagnostics summary:
- `hooks enabled=true diagnostics=<n> denied=<n> errors=<n>`
- up to 5 recent hook outcome lines in shape:
  - `hook source=<env-hook|config-hook|plugin-hook> event=<...> allow=<...> is_error=<...> reason=<...>`
When hooks are disabled, it reports:
- `hooks enabled=false`

Windows hook templates are provided under `scripts/windows/hooks/` with ready-to-use examples.
Includes `permission_request_strict.ps1` for default-deny `bash` policy with whitelist prefixes.
Also includes `hooks.config.example.json` for event-matrix style hook configuration.
For quick enable/disable in the current PowerShell session, use:
- `scripts/windows/hooks/enable_strict_hooks.ps1`
- `scripts/windows/hooks/disable_hooks.ps1`

Hook config matrix (`ASI_HOOK_CONFIG_PATH`) supports multiple handlers with filters:
- `event`: `PreToolUse`, `PermissionRequest`, `PostToolUse`, `SessionStart`, `UserPromptSubmit`, `Stop`, `SubagentStop`, `PreCompact`, `PostCompact`, or `*`
- `script`: command string to execute
- `timeout_secs`: optional per-handler override
- `json_protocol`: optional per-handler override
- `tool_prefix`: optional tool name prefix filter (for example `bash`)
- `permission_mode`: optional permission mode filter (for example `workspace-write`)
- `failure_policy`: optional per-handler override (`fail-closed` or `fail-open`)

Failure policy behavior:
- `fail-closed`: hook runtime errors (spawn failure, timeout, non-json parse/exec errors) block the action.
- `fail-open`: hook runtime errors are treated as allow, but explicit deny results still block.

If both legacy env hooks and config handlers are set, both run for the same event.

Hook JSON versioning policy:
- `schema_version` is the top-level hook payload schema version (currently `"1"`).
- `event_version` is per-event payload contract version (currently `"1"` for all events).
- Backward-compatible field additions keep the same version; breaking changes require bumping the corresponding version and update notes.

Prompt JSON output includes a `review` object for `/review ...` tasks:
- `schema_version` (`"1"`, stable for backward-compatible additions)
- `is_review_task` (bool)
- `schema_valid` (bool)
- `schema_error` (string|null)
- `sections` when parseable:
  - `findings` (structured objects: `severity`, `normalized_severity`, `path`, `line`, `location`, `summary`, `evidence`, `risk`, `raw`)
  - `findings_sorted` (same shape as `findings`, sorted by severity and location for deterministic UI ordering)
  - `findings_raw` (original lines for compatibility)
  - `missing_tests`, `open_questions`, `summary`
- `stats`:
  - `has_findings` (bool)
  - `total_findings` (number)
  - `severity_counts` (`critical`, `high`, `medium`, `low`, `unknown`)
  - `risk_score_total` (number, weighted total across all findings using configured severity weights)
  - `missing_tests_count` (number, excludes `None`)
  - `top_risk_paths` (top files by weighted risk score, includes per-path severity counts)

Machine-consumable example:

```powershell
cargo run --release -- prompt "/review inspect parser" `
  --provider openai `
  --model gpt-5.3-codex `
  --project "D:\Code\Rust" `
  --output-format json | `
  ConvertFrom-Json | `
  Select-Object -ExpandProperty review | `
  ConvertTo-Json -Depth 8
```

Fast review-only JSON (no wrapper envelope):

```powershell
cargo run --release -- prompt "/review inspect parser --json-only" `
  --provider openai `
  --model gpt-5.3-codex `
  --project "D:\Code\Rust"
```

Optional strict CI gate for `/review --json-only`:

```powershell
$env:ASI_REVIEW_JSON_ONLY_STRICT_SCHEMA="true"
$env:ASI_REVIEW_JSON_ONLY_STRICT_FAIL_EXIT="true"
cargo run --release -- prompt "/review inspect parser --json-only" `
  --provider openai `
  --model gpt-5.3-codex `
  --project "D:\Code\Rust" `
  --output-format json
```

When strict mode is enabled and schema validation fails, JSON-only output is:
- REPL `/review ... --json-only` success shape: `{ "schema_version":"1", "status":"ok", "review":{...} }`
- REPL `/review ... --json-only` strict-fail shape: `{ "schema_version":"1", "status":"error", "error":"review_schema_invalid", "schema_error":"...", "review":{...} }`
- Prompt `/review ... --json-only --output-format json` default success shape: raw `review` object (no outer `status`)
- Prompt `/review ... --json-only --output-format json` strict-fail shape: same error envelope
- Prompt success envelope optional: set `ASI_REVIEW_JSON_ONLY_PROMPT_ENVELOPE=true` to emit `{ "schema_version":"1", "status":"ok", "review":{...} }`
- `smoke_review_json.ps1` hard-checks `status=="ok"` and top-level `schema_version=="1"` when `-PromptEnvelope on` is used.
- If `ASI_REVIEW_JSON_ONLY_STRICT_FAIL_EXIT=true`, prompt mode returns a non-zero exit code after emitting the error envelope (CI-friendly failure signal).

## Multi-Provider Smoke Recipes (Windows)

The smoke scripts now support provider-scoped runs for API-compat and gateway checks.
You can run OpenAI/DeepSeek flows independently without requiring Claude credentials.

### 1) DeepSeek-only API Compat (no OpenAI key required)

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_api_compat.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\test_code" `
  -SkipOpenAi `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>" `
  -DeepSeekModel "deepseek-reasoner"
```

### 2) OpenAI-only API Compat (no DeepSeek key required)

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_api_compat.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\test_code" `
  -SkipDeepSeek `
  -OpenAiBaseUrl "https://api.openai.com/v1" `
  -OpenAiApiKey "<YOUR_OPENAI_KEY>" `
  -OpenAiModel "gpt-5.3-codex"
```

### 3) Gateway Smoke by Provider (openai|deepseek|claude)

DeepSeek example:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_gateway.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\test_code" `
  -Provider deepseek `
  -ApiKey "<YOUR_DEEPSEEK_KEY>" `
  -Model "deepseek-reasoner"
```

OpenAI example:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_gateway.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\test_code" `
  -Provider openai `
  -BaseUrl "https://api.openai.com/v1" `
  -ApiKey "<YOUR_OPENAI_KEY>" `
  -Model "gpt-5.3-codex"
```

### 4) One-Command `smoke_all` with Provider-Scoped Gateway

Fast DeepSeek gateway path (skip tool turn for speed):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_all.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\test_code" `
  -Repo "D:\Code\rustbpe" `
  -Quick `
  -SkipApiCompat -SkipProviderModel -SkipTokenizer -SkipCheckpoint `
  -RunGateway `
  -GatewayProvider deepseek `
  -GatewaySkipToolTurn `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>" `
  -DeepSeekModel "deepseek-reasoner"
```

Optional auto-summary for smoke reports:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_all.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -Repo "D:\Code\rustbpe" `
  -SkipApiCompat -SkipProviderModel -SkipTokenizer -SkipCheckpoint -SkipGateway `
  -ReportJsonPath ".\artifacts\smoke_all.json" `
  -RenderSummary
```

Optional custom summary output path:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_all.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -Repo "D:\Code\rustbpe" `
  -SkipApiCompat -SkipProviderModel -SkipTokenizer -SkipCheckpoint -SkipGateway `
  -ReportJsonPath ".\artifacts\smoke_all.json" `
  -RenderSummary `
  -SummaryOutFile ".\artifacts\reports\custom_summary.md"
```

Strict profile (CI parity) example for `smoke_all.ps1`:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_all.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -Repo "D:\Code\rustbpe" `
  -StrictProfile `
  -SkipApiCompat -SkipProviderModel -SkipTokenizer -SkipCheckpoint -SkipGateway `
  -ReportJsonPath ".\artifacts\smoke_all_strict_profile.json"
```

`-StrictProfile` requires these checks to remain enabled and rejects:
- `-SkipHookMatrix`
- `-SkipHooksCliAdvanced`
- `-SkipSubagent`

### Notes
- `smoke_api_compat.ps1` and `smoke_api_compat_min.ps1` support:
  - `-SkipOpenAi`
  - `-SkipDeepSeek`
- `smoke_review_json.ps1` validates `/review` structured JSON fields (`schema_valid`, `sections.findings_sorted`, `stats.top_risk_paths`, `stats.missing_tests_count`).
- `smoke_prompt_jsonl.ps1` validates prompt `--output-format jsonl` event stream fields (`schema_version`, `event`, `data`) and required events (`prompt.result`, `prompt.changed_files`, `prompt.native_tool_calls`, `prompt.auto_validation`, `prompt.review`, `prompt.runtime`).
- `smoke_all.ps1` supports env fallbacks when flags are not passed:
  - `ASI_SMOKE_SKIP_OPENAI=1`
  - `ASI_SMOKE_SKIP_DEEPSEEK=1`
- `smoke_gateway.ps1` supports generic provider args (`-Provider/-BaseUrl/-ApiKey/-Model`) and legacy provider-specific args for backward compatibility.

### 5) Interactive Recipe Runner (OpenAI/DeepSeek)

Use `run_smoke_recipes.ps1` to avoid manual command assembly:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\run_smoke_recipes.ps1 `
  -Provider deepseek `
  -Recipe smoke-all-gateway `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\test_code" `
  -Repo "D:\Code\rustbpe" `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>" `
  -Quick `
  -GatewaySkipToolTurn
```

Unified smoke entrypoint (mode router):

```powershell
# strict (default)
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke.ps1 `
  -Mode strict `
  -Provider deepseek `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>"

# risk review + gateway
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke.ps1 `
  -Mode risk `
  -Provider deepseek `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>"

# gateway path
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke.ps1 `
  -Mode gateway `
  -Provider openai `
  -OpenAiApiKey "<YOUR_OPENAI_KEY>" `
  -NoQuick
```

For a shorter strict-regression entrypoint, use `smoke_strict_quick.ps1` (compatibility alias to `smoke.ps1 -Mode strict`).

One-command strict quick wrapper (recommended for daily regression):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_strict_quick.ps1 `
  -Provider deepseek `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>"
```

OpenAI variant:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_strict_quick.ps1 `
  -Provider openai `
  -OpenAiApiKey "<YOUR_OPENAI_KEY>" `
  -NoQuick
```

One-command risk review + gateway (relaxed schema gate):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\run_smoke_recipes.ps1 `
  -Provider deepseek `
  -Recipe smoke-all-gateway-risk `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -Repo "D:\Code\rustbpe" `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>" `
  -ReportDir ".\artifacts" `
  -RenderSummary
```

Recommended standard regression template (copy and run):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\run_smoke_recipes.ps1 `
  -Provider deepseek `
  -Recipe smoke-all-gateway-risk `
  -AsiExe ".\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -Repo "D:\Code\rustbpe" `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>" `
  -ReportDir ".\artifacts\risk_recipe_live" `
  -RenderSummary `
  -SummaryOutFile ".\artifacts\risk_recipe_live\SUMMARY_SMOKE_ALL_GATEWAY_RISK.md"
```

Recommended template for the OpenAI API endpoint (copy and run):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\run_smoke_recipes.ps1 `
  -Provider openai `
  -Recipe smoke-all-gateway-risk `
  -AsiExe ".\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -Repo "D:\Code\rustbpe" `
  -OpenAiApiKey "<YOUR_OPENAI_KEY>" `
  -OpenAiBaseUrl "https://api.openai.com/v1" `
  -ReportDir ".\artifacts\risk_recipe_live_openai" `
  -RenderSummary `
  -SummaryOutFile ".\artifacts\risk_recipe_live_openai\SUMMARY_SMOKE_ALL_GATEWAY_RISK.md"
```

Direct review JSON smoke example:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_review_json.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -Provider deepseek `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>" `
  -Model "deepseek-reasoner" `
  -PromptAutoTools off `
  -PromptEnvelope off `
  -SchemaRetries 1 `
  -FailOnSchemaInvalid on `
  -ReviewTask "Provide exactly the required sectioned review format with one low severity finding for src/main.rs:1 and no tool calls"
```

Direct prompt JSONL smoke example:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_prompt_jsonl.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -Provider deepseek `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>" `
  -Model "deepseek-reasoner" `
  -PromptText "Reply with exactly JSONL_SMOKE_OK"
```

One-command artifacts + summary via recipe runner:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\run_smoke_recipes.ps1 `
  -Provider deepseek `
  -Recipe review-json `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>" `
  -ReviewJsonPromptAutoTools off `
  -ReviewJsonPromptEnvelope off `
  -ReviewJsonSchemaRetries 1 `
  -ReviewJsonFailOnSchemaInvalid on `
  -ReviewJsonTask "Provide exactly the required sectioned review format with one low severity finding for src/main.rs:1 and no tool calls" `
  -ReportDir ".\artifacts" `
  -RenderSummary
```

Supported recipes:
- `api-compat`
- `api-compat-min`
- `gateway`
- `review-json`
- `hook-matrix`
- `hooks-cli-advanced`
- `subagent`
- `subagent-strict`
- `smoke-all-gateway`
- `smoke-all-gateway-risk`
- `smoke-all-strict`

Unified wrapper modes:
- `smoke.ps1 -Mode strict`
- `smoke.ps1 -Mode risk`
- `smoke.ps1 -Mode gateway`

### CI Machine-Readable Template (Windows)

Run one command to build/test/smoke and emit a single JSON report:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\ci_machine_json.ps1 `
  -Provider deepseek `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>"
```

Optional gate controls:

```powershell
# Use a custom gate template and keep process success even when gates fail
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\ci_machine_json.ps1 `
  -Provider deepseek `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>" `
  -GateRulesPath .\scripts\windows\ci_gate_rules.template.json `
  -SkipFailGate

# Validate CI report schema + gates
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\validate_ci_machine_report.ps1 `
  -ReportPath .\artifacts\ci_machine_json\ci_machine_report.json `
  -Strict
```

Output:
- JSON report: `.\artifacts\ci_machine_json\ci_machine_report.json`
- Smoke summary: `.\artifacts\ci_machine_json\smoke\SMOKE_SUMMARY.md`
- Prompt JSONL smoke report: `.\artifacts\ci_machine_json\smoke\smoke_prompt_jsonl.json`
- Gate rules template: `.\scripts\windows\ci_gate_rules.template.json`

Tips:
- Set `-DryRun` to print the exact command without executing.
- Use `-Provider ask -Recipe ask` for interactive menu selection.
- Use `-StrictCi` for one-command strict CI mode. It forces `smoke-all-strict`, enables summary rendering, requires strict review-json validation, and defaults `ReportDir` to `.\artifacts\strict_ci` when not provided.
- `-StrictCi` defaults review-json task to deterministic schema validation prompt (`Provide exactly the required sectioned review format...`) and `ReviewJsonPromptAutoTools=off` for stability; override via explicit flags when needed.
- `smoke-all-gateway` and `smoke-all-gateway-risk` now include `hook-matrix smoke`, `hooks-cli-advanced smoke`, and `subagent smoke` by default. Use `-SkipHookMatrix`, `-SkipHooksCliAdvanced`, and/or `-SkipSubagent` to disable them.
- `smoke-all-strict` enforces hook-matrix, hooks-cli-advanced, and subagent checks; `-SkipHookMatrix`, `-SkipHooksCliAdvanced`, and `-SkipSubagent` are rejected.
- Use `-AllowProviderNetworkError` to enable all network/provider downgrade toggles at once for `smoke-all-*` recipes.
- Use `-AllowReviewJsonNetworkError` for `smoke-all-*` recipes when provider/network instability is expected. `review-json smoke` network/provider failures are downgraded to `warn` so later steps (hook-matrix/subagent/gateway) can still run.
- Use `-AllowSubagentNetworkError` for `smoke-all-*` recipes when provider/network instability is expected. `subagent smoke` network/provider failures are downgraded to `warn` so gateway checks can still run.
- Use `-AllowGatewayNetworkError` for `smoke-all-*` recipes when provider/network instability is expected. `gateway smoke` network/provider failures are downgraded to `warn` so the aggregate run can finish and emit reports.
- CI machine report gate behavior:
  - `schema_version="1"` and `ci_schema="asi.ci.machine_report.v1"` are fixed.
  - `gates.pass=false` will exit non-zero by default.
  - pass `-SkipFailGate` to always emit report and exit zero (useful for flaky network runs).
- For recipe `subagent`, use `-SubagentAllowWaitError` to downgrade wait failure to warning-pass mode.
- Recipe `subagent-strict` always enforces wait success and rejects `-SubagentAllowWaitError`.
- When using recipe `smoke-all-gateway`, summary generation is handled by `smoke_all.ps1`; the runner will not invoke `render_smoke_summary.ps1` a second time.
- `smoke-all-gateway-risk` defaults to:
  - `ReviewJsonTask = "inspect current project and report top bug risks"`
  - `ReviewJsonPromptAutoTools = on`
  - `ReviewJsonSchemaRetries = 1`
  - `ReviewJsonFailOnSchemaInvalid = off`
  You can still override these via CLI flags.
- Use `-ReviewJsonTask` to switch between deterministic schema validation prompts and real project risk-review prompts.
- Use `-ReviewJsonPromptAutoTools on|off` to control review-json prompt continuation behavior.
- Use `-ReviewJsonPromptEnvelope on|off` to control prompt `/review --json-only` success envelope mode in smoke runs.

Hook matrix smoke (event handler config) example:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_hook_matrix.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -ReportJsonPath "D:\Code\Rust\artifacts\smoke_hook_matrix.json"
```

Recipe runner equivalent:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\run_smoke_recipes.ps1 `
  -Recipe hook-matrix `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -ReportDir "D:\Code\Rust\artifacts"
```

Hooks CLI advanced smoke example:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_hooks_cli_advanced.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -ReportJsonPath "D:\Code\Rust\artifacts\smoke_hooks_cli_advanced.json"
```

### 6) Subagent JSON Smoke (spawn/list/wait envelope checks)

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_subagent.ps1 `
  -AsiExe "D:\Code\Rust\target\release\asi.exe" `
  -Project "D:\test-cli" `
  -Provider deepseek `
  -Model deepseek-reasoner `
  -OutputMode json `
  -ReportJsonPath "D:\Code\Rust\artifacts\smoke_subagent.json"
```
- `-OutputMode` supports `json` (default) and `jsonl`.
- For JSONL mode use: `-OutputMode jsonl` (parses `agent.<command>` events).
- Pass criteria are strict: `/agent wait --json` must return `agent.status == "done"` and `agent.ok == true`; otherwise smoke fails with `failure_category=subagent_wait_not_successful`.
- Optional downgrade mode: add `-AllowWaitError` to keep `status=pass` when wait fails, and record `metrics.wait_warning=true` with failure details in metrics.
- Use `-ReviewJsonSchemaRetries <n>` to retry schema validation by rerunning the prompt when output format is unstable.
- Use `-ReviewJsonFailOnSchemaInvalid on|off` to choose whether schema-invalid review-json should fail smoke or only warn.

### Smoke Report Naming

Recommended report artifacts in the report directory:
- `smoke_all.json`: aggregated run result from `smoke_all.ps1`
- `smoke_gateway.json`: provider gateway smoke result
- `smoke_review_json.json`: `/review` JSON schema/stats smoke result
- `smoke_prompt_jsonl.json`: prompt `--output-format jsonl` event stream smoke result
- `smoke_hook_matrix.json`: hook event-matrix allow/deny regression result
- `SMOKE_SUMMARY.md`: markdown summary generated by `render_smoke_summary.ps1`

By default, `smoke_all.ps1 -RenderSummary` writes `SMOKE_SUMMARY.md` under the same directory as `-ReportJsonPath`.
Use `-SummaryOutFile <path>` to override the summary output file path.



























## Beta Known Issues

- Unsigned Windows binaries can still trigger SmartScreen warnings. Signed artifacts are supported by `scripts/windows/package_release.ps1 -SignArtifacts ...`.
- New builds auto-validate provider/model compatibility. If you still see DeepSeek `Model Not Exist`, verify you are not running an old binary and check `/status`.
- API keys are now masked in startup flows, but previously saved plaintext environment variables remain visible via OS environment inspection.
- Some enterprise/proxy networks can block provider endpoints and cause connection or TLS errors.

## Install & Troubleshooting

### Quick Checks

1. Verify binary works:
   - `.\asi.exe` (double-click launcher) or `.\asi.cmd version` or `.\bin\asi.exe version`
2. Start REPL and check runtime state:
   - `/status`
3. Confirm selected provider key is set:
   - OpenAI: `OPENAI_API_KEY`
   - DeepSeek: `DEEPSEEK_API_KEY`
   - Claude: `ANTHROPIC_API_KEY`

### Windows ZIP: `asi.exe` Does Not Open

1. Fully extract the ZIP before running `asi.exe`.
2. If downloaded from browser, open file Properties and click `Unblock` if present.
3. If SmartScreen appears for unsigned beta builds, click `More info` -> `Run anyway`.
4. Prefer signed release artifacts for distribution to reduce SmartScreen friction.

### Provider Errors

- DeepSeek `Model Not Exist`:
  1. Ensure you are on a recent build (provider/model mismatch now auto-falls back).
  2. Verify provider/model via `/status` (or run explicitly with `--provider deepseek --model deepseek-chat`).
- Claude authentication issues:
  1. Verify `ANTHROPIC_API_KEY` or `/oauth login <token>` state.
  2. Recheck with `/status`.

### Local Validation Before Release

Run these checks before packaging:

```powershell
cargo check --release
cargo test --release
```

### Optional: Sign Windows Artifacts

```powershell
$env:ASI_SIGN_CERT_PASSWORD="<pfx-password>"
powershell -ExecutionPolicy Bypass -File .\scripts\windows\package_release.ps1 -SignArtifacts -CertFile "C:\certs\asi-code.pfx" -TimestampUrl "http://timestamp.digicert.com"
```

If you use certificate store instead of PFX file:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\windows\package_release.ps1 -SignArtifacts -CertThumbprint "<sha1-thumbprint>"
```
## Community and Feedback

- Report bugs in the repository **Issues** tab (use the bug template).
- Share product ideas and UX feedback in **Discussions**.
- Contribution process: [CONTRIBUTING.md](CONTRIBUTING.md)
- Security reporting: [SECURITY.md](SECURITY.md)

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE).

## Open-Core Model (Community + Commercial)

- Community (MIT): local CLI core, REPL, provider integration, tool calls, and wiki workflows.
- Commercial (planned): managed gateway, team workspace, policy packs, audit dashboard, and priority support.
- Positioning: keep individual developer productivity open; monetize team/enterprise governance and reliability.

See: [GITHUB_REPO_COPY_BILINGUAL.md](GITHUB_REPO_COPY_BILINGUAL.md)

## CLI x API Collaboration (ZH)

- 协同机制校准与升级蓝图（中文）：[CLI_API_COLLABORATION_ZH.md](CLI_API_COLLABORATION_ZH.md)
- Collaboration reality and upgrade blueprint (English): [CLI_API_COLLABORATION_EN.md](CLI_API_COLLABORATION_EN.md)
