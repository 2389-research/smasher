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
    pub working_dir: String,
}

impl AppState {
    pub fn new(
        client: smasher_llm::client::Client,
        default_model: String,
        working_dir: String,
    ) -> Self {
        Self {
            runs: Arc::new(RwLock::new(HashMap::new())),
            client: Arc::new(client),
            default_model,
            working_dir,
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
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["error"], "node X failed");
        assert!(json["completed_at"].is_string());
    }
}
