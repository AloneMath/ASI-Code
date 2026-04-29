param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\test-cli",
    [string]$Provider = "deepseek",
    [string]$Model = "deepseek-v4-pro",
    [int]$TimeoutSecs = 45,
    [switch]$AllowWaitError,
    [ValidateSet("json", "jsonl")]
    [string]$OutputMode = "json",
    [string]$ReportJsonPath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$startedAt = Get-Date
$waitFailurePayload = $null

function Clip-Text([string]$Value, [int]$MaxLen = 240) {
    if ([string]::IsNullOrWhiteSpace($Value)) { return "" }
    if ($Value.Length -le $MaxLen) { return $Value }
    return $Value.Substring(0, $MaxLen)
}

function Get-ProviderErrorCategoryFromText([string]$Message) {
    if ([string]::IsNullOrWhiteSpace($Message)) { return "unknown" }
    $m = $Message.ToLowerInvariant()
    if ($m.Contains("[provider-error] network_error")) { return "network_error" }
    if ($m.Contains("[provider-error] auth_error")) { return "auth_error" }
    if ($m.Contains("[provider-error] quota_error")) { return "quota_error" }
    if ($m.Contains("[provider-error] model_error")) { return "model_error" }
    if ($m.Contains("[provider-error] provider_error")) { return "provider_error" }
    if (
        $m.Contains("error sending request for url") -or
        $m.Contains("timed out") -or
        $m.Contains("timeout") -or
        $m.Contains("connection reset") -or
        $m.Contains("connection refused") -or
        $m.Contains("temporary failure in name resolution") -or
        $m.Contains("name or service not known") -or
        $m.Contains("dns") -or
        $m.Contains("tls")
    ) { return "network_error" }
    if (
        $m.Contains("401") -or
        $m.Contains("403") -or
        $m.Contains("unauthorized") -or
        $m.Contains("authentication") -or
        $m.Contains("invalid api key")
    ) { return "auth_error" }
    if (
        $m.Contains("429") -or
        $m.Contains("rate limit") -or
        $m.Contains("quota")
    ) { return "quota_error" }
    if (
        $m.Contains("model not found") -or
        $m.Contains("model does not exist") -or
        $m.Contains("unsupported model") -or
        $m.Contains("invalid model")
    ) { return "model_error" }
    if (
        $m.Contains("service unavailable") -or
        $m.Contains("internal server error") -or
        $m.Contains("bad gateway") -or
        $m.Contains("provider error")
    ) { return "provider_error" }
    return "unknown"
}

function Get-SubagentFailureHint([string]$Category) {
    switch ($Category) {
        "network_error" { return "Check outbound connectivity, DNS, proxy, and timeout settings." }
        "auth_error" { return "Verify API key validity and provider account permissions." }
        "quota_error" { return "Check account quota/billing before retry." }
        "model_error" { return "Verify model name and model access for this key." }
        "provider_error" { return "Check provider endpoint reachability and inspect runtime provider error details." }
        "subagent_command_missing" { return "Verify /agent list/log/retry/cancel parser and machine-output payload paths." }
        "subagent_wait_not_successful" { return "Inspect /agent wait payload; require status=done and ok=true for pass." }
        default { return "Inspect /agent wait payload and provider runtime output." }
    }
}

function Write-Report(
    [string]$Status,
    [string]$FailureCategory = "",
    [string]$Hint = "",
    [string]$Message = "",
    [double]$DurationSecs = 0.0,
    [hashtable]$Extra = @{}
) {
    if ([string]::IsNullOrWhiteSpace($ReportJsonPath)) { return }
    $dir = Split-Path -Parent $ReportJsonPath
    if (-not [string]::IsNullOrWhiteSpace($dir) -and -not (Test-Path -LiteralPath $dir)) {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
    }
    $payload = [ordered]@{
        script = "smoke_subagent"
        timestamp_utc = (Get-Date).ToUniversalTime().ToString("o")
        status = $Status
        duration_secs = [Math]::Round($DurationSecs, 3)
        failure_category = if ($Status -eq "fail") { $FailureCategory } else { $null }
        hint = if ($Status -eq "fail") { $Hint } else { $null }
        message = if ($Status -eq "fail") { $Message } else { $null }
        config = [ordered]@{
            asi_exe = $AsiExe
            project = $Project
            provider = $Provider
            model = $Model
            timeout_secs = $TimeoutSecs
            allow_wait_error = [bool]$AllowWaitError
            output_mode = $OutputMode
        }
        metrics = $Extra
    }
    $payload | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ReportJsonPath -Encoding UTF8
}

if (-not (Test-Path -LiteralPath $AsiExe)) {
    $msg = "asi binary not found: $AsiExe"
    Write-Error $msg
    $elapsed = (Get-Date) - $startedAt
    Write-Report -Status "fail" -FailureCategory "binary_missing" -Hint "Build release binary first: cargo build --release." -Message $msg -DurationSecs $elapsed.TotalSeconds
    exit 1
}

