param(
    [string]$RepoRoot = "D:\Code\Rust",
    [string]$AllowPrefixes = "cargo check;cargo test;git status;git diff;python -m pytest"
)

$ErrorActionPreference = "Stop"

$hookRoot = Join-Path $RepoRoot "scripts\windows\hooks"
$pre = Join-Path $hookRoot "pre_tool_use.ps1"
$perm = Join-Path $hookRoot "permission_request_strict.ps1"
$post = Join-Path $hookRoot "post_tool_use.ps1"
$lifecycle = Join-Path $hookRoot "lifecycle_event.ps1"

if (-not (Test-Path -LiteralPath $pre)) {
    throw "missing hook script: $pre"
}
if (-not (Test-Path -LiteralPath $perm)) {
    throw "missing hook script: $perm"
}
if (-not (Test-Path -LiteralPath $post)) {
    throw "missing hook script: $post"
}
if (-not (Test-Path -LiteralPath $lifecycle)) {
    throw "missing hook script: $lifecycle"
}

$env:ASI_HOOKS_ENABLED = "1"
$env:ASI_HOOK_JSON = "1"
$env:ASI_HOOK_TIMEOUT_SECS = "10"
$env:ASI_HOOK_PRE_TOOL_USE = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$pre`""
$env:ASI_HOOK_PERMISSION_REQUEST = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$perm`""
$env:ASI_HOOK_POST_TOOL_USE = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$post`""
$env:ASI_HOOK_SESSION_START = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$lifecycle`""
$env:ASI_HOOK_USER_PROMPT_SUBMIT = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$lifecycle`""
$env:ASI_HOOK_STOP = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$lifecycle`""
$env:ASI_HOOK_SUBAGENT_STOP = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$lifecycle`""
$env:ASI_HOOK_PRE_COMPACT = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$lifecycle`""
$env:ASI_HOOK_POST_COMPACT = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$lifecycle`""
$env:ASI_HOOK_BASH_ALLOW_PREFIXES = $AllowPrefixes

Write-Host "strict hooks enabled for current PowerShell session"
Write-Host "ASI_HOOKS_ENABLED=$env:ASI_HOOKS_ENABLED"
Write-Host "ASI_HOOK_JSON=$env:ASI_HOOK_JSON"
Write-Host "ASI_HOOK_TIMEOUT_SECS=$env:ASI_HOOK_TIMEOUT_SECS"
Write-Host "ASI_HOOK_PRE_TOOL_USE=$env:ASI_HOOK_PRE_TOOL_USE"
Write-Host "ASI_HOOK_PERMISSION_REQUEST=$env:ASI_HOOK_PERMISSION_REQUEST"
Write-Host "ASI_HOOK_POST_TOOL_USE=$env:ASI_HOOK_POST_TOOL_USE"
Write-Host "ASI_HOOK_SESSION_START=$env:ASI_HOOK_SESSION_START"
Write-Host "ASI_HOOK_USER_PROMPT_SUBMIT=$env:ASI_HOOK_USER_PROMPT_SUBMIT"
Write-Host "ASI_HOOK_STOP=$env:ASI_HOOK_STOP"
Write-Host "ASI_HOOK_SUBAGENT_STOP=$env:ASI_HOOK_SUBAGENT_STOP"
Write-Host "ASI_HOOK_PRE_COMPACT=$env:ASI_HOOK_PRE_COMPACT"
Write-Host "ASI_HOOK_POST_COMPACT=$env:ASI_HOOK_POST_COMPACT"
Write-Host "ASI_HOOK_BASH_ALLOW_PREFIXES=$env:ASI_HOOK_BASH_ALLOW_PREFIXES"

Write-Host ""
Write-Host "example run:"
Write-Host "cargo run --release -- repl --provider openai --model gpt-5.3-codex --project D:\Code\Rust --no-setup"
