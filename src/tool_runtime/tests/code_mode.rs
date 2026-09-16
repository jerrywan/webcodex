//! Feature-gated integration evidence for Experimental Code Mode E1.

use super::support::*;
use crate::tool_runtime::kernel::{
    HostFileImportTrust, ToolCallContext, ToolCallOutcome, ToolCallRequest, ToolTransport,
};
use crate::tool_runtime::orchestration_host::{CanonicalOrchestrationHost, OrchestrationPolicy};
use crate::tool_runtime::ToolRuntime;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug)]
struct ObservedRunnerRequest {
    client_id: String,
    cwd: Option<String>,
}

async fn call_code_mode_with_local_runners(
    runtime: &ToolRuntime,
    clients: &[&str],
    project: &str,
    session_id: &str,
    source: &str,
) -> (ToolCallOutcome, Vec<ObservedRunnerRequest>) {
    let runtime_for_task = runtime.clone();
    let project = project.to_string();
    let session_id_owned = session_id.to_string();
    let source = source.to_string();
    let task = tokio::spawn(async move {
        let auth = bootstrap_auth_context();
        runtime_for_task
            .call_tool_with_context(
                ToolCallRequest {
                    tool_name: "code_mode_exec".to_string(),
                    arguments: json!({
                        "project": project,
                        "session_id": session_id_owned,
                        "source": source,
                        "timeout_ms": 5_000,
                    }),
                },
                ToolCallContext {
                    transport: ToolTransport::Mcp,
                    session_id: Some(&session_id_owned),
                    auth: Some(&auth),
                    window: None,
                    record_oauth_scope_denials: true,
                    host_file_import_trust: HostFileImportTrust::Untrusted,
                },
            )
            .await
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut observed = Vec::new();
    while !task.is_finished() {
        assert!(
            Instant::now() < deadline,
            "Code Mode integration call did not finish within the test deadline"
        );
        let mut made_progress = false;
        for client_id in clients {
            let request = runtime
                .runner_registry
                .poll(crate::runner_protocol::RunnerPollRequest {
                    client_id: (*client_id).to_string(),
                    runner_instance_id: "inst".to_string(),
                })
                .await
                .unwrap();
            let Some(request) = request else {
                continue;
            };
            made_progress = true;
            observed.push(ObservedRunnerRequest {
                client_id: (*client_id).to_string(),
                cwd: request.cwd.clone(),
            });
            complete_agent_request_by_running_locally(runtime, client_id, request).await;
        }
        if !made_progress {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }
    (task.await.unwrap(), observed)
}

fn init_git_repo(path: &std::path::Path) {
    let output = std::process::Command::new("git")
        .arg("init")
        .arg("--quiet")
        .arg(path)
        .output()
        .expect("git init");
    assert!(output.status.success(), "git init failed: {output:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn canonical_orchestration_host_runs_without_the_v8_frontend() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("README.md"), "frontend-independent host\n").unwrap();

    let runtime = test_runtime();
    let client_id = "orchestration-host-direct";
    let exact_project =
        register_runner_project_at_path(&runtime, client_id, "demo", tmp.path()).await;
    let session = runtime.sessions.start_session(
        Some(exact_project.clone()),
        Some("orchestration host direct test".to_string()),
    );
    let auth = bootstrap_auth_context();
    let policy = OrchestrationPolicy {
        frontend: "test_structured_plan",
        policy_name: "test structured plan",
        admitted_tools: &["read_files"],
        denied_tools: &[],
        additional_forbidden_argument_fields: &[],
    };
    let host = Arc::new(CanonicalOrchestrationHost::new(
        runtime.clone(),
        Some(&auth),
        exact_project.clone(),
        session.session_id.clone(),
        ToolTransport::Mcp,
        Some("test-parent".to_string()),
        policy,
    ));
    let host_for_task = Arc::clone(&host);
    let task = tokio::spawn(async move {
        host_for_task
            .invoke_tool(
                "read_files".to_string(),
                json!({"items": [{"path": "README.md", "start_line": 1, "limit": 20}]}),
            )
            .await
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while !task.is_finished() {
        assert!(
            Instant::now() < deadline,
            "direct orchestration host call did not finish within the test deadline"
        );
        let request = runtime
            .runner_registry
            .poll(crate::runner_protocol::RunnerPollRequest {
                client_id: client_id.to_string(),
                runner_instance_id: "inst".to_string(),
            })
            .await
            .unwrap();
        if let Some(request) = request {
            complete_agent_request_by_running_locally(&runtime, client_id, request).await;
        } else {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    let response = task.await.unwrap().expect("canonical nested read");
    assert!(response.success, "{response:?}");
    let composition = host.composition_summary(17, 123, 5);
    assert_eq!(composition.nested_calls, 1);
    assert_eq!(composition.nested_successes, 1);
    assert_eq!(composition.nested_failures, 0);
    assert_eq!(composition.max_in_flight, 1);
    assert_eq!(composition.duration_ms, 17);
    assert_eq!(composition.slot_wait_ms, 5);
    assert_eq!(composition.returned_bytes, 123);
    assert!(composition.nested_raw_result_bytes_total > 0);
    assert_eq!(composition.nested_tool_counts.get("read_files"), Some(&1));

    let summary = runtime
        .sessions
        .summary(&session.session_id, Some(20))
        .expect("session summary");
    let read_start = summary
        .events
        .iter()
        .find(|event| {
            event.kind == "tool_call_started"
                && event.tool_name == "read_files"
                && event.logical_invocation_role.as_deref() == Some("business")
        })
        .expect("canonical child business evidence");
    assert_eq!(read_start.session_id, session.session_id);
    assert_eq!(
        read_start.resolved_project.as_deref(),
        Some(exact_project.as_str())
    );
    let input_summary = read_start
        .input_summary
        .as_ref()
        .expect("canonical child input summary");
    assert_eq!(input_summary["project"], exact_project);
    assert!(summary
        .events
        .iter()
        .all(|event| event.tool_name != "code_mode_exec"));
}

#[tokio::test]
async fn canonical_orchestration_host_rejects_server_owned_metadata_without_frontend_help() {
    let runtime = test_runtime();
    let session = runtime
        .sessions
        .start_session(None, Some("orchestration metadata guard".to_string()));
    let policy = OrchestrationPolicy {
        frontend: "test_structured_plan",
        policy_name: "test structured plan",
        admitted_tools: &["read_files"],
        denied_tools: &[],
        additional_forbidden_argument_fields: &[],
    };
    let host = CanonicalOrchestrationHost::new(
        runtime,
        None,
        "agent:unused:demo".to_string(),
        session.session_id,
        ToolTransport::Mcp,
        Some("test-parent".to_string()),
        policy,
    );

    for (field, value) in [
        ("project", json!("agent:other:demo")),
        ("session_id", json!("wc_sess_0000000000000000")),
        ("recording_session_id", json!("wc_sess_0000000000000000")),
        ("ack_session_context_revision", json!(1)),
        ("ack_session_message_ids", json!([])),
        ("context_request", json!(["webcodex.workflow"])),
        (
            "session_message_resolution",
            json!({"message_id": "wc_msg_0000000000000000", "resolution": "handled"}),
        ),
        ("expected_failure", json!(true)),
        ("expected_failure_kind", json!("anything")),
        ("result_expectation", json!("failure")),
        ("accepted_exit_codes", json!([0, 1])),
        ("assertion_name", json!("nested-assertion")),
        ("__webcodex_private", json!(true)),
    ] {
        let mut arguments = serde_json::Map::new();
        arguments.insert(field.to_string(), value);
        arguments.insert("items".to_string(), json!([{"path": "README.md"}]));
        let error = host
            .invoke_tool("read_files".to_string(), Value::Object(arguments))
            .await
            .expect_err("server-owned nested metadata must fail before canonical dispatch");
        assert!(error.into_message().contains(field), "{field}");
    }
    let composition = host.composition_summary(0, 0, 0);
    assert_eq!(composition.nested_calls, 0);
    assert!(composition.nested_tool_counts.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_mode_binds_exact_project_and_session_through_real_canonical_reads() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    std::fs::write(
        tmp.path().join("README.md"),
        "WebCodex Code Mode integration fixture\nToolRuntime\n",
    )
    .unwrap();

    let runtime = test_runtime();
    let client_id = "code-mode-exact";
    let exact_project =
        register_runner_project_at_path(&runtime, client_id, "demo", tmp.path()).await;
    let session = runtime.sessions.start_session(
        Some(exact_project.clone()),
        Some("code mode integration".to_string()),
    );

    // The outer caller deliberately uses the project shorthand. The root resolver
    // must bind it once and nested calls must receive the exact resolved id.
    let source = r#"
        const [status, hits] = await Promise.all([
            tools.git_status({}),
            tools.search_project_texts({
                queries: [{
                    pattern: "ToolRuntime",
                    pattern_mode: "literal",
                    result_mode: "files_with_matches",
                    limit: 10
                }]
            })
        ]);
        const files = await tools.read_files({
            items: [{path: "README.md", start_line: 1, limit: 20}]
        });
        text({
            status_success: status.success,
            search_success: hits.success,
            read_success: files.success
        });
    "#;
    let (outcome, observed) = call_code_mode_with_local_runners(
        &runtime,
        &[client_id],
        "demo",
        &session.session_id,
        source,
    )
    .await;
    assert!(outcome.success, "{outcome:?}");
    let composition = outcome
        .correlation
        .code_mode_composition
        .clone()
        .expect("outer Code Mode composition diagnostic");
    assert_eq!(composition.nested_calls, 3);
    assert_eq!(composition.nested_successes, 3);
    assert_eq!(composition.nested_failures, 0);
    assert!(composition.max_in_flight >= 2);
    let mut nested_tools = composition
        .nested_tool_counts
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    nested_tools.sort_unstable();
    assert_eq!(
        nested_tools,
        ["git_status", "read_files", "search_project_texts"]
    );
    assert_eq!(composition.nested_tool_counts.values().sum::<usize>(), 3);
    assert!(composition.nested_raw_result_bytes_total > composition.returned_bytes);
    assert!(composition.slot_wait_ms <= composition.duration_ms);
    assert!(composition
        .nested_tool_counts
        .keys()
        .all(|tool| super::super::code_mode::is_admitted_nested_tool(tool)));
    let diagnostic = serde_json::to_string(&composition).unwrap();
    for private in [
        "ToolRuntime",
        "README.md",
        tmp.path().to_string_lossy().as_ref(),
    ] {
        assert!(
            !diagnostic.contains(private),
            "composition diagnostic leaked nested private text: {diagnostic}"
        );
    }
    let result = outcome.result.expect("code_mode_exec ToolResult");
    assert!(result.success, "{:?}", result.error);
    assert!(result.output.get("content").is_some(), "{result:?}");
    assert!(result.output.get("stats").is_some(), "{result:?}");
    assert!(result.output.get("nested_results").is_none(), "{result:?}");
    assert!(result.output.get("tool_results").is_none(), "{result:?}");
    let mut stats_keys = result.output["stats"]
        .as_object()
        .expect("sparse Code Mode stats")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    stats_keys.sort_unstable();
    assert_eq!(
        stats_keys,
        [
            "duration_ms",
            "max_in_flight",
            "returned_bytes",
            "tool_calls"
        ]
    );
    assert!(result.output.get("code_mode_composition").is_none());
    let emitted: Value = serde_json::from_str(
        result.output["content"][0]
            .as_str()
            .expect("one text() emission"),
    )
    .unwrap();
    assert_eq!(emitted["status_success"], true);
    assert_eq!(emitted["search_success"], true);
    assert_eq!(emitted["read_success"], true);
    assert_eq!(result.output["stats"]["tool_calls"], 3);
    assert!(result.output["stats"]["max_in_flight"].as_u64().unwrap() >= 2);
    assert!(
        observed
            .iter()
            .all(|request| request.client_id == client_id),
        "{observed:?}"
    );
    assert!(
        observed
            .iter()
            .all(|request| request.cwd.as_deref() == Some(tmp.path().to_string_lossy().as_ref())),
        "{observed:?}"
    );

    let summary = runtime
        .sessions
        .summary(&session.session_id, Some(40))
        .expect("session summary");
    for tool_name in [
        "code_mode_exec",
        "git_status",
        "search_project_texts",
        "read_files",
    ] {
        let started = summary
            .events
            .iter()
            .find(|event| {
                event.kind == "tool_call_started"
                    && event.tool_name == tool_name
                    && event.logical_invocation_role.as_deref() == Some("business")
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing {tool_name} business start event: {:?}",
                    summary.events
                )
            });
        assert_eq!(started.session_id, session.session_id, "{tool_name}");
        assert_eq!(
            started.resolved_project.as_deref(),
            Some(exact_project.as_str()),
            "{tool_name}"
        );
        if tool_name != "code_mode_exec" {
            let input_summary = started
                .input_summary
                .as_ref()
                .unwrap_or_else(|| panic!("missing {tool_name} input summary"));
            assert_eq!(
                input_summary["project"], exact_project,
                "nested {tool_name} must receive the exact resolved Project id"
            );
        }
    }
    let business_invocation_ids = summary
        .events
        .iter()
        .filter(|event| {
            event.kind == "tool_call_started"
                && event.logical_invocation_role.as_deref() == Some("business")
                && [
                    "code_mode_exec",
                    "git_status",
                    "search_project_texts",
                    "read_files",
                ]
                .contains(&event.tool_name.as_str())
        })
        .filter_map(|event| event.logical_invocation_id.as_deref())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        business_invocation_ids.len(),
        4,
        "outer and each canonical child must retain independent invocation evidence"
    );
    let outer_start = summary
        .events
        .iter()
        .find(|event| {
            event.kind == "tool_call_started"
                && event.tool_name == "code_mode_exec"
                && event.logical_invocation_role.as_deref() == Some("business")
        })
        .unwrap();
    let audit = outer_start
        .input_summary
        .as_ref()
        .expect("outer audit summary");
    assert_eq!(audit["project"], "demo");
    assert_eq!(audit["source_bytes"], source.len());
    assert!(!audit.to_string().contains("ToolRuntime"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_mode_rejects_nested_target_override_before_runner_dispatch() {
    let root_a = tempfile::tempdir().unwrap();
    let root_b = tempfile::tempdir().unwrap();
    std::fs::write(root_a.path().join("README.md"), "alpha\n").unwrap();
    std::fs::write(root_b.path().join("README.md"), "bravo\n").unwrap();
    let runtime = test_runtime();
    let project_a =
        register_runner_project_at_path(&runtime, "code-mode-a", "alpha", root_a.path()).await;
    let project_b =
        register_runner_project_at_path(&runtime, "code-mode-b", "bravo", root_b.path()).await;
    let session = runtime.sessions.start_session(
        Some(project_a.clone()),
        Some("code mode target guard".to_string()),
    );
    let source = format!(
        "await tools.read_files({{project: {project_b:?}, items: [{{path: 'README.md'}}]}});"
    );
    let (outcome, observed) = call_code_mode_with_local_runners(
        &runtime,
        &["code-mode-a", "code-mode-b"],
        "alpha",
        &session.session_id,
        &source,
    )
    .await;
    let result = outcome.result.expect("outer ToolResult");
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("code mode execution failed"));
    assert!(
        result.output["message"]
            .as_str()
            .unwrap_or_default()
            .contains("server-owned field `project`"),
        "{result:?}"
    );
    assert!(
        observed.is_empty(),
        "override dispatched Runner work: {observed:?}"
    );
    let summary = runtime
        .sessions
        .summary(&session.session_id, Some(20))
        .expect("session summary");
    assert!(!summary
        .events
        .iter()
        .any(|event| event.tool_name == "read_files"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_mode_failure_detail_is_bounded_without_persisting_source_derived_text() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = test_runtime();
    let client_id = "code-mode-error-privacy";
    let project = register_runner_project_at_path(&runtime, client_id, "demo", tmp.path()).await;
    let session = runtime
        .sessions
        .start_session(Some(project), Some("code mode error privacy".to_string()));
    let (outcome, observed) = call_code_mode_with_local_runners(
        &runtime,
        &[client_id],
        "demo",
        &session.session_id,
        "throw('PRIVATE_RUNTIME_DETAIL_'.repeat(2000));",
    )
    .await;
    assert!(observed.is_empty());
    let result = outcome.result.expect("outer ToolResult");
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("code mode execution failed"));
    let detail = result.output["message"]
        .as_str()
        .expect("bounded runtime detail");
    assert!(detail.starts_with("PRIVATE_RUNTIME_DETAIL_"));
    assert!(detail.len() <= super::super::code_mode::MAX_MODEL_ERROR_BYTES);
    assert_eq!(result.output["failure_kind"], "runtime_error");

    let summary = runtime
        .sessions
        .summary(&session.session_id, Some(20))
        .expect("session summary");
    let finished = summary
        .events
        .iter()
        .find(|event| {
            event.kind == "tool_call_finished"
                && event.tool_name == "code_mode_exec"
                && event.logical_invocation_role.as_deref() == Some("business")
        })
        .expect("business finish event");
    assert_eq!(
        finished.error_message_summary.as_deref(),
        Some("code mode execution failed")
    );
    assert!(!format!("{finished:?}").contains("PRIVATE_RUNTIME_DETAIL_"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn code_mode_does_not_admit_effectful_or_recursive_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = test_runtime();
    let project =
        register_runner_project_at_path(&runtime, "code-mode-effects", "demo", tmp.path()).await;
    for (label, source) in [
        (
            "run_shell",
            "await tools.run_shell({command: 'echo forbidden'});",
        ),
        (
            "recursive",
            "await tools.code_mode_exec({source: `text('nested')`});",
        ),
    ] {
        let session = runtime
            .sessions
            .start_session(Some(project.clone()), Some(format!("code mode {label}")));
        let (outcome, observed) = call_code_mode_with_local_runners(
            &runtime,
            &["code-mode-effects"],
            "demo",
            &session.session_id,
            source,
        )
        .await;
        let result = outcome.result.expect("outer ToolResult");
        assert!(!result.success, "{label}: {result:?}");
        assert!(
            observed.is_empty(),
            "{label} dispatched Runner work: {observed:?}"
        );
        let summary = runtime
            .sessions
            .summary(&session.session_id, Some(20))
            .expect("session summary");
        assert!(summary.events.iter().all(|event| {
            event.tool_name != "run_shell"
                && !(event.tool_name == "code_mode_exec"
                    && event
                        .input_summary
                        .as_ref()
                        .and_then(|value| value.get("source"))
                        .is_some())
        }));
        let outer_starts = summary
            .events
            .iter()
            .filter(|event| {
                event.tool_name == "code_mode_exec" && event.kind == "tool_call_started"
            })
            .collect::<Vec<_>>();
        assert_eq!(outer_starts.len(), 2, "{label}: {outer_starts:?}");
        assert!(
            outer_starts.iter().all(|event| {
                event
                    .input_summary
                    .as_ref()
                    .and_then(|value| value.get("source"))
                    .is_none()
            }),
            "{label}: JavaScript source entered durable audit evidence"
        );
        assert!(
            outer_starts
                .iter()
                .any(|event| { event.logical_invocation_role.as_deref() == Some("recorder") }),
            "{label}"
        );
        assert!(
            outer_starts
                .iter()
                .any(|event| { event.logical_invocation_role.as_deref() == Some("business") }),
            "{label}"
        );
    }
}
