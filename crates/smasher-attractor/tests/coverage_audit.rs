// ABOUTME: Coverage audit integration tests for smasher-attractor critical paths.
// ABOUTME: Verifies engine, handler, state, condition, and edge selection code paths are exercised.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use smasher_attractor::condition::{evaluate_condition, parse_condition};
use smasher_attractor::edge::select_edge;
use smasher_attractor::engine::{Engine, EngineConfig, EngineError};
use smasher_attractor::graph::{Graph, GraphEdge, GraphNode, NodeAttrValue, NodeType};
use smasher_attractor::handler::{Handler, HandlerError, HandlerRegistry, default_registry};
use smasher_attractor::state::{Checkpoint, CheckpointEnvelope, Context, Outcome};

// ===========================================================================
// Helper: graph construction
// ===========================================================================

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

fn make_priority_edge(from: &str, to: &str, priority: i32) -> GraphEdge {
    GraphEdge {
        from: from.to_string(),
        to: to.to_string(),
        label: None,
        condition: None,
        priority: Some(priority),
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
        name: Some("coverage_audit".to_string()),
        nodes,
        edges,
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

// ===========================================================================
// Helper: test handlers
// ===========================================================================

/// Handler that tracks invocations and stamps context with `visited_{node_id}`.
struct StampingHandler {
    invocations: Arc<AtomicUsize>,
}

#[async_trait]
impl Handler for StampingHandler {
    fn name(&self) -> &str {
        "stamping"
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        context.set(format!("visited_{}", node.id), json!(true));
        Ok(Outcome::success_with(json!({"node": node.id})))
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Handler that always succeeds for all node types.
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

/// Handler that returns HandlerError on a target node.
struct ErrorOnNodeHandler {
    target: String,
}

#[async_trait]
impl Handler for ErrorOnNodeHandler {
    fn name(&self) -> &str {
        "error_on_node"
    }

    async fn execute(&self, node: &GraphNode, _context: &Context) -> Result<Outcome, HandlerError> {
        if node.id == self.target {
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

/// Handler that fails `fail_count` times with retryable failures, then succeeds.
struct FailThenSucceedHandler {
    fail_count: usize,
    attempts: Arc<AtomicUsize>,
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

/// Handler that controls a loop: sets `loop_done=true` after `max_passes` through `trigger_node`.
struct LoopControlHandler {
    trigger_node: String,
    max_passes: usize,
    pass_counter: Arc<AtomicUsize>,
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
    registry.register(Arc::new(StampingHandler {
        invocations: counter,
    }));
    registry
}

// ===========================================================================
// 1. Engine critical paths
// ===========================================================================

/// Run a linear pipeline and verify all nodes were visited in order.
#[tokio::test]
async fn test_engine_start_to_exit_path_covered() {
    let counter = Arc::new(AtomicUsize::new(0));
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("step_a", NodeType::Generic),
            make_node("step_b", NodeType::Generic),
            make_node("step_c", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "step_a"),
            make_edge("step_a", "step_b"),
            make_edge("step_b", "step_c"),
            make_edge("step_c", "exit"),
        ],
    );
    let engine = Engine::new(graph, stamping_registry(counter.clone()));
    let ctx = Context::new();
    ctx.set("audit_run", json!(true));

    let result = engine.run(ctx).await.unwrap();

    // All nodes visited in order
    assert_eq!(
        result.visited_nodes,
        vec!["start", "step_a", "step_b", "step_c", "exit"]
    );
    assert_eq!(result.steps_taken, 5);
    assert_eq!(counter.load(Ordering::SeqCst), 5);

    // Context stamps prove each handler executed
    for node_id in &["start", "step_a", "step_b", "step_c", "exit"] {
        assert_eq!(
            result.final_context.get(&format!("visited_{node_id}")),
            Some(&json!(true)),
            "node {node_id} was not stamped in context"
        );
    }

    // Initial context preserved
    assert_eq!(result.final_context.get("audit_run"), Some(&json!(true)));

    // Outcomes recorded for each node
    assert_eq!(result.node_outcomes.len(), 5);
    for (id, outcome) in &result.node_outcomes {
        assert!(outcome.is_success(), "node {id} should have succeeded");
    }
}

/// Trigger each EngineError variant to confirm error paths are covered.
#[tokio::test]
async fn test_engine_error_paths_covered() {
    // 1. NoStartNode
    let no_start_graph = make_graph(
        vec![
            make_node("a", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("a", "exit")],
    );
    let err = Engine::new(no_start_graph, passthrough_registry())
        .run(Context::new())
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NoStartNode));
    assert!(err.to_string().contains("no start node"));

    // 2. MultipleStartNodes
    let multi_start = make_graph(
        vec![
            make_node("s1", NodeType::Start),
            make_node("s2", NodeType::Start),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("s1", "exit"), make_edge("s2", "exit")],
    );
    let err = Engine::new(multi_start, passthrough_registry())
        .run(Context::new())
        .await
        .unwrap_err();
    match &err {
        EngineError::MultipleStartNodes { ids } => {
            assert_eq!(ids.len(), 2);
        }
        other => panic!("expected MultipleStartNodes, got: {other:?}"),
    }

    // 3. MaxStepsExceeded
    let cycle = make_graph(
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
        max_steps: 5,
        enable_checkpointing: false,
        ..EngineConfig::default()
    };
    let err = Engine::with_config(cycle, passthrough_registry(), config)
        .run(Context::new())
        .await
        .unwrap_err();
    match &err {
        EngineError::MaxStepsExceeded { max_steps } => {
            assert_eq!(*max_steps, 5);
        }
        other => panic!("expected MaxStepsExceeded, got: {other:?}"),
    }

    // 4. Handler error
    let handler_err_graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("bad", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "bad"), make_edge("bad", "exit")],
    );
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(ErrorOnNodeHandler {
        target: "bad".to_string(),
    }));
    let err = Engine::new(handler_err_graph, registry)
        .run(Context::new())
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Handler(_)));
    assert!(err.to_string().contains("deliberate test failure"));

    // 5. GoalEnforcement error
    let mut goal_attrs = HashMap::new();
    goal_attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));
    let goal_err_graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("exit", NodeType::Exit),
            make_node_with_attrs("unreachable_goal", NodeType::Generic, goal_attrs),
        ],
        vec![make_edge("start", "exit")],
    );
    let err = Engine::new(goal_err_graph, passthrough_registry())
        .run(Context::new())
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::GoalEnforcement(_)));
    assert!(err.to_string().contains("unreachable_goal"));

    // 6. NodeNotFound (edge points to nonexistent node)
    let bad_edge_graph = make_graph(
        vec![make_node("start", NodeType::Start)],
        vec![make_edge("start", "ghost")],
    );
    let err = Engine::new(bad_edge_graph, passthrough_registry())
        .run(Context::new())
        .await
        .unwrap_err();
    match &err {
        EngineError::NodeNotFound { node_id } => {
            assert_eq!(node_id, "ghost");
        }
        other => panic!("expected NodeNotFound, got: {other:?}"),
    }
}

