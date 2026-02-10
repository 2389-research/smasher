// ABOUTME: End-to-end smoke tests exercising full pipeline execution scenarios.
// ABOUTME: Covers multi-stage pipelines, events, checkpointing, retries, and conditional routing.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::json;

use smasher_attractor::engine::{Engine, EngineConfig};
use smasher_attractor::events::{PipelineEvent, PipelineEventEmitter, PipelineEventLog};
use smasher_attractor::goals::GoalGate;
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

fn make_graph(nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> Graph {
    Graph {
        name: Some("smoke_test".to_string()),
        nodes,
        edges,
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

// ---------------------------------------------------------------------------
// Smoke test handlers
// ---------------------------------------------------------------------------

/// All-types handler that delegates to stage-specific logic based on node ID.
/// Combines plan, implement, and review into a single handler for simplicity.
struct SmokeTestHandler;

#[async_trait]
impl Handler for SmokeTestHandler {
    fn name(&self) -> &str {
        "smoke_test_handler"
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        context.set(format!("visited_{}", node.id), json!(true));

        match node.id.as_str() {
            "plan" => {
                context.set("plan_output", json!("implement feature X with module Y"));
                Ok(Outcome::success_with(json!({"stage": "plan"})))
            }
            "implement" => {
                let plan = context.get_string("plan_output").unwrap_or_default();
                context.set("code_output", json!(format!("code based on: {plan}")));
                Ok(Outcome::success_with(json!({"stage": "implement"})))
            }
            "review" => {
                let decision = context
                    .get_string("review_decision")
                    .unwrap_or_else(|| "pass".to_string());
                context.set("review_result", json!(decision.clone()));
                if decision == "pass" {
                    Ok(Outcome::success_with(json!({"review": "pass"})))
                } else {
                    Ok(Outcome::failure("review failed"))
                }
            }
            _ => Ok(Outcome::success()),
        }
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Handler that fails with a retryable error on the first N invocations
/// of a specific node, then succeeds. Tracks total attempts.
struct RetryableHandler {
    target_node: String,
    fail_count: usize,
    attempts: Arc<AtomicUsize>,
}

impl RetryableHandler {
    fn new(target_node: &str, fail_count: usize) -> (Self, Arc<AtomicUsize>) {
        let attempts = Arc::new(AtomicUsize::new(0));
        (
            Self {
                target_node: target_node.to_string(),
                fail_count,
                attempts: attempts.clone(),
            },
            attempts,
        )
    }
}

#[async_trait]
impl Handler for RetryableHandler {
    fn name(&self) -> &str {
        "retryable_handler"
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        context.set(format!("visited_{}", node.id), json!(true));

        if node.id == self.target_node {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt < self.fail_count {
                return Ok(Outcome::retryable_failure(format!(
                    "transient failure attempt {attempt}"
                )));
            }
        }
        Ok(Outcome::success())
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

/// Handler for conditional routing tests. Reads a context key to determine
/// the outcome label for edge selection.
struct ContextDrivenRouter;

#[async_trait]
impl Handler for ContextDrivenRouter {
    fn name(&self) -> &str {
        "context_driven_router"
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        context.set(format!("visited_{}", node.id), json!(true));

        if node.node_type == NodeType::Conditional {
            let decision = context
                .get_string("route_decision")
                .unwrap_or_else(|| "true".to_string());
            if decision == "true" {
                Ok(Outcome::success_with(json!({"result": true})))
            } else {
                Ok(Outcome::failure("route to failure path"))
            }
        } else {
            Ok(Outcome::success())
        }
    }

    fn handles(&self, _node_type: &NodeType) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Helper: build the multi-stage pipeline used in tests 1-3
//
// Topology:
//   start -> plan(box) -> implement(box) -> review(diamond) -> exit_pass / exit_fail
//
// The review node is Conditional. Edge to exit_pass has label "success",
// edge to exit_fail has label "failure".
// ---------------------------------------------------------------------------

fn build_smoke_pipeline() -> Graph {
    let mut implement_attrs = HashMap::new();
    implement_attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));

    make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("plan", NodeType::Generic),
            make_node_with_attrs("implement", NodeType::Generic, implement_attrs),
            make_node("review", NodeType::Conditional),
            make_node("exit_pass", NodeType::Exit),
            make_node("exit_fail", NodeType::Exit),
        ],
        vec![
            make_edge("start", "plan"),
            make_edge("plan", "implement"),
            make_edge("implement", "review"),
            make_labeled_edge("review", "exit_pass", "success"),
            make_labeled_edge("review", "exit_fail", "failure"),
        ],
    )
}

fn smoke_test_registry() -> HandlerRegistry {
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(SmokeTestHandler));
    registry
}

// ============================================================================
// Test 1: Full pipeline smoke test
// ============================================================================

#[tokio::test]
async fn smoke_full_pipeline_pass() {
    let graph = build_smoke_pipeline();
    let engine = Engine::new(graph, smoke_test_registry());

    let ctx = Context::new();
    ctx.set("review_decision", json!("pass"));

    let result = engine.run(ctx).await.unwrap();

    // All nodes except exit_fail should be visited
    assert!(result.visited_nodes.contains(&"start".to_string()));
    assert!(result.visited_nodes.contains(&"plan".to_string()));
    assert!(result.visited_nodes.contains(&"implement".to_string()));
    assert!(result.visited_nodes.contains(&"review".to_string()));
    assert!(result.visited_nodes.contains(&"exit_pass".to_string()));
    assert!(
        !result.visited_nodes.contains(&"exit_fail".to_string()),
        "exit_fail should NOT be visited when review passes"
    );

    // Verify context outputs from each stage
    assert_eq!(
        result.final_context.get("plan_output"),
        Some(&json!("implement feature X with module Y"))
    );
    assert!(
        result
            .final_context
            .get("code_output")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("code based on: implement feature X with module Y")
    );
    assert_eq!(
        result.final_context.get("review_result"),
        Some(&json!("pass"))
    );

    // Verify goal gate was satisfied (implement node has goal=true)
    // If the goal wasn't met, engine.run would have returned an error
    assert_eq!(result.steps_taken, 5);
}

#[tokio::test]
async fn smoke_full_pipeline_fail() {
    let graph = build_smoke_pipeline();
    let engine = Engine::new(graph, smoke_test_registry());

    let ctx = Context::new();
    ctx.set("review_decision", json!("fail"));

    let result = engine.run(ctx).await.unwrap();

    // Review fails, so exit_fail should be reached
    assert!(result.visited_nodes.contains(&"review".to_string()));
    assert!(result.visited_nodes.contains(&"exit_fail".to_string()));
    assert!(
        !result.visited_nodes.contains(&"exit_pass".to_string()),
        "exit_pass should NOT be visited when review fails"
    );

    // Context should still have all stage outputs
    assert_eq!(
        result.final_context.get("review_result"),
        Some(&json!("fail"))
    );
}

#[tokio::test]
async fn smoke_full_pipeline_goal_gate_satisfied() {
    let graph = build_smoke_pipeline();

    // Verify goal gate independently
    let goal_gate = GoalGate::from_graph(&graph);
    let goals = goal_gate.goals();
    assert_eq!(goals.len(), 1);
    assert_eq!(goals[0], "implement");

    // Run the pipeline and confirm goal enforcement passes
    let engine = Engine::new(graph, smoke_test_registry());
    let ctx = Context::new();
    ctx.set("review_decision", json!("pass"));

    let result = engine.run(ctx).await;
    assert!(
        result.is_ok(),
        "pipeline should succeed with goal node visited"
    );
}

// ============================================================================
// Test 2: Pipeline with events
// ============================================================================

#[tokio::test]
async fn smoke_events_emitter_and_log() {
    // The events infrastructure operates independently from the engine.
    // This test verifies the emitter/log pipeline with realistic events.

    let emitter = PipelineEventEmitter::new(64);
    let log = PipelineEventLog::new();

    // Subscribe and collect events in a background task
    let mut receiver = emitter.subscribe();
    let log_clone = log.clone();
    let collector = tokio::spawn(async move {
        while let Ok(event) = receiver.recv().await {
            log_clone.push(event);
        }
    });

    // Simulate a pipeline execution sequence
    let node_ids = ["start", "plan", "implement", "review", "exit_pass"];

    emitter.emit(PipelineEvent::PipelineStarted {
        graph_name: "smoke_test".to_string(),
        timestamp: Utc::now(),
    });

    for node_id in &node_ids {
        emitter.emit(PipelineEvent::NodeStarted {
            node_id: node_id.to_string(),
            node_type: "Generic".to_string(),
            timestamp: Utc::now(),
        });
        emitter.emit(PipelineEvent::NodeCompleted {
            node_id: node_id.to_string(),
            outcome: Outcome::success(),
            duration_ms: 10,
            timestamp: Utc::now(),
        });
    }

    emitter.emit(PipelineEvent::PipelineCompleted {
        outcome: Outcome::success(),
        total_nodes: 5,
        duration_ms: 50,
        timestamp: Utc::now(),
    });

    // Drop the emitter to close the channel so the collector task ends
    drop(emitter);
    let _ = collector.await;

    // Verify event sequence
    let events = log.events();

    // Should have: PipelineStarted + (NodeStarted + NodeCompleted) * 5 + PipelineCompleted
    // = 1 + 10 + 1 = 12
    assert_eq!(
        events.len(),
        12,
        "expected 12 events (1 start + 5*2 node events + 1 complete)"
    );

    // First event should be PipelineStarted
    assert!(
        matches!(&events[0], PipelineEvent::PipelineStarted { .. }),
        "first event should be PipelineStarted"
    );

    // Last event should be PipelineCompleted
    assert!(
        matches!(
            &events[events.len() - 1],
            PipelineEvent::PipelineCompleted { .. }
        ),
        "last event should be PipelineCompleted"
    );

    // Each node should have a NodeStarted and NodeCompleted pair
    for node_id in &node_ids {
        let node_events = log.events_for_node(node_id);
        assert_eq!(
            node_events.len(),
            2,
            "node '{node_id}' should have 2 events (started + completed)"
        );
    }

    // Verify node event count
    let node_events_count = events.iter().filter(|e| e.is_node_event()).count();
    assert_eq!(node_events_count, 10, "should have 10 node-level events");

    let pipeline_events_count = events.iter().filter(|e| e.is_pipeline_event()).count();
    assert_eq!(
        pipeline_events_count, 2,
        "should have 2 pipeline-level events"
    );
}

#[tokio::test]
async fn smoke_event_log_summary() {
    let log = PipelineEventLog::new();

    log.push(PipelineEvent::PipelineStarted {
        graph_name: "smoke_test".to_string(),
        timestamp: Utc::now(),
    });

    let nodes = ["start", "plan", "implement", "review", "exit"];
    for node_id in &nodes {
        log.push(PipelineEvent::NodeStarted {
            node_id: node_id.to_string(),
            node_type: "Generic".to_string(),
            timestamp: Utc::now(),
        });
        log.push(PipelineEvent::NodeCompleted {
            node_id: node_id.to_string(),
            outcome: Outcome::success(),
            duration_ms: 5,
            timestamp: Utc::now(),
        });
    }

    log.push(PipelineEvent::PipelineCompleted {
        outcome: Outcome::success(),
        total_nodes: 5,
        duration_ms: 25,
        timestamp: Utc::now(),
    });

    let summary = log.summary().expect("summary should be present");
    assert_eq!(summary.graph_name, "smoke_test");
    assert_eq!(summary.total_nodes, 5);
    assert!(summary.final_outcome.is_success());
    assert_eq!(summary.node_summaries.len(), 5);
}

// ============================================================================
// Test 3: Pipeline with checkpointing and resume
// ============================================================================

#[tokio::test]
async fn smoke_checkpoint_and_resume() {
    // Phase 1: Run pipeline to the plan node, then stop.
    // We build a truncated graph: start -> plan (no outgoing edges from plan)
    // so the engine stops naturally after plan.

    let phase1_graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("plan", NodeType::Generic),
        ],
        vec![make_edge("start", "plan")],
    );

    let config = EngineConfig {
        max_steps: 100,
        enable_checkpointing: true,
        cancellation_token: None,
    };
    let engine1 = Engine::with_config(phase1_graph, smoke_test_registry(), config);
    let ctx = Context::new();
    let phase1_result = engine1.run(ctx).await.unwrap();

    // Verify phase 1 stopped at plan
    assert_eq!(phase1_result.visited_nodes, vec!["start", "plan"]);
    assert_eq!(
        phase1_result.final_context.get("plan_output"),
        Some(&json!("implement feature X with module Y"))
    );

    // Get the checkpoint
    let checkpoint = phase1_result
        .checkpoint
        .expect("checkpoint should be captured");
    assert!(checkpoint.was_visited("start"));
    assert!(checkpoint.was_visited("plan"));

    // Serialize and deserialize the checkpoint to simulate persistence
    let checkpoint_json = checkpoint.to_json().expect("checkpoint serialization");
    let restored_checkpoint =
        Checkpoint::from_json(&checkpoint_json).expect("checkpoint deserialization");

    // Phase 2: Resume from the implement node.
    // Build the full remaining graph. The engine resumes at a specified node.
    let mut implement_attrs = HashMap::new();
    implement_attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));

    let phase2_graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("plan", NodeType::Generic),
            make_node_with_attrs("implement", NodeType::Generic, implement_attrs),
            make_node("review", NodeType::Conditional),
            make_node("exit_pass", NodeType::Exit),
            make_node("exit_fail", NodeType::Exit),
        ],
        vec![
            make_edge("start", "plan"),
            make_edge("plan", "implement"),
            make_edge("implement", "review"),
            make_labeled_edge("review", "exit_pass", "success"),
            make_labeled_edge("review", "exit_fail", "failure"),
        ],
    );

    // Create a checkpoint that resumes at "implement"
    let resume_ctx = Context::new();
    let mut resume_checkpoint = Checkpoint::new("smoke_test", "implement", &resume_ctx);
    resume_checkpoint.mark_visited("start");
    resume_checkpoint.mark_visited("plan");
    resume_checkpoint.add_outcome("start", Outcome::success());
    resume_checkpoint.add_outcome("plan", Outcome::success());
    // Restore context from the original checkpoint
    for (key, value) in &restored_checkpoint.context_snapshot {
        resume_checkpoint
            .context_snapshot
            .insert(key.clone(), value.clone());
    }

    let config2 = EngineConfig {
        max_steps: 100,
        enable_checkpointing: true,
        cancellation_token: None,
    };
    let engine2 = Engine::with_config(phase2_graph, smoke_test_registry(), config2);
    let resume_ctx = Context::new();
    resume_ctx.set("review_decision", json!("pass"));

    let phase2_result = engine2
        .run_from_checkpoint(resume_checkpoint, resume_ctx)
        .await
        .unwrap();

    // Verify the resumed execution includes prior visited nodes plus new ones
    assert!(phase2_result.visited_nodes.contains(&"start".to_string()));
    assert!(phase2_result.visited_nodes.contains(&"plan".to_string()));
    assert!(
        phase2_result
            .visited_nodes
            .contains(&"implement".to_string())
    );
    assert!(phase2_result.visited_nodes.contains(&"review".to_string()));
    assert!(
        phase2_result
            .visited_nodes
            .contains(&"exit_pass".to_string())
    );

    // Only 3 new steps were executed (implement, review, exit_pass)
    assert_eq!(phase2_result.steps_taken, 3);

    // Context from phase 1 was restored (plan_output) and phase 2 added code_output
    assert!(phase2_result.final_context.contains_key("plan_output"));
    assert!(phase2_result.final_context.contains_key("code_output"));
}

