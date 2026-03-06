// ABOUTME: Lint rules and validation for pipeline graphs.
// ABOUTME: Checks for structural issues like missing start/exit nodes, unreachable nodes, and cycles.

use std::collections::{HashMap, HashSet};

use super::{Graph, GraphNode, NodeType};

/// The severity level of a lint warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// A lint warning produced by graph validation.
#[derive(Debug, Clone)]
pub struct LintWarning {
    pub rule: String,
    pub severity: Severity,
    pub message: String,
    pub node_id: Option<String>,
}

/// Validate a graph and return all lint warnings.
pub fn validate(graph: &Graph) -> Vec<LintWarning> {
    let mut warnings = Vec::new();
    warnings.extend(check_empty_graph(graph));
    warnings.extend(check_no_start_node(graph));
    warnings.extend(check_no_exit_node(graph));
    warnings.extend(check_multiple_start_nodes(graph));
    warnings.extend(check_unreachable_node(graph));
    warnings.extend(check_dead_end_node(graph));
    warnings.extend(check_self_loop(graph));
    warnings.extend(check_orphan_node(graph));
    warnings.extend(check_missing_edge_target(graph));
    warnings.extend(check_conditional_without_condition(graph));
    warnings.extend(check_duplicate_edge(graph));
    warnings.extend(check_missing_label(graph));
    warnings
}

/// Rule: empty_graph (Info) - Graph has no nodes.
fn check_empty_graph(graph: &Graph) -> Vec<LintWarning> {
    if graph.nodes.is_empty() {
        vec![LintWarning {
            rule: "empty_graph".to_string(),
            severity: Severity::Info,
            message: "Graph has no nodes".to_string(),
            node_id: None,
        }]
    } else {
        vec![]
    }
}

/// Rule: no_start_node (Error) - Graph has no Start-type node.
fn check_no_start_node(graph: &Graph) -> Vec<LintWarning> {
    if graph.nodes.is_empty() {
        return vec![];
    }
    let has_start = graph.nodes.iter().any(|n| n.node_type == NodeType::Start);
    if !has_start {
        vec![LintWarning {
            rule: "no_start_node".to_string(),
            severity: Severity::Error,
            message: "Graph has no Start-type node".to_string(),
            node_id: None,
        }]
    } else {
        vec![]
    }
}

/// Rule: no_exit_node (Warning) - Graph has no Exit-type node.
fn check_no_exit_node(graph: &Graph) -> Vec<LintWarning> {
    if graph.nodes.is_empty() {
        return vec![];
    }
    let has_exit = graph.nodes.iter().any(|n| n.node_type == NodeType::Exit);
    if !has_exit {
        vec![LintWarning {
            rule: "no_exit_node".to_string(),
            severity: Severity::Warning,
            message: "Graph has no Exit-type node".to_string(),
            node_id: None,
        }]
    } else {
        vec![]
    }
}

/// Rule: multiple_start_nodes (Error) - More than one Start node.
fn check_multiple_start_nodes(graph: &Graph) -> Vec<LintWarning> {
    let start_nodes: Vec<&GraphNode> = graph
        .nodes
        .iter()
        .filter(|n| n.node_type == NodeType::Start)
        .collect();
    if start_nodes.len() > 1 {
        start_nodes
            .iter()
            .map(|n| LintWarning {
                rule: "multiple_start_nodes".to_string(),
                severity: Severity::Error,
                message: format!("Multiple Start nodes found: '{}'", n.id),
                node_id: Some(n.id.clone()),
            })
            .collect()
    } else {
        vec![]
    }
}

/// Rule: unreachable_node (Warning) - Node has no incoming edges (except Start nodes).
fn check_unreachable_node(graph: &Graph) -> Vec<LintWarning> {
    let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    let targets_with_incoming: HashSet<&str> = graph
        .edges
        .iter()
        .filter(|e| node_ids.contains(e.to.as_str()))
        .map(|e| e.to.as_str())
        .collect();

    graph
        .nodes
        .iter()
        .filter(|n| n.node_type != NodeType::Start)
        .filter(|n| !targets_with_incoming.contains(n.id.as_str()))
        .map(|n| LintWarning {
            rule: "unreachable_node".to_string(),
            severity: Severity::Warning,
            message: format!("Node '{}' has no incoming edges", n.id),
            node_id: Some(n.id.clone()),
        })
        .collect()
}

/// Rule: dead_end_node (Warning) - Node has no outgoing edges (except Exit nodes).
fn check_dead_end_node(graph: &Graph) -> Vec<LintWarning> {
    let sources_with_outgoing: HashSet<&str> =
        graph.edges.iter().map(|e| e.from.as_str()).collect();

    graph
        .nodes
        .iter()
        .filter(|n| n.node_type != NodeType::Exit)
        .filter(|n| !sources_with_outgoing.contains(n.id.as_str()))
        .map(|n| LintWarning {
            rule: "dead_end_node".to_string(),
            severity: Severity::Warning,
            message: format!("Node '{}' has no outgoing edges", n.id),
            node_id: Some(n.id.clone()),
        })
        .collect()
}

