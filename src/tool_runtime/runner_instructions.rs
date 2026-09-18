use super::project_instructions::{ProjectInstructionFile, ProjectInstructionsSnapshot};
use super::project_resolution::ResolvedProject;
use super::ToolRuntime;
use crate::auth::AuthContext;
use crate::runner_http::{requested_by_from_auth, runner_access_from_auth, RunnerFeature};
use std::time::Duration;
use webcodex_core::runner_instruction::{
    RunnerInstructionRequest, RunnerInstructionSnapshotResponse,
    RUNNER_INSTRUCTION_RESPONSE_MAX_BYTES,
};

const RUNNER_INSTRUCTION_WAIT_SECS: u64 = 20;

impl ToolRuntime {
    pub(crate) async fn load_effective_coding_instructions(
        &self,
        project: &ResolvedProject,
        auth: Option<&AuthContext>,
    ) -> ProjectInstructionsSnapshot {
        let local = self.load_coding_project_instructions(&project.config);
        if !project.resolved_id.starts_with("agent:") {
            return ProjectInstructionsSnapshot::with_runner_files(Vec::new(), local.await, true);
        }
        let runner = self.load_runner_instruction_files(&project.config.client_id, auth);
        let ((runner_files, runner_complete), local) =
            futures_util::future::join(runner, local).await;
        ProjectInstructionsSnapshot::with_runner_files(runner_files, local, runner_complete)
    }

    pub(crate) async fn load_effective_session_instructions(
        &self,
        project: &ResolvedProject,
        auth: Option<&AuthContext>,
    ) -> ProjectInstructionsSnapshot {
        let local = self.load_project_instructions(&project.config);
        if !project.resolved_id.starts_with("agent:") {
            return ProjectInstructionsSnapshot::with_runner_files(Vec::new(), local.await, true);
        }
        let runner = self.load_runner_instruction_files(&project.config.client_id, auth);
        let ((runner_files, runner_complete), local) =
            futures_util::future::join(runner, local).await;
        ProjectInstructionsSnapshot::with_runner_files(runner_files, local, runner_complete)
    }

    async fn load_runner_instruction_files(
        &self,
        client_id: &str,
        auth: Option<&AuthContext>,
    ) -> (Vec<ProjectInstructionFile>, bool) {
        let access = runner_access_from_auth(auth);
        let semantic = match self
            .runner_registry
            .get_runner_semantic_view_checked_for_auth(client_id, access.as_ref())
            .await
        {
            Ok(semantic) => semantic,
            Err(_) => return (Vec::new(), false),
        };
        if !semantic.supports(RunnerFeature::InstructionRuntime) {
            // Older Runner binaries cannot have `[instructions].files` semantics at all,
            // so absence of the additive capability is a complete empty global snapshot.
            // Do not degrade existing project-local instruction observation to unavailable.
            return (Vec::new(), true);
        }
        let runner_instance_id = semantic.view.runner_instance_id.clone();
        if runner_instance_id.is_empty() {
            return (Vec::new(), false);
        }

        let (request_id, receiver) = match self
            .runner_registry
            .enqueue_runner_instruction(
                client_id,
                &runner_instance_id,
                RunnerInstructionRequest::snapshot(),
                access.as_ref(),
                requested_by_from_auth(auth),
            )
            .await
        {
            Ok(request) => request,
            Err(_) => return (Vec::new(), false),
        };
        let response =
            match tokio::time::timeout(Duration::from_secs(RUNNER_INSTRUCTION_WAIT_SECS), receiver)
                .await
            {
                Ok(Ok(response)) => response,
                Ok(Err(_)) | Err(_) => {
                    self.runner_registry
                        .cancel_request_dispatch_state(&request_id)
                        .await;
                    return (Vec::new(), false);
                }
            };
        if response.error.is_some() || response.exit_code != Some(0) {
            return (Vec::new(), false);
        }
        let Some(stdout) = response.stdout.as_deref() else {
            return (Vec::new(), false);
        };
        if stdout.len() > RUNNER_INSTRUCTION_RESPONSE_MAX_BYTES {
            return (Vec::new(), false);
        }
        let mut parsed = match serde_json::from_str::<RunnerInstructionSnapshotResponse>(stdout) {
            Ok(parsed) => parsed,
            Err(_) => return (Vec::new(), false),
        };
        if parsed.bind_visible_fingerprints().is_err() {
            return (Vec::new(), false);
        }
        (parsed.files, parsed.scan_complete)
    }
}
