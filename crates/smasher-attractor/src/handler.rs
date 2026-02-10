// ABOUTME: Handler trait and registry for pipeline node execution strategies.
// ABOUTME: Includes Start, Exit, Conditional, and Codergen handler implementations.

//! Handler trait and registry for pipeline node execution.
//!
//! Every node in a pipeline graph is processed by a [`Handler`] -- an async
//! trait that receives the node definition and the shared [`Context`], then
//! returns an [`Outcome`] (success, failure, or skip).
//!
//! The [`HandlerRegistry`] maps node types to concrete handler implementations.
//! When the engine visits a node, the registry finds the first handler whose
//! [`Handler::handles`] method returns `true` for that node's type and
//! delegates execution to it.
//!
//! Built-in handlers are provided for the fundamental node types:
//!
//! - [`StartHandler`] -- marks the pipeline as started.
//! - [`ExitHandler`] -- marks the pipeline as completed.
//! - [`ConditionalHandler`] -- evaluates a boolean condition from node attributes.
//! - [`CodergenHandler`] -- delegates to a pluggable [`CodergenBackend`] for
//!   LLM-powered code generation.
//!
//! Use [`default_registry`] to obtain a registry pre-loaded with Start, Exit,
//! and Conditional handlers.
//!
//! # Implementing a custom handler
//!
//! ```
//! use std::sync::Arc;
//! use async_trait::async_trait;
//! use smasher_attractor::graph::{GraphNode, NodeType};
//! use smasher_attractor::handler::{Handler, HandlerError, HandlerRegistry};
//! use smasher_attractor::state::{Context, Outcome};
//! use serde_json::json;
//!
//! struct MyHandler;
//!
//! #[async_trait]
//! impl Handler for MyHandler {
//!     fn name(&self) -> &str { "my_handler" }
//!
//!     async fn execute(
//!         &self,
//!         node: &GraphNode,
//!         context: &Context,
//!     ) -> Result<Outcome, HandlerError> {
//!         // Read from context, do work, write results back.
//!         context.set(format!("{}_done", node.id), json!(true));
//!         Ok(Outcome::success())
//!     }
//!
//!     fn handles(&self, node_type: &NodeType) -> bool {
//!         matches!(node_type, NodeType::Generic)
//!     }
//! }
//!
//! let mut registry = HandlerRegistry::new();
//! registry.register(Arc::new(MyHandler));
//! assert!(registry.get_handler(&NodeType::Generic).is_some());
//! ```

use std::sync::Arc;

use serde_json::json;

use crate::condition::{evaluate_condition, parse_condition};
use crate::graph::{GraphNode, NodeAttrValue, NodeType};
use crate::state::{Context, Outcome};

/// Errors that arise during handler lookup or execution.
#[derive(Debug, thiserror::Error)]
pub enum HandlerError {
    #[error("handler '{handler}' failed on node '{node_id}': {message}")]
    ExecutionFailed {
        handler: String,
        node_id: String,
        message: String,
    },
    #[error("no handler registered for node type '{node_type}'")]
    NoHandler { node_type: String },
    #[error("handler error: {0}")]
    Other(String),
}

/// Core abstraction for executing a pipeline node.
///
/// Each handler declares which node types it can process and provides
/// an async `execute` method that runs the node logic against a shared context.
#[async_trait::async_trait]
pub trait Handler: Send + Sync {
    /// The name of this handler type.
    fn name(&self) -> &str;

    /// Execute this handler for the given node with the given context.
    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError>;

    /// Whether this handler can handle the given node type.
    fn handles(&self, node_type: &NodeType) -> bool;
}

/// Trait for bridging Codergen nodes to the agent / LLM layer.
#[async_trait::async_trait]
pub trait CodergenBackend: Send + Sync {
    /// Execute a code generation task.
    async fn generate(
        &self,
        prompt: &str,
        model: Option<&str>,
        context: &Context,
    ) -> Result<Outcome, HandlerError>;
}

// ---------------------------------------------------------------------------
// Built-in handlers
// ---------------------------------------------------------------------------

/// Handler for Start nodes. Sets `_started` in context and returns success.
pub struct StartHandler;

#[async_trait::async_trait]
impl Handler for StartHandler {
    fn name(&self) -> &str {
        "start"
    }

