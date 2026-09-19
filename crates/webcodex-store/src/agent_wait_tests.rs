use super::agent_task::{AgentTaskAttemptStartMutation, NewAgentTask};
use super::agent_wait::*;
use super::agent_wake::AgentWakeState;
use super::communication::{
    CommunicationPrincipal, NewAgentEndpoint, NewAgentIdentity,
    COMMUNICATION_PRINCIPAL_DIGEST_PREFIX,
};
use super::Database;

fn principal(hex: char) -> CommunicationPrincipal {
    CommunicationPrincipal {
        kind: "user".to_string(),
        digest: format!(
            "{COMMUNICATION_PRINCIPAL_DIGEST_PREFIX}{}",
            hex.to_string().repeat(64)
        ),
    }
}

fn agent(db: &Database, owner: &CommunicationPrincipal, label: &str) -> String {
    db.create_agent_identity(
        owner,
        NewAgentIdentity {
            handle: label.to_string(),
            display_name: label.to_string(),
            description: String::new(),
            specialty_labels: Vec::new(),
            idempotency_key: format!("create-{label}"),
        },
    )
    .unwrap()
    .agent
    .agent_id
}

fn endpoint(
    db: &Database,
    owner: &CommunicationPrincipal,
    agent_id: &str,
    label: &str,
) -> super::communication::AgentEndpointRecord {
    db.attach_agent_endpoint(
        owner,
        NewAgentEndpoint {
            agent_id: agent_id.to_string(),
            host: "ChatGPT".to_string(),
            client_attachment_id: Some(label.to_string()),
            wake_capable: true,
            idempotency_key: format!("endpoint-{label}"),
        },
    )
    .unwrap()
    .endpoint
}

fn task(
    db: &Database,
    owner: &CommunicationPrincipal,
    assignee_agent_id: &str,
    label: &str,
) -> String {
    db.create_agent_task(
        owner,
        NewAgentTask {
            title: format!("Task {label}"),
            instruction: format!("PRIVATE instruction {label}"),
            assignee_agent_id: Some(assignee_agent_id.to_string()),
            source_conversation_id: None,
            source_message_id: None,
            referenced_project_id: Some(format!("agent:special:private-{label}")),
            idempotency_key: format!("task-{label}"),
        },
    )
    .unwrap()
    .task
    .summary
    .task_id
}

fn start(
    db: &Database,
    owner: &CommunicationPrincipal,
    task_id: &str,
    assignee_agent_id: &str,
    label: &str,
) -> AgentTaskAttemptStartMutation {
    db.start_agent_task_attempt(
        owner,
        task_id,
        assignee_agent_id,
        &format!("attempt-{label}"),
    )
    .unwrap()
}

fn complete(
    db: &Database,
    owner: &CommunicationPrincipal,
    task_id: &str,
    assignee_agent_id: &str,
    started: &AgentTaskAttemptStartMutation,
    label: &str,
) {
    db.complete_agent_task_attempt(
        owner,
        task_id,
        &started.attempt.attempt_id,
        assignee_agent_id,
        &started.attempt_fence,
        started.attempt.attempt_controller_generation,
        super::agent_task::AgentTaskState::Succeeded,
        Some(&format!("PRIVATE terminal result {label}")),
        Some(&format!("PRIVATE terminal reason {label}")),
        &format!("complete-{label}"),
    )
    .unwrap();
}

fn wait_input(
    target_agent_id: &str,
    endpoint: &super::communication::AgentEndpointRecord,
    task_ids: &[String],
    key: &str,
) -> NewAgentWait {
    wait_input_mode(target_agent_id, endpoint, task_ids, key, AgentWaitMode::Any)
}

fn wait_input_mode(
    target_agent_id: &str,
    endpoint: &super::communication::AgentEndpointRecord,
    task_ids: &[String],
    key: &str,
    mode: AgentWaitMode,
) -> NewAgentWait {
    NewAgentWait {
        target_agent_id: target_agent_id.to_string(),
        endpoint_id: endpoint.endpoint_id.clone(),
        expected_controller_generation: endpoint.controller_generation,
        mode,
        events: task_ids
            .iter()
            .map(|task_id| AgentWaitEventSelector {
                kind: AGENT_WAIT_EVENT_KIND_AGENT_TASK_TERMINAL.to_string(),
                task_id: task_id.clone(),
            })
            .collect(),
        idempotency_key: key.to_string(),
    }
}

