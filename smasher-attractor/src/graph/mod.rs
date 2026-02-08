// ABOUTME: Graph resolution module that transforms DOT AST into a typed semantic graph.
// ABOUTME: Maps DOT shapes to node types, extracts edge metadata, and provides graph queries.

pub mod validation;

use std::collections::HashMap;
use std::time::Duration;

use crate::dot::{DotAttr, DotGraph, DotStatement, DotValue};

/// Semantic node type determined by the DOT `shape` attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeType {
    /// Entry point of the pipeline.
    Start,
    /// Exit point (terminal node).
    Exit,
    /// Code generation node (runs an agent session).
    Codergen,
    /// Conditional branching node.
    Conditional,
    /// Tool execution node.
    Tool,
    /// Human interaction node.
    Interviewer,
    /// Parallel fan-out node.
    Parallel,
    /// Manager/coordinator node.
    Manager,
    /// Generic processing node.
    Generic,
}

/// A typed attribute value extracted from the DOT AST.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeAttrValue {
    String(String),
    Number(f64),
    Duration(Duration),
    Bool(bool),
}

/// A resolved graph node with semantic type and attributes.
#[derive(Debug, Clone)]
pub struct GraphNode {
    pub id: String,
    pub node_type: NodeType,
    pub label: Option<String>,
    pub attrs: HashMap<String, NodeAttrValue>,
}

/// A resolved graph edge with extracted metadata.
#[derive(Debug, Clone)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
    pub condition: Option<String>,
    pub priority: Option<i32>,
    pub attrs: HashMap<String, NodeAttrValue>,
}

/// A fully resolved semantic graph with typed nodes and edges.
#[derive(Debug, Clone)]
pub struct Graph {
    pub name: Option<String>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub default_node_attrs: HashMap<String, NodeAttrValue>,
    pub default_edge_attrs: HashMap<String, NodeAttrValue>,
}

/// Errors that can occur during DOT AST to Graph resolution.
#[derive(Debug, thiserror::Error)]
pub enum ResolutionError {
    #[error("duplicate node '{id}'")]
    DuplicateNode { id: String },
    #[error("invalid attribute value for '{key}': {message}")]
    InvalidAttribute { key: String, message: String },
}

impl Graph {
    /// Find a node by its identifier.
    pub fn node(&self, id: &str) -> Option<&GraphNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Return all edges leaving the given node.
    pub fn edges_from(&self, id: &str) -> Vec<&GraphEdge> {
        self.edges.iter().filter(|e| e.from == id).collect()
    }

    /// Return all edges entering the given node.
    pub fn edges_to(&self, id: &str) -> Vec<&GraphEdge> {
        self.edges.iter().filter(|e| e.to == id).collect()
    }

    /// Return all nodes of type Start.
    pub fn start_nodes(&self) -> Vec<&GraphNode> {
        self.nodes
            .iter()
            .filter(|n| n.node_type == NodeType::Start)
            .collect()
    }

    /// Return all nodes of type Exit.
    pub fn exit_nodes(&self) -> Vec<&GraphNode> {
        self.nodes
            .iter()
            .filter(|n| n.node_type == NodeType::Exit)
            .collect()
    }

    /// Return the IDs of all nodes reachable via outgoing edges from the given node.
    pub fn successors(&self, id: &str) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|e| e.from == id)
            .map(|e| e.to.as_str())
            .collect()
    }

    /// Return the IDs of all nodes that have edges leading to the given node.
    pub fn predecessors(&self, id: &str) -> Vec<&str> {
        self.edges
            .iter()
            .filter(|e| e.to == id)
            .map(|e| e.from.as_str())
            .collect()
    }
}

/// Convert a DotValue into a NodeAttrValue.
fn convert_value(value: &DotValue) -> NodeAttrValue {
    match value {
        DotValue::String(s) => NodeAttrValue::String(s.clone()),
        DotValue::Number(n) => NodeAttrValue::Number(*n),
        DotValue::Duration(d) => NodeAttrValue::Duration(*d),
        DotValue::Bool(b) => NodeAttrValue::Bool(*b),
    }
}

