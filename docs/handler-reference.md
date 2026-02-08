<!-- ABOUTME: Reference for all handler types in the smasher-attractor pipeline engine. -->
<!-- ABOUTME: Documents handler behavior, node attributes, context keys, and backend traits. -->

# Handler Reference

## Overview

Handlers are responsible for executing pipeline nodes. Each handler implements the
`Handler` trait and declares which `NodeType` it handles. The engine uses a
`HandlerRegistry` to find the first handler that matches a node's type.

```
trait Handler: Send + Sync {
    fn name(&self) -> &str;
    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError>;
    fn handles(&self, node_type: &NodeType) -> bool;
}
```

### Outcome

Every handler returns an `Outcome`:

| Variant | Description |
|---------|-------------|
| `Success { data }` | Node completed. Optional JSON data payload. |
| `Failure { error, retryable }` | Node failed. Error message and whether retry is possible. |
| `Skip { reason }` | Node was skipped. Reason string explains why. |

### HandlerRegistry

The registry holds an ordered list of handlers. When the engine needs to execute a
node, it iterates the list and uses the first handler whose `handles()` method
returns `true` for the node's type. The `default_registry()` function creates a
registry with the built-in Start, Exit, and Conditional handlers.

---

## Built-in Handlers

### StartHandler

Handles `Start` nodes. Sets a context flag to mark the pipeline as started.

| Property | Value |
|----------|-------|
| **Name** | `start` |
| **Node type** | `Start` |
| **DOT shape** | `circle` |
| **Attributes** | None |
| **Context key** | `_started` = `true` |
| **Outcome** | Always `Success` |

### ExitHandler

Handles `Exit` nodes. Sets a context flag to mark the pipeline as completed.

| Property | Value |
|----------|-------|
| **Name** | `exit` |
| **Node type** | `Exit` |
| **DOT shape** | `doublecircle` |
| **Attributes** | None |
| **Context key** | `_completed` = `true` |
| **Outcome** | Always `Success` |

### ConditionalHandler

Handles `Conditional` nodes. Evaluates a condition expression against the current
context and returns success if the condition is true, failure if false.

| Property | Value |
|----------|-------|
| **Name** | `conditional` |
| **Node type** | `Conditional` |
| **DOT shape** | `diamond` |
| **Attributes** | `condition` (String) |
| **Outcome** | `Success` if condition is true, `Failure` if false |