fn wait_wake_id(db: &Database, wait_id: &str) -> String {
    db.conn_for_tests()
        .query_row(
            "SELECT wake_id FROM wc_agent_wakes WHERE source_wait_id = ?1",
            [wait_id],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn create_wait_is_exact_keyed_private_and_snapshots_already_terminal_sources() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-create.db")).unwrap();
    let owner = principal('1');
    let watcher = agent(&db, &owner, "wait-create-watcher");
    let worker = agent(&db, &owner, "wait-create-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-create-view");
    let task_a = task(&db, &owner, &worker, "wait-create-a");
    let task_b = task(&db, &owner, &worker, "wait-create-b");
    let a = start(&db, &owner, &task_a, &worker, "wait-create-a");
    let b = start(&db, &owner, &task_b, &worker, "wait-create-b");
    complete(&db, &owner, &task_a, &worker, &a, "wait-create-a");
    complete(&db, &owner, &task_b, &worker, &b, "wait-create-b");

    let input = wait_input(
        &watcher,
        &endpoint,
        &[task_a.clone(), task_b.clone()],
        "wait-create-key",
    );
    let created = db.create_agent_wait(&owner, input.clone()).unwrap();
    assert_eq!(created.agent_wait.state, AgentWaitState::Triggered);
    assert_eq!(created.agent_wait.source_count, 2);
    assert_eq!(created.agent_wait.match_count, 2);
    assert!(created.schedule_required);
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1 AND state = 'pending'",
                [created.agent_wait.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "multiple already-terminal selectors must create exactly one queueable Wake"
    );

    let replay = db.create_agent_wait(&owner, input).unwrap();
    assert!(replay.replayed);
    assert!(!replay.state_changed);
    assert_eq!(replay.agent_wait.wait_id, created.agent_wait.wait_id);

    let mut changed = wait_input(&watcher, &endpoint, &[task_a], "wait-create-key");
    changed.expected_controller_generation = endpoint.controller_generation;
    let conflict = db.create_agent_wait(&owner, changed).unwrap_err();
    assert_eq!(conflict.code(), "communication_idempotency_conflict");

    let serialized = serde_json::to_string(&created.agent_wait).unwrap();
    for private in [
        "PRIVATE instruction",
        "PRIVATE terminal result",
        "PRIVATE terminal reason",
        "agent:special:private-",
    ] {
        assert!(!serialized.contains(private));
    }
}

#[test]
fn wait_source_authority_is_independent_and_foreign_task_is_existence_hidden() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-authority.db")).unwrap();
    let alice = principal('2');
    let bob = principal('3');
    let alice_agent = agent(&db, &alice, "wait-authority-alice");
    let bob_agent = agent(&db, &bob, "wait-authority-bob");
    let endpoint = endpoint(&db, &alice, &alice_agent, "wait-authority-view");
    let foreign_task = task(&db, &bob, &bob_agent, "wait-authority-foreign");
    let foreign = db
        .create_agent_wait(
            &alice,
            wait_input(
                &alice_agent,
                &endpoint,
                &[foreign_task],
                "wait-authority-foreign",
            ),
        )
        .unwrap_err();
    let missing = db
        .create_agent_wait(
            &alice,
            wait_input(
                &alice_agent,
                &endpoint,
                &["wc_agent_task_________________".to_string()],
                "wait-authority-missing",
            ),
        )
        .unwrap_err();
    assert_eq!(foreign.code(), "agent_task_not_found");
    assert_eq!(missing.code(), "agent_task_not_found");
    assert_eq!(foreign.message(), missing.message());
}

#[test]
fn future_matches_coalesce_only_before_prepare_and_exact_consume_resumes_once() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-coalesce.db")).unwrap();
    let owner = principal('4');
    let watcher = agent(&db, &owner, "wait-coalesce-watcher");
    let worker = agent(&db, &owner, "wait-coalesce-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-coalesce-view");
    let task_a = task(&db, &owner, &worker, "wait-coalesce-a");
    let task_b = task(&db, &owner, &worker, "wait-coalesce-b");
    let a = start(&db, &owner, &task_a, &worker, "wait-coalesce-a");
    let b = start(&db, &owner, &task_b, &worker, "wait-coalesce-b");
    let created = db
        .create_agent_wait(
            &owner,
            wait_input(
                &watcher,
                &endpoint,
                &[task_a.clone(), task_b.clone()],
                "wait-coalesce",
            ),
        )
        .unwrap();
    assert_eq!(created.agent_wait.state, AgentWaitState::Waiting);

    complete(&db, &owner, &task_a, &worker, &a, "wait-coalesce-a");
    let wake_id = wait_wake_id(&db, &created.agent_wait.wait_id);
    let first = db.agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(first.wait_match_count_snapshot, Some(1));
    let explicit = db
        .accept_explicit_agent_wake_activation(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &wake_id,
            "wait-coalesce-explicit-activation",
        )
        .unwrap_err();
    assert_eq!(
        explicit.code(),
        "agent_wait_wake_requires_endpoint_dispatch"
    );
    let claim = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    assert_eq!(claim.wake.wake_id, wake_id);

    complete(&db, &owner, &task_b, &worker, &b, "wait-coalesce-b");
    let coalesced = db.agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(coalesced.state, AgentWakeState::Claimed);
    assert_eq!(coalesced.wait_match_count_snapshot, Some(2));
    assert!(coalesced.revision > first.revision);

    let prepared = db
        .prepare_agent_wake_dispatch(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &wake_id,
            &claim.attempt.attempt_id,
            &claim.claim_fence,
            &claim.consume_token,
        )
        .unwrap();
    assert!(prepared
        .envelope
        .resume_hint
        .contains(&created.agent_wait.wait_id));
    assert!(prepared.envelope.resume_hint.contains("mode=any"));
    assert!(prepared.envelope.resume_hint.contains("matched=2/2"));
    let hint = &prepared.envelope.resume_hint;
    for required in [
        "agent_id=",
        "endpoint_id=",
        "controller_generation=",
        "wake_id=",
        "consume_token=",
        "wait_id=",
        "mode=any",
        "matched=2/2",
        "match_sequence=",
        "bootstrap_agent_conversation",
        "consume_agent_wake",
        "read_agent_wait(wait_id)",
        "authoritative source AgentTasks",
        "one-shot",
    ] {
        assert!(
            hint.contains(required),
            "missing AgentWait continuation detail {required}"
        );
    }
    assert!(
        hint.chars().count() <= 1_000,
        "AgentWait hint too long: {}",
        hint.chars().count()
    );
    for removed_prose in [
        "already dispatched by the Endpoint continuation carrier",
        "grants no Task, Project, Goal",
        "OMIT activation_idempotency_key",
    ] {
        assert!(!hint.contains(removed_prose));
    }
    assert!(!hint.contains("PRIVATE"));

    let consumed = db
        .consume_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &wake_id,
            &claim.consume_token,
        )
        .unwrap();
    assert!(!consumed.already_consumed);
    assert_eq!(
        db.read_agent_wait(&owner, &created.agent_wait.wait_id)
            .unwrap()
            .state,
        AgentWaitState::Resumed
    );
    let late_ack = db
        .complete_agent_wake_delivery(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &wake_id,
            &claim.attempt.attempt_id,
            &claim.claim_fence,
        )
        .unwrap();
    assert_eq!(late_ack.state, AgentWakeState::Consumed);
    assert_eq!(
        db.read_agent_wait(&owner, &created.agent_wait.wait_id)
            .unwrap()
            .state,
        AgentWaitState::Resumed,
        "a late Host ACK after exact consume must remain idempotent and never regress the one-shot Wait"
    );
    let replay = db
        .consume_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &wake_id,
            &claim.consume_token,
        )
        .unwrap();
    assert!(replay.already_consumed);
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [created.agent_wait.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
}

