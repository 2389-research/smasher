# Goal Gate Enforcement & Failure Routing Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make goal gate enforcement and failure routing spec-compliant per attractor-spec.md sections 3.4 and 3.7.

**Architecture:** Three independent changes to the attractor engine: (1) outcome-aware goal gate checking replaces visitation-only checking, (2) full 4-level retry_target fallback chain replaces Exit-node-only retry_target, (3) failure routing falls back to node retry_target when edge selection finds no fail edge. A prerequisite step adds graph-level attribute storage so the retry_target fallback chain can reach graph-level defaults.

**Tech Stack:** Rust 1.92, tokio, serde, thiserror, TDD

---

## Task 1: Add graph_attrs field to Graph struct and wire parser

**Files:**
- Modify: `crates/smasher-attractor/src/graph/mod.rs:72-78` (Graph struct)
- Modify: `crates/smasher-attractor/src/graph/mod.rs:257-279` (resolve function)
- Test: same file, `#[cfg(test)] mod tests`

**Why:** Graph-level attributes like `retry_target` and `fallback_retry_target` are parsed as `DotStatement::Attr` but silently dropped during resolution. The engine needs access to graph-level `retry_target` and `fallback_retry_target` for the spec's 4-level fallback chain.

**Step 1: Write failing tests**

```rust
#[test]
fn graph_level_attrs_stored() {
    let dot = r#"
        digraph G {
            graph [retry_target="start", fallback_retry_target="fallback"]
            Start [shape=Mdiamond]
            Exit [shape=Msquare]
            Start -> Exit
        }
    "#;
    let ast = crate::dot::parse(dot).unwrap();
    let g = resolve(&ast).unwrap();
    assert_eq!(
        g.graph_attrs.get("retry_target"),
        Some(&NodeAttrValue::String("start".to_string()))
    );
    assert_eq!(
        g.graph_attrs.get("fallback_retry_target"),
        Some(&NodeAttrValue::String("fallback".to_string()))
    );
}

#[test]
fn graph_level_attrs_empty_when_not_set() {
    let dot = r#"
        digraph G {
            Start [shape=Mdiamond]
            Exit [shape=Msquare]
            Start -> Exit
        }
    "#;
    let ast = crate::dot::parse(dot).unwrap();
    let g = resolve(&ast).unwrap();
    assert!(g.graph_attrs.is_empty());
}

#[test]
fn graph_level_attr_inline_syntax() {
    let dot = r#"
        digraph G {
            goal="Build a thing"
            rankdir=LR
            Start [shape=Mdiamond]
            Exit [shape=Msquare]
            Start -> Exit
        }
    "#;
    let ast = crate::dot::parse(dot).unwrap();
    let g = resolve(&ast).unwrap();
    assert_eq!(
        g.graph_attrs.get("goal"),
        Some(&NodeAttrValue::String("Build a thing".to_string()))
    );
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p smasher-attractor graph_level_attrs -- --nocapture`
Expected: FAIL — `graph_attrs` field doesn't exist on Graph

**Step 3: Add graph_attrs field and wire resolution**

In the `Graph` struct (~line 72), add:

```rust
pub struct Graph {
    pub name: Option<String>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub default_node_attrs: HashMap<String, NodeAttrValue>,
    pub default_edge_attrs: HashMap<String, NodeAttrValue>,
    pub graph_attrs: HashMap<String, NodeAttrValue>,
}
```

In `resolve()` (~line 258), add a `graph_attrs` map and collect `DotStatement::Attr` entries:

```rust
let mut graph_attrs: HashMap<String, NodeAttrValue> = HashMap::new();
```

In the first-pass loop (or a separate pass), add:

```rust
DotStatement::Attr(attr) => {
    graph_attrs.insert(attr.key.clone(), convert_value(&attr.value));
}
```

At the end of `resolve()`, include `graph_attrs` in the returned Graph struct.

