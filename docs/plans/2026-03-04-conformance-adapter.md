# Conformance Adapter Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Build a `./bin/conformance` CLI that bridges smasher's existing crate APIs to AttractorBench's conformance test contract, passing all 79 conformance checks across tiers 1-3.

**Architecture:** New `smasher-conformance` binary crate under `crates/`. Each of the 15 subcommands reads JSON from stdin (where applicable), calls smasher library APIs, and writes JSON to stdout. A workspace-root Makefile provides `build` and `test` targets. Types that don't implement Serialize (Graph, SessionEvent, ExecutionResult) get manual JSON conversion in a `convert` module.

**Tech Stack:** Rust (edition 2024), clap 4 (derive), serde_json, tokio, smasher-llm + smasher-agent + smasher-attractor

---

### Task 1: Scaffold the smasher-conformance crate

**Files:**
- Create: `crates/smasher-conformance/Cargo.toml`
- Create: `crates/smasher-conformance/src/main.rs`
- Modify: `Cargo.toml` (workspace members)

**Context:** The conformance binary must support 15 subcommands routed by clap. The binary name is `conformance`. All subcommands are async (tokio runtime). Tracing goes to stderr. Exit code 0 = success, non-zero = failure. Use the same clap pattern as smasher-cli/src/main.rs.

**Step 1: Create Cargo.toml**

```toml
[package]
name = "smasher-conformance"
version.workspace = true
edition.workspace = true
license.workspace = true

[[bin]]
name = "conformance"
path = "src/main.rs"

[dependencies]
smasher-llm.workspace = true
smasher-agent.workspace = true
smasher-attractor.workspace = true
clap.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
futures.workspace = true
```

**Step 2: Create main.rs with all 15 subcommands (stubs)**

```rust
// ABOUTME: Conformance adapter CLI bridging smasher crates to AttractorBench test contract.
// ABOUTME: 15 subcommands across 3 tiers: LLM SDK, Agent Loop, Attractor Pipeline.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod convert;
mod tier1;
mod tier2;
mod tier3;

#[derive(Debug, Parser)]
#[command(name = "conformance")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    // Tier 1: Unified LLM SDK
    ClientFromEnv,
    Complete,
    Stream,
    ToolCall,
    GenerateObject,
    ListModels,
    // Tier 2: Agent Loop
    SessionCreate,
    ProcessInput,
    ToolDispatch,
    Steering,
    Events,
    // Tier 3: Attractor Pipeline
    Parse { dotfile: PathBuf },
    Validate { dotfile: PathBuf },
    Run { dotfile: PathBuf },
    ListHandlers,
}

fn main() {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter("warn")
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let code = runtime.block_on(async {
        match cli.command {
            Command::ClientFromEnv => tier1::client_from_env().await,
            Command::Complete => tier1::complete().await,
            Command::Stream => tier1::stream().await,
            Command::ToolCall => tier1::tool_call().await,
            Command::GenerateObject => tier1::generate_object().await,
            Command::ListModels => tier1::list_models().await,
            Command::SessionCreate => tier2::session_create().await,
            Command::ProcessInput => tier2::process_input().await,
            Command::ToolDispatch => tier2::tool_dispatch().await,
            Command::Steering => tier2::steering().await,
            Command::Events => tier2::events().await,
            Command::Parse { dotfile } => tier3::parse(&dotfile).await,
            Command::Validate { dotfile } => tier3::validate(&dotfile).await,
            Command::Run { dotfile } => tier3::run(&dotfile).await,
            Command::ListHandlers => tier3::list_handlers().await,
        }
    });
    std::process::exit(code);
}
```

Each tier module returns `i32` (exit code). Create stub files `tier1.rs`, `tier2.rs`, `tier3.rs`, `convert.rs` that return exit code 1 with a "not implemented" message on stderr.

**Step 3: Add to workspace Cargo.toml**

Add `"crates/smasher-conformance"` to the `[workspace] members` array.

**Step 4: Create Makefile at workspace root**

```makefile
.PHONY: build test

build:
	cargo build --release -p smasher-conformance
	mkdir -p bin
	cp target/release/conformance bin/conformance

test:
	cargo test --workspace
```

**Step 5: Verify it builds**