#[test]
fn prepared_wait_batch_is_sealed_and_post_fence_cancel_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-sealed.db")).unwrap();
    let owner = principal('5');
    let watcher = agent(&db, &owner, "wait-sealed-watcher");
    let worker = agent(&db, &owner, "wait-sealed-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-sealed-view");
    let task_a = task(&db, &owner, &worker, "wait-sealed-a");
    let task_b = task(&db, &owner, &worker, "wait-sealed-b");
    let a = start(&db, &owner, &task_a, &worker, "wait-sealed-a");
    let b = start(&db, &owner, &task_b, &worker, "wait-sealed-b");
    let wait = db
        .create_agent_wait(
            &owner,
            wait_input(
                &watcher,
                &endpoint,
                &[task_a.clone(), task_b.clone()],
                "wait-sealed",
            ),
        )
        .unwrap()
        .agent_wait;
    complete(&db, &owner, &task_a, &worker, &a, "wait-sealed-a");
    let wake_id = wait_wake_id(&db, &wait.wait_id);
    let claim = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    db.prepare_agent_wake_dispatch(
        &owner,
        &watcher,
        &endpoint.endpoint_id,
        endpoint.controller_generation,
        &wake_id,
        &claim.attempt.attempt_id,
        &claim.claim_fence,
        &claim.consume_token,
    )
    .unwrap();
    let sealed = db.agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(sealed.wait_match_count_snapshot, Some(1));

    complete(&db, &owner, &task_b, &worker, &b, "wait-sealed-b");
    let after = db.agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(after.state, AgentWakeState::Prepared);
    assert_eq!(after.wait_match_count_snapshot, Some(1));
    assert_eq!(
        db.read_agent_wait(&owner, &wait.wait_id)
            .unwrap()
            .match_count,
        2
    );
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [wait.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "sealed one-shot Wait must not manufacture a successor model opportunity"
    );

    let cancel = db
        .cancel_agent_wait(&owner, &wait.wait_id, "wait-sealed-cancel")
        .unwrap_err();
    assert_eq!(cancel.code(), "agent_wait_dispatch_fence_crossed");
    assert_eq!(
        db.read_agent_wait(&owner, &wait.wait_id).unwrap().state,
        AgentWaitState::Triggered
    );
}

#[test]
fn cancellation_is_keyed_and_retires_only_predispatch_wait_wake() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-cancel.db")).unwrap();
    let owner = principal('6');
    let watcher = agent(&db, &owner, "wait-cancel-watcher");
    let worker = agent(&db, &owner, "wait-cancel-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-cancel-view");
    let task_id = task(&db, &owner, &worker, "wait-cancel-task");
    let started = start(&db, &owner, &task_id, &worker, "wait-cancel-task");
    let wait = db
        .create_agent_wait(
            &owner,
            wait_input(&watcher, &endpoint, &[task_id.clone()], "wait-cancel"),
        )
        .unwrap()
        .agent_wait;
    complete(&db, &owner, &task_id, &worker, &started, "wait-cancel-task");
    let wake_id = wait_wake_id(&db, &wait.wait_id);
    let claim = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    let cancelled = db
        .cancel_agent_wait(&owner, &wait.wait_id, "wait-cancel-key")
        .unwrap();
    assert_eq!(cancelled.agent_wait.state, AgentWaitState::Cancelled);
    assert_eq!(
        db.agent_wake(&wake_id).unwrap().unwrap().state,
        AgentWakeState::Retired
    );
    assert_eq!(
        db.agent_wake_attempts(&wake_id).unwrap()[0].state,
        super::agent_wake::AgentWakeAttemptState::Revoked
    );
    let replay = db
        .cancel_agent_wait(&owner, &wait.wait_id, "wait-cancel-key")
        .unwrap();
    assert!(replay.replayed);
    assert!(!replay.state_changed);
    assert_eq!(
        claim.wake.source_wait_id.as_deref(),
        Some(wait.wait_id.as_str())
    );
}

#[test]
fn wait_and_pending_wake_survive_reopen_and_can_resume_on_new_endpoint() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("wait-restart.db");
    let owner = principal('7');
    let (watcher, worker, wait_id) = {
        let db = Database::open(&path).unwrap();
        let watcher = agent(&db, &owner, "wait-restart-watcher");
        let worker = agent(&db, &owner, "wait-restart-worker");
        let endpoint = endpoint(&db, &owner, &watcher, "wait-restart-view-a");
        let task_id = task(&db, &owner, &worker, "wait-restart-task");
        let started = start(&db, &owner, &task_id, &worker, "wait-restart-task");
        let wait = db
            .create_agent_wait(
                &owner,
                wait_input(&watcher, &endpoint, &[task_id.clone()], "wait-restart"),
            )
            .unwrap()
            .agent_wait;
        complete(
            &db,
            &owner,
            &task_id,
            &worker,
            &started,
            "wait-restart-task",
        );
        (watcher, worker, wait.wait_id)
    };
    let db = Database::open(&path).unwrap();
    assert_eq!(
        db.read_agent_wait(&owner, &wait_id).unwrap().state,
        AgentWaitState::Triggered
    );
    let replacement = endpoint(&db, &owner, &watcher, "wait-restart-view-b");
    let claim = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &replacement.endpoint_id,
            replacement.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    assert_eq!(claim.wake.source_wait_id.as_deref(), Some(wait_id.as_str()));
    let prepared = db
        .prepare_agent_wake_dispatch(
            &owner,
            &watcher,
            &replacement.endpoint_id,
            replacement.controller_generation,
            &claim.wake.wake_id,
            &claim.attempt.attempt_id,
            &claim.claim_fence,
            &claim.consume_token,
        )
        .unwrap();
    assert!(prepared.envelope.resume_hint.contains(&wait_id));
    db.consume_agent_wake(
        &owner,
        &watcher,
        &replacement.endpoint_id,
        replacement.controller_generation,
        &claim.wake.wake_id,
        &claim.consume_token,
    )
    .unwrap();
    assert_eq!(
        db.read_agent_wait(&owner, &wait_id).unwrap().state,
        AgentWaitState::Resumed
    );
    assert!(!worker.is_empty());
}

