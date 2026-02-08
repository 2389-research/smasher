# Smasher API Reference

Complete reference for the smasher CLI, configuration, handler interfaces, HTTP API,
event system, lint rules, and stylesheet format.

---

## Table of Contents

1. [CLI Commands](#cli-commands)
2. [Environment Variables](#environment-variables)
3. [Handler Interface](#handler-interface)
4. [Engine Configuration](#engine-configuration)
5. [HTTP API Endpoints](#http-api-endpoints)
6. [Event System](#event-system)
7. [Lint Rules](#lint-rules)
8. [Stylesheet Format](#stylesheet-format)
9. [DOT Shape-to-NodeType Mapping](#dot-shape-to-nodetype-mapping)

---

## CLI Commands

The `smasher` binary provides three subcommands for AI workflow orchestration.

### Global Flags

| Flag | Description |
|---|---|
| `-v`, `--verbose` | Enable verbose (debug-level) logging to stderr. Without this flag, only `warn`-level messages are shown. The `RUST_LOG` environment variable overrides the default filter. |
| `--env-file <PATH>` | Load environment variables from a specific `.env` file. Overrides any vars already set by the default `.env` in the current directory. |
| `-h`, `--help` | Print help information. |
| `-V`, `--version` | Print version information. |

### `smasher complete`

Send a one-shot prompt to an LLM. Streams text deltas to stdout by default.

```
smasher complete [OPTIONS] [PROMPT]
```

| Argument / Flag | Description |
|---|---|
| `<PROMPT>` | Positional prompt text. Omit if using `--file`. |
| `--file <PATH>` | Read the prompt from a file instead. |
| `--model <MODEL>` | Model identifier (default: `claude-sonnet-4-20250514`). |
| `--max-tokens <N>` | Maximum tokens to generate. |
| `--temperature <FLOAT>` | Sampling temperature (0.0 - 2.0). |
| `--system <TEXT>` | System prompt to prepend. |
| `--json` | Output the full Response as pretty-printed JSON instead of streaming text. |

Examples:

```bash
# Stream a completion to stdout
smasher complete "Explain monads in one paragraph"

# Use a specific model with a system prompt
smasher complete --model gpt-4o --system "You are a Rust expert" "How do I use Pin?"

# Read prompt from file, get JSON response
smasher complete --file prompt.txt --json

# Control generation length
smasher complete --max-tokens 200 --temperature 0.3 "Write a haiku"
```

### `smasher chat`

Start an interactive agent chat session with tool access. The agent has access to
six built-in tools: `read_file`, `write_file`, `edit_file`, `shell`, `grep`, and `glob`.

```
smasher chat [OPTIONS]
```

| Argument / Flag | Description |
|---|---|
| `--model <MODEL>` | Model identifier (default: `claude-sonnet-4-20250514`). |
| `--max-turns <N>` | Maximum agentic turns before the session ends (default: 100). |
| `--system <TEXT>` | System prompt override. |
| `--working-dir <PATH>` | Working directory for tool operations (default: current directory). |

Interactive commands:
- Type a message and press Enter to send it.
- Type `exit`, `quit`, or `/quit` to leave.
- EOF (Ctrl-D) also ends the session.

Tool call progress is printed to stderr. At session end, usage statistics are printed.

Example:

```bash
smasher chat --model claude-sonnet-4-20250514 --working-dir ./my-project
```

### `smasher run`

Execute a DOT-based pipeline.

```
smasher run [OPTIONS] <PIPELINE>
```

| Argument / Flag | Description |
|---|---|
| `<PIPELINE>` | Path to the DOT pipeline file (positional, required). |
| `--var <KEY=VALUE>` | Variable assignment, repeatable. Variables are injected into the pipeline context. |
| `--model <MODEL>` | Model identifier for codergen nodes (default: `claude-sonnet-4-20250514`). Also injected as the `model` variable. |
| `--max-steps <N>` | Maximum pipeline steps before forced stop (default: 1000). |
| `--stylesheet <PATH>` | Path to a stylesheet file for graph attribute overrides. |

The pipeline output is the final context snapshot, printed to stdout as pretty-printed JSON.

Examples:

```bash
# Run a pipeline
smasher run pipeline.dot

# With variables and a stylesheet
smasher run pipeline.dot --var env=production --var version=2.0 --stylesheet styles.css

# Limit execution steps
smasher run pipeline.dot --max-steps 50
```

### Exit Codes

| Code | Meaning |
|---|---|
| 0 | Success. |
| 1 | General / uncategorized error (`CliError::Other`). |
| 2 | LLM provider error (`CliError::Llm`). |
| 3 | Agent session error (`CliError::Session`). |
| 4 | Pipeline engine error (`CliError::Engine`). |
| 5 | Parse, resolution, or stylesheet error (`CliError::Resolution`, `CliError::DotParse`, `CliError::Stylesheet`). |
| 6 | I/O error (`CliError::Io`). |

### Output Conventions

- **stdout**: primary output (streamed text, JSON responses, pipeline results).
- **stderr**: logging, diagnostics, tool call progress, and error messages.
- Verbose mode (`-v`) enables `debug`-level tracing to stderr.
- Without `-v`, only `warn`-level messages are shown.
- The `RUST_LOG` environment variable overrides the default filter level.

---

## Environment Variables

### API Keys

The LLM client discovers providers from the environment. Set one or more:

| Variable | Provider | Required For |
|---|---|---|
| `ANTHROPIC_API_KEY` | Anthropic (Claude models) | `claude-*` model identifiers |
| `OPENAI_API_KEY` | OpenAI | `gpt-*`, `o1-*`, `o3-*` model identifiers |
| `GEMINI_API_KEY` | Google Gemini | `gemini-*` model identifiers |

At least one API key must be set. If none are found, the CLI exits with an error.

### .env File Support

The CLI loads environment variables from `.env` files in two stages:

1. **Automatic**: On startup, `dotenvy` loads `.env` from the current directory (silently
   ignored if the file does not exist).
2. **Explicit**: If `--env-file <PATH>` is passed, that file is loaded and its values
   override any previously set variables.

### Logging

| Variable | Description |
|---|---|
| `RUST_LOG` | Override the default log filter. Takes precedence over `-v`. Follows the `tracing_subscriber::EnvFilter` syntax (e.g. `debug`, `smasher_llm=trace`, `warn`). |

---

## Handler Interface

The `Handler` trait is the core abstraction for executing pipeline nodes. Every node in a
pipeline graph is processed by a handler that matches its type.

### The Handler Trait

```rust
#[async_trait]
pub trait Handler: Send + Sync {
    /// A short name identifying this handler type.
    fn name(&self) -> &str;

    /// Execute the handler for a given node with shared context.
    async fn execute(
        &self,
        node: &GraphNode,
        context: &Context,
    ) -> Result<Outcome, HandlerError>;

    /// Whether this handler can process the given node type.
    fn handles(&self, node_type: &NodeType) -> bool;
}
```

### Implementing a Custom Handler

```rust
use std::sync::Arc;
use async_trait::async_trait;
use smasher_attractor::graph::{GraphNode, NodeType};
use smasher_attractor::handler::{Handler, HandlerError, HandlerRegistry};
use smasher_attractor::state::{Context, Outcome};
use serde_json::json;

struct MyHandler;

#[async_trait]
impl Handler for MyHandler {
    fn name(&self) -> &str { "my_handler" }

    async fn execute(
        &self,
        node: &GraphNode,
        context: &Context,
    ) -> Result<Outcome, HandlerError> {
        // Read from context, do work, write results back.
        context.set(format!("{}_done", node.id), json!(true));
        Ok(Outcome::success())
    }

    fn handles(&self, node_type: &NodeType) -> bool {
        matches!(node_type, NodeType::Generic)
    }
}

// Register with a HandlerRegistry:
let mut registry = HandlerRegistry::new();
registry.register(Arc::new(MyHandler));
```

### HandlerRegistry

The `HandlerRegistry` maps node types to concrete handler implementations. The first
registered handler whose `handles()` method returns `true` for a node type wins.

| Method | Description |
|---|---|
| `HandlerRegistry::new()` | Create an empty registry. |
| `register(handler)` | Register a handler. First match wins. |
| `get_handler(node_type)` | Find the first handler that can process the given type. |
| `execute(node, context)` | Look up and execute the matching handler. Returns `HandlerError::NoHandler` if none matches. |

### Built-in Handlers

| Handler | Node Type | Behavior |
|---|---|---|
| `StartHandler` | `NodeType::Start` | Sets `_started` in context, returns success. |
| `ExitHandler` | `NodeType::Exit` | Sets `_completed` in context, returns success. |
| `ConditionalHandler` | `NodeType::Conditional` | Parses and evaluates the `condition` attribute against the context. Returns success with `{"result": true/false}`. |
| `CodergenHandler` | `NodeType::Codergen` | Delegates to a pluggable `CodergenBackend`. Uses `prompt` attribute or falls back to node label. |

### default_registry()

Returns a registry pre-loaded with `StartHandler`, `ExitHandler`, and `ConditionalHandler`.
`CodergenHandler` is excluded because it requires a `CodergenBackend` instance.

### HandlerError

```rust
pub enum HandlerError {
    ExecutionFailed { handler: String, node_id: String, message: String },
    NoHandler { node_type: String },
    Other(String),
}
```

### CodergenBackend Trait

For LLM-powered code generation nodes, implement this trait:

```rust
#[async_trait]
pub trait CodergenBackend: Send + Sync {
    async fn generate(
        &self,
        prompt: &str,
        model: Option<&str>,
        context: &Context,
    ) -> Result<Outcome, HandlerError>;
}
```

The `CodergenHandler` reads the `prompt` attribute from the node (falling back to the
node's `label`), and optionally the `model` attribute, then delegates to the backend.

---

## Engine Configuration

The `EngineConfig` struct controls pipeline execution behavior.

```rust
pub struct EngineConfig {
    /// Maximum nodes to visit before forced stop (prevents infinite loops).
    pub max_steps: usize,
    /// Whether to create checkpoints during execution.
    pub enable_checkpointing: bool,
}
```

### Defaults

| Field | Default Value |
|---|---|
| `max_steps` | 1000 |
| `enable_checkpointing` | true |

### Engine Methods

| Method | Description |
|---|---|
| `Engine::new(graph, registry)` | Create an engine with default configuration. |
| `Engine::with_config(graph, registry, config)` | Create an engine with custom configuration. |
| `engine.run(context)` | Run the pipeline from the start node. |
| `engine.run_from_checkpoint(checkpoint, context)` | Resume from a saved checkpoint. |

### ExecutionResult

The result of a completed pipeline execution:

```rust
pub struct ExecutionResult {
    pub visited_nodes: Vec<String>,
    pub node_outcomes: HashMap<String, Outcome>,
    pub final_context: HashMap<String, serde_json::Value>,
    pub steps_taken: usize,
    pub checkpoint: Option<Checkpoint>,
    pub loop_restarts: LoopCounter,
}
```

### EngineError

```rust
pub enum EngineError {
    NoStartNode,
    MultipleStartNodes { ids: Vec<String> },
    NodeNotFound { node_id: String },
    MaxStepsExceeded { max_steps: usize },
    Handler(HandlerError),
    EdgeSelection(EdgeSelectionError),
    GoalEnforcement(GoalError),
    RetryExhausted { node_id: String, message: String },
}
```

### Execution Behavior

- The engine finds the single `Start` node and walks the graph, executing handlers.
- After each node, it selects the next edge based on conditions, outcome, and priority.
- Nodes returning retryable failures are retried according to per-node `RetryPolicy`.
- `loop_restart` edges increment a loop counter and clear context entries prefixed
  with the source node's ID.
- At completion, goal gates are enforced: all nodes with `goal=true` must have been visited.
- If checkpointing is enabled, a `Checkpoint` is included in the result.

---

## HTTP API Endpoints

The `ApiRouter` defines the canonical REST API for pipeline execution over HTTP.
The default server binds to `127.0.0.1:2389`.

### Routes

| Method | Path | Description |
|---|---|---|
| `POST` | `/api/runs` | Submit a pipeline for execution. |
| `GET` | `/api/runs` | List all pipeline runs. |
| `GET` | `/api/runs/{id}` | Get status of a specific run. |
| `GET` | `/api/runs/{id}/events` | Stream run events via SSE. |
| `POST` | `/api/runs/{id}/cancel` | Cancel a running pipeline. |
| `GET` | `/api/health` | Health check endpoint. |

### POST /api/runs -- Submit Pipeline

Request body:

```json
{
  "dot_source": "digraph { start [shape=circle]; exit [shape=doublecircle]; start -> exit; }",
  "variables": {"env": "production", "version": "1.0"},
  "model": "claude-sonnet-4-20250514"
}
```

| Field | Type | Required | Description |
|---|---|---|---|
| `dot_source` | string | yes | DOT graph source defining the pipeline topology. |
| `variables` | object | yes | Key-value pairs injected into the pipeline context. |
| `model` | string | no | Optional model override for LLM nodes. |

Response body:

```json
{
  "run_id": "run-abc-123",
  "status": "running"
}
```

### GET /api/runs -- List Runs

Response body:

```json
{
  "runs": [
    {
      "run_id": "run-1",
      "graph_name": "pipeline_a",
      "started_at": "2026-02-07T10:00:00Z",
      "completed_at": null,
      "status": "running",
      "total_nodes_executed": 3,
      "variables": {}
    }
  ]
}
```

### GET /api/runs/{id} -- Run Status

Response body:

```json
{
  "run_id": "run-abc-123",
  "status": "running",
  "metadata": {
    "run_id": "run-abc-123",
    "graph_name": "my_pipeline",
    "started_at": "2026-02-07T10:00:00Z",
    "completed_at": null,
    "status": "running",
    "total_nodes_executed": 5,
    "variables": {"env": "production"}
  }
}
```

### GET /api/runs/{id}/events -- SSE Event Stream

Streams `PipelineEvent` objects as Server-Sent Events. Each event is JSON-encoded
with a `kind` tag. See the [Event System](#event-system) section for event types.

### POST /api/runs/{id}/cancel -- Cancel Run

Request body:

```json
{
  "run_id": "run-abc-123"
}
```

Response body:

```json
{
  "run_id": "run-abc-123",
  "cancelled": true
}
```

### GET /api/health -- Health Check

Returns a 200 OK if the server is running.

### Run Statuses

| Status | Description |
|---|---|
| `running` | Pipeline is actively executing. |
| `completed` | Pipeline finished successfully. |
| `failed` | Pipeline terminated with an error. |
| `aborted` | Pipeline was cancelled before completion. |

### Server Configuration

```rust
pub struct ServerConfig {
    pub host: String,           // Default: "127.0.0.1"
    pub port: u16,              // Default: 2389
    pub enable_status_endpoint: bool,   // Default: true
    pub enable_trigger_endpoint: bool,  // Default: true
}
```

---

## Event System

Pipeline execution emits structured events via `tokio::sync::broadcast` for real-time
observability. Events carry a UTC timestamp and a `kind` tag for JSON serialization.

### PipelineEvent Variants

| Kind (JSON tag) | Fields | Description |
|---|---|---|
| `node_started` | `node_id`, `node_type`, `timestamp` | A node has begun execution. |
| `node_completed` | `node_id`, `outcome`, `duration_ms`, `timestamp` | A node finished with an outcome. |
| `node_failed` | `node_id`, `error`, `duration_ms`, `timestamp` | A node execution failed. |
| `edge_traversed` | `from`, `to`, `label`, `timestamp` | An edge was followed between two nodes. |
| `human_prompt_issued` | `node_id`, `question`, `timestamp` | A human-in-the-loop prompt was issued. |
| `human_response_received` | `node_id`, `response`, `timestamp` | A response was received from the operator. |
| `context_updated` | `key`, `timestamp` | A key in the shared pipeline context was updated. |
| `checkpoint_created` | `node_id`, `timestamp` | A checkpoint was persisted. |
| `pipeline_started` | `graph_name`, `timestamp` | The pipeline began executing. |
| `pipeline_completed` | `outcome`, `total_nodes`, `duration_ms`, `timestamp` | The pipeline completed. |
| `pipeline_aborted` | `reason`, `timestamp` | The pipeline was aborted. |
| `loop_restarted` | `from`, `to`, `restart_count`, `timestamp` | A loop edge was followed. |

### Event Classification

- **Node events** (`is_node_event()`): `node_started`, `node_completed`, `node_failed`
- **Pipeline events** (`is_pipeline_event()`): `pipeline_started`, `pipeline_completed`, `pipeline_aborted`

### PipelineEventEmitter

Broadcasts events to multiple subscribers.

```rust
let emitter = PipelineEventEmitter::new(64);  // channel capacity
let mut rx = emitter.subscribe();

emitter.emit(PipelineEvent::PipelineStarted {
    graph_name: "demo".into(),
    timestamp: Utc::now(),
});

// In an async task:
let event = rx.recv().await.unwrap();
```

| Method | Description |
|---|---|
| `PipelineEventEmitter::new(capacity)` | Create with given broadcast channel capacity. |
| `PipelineEventEmitter::default()` | Create with capacity 256. |
| `emit(event)` | Send an event. Silently dropped if no subscribers. |
| `subscribe()` | Get a new receiver for future events. |
| `subscriber_count()` | Number of active subscribers. |

### PipelineEventLog

Collects events in memory for post-hoc analysis. Thread-safe via `Arc<Mutex<Vec>>`.

| Method | Description |
|---|---|
| `PipelineEventLog::new()` | Create an empty log. |
| `push(event)` | Append an event. |
| `events()` | Clone of all collected events. |
| `events_for_node(node_id)` | Events matching a specific node. |
| `len()` / `is_empty()` | Event count queries. |
| `summary()` | Build a `PipelineExecutionSummary` from collected events. Returns `None` if no `PipelineStarted` event exists. |

---

## Lint Rules

The lint system validates pipeline graph structure before execution. Each issue is
reported as a `Diagnostic` with severity, code, message, optional node ID, and suggestion.

### Built-in Rules

| Code | Severity | Rule Name | Description | Fix |
|---|---|---|---|---|
| E001 | Error | `no_start_node` | Graph has no start node. | Add a node with `shape="circle"` or `shape="point"`. |
| E002 | Error | `multiple_start_nodes` | Graph has more than one start node. | Remove extra start nodes so the graph has exactly one entry point. |
| E003 | Error | `no_exit_node` | Graph has no exit node. | Add a node with `shape="doublecircle"`. |
| W001 | Warning | `unreachable_node` | A non-start node has no incoming edges. | Add an edge leading to the node, or remove it. |
| W002 | Warning | `dead_end_node` | A non-exit node has no outgoing edges. | Add an outgoing edge or change it to an exit node. |
| W003 | Warning | `missing_condition` | A conditional node has an outgoing edge without a condition. | Add a `condition` attribute to the edge. |
| I001 | Info | `empty_label_edge` | An edge has no label or an empty label. | Add a label for clarity. |

### Severity Levels

Ordered from least to most severe: `Info` < `Warning` < `Error`.

### Using the Lint System

```rust
use smasher_attractor::lint::{LintRunner, Severity};

// Run all built-in rules:
let runner = LintRunner::with_builtins();
let report = runner.run(&graph);

if report.has_errors() {
    for diag in report.errors() {
        eprintln!("[{}] {}: {}", diag.code, diag.severity, diag.message);
        if let Some(suggestion) = &diag.suggestion {
            eprintln!("  suggestion: {}", suggestion);
        }
    }
}

// Check if the graph is fully clean (no errors or warnings):
assert!(report.is_clean());
```

### Custom Rules

Implement the `LintRule` trait and register with `LintRunner::add_rule`:

```rust
pub trait LintRule {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn check(&self, graph: &Graph) -> Vec<Diagnostic>;
}
```

### Diagnostic Structure

```rust
pub struct Diagnostic {
    pub severity: Severity,      // Info, Warning, or Error
    pub code: String,            // e.g. "E001", "W002"
    pub message: String,         // Human-readable description
    pub node_id: Option<String>, // Node involved, if applicable
    pub suggestion: Option<String>, // How to fix the issue
}
```

---

## Stylesheet Format

Stylesheets use a CSS-like syntax to configure graph node attributes without modifying
the DOT source. They support selectors, typed values, and specificity-based cascading.

### Syntax

```css
selector {
    property: value;
    property: value;
}
```

### Selectors

| Syntax | Selector Type | Specificity | Description |
|---|---|---|---|
| `*` | All | 0 (lowest) | Matches every node. |
| `codergen` | NodeType | 1 | Matches nodes of the named type. |
| `.critical` | Class | 2 | Matches nodes whose `class` attribute contains the name. |
| `#node_42` | Id | 3 (highest) | Matches the node with the exact ID. |

Valid node type names: `start`, `exit`, `codergen`, `conditional`, `tool`,
`interviewer`, `parallel`, `manager`, `subpipeline`, `generic`.

### Values

| Type | Syntax | Examples |
|---|---|---|
| String | `"quoted text"` | `"claude-sonnet-4-20250514"`, `"hello world"` |
| Number | bare numeric | `4096`, `0.7`, `3` |
| Duration | number + suffix | `30s` (seconds), `5m` (minutes), `2h` (hours) |
| Boolean | `true` / `false` | `true`, `false` |

### Comments

Block comments are supported: `/* comment text */`

### Specificity and Cascading

Rules are applied in specificity order. Within the same specificity level, later rules
override earlier ones. Properties from lower-specificity rules are preserved unless
explicitly overridden by a higher-specificity rule.

Cascade order (lowest to highest): `*` < NodeType < `.class` < `#id`

### Example Stylesheet

```css
/* Base settings for all nodes */
* {
    temperature: 0.5;
    timeout: 30s;
}

/* All codergen nodes use this model by default */
codergen {
    model: "claude-sonnet-4-20250514";
    max_tokens: 4096;
    temperature: 0.7;
}

/* Critical nodes get more retries */
.critical {
    retries: 3;
    timeout: 120s;
}

/* Override a specific node */
#final_review {
    model: "claude-opus-4-20250514";
    temperature: 0.2;
}
```

Given a node `#final_review` of type `codergen` with class `critical`, the effective
attributes are:

| Property | Value | Source |
|---|---|---|
| `temperature` | 0.2 | `#final_review` (Id, specificity 3) |
| `model` | `"claude-opus-4-20250514"` | `#final_review` (Id, specificity 3) |
| `max_tokens` | 4096 | `codergen` (NodeType, specificity 1) |
| `retries` | 3 | `.critical` (Class, specificity 2) |
| `timeout` | 120s | `.critical` (Class, specificity 2) |

### Stylesheet API

```rust
use smasher_attractor::stylesheet::Stylesheet;

let stylesheet = Stylesheet::parse(input)?;

// Get effective attributes for a node:
let attrs = stylesheet.apply(&node);

// Get all matching rules for a node:
let rules = stylesheet.matching_rules(&node);
```

---

## DOT Shape-to-NodeType Mapping

When resolving a DOT file into a semantic graph, the `shape` attribute on nodes
determines their `NodeType`.

| Shape | NodeType | Description |
|---|---|---|
| `circle`, `point` | `Start` | Entry point of the pipeline. |
| `doublecircle` | `Exit` | Terminal node. |
| `box`, `rectangle` | `Codergen` | Code generation node (runs an agent session). |
| `diamond` | `Conditional` | Conditional branching node. |
| `hexagon` | `Tool` | Tool execution node. |
| `oval`, `ellipse` | `Interviewer` | Human interaction node. |
| `parallelogram` | `Parallel` | Parallel fan-out node. |
| `house` | `Manager` | Manager/coordinator node. |
| `component` | `SubPipeline` | Sub-pipeline node referencing an external DOT file. |
| *(any other)* | `Generic` | Generic processing node. |

### Edge Attributes

| Attribute | Type | Description |
|---|---|---|
| `label` | string | Human-readable edge label. Also used as fallback for `condition`. |
| `condition` | string | Boolean expression evaluated against context (e.g. `status=done`). |
| `priority` | integer | Edge selection priority (higher values are preferred). |
| `loop_restart` | boolean | When true, traversing this edge clears source-node context entries and increments the loop counter. |

### Node Attributes

| Attribute | Type | Description |
|---|---|---|
| `shape` | string | Determines node type (see table above). |
| `label` | string | Human-readable node label. Used as fallback prompt for codergen nodes. |
| `prompt` | string | Explicit prompt for codergen nodes. |
| `model` | string | Model override for codergen nodes. |
| `condition` | string | Boolean condition for conditional nodes (e.g. `key=value`). |
| `goal` | boolean | When `true`, the engine enforces that this node is visited. |
| `class` | string | Space-separated class names for stylesheet matching. |
