// ABOUTME: Integration tests validating all example DOT pipelines against the lint framework.
// ABOUTME: Ensures examples are structurally sound and tests lint rules against known-bad DOT.

use std::collections::{HashSet, VecDeque};
use std::path::Path;

use smasher_attractor::dot::parser;
use smasher_attractor::graph::{self, Graph, NodeType};
use smasher_attractor::lint::{LintRunner, Severity};

/// Helper: read, parse, and resolve a DOT file from the examples directory.
fn load_example(filename: &str) -> Graph {
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

/// All current example filenames.
const EXAMPLES: &[&str] = &[
    "consensus_task.dot",
    "consensus_task_parity.dot",
    "megaplan.dot",
    "megaplan_quality.dot",
    "semport.dot",
    "semport_thematic.dot",
    "sprint_exec.dot",
    "vulnerability_analyzer.dot",
];

// ============================================================================
// Per-example lint validation: no Error-level diagnostics
// ============================================================================

#[test]
fn consensus_task_passes_lint() {
    let g = load_example("consensus_task.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "consensus_task.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn consensus_task_parity_passes_lint() {
    let g = load_example("consensus_task_parity.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "consensus_task_parity.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn megaplan_passes_lint() {
    let g = load_example("megaplan.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "megaplan.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn megaplan_quality_passes_lint() {
    let g = load_example("megaplan_quality.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "megaplan_quality.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn semport_passes_lint() {
    let g = load_example("semport.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "semport.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn semport_thematic_passes_lint() {
    let g = load_example("semport_thematic.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "semport_thematic.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn sprint_exec_passes_lint() {
    let g = load_example("sprint_exec.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "sprint_exec.dot has lint errors: {:?}",
        report.errors()
    );
}

#[test]
fn vulnerability_analyzer_passes_lint() {
    let g = load_example("vulnerability_analyzer.dot");
    let runner = LintRunner::with_builtins();
    let report = runner.run(&g);
    assert!(
        !report.has_errors(),
        "vulnerability_analyzer.dot has lint errors: {:?}",
        report.errors()
    );
}

// ============================================================================
// Structural validation: every example has exactly one start node
// ============================================================================

#[test]
fn all_examples_have_exactly_one_start() {
    for filename in EXAMPLES {
        let g = load_example(filename);
        assert_eq!(
            g.start_nodes().len(),
            1,
            "{filename} should have exactly 1 start node"
        );
    }
}

// ============================================================================
// Structural validation: every example has at least one exit node
// ============================================================================

#[test]
fn all_examples_have_at_least_one_exit() {
    for filename in EXAMPLES {
        let g = load_example(filename);
        assert!(
            !g.exit_nodes().is_empty(),
            "{filename} should have at least 1 exit node"
        );
    }
}

// ============================================================================
// Structural validation: all nodes reachable from start (BFS)
// ============================================================================

#[test]
fn all_examples_all_nodes_reachable() {
    for filename in EXAMPLES {
        let g = load_example(filename);
        let reachable = reachable_from_start(&g);
        for node in &g.nodes {
            assert!(
                reachable.contains(node.id.as_str()),
                "{filename}: node '{}' is not reachable from start",
                node.id
            );
        }
    }
}

// ============================================================================
// Structural validation: no orphan nodes
// ============================================================================

#[test]
fn all_examples_no_orphan_nodes() {
    for filename in EXAMPLES {
        let g = load_example(filename);
        assert_no_orphans(&g, filename);
    }
}

/// Assert that a graph has no orphan nodes. An orphan is a node that has
/// no incoming edges AND no outgoing edges. Start nodes are excluded since
/// they naturally lack incoming edges.
fn assert_no_orphans(graph: &Graph, filename: &str) {
    let sources: HashSet<&str> = graph.edges.iter().map(|e| e.from.as_str()).collect();
    let targets: HashSet<&str> = graph.edges.iter().map(|e| e.to.as_str()).collect();

    for node in &graph.nodes {
        if node.node_type == NodeType::Start {
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
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
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

    assert!(codes.contains("E001"), "Expected E001, got: {codes:?}");
    assert!(codes.contains("E003"), "Expected E003, got: {codes:?}");
    assert!(codes.contains("W001"), "Expected W001, got: {codes:?}");
    assert!(codes.contains("W002"), "Expected W002, got: {codes:?}");
    assert!(codes.contains("W003"), "Expected W003, got: {codes:?}");
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
    assert!(e002.message.contains("s1"));
    assert!(e002.message.contains("s2"));
    assert!(e002.message.contains("s3"));
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