#[test]
fn wait_creation_enforces_selector_agent_and_endpoint_bounds() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-bounds.db")).unwrap();
    let owner = principal('9');
    let watcher = agent(&db, &owner, "wait-bounds-watcher");
    let worker = agent(&db, &owner, "wait-bounds-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-bounds-view");
    let empty = db
        .create_agent_wait(
            &owner,
            wait_input(&watcher, &endpoint, &[], "wait-bounds-empty"),
        )
        .unwrap_err();
    assert_eq!(empty.code(), "invalid_agent_wait_events");

    let nine = (0..9)
        .map(|index| {
            format!(
                "wc_agent_task_{}",
                webcodex_core::compact::encode(&(index as u128).to_be_bytes()[4..])
            )
        })
        .collect::<Vec<_>>();
    let oversized = db
        .create_agent_wait(
            &owner,
            wait_input(&watcher, &endpoint, &nine, "wait-bounds-nine"),
        )
        .unwrap_err();
    assert_eq!(oversized.code(), "invalid_agent_wait_events");

    let real_task = task(&db, &owner, &worker, "wait-bounds-real");
    let mut stale = wait_input(
        &watcher,
        &endpoint,
        std::slice::from_ref(&real_task),
        "wait-bounds-stale",
    );
    stale.expected_controller_generation += 1;
    assert!(db.create_agent_wait(&owner, stale).is_err());

    for index in 0..MAX_ACTIVE_AGENT_WAITS_PER_AGENT {
        let task_id = task(&db, &owner, &worker, &format!("wait-bounds-agent-{index}"));
        db.create_agent_wait(
            &owner,
            wait_input(
                &watcher,
                &endpoint,
                std::slice::from_ref(&task_id),
                &format!("wait-bounds-agent-{index}"),
            ),
        )
        .unwrap();
    }
    let overflow_task = task(&db, &owner, &worker, "wait-bounds-agent-overflow");
    let overflow = db
        .create_agent_wait(
            &owner,
            wait_input(
                &watcher,
                &endpoint,
                std::slice::from_ref(&overflow_task),
                "wait-bounds-agent-overflow",
            ),
        )
        .unwrap_err();
    assert_eq!(overflow.code(), "agent_wait_agent_capacity_reached");
}

#[test]
fn wait_and_task_wakes_share_the_existing_one_dispatched_wake_fence() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-competition.db")).unwrap();
    let owner = principal('a');
    let watcher = agent(&db, &owner, "wait-competition-watcher");
    let worker = agent(&db, &owner, "wait-competition-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-competition-view");

    let source_task = task(&db, &owner, &worker, "wait-competition-source");
    let source_attempt = start(
        &db,
        &owner,
        &source_task,
        &worker,
        "wait-competition-source",
    );
    let wait = db
        .create_agent_wait(
            &owner,
            wait_input(
                &watcher,
                &endpoint,
                std::slice::from_ref(&source_task),
                "wait-competition-wait",
            ),
        )
        .unwrap()
        .agent_wait;
    complete(
        &db,
        &owner,
        &source_task,
        &worker,
        &source_attempt,
        "wait-competition-source",
    );

    let task_wake_task = task(&db, &owner, &watcher, "wait-competition-task-wake");
    let task_wake_attempt = start(
        &db,
        &owner,
        &task_wake_task,
        &watcher,
        "wait-competition-task-wake",
    );
    db.start_agent_task_endpoint_continuation(
        &owner,
        &task_wake_task,
        &task_wake_attempt.attempt.attempt_id,
        &watcher,
        &task_wake_attempt.attempt_fence,
        task_wake_attempt.attempt.attempt_controller_generation,
    )
    .unwrap();

    let first = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    let second = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    assert_ne!(first.wake.wake_id, second.wake.wake_id);
    assert!(
        [
            first.wake.source_wait_id.as_deref(),
            second.wake.source_wait_id.as_deref()
        ]
        .contains(&Some(wait.wait_id.as_str())),
        "one claimed Wake must be the Wait-origin opportunity"
    );

    db.prepare_agent_wake_dispatch(
        &owner,
        &watcher,
        &endpoint.endpoint_id,
        endpoint.controller_generation,
        &first.wake.wake_id,
        &first.attempt.attempt_id,
        &first.claim_fence,
        &first.consume_token,
    )
    .unwrap();
    assert!(db
        .prepare_agent_wake_dispatch(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &second.wake.wake_id,
            &second.attempt.attempt_id,
            &second.claim_fence,
            &second.consume_token,
        )
        .is_err());
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes
                 WHERE target_agent_id = ?1 AND state IN ('prepared', 'delivered', 'delivery_unknown')",
                [watcher.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "Wait-origin Wakes must reuse the existing Agent-global durable dispatch fence"
    );
}

