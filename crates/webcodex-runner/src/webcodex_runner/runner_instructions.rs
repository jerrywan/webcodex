use super::config::InstructionsConfig;
use super::CommandResult;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::time::Instant;
use webcodex_core::project_instructions::{
    InstructionSourceScope, LoadedInstructionCandidate, ProjectInstructionsSnapshot,
};
use webcodex_core::runner_instruction::{
    RunnerInstructionAction, RunnerInstructionRequest, RunnerInstructionSnapshotResponse,
    RUNNER_INSTRUCTION_RESPONSE_FORMAT, RUNNER_INSTRUCTION_RESPONSE_MAX_BYTES,
};

const MAX_CONFIGURED_INSTRUCTION_FILE_BYTES: u64 = 1024 * 1024;

pub(crate) fn handle_runner_instruction_request(
    generation: u64,
    config: &InstructionsConfig,
    request: RunnerInstructionRequest,
) -> CommandResult {
    let started = Instant::now();
    if request.validate().is_err() {
        return error_result(started, "instruction_invalid_request");
    }
    match request.action {
        RunnerInstructionAction::Snapshot => snapshot(generation, config, started),
    }
}

fn snapshot(generation: u64, config: &InstructionsConfig, started: Instant) -> CommandResult {
    let mut candidates = Vec::with_capacity(config.files.len());
    let mut identities = HashSet::with_capacity(config.files.len());
    let mut scan_complete = true;

    for (index, configured) in config.files.iter().enumerate() {
        let canonical = match std::fs::canonicalize(configured) {
            Ok(path) if path.is_file() => path,
            _ => {
                scan_complete = false;
                continue;
            }
        };
        let identity = crate::runner_config::paths::normalize_path_identity(&canonical);
        if !identities.insert(identity) {
            scan_complete = false;
            continue;
        }
        let metadata = match std::fs::metadata(&canonical) {
            Ok(metadata) if metadata.len() <= MAX_CONFIGURED_INSTRUCTION_FILE_BYTES => metadata,
            _ => {
                scan_complete = false;
                continue;
            }
        };
        let bytes = match std::fs::read(&canonical) {
            Ok(bytes) if bytes.len() as u64 == metadata.len() => bytes,
            _ => {
                scan_complete = false;
                continue;
            }
        };
        let content = match String::from_utf8(bytes) {
            Ok(content) => content,
            Err(_) => {
                scan_complete = false;
                continue;
            }
        };
        if content.is_empty() {
            continue;
        }
        let basename = configured
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("instructions");
        let logical_source = format!("runner/{index}/{basename}");
        let total_lines = line_count(&content);
        let full_sha256 = format!("{:x}", Sha256::digest(content.as_bytes()));
        candidates.push(LoadedInstructionCandidate {
            source_scope: InstructionSourceScope::Runner,
            path: logical_source,
            content,
            total_lines,
            full_sha256: Some(full_sha256),
        });
    }

    let snapshot = ProjectInstructionsSnapshot::from_candidates(candidates, scan_complete);
    let response = RunnerInstructionSnapshotResponse {
        format: RUNNER_INSTRUCTION_RESPONSE_FORMAT.to_string(),
        generation,
        scan_complete: snapshot.scan_complete,
        files: snapshot.files,
    };
    if response.validate().is_err() {
        return error_result(started, "instruction_response_invalid");
    }
    let stdout = match serde_json::to_string(&response) {
        Ok(stdout) if stdout.len() <= RUNNER_INSTRUCTION_RESPONSE_MAX_BYTES => stdout,
        _ => return error_result(started, "instruction_response_too_large"),
    };
    CommandResult {
        exit_code: Some(0),
        stdout: Some(stdout),
        stderr: None,
        duration_ms: Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
        error: None,
    }
}

fn line_count(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.bytes().filter(|byte| *byte == b'\n').count()
            + usize::from(!content.ends_with('\n'))
    }
}

fn error_result(started: Instant, code: &str) -> CommandResult {
    CommandResult {
        exit_code: Some(1),
        stdout: None,
        stderr: None,
        duration_ms: Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)),
        error: Some(code.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn omitted_config_produces_complete_empty_snapshot() {
        let response = handle_runner_instruction_request(
            1,
            &InstructionsConfig::default(),
            RunnerInstructionRequest::snapshot(),
        );
        let parsed: RunnerInstructionSnapshotResponse =
            serde_json::from_str(response.stdout.as_deref().unwrap()).unwrap();
        assert!(parsed.scan_complete);
        assert!(parsed.files.is_empty());
    }

    #[test]
    fn snapshot_uses_logical_sources_and_live_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        std::fs::write(&path, "first\n").unwrap();
        let config = InstructionsConfig {
            files: vec![path.clone()],
        };

        let first =
            handle_runner_instruction_request(7, &config, RunnerInstructionRequest::snapshot());
        let first_stdout = first.stdout.as_deref().unwrap();
        assert!(!first_stdout.contains(path.to_string_lossy().as_ref()));
        let first: RunnerInstructionSnapshotResponse = serde_json::from_str(first_stdout).unwrap();
        assert_eq!(first.files.len(), 1);
        assert_eq!(first.files[0].path, "runner/0/AGENTS.md");
        assert_eq!(first.files[0].content, "first");
        assert!(first.files[0].read_more.is_none());
        let first_fingerprint = first.files[0].fingerprint.clone();

        std::fs::write(&path, "second\n").unwrap();
        let second =
            handle_runner_instruction_request(7, &config, RunnerInstructionRequest::snapshot());
        let second: RunnerInstructionSnapshotResponse =
            serde_json::from_str(second.stdout.as_deref().unwrap()).unwrap();
        assert_eq!(second.generation, 7);
        assert_eq!(second.files[0].content, "second");
        assert_ne!(second.files[0].fingerprint, first_fingerprint);
    }

    #[test]
    fn snapshot_does_not_require_project_allowed_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("global.md");
        std::fs::write(&path, "runner only").unwrap();
        let config = InstructionsConfig {
            files: vec![PathBuf::from(&path)],
        };
        let response =
            handle_runner_instruction_request(1, &config, RunnerInstructionRequest::snapshot());
        assert_eq!(response.exit_code, Some(0));
    }
}
