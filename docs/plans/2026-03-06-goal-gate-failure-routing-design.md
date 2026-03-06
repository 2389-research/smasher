# Goal Gate Enforcement & Failure Routing — Spec Compliance Design

**Date:** 2026-03-06
**Spec Reference:** `attractorbench/specs/attractor-spec.md` sections 3.4, 3.7
**BBS Threads:** `4b191e95` (goal-gated pipeline reached Exit despite failures), `68d796d0` (barrier nodes don't enforce branch satisfaction)

---

## Problem

The engine's goal gate enforcement has three bugs relative to the upstream attractor spec:

1. **Goal gates check visitation, not outcome.** `GoalGate::all_met()` only checks if a goal node ID appears in `visited_nodes`. A visited-but-failed goal (e.g. `L3_dashboard` timing out) still passes the check. Spec section 3.4 requires outcome to be `SUCCESS` or `PARTIAL_SUCCESS`.

2. **Retry target fallback chain is incomplete.** The engine only checks the Exit node's `retry_target` attribute. The spec says the retry target comes from the *failed goal gate node*, with a 4-level fallback: `failed_node.retry_target` → `failed_node.fallback_retry_target` → `graph.retry_target` → `graph.fallback_retry_target`.

3. **Failure routing (section 3.7) is missing.** When a node fails and edge selection finds no matching fail edge, the spec says to try the node's `retry_target` → `fallback_retry_target` before terminating the pipeline. The engine currently just breaks out of the loop.

## Non-Goals

- **FanIn/barrier node changes.** The spec says FanIn reads `parallel.results` from context and explicitly allows running "even when some candidates failed." No predecessor-satisfaction checks needed.
- **Goal gate checks at non-Exit nodes.** The spec only checks goal gates at terminal nodes (Exit). Mid-pipeline enforcement is not spec-mandated.

---

## Design

### Change 1: Outcome-aware goal gate checking

Add `UnsatisfiedGoal` struct and `check_outcomes()` method to `GoalGate` in `goals.rs`:

```rust
pub struct UnsatisfiedGoal {
    pub node_id: String,
    pub reason: String,
}

impl GoalGate {
    /// Spec section 3.4: check that all goal_gate=true nodes have
    /// SUCCESS or PARTIAL_SUCCESS outcomes.
    pub fn check_outcomes(
        &self,
        node_outcomes: &HashMap<String, Outcome>,
    ) -> Result<(), UnsatisfiedGoal> {
        for goal_id in &self.goals {
            match node_outcomes.get(goal_id) {
                Some(outcome) if outcome.is_success() => continue,
                Some(outcome) => return Err(UnsatisfiedGoal {
                    node_id: goal_id.clone(),
                    reason: format!("non-success outcome"),
                }),
                None => return Err(UnsatisfiedGoal {
                    node_id: goal_id.clone(),
                    reason: "not visited".to_string(),
                }),
            }
        }
        Ok(())
    }
}
```

Existing `all_met()`, `enforce()`, `check()` methods stay for backward compatibility (TUI progress display uses them). The engine switches to `check_outcomes()`.

### Change 2: Full retry_target fallback chain

Add a helper function to resolve the retry target from a failed goal gate node, with the spec's 4-level fallback:

```rust
/// Spec section 3.4: resolve retry target for an unsatisfied goal gate.
/// Fallback chain: node.retry_target → node.fallback_retry_target
///                → graph.retry_target → graph.fallback_retry_target
fn resolve_retry_target(
    node: &GraphNode,
    graph: &Graph,
) -> Option<String> {
    // 1. Node-level retry_target
    if let Some(NodeAttrValue::String(t)) = node.attrs.get("retry_target") {
        return Some(t.clone());
    }
    // 2. Node-level fallback_retry_target
    if let Some(NodeAttrValue::String(t)) = node.attrs.get("fallback_retry_target") {
        return Some(t.clone());
    }
    // 3. Graph-level retry_target
    if let Some(NodeAttrValue::String(t)) = graph.default_node_attrs.get("retry_target") {
        return Some(t.clone());
    }
    // 4. Graph-level fallback_retry_target
    if let Some(NodeAttrValue::String(t)) = graph.default_node_attrs.get("fallback_retry_target") {
        return Some(t.clone());
    }
    None
}
```

Note: graph-level attributes are stored in `graph[]` block in DOT. Need to verify whether they end up in `default_node_attrs` or a separate `graph_attrs` field. If the latter doesn't exist, may need to add it to the Graph struct and parser.

### Change 3: Update engine Exit handling

In `engine.rs`, the Exit node check (currently ~line 664) changes from:

```rust
// BEFORE: only checks visitation, only reads Exit node's retry_target
if node.node_type == NodeType::Exit {
    if !self.goal_gate.all_met(&visited_nodes)
        && let Some(NodeAttrValue::String(target)) =
            node.attrs.get("retry_target")
    {
        current_node_id = target.clone();
        continue;
    }
    break;
}
```

To:

```rust
// AFTER: checks outcomes, uses failed node's retry chain
if node.node_type == NodeType::Exit {
    if let Err(unsatisfied) = self.goal_gate.check_outcomes(&node_outcomes) {
        let failed_node = self.graph.node(&unsatisfied.node_id);
        if let Some(failed_node) = failed_node {
            if let Some(target) = resolve_retry_target(failed_node, &self.graph) {
                current_node_id = target;
                continue;
            }
        }
        // No retry target at any level — pipeline fails
        return Err(EngineError::GoalGateUnsatisfied {
            node_id: unsatisfied.node_id,
            reason: unsatisfied.reason,
        });
    }
    break;
}
```

### Change 4: Failure routing (Section 3.7)

After edge selection in the engine loop, if a node FAILED and no edge was selected, try the node's `retry_target` → `fallback_retry_target` before terminating:

```rust
// AFTER edge selection (currently ~line 700):
let next_edge = select_edge(&self.graph, &current_node_id, &context, Some(&outcome))?;

match next_edge {
    Some(edge) => {
        // ... existing edge handling ...
    }
    None => {
        // Spec 3.7: failure routing fallback
        if outcome.is_failure() {
            if let Some(target) = resolve_retry_target(node, &self.graph) {
                current_node_id = target;
                continue;
            }
        }
        break;
    }
}
```

### Change 5: Graph-level attributes

The DOT parser needs to store graph-level attributes (from `graph [retry_target="...", ...]` blocks) in a place the engine can read. Check if `Graph` already has a `graph_attrs` or similar field. If not, add one and update the parser.

---

## Files to Modify

| File | Change |
|------|--------|
| `crates/smasher-attractor/src/goals.rs` | Add `UnsatisfiedGoal`, `check_outcomes()` |
| `crates/smasher-attractor/src/engine.rs` | Update Exit handling, add failure routing, add `resolve_retry_target()` |
| `crates/smasher-attractor/src/graph/mod.rs` | Add `graph_attrs` field if missing, add `node()` accessor if missing |
| `crates/smasher-attractor/src/dot/parser.rs` | Store graph-level attrs in `graph_attrs` |
| `crates/smasher-attractor/src/error.rs` | Add `GoalGateUnsatisfied` variant to `EngineError` |

## Testing

- Goal gate with visited-but-failed node → should trigger retry chain
- Goal gate with unvisited node → should trigger retry chain
- 4-level retry target fallback chain (node → node fallback → graph → graph fallback)
- No retry target at any level → pipeline error
- Node failure with fail edge → follow fail edge (existing behavior, unchanged)
- Node failure with no fail edge + retry_target → jump to retry_target
- Node failure with no fail edge + no retry_target → pipeline terminates
- Existing goal gate tests continue passing