#[test]
fn all_wait_records_partial_matches_without_wake_and_triggers_once_on_final_match() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-all-final.db")).unwrap();
    let owner = principal('b');
    let watcher = agent(&db, &owner, "wait-all-final-watcher");
    let worker = agent(&db, &owner, "wait-all-final-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-all-final-view");
    let task_a = task(&db, &owner, &worker, "wait-all-final-a");
    let task_b = task(&db, &owner, &worker, "wait-all-final-b");
    let a = start(&db, &owner, &task_a, &worker, "wait-all-final-a");
    let b = start(&db, &owner, &task_b, &worker, "wait-all-final-b");

    let created = db
        .create_agent_wait(
            &owner,
            wait_input_mode(
                &watcher,
                &endpoint,
                &[task_a.clone(), task_b.clone()],
                "wait-all-final",
                AgentWaitMode::All,
            ),
        )
        .unwrap();
    assert_eq!(created.agent_wait.mode, AgentWaitMode::All);
    assert_eq!(created.agent_wait.state, AgentWaitState::Waiting);
    assert_eq!(created.agent_wait.match_count, 0);
    assert!(!created.schedule_required);
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [created.agent_wait.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );

    complete(&db, &owner, &task_a, &worker, &a, "wait-all-final-a");
    let partial = db
        .read_agent_wait(&owner, &created.agent_wait.wait_id)
        .unwrap();
    assert_eq!(partial.state, AgentWaitState::Waiting);
    assert_eq!(partial.match_count, 1);
    assert!(partial.revision > created.agent_wait.revision);
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [partial.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0,
        "partial ALL match must never create a queueable Wake"
    );

    {
        let mut conn = db.conn_for_tests();
        let transaction = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let duplicate = record_agent_task_terminal_wait_matches_in_transaction(
            &transaction,
            &owner,
            &task_a,
            &a.attempt.attempt_id,
            super::agent_task::AgentTaskState::Succeeded,
            partial.updated_at_unix_ms + 1,
        )
        .unwrap();
        assert!(duplicate.schedule_agent_ids.is_empty());
        transaction.commit().unwrap();
    }
    let after_duplicate = db.read_agent_wait(&owner, &partial.wait_id).unwrap();
    assert_eq!(after_duplicate.match_count, 1);
    assert_eq!(after_duplicate.revision, partial.revision);

    complete(&db, &owner, &task_b, &worker, &b, "wait-all-final-b");
    let final_wait = db.read_agent_wait(&owner, &partial.wait_id).unwrap();
    assert_eq!(final_wait.state, AgentWaitState::Triggered);
    assert_eq!(final_wait.match_count, 2);
    let wake_id = wait_wake_id(&db, &final_wait.wait_id);
    let wake = db.agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(wake.wait_match_count_snapshot, Some(2));
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [final_wait.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );

    {
        let mut conn = db.conn_for_tests();
        let transaction = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let duplicate = record_agent_task_terminal_wait_matches_in_transaction(
            &transaction,
            &owner,
            &task_b,
            &b.attempt.attempt_id,
            super::agent_task::AgentTaskState::Succeeded,
            final_wait.updated_at_unix_ms + 1,
        )
        .unwrap();
        assert!(duplicate.schedule_agent_ids.is_empty());
        transaction.commit().unwrap();
    }
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [final_wait.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "duplicate terminal reconciliation must not create another Wake"
    );
}

#[test]
fn all_wait_registration_snapshots_mixed_and_complete_terminal_sets_atomically() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-all-registration.db")).unwrap();
    let owner = principal('c');
    let watcher = agent(&db, &owner, "wait-all-registration-watcher");
    let worker = agent(&db, &owner, "wait-all-registration-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-all-registration-view");

    let task_a = task(&db, &owner, &worker, "wait-all-registration-a");
    let task_b = task(&db, &owner, &worker, "wait-all-registration-b");
    let a = start(&db, &owner, &task_a, &worker, "wait-all-registration-a");
    let _b = start(&db, &owner, &task_b, &worker, "wait-all-registration-b");
    complete(&db, &owner, &task_a, &worker, &a, "wait-all-registration-a");
    let mixed = db
        .create_agent_wait(
            &owner,
            wait_input_mode(
                &watcher,
                &endpoint,
                &[task_a.clone(), task_b],
                "wait-all-registration-mixed",
                AgentWaitMode::All,
            ),
        )
        .unwrap();
    assert_eq!(mixed.agent_wait.state, AgentWaitState::Waiting);
    assert_eq!(mixed.agent_wait.match_count, 1);
    assert!(!mixed.schedule_required);
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [mixed.agent_wait.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );

    let task_c = task(&db, &owner, &worker, "wait-all-registration-c");
    let task_d = task(&db, &owner, &worker, "wait-all-registration-d");
    let c = start(&db, &owner, &task_c, &worker, "wait-all-registration-c");
    let d = start(&db, &owner, &task_d, &worker, "wait-all-registration-d");
    complete(&db, &owner, &task_c, &worker, &c, "wait-all-registration-c");
    complete(&db, &owner, &task_d, &worker, &d, "wait-all-registration-d");
    let complete_set = db
        .create_agent_wait(
            &owner,
            wait_input_mode(
                &watcher,
                &endpoint,
                &[task_c, task_d],
                "wait-all-registration-complete",
                AgentWaitMode::All,
            ),
        )
        .unwrap();
    assert_eq!(complete_set.agent_wait.state, AgentWaitState::Triggered);
    assert_eq!(complete_set.agent_wait.match_count, 2);
    assert!(complete_set.schedule_required);
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [complete_set.agent_wait.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "all-terminal registration must produce exactly one Wake inside the registration transaction"
    );
}

#[test]
fn any_request_hash_stays_v1_compatible_and_same_key_all_conflicts() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-idempotency-mode.db")).unwrap();
    let owner = principal('d');
    let watcher = agent(&db, &owner, "wait-idempotency-watcher");
    let worker = agent(&db, &owner, "wait-idempotency-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-idempotency-view");
    let task_id = task(&db, &owner, &worker, "wait-idempotency-task");
    let input = wait_input(
        &watcher,
        &endpoint,
        std::slice::from_ref(&task_id),
        "wait-idempotency-mode",
    );
    let created = db.create_agent_wait(&owner, input.clone()).unwrap();
    let replay = db.create_agent_wait(&owner, input.clone()).unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.agent_wait.wait_id, created.agent_wait.wait_id);

    let expected_v1_hash = super::communication::digest_json(
        "webcodex.agent-wait.request.v1",
        &serde_json::json!({
            "agent_id": input.target_agent_id,
            "endpoint_id": input.endpoint_id,
            "expected_controller_generation": input.expected_controller_generation,
            "events": input.events,
        }),
    )
    .unwrap();
    let stored_hash: String = db
        .conn_for_tests()
        .query_row(
            "SELECT request_hash FROM wc_communication_idempotency WHERE resource_id = ?1",
            [created.agent_wait.wait_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        stored_hash, expected_v1_hash,
        "mode=any must retain the exact production v1 request-hash identity"
    );

    let conflict = db
        .create_agent_wait(
            &owner,
            wait_input_mode(
                &watcher,
                &endpoint,
                std::slice::from_ref(&task_id),
                "wait-idempotency-mode",
                AgentWaitMode::All,
            ),
        )
        .unwrap_err();
    assert_eq!(conflict.code(), "communication_idempotency_conflict");
}

