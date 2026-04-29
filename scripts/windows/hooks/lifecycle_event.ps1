param()

$ErrorActionPreference = "Stop"

function Write-Allow([string]$reason) {
    Write-Output (@{ allow = $true; reason = $reason } | ConvertTo-Json -Compress)
}

try {
    $payload = $null
    if (-not [string]::IsNullOrWhiteSpace($env:ASI_HOOK_INPUT_JSON)) {
        $payload = $env:ASI_HOOK_INPUT_JSON | ConvertFrom-Json
    }

    $event = if ($payload -and $payload.event) { [string]$payload.event } else { [string]$env:ASI_HOOK_EVENT }
    $tool = if ($payload -and $payload.tool) { [string]$payload.tool } else { [string]$env:ASI_HOOK_TOOL }
    $args = if ($payload -and $payload.args) { [string]$payload.args } else { [string]$env:ASI_HOOK_ARGS }
    $mode = if ($payload -and $payload.permission_mode) { [string]$payload.permission_mode } else { [string]$env:ASI_HOOK_PERMISSION_MODE }

    $logDir = Join-Path (Get-Location) ".asi"
    if (-not (Test-Path -LiteralPath $logDir)) {
        New-Item -ItemType Directory -Path $logDir | Out-Null
    }

    $line = ("{0}`t{1}`t{2}`t{3}`t{4}" -f (Get-Date).ToString("o"), $event, $tool, $mode, $args)
    Add-Content -LiteralPath (Join-Path $logDir "hooks.lifecycle.log") -Value $line

    Write-Allow ("lifecycle hook logged event=" + $event)
    exit 0
}
catch {
    # Lifecycle hooks are observability hooks; keep execution non-blocking.
    Write-Allow ("lifecycle hook soft-fail: " + $_.Exception.Message)
    exit 0
}
