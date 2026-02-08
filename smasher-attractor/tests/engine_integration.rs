// ABOUTME: Integration tests for the pipeline execution engine.
// ABOUTME: Exercises multi-node traversal, checkpointing, handlers, retries, and error cases.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::json;

use smasher_attractor::engine::{Engine, EngineConfig, EngineError};
use smasher_attractor::graph::{Graph, GraphEdge, GraphNode, NodeAttrValue, NodeType};
use smasher_attractor::handler::{Handler, HandlerError, HandlerRegistry};
use smasher_attractor::state::{Checkpoint, Context, Outcome};

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

fn make_labeled_edge(from: &str, to: &str, label: &str) -> GraphEdge {
    GraphEdge {
        from: from.to_string(),
        to: to.to_string(),
        label: Some(label.to_string()),
        condition: Some(label.to_string()),
        priority: None,
        loop_restart: false,
        attrs: HashMap::new(),
    }
}

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

fn make_graph(nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> Graph {
    Graph {
        name: Some("integration_test".to_string()),
        nodes,
        edges,
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Test handlers
// ---------------------------------------------------------------------------

/// Handler that stamps the context with `visited_{node_id}=true` and a `data_{node_id}` payload.
/// Tracks invocation count via an atomic counter.
struct StampingHandler {
    invocations: Arc<AtomicUsize>,
}

impl StampingHandler {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        Self {
            invocations: counter,
        }
    }
}

#[async_trait]
impl Handler for StampingHandler {
    fn name(&self) -> &str {
        "stamping"
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        context.set(format!("visited_{}", node.id), json!(true));
        context.set(
            format!("data_{}", node.id),
            json!(format!("payload_from_{}", node.id)),
        );
        Ok(Outcome::success_with(json!({"node": node.id})))
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Handler that returns a HandlerError on execution, simulating a catastrophic failure.
struct ErrorOnNodeHandler {
    target_node_id: String,
}

impl ErrorOnNodeHandler {
    fn new(target_node_id: &str) -> Self {
        Self {
            target_node_id: target_node_id.to_string(),
        }
    }
}

#[async_trait]
impl Handler for ErrorOnNodeHandler {
    fn name(&self) -> &str {
        "error_on_node"
    }

    async fn execute(&self, node: &GraphNode, _context: &Context) -> Result<Outcome, HandlerError> {
        if node.id == self.target_node_id {
            Err(HandlerError::ExecutionFailed {
                handler: "error_on_node".to_string(),
                node_id: node.id.clone(),
                message: "deliberate test failure".to_string(),
            })
        } else {
            Ok(Outcome::success())
        }
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Handler that returns success for all node types. No side effects.
struct PassthroughHandler;

#[async_trait]
impl Handler for PassthroughHandler {
    fn name(&self) -> &str {
        "passthrough"
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

/// Handler that returns a retryable failure for the first N invocations,
/// then succeeds thereafter. Uses an atomic counter for thread safety.
struct FailThenSucceedHandler {
    fail_count: usize,
    attempts: Arc<AtomicUsize>,
}

impl FailThenSucceedHandler {
    fn new(fail_count: usize, attempts: Arc<AtomicUsize>) -> Self {
        Self {
            fail_count,
            attempts,
        }
    }
}

#[async_trait]
impl Handler for FailThenSucceedHandler {
    fn name(&self) -> &str {
        "fail_then_succeed"
    }

    async fn execute(
        &self,
        _node: &GraphNode,
        _context: &Context,
    ) -> Result<Outcome, HandlerError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        if attempt < self.fail_count {
            Ok(Outcome::retryable_failure(format!(
                "transient failure attempt {attempt}"
            )))
        } else {
            Ok(Outcome::success())
        }
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Handler that always returns a retryable failure, used to test retry exhaustion.
struct AlwaysRetryableFailHandler {
    attempts: Arc<AtomicUsize>,
}

impl AlwaysRetryableFailHandler {
    fn new(attempts: Arc<AtomicUsize>) -> Self {
        Self { attempts }
    }
}

#[async_trait]
impl Handler for AlwaysRetryableFailHandler {
    fn name(&self) -> &str {
        "always_retryable_fail"
    }

    async fn execute(
        &self,
        _node: &GraphNode,
        _context: &Context,
    ) -> Result<Outcome, HandlerError> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Ok(Outcome::retryable_failure("persistent transient error"))
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Handler for conditional branching tests that evaluates a context variable
/// and returns success (with result data) or failure depending on the outcome.
struct BranchEvalHandler;

#[async_trait]
impl Handler for BranchEvalHandler {
    fn name(&self) -> &str {
        "branch_eval"
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        // For Conditional nodes, check a context variable to decide outcome
        if node.node_type == NodeType::Conditional {
            let decision = context.get_string("decision").unwrap_or_default();
            if decision == "yes" {
                Ok(Outcome::success_with(json!({"result": true})))
            } else {
                Ok(Outcome::failure("decision was not yes"))
            }
        } else {
            // All other node types just succeed and stamp context
            context.set(format!("visited_{}", node.id), json!(true));
            Ok(Outcome::success())
        }
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Handler that controls loop iteration: sets `loop_done=true` after a target number of
/// passes through a specific node, enabling conditional exit from a loop.
struct LoopControlHandler {
    trigger_node: String,
    max_passes: usize,
    pass_counter: Arc<AtomicUsize>,
}

impl LoopControlHandler {
    fn new(trigger_node: &str, max_passes: usize, counter: Arc<AtomicUsize>) -> Self {
        Self {
            trigger_node: trigger_node.to_string(),
            max_passes,
            pass_counter: counter,
        }
    }
}

#[async_trait]
impl Handler for LoopControlHandler {
    fn name(&self) -> &str {
        "loop_control"
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        context.set(format!("visited_{}", node.id), json!(true));

        if node.id == self.trigger_node {
            let pass = self.pass_counter.fetch_add(1, Ordering::SeqCst);
            // Set node-prefixed data to verify loop_restart clearing
            context.set(format!("{}_data", node.id), json!(format!("pass_{}", pass)));

            if pass + 1 >= self.max_passes {
                context.set("loop_done", json!("true"));
            }
        }

        Ok(Outcome::success())
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

fn passthrough_registry() -> HandlerRegistry {
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(PassthroughHandler));
    registry
}

fn stamping_registry(counter: Arc<AtomicUsize>) -> HandlerRegistry {
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(StampingHandler::new(counter)));
    registry
}

// ============================================================================
// 1. Linear pipeline tests
// ============================================================================

#[tokio::test]
async fn linear_start_box_exit_traversal() {
    let counter = Arc::new(AtomicUsize::new(0));
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("process", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "process"), make_edge("process", "exit")],
    );
    let engine = Engine::new(graph, stamping_registry(counter.clone()));
    let ctx = Context::new();
    let result = engine.run(ctx).await.unwrap();

    assert_eq!(result.visited_nodes, vec!["start", "process", "exit"]);
    assert_eq!(result.steps_taken, 3);
    assert_eq!(counter.load(Ordering::SeqCst), 3);

    // Verify all context stamps are present
    assert_eq!(
        result.final_context.get("visited_start"),
        Some(&json!(true))
    );
    assert_eq!(
        result.final_context.get("visited_process"),
        Some(&json!(true))
    );
    assert_eq!(result.final_context.get("visited_exit"), Some(&json!(true)));

    // Verify data payloads from each node
    assert!(result.final_context.contains_key("data_start"));
    assert!(result.final_context.contains_key("data_process"));
    assert!(result.final_context.contains_key("data_exit"));
}

#[tokio::test]
async fn linear_multi_node_traversal() {
    let counter = Arc::new(AtomicUsize::new(0));
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("step1", NodeType::Generic),
            make_node("step2", NodeType::Generic),
            make_node("step3", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "step1"),
            make_edge("step1", "step2"),
            make_edge("step2", "step3"),
            make_edge("step3", "exit"),
        ],
    );
    let engine = Engine::new(graph, stamping_registry(counter.clone()));
    let ctx = Context::new();
    let result = engine.run(ctx).await.unwrap();

    assert_eq!(
        result.visited_nodes,
        vec!["start", "step1", "step2", "step3", "exit"]
    );
    assert_eq!(result.steps_taken, 5);
    assert_eq!(counter.load(Ordering::SeqCst), 5);

    // All 5 nodes should have stamped the context
    for node_id in &["start", "step1", "step2", "step3", "exit"] {
        assert_eq!(
            result.final_context.get(&format!("visited_{node_id}")),
            Some(&json!(true)),
            "missing visited stamp for {node_id}"
        );
        assert!(
            result
                .final_context
                .contains_key(&format!("data_{node_id}")),
            "missing data payload for {node_id}"
        );
    }
}

#[tokio::test]
async fn linear_pipeline_with_initial_context_preserved() {
    let counter = Arc::new(AtomicUsize::new(0));
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("worker", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "worker"), make_edge("worker", "exit")],
    );
    let engine = Engine::new(graph, stamping_registry(counter));
    let ctx = Context::new();
    ctx.set("initial_key", json!("initial_value"));
    ctx.set("pipeline_id", json!(42));

    let result = engine.run(ctx).await.unwrap();

    // Initial context values should survive through execution
    assert_eq!(
        result.final_context.get("initial_key"),
        Some(&json!("initial_value"))
    );
    assert_eq!(result.final_context.get("pipeline_id"), Some(&json!(42)));

    // Handler-set values should also be present
    assert_eq!(
        result.final_context.get("visited_worker"),
        Some(&json!(true))
    );
}

// ============================================================================
// 2. Conditional branching tests
// ============================================================================

#[tokio::test]
async fn conditional_branch_takes_yes_path() {
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("check", NodeType::Conditional),
            make_node("yes_path", NodeType::Generic),
            make_node("no_path", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "check"),
            make_labeled_edge("check", "yes_path", "success"),
            make_labeled_edge("check", "no_path", "failure"),
            make_edge("yes_path", "exit"),
            make_edge("no_path", "exit"),
        ],
    );
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(BranchEvalHandler));
    let engine = Engine::new(graph, registry);

    let ctx = Context::new();
    ctx.set("decision", json!("yes"));
    let result = engine.run(ctx).await.unwrap();

    assert!(result.visited_nodes.contains(&"yes_path".to_string()));
    assert!(!result.visited_nodes.contains(&"no_path".to_string()));
    assert_eq!(
        result.final_context.get("visited_yes_path"),
        Some(&json!(true))
    );
}

#[tokio::test]
async fn conditional_branch_takes_no_path() {
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("check", NodeType::Conditional),
            make_node("yes_path", NodeType::Generic),
            make_node("no_path", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "check"),
            make_labeled_edge("check", "yes_path", "success"),
            make_labeled_edge("check", "no_path", "failure"),
            make_edge("yes_path", "exit"),
            make_edge("no_path", "exit"),
        ],
    );
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(BranchEvalHandler));
    let engine = Engine::new(graph, registry);

    let ctx = Context::new();
    ctx.set("decision", json!("no"));
    let result = engine.run(ctx).await.unwrap();

    assert!(result.visited_nodes.contains(&"no_path".to_string()));
    assert!(!result.visited_nodes.contains(&"yes_path".to_string()));
    assert_eq!(
        result.final_context.get("visited_no_path"),
        Some(&json!(true))
    );
}