$prompt = @"
/agent spawn --$OutputMode --background inspect README.md
/agent list --$OutputMode
/agent log --$OutputMode sa-1 --tail 5
/agent cancel --$OutputMode sa-1
/agent retry --$OutputMode sa-1
/agent log --$OutputMode sa-1 --tail 8
/agent wait --$OutputMode $TimeoutSecs
/exit
"@

try {
    $output = $prompt | & $AsiExe repl --provider $Provider --model $Model --project $Project --no-setup 2>&1
    $text = ($output | Out-String)
    $ansiPattern = '\x1B\[[0-9;?]*[ -/]*[@-~]'
    $cleanText = [System.Text.RegularExpressions.Regex]::Replace($text, $ansiPattern, "")
    $jsonLines = $cleanText -split "`r?`n" | ForEach-Object {
        $line = $_.Trim()
        if ($line.StartsWith("{") -and $line.EndsWith("}")) {
            $line
        } else {
            $idx = $line.IndexOf("{")
            if ($idx -ge 0) {
                $candidate = $line.Substring($idx).Trim()
                if ($candidate.StartsWith("{") -and $candidate.EndsWith("}")) { $candidate } else { $null }
            } else {
                $null
            }
        }
    } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    $parsed = @()
    foreach ($line in $jsonLines) {
        try {
            $obj = $line | ConvertFrom-Json -ErrorAction Stop
            if ($OutputMode -eq "json") {
                if ($null -ne $obj.schema_version -and $null -ne $obj.command -and $null -ne $obj.agent) {
                    $parsed += $obj
                }
            } else {
                $evt = ""
                if ($null -ne $obj.event) { $evt = [string]$obj.event }
                $cmd = ""
                $agent = $null
                if ($null -ne $obj.data) {
                    if ($null -ne $obj.data.command) { $cmd = [string]$obj.data.command }
                    $agent = $obj.data.agent
                }
                if ($null -ne $obj.schema_version -and -not [string]::IsNullOrWhiteSpace($evt) -and -not [string]::IsNullOrWhiteSpace($cmd) -and $null -ne $agent) {
                    $parsed += [pscustomobject]@{
                        schema_version = $obj.schema_version
                        event = $evt
                        command = $cmd
                        agent = $agent
                    }
                }
            }
        } catch {
        }
    }

    if ($parsed.Count -lt 4) {
        $msg = "expected at least four /agent machine responses, got $($parsed.Count)"
        Write-Error $msg
        $elapsed = (Get-Date) - $startedAt
        Write-Report -Status "fail" -FailureCategory "subagent_json_missing" -Hint "Verify /agent --json/--jsonl output and provider connectivity." -Message $msg -DurationSecs $elapsed.TotalSeconds
        exit 1
    }

    $commands = @($parsed | ForEach-Object { [string]$_.command })
    $requiredCommands = @("spawn", "list", "log", "cancel", "retry", "wait")
    $missingCommands = @($requiredCommands | Where-Object { -not ($commands -contains $_) })
    if ($missingCommands.Count -gt 0) {
        $msg = "missing required /agent responses: $($missingCommands -join ',')"
        Write-Error $msg
        $elapsed = (Get-Date) - $startedAt
        Write-Report -Status "fail" -FailureCategory "subagent_command_missing" -Hint (Get-SubagentFailureHint "subagent_command_missing") -Message $msg -DurationSecs $elapsed.TotalSeconds -Extra @{
            json_count = $parsed.Count
            commands = $commands
            required_commands = $requiredCommands
            missing_commands = $missingCommands
        }
        exit 1
    }

    if (-not ($commands -contains "spawn")) {
        $msg = "missing spawn response in /agent machine output"
        Write-Error $msg
        $elapsed = (Get-Date) - $startedAt
        Write-Report -Status "fail" -FailureCategory "subagent_spawn_missing" -Hint "Check /agent spawn parser and json envelope." -Message $msg -DurationSecs $elapsed.TotalSeconds
        exit 1
    }
    if (-not ($commands -contains "wait")) {
        $msg = "missing wait response in /agent machine output"
        Write-Error $msg
        $elapsed = (Get-Date) - $startedAt
        Write-Report -Status "fail" -FailureCategory "subagent_wait_missing" -Hint "Check /agent wait parser and async worker completion path." -Message $msg -DurationSecs $elapsed.TotalSeconds
        exit 1
    }

    $wait = $parsed | Where-Object { $_.command -eq "wait" } | Select-Object -Last 1
    $waitStatus = ""
    $waitOk = $false
    $waitResultPreview = ""
    $waitAgent = $null
    if ($null -ne $wait) {
        $waitAgent = $wait.agent
    }
    if ($null -ne $waitAgent) {
        $statusProp = $waitAgent.PSObject.Properties["status"]
        if ($null -ne $statusProp -and $null -ne $statusProp.Value) {
            $waitStatus = [string]$statusProp.Value
        }
        $okProp = $waitAgent.PSObject.Properties["ok"]
        if ($null -ne $okProp -and $null -ne $okProp.Value) {
            $waitOk = [bool]$okProp.Value
        }
        $resultProp = $waitAgent.PSObject.Properties["result"]
        if ($null -ne $resultProp -and $null -ne $resultProp.Value) {
            $waitResultPreview = Clip-Text ([string]$resultProp.Value) 240
        }
    }

    if (-not ($waitStatus -eq "done" -and $waitOk)) {
        $msg = "wait status is not successful (status=$waitStatus ok=$waitOk)"
        $providerCategory = Get-ProviderErrorCategoryFromText ($msg + " | " + $waitResultPreview)
        $failureCategory = if ($providerCategory -ne "unknown") { $providerCategory } else { "subagent_wait_not_successful" }
        $failureHint = Get-SubagentFailureHint $failureCategory
        $failureMessage = if ($providerCategory -ne "unknown") {
            "[provider-error] $providerCategory :: $msg"
        } else {
            $msg
        }
        $waitFailurePayload = @{
            json_count = $parsed.Count
            commands = $commands
            required_commands = $requiredCommands
            missing_commands = $missingCommands
            wait_status = $waitStatus
            wait_ok = $waitOk
            wait_result_preview = $waitResultPreview
            failure_category = $failureCategory
            failure_hint = $failureHint
            failure_message = $failureMessage
        }
        if ($AllowWaitError) {
            Write-Warning ($failureMessage + " [downgraded by -AllowWaitError]")
            Write-Host "smoke_subagent: PASS (wait warning)"
            Write-Host "commands=$($commands -join ',') wait_status=$waitStatus wait_ok=$waitOk warning=true"
            $elapsed = (Get-Date) - $startedAt
            Write-Report -Status "pass" -Message $failureMessage -DurationSecs $elapsed.TotalSeconds -Extra @{
                json_count = $waitFailurePayload.json_count
                commands = $waitFailurePayload.commands
                required_commands = $waitFailurePayload.required_commands
                missing_commands = $waitFailurePayload.missing_commands
                wait_status = $waitFailurePayload.wait_status
                wait_ok = $waitFailurePayload.wait_ok
                wait_warning = $true
                wait_failure_category = $waitFailurePayload.failure_category
                wait_result_preview = $waitFailurePayload.wait_result_preview
            }
            exit 0
        }
        $elapsed = (Get-Date) - $startedAt
        Write-Report -Status "fail" -FailureCategory $waitFailurePayload.failure_category -Hint $waitFailurePayload.failure_hint -Message $waitFailurePayload.failure_message -DurationSecs $elapsed.TotalSeconds -Extra @{
            json_count = $waitFailurePayload.json_count
            commands = $waitFailurePayload.commands
            required_commands = $waitFailurePayload.required_commands
            missing_commands = $waitFailurePayload.missing_commands
            wait_status = $waitFailurePayload.wait_status
            wait_ok = $waitFailurePayload.wait_ok
            wait_failure_category = $waitFailurePayload.failure_category
            wait_result_preview = $waitFailurePayload.wait_result_preview
        }
        Write-Error $waitFailurePayload.failure_message
        exit 1
    }

    Write-Host "smoke_subagent: PASS"
    Write-Host "commands=$($commands -join ',') wait_status=$waitStatus wait_ok=$waitOk"
    $elapsed = (Get-Date) - $startedAt
    Write-Report -Status "pass" -Extra @{
        json_count = $parsed.Count
        commands = $commands
        required_commands = $requiredCommands
        missing_commands = $missingCommands
        wait_status = $waitStatus
        wait_ok = $waitOk
    } -DurationSecs $elapsed.TotalSeconds
    exit 0
} catch {
    $msg = $_.Exception.Message
    $elapsed = (Get-Date) - $startedAt
    if ($null -ne $waitFailurePayload) {
        Write-Report -Status "fail" -FailureCategory $waitFailurePayload.failure_category -Hint $waitFailurePayload.failure_hint -Message $waitFailurePayload.failure_message -DurationSecs $elapsed.TotalSeconds -Extra @{
            json_count = $waitFailurePayload.json_count
            commands = $waitFailurePayload.commands
            required_commands = $waitFailurePayload.required_commands
            missing_commands = $waitFailurePayload.missing_commands
            wait_status = $waitFailurePayload.wait_status
            wait_ok = $waitFailurePayload.wait_ok
            wait_failure_category = $waitFailurePayload.failure_category
            wait_result_preview = $waitFailurePayload.wait_result_preview
        }
    } else {
        Write-Report -Status "fail" -FailureCategory "subagent_smoke_failed" -Hint "Inspect REPL output and provider auth/model settings." -Message $msg -DurationSecs $elapsed.TotalSeconds
    }
    Write-Error $msg
    exit 1
}
