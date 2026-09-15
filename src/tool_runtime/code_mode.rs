use super::kernel::{
    HostFileImportTrust, ToolCallContext, ToolCallErrorStatus, ToolCallRequest, ToolTransport,
};
use super::{ResolvedProject, ToolResult, ToolRuntime};
use crate::auth::AuthContext;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use webcodex_code_mode::{
    CodeModeExecuteRequest, CodeModeHost, CodeModeHostError, CodeModeHostFuture, CodeModeStats,
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
    "git_review_summary",
    "show_changes",
];

pub(crate) fn is_admitted_nested_tool(tool_name: &str) -> bool {
    READ_ONLY_NESTED_TOOLS.contains(&tool_name)
}

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

/// Payload-free diagnostic summary for one outer Code Mode composition. This is
/// observability only and never participates in authority, routing, Session,
/// Window, retry, or idempotency decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct CodeModeCompositionSummary {
    pub(crate) nested_calls: usize,
    pub(crate) nested_successes: usize,
    pub(crate) nested_failures: usize,
    pub(crate) max_in_flight: usize,
    pub(crate) duration_ms: u64,
    pub(crate) returned_bytes: usize,
    pub(crate) nested_tool_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Default)]
struct CodeModeCompositionAccumulator {
    nested_calls: usize,
    nested_successes: usize,
    nested_failures: usize,
    in_flight: usize,
    max_in_flight: usize,
    nested_tool_counts: BTreeMap<String, usize>,
}

impl CodeModeCompositionAccumulator {
    fn begin_call(&mut self, tool_name: &str) -> usize {
        self.nested_calls = self.nested_calls.saturating_add(1);
        self.in_flight = self.in_flight.saturating_add(1);
        self.max_in_flight = self.max_in_flight.max(self.in_flight);
        *self
            .nested_tool_counts
            .entry(tool_name.to_string())
            .or_default() += 1;
        self.nested_calls
    }

    fn finish_call(&mut self, success: bool) {
        self.in_flight = self.in_flight.saturating_sub(1);
        if success {
            self.nested_successes = self.nested_successes.saturating_add(1);
        } else {
            self.nested_failures = self.nested_failures.saturating_add(1);
        }
    }

    fn summary(&self, stats: &CodeModeStats) -> CodeModeCompositionSummary {
        CodeModeCompositionSummary {
            nested_calls: self.nested_calls,
            nested_successes: self.nested_successes,
            nested_failures: self.nested_failures,
            max_in_flight: self.max_in_flight,
            duration_ms: stats.duration_ms,
            returned_bytes: stats.returned_bytes,
            nested_tool_counts: self.nested_tool_counts.clone(),
        }
    }
}

struct NestedCallGuard<'a> {
    composition: &'a Mutex<CodeModeCompositionAccumulator>,
    finished: bool,
}

impl NestedCallGuard<'_> {
    fn finish(mut self, success: bool) {
        self.composition
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish_call(success);
        self.finished = true;
    }
}

impl Drop for NestedCallGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.composition
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .finish_call(false);
        }
    }
}

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

pub(crate) struct RootCodeModeHost {
    tools: Arc<ToolRuntime>,
    auth: Option<AuthContext>,
    project: String,
    session_id: String,
    transport: ToolTransport,
    composition_parent_invocation_id: Option<String>,
    composition: Mutex<CodeModeCompositionAccumulator>,
}

impl RootCodeModeHost {
    pub(crate) fn new(
        tools: ToolRuntime,
        auth: Option<&AuthContext>,
        project: String,
        session_id: String,
        transport: ToolTransport,
        composition_parent_invocation_id: Option<String>,
    ) -> Self {
        Self {
            tools: Arc::new(tools),
            auth: auth.cloned(),
            project,
            session_id,
            transport,
            composition_parent_invocation_id,
            composition: Mutex::new(CodeModeCompositionAccumulator::default()),
        }
    }

    fn begin_nested_call(&self, tool_name: &str) -> (usize, NestedCallGuard<'_>) {
        let ordinal = self
            .composition
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .begin_call(tool_name);
        (
            ordinal,
            NestedCallGuard {
                composition: &self.composition,
                finished: false,
            },
        )
    }

    fn composition_summary(&self, stats: &CodeModeStats) -> CodeModeCompositionSummary {
        self.composition
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .summary(stats)
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
            let tool_name = request.tool_name.clone();
            let (child_ordinal, child_guard) = self.begin_nested_call(&tool_name);
            tracing::debug!(
                composition_parent_invocation_id = self
                    .composition_parent_invocation_id
                    .as_deref()
                    .unwrap_or("unavailable"),
                composition_child_ordinal = child_ordinal,
                nested_tool = tool_name.as_str(),
                "code_mode_nested_call_started"
            );
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
            let nested_success = outcome.error_status.is_none()
                && outcome.result.as_ref().is_some_and(|result| result.success);
            child_guard.finish(nested_success);
            tracing::debug!(
                composition_parent_invocation_id = self
                    .composition_parent_invocation_id
                    .as_deref()
                    .unwrap_or("unavailable"),
                composition_child_ordinal = child_ordinal,
                nested_tool = tool_name.as_str(),
                success = nested_success,
                "code_mode_nested_call_finished"
            );
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
        composition_parent_invocation_id: Option<String>,
    ) -> (ToolResult, CodeModeCompositionSummary) {
        let transport = match transport {
            super::sessions::SessionTransport::Api => ToolTransport::Api,
            super::sessions::SessionTransport::Mcp => ToolTransport::Mcp,
        };
        let host = Arc::new(RootCodeModeHost::new(
            self.clone(),
            auth,
            project.resolved_id,
            session_id,
            transport,
            composition_parent_invocation_id.clone(),
        ));
        let execution = webcodex_code_mode::execute(
            Arc::clone(&host) as Arc<dyn CodeModeHost>,
            CodeModeExecuteRequest {
                source,
                allowed_tools: READ_ONLY_NESTED_TOOLS
                    .iter()
                    .map(|tool| (*tool).to_string())
                    .collect(),
                timeout_ms,
            },
        )
        .await;
        let (result, stats) = match execution {
            Ok(execution) => {
                let stats = execution.stats.clone();
                (
                    ToolResult::ok(json!({
                        "content": execution.content,
                        "stats": execution.stats,
                    })),
                    stats,
                )
            }
            Err(error) => {
                let stats = error.stats.clone();
                (
                    ToolResult::err_with_output(
                        "code mode execution failed",
                        json!({
                            "failure_kind": error.kind.as_str(),
                            "message": bounded_model_error(&error.message),
                            "stats": error.stats,
                        }),
                    ),
                    stats,
                )
            }
        };
        let composition = host.composition_summary(&stats);
        super::runtime_metrics::observe_code_mode_composition(self.metrics.as_ref(), &composition);
        tracing::debug!(
            composition_parent_invocation_id = composition_parent_invocation_id
                .as_deref()
                .unwrap_or("unavailable"),
            nested_calls = composition.nested_calls,
            nested_successes = composition.nested_successes,
            nested_failures = composition.nested_failures,
            max_in_flight = composition.max_in_flight,
            duration_ms = composition.duration_ms,
            returned_bytes = composition.returned_bytes,
            "code_mode_composition_finished"
        );
        (result, composition)
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
        assert!(READ_ONLY_NESTED_TOOLS.contains(&"git_review_summary"));
        assert!(READ_ONLY_NESTED_TOOLS.contains(&"show_changes"));
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
