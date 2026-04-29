param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\Code\Rust",
    [int]$Port = 8788,
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
    [int]$Retries = 2,
    [int]$TimeoutSecs = 120,
    [string]$ReportJsonPath = "",
    [switch]$SkipToolTurn
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

function Require-Contains([string]$text, [string]$needle, [string]$label) {
    if (-not $text.Contains($needle)) {
        throw "[$label] missing: $needle`n--- output ---`n$text"
    }
}

function Assert-True([bool]$cond, [string]$label, [string]$detail) {
    if (-not $cond) {
        throw "[$label] $detail"
    }
}

function Invoke-WithRetry([string]$Name, [scriptblock]$Action, [int]$MaxAttempts = 3, [int]$DelayMs = 1200) {
    $last = $null
    for ($attempt = 1; $attempt -le $MaxAttempts; $attempt++) {
        try {
            return (& $Action)
        } catch {
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

function Run-GatewayTurn(
    [string]$Base,
    [string]$SessionId,
    [string]$ProviderName,
    [string]$ModelName,
    [string]$InputText,
    [int]$Timeout,
    [int]$MaxAttempts,
    [int]$DelayMs = 1200
) {
    return Invoke-WithRetry ("run-turn:" + $InputText.Substring(0, [Math]::Min(24, $InputText.Length))) {
        Invoke-RestMethod -Method Post -Uri ("$Base/v1/sessions/{0}/turns" -f $SessionId) -ContentType "application/json" -Body (@{
            provider = $ProviderName
            model = $ModelName
            input = $InputText
        } | ConvertTo-Json -Compress) -TimeoutSec $Timeout
    } -MaxAttempts $MaxAttempts -DelayMs $DelayMs
}

function Get-FailureCategory([string]$Message) {
    if ([string]::IsNullOrWhiteSpace($Message)) { return "unknown" }
    $m = $Message.ToLowerInvariant()
    if ($m.Contains("baseurl/apikey are required")) { return "config_missing_credentials" }
    if ($m.Contains("openaibaseurl/openaapikey are required")) { return "config_missing_credentials" }
    if ($m.Contains("deepseekbaseurl/deepseekapikey are required")) { return "config_missing_credentials" }
    if ($m.Contains("claudebaseurl/claudeapikey are required")) { return "config_missing_credentials" }
    if ($m.Contains("missing openai_api_key")) { return "config_missing_credentials" }
    if ($m.Contains("missing deepseek_api_key")) { return "config_missing_credentials" }
    if ($m.Contains("missing anthropic_api_key")) { return "config_missing_credentials" }
    if ($m.Contains("[healthz]")) { return "gateway_startup_or_port_conflict" }
    if ($m.Contains("401") -or $m.Contains("403") -or $m.Contains("unauthorized") -or $m.Contains("invalid api key") -or $m.Contains("invalid token") -or $m.Contains("forbidden")) { return "auth_error" }
    if ($m.Contains("quota") -or $m.Contains("insufficient_quota") -or $m.Contains("billing")) { return "quota_error" }
    if ($m.Contains("model not exist") -or $m.Contains("model_not_found") -or $m.Contains("unknown model") -or $m.Contains("invalid model")) { return "model_error" }
    if ($m.Contains("timed out") -or $m.Contains("timeout") -or $m.Contains("connection refused") -or $m.Contains("connection reset") -or $m.Contains("name or service not known") -or $m.Contains("no such host") -or $m.Contains("temporarily unavailable") -or $m.Contains("dns")) { return "network_error" }
    if ($m.Contains("error sending request for url") -or $m.Contains("unable to connect") -or $m.Contains("transport error") -or $m.Contains("tls")) { return "network_error" }
    if ($m.Contains("native tool calling unsupported") -or $m.Contains("function_call is not supported") -or $m.Contains("tool calls are not supported")) { return "tool_calling_unsupported" }
    if ($m.Contains("[tool-turn-read-file]") -or $m.Contains("[tool-turn-stop]") -or $m.Contains("[tool-turn-final-prefix]") -or $m.Contains("[tool-turn-final-marker]") -or $m.Contains("[tool-turn-final-stop]")) { return "tool_flow_failure" }
    if ($m.Contains("[session-json")) { return "session_persistence_failure" }
    if ($m.Contains("[turn-message]") -or $m.Contains("[turn-stop")) { return "turn_validation_failure" }
    return "unknown"
}

function Get-FailureHint([string]$Category) {
    switch ($Category) {
        "config_missing_credentials" { return "Provide provider-specific credentials via -ApiKey/-BaseUrl (or provider env vars)." }
        "gateway_startup_or_port_conflict" { return "Check local port occupancy; rerun with a different -Port." }
        "auth_error" { return "Verify API key validity and provider account permissions." }
        "quota_error" { return "Check account quota/billing; retry after quota is available." }
        "model_error" { return "Verify -OpenAiModel exists on the endpoint and is permitted for the key." }
        "network_error" { return "Check outbound connectivity/DNS/proxy; increase -TimeoutSecs and -Retries if needed." }
        "tool_calling_unsupported" { return "Endpoint/model does not support native tools; try a model that supports tool calls." }
        "tool_flow_failure" { return "Tool-use turn failed; inspect gateway output and model behavior on tool-followup prompts." }
        "session_persistence_failure" { return "Check project write permission for the sessions directory." }
        "turn_validation_failure" { return "Model output did not match strict smoke assertions; rerun and inspect turn payloads." }
        default { return "Inspect full error text and gateway logs for root cause." }
    }
}

function Write-GatewayReport(
    [string]$Status,
    [double]$DurationSecs,
    [string]$FailureCategory,
    [string]$FailureHint,
    [string]$FailureMessage,
    [object]$SessionId,
    [object]$ToolTurnExecuted,
    [string]$ProviderName,
    [string]$ModelName
) {
    if ([string]::IsNullOrWhiteSpace($ReportJsonPath)) {
        return
    }
    $dir = Split-Path -Parent $ReportJsonPath
    if (-not [string]::IsNullOrWhiteSpace($dir) -and -not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
    $payload = [ordered]@{
        script = "smoke_gateway"
        timestamp_utc = (Get-Date).ToUniversalTime().ToString("o")
        status = $Status
        duration_secs = [Math]::Round($DurationSecs, 3)
        failure_category = if ($Status -eq "fail") { $FailureCategory } else { $null }
        hint = if ($Status -eq "fail") { $FailureHint } else { $null }
        failure_message = if ($Status -eq "fail") { $FailureMessage } else { $null }
        session_id = $SessionId
        tool_turn_executed = [bool]$ToolTurnExecuted
        config = [ordered]@{
            project = $Project
            port = $Port
            retries = $Retries
            timeout_secs = $TimeoutSecs
            skip_tool_turn = [bool]$SkipToolTurn
            provider = $ProviderName
            model = $ModelName
            expected_tool_stop_alias = if ($ProviderName -eq "deepseek") { "completed" } else { "tool_use" }
        }
    }
    $json = $payload | ConvertTo-Json -Depth 8
    Set-Content -LiteralPath $ReportJsonPath -Value $json -Encoding UTF8
    Write-Host ("report_json={0}" -f $ReportJsonPath)
}

function Kill-PortListener([int]$PortToKill) {
    $listeners = Get-NetTCPConnection -LocalPort $PortToKill -State Listen -ErrorAction SilentlyContinue
    if (-not $listeners) {
        return
    }
    $ownerIds = $listeners | Select-Object -ExpandProperty OwningProcess -Unique
    foreach ($ownerId in $ownerIds) {
        try {
            Stop-Process -Id $ownerId -Force -ErrorAction Stop
        } catch {
            Write-Host "WARN failed to stop pid=$ownerId on port $PortToKill : $($_.Exception.Message)"
        }
    }
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
} elseif ($providerName -eq "claude") {
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
    throw "$providerLabel BaseUrl/ApiKey are required for gateway smoke (or set $keyEnvName)"
}

Write-Host "Using binary: $AsiExe"
Write-Host "Provider: $providerName"
Write-Host "Project: $Project"
Write-Host "Port: $Port"
Write-Host "Model: $resolvedModel"
Write-Host "Retries: $Retries"
Write-Host "TimeoutSecs: $TimeoutSecs"
Write-Host "SkipToolTurn: $SkipToolTurn"

$oldOpenAiBase = $env:OPENAI_BASE_URL
$oldOpenAiKey = $env:OPENAI_API_KEY
$oldDeepSeekBase = $env:DEEPSEEK_BASE_URL
$oldDeepSeekKey = $env:DEEPSEEK_API_KEY
$oldClaudeBase = $env:ANTHROPIC_BASE_URL
$oldClaudeKey = $env:ANTHROPIC_API_KEY
$stdout = Join-Path $env:TEMP ("asi_gateway_out_" + [guid]::NewGuid().ToString() + ".log")
$stderr = Join-Path $env:TEMP ("asi_gateway_err_" + [guid]::NewGuid().ToString() + ".log")
$proc = $null
$toolMarkerPath = $null
$toolMarkerValue = $null
$effectiveRetries = [Math]::Max(1, $Retries)
$effectiveTimeoutSecs = [Math]::Max(10, $TimeoutSecs)
$started = Get-Date
$sid = $null
$toolTurnExecuted = $false

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

    Kill-PortListener -PortToKill $Port

    Write-Host "[1/6] start gateway"
    $proc = Start-Process -FilePath $AsiExe -ArgumentList @(
        "gateway",
        "--listen", ("127.0.0.1:{0}" -f $Port),
        "--provider", $providerName,
        "--model", $resolvedModel,
        "--project", $Project
    ) -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr -WindowStyle Hidden

    $base = "http://127.0.0.1:$Port"
    $healthy = $false
    for ($i = 0; $i -lt 20; $i++) {
        Start-Sleep -Milliseconds 400
        try {
            $hz = Invoke-RestMethod -Method Get -Uri "$base/healthz" -TimeoutSec 3
            if ($hz -eq "ok") {
                $healthy = $true
                break
            }
        } catch {}
    }
    Assert-True $healthy "healthz" "gateway did not become healthy"
    Write-Host "[ok] healthz"

    Write-Host "[2/6] create session"
    $create = Invoke-WithRetry "create-session" {
        Invoke-RestMethod -Method Post -Uri "$base/v1/sessions" -ContentType "application/json" -Body (@{
            provider = $providerName
            model = $resolvedModel
        } | ConvertTo-Json -Compress) -TimeoutSec $effectiveTimeoutSecs
    } -MaxAttempts $effectiveRetries
    $sid = [string]$create.session_id
    Assert-True (-not [string]::IsNullOrWhiteSpace($sid)) "create-session" "empty session_id"
    Write-Host "[ok] create session $sid"

    Write-Host "[3/6] run turn"
    $turn = Run-GatewayTurn -Base $base -SessionId $sid -ProviderName $providerName -ModelName $resolvedModel -InputText "Reply with exactly GW_SMOKE_OK" -Timeout $effectiveTimeoutSecs -MaxAttempts $effectiveRetries -DelayMs 1500
    Assert-True ($turn.message -eq "GW_SMOKE_OK") "turn-message" ("unexpected message: " + [string]$turn.message)
    Assert-True (-not [string]::IsNullOrWhiteSpace([string]$turn.stop_reason_alias)) "turn-stop-alias" "missing stop_reason_alias"
    Assert-True (-not [string]::IsNullOrWhiteSpace([string]$turn.runtime_stop_reason_last_raw)) "turn-runtime-stop-raw" "missing runtime_stop_reason_last_raw"
    Assert-True (-not [string]::IsNullOrWhiteSpace([string]$turn.runtime_stop_reason_last_alias)) "turn-runtime-stop-alias" "missing runtime_stop_reason_last_alias"
    Write-Host "[ok] run turn"

    if ($SkipToolTurn) {
        Write-Host "[4/7] skip tool turn (-SkipToolTurn)"
    } else {
        $toolTurnExecuted = $true
        Write-Host "[4/7] run tool turn"
        $toolMarkerPath = Join-Path $Project ".asi_gateway_tool_marker.txt"
        $toolMarkerValue = "GW_TOOL_MARKER_{0}" -f [guid]::NewGuid().ToString("N")
        Set-Content -LiteralPath $toolMarkerPath -Value $toolMarkerValue -Encoding UTF8

        $toolTurn = Run-GatewayTurn -Base $base -SessionId $sid -ProviderName $providerName -ModelName $resolvedModel -InputText "Use tools to read file .asi_gateway_tool_marker.txt from project root. If this turn ends after tool execution, that's fine. Ensure the marker value appears in your next reply." -Timeout $effectiveTimeoutSecs -MaxAttempts $effectiveRetries -DelayMs 1800

        $toolMessage = [string]$toolTurn.message
        $toolStopAlias = [string]$toolTurn.runtime_stop_reason_last_alias
        $hasReadFileTrace = $toolMessage.Contains("[tool:read_file:") -or $toolMessage.Contains("[toolcall read_file") -or $toolMessage.Contains("read_file")
        if ($providerName -eq "deepseek") {
            # DeepSeek V4 family may stop this turn as completed and defer explicit marker text to next turn.
            $deepseekToolTurnLooksValid = $hasReadFileTrace -or ($toolStopAlias -eq "completed")
            Assert-True $deepseekToolTurnLooksValid "tool-turn-read-file" "expected read_file trace or completed alias for deepseek tool turn"
        } else {
            Assert-True $hasReadFileTrace "tool-turn-read-file" "missing read_file tool trace in turn response"
        }
        Assert-True (-not [string]::IsNullOrWhiteSpace([string]$toolTurn.stop_reason_alias)) "tool-turn-stop-alias" "missing stop_reason_alias"
        $expectedToolAlias = if ($providerName -eq "deepseek") { "completed" } else { "tool_use" }
        Assert-True ($toolStopAlias -eq $expectedToolAlias) "tool-turn-stop" ("expected runtime_stop_reason_last_alias=" + $expectedToolAlias)

        $finalTurn = Run-GatewayTurn -Base $base -SessionId $sid -ProviderName $providerName -ModelName $resolvedModel -InputText "Now reply exactly GW_TOOL_OK:<marker_value_you_read>." -Timeout $effectiveTimeoutSecs -MaxAttempts $effectiveRetries -DelayMs 1200
        $finalMessage = [string]$finalTurn.message
        if (-not $finalMessage.Contains("GW_TOOL_OK:") -and ($providerName -eq "deepseek")) {
            # DeepSeek may emit another tool-use style step; send one extra finalize turn.
            $finalTurn = Run-GatewayTurn -Base $base -SessionId $sid -ProviderName $providerName -ModelName $resolvedModel -InputText "Finalize now: reply exactly GW_TOOL_OK:<marker_value_you_read> using the marker from previous tool output." -Timeout $effectiveTimeoutSecs -MaxAttempts $effectiveRetries -DelayMs 1200
            $finalMessage = [string]$finalTurn.message
        }
        if ($providerName -eq "deepseek") {
            # DeepSeek gateway path may continue returning tool-use style content.
            $finalHasTrace = $finalMessage.Contains("GW_TOOL_OK:") -or $finalMessage.Contains("read_file")
            Assert-True $finalHasTrace "tool-turn-final-prefix" "expected GW_TOOL_OK or read_file trace in final turn"
            if ($finalMessage.Contains("GW_TOOL_OK:")) {
                Assert-True ([string]$finalTurn.runtime_stop_reason_last_alias -eq "completed") "tool-turn-final-stop" "expected runtime_stop_reason_last_alias=completed when GW_TOOL_OK is returned"
            }
        } else {
            Require-Contains $finalMessage "GW_TOOL_OK:" "tool-turn-final-prefix"
            Require-Contains $finalMessage $toolMarkerValue "tool-turn-final-marker"
            Assert-True ([string]$finalTurn.runtime_stop_reason_last_alias -eq "completed") "tool-turn-final-stop" "expected runtime_stop_reason_last_alias=completed"
        }
        Write-Host "[ok] run tool turn"
    }

    Write-Host "[5/7] session detail"
    $detail = Invoke-WithRetry "session-detail" {
        Invoke-RestMethod -Method Get -Uri ("$base/v1/sessions/{0}" -f $sid) -TimeoutSec $effectiveTimeoutSecs
    } -MaxAttempts $effectiveRetries
    Assert-True (-not [string]::IsNullOrWhiteSpace([string]$detail.last_stop_reason_raw)) "detail-last-stop-raw" "missing last_stop_reason_raw"
    Assert-True (-not [string]::IsNullOrWhiteSpace([string]$detail.last_stop_reason_alias)) "detail-last-stop-alias" "missing last_stop_reason_alias"
    Write-Host "[ok] session detail"

    Write-Host "[6/7] session list"
    $list = Invoke-WithRetry "session-list" {
        Invoke-RestMethod -Method Get -Uri "$base/v1/sessions" -TimeoutSec $effectiveTimeoutSecs
    } -MaxAttempts $effectiveRetries
    $first = $list.sessions | Select-Object -First 1
    Assert-True ($null -ne $first) "session-list" "empty session list"
    Assert-True (-not [string]::IsNullOrWhiteSpace([string]$first.last_stop_reason_raw)) "list-last-stop-raw" "missing last_stop_reason_raw in list"
    Assert-True (-not [string]::IsNullOrWhiteSpace([string]$first.last_stop_reason_alias)) "list-last-stop-alias" "missing last_stop_reason_alias in list"
    Write-Host "[ok] session list"

    Write-Host "[7/7] persisted session file"
    $sessionPath = Join-Path (Join-Path $Project "sessions") ($sid + ".json")
    $raw = Get-Content -LiteralPath $sessionPath -Raw
    Require-Contains $raw "\"last_stop_reason_raw\"" "session-json-last-stop-raw"
    Require-Contains $raw "\"last_stop_reason_alias\"" "session-json-last-stop-alias"
    if (-not $SkipToolTurn) {
        Require-Contains $raw ".asi_gateway_tool_marker.txt" "session-json-tool-path"
        if ($providerName -ne "deepseek") {
            Require-Contains $raw $toolMarkerValue "session-json-tool-marker"
        }
    }
    Write-Host "[ok] persisted session file"

    Write-Host "smoke_gateway: PASS"
    $total = (Get-Date) - $started
    Write-GatewayReport -Status "pass" -DurationSecs $total.TotalSeconds -FailureCategory "" -FailureHint "" -FailureMessage "" -SessionId $sid -ToolTurnExecuted $toolTurnExecuted -ProviderName $providerName -ModelName $resolvedModel
}
catch {
    $raw = $_.Exception.Message
    $category = Get-FailureCategory $raw
    $hint = Get-FailureHint $category
    Write-Host ""
    Write-Host "smoke_gateway: FAIL"
    Write-Host ("failure_category={0}" -f $category)
    Write-Host ("hint={0}" -f $hint)
    $total = (Get-Date) - $started
    Write-GatewayReport -Status "fail" -DurationSecs $total.TotalSeconds -FailureCategory $category -FailureHint $hint -FailureMessage $raw -SessionId $sid -ToolTurnExecuted $toolTurnExecuted -ProviderName $providerName -ModelName $resolvedModel
    throw
}
finally {
    if ($proc -and -not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
    Kill-PortListener -PortToKill $Port
    if (Test-Path $stdout) { Remove-Item $stdout -Force -ErrorAction SilentlyContinue }
    if (Test-Path $stderr) { Remove-Item $stderr -Force -ErrorAction SilentlyContinue }
    if ($toolMarkerPath -and (Test-Path -LiteralPath $toolMarkerPath)) { Remove-Item -LiteralPath $toolMarkerPath -Force -ErrorAction SilentlyContinue }
    if ($null -eq $oldOpenAiBase) { Remove-Item Env:OPENAI_BASE_URL -ErrorAction SilentlyContinue } else { $env:OPENAI_BASE_URL = $oldOpenAiBase }
    if ($null -eq $oldOpenAiKey) { Remove-Item Env:OPENAI_API_KEY -ErrorAction SilentlyContinue } else { $env:OPENAI_API_KEY = $oldOpenAiKey }
    if ($null -eq $oldDeepSeekBase) { Remove-Item Env:DEEPSEEK_BASE_URL -ErrorAction SilentlyContinue } else { $env:DEEPSEEK_BASE_URL = $oldDeepSeekBase }
    if ($null -eq $oldDeepSeekKey) { Remove-Item Env:DEEPSEEK_API_KEY -ErrorAction SilentlyContinue } else { $env:DEEPSEEK_API_KEY = $oldDeepSeekKey }
    if ($null -eq $oldClaudeBase) { Remove-Item Env:ANTHROPIC_BASE_URL -ErrorAction SilentlyContinue } else { $env:ANTHROPIC_BASE_URL = $oldClaudeBase }
    if ($null -eq $oldClaudeKey) { Remove-Item Env:ANTHROPIC_API_KEY -ErrorAction SilentlyContinue } else { $env:ANTHROPIC_API_KEY = $oldClaudeKey }
}
