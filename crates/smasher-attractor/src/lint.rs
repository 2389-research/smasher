// ABOUTME: Trait-based lint framework for validating DOT pipeline graphs with severity levels and diagnostic codes.
// ABOUTME: Provides built-in rules for structural checks and supports custom rule registration via LintRunner.

//! Graph lint framework for validating pipeline structure.
//!
//! Before a pipeline is executed, the lint system can catch structural problems
//! such as missing start/exit nodes, unreachable nodes, or conditional edges
//! without conditions. Each issue is reported as a [`Diagnostic`] carrying a
//! severity level ([`Severity`]), a machine-readable code (e.g. `"E001"`), and
//! a human-readable message with an optional suggestion.
//!
//! [`LintRunner`] orchestrates the checks. Use [`LintRunner::with_builtins`] to
//! get all built-in rules, or build a custom set with [`LintRunner::new`] and
//! [`LintRunner::add_rule`].
//!
//! Built-in rules:
//!
//! | Code  | Severity | Rule                     |
//! |-------|----------|--------------------------|
//! | E001  | Error    | No start node            |
//! | E002  | Error    | Multiple start nodes     |
//! | E003  | Error    | No exit node             |
//! | W001  | Warning  | Unreachable node         |
//! | W002  | Warning  | Dead-end non-exit node   |
//! | W003  | Warning  | Missing condition on edge |
//! | I001  | Info     | Edge without label       |
//!
//! # Examples
//!
//! ```
//! use std::collections::HashMap;
//! use smasher_attractor::graph::{Graph, GraphNode, GraphEdge, NodeType};
//! use smasher_attractor::lint::{LintRunner, Severity};
//!
//! let graph = Graph {
//!     name: Some("example".into()),
//!     nodes: vec![
//!         GraphNode { id: "start".into(), node_type: NodeType::Start,
//!                     label: None, attrs: HashMap::new() },
//!         GraphNode { id: "exit".into(), node_type: NodeType::Exit,
//!                     label: None, attrs: HashMap::new() },
//!     ],
//!     edges: vec![
//!         GraphEdge { from: "start".into(), to: "exit".into(),
//!                     label: None, condition: None, priority: None,
//!                     loop_restart: false, attrs: HashMap::new() },
//!     ],
//!     default_node_attrs: HashMap::new(),
//!     default_edge_attrs: HashMap::new(),
//! };
//!
//! let runner = LintRunner::with_builtins();
//! let report = runner.run(&graph);
//!
//! // A well-formed graph produces no errors.
//! assert!(!report.has_errors());
//! ```

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::graph::{Graph, NodeType};

/// Severity level for a diagnostic produced by a lint rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// A diagnostic message produced by a lint rule during graph validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub node_id: Option<String>,
    pub suggestion: Option<String>,
}

/// Trait for implementing custom lint rules against a pipeline graph.
pub trait LintRule {
    /// A short, unique name identifying this rule.
    fn name(&self) -> &str;

    /// A human-readable description of what this rule checks.
    fn description(&self) -> &str;

    /// Run this rule against the given graph and return any diagnostics.
    fn check(&self, graph: &Graph) -> Vec<Diagnostic>;
}

/// Collects diagnostics from a lint run and provides filtering methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintReport {
    pub diagnostics: Vec<Diagnostic>,
}

impl LintReport {
    /// Return all diagnostics with Error severity.
    pub fn errors(&self) -> Vec<&Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .collect()
    }

    /// Return all diagnostics with Warning severity.
    pub fn warnings(&self) -> Vec<&Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .collect()
    }

    /// Returns true if there is at least one Error-severity diagnostic.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Returns true if there are no Error or Warning diagnostics.
    pub fn is_clean(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error || d.severity == Severity::Warning)
    }
}

/// Runs a collection of lint rules against a graph and produces a report.
pub struct LintRunner {
    rules: Vec<Box<dyn LintRule>>,
}

impl LintRunner {
    /// Create a runner with no rules.
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Create a runner pre-loaded with all built-in lint rules.
    pub fn with_builtins() -> Self {
        let mut runner = Self::new();
        runner.add_rule(Box::new(NoStartNodeRule));
        runner.add_rule(Box::new(MultipleStartNodesRule));
        runner.add_rule(Box::new(NoExitNodeRule));
        runner.add_rule(Box::new(UnreachableNodeRule));
        runner.add_rule(Box::new(DeadEndNodeRule));
        runner.add_rule(Box::new(MissingConditionRule));
        runner.add_rule(Box::new(EmptyLabelEdgeRule));
        runner
    }

