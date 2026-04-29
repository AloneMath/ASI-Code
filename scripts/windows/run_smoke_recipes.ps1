<#
.SYNOPSIS
Runs smoke recipes for ASI CLI with provider/recipe selection.

.DESCRIPTION
Dispatches to individual smoke scripts and supports bundled recipes such as:
  api-compat, review-json, gateway, hook-matrix, hooks-cli-advanced, subagent.

The recipe smoke-all-strict enforces strict coverage and forwards -StrictProfile
to smoke_all.ps1. In this mode, skip flags below are not allowed:
  -SkipHookMatrix
  -SkipHooksCliAdvanced
  -SkipSubagent

When -StrictCi is enabled, recipe is forced to smoke-all-strict.

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File .\run_smoke_recipes.ps1 -Provider deepseek -Recipe smoke-all-strict

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File .\run_smoke_recipes.ps1 -Provider openai -Recipe gateway -Quick
#>

param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\test_code",
    [string]$Repo = "D:\Code\rustbpe",
    [ValidateSet("ask", "openai", "deepseek")]
    [string]$Provider = "ask",
    [ValidateSet("ask", "api-compat", "api-compat-min", "review-json", "gateway", "hook-matrix", "hooks-cli-advanced", "subagent", "subagent-strict", "smoke-all-gateway", "smoke-all-gateway-risk", "smoke-all-strict")]
    [string]$Recipe = "ask",
    [switch]$Quick,
    [switch]$GatewaySkipToolTurn,
    [switch]$SkipHookMatrix,
    [switch]$SkipHooksCliAdvanced,
    [switch]$SkipSubagent,
    [switch]$SubagentAllowWaitError,
    [ValidateSet("json", "jsonl")]
    [string]$SubagentOutputMode = "json",
    [switch]$AllowProviderNetworkError,
    [switch]$AllowReviewJsonNetworkError,
    [switch]$AllowSubagentNetworkError,
    [switch]$AllowGatewayNetworkError,
    [switch]$StrictCi,
    [switch]$DryRun,
    [string]$ReportDir = "",
    [switch]$RenderSummary,
    [string]$SummaryOutFile = "",
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

function Resolve-Provider([string]$Value) {
    $v = $Value.Trim().ToLowerInvariant()
    switch ($v) {
        "openai" { return "openai" }
        "deepseek" { return "deepseek" }
        default { throw "Unsupported provider: $Value" }
    }
}

function Resolve-Recipe([string]$Value) {
    $v = $Value.Trim().ToLowerInvariant()
    switch ($v) {
        "api-compat" { return "api-compat" }
        "api-compat-min" { return "api-compat-min" }
        "review-json" { return "review-json" }
        "gateway" { return "gateway" }
        "hook-matrix" { return "hook-matrix" }
        "hooks-cli-advanced" { return "hooks-cli-advanced" }
        "subagent" { return "subagent" }
        "subagent-strict" { return "subagent-strict" }
        "smoke-all-gateway" { return "smoke-all-gateway" }
        "smoke-all-gateway-risk" { return "smoke-all-gateway-risk" }
        "smoke-all-strict" { return "smoke-all-strict" }
        default { throw "Unsupported recipe: $Value" }
    }
}

function Prompt-SelectProvider {
    Write-Host ""
    Write-Host "Select provider:"
    Write-Host "  1) OpenAI"
    Write-Host "  2) DeepSeek"
    $x = Read-Host "Enter 1 or 2"
    switch ($x.Trim()) {
        "1" { return "openai" }
        "2" { return "deepseek" }
        default { throw "Invalid provider selection: $x" }
    }
}

function Prompt-SelectRecipe {
    Write-Host ""
    Write-Host "Select recipe:"
    Write-Host "  1) api-compat       (full)"
    Write-Host "  2) api-compat-min   (fast)"
    Write-Host "  3) review-json      (review payload schema/stats check)"
    Write-Host "  4) gateway          (single script)"
    Write-Host "  5) hook-matrix      (ASI_HOOK_CONFIG_PATH allow/deny regression)"
    Write-Host "  6) hooks-cli-advanced (edit-handler none + validate --strict positive/negative)"
    Write-Host "  7) subagent         (spawn/list/wait envelope check)"
    Write-Host "  8) subagent-strict  (spawn/list/wait strict gate)"
    Write-Host "  9) smoke-all-gateway (one-command gateway path)"
    Write-Host " 10) smoke-all-gateway-risk (one-command gateway + risk-review relaxed)"
    Write-Host " 11) smoke-all-strict (one-command strict gateway + hooks + subagent)"
    $x = Read-Host "Enter 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, or 11"
    switch ($x.Trim()) {
        "1" { return "api-compat" }
        "2" { return "api-compat-min" }
        "3" { return "review-json" }
        "4" { return "gateway" }
        "5" { return "hook-matrix" }
        "6" { return "hooks-cli-advanced" }
        "7" { return "subagent" }
        "8" { return "subagent-strict" }
        "9" { return "smoke-all-gateway" }
        "10" { return "smoke-all-gateway-risk" }
        "11" { return "smoke-all-strict" }
        default { throw "Invalid recipe selection: $x" }
    }
}

