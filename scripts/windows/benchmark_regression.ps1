param(
    [string]$AsiExe = "$(Join-Path $PSScriptRoot "..\..\target\release\asi.exe")",
    [string]$Project = "D:\Code",
    [string[]]$Cases = @(
        "openai:gpt-4.1-mini",
        "deepseek:deepseek-v4-pro",
        "claude:claude-3-7-sonnet-latest"
    ),
    [string]$PromptText = "Summarize the current project in 3 short bullet points.",
    [switch]$Agent,
    [int]$AgentMaxSteps = 20,
    [string]$OutDir = "$(Join-Path $PSScriptRoot "..\..\dist\benchmarks")",
    [switch]$DryRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $AsiExe)) {
    throw "asi binary not found: $AsiExe"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$rows = @()
$started = Get-Date

foreach ($case in $Cases) {
    $parts = $case.Split(':', 2)
    if ($parts.Count -ne 2 -or [string]::IsNullOrWhiteSpace($parts[0]) -or [string]::IsNullOrWhiteSpace($parts[1])) {
        Write-Warning "skip invalid case: $case"
        continue
    }

    $provider = $parts[0].Trim()
    $model = $parts[1].Trim()

    $args = @(
        "prompt",
        $PromptText,
        "--provider", $provider,
        "--model", $model,
        "--project", $Project,
        "--output-format", "json"
    )
    if ($Agent) {
        $args += @("--agent", "--agent-max-steps", "$AgentMaxSteps")
    }

    Write-Host "[run] provider=$provider model=$model agent=$Agent"

    if ($DryRun) {
        $rows += [pscustomobject]@{
            timestamp = (Get-Date).ToString("s")
            provider = $provider
            requested_model = $model
            stop_reason = "dry_run"
            exit_code = 0
            elapsed_ms = 0
            input_tokens = 0
            output_tokens = 0
            turn_cost_usd = 0
            total_cost_usd = 0
            parse_ok = $true
            parse_error = ""
            raw_excerpt = ""
        }
        continue
    }

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = & $AsiExe @args 2>&1
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $prev
        $sw.Stop()
    }

    $text = ($output | Out-String -Width 16384).Trim()
    $jsonObj = $null
    $parseError = ""

    $jsonStart = $text.IndexOf('{')
    if ($jsonStart -ge 0) {
        $jsonText = $text.Substring($jsonStart)
        try {
            $jsonObj = $jsonText | ConvertFrom-Json -ErrorAction Stop
        }
        catch {
            $parseError = $_.Exception.Message
        }
    }
    else {
        $parseError = "json payload not found"
    }

    $usage = $null
    if ($jsonObj -and $jsonObj.PSObject.Properties.Name -contains "usage") {
        $usage = $jsonObj.usage
    }

    $rows += [pscustomobject]@{
        timestamp = (Get-Date).ToString("s")
        provider = $provider
        requested_model = $model
        stop_reason = if ($jsonObj) { "$($jsonObj.stop_reason)" } else { "" }
        exit_code = $exitCode
        elapsed_ms = [int]$sw.ElapsedMilliseconds
        input_tokens = if ($usage) { [int]$usage.input_tokens } else { 0 }
        output_tokens = if ($usage) { [int]$usage.output_tokens } else { 0 }
        turn_cost_usd = if ($usage) { [double]$usage.turn_cost_usd } else { 0.0 }
        total_cost_usd = if ($usage) { [double]$usage.total_cost_usd } else { 0.0 }
        parse_ok = [string]::IsNullOrEmpty($parseError)
        parse_error = $parseError
        raw_excerpt = if ($text.Length -gt 300) { $text.Substring(0, 300) } else { $text }
    }
}

$stamp = Get-Date -Format "yyyyMMdd_HHmmss"
$csvPath = Join-Path $OutDir "benchmark_regression_$stamp.csv"
$jsonPath = Join-Path $OutDir "benchmark_regression_$stamp.json"

$rows | Export-Csv -NoTypeInformation -Encoding UTF8 -Path $csvPath
$rows | ConvertTo-Json -Depth 8 | Set-Content -Path $jsonPath -Encoding UTF8

$elapsed = (Get-Date) - $started
Write-Host ""
Write-Host "benchmark-regression finished in $([math]::Round($elapsed.TotalSeconds, 1))s"
Write-Host "csv:  $csvPath"
Write-Host "json: $jsonPath"
Write-Host ""
$rows | Format-Table provider, requested_model, stop_reason, exit_code, elapsed_ms, parse_ok -AutoSize
