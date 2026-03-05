// ABOUTME: Tier 3 conformance subcommands for the Attractor Pipeline layer.
// ABOUTME: parse, validate, run, list-handlers.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use smasher_attractor::dot;
use smasher_attractor::engine::{Engine, EngineConfig};
use smasher_attractor::graph;
use smasher_attractor::handler::{
    CodergenBackend, CodergenHandler, HandlerError, default_registry,
};
use smasher_attractor::lint::LintRunner;
use smasher_attractor::state::{Context, Outcome};
use smasher_llm::client::Client;
use smasher_llm::types::{Message, Request};

use crate::convert::{execution_result_to_json, graph_to_json};

/// Parse a DOT file into a resolved Graph and emit its JSON representation.
pub async fn parse(dotfile: &Path) -> i32 {
    let content = match std::fs::read_to_string(dotfile) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to read DOT file: {e}");
            return 1;
        }
    };

    let dot_graph = match dot::parse(&content) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("DOT parse error: {e}");
            return 1;
        }
    };

    let graph = match graph::resolve(&dot_graph) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("graph resolution error: {e}");
            return 1;
        }
    };

    let json = graph_to_json(&graph);
    println!(
        "{}",
        serde_json::to_string_pretty(&json).expect("JSON serialization failed")
    );
    0
}

/// Validate a DOT file and output lint diagnostics as JSON.
pub async fn validate(dotfile: &Path) -> i32 {
    let content = match std::fs::read_to_string(dotfile) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to read DOT file: {e}");
            let diag = json!({
                "diagnostics": [{
                    "severity": "error",
                    "message": format!("failed to read DOT file: {e}"),
                    "code": "PARSE"
                }]
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&diag).expect("JSON serialization failed")
            );
            return 1;
        }
    };

    let dot_graph = match dot::parse(&content) {
        Ok(g) => g,
        Err(e) => {
            let diag = json!({
                "diagnostics": [{
                    "severity": "error",
                    "message": e.to_string(),
                    "code": "PARSE"
                }]
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&diag).expect("JSON serialization failed")
            );
            return 1;
        }
    };

    let graph = match graph::resolve(&dot_graph) {
        Ok(g) => g,
        Err(e) => {
            let diag = json!({
                "diagnostics": [{
                    "severity": "error",
                    "message": e.to_string(),
                    "code": "PARSE"
                }]
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&diag).expect("JSON serialization failed")
            );
            return 1;
        }
    };

    let runner = LintRunner::with_builtins();
    let report = runner.run(&graph);

    let output = json!({ "diagnostics": report.diagnostics });
    println!(
        "{}",
        serde_json::to_string_pretty(&output).expect("JSON serialization failed")
    );
    0
}

/// Execute a DOT pipeline with a mock LLM backend and output the result as JSON.
pub async fn run(dotfile: &Path) -> i32 {
    let content = match std::fs::read_to_string(dotfile) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to read DOT file: {e}");
            return 1;
        }
    };

    let dot_graph = match dot::parse(&content) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("DOT parse error: {e}");
            return 1;
        }
    };

    let graph = match graph::resolve(&dot_graph) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("graph resolution error: {e}");
            return 1;
        }
    };

    let client = Arc::new(Client::from_env());
    let backend = Arc::new(MockCodergenBackend { client });
    let mut registry = default_registry();
    registry.register(Arc::new(CodergenHandler::new(backend)));

    let config = EngineConfig {
        max_steps: 100,
        enable_checkpointing: false,
        ..Default::default()
    };
    let engine = Engine::with_config(graph.clone(), registry, config);
    let result = engine.run(Context::default()).await;

    match result {
        Ok(exec_result) => {
            let json = execution_result_to_json(&exec_result, &graph);
            println!(
                "{}",
                serde_json::to_string_pretty(&json).expect("JSON serialization failed")
            );
            // Propagate pipeline failure status as a non-zero exit code.
            if json["status"] == "failure" { 1 } else { 0 }
        }
        Err(e) => {
            eprintln!("pipeline execution error: {e}");
            1
        }
    }
}
/// Output the list of registered handler types as a JSON array.
pub async fn list_handlers() -> i32 {
    let handlers = json!([
        {
            "name": "start",
            "type": "Mdiamond",
            "description": "Start node handler"
        },
        {
            "name": "exit",
            "type": "Msquare",
            "description": "Exit/done node handler"
        },
        {
            "name": "codergen",
            "type": "box",
            "description": "Codergen code generation handler"
        },
        {
            "name": "conditional",
            "type": "diamond",
            "description": "Conditional routing handler"
        }
    ]);
    println!(
        "{}",
        serde_json::to_string_pretty(&handlers).expect("JSON serialization failed")
    );
    0
}

// ---------------------------------------------------------------------------
// Mock CodergenBackend for conformance testing
// ---------------------------------------------------------------------------

/// A CodergenBackend that forwards prompts to the LLM client for conformance testing.
struct MockCodergenBackend {
    client: Arc<Client>,
}

#[async_trait]
impl CodergenBackend for MockCodergenBackend {
    async fn generate(
        &self,
        prompt: &str,
        model: Option<&str>,
        _context: &Context,
    ) -> Result<Outcome, HandlerError> {
        let model_id = model.unwrap_or("gpt-4o");
        let request = Request::new(model_id, vec![Message::user(prompt)]).max_tokens(1000);
        match self.client.complete(request).await {
            Ok(response) => {
                let text = response.text().unwrap_or_default();
                Ok(Outcome::success_with(json!({"response": text})))
            }
            Err(e) => Ok(Outcome::failure(e.to_string())),
        }
    }
}