```bash
cargo build -p smasher-conformance
./target/debug/conformance --help
```

Expected: Help text showing all 15 subcommands.

**Step 6: Commit**

```bash
git add crates/smasher-conformance/ Cargo.toml Makefile
git commit -m "feat: scaffold smasher-conformance crate with 15 subcommand stubs"
```

---

### Task 2: Implement the `convert` module for JSON serialization bridges

**Files:**
- Create: `crates/smasher-conformance/src/convert.rs`

**Context:** Several smasher types don't implement Serialize: `Graph`, `GraphNode`, `GraphEdge`, `NodeType`, `SessionEvent`, `ExecutionResult`. The conformance contract expects specific JSON shapes. This module provides `to_json()` functions for each.

**Step 1: Implement graph-to-JSON conversion**

The conformance contract expects:
```json
{
  "nodes": [{"id": "start", "shape": "Mdiamond", ...attrs}],
  "edges": [{"from": "start", "to": "step_a", ...attrs}]
}
```

Map `NodeType` → shape string: `Start→"Mdiamond"`, `Exit→"Msquare"`, `Codergen→"box"`, `Conditional→"diamond"`, `Tool→"component"`, `Interviewer→"hexagon"`, `Parallel→"parallelogram"`, `FanIn→"trapezium"`, `Manager→"house"`, `SubPipeline→"folder"`, `Generic→"ellipse"`.

Include all `attrs` from `GraphNode.attrs` and `GraphEdge.attrs` as flattened fields. Include `condition`, `label`, `priority` from the edge struct directly.

**Step 2: Implement execution-result-to-JSON conversion**

The conformance contract expects:
```json
{
  "status": "success",
  "context": {...},
  "visited_nodes": [...],
  "steps_taken": N
}
```

Derive status from whether all goal-gate nodes succeeded. Include `final_context`, `visited_nodes`, `steps_taken`, `node_outcomes`.

**Step 3: Implement session-event-to-JSON conversion**

Each SessionEvent variant → JSON object with `"type"` field:
- `SessionStarted` → `{"type": "session_start", "session_id": "..."}`
- `TurnStarted` → `{"type": "turn_start", "turn_number": N}`
- `ToolCallStarted` → `{"type": "tool_call_start", ...}`
- `ToolCallCompleted` → `{"type": "tool_call_end", ...}`
- `SessionCompleted` → `{"type": "session_end", ...}`
- etc.

**Step 4: Implement diagnostics list-to-JSON normalization**

`LintReport` → `{"diagnostics": [...]}` — Diagnostic already serializes, so just wrap in the expected shape.

**Step 5: Commit**

```bash
git add crates/smasher-conformance/src/convert.rs
git commit -m "feat(conformance): add convert module for JSON serialization bridges"
```

---

### Task 3: Implement Tier 1 subcommands (LLM SDK)

**Files:**
- Create: `crates/smasher-conformance/src/tier1.rs`

**Context:** 6 subcommands. All read JSON from stdin (except `client-from-env` and `list-models`). The mock server at localhost:9999 replaces real LLM APIs. Environment variables `OPENAI_API_KEY`, `OPENAI_BASE_URL`, `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL`, `GEMINI_API_KEY`, `GEMINI_BASE_URL` are pre-set.

**Subcommand details:**

#### `client-from-env`
- No stdin. Construct `Client::from_env()`. If any provider is configured, exit 0. On error, exit 1.
- The test also checks the inverse: with all API keys empty, exit non-zero.

#### `list-models`
- No stdin. Call the catalog to get all known models. Output a JSON array:
  ```json
  [{"id": "gpt-4o", "provider": "openai", ...}, ...]
  ```
- Use `smasher_llm::types::models_for_provider()` for each provider, collect into a Vec, serialize.

#### `complete`
- Read JSON Request from stdin. Deserialize into `smasher_llm::types::Request`.
- Check for `_test_endpoint` field — if present, override the base URL for the provider to that path (for rate-limit and auth-error tests).
- Call `client.complete(request).await`.
- On success: serialize the `Response` to stdout, exit 0.
- On error: output `{"error": "...", "error_type": "..."}` to stdout, exit 1.

**Important:** The conformance contract's Request JSON may have `"provider"` as a string field. Our `Request` already has `provider: Option<String>`, so serde deserialization should work directly.

