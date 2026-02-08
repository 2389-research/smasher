#!/usr/bin/env bash
# ABOUTME: CI test summary script that runs workspace tests and formats results as a table.
# ABOUTME: Shows per-crate test counts and pass/fail status; exits 0 if all pass, 1 otherwise.

set -euo pipefail

CRATES=("smasher-llm" "smasher-agent" "smasher-attractor" "smasher-cli")
ALL_PASSED=true

printf "\n"
printf "%-25s %10s %10s %10s %s\n" "Crate" "Passed" "Failed" "Ignored" "Status"
printf "%-25s %10s %10s %10s %s\n" "-------------------------" "----------" "----------" "----------" "------"

for crate in "${CRATES[@]}"; do
  output=$(cargo test -p "$crate" 2>&1) || true

  # Aggregate counts across all "test result:" lines (unit tests + doc tests)
  total_passed=0
  total_failed=0
  total_ignored=0
  found_results=false

  while IFS= read -r result_line; do
    found_results=true
    p=$(echo "$result_line" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+')
    f=$(echo "$result_line" | grep -oE '[0-9]+ failed' | grep -oE '[0-9]+')
    i=$(echo "$result_line" | grep -oE '[0-9]+ ignored' | grep -oE '[0-9]+')
    total_passed=$((total_passed + ${p:-0}))
    total_failed=$((total_failed + ${f:-0}))
    total_ignored=$((total_ignored + ${i:-0}))
  done < <(echo "$output" | grep "^test result:")

  if [ "$found_results" = false ]; then
    status="NO TESTS"
    total_passed=0
    total_failed=0
    total_ignored=0
  elif [ "$total_failed" -gt 0 ]; then
    status="FAIL"
    ALL_PASSED=false
  else
    status="PASS"
  fi

  printf "%-25s %10s %10s %10s %s\n" "$crate" "$total_passed" "$total_failed" "$total_ignored" "$status"
done

printf "\n"

if [ "$ALL_PASSED" = true ]; then
  echo "All crates passed."
  exit 0
else
  echo "Some crates had failures."
  exit 1
fi
