// ABOUTME: JSON serialization bridges for smasher types that don't implement Serialize.
// ABOUTME: Converts Graph, ExecutionResult, SessionEvent, etc. to conformance contract JSON.

use serde_json::{Value, json};

use smasher_agent::types::SessionEvent;
use smasher_attractor::engine::ExecutionResult;
use smasher_attractor::graph::{Graph, GraphEdge, GraphNode, NodeAttrValue, NodeType};
use smasher_attractor::state::Outcome;

/// Convert a `NodeAttrValue` to a `serde_json::Value`.
pub fn node_attr_value_to_json(val: &NodeAttrValue) -> Value {
    match val {
        NodeAttrValue::String(s) => Value::String(s.clone()),
        NodeAttrValue::Number(n) => json!(n),
        NodeAttrValue::Bool(b) => json!(b),
        NodeAttrValue::Duration(d) => {
            let secs = d.as_secs();
            let millis = d.subsec_millis();
            if millis > 0 {
                Value::String(format!("{}.{:03}s", secs, millis))
            } else {
                Value::String(format!("{}s", secs))
            }
        }
    }
}

/// Map a `NodeType` to its canonical DOT shape string for the conformance contract.
fn node_type_to_shape(node_type: &NodeType) -> &'static str {
    match node_type {
        NodeType::Start => "Mdiamond",
        NodeType::Exit => "Msquare",
        NodeType::Codergen => "box",
        NodeType::Conditional => "diamond",
        NodeType::Tool => "component",
        NodeType::Interviewer => "hexagon",
        NodeType::Parallel => "parallelogram",
        NodeType::FanIn => "trapezium",
        NodeType::Manager => "house",
        NodeType::SubPipeline => "folder",
        NodeType::Generic => "ellipse",
    }
}

/// Convert a `GraphNode` to a JSON object with `id`, `shape`, and all extra attrs.
///
/// Attrs are inserted first so that canonical fields (`id`, `shape`, `label`) always
/// win if an untrusted attr tries to overwrite a structural key.
fn node_to_json(node: &GraphNode) -> Value {
    let mut obj = serde_json::Map::new();

    // Insert untrusted attrs first so canonical fields can overwrite them.
    for (key, val) in &node.attrs {
        obj.insert(key.clone(), node_attr_value_to_json(val));
    }

    // Canonical fields are inserted last and always win.
    obj.insert("id".to_string(), json!(node.id));
    obj.insert(
        "shape".to_string(),
        json!(node_type_to_shape(&node.node_type)),
    );
    if let Some(ref label) = node.label {
        obj.insert("label".to_string(), json!(label));
    }

    Value::Object(obj)
}

/// Convert a `GraphEdge` to a JSON object with `from`, `to`, and all metadata.
///
/// Attrs are inserted first so that canonical fields (`from`, `to`, `label`,
/// `condition`, `priority`) always win if an untrusted attr tries to overwrite
/// a structural key.
fn edge_to_json(edge: &GraphEdge) -> Value {
    let mut obj = serde_json::Map::new();

    // Insert untrusted attrs first so canonical fields can overwrite them.
    for (key, val) in &edge.attrs {
        obj.insert(key.clone(), node_attr_value_to_json(val));
    }

    // Canonical fields are inserted last and always win.
    obj.insert("from".to_string(), json!(edge.from));
    obj.insert("to".to_string(), json!(edge.to));
    if let Some(ref label) = edge.label {
        obj.insert("label".to_string(), json!(label));
    }
    if let Some(ref condition) = edge.condition {
        obj.insert("condition".to_string(), json!(condition));
    }
    if let Some(priority) = edge.priority {
        obj.insert("priority".to_string(), json!(priority));
    }

    Value::Object(obj)
}

/// Convert a `Graph` to the conformance contract JSON shape.
///
/// Produces `{"nodes": [...], "edges": [...]}` where each node has `id`, `shape`,
/// and all attrs, and each edge has `from`, `to`, `label`, `condition`, `priority`,
/// and all attrs.
pub fn graph_to_json(graph: &Graph) -> Value {
    let nodes: Vec<Value> = graph.nodes.iter().map(node_to_json).collect();
    let edges: Vec<Value> = graph.edges.iter().map(edge_to_json).collect();

    json!({
        "nodes": nodes,
        "edges": edges,
    })
}

