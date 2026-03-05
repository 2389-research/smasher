// ABOUTME: Tool handler for executing external tool nodes in the pipeline.
// ABOUTME: Maps tool node attributes to tool invocations and captures results.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::graph::{GraphNode, NodeAttrValue, NodeType};
use crate::handler::{Handler, HandlerError};
use crate::state::{Context, Outcome};

/// Abstraction for executing external tools during pipeline processing.
///
/// Implementors provide the actual mechanism for running tools (CLI, API calls,
/// subprocess spawning, etc.) while the handler takes care of attribute extraction
/// and context bookkeeping.
#[async_trait::async_trait]
pub trait ToolBackend: Send + Sync {
    /// Execute a tool with the given name, arguments, and context.
    async fn execute_tool(
        &self,
        tool_name: &str,
        args: &Value,
        context: &Context,
    ) -> Result<Outcome, HandlerError>;

    /// List available tool names.
    fn available_tools(&self) -> Vec<String>;
}

/// Handler for Tool nodes. Delegates execution to a pluggable `ToolBackend`.
///
/// Attribute extraction:
/// - `tool` (String): The name of the tool to run. Falls back to node label.
/// - `args` (String): Optional JSON-encoded arguments for the tool.
pub struct ToolHandler {
    backend: Arc<dyn ToolBackend>,
}

impl ToolHandler {
    /// Create a new ToolHandler backed by the given backend.
    pub fn new(backend: Arc<dyn ToolBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait::async_trait]
impl Handler for ToolHandler {
    fn name(&self) -> &str {
        "tool"
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        // Determine tool name: explicit attribute first, then label fallback.
        let tool_name = match node.attrs.get("tool") {
            Some(NodeAttrValue::String(s)) => s.clone(),
            _ => match &node.label {
                Some(label) => label.clone(),
                None => {
                    return Ok(Outcome::failure("no tool specified"));
                }
            },
        };

        // Parse optional args attribute as JSON.
        let args = match node.attrs.get("args") {
            Some(NodeAttrValue::String(s)) => match serde_json::from_str::<Value>(s) {
                Ok(parsed) => parsed,
                Err(e) => {
                    return Ok(Outcome::failure(format!(
                        "invalid JSON in args attribute: {e}"
                    )));
                }
            },
            _ => json!({}),
        };

        let outcome = self
            .backend
            .execute_tool(&tool_name, &args, context)
            .await?;

        // Store the result in context for downstream nodes.
        let serialized = match &outcome {
            Outcome::Success { data, .. } => json!({
                "status": "success",
                "data": data,
            }),
            Outcome::Failure {
                error, retryable, ..
            } => json!({
                "status": "failure",
                "error": error,
                "retryable": retryable,
            }),
            Outcome::PartialSuccess { data, .. } => json!({
                "status": "partial_success",
                "data": data,
            }),
            Outcome::Retry { reason, .. } => json!({
                "status": "retry",
                "reason": reason,
            }),
            Outcome::Skip { reason, .. } => json!({
                "status": "skip",
                "reason": reason,
            }),
        };
        context.set(format!("_tool_{}", node.id), serialized);

        Ok(outcome)
    }

    fn handles(&self, node_type: &NodeType) -> bool {
        matches!(node_type, NodeType::Tool)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // -- Test helpers -------------------------------------------------------

    /// Build a minimal GraphNode with the given type and optional attributes.
    fn make_node(id: &str, node_type: NodeType) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
            label: None,
            attrs: HashMap::new(),
        }
    }

