param(
    [string]$ProjectPath,
    [string]$Provider = "deepseek",
    [switch]$WithSetup
)

$ErrorActionPreference = "Stop"

function Resolve-ProviderChoice {
    param(
        [string]$Raw,
        [string]$Current
    )

    $rawText = if ($null -eq $Raw) { "" } else { $Raw }
    $v = $rawText.Trim().ToLowerInvariant()
    if ([string]::IsNullOrWhiteSpace($v)) {
        return $Current
    }

    switch ($v) {
        "1" { return "openai" }
        "2" { return "deepseek" }
        "3" { return "claude" }
        "openai" { return "openai" }
        "deepseek" { return "deepseek" }
        "claude" { return "claude" }
        "claude-code" { return "claude" }
        "claude code" { return "claude" }
        default { return $Current }
    }
}

function Get-ProviderApiEnvName {
    param([string]$Name)

    $nameText = if ($null -eq $Name) { "" } else { $Name }
    switch ($nameText.Trim().ToLowerInvariant()) {
        "openai" { return "OPENAI_API_KEY" }
        "deepseek" { return "DEEPSEEK_API_KEY" }
        "claude" { return "ANTHROPIC_API_KEY" }
        default { return "OPENAI_API_KEY" }
    }
}

function Read-OptionalSecretPlaintext {
    param([string]$Prompt)

    $secure = Read-Host -AsSecureString $Prompt
    if ($null -eq $secure) {
        return ""
    }

    $ptr = [IntPtr]::Zero
    try {
        $ptr = [System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
        $plain = [System.Runtime.InteropServices.Marshal]::PtrToStringBSTR($ptr)
        if ($null -eq $plain) {
            return ""
        }
        return $plain.Trim()
    }
    finally {
        if ($ptr -ne [IntPtr]::Zero) {
            [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($ptr)
        }
    }
}
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $scriptDir

if (-not (Test-Path (Join-Path $scriptDir "Cargo.toml"))) {
    Write-Error "Cargo.toml not found in $scriptDir"
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo not found. Install Rust first: https://rustup.rs"
}

$Provider = Resolve-ProviderChoice -Raw $Provider -Current "deepseek"

Write-Host "ASI Code Terminal App" -ForegroundColor Cyan
Write-Host ""
Write-Host "Select provider:"
Write-Host "1) OpenAI"
Write-Host "2) DeepSeek"
Write-Host "3) Claude"

$providerInput = Read-Host "Provider [1-3|name, Enter keep $Provider]"
$Provider = Resolve-ProviderChoice -Raw $providerInput -Current $Provider

$keyEnvName = Get-ProviderApiEnvName -Name $Provider
$currentKey = [Environment]::GetEnvironmentVariable($keyEnvName, "Process")
$keyStatus = if ([string]::IsNullOrWhiteSpace($currentKey)) { "not set" } else { "set" }

Write-Host ""
Write-Host "Mode: cargo run --release -- repl --provider $Provider --project <path> --no-setup"
Write-Host ("{0}: {1}" -f $keyEnvName, $keyStatus)

$keyInput = Read-OptionalSecretPlaintext -Prompt "Input $keyEnvName (optional, Enter keep current)"
if (-not [string]::IsNullOrWhiteSpace($keyInput)) {
    [Environment]::SetEnvironmentVariable($keyEnvName, $keyInput, "Process")
}

$defaultProject = if (Test-Path "D:\Code") { "D:\Code" } else { $scriptDir }

if ([string]::IsNullOrWhiteSpace($ProjectPath)) {
    $projectInput = Read-Host "Project path (Enter use $defaultProject)"
    if ([string]::IsNullOrWhiteSpace($projectInput)) {
        $ProjectPath = $defaultProject
    } else {
        $ProjectPath = $projectInput.Trim().Trim('"').Trim("'")
    }
}

if (-not (Test-Path $ProjectPath)) {
    Write-Error "Project path not found: $ProjectPath"
}

$args = @(
    "run",
    "--release",
    "--",
    "repl",
    "--provider", $Provider,
    "--project", $ProjectPath
)

if (-not $WithSetup.IsPresent) {
    $args += "--no-setup"
}

Write-Host ""
Write-Host ("Running: cargo " + ($args -join " ")) -ForegroundColor DarkCyan
Write-Host ""

& cargo @args
exit $LASTEXITCODE


