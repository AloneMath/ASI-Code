use crate::config::{normalize_provider_name, resolve_model_alias};
use crate::oauth;
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT_ENCODING, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::io::{BufRead, BufReader};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    /// Native tool calls attached to an assistant message, stored in the
    /// OpenAI wire shape so history replays the exact protocol the model
    /// was trained on. Empty for plain text messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallSpec>,
    /// For role="tool" messages: the id of the assistant tool call this
    /// result answers. None for all other roles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Thinking-mode providers may require this to be replayed after a tool
    /// call. Older history files omit it, so it remains optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl ChatMessage {
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant_with_tool_calls_and_reasoning(
        content: impl Into<String>,
        tool_calls: Vec<ToolCallSpec>,
        reasoning_content: impl Into<String>,
    ) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_calls,
            tool_call_id: None,
            reasoning_content: Some(reasoning_content.into()),
        }
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            reasoning_content: None,
        }
    }
}

/// A tool call in the OpenAI wire shape, as stored in conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSpec {
    pub id: String,
    #[serde(rename = "type", default = "default_tool_call_type")]
    pub call_type: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

fn default_tool_call_type() -> String {
    "function".to_string()
}

impl ToolCallSpec {
    pub fn from_call(call: &ToolCall) -> Self {
        Self {
            id: call.id.clone(),
            call_type: "function".to_string(),
            function: ToolCallFunction {
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            },
        }
    }
}

/// Result from a text streaming request.
#[derive(Debug, Clone)]
pub struct StreamingResult {
    pub text: String,
    pub stop_reason: String,
}

/// A structured tool call parsed from the API response.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Result from a completion request — may contain text and/or tool calls.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CompletionResult {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: String,
    pub reasoning_content: String,
}

pub trait ChatProvider: std::fmt::Debug + Send + Sync {
    fn complete_streaming_dyn(
        &self,
        messages: &[ChatMessage],
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<StreamingResult, String>;

    fn complete_with_tools_dyn(
        &self,
        messages: &[ChatMessage],
        tool_results: &[(String, String)],
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<CompletionResult, String>;
}

#[derive(Debug)]
pub struct ProviderClient {
    pub provider: String,
    pub model: String,
    http: Client,
}

/// Return a safe max_tokens value for the given provider.
/// These values are conservative defaults that work for most models.
/// The API will cap to the model's actual context limit if lower.
fn max_tokens_for_provider(provider: &str) -> u64 {
    match provider {
        "deepseek" => 4096, // DeepSeek is stricter on max_tokens; keep conservative to avoid 400 errors
        "claude" => 32768,  // Claude models support large contexts (200k), 32k is reasonable
        _ => 16384,         // OpenAI-compatible defaults (GPT-4, etc.)
    }
}

fn should_enable_deepseek_reasoning_effort(provider: &str, model: &str) -> bool {
    if provider != "deepseek" {
        return false;
    }
    let m = model.to_ascii_lowercase();
    m.contains("reasoner") || m.contains("v4-pro")
}

fn deepseek_reasoning_effort_value() -> Option<String> {
    let raw = env::var("ASI_DEEPSEEK_REASONING_EFFORT")
        .ok()
        .unwrap_or_else(|| "max".to_string());
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() || normalized == "off" || normalized == "none" {
        return None;
    }
    let mapped = match normalized.as_str() {
        "max" | "high" | "medium" | "low" => normalized,
        "true" | "on" => "max".to_string(),
        _ => normalized,
    };
    Some(mapped)
}

/// JSON tool definitions for native function calling.

fn http_error_with_body(label: &str, response: Response) -> String {
    let status = response.status();
    let body = response.text().unwrap_or_default();
    let snippet: String = body.chars().take(800).collect();
    if snippet.is_empty() {
        format!("{} HTTP error: {}", label, status)
    } else {
        format!("{} HTTP error: {} | body: {}", label, status, snippet)
    }
}

fn ensure_success_response(label: &str, response: Response) -> Result<Response, String> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(http_error_with_body(label, response))
    }
}

fn configured_max_tokens(provider: &str) -> u64 {
    let key = match provider {
        "deepseek" => "ASI_MAX_TOKENS_DEEPSEEK",
        "claude" => "ASI_MAX_TOKENS_CLAUDE",
        _ => "ASI_MAX_TOKENS_OPENAI",
    };
    let default_value = max_tokens_for_provider(provider);
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|v| v.clamp(256, 128_000))
        .unwrap_or(default_value)
}

/// Newer OpenAI reasoning models (for example GPT-5 and the o-series) use
/// `max_completion_tokens` and reject sampling parameters such as
/// `temperature`.  Keep this decision model-aware instead of tying it to a
/// short, fast-changing allow-list in the CLI.
fn uses_modern_openai_parameters(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    m.starts_with("gpt-5")
        || m.starts_with("o1")
        || m.starts_with("o3")
        || m.starts_with("o4")
        || m.contains("codex")
}

fn apply_openai_model_parameters(payload: &mut Value, model: &str, provider: &str) {
    if provider == "deepseek" || !uses_modern_openai_parameters(model) {
        payload["max_tokens"] = Value::from(configured_max_tokens(provider));
        return;
    }

    let max_tokens = configured_max_tokens(provider);
    if let Some(object) = payload.as_object_mut() {
        object.remove("max_tokens");
        object.remove("temperature");
        object.insert("max_completion_tokens".to_string(), Value::from(max_tokens));
    }
}

fn should_retry_openai_parameter_variant(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    if !(lower.contains("http error: 400")
        || lower.contains("status: 400")
        || lower.contains("invalid_request_error"))
    {
        return false;
    }
    [
        "max_tokens",
        "max_completion_tokens",
        "temperature",
        "reasoning_effort",
        "stream",
    ]
    .iter()
    .any(|field| lower.contains(field))
        && [
            "unsupported",
            "not support",
            "not allowed",
            "unknown parameter",
            "unrecognized",
            "invalid",
            "extra inputs",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
}

fn flip_openai_parameter_variant(payload: &mut Value, error: &str) {
    if let Some(object) = payload.as_object_mut() {
        let lower = error.to_ascii_lowercase();
        if lower.contains("max_completion_tokens") || lower.contains("max_tokens") {
            let (max_key, max) = if object.contains_key("max_completion_tokens") {
                (
                    "max_completion_tokens",
                    object.remove("max_completion_tokens"),
                )
            } else {
                ("max_tokens", object.remove("max_tokens"))
            };
            if let Some(value) = max {
                let key = if max_key == "max_completion_tokens" {
                    "max_tokens"
                } else {
                    "max_completion_tokens"
                };
                object.insert(key.to_string(), value);
            }
        }
        if lower.contains("temperature") {
            object.remove("temperature");
        }
        if lower.contains("reasoning_effort") {
            object.remove("reasoning_effort");
        }
        if lower.contains("stream") && object.get("stream").and_then(Value::as_bool) == Some(true) {
            object.insert("stream".to_string(), Value::Bool(false));
        }
    }
}

fn parse_openai_json_response(raw_json: &str) -> Result<Value, String> {
    let value = serde_json::from_str::<Value>(raw_json)
        .map_err(|e| format!("invalid OpenAI-compatible response JSON: {}", e))?;
    if value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .is_none()
    {
        return Err("OpenAI-compatible response did not contain choices[0]".to_string());
    }
    Ok(value)
}

fn tool_arguments_string(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(value) if !value.is_null() => value.to_string(),
        _ => "{}".to_string(),
    }
}

fn append_tool_call_value(
    tc_index_map: &mut std::collections::HashMap<usize, (String, String, String)>,
    tc: &Value,
    fallback_index: usize,
) {
    let idx = tc
        .get("index")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(fallback_index);
    let function = tc.get("function").unwrap_or(tc);
    let entry = tc_index_map.entry(idx).or_insert_with(|| {
        let id = tc
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        (id, name, String::new())
    });
    if let Some(id) = tc.get("id").and_then(Value::as_str) {
        if !id.is_empty() && entry.0.is_empty() {
            entry.0 = id.to_string();
        }
    }
    if let Some(name) = function.get("name").and_then(Value::as_str) {
        if !name.is_empty() && entry.1.is_empty() {
            entry.1 = name.to_string();
        }
    }
    if let Some(args) = function.get("arguments") {
        if let Some(args_str) = args.as_str() {
            entry.2.push_str(args_str);
        } else if !args.is_null() {
            entry.2.push_str(&args.to_string());
        }
    }
}

