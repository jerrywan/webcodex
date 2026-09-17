//! E3 generic Job terminal attention contract and registration-race coverage.

use super::super::*;
use super::support::*;
use crate::job_terminal_attention::{principal_for_auth, JobTerminalContinuationController};
use crate::runner_protocol::{RunnerCapabilities, RunnerJobUpdateRequest, RunnerRequest};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use webcodex_store::{JobTerminalDeliveryState, JobTerminalWaitState};

async fn attention_runtime() -> (TempDir, ToolRuntime, Arc<crate::Database>) {
    let temp = tempfile::tempdir().unwrap();
    let db =
        Arc::new(crate::Database::open(&temp.path().join("job-terminal-attention.db")).unwrap());
    let controller = JobTerminalContinuationController::new(db.clone());
    let registry = Arc::new(
        crate::job_receipts::production_registry_with_terminal_attention(
            db.clone(),
            controller.clone(),
        )
        .await,
    );
    let runtime = ToolRuntime::new(registry, Arc::new(RuntimeInfo::default()))
        .with_job_terminal_attention(db.clone(), controller);
    (temp, runtime, db)
}

async fn start_owned_job(
    runtime: &ToolRuntime,
    client_id: &str,
    project_id: &str,
    auth: &crate::auth::AuthContext,
) -> (String, RunnerRequest) {
    let caps = RunnerCapabilities {
        async_jobs: true,
        async_shell_jobs: true,
        ..Default::default()
    };
    register_agent_projects_for_auth(
        runtime,
        client_id,
        auth,
        caps,
        vec![registered_project(
            project_id,
            &format!("/tmp/{project_id}"),
        )],
    )
    .await;
    let started = runtime
        .dispatch_with_auth(
            ToolCall::RunJob {
                project: format!("agent:{client_id}:{project_id}"),
                command: format!("echo {client_id}"),
                session_id: None,
                timeout_secs: Some(60),
                cwd: None,
                purpose: None,
                shell: None,
            },
            Some(auth),
        )
        .await;
    assert!(started.success, "{:?}", started.error);
    let job_id = started.output["job_id"].as_str().unwrap().to_string();
    let request = wait_for_runner_request_for_client(runtime, client_id).await;
    assert_eq!(request.job_id.as_deref(), Some(job_id.as_str()));
    (job_id, request)
}

