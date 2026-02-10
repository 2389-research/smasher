// ABOUTME: CLI subcommand that starts the smasher-web dashboard server.
// ABOUTME: Accepts --port, --model, and --working-dir to configure the web UI.

use std::path::PathBuf;

use clap::Args;
use smasher_web::server::{DEFAULT_PORT, ServerConfig};

use crate::error::CliError;

/// Arguments for the `serve` subcommand.
#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Port to listen on.
    #[arg(long, short, default_value_t = DEFAULT_PORT)]
    pub port: u16,

    /// Default LLM model for pipeline execution.
    #[arg(long, short)]
    pub model: Option<String>,

    /// Working directory for agent file operations.
    #[arg(long, short)]
    pub working_dir: Option<PathBuf>,
}

pub async fn run(args: ServeArgs) -> Result<(), CliError> {
    let defaults = ServerConfig::default();

    let model = args.model.unwrap_or(defaults.model);

    let working_dir = match args.working_dir {
        Some(dir) => {
            let dir_str = dir.display().to_string();
            std::path::Path::new(&dir_str)
                .canonicalize()
                .map(|p| p.display().to_string())
                .unwrap_or(dir_str)
        }
        None => defaults.working_dir,
    };

    let config = ServerConfig {
        port: args.port,
        model,
        working_dir,
    };

    smasher_web::server::run_with_config(config)
        .await
        .map_err(|e| CliError::Web(e.to_string()))
}
