// ABOUTME: Axum router assembly, binding, and graceful shutdown for the web server.
// ABOUTME: Configures routes, static file serving, CORS, and listens on port 21541.

use std::net::SocketAddr;

use axum::Router;
use tower_http::services::ServeDir;

use crate::state::AppState;

/// Default port for the smasher-web dashboard (5MA5H in leet).
pub const DEFAULT_PORT: u16 = 21541;

/// Build the complete axum router with all routes and middleware.
pub fn build_router(state: AppState) -> Router {
    let api_routes = crate::routes::api::router();
    let page_routes = crate::routes::pages::router();
    let question_routes = crate::routes::questions::router();

    // Resolve static dir relative to the crate manifest, not the cwd.
    let static_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static");

    Router::new()
        .merge(page_routes)
        .merge(api_routes)
        .merge(question_routes)
        .nest_service("/static", ServeDir::new(static_dir))
        .with_state(state)
}

/// Configuration for the web dashboard server.
pub struct ServerConfig {
    pub port: u16,
    pub host: [u8; 4],
    pub model: String,
    pub data_dir: String,
}

/// Return the default data directory for smasher (~/.smasher).
///
/// Override with SMASHER_DATA_DIR env var.
pub fn default_data_dir() -> String {
    if let Ok(dir) = std::env::var("SMASHER_DATA_DIR") {
        return dir;
    }
    dirs::home_dir()
        .map(|h| h.join(".smasher").display().to_string())
        .unwrap_or_else(|| ".smasher".into())
}

impl Default for ServerConfig {
    fn default() -> Self {
        let port = std::env::var("SMASHER_WEB_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(DEFAULT_PORT);

        let host = match std::env::var("SMASHER_WEB_HOST").ok().as_deref() {
            Some("0.0.0.0") => [0, 0, 0, 0],
            _ => [127, 0, 0, 1],
        };

        let model =
            std::env::var("SMASHER_MODEL").unwrap_or_else(|_| "claude-sonnet-4-20250514".into());

        let data_dir = default_data_dir();

        Self {
            port,
            host,
            model,
            data_dir,
        }
    }
}

/// Start the web server with default configuration from env vars.
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    run_with_config(ServerConfig::default()).await
}

/// Start the web server with explicit configuration.
pub async fn run_with_config(config: ServerConfig) -> Result<(), Box<dyn std::error::Error>> {
    let client = smasher_llm::client::Client::from_env();
    if client.registered_providers().is_empty() {
        return Err(
            "no API keys found. Set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY.".into(),
        );
    }

    tracing::info!(data_dir = %config.data_dir, model = %config.model, "agent configuration");

    let state = AppState::new(client, config.model, config.data_dir);
    let app = build_router(state);

    let addr = SocketAddr::from((config.host, config.port));
    tracing::info!(%addr, "smasher dashboard starting");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install ctrl+c handler");
    tracing::info!("shutdown signal received, draining connections...");
    // SSE streams for active runs stay open indefinitely, which prevents
    // graceful shutdown from completing. Force exit after a short grace period.
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        tracing::info!("shutdown timeout reached, exiting");
        std::process::exit(0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_is_21541() {
        assert_eq!(DEFAULT_PORT, 21541);
    }
}
