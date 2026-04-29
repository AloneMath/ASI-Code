[CmdletBinding()]
param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\Code\Rust",
    [string]$HookConfigPath = "",
    [int]$TimeoutSecs = 45,
    [string]$ReportJsonPath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$startedAt = Get-Date

function Write-Report(
    [string]$Status,
    [string]$FailureCategory = "",
    [string]$Hint = "",
    [string]$Message = "",
    [double]$DurationSecs = 0.0,
    [hashtable]$Extra = @{}
) {
    if ([string]::IsNullOrWhiteSpace($ReportJsonPath)) { return }
    $dir = Split-Path -Parent $ReportJsonPath
    if (-not [string]::IsNullOrWhiteSpace($dir) -and -not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
    $payload = [ordered]@{
        script = "smoke_hook_matrix"
        timestamp_utc = (Get-Date).ToUniversalTime().ToString("o")
        status = $Status
        duration_secs = [Math]::Round($DurationSecs, 3)
        failure_category = if ($Status -eq "fail") { $FailureCategory } else { $null }
        hint = if ($Status -eq "fail") { $Hint } else { $null }
        message = if ($Status -eq "fail") { $Message } else { $null }
        config = [ordered]@{
            asi_exe = $AsiExe
            project = $Project
            hook_config_path = $resolvedHookConfigPath
            timeout_secs = $TimeoutSecs
        }
        metrics = $Extra
    }
    $payload | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ReportJsonPath -Encoding UTF8
}

if (-not (Test-Path -LiteralPath $AsiExe)) {
    $msg = "asi binary not found: $AsiExe"
    Write-Error $msg
    $elapsed = (Get-Date) - $startedAt
    Write-Report -Status "fail" -FailureCategory "binary_missing" -Hint "Build release binary first: cargo build --release." -Message $msg -DurationSecs $elapsed.TotalSeconds
    exit 1
}

$resolvedHookConfigPath = if (-not [string]::IsNullOrWhiteSpace($HookConfigPath)) {
    $HookConfigPath
} else {
    Join-Path $PSScriptRoot "hooks\hooks.config.example.json"
}

if (-not (Test-Path -LiteralPath $resolvedHookConfigPath)) {
    $msg = "hook config not found: $resolvedHookConfigPath"
    Write-Error $msg
    $elapsed = (Get-Date) - $startedAt
    Write-Report -Status "fail" -FailureCategory "hook_config_missing" -Hint "Pass -HookConfigPath or provide scripts/windows/hooks/hooks.config.example.json." -Message $msg -DurationSecs $elapsed.TotalSeconds
    exit 1
}

$oldHooksEnabled = $env:ASI_HOOKS_ENABLED
$oldHookConfigPath = $env:ASI_HOOK_CONFIG_PATH
$oldHookJson = $env:ASI_HOOK_JSON
$oldHookTimeout = $env:ASI_HOOK_TIMEOUT_SECS
$oldHookPrefixes = $env:ASI_HOOK_BASH_ALLOW_PREFIXES

try {
    $env:ASI_HOOKS_ENABLED = "1"
    $env:ASI_HOOK_CONFIG_PATH = $resolvedHookConfigPath
    $env:ASI_HOOK_JSON = "1"
    $env:ASI_HOOK_TIMEOUT_SECS = "10"
    $env:ASI_HOOK_BASH_ALLOW_PREFIXES = "cargo --version"

    $denyPrompt = @"
/toolcall bash "echo smoke_hook_matrix_deny"
/exit
"@

    $allowPrompt = @"
/toolcall bash "cargo --version"
/exit
"@

    $denyOutput = $denyPrompt | & $AsiExe repl --provider deepseek --model deepseek-v4-pro --project $Project --no-setup 2>&1
    $allowOutput = $allowPrompt | & $AsiExe repl --provider deepseek --model deepseek-v4-pro --project $Project --no-setup 2>&1

    $denyText = ($denyOutput | Out-String)
    $allowText = ($allowOutput | Out-String)

    $tempHooks = Join-Path ([System.IO.Path]::GetTempPath()) ("asi_hooks_cli_smoke_{0}.json" -f ([Guid]::NewGuid().ToString("N")))
    try {
        & $AsiExe hooks config set-handler SessionStart "python hook_matrix.py" --path $tempHooks --timeout-secs 12 --json-protocol on --tool-prefix bash --permission-mode on-request --failure-policy fail-open --json | Out-Null
        & $AsiExe hooks config edit-handler SessionStart "python hook_matrix.py" --path $tempHooks --timeout-secs none --json-protocol none --tool-prefix none --permission-mode none --failure-policy none --json | Out-Null
        $validateOut = & $AsiExe hooks config validate --path $tempHooks --strict --json
    }
    finally {
        if (Test-Path -LiteralPath $tempHooks) {
            Remove-Item -LiteralPath $tempHooks -Force -ErrorAction SilentlyContinue
        }
    }
    $validateText = ($validateOut | Out-String)
    if (-not $validateText.Contains('"command":"hooks_config_validate"') -or -not $validateText.Contains('"strict":true') -or -not $validateText.Contains('"valid":true')) {
        $msg = "hooks cli smoke failed: expected strict validate json output"
        $elapsed = (Get-Date) - $startedAt
        Write-Report -Status "fail" -FailureCategory "hook_matrix_hooks_cli_smoke_failed" -Hint "Verify hooks config edit-handler none semantics and validate --strict output." -Message $msg -Extra @{
            validate_text_excerpt = $validateText.Substring(0, [Math]::Min(400, $validateText.Length))
        } -DurationSecs $elapsed.TotalSeconds
        Write-Error $msg
        exit 1
    }

    if (-not $denyText.Contains("Permission denied: hook PermissionRequest denied: strict permission hook")) {
        $msg = "deny assertion failed: expected strict hook deny text"
        $elapsed = (Get-Date) - $startedAt
        Write-Report -Status "fail" -FailureCategory "hook_matrix_deny_missing" -Hint "Verify ASI_HOOK_CONFIG_PATH and strict permission handler script." -Message $msg -Extra @{
            deny_contains_permission_denied = $denyText.Contains("Permission denied")
            deny_text_excerpt = $denyText.Substring(0, [Math]::Min(400, $denyText.Length))
        } -DurationSecs $elapsed.TotalSeconds
        Write-Error $msg
        exit 1
    }

    if (-not $allowText.Contains("TOOL")) {
        $msg = "allow assertion failed: expected successful bash tool result"
        $elapsed = (Get-Date) - $startedAt
        Write-Report -Status "fail" -FailureCategory "hook_matrix_allow_missing" -Hint "Verify ASI_HOOK_BASH_ALLOW_PREFIXES and hook config handler filters." -Message $msg -Extra @{
            allow_contains_tool_token = $allowText.Contains("TOOL")
            allow_contains_permission_denied = $allowText.Contains("Permission denied")
            allow_text_excerpt = $allowText.Substring(0, [Math]::Min(400, $allowText.Length))
        } -DurationSecs $elapsed.TotalSeconds
        Write-Error $msg
        exit 1
    }

    if ($allowText.Contains("Permission denied")) {
        $msg = "allow assertion failed: tool call was still denied"
        $elapsed = (Get-Date) - $startedAt
        Write-Report -Status "fail" -FailureCategory "hook_matrix_allow_still_denied" -Hint "Verify ASI_HOOK_BASH_ALLOW_PREFIXES is honored by permission_request_strict.ps1 in hook matrix mode." -Message $msg -Extra @{
            allow_contains_permission_denied = $true
            allow_text_excerpt = $allowText.Substring(0, [Math]::Min(400, $allowText.Length))
        } -DurationSecs $elapsed.TotalSeconds
        Write-Error $msg
        exit 1
    }

    Write-Host "smoke_hook_matrix: PASS"
    $elapsed = (Get-Date) - $startedAt
    Write-Report -Status "pass" -Extra @{
        deny_contains_permission_denied = $true
        allow_contains_tool_token = $true
        hooks_cli_validate_strict_passed = $true
    } -DurationSecs $elapsed.TotalSeconds
    exit 0
}
catch {
    $msg = $_.Exception.Message
    $elapsed = (Get-Date) - $startedAt
    Write-Report -Status "fail" -FailureCategory "hook_matrix_smoke_failed" -Hint "Inspect REPL output and hook script paths in ASI_HOOK_CONFIG_PATH." -Message $msg -DurationSecs $elapsed.TotalSeconds
    Write-Error $msg
    exit 1
}
finally {
    if ($null -ne $oldHooksEnabled) { $env:ASI_HOOKS_ENABLED = $oldHooksEnabled } else { Remove-Item Env:ASI_HOOKS_ENABLED -ErrorAction SilentlyContinue }
    if ($null -ne $oldHookConfigPath) { $env:ASI_HOOK_CONFIG_PATH = $oldHookConfigPath } else { Remove-Item Env:ASI_HOOK_CONFIG_PATH -ErrorAction SilentlyContinue }
    if ($null -ne $oldHookJson) { $env:ASI_HOOK_JSON = $oldHookJson } else { Remove-Item Env:ASI_HOOK_JSON -ErrorAction SilentlyContinue }
    if ($null -ne $oldHookTimeout) { $env:ASI_HOOK_TIMEOUT_SECS = $oldHookTimeout } else { Remove-Item Env:ASI_HOOK_TIMEOUT_SECS -ErrorAction SilentlyContinue }
    if ($null -ne $oldHookPrefixes) { $env:ASI_HOOK_BASH_ALLOW_PREFIXES = $oldHookPrefixes } else { Remove-Item Env:ASI_HOOK_BASH_ALLOW_PREFIXES -ErrorAction SilentlyContinue }
}
