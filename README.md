<!-- ABOUTME: Project README with setup instructions, usage examples, and crate overview. -->
<!-- ABOUTME: Entry point for anyone cloning the repo for the first time. -->

# smasher

AI workflow orchestration in Rust. Write your pipeline as a DOT directed graph, point it at an LLM, and run it.

Based on [attractor](https://github.com/strongdm/attractor) by [strongDM](https://www.strongdm.com/), reimplemented from scratch in Rust. The original attractor defined the idea of DOT-graph-driven AI pipelines; smasher takes that concept and rebuilds it with a layered crate architecture, multi-provider LLM support, and a web dashboard.

Five crates, bottom to top:

| Crate | What it does |
|-------|-------------|
| `smasher-llm` | Talks to OpenAI, Anthropic, and Gemini through one client. Handles streaming, retries, the usual. |
| `smasher-agent` | Agent loop with tools (read, write, edit, shell, grep, glob). Steering rules, subagents, sandboxed execution. |
| `smasher-attractor` | The graph engine. Parses DOT, resolves node types from shapes, dispatches handlers, runs the pipeline. |
| `smasher-cli` | `smasher complete`, `smasher chat`, `smasher run`, `smasher serve`. |
| `smasher-web` | HTMX dashboard on port 21541. Live event stream, graph visualization, human Q&A. |

## Setup

You need Rust 1.92+ and at least one API key.

```bash
git clone https://github.com/2389-ai/smasher.git
cd smasher
cargo build --release
```

Set a provider key (any one works, or set all three):

```bash
export ANTHROPIC_API_KEY=sk-ant-...
export OPENAI_API_KEY=sk-...
export GEMINI_API_KEY=...
```

Or drop them in a `.env` file at the repo root.

## Usage

### One-shot completion

```bash
smasher complete "explain quicksort in three sentences"
smasher complete "explain quicksort" --json  # full response object
```

### Interactive chat

```bash
smasher chat
```

The chat REPL gives the agent all six tools (read, write, edit, shell, grep, glob), so it can work on files in the current directory.

### Run a pipeline

```bash
smasher run examples/hello-world.dot
smasher run examples/conditional.dot --var route=yes
smasher run examples/multi-step.dot --var model=claude-sonnet-4-20250514
```

Pipelines are standard DOT digraphs. Node shapes tell the engine what each node does:

- `circle` = start
- `doublecircle` = exit
- `box` = codergen (runs an LLM agent)
- `diamond` = conditional (branches on variables)
- `house` = manager (asks a human, routes on the answer)
- `parallelogram` = parallel fan-out
- `hexagon` = tool

See [`examples/`](examples/) for working pipelines and [`docs/dot-reference.md`](docs/dot-reference.md) for the full spec.

### Web dashboard

```bash
smasher serve
# opens on http://127.0.0.1:21541
```

Submit pipelines from the browser, watch events arrive over SSE, answer human gate questions through the web UI.

## Development

```bash
cargo check --workspace
cargo test --workspace       # ~2,400 tests
cargo clippy --workspace
```

There's a `Makefile` with shortcuts if you prefer that.

## Project structure

```
crates/
  smasher-llm/          # LLM client, provider catalog, types
  smasher-agent/        # Agent loop, tools, steering, events
  smasher-attractor/    # DOT parser, graph engine, handlers
  smasher-cli/          # CLI binary
  smasher-web/          # Web dashboard (axum + askama + HTMX)
docs/                   # Reference docs, quickstart guide
examples/               # Sample DOT pipelines
scripts/                # CI helpers
```

## Docs

- [Quickstart](docs/quickstart.md)
- [API reference](docs/api-reference.md)
- [DOT reference](docs/dot-reference.md)
- [Handler reference](docs/handler-reference.md)
- [CLI reference](docs/cli-reference.md)
- [Config reference](docs/config-reference.md)

## License

See [LICENSE](LICENSE) for details.
