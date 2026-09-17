use crate::auth::AuthContext;
use crate::Database;
use sha2::{Digest, Sha256};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, RwLock};
use webcodex_runner_registry::{
    JobTerminalEvent, JobTerminalEventSink, JobTerminalRegistrationSnapshot, RunnerAccessGroup,
};
use webcodex_store::{
    JobTerminalDeliveryState, JobTerminalFact, JobTerminalSourceIdentity, JobTerminalWaitPrincipal,
    JobTerminalWaitRecord, JobTerminalWaitStoreError,
};

pub(crate) const JOB_TERMINAL_ATTENTION_METRIC: &str = "job_terminal_attention_total";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JobTerminalDeliveryEnvelope {
    pub wait_id: String,
    pub job_id: String,
    pub status: String,
    pub outcome: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobTerminalDispatchOutcome {
    Delivered,
    OutcomeUnknown,
}

/// Process-local presentation seam for one already-durable Job terminal fact.
/// It grants no Job visibility or execution authority and owns no retry policy.
pub(crate) trait JobTerminalContinuationAdapter: std::fmt::Debug + Send + Sync {
    fn adapter_kind(&self) -> &'static str;
    fn production_auto_resume_available(&self) -> bool;
    fn preflight(&self, envelope: &JobTerminalDeliveryEnvelope) -> Result<(), ()>;
    fn dispatch(&self, envelope: JobTerminalDeliveryEnvelope) -> JobTerminalDispatchOutcome;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobTerminalDeliveryAttempt {
    NoCarrier,
    PreflightFailed,
    Deduplicated,
    Delivered,
    DeliveryUnknown,
}

#[derive(Clone)]
pub(crate) struct JobTerminalContinuationController {
    db: Arc<Database>,
    adapter: Arc<RwLock<Option<Arc<dyn JobTerminalContinuationAdapter>>>>,
}

impl std::fmt::Debug for JobTerminalContinuationController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobTerminalContinuationController")
            .field(
                "automatic_resume_available",
                &self.automatic_resume_available(),
            )
            .finish_non_exhaustive()
    }
}

impl JobTerminalContinuationController {
    pub(crate) fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            adapter: Arc::new(RwLock::new(None)),
        }
    }

    pub(crate) fn automatic_resume_available(&self) -> bool {
        self.adapter
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|adapter| adapter.production_auto_resume_available())
    }

    #[cfg(test)]
    pub(crate) fn install_adapter_for_tests(
        &self,
        adapter: Arc<dyn JobTerminalContinuationAdapter>,
    ) {
        *self
            .adapter
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(adapter);
    }

    pub(crate) fn attempt_delivery(
        &self,
        principal: &JobTerminalWaitPrincipal,
        wait_id: &str,
        now: i64,
    ) -> Result<JobTerminalDeliveryAttempt, JobTerminalWaitStoreError> {
        let wait = self.db.read_job_terminal_wait(principal, wait_id, now)?;
        if wait.delivery_state != JobTerminalDeliveryState::Pending {
            metric("deduplicated");
            return Ok(JobTerminalDeliveryAttempt::Deduplicated);
        }
        let adapter = self
            .adapter
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(adapter) = adapter else {
            metric("no_carrier");
            return Ok(JobTerminalDeliveryAttempt::NoCarrier);
        };
        tracing::debug!(
            adapter_kind = adapter.adapter_kind(),
            "Job terminal Host carrier preflight"
        );
        let envelope = envelope(&wait)?;
        if adapter.preflight(&envelope).is_err() {
            metric("preflight_failed");
            return Ok(JobTerminalDeliveryAttempt::PreflightFailed);
        }
        let Some(prepared) = self
            .db
            .prepare_job_terminal_delivery(principal, wait_id, now)?
        else {
            metric("deduplicated");
            return Ok(JobTerminalDeliveryAttempt::Deduplicated);
        };
        metric("delivery_attempted");
        let dispatch = catch_unwind(AssertUnwindSafe(|| adapter.dispatch(envelope)))
            .unwrap_or(JobTerminalDispatchOutcome::OutcomeUnknown);
        let delivered = dispatch == JobTerminalDispatchOutcome::Delivered;
        self.db.finish_job_terminal_delivery(
            principal,
            wait_id,
            &prepared.attempt_id,
            delivered,
            now,
        )?;
        if delivered {
            metric("delivery_accepted");
            Ok(JobTerminalDeliveryAttempt::Delivered)
        } else {
            metric("delivery_unknown");
            Ok(JobTerminalDeliveryAttempt::DeliveryUnknown)
        }
    }
}

