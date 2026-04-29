param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\Code\Rust",
    [string]$OpenAiBaseUrl = "",
    [string]$OpenAiApiKey = "",
    [string]$OpenAiModel = "gpt-5.3-codex",
    [string]$DeepSeekApiKey = "",
    [string]$DeepSeekModel = "deepseek-v4-pro",
    [switch]$SkipOpenAi,
    [switch]$SkipDeepSeek
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Require-Contains([string]$text, [string]$needle, [string]$label) {
    if (-not $text.Contains($needle)) {
        throw "[$label] missing: $needle`n--- output ---`n$text"
    }
}

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

if (-not (Test-Path -LiteralPath $AsiExe)) {
    throw "asi binary not found: $AsiExe"
}

$oldOpenAiBase = $env:OPENAI_BASE_URL
$oldOpenAiKey = $env:OPENAI_API_KEY
$oldDeepSeekKey = $env:DEEPSEEK_API_KEY

try {
    if (-not [string]::IsNullOrWhiteSpace($OpenAiBaseUrl)) { $env:OPENAI_BASE_URL = $OpenAiBaseUrl }
    if (-not [string]::IsNullOrWhiteSpace($OpenAiApiKey)) { $env:OPENAI_API_KEY = $OpenAiApiKey }
    if (-not [string]::IsNullOrWhiteSpace($DeepSeekApiKey)) { $env:DEEPSEEK_API_KEY = $DeepSeekApiKey }

    if ($SkipOpenAi) {
        Write-Host "[min 1/2] openai-compatible (skipped)"
    }
    else {
        Write-Host "[min 1/2] openai-compatible"
        $o1 = Run-And-Capture "& `"$AsiExe`" prompt `"Reply with exactly MIN_OK`" --provider openai --model `"$OpenAiModel`" --project `"$Project`" --output-format text"
        Require-Contains $o1 "MIN_OK" "openai-min"
        Write-Host "[ok] openai-compatible"
    }

    if ($SkipDeepSeek) {
        Write-Host "[min 2/2] deepseek (skipped)"
    }
    else {
        Write-Host "[min 2/2] deepseek"
        $o2 = Run-And-Capture "& `"$AsiExe`" prompt `"Reply with exactly MIN_DEEPSEEK_OK`" --provider deepseek --model `"$DeepSeekModel`" --project `"$Project`" --output-format text"
        Require-Contains $o2 "MIN_DEEPSEEK_OK" "deepseek-min"
        Write-Host "[ok] deepseek"
    }

    Write-Host "smoke_api_compat_min: PASS"
}
finally {
    if ($null -eq $oldOpenAiBase) { Remove-Item Env:OPENAI_BASE_URL -ErrorAction SilentlyContinue } else { $env:OPENAI_BASE_URL = $oldOpenAiBase }
    if ($null -eq $oldOpenAiKey) { Remove-Item Env:OPENAI_API_KEY -ErrorAction SilentlyContinue } else { $env:OPENAI_API_KEY = $oldOpenAiKey }
    if ($null -eq $oldDeepSeekKey) { Remove-Item Env:DEEPSEEK_API_KEY -ErrorAction SilentlyContinue } else { $env:DEEPSEEK_API_KEY = $oldDeepSeekKey }
}
