// ABOUTME: Integration tests validating all example DOT pipelines against the lint framework.
// ABOUTME: Ensures examples are structurally sound and tests lint rules against known-bad DOT.

use std::collections::{HashSet, VecDeque};
use std::path::Path;

use smasher_attractor::dot::parser;
use smasher_attractor::graph::{self, Graph, NodeType};
use smasher_attractor::lint::{LintRunner, Severity};

/// Helper: read, parse, and resolve a DOT file from the examples directory.
fn load_example(filename: &str) -> Graph {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let path = workspace_root.join("examples").join(filename);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let dot = parser::parse(&source)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));
    graph::resolve(&dot).unwrap_or_else(|e| panic!("failed to resolve {}: {e}", path.display()))
}

/// Helper: collect all node IDs reachable from start nodes via BFS.
fn reachable_from_start(graph: &Graph) -> HashSet<&str> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    for start in graph.start_nodes() {
        queue.push_back(start.id.as_str());
        visited.insert(start.id.as_str());
    }

    while let Some(current) = queue.pop_front() {
        for successor in graph.successors(current) {
            if visited.insert(successor) {
                queue.push_back(successor);
            }
        }
    }

    visited
}

// ============================================================================
// Per-example lint validation: no Error-level diagnostics
// ============================================================================

