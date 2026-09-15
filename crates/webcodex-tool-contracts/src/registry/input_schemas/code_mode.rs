use serde_json::{json, Value};

use super::common::object_schema;

pub fn code_mode_exec_input_schema() -> Value {
    let mut schema = object_schema(vec![
        (
            "project",
            "string",
            "Required Project target. Nested JavaScript tool calls cannot select or override Project authority.",
            true,
        ),
        (
            "session_id",
            "string",
            "Required exact Workflow Session. Nested JavaScript tool calls remain bound to this Session and record canonical evidence there.",
            true,
        ),
        (
            "source",
            "string",
            "Bounded JavaScript orchestration source. tools.<name>(args) returns a Promise for admitted read-only tools; use Promise.all only for independent observations, keep result-dependent/adaptive calls sequential, and call text(value) for final bounded output. Project/Session are outer-bound. No shell, filesystem, network, Node, Deno, WebAssembly, mutation, validation, Jobs, plugins, or MCP are exposed.",
            true,
        ),
        (
            "timeout_ms",
            "integer",
            "Optional wall-clock budget in milliseconds. Defaults to 5000 and is server-clamped to 1..30000.",
            false,
        ),
    ]);
    schema["properties"]["source"]["maxLength"] = Value::from(65_536);
    schema["properties"]["timeout_ms"]["minimum"] = Value::from(0);
    schema["properties"]["session_id"]["pattern"] =
        json!("^wc_sess_([A-Za-z0-9_-]{16}|[0-9a-f]{32})$");
    schema
}
