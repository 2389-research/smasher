// ABOUTME: AST types for the DOT graph language used to define AI workflow pipelines.
// ABOUTME: Defines DotGraph, DotStatement, DotNode, DotEdge, DotAttr, and DotValue.

use std::fmt;
use std::time::Duration;

/// A parsed DOT graph containing statements (nodes, edges, attributes, subgraphs).
#[derive(Debug, Clone)]
pub struct DotGraph {
    pub name: Option<String>,
    pub is_digraph: bool,
    pub statements: Vec<DotStatement>,
}

/// A single statement within a DOT graph.
#[derive(Debug, Clone)]
pub enum DotStatement {
    Node(DotNode),
    Edge(DotEdge),
    Attr(DotAttr),
    Subgraph(DotGraph),
    DefaultNode(Vec<DotAttr>),
    DefaultEdge(Vec<DotAttr>),
}

/// A node declaration with an identifier and optional attributes.
#[derive(Debug, Clone)]
pub struct DotNode {
    pub id: String,
    pub attrs: Vec<DotAttr>,
}

/// An edge connecting two nodes with optional attributes.
#[derive(Debug, Clone)]
pub struct DotEdge {
    pub from: String,
    pub to: String,
    pub attrs: Vec<DotAttr>,
}

/// A key-value attribute pair.
#[derive(Debug, Clone)]
pub struct DotAttr {
    pub key: String,
    pub value: DotValue,
}

/// A typed attribute value supporting strings, numbers, durations, and booleans.
#[derive(Debug, Clone, PartialEq)]
pub enum DotValue {
    String(String),
    Number(f64),
    Duration(Duration),
    Bool(bool),
}

impl fmt::Display for DotValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DotValue::String(s) => write!(f, "{s}"),
            DotValue::Number(n) => write!(f, "{n}"),
            DotValue::Duration(d) => write!(f, "{}s", d.as_secs()),
            DotValue::Bool(b) => write!(f, "{b}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_string_value() {
        let val = DotValue::String("hello".to_string());
        assert_eq!(val.to_string(), "hello");
    }

    #[test]
    fn display_number_value() {
        let val = DotValue::Number(3.15);
        assert_eq!(val.to_string(), "3.15");

        let val_int = DotValue::Number(42.0);
        assert_eq!(val_int.to_string(), "42");
    }

    #[test]
    fn display_duration_value() {
        let val = DotValue::Duration(Duration::from_secs(900));
        assert_eq!(val.to_string(), "900s");
    }

    #[test]
    fn display_bool_value() {
        assert_eq!(DotValue::Bool(true).to_string(), "true");
        assert_eq!(DotValue::Bool(false).to_string(), "false");
    }

    #[test]
    fn dot_graph_construction() {
        let graph = DotGraph {
            name: Some("TestGraph".to_string()),
            is_digraph: true,
            statements: vec![
                DotStatement::Node(DotNode {
                    id: "a".to_string(),
                    attrs: vec![DotAttr {
                        key: "label".to_string(),
                        value: DotValue::String("Start".to_string()),
                    }],
                }),
                DotStatement::Edge(DotEdge {
                    from: "a".to_string(),
                    to: "b".to_string(),
                    attrs: vec![],
                }),
            ],
        };

        assert_eq!(graph.name, Some("TestGraph".to_string()));
        assert!(graph.is_digraph);
        assert_eq!(graph.statements.len(), 2);
    }
}
