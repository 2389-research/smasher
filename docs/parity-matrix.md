# Feature Parity Matrix

Cross-feature parity assessment for the smasher workspace. Each feature is marked as
**Implemented**, **Partial**, or **Not Started** based on code review of the actual source.

Last verified: 2026-03-06 against the `tui-attractor` branch (commit 94c23df).

---

## 1. DOT Parser (`smasher-attractor::dot`)

| Feature | Status | Notes |
|---------|--------|-------|
| Lexer tokenization | Implemented | 16 token types, handles identifiers, strings, numbers, keywords, HTML labels |
| Recursive descent parser | Implemented | Parses digraph/graph, node/edge/attr statements, subgraphs, defaults |
| AST types | Implemented | `DotGraph`, `DotStatement`, `DotNode`, `DotEdge`, `DotAttr`, `DotValue` |
| Graph resolution (AST to semantic Graph) | Implemented | `graph::resolve()` maps shapes to NodeType, builds edges, applies defaults |
| NodeType inference from shape | Implemented | 10 variants: Start, Exit, Codergen, Conditional, Tool, Interviewer, Parallel, Manager, SubPipeline, Generic |
| HTML label support | Partial | Lexer tokenizes HTML labels but `cli-messageboard.dot` with complex HTML triggers lexer error |
| Error reporting with position | Implemented | Parse errors include character position |
| Comment handling | Implemented | `//` and `/* */` style comments |
| Subgraph parsing | Implemented | Named and anonymous subgraphs |
| Default attribute blocks | Implemented | `node [...]` and `edge [...]` default statements |

**Test coverage:** ~40 unit tests across lexer, parser, and AST modules.

---

## 2. Engine (`smasher-attractor::engine`)

| Feature | Status | Notes |
|---------|--------|-------|
| Core execution loop | Implemented | `Engine::run()` traverses graph start-to-exit via handler dispatch |
| Handler registry dispatch | Implemented | First-match-wins via `HandlerRegistry::handle()` |
| Edge selection (5-step algorithm) | Implemented | Priority, condition evaluation, default edges, first-available fallback |
| Condition expression evaluation | Implemented | Operators: `==`, `!=`, `>`, `<`, `>=`, `<=`, `&&`, `\|\|`, `!`, parentheses |
| Loop restart via `loop_restart` edges | Implemented | `LoopCounter` with configurable max iterations |
| Step limit (`max_steps`) | Implemented | Configurable via `EngineConfig::max_steps` |
| Checkpointing during execution | Implemented | `enable_checkpointing` flag, `Checkpoint` struct with version+migration |
| Resume from checkpoint | Implemented | `Engine::run_from_checkpoint()` restores context and position |
| `ExecutionResult` output | Implemented | Returns `final_context`, `steps_taken`, `visited_nodes`, `outcome` |
| Event emission during execution | Implemented | Emits `PipelineEvent` variants during lifecycle via broadcast channel |
| Variable expansion | Implemented | `{{key}}` replacement in string attributes and labels |
| Stylesheet application | Implemented | CSS-like rules applied before variable expansion |
| Retry with exponential backoff | Implemented | `RetryPolicy` with max attempts, base/max delay, jitter |

**Test coverage:** ~33 unit tests in engine.rs, ~40 tests in condition/mod.rs, ~22 tests in edge.rs, 18 tests in retry.rs.

---

## 3. Handlers (`smasher-attractor::handler`, `tool_handler`, `manager_handler`, `interviewer`, `parallel`)

