param(
    [string]$OutputDir = "dist",
    [switch]$SkipBuild,
    [switch]$Offline,
    [switch]$SkipInstallerExe,
    [switch]$SignArtifacts,
    [string]$SignToolPath = "signtool.exe",
    [string]$CertFile,
    [string]$CertThumbprint,
    [string]$TimestampUrl = "http://timestamp.digicert.com",
    [string]$CertPasswordEnv = "ASI_SIGN_CERT_PASSWORD"
)

$ErrorActionPreference = "Stop"

function Invoke-CodeSign {
    param([string]$FilePath)

    if (-not $SignArtifacts) {
        return
    }

    if (-not (Test-Path -LiteralPath $FilePath)) {
        throw "File to sign not found: $FilePath"
    }

    if ([string]::IsNullOrWhiteSpace($CertFile) -and [string]::IsNullOrWhiteSpace($CertThumbprint)) {
        throw "SignArtifacts enabled but neither -CertFile nor -CertThumbprint was provided."
    }

    if (-not (Get-Command $SignToolPath -ErrorAction SilentlyContinue)) {
        throw "signtool not found: $SignToolPath"
    }

    $args = @("sign", "/fd", "SHA256")

    if (-not [string]::IsNullOrWhiteSpace($TimestampUrl)) {
        $args += @("/tr", $TimestampUrl, "/td", "SHA256")
    }

    if (-not [string]::IsNullOrWhiteSpace($CertFile)) {
        $args += @("/f", $CertFile)

        $certPassword = [Environment]::GetEnvironmentVariable($CertPasswordEnv, "Process")
        if ([string]::IsNullOrWhiteSpace($certPassword)) {
            $certPassword = [Environment]::GetEnvironmentVariable($CertPasswordEnv, "User")
        }
        if ([string]::IsNullOrWhiteSpace($certPassword)) {
            $certPassword = [Environment]::GetEnvironmentVariable($CertPasswordEnv, "Machine")
        }
        if (-not [string]::IsNullOrWhiteSpace($certPassword)) {
            $args += @("/p", $certPassword)
        }
    } else {
        $args += @("/sha1", $CertThumbprint)
    }

    $args += @("/v", $FilePath)

    Write-Host "Signing: $FilePath"
    & $SignToolPath @args
    if ($LASTEXITCODE -ne 0) {
        throw "signtool failed for $FilePath (exit code $LASTEXITCODE)"
    }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\")).Path
Set-Location $repoRoot

if (-not $SkipBuild) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        throw "cargo not found. Install Rust from https://rustup.rs"
    }

    $args = @("build", "--release")
    if ($Offline) {
        $args += "--offline"
    }

    Write-Host "Building release binary..."
    & cargo @args
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($args -join ' ') failed with exit code $LASTEXITCODE"
    }
}

$exePath = Join-Path $repoRoot "target\release\asi.exe"
if (-not (Test-Path -LiteralPath $exePath)) {
    throw "Release binary not found: $exePath"
}

$outputRoot = Join-Path $repoRoot $OutputDir
$stageRoot = Join-Path $outputRoot "asi-code-windows-x64"
$binDir = Join-Path $stageRoot "bin"
$scriptsDir = Join-Path $stageRoot "scripts\windows"

if (Test-Path -LiteralPath $stageRoot) {
    Remove-Item -LiteralPath $stageRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $binDir -Force | Out-Null
New-Item -ItemType Directory -Path $scriptsDir -Force | Out-Null

$stagedExePath = Join-Path $binDir "asi.exe"
Copy-Item -LiteralPath $exePath -Destination $stagedExePath -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "scripts\windows\install_asi.ps1") -Destination (Join-Path $scriptsDir "install_asi.ps1") -Force
Copy-Item -LiteralPath (Join-Path $repoRoot "scripts\windows\uninstall_asi.ps1") -Destination (Join-Path $scriptsDir "uninstall_asi.ps1") -Force