**Important:** The mock returns OpenAI Responses API format (`"output"` array). Our Response type uses `"content"` array. The conformance tests accept EITHER `"output"`, `"content"`, OR `"choices"` — so our serialized Response (which has `"content"`) will pass.

#### `stream`
- Read JSON Request from stdin. Set `stream: true`.
- Call `client.stream(&request).await` to get the stream.
- For each `StreamEvent` yielded, serialize it as JSON and print one line to stdout.
- Exit 0 on completion.

**Critical for tests:** The conformance runner checks:
- All lines are valid JSON
- At least one event has `"text_delta"` in the serialized `event_type` (or `delta` key) — our `StreamEvent` serializes `event_type: "content_delta"` and `text_delta: "..."`. The test checks `e.get("delta")` OR `"delta" in e.get("type", "")`. Our `text_delta` field name matches `delta` substring check. Also `event_type: "content_delta"` contains "delta".
- At least one terminal event — our `StreamEvent` with `event_type: "end"` should match the check for `"done"` or `"completed"` or `"end"` in the type.

#### `tool-call`
- Read JSON Request from stdin (includes `tools` array).
- Call `client.complete(request).await`.
- Output the Response JSON. Exit 0.
- Test checks `"get_weather"` appears in serialized response, `"location"` appears in serialized response.

#### `generate-object`
- Read JSON Request from stdin (includes `response_schema`).
- The mock just returns a normal text response, but the test only checks exit 0 + valid JSON dict + mock was called.
- Call `client.complete(request).await`. Output the Response JSON. Exit 0.
- NOTE: Our Request has `response_format: Option<ResponseFormat>` but the conformance contract sends `response_schema`. We may need to map `response_schema` → `response_format` during deserialization, OR use `serde_json::Value` as an intermediate.

**Implementation approach:** Read stdin as `serde_json::Value`, extract and transform fields as needed (e.g., `response_schema` → `response_format`), then construct the `Request`.

**Step 1: Implement all 6 functions**

Each function signature: `pub async fn <name>() -> i32` (returns exit code).

Helper: `read_stdin_json() -> Result<serde_json::Value, String>` reads all of stdin, parses as JSON.

Helper: `build_client() -> Result<Client, String>` constructs `Client::from_env()`.

**Step 2: Commit**

```bash
git add crates/smasher-conformance/src/tier1.rs
git commit -m "feat(conformance): implement 6 tier-1 LLM SDK subcommands"
```

---

### Task 4: Implement Tier 2 subcommands (Agent Loop)

**Files:**
- Create: `crates/smasher-conformance/src/tier2.rs`

**Context:** 5 subcommands. The agent loop talks to the mock LLM at localhost:9999. The mock returns a plain text response (no tool calls) on the second request per path, causing the agent to complete naturally.

**Subcommand details:**

#### `session-create`
- No stdin. Create a Session with default config + mock provider.
- Output: `{"session_id": "<uuid>", "status": "created"}`. Exit 0.
- Need to construct: `SessionConfig::default()`, `Client::from_env()`, `ToolRegistry::new()`, `EventEmitter::new()`, `Session::new(config, Arc::new(client), registry, emitter)`.

#### `process-input`
- Read JSON from stdin: `{"prompt": "...", "system_prompt": "...", "_test_base_url": "..."}`.
- Create Session with the prompt's `system_prompt` if present.
- Call `session.process_input(&prompt).await`.
- Output: `{"status": "success", "result": "...", "turns": N}`.
- The `_test_base_url` field: if present and points to invalid URL, the session should fail gracefully (exit non-zero but NOT timeout/hang).
- Timeout: the conformance runner gives 60 seconds. Our agent loop should complete quickly against the mock.

**Important for mock interaction:** The mock server returns a text-only response (no tool calls) for the second request to any endpoint. The agent loop sees text-only → natural completion → exits. First request may return tool calls, but the agent processes them and loops. Register at least one shared tool (like `shell`) so the tool call roundtrip works.

