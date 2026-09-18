use super::ToolVisibility::ModelVisible;
use super::{
    def, model_spec, permission_risk, require_any_scopes, ToolDefinition,
    PERMISSION_RISK_BROWSER_CONTROL, TOOL_CATEGORY_BROWSER,
};
use crate::metadata::{
    ToolPathHint::None as NoPath,
    ToolRisk::{BrowserControl as BrowserControlRisk, Read},
    BROWSER_CONTROL, BROWSER_LAUNCH, BROWSER_READ, TOOL_PROVIDER_CONTROL,
};

const BROWSER_ACT_GATEWAY_SCOPES: &[&str] = &[BROWSER_CONTROL, BROWSER_LAUNCH];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_model_surface_metrics_are_bounded() {
        assert_eq!(DEFINITIONS.len(), 2);
        let observe = DEFINITIONS[0]
            .model_spec
            .expect("browser_observe model spec");
        let act = DEFINITIONS[1].model_spec.expect("browser_act model spec");
        let observe_schema_bytes =
            serde_json::to_vec(&crate::input_schema_for_tool("browser_observe"))
                .unwrap()
                .len();
        let act_schema_bytes = serde_json::to_vec(&crate::input_schema_for_tool("browser_act"))
            .unwrap()
            .len();
        let combined_schema_bytes = observe_schema_bytes + act_schema_bytes;
        let combined_description_bytes = observe.description.len() + act.description.len();
        println!(
            "browser_surface_metrics observe_schema_bytes={observe_schema_bytes} act_schema_bytes={act_schema_bytes} combined_schema_bytes={combined_schema_bytes} combined_description_bytes={combined_description_bytes}"
        );
        // Correctness-bearing branch schemas stay explicit; common semantics belong in
        // canonical descriptions rather than duplicated prose in each oneOf branch.
        assert!(observe_schema_bytes <= 8 * 1024);
        assert!(act_schema_bytes <= 16 * 1024);
        assert!(combined_description_bytes <= 2 * 1024);
    }
}

pub(super) const DEFINITIONS: &[ToolDefinition] = &[
    model_spec(
        def(
            "browser_observe",
            super::ToolAuditPolicy::typed_semantic(
                super::ToolAuditSemanticResultPolicy::BrowserObservation,
            ),
            ModelVisible,
            TOOL_CATEGORY_BROWSER,
            None,
            TOOL_PROVIDER_CONTROL,
            super::ToolSemanticContract {
                effect: super::ToolEffect::Observe,
                risk: Read,
                approval: super::ToolApprovalPolicy::None,
                idempotency: super::ToolIdempotency::PureRead,
            },
            Some(BROWSER_READ),
            false,
            NoPath,
            false,
            false,
            super::ToolSessionEvidencePolicy::NONE,
        ),
        "Guaranteed read-only Browser observation gateway with a closed action vocabulary: targets, browsers, pages, semantic snapshot, and screenshot. Browser/Page/Element identities are opaque and process-local; semantic snapshots are bounded, expose typed form-control state and snapshot-local group context when Chromium provides it, and navigation makes older element identities stale. Screenshots use the shared native-image delivery contract at the MCP boundary. Exact Runner capability and browser:read authority are checked before dispatch. No effect, process launch, arbitrary protocol input, script execution, profile attachment, or shell fallback is available here.",
    ),
    require_any_scopes(
        permission_risk(
            model_spec(
                def(
                    "browser_act",
                    super::ToolAuditPolicy::typed_semantic(
                        super::ToolAuditSemanticResultPolicy::BrowserControl,
                    ),
                    ModelVisible,
                    TOOL_CATEGORY_BROWSER,
                    None,
                    TOOL_PROVIDER_CONTROL,
                    super::ToolSemanticContract {
                        effect: super::ToolEffect::Execute,
                        risk: BrowserControlRisk,
                        approval: super::ToolApprovalPolicy::Standard,
                        idempotency: super::ToolIdempotency::NonIdempotent,
                    },
                    None,
                    false,
                    NoPath,
                    true,
                    false,
                    super::ToolSessionEvidencePolicy::NONE,
                ),
                "Effectful Browser action gateway with the closed actions launch, new_page, navigate, click, input_text, select_option, set_value, upload_file, key, close_page, and close_browser. Exact action authority is resolved by canonical specialized governance before dispatch: launch requires browser:launch, upload_file requires browser:control plus project:read, and the remaining effects require browser:control. Uploads accept only one project-relative regular file from the same Runner and are Runner-policy bounded. Effects preserve not_started/completed/outcome_unknown certainty and never blindly retry an uncertain action. Element effects revalidate Browser, Page, document, and snapshot identity before dispatch. No arbitrary protocol, executable, profile, remote endpoint, or script input is accepted.",
            ),
            PERMISSION_RISK_BROWSER_CONTROL,
        ),
        BROWSER_ACT_GATEWAY_SCOPES,
    ),
];
