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
use crate::registry::input_schemas::{
    apply_text_edits_input_schema, write_project_file_input_schema,
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
            "Create a new file or perform an intentional whole-file replacement. Existing-file replacement requires expected_read_revision from read_files; ToolRuntime resolves that model-facing snapshot handle to the Runner's existing SHA guard, so the model does not copy a digest. Choose this path when whole-file replacement is genuinely the clearest reliable mutation, then inspect the resulting diff and validate the final source.",
            write_project_file_input_schema,
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
                ),
                "Transactional structured option for small/local exact edits where old_text or anchor identity is natural and likely to succeed. Globally unique replace_exact/delete_exact/insert_before/insert_after edits may omit expected_read_revision and are re-proved against current Runner content; occurrence stays global source order, and occurrence or line_scope is positional and requires expected_read_revision. Callers may also supply expected_read_revision for a stronger whole-file stale-context fence. ToolRuntime resolves read revisions to the existing Runner SHA wire guard, so model input never needs a digest. The whole batch is preflighted transactionally, conflicts fail closed, and the Runner still rechecks planned source content before mutation. Choose mutation methods by expected correctness and reliability, not tool usage; inspect the resulting diff and validate the final source.",
                apply_text_edits_input_schema,
            ).with_gpt_action_description("Apply 1..16 transactional exact file changes. Globally unique local edits may omit expected_read_revision; delete/rename and positional occurrence/line_scope require it. ToolRuntime resolves revision guards internally and the whole batch preflights before mutation."),
            PERMISSION_RISK_WRITE,
        ),
        60,
    ),
];
