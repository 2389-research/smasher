// ABOUTME: Tier 3 conformance subcommands for the Attractor Pipeline layer.
// ABOUTME: parse, validate, run, list-handlers.

use std::path::Path;

pub async fn parse(_dotfile: &Path) -> i32 {
    eprintln!("parse: not yet implemented");
    1
}

pub async fn validate(_dotfile: &Path) -> i32 {
    eprintln!("validate: not yet implemented");
    1
}

pub async fn run(_dotfile: &Path) -> i32 {
    eprintln!("run: not yet implemented");
    1
}

pub async fn list_handlers() -> i32 {
    eprintln!("list-handlers: not yet implemented");
    1
}
