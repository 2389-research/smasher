// ABOUTME: Unified CLI error type that wraps errors from all three smasher crate layers.
// ABOUTME: Provides Display and process exit code mapping for user-friendly error reporting.

use smasher_agent::session::SessionError;
use smasher_attractor::dot::parser::ParseError;
use smasher_attractor::engine::EngineError;
use smasher_attractor::graph::ResolutionError;
use smasher_attractor::state::StateError;
use smasher_attractor::stylesheet::StylesheetError;

/// Unified error type for the smasher CLI.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("{0}")]
    Llm(#[from] smasher_llm::types::Error),

    #[error("{0}")]
    Session(#[from] SessionError),

    #[error("{0}")]
    Engine(#[from] EngineError),

    #[error("{0}")]
    Resolution(#[from] ResolutionError),

    #[error("{0}")]
    Stylesheet(#[from] StylesheetError),

    #[error("DOT parse error: {0}")]
    DotParse(#[from] ParseError),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("state error: {0}")]
    State(#[from] StateError),

    #[error("web server error: {0}")]
    Web(String),

    #[error("{0}")]
    Other(String),
}

impl CliError {
    /// Map the error to a process exit code.
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Llm(_) => 2,
            CliError::Session(_) => 3,
            CliError::Engine(_) => 4,
            CliError::Resolution(_) | CliError::DotParse(_) | CliError::Stylesheet(_) => 5,
            CliError::Io(_) => 6,
            CliError::Web(_) => 7,
            CliError::State(_) => 8,
            CliError::Other(_) => 1,
        }
    }
}
