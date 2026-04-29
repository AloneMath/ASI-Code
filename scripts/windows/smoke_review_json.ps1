param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\Code\Rust",
    [ValidateSet("openai", "deepseek", "claude")]
    [string]$Provider = "openai",
    [string]$BaseUrl = "",
    [string]$ApiKey = "",
    [string]$Model = "",
    [string]$OpenAiBaseUrl = "",
    [string]$OpenAiApiKey = "",
    [string]$OpenAiModel = "gpt-5.3-codex",
    [string]$DeepSeekBaseUrl = "",
    [string]$DeepSeekApiKey = "",
    [string]$DeepSeekModel = "deepseek-v4-pro",
    [string]$ClaudeBaseUrl = "",
    [string]$ClaudeApiKey = "",
    [string]$ClaudeModel = "claude-3-7-sonnet-latest",
    [string]$ReviewTask = "Provide exactly the required sectioned review format with one low severity finding for src/main.rs:1 and no tool calls",
    [int]$Retries = 2,
    [int]$TimeoutSecs = 240,
    [ValidateSet("on", "off")]
    [string]$PromptAutoTools = "off",
    [int]$SchemaRetries = 1,
    [ValidateSet("on", "off")]
    [string]$FailOnSchemaInvalid = "on",
    [ValidateSet("on", "off")]
    [string]$PromptEnvelope = "off",
    [switch]$UseEnvelope,
    [switch]$DryRun,
    [string]$ReportJsonPath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Normalize-Provider([string]$RawProvider) {
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
            throw "Unsupported provider: $RawProvider (expected: openai|deepseek|claude)"
        }
    }
}

function Provider-KeyEnv([string]$ProviderName) {
    switch ($ProviderName) {
        "deepseek" { return "DEEPSEEK_API_KEY" }
        "claude" { return "ANTHROPIC_API_KEY" }
        default { return "OPENAI_API_KEY" }
    }
}

function Provider-BaseEnv([string]$ProviderName) {
    switch ($ProviderName) {
        "deepseek" { return "DEEPSEEK_BASE_URL" }
        "claude" { return "ANTHROPIC_BASE_URL" }
        default { return "OPENAI_BASE_URL" }
    }
}

function Provider-DefaultModel([string]$ProviderName) {
    switch ($ProviderName) {
        "deepseek" { return "deepseek-v4-pro" }
        "claude" { return "claude-3-7-sonnet-latest" }
        default { return "gpt-5.3-codex" }
    }
}

function Provider-Label([string]$ProviderName) {
    switch ($ProviderName) {
        "deepseek" { return "DeepSeek" }
        "claude" { return "Claude" }
        default { return "OpenAI" }
    }
}

function Assert-True([bool]$Cond, [string]$Label, [string]$Detail) {
    if (-not $Cond) {
        throw "[$Label] $Detail"
    }
}

function Get-ObjectProperty([object]$Obj, [string]$Name) {
    if ($null -eq $Obj) { return $null }
    $prop = $Obj.PSObject.Properties[$Name]
    if ($null -eq $prop) { return $null }
    Write-Output -NoEnumerate $prop.Value
}

function Array-Count([object]$Value) {
    return @($Value).Count
}

function Ensure-Array([object]$Value, [string]$Label) {
    if ($null -eq $Value) {
        throw "[$Label] missing"
    }
    # Normalize both true arrays and single-item values (PowerShell JSON often collapses 1-item arrays)
    return @($Value)
}

function Severity-Rank([string]$RawSeverity) {
    $s = if ($null -eq $RawSeverity) { "" } else { $RawSeverity.Trim().ToLowerInvariant() }
    switch ($s) {
        "critical" { return 0 }
        "high" { return 1 }
        "medium" { return 2 }
        "low" { return 3 }
        default { return 4 }
    }
}

function Invoke-WithRetry([string]$Name, [scriptblock]$Action, [int]$MaxAttempts = 2, [int]$DelayMs = 1200) {
    $last = $null
    for ($attempt = 1; $attempt -le $MaxAttempts; $attempt++) {
        try {
            return (& $Action)
        }
        catch {
            $last = $_
            if ($attempt -ge $MaxAttempts) {
                throw
            }
            Write-Host ("WARN {0} attempt {1}/{2} failed: {3}" -f $Name, $attempt, $MaxAttempts, $_.Exception.Message)
            Start-Sleep -Milliseconds $DelayMs
        }
    }
    throw $last
}

