param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\Code"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Strip-Ansi([string]$text) {
    return [regex]::Replace($text, "`e\[[0-9;]*m", "")
}

function Run-Repl([string]$provider, [string]$model, [string[]]$commands) {
    $payload = (($commands + "/exit") -join "`n") + "`n"
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = $payload | & $AsiExe repl --provider $provider --model $model --project $Project --no-setup 2>&1
    }
    finally {
        $ErrorActionPreference = $prev
    }
    return (Strip-Ansi ($output | Out-String -Width 4096))
}

function Assert-Contains([string]$text, [string]$needle, [string]$label) {
    if (-not $text.Contains($needle)) {
        throw "[$label] missing: $needle`n--- output ---`n$text"
    }
}

if (-not (Test-Path -LiteralPath $AsiExe)) {
    throw "asi binary not found: $AsiExe"
}

Write-Host "Using binary: $AsiExe"
Write-Host "Project: $Project"

# Case 1: deepseek + openai model -> fallback deepseek-v4-pro
$o1 = Run-Repl "deepseek" "gpt-4.1-mini" @("/status")
Assert-Contains $o1 "provider=deepseek model=deepseek-v4-pro" "case1-status"
Assert-Contains $o1 "WARN model gpt-4.1-mini incompatible with provider deepseek, fallback=deepseek-v4-pro" "case1-warn"
Write-Host "[ok] case1"

# Case 2: claude + deepseek model -> fallback claude sonnet alias
$o2 = Run-Repl "claude" "deepseek-v4-pro" @("/status")
Assert-Contains $o2 "provider=claude model=claude-3-7-sonnet-latest" "case2-status"
Assert-Contains $o2 "WARN model deepseek-v4-pro incompatible with provider claude, fallback=claude-3-7-sonnet-latest" "case2-warn"
Write-Host "[ok] case2"

# Case 3: openai + claude model -> fallback gpt-4.1-mini
$o3 = Run-Repl "openai" "claude-3-7-sonnet-latest" @("/status")
Assert-Contains $o3 "provider=openai model=gpt-4.1-mini" "case3-status"
Assert-Contains $o3 "WARN model claude-3-7-sonnet-latest incompatible with provider openai, fallback=gpt-4.1-mini" "case3-warn"
Write-Host "[ok] case3"

# Case 4: runtime switching via /model and /provider
$o4 = Run-Repl "openai" "gpt-4.1-mini" @(
    "/model claude-3-7-sonnet-latest",
    "/provider deepseek",
    "/status"
)
Assert-Contains $o4 "incompatible with provider openai" "case4-model"
Assert-Contains $o4 "incompatible with provider deepseek" "case4-provider"
Assert-Contains $o4 "provider=deepseek model=deepseek-v4-pro" "case4-status"
Write-Host "[ok] case4"

Write-Host "provider-model smoke: PASS"



