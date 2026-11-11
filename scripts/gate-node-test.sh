#!/usr/bin/env bash
# gate-node-test.sh —— 跑 scripts/*.test.mjs 合同用例并自证「确有其事」。
#
# 起因（2026-08-31 全量门规格三修 F3）：`node --test scripts/` 指向空目录、或目标文件
# 被改名/移走、或在错误 cwd 执行时，Node 的 test runner 都是 rc=0（0 tests / 0 pass / 0
# fail），与「全部用例真的跑过且通过」在 shell 层完全不可区分——门会静默判绿。
# 实测：`node --test /空目录/` → rc=0，tests 0 / pass 0 / fail 0。
#
# 只枚举 scripts/ 一级与二级目录下的 *.test.mjs，未来新增同名合同自动纳入；目录自动发现
# 还会把 test-android-*.mjs 当作测试执行，但那些是真机脚本，必须由显式设备序列号的入口单独运行。
#
# CI（.github/workflows/release-risk.yml）与本机全量门单（vault SoT §12.3c）都调用本脚本，
# 而不是各自重复一份 `node --test scripts/`，以免两处判据将来各自漂移。
set -euo pipefail
cd "$(dirname "$0")/.."

# Stream TAP directly without retaining a second complete log in the shell.
# pipefail keeps the runner exit status authoritative; the terminal count only
# rejects an empty run and never substitutes for successful test execution.
# Diagnostics retain stderr; only the runner's stdout is the TAP protocol.
node --test --test-reporter=tap scripts/*.test.mjs scripts/*/*.test.mjs | (
  summaries=0
  passed=0
  while IFS= read -r line || [ -n "$line" ]; do
    printf '%s\n' "$line"
    if [[ "$line" =~ ^#\ pass\ ([0-9]+)$ ]]; then
      summaries=$((summaries + 1))
      passed="${BASH_REMATCH[1]}"
    fi
  done
  if [ "$summaries" -ne 1 ]; then
    echo '::error::gate-node-test: expected exactly one TAP pass summary' >&2
    exit 1
  fi
  if [ "$passed" -lt 1 ]; then
    echo '::error::gate-node-test: no test passed' >&2
    exit 1
  fi
)
