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
use crate::tool_runtime::{registered_tool_specs, ToolResult, ToolSpec};
use serde_json::{json, Value};
use std::collections::HashSet;

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

/// Presentation route for one canonical SuggestedToolCall target. This is not
/// authority: adapters resolve the route from their already-admitted model
/// surface and the canonical target still runs through ordinary ToolRuntime
/// validation, authorization, permission, capability, and effect checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuggestedToolCallRoute {
    Direct,
    Gateway(&'static str),
    Unavailable,
}

pub(crate) fn suggested_tool_call_route(
    target: &str,
    operator_extension_admitted: bool,
) -> SuggestedToolCallRoute {
    match adaptive_runtime_tool_invocation_route_with_operator_extension(
        target,
        operator_extension_admitted,
    ) {
        (TOOL_SURFACE_AVAILABILITY_DIRECT, None) => SuggestedToolCallRoute::Direct,
        (TOOL_SURFACE_AVAILABILITY_GATEWAY, Some(gateway)) => {
            SuggestedToolCallRoute::Gateway(gateway)
        }
        _ => SuggestedToolCallRoute::Unavailable,
    }
}

/// Project formally declared SuggestedToolCall schemas to the callable shape of
/// one model surface. Domain ToolSpecs remain canonical; adapters apply this to
/// response-schema copies only.
pub(crate) fn project_suggested_tool_call_schema<F>(schema: &mut Value, route_for: &F)
where
    F: Fn(&str) -> SuggestedToolCallRoute,
{
    if project_suggested_tool_call_schema_node(schema, route_for) {
        *schema = json!({"not": {}});
    }
}

fn project_suggested_tool_call_schema_node<F>(schema: &mut Value, route_for: &F) -> bool
where
    F: Fn(&str) -> SuggestedToolCallRoute,
{
    if let Some(target) =
        webcodex_tool_contracts::suggested_tool_call_schema_target(schema).map(str::to_string)
    {
        return match route_for(&target) {
            SuggestedToolCallRoute::Direct => false,
            SuggestedToolCallRoute::Gateway(gateway) => {
                let mut canonical = std::mem::take(schema);
                let description = canonical.get("description").cloned();
                let properties = canonical
                    .get_mut("properties")
                    .and_then(Value::as_object_mut)
                    .expect("recognized SuggestedToolCall schema has properties");
                let canonical_tool = properties
                    .remove("tool")
                    .expect("recognized SuggestedToolCall schema has tool");
                let canonical_arguments = properties
                    .remove("arguments")
                    .expect("recognized SuggestedToolCall schema has arguments");
                *schema = json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "tool": {"type": "string", "const": gateway},
                        "arguments": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "tool": canonical_tool,
                                "arguments": canonical_arguments
                            },
                            "required": ["tool", "arguments"]
                        }
                    },
                    "required": ["tool", "arguments"]
                });
                if let Some(description) = description {
                    schema["description"] = description;
                }
                false
            }
            SuggestedToolCallRoute::Unavailable => true,
        };
    }

    let mut removed_properties = Vec::new();
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        let names = properties.keys().cloned().collect::<Vec<_>>();
        for name in names {
            if properties
                .get_mut(&name)
                .is_some_and(|child| project_suggested_tool_call_schema_node(child, route_for))
            {
                removed_properties.push(name);
            }
        }
        for name in &removed_properties {
            properties.remove(name);
        }
    }
    if !removed_properties.is_empty() {
        if let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
            required.retain(|field| {
                field
                    .as_str()
                    .is_none_or(|field| !removed_properties.iter().any(|name| name == field))
            });
        }
    }

    let item_became_unavailable = schema
        .get_mut("items")
        .is_some_and(|items| project_suggested_tool_call_schema_node(items, route_for));
    if item_became_unavailable {
        if schema.get("minItems").and_then(Value::as_u64).unwrap_or(0) > 0 {
            return true;
        }
        schema["items"] = json!({"not": {}});
    }

    for keyword in ["anyOf", "oneOf"] {
        let Some(branches) = schema.get_mut(keyword).and_then(Value::as_array_mut) else {
            continue;
        };
        let mut index = 0;
        while index < branches.len() {
            if project_suggested_tool_call_schema_node(&mut branches[index], route_for) {
                branches.remove(index);
            } else {
                index += 1;
            }
        }
        if branches.is_empty() {
            return true;
        }
    }
    if let Some(branches) = schema.get_mut("allOf").and_then(Value::as_array_mut) {
        for branch in branches {
            if project_suggested_tool_call_schema_node(branch, route_for) {
                return true;
            }
        }
    }
    for keyword in ["then", "else"] {
        if let Some(branch) = schema.get_mut(keyword) {
            if project_suggested_tool_call_schema_node(branch, route_for) {
                *branch = json!({"not": {}});
            }
        }
    }
    false
}

