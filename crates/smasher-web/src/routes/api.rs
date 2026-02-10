// ABOUTME: JSON API route handlers for pipeline submission, status, cancellation, and events.
// ABOUTME: Provides the /api/* endpoints consumed by HTMX and external clients.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use smasher_attractor::dot::parser;
use smasher_attractor::engine::{Engine, EngineConfig};
use smasher_attractor::events::{PipelineEvent, PipelineEventEmitter, PipelineEventLog};
use smasher_attractor::graph;
use smasher_attractor::handler::{CodergenHandler, default_registry};
use smasher_attractor::http_interviewer::HttpInterviewer;
use smasher_attractor::rendering::{
    CachedRenderer, GraphRenderer, NodeExecutionStatus, RenderFormat, StatusGraphvizRenderer,
};
use smasher_attractor::state::{Context, RunStatus};
use smasher_attractor::transforms;

use crate::backend::AgentCodergenBackend;
use crate::error::WebError;
use crate::sse;
use crate::state::{AppState, RunRecord, RunSummary};

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct SubmitRequest {
    pub dot_source: String,
    #[serde(default)]
    pub variables: HashMap<String, String>,
    pub model: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubmitResponse {
    pub run_id: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListRunsResponse {
    pub runs: Vec<RunSummary>,
}

#[derive(Debug, Serialize)]
pub struct CancelResponse {
    pub success: bool,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/runs", post(submit_pipeline).get(list_runs))
        .route("/api/runs/{id}", get(get_run))
        .route("/api/runs/{id}/events", get(events_stream))
        .route("/api/runs/{id}/cancel", post(cancel_run))
        .route("/api/runs/{id}/tokens", get(get_tokens))
        .route("/api/runs/{id}/graph", get(render_graph))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
    })
}

async fn submit_pipeline(
    State(state): State<AppState>,
    Json(req): Json<SubmitRequest>,
) -> Result<Json<SubmitResponse>, WebError> {
    // Parse and resolve the graph.
    let dot_graph = parser::parse(&req.dot_source)?;
    let mut resolved = graph::resolve(&dot_graph)?;

    let mut variables = req.variables.clone();
    let model = req.model.unwrap_or_else(|| state.default_model.clone());
    variables.insert("model".into(), model.clone());

    transforms::apply_transforms(&mut resolved, &variables, None);

    let run_id = uuid::Uuid::new_v4().to_string();
    let emitter = Arc::new(PipelineEventEmitter::default());
    let event_log = Arc::new(PipelineEventLog::new());
    let cancellation = CancellationToken::new();
    let interviewer = HttpInterviewer::new();
    let input_tokens = Arc::new(AtomicU64::new(0));
    let output_tokens = Arc::new(AtomicU64::new(0));

    let record = RunRecord {
        id: run_id.clone(),
        dot_source: req.dot_source.clone(),
        graph: resolved.clone(),
        status: RunStatus::Running,
        started_at: Utc::now(),
        completed_at: None,
        emitter: Arc::clone(&emitter),
        event_log: Arc::clone(&event_log),
        cancellation: cancellation.clone(),
        interviewer: interviewer.clone(),
        variables: variables.clone(),
        error: None,
        input_tokens: Arc::clone(&input_tokens),
        output_tokens: Arc::clone(&output_tokens),
    };

    {
        let mut runs = state.runs.write().await;
        runs.insert(run_id.clone(), record);
    }

    // Subscribe to events and drain into the log.
    let mut log_rx = emitter.subscribe();
    let log_clone = Arc::clone(&event_log);
    tokio::spawn(async move {
        loop {
            match log_rx.recv().await {
                Ok(event) => log_clone.push(event),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(missed = n, "event log subscriber lagged");
                }
            }
        }
    });

    // Spawn pipeline execution.
    let run_id_clone = run_id.clone();
    let runs = Arc::clone(&state.runs);
    let client = Arc::clone(&state.client);
    tokio::spawn(async move {
        let backend = Arc::new(AgentCodergenBackend::new(
            Arc::clone(&client),
            model.clone(),
            state.working_dir.clone(),
            input_tokens,
            output_tokens,
            Arc::clone(&emitter),
        ));
        let mut registry = default_registry();
        registry.register(Arc::new(CodergenHandler::new(backend)));

        let config = EngineConfig {
            max_steps: 1000,
            enable_checkpointing: false,
            cancellation_token: Some(cancellation),
        };

        let engine = Engine::with_config(resolved, registry, config).with_emitter(emitter);
        let context = Context::default();

        for (key, value) in &variables {
            context.set(key, serde_json::Value::String(value.clone()));
        }

        let result = engine.run(context).await;

        let mut runs = runs.write().await;
        if let Some(record) = runs.get_mut(&run_id_clone) {
            record.completed_at = Some(Utc::now());
            match result {
                Ok(_) => {
                    record.status = RunStatus::Completed;
                }
                Err(e) => {
                    record.status = RunStatus::Failed;
                    record.error = Some(e.to_string());
                }
            }
        }
    });

    Ok(Json(SubmitResponse {
        run_id,
        status: "Running".into(),
    }))
}

