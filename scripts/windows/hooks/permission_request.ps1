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
    $mode = if ($payload -and $payload.permission_mode) { [string]$payload.permission_mode } else { [string]$env:ASI_HOOK_PERMISSION_MODE }

    # Sample gate:
    # - In read-only mode, block bash entirely.
    # - In workspace-write mode, allow bash only for read/build/test-style commands.
    if ($tool -eq "bash") {
        if ($mode -eq "read-only") {
            Write-Deny "permission hook: bash blocked in read-only mode"
            exit 0
        }

        if ($mode -eq "workspace-write") {
            $lower = $args.ToLowerInvariant().Trim()
            $allow = @(
                "cargo check", "cargo test", "cargo fmt", "cargo clippy",
                "python -m", "pytest", "npm test", "npm run", "pnpm test",
                "git status", "git diff", "Get-ChildItem".ToLowerInvariant()
            )
            $matched = $false
            foreach ($prefix in $allow) {
                if ($lower.StartsWith($prefix.ToLowerInvariant())) {
                    $matched = $true
                    break
                }
            }
            if (-not $matched) {
                Write-Deny "permission hook: bash command not in allow-prefix list"
                exit 0
            }
        }
    }

    Write-Allow "permission hook passed"
    exit 0
}
catch {
    Write-Deny ("permission hook exception: " + $_.Exception.Message)
    exit 0
}
