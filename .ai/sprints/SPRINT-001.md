# SPRINT-001: Add Pipeline Execution Statistics

## Goal
Add a summary statistics report printed at the end of every `smasher run` pipeline execution showing node counts, timing, token usage, and outcome.

## Requirements

1. After a pipeline run completes, print a summary table to stderr showing:
   - Total nodes visited
   - Nodes by outcome (success / fail / retry)
   - Total wall-clock duration
   - Per-node timing (top 3 slowest)
   - Total LLM tokens used (if available from context)

2. The summary should be formatted as a clean ASCII table.

3. The summary should be gated behind a `--stats` CLI flag (default off) or always shown at verbosity >= info.

## Definition of Done

- [ ] `PipelineStats` struct defined in `smasher-attractor` that collects node timings and outcomes
- [ ] Stats populated during pipeline execution from checkpoint data
- [ ] ASCII table formatter for the stats
- [ ] CLI flag `--stats` added to `smasher run`
- [ ] Integration test verifying stats output format
- [ ] Stats printed to stderr after run completion
