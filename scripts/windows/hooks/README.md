# Windows Hook Templates

These templates are compatible with ASI hook integration in `src/runtime.rs`.

## Files

- `pre_tool_use.ps1`
- `permission_request.ps1`
- `permission_request_strict.ps1` (default-deny for `bash`, whitelist prefixes only)
- `post_tool_use.ps1`
- `lifecycle_event.ps1` (shared lifecycle logger for SessionStart/UserPromptSubmit/Stop/SubagentStop/PreCompact/PostCompact)
- `hooks.config.example.json` (event matrix config for `ASI_HOOK_CONFIG_PATH`)

## One-time enable (PowerShell)

```powershell
$env:ASI_HOOKS_ENABLED='1'
$env:ASI_HOOK_JSON='1'
$env:ASI_HOOK_TIMEOUT_SECS='10'

$env:ASI_HOOK_PRE_TOOL_USE='& "D:\Code\Rust\scripts\windows\hooks\pre_tool_use.ps1"'
$env:ASI_HOOK_PERMISSION_REQUEST='& "D:\Code\Rust\scripts\windows\hooks\permission_request.ps1"'
$env:ASI_HOOK_POST_TOOL_USE='& "D:\Code\Rust\scripts\windows\hooks\post_tool_use.ps1"'
$env:ASI_HOOK_SESSION_START='& "D:\Code\Rust\scripts\windows\hooks\lifecycle_event.ps1"'
$env:ASI_HOOK_USER_PROMPT_SUBMIT='& "D:\Code\Rust\scripts\windows\hooks\lifecycle_event.ps1"'
$env:ASI_HOOK_STOP='& "D:\Code\Rust\scripts\windows\hooks\lifecycle_event.ps1"'
$env:ASI_HOOK_SUBAGENT_STOP='& "D:\Code\Rust\scripts\windows\hooks\lifecycle_event.ps1"'
$env:ASI_HOOK_PRE_COMPACT='& "D:\Code\Rust\scripts\windows\hooks\lifecycle_event.ps1"'
$env:ASI_HOOK_POST_COMPACT='& "D:\Code\Rust\scripts\windows\hooks\lifecycle_event.ps1"'
```

Then run:

```powershell
cargo run --release -- repl --provider openai --model gpt-5.3-codex --project D:\Code\Rust --no-setup
```

## Disable quickly

```powershell
Remove-Item Env:ASI_HOOKS_ENABLED -ErrorAction SilentlyContinue
Remove-Item Env:ASI_HOOK_JSON -ErrorAction SilentlyContinue
Remove-Item Env:ASI_HOOK_TIMEOUT_SECS -ErrorAction SilentlyContinue
Remove-Item Env:ASI_HOOK_PRE_TOOL_USE -ErrorAction SilentlyContinue
Remove-Item Env:ASI_HOOK_PERMISSION_REQUEST -ErrorAction SilentlyContinue
Remove-Item Env:ASI_HOOK_POST_TOOL_USE -ErrorAction SilentlyContinue
Remove-Item Env:ASI_HOOK_SESSION_START -ErrorAction SilentlyContinue
Remove-Item Env:ASI_HOOK_USER_PROMPT_SUBMIT -ErrorAction SilentlyContinue
Remove-Item Env:ASI_HOOK_STOP -ErrorAction SilentlyContinue
Remove-Item Env:ASI_HOOK_SUBAGENT_STOP -ErrorAction SilentlyContinue
Remove-Item Env:ASI_HOOK_PRE_COMPACT -ErrorAction SilentlyContinue
Remove-Item Env:ASI_HOOK_POST_COMPACT -ErrorAction SilentlyContinue
```

Or use helper scripts in this directory:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\hooks\enable_strict_hooks.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\hooks\disable_hooks.ps1
```

## Strict permission mode example

This mode denies all `bash` by default and only allows explicit command prefixes.

```powershell
$env:ASI_HOOKS_ENABLED='1'
$env:ASI_HOOK_JSON='1'
$env:ASI_HOOK_TIMEOUT_SECS='10'

