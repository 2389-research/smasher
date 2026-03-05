// ABOUTME: Tier 1 conformance subcommands for the Unified LLM SDK layer.
// ABOUTME: client-from-env, complete, stream, tool-call, generate-object, list-models.

use std::io::Read;

use futures::StreamExt;
use serde_json::json;
use smasher_llm::types::{Provider, Request, models_for_provider};

/// Read all of stdin into a String.
fn read_stdin() -> Result<String, String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Build a Client from environment variables and verify at least one provider is configured.
fn build_client() -> Result<smasher_llm::client::Client, String> {
    let client = smasher_llm::client::Client::from_env();
    if client.registered_providers().is_empty() {
        return Err("no providers configured".to_string());
    }
    Ok(client)
}

/// Map an LLM error to a (error_message, error_type) pair for conformance JSON output.
fn error_to_type(err: &smasher_llm::types::Error) -> &'static str {
    match err {
        smasher_llm::types::Error::Authentication { .. } => "authentication",
        smasher_llm::types::Error::RateLimited { .. } => "rate_limited",
        smasher_llm::types::Error::Timeout { .. } => "timeout",
        smasher_llm::types::Error::ServerError { .. } => "server_error",
        smasher_llm::types::Error::InvalidRequest { .. } => "invalid_request",
        smasher_llm::types::Error::ModelNotFound { .. } => "model_not_found",
        smasher_llm::types::Error::ContentFiltered { .. } => "content_filtered",
        smasher_llm::types::Error::ContextLengthExceeded { .. } => "context_length_exceeded",
        smasher_llm::types::Error::StreamError { .. } => "stream_error",
        smasher_llm::types::Error::ResponseParse { .. } => "response_parse",
        smasher_llm::types::Error::Http { .. } => "http",
        smasher_llm::types::Error::Serialization { .. } => "serialization",
        smasher_llm::types::Error::ProviderNotConfigured { .. } => "provider_not_configured",
        smasher_llm::types::Error::Cancelled => "cancelled",
        smasher_llm::types::Error::AccessDenied { .. } => "access_denied",
        smasher_llm::types::Error::QuotaExceeded { .. } => "quota_exceeded",
        smasher_llm::types::Error::NetworkError { .. } => "network_error",
        smasher_llm::types::Error::Aborted { .. } => "aborted",
        smasher_llm::types::Error::InvalidToolCall { .. } => "invalid_tool_call",
        smasher_llm::types::Error::NoObjectGenerated { .. } => "no_object_generated",
        smasher_llm::types::Error::ConfigurationError { .. } => "configuration_error",
        smasher_llm::types::Error::Other { .. } => "other",
    }
}

/// Normalize message content fields: the conformance contract sends `"content": "string"`
/// but smasher expects `"content": [{"kind": "text", "text": "string"}]`.
fn normalize_message_content(value: &mut serde_json::Value) {
    if let Some(obj) = value.as_object_mut()
        && let Some(messages) = obj.get_mut("messages")
        && let Some(arr) = messages.as_array_mut()
    {
        for msg in arr.iter_mut() {
            if let Some(msg_obj) = msg.as_object_mut()
                && let Some(content) = msg_obj.get("content")
                && let Some(text) = content.as_str()
            {
                let text = text.to_string();
                msg_obj.insert(
                    "content".to_string(),
                    json!([{"kind": "text", "text": text}]),
                );
            }
        }
    }
}

/// Normalize tools from OpenAI format `{type: "function", function: {name, parameters}}`
/// to smasher format `{name, description, parameters}`.
fn normalize_tools(value: &mut serde_json::Value) {
    if let Some(obj) = value.as_object_mut()
        && let Some(tools) = obj.get_mut("tools")
        && let Some(arr) = tools.as_array_mut()
    {
        for tool in arr.iter_mut() {
            if let Some(tool_obj) = tool.as_object_mut()
                && tool_obj.get("type").and_then(|v| v.as_str()) == Some("function")
                && let Some(func) = tool_obj.remove("function")
                && let Some(func_obj) = func.as_object()
            {
                let mut new_tool = serde_json::Map::new();
                if let Some(name) = func_obj.get("name") {
                    new_tool.insert("name".to_string(), name.clone());
                }
                new_tool.insert(
                    "description".to_string(),
                    func_obj.get("description").cloned().unwrap_or(json!("")),
                );
                let params = func_obj
                    .get("parameters")
                    .cloned()
                    .unwrap_or(json!({"type": "object"}));
                new_tool.insert("parameters".to_string(), params);
                *tool = serde_json::Value::Object(new_tool);
            }
        }
    }
}

