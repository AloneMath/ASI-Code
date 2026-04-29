param(
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\ASI Code",
    [switch]$KeepFiles
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

function Remove-UserPathEntry {
    param([string]$Entry)

    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    if ([string]::IsNullOrWhiteSpace($current)) {
        return $false
    }

    $normalizedEntry = Normalize-PathForCompare $Entry
    $items = $current.Split(';') | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    $kept = @()
    $removed = $false

    foreach ($item in $items) {
        if ((Normalize-PathForCompare $item) -eq $normalizedEntry) {
            $removed = $true
            continue
        }
        $kept += $item
    }

    if ($removed) {
        [Environment]::SetEnvironmentVariable("Path", ($kept -join ';'), "User")
    }

    return $removed
}

$InstallDir = Resolve-FullPath $InstallDir
$binDir = Join-Path $InstallDir "bin"
$startMenuDir = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\ASI Code"

$pathRemoved = Remove-UserPathEntry -Entry $binDir

$startMenuRemoved = $false
if (Test-Path -LiteralPath $startMenuDir) {
    Remove-Item -LiteralPath $startMenuDir -Recurse -Force
    $startMenuRemoved = $true
}

if (-not $KeepFiles -and (Test-Path -LiteralPath $InstallDir)) {
    Remove-Item -LiteralPath $InstallDir -Recurse -Force
}

Write-Host ""
Write-Host "Uninstall complete"
Write-Host "  install: $InstallDir"
if ($pathRemoved) {
    Write-Host "  PATH:    removed $binDir from User path"
} else {
    Write-Host "  PATH:    no change"
}
if ($startMenuRemoved) {
    Write-Host "  StartMenu: removed $startMenuDir"
} else {
    Write-Host "  StartMenu: no change"
}
if ($KeepFiles) {
    Write-Host "  files:   kept (-KeepFiles)"
} else {
    Write-Host "  files:   removed"
}
Write-Host ""
Write-Host "Open a new terminal to refresh PATH."
