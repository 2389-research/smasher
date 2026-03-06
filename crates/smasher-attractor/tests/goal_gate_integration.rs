// ABOUTME: Integration tests for spec-compliant goal gate enforcement and failure routing.
// ABOUTME: Exercises outcome-aware goal checking, 4-level retry fallback chain, and section 3.7 failure routing.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use smasher_attractor::engine::{Engine, EngineConfig, EngineError};
use smasher_attractor::graph::{Graph, GraphEdge, GraphNode, NodeAttrValue, NodeType};
use smasher_attractor::handler::{Handler, HandlerError, HandlerRegistry};
use smasher_attractor::state::{Context, Outcome};

// ---------------------------------------------------------------------------
// Graph construction helpers
// ---------------------------------------------------------------------------

fn make_node(id: &str, node_type: NodeType) -> GraphNode {
    GraphNode {
        id: id.to_string(),
        node_type,
        label: None,
        attrs: HashMap::new(),
    }
}

fn make_node_with_attrs(
    id: &str,
    node_type: NodeType,
    attrs: HashMap<String, NodeAttrValue>,
) -> GraphNode {
    GraphNode {
        id: id.to_string(),
        node_type,
        label: None,
        attrs,
    }
}

fn make_edge(from: &str, to: &str) -> GraphEdge {
    GraphEdge {
        from: from.to_string(),
        to: to.to_string(),
        label: None,
        condition: None,
        priority: None,
        loop_restart: false,
        attrs: HashMap::new(),
    }
}

