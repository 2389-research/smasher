// ABOUTME: Preflight probe system that health-checks LLM providers before pipeline execution.
// ABOUTME: Verifies that required API keys are present and providers are reachable.

use std::time::Instant;

use serde::Serialize;

use crate::graph::Graph;

/// Result of probing a single provider/model combination.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    /// The provider name (e.g. "anthropic", "openai", "gemini").
    pub provider: String,
    /// The model ID that was probed.
    pub model: String,
    /// Whether the probe succeeded.
    pub passed: bool,
    /// Error message if the probe failed.
    pub error: Option<String>,
    /// Round-trip latency in milliseconds.
    pub latency_ms: u64,
}

/// Aggregated report from probing all providers referenced by a pipeline.
#[derive(Debug, Clone, Serialize)]
pub struct PreflightReport {
    /// Individual probe results for each model.
    pub probes: Vec<ProbeResult>,
    /// True only if every probe passed.
    pub all_passed: bool,
}

impl PreflightReport {
    /// Build a report from a list of probe results.
    pub fn from_probes(probes: Vec<ProbeResult>) -> Self {
        let all_passed = probes.iter().all(|p| p.passed);
        Self { probes, all_passed }
    }

    /// Count how many probes failed.
    pub fn failure_count(&self) -> usize {
        self.probes.iter().filter(|p| !p.passed).count()
    }
}

/// Errors from the preflight check process.
#[derive(Debug, thiserror::Error)]
pub enum PreflightError {
    #[error("preflight failed: {failures} provider(s) unreachable")]
    ProviderUnreachable {
        failures: usize,
        report: PreflightReport,
    },
}

/// Scan a resolved graph for required LLM models and probe each one.
///
/// Extracts unique model IDs from the graph, then sends a minimal health-check
/// completion request to each model's provider. Returns a report with pass/fail
/// status and latency for each probe.
pub async fn preflight_check(
    graph: &Graph,
    client: &smasher_llm::client::Client,
) -> Result<PreflightReport, PreflightError> {
    let models = graph.referenced_models();

    let mut probes = Vec::new();
    for model_id in &models {
        let probe = probe_model(client, model_id).await;
        probes.push(probe);
    }

    let report = PreflightReport::from_probes(probes);
    if !report.all_passed {
        return Err(PreflightError::ProviderUnreachable {
            failures: report.failure_count(),
            report,
        });
    }

    Ok(report)
}