$launcherSourcePath = Join-Path $repoRoot "scripts\windows\asi_launcher.rs"
if (-not (Test-Path -LiteralPath $launcherSourcePath)) {
    throw "Portable launcher source not found: $launcherSourcePath"
}
if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) {
    throw "rustc not found. Cannot build portable launcher exe."
}

$portableLauncherPath = Join-Path $stageRoot "asi.exe"
Write-Host "Building portable launcher..."
& rustc --edition=2021 -O -C debuginfo=0 $launcherSourcePath -o $portableLauncherPath
$launcherPdbPath = Join-Path $stageRoot "asi.pdb"
if (Test-Path -LiteralPath $launcherPdbPath) {
    Remove-Item -LiteralPath $launcherPdbPath -Force
}
if ($LASTEXITCODE -ne 0) {
    throw "rustc failed to build portable launcher"
}


Invoke-CodeSign -FilePath $stagedExePath
Invoke-CodeSign -FilePath $portableLauncherPath

Copy-Item -LiteralPath $portableLauncherPath -Destination (Join-Path $stageRoot "ASI Launcher.exe") -Force

$wrapperCmd = @"
@echo off
"%~dp0bin\asi.exe" %*
"@
$wrapperCmd | Set-Content -Path (Join-Path $stageRoot "asi.cmd") -Encoding Ascii

$startCmd = @'
@echo off
setlocal EnableDelayedExpansion
cd /d "%~dp0"
title ASI Code Portable Terminal App
echo.
echo ASI Code Portable Terminal App
echo.

echo Select provider:
echo   1^) OpenAI
echo   2^) DeepSeek
echo   3^) Claude
set "PROVIDER=deepseek"
set /p PROVIDER_INPUT=Provider [1-3^|name, Enter keep deepseek]: 
if /I "!PROVIDER_INPUT!"=="1" set "PROVIDER=openai"
if /I "!PROVIDER_INPUT!"=="2" set "PROVIDER=deepseek"
if /I "!PROVIDER_INPUT!"=="3" set "PROVIDER=claude"
if /I "!PROVIDER_INPUT!"=="openai" set "PROVIDER=openai"
if /I "!PROVIDER_INPUT!"=="deepseek" set "PROVIDER=deepseek"
if /I "!PROVIDER_INPUT!"=="claude" set "PROVIDER=claude"
if /I "!PROVIDER_INPUT!"=="claude-code" set "PROVIDER=claude"
if /I "!PROVIDER_INPUT!"=="claude code" set "PROVIDER=claude"

set "KEY_ENV=DEEPSEEK_API_KEY"
if /I "!PROVIDER!"=="openai" set "KEY_ENV=OPENAI_API_KEY"
if /I "!PROVIDER!"=="claude" set "KEY_ENV=ANTHROPIC_API_KEY"

set "KEY_STATUS=not set"
if /I "!KEY_ENV!"=="OPENAI_API_KEY" if defined OPENAI_API_KEY set "KEY_STATUS=set"
if /I "!KEY_ENV!"=="DEEPSEEK_API_KEY" if defined DEEPSEEK_API_KEY set "KEY_STATUS=set"
if /I "!KEY_ENV!"=="ANTHROPIC_API_KEY" if defined ANTHROPIC_API_KEY set "KEY_STATUS=set"

echo.
echo Mode:
echo   .\bin\asi.exe repl --provider !PROVIDER! --project ^<path^> --no-setup
echo !KEY_ENV!: !KEY_STATUS!