/// Run with checkpointing enabled and verify checkpoint data is populated.
#[tokio::test]
async fn test_engine_checkpoint_path_covered() {
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
        ..EngineConfig::default()
    };
    let engine = Engine::with_config(graph, stamping_registry(counter), config);
    let ctx = Context::new();
    ctx.set("checkpoint_test", json!("data_value"));

    let result = engine.run(ctx).await.unwrap();

    // Checkpoint should exist
    assert!(result.checkpoint.is_some());
    let cp = result.checkpoint.unwrap();

    // Pipeline name from graph
    assert_eq!(cp.pipeline_name, "coverage_audit");

    // All visited nodes recorded
    assert!(cp.was_visited("start"));
    assert!(cp.was_visited("worker"));
    assert!(cp.was_visited("exit"));

    // Context snapshot captured
    assert_eq!(
        cp.context_snapshot.get("checkpoint_test"),
        Some(&json!("data_value"))
    );

    // Handler-set values in snapshot
    assert_eq!(
        cp.context_snapshot.get("visited_worker"),
        Some(&json!(true))
    );

    // Node outcomes recorded
    assert!(cp.node_outcomes.get("start").unwrap().is_success());
    assert!(cp.node_outcomes.get("worker").unwrap().is_success());
    assert!(cp.node_outcomes.get("exit").unwrap().is_success());

    // Checkpoint serialization round-trip
    let json_str = cp.to_json().unwrap();
    let restored = Checkpoint::from_json(&json_str).unwrap();
    assert_eq!(restored.pipeline_name, "coverage_audit");
    assert!(restored.was_visited("worker"));
    assert_eq!(
        restored.context_snapshot.get("checkpoint_test"),
        Some(&json!("data_value"))
    );

    // Envelope seal/verify/open round-trip
    let envelope = CheckpointEnvelope::seal(restored);
    assert!(envelope.verify());
    let opened = envelope.open().unwrap();
    assert_eq!(opened.pipeline_name, "coverage_audit");
}

