param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\Code\Rust",
    [string]$OpenAiBaseUrl = "",
    [string]$OpenAiApiKey = "",
    [string]$OpenAiModel = "gpt-5.3-codex",
    [string]$DeepSeekBaseUrl = "",
    [string]$DeepSeekApiKey = "",
    [string]$DeepSeekModel = "deepseek-v4-pro",
    [switch]$SkipOpenAi,
    [switch]$SkipDeepSeek
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Run-And-Capture([string]$commandLine) {
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = Invoke-Expression "$commandLine 2>&1"
    }
    finally {
        $ErrorActionPreference = $prev
    }
    return ($output | Out-String -Width 4096)
}

function Require-Contains([string]$text, [string]$needle, [string]$label) {
    if (-not $text.Contains($needle)) {
        throw "[$label] missing: $needle`n--- output ---`n$text"
    }
}

if (-not (Test-Path -LiteralPath $AsiExe)) {
    throw "asi binary not found: $AsiExe"
}

Write-Host "Using binary: $AsiExe"
Write-Host "Project: $Project"

$previousOpenAiBase = $env:OPENAI_BASE_URL
$previousOpenAiKey = $env:OPENAI_API_KEY
$previousDeepSeekBase = $env:DEEPSEEK_BASE_URL
$previousDeepSeekKey = $env:DEEPSEEK_API_KEY

try {
    if (-not [string]::IsNullOrWhiteSpace($OpenAiBaseUrl)) { $env:OPENAI_BASE_URL = $OpenAiBaseUrl }
    if (-not [string]::IsNullOrWhiteSpace($OpenAiApiKey)) { $env:OPENAI_API_KEY = $OpenAiApiKey }
    if (-not [string]::IsNullOrWhiteSpace($DeepSeekBaseUrl)) { $env:DEEPSEEK_BASE_URL = $DeepSeekBaseUrl }
    if (-not [string]::IsNullOrWhiteSpace($DeepSeekApiKey)) { $env:DEEPSEEK_API_KEY = $DeepSeekApiKey }

    if ($SkipOpenAi) {
        Write-Host "`n[1/4] OpenAI-compatible prompt smoke (skipped)"
    }
    else {
        Write-Host "`n[1/4] OpenAI-compatible prompt smoke"
        $o1 = Run-And-Capture "& `"$AsiExe`" prompt `"Reply with exactly API_SMOKE_OK`" --provider openai --model `"$OpenAiModel`" --project `"$Project`" --output-format text"
        Require-Contains $o1 "API_SMOKE_OK" "openai-prompt"
        Write-Host "[ok] openai prompt"
    }

    if ($SkipOpenAi) {
        Write-Host "`n[2/4] OpenAI-compatible agent/tool smoke (skipped)"
    }
    else {
        Write-Host "`n[2/4] OpenAI-compatible agent/tool smoke"
        $o2 = Run-And-Capture "& `"$AsiExe`" prompt `"Count top-level entries in current directory and output exactly TOP_LEVEL_COUNT=<number>.`" --provider openai --model `"$OpenAiModel`" --project `"$Project`" --output-format text --agent --agent-max-steps 6"
        Require-Contains $o2 "TOP_LEVEL_COUNT=" "openai-agent"
        Write-Host "[ok] openai agent"
    }

    if ($SkipDeepSeek) {
        Write-Host "`n[3/4] DeepSeek status and native default (skipped)"
    }
    else {
        Write-Host "`n[3/4] DeepSeek status and native default"
        $payload = "/status`n/exit`n"
        $o3 = ($payload | & $AsiExe repl --provider deepseek --model $DeepSeekModel --project $Project --no-setup 2>&1 | Out-String -Width 4096)
        Require-Contains $o3 "provider_runtime native=false" "deepseek-status-native"
        Write-Host "[ok] deepseek status"
    }

    if ($SkipDeepSeek) {
        Write-Host "`n[4/4] DeepSeek prompt smoke (skipped)"
    }
    else {
        Write-Host "`n[4/4] DeepSeek prompt smoke"
        $o4 = Run-And-Capture "& `"$AsiExe`" prompt `"Reply with exactly DEEPSEEK_SMOKE_OK`" --provider deepseek --model `"$DeepSeekModel`" --project `"$Project`" --output-format text"
        Require-Contains $o4 "DEEPSEEK_SMOKE_OK" "deepseek-prompt"
        Write-Host "[ok] deepseek prompt"
    }

    Write-Host "`nsmoke_api_compat: PASS"
}
finally {
    if ($null -eq $previousOpenAiBase) { Remove-Item Env:OPENAI_BASE_URL -ErrorAction SilentlyContinue } else { $env:OPENAI_BASE_URL = $previousOpenAiBase }
    if ($null -eq $previousOpenAiKey) { Remove-Item Env:OPENAI_API_KEY -ErrorAction SilentlyContinue } else { $env:OPENAI_API_KEY = $previousOpenAiKey }
    if ($null -eq $previousDeepSeekBase) { Remove-Item Env:DEEPSEEK_BASE_URL -ErrorAction SilentlyContinue } else { $env:DEEPSEEK_BASE_URL = $previousDeepSeekBase }
    if ($null -eq $previousDeepSeekKey) { Remove-Item Env:DEEPSEEK_API_KEY -ErrorAction SilentlyContinue } else { $env:DEEPSEEK_API_KEY = $previousDeepSeekKey }
}
