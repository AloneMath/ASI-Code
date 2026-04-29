use std::env;

use super::local::LocalSandbox;
use super::noop::NoopSandbox;

pub struct SandboxPreflightContext<'a> {
    pub tool: &'a str,
    pub args: &'a str,
    pub permission_mode: &'a str,
}

pub trait SandboxAdapter {
    fn name(&self) -> &'static str;
    fn preflight(&self, ctx: &SandboxPreflightContext<'_>) -> Result<(), String>;
}

#[derive(Debug, Clone)]
pub enum SandboxStrategy {
    Disabled(NoopSandbox),
    Local(LocalSandbox),
}

impl SandboxStrategy {
    pub fn from_env() -> Self {
        let mode = env::var("ASI_SANDBOX").unwrap_or_else(|_| "local".to_string());
        match mode.trim().to_ascii_lowercase().as_str() {
            "local" => Self::Local(LocalSandbox::from_env()),
            "disabled" | "off" | "none" => Self::Disabled(NoopSandbox),
            _ => Self::Local(LocalSandbox::from_env()),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Disabled(adapter) => adapter.name(),
            Self::Local(adapter) => adapter.name(),
        }
    }

    pub fn preflight(&self, ctx: &SandboxPreflightContext<'_>) -> Result<(), String> {
        match self {
            Self::Disabled(adapter) => adapter.preflight(ctx),
            Self::Local(adapter) => adapter.preflight(ctx),
        }
    }
}