| Feature | Status | Notes |
|---------|--------|-------|
| `Handler` trait | Implemented | `async fn handle(&self, node, context) -> Result<Outcome>` |
| `StartHandler` | Implemented | Sets `_started` context key |
| `ExitHandler` | Implemented | Marks pipeline complete |
| `ConditionalHandler` | Implemented | Evaluates `condition` attr against context |
| `CodergenHandler` | Implemented | Extracts model/prompt/system_prompt from attrs, stores result in context |
| `ToolHandler` + `ToolBackend` trait | Implemented | `execute_tool(name, args)`, stores `_tool_{node_id}` in context |
| `ManagerHandler` + `ManagerBackend` trait | Implemented | `coordinate(task, config)`, stores `_manager_{node_id}` in context |
| `InterviewerHandler` + `Interviewer` trait | Implemented | `ask()`, `ask_with_options()`, `approve()` |
| `HumanGateHandler` (builder pattern) | Implemented | Timeout, default choice, node-attribute overrides |
| `AutoApproveInterviewer` | Implemented | Always approves/returns configured response |
| `QueueInterviewer` | Implemented | Pre-loaded answer queue |
| `CallbackInterviewer` | Implemented | Custom closure-based answering |
| `ConsoleInterviewer` (stdin/stdout) | Implemented | Real terminal I/O |
| `RecordingInterviewer` (decorator) | Implemented | Wraps another interviewer, records Q&A |
| `TimeoutInterviewer` (decorator) | Implemented | Wraps another interviewer with timeout and default |
| `HttpInterviewer` (REST-backed) | Implemented | Queues questions for HTTP clients via oneshot channels |
| `ParallelHandler` | Implemented | Fan-out/fan-in with bounded concurrency |
| 4 merge strategies | Implemented | MergeAll, FirstSuccess, MajorityVote, Custom |
| `HandlerRegistry` (first-match-wins) | Implemented | `register()`, `handle()`, `default_registry()` |
| `default_registry()` factory | Implemented | Pre-registers Start, Exit, Conditional, Codergen handlers |

**Test coverage:** ~25 tests (handler.rs), 14 tests (tool_handler.rs), 14 tests (manager_handler.rs), ~60 tests (interviewer.rs), ~50 tests (parallel.rs), ~30 tests (http_interviewer.rs).

---

## 4. Context and State (`smasher-attractor::state`)

| Feature | Status | Notes |
|---------|--------|-------|
| `Context` (thread-safe key-value) | Implemented | `Arc<RwLock<HashMap<String, Value>>>`, cloneable |
| `Context::set/get/get_string/contains/remove/snapshot` | Implemented | Full CRUD with JSON Value storage |
| `Checkpoint` with versioning | Implemented | `version` field + `CheckpointMigrator` trait |
| `CheckpointDiff` | Implemented | Computes added/removed/changed between checkpoints |
| `RunStore` trait | Implemented | `save_run/load_run/list_runs/delete_run` |
| `RunMetadata` | Implemented | `run_id`, `graph_name`, `status`, `created_at`, `updated_at` |
| `RunStatus` enum | Implemented | Pending, Running, Completed, Failed, Cancelled |
| `Outcome` struct | Implemented | `success` bool, optional message, `success()`/`failure()` constructors |
| `RunDirectory` layout | Implemented | Creates directory structure with manifest, checkpoints, logs, artifacts, events |
| `RunManifest` | Implemented | `run_id`, `graph_name`, `graph_hash` (SHA-256), `created_at`, `layout_version` |
| `PipelineStatus` lifecycle | Implemented | Pending->Running->Completed/Failed with node tracking |
| `PipelinePhase` enum | Implemented | Pending, Running, Paused, Completed, Failed with `is_terminal()` |

**Test coverage:** state.rs has extensive tests, ~10 tests (run_dir.rs), ~20 tests (status.rs).

---

## 5. Fidelity (`smasher-attractor::fidelity`)

| Feature | Status | Notes |
|---------|--------|-------|
| `FidelityMode` enum (8 modes) | Implemented | Full, Summarize, DropTools, DropToolResults, TruncateOld, KeepLastN, SlidingWindow, Custom |
| `FidelityProcessor` | Implemented | Applies mode rules to context/message arrays |
| `compact_context()` | Implemented | Reduces context based on active fidelity mode |
| `generate_preamble()` | Implemented | Produces summary preamble for compacted contexts |
| Mode chaining | Implemented | Multiple modes applied in sequence |
| Context window awareness | Implemented | Token counting integration |

