#!/usr/bin/env bash
# Build the frozen patched core for iPhone/iPad and simulators, with a receipt.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$REPO_ROOT/scripts/ios-libbox.py" build "$@"
