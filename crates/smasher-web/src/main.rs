// ABOUTME: Binary entry point for the smasher-web pipeline dashboard server.
// ABOUTME: Loads environment, initializes tracing, and starts the axum server on port 21541.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Load .env from cwd first, then from SMASHER_WORKING_DIR if set.
    let _ = dotenvy::dotenv();
    if let Ok(dir) = std::env::var("SMASHER_WORKING_DIR") {
        let env_path = std::path::Path::new(&dir).join(".env");
        if env_path.exists() {
            let _ = dotenvy::from_path(&env_path);
        }
    }

    // Initialize tracing with RUST_LOG support.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("smasher_web=info,smasher_attractor=info,smasher_agent=info,smasher_llm=info,tower_http=info")
        }))
        .init();

    if let Err(e) = smasher_web::server::run().await {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}
