// ABOUTME: Structured event types and broadcast infrastructure for pipeline execution observability.
// ABOUTME: Enables UIs, loggers, and post-hoc analyzers to hook into node lifecycle, edge traversal, and pipeline state changes.

//! Pipeline event system for execution observability.
//!
//! Every meaningful moment during pipeline execution -- node starts, completions,
//! edge traversals, checkpoint creation, human-in-the-loop prompts -- is
//! represented as a [`PipelineEvent`]. Events carry a UTC timestamp and
//! variant-specific payload so that consumers can reconstruct a full execution
//! timeline.
//!
//! The delivery mechanism is [`PipelineEventEmitter`], backed by
//! `tokio::sync::broadcast`. Multiple subscribers can listen concurrently:
//! loggers, UIs, metrics collectors, or a [`PipelineEventLog`] that accumulates
//! events in memory for post-hoc analysis.
//!
//! # Subscribe pattern
//!
//! ```
//! use smasher_attractor::events::{PipelineEvent, PipelineEventEmitter};
//! use chrono::Utc;
//!
//! let emitter = PipelineEventEmitter::new(64);
//!
//! // Subscribe before emitting so the receiver sees events.
//! let mut rx = emitter.subscribe();
//!
//! emitter.emit(PipelineEvent::PipelineStarted {
//!     graph_name: "demo".into(),
//!     timestamp: Utc::now(),
//! });
//!
//! // In a real pipeline this would be `rx.recv().await` in an async task.
//! let event = rx.try_recv().unwrap();
//! assert!(event.is_pipeline_event());
//! ```

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::state::Outcome;

/// Structured events emitted during pipeline execution.
///
/// Each variant captures a meaningful moment in the pipeline lifecycle along with
/// a UTC timestamp so that consumers can reconstruct execution timelines.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PipelineEvent {
    /// A node has begun execution.
    NodeStarted {
        node_id: String,
        node_type: String,
        timestamp: DateTime<Utc>,
    },
    /// A node finished successfully with an outcome.
    NodeCompleted {
        node_id: String,
        outcome: Outcome,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },
    /// A node execution failed.
    NodeFailed {
        node_id: String,
        error: String,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },
    /// An edge was followed between two nodes.
    EdgeTraversed {
        from: String,
        to: String,
        label: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// A human-in-the-loop prompt was issued to the operator.
    HumanPromptIssued {
        node_id: String,
        question: String,
        timestamp: DateTime<Utc>,
    },
    /// A response was received from the human operator.
    HumanResponseReceived {
        node_id: String,
        response: String,
        timestamp: DateTime<Utc>,
    },
    /// A key in the shared pipeline context was updated.
    ContextUpdated {
        key: String,
        timestamp: DateTime<Utc>,
    },
    /// A checkpoint was persisted for resume support.
    CheckpointCreated {
        node_id: String,
        timestamp: DateTime<Utc>,
    },
    /// The pipeline began executing.
    PipelineStarted {
        graph_name: String,
        timestamp: DateTime<Utc>,
    },
    /// The pipeline completed (successfully or with a terminal outcome).
    PipelineCompleted {
        outcome: Outcome,
        total_nodes: usize,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },
    /// The pipeline was aborted before normal completion.
    PipelineAborted {
        reason: String,
        timestamp: DateTime<Utc>,
    },
    /// A loop edge was followed, restarting execution from an earlier node.
    LoopRestarted {
        from: String,
        to: String,
        restart_count: usize,
        timestamp: DateTime<Utc>,
    },
}

/// Summary of a single node's execution within a pipeline run.
///
/// Built from matching `NodeStarted`/`NodeCompleted` event pairs captured by
/// `PipelineEventLog`. When the node was never completed (partial execution),
/// the `completed_at` field matches `started_at` and `outcome` is `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeExecutionSummary {
    pub node_id: String,
    pub node_type: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub outcome: Option<Outcome>,
    pub retry_count: u32,
}

/// Aggregated summary of an entire pipeline execution.
///
/// Produced by [`PipelineEventLog::summary`] from collected events, giving
/// callers a single struct that captures timing, node-level detail, and the
/// final pipeline-level outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineExecutionSummary {
    pub graph_name: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub total_nodes: usize,
    pub node_summaries: Vec<NodeExecutionSummary>,
    pub final_outcome: Outcome,
    pub loop_restarts: usize,
}

impl PipelineEvent {
    /// Returns `true` for node-level lifecycle events: `NodeStarted`, `NodeCompleted`, `NodeFailed`.
    pub fn is_node_event(&self) -> bool {
        matches!(
            self,
            Self::NodeStarted { .. } | Self::NodeCompleted { .. } | Self::NodeFailed { .. }
        )
    }

    /// Returns `true` for pipeline-level lifecycle events: `PipelineStarted`, `PipelineCompleted`, `PipelineAborted`.
    pub fn is_pipeline_event(&self) -> bool {
        matches!(
            self,
            Self::PipelineStarted { .. }
                | Self::PipelineCompleted { .. }
                | Self::PipelineAborted { .. }
        )
    }

    /// Extract the node_id from variants that carry one, or `None` for pipeline-level events.
    pub fn node_id(&self) -> Option<&str> {
        match self {
            Self::NodeStarted { node_id, .. }
            | Self::NodeCompleted { node_id, .. }
            | Self::NodeFailed { node_id, .. }
            | Self::HumanPromptIssued { node_id, .. }
            | Self::HumanResponseReceived { node_id, .. }
            | Self::CheckpointCreated { node_id, .. } => Some(node_id),
            Self::EdgeTraversed { .. }
            | Self::ContextUpdated { .. }
            | Self::PipelineStarted { .. }
            | Self::PipelineCompleted { .. }
            | Self::PipelineAborted { .. }
            | Self::LoopRestarted { .. } => None,
        }
    }

