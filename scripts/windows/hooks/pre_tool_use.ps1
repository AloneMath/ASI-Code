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

    $tool = if ($payload -and $payload.tool) { [string]$payload.tool } else { [string]$env:ASI_HOOK_TOOL }
    $args = if ($payload -and $payload.args) { [string]$payload.args } else { [string]$env:ASI_HOOK_ARGS }

    # Block high-risk destructive shell patterns before permission checks.
    if ($tool -eq "bash") {
        $lower = $args.ToLowerInvariant()
        if ($lower -match "(\brm\s+-rf\b)|(\bdel\s+/f\b)|(\bformat\s+[a-z]:\b)|(\bgit\s+reset\s+--hard\b)") {
            Write-Deny "blocked by pre-tool hook: destructive shell pattern"
            exit 0
        }
    }

    # Example: block writes to lockfile by policy in pre-hook (adjust to your needs)
    if (($tool -eq "write_file" -or $tool -eq "edit_file") -and $args.ToLowerInvariant().Contains("cargo.lock")) {
        Write-Deny "blocked by pre-tool hook: cargo.lock edit requires explicit approval"
        exit 0
    }

    Write-Allow "pre-tool policy passed"
    exit 0
}
catch {
    Write-Deny ("pre-tool hook exception: " + $_.Exception.Message)
    exit 0
}
