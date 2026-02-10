// ABOUTME: Shared application state for the web server including run records and LLM client.
// ABOUTME: Manages the lifecycle of pipeline runs with thread-safe concurrent access.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use smasher_attractor::events::{PipelineEventEmitter, PipelineEventLog};
use smasher_attractor::graph::Graph;
use smasher_attractor::http_interviewer::HttpInterviewer;
use smasher_attractor::state::RunStatus;

/// Shared application state cloned into each axum handler.
#[derive(Clone)]
pub struct AppState {
    pub runs: Arc<RwLock<HashMap<String, RunRecord>>>,
    pub client: Arc<smasher_llm::client::Client>,
    pub default_model: String,
    pub data_dir: String,
}

impl AppState {
    pub fn new(
        client: smasher_llm::client::Client,
        default_model: String,
        data_dir: String,
    ) -> Self {
        Self {
            runs: Arc::new(RwLock::new(HashMap::new())),
            client: Arc::new(client),
            default_model,
            data_dir,
        }
    }
}

/// A single pipeline execution record.
pub struct RunRecord {
    pub id: String,
    pub dot_source: String,
    pub graph: Graph,
    pub status: RunStatus,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub emitter: Arc<PipelineEventEmitter>,
    pub event_log: Arc<PipelineEventLog>,
    pub cancellation: CancellationToken,
    pub interviewer: HttpInterviewer,
    pub variables: HashMap<String, String>,
    pub error: Option<String>,
    pub input_tokens: Arc<AtomicU64>,
    pub output_tokens: Arc<AtomicU64>,
    pub run_working_dir: Option<String>,
}

/// Serializable summary of a run for API responses.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct RunSummary {
    pub id: String,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub graph_name: Option<String>,
    pub error: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub run_working_dir: Option<String>,
}

impl RunRecord {
    pub fn to_summary(&self) -> RunSummary {
        RunSummary {
            id: self.id.clone(),
            status: format!("{:?}", self.status),
            started_at: self.started_at.to_rfc3339(),
            completed_at: self.completed_at.map(|t| t.to_rfc3339()),
            graph_name: self.graph.name.clone(),
            error: self.error.clone(),
            input_tokens: self.input_tokens.load(Ordering::Relaxed),
            output_tokens: self.output_tokens.load(Ordering::Relaxed),
            run_working_dir: self.run_working_dir.as_ref().map(|dir| {
                // Expose only the relative path (artifacts/{run_id}/...) to avoid
                // leaking absolute server filesystem paths in API responses and UI.
                let path = std::path::Path::new(dir);
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| format!("artifacts/{name}"))
                    .unwrap_or_else(|| dir.clone())
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_state_new_creates_empty_runs() {
        let client = smasher_llm::client::Client::from_env();
        let state = AppState::new(client, "test-model".into(), "/tmp".into());
        // runs map should be empty on creation
        let runs = state.runs.try_read().unwrap();
        assert!(runs.is_empty());
    }

    #[test]
    fn run_summary_serializes_to_json() {
        let summary = RunSummary {
            id: "test-123".into(),
            status: "Running".into(),
            started_at: "2025-01-01T00:00:00Z".into(),
            completed_at: None,
            graph_name: Some("my_pipeline".into()),
            error: None,
            input_tokens: 0,
            output_tokens: 0,
            run_working_dir: None,
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["id"], "test-123");
        assert_eq!(json["status"], "Running");
        assert!(json["completed_at"].is_null());
    }

    #[test]
    fn run_summary_with_error() {
        let summary = RunSummary {
            id: "test-456".into(),
            status: "Failed".into(),
            started_at: "2025-01-01T00:00:00Z".into(),
            completed_at: Some("2025-01-01T00:01:00Z".into()),
            graph_name: None,
            error: Some("node X failed".into()),
            input_tokens: 100,
            output_tokens: 50,
            run_working_dir: None,
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["error"], "node X failed");
        assert!(json["completed_at"].is_string());
    }

    /// Helper to create a minimal RunRecord for testing to_summary().
    fn make_test_record(status: RunStatus, working_dir: Option<String>) -> RunRecord {
        use smasher_attractor::dot::parser;
        use smasher_attractor::graph;
        let dot_graph = parser::parse("digraph { a -> b }").unwrap();
        let resolved = graph::resolve(&dot_graph).unwrap();
        RunRecord {
            id: "test-run-id".into(),
            dot_source: "digraph { a -> b }".into(),
            graph: resolved,
            status,
            started_at: Utc::now(),
            completed_at: None,
            emitter: Arc::new(PipelineEventEmitter::default()),
            event_log: Arc::new(PipelineEventLog::new()),
            cancellation: CancellationToken::new(),
            interviewer: HttpInterviewer::new(),
            variables: HashMap::new(),
            error: None,
            input_tokens: Arc::new(AtomicU64::new(0)),
            output_tokens: Arc::new(AtomicU64::new(0)),
            run_working_dir: working_dir,
        }
    }

    #[test]
    fn to_summary_exposes_relative_working_dir() {
        let record = make_test_record(
            RunStatus::Running,
            Some("/home/user/project/artifacts/abc-123".into()),
        );
        let summary = record.to_summary();
        assert_eq!(summary.run_working_dir, Some("artifacts/abc-123".into()));
    }

    #[test]
    fn to_summary_handles_none_working_dir() {
        let record = make_test_record(RunStatus::Running, None);
        let summary = record.to_summary();
        assert!(summary.run_working_dir.is_none());
    }

    #[test]
    fn to_summary_aborted_status() {
        let record = make_test_record(RunStatus::Aborted, None);
        let summary = record.to_summary();
        assert_eq!(summary.status, "Aborted");
    }

    #[test]
    fn to_summary_completed_status() {
        let record = make_test_record(RunStatus::Completed, None);
        let summary = record.to_summary();
        assert_eq!(summary.status, "Completed");
    }
}
