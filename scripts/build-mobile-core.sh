#!/usr/bin/env bash
# Explicit formal mobile component; all source/tool/work/cache paths are required.
set -euo pipefail
mobile_repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
exec node "$mobile_repo_root/scripts/mobile-core/producer.mjs" "$@"
