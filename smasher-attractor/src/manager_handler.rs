// ABOUTME: Manager handler for coordinating sub-pipeline execution.
// ABOUTME: Orchestrates child pipelines and aggregates their results.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::graph::{GraphNode, NodeAttrValue, NodeType};
use crate::handler::{Handler, HandlerError};
use crate::state::{Context, Outcome};

/// Abstraction for managing sub-pipelines or coordination tasks.
///
/// Implementors provide the mechanism for running sub-pipelines, delegating
/// work to child agents, or performing multi-step coordination workflows.
#[async_trait::async_trait]
pub trait ManagerBackend: Send + Sync {
    /// Execute a sub-pipeline or coordination task.
    async fn coordinate(
        &self,
        task: &str,
        config: &Value,
        context: &Context,
    ) -> Result<Outcome, HandlerError>;
}

/// Handler for Manager nodes. Delegates coordination to a pluggable `ManagerBackend`.
///
/// Attribute extraction:
/// - `task` (String): The name/identifier of the coordination task. Falls back to node label.
/// - `config` (String): Optional JSON-encoded configuration for the task.
pub struct ManagerHandler {
    backend: Arc<dyn ManagerBackend>,
}

impl ManagerHandler {
    /// Create a new ManagerHandler backed by the given backend.
    pub fn new(backend: Arc<dyn ManagerBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait::async_trait]
impl Handler for ManagerHandler {
    fn name(&self) -> &str {
        "manager"
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        // Determine task: explicit attribute first, then label fallback.
        let task = match node.attrs.get("task") {
            Some(NodeAttrValue::String(s)) => s.clone(),
            _ => match &node.label {
                Some(label) => label.clone(),
                None => {
                    return Ok(Outcome::failure("no task specified for manager node"));
                }
            },
        };

        // Parse optional config attribute as JSON.
        let config = match node.attrs.get("config") {
            Some(NodeAttrValue::String(s)) => match serde_json::from_str::<Value>(s) {
                Ok(parsed) => parsed,
                Err(e) => {
                    return Ok(Outcome::failure(format!(
                        "invalid JSON in config attribute: {e}"
                    )));
                }
            },
            _ => json!({}),
        };

        let outcome = self.backend.coordinate(&task, &config, context).await?;

        // Store the result in context for downstream nodes.
        let serialized = match &outcome {
            Outcome::Success { data } => json!({
                "status": "success",
                "data": data,
            }),
            Outcome::Failure { error, retryable } => json!({
                "status": "failure",
                "error": error,
                "retryable": retryable,
            }),
            Outcome::Skip { reason } => json!({
                "status": "skip",
                "reason": reason,
            }),
        };
        context.set(format!("_manager_{}", node.id), serialized);

        Ok(outcome)
    }

