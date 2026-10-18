#!/usr/bin/env bash
# Observe the original Tauri invocation once. Never rerun a failed bundler.
# The workflow enables CLI Debug and supplies an attempt-local log path.
set -uo pipefail

: "${POLARIS_TAURI_LOG:?POLARIS_TAURI_LOG must identify the diagnostic log}"
# Fail before building if the evidence destination cannot be opened.
: > "$POLARIS_TAURI_LOG" || exit "$?"

tauri "$@" 2>&1 | tee "$POLARIS_TAURI_LOG"
statuses=("${PIPESTATUS[@]}")
# Keep the original Tauri failure, including when tee also fails. Otherwise a
# logging failure must remain visible instead of claiming evidence was saved.
if [ "${statuses[0]}" -ne 0 ]; then
  exit "${statuses[0]}"
fi
exit "${statuses[1]}"