fn build_openai_api_messages(
    provider: &str,
    messages: &[ChatMessage],
    tool_results: &[(String, String)],
) -> Vec<Value> {
    let mut api_messages: Vec<Value> = Vec::with_capacity(messages.len() + tool_results.len());
    for m in messages {
        if m.role == "assistant" && !m.tool_calls.is_empty() {
            let content_value = if m.content.is_empty() {
                Value::Null
            } else {
                Value::String(m.content.clone())
            };
            let mut assistant = json!({
                "role": "assistant",
                "content": content_value,
                "tool_calls": m.tool_calls
            });
            if provider == "deepseek" {
                assistant["reasoning_content"] =
                    Value::String(m.reasoning_content.clone().unwrap_or_default());
            }
            api_messages.push(assistant);
        } else if m.role == "tool" {
            api_messages.push(json!({
                "role": "tool",
                "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                "content": m.content
            }));
        } else {
            let mut message = json!({"role": m.role, "content": m.content});
            if provider == "deepseek" && m.role == "assistant" {
                message["reasoning_content"] =
                    Value::String(m.reasoning_content.clone().unwrap_or_default());
            }
            api_messages.push(message);
        }
    }

    for (call_id, result_text) in tool_results {
        api_messages.push(json!({
            "role": "tool",
            "tool_call_id": call_id,
            "content": result_text
        }));
    }
    api_messages
}

pub fn tool_definitions_openai() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read file content. Returns numbered lines. Use start_line and max_lines for large files.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path to read"},
                        "start_line": {"type": "integer", "description": "Starting line number (1-based, optional)"},
                        "max_lines": {"type": "integer", "description": "Maximum lines to read (max 2000, default 300). Use large values for big files."}
                    },
                    "required": ["path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Write content to a file. Creates parent directories if needed. Overwrites existing content.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path to write"},
                        "content": {"type": "string", "description": "Full file content to write"}
                    },
                    "required": ["path", "content"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace exact text in a file. The old_text must exist exactly once. Always read the file first.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path to edit"},
                        "old_text": {"type": "string", "description": "Exact existing text to find and replace"},
                        "new_text": {"type": "string", "description": "Replacement text"}
                    },
                    "required": ["path", "old_text", "new_text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "glob_search",
                "description": "Find files matching a glob pattern. Returns up to 300 matching paths.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "Glob pattern, e.g. src/**/*.rs"}
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "grep_search",
                "description": "Search file contents by regex pattern. Returns matching lines with file paths and line numbers.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "Regular expression pattern"},
                        "base_path": {"type": "string", "description": "Directory to search in (default: current dir)"}
                    },
                    "required": ["pattern"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "bash",
                "description": "Execute a shell command. Use for builds, tests, git, package managers. On Windows use PowerShell syntax (`;` separators, not `&&`); this tool already captures stdout/stderr, so `2>&1` is usually unnecessary.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "description": "Shell command to execute"}
                    },
                    "required": ["command"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "web_search",
                "description": "Search the web using DuckDuckGo.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Search query"}
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "web_fetch",
                "description": "Fetch and return content from a URL.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {"type": "string", "description": "URL to fetch"}
                    },
                    "required": ["url"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "screenshot",
                "description": "Capture the current screen and return base64 PNG data plus dimensions.",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "find_window",
                "description": "Find visible top-level windows whose title contains the query.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Window title query"}
                    },
                    "required": ["title"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "click",
                "description": "Click mouse at absolute screen coordinates.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "x": {"type": "integer", "description": "X coordinate"},
                        "y": {"type": "integer", "description": "Y coordinate"}
                    },
                    "required": ["x", "y"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "click_text",
                "description": "Click UI element by visible text label using Windows UI Automation.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": {"type": "string", "description": "Exact label text"}
                    },
                    "required": ["text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "type_text",
                "description": "Type text into a target window via synthetic keystrokes. IMPORTANT: SendKeys posts to the foreground window, so when targeting a GUI app (Notepad, browser, editor) you MUST pass window_pid (preferred) or window_title so the tool can activate that window first. Without one of those, keystrokes will go to the terminal running asi-code, not your app.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "text": {"type": "string", "description": "Text to type"},
                        "window_pid": {"type": "integer", "description": "PID of the process whose main window should receive keystrokes (preferred; obtain from find_window)"},
                        "window_title": {"type": "string", "description": "Case-insensitive substring of the target window title (used only if window_pid is not provided)"}
                    },
                    "required": ["text"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "read_screen_text",
                "description": "Capture screen and run OCR, returning extracted text.",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "ue5_bridge",
                "description": "Run Python script through Unreal Engine 5 bridge. Uses UnrealEditor command line execution.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "script": {"type": "string", "description": "Python script source to execute"},
                        "project": {"type": "string", "description": "Optional UE project path"}
                    },
                    "required": ["script"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "ue5_scene_probe",
                "description": "Collect structured UE5 level scene state as JSON for deterministic 3D reasoning.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "project": {"type": "string", "description": "UE project path (.uproject) for UnrealEditor-Cmd"},
                        "max_objects": {"type": "integer", "description": "Maximum actors to include in output (default 500)"}
                    },
                    "required": ["project"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "blender_bridge",
                "description": "Run Python script through Blender bridge (`blender --background --python`).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "script": {"type": "string", "description": "Python script source to execute"},
                        "project": {"type": "string", "description": "Optional blend file path (reserved)"}
                    },
                    "required": ["script"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "blender_scene_probe",
                "description": "Collect structured Blender scene state as JSON (objects, transforms, cameras, lights, actions) for deterministic 3D reasoning.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "project": {"type": "string", "description": "Optional .blend path (reserved for future direct-open support)"},
                        "script": {"type": "string", "description": "Optional Python snippet appended after probe collection"}
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "unity_bridge",
                "description": "Drive a Unity project. The `action` field selects one of: `python` (default; runs Python via batchmode `PythonRunner.RunFile` — spawns a fresh headless Unity, will fail if the project is already open in another Editor), `open` (launches the Editor for the project and returns immediately; use this before csharp/create_terrain to make sure the live Editor is running), `csharp` (executes C# inside the ALREADY-RUNNING Editor by dropping a one-shot script into Assets/Editor/ and polling its result — required for interactive edits to the open scene), `create_terrain` (convenience: spawns a Terrain GameObject in the active scene, dispatches via csharp), `save` (saves open scenes + asset database, via csharp).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "project": {"type": "string", "description": "Absolute path to the Unity project root (the folder containing Assets/, Library/, ProjectSettings/)."},
                        "action": {"type": "string", "description": "One of: python | open | csharp | create_terrain | save. Defaults to python.", "enum": ["python", "open", "csharp", "create_terrain", "save"]},
                        "script": {"type": "string", "description": "Source code body. Python source when action=python; C# source when action=csharp (`csharp` field also accepted). Unused for open/save."},
                        "csharp": {"type": "string", "description": "Alias for `script` when action=csharp, for clarity."},
                        "size": {"type": "array", "items": {"type": "number"}, "description": "Terrain size [x,y,z] in world units. Only used by action=create_terrain. Defaults to [500,50,500]."},
                        "heightmap_resolution": {"type": "integer", "description": "Heightmap resolution (33-4097) for create_terrain. Defaults to 513."},
                        "name": {"type": "string", "description": "GameObject name for create_terrain. Defaults to AsiTerrain."}
                    },
                    "required": ["project"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "unity_scene_probe",
                "description": "Collect structured Unity active-scene state as JSON by running C# in the live Editor.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "project": {"type": "string", "description": "Absolute path to Unity project root"},
                        "max_objects": {"type": "integer", "description": "Maximum objects to include (default 1000)"},
                        "include_inactive": {"type": "boolean", "description": "Include inactive objects (default true)"}
                    },
                    "required": ["project"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "probe_diff",
                "description": "Compute deterministic JSON diff between two scene probes (`before` and `after`).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "before": {"type": "object", "description": "Probe JSON before mutation"},
                        "after": {"type": "object", "description": "Probe JSON after mutation"},
                        "numeric_tolerance": {"type": "number", "description": "Optional absolute epsilon for numeric comparisons (default 0.0001)"}
                    },
                    "required": ["before", "after"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "video_capture",
                "description": "Capture frame sequence to a directory (v1 frame-folder approach, no mp4 state).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "duration_sec": {"type": "integer", "description": "Capture duration in seconds (default 6)"},
                        "fps": {"type": "integer", "description": "Sampling FPS (default 2)"},
                        "target": {"type": "string", "description": "Capture target: window or fullscreen", "enum": ["window", "fullscreen"]},
                        "window_title": {"type": "string", "description": "Window-title substring when target=window"},
                        "window_pid": {"type": "integer", "description": "Preferred target window PID when target=window"},
                        "out_dir": {"type": "string", "description": "Optional output directory; auto-generated temp dir when omitted"}
                    },
                    "required": []
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "video_keyframes",
                "description": "Select a compact keyframe set from captured PNG frames in a directory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "dir": {"type": "string", "description": "Frame directory from video_capture"},
                        "max_frames": {"type": "integer", "description": "Maximum selected frames to return (default 12)"}
                    },
                    "required": ["dir"]
                }
            }
        }
    ])
}

