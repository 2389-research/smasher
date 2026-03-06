<!-- ABOUTME: Complete CLI reference for the smasher binary. -->
<!-- ABOUTME: Documents all subcommands, flags, options, exit codes, and usage examples. -->

# CLI Reference

## Overview

The `smasher` binary provides nine subcommands for AI workflow orchestration:

| Subcommand | Purpose |
|------------|---------|
| `complete` | Send a one-shot prompt to an LLM |
| `chat` | Start an interactive agent chat session |
| `run` | Execute a DOT-based pipeline |
| `resume` | Resume a checkpointed pipeline run |
| `render` | Render a DOT pipeline file to SVG or PNG |
| `serve` | Start the web dashboard server |
| `ingest` | Convert English requirements into a DOT pipeline file using an LLM |
| `archive` | Create a compressed archive of a run directory |
| `lint` | Validate a DOT pipeline file with lint rules |

## Global Flags

These flags apply to all subcommands:

| Flag | Short | Description |
|------|-------|-------------|
| `--verbose` | `-v` | Enable verbose logging (writes to stderr) |
| `--env-file <PATH>` | | Load environment variables from a specific `.env` file |
| `--help` | `-h` | Print help information |
| `--version` | `-V` | Print version information |

### Environment File Loading

By default, smasher loads a `.env` file from the current working directory before
parsing CLI arguments, so environment variables are available for any default-value
logic. If `--env-file` is specified, it loads that file and overrides any variables
already set by the default `.env`.

## `smasher complete`

Send a one-shot prompt to an LLM and stream the response to stdout.

```
smasher complete [OPTIONS] <PROMPT>
```

### Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `<PROMPT>` | Yes | The prompt text to send to the model |

### Options

| Flag | Description | Default |
|------|-------------|---------|
| `--file <PATH>` | Read prompt from a file instead of the argument | |
| `--model <MODEL>` | LLM model identifier | `claude-sonnet-4-20250514` |
| `--max-tokens <N>` | Maximum tokens in the response | |
| `--temperature <FLOAT>` | Sampling temperature (0.0 to 2.0) | |
| `--system <TEXT>` | System prompt to prepend | |
| `--json` | Output the full JSON response instead of streaming text | `false` |

### Examples

```bash
# Simple one-shot prompt
smasher complete "Explain the Rust borrow checker in two sentences"

# Read prompt from a file with a specific model
smasher complete --file prompt.txt --model gpt-4o

# Get JSON response with custom temperature
smasher complete --json --temperature 0.7 "Write a haiku about compilers"

# With a system prompt and token limit
smasher complete --system "You are a Rust expert." --max-tokens 500 "What is Pin?"
```

## `smasher chat`

Start an interactive agent chat session with tool access. The session continues
until the turn limit is reached or the user exits.

```
smasher chat [OPTIONS]
```

### Options

| Flag | Description | Default |
|------|-------------|---------|
| `--model <MODEL>` | LLM model identifier | `claude-sonnet-4-20250514` |
| `--max-turns <N>` | Maximum conversation turns | `100` |
| `--system <TEXT>` | System prompt for the agent | |
| `--working-dir <PATH>` | Working directory for tool execution | |

### Examples

```bash
# Start a chat with default settings
smasher chat

# Use a specific model with a custom system prompt
smasher chat --model gpt-4o --system "You are a code reviewer."

# Limit turns and set working directory
smasher chat --max-turns 20 --working-dir ./my-project
```

## `smasher run`

Execute a DOT-based pipeline. The pipeline graph is read from a `.dot` file,
and nodes are executed according to the directed edges.

```
smasher run [OPTIONS] <PIPELINE>
```

### Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `<PIPELINE>` | Yes | Path to the DOT pipeline file |

### Options

| Flag | Description | Default |
|------|-------------|---------|
| `--var <KEY=VALUE>` | Set a pipeline variable (repeatable) | |
| `--model <MODEL>` | LLM model identifier | `claude-sonnet-4-20250514` |
| `--max-steps <N>` | Maximum engine execution steps | `1000` |
| `--stylesheet <PATH>` | Path to a stylesheet file for node configuration | |

### Examples

```bash
# Run a pipeline
smasher run pipeline.dot

# Pass variables into the pipeline
smasher run --var input="hello world" --var mode=fast pipeline.dot

# Use a stylesheet and limit steps
smasher run --stylesheet style.ss --max-steps 500 pipeline.dot

# Verbose output with a specific model
smasher -v run --model claude-sonnet-4-20250514 pipeline.dot
```

## Exit Codes

| Code | Category | Meaning |
|------|----------|---------|
| `0` | Success | The command completed without error |
| `1` | Other | General / uncategorized error |
| `2` | LLM | LLM provider error (API failure, rate limit, etc.) |
| `3` | Session | Agent session error (tool failure, turn limit, etc.) |
| `4` | Engine | Pipeline engine error (max steps, handler failure, etc.) |
| `5` | Parse / Resolution / Stylesheet | DOT parse error, node resolution error, or stylesheet error |
| `6` | I/O | File system or network I/O error |

## Logging

Logging is controlled by the `--verbose` flag and the `RUST_LOG` environment variable:

- Without `--verbose`: only warnings and errors are logged.
- With `--verbose`: debug-level logging is enabled.
- If `RUST_LOG` is set, it takes precedence over the `--verbose` flag.

All log output goes to **stderr**, so stdout remains clean for command output
and can be piped safely.
