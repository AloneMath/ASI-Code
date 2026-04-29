[CmdletBinding()]
param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\Code\Rust",
    [string]$ReportJsonPath = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$startedAt = Get-Date

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
        script = "smoke_hooks_cli_advanced"
        timestamp_utc = (Get-Date).ToUniversalTime().ToString("o")
        status = $Status
        duration_secs = [Math]::Round($DurationSecs, 3)
        failure_category = if ($Status -eq "fail") { $FailureCategory } else { $null }
        hint = if ($Status -eq "fail") { $Hint } else { $null }
        message = if ($Status -eq "fail") { $Message } else { $null }
        config = [ordered]@{
            asi_exe = $AsiExe
            project = $Project
        }
        metrics = $Extra
    }
    $payload | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $ReportJsonPath -Encoding UTF8
}

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) {
        throw $Message
    }
}

function Extract-JsonLine([string]$Text) {
    if ([string]::IsNullOrWhiteSpace($Text)) { return "" }
    $lines = $Text -split "`r?`n"
    foreach ($line in $lines) {
        $t = $line.Trim()
        if ($t.StartsWith("{") -and $t.Contains('"schema_version"')) {
            return $t
        }
    }
    return ""
}

function Invoke-AsiText([string[]]$CliArgs) {
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = & $AsiExe @CliArgs 2>&1
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $prev
    }
    $text = $output | Out-String
    if ($exitCode -ne 0) {
        $argText = [string]::Join(" ", $CliArgs)
        throw "asi command failed exit=$exitCode args=$argText output=$text"
    }
    return $text
}

function Write-Utf8NoBom([string]$Path, [string]$Content) {
    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    [System.IO.File]::WriteAllText($Path, $Content, $utf8NoBom)
}

if (-not (Test-Path -LiteralPath $AsiExe)) {
    $msg = "asi binary not found: $AsiExe"
    $elapsed = (Get-Date) - $startedAt
    Write-Report -Status "fail" -FailureCategory "binary_missing" -Hint "Build release binary first: cargo build --release." -Message $msg -DurationSecs $elapsed.TotalSeconds
    throw $msg
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("asi_hooks_cli_advanced_{0}.json" -f ([Guid]::NewGuid().ToString("N")))

try {
    $setText = Invoke-AsiText -CliArgs @(
        "hooks","config","set-handler","SessionStart","python hook_adv.py",
        "--path",$tmp,"--timeout-secs","10","--json-protocol","on",
        "--tool-prefix","bash","--permission-mode","on-request","--failure-policy","fail-open","--json"
    )
    $setJson = Extract-JsonLine $setText
    Assert-True ($setJson.Contains('"command":"hooks_config_set_handler"')) ("set-handler did not return hooks_config_set_handler output=" + $setText)

    $editText = Invoke-AsiText -CliArgs @(
        "hooks","config","edit-handler","SessionStart","python hook_adv.py",
        "--path",$tmp,"--timeout-secs","none","--json-protocol","none",
        "--tool-prefix","none","--permission-mode","none","--failure-policy","none","--json"
    )
    $editJson = Extract-JsonLine $editText
    Assert-True ($editJson.Contains('"command":"hooks_config_edit_handler"')) ("edit-handler did not return hooks_config_edit_handler output=" + $editText)
    Assert-True ($editJson.Contains('"timeout_secs":null')) "edit-handler did not clear timeout_secs"
    Assert-True ($editJson.Contains('"json_protocol":null')) "edit-handler did not clear json_protocol"
    Assert-True ($editJson.Contains('"tool_prefix":null')) "edit-handler did not clear tool_prefix"
    Assert-True ($editJson.Contains('"permission_mode":null')) "edit-handler did not clear permission_mode"
    Assert-True ($editJson.Contains('"failure_policy":null')) "edit-handler did not clear failure_policy"

    $validateStrictPassText = Invoke-AsiText -CliArgs @("hooks","config","validate","--path",$tmp,"--strict","--json")
    $validateStrictPassJson = Extract-JsonLine $validateStrictPassText
    Assert-True ($validateStrictPassJson.Contains('"command":"hooks_config_validate"')) "validate did not return hooks_config_validate"
    Assert-True ($validateStrictPassJson.Contains('"strict":true')) "validate output missing strict=true"
    Assert-True ($validateStrictPassJson.Contains('"valid":true')) "validate strict expected valid=true"

    $dupText = @'
{
  "handlers": [
    { "event": "SessionStart", "script": "python dup.py" },
    { "event": "SessionStart", "script": "python dup.py" }
  ]
}
'@
    Write-Utf8NoBom -Path $tmp -Content $dupText

    $validateNonStrictText = Invoke-AsiText -CliArgs @("hooks","config","validate","--path",$tmp,"--json")
    $validateNonStrictJson = Extract-JsonLine $validateNonStrictText
    Assert-True ($validateNonStrictJson.Contains('"valid":true')) "validate non-strict expected valid=true with duplicate warning"
    Assert-True ($validateNonStrictJson.Contains('"warning_count":1')) "validate non-strict expected warning_count=1"

    $validateStrictFailText = Invoke-AsiText -CliArgs @("hooks","config","validate","--path",$tmp,"--strict","--json")
    $validateStrictFailJson = Extract-JsonLine $validateStrictFailText
    Assert-True ($validateStrictFailJson.Contains('"strict":true')) "validate strict-fail output missing strict=true"
    Assert-True ($validateStrictFailJson.Contains('"valid":false')) "validate strict expected valid=false with duplicate warning"
    Assert-True ($validateStrictFailJson.Contains('"warning_count":1')) "validate strict expected warning_count=1"

    Write-Host "smoke_hooks_cli_advanced: PASS"
    $elapsed = (Get-Date) - $startedAt
    Write-Report -Status "pass" -DurationSecs $elapsed.TotalSeconds -Extra @{
        edit_none_clears_fields = $true
        validate_strict_positive = $true
        validate_strict_negative = $true
    }
    exit 0
}
catch {
    $msg = $_.Exception.Message
    $elapsed = (Get-Date) - $startedAt
    Write-Report -Status "fail" -FailureCategory "hooks_cli_advanced_smoke_failure" -Hint "Run smoke_hooks_cli_advanced.ps1 standalone and inspect edit-handler/validate --strict JSON output." -Message $msg -DurationSecs $elapsed.TotalSeconds
    throw
}
finally {
    if (Test-Path -LiteralPath $tmp) {
        Remove-Item -LiteralPath $tmp -Force -ErrorAction SilentlyContinue
    }
}
