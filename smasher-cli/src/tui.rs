// ABOUTME: TUI display models and formatters for pipeline execution visualization.
// ABOUTME: Provides data structures that translate PipelineEvents into renderable view state.

use chrono::{DateTime, Utc};
use serde::Serialize;
use smasher_attractor::events::PipelineEvent;

/// Status of an individual node in the pipeline view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl std::fmt::Display for NodeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeStatus::Pending => write!(f, "pending"),
            NodeStatus::Running => write!(f, "running"),
            NodeStatus::Completed => write!(f, "completed"),
            NodeStatus::Failed => write!(f, "failed"),
            NodeStatus::Skipped => write!(f, "skipped"),
        }
    }
}

/// Overall status of the pipeline execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineStatus {
    NotStarted,
    Running,
    Completed,
    Failed,
    Aborted,
}

impl std::fmt::Display for PipelineStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PipelineStatus::NotStarted => write!(f, "not started"),
            PipelineStatus::Running => write!(f, "running"),
            PipelineStatus::Completed => write!(f, "completed"),
            PipelineStatus::Failed => write!(f, "failed"),
            PipelineStatus::Aborted => write!(f, "aborted"),
        }
    }
}

/// Severity level for log lines displayed in the TUI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
    Debug,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Info => write!(f, "INFO"),
            LogLevel::Warning => write!(f, "WARN"),
            LogLevel::Error => write!(f, "ERROR"),
            LogLevel::Debug => write!(f, "DEBUG"),
        }
    }
}

/// A single timestamped log entry for the TUI log stream.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub message: String,
}

/// View model for a single node in the pipeline graph.
#[derive(Debug, Clone)]
pub struct NodeView {
    pub id: String,
    pub node_type: String,
    pub status: NodeStatus,
    pub duration_ms: Option<u64>,
}

/// Top-level view model representing the entire pipeline execution state.
///
/// This struct is the bridge between raw `PipelineEvent`s and the TUI rendering
/// layer. Call `apply_event` to incrementally update the view as events arrive.
#[derive(Debug)]
pub struct PipelineView {
    pub graph_name: String,
    pub nodes: Vec<NodeView>,
    pub current_node: Option<String>,
    pub status: PipelineStatus,
    pub elapsed_ms: u64,
    pub log_lines: Vec<LogLine>,
}

impl PipelineView {
    /// Create a view initialized with all nodes in Pending status.
    ///
    /// `node_ids` is a list of `(id, node_type)` pairs describing the graph topology.
    pub fn new(graph_name: String, node_ids: Vec<(String, String)>) -> Self {
        let nodes = node_ids
            .into_iter()
            .map(|(id, node_type)| NodeView {
                id,
                node_type,
                status: NodeStatus::Pending,
                duration_ms: None,
            })
            .collect();

        Self {
            graph_name,
            nodes,
            current_node: None,
            status: PipelineStatus::NotStarted,
            elapsed_ms: 0,
            log_lines: Vec::new(),
        }
    }

    /// Apply a pipeline event to update the view state.
    ///
    /// Maps each `PipelineEvent` variant to the corresponding view mutation:
    /// node status changes, pipeline status transitions, and log line generation.
    pub fn apply_event(&mut self, event: &PipelineEvent) {
        match event {
            PipelineEvent::PipelineStarted { timestamp, .. } => {
                self.status = PipelineStatus::Running;
                self.log_lines.push(LogLine {
                    timestamp: *timestamp,
                    level: LogLevel::Info,
                    message: format!("Pipeline '{}' started", self.graph_name),
                });
            }
            PipelineEvent::NodeStarted {
                node_id, timestamp, ..
            } => {
                self.current_node = Some(node_id.clone());
                if let Some(node) = self.nodes.iter_mut().find(|n| n.id == *node_id) {
                    node.status = NodeStatus::Running;
                }
                self.log_lines.push(LogLine {
                    timestamp: *timestamp,
                    level: LogLevel::Info,
                    message: format!("Node '{}' started", node_id),
                });
            }
            PipelineEvent::NodeCompleted {
                node_id,
                duration_ms,
                timestamp,
                ..
            } => {
                if let Some(node) = self.nodes.iter_mut().find(|n| n.id == *node_id) {
                    node.status = NodeStatus::Completed;
                    node.duration_ms = Some(*duration_ms);
                }
                if self.current_node.as_deref() == Some(node_id) {
                    self.current_node = None;
                }
                self.log_lines.push(LogLine {
                    timestamp: *timestamp,
                    level: LogLevel::Info,
                    message: format!("Node '{}' completed in {}ms", node_id, duration_ms),
                });
            }
            PipelineEvent::NodeFailed {
                node_id,
                error,
                duration_ms,
                timestamp,
            } => {
                if let Some(node) = self.nodes.iter_mut().find(|n| n.id == *node_id) {
                    node.status = NodeStatus::Failed;
                    node.duration_ms = Some(*duration_ms);
                }
                if self.current_node.as_deref() == Some(node_id) {
                    self.current_node = None;
                }
                self.status = PipelineStatus::Failed;
                self.log_lines.push(LogLine {
                    timestamp: *timestamp,
                    level: LogLevel::Error,
                    message: format!("Node '{}' failed: {}", node_id, error),
                });
            }
            PipelineEvent::PipelineCompleted {
                duration_ms,
                timestamp,
                ..
            } => {
                // Only transition to Completed if we haven't already marked it Failed.
                if self.status != PipelineStatus::Failed {
                    self.status = PipelineStatus::Completed;
                }
                self.elapsed_ms = *duration_ms;
                self.current_node = None;
                self.log_lines.push(LogLine {
                    timestamp: *timestamp,
                    level: LogLevel::Info,
                    message: format!(
                        "Pipeline completed in {}ms ({}/{} nodes)",
                        duration_ms,
                        self.completed_count(),
                        self.total_count()
                    ),
                });
            }
            PipelineEvent::PipelineAborted {
                reason, timestamp, ..
            } => {
                self.status = PipelineStatus::Aborted;
                self.current_node = None;
                self.log_lines.push(LogLine {
                    timestamp: *timestamp,
                    level: LogLevel::Error,
                    message: format!("Pipeline aborted: {}", reason),
                });
            }
            PipelineEvent::EdgeTraversed {
                from,
                to,
                timestamp,
                ..
            } => {
                self.log_lines.push(LogLine {
                    timestamp: *timestamp,
                    level: LogLevel::Debug,
                    message: format!("Edge traversed: {} -> {}", from, to),
                });
            }
            PipelineEvent::HumanPromptIssued {
                node_id,
                question,
                timestamp,
            } => {
                self.log_lines.push(LogLine {
                    timestamp: *timestamp,
                    level: LogLevel::Warning,
                    message: format!("Human gate at '{}': {}", node_id, question),
                });
            }
            PipelineEvent::HumanResponseReceived {
                node_id,
                response,
                timestamp,
            } => {
                self.log_lines.push(LogLine {
                    timestamp: *timestamp,
                    level: LogLevel::Info,
                    message: format!("Human response at '{}': {}", node_id, response),
                });
            }
            PipelineEvent::ContextUpdated { key, timestamp } => {
                self.log_lines.push(LogLine {
                    timestamp: *timestamp,
                    level: LogLevel::Debug,
                    message: format!("Context updated: key '{}'", key),
                });
            }
            PipelineEvent::CheckpointCreated {
                node_id, timestamp, ..
            } => {
                self.log_lines.push(LogLine {
                    timestamp: *timestamp,
                    level: LogLevel::Debug,
                    message: format!("Checkpoint created at '{}'", node_id),
                });
            }
            PipelineEvent::LoopRestarted {
                from,
                to,
                restart_count,
                timestamp,
            } => {
                self.log_lines.push(LogLine {
                    timestamp: *timestamp,
                    level: LogLevel::Info,
                    message: format!(
                        "Loop restarted: {} -> {} (attempt {})",
                        from, to, restart_count
                    ),
                });
            }
        }
    }

