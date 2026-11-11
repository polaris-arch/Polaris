#!/usr/bin/env bash
# Observe the original Tauri invocation once. Never rerun a failed bundler.
# Explicit CLI verbosity exposes linuxdeploy stdout/stderr; logger env alone
# does not enable the bundler child output. Keep all product arguments intact.
set -uo pipefail

: "${POLARIS_TAURI_LOG:?POLARIS_TAURI_LOG must identify the diagnostic log}"
# Fail before building if the evidence destination cannot be opened.
: > "$POLARIS_TAURI_LOG" || exit "$?"

tauri --verbose "$@" 2>&1 | tee "$POLARIS_TAURI_LOG"
statuses=("${PIPESTATUS[@]}")
# Keep the original Tauri failure, including when tee also fails. Otherwise a
# logging failure must remain visible instead of claiming evidence was saved.
if [ "${statuses[0]}" -ne 0 ]; then
  exit "${statuses[0]}"
fi
exit "${statuses[1]}"