function Ensure-FileExists([string]$PathValue, [string]$Label) {
    if (-not (Test-Path -LiteralPath $PathValue)) {
        throw "$Label not found: $PathValue"
    }
}

function Resolve-OpenAiKey {
    if (-not [string]::IsNullOrWhiteSpace($OpenAiApiKey)) { return $OpenAiApiKey }
    if (-not [string]::IsNullOrWhiteSpace($env:OPENAI_API_KEY)) { return $env:OPENAI_API_KEY }
    return ""
}

function Resolve-OpenAiBase {
    if (-not [string]::IsNullOrWhiteSpace($OpenAiBaseUrl)) { return $OpenAiBaseUrl }
    if (-not [string]::IsNullOrWhiteSpace($env:OPENAI_BASE_URL)) { return $env:OPENAI_BASE_URL }
    return "https://api.openai.com/v1"
}

function Resolve-DeepSeekKey {
    if (-not [string]::IsNullOrWhiteSpace($DeepSeekApiKey)) { return $DeepSeekApiKey }
    if (-not [string]::IsNullOrWhiteSpace($env:DEEPSEEK_API_KEY)) { return $env:DEEPSEEK_API_KEY }
    return ""
}

function Resolve-DeepSeekBase {
    if (-not [string]::IsNullOrWhiteSpace($DeepSeekBaseUrl)) { return $DeepSeekBaseUrl }
    if (-not [string]::IsNullOrWhiteSpace($env:DEEPSEEK_BASE_URL)) { return $env:DEEPSEEK_BASE_URL }
    return ""
}

function Mask-Secret([string]$s) {
    if ([string]::IsNullOrWhiteSpace($s)) { return "<empty>" }
    if ($s.Length -le 8) { return "****" }
    return $s.Substring(0, 4) + "..." + $s.Substring($s.Length - 4, 4)
}

function Add-IfValue([System.Collections.Generic.List[string]]$Target, [string]$Name, [string]$Value) {
    if (-not [string]::IsNullOrWhiteSpace($Value)) {
        $Target.Add($Name) | Out-Null
        $Target.Add($Value) | Out-Null
    }
}

function Ensure-Directory([string]$PathValue) {
    if ([string]::IsNullOrWhiteSpace($PathValue)) { return }
    if (-not (Test-Path -LiteralPath $PathValue)) {
        New-Item -ItemType Directory -Force -Path $PathValue | Out-Null
    }
}

function Quote-Arg([string]$Value) {
    return '"' + ($Value -replace '"', '\"') + '"'
}

function Build-CommandText([string]$FilePath, [string[]]$ArgumentList) {
    $parts = @(
        "powershell -NoProfile -ExecutionPolicy Bypass -File",
        (Quote-Arg $FilePath)
    )
    foreach ($a in $ArgumentList) {
        if ($a -match "\s") {
            $parts += (Quote-Arg $a)
        } else {
            $parts += $a
        }
    }
    return ($parts -join " ")
}

function Run-Or-Print([string]$FilePath, [string[]]$ArgumentList, [switch]$Dry) {
    $cmd = Build-CommandText -FilePath $FilePath -ArgumentList $ArgumentList
    Write-Host ""
    Write-Host "Command:"
    Write-Host $cmd
    if ($Dry) {
        Write-Host ""
        Write-Host "DryRun=true, command not executed."
        return
    }
    & powershell -NoProfile -ExecutionPolicy Bypass -File $FilePath @ArgumentList
}

$defaultRiskReviewTask = "inspect current project and report top bug risks"
$defaultSchemaReviewTask = "Provide exactly the required sectioned review format with one low severity finding for src/main.rs:1 and no tool calls"

$apiCompatScript = Join-Path $PSScriptRoot "smoke_api_compat.ps1"
$apiCompatMinScript = Join-Path $PSScriptRoot "smoke_api_compat_min.ps1"
$reviewJsonScript = Join-Path $PSScriptRoot "smoke_review_json.ps1"
$gatewayScript = Join-Path $PSScriptRoot "smoke_gateway.ps1"
$hookMatrixScript = Join-Path $PSScriptRoot "smoke_hook_matrix.ps1"
$hooksCliAdvancedScript = Join-Path $PSScriptRoot "smoke_hooks_cli_advanced.ps1"
$subagentScript = Join-Path $PSScriptRoot "smoke_subagent.ps1"
$smokeAllScript = Join-Path $PSScriptRoot "smoke_all.ps1"
$renderSummaryScript = Join-Path $PSScriptRoot "render_smoke_summary.ps1"