#[tokio::test]
async fn conditional_edge_with_context_variable() {
    // Uses explicit condition attributes on edges instead of outcome-based routing
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("router", NodeType::Generic),
            make_node("fast_path", NodeType::Generic),
            make_node("slow_path", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "router"),
            make_conditional_edge("router", "fast_path", "mode=fast"),
            make_conditional_edge("router", "slow_path", "mode=slow"),
            make_edge("fast_path", "exit"),
            make_edge("slow_path", "exit"),
        ],
    );
    let engine = Engine::new(graph, passthrough_registry());

    // Test fast path
    let ctx = Context::new();
    ctx.set("mode", json!("fast"));
    let result = engine.run(ctx).await.unwrap();
    assert!(result.visited_nodes.contains(&"fast_path".to_string()));
    assert!(!result.visited_nodes.contains(&"slow_path".to_string()));

    // Test slow path
    let engine2 = Engine::new(
        make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("router", NodeType::Generic),
                make_node("fast_path", NodeType::Generic),
                make_node("slow_path", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "router"),
                make_conditional_edge("router", "fast_path", "mode=fast"),
                make_conditional_edge("router", "slow_path", "mode=slow"),
                make_edge("fast_path", "exit"),
                make_edge("slow_path", "exit"),
            ],
        ),
        passthrough_registry(),
    );
    let ctx2 = Context::new();
    ctx2.set("mode", json!("slow"));
    let result2 = engine2.run(ctx2).await.unwrap();
    assert!(result2.visited_nodes.contains(&"slow_path".to_string()));
    assert!(!result2.visited_nodes.contains(&"fast_path".to_string()));
}

