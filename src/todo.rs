use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: u64,
    pub text: String,
    pub done: bool,
}

fn todo_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".asi_todos.json")
}

fn load_all() -> Vec<TodoItem> {
    let path = todo_path();
    let text = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_all(items: &[TodoItem]) -> Result<(), String> {
    let path = todo_path();
    let body = serde_json::to_string_pretty(items).map_err(|e| e.to_string())?;
    fs::write(path, body).map_err(|e| e.to_string())
}

pub fn add(text: &str) -> Result<TodoItem, String> {
    let mut items = load_all();
    let next_id = items.iter().map(|x| x.id).max().unwrap_or(0) + 1;
    let item = TodoItem {
        id: next_id,
        text: text.trim().to_string(),
        done: false,
    };
    items.push(item.clone());
    save_all(&items)?;
    Ok(item)
}

pub fn list() -> Vec<TodoItem> {
    load_all()
}

pub fn mark_done(id: u64) -> Result<bool, String> {
    let mut items = load_all();
    let mut changed = false;
    for item in &mut items {
        if item.id == id {
            item.done = true;
            changed = true;
        }
    }
    save_all(&items)?;
    Ok(changed)
}

pub fn remove(id: u64) -> Result<bool, String> {
    let mut items = load_all();
    let before = items.len();
    items.retain(|x| x.id != id);
    let changed = items.len() != before;
    save_all(&items)?;
    Ok(changed)
}