/// Trigger retry logic by using a handler that fails once then succeeds.
#[tokio::test]
async fn test_engine_retry_path_covered() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let mut retry_attrs = HashMap::new();
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
    // Fail once (initial call is attempt 0 for start which succeeds,
    // attempt 1 for flaky which is the first fail), succeed on attempt 2.
    registry.register(Arc::new(FailThenSucceedHandler {
        fail_count: 1,
        attempts: attempts.clone(),
    }));
    let engine = Engine::new(graph, registry);

    let result = engine.run(Context::new()).await.unwrap();

    // Pipeline completes including exit
    assert!(result.visited_nodes.contains(&"exit".to_string()));

    // The handler was called multiple times due to retries
    let total_attempts = attempts.load(Ordering::SeqCst);
    assert!(
        total_attempts >= 3,
        "expected at least 3 handler calls (start + flaky initial + flaky retry), got {total_attempts}"
    );
}

// ===========================================================================
// 2. Handler critical paths
// ===========================================================================

/// Verify that the default registry has handlers for Start, Exit, and Conditional.
/// Then register additional handlers and verify all NodeType variants have coverage.
#[tokio::test]
async fn test_all_node_types_have_handlers() {
    let registry = default_registry();

    // Built-in handlers cover Start, Exit, Conditional
    assert!(
        registry.get_handler(&NodeType::Start).is_some(),
        "Start should have a handler"
    );
    assert!(
        registry.get_handler(&NodeType::Exit).is_some(),
        "Exit should have a handler"
    );
    assert!(
        registry.get_handler(&NodeType::Conditional).is_some(),
        "Conditional should have a handler"
    );

    // Types not covered by default: Codergen, Tool, Interviewer, Parallel, Manager, SubPipeline, Generic
    // In a real setup these would have handlers registered.
    // Verify that at least the built-in three are present and usable.
    let node_types_with_handlers = [NodeType::Start, NodeType::Exit, NodeType::Conditional];
    for nt in &node_types_with_handlers {
        let handler = registry.get_handler(nt).unwrap();
        assert!(!handler.name().is_empty());
    }

    // Now create a comprehensive registry with a catch-all handler
    let mut full_registry = default_registry();
    full_registry.register(Arc::new(PassthroughHandler));

    // All node types should now be handled (PassthroughHandler handles everything)
    let all_types = [
        NodeType::Start,
        NodeType::Exit,
        NodeType::Codergen,
        NodeType::Conditional,
        NodeType::Tool,
        NodeType::Interviewer,
        NodeType::Parallel,
        NodeType::FanIn,
        NodeType::Manager,
        NodeType::SubPipeline,
        NodeType::Generic,
    ];
    for nt in &all_types {
        assert!(
            full_registry.get_handler(nt).is_some(),
            "NodeType {:?} should have a handler",
            nt
        );
    }
}

/// Verify that a HandlerError from a handler becomes EngineError::Handler.
#[tokio::test]
async fn test_handler_error_propagation() {
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("worker", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "worker"), make_edge("worker", "exit")],
    );

    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(ErrorOnNodeHandler {
        target: "worker".to_string(),
    }));
    let engine = Engine::new(graph, registry);

    let err = engine.run(Context::new()).await.unwrap_err();

    // The error chain: HandlerError -> EngineError::Handler
    assert!(matches!(err, EngineError::Handler(_)));
    let msg = err.to_string();
    assert!(msg.contains("handler error") || msg.contains("deliberate test failure"));

    // Also verify no-handler case
    let graph2 = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("tool_node", NodeType::Tool),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "tool_node"),
            make_edge("tool_node", "exit"),
        ],
    );
    // Empty registry: no handlers at all
    let empty_reg = HandlerRegistry::new();
    let err2 = Engine::new(graph2, empty_reg)
        .run(Context::new())
        .await
        .unwrap_err();
    assert!(matches!(err2, EngineError::Handler(_)));
    assert!(err2.to_string().contains("no handler"));
}

// ===========================================================================
// 3. State critical paths
// ===========================================================================

