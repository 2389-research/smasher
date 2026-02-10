// ABOUTME: Validation tests for the reusable test fixtures in common/test_fixtures.
// ABOUTME: Ensures each fixture function produces a structurally correct graph.

mod common;

use std::sync::atomic::Ordering;

use smasher_attractor::graph::NodeType;
use smasher_attractor::handler::HandlerRegistry;

use smasher_attractor::handler::HandlerError;
use smasher_attractor::state::Outcome;

use common::test_fixtures::{
    branching_pipeline, codergen_pipeline, complex_pipeline, human_gate_pipeline, linear_pipeline,
    looping_pipeline, make_counting_handler, make_delayed_handler, make_failing_handler,
    make_outcome_handler, make_test_context, parallel_pipeline, sub_pipeline_parent,
};

// ============================================================================
// linear_pipeline
// ============================================================================

#[test]
fn linear_pipeline_zero_intermediates_has_start_and_exit() {
    let graph = linear_pipeline(0);
    assert_eq!(graph.nodes.len(), 2);
    assert_eq!(graph.edges.len(), 1);

    let start_nodes = graph.start_nodes();
    assert_eq!(start_nodes.len(), 1);
    assert_eq!(start_nodes[0].id, "start");

    let exit_nodes = graph.exit_nodes();
    assert_eq!(exit_nodes.len(), 1);
    assert_eq!(exit_nodes[0].id, "exit");

    // Edge goes directly from start to exit
    assert_eq!(graph.edges[0].from, "start");
    assert_eq!(graph.edges[0].to, "exit");
}

#[test]
fn linear_pipeline_one_intermediate() {
    let graph = linear_pipeline(1);
    assert_eq!(graph.nodes.len(), 3);
    assert_eq!(graph.edges.len(), 2);

    assert!(graph.node("start").is_some());
    assert!(graph.node("box_0").is_some());
    assert!(graph.node("exit").is_some());

    // start -> box_0 -> exit
    let from_start = graph.edges_from("start");
    assert_eq!(from_start.len(), 1);
    assert_eq!(from_start[0].to, "box_0");

    let from_box = graph.edges_from("box_0");
    assert_eq!(from_box.len(), 1);
    assert_eq!(from_box[0].to, "exit");
}

#[test]
fn linear_pipeline_multiple_intermediates() {
    let graph = linear_pipeline(5);
    // start + 5 boxes + exit = 7
    assert_eq!(graph.nodes.len(), 7);
    // start->box_0, box_0->box_1, ..., box_4->exit = 6
    assert_eq!(graph.edges.len(), 6);

    assert_eq!(graph.start_nodes().len(), 1);
    assert_eq!(graph.exit_nodes().len(), 1);

    for i in 0..5 {
        let id = format!("box_{i}");
        let node = graph
            .node(&id)
            .unwrap_or_else(|| panic!("missing node {id}"));
        assert_eq!(node.node_type, NodeType::Generic);
    }

    // Verify the chain: start -> box_0 -> box_1 -> ... -> box_4 -> exit
    let mut current = "start".to_string();
    let expected_sequence = ["box_0", "box_1", "box_2", "box_3", "box_4", "exit"];
    for expected_next in &expected_sequence {
        let outgoing = graph.edges_from(&current);
        assert_eq!(
            outgoing.len(),
            1,
            "node {current} should have exactly 1 outgoing edge"
        );
        assert_eq!(outgoing[0].to, *expected_next);
        current = expected_next.to_string();
    }
}

#[test]
fn linear_pipeline_has_name() {
    let graph = linear_pipeline(3);
    assert_eq!(graph.name, Some("linear_pipeline".to_string()));
}

// ============================================================================
// branching_pipeline
// ============================================================================

#[test]
fn branching_pipeline_has_correct_node_count() {
    let graph = branching_pipeline();
    // start, diamond, exit_a, exit_b = 4
    assert_eq!(graph.nodes.len(), 4);
}

