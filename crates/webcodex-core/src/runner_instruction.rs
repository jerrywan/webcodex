use crate::project_instructions::{
    InstructionSourceScope, ProjectInstructionFile, MAX_LINES_PER_FILE, MAX_TOTAL_CHARS,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const RUNNER_INSTRUCTION_REQUEST_KIND: &str = "runner_instruction";
pub const RUNNER_INSTRUCTION_REQUEST_MAX_BYTES: usize = 128;
pub const RUNNER_INSTRUCTION_RESPONSE_MAX_BYTES: usize = 192 * 1024;
pub const RUNNER_INSTRUCTION_RESPONSE_MAX_FILES: usize = 16;
pub const RUNNER_INSTRUCTION_RESPONSE_FORMAT: &str = "webcodex.runner_instruction_snapshot.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerInstructionAction {
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerInstructionRequest {
    pub action: RunnerInstructionAction,
}

impl RunnerInstructionRequest {
    pub fn snapshot() -> Self {
        Self {
            action: RunnerInstructionAction::Snapshot,
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        match self.action {
            RunnerInstructionAction::Snapshot => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerInstructionSnapshotResponse {
    pub format: String,
    pub generation: u64,
    pub scan_complete: bool,
    pub files: Vec<ProjectInstructionFile>,
}

impl RunnerInstructionSnapshotResponse {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.format != RUNNER_INSTRUCTION_RESPONSE_FORMAT {
            return Err("unexpected instruction response format");
        }
        if self.generation == 0 {
            return Err("instruction config generation must be positive");
        }
        if self.files.len() > RUNNER_INSTRUCTION_RESPONSE_MAX_FILES {
            return Err("Runner instruction response contains too many sources");
        }
        let mut sources = HashSet::with_capacity(self.files.len());
        for file in &self.files {
            if file.source_scope != InstructionSourceScope::Runner {
                return Err("Runner instruction response contains non-Runner source");
            }
            if !valid_runner_logical_source(&file.path) || !sources.insert(file.path.as_str()) {
                return Err("Runner instruction response contains invalid logical source");
            }
            if file.read_more.is_some() {
                return Err("Runner instruction response must not expose read_more");
            }
            if file.start_line != 1
                || file.limit != MAX_LINES_PER_FILE
                || file.chars != file.content.chars().count()
                || file.chars > MAX_TOTAL_CHARS
                || file.total_lines < file.content.lines().count()
                || !is_lower_hex_sha256(&file.fingerprint)
            {
                return Err("Runner instruction response contains invalid bounded source metadata");
            }
        }
        Ok(())
    }
}

fn valid_runner_logical_source(path: &str) -> bool {
    if path.contains('\\') || path.contains('\0') {
        return false;
    }
    let mut parts = path.split('/');
    let scope = parts.next();
    let index = parts.next();
    let basename = parts.next();
    scope == Some("runner")
        && index.is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()))
        && basename.is_some_and(|name| {
            !name.is_empty() && name.len() <= 255 && name != "." && name != ".."
        })
        && parts.next().is_none()
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_runner_sources_cannot_encode_native_or_traversal_paths() {
        assert!(valid_runner_logical_source("runner/0/AGENTS.md"));
        assert!(valid_runner_logical_source("runner/15/company-guidance.md"));
        assert!(!valid_runner_logical_source(
            "/Users/alice/.codex/AGENTS.md"
        ));
        assert!(!valid_runner_logical_source(
            "runner/0//Users/alice/AGENTS.md"
        ));
        assert!(!valid_runner_logical_source("runner/0/../AGENTS.md"));
        assert!(!valid_runner_logical_source(
            "runner/0/C:\\Users\\alice\\AGENTS.md"
        ));
        assert!(!valid_runner_logical_source(
            "runner/not-an-index/AGENTS.md"
        ));
    }
}