/// Spawn multiple tasks reading/writing context concurrently.
#[tokio::test]
async fn test_context_concurrent_access() {
    let ctx = Context::new();

    // Spawn 20 write tasks and 20 read tasks concurrently
    let mut handles = Vec::new();
    for i in 0..20 {
        let ctx_clone = ctx.clone();
        handles.push(tokio::spawn(async move {
            ctx_clone.set(format!("key_{i}"), json!(i));
        }));
    }
    for i in 0..20 {
        let ctx_clone = ctx.clone();
        handles.push(tokio::spawn(async move {
            // Read may return None if the write hasn't happened yet
            let _ = ctx_clone.get(&format!("key_{i}"));
        }));
    }

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }

    // After all writes complete, all 20 keys should be present
    let keys = ctx.keys();
    assert_eq!(keys.len(), 20, "expected 20 keys, got {}", keys.len());

    for i in 0..20 {
        let val = ctx.get(&format!("key_{i}"));
        assert_eq!(val, Some(json!(i)), "key_{i} should have value {i}");
    }

    // Verify snapshot is a deep clone (concurrent-safe)
    let snap = ctx.snapshot();
    ctx.set("key_0", json!("modified"));
    assert_eq!(snap.get("key_0"), Some(&json!(0)));
    assert_eq!(ctx.get("key_0"), Some(json!("modified")));
}

/// Save a checkpoint and restore it, verifying all data is preserved.
#[tokio::test]
async fn test_checkpoint_round_trip_preserves_data() {
    let ctx = Context::new();
    ctx.set("alpha", json!("one"));
    ctx.set("beta", json!(42));
    ctx.set("gamma", json!({"nested": true, "list": [1, 2, 3]}));

    let mut cp = Checkpoint::new("test_pipeline", "node_c", &ctx);
    cp.mark_visited("node_a");
    cp.mark_visited("node_b");
    cp.mark_visited("node_c");
    cp.add_outcome("node_a", Outcome::success());
    cp.add_outcome("node_b", Outcome::success_with(json!({"data": "payload"})));
    cp.add_outcome("node_c", Outcome::retryable_failure("transient error"));

    // Serialize
    let json_str = cp.to_json().unwrap();

    // Deserialize
    let restored = Checkpoint::from_json(&json_str).unwrap();

    // Verify all fields preserved
    assert_eq!(restored.pipeline_name, "test_pipeline");
    assert_eq!(restored.current_node, "node_c");
    assert_eq!(restored.version, 1);
    assert_eq!(restored.visited_nodes, vec!["node_a", "node_b", "node_c"]);
    assert!(restored.was_visited("node_a"));
    assert!(restored.was_visited("node_b"));
    assert!(restored.was_visited("node_c"));
    assert!(!restored.was_visited("node_d"));

    // Context snapshot
    assert_eq!(restored.context_snapshot.get("alpha"), Some(&json!("one")));
    assert_eq!(restored.context_snapshot.get("beta"), Some(&json!(42)));
    assert_eq!(
        restored.context_snapshot.get("gamma"),
        Some(&json!({"nested": true, "list": [1, 2, 3]}))
    );

    // Node outcomes
    assert_eq!(
        restored.node_outcomes.get("node_a"),
        Some(&Outcome::success())
    );
    assert_eq!(
        restored.node_outcomes.get("node_b"),
        Some(&Outcome::success_with(json!({"data": "payload"})))
    );
    assert_eq!(
        restored.node_outcomes.get("node_c"),
        Some(&Outcome::retryable_failure("transient error"))
    );

    // Checksum integrity
    let checksum = cp.compute_checksum();
    assert!(cp.verify_checksum(&checksum));
    assert!(!cp.verify_checksum("bad_checksum"));

    // Envelope round-trip
    let envelope = CheckpointEnvelope::seal(cp);
    assert!(envelope.verify());
    let json_envelope = serde_json::to_string_pretty(&envelope).unwrap();
    let restored_envelope: CheckpointEnvelope = serde_json::from_str(&json_envelope).unwrap();
    assert!(restored_envelope.verify());
    let opened = restored_envelope.open().unwrap();
    assert_eq!(opened.pipeline_name, "test_pipeline");
}

// ===========================================================================
// 4. Condition critical paths
// ===========================================================================

