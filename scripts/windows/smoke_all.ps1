<#
.SYNOPSIS
Runs the consolidated smoke suite for ASI CLI.

.DESCRIPTION
Executes api-compat/provider-model/review-json/hook-matrix/hooks-cli-advanced/
subagent/tokenizer/checkpoint/gateway smoke steps and writes a unified report.

When -StrictProfile is enabled, the following skip switches are forbidden:
  -SkipHookMatrix
  -SkipHooksCliAdvanced
  -SkipSubagent

Use StrictProfile for strict CI parity where hooks and subagent checks are mandatory.

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File .\smoke_all.ps1 -StrictProfile -RunGateway -GatewayProvider deepseek

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File .\smoke_all.ps1 -Quick -SkipGateway -SkipTokenizer
#>

param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\Code",
    [string]$Repo = "D:\Code\rustbpe",
    [int]$TimeoutSecs = 1,
    [int]$GatewayPort = 8788,
    [int]$GatewayRetries = 2,
    [int]$GatewayTimeoutSecs = 120,
    [string]$GatewayProvider = "openai",
    [switch]$GatewaySkipToolTurn,
    [string]$OpenAiBaseUrl = "",
    [string]$OpenAiApiKey = "",
    [string]$OpenAiModel = "gpt-5.3-codex",
    [string]$DeepSeekBaseUrl = "",
    [string]$DeepSeekApiKey = "",
    [string]$DeepSeekModel = "deepseek-v4-pro",
    [string]$ClaudeBaseUrl = "",
    [string]$ClaudeApiKey = "",
    [string]$ClaudeModel = "claude-3-7-sonnet-latest",
    [string]$ReviewJsonTask = "",
    [ValidateSet("on", "off")]
    [string]$ReviewJsonPromptAutoTools = "off",
    [ValidateSet("on", "off")]
    [string]$ReviewJsonPromptEnvelope = "off",
    [int]$ReviewJsonSchemaRetries = 1,
    [ValidateSet("on", "off")]
    [string]$ReviewJsonFailOnSchemaInvalid = "on",
    [switch]$AllowProviderNetworkError,
    [switch]$AllowReviewJsonNetworkError,
    [switch]$AllowSubagentNetworkError,
    [ValidateSet("json", "jsonl")]
    [string]$SubagentOutputMode = "json",
    [switch]$AllowGatewayNetworkError,
    [string]$ReportJsonPath = "",
    [string]$SummaryOutFile = "",
    [switch]$RenderSummary,
    [switch]$RunGateway,
    [switch]$StrictProfile,
    [switch]$Quick,
    [switch]$SkipApiCompat,
    [switch]$SkipProviderModel,
    [switch]$SkipReviewJson,
    [switch]$SkipHookMatrix,
    [switch]$SkipHooksCliAdvanced,
    [switch]$SkipSubagent,
    [switch]$SkipTokenizer,
    [switch]$SkipCheckpoint,
    [switch]$SkipGateway,
    [switch]$SkipOpenAi,
    [switch]$SkipDeepSeek
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$providerScript = Join-Path $PSScriptRoot "smoke_provider_model.ps1"
$apiCompatScript = Join-Path $PSScriptRoot "smoke_api_compat.ps1"
$apiCompatMinScript = Join-Path $PSScriptRoot "smoke_api_compat_min.ps1"
$tokenizerScript = Join-Path $PSScriptRoot "smoke_tokenizer_timeout.ps1"
$checkpointScript = Join-Path $PSScriptRoot "smoke_checkpoint.ps1"
$gatewayScript = Join-Path $PSScriptRoot "smoke_gateway.ps1"
$reviewJsonScript = Join-Path $PSScriptRoot "smoke_review_json.ps1"
$hookMatrixScript = Join-Path $PSScriptRoot "smoke_hook_matrix.ps1"
$hooksCliAdvancedScript = Join-Path $PSScriptRoot "smoke_hooks_cli_advanced.ps1"
$subagentScript = Join-Path $PSScriptRoot "smoke_subagent.ps1"
$renderSummaryScript = Join-Path $PSScriptRoot "render_smoke_summary.ps1"

if (-not (Test-Path -LiteralPath $providerScript)) {
    throw "missing script: $providerScript"
}
if (-not (Test-Path -LiteralPath $apiCompatScript)) {
    throw "missing script: $apiCompatScript"
}
if (-not (Test-Path -LiteralPath $apiCompatMinScript)) {
    throw "missing script: $apiCompatMinScript"
}
if (-not (Test-Path -LiteralPath $tokenizerScript)) {
    throw "missing script: $tokenizerScript"
}
if (-not (Test-Path -LiteralPath $checkpointScript)) {
    throw "missing script: $checkpointScript"
}
if (-not (Test-Path -LiteralPath $gatewayScript)) {
    throw "missing script: $gatewayScript"
}
if (-not (Test-Path -LiteralPath $reviewJsonScript)) {
    throw "missing script: $reviewJsonScript"
}
if (-not (Test-Path -LiteralPath $hookMatrixScript)) {
    throw "missing script: $hookMatrixScript"
}
if (-not (Test-Path -LiteralPath $hooksCliAdvancedScript)) {
    throw "missing script: $hooksCliAdvancedScript"
}
if (-not (Test-Path -LiteralPath $subagentScript)) {
    throw "missing script: $subagentScript"
}
if (-not (Test-Path -LiteralPath $renderSummaryScript)) {
    throw "missing script: $renderSummaryScript"
}

