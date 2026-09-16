use super::context_projection::TOOL_CALL_CONTEXT_REQUEST_FIELD;
use super::kernel::{
    HostFileImportTrust, ToolCallContext, ToolCallErrorStatus, ToolCallRequest, ToolTransport,
};
use super::ToolRuntime;
use crate::auth::AuthContext;
use crate::json_measurement::serialized_json_len;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use webcodex_core::workflow_session_contract::{
    TOOL_ACCEPTED_EXIT_CODES_FIELD, TOOL_ASSERTION_NAME_FIELD,
    TOOL_CALL_ACK_SESSION_CONTEXT_REVISION_FIELD, TOOL_CALL_ACK_SESSION_MESSAGE_IDS_FIELD,
    TOOL_CALL_RECORDING_SESSION_ID_FIELD, TOOL_CALL_SESSION_MESSAGE_RESOLUTION_FIELD,
    TOOL_EXPECTED_FAILURE_FIELD, TOOL_EXPECTED_FAILURE_KIND_FIELD, TOOL_RESULT_EXPECTATION_FIELD,
};

/// Immutable admission and authority-shaping policy for one orchestration frontend.
///
/// This policy does not grant tool authority. It only narrows which nested calls
/// may re-enter the canonical ToolRuntime and which argument fields remain owned
/// by the outer trusted request context.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OrchestrationPolicy {
    pub(crate) frontend: &'static str,
    pub(crate) policy_name: &'static str,
    pub(crate) admitted_tools: &'static [&'static str],
    pub(crate) denied_tools: &'static [&'static str],
    /// Frontend-specific argument fields that are additionally reserved. The
    /// canonical Server-owned target/invocation fields below are always denied
    /// by the host and cannot be weakened by a frontend policy.
    pub(crate) additional_forbidden_argument_fields: &'static [&'static str],
}

impl OrchestrationPolicy {
    pub(crate) fn is_admitted(self, tool_name: &str) -> bool {
        !self.denied_tools.contains(&tool_name) && self.admitted_tools.contains(&tool_name)
    }
}

/// These fields are owned by the canonical orchestration boundary rather than
/// by an individual frontend program. A frontend may further narrow arguments,
/// but it cannot opt back into target selection, recorder/context metadata, or
/// result-expectation evidence shaping.
const SERVER_OWNED_ARGUMENT_FIELDS: &[&str] = &[
    "project",
    "session_id",
    TOOL_CALL_RECORDING_SESSION_ID_FIELD,
    TOOL_CALL_ACK_SESSION_MESSAGE_IDS_FIELD,
    TOOL_CALL_SESSION_MESSAGE_RESOLUTION_FIELD,
    TOOL_CALL_CONTEXT_REQUEST_FIELD,
    TOOL_CALL_ACK_SESSION_CONTEXT_REVISION_FIELD,
    TOOL_EXPECTED_FAILURE_FIELD,
    TOOL_EXPECTED_FAILURE_KIND_FIELD,
    TOOL_RESULT_EXPECTATION_FIELD,
    TOOL_ACCEPTED_EXIT_CODES_FIELD,
    TOOL_ASSERTION_NAME_FIELD,
];

pub(crate) fn is_server_owned_orchestration_argument(field: &str) -> bool {
    SERVER_OWNED_ARGUMENT_FIELDS.contains(&field) || field.starts_with("__webcodex_")
}

/// Payload-free diagnostic summary for one outer orchestration program. It is
/// observability only and never participates in authority, routing, Session,
/// Window, retry, or idempotency decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OrchestrationCompositionSummary {
    pub(crate) nested_calls: usize,
    pub(crate) nested_successes: usize,
    pub(crate) nested_failures: usize,
    pub(crate) max_in_flight: usize,
    pub(crate) duration_ms: u64,
    pub(crate) slot_wait_ms: u64,
    pub(crate) returned_bytes: usize,
    pub(crate) nested_raw_result_bytes_total: usize,
    pub(crate) nested_tool_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Default)]
struct OrchestrationCompositionAccumulator {
    nested_calls: usize,
    nested_successes: usize,
    nested_failures: usize,
    in_flight: usize,
    max_in_flight: usize,
    nested_raw_result_bytes_total: usize,
    nested_tool_counts: BTreeMap<String, usize>,
}

impl OrchestrationCompositionAccumulator {
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

    fn finish_call(&mut self, success: bool, raw_result_bytes: usize) {
        self.in_flight = self.in_flight.saturating_sub(1);
        self.nested_raw_result_bytes_total = self
            .nested_raw_result_bytes_total
            .saturating_add(raw_result_bytes);
        if success {
            self.nested_successes = self.nested_successes.saturating_add(1);
        } else {
            self.nested_failures = self.nested_failures.saturating_add(1);
        }
    }

    fn summary(
        &self,
        duration_ms: u64,
        returned_bytes: usize,
        slot_wait_ms: u64,
    ) -> OrchestrationCompositionSummary {
        OrchestrationCompositionSummary {
            nested_calls: self.nested_calls,
            nested_successes: self.nested_successes,
            nested_failures: self.nested_failures,
            max_in_flight: self.max_in_flight,
            duration_ms,
            slot_wait_ms,
            returned_bytes,
            nested_raw_result_bytes_total: self.nested_raw_result_bytes_total,
            nested_tool_counts: self.nested_tool_counts.clone(),
        }
    }
}

struct NestedCallGuard<'a> {
    composition: &'a Mutex<OrchestrationCompositionAccumulator>,
    finished: bool,
}