    /// Format a one-line status summary suitable for a terminal status bar.
    ///
    /// Examples:
    /// - `"Not started: my_pipeline (0/5 nodes)"`
    /// - `"Running: node_3 (4/7 nodes, 1.2s)"`
    /// - `"Completed: 7/7 nodes in 3.4s"`
    /// - `"Failed at node_5 (4/7 nodes, 2.1s)"`
    pub fn format_status_line(&self) -> String {
        let completed = self.completed_count();
        let total = self.total_count();

        match &self.status {
            PipelineStatus::NotStarted => {
                format!("Not started: {} (0/{} nodes)", self.graph_name, total)
            }
            PipelineStatus::Running => {
                let node_info = self.current_node.as_deref().unwrap_or("waiting");
                let elapsed_secs = self.elapsed_ms as f64 / 1000.0;
                if self.elapsed_ms > 0 {
                    format!(
                        "Running: {} ({}/{} nodes, {:.1}s)",
                        node_info, completed, total, elapsed_secs
                    )
                } else {
                    format!("Running: {} ({}/{} nodes)", node_info, completed, total)
                }
            }
            PipelineStatus::Completed => {
                let elapsed_secs = self.elapsed_ms as f64 / 1000.0;
                format!(
                    "Completed: {}/{} nodes in {:.1}s",
                    completed, total, elapsed_secs
                )
            }
            PipelineStatus::Failed => {
                let elapsed_secs = self.elapsed_ms as f64 / 1000.0;
                let failed_node = self
                    .nodes
                    .iter()
                    .find(|n| n.status == NodeStatus::Failed)
                    .map(|n| n.id.as_str())
                    .unwrap_or("unknown");
                if self.elapsed_ms > 0 {
                    format!(
                        "Failed at {} ({}/{} nodes, {:.1}s)",
                        failed_node, completed, total, elapsed_secs
                    )
                } else {
                    format!("Failed at {} ({}/{} nodes)", failed_node, completed, total)
                }
            }
            PipelineStatus::Aborted => {
                let elapsed_secs = self.elapsed_ms as f64 / 1000.0;
                if self.elapsed_ms > 0 {
                    format!(
                        "Aborted ({}/{} nodes, {:.1}s)",
                        completed, total, elapsed_secs
                    )
                } else {
                    format!("Aborted ({}/{} nodes)", completed, total)
                }
            }
        }
    }

    /// Count how many nodes have reached Completed status.
    pub fn completed_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.status == NodeStatus::Completed)
            .count()
    }

    /// Return the total number of nodes in the pipeline.
    pub fn total_count(&self) -> usize {
        self.nodes.len()
    }
}

/// Drives the TUI display by processing pipeline events and tracking execution state.
///
/// Wraps a `PipelineView` with event counting and timestamp tracking so that
/// callers have a single entry point for feeding events and querying display state.
pub struct TuiRunner {
    pub view: PipelineView,
    pub event_count: usize,
    pub last_update: Option<DateTime<Utc>>,
}

impl TuiRunner {
    /// Create a runner with an initialized view containing the given nodes.
    pub fn new(graph_name: String, node_ids: Vec<(String, String)>) -> Self {
        Self {
            view: PipelineView::new(graph_name, node_ids),
            event_count: 0,
            last_update: None,
        }
    }

    /// Apply a pipeline event to the view, increment the event counter, and record the timestamp.
    pub fn process_event(&mut self, event: &PipelineEvent) {
        self.view.apply_event(event);
        self.event_count += 1;
        self.last_update = Some(event.timestamp());
    }

    /// Delegate to the view's compact status-line formatter.
    pub fn render_line(&self) -> String {
        self.view.format_status_line()
    }

    /// Return `true` when the pipeline has reached a terminal state.
    pub fn is_complete(&self) -> bool {
        matches!(
            self.view.status,
            PipelineStatus::Completed | PipelineStatus::Failed | PipelineStatus::Aborted
        )
    }

    /// Return the number of events processed so far.
    pub fn event_count(&self) -> usize {
        self.event_count
    }

    /// Borrow the underlying view for inspection.
    pub fn view(&self) -> &PipelineView {
        &self.view
    }
}

/// View model for a rendered graph image within the TUI.
///
/// Holds the most recently rendered graph output bytes along with metadata
/// about when it was rendered and what format it is in. The TUI layer can
/// use this to decide whether to refresh the render or display the cached image.
#[derive(Debug, Clone)]
pub struct GraphView {
    /// The rendered content bytes (DOT text, SVG text, or PNG binary).
    pub content: Vec<u8>,
    /// The format of the rendered content.
    pub format: smasher_attractor::rendering::RenderFormat,
    /// Timestamp when this render was produced.
    pub rendered_at: DateTime<Utc>,
    /// Whether the render is stale because the pipeline state has changed
    /// since the last render.
    pub stale: bool,
}

impl GraphView {
    /// Create a new graph view from a render output.
    pub fn from_render_output(
        output: smasher_attractor::rendering::RenderOutput,
        rendered_at: DateTime<Utc>,
    ) -> Self {
        Self {
            content: output.content,
            format: output.format,
            rendered_at,
            stale: false,
        }
    }

    /// Mark this view as stale, indicating it should be re-rendered.
    pub fn mark_stale(&mut self) {
        self.stale = true;
    }

    /// Mark this view as fresh after a re-render.
    pub fn mark_fresh(&mut self, content: Vec<u8>, rendered_at: DateTime<Utc>) {
        self.content = content;
        self.rendered_at = rendered_at;
        self.stale = false;
    }

    /// Return the rendered content as UTF-8 text, if applicable.
    pub fn as_text(&self) -> Option<&str> {
        std::str::from_utf8(&self.content).ok()
    }

    /// Return the content byte length.
    pub fn content_len(&self) -> usize {
        self.content.len()
    }
}

/// Output format selection for pipeline event display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayFormat {
    /// One-line status updates suitable for overwriting the same terminal line.
    Compact,
    /// Full event log with timestamps, one line per event.
    Verbose,
    /// Raw JSON event stream, one JSON object per line.
    Json,
    /// No output at all.
    Silent,
}

