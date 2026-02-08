// ABOUTME: HTTP server configuration for exposing pipeline execution via REST.
// ABOUTME: Defines routes, request/response types, and server lifecycle.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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
        assert_eq!(
            request.variables.get("key"),
            Some(&"value".to_string())
        );
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
        assert_eq!(
            restored.pipeline_status.pipeline_name,
            "monitored_pipeline"
        );
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
}
