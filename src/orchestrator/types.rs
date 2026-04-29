#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCallKind {
    Read,
    Search,
    Web,
    Bash,
    Write,
    Edit,
    Other,
}

#[derive(Debug, Clone)]
pub struct ToolCallRequest {
    pub raw: String,
    pub name: String,
    pub args: String,
    pub kind: ToolCallKind,
}

impl ToolCallRequest {
    pub fn new(raw: String, name: String, args: String) -> Self {
        let kind = classify_tool_call(&name);
        Self {
            raw,
            name,
            args,
            kind,
        }
    }

    pub fn is_concurrency_safe(&self) -> bool {
        matches!(
            self.kind,
            ToolCallKind::Read | ToolCallKind::Search | ToolCallKind::Web
        )
    }

    pub fn to_command(&self) -> String {
        if self.args.is_empty() {
            format!("/toolcall {}", self.name)
        } else {
            format!("/toolcall {} {}", self.name, self.args)
        }
    }
}

pub fn classify_tool_call(name: &str) -> ToolCallKind {
    match name {
        "read_file" => ToolCallKind::Read,
        "glob_search" | "grep_search" => ToolCallKind::Search,
        "web_search" | "web_fetch" => ToolCallKind::Web,
        "bash" => ToolCallKind::Bash,
        "write_file" => ToolCallKind::Write,
        "edit_file" => ToolCallKind::Edit,
        _ => ToolCallKind::Other,
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionBatch {
    pub concurrency_safe: bool,
    pub calls: Vec<ToolCallRequest>,
}
