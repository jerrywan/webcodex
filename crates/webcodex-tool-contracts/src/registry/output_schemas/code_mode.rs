use serde_json::{json, Value};

use super::common::{array_schema, schema_type, wrapped_output_schema};

fn stats_schema() -> Value {
    json!({
        "type": "object",
        "description": "Bounded orchestration evidence for this one-shot Code Mode execution.",
        "additionalProperties": false,
        "properties": {
            "tool_calls": {"type": "integer", "minimum": 0, "maximum": 32},
            "max_in_flight": {"type": "integer", "minimum": 0, "maximum": 8},
            "duration_ms": {"type": "integer", "minimum": 0},
            "returned_bytes": {"type": "integer", "minimum": 0, "maximum": 65536}
        },
        "required": ["tool_calls", "max_in_flight", "duration_ms", "returned_bytes"]
    })
}

fn content_schema() -> Value {
    let mut schema = array_schema(
        schema_type("string", "One bounded text(value) emission."),
        "Only text(value) emissions selected by the JavaScript orchestration. Nested raw ToolResults are not copied here automatically.",
    );
    schema["maxItems"] = json!(256);
    schema
}

pub(super) fn output_schema_for_tool(name: &str) -> Option<Value> {
    match name {
        "code_mode_exec" => Some(wrapped_output_schema(vec![
            ("content", content_schema()),
            ("stats", stats_schema()),
            ("message", {
                let mut schema = schema_type(
                        "string",
                        "Model-facing bounded runtime detail for a failed Code Mode execution. Durable Session result audit omits this field.",
                    );
                schema["maxLength"] = json!(16_384);
                schema
            }),
            (
                "failure_kind",
                json!({
                    "type": "string",
                    "enum": [
                        "invalid_request",
                        "runtime_error",
                        "timeout",
                        "tool_call_budget_exceeded",
                        "output_limit_exceeded"
                    ],
                    "description": "Present on a bounded Code Mode runtime/host failure. Ordinary nested ToolResult business failures remain JavaScript values and do not become this field."
                }),
            ),
        ])),
        _ => None,
    }
}