// ============================================================================
// 3. Error handling tests
// ============================================================================

#[tokio::test]
async fn error_no_start_node() {
    let graph = make_graph(
        vec![
            make_node("a", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("a", "exit")],
    );
    let engine = Engine::new(graph, passthrough_registry());
    let err = engine.run(Context::new()).await.unwrap_err();

    assert!(matches!(err, EngineError::NoStartNode));
    assert!(err.to_string().contains("no start node"));
}

#[tokio::test]
async fn error_multiple_start_nodes() {
    let graph = make_graph(
        vec![
            make_node("start1", NodeType::Start),
            make_node("start2", NodeType::Start),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start1", "exit"), make_edge("start2", "exit")],
    );
    let engine = Engine::new(graph, passthrough_registry());
    let err = engine.run(Context::new()).await.unwrap_err();

    match err {
        EngineError::MultipleStartNodes { ids } => {
            assert_eq!(ids.len(), 2);
            assert!(ids.contains(&"start1".to_string()));
            assert!(ids.contains(&"start2".to_string()));
        }
        other => panic!("expected MultipleStartNodes, got: {other:?}"),
    }
}

#[tokio::test]
async fn error_max_steps_exceeded_on_loop() {
    // Create a tight cycle: start -> a -> b -> a
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("a", NodeType::Generic),
            make_node("b", NodeType::Generic),
        ],
        vec![
            make_edge("start", "a"),
            make_edge("a", "b"),
            make_edge("b", "a"),
        ],
    );
    let config = EngineConfig {
        max_steps: 7,
        enable_checkpointing: false,
    };
    let engine = Engine::with_config(graph, passthrough_registry(), config);
    let err = engine.run(Context::new()).await.unwrap_err();

    match err {
        EngineError::MaxStepsExceeded { max_steps } => {
            assert_eq!(max_steps, 7);
        }
        other => panic!("expected MaxStepsExceeded, got: {other:?}"),
    }
}

