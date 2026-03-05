// ABOUTME: Status file JSON contract for inter-process pipeline communication.
// ABOUTME: Serializable pipeline status for monitoring, coordination, and resume.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::state::Outcome;

/// The current phase of pipeline execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelinePhase {
    /// Pipeline has been created but execution has not started.
    Pending,
    /// Pipeline is actively executing nodes.
    Running,
    /// Pipeline execution is paused (can be resumed).
    Paused,
    /// Pipeline finished executing all reachable nodes successfully.
    Completed,
    /// Pipeline terminated due to an error.
    Failed,
}

impl PipelinePhase {
    /// Returns true if the phase represents a terminal state (Completed or Failed).
    pub fn is_terminal(&self) -> bool {
        matches!(self, PipelinePhase::Completed | PipelinePhase::Failed)
    }
}

/// Outcome status for a single node within the pipeline status report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeOutcomeStatus {
    /// The result status: "success", "failure", or "skip".
    pub status: String,
    /// Error message if the node failed.
    pub error: Option<String>,
    /// Whether the failure is retryable.
    pub retryable: bool,
    /// Number of execution attempts for this node.
    pub attempts: u32,
}

impl NodeOutcomeStatus {
    /// Create a NodeOutcomeStatus from a pipeline Outcome.
    pub fn from_outcome(outcome: &Outcome, attempts: u32) -> Self {
        match outcome {
            Outcome::Success { .. } => Self {
                status: "success".to_string(),
                error: None,
                retryable: false,
                attempts,
            },
            Outcome::PartialSuccess { .. } => Self {
                status: "partial_success".to_string(),
                error: None,
                retryable: false,
                attempts,
            },
            Outcome::Failure {
                error, retryable, ..
            } => Self {
                status: "failure".to_string(),
                error: Some(error.clone()),
                retryable: *retryable,
                attempts,
            },
            Outcome::Retry { reason, .. } => Self {
                status: "retry".to_string(),
                error: Some(reason.clone()),
                retryable: true,
                attempts,
            },
            Outcome::Skip { reason, .. } => Self {
                status: "skip".to_string(),
                error: Some(reason.clone()),
                retryable: false,
                attempts,
            },
        }
    }
}

/// Top-level pipeline status for inter-process communication and monitoring.
///
/// Represents a serializable snapshot of pipeline execution state that can be
/// written to a status file, returned from an HTTP endpoint, or exchanged
/// between processes for coordination and resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStatus {
    /// Name of the pipeline being executed.
    pub pipeline_name: String,
    /// Current execution phase.
    pub status: PipelinePhase,
    /// The node currently being executed (if any).
    pub current_node: Option<String>,
    /// Ordered list of node IDs that have been visited.
    pub visited_nodes: Vec<String>,
    /// Per-node outcome information.
    pub node_outcomes: HashMap<String, NodeOutcomeStatus>,
    /// When pipeline execution started.
    pub started_at: DateTime<Utc>,
    /// When this status was last updated.
    pub updated_at: DateTime<Utc>,
    /// Error message if the pipeline is in a failed state.
    pub error: Option<String>,
}

impl PipelineStatus {
    /// Create a new PipelineStatus in the Pending phase.
    pub fn new(pipeline_name: &str) -> Self {
        let now = Utc::now();
        Self {
            pipeline_name: pipeline_name.to_string(),
            status: PipelinePhase::Pending,
            current_node: None,
            visited_nodes: Vec::new(),
            node_outcomes: HashMap::new(),
            started_at: now,
            updated_at: now,
            error: None,
        }
    }

    /// Transition to Running phase and set the current node.
    pub fn mark_running(&mut self, node_id: &str) {
        self.status = PipelinePhase::Running;
        self.current_node = Some(node_id.to_string());
        self.updated_at = Utc::now();
    }

    /// Record a node's completion outcome and add it to visited nodes.
    pub fn mark_node_complete(&mut self, node_id: &str, outcome: &Outcome) {
        let status = NodeOutcomeStatus::from_outcome(outcome, 1);
        self.node_outcomes.insert(node_id.to_string(), status);
        if !self.visited_nodes.contains(&node_id.to_string()) {
            self.visited_nodes.push(node_id.to_string());
        }
        self.updated_at = Utc::now();
    }

    /// Transition to Completed phase.
    pub fn mark_completed(&mut self) {
        self.status = PipelinePhase::Completed;
        self.current_node = None;
        self.updated_at = Utc::now();
    }

    /// Transition to Failed phase with an error message.
    pub fn mark_failed(&mut self, error: &str) {
        self.status = PipelinePhase::Failed;
        self.error = Some(error.to_string());
        self.current_node = None;
        self.updated_at = Utc::now();
    }

    /// Serialize this status to a JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Serialize this status to a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Deserialize a PipelineStatus from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Returns true if the pipeline is in a terminal state (Completed or Failed).
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Outcome;

    // ---------------------------------------------------------------
    // PipelinePhase tests
    // ---------------------------------------------------------------

    #[test]
    fn pipeline_phase_is_terminal_for_completed() {
        assert!(PipelinePhase::Completed.is_terminal());
    }