async fn complete_job(
    runtime: &ToolRuntime,
    client_id: &str,
    request: &RunnerRequest,
    update_seq: Option<u64>,
) {
    runtime
        .runner_registry
        .update_job(RunnerJobUpdateRequest {
            client_id: client_id.to_string(),
            runner_instance_id: format!("inst-{client_id}"),
            update_seq,
            job_id: request.job_id.clone().expect("Job request id"),
            request_id: Some(request.request_id.clone()),
            status: "completed".to_string(),
            stdout_chunk: Some("done\n".to_string()),
            stderr_chunk: None,
            stdout_tail: None,
            stderr_tail: None,
            log_snapshot: None,
            exit_code: Some(0),
            duration_ms: Some(25),
            error: None,
            command_execution_state: None,
            validation_progress: None,
            test_count_evidence: None,
            activity: None,
            finished: true,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn active_job_arms_then_terminalizes_durably_without_observe_jobs_polling() {
    let (_temp, runtime, db) = attention_runtime().await;
    let hash = "a".repeat(64);
    let auth = shared_key_auth_context(&hash);
    let (job_id, request) = start_owned_job(&runtime, "e3-active", "project-a", &auth).await;

    let armed = runtime
        .dispatch_with_auth(
            ToolCall::WaitForJobTerminal {
                job_id: job_id.clone(),
                idempotency_key: "e3-active-arm".to_string(),
            },
            Some(&auth),
        )
        .await;
    assert!(armed.success, "{:?}", armed.error);
    assert_eq!(armed.output["job_id"], job_id);
    assert_eq!(armed.output["state"], "waiting");
    assert_eq!(armed.output["delivery_state"], "not_ready");
    assert_eq!(armed.output["automatic_resume_available"], false);
    assert_eq!(armed.output["fallback_tool"], "observe_jobs");
    for forbidden in ["stdout", "stderr", "command", "cwd", "environment"] {
        assert!(armed.output.get(forbidden).is_none(), "leaked {forbidden}");
    }

    // Canonical Runner terminal truth is sufficient: the test intentionally does
    // not call observe_jobs between the Job handoff and the durable terminal fact.
    complete_job(&runtime, "e3-active", &request, Some(1)).await;

    let principal = principal_for_auth(Some(&auth));
    let wait_id = armed.output["wait_id"].as_str().unwrap();
    let stored = db
        .read_job_terminal_wait(&principal, wait_id, chrono::Utc::now().timestamp())
        .unwrap();
    assert_eq!(stored.state, JobTerminalWaitState::Triggered);
    assert_eq!(stored.delivery_state, JobTerminalDeliveryState::Pending);
    assert_eq!(stored.terminal_status.as_deref(), Some("completed"));
    assert_eq!(stored.terminal_outcome.as_deref(), Some("succeeded"));
}

#[tokio::test]
async fn already_terminal_registration_is_immediate_and_keyed_replay_is_stable() {
    let (_temp, runtime, _db) = attention_runtime().await;
    let auth = shared_key_auth_context(&"b".repeat(64));
    let (job_id, request) = start_owned_job(&runtime, "e3-terminal", "project-b", &auth).await;
    complete_job(&runtime, "e3-terminal", &request, Some(1)).await;

    let first = runtime
        .dispatch_with_auth(
            ToolCall::WaitForJobTerminal {
                job_id: job_id.clone(),
                idempotency_key: "e3-after-terminal".to_string(),
            },
            Some(&auth),
        )
        .await;
    assert!(first.success, "{:?}", first.error);
    assert_eq!(first.output["state"], "triggered");
    assert_eq!(first.output["delivery_state"], "pending");
    assert_eq!(first.output["terminal_status"], "completed");
    assert_eq!(first.output["replayed"], false);

    let replay = runtime
        .dispatch_with_auth(
            ToolCall::WaitForJobTerminal {
                job_id,
                idempotency_key: "e3-after-terminal".to_string(),
            },
            Some(&auth),
        )
        .await;
    assert!(replay.success, "{:?}", replay.error);
    assert_eq!(replay.output["wait_id"], first.output["wait_id"]);
    assert_eq!(replay.output["replayed"], true);
    assert_eq!(replay.output["automatic_resume_available"], false);
}

#[tokio::test]
async fn unauthorized_registration_is_existence_hiding_and_session_window_are_not_authority() {
    let (_temp, runtime, _db) = attention_runtime().await;
    let auth_a = shared_key_auth_context(&"c".repeat(64));
    let auth_b = shared_key_auth_context(&"d".repeat(64));
    let (job_id, _request) = start_owned_job(&runtime, "e3-owner", "project-c", &auth_a).await;

    let denied = runtime
        .dispatch_with_auth(
            ToolCall::WaitForJobTerminal {
                job_id: job_id.clone(),
                idempotency_key: "foreign-arm".to_string(),
            },
            Some(&auth_b),
        )
        .await;
    let missing = runtime
        .dispatch_with_auth(
            ToolCall::WaitForJobTerminal {
                job_id: "missing-e3-job".to_string(),
                idempotency_key: "missing-arm".to_string(),
            },
            Some(&auth_b),
        )
        .await;
    assert!(!denied.success);
    assert!(!missing.success);
    assert_eq!(denied.output["error_kind"], "unknown_job");
    assert_eq!(missing.output["error_kind"], "unknown_job");

    // Session/window/observation fields do not exist in the E3 registration
    // contract and therefore cannot become visibility or execution authority.
    let spec = crate::tool_runtime::registered_tool_specs()
        .into_iter()
        .find(|spec| spec.name == "wait_for_job_terminal")
        .unwrap();
    assert_eq!(spec.input_schema["additionalProperties"], false);
    for field in ["job_id", "idempotency_key"] {
        assert_eq!(spec.input_schema["properties"][field]["minLength"], 1);
        assert_eq!(spec.input_schema["properties"][field]["maxLength"], 128);
    }
    for absent in ["session_id", "client_window", "after_observation_token"] {
        assert!(spec.input_schema["properties"].get(absent).is_none());
    }
    let old_token_error = ToolCall::from_tool_name(
        "wait_for_job_terminal",
        json!({
            "job_id": job_id.clone(),
            "idempotency_key": "old-token-must-not-participate",
            "after_observation_token": "obsolete-process-local-cursor"
        }),
    )
    .expect_err("obsolete observation cursors must fail closed instead of being ignored");
    assert!(
        old_token_error.contains("unknown field `after_observation_token`"),
        "{old_token_error}"
    );
}

#[tokio::test]
async fn retained_terminal_receipt_reconciles_waiting_registration_after_store_reopen() {
    let (temp, runtime, db) = attention_runtime().await;
    let db_path = temp.path().join("job-terminal-attention.db");
    let auth = shared_key_auth_context(&"f".repeat(64));
    let (job_id, request) = start_owned_job(&runtime, "e3-restart", "project-restart", &auth).await;
    complete_job(&runtime, "e3-restart", &request, Some(1)).await;

    let access = crate::runner_http::runner_access_from_auth(Some(&auth));
    let snapshot = runtime
        .runner_registry
        .job_terminal_registration_snapshot_for_auth(access.as_ref(), &job_id)
        .await
        .unwrap();
    let source = crate::job_terminal_attention::source_from_snapshot(&snapshot);
    let principal = principal_for_auth(Some(&auth));
    let now = chrono::Utc::now().timestamp();
    let waiting = db
        .create_job_terminal_wait(
            &principal,
            webcodex_store::NewJobTerminalWait {
                source,
                idempotency_key: "e3-restart-wait".to_string(),
                expires_at: snapshot.wait_expires_at,
                already_terminal: None,
            },
            now,
        )
        .unwrap();
    assert_eq!(waiting.wait.state, JobTerminalWaitState::Waiting);
    let wait_id = waiting.wait.wait_id.clone();

    drop(runtime);
    drop(db);
    let reopened = Arc::new(crate::Database::open(&db_path).unwrap());
    reopened
        .recover_job_terminal_deliveries_after_restart(chrono::Utc::now().timestamp())
        .unwrap();
    let controller = JobTerminalContinuationController::new(reopened.clone());
    let _registry = crate::job_receipts::production_registry_with_terminal_attention(
        reopened.clone(),
        controller,
    )
    .await;

    let restored = reopened
        .read_job_terminal_wait(&principal, &wait_id, chrono::Utc::now().timestamp())
        .unwrap();
    assert_eq!(restored.state, JobTerminalWaitState::Triggered);
    assert_eq!(restored.delivery_state, JobTerminalDeliveryState::Pending);
    assert_eq!(restored.terminal_status.as_deref(), Some("completed"));
    assert_eq!(restored.terminal_outcome.as_deref(), Some("succeeded"));
}
#[tokio::test]
async fn registration_concurrent_with_terminalization_has_exactly_one_triggered_result() {
    let (_temp, runtime, db) = attention_runtime().await;
    let hook = super::super::job_terminal_wait::JobTerminalRegistrationTestHook::new();
    let first_snapshot = hook.first_snapshot.clone();
    let resume = hook.resume_after_terminal.clone();
    let runtime = Arc::new(runtime.with_job_terminal_registration_test_hook(hook));
    let auth = shared_key_auth_context(&"e".repeat(64));
    let (job_id, request) = start_owned_job(&runtime, "e3-race", "project-race", &auth).await;

    let runtime_for_arm = runtime.clone();
    let auth_for_arm = auth.clone();
    let job_for_arm = job_id.clone();
    let arm = tokio::spawn(async move {
        runtime_for_arm
            .dispatch_with_auth(
                ToolCall::WaitForJobTerminal {
                    job_id: job_for_arm,
                    idempotency_key: "e3-race-arm".to_string(),
                },
                Some(&auth_for_arm),
            )
            .await
    });

    first_snapshot.wait().await;
    complete_job(&runtime, "e3-race", &request, Some(1)).await;
    resume.wait().await;

    let armed = arm.await.unwrap();
    assert!(armed.success, "{:?}", armed.error);
    assert_eq!(armed.output["state"], "triggered");
    assert_eq!(armed.output["delivery_state"], "pending");

    let principal = principal_for_auth(Some(&auth));
    let wait_id = armed.output["wait_id"].as_str().unwrap();
    let stored = db
        .read_job_terminal_wait(&principal, wait_id, chrono::Utc::now().timestamp())
        .unwrap();
    assert_eq!(stored.state, JobTerminalWaitState::Triggered);
    assert_eq!(stored.delivery_state, JobTerminalDeliveryState::Pending);
}
