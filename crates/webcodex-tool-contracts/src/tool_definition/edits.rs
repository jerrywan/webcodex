use super::RunnerCapabilityRequirement::FileWrite;
use super::ToolVisibility::ModelVisible;
use super::{
    adaptive_runtime_direct, def, model_spec, permission_risk, ToolDefinition,
    PERMISSION_RISK_WRITE, TOOL_CATEGORY_EDIT,
};
use crate::metadata::{
    ToolPathHint::{PathList, SinglePath},
    ToolRisk::ProjectWrite,
    PROJECT_WRITE, TOOL_PROVIDER_RUNNER,
};

pub(super) const DEFINITIONS: &[ToolDefinition] = &[
    permission_risk(
        model_spec(
            def(
            "write_project_file",
            super::ToolAuditPolicy::TYPED_CANONICAL,
            ModelVisible,
            TOOL_CATEGORY_EDIT,
            Some(FileWrite),
            TOOL_PROVIDER_RUNNER,
            super::ToolSemanticContract {
                effect: super::ToolEffect::Mutate,
                risk: ProjectWrite,
                approval: super::ToolApprovalPolicy::Standard,
                idempotency: super::ToolIdempotency::NonIdempotent,
            },
            Some(PROJECT_WRITE),
            true,
            SinglePath,
            true,
            false,
            super::ToolSessionEvidencePolicy::NONE,
            ),
            "Create a new file or perform an intentional whole-file replacement. Existing-file replacement requires expected_read_revision from read_files; ToolRuntime resolves that model-facing snapshot handle to the Runner guard, so the model does not copy a digest. Failures expose minimal error facts and at most one parser-ready read_files recovery call when a fresh read is required. Choose this path when whole-file replacement is genuinely the clearest reliable mutation, then inspect the resulting diff and validate the final source.",
        ),
        PERMISSION_RISK_WRITE,
    ),
    adaptive_runtime_direct(
        permission_risk(
            model_spec(
                def(
                "apply_text_edits",
                super::ToolAuditPolicy::TYPED_CANONICAL,
                ModelVisible,
                TOOL_CATEGORY_EDIT,
                Some(FileWrite),
                TOOL_PROVIDER_RUNNER,
                super::ToolSemanticContract {
                    effect: super::ToolEffect::Mutate,
                    risk: ProjectWrite,
                    approval: super::ToolApprovalPolicy::Standard,
                    idempotency: super::ToolIdempotency::NonIdempotent,
                },
                Some(PROJECT_WRITE),
                true,
                PathList,
                true,
                false,
                super::ToolSessionEvidencePolicy::NONE,
                )
                .with_composition_policy(super::ToolCompositionPolicy::Sequential),
                "Transactional structured option for small/local exact edits. For multiple independent edits to the same file, use ONE change with multiple entries in edits. Never repeat a source or destination path in changes. All edits in one file change resolve against the same original source snapshot; sequential changes cannot be combined automatically. Shorthand path + old_text + new_text is only for one simple exact replacement. For multiple edits or occurrence/line_scope use canonical kind=edit with edits[]; put occurrence/line_scope inside each edit, not on the change. Globally unique edits may omit expected_read_revision; occurrence or line_scope requires expected_read_revision. Occurrence stays in global source order. Revisions fence whole-file snapshots; model input never needs a digest. Batches are preflighted transactionally; conflicts fail closed; Runner rechecks source before mutation. Failures may include one parser-ready read_files recovery call. Inspect the resulting diff and validate the final source.",
            ).with_gpt_action_description("Apply 1..16 transactional exact file changes. Globally unique local edits may omit expected_read_revision; delete/rename and positional occurrence/line_scope require it. ToolRuntime resolves revision guards internally and the whole batch preflights before mutation."),
            PERMISSION_RISK_WRITE,
        ),
        60,
    ),
];
