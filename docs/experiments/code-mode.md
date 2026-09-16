# Experimental Code Mode E1 — Read-Only Nested Tool Orchestration

> This is an experiment, not a stable compatibility surface.

## Purpose

E1 tests one hypothesis: WebCodex can move bounded read-only orchestration below the model round-trip boundary while keeping every real Project operation inside the existing canonical `ToolRuntime`.

One model-visible `code_mode_exec` call evaluates a bounded JavaScript program. The program may invoke several explicitly admitted read-only WebCodex tools, use ordinary JavaScript control flow, overlap independent observations with `Promise.all`, make later calls conditional on earlier results, and emit only the useful aggregate with `text(value)`.

E1 does **not** add a second filesystem, shell, permission system, Project resolver, Session recorder, Runner protocol, or workflow engine.

## Feature flag

The experiment is disabled by default.

```bash
cargo check -p webcodex-code-mode
cargo test -p webcodex-code-mode --features v8-runtime
cargo check --features experimental-code-mode --all-targets
```

The root `experimental-code-mode` feature enables:

- the optional `webcodex-code-mode` dependency;
- `webcodex-code-mode/v8-runtime`;
- `webcodex-tool-contracts/experimental-code-mode`;
- `webcodex-tool-runtime-contracts/experimental-code-mode`.

Without that feature, `code_mode_exec` is absent from the canonical `ToolDefinition`, `ToolSpec`, `ToolCall`, discovery, Adaptive Runtime, OpenAPI, and MCP surfaces. The default `webcodex-code-mode` crate contains only lightweight transport-neutral contracts and does not compile or link V8.

## Architecture

Dependency direction is intentionally narrow:

```text
model
  |
  v
code_mode_exec
  |
  v
root ToolRuntime resolves/authorizes exact Project + Workflow Session
  |
  v
V8CodeModeHost (thin frontend adapter)
  |
  v
CanonicalOrchestrationHost
  |  admission / server-owned argument injection
  |  child correlation / composition accounting
  |  exact auth + Project + Session + transport context
  v
ToolRuntime::call_tool_with_context(...)
  |
  v
canonical parsing / OAuth / Project authority
permission evaluation / Session evidence
Runner routing / ToolResult projection

webcodex-code-mode (V8 thread)
  |
  | tools.<name>(args) Promise
  +---- CodeModeHost callback ----> V8CodeModeHost
```

`webcodex-code-mode` does not depend on the root WebCodex crate, `ToolRuntime`, `AuthContext`, `RunnerRegistry`, or Session storage. It owns only one-shot JavaScript execution, JSON/V8 conversion, bounded output, nested-call scheduling, termination, and the transport-neutral `CodeModeHost` callback contract.

The root-side canonical callback implementation is intentionally no longer V8-specific. `CanonicalOrchestrationHost` owns the reusable authority-preserving nested-tool boundary; `V8CodeModeHost` only adapts the Code Mode crate's request/response types. Canonical target, recorder, context/ACK, result-expectation, and private wrapper fields are denied by the host itself; a frontend policy may add restrictions but cannot opt those Server-owned fields back in. This is an E1.x architectural probe, not a new workflow engine or stable extension API.

The V8 integration follows the minimal runtime/thread, Promise callback, microtask-checkpoint, JSON conversion, and thread-safe isolate termination patterns used by OpenAI Codex's Apache-2.0-licensed code-mode implementation. WebCodex E1 does not copy Codex's persistent cells, remote sessions, stored values, media, module ecosystem, notification protocol, or full Code Mode subsystem.

### E1.x frontend/host separation

The experiment now distinguishes the orchestration **frontend** from the canonical **host**:

```text
frontend program/runtime
    |      current: bounded V8 JavaScript
    |      possible later: tested TS composition package or structured plan
    v
CanonicalOrchestrationHost
    |
    v
canonical ToolRuntime
```