**Test coverage:** ~50 unit tests.

---

## 6. Events (`smasher-attractor::events`)

| Feature | Status | Notes |
|---------|--------|-------|
| `PipelineEvent` enum (12 variants) | Implemented | PipelineStarted/Completed/Aborted, NodeStarted/Completed/Failed, EdgeTraversed, HumanPromptIssued/ResponseReceived, ContextUpdated, CheckpointCreated, LoopRestarted |
| `PipelineEventEmitter` (broadcast) | Implemented | `tokio::broadcast` channel, subscribe + emit |
| `PipelineEventLog` (in-memory) | Implemented | Stores and queries events by kind/node |
| Event summaries | Implemented | `total_events()`, `duration()`, `failed_nodes()`, `event_kinds()` |
| Serde serialization | Implemented | `#[serde(tag = "kind")]` for all event variants |
| Timestamp on every event | Implemented | `DateTime<Utc>` via `.timestamp()` method |
| `LogSink` trait (async) | Implemented | `append()`, `query()`, `count()` |
| `InMemoryLogSink` | Implemented | `Mutex<Vec>` + `AtomicU64` sequence counter |
| `FileLogSink` (JSONL) | Implemented | Append-only JSON Lines file |
| `LogFilter` builder | Implemented | Filter by `node_id`, `event_kinds`, `since`, `until`, `limit` |
| `LogIndex` | Implemented | Hash maps for `by_node` and `by_kind` lookups |
| `RetentionPolicy` | Implemented | `max_entries`, `max_age`, `max_file_size_bytes` |

**Test coverage:** ~50 tests (events.rs), ~40 tests (log_sink.rs).

---

## 7. Lint (`smasher-attractor::lint`, `smasher-attractor::graph::validation`)

### Static lint rules (lint.rs)

| Rule | Severity | Status |
|------|----------|--------|
| E001: No start node | Error | Implemented |
| E002: Multiple start nodes | Error | Implemented |
| E003: Unreachable node | Error | Implemented |
| W001: No exit node | Warning | Implemented |
| W002: Dead-end non-exit node | Warning | Implemented |
| W003: Self-loop edge | Warning | Implemented |
| I001: Unlabeled node | Info | Implemented |

| Feature | Status | Notes |
|---------|--------|-------|
| `LintRunner` with rule registry | Implemented | `add_rule()`, `run()`, severity filtering |
| `LintDiagnostic` output | Implemented | Rule ID, severity, message, optional node_id |
| `LintSeverity` enum | Implemented | Error, Warning, Info |
| Custom lint rule registration | Implemented | Via `LintRule` trait |

### Validation rules (graph/validation.rs)

| Rule | Severity | Status |
|------|----------|--------|
| empty_graph | Info | Implemented |
| no_start_node | Error | Implemented |
| no_exit_node | Warning | Implemented |
| multiple_start_nodes | Error | Implemented |
| unreachable_node | Warning | Implemented |
| dead_end_node | Warning | Implemented |
| self_loop | Warning | Implemented |
| orphan_node | Warning | Implemented |
| missing_edge_target | Error | Implemented |
| conditional_without_condition | Warning | Implemented |
| duplicate_edge | Warning | Implemented |
| missing_label | Info | Implemented |

**Test coverage:** ~30 tests (lint.rs), ~12 tests (validation.rs), 56 integration tests (example_lint.rs, 1 failing on HTML label edge case).

---

## 8. CLI (`smasher-cli`)

