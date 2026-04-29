use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run_self_update(
    source: &str,
    expected_sha256: Option<&str>,
    restart: bool,
) -> Result<String, String> {
    if !cfg!(target_os = "windows") {
        return Err("self-update currently supports Windows only".to_string());
    }

    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err("source is empty; use --source <file-or-url>".to_string());
    }

    let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let temp_root = std::env::temp_dir().join(format!("asi-self-update-{}", stamp));
    fs::create_dir_all(&temp_root).map_err(|e| e.to_string())?;

    let package_path = resolve_package_source(trimmed, &temp_root)?;
    if let Some(expected) = expected_sha256 {
        verify_sha256(&package_path, expected)?;
    }

    let staged_exe = temp_root.join("asi.new.exe");
    stage_update_exe(&package_path, &temp_root, &staged_exe)?;
    validate_candidate_exe(&staged_exe)?;

    let apply_script = temp_root.join("apply_update.ps1");
    fs::write(&apply_script, build_apply_script()).map_err(|e| e.to_string())?;

    let mut cmd = Command::new("powershell");
    cmd.arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(&apply_script)
        .arg("-TargetExe")
        .arg(&current_exe)
        .arg("-NewExe")
        .arg(&staged_exe)
        .arg("-ParentPid")
        .arg(std::process::id().to_string());
    if restart {
        cmd.arg("-Restart");
    }

    cmd.spawn()
        .map_err(|e| format!("failed to start updater process: {}", e))?;

    Ok(format!(
        "self-update scheduled from={} target={} restart={}. This process can now exit.",
        package_path.display(),
        current_exe.display(),
        restart
    ))
}

fn resolve_package_source(source: &str, temp_root: &Path) -> Result<PathBuf, String> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let bytes = Client::new()
            .get(source)
            .send()
            .map_err(|e| format!("download failed: {}", e))?
            .error_for_status()
            .map_err(|e| format!("download http error: {}", e))?
            .bytes()
            .map_err(|e| format!("download read failed: {}", e))?;

        let ext = infer_extension_from_url(source);
        let out = temp_root.join(format!("package{}", ext));
        let mut file = File::create(&out).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        return Ok(out);
    }

    let candidate = PathBuf::from(source);
    let resolved = if candidate.is_absolute() {
        candidate
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(candidate)
    };
    if !resolved.exists() {
        return Err(format!("source not found: {}", resolved.display()));
    }
    Ok(resolved)
}

fn infer_extension_from_url(url: &str) -> &'static str {
    let lower = url.to_ascii_lowercase();
    if lower.contains(".zip") {
        ".zip"
    } else if lower.contains(".exe") {
        ".exe"
    } else {
        ".bin"
    }
}

fn stage_update_exe(
    package_path: &Path,
    temp_root: &Path,
    staged_exe: &Path,
) -> Result<(), String> {
    let ext = package_path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "exe" => {
            fs::copy(package_path, staged_exe).map_err(|e| e.to_string())?;
            Ok(())
        }
        "zip" => {
            let extract_dir = temp_root.join("extracted");
            if extract_dir.exists() {
                fs::remove_dir_all(&extract_dir).map_err(|e| e.to_string())?;
            }
            fs::create_dir_all(&extract_dir).map_err(|e| e.to_string())?;
            extract_zip_with_powershell(package_path, &extract_dir)?;
            let candidate = find_asi_exe(&extract_dir)?
                .ok_or_else(|| "could not find asi.exe in zip package".to_string())?;
            fs::copy(candidate, staged_exe).map_err(|e| e.to_string())?;
            Ok(())
        }
        _ => Err(format!(
            "unsupported package type: {} (supported: .exe, .zip)",
            package_path.display()
        )),
    }
}