#[test]
fn partial_all_can_cancel_or_survive_reopen_until_final_match() {
    let temp = tempfile::tempdir().unwrap();
    let cancel_db = Database::open(&temp.path().join("wait-all-cancel.db")).unwrap();
    let owner = principal('e');
    let watcher = agent(&cancel_db, &owner, "wait-all-cancel-watcher");
    let worker = agent(&cancel_db, &owner, "wait-all-cancel-worker");
    let cancel_endpoint = endpoint(&cancel_db, &owner, &watcher, "wait-all-cancel-view");
    let task_a = task(&cancel_db, &owner, &worker, "wait-all-cancel-a");
    let task_b = task(&cancel_db, &owner, &worker, "wait-all-cancel-b");
    let a = start(&cancel_db, &owner, &task_a, &worker, "wait-all-cancel-a");
    let _b = start(&cancel_db, &owner, &task_b, &worker, "wait-all-cancel-b");
    let wait = cancel_db
        .create_agent_wait(
            &owner,
            wait_input_mode(
                &watcher,
                &cancel_endpoint,
                &[task_a.clone(), task_b],
                "wait-all-cancel",
                AgentWaitMode::All,
            ),
        )
        .unwrap()
        .agent_wait;
    complete(
        &cancel_db,
        &owner,
        &task_a,
        &worker,
        &a,
        "wait-all-cancel-a",
    );
    let cancelled = cancel_db
        .cancel_agent_wait(&owner, &wait.wait_id, "wait-all-cancel-op")
        .unwrap();
    assert_eq!(cancelled.agent_wait.state, AgentWaitState::Cancelled);
    assert_eq!(cancelled.agent_wait.match_count, 1);
    assert_eq!(
        cancel_db
            .conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [wait.wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );

    let path = temp.path().join("wait-all-reopen.db");
    let (owner, watcher, worker, task_b, b, wait_id) = {
        let db = Database::open(&path).unwrap();
        let owner = principal('f');
        let watcher = agent(&db, &owner, "wait-all-reopen-watcher");
        let worker = agent(&db, &owner, "wait-all-reopen-worker");
        let endpoint = endpoint(&db, &owner, &watcher, "wait-all-reopen-view");
        let task_a = task(&db, &owner, &worker, "wait-all-reopen-a");
        let task_b = task(&db, &owner, &worker, "wait-all-reopen-b");
        let a = start(&db, &owner, &task_a, &worker, "wait-all-reopen-a");
        let b = start(&db, &owner, &task_b, &worker, "wait-all-reopen-b");
        let wait = db
            .create_agent_wait(
                &owner,
                wait_input_mode(
                    &watcher,
                    &endpoint,
                    &[task_a.clone(), task_b.clone()],
                    "wait-all-reopen",
                    AgentWaitMode::All,
                ),
            )
            .unwrap()
            .agent_wait;
        complete(&db, &owner, &task_a, &worker, &a, "wait-all-reopen-a");
        (owner, watcher, worker, task_b, b, wait.wait_id)
    };
    let db = Database::open(&path).unwrap();
    let reopened = db.read_agent_wait(&owner, &wait_id).unwrap();
    assert_eq!(reopened.mode, AgentWaitMode::All);
    assert_eq!(reopened.state, AgentWaitState::Waiting);
    assert_eq!(reopened.match_count, 1);
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    complete(&db, &owner, &task_b, &worker, &b, "wait-all-reopen-b");
    let triggered = db.read_agent_wait(&owner, &wait_id).unwrap();
    assert_eq!(triggered.state, AgentWaitState::Triggered);
    assert_eq!(triggered.match_count, 2);
    assert_eq!(
        db.conn_for_tests()
            .query_row(
                "SELECT COUNT(*) FROM wc_agent_wakes WHERE source_wait_id = ?1",
                [wait_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert!(!watcher.is_empty());
}

#[test]
fn old_wait_schema_migrates_mode_to_any_and_malformed_all_fails_closed() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "
        CREATE TABLE wc_agent_waits (
            wait_id TEXT PRIMARY KEY,
            owner_principal_kind TEXT NOT NULL,
            owner_principal_digest TEXT NOT NULL,
            target_agent_id TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('waiting', 'triggered', 'resumed', 'cancelled')),
            revision INTEGER NOT NULL CHECK(revision >= 1),
            created_at_unix_ms INTEGER NOT NULL,
            updated_at_unix_ms INTEGER NOT NULL,
            triggered_at_unix_ms INTEGER,
            resumed_at_unix_ms INTEGER,
            cancelled_at_unix_ms INTEGER
        );
        INSERT INTO wc_agent_waits (
            wait_id, owner_principal_kind, owner_principal_digest, target_agent_id,
            state, revision, created_at_unix_ms, updated_at_unix_ms,
            triggered_at_unix_ms, resumed_at_unix_ms, cancelled_at_unix_ms
        ) VALUES (
            'wc_agent_wait_aaaaaaaaaaaaaaaa', 'user',
            'wc_principal_sha256_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
            'wc_dagent_aaaaaaaaaaaaaaaa', 'waiting', 1, 1, 1, NULL, NULL, NULL
        );
        ",
    )
    .unwrap();
    Database::ensure_agent_wait_schema(&mut conn).unwrap();
    let migrated_mode: String = conn
        .query_row(
            "SELECT mode FROM wc_agent_waits WHERE wait_id = 'wc_agent_wait_aaaaaaaaaaaaaaaa'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(migrated_mode, "any");

    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-all-malformed.db")).unwrap();
    let owner = principal('1');
    let watcher = agent(&db, &owner, "wait-all-malformed-watcher");
    let worker = agent(&db, &owner, "wait-all-malformed-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-all-malformed-view");
    let task_a = task(&db, &owner, &worker, "wait-all-malformed-a");
    let task_b = task(&db, &owner, &worker, "wait-all-malformed-b");
    let a = start(&db, &owner, &task_a, &worker, "wait-all-malformed-a");
    let _b = start(&db, &owner, &task_b, &worker, "wait-all-malformed-b");
    let wait = db
        .create_agent_wait(
            &owner,
            wait_input_mode(
                &watcher,
                &endpoint,
                &[task_a.clone(), task_b],
                "wait-all-malformed",
                AgentWaitMode::All,
            ),
        )
        .unwrap()
        .agent_wait;
    complete(&db, &owner, &task_a, &worker, &a, "wait-all-malformed-a");
    db.conn_for_tests()
        .execute(
            "UPDATE wc_agent_waits
             SET state = 'triggered', triggered_at_unix_ms = updated_at_unix_ms
             WHERE wait_id = ?1",
            [wait.wait_id.as_str()],
        )
        .unwrap();
    let malformed = db.read_agent_wait(&owner, &wait.wait_id).unwrap_err();
    assert_eq!(malformed.code(), "agent_wait_join_invariant");
}

#[test]
fn malformed_complete_all_wait_cannot_be_cancelled_into_a_valid_terminal_state() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-all-malformed-cancel.db")).unwrap();
    let owner = principal('2');
    let watcher = agent(&db, &owner, "wait-all-malformed-cancel-watcher");
    let worker = agent(&db, &owner, "wait-all-malformed-cancel-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-all-malformed-cancel-view");
    let task_a = task(&db, &owner, &worker, "wait-all-malformed-cancel-a");
    let task_b = task(&db, &owner, &worker, "wait-all-malformed-cancel-b");
    let a = start(&db, &owner, &task_a, &worker, "wait-all-malformed-cancel-a");
    let b = start(&db, &owner, &task_b, &worker, "wait-all-malformed-cancel-b");
    complete(
        &db,
        &owner,
        &task_a,
        &worker,
        &a,
        "wait-all-malformed-cancel-a",
    );
    complete(
        &db,
        &owner,
        &task_b,
        &worker,
        &b,
        "wait-all-malformed-cancel-b",
    );
    let wait = db
        .create_agent_wait(
            &owner,
            wait_input_mode(
                &watcher,
                &endpoint,
                &[task_a, task_b],
                "wait-all-malformed-cancel",
                AgentWaitMode::All,
            ),
        )
        .unwrap()
        .agent_wait;
    let wake_id = wait_wake_id(&db, &wait.wait_id);
    db.conn_for_tests()
        .execute(
            "UPDATE wc_agent_waits
             SET state = 'waiting', triggered_at_unix_ms = NULL
             WHERE wait_id = ?1",
            [wait.wait_id.as_str()],
        )
        .unwrap();

    let error = db
        .cancel_agent_wait(&owner, &wait.wait_id, "wait-all-malformed-cancel-op")
        .unwrap_err();
    assert_eq!(error.code(), "agent_wait_join_invariant");
    let state: String = db
        .conn_for_tests()
        .query_row(
            "SELECT state FROM wc_agent_waits WHERE wait_id = ?1",
            [wait.wait_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "waiting");
    assert_eq!(
        db.agent_wake(&wake_id).unwrap().unwrap().state,
        AgentWakeState::Pending
    );
}

#[test]
fn all_wait_prepare_fails_closed_on_incomplete_wake_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-all-malformed-wake.db")).unwrap();
    let owner = principal('3');
    let watcher = agent(&db, &owner, "wait-all-malformed-wake-watcher");
    let worker = agent(&db, &owner, "wait-all-malformed-wake-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-all-malformed-wake-view");
    let task_a = task(&db, &owner, &worker, "wait-all-malformed-wake-a");
    let task_b = task(&db, &owner, &worker, "wait-all-malformed-wake-b");
    let a = start(&db, &owner, &task_a, &worker, "wait-all-malformed-wake-a");
    let b = start(&db, &owner, &task_b, &worker, "wait-all-malformed-wake-b");
    complete(
        &db,
        &owner,
        &task_a,
        &worker,
        &a,
        "wait-all-malformed-wake-a",
    );
    complete(
        &db,
        &owner,
        &task_b,
        &worker,
        &b,
        "wait-all-malformed-wake-b",
    );
    let wait = db
        .create_agent_wait(
            &owner,
            wait_input_mode(
                &watcher,
                &endpoint,
                &[task_a, task_b],
                "wait-all-malformed-wake",
                AgentWaitMode::All,
            ),
        )
        .unwrap()
        .agent_wait;
    let wake_id = wait_wake_id(&db, &wait.wait_id);
    let claim = db
        .claim_next_agent_wake(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            "mcp_app",
        )
        .unwrap()
        .unwrap();
    db.conn_for_tests()
        .execute(
            "UPDATE wc_agent_wakes
             SET wait_match_count_snapshot = 1, wait_match_sequence_snapshot = 1
             WHERE wake_id = ?1",
            [wake_id.as_str()],
        )
        .unwrap();

    let error = db
        .prepare_agent_wake_dispatch(
            &owner,
            &watcher,
            &endpoint.endpoint_id,
            endpoint.controller_generation,
            &wake_id,
            &claim.attempt.attempt_id,
            &claim.claim_fence,
            &claim.consume_token,
        )
        .unwrap_err();
    assert_eq!(error.code(), "agent_wait_wake_invariant");
    assert_eq!(
        db.agent_wake(&wake_id).unwrap().unwrap().state,
        AgentWakeState::Claimed
    );
}

#[test]
fn malformed_wait_match_sequence_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-all-malformed-sequence.db")).unwrap();
    let owner = principal('4');
    let watcher = agent(&db, &owner, "wait-all-malformed-sequence-watcher");
    let worker = agent(&db, &owner, "wait-all-malformed-sequence-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-all-malformed-sequence-view");
    let task_a = task(&db, &owner, &worker, "wait-all-malformed-sequence-a");
    let task_b = task(&db, &owner, &worker, "wait-all-malformed-sequence-b");
    let a = start(
        &db,
        &owner,
        &task_a,
        &worker,
        "wait-all-malformed-sequence-a",
    );
    let _b = start(
        &db,
        &owner,
        &task_b,
        &worker,
        "wait-all-malformed-sequence-b",
    );
    let wait = db
        .create_agent_wait(
            &owner,
            wait_input_mode(
                &watcher,
                &endpoint,
                &[task_a.clone(), task_b],
                "wait-all-malformed-sequence",
                AgentWaitMode::All,
            ),
        )
        .unwrap()
        .agent_wait;
    complete(
        &db,
        &owner,
        &task_a,
        &worker,
        &a,
        "wait-all-malformed-sequence-a",
    );
    db.conn_for_tests()
        .execute(
            "UPDATE wc_agent_wait_matches SET sequence = 2 WHERE wait_id = ?1",
            [wait.wait_id.as_str()],
        )
        .unwrap();

    let error = db.read_agent_wait(&owner, &wait.wait_id).unwrap_err();
    assert_eq!(error.code(), "agent_wait_match_sequence_invariant");
}

