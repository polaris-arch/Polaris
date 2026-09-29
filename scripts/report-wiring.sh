#!/usr/bin/env bash
# report-wiring.sh —— 移动端接线完成度的**一条命令**入口。
#
# 它回答的问题只有一个：**还剩几条未接线、分别是哪些。**
# 判据本体在 `ui/src/mobile/wiring-completeness.test.ts`（三个取材面 + 处置表，射程自曝写在那份
# 文件的头注里）与 `ui/src/mobile/half-truth-facts.test.ts`（半真文案的正面事实断言）。
# 本脚本不重复实现任何判据 —— 重复一份就是两个真值源，将来必然各自漂移。
#
# 🔴 **两个门一起跑**（2026-09-06 复审 major，已修）。第一版只喂主门那一个文件，
#    而主门自己的头注写着「本门与半真门是一对，缺任何一半都留着一条绕行路」。
#    实测：删掉 src-tauri/src/lib.rs 的 dialog 插件注册（导出腿后端整条没了，正是半真门专门守的
#    那个回归），半真门当场红，而本脚本的输出**逐字不变**、rc 不变。等债务清零那天，
#    它会在半真门红着的状态下退 0 并打印「全部接线达成」—— 两扇门之间的缝正好落在
#    唯一那条对外宣布完成的命令上。
#
# 🔴 退出码逻辑住在 `scripts/wiring-verdict.mjs`，**因为它必须有测试**
#    （`scripts/wiring-verdict.test.mjs`；本仓 gate-node-test.sh 存在的全部理由就是这一条）。
#    第一版把它写在这里、没有测试，于是带着两个 bug 交付：销账那天被报成「门坏了」，
#    以及清零那天的提示是死循环。退出码语义见那份文件的头注。
#
# 🔴 输出与退出码**分两步取**（沿用 gate-node-test.sh 那条已修过的教训）：写成
#    `out="$(npx vitest …)"` 时，vitest 非零会让 `set -e` 在赋值那一行就把脚本掐掉，
#    下面一行报告都打不出来 —— 门红的时候看不见红在哪。故此处不开 errexit，
#    用 `|| rc=$?` 让赋值先落地。
#
# 🔴 `--silent=false` 不能省：vitest 4 默认只在**失败**的用例上放行 console 输出，
#    而这份报告是在一条**通过**的用例里打印的 —— 少这个开关，报告一个字都不会出现，
#    而脚本会因为找不到 marker 退 2（不会假装成功，但也拿不到东西）。
set -uo pipefail
cd "$(dirname "$0")/.."

GATES=(
  src/mobile/wiring-completeness.test.ts
  src/mobile/half-truth-facts.test.ts
)
VERDICT=scripts/wiring-verdict.mjs

for gate in "${GATES[@]}"; do
  [ -f "ui/$gate" ] || {
    echo "::error::report-wiring: 判据文件缺失：ui/$gate" >&2
    exit 2
  }
done
[ -f "$VERDICT" ] || {
  echo "::error::report-wiring: 退出码判据缺失：$VERDICT" >&2
  exit 2
}

rc=0
out="$(cd ui && POLARIS_WIRING_REPORT=1 npx vitest run --silent=false --reporter=verbose "${GATES[@]}" 2>&1)" || rc=$?

printf '%s\n' "$out" | node "$VERDICT" --rc="$rc"
