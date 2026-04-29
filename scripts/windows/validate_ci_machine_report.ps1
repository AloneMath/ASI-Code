param(
    [string]$ReportPath = "",
    [switch]$Strict
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($ReportPath)) {
    Write-Error "Usage: validate_ci_machine_report.ps1 -ReportPath <ci_machine_report.json> [-Strict]"
    exit 2
}
if (-not (Test-Path -LiteralPath $ReportPath)) {
    Write-Error "report not found: $ReportPath"
    exit 2
}

try {
    $report = Get-Content -LiteralPath $ReportPath -Raw | ConvertFrom-Json
} catch {
    Write-Error "invalid json: $ReportPath"
    exit 2
}

function Require-Field($obj, [string]$name) {
    if ($null -eq $obj.PSObject.Properties[$name]) {
        throw "missing field: $name"
    }
}

try {
    Require-Field $report "schema_version"
    Require-Field $report "ci_schema"
    Require-Field $report "status"
    Require-Field $report "steps"
    Require-Field $report "gates"
    Require-Field $report.gates "pass"
    Require-Field $report.gates "failed_rules"
} catch {
    Write-Error $_.Exception.Message
    exit 2
}

if ([string]$report.schema_version -ne "1") {
    Write-Error "schema_version must be 1"
    exit 2
}
if ([string]$report.ci_schema -ne "asi.ci.machine_report.v1") {
    Write-Error "ci_schema must be asi.ci.machine_report.v1"
    exit 2
}

$stepCount = @($report.steps).Count
if ($stepCount -lt 1) {
    Write-Error "steps must be non-empty"
    exit 2
}

if ($Strict -and -not [bool]$report.gates.pass) {
    $codes = @($report.gates.failed_rules | ForEach-Object { $_.code }) -join ","
    Write-Error "gate failed: $codes"
    exit 3
}

Write-Host "validate_ci_machine_report: PASS"
Write-Host ("report={0}" -f $ReportPath)
Write-Host ("status={0} gates_pass={1} steps={2}" -f $report.status, [bool]$report.gates.pass, $stepCount)
exit 0
