// ABOUTME: Integration tests verifying that all example DOT files parse and resolve correctly.
// ABOUTME: Catches syntax errors in example pipelines and validates structural expectations.

use std::path::Path;

use smasher_attractor::dot::parser;
use smasher_attractor::graph::{self, NodeType};

/// Helper: read, parse, and resolve a DOT file from the examples directory.
fn load_example(filename: &str) -> graph::Graph {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let path = workspace_root.join("examples").join(filename);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let dot = parser::parse(&source)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
    graph::resolve(&dot).unwrap_or_else(|e| panic!("failed to resolve {}: {e}", path.display()))
}

// ============================================================================
// human-gate.dot
// ============================================================================

#[test]
fn human_gate_parses_and_resolves() {
    let g = load_example("human-gate.dot");
    assert!(!g.nodes.is_empty(), "human-gate should have nodes");
    assert!(!g.edges.is_empty(), "human-gate should have edges");
}

#[test]
fn human_gate_has_start_and_exit() {
    let g = load_example("human-gate.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(
        !g.exit_nodes().is_empty(),
        "should have at least one exit node"
    );
}

#[test]
fn human_gate_has_manager_node() {
    let g = load_example("human-gate.dot");
    let gate = g.node("human_gate").expect("missing human_gate node");
    assert_eq!(gate.node_type, NodeType::Manager);
}

#[test]
fn human_gate_has_question_attribute() {
    let g = load_example("human-gate.dot");
    let gate = g.node("human_gate").expect("missing human_gate node");
    assert!(
        gate.attrs.contains_key("question"),
        "human_gate should have a question attribute"
    );
}

// ============================================================================
// codergen.dot
// ============================================================================

#[test]
fn codergen_parses_and_resolves() {
    let g = load_example("codergen.dot");
    assert!(!g.nodes.is_empty(), "codergen should have nodes");
    assert!(!g.edges.is_empty(), "codergen should have edges");
}

#[test]
fn codergen_has_start_and_exit() {
    let g = load_example("codergen.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(
        !g.exit_nodes().is_empty(),
        "should have at least one exit node"
    );
}

#[test]
fn codergen_has_codergen_nodes() {
    let g = load_example("codergen.dot");
    let plan = g.node("plan").expect("missing plan node");
    assert_eq!(plan.node_type, NodeType::Codergen);

    let generate = g.node("generate").expect("missing generate node");
    assert_eq!(generate.node_type, NodeType::Codergen);
}

#[test]
fn codergen_has_model_attributes() {
    let g = load_example("codergen.dot");
    let plan = g.node("plan").expect("missing plan node");
    assert!(
        plan.attrs.contains_key("model"),
        "plan should have a model attribute"
    );

    let generate = g.node("generate").expect("missing generate node");
    assert!(
        generate.attrs.contains_key("model"),
        "generate should have a model attribute"
    );
}

#[test]
fn codergen_has_conditional_review() {
    let g = load_example("codergen.dot");
    let review = g.node("review").expect("missing review node");
    assert_eq!(review.node_type, NodeType::Conditional);
}

// ============================================================================
// loop-with-exit.dot
// ============================================================================

#[test]
fn loop_with_exit_parses_and_resolves() {
    let g = load_example("loop-with-exit.dot");
    assert!(!g.nodes.is_empty(), "loop-with-exit should have nodes");
    assert!(!g.edges.is_empty(), "loop-with-exit should have edges");
}

#[test]
fn loop_with_exit_has_start_and_exit() {
    let g = load_example("loop-with-exit.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(
        !g.exit_nodes().is_empty(),
        "should have at least one exit node"
    );
}

#[test]
fn loop_with_exit_has_loop_restart_edge() {
    let g = load_example("loop-with-exit.dot");
    let loop_edges: Vec<_> = g.edges.iter().filter(|e| e.loop_restart).collect();
    assert_eq!(
        loop_edges.len(),
        1,
        "should have exactly one loop_restart edge"
    );
    assert_eq!(loop_edges[0].from, "check");
    assert_eq!(loop_edges[0].to, "process");
}

#[test]
fn loop_with_exit_has_conditional_check() {
    let g = load_example("loop-with-exit.dot");
    let check = g.node("check").expect("missing check node");
    assert_eq!(check.node_type, NodeType::Conditional);
}

// ============================================================================
// multi-gate.dot
// ============================================================================

#[test]
fn multi_gate_parses_and_resolves() {
    let g = load_example("multi-gate.dot");
    assert!(!g.nodes.is_empty(), "multi-gate should have nodes");
    assert!(!g.edges.is_empty(), "multi-gate should have edges");
}

#[test]
fn multi_gate_has_start_and_exit() {
    let g = load_example("multi-gate.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(
        !g.exit_nodes().is_empty(),
        "should have at least one exit node"
    );
}

#[test]
fn multi_gate_has_two_manager_nodes() {
    let g = load_example("multi-gate.dot");

    let security = g
        .node("security_check")
        .expect("missing security_check node");
    assert_eq!(security.node_type, NodeType::Manager);

    let compliance = g
        .node("compliance_check")
        .expect("missing compliance_check node");
    assert_eq!(compliance.node_type, NodeType::Manager);
}

#[test]
fn multi_gate_managers_are_sequential() {
    let g = load_example("multi-gate.dot");

    // security_check should have an edge to compliance_check
    let from_security = g.edges_from("security_check");
    assert!(
        from_security.iter().any(|e| e.to == "compliance_check"),
        "security_check should have an edge to compliance_check"
    );
}

#[test]
fn multi_gate_managers_have_question_attributes() {
    let g = load_example("multi-gate.dot");

    let security = g
        .node("security_check")
        .expect("missing security_check node");
    assert!(
        security.attrs.contains_key("question"),
        "security_check should have a question attribute"
    );

    let compliance = g
        .node("compliance_check")
        .expect("missing compliance_check node");
    assert!(
        compliance.attrs.contains_key("question"),
        "compliance_check should have a question attribute"
    );
}

// ============================================================================
// Existing examples still parse (regression guard)
// ============================================================================

#[test]
fn hello_world_parses_and_resolves() {
    let g = load_example("hello-world.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert_eq!(g.exit_nodes().len(), 1);
}

#[test]
fn conditional_parses_and_resolves() {
    let g = load_example("conditional.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(g.exit_nodes().len() >= 2);
}

#[test]
fn multi_step_parses_and_resolves() {
    let g = load_example("multi-step.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(g.exit_nodes().len() >= 2);
}

#[test]
fn parallel_fanout_parses_and_resolves() {
    let g = load_example("parallel-fanout.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert_eq!(g.exit_nodes().len(), 1);
}

#[test]
fn retry_loop_parses_and_resolves() {
    let g = load_example("retry-loop.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(!g.exit_nodes().is_empty());
}
