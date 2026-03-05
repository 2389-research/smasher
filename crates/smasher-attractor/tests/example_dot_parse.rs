// ABOUTME: Integration tests verifying that all example DOT files parse and resolve correctly.
// ABOUTME: Catches syntax errors in example pipelines and validates structural expectations.

use std::path::Path;

use smasher_attractor::dot::parser;
use smasher_attractor::graph::{self, NodeType};

/// Helper: read, parse, and resolve a DOT file from the examples directory.
fn load_example(filename: &str) -> graph::Graph {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let path = workspace_root.join("examples").join(filename);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let dot = parser::parse(&source)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
    graph::resolve(&dot).unwrap_or_else(|e| panic!("failed to resolve {}: {e}", path.display()))
}

// ============================================================================
// consensus_task.dot
// ============================================================================

#[test]
fn consensus_task_parses_and_resolves() {
    let g = load_example("consensus_task.dot");
    assert!(!g.nodes.is_empty());
    assert!(!g.edges.is_empty());
}

#[test]
fn consensus_task_has_start_and_exit() {
    let g = load_example("consensus_task.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(!g.exit_nodes().is_empty());
}

#[test]
fn consensus_task_has_review_consensus_node() {
    let g = load_example("consensus_task.dot");
    let node = g.node("ReviewConsensus").expect("missing ReviewConsensus");
    assert_eq!(node.node_type, NodeType::Codergen);
}

#[test]
fn consensus_task_has_loop_restart() {
    let g = load_example("consensus_task.dot");
    let loop_edges: Vec<_> = g.edges.iter().filter(|e| e.loop_restart).collect();
    assert!(
        !loop_edges.is_empty(),
        "consensus_task should have at least one loop_restart edge"
    );
}

#[test]
fn consensus_task_has_conditional_edges() {
    let g = load_example("consensus_task.dot");
    let cond_edges: Vec<_> = g.edges.iter().filter(|e| e.condition.is_some()).collect();
    assert!(
        cond_edges.len() >= 2,
        "consensus_task should have conditional edges"
    );
}

// ============================================================================
// consensus_task_parity.dot
// ============================================================================

#[test]
fn consensus_task_parity_parses_and_resolves() {
    let g = load_example("consensus_task_parity.dot");
    assert!(!g.nodes.is_empty());
    assert!(!g.edges.is_empty());
}

#[test]
fn consensus_task_parity_has_start_and_exit() {
    let g = load_example("consensus_task_parity.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(!g.exit_nodes().is_empty());
}

#[test]
fn consensus_task_parity_has_parallel_fanout() {
    let g = load_example("consensus_task_parity.dot");
    let parallel_nodes: Vec<_> = g
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::Parallel)
        .collect();
    assert!(
        !parallel_nodes.is_empty(),
        "parity variant should have Parallel (component) nodes"
    );
}

#[test]
fn consensus_task_parity_has_fanin_joins() {
    let g = load_example("consensus_task_parity.dot");
    let fanin_nodes: Vec<_> = g
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::FanIn)
        .collect();
    assert!(
        !fanin_nodes.is_empty(),
        "parity variant should have FanIn (tripleoctagon) nodes"
    );
}

// ============================================================================
// megaplan.dot
// ============================================================================

#[test]
fn megaplan_parses_and_resolves() {
    let g = load_example("megaplan.dot");
    assert!(!g.nodes.is_empty());
    assert!(!g.edges.is_empty());
}

#[test]
fn megaplan_has_start_and_exit() {
    let g = load_example("megaplan.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(!g.exit_nodes().is_empty());
}

#[test]
fn megaplan_has_interview_gate() {
    let g = load_example("megaplan.dot");
    let gate = g.node("InterviewGate").expect("missing InterviewGate");
    assert_eq!(gate.node_type, NodeType::Interviewer);
}

#[test]
fn megaplan_has_parallel_critique_fanout() {
    let g = load_example("megaplan.dot");
    let parallel_nodes: Vec<_> = g
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::Parallel)
        .collect();
    assert!(
        parallel_nodes.len() >= 3,
        "megaplan should have multiple parallel fan-out nodes, got {}",
        parallel_nodes.len()
    );
}

#[test]
fn megaplan_is_largest_example() {
    let g = load_example("megaplan.dot");
    assert!(
        g.nodes.len() >= 40,
        "megaplan should have 40+ nodes, got {}",
        g.nodes.len()
    );
}

// ============================================================================
// megaplan_quality.dot
// ============================================================================

#[test]
fn megaplan_quality_parses_and_resolves() {
    let g = load_example("megaplan_quality.dot");
    assert!(!g.nodes.is_empty());
    assert!(!g.edges.is_empty());
}

