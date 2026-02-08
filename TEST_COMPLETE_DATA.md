# Test Scenarios - Complete Structured Data

## All 61 Test Scenarios

### ENVIRONMENT CONFIGURATION (6 tests)

#### 1. env-default-loading
- **Description**: Default .env file loads API keys at startup
- **Given**: A .env file with ANTHROPIC_API_KEY in the working directory
- **When**: smasher complete is run without env vars set in shell
- **Then**: The API key from .env is used and the completion succeeds
- **Validates**: dotenvy integration loads .env before provider init

#### 2. env-file-flag
- **Description**: --env-file loads a custom .env path
- **Given**: A custom .env file at a non-standard path
- **When**: smasher --env-file <path> complete is run
- **Then**: Environment variables from the custom file are loaded
- **Validates**: --env-file CLI arg passes path to dotenvy::from_path()

#### 3. env-file-missing-error
- **Description**: Missing --env-file produces clean error
- **Given**: --env-file pointing to a nonexistent path
- **When**: smasher --env-file /bad/path complete is run
- **Then**: Exit code 1 with 'failed to load env file' message
- **Validates**: Error handling for invalid --env-file path

#### 4. env-shell-precedence
- **Description**: Shell env vars take precedence over .env
- **Given**: A shell env var overriding a key also present in .env
- **When**: smasher complete is run with conflicting key values
- **Then**: The shell env var value is used (dotenvy does not override existing vars)
- **Validates**: dotenvy precedence behavior - existing vars are preserved

#### 5. env-parent-walk
- **Description**: dotenvy finds .env in parent directories
- **Given**: Running from a subdirectory of a project with .env in the parent
- **When**: smasher complete is run from the subdirectory
- **Then**: The .env from the parent directory is loaded
- **Validates**: dotenvy's directory walk behavior finds .env up the tree

#### 6. env-no-keys-error
- **Description**: No API keys produces clear error
- **Given**: No .env file in any parent directory and no env vars set
- **When**: smasher complete is run
- **Then**: Exit code 1 with helpful error about setting API keys
- **Validates**: Graceful failure when no provider credentials available

---

### LLM COMPLETION API (10 tests)

#### 7. complete-anthropic-streaming
- **Description**: Anthropic streaming completion works end-to-end
- **Given**: Valid ANTHROPIC_API_KEY in environment
- **When**: smasher complete --model claude-haiku-4-5 is run with a prompt
- **Then**: Streaming text deltas are written to stdout
- **Validates**: Anthropic Messages API streaming integration

#### 8. complete-openai-streaming
- **Description**: OpenAI streaming completion works end-to-end
- **Given**: Valid OPENAI_API_KEY in environment
- **When**: smasher complete --model gpt-4.1-nano is run with a prompt
- **Then**: Streaming text deltas are written to stdout
- **Validates**: OpenAI Responses API streaming integration

#### 9. complete-gemini-streaming
- **Description**: Gemini streaming completion works end-to-end
- **Given**: Valid GEMINI_API_KEY in environment
- **When**: smasher complete --model gemini-2.0-flash is run with a prompt
- **Then**: Streaming text deltas are written to stdout (or rate-limit error with exit 2)
- **Validates**: Gemini API streaming integration and rate-limit error handling

#### 10. complete-json-mode
- **Description**: --json returns full structured Response
- **Given**: Any valid provider API key
- **When**: smasher complete --json is run
- **Then**: stdout contains valid JSON with 'content' field (Response struct)
- **Validates**: JSON output mode serializes Response correctly

#### 11. complete-system-prompt
- **Description**: --system prompt modifies model behavior
- **Given**: A system prompt instructing specific behavior
- **When**: smasher complete --system <text> is run
- **Then**: Model output reflects the system prompt instructions
- **Validates**: System prompt is correctly forwarded to provider API

#### 12. complete-temperature
- **Description**: --temperature 0 gives deterministic output
- **Given**: Temperature set to 0
- **When**: Same prompt is run twice with --temperature 0
- **Then**: Both outputs contain the same core content
- **Validates**: Temperature parameter is forwarded to provider API

