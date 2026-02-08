// ABOUTME: Reusable test fixtures for smasher-attractor integration tests.
// ABOUTME: Provides graph builders for common pipeline topologies and test handler utilities.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::json;

use smasher_attractor::graph::{Graph, GraphEdge, GraphNode, NodeType};
use smasher_attractor::handler::{Handler, HandlerError};
use smasher_attractor::state::{Context, Outcome};

// ---------------------------------------------------------------------------
// Graph construction helpers (low-level)
// ---------------------------------------------------------------------------

/// Build a minimal GraphNode with the given type and no extra attributes.
fn make_node(id: &str, node_type: NodeType) -> GraphNode {
    GraphNode {
        id: id.to_string(),
        node_type,
        label: None,
        attrs: HashMap::new(),
    }
}

/// Build a minimal GraphEdge with no conditions, labels, or special attributes.
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

/// Build a conditional edge with the given condition expression.
fn make_conditional_edge(from: &str, to: &str, condition: &str) -> GraphEdge {
    GraphEdge {
        from: from.to_string(),
        to: to.to_string(),
        label: None,
        condition: Some(condition.to_string()),
        priority: None,
        loop_restart: false,
        attrs: HashMap::new(),
    }
}

/// Build a loop-restart edge that clears source-prefixed context entries.
fn make_loop_restart_edge(from: &str, to: &str) -> GraphEdge {
    GraphEdge {
        from: from.to_string(),
        to: to.to_string(),
        label: None,
        condition: None,
        priority: None,
        loop_restart: true,
        attrs: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Pipeline fixtures
// ---------------------------------------------------------------------------

/// Create a linear pipeline: start -> box_0 -> box_1 -> ... -> box_(n-1) -> exit.
///
/// `n` is the number of intermediate Generic nodes between start and exit.
/// When `n == 0`, the graph is just start -> exit.
pub fn linear_pipeline(n: usize) -> Graph {
    let mut nodes = Vec::with_capacity(n + 2);
    let mut edges = Vec::with_capacity(n + 1);

    nodes.push(make_node("start", NodeType::Start));

    let mut prev_id = "start".to_string();
    for i in 0..n {
        let id = format!("box_{i}");
        nodes.push(make_node(&id, NodeType::Generic));
        edges.push(make_edge(&prev_id, &id));
        prev_id = id;
    }

    nodes.push(make_node("exit", NodeType::Exit));
    edges.push(make_edge(&prev_id, "exit"));

    Graph {
        name: Some("linear_pipeline".to_string()),
        nodes,
        edges,
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

/// Create a branching pipeline: start -> diamond -> exit_a / exit_b.
///
/// The diamond node is a Conditional. The edge to exit_a has condition
/// `"result=true"` and the edge to exit_b has condition `"result=false"`.
/// Both exit_a and exit_b are Exit nodes.
pub fn branching_pipeline() -> Graph {
    let nodes = vec![
        make_node("start", NodeType::Start),
        make_node("diamond", NodeType::Conditional),
        make_node("exit_a", NodeType::Exit),
        make_node("exit_b", NodeType::Exit),
    ];

    let mut edge_to_a = make_conditional_edge("diamond", "exit_a", "result=true");
    edge_to_a.label = Some("true".to_string());
    edge_to_a.priority = Some(1);

    let mut edge_to_b = make_conditional_edge("diamond", "exit_b", "result=false");
    edge_to_b.label = Some("false".to_string());
    edge_to_b.priority = Some(2);

    let edges = vec![make_edge("start", "diamond"), edge_to_a, edge_to_b];

    Graph {
        name: Some("branching_pipeline".to_string()),
        nodes,
        edges,
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

/// Create a looping pipeline: start -> worker -> decision (loop back or exit).
///
/// The decision node is a Conditional. One edge goes to exit with condition
/// `"loop_done=true"` (higher priority). The other is a loop_restart edge
/// back to worker (fallback).
///
/// `max_loops` is stored in the graph name for documentation but does not
/// enforce a limit -- that is the handler's responsibility. The graph itself
/// supports unbounded looping; max_steps on the engine prevents runaways.
pub fn looping_pipeline(max_loops: usize) -> Graph {
    let nodes = vec![
        make_node("start", NodeType::Start),
        make_node("worker", NodeType::Generic),
        make_node("decision", NodeType::Conditional),
        make_node("exit", NodeType::Exit),
    ];

    let mut exit_edge = make_conditional_edge("decision", "exit", "loop_done=true");
    exit_edge.priority = Some(10);

    let edges = vec![
        make_edge("start", "worker"),
        make_edge("worker", "decision"),
        exit_edge,
        make_loop_restart_edge("decision", "worker"),
    ];

    Graph {
        name: Some(format!("looping_pipeline_max_{max_loops}")),
        nodes,
        edges,
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

/// Create a parallel fan-out pipeline: start -> parallel -> n branches -> exit.
///
/// The parallel node is of type Parallel. Each branch is a Generic node named
/// `branch_0` through `branch_(n-1)`. All branches converge to a single exit node.
///
/// `branches` must be >= 1.
pub fn parallel_pipeline(branches: usize) -> Graph {
    assert!(
        branches >= 1,
        "parallel_pipeline requires at least 1 branch"
    );

    let mut nodes = Vec::with_capacity(branches + 3);
    let mut edges = Vec::with_capacity(branches * 2 + 1);

    nodes.push(make_node("start", NodeType::Start));
    nodes.push(make_node("parallel", NodeType::Parallel));
    edges.push(make_edge("start", "parallel"));

    for i in 0..branches {
        let branch_id = format!("branch_{i}");
        nodes.push(make_node(&branch_id, NodeType::Generic));
        edges.push(make_edge("parallel", &branch_id));
        edges.push(make_edge(&branch_id, "exit"));
    }

    nodes.push(make_node("exit", NodeType::Exit));

    Graph {
        name: Some("parallel_pipeline".to_string()),
        nodes,
        edges,
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

/// Create a human gate pipeline: start -> manager(Manager) -> exit_approved / exit_rejected.
///
/// The manager node is of type Manager (shape: house). It routes to exit_approved
/// when the condition `approved=true` is met, otherwise to exit_rejected.
/// Both exit nodes are of type Exit.
pub fn human_gate_pipeline() -> Graph {
    let nodes = vec![
        make_node("start", NodeType::Start),
        make_node("manager", NodeType::Manager),
        make_node("exit_approved", NodeType::Exit),
        make_node("exit_rejected", NodeType::Exit),
    ];

    let mut edge_approved = make_conditional_edge("manager", "exit_approved", "approved=true");
    edge_approved.label = Some("approved".to_string());
    edge_approved.priority = Some(1);

    let mut edge_rejected = make_conditional_edge("manager", "exit_rejected", "approved=false");
    edge_rejected.label = Some("rejected".to_string());
    edge_rejected.priority = Some(2);

    let edges = vec![make_edge("start", "manager"), edge_approved, edge_rejected];

    Graph {
        name: Some("human_gate_pipeline".to_string()),
        nodes,
        edges,
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

/// Create a code generation pipeline: start -> plan(Codergen) -> generate(Codergen) -> review(Conditional) -> exit.
///
/// The plan and generate nodes are of type Codergen (shape: box). The review
/// node is a Conditional (shape: diamond) that decides whether the generated
/// code passes review. The edge from review to exit has condition `review_passed=true`.
pub fn codergen_pipeline() -> Graph {
    let nodes = vec![
        make_node("start", NodeType::Start),
        make_node("plan", NodeType::Codergen),
        make_node("generate", NodeType::Codergen),
        make_node("review", NodeType::Conditional),
        make_node("exit", NodeType::Exit),
    ];

    let mut review_exit_edge = make_conditional_edge("review", "exit", "review_passed=true");
    review_exit_edge.label = Some("passed".to_string());
    review_exit_edge.priority = Some(1);

    let edges = vec![
        make_edge("start", "plan"),
        make_edge("plan", "generate"),
        make_edge("generate", "review"),
        review_exit_edge,
    ];

    Graph {
        name: Some("codergen_pipeline".to_string()),
        nodes,
        edges,
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

/// Create a sub-pipeline parent graph: start -> sub(SubPipeline) -> exit.
///
/// The sub node is of type SubPipeline (shape: component), representing an
/// inline composition that references an external DOT file. This fixture
/// provides the parent graph structure for testing sub-pipeline integration.
pub fn sub_pipeline_parent() -> Graph {
    let nodes = vec![
        make_node("start", NodeType::Start),
        make_node("sub", NodeType::SubPipeline),
        make_node("exit", NodeType::Exit),
    ];

    let edges = vec![make_edge("start", "sub"), make_edge("sub", "exit")];

    Graph {
        name: Some("sub_pipeline_parent".to_string()),
        nodes,
        edges,
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

/// Create a complex pipeline with conditional branching into two distinct paths.
///
/// Structure:
/// - start -> conditional(Conditional)
/// - Branch A (condition `path=a`): step_a1(Codergen) -> step_a2(Codergen) -> exit_a(Exit)
/// - Branch B (condition `path=b`): manager_b(Manager) -> exit_b(Exit)
///
/// This exercises mixed node types and multi-step branches with separate exits.
pub fn complex_pipeline() -> Graph {
    let nodes = vec![
        make_node("start", NodeType::Start),
        make_node("conditional", NodeType::Conditional),
        make_node("step_a1", NodeType::Codergen),
        make_node("step_a2", NodeType::Codergen),
        make_node("exit_a", NodeType::Exit),
        make_node("manager_b", NodeType::Manager),
        make_node("exit_b", NodeType::Exit),
    ];

    let mut edge_branch_a = make_conditional_edge("conditional", "step_a1", "path=a");
    edge_branch_a.label = Some("branch_a".to_string());
    edge_branch_a.priority = Some(1);

    let mut edge_branch_b = make_conditional_edge("conditional", "manager_b", "path=b");
    edge_branch_b.label = Some("branch_b".to_string());
    edge_branch_b.priority = Some(2);

    let edges = vec![
        make_edge("start", "conditional"),
        edge_branch_a,
        make_edge("step_a1", "step_a2"),
        make_edge("step_a2", "exit_a"),
        edge_branch_b,
        make_edge("manager_b", "exit_b"),
    ];

    Graph {
        name: Some("complex_pipeline".to_string()),
        nodes,
        edges,
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Context helpers
// ---------------------------------------------------------------------------

/// Build a Context pre-populated with the given key-value pairs.
///
/// Values are stored as JSON strings. For richer types, construct
/// the Context manually.
pub fn make_test_context(vars: &[(&str, &str)]) -> Context {
    let ctx = Context::new();
    for (key, value) in vars {
        ctx.set(*key, json!(*value));
    }
    ctx
}

// ---------------------------------------------------------------------------
// Test handlers
// ---------------------------------------------------------------------------

/// A test handler that counts how many times it is invoked.
///
/// Returns success for every node type. The counter is shared via
/// `Arc<AtomicUsize>` so callers can inspect the invocation count
/// after execution.
pub struct CountingHandler {
    handler_name: String,
    counter: Arc<AtomicUsize>,
}

impl CountingHandler {
    fn new(name: String, counter: Arc<AtomicUsize>) -> Self {
        Self {
            handler_name: name,
            counter,
        }
    }
}

#[async_trait]
impl Handler for CountingHandler {
    fn name(&self) -> &str {
        &self.handler_name
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        self.counter.fetch_add(1, Ordering::SeqCst);
        context.set(format!("visited_{}", node.id), json!(true));
        Ok(Outcome::success_with(json!({ "node": node.id })))
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Create a counting handler and its associated counter.
///
/// The handler succeeds for all node types, stamps `visited_{node_id}` in context,
/// and increments the shared counter on each invocation.
///
/// Returns the handler (as an `Arc<dyn Handler>`) and the counter for inspection.
pub fn make_counting_handler(name: &str) -> (Arc<dyn Handler>, Arc<AtomicUsize>) {
    let counter = Arc::new(AtomicUsize::new(0));
    let handler = Arc::new(CountingHandler::new(name.to_string(), counter.clone()));
    (handler as Arc<dyn Handler>, counter)
}

/// A test handler that fails with HandlerError when executing a specific node.
///
/// For all other nodes, it succeeds. This is useful for testing error propagation
/// and handler failure behavior within a pipeline.
struct FailingHandler {
    target_node_id: String,
}

#[async_trait]
impl Handler for FailingHandler {
    fn name(&self) -> &str {
        "failing_handler"
    }

    async fn execute(&self, node: &GraphNode, _context: &Context) -> Result<Outcome, HandlerError> {
        if node.id == self.target_node_id {
            Err(HandlerError::ExecutionFailed {
                handler: "failing_handler".to_string(),
                node_id: node.id.clone(),
                message: format!("deliberate failure on node '{}'", node.id),
            })
        } else {
            Ok(Outcome::success())
        }
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Create a handler that fails with `HandlerError::ExecutionFailed` when
/// executing the node with the given `node_id`, and succeeds for all others.
///
/// Handles all node types.
pub fn make_failing_handler(node_id: &str) -> Arc<dyn Handler> {
    Arc::new(FailingHandler {
        target_node_id: node_id.to_string(),
    })
}

/// A test handler that always returns the specified outcome regardless of node.
///
/// Handles all node types, making it a universal fixture for testing specific
/// outcome-driven behaviors such as failure routing or skip handling.
struct OutcomeHandler {
    outcome: Outcome,
}

#[async_trait]
impl Handler for OutcomeHandler {
    fn name(&self) -> &str {
        "outcome_handler"
    }

    async fn execute(
        &self,
        _node: &GraphNode,
        _context: &Context,
    ) -> Result<Outcome, HandlerError> {
        Ok(self.outcome.clone())
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Create a handler that always returns the specified `Outcome` for every node.
///
/// Handles all node types.
pub fn make_outcome_handler(outcome: Outcome) -> Arc<dyn Handler> {
    Arc::new(OutcomeHandler { outcome })
}

/// A test handler that sleeps for a configurable duration before succeeding.
///
/// Handles all node types. The delay uses `tokio::time::sleep` and requires
/// a tokio runtime. Stamps `visited_{node_id}` in context after the delay.
struct DelayedHandler {
    delay: std::time::Duration,
}

#[async_trait]
impl Handler for DelayedHandler {
    fn name(&self) -> &str {
        "delayed_handler"
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        tokio::time::sleep(self.delay).await;
        context.set(format!("visited_{}", node.id), json!(true));
        Ok(Outcome::success_with(
            json!({ "node": node.id, "delayed_ms": self.delay.as_millis() as u64 }),
        ))
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Create a handler that sleeps for `delay_ms` milliseconds before succeeding.
///
/// Handles all node types. After the delay, stamps `visited_{node_id}` in the
/// context and returns success with data containing the node id and delay.
pub fn make_delayed_handler(delay_ms: u64) -> Arc<dyn Handler> {
    Arc::new(DelayedHandler {
        delay: std::time::Duration::from_millis(delay_ms),
    })
}