/// Adapt incoming conformance JSON: extract _test_endpoint, normalize message content
/// and tools, and map response_schema to response_format. Returns the cleaned-up Value
/// and the optional test endpoint.
fn adapt_request_json(mut value: serde_json::Value) -> (serde_json::Value, Option<String>) {
    let test_endpoint = value
        .as_object_mut()
        .and_then(|obj| obj.remove("_test_endpoint"))
        .and_then(|v| v.as_str().map(String::from));

    // Normalize string content in messages to array format.
    normalize_message_content(&mut value);

    // Normalize OpenAI-format tools to smasher format.
    normalize_tools(&mut value);

    // Map `response_schema` into our `ResponseFormat::JsonSchema` format.
    // Conformance sends `response_schema: {...}`, which we convert to
    // `response_format: {type: "json_schema", name: "response", schema: ..., strict: false}`.
    if let Some(obj) = value.as_object_mut()
        && let Some(schema) = obj.remove("response_schema")
    {
        obj.insert(
            "response_format".to_string(),
            json!({
                "type": "json_schema",
                "name": "response",
                "schema": schema,
                "strict": false
            }),
        );
    }

    (value, test_endpoint)
}

pub async fn client_from_env() -> i32 {
    match build_client() {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

pub async fn list_models() -> i32 {
    let mut models = Vec::new();

    for provider in [Provider::Anthropic, Provider::OpenAi, Provider::Gemini] {
        // Use Display impl for provider name (lowercase: "anthropic", "openai", "gemini").
        let provider_name = provider.to_string();

        for info in models_for_provider(provider) {
            models.push(json!({
                "id": info.id,
                "provider": provider_name,
                "display_name": info.display_name,
                "context_window": info.context_window,
                "max_output_tokens": info.max_output_tokens,
                "supports_images": info.supports_images,
                "supports_tool_use": info.supports_tool_use,
                "supports_streaming": info.supports_streaming,
            }));
        }
    }

    match serde_json::to_string(&models) {
        Ok(json_str) => {
            println!("{json_str}");
            0
        }
        Err(e) => {
            eprintln!("failed to serialize models: {e}");
            1
        }
    }
}

pub async fn complete() -> i32 {
    run_complete().await
}

pub async fn tool_call() -> i32 {
    run_complete().await
}

pub async fn generate_object() -> i32 {
    run_complete().await
}

/// Shared implementation for complete, tool_call, and generate_object subcommands.
async fn run_complete() -> i32 {
    let input = match read_stdin() {
        Ok(s) => s,
        Err(e) => {
            let out = json!({"error": e, "error_type": "stdin_read"});
            println!("{out}");
            return 1;
        }
    };

    let value: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(e) => {
            let out = json!({"error": e.to_string(), "error_type": "json_parse"});
            println!("{out}");
            return 1;
        }
    };

    let (value, _test_endpoint) = adapt_request_json(value);

    let request: Request = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(e) => {
            let out = json!({"error": e.to_string(), "error_type": "request_deserialize"});
            println!("{out}");
            return 1;
        }
    };

    let client = match build_client() {
        Ok(c) => c,
        Err(e) => {
            let out = json!({"error": e, "error_type": "client_init"});
            println!("{out}");
            return 1;
        }
    };

    match client.complete(request).await {
        Ok(response) => match serde_json::to_string(&response) {
            Ok(json_str) => {
                println!("{json_str}");
                0
            }
            Err(e) => {
                let out = json!({"error": e.to_string(), "error_type": "serialization"});
                println!("{out}");
                1
            }
        },
        Err(e) => {
            let out = json!({"error": e.to_string(), "error_type": error_to_type(&e)});
            println!("{out}");
            1
        }
    }
}

pub async fn stream() -> i32 {
    let input = match read_stdin() {
        Ok(s) => s,
        Err(e) => {
            let out = json!({"error": e, "error_type": "stdin_read"});
            println!("{out}");
            return 1;
        }
    };

    let value: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(e) => {
            let out = json!({"error": e.to_string(), "error_type": "json_parse"});
            println!("{out}");
            return 1;
        }
    };

    let (mut value, _test_endpoint) = adapt_request_json(value);

    // Ensure stream: true is set on the request.
    if let Some(obj) = value.as_object_mut() {
        obj.insert("stream".to_string(), serde_json::Value::Bool(true));
    }

    let request: Request = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(e) => {
            let out = json!({"error": e.to_string(), "error_type": "request_deserialize"});
            println!("{out}");
            return 1;
        }
    };

    let client = match build_client() {
        Ok(c) => c,
        Err(e) => {
            let out = json!({"error": e, "error_type": "client_init"});
            println!("{out}");
            return 1;
        }
    };

    let mut stream = match client.stream(&request).await {
        Ok(s) => s,
        Err(e) => {
            let out = json!({"error": e.to_string(), "error_type": error_to_type(&e)});
            println!("{out}");
            return 1;
        }
    };

    while let Some(event_result) = stream.next().await {
        match event_result {
            Ok(event) => match serde_json::to_string(&event) {
                Ok(json_str) => {
                    println!("{json_str}");
                }
                Err(e) => {
                    eprintln!("failed to serialize stream event: {e}");
                }
            },
            Err(e) => {
                let out = json!({"error": e.to_string(), "error_type": error_to_type(&e)});
                println!("{out}");
                return 1;
            }
        }
    }

    0
}