| Feature | Status | Notes |
|---------|--------|-------|
| `smasher complete "prompt"` | Implemented | One-shot streaming LLM completion to stdout |
| `--model`, `--max-tokens`, `--temperature` | Implemented | Sampling parameter flags |
| `--system` prompt override | Implemented | System prompt flag |
| `--json` full response output | Implemented | Non-streaming JSON mode |
| `--file` prompt from file | Implemented | Reads prompt from disk |
| `smasher chat` | Implemented | Interactive REPL with tool execution |
| Chat tool registry (shared tools) | Implemented | `register_shared_tools()` wires 6 tools |
| Chat event display (stderr) | Implemented | Tool call start/complete events printed to stderr |
| Chat session usage summary | Implemented | Prints turns, input/output token counts on exit |
| `smasher run pipeline.dot` | Implemented | Parses, resolves, transforms, runs pipeline |
| `--var KEY=VALUE` (repeatable) | Implemented | Injects variables into graph and context |
| `--stylesheet path` | Implemented | Loads and applies CSS-like stylesheet |
| `--max-steps` limit | Implemented | Configurable step limit (default 1000) |
| `--model` for codergen nodes | Implemented | Injected as `model` variable |
| JSON context output on success | Implemented | Final context printed as pretty JSON to stdout |
| TUI view models | Implemented | `PipelineView`, `TuiRunner`, `NodeView`, `LogLine` |
| TUI event-driven state machine | Implemented | `apply_event()` maps all 12 event types |
| TUI display formats | Implemented | Compact, Verbose, JSON, Silent via `format_event()` |
| TUI status line formatting | Implemented | `format_status_line()` for terminal status bar |
| `--verbose` flag | Implemented | Tracing to stderr at debug level |
| `--env-file` flag | Implemented | Custom .env file loading |
| Error exit codes | Implemented | `CliError::exit_code()` |
| Interactive TUI rendering (ratatui) | Implemented | Full Elm-architecture TUI via boba + ratatui (`tui/mod.rs`) with node panel, log view, status bar, and spinner |

**Test coverage:** ~80 TUI tests (tui.rs), CLI spec tests (cli_spec.rs), layout check tests (layout_check.rs).

---

## 9. HTTP / API (`smasher-attractor::server`, `http_interviewer`, `rendering`)

| Feature | Status | Notes |
|---------|--------|-------|
| `ServerConfig` | Implemented | Host, port, endpoint toggle flags |
| Route definitions (6 routes) | Implemented | Type-level route metadata: `/status`, `/trigger`, `/runs`, `/runs/{id}`, `/runs/{id}/events`, `/runs/{id}/graph` |
| Request/response types | Implemented | `TriggerRequest`, `RunSummary`, `StatusResponse`, etc. with full serde |
| `HttpInterviewer` (REST question queue) | Implemented | `GET /api/v1/questions`, `POST /api/v1/questions/{id}/answer` |
| Question queue (thread-safe) | Implemented | `QuestionQueue` with push/list/take/len |
| Graph rendering types | Implemented | `RenderFormat` (Dot/Svg/Png), `RenderOutput`, `GraphRenderer` trait |
| `DotRenderer` | Implemented | Renders Graph back to styled DOT format |
| `render_to_dot()` | Implemented | Full DOT output with node styling per NodeType |
| `GraphRenderer` trait | Implemented | Async trait for pluggable renderers |
| Render API types | Implemented | `RenderGraphQuery`, `RenderGraphResponse` |
| Actual HTTP server (axum/actix/etc.) | Implemented | `smasher-web` crate implements axum + askama + HTMX dashboard on port 21541 (`smasher serve`) |
| WebSocket event streaming | Not Started | No WebSocket support |
| Authentication / API keys | Not Started | No auth layer |

**Test coverage:** ~30 serde tests (server.rs), ~30 tests (http_interviewer.rs), ~40 tests (rendering.rs).

---

## 10. Additional Modules

### Sub-pipeline Composition (`composition.rs`)

| Feature | Status | Notes |
|---------|--------|-------|
| `compose_graphs()` | Implemented | Inlines sub-graph into parent with prefixed node IDs |
| `SubPipelineTransform` | Implemented | Resolves SubPipeline nodes from DOT files on disk |
| Edge reconnection | Implemented | Redirects edges through inlined sub-graph |
| Start/Exit node conversion | Implemented | Converts sub-graph Start/Exit to Generic |
| `CompositionError` (8 variants) | Implemented | Cycle detection, missing file, parse errors |

