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
    [string]$PromptText = "Reply with exactly JSONL_SMOKE_OK",
    [int]$Retries = 2,
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

function Get-FailureCategory([string]$Message) {
    if ([string]::IsNullOrWhiteSpace($Message)) { return "unknown" }
    $m = $Message.ToLowerInvariant()
    if ($m.Contains("missing openai_api_key")) { return "config_missing_credentials" }
    if ($m.Contains("missing deepseek_api_key")) { return "config_missing_credentials" }
    if ($m.Contains("missing anthropic_api_key")) { return "config_missing_credentials" }
    if ($m.Contains("401") -or $m.Contains("403") -or $m.Contains("unauthorized") -or $m.Contains("invalid api key") -or $m.Contains("invalid token")) { return "auth_error" }
    if ($m.Contains("quota") -or $m.Contains("insufficient_quota") -or $m.Contains("billing")) { return "quota_error" }
    if ($m.Contains("model not exist") -or $m.Contains("model_not_found") -or $m.Contains("unknown model") -or $m.Contains("invalid model")) { return "model_error" }
    if ($m.Contains("timed out") -or $m.Contains("timeout") -or $m.Contains("connection refused") -or $m.Contains("connection reset") -or $m.Contains("name or service not known") -or $m.Contains("no such host") -or $m.Contains("dns") -or $m.Contains("error sending request for url")) { return "network_error" }
    if ($m.Contains("provider_error")) { return "provider_error" }
    if ($m.Contains("[jsonl.")) { return "jsonl_validation_error" }
    if ($m.Contains("prompt command failed")) { return "prompt_command_failure" }
    return "output_validation_failure"
}

function Get-FailureHint([string]$Category) {
    switch ($Category) {
        "config_missing_credentials" { return "Provide provider credentials via -ApiKey/-BaseUrl or provider env vars." }
        "auth_error" { return "Verify API key validity and provider account permissions." }
        "quota_error" { return "Check account quota/billing before retry." }
        "model_error" { return "Verify model name and model access for this key." }
        "network_error" { return "Check outbound connectivity, DNS, proxy, and timeout settings." }
        "provider_error" { return "Check provider endpoint reachability and inspect runtime provider error details." }
        "jsonl_validation_error" { return "Ensure --output-format jsonl emits required schema_version=1 events." }
        default { return "Inspect full output and stack message to identify root cause." }
    }
}

function Should-Retry([string]$Message) {
    $cat = Get-FailureCategory $Message
    return $cat -eq "network_error" -or $cat -eq "provider_error"
}

function Parse-JsonLines([string]$OutputText) {
    $rows = @()
    foreach ($line in ($OutputText -split "`r?`n")) {
        $t = $line.Trim()
        if ([string]::IsNullOrWhiteSpace($t)) { continue }
        if (-not $t.StartsWith("{")) { continue }
        if (-not $t.EndsWith("}")) { continue }
        try {
            $rows += ,($t | ConvertFrom-Json)
        } catch {
            continue
        }
    }
    return $rows
}

function Is-NullValue([object]$Value) {
    return [System.Object]::ReferenceEquals($Value, $null)
}

