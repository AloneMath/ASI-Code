<#
.SYNOPSIS
Runs machine-readable CI checks for ASI CLI and writes one JSON report.

.DESCRIPTION
Builds the release binary, runs release tests, runs unified smoke in risk mode,
collects smoke summary markdown, and emits a stable JSON payload for CI systems.

.EXAMPLE
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\ci_machine_json.ps1 `
  -Provider deepseek -DeepSeekApiKey "<KEY>"
#>

param(
    [string]$RepoRoot = "$(Join-Path $PSScriptRoot "..\..")",
    [ValidateSet("openai", "deepseek")]
    [string]$Provider = "deepseek",
    [string]$OpenAiBaseUrl = "",
    [string]$OpenAiApiKey = "",
    [string]$OpenAiModel = "gpt-5.3-codex",
    [string]$DeepSeekBaseUrl = "",
    [string]$DeepSeekApiKey = "",
    [string]$DeepSeekModel = "deepseek-v4-pro",
    [string]$ArtifactsDir = "",
    [string]$OutJson = "",
    [switch]$NoQuick,
    [switch]$AllowProviderNetworkError,
    [switch]$AllowReviewJsonNetworkError,
    [switch]$AllowSubagentNetworkError,
    [switch]$AllowGatewayNetworkError,
    [string]$GateRulesPath = "",
    [switch]$SkipFailGate
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($ArtifactsDir)) {
    $ArtifactsDir = Join-Path $RepoRoot "artifacts\ci_machine_json"
}
if ([string]::IsNullOrWhiteSpace($OutJson)) {
    $OutJson = Join-Path $ArtifactsDir "ci_machine_report.json"
}

New-Item -ItemType Directory -Force -Path $ArtifactsDir | Out-Null

$steps = New-Object System.Collections.Generic.List[object]
$started = Get-Date

function Add-Step(
    [string]$name,
    [string]$status,
    [double]$duration_secs,
    [string]$command,
    [int]$exit_code,
    [string]$detail
) {
    $steps.Add([ordered]@{
        name = $name
        status = $status
        duration_secs = [Math]::Round($duration_secs, 3)
        command = $command
        exit_code = $exit_code
        detail = $detail
    }) | Out-Null
}

function Run-Step([string]$name, [string]$command) {
    $stepStart = Get-Date
    $tmpOut = [System.IO.Path]::GetTempFileName()
    $tmpErr = [System.IO.Path]::GetTempFileName()
    $log = @()
    try {
        $proc = Start-Process `
            -FilePath "powershell" `
            -ArgumentList @("-NoProfile", "-Command", $command) `
            -WorkingDirectory $RepoRoot `
            -RedirectStandardOutput $tmpOut `
            -RedirectStandardError $tmpErr `
            -Wait `
            -PassThru
        $exitCode = $proc.ExitCode

        $stdout = if (Test-Path -LiteralPath $tmpOut) {
            Get-Content -LiteralPath $tmpOut -Raw
        } else {
            ""
        }
        $stderr = if (Test-Path -LiteralPath $tmpErr) {
            Get-Content -LiteralPath $tmpErr -Raw
        } else {
            ""
        }
        if (-not [string]::IsNullOrWhiteSpace($stdout)) {
            $log += ($stdout -split "`r?`n")
        }
        if (-not [string]::IsNullOrWhiteSpace($stderr)) {
            $log += ($stderr -split "`r?`n")
        }
        foreach ($line in $log) {
            if (-not [string]::IsNullOrWhiteSpace($line)) {
                Write-Host $line
            }
        }
    } finally {
        Remove-Item -LiteralPath $tmpOut -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $tmpErr -Force -ErrorAction SilentlyContinue
    }
    $elapsed = (Get-Date) - $stepStart
    $joined = ($log -join "`n")
    if ($exitCode -eq 0) {
        Add-Step $name "pass" $elapsed.TotalSeconds $command $exitCode ""
        return $true
    } else {
        Add-Step $name "fail" $elapsed.TotalSeconds $command $exitCode $joined
        return $false
    }
}

function Read-JsonFile([string]$path) {
    if (-not (Test-Path -LiteralPath $path)) { return $null }
    try {
        return (Get-Content -LiteralPath $path -Raw | ConvertFrom-Json)
    } catch {
        return $null
    }
}

function Get-DefaultGateRules {
    return [ordered]@{
        version = "1"
        require_step_status = "pass"
        require_report_status_pass = @("smoke", "smoke_prompt_jsonl")
        allow_missing_reports = @()
    }
}

function Load-GateRules([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) {
        return Get-DefaultGateRules
    }
    if (-not (Test-Path -LiteralPath $Path)) {
        throw "gate rules file not found: $Path"
    }
    $raw = Read-JsonFile $Path
    if ($null -eq $raw) {
        throw "invalid gate rules JSON: $Path"
    }
    return $raw
}

