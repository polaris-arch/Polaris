#!/usr/bin/env bash
# Rebuild all Android libbox ABIs from the pinned official source plus local patch.
# Version follows src-tauri/core-manifest.json; toolchain and patch hashes are in
# scripts/libbox-patches/source-manifest.json. Uses cached tools/modules only.
# The optional source repository is read-only; each build uses a fresh checkout
# under ~/Code. Output and build-receipt.json are published after API, ABI, tags,
# runtime dependency and 16K ELF alignment validation.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 "$REPO_ROOT/scripts/libbox-patches/build.py" "$@"