function Should-RetryOnProviderError([string]$Message) {
    if ([string]::IsNullOrWhiteSpace($Message)) { return $false }
    $cat = Get-FailureCategory $Message
    if ($cat -eq "network_error" -or $cat -eq "provider_error") {
        return $true
    }
    # When provider category text is passed directly (e.g. "network_error"),
    # treat it as retryable too.
    $norm = $Message.Trim().ToLowerInvariant()
    return $norm -eq "network_error" -or $norm -eq "provider_error"
}

function Parse-JsonOutput([string]$OutputText) {
    if ([string]::IsNullOrWhiteSpace($OutputText)) {
        throw "[json-parse] empty output"
    }
    try {
        return ($OutputText | ConvertFrom-Json)
    }
    catch {}

    $start = $OutputText.IndexOf("{")
    $end = $OutputText.LastIndexOf("}")
    if ($start -lt 0 -or $end -lt 0 -or $end -le $start) {
        throw "[json-parse] could not find JSON object boundaries"
    }
    $candidate = $OutputText.Substring($start, $end - $start + 1)
    try {
        return ($candidate | ConvertFrom-Json)
    }
    catch {
        throw "[json-parse] failed to parse extracted JSON object: $($_.Exception.Message)"
    }
}

function Validate-ReviewPayload([object]$ReviewPayload) {
    Assert-True ($null -ne $ReviewPayload) "review-payload" "payload is null"
    $schemaVersion = Get-ObjectProperty $ReviewPayload "schema_version"
    Assert-True ($null -ne $schemaVersion) "review.schema_version" "field missing"
    Assert-True ([string]$schemaVersion -eq "1") "review.schema_version" ("unexpected value: " + [string]$schemaVersion)

    $isReviewTask = Get-ObjectProperty $ReviewPayload "is_review_task"
    Assert-True ($null -ne $isReviewTask) "review.is_review_task" "field missing"
    Assert-True ([bool]$isReviewTask) "review.is_review_task" "expected true"

    $schemaValid = Get-ObjectProperty $ReviewPayload "schema_valid"
    $schemaError = Get-ObjectProperty $ReviewPayload "schema_error"
    Assert-True ($null -ne $schemaValid) "review.schema_valid" "field missing"
    Assert-True ([bool]$schemaValid) "review.schema_valid" ("schema invalid: " + [string]$schemaError)

    $sections = Get-ObjectProperty $ReviewPayload "sections"
    Assert-True ($null -ne $sections) "review.sections" "field missing"
    $findings = Ensure-Array (Get-ObjectProperty $sections "findings") "review.sections.findings"
    $findingsSorted = Ensure-Array (Get-ObjectProperty $sections "findings_sorted") "review.sections.findings_sorted"
    $missingTests = Ensure-Array (Get-ObjectProperty $sections "missing_tests") "review.sections.missing_tests"
    $openQuestions = Ensure-Array (Get-ObjectProperty $sections "open_questions") "review.sections.open_questions"
    $summary = Ensure-Array (Get-ObjectProperty $sections "summary") "review.sections.summary"
    Assert-True ((Array-Count $findingsSorted) -eq (Array-Count $findings)) "review.sections.findings_sorted" "length mismatch with findings"

    $stats = Get-ObjectProperty $ReviewPayload "stats"
    Assert-True ($null -ne $stats) "review.stats" "field missing"
    $statsHasFindings = Get-ObjectProperty $stats "has_findings"
    $statsTotalFindings = Get-ObjectProperty $stats "total_findings"
    $statsMissingTestsCount = Get-ObjectProperty $stats "missing_tests_count"
    Assert-True ($null -ne $statsHasFindings) "review.stats.has_findings" "field missing"
    Assert-True ($null -ne $statsTotalFindings) "review.stats.total_findings" "field missing"
    Assert-True ($null -ne $statsMissingTestsCount) "review.stats.missing_tests_count" "field missing"
    Assert-True ([int64]$statsTotalFindings -eq [int64](Array-Count $findings)) "review.stats.total_findings" "count does not match findings length"
    Assert-True ([int64]$statsMissingTestsCount -ge 0) "review.stats.missing_tests_count" "must be >= 0"

    $severityCounts = Get-ObjectProperty $stats "severity_counts"
    Assert-True ($null -ne $severityCounts) "review.stats.severity_counts" "field missing"
    foreach ($key in @("critical", "high", "medium", "low", "unknown")) {
        $value = Get-ObjectProperty $severityCounts $key
        Assert-True ($null -ne $value) ("review.stats.severity_counts." + $key) "field missing"
        $null = [int64]$value
    }

    $topRiskPaths = Ensure-Array (Get-ObjectProperty $stats "top_risk_paths") "review.stats.top_risk_paths"
    foreach ($entry in $topRiskPaths) {
        $entryPath = Get-ObjectProperty $entry "path"
        $entryRiskScore = Get-ObjectProperty $entry "risk_score"
        $entryFindings = Get-ObjectProperty $entry "findings"
        $entrySeverityCounts = Get-ObjectProperty $entry "severity_counts"
        Assert-True ($null -ne $entryPath) "review.stats.top_risk_paths.path" "field missing"
        Assert-True ($null -ne $entryRiskScore) "review.stats.top_risk_paths.risk_score" "field missing"
        Assert-True ($null -ne $entryFindings) "review.stats.top_risk_paths.findings" "field missing"
        Assert-True ($null -ne $entrySeverityCounts) "review.stats.top_risk_paths.severity_counts" "field missing"
        $null = [int64]$entryRiskScore
        $null = [int64]$entryFindings
    }

    $previousRank = -1
    foreach ($entry in $findingsSorted) {
        $normalized = [string](Get-ObjectProperty $entry "normalized_severity")
        if ([string]::IsNullOrWhiteSpace($normalized)) {
            $normalized = [string](Get-ObjectProperty $entry "severity")
        }
        $rank = Severity-Rank $normalized
        Assert-True ($rank -ge $previousRank) "review.sections.findings_sorted" "severity order is not monotonic"
        $previousRank = $rank
    }

    return [ordered]@{
        total_findings = [int64]$statsTotalFindings
        missing_tests_count = [int64]$statsMissingTestsCount
        findings_count = [int64](Array-Count $findings)
        findings_sorted_count = [int64](Array-Count $findingsSorted)
        top_risk_paths_count = [int64](Array-Count $topRiskPaths)
        open_questions_count = [int64](Array-Count $openQuestions)
        summary_count = [int64](Array-Count $summary)
    }
}