#### `tool-dispatch`
- Read JSON: `{"tool_name": "...", "arguments": {...}}`.
- Create a ToolRegistry with shared tools registered.
- Call `registry.execute(tool_name, &serde_json::to_string(&arguments)?).await`.
- Output: `{"result": "...", "output": "..."}` or `{"error": "..."}`.
- For the `read_file` test: reads `/tmp/attractorbench_test_file.txt`.
- For `shell`: runs `echo hello`.
- For unknown tool: return `{"error": "unknown tool: nonexistent_tool_xyz"}`.

**Important:** The test pre-creates `/tmp/attractorbench_test_file.txt` with content `test_content_xyz`. The `read_file` tool should be able to read it.

#### `steering`
- Read JSON: `{"message": "..."}`.
- Queue the message. Output: `{"status": "queued", "acknowledged": true}`. Exit 0.
- This is a simple acknowledgment — no actual session running.

#### `events`
- Create a Session, subscribe to events, run a short task against the mock.
- Collect all SessionEvent variants, convert to JSON via `convert::session_event_to_json()`.
- Output: newline-delimited JSON, one event per line.
- Must have at least 3 events, include lifecycle start+end events.
- Timeout: 60 seconds.

**Implementation approach:** For `events`, run `process_input` in a background task while collecting events from the `EventEmitter::subscribe()` broadcast channel. Convert each event and print. After the session completes, flush remaining events.

**Step 1: Register shared tools**

Use `smasher_agent::tools::shared::register_shared_tools()` (or equivalent) to get read_file, shell, grep_search, glob, list_directory, write_file tools.

Check what shared tools are available:
- Look at `crates/smasher-agent/src/tools/shared.rs` for the tool registration functions.
- Register them into a `ToolRegistry` for process-input and tool-dispatch.

**Step 2: Implement all 5 functions**

Each function signature: `pub async fn <name>() -> i32`.

**Step 3: Commit**

```bash
git add crates/smasher-conformance/src/tier2.rs
git commit -m "feat(conformance): implement 5 tier-2 agent loop subcommands"
```

---

### Task 5: Implement Tier 3 subcommands (Attractor Pipeline)

**Files:**
- Create: `crates/smasher-conformance/src/tier3.rs`

**Context:** 4 subcommands. Uses smasher-attractor's DOT parser, lint runner, and execution engine.

**Subcommand details:**

#### `parse <dotfile>`
- Read and parse the DOT file.
- Convert the resulting `Graph` to JSON via `convert::graph_to_json()`.
- Output the JSON. Exit 0.
- Flow: `std::fs::read_to_string(dotfile)` → `smasher_attractor::dot::parse()` → `smasher_attractor::graph::resolve()` → `convert::graph_to_json()` → stdout.

#### `validate <dotfile>`
- Parse the DOT file into a Graph.
- Run `LintRunner::with_builtins().run(&graph)`.
- Output: `{"diagnostics": [...]}` — Diagnostic already serializes via serde.
- Exit 0 always (diagnostics are in the output, not signaled by exit code).
- BUT: if parse itself fails, exit non-zero (the conformance tests accept exit non-zero for validation failures too).

#### `run <dotfile>`
- Parse DOT → Graph.
- Create a `HandlerRegistry` with a mock `CodergenBackend` that calls the mock LLM at localhost:9999.
- Create an `Engine` with the graph and registry.
- Run the engine with an empty `Context`.
- Convert `ExecutionResult` to JSON via `convert::execution_result_to_json()`.
- Output the JSON. Exit 0.

**Mock CodergenBackend:** Implement a struct that takes a `Client` from smasher-llm, sends the prompt to the mock via `client.complete()`, and returns an `Outcome::success()` (or failure based on the response).

```rust
struct MockCodergenBackend {
    client: Arc<Client>,
}

#[async_trait]
impl CodergenBackend for MockCodergenBackend {
    async fn generate(&self, prompt: &str, model: Option<&str>, context: &Context) -> Result<Outcome, HandlerError> {
        let model_id = model.unwrap_or("gpt-4o");
        let request = Request::new(model_id, vec![Message::user(prompt)])
            .max_tokens(1000);
        match self.client.complete(request).await {
            Ok(response) => {
                let text = response.text().unwrap_or_default();
                Ok(Outcome::success_with(serde_json::json!({"response": text})))
            }
            Err(e) => Ok(Outcome::failure(e.to_string())),
        }
    }
}
```

Register this as the CodergenHandler's backend.

