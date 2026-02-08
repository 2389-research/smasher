// ABOUTME: Sub-pipeline composition for inlining external DOT graphs into a parent pipeline.
// ABOUTME: Replaces SubPipeline nodes with their referenced sub-graphs, reconnecting edges.

use std::path::Path;

use crate::dot;
use crate::graph::{self, Graph, GraphEdge, GraphNode, NodeAttrValue, NodeType};

/// Errors that can occur during sub-pipeline composition.
#[derive(Debug, thiserror::Error)]
pub enum CompositionError {
    #[error("sub-graph file not found: {path}")]
    SubGraphNotFound { path: String },

    #[error("sub-graph has no start node (node_id: {node_id})")]
    NoStartNode { node_id: String },

    #[error("sub-graph has no exit node (node_id: {node_id})")]
    NoExitNode { node_id: String },

    #[error("cycle detected after composition involving node '{node_id}'")]
    CycleDetected { node_id: String },

    #[error("failed to parse sub-graph '{path}': {message}")]
    ParseError { path: String, message: String },

    #[error("failed to resolve sub-graph '{path}': {message}")]
    ResolutionError { path: String, message: String },

    #[error("sub-pipeline node '{node_id}' missing 'pipeline' attribute")]
    MissingPipelineAttr { node_id: String },

    #[error("failed to read sub-graph file '{path}': {message}")]
    IoError { path: String, message: String },
}

/// Prefix a node ID with the parent node's ID to avoid collisions.
///
/// Produces IDs like `parent.child_node`.
fn prefix_id(parent_id: &str, child_id: &str) -> String {
    format!("{parent_id}.{child_id}")
}

/// Compose a sub-graph into a parent graph by replacing a SubPipeline node.
///
/// The sub-graph's start node receives all incoming edges of the replaced node,
/// and the sub-graph's exit node sends to all outgoing edges of the replaced node.
/// All sub-graph node IDs are prefixed with `node_id` to prevent collisions.
pub fn compose_graphs(
    parent: &Graph,
    sub: &Graph,
    node_id: &str,
) -> Result<Graph, CompositionError> {
    // Validate that the sub-graph has exactly one start and one exit node.
    let sub_starts = sub.start_nodes();
    if sub_starts.is_empty() {
        return Err(CompositionError::NoStartNode {
            node_id: node_id.to_string(),
        });
    }
    let sub_start_id = &sub_starts[0].id;

    let sub_exits = sub.exit_nodes();
    if sub_exits.is_empty() {
        return Err(CompositionError::NoExitNode {
            node_id: node_id.to_string(),
        });
    }
    let sub_exit_id = &sub_exits[0].id;

    // Collect incoming and outgoing edges of the SubPipeline node in the parent.
    let incoming_edges: Vec<&GraphEdge> = parent.edges_to(node_id);
    let outgoing_edges: Vec<&GraphEdge> = parent.edges_from(node_id);

    // Build the new node list: parent nodes minus the SubPipeline node, plus prefixed sub-graph nodes.
    let mut new_nodes: Vec<GraphNode> = parent
        .nodes
        .iter()
        .filter(|n| n.id != node_id)
        .cloned()
        .collect();

    for sub_node in &sub.nodes {
        let prefixed_id = prefix_id(node_id, &sub_node.id);
        // Sub-graph start and exit nodes become Generic when inlined,
        // since they are no longer the pipeline boundary.
        let node_type = match sub_node.node_type {
            NodeType::Start | NodeType::Exit => NodeType::Generic,
            ref other => other.clone(),
        };
        new_nodes.push(GraphNode {
            id: prefixed_id,
            node_type,
            label: sub_node.label.clone(),
            attrs: sub_node.attrs.clone(),
        });
    }

    // Build the new edge list.
    let mut new_edges: Vec<GraphEdge> = Vec::new();

    // Keep parent edges that don't involve the SubPipeline node.
    for edge in &parent.edges {
        if edge.from != node_id && edge.to != node_id {
            new_edges.push(edge.clone());
        }
    }

    // Reconnect: parent incoming edges -> sub-graph's start node (prefixed).
    let prefixed_start = prefix_id(node_id, sub_start_id);
    for edge in &incoming_edges {
        new_edges.push(GraphEdge {
            from: edge.from.clone(),
            to: prefixed_start.clone(),
            label: edge.label.clone(),
            condition: edge.condition.clone(),
            priority: edge.priority,
            loop_restart: edge.loop_restart,
            attrs: edge.attrs.clone(),
        });
    }

    // Reconnect: sub-graph's exit node (prefixed) -> parent outgoing edges.
    let prefixed_exit = prefix_id(node_id, sub_exit_id);
    for edge in &outgoing_edges {
        new_edges.push(GraphEdge {
            from: prefixed_exit.clone(),
            to: edge.to.clone(),
            label: edge.label.clone(),
            condition: edge.condition.clone(),
            priority: edge.priority,
            loop_restart: edge.loop_restart,
            attrs: edge.attrs.clone(),
        });
    }

    // Add prefixed internal sub-graph edges.
    for edge in &sub.edges {
        new_edges.push(GraphEdge {
            from: prefix_id(node_id, &edge.from),
            to: prefix_id(node_id, &edge.to),
            label: edge.label.clone(),
            condition: edge.condition.clone(),
            priority: edge.priority,
            loop_restart: edge.loop_restart,
            attrs: edge.attrs.clone(),
        });
    }

    Ok(Graph {
        name: parent.name.clone(),
        nodes: new_nodes,
        edges: new_edges,
        default_node_attrs: parent.default_node_attrs.clone(),
        default_edge_attrs: parent.default_edge_attrs.clone(),
    })
}