$env:ASI_HOOK_PRE_TOOL_USE='powershell -NoProfile -ExecutionPolicy Bypass -File "D:\Code\Rust\scripts\windows\hooks\pre_tool_use.ps1"'
$env:ASI_HOOK_PERMISSION_REQUEST='powershell -NoProfile -ExecutionPolicy Bypass -File "D:\Code\Rust\scripts\windows\hooks\permission_request_strict.ps1"'
$env:ASI_HOOK_POST_TOOL_USE='powershell -NoProfile -ExecutionPolicy Bypass -File "D:\Code\Rust\scripts\windows\hooks\post_tool_use.ps1"'
$env:ASI_HOOK_SESSION_START='powershell -NoProfile -ExecutionPolicy Bypass -File "D:\Code\Rust\scripts\windows\hooks\lifecycle_event.ps1"'
$env:ASI_HOOK_USER_PROMPT_SUBMIT='powershell -NoProfile -ExecutionPolicy Bypass -File "D:\Code\Rust\scripts\windows\hooks\lifecycle_event.ps1"'
$env:ASI_HOOK_STOP='powershell -NoProfile -ExecutionPolicy Bypass -File "D:\Code\Rust\scripts\windows\hooks\lifecycle_event.ps1"'
$env:ASI_HOOK_SUBAGENT_STOP='powershell -NoProfile -ExecutionPolicy Bypass -File "D:\Code\Rust\scripts\windows\hooks\lifecycle_event.ps1"'
$env:ASI_HOOK_PRE_COMPACT='powershell -NoProfile -ExecutionPolicy Bypass -File "D:\Code\Rust\scripts\windows\hooks\lifecycle_event.ps1"'
$env:ASI_HOOK_POST_COMPACT='powershell -NoProfile -ExecutionPolicy Bypass -File "D:\Code\Rust\scripts\windows\hooks\lifecycle_event.ps1"'

$env:ASI_HOOK_BASH_ALLOW_PREFIXES='cargo check;cargo test;git status;git diff;python -m pytest'
```

You can tune `ASI_HOOK_BASH_ALLOW_PREFIXES` at runtime without editing scripts.

## JSON contract

When `ASI_HOOK_JSON=1`, runtime sends:

- `ASI_HOOK_INPUT_JSON={"schema_version":"1","event_version":"1","event":"...","tool":"...","args":"...","args_details":{...},"permission_mode":"..."}`

`args_details` is event-aware structured metadata. Examples:

- `Stop`: `{"kind":"stop","stop_reason":"max_turns_reached"}`
- `SubagentStop`: `{"kind":"subagent_stop","tool":"subagent","fields":{"id":"sa-1","status":"done"}}`
- `PreCompact`: `{"kind":"pre_compact","action":"runtime_compact"}`
- `PostCompact`: `{"kind":"post_compact","action":"runtime_compact"}`
- `UserPromptSubmit`: `{"kind":"user_prompt_submit","prompt":"...","prompt_len":123}`

Hook should print JSON to stdout:

- `{"allow":true,"reason":"ok"}`
- `{"allow":false,"reason":"blocked by policy"}`

If JSON is missing/invalid, runtime falls back to exit-code behavior.

## Event Matrix Config (`ASI_HOOK_CONFIG_PATH`)

You can combine the legacy env hooks with a JSON handler matrix:

```powershell
$env:ASI_HOOKS_ENABLED='1'
$env:ASI_HOOK_CONFIG_PATH='D:\Code\Rust\scripts\windows\hooks\hooks.config.example.json'
```

Handler fields:

- `event`: `PreToolUse`, `PermissionRequest`, `PostToolUse`, `SessionStart`, `UserPromptSubmit`, `Stop`, `SubagentStop`, `PreCompact`, `PostCompact`, or `*`
- `script`: command line to execute
- `timeout_secs`: optional override
- `json_protocol`: optional override
- `tool_prefix`: optional tool-name prefix filter
- `permission_mode`: optional permission-mode filter

`lifecycle_event.ps1` writes lifecycle lines to `.asi/hooks.lifecycle.log`.