/// Convert a list of DotAttr into a HashMap of NodeAttrValue.
fn convert_attrs(attrs: &[DotAttr]) -> HashMap<String, NodeAttrValue> {
    attrs
        .iter()
        .map(|a| (a.key.clone(), convert_value(&a.value)))
        .collect()
}

/// Determine the NodeType from a shape string.
fn node_type_from_shape(shape: &str) -> NodeType {
    match shape {
        "circle" | "point" => NodeType::Start,
        "doublecircle" => NodeType::Exit,
        "box" | "rectangle" => NodeType::Codergen,
        "diamond" => NodeType::Conditional,
        "hexagon" => NodeType::Tool,
        "oval" | "ellipse" => NodeType::Interviewer,
        "parallelogram" => NodeType::Parallel,
        "house" => NodeType::Manager,
        _ => NodeType::Generic,
    }
}

/// Look up a shape string from an attribute map, falling back to defaults.
fn resolve_shape(
    node_attrs: &HashMap<String, NodeAttrValue>,
    default_attrs: &HashMap<String, NodeAttrValue>,
) -> Option<String> {
    let shape_val = node_attrs
        .get("shape")
        .or_else(|| default_attrs.get("shape"));

    match shape_val {
        Some(NodeAttrValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Look up a label string from an attribute map.
fn extract_label(attrs: &HashMap<String, NodeAttrValue>) -> Option<String> {
    match attrs.get("label") {
        Some(NodeAttrValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Extract the condition from edge attributes. Checks `condition` first, then `label`.
fn extract_condition(attrs: &HashMap<String, NodeAttrValue>) -> Option<String> {
    if let Some(NodeAttrValue::String(s)) = attrs.get("condition") {
        return Some(s.clone());
    }
    // Fall back to label if no explicit condition attribute.
    match attrs.get("label") {
        Some(NodeAttrValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Extract priority from edge attributes.
fn extract_priority(attrs: &HashMap<String, NodeAttrValue>) -> Result<Option<i32>, ResolutionError> {
    match attrs.get("priority") {
        Some(NodeAttrValue::Number(n)) => Ok(Some(*n as i32)),
        Some(other) => Err(ResolutionError::InvalidAttribute {
            key: "priority".to_string(),
            message: format!("expected a number, got {other:?}"),
        }),
        None => Ok(None),
    }
}

/// Resolve a DOT AST into a typed semantic Graph.
///
/// Processes default attributes, explicit node/edge declarations, and auto-creates
/// nodes that are referenced in edges but not explicitly declared.
pub fn resolve(dot_graph: &DotGraph) -> Result<Graph, ResolutionError> {
    let mut default_node_attrs: HashMap<String, NodeAttrValue> = HashMap::new();
    let mut default_edge_attrs: HashMap<String, NodeAttrValue> = HashMap::new();
    let mut nodes: Vec<GraphNode> = Vec::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut seen_node_ids: HashMap<String, usize> = HashMap::new();

    // First pass: collect defaults.
    for stmt in &dot_graph.statements {
        match stmt {
            DotStatement::DefaultNode(attrs) => {
                for attr in attrs {
                    default_node_attrs.insert(attr.key.clone(), convert_value(&attr.value));
                }
            }
            DotStatement::DefaultEdge(attrs) => {
                for attr in attrs {
                    default_edge_attrs.insert(attr.key.clone(), convert_value(&attr.value));
                }
            }
            _ => {}
        }
    }

    // Second pass: process nodes.
    for stmt in &dot_graph.statements {
        if let DotStatement::Node(dot_node) = stmt {
            if seen_node_ids.contains_key(&dot_node.id) {
                return Err(ResolutionError::DuplicateNode {
                    id: dot_node.id.clone(),
                });
            }

            let all_attrs = convert_attrs(&dot_node.attrs);
            let shape = resolve_shape(&all_attrs, &default_node_attrs);
            let node_type = shape
                .as_deref()
                .map(node_type_from_shape)
                .unwrap_or(NodeType::Generic);
            let label = extract_label(&all_attrs);

            // Store non-reserved attributes.
            let mut extra_attrs = HashMap::new();
            for (k, v) in &all_attrs {
                if k != "shape" && k != "label" {
                    extra_attrs.insert(k.clone(), v.clone());
                }
            }

            let idx = nodes.len();
            seen_node_ids.insert(dot_node.id.clone(), idx);
            nodes.push(GraphNode {
                id: dot_node.id.clone(),
                node_type,
                label,
                attrs: extra_attrs,
            });
        }
    }

    // Third pass: process edges.
    for stmt in &dot_graph.statements {
        if let DotStatement::Edge(dot_edge) = stmt {
            let all_attrs = convert_attrs(&dot_edge.attrs);
            let label = extract_label(&all_attrs);
            let condition = extract_condition(&all_attrs);
            let priority = extract_priority(&all_attrs)?;

            // Store non-reserved attributes.
            let mut extra_attrs = HashMap::new();
            for (k, v) in &all_attrs {
                if k != "label" && k != "condition" && k != "priority" {
                    extra_attrs.insert(k.clone(), v.clone());
                }
            }

            edges.push(GraphEdge {
                from: dot_edge.from.clone(),
                to: dot_edge.to.clone(),
                label,
                condition,
                priority,
                attrs: extra_attrs,
            });
        }
    }

    // Auto-create nodes referenced in edges but not explicitly declared.
    for edge in &edges {
        for node_id in [&edge.from, &edge.to] {
            if !seen_node_ids.contains_key(node_id) {
                let shape = resolve_shape(&HashMap::new(), &default_node_attrs);
                let node_type = shape
                    .as_deref()
                    .map(node_type_from_shape)
                    .unwrap_or(NodeType::Generic);

                let idx = nodes.len();
                seen_node_ids.insert(node_id.clone(), idx);
                nodes.push(GraphNode {
                    id: node_id.clone(),
                    node_type,
                    label: None,
                    attrs: HashMap::new(),
                });
            }
        }
    }

    Ok(Graph {
        name: dot_graph.name.clone(),
        nodes,
        edges,
        default_node_attrs,
        default_edge_attrs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dot::{DotAttr, DotEdge, DotGraph, DotNode, DotStatement, DotValue};

    /// Helper: build a minimal DotGraph with the given statements.
    fn make_graph(statements: Vec<DotStatement>) -> DotGraph {
        DotGraph {
            name: None,
            is_digraph: true,
            statements,
        }
    }

    /// Helper: build a DotNode with a single shape attribute.
    fn node_with_shape(id: &str, shape: &str) -> DotStatement {
        DotStatement::Node(DotNode {
            id: id.to_string(),
            attrs: vec![DotAttr {
                key: "shape".to_string(),
                value: DotValue::String(shape.to_string()),
            }],
        })
    }

    /// Helper: build a plain DotNode with no attributes.
    fn plain_node(id: &str) -> DotStatement {
        DotStatement::Node(DotNode {
            id: id.to_string(),
            attrs: vec![],
        })
    }

    /// Helper: build a DotEdge with no attributes.
    fn plain_edge(from: &str, to: &str) -> DotStatement {
        DotStatement::Edge(DotEdge {
            from: from.to_string(),
            to: to.to_string(),
            attrs: vec![],
        })
    }

    // ---- Test 1: Resolve empty graph ----
    #[test]
    fn resolve_empty_graph() {
        let dot = make_graph(vec![]);
        let g = resolve(&dot).unwrap();
        assert!(g.nodes.is_empty());
        assert!(g.edges.is_empty());
        assert!(g.default_node_attrs.is_empty());
        assert!(g.default_edge_attrs.is_empty());
    }

    // ---- Test 2: Single node with shape resolves to correct NodeType ----
    #[test]
    fn resolve_single_node_with_shape() {
        let dot = make_graph(vec![node_with_shape("start", "circle")]);
        let g = resolve(&dot).unwrap();
        assert_eq!(g.nodes.len(), 1);
        assert_eq!(g.nodes[0].id, "start");
        assert_eq!(g.nodes[0].node_type, NodeType::Start);
    }

    // ---- Test 3: All shape mappings ----
    #[test]
    fn resolve_all_shape_mappings() {
        let cases = vec![
            ("circle", NodeType::Start),
            ("point", NodeType::Start),
            ("doublecircle", NodeType::Exit),
            ("box", NodeType::Codergen),
            ("rectangle", NodeType::Codergen),
            ("diamond", NodeType::Conditional),
            ("hexagon", NodeType::Tool),
            ("oval", NodeType::Interviewer),
            ("ellipse", NodeType::Interviewer),
            ("parallelogram", NodeType::Parallel),
            ("house", NodeType::Manager),
            ("unknownshape", NodeType::Generic),
        ];

        for (shape, expected_type) in cases {
            let dot = make_graph(vec![node_with_shape("n", shape)]);
            let g = resolve(&dot).unwrap();
            assert_eq!(
                g.nodes[0].node_type, expected_type,
                "shape '{shape}' should map to {expected_type:?}"
            );
        }
    }

    // ---- Test 4: Edge extracts from/to ----
    #[test]
    fn resolve_edge_from_to() {
        let dot = make_graph(vec![
            plain_node("a"),
            plain_node("b"),
            plain_edge("a", "b"),
        ]);
        let g = resolve(&dot).unwrap();
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].from, "a");
        assert_eq!(g.edges[0].to, "b");
    }

    // ---- Test 5: Edge with condition attribute ----
    #[test]
    fn resolve_edge_with_condition() {
        let dot = make_graph(vec![
            plain_node("a"),
            plain_node("b"),
            DotStatement::Edge(DotEdge {
                from: "a".to_string(),
                to: "b".to_string(),
                attrs: vec![DotAttr {
                    key: "condition".to_string(),
                    value: DotValue::String("x > 5".to_string()),
                }],
            }),
        ]);
        let g = resolve(&dot).unwrap();
        assert_eq!(g.edges[0].condition, Some("x > 5".to_string()));
    }

    // ---- Test 6: Edge with priority ----
    #[test]
    fn resolve_edge_with_priority() {
        let dot = make_graph(vec![
            plain_node("a"),
            plain_node("b"),
            DotStatement::Edge(DotEdge {
                from: "a".to_string(),
                to: "b".to_string(),
                attrs: vec![DotAttr {
                    key: "priority".to_string(),
                    value: DotValue::Number(10.0),
                }],
            }),
        ]);
        let g = resolve(&dot).unwrap();
        assert_eq!(g.edges[0].priority, Some(10));
    }

    // ---- Test 7: Default node attributes applied ----
    #[test]
    fn default_node_attrs_applied() {
        let dot = make_graph(vec![
            DotStatement::DefaultNode(vec![DotAttr {
                key: "shape".to_string(),
                value: DotValue::String("diamond".to_string()),
            }]),
            plain_node("x"),
        ]);
        let g = resolve(&dot).unwrap();
        assert_eq!(g.nodes[0].node_type, NodeType::Conditional);
    }

    // ---- Test 8: Auto-create nodes from edges ----
    #[test]
    fn auto_create_nodes_from_edges() {
        let dot = make_graph(vec![plain_edge("alpha", "beta")]);
        let g = resolve(&dot).unwrap();
        assert_eq!(g.nodes.len(), 2);
        assert!(g.node("alpha").is_some());
        assert!(g.node("beta").is_some());
    }

    // ---- Test 9: Node label extracted ----
    #[test]
    fn node_label_extracted() {
        let dot = make_graph(vec![DotStatement::Node(DotNode {
            id: "n1".to_string(),
            attrs: vec![
                DotAttr {
                    key: "label".to_string(),
                    value: DotValue::String("My Node".to_string()),
                },
                DotAttr {
                    key: "shape".to_string(),
                    value: DotValue::String("box".to_string()),
                },
            ],
        })]);
        let g = resolve(&dot).unwrap();
        assert_eq!(g.nodes[0].label, Some("My Node".to_string()));
        assert_eq!(g.nodes[0].node_type, NodeType::Codergen);
    }

    // ---- Test 10: edges_from returns correct edges ----
    #[test]
    fn edges_from_returns_correct_edges() {
        let dot = make_graph(vec![
            plain_node("a"),
            plain_node("b"),
            plain_node("c"),
            plain_edge("a", "b"),
            plain_edge("a", "c"),
            plain_edge("b", "c"),
        ]);
        let g = resolve(&dot).unwrap();
        let from_a = g.edges_from("a");
        assert_eq!(from_a.len(), 2);
        assert!(from_a.iter().any(|e| e.to == "b"));
        assert!(from_a.iter().any(|e| e.to == "c"));

        let from_b = g.edges_from("b");
        assert_eq!(from_b.len(), 1);
        assert_eq!(from_b[0].to, "c");
    }

    // ---- Test 11: edges_to returns correct edges ----
    #[test]
    fn edges_to_returns_correct_edges() {
        let dot = make_graph(vec![
            plain_node("a"),
            plain_node("b"),
            plain_node("c"),
            plain_edge("a", "c"),
            plain_edge("b", "c"),
        ]);
        let g = resolve(&dot).unwrap();
        let to_c = g.edges_to("c");
        assert_eq!(to_c.len(), 2);
        assert!(to_c.iter().any(|e| e.from == "a"));
        assert!(to_c.iter().any(|e| e.from == "b"));
    }

    // ---- Test 12: start_nodes returns Start-type nodes ----
    #[test]
    fn start_nodes_returns_start_type() {
        let dot = make_graph(vec![
            node_with_shape("s1", "circle"),
            node_with_shape("s2", "point"),
            node_with_shape("other", "box"),
        ]);
        let g = resolve(&dot).unwrap();
        let starts = g.start_nodes();
        assert_eq!(starts.len(), 2);
        assert!(starts.iter().any(|n| n.id == "s1"));
        assert!(starts.iter().any(|n| n.id == "s2"));
    }

    // ---- Test 13: exit_nodes returns Exit-type nodes ----
    #[test]
    fn exit_nodes_returns_exit_type() {
        let dot = make_graph(vec![
            node_with_shape("e1", "doublecircle"),
            node_with_shape("other", "diamond"),
        ]);
        let g = resolve(&dot).unwrap();
        let exits = g.exit_nodes();
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].id, "e1");
    }

    // ---- Test 14: successors returns correct IDs ----
    #[test]
    fn successors_returns_correct_ids() {
        let dot = make_graph(vec![
            plain_node("a"),
            plain_node("b"),
            plain_node("c"),
            plain_edge("a", "b"),
            plain_edge("a", "c"),
        ]);
        let g = resolve(&dot).unwrap();
        let mut succs: Vec<&str> = g.successors("a");
        succs.sort();
        assert_eq!(succs, vec!["b", "c"]);
    }

    // ---- Test 15: predecessors returns correct IDs ----
    #[test]
    fn predecessors_returns_correct_ids() {
        let dot = make_graph(vec![
            plain_node("a"),
            plain_node("b"),
            plain_node("c"),
            plain_edge("a", "c"),
            plain_edge("b", "c"),
        ]);
        let g = resolve(&dot).unwrap();
        let mut preds: Vec<&str> = g.predecessors("c");
        preds.sort();
        assert_eq!(preds, vec!["a", "b"]);
    }

    // ---- Test 16: node() lookup by ID ----
    #[test]
    fn node_lookup_by_id() {
        let dot = make_graph(vec![
            node_with_shape("alpha", "circle"),
            node_with_shape("beta", "box"),
        ]);
        let g = resolve(&dot).unwrap();

        let alpha = g.node("alpha").expect("should find alpha");
        assert_eq!(alpha.node_type, NodeType::Start);

        let beta = g.node("beta").expect("should find beta");
        assert_eq!(beta.node_type, NodeType::Codergen);

        assert!(g.node("nonexistent").is_none());
    }

    // ---- Edge condition falls back to label ----
    #[test]
    fn edge_condition_falls_back_to_label() {
        let dot = make_graph(vec![
            plain_node("a"),
            plain_node("b"),
            DotStatement::Edge(DotEdge {
                from: "a".to_string(),
                to: "b".to_string(),
                attrs: vec![DotAttr {
                    key: "label".to_string(),
                    value: DotValue::String("yes".to_string()),
                }],
            }),
        ]);
        let g = resolve(&dot).unwrap();
        assert_eq!(g.edges[0].condition, Some("yes".to_string()));
        assert_eq!(g.edges[0].label, Some("yes".to_string()));
    }

    // ---- Condition attribute takes precedence over label ----
    #[test]
    fn edge_condition_prefers_condition_over_label() {
        let dot = make_graph(vec![
            plain_node("a"),
            plain_node("b"),
            DotStatement::Edge(DotEdge {
                from: "a".to_string(),
                to: "b".to_string(),
                attrs: vec![
                    DotAttr {
                        key: "condition".to_string(),
                        value: DotValue::String("x > 5".to_string()),
                    },
                    DotAttr {
                        key: "label".to_string(),
                        value: DotValue::String("display label".to_string()),
                    },
                ],
            }),
        ]);
        let g = resolve(&dot).unwrap();
        assert_eq!(g.edges[0].condition, Some("x > 5".to_string()));
        assert_eq!(g.edges[0].label, Some("display label".to_string()));
    }

    // ---- Duplicate node returns error ----
    #[test]
    fn duplicate_node_returns_error() {
        let dot = make_graph(vec![plain_node("x"), plain_node("x")]);
        let result = resolve(&dot);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("duplicate node 'x'"));
    }

    // ---- Graph name is preserved ----
    #[test]
    fn graph_name_preserved() {
        let dot = DotGraph {
            name: Some("MyPipeline".to_string()),
            is_digraph: true,
            statements: vec![],
        };
        let g = resolve(&dot).unwrap();
        assert_eq!(g.name, Some("MyPipeline".to_string()));
    }

    // ---- Node extra attributes are stored ----
    #[test]
    fn node_extra_attrs_stored() {
        let dot = make_graph(vec![DotStatement::Node(DotNode {
            id: "n".to_string(),
            attrs: vec![
                DotAttr {
                    key: "shape".to_string(),
                    value: DotValue::String("box".to_string()),
                },
                DotAttr {
                    key: "timeout".to_string(),
                    value: DotValue::Duration(Duration::from_secs(30)),
                },
                DotAttr {
                    key: "retries".to_string(),
                    value: DotValue::Number(3.0),
                },
            ],
        })]);
        let g = resolve(&dot).unwrap();
        assert_eq!(
            g.nodes[0].attrs.get("timeout"),
            Some(&NodeAttrValue::Duration(Duration::from_secs(30)))
        );
        assert_eq!(
            g.nodes[0].attrs.get("retries"),
            Some(&NodeAttrValue::Number(3.0))
        );
        // shape and label should not be in extra attrs
        assert!(g.nodes[0].attrs.get("shape").is_none());
    }

    // ---- Default edge attributes stored in graph ----
    #[test]
    fn default_edge_attrs_stored() {
        let dot = make_graph(vec![DotStatement::DefaultEdge(vec![DotAttr {
            key: "color".to_string(),
            value: DotValue::String("red".to_string()),
        }])]);
        let g = resolve(&dot).unwrap();
        assert_eq!(
            g.default_edge_attrs.get("color"),
            Some(&NodeAttrValue::String("red".to_string()))
        );
    }

    // ---- Auto-created nodes use default shape ----
    #[test]
    fn auto_created_nodes_use_default_shape() {
        let dot = make_graph(vec![
            DotStatement::DefaultNode(vec![DotAttr {
                key: "shape".to_string(),
                value: DotValue::String("hexagon".to_string()),
            }]),
            plain_edge("implicit_a", "implicit_b"),
        ]);
        let g = resolve(&dot).unwrap();
        assert_eq!(g.nodes.len(), 2);
        for node in &g.nodes {
            assert_eq!(node.node_type, NodeType::Tool);
        }
    }
}