**Step 4: Fix all Graph construction sites**

Grep for `Graph {` across the workspace. Every place that constructs a `Graph` struct literal needs `graph_attrs: HashMap::new()` added. Key locations:
- Test helpers in `graph/mod.rs` (e.g. `make_graph()`)
- Test helpers in `goals.rs` (`make_graph()`)
- Test helpers in `engine.rs`
- Test helpers in `edge.rs`
- Test helpers in `lint.rs`
- Any other test or production code that builds Graph directly

Use `..Default::default()` if Graph derives Default, otherwise add the field explicitly.

**Step 5: Run tests to verify they pass**

Run: `cargo test --workspace`
Expected: ALL PASS

**Step 6: Commit**

```bash
git add -A && git commit -m "feat(attractor): store graph-level attributes in Graph.graph_attrs"
```

---

## Task 2: Add UnsatisfiedGoal and outcome-aware goal gate checking

**Files:**
- Modify: `crates/smasher-attractor/src/goals.rs`
- Test: same file, `#[cfg(test)] mod tests`

**Why:** Current `GoalGate::all_met()` and `enforce()` only check visitation, not outcome status. Spec section 3.4 requires goal gate nodes to have SUCCESS or PARTIAL_SUCCESS outcomes. A visited-but-failed goal (like `L3_dashboard` timing out) must count as unsatisfied.

**Step 1: Write failing tests**

```rust
use crate::state::Outcome;
use std::collections::HashMap as StdHashMap;

#[test]
fn check_outcomes_all_succeeded() {
    let graph = make_graph(vec![make_node("g1", true), make_node("g2", true)]);
    let gate = GoalGate::from_graph(&graph);
    let mut outcomes = StdHashMap::new();
    outcomes.insert("g1".to_string(), Outcome::success());
    outcomes.insert("g2".to_string(), Outcome::success());
    assert!(gate.check_outcomes(&outcomes).is_ok());
}

#[test]
fn check_outcomes_partial_success_counts_as_met() {
    let graph = make_graph(vec![make_node("g1", true)]);
    let gate = GoalGate::from_graph(&graph);
    let mut outcomes = StdHashMap::new();
    outcomes.insert("g1".to_string(), Outcome::partial_success());
    assert!(gate.check_outcomes(&outcomes).is_ok());
}

#[test]
fn check_outcomes_failed_goal_returns_unsatisfied() {
    let graph = make_graph(vec![make_node("g1", true), make_node("g2", true)]);
    let gate = GoalGate::from_graph(&graph);
    let mut outcomes = StdHashMap::new();
    outcomes.insert("g1".to_string(), Outcome::success());
    outcomes.insert("g2".to_string(), Outcome::failure("timed out"));
    let err = gate.check_outcomes(&outcomes).unwrap_err();
    assert_eq!(err.node_id, "g2");
}

#[test]
fn check_outcomes_unvisited_goal_returns_unsatisfied() {
    let graph = make_graph(vec![make_node("g1", true), make_node("g2", true)]);
    let gate = GoalGate::from_graph(&graph);
    let mut outcomes = StdHashMap::new();
    outcomes.insert("g1".to_string(), Outcome::success());
    // g2 not in outcomes at all — unvisited
    let err = gate.check_outcomes(&outcomes).unwrap_err();
    assert_eq!(err.node_id, "g2");
    assert!(err.reason.contains("not visited"));
}

#[test]
fn check_outcomes_skipped_goal_returns_unsatisfied() {
    let graph = make_graph(vec![make_node("g1", true)]);
    let gate = GoalGate::from_graph(&graph);
    let mut outcomes = StdHashMap::new();
    outcomes.insert("g1".to_string(), Outcome::skip("branch not taken"));
    let err = gate.check_outcomes(&outcomes).unwrap_err();
    assert_eq!(err.node_id, "g1");
}

#[test]
fn check_outcomes_no_goals_always_ok() {
    let graph = make_graph(vec![make_node("a", false)]);
    let gate = GoalGate::from_graph(&graph);
    let outcomes = StdHashMap::new();
    assert!(gate.check_outcomes(&outcomes).is_ok());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p smasher-attractor check_outcomes -- --nocapture`
