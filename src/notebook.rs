use serde_json::{json, Value};
use std::fs;
use std::path::Path;

pub fn add_markdown_cell(path: &str, text: &str) -> Result<String, String> {
    let p = Path::new(path);
    let mut doc: Value = if p.exists() {
        let content = fs::read_to_string(p).map_err(|e| e.to_string())?;
        serde_json::from_str(&content).map_err(|e| e.to_string())?
    } else {
        json!({
            "cells": [],
            "metadata": {},
            "nbformat": 4,
            "nbformat_minor": 5
        })
    };

    if !doc.get("cells").map(|v| v.is_array()).unwrap_or(false) {
        doc["cells"] = json!([]);
    }

    let cell = json!({
        "cell_type": "markdown",
        "metadata": {},
        "source": [format!("{}\n", text)]
    });

    doc["cells"].as_array_mut().expect("cells array").push(cell);

    let body = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    fs::write(p, body).map_err(|e| e.to_string())?;

    Ok(format!("Notebook updated: {}", p.display()))
}

pub fn list_cells(path: &str) -> Result<String, String> {
    let p = Path::new(path);
    let content = fs::read_to_string(p).map_err(|e| e.to_string())?;
    let doc: Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    let Some(cells) = doc.get("cells").and_then(|v| v.as_array()) else {
        return Ok("No cells".to_string());
    };

    let mut out = Vec::new();
    for (idx, cell) in cells.iter().enumerate() {
        let kind = cell
            .get("cell_type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let preview = cell
            .get("source")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .replace('\n', " ");
        out.push(format!("{}: {} - {}", idx, kind, preview));
    }

    Ok(out.join("\n"))
}