fn make_graph(nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> Graph {
    Graph {
        name: Some("test_pipeline".to_string()),
        nodes,
        edges,
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
        graph_attrs: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// Handler that always succeeds for all nodes.
struct AlwaysSuccessHandler;

#[async_trait]
impl Handler for AlwaysSuccessHandler {
    fn name(&self) -> &str {
        "always_success"
    }
    async fn execute(
        &self,
        _node: &GraphNode,
        _context: &Context,
    ) -> Result<Outcome, HandlerError> {
        Ok(Outcome::success())
    }
    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Handler that dispatches per node ID: specified nodes fail on their Nth call,
/// all other nodes always succeed.
struct NodeSpecificHandler {
    /// Node IDs that should fail (on first call only, succeed on subsequent calls).
    fail_once_nodes: Vec<String>,
    /// Node IDs that should always fail.
    always_fail_nodes: Vec<String>,
    /// Track call counts per node ID.
    call_counts: std::sync::Mutex<HashMap<String, usize>>,
}

impl NodeSpecificHandler {
    fn fail_once(node_ids: Vec<&str>) -> Self {
        Self {
            fail_once_nodes: node_ids.into_iter().map(String::from).collect(),
            always_fail_nodes: Vec::new(),
            call_counts: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn always_fail(node_ids: Vec<&str>) -> Self {
        Self {
            fail_once_nodes: Vec::new(),
            always_fail_nodes: node_ids.into_iter().map(String::from).collect(),
            call_counts: std::sync::Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl Handler for NodeSpecificHandler {
    fn name(&self) -> &str {
        "node_specific"
    }
    async fn execute(&self, node: &GraphNode, _context: &Context) -> Result<Outcome, HandlerError> {
        let mut counts = self.call_counts.lock().unwrap();
        let count = counts.entry(node.id.clone()).or_insert(0);
        *count += 1;

        if self.always_fail_nodes.contains(&node.id) {
            return Ok(Outcome::failure("always fails"));
        }

        if self.fail_once_nodes.contains(&node.id) && *count == 1 {
            return Ok(Outcome::failure("first attempt fails"));
        }

        Ok(Outcome::success())
    }
    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

fn success_registry() -> HandlerRegistry {
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(AlwaysSuccessHandler));
    registry
}

fn config_with_max_steps(max_steps: usize) -> EngineConfig {
    EngineConfig {
        max_steps,
        ..EngineConfig::default()
    }
}

// ---------------------------------------------------------------------------
// Test 1: goal_gate_retry_on_failed_goal
// ---------------------------------------------------------------------------
// Graph: Start → DoWork(goal_gate=true, retry_target="Recovery") → Exit
//        Recovery → DoWork
// DoWork fails on first call, succeeds on second.
// Expected: Pipeline succeeds. DoWork fails → Exit detects unsatisfied goal →
//           routes to Recovery → Recovery succeeds → DoWork succeeds → Exit satisfied.
#[tokio::test]
async fn goal_gate_retry_on_failed_goal() {
    let mut dowork_attrs = HashMap::new();
    dowork_attrs.insert("goal_gate".to_string(), NodeAttrValue::Bool(true));
    dowork_attrs.insert(
        "retry_target".to_string(),
        NodeAttrValue::String("Recovery".to_string()),
    );

    let graph = make_graph(
        vec![
            make_node("Start", NodeType::Start),
            make_node_with_attrs("DoWork", NodeType::Generic, dowork_attrs),
            make_node("Recovery", NodeType::Generic),
            make_node("Exit", NodeType::Exit),
        ],
        vec![
            make_edge("Start", "DoWork"),
            make_edge("DoWork", "Exit"),
            make_edge("Recovery", "DoWork"),
        ],
    );

    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(NodeSpecificHandler::fail_once(vec!["DoWork"])));

    let engine = Engine::with_config(graph, registry, config_with_max_steps(20));
    let result = engine.run(Context::new()).await;

    let result = result.expect("pipeline should succeed");
    assert!(
        result.visited_nodes.contains(&"Recovery".to_string()),
        "Recovery should have been visited"
    );
    assert!(
        result.visited_nodes.contains(&"DoWork".to_string()),
        "DoWork should have been visited"
    );
    assert!(
        result.visited_nodes.contains(&"Exit".to_string()),
        "Exit should have been visited"
    );
}

// ---------------------------------------------------------------------------
// Test 2: goal_gate_unvisited_goal_triggers_retry
// ---------------------------------------------------------------------------
// Graph: Start → Exit, GoalNode(goal_gate=true, retry_target="Setup")
//        Setup → GoalNode → Exit
// All handlers succeed. GoalNode is never on the direct path Start→Exit.
// Expected: Pipeline succeeds. Start→Exit, goal unvisited → routes to Setup →
//           GoalNode visited+success → Exit satisfied.
#[tokio::test]
async fn goal_gate_unvisited_goal_triggers_retry() {
    let mut goal_attrs = HashMap::new();
    goal_attrs.insert("goal_gate".to_string(), NodeAttrValue::Bool(true));
    goal_attrs.insert(
        "retry_target".to_string(),
        NodeAttrValue::String("Setup".to_string()),
    );

    let graph = make_graph(
        vec![
            make_node("Start", NodeType::Start),
            make_node_with_attrs("GoalNode", NodeType::Generic, goal_attrs),
            make_node("Setup", NodeType::Generic),
            make_node("Exit", NodeType::Exit),
        ],
        vec![
            make_edge("Start", "Exit"),
            make_edge("Setup", "GoalNode"),
            make_edge("GoalNode", "Exit"),
        ],
    );

    let engine = Engine::with_config(graph, success_registry(), config_with_max_steps(20));
    let result = engine.run(Context::new()).await;

    let result = result.expect("pipeline should succeed");
    assert!(
        result.visited_nodes.contains(&"Setup".to_string()),
        "Setup should have been visited"
    );
    assert!(
        result.visited_nodes.contains(&"GoalNode".to_string()),
        "GoalNode should have been visited"
    );
    assert!(
        result.visited_nodes.contains(&"Exit".to_string()),
        "Exit should have been visited"
    );
}

// ---------------------------------------------------------------------------
// Test 3: goal_gate_no_retry_target_fails_pipeline
// ---------------------------------------------------------------------------
// Graph: Start → Exit, Required(goal_gate=true) — no retry_target anywhere
// Expected: Pipeline FAILS. Required never visited, no retry target → GoalEnforcement error.
#[tokio::test]
async fn goal_gate_no_retry_target_fails_pipeline() {
    let mut required_attrs = HashMap::new();
    required_attrs.insert("goal_gate".to_string(), NodeAttrValue::Bool(true));

    let graph = make_graph(
        vec![
            make_node("Start", NodeType::Start),
            make_node_with_attrs("Required", NodeType::Generic, required_attrs),
            make_node("Exit", NodeType::Exit),
        ],
        vec![make_edge("Start", "Exit")],
    );

    let engine = Engine::with_config(graph, success_registry(), config_with_max_steps(20));
    let result = engine.run(Context::new()).await;

    let err = result.expect_err("pipeline should fail due to unmet goal");
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("Required"),
        "error should mention the unmet goal 'Required', got: {err_msg}"
    );
    assert!(
        matches!(err, EngineError::GoalEnforcement(_)),
        "error should be GoalEnforcement variant, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 4: graph_level_retry_target_used_for_goal_gate
// ---------------------------------------------------------------------------
// Graph: Start → DoWork(goal_gate=true) → Exit, Recovery → DoWork
// graph_attrs has retry_target="Recovery"
// DoWork fails first time, succeeds second.
// Expected: Pipeline succeeds. DoWork has no node-level retry_target, graph has one →
//           falls back to graph-level → routes to Recovery → succeeds.
#[tokio::test]
async fn graph_level_retry_target_used_for_goal_gate() {
    let mut dowork_attrs = HashMap::new();
    dowork_attrs.insert("goal_gate".to_string(), NodeAttrValue::Bool(true));
    // No retry_target on the node — should fall back to graph-level

    let mut graph_attrs = HashMap::new();
    graph_attrs.insert(
        "retry_target".to_string(),
        NodeAttrValue::String("Recovery".to_string()),
    );

    let graph = Graph {
        name: Some("test_pipeline".to_string()),
        nodes: vec![
            make_node("Start", NodeType::Start),
            make_node_with_attrs("DoWork", NodeType::Generic, dowork_attrs),
            make_node("Recovery", NodeType::Generic),
            make_node("Exit", NodeType::Exit),
        ],
        edges: vec![
            make_edge("Start", "DoWork"),
            make_edge("DoWork", "Exit"),
            make_edge("Recovery", "DoWork"),
        ],
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
        graph_attrs,
    };

    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(NodeSpecificHandler::fail_once(vec!["DoWork"])));

    let engine = Engine::with_config(graph, registry, config_with_max_steps(20));
    let result = engine.run(Context::new()).await;

    let result = result.expect("pipeline should succeed via graph-level retry_target");
    assert!(
        result.visited_nodes.contains(&"Recovery".to_string()),
        "Recovery should have been visited via graph-level retry_target"
    );
    assert!(
        result.visited_nodes.contains(&"DoWork".to_string()),
        "DoWork should have been visited"
    );
    assert!(
        result.visited_nodes.contains(&"Exit".to_string()),
        "Exit should have been reached after retry"
    );
}

// ---------------------------------------------------------------------------
// Test 5: failure_routing_with_retry_target
// ---------------------------------------------------------------------------
// Graph: Start → Risky(retry_target="Recovery") → Exit, Recovery → Exit
// Risky always fails, no fail edge from Risky.
// Expected: Pipeline succeeds. Risky fails → no edge matches → retry_target sends
//           to Recovery → Recovery succeeds → Exit.
#[tokio::test]
async fn failure_routing_with_retry_target() {
    let mut risky_attrs = HashMap::new();
    risky_attrs.insert(
        "retry_target".to_string(),
        NodeAttrValue::String("Recovery".to_string()),
    );

    let graph = make_graph(
        vec![
            make_node("Start", NodeType::Start),
            make_node_with_attrs("Risky", NodeType::Generic, risky_attrs),
            make_node("Recovery", NodeType::Generic),
            make_node("Exit", NodeType::Exit),
        ],
        vec![
            make_edge("Start", "Risky"),
            // No edge from Risky to Exit — it will fail and use retry_target
            make_edge("Recovery", "Exit"),
        ],
    );

    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(NodeSpecificHandler::always_fail(vec!["Risky"])));

    let engine = Engine::with_config(graph, registry, config_with_max_steps(20));
    let result = engine.run(Context::new()).await;

    let result = result.expect("pipeline should succeed via failure routing to Recovery");
    assert!(
        result.visited_nodes.contains(&"Recovery".to_string()),
        "Recovery should have been visited after Risky failed"
    );
    assert!(
        result.visited_nodes.contains(&"Exit".to_string()),
        "Exit should have been reached"
    );
}

// ---------------------------------------------------------------------------
// Test 6: failure_routing_terminates_without_retry_target
// ---------------------------------------------------------------------------
// Graph: Start → Risky → [no edges from Risky], no retry_target
// Risky always fails.
// Expected: Pipeline terminates (loop breaks). visited_nodes has Start and Risky but not Exit.
#[tokio::test]
async fn failure_routing_terminates_without_retry_target() {
    let graph = make_graph(
        vec![
            make_node("Start", NodeType::Start),
            make_node("Risky", NodeType::Generic),
            make_node("Exit", NodeType::Exit),
        ],
        vec![
            make_edge("Start", "Risky"),
            // No edges from Risky — and no retry_target
        ],
    );

    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(NodeSpecificHandler::always_fail(vec!["Risky"])));

    let engine = Engine::with_config(graph, registry, config_with_max_steps(20));
    let result = engine.run(Context::new()).await;

    // The engine should return Ok since it just ran out of edges (no goal gates defined).
    let result = result.expect("pipeline should terminate without error (no goal gates)");
    assert!(
        result.visited_nodes.contains(&"Start".to_string()),
        "Start should have been visited"
    );
    assert!(
        result.visited_nodes.contains(&"Risky".to_string()),
        "Risky should have been visited"
    );
    assert!(
        !result.visited_nodes.contains(&"Exit".to_string()),
        "Exit should NOT have been visited — pipeline terminated early"
    );
}
