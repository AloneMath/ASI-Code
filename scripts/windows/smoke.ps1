<#
.SYNOPSIS
Unified smoke entrypoint for Windows.

.DESCRIPTION
Routes to run_smoke_recipes.ps1 with a simplified mode switch:
  - strict  -> smoke-all-strict (StrictCi enabled)
  - risk    -> smoke-all-gateway-risk
  - gateway -> smoke-all-gateway

Defaults:
  - Provider: deepseek
  - Quick: enabled (disable with -NoQuick)
  - RenderSummary: enabled

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke.ps1 `
  -Mode strict -Provider deepseek -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>"

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke.ps1 `
  -Mode gateway -Provider openai -OpenAiApiKey "<YOUR_OPENAI_KEY>" -NoQuick
#>

param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\Code\Rust",
    [string]$Repo = "D:\Code\rustbpe",
    [ValidateSet("strict", "risk", "gateway")]
    [string]$Mode = "strict",
    [ValidateSet("openai", "deepseek")]
    [string]$Provider = "deepseek",
    [string]$ReportDir = "",
    [string]$SummaryOutFile = "",
    [switch]$NoQuick,
    [switch]$GatewaySkipToolTurn,
    [switch]$AllowProviderNetworkError,
    [switch]$AllowReviewJsonNetworkError,
    [switch]$AllowSubagentNetworkError,
    [switch]$AllowGatewayNetworkError,
    [switch]$DryRun,
    [string]$ReviewJsonTask = "",
    [ValidateSet("on", "off")]
    [string]$ReviewJsonPromptAutoTools = "off",
    [ValidateSet("on", "off")]
    [string]$ReviewJsonPromptEnvelope = "off",
    [int]$ReviewJsonSchemaRetries = 1,
    [ValidateSet("on", "off")]
    [string]$ReviewJsonFailOnSchemaInvalid = "on",
    [string]$OpenAiBaseUrl = "",
    [string]$OpenAiApiKey = "",
    [string]$OpenAiModel = "gpt-5.3-codex",
    [string]$DeepSeekBaseUrl = "",
    [string]$DeepSeekApiKey = "",
    [string]$DeepSeekModel = "deepseek-v4-pro"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Add-IfValue([System.Collections.Generic.List[string]]$Target, [string]$Name, [string]$Value) {
    if (-not [string]::IsNullOrWhiteSpace($Value)) {
        $Target.Add($Name) | Out-Null
        $Target.Add($Value) | Out-Null
    }
}

function Mask-Secret([string]$s) {
    if ([string]::IsNullOrWhiteSpace($s)) { return "<empty>" }
    if ($s.Length -le 8) { return "****" }
    return $s.Substring(0, 4) + "..." + $s.Substring($s.Length - 4, 4)
}

function Resolve-OpenAiKey {
    if (-not [string]::IsNullOrWhiteSpace($OpenAiApiKey)) { return $OpenAiApiKey }
    if (-not [string]::IsNullOrWhiteSpace($env:OPENAI_API_KEY)) { return $env:OPENAI_API_KEY }
    return ""
}

function Resolve-DeepSeekKey {
    if (-not [string]::IsNullOrWhiteSpace($DeepSeekApiKey)) { return $DeepSeekApiKey }
    if (-not [string]::IsNullOrWhiteSpace($env:DEEPSEEK_API_KEY)) { return $env:DEEPSEEK_API_KEY }
    return ""
}

function Resolve-RecipeFromMode([string]$SelectedMode) {
    switch ($SelectedMode) {
        "strict" { return "smoke-all-strict" }
        "risk" { return "smoke-all-gateway-risk" }
        "gateway" { return "smoke-all-gateway" }
        default { throw "Unsupported mode: $SelectedMode" }
    }
}

$runnerScript = Join-Path $PSScriptRoot "run_smoke_recipes.ps1"
if (-not (Test-Path -LiteralPath $runnerScript)) {
    throw "missing script: $runnerScript"
}

