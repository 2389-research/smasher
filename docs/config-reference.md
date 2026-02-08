<!-- ABOUTME: Configuration reference for stylesheets, fidelity modes, and condition expressions. -->
<!-- ABOUTME: Documents the stylesheet format, fidelity processing, and the condition expression language. -->

# Configuration Reference

## Stylesheets

Stylesheets provide a CSS-like mechanism for configuring pipeline node attributes
externally, without modifying the DOT file itself. They are loaded via the
`--stylesheet` CLI flag.

### Syntax

```css
selector {
    property: value;
}

/* Block comments are supported */
```

### Selectors

| Selector | Specificity | Description | Example |
|----------|-------------|-------------|---------|
| `*` | 0 (All) | Matches every node | `* { model: "gpt-4o"; }` |
| `<type>` | 1 (NodeType) | Matches nodes of a given type | `codergen { model: "gpt-4o"; }` |
| `.<class>` | 2 (Class) | Matches nodes with the given class | `.fast { temperature: 0.1; }` |
| `#<id>` | 3 (Id) | Matches a node by its exact ID | `#summarize { max_tokens: 500; }` |

### Value Types

| Type | Syntax | Examples |
|------|--------|---------|
| String | Quoted | `"hello world"`, `"gpt-4o"` |
| Number | Unquoted numeric | `42`, `3.14`, `0.7` |
| Duration | Number + suffix | `30s`, `5m`, `2h` |
| Bool | Unquoted keyword | `true`, `false` |

### Specificity and Cascading

When multiple rules match a node, they are applied in order of specificity
(lowest first). Higher-specificity rules override lower ones:

```
All (0) < NodeType (1) < Class (2) < Id (3)
```

Within the same specificity level, later rules override earlier ones.

### Example Stylesheet

```css
/* Default model for all nodes */
* {
    model: "claude-sonnet-4-20250514";
}

/* Codergen nodes get higher token limits */
codergen {
    max_tokens: 4096;
    temperature: 0.3;
}

/* Fast nodes use a cheaper model */
.fast {
    model: "gpt-4o-mini";
    temperature: 0.1;
}

/* Override a specific node */
#final_review {
    model: "claude-sonnet-4-20250514";
    max_tokens: 8192;
}
```

### Applying Stylesheets

The `apply()` method merges all matching rules for a node by specificity order.
The `matching_rules()` method returns the subset of rules that match a given node,
which is useful for debugging which styles are active.

---

## Fidelity Modes

Fidelity modes control how context is carried between pipeline nodes. They determine
how much of the accumulated context from previous nodes is preserved when entering
a new node.

### Available Modes

| Mode | Description |
|------|-------------|
| `Full` | Carry all context unchanged (default). |
| `Truncate` | Truncate context values to a maximum length (default: 1000 chars). Adds `...[truncated]` suffix. |
| `Compact` | Remove whitespace and condense context values. |
| `SummaryLow` | Generate a brief summary of the context (low detail). |
| `SummaryMedium` | Generate a moderate-detail summary of the context. |
| `SummaryHigh` | Generate a detailed summary of the context. |
| `Reset` | Clear all context, starting fresh. |
| `ResultOnly` | Keep only the most recent result, discarding history. |

### FidelityConfig