    fn handles(&self, node_type: &NodeType) -> bool {
        matches!(node_type, NodeType::Manager)
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

    /// A test ManagerBackend that echoes the task and config back as success data.
    struct EchoManagerBackend;

    #[async_trait::async_trait]
    impl ManagerBackend for EchoManagerBackend {
        async fn coordinate(
            &self,
            task: &str,
            config: &Value,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            Ok(Outcome::success_with(json!({
                "task": task,
                "config": config,
            })))
        }
    }

    /// A test ManagerBackend that always returns a failure.
    struct FailingManagerBackend;

    #[async_trait::async_trait]
    impl ManagerBackend for FailingManagerBackend {
        async fn coordinate(
            &self,
            task: &str,
            _config: &Value,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            Ok(Outcome::failure(format!(
                "coordination failed for task '{task}'"
            )))
        }
    }

    /// A test ManagerBackend that returns a HandlerError instead of an Outcome.
    struct ErroringManagerBackend;

    #[async_trait::async_trait]
    impl ManagerBackend for ErroringManagerBackend {
        async fn coordinate(
            &self,
            task: &str,
            _config: &Value,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            Err(HandlerError::ExecutionFailed {
                handler: "manager".to_string(),
                node_id: "unknown".to_string(),
                message: format!("backend error for task '{task}'"),
            })
        }
    }

    /// A test ManagerBackend that tracks invocation count.
    struct CountingManagerBackend {
        call_count: AtomicUsize,
    }

    impl CountingManagerBackend {
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
    impl ManagerBackend for CountingManagerBackend {
        async fn coordinate(
            &self,
            _task: &str,
            _config: &Value,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(Outcome::success())
        }
    }

    /// A test ManagerBackend that reads from context to verify context is passed.
    struct ContextReadingManagerBackend;

    #[async_trait::async_trait]
    impl ManagerBackend for ContextReadingManagerBackend {
        async fn coordinate(
            &self,
            task: &str,
            _config: &Value,
            context: &Context,
        ) -> Result<Outcome, HandlerError> {
            let value = context
                .get_string("upstream_data")
                .unwrap_or_else(|| "missing".to_string());
            Ok(Outcome::success_with(json!({
                "task": task,
                "upstream": value,
            })))
        }
    }

    // ---------------------------------------------------------------
    // Test 1: ManagerHandler delegates to backend
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn manager_handler_delegates_to_backend() {
        let backend = Arc::new(EchoManagerBackend);
        let handler = ManagerHandler::new(backend);

        let mut node = make_node("m1", NodeType::Manager);
        node.attrs.insert(
            "task".to_string(),
            NodeAttrValue::String("deploy_pipeline".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_success());
        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data["task"], "deploy_pipeline");
                assert_eq!(data["config"], json!({}));
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 2: ManagerHandler with config attribute
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn manager_handler_with_config_attribute() {
        let backend = Arc::new(EchoManagerBackend);
        let handler = ManagerHandler::new(backend);

        let mut node = make_node("m2", NodeType::Manager);
        node.attrs.insert(
            "task".to_string(),
            NodeAttrValue::String("run_tests".to_string()),
        );
        node.attrs.insert(
            "config".to_string(),
            NodeAttrValue::String(r#"{"parallelism": 4, "timeout": 300}"#.to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_success());
        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data["task"], "run_tests");
                assert_eq!(data["config"]["parallelism"], 4);
                assert_eq!(data["config"]["timeout"], 300);
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 3: ManagerHandler falls back to label
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn manager_handler_falls_back_to_label() {
        let backend = Arc::new(EchoManagerBackend);
        let handler = ManagerHandler::new(backend);

        let node = make_node_with_label("m3", NodeType::Manager, "build_and_deploy");

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_success());
        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data["task"], "build_and_deploy");
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 4: ManagerHandler no task returns failure
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn manager_handler_no_task_returns_failure() {
        let backend = Arc::new(EchoManagerBackend);
        let handler = ManagerHandler::new(backend);

        let node = make_node("m4", NodeType::Manager);

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_failure());
        match result {
            Outcome::Failure { error, .. } => {
                assert!(error.contains("no task specified for manager node"));
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 5: ManagerHandler handles only Manager nodes
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn manager_handler_handles_only_manager_nodes() {
        let backend = Arc::new(EchoManagerBackend);
        let handler = ManagerHandler::new(backend);

        assert!(handler.handles(&NodeType::Manager));
        assert!(!handler.handles(&NodeType::Start));
        assert!(!handler.handles(&NodeType::Exit));
        assert!(!handler.handles(&NodeType::Codergen));
        assert!(!handler.handles(&NodeType::Conditional));
        assert!(!handler.handles(&NodeType::Interviewer));
        assert!(!handler.handles(&NodeType::Parallel));
        assert!(!handler.handles(&NodeType::Tool));
        assert!(!handler.handles(&NodeType::SubPipeline));
        assert!(!handler.handles(&NodeType::Generic));
    }

    // ---------------------------------------------------------------
    // Test 6: ManagerHandler stores result in context
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn manager_handler_stores_result_in_context() {
        let backend = Arc::new(EchoManagerBackend);
        let handler = ManagerHandler::new(backend);

        let mut node = make_node("m5", NodeType::Manager);
        node.attrs.insert(
            "task".to_string(),
            NodeAttrValue::String("orchestrate".to_string()),
        );

        let ctx = Context::new();
        handler.execute(&node, &ctx).await.unwrap();

        let stored = ctx.get("_manager_m5").expect("should have stored result");
        assert_eq!(stored["status"], "success");
        assert!(stored["data"].is_object());
    }

    // ---------------------------------------------------------------
    // Test 7: ManagerBackend coordination (context is passed through)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn manager_backend_receives_context() {
        let backend = Arc::new(ContextReadingManagerBackend);
        let handler = ManagerHandler::new(backend);

        let mut node = make_node("m6", NodeType::Manager);
        node.attrs.insert(
            "task".to_string(),
            NodeAttrValue::String("check_upstream".to_string()),
        );

        let ctx = Context::new();
        ctx.set("upstream_data", json!("some_value"));

        let result = handler.execute(&node, &ctx).await.unwrap();

        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data["task"], "check_upstream");
                assert_eq!(data["upstream"], "some_value");
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 8: ManagerHandler with invalid config JSON
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn manager_handler_invalid_config_json() {
        let backend = Arc::new(EchoManagerBackend);
        let handler = ManagerHandler::new(backend);

        let mut node = make_node("m7", NodeType::Manager);
        node.attrs.insert(
            "task".to_string(),
            NodeAttrValue::String("deploy".to_string()),
        );
        node.attrs.insert(
            "config".to_string(),
            NodeAttrValue::String("{broken json!!!}".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_failure());
        match result {
            Outcome::Failure { error, .. } => {
                assert!(error.contains("invalid JSON in config attribute"));
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 9: ManagerHandler name is correct
    // ---------------------------------------------------------------

    #[test]
    fn manager_handler_name_is_correct() {
        let backend = Arc::new(EchoManagerBackend);
        let handler = ManagerHandler::new(backend);
        assert_eq!(handler.name(), "manager");
    }

    // ---------------------------------------------------------------
    // Test 10: Task attribute takes precedence over label
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn task_attribute_takes_precedence_over_label() {
        let backend = Arc::new(EchoManagerBackend);
        let handler = ManagerHandler::new(backend);

        let mut node = make_node_with_label("m8", NodeType::Manager, "label_task");
        node.attrs.insert(
            "task".to_string(),
            NodeAttrValue::String("attr_task".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data["task"], "attr_task");
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 11: ManagerHandler stores failure outcome in context
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn manager_handler_stores_failure_in_context() {
        let backend = Arc::new(FailingManagerBackend);
        let handler = ManagerHandler::new(backend);

        let mut node = make_node("m9", NodeType::Manager);
        node.attrs.insert(
            "task".to_string(),
            NodeAttrValue::String("broken_task".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_failure());

        let stored = ctx.get("_manager_m9").expect("should have stored result");
        assert_eq!(stored["status"], "failure");
        assert!(stored["error"].as_str().unwrap().contains("broken_task"));
    }

    // ---------------------------------------------------------------
    // Test 12: ManagerHandler propagates HandlerError from backend
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn manager_handler_propagates_handler_error() {
        let backend = Arc::new(ErroringManagerBackend);
        let handler = ManagerHandler::new(backend);

        let mut node = make_node("m10", NodeType::Manager);
        node.attrs.insert(
            "task".to_string(),
            NodeAttrValue::String("crash_task".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            HandlerError::ExecutionFailed { message, .. } => {
                assert!(message.contains("backend error for task 'crash_task'"));
            }
            other => panic!("expected ExecutionFailed, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 13: ManagerHandler invokes backend exactly once
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn manager_handler_invokes_backend_once() {
        let backend = Arc::new(CountingManagerBackend::new());
        let backend_trait: Arc<dyn ManagerBackend> =
            Arc::clone(&backend) as Arc<dyn ManagerBackend>;
        let handler = ManagerHandler::new(backend_trait);

        let mut node = make_node("m11", NodeType::Manager);
        node.attrs.insert(
            "task".to_string(),
            NodeAttrValue::String("count_me".to_string()),
        );

        let ctx = Context::new();
        handler.execute(&node, &ctx).await.unwrap();

        assert_eq!(backend.count(), 1);
    }

    // ---------------------------------------------------------------
    // Test 14: ManagerHandler with empty config (no config attr)
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn manager_handler_no_config_defaults_to_empty_object() {
        let backend = Arc::new(EchoManagerBackend);
        let handler = ManagerHandler::new(backend);

        let mut node = make_node("m12", NodeType::Manager);
        node.attrs.insert(
            "task".to_string(),
            NodeAttrValue::String("simple_task".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data["config"], json!({}));
            }
            other => panic!("expected success, got {other:?}"),
        }
    }
}