#[test]
fn malformed_all_wait_match_must_belong_to_registered_source() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-all-malformed-source.db")).unwrap();
    let owner = principal('5');
    let watcher = agent(&db, &owner, "wait-all-malformed-source-watcher");
    let worker = agent(&db, &owner, "wait-all-malformed-source-worker");
    let endpoint = endpoint(&db, &owner, &watcher, "wait-all-malformed-source-view");
    let task_a = task(&db, &owner, &worker, "wait-all-malformed-source-a");
    let task_b = task(&db, &owner, &worker, "wait-all-malformed-source-b");
    let task_c = task(&db, &owner, &worker, "wait-all-malformed-source-c");
    let a = start(&db, &owner, &task_a, &worker, "wait-all-malformed-source-a");
    let b = start(&db, &owner, &task_b, &worker, "wait-all-malformed-source-b");
    let c = start(&db, &owner, &task_c, &worker, "wait-all-malformed-source-c");
    let wait = db
        .create_agent_wait(
            &owner,
            wait_input_mode(
                &watcher,
                &endpoint,
                &[task_a.clone(), task_b.clone()],
                "wait-all-malformed-source",
                AgentWaitMode::All,
            ),
        )
        .unwrap()
        .agent_wait;
    complete(
        &db,
        &owner,
        &task_a,
        &worker,
        &a,
        "wait-all-malformed-source-a",
    );
    let partial = db.read_agent_wait(&owner, &wait.wait_id).unwrap();
    assert_eq!(partial.state, AgentWaitState::Waiting);
    assert_eq!(partial.match_count, 1);

    {
        let conn = db.conn_for_tests();
        conn.execute(
            "INSERT INTO wc_agent_wait_matches (
                 wait_id, sequence, kind, task_id, task_attempt_id, terminal_task_state, occurred_at_unix_ms
             ) VALUES (?1, 2, 'agent_task_terminal', ?2, ?3, 'succeeded', ?4)",
            rusqlite::params![
                wait.wait_id,
                task_c,
                c.attempt.attempt_id,
                partial.updated_at_unix_ms + 1
            ],
        )
        .unwrap();
        conn.execute(
            "UPDATE wc_agent_waits
             SET state = 'triggered', triggered_at_unix_ms = updated_at_unix_ms
             WHERE wait_id = ?1",
            [wait.wait_id.as_str()],
        )
        .unwrap();
    }

    let read_error = db.read_agent_wait(&owner, &wait.wait_id).unwrap_err();
    assert_eq!(read_error.code(), "agent_wait_match_source_invariant");
    let cancel_error = db
        .cancel_agent_wait(&owner, &wait.wait_id, "wait-all-malformed-source-cancel")
        .unwrap_err();
    assert_eq!(cancel_error.code(), "agent_wait_match_source_invariant");
    let wake_error =
        require_agent_wait_for_wake(&db.conn_for_tests(), &owner, Some(&wait.wait_id), &watcher)
            .unwrap_err();
    assert_eq!(wake_error.code(), "agent_wait_match_source_invariant");

    let mut conn = db.conn_for_tests();
    let transaction = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let terminal_error = record_agent_task_terminal_wait_matches_in_transaction(
        &transaction,
        &owner,
        &task_b,
        &b.attempt.attempt_id,
        super::agent_task::AgentTaskState::Succeeded,
        partial.updated_at_unix_ms + 2,
    )
    .unwrap_err();
    assert_eq!(terminal_error.code(), "agent_wait_match_source_invariant");
    transaction.rollback().unwrap();
}

