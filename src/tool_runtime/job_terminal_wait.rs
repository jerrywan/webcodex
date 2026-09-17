use super::{ToolResult, ToolRuntime};
use crate::auth::AuthContext;
use crate::job_terminal_attention::{
    fact_from_event, metric as attention_metric, principal_for_auth, source_from_snapshot,
    JobTerminalDeliveryAttempt,
};
use serde_json::json;
use webcodex_store::{JobTerminalWaitRecord, JobTerminalWaitState, NewJobTerminalWait};

#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct JobTerminalRegistrationTestHook {
    pub(crate) first_snapshot: std::sync::Arc<tokio::sync::Barrier>,
    pub(crate) resume_after_terminal: std::sync::Arc<tokio::sync::Barrier>,
}

#[cfg(test)]
impl JobTerminalRegistrationTestHook {
    pub(crate) fn new() -> Self {
        Self {
            first_snapshot: std::sync::Arc::new(tokio::sync::Barrier::new(2)),
            resume_after_terminal: std::sync::Arc::new(tokio::sync::Barrier::new(2)),
        }
    }
}

impl ToolRuntime {
    pub(crate) async fn wait_for_job_terminal(
        &self,
        job_id: String,
        idempotency_key: String,
        auth: Option<&AuthContext>,
    ) -> ToolResult {
        let Some(db) = self.job_terminal_db.as_ref() else {
            return unavailable();
        };
        let Some(controller) = self.job_terminal_continuations.as_ref() else {
            return unavailable();
        };

        let access = crate::runner_http::runner_access_from_auth(auth);
        let first = match self
            .runner_registry
            .job_terminal_registration_snapshot_for_auth(access.as_ref(), &job_id)
            .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return ToolResult::err_with_output(error, json!({"error_kind": "unknown_job"}))
            }
        };
        #[cfg(test)]
        if let Some(hook) = &self.job_terminal_registration_test_hook {
            hook.first_snapshot.wait().await;
            hook.resume_after_terminal.wait().await;
        }
        let principal = principal_for_auth(auth);
        let source = source_from_snapshot(&first);
        let now = chrono::Utc::now().timestamp();
        let already_terminal = first.terminal_event.as_ref().map(fact_from_event);
        let mutation = match db.create_job_terminal_wait(
            &principal,
            NewJobTerminalWait {
                source: source.clone(),
                idempotency_key,
                expires_at: first.wait_expires_at,
                already_terminal,
            },
            now,
        ) {
            Ok(mutation) => mutation,
            Err(error) => return store_failure(error.code),
        };

        if mutation.replayed {
            attention_metric("registration_replayed");
        } else {
            attention_metric("armed");
            if mutation.wait.state == JobTerminalWaitState::Triggered {
                attention_metric("already_terminal_immediate_match");
            }
        }

        let mut state_changed = mutation.state_changed;
        let mut wait = mutation.wait;

        // Registration/terminalization race handshake. The first snapshot proves
        // visibility and exact source identity; the durable insert closes the
        // waiting side; this second canonical snapshot closes the already-terminal
        // side. The post-lock Runner sink handles all later terminal transitions.
        if wait.state == JobTerminalWaitState::Waiting {
            let second = match self
                .runner_registry
                .job_terminal_registration_snapshot_for_auth(access.as_ref(), &job_id)
                .await
            {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    return ToolResult::err_with_output(
                        error,
                        json!({
                            "error_kind": "unknown_job",
                            "wait_id": wait.wait_id,
                        }),
                    )
                }
            };
            if source_from_snapshot(&second) != source {
                return ToolResult::err_with_output(
                    "Job identity changed while terminal attention was being armed",
                    json!({
                        "error_kind": "job_identity_changed",
                        "wait_id": wait.wait_id,
                    }),
                );
            }
            if let Some(event) = second.terminal_event.as_ref() {
                match db.match_job_terminal_fact(
                    &fact_from_event(event),
                    chrono::Utc::now().timestamp(),
                ) {
                    Ok(matched) => {
                        state_changed |= matched.matched_count > 0;
                        for (owner, candidate_wait_id) in matched.delivery_candidates {
                            let _ = controller.attempt_delivery(
                                &owner,
                                &candidate_wait_id,
                                chrono::Utc::now().timestamp(),
                            );
                        }
                    }
                    Err(error) => return store_failure(error.code),
                }
            }
        }

        wait = match db.read_job_terminal_wait(
            &principal,
            &wait.wait_id,
            chrono::Utc::now().timestamp(),
        ) {
            Ok(wait) => wait,
            Err(error) => return store_failure(error.code),
        };

        if wait.state == JobTerminalWaitState::Triggered {
            match controller.attempt_delivery(
                &principal,
                &wait.wait_id,
                chrono::Utc::now().timestamp(),
            ) {
                Ok(
                    JobTerminalDeliveryAttempt::Delivered
                    | JobTerminalDeliveryAttempt::DeliveryUnknown,
                ) => {
                    state_changed = true;
                }
                Ok(
                    JobTerminalDeliveryAttempt::NoCarrier
                    | JobTerminalDeliveryAttempt::PreflightFailed
                    | JobTerminalDeliveryAttempt::Deduplicated,
                ) => {}
                Err(error) => return store_failure(error.code),
            }
            wait = match db.read_job_terminal_wait(
                &principal,
                &wait.wait_id,
                chrono::Utc::now().timestamp(),
            ) {
                Ok(wait) => wait,
                Err(error) => return store_failure(error.code),
            };
        }

        ToolResult::ok(wait_output(
            &wait,
            mutation.replayed,
            state_changed,
            controller.automatic_resume_available(),
        ))
    }
}

fn wait_output(
    wait: &JobTerminalWaitRecord,
    replayed: bool,
    state_changed: bool,
    automatic_resume_available: bool,
) -> serde_json::Value {
    json!({
        "wait_id": wait.wait_id,
        "job_id": wait.source.job_id,
        "state": wait.state.as_str(),
        "delivery_state": wait.delivery_state.as_str(),
        "terminal_status": wait.terminal_status,
        "terminal_outcome": wait.terminal_outcome,
        "replayed": replayed,
        "state_changed": state_changed,
        "automatic_resume_available": automatic_resume_available,
        "expires_at": wait.expires_at,
        "fallback_tool": "observe_jobs",
    })
}

fn unavailable() -> ToolResult {
    ToolResult::err_with_output(
        "Job terminal attention is not configured",
        json!({"error_kind": "job_terminal_attention_unavailable"}),
    )
}

fn store_failure(code: &'static str) -> ToolResult {
    ToolResult::err_with_output(
        "Job terminal attention operation failed",
        json!({"error_kind": code}),
    )
}