Expected: FAIL — `check_outcomes` method doesn't exist

**Step 3: Implement UnsatisfiedGoal and check_outcomes**

Add to `goals.rs`:

```rust
/// Describes which goal gate node is unsatisfied and why.
#[derive(Debug, Clone)]
pub struct UnsatisfiedGoal {
    pub node_id: String,
    pub reason: String,
}

impl fmt::Display for UnsatisfiedGoal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "goal '{}' unsatisfied: {}", self.node_id, self.reason)
    }
}
```

Add method to `GoalGate`:

```rust
/// Check goal gates per spec section 3.4.
///
/// All nodes with `goal_gate=true` must have an outcome of SUCCESS or
/// PARTIAL_SUCCESS. Unvisited goals (missing from outcomes map) and goals
/// with any other outcome status are considered unsatisfied.
///
/// Returns the first unsatisfied goal found, or Ok(()) if all gates pass.
pub fn check_outcomes(
    &self,
    node_outcomes: &std::collections::HashMap<String, Outcome>,
) -> Result<(), UnsatisfiedGoal> {
    for goal_id in &self.goals {
        match node_outcomes.get(goal_id) {
            Some(outcome) if outcome.is_success() => continue,
            Some(_outcome) => {
                return Err(UnsatisfiedGoal {
                    node_id: goal_id.clone(),
                    reason: "non-success outcome".to_string(),
                });
            }
            None => {
                return Err(UnsatisfiedGoal {
                    node_id: goal_id.clone(),
                    reason: "not visited".to_string(),
                });
            }
        }
    }
    Ok(())
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p smasher-attractor check_outcomes -- --nocapture`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add crates/smasher-attractor/src/goals.rs && git commit -m "feat(attractor): add outcome-aware goal gate checking (check_outcomes)"
```

---

## Task 3: Add resolve_retry_target helper with 4-level fallback chain

**Files:**
- Modify: `crates/smasher-attractor/src/engine.rs`
- Test: same file, `#[cfg(test)] mod tests`

**Why:** Spec section 3.4 defines a 4-level retry target fallback: failed_node.retry_target → failed_node.fallback_retry_target → graph.retry_target → graph.fallback_retry_target. The current engine only checks Exit node's retry_target.

**Step 1: Write failing tests**