function Get-FailureCategory([string]$Message) {
    if ([string]::IsNullOrWhiteSpace($Message)) { return "unknown" }
    $m = $Message.ToLowerInvariant()
    if ($m.Contains("baseurl/apikey are required")) { return "config_missing_credentials" }
    if ($m.Contains("missing openai_api_key")) { return "config_missing_credentials" }
    if ($m.Contains("missing deepseek_api_key")) { return "config_missing_credentials" }
    if ($m.Contains("missing anthropic_api_key")) { return "config_missing_credentials" }
    if ($m.Contains("401") -or $m.Contains("403") -or $m.Contains("unauthorized") -or $m.Contains("invalid api key") -or $m.Contains("invalid token") -or $m.Contains("forbidden")) { return "auth_error" }
    if ($m.Contains("quota") -or $m.Contains("insufficient_quota") -or $m.Contains("billing")) { return "quota_error" }
    if ($m.Contains("model not exist") -or $m.Contains("model_not_found") -or $m.Contains("unknown model") -or $m.Contains("invalid model")) { return "model_error" }
    if ($m.Contains("timed out") -or $m.Contains("timeout") -or $m.Contains("connection refused") -or $m.Contains("connection reset") -or $m.Contains("name or service not known") -or $m.Contains("no such host") -or $m.Contains("dns") -or $m.Contains("error sending request for url")) { return "network_error" }
    if ($m.Contains("[provider-error]") -or $m.Contains("provider_error")) { return "provider_error" }
    if ($m.Contains("[json-parse]")) { return "json_decode_error" }
    if ($m.Contains("[json_only.status]") -or $m.Contains("[json_only.schema_version]")) { return "review_json_envelope_invalid" }
    if ($m.Contains("[review.schema_valid]")) { return "review_schema_invalid" }
    if ($m.Contains("[review.")) { return "review_payload_missing_field" }
    if ($m.Contains("prompt command failed")) { return "prompt_command_failure" }
    return "output_validation_failure"
}