    /// Register a custom lint rule.
    pub fn add_rule(&mut self, rule: Box<dyn LintRule>) {
        self.rules.push(rule);
    }

    /// Run all registered rules against the graph and collect results.
    pub fn run(&self, graph: &Graph) -> LintReport {
        let mut diagnostics = Vec::new();
        for rule in &self.rules {
            diagnostics.extend(rule.check(graph));
        }
        LintReport { diagnostics }
    }
}

impl Default for LintRunner {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Built-in lint rules
// ---------------------------------------------------------------------------

/// E001: Graph has no start node.
pub struct NoStartNodeRule;

impl LintRule for NoStartNodeRule {
    fn name(&self) -> &str {
        "no_start_node"
    }

    fn description(&self) -> &str {
        "Checks that the graph has at least one Start-type node"
    }

    fn check(&self, graph: &Graph) -> Vec<Diagnostic> {
        let has_start = graph.nodes.iter().any(|n| n.node_type == NodeType::Start);
        if !has_start {
            vec![Diagnostic {
                severity: Severity::Error,
                code: "E001".to_string(),
                message: "Graph has no start node".to_string(),
                node_id: None,
                suggestion: Some(
                    "Add a node with shape=\"circle\" or shape=\"point\" to serve as the entry point"
                        .to_string(),
                ),
            }]
        } else {
            vec![]
        }
    }
}

/// E002: Graph has multiple start nodes.
pub struct MultipleStartNodesRule;

impl LintRule for MultipleStartNodesRule {
    fn name(&self) -> &str {
        "multiple_start_nodes"
    }

    fn description(&self) -> &str {
        "Checks that the graph has at most one Start-type node"
    }