#[tokio::test]
async fn error_handler_returns_handler_error() {
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("bad_node", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "bad_node"),
            make_edge("bad_node", "exit"),
        ],
    );
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(ErrorOnNodeHandler::new("bad_node")));
    let engine = Engine::new(graph, registry);
    let err = engine.run(Context::new()).await.unwrap_err();

    assert!(matches!(err, EngineError::Handler(_)));
    assert!(err.to_string().contains("deliberate test failure"));
}

#[tokio::test]
async fn error_handler_on_start_node() {
    // The very first node execution fails with a HandlerError
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "exit")],
    );
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(ErrorOnNodeHandler::new("start")));
    let engine = Engine::new(graph, registry);
    let err = engine.run(Context::new()).await.unwrap_err();

    assert!(matches!(err, EngineError::Handler(_)));
}

// ============================================================================
// 4. Checkpointing tests
// ============================================================================

#[tokio::test]
async fn checkpoint_created_with_correct_state() {
    let counter = Arc::new(AtomicUsize::new(0));
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("middle", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "middle"), make_edge("middle", "exit")],
    );
    let config = EngineConfig {
        max_steps: 1000,
        enable_checkpointing: true,
    };
    let engine = Engine::with_config(graph, stamping_registry(counter), config);
    let ctx = Context::new();
    ctx.set("input", json!("test_data"));
    let result = engine.run(ctx).await.unwrap();

    assert!(result.checkpoint.is_some());
    let cp = result.checkpoint.unwrap();

    // Pipeline name should match the graph name
    assert_eq!(cp.pipeline_name, "integration_test");

    // All visited nodes should be recorded in the checkpoint
    assert!(cp.was_visited("start"));
    assert!(cp.was_visited("middle"));
    assert!(cp.was_visited("exit"));

    // Node outcomes should be recorded
    assert!(cp.node_outcomes.get("start").unwrap().is_success());
    assert!(cp.node_outcomes.get("middle").unwrap().is_success());
    assert!(cp.node_outcomes.get("exit").unwrap().is_success());

    // Context snapshot should include both initial and handler-set values
    assert_eq!(cp.context_snapshot.get("input"), Some(&json!("test_data")));
    assert_eq!(
        cp.context_snapshot.get("visited_middle"),
        Some(&json!(true))
    );
}