Only the V8 frontend exists today. The separation is meant to answer a narrower architectural question: can different orchestration representations share one authority, evidence, canonical dispatch, and composition-accounting boundary instead of each reimplementing WebCodex semantics? The frontend/runtime still owns program evaluation, scheduling/concurrency limits, timeout/cancellation, and output shaping. This does not add a TypeScript Composition Plugin, a Rust plan executor, bidirectional Native Plugin RPC, or another durable workflow lifecycle.

Native Tool Plugins remain capability providers. A future reusable TypeScript composition layer, if dogfood justifies one, should consume this same canonical host boundary rather than teaching the existing stdin/stdout Native Plugin protocol to call back recursively into ToolRuntime.

## JavaScript API

The global API is deliberately small:

```javascript
const status = await tools.git_status({});

const [files, hits] = await Promise.all([
  tools.read_files({
    items: [{ path: "src/lib.rs", start_line: 1, limit: 120 }]
  }),
  tools.search_project_texts({
    queries: [{
      pattern: "ToolRuntime",
      pattern_mode: "literal",
      result_mode: "files_with_matches",
      limit: 20
    }]
  })
]);

if (hits.success && shouldReadMore(hits.output)) {
  const detail = await tools.read_files({
    items: [{ path: "src/tool_runtime/kernel.rs", start_line: 1, limit: 80 }]
  });
  text({ status: status.output, detail: detail.output });
} else {
  text({ status: status.output, files: files.output, hits: hits.output });
}
```

`tools.<name>(args)` returns a Promise. An ordinary nested `ToolResult` is resolved as a JavaScript object with `success`, `output`, and optional `error`. This lets the program inspect expected business failures without turning every failure into an exception.

Host/protocol failures such as an unsupported nested tool, a forbidden target override, or failure to form a canonical ToolResult reject the Promise.

`text(value)` appends model-facing output. Strings are emitted directly; JSON-safe objects and arrays are compact-JSON encoded. Nested raw results are not automatically copied into the outer result.

Use `Promise.all` only for observations that are independent. Result-dependent follow-ups remain sequential JavaScript control flow. For one simple observation, call the ordinary tool directly; Code Mode is useful only when local orchestration removes meaningful model/tool round trips.

Code Mode should transform, select, filter, summarize, and aggregate nested observations. It should not merely concatenate complete raw `ToolResult` values. The 64 KiB output bound is intentional and is not a substitute for result selection. In particular, prefer:

```text
search -> choose relevant files/ranges -> read -> emit selected aggregate
```

over:

```text
search + many reads -> dump every raw result
```

This keeps Code Mode useful as a composition boundary rather than turning it into a larger transport envelope.

## Read-only allowlist

E1 admits only this explicit set:

```text
read_files
search_project_texts
project_overview
list_project_tracked_files
git_status
git_log
git_diff_hunks
git_review_summary
show_changes
```

Admission is not inferred from future tools. A canonical metadata regression test requires every admitted tool to remain `Observe`/read-only, non-shell-like, non-write-like, and free of mutation permission requirements.

`code_mode_exec` itself is not in the nested allowlist, so recursive Code Mode is impossible.

## Authority and Session model

The outer call requires `project`, `session_id`, and `source`.

The normal root `ToolRuntime` resolves and authorizes the outer Project and exact Workflow Session before starting V8. The root adapter receives the **resolved runtime Project id**, not merely the caller's shorthand.

JavaScript never selects its Project or Workflow Session. Nested arguments are rejected if they try to set server-owned target/recorder fields such as:

```text
project
session_id
recording_session_id
ack_session_context_revision
ack_session_message_ids
context_request
session_message_resolution
expected_failure
expected_failure_kind
result_expectation
accepted_exit_codes
assertion_name
```

The adapter injects the exact outer Project and Session and re-enters `ToolRuntime::call_tool_with_context` with the original caller authentication and transport. Canonical nested OAuth/scope checks remain enabled, including Session recording of scope denials where applicable.