/// Project only values proven by the canonical output schema to be
/// SuggestedToolCall edges. The visited JSON-pointer set prevents one runtime
/// value from being rewritten twice when `oneOf`/`allOf` branches describe the
/// same output location.
pub(crate) fn project_suggested_tool_calls_in_value<F>(
    value: &mut Value,
    schema: &Value,
    route_for: &F,
) where
    F: Fn(&str) -> SuggestedToolCallRoute,
{
    let mut visited = HashSet::new();
    if project_suggested_tool_calls_in_value_node(value, schema, route_for, "", &mut visited) {
        *value = Value::Null;
    }
}

fn project_suggested_tool_calls_in_value_node<F>(
    value: &mut Value,
    schema: &Value,
    route_for: &F,
    path: &str,
    visited: &mut HashSet<String>,
) -> bool
where
    F: Fn(&str) -> SuggestedToolCallRoute,
{
    if let Some(target) = webcodex_tool_contracts::suggested_tool_call_schema_target(schema) {
        if value.get("tool").and_then(Value::as_str) != Some(target)
            || value.get("arguments").is_none()
        {
            return false;
        }
        if !visited.insert(path.to_string()) {
            return false;
        }
        return match route_for(target) {
            SuggestedToolCallRoute::Direct => false,
            SuggestedToolCallRoute::Gateway(gateway) => {
                let arguments = value
                    .get_mut("arguments")
                    .map(Value::take)
                    .unwrap_or(Value::Null);
                *value = json!({
                    "tool": gateway,
                    "arguments": {
                        "tool": target,
                        "arguments": arguments
                    }
                });
                false
            }
            SuggestedToolCallRoute::Unavailable => true,
        };
    }

    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        value.as_object_mut(),
    ) {
        let names = properties.keys().cloned().collect::<Vec<_>>();
        let mut remove = Vec::new();
        for name in names {
            let Some(child_value) = object.get_mut(&name) else {
                continue;
            };
            let Some(child_schema) = properties.get(&name) else {
                continue;
            };
            let child_path = format!("{}/{}", path, json_pointer_segment(&name));
            if project_suggested_tool_calls_in_value_node(
                child_value,
                child_schema,
                route_for,
                &child_path,
                visited,
            ) {
                remove.push(name);
            }
        }
        for name in remove {
            object.remove(&name);
        }
    }

    if let (Some(item_schema), Some(items)) = (schema.get("items"), value.as_array_mut()) {
        let mut remove = Vec::new();
        for (index, item) in items.iter_mut().enumerate() {
            let child_path = format!("{path}/{index}");
            if project_suggested_tool_calls_in_value_node(
                item,
                item_schema,
                route_for,
                &child_path,
                visited,
            ) {
                remove.push(index);
            }
        }
        for index in remove.into_iter().rev() {
            items.remove(index);
        }
    }

    for keyword in ["anyOf", "oneOf", "allOf"] {
        if let Some(branches) = schema.get(keyword).and_then(Value::as_array) {
            for branch in branches {
                if project_suggested_tool_calls_in_value_node(
                    value, branch, route_for, path, visited,
                ) {
                    return true;
                }
            }
        }
    }
    for keyword in ["then", "else"] {
        if let Some(branch) = schema.get(keyword) {
            if project_suggested_tool_calls_in_value_node(value, branch, route_for, path, visited) {
                return true;
            }
        }
    }
    false
}

fn json_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

pub(crate) fn project_tool_result_suggested_calls<F>(
    tool_name: &str,
    result: &mut ToolResult,
    route_for: &F,
) where
    F: Fn(&str) -> SuggestedToolCallRoute,
{
    let output = result.output.take();
    let mut envelope = json!({"success": result.success, "output": output});
    if let Some(error) = result.error.as_ref() {
        envelope["error"] = Value::String(error.clone());
    }
    let schema = webcodex_tool_contracts::output_schema_for_tool(tool_name);
    project_suggested_tool_calls_in_value(&mut envelope, &schema, route_for);
    result.output = envelope
        .as_object_mut()
        .and_then(|object| object.remove("output"))
        .unwrap_or(Value::Null);
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