/// Tool definitions for Claude API format.
pub fn tool_definitions_claude() -> Value {
    json!([
        {
            "name": "read_file",
            "description": "Read file content. Returns numbered lines. Use start_line and max_lines for large files.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to read"},
                    "start_line": {"type": "integer", "description": "Starting line number (1-based, optional)"},
                    "max_lines": {"type": "integer", "description": "Maximum lines to read (max 2000, default 300). Use large values for big files."}
                },
                "required": ["path"]
            }
        },
        {
            "name": "write_file",
            "description": "Write content to a file. Creates parent directories if needed. Overwrites existing content.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to write"},
                    "content": {"type": "string", "description": "Full file content to write"}
                },
                "required": ["path", "content"]
            }
        },
        {
            "name": "edit_file",
            "description": "Replace exact text in a file. The old_text must exist exactly once. Always read the file first.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to edit"},
                    "old_text": {"type": "string", "description": "Exact existing text to find and replace"},
                    "new_text": {"type": "string", "description": "Replacement text"}
                },
                "required": ["path", "old_text", "new_text"]
            }
        },
        {
            "name": "glob_search",
            "description": "Find files matching a glob pattern. Returns up to 300 matching paths.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Glob pattern, e.g. src/**/*.rs"}
                },
                "required": ["pattern"]
            }
        },
        {
            "name": "grep_search",
            "description": "Search file contents by regex pattern. Returns matching lines with file paths and line numbers.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Regular expression pattern"},
                    "base_path": {"type": "string", "description": "Directory to search in (default: current dir)"}
                },
                "required": ["pattern"]
            }
        },
        {
            "name": "bash",
            "description": "Execute a shell command. Use for builds, tests, git, package managers. On Windows use PowerShell syntax (`;` separators, not `&&`); this tool already captures stdout/stderr, so `2>&1` is usually unnecessary.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "Shell command to execute"}
                },
                "required": ["command"]
            }
        },
        {
            "name": "web_search",
            "description": "Search the web using DuckDuckGo.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query"}
                },
                "required": ["query"]
            }
        },
        {
            "name": "web_fetch",
            "description": "Fetch and return content from a URL.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "URL to fetch"}
                },
                "required": ["url"]
            }
        },
        {
            "name": "screenshot",
            "description": "Capture the current screen and return base64 PNG data plus dimensions.",
            "input_schema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        },
        {
            "name": "find_window",
            "description": "Find visible top-level windows whose title contains the query.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Window title query"}
                },
                "required": ["title"]
            }
        },
        {
            "name": "click",
            "description": "Click mouse at absolute screen coordinates.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "x": {"type": "integer", "description": "X coordinate"},
                    "y": {"type": "integer", "description": "Y coordinate"}
                },
                "required": ["x", "y"]
            }
        },
        {
            "name": "click_text",
            "description": "Click UI element by visible text label using Windows UI Automation.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Exact label text"}
                },
                "required": ["text"]
            }
        },
        {
            "name": "type_text",
            "description": "Type text into a target window via synthetic keystrokes. IMPORTANT: SendKeys posts to the foreground window, so when targeting a GUI app (Notepad, browser, editor) you MUST pass window_pid (preferred) or window_title so the tool can activate that window first. Without one of those, keystrokes will go to the terminal running asi-code, not your app.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Text to type"},
                    "window_pid": {"type": "integer", "description": "PID of the process whose main window should receive keystrokes (preferred; obtain from find_window)"},
                    "window_title": {"type": "string", "description": "Case-insensitive substring of the target window title (used only if window_pid is not provided)"}
                },
                "required": ["text"]
            }
        },
        {
            "name": "read_screen_text",
            "description": "Capture screen and run OCR, returning extracted text.",
            "input_schema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        },
        {
            "name": "ue5_bridge",
            "description": "Run Python script through Unreal Engine 5 bridge. Uses UnrealEditor command line execution.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "Python script source to execute"},
                    "project": {"type": "string", "description": "Optional UE project path"}
                },
                "required": ["script"]
            }
        },
        {
            "name": "ue5_scene_probe",
            "description": "Collect structured UE5 level scene state as JSON for deterministic 3D reasoning.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "UE project path (.uproject) for UnrealEditor-Cmd"},
                    "max_objects": {"type": "integer", "description": "Maximum actors to include in output (default 500)"}
                },
                "required": ["project"]
            }
        },
        {
            "name": "blender_bridge",
            "description": "Run Python script through Blender bridge (`blender --background --python`).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "script": {"type": "string", "description": "Python script source to execute"},
                    "project": {"type": "string", "description": "Optional blend file path (reserved)"}
                },
                "required": ["script"]
            }
        },
        {
            "name": "blender_scene_probe",
            "description": "Collect structured Blender scene state as JSON (objects, transforms, cameras, lights, actions) for deterministic 3D reasoning.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "Optional .blend path (reserved for future direct-open support)"},
                    "script": {"type": "string", "description": "Optional Python snippet appended after probe collection"}
                },
                "required": []
            }
        },
        {
            "name": "unity_bridge",
            "description": "Drive a Unity project. The `action` field selects one of: `python` (default; runs Python via batchmode `PythonRunner.RunFile` — spawns a fresh headless Unity, will fail if the project is already open in another Editor), `open` (launches the Editor for the project and returns immediately; use this before csharp/create_terrain to make sure the live Editor is running), `csharp` (executes C# inside the ALREADY-RUNNING Editor by dropping a one-shot script into Assets/Editor/ and polling its result — required for interactive edits to the open scene), `create_terrain` (convenience: spawns a Terrain GameObject in the active scene, dispatches via csharp), `save` (saves open scenes + asset database, via csharp).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "Absolute path to the Unity project root (the folder containing Assets/, Library/, ProjectSettings/)."},
                    "action": {"type": "string", "description": "One of: python | open | csharp | create_terrain | save. Defaults to python.", "enum": ["python", "open", "csharp", "create_terrain", "save"]},
                    "script": {"type": "string", "description": "Source code body. Python source when action=python; C# source when action=csharp (`csharp` field also accepted). Unused for open/save."},
                    "csharp": {"type": "string", "description": "Alias for `script` when action=csharp, for clarity."},
                    "size": {"type": "array", "items": {"type": "number"}, "description": "Terrain size [x,y,z] in world units. Only used by action=create_terrain. Defaults to [500,50,500]."},
                    "heightmap_resolution": {"type": "integer", "description": "Heightmap resolution (33-4097) for create_terrain. Defaults to 513."},
                    "name": {"type": "string", "description": "GameObject name for create_terrain. Defaults to AsiTerrain."}
                },
                "required": ["project"]
            }
        },
        {
            "name": "unity_scene_probe",
            "description": "Collect structured Unity active-scene state as JSON by running C# in the live Editor.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "project": {"type": "string", "description": "Absolute path to Unity project root"},
                    "max_objects": {"type": "integer", "description": "Maximum objects to include (default 1000)"},
                    "include_inactive": {"type": "boolean", "description": "Include inactive objects (default true)"}
                },
                "required": ["project"]
            }
        },
        {
            "name": "probe_diff",
            "description": "Compute deterministic JSON diff between two scene probes (`before` and `after`).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "before": {"type": "object", "description": "Probe JSON before mutation"},
                    "after": {"type": "object", "description": "Probe JSON after mutation"},
                    "numeric_tolerance": {"type": "number", "description": "Optional absolute epsilon for numeric comparisons (default 0.0001)"}
                },
                "required": ["before", "after"]
            }
        },
        {
            "name": "video_capture",
            "description": "Capture frame sequence to a directory (v1 frame-folder approach, no mp4 state).",
            "input_schema": {
                "type": "object",
                "properties": {
                    "duration_sec": {"type": "integer", "description": "Capture duration in seconds (default 6)"},
                    "fps": {"type": "integer", "description": "Sampling FPS (default 2)"},
                    "target": {"type": "string", "description": "Capture target: window or fullscreen", "enum": ["window", "fullscreen"]},
                    "window_title": {"type": "string", "description": "Window-title substring when target=window"},
                    "window_pid": {"type": "integer", "description": "Preferred target window PID when target=window"},
                    "out_dir": {"type": "string", "description": "Optional output directory; auto-generated temp dir when omitted"}
                },
                "required": []
            }
        },
        {
            "name": "video_keyframes",
            "description": "Select a compact keyframe set from captured PNG frames in a directory.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "dir": {"type": "string", "description": "Frame directory from video_capture"},
                    "max_frames": {"type": "integer", "description": "Maximum selected frames to return (default 12)"}
                },
                "required": ["dir"]
            }
        }
    ])
}