    #[test]
    fn pipeline_phase_is_terminal_for_failed() {
        assert!(PipelinePhase::Failed.is_terminal());
    }

    #[test]
    fn pipeline_phase_is_not_terminal_for_pending() {
        assert!(!PipelinePhase::Pending.is_terminal());
    }

    #[test]
    fn pipeline_phase_is_not_terminal_for_running() {
        assert!(!PipelinePhase::Running.is_terminal());
    }

    #[test]
    fn pipeline_phase_is_not_terminal_for_paused() {
        assert!(!PipelinePhase::Paused.is_terminal());
    }

    #[test]
    fn pipeline_phase_serde_roundtrip() {
        let phases = vec![
            PipelinePhase::Pending,
            PipelinePhase::Running,
            PipelinePhase::Paused,
            PipelinePhase::Completed,
            PipelinePhase::Failed,
        ];
        for phase in &phases {
            let json_str = serde_json::to_string(phase).unwrap();
            let deserialized: PipelinePhase = serde_json::from_str(&json_str).unwrap();
            assert_eq!(*phase, deserialized);
        }
    }

    #[test]
    fn pipeline_phase_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&PipelinePhase::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&PipelinePhase::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&PipelinePhase::Paused).unwrap(),
            "\"paused\""
        );
        assert_eq!(
            serde_json::to_string(&PipelinePhase::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&PipelinePhase::Failed).unwrap(),
            "\"failed\""
        );
    }

    // ---------------------------------------------------------------
    // NodeOutcomeStatus tests
    // ---------------------------------------------------------------

    #[test]
    fn node_outcome_status_from_success() {
        let outcome = Outcome::success();
        let status = NodeOutcomeStatus::from_outcome(&outcome, 1);
        assert_eq!(status.status, "success");
        assert_eq!(status.error, None);
        assert!(!status.retryable);
        assert_eq!(status.attempts, 1);
    }

    #[test]
    fn node_outcome_status_from_failure() {
        let outcome = Outcome::failure("something broke");
        let status = NodeOutcomeStatus::from_outcome(&outcome, 3);
        assert_eq!(status.status, "failure");
        assert_eq!(status.error, Some("something broke".to_string()));
        assert!(!status.retryable);
        assert_eq!(status.attempts, 3);
    }

    #[test]
    fn node_outcome_status_from_retryable_failure() {
        let outcome = Outcome::retryable_failure("transient");
        let status = NodeOutcomeStatus::from_outcome(&outcome, 2);
        assert_eq!(status.status, "failure");
        assert_eq!(status.error, Some("transient".to_string()));
        assert!(status.retryable);
        assert_eq!(status.attempts, 2);
    }

    #[test]
    fn node_outcome_status_from_skip() {
        let outcome = Outcome::skip("not needed");
        let status = NodeOutcomeStatus::from_outcome(&outcome, 1);
        assert_eq!(status.status, "skip");
        assert_eq!(status.error, Some("not needed".to_string()));
        assert!(!status.retryable);
        assert_eq!(status.attempts, 1);
    }

    // ---------------------------------------------------------------
    // PipelineStatus::new
    // ---------------------------------------------------------------

    #[test]
    fn pipeline_status_new_is_pending() {
        let status = PipelineStatus::new("my_pipeline");
        assert_eq!(status.pipeline_name, "my_pipeline");
        assert_eq!(status.status, PipelinePhase::Pending);
        assert_eq!(status.current_node, None);
        assert!(status.visited_nodes.is_empty());
        assert!(status.node_outcomes.is_empty());
        assert_eq!(status.error, None);
        assert!(!status.is_terminal());
    }

    // ---------------------------------------------------------------
    // mark_running
    // ---------------------------------------------------------------

    #[test]
    fn mark_running_updates_phase_and_node() {
        let mut status = PipelineStatus::new("pipeline");
        let before = status.updated_at;
        status.mark_running("node_a");

        assert_eq!(status.status, PipelinePhase::Running);
        assert_eq!(status.current_node, Some("node_a".to_string()));
        assert!(status.updated_at >= before);
        assert!(!status.is_terminal());
    }

    // ---------------------------------------------------------------
    // mark_node_complete
    // ---------------------------------------------------------------

    #[test]
    fn mark_node_complete_success() {
        let mut status = PipelineStatus::new("pipeline");
        status.mark_running("node_a");
        status.mark_node_complete("node_a", &Outcome::success());

        assert!(status.visited_nodes.contains(&"node_a".to_string()));
        let node_status = status.node_outcomes.get("node_a").unwrap();
        assert_eq!(node_status.status, "success");
        assert_eq!(node_status.error, None);
    }

    #[test]
    fn mark_node_complete_failure() {
        let mut status = PipelineStatus::new("pipeline");
        status.mark_running("node_b");
        status.mark_node_complete("node_b", &Outcome::failure("broke"));

        assert!(status.visited_nodes.contains(&"node_b".to_string()));
        let node_status = status.node_outcomes.get("node_b").unwrap();
        assert_eq!(node_status.status, "failure");
        assert_eq!(node_status.error, Some("broke".to_string()));
    }

    #[test]
    fn mark_node_complete_does_not_duplicate_visited() {
        let mut status = PipelineStatus::new("pipeline");
        status.mark_node_complete("node_a", &Outcome::success());
        status.mark_node_complete("node_a", &Outcome::success());

        assert_eq!(
            status
                .visited_nodes
                .iter()
                .filter(|n| *n == "node_a")
                .count(),
            1
        );
    }

    // ---------------------------------------------------------------
    // mark_completed
    // ---------------------------------------------------------------

    #[test]
    fn mark_completed_sets_phase() {
        let mut status = PipelineStatus::new("pipeline");
        status.mark_running("node_a");
        status.mark_completed();

        assert_eq!(status.status, PipelinePhase::Completed);
        assert_eq!(status.current_node, None);
        assert!(status.is_terminal());
    }

    // ---------------------------------------------------------------
    // mark_failed
    // ---------------------------------------------------------------

    #[test]
    fn mark_failed_sets_phase_and_error() {
        let mut status = PipelineStatus::new("pipeline");
        status.mark_running("node_a");
        status.mark_failed("catastrophic failure");

        assert_eq!(status.status, PipelinePhase::Failed);
        assert_eq!(status.error, Some("catastrophic failure".to_string()));
        assert_eq!(status.current_node, None);
        assert!(status.is_terminal());
    }

    // ---------------------------------------------------------------
    // is_terminal
    // ---------------------------------------------------------------

    #[test]
    fn is_terminal_for_completed_and_failed() {
        let mut status = PipelineStatus::new("pipeline");
        assert!(!status.is_terminal());

        status.mark_running("node_a");
        assert!(!status.is_terminal());

        status.mark_completed();
        assert!(status.is_terminal());

        let mut status2 = PipelineStatus::new("pipeline2");
        status2.mark_failed("error");
        assert!(status2.is_terminal());
    }

    // ---------------------------------------------------------------
    // Serialization roundtrip
    // ---------------------------------------------------------------

    #[test]
    fn serialization_roundtrip() {
        let mut status = PipelineStatus::new("test_pipeline");
        status.mark_running("step_1");
        status.mark_node_complete("step_1", &Outcome::success());
        status.mark_running("step_2");
        status.mark_node_complete("step_2", &Outcome::failure("bad"));
        status.mark_failed("step_2 failed");

        let json_str = status.to_json().unwrap();
        let restored = PipelineStatus::from_json(&json_str).unwrap();

        assert_eq!(restored.pipeline_name, "test_pipeline");
        assert_eq!(restored.status, PipelinePhase::Failed);
        assert_eq!(restored.error, Some("step_2 failed".to_string()));
        assert!(restored.visited_nodes.contains(&"step_1".to_string()));
        assert!(restored.visited_nodes.contains(&"step_2".to_string()));
        assert_eq!(
            restored.node_outcomes.get("step_1").unwrap().status,
            "success"
        );
        assert_eq!(
            restored.node_outcomes.get("step_2").unwrap().status,
            "failure"
        );
    }

    #[test]
    fn from_json_invalid_returns_error() {
        let result = PipelineStatus::from_json("not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn to_json_pretty_produces_formatted_output() {
        let status = PipelineStatus::new("pretty_pipeline");
        let json_str = status.to_json_pretty().unwrap();
        assert!(json_str.contains('\n'));
        assert!(json_str.contains("pretty_pipeline"));
    }

    // ---------------------------------------------------------------
    // Full lifecycle test
    // ---------------------------------------------------------------

    #[test]
    fn full_lifecycle_pending_to_completed() {
        let mut status = PipelineStatus::new("lifecycle");

        // Start pending
        assert_eq!(status.status, PipelinePhase::Pending);

        // Begin running
        status.mark_running("start");
        assert_eq!(status.status, PipelinePhase::Running);
        assert_eq!(status.current_node, Some("start".to_string()));

        // Complete start node
        status.mark_node_complete("start", &Outcome::success());
        assert!(status.visited_nodes.contains(&"start".to_string()));

        // Move to next node
        status.mark_running("process");
        assert_eq!(status.current_node, Some("process".to_string()));

        // Complete process node
        status.mark_node_complete("process", &Outcome::success());

        // Move to exit
        status.mark_running("exit");
        status.mark_node_complete("exit", &Outcome::success());

        // Mark pipeline completed
        status.mark_completed();
        assert_eq!(status.status, PipelinePhase::Completed);
        assert!(status.is_terminal());
        assert_eq!(status.visited_nodes.len(), 3);
    }

    #[test]
    fn full_lifecycle_pending_to_failed() {
        let mut status = PipelineStatus::new("fail_lifecycle");

        status.mark_running("start");
        status.mark_node_complete("start", &Outcome::success());

        status.mark_running("broken_node");
        status.mark_node_complete("broken_node", &Outcome::failure("it broke"));
        status.mark_failed("broken_node failed: it broke");

        assert_eq!(status.status, PipelinePhase::Failed);
        assert!(status.is_terminal());
        assert!(status.error.as_ref().unwrap().contains("it broke"));
    }
}