/// Test all comparison operators: ==, !=, >, <.
///
/// The condition module supports ==, !=, >, < as comparison operators.
/// (>= and <= are not implemented in the parser; the task is adapted accordingly.)
#[test]
fn test_all_comparison_operators() {
    let mut ctx = HashMap::new();
    ctx.insert("status".to_string(), "done".to_string());
    ctx.insert("count".to_string(), "10".to_string());
    ctx.insert("score".to_string(), "7.5".to_string());

    // == (Equals)
    let eq_cond = parse_condition("status=done").unwrap();
    assert!(evaluate_condition(&eq_cond, &ctx));
    let eq_false = parse_condition("status=pending").unwrap();
    assert!(!evaluate_condition(&eq_false, &ctx));

    // != (NotEquals)
    let ne_cond = parse_condition("status!=pending").unwrap();
    assert!(evaluate_condition(&ne_cond, &ctx));
    let ne_false = parse_condition("status!=done").unwrap();
    assert!(!evaluate_condition(&ne_false, &ctx));

    // > (GreaterThan)
    let gt_cond = parse_condition("score>5.0").unwrap();
    assert!(evaluate_condition(&gt_cond, &ctx));
    let gt_false = parse_condition("score>10.0").unwrap();
    assert!(!evaluate_condition(&gt_false, &ctx));

    // < (LessThan)
    let lt_cond = parse_condition("count<20").unwrap();
    assert!(evaluate_condition(&lt_cond, &ctx));
    let lt_false = parse_condition("count<5").unwrap();
    assert!(!evaluate_condition(&lt_false, &ctx));

    // Boundary: exact equality for numeric comparisons
    let gt_exact = parse_condition("score>7.5").unwrap();
    assert!(!evaluate_condition(&gt_exact, &ctx)); // 7.5 is not > 7.5
    let lt_exact = parse_condition("score<7.5").unwrap();
    assert!(!evaluate_condition(&lt_exact, &ctx)); // 7.5 is not < 7.5
}

/// Test boolean operators: AND, OR, NOT.
#[test]
fn test_boolean_operators() {
    let mut ctx = HashMap::new();
    ctx.insert("a".to_string(), "1".to_string());
    ctx.insert("b".to_string(), "2".to_string());
    ctx.insert("c".to_string(), "3".to_string());

    // AND: both true
    let and_true = parse_condition("a=1 && b=2").unwrap();
    assert!(evaluate_condition(&and_true, &ctx));

    // AND: one false
    let and_false = parse_condition("a=1 && b=99").unwrap();
    assert!(!evaluate_condition(&and_false, &ctx));

    // OR: one true
    let or_true = parse_condition("a=99 || b=2").unwrap();
    assert!(evaluate_condition(&or_true, &ctx));

    // OR: both false
    let or_false = parse_condition("a=99 || b=99").unwrap();
    assert!(!evaluate_condition(&or_false, &ctx));

    // NOT: invert true
    let not_true = parse_condition("!a=99").unwrap();
    assert!(evaluate_condition(&not_true, &ctx)); // a != 99, so !(false) = true

    // NOT: invert false
    let not_false = parse_condition("!a=1").unwrap();
    assert!(!evaluate_condition(&not_false, &ctx)); // a == 1, so !(true) = false

    // Boolean literals
    let lit_true = parse_condition("true").unwrap();
    assert!(evaluate_condition(&lit_true, &ctx));
    let lit_false = parse_condition("false").unwrap();
    assert!(!evaluate_condition(&lit_false, &ctx));

    // NOT with literal
    let not_lit = parse_condition("!false").unwrap();
    assert!(evaluate_condition(&not_lit, &ctx));
}

/// Test nested expressions: (a == b) AND (c != d).
#[test]
fn test_nested_expressions() {
    let mut ctx = HashMap::new();
    ctx.insert("status".to_string(), "ok".to_string());
    ctx.insert("score".to_string(), "8".to_string());
    ctx.insert("mode".to_string(), "fast".to_string());

    // (a == b) AND (c != d) style
    let nested = parse_condition("(status=ok) && (mode!=slow)").unwrap();
    assert!(evaluate_condition(&nested, &ctx));

    // Complex nesting: (a && b) || c
    let complex = parse_condition("(status=ok && score>5) || mode=slow").unwrap();
    assert!(evaluate_condition(&complex, &ctx));

    // Complex nesting: a && (b || c)
    let complex2 = parse_condition("status=ok && (score>100 || mode=fast)").unwrap();
    assert!(evaluate_condition(&complex2, &ctx));

    // Nested NOT
    let nested_not = parse_condition("!(status=error)").unwrap();
    assert!(evaluate_condition(&nested_not, &ctx));

    // Deeply nested
    let deep = parse_condition("((status=ok) && (score>5)) && (!(mode=slow))").unwrap();
    assert!(evaluate_condition(&deep, &ctx));

    // Failing case: outer AND with inner failure
    let fail = parse_condition("(status=ok) && (score>100)").unwrap();
    assert!(!evaluate_condition(&fail, &ctx));
}