```rust
#[test]
fn resolve_retry_target_from_node() {
    let mut attrs = HashMap::new();
    attrs.insert("retry_target".to_string(), NodeAttrValue::String("target_a".to_string()));
    let node = GraphNode {
        id: "g1".to_string(),
        node_type: NodeType::Codergen,
        label: None,
        attrs,
    };
    let graph = Graph {
        name: None,
        nodes: vec![node.clone()],
        edges: vec![],
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
        graph_attrs: HashMap::new(),
    };
    assert_eq!(resolve_retry_target(&node, &graph), Some("target_a".to_string()));
}

#[test]
fn resolve_retry_target_fallback_to_node_fallback() {
    let mut attrs = HashMap::new();
    attrs.insert("fallback_retry_target".to_string(), NodeAttrValue::String("target_b".to_string()));
    let node = GraphNode {
        id: "g1".to_string(),
        node_type: NodeType::Codergen,
        label: None,
        attrs,
    };
    let graph = Graph {
        name: None,
        nodes: vec![node.clone()],
        edges: vec![],
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
        graph_attrs: HashMap::new(),
    };
    assert_eq!(resolve_retry_target(&node, &graph), Some("target_b".to_string()));
}

#[test]
fn resolve_retry_target_fallback_to_graph_level() {
    let node = GraphNode {
        id: "g1".to_string(),
        node_type: NodeType::Codergen,
        label: None,
        attrs: HashMap::new(),
    };
    let mut graph_attrs = HashMap::new();
    graph_attrs.insert("retry_target".to_string(), NodeAttrValue::String("graph_target".to_string()));
    let graph = Graph {
        name: None,
        nodes: vec![node.clone()],
        edges: vec![],
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
        graph_attrs,
    };
    assert_eq!(resolve_retry_target(&node, &graph), Some("graph_target".to_string()));
}

#[test]
fn resolve_retry_target_fallback_to_graph_fallback() {
    let node = GraphNode {
        id: "g1".to_string(),
        node_type: NodeType::Codergen,
        label: None,
        attrs: HashMap::new(),
    };
    let mut graph_attrs = HashMap::new();
    graph_attrs.insert("fallback_retry_target".to_string(), NodeAttrValue::String("graph_fb".to_string()));
    let graph = Graph {
        name: None,
        nodes: vec![node.clone()],
        edges: vec![],
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
        graph_attrs,
    };
    assert_eq!(resolve_retry_target(&node, &graph), Some("graph_fb".to_string()));
}

#[test]
fn resolve_retry_target_none_when_no_targets() {
    let node = GraphNode {
        id: "g1".to_string(),
        node_type: NodeType::Codergen,
        label: None,
        attrs: HashMap::new(),
    };
    let graph = Graph {
        name: None,
        nodes: vec![node.clone()],
        edges: vec![],
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
        graph_attrs: HashMap::new(),
    };
    assert_eq!(resolve_retry_target(&node, &graph), None);
}

#[test]
fn resolve_retry_target_priority_order() {
    // Node retry_target takes precedence over everything else
    let mut attrs = HashMap::new();
    attrs.insert("retry_target".to_string(), NodeAttrValue::String("node_rt".to_string()));
    attrs.insert("fallback_retry_target".to_string(), NodeAttrValue::String("node_fb".to_string()));
    let node = GraphNode {
        id: "g1".to_string(),
        node_type: NodeType::Codergen,
        label: None,
        attrs,
    };
    let mut graph_attrs = HashMap::new();
    graph_attrs.insert("retry_target".to_string(), NodeAttrValue::String("graph_rt".to_string()));
    graph_attrs.insert("fallback_retry_target".to_string(), NodeAttrValue::String("graph_fb".to_string()));
    let graph = Graph {
        name: None,
        nodes: vec![node.clone()],
        edges: vec![],
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
        graph_attrs,
    };
    assert_eq!(resolve_retry_target(&node, &graph), Some("node_rt".to_string()));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p smasher-attractor resolve_retry_target -- --nocapture`
Expected: FAIL — `resolve_retry_target` doesn't exist

**Step 3: Implement resolve_retry_target**

Add as a free function in `engine.rs` (near the top, after the imports and before the Engine impl):

```rust
/// Resolve a retry target for a node using the spec's 4-level fallback chain.
///
/// Checks in order:
/// 1. node.retry_target
/// 2. node.fallback_retry_target
/// 3. graph.retry_target (graph-level attribute)
/// 4. graph.fallback_retry_target (graph-level attribute)
///
/// Returns None if no retry target is found at any level.
fn resolve_retry_target(node: &GraphNode, graph: &Graph) -> Option<String> {
    // 1. Node-level retry_target
    if let Some(NodeAttrValue::String(t)) = node.attrs.get("retry_target") {
        if !t.is_empty() {
            return Some(t.clone());
        }
    }
    // 2. Node-level fallback_retry_target
    if let Some(NodeAttrValue::String(t)) = node.attrs.get("fallback_retry_target") {
        if !t.is_empty() {
            return Some(t.clone());
        }
    }
    // 3. Graph-level retry_target
    if let Some(NodeAttrValue::String(t)) = graph.graph_attrs.get("retry_target") {
        if !t.is_empty() {
            return Some(t.clone());
        }
    }
    // 4. Graph-level fallback_retry_target
    if let Some(NodeAttrValue::String(t)) = graph.graph_attrs.get("fallback_retry_target") {
        if !t.is_empty() {
            return Some(t.clone());
        }
    }
    None
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p smasher-attractor resolve_retry_target -- --nocapture`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add crates/smasher-attractor/src/engine.rs && git commit -m "feat(attractor): add resolve_retry_target with 4-level fallback chain"
```

---

## Task 4: Update engine Exit handling to use outcome-aware goal checking

**Files:**
- Modify: `crates/smasher-attractor/src/engine.rs:661-673` (Exit node check)
- Modify: `crates/smasher-attractor/src/engine.rs:763-764` (final enforce call)
- Test: same file, `#[cfg(test)] mod tests`

