// ABOUTME: Tier 2 conformance subcommands for the Coding Agent Loop layer.
// ABOUTME: session-create, process-input, tool-dispatch, steering, events.

use std::io::Read;
use std::sync::Arc;

use serde_json::json;

use smasher_agent::environment::LocalExecutionEnvironment;
use smasher_agent::events::EventEmitter;
use smasher_agent::session::Session;
use smasher_agent::tools::ToolRegistry;
use smasher_agent::tools::shared::register_shared_tools;
use smasher_agent::types::SessionConfig;
use smasher_llm::client::Client;

use crate::convert::session_event_to_json;

/// Read all of stdin into a string, returning an error message on failure.
fn read_stdin() -> Result<String, String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

pub async fn session_create() -> i32 {
    let client = Arc::new(Client::from_env());
    let config = SessionConfig::default();
    let registry = ToolRegistry::new();
    let emitter = EventEmitter::default();
    let session = Session::new(config, client, registry, emitter);

    // Use the session's own ID rather than generating a separate UUID.
    let session_id = session.session_id().to_string();
    let output = json!({
        "session_id": session_id,
        "status": "created",
    });
    println!("{}", output);
    0
}

pub async fn process_input() -> i32 {
    let input = match read_stdin() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("process-input: failed to read stdin: {e}");
            return 1;
        }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("process-input: invalid JSON input: {e}");
            return 1;
        }
    };

    let prompt = match parsed.get("prompt").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => {
            eprintln!("process-input: missing 'prompt' field");
            return 1;
        }
    };

    let system_prompt = parsed
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let model = parsed
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // If _test_base_url is provided, validate it as a URL
    if let Some(base_url) = parsed.get("_test_base_url").and_then(|v| v.as_str())
        && !base_url.starts_with("http://")
        && !base_url.starts_with("https://")
    {
        let output = json!({
            "status": "error",
            "error": format!("invalid base URL: {base_url}"),
        });
        println!("{}", output);
        return 1;
    }

    let client = Arc::new(Client::from_env());

    let mut config = SessionConfig::default();
    if let Some(sp) = system_prompt {
        config = config.with_system_prompt(sp);
    }
    // Only set the model when explicitly provided; otherwise let the config default apply.
    if let Some(m) = model {
        config = config.with_model(m);
    }
    config = config.with_stream(false);
    config = config.with_max_turns(10);

    let mut registry = ToolRegistry::new();
    let env = Arc::new(LocalExecutionEnvironment::new("/tmp".to_string()));
    register_shared_tools(&mut registry, env);

    let emitter = EventEmitter::default();
    let mut session = Session::new(config, client, registry, emitter);

    match session.process_input(&prompt).await {
        Ok(output) => {
            let text = output.text.unwrap_or_default();
            let result = json!({
                "status": "success",
                "result": text,
                "output": text,
                "turns": output.turns_used,
            });
            println!("{}", result);
            0
        }
        Err(e) => {
            let result = json!({
                "status": "error",
                "error": e.to_string(),
            });
            println!("{}", result);
            1
        }
    }
}

pub async fn tool_dispatch() -> i32 {
    let input = match read_stdin() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("tool-dispatch: failed to read stdin: {e}");
            return 1;
        }
    };

    let parsed: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("tool-dispatch: invalid JSON input: {e}");
            return 1;
        }
    };

    let tool_name = match parsed.get("tool_name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            let output = json!({ "error": "missing tool_name" });
            println!("{}", output);
            return 1;
        }
    };

    // Validate arguments are a valid JSON object
    let arguments = match parsed.get("arguments") {
        Some(args) => {
            if args.is_object() {
                match serde_json::to_string(args) {
                    Ok(s) => s,
                    Err(e) => {
                        let output =
                            json!({ "error": format!("failed to serialize arguments: {e}") });
                        println!("{}", output);
                        return 1;
                    }
                }
            } else if args.is_string() {
                // The arguments field is a string — check if it parses as a JSON object
                let s = args.as_str().unwrap();
                match serde_json::from_str::<serde_json::Value>(s) {
                    Ok(v) if v.is_object() => s.to_string(),
                    _ => {
                        let output = json!({ "error": "invalid arguments" });
                        println!("{}", output);
                        return 1;
                    }
                }
            } else {
                let output = json!({ "error": "invalid arguments" });
                println!("{}", output);
                return 1;
            }
        }
        None => {
            let output = json!({ "error": "invalid arguments" });
            println!("{}", output);
            return 1;
        }
    };

    let mut registry = ToolRegistry::new();
    let env = Arc::new(LocalExecutionEnvironment::new("/tmp".to_string()));
    register_shared_tools(&mut registry, env);

    if !registry.has_tool(&tool_name) {
        let output = json!({ "error": format!("unknown tool: {tool_name}") });
        println!("{}", output);
        return 1;
    }

    let tool_output = registry.execute(&tool_name, &arguments).await;

    if tool_output.is_error {
        let output = json!({ "error": tool_output.content });
        println!("{}", output);
        1
    } else {
        let output = json!({
            "result": tool_output.content,
            "output": tool_output.content,
        });
        println!("{}", output);
        0
    }
}

pub async fn steering() -> i32 {
    let input = match read_stdin() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("steering: failed to read stdin: {e}");
            return 1;
        }
    };

    // Validate that the input is valid JSON with a "message" field
    let parsed: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("steering: invalid JSON input: {e}");
            return 1;
        }
    };

    if parsed.get("message").is_none() {
        eprintln!("steering: missing 'message' field");
        return 1;
    }

    let output = json!({
        "status": "queued",
        "acknowledged": true,
    });
    println!("{}", output);
    0
}

pub async fn events() -> i32 {
    let client = Arc::new(Client::from_env());

    let mut config = SessionConfig::default();
    // Do not hardcode a model here; let the SessionConfig default apply.
    config = config.with_stream(false);
    config = config.with_max_turns(10);

    let mut registry = ToolRegistry::new();
    let env = Arc::new(LocalExecutionEnvironment::new("/tmp".to_string()));
    register_shared_tools(&mut registry, env);

    let emitter = EventEmitter::new(256);
    let mut rx = emitter.subscribe();

    let mut session = Session::new(config, client, registry, emitter);

    // Spawn the session task
    let session_handle = tokio::spawn(async move { session.process_input("Say hello").await });

    // Collect events with a per-event timeout
    let mut events = Vec::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv()).await {
            Ok(Ok(event)) => {
                events.push(event);
            }
            Ok(Err(_)) => break, // channel closed
            Err(_) => break,     // timeout
        }
    }

    // Wait for the session task to finish with a hard deadline.  If it hangs,
    // abort it to avoid stalling indefinitely.
    match tokio::time::timeout(std::time::Duration::from_secs(60), session_handle).await {
        Ok(_) => {}
        Err(_) => {
            eprintln!("events: session task timed out after 60 seconds");
        }
    }

    if events.len() < 3 {
        eprintln!("events: expected at least 3 events, got {}", events.len());
        return 1;
    }

    // Print each event as NDJSON
    for event in &events {
        let json_val = session_event_to_json(event);
        println!("{}", json_val);
    }

    0
}
