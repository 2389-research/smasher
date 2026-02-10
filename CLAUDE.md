# Smasher - AI Workflow Orchestration in Rust

## Names
- AI: **TURBOSAURUS REX** (or Turbo)
- Human: **Harp-Dogg the Annihilator**

## What is this?

A Rust implementation of [strongdm/attractor](https://github.com/strongdm/attractor) — three-layer AI workflow orchestration:

1. **smasher-llm** — Unified LLM client (OpenAI Responses API, Anthropic Messages API, Gemini API)
2. **smasher-agent** — Programmable coding agent loop with tools, steering, subagents
3. **smasher-attractor** — DOT-based directed graph pipeline orchestrator
4. **smasher-cli** — CLI binary (`smasher complete`, `smasher chat`, `smasher run`)
5. **smasher-web** — HTMX dashboard for pipeline execution (axum + askama, port 21541)

## Architecture

Cargo workspace, 5 crates under `crates/`. Each layer depends only on the ones below it.

## Key Rust Patterns

- `ContentPart` as tagged enum with `#[serde(tag = "kind")]`
- Streaming via `Pin<Box<dyn Stream<Item = Result<StreamEvent, Error>> + Send>>`
- Flat `thiserror` enum with `retryable()` method
- `async-trait` for provider/handler/executor traits
- `tokio::broadcast` for event delivery
- `CancellationToken` for abort
- `winnow` for DOT parsing

## Build & Test

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace
```

## File Conventions

- All files start with two `ABOUTME:` comment lines
- Tests go in same file (`#[cfg(test)] mod tests`) or in `tests/` dir for integration tests
- TDD: write test first, then implementation
