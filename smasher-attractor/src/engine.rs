// ABOUTME: Core pipeline execution engine that traverses the graph.
// ABOUTME: Manages node execution, edge selection, checkpointing, and resume.

use std::collections::HashMap;

use crate::edge::{select_edge, EdgeSelectionError};
use crate::goals::{GoalError, GoalGate};
use crate::graph::{Graph, NodeType};
use crate::handler::{HandlerError, HandlerRegistry};
use crate::retry::{compute_delay, RetryPolicy, RetryState};
use crate::state::{Checkpoint, Context, Outcome};

/// Configuration for the pipeline execution engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Maximum nodes to visit before forced stop (prevents infinite loops).
    pub max_steps: usize,
    /// Whether to create checkpoints during execution.
    pub enable_checkpointing: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_steps: 1000,
            enable_checkpointing: true,
        }
    }
}

/// Errors that can occur during pipeline execution.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("no start node found in graph")]
    NoStartNode,
    #[error("multiple start nodes found: {ids:?}")]
    MultipleStartNodes { ids: Vec<String> },
    #[error("node '{node_id}' not found in graph")]
    NodeNotFound { node_id: String },
    #[error("max steps ({max_steps}) exceeded")]
    MaxStepsExceeded { max_steps: usize },
    #[error("handler error: {0}")]
    Handler(#[from] HandlerError),
    #[error("edge selection error: {0}")]
    EdgeSelection(#[from] EdgeSelectionError),
    #[error("goal enforcement failed: {0}")]
    GoalEnforcement(#[from] GoalError),
    #[error("retry exhausted for node '{node_id}': {message}")]
    RetryExhausted { node_id: String, message: String },
}

/// The result of a completed pipeline execution.
#[derive(Debug)]
pub struct ExecutionResult {
    /// Ordered list of node IDs that were visited during execution.
    pub visited_nodes: Vec<String>,
    /// Mapping from node ID to the outcome of executing that node.
    pub node_outcomes: HashMap<String, Outcome>,
    /// Snapshot of the context at the end of execution.
    pub final_context: HashMap<String, serde_json::Value>,
    /// Total number of steps (node executions) taken.
    pub steps_taken: usize,
    /// Checkpoint captured at the end of execution (if checkpointing is enabled).
    pub checkpoint: Option<Checkpoint>,
}

/// The core pipeline execution engine.
///
/// Traverses a graph by executing handlers for each node, selecting edges
/// to determine the next node, and enforcing goal gates at completion.
pub struct Engine {
    graph: Graph,
    registry: HandlerRegistry,
    config: EngineConfig,
    goal_gate: GoalGate,
}

impl Engine {
    /// Create an engine with default configuration.
    pub fn new(graph: Graph, registry: HandlerRegistry) -> Self {
        Self::with_config(graph, registry, EngineConfig::default())
    }

    /// Create an engine with custom configuration.
    pub fn with_config(graph: Graph, registry: HandlerRegistry, config: EngineConfig) -> Self {
        let goal_gate = GoalGate::from_graph(&graph);
        Self {
            graph,
            registry,
            config,
            goal_gate,
        }
    }

    /// Run the pipeline from the start node.
    ///
    /// Finds the single start node, then executes nodes in sequence
    /// following edge selections until an exit node is reached, no edges
    /// remain, or the max step limit is hit.
    pub async fn run(&self, context: Context) -> Result<ExecutionResult, EngineError> {
        let start_nodes = self.graph.start_nodes();
        match start_nodes.len() {
            0 => return Err(EngineError::NoStartNode),
            1 => {}
            _ => {
                let ids: Vec<String> = start_nodes.iter().map(|n| n.id.clone()).collect();
                return Err(EngineError::MultipleStartNodes { ids });
            }
        }

        let start_id = start_nodes[0].id.clone();
        let visited_nodes = Vec::new();
        let node_outcomes = HashMap::new();

        self.execute_loop(start_id, visited_nodes, node_outcomes, context)
            .await
    }

    /// Resume pipeline execution from a saved checkpoint.
    ///
    /// Restores the visited nodes and outcomes from the checkpoint, then
    /// continues execution from the checkpoint's current node.
    pub async fn run_from_checkpoint(
        &self,
        checkpoint: Checkpoint,
        context: Context,
    ) -> Result<ExecutionResult, EngineError> {
        let current_node = checkpoint.current_node.clone();
        let visited_nodes = checkpoint.visited_nodes.clone();
        let node_outcomes = checkpoint.node_outcomes.clone();

        // Restore context from checkpoint snapshot
        for (key, value) in &checkpoint.context_snapshot {
            context.set(key.clone(), value.clone());
        }

        self.execute_loop(current_node, visited_nodes, node_outcomes, context)
            .await
    }

    /// The core execution loop shared by `run` and `run_from_checkpoint`.
    async fn execute_loop(
        &self,
        start_node_id: String,
        mut visited_nodes: Vec<String>,
        mut node_outcomes: HashMap<String, Outcome>,
        context: Context,
    ) -> Result<ExecutionResult, EngineError> {
        let mut current_node_id = start_node_id;
        let mut steps: usize = 0;

        loop {
            // Check max steps limit
            if steps >= self.config.max_steps {
                return Err(EngineError::MaxStepsExceeded {
                    max_steps: self.config.max_steps,
                });
            }

            // Look up the current node
            let node = self
                .graph
                .node(&current_node_id)
                .ok_or_else(|| EngineError::NodeNotFound {
                    node_id: current_node_id.clone(),
                })?;

            // Execute the handler for this node
            let mut outcome = self.registry.execute(node, &context).await?;

            // Handle retries for retryable failures
            if outcome.is_retryable() {
                let policy = RetryPolicy::from_node(node);
                let mut retry_state = RetryState::new();
                retry_state.record_attempt(&outcome);

                while retry_state.should_retry(&policy, &outcome) {
                    let delay = compute_delay(&policy, retry_state.attempts);
                    tokio::time::sleep(delay).await;

                    outcome = self.registry.execute(node, &context).await?;
                    retry_state.record_attempt(&outcome);
                }

                // If still a failure after all retries, record as failed
                // and continue to edge selection (the outcome might route to an error edge)
            }

            steps += 1;

            // Record outcome and mark visited
            node_outcomes.insert(current_node_id.clone(), outcome.clone());
            if !visited_nodes.contains(&current_node_id) {
                visited_nodes.push(current_node_id.clone());
            }

            // If exit node, break the loop
            if node.node_type == NodeType::Exit {
                break;
            }

            // Select next edge
            let last_outcome = node_outcomes.get(&current_node_id);
            let next_edge = select_edge(&self.graph, &current_node_id, &context, last_outcome)?;

            match next_edge {
                Some(edge) => {
                    current_node_id = edge.to.clone();
                }
                None => {
                    // No outgoing edge, end execution
                    break;
                }
            }
        }

        // Enforce goal gates
        self.goal_gate.enforce(&visited_nodes)?;

        // Build checkpoint if enabled
        let checkpoint = if self.config.enable_checkpointing {
            let pipeline_name = self
                .graph
                .name
                .clone()
                .unwrap_or_else(|| "unnamed".to_string());
            let last_node = visited_nodes.last().cloned().unwrap_or_default();
            let mut cp = Checkpoint::new(pipeline_name, last_node, &context);
            for id in &visited_nodes {
                cp.mark_visited(id);
            }
            for (id, outcome) in &node_outcomes {
                cp.add_outcome(id, outcome.clone());
            }
            Some(cp)
        } else {
            None
        };

        Ok(ExecutionResult {
            visited_nodes,
            node_outcomes,
            final_context: context.snapshot(),
            steps_taken: steps,
            checkpoint,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, GraphEdge, GraphNode, NodeAttrValue, NodeType};
    use crate::handler::{Handler, HandlerError, HandlerRegistry};
    use crate::state::{Context, Outcome};
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;

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

    fn make_node_with_attrs(
        id: &str,
        node_type: NodeType,
        attrs: HashMap<String, NodeAttrValue>,
    ) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
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
            attrs: HashMap::new(),
        }
    }

    fn make_conditional_edge(from: &str, to: &str, condition: &str) -> GraphEdge {
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: None,
            condition: Some(condition.to_string()),
            priority: None,
            attrs: HashMap::new(),
        }
    }

    fn make_graph(nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> Graph {
        Graph {
            name: Some("test_pipeline".to_string()),
            nodes,
            edges,
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
        }
    }

    // ---------------------------------------------------------------
    // Test handlers
    // ---------------------------------------------------------------

    /// Handler that always returns success for any node type.
    struct AlwaysSuccessHandler;

    #[async_trait]
    impl Handler for AlwaysSuccessHandler {
        fn name(&self) -> &str {
            "always_success"
        }
        async fn execute(
            &self,
            _node: &GraphNode,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            Ok(Outcome::success())
        }
        fn handles(&self, _node_type: &NodeType) -> bool {
            true
        }
    }

    /// Handler that always returns a non-retryable failure.
    struct AlwaysFailHandler;

    #[async_trait]
    impl Handler for AlwaysFailHandler {
        fn name(&self) -> &str {
            "always_fail"
        }
        async fn execute(
            &self,
            _node: &GraphNode,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            Ok(Outcome::failure("handler always fails"))
        }
        fn handles(&self, _node_type: &NodeType) -> bool {
            true
        }
    }

    /// Handler that returns a HandlerError (not an Outcome failure).
    struct ErrorHandler;

    #[async_trait]
    impl Handler for ErrorHandler {
        fn name(&self) -> &str {
            "error_handler"
        }
        async fn execute(
            &self,
            node: &GraphNode,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            Err(HandlerError::ExecutionFailed {
                handler: "error_handler".to_string(),
                node_id: node.id.clone(),
                message: "catastrophic failure".to_string(),
            })
        }
        fn handles(&self, _node_type: &NodeType) -> bool {
            true
        }
    }

    /// Handler that sets a context value upon execution.
    struct ContextSettingHandler;

    #[async_trait]
    impl Handler for ContextSettingHandler {
        fn name(&self) -> &str {
            "context_setter"
        }
        async fn execute(
            &self,
            node: &GraphNode,
            context: &Context,
        ) -> Result<Outcome, HandlerError> {
            context.set(format!("visited_{}", node.id), json!(true));
            Ok(Outcome::success_with(json!({"node": node.id})))
        }
        fn handles(&self, _node_type: &NodeType) -> bool {
            true
        }
    }

    /// Handler that returns success for most types and failure for Conditional.
    struct ConditionalFailHandler;

    #[async_trait]
    impl Handler for ConditionalFailHandler {
        fn name(&self) -> &str {
            "conditional_fail"
        }
        async fn execute(
            &self,
            node: &GraphNode,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            if node.node_type == NodeType::Conditional {
                Ok(Outcome::failure("conditional node failed"))
            } else {
                Ok(Outcome::success())
            }
        }
        fn handles(&self, _node_type: &NodeType) -> bool {
            true
        }
    }

    fn success_registry() -> HandlerRegistry {
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(AlwaysSuccessHandler));
        registry
    }

    fn context_setting_registry() -> HandlerRegistry {
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(ContextSettingHandler));
        registry
    }

    // ---------------------------------------------------------------
    // Test 1: Config default values
    // ---------------------------------------------------------------
    #[test]
    fn config_default_values() {
        let config = EngineConfig::default();
        assert_eq!(config.max_steps, 1000);
        assert!(config.enable_checkpointing);
    }

    // ---------------------------------------------------------------
    // Test 2: Simple linear pipeline: Start -> A -> Exit
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn simple_linear_pipeline() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "a"), make_edge("a", "exit")],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        assert_eq!(result.visited_nodes, vec!["start", "a", "exit"]);
        assert_eq!(result.steps_taken, 3);
        assert!(result.node_outcomes.get("start").unwrap().is_success());
        assert!(result.node_outcomes.get("a").unwrap().is_success());
        assert!(result.node_outcomes.get("exit").unwrap().is_success());
    }

    // ---------------------------------------------------------------
    // Test 3: No start node returns error
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn no_start_node_returns_error() {
        let graph = make_graph(
            vec![
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("a", "exit")],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, EngineError::NoStartNode));
        assert!(err.to_string().contains("no start node"));
    }

    // ---------------------------------------------------------------
    // Test 4: Multiple start nodes returns error
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn multiple_start_nodes_returns_error() {
        let graph = make_graph(
            vec![
                make_node("start1", NodeType::Start),
                make_node("start2", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start1", "exit"), make_edge("start2", "exit")],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            EngineError::MultipleStartNodes { ids } => {
                assert_eq!(ids.len(), 2);
                assert!(ids.contains(&"start1".to_string()));
                assert!(ids.contains(&"start2".to_string()));
            }
            other => panic!("expected MultipleStartNodes, got: {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 5: Max steps exceeded returns error
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn max_steps_exceeded_returns_error() {
        // Create a cycle: start -> a -> b -> a (infinite loop)
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("b", NodeType::Generic),
            ],
            vec![
                make_edge("start", "a"),
                make_edge("a", "b"),
                make_edge("b", "a"),
            ],
        );
        let config = EngineConfig {
            max_steps: 5,
            enable_checkpointing: false,
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            EngineError::MaxStepsExceeded { max_steps } => {
                assert_eq!(max_steps, 5);
            }
            other => panic!("expected MaxStepsExceeded, got: {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 6: Exit node terminates execution
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn exit_node_terminates_execution() {
        // Exit is in the middle, with an edge that shouldn't be followed
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
                make_node("unreachable", NodeType::Generic),
            ],
            vec![
                make_edge("start", "exit"),
                make_edge("exit", "unreachable"),
            ],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        assert_eq!(result.visited_nodes, vec!["start", "exit"]);
        assert!(!result.visited_nodes.contains(&"unreachable".to_string()));
        assert_eq!(result.steps_taken, 2);
    }

    // ---------------------------------------------------------------
    // Test 7: Node not found returns error (corrupted graph)
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn node_not_found_returns_error() {
        // Edge points to a node that doesn't exist in the nodes list
        let graph = make_graph(
            vec![make_node("start", NodeType::Start)],
            vec![make_edge("start", "nonexistent")],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        // Start executes fine, then edge leads to "nonexistent" which is not found
        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::NodeNotFound { node_id } => {
                assert_eq!(node_id, "nonexistent");
            }
            other => panic!("expected NodeNotFound, got: {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 8: Handler error propagates
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn handler_error_propagates() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(ErrorHandler));
        let engine = Engine::new(graph, registry);
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, EngineError::Handler(_)));
        assert!(err.to_string().contains("catastrophic failure"));
    }

    // ---------------------------------------------------------------
    // Test 9: Edge selection with conditions
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn edge_selection_with_conditions() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("path_a", NodeType::Generic),
                make_node("path_b", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_conditional_edge("start", "path_a", "route=a"),
                make_conditional_edge("start", "path_b", "route=b"),
                make_edge("path_a", "exit"),
                make_edge("path_b", "exit"),
            ],
        );
        let engine = Engine::new(graph, success_registry());

        // Set context so route=b is chosen
        let ctx = Context::new();
        ctx.set("route", json!("b"));
        let result = engine.run(ctx).await.unwrap();

        assert!(result.visited_nodes.contains(&"path_b".to_string()));
        assert!(!result.visited_nodes.contains(&"path_a".to_string()));
    }

    // ---------------------------------------------------------------
    // Test 10: Goal gate enforcement passes
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn goal_gate_enforcement_passes() {
        let mut goal_attrs = HashMap::new();
        goal_attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));

        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node_with_attrs("critical", NodeType::Generic, goal_attrs),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "critical"),
                make_edge("critical", "exit"),
            ],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.visited_nodes.contains(&"critical".to_string()));
    }

    // ---------------------------------------------------------------
    // Test 11: Goal gate enforcement fails (unmet goals)
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn goal_gate_enforcement_fails() {
        let mut goal_attrs = HashMap::new();
        goal_attrs.insert("goal".to_string(), NodeAttrValue::Bool(true));

        // The goal node is not reachable from the execution path
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
                make_node_with_attrs("unreachable_goal", NodeType::Generic, goal_attrs),
            ],
            vec![make_edge("start", "exit")],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, EngineError::GoalEnforcement(_)));
        assert!(err.to_string().contains("unreachable_goal"));
    }

    // ---------------------------------------------------------------
    // Test 12: Run produces correct visited_nodes list
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn run_produces_correct_visited_nodes() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("step1", NodeType::Generic),
                make_node("step2", NodeType::Generic),
                make_node("step3", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "step1"),
                make_edge("step1", "step2"),
                make_edge("step2", "step3"),
                make_edge("step3", "exit"),
            ],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        assert_eq!(
            result.visited_nodes,
            vec!["start", "step1", "step2", "step3", "exit"]
        );
        assert_eq!(result.steps_taken, 5);
    }

    // ---------------------------------------------------------------
    // Test 13: Run produces correct node_outcomes
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn run_produces_correct_node_outcomes() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("worker", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "worker"), make_edge("worker", "exit")],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        assert_eq!(result.node_outcomes.len(), 3);
        for (_, outcome) in &result.node_outcomes {
            assert!(outcome.is_success());
        }
    }

    // ---------------------------------------------------------------
    // Test 14: ExecutionResult contains context snapshot
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn execution_result_contains_context_snapshot() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let engine = Engine::new(graph, context_setting_registry());
        let ctx = Context::new();
        ctx.set("initial", json!("value"));

        let result = engine.run(ctx).await.unwrap();

        // Should contain the initial value and the values set by ContextSettingHandler
        assert_eq!(
            result.final_context.get("initial"),
            Some(&json!("value"))
        );
        assert_eq!(
            result.final_context.get("visited_start"),
            Some(&json!(true))
        );
        assert_eq!(
            result.final_context.get("visited_exit"),
            Some(&json!(true))
        );
    }

    // ---------------------------------------------------------------
    // Test 15: Checkpoint is produced when enabled
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn checkpoint_produced_when_enabled() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let config = EngineConfig {
            max_steps: 1000,
            enable_checkpointing: true,
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        assert!(result.checkpoint.is_some());
        let cp = result.checkpoint.unwrap();
        assert_eq!(cp.pipeline_name, "test_pipeline");
        assert!(cp.was_visited("start"));
        assert!(cp.was_visited("exit"));
        assert!(cp.node_outcomes.get("start").unwrap().is_success());
        assert!(cp.node_outcomes.get("exit").unwrap().is_success());
    }

    // ---------------------------------------------------------------
    // Test 16: No checkpoint when disabled
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn no_checkpoint_when_disabled() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let config = EngineConfig {
            max_steps: 1000,
            enable_checkpointing: false,
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        assert!(result.checkpoint.is_none());
    }

    // ---------------------------------------------------------------
    // Test 17: Run from checkpoint resumes correctly
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn run_from_checkpoint_resumes_correctly() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("b", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "a"),
                make_edge("a", "b"),
                make_edge("b", "exit"),
            ],
        );
        let engine = Engine::new(graph, success_registry());

        // Create a checkpoint as if we already visited start and a, resuming at b
        let cp_ctx = Context::new();
        let mut checkpoint = Checkpoint::new("test_pipeline", "b", &cp_ctx);
        checkpoint.mark_visited("start");
        checkpoint.mark_visited("a");
        checkpoint.add_outcome("start", Outcome::success());
        checkpoint.add_outcome("a", Outcome::success());

        let ctx = Context::new();
        let result = engine.run_from_checkpoint(checkpoint, ctx).await.unwrap();

        // Should have all nodes (start/a from checkpoint, b/exit from resumed execution)
        assert!(result.visited_nodes.contains(&"start".to_string()));
        assert!(result.visited_nodes.contains(&"a".to_string()));
        assert!(result.visited_nodes.contains(&"b".to_string()));
        assert!(result.visited_nodes.contains(&"exit".to_string()));
        // Steps taken in this run should be 2 (b and exit)
        assert_eq!(result.steps_taken, 2);
    }

    // ---------------------------------------------------------------
    // Test 18: No edges from node terminates execution
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn no_edges_terminates_execution() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("dead_end", NodeType::Generic),
            ],
            vec![make_edge("start", "dead_end")],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        assert_eq!(result.visited_nodes, vec!["start", "dead_end"]);
        assert_eq!(result.steps_taken, 2);
    }

    // ---------------------------------------------------------------
    // Test 19: Outcome-based edge routing
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn outcome_based_edge_routing() {
        // ConditionalFailHandler returns failure for Conditional node types
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("check", NodeType::Conditional),
                make_node("success_path", NodeType::Exit),
                make_node("failure_path", NodeType::Exit),
            ],
            vec![
                make_edge("start", "check"),
                make_labeled_edge("check", "success_path", "success"),
                make_labeled_edge("check", "failure_path", "failure"),
            ],
        );
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(ConditionalFailHandler));
        let engine = Engine::new(graph, registry);
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        // The check node fails, so the failure edge should be taken
        assert!(result.visited_nodes.contains(&"failure_path".to_string()));
        assert!(!result.visited_nodes.contains(&"success_path".to_string()));
    }

    // ---------------------------------------------------------------
    // Test 20: Engine error display messages
    // ---------------------------------------------------------------
    #[test]
    fn engine_error_display_messages() {
        let err1 = EngineError::NoStartNode;
        assert_eq!(err1.to_string(), "no start node found in graph");

        let err2 = EngineError::MultipleStartNodes {
            ids: vec!["a".to_string(), "b".to_string()],
        };
        assert!(err2.to_string().contains("multiple start nodes"));

        let err3 = EngineError::NodeNotFound {
            node_id: "missing".to_string(),
        };
        assert!(err3.to_string().contains("missing"));

        let err4 = EngineError::MaxStepsExceeded { max_steps: 42 };
        assert!(err4.to_string().contains("42"));

        let err5 = EngineError::RetryExhausted {
            node_id: "retry_node".to_string(),
            message: "gave up".to_string(),
        };
        assert!(err5.to_string().contains("retry_node"));
        assert!(err5.to_string().contains("gave up"));
    }

    // ---------------------------------------------------------------
    // Test 21: Pipeline with only start and exit (minimal)
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn minimal_pipeline_start_to_exit() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        assert_eq!(result.visited_nodes, vec!["start", "exit"]);
        assert_eq!(result.steps_taken, 2);
        assert_eq!(result.node_outcomes.len(), 2);
    }

    // ---------------------------------------------------------------
    // Test 22: Handler failure is recorded as outcome (not error)
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn handler_failure_outcome_is_recorded() {
        // AlwaysFailHandler returns Outcome::failure (not HandlerError)
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(AlwaysFailHandler));
        let engine = Engine::new(graph, registry);
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        // Failure outcomes are still recorded; execution continues via edge selection
        let start_outcome = result.node_outcomes.get("start").unwrap();
        assert!(start_outcome.is_failure());
    }

    // ---------------------------------------------------------------
    // Test 23: Graph with no name uses "unnamed" in checkpoint
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn unnamed_graph_checkpoint_uses_unnamed() {
        let graph = Graph {
            name: None,
            nodes: vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            edges: vec![make_edge("start", "exit")],
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
        };
        let config = EngineConfig {
            max_steps: 1000,
            enable_checkpointing: true,
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        let cp = result.checkpoint.unwrap();
        assert_eq!(cp.pipeline_name, "unnamed");
    }

    // ---------------------------------------------------------------
    // Test 24: Multiple goals all met
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn multiple_goals_all_met() {
        let mut goal_attrs_1 = HashMap::new();
        goal_attrs_1.insert("goal".to_string(), NodeAttrValue::Bool(true));
        let mut goal_attrs_2 = HashMap::new();
        goal_attrs_2.insert("goal".to_string(), NodeAttrValue::Bool(true));

        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node_with_attrs("goal1", NodeType::Generic, goal_attrs_1),
                make_node_with_attrs("goal2", NodeType::Generic, goal_attrs_2),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "goal1"),
                make_edge("goal1", "goal2"),
                make_edge("goal2", "exit"),
            ],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_ok());
    }

    // ---------------------------------------------------------------
    // Test 25: Checkpoint from resume contains all nodes
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn checkpoint_from_resume_contains_all_nodes() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "a"), make_edge("a", "exit")],
        );
        let config = EngineConfig {
            max_steps: 1000,
            enable_checkpointing: true,
        };
        let engine = Engine::with_config(graph, success_registry(), config);

        // Resume from "a" with "start" already visited
        let cp_ctx = Context::new();
        let mut checkpoint = Checkpoint::new("test_pipeline", "a", &cp_ctx);
        checkpoint.mark_visited("start");
        checkpoint.add_outcome("start", Outcome::success());

        let ctx = Context::new();
        let result = engine.run_from_checkpoint(checkpoint, ctx).await.unwrap();

        let final_cp = result.checkpoint.unwrap();
        assert!(final_cp.was_visited("start"));
        assert!(final_cp.was_visited("a"));
        assert!(final_cp.was_visited("exit"));
    }

    // ---------------------------------------------------------------
    // Test 26: with_config uses provided config
    // ---------------------------------------------------------------
    #[test]
    fn with_config_uses_provided_config() {
        let graph = make_graph(vec![make_node("start", NodeType::Start)], vec![]);
        let config = EngineConfig {
            max_steps: 42,
            enable_checkpointing: false,
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        assert_eq!(engine.config.max_steps, 42);
        assert!(!engine.config.enable_checkpointing);
    }
}