impl NestedCallGuard<'_> {
    fn finish(mut self, success: bool, raw_result_bytes: usize) {
        self.composition
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish_call(success, raw_result_bytes);
        self.finished = true;
    }
}

impl Drop for NestedCallGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.composition
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .finish_call(false, 0);
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OrchestrationToolResponse {
    pub(crate) success: bool,
    pub(crate) output: Value,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrchestrationHostError {
    message: String,
}

impl OrchestrationHostError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn into_message(self) -> String {
        self.message
    }
}

/// Canonical nested-tool host shared by orchestration frontends.
///
/// The host owns no new authority or effect semantics. It freezes the exact outer
/// Project/Session/auth/transport context, applies a frontend-specific admission
/// policy, injects server-owned target fields, and then re-enters the same
/// ToolRuntime path used by direct model calls. Frontend runtimes such as V8 Code
/// Mode continue to own program evaluation, scheduling/concurrency bounds, timeout
/// and cancellation, and final output shaping; this host owns the canonical child
/// invocation boundary they share.
pub(crate) struct CanonicalOrchestrationHost {
    tools: Arc<ToolRuntime>,
    auth: Option<AuthContext>,
    project: String,
    session_id: String,
    transport: ToolTransport,
    composition_parent_invocation_id: Option<String>,
    policy: OrchestrationPolicy,
    composition: Mutex<OrchestrationCompositionAccumulator>,
}

impl CanonicalOrchestrationHost {
    pub(crate) fn new(
        tools: ToolRuntime,
        auth: Option<&AuthContext>,
        project: String,
        session_id: String,
        transport: ToolTransport,
        composition_parent_invocation_id: Option<String>,
        policy: OrchestrationPolicy,
    ) -> Self {
        Self {
            tools: Arc::new(tools),
            auth: auth.cloned(),
            project,
            session_id,
            transport,
            composition_parent_invocation_id,
            policy,
            composition: Mutex::new(OrchestrationCompositionAccumulator::default()),
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

    pub(crate) fn composition_summary(
        &self,
        duration_ms: u64,
        returned_bytes: usize,
        slot_wait_ms: u64,
    ) -> OrchestrationCompositionSummary {
        self.composition
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .summary(duration_ms, returned_bytes, slot_wait_ms)
    }

    fn prepare_arguments(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<Value, OrchestrationHostError> {
        if !self.policy.is_admitted(tool_name) {
            return Err(OrchestrationHostError::new(format!(
                "nested tool `{tool_name}` is not admitted by {}",
                self.policy.policy_name
            )));
        }
        let Some(mut arguments) = arguments.as_object().cloned() else {
            return Err(OrchestrationHostError::new(
                "nested tool arguments must be a JSON object",
            ));
        };
        if let Some(field) = arguments
            .keys()
            .find(|field| is_server_owned_orchestration_argument(field))
        {
            return Err(OrchestrationHostError::new(format!(
                "nested tool arguments may not set server-owned field `{field}`"
            )));
        }
        if let Some(field) = self
            .policy
            .additional_forbidden_argument_fields
            .iter()
            .find(|field| arguments.contains_key(**field))
        {
            return Err(OrchestrationHostError::new(format!(
                "nested tool arguments may not set frontend-reserved field `{field}`"
            )));
        }
        arguments.insert("project".to_string(), Value::String(self.project.clone()));
        arguments.insert(
            "session_id".to_string(),
            Value::String(self.session_id.clone()),
        );
        Ok(Value::Object(arguments))
    }

    pub(crate) async fn invoke_tool(
        &self,
        tool_name: String,
        arguments: Value,
    ) -> Result<OrchestrationToolResponse, OrchestrationHostError> {
        let arguments = self.prepare_arguments(&tool_name, arguments)?;
        let (child_ordinal, child_guard) = self.begin_nested_call(&tool_name);
        tracing::debug!(
            orchestration_frontend = self.policy.frontend,
            composition_parent_invocation_id = self
                .composition_parent_invocation_id
                .as_deref()
                .unwrap_or("unavailable"),
            composition_child_ordinal = child_ordinal,
            nested_tool = tool_name.as_str(),
            "orchestration_nested_call_started"
        );
        let outcome = self
            .tools
            .call_tool_with_context(
                ToolCallRequest {
                    tool_name: tool_name.clone(),
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
        let raw_result_bytes = outcome
            .result
            .as_ref()
            .and_then(|result| serialized_json_len(result).ok())
            .unwrap_or(0);
        child_guard.finish(nested_success, raw_result_bytes);
        tracing::debug!(
            orchestration_frontend = self.policy.frontend,
            composition_parent_invocation_id = self
                .composition_parent_invocation_id
                .as_deref()
                .unwrap_or("unavailable"),
            composition_child_ordinal = child_ordinal,
            nested_tool = tool_name.as_str(),
            success = nested_success,
            "orchestration_nested_call_finished"
        );
        if let Some(error_status) = outcome.error_status {
            let message = match error_status {
                ToolCallErrorStatus::InvalidArguments { message } => message,
                ToolCallErrorStatus::InsufficientScope { description, .. } => description,
            };
            return Err(OrchestrationHostError::new(message));
        }
        let result = outcome.result.ok_or_else(|| {
            OrchestrationHostError::new("canonical ToolRuntime returned no nested ToolResult")
        })?;
        Ok(OrchestrationToolResponse {
            success: result.success,
            output: result.output,
            error: result.error,
        })
    }
}