Ensure-FileExists $AsiExe "asi binary"
Ensure-FileExists $apiCompatScript "smoke_api_compat.ps1"
Ensure-FileExists $apiCompatMinScript "smoke_api_compat_min.ps1"
Ensure-FileExists $reviewJsonScript "smoke_review_json.ps1"
Ensure-FileExists $gatewayScript "smoke_gateway.ps1"
Ensure-FileExists $hookMatrixScript "smoke_hook_matrix.ps1"
Ensure-FileExists $hooksCliAdvancedScript "smoke_hooks_cli_advanced.ps1"
Ensure-FileExists $subagentScript "smoke_subagent.ps1"
Ensure-FileExists $smokeAllScript "smoke_all.ps1"
Ensure-FileExists $renderSummaryScript "render_smoke_summary.ps1"

if ($StrictCi -and $Recipe -ne "ask" -and (Resolve-Recipe $Recipe) -ne "smoke-all-strict") {
    throw "-StrictCi cannot be combined with a non-strict recipe. Use -Recipe smoke-all-strict or omit -Recipe."
}

$selectedProvider = if ($Provider -eq "ask") { Prompt-SelectProvider } else { Resolve-Provider $Provider }
$selectedRecipe = if ($StrictCi) {
    "smoke-all-strict"
} elseif ($Recipe -eq "ask") {
    Prompt-SelectRecipe
} else {
    Resolve-Recipe $Recipe
}

if ($StrictCi) {
    $RenderSummary = $true
    $SkipHookMatrix = $false
    $SkipSubagent = $false
    $SubagentAllowWaitError = $false

    if (-not $PSBoundParameters.ContainsKey("ReviewJsonPromptAutoTools")) {
        $ReviewJsonPromptAutoTools = "off"
    }
    if (-not $PSBoundParameters.ContainsKey("ReviewJsonPromptEnvelope")) {
        $ReviewJsonPromptEnvelope = "off"
    }
    if (-not $PSBoundParameters.ContainsKey("ReviewJsonSchemaRetries")) {
        $ReviewJsonSchemaRetries = 1
    }
    if (-not $PSBoundParameters.ContainsKey("ReviewJsonFailOnSchemaInvalid")) {
        $ReviewJsonFailOnSchemaInvalid = "on"
    }
    if ($ReviewJsonFailOnSchemaInvalid -ne "on") {
        throw "-StrictCi requires -ReviewJsonFailOnSchemaInvalid on."
    }
    if (-not $PSBoundParameters.ContainsKey("ReviewJsonTask")) {
        $ReviewJsonTask = $defaultSchemaReviewTask
    }
    if ([string]::IsNullOrWhiteSpace($ReportDir)) {
        $ReportDir = Join-Path (Join-Path $PSScriptRoot "..\..\artifacts") "strict_ci"
    }
}

if ($AllowProviderNetworkError) {
    $AllowReviewJsonNetworkError = $true
    $AllowSubagentNetworkError = $true
    $AllowGatewayNetworkError = $true
}

Ensure-Directory $ReportDir

$resolvedOpenAiKey = Resolve-OpenAiKey
$resolvedOpenAiBase = Resolve-OpenAiBase
$resolvedDeepSeekKey = Resolve-DeepSeekKey
$resolvedDeepSeekBase = Resolve-DeepSeekBase

Write-Host "============================================================"
Write-Host "ASI Smoke Recipes Runner"
Write-Host "Provider: $selectedProvider"
Write-Host "Recipe: $selectedRecipe"
Write-Host "AsiExe: $AsiExe"
Write-Host "Project: $Project"
Write-Host "Repo: $Repo"
Write-Host ("OpenAI key: {0}" -f (Mask-Secret $resolvedOpenAiKey))
Write-Host ("DeepSeek key: {0}" -f (Mask-Secret $resolvedDeepSeekKey))
Write-Host "Quick: $Quick"
Write-Host "GatewaySkipToolTurn: $GatewaySkipToolTurn"
Write-Host "SkipHookMatrix: $SkipHookMatrix"
Write-Host "SkipHooksCliAdvanced: $SkipHooksCliAdvanced"
Write-Host "SkipSubagent: $SkipSubagent"
Write-Host "SubagentAllowWaitError: $SubagentAllowWaitError"
Write-Host "SubagentOutputMode: $SubagentOutputMode"
Write-Host "AllowProviderNetworkError: $AllowProviderNetworkError"
Write-Host "AllowReviewJsonNetworkError: $AllowReviewJsonNetworkError"
Write-Host "AllowSubagentNetworkError: $AllowSubagentNetworkError"
Write-Host "AllowGatewayNetworkError: $AllowGatewayNetworkError"
Write-Host "StrictCi: $StrictCi"
Write-Host "DryRun: $DryRun"
if (-not [string]::IsNullOrWhiteSpace($ReportDir)) { Write-Host "ReportDir: $ReportDir" }
Write-Host "RenderSummary: $RenderSummary"
if (-not [string]::IsNullOrWhiteSpace($SummaryOutFile)) { Write-Host "SummaryOutFile: $SummaryOutFile" }
if (-not [string]::IsNullOrWhiteSpace($ReviewJsonTask)) { Write-Host "ReviewJsonTask: $ReviewJsonTask" }
Write-Host "ReviewJsonPromptAutoTools: $ReviewJsonPromptAutoTools"
Write-Host "ReviewJsonPromptEnvelope: $ReviewJsonPromptEnvelope"
Write-Host "ReviewJsonSchemaRetries: $ReviewJsonSchemaRetries"
Write-Host "ReviewJsonFailOnSchemaInvalid: $ReviewJsonFailOnSchemaInvalid"
Write-Host "============================================================"

