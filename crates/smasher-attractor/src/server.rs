// ABOUTME: HTTP server configuration for exposing pipeline execution via REST.
// ABOUTME: Defines routes, request/response types, and server lifecycle.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::http_interviewer::HttpInterviewer;
use crate::state::{RunMetadata, RunStatus};
use crate::status::{PipelinePhase, PipelineStatus};

/// Configuration for HTTP pipeline server mode.
///
/// Consumers bring their own HTTP framework (axum, actix-web, etc.)
/// and use these types to wire up endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerConfig {
    /// Host address to bind to.
    pub host: String,
    /// Port number to listen on.
    pub port: u16,
    /// Whether the GET /status endpoint is enabled.
    pub enable_status_endpoint: bool,
    /// Whether the POST /trigger endpoint is enabled.
    pub enable_trigger_endpoint: bool,
}

impl ServerConfig {
    /// Create a new ServerConfig with the given host and port.
    ///
    /// Both status and trigger endpoints are enabled by default.
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_string(),
            port,
            enable_status_endpoint: true,
            enable_trigger_endpoint: true,
        }
    }

    /// Create a config bound to localhost on port 2389.
    pub fn default_local() -> Self {
        Self::new("127.0.0.1", 2389)
    }

    /// Return the bind address as "host:port".
    pub fn bind_address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Request body for the POST /trigger endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TriggerRequest {
    /// Name or path of the pipeline to execute.
    pub pipeline: String,
    /// Variables to inject into the pipeline context.
    pub variables: HashMap<String, String>,
    /// Optional stylesheet to apply to the pipeline graph.
    pub stylesheet: Option<String>,
}

/// Response body from the POST /trigger endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TriggerResponse {
    /// Unique identifier for this execution run.
    pub execution_id: String,
    /// Current phase of the triggered pipeline.
    pub status: PipelinePhase,
}

/// Response body from the GET /status endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    /// Full pipeline status snapshot.
    pub pipeline_status: PipelineStatus,
}

// ---------------------------------------------------------------------------
// HTTP API types for pipeline run management
// ---------------------------------------------------------------------------

/// Request body for POST /api/runs — submit a pipeline for execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubmitPipelineRequest {
    /// DOT graph source defining the pipeline topology.
    pub dot_source: String,
    /// Variables to inject into the pipeline context.
    pub variables: HashMap<String, String>,
    /// Optional model override for LLM nodes.
    pub model: Option<String>,
}

/// Response body from POST /api/runs — acknowledges pipeline submission.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubmitPipelineResponse {
    /// Unique identifier assigned to this run.
    pub run_id: String,
    /// Initial status of the submitted run.
    pub status: RunStatus,
}

/// Response body from GET /api/runs/{id} — current status of a run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunStatusResponse {
    /// The run identifier.
    pub run_id: String,
    /// Current execution status.
    pub status: RunStatus,
    /// Full run metadata, if available.
    pub metadata: Option<RunMetadata>,
}

/// Request body for POST /api/runs/{id}/cancel — cancel a running pipeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CancelRunRequest {
    /// The run identifier to cancel.
    pub run_id: String,
}

/// Response body from POST /api/runs/{id}/cancel — cancellation result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CancelRunResponse {
    /// The run identifier that was targeted.
    pub run_id: String,
    /// Whether the cancellation was accepted.
    pub cancelled: bool,
}

/// Response body from GET /api/runs — list all known runs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListRunsResponse {
    /// Metadata for each known run.
    pub runs: Vec<RunMetadata>,
}

// ---------------------------------------------------------------------------
// HTTP method and route definitions
// ---------------------------------------------------------------------------

/// HTTP methods used by the API surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Delete,
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpMethod::Get => write!(f, "GET"),
            HttpMethod::Post => write!(f, "POST"),
            HttpMethod::Delete => write!(f, "DELETE"),
        }
    }
}

/// A single API route definition mapping an HTTP method and path to a description.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Route {
    /// HTTP method for this route.
    pub method: HttpMethod,
    /// URL path pattern (e.g. "/api/runs/{id}").
    pub path: String,
    /// Human-readable description of what this route does.
    pub description: String,
}

/// Provides the canonical set of API routes for the pipeline HTTP service.
///
/// Consumers use `ApiRouter::routes()` to discover available endpoints and
/// wire them into their HTTP framework of choice.
pub struct ApiRouter;