#[test]
fn branching_pipeline_has_start_node() {
    let graph = branching_pipeline();
    let start_nodes = graph.start_nodes();
    assert_eq!(start_nodes.len(), 1);
    assert_eq!(start_nodes[0].id, "start");
}

#[test]
fn branching_pipeline_has_two_exit_nodes() {
    let graph = branching_pipeline();
    let exit_nodes = graph.exit_nodes();
    assert_eq!(exit_nodes.len(), 2);
    let exit_ids: Vec<&str> = exit_nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(exit_ids.contains(&"exit_a"));
    assert!(exit_ids.contains(&"exit_b"));
}

#[test]
fn branching_pipeline_has_conditional_diamond() {
    let graph = branching_pipeline();
    let diamond = graph.node("diamond").expect("missing diamond node");
    assert_eq!(diamond.node_type, NodeType::Conditional);
}

#[test]
fn branching_pipeline_has_correct_edges() {
    let graph = branching_pipeline();
    // 3 edges: start->diamond, diamond->exit_a, diamond->exit_b
    assert_eq!(graph.edges.len(), 3);

    let from_start = graph.edges_from("start");
    assert_eq!(from_start.len(), 1);
    assert_eq!(from_start[0].to, "diamond");

    let from_diamond = graph.edges_from("diamond");
    assert_eq!(from_diamond.len(), 2);

    let to_a = from_diamond
        .iter()
        .find(|e| e.to == "exit_a")
        .expect("missing edge to exit_a");
    assert_eq!(to_a.condition, Some("result=true".to_string()));
    assert_eq!(to_a.priority, Some(1));

    let to_b = from_diamond
        .iter()
        .find(|e| e.to == "exit_b")
        .expect("missing edge to exit_b");
    assert_eq!(to_b.condition, Some("result=false".to_string()));
    assert_eq!(to_b.priority, Some(2));
}

#[test]
fn branching_pipeline_has_name() {
    let graph = branching_pipeline();
    assert_eq!(graph.name, Some("branching_pipeline".to_string()));
}

// ============================================================================
// looping_pipeline
// ============================================================================

#[test]
fn looping_pipeline_has_correct_node_count() {
    let graph = looping_pipeline(3);
    // start, worker, decision, exit = 4
    assert_eq!(graph.nodes.len(), 4);
}

#[test]
fn looping_pipeline_has_start_node() {
    let graph = looping_pipeline(3);
    let start_nodes = graph.start_nodes();
    assert_eq!(start_nodes.len(), 1);
    assert_eq!(start_nodes[0].id, "start");
}

#[test]
fn looping_pipeline_has_exit_node() {
    let graph = looping_pipeline(3);
    let exit_nodes = graph.exit_nodes();
    assert_eq!(exit_nodes.len(), 1);
    assert_eq!(exit_nodes[0].id, "exit");
}

#[test]
fn looping_pipeline_has_conditional_decision() {
    let graph = looping_pipeline(3);
    let decision = graph.node("decision").expect("missing decision node");
    assert_eq!(decision.node_type, NodeType::Conditional);
}

#[test]
fn looping_pipeline_has_loop_restart_edge() {
    let graph = looping_pipeline(3);

    let from_decision = graph.edges_from("decision");
    assert_eq!(from_decision.len(), 2);

    // One edge to exit with condition
    let exit_edge = from_decision
        .iter()
        .find(|e| e.to == "exit")
        .expect("missing edge to exit");
    assert_eq!(exit_edge.condition, Some("loop_done=true".to_string()));
    assert_eq!(exit_edge.priority, Some(10));
    assert!(!exit_edge.loop_restart);

    // One loop_restart edge back to worker
    let loop_edge = from_decision
        .iter()
        .find(|e| e.to == "worker")
        .expect("missing loop edge to worker");
    assert!(loop_edge.loop_restart);
}

#[test]
fn looping_pipeline_has_correct_edge_count() {
    let graph = looping_pipeline(5);
    // start->worker, worker->decision, decision->exit, decision->worker = 4
    assert_eq!(graph.edges.len(), 4);
}