fn envelope(
    wait: &JobTerminalWaitRecord,
) -> Result<JobTerminalDeliveryEnvelope, JobTerminalWaitStoreError> {
    let Some(status) = wait.terminal_status.clone() else {
        return Err(JobTerminalWaitStoreError {
            code: "job_terminal_wait_storage_invariant",
            message: "triggered Job terminal wait is missing terminal status".to_string(),
        });
    };
    let Some(outcome) = wait.terminal_outcome.clone() else {
        return Err(JobTerminalWaitStoreError {
            code: "job_terminal_wait_storage_invariant",
            message: "triggered Job terminal wait is missing terminal outcome".to_string(),
        });
    };
    Ok(JobTerminalDeliveryEnvelope {
        wait_id: wait.wait_id.clone(),
        job_id: wait.source.job_id.clone(),
        status,
        outcome,
    })
}

pub(crate) struct SqliteJobTerminalEventSink {
    db: Arc<Database>,
    controller: JobTerminalContinuationController,
}

impl std::fmt::Debug for SqliteJobTerminalEventSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SqliteJobTerminalEventSink")
    }
}

impl SqliteJobTerminalEventSink {
    pub(crate) fn new(db: Arc<Database>, controller: JobTerminalContinuationController) -> Self {
        Self { db, controller }
    }
}

impl JobTerminalEventSink for SqliteJobTerminalEventSink {
    fn record_terminal_event(&self, event: &JobTerminalEvent) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        let fact = fact_from_event(event);
        let matched = self
            .db
            .match_job_terminal_fact(&fact, now)
            .map_err(|error| error.code.to_string())?;
        if matched.matched_count > 0 {
            metric("async_match");
        }
        let mut first_error = None;
        for (principal, wait_id) in matched.delivery_candidates {
            if let Err(error) = self.controller.attempt_delivery(&principal, &wait_id, now) {
                first_error.get_or_insert_with(|| error.code.to_string());
            }
        }
        if let Some(error) = first_error {
            Err(error)
        } else {
            Ok(())
        }
    }
}

pub(crate) fn principal_for_auth(auth: Option<&AuthContext>) -> JobTerminalWaitPrincipal {
    let access = crate::runner_http::runner_access_from_auth(auth);
    let (kind, subject) = match access.as_ref() {
        None => ("internal", "internal".to_string()),
        Some(access) if access.owner_bypass => ("bootstrap", "bootstrap".to_string()),
        Some(access) if access.global_visibility => {
            let subject = auth
                .and_then(|auth| auth.user_id.as_deref())
                .or_else(|| auth.and_then(|auth| auth.username.as_deref()))
                .or_else(|| auth.and_then(|auth| auth.api_key_id.as_deref()))
                .unwrap_or("global");
            ("global_user", subject.to_string())
        }
        Some(access) => match access.group.as_ref() {
            Some(RunnerAccessGroup::SharedKey(value)) => ("shared_key", value.clone()),
            Some(RunnerAccessGroup::ProjectGrant(value)) => ("project_grant", value.clone()),
            Some(RunnerAccessGroup::OpenAnonymous) => {
                ("open_anonymous", "open_anonymous".to_string())
            }
            None => (
                "managed_owner",
                access
                    .username
                    .clone()
                    .unwrap_or_else(|| "managed_unowned".to_string()),
            ),
        },
    };
    let mut hasher = Sha256::new();
    hasher.update(b"webcodex.job-terminal-wait.principal.v1\0");
    hash_field(&mut hasher, kind);
    hash_field(&mut hasher, &subject);
    JobTerminalWaitPrincipal {
        kind: kind.to_string(),
        digest: format!("{:x}", hasher.finalize()),
    }
}

fn hash_field(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
}

pub(crate) fn source_from_snapshot(
    snapshot: &JobTerminalRegistrationSnapshot,
) -> JobTerminalSourceIdentity {
    source(
        &snapshot.job_id,
        &snapshot.client_id,
        snapshot.auth_group.as_ref(),
        snapshot.owner_at_admission.as_deref(),
    )
}

pub(crate) fn fact_from_event(event: &JobTerminalEvent) -> JobTerminalFact {
    JobTerminalFact {
        source: source(
            &event.job_id,
            &event.client_id,
            event.auth_group.as_ref(),
            event.owner_at_admission.as_deref(),
        ),
        status: event.status.clone(),
        outcome: event.outcome.clone(),
        terminal_observed_at: event.terminal_observed_at,
        expires_at: event.expires_at,
    }
}