/// Format a pipeline event according to the chosen display format.
///
/// Returns `None` for `Silent` mode and `Some(line)` for all others.
pub fn format_event(event: &PipelineEvent, format: &DisplayFormat) -> Option<String> {
    match format {
        DisplayFormat::Compact => {
            // Produce a concise description of the event itself
            let description = match event {
                PipelineEvent::PipelineStarted { graph_name, .. } => {
                    format!("Pipeline '{}' started", graph_name)
                }
                PipelineEvent::NodeStarted {
                    node_id, node_type, ..
                } => {
                    format!("Node '{}' ({}) started", node_id, node_type)
                }
                PipelineEvent::NodeCompleted {
                    node_id,
                    duration_ms,
                    ..
                } => {
                    format!("Node '{}' completed in {}ms", node_id, duration_ms)
                }
                PipelineEvent::NodeFailed { node_id, error, .. } => {
                    format!("Node '{}' failed: {}", node_id, error)
                }
                PipelineEvent::PipelineCompleted { duration_ms, .. } => {
                    format!("Pipeline completed in {}ms", duration_ms)
                }
                PipelineEvent::PipelineAborted { reason, .. } => {
                    format!("Pipeline aborted: {}", reason)
                }
                PipelineEvent::EdgeTraversed { from, to, .. } => {
                    format!("Edge: {} -> {}", from, to)
                }
                PipelineEvent::HumanPromptIssued {
                    node_id, question, ..
                } => {
                    format!("Human gate at '{}': {}", node_id, question)
                }
                PipelineEvent::HumanResponseReceived {
                    node_id, response, ..
                } => {
                    format!("Human response at '{}': {}", node_id, response)
                }
                PipelineEvent::ContextUpdated { key, .. } => {
                    format!("Context updated: '{}'", key)
                }
                PipelineEvent::CheckpointCreated { node_id, .. } => {
                    format!("Checkpoint at '{}'", node_id)
                }
                PipelineEvent::LoopRestarted {
                    from,
                    to,
                    restart_count,
                    ..
                } => {
                    format!("Loop: {} -> {} (attempt {})", from, to, restart_count)
                }
            };
            Some(description)
        }
        DisplayFormat::Verbose => {
            let ts = event.timestamp().format("%Y-%m-%dT%H:%M:%S%.3fZ");
            let kind = match event {
                PipelineEvent::PipelineStarted { .. } => "pipeline_started",
                PipelineEvent::NodeStarted { .. } => "node_started",
                PipelineEvent::NodeCompleted { .. } => "node_completed",
                PipelineEvent::NodeFailed { .. } => "node_failed",
                PipelineEvent::PipelineCompleted { .. } => "pipeline_completed",
                PipelineEvent::PipelineAborted { .. } => "pipeline_aborted",
                PipelineEvent::EdgeTraversed { .. } => "edge_traversed",
                PipelineEvent::HumanPromptIssued { .. } => "human_prompt_issued",
                PipelineEvent::HumanResponseReceived { .. } => "human_response_received",
                PipelineEvent::ContextUpdated { .. } => "context_updated",
                PipelineEvent::CheckpointCreated { .. } => "checkpoint_created",
                PipelineEvent::LoopRestarted { .. } => "loop_restarted",
            };
            let details = match event {
                PipelineEvent::PipelineStarted { graph_name, .. } => {
                    format!("graph={}", graph_name)
                }
                PipelineEvent::NodeStarted {
                    node_id, node_type, ..
                } => {
                    format!("node={} type={}", node_id, node_type)
                }
                PipelineEvent::NodeCompleted {
                    node_id,
                    duration_ms,
                    ..
                } => {
                    format!("node={} duration={}ms", node_id, duration_ms)
                }
                PipelineEvent::NodeFailed {
                    node_id,
                    error,
                    duration_ms,
                    ..
                } => {
                    format!(
                        "node={} error=\"{}\" duration={}ms",
                        node_id, error, duration_ms
                    )
                }
                PipelineEvent::PipelineCompleted {
                    total_nodes,
                    duration_ms,
                    ..
                } => {
                    format!("nodes={} duration={}ms", total_nodes, duration_ms)
                }
                PipelineEvent::PipelineAborted { reason, .. } => {
                    format!("reason=\"{}\"", reason)
                }
                PipelineEvent::EdgeTraversed {
                    from, to, label, ..
                } => {
                    let label_str = label.as_deref().unwrap_or("(none)");
                    format!("from={} to={} label={}", from, to, label_str)
                }
                PipelineEvent::HumanPromptIssued {
                    node_id, question, ..
                } => {
                    format!("node={} question=\"{}\"", node_id, question)
                }
                PipelineEvent::HumanResponseReceived {
                    node_id, response, ..
                } => {
                    format!("node={} response=\"{}\"", node_id, response)
                }
                PipelineEvent::ContextUpdated { key, .. } => {
                    format!("key={}", key)
                }
                PipelineEvent::CheckpointCreated { node_id, .. } => {
                    format!("node={}", node_id)
                }
                PipelineEvent::LoopRestarted {
                    from,
                    to,
                    restart_count,
                    ..
                } => {
                    format!("from={} to={} attempt={}", from, to, restart_count)
                }
            };
            Some(format!("[{}] {}: {}", ts, kind, details))
        }
        DisplayFormat::Json => {
            // PipelineEvent derives Serialize, so we can serialize directly
            let json = serde_json::to_string(event)
                .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {}\"}}", e));
            Some(json)
        }
        DisplayFormat::Silent => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use smasher_attractor::state::Outcome;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    fn sample_nodes() -> Vec<(String, String)> {
        vec![
            ("fetch".into(), "http".into()),
            ("summarize".into(), "llm".into()),
            ("validate".into(), "tool".into()),
            ("review".into(), "human".into()),
            ("publish".into(), "http".into()),
        ]
    }

    // ---------------------------------------------------------------
    // PipelineView::new
    // ---------------------------------------------------------------

    #[test]
    fn new_initializes_all_nodes_as_pending() {
        let view = PipelineView::new("test_graph".into(), sample_nodes());
        assert_eq!(view.graph_name, "test_graph");
        assert_eq!(view.nodes.len(), 5);
        assert_eq!(view.status, PipelineStatus::NotStarted);
        assert!(view.current_node.is_none());
        assert_eq!(view.elapsed_ms, 0);
        assert!(view.log_lines.is_empty());

        for node in &view.nodes {
            assert_eq!(node.status, NodeStatus::Pending);
            assert!(node.duration_ms.is_none());
        }
    }

    #[test]
    fn new_preserves_node_types() {
        let view = PipelineView::new("g".into(), sample_nodes());
        assert_eq!(view.nodes[0].id, "fetch");
        assert_eq!(view.nodes[0].node_type, "http");
        assert_eq!(view.nodes[1].id, "summarize");
        assert_eq!(view.nodes[1].node_type, "llm");
    }

    #[test]
    fn new_with_empty_nodes() {
        let view = PipelineView::new("empty".into(), vec![]);
        assert_eq!(view.total_count(), 0);
        assert_eq!(view.completed_count(), 0);
    }

    // ---------------------------------------------------------------
    // apply_event: PipelineStarted
    // ---------------------------------------------------------------

    #[test]
    fn apply_pipeline_started_sets_running() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });

        assert_eq!(view.status, PipelineStatus::Running);
        assert_eq!(view.log_lines.len(), 1);
        assert_eq!(view.log_lines[0].level, LogLevel::Info);
        assert!(view.log_lines[0].message.contains("started"));
    }

    // ---------------------------------------------------------------
    // apply_event: NodeStarted
    // ---------------------------------------------------------------

    #[test]
    fn apply_node_started_sets_running_and_current() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::NodeStarted {
            node_id: "fetch".into(),
            node_type: "http".into(),
            timestamp: now(),
        });

        assert_eq!(view.current_node, Some("fetch".to_string()));
        assert_eq!(view.nodes[0].status, NodeStatus::Running);
        // Other nodes remain pending
        assert_eq!(view.nodes[1].status, NodeStatus::Pending);
        assert_eq!(view.log_lines.len(), 1);
    }

    // ---------------------------------------------------------------
    // apply_event: NodeCompleted
    // ---------------------------------------------------------------

    #[test]
    fn apply_node_completed_updates_status_and_duration() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::NodeStarted {
            node_id: "fetch".into(),
            node_type: "http".into(),
            timestamp: now(),
        });

        view.apply_event(&PipelineEvent::NodeCompleted {
            node_id: "fetch".into(),
            outcome: Outcome::success(),
            duration_ms: 250,
            timestamp: now(),
        });

        assert_eq!(view.nodes[0].status, NodeStatus::Completed);
        assert_eq!(view.nodes[0].duration_ms, Some(250));
        assert!(view.current_node.is_none());
        assert_eq!(view.completed_count(), 1);
    }

    // ---------------------------------------------------------------
    // apply_event: NodeFailed
    // ---------------------------------------------------------------

    #[test]
    fn apply_node_failed_sets_failed_status() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });

        view.apply_event(&PipelineEvent::NodeStarted {
            node_id: "validate".into(),
            node_type: "tool".into(),
            timestamp: now(),
        });

        view.apply_event(&PipelineEvent::NodeFailed {
            node_id: "validate".into(),
            error: "schema mismatch".into(),
            duration_ms: 100,
            timestamp: now(),
        });

        assert_eq!(view.nodes[2].status, NodeStatus::Failed);
        assert_eq!(view.nodes[2].duration_ms, Some(100));
        assert!(view.current_node.is_none());
        assert_eq!(view.status, PipelineStatus::Failed);

        // The error log line should be present
        let error_lines: Vec<_> = view
            .log_lines
            .iter()
            .filter(|l| l.level == LogLevel::Error)
            .collect();
        assert_eq!(error_lines.len(), 1);
        assert!(error_lines[0].message.contains("schema mismatch"));
    }

    // ---------------------------------------------------------------
    // apply_event: PipelineCompleted
    // ---------------------------------------------------------------

    #[test]
    fn apply_pipeline_completed_sets_elapsed_and_status() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });

        view.apply_event(&PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 5,
            duration_ms: 3400,
            timestamp: now(),
        });

        assert_eq!(view.status, PipelineStatus::Completed);
        assert_eq!(view.elapsed_ms, 3400);
        assert!(view.current_node.is_none());
    }

    #[test]
    fn pipeline_completed_after_failure_stays_failed() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });

        // A node fails first
        view.apply_event(&PipelineEvent::NodeFailed {
            node_id: "fetch".into(),
            error: "timeout".into(),
            duration_ms: 5000,
            timestamp: now(),
        });

        // Pipeline still completes (with failure outcome)
        view.apply_event(&PipelineEvent::PipelineCompleted {
            outcome: Outcome::failure("node failed"),
            total_nodes: 5,
            duration_ms: 5100,
            timestamp: now(),
        });

        // Status should stay Failed, not transition to Completed
        assert_eq!(view.status, PipelineStatus::Failed);
        assert_eq!(view.elapsed_ms, 5100);
    }

    // ---------------------------------------------------------------
    // apply_event: PipelineAborted
    // ---------------------------------------------------------------

    #[test]
    fn apply_pipeline_aborted() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });

        view.apply_event(&PipelineEvent::PipelineAborted {
            reason: "user cancelled".into(),
            timestamp: now(),
        });

        assert_eq!(view.status, PipelineStatus::Aborted);
        assert!(view.current_node.is_none());

        let error_lines: Vec<_> = view
            .log_lines
            .iter()
            .filter(|l| l.level == LogLevel::Error)
            .collect();
        assert_eq!(error_lines.len(), 1);
        assert!(error_lines[0].message.contains("user cancelled"));
    }

    // ---------------------------------------------------------------
    // apply_event: EdgeTraversed
    // ---------------------------------------------------------------

    #[test]
    fn apply_edge_traversed_adds_debug_log() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::EdgeTraversed {
            from: "fetch".into(),
            to: "summarize".into(),
            label: Some("success".into()),
            timestamp: now(),
        });

        assert_eq!(view.log_lines.len(), 1);
        assert_eq!(view.log_lines[0].level, LogLevel::Debug);
        assert!(view.log_lines[0].message.contains("fetch"));
        assert!(view.log_lines[0].message.contains("summarize"));
    }

    // ---------------------------------------------------------------
    // apply_event: HumanPromptIssued
    // ---------------------------------------------------------------

    #[test]
    fn apply_human_prompt_issued_adds_warning_log() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::HumanPromptIssued {
            node_id: "review".into(),
            question: "Approve deployment?".into(),
            timestamp: now(),
        });

        assert_eq!(view.log_lines.len(), 1);
        assert_eq!(view.log_lines[0].level, LogLevel::Warning);
        assert!(view.log_lines[0].message.contains("Approve deployment?"));
        assert!(view.log_lines[0].message.contains("review"));
    }

    // ---------------------------------------------------------------
    // apply_event: HumanResponseReceived
    // ---------------------------------------------------------------

    #[test]
    fn apply_human_response_received_adds_info_log() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::HumanResponseReceived {
            node_id: "review".into(),
            response: "approved".into(),
            timestamp: now(),
        });

        assert_eq!(view.log_lines.len(), 1);
        assert_eq!(view.log_lines[0].level, LogLevel::Info);
        assert!(view.log_lines[0].message.contains("approved"));
    }

    // ---------------------------------------------------------------
    // apply_event: ContextUpdated
    // ---------------------------------------------------------------

    #[test]
    fn apply_context_updated_adds_debug_log() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::ContextUpdated {
            key: "output_summary".into(),
            timestamp: now(),
        });

        assert_eq!(view.log_lines.len(), 1);
        assert_eq!(view.log_lines[0].level, LogLevel::Debug);
        assert!(view.log_lines[0].message.contains("output_summary"));
    }

    // ---------------------------------------------------------------
    // apply_event: CheckpointCreated
    // ---------------------------------------------------------------

    #[test]
    fn apply_checkpoint_created_adds_debug_log() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::CheckpointCreated {
            node_id: "summarize".into(),
            timestamp: now(),
        });

        assert_eq!(view.log_lines.len(), 1);
        assert_eq!(view.log_lines[0].level, LogLevel::Debug);
        assert!(view.log_lines[0].message.contains("summarize"));
    }

    // ---------------------------------------------------------------
    // apply_event: LoopRestarted
    // ---------------------------------------------------------------

    #[test]
    fn apply_loop_restarted_adds_info_log() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::LoopRestarted {
            from: "validate".into(),
            to: "summarize".into(),
            restart_count: 2,
            timestamp: now(),
        });

        assert_eq!(view.log_lines.len(), 1);
        assert_eq!(view.log_lines[0].level, LogLevel::Info);
        assert!(view.log_lines[0].message.contains("attempt 2"));
        assert!(view.log_lines[0].message.contains("validate"));
        assert!(view.log_lines[0].message.contains("summarize"));
    }

    // ---------------------------------------------------------------
    // completed_count and total_count
    // ---------------------------------------------------------------

    #[test]
    fn completed_count_starts_at_zero() {
        let view = PipelineView::new("g".into(), sample_nodes());
        assert_eq!(view.completed_count(), 0);
    }

    #[test]
    fn completed_count_tracks_completed_nodes() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::NodeCompleted {
            node_id: "fetch".into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: now(),
        });
        assert_eq!(view.completed_count(), 1);

        view.apply_event(&PipelineEvent::NodeCompleted {
            node_id: "summarize".into(),
            outcome: Outcome::success(),
            duration_ms: 200,
            timestamp: now(),
        });
        assert_eq!(view.completed_count(), 2);
    }

    #[test]
    fn total_count_matches_initial_nodes() {
        let view = PipelineView::new("g".into(), sample_nodes());
        assert_eq!(view.total_count(), 5);
    }

    #[test]
    fn failed_nodes_do_not_count_as_completed() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::NodeCompleted {
            node_id: "fetch".into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: now(),
        });
        view.apply_event(&PipelineEvent::NodeFailed {
            node_id: "summarize".into(),
            error: "crash".into(),
            duration_ms: 50,
            timestamp: now(),
        });

        assert_eq!(view.completed_count(), 1);
    }

    // ---------------------------------------------------------------
    // format_status_line
    // ---------------------------------------------------------------

    #[test]
    fn format_status_line_not_started() {
        let view = PipelineView::new("my_pipeline".into(), sample_nodes());
        let line = view.format_status_line();
        assert_eq!(line, "Not started: my_pipeline (0/5 nodes)");
    }

    #[test]
    fn format_status_line_running_with_current_node() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });
        view.apply_event(&PipelineEvent::NodeCompleted {
            node_id: "fetch".into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: now(),
        });
        view.apply_event(&PipelineEvent::NodeStarted {
            node_id: "summarize".into(),
            node_type: "llm".into(),
            timestamp: now(),
        });

        let line = view.format_status_line();
        assert_eq!(line, "Running: summarize (1/5 nodes)");
    }

    #[test]
    fn format_status_line_running_with_elapsed() {
        let mut view = PipelineView::new("g".into(), sample_nodes());
        view.status = PipelineStatus::Running;
        view.current_node = Some("node_3".into());
        view.elapsed_ms = 1200;

        // Manually set a node as completed for count
        view.nodes[0].status = NodeStatus::Completed;
        view.nodes[1].status = NodeStatus::Completed;
        view.nodes[2].status = NodeStatus::Completed;
        view.nodes[3].status = NodeStatus::Completed;

        let line = view.format_status_line();
        assert_eq!(line, "Running: node_3 (4/5 nodes, 1.2s)");
    }

    #[test]
    fn format_status_line_completed() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        // Complete all nodes
        for node in &mut view.nodes {
            node.status = NodeStatus::Completed;
        }
        view.status = PipelineStatus::Completed;
        view.elapsed_ms = 3400;

        let line = view.format_status_line();
        assert_eq!(line, "Completed: 5/5 nodes in 3.4s");
    }

    #[test]
    fn format_status_line_failed() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.nodes[0].status = NodeStatus::Completed;
        view.nodes[1].status = NodeStatus::Completed;
        view.nodes[2].status = NodeStatus::Failed;
        view.status = PipelineStatus::Failed;
        view.elapsed_ms = 2100;

        let line = view.format_status_line();
        assert_eq!(line, "Failed at validate (2/5 nodes, 2.1s)");
    }

    #[test]
    fn format_status_line_aborted() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.nodes[0].status = NodeStatus::Completed;
        view.status = PipelineStatus::Aborted;
        view.elapsed_ms = 500;

        let line = view.format_status_line();
        assert_eq!(line, "Aborted (1/5 nodes, 0.5s)");
    }

    #[test]
    fn format_status_line_aborted_no_elapsed() {
        let mut view = PipelineView::new("g".into(), sample_nodes());
        view.status = PipelineStatus::Aborted;

        let line = view.format_status_line();
        assert_eq!(line, "Aborted (0/5 nodes)");
    }

    // ---------------------------------------------------------------
    // NodeStatus transitions through events
    // ---------------------------------------------------------------

    #[test]
    fn node_transitions_pending_to_running_to_completed() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        assert_eq!(view.nodes[0].status, NodeStatus::Pending);

        view.apply_event(&PipelineEvent::NodeStarted {
            node_id: "fetch".into(),
            node_type: "http".into(),
            timestamp: now(),
        });
        assert_eq!(view.nodes[0].status, NodeStatus::Running);

        view.apply_event(&PipelineEvent::NodeCompleted {
            node_id: "fetch".into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: now(),
        });
        assert_eq!(view.nodes[0].status, NodeStatus::Completed);
    }

    #[test]
    fn node_transitions_pending_to_running_to_failed() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::NodeStarted {
            node_id: "fetch".into(),
            node_type: "http".into(),
            timestamp: now(),
        });
        assert_eq!(view.nodes[0].status, NodeStatus::Running);

        view.apply_event(&PipelineEvent::NodeFailed {
            node_id: "fetch".into(),
            error: "connection refused".into(),
            duration_ms: 50,
            timestamp: now(),
        });
        assert_eq!(view.nodes[0].status, NodeStatus::Failed);
    }

    // ---------------------------------------------------------------
    // LogLine from events
    // ---------------------------------------------------------------

    #[test]
    fn log_lines_accumulate_in_order() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });
        view.apply_event(&PipelineEvent::NodeStarted {
            node_id: "fetch".into(),
            node_type: "http".into(),
            timestamp: now(),
        });
        view.apply_event(&PipelineEvent::NodeCompleted {
            node_id: "fetch".into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: now(),
        });

        assert_eq!(view.log_lines.len(), 3);
        assert_eq!(view.log_lines[0].level, LogLevel::Info);
        assert_eq!(view.log_lines[1].level, LogLevel::Info);
        assert_eq!(view.log_lines[2].level, LogLevel::Info);
    }

    #[test]
    fn log_lines_have_timestamps() {
        let mut view = PipelineView::new("g".into(), sample_nodes());
        let ts = now();

        view.apply_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: ts,
        });

        assert_eq!(view.log_lines[0].timestamp, ts);
    }

    #[test]
    fn log_levels_correct_for_different_events() {
        let mut view = PipelineView::new("g".into(), sample_nodes());
        let ts = now();

        // Info: PipelineStarted
        view.apply_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: ts,
        });

        // Debug: EdgeTraversed
        view.apply_event(&PipelineEvent::EdgeTraversed {
            from: "a".into(),
            to: "b".into(),
            label: None,
            timestamp: ts,
        });

        // Warning: HumanPromptIssued
        view.apply_event(&PipelineEvent::HumanPromptIssued {
            node_id: "review".into(),
            question: "ok?".into(),
            timestamp: ts,
        });

        // Error: NodeFailed
        view.apply_event(&PipelineEvent::NodeFailed {
            node_id: "fetch".into(),
            error: "boom".into(),
            duration_ms: 10,
            timestamp: ts,
        });

        assert_eq!(view.log_lines[0].level, LogLevel::Info);
        assert_eq!(view.log_lines[1].level, LogLevel::Debug);
        assert_eq!(view.log_lines[2].level, LogLevel::Warning);
        assert_eq!(view.log_lines[3].level, LogLevel::Error);
    }

    // ---------------------------------------------------------------
    // Full pipeline lifecycle
    // ---------------------------------------------------------------

    #[test]
    fn full_successful_pipeline_lifecycle() {
        let nodes = vec![
            ("step1".into(), "llm".into()),
            ("step2".into(), "tool".into()),
            ("step3".into(), "llm".into()),
        ];
        let mut view = PipelineView::new("workflow".into(), nodes);
        let ts = now();

        // Start pipeline
        view.apply_event(&PipelineEvent::PipelineStarted {
            graph_name: "workflow".into(),
            timestamp: ts,
        });
        assert_eq!(view.status, PipelineStatus::Running);

        // Execute step1
        view.apply_event(&PipelineEvent::NodeStarted {
            node_id: "step1".into(),
            node_type: "llm".into(),
            timestamp: ts,
        });
        view.apply_event(&PipelineEvent::NodeCompleted {
            node_id: "step1".into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: ts,
        });
        view.apply_event(&PipelineEvent::EdgeTraversed {
            from: "step1".into(),
            to: "step2".into(),
            label: None,
            timestamp: ts,
        });

        // Execute step2
        view.apply_event(&PipelineEvent::NodeStarted {
            node_id: "step2".into(),
            node_type: "tool".into(),
            timestamp: ts,
        });
        view.apply_event(&PipelineEvent::NodeCompleted {
            node_id: "step2".into(),
            outcome: Outcome::success(),
            duration_ms: 200,
            timestamp: ts,
        });
        view.apply_event(&PipelineEvent::EdgeTraversed {
            from: "step2".into(),
            to: "step3".into(),
            label: None,
            timestamp: ts,
        });

        // Execute step3
        view.apply_event(&PipelineEvent::NodeStarted {
            node_id: "step3".into(),
            node_type: "llm".into(),
            timestamp: ts,
        });
        view.apply_event(&PipelineEvent::NodeCompleted {
            node_id: "step3".into(),
            outcome: Outcome::success(),
            duration_ms: 300,
            timestamp: ts,
        });

        // Pipeline completed
        view.apply_event(&PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 3,
            duration_ms: 650,
            timestamp: ts,
        });

        assert_eq!(view.status, PipelineStatus::Completed);
        assert_eq!(view.completed_count(), 3);
        assert_eq!(view.total_count(), 3);
        assert_eq!(view.elapsed_ms, 650);
        assert!(view.current_node.is_none());
        assert_eq!(view.format_status_line(), "Completed: 3/3 nodes in 0.7s");
    }

    // ---------------------------------------------------------------
    // Display impls
    // ---------------------------------------------------------------

    #[test]
    fn node_status_display() {
        assert_eq!(NodeStatus::Pending.to_string(), "pending");
        assert_eq!(NodeStatus::Running.to_string(), "running");
        assert_eq!(NodeStatus::Completed.to_string(), "completed");
        assert_eq!(NodeStatus::Failed.to_string(), "failed");
        assert_eq!(NodeStatus::Skipped.to_string(), "skipped");
    }

    #[test]
    fn pipeline_status_display() {
        assert_eq!(PipelineStatus::NotStarted.to_string(), "not started");
        assert_eq!(PipelineStatus::Running.to_string(), "running");
        assert_eq!(PipelineStatus::Completed.to_string(), "completed");
        assert_eq!(PipelineStatus::Failed.to_string(), "failed");
        assert_eq!(PipelineStatus::Aborted.to_string(), "aborted");
    }

    #[test]
    fn log_level_display() {
        assert_eq!(LogLevel::Info.to_string(), "INFO");
        assert_eq!(LogLevel::Warning.to_string(), "WARN");
        assert_eq!(LogLevel::Error.to_string(), "ERROR");
        assert_eq!(LogLevel::Debug.to_string(), "DEBUG");
    }

    // ---------------------------------------------------------------
    // Event for unknown node id (no crash)
    // ---------------------------------------------------------------

    #[test]
    fn apply_event_for_unknown_node_does_not_crash() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::NodeStarted {
            node_id: "nonexistent".into(),
            node_type: "mystery".into(),
            timestamp: now(),
        });

        // current_node is set even though it doesn't exist in the nodes list
        assert_eq!(view.current_node, Some("nonexistent".to_string()));
        // No node in the list changed status
        for node in &view.nodes {
            assert_eq!(node.status, NodeStatus::Pending);
        }
    }

    #[test]
    fn apply_node_completed_for_unknown_node_does_not_crash() {
        let mut view = PipelineView::new("g".into(), sample_nodes());

        view.apply_event(&PipelineEvent::NodeCompleted {
            node_id: "ghost".into(),
            outcome: Outcome::success(),
            duration_ms: 42,
            timestamp: now(),
        });

        // Nothing breaks, log line is still added
        assert_eq!(view.log_lines.len(), 1);
        assert_eq!(view.completed_count(), 0);
    }

    // ---------------------------------------------------------------
    // TuiRunner::new
    // ---------------------------------------------------------------

    #[test]
    fn tui_runner_new_initializes_correctly() {
        let runner = TuiRunner::new("test_graph".into(), sample_nodes());

        assert_eq!(runner.view().graph_name, "test_graph");
        assert_eq!(runner.view().nodes.len(), 5);
        assert_eq!(runner.view().status, PipelineStatus::NotStarted);
        assert_eq!(runner.event_count(), 0);
        assert!(runner.last_update.is_none());
    }

    #[test]
    fn tui_runner_new_with_empty_nodes() {
        let runner = TuiRunner::new("empty".into(), vec![]);

        assert_eq!(runner.view().total_count(), 0);
        assert_eq!(runner.event_count(), 0);
        assert!(!runner.is_complete());
    }

    // ---------------------------------------------------------------
    // TuiRunner::process_event
    // ---------------------------------------------------------------

    #[test]
    fn tui_runner_process_event_updates_count_and_view() {
        let mut runner = TuiRunner::new("g".into(), sample_nodes());

        let ts = now();
        runner.process_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: ts,
        });

        assert_eq!(runner.event_count(), 1);
        assert_eq!(runner.last_update, Some(ts));
        assert_eq!(runner.view().status, PipelineStatus::Running);
    }

    #[test]
    fn tui_runner_process_event_increments_counter_per_event() {
        let mut runner = TuiRunner::new("g".into(), sample_nodes());

        runner.process_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });
        runner.process_event(&PipelineEvent::NodeStarted {
            node_id: "fetch".into(),
            node_type: "http".into(),
            timestamp: now(),
        });
        runner.process_event(&PipelineEvent::NodeCompleted {
            node_id: "fetch".into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: now(),
        });

        assert_eq!(runner.event_count(), 3);
        assert_eq!(runner.view().completed_count(), 1);
    }

    #[test]
    fn tui_runner_last_update_tracks_latest_event_timestamp() {
        let mut runner = TuiRunner::new("g".into(), sample_nodes());

        let ts1 = now();
        runner.process_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: ts1,
        });
        assert_eq!(runner.last_update, Some(ts1));

        let ts2 = now();
        runner.process_event(&PipelineEvent::NodeStarted {
            node_id: "fetch".into(),
            node_type: "http".into(),
            timestamp: ts2,
        });
        assert_eq!(runner.last_update, Some(ts2));
    }

    // ---------------------------------------------------------------
    // TuiRunner::render_line
    // ---------------------------------------------------------------

    #[test]
    fn tui_runner_render_line_delegates_to_view() {
        let runner = TuiRunner::new("my_pipeline".into(), sample_nodes());
        assert_eq!(runner.render_line(), "Not started: my_pipeline (0/5 nodes)");
    }

    // ---------------------------------------------------------------
    // TuiRunner::is_complete
    // ---------------------------------------------------------------

    #[test]
    fn tui_runner_is_complete_false_for_not_started() {
        let runner = TuiRunner::new("g".into(), sample_nodes());
        assert!(!runner.is_complete());
    }

    #[test]
    fn tui_runner_is_complete_false_for_running() {
        let mut runner = TuiRunner::new("g".into(), sample_nodes());
        runner.process_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });
        assert!(!runner.is_complete());
    }

    #[test]
    fn tui_runner_is_complete_true_for_completed() {
        let mut runner = TuiRunner::new("g".into(), sample_nodes());
        runner.process_event(&PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });
        runner.process_event(&PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 5,
            duration_ms: 1000,
            timestamp: now(),
        });
        assert!(runner.is_complete());
    }

    #[test]
    fn tui_runner_is_complete_true_for_failed() {
        let mut runner = TuiRunner::new("g".into(), sample_nodes());
        runner.process_event(&PipelineEvent::NodeFailed {
            node_id: "fetch".into(),
            error: "boom".into(),
            duration_ms: 50,
            timestamp: now(),
        });
        assert!(runner.is_complete());
    }

    #[test]
    fn tui_runner_is_complete_true_for_aborted() {
        let mut runner = TuiRunner::new("g".into(), sample_nodes());
        runner.process_event(&PipelineEvent::PipelineAborted {
            reason: "user cancelled".into(),
            timestamp: now(),
        });
        assert!(runner.is_complete());
    }

    // ---------------------------------------------------------------
    // TuiRunner event processing sequence
    // ---------------------------------------------------------------

    #[test]
    fn tui_runner_full_event_sequence() {
        let nodes = vec![
            ("step1".into(), "llm".into()),
            ("step2".into(), "tool".into()),
        ];
        let mut runner = TuiRunner::new("workflow".into(), nodes);

        // Pipeline starts
        runner.process_event(&PipelineEvent::PipelineStarted {
            graph_name: "workflow".into(),
            timestamp: now(),
        });
        assert!(!runner.is_complete());
        assert_eq!(runner.event_count(), 1);

        // Node executes
        runner.process_event(&PipelineEvent::NodeStarted {
            node_id: "step1".into(),
            node_type: "llm".into(),
            timestamp: now(),
        });
        runner.process_event(&PipelineEvent::NodeCompleted {
            node_id: "step1".into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: now(),
        });
        runner.process_event(&PipelineEvent::NodeStarted {
            node_id: "step2".into(),
            node_type: "tool".into(),
            timestamp: now(),
        });
        runner.process_event(&PipelineEvent::NodeCompleted {
            node_id: "step2".into(),
            outcome: Outcome::success(),
            duration_ms: 200,
            timestamp: now(),
        });

        // Pipeline completes
        runner.process_event(&PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 2,
            duration_ms: 400,
            timestamp: now(),
        });

        assert!(runner.is_complete());
        assert_eq!(runner.event_count(), 6);
        assert_eq!(runner.view().completed_count(), 2);
        assert_eq!(runner.render_line(), "Completed: 2/2 nodes in 0.4s");
    }

    // ---------------------------------------------------------------
    // DisplayFormat basics
    // ---------------------------------------------------------------

    #[test]
    fn display_format_variants_are_distinct() {
        assert_ne!(DisplayFormat::Compact, DisplayFormat::Verbose);
        assert_ne!(DisplayFormat::Json, DisplayFormat::Silent);
        assert_eq!(DisplayFormat::Compact, DisplayFormat::Compact);
    }

    // ---------------------------------------------------------------
    // format_event: Compact
    // ---------------------------------------------------------------

    #[test]
    fn format_event_compact_pipeline_started() {
        let event = PipelineEvent::PipelineStarted {
            graph_name: "my_graph".into(),
            timestamp: now(),
        };
        let result = format_event(&event, &DisplayFormat::Compact);
        assert_eq!(result, Some("Pipeline 'my_graph' started".to_string()));
    }

    #[test]
    fn format_event_compact_node_started() {
        let event = PipelineEvent::NodeStarted {
            node_id: "fetch".into(),
            node_type: "http".into(),
            timestamp: now(),
        };
        let result = format_event(&event, &DisplayFormat::Compact);
        assert_eq!(result, Some("Node 'fetch' (http) started".to_string()));
    }

    #[test]
    fn format_event_compact_node_completed() {
        let event = PipelineEvent::NodeCompleted {
            node_id: "fetch".into(),
            outcome: Outcome::success(),
            duration_ms: 250,
            timestamp: now(),
        };
        let result = format_event(&event, &DisplayFormat::Compact);
        assert_eq!(result, Some("Node 'fetch' completed in 250ms".to_string()));
    }

    #[test]
    fn format_event_compact_node_failed() {
        let event = PipelineEvent::NodeFailed {
            node_id: "validate".into(),
            error: "schema mismatch".into(),
            duration_ms: 100,
            timestamp: now(),
        };
        let result = format_event(&event, &DisplayFormat::Compact);
        assert_eq!(
            result,
            Some("Node 'validate' failed: schema mismatch".to_string())
        );
    }

    #[test]
    fn format_event_compact_pipeline_completed() {
        let event = PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 5,
            duration_ms: 3400,
            timestamp: now(),
        };
        let result = format_event(&event, &DisplayFormat::Compact);
        assert_eq!(result, Some("Pipeline completed in 3400ms".to_string()));
    }

    #[test]
    fn format_event_compact_pipeline_aborted() {
        let event = PipelineEvent::PipelineAborted {
            reason: "user cancelled".into(),
            timestamp: now(),
        };
        let result = format_event(&event, &DisplayFormat::Compact);
        assert_eq!(result, Some("Pipeline aborted: user cancelled".to_string()));
    }

    #[test]
    fn format_event_compact_edge_traversed() {
        let event = PipelineEvent::EdgeTraversed {
            from: "a".into(),
            to: "b".into(),
            label: Some("success".into()),
            timestamp: now(),
        };
        let result = format_event(&event, &DisplayFormat::Compact);
        assert_eq!(result, Some("Edge: a -> b".to_string()));
    }

    // ---------------------------------------------------------------
    // format_event: Verbose
    // ---------------------------------------------------------------

    #[test]
    fn format_event_verbose_contains_timestamp_and_kind() {
        let event = PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        };
        let result = format_event(&event, &DisplayFormat::Verbose).unwrap();
        assert!(result.starts_with('['));
        assert!(result.contains("pipeline_started"));
        assert!(result.contains("graph=g"));
    }

    #[test]
    fn format_event_verbose_node_started() {
        let event = PipelineEvent::NodeStarted {
            node_id: "fetch".into(),
            node_type: "http".into(),
            timestamp: now(),
        };
        let result = format_event(&event, &DisplayFormat::Verbose).unwrap();
        assert!(result.contains("node_started"));
        assert!(result.contains("node=fetch"));
        assert!(result.contains("type=http"));
    }

    #[test]
    fn format_event_verbose_node_failed() {
        let event = PipelineEvent::NodeFailed {
            node_id: "step1".into(),
            error: "timeout".into(),
            duration_ms: 5000,
            timestamp: now(),
        };
        let result = format_event(&event, &DisplayFormat::Verbose).unwrap();
        assert!(result.contains("node_failed"));
        assert!(result.contains("node=step1"));
        assert!(result.contains("error=\"timeout\""));
        assert!(result.contains("duration=5000ms"));
    }

    // ---------------------------------------------------------------
    // format_event: Json
    // ---------------------------------------------------------------

    #[test]
    fn format_event_json_produces_valid_json() {
        let event = PipelineEvent::PipelineStarted {
            graph_name: "my_graph".into(),
            timestamp: now(),
        };
        let result = format_event(&event, &DisplayFormat::Json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["kind"], "pipeline_started");
        assert_eq!(parsed["graph_name"], "my_graph");
    }

    #[test]
    fn format_event_json_node_completed() {
        let event = PipelineEvent::NodeCompleted {
            node_id: "fetch".into(),
            outcome: Outcome::success(),
            duration_ms: 250,
            timestamp: now(),
        };
        let result = format_event(&event, &DisplayFormat::Json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["kind"], "node_completed");
        assert_eq!(parsed["node_id"], "fetch");
        assert_eq!(parsed["duration_ms"], 250);
    }

    // ---------------------------------------------------------------
    // format_event: Silent
    // ---------------------------------------------------------------

    #[test]
    fn format_event_silent_returns_none() {
        let event = PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        };
        assert!(format_event(&event, &DisplayFormat::Silent).is_none());
    }

    #[test]
    fn format_event_silent_returns_none_for_all_event_types() {
        let ts = now();
        let events = vec![
            PipelineEvent::PipelineStarted {
                graph_name: "g".into(),
                timestamp: ts,
            },
            PipelineEvent::NodeStarted {
                node_id: "n".into(),
                node_type: "t".into(),
                timestamp: ts,
            },
            PipelineEvent::NodeCompleted {
                node_id: "n".into(),
                outcome: Outcome::success(),
                duration_ms: 100,
                timestamp: ts,
            },
            PipelineEvent::NodeFailed {
                node_id: "n".into(),
                error: "e".into(),
                duration_ms: 10,
                timestamp: ts,
            },
            PipelineEvent::PipelineCompleted {
                outcome: Outcome::success(),
                total_nodes: 1,
                duration_ms: 200,
                timestamp: ts,
            },
            PipelineEvent::PipelineAborted {
                reason: "r".into(),
                timestamp: ts,
            },
        ];

        for event in &events {
            assert!(
                format_event(event, &DisplayFormat::Silent).is_none(),
                "Silent should return None for all events"
            );
        }
    }

    // ---------------------------------------------------------------
    // GraphView tests
    // ---------------------------------------------------------------

    #[test]
    fn graph_view_from_render_output() {
        use smasher_attractor::rendering::{RenderFormat, RenderOutput};

        let output = RenderOutput {
            format: RenderFormat::Dot,
            content: "digraph { a -> b }".as_bytes().to_vec(),
        };
        let ts = now();
        let view = GraphView::from_render_output(output, ts);

        assert_eq!(view.format, RenderFormat::Dot);
        assert_eq!(view.as_text(), Some("digraph { a -> b }"));
        assert_eq!(view.rendered_at, ts);
        assert!(!view.stale);
    }

    #[test]
    fn graph_view_mark_stale() {
        use smasher_attractor::rendering::{RenderFormat, RenderOutput};

        let output = RenderOutput {
            format: RenderFormat::Svg,
            content: "<svg></svg>".as_bytes().to_vec(),
        };
        let mut view = GraphView::from_render_output(output, now());

        assert!(!view.stale);
        view.mark_stale();
        assert!(view.stale);
    }

    #[test]
    fn graph_view_mark_fresh() {
        use smasher_attractor::rendering::{RenderFormat, RenderOutput};

        let output = RenderOutput {
            format: RenderFormat::Svg,
            content: "<svg>old</svg>".as_bytes().to_vec(),
        };
        let mut view = GraphView::from_render_output(output, now());
        view.mark_stale();
        assert!(view.stale);

        let ts2 = now();
        view.mark_fresh("<svg>updated</svg>".as_bytes().to_vec(), ts2);

        assert!(!view.stale);
        assert_eq!(view.as_text(), Some("<svg>updated</svg>"));
        assert_eq!(view.rendered_at, ts2);
    }

    #[test]
    fn graph_view_as_text_returns_none_for_binary() {
        use smasher_attractor::rendering::{RenderFormat, RenderOutput};

        let output = RenderOutput {
            format: RenderFormat::Png,
            content: vec![0xFF, 0xFE, 0x00],
        };
        let view = GraphView::from_render_output(output, now());

        assert!(view.as_text().is_none());
    }

    #[test]
    fn graph_view_content_len() {
        use smasher_attractor::rendering::{RenderFormat, RenderOutput};

        let output = RenderOutput {
            format: RenderFormat::Dot,
            content: "digraph {}".as_bytes().to_vec(),
        };
        let view = GraphView::from_render_output(output, now());

        assert_eq!(view.content_len(), 10);
    }
}