/// Determine whether an `ExecutionResult` represents overall success.
///
/// The result is considered successful when every exit node that was visited
/// has a `Success` outcome. If no exit nodes were visited, the result is
/// considered a failure.
fn is_execution_success(result: &ExecutionResult, graph: &Graph) -> bool {
    let exit_ids: Vec<&str> = graph.exit_nodes().iter().map(|n| n.id.as_str()).collect();

    let visited_exits: Vec<&&str> = exit_ids
        .iter()
        .filter(|id| result.visited_nodes.contains(&id.to_string()))
        .collect();

    if visited_exits.is_empty() {
        return false;
    }

    visited_exits.iter().all(|id| {
        matches!(
            result.node_outcomes.get(**id),
            Some(Outcome::Success { .. })
        )
    })
}

/// Convert an `ExecutionResult` to the conformance contract JSON shape.
///
/// Produces `{"status": "success"|"failure", "context": {...}, "visited_nodes": [...],
/// "steps_taken": N, "node_outcomes": {...}}`.
pub fn execution_result_to_json(result: &ExecutionResult, graph: &Graph) -> Value {
    let status = if is_execution_success(result, graph) {
        "success"
    } else {
        "failure"
    };

    // Outcome derives Serialize, so we can convert each one via serde_json.
    let node_outcomes: serde_json::Map<String, Value> = result
        .node_outcomes
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::to_value(v).unwrap_or(Value::Null)))
        .collect();

    json!({
        "status": status,
        "context": result.final_context,
        "visited_nodes": result.visited_nodes,
        "steps_taken": result.steps_taken,
        "node_outcomes": node_outcomes,
    })
}