    /// Build a GraphNode with a label.
    fn make_node_with_label(id: &str, node_type: NodeType, label: &str) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
            label: Some(label.to_string()),
            attrs: HashMap::new(),
        }
    }

    /// A test ToolBackend that echoes the tool name and args back as success data.
    struct EchoToolBackend;

    #[async_trait::async_trait]
    impl ToolBackend for EchoToolBackend {
        async fn execute_tool(
            &self,
            tool_name: &str,
            args: &Value,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            Ok(Outcome::success_with(json!({
                "tool": tool_name,
                "args": args,
            })))
        }

        fn available_tools(&self) -> Vec<String> {
            vec!["echo".to_string(), "format".to_string()]
        }
    }

    /// A test ToolBackend that always returns a failure.
    struct FailingToolBackend;

    #[async_trait::async_trait]
    impl ToolBackend for FailingToolBackend {
        async fn execute_tool(
            &self,
            tool_name: &str,
            _args: &Value,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            Ok(Outcome::failure(format!("tool '{tool_name}' is broken")))
        }

        fn available_tools(&self) -> Vec<String> {
            vec![]
        }
    }

    /// A test ToolBackend that returns a HandlerError instead of an Outcome.
    struct ErroringToolBackend;

    #[async_trait::async_trait]
    impl ToolBackend for ErroringToolBackend {
        async fn execute_tool(
            &self,
            tool_name: &str,
            _args: &Value,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            Err(HandlerError::ExecutionFailed {
                handler: "tool".to_string(),
                node_id: "unknown".to_string(),
                message: format!("backend error for '{tool_name}'"),
            })
        }

        fn available_tools(&self) -> Vec<String> {
            vec![]
        }
    }

    /// A test ToolBackend that tracks invocation count.
    struct CountingToolBackend {
        call_count: AtomicUsize,
    }

    impl CountingToolBackend {
        fn new() -> Self {
            Self {
                call_count: AtomicUsize::new(0),
            }
        }

        fn count(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl ToolBackend for CountingToolBackend {
        async fn execute_tool(
            &self,
            _tool_name: &str,
            _args: &Value,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(Outcome::success())
        }

        fn available_tools(&self) -> Vec<String> {
            vec!["counter".to_string()]
        }
    }

    // ---------------------------------------------------------------
    // Test 1: ToolHandler delegates to backend
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn tool_handler_delegates_to_backend() {
        let backend = Arc::new(EchoToolBackend);
        let handler = ToolHandler::new(backend);

        let mut node = make_node("t1", NodeType::Tool);
        node.attrs.insert(
            "tool".to_string(),
            NodeAttrValue::String("echo".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_success());
        match result {
            Outcome::Success {
                data: Some(data), ..
            } => {
                assert_eq!(data["tool"], "echo");
                assert_eq!(data["args"], json!({}));
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 2: ToolHandler with args attribute (JSON)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn tool_handler_with_args_attribute() {
        let backend = Arc::new(EchoToolBackend);
        let handler = ToolHandler::new(backend);

        let mut node = make_node("t2", NodeType::Tool);
        node.attrs.insert(
            "tool".to_string(),
            NodeAttrValue::String("format".to_string()),
        );
        node.attrs.insert(
            "args".to_string(),
            NodeAttrValue::String(r#"{"input": "hello", "style": "bold"}"#.to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_success());
        match result {
            Outcome::Success {
                data: Some(data), ..
            } => {
                assert_eq!(data["tool"], "format");
                assert_eq!(data["args"]["input"], "hello");
                assert_eq!(data["args"]["style"], "bold");
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 3: ToolHandler falls back to label as tool name
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn tool_handler_falls_back_to_label() {
        let backend = Arc::new(EchoToolBackend);
        let handler = ToolHandler::new(backend);

        let node = make_node_with_label("t3", NodeType::Tool, "my_tool");

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_success());
        match result {
            Outcome::Success {
                data: Some(data), ..
            } => {
                assert_eq!(data["tool"], "my_tool");
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 4: ToolHandler no tool returns failure
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn tool_handler_no_tool_returns_failure() {
        let backend = Arc::new(EchoToolBackend);
        let handler = ToolHandler::new(backend);

        let node = make_node("t4", NodeType::Tool);

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_failure());
        match result {
            Outcome::Failure { error, .. } => {
                assert!(error.contains("no tool specified"));
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 5: ToolHandler handles only Tool nodes
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn tool_handler_handles_only_tool_nodes() {
        let backend = Arc::new(EchoToolBackend);
        let handler = ToolHandler::new(backend);

        assert!(handler.handles(&NodeType::Tool));
        assert!(!handler.handles(&NodeType::Start));
        assert!(!handler.handles(&NodeType::Exit));
        assert!(!handler.handles(&NodeType::Codergen));
        assert!(!handler.handles(&NodeType::Conditional));
        assert!(!handler.handles(&NodeType::Interviewer));
        assert!(!handler.handles(&NodeType::Parallel));
        assert!(!handler.handles(&NodeType::Manager));
        assert!(!handler.handles(&NodeType::Generic));
    }

    // ---------------------------------------------------------------
    // Test 6: ToolHandler stores result in context
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn tool_handler_stores_result_in_context() {
        let backend = Arc::new(EchoToolBackend);
        let handler = ToolHandler::new(backend);

        let mut node = make_node("t5", NodeType::Tool);
        node.attrs.insert(
            "tool".to_string(),
            NodeAttrValue::String("echo".to_string()),
        );

        let ctx = Context::new();
        handler.execute(&node, &ctx).await.unwrap();

        let stored = ctx.get("_tool_t5").expect("should have stored result");
        assert_eq!(stored["status"], "success");
        assert!(stored["data"].is_object());
    }

    // ---------------------------------------------------------------
    // Test 7: ToolBackend available_tools
    // ---------------------------------------------------------------

    #[test]
    fn tool_backend_available_tools() {
        let backend = EchoToolBackend;
        let tools = backend.available_tools();
        assert_eq!(tools.len(), 2);
        assert!(tools.contains(&"echo".to_string()));
        assert!(tools.contains(&"format".to_string()));
    }

    // ---------------------------------------------------------------
    // Test 8: ToolHandler with invalid JSON args returns failure
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn tool_handler_invalid_json_args_returns_failure() {
        let backend = Arc::new(EchoToolBackend);
        let handler = ToolHandler::new(backend);

        let mut node = make_node("t6", NodeType::Tool);
        node.attrs.insert(
            "tool".to_string(),
            NodeAttrValue::String("echo".to_string()),
        );
        node.attrs.insert(
            "args".to_string(),
            NodeAttrValue::String("{not valid json}".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_failure());
        match result {
            Outcome::Failure { error, .. } => {
                assert!(error.contains("invalid JSON in args attribute"));
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 9: ToolHandler name is correct
    // ---------------------------------------------------------------

    #[test]
    fn tool_handler_name_is_correct() {
        let backend = Arc::new(EchoToolBackend);
        let handler = ToolHandler::new(backend);
        assert_eq!(handler.name(), "tool");
    }

    // ---------------------------------------------------------------
    // Test 10: ToolHandler stores failure outcome in context
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn tool_handler_stores_failure_in_context() {
        let backend = Arc::new(FailingToolBackend);
        let handler = ToolHandler::new(backend);

        let mut node = make_node("t7", NodeType::Tool);
        node.attrs.insert(
            "tool".to_string(),
            NodeAttrValue::String("broken".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_failure());

        let stored = ctx.get("_tool_t7").expect("should have stored result");
        assert_eq!(stored["status"], "failure");
        assert!(stored["error"].as_str().unwrap().contains("broken"));
    }

    // ---------------------------------------------------------------
    // Test 11: ToolHandler propagates HandlerError from backend
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn tool_handler_propagates_handler_error() {
        let backend = Arc::new(ErroringToolBackend);
        let handler = ToolHandler::new(backend);

        let mut node = make_node("t8", NodeType::Tool);
        node.attrs.insert(
            "tool".to_string(),
            NodeAttrValue::String("crash".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            HandlerError::ExecutionFailed { message, .. } => {
                assert!(message.contains("backend error for 'crash'"));
            }
            other => panic!("expected ExecutionFailed, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 12: Tool attribute takes precedence over label
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn tool_attribute_takes_precedence_over_label() {
        let backend = Arc::new(EchoToolBackend);
        let handler = ToolHandler::new(backend);

        let mut node = make_node_with_label("t9", NodeType::Tool, "label_tool");
        node.attrs.insert(
            "tool".to_string(),
            NodeAttrValue::String("attr_tool".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        match result {
            Outcome::Success {
                data: Some(data), ..
            } => {
                assert_eq!(data["tool"], "attr_tool");
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 13: ToolHandler invokes backend exactly once
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn tool_handler_invokes_backend_once() {
        let backend = Arc::new(CountingToolBackend::new());
        let backend_trait: Arc<dyn ToolBackend> = Arc::clone(&backend) as Arc<dyn ToolBackend>;
        let handler = ToolHandler::new(backend_trait);

        let mut node = make_node("t10", NodeType::Tool);
        node.attrs.insert(
            "tool".to_string(),
            NodeAttrValue::String("counter".to_string()),
        );

        let ctx = Context::new();
        handler.execute(&node, &ctx).await.unwrap();

        assert_eq!(backend.count(), 1);
    }

    // ---------------------------------------------------------------
    // Test 14: Empty tools list from failing backend
    // ---------------------------------------------------------------

    #[test]
    fn failing_backend_has_no_available_tools() {
        let backend = FailingToolBackend;
        assert!(backend.available_tools().is_empty());
    }
}