#[test]
fn looping_pipeline_embeds_max_in_name() {
    let graph = looping_pipeline(7);
    assert_eq!(graph.name, Some("looping_pipeline_max_7".to_string()));
}

// ============================================================================
// parallel_pipeline
// ============================================================================

#[test]
fn parallel_pipeline_one_branch() {
    let graph = parallel_pipeline(1);
    // start, parallel, branch_0, exit = 4
    assert_eq!(graph.nodes.len(), 4);
    // start->parallel, parallel->branch_0, branch_0->exit = 3
    assert_eq!(graph.edges.len(), 3);
}

#[test]
fn parallel_pipeline_multiple_branches() {
    let graph = parallel_pipeline(4);
    // start, parallel, branch_0..branch_3, exit = 7
    assert_eq!(graph.nodes.len(), 7);
    // start->parallel, parallel->branch_0..3, branch_0..3->exit = 1 + 4 + 4 = 9
    assert_eq!(graph.edges.len(), 9);
}

#[test]
fn parallel_pipeline_has_start_node() {
    let graph = parallel_pipeline(3);
    let start_nodes = graph.start_nodes();
    assert_eq!(start_nodes.len(), 1);
    assert_eq!(start_nodes[0].id, "start");
}

#[test]
fn parallel_pipeline_has_exit_node() {
    let graph = parallel_pipeline(3);
    let exit_nodes = graph.exit_nodes();
    assert_eq!(exit_nodes.len(), 1);
    assert_eq!(exit_nodes[0].id, "exit");
}

#[test]
fn parallel_pipeline_has_parallel_node() {
    let graph = parallel_pipeline(3);
    let parallel = graph.node("parallel").expect("missing parallel node");
    assert_eq!(parallel.node_type, NodeType::Parallel);
}

#[test]
fn parallel_pipeline_branches_fan_out_from_parallel() {
    let graph = parallel_pipeline(3);
    let from_parallel = graph.edges_from("parallel");
    assert_eq!(from_parallel.len(), 3);

    for i in 0..3 {
        let branch_id = format!("branch_{i}");
        assert!(
            from_parallel.iter().any(|e| e.to == branch_id),
            "missing edge from parallel to {branch_id}"
        );
    }
}

#[test]
fn parallel_pipeline_branches_converge_to_exit() {
    let graph = parallel_pipeline(3);
    let to_exit = graph.edges_to("exit");
    assert_eq!(to_exit.len(), 3);

    for i in 0..3 {
        let branch_id = format!("branch_{i}");
        assert!(
            to_exit.iter().any(|e| e.from == branch_id),
            "missing edge from {branch_id} to exit"
        );
    }
}

#[test]
fn parallel_pipeline_has_name() {
    let graph = parallel_pipeline(2);
    assert_eq!(graph.name, Some("parallel_pipeline".to_string()));
}

#[test]
#[should_panic(expected = "parallel_pipeline requires at least 1 branch")]
fn parallel_pipeline_zero_branches_panics() {
    parallel_pipeline(0);
}

// ============================================================================
// make_test_context
// ============================================================================

#[test]
fn make_test_context_creates_populated_context() {
    let ctx = make_test_context(&[("key1", "value1"), ("key2", "value2")]);
    assert_eq!(ctx.get_string("key1"), Some("value1".to_string()));
    assert_eq!(ctx.get_string("key2"), Some("value2".to_string()));
}

#[test]
fn make_test_context_empty_is_empty() {
    let ctx = make_test_context(&[]);
    assert!(ctx.keys().is_empty());
}

// ============================================================================
// make_counting_handler
// ============================================================================