    /// Return the timestamp of the event.
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            Self::NodeStarted { timestamp, .. }
            | Self::NodeCompleted { timestamp, .. }
            | Self::NodeFailed { timestamp, .. }
            | Self::EdgeTraversed { timestamp, .. }
            | Self::HumanPromptIssued { timestamp, .. }
            | Self::HumanResponseReceived { timestamp, .. }
            | Self::ContextUpdated { timestamp, .. }
            | Self::CheckpointCreated { timestamp, .. }
            | Self::PipelineStarted { timestamp, .. }
            | Self::PipelineCompleted { timestamp, .. }
            | Self::PipelineAborted { timestamp, .. }
            | Self::LoopRestarted { timestamp, .. } => *timestamp,
        }
    }
}

/// Broadcasts `PipelineEvent`s to multiple subscribers via `tokio::sync::broadcast`.
///
/// Consumers call [`subscribe`](PipelineEventEmitter::subscribe) to get a receiver,
/// then await events as the pipeline progresses. Events emitted before any subscriber
/// exists are silently dropped.
pub struct PipelineEventEmitter {
    sender: broadcast::Sender<PipelineEvent>,
}

impl PipelineEventEmitter {
    /// Create an emitter with the given channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Send an event to all active subscribers.
    ///
    /// If no subscribers exist the event is silently dropped.
    pub fn emit(&self, event: PipelineEvent) {
        let _ = self.sender.send(event);
    }

    /// Create a new subscription that will receive future events.
    pub fn subscribe(&self) -> broadcast::Receiver<PipelineEvent> {
        self.sender.subscribe()
    }

    /// Return the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for PipelineEventEmitter {
    fn default() -> Self {
        Self::new(256)
    }
}

/// Collects `PipelineEvent`s in memory for post-hoc analysis and testing.
///
/// Thread-safe via `Arc<Mutex<Vec>>`, so it can be shared across async tasks.
#[derive(Clone)]
pub struct PipelineEventLog {
    events: Arc<Mutex<Vec<PipelineEvent>>>,
}

impl PipelineEventLog {
    /// Create an empty event log.
    pub fn new() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Append an event to the log.
    pub fn push(&self, event: PipelineEvent) {
        if let Ok(mut guard) = self.events.lock() {
            guard.push(event);
        }
    }

    /// Return a clone of all collected events.
    pub fn events(&self) -> Vec<PipelineEvent> {
        match self.events.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => Vec::new(),
        }
    }

    /// Return events that belong to the given node (variants carrying a `node_id` field).
    pub fn events_for_node(&self, node_id: &str) -> Vec<PipelineEvent> {
        self.events()
            .into_iter()
            .filter(|e| e.node_id() == Some(node_id))
            .collect()
    }

    /// Return the number of events collected so far.
    pub fn len(&self) -> usize {
        match self.events.lock() {
            Ok(guard) => guard.len(),
            Err(_) => 0,
        }
    }

    /// Return `true` if no events have been collected.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Build an aggregated summary from the collected events.
    ///
    /// Returns `None` if no `PipelineStarted` event exists. Pairs
    /// `NodeStarted`/`NodeCompleted` events by `node_id` to build per-node
    /// summaries. Nodes that were started but never completed are included
    /// with `outcome: None` and a duration of zero.
    pub fn summary(&self) -> Option<PipelineExecutionSummary> {
        let events = self.events();

        // Find PipelineStarted
        let (graph_name, started_at) = events.iter().find_map(|e| match e {
            PipelineEvent::PipelineStarted {
                graph_name,
                timestamp,
            } => Some((graph_name.clone(), *timestamp)),
            _ => None,
        })?;

        // Find PipelineCompleted (may not exist for partial runs)
        let (final_outcome, total_nodes, pipeline_duration_ms, completed_at) = events
            .iter()
            .find_map(|e| match e {
                PipelineEvent::PipelineCompleted {
                    outcome,
                    total_nodes,
                    duration_ms,
                    timestamp,
                } => Some((outcome.clone(), *total_nodes, *duration_ms, *timestamp)),
                _ => None,
            })
            .unwrap_or_else(|| {
                // Pipeline never completed: use last event timestamp and a failure outcome
                let last_ts = events.last().map(|e| e.timestamp()).unwrap_or(started_at);
                (
                    Outcome::failure("pipeline did not complete"),
                    0,
                    (last_ts - started_at).num_milliseconds().max(0) as u64,
                    last_ts,
                )
            });

        // Collect NodeStarted events keyed by node_id
        let mut started_nodes: Vec<(String, String, DateTime<Utc>)> = Vec::new();
        for event in &events {
            if let PipelineEvent::NodeStarted {
                node_id,
                node_type,
                timestamp,
            } = event
            {
                started_nodes.push((node_id.clone(), node_type.clone(), *timestamp));
            }
        }

        // Build node summaries by pairing with NodeCompleted events
        let mut node_summaries: Vec<NodeExecutionSummary> = Vec::new();
        for (node_id, node_type, start_ts) in &started_nodes {
            // Find the matching NodeCompleted
            let completion = events.iter().find_map(|e| match e {
                PipelineEvent::NodeCompleted {
                    node_id: nid,
                    outcome,
                    duration_ms,
                    timestamp,
                } if nid == node_id => Some((outcome.clone(), *duration_ms, *timestamp)),
                _ => None,
            });

            match completion {
                Some((outcome, duration_ms, end_ts)) => {
                    node_summaries.push(NodeExecutionSummary {
                        node_id: node_id.clone(),
                        node_type: node_type.clone(),
                        started_at: *start_ts,
                        completed_at: end_ts,
                        duration_ms,
                        outcome: Some(outcome),
                        retry_count: 0,
                    });
                }
                None => {
                    // Node started but never completed
                    node_summaries.push(NodeExecutionSummary {
                        node_id: node_id.clone(),
                        node_type: node_type.clone(),
                        started_at: *start_ts,
                        completed_at: *start_ts,
                        duration_ms: 0,
                        outcome: None,
                        retry_count: 0,
                    });
                }
            }
        }

        // Count loop restarts
        let loop_restarts = events
            .iter()
            .filter(|e| matches!(e, PipelineEvent::LoopRestarted { .. }))
            .count();

        Some(PipelineExecutionSummary {
            graph_name,
            started_at,
            completed_at,
            duration_ms: pipeline_duration_ms,
            total_nodes,
            node_summaries,
            final_outcome,
            loop_restarts,
        })
    }
}