#[test]
fn hello_world_passes_lint() {
    let g = load_example("hello-world.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "hello-world.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn conditional_passes_lint() {
    let g = load_example("conditional.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "conditional.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn multi_step_passes_lint() {
    let g = load_example("multi-step.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "multi-step.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn retry_loop_passes_lint() {
    let g = load_example("retry-loop.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "retry-loop.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn parallel_fanout_passes_lint() {
    let g = load_example("parallel-fanout.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "parallel-fanout.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn human_gate_passes_lint() {
    let g = load_example("human-gate.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "human-gate.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn codergen_passes_lint() {
    let g = load_example("codergen.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "codergen.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn loop_with_exit_passes_lint() {
    let g = load_example("loop-with-exit.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "loop-with-exit.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn multi_gate_passes_lint() {
    let g = load_example("multi-gate.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "multi-gate.dot has lint errors: {:?}",
        report.errors()
    );
}

// ============================================================================
// Structural validation: start nodes
// ============================================================================

#[test]
fn hello_world_has_exactly_one_start() {
    let g = load_example("hello-world.dot");
    assert_eq!(
        g.start_nodes().len(),
        1,
        "hello-world.dot should have exactly 1 start node"
    );
}

#[test]
fn conditional_has_exactly_one_start() {
    let g = load_example("conditional.dot");
    assert_eq!(
        g.start_nodes().len(),
        1,
        "conditional.dot should have exactly 1 start node"
    );
}

#[test]
fn multi_step_has_exactly_one_start() {
    let g = load_example("multi-step.dot");
    assert_eq!(
        g.start_nodes().len(),
        1,
        "multi-step.dot should have exactly 1 start node"
    );
}

#[test]
fn retry_loop_has_exactly_one_start() {
    let g = load_example("retry-loop.dot");
    assert_eq!(
        g.start_nodes().len(),
        1,
        "retry-loop.dot should have exactly 1 start node"
    );
}

#[test]
fn parallel_fanout_has_exactly_one_start() {
    let g = load_example("parallel-fanout.dot");
    assert_eq!(
        g.start_nodes().len(),
        1,
        "parallel-fanout.dot should have exactly 1 start node"
    );
}

#[test]
fn human_gate_has_exactly_one_start() {
    let g = load_example("human-gate.dot");
    assert_eq!(
        g.start_nodes().len(),
        1,
        "human-gate.dot should have exactly 1 start node"
    );
}

#[test]
fn codergen_has_exactly_one_start() {
    let g = load_example("codergen.dot");
    assert_eq!(
        g.start_nodes().len(),
        1,
        "codergen.dot should have exactly 1 start node"
    );
}

#[test]
fn loop_with_exit_has_exactly_one_start() {
    let g = load_example("loop-with-exit.dot");
    assert_eq!(
        g.start_nodes().len(),
        1,
        "loop-with-exit.dot should have exactly 1 start node"
    );
}

#[test]
fn multi_gate_has_exactly_one_start() {
    let g = load_example("multi-gate.dot");
    assert_eq!(
        g.start_nodes().len(),
        1,
        "multi-gate.dot should have exactly 1 start node"
    );
}

// ============================================================================
// Structural validation: exit nodes
// ============================================================================

#[test]
fn hello_world_has_at_least_one_exit() {
    let g = load_example("hello-world.dot");
    assert!(
        !g.exit_nodes().is_empty(),
        "hello-world.dot should have at least 1 exit node"
    );
}

#[test]
fn conditional_has_at_least_one_exit() {
    let g = load_example("conditional.dot");
    assert!(
        !g.exit_nodes().is_empty(),
        "conditional.dot should have at least 1 exit node"
    );
}

#[test]
fn multi_step_has_at_least_one_exit() {
    let g = load_example("multi-step.dot");
    assert!(
        !g.exit_nodes().is_empty(),
        "multi-step.dot should have at least 1 exit node"
    );
}

#[test]
fn retry_loop_has_at_least_one_exit() {
    let g = load_example("retry-loop.dot");
    assert!(
        !g.exit_nodes().is_empty(),
        "retry-loop.dot should have at least 1 exit node"
    );
}

#[test]
fn parallel_fanout_has_at_least_one_exit() {
    let g = load_example("parallel-fanout.dot");
    assert!(
        !g.exit_nodes().is_empty(),
        "parallel-fanout.dot should have at least 1 exit node"
    );
}

#[test]
fn human_gate_has_at_least_one_exit() {
    let g = load_example("human-gate.dot");
    assert!(
        !g.exit_nodes().is_empty(),
        "human-gate.dot should have at least 1 exit node"
    );
}

#[test]
fn codergen_has_at_least_one_exit() {
    let g = load_example("codergen.dot");
    assert!(
        !g.exit_nodes().is_empty(),
        "codergen.dot should have at least 1 exit node"
    );
}

#[test]
fn loop_with_exit_has_at_least_one_exit() {
    let g = load_example("loop-with-exit.dot");
    assert!(
        !g.exit_nodes().is_empty(),
        "loop-with-exit.dot should have at least 1 exit node"
    );
}

#[test]
fn multi_gate_has_at_least_one_exit() {
    let g = load_example("multi-gate.dot");
    assert!(
        !g.exit_nodes().is_empty(),
        "multi-gate.dot should have at least 1 exit node"
    );
}

// ============================================================================
// Structural validation: all nodes reachable from start (BFS)
// ============================================================================

#[test]
fn hello_world_all_nodes_reachable() {
    let g = load_example("hello-world.dot");
    let reachable = reachable_from_start(&g);
    for node in &g.nodes {
        assert!(
            reachable.contains(node.id.as_str()),
            "hello-world.dot: node '{}' is not reachable from start",
            node.id
        );
    }
}

#[test]
fn conditional_all_nodes_reachable() {
    let g = load_example("conditional.dot");
    let reachable = reachable_from_start(&g);
    for node in &g.nodes {
        assert!(
            reachable.contains(node.id.as_str()),
            "conditional.dot: node '{}' is not reachable from start",
            node.id
        );
    }
}

#[test]
fn multi_step_all_nodes_reachable() {
    let g = load_example("multi-step.dot");
    let reachable = reachable_from_start(&g);
    for node in &g.nodes {
        assert!(
            reachable.contains(node.id.as_str()),
            "multi-step.dot: node '{}' is not reachable from start",
            node.id
        );
    }
}

#[test]
fn retry_loop_all_nodes_reachable() {
    let g = load_example("retry-loop.dot");
    let reachable = reachable_from_start(&g);
    for node in &g.nodes {
        assert!(
            reachable.contains(node.id.as_str()),
            "retry-loop.dot: node '{}' is not reachable from start",
            node.id
        );
    }
}

#[test]
fn parallel_fanout_all_nodes_reachable() {
    let g = load_example("parallel-fanout.dot");
    let reachable = reachable_from_start(&g);
    for node in &g.nodes {
        assert!(
            reachable.contains(node.id.as_str()),
            "parallel-fanout.dot: node '{}' is not reachable from start",
            node.id
        );
    }
}

#[test]
fn human_gate_all_nodes_reachable() {
    let g = load_example("human-gate.dot");
    let reachable = reachable_from_start(&g);
    for node in &g.nodes {
        assert!(
            reachable.contains(node.id.as_str()),
            "human-gate.dot: node '{}' is not reachable from start",
            node.id
        );
    }
}

#[test]
fn codergen_all_nodes_reachable() {
    let g = load_example("codergen.dot");
    let reachable = reachable_from_start(&g);
    for node in &g.nodes {
        assert!(
            reachable.contains(node.id.as_str()),
            "codergen.dot: node '{}' is not reachable from start",
            node.id
        );
    }
}

#[test]
fn loop_with_exit_all_nodes_reachable() {
    let g = load_example("loop-with-exit.dot");
    let reachable = reachable_from_start(&g);

    // The `timed_out` node is a documented orphan: it exists as a safety-valve
    // exit node that the engine reaches via max_steps, not via graph edges.
    let documented_orphans: HashSet<&str> = HashSet::from(["timed_out"]);

    for node in &g.nodes {
        if documented_orphans.contains(node.id.as_str()) {
            continue;
        }
        assert!(
            reachable.contains(node.id.as_str()),
            "loop-with-exit.dot: node '{}' is not reachable from start",
            node.id
        );
    }
}

#[test]
fn multi_gate_all_nodes_reachable() {
    let g = load_example("multi-gate.dot");
    let reachable = reachable_from_start(&g);
    for node in &g.nodes {
        assert!(
            reachable.contains(node.id.as_str()),
            "multi-gate.dot: node '{}' is not reachable from start",
            node.id
        );
    }
}

// ============================================================================
// Structural validation: no orphan nodes (no incoming AND no outgoing edges,
// except start nodes which have no incoming by design)
// ============================================================================

#[test]
fn hello_world_no_orphan_nodes() {
    let g = load_example("hello-world.dot");
    assert_no_orphans(&g, "hello-world.dot");
}

#[test]
fn conditional_no_orphan_nodes() {
    let g = load_example("conditional.dot");
    assert_no_orphans(&g, "conditional.dot");
}

#[test]
fn multi_step_no_orphan_nodes() {
    let g = load_example("multi-step.dot");
    assert_no_orphans(&g, "multi-step.dot");
}

#[test]
fn retry_loop_no_orphan_nodes() {
    let g = load_example("retry-loop.dot");
    assert_no_orphans(&g, "retry-loop.dot");
}

#[test]
fn parallel_fanout_no_orphan_nodes() {
    let g = load_example("parallel-fanout.dot");
    assert_no_orphans(&g, "parallel-fanout.dot");
}

#[test]
fn human_gate_no_orphan_nodes() {
    let g = load_example("human-gate.dot");
    assert_no_orphans(&g, "human-gate.dot");
}

#[test]
fn codergen_no_orphan_nodes() {
    let g = load_example("codergen.dot");
    assert_no_orphans(&g, "codergen.dot");
}

#[test]
fn loop_with_exit_no_orphan_nodes() {
    let g = load_example("loop-with-exit.dot");
    // The `timed_out` node is a documented orphan: it exists as a safety-valve
    // exit node that the engine reaches via max_steps, not via graph edges.
    assert_no_orphans_except(&g, "loop-with-exit.dot", &["timed_out"]);
}

#[test]
fn multi_gate_no_orphan_nodes() {
    let g = load_example("multi-gate.dot");
    assert_no_orphans(&g, "multi-gate.dot");
}

/// Assert that a graph has no orphan nodes. An orphan is a node that has
/// no incoming edges AND no outgoing edges. Start nodes are excluded since
/// they naturally lack incoming edges.
fn assert_no_orphans(graph: &Graph, filename: &str) {
    assert_no_orphans_except(graph, filename, &[]);
}

/// Assert that a graph has no orphan nodes, with an explicit list of
/// documented exceptions (nodes that are intentionally disconnected).
fn assert_no_orphans_except(graph: &Graph, filename: &str, exceptions: &[&str]) {
    let exception_set: HashSet<&str> = exceptions.iter().copied().collect();
    let sources: HashSet<&str> = graph.edges.iter().map(|e| e.from.as_str()).collect();
    let targets: HashSet<&str> = graph.edges.iter().map(|e| e.to.as_str()).collect();

    for node in &graph.nodes {
        if exception_set.contains(node.id.as_str()) {
            continue;
        }
        if node.node_type == NodeType::Start {
            // Start nodes only need outgoing edges.
            assert!(
                sources.contains(node.id.as_str()),
                "{filename}: start node '{}' has no outgoing edges (orphan)",
                node.id
            );
        } else {
            let has_incoming = targets.contains(node.id.as_str());
            let has_outgoing = sources.contains(node.id.as_str());
            assert!(
                has_incoming || has_outgoing,
                "{filename}: node '{}' is an orphan (no incoming or outgoing edges)",
                node.id
            );
        }
    }
}

// ============================================================================
// Comprehensive test: glob all examples, parse+lint, collect failures
// ============================================================================

#[test]
fn all_examples_pass_lint() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let examples_dir = workspace_root.join("examples");

    let mut failures: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(&examples_dir)
        .unwrap_or_else(|e| panic!("failed to read examples dir: {e}"))
    {
        let entry = entry.unwrap();
        let path = entry.path();

        if path.extension().and_then(|ext| ext.to_str()) != Some("dot") {
            continue;
        }

        let filename = path.file_name().unwrap().to_str().unwrap();
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));

        let dot = match parser::parse(&source) {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{filename}: parse error: {e}"));
                continue;
            }
        };

        let graph = match graph::resolve(&dot) {
            Ok(g) => g,
            Err(e) => {
                failures.push(format!("{filename}: resolve error: {e}"));
                continue;
            }
        };

        let runner = LintRunner::with_builtins();
        let report = runner.run(&graph);

        if report.has_errors() {
            let error_msgs: Vec<String> = report
                .errors()
                .iter()
                .map(|d| format!("[{}] {}", d.code, d.message))
                .collect();
            failures.push(format!("{filename}: {}", error_msgs.join("; ")));
        }
    }

    assert!(
        failures.is_empty(),
        "The following example pipelines failed lint:\n{}",
        failures.join("\n")
    );
}

// ============================================================================
// Lint rules against known-bad DOT: E001 — no start node
// ============================================================================

#[test]
fn lint_known_bad_e001_no_start_node() {
    let source = r#"
        digraph NoStart {
            process [shape=box, label="Process"];
            done [shape=doublecircle, label="Done"];
            process -> done;
        }
    "#;
    let dot = parser::parse(source).unwrap();
    let graph = graph::resolve(&dot).unwrap();
    let runner = LintRunner::with_builtins();
    let report = runner.run(&graph);

    let codes: Vec<&str> = report.diagnostics.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"E001"), "Expected E001, got: {codes:?}");

    let e001 = report
        .diagnostics
        .iter()
        .find(|d| d.code == "E001")
        .unwrap();
    assert_eq!(e001.severity, Severity::Error);
}

// ============================================================================
// Lint rules against known-bad DOT: E002 — multiple start nodes
// ============================================================================

#[test]
fn lint_known_bad_e002_multiple_start_nodes() {
    let source = r#"
        digraph MultiStart {
            s1 [shape=circle, label="Start 1"];
            s2 [shape=circle, label="Start 2"];
            done [shape=doublecircle, label="Done"];
            s1 -> done;
            s2 -> done;
        }
    "#;
    let dot = parser::parse(source).unwrap();
    let graph = graph::resolve(&dot).unwrap();
    let runner = LintRunner::with_builtins();
    let report = runner.run(&graph);

    let codes: Vec<&str> = report.diagnostics.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"E002"), "Expected E002, got: {codes:?}");

    let e002 = report
        .diagnostics
        .iter()
        .find(|d| d.code == "E002")
        .unwrap();
    assert_eq!(e002.severity, Severity::Error);
    assert!(e002.message.contains("s1"));
    assert!(e002.message.contains("s2"));
}

// ============================================================================
// Lint rules against known-bad DOT: E003 — no exit node
// ============================================================================

#[test]
fn lint_known_bad_e003_no_exit_node() {
    let source = r#"
        digraph NoExit {
            start [shape=circle, label="Start"];
            process [shape=box, label="Process"];
            start -> process;
        }
    "#;
    let dot = parser::parse(source).unwrap();
    let graph = graph::resolve(&dot).unwrap();
    let runner = LintRunner::with_builtins();
    let report = runner.run(&graph);

    let codes: Vec<&str> = report.diagnostics.iter().map(|d| d.code.as_str()).collect();
    assert!(codes.contains(&"E003"), "Expected E003, got: {codes:?}");

    let e003 = report
        .diagnostics
        .iter()
        .find(|d| d.code == "E003")
        .unwrap();
    assert_eq!(e003.severity, Severity::Error);
}

// ============================================================================
// Lint rules against known-bad DOT: W001 — unreachable node
// ============================================================================

#[test]
fn lint_known_bad_w001_unreachable_node() {
    let source = r#"
        digraph Unreachable {
            start [shape=circle, label="Start"];
            process [shape=box, label="Process"];
            orphan [shape=box, label="Orphan"];
            done [shape=doublecircle, label="Done"];
            start -> process;
            process -> done;
        }
    "#;
    let dot = parser::parse(source).unwrap();
    let graph = graph::resolve(&dot).unwrap();
    let runner = LintRunner::with_builtins();
    let report = runner.run(&graph);

    let w001_diags: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "W001")
        .collect();
    assert!(
        !w001_diags.is_empty(),
        "Expected W001 for unreachable 'orphan' node"
    );
    assert_eq!(w001_diags[0].severity, Severity::Warning);
    assert_eq!(w001_diags[0].node_id.as_deref(), Some("orphan"));
}

// ============================================================================
// Lint rules against known-bad DOT: W002 — dead end node
// ============================================================================

#[test]
fn lint_known_bad_w002_dead_end_node() {
    let source = r#"
        digraph DeadEnd {
            start [shape=circle, label="Start"];
            process [shape=box, label="Process"];
            dead [shape=box, label="Dead End"];
            done [shape=doublecircle, label="Done"];
            start -> process;
            start -> dead;
            process -> done;
        }
    "#;
    let dot = parser::parse(source).unwrap();
    let graph = graph::resolve(&dot).unwrap();
    let runner = LintRunner::with_builtins();
    let report = runner.run(&graph);

    let w002_diags: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "W002")
        .collect();
    assert!(
        !w002_diags.is_empty(),
        "Expected W002 for dead-end 'dead' node"
    );
    assert_eq!(w002_diags[0].severity, Severity::Warning);
    assert_eq!(w002_diags[0].node_id.as_deref(), Some("dead"));
}

// ============================================================================
// Lint rules against known-bad DOT: W003 — missing condition on conditional
// ============================================================================

#[test]
fn lint_known_bad_w003_missing_condition() {
    let source = r#"
        digraph MissingCondition {
            start [shape=circle, label="Start"];
            check [shape=diamond, label="Check"];
            a [shape=box, label="A"];
            b [shape=box, label="B"];
            done [shape=doublecircle, label="Done"];
            start -> check;
            check -> a;
            check -> b;
            a -> done;
            b -> done;
        }
    "#;
    let dot = parser::parse(source).unwrap();
    let graph = graph::resolve(&dot).unwrap();
    let runner = LintRunner::with_builtins();
    let report = runner.run(&graph);

    let w003_diags: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "W003")
        .collect();
    assert!(
        !w003_diags.is_empty(),
        "Expected W003 for conditional 'check' without conditions on edges"
    );
    assert_eq!(w003_diags[0].severity, Severity::Warning);
    assert_eq!(w003_diags[0].node_id.as_deref(), Some("check"));
    // Both edges from check should trigger W003
    assert_eq!(
        w003_diags.len(),
        2,
        "Expected 2 W003 diagnostics (one per edge from 'check'), got {}",
        w003_diags.len()
    );
}

// ============================================================================
// Lint rules against known-bad DOT: I001 — empty/missing edge label
// ============================================================================

#[test]
fn lint_known_bad_i001_empty_edge_label() {
    let source = r#"
        digraph EmptyLabel {
            start [shape=circle, label="Start"];
            process [shape=box, label="Process"];
            done [shape=doublecircle, label="Done"];
            start -> process;
            process -> done;
        }
    "#;
    let dot = parser::parse(source).unwrap();
    let graph = graph::resolve(&dot).unwrap();
    let runner = LintRunner::with_builtins();
    let report = runner.run(&graph);

    let i001_diags: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code == "I001")
        .collect();
    assert!(
        !i001_diags.is_empty(),
        "Expected I001 for edges without labels"
    );
    assert_eq!(i001_diags[0].severity, Severity::Info);
    // Both edges lack labels
    assert_eq!(
        i001_diags.len(),
        2,
        "Expected 2 I001 diagnostics (one per unlabeled edge), got {}",
        i001_diags.len()
    );
}

// ============================================================================
// Known-bad DOT: all errors combined
// ============================================================================

#[test]
fn lint_known_bad_all_errors_combined() {
    // A graph that triggers every rule at once.
    let source = r#"
        digraph AllBad {
            process [shape=box, label="Process"];
            orphan [shape=box, label="Orphan"];
            check [shape=diamond, label="Check"];
            process -> check;
            check -> process;
        }
    "#;
    let dot = parser::parse(source).unwrap();
    let graph = graph::resolve(&dot).unwrap();
    let runner = LintRunner::with_builtins();
    let report = runner.run(&graph);

    let codes: HashSet<&str> = report.diagnostics.iter().map(|d| d.code.as_str()).collect();

    // E001: no start node
    assert!(codes.contains("E001"), "Expected E001, got: {codes:?}");
    // E003: no exit node
    assert!(codes.contains("E003"), "Expected E003, got: {codes:?}");
    // W001: orphan has no incoming edges
    assert!(codes.contains("W001"), "Expected W001, got: {codes:?}");
    // W002: orphan has no outgoing edges
    assert!(codes.contains("W002"), "Expected W002, got: {codes:?}");
    // W003: check is conditional with no condition on edges
    assert!(codes.contains("W003"), "Expected W003, got: {codes:?}");
    // I001: edges have no labels
    assert!(codes.contains("I001"), "Expected I001, got: {codes:?}");
}

// ============================================================================
// Known-good DOT: clean graph produces no errors
// ============================================================================

#[test]
fn lint_known_good_clean_graph() {
    let source = r#"
        digraph Clean {
            start [shape=circle, label="Start"];
            process [shape=box, label="Process"];
            done [shape=doublecircle, label="Done"];
            start -> process [label="begin"];
            process -> done [label="finish"];
        }
    "#;
    let dot = parser::parse(source).unwrap();
    let graph = graph::resolve(&dot).unwrap();
    let runner = LintRunner::with_builtins();
    let report = runner.run(&graph);

    assert!(
        !report.has_errors(),
        "Clean graph should have no errors: {:?}",
        report.errors()
    );
    assert!(
        report.is_clean(),
        "Clean graph should be fully clean: {:?}",
        report.diagnostics
    );
}

// ============================================================================
// Known-bad DOT: E002 with three start nodes
// ============================================================================

#[test]
fn lint_known_bad_e002_three_start_nodes() {
    let source = r#"
        digraph ThreeStarts {
            s1 [shape=circle, label="S1"];
            s2 [shape=point, label="S2"];
            s3 [shape=circle, label="S3"];
            done [shape=doublecircle, label="Done"];
            s1 -> done;
            s2 -> done;
            s3 -> done;
        }
    "#;
    let dot = parser::parse(source).unwrap();
    let graph = graph::resolve(&dot).unwrap();
    let runner = LintRunner::with_builtins();
    let report = runner.run(&graph);

    let e002 = report
        .diagnostics
        .iter()
        .find(|d| d.code == "E002")
        .expect("Expected E002 for three start nodes");
    assert_eq!(e002.severity, Severity::Error);
    // All three start node IDs should be mentioned
    assert!(
        e002.message.contains("s1"),
        "E002 message should mention s1"
    );
    assert!(
        e002.message.contains("s2"),
        "E002 message should mention s2"
    );
    assert!(
        e002.message.contains("s3"),
        "E002 message should mention s3"
    );
}

// ============================================================================
// Lint diagnostic suggestions are present
// ============================================================================

#[test]
fn lint_diagnostics_have_suggestions() {
    let source = r#"
        digraph NoStart {
            process [shape=box, label="Process"];
            done [shape=doublecircle, label="Done"];
            process -> done;
        }
    "#;
    let dot = parser::parse(source).unwrap();
    let graph = graph::resolve(&dot).unwrap();
    let runner = LintRunner::with_builtins();
    let report = runner.run(&graph);

    for diag in &report.diagnostics {
        assert!(
            diag.suggestion.is_some(),
            "Diagnostic {} ({}) should have a suggestion",
            diag.code,
            diag.message
        );
    }
}