set "KEY_INPUT="
for /f "usebackq delims=" %%K in (`powershell -NoProfile -Command "$prompt='Input ' + $env:KEY_ENV + ' (optional, Enter keep current)'; $secure=Read-Host -AsSecureString $prompt; if($secure){ $ptr=[System.Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure); try { [System.Runtime.InteropServices.Marshal]::PtrToStringBSTR($ptr) } finally { [System.Runtime.InteropServices.Marshal]::ZeroFreeBSTR($ptr) } }"`) do set "KEY_INPUT=%%K"
if not "!KEY_INPUT!"=="" (
  if /I "!KEY_ENV!"=="OPENAI_API_KEY" set "OPENAI_API_KEY=!KEY_INPUT!"
  if /I "!KEY_ENV!"=="DEEPSEEK_API_KEY" set "DEEPSEEK_API_KEY=!KEY_INPUT!"
  if /I "!KEY_ENV!"=="ANTHROPIC_API_KEY" set "ANTHROPIC_API_KEY=!KEY_INPUT!"
)

set "DEFAULT_PROJECT=D:\Code"
if not exist "%DEFAULT_PROJECT%" set "DEFAULT_PROJECT=%CD%"
set "PROJECT_PATH=%DEFAULT_PROJECT%"
set /p PROJECT_INPUT=Project path [!DEFAULT_PROJECT!]: 
if not "!PROJECT_INPUT!"=="" set "PROJECT_PATH=!PROJECT_INPUT!"

if not exist "!PROJECT_PATH!" (
  echo.
  echo [ERROR] Project path not found: !PROJECT_PATH!
  pause
  exit /b 1
)

echo.
echo Running ASI Code...
echo.
"%~dp0bin\asi.exe" repl --provider !PROVIDER! --project "!PROJECT_PATH!" --no-setup
set "ASI_EXIT=%ERRORLEVEL%"

echo.
if not "%ASI_EXIT%"=="0" (
  echo ASI exited with code %ASI_EXIT%.
) else (
  echo ASI session ended.
)
pause
exit /b %ASI_EXIT%
'@
$startCmd | Set-Content -Path (Join-Path $stageRoot "start_asi.cmd") -Encoding Ascii

$versionLine = "asi unknown"
try {
    $line = & $exePath version 2>$null | Select-Object -First 1
    if (-not [string]::IsNullOrWhiteSpace($line)) {
        $versionLine = $line.Trim()
    }
} catch {
}

$versionTag = "dev"
if ($versionLine -match '(\d+\.\d+\.\d+)') {
    $versionTag = $Matches[1]
}

$installerName = "asi-code-installer-$versionTag.exe"
$installerPath = Join-Path $outputRoot $installerName
$zipPath = Join-Path $outputRoot ("asi-code-windows-x64-{0}.zip" -f $versionTag)

$packageReadme = @"
# ASI Code Windows Package

## One-Click Installer (Recommended)

- $installerName
- Double click to install.
- Optional custom install dir:
  .\$installerName "D:\\Apps\\ASI Code"

## Quick Install From Package Files

1. Open PowerShell in this package directory.
2. Run:
   powershell -ExecutionPolicy Bypass -File .\scripts\windows\install_asi.ps1 -SourceExe .\bin\asi.exe

## Quick Start

1. Double click asi.exe (portable launcher) or ASI Launcher.exe (same behavior).
2. Select provider: OpenAI / DeepSeek / Claude.
3. Input API key for selected provider (masked input, optional):
   - OpenAI: OPENAI_API_KEY
   - DeepSeek: DEEPSEEK_API_KEY
   - Claude: ANTHROPIC_API_KEY
4. Input your project path.
5. ASI runs automatically:
   - .\bin\asi.exe repl --provider <selected_provider> --project <your_project_path> --no-setup

Manual commands:
- .\asi.exe
- .\asi.cmd version
- .\asi.cmd repl --provider deepseek --project D:\test_code --no-setup
- .\asi.cmd repl --provider openai --project D:\test_code --no-setup
- .\asi.cmd repl --provider claude --project D:\test_code --no-setup

## Uninstall

- powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\ASI Code\uninstall_asi.ps1"
"@
$packageReadme | Set-Content -Path (Join-Path $stageRoot "README_WINDOWS.md")

