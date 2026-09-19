//! Compact MCP selection copy, applied only to owned tools/list projections.
//! Exact manifests and canonical ToolSpecs retain the operational contract.

use serde_json::Value;

// MCP discovery targets, independent of GPT Actions' importer limits. Keep
// purpose, the nearest selection boundary, and essential continuation guidance.
pub(super) const TOOL_DESCRIPTION_MAX_CHARS: usize = 420;
pub(super) const INPUT_DESCRIPTION_MAX_CHARS: usize = 180;

pub(super) fn compact_tool(tool: &mut Value) {
    let name = tool["name"].as_str().unwrap_or_default();
    if let Some(description) = tool["description"].as_str() {
        let selection = match name {
            "work_on_project" => "Start ordinary coding/review with project or client_id+path. Omit session_id for a fresh Workflow Session; supply it only for exact resume. Defaults return project instructions, workflow and extension guidance. Use mode=worktree for an isolated Git worktree.",
            "tool_manifest" => "Discover tools by intent/category, or pass tool_name for one exact canonical contract and its direct/gateway route. Use exact lookup when arguments or operational details are not already known.",
            "call_runtime_tool" => "Call one admitted runtime tool with its exact arguments. Use tool_manifest to discover the contract. Prefer an available direct callable; this gateway also supports admitted direct tools when that callable is unavailable. Target validation and authority checks still apply.",
            "run_process" => "Run one native executable with literal argv. Use run_shell for shell grammar or a short related command chain. Long work continues as the same Runner-owned Job through observe_jobs; retain the returned continuation instead of redispatching.",
            "run_shell" => "Run shell grammar or a short related command chain. Use run_process for one native executable with literal argv. Long work continues as the same Runner-owned Job through observe_jobs; retain the returned continuation instead of redispatching.",
            "run_detached_process" => "Start a native child that intentionally survives Runner restart or replacement as a durable Job. Duration alone does not require detachment. Requires an idempotency_key; retain the same Job and use observe_jobs or stop_job after handoff uncertainty.",
            "observe_jobs" => "Continue known Jobs by job_id; do not list first. Pass observation_token unchanged as after_observation_token. Follow the returned continuation for more output; observation never redispatches work. Use wait_for_job_terminal when blocked only on terminal completion.",
            "list_jobs" => "Recover or inventory caller-visible Job identities. When a job_id or continuation is already known, use observe_jobs directly.",
            "wait_for_job_terminal" => "Arm a bounded one-shot terminal wait for one exact existing Job. Reuse the keyed wait and returned continuation; never redispatch the Job. Continue independent work, or follow the offered Host continuation when only terminal completion blocks progress.",
            "stop_job" => "Stop one existing Job by exact job_id with confirm=true. Preserves Project and Session ownership. Use observe_jobs to inspect output or wait_for_job_terminal to wait without stopping.",
            _ => description,
        };
        tool["description"] =
            Value::String(bound_description(selection, TOOL_DESCRIPTION_MAX_CHARS));
    }
    if let Some(schema) = tool.get_mut("inputSchema") {
        compact_input_descriptions(schema);
        // Only these root properties are protocol wrappers. A business
        // session_id keeps its own canonical-derived copy and requiredness.
        for (field, description) in [
            ("recording_session_id", "Optional Workflow Session recorder provenance; never authority or a business Session selector."),
            ("ack_session_message_ids", "IDs of ACK-required Session/Peer messages retained in current model context. Repeat while retained; never resolves messages or grants authority."),
            ("session_message_resolution", "Resolve one already-handled non-todo message in the explicit recording Session; ACK if required. Unrelated to main call success."),
            ("context_request", "Optional context sidecar after the result; never authority. Keys: project.instructions, webcodex.workflow, jobs.attention, skills.catalog, plugins.catalog, memory.bootstrap."),
        ] {
            if let Some(property) = schema.pointer_mut(&format!("/properties/{field}")) {
                if property.get("description").is_some_and(Value::is_string) {
                    property["description"] = Value::String(description.to_string());
                }
            }
        }
    }
}

fn compact_input_descriptions(schema: &mut Value) {
    // Traverse schema positions only: const/default/enum/examples may contain
    // business data named "description" that must never be rewritten.
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    if let Some(Value::String(description)) = object.get_mut("description") {
        *description = bound_description(description, INPUT_DESCRIPTION_MAX_CHARS);
    }
    for keyword in [
        "properties",
        "patternProperties",
        "$defs",
        "definitions",
        "dependentSchemas",
        "dependencies",
    ] {
        if let Some(children) = object.get_mut(keyword).and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                compact_input_descriptions(child);
            }
        }
    }
    for keyword in [
        "items",
        "prefixItems",
        "allOf",
        "anyOf",
        "oneOf",
        "additionalItems",
        "additionalProperties",
        "unevaluatedItems",
        "unevaluatedProperties",
        "propertyNames",
        "contains",
        "not",
        "if",
        "then",
        "else",
    ] {
        if let Some(child) = object.get_mut(keyword) {
            if let Some(children) = child.as_array_mut() {
                for child in children {
                    compact_input_descriptions(child);
                }
            } else {
                compact_input_descriptions(child);
            }
        }
    }
}

pub(super) fn bound_description(description: &str, max_chars: usize) -> String {
    let description = description.trim();
    if description.chars().count() <= max_chars {
        return description.to_string();
    }
    // Prefer complete sentences; periods in foo.rs, v0.4.0, and context keys
    // are not sentence boundaries. Fall back to a Unicode-safe word prefix.
    let prefix: String = description.chars().take(max_chars - 1).collect();
    if let Some(end) = prefix
        .char_indices()
        .filter_map(|(index, ch)| {
            let end = index + ch.len_utf8();
            (matches!(ch, '.' | '!' | '?')
                && description[end..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace))
            .then_some(end)
        })
        .last()
    {
        return prefix[..end].to_string();
    }
    let end = prefix
        .rfind(char::is_whitespace)
        .filter(|index| *index > 0)
        .unwrap_or(prefix.len());
    format!("{}…", prefix[..end].trim_end())
}