#[tokio::test]
async fn checkpoint_disabled_produces_none() {
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "exit")],
    );
    let config = EngineConfig {
        max_steps: 1000,
        enable_checkpointing: false,
    };
    let engine = Engine::with_config(graph, passthrough_registry(), config);
    let result = engine.run(Context::new()).await.unwrap();

    assert!(result.checkpoint.is_none());
}

#[tokio::test]
async fn checkpoint_serialization_roundtrip() {
    let counter = Arc::new(AtomicUsize::new(0));
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("worker", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "worker"), make_edge("worker", "exit")],
    );
    let config = EngineConfig {
        max_steps: 1000,
        enable_checkpointing: true,
    };
    let engine = Engine::with_config(graph, stamping_registry(counter), config);
    let ctx = Context::new();
    ctx.set("run_id", json!("test-run-42"));
    let result = engine.run(ctx).await.unwrap();

    let cp = result.checkpoint.unwrap();
    let json_str = cp.to_json().unwrap();
    let restored = Checkpoint::from_json(&json_str).unwrap();

    assert_eq!(restored.pipeline_name, cp.pipeline_name);
    assert_eq!(restored.visited_nodes, cp.visited_nodes);
    assert_eq!(
        restored.context_snapshot.get("run_id"),
        Some(&json!("test-run-42"))
    );
    assert!(restored.was_visited("start"));
    assert!(restored.was_visited("worker"));
    assert!(restored.was_visited("exit"));
}

#[tokio::test]
async fn resume_from_checkpoint_continues_execution() {
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("a", NodeType::Generic),
            make_node("b", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "a"),
            make_edge("a", "b"),
            make_edge("b", "exit"),
        ],
    );
    let engine = Engine::new(graph, passthrough_registry());

    // Create a checkpoint as if we already visited start and a, resuming at b
    let cp_ctx = Context::new();
    let mut checkpoint = Checkpoint::new("integration_test", "b", &cp_ctx);
    checkpoint.mark_visited("start");
    checkpoint.mark_visited("a");
    checkpoint.add_outcome("start", Outcome::success());
    checkpoint.add_outcome("a", Outcome::success());

    let ctx = Context::new();
    let result = engine.run_from_checkpoint(checkpoint, ctx).await.unwrap();

    // Should have all nodes visited (start/a from checkpoint, b/exit from resumed execution)
    assert!(result.visited_nodes.contains(&"start".to_string()));
    assert!(result.visited_nodes.contains(&"a".to_string()));
    assert!(result.visited_nodes.contains(&"b".to_string()));
    assert!(result.visited_nodes.contains(&"exit".to_string()));

    // Steps taken should only count the resumed portion (b and exit)
    assert_eq!(result.steps_taken, 2);
}

// ============================================================================
// 5. Goal gate tests
// ============================================================================

#[tokio::test]
async fn goal_gate_passes_when_all_goals_visited() {
    let mut goal_attrs = HashMap::new();
    goal_attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));

    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node_with_attrs("critical_task", NodeType::Generic, goal_attrs),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "critical_task"),
            make_edge("critical_task", "exit"),
        ],
    );
    let engine = Engine::new(graph, passthrough_registry());
    let result = engine.run(Context::new()).await;

    assert!(result.is_ok());
    let result = result.unwrap();
    assert!(result.visited_nodes.contains(&"critical_task".to_string()));
}