#[test]
fn megaplan_quality_has_start_and_exit() {
    let g = load_example("megaplan_quality.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(!g.exit_nodes().is_empty());
}

#[test]
fn megaplan_quality_has_goal_gate() {
    let g = load_example("megaplan_quality.dot");
    let gate = g
        .node("FinalQualityGate")
        .expect("missing FinalQualityGate");
    assert!(
        gate.attrs.contains_key("goal_gate"),
        "FinalQualityGate should have goal_gate attribute"
    );
}

// ============================================================================
// semport.dot
// ============================================================================

#[test]
fn semport_parses_and_resolves() {
    let g = load_example("semport.dot");
    assert!(!g.nodes.is_empty());
    assert!(!g.edges.is_empty());
}

#[test]
fn semport_has_start_and_exit() {
    let g = load_example("semport.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(!g.exit_nodes().is_empty());
}

#[test]
fn semport_has_two_loop_restarts() {
    let g = load_example("semport.dot");
    let loop_edges: Vec<_> = g.edges.iter().filter(|e| e.loop_restart).collect();
    assert_eq!(
        loop_edges.len(),
        2,
        "semport has two loop phases (port loop + fix loop)"
    );
}

#[test]
fn semport_has_tool_nodes() {
    let g = load_example("semport.dot");
    let tool_nodes: Vec<_> = g
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::Tool)
        .collect();
    assert!(
        tool_nodes.len() >= 5,
        "semport should have several tool (parallelogram) nodes, got {}",
        tool_nodes.len()
    );
}

// ============================================================================
// semport_thematic.dot
// ============================================================================

#[test]
fn semport_thematic_parses_and_resolves() {
    let g = load_example("semport_thematic.dot");
    assert!(!g.nodes.is_empty());
    assert!(!g.edges.is_empty());
}

#[test]
fn semport_thematic_has_start_and_exit() {
    let g = load_example("semport_thematic.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(!g.exit_nodes().is_empty());
}

#[test]
fn semport_thematic_has_parallel_and_fanin() {
    let g = load_example("semport_thematic.dot");
    let has_parallel = g.nodes.iter().any(|n| n.node_type == NodeType::Parallel);
    let has_fanin = g.nodes.iter().any(|n| n.node_type == NodeType::FanIn);
    assert!(has_parallel, "should have Parallel nodes");
    assert!(has_fanin, "should have FanIn nodes");
}

// ============================================================================
// sprint_exec.dot
// ============================================================================

#[test]
fn sprint_exec_parses_and_resolves() {
    let g = load_example("sprint_exec.dot");
    assert!(!g.nodes.is_empty());
    assert!(!g.edges.is_empty());
}

#[test]
fn sprint_exec_has_start_and_exit() {
    let g = load_example("sprint_exec.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(!g.exit_nodes().is_empty());
}

#[test]
fn sprint_exec_has_review_analysis_with_retry() {
    let g = load_example("sprint_exec.dot");
    let node = g.node("ReviewAnalysis").expect("missing ReviewAnalysis");
    assert_eq!(node.node_type, NodeType::Codergen);
    assert!(
        node.attrs.contains_key("goal_gate"),
        "ReviewAnalysis should be a goal_gate"
    );
}

#[test]
fn sprint_exec_has_multi_model_reviews() {
    let g = load_example("sprint_exec.dot");
    assert!(g.node("ReviewClaude").is_some());
    assert!(g.node("ReviewCodex").is_some());
    assert!(g.node("ReviewGemini").is_some());
}

#[test]
fn sprint_exec_has_cross_model_critiques() {
    let g = load_example("sprint_exec.dot");
    assert!(g.node("CritiqueClaudeOnCodex").is_some());
    assert!(g.node("CritiqueCodexOnClaude").is_some());
    assert!(g.node("CritiqueGeminiOnClaude").is_some());
}

// ============================================================================
// vulnerability_analyzer.dot
// ============================================================================

#[test]
fn vulnerability_analyzer_parses_and_resolves() {
    let g = load_example("vulnerability_analyzer.dot");
    assert!(!g.nodes.is_empty());
    assert!(!g.edges.is_empty());
}

#[test]
fn vulnerability_analyzer_has_start_and_exit() {
    let g = load_example("vulnerability_analyzer.dot");
    assert_eq!(g.start_nodes().len(), 1);
    assert!(!g.exit_nodes().is_empty());
}

#[test]
fn vulnerability_analyzer_is_tool_only() {
    let g = load_example("vulnerability_analyzer.dot");
    let non_terminal: Vec<_> = g
        .nodes
        .iter()
        .filter(|n| n.node_type != NodeType::Start && n.node_type != NodeType::Exit)
        .collect();
    assert!(
        non_terminal.iter().all(|n| n.node_type == NodeType::Tool),
        "vulnerability_analyzer should be a pure tool pipeline (no LLM nodes)"
    );
}

#[test]
fn vulnerability_analyzer_has_conditional_findings_check() {
    let g = load_example("vulnerability_analyzer.dot");
    let cond_edges: Vec<_> = g
        .edges
        .iter()
        .filter(|e| e.condition.is_some() && e.from == "EvaluateFindings")
        .collect();
    assert_eq!(
        cond_edges.len(),
        2,
        "EvaluateFindings should branch on findings vs no_findings"
    );
}
