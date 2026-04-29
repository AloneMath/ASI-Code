# Windows Standalone Install Guide

This guide installs ASI Code as a standalone command-line app (`asi.exe`) without npm/pnpm.

## 1) One-Click Installer (`.exe`, Recommended)

After packaging, run the generated installer:

```powershell
.\dist\asi-code-installer-<version>.exe
```

Optional custom install directory:

```powershell
.\dist\asi-code-installer-<version>.exe "D:\Apps\ASI Code"
```

Installer behavior:
- Installs `asi.exe` under `%LOCALAPPDATA%\Programs\ASI Code\bin` (or custom directory).
- Adds user PATH entry (unless using custom script flags).
- Creates Start Menu entries:
  - `ASI Code`
  - `ASI Code (No Setup)`
  - `Uninstall ASI Code`

## 2) Install from Source Repo (PowerShell Script)

From `D:\Code\Rust`:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\windows\install_asi.ps1 -BuildRelease
```

Optional parameters:

```powershell
# Install an existing exe directly
powershell -ExecutionPolicy Bypass -File .\scripts\windows\install_asi.ps1 -SourceExe .\target\release\asi.exe

# Skip PATH update
powershell -ExecutionPolicy Bypass -File .\scripts\windows\install_asi.ps1 -BuildRelease -NoPathUpdate

# Skip Start Menu entries
powershell -ExecutionPolicy Bypass -File .\scripts\windows\install_asi.ps1 -BuildRelease -NoStartMenu
```

## 3) Verify

```powershell
asi version
asi repl --project D:\test_code --provider deepseek --no-setup
```

## 4) Build Distributable Release Artifacts

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\windows\package_release.ps1
```

Output files:
- `dist\asi-code-installer-<version>.exe`
- `dist\asi-code-installer-<version>.exe.sha256.txt`
- `dist\asi-code-windows-x64-<version>.zip`
- `dist\asi-code-windows-x64-<version>.zip.sha256.txt`

Portable ZIP launch entries (after extraction):
- `asi.exe` (double-click portable launcher, recommended)
- `ASI Launcher.exe` (same launcher name for clarity)
- `start_asi.cmd` (script launcher)
- `bin\asi.exe` (CLI engine binary)


## 5) Self-Update

Use CLI self-update with local zip/exe or remote URL:

```powershell
asi self-update --source "D:\Code\Rust\dist\asi-code-windows-x64-0.3.0.zip"
asi self-update --source "https://example.com/asi-code-windows-x64-0.3.0.zip" --sha256 "<sha256>"
```

Optional restart into REPL after update:

```powershell
asi self-update --source "D:\Code\Rust\dist\asi-code-windows-x64-0.3.0.zip" --restart
```

## 6) Uninstall

```powershell
powershell -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\ASI Code\uninstall_asi.ps1"
```

Or from repo:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\windows\uninstall_asi.ps1
```

## 7) Smoke Regression (Windows)

After install/update, run strict smoke regression from repo root.

Recommended quick wrapper:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_strict_quick.ps1 `
  -Provider deepseek `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>"
```

Unified wrapper (strict/risk/gateway):

```powershell
# strict
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke.ps1 `
  -Mode strict `
  -Provider deepseek `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>"

# risk review + gateway
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke.ps1 `
  -Mode risk `
  -Provider deepseek `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>"

# gateway only path
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke.ps1 `
  -Mode gateway `
  -Provider openai `
  -OpenAiApiKey "<YOUR_OPENAI_KEY>" `
  -NoQuick
```

Equivalent explicit recipe command:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\run_smoke_recipes.ps1 `
  -Provider deepseek `
  -Recipe smoke-all-strict `
  -AsiExe ".\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -Repo "D:\Code\rustbpe" `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>" `
  -ReportDir ".\artifacts\strict_ci" `
  -RenderSummary
```

Strict recipe behavior:
- `smoke-all-strict` forwards `-StrictProfile` to `smoke_all.ps1`.
- The following flags are rejected in strict mode:
  - `-SkipHookMatrix`
  - `-SkipHooksCliAdvanced`
  - `-SkipSubagent`

Direct strict-profile smoke_all example:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\smoke_all.ps1 `
  -AsiExe ".\target\release\asi.exe" `
  -Project "D:\Code\Rust" `
  -Repo "D:\Code\rustbpe" `
  -StrictProfile `
  -SkipApiCompat -SkipProviderModel -SkipTokenizer -SkipCheckpoint -SkipGateway `
  -ReportJsonPath ".\artifacts\smoke_all_strict_profile.json"
```

Useful options:
- `-DryRun` on `run_smoke_recipes.ps1` prints the fully expanded command.
- `-StrictCi` forces recipe `smoke-all-strict` and defaults report output to `.\artifacts\strict_ci` when omitted.

## 8) Machine-Readable CI JSON (Optional)

Single-command Windows CI report (build + test + smoke + summary):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\ci_machine_json.ps1 `
  -Provider deepseek `
  -DeepSeekApiKey "<YOUR_DEEPSEEK_KEY>"
```

Primary output:
- `.\artifacts\ci_machine_json\ci_machine_report.json`