#### `list-handlers`
- Create a `default_registry()` + register CodergenHandler.
- Output a JSON array of handler descriptors:
  ```json
  [
    {"name": "start", "type": "Mdiamond", "description": "Start node handler"},
    {"name": "exit", "type": "Msquare", "description": "Exit/done node handler"},
    {"name": "codergen", "type": "box", "description": "Codergen/box node handler"},
    {"name": "conditional", "type": "diamond", "description": "Conditional routing handler"}
  ]
  ```
- The test checks (lowercased): `"start"` OR `"mdiamond"`, `"box"` OR `"codergen"`, `"exit"` OR `"msquare"` OR `"done"`.

**Step 1: Implement MockCodergenBackend**

**Step 2: Implement all 4 functions**

**Step 3: Commit**

```bash
git add crates/smasher-conformance/src/tier3.rs
git commit -m "feat(conformance): implement 4 tier-3 attractor pipeline subcommands"
```

---

### Task 6: Add missing public API surface (steer method, shared tools registration)

**Files:**
- Modify: `crates/smasher-agent/src/session.rs` — add `pub fn steer(&mut self, text: &str)` if not present
- Modify: `crates/smasher-agent/src/tools/shared.rs` — ensure shared tools are registerable from outside the crate

**Context:** The tier-2 `steering` and `tool-dispatch` subcommands need to access Session's steering queue and register shared tools. Check if these are already public. If `Session::steer()` doesn't exist, add it as a thin wrapper around `self.state.queue_steering(text)`. If shared tool registration is private, expose a `pub fn register_shared_tools(registry: &mut ToolRegistry, working_dir: &str)` function.

**Step 1: Check and add Session::steer() if needed**

**Step 2: Check and expose shared tool registration if needed**

**Step 3: Run tests to verify nothing breaks**

```bash
cargo test --workspace
```

**Step 4: Commit**

```bash
git commit -m "feat(agent): expose steer() and shared tool registration for conformance adapter"
```

---

### Task 7: Wire CodergenHandler with backend and fix handler registry access

**Files:**
- Modify: `crates/smasher-attractor/src/handler.rs` — ensure CodergenHandler can be constructed with a backend from outside

**Context:** The tier-3 `run` subcommand needs to create a CodergenHandler with a mock backend. Check if `CodergenHandler::new(backend: Arc<dyn CodergenBackend>)` is public. If not, expose it. Also ensure `HandlerRegistry::register()` can accept a CodergenHandler.

**Step 1: Check CodergenHandler constructor accessibility**

**Step 2: Expose if needed**

**Step 3: Verify**

```bash
cargo test -p smasher-attractor
```

**Step 4: Commit**

```bash
git commit -m "feat(attractor): expose CodergenHandler constructor for conformance adapter"
```

---

### Task 8: Integration testing — run conformance suite locally

**Files:**
- Create: `crates/smasher-conformance/tests/smoke.rs` — basic integration tests

**Context:** Write integration tests that exercise each subcommand with known inputs and verify the JSON output shape. These tests don't need the mock server — they test the binary's argument parsing and output format.

**Step 1: Write smoke tests for each subcommand**

Test that:
- `conformance --help` shows all subcommands
- `conformance client-from-env` with API keys set exits 0
- `conformance list-models` outputs a JSON array
- `conformance complete` with a simple request JSON exits 0 (requires mock server — skip in unit tests, test JSON parsing only)
- `conformance parse <dotfile>` with a simple DOT file outputs valid JSON with nodes and edges
- `conformance validate <dotfile>` outputs diagnostics JSON
- `conformance list-handlers` outputs a JSON array with start/exit/box handlers

**Step 2: Write DOT fixture files for tests**

Create the same DOT fixtures the conformance suite uses (simple.dot, conditional.dot, etc.) in a `tests/fixtures/` directory.

**Step 3: Run and verify**

```bash
cargo test -p smasher-conformance
```

**Step 4: Commit**

```bash
git commit -m "test(conformance): add integration smoke tests for all subcommands"
```

---

### Task 9: Request deserialization adapter

**Files:**
- Modify: `crates/smasher-conformance/src/tier1.rs` — handle conformance-specific JSON fields

