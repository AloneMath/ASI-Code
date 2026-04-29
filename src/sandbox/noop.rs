use super::adapter::{SandboxAdapter, SandboxPreflightContext};

#[derive(Debug, Clone, Default)]
pub struct NoopSandbox;

impl SandboxAdapter for NoopSandbox {
    fn name(&self) -> &'static str {
        "disabled"
    }

    fn preflight(&self, _ctx: &SandboxPreflightContext<'_>) -> Result<(), String> {
        Ok(())
    }
}