#### 13. complete-file-input
- **Description**: --file reads prompt from a file
- **Given**: A text file containing a prompt
- **When**: smasher complete --file <path> is run
- **Then**: The file content is used as the prompt
- **Validates**: File input mode reads and uses file content as prompt

#### 14. complete-missing-file-error
- **Description**: --file with missing file produces I/O error
- **Given**: --file pointing to a nonexistent path
- **When**: smasher complete --file /bad/path is run
- **Then**: Exit code 6 (I/O error)
- **Validates**: File not found error handling with correct exit code

#### 15. complete-no-prompt-error
- **Description**: No prompt and no --file produces usage error
- **Given**: Neither positional prompt nor --file provided
- **When**: smasher complete is run with no input
- **Then**: Exit code 1 (usage error from clap or validation)
- **Validates**: Input validation requires prompt or --file

#### 16. complete-verbose-logging
- **Description**: --verbose enables debug logs on stderr
- **Given**: --verbose flag is set
- **When**: smasher --verbose complete is run
- **Then**: stderr contains debug/trace level log lines
- **Validates**: Tracing subscriber respects verbose flag, logs to stderr

---

### INTERACTIVE CHAT AGENT (7 tests)

#### 17. chat-read-file
- **Description**: Chat agent reads files via read_file tool
- **Given**: A file exists in the working directory
- **When**: User asks agent to read a specific file
- **Then**: Agent uses read_file tool and reports file contents
- **Validates**: read_file tool integration in agent loop

#### 18. chat-write-file
- **Description**: Chat agent creates files via write_file tool
- **Given**: A working directory
- **When**: User asks agent to create a file with specific content
- **Then**: File is created on disk with correct content
- **Validates**: write_file tool creates files and parent directories

#### 19. chat-shell-command
- **Description**: Chat agent executes shell commands
- **Given**: A working directory
- **When**: User asks agent to run a shell command
- **Then**: Agent executes command and reports output
- **Validates**: shell tool executes commands and captures output

#### 20. chat-edit-file
- **Description**: Chat agent edits files via exact string replacement
- **Given**: A file with known content in the working directory
- **When**: User asks agent to replace a specific string
- **Then**: File is modified with the replacement applied
- **Validates**: edit_file tool performs exact string replacement

#### 21. chat-grep
- **Description**: Chat agent searches file contents via grep tool
- **Given**: Files with unique markers in the working directory
- **When**: User asks agent to search for a pattern
- **Then**: Agent finds the file containing the pattern
- **Validates**: grep tool searches with regex and reports matches

#### 22. chat-glob
- **Description**: Chat agent finds files via glob_files tool
- **Given**: Multiple files with different extensions in working directory
- **When**: User asks agent to find files matching a glob pattern
- **Then**: Agent lists matching files
- **Validates**: glob_files tool matches file patterns correctly

#### 23. chat-eof-exit
- **Description**: Chat session ends cleanly on EOF
- **Given**: An empty input stream (immediate EOF)
- **When**: smasher chat is run
- **Then**: Exit code 0, session ends gracefully
- **Validates**: EOF/Ctrl-D handling in interactive REPL

#### 24. chat-max-turns
- **Description**: --max-turns limits agent execution
- **Given**: A complex task requiring many tool calls
- **When**: smasher chat --max-turns 1 is run
- **Then**: Agent stops after the turn limit, preventing runaway sessions
- **Validates**: Turn limit enforcement in session loop

---

### PIPELINE EXECUTION (11 tests)

#### 25. pipeline-minimal
- **Description**: Minimal start-to-exit pipeline completes
- **Given**: A DOT file with circle (start) → doublecircle (exit)
- **When**: smasher run is executed
- **Then**: Exit 0, context JSON contains _started and _completed
- **Validates**: Basic pipeline execution with start/exit handlers

#### 26. pipeline-variables
- **Description**: --var injects variables into pipeline context
- **Given**: A minimal pipeline with --var key=value flags
- **When**: smasher run --var greeting=HELLO is executed
- **Then**: Context JSON contains the injected variables
- **Validates**: Variable injection into pipeline execution context