impl Default for PipelineEventLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;

    // ---------------------------------------------------------------
    // Helper: make a fixed timestamp for deterministic tests
    // ---------------------------------------------------------------

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    // ---------------------------------------------------------------
    // PipelineEvent serde round-trip for all variants
    // ---------------------------------------------------------------

    #[test]
    fn serde_roundtrip_node_started() {
        let ts = now();
        let event = PipelineEvent::NodeStarted {
            node_id: "summarize".into(),
            node_type: "llm".into(),
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::NodeStarted { ref node_id, ref node_type, .. }
            if node_id == "summarize" && node_type == "llm"
        ));
    }

    #[test]
    fn serde_roundtrip_node_completed() {
        let ts = now();
        let event = PipelineEvent::NodeCompleted {
            node_id: "transform".into(),
            outcome: Outcome::success_with(json!({"tokens": 42})),
            duration_ms: 150,
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::NodeCompleted { ref node_id, duration_ms: 150, .. }
            if node_id == "transform"
        ));
    }

    #[test]
    fn serde_roundtrip_node_failed() {
        let ts = now();
        let event = PipelineEvent::NodeFailed {
            node_id: "fetch".into(),
            error: "timeout".into(),
            duration_ms: 5000,
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::NodeFailed { ref node_id, ref error, duration_ms: 5000, .. }
            if node_id == "fetch" && error == "timeout"
        ));
    }

    #[test]
    fn serde_roundtrip_edge_traversed() {
        let ts = now();
        let event = PipelineEvent::EdgeTraversed {
            from: "a".into(),
            to: "b".into(),
            label: Some("success".into()),
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::EdgeTraversed { ref from, ref to, ref label, .. }
            if from == "a" && to == "b" && label.as_deref() == Some("success")
        ));
    }

    #[test]
    fn serde_roundtrip_edge_traversed_no_label() {
        let ts = now();
        let event = PipelineEvent::EdgeTraversed {
            from: "x".into(),
            to: "y".into(),
            label: None,
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::EdgeTraversed { ref label, .. }
            if label.is_none()
        ));
    }

    #[test]
    fn serde_roundtrip_human_prompt_issued() {
        let ts = now();
        let event = PipelineEvent::HumanPromptIssued {
            node_id: "review".into(),
            question: "Approve this change?".into(),
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::HumanPromptIssued { ref node_id, ref question, .. }
            if node_id == "review" && question == "Approve this change?"
        ));
    }

    #[test]
    fn serde_roundtrip_human_response_received() {
        let ts = now();
        let event = PipelineEvent::HumanResponseReceived {
            node_id: "review".into(),
            response: "yes".into(),
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::HumanResponseReceived { ref node_id, ref response, .. }
            if node_id == "review" && response == "yes"
        ));
    }

    #[test]
    fn serde_roundtrip_context_updated() {
        let ts = now();
        let event = PipelineEvent::ContextUpdated {
            key: "result_summary".into(),
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::ContextUpdated { ref key, .. }
            if key == "result_summary"
        ));
    }

    #[test]
    fn serde_roundtrip_checkpoint_created() {
        let ts = now();
        let event = PipelineEvent::CheckpointCreated {
            node_id: "step_3".into(),
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::CheckpointCreated { ref node_id, .. }
            if node_id == "step_3"
        ));
    }

    #[test]
    fn serde_roundtrip_pipeline_started() {
        let ts = now();
        let event = PipelineEvent::PipelineStarted {
            graph_name: "code_review".into(),
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::PipelineStarted { ref graph_name, .. }
            if graph_name == "code_review"
        ));
    }

    #[test]
    fn serde_roundtrip_pipeline_completed() {
        let ts = now();
        let event = PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 5,
            duration_ms: 12345,
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::PipelineCompleted {
                total_nodes: 5,
                duration_ms: 12345,
                ..
            }
        ));
    }

    #[test]
    fn serde_roundtrip_pipeline_aborted() {
        let ts = now();
        let event = PipelineEvent::PipelineAborted {
            reason: "cancellation token fired".into(),
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::PipelineAborted { ref reason, .. }
            if reason == "cancellation token fired"
        ));
    }

    #[test]
    fn serde_roundtrip_loop_restarted() {
        let ts = now();
        let event = PipelineEvent::LoopRestarted {
            from: "validate".into(),
            to: "generate".into(),
            restart_count: 3,
            timestamp: ts,
        };
        let json = serde_json::to_string(&event).unwrap();
        let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            restored,
            PipelineEvent::LoopRestarted { ref from, ref to, restart_count: 3, .. }
            if from == "validate" && to == "generate"
        ));
    }

    #[test]
    fn serde_json_contains_kind_tag() {
        let event = PipelineEvent::NodeStarted {
            node_id: "n".into(),
            node_type: "t".into(),
            timestamp: now(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"kind\":\"node_started\""));
    }

    #[test]
    fn serde_all_variants_roundtrip_batch() {
        let ts = now();
        let events = vec![
            PipelineEvent::NodeStarted {
                node_id: "a".into(),
                node_type: "llm".into(),
                timestamp: ts,
            },
            PipelineEvent::NodeCompleted {
                node_id: "a".into(),
                outcome: Outcome::success(),
                duration_ms: 100,
                timestamp: ts,
            },
            PipelineEvent::NodeFailed {
                node_id: "b".into(),
                error: "oops".into(),
                duration_ms: 50,
                timestamp: ts,
            },
            PipelineEvent::EdgeTraversed {
                from: "a".into(),
                to: "b".into(),
                label: None,
                timestamp: ts,
            },
            PipelineEvent::HumanPromptIssued {
                node_id: "c".into(),
                question: "?".into(),
                timestamp: ts,
            },
            PipelineEvent::HumanResponseReceived {
                node_id: "c".into(),
                response: "yes".into(),
                timestamp: ts,
            },
            PipelineEvent::ContextUpdated {
                key: "k".into(),
                timestamp: ts,
            },
            PipelineEvent::CheckpointCreated {
                node_id: "d".into(),
                timestamp: ts,
            },
            PipelineEvent::PipelineStarted {
                graph_name: "g".into(),
                timestamp: ts,
            },
            PipelineEvent::PipelineCompleted {
                outcome: Outcome::failure("done"),
                total_nodes: 2,
                duration_ms: 999,
                timestamp: ts,
            },
            PipelineEvent::PipelineAborted {
                reason: "cancelled".into(),
                timestamp: ts,
            },
            PipelineEvent::LoopRestarted {
                from: "x".into(),
                to: "y".into(),
                restart_count: 1,
                timestamp: ts,
            },
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let restored: PipelineEvent = serde_json::from_str(&json).unwrap();
            // Verify the kind tag round-trips by checking we get the same JSON
            let json2 = serde_json::to_string(&restored).unwrap();
            assert_eq!(json, json2, "round-trip mismatch for {json}");
        }
    }

    // ---------------------------------------------------------------
    // PipelineEvent::node_id() helper
    // ---------------------------------------------------------------

    #[test]
    fn node_id_returns_some_for_node_events() {
        let ts = now();
        assert_eq!(
            PipelineEvent::NodeStarted {
                node_id: "n1".into(),
                node_type: "t".into(),
                timestamp: ts
            }
            .node_id(),
            Some("n1")
        );
        assert_eq!(
            PipelineEvent::NodeCompleted {
                node_id: "n2".into(),
                outcome: Outcome::success(),
                duration_ms: 0,
                timestamp: ts
            }
            .node_id(),
            Some("n2")
        );
        assert_eq!(
            PipelineEvent::NodeFailed {
                node_id: "n3".into(),
                error: "e".into(),
                duration_ms: 0,
                timestamp: ts
            }
            .node_id(),
            Some("n3")
        );
        assert_eq!(
            PipelineEvent::HumanPromptIssued {
                node_id: "n4".into(),
                question: "q".into(),
                timestamp: ts
            }
            .node_id(),
            Some("n4")
        );
        assert_eq!(
            PipelineEvent::HumanResponseReceived {
                node_id: "n5".into(),
                response: "r".into(),
                timestamp: ts
            }
            .node_id(),
            Some("n5")
        );
        assert_eq!(
            PipelineEvent::CheckpointCreated {
                node_id: "n6".into(),
                timestamp: ts
            }
            .node_id(),
            Some("n6")
        );
    }

    #[test]
    fn node_id_returns_none_for_pipeline_level_events() {
        let ts = now();
        assert!(
            PipelineEvent::EdgeTraversed {
                from: "a".into(),
                to: "b".into(),
                label: None,
                timestamp: ts
            }
            .node_id()
            .is_none()
        );
        assert!(
            PipelineEvent::ContextUpdated {
                key: "k".into(),
                timestamp: ts
            }
            .node_id()
            .is_none()
        );
        assert!(
            PipelineEvent::PipelineStarted {
                graph_name: "g".into(),
                timestamp: ts
            }
            .node_id()
            .is_none()
        );
        assert!(
            PipelineEvent::PipelineCompleted {
                outcome: Outcome::success(),
                total_nodes: 0,
                duration_ms: 0,
                timestamp: ts
            }
            .node_id()
            .is_none()
        );
        assert!(
            PipelineEvent::PipelineAborted {
                reason: "r".into(),
                timestamp: ts
            }
            .node_id()
            .is_none()
        );
        assert!(
            PipelineEvent::LoopRestarted {
                from: "a".into(),
                to: "b".into(),
                restart_count: 0,
                timestamp: ts
            }
            .node_id()
            .is_none()
        );
    }

    #[test]
    fn timestamp_accessor_works() {
        let ts = now();
        let event = PipelineEvent::NodeStarted {
            node_id: "n".into(),
            node_type: "t".into(),
            timestamp: ts,
        };
        assert_eq!(event.timestamp(), ts);
    }

    // ---------------------------------------------------------------
    // PipelineEventEmitter: construction
    // ---------------------------------------------------------------

    #[test]
    fn emitter_new_has_zero_subscribers() {
        let emitter = PipelineEventEmitter::new(64);
        assert_eq!(emitter.subscriber_count(), 0);
    }

    #[test]
    fn emitter_default_has_zero_subscribers() {
        let emitter = PipelineEventEmitter::default();
        assert_eq!(emitter.subscriber_count(), 0);
    }

    // ---------------------------------------------------------------
    // PipelineEventEmitter: subscriptions
    // ---------------------------------------------------------------

    #[test]
    fn subscribe_increments_count() {
        let emitter = PipelineEventEmitter::new(16);
        let _rx1 = emitter.subscribe();
        assert_eq!(emitter.subscriber_count(), 1);
        let _rx2 = emitter.subscribe();
        assert_eq!(emitter.subscriber_count(), 2);
    }

    #[test]
    fn dropping_receiver_decrements_count() {
        let emitter = PipelineEventEmitter::new(16);
        let rx1 = emitter.subscribe();
        let _rx2 = emitter.subscribe();
        assert_eq!(emitter.subscriber_count(), 2);
        drop(rx1);
        assert_eq!(emitter.subscriber_count(), 1);
    }

    #[test]
    fn all_receivers_dropped_gives_zero_count() {
        let emitter = PipelineEventEmitter::new(16);
        let rx1 = emitter.subscribe();
        let rx2 = emitter.subscribe();
        drop(rx1);
        drop(rx2);
        assert_eq!(emitter.subscriber_count(), 0);
    }

    // ---------------------------------------------------------------
    // PipelineEventEmitter: emit/receive
    // ---------------------------------------------------------------

    #[test]
    fn emit_with_no_subscribers_does_not_panic() {
        let emitter = PipelineEventEmitter::new(16);
        emitter.emit(PipelineEvent::PipelineStarted {
            graph_name: "test".into(),
            timestamp: now(),
        });
    }

    #[tokio::test]
    async fn emitted_event_is_received() {
        let emitter = PipelineEventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(PipelineEvent::PipelineStarted {
            graph_name: "my_graph".into(),
            timestamp: now(),
        });

        let event = rx.recv().await.expect("should receive event");
        assert!(matches!(
            event,
            PipelineEvent::PipelineStarted { ref graph_name, .. }
            if graph_name == "my_graph"
        ));
    }

    #[tokio::test]
    async fn multiple_subscribers_receive_same_event() {
        let emitter = PipelineEventEmitter::new(16);
        let mut rx1 = emitter.subscribe();
        let mut rx2 = emitter.subscribe();

        emitter.emit(PipelineEvent::NodeStarted {
            node_id: "n1".into(),
            node_type: "llm".into(),
            timestamp: now(),
        });

        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();

        assert!(matches!(e1, PipelineEvent::NodeStarted { ref node_id, .. } if node_id == "n1"));
        assert!(matches!(e2, PipelineEvent::NodeStarted { ref node_id, .. } if node_id == "n1"));
    }

    #[tokio::test]
    async fn events_received_in_order() {
        let emitter = PipelineEventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(PipelineEvent::NodeStarted {
            node_id: "a".into(),
            node_type: "llm".into(),
            timestamp: now(),
        });
        emitter.emit(PipelineEvent::NodeCompleted {
            node_id: "a".into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: now(),
        });
        emitter.emit(PipelineEvent::EdgeTraversed {
            from: "a".into(),
            to: "b".into(),
            label: None,
            timestamp: now(),
        });

        let e1 = rx.recv().await.unwrap();
        let e2 = rx.recv().await.unwrap();
        let e3 = rx.recv().await.unwrap();

        assert!(matches!(e1, PipelineEvent::NodeStarted { .. }));
        assert!(matches!(e2, PipelineEvent::NodeCompleted { .. }));
        assert!(matches!(e3, PipelineEvent::EdgeTraversed { .. }));
    }

    #[tokio::test]
    async fn late_subscriber_misses_prior_events() {
        let emitter = PipelineEventEmitter::new(16);

        emitter.emit(PipelineEvent::PipelineStarted {
            graph_name: "before".into(),
            timestamp: now(),
        });

        let mut rx = emitter.subscribe();

        emitter.emit(PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 1,
            duration_ms: 50,
            timestamp: now(),
        });

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, PipelineEvent::PipelineCompleted { .. }));
    }

    #[tokio::test]
    async fn receiver_gets_closed_when_emitter_dropped() {
        let emitter = PipelineEventEmitter::new(16);
        let mut rx = emitter.subscribe();

        emitter.emit(PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });
        drop(emitter);

        // Buffered event still arrives
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, PipelineEvent::PipelineStarted { .. }));

        // Next recv should fail with Closed
        let result = rx.recv().await;
        assert!(matches!(result, Err(broadcast::error::RecvError::Closed)));
    }

    // ---------------------------------------------------------------
    // PipelineEventLog: construction
    // ---------------------------------------------------------------

    #[test]
    fn log_new_is_empty() {
        let log = PipelineEventLog::new();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn log_default_is_empty() {
        let log = PipelineEventLog::default();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    // ---------------------------------------------------------------
    // PipelineEventLog: push and collect
    // ---------------------------------------------------------------

    #[test]
    fn log_push_and_len() {
        let log = PipelineEventLog::new();
        log.push(PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });
        assert_eq!(log.len(), 1);
        assert!(!log.is_empty());

        log.push(PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 1,
            duration_ms: 100,
            timestamp: now(),
        });
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn log_events_returns_all_events() {
        let log = PipelineEventLog::new();
        log.push(PipelineEvent::NodeStarted {
            node_id: "a".into(),
            node_type: "llm".into(),
            timestamp: now(),
        });
        log.push(PipelineEvent::NodeCompleted {
            node_id: "a".into(),
            outcome: Outcome::success(),
            duration_ms: 50,
            timestamp: now(),
        });

        let events = log.events();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], PipelineEvent::NodeStarted { .. }));
        assert!(matches!(events[1], PipelineEvent::NodeCompleted { .. }));
    }

    // ---------------------------------------------------------------
    // PipelineEventLog: filtering by node
    // ---------------------------------------------------------------

    #[test]
    fn log_events_for_node_filters_correctly() {
        let log = PipelineEventLog::new();
        let ts = now();

        log.push(PipelineEvent::NodeStarted {
            node_id: "alpha".into(),
            node_type: "llm".into(),
            timestamp: ts,
        });
        log.push(PipelineEvent::NodeStarted {
            node_id: "beta".into(),
            node_type: "tool".into(),
            timestamp: ts,
        });
        log.push(PipelineEvent::NodeCompleted {
            node_id: "alpha".into(),
            outcome: Outcome::success(),
            duration_ms: 200,
            timestamp: ts,
        });
        log.push(PipelineEvent::EdgeTraversed {
            from: "alpha".into(),
            to: "beta".into(),
            label: None,
            timestamp: ts,
        });
        log.push(PipelineEvent::NodeFailed {
            node_id: "beta".into(),
            error: "crash".into(),
            duration_ms: 10,
            timestamp: ts,
        });

        let alpha_events = log.events_for_node("alpha");
        assert_eq!(alpha_events.len(), 2);
        assert!(
            matches!(alpha_events[0], PipelineEvent::NodeStarted { ref node_id, .. } if node_id == "alpha")
        );
        assert!(
            matches!(alpha_events[1], PipelineEvent::NodeCompleted { ref node_id, .. } if node_id == "alpha")
        );

        let beta_events = log.events_for_node("beta");
        assert_eq!(beta_events.len(), 2);
        assert!(
            matches!(beta_events[0], PipelineEvent::NodeStarted { ref node_id, .. } if node_id == "beta")
        );
        assert!(
            matches!(beta_events[1], PipelineEvent::NodeFailed { ref node_id, .. } if node_id == "beta")
        );
    }

    #[test]
    fn log_events_for_node_returns_empty_for_unknown_node() {
        let log = PipelineEventLog::new();
        log.push(PipelineEvent::NodeStarted {
            node_id: "exists".into(),
            node_type: "t".into(),
            timestamp: now(),
        });
        let result = log.events_for_node("does_not_exist");
        assert!(result.is_empty());
    }

    #[test]
    fn log_events_for_node_ignores_pipeline_level_events() {
        let log = PipelineEventLog::new();
        log.push(PipelineEvent::PipelineStarted {
            graph_name: "g".into(),
            timestamp: now(),
        });
        log.push(PipelineEvent::ContextUpdated {
            key: "k".into(),
            timestamp: now(),
        });
        log.push(PipelineEvent::PipelineAborted {
            reason: "r".into(),
            timestamp: now(),
        });

        // No pipeline-level events should match any node
        assert!(log.events_for_node("g").is_empty());
        assert!(log.events_for_node("k").is_empty());
    }

    // ---------------------------------------------------------------
    // PipelineEventLog: clone shares state
    // ---------------------------------------------------------------

    #[test]
    fn log_clone_shares_state() {
        let log1 = PipelineEventLog::new();
        let log2 = log1.clone();

        log1.push(PipelineEvent::PipelineStarted {
            graph_name: "shared".into(),
            timestamp: now(),
        });

        // log2 should see the event pushed via log1 (shared Arc)
        assert_eq!(log2.len(), 1);
        assert_eq!(log2.events().len(), 1);
    }

    // ---------------------------------------------------------------
    // PipelineEventLog: thread safety
    // ---------------------------------------------------------------

    #[test]
    fn log_concurrent_pushes() {
        let log = PipelineEventLog::new();
        let handles: Vec<_> = (0..20)
            .map(|i| {
                let log = log.clone();
                std::thread::spawn(move || {
                    log.push(PipelineEvent::NodeStarted {
                        node_id: format!("node_{i}"),
                        node_type: "t".into(),
                        timestamp: now(),
                    });
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(log.len(), 20);
    }

    // ---------------------------------------------------------------
    // Integration: emitter -> log via subscriber
    // ---------------------------------------------------------------

    // ---------------------------------------------------------------
    // NodeExecutionSummary serde round-trip
    // ---------------------------------------------------------------

    #[test]
    fn node_execution_summary_serde_roundtrip() {
        let ts = now();
        let summary = NodeExecutionSummary {
            node_id: "summarize".into(),
            node_type: "llm".into(),
            started_at: ts,
            completed_at: ts,
            duration_ms: 250,
            outcome: Some(Outcome::success()),
            retry_count: 0,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let restored: NodeExecutionSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.node_id, "summarize");
        assert_eq!(restored.node_type, "llm");
        assert_eq!(restored.duration_ms, 250);
        assert!(restored.outcome.as_ref().unwrap().is_success());
        assert_eq!(restored.retry_count, 0);
    }

    #[test]
    fn node_execution_summary_serde_roundtrip_with_none_outcome() {
        let ts = now();
        let summary = NodeExecutionSummary {
            node_id: "fetch".into(),
            node_type: "tool".into(),
            started_at: ts,
            completed_at: ts,
            duration_ms: 0,
            outcome: None,
            retry_count: 1,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let restored: NodeExecutionSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.node_id, "fetch");
        assert!(restored.outcome.is_none());
        assert_eq!(restored.retry_count, 1);
    }

    // ---------------------------------------------------------------
    // PipelineExecutionSummary serde round-trip
    // ---------------------------------------------------------------

    #[test]
    fn pipeline_execution_summary_serde_roundtrip() {
        let ts = now();
        let node_sum = NodeExecutionSummary {
            node_id: "step_1".into(),
            node_type: "llm".into(),
            started_at: ts,
            completed_at: ts,
            duration_ms: 100,
            outcome: Some(Outcome::success()),
            retry_count: 0,
        };
        let summary = PipelineExecutionSummary {
            graph_name: "code_review".into(),
            started_at: ts,
            completed_at: ts,
            duration_ms: 5000,
            total_nodes: 3,
            node_summaries: vec![node_sum],
            final_outcome: Outcome::success(),
            loop_restarts: 2,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let restored: PipelineExecutionSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.graph_name, "code_review");
        assert_eq!(restored.duration_ms, 5000);
        assert_eq!(restored.total_nodes, 3);
        assert_eq!(restored.node_summaries.len(), 1);
        assert_eq!(restored.node_summaries[0].node_id, "step_1");
        assert!(restored.final_outcome.is_success());
        assert_eq!(restored.loop_restarts, 2);
    }

    // ---------------------------------------------------------------
    // PipelineEventLog::summary
    // ---------------------------------------------------------------

    #[test]
    fn summary_returns_none_with_no_events() {
        let log = PipelineEventLog::new();
        assert!(log.summary().is_none());
    }

    #[test]
    fn summary_returns_none_without_pipeline_started() {
        let log = PipelineEventLog::new();
        let ts = now();
        // Only node events, no PipelineStarted
        log.push(PipelineEvent::NodeStarted {
            node_id: "a".into(),
            node_type: "llm".into(),
            timestamp: ts,
        });
        log.push(PipelineEvent::NodeCompleted {
            node_id: "a".into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: ts,
        });
        assert!(log.summary().is_none());
    }

    #[test]
    fn summary_with_complete_event_sequence() {
        let log = PipelineEventLog::new();
        let ts = now();

        log.push(PipelineEvent::PipelineStarted {
            graph_name: "my_pipeline".into(),
            timestamp: ts,
        });
        log.push(PipelineEvent::NodeStarted {
            node_id: "step_1".into(),
            node_type: "llm".into(),
            timestamp: ts,
        });
        log.push(PipelineEvent::NodeCompleted {
            node_id: "step_1".into(),
            outcome: Outcome::success_with(json!({"tokens": 42})),
            duration_ms: 200,
            timestamp: ts,
        });
        log.push(PipelineEvent::EdgeTraversed {
            from: "step_1".into(),
            to: "step_2".into(),
            label: Some("success".into()),
            timestamp: ts,
        });
        log.push(PipelineEvent::NodeStarted {
            node_id: "step_2".into(),
            node_type: "tool".into(),
            timestamp: ts,
        });
        log.push(PipelineEvent::NodeCompleted {
            node_id: "step_2".into(),
            outcome: Outcome::success(),
            duration_ms: 50,
            timestamp: ts,
        });
        log.push(PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 2,
            duration_ms: 300,
            timestamp: ts,
        });

        let summary = log.summary().expect("should produce summary");
        assert_eq!(summary.graph_name, "my_pipeline");
        assert_eq!(summary.total_nodes, 2);
        assert_eq!(summary.duration_ms, 300);
        assert!(summary.final_outcome.is_success());
        assert_eq!(summary.loop_restarts, 0);

        assert_eq!(summary.node_summaries.len(), 2);
        assert_eq!(summary.node_summaries[0].node_id, "step_1");
        assert_eq!(summary.node_summaries[0].node_type, "llm");
        assert_eq!(summary.node_summaries[0].duration_ms, 200);
        assert!(
            summary.node_summaries[0]
                .outcome
                .as_ref()
                .unwrap()
                .is_success()
        );
        assert_eq!(summary.node_summaries[1].node_id, "step_2");
        assert_eq!(summary.node_summaries[1].duration_ms, 50);
    }

    #[test]
    fn summary_handles_missing_node_completed() {
        let log = PipelineEventLog::new();
        let ts = now();

        log.push(PipelineEvent::PipelineStarted {
            graph_name: "partial_run".into(),
            timestamp: ts,
        });
        log.push(PipelineEvent::NodeStarted {
            node_id: "step_1".into(),
            node_type: "llm".into(),
            timestamp: ts,
        });
        log.push(PipelineEvent::NodeCompleted {
            node_id: "step_1".into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: ts,
        });
        // step_2 starts but never completes
        log.push(PipelineEvent::NodeStarted {
            node_id: "step_2".into(),
            node_type: "tool".into(),
            timestamp: ts,
        });

        let summary = log.summary().expect("should produce summary");
        assert_eq!(summary.graph_name, "partial_run");
        // Pipeline never completed, so final_outcome should be failure
        assert!(summary.final_outcome.is_failure());

        assert_eq!(summary.node_summaries.len(), 2);
        // step_1 completed normally
        assert!(summary.node_summaries[0].outcome.is_some());
        assert!(
            summary.node_summaries[0]
                .outcome
                .as_ref()
                .unwrap()
                .is_success()
        );
        // step_2 has no outcome (never completed)
        assert!(summary.node_summaries[1].outcome.is_none());
        assert_eq!(summary.node_summaries[1].duration_ms, 0);
    }

    #[test]
    fn summary_counts_loop_restarts() {
        let log = PipelineEventLog::new();
        let ts = now();

        log.push(PipelineEvent::PipelineStarted {
            graph_name: "loopy".into(),
            timestamp: ts,
        });
        log.push(PipelineEvent::NodeStarted {
            node_id: "generate".into(),
            node_type: "llm".into(),
            timestamp: ts,
        });
        log.push(PipelineEvent::NodeCompleted {
            node_id: "generate".into(),
            outcome: Outcome::success(),
            duration_ms: 100,
            timestamp: ts,
        });
        log.push(PipelineEvent::LoopRestarted {
            from: "validate".into(),
            to: "generate".into(),
            restart_count: 1,
            timestamp: ts,
        });
        log.push(PipelineEvent::LoopRestarted {
            from: "validate".into(),
            to: "generate".into(),
            restart_count: 2,
            timestamp: ts,
        });
        log.push(PipelineEvent::PipelineCompleted {
            outcome: Outcome::success(),
            total_nodes: 1,
            duration_ms: 500,
            timestamp: ts,
        });

        let summary = log.summary().expect("should produce summary");
        assert_eq!(summary.loop_restarts, 2);
    }

    // ---------------------------------------------------------------
    // PipelineEvent::is_node_event / is_pipeline_event classification
    // ---------------------------------------------------------------

    #[test]
    fn is_node_event_classification() {
        let ts = now();
        // These should be node events
        assert!(
            PipelineEvent::NodeStarted {
                node_id: "n".into(),
                node_type: "t".into(),
                timestamp: ts,
            }
            .is_node_event()
        );
        assert!(
            PipelineEvent::NodeCompleted {
                node_id: "n".into(),
                outcome: Outcome::success(),
                duration_ms: 0,
                timestamp: ts,
            }
            .is_node_event()
        );
        assert!(
            PipelineEvent::NodeFailed {
                node_id: "n".into(),
                error: "e".into(),
                duration_ms: 0,
                timestamp: ts,
            }
            .is_node_event()
        );

        // These should NOT be node events
        assert!(
            !PipelineEvent::PipelineStarted {
                graph_name: "g".into(),
                timestamp: ts,
            }
            .is_node_event()
        );
        assert!(
            !PipelineEvent::PipelineCompleted {
                outcome: Outcome::success(),
                total_nodes: 0,
                duration_ms: 0,
                timestamp: ts,
            }
            .is_node_event()
        );
        assert!(
            !PipelineEvent::PipelineAborted {
                reason: "r".into(),
                timestamp: ts,
            }
            .is_node_event()
        );
        assert!(
            !PipelineEvent::EdgeTraversed {
                from: "a".into(),
                to: "b".into(),
                label: None,
                timestamp: ts,
            }
            .is_node_event()
        );
        assert!(
            !PipelineEvent::HumanPromptIssued {
                node_id: "n".into(),
                question: "q".into(),
                timestamp: ts,
            }
            .is_node_event()
        );
        assert!(
            !PipelineEvent::ContextUpdated {
                key: "k".into(),
                timestamp: ts,
            }
            .is_node_event()
        );
        assert!(
            !PipelineEvent::CheckpointCreated {
                node_id: "n".into(),
                timestamp: ts,
            }
            .is_node_event()
        );
        assert!(
            !PipelineEvent::LoopRestarted {
                from: "a".into(),
                to: "b".into(),
                restart_count: 0,
                timestamp: ts,
            }
            .is_node_event()
        );
        assert!(
            !PipelineEvent::HumanResponseReceived {
                node_id: "n".into(),
                response: "r".into(),
                timestamp: ts,
            }
            .is_node_event()
        );
    }

    #[test]
    fn is_pipeline_event_classification() {
        let ts = now();
        // These should be pipeline events
        assert!(
            PipelineEvent::PipelineStarted {
                graph_name: "g".into(),
                timestamp: ts,
            }
            .is_pipeline_event()
        );
        assert!(
            PipelineEvent::PipelineCompleted {
                outcome: Outcome::success(),
                total_nodes: 0,
                duration_ms: 0,
                timestamp: ts,
            }
            .is_pipeline_event()
        );
        assert!(
            PipelineEvent::PipelineAborted {
                reason: "r".into(),
                timestamp: ts,
            }
            .is_pipeline_event()
        );

        // These should NOT be pipeline events
        assert!(
            !PipelineEvent::NodeStarted {
                node_id: "n".into(),
                node_type: "t".into(),
                timestamp: ts,
            }
            .is_pipeline_event()
        );
        assert!(
            !PipelineEvent::NodeCompleted {
                node_id: "n".into(),
                outcome: Outcome::success(),
                duration_ms: 0,
                timestamp: ts,
            }
            .is_pipeline_event()
        );
        assert!(
            !PipelineEvent::NodeFailed {
                node_id: "n".into(),
                error: "e".into(),
                duration_ms: 0,
                timestamp: ts,
            }
            .is_pipeline_event()
        );
        assert!(
            !PipelineEvent::EdgeTraversed {
                from: "a".into(),
                to: "b".into(),
                label: None,
                timestamp: ts,
            }
            .is_pipeline_event()
        );
        assert!(
            !PipelineEvent::HumanPromptIssued {
                node_id: "n".into(),
                question: "q".into(),
                timestamp: ts,
            }
            .is_pipeline_event()
        );
        assert!(
            !PipelineEvent::ContextUpdated {
                key: "k".into(),
                timestamp: ts,
            }
            .is_pipeline_event()
        );
        assert!(
            !PipelineEvent::CheckpointCreated {
                node_id: "n".into(),
                timestamp: ts,
            }
            .is_pipeline_event()
        );
        assert!(
            !PipelineEvent::LoopRestarted {
                from: "a".into(),
                to: "b".into(),
                restart_count: 0,
                timestamp: ts,
            }
            .is_pipeline_event()
        );
        assert!(
            !PipelineEvent::HumanResponseReceived {
                node_id: "n".into(),
                response: "r".into(),
                timestamp: ts,
            }
            .is_pipeline_event()
        );
    }

    #[test]
    fn is_node_event_and_is_pipeline_event_are_mutually_exclusive_for_lifecycle() {
        let ts = now();
        // Node lifecycle events
        let node_events = vec![
            PipelineEvent::NodeStarted {
                node_id: "n".into(),
                node_type: "t".into(),
                timestamp: ts,
            },
            PipelineEvent::NodeCompleted {
                node_id: "n".into(),
                outcome: Outcome::success(),
                duration_ms: 0,
                timestamp: ts,
            },
            PipelineEvent::NodeFailed {
                node_id: "n".into(),
                error: "e".into(),
                duration_ms: 0,
                timestamp: ts,
            },
        ];
        for event in &node_events {
            assert!(event.is_node_event());
            assert!(!event.is_pipeline_event());
        }

        // Pipeline lifecycle events
        let pipeline_events = vec![
            PipelineEvent::PipelineStarted {
                graph_name: "g".into(),
                timestamp: ts,
            },
            PipelineEvent::PipelineCompleted {
                outcome: Outcome::success(),
                total_nodes: 0,
                duration_ms: 0,
                timestamp: ts,
            },
            PipelineEvent::PipelineAborted {
                reason: "r".into(),
                timestamp: ts,
            },
        ];
        for event in &pipeline_events {
            assert!(event.is_pipeline_event());
            assert!(!event.is_node_event());
        }
    }

    // ---------------------------------------------------------------
    // Integration: emitter -> log via subscriber
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn emitter_feeds_log_via_subscriber() {
        let emitter = PipelineEventEmitter::new(64);
        let mut rx = emitter.subscribe();
        let log = PipelineEventLog::new();

        // Emit a few events
        emitter.emit(PipelineEvent::PipelineStarted {
            graph_name: "integration".into(),
            timestamp: now(),
        });
        emitter.emit(PipelineEvent::NodeStarted {
            node_id: "step1".into(),
            node_type: "llm".into(),
            timestamp: now(),
        });
        emitter.emit(PipelineEvent::NodeCompleted {
            node_id: "step1".into(),
            outcome: Outcome::success(),
            duration_ms: 42,
            timestamp: now(),
        });

        // Drain events from receiver into log
        for _ in 0..3 {
            let event = rx.recv().await.unwrap();
            log.push(event);
        }

        assert_eq!(log.len(), 3);
        let step1_events = log.events_for_node("step1");
        assert_eq!(step1_events.len(), 2);
    }
}