fn source(
    job_id: &str,
    client_id: &str,
    auth_group: Option<&RunnerAccessGroup>,
    owner_at_admission: Option<&str>,
) -> JobTerminalSourceIdentity {
    let (auth_kind, auth_value) = match auth_group {
        Some(RunnerAccessGroup::SharedKey(value)) => ("shared_key", Some(value.clone())),
        Some(RunnerAccessGroup::ProjectGrant(value)) => ("project_grant", Some(value.clone())),
        Some(RunnerAccessGroup::OpenAnonymous) => ("open_anonymous", None),
        None => match owner_at_admission {
            Some(owner) => ("managed_owner", Some(owner.to_string())),
            None => ("managed_unowned", None),
        },
    };
    JobTerminalSourceIdentity {
        job_id: job_id.to_string(),
        client_id: client_id.to_string(),
        auth_kind: auth_kind.to_string(),
        auth_value,
    }
}

pub(crate) fn metric(outcome: &'static str) {
    tracing::info!(
        target: "webcodex::job_terminal_attention",
        metric = JOB_TERMINAL_ATTENTION_METRIC,
        outcome,
        value = 1_u64,
        "Job terminal attention observation"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use webcodex_store::{JobTerminalWaitState, NewJobTerminalWait};

    #[derive(Debug)]
    struct TestAdapter {
        preflight_ok: bool,
        outcome: JobTerminalDispatchOutcome,
        preflights: AtomicUsize,
        dispatches: AtomicUsize,
    }

    impl TestAdapter {
        fn new(preflight_ok: bool, outcome: JobTerminalDispatchOutcome) -> Self {
            Self {
                preflight_ok,
                outcome,
                preflights: AtomicUsize::new(0),
                dispatches: AtomicUsize::new(0),
            }
        }
    }

    impl JobTerminalContinuationAdapter for TestAdapter {
        fn adapter_kind(&self) -> &'static str {
            "test"
        }
        fn production_auto_resume_available(&self) -> bool {
            true
        }
        fn preflight(&self, _envelope: &JobTerminalDeliveryEnvelope) -> Result<(), ()> {
            self.preflights.fetch_add(1, Ordering::SeqCst);
            if self.preflight_ok {
                Ok(())
            } else {
                Err(())
            }
        }
        fn dispatch(&self, _envelope: JobTerminalDeliveryEnvelope) -> JobTerminalDispatchOutcome {
            self.dispatches.fetch_add(1, Ordering::SeqCst);
            self.outcome
        }
    }

    fn principal() -> JobTerminalWaitPrincipal {
        JobTerminalWaitPrincipal {
            kind: "test".to_string(),
            digest: "a".repeat(64),
        }
    }

    fn source(job_id: &str) -> JobTerminalSourceIdentity {
        JobTerminalSourceIdentity {
            job_id: job_id.to_string(),
            client_id: "runner".to_string(),
            auth_kind: "managed_owner".to_string(),
            auth_value: Some("alice".to_string()),
        }
    }

    #[test]
    fn terminal_wait_principal_tracks_job_visibility_not_rotating_credentials() {
        let mut first = AuthContext::new(crate::auth::AuthKind::ApiToken);
        first.user_id = Some("user-alice".to_string());
        first.username = Some("alice".to_string());
        first.api_key_id = Some("key-a".to_string());
        let mut rotated = first.clone();
        rotated.api_key_id = Some("key-b".to_string());
        rotated.allowed_client_id = Some("unrelated-transport-hint".to_string());
        assert_eq!(
            principal_for_auth(Some(&first)),
            principal_for_auth(Some(&rotated))
        );

        let mut bob = rotated;
        bob.user_id = Some("user-bob".to_string());
        bob.username = Some("bob".to_string());
        assert_ne!(
            principal_for_auth(Some(&first)),
            principal_for_auth(Some(&bob))
        );
    }

    #[test]
    fn logical_job_source_survives_runner_instance_transfer() {
        let snapshot = JobTerminalRegistrationSnapshot {
            job_id: "job-detached".to_string(),
            client_id: "runner".to_string(),
            runner_instance_id: "instance-old".to_string(),
            auth_group: None,
            owner_at_admission: Some("alice".to_string()),
            terminal_event: None,
            wait_expires_at: 1_900,
        };
        let event = JobTerminalEvent {
            job_id: "job-detached".to_string(),
            client_id: "runner".to_string(),
            runner_instance_id: "instance-new".to_string(),
            auth_group: None,
            owner_at_admission: Some("alice".to_string()),
            status: "completed".to_string(),
            outcome: "succeeded".to_string(),
            terminal_observed_at: 1_000,
            expires_at: 1_900,
        };
        assert_eq!(
            source_from_snapshot(&snapshot),
            fact_from_event(&event).source
        );
    }

    fn create_triggered(db: &Database, job_id: &str, key: &str, now: i64) -> String {
        let terminal = JobTerminalFact {
            source: source(job_id),
            status: "completed".to_string(),
            outcome: "succeeded".to_string(),
            terminal_observed_at: now,
            expires_at: now + 900,
        };
        db.create_job_terminal_wait(
            &principal(),
            NewJobTerminalWait {
                source: source(job_id),
                idempotency_key: key.to_string(),
                expires_at: now + 900,
                already_terminal: Some(terminal),
            },
            now,
        )
        .unwrap()
        .wait
        .wait_id
    }

    #[test]
    fn no_carrier_and_preflight_failure_leave_triggered_event_pending() {
        let temp = tempdir().unwrap();
        let db = Arc::new(Database::open(&temp.path().join("host-pending.db")).unwrap());
        let controller = JobTerminalContinuationController::new(db.clone());
        let wait_id = create_triggered(&db, "job-no-carrier", "key-no-carrier", 1_000);
        assert_eq!(
            controller
                .attempt_delivery(&principal(), &wait_id, 1_001)
                .unwrap(),
            JobTerminalDeliveryAttempt::NoCarrier
        );
        assert_eq!(
            db.read_job_terminal_wait(&principal(), &wait_id, 1_001)
                .unwrap()
                .delivery_state,
            JobTerminalDeliveryState::Pending
        );

        let adapter = Arc::new(TestAdapter::new(
            false,
            JobTerminalDispatchOutcome::Delivered,
        ));
        controller.install_adapter_for_tests(adapter.clone());
        assert!(controller.automatic_resume_available());
        assert_eq!(
            controller
                .attempt_delivery(&principal(), &wait_id, 1_002)
                .unwrap(),
            JobTerminalDeliveryAttempt::PreflightFailed
        );
        assert_eq!(adapter.preflights.load(Ordering::SeqCst), 1);
        assert_eq!(adapter.dispatches.load(Ordering::SeqCst), 0);
        let wait = db
            .read_job_terminal_wait(&principal(), &wait_id, 1_002)
            .unwrap();
        assert_eq!(wait.state, JobTerminalWaitState::Triggered);
        assert_eq!(wait.delivery_state, JobTerminalDeliveryState::Pending);
    }

    #[test]
    fn accepted_dispatch_delivers_once_and_duplicate_scheduling_is_deduplicated() {
        let temp = tempdir().unwrap();
        let db = Arc::new(Database::open(&temp.path().join("host-delivered.db")).unwrap());
        let controller = JobTerminalContinuationController::new(db.clone());
        let adapter = Arc::new(TestAdapter::new(
            true,
            JobTerminalDispatchOutcome::Delivered,
        ));
        controller.install_adapter_for_tests(adapter.clone());
        let wait_id = create_triggered(&db, "job-delivered", "key-delivered", 2_000);
        assert_eq!(
            controller
                .attempt_delivery(&principal(), &wait_id, 2_001)
                .unwrap(),
            JobTerminalDeliveryAttempt::Delivered
        );
        assert_eq!(
            controller
                .attempt_delivery(&principal(), &wait_id, 2_002)
                .unwrap(),
            JobTerminalDeliveryAttempt::Deduplicated
        );
        assert_eq!(adapter.dispatches.load(Ordering::SeqCst), 1);
        assert_eq!(
            db.read_job_terminal_wait(&principal(), &wait_id, 2_002)
                .unwrap()
                .delivery_state,
            JobTerminalDeliveryState::Delivered
        );
    }

    #[test]
    fn unknown_dispatch_outcome_is_fenced_and_never_silently_redispatched() {
        let temp = tempdir().unwrap();
        let db = Arc::new(Database::open(&temp.path().join("host-unknown.db")).unwrap());
        let controller = JobTerminalContinuationController::new(db.clone());
        let adapter = Arc::new(TestAdapter::new(
            true,
            JobTerminalDispatchOutcome::OutcomeUnknown,
        ));
        controller.install_adapter_for_tests(adapter.clone());
        let wait_id = create_triggered(&db, "job-unknown", "key-unknown", 3_000);
        assert_eq!(
            controller
                .attempt_delivery(&principal(), &wait_id, 3_001)
                .unwrap(),
            JobTerminalDeliveryAttempt::DeliveryUnknown
        );
        assert_eq!(
            controller
                .attempt_delivery(&principal(), &wait_id, 3_002)
                .unwrap(),
            JobTerminalDeliveryAttempt::Deduplicated
        );
        assert_eq!(adapter.dispatches.load(Ordering::SeqCst), 1);
        assert_eq!(
            db.read_job_terminal_wait(&principal(), &wait_id, 3_002)
                .unwrap()
                .delivery_state,
            JobTerminalDeliveryState::DeliveryUnknown
        );
    }
}
