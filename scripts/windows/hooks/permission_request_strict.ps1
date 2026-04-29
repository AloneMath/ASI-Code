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

    # Strict policy: only bash is gated here. Non-bash tools pass.
    if ($tool -ne "bash") {
        Write-Allow "strict permission hook: non-bash tool allowed"
        exit 0
    }

    $command = $args.Trim()
    if ([string]::IsNullOrWhiteSpace($command)) {
        Write-Deny "strict permission hook: empty bash command denied"
        exit 0
    }

    # Whitelist format:
    #   ASI_HOOK_BASH_ALLOW_PREFIXES='cargo check;cargo test;git status'
    # Matching is case-insensitive prefix match.
    $raw = [string]$env:ASI_HOOK_BASH_ALLOW_PREFIXES
    $allowPrefixes = @()
    if (-not [string]::IsNullOrWhiteSpace($raw)) {
        $allowPrefixes = $raw -split '[;,`n]' |
            ForEach-Object { $_.Trim() } |
            Where-Object { $_ -ne "" }
    }

    if ($allowPrefixes.Count -eq 0) {
        Write-Deny "strict permission hook: bash denied by default (set ASI_HOOK_BASH_ALLOW_PREFIXES)"
        exit 0
    }

    $lower = $command.ToLowerInvariant()
    foreach ($prefix in $allowPrefixes) {
        $p = $prefix.ToLowerInvariant()
        if ($lower.StartsWith($p)) {
            Write-Allow ("strict permission hook: bash allowed by prefix '" + $prefix + "'")
            exit 0
        }
    }

    Write-Deny "strict permission hook: bash command not in allow-prefix list"
    exit 0
}
catch {
    Write-Deny ("strict permission hook exception: " + $_.Exception.Message)
    exit 0
}