// ============================================================================
// Test 4: Error recovery smoke test (retryable handler)
// ============================================================================

#[tokio::test]
async fn smoke_retry_recovers_after_transient_failure() {
    // Build a simple pipeline: start -> flaky -> exit
    // The flaky node has retries=2 (so max_attempts=3) and minimal delays.
    let mut flaky_attrs = HashMap::new();
    flaky_attrs.insert("retries".to_string(), NodeAttrValue::Number(2.0));
    flaky_attrs.insert(
        "retry_delay".to_string(),
        NodeAttrValue::Duration(Duration::from_millis(1)),
    );
    flaky_attrs.insert(
        "max_retry_delay".to_string(),
        NodeAttrValue::Duration(Duration::from_millis(5)),
    );
    flaky_attrs.insert("retry_jitter".to_string(), NodeAttrValue::Bool(false));

    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node_with_attrs("flaky", NodeType::Generic, flaky_attrs),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "flaky"), make_edge("flaky", "exit")],
    );

    // Handler that fails once then succeeds (within the 3-attempt retry budget)
    let (handler, attempts) = RetryableHandler::new("flaky", 1);
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(handler));

    let engine = Engine::new(graph, registry);
    let ctx = Context::new();
    let result = engine.run(ctx).await.unwrap();

    // Pipeline should complete despite the initial failure
    assert!(result.visited_nodes.contains(&"start".to_string()));
    assert!(result.visited_nodes.contains(&"flaky".to_string()));
    assert!(result.visited_nodes.contains(&"exit".to_string()));
    assert_eq!(result.steps_taken, 3);

    // The retryable handler was called twice for the flaky node
    // (once failing, once succeeding)
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "flaky node should have been attempted twice (1 fail + 1 success)"
    );

    // The final outcome for flaky should be success (after retry)
    let flaky_outcome = result
        .node_outcomes
        .get("flaky")
        .expect("flaky outcome should exist");
    assert!(
        flaky_outcome.is_success(),
        "flaky node outcome should be success after retry"
    );
}

