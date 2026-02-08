.PHONY: all check build test clippy lint fmt clean doc test-cli

all: check clippy test

# ── Build ────────────────────────────────────────────────────────────
check:
	cargo check --workspace

build:
	cargo build --workspace

release:
	cargo build --workspace --release

# ── Quality ──────────────────────────────────────────────────────────
test:
	cargo test --workspace

test-llm:
	cargo test -p smasher-llm

test-agent:
	cargo test -p smasher-agent

test-attractor:
	cargo test -p smasher-attractor

test-cli:
	cargo check -p smasher-cli
	cargo clippy -p smasher-cli -- -D warnings

clippy:
	cargo clippy --workspace -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

lint: clippy fmt-check

# ── Docs ─────────────────────────────────────────────────────────────
doc:
	cargo doc --workspace --no-deps

doc-open:
	cargo doc --workspace --no-deps --open

# ── Cleanup ──────────────────────────────────────────────────────────
clean:
	cargo clean

# ── CI (run everything a PR check would) ─────────────────────────────
ci: fmt-check clippy test
	@echo "All CI checks passed."