#### 27. pipeline-model-injection
- **Description**: --model auto-injected as context variable
- **Given**: A pipeline run with --model test-id
- **When**: smasher run --model test-id is executed
- **Then**: Context JSON contains model=test-id
- **Validates**: Model auto-injection as pipeline variable

#### 28. pipeline-conditional-branching
- **Description**: Conditional nodes branch on variable value
- **Given**: A pipeline with diamond (conditional) node and labeled edges
- **When**: smasher run --var route=yes selects the 'yes' edge
- **Then**: Pipeline follows the correct branch to exit
- **Validates**: ConditionalHandler evaluates condition attribute against context

#### 29. pipeline-linear-multi-node
- **Description**: Multi-node linear pipelines complete
- **Given**: A pipeline with start → conditional → conditional → exit
- **When**: Variables match edge labels for each conditional
- **Then**: Pipeline traverses all nodes and completes
- **Validates**: Sequential node execution through multiple handlers

#### 30. pipeline-missing-file
- **Description**: Missing DOT file produces I/O error (exit 6)
- **Given**: A nonexistent DOT file path
- **When**: smasher run /bad/path.dot is executed
- **Then**: Exit code 6
- **Validates**: I/O error exit code mapping

#### 31. pipeline-parse-error
- **Description**: Invalid DOT syntax produces parse error (exit 5)
- **Given**: A DOT file with invalid syntax
- **When**: smasher run is executed
- **Then**: Exit code 5
- **Validates**: Parse error exit code mapping

#### 32. pipeline-no-start-node
- **Description**: No start node produces engine error (exit 4)
- **Given**: A DOT file with only exit nodes
- **When**: smasher run is executed
- **Then**: Exit code 4 with 'no start node' message
- **Validates**: Graph validation requires exactly one start node

#### 33. pipeline-stylesheet
- **Description**: Stylesheet transforms apply to pipeline
- **Given**: A CSS-like stylesheet file and a DOT pipeline
- **When**: smasher run --stylesheet style.css is executed
- **Then**: Pipeline completes with stylesheet applied
- **Validates**: Stylesheet parsing and application to graph nodes

#### 34. pipeline-max-steps
- **Description**: --max-steps prevents infinite loops
- **Given**: A pipeline with a self-referencing conditional loop
- **When**: smasher run --max-steps 5 is executed
- **Then**: Execution stops with engine error (exit 4)
- **Validates**: Max steps safety limit prevents runaway execution

#### 35. pipeline-point-shape
- **Description**: shape=point also creates a start node
- **Given**: A DOT file using point shape instead of circle
- **When**: smasher run is executed
- **Then**: Pipeline completes successfully
- **Validates**: Both circle and point shapes map to Start NodeType

---

### ERROR HANDLING & EXIT CODES (7 tests)

#### 36. error-help-flags
- **Description**: --help and subcommand --help work correctly
- **Given**: The smasher binary
- **When**: smasher --help, complete --help, chat --help, run --help are run
- **Then**: Each produces usage info with exit 0
- **Validates**: Clap help generation for all commands

#### 37. error-invalid-subcommand
- **Description**: Invalid subcommand produces error
- **Given**: A nonexistent subcommand name
- **When**: smasher foobar is run
- **Then**: Non-zero exit code with error message
- **Validates**: Clap error handling for unknown subcommands

#### 38. error-bad-api-key
- **Description**: Invalid API key produces LLM error (exit 2)
- **Given**: A bogus API key in environment
- **When**: smasher complete is run
- **Then**: Exit code 2 (LLM error from provider auth rejection)
- **Validates**: API authentication error handling and exit code mapping

#### 39. error-version-flag
- **Description**: --version shows version info
- **Given**: The smasher binary
- **When**: smasher --version is run
- **Then**: Exit 0 with version string
- **Validates**: Clap version flag configuration

#### 40. error-env-file-in-help
- **Description**: --env-file documented in help output
- **Given**: The smasher binary
- **When**: smasher --help is run
- **Then**: Output contains 'env-file'
- **Validates**: New --env-file flag is visible in CLI help