if (-not (Test-Path -LiteralPath $AsiExe)) {
    throw "asi binary not found: $AsiExe"
}

function Get-RandomLoopbackPort() {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        return ([System.Net.IPEndPoint]$listener.LocalEndpoint).Port
    }
    finally {
        $listener.Stop()
    }
}

if ($Quick) {
    if (-not $PSBoundParameters.ContainsKey("SkipTokenizer")) {
        $SkipTokenizer = $true
    }
    if (-not $PSBoundParameters.ContainsKey("GatewayPort")) {
        $GatewayPort = Get-RandomLoopbackPort
    }
}

if ($AllowProviderNetworkError) {
    $AllowReviewJsonNetworkError = $true
    $AllowSubagentNetworkError = $true
    $AllowGatewayNetworkError = $true
}

if ($StrictProfile) {
    if ($SkipHookMatrix) {
        throw "-SkipHookMatrix is not allowed with -StrictProfile."
    }
    if ($SkipHooksCliAdvanced) {
        throw "-SkipHooksCliAdvanced is not allowed with -StrictProfile."
    }
    if ($SkipSubagent) {
        throw "-SkipSubagent is not allowed with -StrictProfile."
    }
}

Write-Host "============================================================"
Write-Host "ASI smoke-all"
Write-Host "AsiExe: $AsiExe"
Write-Host "Project: $Project"
Write-Host "Repo: $Repo"
Write-Host "TimeoutSecs: $TimeoutSecs"
Write-Host "GatewayPort: $GatewayPort"
Write-Host "GatewayRetries: $GatewayRetries"
Write-Host "GatewayTimeoutSecs: $GatewayTimeoutSecs"
Write-Host "GatewayProvider: $GatewayProvider"
Write-Host "GatewaySkipToolTurn: $GatewaySkipToolTurn"
Write-Host "RunGateway: $RunGateway"
Write-Host "StrictProfile: $StrictProfile"
Write-Host "Quick: $Quick"
Write-Host "SkipApiCompat: $SkipApiCompat"
Write-Host "SkipProviderModel: $SkipProviderModel"
Write-Host "SkipReviewJson: $SkipReviewJson"
Write-Host "SkipHookMatrix: $SkipHookMatrix"
Write-Host "SkipHooksCliAdvanced: $SkipHooksCliAdvanced"
Write-Host "SkipSubagent: $SkipSubagent"
Write-Host "SkipTokenizer: $SkipTokenizer"
Write-Host "SkipCheckpoint: $SkipCheckpoint"
Write-Host "SkipGateway: $SkipGateway"
Write-Host "SkipOpenAi: $SkipOpenAi"
Write-Host "SkipDeepSeek: $SkipDeepSeek"
Write-Host "RenderSummary: $RenderSummary"
if (-not [string]::IsNullOrWhiteSpace($ReviewJsonTask)) {
    Write-Host "ReviewJsonTask: $ReviewJsonTask"
}
Write-Host "ReviewJsonPromptAutoTools: $ReviewJsonPromptAutoTools"
Write-Host "ReviewJsonPromptEnvelope: $ReviewJsonPromptEnvelope"
Write-Host "ReviewJsonSchemaRetries: $ReviewJsonSchemaRetries"
Write-Host "ReviewJsonFailOnSchemaInvalid: $ReviewJsonFailOnSchemaInvalid"
Write-Host "AllowProviderNetworkError: $AllowProviderNetworkError"
Write-Host "AllowReviewJsonNetworkError: $AllowReviewJsonNetworkError"
Write-Host "AllowSubagentNetworkError: $AllowSubagentNetworkError"
Write-Host "SubagentOutputMode: $SubagentOutputMode"
Write-Host "AllowGatewayNetworkError: $AllowGatewayNetworkError"
if (-not [string]::IsNullOrWhiteSpace($SummaryOutFile)) {
    Write-Host "SummaryOutFile: $SummaryOutFile"
}
Write-Host "============================================================"

$started = Get-Date
$stepReports = New-Object System.Collections.Generic.List[object]
$hasGatewayCreds = $false
$reviewJsonReportPath = ""
$hooksCliAdvancedReportPath = ""
$gatewayReportPath = ""
$subagentReportPath = ""
$summaryReportDir = ""
$summaryOutPath = if (-not [string]::IsNullOrWhiteSpace($SummaryOutFile)) { $SummaryOutFile } else { "" }

function Add-StepReport(
    [string]$Name,
    [string]$Status,
    [double]$DurationSecs = 0.0,
    [string]$Detail = ""
) {
    $step = [ordered]@{
        name = $Name
        status = $Status
        duration_secs = [Math]::Round($DurationSecs, 3)
        detail = $Detail
    }
    $null = $script:stepReports.Add($step)
}

function Invoke-Step([string]$name, [scriptblock]$action) {
    Write-Host ""
    Write-Host ("[run ] {0}" -f $name)
    $stepStart = Get-Date
    try {
        & $action
        $elapsed = (Get-Date) - $stepStart
        Add-StepReport -Name $name -Status "pass" -DurationSecs $elapsed.TotalSeconds
        Write-Host ("[pass] {0} ({1:n1}s)" -f $name, $elapsed.TotalSeconds)
    }
    catch {
        $elapsed = (Get-Date) - $stepStart
        Add-StepReport -Name $name -Status "fail" -DurationSecs $elapsed.TotalSeconds -Detail $_.Exception.Message
        throw
    }
}

