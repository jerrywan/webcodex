//! Canonical model-facing routing for Adaptive Runtime.
//!
//! WebCodex has one model-facing runtime contract. Canonical ToolDefinitions
//! decide which model-visible tools are directly exposed; every other
//! model-visible runtime tool is reached through `call_runtime_tool`. Hidden
//! protocol extensions are reachable only after the adapter independently
//! admits their protocol capability.

use crate::tool_runtime::tool_definition::{
    adaptive_runtime_direct_tool_definitions, is_adaptive_runtime_direct_tool,
    is_model_visible_tool_name,
};
use crate::tool_runtime::{registered_tool_specs, ToolSpec};

pub(crate) const ADAPTIVE_RUNTIME_GATEWAY_TOOL_NAME: &str = "call_runtime_tool";
pub(crate) const TOOL_SURFACE_AVAILABILITY_DIRECT: &str = "direct";
pub(crate) const TOOL_SURFACE_AVAILABILITY_GATEWAY: &str = "gateway";
pub(crate) const TOOL_SURFACE_AVAILABILITY_UNAVAILABLE: &str = "unavailable";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdaptiveRuntimeGatewayTargetRoute {
    Gateway,
    Direct,
    Recursive,
    Unknown,
}

/// Canonical Adaptive Runtime routing for one ordinary registered tool.
/// Routing changes model exposure only; canonical authority checks are unchanged.
pub(crate) fn adaptive_runtime_tool_invocation_route(
    tool_name: &str,
) -> (&'static str, Option<&'static str>) {
    adaptive_runtime_tool_invocation_route_with_operator_extension(tool_name, false)
}

/// Route a tool after a protocol adapter has independently admitted a hidden
/// extension. This flag is server-owned request context, not caller authority.
pub(crate) fn adaptive_runtime_tool_invocation_route_with_operator_extension(
    tool_name: &str,
    operator_extension_admitted: bool,
) -> (&'static str, Option<&'static str>) {
    if operator_extension_admitted {
        return (
            TOOL_SURFACE_AVAILABILITY_GATEWAY,
            Some(ADAPTIVE_RUNTIME_GATEWAY_TOOL_NAME),
        );
    }
    if !is_model_visible_tool_name(tool_name) {
        return (TOOL_SURFACE_AVAILABILITY_UNAVAILABLE, None);
    }
    if is_adaptive_runtime_direct_tool(tool_name) {
        (TOOL_SURFACE_AVAILABILITY_DIRECT, None)
    } else {
        (
            TOOL_SURFACE_AVAILABILITY_GATEWAY,
            Some(ADAPTIVE_RUNTIME_GATEWAY_TOOL_NAME),
        )
    }
}

pub(crate) fn adaptive_runtime_gateway_target_route(
    target: &str,
) -> AdaptiveRuntimeGatewayTargetRoute {
    if target == ADAPTIVE_RUNTIME_GATEWAY_TOOL_NAME {
        return AdaptiveRuntimeGatewayTargetRoute::Recursive;
    }
    match adaptive_runtime_tool_invocation_route(target) {
        (TOOL_SURFACE_AVAILABILITY_DIRECT, None) => AdaptiveRuntimeGatewayTargetRoute::Direct,
        (TOOL_SURFACE_AVAILABILITY_GATEWAY, Some(ADAPTIVE_RUNTIME_GATEWAY_TOOL_NAME)) => {
            AdaptiveRuntimeGatewayTargetRoute::Gateway
        }
        _ => AdaptiveRuntimeGatewayTargetRoute::Unknown,
    }
}

/// Compact MCP discovery is the Adaptive Runtime default. The explicit
/// operator override changes schema projection only, never tool behavior.
pub(crate) fn effective_mcp_compact_schemas(configured_override: Option<bool>) -> bool {
    configured_override.unwrap_or(true)
}

/// Direct ToolSpecs ordered by rank declared on canonical ToolDefinitions.
pub(crate) fn adaptive_runtime_direct_tool_specs() -> Vec<ToolSpec> {
    let mut by_name: std::collections::HashMap<String, ToolSpec> = registered_tool_specs()
        .into_iter()
        .map(|spec| (spec.name.clone(), spec))
        .collect();
    adaptive_runtime_direct_tool_definitions()
        .into_iter()
        .map(|definition| {
            by_name.remove(definition.name).unwrap_or_else(|| {
                panic!(
                    "{} adaptive_runtime direct tool is missing a registered ToolSpec",
                    definition.name
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_specs_are_definition_derived_and_model_visible() {
        let specs = adaptive_runtime_direct_tool_specs();
        let expected = adaptive_runtime_direct_tool_definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect::<Vec<_>>();
        let actual = specs
            .iter()
            .map(|spec| spec.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
        for spec in specs {
            assert!(is_model_visible_tool_name(&spec.name), "{}", spec.name);
            assert!(is_adaptive_runtime_direct_tool(&spec.name), "{}", spec.name);
        }
    }

    #[test]
    fn coding_intent_tools_are_reachable() {
        for tool_name in crate::tool_runtime::tool_definition::CODING_INTENT_TOOL_NAMES {
            let (availability, gateway) = adaptive_runtime_tool_invocation_route(tool_name);
            assert_ne!(
                availability, TOOL_SURFACE_AVAILABILITY_UNAVAILABLE,
                "{tool_name}"
            );
            if availability == TOOL_SURFACE_AVAILABILITY_DIRECT {
                assert_eq!(gateway, None, "{tool_name}");
            } else {
                assert_eq!(
                    gateway,
                    Some(ADAPTIVE_RUNTIME_GATEWAY_TOOL_NAME),
                    "{tool_name}"
                );
            }
        }
    }

    #[test]
    fn long_tail_and_direct_fallback_routes_are_canonical() {
        for tool_name in ["run_script", "apply_patch"] {
            assert_eq!(
                adaptive_runtime_gateway_target_route(tool_name),
                AdaptiveRuntimeGatewayTargetRoute::Gateway
            );
        }
        assert_eq!(
            adaptive_runtime_gateway_target_route("read_files"),
            AdaptiveRuntimeGatewayTargetRoute::Direct
        );
    }

    #[test]
    fn hidden_tools_fail_closed_without_protocol_admission() {
        assert!(!is_model_visible_tool_name("skill_list"));
        assert_eq!(
            adaptive_runtime_gateway_target_route("skill_list"),
            AdaptiveRuntimeGatewayTargetRoute::Unknown
        );
        assert_eq!(
            adaptive_runtime_tool_invocation_route_with_operator_extension("skill_list", true),
            (
                TOOL_SURFACE_AVAILABILITY_GATEWAY,
                Some(ADAPTIVE_RUNTIME_GATEWAY_TOOL_NAME)
            )
        );
    }

    #[test]
    fn gateway_is_not_recursive() {
        assert_eq!(
            adaptive_runtime_gateway_target_route(ADAPTIVE_RUNTIME_GATEWAY_TOOL_NAME),
            AdaptiveRuntimeGatewayTargetRoute::Recursive
        );
    }

    #[test]
    fn compact_schema_policy_defaults_true_and_respects_override() {
        assert!(effective_mcp_compact_schemas(None));
        assert!(effective_mcp_compact_schemas(Some(true)));
        assert!(!effective_mcp_compact_schemas(Some(false)));
    }
}