function Has-ListValue($listObj, [string]$value) {
    if ($null -eq $listObj) { return $false }
    foreach ($item in @($listObj)) {
        if ([string]$item -eq $value) { return $true }
    }
    return $false
}

function Get-PayloadStatus($payloadObj) {
    if ($null -eq $payloadObj) { return "missing" }
    $status = $payloadObj.status
    if ([string]::IsNullOrWhiteSpace([string]$status)) { return "unknown" }
    return [string]$status
}

function To-Array($obj) {
    if ($null -eq $obj) { return @() }
    if ($obj -is [System.Array]) { return $obj }
    if ($obj -is [System.Collections.IEnumerable] -and -not ($obj -is [string])) {
        return @($obj | ForEach-Object { $_ })
    }
    return ,$obj
}

function Evaluate-Gates($stepsObj, $reportsObj, $rulesObj) {
    $failedRules = New-Object System.Collections.Generic.List[object]
    $stepRequiredStatus = [string]$rulesObj.require_step_status
    if ([string]::IsNullOrWhiteSpace($stepRequiredStatus)) {
        $stepRequiredStatus = "pass"
    }

    foreach ($step in (To-Array $stepsObj)) {
        if ([string]$step.status -ne $stepRequiredStatus) {
            $failedRules.Add([ordered]@{
                code = "step_status_mismatch"
                message = "step '$($step.name)' status '$($step.status)' does not match required '$stepRequiredStatus'"
            }) | Out-Null
        }
    }

    foreach ($reportKey in (To-Array $rulesObj.require_report_status_pass)) {
        $key = [string]$reportKey
        $payload = $reportsObj.$key
        if ($null -eq $payload) {
            if (-not (Has-ListValue $rulesObj.allow_missing_reports $key)) {
                $failedRules.Add([ordered]@{
                    code = "report_missing"
                    message = "required report '$key' is missing"
                }) | Out-Null
            }
            continue
        }
        $status = Get-PayloadStatus $payload
        if ($status -ne "pass") {
            $failedRules.Add([ordered]@{
                code = "report_status_mismatch"
                message = "report '$key' status '$status' is not pass"
            }) | Out-Null
        }
    }

    $pass = ($failedRules.Count -eq 0)
    return [ordered]@{
        pass = $pass
        failed_rules = $failedRules
    }
}

$smokeScript = Join-Path $RepoRoot "scripts\windows\smoke.ps1"
$summaryScript = Join-Path $RepoRoot "scripts\windows\render_smoke_summary.ps1"
$promptJsonlScript = Join-Path $RepoRoot "scripts\windows\smoke_prompt_jsonl.ps1"
$smokeReportDir = Join-Path $ArtifactsDir "smoke"
$smokeSummaryPath = Join-Path $smokeReportDir "SMOKE_SUMMARY.md"
$smokeJson = Join-Path $smokeReportDir "smoke_all.json"
$promptJsonlReport = Join-Path $smokeReportDir "smoke_prompt_jsonl.json"

$pipelineOk = $true
$pipelineOk = (Run-Step "cargo_build_release" "cargo build --release") -and $pipelineOk
if ($pipelineOk) {
    $pipelineOk = (Run-Step "cargo_test_release" "cargo test --release") -and $pipelineOk
}

