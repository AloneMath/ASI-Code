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

        // Build messages array including tool results
        let mut api_messages: Vec<Value> = messages
            .iter()
            .map(|m| json!({"role": m.role, "content": m.content}))
            .collect();

        for (call_id, result_text) in tool_results {
            api_messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": result_text
            }));
        }

        let mut payload = json!({
            "model": self.model,
            "messages": api_messages,
            "temperature": 0.2,
            "max_tokens": configured_max_tokens(&self.provider),
            "stream": true,
            "tools": tool_definitions_openai()
        });
        if should_enable_deepseek_reasoning_effort(&self.provider, &self.model) {
            if let Some(level) = deepseek_reasoning_effort_value() {
                payload["reasoning_effort"] = Value::String(level);
            }
        }

        let response = self
            .http
            .post(format!("{}/chat/completions", base.trim_end_matches('/')))
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", key))
            .header(ACCEPT_ENCODING, "identity")
            .json(&payload)
            .send()
            .map_err(|e| e.to_string())?;
        let response = ensure_success_response(label, response)?;

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
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut finish_reason = String::new();

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
                continue;
            }
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

            if let Some(fr) = choice.get("finish_reason").and_then(Value::as_str) {
                finish_reason = fr.to_string();
            }

            if let Some(delta) = choice.get("delta") {
                // Text content
                if let Some(content) = delta.get("content").and_then(Value::as_str) {
                    if !content.is_empty() {
                        text_output.push_str(content);
                        on_delta(content);
                    }
                }

                // Tool calls (streamed in chunks)
                if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
                    for tc in tcs {
                        let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        let entry = tc_index_map.entry(idx).or_insert_with(|| {
                            let id = tc
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            (id, name, String::new())
                        });

                        // Accumulate id and name from first chunk
                        if let Some(id) = tc.get("id").and_then(Value::as_str) {
                            if !id.is_empty() && entry.0.is_empty() {
                                entry.0 = id.to_string();
                            }
                        }
                        if let Some(name) = tc
                            .get("function")
                            .and_then(|f| f.get("name"))
                            .and_then(Value::as_str)
                        {
                            if !name.is_empty() && entry.1.is_empty() {
                                entry.1 = name.to_string();
                            }
                        }

                        // Accumulate arguments
                        if let Some(args_chunk) = tc
                            .get("function")
                            .and_then(|f| f.get("arguments"))
                            .and_then(Value::as_str)
                        {
                            entry.2.push_str(args_chunk);
                        }
                    }
                }
            }
        }

        // Convert accumulated tool call chunks into ToolCall structs
        let mut indices: Vec<usize> = tc_index_map.keys().copied().collect();
        indices.sort();
        for idx in indices {
            if let Some((id, name, arguments)) = tc_index_map.remove(&idx) {
                if !name.is_empty() {
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

        // Build messages for Claude, converting to content blocks if needed
        let mut api_messages: Vec<Value> = Vec::new();
        for m in messages
            .iter()
            .filter(|m| m.role == "user" || m.role == "assistant")
        {
            api_messages.push(json!({"role": m.role, "content": m.content}));
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
        if should_enable_deepseek_reasoning_effort(&self.provider, &self.model) {
            if let Some(level) = deepseek_reasoning_effort_value() {
                payload["reasoning_effort"] = Value::String(level);
            }
        }

        let response = self
            .http
            .post(format!("{}/chat/completions", base.trim_end_matches('/')))
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", key))
            .header(ACCEPT_ENCODING, "identity")
            .json(&payload)
            .send()
            .map_err(|e| e.to_string())?;
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
        if should_enable_deepseek_reasoning_effort(&self.provider, &self.model) {
            if let Some(level) = deepseek_reasoning_effort_value() {
                payload["reasoning_effort"] = Value::String(level);
            }
        }

        let response = self
            .http
            .post(format!("{}/chat/completions", base.trim_end_matches('/')))
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {}", key))
            .header(ACCEPT_ENCODING, "identity")
            .json(&payload)
            .send()
            .map_err(|e| e.to_string())?;
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
    use super::tool_call_to_legacy_args;
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
        assert_eq!(
            tool_call_to_legacy_args("screenshot", "{}"),
            ""
        );
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
        assert_eq!(
            tool_call_to_legacy_args("read_screen_text", "{}"),
            ""
        );
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
        assert_json_passthrough("ue5_scene_probe", r#"{"project":"D:/UE/P/P.uproject","max_objects":321}"#);
        assert_json_passthrough("unity_scene_probe", r#"{"project":"D:/Unity/P","max_objects":256,"include_inactive":false}"#);
        assert_json_passthrough("probe_diff", r#"{"before":{"objects":[]},"after":{"objects":[]}}"#);
        assert_json_passthrough("video_capture", r#"{"duration_sec":8,"fps":2}"#);
        assert_json_passthrough("video_keyframes", r#"{"dir":"C:/tmp/cap","max_frames":12}"#);
    }
}
