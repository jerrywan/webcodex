use super::kernel::{
    HostFileImportTrust, ToolCallContext, ToolCallErrorStatus, ToolCallRequest, ToolTransport,
};
use super::{ResolvedProject, ToolResult, ToolRuntime};
use crate::auth::AuthContext;
use serde_json::{json, Value};
use std::sync::Arc;
use webcodex_code_mode::{
    CodeModeExecuteRequest, CodeModeHost, CodeModeHostError, CodeModeHostFuture,
    CodeModeToolRequest, CodeModeToolResponse,
};

/// E1 admission is intentionally explicit. A future tool becoming read-only does
/// not opt it into Code Mode automatically.
pub(crate) const READ_ONLY_NESTED_TOOLS: &[&str] = &[
    "read_files",
    "search_project_texts",
    "project_overview",
    "list_project_tracked_files",
    "git_status",
    "git_log",
    "git_diff_hunks",
];

/// Nested JavaScript owns only business arguments below the outer authority
/// target. These fields are either target selectors or canonical wrapper/evidence
/// metadata and therefore remain exclusively server-owned.
const FORBIDDEN_NESTED_FIELDS: &[&str] = &[
    "project",
    "session_id",
    "recording_session_id",
    "ack_session_context_revision",
    "ack_session_message_ids",
    "context_request",
    "session_message_resolution",
    "expected_failure",
    "expected_failure_kind",
    "result_expectation",
    "accepted_exit_codes",
    "assertion_name",
];

pub(crate) const MAX_MODEL_ERROR_BYTES: usize = 16 * 1024;

fn bounded_model_error(message: &str) -> String {
    if message.len() <= MAX_MODEL_ERROR_BYTES {
        return message.to_string();
    }
    let suffix = "…";
    let mut end = MAX_MODEL_ERROR_BYTES.saturating_sub(suffix.len());
    while end > 0 && !message.is_char_boundary(end) {
        end -= 1;
    }
    let mut bounded = message[..end].to_string();
    bounded.push_str(suffix);
    bounded
}

#[derive(Clone)]
pub(crate) struct RootCodeModeHost {
    tools: Arc<ToolRuntime>,
    auth: Option<AuthContext>,
    project: String,
    session_id: String,
    transport: ToolTransport,
}

impl RootCodeModeHost {
    pub(crate) fn new(
        tools: ToolRuntime,
        auth: Option<&AuthContext>,
        project: String,
        session_id: String,
        transport: ToolTransport,
    ) -> Self {
        Self {
            tools: Arc::new(tools),
            auth: auth.cloned(),
            project,
            session_id,
            transport,
        }
    }

    fn prepare_arguments(&self, request: &CodeModeToolRequest) -> Result<Value, CodeModeHostError> {
        if request.tool_name == "code_mode_exec"
            || !READ_ONLY_NESTED_TOOLS.contains(&request.tool_name.as_str())
        {
            return Err(CodeModeHostError::new(format!(
                "nested tool `{}` is not admitted by Code Mode E1",
                request.tool_name
            )));
        }
        let Some(mut arguments) = request.arguments.as_object().cloned() else {
            return Err(CodeModeHostError::new(
                "nested tool arguments must be a JSON object",
            ));
        };
        if let Some(field) = FORBIDDEN_NESTED_FIELDS
            .iter()
            .find(|field| arguments.contains_key(**field))
        {
            return Err(CodeModeHostError::new(format!(
                "nested tool arguments may not set server-owned field `{field}`"
            )));
        }
        arguments.insert("project".to_string(), Value::String(self.project.clone()));
        arguments.insert(
            "session_id".to_string(),
            Value::String(self.session_id.clone()),
        );
        Ok(Value::Object(arguments))
    }
}