#[tokio::test]
async fn goal_gate_fails_when_goal_not_visited() {
    let mut goal_attrs = HashMap::new();
    goal_attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));

    // Goal node is unreachable from the execution path
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("exit", NodeType::Exit),
            make_node_with_attrs("unreachable_goal", NodeType::Generic, goal_attrs),
        ],
        vec![make_edge("start", "exit")],
    );
    let engine = Engine::new(graph, passthrough_registry());
    let err = engine.run(Context::new()).await.unwrap_err();

    assert!(matches!(err, EngineError::GoalEnforcement(_)));
    assert!(err.to_string().contains("unreachable_goal"));
}

#[tokio::test]
async fn goal_gate_with_multiple_goals_all_met() {
    let mut g1_attrs = HashMap::new();
    g1_attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));
    let mut g2_attrs = HashMap::new();
    g2_attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));

    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node_with_attrs("goal_1", NodeType::Generic, g1_attrs),
            make_node_with_attrs("goal_2", NodeType::Generic, g2_attrs),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "goal_1"),
            make_edge("goal_1", "goal_2"),
            make_edge("goal_2", "exit"),
        ],
    );
    let engine = Engine::new(graph, passthrough_registry());
    let result = engine.run(Context::new()).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn goal_gate_with_multiple_goals_some_unmet() {
    let mut g1_attrs = HashMap::new();
    g1_attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));
    let mut g2_attrs = HashMap::new();
    g2_attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));

    // Only goal_1 is reachable; goal_2 is not connected
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node_with_attrs("goal_1", NodeType::Generic, g1_attrs),
            make_node("exit", NodeType::Exit),
            make_node_with_attrs("goal_2", NodeType::Generic, g2_attrs),
        ],
        vec![make_edge("start", "goal_1"), make_edge("goal_1", "exit")],
    );
    let engine = Engine::new(graph, passthrough_registry());
    let err = engine.run(Context::new()).await.unwrap_err();

    assert!(matches!(err, EngineError::GoalEnforcement(_)));
    assert!(err.to_string().contains("goal_2"));
}

// ============================================================================
// 6. Loop restart tests
// ============================================================================

#[tokio::test]
async fn loop_restart_tracks_counter() {
    // Pipeline: start -> worker -> router -> exit (conditional) or -> worker (loop_restart)
    // After 2 passes through router, loop_done is set, exiting to exit.
    let pass_counter = Arc::new(AtomicUsize::new(0));

    let mut exit_edge = make_conditional_edge("router", "exit", "loop_done=true");
    exit_edge.priority = Some(10);

    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("worker", NodeType::Generic),
            make_node("router", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "worker"),
            make_edge("worker", "router"),
            exit_edge,
            make_loop_restart_edge("router", "worker"),
        ],
    );

    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(LoopControlHandler::new(
        "router",
        2,
        pass_counter.clone(),
    )));
    let config = EngineConfig {
        max_steps: 20,
        enable_checkpointing: false,
    };
    let engine = Engine::with_config(graph, registry, config);

    let result = engine.run(Context::new()).await.unwrap();

    // Should have visited: start, worker, router, (loop back), worker, router, exit
    assert!(result.visited_nodes.contains(&"exit".to_string()));
    assert_eq!(result.loop_restarts.count("router", "worker"), 1);
    assert_eq!(result.loop_restarts.total(), 1);
}

#[tokio::test]
async fn loop_restart_clears_prefixed_context_entries() {
    let pass_counter = Arc::new(AtomicUsize::new(0));

    let mut exit_edge = make_conditional_edge("router", "exit", "loop_done=true");
    exit_edge.priority = Some(10);

    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("worker", NodeType::Generic),
            make_node("router", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "worker"),
            make_edge("worker", "router"),
            exit_edge,
            make_loop_restart_edge("router", "worker"),
        ],
    );

    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(LoopControlHandler::new("router", 2, pass_counter)));
    let config = EngineConfig {
        max_steps: 20,
        enable_checkpointing: false,
    };
    let engine = Engine::with_config(graph, registry, config);

    let ctx = Context::new();
    // Set a router-prefixed key that should be cleared by loop_restart
    ctx.set("router_initial_key", json!("should_be_cleared"));

    let result = engine.run(ctx).await.unwrap();

    // "router_initial_key" was prefixed with "router_" so the loop_restart
    // edge from router -> worker should have cleared it
    assert!(
        !result.final_context.contains_key("router_initial_key"),
        "router-prefixed key should have been cleared by loop_restart"
    );

    // Non-prefixed context keys should still be present
    assert!(result.final_context.contains_key("loop_done"));
}

