//! Experimental transport-neutral JavaScript orchestration for WebCodex.
//!
//! The default build contains only host/runtime contracts. The V8 implementation
//! is available exclusively through the `v8-runtime` feature.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub const MAX_SOURCE_BYTES: usize = 64 * 1024;
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;
pub const MAX_TIMEOUT_MS: u64 = 30_000;
pub const MAX_TOOL_CALLS: usize = 32;
pub const MAX_CONCURRENT_TOOL_CALLS: usize = 8;
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;
pub const MAX_OUTPUT_ITEMS: usize = 256;

pub type CodeModeHostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Host callback boundary for one nested Code Mode tool call.
///
/// Implementations are responsible for applying their normal tool authority,
/// permission, routing, and evidence semantics. The JavaScript runtime itself
/// owns no filesystem, network, Project, Session, or tool authority.
pub trait CodeModeHost: Send + Sync {
    fn invoke_tool(
        &self,
        request: CodeModeToolRequest,
    ) -> CodeModeHostFuture<'_, Result<CodeModeToolResponse, CodeModeHostError>>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeModeToolRequest {
    pub tool_name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeModeToolResponse {
    pub success: bool,
    pub output: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeModeHostError {
    message: String,
}

impl CodeModeHostError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CodeModeHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CodeModeHostError {}

#[derive(Debug, Clone)]
pub struct CodeModeExecuteRequest {
    pub source: String,
    pub allowed_tools: Vec<String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModeStats {
    pub tool_calls: usize,
    pub max_in_flight: usize,
    pub duration_ms: u64,
    pub returned_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeModeErrorKind {
    InvalidRequest,
    Runtime,
    Timeout,
    ToolCallBudgetExceeded,
    OutputLimitExceeded,
}

impl CodeModeErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::Runtime => "runtime_error",
            Self::Timeout => "timeout",
            Self::ToolCallBudgetExceeded => "tool_call_budget_exceeded",
            Self::OutputLimitExceeded => "output_limit_exceeded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeModeError {
    pub kind: CodeModeErrorKind,
    pub message: String,
    pub stats: CodeModeStats,
}

impl std::fmt::Display for CodeModeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CodeModeError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeModeExecution {
    pub content: Vec<String>,
    pub stats: CodeModeStats,
}

pub fn normalized_timeout_ms(timeout_ms: Option<u64>) -> u64 {
    timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1, MAX_TIMEOUT_MS)
}

#[cfg(feature = "v8-runtime")]
mod runtime;

#[cfg(feature = "v8-runtime")]
pub use runtime::execute;