/// Rule: self_loop (Warning) - Edge from a node to itself.
fn check_self_loop(graph: &Graph) -> Vec<LintWarning> {
    graph
        .edges
        .iter()
        .filter(|e| e.from == e.to)
        .map(|e| LintWarning {
            rule: "self_loop".to_string(),
            severity: Severity::Warning,
            message: format!("Edge from '{}' to itself", e.from),
            node_id: Some(e.from.clone()),
        })
        .collect()
}

/// Rule: orphan_node (Warning) - Node with no edges at all (neither in nor out).
fn check_orphan_node(graph: &Graph) -> Vec<LintWarning> {
    let nodes_in_edges: HashSet<&str> = graph
        .edges
        .iter()
        .flat_map(|e| [e.from.as_str(), e.to.as_str()])
        .collect();

    graph
        .nodes
        .iter()
        .filter(|n| !nodes_in_edges.contains(n.id.as_str()))
        .map(|n| LintWarning {
            rule: "orphan_node".to_string(),
            severity: Severity::Warning,
            message: format!("Node '{}' has no edges", n.id),
            node_id: Some(n.id.clone()),
        })
        .collect()
}

/// Rule: missing_edge_target (Error) - Edge references a node ID that doesn't exist.
fn check_missing_edge_target(graph: &Graph) -> Vec<LintWarning> {
    let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    let mut warnings = Vec::new();

    for edge in &graph.edges {
        if !node_ids.contains(edge.from.as_str()) {
            warnings.push(LintWarning {
                rule: "missing_edge_target".to_string(),
                severity: Severity::Error,
                message: format!("Edge references unknown source node '{}'", edge.from),
                node_id: None,
            });
        }
        if !node_ids.contains(edge.to.as_str()) {
            warnings.push(LintWarning {
                rule: "missing_edge_target".to_string(),
                severity: Severity::Error,
                message: format!("Edge references unknown target node '{}'", edge.to),
                node_id: None,
            });
        }
    }

    warnings
}

/// Rule: conditional_without_condition (Warning) - Conditional node has outgoing edges without conditions.
fn check_conditional_without_condition(graph: &Graph) -> Vec<LintWarning> {
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
        .map(|e| LintWarning {
            rule: "conditional_without_condition".to_string(),
            severity: Severity::Warning,
            message: format!(
                "Conditional node '{}' has outgoing edge to '{}' without a condition",
                e.from, e.to
            ),
            node_id: Some(e.from.clone()),
        })
        .collect()
}

/// Rule: duplicate_edge (Warning) - Multiple edges from same source to same target.
fn check_duplicate_edge(graph: &Graph) -> Vec<LintWarning> {
    let mut seen: HashMap<(&str, &str), usize> = HashMap::new();
    for edge in &graph.edges {
        *seen
            .entry((edge.from.as_str(), edge.to.as_str()))
            .or_insert(0) += 1;
    }

    seen.into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|((from, to), count)| LintWarning {
            rule: "duplicate_edge".to_string(),
            severity: Severity::Warning,
            message: format!(
                "Duplicate edge from '{}' to '{}' ({} occurrences)",
                from, to, count
            ),
            node_id: Some(from.to_string()),
        })
        .collect()
}

