// ABOUTME: Core pipeline execution engine that traverses the graph.
// ABOUTME: Manages node execution, edge selection, checkpointing, and resume.

//! Pipeline execution engine.
//!
//! The engine is the core runtime for smasher-attractor pipelines. It takes a
//! [`Graph`] (a resolved DOT file) together with a [`HandlerRegistry`] that maps
//! node types to executable logic, and walks the graph from the single start
//! node to an exit node (or until the step limit is reached).
//!
//! Key responsibilities:
//!
//! - **Node execution** -- delegates each visited node to the matching handler.
//! - **Edge selection** -- picks the next edge based on conditions, outcomes,
//!   and priorities.
//! - **Retry** -- re-executes nodes that return retryable failures according to
//!   per-node [`RetryPolicy`] attributes.
//! - **Loop tracking** -- counts `loop_restart` edge traversals and clears
//!   source-node context entries on restart.
//! - **Goal enforcement** -- after execution completes, verifies that all goal
//!   nodes were visited.
//! - **Checkpointing** -- optionally snapshots the full execution state so a
//!   pipeline can be resumed later with [`Engine::run_from_checkpoint`].

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::artifact::ArtifactStore;
use crate::composition::SubPipelineTransform;
use crate::edge::{EdgeSelectionError, select_edge};
use crate::events::{PipelineEvent, PipelineEventEmitter};
use crate::fidelity::{FidelityConfig, FidelityProcessor};
use crate::goals::{GoalError, GoalGate};
use crate::graph::{Graph, GraphNode, NodeAttrValue, NodeType};
use crate::handler::{HandlerError, HandlerRegistry};
use crate::retry::{RetryPolicy, RetryState, compute_delay};
use crate::state::{Checkpoint, Context, Outcome};
use crate::stats::{NodeStats, OutcomeKind, PipelineStats};

/// Configuration for the pipeline execution engine.
///
/// # Examples
///
/// ```
/// use smasher_attractor::engine::EngineConfig;
///
/// // Use the defaults (1000 max steps, checkpointing enabled).
/// let config = EngineConfig::default();
/// assert_eq!(config.max_steps, 1000);
/// assert!(config.enable_checkpointing);
///
/// // Customize for a tight test loop.
/// let config = EngineConfig {
///     max_steps: 50,
///     enable_checkpointing: false,
///     ..EngineConfig::default()
/// };
/// assert_eq!(config.max_steps, 50);
/// ```
#[derive(Clone)]
pub struct EngineConfig {
    /// Maximum nodes to visit before forced stop (prevents infinite loops).
    pub max_steps: usize,
    /// Whether to create checkpoints during execution.
    pub enable_checkpointing: bool,
    /// Optional cancellation token checked before each node execution.
    pub cancellation_token: Option<CancellationToken>,
    /// Directory to write checkpoint files into after each node. Requires
    /// `enable_checkpointing` to be `true` for auto-save to take effect.
    pub checkpoint_dir: Option<PathBuf>,
    /// Maximum time to wait without forward progress before aborting. When set,
    /// a background watchdog task monitors the engine and cancels execution if
    /// no node starts or completes within this duration.
    pub stall_timeout: Option<Duration>,
    /// How often the stall watchdog checks for progress. Defaults to 5 seconds
    /// when `stall_timeout` is set and this field is `None`.
    pub stall_check_interval: Option<Duration>,
    /// Maximum number of identical (same node + same error) failures before
    /// the engine aborts with `EngineError::DeterministicFailureCycle`.
    /// `None` disables the check. Default: `Some(3)`.
    pub max_identical_failures: Option<u32>,
    /// Optional fidelity configuration controlling context carryover between nodes.
    /// When set, the engine applies fidelity transformations after each edge traversal.
    pub fidelity_config: Option<FidelityConfig>,
    /// Optional artifact store for capturing node outputs during execution.
    /// When set, node outcomes are stored as queryable artifacts.
    pub artifact_store: Option<ArtifactStore>,
}

impl std::fmt::Debug for EngineConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineConfig")
            .field("max_steps", &self.max_steps)
            .field("enable_checkpointing", &self.enable_checkpointing)
            .field("cancellation_token", &self.cancellation_token)
            .field("checkpoint_dir", &self.checkpoint_dir)
            .field("stall_timeout", &self.stall_timeout)
            .field("stall_check_interval", &self.stall_check_interval)
            .field("max_identical_failures", &self.max_identical_failures)
            .field("fidelity_config", &self.fidelity_config)
            .field(
                "artifact_store",
                &self
                    .artifact_store
                    .as_ref()
                    .map(|s| format!("ArtifactStore({} items)", s.count())),
            )
            .finish()
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_steps: 1000,
            enable_checkpointing: true,
            cancellation_token: None,
            checkpoint_dir: None,
            stall_timeout: None,
            stall_check_interval: None,
            max_identical_failures: Some(3),
            fidelity_config: None,
            artifact_store: None,
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
    #[error("pipeline cancelled")]
    Cancelled,
    #[error("pipeline stalled: no progress for {timeout_secs}s")]
    Stalled { timeout_secs: u64 },
    #[error("deterministic failure cycle: node '{node_id}' failed {count} times with: {error}")]
    DeterministicFailureCycle {
        node_id: String,
        error: String,
        count: u32,
    },
    #[error("node '{node_id}' failed with no available route: {error}")]
    UnroutableFailure { node_id: String, error: String },
    #[error("composition error: {0}")]
    Composition(#[from] crate::composition::CompositionError),
}

/// Tracks how many times each loop_restart edge has been traversed.
///
/// Each edge is identified by its (from, to) pair. The counter is useful
/// for debugging loops and is included in the execution result.
#[derive(Debug, Clone, Default)]
pub struct LoopCounter {
    counts: HashMap<(String, String), usize>,
}

impl LoopCounter {
    /// Create a new empty loop counter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment the traversal count for a loop_restart edge.
    pub fn increment(&mut self, from: &str, to: &str) {
        let key = (from.to_string(), to.to_string());
        *self.counts.entry(key).or_insert(0) += 1;
    }

    /// Get the traversal count for a specific edge.
    pub fn count(&self, from: &str, to: &str) -> usize {
        let key = (from.to_string(), to.to_string());
        self.counts.get(&key).copied().unwrap_or(0)
    }

    /// Get all loop restart edge counts.
    pub fn counts(&self) -> &HashMap<(String, String), usize> {
        &self.counts
    }

    /// Total number of loop restarts across all edges.
    pub fn total(&self) -> usize {
        self.counts.values().sum()
    }
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
    /// Counts of how many times each loop_restart edge was traversed.
    pub loop_restarts: LoopCounter,
    /// Aggregate timing and outcome statistics for this run.
    pub stats: PipelineStats,
}

