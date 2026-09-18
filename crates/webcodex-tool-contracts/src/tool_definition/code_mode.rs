use super::ToolVisibility::ModelVisible;
use super::{
    adaptive_runtime_direct, def, model_spec, permission_risk, requires_explicit_business_session,
    ToolDefinition, PERMISSION_RISK_WRITE, TOOL_CATEGORY_RUNTIME,
};
use crate::metadata::{
    ToolPathHint::None as NoPath,
    ToolRisk::{JobRun, ProjectWrite, Read},
    JOB_RUN, PROJECT_READ, PROJECT_WRITE, TOOL_PROVIDER_CONTROL,
};

const RESULT_AUDIT_FIELDS: &[super::ToolAuditResultField] = &[
    super::ToolAuditResultField::pointer("tool_calls", "/stats/tool_calls"),
    super::ToolAuditResultField::pointer("max_in_flight", "/stats/max_in_flight"),
    super::ToolAuditResultField::pointer("duration_ms", "/stats/duration_ms"),
    super::ToolAuditResultField::pointer("returned_bytes", "/stats/returned_bytes"),
    super::ToolAuditResultField::value("failure_kind"),
];

const EFFECTFUL_RESULT_AUDIT_FIELDS: &[super::ToolAuditResultField] = &[
    super::ToolAuditResultField::pointer(
        "consequential_calls",
        "/effect_receipt/consequential_calls",
    ),
    super::ToolAuditResultField::pointer("known_results", "/effect_receipt/known_results"),
    super::ToolAuditResultField::pointer("job_handoffs", "/effect_receipt/job_handoffs"),
    super::ToolAuditResultField::pointer("outcome_unknown", "/effect_receipt/outcome_unknown"),
    super::ToolAuditResultField::value("failure_kind"),
];

pub(super) const DEFINITIONS: &[ToolDefinition] = &[
    adaptive_runtime_direct(
        requires_explicit_business_session(model_spec(
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
            "Read-only Code Mode for related inspections. Prefer a direct tool for one simple observation. Use Promise.all only for independent calls; keep dependent follow-ups sequential inside one cell. Filter child results before text(value); never a raw-result dump. Project before the outer-output limit. Children keep canonical authority; no shell/fs/network/mutation/validation/Jobs.",
        ).with_gpt_action_description("Read-only orchestration for related inspections. Use direct tools for simple observations; parallelize only independent calls, keep adaptive follow-ups inside the cell. Distill evidence before text(value); avoid raw-result dumps. Canonical Project/Session checks remain.")),
        45,
    ),
    adaptive_runtime_direct(
        requires_explicit_business_session(model_spec(
            def(
                "code_mode_exec_effectful",
                super::ToolAuditPolicy::typed_fields(EFFECTFUL_RESULT_AUDIT_FIELDS),
                ModelVisible,
                TOOL_CATEGORY_RUNTIME,
                None,
                TOOL_PROVIDER_CONTROL,
                super::ToolSemanticContract {
                    effect: super::ToolEffect::Execute,
                    risk: JobRun,
                    approval: super::ToolApprovalPolicy::Standard,
                    idempotency: super::ToolIdempotency::NonIdempotent,
                },
                Some(JOB_RUN),
                true,
                NoPath,
                false,
                false,
                super::ToolSessionEvidencePolicy::NONE,
            ),
            "Validation Code Mode for E1 reads plus cargo_check/cargo_test. Default to direct validators; use only when related validations save model turns. Distill results before text(value). Children retain canonical Project/Session, permission, validation and Job semantics; no mutation, shell/process, nested Job observation, gateways or recursion.",
        ).with_gpt_action_description("Validation orchestration for E1 reads plus cargo_check/cargo_test when multiple related validations save model turns. Default to direct validators. Canonical authority/evidence/Jobs remain; no mutation, shell/process, nested Job observation or recursion.")),
        105,
    ),
    adaptive_runtime_direct(
        permission_risk(
            requires_explicit_business_session(model_spec(
                def(
                    "code_mode_exec_mutating",
                    super::ToolAuditPolicy::typed_fields(EFFECTFUL_RESULT_AUDIT_FIELDS),
                    ModelVisible,
                    TOOL_CATEGORY_RUNTIME,
                    None,
                    TOOL_PROVIDER_CONTROL,
                    super::ToolSemanticContract {
                        effect: super::ToolEffect::Mutate,
                        risk: ProjectWrite,
                        approval: super::ToolApprovalPolicy::Standard,
                        idempotency: super::ToolIdempotency::NonIdempotent,
                    },
                    Some(PROJECT_WRITE),
                    true,
                    NoPath,
                    true,
                    false,
                    super::ToolSessionEvidencePolicy::NONE,
                ),
                "Guarded edit Code Mode for adaptive read -> one canonical apply_text_edits attempt. Default to direct mutation; use only when it saves model turns. Distill results before text(value). Nested canonical write authority and Edit evidence remain authoritative; no validation, shell/process, Jobs, other writes, gateways or recursion.",
            ).with_gpt_action_description("Guarded edit orchestration for adaptive read -> one canonical apply_text_edits attempt when it saves model turns. Default to direct edits. Canonical authority/effects remain; no nested validation, shell/process, Jobs, other writes or recursion.")),
            PERMISSION_RISK_WRITE,
        ),
        65,
    ),
];