if (Test-Path -LiteralPath $zipPath) {
    Remove-Item -LiteralPath $zipPath -Force
}
Compress-Archive -Path (Join-Path $stageRoot "*") -DestinationPath $zipPath -CompressionLevel Optimal
$zipHash = Get-FileHash -LiteralPath $zipPath -Algorithm SHA256
"$($zipHash.Hash)  $([System.IO.Path]::GetFileName($zipPath))" | Set-Content -Path ($zipPath + ".sha256.txt")

$installerHash = $null
if (-not $SkipInstallerExe) {
    if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) {
        throw "rustc not found. Cannot build one-click installer exe."
    }

    $stubPath = Join-Path $stageRoot "installer_stub.rs"
    $stubCode = @"
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const ASI_EXE: &[u8] = include_bytes!(r"bin\\asi.exe");
const INSTALL_PS1: &str = include_str!(r"scripts\\windows\\install_asi.ps1");
const UNINSTALL_PS1: &str = include_str!(r"scripts\\windows\\uninstall_asi.ps1");

fn fail(msg: &str) -> ! {
    eprintln!("{}", msg);
    std::process::exit(1);
}

fn write_file(path: &PathBuf, data: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| fail(&format!("create dir failed {}: {}", parent.display(), e)));
    }
    fs::write(path, data).unwrap_or_else(|e| fail(&format!("write file failed {}: {}", path.display(), e)));
}

fn main() {
    let install_dir = env::args().nth(1);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let temp_root = env::temp_dir().join(format!("asi-installer-{}", stamp));

    let source_exe = temp_root.join("bin").join("asi.exe");
    let install_script = temp_root.join("scripts").join("windows").join("install_asi.ps1");
    let uninstall_script = temp_root.join("scripts").join("windows").join("uninstall_asi.ps1");

    write_file(&source_exe, ASI_EXE);
    write_file(&install_script, INSTALL_PS1.as_bytes());
    write_file(&uninstall_script, UNINSTALL_PS1.as_bytes());

    let mut cmd = Command::new("powershell");
    cmd.arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(&install_script)
        .arg("-SourceExe")
        .arg(&source_exe)
        .arg("-Force");

    if let Some(dir) = install_dir {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            cmd.arg("-InstallDir").arg(trimmed);
        }
    }

    let status = cmd
        .status()
        .unwrap_or_else(|e| fail(&format!("failed to execute installer script: {}", e)));

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    println!("ASI Code install completed.");
}
"@
    $stubCode | Set-Content -Path $stubPath

    Push-Location $stageRoot
    try {
        & rustc --edition=2021 -O -C debuginfo=0 installer_stub.rs -o $installerPath
        if ($LASTEXITCODE -ne 0) {
            throw "rustc failed to build one-click installer"
        }
    } finally {
        Pop-Location
    }

    Remove-Item -LiteralPath $stubPath -Force -ErrorAction SilentlyContinue

    $installerPdbPath = [System.IO.Path]::ChangeExtension($installerPath, ".pdb")
    if (Test-Path -LiteralPath $installerPdbPath) {
        Remove-Item -LiteralPath $installerPdbPath -Force
    }

    Invoke-CodeSign -FilePath $installerPath

    $installerHash = Get-FileHash -LiteralPath $installerPath -Algorithm SHA256
    "$($installerHash.Hash)  $([System.IO.Path]::GetFileName($installerPath))" | Set-Content -Path ($installerPath + ".sha256.txt")
}

Write-Host ""
Write-Host "Package complete"
Write-Host "  stage:       $stageRoot"
Write-Host "  archive:     $zipPath"
Write-Host "  zip sha256:  $($zipHash.Hash)"
if ($installerHash -ne $null) {
    Write-Host "  installer:   $installerPath"
    Write-Host "  exe sha256:  $($installerHash.Hash)"
} else {
    Write-Host "  installer:   skipped (-SkipInstallerExe)"
}