$effectiveReportDir = if (-not [string]::IsNullOrWhiteSpace($ReportDir)) {
    $ReportDir
} else {
    switch ($Mode) {
        "strict" { Join-Path (Join-Path $PSScriptRoot "..\..\artifacts") "strict_ci" }
        "risk" { Join-Path (Join-Path $PSScriptRoot "..\..\artifacts") "risk_ci" }
        default { Join-Path (Join-Path $PSScriptRoot "..\..\artifacts") "gateway_ci" }
    }
}

$recipe = Resolve-RecipeFromMode $Mode
$resolvedOpenAiKey = Resolve-OpenAiKey
$resolvedDeepSeekKey = Resolve-DeepSeekKey

Write-Host "============================================================"
Write-Host "ASI smoke wrapper"
Write-Host "Runner: $runnerScript"
Write-Host "Mode: $Mode"
Write-Host "Recipe: $recipe"
Write-Host "Provider: $Provider"
Write-Host "AsiExe: $AsiExe"
Write-Host "Project: $Project"
Write-Host "Repo: $Repo"
Write-Host "Quick: $(-not $NoQuick)"
Write-Host "DryRun: $DryRun"
Write-Host ("OpenAI key: {0}" -f (Mask-Secret $resolvedOpenAiKey))
Write-Host ("DeepSeek key: {0}" -f (Mask-Secret $resolvedDeepSeekKey))
Write-Host "ReportDir: $effectiveReportDir"
if (-not [string]::IsNullOrWhiteSpace($SummaryOutFile)) {
    Write-Host "SummaryOutFile: $SummaryOutFile"
}
Write-Host "============================================================"

$args = New-Object System.Collections.Generic.List[string]
$args.AddRange([string[]]@(
    "-AsiExe", $AsiExe,
    "-Project", $Project,
    "-Repo", $Repo,
    "-Provider", $Provider,
    "-Recipe", $recipe,
    "-RenderSummary",
    "-ReportDir", $effectiveReportDir,
    "-ReviewJsonPromptAutoTools", $ReviewJsonPromptAutoTools,
    "-ReviewJsonPromptEnvelope", $ReviewJsonPromptEnvelope,
    "-ReviewJsonSchemaRetries", ([Math]::Max(0, $ReviewJsonSchemaRetries).ToString()),
    "-ReviewJsonFailOnSchemaInvalid", $ReviewJsonFailOnSchemaInvalid,
    "-OpenAiModel", $OpenAiModel,
    "-DeepSeekModel", $DeepSeekModel
))

if ($Mode -eq "strict") { $args.Add("-StrictCi") | Out-Null }
if (-not $NoQuick) { $args.Add("-Quick") | Out-Null }
if ($GatewaySkipToolTurn) { $args.Add("-GatewaySkipToolTurn") | Out-Null }
if ($AllowProviderNetworkError) { $args.Add("-AllowProviderNetworkError") | Out-Null }
if ($AllowReviewJsonNetworkError) { $args.Add("-AllowReviewJsonNetworkError") | Out-Null }
if ($AllowSubagentNetworkError) { $args.Add("-AllowSubagentNetworkError") | Out-Null }
if ($AllowGatewayNetworkError) { $args.Add("-AllowGatewayNetworkError") | Out-Null }
if ($DryRun) { $args.Add("-DryRun") | Out-Null }
if (-not [string]::IsNullOrWhiteSpace($SummaryOutFile)) {
    $args.AddRange([string[]]@("-SummaryOutFile", $SummaryOutFile))
}
if (-not [string]::IsNullOrWhiteSpace($ReviewJsonTask)) {
    $args.AddRange([string[]]@("-ReviewJsonTask", $ReviewJsonTask))
}

Add-IfValue $args "-OpenAiBaseUrl" $OpenAiBaseUrl
Add-IfValue $args "-OpenAiApiKey" $OpenAiApiKey
Add-IfValue $args "-DeepSeekBaseUrl" $DeepSeekBaseUrl
Add-IfValue $args "-DeepSeekApiKey" $DeepSeekApiKey

& powershell -NoProfile -ExecutionPolicy Bypass -File $runnerScript @args