#### 41. error-stderr-clean
- **Description**: Stderr is clean without --verbose
- **Given**: No --verbose flag
- **When**: smasher complete is run normally
- **Then**: stderr contains no debug/trace output
- **Validates**: Default log level is warn, keeping stderr clean

---

### MAKEATRON REFERENCE PARSING (9 tests)

#### 42. makeatron-parse-simple
- **Description**: Parse makeatron simple.dot reference example
- **Given**: The simple.dot example from makeatron/examples with graph defaults and edge chains
- **When**: smasher render simple.dot --format dot is run
- **Then**: DOT output is produced without parse errors
- **Validates**: Parser handles graph [...] default statements and bare attribute assignments

#### 43. makeatron-parse-branching
- **Description**: Parse makeatron branching.dot with conditional gates
- **Given**: branching.dot with diamond conditionals, node defaults, and condition attributes
- **When**: smasher render branching.dot --format dot is run
- **Then**: DOT output is produced without parse errors
- **Validates**: Parser handles diamond conditionals and condition=outcome edge attributes

#### 44. makeatron-parse-human-gate
- **Description**: Parse makeatron human_gate.dot with human review nodes
- **Given**: human_gate.dot with hexagon shape, edge chains, and type=wait.human
- **When**: smasher render human_gate.dot --format dot is run
- **Then**: DOT output is produced without parse errors
- **Validates**: Parser handles edge chains (a -> b -> c) and type attribute on nodes

#### 45. makeatron-parse-full-pipeline
- **Description**: Parse makeatron full_pipeline.dot with all features
- **Given**: full_pipeline.dot with model_stylesheet, goal gates, human review, conditional branching
- **When**: smasher render full_pipeline.dot --format dot is run
- **Then**: DOT output is produced without parse errors
- **Validates**: Parser handles multi-line embedded stylesheets, goal_gate, max_retries, class attributes

#### 46. makeatron-parse-goal-gate
- **Description**: Parse makeatron goal_gate.dot with retry logic
- **Given**: goal_gate.dot with goal_gate=true, max_retries, and retry edges
- **When**: smasher render goal_gate.dot --format dot is run
- **Then**: DOT output is produced without parse errors
- **Validates**: Parser handles goal_gate and max_retries attributes

#### 47. makeatron-parse-build-pong
- **Description**: Parse makeatron build_pong.dot complex build pipeline
- **Given**: build_pong.dot with 10+ nodes, stylesheet, goal gates, human review, compile loop
- **When**: smasher render build_pong.dot --format dot is run
- **Then**: DOT output is produced without parse errors
- **Validates**: Parser handles complex multi-phase build pipelines from reference implementation

#### 48. makeatron-parse-build-htmx-blog
- **Description**: Parse makeatron build_htmx_blog.dot multi-phase web app build
- **Given**: build_htmx_blog.dot with setup, backend, templates, CSS, smoke test phases
- **When**: smasher render build_htmx_blog.dot --format dot is run
- **Then**: DOT output is produced without parse errors
- **Validates**: Parser handles multi-phase pipelines with frontend/backend separation

#### 49. makeatron-parse-build-dvd-bounce
- **Description**: Parse makeatron build_dvd_bounce.dot lightweight pipeline
- **Given**: build_dvd_bounce.dot with design, implement, validate, preview phases
- **When**: smasher render build_dvd_bounce.dot --format dot is run
- **Then**: DOT output is produced without parse errors
- **Validates**: Parser handles lightweight goal-gated pipelines with human preview

#### 50. makeatron-parse-build-python-agent
- **Description**: Parse makeatron build_python_code_agent.dot complex agent build
- **Given**: build_python_code_agent.dot with 8 nodes, multiple goal gates, integration test
- **When**: smasher render build_python_code_agent.dot --format dot is run
- **Then**: DOT output is produced without parse errors
- **Validates**: Parser handles complex pipelines with multiple goal gates and reasoning_effort

#### 51. makeatron-parse-build-markdown-editor
- **Description**: Parse makeatron build_markdown_editor.dot TUI editor build
- **Given**: build_markdown_editor.dot with editor, renderer, layout, compile loop
- **When**: smasher render build_markdown_editor.dot --format dot is run
- **Then**: DOT output is produced without parse errors
- **Validates**: Parser handles multi-phase compile-loop pipelines with polish feedback