function Get-BoolProperty([object]$Obj, [string]$Name) {
    $v = Get-ObjectProperty $Obj $Name
    if ($null -eq $v) { return $null }
    try { return [bool]$v } catch { return $null }
}

function Get-ProviderErrorCategoryFromJson([object]$JsonObject) {
    if ($null -eq $JsonObject) { return $null }
    $providerError = Get-BoolProperty $JsonObject "provider_error"
    $stopAlias = [string](Get-ObjectProperty $JsonObject "stop_reason_alias")
    $stopRaw = [string](Get-ObjectProperty $JsonObject "stop_reason")
    if (($null -ne $providerError -and $providerError) -or $stopAlias -eq "provider_error" -or $stopRaw -eq "provider_error") {
        $msg = [string](Get-ObjectProperty $JsonObject "provider_error_message")
        if ([string]::IsNullOrWhiteSpace($msg)) {
            $msg = [string](Get-ObjectProperty $JsonObject "message")
        }
        if ([string]::IsNullOrWhiteSpace($msg)) {
            return "provider_error"
        }
        return (Get-FailureCategory $msg)
    }
    return $null
}

function Get-FailureHint([string]$Category) {
    switch ($Category) {
        "config_missing_credentials" { return "Provide provider credentials via -ApiKey/-BaseUrl or provider env vars." }
        "auth_error" { return "Verify API key validity and provider account permissions." }
        "quota_error" { return "Check account quota/billing before retry." }
        "model_error" { return "Verify model name and model access for this key." }
        "network_error" { return "Check outbound connectivity, DNS, proxy, and timeout settings." }
        "provider_error" { return "Check provider endpoint reachability and inspect runtime provider error details." }
        "json_decode_error" { return "Inspect raw output; ensure prompt command runs with --output-format json." }
        "review_json_envelope_invalid" { return "PromptEnvelope=on requires top-level schema_version=1 and status=ok for /review --json-only success output." }
        "review_schema_invalid" { return "Inspect review schema_error and prompt behavior for /review output." }
        "review_payload_missing_field" { return "Check review payload fields in CLI output and parser compatibility." }
        "prompt_command_failure" { return "Check provider/model/project arguments and runtime stderr output." }
        default { return "Inspect full output and stack message to identify root cause." }
    }
}