function Get-SmokeAllFailureCategory([string]$Message) {
    if ([string]::IsNullOrWhiteSpace($Message)) { return "unknown" }
    $m = $Message.ToLowerInvariant()
    if ($m.Contains("[provider-error] network_error")) { return "network_error" }
    if ($m.Contains("[provider-error] auth_error")) { return "auth_error" }
    if ($m.Contains("[provider-error] quota_error")) { return "quota_error" }
    if ($m.Contains("[provider-error] model_error")) { return "model_error" }
    if ($m.Contains("[provider-error] provider_error")) { return "provider_error" }
    if ($m.Contains("error sending request for url")) { return "network_error" }
    if ($m.Contains("timed out") -or $m.Contains("timeout")) { return "network_error" }
    if ($m.Contains("connection reset") -or $m.Contains("connection refused")) { return "network_error" }
    if ($m.Contains("dns") -or $m.Contains("tls")) { return "network_error" }
    if ($m.Contains("asi binary not found")) { return "binary_missing" }
    if ($m.Contains("gateway smoke requested but openaibaseurl/openaapikey are missing")) { return "config_missing_credentials" }
    if ($m.Contains("smoke_api_compat: fail") -or $m.Contains("smoke_api_compat_min: fail")) { return "api_compat_smoke_failure" }
    if ($m.Contains("[openai-prompt]") -or $m.Contains("[openai-agent]") -or $m.Contains("[openai-min]")) { return "api_compat_smoke_failure" }
    if ($m.Contains("[deepseek-status-native]") -or $m.Contains("[deepseek-prompt]") -or $m.Contains("[deepseek-min]")) { return "api_compat_smoke_failure" }
    if ($m.Contains("provider-model smoke")) { return "provider_model_smoke_failure" }
    if ($m.Contains("review-json smoke")) { return "review_json_smoke_failure" }
    if ($m.Contains("smoke_review_json: fail")) { return "review_json_smoke_failure" }
    if ($m.Contains("[review.") -or $m.Contains("[json-parse]")) { return "review_json_smoke_failure" }
    if ($m.Contains("hook-matrix smoke")) { return "hook_matrix_smoke_failure" }
    if ($m.Contains("smoke_hook_matrix")) { return "hook_matrix_smoke_failure" }
    if ($m.Contains("hooks-cli-advanced smoke")) { return "hooks_cli_advanced_smoke_failure" }
    if ($m.Contains("smoke_hooks_cli_advanced")) { return "hooks_cli_advanced_smoke_failure" }
    if ($m.Contains("subagent smoke")) { return "subagent_smoke_failure" }
    if ($m.Contains("smoke_subagent")) { return "subagent_smoke_failure" }
    if ($m.Contains("subagent_wait_not_successful")) { return "subagent_smoke_failure" }
    if ($m.Contains("wait status is not successful")) { return "subagent_smoke_failure" }
    if ($m.Contains("tokenizer-timeout smoke")) { return "tokenizer_smoke_failure" }
    if ($m.Contains("checkpoint smoke")) { return "checkpoint_smoke_failure" }
    if (
        $m.Contains("smoke_gateway: fail") -or
        $m.Contains("failure_category=") -or
        $m.Contains("[turn-message]") -or
        $m.Contains("openai http error") -or
        $m.Contains("native tool calling failed")
    ) { return "gateway_smoke_failure" }
    return "unknown"
}

function Get-SmokeAllFailureHint([string]$Category) {
    switch ($Category) {
        "binary_missing" { return "Build release binary first: cargo build --release." }
        "network_error" { return "Check outbound connectivity, DNS, proxy, and timeout settings." }
        "auth_error" { return "Verify API key validity and provider account permissions." }
        "quota_error" { return "Check account quota/billing before retry." }
        "model_error" { return "Verify model name and model access for this key." }
        "provider_error" { return "Check provider endpoint reachability and inspect runtime provider error details." }
        "config_missing_credentials" { return "Set provider-specific gateway credentials (ApiKey/BaseUrl) or use -SkipGateway." }
        "api_compat_smoke_failure" { return "Run smoke_api_compat.ps1 or smoke_api_compat_min.ps1 standalone; validate provider keys and model access." }
        "provider_model_smoke_failure" { return "Run smoke_provider_model.ps1 standalone and inspect provider/model fallback behavior." }
        "review_json_smoke_failure" { return "Run smoke_review_json.ps1 standalone; inspect failure_category/hint and review payload fields." }
        "hook_matrix_smoke_failure" { return "Run smoke_hook_matrix.ps1 standalone and inspect ASI_HOOK_CONFIG_PATH handler behavior." }
        "hooks_cli_advanced_smoke_failure" { return "Run smoke_hooks_cli_advanced.ps1 standalone and verify edit-handler none semantics and validate --strict behavior." }
        "subagent_smoke_failure" { return "Run smoke_subagent.ps1 standalone; inspect /agent wait status/ok and provider connectivity." }
        "tokenizer_smoke_failure" { return "Run smoke_tokenizer_timeout.ps1 standalone and verify repo path and timeout settings." }
        "checkpoint_smoke_failure" { return "Run smoke_checkpoint.ps1 standalone and verify sessions/checkpoint write access." }
        "gateway_smoke_failure" { return "Run smoke_gateway.ps1 standalone; use printed failure_category/hint for diagnosis." }
        default { return "Run each sub-smoke script standalone to isolate the failing step." }
    }
}