The `FidelityConfig` struct controls fidelity behavior:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_mode` | FidelityMode | `Full` | Mode applied to all edges unless overridden. |
| `edge_overrides` | Map | empty | Per-edge overrides keyed by `"from->to"` format. |
| `max_context_tokens` | Option<usize> | None | Maximum context token count (triggers compaction). |

### Edge Overrides

You can set different fidelity modes for specific edges using the `"from->to"` key
format:

```rust
let mut config = FidelityConfig::default();
config.edge_overrides.insert(
    "summarize->review".to_string(),
    FidelityMode::SummaryMedium,
);
```

### FidelityProcessor

The `FidelityProcessor` applies fidelity transformations at node boundaries:

1. Looks up the fidelity mode for the current edge (override or default).
2. Applies the mode-specific transformation to the context.
3. If `max_context_tokens` is set and exceeded, applies additional compaction.

### Mode Details

**Truncate**: Cuts each context value to a fixed character limit (default 1000).
Values that exceed the limit get `...[truncated]` appended.

**Compact**: Collapses runs of whitespace into single spaces and trims values.

**SummaryLow / SummaryMedium / SummaryHigh**: Generates a human-readable preamble
summarizing the context at varying levels of detail. The preamble is stored as the
new context.

**Reset**: Clears the context entirely. Useful when a downstream node should not
be influenced by prior state.

**ResultOnly**: Keeps only the last result entry in the context, discarding all
accumulated history.

---

## Condition Expressions

Condition expressions are used by `Conditional` nodes and edge conditions to control
pipeline flow. They are evaluated against the current pipeline context.

### Syntax

```
expression := or_expr
or_expr    := and_expr ( "||" and_expr )*
and_expr   := not_expr ( "&&" not_expr )*
not_expr   := "!" not_expr | atom
atom       := comparison | "(" expression ")" | "true" | "false"
comparison := identifier operator value
identifier := [a-zA-Z_][a-zA-Z0-9_.]*
operator   := "=" | "!=" | ">" | "<"
value      := quoted_string | number | identifier
```

### Operators

| Operator | Meaning | Example |
|----------|---------|---------|
| `=` | Equals | `status = "success"` |
| `!=` | Not equals | `status != "failed"` |
| `>` | Greater than | `count > 5` |
| `<` | Less than | `count < 100` |
| `&&` | Logical AND | `status = "ok" && count > 0` |
| `\|\|` | Logical OR | `mode = "fast" \|\| mode = "turbo"` |
| `!` | Logical NOT | `!done` |

### Operator Precedence

From lowest to highest:

1. `||` (OR)
2. `&&` (AND)
3. `!` (NOT)
4. Atoms, parenthesized expressions, comparisons

### Built-in Constants

| Constant | Value |
|----------|-------|
| `true` | Always true |
| `false` | Always false |

### Evaluation Modes

| Function | Behavior on missing variable |
|----------|------------------------------|
| `evaluate_condition()` | Lenient: missing variables evaluate to false |
| `evaluate_condition_strict()` | Strict: missing variables return an error |

### Validation

The `validate()` function checks a condition expression for syntactic correctness
without evaluating it. Returns a list of any parse errors found.

### Examples

```
# Simple equality
status = "success"

# Compound conditions
status = "success" && retries < 3

# Negation
!is_cached

# Grouped expressions
(mode = "fast" || mode = "turbo") && enabled = "true"

# Nested context keys (dot-separated)
_tool_fetch.status = "success"
```

---

## Edge Configuration

Edges in the DOT pipeline carry optional configuration that affects flow control.

### Edge Attributes

| Attribute | Type | Description |
|-----------|------|-------------|
| `label` | String | Display label and outcome-matching label for the edge. |
| `condition` | String | Condition expression evaluated at runtime. |
| `priority` | Number | Numeric priority for edge selection (higher wins). |
| `loop_restart` | Bool | If true, marks this edge as a loop-back that restarts execution. |

### Outcome Matching

Edge labels are matched against the outcome of the source node:

| Outcome | Matching labels |
|---------|----------------|
| Success | `success`, `yes`, `true` |
| Failure | `failure`, `error`, `no`, `false` |
| Skip | `skip` |

Labels are matched case-insensitively.

### Edge Selection Algorithm

The engine selects the next edge using a 5-step algorithm:

1. **Gather candidates**: Collect all outgoing edges from the current node.
2. **Evaluate conditions**: Filter out edges whose condition evaluates to false.
3. **Apply outcome matching**: Filter edges whose label matches the current outcome.
4. **Sort by priority**: Order remaining edges by priority (descending).
5. **Select highest**: Return the edge with the highest priority.

If no edges match, the pipeline halts at the current node.

---

## Engine Configuration

The pipeline engine has top-level configuration options:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_steps` | usize | `1000` | Maximum number of node executions before the engine halts. |
| `enable_checkpointing` | bool | `true` | Whether to save checkpoints during execution. |

These can be set via the CLI (`--max-steps`) or programmatically via `EngineConfig`.
