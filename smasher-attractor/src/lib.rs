// ABOUTME: DOT-based directed graph orchestrator for multi-stage AI workflows.
// ABOUTME: Parses DOT graphs, traverses nodes with handlers, and manages pipeline state.

pub mod artifact;
pub mod condition;
pub mod dot;
pub mod edge;
pub mod engine;
pub mod fidelity;
pub mod goals;
pub mod graph;
pub mod handler;
pub mod interviewer;
pub mod manager_handler;
pub mod parallel;
pub mod retry;
pub mod server;
pub mod state;
pub mod status;
pub mod stylesheet;
pub mod tool_handler;
pub mod transforms;