impl CodeModeHost for RootCodeModeHost {
    fn invoke_tool(
        &self,
        request: CodeModeToolRequest,
    ) -> CodeModeHostFuture<'_, Result<CodeModeToolResponse, CodeModeHostError>> {
        Box::pin(async move {
            let arguments = self.prepare_arguments(&request)?;
            let outcome = self
                .tools
                .call_tool_with_context(
                    ToolCallRequest {
                        tool_name: request.tool_name,
                        arguments,
                    },
                    ToolCallContext {
                        transport: self.transport,
                        session_id: Some(self.session_id.as_str()),
                        auth: self.auth.as_ref(),
                        window: None,
                        // Nested scope denials are real Session evidence. The
                        // canonical kernel still owns the scope decision.
                        record_oauth_scope_denials: true,
                        host_file_import_trust: HostFileImportTrust::Untrusted,
                    },
                )
                .await;
            if let Some(error_status) = outcome.error_status {
                let message = match error_status {
                    ToolCallErrorStatus::InvalidArguments { message } => message,
                    ToolCallErrorStatus::InsufficientScope { description, .. } => description,
                };
                return Err(CodeModeHostError::new(message));
            }
            let result = outcome.result.ok_or_else(|| {
                CodeModeHostError::new("canonical ToolRuntime returned no nested ToolResult")
            })?;
            Ok(CodeModeToolResponse {
                success: result.success,
                output: result.output,
                error: result.error,
            })
        })
    }
}

impl ToolRuntime {
    pub(crate) async fn code_mode_exec(
        &self,
        project: ResolvedProject,
        session_id: String,
        source: String,
        timeout_ms: Option<u64>,
        auth: Option<&AuthContext>,
        transport: super::sessions::SessionTransport,
    ) -> ToolResult {
        let transport = match transport {
            super::sessions::SessionTransport::Api => ToolTransport::Api,
            super::sessions::SessionTransport::Mcp => ToolTransport::Mcp,
        };
        let host: Arc<dyn CodeModeHost> = Arc::new(RootCodeModeHost::new(
            self.clone(),
            auth,
            project.resolved_id,
            session_id,
            transport,
        ));
        match webcodex_code_mode::execute(
            host,
            CodeModeExecuteRequest {
                source,
                allowed_tools: READ_ONLY_NESTED_TOOLS
                    .iter()
                    .map(|tool| (*tool).to_string())
                    .collect(),
                timeout_ms,
            },
        )
        .await
        {
            Ok(execution) => ToolResult::ok(json!({
                "content": execution.content,
                "stats": execution.stats,
            })),
            Err(error) => ToolResult::err_with_output(
                "code mode execution failed",
                json!({
                    "failure_kind": error.kind.as_str(),
                    "message": bounded_model_error(&error.message),
                    "stats": error.stats,
                }),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webcodex_tool_contracts::{lookup_tool_definition, ToolEffect, ToolRisk};

    #[test]
    fn e1_allowlist_remains_canonically_read_only() {
        for tool in READ_ONLY_NESTED_TOOLS {
            let definition = lookup_tool_definition(tool)
                .unwrap_or_else(|| panic!("missing canonical definition for {tool}"));
            let metadata = definition.metadata();
            assert_eq!(metadata.effect, ToolEffect::Observe, "{tool}");
            assert_eq!(metadata.risk, ToolRisk::Read, "{tool}");
            assert!(!definition.is_shell_like(), "{tool}");
            assert!(!definition.is_write_like(), "{tool}");
            assert!(!definition.requires_permission(), "{tool}");
        }
        assert!(!READ_ONLY_NESTED_TOOLS.contains(&"code_mode_exec"));
        assert!(!READ_ONLY_NESTED_TOOLS.contains(&"run_shell"));
    }

    #[test]
    fn nested_target_and_wrapper_fields_are_server_owned() {
        for field in [
            "project",
            "session_id",
            "recording_session_id",
            "ack_session_context_revision",
            "ack_session_message_ids",
            "context_request",
            "session_message_resolution",
            "expected_failure",
            "expected_failure_kind",
            "result_expectation",
            "accepted_exit_codes",
            "assertion_name",
        ] {
            assert!(FORBIDDEN_NESTED_FIELDS.contains(&field), "{field}");
        }
    }
}
