param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Repo = "D:\Code\rustbpe",
    [int]$TimeoutSecs = 1
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Assert-Contains([string]$text, [string]$needle, [string]$label) {
    if (-not $text.Contains($needle)) {
        throw "[$label] missing: $needle`n--- output ---`n$text"
    }
}

if (-not (Test-Path -LiteralPath $AsiExe)) {
    throw "asi binary not found: $AsiExe"
}
if (-not (Test-Path -LiteralPath $Repo)) {
    throw "rustbpe repo not found: $Repo"
}

$work = Join-Path $PSScriptRoot "..\..\tmp"
New-Item -ItemType Directory -Force -Path $work | Out-Null

$corpus = Join-Path $work "smoke_timeout_corpus.txt"
$sleeper = Join-Path $work "smoke_sleep5.cmd"

@(
    "hello",
    "world",
    "asi"
) | Set-Content -Path $corpus -Encoding UTF8

@(
    "@echo off",
    'powershell -NoProfile -Command "Start-Sleep -Seconds 5"'
) | Set-Content -Path $sleeper -Encoding ASCII

Write-Host "Using binary: $AsiExe"
Write-Host "Repo: $Repo"
Write-Host "Timeout: $TimeoutSecs s"

$prev = $ErrorActionPreference
$ErrorActionPreference = "Continue"
try {
    $output = & $AsiExe tokenizer train --repo $Repo --input $corpus --python-cmd $sleeper --timeout-secs $TimeoutSecs 2>&1
    $exitCode = $LASTEXITCODE
}
finally {
    $ErrorActionPreference = $prev
}

$text = ($output | Out-String -Width 4096)

if ($exitCode -eq 0) {
    throw "expected timeout failure but command succeeded`n--- output ---`n$text"
}

Assert-Contains $text "command timed out after" "timeout-message"
Assert-Contains $text "hint=increase --timeout-secs and retry" "timeout-hint"

Write-Host "tokenizer-timeout smoke: PASS"

Remove-Item -LiteralPath $corpus -ErrorAction SilentlyContinue
Remove-Item -LiteralPath $sleeper -ErrorAction SilentlyContinue