fn extract_zip_with_powershell(package_path: &Path, extract_dir: &Path) -> Result<(), String> {
    let zip = ps_single_quote(package_path.as_os_str());
    let dest = ps_single_quote(extract_dir.as_os_str());
    let command = format!(
        "Expand-Archive -LiteralPath '{}' -DestinationPath '{}' -Force",
        zip, dest
    );

    let status = Command::new("powershell")
        .arg("-NoProfile")
        .arg("-Command")
        .arg(command)
        .status()
        .map_err(|e| format!("failed to run powershell Expand-Archive: {}", e))?;
    if !status.success() {
        return Err(format!(
            "Expand-Archive failed for {}",
            package_path.display()
        ));
    }
    Ok(())
}

fn ps_single_quote(value: &OsStr) -> String {
    let s = value.to_string_lossy();
    s.replace('\'', "''")
}

fn find_asi_exe(root: &Path) -> Result<Option<PathBuf>, String> {
    let preferred = root.join("bin").join("asi.exe");
    if preferred.exists() {
        return Ok(Some(preferred));
    }
    let direct = root.join("asi.exe");
    if direct.exists() {
        return Ok(Some(direct));
    }
    walk_find_asi_exe(root)
}

fn walk_find_asi_exe(dir: &Path) -> Result<Option<PathBuf>, String> {
    let rd = fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in rd {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = walk_find_asi_exe(&path)? {
                return Ok(Some(found));
            }
            continue;
        }
        if path
            .file_name()
            .and_then(OsStr::to_str)
            .map(|n| n.eq_ignore_ascii_case("asi.exe"))
            .unwrap_or(false)
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let want = normalize_sha256(expected)?;
    let got = sha256_file(path)?;
    if got != want {
        return Err(format!(
            "sha256 mismatch for {}: expected {} got {}",
            path.display(),
            want,
            got
        ));
    }
    Ok(())
}

fn normalize_sha256(raw: &str) -> Result<String, String> {
    let normalized: String = raw
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if normalized.len() != 64 {
        return Err("sha256 must contain 64 hex chars".to_string());
    }
    Ok(normalized)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_candidate_exe(path: &Path) -> Result<(), String> {
    let output = Command::new(path)
        .arg("version")
        .output()
        .map_err(|e| format!("failed to execute candidate binary: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "candidate binary validation failed (exit={})",
            output.status.code().unwrap_or(-1)
        ));
    }
    Ok(())
}

fn build_apply_script() -> &'static str {
    r#"param(
    [string]$TargetExe,
    [string]$NewExe,
    [int]$ParentPid,
    [switch]$Restart
)

$ErrorActionPreference = "Stop"

# Wait for current ASI process to exit.
for ($i = 0; $i -lt 300; $i++) {
    try {
        Get-Process -Id $ParentPid -ErrorAction Stop | Out-Null
        Start-Sleep -Milliseconds 120
    } catch {
        break
    }
}

$copied = $false
for ($i = 0; $i -lt 120; $i++) {
    try {
        Copy-Item -LiteralPath $NewExe -Destination $TargetExe -Force
        $copied = $true
        break
    } catch {
        Start-Sleep -Milliseconds 250
    }
}

if (-not $copied) {
    Write-Error "failed to replace target binary: $TargetExe"
    exit 1
}

if ($Restart) {
    Start-Process -FilePath $TargetExe -ArgumentList @("repl") -WorkingDirectory (Split-Path -Parent $TargetExe) | Out-Null
}
"#
}

#[cfg(test)]
mod tests {
    use super::normalize_sha256;

    #[test]
    fn normalize_sha256_accepts_upper_and_spaces() {
        let s = normalize_sha256("AA BB CC DD EE FF 00 11 22 33 44 55 66 77 88 99 AA BB CC DD EE FF 00 11 22 33 44 55 66 77 88 99").unwrap();
        assert_eq!(s.len(), 64);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn normalize_sha256_rejects_short() {
        let err = normalize_sha256("1234").unwrap_err();
        assert!(err.contains("64"));
    }
}