/// Convert a `SessionEvent` to the conformance contract JSON shape.
///
/// Each variant maps to a JSON object with a `"type"` discriminator field
/// and variant-specific fields.
pub fn session_event_to_json(event: &SessionEvent) -> Value {
    match event {
        SessionEvent::SessionStarted { session_id } => json!({
            "type": "session_start",
            "session_id": session_id,
        }),
        SessionEvent::TurnStarted { turn_number } => json!({
            "type": "turn_start",
            "turn_number": turn_number,
        }),
        SessionEvent::AssistantMessage { response } => {
            let text = response.text().unwrap_or_default();
            json!({
                "type": "assistant_message",
                "text": text,
            })
        }
        SessionEvent::ToolCallStarted {
            tool_name,
            tool_call_id,
            input_preview,
        } => json!({
            "type": "tool_call_start",
            "tool_name": tool_name,
            "tool_call_id": tool_call_id,
            "input_preview": input_preview,
        }),
        SessionEvent::ToolCallCompleted {
            tool_name,
            tool_call_id,
            result,
            is_error,
            duration_ms,
        } => json!({
            "type": "tool_call_end",
            "tool_name": tool_name,
            "tool_call_id": tool_call_id,
            "result": result,
            "is_error": is_error,
            "duration_ms": duration_ms,
        }),
        SessionEvent::TextDelta { text } => json!({
            "type": "text_delta",
            "text": text,
        }),
        SessionEvent::SteeringApplied { text } => json!({
            "type": "steering_applied",
            "text": text,
        }),
        SessionEvent::SessionCompleted {
            session_id,
            total_turns,
            total_usage: _,
        } => json!({
            "type": "session_end",
            "session_id": session_id,
            "total_turns": total_turns,
        }),
        SessionEvent::SessionError { session_id, error } => json!({
            "type": "session_error",
            "session_id": session_id,
            "error": error,
        }),
        SessionEvent::LoopDetected {
            pattern,
            window_size,
        } => json!({
            "type": "loop_detected",
            "pattern": pattern,
            "window_size": window_size,
        }),
        SessionEvent::ContextWindowWarning {
            used,
            limit,
            fraction,
        } => json!({
            "type": "context_window_warning",
            "used": used,
            "limit": limit,
            "fraction": fraction,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smasher_attractor::engine::LoopCounter;
    use smasher_attractor::graph::{Graph, GraphEdge, GraphNode, NodeAttrValue, NodeType};
    use smasher_attractor::state::Outcome;
    use smasher_attractor::stats::PipelineStats;
    use std::collections::HashMap;
    use std::time::Duration;

    fn simple_graph() -> Graph {
        Graph {
            name: Some("test_pipeline".to_string()),
            nodes: vec![
                GraphNode {
                    id: "start".to_string(),
                    node_type: NodeType::Start,
                    label: Some("Begin".to_string()),
                    attrs: HashMap::new(),
                },
                GraphNode {
                    id: "step_a".to_string(),
                    node_type: NodeType::Codergen,
                    label: Some("Code Gen".to_string()),
                    attrs: {
                        let mut m = HashMap::new();
                        m.insert(
                            "timeout".to_string(),
                            NodeAttrValue::Duration(Duration::from_secs(30)),
                        );
                        m.insert("retries".to_string(), NodeAttrValue::Number(3.0));
                        m
                    },
                },
                GraphNode {
                    id: "done".to_string(),
                    node_type: NodeType::Exit,
                    label: None,
                    attrs: HashMap::new(),
                },
            ],
            edges: vec![
                GraphEdge {
                    from: "start".to_string(),
                    to: "step_a".to_string(),
                    label: Some("go".to_string()),
                    condition: None,
                    priority: None,
                    loop_restart: false,
                    attrs: HashMap::new(),
                },
                GraphEdge {
                    from: "step_a".to_string(),
                    to: "done".to_string(),
                    label: None,
                    condition: Some("success".to_string()),
                    priority: Some(1),
                    loop_restart: false,
                    attrs: HashMap::new(),
                },
            ],
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
            graph_attrs: HashMap::new(),
        }
    }

    #[test]
    fn graph_to_json_produces_correct_structure() {
        let graph = simple_graph();
        let json = graph_to_json(&graph);

        // Verify top-level keys
        assert!(json.get("nodes").is_some());
        assert!(json.get("edges").is_some());

        let nodes = json["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3);

        // Check start node
        let start = &nodes[0];
        assert_eq!(start["id"], "start");
        assert_eq!(start["shape"], "Mdiamond");
        assert_eq!(start["label"], "Begin");

        // Check step_a node with extra attrs
        let step_a = &nodes[1];
        assert_eq!(step_a["id"], "step_a");
        assert_eq!(step_a["shape"], "box");
        assert_eq!(step_a["label"], "Code Gen");
        assert_eq!(step_a["timeout"], "30s");
        assert_eq!(step_a["retries"], 3.0);

        // Check done (exit) node
        let done = &nodes[2];
        assert_eq!(done["id"], "done");
        assert_eq!(done["shape"], "Msquare");
        assert!(done.get("label").is_none());
    }

    #[test]
    fn graph_to_json_edges_correct() {
        let graph = simple_graph();
        let json = graph_to_json(&graph);
        let edges = json["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 2);

        let e0 = &edges[0];
        assert_eq!(e0["from"], "start");
        assert_eq!(e0["to"], "step_a");
        assert_eq!(e0["label"], "go");
        assert!(e0.get("condition").is_none());
        assert!(e0.get("priority").is_none());

        let e1 = &edges[1];
        assert_eq!(e1["from"], "step_a");
        assert_eq!(e1["to"], "done");
        assert_eq!(e1["condition"], "success");
        assert_eq!(e1["priority"], 1);
    }

    #[test]
    fn node_type_shape_round_trip() {
        let cases = vec![
            (NodeType::Start, "Mdiamond"),
            (NodeType::Exit, "Msquare"),
            (NodeType::Codergen, "box"),
            (NodeType::Conditional, "diamond"),
            (NodeType::Tool, "component"),
            (NodeType::Interviewer, "hexagon"),
            (NodeType::Parallel, "parallelogram"),
            (NodeType::FanIn, "trapezium"),
            (NodeType::Manager, "house"),
            (NodeType::SubPipeline, "folder"),
            (NodeType::Generic, "ellipse"),
        ];

        for (node_type, expected_shape) in cases {
            assert_eq!(
                node_type_to_shape(&node_type),
                expected_shape,
                "{:?} should map to shape '{}'",
                node_type,
                expected_shape,
            );
        }
    }

    #[test]
    fn node_canonical_fields_win_over_attrs() {
        // An attr with a key matching a canonical field must not overwrite it.
        let node = GraphNode {
            id: "real_id".to_string(),
            node_type: NodeType::Codergen,
            label: Some("real_label".to_string()),
            attrs: {
                let mut m = HashMap::new();
                m.insert(
                    "id".to_string(),
                    NodeAttrValue::String("evil_id".to_string()),
                );
                m.insert(
                    "shape".to_string(),
                    NodeAttrValue::String("evil_shape".to_string()),
                );
                m.insert(
                    "label".to_string(),
                    NodeAttrValue::String("evil_label".to_string()),
                );
                m
            },
        };
        let json = node_to_json(&node);
        assert_eq!(json["id"], "real_id");
        assert_eq!(json["shape"], "box");
        assert_eq!(json["label"], "real_label");
    }

    #[test]
    fn edge_canonical_fields_win_over_attrs() {
        // An attr with a key matching a canonical field must not overwrite it.
        let edge = GraphEdge {
            from: "real_from".to_string(),
            to: "real_to".to_string(),
            label: Some("real_label".to_string()),
            condition: Some("real_condition".to_string()),
            priority: Some(5),
            loop_restart: false,
            attrs: {
                let mut m = HashMap::new();
                m.insert(
                    "from".to_string(),
                    NodeAttrValue::String("evil_from".to_string()),
                );
                m.insert(
                    "to".to_string(),
                    NodeAttrValue::String("evil_to".to_string()),
                );
                m.insert(
                    "condition".to_string(),
                    NodeAttrValue::String("evil_condition".to_string()),
                );
                m.insert("priority".to_string(), NodeAttrValue::Number(999.0));
                m
            },
        };
        let json = edge_to_json(&edge);
        assert_eq!(json["from"], "real_from");
        assert_eq!(json["to"], "real_to");
        assert_eq!(json["label"], "real_label");
        assert_eq!(json["condition"], "real_condition");
        assert_eq!(json["priority"], 5);
    }

    #[test]
    fn node_attr_value_conversions() {
        assert_eq!(
            node_attr_value_to_json(&NodeAttrValue::String("hello".to_string())),
            json!("hello")
        );
        assert_eq!(
            node_attr_value_to_json(&NodeAttrValue::Number(42.0)),
            json!(42.0)
        );
        assert_eq!(
            node_attr_value_to_json(&NodeAttrValue::Bool(true)),
            json!(true)
        );
        assert_eq!(
            node_attr_value_to_json(&NodeAttrValue::Duration(Duration::from_secs(5))),
            json!("5s")
        );
        assert_eq!(
            node_attr_value_to_json(&NodeAttrValue::Duration(Duration::from_millis(1500))),
            json!("1.500s")
        );
    }

    #[test]
    fn execution_result_success() {
        let graph = simple_graph();
        let result = ExecutionResult {
            visited_nodes: vec![
                "start".to_string(),
                "step_a".to_string(),
                "done".to_string(),
            ],
            node_outcomes: {
                let mut m = HashMap::new();
                m.insert("start".to_string(), Outcome::success());
                m.insert("step_a".to_string(), Outcome::success());
                m.insert("done".to_string(), Outcome::success());
                m
            },
            final_context: {
                let mut m = HashMap::new();
                m.insert("key".to_string(), json!("value"));
                m
            },
            steps_taken: 3,
            checkpoint: None,
            loop_restarts: LoopCounter::new(),
            stats: PipelineStats::from_node_timings(vec![], 0),
        };

        let json = execution_result_to_json(&result, &graph);
        assert_eq!(json["status"], "success");
        assert_eq!(json["steps_taken"], 3);
        assert_eq!(json["context"]["key"], "value");
        assert_eq!(json["visited_nodes"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn execution_result_failure_when_exit_not_visited() {
        let graph = simple_graph();
        let result = ExecutionResult {
            visited_nodes: vec!["start".to_string()],
            node_outcomes: {
                let mut m = HashMap::new();
                m.insert("start".to_string(), Outcome::success());
                m
            },
            final_context: HashMap::new(),
            steps_taken: 1,
            checkpoint: None,
            loop_restarts: LoopCounter::new(),
            stats: PipelineStats::from_node_timings(vec![], 0),
        };

        let json = execution_result_to_json(&result, &graph);
        assert_eq!(json["status"], "failure");
    }

    #[test]
    fn session_event_session_started() {
        let event = SessionEvent::SessionStarted {
            session_id: "abc123".to_string(),
        };
        let json = session_event_to_json(&event);
        assert_eq!(json["type"], "session_start");
        assert_eq!(json["session_id"], "abc123");
    }

    #[test]
    fn session_event_tool_call_completed() {
        let event = SessionEvent::ToolCallCompleted {
            tool_name: "read_file".to_string(),
            tool_call_id: "tc_1".to_string(),
            result: "file contents".to_string(),
            is_error: false,
            duration_ms: 150,
        };
        let json = session_event_to_json(&event);
        assert_eq!(json["type"], "tool_call_end");
        assert_eq!(json["tool_name"], "read_file");
        assert_eq!(json["is_error"], false);
        assert_eq!(json["duration_ms"], 150);
    }

    #[test]
    fn session_event_context_window_warning() {
        let event = SessionEvent::ContextWindowWarning {
            used: 80000,
            limit: 100000,
            fraction: 0.8,
        };
        let json = session_event_to_json(&event);
        assert_eq!(json["type"], "context_window_warning");
        assert_eq!(json["used"], 80000);
        assert_eq!(json["limit"], 100000);
        assert_eq!(json["fraction"], 0.8);
    }
}