**Why:** The current Exit check uses `goal_gate.all_met(&visited_nodes)` (visitation-only) and only reads Exit node's `retry_target`. Must switch to `check_outcomes(&node_outcomes)` and the 4-level fallback chain from the *failed goal gate node*, not from Exit.

**Step 1: Write failing tests**

```rust
// Test: visited-but-failed goal triggers retry target
#[tokio::test]
async fn failed_goal_gate_triggers_retry_from_failed_node() {
    // Graph:
    //   Start -> goal_node(goal_gate=true, retry_target="recovery")
    //         -> Exit
    //   recovery -> goal_node
    //
    // goal_node fails on first attempt, then succeeds on second (via recovery path).
    // Engine should detect the failed goal at Exit, find retry_target on goal_node,
    // and route to recovery.
    //
    // Use a handler that fails the first time and succeeds the second.
    // ... (test setup with appropriate mock handlers) ...
}

// Test: unvisited goal triggers retry target
#[tokio::test]
async fn unvisited_goal_gate_triggers_retry_from_unvisited_node() {
    // Graph:
    //   Start -> Exit
    //   goal_node(goal_gate=true, retry_target="setup")
    //   setup -> goal_node -> Exit
    //
    // goal_node is never visited on the direct Start->Exit path.
    // At Exit, check_outcomes sees goal_node missing, uses its retry_target.
}

// Test: no retry target at any level → pipeline error
#[tokio::test]
async fn unsatisfied_goal_no_retry_target_errors() {
    // Graph:
    //   Start -> Exit
    //   goal_node(goal_gate=true)  // no retry_target anywhere
    //
    // Pipeline should fail with GoalEnforcement error.
}

// Test: graph-level retry_target used as fallback
#[tokio::test]
async fn graph_level_retry_target_used_as_fallback() {
    // Graph with graph [retry_target="recovery"]:
    //   Start -> goal_node(goal_gate=true) -> Exit
    //   recovery -> goal_node
    //
    // goal_node has no retry_target, but graph does.
    // Engine should fall back to graph-level retry_target.
}
```

Note: The exact test bodies depend on the test infrastructure already in engine.rs. Read the existing test helpers (search for `make_test_engine` or similar patterns in the test module) and follow the same conventions.

**Step 2: Run tests to verify they fail**

Run: `cargo test -p smasher-attractor failed_goal_gate -- --nocapture`
Expected: FAIL

**Step 3: Update the Exit handling block**

Replace lines 661-673 in engine.rs:

```rust
// BEFORE:
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

// AFTER:
if node.node_type == NodeType::Exit {
    if let Err(unsatisfied) = self.goal_gate.check_outcomes(&node_outcomes) {
        // Find the failed goal node and resolve its retry target
        if let Some(failed_node) = self.graph.node(&unsatisfied.node_id) {
            if let Some(target) = resolve_retry_target(failed_node, &self.graph) {
                tracing::info!(
                    goal = %unsatisfied.node_id,
                    reason = %unsatisfied.reason,
                    retry_target = %target,
                    "goal gate unsatisfied, routing to retry target"
                );
                current_node_id = target;
                continue;
            }
        }
        // No retry target at any level — fail the pipeline
        return Err(EngineError::GoalEnforcement(GoalError::GoalsNotMet {
            unmet_count: 1,
            unmet_goals: format!("{} ({})", unsatisfied.node_id, unsatisfied.reason),
        }));
    }
    break;
}
```