#### 52. makeatron-parse-plan-implement-review
- **Description**: Parse makeatron plan_implement_review.dot smoke test pipeline
- **Given**: plan_implement_review.dot with plan->implement->review flow and conditional retry
- **When**: smasher render plan_implement_review.dot --format dot is run
- **Then**: DOT output is produced without parse errors
- **Validates**: Parser handles the canonical plan-implement-review pattern with conditional edges

---

### MAKEATRON PIPELINE EXECUTION (5 tests)

#### 53. makeatron-run-simple
- **Description**: Execute simple.dot pipeline end-to-end with real LLM
- **Given**: simple.dot with start -> codergen -> codergen -> exit and ANTHROPIC_API_KEY set
- **When**: smasher run simple.dot --model claude-haiku-4-5 --max-steps 10
- **Then**: Exit 0, JSON output contains _started and _completed
- **Validates**: Full pipeline execution with real LLM codergen backend, Mdiamond/Msquare shape mapping

#### 54. makeatron-run-plan-implement-review
- **Description**: Execute plan_implement_review.dot with conditional branching
- **Given**: plan_implement_review.dot with goal_gate and conditional retry edges
- **When**: smasher run plan_implement_review.dot --model claude-haiku-4-5 --max-steps 10
- **Then**: Exit 0, pipeline completes within max steps
- **Validates**: Conditional edge evaluation and codergen backend for multi-step pipeline

#### 55. makeatron-run-branching
- **Description**: Execute branching.dot with diamond conditional gate
- **Given**: branching.dot with node defaults, diamond gate, and outcome-based branching
- **When**: smasher run branching.dot --model claude-haiku-4-5 --max-steps 10
- **Then**: Exit 0, pipeline runs codergen nodes and evaluates conditional
- **Validates**: Node default attributes, conditional handler, and codergen execution

#### 56. makeatron-run-human-gate-expected-fail
- **Description**: human_gate.dot fails on Interviewer node as expected
- **Given**: human_gate.dot with hexagon review_gate node (Interviewer type, no handler)
- **When**: smasher run human_gate.dot --model claude-haiku-4-5 --max-steps 10
- **Then**: Exit 4, error message about no handler for Interviewer
- **Validates**: Interviewer nodes require explicit handler registration; graceful error on missing handler

#### 57. makeatron-run-full-pipeline
- **Description**: Execute full_pipeline.dot with all features integrated
- **Given**: full_pipeline.dot with stylesheet, goal gates, human nodes (expected to fail on human), conditionals
- **When**: smasher run full_pipeline.dot --model claude-haiku-4-5 --max-steps 20
- **Then**: Executes with all handlers working except human nodes (exit 4 when hitting human gate)
- **Validates**: Integration of all pipeline features except unimplemented human gates

---

## Summary Statistics

- **Total Scenarios**: 61
- **Env Configuration**: 6 tests (9.8%)
- **LLM Completion**: 10 tests (16.4%)
- **Chat Agent**: 8 tests (13.1%)
- **Pipeline Execution**: 11 tests (18.0%)
- **Error Handling**: 7 tests (11.5%)
- **Makeatron Parsing**: 9 tests (14.8%)
- **Makeatron Execution**: 5 tests (8.2%)

## Coverage Matrix

| Component | Tested | Scenarios |
|-----------|--------|-----------|
| dotenvy   | ✓ | 6 |
| Anthropic API | ✓ | 3 |
| OpenAI API | ✓ | 2 |
| Gemini API | ✓ | 2 |
| Streaming | ✓ | 3 |
| File Operations | ✓ | 7 |
| Shell Execution | ✓ | 1 |
| DOT Parsing | ✓ | 10 |
| Graph Engine | ✓ | 11 |
| Error Codes | ✓ | 7 |
| CLI Flags | ✓ | 25+ |

---

*Last Updated: Test Results Analysis*
*Status: Complete - Ready for Implementation*