if ($selectedProvider -eq "openai" -and [string]::IsNullOrWhiteSpace($resolvedOpenAiKey)) {
    throw "OpenAI key is missing. Pass -OpenAiApiKey or set OPENAI_API_KEY."
}
if ($selectedProvider -eq "deepseek" -and [string]::IsNullOrWhiteSpace($resolvedDeepSeekKey)) {
    throw "DeepSeek key is missing. Pass -DeepSeekApiKey or set DEEPSEEK_API_KEY."
}
if ($selectedRecipe -eq "subagent-strict" -and $SubagentAllowWaitError) {
    throw "-SubagentAllowWaitError is not allowed with -Recipe subagent-strict."
}
if ($selectedRecipe -eq "smoke-all-strict" -and $SkipHookMatrix) {
    throw "-SkipHookMatrix is not allowed with -Recipe smoke-all-strict."
}
if ($selectedRecipe -eq "smoke-all-strict" -and $SkipHooksCliAdvanced) {
    throw "-SkipHooksCliAdvanced is not allowed with -Recipe smoke-all-strict."
}
if ($selectedRecipe -eq "smoke-all-strict" -and $SkipSubagent) {
    throw "-SkipSubagent is not allowed with -Recipe smoke-all-strict."
}

switch ($selectedRecipe) {
    "api-compat" {
        $args = New-Object System.Collections.Generic.List[string]
        $args.AddRange([string[]]@(
            "-AsiExe", $AsiExe,
            "-Project", $Project,
            "-OpenAiModel", $OpenAiModel,
            "-DeepSeekModel", $DeepSeekModel
        ))
        Add-IfValue $args "-OpenAiBaseUrl" $resolvedOpenAiBase
        Add-IfValue $args "-OpenAiApiKey" $resolvedOpenAiKey
        Add-IfValue $args "-DeepSeekBaseUrl" $resolvedDeepSeekBase
        Add-IfValue $args "-DeepSeekApiKey" $resolvedDeepSeekKey
        if ($selectedProvider -eq "openai") { $args.Add("-SkipDeepSeek") | Out-Null }
        if ($selectedProvider -eq "deepseek") { $args.Add("-SkipOpenAi") | Out-Null }
        Run-Or-Print -FilePath $apiCompatScript -ArgumentList $args.ToArray() -Dry:$DryRun
        break
    }
    "api-compat-min" {
        $args = New-Object System.Collections.Generic.List[string]
        $args.AddRange([string[]]@(
            "-AsiExe", $AsiExe,
            "-Project", $Project,
            "-OpenAiModel", $OpenAiModel,
            "-DeepSeekModel", $DeepSeekModel
        ))
        Add-IfValue $args "-OpenAiBaseUrl" $resolvedOpenAiBase
        Add-IfValue $args "-OpenAiApiKey" $resolvedOpenAiKey
        Add-IfValue $args "-DeepSeekApiKey" $resolvedDeepSeekKey
        if ($selectedProvider -eq "openai") { $args.Add("-SkipDeepSeek") | Out-Null }
        if ($selectedProvider -eq "deepseek") { $args.Add("-SkipOpenAi") | Out-Null }
        Run-Or-Print -FilePath $apiCompatMinScript -ArgumentList $args.ToArray() -Dry:$DryRun
        break
    }
    "gateway" {
        $args = New-Object System.Collections.Generic.List[string]
        $args.AddRange([string[]]@(
            "-AsiExe", $AsiExe,
            "-Project", $Project,
            "-Provider", $selectedProvider
        ))
        if ($selectedProvider -eq "openai") {
            Add-IfValue $args "-BaseUrl" $resolvedOpenAiBase
            Add-IfValue $args "-ApiKey" $resolvedOpenAiKey
            Add-IfValue $args "-Model" $OpenAiModel
        } else {
            Add-IfValue $args "-BaseUrl" $resolvedDeepSeekBase
            Add-IfValue $args "-ApiKey" $resolvedDeepSeekKey
            Add-IfValue $args "-Model" $DeepSeekModel
        }
        if ($Quick) { $args.AddRange([string[]]@("-Retries", "1", "-TimeoutSecs", "90")) }
        if ($GatewaySkipToolTurn) { $args.Add("-SkipToolTurn") | Out-Null }
        if (-not [string]::IsNullOrWhiteSpace($ReportDir)) {
            $args.AddRange([string[]]@("-ReportJsonPath", (Join-Path $ReportDir "smoke_gateway.json")))
        }
        Run-Or-Print -FilePath $gatewayScript -ArgumentList $args.ToArray() -Dry:$DryRun
        break
    }
    "hook-matrix" {
        $args = New-Object System.Collections.Generic.List[string]
        $args.AddRange([string[]]@(
            "-AsiExe", $AsiExe,
            "-Project", $Project
        ))
        if ($Quick) { $args.AddRange([string[]]@("-TimeoutSecs", "30")) }
        if (-not [string]::IsNullOrWhiteSpace($ReportDir)) {
            $args.AddRange([string[]]@("-ReportJsonPath", (Join-Path $ReportDir "smoke_hook_matrix.json")))
        }
        Run-Or-Print -FilePath $hookMatrixScript -ArgumentList $args.ToArray() -Dry:$DryRun
        break
    }
    "hooks-cli-advanced" {
        $args = New-Object System.Collections.Generic.List[string]
        $args.AddRange([string[]]@(
            "-AsiExe", $AsiExe,
            "-Project", $Project
        ))
        if (-not [string]::IsNullOrWhiteSpace($ReportDir)) {
            $args.AddRange([string[]]@("-ReportJsonPath", (Join-Path $ReportDir "smoke_hooks_cli_advanced.json")))
        }
        Run-Or-Print -FilePath $hooksCliAdvancedScript -ArgumentList $args.ToArray() -Dry:$DryRun
        break
    }
    "subagent" {
        $args = New-Object System.Collections.Generic.List[string]
        $args.AddRange([string[]]@(
            "-AsiExe", $AsiExe,
            "-Project", $Project,
            "-Provider", $selectedProvider,
            "-OutputMode", $SubagentOutputMode
        ))
        if ($selectedProvider -eq "openai") {
            $args.AddRange([string[]]@("-Model", $OpenAiModel))
        } else {
            $args.AddRange([string[]]@("-Model", $DeepSeekModel))
        }
        if ($SubagentAllowWaitError) { $args.Add("-AllowWaitError") | Out-Null }
        if (-not [string]::IsNullOrWhiteSpace($ReportDir)) {
            $args.AddRange([string[]]@("-ReportJsonPath", (Join-Path $ReportDir "smoke_subagent.json")))
        }
        Run-Or-Print -FilePath $subagentScript -ArgumentList $args.ToArray() -Dry:$DryRun
        break
    }
    "subagent-strict" {
        $args = New-Object System.Collections.Generic.List[string]
        $args.AddRange([string[]]@(
            "-AsiExe", $AsiExe,
            "-Project", $Project,
            "-Provider", $selectedProvider,
            "-OutputMode", $SubagentOutputMode
        ))
        if ($selectedProvider -eq "openai") {
            $args.AddRange([string[]]@("-Model", $OpenAiModel))
        } else {
            $args.AddRange([string[]]@("-Model", $DeepSeekModel))
        }
        if (-not [string]::IsNullOrWhiteSpace($ReportDir)) {
            $args.AddRange([string[]]@("-ReportJsonPath", (Join-Path $ReportDir "smoke_subagent_strict.json")))
        }
        Run-Or-Print -FilePath $subagentScript -ArgumentList $args.ToArray() -Dry:$DryRun
        break
    }
    "review-json" {
        $args = New-Object System.Collections.Generic.List[string]
        $args.AddRange([string[]]@(
            "-AsiExe", $AsiExe,
            "-Project", $Project,
            "-Provider", $selectedProvider
        ))
        if ($selectedProvider -eq "openai") {
            Add-IfValue $args "-BaseUrl" $resolvedOpenAiBase
            Add-IfValue $args "-ApiKey" $resolvedOpenAiKey
            Add-IfValue $args "-Model" $OpenAiModel
        } else {
            Add-IfValue $args "-BaseUrl" $resolvedDeepSeekBase
            Add-IfValue $args "-ApiKey" $resolvedDeepSeekKey
            Add-IfValue $args "-Model" $DeepSeekModel
        }
        if ($Quick) { $args.AddRange([string[]]@("-Retries", "1", "-TimeoutSecs", "120")) }
        $args.AddRange([string[]]@(
            "-PromptAutoTools", $ReviewJsonPromptAutoTools,
            "-PromptEnvelope", $ReviewJsonPromptEnvelope,
            "-SchemaRetries", ([Math]::Max(0, $ReviewJsonSchemaRetries).ToString()),
            "-FailOnSchemaInvalid", $ReviewJsonFailOnSchemaInvalid
        ))
        if (-not [string]::IsNullOrWhiteSpace($ReviewJsonTask)) {
            $args.AddRange([string[]]@("-ReviewTask", $ReviewJsonTask))
        }
        if (-not [string]::IsNullOrWhiteSpace($ReportDir)) {
            $args.AddRange([string[]]@("-ReportJsonPath", (Join-Path $ReportDir "smoke_review_json.json")))
        }
        Run-Or-Print -FilePath $reviewJsonScript -ArgumentList $args.ToArray() -Dry:$DryRun
        break
    }
    "smoke-all-gateway" {
        $args = New-Object System.Collections.Generic.List[string]
        $args.AddRange([string[]]@(
            "-AsiExe", $AsiExe,
            "-Project", $Project,
            "-Repo", $Repo,
            "-RunGateway",
            "-GatewayProvider", $selectedProvider,
            "-OpenAiModel", $OpenAiModel,
            "-DeepSeekModel", $DeepSeekModel,
            "-SkipApiCompat", "-SkipProviderModel", "-SkipTokenizer", "-SkipCheckpoint"
        ))
        Add-IfValue $args "-OpenAiBaseUrl" $resolvedOpenAiBase
        Add-IfValue $args "-OpenAiApiKey" $resolvedOpenAiKey
        Add-IfValue $args "-DeepSeekBaseUrl" $resolvedDeepSeekBase
        Add-IfValue $args "-DeepSeekApiKey" $resolvedDeepSeekKey
        if ($Quick) { $args.Add("-Quick") | Out-Null }
        if ($SkipHookMatrix) { $args.Add("-SkipHookMatrix") | Out-Null }
        if ($SkipHooksCliAdvanced) { $args.Add("-SkipHooksCliAdvanced") | Out-Null }
        if ($SkipSubagent) { $args.Add("-SkipSubagent") | Out-Null }
        if ($AllowProviderNetworkError) { $args.Add("-AllowProviderNetworkError") | Out-Null }
        if ($AllowReviewJsonNetworkError) { $args.Add("-AllowReviewJsonNetworkError") | Out-Null }
        if ($AllowSubagentNetworkError) { $args.Add("-AllowSubagentNetworkError") | Out-Null }
        if ($AllowGatewayNetworkError) { $args.Add("-AllowGatewayNetworkError") | Out-Null }
        if ($GatewaySkipToolTurn) { $args.Add("-GatewaySkipToolTurn") | Out-Null }
        $args.AddRange([string[]]@("-SubagentOutputMode", $SubagentOutputMode))
        if (-not [string]::IsNullOrWhiteSpace($ReportDir)) {
            $args.AddRange([string[]]@("-ReportJsonPath", (Join-Path $ReportDir "smoke_all.json")))
        }
        $args.AddRange([string[]]@("-ReviewJsonPromptAutoTools", $ReviewJsonPromptAutoTools))
        $args.AddRange([string[]]@("-ReviewJsonPromptEnvelope", $ReviewJsonPromptEnvelope))
        $args.AddRange([string[]]@("-ReviewJsonSchemaRetries", ([Math]::Max(0, $ReviewJsonSchemaRetries).ToString())))
        $args.AddRange([string[]]@("-ReviewJsonFailOnSchemaInvalid", $ReviewJsonFailOnSchemaInvalid))
        if (-not [string]::IsNullOrWhiteSpace($ReviewJsonTask)) {
            $args.AddRange([string[]]@("-ReviewJsonTask", $ReviewJsonTask))
        }
        if (-not [string]::IsNullOrWhiteSpace($SummaryOutFile)) {
            $args.AddRange([string[]]@("-SummaryOutFile", $SummaryOutFile))
        }
        if ($RenderSummary) { $args.Add("-RenderSummary") | Out-Null }
        Run-Or-Print -FilePath $smokeAllScript -ArgumentList $args.ToArray() -Dry:$DryRun
        break
    }
    "smoke-all-gateway-risk" {
        $riskReviewTask = if (-not [string]::IsNullOrWhiteSpace($ReviewJsonTask)) {
            $ReviewJsonTask
        } else {
            $defaultRiskReviewTask
        }
        $riskPromptAutoTools = if ($PSBoundParameters.ContainsKey("ReviewJsonPromptAutoTools")) {
            $ReviewJsonPromptAutoTools
        } else {
            "on"
        }
        $riskPromptEnvelope = if ($PSBoundParameters.ContainsKey("ReviewJsonPromptEnvelope")) {
            $ReviewJsonPromptEnvelope
        } else {
            "off"
        }
        $riskSchemaRetries = if ($PSBoundParameters.ContainsKey("ReviewJsonSchemaRetries")) {
            [Math]::Max(0, $ReviewJsonSchemaRetries)
        } else {
            1
        }
        $riskFailOnSchemaInvalid = if ($PSBoundParameters.ContainsKey("ReviewJsonFailOnSchemaInvalid")) {
            $ReviewJsonFailOnSchemaInvalid
        } else {
            "off"
        }

        Write-Host "Risk recipe effective config:"
        Write-Host ("  ReviewJsonTask: {0}" -f $riskReviewTask)
        Write-Host ("  ReviewJsonPromptAutoTools: {0}" -f $riskPromptAutoTools)
        Write-Host ("  ReviewJsonPromptEnvelope: {0}" -f $riskPromptEnvelope)
        Write-Host ("  ReviewJsonSchemaRetries: {0}" -f $riskSchemaRetries)
        Write-Host ("  ReviewJsonFailOnSchemaInvalid: {0}" -f $riskFailOnSchemaInvalid)

        $args = New-Object System.Collections.Generic.List[string]
        $args.AddRange([string[]]@(
            "-AsiExe", $AsiExe,
            "-Project", $Project,
            "-Repo", $Repo,
            "-RunGateway",
            "-GatewayProvider", $selectedProvider,
            "-OpenAiModel", $OpenAiModel,
            "-DeepSeekModel", $DeepSeekModel,
            "-SkipApiCompat", "-SkipProviderModel", "-SkipTokenizer", "-SkipCheckpoint",
            "-ReviewJsonPromptAutoTools", $riskPromptAutoTools,
            "-ReviewJsonPromptEnvelope", $riskPromptEnvelope,
            "-ReviewJsonSchemaRetries", $riskSchemaRetries.ToString(),
            "-ReviewJsonFailOnSchemaInvalid", $riskFailOnSchemaInvalid,
            "-ReviewJsonTask", $riskReviewTask
        ))
        Add-IfValue $args "-OpenAiBaseUrl" $resolvedOpenAiBase
        Add-IfValue $args "-OpenAiApiKey" $resolvedOpenAiKey
        Add-IfValue $args "-DeepSeekBaseUrl" $resolvedDeepSeekBase
        Add-IfValue $args "-DeepSeekApiKey" $resolvedDeepSeekKey
        if ($Quick) { $args.Add("-Quick") | Out-Null }
        if ($SkipHookMatrix) { $args.Add("-SkipHookMatrix") | Out-Null }
        if ($SkipHooksCliAdvanced) { $args.Add("-SkipHooksCliAdvanced") | Out-Null }
        if ($SkipSubagent) { $args.Add("-SkipSubagent") | Out-Null }
        if ($AllowProviderNetworkError) { $args.Add("-AllowProviderNetworkError") | Out-Null }
        if ($AllowReviewJsonNetworkError) { $args.Add("-AllowReviewJsonNetworkError") | Out-Null }
        if ($AllowSubagentNetworkError) { $args.Add("-AllowSubagentNetworkError") | Out-Null }
        if ($AllowGatewayNetworkError) { $args.Add("-AllowGatewayNetworkError") | Out-Null }
        if ($GatewaySkipToolTurn) { $args.Add("-GatewaySkipToolTurn") | Out-Null }
        $args.AddRange([string[]]@("-SubagentOutputMode", $SubagentOutputMode))
        if (-not [string]::IsNullOrWhiteSpace($ReportDir)) {
            $args.AddRange([string[]]@("-ReportJsonPath", (Join-Path $ReportDir "smoke_all.json")))
        }
        if (-not [string]::IsNullOrWhiteSpace($SummaryOutFile)) {
            $args.AddRange([string[]]@("-SummaryOutFile", $SummaryOutFile))
        }
        if ($RenderSummary) { $args.Add("-RenderSummary") | Out-Null }
        Run-Or-Print -FilePath $smokeAllScript -ArgumentList $args.ToArray() -Dry:$DryRun
        break
    }
    "smoke-all-strict" {
        $strictReviewTask = if (-not [string]::IsNullOrWhiteSpace($ReviewJsonTask)) {
            $ReviewJsonTask
        } else {
            $defaultSchemaReviewTask
        }

        $strictPromptAutoTools = if ($PSBoundParameters.ContainsKey("ReviewJsonPromptAutoTools")) {
            $ReviewJsonPromptAutoTools
        } else {
            "off"
        }
        $strictPromptEnvelope = if ($PSBoundParameters.ContainsKey("ReviewJsonPromptEnvelope")) {
            $ReviewJsonPromptEnvelope
        } else {
            "off"
        }
        $strictSchemaRetries = if ($PSBoundParameters.ContainsKey("ReviewJsonSchemaRetries")) {
            [Math]::Max(0, $ReviewJsonSchemaRetries)
        } else {
            1
        }
        $strictFailOnSchemaInvalid = if ($PSBoundParameters.ContainsKey("ReviewJsonFailOnSchemaInvalid")) {
            $ReviewJsonFailOnSchemaInvalid
        } else {
            "on"
        }

        Write-Host "Strict recipe effective config:"
        Write-Host ("  ReviewJsonTask: {0}" -f $strictReviewTask)
        Write-Host ("  ReviewJsonPromptAutoTools: {0}" -f $strictPromptAutoTools)
        Write-Host ("  ReviewJsonPromptEnvelope: {0}" -f $strictPromptEnvelope)
        Write-Host ("  ReviewJsonSchemaRetries: {0}" -f $strictSchemaRetries)
        Write-Host ("  ReviewJsonFailOnSchemaInvalid: {0}" -f $strictFailOnSchemaInvalid)
        Write-Host "  HookMatrix: required"
        Write-Host "  HooksCliAdvanced: required"
        Write-Host "  Subagent: required"

        $args = New-Object System.Collections.Generic.List[string]
        $args.AddRange([string[]]@(
            "-AsiExe", $AsiExe,
            "-Project", $Project,
            "-Repo", $Repo,
            "-RunGateway",
            "-GatewayProvider", $selectedProvider,
            "-OpenAiModel", $OpenAiModel,
            "-DeepSeekModel", $DeepSeekModel,
            "-StrictProfile",
            "-SkipApiCompat", "-SkipProviderModel", "-SkipTokenizer", "-SkipCheckpoint",
            "-ReviewJsonPromptAutoTools", $strictPromptAutoTools,
            "-ReviewJsonPromptEnvelope", $strictPromptEnvelope,
            "-ReviewJsonSchemaRetries", $strictSchemaRetries.ToString(),
            "-ReviewJsonFailOnSchemaInvalid", $strictFailOnSchemaInvalid,
            "-ReviewJsonTask", $strictReviewTask
        ))
        Add-IfValue $args "-OpenAiBaseUrl" $resolvedOpenAiBase
        Add-IfValue $args "-OpenAiApiKey" $resolvedOpenAiKey
        Add-IfValue $args "-DeepSeekBaseUrl" $resolvedDeepSeekBase
        Add-IfValue $args "-DeepSeekApiKey" $resolvedDeepSeekKey
        if ($Quick) { $args.Add("-Quick") | Out-Null }
        if ($GatewaySkipToolTurn) { $args.Add("-GatewaySkipToolTurn") | Out-Null }
        if ($AllowProviderNetworkError) { $args.Add("-AllowProviderNetworkError") | Out-Null }
        if ($AllowReviewJsonNetworkError) { $args.Add("-AllowReviewJsonNetworkError") | Out-Null }
        if ($AllowSubagentNetworkError) { $args.Add("-AllowSubagentNetworkError") | Out-Null }
        if ($AllowGatewayNetworkError) { $args.Add("-AllowGatewayNetworkError") | Out-Null }
        if (-not [string]::IsNullOrWhiteSpace($ReportDir)) {
            $args.AddRange([string[]]@("-ReportJsonPath", (Join-Path $ReportDir "smoke_all_strict.json")))
        }
        $args.AddRange([string[]]@("-SubagentOutputMode", $SubagentOutputMode))
        if (-not [string]::IsNullOrWhiteSpace($SummaryOutFile)) {
            $args.AddRange([string[]]@("-SummaryOutFile", $SummaryOutFile))
        }
        if ($RenderSummary) { $args.Add("-RenderSummary") | Out-Null }
        Run-Or-Print -FilePath $smokeAllScript -ArgumentList $args.ToArray() -Dry:$DryRun
        break
    }
}

if ($RenderSummary -and (-not [string]::IsNullOrWhiteSpace($ReportDir)) -and ($selectedRecipe -ne "smoke-all-gateway") -and ($selectedRecipe -ne "smoke-all-gateway-risk")) {
    $summaryOut = if (-not [string]::IsNullOrWhiteSpace($SummaryOutFile)) {
        $SummaryOutFile
    } else {
        Join-Path $ReportDir "SMOKE_SUMMARY.md"
    }
    Run-Or-Print -FilePath $renderSummaryScript -ArgumentList @(
        "-ReportDir", $ReportDir,
        "-OutFile", $summaryOut,
        "-PassThru"
    ) -Dry:$DryRun
}