#[tokio::test]
async fn loop_restart_max_steps_prevents_infinite_loop() {
    // A tight loop with no exit condition, relying on max_steps to terminate
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("a", NodeType::Generic),
            make_node("b", NodeType::Generic),
        ],
        vec![
            make_edge("start", "a"),
            make_edge("a", "b"),
            make_loop_restart_edge("b", "a"),
        ],
    );
    let config = EngineConfig {
        max_steps: 10,
        enable_checkpointing: false,
    };
    let engine = Engine::with_config(graph, passthrough_registry(), config);
    let err = engine.run(Context::new()).await.unwrap_err();

    match err {
        EngineError::MaxStepsExceeded { max_steps } => {
            assert_eq!(max_steps, 10);
        }
        other => panic!("expected MaxStepsExceeded, got: {other:?}"),
    }
}

// ============================================================================
// 7. Retry tests
// ============================================================================

#[tokio::test]
async fn retry_handler_fails_once_then_succeeds() {
    use std::time::Duration;

    let attempts = Arc::new(AtomicUsize::new(0));
    let mut retry_attrs = HashMap::new();
    // Configure 3 retries (4 total attempts), no jitter, small delay
    retry_attrs.insert("retries".to_string(), NodeAttrValue::Number(3.0));
    retry_attrs.insert(
        "retry_delay".to_string(),
        NodeAttrValue::Duration(Duration::from_millis(10)),
    );
    retry_attrs.insert("retry_jitter".to_string(), NodeAttrValue::Bool(false));

    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node_with_attrs("flaky", NodeType::Generic, retry_attrs),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "flaky"), make_edge("flaky", "exit")],
    );
    let mut registry = HandlerRegistry::new();
    // Fail once (the initial attempt), then succeed on retry
    registry.register(Arc::new(FailThenSucceedHandler::new(1, attempts.clone())));
    let engine = Engine::new(graph, registry);

    let result = engine.run(Context::new()).await.unwrap();

    // Pipeline should complete successfully after retry
    assert!(result.visited_nodes.contains(&"exit".to_string()));

    // The flaky node should have been called twice (one failure + one success)
    // But note: the start and exit nodes also use this handler.
    // The engine calls the handler for start (attempt 0 -> success), then
    // for flaky: initial call (attempt 1 -> fail), retry (attempt 2 -> success),
    // then exit (attempt 3 -> success).
    // Total = 4 handler calls (start, flaky initial, flaky retry, exit)
    assert!(attempts.load(Ordering::SeqCst) >= 3);
}

#[tokio::test]
async fn retry_handler_exhausts_retries_outcome_recorded() {
    use std::time::Duration;

    let attempts = Arc::new(AtomicUsize::new(0));
    let mut retry_attrs = HashMap::new();
    // Configure 2 retries (3 total attempts), no jitter, small delay
    retry_attrs.insert("retries".to_string(), NodeAttrValue::Number(2.0));
    retry_attrs.insert(
        "retry_delay".to_string(),
        NodeAttrValue::Duration(Duration::from_millis(10)),
    );
    retry_attrs.insert("retry_jitter".to_string(), NodeAttrValue::Bool(false));

    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node_with_attrs("always_fails", NodeType::Generic, retry_attrs),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "always_fails"),
            make_edge("always_fails", "exit"),
        ],
    );
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(AlwaysRetryableFailHandler::new(attempts.clone())));
    let engine = Engine::new(graph, registry);

    let result = engine.run(Context::new()).await.unwrap();

    // The pipeline should still complete (failure outcome is recorded, execution continues)
    // The always_fails node has a retryable failure which gets retried, then recorded as failure
    // Edge selection sees the failure and tries to route accordingly
    let always_fails_outcome = result.node_outcomes.get("always_fails").unwrap();
    assert!(always_fails_outcome.is_failure());
}

