<#
.SYNOPSIS
Compatibility alias for strict smoke regression.

.DESCRIPTION
This script forwards to:
  scripts/windows/smoke.ps1 -Mode strict

It keeps the historical entrypoint name `smoke_strict_quick.ps1` to avoid
breaking existing automation while centralizing maintenance in smoke.ps1.

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_strict_quick.ps1 `
  -Provider deepseek `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>"
#>

param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\Code\Rust",
    [string]$Repo = "D:\Code\rustbpe",
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

$smokeWrapper = Join-Path $PSScriptRoot "smoke.ps1"
if (-not (Test-Path -LiteralPath $smokeWrapper)) {
    throw "missing script: $smokeWrapper"
}

$args = New-Object System.Collections.Generic.List[string]
$args.AddRange([string[]]@(
    "-Mode", "strict",
    "-AsiExe", $AsiExe,
    "-Project", $Project,
    "-Repo", $Repo,
    "-Provider", $Provider,
    "-ReviewJsonPromptAutoTools", $ReviewJsonPromptAutoTools,
    "-ReviewJsonPromptEnvelope", $ReviewJsonPromptEnvelope,
    "-ReviewJsonSchemaRetries", ([Math]::Max(0, $ReviewJsonSchemaRetries).ToString()),
    "-ReviewJsonFailOnSchemaInvalid", $ReviewJsonFailOnSchemaInvalid,
    "-OpenAiModel", $OpenAiModel,
    "-DeepSeekModel", $DeepSeekModel
))

if (-not [string]::IsNullOrWhiteSpace($ReportDir)) {
    $args.AddRange([string[]]@("-ReportDir", $ReportDir))
}
if (-not [string]::IsNullOrWhiteSpace($SummaryOutFile)) {
    $args.AddRange([string[]]@("-SummaryOutFile", $SummaryOutFile))
}
if ($NoQuick) { $args.Add("-NoQuick") | Out-Null }
if ($GatewaySkipToolTurn) { $args.Add("-GatewaySkipToolTurn") | Out-Null }
if ($AllowProviderNetworkError) { $args.Add("-AllowProviderNetworkError") | Out-Null }
if ($AllowReviewJsonNetworkError) { $args.Add("-AllowReviewJsonNetworkError") | Out-Null }
if ($AllowSubagentNetworkError) { $args.Add("-AllowSubagentNetworkError") | Out-Null }
if ($AllowGatewayNetworkError) { $args.Add("-AllowGatewayNetworkError") | Out-Null }
if ($DryRun) { $args.Add("-DryRun") | Out-Null }
if (-not [string]::IsNullOrWhiteSpace($ReviewJsonTask)) {
    $args.AddRange([string[]]@("-ReviewJsonTask", $ReviewJsonTask))
}
if (-not [string]::IsNullOrWhiteSpace($OpenAiBaseUrl)) {
    $args.AddRange([string[]]@("-OpenAiBaseUrl", $OpenAiBaseUrl))
}
if (-not [string]::IsNullOrWhiteSpace($OpenAiApiKey)) {
    $args.AddRange([string[]]@("-OpenAiApiKey", $OpenAiApiKey))
}
if (-not [string]::IsNullOrWhiteSpace($DeepSeekBaseUrl)) {
    $args.AddRange([string[]]@("-DeepSeekBaseUrl", $DeepSeekBaseUrl))
}
if (-not [string]::IsNullOrWhiteSpace($DeepSeekApiKey)) {
    $args.AddRange([string[]]@("-DeepSeekApiKey", $DeepSeekApiKey))
}

& powershell -NoProfile -ExecutionPolicy Bypass -File $smokeWrapper @args