function Parse-BoolLike([object]$Value) {
    if ($null -eq $Value) { return $false }
    $text = [string]$Value
    if ([string]::IsNullOrWhiteSpace($text)) { return $false }
    switch ($text.Trim().ToLowerInvariant()) {
        "1" { return $true }
        "true" { return $true }
        "on" { return $true }
        "yes" { return $true }
        "enabled" { return $true }
        default { return $false }
    }
}

function Normalize-GatewayProvider([string]$RawProvider) {
    $v = if ($null -eq $RawProvider) { "" } else { $RawProvider.Trim().ToLowerInvariant() }
    switch ($v) {
        "gpt" { return "openai" }
        "deepseek-ai" { return "deepseek" }
        "anthropic" { return "claude" }
        "claude-code" { return "claude" }
        "openai" { return "openai" }
        "deepseek" { return "deepseek" }
        "claude" { return "claude" }
        default {
            if ($v -eq "") { return "openai" }
            throw "Unsupported GatewayProvider: $RawProvider (expected: openai|deepseek|claude)"
        }
    }
}

function Provider-KeyEnvName([string]$ProviderName) {
    switch ($ProviderName) {
        "deepseek" { return "DEEPSEEK_API_KEY" }
        "claude" { return "ANTHROPIC_API_KEY" }
        default { return "OPENAI_API_KEY" }
    }
}

function Provider-BaseEnvName([string]$ProviderName) {
    switch ($ProviderName) {
        "deepseek" { return "DEEPSEEK_BASE_URL" }
        "claude" { return "ANTHROPIC_BASE_URL" }
        default { return "OPENAI_BASE_URL" }
    }
}