// ============================================================================
// 8. Combined/integration scenario tests
// ============================================================================

#[tokio::test]
async fn full_pipeline_with_branch_and_goals() {
    // A realistic pipeline: start -> validate -> [check: if valid -> deploy, else -> fix] -> exit
    // deploy is a goal node.
    let mut goal_attrs = HashMap::new();
    goal_attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));

    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("validate", NodeType::Generic),
            make_node("check", NodeType::Conditional),
            make_node_with_attrs("deploy", NodeType::Generic, goal_attrs),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "validate"),
            make_edge("validate", "check"),
            make_labeled_edge("check", "deploy", "success"),
            make_edge("deploy", "exit"),
        ],
    );

    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(BranchEvalHandler));
    let engine = Engine::new(graph, registry);

    let ctx = Context::new();
    ctx.set("decision", json!("yes"));
    let result = engine.run(ctx).await.unwrap();

    assert!(result.visited_nodes.contains(&"deploy".to_string()));
    assert!(result.visited_nodes.contains(&"exit".to_string()));
    assert_eq!(
        result.final_context.get("visited_deploy"),
        Some(&json!(true))
    );
}

#[tokio::test]
async fn node_outcomes_record_all_executed_nodes() {
    let counter = Arc::new(AtomicUsize::new(0));
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("a", NodeType::Generic),
            make_node("b", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "a"),
            make_edge("a", "b"),
            make_edge("b", "exit"),
        ],
    );
    let engine = Engine::new(graph, stamping_registry(counter));
    let result = engine.run(Context::new()).await.unwrap();

    assert_eq!(result.node_outcomes.len(), 4);
    for node_id in &["start", "a", "b", "exit"] {
        let outcome = result.node_outcomes.get(*node_id);
        assert!(outcome.is_some(), "missing outcome for node {node_id}");
        assert!(
            outcome.unwrap().is_success(),
            "node {node_id} should have succeeded"
        );
    }
}

#[tokio::test]
async fn dead_end_node_terminates_execution() {
    // Node with no outgoing edges should terminate execution cleanly
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("dead_end", NodeType::Generic),
        ],
        vec![make_edge("start", "dead_end")],
    );
    let engine = Engine::new(graph, passthrough_registry());
    let result = engine.run(Context::new()).await.unwrap();

    assert_eq!(result.visited_nodes, vec!["start", "dead_end"]);
    assert_eq!(result.steps_taken, 2);
}

#[tokio::test]
async fn exit_node_stops_traversal_even_with_outgoing_edges() {
    // An exit node should stop execution even if it has outgoing edges
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("exit", NodeType::Exit),
            make_node("unreachable", NodeType::Generic),
        ],
        vec![make_edge("start", "exit"), make_edge("exit", "unreachable")],
    );
    let engine = Engine::new(graph, passthrough_registry());
    let result = engine.run(Context::new()).await.unwrap();

    assert_eq!(result.visited_nodes, vec!["start", "exit"]);
    assert!(!result.visited_nodes.contains(&"unreachable".to_string()));
}

#[tokio::test]
async fn multiple_handler_invocations_counted_correctly() {
    // Verify that handler invocation counts match the number of nodes executed
    let counter = Arc::new(AtomicUsize::new(0));
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("a", NodeType::Generic),
            make_node("b", NodeType::Generic),
            make_node("c", NodeType::Generic),
            make_node("d", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "a"),
            make_edge("a", "b"),
            make_edge("b", "c"),
            make_edge("c", "d"),
            make_edge("d", "exit"),
        ],
    );
    let engine = Engine::new(graph, stamping_registry(counter.clone()));
    let result = engine.run(Context::new()).await.unwrap();

    assert_eq!(result.steps_taken, 6);
    assert_eq!(counter.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn loop_restarts_with_non_loop_edges_have_zero_counter() {
    // A simple linear pipeline should have zero loop restarts
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("a", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "a"), make_edge("a", "exit")],
    );
    let engine = Engine::new(graph, passthrough_registry());
    let result = engine.run(Context::new()).await.unwrap();

    assert_eq!(result.loop_restarts.total(), 0);
    assert!(result.loop_restarts.counts().is_empty());
}
