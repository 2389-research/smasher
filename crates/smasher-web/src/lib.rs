// ABOUTME: Crate root for smasher-web, the HTMX pipeline dashboard.
// ABOUTME: Re-exports modules for the web server, SSE bridge, and API routes.

pub mod backend;
pub mod error;
pub mod routes;
pub mod server;
pub mod sse;
pub mod state;