#[test]
fn source_fanout_is_bounded_at_wait_admission() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(&temp.path().join("wait-fanout.db")).unwrap();
    let owner = principal('8');
    let worker = agent(&db, &owner, "wait-fanout-worker");
    let task_id = task(&db, &owner, &worker, "wait-fanout-task");
    let watchers = [
        agent(&db, &owner, "wait-fanout-a"),
        agent(&db, &owner, "wait-fanout-b"),
    ];
    let endpoints = [
        endpoint(&db, &owner, &watchers[0], "wait-fanout-a"),
        endpoint(&db, &owner, &watchers[1], "wait-fanout-b"),
    ];
    for index in 0..MAX_AGENT_WAITS_PER_SOURCE {
        let slot = (index as usize) % watchers.len();
        db.create_agent_wait(
            &owner,
            wait_input(
                &watchers[slot],
                &endpoints[slot],
                std::slice::from_ref(&task_id),
                &format!("wait-fanout-{index}"),
            ),
        )
        .unwrap();
    }
    let overflow = db
        .create_agent_wait(
            &owner,
            wait_input(
                &watchers[0],
                &endpoints[0],
                std::slice::from_ref(&task_id),
                "wait-fanout-overflow",
            ),
        )
        .unwrap_err();
    assert_eq!(overflow.code(), "agent_wait_source_capacity_reached");
}