function Write-SmokeAllReport(
    [string]$Status,
    [double]$DurationSecs,
    [string]$FailureCategory,
    [string]$FailureHint,
    [string]$FailureMessage
) {
    if ([string]::IsNullOrWhiteSpace($ReportJsonPath)) {
        return
    }
    $dir = Split-Path -Parent $ReportJsonPath
    if (-not [string]::IsNullOrWhiteSpace($dir) -and -not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
    $payload = [ordered]@{
        script = "smoke_all"
        timestamp_utc = (Get-Date).ToUniversalTime().ToString("o")
        status = $Status
        duration_secs = [Math]::Round($DurationSecs, 3)
        failure_category = if ($Status -eq "fail") { $FailureCategory } else { $null }
        hint = if ($Status -eq "fail") { $FailureHint } else { $null }
        failure_message = if ($Status -eq "fail") { $FailureMessage } else { $null }
        steps = $stepReports
        config = [ordered]@{
            quick = [bool]$Quick
            run_gateway = [bool]$RunGateway
            strict_profile = [bool]$StrictProfile
            skip_api_compat = [bool]$SkipApiCompat
            skip_provider_model = [bool]$SkipProviderModel
            skip_review_json = [bool]$SkipReviewJson
            skip_hook_matrix = [bool]$SkipHookMatrix
            skip_hooks_cli_advanced = [bool]$SkipHooksCliAdvanced
            skip_subagent = [bool]$SkipSubagent
            skip_tokenizer = [bool]$SkipTokenizer
            skip_checkpoint = [bool]$SkipCheckpoint
            skip_gateway = [bool]$SkipGateway
            skip_openai = [bool]$SkipOpenAi
            skip_deepseek = [bool]$SkipDeepSeek
            timeout_secs = $TimeoutSecs
            gateway_port = $GatewayPort
            gateway_retries = $GatewayRetries
            gateway_timeout_secs = $GatewayTimeoutSecs
            gateway_credentials_provided = $hasGatewayCreds
            gateway_provider = $gatewayProviderNormalized
            gateway_skip_tool_turn = [bool]$GatewaySkipToolTurn
            openai_model = $OpenAiModel
            deepseek_model = $DeepSeekModel
            claude_model = $ClaudeModel
            review_json_task = if ([string]::IsNullOrWhiteSpace($ReviewJsonTask)) { $null } else { $ReviewJsonTask }
            review_json_prompt_auto_tools = $ReviewJsonPromptAutoTools
            review_json_prompt_envelope = $ReviewJsonPromptEnvelope
            review_json_schema_retries = $ReviewJsonSchemaRetries
            review_json_fail_on_schema_invalid = $ReviewJsonFailOnSchemaInvalid
            allow_provider_network_error = [bool]$AllowProviderNetworkError
            allow_review_json_network_error = [bool]$AllowReviewJsonNetworkError
            allow_subagent_network_error = [bool]$AllowSubagentNetworkError
            allow_gateway_network_error = [bool]$AllowGatewayNetworkError
            summary_out_file = if ([string]::IsNullOrWhiteSpace($summaryOutPath)) { $null } else { $summaryOutPath }
        }
    }
    $json = $payload | ConvertTo-Json -Depth 8
    Set-Content -LiteralPath $ReportJsonPath -Value $json -Encoding UTF8
    Write-Host ("report_json={0}" -f $ReportJsonPath)
}

function Write-SmokeSummaryIfNeeded([bool]$Force) {
    if (-not $RenderSummary -and -not $Force) {
        return
    }
    if ([string]::IsNullOrWhiteSpace($summaryReportDir)) {
        return
    }
    if ([string]::IsNullOrWhiteSpace($summaryOutPath)) {
        $summaryOutPath = Join-Path $summaryReportDir "SMOKE_SUMMARY.md"
    }
    & powershell -NoProfile -ExecutionPolicy Bypass -File $renderSummaryScript `
        -ReportDir $summaryReportDir `
        -OutFile $summaryOutPath `
        -PassThru
}

try {
    $gatewayProviderNormalized = Normalize-GatewayProvider $GatewayProvider

    if (-not $PSBoundParameters.ContainsKey("SkipOpenAi")) {
        $SkipOpenAi = Parse-BoolLike $env:ASI_SMOKE_SKIP_OPENAI
    }
    if (-not $PSBoundParameters.ContainsKey("SkipDeepSeek")) {
        $SkipDeepSeek = Parse-BoolLike $env:ASI_SMOKE_SKIP_DEEPSEEK
    }

    $effectiveOpenAiBase = if (-not [string]::IsNullOrWhiteSpace($OpenAiBaseUrl)) { $OpenAiBaseUrl } else { $env:OPENAI_BASE_URL }
    $effectiveOpenAiKey = if (-not [string]::IsNullOrWhiteSpace($OpenAiApiKey)) { $OpenAiApiKey } else { $env:OPENAI_API_KEY }
    $effectiveDeepSeekBase = if (-not [string]::IsNullOrWhiteSpace($DeepSeekBaseUrl)) { $DeepSeekBaseUrl } else { $env:DEEPSEEK_BASE_URL }
    $effectiveDeepSeekKey = if (-not [string]::IsNullOrWhiteSpace($DeepSeekApiKey)) { $DeepSeekApiKey } else { $env:DEEPSEEK_API_KEY }
    $effectiveClaudeBase = if (-not [string]::IsNullOrWhiteSpace($ClaudeBaseUrl)) { $ClaudeBaseUrl } else { $env:ANTHROPIC_BASE_URL }
    $effectiveClaudeKey = if (-not [string]::IsNullOrWhiteSpace($ClaudeApiKey)) { $ClaudeApiKey } else { $env:ANTHROPIC_API_KEY }
    $hasOpenAiCreds = -not [string]::IsNullOrWhiteSpace($effectiveOpenAiKey)
    $hasDeepSeekCreds = -not [string]::IsNullOrWhiteSpace($effectiveDeepSeekKey)
    $hasClaudeCreds = -not [string]::IsNullOrWhiteSpace($effectiveClaudeKey)

    $gatewayModel = $OpenAiModel
    $gatewayBase = $effectiveOpenAiBase
    $gatewayKey = $effectiveOpenAiKey
    switch ($gatewayProviderNormalized) {
        "deepseek" {
            $gatewayModel = $DeepSeekModel
            $gatewayBase = $effectiveDeepSeekBase
            $gatewayKey = $effectiveDeepSeekKey
        }
        "claude" {
            $gatewayModel = $ClaudeModel
            $gatewayBase = $effectiveClaudeBase
            $gatewayKey = $effectiveClaudeKey
        }
        default {
            $gatewayModel = $OpenAiModel
            $gatewayBase = $effectiveOpenAiBase
            $gatewayKey = $effectiveOpenAiKey
        }
    }
    $hasGatewayCreds = -not [string]::IsNullOrWhiteSpace($gatewayKey)
    $gatewayCredsStatus = if ($hasGatewayCreds) { "provided" } else { "not-provided" }
    Write-Host ("GatewayCreds[{0}]: {1}" -f $gatewayProviderNormalized, $gatewayCredsStatus)

    if (-not [string]::IsNullOrWhiteSpace($ReportJsonPath)) {
        $reportDir = Split-Path -Parent $ReportJsonPath
        if (-not [string]::IsNullOrWhiteSpace($reportDir)) {
            $summaryReportDir = $reportDir
            if ([string]::IsNullOrWhiteSpace($summaryOutPath)) {
                $summaryOutPath = Join-Path $reportDir "SMOKE_SUMMARY.md"
            }
            $reviewJsonReportPath = Join-Path $reportDir "smoke_review_json.json"
            $hooksCliAdvancedReportPath = Join-Path $reportDir "smoke_hooks_cli_advanced.json"
            $gatewayReportPath = Join-Path $reportDir "smoke_gateway.json"
            $subagentReportPath = Join-Path $reportDir "smoke_subagent_strict.json"
        }
    }

    if ($SkipApiCompat) {
        Write-Host ""
        Write-Host "[skip] api-compat smoke (-SkipApiCompat)"
        Add-StepReport -Name "api-compat smoke" -Status "skip" -Detail "flag: -SkipApiCompat"
    } else {
        $openAiReady = (-not $SkipOpenAi) -and $hasOpenAiCreds
        $deepSeekReady = (-not $SkipDeepSeek) -and $hasDeepSeekCreds
        if (-not $openAiReady -and -not $deepSeekReady) {
            $reasons = @()
            if (-not $SkipOpenAi -and -not $hasOpenAiCreds) { $reasons += "missing OpenAI key" }
            if (-not $SkipDeepSeek -and -not $hasDeepSeekCreds) { $reasons += "missing DeepSeek key" }
            if ($SkipOpenAi -and $SkipDeepSeek) { $reasons += "both providers skipped" }
            $detail = if ($reasons.Count -gt 0) { [string]::Join("; ", $reasons) } else { "no runnable provider" }
            Write-Host ""
            Write-Host ("[skip] api-compat smoke ({0})" -f $detail)
            Add-StepReport -Name "api-compat smoke" -Status "skip" -Detail $detail
        } else {
            $apiScriptToRun = if ($Quick) { $apiCompatMinScript } else { $apiCompatScript }
            $apiStepName = if ($Quick) { "api-compat smoke (min)" } else { "api-compat smoke" }
            $apiOpenAiSkip = $SkipOpenAi
            $apiDeepSeekSkip = $SkipDeepSeek
            if (-not $hasOpenAiCreds) {
                $apiOpenAiSkip = $true
            }
            if (-not $hasDeepSeekCreds) {
                $apiDeepSeekSkip = $true
            }
            Invoke-Step $apiStepName {
                & $apiScriptToRun `
                    -AsiExe $AsiExe `
                    -Project $Project `
                    -OpenAiBaseUrl $OpenAiBaseUrl `
                    -OpenAiApiKey $OpenAiApiKey `
                    -OpenAiModel $OpenAiModel `
                    -DeepSeekBaseUrl $DeepSeekBaseUrl `
                    -DeepSeekApiKey $DeepSeekApiKey `
                    -DeepSeekModel $DeepSeekModel `
                    -SkipOpenAi:$apiOpenAiSkip `
                    -SkipDeepSeek:$apiDeepSeekSkip
            }
        }
    }

    if ($SkipProviderModel) {
        Write-Host ""
        Write-Host "[skip] provider-model smoke (-SkipProviderModel)"
        Add-StepReport -Name "provider-model smoke" -Status "skip" -Detail "flag: -SkipProviderModel"
    } else {
        Invoke-Step "provider-model smoke" {
            & $providerScript -AsiExe $AsiExe -Project $Project
        }
    }

    if ($SkipReviewJson) {
        Write-Host ""
        Write-Host "[skip] review-json smoke (-SkipReviewJson)"
        Add-StepReport -Name "review-json smoke" -Status "skip" -Detail "flag: -SkipReviewJson"
    } else {
        if (-not $hasGatewayCreds) {
            Write-Host ""
            Write-Host ("[skip] review-json smoke (missing creds for provider={0})" -f $gatewayProviderNormalized)
            Add-StepReport -Name "review-json smoke" -Status "skip" -Detail ("missing provider credentials for " + $gatewayProviderNormalized)
        } else {
            $reviewStepStart = Get-Date
            Write-Host ""
            Write-Host "[run ] review-json smoke"
            try {
                $reviewArgs = @{
                    AsiExe = $AsiExe
                    Project = $Project
                    Provider = $gatewayProviderNormalized
                    BaseUrl = $gatewayBase
                    ApiKey = $gatewayKey
                    Model = $gatewayModel
                    PromptAutoTools = $ReviewJsonPromptAutoTools
                    PromptEnvelope = $ReviewJsonPromptEnvelope
                    SchemaRetries = [Math]::Max(0, $ReviewJsonSchemaRetries)
                    FailOnSchemaInvalid = $ReviewJsonFailOnSchemaInvalid
                    ReportJsonPath = $reviewJsonReportPath
                }
                if (-not [string]::IsNullOrWhiteSpace($ReviewJsonTask)) {
                    $reviewArgs["ReviewTask"] = $ReviewJsonTask
                }
                & $reviewJsonScript @reviewArgs
                $reviewElapsed = (Get-Date) - $reviewStepStart
                Add-StepReport -Name "review-json smoke" -Status "pass" -DurationSecs $reviewElapsed.TotalSeconds
                Write-Host ("[pass] review-json smoke ({0:n1}s)" -f $reviewElapsed.TotalSeconds)
            } catch {
                $reviewElapsed = (Get-Date) - $reviewStepStart
                $reviewDetail = $_.Exception.Message
                $allowAsWarn = $false
                if ($AllowReviewJsonNetworkError) {
                    $cat = Get-SmokeAllFailureCategory $reviewDetail
                    if ($cat -eq "network_error" -or $cat -eq "provider_error") {
                        $allowAsWarn = $true
                    }
                }
                if ($allowAsWarn) {
                    Add-StepReport -Name "review-json smoke" -Status "warn" -DurationSecs $reviewElapsed.TotalSeconds -Detail ("allowed network/provider error: " + $reviewDetail)
                    Write-Host ("[warn] review-json smoke ({0:n1}s) allowed by -AllowReviewJsonNetworkError" -f $reviewElapsed.TotalSeconds)
                } else {
                    Add-StepReport -Name "review-json smoke" -Status "fail" -DurationSecs $reviewElapsed.TotalSeconds -Detail $reviewDetail
                    throw
                }
            }
        }
    }

    if ($SkipHookMatrix) {
        Write-Host ""
        Write-Host "[skip] hook-matrix smoke (-SkipHookMatrix)"
        Add-StepReport -Name "hook-matrix smoke" -Status "skip" -Detail "flag: -SkipHookMatrix"
    } else {
        Invoke-Step "hook-matrix smoke" {
            $hookMatrixReportPath = ""
            if (-not [string]::IsNullOrWhiteSpace($summaryReportDir)) {
                $hookMatrixReportPath = Join-Path $summaryReportDir "smoke_hook_matrix.json"
            }
            if ([string]::IsNullOrWhiteSpace($hookMatrixReportPath)) {
                & $hookMatrixScript `
                    -AsiExe $AsiExe `
                    -Project $Project
            } else {
                & $hookMatrixScript `
                    -AsiExe $AsiExe `
                    -Project $Project `
                    -ReportJsonPath $hookMatrixReportPath
            }
        }
    }

    if ($SkipHooksCliAdvanced) {
        Write-Host ""
        Write-Host "[skip] hooks-cli-advanced smoke (-SkipHooksCliAdvanced)"
        Add-StepReport -Name "hooks-cli-advanced smoke" -Status "skip" -Detail "flag: -SkipHooksCliAdvanced"
    } else {
        Invoke-Step "hooks-cli-advanced smoke" {
            if ([string]::IsNullOrWhiteSpace($hooksCliAdvancedReportPath)) {
                & $hooksCliAdvancedScript `
                    -AsiExe $AsiExe `
                    -Project $Project
            } else {
                & $hooksCliAdvancedScript `
                    -AsiExe $AsiExe `
                    -Project $Project `
                    -ReportJsonPath $hooksCliAdvancedReportPath
            }
        }
    }

    if ($SkipSubagent) {
        Write-Host ""
        Write-Host "[skip] subagent smoke (-SkipSubagent)"
        Add-StepReport -Name "subagent smoke" -Status "skip" -Detail "flag: -SkipSubagent"
    } else {
        if (-not $hasGatewayCreds) {
            Write-Host ""
            Write-Host ("[skip] subagent smoke (missing creds for provider={0})" -f $gatewayProviderNormalized)
            Add-StepReport -Name "subagent smoke" -Status "skip" -Detail ("missing provider credentials for " + $gatewayProviderNormalized)
        } else {
            $subagentStepStart = Get-Date
            Write-Host ""
            Write-Host "[run ] subagent smoke"
            try {
                $keyEnvName = Provider-KeyEnvName $gatewayProviderNormalized
                $baseEnvName = Provider-BaseEnvName $gatewayProviderNormalized
                $oldKey = [Environment]::GetEnvironmentVariable($keyEnvName, "Process")
                $oldBase = [Environment]::GetEnvironmentVariable($baseEnvName, "Process")
                try {
                    if (-not [string]::IsNullOrWhiteSpace($gatewayKey)) {
                        [Environment]::SetEnvironmentVariable($keyEnvName, $gatewayKey, "Process")
                    }
                    if (-not [string]::IsNullOrWhiteSpace($gatewayBase)) {
                        [Environment]::SetEnvironmentVariable($baseEnvName, $gatewayBase, "Process")
                    }

                    if ([string]::IsNullOrWhiteSpace($subagentReportPath)) {
                        & $subagentScript `
                            -AsiExe $AsiExe `
                            -Project $Project `
                            -Provider $gatewayProviderNormalized `
                            -OutputMode $SubagentOutputMode `
                            -Model $gatewayModel
                    } else {
                        & $subagentScript `
                            -AsiExe $AsiExe `
                            -Project $Project `
                            -Provider $gatewayProviderNormalized `
                            -OutputMode $SubagentOutputMode `
                            -Model $gatewayModel `
                            -ReportJsonPath $subagentReportPath
                    }
                } finally {
                    [Environment]::SetEnvironmentVariable($keyEnvName, $oldKey, "Process")
                    [Environment]::SetEnvironmentVariable($baseEnvName, $oldBase, "Process")
                }

                $subagentElapsed = (Get-Date) - $subagentStepStart
                Add-StepReport -Name "subagent smoke" -Status "pass" -DurationSecs $subagentElapsed.TotalSeconds
                Write-Host ("[pass] subagent smoke ({0:n1}s)" -f $subagentElapsed.TotalSeconds)
            } catch {
                $subagentElapsed = (Get-Date) - $subagentStepStart
                $subagentDetail = $_.Exception.Message
                $allowSubagentWarn = $false
                if ($AllowSubagentNetworkError) {
                    $subagentCat = Get-SmokeAllFailureCategory $subagentDetail
                    if ($subagentCat -eq "network_error" -or $subagentCat -eq "provider_error") {
                        $allowSubagentWarn = $true
                    }
                }
                if ($allowSubagentWarn) {
                    Add-StepReport -Name "subagent smoke" -Status "warn" -DurationSecs $subagentElapsed.TotalSeconds -Detail ("allowed network/provider error: " + $subagentDetail)
                    Write-Host ("[warn] subagent smoke ({0:n1}s) allowed by -AllowSubagentNetworkError" -f $subagentElapsed.TotalSeconds)
                } else {
                    Add-StepReport -Name "subagent smoke" -Status "fail" -DurationSecs $subagentElapsed.TotalSeconds -Detail $subagentDetail
                    throw
                }
            }
        }
    }

    if ($SkipTokenizer) {
        Write-Host ""
        Write-Host "[skip] tokenizer-timeout smoke (-SkipTokenizer)"
        Add-StepReport -Name "tokenizer-timeout smoke" -Status "skip" -Detail "flag: -SkipTokenizer"
    } else {
        Invoke-Step "tokenizer-timeout smoke" {
            & $tokenizerScript -AsiExe $AsiExe -Repo $Repo -TimeoutSecs $TimeoutSecs
        }
    }

    if ($SkipCheckpoint) {
        Write-Host ""
        Write-Host "[skip] checkpoint smoke (-SkipCheckpoint)"
        Add-StepReport -Name "checkpoint smoke" -Status "skip" -Detail "flag: -SkipCheckpoint"
    } else {
        Invoke-Step "checkpoint smoke" {
            & $checkpointScript -AsiExe $AsiExe -Project $Project
        }
    }

    $shouldRunGateway = $RunGateway -or $hasGatewayCreds

    if ($SkipGateway) {
        Write-Host ""
        Write-Host "[skip] gateway smoke (-SkipGateway)"
        Add-StepReport -Name "gateway smoke" -Status "skip" -Detail "flag: -SkipGateway"
    } elseif ($shouldRunGateway) {
        if (-not $hasGatewayCreds) {
            throw "gateway smoke requested but provider credentials are missing for $gatewayProviderNormalized"
        }
        $gatewayStepStart = Get-Date
        Write-Host ""
        Write-Host "[run ] gateway smoke"
        try {
            & $gatewayScript `
                -AsiExe $AsiExe `
                -Project $Project `
                -Port $GatewayPort `
                -Retries $GatewayRetries `
                -TimeoutSecs $GatewayTimeoutSecs `
                -Provider $gatewayProviderNormalized `
                -BaseUrl $gatewayBase `
                -ApiKey $gatewayKey `
                -Model $gatewayModel `
                -SkipToolTurn:$GatewaySkipToolTurn `
                -OpenAiBaseUrl $OpenAiBaseUrl `
                -OpenAiApiKey $OpenAiApiKey `
                -OpenAiModel $OpenAiModel `
                -DeepSeekBaseUrl $DeepSeekBaseUrl `
                -DeepSeekApiKey $DeepSeekApiKey `
                -DeepSeekModel $DeepSeekModel `
                -ClaudeBaseUrl $ClaudeBaseUrl `
                -ClaudeApiKey $ClaudeApiKey `
                -ClaudeModel $ClaudeModel `
                -ReportJsonPath $gatewayReportPath

            $gatewayElapsed = (Get-Date) - $gatewayStepStart
            Add-StepReport -Name "gateway smoke" -Status "pass" -DurationSecs $gatewayElapsed.TotalSeconds
            Write-Host ("[pass] gateway smoke ({0:n1}s)" -f $gatewayElapsed.TotalSeconds)
        } catch {
            $gatewayElapsed = (Get-Date) - $gatewayStepStart
            $gatewayDetail = $_.Exception.Message
            $allowGatewayWarn = $false
            if ($AllowGatewayNetworkError) {
                $gatewayCat = Get-SmokeAllFailureCategory $gatewayDetail
                if ($gatewayCat -eq "network_error" -or $gatewayCat -eq "provider_error") {
                    $allowGatewayWarn = $true
                }
            }
            if ($allowGatewayWarn) {
                Add-StepReport -Name "gateway smoke" -Status "warn" -DurationSecs $gatewayElapsed.TotalSeconds -Detail ("allowed network/provider error: " + $gatewayDetail)
                Write-Host ("[warn] gateway smoke ({0:n1}s) allowed by -AllowGatewayNetworkError" -f $gatewayElapsed.TotalSeconds)
            } else {
                Add-StepReport -Name "gateway smoke" -Status "fail" -DurationSecs $gatewayElapsed.TotalSeconds -Detail $gatewayDetail
                throw
            }
        }
    } else {
        Write-Host ""
        Write-Host ("[skip] gateway smoke (set -RunGateway or provide creds for provider={0})" -f $gatewayProviderNormalized)
        Add-StepReport -Name "gateway smoke" -Status "skip" -Detail "missing gateway creds and -RunGateway not set"
    }

    $total = (Get-Date) - $started
    Write-Host ""
    Write-Host ("smoke-all: PASS ({0:n1}s)" -f $total.TotalSeconds)
    Write-SmokeAllReport -Status "pass" -DurationSecs $total.TotalSeconds -FailureCategory "" -FailureHint "" -FailureMessage ""
    Write-SmokeSummaryIfNeeded -Force:$false
}
catch {
    $raw = $_.Exception.Message
    $category = Get-SmokeAllFailureCategory $raw
    $hint = Get-SmokeAllFailureHint $category
    Write-Host ""
    Write-Host "smoke-all: FAIL"
    Write-Host ("failure_category={0}" -f $category)
    Write-Host ("hint={0}" -f $hint)
    $total = (Get-Date) - $started
    Write-SmokeAllReport -Status "fail" -DurationSecs $total.TotalSeconds -FailureCategory $category -FailureHint $hint -FailureMessage $raw
    Write-SmokeSummaryIfNeeded -Force:$true
    throw
}