impl ApiRouter {
    /// Return the full list of API route definitions.
    ///
    /// Includes both pipeline management routes and interviewer question
    /// endpoints so consumers have a single source of truth for wiring.
    pub fn routes() -> Vec<Route> {
        let mut routes = vec![
            Route {
                method: HttpMethod::Post,
                path: "/api/runs".to_string(),
                description: "Submit a pipeline for execution".to_string(),
            },
            Route {
                method: HttpMethod::Get,
                path: "/api/runs".to_string(),
                description: "List all pipeline runs".to_string(),
            },
            Route {
                method: HttpMethod::Get,
                path: "/api/runs/{id}".to_string(),
                description: "Get status of a specific run".to_string(),
            },
            Route {
                method: HttpMethod::Get,
                path: "/api/runs/{id}/events".to_string(),
                description: "Stream run events via SSE".to_string(),
            },
            Route {
                method: HttpMethod::Post,
                path: "/api/runs/{id}/cancel".to_string(),
                description: "Cancel a running pipeline".to_string(),
            },
            Route {
                method: HttpMethod::Get,
                path: "/api/runs/{id}/graph".to_string(),
                description: "Render pipeline graph as DOT or SVG".to_string(),
            },
            Route {
                method: HttpMethod::Get,
                path: "/api/health".to_string(),
                description: "Health check endpoint".to_string(),
            },
        ];
        routes.extend(HttpInterviewer::routes());
        routes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::PipelinePhase;

    // ---------------------------------------------------------------
    // ServerConfig tests
    // ---------------------------------------------------------------

    #[test]
    fn server_config_default_local() {
        let config = ServerConfig::default_local();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 2389);
        assert!(config.enable_status_endpoint);
        assert!(config.enable_trigger_endpoint);
    }

    #[test]
    fn server_config_new() {
        let config = ServerConfig::new("0.0.0.0", 8080);
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert!(config.enable_status_endpoint);
        assert!(config.enable_trigger_endpoint);
    }

    #[test]
    fn server_config_bind_address() {
        let config = ServerConfig::new("127.0.0.1", 2389);
        assert_eq!(config.bind_address(), "127.0.0.1:2389");
    }

    #[test]
    fn server_config_bind_address_custom() {
        let config = ServerConfig::new("0.0.0.0", 9999);
        assert_eq!(config.bind_address(), "0.0.0.0:9999");
    }

    #[test]
    fn server_config_serialization() {
        let config = ServerConfig::default_local();
        let json_str = serde_json::to_string(&config).unwrap();
        let restored: ServerConfig = serde_json::from_str(&json_str).unwrap();
        assert_eq!(config, restored);
    }

