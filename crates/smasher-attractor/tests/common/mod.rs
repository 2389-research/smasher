// ABOUTME: Shared test helpers for smasher-attractor integration tests.
// ABOUTME: Provides graph builders, test handlers, and context setup utilities.

#![allow(dead_code)]

pub mod test_fixtures;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use smasher_attractor::graph::{Graph, GraphEdge, GraphNode, NodeType};
use smasher_attractor::handler::{Handler, HandlerError};
use smasher_attractor::state::{Context, Outcome};

/// Build a minimal graph: start -> exit.
///
/// The start node has `NodeType::Start` and the exit node has `NodeType::Exit`.
/// A single edge connects them with no conditions or attributes.
pub fn minimal_graph() -> Graph {
    Graph {
        name: Some("minimal".to_string()),
        nodes: vec![
            GraphNode {
                id: "start".to_string(),
                node_type: NodeType::Start,
                label: None,
                attrs: HashMap::new(),
            },
            GraphNode {
                id: "exit".to_string(),
                node_type: NodeType::Exit,
                label: None,
                attrs: HashMap::new(),
            },
        ],
        edges: vec![GraphEdge {
            from: "start".to_string(),
            to: "exit".to_string(),
            label: None,
            condition: None,
            priority: None,
            loop_restart: false,
            attrs: HashMap::new(),
        }],
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

/// Build a conditional graph: start -> diamond -> exit_a / exit_b.
///
/// The diamond node has `NodeType::Conditional`. The edge to `exit_a` has
/// condition `"result=true"` and the edge to `exit_b` has condition
/// `"result=false"`. Both exit nodes are `NodeType::Exit`.
pub fn conditional_graph() -> Graph {
    Graph {
        name: Some("conditional".to_string()),
        nodes: vec![
            GraphNode {
                id: "start".to_string(),
                node_type: NodeType::Start,
                label: None,
                attrs: HashMap::new(),
            },
            GraphNode {
                id: "diamond".to_string(),
                node_type: NodeType::Conditional,
                label: Some("branch".to_string()),
                attrs: HashMap::new(),
            },
            GraphNode {
                id: "exit_a".to_string(),
                node_type: NodeType::Exit,
                label: Some("Path A".to_string()),
                attrs: HashMap::new(),
            },
            GraphNode {
                id: "exit_b".to_string(),
                node_type: NodeType::Exit,
                label: Some("Path B".to_string()),
                attrs: HashMap::new(),
            },
        ],
        edges: vec![
            GraphEdge {
                from: "start".to_string(),
                to: "diamond".to_string(),
                label: None,
                condition: None,
                priority: None,
                loop_restart: false,
                attrs: HashMap::new(),
            },
            GraphEdge {
                from: "diamond".to_string(),
                to: "exit_a".to_string(),
                label: Some("true".to_string()),
                condition: Some("result=true".to_string()),
                priority: Some(1),
                loop_restart: false,
                attrs: HashMap::new(),
            },
            GraphEdge {
                from: "diamond".to_string(),
                to: "exit_b".to_string(),
                label: Some("false".to_string()),
                condition: Some("result=false".to_string()),
                priority: Some(2),
                loop_restart: false,
                attrs: HashMap::new(),
            },
        ],
        default_node_attrs: HashMap::new(),
        default_edge_attrs: HashMap::new(),
    }
}

/// A test handler that sets a configurable context key when executed.
///
/// Use this to verify that a handler was invoked during pipeline traversal
/// by checking for the key's presence in the context after execution.
pub struct ContextSettingHandler {
    handler_name: String,
    target_node_type: NodeType,
    context_key: String,
    context_value: serde_json::Value,
}

impl ContextSettingHandler {
    /// Create a handler that sets `key` to `value` in the context when executed.
    pub fn new(
        name: impl Into<String>,
        node_type: NodeType,
        key: impl Into<String>,
        value: serde_json::Value,
    ) -> Arc<Self> {
        Arc::new(Self {
            handler_name: name.into(),
            target_node_type: node_type,
            context_key: key.into(),
            context_value: value,
        })
    }
}

#[async_trait]
impl Handler for ContextSettingHandler {
    fn name(&self) -> &str {
        &self.handler_name
    }

    async fn execute(&self, _node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        context.set(&self.context_key, self.context_value.clone());
        Ok(Outcome::success_with(
            json!({ "set_key": self.context_key }),
        ))
    }

    fn handles(&self, node_type: &NodeType) -> bool {
        *node_type == self.target_node_type
    }
}
