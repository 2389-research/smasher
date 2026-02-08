<!-- ABOUTME: Documentation for the smasher example pipelines. -->
<!-- ABOUTME: Covers shape mappings, CLI usage, variables, and stylesheets. -->

# Smasher Example Pipelines

Sample DOT pipelines for learning the smasher orchestration system.

## Quick Start

```bash
# Run the simplest pipeline
smasher run examples/hello-world.dot

# Run a conditional pipeline with a variable
smasher run examples/conditional.dot --var route=yes
```

## Shape-to-Type Mapping

The engine determines each node's role from its DOT `shape` attribute:

| Shape            | Node Type     | Purpose                                  |
|------------------|---------------|------------------------------------------|
| `circle`         | Start         | Pipeline entry point                     |
| `point`          | Start         | Alternative start marker                 |
| `doublecircle`   | Exit          | Pipeline terminal node                   |
| `diamond`        | Conditional   | Branch based on variable conditions      |
| `box`            | Codergen      | Run an LLM agent session                 |
| `rectangle`      | Codergen      | Alternative codergen marker              |
| `hexagon`        | Tool          | Execute a tool                           |
| `oval`           | Interviewer   | Human interaction node                   |
| `ellipse`        | Interviewer   | Alternative interviewer marker           |
| `parallelogram`  | Parallel      | Fan-out concurrent execution             |
| `house`          | Manager       | Coordinator node                         |
| *(anything else)*| Generic       | Passthrough processing node              |

## Running Pipelines

```bash
smasher run <pipeline.dot> [OPTIONS]
```

### Options

| Flag                  | Description                              |
|-----------------------|------------------------------------------|
| `--var KEY=VALUE`     | Set a context variable (repeatable)      |
| `--model MODEL`      | Model for codergen nodes (default: claude-sonnet-4-20250514) |
| `--max-steps N`      | Max node visits before forced stop (default: 1000) |
| `--stylesheet PATH`  | Apply a stylesheet for graph transforms  |

## How Variables Work

Variables are key-value pairs injected into the pipeline context before
execution. Conditional nodes evaluate expressions against these variables.

```bash
# Set a single variable
smasher run pipeline.dot --var status=ready

# Set multiple variables
smasher run pipeline.dot --var auth=valid --var quota=ok --var mode=fast
```

Inside a DOT file, conditional nodes reference variables in their `condition`
attribute:

```dot
check [shape=diamond, condition="status=ready"];
```

Supported condition operators:
- `key=value` — equality
- `key!=value` — inequality
- `key>N` — greater than (numeric)
- `key<N` — less than (numeric)
- `a=b && c=d` — logical AND
- `a=b || c=d` — logical OR
- `!condition` — logical NOT
- `(grouped)` — parentheses for precedence

## How Stylesheets Work

Stylesheets modify graph attributes before execution, allowing you to
override node properties without editing the DOT file.

```bash
smasher run pipeline.dot --stylesheet overrides.css
```

## How Edge Labels Work

Edge labels control routing after node execution:

- **Outcome-based**: Labels like `success`, `yes`, `true` match successful
  outcomes. Labels like `failure`, `error`, `no`, `false` match failures.
- **Condition-based**: Edges with explicit `condition` attributes are
  evaluated as expressions against the context.
- **Priority**: When multiple edges match, the one with the highest
  `priority` attribute wins (default priority is 0).

## Pipeline Output

The `smasher run` command prints the final pipeline context as JSON to
stdout. All variables (both input and those set during execution) are
included in the output.

## Examples

| File                  | Runnable | Description                                      |
|-----------------------|----------|--------------------------------------------------|
| `hello-world.dot`     | Yes      | Minimal start-to-exit pipeline                   |
| `conditional.dot`     | Yes      | Single conditional branch with two outcomes      |
| `multi-step.dot`      | Yes      | Chained conditional gates                        |
| `parallel-fanout.dot` | No*      | Concurrent branch execution (graph reference)    |
| `retry-loop.dot`      | Yes      | Retry logic with configurable backoff            |
| `human-gate.dot`      | No*      | Human approval gate before proceeding            |
| `codergen.dot`        | No*      | LLM code generation with plan-generate-review    |
| `loop-with-exit.dot`  | No*      | Explicit loop_restart iteration with exit condition |
| `multi-gate.dot`      | No*      | Multiple sequential human approval gates         |

*Pipelines marked "No" require handlers not included in the default CLI
registry. The default registry includes Start, Exit, and Conditional
handlers only. These examples document valid graph structures for use
with the programmatic Engine API or a custom handler registry.

## How Loop Restart Works

Edges can carry a `loop_restart=true` attribute to model iterative
processing. When the engine traverses a loop_restart edge, it:

1. Resets node-specific context entries for the target node
2. Increments an internal loop counter
3. Re-enters the target node for another iteration

This is distinct from the retry mechanism (which re-runs a node on
transient failure). Loop restarts are explicit graph-level iteration
driven by conditional routing.

```dot
// Loop back to process when not done
check -> process [label="failure", loop_restart=true];
```

## How Human Gates Work

Manager nodes (`shape=house`) model human-in-the-loop decision points.
The engine pauses at a Manager node and presents the `question` attribute
to the operator. The operator's response routes execution along the
matching outgoing edge.

```dot
gate [shape=house, label="Approve?", question="Do you approve this deployment?"];
gate -> proceed  [label="success"];
gate -> rollback [label="failure"];
```

Multiple Manager nodes can be chained for multi-stage approval workflows
(see `multi-gate.dot`).

## How Codergen Nodes Work

Codergen nodes (`shape=box`) spawn LLM agent sessions. Key attributes:

- `prompt` — system instruction for the agent
- `model` — LLM provider/model identifier (overridable via `--model` CLI flag)

```dot
generate [shape=box, label="Write Code", model="claude-sonnet-4-20250514", prompt="Implement the feature."];
```

Different nodes can use different models within the same pipeline for
cost/quality trade-offs (e.g., fast model for planning, powerful model
for generation).
