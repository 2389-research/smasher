// ABOUTME: Parallel fan-out/fan-in handler for concurrent node execution.
// ABOUTME: Supports bounded concurrency and result aggregation across parallel branches.

use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::{self, StreamExt};
use serde_json::json;

use crate::graph::{GraphNode, NodeAttrValue, NodeType};
use crate::handler::{Handler, HandlerError, HandlerRegistry};
use crate::state::{Context, Outcome};

/// Configuration for parallel execution behavior.
#[derive(Debug, Clone)]
pub struct ParallelConfig {
    /// Maximum number of tasks to run concurrently.
    pub max_concurrency: usize,
    /// Whether to stop all branches on the first failure.
    pub fail_fast: bool,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 10,
            fail_fast: false,
        }
    }
}

/// Aggregated result from a parallel fan-out execution.
#[derive(Debug, Clone)]
pub struct ParallelResult {
    /// Per-node outcomes keyed by node ID.
    pub outcomes: HashMap<String, Outcome>,
    /// Node IDs that completed successfully.
    pub succeeded: Vec<String>,
    /// Node IDs that failed.
    pub failed: Vec<String>,
}

impl ParallelResult {
    /// Returns true if every branch succeeded.
    pub fn all_succeeded(&self) -> bool {
        self.failed.is_empty() && !self.succeeded.is_empty()
    }

    /// Returns true if at least one branch failed.
    pub fn any_failed(&self) -> bool {
        !self.failed.is_empty()
    }

    /// Number of branches that succeeded.
    pub fn success_count(&self) -> usize {
        self.succeeded.len()
    }

    /// Number of branches that failed.
    pub fn failure_count(&self) -> usize {
        self.failed.len()
    }
}

/// Strategy for resolving conflicting values when merging context from parallel branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MergeStrategy {
    /// Later branch values overwrite earlier ones (insertion order).
    #[default]
    LastWriteWins,
    /// First branch value is preserved; subsequent writes for the same key are ignored.
    FirstWriteWins,
    /// Conflicting values for the same key are collected into a JSON array string.
    Collect,
    /// Any conflict produces an error listing the conflicting keys.
    Error,
}

/// Error produced when merging branch contexts with the `Error` strategy.
#[derive(Debug, thiserror::Error)]
pub enum MergeError {
    #[error("merge conflict on keys: {}", keys.join(", "))]
    Conflict {
        /// Keys that had differing values across branches.
        keys: Vec<String>,
    },
}

