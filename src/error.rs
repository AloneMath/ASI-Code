//! Unified error handling for ASI Code
//!
//! This module defines a comprehensive error hierarchy using `thiserror` for
//! structured error handling and `anyhow` for context-rich error propagation.

use std::path::PathBuf;
use thiserror::Error;

/// Main error type for the ASI Code application.
#[derive(Debug, Error)]
pub enum AppError {
    /// Configuration-related errors
    #[error("Configuration error: {0}")]
    Config(#[from] ConfigError),

    /// Tool execution errors
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),

    /// Provider/API errors
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),

    /// Security and permission errors
    #[error("Security error: {0}")]
    Security(#[from] SecurityError),

    /// File system errors
    #[error("File system error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization errors
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Generic error with context
    #[error("{0}")]
    Message(String),
}

/// Configuration-specific errors
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Failed to load config from {path}: {source}")]
    Load {
        path: PathBuf,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("Invalid configuration value for {key}: {value}")]
    InvalidValue { key: String, value: String },

    #[error("Missing required configuration: {0}")]
    Missing(String),

    #[error("Configuration merge conflict: {0}")]
    MergeConflict(String),
}

/// Tool execution errors
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("Tool not found: {0}")]
    NotFound(String),

    #[error("Tool execution failed: {0}")]
    Execution(String),

    #[error("Tool permission denied: {0}")]
    PermissionDenied(String),

    #[error("Tool validation failed: {0}")]
    Validation(String),

    #[error("Tool timeout after {0} seconds")]
    Timeout(u64),

    #[error("Tool resource limit exceeded: {0}")]
    ResourceLimit(String),

    #[error("Tool argument parsing failed: {0}")]
    ArgumentParse(String),
}

/// Provider/API errors
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("API request failed: {0}")]
    Request(String),

    #[error("API response error: {0}")]
    Response(String),

    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Rate limit exceeded: {0}")]
    RateLimit(String),

    #[error("Model not available: {0}")]
    ModelUnavailable(String),

    #[error("Token limit exceeded: {0}")]
    TokenLimit(String),
}

/// Security and permission errors
#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("Path traversal attempt detected: {0}")]
    PathTraversal(String),

    #[error("Dangerous command blocked: {0}")]
    DangerousCommand(String),

    #[error("Access denied to path: {0}")]
    PathAccessDenied(String),

    #[error("Sandbox execution failed: {0}")]
    SandboxFailure(String),

    #[error("Resource limit violation: {0}")]
    ResourceViolation(String),

    #[error("Invalid permission mode: {0}")]
    InvalidPermissionMode(String),
}

/// Convenience type alias for Result with AppError
pub type Result<T> = std::result::Result<T, AppError>;

/// Extension trait for adding context to Result and Option
pub trait ContextExt<T> {
    /// Add context to a Result error
    fn context<C>(self, context: C) -> Result<T>
    where
        C: std::fmt::Display + Send + Sync + 'static;

    /// Add context to an Option if None
    fn with_context<C, F>(self, f: F) -> Result<T>
    where
        C: std::fmt::Display + Send + Sync + 'static,
        F: FnOnce() -> C;
}

impl<T, E> ContextExt<T> for std::result::Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn context<C>(self, context: C) -> Result<T>
    where
        C: std::fmt::Display + Send + Sync + 'static,
    {
        self.map_err(|e| AppError::Message(format!("{}: {}", context, e)))
    }

    fn with_context<C, F>(self, f: F) -> Result<T>
    where
        C: std::fmt::Display + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        self.map_err(|e| AppError::Message(format!("{}: {}", f(), e)))
    }
}

impl<T> ContextExt<T> for Option<T> {
    fn context<C>(self, context: C) -> Result<T>
    where
        C: std::fmt::Display + Send + Sync + 'static,
    {
        self.ok_or_else(|| AppError::Message(context.to_string()))
    }

    fn with_context<C, F>(self, f: F) -> Result<T>
    where
        C: std::fmt::Display + Send + Sync + 'static,
        F: FnOnce() -> C,
    {
        self.ok_or_else(|| AppError::Message(f().to_string()))
    }
}

/// Helper function to create a generic error message
pub fn err_msg<S: Into<String>>(msg: S) -> AppError {
    AppError::Message(msg.into())
}

/// Convert from String to AppError
impl From<String> for AppError {
    fn from(s: String) -> Self {
        AppError::Message(s)
    }
}

/// Convert from &str to AppError
impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        AppError::Message(s.to_string())
    }
}