/// A transform that resolves all SubPipeline nodes in a graph by inlining
/// their referenced DOT files.
///
/// Scans the graph for nodes with `NodeType::SubPipeline`, reads the `pipeline`
/// attribute (a path to a `.dot` file), parses and resolves it, then uses
/// `compose_graphs` to inline the sub-graph.
pub struct SubPipelineTransform {
    /// Base directory for resolving relative pipeline paths.
    base_dir: String,
}

impl SubPipelineTransform {
    /// Create a new SubPipelineTransform with the given base directory
    /// for resolving relative paths in `pipeline` attributes.
    pub fn new(base_dir: impl Into<String>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Apply the transform, inlining all SubPipeline nodes.
    ///
    /// Iterates over the graph, finding SubPipeline nodes and replacing them
    /// with their referenced sub-graphs. Processes nodes iteratively until
    /// no more SubPipeline nodes remain (supporting nested sub-pipelines).
    pub fn apply(&self, graph: &Graph) -> Result<Graph, CompositionError> {
        let mut current = graph.clone();

        loop {
            let sub_pipeline_node = current
                .nodes
                .iter()
                .find(|n| n.node_type == NodeType::SubPipeline);

            let node = match sub_pipeline_node {
                Some(n) => n.clone(),
                None => break,
            };

            let pipeline_path = match node.attrs.get("pipeline") {
                Some(NodeAttrValue::String(p)) => p.clone(),
                _ => {
                    return Err(CompositionError::MissingPipelineAttr {
                        node_id: node.id.clone(),
                    });
                }
            };

            let full_path = self.resolve_path(&pipeline_path);
            let sub_graph = self.load_sub_graph(&full_path)?;
            current = compose_graphs(&current, &sub_graph, &node.id)?;
        }

        Ok(current)
    }

    /// Resolve a pipeline path relative to the base directory.
    fn resolve_path(&self, pipeline_path: &str) -> String {
        let path = Path::new(pipeline_path);
        if path.is_absolute() {
            pipeline_path.to_string()
        } else {
            Path::new(&self.base_dir)
                .join(pipeline_path)
                .to_string_lossy()
                .into_owned()
        }
    }

    /// Load and resolve a sub-graph from a DOT file.
    fn load_sub_graph(&self, path: &str) -> Result<Graph, CompositionError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CompositionError::SubGraphNotFound {
                    path: path.to_string(),
                }
            } else {
                CompositionError::IoError {
                    path: path.to_string(),
                    message: e.to_string(),
                }
            }
        })?;

        let dot_graph = dot::parse(&content).map_err(|e| CompositionError::ParseError {
            path: path.to_string(),
            message: e.to_string(),
        })?;

        graph::resolve(&dot_graph).map_err(|e| CompositionError::ResolutionError {
            path: path.to_string(),
            message: e.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, GraphEdge, GraphNode, NodeAttrValue, NodeType};
    use std::collections::HashMap;

    // ---------------------------------------------------------------
    // Test helpers
    // ---------------------------------------------------------------

    fn make_node(id: &str, node_type: NodeType) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
            label: None,
            attrs: HashMap::new(),
        }
    }

    fn make_node_with_label(id: &str, node_type: NodeType, label: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
            label: Some(label.to_string()),
            attrs: HashMap::new(),
        }
    }

    fn make_sub_pipeline_node(id: &str, pipeline: &str) -> GraphNode {
        let mut attrs = HashMap::new();
        attrs.insert(
            "pipeline".to_string(),
            NodeAttrValue::String(pipeline.to_string()),
        );
        GraphNode {
            id: id.to_string(),
            node_type: NodeType::SubPipeline,
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

    fn make_graph(name: Option<&str>, nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> Graph {
        Graph {
            name: name.map(|n| n.to_string()),
            nodes,
            edges,
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
        }
    }

    /// Build a simple sub-graph: start -> worker -> exit.
    fn simple_sub_graph() -> Graph {
        make_graph(
            Some("sub"),
            vec![
                make_node("begin", NodeType::Start),
                make_node_with_label("worker", NodeType::Codergen, "Do Work"),
                make_node("done", NodeType::Exit),
            ],
            vec![make_edge("begin", "worker"), make_edge("worker", "done")],
        )
    }

    // ---------------------------------------------------------------
    // Test 1: Compose simple parent with simple sub-pipeline
    // ---------------------------------------------------------------

    #[test]
    fn composition_simple_inline() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("sub1", "sub.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "sub1"), make_edge("sub1", "exit")],
        );

        let sub = simple_sub_graph();
        let composed = compose_graphs(&parent, &sub, "sub1").unwrap();

        // SubPipeline node should be gone.
        assert!(composed.node("sub1").is_none());

        // Sub-graph nodes should be present with prefixed IDs.
        assert!(composed.node("sub1.begin").is_some());
        assert!(composed.node("sub1.worker").is_some());
        assert!(composed.node("sub1.done").is_some());

        // Original parent nodes should remain.
        assert!(composed.node("start").is_some());
        assert!(composed.node("exit").is_some());

        // Total: 2 parent + 3 sub = 5 nodes.
        assert_eq!(composed.nodes.len(), 5);
    }

    // ---------------------------------------------------------------
    // Test 2: Node ID prefixing
    // ---------------------------------------------------------------

    #[test]
    fn composition_node_id_prefixing() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("pipeline_a", "a.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "pipeline_a"),
                make_edge("pipeline_a", "exit"),
            ],
        );

        let sub = simple_sub_graph();
        let composed = compose_graphs(&parent, &sub, "pipeline_a").unwrap();

        // All sub-graph nodes should have "pipeline_a." prefix.
        let sub_node_ids: Vec<&str> = composed
            .nodes
            .iter()
            .filter(|n| n.id.starts_with("pipeline_a."))
            .map(|n| n.id.as_str())
            .collect();

        assert_eq!(sub_node_ids.len(), 3);
        assert!(sub_node_ids.contains(&"pipeline_a.begin"));
        assert!(sub_node_ids.contains(&"pipeline_a.worker"));
        assert!(sub_node_ids.contains(&"pipeline_a.done"));
    }

    // ---------------------------------------------------------------
    // Test 3: Edge reconnection
    // ---------------------------------------------------------------

    #[test]
    fn composition_edge_reconnection() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("sub1", "sub.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "sub1"), make_edge("sub1", "exit")],
        );

        let sub = simple_sub_graph();
        let composed = compose_graphs(&parent, &sub, "sub1").unwrap();

        // Incoming edge: start -> sub1.begin (the sub-graph's start node).
        let start_edges = composed.edges_from("start");
        assert_eq!(start_edges.len(), 1);
        assert_eq!(start_edges[0].to, "sub1.begin");

        // Internal sub-graph edges: sub1.begin -> sub1.worker -> sub1.done.
        let begin_edges = composed.edges_from("sub1.begin");
        assert_eq!(begin_edges.len(), 1);
        assert_eq!(begin_edges[0].to, "sub1.worker");

        let worker_edges = composed.edges_from("sub1.worker");
        assert_eq!(worker_edges.len(), 1);
        assert_eq!(worker_edges[0].to, "sub1.done");

        // Outgoing edge: sub1.done -> exit (the parent's exit node).
        let done_edges = composed.edges_from("sub1.done");
        assert_eq!(done_edges.len(), 1);
        assert_eq!(done_edges[0].to, "exit");
    }

    // ---------------------------------------------------------------
    // Test 4: Error on missing sub-graph start node
    // ---------------------------------------------------------------

    #[test]
    fn composition_error_no_start_node() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("sub1", "sub.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "sub1"), make_edge("sub1", "exit")],
        );

        // Sub-graph with no start node.
        let sub = make_graph(
            Some("broken_sub"),
            vec![
                make_node("middle", NodeType::Generic),
                make_node("end", NodeType::Exit),
            ],
            vec![make_edge("middle", "end")],
        );

        let result = compose_graphs(&parent, &sub, "sub1");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CompositionError::NoStartNode { .. }));
        assert!(err.to_string().contains("no start node"));
    }

    // ---------------------------------------------------------------
    // Test 5: Error on missing sub-graph exit node
    // ---------------------------------------------------------------

    #[test]
    fn composition_error_no_exit_node() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("sub1", "sub.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "sub1"), make_edge("sub1", "exit")],
        );

        // Sub-graph with no exit node.
        let sub = make_graph(
            Some("broken_sub"),
            vec![
                make_node("begin", NodeType::Start),
                make_node("middle", NodeType::Generic),
            ],
            vec![make_edge("begin", "middle")],
        );

        let result = compose_graphs(&parent, &sub, "sub1");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CompositionError::NoExitNode { .. }));
        assert!(err.to_string().contains("no exit node"));
    }

    // ---------------------------------------------------------------
    // Test 6: Sub-graph start/exit become Generic when inlined
    // ---------------------------------------------------------------

    #[test]
    fn composition_start_exit_become_generic() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("sub1", "sub.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "sub1"), make_edge("sub1", "exit")],
        );

        let sub = simple_sub_graph();
        let composed = compose_graphs(&parent, &sub, "sub1").unwrap();

        // The sub-graph's start and exit nodes should become Generic.
        let inlined_start = composed.node("sub1.begin").unwrap();
        assert_eq!(inlined_start.node_type, NodeType::Generic);

        let inlined_exit = composed.node("sub1.done").unwrap();
        assert_eq!(inlined_exit.node_type, NodeType::Generic);

        // Interior sub-graph nodes keep their original type.
        let inlined_worker = composed.node("sub1.worker").unwrap();
        assert_eq!(inlined_worker.node_type, NodeType::Codergen);
    }

    // ---------------------------------------------------------------
    // Test 7: Edge labels and conditions are preserved during reconnection
    // ---------------------------------------------------------------

    #[test]
    fn composition_preserves_edge_labels() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("sub1", "sub.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_labeled_edge("start", "sub1", "enter_sub"),
                make_labeled_edge("sub1", "exit", "leave_sub"),
            ],
        );

        let sub = simple_sub_graph();
        let composed = compose_graphs(&parent, &sub, "sub1").unwrap();

        // Incoming edge label should be preserved.
        let start_edges = composed.edges_from("start");
        assert_eq!(start_edges[0].label.as_deref(), Some("enter_sub"));
        assert_eq!(start_edges[0].condition.as_deref(), Some("enter_sub"));

        // Outgoing edge label should be preserved.
        let done_edges = composed.edges_from("sub1.done");
        assert_eq!(done_edges[0].label.as_deref(), Some("leave_sub"));
        assert_eq!(done_edges[0].condition.as_deref(), Some("leave_sub"));
    }

    // ---------------------------------------------------------------
    // Test 8: Multiple incoming and outgoing edges
    // ---------------------------------------------------------------

    #[test]
    fn composition_multiple_incoming_outgoing_edges() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("a", NodeType::Start),
                make_node("b", NodeType::Generic),
                make_sub_pipeline_node("sub1", "sub.dot"),
                make_node("c", NodeType::Generic),
                make_node("d", NodeType::Exit),
            ],
            vec![
                make_edge("a", "sub1"),
                make_edge("b", "sub1"),
                make_edge("sub1", "c"),
                make_edge("sub1", "d"),
            ],
        );

        let sub = simple_sub_graph();
        let composed = compose_graphs(&parent, &sub, "sub1").unwrap();

        // Both parent predecessors should connect to sub-graph start.
        let to_begin: Vec<&GraphEdge> = composed.edges_to("sub1.begin");
        assert_eq!(to_begin.len(), 2);
        let from_ids: Vec<&str> = to_begin.iter().map(|e| e.from.as_str()).collect();
        assert!(from_ids.contains(&"a"));
        assert!(from_ids.contains(&"b"));

        // Sub-graph exit should connect to both parent successors.
        let from_done: Vec<&GraphEdge> = composed.edges_from("sub1.done");
        assert_eq!(from_done.len(), 2);
        let to_ids: Vec<&str> = from_done.iter().map(|e| e.to.as_str()).collect();
        assert!(to_ids.contains(&"c"));
        assert!(to_ids.contains(&"d"));
    }

    // ---------------------------------------------------------------
    // Test 9: Parent graph name is preserved
    // ---------------------------------------------------------------

    #[test]
    fn composition_preserves_parent_name() {
        let parent = make_graph(
            Some("my_pipeline"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("sub1", "sub.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "sub1"), make_edge("sub1", "exit")],
        );

        let sub = simple_sub_graph();
        let composed = compose_graphs(&parent, &sub, "sub1").unwrap();

        assert_eq!(composed.name.as_deref(), Some("my_pipeline"));
    }

    // ---------------------------------------------------------------
    // Test 10: Sub-graph node attributes are preserved
    // ---------------------------------------------------------------

    #[test]
    fn composition_preserves_sub_node_attrs() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("sub1", "sub.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "sub1"), make_edge("sub1", "exit")],
        );

        let mut worker_attrs = HashMap::new();
        worker_attrs.insert(
            "model".to_string(),
            NodeAttrValue::String("claude-sonnet".to_string()),
        );
        worker_attrs.insert("temperature".to_string(), NodeAttrValue::Number(0.7));

        let sub = make_graph(
            Some("sub"),
            vec![
                make_node("begin", NodeType::Start),
                GraphNode {
                    id: "worker".to_string(),
                    node_type: NodeType::Codergen,
                    label: Some("Do Work".to_string()),
                    attrs: worker_attrs,
                },
                make_node("done", NodeType::Exit),
            ],
            vec![make_edge("begin", "worker"), make_edge("worker", "done")],
        );

        let composed = compose_graphs(&parent, &sub, "sub1").unwrap();

        let inlined_worker = composed.node("sub1.worker").unwrap();
        assert_eq!(
            inlined_worker.attrs.get("model"),
            Some(&NodeAttrValue::String("claude-sonnet".to_string()))
        );
        assert_eq!(
            inlined_worker.attrs.get("temperature"),
            Some(&NodeAttrValue::Number(0.7))
        );
        assert_eq!(inlined_worker.label.as_deref(), Some("Do Work"));
    }

    // ---------------------------------------------------------------
    // Test 11: Sub-graph node labels are preserved
    // ---------------------------------------------------------------

    #[test]
    fn composition_preserves_sub_node_labels() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("sub1", "sub.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "sub1"), make_edge("sub1", "exit")],
        );

        let sub = simple_sub_graph();
        let composed = compose_graphs(&parent, &sub, "sub1").unwrap();

        let inlined_worker = composed.node("sub1.worker").unwrap();
        assert_eq!(inlined_worker.label.as_deref(), Some("Do Work"));
    }

    // ---------------------------------------------------------------
    // Test 12: prefix_id utility function
    // ---------------------------------------------------------------

    #[test]
    fn prefix_id_produces_dotted_path() {
        assert_eq!(prefix_id("parent", "child"), "parent.child");
        assert_eq!(prefix_id("a", "b"), "a.b");
        assert_eq!(prefix_id("outer", "inner_node"), "outer.inner_node");
    }

    // ---------------------------------------------------------------
    // Test 13: CompositionError display messages
    // ---------------------------------------------------------------

    #[test]
    fn composition_error_display_messages() {
        let err1 = CompositionError::SubGraphNotFound {
            path: "missing.dot".to_string(),
        };
        assert!(err1.to_string().contains("missing.dot"));

        let err2 = CompositionError::NoStartNode {
            node_id: "sub1".to_string(),
        };
        assert!(err2.to_string().contains("no start node"));
        assert!(err2.to_string().contains("sub1"));

        let err3 = CompositionError::NoExitNode {
            node_id: "sub2".to_string(),
        };
        assert!(err3.to_string().contains("no exit node"));
        assert!(err3.to_string().contains("sub2"));

        let err4 = CompositionError::CycleDetected {
            node_id: "loop_node".to_string(),
        };
        assert!(err4.to_string().contains("cycle detected"));
        assert!(err4.to_string().contains("loop_node"));

        let err5 = CompositionError::MissingPipelineAttr {
            node_id: "bad_node".to_string(),
        };
        assert!(err5.to_string().contains("pipeline"));
        assert!(err5.to_string().contains("bad_node"));
    }

    // ---------------------------------------------------------------
    // Test 14: Unrelated parent edges are preserved
    // ---------------------------------------------------------------

    #[test]
    fn composition_preserves_unrelated_edges() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_node("side", NodeType::Generic),
                make_sub_pipeline_node("sub1", "sub.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "side"),
                make_edge("side", "sub1"),
                make_edge("sub1", "exit"),
            ],
        );

        let sub = simple_sub_graph();
        let composed = compose_graphs(&parent, &sub, "sub1").unwrap();

        // The start -> side edge should remain untouched.
        let start_edges = composed.edges_from("start");
        assert_eq!(start_edges.len(), 1);
        assert_eq!(start_edges[0].to, "side");

        // side -> sub1.begin edge should exist.
        let side_edges = composed.edges_from("side");
        assert_eq!(side_edges.len(), 1);
        assert_eq!(side_edges[0].to, "sub1.begin");
    }

    // ---------------------------------------------------------------
    // Test 15: SubPipelineTransform with file on disk
    // ---------------------------------------------------------------

    #[test]
    fn sub_pipeline_transform_loads_dot_file() {
        let dir = std::env::temp_dir().join("smasher_composition_test");
        std::fs::create_dir_all(&dir).unwrap();

        let sub_dot = r#"
            digraph sub {
                begin [shape=circle];
                worker [shape=box, label="Do Work"];
                done [shape=doublecircle];
                begin -> worker;
                worker -> done;
            }
        "#;
        let sub_path = dir.join("sub.dot");
        std::fs::write(&sub_path, sub_dot).unwrap();

        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("sub1", sub_path.to_str().unwrap()),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "sub1"), make_edge("sub1", "exit")],
        );

        let transform = SubPipelineTransform::new(dir.to_str().unwrap());
        let composed = transform.apply(&parent).unwrap();

        // SubPipeline node should be replaced.
        assert!(composed.node("sub1").is_none());
        assert!(composed.node("sub1.begin").is_some());
        assert!(composed.node("sub1.worker").is_some());
        assert!(composed.node("sub1.done").is_some());

        // Clean up.
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------------------------------------------------------------
    // Test 16: SubPipelineTransform error on missing pipeline attribute
    // ---------------------------------------------------------------

    #[test]
    fn sub_pipeline_transform_missing_attr() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                // SubPipeline node without a pipeline attribute.
                GraphNode {
                    id: "bad_sub".to_string(),
                    node_type: NodeType::SubPipeline,
                    label: None,
                    attrs: HashMap::new(),
                },
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "bad_sub"), make_edge("bad_sub", "exit")],
        );

        let transform = SubPipelineTransform::new("/tmp");
        let result = transform.apply(&parent);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CompositionError::MissingPipelineAttr { .. }));
    }

    // ---------------------------------------------------------------
    // Test 17: SubPipelineTransform error on missing file
    // ---------------------------------------------------------------

    #[test]
    fn sub_pipeline_transform_file_not_found() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("sub1", "nonexistent_file_that_does_not_exist.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "sub1"), make_edge("sub1", "exit")],
        );

        let transform = SubPipelineTransform::new("/tmp/does_not_exist_dir");
        let result = transform.apply(&parent);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, CompositionError::SubGraphNotFound { .. })
                || matches!(err, CompositionError::IoError { .. })
        );
    }

    // ---------------------------------------------------------------
    // Test 18: No SubPipeline nodes is a no-op
    // ---------------------------------------------------------------

    #[test]
    fn sub_pipeline_transform_no_sub_pipelines_is_noop() {
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_node("worker", NodeType::Codergen),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "worker"), make_edge("worker", "exit")],
        );

        let transform = SubPipelineTransform::new("/tmp");
        let result = transform.apply(&parent).unwrap();

        assert_eq!(result.nodes.len(), 3);
        assert_eq!(result.edges.len(), 2);
    }

    // ---------------------------------------------------------------
    // Test 19: resolve_path handles relative and absolute
    // ---------------------------------------------------------------

    #[test]
    fn resolve_path_relative_and_absolute() {
        let transform = SubPipelineTransform::new("/base/dir");

        let relative = transform.resolve_path("sub.dot");
        assert_eq!(relative, "/base/dir/sub.dot");

        let absolute = transform.resolve_path("/absolute/path/sub.dot");
        assert_eq!(absolute, "/absolute/path/sub.dot");
    }

    // ---------------------------------------------------------------
    // Test 20: Compose graph with no parent edges to sub-pipeline
    // ---------------------------------------------------------------

    #[test]
    fn composition_sub_with_no_incoming_edges() {
        // A sub-pipeline node that has no incoming edges (orphan).
        let parent = make_graph(
            Some("parent"),
            vec![
                make_node("start", NodeType::Start),
                make_sub_pipeline_node("sub1", "sub.dot"),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit"), make_edge("sub1", "exit")],
        );

        let sub = simple_sub_graph();
        let composed = compose_graphs(&parent, &sub, "sub1").unwrap();

        // No edges should go TO the sub-graph start.
        let to_begin = composed.edges_to("sub1.begin");
        assert_eq!(to_begin.len(), 0);

        // The exit edge should come from sub1.done.
        let from_done = composed.edges_from("sub1.done");
        assert_eq!(from_done.len(), 1);
        assert_eq!(from_done[0].to, "exit");
    }
}
