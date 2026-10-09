#!/usr/bin/env bash
# check-vendored-path-sources.sh —— 随仓第三方副本（`[patch]` 指向的路径源）在交叉目标上不得出任何诊断。
#
# 用法: scripts/check-vendored-path-sources.sh <target-triple>...
#   目标清单由调用方给（ci.yml 的 cross job 与 scripts/gate-rust.sh 传同一份，两处由
#   gate-rust-ci-parity.test.mjs 对拍），本脚本不自带清单，免得多出第三份要跟着改的三元组。
#   调用方须已 `rustup target add` 过这些目标。
#
# 守的是什么：
#   Cargo 只给注册表与 git 来源的依赖加 lint 上限；路径源一律按本仓代码对待，原生腿的
#   `RUSTFLAGS="-D warnings"` 会拿最新工具链新增的 lint 去审它。而这类依赖往往只在某一个平台的
#   依赖图里（swift-rs：macOS 宿主的构建依赖，加 iOS 目标的普通依赖），Linux 宿主的图里没有它，
#   上面那条按 workspace 成员派生的交叉 clippy 循环也不含它 ⇒ 要到 macOS 原生腿才红。
#
# 判据是「零诊断」而不是 `-D warnings` 的退出码：
#   · 带 `--target` 时 RUSTFLAGS 不作用于宿主侧产物，副本的**构建脚本**就算出了警告，cargo 照样
#     rc=0（实测）；而原生腿没有 `--target`，同一条警告在那里是错误。
#   · clippy 的 `-- -D warnings` 只作用于 workspace 成员，对路径依赖不生效（实测）。
#   故改读 cargo 的 JSON 诊断流，按包 id 数这份副本自己的 warning / error：lib 与构建脚本同一把尺。
#
# 射程与已知上限（别读成「副本已在所有平台验过」）：
#   · 对象从 `cargo metadata` 派生：来源为空（路径源）且不是 workspace 成员的包。新增一份副本
#     默认被覆盖。
#   · 每份副本至少要在一个给定目标的依赖图里；一个都不在 ⇒ 当场红（本机够不着它，得另想办法，
#     不能静默算过）。
#   · 特性取该目标下依赖图实际启用的那一组。swift-rs 的 `build` 特性只由 macOS 宿主的构建依赖
#     启用，这里编不到 `src-rs/build.rs`；那一段仍只有 macOS 原生腿看得见。
#   · 副本自带的上限（crate 级 `#![allow(warnings)]`）由
#     `src-tauri/tests/cross_target_coverage.rs` 的源码级门守；本脚本验的是上限在真工具链上确实生效，
#     以及副本在该目标上编得过。
set -euo pipefail
cd "$(dirname "$0")/.."

[ "$#" -gt 0 ] || { echo "用法: $0 <target-triple>..." >&2; exit 2; }

out="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/var/tmp}}/polaris-vendored.XXXXXX")"
trap 'rm -rf "$out"' EXIT

cargo metadata --format-version 1 > "$out/metadata.json"
mapfile -t VENDORED < <(jq -r '. as $m | .packages[]
  | select(.source == null)
  | select(.id as $id | $m.workspace_members | index($id) | not)
  | "\(.name)\t\(.id)"' "$out/metadata.json" | sort)
echo "随仓路径源 ${#VENDORED[@]} 个"

rc=0
for entry in "${VENDORED[@]}"; do
  name="${entry%%$'\t'*}"
  id="${entry#*$'\t'}"
  reached=0
  for t in "$@"; do
    # 不在该目标的依赖图里就没有可编的单元：`cargo tree -i` 此时 stdout 为空。
    [ -n "$(cargo tree --workspace --target "$t" -i "$name" -e normal,build --depth 0 2>/dev/null)" ] || continue
    reached=1
    extra=""
    [ "$t" = aarch64-linux-android ] && [ -n "${ANDROID_NDK_BIN:-}" ] && extra="$ANDROID_NDK_BIN:"
    # `json` 而不是 `json-render-diagnostics`：后者把诊断渲染到 stderr、不再放进 JSON 流，
    # 下面就没有东西可数（实测：两条警告在屏幕上，数出来却是 0）。
    # 不接管道、`|| built=$?`：cargo 自己的退出码要留着，但编不过时也得先把诊断原文打出来再红。
    built=0
    PATH="${extra}${PATH}" cargo clippy --target "$t" -p "$name" --message-format=json > "$out/$name-$t.json" || built=$?
    jq -r --arg id "$id" 'select(.reason == "compiler-message" and .package_id == $id)
      | select(.message.level == "warning" or .message.level == "error")
      | .message.rendered' "$out/$name-$t.json" > "$out/$name-$t.txt"
    n="$(jq -s --arg id "$id" '[.[]
      | select(.reason == "compiler-message" and .package_id == $id)
      | select(.message.level == "warning" or .message.level == "error")] | length' "$out/$name-$t.json")"
    echo "  · $name @ $t：$n 条诊断，cargo rc=$built"
    if [ "$n" != 0 ] || [ "$built" != 0 ]; then
      cat "$out/$name-$t.txt"
      echo "::error::随仓路径源 $name 在 $t 上有 $n 条诊断（cargo rc=$built）—— 原生腿的 -D warnings 会把警告当错误" >&2
      rc=1
    fi
  done
  if [ "$reached" = 0 ]; then
    echo "::error::随仓路径源 $name 不在任何给定目标（$*）的依赖图里 —— 本门够不着它，不能算过" >&2
    rc=1
  fi
done
exit "$rc"