$smokeCmdParts = @(
    "powershell -NoProfile -ExecutionPolicy Bypass -File `"$smokeScript`"",
    "-Mode risk",
    "-Provider $Provider",
    "-Project `"$RepoRoot`"",
    "-Repo `"$RepoRoot`"",
    "-ReportDir `"$smokeReportDir`"",
    "-SummaryOutFile `"$smokeSummaryPath`""
)
if (-not $NoQuick) { $smokeCmdParts += "-Quick" }
if ($AllowProviderNetworkError) { $smokeCmdParts += "-AllowProviderNetworkError" }
if ($AllowReviewJsonNetworkError) { $smokeCmdParts += "-AllowReviewJsonNetworkError" }
if ($AllowSubagentNetworkError) { $smokeCmdParts += "-AllowSubagentNetworkError" }
if ($AllowGatewayNetworkError) { $smokeCmdParts += "-AllowGatewayNetworkError" }
if (-not [string]::IsNullOrWhiteSpace($OpenAiBaseUrl)) { $smokeCmdParts += "-OpenAiBaseUrl `"$OpenAiBaseUrl`"" }
if (-not [string]::IsNullOrWhiteSpace($OpenAiApiKey)) { $smokeCmdParts += "-OpenAiApiKey `"$OpenAiApiKey`"" }
if (-not [string]::IsNullOrWhiteSpace($OpenAiModel)) { $smokeCmdParts += "-OpenAiModel `"$OpenAiModel`"" }
if (-not [string]::IsNullOrWhiteSpace($DeepSeekBaseUrl)) { $smokeCmdParts += "-DeepSeekBaseUrl `"$DeepSeekBaseUrl`"" }
if (-not [string]::IsNullOrWhiteSpace($DeepSeekApiKey)) { $smokeCmdParts += "-DeepSeekApiKey `"$DeepSeekApiKey`"" }
if (-not [string]::IsNullOrWhiteSpace($DeepSeekModel)) { $smokeCmdParts += "-DeepSeekModel `"$DeepSeekModel`"" }

$smokeCmd = ($smokeCmdParts -join " ")
if ($pipelineOk) {
    $pipelineOk = (Run-Step "smoke_risk" $smokeCmd) -and $pipelineOk
}

$promptJsonlCmdParts = @(
    "powershell -NoProfile -ExecutionPolicy Bypass -File `"$promptJsonlScript`"",
    "-Provider $Provider",
    "-Project `"$RepoRoot`"",
    "-ReportJsonPath `"$promptJsonlReport`""
)
if (-not [string]::IsNullOrWhiteSpace($OpenAiBaseUrl)) { $promptJsonlCmdParts += "-OpenAiBaseUrl `"$OpenAiBaseUrl`"" }
if (-not [string]::IsNullOrWhiteSpace($OpenAiApiKey)) { $promptJsonlCmdParts += "-OpenAiApiKey `"$OpenAiApiKey`"" }
if (-not [string]::IsNullOrWhiteSpace($OpenAiModel)) { $promptJsonlCmdParts += "-OpenAiModel `"$OpenAiModel`"" }
if (-not [string]::IsNullOrWhiteSpace($DeepSeekBaseUrl)) { $promptJsonlCmdParts += "-DeepSeekBaseUrl `"$DeepSeekBaseUrl`"" }
if (-not [string]::IsNullOrWhiteSpace($DeepSeekApiKey)) { $promptJsonlCmdParts += "-DeepSeekApiKey `"$DeepSeekApiKey`"" }
if (-not [string]::IsNullOrWhiteSpace($DeepSeekModel)) { $promptJsonlCmdParts += "-DeepSeekModel `"$DeepSeekModel`"" }
$promptJsonlCmd = ($promptJsonlCmdParts -join " ")
if ($pipelineOk) {
    $pipelineOk = (Run-Step "smoke_prompt_jsonl" $promptJsonlCmd) -and $pipelineOk
}

$summaryCmd = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$summaryScript`" -ReportDir `"$smokeReportDir`" -OutFile `"$smokeSummaryPath`""
if ($pipelineOk) {
    $pipelineOk = (Run-Step "render_smoke_summary" $summaryCmd) -and $pipelineOk
}

$finished = Get-Date
$duration = ($finished - $started).TotalSeconds
$smokePayload = Read-JsonFile $smokeJson
$promptJsonlPayload = Read-JsonFile $promptJsonlReport
$gateRules = Load-GateRules $GateRulesPath
$reports = [ordered]@{
    smoke = $smokePayload
    smoke_prompt_jsonl = $promptJsonlPayload
}
$gates = Evaluate-Gates $steps $reports $gateRules

$failedCount = @($steps | Where-Object { $_.status -eq "fail" }).Count
$status = if ($failedCount -eq 0 -and $gates.pass) { "pass" } else { "fail" }
$payload = [ordered]@{
    schema_version = "1"
    script = "ci_machine_json"
    status = $status
    ci_schema = "asi.ci.machine_report.v1"
    started_at = $started.ToString("o")
    finished_at = $finished.ToString("o")
    duration_secs = [Math]::Round($duration, 3)
    repo_root = $RepoRoot
    provider = $Provider
    artifacts_dir = $ArtifactsDir
    out_json = $OutJson
    smoke_report_path = $smokeJson
    smoke_prompt_jsonl_report_path = $promptJsonlReport
    smoke_summary_path = $smokeSummaryPath
    smoke = $smokePayload
    smoke_prompt_jsonl = $promptJsonlPayload
    gates = [ordered]@{
        pass = [bool]$gates.pass
        rules = $gateRules
        failed_rules = $gates.failed_rules
    }
    steps = $steps
}

$payload | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $OutJson -Encoding UTF8
if ($status -eq "pass") {
    Write-Host "ci_machine_json: PASS"
    Write-Host ("report_json={0}" -f $OutJson)
    exit 0
}
Write-Error "ci_machine_json: FAIL (see report_json)"
Write-Host ("report_json={0}" -f $OutJson)
if ($SkipFailGate) {
    exit 0
}
exit 2