function Write-JsonlReport(
    [string]$Status,
    [double]$DurationSecs,
    [string]$FailureCategory,
    [string]$FailureHint,
    [string]$FailureMessage,
    [string]$ProviderName,
    [string]$ModelName,
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
        script = "smoke_prompt_jsonl"
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
            retries = $Retries
            prompt_text = $PromptText
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

if (-not $DryRun -and [string]::IsNullOrWhiteSpace($resolvedApiKey)) {
    throw "missing $keyEnvName for prompt-jsonl smoke"
}

$oldOpenAiBase = $env:OPENAI_BASE_URL
$oldOpenAiKey = $env:OPENAI_API_KEY
$oldDeepSeekBase = $env:DEEPSEEK_BASE_URL
$oldDeepSeekKey = $env:DEEPSEEK_API_KEY
$oldClaudeBase = $env:ANTHROPIC_BASE_URL
$oldClaudeKey = $env:ANTHROPIC_API_KEY
$started = Get-Date

try {
    if ($providerName -eq "openai") {
        if (-not [string]::IsNullOrWhiteSpace($resolvedBaseUrl)) { $env:OPENAI_BASE_URL = $resolvedBaseUrl }
        if (-not [string]::IsNullOrWhiteSpace($resolvedApiKey)) { $env:OPENAI_API_KEY = $resolvedApiKey }
    } elseif ($providerName -eq "deepseek") {
        if (-not [string]::IsNullOrWhiteSpace($resolvedBaseUrl)) { $env:DEEPSEEK_BASE_URL = $resolvedBaseUrl }
        if (-not [string]::IsNullOrWhiteSpace($resolvedApiKey)) { $env:DEEPSEEK_API_KEY = $resolvedApiKey }
    } else {
        if (-not [string]::IsNullOrWhiteSpace($resolvedBaseUrl)) { $env:ANTHROPIC_BASE_URL = $resolvedBaseUrl }
        if (-not [string]::IsNullOrWhiteSpace($resolvedApiKey)) { $env:ANTHROPIC_API_KEY = $resolvedApiKey }
    }

    Write-Host "Using binary: $AsiExe"
    Write-Host "Provider: $providerName"
    Write-Host "Project: $Project"
    Write-Host "Model: $resolvedModel"
    Write-Host "PromptText: $PromptText"
    Write-Host "Retries: $Retries"

    if ($DryRun) {
        Write-Host ""
        Write-Host "DryRun command:"
        Write-Host "& `"$AsiExe`" prompt `"$PromptText`" --provider $providerName --model `"$resolvedModel`" --project `"$Project`" --output-format jsonl --prompt-auto-tools off"
        Write-Host "smoke_prompt_jsonl: DRY_RUN"
        exit 0
    }

    $effectiveRetries = [Math]::Max(1, $Retries)
    $output = $null
    for ($attempt = 1; $attempt -le $effectiveRetries; $attempt++) {
        $prev = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $result = & $AsiExe prompt $PromptText --provider $providerName --model $resolvedModel --project $Project --output-format jsonl --prompt-auto-tools off 2>&1
            $exitCode = $LASTEXITCODE
            $output = ($result | Out-String -Width 4096)
            if ($exitCode -eq 0) {
                break
            }
            $msg = "prompt command failed (exit=$exitCode):`n$output"
            if ($attempt -lt $effectiveRetries -and (Should-Retry $msg)) {
                Write-Host ("WARN retrying prompt-jsonl attempt {0}/{1}: {2}" -f $attempt, $effectiveRetries, (Get-FailureCategory $msg))
                Start-Sleep -Milliseconds 1200
                continue
            }
            throw $msg
        }
        finally {
            $ErrorActionPreference = $prev
        }
    }

    Assert-True (-not [string]::IsNullOrWhiteSpace($output)) "jsonl.output" "empty output"

    $rows = Parse-JsonLines $output
    Assert-True (@($rows).Count -gt 0) "jsonl.lines" "no JSONL lines found"

    $events = @{}
    foreach ($row in $rows) {
        $schema = [string](Get-ObjectProperty $row "schema_version")
        Assert-True ($schema -eq "1") "jsonl.schema_version" ("unexpected value: " + $schema)
        $event = [string](Get-ObjectProperty $row "event")
        Assert-True (-not [string]::IsNullOrWhiteSpace($event)) "jsonl.event" "missing event"
        $data = Get-ObjectProperty $row "data"
        Assert-True (-not (Is-NullValue $data)) "jsonl.data" ("missing data for event=" + $event)
        $events[$event] = 1
    }

    $requiredEvents = @(
        "prompt.result",
        "prompt.changed_files",
        "prompt.native_tool_calls",
        "prompt.auto_validation",
        "prompt.review",
        "prompt.runtime"
    )
    foreach ($required in $requiredEvents) {
        Assert-True ($events.ContainsKey($required)) "jsonl.required_event" ("missing event=" + $required)
    }

    $resultRow = $rows | Where-Object { [string](Get-ObjectProperty $_ "event") -eq "prompt.result" } | Select-Object -First 1
    Assert-True ($null -ne $resultRow) "jsonl.prompt_result" "missing prompt.result row"
    $resultData = Get-ObjectProperty $resultRow "data"
    $stopReasonAlias = [string](Get-ObjectProperty $resultData "stop_reason_alias")
    Assert-True (-not [string]::IsNullOrWhiteSpace($stopReasonAlias)) "jsonl.stop_reason_alias" "missing in prompt.result.data"

    $nativeToolsRow = $rows | Where-Object { [string](Get-ObjectProperty $_ "event") -eq "prompt.native_tool_calls" } | Select-Object -First 1
    Assert-True ($null -ne $nativeToolsRow) "jsonl.prompt_native_tool_calls" "missing prompt.native_tool_calls row"
    $nativeToolsData = Get-ObjectProperty $nativeToolsRow "data"
    $nativeToolsCount = Get-ObjectProperty $nativeToolsData "count"
    Assert-True ($null -ne $nativeToolsCount) "jsonl.native_tool_calls.count" "missing count in prompt.native_tool_calls.data"
    $nativeToolsItemsValue = Get-ObjectProperty $nativeToolsData "items"
    Assert-True (($nativeToolsItemsValue -is [System.Array])) "jsonl.native_tool_calls.items" "items must be an array"
    $nativeToolsItems = @($nativeToolsItemsValue)
    if (@($nativeToolsItems).Count -eq 1 -and $nativeToolsItems[0] -is [System.Array]) {
        $nativeToolsItems = @($nativeToolsItems[0])
    }
    $nativeToolsCountNumber = 0
    try {
        $nativeToolsCountNumber = [int]$nativeToolsCount
    } catch {
        throw "[jsonl.native_tool_calls.count] count must be an integer-compatible value"
    }
    Assert-True ($nativeToolsCountNumber -eq @($nativeToolsItems).Count) "jsonl.native_tool_calls.count_match" ("count mismatch: count=" + $nativeToolsCountNumber + " items=" + @($nativeToolsItems).Count)
    if (@($nativeToolsItems).Count -gt 0) {
        $firstTool = $nativeToolsItems[0]
        Assert-True ($null -ne (Get-ObjectProperty $firstTool "tool_call_id")) "jsonl.native_tool_calls.tool_call_id" "missing tool_call_id"
        Assert-True ($null -ne (Get-ObjectProperty $firstTool "tool_name")) "jsonl.native_tool_calls.tool_name" "missing tool_name"
        Assert-True ($null -ne (Get-ObjectProperty $firstTool "ok")) "jsonl.native_tool_calls.ok" "missing ok"
    }

    $metrics = [ordered]@{
        row_count = @($rows).Count
        required_events = $requiredEvents
        observed_events = @($events.Keys | Sort-Object)
        stop_reason_alias = $stopReasonAlias
    }

    $total = (Get-Date) - $started
    Write-Host ("smoke_prompt_jsonl: PASS ({0:n1}s)" -f $total.TotalSeconds)
    Write-JsonlReport -Status "pass" -DurationSecs $total.TotalSeconds -FailureCategory "" -FailureHint "" -FailureMessage "" -ProviderName $providerName -ModelName $resolvedModel -Metrics $metrics
}
catch {
    $raw = $_.Exception.Message
    $category = Get-FailureCategory $raw
    $hint = Get-FailureHint $category
    Write-Host ""
    Write-Host "smoke_prompt_jsonl: FAIL"
    Write-Host ("failure_category={0}" -f $category)
    Write-Host ("hint={0}" -f $hint)
    $total = (Get-Date) - $started
    Write-JsonlReport -Status "fail" -DurationSecs $total.TotalSeconds -FailureCategory $category -FailureHint $hint -FailureMessage $raw -ProviderName $providerName -ModelName $resolvedModel -Metrics @{}
    throw
}
finally {
    if ($null -eq $oldOpenAiBase) { Remove-Item Env:OPENAI_BASE_URL -ErrorAction SilentlyContinue } else { $env:OPENAI_BASE_URL = $oldOpenAiBase }
    if ($null -eq $oldOpenAiKey) { Remove-Item Env:OPENAI_API_KEY -ErrorAction SilentlyContinue } else { $env:OPENAI_API_KEY = $oldOpenAiKey }
    if ($null -eq $oldDeepSeekBase) { Remove-Item Env:DEEPSEEK_BASE_URL -ErrorAction SilentlyContinue } else { $env:DEEPSEEK_BASE_URL = $oldDeepSeekBase }
    if ($null -eq $oldDeepSeekKey) { Remove-Item Env:DEEPSEEK_API_KEY -ErrorAction SilentlyContinue } else { $env:DEEPSEEK_API_KEY = $oldDeepSeekKey }
    if ($null -eq $oldClaudeBase) { Remove-Item Env:ANTHROPIC_BASE_URL -ErrorAction SilentlyContinue } else { $env:ANTHROPIC_BASE_URL = $oldClaudeBase }
    if ($null -eq $oldClaudeKey) { Remove-Item Env:ANTHROPIC_API_KEY -ErrorAction SilentlyContinue } else { $env:ANTHROPIC_API_KEY = $oldClaudeKey }
}