    fn check(&self, graph: &Graph) -> Vec<Diagnostic> {
        let start_nodes: Vec<&str> = graph
            .nodes
            .iter()
            .filter(|n| n.node_type == NodeType::Start)
            .map(|n| n.id.as_str())
            .collect();

        if start_nodes.len() > 1 {
            vec![Diagnostic {
                severity: Severity::Error,
                code: "E002".to_string(),
                message: format!(
                    "Graph has multiple start nodes: {}",
                    start_nodes
                        .iter()
                        .map(|id| format!("'{id}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                node_id: None,
                suggestion: Some(
                    "Remove extra start nodes so the graph has exactly one entry point".to_string(),
                ),
            }]
        } else {
            vec![]
        }
    }
}

/// E003: Graph has no exit node.
pub struct NoExitNodeRule;

impl LintRule for NoExitNodeRule {
    fn name(&self) -> &str {
        "no_exit_node"
    }

    fn description(&self) -> &str {
        "Checks that the graph has at least one Exit-type node"
    }

    fn check(&self, graph: &Graph) -> Vec<Diagnostic> {
        let has_exit = graph.nodes.iter().any(|n| n.node_type == NodeType::Exit);
        if !has_exit {
            vec![Diagnostic {
                severity: Severity::Error,
                code: "E003".to_string(),
                message: "Graph has no exit node".to_string(),
                node_id: None,
                suggestion: Some(
                    "Add a node with shape=\"doublecircle\" to serve as the exit point".to_string(),
                ),
            }]
        } else {
            vec![]
        }
    }
}

/// W001: Node has no incoming edges (except start nodes).
pub struct UnreachableNodeRule;

impl LintRule for UnreachableNodeRule {
    fn name(&self) -> &str {
        "unreachable_node"
    }

    fn description(&self) -> &str {
        "Checks for non-start nodes that have no incoming edges"
    }

    fn check(&self, graph: &Graph) -> Vec<Diagnostic> {
        let targets_with_incoming: HashSet<&str> =
            graph.edges.iter().map(|e| e.to.as_str()).collect();

        graph
            .nodes
            .iter()
            .filter(|n| n.node_type != NodeType::Start)
            .filter(|n| !targets_with_incoming.contains(n.id.as_str()))
            .map(|n| Diagnostic {
                severity: Severity::Warning,
                code: "W001".to_string(),
                message: format!("Node '{}' has no incoming edges and is unreachable", n.id),
                node_id: Some(n.id.clone()),
                suggestion: Some(format!(
                    "Add an edge leading to '{}' or remove it if unused",
                    n.id
                )),
            })
            .collect()
    }
}

/// W002: Non-exit node has no outgoing edges.
pub struct DeadEndNodeRule;

impl LintRule for DeadEndNodeRule {
    fn name(&self) -> &str {
        "dead_end_node"
    }

    fn description(&self) -> &str {
        "Checks for non-exit nodes that have no outgoing edges"
    }

    fn check(&self, graph: &Graph) -> Vec<Diagnostic> {
        let sources_with_outgoing: HashSet<&str> =
            graph.edges.iter().map(|e| e.from.as_str()).collect();

        graph
            .nodes
            .iter()
            .filter(|n| n.node_type != NodeType::Exit)
            .filter(|n| !sources_with_outgoing.contains(n.id.as_str()))
            .map(|n| Diagnostic {
                severity: Severity::Warning,
                code: "W002".to_string(),
                message: format!("Non-exit node '{}' has no outgoing edges (dead end)", n.id),
                node_id: Some(n.id.clone()),
                suggestion: Some(format!(
                    "Add an outgoing edge from '{}' or change it to an exit node",
                    n.id
                )),
            })
            .collect()
    }
}

/// W003: Diamond (conditional) node with no condition attribute on outgoing edges.
pub struct MissingConditionRule;

impl LintRule for MissingConditionRule {
    fn name(&self) -> &str {
        "missing_condition"
    }

    fn description(&self) -> &str {
        "Checks that diamond/conditional nodes have condition attributes on their outgoing edges"
    }

    fn check(&self, graph: &Graph) -> Vec<Diagnostic> {
        let conditional_ids: HashSet<&str> = graph
            .nodes
            .iter()
            .filter(|n| n.node_type == NodeType::Conditional)
            .map(|n| n.id.as_str())
            .collect();

        graph
            .edges
            .iter()
            .filter(|e| conditional_ids.contains(e.from.as_str()))
            .filter(|e| e.condition.is_none())
            .map(|e| Diagnostic {
                severity: Severity::Warning,
                code: "W003".to_string(),
                message: format!(
                    "Conditional node '{}' has an outgoing edge to '{}' without a condition",
                    e.from, e.to
                ),
                node_id: Some(e.from.clone()),
                suggestion: Some(format!(
                    "Add a condition attribute to the edge from '{}' to '{}'",
                    e.from, e.to
                )),
            })
            .collect()
    }
}

/// I001: Edge has empty or missing label.
pub struct EmptyLabelEdgeRule;

impl LintRule for EmptyLabelEdgeRule {
    fn name(&self) -> &str {
        "empty_label_edge"
    }

    fn description(&self) -> &str {
        "Checks for edges that have an empty or missing label"
    }

    fn check(&self, graph: &Graph) -> Vec<Diagnostic> {
        graph
            .edges
            .iter()
            .filter(|e| match &e.label {
                None => true,
                Some(label) => label.trim().is_empty(),
            })
            .map(|e| Diagnostic {
                severity: Severity::Info,
                code: "I001".to_string(),
                message: format!("Edge from '{}' to '{}' has no label", e.from, e.to),
                node_id: None,
                suggestion: Some(format!(
                    "Add a label to the edge from '{}' to '{}' for clarity",
                    e.from, e.to
                )),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, GraphEdge, GraphNode, NodeType};
    use std::collections::HashMap;

    // ---------------------------------------------------------------
    // Test helpers
    // ---------------------------------------------------------------

    fn make_node(id: &str, node_type: NodeType) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
            label: Some(id.to_string()),
            attrs: HashMap::new(),
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
            name: Some("test".to_string()),
            nodes,
            edges,
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
        }
    }

    /// A well-formed graph with one start, one process, one exit, all connected.
    fn clean_graph() -> Graph {
        make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("process", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_labeled_edge("start", "process", "begin"),
                make_labeled_edge("process", "exit", "done"),
            ],
        )
    }

    // ---------------------------------------------------------------
    // Severity ordering tests
    // ---------------------------------------------------------------

    #[test]
    fn severity_ordering() {
        assert!(Severity::Info < Severity::Warning);
        assert!(Severity::Warning < Severity::Error);
        assert!(Severity::Info < Severity::Error);
    }

    // ---------------------------------------------------------------
    // NoStartNodeRule tests
    // ---------------------------------------------------------------

    #[test]
    fn no_start_node_rule_triggers_on_graph_without_start() {
        let graph = make_graph(
            vec![
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("a", "exit")],
        );
        let rule = NoStartNodeRule;
        let diags = rule.check(&graph);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "E001");
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].suggestion.is_some());
    }

