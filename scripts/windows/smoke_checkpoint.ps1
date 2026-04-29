param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\Code"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Strip-Ansi([string]$text) {
    return [regex]::Replace($text, "`e\[[0-9;]*m", "")
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

$payload = @(
    "/checkpoint status",
    "/checkpoint save",
    "/clear",
    "/checkpoint load",
    "/checkpoint clear",
    "/checkpoint status",
    "/exit"
) -join "`n"
$payload += "`n"

$prev = $ErrorActionPreference
$ErrorActionPreference = "Continue"
try {
    $output = $payload | & $AsiExe repl --provider deepseek --model deepseek-v4-pro --project $Project --no-setup 2>&1
}
finally {
    $ErrorActionPreference = $prev
}

$text = Strip-Ansi ($output | Out-String -Width 4096)

Assert-Contains $text "checkpoint_auto=" "checkpoint-status"
Assert-Contains $text "checkpoint_saved=" "checkpoint-save"
Assert-Contains $text "checkpoint_loaded=" "checkpoint-load"
Assert-Contains $text "checkpoint_cleared=true" "checkpoint-clear"

Write-Host "checkpoint smoke: PASS"
