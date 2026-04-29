param()

$ErrorActionPreference = "Stop"

function Write-Allow([string]$reason) {
    Write-Output (@{ allow = $true; reason = $reason } | ConvertTo-Json -Compress)
}

function Write-Deny([string]$reason) {
    Write-Output (@{ allow = $false; reason = $reason } | ConvertTo-Json -Compress)
}

try {
    $payload = $null
    if (-not [string]::IsNullOrWhiteSpace($env:ASI_HOOK_INPUT_JSON)) {
        $payload = $env:ASI_HOOK_INPUT_JSON | ConvertFrom-Json
    }

    $event = if ($payload -and $payload.event) { [string]$payload.event } else { [string]$env:ASI_HOOK_EVENT }
    $tool = if ($payload -and $payload.tool) { [string]$payload.tool } else { [string]$env:ASI_HOOK_TOOL }
    $args = if ($payload -and $payload.args) { [string]$payload.args } else { [string]$env:ASI_HOOK_ARGS }

    # Post hook example: append a compact audit line.
    $logDir = Join-Path (Get-Location) ".asi"
    if (-not (Test-Path -LiteralPath $logDir)) {
        New-Item -ItemType Directory -Path $logDir | Out-Null
    }
    $line = ("{0}`t{1}`t{2}`t{3}" -f (Get-Date).ToString("o"), $event, $tool, $args)
    Add-Content -LiteralPath (Join-Path $logDir "hooks.log") -Value $line

    Write-Allow "post-tool hook logged"
    exit 0
}
catch {
    # Post hook should never block execution flow at runtime.
    Write-Allow ("post-tool hook soft-fail: " + $_.Exception.Message)
    exit 0
}