/// Convert structured tool call arguments to the legacy /toolcall format args string.
pub fn tool_call_to_legacy_args(name: &str, arguments: &str) -> String {
    let Ok(args) = serde_json::from_str::<Value>(arguments) else {
        return arguments.to_string();
    };

    match name {
        "read_file" => {
            let path = args.get("path").and_then(Value::as_str).unwrap_or("");
            let start = args.get("start_line").and_then(Value::as_u64);
            let max = args.get("max_lines").and_then(Value::as_u64);
            match (start, max) {
                (Some(s), Some(m)) => format!("{} {} {}", path, s, m),
                _ => path.to_string(),
            }
        }
        "write_file" => {
            let path = args.get("path").and_then(Value::as_str).unwrap_or("");
            let content = args.get("content").and_then(Value::as_str).unwrap_or("");
            format!("{} <<<CONTENT\n{}\n<<<END", path, content)
        }
        "edit_file" => {
            let path = args.get("path").and_then(Value::as_str).unwrap_or("");
            let old = args.get("old_text").and_then(Value::as_str).unwrap_or("");
            let new = args.get("new_text").and_then(Value::as_str).unwrap_or("");
            format!("{} <<<OLD\n{}\n<<<NEW\n{}\n<<<END", path, old, new)
        }
        "glob_search" => args
            .get("pattern")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "grep_search" => {
            let pattern = args.get("pattern").and_then(Value::as_str).unwrap_or("");
            let base = args.get("base_path").and_then(Value::as_str).unwrap_or(".");
            format!("{} {}", pattern, base)
        }
        "bash" => args
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "web_search" => args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "web_fetch" => args
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "screenshot" => String::new(),
        "find_window" => args
            .get("title")
            .or_else(|| args.get("query"))
            .or_else(|| args.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "click" | "mouse_click" => {
            let x = args
                .get("x")
                .and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
                })
                .unwrap_or(0);
            let y = args
                .get("y")
                .and_then(|v| {
                    v.as_i64()
                        .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
                })
                .unwrap_or(0);
            format!("{} {}", x, y)
        }
        "click_text" => args
            .get("text")
            .or_else(|| args.get("label"))
            .or_else(|| args.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "type" | "type_text" => args
            .get("text")
            .or_else(|| args.get("content"))
            .or_else(|| args.get("input"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        "read_screen_text" => String::new(),
        "ue5_bridge"
        | "ue5_scene_probe"
        | "blender_bridge"
        | "blender_scene_probe"
        | "unity_bridge"
        | "unity_scene_probe"
        | "probe_diff"
        | "video_capture"
        | "video_keyframes" => args.to_string(),
        _ => arguments.to_string(),
    }
}

impl ProviderClient {
    pub fn new(provider: String, model: String) -> Self {
        Self {
            provider: normalize_provider_name(&provider),
            model: resolve_model_alias(&model),
            http: Client::new(),
        }
    }

    /// Streaming completion (text only, legacy).
    pub fn complete_streaming<F>(
        &self,
        messages: &[ChatMessage],
        mut on_delta: F,
    ) -> Result<StreamingResult, String>
    where
        F: FnMut(&str),
    {
        match self.provider.as_str() {
            "claude" => self.claude_streaming(messages, &mut on_delta),
            "deepseek" => self.openai_compat_streaming(
                messages,
                "DEEPSEEK_API_KEY",
                "DEEPSEEK_BASE_URL",
                "https://api.deepseek.com/v1",
                "DeepSeek",
                &mut on_delta,
            ),
            _ => self.openai_compat_streaming(
                messages,
                "OPENAI_API_KEY",
                "OPENAI_BASE_URL",
                "https://api.openai.com/v1",
                "OpenAI",
                &mut on_delta,
            ),
        }
    }

    /// Streaming completion with native tool calling support.
    /// Returns structured CompletionResult with text and/or tool calls.
    pub fn complete_with_tools<F>(
        &self,
        messages: &[ChatMessage],
        tool_results: &[(String, String)], // (tool_call_id, result_text) for previous tool results
        on_delta: &mut F,
    ) -> Result<CompletionResult, String>
    where
        F: FnMut(&str),
    {
        match self.provider.as_str() {
            "claude" => self.claude_with_tools(messages, tool_results, on_delta),
            "deepseek" => self.openai_with_tools(
                messages,
                tool_results,
                "DEEPSEEK_API_KEY",
                "DEEPSEEK_BASE_URL",
                "https://api.deepseek.com/v1",
                "DeepSeek",
                on_delta,
            ),
            _ => self.openai_with_tools(
                messages,
                tool_results,
                "OPENAI_API_KEY",
                "OPENAI_BASE_URL",
                "https://api.openai.com/v1",
                "OpenAI",
                on_delta,
            ),
        }
    }

    // ── OpenAI-compatible with native tool calling ──

    fn openai_with_tools<F>(
        &self,
        messages: &[ChatMessage],
        tool_results: &[(String, String)],
        key_env: &str,
        url_env: &str,
        default_url: &str,
        label: &str,
        on_delta: &mut F,
    ) -> Result<CompletionResult, String>
    where
        F: FnMut(&str),
    {
        let key = env::var(key_env).map_err(|_| format!("Missing {}", key_env))?;
        let base = env::var(url_env).unwrap_or_else(|_| default_url.to_string());

        // Build messages array, replaying native tool-calling protocol from
        // history: assistant messages keep their tool_calls array, and tool
        // results go out as role="tool" with the matching tool_call_id. This
        // is the exact shape tool-trained models expect; flattening it into
        // user text (the old behavior) pushed models off-distribution and
        // caused erratic mid-task stops.
        let api_messages = build_openai_api_messages(&self.provider, messages, tool_results);

        let mut payload = json!({
            "model": self.model,
            "messages": api_messages,
            "temperature": 0.2,
            "max_tokens": configured_max_tokens(&self.provider),
            "stream": true,
            "tools": tool_definitions_openai()
        });
        apply_openai_model_parameters(&mut payload, &self.model, &self.provider);
        if should_enable_deepseek_reasoning_effort(&self.provider, &self.model) {
            if let Some(level) = deepseek_reasoning_effort_value() {
                payload["reasoning_effort"] = Value::String(level);
            }
        }

        let endpoint = format!("{}/chat/completions", base.trim_end_matches('/'));
        // Gateways often roll model versions independently and may reject
        // several formerly accepted fields one at a time. Adapt only the
        // field named by each 400 response, with a strict retry bound.
        let mut compatibility_retries = 0usize;
        let response = loop {
            let response = self
                .http
                .post(&endpoint)
                .header(CONTENT_TYPE, "application/json")
                .header(AUTHORIZATION, format!("Bearer {}", key))
                .header(ACCEPT_ENCODING, "identity")
                .json(&payload)
                .send()
                .map_err(|e| e.to_string())?;
            if response.status().is_success() {
                break response;
            }

            let error = http_error_with_body(label, response);
            if compatibility_retries >= 4 || !should_retry_openai_parameter_variant(&error) {
                return Err(error);
            }

            let previous_payload = payload.clone();
            flip_openai_parameter_variant(&mut payload, &error);
            if payload == previous_payload {
                return Err(error);
            }
            compatibility_retries += 1;
        };

        self.read_openai_sse_with_tools(response, on_delta)
    }

    fn read_openai_sse_with_tools<F>(
        &self,
        response: Response,
        on_delta: &mut F,
    ) -> Result<CompletionResult, String>
    where
        F: FnMut(&str),
    {
        let mut reader = BufReader::new(response);
        let mut line = String::new();
        let mut text_output = String::new();
        let mut reasoning_output = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut finish_reason = String::new();
        let mut saw_sse = false;
        let mut raw_json = String::new();
        let mut parsed_sse_choices = 0usize;

        // Accumulate tool call chunks (OpenAI streams tool calls in pieces)
        let mut tc_index_map: std::collections::HashMap<usize, (String, String, String)> =
            std::collections::HashMap::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            let trimmed = line.trim();
            if !trimmed.starts_with("data:") {
                if !trimmed.is_empty() {
                    raw_json.push_str(trimmed);
                }
                continue;
            }
            saw_sse = true;
            let data = trimmed.trim_start_matches("data:").trim();
            if data == "[DONE]" {
                break;
            }

            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };

            let choice = match v.get("choices").and_then(|c| c.get(0)) {
                Some(c) => c,
                None => continue,
            };
            parsed_sse_choices += 1;

            if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
                finish_reason = fr.to_string();
            }
            if finish_reason.is_empty() {
                if let Some(sr) = choice.get("stop_reason").and_then(Value::as_str) {
                    finish_reason = sr.to_string();
                }
            }

            if let Some(delta) = choice.get("delta") {
                if let Some(reasoning) = delta.get("reasoning_content").and_then(Value::as_str) {
                    reasoning_output.push_str(reasoning);
                }
                // Text content
                if let Some(content) = delta.get("content").and_then(Value::as_str) {
                    if !content.is_empty() {
                        text_output.push_str(content);
                        on_delta(content);
                    }
                }

                // Tool calls (streamed in chunks). OpenAI-compatible
                // gateways use both the standard plural field and a
                // singular `tool_call`/`function_call` variant.
                if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
                    for (offset, tc) in tcs.iter().enumerate() {
                        append_tool_call_value(&mut tc_index_map, tc, offset);
                    }
                }
                if let Some(tc) = delta.get("tool_call") {
                    append_tool_call_value(&mut tc_index_map, tc, 0);
                }
                if let Some(fc) = delta.get("function_call") {
                    append_tool_call_value(&mut tc_index_map, fc, 0);
                }
            }

            // Some proxies put complete tool calls on `choice.message` even
            // while returning an SSE envelope. Preserve those calls too.
            if let Some(message) = choice.get("message") {
                if let Some(reasoning) = message.get("reasoning_content").and_then(Value::as_str) {
                    reasoning_output.push_str(reasoning);
                }
                if let Some(content) = message.get("content").and_then(Value::as_str) {
                    if !content.is_empty() {
                        text_output.push_str(content);
                        on_delta(content);
                    }
                }
                if let Some(tcs) = message.get("tool_calls").and_then(Value::as_array) {
                    for (offset, tc) in tcs.iter().enumerate() {
                        append_tool_call_value(&mut tc_index_map, tc, offset);
                    }
                }
                if let Some(fc) = message.get("function_call") {
                    append_tool_call_value(&mut tc_index_map, fc, 0);
                }
            }
        }

        // A number of OpenAI-compatible gateways ignore `stream=true` and
        // return one ordinary JSON completion. Treat that response as a
        // valid native completion instead of silently converting it to an
        // empty `completed` turn (the source of premature post-tool stops).
        if !saw_sse {
            if raw_json.is_empty() {
                return Err(
                    "OpenAI-compatible response contained neither SSE data nor JSON".to_string(),
                );
            }
            let v = parse_openai_json_response(&raw_json)?;
            if let Some(choice) = v.get("choices").and_then(|c| c.get(0)) {
                if let Some(message) = choice.get("message") {
                    if let Some(reasoning) =
                        message.get("reasoning_content").and_then(Value::as_str)
                    {
                        reasoning_output.push_str(reasoning);
                    }
                    if let Some(content) = message.get("content").and_then(Value::as_str) {
                        text_output.push_str(content);
                        if !content.is_empty() {
                            on_delta(content);
                        }
                    }
                    if let Some(tcs) = message.get("tool_calls").and_then(Value::as_array) {
                        for (idx, tc) in tcs.iter().enumerate() {
                            let id = tc
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|v| !v.is_empty())
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("call_auto_{}", idx));
                            let function = tc.get("function").unwrap_or(tc);
                            let name = function.get("name").and_then(Value::as_str).unwrap_or("");
                            let arguments = tool_arguments_string(function.get("arguments"));
                            if !name.is_empty() {
                                tool_calls.push(ToolCall {
                                    id,
                                    name: name.to_string(),
                                    arguments,
                                });
                            }
                        }
                    }
                    if let Some(fc) = message.get("function_call") {
                        let function = fc.get("function").unwrap_or(fc);
                        let name = function.get("name").and_then(Value::as_str).unwrap_or("");
                        if !name.is_empty() {
                            tool_calls.push(ToolCall {
                                id: fc
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .filter(|value| !value.is_empty())
                                    .unwrap_or("call_auto_0")
                                    .to_string(),
                                name: name.to_string(),
                                arguments: tool_arguments_string(function.get("arguments")),
                            });
                        }
                    }
                }
                if finish_reason.is_empty() {
                    finish_reason = choice
                        .get("finish_reason")
                        .and_then(Value::as_str)
                        .or_else(|| choice.get("stop_reason").and_then(Value::as_str))
                        .unwrap_or("")
                        .to_string();
                }
            }
        } else if parsed_sse_choices == 0 {
            return Err(
                "OpenAI-compatible stream contained no valid completion choices".to_string(),
            );
        }

        // Convert accumulated tool call chunks into ToolCall structs
        let mut indices: Vec<usize> = tc_index_map.keys().copied().collect();
        indices.sort();
        for idx in indices {
            if let Some((id, name, arguments)) = tc_index_map.remove(&idx) {
                if !name.is_empty() {
                    // Some OpenAI-compatible gateways omit tool call ids in the
                    // stream. History replay needs a stable id to pair the
                    // assistant tool_calls entry with its role="tool" result,
                    // so synthesize one when missing.
                    let id = if id.is_empty() {
                        format!("call_auto_{}", idx)
                    } else {
                        id
                    };
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments,
                    });
                }
            }
        }

        let stop = if !tool_calls.is_empty() {
            "tool_use".to_string()
        } else if finish_reason.is_empty() {
            "completed".to_string()
        } else {
            finish_reason
        };

        Ok(CompletionResult {
            text: text_output,
            tool_calls,
            stop_reason: stop,
            reasoning_content: reasoning_output,
        })
    }

    // ── Claude with native tool_use ──

    fn claude_with_tools<F>(
        &self,
        messages: &[ChatMessage],
        tool_results: &[(String, String)],
        on_delta: &mut F,
    ) -> Result<CompletionResult, String>
    where
        F: FnMut(&str),
    {
        let api_key = env::var("ANTHROPIC_API_KEY").ok();
        let oauth_token = oauth::load_token("claude");
        if api_key.is_none() && oauth_token.is_none() {
            return Err("Missing ANTHROPIC_API_KEY and no Claude OAuth token".to_string());
        }

        let base = env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());

        let system = messages
            .iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.clone())
            .unwrap_or_else(|| "You are ASI Code".to_string());

        // Build messages for Claude, replaying native tool history as content
        // blocks: assistant tool_calls become tool_use blocks, and role="tool"
        // messages become tool_result blocks grouped into the single user
        // message that immediately follows the assistant turn (the shape the
        // Anthropic API requires).
        let mut api_messages: Vec<Value> = Vec::new();
        let mut pending_tool_results: Vec<Value> = Vec::new();
        for m in messages.iter() {
            if m.role == "tool" {
                pending_tool_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.content
                }));
                continue;
            }
            if m.role != "user" && m.role != "assistant" {
                continue;
            }
            if !pending_tool_results.is_empty() {
                api_messages.push(json!({
                    "role": "user",
                    "content": std::mem::take(&mut pending_tool_results)
                }));
            }
            if m.role == "assistant" && !m.tool_calls.is_empty() {
                let mut blocks: Vec<Value> = Vec::new();
                if !m.content.is_empty() {
                    blocks.push(json!({"type": "text", "text": m.content}));
                }
                for tc in &m.tool_calls {
                    let input: Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({}));
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.function.name,
                        "input": input
                    }));
                }
                api_messages.push(json!({"role": "assistant", "content": blocks}));
            } else {
                api_messages.push(json!({"role": m.role, "content": m.content}));
            }
        }
        if !pending_tool_results.is_empty() {
            api_messages.push(json!({
                "role": "user",
                "content": std::mem::take(&mut pending_tool_results)
            }));
        }

        // Append tool results as user messages with tool_result content blocks
        if !tool_results.is_empty() {
            let mut blocks: Vec<Value> = Vec::new();
            for (tool_use_id, result_text) in tool_results {
                blocks.push(json!({
                    "type": "tool_result",
                    "tool_use_id": tool_use_id,
                    "content": result_text
                }));
            }
            api_messages.push(json!({"role": "user", "content": blocks}));
        }

        let payload = json!({
            "model": self.model,
            "max_tokens": configured_max_tokens(&self.provider),
            "system": system,
            "messages": api_messages,
            "tools": tool_definitions_claude(),
            "stream": true,
        });

        let mut req = self
            .http
            .post(format!("{}/v1/messages", base.trim_end_matches('/')))
            .header(CONTENT_TYPE, "application/json")
            .header("anthropic-version", "2023-06-01");

        if let Some(v) = api_key {
            req = req.header("x-api-key", v);
        } else if let Some(v) = oauth_token {
            req = req.header(AUTHORIZATION, format!("Bearer {}", v));
        }

        let response = req.json(&payload).send().map_err(|e| e.to_string())?;
        let response = ensure_success_response("Claude", response)?;

        self.read_claude_sse_with_tools(response, on_delta)
    }

    fn read_claude_sse_with_tools<F>(
        &self,
        response: Response,
        on_delta: &mut F,
    ) -> Result<CompletionResult, String>
    where
        F: FnMut(&str),
    {
        let mut reader = BufReader::new(response);
        let mut line = String::new();
        let mut text_output = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut stop_reason = String::new();

        // Track current content block type
        let mut current_block_type = String::new();
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_args = String::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            let trimmed = line.trim();
            if !trimmed.starts_with("data:") {
                continue;
            }
            let data = trimmed.trim_start_matches("data:").trim();
            if data.is_empty() {
                continue;
            }

            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };

            let kind = v.get("type").and_then(Value::as_str).unwrap_or("");

            match kind {
                "content_block_start" => {
                    if let Some(cb) = v.get("content_block") {
                        let btype = cb.get("type").and_then(Value::as_str).unwrap_or("");
                        current_block_type = btype.to_string();
                        if btype == "tool_use" {
                            current_tool_id = cb
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            current_tool_name = cb
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            current_tool_args.clear();
                        }
                    }
                }
                "content_block_delta" => {
                    if let Some(delta) = v.get("delta") {
                        let delta_type = delta.get("type").and_then(Value::as_str).unwrap_or("");
                        if delta_type == "text_delta" {
                            if let Some(text) = delta.get("text").and_then(Value::as_str) {
                                if !text.is_empty() {
                                    text_output.push_str(text);
                                    on_delta(text);
                                }
                            }
                        } else if delta_type == "input_json_delta" {
                            if let Some(partial) = delta.get("partial_json").and_then(Value::as_str)
                            {
                                current_tool_args.push_str(partial);
                            }
                        }
                    }
                }
                "content_block_stop" => {
                    if current_block_type == "tool_use" && !current_tool_name.is_empty() {
                        tool_calls.push(ToolCall {
                            id: current_tool_id.clone(),
                            name: current_tool_name.clone(),
                            arguments: current_tool_args.clone(),
                        });
                        current_tool_id.clear();
                        current_tool_name.clear();
                        current_tool_args.clear();
                    }
                    current_block_type.clear();
                }
                "message_delta" => {
                    if let Some(delta) = v.get("delta") {
                        if let Some(sr) = delta.get("stop_reason").and_then(Value::as_str) {
                            stop_reason = sr.to_string();
                        }
                    }
                }
                "message_stop" => {
                    break;
                }
                _ => {}
            }
        }

        let stop = if !tool_calls.is_empty() && stop_reason.is_empty() {
            "tool_use".to_string()
        } else if stop_reason.is_empty() {
            "completed".to_string()
        } else {
            stop_reason
        };

        Ok(CompletionResult {
            text: text_output,
            tool_calls,
            stop_reason: stop,
            reasoning_content: String::new(),
        })
    }

    // ── Legacy streaming methods (kept for backward compatibility) ──

    fn openai_compat(
        &self,
        messages: &[ChatMessage],
        key_env: &str,
        url_env: &str,
        default_url: &str,
        label: &str,
    ) -> Result<StreamingResult, String> {
        let key = env::var(key_env).map_err(|_| format!("Missing {}", key_env))?;
        let base = env::var(url_env).unwrap_or_else(|_| default_url.to_string());
        let mut payload = json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0.2,
            "max_tokens": configured_max_tokens(&self.provider)
        });
        apply_openai_model_parameters(&mut payload, &self.model, &self.provider);
        if should_enable_deepseek_reasoning_effort(&self.provider, &self.model) {
            if let Some(level) = deepseek_reasoning_effort_value() {
                payload["reasoning_effort"] = Value::String(level);
            }
        }

        let endpoint = format!("{}/chat/completions", base.trim_end_matches('/'));
        let mut response = self
            .http
            .post(&endpoint)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", key))
            .header(ACCEPT_ENCODING, "identity")
            .json(&payload)
            .send()
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            let error = http_error_with_body(label, response);
            if should_retry_openai_parameter_variant(&error) {
                flip_openai_parameter_variant(&mut payload, &error);
                response = self
                    .http
                    .post(&endpoint)
                    .header(CONTENT_TYPE, "application/json")
                    .header(AUTHORIZATION, format!("Bearer {}", key))
                    .header(ACCEPT_ENCODING, "identity")
                    .json(&payload)
                    .send()
                    .map_err(|e| e.to_string())?;
            } else {
                return Err(error);
            }
        }
        let response = ensure_success_response(label, response)?;
        let v = response.json::<Value>().map_err(|e| e.to_string())?;

        let text = v
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let stop_reason = v
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("finish_reason"))
            .and_then(Value::as_str)
            .unwrap_or("completed")
            .to_string();
        Ok(StreamingResult { text, stop_reason })
    }

    fn openai_compat_streaming<F>(
        &self,
        messages: &[ChatMessage],
        key_env: &str,
        url_env: &str,
        default_url: &str,
        label: &str,
        on_delta: &mut F,
    ) -> Result<StreamingResult, String>
    where
        F: FnMut(&str),
    {
        let key = env::var(key_env).map_err(|_| format!("Missing {}", key_env))?;
        let base = env::var(url_env).unwrap_or_else(|_| default_url.to_string());
        let mut payload = json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0.2,
            "max_tokens": configured_max_tokens(&self.provider),
            "stream": true
        });
        apply_openai_model_parameters(&mut payload, &self.model, &self.provider);
        if should_enable_deepseek_reasoning_effort(&self.provider, &self.model) {
            if let Some(level) = deepseek_reasoning_effort_value() {
                payload["reasoning_effort"] = Value::String(level);
            }
        }

        let endpoint = format!("{}/chat/completions", base.trim_end_matches('/'));
        let mut response = self
            .http
            .post(&endpoint)
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", key))
            .header(ACCEPT_ENCODING, "identity")
            .json(&payload)
            .send()
            .map_err(|e| e.to_string())?;
        if !response.status().is_success() {
            let error = http_error_with_body(label, response);
            if should_retry_openai_parameter_variant(&error) {
                flip_openai_parameter_variant(&mut payload, &error);
                response = self
                    .http
                    .post(&endpoint)
                    .header(CONTENT_TYPE, "application/json")
                    .header(AUTHORIZATION, format!("Bearer {}", key))
                    .header(ACCEPT_ENCODING, "identity")
                    .json(&payload)
                    .send()
                    .map_err(|e| e.to_string())?;
            } else {
                return Err(error);
            }
        }
        let response = ensure_success_response(label, response)?;

        let mut streamed = self.read_openai_sse(response, on_delta)?;
        if streamed.text.is_empty() {
            streamed = self.openai_compat(messages, key_env, url_env, default_url, label)?;
            if !streamed.text.is_empty() {
                on_delta(&streamed.text);
            }
        }
        Ok(streamed)
    }

    fn read_openai_sse<F>(
        &self,
        response: Response,
        on_delta: &mut F,
    ) -> Result<StreamingResult, String>
    where
        F: FnMut(&str),
    {
        let mut reader = BufReader::new(response);
        let mut line = String::new();
        let mut output = String::new();
        let mut finish_reason = String::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            let trimmed = line.trim();
            if !trimmed.starts_with("data:") {
                continue;
            }
            let data = trimmed.trim_start_matches("data:").trim();
            if data == "[DONE]" {
                break;
            }

            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if let Some(fr) = v
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("finish_reason"))
                .and_then(Value::as_str)
            {
                finish_reason = fr.to_string();
            }
            if let Some(delta) = v
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("delta"))
                .and_then(|d| d.get("content"))
                .and_then(Value::as_str)
            {
                if !delta.is_empty() {
                    output.push_str(delta);
                    on_delta(delta);
                }
            }
        }

        let stop_reason = if finish_reason.is_empty() {
            "completed".to_string()
        } else {
            finish_reason
        };
        Ok(StreamingResult {
            text: output,
            stop_reason,
        })
    }

    fn claude(&self, messages: &[ChatMessage]) -> Result<StreamingResult, String> {
        let api_key = env::var("ANTHROPIC_API_KEY").ok();
        let oauth_token = oauth::load_token("claude");
        if api_key.is_none() && oauth_token.is_none() {
            return Err("Missing ANTHROPIC_API_KEY and no Claude OAuth token".to_string());
        }

        let base = env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());

        let system = messages
            .iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.clone())
            .unwrap_or_else(|| "You are ASI Code".to_string());

        let filtered: Vec<Value> = messages
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .map(|m| json!({"role": m.role, "content": m.content}))
            .collect();

        let payload = json!({
            "model": self.model,
            "max_tokens": configured_max_tokens(&self.provider),
            "system": system,
            "messages": filtered,
        });

        let mut req = self
            .http
            .post(format!("{}/v1/messages", base.trim_end_matches('/')))
            .header(CONTENT_TYPE, "application/json")
            .header("anthropic-version", "2023-06-01");

        if let Some(v) = api_key {
            req = req.header("x-api-key", v);
        } else if let Some(v) = oauth_token {
            req = req.header(AUTHORIZATION, format!("Bearer {}", v));
        }

        let response = req.json(&payload).send().map_err(|e| e.to_string())?;
        let response = ensure_success_response("Claude", response)?;
        let v = response.json::<Value>().map_err(|e| e.to_string())?;

        let mut out = Vec::new();
        if let Some(arr) = v.get("content").and_then(Value::as_array) {
            for item in arr {
                if item.get("type").and_then(Value::as_str) == Some("text") {
                    out.push(
                        item.get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    );
                }
            }
        }
        let stop_reason = v
            .get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("completed")
            .to_string();
        Ok(StreamingResult {
            text: out.join("\n"),
            stop_reason,
        })
    }

    fn claude_streaming<F>(
        &self,
        messages: &[ChatMessage],
        on_delta: &mut F,
    ) -> Result<StreamingResult, String>
    where
        F: FnMut(&str),
    {
        let api_key = env::var("ANTHROPIC_API_KEY").ok();
        let oauth_token = oauth::load_token("claude");
        if api_key.is_none() && oauth_token.is_none() {
            return Err("Missing ANTHROPIC_API_KEY and no Claude OAuth token".to_string());
        }

        let base = env::var("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com".to_string());

        let system = messages
            .iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.clone())
            .unwrap_or_else(|| "You are ASI Code".to_string());

        let filtered: Vec<Value> = messages
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
            .map(|m| json!({"role": m.role, "content": m.content}))
            .collect();

        let payload = json!({
            "model": self.model,
            "max_tokens": configured_max_tokens(&self.provider),
            "system": system,
            "messages": filtered,
            "stream": true,
        });

        let mut req = self
            .http
            .post(format!("{}/v1/messages", base.trim_end_matches('/')))
            .header(CONTENT_TYPE, "application/json")
            .header("anthropic-version", "2023-06-01");

        if let Some(v) = api_key {
            req = req.header("x-api-key", v);
        } else if let Some(v) = oauth_token {
            req = req.header(AUTHORIZATION, format!("Bearer {}", v));
        }

        let response = req.json(&payload).send().map_err(|e| e.to_string())?;
        let response = ensure_success_response("Claude", response)?;

        let mut reader = BufReader::new(response);
        let mut line = String::new();
        let mut output = String::new();

        loop {
            line.clear();
            let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            let trimmed = line.trim();
            if !trimmed.starts_with("data:") {
                continue;
            }
            let data = trimmed.trim_start_matches("data:").trim();
            if data.is_empty() {
                continue;
            }

            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
            if kind == "content_block_delta" {
                if let Some(delta) = v
                    .get("delta")
                    .and_then(|d| d.get("text"))
                    .and_then(Value::as_str)
                {
                    if !delta.is_empty() {
                        output.push_str(delta);
                        on_delta(delta);
                    }
                }
            }
            if kind == "message_stop" {
                break;
            }
        }

        if output.is_empty() {
            let fallback = self.claude(messages)?;
            output = fallback.text;
            if !output.is_empty() {
                on_delta(&output);
            }
        }

        Ok(StreamingResult {
            text: output,
            stop_reason: "completed".to_string(),
        })
    }
}

