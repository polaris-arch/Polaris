#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
rust_ndk_version="$(cat "$repo_root/scripts/android-rust-ndk.version")"
default_ndk="$sdk/ndk/$rust_ndk_version"

# Tauri selects the compiler from NDK_HOME; btls-sys selects its CMake
# toolchain from ANDROID_NDK_HOME. A split pair mixes incompatible NDKs.
selected="${ANDROID_NDK_HOME:-${NDK_HOME:-$default_ndk}}"
if [[ ! -f "$selected/source.properties" || ! -f "$selected/build/cmake/android.toolchain.cmake" ]]; then
  echo "Rust NDK is missing or incomplete: $selected" >&2
  echo "Install ndk;$rust_ndk_version with sdkmanager, or point both NDK variables to an installed copy of that version" >&2
  exit 1
fi
selected="$(cd "$selected" && pwd -P)"
for name in ANDROID_NDK_HOME NDK_HOME ANDROID_NDK_ROOT; do
  value="${!name:-}"
  if [[ -n "$value" ]]; then
    if [[ ! -d "$value" || "$(cd "$value" && pwd -P)" != "$selected" ]]; then
      echo "$name conflicts with the selected Rust NDK; use one NDK for all Android build tools" >&2
      exit 1
    fi
  fi
done
revision="$(sed -n 's/^Pkg.Revision[[:space:]]*=[[:space:]]*//p' "$selected/source.properties" | head -1)"
if [[ "$revision" != "$rust_ndk_version" ]]; then
  echo "Rust NDK Pkg.Revision must be $rust_ndk_version (found: ${revision:-missing})" >&2
  exit 1
fi
export ANDROID_NDK_HOME="$selected" NDK_HOME="$selected" ANDROID_NDK_ROOT="$selected"

cli="$repo_root/ui/node_modules/@tauri-apps/cli/tauri.js"
if [[ ! -f "$cli" ]]; then
  echo "Tauri CLI is missing at $cli; install the ui/ dependencies first" >&2
  exit 1
fi
cd "$repo_root"
exec node "$cli" android build "$@"