    async fn execute(&self, _node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        context.set("_started", json!("true"));
        Ok(Outcome::success())
    }

    fn handles(&self, node_type: &NodeType) -> bool {
        matches!(node_type, NodeType::Start)
    }
}

/// Handler for Exit nodes. Sets `_completed` in context and returns success.
pub struct ExitHandler;

#[async_trait::async_trait]
impl Handler for ExitHandler {
    fn name(&self) -> &str {
        "exit"
    }

    async fn execute(&self, _node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        context.set("_completed", json!("true"));
        Ok(Outcome::success())
    }

    fn handles(&self, node_type: &NodeType) -> bool {
        matches!(node_type, NodeType::Exit)
    }
}

/// Handler for Conditional nodes. Parses and evaluates the `condition`
/// attribute against the context's string map.
pub struct ConditionalHandler;

#[async_trait::async_trait]
impl Handler for ConditionalHandler {
    fn name(&self) -> &str {
        "conditional"
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        let condition_str = match node.attrs.get("condition") {
            Some(NodeAttrValue::String(s)) => s.clone(),
            _ => {
                return Ok(Outcome::failure(
                    "no condition attribute on conditional node",
                ));
            }
        };

        let parsed = match parse_condition(&condition_str) {
            Ok(c) => c,
            Err(e) => {
                return Ok(Outcome::failure(e.to_string()));
            }
        };

        let string_map = context.to_string_map();
        let result = evaluate_condition(&parsed, &string_map);
        Ok(Outcome::success_with(json!({"result": result})))
    }

    fn handles(&self, node_type: &NodeType) -> bool {
        matches!(node_type, NodeType::Conditional)
    }
}

/// Handler for Codergen nodes. Delegates to a pluggable `CodergenBackend`.
pub struct CodergenHandler {
    backend: Arc<dyn CodergenBackend>,
}

impl CodergenHandler {
    /// Create a new CodergenHandler backed by the given backend.
    pub fn new(backend: Arc<dyn CodergenBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait::async_trait]
impl Handler for CodergenHandler {
    fn name(&self) -> &str {
        "codergen"
    }

    async fn execute(&self, node: &GraphNode, context: &Context) -> Result<Outcome, HandlerError> {
        // Determine prompt: explicit attribute first, then label fallback.
        let prompt = match node.attrs.get("prompt") {
            Some(NodeAttrValue::String(s)) => s.clone(),
            _ => match &node.label {
                Some(label) => label.clone(),
                None => {
                    return Ok(Outcome::failure("no prompt specified"));
                }
            },
        };

        // Determine optional model override.
        let model = match node.attrs.get("model") {
            Some(NodeAttrValue::String(s)) => Some(s.as_str()),
            _ => None,
        };

        // Store the current node id so the backend can tag agent-level events.
        context.set("_current_node_id", json!(node.id));

        self.backend.generate(&prompt, model, context).await
    }

    fn handles(&self, node_type: &NodeType) -> bool {
        matches!(node_type, NodeType::Codergen)
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Maps node types to handlers and dispatches execution.
pub struct HandlerRegistry {
    handlers: Vec<Arc<dyn Handler>>,
}

impl std::fmt::Debug for HandlerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerRegistry")
            .field("handler_count", &self.handlers.len())
            .finish()
    }
}

impl HandlerRegistry {
    /// Create an empty handler registry.
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
        }
    }

    /// Register a handler. The first handler that returns `true` from
    /// `handles()` for a given node type will be used.
    pub fn register(&mut self, handler: Arc<dyn Handler>) {
        self.handlers.push(handler);
    }

    /// Find the first registered handler that can process the given node type.
    pub fn get_handler(&self, node_type: &NodeType) -> Option<&Arc<dyn Handler>> {
        self.handlers.iter().find(|h| h.handles(node_type))
    }

    /// Look up a handler for the node's type and execute it.
    ///
    /// Returns `HandlerError::NoHandler` if no registered handler matches.
    pub async fn execute(
        &self,
        node: &GraphNode,
        context: &Context,
    ) -> Result<Outcome, HandlerError> {
        let handler = self
            .get_handler(&node.node_type)
            .ok_or_else(|| HandlerError::NoHandler {
                node_type: format!("{:?}", node.node_type),
            })?;
        handler.execute(node, context).await
    }
}

