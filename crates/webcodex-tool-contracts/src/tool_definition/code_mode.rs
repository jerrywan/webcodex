use super::ToolVisibility::ModelVisible;
use super::{
    adaptive_runtime_direct, context_reobservable, def, model_spec,
    requires_explicit_business_session, ToolDefinition, TOOL_CATEGORY_RUNTIME,
};
use crate::metadata::{
    ToolPathHint::None as NoPath, ToolRisk::Read, PROJECT_READ, TOOL_PROVIDER_CONTROL,
};
use crate::registry::input_schemas::code_mode_exec_input_schema;

const RESULT_AUDIT_FIELDS: &[super::ToolAuditResultField] = &[
    super::ToolAuditResultField::pointer("tool_calls", "/stats/tool_calls"),
    super::ToolAuditResultField::pointer("max_in_flight", "/stats/max_in_flight"),
    super::ToolAuditResultField::pointer("duration_ms", "/stats/duration_ms"),
    super::ToolAuditResultField::pointer("returned_bytes", "/stats/returned_bytes"),
    super::ToolAuditResultField::value("failure_kind"),
];

pub(super) const DEFINITIONS: &[ToolDefinition] = &[adaptive_runtime_direct(
    context_reobservable(requires_explicit_business_session(model_spec(
        def(
            "code_mode_exec",
            super::ToolAuditPolicy::typed_fields(RESULT_AUDIT_FIELDS),
            ModelVisible,
            TOOL_CATEGORY_RUNTIME,
            None,
            TOOL_PROVIDER_CONTROL,
            super::ToolSemanticContract {
                effect: super::ToolEffect::Observe,
                risk: Read,
                approval: super::ToolApprovalPolicy::None,
                idempotency: super::ToolIdempotency::PureRead,
            },
            Some(PROJECT_READ),
            true,
            NoPath,
            false,
            false,
            super::ToolSessionEvidencePolicy::NONE
                .review(super::ToolReviewEvidence::ReadOnlyInspection),
        ),
        "Experimental read-only JavaScript orchestration for related/adaptive inspections. tools.<name>(args) re-enters canonical ToolRuntime under the outer-bound Project/Session; text(value) emits bounded output. Prefer a direct tool for one simple observation. No shell/fs/network/mutation/Jobs.",
        code_mode_exec_input_schema,
    ))),
    45,
)];