    #[test]
    fn no_start_node_rule_clean_on_graph_with_start() {
        let graph = clean_graph();
        let rule = NoStartNodeRule;
        let diags = rule.check(&graph);
        assert!(diags.is_empty());
    }

    #[test]
    fn no_start_node_rule_metadata() {
        let rule = NoStartNodeRule;
        assert_eq!(rule.name(), "no_start_node");
        assert!(!rule.description().is_empty());
    }

    // ---------------------------------------------------------------
    // MultipleStartNodesRule tests
    // ---------------------------------------------------------------

    #[test]
    fn multiple_start_nodes_rule_triggers_with_two_starts() {
        let graph = make_graph(
            vec![
                make_node("s1", NodeType::Start),
                make_node("s2", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("s1", "exit"), make_edge("s2", "exit")],
        );
        let rule = MultipleStartNodesRule;
        let diags = rule.check(&graph);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "E002");
        assert_eq!(diags[0].severity, Severity::Error);
        assert!(diags[0].message.contains("s1"));
        assert!(diags[0].message.contains("s2"));
    }

    #[test]
    fn multiple_start_nodes_rule_clean_with_one_start() {
        let graph = clean_graph();
        let rule = MultipleStartNodesRule;
        let diags = rule.check(&graph);
        assert!(diags.is_empty());
    }

    // ---------------------------------------------------------------
    // NoExitNodeRule tests
    // ---------------------------------------------------------------