The `condition` attribute is parsed using the condition expression language
(see [Config Reference](config-reference.md#condition-expressions)).

---

## CodergenHandler

Handles `Codergen` nodes. Delegates prompt execution to a pluggable `CodergenBackend`
which manages the actual LLM interaction.

| Property | Value |
|----------|-------|
| **Name** | `codergen` |
| **Node type** | `Codergen` |
| **DOT shape** | `box` |

### Attributes

| Attribute | Type | Required | Description |
|-----------|------|----------|-------------|
| `prompt` | String | No | The prompt text to send to the LLM. Falls back to node label if absent. |
| `model` | String | No | Override the LLM model for this node. |

### Backend Trait

```
trait CodergenBackend: Send + Sync {
    async fn generate(&self, prompt: &str, model: Option<&str>, context: &Context)
        -> Result<Outcome, HandlerError>;
}
```

If neither `prompt` nor label is present, returns `Failure("no prompt specified")`.

---

## ToolHandler

Handles `Tool` nodes. Delegates tool execution to a pluggable `ToolBackend`.

| Property | Value |
|----------|-------|
| **Name** | `tool` |
| **Node type** | `Tool` |
| **DOT shape** | `hexagon` |
| **Context key** | `_tool_{node_id}` |

### Attributes

| Attribute | Type | Required | Description |
|-----------|------|----------|-------------|
| `tool` | String | No | Name of the tool to invoke. Falls back to node label if absent. |
| `args` | String (JSON) | No | JSON-encoded arguments for the tool. Defaults to `{}`. |

### Backend Trait

```
trait ToolBackend: Send + Sync {
    async fn execute_tool(&self, tool_name: &str, args: &Value, context: &Context)
        -> Result<Outcome, HandlerError>;
    fn available_tools(&self) -> Vec<String>;
}
```

### Context Storage

The handler stores the outcome in context under `_tool_{node_id}`:

```json
// On success:
{ "status": "success", "data": { ... } }

// On failure:
{ "status": "failure", "error": "message", "retryable": false }

// On skip:
{ "status": "skip", "reason": "message" }
```

### Error Handling

- If `args` contains invalid JSON, returns `Failure("invalid JSON in args attribute: ...")`.
- If neither `tool` attribute nor label is present, returns `Failure("no tool specified")`.
- If the backend returns a `HandlerError`, it propagates upward.

---

## ManagerHandler

Handles `Manager` nodes. Delegates task coordination to a pluggable `ManagerBackend`.

| Property | Value |
|----------|-------|
| **Name** | `manager` |
| **Node type** | `Manager` |
| **DOT shape** | `house` |
| **Context key** | `_manager_{node_id}` |

### Attributes

| Attribute | Type | Required | Description |
|-----------|------|----------|-------------|
| `task` | String | No | Description of the task to coordinate. Falls back to node label. |
| `config` | String (JSON) | No | JSON-encoded configuration for the manager. Defaults to `{}`. |

### Backend Trait

```
trait ManagerBackend: Send + Sync {
    async fn coordinate(&self, task: &str, config: &Value, context: &Context)
        -> Result<Outcome, HandlerError>;
}
```

### Context Storage

Same structure as ToolHandler, stored under `_manager_{node_id}`.

### Error Handling

- If `config` contains invalid JSON, returns `Failure("invalid JSON in config attribute: ...")`.
- If neither `task` attribute nor label is present, returns `Failure("no task specified")`.

---

## ParallelHandler

Handles `Parallel` nodes. Executes downstream branches concurrently with bounded
concurrency and configurable failure behavior.

| Property | Value |
|----------|-------|
| **Name** | `parallel` |
| **Node type** | `Parallel` |
| **DOT shape** | `parallelogram` |

### Attributes

| Attribute | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `max_concurrency` | Number | No | `10` | Maximum number of branches to execute simultaneously. |
| `fail_fast` | Bool | No | `false` | If true, cancel remaining branches on first failure. |

### Merge Strategies

After parallel branches complete, their context changes must be merged. Available
strategies:

| Strategy | Behavior |
|----------|----------|
| `LastWriteWins` | Later writes overwrite earlier ones (default). |
| `FirstWriteWins` | First write is preserved, later writes are discarded. |
| `Collect` | All values for the same key are collected into a JSON array. |
| `Error` | Conflicting writes cause a merge error. |

### Parallel Execution

The `execute_parallel()` function uses `futures::stream::buffer_unordered` to run
branches with bounded concurrency. It returns a `ParallelResult` containing:

- `outcomes`: Map of node ID to outcome.
- `succeeded`: List of node IDs that returned `Success`.
- `failed`: List of node IDs that returned `Failure`.

---

## InterviewerHandler

Handles `Interviewer` nodes. Poses questions to a human or automated interviewer
and captures responses.

| Property | Value |
|----------|-------|
| **Name** | `interviewer` |
| **Node type** | `Interviewer` |
| **DOT shape** | `oval` or `ellipse` |
| **Context key** | `_interview_{node_id}` |

### Attributes

| Attribute | Type | Required | Description |
|-----------|------|----------|-------------|
| `question` | String | No | The question to ask. Falls back to node label. |
| `options` | String | No | Comma-separated list of answer options (multiple choice). |
| `approve` | Bool | No | If true, use the approval flow instead of free-form question. |

### Interviewer Trait

```
trait Interviewer: Send + Sync {
    async fn ask(&self, question: &str) -> Result<String, HandlerError>;
    async fn ask_with_options(&self, question: &str, options: &[String]) -> Result<String, HandlerError>;
    async fn approve(&self, question: &str) -> Result<bool, HandlerError>;
}
```

### Interviewer Implementations

| Implementation | Description |
|---------------|-------------|
| `AutoApproveInterviewer` | Always approves and returns a default answer. |
| `QueueInterviewer` | Reads answers from a pre-loaded queue (useful for testing). |
| `CallbackInterviewer` | Delegates to a caller-provided async closure. |
| `ConsoleInterviewer` | Reads answers from stdin interactively. |
| `RecordingInterviewer` | Wraps another interviewer and records all Q&A pairs. |
| `TimeoutInterviewer` | Wraps another interviewer with a configurable timeout. |
| `HttpInterviewer` | Queues questions for external HTTP clients via REST API. |

### Context Storage

The interviewer response string is stored in context under `_interview_{node_id}`.

---

## HumanGateHandler

A specialized handler for human approval gates with timeout and default behavior.

| Property | Value |
|----------|-------|
| **Name** | `human_gate` |
| **Node type** | `Interviewer` |

### Attributes

| Attribute | Type | Required | Description |
|-----------|------|----------|-------------|
| `question` | String | No | The approval question. Falls back to `prompt`, then label. |
| `prompt` | String | No | Alias for `question`. |
| `human.timeout_secs` | Number | No | Seconds to wait before using default choice. |
| `human.default_choice` | String | No | Default answer if timeout expires. |

### Behavior

1. Reads the question from `question`, `prompt`, or label (in that order).
2. If a timeout is configured, wraps the interviewer in a `TimeoutInterviewer`.
3. Calls `approve()` on the interviewer.
4. On approval: returns `Success` with the response stored under the node ID.
5. On rejection: returns `Failure("Human gate rejected")`.
6. On timeout: uses `default_choice` if available, otherwise returns `Failure`.

---

## HttpInterviewer

The HTTP interviewer enables external systems to answer pipeline questions via a
REST API.

### REST Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/questions` | List pending unanswered questions |
| `POST` | `/api/v1/questions/{id}/answer` | Submit an answer to a question |

### Question Types

| Kind | Description |
|------|-------------|
| `FreeForm` | Open-ended text question |
| `MultipleChoice` | Question with predefined answer options |
| `Approval` | Yes/no approval prompt |

### Flow

1. Pipeline reaches an interviewer node and queues a question.
2. External client polls `GET /api/v1/questions` to discover pending questions.
3. Client submits answer via `POST /api/v1/questions/{id}/answer`.
4. Pipeline receives the answer via an internal oneshot channel and resumes.

---

## SubPipelineHandler

Handles `SubPipeline` nodes. Loads and executes a nested DOT pipeline within the
current pipeline's context.

| Property | Value |
|----------|-------|
| **Node type** | `SubPipeline` |
| **DOT shape** | `component` |

### Attributes

| Attribute | Type | Required | Description |
|-----------|------|----------|-------------|
| `pipeline` | String | Yes | Path to the nested DOT pipeline file. |

---

## Node Type to Shape Mapping

The node type is determined by the `shape` attribute in the DOT file:

| DOT Shape | Node Type |
|-----------|-----------|
| `circle` | Start |
| `doublecircle` | Exit |
| `box` | Codergen |
| `diamond` | Conditional |
| `hexagon` | Tool |
| `oval` or `ellipse` | Interviewer |
| `parallelogram` | Parallel |
| `house` | Manager |
| `component` | SubPipeline |
| (anything else) | Generic |