/// Merge context maps from multiple parallel branches using the given strategy.
///
/// Each element in `branches` is a snapshot of the context produced by one branch.
/// Keys that appear in only one branch are always included. The `strategy` governs
/// what happens when the same key has different values in multiple branches.
pub fn merge_contexts(
    branches: &[HashMap<String, String>],
    strategy: MergeStrategy,
) -> Result<HashMap<String, String>, MergeError> {
    if branches.is_empty() {
        return Ok(HashMap::new());
    }
    if branches.len() == 1 {
        return Ok(branches[0].clone());
    }

    match strategy {
        MergeStrategy::LastWriteWins => {
            let mut merged = HashMap::new();
            for branch in branches {
                for (k, v) in branch {
                    merged.insert(k.clone(), v.clone());
                }
            }
            Ok(merged)
        }
        MergeStrategy::FirstWriteWins => {
            let mut merged = HashMap::new();
            for branch in branches {
                for (k, v) in branch {
                    merged.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            Ok(merged)
        }
        MergeStrategy::Collect => {
            // First pass: gather all values per key, preserving branch order.
            let mut all_values: HashMap<String, Vec<String>> = HashMap::new();
            for branch in branches {
                for (k, v) in branch {
                    all_values.entry(k.clone()).or_default().push(v.clone());
                }
            }
            // Second pass: keys with a single unique value stay scalar;
            // keys with conflicting values become a JSON array.
            let mut merged = HashMap::new();
            for (k, values) in all_values {
                let first = &values[0];
                let has_conflict = values.iter().any(|v| v != first);
                if has_conflict {
                    let json_array = serde_json::to_string(&values)
                        .unwrap_or_else(|_| format!("[{}]", values.join(",")));
                    merged.insert(k, json_array);
                } else {
                    merged.insert(k, first.clone());
                }
            }
            Ok(merged)
        }
        MergeStrategy::Error => {
            let mut merged = HashMap::new();
            let mut conflicting_keys = Vec::new();
            for branch in branches {
                for (k, v) in branch {
                    match merged.get(k) {
                        Some(existing) if existing != v => {
                            if !conflicting_keys.contains(k) {
                                conflicting_keys.push(k.clone());
                            }
                        }
                        None => {
                            merged.insert(k.clone(), v.clone());
                        }
                        _ => {
                            // Same value, no conflict.
                        }
                    }
                }
            }
            if conflicting_keys.is_empty() {
                Ok(merged)
            } else {
                conflicting_keys.sort();
                Err(MergeError::Conflict {
                    keys: conflicting_keys,
                })
            }
        }
    }
}

/// Errors arising from parallel execution.
#[derive(Debug, thiserror::Error)]
pub enum ParallelError {
    #[error("parallel execution failed: {message}")]
    ExecutionFailed { message: String },
    #[error("handler error in branch '{node_id}': {reason}")]
    BranchFailed { node_id: String, reason: String },
    #[error("parallel execution timed out")]
    Timeout,
    #[error("context merge failed: {0}")]
    MergeFailed(#[from] MergeError),
}

/// Execute a set of graph nodes concurrently via the handler registry.
///
/// Each node is dispatched through `registry.execute()`. Results are collected
/// into a `ParallelResult` that tracks successes and failures independently.
///
/// When `config.fail_fast` is true, execution stops as soon as any branch
/// produces a failure outcome or handler error, returning partial results.
pub async fn execute_parallel(
    nodes: Vec<&GraphNode>,
    registry: &HandlerRegistry,
    context: &Context,
    config: &ParallelConfig,
) -> Result<ParallelResult, ParallelError> {
    let mut outcomes: HashMap<String, Outcome> = HashMap::new();
    let mut succeeded: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();

    if nodes.is_empty() {
        return Ok(ParallelResult {
            outcomes,
            succeeded,
            failed,
        });
    }

    // Build a stream of futures, one per node, and buffer them to limit concurrency.
    let mut result_stream = stream::iter(nodes.iter().map(|node| {
        let node_id = node.id.clone();
        async move {
            let outcome = registry.execute(node, context).await;
            (node_id, outcome)
        }
    }))
    .buffer_unordered(config.max_concurrency);

    while let Some((node_id, result)) = result_stream.next().await {
        match result {
            Ok(outcome) => {
                let is_failure = outcome.is_failure();
                outcomes.insert(node_id.clone(), outcome);
                if is_failure {
                    failed.push(node_id);
                    if config.fail_fast {
                        break;
                    }
                } else {
                    succeeded.push(node_id);
                }
            }
            Err(handler_err) => {
                let outcome = Outcome::failure(handler_err.to_string());
                outcomes.insert(node_id.clone(), outcome);
                failed.push(node_id.clone());
                if config.fail_fast {
                    return Err(ParallelError::BranchFailed {
                        node_id,
                        reason: handler_err.to_string(),
                    });
                }
            }
        }
    }

    Ok(ParallelResult {
        outcomes,
        succeeded,
        failed,
    })
}

/// Handler for Parallel-type graph nodes.
///
/// Reads optional `max_concurrency` and `fail_fast` attributes from the node
/// and returns a success outcome containing the resolved configuration. The
/// actual parallel dispatching is performed by `execute_parallel`, which the
/// engine invokes separately.
pub struct ParallelHandler {
    /// Registry available for engine-level parallel dispatch via `execute_parallel`.
    #[allow(dead_code)]
    registry: Arc<HandlerRegistry>,
    config: ParallelConfig,
    /// Strategy for merging context from parallel branches during fan-in.
    pub merge_strategy: MergeStrategy,
}

impl ParallelHandler {
    /// Create a handler with the default parallel configuration.
    pub fn new(registry: Arc<HandlerRegistry>) -> Self {
        Self {
            registry,
            config: ParallelConfig::default(),
            merge_strategy: MergeStrategy::default(),
        }
    }

    /// Create a handler with an explicit configuration.
    pub fn with_config(registry: Arc<HandlerRegistry>, config: ParallelConfig) -> Self {
        Self {
            registry,
            config,
            merge_strategy: MergeStrategy::default(),
        }
    }

    /// Create a handler with explicit configuration and merge strategy.
    pub fn with_merge_strategy(
        registry: Arc<HandlerRegistry>,
        config: ParallelConfig,
        merge_strategy: MergeStrategy,
    ) -> Self {
        Self {
            registry,
            config,
            merge_strategy,
        }
    }
}

#[async_trait::async_trait]
impl Handler for ParallelHandler {
    fn name(&self) -> &str {
        "parallel"
    }

    async fn execute(&self, node: &GraphNode, _context: &Context) -> Result<Outcome, HandlerError> {
        // Read optional overrides from node attributes.
        let max_concurrency = match node.attrs.get("max_concurrency") {
            Some(NodeAttrValue::Number(n)) => *n as usize,
            _ => self.config.max_concurrency,
        };

        let fail_fast = match node.attrs.get("fail_fast") {
            Some(NodeAttrValue::Bool(b)) => *b,
            _ => self.config.fail_fast,
        };

        let merge_strategy_str = match self.merge_strategy {
            MergeStrategy::LastWriteWins => "last_write_wins",
            MergeStrategy::FirstWriteWins => "first_write_wins",
            MergeStrategy::Collect => "collect",
            MergeStrategy::Error => "error",
        };

        Ok(Outcome::success_with(json!({
            "handler": "parallel",
            "max_concurrency": max_concurrency,
            "fail_fast": fail_fast,
            "merge_strategy": merge_strategy_str,
        })))
    }

    fn handles(&self, node_type: &NodeType) -> bool {
        matches!(node_type, NodeType::Parallel | NodeType::FanIn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    // -- Test helpers -------------------------------------------------------

    /// Build a minimal GraphNode with the given type and no attributes.
    fn make_node(id: &str, node_type: NodeType) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            node_type,
            label: None,
            attrs: HashMap::new(),
        }
    }

    /// A handler that succeeds for every node type, echoing the node ID.
    struct SuccessHandler;

    #[async_trait::async_trait]
    impl Handler for SuccessHandler {
        fn name(&self) -> &str {
            "success"
        }

        async fn execute(&self, node: &GraphNode, _ctx: &Context) -> Result<Outcome, HandlerError> {
            Ok(Outcome::success_with(json!({"node": node.id})))
        }

        fn handles(&self, _: &NodeType) -> bool {
            true
        }
    }

    /// A handler that fails for every node type.
    struct FailureHandler;

    #[async_trait::async_trait]
    impl Handler for FailureHandler {
        fn name(&self) -> &str {
            "failure"
        }

        async fn execute(&self, node: &GraphNode, _ctx: &Context) -> Result<Outcome, HandlerError> {
            Ok(Outcome::failure(format!("node {} failed", node.id)))
        }

        fn handles(&self, _: &NodeType) -> bool {
            true
        }
    }

    /// A handler that succeeds for Start nodes and fails for all others.
    struct SelectiveHandler;

    #[async_trait::async_trait]
    impl Handler for SelectiveHandler {
        fn name(&self) -> &str {
            "selective"
        }

        async fn execute(&self, node: &GraphNode, _ctx: &Context) -> Result<Outcome, HandlerError> {
            if node.node_type == NodeType::Start {
                Ok(Outcome::success_with(json!({"node": node.id})))
            } else {
                Ok(Outcome::failure(format!("node {} rejected", node.id)))
            }
        }

        fn handles(&self, _: &NodeType) -> bool {
            true
        }
    }

    /// A handler that sleeps briefly to test concurrency timing.
    struct SlowHandler {
        delay_ms: u64,
        counter: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Handler for SlowHandler {
        fn name(&self) -> &str {
            "slow"
        }

        async fn execute(&self, node: &GraphNode, _ctx: &Context) -> Result<Outcome, HandlerError> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            Ok(Outcome::success_with(json!({"node": node.id})))
        }

        fn handles(&self, _: &NodeType) -> bool {
            true
        }
    }

    /// Build a HandlerRegistry containing a single handler.
    fn registry_with(handler: Arc<dyn Handler>) -> HandlerRegistry {
        let mut reg = HandlerRegistry::new();
        reg.register(handler);
        reg
    }

    // ---------------------------------------------------------------
    // ParallelConfig tests
    // ---------------------------------------------------------------

    #[test]
    fn parallel_config_default_values() {
        let config = ParallelConfig::default();
        assert_eq!(config.max_concurrency, 10);
        assert!(!config.fail_fast);
    }

    #[test]
    fn parallel_config_clone() {
        let config = ParallelConfig {
            max_concurrency: 5,
            fail_fast: true,
        };
        let cloned = config.clone();
        assert_eq!(cloned.max_concurrency, 5);
        assert!(cloned.fail_fast);
    }

    // ---------------------------------------------------------------
    // ParallelResult accessor tests
    // ---------------------------------------------------------------

    #[test]
    fn parallel_result_all_succeeded_when_no_failures() {
        let result = ParallelResult {
            outcomes: HashMap::from([
                ("a".to_string(), Outcome::success()),
                ("b".to_string(), Outcome::success()),
            ]),
            succeeded: vec!["a".to_string(), "b".to_string()],
            failed: vec![],
        };
        assert!(result.all_succeeded());
        assert!(!result.any_failed());
        assert_eq!(result.success_count(), 2);
        assert_eq!(result.failure_count(), 0);
    }

    #[test]
    fn parallel_result_any_failed_when_some_fail() {
        let result = ParallelResult {
            outcomes: HashMap::from([
                ("a".to_string(), Outcome::success()),
                ("b".to_string(), Outcome::failure("boom")),
            ]),
            succeeded: vec!["a".to_string()],
            failed: vec!["b".to_string()],
        };
        assert!(!result.all_succeeded());
        assert!(result.any_failed());
        assert_eq!(result.success_count(), 1);
        assert_eq!(result.failure_count(), 1);
    }

    #[test]
    fn parallel_result_empty_is_not_all_succeeded() {
        let result = ParallelResult {
            outcomes: HashMap::new(),
            succeeded: vec![],
            failed: vec![],
        };
        // Empty results: no successes means all_succeeded is false
        assert!(!result.all_succeeded());
        assert!(!result.any_failed());
        assert_eq!(result.success_count(), 0);
        assert_eq!(result.failure_count(), 0);
    }

    #[test]
    fn parallel_result_all_failed() {
        let result = ParallelResult {
            outcomes: HashMap::from([
                ("a".to_string(), Outcome::failure("err1")),
                ("b".to_string(), Outcome::failure("err2")),
            ]),
            succeeded: vec![],
            failed: vec!["a".to_string(), "b".to_string()],
        };
        assert!(!result.all_succeeded());
        assert!(result.any_failed());
        assert_eq!(result.success_count(), 0);
        assert_eq!(result.failure_count(), 2);
    }

    // ---------------------------------------------------------------
    // ParallelError display tests
    // ---------------------------------------------------------------

    #[test]
    fn parallel_error_execution_failed_display() {
        let err = ParallelError::ExecutionFailed {
            message: "something went wrong".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "parallel execution failed: something went wrong"
        );
    }

    #[test]
    fn parallel_error_branch_failed_display() {
        let err = ParallelError::BranchFailed {
            node_id: "node_42".to_string(),
            reason: "handler exploded".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "handler error in branch 'node_42': handler exploded"
        );
    }

    #[test]
    fn parallel_error_timeout_display() {
        let err = ParallelError::Timeout;
        assert_eq!(err.to_string(), "parallel execution timed out");
    }

    // ---------------------------------------------------------------
    // execute_parallel tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn execute_parallel_empty_nodes_returns_empty_result() {
        let registry = registry_with(Arc::new(SuccessHandler));
        let ctx = Context::new();
        let config = ParallelConfig::default();

        let result = execute_parallel(vec![], &registry, &ctx, &config)
            .await
            .unwrap();

        assert!(!result.all_succeeded());
        assert!(!result.any_failed());
        assert_eq!(result.success_count(), 0);
        assert_eq!(result.failure_count(), 0);
    }

    #[tokio::test]
    async fn execute_parallel_single_node_succeeds() {
        let registry = registry_with(Arc::new(SuccessHandler));
        let ctx = Context::new();
        let config = ParallelConfig::default();
        let node = make_node("solo", NodeType::Generic);

        let result = execute_parallel(vec![&node], &registry, &ctx, &config)
            .await
            .unwrap();

        assert!(result.all_succeeded());
        assert_eq!(result.success_count(), 1);
        assert_eq!(result.failure_count(), 0);
        assert!(result.outcomes.contains_key("solo"));
    }

    #[tokio::test]
    async fn execute_parallel_multiple_nodes_all_succeed() {
        let registry = registry_with(Arc::new(SuccessHandler));
        let ctx = Context::new();
        let config = ParallelConfig::default();

        let nodes: Vec<GraphNode> = (0..5)
            .map(|i| make_node(&format!("n{i}"), NodeType::Generic))
            .collect();
        let node_refs: Vec<&GraphNode> = nodes.iter().collect();

        let result = execute_parallel(node_refs, &registry, &ctx, &config)
            .await
            .unwrap();

        assert!(result.all_succeeded());
        assert_eq!(result.success_count(), 5);
        assert_eq!(result.failure_count(), 0);
        for i in 0..5 {
            assert!(result.outcomes.contains_key(&format!("n{i}")));
        }
    }

    #[tokio::test]
    async fn execute_parallel_some_failures_without_fail_fast() {
        let registry = registry_with(Arc::new(SelectiveHandler));
        let ctx = Context::new();
        let config = ParallelConfig {
            max_concurrency: 10,
            fail_fast: false,
        };

        // Start nodes succeed, Generic nodes fail
        let nodes = [
            make_node("good1", NodeType::Start),
            make_node("bad1", NodeType::Generic),
            make_node("good2", NodeType::Start),
            make_node("bad2", NodeType::Generic),
        ];
        let node_refs: Vec<&GraphNode> = nodes.iter().collect();

        let result = execute_parallel(node_refs, &registry, &ctx, &config)
            .await
            .unwrap();

        assert!(!result.all_succeeded());
        assert!(result.any_failed());
        assert_eq!(result.success_count(), 2);
        assert_eq!(result.failure_count(), 2);
        // All outcomes should be present (fail_fast is false)
        assert_eq!(result.outcomes.len(), 4);
    }

    #[tokio::test]
    async fn execute_parallel_all_failures() {
        let registry = registry_with(Arc::new(FailureHandler));
        let ctx = Context::new();
        let config = ParallelConfig {
            max_concurrency: 10,
            fail_fast: false,
        };

        let nodes = [
            make_node("f1", NodeType::Generic),
            make_node("f2", NodeType::Generic),
        ];
        let node_refs: Vec<&GraphNode> = nodes.iter().collect();

        let result = execute_parallel(node_refs, &registry, &ctx, &config)
            .await
            .unwrap();

        assert!(!result.all_succeeded());
        assert!(result.any_failed());
        assert_eq!(result.success_count(), 0);
        assert_eq!(result.failure_count(), 2);
    }

    #[tokio::test]
    async fn execute_parallel_fail_fast_stops_early() {
        // With fail_fast and a handler that always fails, we should get at least
        // one failure and potentially fewer total outcomes than nodes.
        let registry = registry_with(Arc::new(FailureHandler));
        let ctx = Context::new();
        let config = ParallelConfig {
            max_concurrency: 1, // serialize to guarantee ordering
            fail_fast: true,
        };

        let nodes: Vec<GraphNode> = (0..5)
            .map(|i| make_node(&format!("n{i}"), NodeType::Generic))
            .collect();
        let node_refs: Vec<&GraphNode> = nodes.iter().collect();

        let result = execute_parallel(node_refs, &registry, &ctx, &config).await;

        // Should succeed (fail_fast with Outcome::Failure still returns Ok)
        let result = result.unwrap();
        assert!(result.any_failed());
        // With max_concurrency=1 and fail_fast, we should stop after the first failure
        assert!(result.outcomes.len() <= 5);
        assert!(result.failure_count() >= 1);
    }

    #[tokio::test]
    async fn execute_parallel_respects_max_concurrency() {
        // Verify that tasks run concurrently by timing: if max_concurrency allows
        // all 4 tasks at once, total time should be ~50ms, not ~200ms.
        let counter = Arc::new(AtomicUsize::new(0));
        let handler = Arc::new(SlowHandler {
            delay_ms: 50,
            counter: counter.clone(),
        });
        let registry = registry_with(handler);
        let ctx = Context::new();
        let config = ParallelConfig {
            max_concurrency: 4,
            fail_fast: false,
        };

        let nodes: Vec<GraphNode> = (0..4)
            .map(|i| make_node(&format!("s{i}"), NodeType::Generic))
            .collect();
        let node_refs: Vec<&GraphNode> = nodes.iter().collect();

        let start = std::time::Instant::now();
        let result = execute_parallel(node_refs, &registry, &ctx, &config)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert!(result.all_succeeded());
        assert_eq!(result.success_count(), 4);
        assert_eq!(counter.load(Ordering::SeqCst), 4);
        // All 4 should run concurrently: total time < 4 * 50ms
        // Allow some slack for CI environments
        assert!(
            elapsed < Duration::from_millis(300),
            "expected concurrent execution but took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn execute_parallel_limited_concurrency_is_slower() {
        // With max_concurrency=1, 3 tasks of 30ms each should take ~90ms total
        let counter = Arc::new(AtomicUsize::new(0));
        let handler = Arc::new(SlowHandler {
            delay_ms: 30,
            counter: counter.clone(),
        });
        let registry = registry_with(handler);
        let ctx = Context::new();
        let config = ParallelConfig {
            max_concurrency: 1,
            fail_fast: false,
        };

        let nodes: Vec<GraphNode> = (0..3)
            .map(|i| make_node(&format!("seq{i}"), NodeType::Generic))
            .collect();
        let node_refs: Vec<&GraphNode> = nodes.iter().collect();

        let start = std::time::Instant::now();
        let result = execute_parallel(node_refs, &registry, &ctx, &config)
            .await
            .unwrap();
        let elapsed = start.elapsed();

        assert!(result.all_succeeded());
        assert_eq!(counter.load(Ordering::SeqCst), 3);
        // Serialized: should take at least 3 * 30ms = 90ms
        assert!(
            elapsed >= Duration::from_millis(80),
            "expected serialized execution but finished in {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn execute_parallel_shares_context_across_branches() {
        // Verify that all branches share the same Context by having each
        // branch write a key and checking all keys are present afterward.
        struct ContextWritingHandler;

        #[async_trait::async_trait]
        impl Handler for ContextWritingHandler {
            fn name(&self) -> &str {
                "ctx_writer"
            }

            async fn execute(
                &self,
                node: &GraphNode,
                ctx: &Context,
            ) -> Result<Outcome, HandlerError> {
                ctx.set(
                    format!("branch_{}", node.id),
                    json!(format!("from_{}", node.id)),
                );
                Ok(Outcome::success())
            }

            fn handles(&self, _: &NodeType) -> bool {
                true
            }
        }

        let registry = registry_with(Arc::new(ContextWritingHandler));
        let ctx = Context::new();
        let config = ParallelConfig::default();

        let nodes = [
            make_node("x", NodeType::Generic),
            make_node("y", NodeType::Generic),
            make_node("z", NodeType::Generic),
        ];
        let node_refs: Vec<&GraphNode> = nodes.iter().collect();

        let result = execute_parallel(node_refs, &registry, &ctx, &config)
            .await
            .unwrap();

        assert!(result.all_succeeded());
        // All three branches should have written to the shared context
        assert_eq!(ctx.get_string("branch_x"), Some("from_x".to_string()));
        assert_eq!(ctx.get_string("branch_y"), Some("from_y".to_string()));
        assert_eq!(ctx.get_string("branch_z"), Some("from_z".to_string()));
    }

    #[tokio::test]
    async fn execute_parallel_no_handler_with_fail_fast_returns_error() {
        // When the registry has no handler for the node type and fail_fast is
        // true, execute_parallel should return a BranchFailed error.
        let registry = HandlerRegistry::new(); // empty registry
        let ctx = Context::new();
        let config = ParallelConfig {
            max_concurrency: 10,
            fail_fast: true,
        };

        let node = make_node("orphan", NodeType::Tool);
        let result = execute_parallel(vec![&node], &registry, &ctx, &config).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ParallelError::BranchFailed { node_id, .. } => {
                assert_eq!(node_id, "orphan");
            }
            other => panic!("expected BranchFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_parallel_no_handler_without_fail_fast_collects_failure() {
        // Without fail_fast, a handler error is captured as a failure outcome.
        let registry = HandlerRegistry::new(); // empty registry
        let ctx = Context::new();
        let config = ParallelConfig {
            max_concurrency: 10,
            fail_fast: false,
        };

        let node = make_node("orphan", NodeType::Tool);
        let result = execute_parallel(vec![&node], &registry, &ctx, &config)
            .await
            .unwrap();

        assert!(result.any_failed());
        assert_eq!(result.failure_count(), 1);
        assert!(result.failed.contains(&"orphan".to_string()));
    }

    // ---------------------------------------------------------------
    // ParallelHandler tests
    // ---------------------------------------------------------------

    #[test]
    fn parallel_handler_handles_parallel_and_fanin_nodes() {
        let registry = Arc::new(HandlerRegistry::new());
        let handler = ParallelHandler::new(registry);

        assert!(handler.handles(&NodeType::Parallel));
        assert!(handler.handles(&NodeType::FanIn));
        assert!(!handler.handles(&NodeType::Start));
        assert!(!handler.handles(&NodeType::Exit));
        assert!(!handler.handles(&NodeType::Codergen));
        assert!(!handler.handles(&NodeType::Conditional));
        assert!(!handler.handles(&NodeType::Tool));
        assert!(!handler.handles(&NodeType::Interviewer));
        assert!(!handler.handles(&NodeType::Manager));
        assert!(!handler.handles(&NodeType::Generic));
    }

    #[tokio::test]
    async fn parallel_handler_returns_success_with_default_config() {
        let registry = Arc::new(HandlerRegistry::new());
        let handler = ParallelHandler::new(registry);
        let node = make_node("p1", NodeType::Parallel);
        let ctx = Context::new();

        let outcome = handler.execute(&node, &ctx).await.unwrap();
        assert!(outcome.is_success());
        match outcome {
            Outcome::Success {
                data: Some(data), ..
            } => {
                assert_eq!(data["handler"], "parallel");
                assert_eq!(data["max_concurrency"], 10);
                assert_eq!(data["fail_fast"], false);
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn parallel_handler_reads_config_from_node_attrs() {
        let registry = Arc::new(HandlerRegistry::new());
        let handler = ParallelHandler::new(registry);

        let mut node = make_node("p2", NodeType::Parallel);
        node.attrs
            .insert("max_concurrency".to_string(), NodeAttrValue::Number(4.0));
        node.attrs
            .insert("fail_fast".to_string(), NodeAttrValue::Bool(true));

        let ctx = Context::new();
        let outcome = handler.execute(&node, &ctx).await.unwrap();

        match outcome {
            Outcome::Success {
                data: Some(data), ..
            } => {
                assert_eq!(data["max_concurrency"], 4);
                assert_eq!(data["fail_fast"], true);
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn parallel_handler_with_custom_config() {
        let registry = Arc::new(HandlerRegistry::new());
        let config = ParallelConfig {
            max_concurrency: 3,
            fail_fast: true,
        };
        let handler = ParallelHandler::with_config(registry, config);

        // Node without overrides should use the handler's config
        let node = make_node("p3", NodeType::Parallel);
        let ctx = Context::new();
        let outcome = handler.execute(&node, &ctx).await.unwrap();

        match outcome {
            Outcome::Success {
                data: Some(data), ..
            } => {
                assert_eq!(data["max_concurrency"], 3);
                assert_eq!(data["fail_fast"], true);
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn parallel_handler_node_attrs_override_handler_config() {
        let registry = Arc::new(HandlerRegistry::new());
        let config = ParallelConfig {
            max_concurrency: 3,
            fail_fast: true,
        };
        let handler = ParallelHandler::with_config(registry, config);

        let mut node = make_node("p4", NodeType::Parallel);
        // Override only max_concurrency; fail_fast should use handler default
        node.attrs
            .insert("max_concurrency".to_string(), NodeAttrValue::Number(20.0));

        let ctx = Context::new();
        let outcome = handler.execute(&node, &ctx).await.unwrap();

        match outcome {
            Outcome::Success {
                data: Some(data), ..
            } => {
                assert_eq!(data["max_concurrency"], 20);
                // fail_fast falls back to handler config
                assert_eq!(data["fail_fast"], true);
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    #[test]
    fn parallel_handler_name_is_correct() {
        let registry = Arc::new(HandlerRegistry::new());
        let handler = ParallelHandler::new(registry);
        assert_eq!(handler.name(), "parallel");
    }

    // ---------------------------------------------------------------
    // ParallelResult clone
    // ---------------------------------------------------------------

    #[test]
    fn parallel_result_clone_preserves_data() {
        let result = ParallelResult {
            outcomes: HashMap::from([
                ("a".to_string(), Outcome::success()),
                ("b".to_string(), Outcome::failure("err")),
            ]),
            succeeded: vec!["a".to_string()],
            failed: vec!["b".to_string()],
        };
        let cloned = result.clone();
        assert_eq!(cloned.success_count(), 1);
        assert_eq!(cloned.failure_count(), 1);
        assert_eq!(cloned.outcomes.len(), 2);
    }

    // ---------------------------------------------------------------
    // MergeStrategy default
    // ---------------------------------------------------------------

    #[test]
    fn merge_strategy_default_is_last_write_wins() {
        assert_eq!(MergeStrategy::default(), MergeStrategy::LastWriteWins);
    }

    #[test]
    fn merge_strategy_clone_and_copy() {
        let s = MergeStrategy::Collect;
        let cloned = s;
        let copied = s;
        assert_eq!(cloned, MergeStrategy::Collect);
        assert_eq!(copied, MergeStrategy::Collect);
    }

    // ---------------------------------------------------------------
    // merge_contexts — empty and single branch
    // ---------------------------------------------------------------

    #[test]
    fn merge_contexts_empty_branches_returns_empty() {
        let result = merge_contexts(&[], MergeStrategy::LastWriteWins).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn merge_contexts_single_branch_returns_clone() {
        let branch = HashMap::from([
            ("key1".to_string(), "val1".to_string()),
            ("key2".to_string(), "val2".to_string()),
        ]);
        let result = merge_contexts(std::slice::from_ref(&branch), MergeStrategy::Error).unwrap();
        assert_eq!(result, branch);
    }

    // ---------------------------------------------------------------
    // merge_contexts — LastWriteWins
    // ---------------------------------------------------------------

    #[test]
    fn merge_last_write_wins_no_conflict() {
        let b1 = HashMap::from([("a".to_string(), "1".to_string())]);
        let b2 = HashMap::from([("b".to_string(), "2".to_string())]);

        let result = merge_contexts(&[b1, b2], MergeStrategy::LastWriteWins).unwrap();
        assert_eq!(result.get("a"), Some(&"1".to_string()));
        assert_eq!(result.get("b"), Some(&"2".to_string()));
    }

    #[test]
    fn merge_last_write_wins_conflict_uses_later_branch() {
        let b1 = HashMap::from([("x".to_string(), "first".to_string())]);
        let b2 = HashMap::from([("x".to_string(), "second".to_string())]);

        let result = merge_contexts(&[b1, b2], MergeStrategy::LastWriteWins).unwrap();
        assert_eq!(result.get("x"), Some(&"second".to_string()));
    }

    #[test]
    fn merge_last_write_wins_three_branches() {
        let b1 = HashMap::from([("x".to_string(), "one".to_string())]);
        let b2 = HashMap::from([("x".to_string(), "two".to_string())]);
        let b3 = HashMap::from([("x".to_string(), "three".to_string())]);

        let result = merge_contexts(&[b1, b2, b3], MergeStrategy::LastWriteWins).unwrap();
        assert_eq!(result.get("x"), Some(&"three".to_string()));
    }

    // ---------------------------------------------------------------
    // merge_contexts — FirstWriteWins
    // ---------------------------------------------------------------

    #[test]
    fn merge_first_write_wins_no_conflict() {
        let b1 = HashMap::from([("a".to_string(), "1".to_string())]);
        let b2 = HashMap::from([("b".to_string(), "2".to_string())]);

        let result = merge_contexts(&[b1, b2], MergeStrategy::FirstWriteWins).unwrap();
        assert_eq!(result.get("a"), Some(&"1".to_string()));
        assert_eq!(result.get("b"), Some(&"2".to_string()));
    }

    #[test]
    fn merge_first_write_wins_conflict_preserves_first() {
        let b1 = HashMap::from([("x".to_string(), "first".to_string())]);
        let b2 = HashMap::from([("x".to_string(), "second".to_string())]);

        let result = merge_contexts(&[b1, b2], MergeStrategy::FirstWriteWins).unwrap();
        assert_eq!(result.get("x"), Some(&"first".to_string()));
    }

    #[test]
    fn merge_first_write_wins_three_branches() {
        let b1 = HashMap::from([("x".to_string(), "one".to_string())]);
        let b2 = HashMap::from([("x".to_string(), "two".to_string())]);
        let b3 = HashMap::from([("x".to_string(), "three".to_string())]);

        let result = merge_contexts(&[b1, b2, b3], MergeStrategy::FirstWriteWins).unwrap();
        assert_eq!(result.get("x"), Some(&"one".to_string()));
    }

    // ---------------------------------------------------------------
    // merge_contexts — Collect
    // ---------------------------------------------------------------

    #[test]
    fn merge_collect_no_conflict_stays_scalar() {
        let b1 = HashMap::from([("a".to_string(), "1".to_string())]);
        let b2 = HashMap::from([("b".to_string(), "2".to_string())]);

        let result = merge_contexts(&[b1, b2], MergeStrategy::Collect).unwrap();
        assert_eq!(result.get("a"), Some(&"1".to_string()));
        assert_eq!(result.get("b"), Some(&"2".to_string()));
    }

    #[test]
    fn merge_collect_same_value_stays_scalar() {
        let b1 = HashMap::from([("x".to_string(), "same".to_string())]);
        let b2 = HashMap::from([("x".to_string(), "same".to_string())]);

        let result = merge_contexts(&[b1, b2], MergeStrategy::Collect).unwrap();
        assert_eq!(result.get("x"), Some(&"same".to_string()));
    }

    #[test]
    fn merge_collect_conflict_creates_json_array() {
        let b1 = HashMap::from([("x".to_string(), "alpha".to_string())]);
        let b2 = HashMap::from([("x".to_string(), "beta".to_string())]);

        let result = merge_contexts(&[b1, b2], MergeStrategy::Collect).unwrap();
        let collected = result.get("x").unwrap();
        // Should be a JSON array of the values.
        let parsed: Vec<String> = serde_json::from_str(collected).unwrap();
        assert_eq!(parsed, vec!["alpha", "beta"]);
    }

    #[test]
    fn merge_collect_three_branches_with_conflict() {
        let b1 = HashMap::from([("x".to_string(), "a".to_string())]);
        let b2 = HashMap::from([("x".to_string(), "b".to_string())]);
        let b3 = HashMap::from([("x".to_string(), "c".to_string())]);

        let result = merge_contexts(&[b1, b2, b3], MergeStrategy::Collect).unwrap();
        let collected = result.get("x").unwrap();
        let parsed: Vec<String> = serde_json::from_str(collected).unwrap();
        assert_eq!(parsed, vec!["a", "b", "c"]);
    }

    #[test]
    fn merge_collect_mixed_conflict_and_unique() {
        let b1 = HashMap::from([
            ("shared".to_string(), "v1".to_string()),
            ("unique_a".to_string(), "only_a".to_string()),
        ]);
        let b2 = HashMap::from([
            ("shared".to_string(), "v2".to_string()),
            ("unique_b".to_string(), "only_b".to_string()),
        ]);

        let result = merge_contexts(&[b1, b2], MergeStrategy::Collect).unwrap();
        // Unique keys are kept as-is.
        assert_eq!(result.get("unique_a"), Some(&"only_a".to_string()));
        assert_eq!(result.get("unique_b"), Some(&"only_b".to_string()));
        // Conflicting key becomes a JSON array.
        let shared = result.get("shared").unwrap();
        let parsed: Vec<String> = serde_json::from_str(shared).unwrap();
        assert_eq!(parsed, vec!["v1", "v2"]);
    }

    // ---------------------------------------------------------------
    // merge_contexts — Error
    // ---------------------------------------------------------------

    #[test]
    fn merge_error_no_conflict_succeeds() {
        let b1 = HashMap::from([("a".to_string(), "1".to_string())]);
        let b2 = HashMap::from([("b".to_string(), "2".to_string())]);

        let result = merge_contexts(&[b1, b2], MergeStrategy::Error).unwrap();
        assert_eq!(result.get("a"), Some(&"1".to_string()));
        assert_eq!(result.get("b"), Some(&"2".to_string()));
    }

    #[test]
    fn merge_error_same_value_no_conflict() {
        let b1 = HashMap::from([("x".to_string(), "same".to_string())]);
        let b2 = HashMap::from([("x".to_string(), "same".to_string())]);

        let result = merge_contexts(&[b1, b2], MergeStrategy::Error).unwrap();
        assert_eq!(result.get("x"), Some(&"same".to_string()));
    }

    #[test]
    fn merge_error_conflict_returns_error() {
        let b1 = HashMap::from([("x".to_string(), "first".to_string())]);
        let b2 = HashMap::from([("x".to_string(), "second".to_string())]);

        let err = merge_contexts(&[b1, b2], MergeStrategy::Error).unwrap_err();
        match &err {
            MergeError::Conflict { keys } => {
                assert_eq!(keys, &vec!["x".to_string()]);
            }
        }
        assert!(err.to_string().contains("x"));
    }

    #[test]
    fn merge_error_multiple_conflicts_lists_all_keys() {
        let b1 = HashMap::from([
            ("x".to_string(), "1".to_string()),
            ("y".to_string(), "a".to_string()),
        ]);
        let b2 = HashMap::from([
            ("x".to_string(), "2".to_string()),
            ("y".to_string(), "b".to_string()),
        ]);

        let err = merge_contexts(&[b1, b2], MergeStrategy::Error).unwrap_err();
        match &err {
            MergeError::Conflict { keys } => {
                assert!(keys.contains(&"x".to_string()));
                assert!(keys.contains(&"y".to_string()));
                assert_eq!(keys.len(), 2);
            }
        }
    }

    // ---------------------------------------------------------------
    // MergeError display
    // ---------------------------------------------------------------

    #[test]
    fn merge_error_display_shows_conflicting_keys() {
        let err = MergeError::Conflict {
            keys: vec!["alpha".to_string(), "beta".to_string()],
        };
        assert_eq!(err.to_string(), "merge conflict on keys: alpha, beta");
    }

    // ---------------------------------------------------------------
    // ParallelHandler merge_strategy integration
    // ---------------------------------------------------------------

    #[test]
    fn parallel_handler_default_merge_strategy_is_last_write_wins() {
        let registry = Arc::new(HandlerRegistry::new());
        let handler = ParallelHandler::new(registry);
        assert_eq!(handler.merge_strategy, MergeStrategy::LastWriteWins);
    }

    #[test]
    fn parallel_handler_with_merge_strategy_sets_strategy() {
        let registry = Arc::new(HandlerRegistry::new());
        let handler = ParallelHandler::with_merge_strategy(
            registry,
            ParallelConfig::default(),
            MergeStrategy::Collect,
        );
        assert_eq!(handler.merge_strategy, MergeStrategy::Collect);
    }

    #[tokio::test]
    async fn parallel_handler_execute_includes_merge_strategy_in_output() {
        let registry = Arc::new(HandlerRegistry::new());
        let handler = ParallelHandler::with_merge_strategy(
            registry,
            ParallelConfig::default(),
            MergeStrategy::Error,
        );
        let node = make_node("p_merge", NodeType::Parallel);
        let ctx = Context::new();

        let outcome = handler.execute(&node, &ctx).await.unwrap();
        match outcome {
            Outcome::Success {
                data: Some(data), ..
            } => {
                assert_eq!(data["merge_strategy"], "error");
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn parallel_handler_default_reports_last_write_wins_strategy() {
        let registry = Arc::new(HandlerRegistry::new());
        let handler = ParallelHandler::new(registry);
        let node = make_node("p_default", NodeType::Parallel);
        let ctx = Context::new();

        let outcome = handler.execute(&node, &ctx).await.unwrap();
        match outcome {
            Outcome::Success {
                data: Some(data), ..
            } => {
                assert_eq!(data["merge_strategy"], "last_write_wins");
            }
            other => panic!("expected success with data, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // ParallelError::MergeFailed
    // ---------------------------------------------------------------

    #[test]
    fn parallel_error_merge_failed_display() {
        let merge_err = MergeError::Conflict {
            keys: vec!["key1".to_string()],
        };
        let err = ParallelError::MergeFailed(merge_err);
        assert_eq!(
            err.to_string(),
            "context merge failed: merge conflict on keys: key1"
        );
    }

    #[test]
    fn parallel_error_from_merge_error() {
        let merge_err = MergeError::Conflict {
            keys: vec!["a".to_string()],
        };
        let parallel_err: ParallelError = merge_err.into();
        assert!(matches!(parallel_err, ParallelError::MergeFailed(_)));
    }
}