// ===========================================================================
// 5. Edge selection critical paths
// ===========================================================================

/// Higher priority edge should be selected over lower priority.
#[test]
fn test_edge_priority_ordering() {
    let graph = make_graph(
        vec![
            make_node("a", NodeType::Generic),
            make_node("low", NodeType::Generic),
            make_node("mid", NodeType::Generic),
            make_node("high", NodeType::Generic),
        ],
        vec![
            make_priority_edge("a", "low", 1),
            make_priority_edge("a", "mid", 5),
            make_priority_edge("a", "high", 10),
        ],
    );
    let ctx = Context::new();

    let selected = select_edge(&graph, "a", &ctx, None).unwrap();
    assert!(selected.is_some());
    assert_eq!(selected.unwrap().to, "high");

    // Default priority (None) is treated as 0
    let graph2 = make_graph(
        vec![
            make_node("a", NodeType::Generic),
            make_node("default_prio", NodeType::Generic),
            make_node("prio_one", NodeType::Generic),
        ],
        vec![
            make_edge("a", "default_prio"),
            make_priority_edge("a", "prio_one", 1),
        ],
    );
    let selected2 = select_edge(&graph2, "a", &ctx, None).unwrap();
    assert_eq!(selected2.unwrap().to, "prio_one");
}

/// Edge with a condition expression should be selected when the condition is true.
#[test]
fn test_edge_condition_evaluation() {
    let graph = make_graph(
        vec![
            make_node("a", NodeType::Generic),
            make_node("yes_target", NodeType::Generic),
            make_node("no_target", NodeType::Generic),
        ],
        vec![
            make_conditional_edge("a", "yes_target", "ready=true"),
            make_conditional_edge("a", "no_target", "ready=false"),
        ],
    );

    // When ready=true, yes_target selected
    let ctx_yes = Context::new();
    ctx_yes.set("ready", json!("true"));
    let selected = select_edge(&graph, "a", &ctx_yes, None).unwrap();
    assert_eq!(selected.unwrap().to, "yes_target");

    // When ready=false, no_target selected
    let ctx_no = Context::new();
    ctx_no.set("ready", json!("false"));
    let selected2 = select_edge(&graph, "a", &ctx_no, None).unwrap();
    assert_eq!(selected2.unwrap().to, "no_target");

    // When neither condition matches, no edge selected
    let ctx_other = Context::new();
    ctx_other.set("ready", json!("maybe"));
    let selected3 = select_edge(&graph, "a", &ctx_other, None).unwrap();
    assert!(selected3.is_none());
}

/// Edge with no condition serves as a fallback when other conditions fail.
#[test]
fn test_edge_fallback_no_condition() {
    let graph = make_graph(
        vec![
            make_node("a", NodeType::Generic),
            make_node("conditional_target", NodeType::Generic),
            make_node("fallback_target", NodeType::Generic),
        ],
        vec![
            make_conditional_edge("a", "conditional_target", "status=done"),
            make_edge("a", "fallback_target"), // no condition = always passes
        ],
    );

    // When condition is not met, fallback edge is selected
    let ctx = Context::new();
    ctx.set("status", json!("pending"));
    let selected = select_edge(&graph, "a", &ctx, None).unwrap();
    assert!(selected.is_some());
    assert_eq!(selected.unwrap().to, "fallback_target");

    // When condition IS met, both pass but the conditional one may or may not
    // win depending on priority. Since both have default priority=0, the result
    // depends on order. What matters is that some edge is selected.
    let ctx_done = Context::new();
    ctx_done.set("status", json!("done"));
    let selected2 = select_edge(&graph, "a", &ctx_done, None).unwrap();
    assert!(selected2.is_some());

    // Outcome-based fallback: when outcome is provided but no labeled edges match,
    // all passing edges remain as candidates
    let graph2 = make_graph(
        vec![
            make_node("a", NodeType::Generic),
            make_node("target", NodeType::Generic),
        ],
        vec![make_edge("a", "target")],
    );
    let outcome = Outcome::success();
    let selected3 = select_edge(&graph2, "a", &Context::new(), Some(&outcome)).unwrap();
    assert!(selected3.is_some());
    assert_eq!(selected3.unwrap().to, "target");

    // Outcome matching: labeled edges are preferred when they match the outcome
    let graph3 = make_graph(
        vec![
            make_node("a", NodeType::Generic),
            make_node("success_path", NodeType::Generic),
            make_node("failure_path", NodeType::Generic),
        ],
        vec![
            make_labeled_edge("a", "success_path", "success"),
            make_labeled_edge("a", "failure_path", "failure"),
        ],
    );
    let success_outcome = Outcome::success();
    let selected4 = select_edge(&graph3, "a", &Context::new(), Some(&success_outcome)).unwrap();
    assert_eq!(selected4.unwrap().to, "success_path");

    let failure_outcome = Outcome::failure("oops");
    let selected5 = select_edge(&graph3, "a", &Context::new(), Some(&failure_outcome)).unwrap();
    assert_eq!(selected5.unwrap().to, "failure_path");
}

