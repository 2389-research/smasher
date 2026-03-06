// ABOUTME: Interactive agent REPL subcommand with tool execution and event streaming.
// ABOUTME: Reads user input from stdin, drives a Session loop, and prints assistant responses.

use std::io::{BufRead, Write};
use std::sync::Arc;

use clap::Args;

use smasher_agent::environment::LocalExecutionEnvironment;
use smasher_agent::events::EventEmitter;
use smasher_agent::session::Session;
use smasher_agent::tools::ToolRegistry;
use smasher_agent::tools::shared::register_shared_tools;
use smasher_agent::types::SessionConfig;

use crate::error::CliError;

/// Interactive agent chat session with tool access.
#[derive(Debug, Args)]
pub struct ChatArgs {
    /// Model identifier.
    #[arg(long, default_value = "claude-sonnet-4-20250514")]
    pub model: String,

    /// Maximum agentic turns before the session ends.
    #[arg(long, default_value = "100")]
    pub max_turns: u32,

    /// System prompt override.
    #[arg(long)]
    pub system: Option<String>,

    /// Working directory for tool operations.
    #[arg(long)]
    pub working_dir: Option<String>,
}

pub async fn run(args: ChatArgs) -> Result<(), CliError> {
    let client = smasher_llm::client::Client::from_env();
    if client.registered_providers().is_empty() {
        return Err(CliError::Other(
            "no API keys found. Set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY.".into(),
        ));
    }
    let client = Arc::new(client);

    let working_dir = args
        .working_dir
        .unwrap_or_else(|| std::env::current_dir().unwrap().display().to_string());

    let env = Arc::new(LocalExecutionEnvironment::new(working_dir.clone()));

    let mut registry = ToolRegistry::new();
    register_shared_tools(&mut registry, env);

    let emitter = EventEmitter::default();
    let mut rx = emitter.subscribe();

    let mut config = SessionConfig::default()
        .with_model(&args.model)
        .with_max_turns(args.max_turns)
        .with_working_directory(&working_dir);

    if let Some(system) = &args.system {
        config = config.with_system_prompt(system);
    }

    let mut session = Session::new(config, client, registry, emitter);

    // Spawn a task to print events (tool calls, etc.) to stderr.
    tokio::spawn(async move {
        use smasher_agent::types::SessionEvent;
        loop {
            match rx.recv().await {
                Ok(SessionEvent::ToolCallStarted {
                    tool_name,
                    input_preview,
                    ..
                }) => {
                    if input_preview.is_empty() {
                        eprintln!("[tool] {tool_name}...");
                    } else {
                        eprintln!("[tool] {tool_name} {input_preview}...");
                    }
                }
                Ok(SessionEvent::ToolCallCompleted {
                    tool_name,
                    is_error,
                    duration_ms,
                    ..
                }) => {
                    let status = if is_error { "ERR" } else { "ok" };
                    eprintln!("[tool] {tool_name} {status} ({duration_ms}ms)");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("[warn] missed {n} events");
                }
                _ => {}
            }
        }
    });

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    eprintln!("smasher chat (model: {}, dir: {})", args.model, working_dir);
    eprintln!("Type a message, or \"exit\"/\"quit\" to leave.\n");

    loop {
        print!("> ");
        stdout.flush()?;

        let mut line = String::new();
        let bytes_read = stdin.lock().read_line(&mut line)?;

        // EOF
        if bytes_read == 0 {
            eprintln!("\n[eof]");
            break;
        }

        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        if matches!(trimmed, "exit" | "quit" | "/quit") {
            break;
        }

        match session.process_input(trimmed).await {
            Ok(output) => {
                if let Some(text) = &output.text {
                    println!("\n{text}\n");
                } else {
                    println!("\n[no text response]\n");
                }
            }
            Err(e) => {
                eprintln!("[error] {e}");
                if !session.is_active() {
                    eprintln!("[session ended]");
                    break;
                }
            }
        }
    }

    let usage = session.total_usage();
    eprintln!(
        "\n[session] {} turns, {} input tokens, {} output tokens",
        session.turn_count(),
        usage.input_tokens,
        usage.output_tokens
    );

    Ok(())
}