async fn list_runs(State(state): State<AppState>) -> Json<ListRunsResponse> {
    let runs = state.runs.read().await;
    let mut summaries: Vec<RunSummary> = runs.values().map(|r| r.to_summary()).collect();
    summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Json(ListRunsResponse { runs: summaries })
}

async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RunSummary>, WebError> {
    let runs = state.runs.read().await;
    let record = runs
        .get(&id)
        .ok_or_else(|| WebError::NotFound(format!("run {id}")))?;
    Ok(Json(record.to_summary()))
}

async fn events_stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<
    axum::response::sse::Sse<
        impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
    >,
    WebError,
> {
    let runs = state.runs.read().await;
    let record = runs
        .get(&id)
        .ok_or_else(|| WebError::NotFound(format!("run {id}")))?;
    let rx = record.emitter.subscribe();
    Ok(sse::event_stream(rx))
}

async fn cancel_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<CancelResponse>, WebError> {
    let runs = state.runs.read().await;
    let record = runs
        .get(&id)
        .ok_or_else(|| WebError::NotFound(format!("run {id}")))?;

    if record.status != RunStatus::Running {
        return Ok(Json(CancelResponse {
            success: false,
            status: format!("{:?}", record.status),
        }));
    }

    record.cancellation.cancel();
    Ok(Json(CancelResponse {
        success: true,
        status: "Aborted".into(),
    }))
}

async fn get_tokens(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TokenResponse>, WebError> {
    let runs = state.runs.read().await;
    let record = runs
        .get(&id)
        .ok_or_else(|| WebError::NotFound(format!("run {id}")))?;
    Ok(Json(TokenResponse {
        input_tokens: record.input_tokens.load(Ordering::Relaxed),
        output_tokens: record.output_tokens.load(Ordering::Relaxed),
    }))
}

async fn render_graph(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<axum::response::Response, WebError> {
    use axum::response::IntoResponse;

    let runs = state.runs.read().await;
    let record = runs
        .get(&id)
        .ok_or_else(|| WebError::NotFound(format!("run {id}")))?;

    // Build node status map from event log.
    let events = record.event_log.events();
    let mut statuses: HashMap<String, NodeExecutionStatus> = HashMap::new();

    for event in &events {
        match event {
            PipelineEvent::NodeStarted { node_id, .. } => {
                statuses.insert(node_id.clone(), NodeExecutionStatus::Running);
            }
            PipelineEvent::NodeCompleted { node_id, .. } => {
                statuses.insert(node_id.clone(), NodeExecutionStatus::Done);
            }
            PipelineEvent::NodeFailed { node_id, .. } => {
                statuses.insert(node_id.clone(), NodeExecutionStatus::Failed);
            }
            _ => {}
        }
    }

    let renderer = StatusGraphvizRenderer::new(statuses);
    let cached = CachedRenderer::new(renderer);
    let output = cached
        .render(&record.graph, RenderFormat::Svg)
        .await
        .map_err(|e| WebError::Internal(format!("graph render failed: {e}")))?;

    Ok((
        [(axum::http::header::CONTENT_TYPE, "image/svg+xml")],
        output.content,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let client = smasher_llm::client::Client::from_env();
        AppState::new(client, "test-model".into(), "/tmp".into())
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let app = router().with_state(test_state());
        let req = Request::builder()
            .uri("/api/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn list_runs_empty() {
        let app = router().with_state(test_state());
        let req = Request::builder()
            .uri("/api/runs")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: ListRunsResponse = serde_json::from_slice(&body).unwrap();
        assert!(parsed.runs.is_empty());
    }

    #[tokio::test]
    async fn get_run_not_found() {
        let app = router().with_state(test_state());
        let req = Request::builder()
            .uri("/api/runs/nonexistent")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn cancel_run_not_found() {
        let app = router().with_state(test_state());
        let req = Request::builder()
            .method("POST")
            .uri("/api/runs/nonexistent/cancel")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn events_stream_not_found() {
        let app = router().with_state(test_state());
        let req = Request::builder()
            .uri("/api/runs/nonexistent/events")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn graph_not_found() {
        let app = router().with_state(test_state());
        let req = Request::builder()
            .uri("/api/runs/nonexistent/graph")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn submit_invalid_dot_returns_error() {
        let app = router().with_state(test_state());
        let body = serde_json::json!({
            "dot_source": "not a valid dot graph",
            "variables": {}
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/runs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn submit_valid_dot_creates_run() {
        let state = test_state();
        let app = router().with_state(state.clone());
        let body = serde_json::json!({
            "dot_source": "digraph { a -> b }",
            "variables": {}
        });
        let req = Request::builder()
            .method("POST")
            .uri("/api/runs")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: SubmitResponse = serde_json::from_slice(&body).unwrap();
        assert!(!parsed.run_id.is_empty());
        assert_eq!(parsed.status, "Running");

        // Verify the run exists in state.
        let runs = state.runs.read().await;
        assert!(runs.contains_key(&parsed.run_id));
    }
}