/// Resolve a retry target for a node using the spec's 4-level fallback chain.
///
/// Checks in order (spec section 3.4):
/// 1. node.retry_target
/// 2. node.fallback_retry_target
/// 3. graph.retry_target (graph-level attribute)
/// 4. graph.fallback_retry_target (graph-level attribute)
///
/// Returns None if no retry target is found at any level.
pub(crate) fn resolve_retry_target(node: &GraphNode, graph: &Graph) -> Option<String> {
    // 1. Node-level retry_target
    if let Some(NodeAttrValue::String(t)) = node.attrs.get("retry_target") {
        if !t.is_empty() {
            return Some(t.clone());
        }
    }
    // 2. Node-level fallback_retry_target
    if let Some(NodeAttrValue::String(t)) = node.attrs.get("fallback_retry_target") {
        if !t.is_empty() {
            return Some(t.clone());
        }
    }
    // 3. Graph-level retry_target
    if let Some(NodeAttrValue::String(t)) = graph.graph_attrs.get("retry_target") {
        if !t.is_empty() {
            return Some(t.clone());
        }
    }
    // 4. Graph-level fallback_retry_target
    if let Some(NodeAttrValue::String(t)) = graph.graph_attrs.get("fallback_retry_target") {
        if !t.is_empty() {
            return Some(t.clone());
        }
    }
    None
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
    emitter: Option<Arc<PipelineEventEmitter>>,
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
            emitter: None,
        }
    }

    /// Attach a pipeline event emitter for real-time observability.
    ///
    /// When set, the engine emits `PipelineEvent` variants at each lifecycle
    /// point: pipeline start/complete, node start/complete/fail, edge traversal.
    pub fn with_emitter(mut self, emitter: Arc<PipelineEventEmitter>) -> Self {
        self.emitter = Some(emitter);
        self
    }

    /// Apply sub-pipeline composition, inlining any SubPipeline nodes.
    ///
    /// Scans the graph for SubPipeline-type nodes and replaces them by
    /// parsing and inlining the referenced DOT files. `base_dir` is used
    /// to resolve relative `pipeline` attribute paths.
    ///
    /// This is a no-op if the graph contains no SubPipeline nodes.
    pub fn apply_sub_pipeline_transform(&mut self, base_dir: &str) -> Result<(), EngineError> {
        let has_sub_pipelines = self
            .graph
            .nodes
            .iter()
            .any(|n| n.node_type == NodeType::SubPipeline);
        if !has_sub_pipelines {
            return Ok(());
        }
        let transform = SubPipelineTransform::new(base_dir);
        self.graph = transform.apply(&self.graph)?;
        // Recompute goal gate after graph modification.
        self.goal_gate = GoalGate::from_graph(&self.graph);
        Ok(())
    }

    /// Helper to emit an event if an emitter is configured.
    fn emit(&self, event: PipelineEvent) {
        if let Some(ref emitter) = self.emitter {
            emitter.emit(event);
        }
    }

    /// Run the pipeline from the start node.
    ///
    /// Finds the single start node, then executes nodes in sequence
    /// following edge selections until an exit node is reached, no edges
    /// remain, or the max step limit is hit.
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::collections::HashMap;
    /// # use std::sync::Arc;
    /// # use async_trait::async_trait;
    /// # use smasher_attractor::graph::{Graph, GraphNode, GraphEdge, NodeType, NodeAttrValue};
    /// # use smasher_attractor::handler::{Handler, HandlerError, HandlerRegistry};
    /// # use smasher_attractor::state::{Context, Outcome};
    /// # use smasher_attractor::engine::Engine;
    /// #
    /// # struct SuccessHandler;
    /// # #[async_trait]
    /// # impl Handler for SuccessHandler {
    /// #     fn name(&self) -> &str { "ok" }
    /// #     async fn execute(&self, _n: &GraphNode, _c: &Context)
    /// #         -> Result<Outcome, HandlerError> { Ok(Outcome::success()) }
    /// #     fn handles(&self, _t: &NodeType) -> bool { true }
    /// # }
    /// #
    /// # #[tokio::main]
    /// # async fn main() {
    /// let graph = Graph {
    ///     name: Some("demo".into()),
    ///     nodes: vec![
    ///         GraphNode { id: "start".into(), node_type: NodeType::Start,
    ///                     label: None, attrs: HashMap::new() },
    ///         GraphNode { id: "exit".into(), node_type: NodeType::Exit,
    ///                     label: None, attrs: HashMap::new() },
    ///     ],
    ///     edges: vec![
    ///         GraphEdge { from: "start".into(), to: "exit".into(),
    ///                     label: None, condition: None, priority: None,
    ///                     loop_restart: false, attrs: HashMap::new() },
    ///     ],
    ///     default_node_attrs: HashMap::new(),
    ///     default_edge_attrs: HashMap::new(),
    ///     graph_attrs: HashMap::new(),
    /// };
    ///
    /// let mut registry = HandlerRegistry::new();
    /// registry.register(Arc::new(SuccessHandler));
    ///
    /// let engine = Engine::new(graph, registry);
    /// let result = engine.run(Context::new()).await.unwrap();
    ///
    /// assert_eq!(result.visited_nodes, vec!["start", "exit"]);
    /// assert_eq!(result.steps_taken, 2);
    /// # }
    /// ```
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
        let mut loop_restarts = LoopCounter::new();
        let pipeline_start = std::time::Instant::now();
        let mut failure_signatures: HashMap<u64, (String, String, u32)> = HashMap::new();
        let mut node_timings: Vec<NodeStats> = Vec::new();

        // Emit PipelineStarted event.
        let graph_name = self
            .graph
            .name
            .clone()
            .unwrap_or_else(|| "unnamed".to_string());
        self.emit(PipelineEvent::PipelineStarted {
            graph_name: graph_name.clone(),
            timestamp: Utc::now(),
        });

        // Set up stall watchdog if configured.
        // Use a unified cancellation token: either the user-provided one or a
        // fresh internal one. Both the watchdog and the main loop check this
        // same token so that stall detection works regardless of whether the
        // caller supplied an external token.
        let effective_cancel_token = self.config.cancellation_token.clone().unwrap_or_default();
        let last_progress = Arc::new(Mutex::new(std::time::Instant::now()));
        let stall_detected = Arc::new(AtomicBool::new(false));
        let watchdog_handle = if let Some(stall_timeout) = self.config.stall_timeout {
            let check_interval = self
                .config
                .stall_check_interval
                .unwrap_or(Duration::from_secs(5));

            let cancel_token = effective_cancel_token.clone();

            let progress = Arc::clone(&last_progress);
            let stall_flag = Arc::clone(&stall_detected);
            let timeout_secs = stall_timeout.as_secs();
            let handle = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(check_interval).await;
                    let last = *progress.lock().await;
                    if last.elapsed() > stall_timeout {
                        stall_flag.store(true, Ordering::Release);
                        cancel_token.cancel();
                        break;
                    }
                }
                timeout_secs
            });
            Some(handle)
        } else {
            None
        };

        let loop_result: Result<_, EngineError> = async {
        loop {
            // Check the unified cancellation token before each node.
            if effective_cancel_token.is_cancelled()
            {
                // Distinguish stall-triggered cancellation from user cancellation
                // using the atomic flag set by the watchdog task.
                if stall_detected.load(Ordering::Acquire) {
                    let timeout_secs = self
                        .config
                        .stall_timeout
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    self.emit(PipelineEvent::PipelineAborted {
                        reason: format!("stalled: no progress for {timeout_secs}s"),
                        timestamp: Utc::now(),
                    });
                    return Err(EngineError::Stalled { timeout_secs });
                }
                self.emit(PipelineEvent::PipelineAborted {
                    reason: "cancelled".to_string(),
                    timestamp: Utc::now(),
                });
                return Err(EngineError::Cancelled);
            }

            // Check max steps limit
            if steps >= self.config.max_steps {
                self.emit(PipelineEvent::PipelineAborted {
                    reason: format!("max steps ({}) exceeded", self.config.max_steps),
                    timestamp: Utc::now(),
                });
                return Err(EngineError::MaxStepsExceeded {
                    max_steps: self.config.max_steps,
                });
            }

            // Look up the current node
            let node =
                self.graph
                    .node(&current_node_id)
                    .ok_or_else(|| EngineError::NodeNotFound {
                        node_id: current_node_id.clone(),
                    })?;

            // Update stall watchdog progress on node start.
            *last_progress.lock().await = std::time::Instant::now();

            // Emit NodeStarted event.
            let node_start = std::time::Instant::now();
            self.emit(PipelineEvent::NodeStarted {
                node_id: current_node_id.clone(),
                node_type: format!("{:?}", node.node_type),
                timestamp: Utc::now(),
            });

            // Execute the handler for this node.
            // Handler errors are converted to failure outcomes so that normal
            // event flow (NodeFailed, edge selection, failure routing) still
            // runs. This ensures the TUI and headless output always see the
            // error instead of the pipeline silently dying.
            let mut outcome = match self.registry.execute(node, &context).await {
                Ok(o) => o,
                Err(handler_err) => {
                    tracing::error!(
                        node = %current_node_id,
                        error = %handler_err,
                        "handler error, converting to failure outcome"
                    );
                    Outcome::failure(handler_err.to_string())
                }
            };

            // Handle retries for retryable failures
            if outcome.is_retryable() {
                let policy = RetryPolicy::from_node(node);
                let mut retry_state = RetryState::new();
                retry_state.record_attempt(&outcome);

                while retry_state.should_retry(&policy, &outcome) {
                    let delay = compute_delay(&policy, retry_state.attempts);
                    tokio::time::sleep(delay).await;

                    outcome = match self.registry.execute(node, &context).await {
                        Ok(o) => o,
                        Err(handler_err) => {
                            tracing::error!(
                                node = %current_node_id,
                                error = %handler_err,
                                "handler error during retry, converting to failure outcome"
                            );
                            Outcome::failure(handler_err.to_string())
                        }
                    };
                    retry_state.record_attempt(&outcome);
                }

                // If still a failure after all retries, record as failed
                // and continue to edge selection (the outcome might route to an error edge)
            }

            // Update stall watchdog progress on node complete.
            *last_progress.lock().await = std::time::Instant::now();

            let node_duration_ms = node_start.elapsed().as_millis() as u64;

            // Record per-node timing and outcome for PipelineStats.
            let outcome_kind = match &outcome {
                Outcome::Success { .. } | Outcome::PartialSuccess { .. } => OutcomeKind::Success,
                Outcome::Failure { .. } => OutcomeKind::Failure,
                Outcome::Retry { .. } => OutcomeKind::Retry,
                Outcome::Skip { .. } => OutcomeKind::Skip,
            };
            node_timings.push(NodeStats {
                node_id: current_node_id.clone(),
                duration_ms: node_duration_ms,
                outcome_kind,
            });

            // Emit NodeCompleted or NodeFailed event.
            if outcome.is_failure() {
                self.emit(PipelineEvent::NodeFailed {
                    node_id: current_node_id.clone(),
                    error: format!("{:?}", outcome),
                    duration_ms: node_duration_ms,
                    timestamp: Utc::now(),
                });
            } else {
                self.emit(PipelineEvent::NodeCompleted {
                    node_id: current_node_id.clone(),
                    outcome: outcome.clone(),
                    duration_ms: node_duration_ms,
                    timestamp: Utc::now(),
                });
            }

            // Store artifact if artifact store is configured.
            if let Some(ref store) = self.config.artifact_store
                && let Ok(value) = serde_json::to_value(&outcome)
            {
                store.store(
                    &current_node_id,
                    "outcome",
                    "application/json",
                    value,
                );
            }

            steps += 1;

            // Record outcome and mark visited
            node_outcomes.insert(current_node_id.clone(), outcome.clone());
            if !visited_nodes.contains(&current_node_id) {
                visited_nodes.push(current_node_id.clone());
            }

            // Deterministic failure cycle detection: track repeated identical failures
            // and abort if the same (node, error) pair recurs too many times.
            if let Some(max_failures) = self.config.max_identical_failures {
                match &outcome {
                    Outcome::Failure { error, .. } => {
                        let mut hasher = DefaultHasher::new();
                        current_node_id.hash(&mut hasher);
                        error.hash(&mut hasher);
                        let sig = hasher.finish();

                        let entry = failure_signatures
                            .entry(sig)
                            .or_insert_with(|| (current_node_id.clone(), error.clone(), 0));
                        entry.2 += 1;

                        if entry.2 >= max_failures {
                            self.emit(PipelineEvent::PipelineAborted {
                                reason: format!(
                                    "deterministic failure cycle: node '{}' failed {} times with: {}",
                                    entry.0, entry.2, entry.1
                                ),
                                timestamp: Utc::now(),
                            });
                            return Err(EngineError::DeterministicFailureCycle {
                                node_id: entry.0.clone(),
                                error: entry.1.clone(),
                                count: entry.2,
                            });
                        }
                    }
                    Outcome::Success { .. } | Outcome::PartialSuccess { .. } => {
                        // On success, remove any failure signatures for this node.
                        failure_signatures.retain(|_, (nid, _, _)| nid != &current_node_id);
                    }
                    Outcome::Retry { .. } | Outcome::Skip { .. } => {}
                }
            }

            // Auto-save checkpoint after each node (when configured).
            if self.config.enable_checkpointing
                && let Some(ref checkpoint_dir) = self.config.checkpoint_dir
            {
                let mut cp =
                    Checkpoint::new(graph_name.clone(), current_node_id.clone(), &context);
                for id in &visited_nodes {
                    cp.mark_visited(id);
                }
                for (id, out) in &node_outcomes {
                    cp.add_outcome(id, out.clone());
                }
                match cp.to_json() {
                    Ok(json_str) => {
                        let cp_path = checkpoint_dir.join("checkpoint.json");
                        if let Err(e) = std::fs::write(&cp_path, &json_str) {
                            tracing::warn!(
                                path = %cp_path.display(),
                                error = %e,
                                "failed to write auto-save checkpoint"
                            );
                        } else {
                            self.emit(PipelineEvent::CheckpointCreated {
                                node_id: current_node_id.clone(),
                                timestamp: Utc::now(),
                            });
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "failed to serialize auto-save checkpoint"
                        );
                    }
                }
            }

            // If exit node, check goal gates before exiting (spec section 3.4).
            // Outcome-aware: goals must have SUCCESS or PARTIAL_SUCCESS, not just visited.
            // Retry target comes from the failed goal node's 4-level fallback chain.
            if node.node_type == NodeType::Exit {
                if let Err(unsatisfied) = self.goal_gate.check_outcomes(&node_outcomes) {
                    if let Some(failed_node) = self.graph.node(&unsatisfied.node_id)
                        && let Some(target) = resolve_retry_target(failed_node, &self.graph)
                    {
                        // Validate retry target exists in graph
                        if self.graph.node(&target).is_none() {
                            return Err(EngineError::NodeNotFound {
                                node_id: target,
                            });
                        }
                        tracing::info!(
                            goal = %unsatisfied.node_id,
                            reason = %unsatisfied.reason,
                            retry_target = %target,
                            "goal gate unsatisfied, routing to retry target"
                        );
                        current_node_id = target;
                        continue;
                    }
                    // No retry target at any level — fail with all unsatisfied goals
                    return Err(EngineError::GoalEnforcement(
                        self.goal_gate.enforce_outcomes(&node_outcomes)
                            .unwrap_err()
                    ));
                }
                break;
            }

            // Inject outcome status into context so condition expressions like
            // `outcome=success` can match against it during edge selection.
            let outcome_label = match &outcome {
                Outcome::Success { .. } | Outcome::PartialSuccess { .. } => "success",
                Outcome::Failure { .. } => "fail",
                Outcome::Retry { .. } => "retry",
                Outcome::Skip { .. } => "skip",
            };
            context.set("outcome", serde_json::json!(outcome_label));

            // Select next edge
            let last_outcome = node_outcomes.get(&current_node_id);
            let next_edge = select_edge(&self.graph, &current_node_id, &context, last_outcome)?;

            match next_edge {
                Some(edge) => {
                    // Emit EdgeTraversed event.
                    self.emit(PipelineEvent::EdgeTraversed {
                        from: edge.from.clone(),
                        to: edge.to.clone(),
                        label: edge.label.clone(),
                        timestamp: Utc::now(),
                    });

                    // Handle loop_restart edge semantics
                    if edge.loop_restart {
                        loop_restarts.increment(&edge.from, &edge.to);

                        self.emit(PipelineEvent::LoopRestarted {
                            from: edge.from.clone(),
                            to: edge.to.clone(),
                            restart_count: loop_restarts.count(&edge.from, &edge.to),
                            timestamp: Utc::now(),
                        });

                        // Clear context entries prefixed with the source node's ID
                        let prefix = format!("{}_", edge.from);
                        let keys_to_remove: Vec<String> = context
                            .keys()
                            .into_iter()
                            .filter(|k| k.starts_with(&prefix))
                            .collect();
                        for key in keys_to_remove {
                            context.remove(&key);
                        }

                        tracing::info!(
                            from = %edge.from,
                            to = %edge.to,
                            traversal_count = loop_restarts.count(&edge.from, &edge.to),
                            "loop_restart edge traversed, context entries for source node cleared"
                        );
                    }

                    // Apply fidelity processing if configured.
                    if let Some(ref fidelity_config) = self.config.fidelity_config {
                        let processor = FidelityProcessor::new(fidelity_config.clone());
                        let processed = processor.process(&edge.from, &edge.to, &context);
                        let new_snapshot = processed.snapshot();
                        // Clear existing context and replace with processed version.
                        for key in context.keys() {
                            context.remove(&key);
                        }
                        for (k, v) in new_snapshot {
                            context.set(k, v);
                        }
                    }

                    current_node_id = edge.to.clone();
                }
                None => {
                    // Spec 3.7: When a node fails and no edge matches,
                    // try the node's retry_target fallback chain.
                    if outcome.is_failure() {
                        if let Some(target) = resolve_retry_target(node, &self.graph) {
                            // Validate retry target exists in graph
                            if self.graph.node(&target).is_none() {
                                return Err(EngineError::NodeNotFound {
                                    node_id: target,
                                });
                            }
                            tracing::info!(
                                node = %current_node_id,
                                retry_target = %target,
                                "no fail edge found, routing to retry target"
                            );
                            current_node_id = target;
                            continue;
                        }
                        // Spec 3.7 step 4: no failure route found — pipeline fails
                        // with the stage's failure reason.
                        let error_msg = match &outcome {
                            Outcome::Failure { error, .. } => error.clone(),
                            _ => "unknown failure".to_string(),
                        };
                        self.emit(PipelineEvent::PipelineAborted {
                            reason: format!(
                                "node '{}' failed with no available route: {}",
                                current_node_id, error_msg
                            ),
                            timestamp: Utc::now(),
                        });
                        return Err(EngineError::UnroutableFailure {
                            node_id: current_node_id.clone(),
                            error: error_msg,
                        });
                    }
                    // Success/skip with no outgoing edge — end execution.
                    break;
                }
            }
        }

        Ok(())
        }.await;

        // Abort the watchdog task if it's running.
        if let Some(handle) = watchdog_handle {
            handle.abort();
        }

        // Propagate any error from the loop.
        loop_result?;

        // Final goal gate enforcement (outcome-aware, spec section 3.4).
        // Reached when the loop exits without hitting an Exit node (no outgoing edge).
        self.goal_gate.enforce_outcomes(&node_outcomes)?;

        let pipeline_duration_ms = pipeline_start.elapsed().as_millis() as u64;

        // Emit PipelineCompleted event.
        let final_outcome = visited_nodes
            .last()
            .and_then(|id| node_outcomes.get(id))
            .cloned()
            .unwrap_or_else(Outcome::success);
        self.emit(PipelineEvent::PipelineCompleted {
            outcome: final_outcome,
            total_nodes: visited_nodes.len(),
            duration_ms: pipeline_duration_ms,
            timestamp: Utc::now(),
        });

        // Build checkpoint if enabled
        let checkpoint = if self.config.enable_checkpointing {
            let last_node = visited_nodes.last().cloned().unwrap_or_default();
            let mut cp = Checkpoint::new(graph_name, last_node, &context);
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

        let stats = PipelineStats::from_node_timings(node_timings, pipeline_duration_ms);

        Ok(ExecutionResult {
            visited_nodes,
            node_outcomes,
            final_context: context.snapshot(),
            steps_taken: steps,
            checkpoint,
            loop_restarts,
            stats,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{PipelineEvent, PipelineEventEmitter};
    use crate::graph::{Graph, GraphEdge, GraphNode, NodeAttrValue, NodeType};
    use crate::handler::{Handler, HandlerError, HandlerRegistry};
    use crate::state::{Context, Outcome};
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

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

    fn make_conditional_edge(from: &str, to: &str, condition: &str) -> GraphEdge {
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: None,
            condition: Some(condition.to_string()),
            priority: None,
            loop_restart: false,
            attrs: HashMap::new(),
        }
    }

    fn make_loop_restart_edge(from: &str, to: &str) -> GraphEdge {
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            label: None,
            condition: None,
            priority: None,
            loop_restart: true,
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
            graph_attrs: HashMap::new(),
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
        assert!(config.cancellation_token.is_none());
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
            ..EngineConfig::default()
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
            vec![make_edge("start", "exit"), make_edge("exit", "unreachable")],
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
    // Test 8: Handler error converts to failure outcome
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn handler_error_converts_to_failure_outcome() {
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

        // Handler errors are converted to Outcome::Failure so that normal
        // event flow (NodeFailed, edge selection) runs. The pipeline completes
        // with failure outcomes rather than aborting silently.
        let result = result.expect("pipeline should complete, not abort on handler error");
        let start_outcome = result.node_outcomes.get("start").unwrap();
        assert!(start_outcome.is_failure());
        match start_outcome {
            Outcome::Failure { error, .. } => {
                assert!(
                    error.contains("catastrophic failure"),
                    "failure outcome should preserve the handler error message, got: {error}"
                );
            }
            other => panic!("expected Failure outcome, got: {other:?}"),
        }
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
        for outcome in result.node_outcomes.values() {
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
        assert_eq!(result.final_context.get("initial"), Some(&json!("value")));
        assert_eq!(
            result.final_context.get("visited_start"),
            Some(&json!(true))
        );
        assert_eq!(result.final_context.get("visited_exit"), Some(&json!(true)));
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
            ..EngineConfig::default()
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
            ..EngineConfig::default()
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
            graph_attrs: HashMap::new(),
        };
        let config = EngineConfig {
            max_steps: 1000,
            enable_checkpointing: true,
            ..EngineConfig::default()
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
            ..EngineConfig::default()
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
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        assert_eq!(engine.config.max_steps, 42);
        assert!(!engine.config.enable_checkpointing);
    }

    // ---------------------------------------------------------------
    // Test 27: LoopCounter basic operations
    // ---------------------------------------------------------------
    #[test]
    fn loop_counter_basic_operations() {
        let mut counter = LoopCounter::new();
        assert_eq!(counter.count("a", "b"), 0);
        assert_eq!(counter.total(), 0);

        counter.increment("a", "b");
        assert_eq!(counter.count("a", "b"), 1);
        assert_eq!(counter.total(), 1);

        counter.increment("a", "b");
        assert_eq!(counter.count("a", "b"), 2);
        assert_eq!(counter.total(), 2);

        counter.increment("c", "d");
        assert_eq!(counter.count("c", "d"), 1);
        assert_eq!(counter.total(), 3);

        let counts = counter.counts();
        assert_eq!(counts.len(), 2);
    }

    // ---------------------------------------------------------------
    // Test 28: LoopCounter default is empty
    // ---------------------------------------------------------------
    #[test]
    fn loop_counter_default_is_empty() {
        let counter = LoopCounter::default();
        assert_eq!(counter.total(), 0);
        assert!(counter.counts().is_empty());
    }

    // ---------------------------------------------------------------
    // Test 29: loop_restart edge increments loop counter
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn loop_restart_edge_increments_counter() {
        // Pipeline: start -> check -> process -> check (loop_restart back)
        // After 2 loops, "check" sets context to break out to exit
        // We use context_setting_registry so the handler sets "visited_<id>"
        // and a conditional edge to route to exit after enough loops.
        //
        // Simpler approach: use a cycle with max_steps to force termination.
        // start -> a -> b --(loop_restart)--> a (cycle until max_steps)
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("b", NodeType::Generic),
            ],
            vec![
                make_edge("start", "a"),
                make_edge("a", "b"),
                make_loop_restart_edge("b", "a"),
            ],
        );
        let config = EngineConfig {
            max_steps: 7,
            enable_checkpointing: false,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        // Should hit max_steps. The loop_restart edge b->a should have been traversed.
        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::MaxStepsExceeded { max_steps } => {
                assert_eq!(max_steps, 7);
            }
            other => panic!("expected MaxStepsExceeded, got: {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 30: loop_restart clears source node context entries
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn loop_restart_clears_source_context() {
        // Pipeline: start -> worker -> exit, but with loop_restart from worker back to worker.
        // Pipeline: start -> worker -> router
        // router -> exit (condition: loop_done=true)
        // router -> worker (loop_restart, no condition, lower priority, fallback)
        //
        // On the first pass, loop_done is not set, so router falls back to worker.
        // The loop_restart edge clears context keys prefixed with "router_".
        // On the second pass through router, loop_done is set, routing to exit.

        /// Handler that counts executions and sets loop_done after first pass.
        struct LoopControlHandler;

        #[async_trait]
        impl Handler for LoopControlHandler {
            fn name(&self) -> &str {
                "loop_control"
            }
            async fn execute(
                &self,
                node: &GraphNode,
                context: &Context,
            ) -> Result<Outcome, HandlerError> {
                // Track per-node execution count using a key NOT prefixed with node ID
                // so it survives loop_restart clearing
                let count_key = format!("exec_count_{}", node.id);
                let current: i64 = context
                    .get(&count_key)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let new_count = current + 1;
                context.set(count_key, json!(new_count));

                // Set a node-prefixed key to test clearing
                context.set(format!("{}_data", node.id), json!("some_data"));

                // After first execution of router, set loop_done
                if node.id == "router" && new_count >= 2 {
                    context.set("loop_done", json!("true"));
                }

                Ok(Outcome::success())
            }
            fn handles(&self, _node_type: &NodeType) -> bool {
                true
            }
        }

        let mut exit_edge = make_conditional_edge("router", "exit", "loop_done=true");
        exit_edge.priority = Some(10);

        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("worker", NodeType::Generic),
                make_node("router", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "worker"),
                make_edge("worker", "router"),
                exit_edge,
                make_loop_restart_edge("router", "worker"),
            ],
        );

        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(LoopControlHandler));
        let config = EngineConfig {
            max_steps: 20,
            enable_checkpointing: false,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, registry, config);

        let ctx = Context::new();
        // Set a worker-prefixed context key to verify it gets cleared on loop_restart
        ctx.set("router_preserved_key", json!("should_be_cleared"));

        let result = engine.run(ctx).await.unwrap();

        // Should have visited: start, worker, router (looped back), worker, router, exit
        assert!(result.visited_nodes.contains(&"exit".to_string()));

        // The loop_restart edge router->worker should have been traversed once
        assert_eq!(result.loop_restarts.count("router", "worker"), 1);
        assert_eq!(result.loop_restarts.total(), 1);

        // After loop_restart, "router_" prefixed keys were cleared
        // But then router executed again and set router_data again,
        // so it should be present in the final context.
        assert!(result.final_context.contains_key("router_data"));

        // The "router_preserved_key" was set initially with the "router_" prefix,
        // so it should have been cleared by the loop_restart
        assert!(!result.final_context.contains_key("router_preserved_key"));
    }

    // ---------------------------------------------------------------
    // Test 31: loop_restart with max_steps prevents infinite loops
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn loop_restart_max_steps_prevents_infinite_loop() {
        // Create a tight loop: start -> a -> b --(loop_restart)--> a
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("b", NodeType::Generic),
            ],
            vec![
                make_edge("start", "a"),
                make_edge("a", "b"),
                make_loop_restart_edge("b", "a"),
            ],
        );
        let config = EngineConfig {
            max_steps: 10,
            enable_checkpointing: false,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::MaxStepsExceeded { max_steps } => {
                assert_eq!(max_steps, 10);
            }
            other => panic!("expected MaxStepsExceeded, got: {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 32: Non-loop_restart edge does not affect loop counter
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn non_loop_restart_edge_no_counter_increment() {
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

        assert_eq!(result.loop_restarts.total(), 0);
        assert!(result.loop_restarts.counts().is_empty());
    }

    // ---------------------------------------------------------------
    // Test 33: loop_restart edge traversal logged in execution result
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn loop_restart_counter_in_execution_result() {
        // start -> a -> b --(loop_restart)--> a, max_steps = 9
        // Pattern: start(1), a(2), b(3), a(4), b(5), a(6), b(7), a(8), b(9) => max_steps
        // loop_restart traversals: b->a happens at steps 3, 5, 7, 9 = 4 times
        // Actually at step 9 it would be step 9, then try to loop but we hit max first.
        // Let's trace: step 1=start, step 2=a, step 3=b, loop_restart->a,
        //   step 4=a, step 5=b, loop_restart->a,
        //   step 6=a, step 7=b, loop_restart->a,
        //   step 8=a, step 9=b, loop_restart->a,
        //   step 10 would be a but max_steps=9 so error at step 10 check... wait no.
        //   max_steps check is: if steps >= max_steps at the TOP of the loop.
        //   So steps is incremented after execution, so:
        //   iteration 1: steps=0 < 9, execute start, steps=1
        //   iteration 2: steps=1 < 9, execute a, steps=2
        //   iteration 3: steps=2 < 9, execute b, steps=3, loop_restart
        //   iteration 4: steps=3 < 9, execute a, steps=4
        //   iteration 5: steps=4 < 9, execute b, steps=5, loop_restart
        //   iteration 6: steps=5 < 9, execute a, steps=6
        //   iteration 7: steps=6 < 9, execute b, steps=7, loop_restart
        //   iteration 8: steps=7 < 9, execute a, steps=8
        //   iteration 9: steps=8 < 9, execute b, steps=9, loop_restart
        //   iteration 10: steps=9 >= 9, MaxStepsExceeded
        // So 4 loop_restart traversals.

        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("b", NodeType::Generic),
            ],
            vec![
                make_edge("start", "a"),
                make_edge("a", "b"),
                make_loop_restart_edge("b", "a"),
            ],
        );
        let config = EngineConfig {
            max_steps: 9,
            enable_checkpointing: false,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        // Should fail with MaxStepsExceeded
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EngineError::MaxStepsExceeded { .. }
        ));
    }

    // ---------------------------------------------------------------
    // Test 34: Engine with emitter emits PipelineStarted and PipelineCompleted
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn emitter_emits_pipeline_lifecycle_events() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let emitter = Arc::new(PipelineEventEmitter::new(64));
        let mut rx = emitter.subscribe();

        let engine = Engine::new(graph, success_registry()).with_emitter(emitter);
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();
        assert_eq!(result.steps_taken, 2);

        // Drain events and check lifecycle ordering.
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Should have: PipelineStarted, NodeStarted(start), NodeCompleted(start),
        // EdgeTraversed, NodeStarted(exit), NodeCompleted(exit), PipelineCompleted
        assert!(matches!(events[0], PipelineEvent::PipelineStarted { .. }));
        assert!(matches!(
            events.last().unwrap(),
            PipelineEvent::PipelineCompleted { .. }
        ));

        let node_started_count = events
            .iter()
            .filter(|e| matches!(e, PipelineEvent::NodeStarted { .. }))
            .count();
        let node_completed_count = events
            .iter()
            .filter(|e| matches!(e, PipelineEvent::NodeCompleted { .. }))
            .count();
        assert_eq!(node_started_count, 2);
        assert_eq!(node_completed_count, 2);
    }

    // ---------------------------------------------------------------
    // Test 35: Engine with emitter emits EdgeTraversed
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn emitter_emits_edge_traversed() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "a"), make_edge("a", "exit")],
        );
        let emitter = Arc::new(PipelineEventEmitter::new(64));
        let mut rx = emitter.subscribe();

        let engine = Engine::new(graph, success_registry()).with_emitter(emitter);
        let ctx = Context::new();
        engine.run(ctx).await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let edge_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, PipelineEvent::EdgeTraversed { .. }))
            .collect();
        assert_eq!(edge_events.len(), 2);
    }

    // ---------------------------------------------------------------
    // Test 36: Cancellation token stops execution
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn cancellation_token_stops_execution() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "a"), make_edge("a", "exit")],
        );

        let token = CancellationToken::new();
        // Cancel immediately before running.
        token.cancel();

        let config = EngineConfig {
            max_steps: 1000,
            enable_checkpointing: false,
            cancellation_token: Some(token),
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), EngineError::Cancelled));
    }

    // ---------------------------------------------------------------
    // Test 37: Cancellation emits PipelineAborted event
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn cancellation_emits_aborted_event() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );

        let token = CancellationToken::new();
        token.cancel();

        let emitter = Arc::new(PipelineEventEmitter::new(64));
        let mut rx = emitter.subscribe();

        let config = EngineConfig {
            max_steps: 1000,
            enable_checkpointing: false,
            cancellation_token: Some(token),
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, success_registry(), config).with_emitter(emitter);
        let ctx = Context::new();
        let _ = engine.run(ctx).await;

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Should have PipelineStarted then PipelineAborted.
        assert!(matches!(events[0], PipelineEvent::PipelineStarted { .. }));
        assert!(
            matches!(events[1], PipelineEvent::PipelineAborted { ref reason, .. } if reason == "cancelled")
        );
    }

    // ---------------------------------------------------------------
    // Test 38: Engine without emitter works (no crash)
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn engine_without_emitter_works() {
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
        assert_eq!(result.steps_taken, 2);
    }

    // ---------------------------------------------------------------
    // Test 39: Cancelled error display
    // ---------------------------------------------------------------
    #[test]
    fn cancelled_error_display() {
        let err = EngineError::Cancelled;
        assert_eq!(err.to_string(), "pipeline cancelled");
    }

    // ---------------------------------------------------------------
    // Test 40: Emitter captures NodeFailed for failure outcomes
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn emitter_emits_node_failed_for_failure_outcome() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let emitter = Arc::new(PipelineEventEmitter::new(64));
        let mut rx = emitter.subscribe();

        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(AlwaysFailHandler));
        let engine = Engine::new(graph, registry).with_emitter(emitter);
        let ctx = Context::new();
        let _ = engine.run(ctx).await;

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let failed_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, PipelineEvent::NodeFailed { .. }))
            .collect();
        // start node should produce a NodeFailed since AlwaysFailHandler returns Outcome::failure
        assert!(!failed_events.is_empty());
    }

    // ---------------------------------------------------------------
    // Test 41: Config default includes new fields
    // ---------------------------------------------------------------
    #[test]
    fn config_default_new_fields() {
        let config = EngineConfig::default();
        assert!(config.checkpoint_dir.is_none());
        assert!(config.stall_timeout.is_none());
        assert!(config.stall_check_interval.is_none());
        assert_eq!(config.max_identical_failures, Some(3));
    }

    // ---------------------------------------------------------------
    // Test 42: Auto-save checkpoint writes file after each node
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn autosave_checkpoint_writes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cp_dir = tmp.path().to_path_buf();

        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "a"), make_edge("a", "exit")],
        );
        let config = EngineConfig {
            max_steps: 50,
            enable_checkpointing: true,
            checkpoint_dir: Some(cp_dir.clone()),
            max_identical_failures: None,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        assert_eq!(result.steps_taken, 3);

        // Verify checkpoint.json was written
        let cp_path = cp_dir.join("checkpoint.json");
        assert!(cp_path.exists(), "checkpoint.json should exist after run");

        // Verify checkpoint content: it should reflect the final node (exit)
        let cp_content = std::fs::read_to_string(&cp_path).unwrap();
        let cp = Checkpoint::from_json(&cp_content).unwrap();
        assert_eq!(cp.pipeline_name, "test_pipeline");
        assert!(cp.was_visited("start"));
        assert!(cp.was_visited("a"));
        assert!(cp.was_visited("exit"));
        assert_eq!(cp.current_node, "exit");
    }

    // ---------------------------------------------------------------
    // Test 43: Auto-save checkpoint emits CheckpointCreated events
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn autosave_checkpoint_emits_events() {
        let tmp = tempfile::tempdir().unwrap();
        let cp_dir = tmp.path().to_path_buf();

        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let emitter = Arc::new(PipelineEventEmitter::new(64));
        let mut rx = emitter.subscribe();

        let config = EngineConfig {
            max_steps: 50,
            enable_checkpointing: true,
            checkpoint_dir: Some(cp_dir),
            max_identical_failures: None,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, success_registry(), config).with_emitter(emitter);
        let ctx = Context::new();
        engine.run(ctx).await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let cp_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, PipelineEvent::CheckpointCreated { .. }))
            .collect();
        // One checkpoint per node: start and exit = 2 events
        assert_eq!(cp_events.len(), 2);
    }

    // ---------------------------------------------------------------
    // Test 44: No auto-save without checkpoint_dir
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn no_autosave_without_checkpoint_dir() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let emitter = Arc::new(PipelineEventEmitter::new(64));
        let mut rx = emitter.subscribe();

        let config = EngineConfig {
            max_steps: 50,
            enable_checkpointing: true,
            // checkpoint_dir is None (default) -- no auto-save
            max_identical_failures: None,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, success_registry(), config).with_emitter(emitter);
        let ctx = Context::new();
        engine.run(ctx).await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        let cp_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, PipelineEvent::CheckpointCreated { .. }))
            .collect();
        assert_eq!(
            cp_events.len(),
            0,
            "no CheckpointCreated without checkpoint_dir"
        );
    }

    // ---------------------------------------------------------------
    // Test 45: Stall watchdog detects stalled pipeline
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn stall_watchdog_detects_stall() {
        /// Handler that sleeps for a long time to trigger stall detection.
        struct SlowHandler;

        #[async_trait]
        impl Handler for SlowHandler {
            fn name(&self) -> &str {
                "slow"
            }
            async fn execute(
                &self,
                node: &GraphNode,
                _context: &Context,
            ) -> Result<Outcome, HandlerError> {
                if node.id == "slow_node" {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                Ok(Outcome::success())
            }
            fn handles(&self, _node_type: &NodeType) -> bool {
                true
            }
        }

        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("slow_node", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "slow_node"),
                make_edge("slow_node", "exit"),
            ],
        );
        let token = CancellationToken::new();
        let config = EngineConfig {
            max_steps: 50,
            enable_checkpointing: false,
            cancellation_token: Some(token),
            stall_timeout: Some(std::time::Duration::from_millis(100)),
            stall_check_interval: Some(std::time::Duration::from_millis(30)),
            max_identical_failures: None,
            ..EngineConfig::default()
        };

        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(SlowHandler));

        let engine = Engine::with_config(graph, registry, config);
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::Stalled { timeout_secs } => {
                // Duration was 100ms, so as_secs() = 0
                assert_eq!(timeout_secs, 0);
            }
            other => panic!("expected Stalled, got: {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 45b: Stall watchdog works without external cancellation token
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn stall_watchdog_works_without_external_token() {
        /// Handler that sleeps to trigger stall detection.
        struct SlowHandler;

        #[async_trait]
        impl Handler for SlowHandler {
            fn name(&self) -> &str {
                "slow"
            }
            async fn execute(
                &self,
                node: &GraphNode,
                _context: &Context,
            ) -> Result<Outcome, HandlerError> {
                if node.id == "slow_node" {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                Ok(Outcome::success())
            }
            fn handles(&self, _node_type: &NodeType) -> bool {
                true
            }
        }

        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("slow_node", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "slow_node"),
                make_edge("slow_node", "exit"),
            ],
        );
        // No external cancellation token — the engine must create one internally.
        let config = EngineConfig {
            max_steps: 50,
            enable_checkpointing: false,
            cancellation_token: None,
            stall_timeout: Some(std::time::Duration::from_millis(100)),
            stall_check_interval: Some(std::time::Duration::from_millis(30)),
            max_identical_failures: None,
            ..EngineConfig::default()
        };

        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(SlowHandler));

        let engine = Engine::with_config(graph, registry, config);
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::Stalled { .. } => { /* expected */ }
            other => panic!("expected Stalled, got: {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 46: No stall with fast execution
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn no_stall_with_fast_execution() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "a"), make_edge("a", "exit")],
        );
        let token = CancellationToken::new();
        let config = EngineConfig {
            max_steps: 50,
            enable_checkpointing: false,
            cancellation_token: Some(token),
            stall_timeout: Some(std::time::Duration::from_secs(10)),
            stall_check_interval: Some(std::time::Duration::from_secs(5)),
            max_identical_failures: None,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().steps_taken, 3);
    }

    // ---------------------------------------------------------------
    // Test 47: Deterministic failure cycle detection triggers
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn deterministic_failure_cycle_detection() {
        // Graph with a loop: start -> fail_node -> fail_node (via loop_restart)
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("fail_node", NodeType::Generic),
            ],
            vec![
                make_edge("start", "fail_node"),
                make_loop_restart_edge("fail_node", "fail_node"),
            ],
        );
        let config = EngineConfig {
            max_steps: 100,
            enable_checkpointing: false,
            max_identical_failures: Some(3),
            ..EngineConfig::default()
        };

        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(AlwaysFailHandler));

        let engine = Engine::with_config(graph, registry, config);
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            EngineError::DeterministicFailureCycle {
                node_id,
                error,
                count,
            } => {
                assert_eq!(node_id, "fail_node");
                assert!(error.contains("handler always fails"));
                assert_eq!(count, 3);
            }
            other => panic!("expected DeterministicFailureCycle, got: {other:?}"),
        }
    }

    // ---------------------------------------------------------------
    // Test 48: Failure cycle detection disabled when None
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn failure_cycle_detection_disabled_when_none() {
        // Same looping-fail graph, but max_identical_failures = None
        // Should hit max_steps instead
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("fail_node", NodeType::Generic),
            ],
            vec![
                make_edge("start", "fail_node"),
                make_loop_restart_edge("fail_node", "fail_node"),
            ],
        );
        let config = EngineConfig {
            max_steps: 10,
            enable_checkpointing: false,
            max_identical_failures: None,
            ..EngineConfig::default()
        };

        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(AlwaysFailHandler));

        let engine = Engine::with_config(graph, registry, config);
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            EngineError::MaxStepsExceeded { .. }
        ));
    }

    // ---------------------------------------------------------------
    // Test 49: Success resets failure cycle counter
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn success_resets_failure_cycle_counter() {
        /// Handler that alternates between fail and success based on context.
        /// Uses a context key ("cycle_runs") that is NOT prefixed with the
        /// node ID to avoid being cleared by loop_restart edge semantics.
        struct AlternatingHandler;

        #[async_trait]
        impl Handler for AlternatingHandler {
            fn name(&self) -> &str {
                "alternating"
            }
            async fn execute(
                &self,
                node: &GraphNode,
                context: &Context,
            ) -> Result<Outcome, HandlerError> {
                if node.id == "cycler" {
                    let count: i64 = context
                        .get("cycle_runs")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    context.set("cycle_runs", serde_json::json!(count + 1));
                    // Fail twice, succeed on third, repeat
                    if count % 3 < 2 {
                        Ok(Outcome::failure("intermittent failure"))
                    } else {
                        Ok(Outcome::success())
                    }
                } else {
                    Ok(Outcome::success())
                }
            }
            fn handles(&self, _node_type: &NodeType) -> bool {
                true
            }
        }

        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("cycler", NodeType::Generic),
            ],
            vec![
                make_edge("start", "cycler"),
                make_loop_restart_edge("cycler", "cycler"),
            ],
        );
        let config = EngineConfig {
            max_steps: 20,
            enable_checkpointing: false,
            max_identical_failures: Some(3),
            ..EngineConfig::default()
        };

        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(AlternatingHandler));

        let engine = Engine::with_config(graph, registry, config);
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        // With max_identical_failures=3, it would trigger if we had 3 consecutive
        // identical failures. But every 3rd iteration succeeds, resetting the counter.
        // So it should hit max_steps instead.
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), EngineError::MaxStepsExceeded { .. }),
            "should hit max_steps, not DeterministicFailureCycle"
        );
    }

    // ---------------------------------------------------------------
    // Test 50: Stalled and DeterministicFailureCycle error display
    // ---------------------------------------------------------------
    #[test]
    fn new_error_display_messages() {
        let err1 = EngineError::Stalled { timeout_secs: 30 };
        assert!(err1.to_string().contains("30"));
        assert!(err1.to_string().contains("stalled"));

        let err2 = EngineError::DeterministicFailureCycle {
            node_id: "node_x".to_string(),
            error: "boom".to_string(),
            count: 5,
        };
        assert!(err2.to_string().contains("node_x"));
        assert!(err2.to_string().contains("boom"));
        assert!(err2.to_string().contains("5"));
    }

    // ---------------------------------------------------------------
    // Test 51: Auto-save checkpoint includes correct node outcomes
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn autosave_checkpoint_includes_outcomes() {
        let tmp = tempfile::tempdir().unwrap();
        let cp_dir = tmp.path().to_path_buf();

        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("worker", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "worker"), make_edge("worker", "exit")],
        );
        let config = EngineConfig {
            max_steps: 50,
            enable_checkpointing: true,
            checkpoint_dir: Some(cp_dir.clone()),
            max_identical_failures: None,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, success_registry(), config);
        let ctx = Context::new();
        engine.run(ctx).await.unwrap();

        let cp_content = std::fs::read_to_string(cp_dir.join("checkpoint.json")).unwrap();
        let cp = Checkpoint::from_json(&cp_content).unwrap();

        // All 3 nodes should have outcomes
        assert!(cp.node_outcomes.get("start").unwrap().is_success());
        assert!(cp.node_outcomes.get("worker").unwrap().is_success());
        assert!(cp.node_outcomes.get("exit").unwrap().is_success());
    }

    // ---------------------------------------------------------------
    // Test 52: EngineConfig defaults None for fidelity and artifact fields
    // ---------------------------------------------------------------
    #[test]
    fn engine_config_defaults_none_for_new_fields() {
        let config = EngineConfig::default();
        assert!(config.fidelity_config.is_none());
        assert!(config.artifact_store.is_none());
    }

    // ---------------------------------------------------------------
    // Test 53: Fidelity config None preserves existing behavior
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn fidelity_config_none_works_identically() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "a"), make_edge("a", "exit")],
        );
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(AlwaysSuccessHandler));

        let config = EngineConfig {
            fidelity_config: None,
            enable_checkpointing: false,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, registry, config);
        let context = Context::new();
        context.set("preserved_key", json!("should_remain"));
        let result = engine.run(context).await.unwrap();
        assert_eq!(result.steps_taken, 3);
        // Context should still have the key since fidelity is off (Full by default)
        assert!(result.final_context.contains_key("preserved_key"));
    }

    // ---------------------------------------------------------------
    // Test 54: Fidelity Reset clears context between nodes
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn fidelity_reset_clears_context_between_nodes() {
        use crate::fidelity::{FidelityConfig, FidelityMode};

        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "a"), make_edge("a", "exit")],
        );
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(ContextSettingHandler));

        let config = EngineConfig {
            fidelity_config: Some(FidelityConfig::new(FidelityMode::Reset)),
            enable_checkpointing: false,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, registry, config);
        let context = Context::new();
        context.set("initial_key", json!("initial_value"));
        let result = engine.run(context).await.unwrap();
        assert_eq!(result.steps_taken, 3);
        // With Reset fidelity, context is cleared between each node transition.
        // The "initial_key" should be wiped after the first edge traversal.
        assert!(!result.final_context.contains_key("initial_key"));
    }

    // ---------------------------------------------------------------
    // Test 55: ArtifactStore captures outcomes for all nodes
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn artifact_store_captures_outcomes() {
        use crate::artifact::ArtifactStore;

        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("a", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "a"), make_edge("a", "exit")],
        );
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(AlwaysSuccessHandler));

        let store = ArtifactStore::new();
        let config = EngineConfig {
            artifact_store: Some(store.clone()),
            enable_checkpointing: false,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(graph, registry, config);
        let result = engine.run(Context::new()).await.unwrap();
        assert_eq!(result.steps_taken, 3);

        // All 3 nodes should have artifacts stored
        let all = store.list();
        assert_eq!(all.len(), 3, "expected 3 artifacts, got {}", all.len());
    }

    // ---------------------------------------------------------------
    // Test 56: Sub-pipeline transform is a no-op without SubPipeline nodes
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn sub_pipeline_transform_noop_without_sub_pipelines() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "exit")],
        );
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(AlwaysSuccessHandler));

        let mut engine = Engine::with_config(graph, registry, EngineConfig::default());
        // Should be a no-op, no error
        engine.apply_sub_pipeline_transform("/tmp").unwrap();
        let result = engine.run(Context::new()).await.unwrap();
        assert_eq!(result.steps_taken, 2);
    }

    // ---------------------------------------------------------------
    // Test 57: goal_gate attribute is recognized (not just goal)
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn goal_gate_attribute_recognized_in_engine() {
        let mut goal_attrs = HashMap::new();
        goal_attrs.insert("goal_gate".to_string(), NodeAttrValue::Bool(true));

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

        // Should pass because the goal_gate node was visited
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.visited_nodes.contains(&"critical".to_string()));
    }

    // ---------------------------------------------------------------
    // Test 58: Unsatisfied goals with retry_target route back instead of error
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn unsatisfied_goals_retry_target_routes_back() {
        let mut goal_attrs = HashMap::new();
        goal_attrs.insert("goal_gate".to_string(), NodeAttrValue::Bool(true));
        goal_attrs.insert(
            "retry_target".to_string(),
            NodeAttrValue::String("middle".to_string()),
        );

        // Build a graph where:
        // start -> exit (first time, goal unmet, so retry_target on goal_node sends to middle)
        // middle -> goal_node -> exit (second time, goal met, so complete)
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
                make_node("middle", NodeType::Generic),
                make_node_with_attrs("goal_node", NodeType::Generic, goal_attrs),
            ],
            vec![
                make_edge("start", "exit"),
                make_edge("middle", "goal_node"),
                make_edge("goal_node", "exit"),
            ],
        );

        let engine = Engine::with_config(
            graph,
            success_registry(),
            EngineConfig {
                max_steps: 20,
                ..EngineConfig::default()
            },
        );
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        // Should succeed because retry_target on goal_node routed to "middle"
        assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
        let result = result.unwrap();
        assert!(result.visited_nodes.contains(&"goal_node".to_string()));
    }

    // ---------------------------------------------------------------
    // Test 59: Unsatisfied goals without retry_target still errors
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn unsatisfied_goals_without_retry_target_errors() {
        let mut goal_attrs = HashMap::new();
        goal_attrs.insert("goal_gate".to_string(), NodeAttrValue::Bool(true));

        // Exit node has no retry_target
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
    }

    // ---------------------------------------------------------------
    // Test 60: PipelineStats fields are populated correctly
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn pipeline_stats_populated_after_run() {
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("middle", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![make_edge("start", "middle"), make_edge("middle", "exit")],
        );
        let engine = Engine::new(graph, success_registry());
        let result = engine.run(Context::new()).await.unwrap();

        // Three nodes were executed.
        assert_eq!(result.stats.total_nodes_visited, 3);
        assert_eq!(result.stats.node_timings.len(), 3);

        // All nodes returned Success outcomes.
        use crate::stats::OutcomeKind;
        assert_eq!(
            result
                .stats
                .nodes_by_outcome
                .get(&OutcomeKind::Success)
                .copied()
                .unwrap_or(0),
            3
        );

        // top_slowest returns at most the available nodes.
        let slowest = result.stats.top_slowest(2);
        assert_eq!(slowest.len(), 2);

        // total_duration_ms is always set (may be 0 in fast tests, never panics).
        let _ = result.stats.total_duration_ms;
    }

    // ---------------------------------------------------------------
    // resolve_retry_target tests
    // ---------------------------------------------------------------

    #[test]
    fn resolve_retry_target_from_node_attr() {
        let mut attrs = HashMap::new();
        attrs.insert(
            "retry_target".to_string(),
            NodeAttrValue::String("recovery".to_string()),
        );
        let node = GraphNode {
            id: "g1".to_string(),
            node_type: NodeType::Codergen,
            label: None,
            attrs,
        };
        let graph = Graph {
            name: None,
            nodes: vec![],
            edges: vec![],
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
            graph_attrs: HashMap::new(),
        };
        assert_eq!(
            resolve_retry_target(&node, &graph),
            Some("recovery".to_string())
        );
    }

    #[test]
    fn resolve_retry_target_fallback_to_node_fallback() {
        let mut attrs = HashMap::new();
        attrs.insert(
            "fallback_retry_target".to_string(),
            NodeAttrValue::String("fb_target".to_string()),
        );
        let node = GraphNode {
            id: "g1".to_string(),
            node_type: NodeType::Codergen,
            label: None,
            attrs,
        };
        let graph = Graph {
            name: None,
            nodes: vec![],
            edges: vec![],
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
            graph_attrs: HashMap::new(),
        };
        assert_eq!(
            resolve_retry_target(&node, &graph),
            Some("fb_target".to_string())
        );
    }

    #[test]
    fn resolve_retry_target_fallback_to_graph_level() {
        let node = GraphNode {
            id: "g1".to_string(),
            node_type: NodeType::Codergen,
            label: None,
            attrs: HashMap::new(),
        };
        let mut graph_attrs = HashMap::new();
        graph_attrs.insert(
            "retry_target".to_string(),
            NodeAttrValue::String("graph_rt".to_string()),
        );
        let graph = Graph {
            name: None,
            nodes: vec![],
            edges: vec![],
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
            graph_attrs,
        };
        assert_eq!(
            resolve_retry_target(&node, &graph),
            Some("graph_rt".to_string())
        );
    }

    #[test]
    fn resolve_retry_target_fallback_to_graph_fallback() {
        let node = GraphNode {
            id: "g1".to_string(),
            node_type: NodeType::Codergen,
            label: None,
            attrs: HashMap::new(),
        };
        let mut graph_attrs = HashMap::new();
        graph_attrs.insert(
            "fallback_retry_target".to_string(),
            NodeAttrValue::String("graph_fb".to_string()),
        );
        let graph = Graph {
            name: None,
            nodes: vec![],
            edges: vec![],
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
            graph_attrs,
        };
        assert_eq!(
            resolve_retry_target(&node, &graph),
            Some("graph_fb".to_string())
        );
    }

    #[test]
    fn resolve_retry_target_none_when_empty() {
        let node = GraphNode {
            id: "g1".to_string(),
            node_type: NodeType::Codergen,
            label: None,
            attrs: HashMap::new(),
        };
        let graph = Graph {
            name: None,
            nodes: vec![],
            edges: vec![],
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
            graph_attrs: HashMap::new(),
        };
        assert_eq!(resolve_retry_target(&node, &graph), None);
    }

    #[test]
    fn resolve_retry_target_priority_order() {
        let mut attrs = HashMap::new();
        attrs.insert(
            "retry_target".to_string(),
            NodeAttrValue::String("node_rt".to_string()),
        );
        attrs.insert(
            "fallback_retry_target".to_string(),
            NodeAttrValue::String("node_fb".to_string()),
        );
        let node = GraphNode {
            id: "g1".to_string(),
            node_type: NodeType::Codergen,
            label: None,
            attrs,
        };
        let mut graph_attrs = HashMap::new();
        graph_attrs.insert(
            "retry_target".to_string(),
            NodeAttrValue::String("graph_rt".to_string()),
        );
        graph_attrs.insert(
            "fallback_retry_target".to_string(),
            NodeAttrValue::String("graph_fb".to_string()),
        );
        let graph = Graph {
            name: None,
            nodes: vec![],
            edges: vec![],
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
            graph_attrs,
        };
        // Node-level retry_target should win
        assert_eq!(
            resolve_retry_target(&node, &graph),
            Some("node_rt".to_string())
        );
    }

    // ---------------------------------------------------------------
    // Failure routing via retry_target fallback (spec 3.7)
    // ---------------------------------------------------------------

    /// Handler that fails on nodes whose id starts with "fail_" and succeeds otherwise.
    struct SelectiveFailHandler;

    #[async_trait]
    impl Handler for SelectiveFailHandler {
        fn name(&self) -> &str {
            "selective_fail"
        }
        async fn execute(
            &self,
            node: &GraphNode,
            _context: &Context,
        ) -> Result<Outcome, HandlerError> {
            if node.id.starts_with("fail_") {
                Ok(Outcome::failure("selective failure"))
            } else {
                Ok(Outcome::success())
            }
        }
        fn handles(&self, _node_type: &NodeType) -> bool {
            true
        }
    }

    fn selective_fail_registry() -> HandlerRegistry {
        let mut registry = HandlerRegistry::new();
        registry.register(Arc::new(SelectiveFailHandler));
        registry
    }

    #[tokio::test]
    async fn failure_routing_uses_node_retry_target() {
        // Graph: start -> fail_node (no fail edge, but has retry_target -> recovery -> exit)
        let mut fail_attrs = HashMap::new();
        fail_attrs.insert(
            "retry_target".to_string(),
            NodeAttrValue::String("recovery".to_string()),
        );
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node_with_attrs("fail_node", NodeType::Generic, fail_attrs),
                make_node("recovery", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "fail_node"),
                // No edge from fail_node — forces the None branch
                make_edge("recovery", "exit"),
            ],
        );
        let engine = Engine::with_config(
            graph,
            selective_fail_registry(),
            EngineConfig {
                max_steps: 20,
                ..EngineConfig::default()
            },
        );
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        // fail_node should fail, then route to recovery via retry_target
        assert!(result.visited_nodes.contains(&"fail_node".to_string()));
        assert!(
            result.visited_nodes.contains(&"recovery".to_string()),
            "expected engine to route to recovery via retry_target, visited: {:?}",
            result.visited_nodes
        );
        assert!(result.visited_nodes.contains(&"exit".to_string()));
    }

    #[tokio::test]
    async fn failure_routing_terminates_without_retry_target() {
        // Graph: start -> fail_node (no fail edge, no retry_target)
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("fail_node", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "fail_node"),
                // No edge from fail_node — forces the None branch, no retry_target either
            ],
        );
        let engine = Engine::new(graph, selective_fail_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        // Spec 3.7 step 4: node failed with no route — pipeline should fail.
        // Error should preserve the original failure reason from the handler.
        let err = result.unwrap_err();
        match &err {
            EngineError::UnroutableFailure { node_id, error } => {
                assert_eq!(node_id, "fail_node");
                assert!(
                    error.contains("selective failure"),
                    "error should preserve handler's failure reason, got: {error}"
                );
            }
            other => panic!("expected UnroutableFailure, got: {other}"),
        }
    }

    #[tokio::test]
    async fn success_with_no_outgoing_edge_still_terminates() {
        // Graph: start -> ok_node (succeeds, but has no outgoing edge)
        // ok_node has a retry_target attr, but it should NOT be used on success.
        let mut attrs = HashMap::new();
        attrs.insert(
            "retry_target".to_string(),
            NodeAttrValue::String("recovery".to_string()),
        );
        let graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node_with_attrs("ok_node", NodeType::Generic, attrs),
                make_node("recovery", NodeType::Generic),
                make_node("exit", NodeType::Exit),
            ],
            vec![
                make_edge("start", "ok_node"),
                // No edge from ok_node — forces the None branch
                make_edge("recovery", "exit"),
            ],
        );
        let engine = Engine::new(graph, success_registry());
        let ctx = Context::new();
        let result = engine.run(ctx).await.unwrap();

        // Engine should terminate after ok_node; retry_target is only for failures
        assert!(result.visited_nodes.contains(&"ok_node".to_string()));
        assert!(
            !result.visited_nodes.contains(&"recovery".to_string()),
            "recovery should not be visited when ok_node succeeds (retry_target is failure-only)"
        );
    }

    #[test]
    fn resolve_retry_target_skips_empty_strings() {
        let mut attrs = HashMap::new();
        attrs.insert(
            "retry_target".to_string(),
            NodeAttrValue::String("".to_string()),
        );
        attrs.insert(
            "fallback_retry_target".to_string(),
            NodeAttrValue::String("actual_target".to_string()),
        );
        let node = GraphNode {
            id: "g1".to_string(),
            node_type: NodeType::Codergen,
            label: None,
            attrs,
        };
        let graph = Graph {
            name: None,
            nodes: vec![],
            edges: vec![],
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
            graph_attrs: HashMap::new(),
        };
        assert_eq!(
            resolve_retry_target(&node, &graph),
            Some("actual_target".to_string())
        );
    }

    // ---------------------------------------------------------------
    // Test: graph-level retry_target used when goal node has no retry_target
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn graph_level_retry_target_used_for_unsatisfied_goal() {
        let mut goal_attrs = HashMap::new();
        goal_attrs.insert("goal_gate".to_string(), NodeAttrValue::Bool(true));
        // No retry_target on the goal node itself

        let mut graph = make_graph(
            vec![
                make_node("start", NodeType::Start),
                make_node("exit", NodeType::Exit),
                make_node("recovery", NodeType::Generic),
                make_node_with_attrs("goal_node", NodeType::Generic, goal_attrs),
            ],
            vec![
                make_edge("start", "exit"),
                make_edge("recovery", "goal_node"),
                make_edge("goal_node", "exit"),
            ],
        );
        // Set graph-level retry_target
        graph.graph_attrs.insert(
            "retry_target".to_string(),
            NodeAttrValue::String("recovery".to_string()),
        );

        let engine = Engine::with_config(
            graph,
            success_registry(),
            EngineConfig {
                max_steps: 20,
                ..EngineConfig::default()
            },
        );
        let ctx = Context::new();
        let result = engine.run(ctx).await;

        assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
        let result = result.unwrap();
        assert!(result.visited_nodes.contains(&"goal_node".to_string()));
    }
}