impl Default for HandlerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a registry pre-loaded with the built-in handlers:
/// StartHandler, ExitHandler, and ConditionalHandler.
///
/// CodergenHandler is not included because it requires a backend.
pub fn default_registry() -> HandlerRegistry {
    let mut registry = HandlerRegistry::new();
    registry.register(Arc::new(StartHandler));
    registry.register(Arc::new(ExitHandler));
    registry.register(Arc::new(ConditionalHandler));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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

    /// A real (non-mock) CodergenBackend for testing that echoes the prompt back.
    struct TestCodergenBackend;

    #[async_trait::async_trait]
    impl CodergenBackend for TestCodergenBackend {
        async fn generate(
            &self,
            prompt: &str,
            _model: Option<&str>,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            Ok(Outcome::success_with(json!({"generated": prompt})))
        }
    }

    /// A CodergenBackend that captures the model parameter for verification.
    struct ModelCapturingBackend;

    #[async_trait::async_trait]
    impl CodergenBackend for ModelCapturingBackend {
        async fn generate(
            &self,
            prompt: &str,
            model: Option<&str>,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            Ok(Outcome::success_with(
                json!({"prompt": prompt, "model": model}),
            ))
        }
    }

    /// A handler that claims to handle Generic nodes, for registry testing.
    struct GenericTestHandler;

    #[async_trait::async_trait]
    impl Handler for GenericTestHandler {
        fn name(&self) -> &str {
            "generic_test"
        }

        async fn execute(
            &self,
            _node: &GraphNode,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            Ok(Outcome::success_with(json!({"handler": "generic_test"})))
        }

        fn handles(&self, node_type: &NodeType) -> bool {
            matches!(node_type, NodeType::Generic)
        }
    }

    /// A second handler for Start nodes, used to verify first-match-wins.
    struct AlternateStartHandler;

    #[async_trait::async_trait]
    impl Handler for AlternateStartHandler {
        fn name(&self) -> &str {
            "alternate_start"
        }

        async fn execute(
            &self,
            _node: &GraphNode,
            context: &Context,
        ) -> Result<Outcome, HandlerError> {
            context.set("_alternate", json!("true"));
            Ok(Outcome::success())
        }

        fn handles(&self, node_type: &NodeType) -> bool {
            matches!(node_type, NodeType::Start)
        }
    }

    // ---------------------------------------------------------------
    // StartHandler tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn start_handler_sets_context_and_returns_success() {
        let handler = StartHandler;
        let node = make_node("s1", NodeType::Start);
        let ctx = Context::new();

        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_success());
        assert_eq!(ctx.get_string("_started"), Some("true".to_string()));
    }

    #[tokio::test]
    async fn start_handler_handles_only_start_nodes() {
        let handler = StartHandler;
        assert!(handler.handles(&NodeType::Start));
        assert!(!handler.handles(&NodeType::Exit));
        assert!(!handler.handles(&NodeType::Codergen));
        assert!(!handler.handles(&NodeType::Conditional));
        assert!(!handler.handles(&NodeType::Generic));
    }

    // ---------------------------------------------------------------
    // ExitHandler tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn exit_handler_sets_context_and_returns_success() {
        let handler = ExitHandler;
        let node = make_node("e1", NodeType::Exit);
        let ctx = Context::new();

        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_success());
        assert_eq!(ctx.get_string("_completed"), Some("true".to_string()));
    }

    #[tokio::test]
    async fn exit_handler_handles_only_exit_nodes() {
        let handler = ExitHandler;
        assert!(handler.handles(&NodeType::Exit));
        assert!(!handler.handles(&NodeType::Start));
        assert!(!handler.handles(&NodeType::Codergen));
    }

    // ---------------------------------------------------------------
    // ConditionalHandler tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn conditional_handler_true_condition() {
        let handler = ConditionalHandler;
        let mut node = make_node("c1", NodeType::Conditional);
        node.attrs.insert(
            "condition".to_string(),
            NodeAttrValue::String("status=done".to_string()),
        );

        let ctx = Context::new();
        ctx.set("status", json!("done"));

        let result = handler.execute(&node, &ctx).await.unwrap();
        assert!(result.is_success());
        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data, json!({"result": true}));
            }
            other => panic!("expected Success with data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn conditional_handler_false_condition() {
        let handler = ConditionalHandler;
        let mut node = make_node("c2", NodeType::Conditional);
        node.attrs.insert(
            "condition".to_string(),
            NodeAttrValue::String("status=done".to_string()),
        );

        let ctx = Context::new();
        ctx.set("status", json!("pending"));

        let result = handler.execute(&node, &ctx).await.unwrap();
        assert!(result.is_success());
        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data, json!({"result": false}));
            }
            other => panic!("expected Success with data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn conditional_handler_no_condition_attribute() {
        let handler = ConditionalHandler;
        let node = make_node("c3", NodeType::Conditional);
        let ctx = Context::new();

        let result = handler.execute(&node, &ctx).await.unwrap();
        assert!(result.is_failure());
        match result {
            Outcome::Failure { error, .. } => {
                assert!(error.contains("no condition attribute"));
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn conditional_handler_invalid_condition() {
        let handler = ConditionalHandler;
        let mut node = make_node("c4", NodeType::Conditional);
        node.attrs.insert(
            "condition".to_string(),
            NodeAttrValue::String("".to_string()),
        );

        let ctx = Context::new();

        let result = handler.execute(&node, &ctx).await.unwrap();
        assert!(result.is_failure());
    }

    #[tokio::test]
    async fn conditional_handler_handles_only_conditional_nodes() {
        let handler = ConditionalHandler;
        assert!(handler.handles(&NodeType::Conditional));
        assert!(!handler.handles(&NodeType::Start));
        assert!(!handler.handles(&NodeType::Tool));
    }

    // ---------------------------------------------------------------
    // CodergenHandler tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn codergen_handler_delegates_to_backend() {
        let backend = Arc::new(TestCodergenBackend);
        let handler = CodergenHandler::new(backend);

        let mut node = make_node("cg1", NodeType::Codergen);
        node.attrs.insert(
            "prompt".to_string(),
            NodeAttrValue::String("write hello world".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data, json!({"generated": "write hello world"}));
            }
            other => panic!("expected success with generated data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn codergen_handler_falls_back_to_label() {
        let backend = Arc::new(TestCodergenBackend);
        let handler = CodergenHandler::new(backend);

        let node = make_node_with_label("cg2", NodeType::Codergen, "label prompt");

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data, json!({"generated": "label prompt"}));
            }
            other => panic!("expected success with generated data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn codergen_handler_no_prompt_or_label_returns_failure() {
        let backend = Arc::new(TestCodergenBackend);
        let handler = CodergenHandler::new(backend);

        let node = make_node("cg3", NodeType::Codergen);

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        assert!(result.is_failure());
        match result {
            Outcome::Failure { error, .. } => {
                assert!(error.contains("no prompt specified"));
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn codergen_handler_passes_model_to_backend() {
        let backend = Arc::new(ModelCapturingBackend);
        let handler = CodergenHandler::new(backend);

        let mut node = make_node("cg4", NodeType::Codergen);
        node.attrs.insert(
            "prompt".to_string(),
            NodeAttrValue::String("do something".to_string()),
        );
        node.attrs.insert(
            "model".to_string(),
            NodeAttrValue::String("gpt-4".to_string()),
        );

        let ctx = Context::new();
        let result = handler.execute(&node, &ctx).await.unwrap();

        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data["prompt"], "do something");
                assert_eq!(data["model"], "gpt-4");
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn codergen_handler_handles_only_codergen_nodes() {
        let backend = Arc::new(TestCodergenBackend);
        let handler = CodergenHandler::new(backend);
        assert!(handler.handles(&NodeType::Codergen));
        assert!(!handler.handles(&NodeType::Start));
        assert!(!handler.handles(&NodeType::Exit));
        assert!(!handler.handles(&NodeType::Conditional));
    }

    // ---------------------------------------------------------------
    // HandlerRegistry tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn registry_finds_correct_handler() {
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(StartHandler));
        registry.register(Arc::new(ExitHandler));

        let start = registry.get_handler(&NodeType::Start);
        assert!(start.is_some());
        assert_eq!(start.unwrap().name(), "start");

        let exit = registry.get_handler(&NodeType::Exit);
        assert!(exit.is_some());
        assert_eq!(exit.unwrap().name(), "exit");
    }

    #[tokio::test]
    async fn registry_returns_none_when_no_handler() {
        let registry = HandlerRegistry::new();
        assert!(registry.get_handler(&NodeType::Tool).is_none());
    }

    #[tokio::test]
    async fn registry_execute_returns_no_handler_error() {
        let registry = HandlerRegistry::new();
        let node = make_node("t1", NodeType::Tool);
        let ctx = Context::new();

        let err = registry.execute(&node, &ctx).await.unwrap_err();
        match err {
            HandlerError::NoHandler { node_type } => {
                assert!(node_type.contains("Tool"));
            }
            other => panic!("expected NoHandler, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn registry_execute_delegates_correctly() {
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(StartHandler));
        registry.register(Arc::new(GenericTestHandler));

        let node = make_node("g1", NodeType::Generic);
        let ctx = Context::new();

        let result = registry.execute(&node, &ctx).await.unwrap();
        match result {
            Outcome::Success { data: Some(data) } => {
                assert_eq!(data, json!({"handler": "generic_test"}));
            }
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn registry_first_matching_handler_wins() {
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(StartHandler));
        registry.register(Arc::new(AlternateStartHandler));

        // The first registered StartHandler should be picked.
        let handler = registry.get_handler(&NodeType::Start).unwrap();
        assert_eq!(handler.name(), "start");

        // Verify execution uses the first one.
        let node = make_node("s1", NodeType::Start);
        let ctx = Context::new();
        registry.execute(&node, &ctx).await.unwrap();
        assert_eq!(ctx.get_string("_started"), Some("true".to_string()));
        assert_eq!(ctx.get("_alternate"), None);
    }

    // ---------------------------------------------------------------
    // default_registry tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn default_registry_has_start_exit_conditional() {
        let registry = default_registry();

        assert!(registry.get_handler(&NodeType::Start).is_some());
        assert!(registry.get_handler(&NodeType::Exit).is_some());
        assert!(registry.get_handler(&NodeType::Conditional).is_some());

        // Codergen should NOT be registered by default.
        assert!(registry.get_handler(&NodeType::Codergen).is_none());
    }

    #[tokio::test]
    async fn default_registry_executes_start_node() {
        let registry = default_registry();
        let node = make_node("s", NodeType::Start);
        let ctx = Context::new();

        let result = registry.execute(&node, &ctx).await.unwrap();
        assert!(result.is_success());
        assert_eq!(ctx.get_string("_started"), Some("true".to_string()));
    }

    #[tokio::test]
    async fn default_registry_executes_exit_node() {
        let registry = default_registry();
        let node = make_node("e", NodeType::Exit);
        let ctx = Context::new();

        let result = registry.execute(&node, &ctx).await.unwrap();
        assert!(result.is_success());
        assert_eq!(ctx.get_string("_completed"), Some("true".to_string()));
    }

    // ---------------------------------------------------------------
    // Handler trait name() correctness
    // ---------------------------------------------------------------

    #[test]
    fn handler_names_are_correct() {
        assert_eq!(StartHandler.name(), "start");
        assert_eq!(ExitHandler.name(), "exit");
        assert_eq!(ConditionalHandler.name(), "conditional");

        let backend = Arc::new(TestCodergenBackend);
        let cg = CodergenHandler::new(backend);
        assert_eq!(cg.name(), "codergen");
    }

    // ---------------------------------------------------------------
    // HandlerError formatting
    // ---------------------------------------------------------------

    #[test]
    fn handler_error_display_execution_failed() {
        let err = HandlerError::ExecutionFailed {
            handler: "start".to_string(),
            node_id: "n1".to_string(),
            message: "boom".to_string(),
        };
        assert_eq!(err.to_string(), "handler 'start' failed on node 'n1': boom");
    }

    #[test]
    fn handler_error_display_no_handler() {
        let err = HandlerError::NoHandler {
            node_type: "Tool".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "no handler registered for node type 'Tool'"
        );
    }

    #[test]
    fn handler_error_display_other() {
        let err = HandlerError::Other("something else".to_string());
        assert_eq!(err.to_string(), "handler error: something else");
    }
}