#[tokio::test]
async fn smoke_retry_multiple_failures_before_success() {
    // Handler fails twice then succeeds on third attempt. retries=3 gives 4 attempts total.
    let mut flaky_attrs = HashMap::new();
    flaky_attrs.insert("retries".to_string(), NodeAttrValue::Number(3.0));
    flaky_attrs.insert(
        "retry_delay".to_string(),
        NodeAttrValue::Duration(Duration::from_millis(1)),
    );
    flaky_attrs.insert("retry_jitter".to_string(), NodeAttrValue::Bool(false));

    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node_with_attrs("flaky", NodeType::Generic, flaky_attrs),
            make_node("exit", NodeType::Exit),
        ],
        vec![make_edge("start", "flaky"), make_edge("flaky", "exit")],
    );

    let (handler, attempts) = RetryableHandler::new("flaky", 2);
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(handler));

    let engine = Engine::new(graph, registry);
    let ctx = Context::new();
    let result = engine.run(ctx).await.unwrap();

    assert_eq!(result.visited_nodes, vec!["start", "flaky", "exit"]);
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        3,
        "flaky node should have been attempted 3 times (2 fails + 1 success)"
    );
}

// ============================================================================
// Test 5: Conditional routing smoke test
// ============================================================================

#[tokio::test]
async fn smoke_conditional_routing_passes() {
    // Pipeline: start -> decision(diamond) -> exit_a / exit_b
    // Decision routes to exit_a when route_decision=true, exit_b when false.

    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("decision", NodeType::Conditional),
            make_node("exit_a", NodeType::Exit),
            make_node("exit_b", NodeType::Exit),
        ],
        vec![
            make_edge("start", "decision"),
            make_labeled_edge("decision", "exit_a", "success"),
            make_labeled_edge("decision", "exit_b", "failure"),
        ],
    );

    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(ContextDrivenRouter));

    // Run 1: route_decision=true -> should reach exit_a
    let engine = Engine::new(graph.clone(), {
        let mut r = HandlerRegistry::new();
        r.register(Arc::new(ContextDrivenRouter));
        r
    });
    let ctx = Context::new();
    ctx.set("route_decision", json!("true"));

    let result = engine.run(ctx).await.unwrap();
    assert!(
        result.visited_nodes.contains(&"exit_a".to_string()),
        "should route to exit_a when decision is true"
    );
    assert!(
        !result.visited_nodes.contains(&"exit_b".to_string()),
        "should NOT visit exit_b when decision is true"
    );
}