Also update the final enforce call at line 763-764. The Exit block now handles all goal checking, but if the loop exits without reaching an Exit node (e.g. no outgoing edge), we still need a final check. Replace `self.goal_gate.enforce(&visited_nodes)?;` with:

```rust
if let Err(unsatisfied) = self.goal_gate.check_outcomes(&node_outcomes) {
    return Err(EngineError::GoalEnforcement(GoalError::GoalsNotMet {
        unmet_count: 1,
        unmet_goals: format!("{} ({})", unsatisfied.node_id, unsatisfied.reason),
    }));
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test --workspace`
Expected: ALL PASS (existing tests that relied on visitation-only checking may need outcome data added to their test setups)

**Step 5: Commit**

```bash
git add crates/smasher-attractor/src/engine.rs && git commit -m "fix(attractor): outcome-aware goal gate checking at Exit with 4-level retry fallback"
```

---

## Task 5: Add failure routing (spec section 3.7)

**Files:**
- Modify: `crates/smasher-attractor/src/engine.rs:744-749` (the `None => break` branch in edge selection)
- Test: same file, `#[cfg(test)] mod tests`

**Why:** Spec section 3.7 says when a node fails and edge selection finds no matching edge, the engine should try the node's `retry_target` → `fallback_retry_target` before terminating. Currently the engine just breaks out of the loop.

**Step 1: Write failing tests**

```rust
// Test: failed node with no fail edge but with retry_target routes to target
#[tokio::test]
async fn failure_routing_uses_node_retry_target() {
    // Graph:
    //   Start -> risky_node(retry_target="recovery") -> Exit
    //   recovery -> Exit
    //
    // risky_node fails, no "failure" edge exists.
    // Engine should route to recovery via retry_target.
}

// Test: failed node with no fail edge and no retry_target terminates
#[tokio::test]
async fn failure_routing_terminates_without_retry_target() {
    // Graph:
    //   Start -> risky_node -> Exit
    //
    // risky_node fails, no "failure" edge, no retry_target.
    // Engine should break out of loop (existing behavior, just verify it still works).
}

// Test: successful node with no outgoing edge still terminates normally
#[tokio::test]
async fn success_with_no_outgoing_edge_still_terminates() {
    // Ensure failure routing doesn't accidentally trigger on success outcomes.
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p smasher-attractor failure_routing -- --nocapture`
Expected: FAIL

**Step 3: Update the None branch in edge selection**

Replace lines 744-749 in engine.rs:

```rust
// BEFORE:
None => {
    // No outgoing edge, end execution
    break;
}

// AFTER:
None => {
    // Spec 3.7: When a node fails and no edge matches,
    // try the node's retry_target fallback chain.
    if outcome.is_failure() {
        if let Some(target) = resolve_retry_target(node, &self.graph) {
            tracing::info!(
                node = %current_node_id,
                retry_target = %target,
                "no fail edge found, routing to retry target"
            );
            current_node_id = target;
            continue;
        }
    }
    // No edge and no retry target (or not a failure) — end execution.
    break;
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test --workspace`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add crates/smasher-attractor/src/engine.rs && git commit -m "feat(attractor): add failure routing via retry_target fallback (spec 3.7)"
```

---

## Task 6: Integration test with full pipeline

**Files:**
- Create: `crates/smasher-attractor/tests/goal_gate_integration.rs`

**Why:** End-to-end test that exercises the full flow: a DOT pipeline with goal gates, failures, and retry targets. Validates that all three changes work together.

**Step 1: Write the integration test**

```rust
//! Integration tests for spec-compliant goal gate enforcement and failure routing.

