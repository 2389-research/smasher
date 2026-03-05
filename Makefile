.PHONY: all check build test clippy lint fmt clean doc test-cli watch run-complete run-chat scenarios stats test-verbose test-single coverage bench pre-commit

all: fmt-check lint check test

# ── Build ────────────────────────────────────────────────────────────
check:
	cargo check --workspace

build:
	cargo build --workspace
	mkdir -p bin
	cp target/debug/conformance bin/conformance

release:
	cargo build --workspace --release
	mkdir -p bin
	cp target/release/conformance bin/conformance

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

lint:
	cargo clippy --workspace -- -D warnings

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

# ── Development ──────────────────────────────────────────────────────
watch:
	cargo watch -x 'test --workspace' -x 'clippy --workspace'

run-complete:
	cargo run -p smasher-cli -- complete "Hello, world!"

run-chat:
	cargo run -p smasher-cli -- chat

# ── Scenarios ────────────────────────────────────────────────────────
scenarios: release
	@bash .scratch/run-scenarios.sh

# ── Stats ────────────────────────────────────────────────────────────
stats:
	@echo "=== Codebase Stats ==="
	@echo "Lines of Rust:"
	@find . -name '*.rs' -not -path './target/*' | xargs wc -l | tail -1
	@echo ""
	@echo "Test count:"
	@cargo test --workspace 2>&1 | grep "^test result:" | awk '{sum += $$3} END {print sum " tests"}'
	@echo ""
	@echo "Crate count:"
	@ls -d */Cargo.toml | wc -l | tr -d ' '

# ── Extended Testing ────────────────────────────────────────────────
test-verbose:
	cargo test --workspace -- --nocapture

CRATE ?= smasher-llm
test-single:
	cargo test -p $(CRATE)

# Requires: cargo install cargo-llvm-cov
coverage:
	cargo llvm-cov --workspace --lcov --output-path lcov.info

# Placeholder: add benchmarks under benches/ when ready
bench:
	@echo "No benchmarks configured yet. Add bench targets under benches/ directories."

pre-commit: fmt-check lint test
	@echo "All pre-commit checks passed."
