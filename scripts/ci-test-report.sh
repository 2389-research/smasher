#!/usr/bin/env bash
# ABOUTME: Runs cargo tests per crate and captures structured output for CI artifact upload.
# ABOUTME: Formats results as GitHub Actions job summary markdown with per-crate and total counts.

set -euo pipefail

CRATES=("smasher-llm" "smasher-agent" "smasher-attractor" "smasher-cli")
ALL_PASSED=true
LOG_DIR="${1:-test-logs}"

mkdir -p "$LOG_DIR"

# Accumulate grand totals
grand_passed=0
grand_failed=0
grand_ignored=0

# Start building the summary markdown
summary="## Test Results\n\n"
summary+="| Crate | Passed | Failed | Ignored | Status |\n"
summary+="|-------|-------:|-------:|--------:|--------|\n"

for crate in "${CRATES[@]}"; do
  log_file="$LOG_DIR/${crate}.log"

  # Run tests, capture output, allow failure
  output=$(cargo test -p "$crate" 2>&1) || true
  echo "$output" > "$log_file"

  # Aggregate counts across all "test result:" lines (unit + doc + integration)
  total_passed=0
  total_failed=0
  total_ignored=0
  found_results=false

  while IFS= read -r result_line; do
    found_results=true
    p=$(echo "$result_line" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+') || true
    f=$(echo "$result_line" | grep -oE '[0-9]+ failed' | grep -oE '[0-9]+') || true
    i=$(echo "$result_line" | grep -oE '[0-9]+ ignored' | grep -oE '[0-9]+') || true
    total_passed=$((total_passed + ${p:-0}))
    total_failed=$((total_failed + ${f:-0}))
    total_ignored=$((total_ignored + ${i:-0}))
  done < <(echo "$output" | grep "^test result:" || true)

  if [ "$found_results" = false ]; then
    status="-- (no tests)"
  elif [ "$total_failed" -gt 0 ]; then
    status="FAIL"
    ALL_PASSED=false
  else
    status="PASS"
  fi

  grand_passed=$((grand_passed + total_passed))
  grand_failed=$((grand_failed + total_failed))
  grand_ignored=$((grand_ignored + total_ignored))

  summary+="| \`$crate\` | $total_passed | $total_failed | $total_ignored | $status |\n"
done

# Add totals row
if [ "$ALL_PASSED" = true ]; then
  total_status="ALL PASS"
else
  total_status="HAS FAILURES"
fi
summary+="| **Total** | **$grand_passed** | **$grand_failed** | **$grand_ignored** | **$total_status** |\n"

# Write the summary to stdout
printf "$summary\n"

# If GITHUB_STEP_SUMMARY is set, write to it for GitHub Actions job summary
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  printf "$summary\n" >> "$GITHUB_STEP_SUMMARY"
fi

# Also write a standalone summary file for artifact upload
printf "$summary\n" > "$LOG_DIR/summary.md"

echo ""
echo "Test logs written to $LOG_DIR/"
echo "Grand total: $grand_passed passed, $grand_failed failed, $grand_ignored ignored"

if [ "$ALL_PASSED" = true ]; then
  exit 0
else
  exit 1
fi