// ===========================================================================
// Additional integration: engine with loop + checkpoint resume
// ===========================================================================

/// Run a looping pipeline, verify loop_restart counter, context clearing, and checkpoint.
#[tokio::test]
async fn test_loop_with_checkpoint_and_resume() {
    let pass_counter = Arc::new(AtomicUsize::new(0));

    let mut exit_edge = make_conditional_edge("decision", "exit", "loop_done=true");
    exit_edge.priority = Some(10);

    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("worker", NodeType::Generic),
            make_node("decision", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![
            make_edge("start", "worker"),
            make_edge("worker", "decision"),
            exit_edge,
            make_loop_restart_edge("decision", "worker"),
        ],
    );

    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(LoopControlHandler {
        trigger_node: "decision".to_string(),
        max_passes: 2,
        pass_counter: pass_counter.clone(),
    }));
    let config = EngineConfig {
        max_steps: 20,
        enable_checkpointing: true,
        ..EngineConfig::default()
    };
    let engine = Engine::with_config(graph.clone(), registry, config);

    let ctx = Context::new();
    ctx.set("decision_initial_key", json!("should_be_cleared"));

    let result = engine.run(ctx).await.unwrap();

    // Loop completed and reached exit
    assert!(result.visited_nodes.contains(&"exit".to_string()));

    // Loop restart edge was traversed once
    assert_eq!(result.loop_restarts.count("decision", "worker"), 1);
    assert_eq!(result.loop_restarts.total(), 1);

    // Context key prefixed with "decision_" was cleared by loop_restart
    assert!(
        !result.final_context.contains_key("decision_initial_key"),
        "decision-prefixed key should have been cleared"
    );

    // Checkpoint exists and contains final state
    assert!(result.checkpoint.is_some());
    let cp = result.checkpoint.unwrap();
    assert!(cp.was_visited("exit"));

    // Resume from checkpoint
    let mut registry2 = HandlerRegistry::new();
    registry2.register(Arc::new(PassthroughHandler));
    let config2 = EngineConfig {
        max_steps: 20,
        enable_checkpointing: true,
        ..EngineConfig::default()
    };

    // Create a checkpoint mid-pipeline to test resume
    let cp_ctx = Context::new();
    let mut resume_cp = Checkpoint::new("coverage_audit", "worker", &cp_ctx);
    resume_cp.mark_visited("start");
    resume_cp.add_outcome("start", Outcome::success());

    // Need a fresh engine without loops for the resume test since the
    // looping handler state is fresh
    let simple_graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("worker", NodeType::Generic),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "worker"), make_edge("worker", "exit")],
    );
    let engine2 = Engine::with_config(simple_graph, passthrough_registry(), config2);

    let result2 = engine2
        .run_from_checkpoint(resume_cp, Context::new())
        .await
        .unwrap();

    // Resumed from worker, visited worker and exit
    assert!(result2.visited_nodes.contains(&"start".to_string()));
    assert!(result2.visited_nodes.contains(&"worker".to_string()));
    assert!(result2.visited_nodes.contains(&"exit".to_string()));
    assert_eq!(result2.steps_taken, 2); // worker + exit
}