#[tokio::test]
async fn counting_handler_increments_counter() {
    let (handler, counter) = make_counting_handler("test_counter");

    assert_eq!(handler.name(), "test_counter");
    assert_eq!(counter.load(Ordering::SeqCst), 0);

    // Execute the handler with a dummy node and context
    let node = smasher_attractor::graph::GraphNode {
        id: "test_node".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    let result = handler.execute(&node, &ctx).await.unwrap();
    assert!(result.is_success());
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Verify it stamps the context
    assert_eq!(ctx.get_string("visited_test_node"), None);
    // The value is json!(true) not a string, so get_string won't work.
    // Use get() instead.
    // Actually the first call already stamped it. Let me re-check.
    // The handler sets visited_test_node = json!(true). get_string returns None
    // for booleans. Use get() instead.
    assert_eq!(ctx.get("visited_test_node"), Some(serde_json::json!(true)));
}

#[tokio::test]
async fn counting_handler_handles_all_node_types() {
    let (handler, _) = make_counting_handler("universal");

    assert!(handler.handles(&NodeType::Start));
    assert!(handler.handles(&NodeType::Exit));
    assert!(handler.handles(&NodeType::Generic));
    assert!(handler.handles(&NodeType::Conditional));
    assert!(handler.handles(&NodeType::Codergen));
    assert!(handler.handles(&NodeType::Tool));
    assert!(handler.handles(&NodeType::Parallel));
}

#[tokio::test]
async fn counting_handler_works_with_registry() {
    let (handler, counter) = make_counting_handler("reg_test");

    let mut registry = HandlerRegistry::new();
    registry.register(handler);

    let node = smasher_attractor::graph::GraphNode {
        id: "node_a".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    registry.execute(&node, &ctx).await.unwrap();
    registry.execute(&node, &ctx).await.unwrap();
    registry.execute(&node, &ctx).await.unwrap();

    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

// ============================================================================
// human_gate_pipeline
// ============================================================================

#[test]
fn human_gate_pipeline_has_correct_node_count() {
    let graph = human_gate_pipeline();
    // start, manager, exit_approved, exit_rejected = 4
    assert_eq!(graph.nodes.len(), 4);
}

#[test]
fn human_gate_pipeline_has_start_node() {
    let graph = human_gate_pipeline();
    let start_nodes = graph.start_nodes();
    assert_eq!(start_nodes.len(), 1);
    assert_eq!(start_nodes[0].id, "start");
}

#[test]
fn human_gate_pipeline_has_two_exit_nodes() {
    let graph = human_gate_pipeline();
    let exit_nodes = graph.exit_nodes();
    assert_eq!(exit_nodes.len(), 2);
    let exit_ids: Vec<&str> = exit_nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(exit_ids.contains(&"exit_approved"));
    assert!(exit_ids.contains(&"exit_rejected"));
}

#[test]
fn human_gate_pipeline_has_manager_node() {
    let graph = human_gate_pipeline();
    let manager = graph.node("manager").expect("missing manager node");
    assert_eq!(manager.node_type, NodeType::Manager);
}

#[test]
fn human_gate_pipeline_has_correct_edges() {
    let graph = human_gate_pipeline();
    // 3 edges: start->manager, manager->exit_approved, manager->exit_rejected
    assert_eq!(graph.edges.len(), 3);

    let from_start = graph.edges_from("start");
    assert_eq!(from_start.len(), 1);
    assert_eq!(from_start[0].to, "manager");

    let from_manager = graph.edges_from("manager");
    assert_eq!(from_manager.len(), 2);

    let to_approved = from_manager
        .iter()
        .find(|e| e.to == "exit_approved")
        .expect("missing edge to exit_approved");
    assert_eq!(to_approved.condition, Some("approved=true".to_string()));
    assert_eq!(to_approved.label, Some("approved".to_string()));
    assert_eq!(to_approved.priority, Some(1));

    let to_rejected = from_manager
        .iter()
        .find(|e| e.to == "exit_rejected")
        .expect("missing edge to exit_rejected");
    assert_eq!(to_rejected.condition, Some("approved=false".to_string()));
    assert_eq!(to_rejected.label, Some("rejected".to_string()));
    assert_eq!(to_rejected.priority, Some(2));
}

#[test]
fn human_gate_pipeline_has_name() {
    let graph = human_gate_pipeline();
    assert_eq!(graph.name, Some("human_gate_pipeline".to_string()));
}

// ============================================================================
// codergen_pipeline
// ============================================================================

#[test]
fn codergen_pipeline_has_correct_node_count() {
    let graph = codergen_pipeline();
    // start, plan, generate, review, exit = 5
    assert_eq!(graph.nodes.len(), 5);
}

#[test]
fn codergen_pipeline_has_start_node() {
    let graph = codergen_pipeline();
    let start_nodes = graph.start_nodes();
    assert_eq!(start_nodes.len(), 1);
    assert_eq!(start_nodes[0].id, "start");
}

#[test]
fn codergen_pipeline_has_exit_node() {
    let graph = codergen_pipeline();
    let exit_nodes = graph.exit_nodes();
    assert_eq!(exit_nodes.len(), 1);
    assert_eq!(exit_nodes[0].id, "exit");
}

#[test]
fn codergen_pipeline_has_codergen_nodes() {
    let graph = codergen_pipeline();

    let plan = graph.node("plan").expect("missing plan node");
    assert_eq!(plan.node_type, NodeType::Codergen);

    let generate = graph.node("generate").expect("missing generate node");
    assert_eq!(generate.node_type, NodeType::Codergen);
}

#[test]
fn codergen_pipeline_has_conditional_review() {
    let graph = codergen_pipeline();
    let review = graph.node("review").expect("missing review node");
    assert_eq!(review.node_type, NodeType::Conditional);
}

#[test]
fn codergen_pipeline_has_correct_edges() {
    let graph = codergen_pipeline();
    // 4 edges: start->plan, plan->generate, generate->review, review->exit
    assert_eq!(graph.edges.len(), 4);

    // Verify the chain: start -> plan -> generate -> review -> exit
    let from_start = graph.edges_from("start");
    assert_eq!(from_start.len(), 1);
    assert_eq!(from_start[0].to, "plan");

    let from_plan = graph.edges_from("plan");
    assert_eq!(from_plan.len(), 1);
    assert_eq!(from_plan[0].to, "generate");

    let from_generate = graph.edges_from("generate");
    assert_eq!(from_generate.len(), 1);
    assert_eq!(from_generate[0].to, "review");

    let from_review = graph.edges_from("review");
    assert_eq!(from_review.len(), 1);
    assert_eq!(from_review[0].to, "exit");
    assert_eq!(
        from_review[0].condition,
        Some("review_passed=true".to_string())
    );
    assert_eq!(from_review[0].label, Some("passed".to_string()));
    assert_eq!(from_review[0].priority, Some(1));
}

#[test]
fn codergen_pipeline_has_name() {
    let graph = codergen_pipeline();
    assert_eq!(graph.name, Some("codergen_pipeline".to_string()));
}

// ============================================================================
// sub_pipeline_parent
// ============================================================================

#[test]
fn sub_pipeline_parent_has_correct_node_count() {
    let graph = sub_pipeline_parent();
    // start, sub, exit = 3
    assert_eq!(graph.nodes.len(), 3);
}

#[test]
fn sub_pipeline_parent_has_start_node() {
    let graph = sub_pipeline_parent();
    let start_nodes = graph.start_nodes();
    assert_eq!(start_nodes.len(), 1);
    assert_eq!(start_nodes[0].id, "start");
}

#[test]
fn sub_pipeline_parent_has_exit_node() {
    let graph = sub_pipeline_parent();
    let exit_nodes = graph.exit_nodes();
    assert_eq!(exit_nodes.len(), 1);
    assert_eq!(exit_nodes[0].id, "exit");
}

#[test]
fn sub_pipeline_parent_has_sub_pipeline_node() {
    let graph = sub_pipeline_parent();
    let sub = graph.node("sub").expect("missing sub node");
    assert_eq!(sub.node_type, NodeType::SubPipeline);
}

#[test]
fn sub_pipeline_parent_has_correct_edges() {
    let graph = sub_pipeline_parent();
    // 2 edges: start->sub, sub->exit
    assert_eq!(graph.edges.len(), 2);

    let from_start = graph.edges_from("start");
    assert_eq!(from_start.len(), 1);
    assert_eq!(from_start[0].to, "sub");

    let from_sub = graph.edges_from("sub");
    assert_eq!(from_sub.len(), 1);
    assert_eq!(from_sub[0].to, "exit");
}

#[test]
fn sub_pipeline_parent_has_name() {
    let graph = sub_pipeline_parent();
    assert_eq!(graph.name, Some("sub_pipeline_parent".to_string()));
}

// ============================================================================
// complex_pipeline
// ============================================================================

#[test]
fn complex_pipeline_has_correct_node_count() {
    let graph = complex_pipeline();
    // start, conditional, step_a1, step_a2, exit_a, manager_b, exit_b = 7
    assert_eq!(graph.nodes.len(), 7);
}

#[test]
fn complex_pipeline_has_start_node() {
    let graph = complex_pipeline();
    let start_nodes = graph.start_nodes();
    assert_eq!(start_nodes.len(), 1);
    assert_eq!(start_nodes[0].id, "start");
}

#[test]
fn complex_pipeline_has_two_exit_nodes() {
    let graph = complex_pipeline();
    let exit_nodes = graph.exit_nodes();
    assert_eq!(exit_nodes.len(), 2);
    let exit_ids: Vec<&str> = exit_nodes.iter().map(|n| n.id.as_str()).collect();
    assert!(exit_ids.contains(&"exit_a"));
    assert!(exit_ids.contains(&"exit_b"));
}

#[test]
fn complex_pipeline_has_conditional_node() {
    let graph = complex_pipeline();
    let cond = graph.node("conditional").expect("missing conditional node");
    assert_eq!(cond.node_type, NodeType::Conditional);
}

#[test]
fn complex_pipeline_has_codergen_nodes_on_branch_a() {
    let graph = complex_pipeline();

    let step_a1 = graph.node("step_a1").expect("missing step_a1 node");
    assert_eq!(step_a1.node_type, NodeType::Codergen);

    let step_a2 = graph.node("step_a2").expect("missing step_a2 node");
    assert_eq!(step_a2.node_type, NodeType::Codergen);
}

#[test]
fn complex_pipeline_has_manager_node_on_branch_b() {
    let graph = complex_pipeline();
    let manager_b = graph.node("manager_b").expect("missing manager_b node");
    assert_eq!(manager_b.node_type, NodeType::Manager);
}

#[test]
fn complex_pipeline_branch_a_edges() {
    let graph = complex_pipeline();

    let from_conditional = graph.edges_from("conditional");
    assert_eq!(from_conditional.len(), 2);

    let to_a1 = from_conditional
        .iter()
        .find(|e| e.to == "step_a1")
        .expect("missing edge to step_a1");
    assert_eq!(to_a1.condition, Some("path=a".to_string()));
    assert_eq!(to_a1.label, Some("branch_a".to_string()));
    assert_eq!(to_a1.priority, Some(1));

    // step_a1 -> step_a2 -> exit_a
    let from_a1 = graph.edges_from("step_a1");
    assert_eq!(from_a1.len(), 1);
    assert_eq!(from_a1[0].to, "step_a2");

    let from_a2 = graph.edges_from("step_a2");
    assert_eq!(from_a2.len(), 1);
    assert_eq!(from_a2[0].to, "exit_a");
}

#[test]
fn complex_pipeline_branch_b_edges() {
    let graph = complex_pipeline();

    let from_conditional = graph.edges_from("conditional");
    let to_manager = from_conditional
        .iter()
        .find(|e| e.to == "manager_b")
        .expect("missing edge to manager_b");
    assert_eq!(to_manager.condition, Some("path=b".to_string()));
    assert_eq!(to_manager.label, Some("branch_b".to_string()));
    assert_eq!(to_manager.priority, Some(2));

    // manager_b -> exit_b
    let from_manager = graph.edges_from("manager_b");
    assert_eq!(from_manager.len(), 1);
    assert_eq!(from_manager[0].to, "exit_b");
}

#[test]
fn complex_pipeline_has_correct_edge_count() {
    let graph = complex_pipeline();
    // start->conditional, conditional->step_a1, step_a1->step_a2, step_a2->exit_a,
    // conditional->manager_b, manager_b->exit_b = 6
    assert_eq!(graph.edges.len(), 6);
}

#[test]
fn complex_pipeline_has_name() {
    let graph = complex_pipeline();
    assert_eq!(graph.name, Some("complex_pipeline".to_string()));
}

// ============================================================================
// make_failing_handler
// ============================================================================

#[tokio::test]
async fn failing_handler_fails_on_target_node() {
    let handler = make_failing_handler("bad_node");

    let node = smasher_attractor::graph::GraphNode {
        id: "bad_node".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    let result = handler.execute(&node, &ctx).await;
    assert!(result.is_err());

    match result.unwrap_err() {
        HandlerError::ExecutionFailed {
            handler: h,
            node_id,
            message,
        } => {
            assert_eq!(h, "failing_handler");
            assert_eq!(node_id, "bad_node");
            assert!(message.contains("deliberate failure"));
        }
        other => panic!("expected ExecutionFailed, got: {other:?}"),
    }
}

#[tokio::test]
async fn failing_handler_succeeds_on_other_nodes() {
    let handler = make_failing_handler("bad_node");

    let node = smasher_attractor::graph::GraphNode {
        id: "good_node".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    let result = handler.execute(&node, &ctx).await.unwrap();
    assert!(result.is_success());
}

#[tokio::test]
async fn failing_handler_handles_all_node_types() {
    let handler = make_failing_handler("any");

    assert!(handler.handles(&NodeType::Start));
    assert!(handler.handles(&NodeType::Exit));
    assert!(handler.handles(&NodeType::Generic));
    assert!(handler.handles(&NodeType::Conditional));
    assert!(handler.handles(&NodeType::Codergen));
    assert!(handler.handles(&NodeType::Tool));
    assert!(handler.handles(&NodeType::Manager));
    assert!(handler.handles(&NodeType::SubPipeline));
}

#[tokio::test]
async fn failing_handler_works_with_registry() {
    let handler = make_failing_handler("target");

    let mut registry = HandlerRegistry::new();
    registry.register(handler);

    let node = smasher_attractor::graph::GraphNode {
        id: "target".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    let result = registry.execute(&node, &ctx).await;
    assert!(result.is_err());
}

// ============================================================================
// make_outcome_handler
// ============================================================================

#[tokio::test]
async fn outcome_handler_returns_success() {
    let handler = make_outcome_handler(Outcome::success());

    let node = smasher_attractor::graph::GraphNode {
        id: "any_node".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    let result = handler.execute(&node, &ctx).await.unwrap();
    assert!(result.is_success());
}

#[tokio::test]
async fn outcome_handler_returns_failure() {
    let handler = make_outcome_handler(Outcome::failure("test error"));

    let node = smasher_attractor::graph::GraphNode {
        id: "any_node".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    let result = handler.execute(&node, &ctx).await.unwrap();
    assert!(result.is_failure());
    match result {
        Outcome::Failure { error, retryable } => {
            assert_eq!(error, "test error");
            assert!(!retryable);
        }
        other => panic!("expected Failure, got: {other:?}"),
    }
}

#[tokio::test]
async fn outcome_handler_returns_retryable_failure() {
    let handler = make_outcome_handler(Outcome::retryable_failure("transient"));

    let node = smasher_attractor::graph::GraphNode {
        id: "any_node".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    let result = handler.execute(&node, &ctx).await.unwrap();
    assert!(result.is_retryable());
}

#[tokio::test]
async fn outcome_handler_returns_skip() {
    let handler = make_outcome_handler(Outcome::skip("not applicable"));

    let node = smasher_attractor::graph::GraphNode {
        id: "any_node".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    let result = handler.execute(&node, &ctx).await.unwrap();
    match result {
        Outcome::Skip { reason } => {
            assert_eq!(reason, "not applicable");
        }
        other => panic!("expected Skip, got: {other:?}"),
    }
}

#[tokio::test]
async fn outcome_handler_returns_success_with_data() {
    let data = serde_json::json!({"key": "value"});
    let handler = make_outcome_handler(Outcome::success_with(data.clone()));

    let node = smasher_attractor::graph::GraphNode {
        id: "any_node".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    let result = handler.execute(&node, &ctx).await.unwrap();
    match result {
        Outcome::Success { data: Some(d) } => {
            assert_eq!(d, data);
        }
        other => panic!("expected Success with data, got: {other:?}"),
    }
}

#[tokio::test]
async fn outcome_handler_handles_all_node_types() {
    let handler = make_outcome_handler(Outcome::success());

    assert!(handler.handles(&NodeType::Start));
    assert!(handler.handles(&NodeType::Exit));
    assert!(handler.handles(&NodeType::Generic));
    assert!(handler.handles(&NodeType::Conditional));
    assert!(handler.handles(&NodeType::Codergen));
    assert!(handler.handles(&NodeType::Tool));
    assert!(handler.handles(&NodeType::Manager));
    assert!(handler.handles(&NodeType::SubPipeline));
}

// ============================================================================
// make_delayed_handler
// ============================================================================

#[tokio::test]
async fn delayed_handler_succeeds_after_delay() {
    let handler = make_delayed_handler(10);

    let node = smasher_attractor::graph::GraphNode {
        id: "delayed_node".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    let start = std::time::Instant::now();
    let result = handler.execute(&node, &ctx).await.unwrap();
    let elapsed = start.elapsed();

    assert!(result.is_success());
    assert!(
        elapsed >= std::time::Duration::from_millis(10),
        "handler should have delayed at least 10ms, took {:?}",
        elapsed
    );
}

#[tokio::test]
async fn delayed_handler_stamps_context() {
    let handler = make_delayed_handler(1);

    let node = smasher_attractor::graph::GraphNode {
        id: "stamp_node".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    handler.execute(&node, &ctx).await.unwrap();
    assert_eq!(ctx.get("visited_stamp_node"), Some(serde_json::json!(true)));
}

#[tokio::test]
async fn delayed_handler_returns_data_with_node_and_delay() {
    let handler = make_delayed_handler(5);

    let node = smasher_attractor::graph::GraphNode {
        id: "data_node".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    let result = handler.execute(&node, &ctx).await.unwrap();
    match result {
        Outcome::Success { data: Some(d) } => {
            assert_eq!(d["node"], "data_node");
            assert_eq!(d["delayed_ms"], 5);
        }
        other => panic!("expected Success with data, got: {other:?}"),
    }
}

#[tokio::test]
async fn delayed_handler_handles_all_node_types() {
    let handler = make_delayed_handler(1);

    assert!(handler.handles(&NodeType::Start));
    assert!(handler.handles(&NodeType::Exit));
    assert!(handler.handles(&NodeType::Generic));
    assert!(handler.handles(&NodeType::Conditional));
    assert!(handler.handles(&NodeType::Codergen));
    assert!(handler.handles(&NodeType::Tool));
    assert!(handler.handles(&NodeType::Manager));
    assert!(handler.handles(&NodeType::SubPipeline));
}

#[tokio::test]
async fn delayed_handler_zero_delay_still_succeeds() {
    let handler = make_delayed_handler(0);

    let node = smasher_attractor::graph::GraphNode {
        id: "zero_delay".to_string(),
        node_type: NodeType::Generic,
        label: None,
        attrs: std::collections::HashMap::new(),
    };
    let ctx = smasher_attractor::state::Context::new();

    let result = handler.execute(&node, &ctx).await.unwrap();
    assert!(result.is_success());
    assert_eq!(ctx.get("visited_zero_delay"), Some(serde_json::json!(true)));
}
