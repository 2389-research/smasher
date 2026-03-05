// ABOUTME: Conformance adapter CLI bridging smasher crates to AttractorBench test contract.
// ABOUTME: 15 subcommands across 3 tiers: LLM SDK, Agent Loop, Attractor Pipeline.

mod convert;
mod tier1;
mod tier2;
mod tier3;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// AttractorBench conformance adapter for smasher.
#[derive(Debug, Parser)]
#[command(name = "conformance")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
#[command(rename_all = "kebab-case")]
enum Command {
    /// Construct a client from environment variables.
    ClientFromEnv,

    /// Send a completion request (JSON from stdin).
    Complete,

    /// Stream a response (JSON from stdin, NDJSON to stdout).
    Stream,

    /// Process a tool call request (JSON from stdin).
    ToolCall,

    /// Generate structured output (JSON from stdin).
    GenerateObject,

    /// List known models.
    ListModels,

    /// Create an agent session.
    SessionCreate,

    /// Run the agentic loop on a task prompt (JSON from stdin).
    ProcessInput,

    /// Dispatch a single tool call (JSON from stdin).
    ToolDispatch,

    /// Inject a steering message (JSON from stdin).
    Steering,

    /// Run a short session and emit events (NDJSON to stdout).
    Events,

    /// Parse a DOT file to JSON AST.
    Parse {
        /// Path to the DOT file.
        dotfile: PathBuf,
    },

    /// Validate a DOT file and output diagnostics.
    Validate {
        /// Path to the DOT file.
        dotfile: PathBuf,
    },

    /// Execute a DOT pipeline with a mock backend.
    Run {
        /// Path to the DOT file.
        dotfile: PathBuf,
    },

    /// List registered handler types.
    ListHandlers,
}

fn main() {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    let code = runtime.block_on(async {
        match cli.command {
            Command::ClientFromEnv => tier1::client_from_env().await,
            Command::Complete => tier1::complete().await,
            Command::Stream => tier1::stream().await,
            Command::ToolCall => tier1::tool_call().await,
            Command::GenerateObject => tier1::generate_object().await,
            Command::ListModels => tier1::list_models().await,
            Command::SessionCreate => tier2::session_create().await,
            Command::ProcessInput => tier2::process_input().await,
            Command::ToolDispatch => tier2::tool_dispatch().await,
            Command::Steering => tier2::steering().await,
            Command::Events => tier2::events().await,
            Command::Parse { dotfile } => tier3::parse(&dotfile).await,
            Command::Validate { dotfile } => tier3::validate(&dotfile).await,
            Command::Run { dotfile } => tier3::run(&dotfile).await,
            Command::ListHandlers => tier3::list_handlers().await,
        }
    });

    std::process::exit(code);
}