#[tokio::test]
async fn smoke_conditional_routing_fails() {
    let graph = make_graph(
        vec![
            make_node("start", NodeType::Start),
            make_node("decision", NodeType::Conditional),
            make_node("exit_a", NodeType::Exit),
            make_node("exit_b", NodeType::Exit),
        ],
        vec![
            make_edge("start", "decision"),
            make_labeled_edge("decision", "exit_a", "success"),
            make_labeled_edge("decision", "exit_b", "failure"),
        ],
    );

    // Run 2: route_decision=false -> should reach exit_b
    let engine = Engine::new(graph, {
        let mut r = HandlerRegistry::new();
        r.register(Arc::new(ContextDrivenRouter));
        r
    });
    let ctx = Context::new();
    ctx.set("route_decision", json!("false"));

    let result = engine.run(ctx).await.unwrap();
    assert!(
        result.visited_nodes.contains(&"exit_b".to_string()),
        "should route to exit_b when decision is false"
    );
    assert!(
        !result.visited_nodes.contains(&"exit_a".to_string()),
        "should NOT visit exit_a when decision is false"
    );
}

#[tokio::test]
async fn smoke_conditional_routing_different_exits() {
    // Run the same pipeline topology twice with different context values
    // and verify different exit nodes are reached.
    let build_graph = || {
        make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("decision", NodeType::Conditional),
                make_node("exit_a", NodeType::Exit),
                make_node("exit_b", NodeType::Exit),
            ],
            vec![
                make_edge("start", "decision"),
                make_labeled_edge("decision", "exit_a", "success"),
                make_labeled_edge("decision", "exit_b", "failure"),
            ],
        )
    };

    let build_registry = || {
        let mut r = HandlerRegistry::new();
        r.register(Arc::new(ContextDrivenRouter));
        r
    };

    // Run 1: true -> exit_a
    let engine1 = Engine::new(build_graph(), build_registry());
    let ctx1 = Context::new();
    ctx1.set("route_decision", json!("true"));
    let result1 = engine1.run(ctx1).await.unwrap();

    // Run 2: false -> exit_b
    let engine2 = Engine::new(build_graph(), build_registry());
    let ctx2 = Context::new();
    ctx2.set("route_decision", json!("false"));
    let result2 = engine2.run(ctx2).await.unwrap();

    // The two runs should reach different exit nodes
    let exit1 = result1.visited_nodes.last().unwrap();
    let exit2 = result2.visited_nodes.last().unwrap();
    assert_ne!(
        exit1, exit2,
        "different context values should lead to different exits"
    );
    assert_eq!(exit1, "exit_a");
    assert_eq!(exit2, "exit_b");

    // Both should have visited start and decision
    assert!(result1.visited_nodes.contains(&"start".to_string()));
    assert!(result1.visited_nodes.contains(&"decision".to_string()));
    assert!(result2.visited_nodes.contains(&"start".to_string()));
    assert!(result2.visited_nodes.contains(&"decision".to_string()));
}
