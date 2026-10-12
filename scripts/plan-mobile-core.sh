#!/usr/bin/env bash
# Source/target plan only. This entry never binds, installs tools or publishes.
set -euo pipefail
task_repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$task_repo_root/scripts/mobile-core/plan.py" "$@"