    #[test]
    fn no_exit_node_rule_triggers_without_exit() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
            ],
            vec![make_edge("start", "a")],
        );
        let rule = NoExitNodeRule;
        let diags = rule.check(&graph);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "E003");
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn no_exit_node_rule_clean_with_exit() {
        let graph = clean_graph();
        let rule = NoExitNodeRule;
        let diags = rule.check(&graph);
        assert!(diags.is_empty());
    }

    // ---------------------------------------------------------------
    // UnreachableNodeRule tests
    // ---------------------------------------------------------------

    #[test]
    fn unreachable_node_rule_triggers_for_isolated_non_start() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("orphan", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "a"), make_edge("a", "exit")],
        );
        let rule = UnreachableNodeRule;
        let diags = rule.check(&graph);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "W001");
        assert_eq!(diags[0].severity, Severity::Warning);
        assert_eq!(diags[0].node_id.as_deref(), Some("orphan"));
    }

    #[test]
    fn unreachable_node_rule_does_not_flag_start_nodes() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let rule = UnreachableNodeRule;
        let diags = rule.check(&graph);
        // start has no incoming edges but should not be flagged
        assert!(diags.is_empty());
    }

    #[test]
    fn unreachable_node_rule_clean_when_all_reachable() {
        let graph = clean_graph();
        let rule = UnreachableNodeRule;
        let diags = rule.check(&graph);
        assert!(diags.is_empty());
    }

    // ---------------------------------------------------------------
    // DeadEndNodeRule tests
    // ---------------------------------------------------------------

    #[test]
    fn dead_end_node_rule_triggers_for_non_exit_with_no_outgoing() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("dead", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "dead"), make_edge("start", "exit")],
        );
        let rule = DeadEndNodeRule;
        let diags = rule.check(&graph);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "W002");
        assert_eq!(diags[0].severity, Severity::Warning);
        assert_eq!(diags[0].node_id.as_deref(), Some("dead"));
    }

    #[test]
    fn dead_end_node_rule_does_not_flag_exit_nodes() {
        let graph = clean_graph();
        let rule = DeadEndNodeRule;
        let diags = rule.check(&graph);
        // exit has no outgoing edges but should not be flagged
        assert!(diags.is_empty());
    }

    #[test]
    fn dead_end_node_rule_clean_when_all_connected() {
        let graph = clean_graph();
        let rule = DeadEndNodeRule;
        let diags = rule.check(&graph);
        assert!(diags.is_empty());
    }

    // ---------------------------------------------------------------
    // MissingConditionRule tests
    // ---------------------------------------------------------------

    #[test]
    fn missing_condition_rule_triggers_for_diamond_without_condition() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("cond", NodeType::Conditional),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "cond"),
                make_edge("cond", "a"), // no condition
                make_edge("a", "exit"),
            ],
        );
        let rule = MissingConditionRule;
        let diags = rule.check(&graph);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "W003");
        assert_eq!(diags[0].severity, Severity::Warning);
        assert_eq!(diags[0].node_id.as_deref(), Some("cond"));
    }

    #[test]
    fn missing_condition_rule_clean_when_conditions_present() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("cond", NodeType::Conditional),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "cond"),
                make_labeled_edge("cond", "a", "yes"),
                make_edge("a", "exit"),
            ],
        );
        let rule = MissingConditionRule;
        let diags = rule.check(&graph);
        assert!(diags.is_empty());
    }

    #[test]
    fn missing_condition_rule_ignores_non_conditional_nodes() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "a"), make_edge("a", "exit")],
        );
        let rule = MissingConditionRule;
        let diags = rule.check(&graph);
        assert!(diags.is_empty());
    }

    // ---------------------------------------------------------------
    // EmptyLabelEdgeRule tests
    // ---------------------------------------------------------------

    #[test]
    fn empty_label_edge_rule_triggers_for_unlabeled_edges() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let rule = EmptyLabelEdgeRule;
        let diags = rule.check(&graph);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "I001");
        assert_eq!(diags[0].severity, Severity::Info);
    }

    #[test]
    fn empty_label_edge_rule_triggers_for_empty_string_label() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![GraphEdge {
                from: "start".to_string(),
                to: "exit".to_string(),
                label: Some("  ".to_string()),
                condition: None,
                priority: None,
                loop_restart: false,
                attrs: HashMap::new(),
            }],
        );
        let rule = EmptyLabelEdgeRule;
        let diags = rule.check(&graph);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "I001");
    }

    #[test]
    fn empty_label_edge_rule_clean_when_labels_present() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_labeled_edge("start", "exit", "go")],
        );
        let rule = EmptyLabelEdgeRule;
        let diags = rule.check(&graph);
        assert!(diags.is_empty());
    }

    // ---------------------------------------------------------------
    // LintReport tests
    // ---------------------------------------------------------------

    #[test]
    fn lint_report_errors_returns_only_errors() {
        let report = LintReport {
            diagnostics: vec![
                Diagnostic {
                    severity: Severity::Error,
                    code: "E001".to_string(),
                    message: "error one".to_string(),
                    node_id: None,
                    suggestion: None,
                },
                Diagnostic {
                    severity: Severity::Warning,
                    code: "W001".to_string(),
                    message: "warning one".to_string(),
                    node_id: None,
                    suggestion: None,
                },
                Diagnostic {
                    severity: Severity::Info,
                    code: "I001".to_string(),
                    message: "info one".to_string(),
                    node_id: None,
                    suggestion: None,
                },
            ],
        };
        let errors = report.errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "E001");
    }

    #[test]
    fn lint_report_warnings_returns_only_warnings() {
        let report = LintReport {
            diagnostics: vec![
                Diagnostic {
                    severity: Severity::Error,
                    code: "E001".to_string(),
                    message: "error one".to_string(),
                    node_id: None,
                    suggestion: None,
                },
                Diagnostic {
                    severity: Severity::Warning,
                    code: "W001".to_string(),
                    message: "warning one".to_string(),
                    node_id: None,
                    suggestion: None,
                },
            ],
        };
        let warnings = report.warnings();
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "W001");
    }

    #[test]
    fn lint_report_has_errors_true_when_errors_exist() {
        let report = LintReport {
            diagnostics: vec![Diagnostic {
                severity: Severity::Error,
                code: "E001".to_string(),
                message: "err".to_string(),
                node_id: None,
                suggestion: None,
            }],
        };
        assert!(report.has_errors());
    }

    #[test]
    fn lint_report_has_errors_false_when_no_errors() {
        let report = LintReport {
            diagnostics: vec![Diagnostic {
                severity: Severity::Warning,
                code: "W001".to_string(),
                message: "warn".to_string(),
                node_id: None,
                suggestion: None,
            }],
        };
        assert!(!report.has_errors());
    }

    #[test]
    fn lint_report_is_clean_true_when_only_info() {
        let report = LintReport {
            diagnostics: vec![Diagnostic {
                severity: Severity::Info,
                code: "I001".to_string(),
                message: "info".to_string(),
                node_id: None,
                suggestion: None,
            }],
        };
        assert!(report.is_clean());
    }

    #[test]
    fn lint_report_is_clean_true_when_empty() {
        let report = LintReport {
            diagnostics: vec![],
        };
        assert!(report.is_clean());
    }

    #[test]
    fn lint_report_is_clean_false_when_warnings_exist() {
        let report = LintReport {
            diagnostics: vec![Diagnostic {
                severity: Severity::Warning,
                code: "W001".to_string(),
                message: "warn".to_string(),
                node_id: None,
                suggestion: None,
            }],
        };
        assert!(!report.is_clean());
    }

    #[test]
    fn lint_report_is_clean_false_when_errors_exist() {
        let report = LintReport {
            diagnostics: vec![Diagnostic {
                severity: Severity::Error,
                code: "E001".to_string(),
                message: "err".to_string(),
                node_id: None,
                suggestion: None,
            }],
        };
        assert!(!report.is_clean());
    }

    // ---------------------------------------------------------------
    // LintRunner tests
    // ---------------------------------------------------------------

    #[test]
    fn lint_runner_with_builtins_finds_issues() {
        let graph = make_graph(vec![make_node("a", NodeType::Generic)], vec![]);
        let runner = LintRunner::with_builtins();
        let report = runner.run(&graph);

        // Should at minimum have E001 (no start) and E003 (no exit)
        assert!(report.has_errors());
        let codes: Vec<&str> = report.diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert!(codes.contains(&"E001"), "Expected E001, got: {codes:?}");
        assert!(codes.contains(&"E003"), "Expected E003, got: {codes:?}");
    }

    #[test]
    fn lint_runner_with_builtins_clean_graph() {
        let graph = clean_graph();
        let runner = LintRunner::with_builtins();
        let report = runner.run(&graph);

        // Clean graph should have no errors or warnings
        assert!(
            !report.has_errors(),
            "Unexpected errors: {:?}",
            report.errors()
        );
        // It should be fully clean (no warnings either)
        assert!(
            report.is_clean(),
            "Unexpected diagnostics: {:?}",
            report.diagnostics
        );
    }

    #[test]
    fn lint_runner_no_rules_returns_clean_report() {
        let graph = make_graph(vec![], vec![]);
        let runner = LintRunner::new();
        let report = runner.run(&graph);
        assert!(report.is_clean());
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn lint_runner_custom_rule_registration() {
        struct CustomRule;
        impl LintRule for CustomRule {
            fn name(&self) -> &str {
                "custom_check"
            }
            fn description(&self) -> &str {
                "A custom rule for testing"
            }
            fn check(&self, _graph: &Graph) -> Vec<Diagnostic> {
                vec![Diagnostic {
                    severity: Severity::Warning,
                    code: "C001".to_string(),
                    message: "Custom rule triggered".to_string(),
                    node_id: None,
                    suggestion: Some("Fix the custom issue".to_string()),
                }]
            }
        }

        let graph = clean_graph();
        let mut runner = LintRunner::new();
        runner.add_rule(Box::new(CustomRule));
        let report = runner.run(&graph);

        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].code, "C001");
        assert_eq!(report.diagnostics[0].message, "Custom rule triggered");
    }

    #[test]
    fn lint_runner_default_is_empty() {
        let runner = LintRunner::default();
        let graph = make_graph(vec![], vec![]);
        let report = runner.run(&graph);
        assert!(report.diagnostics.is_empty());
    }

    // ---------------------------------------------------------------
    // Diagnostic serde round-trip test
    // ---------------------------------------------------------------

    #[test]
    fn diagnostic_serde_round_trip() {
        let diag = Diagnostic {
            severity: Severity::Error,
            code: "E001".to_string(),
            message: "Graph has no start node".to_string(),
            node_id: Some("node_42".to_string()),
            suggestion: Some("Add a start node".to_string()),
        };

        let json = serde_json::to_string(&diag).expect("serialize diagnostic");
        let restored: Diagnostic = serde_json::from_str(&json).expect("deserialize diagnostic");

        assert_eq!(restored.severity, Severity::Error);
        assert_eq!(restored.code, "E001");
        assert_eq!(restored.message, "Graph has no start node");
        assert_eq!(restored.node_id.as_deref(), Some("node_42"));
        assert_eq!(restored.suggestion.as_deref(), Some("Add a start node"));
    }

    #[test]
    fn diagnostic_serde_round_trip_with_none_fields() {
        let diag = Diagnostic {
            severity: Severity::Info,
            code: "I001".to_string(),
            message: "info message".to_string(),
            node_id: None,
            suggestion: None,
        };

        let json = serde_json::to_string(&diag).expect("serialize diagnostic");
        let restored: Diagnostic = serde_json::from_str(&json).expect("deserialize diagnostic");

        assert_eq!(restored.severity, Severity::Info);
        assert_eq!(restored.code, "I001");
        assert!(restored.node_id.is_none());
        assert!(restored.suggestion.is_none());
    }

    #[test]
    fn severity_serde_round_trip() {
        for severity in [Severity::Info, Severity::Warning, Severity::Error] {
            let json = serde_json::to_string(&severity).expect("serialize severity");
            let restored: Severity = serde_json::from_str(&json).expect("deserialize severity");
            assert_eq!(restored, severity);
        }
    }

    #[test]
    fn lint_report_serde_round_trip() {
        let report = LintReport {
            diagnostics: vec![
                Diagnostic {
                    severity: Severity::Error,
                    code: "E001".to_string(),
                    message: "error".to_string(),
                    node_id: None,
                    suggestion: None,
                },
                Diagnostic {
                    severity: Severity::Warning,
                    code: "W001".to_string(),
                    message: "warning".to_string(),
                    node_id: Some("n1".to_string()),
                    suggestion: Some("fix it".to_string()),
                },
            ],
        };

        let json = serde_json::to_string(&report).expect("serialize report");
        let restored: LintReport = serde_json::from_str(&json).expect("deserialize report");

        assert_eq!(restored.diagnostics.len(), 2);
        assert_eq!(restored.diagnostics[0].code, "E001");
        assert_eq!(restored.diagnostics[1].code, "W001");
    }

    // ---------------------------------------------------------------
    // Integration: all builtins with specific issues
    // ---------------------------------------------------------------

    #[test]
    fn all_builtins_detect_multiple_issues() {
        // Graph with: no exit, multiple starts, unreachable node, dead end, unlabeled edges
        let graph = make_graph(
            vec![
                make_node("s1", NodeType::Start),
                make_node("s2", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("orphan", NodeType::Generic),
            ],
            vec![make_edge("s1", "a")],
        );
        let runner = LintRunner::with_builtins();
        let report = runner.run(&graph);

        let codes: Vec<&str> = report.diagnostics.iter().map(|d| d.code.as_str()).collect();

        // E002: multiple start nodes
        assert!(codes.contains(&"E002"), "Expected E002, got: {codes:?}");
        // E003: no exit node
        assert!(codes.contains(&"E003"), "Expected E003, got: {codes:?}");
        // W001: orphan is unreachable
        assert!(codes.contains(&"W001"), "Expected W001, got: {codes:?}");
        // W002: a and orphan are dead ends (no outgoing)
        assert!(codes.contains(&"W002"), "Expected W002, got: {codes:?}");
        // I001: unlabeled edge
        assert!(codes.contains(&"I001"), "Expected I001, got: {codes:?}");
    }

    #[test]
    fn lint_runner_with_builtins_no_issues_for_perfect_graph() {
        let graph = clean_graph();
        let runner = LintRunner::with_builtins();
        let report = runner.run(&graph);
        assert!(report.is_clean());
        assert!(!report.has_errors());
        assert!(report.errors().is_empty());
        assert!(report.warnings().is_empty());
    }

    // ---------------------------------------------------------------
    // Rule metadata tests
    // ---------------------------------------------------------------

    #[test]
    fn all_builtin_rules_have_names_and_descriptions() {
        let rules: Vec<Box<dyn LintRule>> = vec![
            Box::new(NoStartNodeRule),
            Box::new(MultipleStartNodesRule),
            Box::new(NoExitNodeRule),
            Box::new(UnreachableNodeRule),
            Box::new(DeadEndNodeRule),
            Box::new(MissingConditionRule),
            Box::new(EmptyLabelEdgeRule),
        ];

        for rule in &rules {
            assert!(!rule.name().is_empty(), "Rule name should not be empty");
            assert!(
                !rule.description().is_empty(),
                "Rule '{}' description should not be empty",
                rule.name()
            );
        }
    }
}