use smasher_attractor::dot;
use smasher_attractor::engine::{Engine, EngineConfig};
use smasher_attractor::graph::resolve;
use smasher_attractor::handler::HandlerRegistry;
use smasher_attractor::state::Context;
use std::sync::Arc;

/// A pipeline where a goal-gated node fails, Exit detects it,
/// and the retry_target on the failed node routes back for a second attempt.
#[tokio::test]
async fn goal_gate_retry_target_full_pipeline() {
    let dot_source = r#"
        digraph GoalRetry {
            graph [goal="Test goal gate retry"]
            Start [shape=Mdiamond]
            Exit [shape=Msquare]

            DoWork [shape=box, goal_gate=true, retry_target="Recovery",
                    prompt="Do the work"]
            Recovery [shape=box, prompt="Recover from failure"]

            Start -> DoWork
            DoWork -> Exit
            Recovery -> DoWork
        }
    "#;

    let ast = dot::parse(dot_source).unwrap();
    let graph = resolve(&ast).unwrap();

    // Use a handler that fails on first execution of DoWork, succeeds on second.
    // (Register a stateful test handler or use the existing mock infrastructure)
    let registry = HandlerRegistry::new();
    // ... register handlers ...

    let config = EngineConfig { max_steps: 20, ..Default::default() };
    let engine = Engine::with_config(graph, registry, config);
    let context = Context::default();

    let result = engine.run(context).await;
    // Should succeed because: DoWork fails → Exit detects unsatisfied goal →
    // routes to Recovery (from DoWork's retry_target) → Recovery succeeds →
    // DoWork succeeds on second pass → Exit checks goals → all satisfied → done
    assert!(result.is_ok());
}

/// A pipeline where a goal gate has no retry target — should fail cleanly.
#[tokio::test]
async fn goal_gate_no_retry_target_fails() {
    let dot_source = r#"
        digraph NoRetry {
            Start [shape=Mdiamond]
            Exit [shape=Msquare]
            Required [shape=box, goal_gate=true, prompt="Must succeed"]

            Start -> Exit
        }
    "#;

    let ast = dot::parse(dot_source).unwrap();
    let graph = resolve(&ast).unwrap();

    let registry = HandlerRegistry::new();
    // ... register handlers ...

    let config = EngineConfig::default();
    let engine = Engine::with_config(graph, registry, config);
    let context = Context::default();

    let result = engine.run(context).await;
    // Required was never visited, no retry_target → should fail
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Required") || err.contains("not visited"));
}
```

Note: The exact test handler setup depends on existing test patterns in `engine.rs`. Read the test module to understand how mock handlers are set up (look for `TestHandler`, `StatefulHandler`, or similar patterns) and follow the same approach.

**Step 2: Run integration tests**

Run: `cargo test -p smasher-attractor --test goal_gate_integration -- --nocapture`
Expected: ALL PASS

**Step 3: Run full workspace tests**

Run: `cargo test --workspace`
Expected: ALL PASS

**Step 4: Commit**

```bash
git add crates/smasher-attractor/tests/goal_gate_integration.rs && git commit -m "test(attractor): integration tests for goal gate enforcement and failure routing"
```

---

## Summary

| Task | Files | What |
|------|-------|------|
| 1 | graph/mod.rs | Add `graph_attrs` field, wire parser |
| 2 | goals.rs | Add `UnsatisfiedGoal`, `check_outcomes()` |
| 3 | engine.rs | Add `resolve_retry_target()` helper |
| 4 | engine.rs | Update Exit handling + final enforce |
| 5 | engine.rs | Add failure routing (spec 3.7) |
| 6 | tests/ | Integration test |

Tasks 1-3 are independent. Task 4 depends on 1, 2, and 3. Task 5 depends on 3. Task 6 depends on 4 and 5.

Recommended execution order: 1 → 2 → 3 → 4 → 5 → 6