**Context:** The conformance test sends requests with fields that don't map 1:1 to smasher's Request type:
- `_test_endpoint` — custom field to route to error test endpoints
- `response_schema` — should map to `response_format`
- `tools` array format — conformance uses `{"name", "description", "parameters"}` vs smasher's `ToolDefinition`

Parse stdin as `serde_json::Value` first, extract special fields, transform the rest, then construct a `Request`.

**Step 1: Implement request adapter**

```rust
fn adapt_request(mut val: serde_json::Value) -> Result<(Request, Option<String>), String> {
    let test_endpoint = val.as_object_mut()
        .and_then(|o| o.remove("_test_endpoint"))
        .and_then(|v| v.as_str().map(String::from));

    // Map response_schema → response_format if present
    if let Some(schema) = val.as_object_mut().and_then(|o| o.remove("response_schema")) {
        val.as_object_mut().unwrap().insert("response_format".to_string(),
            serde_json::json!({"type": "json_schema", "json_schema": {"schema": schema}}));
    }

    let request: Request = serde_json::from_value(val).map_err(|e| e.to_string())?;
    Ok((request, test_endpoint))
}
```

**Step 2: Wire the adapter into complete, stream, tool-call, generate-object**

**Step 3: Commit**

```bash
git commit -m "feat(conformance): add request deserialization adapter for conformance-specific fields"
```

---

### Task 10: End-to-end verification with mock server

**Files:**
- Create: `scripts/run-conformance-local.sh` — script to run conformance tests locally

**Context:** Set up the Python mock server locally, build the conformance binary, and run the conformance test suite. This verifies everything works end-to-end before submitting to AttractorBench.

**Step 1: Create the local test script**

```bash
#!/usr/bin/env bash
set -euo pipefail

# Build
make build

# Start mock server
python3 /Users/harper/Public/src/2389/attractorbench/tasks/main/tests/mock_server.py &
MOCK_PID=$!
trap "kill $MOCK_PID 2>/dev/null || true" EXIT

# Wait for health
for i in $(seq 1 20); do
    if curl -fsS http://localhost:9999/health >/dev/null 2>&1; then break; fi
    sleep 0.5
done

# Set env
export OPENAI_API_KEY=test-key
export OPENAI_BASE_URL=http://localhost:9999/v1
export ANTHROPIC_API_KEY=test-key
export ANTHROPIC_BASE_URL=http://localhost:9999
export GEMINI_API_KEY=test-key
export GEMINI_BASE_URL=http://localhost:9999

# Run conformance
python3 /Users/harper/Public/src/2389/attractorbench/tasks/main/tests/conformance/run_conformance.py \
    --tier 1 --suite full --output /tmp/conformance_tier1.json

python3 /Users/harper/Public/src/2389/attractorbench/tasks/main/tests/conformance/run_conformance.py \
    --tier 2 --suite full --output /tmp/conformance_tier2.json

python3 /Users/harper/Public/src/2389/attractorbench/tasks/main/tests/conformance/run_conformance.py \
    --tier 3 --suite full --output /tmp/conformance_tier3.json

echo "=== Results ==="
cat /tmp/conformance_tier1.json
cat /tmp/conformance_tier2.json
cat /tmp/conformance_tier3.json
```

**Step 2: Run and iterate**

Run the script, review failures, fix issues in tier1.rs/tier2.rs/tier3.rs/convert.rs.

**Step 3: Commit fixes**

```bash
git commit -m "feat(conformance): fix issues found in end-to-end conformance testing"
```

---

## Task Dependency Graph

```
Task 1 (scaffold) ──────────────────────┐
Task 6 (API surface fixes) ─────────────┤
Task 7 (handler registry access) ───────┤
                                         ▼
Task 2 (convert module) ────────► Task 3 (tier1) ──┐
                          ├─────► Task 4 (tier2) ──┤──► Task 8 (tests)
                          └─────► Task 5 (tier3) ──┘      │
                                                           ▼
Task 9 (request adapter) ──────────────────────────► Task 10 (e2e)
```

Tasks 1, 6, 7 can run in parallel. Tasks 3, 4, 5 can run in parallel after 1+2 are done. Tasks 6 and 7 should complete before 4 and 5 respectively. Task 9 can run after task 3. Task 10 runs last.