Therefore nested calls continue to produce ordinary `tool_call_started` / `tool_call_finished` evidence in the same Workflow Session. E1 reduces model round trips; it does not collapse or hide canonical tool evidence.

The outer durable request audit records only bounded metadata such as Project, source byte count, and timeout. It does not persist the JavaScript source body. The durable result audit likewise retains only failure kind and orchestration stats; it does not copy emitted `content` or detailed runtime error text into Session evidence. Model-facing failures keep their detailed runtime message in the ordinary bounded ToolResult output while the durable error summary uses a fixed generic failure string. Raw request tracing follows the existing WebCodex trace policy; E1 does not introduce a separate secret or tracing system.

## No ambient host authority

The V8 isolate is not a Server shell and exposes no WebCodex host filesystem, network, process, database, environment, Runner socket, Node, or Deno API.

E1 removes or does not provide at least:

```text
console
Atomics
SharedArrayBuffer
WebAssembly
fetch
process
require
Deno
```

Dynamic imports are rejected. All meaningful Project observations must pass through `tools.<name>(args)` and therefore through the canonical root host adapter.

## Hard bounds

The current server-owned E1 limits are intentionally simple and bounded:

| Resource | Bound |
| --- | ---: |
| JavaScript source | 64 KiB UTF-8 |
| default wall clock | 5 s |
| hard wall-clock maximum | 30 s |
| nested tool calls | 32 |
| Code Mode executions concurrently active per Server process | 2 default; env-configurable 1..64 |
| nested calls concurrently in flight per execution | 8 |
| total `text()` output | 64 KiB UTF-8 |
| `text()` emissions | 256 |
| model-facing runtime failure detail | 16 KiB UTF-8 |

The V8 runtime runs on its own OS thread. A Tokio timeout is not treated as proof that CPU-bound JavaScript stopped. At the deadline, the async driver calls `v8::IsolateHandle::terminate_execution()`, signals the runtime thread, joins it, and returns a bounded timeout failure. A regression test covers `while (true) {}`.

E1 V8 execution is **Server-side**, not Runner-side. The process admits two simultaneously active Code Mode executions by default; `WEBCODEX_CODE_MODE_MAX_CONCURRENT_EXECUTIONS` may raise or lower this process-local limit within 1..64 for host-specific dogfood capacity. Waiting for a slot consumes the same wall-clock deadline. Nested Project observations still execute on the owning Runner through canonical ToolRuntime dispatch. Therefore dogfood requires a Server binary built with `--features experimental-code-mode`; existing compatible Runners do not need the feature or a protocol upgrade. Rebuilding a Runner from the same source commit is optional when exact source-alignment telemetry is desired.

## Outer result

A successful call returns only the emitted content plus small orchestration evidence:

```json
{
  "content": ["{\"status\":\"clean\"}"],
  "stats": {
    "tool_calls": 3,
    "max_in_flight": 2,
    "duration_ms": 14,
    "returned_bytes": 18
  }
}
```

The stats are experimental diagnostic evidence, not performance telemetry and not proof of model-level speedup.

## E1.5 composition observability

Phase 2 dogfood adds a diagnostic-only parent/child composition view without changing execution authority. The outer canonical `code_mode_exec` invocation keeps its existing logical invocation identity. Nested calls receive only a short-lived child ordinal for tracing and still re-enter `ToolRuntime::call_tool_with_context` as independent canonical invocations. The parent relation is not a Project, Workflow Session, ClientWindow, Job, retry, idempotency, permission, OAuth, or Runner-routing identity.

The ordinary model-facing result remains the sparse `content` + four-field `stats` shape above. Separately, RuntimeMetrics and the outer ActionAudit row may retain this bounded composition summary:

```text
nested_calls
nested_successes
nested_failures
max_in_flight
duration_ms
returned_bytes
nested_tool_counts
```