impl ChatProvider for ProviderClient {
    fn complete_streaming_dyn(
        &self,
        messages: &[ChatMessage],
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<StreamingResult, String> {
        self.complete_streaming(messages, |delta| on_delta(delta))
    }

    fn complete_with_tools_dyn(
        &self,
        messages: &[ChatMessage],
        tool_results: &[(String, String)],
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<CompletionResult, String> {
        let mut proxy = |delta: &str| on_delta(delta);
        self.complete_with_tools(messages, tool_results, &mut proxy)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_tool_call_value, apply_openai_model_parameters, build_openai_api_messages,
        flip_openai_parameter_variant, parse_openai_json_response, tool_arguments_string,
        tool_call_to_legacy_args, uses_modern_openai_parameters, ChatMessage, ToolCallFunction,
        ToolCallSpec,
    };
    use serde_json::Value;

    fn assert_json_passthrough(tool: &str, args: &str) {
        let actual = tool_call_to_legacy_args(tool, args);
        let actual_json: Value =
            serde_json::from_str(&actual).expect("legacy args output should be valid JSON");
        let expected_json: Value =
            serde_json::from_str(args).expect("test input should be valid JSON");
        assert_eq!(actual_json, expected_json);
    }

    #[test]
    fn tool_call_to_legacy_args_maps_computer_tools() {
        assert_eq!(tool_call_to_legacy_args("screenshot", "{}"), "");
        assert_eq!(
            tool_call_to_legacy_args("find_window", r#"{"title":"Outlook"}"#),
            "Outlook"
        );
        assert_eq!(
            tool_call_to_legacy_args("click", r#"{"x":120,"y":340}"#),
            "120 340"
        );
        assert_eq!(
            tool_call_to_legacy_args("click_text", r#"{"text":"Send"}"#),
            "Send"
        );
        assert_eq!(
            tool_call_to_legacy_args("type_text", r#"{"text":"hello"}"#),
            "hello"
        );
        assert_eq!(tool_call_to_legacy_args("read_screen_text", "{}"), "");
    }

    #[test]
    fn tool_call_to_legacy_args_maps_aliases_for_computer_tools() {
        assert_eq!(
            tool_call_to_legacy_args("find_window", r#"{"query":"Notepad"}"#),
            "Notepad"
        );
        assert_eq!(
            tool_call_to_legacy_args("find_window", r#"{"name":"Calculator"}"#),
            "Calculator"
        );
        assert_eq!(
            tool_call_to_legacy_args("mouse_click", r#"{"x":"42","y":"24"}"#),
            "42 24"
        );
        assert_eq!(
            tool_call_to_legacy_args("type", r#"{"content":"abc"}"#),
            "abc"
        );
        assert_eq!(
            tool_call_to_legacy_args("type_text", r#"{"input":"xyz"}"#),
            "xyz"
        );
    }

    #[test]
    fn tool_call_to_legacy_args_passes_through_probe_and_video_json_tools() {
        assert_json_passthrough("blender_scene_probe", r#"{"project":"D:/blend/a.blend"}"#);
        assert_json_passthrough(
            "ue5_scene_probe",
            r#"{"project":"D:/UE/P/P.uproject","max_objects":321}"#,
        );
        assert_json_passthrough(
            "unity_scene_probe",
            r#"{"project":"D:/Unity/P","max_objects":256,"include_inactive":false}"#,
        );
        assert_json_passthrough(
            "probe_diff",
            r#"{"before":{"objects":[]},"after":{"objects":[]}}"#,
        );
        assert_json_passthrough("video_capture", r#"{"duration_sec":8,"fps":2}"#);
        assert_json_passthrough("video_keyframes", r#"{"dir":"C:/tmp/cap","max_frames":12}"#);
    }

    #[test]
    fn modern_openai_models_use_completion_token_parameter() {
        let mut payload = serde_json::json!({
            "temperature": 0.2,
            "max_tokens": 1024,
        });
        assert!(uses_modern_openai_parameters("gpt-5.4-codex"));
        apply_openai_model_parameters(&mut payload, "gpt-5.4-codex", "openai");
        assert!(payload.get("temperature").is_none());
        assert!(payload.get("max_tokens").is_none());
        assert_eq!(
            payload.get("max_completion_tokens").and_then(Value::as_u64),
            Some(16384)
        );
    }

    #[test]
    fn tool_call_delta_accepts_singular_and_object_arguments() {
        let mut calls = std::collections::HashMap::new();
        append_tool_call_value(
            &mut calls,
            &serde_json::json!({
                "id": "call-1",
                "function": {"name": "read_file", "arguments": {"path": "src/main.rs"}}
            }),
            0,
        );
        let (_, name, args) = calls.remove(&0).expect("tool call should be accumulated");
        assert_eq!(name, "read_file");
        assert!(args.contains("src/main.rs"));
    }

    #[test]
    fn compatibility_retry_switches_tokens_and_disables_streaming() {
        let mut payload = serde_json::json!({
            "max_completion_tokens": 2048,
            "temperature": 0.2,
            "reasoning_effort": "high",
            "stream": true,
        });
        flip_openai_parameter_variant(
            &mut payload,
            "HTTP error: 400 unsupported max_completion_tokens, temperature, reasoning_effort, stream",
        );
        assert_eq!(
            payload.get("max_tokens").and_then(Value::as_u64),
            Some(2048)
        );
        assert_eq!(payload.get("stream").and_then(Value::as_bool), Some(false));
        assert!(payload.get("max_completion_tokens").is_none());
        assert!(payload.get("temperature").is_none());
        assert!(payload.get("reasoning_effort").is_none());
    }

    #[test]
    fn compatibility_retry_only_changes_rejected_fields() {
        let mut payload = serde_json::json!({
            "max_completion_tokens": 2048,
            "temperature": 0.2,
            "stream": true,
        });
        flip_openai_parameter_variant(&mut payload, "HTTP error: 400 temperature is not supported");
        assert_eq!(
            payload.get("max_completion_tokens").and_then(Value::as_u64),
            Some(2048)
        );
        assert_eq!(payload.get("stream").and_then(Value::as_bool), Some(true));
        assert!(payload.get("temperature").is_none());
        assert!(payload.get("max_tokens").is_none());
    }

    #[test]
    fn compatibility_retry_can_adapt_sequential_rejections() {
        let mut payload = serde_json::json!({
            "max_completion_tokens": 2048,
            "temperature": 0.2,
            "stream": true,
        });
        flip_openai_parameter_variant(
            &mut payload,
            "HTTP error: 400 max_completion_tokens is unsupported",
        );
        flip_openai_parameter_variant(&mut payload, "HTTP error: 400 temperature is not supported");
        assert_eq!(
            payload.get("max_tokens").and_then(Value::as_u64),
            Some(2048)
        );
        assert!(payload.get("max_completion_tokens").is_none());
        assert!(payload.get("temperature").is_none());
        assert_eq!(payload.get("stream").and_then(Value::as_bool), Some(true));
    }

    #[test]
    fn non_streaming_response_requires_valid_completion_json() {
        let malformed = parse_openai_json_response("not-json")
            .expect_err("malformed JSON must not look like an empty completion");
        assert!(malformed.contains("invalid OpenAI-compatible response JSON"));

        let missing_choices = parse_openai_json_response(r#"{"id":"response-only"}"#)
            .expect_err("responses without choices must be rejected");
        assert!(missing_choices.contains("choices[0]"));

        let valid = parse_openai_json_response(
            r#"{"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]}"#,
        )
        .expect("ordinary non-streaming completion should remain supported");
        assert_eq!(
            valid["choices"][0]["message"]["content"].as_str(),
            Some("done")
        );
    }

    #[test]
    fn tool_arguments_support_strings_and_objects() {
        let string_value = serde_json::json!(r#"{"path":"src/main.rs"}"#);
        let object_value = serde_json::json!({"path": "src/main.rs"});
        assert_eq!(
            tool_arguments_string(Some(&string_value)),
            r#"{"path":"src/main.rs"}"#
        );
        assert_eq!(
            serde_json::from_str::<Value>(&tool_arguments_string(Some(&object_value))).unwrap(),
            object_value
        );
        assert_eq!(tool_arguments_string(None), "{}");
    }

    #[test]
    fn deepseek_tool_history_replays_reasoning_content() {
        let assistant = ChatMessage::assistant_with_tool_calls_and_reasoning(
            "",
            vec![ToolCallSpec {
                id: "call-1".to_string(),
                call_type: "function".to_string(),
                function: ToolCallFunction {
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"a.txt"}"#.to_string(),
                },
            }],
            "I should inspect the file.",
        );
        let messages = vec![
            ChatMessage::text("user", "Inspect a.txt"),
            assistant,
            ChatMessage::tool_result("call-1", "contents"),
        ];
        let wire = build_openai_api_messages("deepseek", &messages, &[]);
        assert_eq!(
            wire[1].get("reasoning_content").and_then(Value::as_str),
            Some("I should inspect the file.")
        );
        assert_eq!(
            wire[2].get("tool_call_id").and_then(Value::as_str),
            Some("call-1")
        );
    }

    #[test]
    fn deepseek_old_tool_history_supplies_empty_reasoning_content() {
        let assistant: ChatMessage = serde_json::from_value(serde_json::json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "call-old",
                "type": "function",
                "function": {"name": "read_file", "arguments": "{}"}
            }]
        }))
        .expect("older session history should remain readable");
        let wire = build_openai_api_messages("deepseek", &[assistant], &[]);
        assert_eq!(
            wire[0].get("reasoning_content").and_then(Value::as_str),
            Some("")
        );
    }

    #[test]
    fn non_deepseek_tool_history_omits_reasoning_content() {
        let assistant = ChatMessage::assistant_with_tool_calls_and_reasoning(
            "",
            vec![ToolCallSpec {
                id: "call-1".to_string(),
                call_type: "function".to_string(),
                function: ToolCallFunction {
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                },
            }],
            "private reasoning",
        );
        let wire = build_openai_api_messages("openai", &[assistant], &[]);
        assert!(wire[0].get("reasoning_content").is_none());
    }
}