function Write-ReviewJsonReport(
    [string]$Status,
    [double]$DurationSecs,
    [string]$FailureCategory,
    [string]$FailureHint,
    [string]$FailureMessage,
    [string]$ProviderName,
    [string]$ModelName,
    [string]$PromptMode,
    [hashtable]$Metrics
) {
    if ([string]::IsNullOrWhiteSpace($ReportJsonPath)) {
        return
    }
    $dir = Split-Path -Parent $ReportJsonPath
    if (-not [string]::IsNullOrWhiteSpace($dir) -and -not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
    $payload = [ordered]@{
        script = "smoke_review_json"
        timestamp_utc = (Get-Date).ToUniversalTime().ToString("o")
        status = $Status
        duration_secs = [Math]::Round($DurationSecs, 3)
        failure_category = if ($Status -eq "fail") { $FailureCategory } else { $null }
        hint = if ($Status -eq "fail") { $FailureHint } else { $null }
        failure_message = if ($Status -eq "fail") { $FailureMessage } else { $null }
        config = [ordered]@{
            provider = $ProviderName
            model = $ModelName
            project = $Project
            prompt_mode = $PromptMode
            prompt_auto_tools = $PromptAutoTools
            prompt_envelope = $PromptEnvelope
            schema_retries = $SchemaRetries
            fail_on_schema_invalid = $FailOnSchemaInvalid
            retries = $Retries
            timeout_secs = $TimeoutSecs
            review_task = $ReviewTask
        }
        metrics = $Metrics
    }
    $json = $payload | ConvertTo-Json -Depth 10
    Set-Content -LiteralPath $ReportJsonPath -Value $json -Encoding UTF8
    Write-Host ("report_json={0}" -f $ReportJsonPath)
}

if (-not (Test-Path -LiteralPath $AsiExe)) {
    throw "asi binary not found: $AsiExe"
}

$providerName = Normalize-Provider $Provider
$providerLabel = Provider-Label $providerName

$resolvedBaseUrl = $BaseUrl
$resolvedApiKey = $ApiKey
$resolvedModel = $Model

if ($providerName -eq "openai") {
    if (-not [string]::IsNullOrWhiteSpace($OpenAiBaseUrl)) { $resolvedBaseUrl = $OpenAiBaseUrl }
    if (-not [string]::IsNullOrWhiteSpace($OpenAiApiKey)) { $resolvedApiKey = $OpenAiApiKey }
    if ([string]::IsNullOrWhiteSpace($resolvedModel) -and -not [string]::IsNullOrWhiteSpace($OpenAiModel)) {
        $resolvedModel = $OpenAiModel
    }
} elseif ($providerName -eq "deepseek") {
    if (-not [string]::IsNullOrWhiteSpace($DeepSeekBaseUrl)) { $resolvedBaseUrl = $DeepSeekBaseUrl }
    if (-not [string]::IsNullOrWhiteSpace($DeepSeekApiKey)) { $resolvedApiKey = $DeepSeekApiKey }
    if ([string]::IsNullOrWhiteSpace($resolvedModel) -and -not [string]::IsNullOrWhiteSpace($DeepSeekModel)) {
        $resolvedModel = $DeepSeekModel
    }
} else {
    if (-not [string]::IsNullOrWhiteSpace($ClaudeBaseUrl)) { $resolvedBaseUrl = $ClaudeBaseUrl }
    if (-not [string]::IsNullOrWhiteSpace($ClaudeApiKey)) { $resolvedApiKey = $ClaudeApiKey }
    if ([string]::IsNullOrWhiteSpace($resolvedModel) -and -not [string]::IsNullOrWhiteSpace($ClaudeModel)) {
        $resolvedModel = $ClaudeModel
    }
}

$keyEnvName = Provider-KeyEnv $providerName
$baseEnvName = Provider-BaseEnv $providerName
$envKeyValue = [Environment]::GetEnvironmentVariable($keyEnvName, "Process")
$envBaseValue = [Environment]::GetEnvironmentVariable($baseEnvName, "Process")
if ([string]::IsNullOrWhiteSpace($resolvedApiKey)) { $resolvedApiKey = $envKeyValue }
if ([string]::IsNullOrWhiteSpace($resolvedBaseUrl)) { $resolvedBaseUrl = $envBaseValue }
if ([string]::IsNullOrWhiteSpace($resolvedModel)) { $resolvedModel = Provider-DefaultModel $providerName }

if ([string]::IsNullOrWhiteSpace($resolvedApiKey)) {
    throw "$providerLabel BaseUrl/ApiKey are required for review-json smoke (or set $keyEnvName)"
}

$oldOpenAiBase = $env:OPENAI_BASE_URL
$oldOpenAiKey = $env:OPENAI_API_KEY
$oldDeepSeekBase = $env:DEEPSEEK_BASE_URL
$oldDeepSeekKey = $env:DEEPSEEK_API_KEY
$oldClaudeBase = $env:ANTHROPIC_BASE_URL
$oldClaudeKey = $env:ANTHROPIC_API_KEY
$oldPromptEnvelope = $env:ASI_REVIEW_JSON_ONLY_PROMPT_ENVELOPE
$started = Get-Date
$promptMode = if ($UseEnvelope) { "envelope" } else { "json-only" }

try {
    if ($providerName -eq "openai") {
        if (-not [string]::IsNullOrWhiteSpace($resolvedBaseUrl)) { $env:OPENAI_BASE_URL = $resolvedBaseUrl }
        $env:OPENAI_API_KEY = $resolvedApiKey
    } elseif ($providerName -eq "deepseek") {
        if (-not [string]::IsNullOrWhiteSpace($resolvedBaseUrl)) { $env:DEEPSEEK_BASE_URL = $resolvedBaseUrl }
        $env:DEEPSEEK_API_KEY = $resolvedApiKey
    } else {
        if (-not [string]::IsNullOrWhiteSpace($resolvedBaseUrl)) { $env:ANTHROPIC_BASE_URL = $resolvedBaseUrl }
        $env:ANTHROPIC_API_KEY = $resolvedApiKey
    }
    $env:ASI_REVIEW_JSON_ONLY_PROMPT_ENVELOPE = $PromptEnvelope

    $promptText = if ($UseEnvelope) {
        "/review $ReviewTask"
    } else {
        "/review $ReviewTask --json-only"
    }

    Write-Host "Using binary: $AsiExe"
    Write-Host "Provider: $providerName"
    Write-Host "Project: $Project"
    Write-Host "Model: $resolvedModel"
    Write-Host "PromptMode: $promptMode"
    Write-Host "Retries: $Retries"
    Write-Host "TimeoutSecs: $TimeoutSecs"
    Write-Host "SchemaRetries: $SchemaRetries"
    Write-Host "FailOnSchemaInvalid: $FailOnSchemaInvalid"
    Write-Host "PromptEnvelope: $PromptEnvelope"
    Write-Host "ReviewTask: $ReviewTask"

    if ($DryRun) {
        Write-Host ""
        Write-Host "DryRun command:"
        Write-Host "& `"$AsiExe`" prompt `"$promptText`" --provider $providerName --model `"$resolvedModel`" --project `"$Project`" --output-format json --prompt-auto-tools $PromptAutoTools"
        Write-Host "smoke_review_json: DRY_RUN"
        exit 0
    }

    Write-Host "[1/3] run prompt"
    $effectiveRetries = [Math]::Max(1, $Retries)
    $providerRetryBudget = [Math]::Max(2, $effectiveRetries + 1)
    $effectiveTimeout = [Math]::Max(30, $TimeoutSecs)
    $output = Invoke-WithRetry "prompt-review-json" {
        $prev = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $result = & $AsiExe prompt $promptText --provider $providerName --model $resolvedModel --project $Project --output-format json --prompt-auto-tools $PromptAutoTools 2>&1
            $exitCode = $LASTEXITCODE
            $text = ($result | Out-String -Width 4096)
            if ($exitCode -ne 0) {
                throw "prompt command failed (exit=$exitCode):`n$text"
            }
            return $text
        }
        finally {
            $ErrorActionPreference = $prev
        }
    } -MaxAttempts $effectiveRetries -DelayMs 1400
    Assert-True (-not [string]::IsNullOrWhiteSpace($output)) "prompt-output" "empty output"
    Write-Host "[ok] prompt"

    Write-Host "[2/3] parse json"
    $jsonObject = $null
    $reviewPayload = $null
    $parseAttempt = 1
    while ($parseAttempt -le $providerRetryBudget) {
        $jsonObject = Parse-JsonOutput $output
        $providerCategory = Get-ProviderErrorCategoryFromJson $jsonObject
        if ($null -eq $providerCategory) {
            break
        }
        if ($parseAttempt -ge $providerRetryBudget -or -not (Should-RetryOnProviderError $providerCategory)) {
            throw ("[provider-error] " + $providerCategory)
        }
        $nextAttempt = $parseAttempt + 1
        Write-Host ("WARN provider transient parse-stage failure attempt {0}/{1}: {2}" -f $parseAttempt, $providerRetryBudget, $providerCategory)
        Write-Host ("[retry] rerunning prompt after provider error attempt {0}/{1}" -f $nextAttempt, $providerRetryBudget)
        Start-Sleep -Milliseconds (1200 * [Math]::Pow(2, [Math]::Min(3, $parseAttempt - 1)))
        $output = Invoke-WithRetry "prompt-review-json-provider-retry" {
            $prev = $ErrorActionPreference
            $ErrorActionPreference = "Continue"
            try {
                $result = & $AsiExe prompt $promptText --provider $providerName --model $resolvedModel --project $Project --output-format json --prompt-auto-tools $PromptAutoTools 2>&1
                $exitCode = $LASTEXITCODE
                $text = ($result | Out-String -Width 4096)
                if ($exitCode -ne 0) {
                    throw "prompt command failed (exit=$exitCode):`n$text"
                }
                return $text
            }
            finally {
                $ErrorActionPreference = $prev
            }
        } -MaxAttempts $effectiveRetries -DelayMs 1400
        $parseAttempt += 1
    }
    if ($UseEnvelope) {
        $reviewPayload = Get-ObjectProperty $jsonObject "review"
    } else {
        if ($PromptEnvelope -eq "on") {
            $jsonOnlyStatus = [string](Get-ObjectProperty $jsonObject "status")
            $jsonOnlySchemaVersion = [string](Get-ObjectProperty $jsonObject "schema_version")
            Assert-True (-not [string]::IsNullOrWhiteSpace($jsonOnlyStatus)) "json_only.status" "field missing (PromptEnvelope=on)"
            Assert-True ($jsonOnlyStatus -eq "ok") "json_only.status" ("expected ok, got: " + $jsonOnlyStatus)
            Assert-True ($jsonOnlySchemaVersion -eq "1") "json_only.schema_version" ("unexpected value: " + $jsonOnlySchemaVersion)
            $reviewPayload = Get-ObjectProperty $jsonObject "review"
        } else {
            $jsonOnlyReviewTaskFlag = Get-ObjectProperty $jsonObject "is_review_task"
            if ($null -ne $jsonOnlyReviewTaskFlag) {
                $reviewPayload = $jsonObject
            } else {
                $reviewPayload = Get-ObjectProperty $jsonObject "review"
            }
        }
    }
    if ($null -eq $reviewPayload) {
        $messageText = [string](Get-ObjectProperty $jsonObject "message")
        $stopReasonAlias = [string](Get-ObjectProperty $jsonObject "stop_reason_alias")
        if ($stopReasonAlias -eq "provider_error" -or $messageText.ToLowerInvariant().Contains("provider error")) {
            throw "[provider-error] $messageText"
        }
        throw "[review-payload] missing review object in JSON output"
    }
    Write-Host "[ok] parse json"

    Write-Host "[3/3] validate review payload"
    $effectiveSchemaRetries = [Math]::Max(0, $SchemaRetries)
    $schemaAttempt = 0
    $lastSchemaError = $null
    $metrics = $null
    while ($schemaAttempt -le $effectiveSchemaRetries) {
        try {
            $metrics = Validate-ReviewPayload $reviewPayload
            break
        }
        catch {
            $lastSchemaError = $_.Exception.Message
            if ($schemaAttempt -ge $effectiveSchemaRetries) {
                break
            }
            $nextAttempt = $schemaAttempt + 2
            $maxAttempts = $effectiveSchemaRetries + 1
            Write-Host ("WARN schema validation attempt {0}/{1} failed: {2}" -f ($schemaAttempt + 1), $maxAttempts, $lastSchemaError)
            Write-Host ("[retry] schema validation by rerunning prompt attempt {0}/{1}" -f $nextAttempt, $maxAttempts)
            $output = Invoke-WithRetry "prompt-review-json-schema-retry" {
                $prev = $ErrorActionPreference
                $ErrorActionPreference = "Continue"
                try {
                    $result = & $AsiExe prompt $promptText --provider $providerName --model $resolvedModel --project $Project --output-format json --prompt-auto-tools $PromptAutoTools 2>&1
                    $exitCode = $LASTEXITCODE
                    $text = ($result | Out-String -Width 4096)
                    if ($exitCode -ne 0) {
                        throw "prompt command failed (exit=$exitCode):`n$text"
                    }
                    return $text
                }
                finally {
                    $ErrorActionPreference = $prev
                }
            } -MaxAttempts $effectiveRetries -DelayMs 1400
            $jsonObject = Parse-JsonOutput $output
            $providerCategory = Get-ProviderErrorCategoryFromJson $jsonObject
            if ($null -ne $providerCategory) {
                if ($schemaAttempt -ge $effectiveSchemaRetries -or -not (Should-RetryOnProviderError $providerCategory)) {
                    throw ("[provider-error] " + $providerCategory)
                }
                Write-Host ("WARN provider transient schema-retry failure: {0}" -f $providerCategory)
                Start-Sleep -Milliseconds 1200
                continue
            }
            if ($UseEnvelope) {
                $reviewPayload = Get-ObjectProperty $jsonObject "review"
            } else {
                if ($PromptEnvelope -eq "on") {
                    $jsonOnlyStatus = [string](Get-ObjectProperty $jsonObject "status")
                    $jsonOnlySchemaVersion = [string](Get-ObjectProperty $jsonObject "schema_version")
                    Assert-True (-not [string]::IsNullOrWhiteSpace($jsonOnlyStatus)) "json_only.status" "field missing (PromptEnvelope=on)"
                    Assert-True ($jsonOnlyStatus -eq "ok") "json_only.status" ("expected ok, got: " + $jsonOnlyStatus)
                    Assert-True ($jsonOnlySchemaVersion -eq "1") "json_only.schema_version" ("unexpected value: " + $jsonOnlySchemaVersion)
                    $reviewPayload = Get-ObjectProperty $jsonObject "review"
                } else {
                    $jsonOnlyReviewTaskFlag = Get-ObjectProperty $jsonObject "is_review_task"
                    if ($null -ne $jsonOnlyReviewTaskFlag) {
                        $reviewPayload = $jsonObject
                    } else {
                        $reviewPayload = Get-ObjectProperty $jsonObject "review"
                    }
                }
            }
        }
        $schemaAttempt += 1
    }

    if ($null -eq $metrics) {
        if ($FailOnSchemaInvalid -eq "on") {
            if ([string]::IsNullOrWhiteSpace($lastSchemaError)) {
                $lastSchemaError = "[review.schema_valid] schema validation failed after retries"
            }
            throw $lastSchemaError
        }
        Write-Host ("WARN schema invalid but continuing (FailOnSchemaInvalid=off): {0}" -f $lastSchemaError)
        $metrics = [ordered]@{
            total_findings = 0
            missing_tests_count = 0
            findings_count = 0
            findings_sorted_count = 0
            top_risk_paths_count = 0
            open_questions_count = 0
            summary_count = 0
        }
    } else {
        Write-Host ("[ok] validate review payload total_findings={0} top_risk_paths={1}" -f $metrics.total_findings, $metrics.top_risk_paths_count)
    }

    $total = (Get-Date) - $started
    Write-Host ("smoke_review_json: PASS ({0:n1}s)" -f $total.TotalSeconds)
    Write-ReviewJsonReport -Status "pass" -DurationSecs $total.TotalSeconds -FailureCategory "" -FailureHint "" -FailureMessage "" -ProviderName $providerName -ModelName $resolvedModel -PromptMode $promptMode -Metrics $metrics
}
catch {
    $raw = $_.Exception.Message
    $providerPrefix = "[provider-error]"
    $category = if ($raw.StartsWith($providerPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        $msg = $raw.Substring($providerPrefix.Length).Trim()
        if ([string]::IsNullOrWhiteSpace($msg)) { "provider_error" } else { $msg }
    } else {
        Get-FailureCategory $raw
    }
    $hint = Get-FailureHint $category
    Write-Host ""
    Write-Host "smoke_review_json: FAIL"
    Write-Host ("failure_category={0}" -f $category)
    Write-Host ("hint={0}" -f $hint)
    $total = (Get-Date) - $started
    Write-ReviewJsonReport -Status "fail" -DurationSecs $total.TotalSeconds -FailureCategory $category -FailureHint $hint -FailureMessage $raw -ProviderName $providerName -ModelName $resolvedModel -PromptMode $promptMode -Metrics @{}
    throw
}
finally {
    if ($null -eq $oldOpenAiBase) { Remove-Item Env:OPENAI_BASE_URL -ErrorAction SilentlyContinue } else { $env:OPENAI_BASE_URL = $oldOpenAiBase }
    if ($null -eq $oldOpenAiKey) { Remove-Item Env:OPENAI_API_KEY -ErrorAction SilentlyContinue } else { $env:OPENAI_API_KEY = $oldOpenAiKey }
    if ($null -eq $oldDeepSeekBase) { Remove-Item Env:DEEPSEEK_BASE_URL -ErrorAction SilentlyContinue } else { $env:DEEPSEEK_BASE_URL = $oldDeepSeekBase }
    if ($null -eq $oldDeepSeekKey) { Remove-Item Env:DEEPSEEK_API_KEY -ErrorAction SilentlyContinue } else { $env:DEEPSEEK_API_KEY = $oldDeepSeekKey }
    if ($null -eq $oldClaudeBase) { Remove-Item Env:ANTHROPIC_BASE_URL -ErrorAction SilentlyContinue } else { $env:ANTHROPIC_BASE_URL = $oldClaudeBase }
    if ($null -eq $oldClaudeKey) { Remove-Item Env:ANTHROPIC_API_KEY -ErrorAction SilentlyContinue } else { $env:ANTHROPIC_API_KEY = $oldClaudeKey }
    if ($null -eq $oldPromptEnvelope) { Remove-Item Env:ASI_REVIEW_JSON_ONLY_PROMPT_ENVELOPE -ErrorAction SilentlyContinue } else { $env:ASI_REVIEW_JSON_ONLY_PROMPT_ENVELOPE = $oldPromptEnvelope }
}
