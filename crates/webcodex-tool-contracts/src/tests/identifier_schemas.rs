use super::*;

#[test]
fn durable_identifier_schemas_accept_compact_and_reject_retired_hex() {
    let prefixes = [
        "wc_dagent_",
        "wc_endpoint_",
        "wc_agent_task_",
        "wc_agent_task_attempt_",
        "wc_wake_",
        "wc_wake_attempt_",
        "wc_agent_wait_",
        "wc_goal_",
        "wc_conv_",
        "wc_cmsg_",
        "wc_delivery_",
        "wc_attention_event_",
        "wc_agent_task_fence_",
        "wc_wake_consume_",
        "wc_host_binding_",
    ];
    fn visit(
        value: &serde_json::Value,
        prefixes: &[&str],
        seen: &mut std::collections::HashSet<String>,
    ) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(pattern) = object.get("pattern").and_then(|v| v.as_str()) {
                    for &prefix in prefixes {
                        // Match the domain's exact literal prefix, not a prefix
                        // of another domain (task vs task_attempt, for example).
                        if !pattern.contains(&format!("{prefix}[")) {
                            continue;
                        }
                        let regex = regex::Regex::new(pattern).unwrap();
                        let proof = prefix.ends_with("fence_")
                            || prefix.ends_with("consume_")
                            || prefix.ends_with("binding_");
                        let suffix = if proof {
                            webcodex_core::compact::encode([0xfb; 16])
                        } else {
                            webcodex_core::compact::encode([0xfb; 12])
                        };
                        assert!(regex.is_match(&format!("{prefix}{suffix}")), "{pattern}");
                        assert!(
                            !regex.is_match(&format!("{prefix}{}", "a".repeat(32))),
                            "{pattern}"
                        );
                        seen.insert(prefix.to_string());
                    }
                }
                for child in object.values() {
                    visit(child, prefixes, seen);
                }
            }
            serde_json::Value::Array(values) => {
                for child in values {
                    visit(child, prefixes, seen);
                }
            }
            _ => {}
        }
    }
    let mut seen = std::collections::HashSet::new();
    for spec in registered_tool_specs()
        .into_iter()
        .chain(crate::registry::agent_continuation_app_tool_specs())
    {
        visit(&spec.input_schema, &prefixes, &mut seen);
        visit(&spec.output_schema, &prefixes, &mut seen);
    }
    for prefix in prefixes {
        // Attention events are projected only through their enclosing records.
        if prefix != "wc_attention_event_" {
            assert!(seen.contains(prefix), "missing schema coverage: {prefix}");
        }
    }
}
