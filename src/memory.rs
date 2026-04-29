use std::fs;
use std::path::PathBuf;

fn memory_path() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let claw = cwd.join("CLAW.md");
    if claw.exists() {
        return claw;
    }
    cwd.join("ASI_CODE.md")
}

pub fn read_memory() -> Result<String, String> {
    let path = memory_path();
    if !path.exists() {
        return Ok("(memory file does not exist yet)".to_string());
    }
    fs::read_to_string(path).map_err(|e| e.to_string())
}

pub fn append_memory(note: &str) -> Result<PathBuf, String> {
    let path = memory_path();
    let existing = if path.exists() {
        fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    };
    let mut next = existing;
    if !next.ends_with('\n') && !next.is_empty() {
        next.push('\n');
    }
    next.push_str("- ");
    next.push_str(note.trim());
    next.push('\n');
    fs::write(&path, next).map_err(|e| e.to_string())?;
    Ok(path)
}