    #[test]
    fn server_config_deserialization_from_json() {
        let json_str = r#"{
            "host": "10.0.0.1",
            "port": 3000,
            "enable_status_endpoint": false,
            "enable_trigger_endpoint": true
        }"#;
        let config: ServerConfig = serde_json::from_str(json_str).unwrap();
        assert_eq!(config.host, "10.0.0.1");
        assert_eq!(config.port, 3000);
        assert!(!config.enable_status_endpoint);
        assert!(config.enable_trigger_endpoint);
    }

    // ---------------------------------------------------------------
    // TriggerRequest tests
    // ---------------------------------------------------------------

    #[test]
    fn trigger_request_serialization() {
        let mut vars = HashMap::new();
        vars.insert("env".to_string(), "production".to_string());
        vars.insert("version".to_string(), "1.2.3".to_string());

        let request = TriggerRequest {
            pipeline: "deploy_pipeline".to_string(),
            variables: vars,
            stylesheet: Some("dark.css".to_string()),
        };

        let json_str = serde_json::to_string(&request).unwrap();
        let restored: TriggerRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(request, restored);
    }

    #[test]
    fn trigger_request_without_stylesheet() {
        let request = TriggerRequest {
            pipeline: "simple_pipeline".to_string(),
            variables: HashMap::new(),
            stylesheet: None,
        };

        let json_str = serde_json::to_string(&request).unwrap();
        let restored: TriggerRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(request, restored);
        assert_eq!(restored.stylesheet, None);
    }

    #[test]
    fn trigger_request_deserialization_from_json() {
        let json_str = r#"{
            "pipeline": "test_pipe",
            "variables": {"key": "value"},
            "stylesheet": null
        }"#;
        let request: TriggerRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(request.pipeline, "test_pipe");
        assert_eq!(request.variables.get("key"), Some(&"value".to_string()));
        assert_eq!(request.stylesheet, None);
    }

    // ---------------------------------------------------------------
    // TriggerResponse tests
    // ---------------------------------------------------------------

    #[test]
    fn trigger_response_serialization() {
        let response = TriggerResponse {
            execution_id: "exec-abc-123".to_string(),
            status: PipelinePhase::Running,
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let restored: TriggerResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(response, restored);
    }

    #[test]
    fn trigger_response_pending_status() {
        let response = TriggerResponse {
            execution_id: "exec-xyz".to_string(),
            status: PipelinePhase::Pending,
        };

        let json_str = serde_json::to_string(&response).unwrap();
        assert!(json_str.contains("\"pending\""));
        assert!(json_str.contains("exec-xyz"));
    }

    // ---------------------------------------------------------------
    // StatusResponse tests
    // ---------------------------------------------------------------

    #[test]
    fn status_response_serialization() {
        let pipeline_status = PipelineStatus::new("monitored_pipeline");
        let response = StatusResponse { pipeline_status };

        let json_str = serde_json::to_string(&response).unwrap();
        let restored: StatusResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(restored.pipeline_status.pipeline_name, "monitored_pipeline");
        assert_eq!(restored.pipeline_status.status, PipelinePhase::Pending);
    }

    #[test]
    fn status_response_with_active_pipeline() {
        let mut pipeline_status = PipelineStatus::new("active_pipeline");
        pipeline_status.mark_running("step_1");

        let response = StatusResponse { pipeline_status };

        let json_str = serde_json::to_string(&response).unwrap();
        let restored: StatusResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(restored.pipeline_status.status, PipelinePhase::Running);
        assert_eq!(
            restored.pipeline_status.current_node,
            Some("step_1".to_string())
        );
    }

    // ---------------------------------------------------------------
    // SubmitPipelineRequest serde round-trip
    // ---------------------------------------------------------------

    #[test]
    fn submit_pipeline_request_serde_roundtrip() {
        let mut vars = HashMap::new();
        vars.insert("model".to_string(), "gpt-4".to_string());
        vars.insert("temperature".to_string(), "0.7".to_string());

        let request = SubmitPipelineRequest {
            dot_source: "digraph { a -> b }".to_string(),
            variables: vars,
            model: Some("claude-opus-4-6".to_string()),
        };

        let json_str = serde_json::to_string(&request).unwrap();
        let restored: SubmitPipelineRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(request, restored);
    }

    #[test]
    fn submit_pipeline_request_without_model() {
        let request = SubmitPipelineRequest {
            dot_source: "digraph { start -> end }".to_string(),
            variables: HashMap::new(),
            model: None,
        };

        let json_str = serde_json::to_string(&request).unwrap();
        let restored: SubmitPipelineRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(request, restored);
        assert_eq!(restored.model, None);
    }

    #[test]
    fn submit_pipeline_request_deserialization_from_json() {
        let json_str = r#"{
            "dot_source": "digraph { x -> y }",
            "variables": {"env": "staging"},
            "model": null
        }"#;
        let request: SubmitPipelineRequest = serde_json::from_str(json_str).unwrap();
        assert_eq!(request.dot_source, "digraph { x -> y }");
        assert_eq!(request.variables.get("env"), Some(&"staging".to_string()));
        assert_eq!(request.model, None);
    }

    // ---------------------------------------------------------------
    // SubmitPipelineResponse serde round-trip
    // ---------------------------------------------------------------

    #[test]
    fn submit_pipeline_response_serde_roundtrip() {
        let response = SubmitPipelineResponse {
            run_id: "run-abc-123".to_string(),
            status: RunStatus::Running,
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let restored: SubmitPipelineResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(response, restored);
    }

    #[test]
    fn submit_pipeline_response_all_statuses() {
        for status in [
            RunStatus::Running,
            RunStatus::Completed,
            RunStatus::Failed,
            RunStatus::Aborted,
        ] {
            let response = SubmitPipelineResponse {
                run_id: "run-1".to_string(),
                status: status.clone(),
            };
            let json_str = serde_json::to_string(&response).unwrap();
            let restored: SubmitPipelineResponse = serde_json::from_str(&json_str).unwrap();
            assert_eq!(restored.status, status);
        }
    }

    // ---------------------------------------------------------------
    // RunStatusResponse serde round-trip
    // ---------------------------------------------------------------

    #[test]
    fn run_status_response_serde_roundtrip_with_metadata() {
        let metadata = RunMetadata {
            run_id: "run-42".to_string(),
            graph_name: "analysis".to_string(),
            started_at: chrono::Utc::now(),
            completed_at: None,
            status: RunStatus::Running,
            total_nodes_executed: 3,
            variables: HashMap::new(),
        };

        let response = RunStatusResponse {
            run_id: "run-42".to_string(),
            status: RunStatus::Running,
            metadata: Some(metadata),
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let restored: RunStatusResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(restored.run_id, "run-42");
        assert_eq!(restored.status, RunStatus::Running);
        assert!(restored.metadata.is_some());
        assert_eq!(restored.metadata.unwrap().graph_name, "analysis");
    }

    #[test]
    fn run_status_response_serde_roundtrip_without_metadata() {
        let response = RunStatusResponse {
            run_id: "run-99".to_string(),
            status: RunStatus::Failed,
            metadata: None,
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let restored: RunStatusResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(restored.run_id, "run-99");
        assert_eq!(restored.status, RunStatus::Failed);
        assert!(restored.metadata.is_none());
    }

    // ---------------------------------------------------------------
    // CancelRunRequest serde round-trip
    // ---------------------------------------------------------------

    #[test]
    fn cancel_run_request_serde_roundtrip() {
        let request = CancelRunRequest {
            run_id: "run-to-cancel".to_string(),
        };

        let json_str = serde_json::to_string(&request).unwrap();
        let restored: CancelRunRequest = serde_json::from_str(&json_str).unwrap();
        assert_eq!(request, restored);
    }

    // ---------------------------------------------------------------
    // CancelRunResponse serde round-trip
    // ---------------------------------------------------------------

    #[test]
    fn cancel_run_response_serde_roundtrip_cancelled() {
        let response = CancelRunResponse {
            run_id: "run-42".to_string(),
            cancelled: true,
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let restored: CancelRunResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(response, restored);
        assert!(restored.cancelled);
    }

    #[test]
    fn cancel_run_response_serde_roundtrip_not_cancelled() {
        let response = CancelRunResponse {
            run_id: "run-already-done".to_string(),
            cancelled: false,
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let restored: CancelRunResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(response, restored);
        assert!(!restored.cancelled);
    }

    // ---------------------------------------------------------------
    // ListRunsResponse serde round-trip
    // ---------------------------------------------------------------

    #[test]
    fn list_runs_response_serde_roundtrip_empty() {
        let response = ListRunsResponse { runs: vec![] };

        let json_str = serde_json::to_string(&response).unwrap();
        let restored: ListRunsResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(response, restored);
        assert!(restored.runs.is_empty());
    }

    #[test]
    fn list_runs_response_serde_roundtrip_with_runs() {
        let meta1 = RunMetadata {
            run_id: "run-1".to_string(),
            graph_name: "pipeline_a".to_string(),
            started_at: chrono::Utc::now(),
            completed_at: None,
            status: RunStatus::Running,
            total_nodes_executed: 2,
            variables: HashMap::new(),
        };
        let meta2 = RunMetadata {
            run_id: "run-2".to_string(),
            graph_name: "pipeline_b".to_string(),
            started_at: chrono::Utc::now(),
            completed_at: Some(chrono::Utc::now()),
            status: RunStatus::Completed,
            total_nodes_executed: 5,
            variables: HashMap::new(),
        };

        let response = ListRunsResponse {
            runs: vec![meta1, meta2],
        };

        let json_str = serde_json::to_string(&response).unwrap();
        let restored: ListRunsResponse = serde_json::from_str(&json_str).unwrap();
        assert_eq!(restored.runs.len(), 2);
        assert_eq!(restored.runs[0].run_id, "run-1");
        assert_eq!(restored.runs[1].run_id, "run-2");
        assert_eq!(restored.runs[1].status, RunStatus::Completed);
    }

    // ---------------------------------------------------------------
    // HttpMethod Display
    // ---------------------------------------------------------------

    #[test]
    fn http_method_display_get() {
        assert_eq!(format!("{}", HttpMethod::Get), "GET");
    }

    #[test]
    fn http_method_display_post() {
        assert_eq!(format!("{}", HttpMethod::Post), "POST");
    }

    #[test]
    fn http_method_display_delete() {
        assert_eq!(format!("{}", HttpMethod::Delete), "DELETE");
    }

    #[test]
    fn http_method_serde_roundtrip() {
        for method in [HttpMethod::Get, HttpMethod::Post, HttpMethod::Delete] {
            let json_str = serde_json::to_string(&method).unwrap();
            let restored: HttpMethod = serde_json::from_str(&json_str).unwrap();
            assert_eq!(method, restored);
        }
    }

    #[test]
    fn http_method_serializes_as_uppercase() {
        assert_eq!(serde_json::to_string(&HttpMethod::Get).unwrap(), "\"GET\"");
        assert_eq!(
            serde_json::to_string(&HttpMethod::Post).unwrap(),
            "\"POST\""
        );
        assert_eq!(
            serde_json::to_string(&HttpMethod::Delete).unwrap(),
            "\"DELETE\""
        );
    }

    // ---------------------------------------------------------------
    // Route serde round-trip
    // ---------------------------------------------------------------

    #[test]
    fn route_serde_roundtrip() {
        let route = Route {
            method: HttpMethod::Post,
            path: "/api/runs".to_string(),
            description: "Submit a pipeline".to_string(),
        };

        let json_str = serde_json::to_string(&route).unwrap();
        let restored: Route = serde_json::from_str(&json_str).unwrap();
        assert_eq!(route, restored);
    }

    // ---------------------------------------------------------------
    // ApiRouter tests
    // ---------------------------------------------------------------

    #[test]
    fn api_router_returns_nine_routes() {
        let routes = ApiRouter::routes();
        assert_eq!(routes.len(), 9);
    }

    #[test]
    fn api_router_has_submit_pipeline_route() {
        let routes = ApiRouter::routes();
        let submit = routes
            .iter()
            .find(|r| r.method == HttpMethod::Post && r.path == "/api/runs")
            .expect("should have POST /api/runs");
        assert!(!submit.description.is_empty());
    }

    #[test]
    fn api_router_has_list_runs_route() {
        let routes = ApiRouter::routes();
        let list = routes
            .iter()
            .find(|r| r.method == HttpMethod::Get && r.path == "/api/runs")
            .expect("should have GET /api/runs");
        assert!(!list.description.is_empty());
    }

    #[test]
    fn api_router_has_get_run_status_route() {
        let routes = ApiRouter::routes();
        let status = routes
            .iter()
            .find(|r| r.method == HttpMethod::Get && r.path == "/api/runs/{id}")
            .expect("should have GET /api/runs/{id}");
        assert!(!status.description.is_empty());
    }

    #[test]
    fn api_router_has_sse_events_route() {
        let routes = ApiRouter::routes();
        let events = routes
            .iter()
            .find(|r| r.method == HttpMethod::Get && r.path == "/api/runs/{id}/events")
            .expect("should have GET /api/runs/{id}/events");
        assert!(!events.description.is_empty());
    }

    #[test]
    fn api_router_has_cancel_run_route() {
        let routes = ApiRouter::routes();
        let cancel = routes
            .iter()
            .find(|r| r.method == HttpMethod::Post && r.path == "/api/runs/{id}/cancel")
            .expect("should have POST /api/runs/{id}/cancel");
        assert!(!cancel.description.is_empty());
    }

    #[test]
    fn api_router_has_graph_render_route() {
        let routes = ApiRouter::routes();
        let render = routes
            .iter()
            .find(|r| r.method == HttpMethod::Get && r.path == "/api/runs/{id}/graph")
            .expect("should have GET /api/runs/{id}/graph");
        assert!(!render.description.is_empty());
    }

    #[test]
    fn api_router_has_health_check_route() {
        let routes = ApiRouter::routes();
        let health = routes
            .iter()
            .find(|r| r.method == HttpMethod::Get && r.path == "/api/health")
            .expect("should have GET /api/health");
        assert!(!health.description.is_empty());
    }

    #[test]
    fn api_router_has_list_questions_route() {
        let routes = ApiRouter::routes();
        let list = routes
            .iter()
            .find(|r| r.method == HttpMethod::Get && r.path == "/api/v1/questions")
            .expect("should have GET /api/v1/questions");
        assert!(!list.description.is_empty());
    }

    #[test]
    fn api_router_has_answer_question_route() {
        let routes = ApiRouter::routes();
        let answer = routes
            .iter()
            .find(|r| r.method == HttpMethod::Post && r.path == "/api/v1/questions/{id}/answer")
            .expect("should have POST /api/v1/questions/{id}/answer");
        assert!(!answer.description.is_empty());
    }

    #[test]
    fn api_router_route_paths_are_correct() {
        let routes = ApiRouter::routes();
        let paths: Vec<&str> = routes.iter().map(|r| r.path.as_str()).collect();
        assert!(paths.contains(&"/api/runs"));
        assert!(paths.contains(&"/api/runs/{id}"));
        assert!(paths.contains(&"/api/runs/{id}/events"));
        assert!(paths.contains(&"/api/runs/{id}/graph"));
        assert!(paths.contains(&"/api/runs/{id}/cancel"));
        assert!(paths.contains(&"/api/health"));
        assert!(paths.contains(&"/api/v1/questions"));
        assert!(paths.contains(&"/api/v1/questions/{id}/answer"));
    }

    #[test]
    fn api_router_all_routes_have_descriptions() {
        let routes = ApiRouter::routes();
        for route in &routes {
            assert!(
                !route.description.is_empty(),
                "Route {} {} should have a description",
                route.method,
                route.path
            );
        }
    }
}
