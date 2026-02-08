// ABOUTME: Recursive descent parser for the DOT graph language, producing a DotGraph AST.
// ABOUTME: Converts token streams from the lexer into structured graph representations.

use super::ast::{DotAttr, DotEdge, DotGraph, DotNode, DotStatement, DotValue};
use super::lexer::{self, LexerError, Token};
use std::collections::VecDeque;
use std::time::Duration;

/// Errors that can occur during parsing.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("lexer error: {0}")]
    Lexer(#[from] LexerError),
    #[error("expected {expected}, found {found:?}")]
    Expected { expected: String, found: Token },
    #[error("unexpected end of input")]
    UnexpectedEof,
}

/// Parse a DOT language string into a DotGraph AST.
pub fn parse(input: &str) -> Result<DotGraph, ParseError> {
    let tokens = lexer::tokenize(input)?;
    let mut parser = Parser::new(tokens);
    parser.parse_graph()
}

/// Internal parser state holding the token stream and current position.
struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Buffer for statements expanded from edge chains and graph default attrs.
    pending_stmts: VecDeque<DotStatement>,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            pending_stmts: VecDeque::new(),
        }
    }

    /// Peek at the current token without consuming it.
    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    /// Consume the current token and advance.
    fn advance(&mut self) -> Token {
        let tok = self.tokens.get(self.pos).cloned().unwrap_or(Token::Eof);
        self.pos += 1;
        tok
    }

    /// Consume a token, returning an error if it doesn't match the expected kind.
    fn expect(&mut self, expected: &Token) -> Result<Token, ParseError> {
        let tok = self.advance();
        if std::mem::discriminant(&tok) == std::mem::discriminant(expected) {
            Ok(tok)
        } else {
            Err(ParseError::Expected {
                expected: format!("{expected:?}"),
                found: tok,
            })
        }
    }

    /// Parse a full DOT graph: ["strict"] ("digraph" | "graph") [ident] "{" stmt_list "}"
    fn parse_graph(&mut self) -> Result<DotGraph, ParseError> {
        // Optional "strict"
        if matches!(self.peek(), Token::Strict) {
            self.advance();
        }

        // "digraph" or "graph"
        let is_digraph = match self.peek() {
            Token::Digraph => {
                self.advance();
                true
            }
            Token::Graph => {
                self.advance();
                false
            }
            other => {
                return Err(ParseError::Expected {
                    expected: "digraph or graph".to_string(),
                    found: other.clone(),
                });
            }
        };

        // Optional name
        let name = if let Token::Ident(_) = self.peek() {
            if let Token::Ident(name) = self.advance() {
                Some(name)
            } else {
                None
            }
        } else {
            None
        };

        self.expect(&Token::LBrace)?;
        let statements = self.parse_stmt_list()?;
        self.expect(&Token::RBrace)?;

        Ok(DotGraph {
            name,
            is_digraph,
            statements,
        })
    }

    /// Parse a list of statements until we hit a closing brace or EOF.
    fn parse_stmt_list(&mut self) -> Result<Vec<DotStatement>, ParseError> {
        let mut stmts = Vec::new();

        loop {
            // Drain any buffered statements from edge chains or graph defaults
            while let Some(pending) = self.pending_stmts.pop_front() {
                stmts.push(pending);
            }

            match self.peek() {
                Token::RBrace | Token::Eof => break,
                _ => {
                    let stmt = self.parse_stmt()?;
                    stmts.push(stmt);
                    // Optional semicolon
                    if matches!(self.peek(), Token::Semi) {
                        self.advance();
                    }
                }
            }
        }

        // Drain any remaining buffered statements
        while let Some(pending) = self.pending_stmts.pop_front() {
            stmts.push(pending);
        }

        Ok(stmts)
    }

    /// Parse one or more statements from a single source construct.
    /// Edge chains like `a -> b -> c` expand into multiple edge statements.
    /// `graph [k=v, ...]` expands into multiple attribute statements.
    fn parse_stmt(&mut self) -> Result<DotStatement, ParseError> {
        match self.peek() {
            Token::Node => {
                self.advance();
                let attrs = self.parse_attr_list()?;
                Ok(DotStatement::DefaultNode(attrs))
            }
            Token::Edge => {
                self.advance();
                let attrs = self.parse_attr_list()?;
                Ok(DotStatement::DefaultEdge(attrs))
            }
            Token::Graph => {
                self.advance();
                // graph [...] — default graph attributes, expanded into Attr statements
                if matches!(self.peek(), Token::LBracket) {
                    let attrs = self.parse_attr_list()?;
                    self.pending_stmts
                        .extend(attrs.into_iter().map(DotStatement::Attr));
                    // Return the first one; the rest are buffered in pending_stmts
                    self.pending_stmts
                        .pop_front()
                        .ok_or(ParseError::UnexpectedEof)
                } else {
                    Err(ParseError::Expected {
                        expected: "[".to_string(),
                        found: self.peek().clone(),
                    })
                }
            }
            Token::Subgraph => {
                let subgraph = self.parse_subgraph()?;
                Ok(DotStatement::Subgraph(subgraph))
            }
            Token::Ident(_) | Token::StringLit(_) => {
                let id = self.parse_id()?;

                // Check if this is an edge statement (possibly chained)
                if matches!(self.peek(), Token::Arrow | Token::DashDash) {
                    self.parse_edge_chain(id)
                }
                // Check if this is a graph-level attribute: ident = value
                else if matches!(self.peek(), Token::Equals) {
                    self.advance();
                    let value = self.parse_value()?;
                    Ok(DotStatement::Attr(DotAttr { key: id, value }))
                }
                // Otherwise it's a node statement
                else {
                    let attrs = if matches!(self.peek(), Token::LBracket) {
                        self.parse_attr_list()?
                    } else {
                        vec![]
                    };
                    Ok(DotStatement::Node(DotNode { id, attrs }))
                }
            }
            other => Err(ParseError::Expected {
                expected: "statement".to_string(),
                found: other.clone(),
            }),
        }
    }

    /// Parse an edge chain like `a -> b -> c [attrs]`.
    /// Produces multiple DotEdge statements; the first is returned directly,
    /// the rest are buffered in pending_stmts.
    fn parse_edge_chain(&mut self, first: String) -> Result<DotStatement, ParseError> {
        let mut nodes = vec![first];

        // Collect all chained nodes: consume `->` id pairs
        while matches!(self.peek(), Token::Arrow | Token::DashDash) {
            self.advance();
            let next = self.parse_id()?;
            nodes.push(next);
        }

        // Attributes on the chain apply only to the last edge
        let trailing_attrs = if matches!(self.peek(), Token::LBracket) {
            self.parse_attr_list()?
        } else {
            vec![]
        };

        // Build edge statements for each consecutive pair
        let last_idx = nodes.len() - 2;
        for i in 0..=last_idx {
            let attrs = if i == last_idx {
                trailing_attrs.clone()
            } else {
                vec![]
            };
            let edge = DotStatement::Edge(DotEdge {
                from: nodes[i].clone(),
                to: nodes[i + 1].clone(),
                attrs,
            });
            self.pending_stmts.push_back(edge);
        }

        // Return the first edge; the rest are buffered
        self.pending_stmts
            .pop_front()
            .ok_or(ParseError::UnexpectedEof)
    }

    /// Parse a subgraph: "subgraph" [ident] "{" stmt_list "}"
    fn parse_subgraph(&mut self) -> Result<DotGraph, ParseError> {
        self.expect(&Token::Subgraph)?;

        let name = if let Token::Ident(_) = self.peek() {
            if let Token::Ident(name) = self.advance() {
                Some(name)
            } else {
                None
            }
        } else {
            None
        };

        self.expect(&Token::LBrace)?;
        let statements = self.parse_stmt_list()?;
        self.expect(&Token::RBrace)?;

        Ok(DotGraph {
            name,
            is_digraph: false,
            statements,
        })
    }

    /// Parse an identifier (Ident or StringLit).
    fn parse_id(&mut self) -> Result<String, ParseError> {
        match self.advance() {
            Token::Ident(s) | Token::StringLit(s) => Ok(s),
            other => Err(ParseError::Expected {
                expected: "identifier".to_string(),
                found: other,
            }),
        }
    }

    /// Parse an attribute list: "[" (attr ("," | ";")?)* "]"
    fn parse_attr_list(&mut self) -> Result<Vec<DotAttr>, ParseError> {
        self.expect(&Token::LBracket)?;
        let mut attrs = Vec::new();

        loop {
            if matches!(self.peek(), Token::RBracket) {
                self.advance();
                break;
            }

            let key = self.parse_id()?;
            self.expect(&Token::Equals)?;
            let value = self.parse_value()?;
            attrs.push(DotAttr { key, value });

            // Optional comma or semicolon separator
            if matches!(self.peek(), Token::Comma | Token::Semi) {
                self.advance();
            }
        }

        Ok(attrs)
    }

    /// Parse a value, applying duration and boolean coercion to string literals.
    fn parse_value(&mut self) -> Result<DotValue, ParseError> {
        match self.advance() {
            Token::StringLit(s) => Ok(coerce_string_value(s)),
            Token::Number(n) => Ok(DotValue::Number(n)),
            Token::Ident(s) => Ok(coerce_string_value(s)),
            Token::Duration(d) => Ok(DotValue::Duration(d)),
            other => Err(ParseError::Expected {
                expected: "value".to_string(),
                found: other,
            }),
        }
    }
}