/// Probe a single model by sending a minimal completion request.
async fn probe_model(client: &smasher_llm::client::Client, model_id: &str) -> ProbeResult {
    let provider = smasher_llm::types::infer_provider(model_id)
        .map(|p| p.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let request = smasher_llm::types::Request::new(
        model_id,
        vec![smasher_llm::types::Message::user("respond with OK")],
    )
    .max_tokens(16);

    let start = Instant::now();
    match client.complete(request).await {
        Ok(_) => ProbeResult {
            provider,
            model: model_id.to_string(),
            passed: true,
            error: None,
            latency_ms: start.elapsed().as_millis() as u64,
        },
        Err(e) => ProbeResult {
            provider,
            model: model_id.to_string(),
            passed: false,
            error: Some(e.to_string()),
            latency_ms: start.elapsed().as_millis() as u64,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, GraphNode, NodeAttrValue, NodeType};
    use std::collections::HashMap;

    /// Helper: build a minimal graph with given nodes (no edges).
    fn make_graph(nodes: Vec<GraphNode>) -> Graph {
        Graph {
            name: None,
            nodes,
            edges: Vec::new(),
            default_node_attrs: HashMap::new(),
            default_edge_attrs: HashMap::new(),
            graph_attrs: HashMap::new(),
        }
    }

    /// Helper: build a codergen node with a model attribute.
    fn codergen_node(id: &str, model: &str) -> GraphNode {
        let mut attrs = HashMap::new();
        attrs.insert(
            "model".to_string(),
            NodeAttrValue::String(model.to_string()),
        );
        GraphNode {
            id: id.to_string(),
            node_type: NodeType::Codergen,
            label: None,
            attrs,
        }
    }

    #[test]
    fn graph_scanning_extracts_unique_models() {
        let graph = make_graph(vec![
            codergen_node("a", "claude-sonnet-4-20250514"),
            codergen_node("b", "gpt-4o"),
            codergen_node("c", "claude-sonnet-4-20250514"),
        ]);

        let models = graph.referenced_models();
        assert_eq!(models.len(), 2);
        assert!(models.contains(&"claude-sonnet-4-20250514".to_string()));
        assert!(models.contains(&"gpt-4o".to_string()));
    }

    #[test]
    fn graph_scanning_empty_graph_returns_no_models() {
        let graph = make_graph(vec![]);
        let models = graph.referenced_models();
        assert!(models.is_empty());
    }

    #[test]
    fn graph_scanning_nodes_without_model_attr() {
        let graph = make_graph(vec![GraphNode {
            id: "start".to_string(),
            node_type: NodeType::Start,
            label: None,
            attrs: HashMap::new(),
        }]);

        let models = graph.referenced_models();
        assert!(models.is_empty());
    }

    #[test]
    fn preflight_report_from_all_passing_probes() {
        let probes = vec![
            ProbeResult {
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4-20250514".to_string(),
                passed: true,
                error: None,
                latency_ms: 150,
            },
            ProbeResult {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                passed: true,
                error: None,
                latency_ms: 200,
            },
        ];

        let report = PreflightReport::from_probes(probes);
        assert!(report.all_passed);
        assert_eq!(report.failure_count(), 0);
        assert_eq!(report.probes.len(), 2);
    }

    #[test]
    fn preflight_report_from_mixed_probes() {
        let probes = vec![
            ProbeResult {
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4-20250514".to_string(),
                passed: true,
                error: None,
                latency_ms: 150,
            },
            ProbeResult {
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                passed: false,
                error: Some("provider not configured".to_string()),
                latency_ms: 0,
            },
        ];

        let report = PreflightReport::from_probes(probes);
        assert!(!report.all_passed);
        assert_eq!(report.failure_count(), 1);
    }

    #[test]
    fn preflight_report_serializes_to_json() {
        let report = PreflightReport::from_probes(vec![ProbeResult {
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            passed: true,
            error: None,
            latency_ms: 42,
        }]);

        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["all_passed"], true);
        assert_eq!(json["probes"][0]["provider"], "anthropic");
        assert_eq!(json["probes"][0]["model"], "claude-sonnet-4-20250514");
        assert_eq!(json["probes"][0]["passed"], true);
        assert_eq!(json["probes"][0]["latency_ms"], 42);
        assert!(json["probes"][0]["error"].is_null());
    }

    #[test]
    fn preflight_report_empty_probes_is_all_passed() {
        let report = PreflightReport::from_probes(vec![]);
        assert!(report.all_passed);
        assert_eq!(report.failure_count(), 0);
    }

    #[test]
    fn preflight_error_display_includes_failure_count() {
        let report = PreflightReport::from_probes(vec![ProbeResult {
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            passed: false,
            error: Some("timeout".to_string()),
            latency_ms: 5000,
        }]);

        let err = PreflightError::ProviderUnreachable {
            failures: 1,
            report,
        };
        let msg = err.to_string();
        assert!(msg.contains("1 provider(s) unreachable"));
    }

    #[test]
    fn probe_result_serializes_with_error() {
        let probe = ProbeResult {
            provider: "gemini".to_string(),
            model: "gemini-2.5-flash".to_string(),
            passed: false,
            error: Some("API key missing".to_string()),
            latency_ms: 0,
        };

        let json = serde_json::to_value(&probe).unwrap();
        assert_eq!(json["provider"], "gemini");
        assert_eq!(json["passed"], false);
        assert_eq!(json["error"], "API key missing");
    }
}