/// Rule: missing_label (Info) - Node has no label attribute.
fn check_missing_label(graph: &Graph) -> Vec<LintWarning> {
    graph
        .nodes
        .iter()
        .filter(|n| n.label.is_none())
        .map(|n| LintWarning {
            rule: "missing_label".to_string(),
            severity: Severity::Info,
            message: format!("Node '{}' has no label", n.id),
            node_id: Some(n.id.clone()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::GraphEdge;
    use super::*;

    /// Helper to create a simple GraphNode with defaults.
    fn make_node(id: &str, node_type: NodeType) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
            label: Some(id.to_string()),
            attrs: HashMap::new(),
        }
    }

    /// Helper to create a simple GraphEdge.
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

    /// Helper to create a Graph with just nodes and edges.
    fn make_graph_with(name: Option<&str>, nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> Graph {
        Graph {
            name: name.map(|s| s.to_string()),
            nodes,
            edges,
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
            graph_attrs: HashMap::new(),
        }
    }

    fn has_rule(warnings: &[LintWarning], rule: &str) -> bool {
        warnings.iter().any(|w| w.rule == rule)
    }

    fn count_rule(warnings: &[LintWarning], rule: &str) -> usize {
        warnings.iter().filter(|w| w.rule == rule).count()
    }

    fn rules_with_severity(warnings: &[LintWarning], severity: Severity) -> Vec<&LintWarning> {
        warnings.iter().filter(|w| w.severity == severity).collect()
    }

    #[test]
    fn empty_graph_produces_info() {
        let graph = make_graph_with(None, vec![], vec![]);
        let warnings = validate(&graph);
        assert!(has_rule(&warnings, "empty_graph"));
        let w = warnings.iter().find(|w| w.rule == "empty_graph").unwrap();
        assert_eq!(w.severity, Severity::Info);
    }

    #[test]
    fn no_start_node_produces_error() {
        let graph = make_graph_with(None, vec![make_node("a", NodeType::Generic)], vec![]);
        let warnings = validate(&graph);
        assert!(has_rule(&warnings, "no_start_node"));
        let w = warnings.iter().find(|w| w.rule == "no_start_node").unwrap();
        assert_eq!(w.severity, Severity::Error);
    }

    #[test]
    fn no_exit_node_produces_warning() {
        let graph = make_graph_with(None, vec![make_node("start", NodeType::Start)], vec![]);
        let warnings = validate(&graph);
        assert!(has_rule(&warnings, "no_exit_node"));
        let w = warnings.iter().find(|w| w.rule == "no_exit_node").unwrap();
        assert_eq!(w.severity, Severity::Warning);
    }

    #[test]
    fn multiple_start_nodes_produces_error() {
        let graph = make_graph_with(
            None,
            vec![
                make_node("s1", NodeType::Start),
                make_node("s2", NodeType::Start),
            ],
            vec![],
        );
        let warnings = validate(&graph);
        assert!(has_rule(&warnings, "multiple_start_nodes"));
        assert_eq!(count_rule(&warnings, "multiple_start_nodes"), 2);
    }

    #[test]
    fn unreachable_node_detected() {
        let graph = make_graph_with(
            None,
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("b", NodeType::Generic),
            ],
            vec![make_edge("start", "a")],
        );
        let warnings = validate(&graph);
        assert!(has_rule(&warnings, "unreachable_node"));
        let unreachable: Vec<_> = warnings
            .iter()
            .filter(|w| w.rule == "unreachable_node")
            .collect();
        assert_eq!(unreachable.len(), 1);
        assert_eq!(unreachable[0].node_id.as_deref(), Some("b"));
    }

    #[test]
    fn dead_end_node_detected() {
        let graph = make_graph_with(
            None,
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "a")],
        );
        let warnings = validate(&graph);
        // "a" has no outgoing edges and is not Exit
        assert!(has_rule(&warnings, "dead_end_node"));
        let dead_ends: Vec<_> = warnings
            .iter()
            .filter(|w| w.rule == "dead_end_node")
            .collect();
        assert!(dead_ends.iter().any(|w| w.node_id.as_deref() == Some("a")));
    }

    #[test]
    fn self_loop_detected() {
        let graph = make_graph_with(
            None,
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
            ],
            vec![make_edge("start", "a"), make_edge("a", "a")],
        );
        let warnings = validate(&graph);
        assert!(has_rule(&warnings, "self_loop"));
    }

    #[test]
    fn valid_graph_no_errors() {
        let graph = make_graph_with(
            Some("valid"),
            vec![
                make_node("start", NodeType::Start),
                make_node("process", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "process"), make_edge("process", "exit")],
        );
        let warnings = validate(&graph);
        let errors = rules_with_severity(&warnings, Severity::Error);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn orphan_node_detected() {
        let graph = make_graph_with(
            None,
            vec![
                make_node("start", NodeType::Start),
                make_node("orphan", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let warnings = validate(&graph);
        assert!(has_rule(&warnings, "orphan_node"));
        let orphans: Vec<_> = warnings
            .iter()
            .filter(|w| w.rule == "orphan_node")
            .collect();
        assert!(
            orphans
                .iter()
                .any(|w| w.node_id.as_deref() == Some("orphan"))
        );
    }

    #[test]
    fn missing_edge_target_detected() {
        let graph = make_graph_with(
            None,
            vec![make_node("start", NodeType::Start)],
            vec![make_edge("start", "nonexistent")],
        );
        let warnings = validate(&graph);
        assert!(has_rule(&warnings, "missing_edge_target"));
        let missing: Vec<_> = warnings
            .iter()
            .filter(|w| w.rule == "missing_edge_target")
            .collect();
        assert!(missing[0].severity == Severity::Error);
    }

    #[test]
    fn conditional_without_condition_detected() {
        let graph = make_graph_with(
            None,
            vec![
                make_node("start", NodeType::Start),
                make_node("cond", NodeType::Conditional),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "cond"),
                make_edge("cond", "a"), // no condition on edge from conditional
                make_edge("a", "exit"),
            ],
        );
        let warnings = validate(&graph);
        assert!(has_rule(&warnings, "conditional_without_condition"));
    }

    #[test]
    fn duplicate_edge_detected() {
        let graph = make_graph_with(
            None,
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
            ],
            vec![make_edge("start", "a"), make_edge("start", "a")],
        );
        let warnings = validate(&graph);
        assert!(has_rule(&warnings, "duplicate_edge"));
    }

    #[test]
    fn missing_label_detected() {
        let graph = make_graph_with(
            None,
            vec![GraphNode {
                id: "nolabel".to_string(),
                node_type: NodeType::Start,
                label: None,
                attrs: HashMap::new(),
            }],
            vec![],
        );
        let warnings = validate(&graph);
        assert!(has_rule(&warnings, "missing_label"));
    }
}
