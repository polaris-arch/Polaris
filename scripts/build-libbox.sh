#!/usr/bin/env bash
# Retired alpha8 producer. Historical receipt helpers remain read-only consumers.
set -euo pipefail
printf '%s\n' 'Legacy Android libbox build is retired. Use bash scripts/build-mobile-core.sh android with the required source/tool/work/cache arguments; see scripts/mobile-core/README.md.' >&2
exit 1