`nested_tool_counts` is limited to the explicit admitted tool set. Composition telemetry never stores JavaScript source, nested arguments, nested outputs, paths, queries, commands, credentials, raw Window identity, or arbitrary nested error text. RuntimeMetrics remains fail-open: metrics failure cannot change the `ToolResult`.

Nested canonical calls deliberately use no fabricated `ClientWindow`. One host/model-visible `code_mode_exec` request therefore remains one meaningful outer Window call, while the Runtime Console can project the bounded child summary from that outer ActionAudit row. This lets operators distinguish WebCodex-owned outer service time, Code Mode internal time, and the following outside-WebCodex inter-call gap without reclassifying nested calls as host round trips.

## Validation evidence

The E1 tests are intended to prove runtime capability, not real-model throughput:

- one `code_mode_exec` can contain several canonical nested calls;
- a second nested call can depend on the first result;
- independent Promise calls can overlap, proven with an in-flight counter/barrier rather than a wall-clock threshold;
- CPU-bound JavaScript is hard-terminated;
- source/output/call-count/concurrency bounds are enforced;
- a real root `ToolRuntime` fixture binds nested reads to the resolved Project and exact Workflow Session;
- nested target overrides and effectful tools fail closed;
- Session ledger evidence remains visible for nested calls;
- only `text()` emissions are returned to the model by default.

Actual model round-trip reduction and task wall-clock improvement require live dogfood after this branch is reviewed; unit/schema tests cannot establish those claims.

## Phase 2 Direct Tools vs Code Mode dogfood protocol

Use a real review task twice against the same repository state and comparable model context. The **Direct Tools trace is the control**; the Code Mode trace is the treatment. Record at least:

| Evidence | Direct Tools | Code Mode | Interpretation |
| --- | --- | --- | --- |
| outer model-facing tool calls | count | count | primary round-trip surface |
| canonical tool invocations | count | count | child work should not disappear |
| nested tool invocations | 0 | count | composition work moved below the model boundary |
| WebCodex-owned outer duration | per call / total | per call / total | service time owned by WebCodex |
| Window inter-call gaps | bounded samples | bounded samples | outside-WebCodex gap, not reasoning time |
| Code Mode internal duration | n/a | per outer call | V8 + nested orchestration interval |
| returned model-facing bytes | total | total | transport/result pressure |
| Runner requests | where currently provable | where currently provable | canonical backend work actually performed |
| task end-to-end wall time | observed | observed | user-visible completion interval |
| analysis/review findings quality | findings + evidence | findings + evidence | correctness/usefulness guardrail |

Canonical child calls are expected to remain visible as canonical runtime and Session evidence; Code Mode is successful only if it reduces useful **outer model/tool round trips** without degrading review quality. Do not claim that an outside-WebCodex Window gap is model reasoning time: it can include inference, network latency, host scheduling, UI work, or user interaction. Likewise, a synthetic V8 microbenchmark can characterize runtime overhead but cannot establish model-level speedup.

Prefer real ChatGPT dogfood traces over a bespoke benchmark runner while the existing telemetry is sufficient. If repeated real branch reviews do not show a meaningful round-trip, wall-time, or workflow-quality benefit, do not advance to effectful Code Mode merely because the local JavaScript runtime is fast.

## Known limitations / non-goals

E1 intentionally has no:

- shell, process, edit/write, validation, or Job tools;
- automatic Job events or Async Event Delivery;
- persistent cells or variables across calls;
- filesystem/network/module APIs;
- Computer Use, MCP, Plugin, or Agent mutation calls;
- multi-Project or cross-Session orchestration;
- Runner-side Code Mode;
- Windows/macOS Code Mode packaging guarantee;
- stable compatibility promise.

Potential later phases, only if E1 dogfood is useful:

```text
E2: effectful tools with explicit sequential effect boundaries
E3: Jobs plus event delivery
E4: decide whether Code Mode should become a durable/stable product surface
```

Do not infer E2/E3/E4 semantics from this experiment.