**Test coverage:** 20 tests.

### Artifact Store (`artifact.rs`)

| Feature | Status | Notes |
|---------|--------|-------|
| Thread-safe in-memory store | Implemented | `Arc<RwLock<HashMap>>` + `AtomicUsize` counter |
| Store/get/get_by_node/get_by_tag | Implemented | Full CRUD with tag-based queries |
| list/remove/clear/count | Implemented | Store management operations |
| Serde serialization | Implemented | `Artifact` and `ArtifactMetadata` roundtrip |

**Test coverage:** ~15 tests.

### Goal Gates (`goals.rs`)

| Feature | Status | Notes |
|---------|--------|-------|
| `GoalGate::from_graph()` | Implemented | Scans graph for nodes with `goal=true` attribute |
| `GoalStatus` with progress tracking | Implemented | `total`, `met`, `unmet`, `progress_fraction`, `Display` |
| `check()` / `enforce()` | Implemented | Validates goal completion against context/checkpoint |
| `GoalError::GoalsNotMet` | Implemented | Error with unmet goal list |

**Test coverage:** 26 tests.

### Stylesheets (`stylesheet.rs`)

| Feature | Status | Notes |
|---------|--------|-------|
| CSS-like selector syntax | Implemented | `*` (All), NodeType, `#id`, `.class` selectors |
| Style value types | Implemented | String, Number, Duration, Bool |
| Specificity cascade | Implemented | All=0 < NodeType=1 < Class=2 < Id=3 |
| Duration parsing | Implemented | `s`, `m`, `h` suffixes |
| Comment support | Implemented | `/* */` style comments |

**Test coverage:** 28 tests.

### Transforms (`transforms.rs`)

| Feature | Status | Notes |
|---------|--------|-------|
| Variable expansion `{{key}}` | Implemented | In string attributes and node labels |
| Whitespace tolerance `{{ key }}` | Implemented | Trims whitespace inside braces |
| Stylesheet application | Implemented | Node attrs take precedence over stylesheet |
| Transform ordering | Implemented | Stylesheet first, then variable expansion |

**Test coverage:** 16 tests.

---

## 11. Cross-Crate Integration

| Feature | Status | Notes |
|---------|--------|-------|
| smasher-llm types (Request, Response, Message, etc.) | Implemented | Full type system for multi-provider LLM API |
| smasher-llm streaming (StreamEvent, accumulator) | Implemented | Provider-agnostic stream processing |
| smasher-llm Client with provider auto-detection | Implemented | Detects API keys from env for Anthropic/OpenAI/Gemini |
| smasher-agent Session loop | Implemented | Turn-based agent with tool execution |
| smasher-agent ToolRegistry + shared tools | Implemented | 6 shared tools for file/shell/search operations |
| smasher-agent EventEmitter | Implemented | `tokio::broadcast` for session events |
| smasher-agent SubAgent support | Implemented | Module exists for sub-agent orchestration |
| smasher-agent Loop detection | Implemented | Module for detecting agent loops |
| CLI -> LLM integration | Implemented | `complete` and `chat` subcommands use smasher-llm Client |
| CLI -> attractor integration | Implemented | `run` subcommand uses engine, parser, transforms |
| CLI -> agent integration | Implemented | `chat` subcommand uses Session, ToolRegistry, EventEmitter |

---

## 12. Testing Summary

| Category | Count | Status |
|----------|-------|--------|
| Total test functions (workspace) | 2,704 | Implemented |
| Integration tests (example_lint.rs) | 57 total (56 pass, 1 fail) | Partial |
| Unit tests per module | See per-section notes above | Implemented |
| Pre-existing failure | `all_examples_pass_lint` fails on `cli-messageboard.dot` HTML lexer edge case | Known Issue |

---

## Legend

| Status | Meaning |
|--------|---------|
| **Implemented** | Feature is coded, tested, and functional |
| **Partial** | Feature exists but has known gaps or edge cases |
| **Not Started** | Feature is designed/typed but has no runtime implementation |
