param()

$ErrorActionPreference = "Stop"

$vars = @(
    "ASI_HOOKS_ENABLED",
    "ASI_HOOK_JSON",
    "ASI_HOOK_TIMEOUT_SECS",
    "ASI_HOOK_PRE_TOOL_USE",
    "ASI_HOOK_PERMISSION_REQUEST",
    "ASI_HOOK_POST_TOOL_USE",
    "ASI_HOOK_SESSION_START",
    "ASI_HOOK_USER_PROMPT_SUBMIT",
    "ASI_HOOK_STOP",
    "ASI_HOOK_SUBAGENT_STOP",
    "ASI_HOOK_PRE_COMPACT",
    "ASI_HOOK_POST_COMPACT",
    "ASI_HOOK_BASH_ALLOW_PREFIXES"
)

foreach ($name in $vars) {
    Remove-Item "Env:$name" -ErrorAction SilentlyContinue
}

Write-Host "hooks disabled for current PowerShell session"
Write-Host "cleared vars: $($vars -join ', ')"