/// Attempt to coerce a string value into a more specific DotValue type.
/// Checks for duration patterns (e.g., "900s", "100ms") and boolean strings.
fn coerce_string_value(s: String) -> DotValue {
    // Boolean coercion
    match s.as_str() {
        "true" => return DotValue::Bool(true),
        "false" => return DotValue::Bool(false),
        _ => {}
    }

    // Duration coercion
    if let Some(duration) = try_parse_duration(&s) {
        return DotValue::Duration(duration);
    }

    DotValue::String(s)
}

/// Try to parse a string as a duration value (e.g., "900s", "100ms", "5m", "2h").
fn try_parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    if let Some(num_str) = s.strip_suffix("ms") {
        let num: u64 = num_str.parse().ok()?;
        return Some(Duration::from_millis(num));
    }
    if let Some(num_str) = s.strip_suffix('s') {
        let num: u64 = num_str.parse().ok()?;
        return Some(Duration::from_secs(num));
    }
    if let Some(num_str) = s.strip_suffix('m') {
        let num: u64 = num_str.parse().ok()?;
        return Some(Duration::from_secs(num * 60));
    }
    if let Some(num_str) = s.strip_suffix('h') {
        let num: u64 = num_str.parse().ok()?;
        return Some(Duration::from_secs(num * 3600));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_digraph() {
        let graph = parse("digraph {}").unwrap();
        assert!(graph.is_digraph);
        assert_eq!(graph.name, None);
        assert!(graph.statements.is_empty());
    }

    #[test]
    fn parse_named_digraph() {
        let graph = parse("digraph MyGraph {}").unwrap();
        assert!(graph.is_digraph);
        assert_eq!(graph.name, Some("MyGraph".to_string()));
    }

    #[test]
    fn parse_named_graph() {
        let graph = parse("graph G {}").unwrap();
        assert!(!graph.is_digraph);
        assert_eq!(graph.name, Some("G".to_string()));
    }

    #[test]
    fn parse_node_with_attributes() {
        let graph = parse(r#"digraph { a [label="Start"] }"#).unwrap();
        assert_eq!(graph.statements.len(), 1);
        match &graph.statements[0] {
            DotStatement::Node(node) => {
                assert_eq!(node.id, "a");
                assert_eq!(node.attrs.len(), 1);
                assert_eq!(node.attrs[0].key, "label");
                assert_eq!(node.attrs[0].value, DotValue::String("Start".to_string()));
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn parse_edge() {
        let graph = parse("digraph { a -> b }").unwrap();
        assert_eq!(graph.statements.len(), 1);
        match &graph.statements[0] {
            DotStatement::Edge(edge) => {
                assert_eq!(edge.from, "a");
                assert_eq!(edge.to, "b");
                assert!(edge.attrs.is_empty());
            }
            other => panic!("expected Edge, got {other:?}"),
        }
    }

    #[test]
    fn parse_edge_with_attributes() {
        let graph = parse(r#"digraph { a -> b [label="next"] }"#).unwrap();
        assert_eq!(graph.statements.len(), 1);
        match &graph.statements[0] {
            DotStatement::Edge(edge) => {
                assert_eq!(edge.from, "a");
                assert_eq!(edge.to, "b");
                assert_eq!(edge.attrs.len(), 1);
                assert_eq!(edge.attrs[0].key, "label");
                assert_eq!(edge.attrs[0].value, DotValue::String("next".to_string()));
            }
            other => panic!("expected Edge, got {other:?}"),
        }
    }

    #[test]
    fn parse_multiple_statements() {
        let graph = parse(
            r#"digraph {
            a [label="Start"];
            b [label="End"];
            a -> b;
        }"#,
        )
        .unwrap();
        assert_eq!(graph.statements.len(), 3);
    }

    #[test]
    fn parse_default_node_attributes() {
        let graph = parse("digraph { node [shape=box] }").unwrap();
        assert_eq!(graph.statements.len(), 1);
        match &graph.statements[0] {
            DotStatement::DefaultNode(attrs) => {
                assert_eq!(attrs.len(), 1);
                assert_eq!(attrs[0].key, "shape");
                assert_eq!(attrs[0].value, DotValue::String("box".to_string()));
            }
            other => panic!("expected DefaultNode, got {other:?}"),
        }
    }

    #[test]
    fn parse_default_edge_attributes() {
        let graph = parse("digraph { edge [color=red] }").unwrap();
        assert_eq!(graph.statements.len(), 1);
        match &graph.statements[0] {
            DotStatement::DefaultEdge(attrs) => {
                assert_eq!(attrs.len(), 1);
                assert_eq!(attrs[0].key, "color");
                assert_eq!(attrs[0].value, DotValue::String("red".to_string()));
            }
            other => panic!("expected DefaultEdge, got {other:?}"),
        }
    }

    #[test]
    fn parse_graph_level_attributes() {
        let graph = parse(r#"digraph { label = "My Graph" }"#).unwrap();
        assert_eq!(graph.statements.len(), 1);
        match &graph.statements[0] {
            DotStatement::Attr(attr) => {
                assert_eq!(attr.key, "label");
                assert_eq!(attr.value, DotValue::String("My Graph".to_string()));
            }
            other => panic!("expected Attr, got {other:?}"),
        }
    }

    #[test]
    fn parse_complex_graph() {
        let graph = parse(
            r#"digraph Pipeline {
            node [shape=box];
            edge [color=black];

            start [label="Start"];
            process [label="Process"];
            end [label="End"];

            start -> process [label="begin"];
            process -> end [label="finish"];

            rankdir = "LR";
        }"#,
        )
        .unwrap();

        assert!(graph.is_digraph);
        assert_eq!(graph.name, Some("Pipeline".to_string()));
        // default node, default edge, 3 nodes, 2 edges, 1 graph attr = 8
        assert_eq!(graph.statements.len(), 8);
    }

    #[test]
    fn parse_duration_values() {
        let graph = parse(r#"digraph { a [timeout="900s"] }"#).unwrap();
        match &graph.statements[0] {
            DotStatement::Node(node) => {
                assert_eq!(node.attrs[0].key, "timeout");
                assert_eq!(
                    node.attrs[0].value,
                    DotValue::Duration(Duration::from_secs(900))
                );
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn parse_duration_milliseconds() {
        let graph = parse(r#"digraph { a [delay="100ms"] }"#).unwrap();
        match &graph.statements[0] {
            DotStatement::Node(node) => {
                assert_eq!(
                    node.attrs[0].value,
                    DotValue::Duration(Duration::from_millis(100))
                );
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn parse_duration_minutes_and_hours() {
        let graph = parse(r#"digraph { a [short="5m", long="2h"] }"#).unwrap();
        match &graph.statements[0] {
            DotStatement::Node(node) => {
                assert_eq!(
                    node.attrs[0].value,
                    DotValue::Duration(Duration::from_secs(300))
                );
                assert_eq!(
                    node.attrs[1].value,
                    DotValue::Duration(Duration::from_secs(7200))
                );
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn parse_boolean_values() {
        let graph = parse(r#"digraph { a [critical="true", optional="false"] }"#).unwrap();
        match &graph.statements[0] {
            DotStatement::Node(node) => {
                assert_eq!(node.attrs[0].value, DotValue::Bool(true));
                assert_eq!(node.attrs[1].value, DotValue::Bool(false));
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn parse_subgraph() {
        let graph = parse(
            r#"digraph {
            subgraph cluster_0 {
                a; b;
            }
        }"#,
        )
        .unwrap();

        assert_eq!(graph.statements.len(), 1);
        match &graph.statements[0] {
            DotStatement::Subgraph(sub) => {
                assert_eq!(sub.name, Some("cluster_0".to_string()));
                assert_eq!(sub.statements.len(), 2);
            }
            other => panic!("expected Subgraph, got {other:?}"),
        }
    }

    #[test]
    fn parse_graph_default_attributes() {
        let graph = parse(r#"digraph { graph [goal="Run tests"] }"#).unwrap();
        assert_eq!(graph.statements.len(), 1);
        match &graph.statements[0] {
            DotStatement::Attr(attr) => {
                assert_eq!(attr.key, "goal");
                assert_eq!(attr.value, DotValue::String("Run tests".to_string()));
            }
            other => panic!("expected Attr from graph default, got {other:?}"),
        }
    }

    #[test]
    fn parse_graph_default_multiple_attributes() {
        let graph = parse(
            r#"digraph {
            graph [goal="Build", retry_target="implement"]
        }"#,
        )
        .unwrap();
        assert_eq!(graph.statements.len(), 2);
        match &graph.statements[0] {
            DotStatement::Attr(attr) => assert_eq!(attr.key, "goal"),
            other => panic!("expected Attr, got {other:?}"),
        }
        match &graph.statements[1] {
            DotStatement::Attr(attr) => assert_eq!(attr.key, "retry_target"),
            other => panic!("expected Attr, got {other:?}"),
        }
    }

    #[test]
    fn parse_edge_chain() {
        let graph = parse("digraph { a -> b -> c }").unwrap();
        assert_eq!(graph.statements.len(), 2);
        match &graph.statements[0] {
            DotStatement::Edge(edge) => {
                assert_eq!(edge.from, "a");
                assert_eq!(edge.to, "b");
            }
            other => panic!("expected Edge, got {other:?}"),
        }
        match &graph.statements[1] {
            DotStatement::Edge(edge) => {
                assert_eq!(edge.from, "b");
                assert_eq!(edge.to, "c");
            }
            other => panic!("expected Edge, got {other:?}"),
        }
    }

    #[test]
    fn parse_edge_chain_with_trailing_attrs() {
        let graph = parse(r#"digraph { a -> b -> c [label="end"] }"#).unwrap();
        assert_eq!(graph.statements.len(), 2);
        match &graph.statements[0] {
            DotStatement::Edge(edge) => {
                assert_eq!(edge.from, "a");
                assert_eq!(edge.to, "b");
                assert!(edge.attrs.is_empty());
            }
            other => panic!("expected Edge, got {other:?}"),
        }
        match &graph.statements[1] {
            DotStatement::Edge(edge) => {
                assert_eq!(edge.from, "b");
                assert_eq!(edge.to, "c");
                assert_eq!(edge.attrs.len(), 1);
                assert_eq!(edge.attrs[0].key, "label");
            }
            other => panic!("expected Edge, got {other:?}"),
        }
    }

    #[test]
    fn parse_long_edge_chain() {
        let graph = parse("digraph { start -> plan -> implement -> validate -> exit }").unwrap();
        assert_eq!(graph.statements.len(), 4);
        let expected = [
            ("start", "plan"),
            ("plan", "implement"),
            ("implement", "validate"),
            ("validate", "exit"),
        ];
        for (i, (from, to)) in expected.iter().enumerate() {
            match &graph.statements[i] {
                DotStatement::Edge(edge) => {
                    assert_eq!(edge.from, *from);
                    assert_eq!(edge.to, *to);
                }
                other => panic!("expected Edge at position {i}, got {other:?}"),
            }
        }
    }

    #[test]
    fn parse_makeatron_simple() {
        let input = r#"digraph simple {
            graph [goal="Run tests and report results"]
            rankdir=LR

            start [shape=Mdiamond, label="Start"]
            exit  [shape=Msquare, label="Exit"]

            run_tests [label="Run Tests", prompt="Run the test suite and report results"]
            report    [label="Report", prompt="Summarize the test results"]

            start -> run_tests -> report -> exit
        }"#;
        let graph = parse(input).unwrap();
        assert_eq!(graph.name, Some("simple".to_string()));
        // graph attr + rankdir + 4 nodes + 3 edges = 8
        assert!(
            graph.statements.len() >= 8,
            "got {} statements",
            graph.statements.len()
        );
    }

    #[test]
    fn parse_makeatron_human_gate() {
        let input = r#"digraph human_gate {
            rankdir=LR

            start [shape=Mdiamond, label="Start"]
            exit  [shape=Msquare, label="Exit"]

            implement [shape=box, label="Implement", prompt="Write the code"]
            ship_it   [shape=box, label="Ship It", prompt="Prepare the release"]
            fixes     [shape=box, label="Apply Fixes", prompt="Fix the review feedback"]

            review_gate [
                shape=hexagon,
                label="Review Changes",
                type="wait.human"
            ]

            start -> implement -> review_gate
            review_gate -> ship_it [label="[A] Approve"]
            review_gate -> fixes   [label="[F] Fix"]
            ship_it -> exit
            fixes -> review_gate
        }"#;
        let graph = parse(input).unwrap();
        assert_eq!(graph.name, Some("human_gate".to_string()));
    }

    #[test]
    fn parse_error_invalid_syntax() {
        let result = parse("digraph {");
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_missing_graph_keyword() {
        let result = parse("{ a -> b }");
        assert!(result.is_err());
    }

    #[test]
    fn parse_strict_digraph() {
        let graph = parse("strict digraph G {}").unwrap();
        assert!(graph.is_digraph);
        assert_eq!(graph.name, Some("G".to_string()));
    }

    #[test]
    fn coerce_string_value_leaves_normal_strings() {
        let val = super::coerce_string_value("hello world".to_string());
        assert_eq!(val, DotValue::String("hello world".to_string()));
    }

    #[test]
    fn coerce_string_value_parses_durations() {
        assert_eq!(
            super::coerce_string_value("900s".to_string()),
            DotValue::Duration(Duration::from_secs(900))
        );
        assert_eq!(
            super::coerce_string_value("100ms".to_string()),
            DotValue::Duration(Duration::from_millis(100))
        );
        assert_eq!(
            super::coerce_string_value("5m".to_string()),
            DotValue::Duration(Duration::from_secs(300))
        );
        assert_eq!(
            super::coerce_string_value("2h".to_string()),
            DotValue::Duration(Duration::from_secs(7200))
        );
    }

    #[test]
    fn coerce_string_value_parses_booleans() {
        assert_eq!(
            super::coerce_string_value("true".to_string()),
            DotValue::Bool(true)
        );
        assert_eq!(
            super::coerce_string_value("false".to_string()),
            DotValue::Bool(false)
        );
    }

    #[test]
    fn parse_number_attribute_value() {
        let graph = parse("digraph { a [weight=3.15] }").unwrap();
        match &graph.statements[0] {
            DotStatement::Node(node) => {
                assert_eq!(node.attrs[0].value, DotValue::Number(3.15));
            }
            other => panic!("expected Node, got {other:?}"),
        }
    }

    #[test]
    fn parse_undirected_edge() {
        let graph = parse("graph { a -- b }").unwrap();
        assert!(!graph.is_digraph);
        match &graph.statements[0] {
            DotStatement::Edge(edge) => {
                assert_eq!(edge.from, "a");
                assert_eq!(edge.to, "b");
            }
            other => panic!("expected Edge, got {other:?}"),
        }
    }
}
