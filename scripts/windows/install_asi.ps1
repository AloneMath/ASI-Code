param(
    [string]$SourceExe,
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\ASI Code",
    [switch]$BuildRelease,
    [switch]$NoPathUpdate,
    [switch]$NoStartMenu,
    [switch]$Force
)

$ErrorActionPreference = "Stop"

function Normalize-PathForCompare {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) {
        return ""
    }

    $expanded = [Environment]::ExpandEnvironmentVariables($Value.Trim())
    try {
        $full = [System.IO.Path]::GetFullPath($expanded)
        return $full.TrimEnd('\\').ToLowerInvariant()
    } catch {
        return $expanded.TrimEnd('\\').ToLowerInvariant()
    }
}

function Resolve-FullPath {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) {
        throw "path is empty"
    }

    $expanded = [Environment]::ExpandEnvironmentVariables($Value.Trim())
    try {
        return (Resolve-Path -LiteralPath $expanded -ErrorAction Stop).Path
    } catch {
        return [System.IO.Path]::GetFullPath($expanded)
    }
}

function PathList-Contains {
    param(
        [string[]]$List,
        [string]$Entry
    )

    $normalizedEntry = Normalize-PathForCompare $Entry
    if ([string]::IsNullOrWhiteSpace($normalizedEntry)) {
        return $false
    }

    foreach ($item in $List) {
        if ((Normalize-PathForCompare $item) -eq $normalizedEntry) {
            return $true
        }
    }
    return $false
}

function Add-UserPathEntry {
    param([string]$Entry)

    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    $items = @()
    if (-not [string]::IsNullOrWhiteSpace($current)) {
        $items = $current.Split(';') | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    }

    if (PathList-Contains -List $items -Entry $Entry) {
        return $false
    }

    $updated = @($items + $Entry) -join ';'
    [Environment]::SetEnvironmentVariable("Path", $updated, "User")
    return $true
}

function New-Shortcut {
    param(
        [string]$Path,
        [string]$TargetPath,
        [string]$Arguments = "",
        [string]$WorkingDirectory = "",
        [string]$Description = "",
        [string]$IconLocation = ""
    )

    $parent = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }

    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($Path)
    $shortcut.TargetPath = $TargetPath
    if (-not [string]::IsNullOrWhiteSpace($Arguments)) {
        $shortcut.Arguments = $Arguments
    }
    if (-not [string]::IsNullOrWhiteSpace($WorkingDirectory)) {
        $shortcut.WorkingDirectory = $WorkingDirectory
    }
    if (-not [string]::IsNullOrWhiteSpace($Description)) {
        $shortcut.Description = $Description
    }
    if (-not [string]::IsNullOrWhiteSpace($IconLocation)) {
        $shortcut.IconLocation = $IconLocation
    }
    $shortcut.Save()
}

function Set-StartMenuEntries {
    param(
        [string]$InstallDir,
        [string]$TargetExe
    )

    $menuDir = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\ASI Code"
    New-Item -ItemType Directory -Path $menuDir -Force | Out-Null

    $workDir = Split-Path -Parent $TargetExe
    $mainShortcut = Join-Path $menuDir "ASI Code.lnk"
    New-Shortcut -Path $mainShortcut -TargetPath $TargetExe -WorkingDirectory $workDir -Description "ASI Code Terminal App" -IconLocation "$TargetExe,0"

    $noSetupShortcut = Join-Path $menuDir "ASI Code (No Setup).lnk"
    New-Shortcut -Path $noSetupShortcut -TargetPath $TargetExe -Arguments "repl --no-setup" -WorkingDirectory $workDir -Description "ASI Code CLI Agent (Skip setup)" -IconLocation "$TargetExe,0"

    $uninstallScript = Join-Path $InstallDir "uninstall_asi.ps1"
    if (Test-Path -LiteralPath $uninstallScript) {
        $psExe = (Get-Command powershell -ErrorAction SilentlyContinue).Source
        if (-not [string]::IsNullOrWhiteSpace($psExe)) {
            $uninstallShortcut = Join-Path $menuDir "Uninstall ASI Code.lnk"
            $uninstallArgs = "-ExecutionPolicy Bypass -File `"$uninstallScript`""
            New-Shortcut -Path $uninstallShortcut -TargetPath $psExe -Arguments $uninstallArgs -WorkingDirectory $InstallDir -Description "Uninstall ASI Code"
        }
    }

    return $menuDir
}

$repoRoot = Resolve-FullPath (Join-Path $PSScriptRoot "..\..")

if ([string]::IsNullOrWhiteSpace($SourceExe)) {
    $candidate = Join-Path $repoRoot "target\release\asi.exe"
    if ($BuildRelease -or -not (Test-Path -LiteralPath $candidate)) {
        if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
            throw "cargo not found. Install Rust from https://rustup.rs or provide -SourceExe."
        }

        Push-Location $repoRoot
        try {
            Write-Host "Building release binary..."
            & cargo build --release
            if ($LASTEXITCODE -ne 0) {
                throw "cargo build --release failed with exit code $LASTEXITCODE"
            }
        } finally {
            Pop-Location
        }
    }
    $SourceExe = $candidate
}

$SourceExe = Resolve-FullPath $SourceExe
if (-not (Test-Path -LiteralPath $SourceExe)) {
    throw "Source exe not found: $SourceExe"
}

$InstallDir = Resolve-FullPath $InstallDir
$binDir = Join-Path $InstallDir "bin"
New-Item -ItemType Directory -Path $binDir -Force | Out-Null

$targetExe = Join-Path $binDir "asi.exe"
if ((Test-Path -LiteralPath $targetExe) -and -not $Force) {
    Write-Host "Existing install found; overwriting asi.exe (use -Force to make this explicit)."
}
Copy-Item -LiteralPath $SourceExe -Destination $targetExe -Force

$uninstallSource = Join-Path $PSScriptRoot "uninstall_asi.ps1"
if (Test-Path -LiteralPath $uninstallSource) {
    Copy-Item -LiteralPath $uninstallSource -Destination (Join-Path $InstallDir "uninstall_asi.ps1") -Force
}

$startMenuDir = ""
if (-not $NoStartMenu) {
    $startMenuDir = Set-StartMenuEntries -InstallDir $InstallDir -TargetExe $targetExe
}

$versionText = "unknown"
try {
    $line = & $targetExe version 2>$null | Select-Object -First 1
    if (-not [string]::IsNullOrWhiteSpace($line)) {
        $versionText = $line.Trim()
    }
} catch {
}

$metadata = [ordered]@{
    app = "ASI Code"
    installed_at = (Get-Date).ToString("s")
    install_dir = $InstallDir
    bin = $targetExe
    source = $SourceExe
    version = $versionText
    start_menu = $startMenuDir
}
$metadata | ConvertTo-Json -Depth 4 | Set-Content -Path (Join-Path $InstallDir "install.json")

$pathChanged = $false
if (-not $NoPathUpdate) {
    $pathChanged = Add-UserPathEntry -Entry $binDir
    if ($pathChanged) {
        if (-not (PathList-Contains -List ($env:Path -split ';') -Entry $binDir)) {
            $env:Path = "$binDir;$env:Path"
        }
    }
}

Write-Host ""
Write-Host "Install complete"
Write-Host "  app:      ASI Code"
Write-Host "  version:  $versionText"
Write-Host "  install:  $InstallDir"
Write-Host "  binary:   $targetExe"
if ($NoPathUpdate) {
    Write-Host "  PATH:     skipped (-NoPathUpdate)"
} elseif ($pathChanged) {
    Write-Host "  PATH:     updated (User scope)"
    Write-Host "            open a new terminal to use 'asi'"
} else {
    Write-Host "  PATH:     already contains $binDir"
}
if ($NoStartMenu) {
    Write-Host "  StartMenu: skipped (-NoStartMenu)"
} else {
    Write-Host "  StartMenu: $startMenuDir"
}
Write-Host ""
Write-Host "Try: asi version"
Write-Host "Then: asi repl --project D:\\test_code --provider deepseek --no-setup"

