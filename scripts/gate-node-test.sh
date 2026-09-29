#!/usr/bin/env bash
# gate-node-test.sh —— 跑 scripts/*.test.mjs 合同用例并自证「确有其事」。
#
# 起因（2026-08-31 全量门规格三修 F3）：`node --test scripts/` 指向空目录、或目标文件
# 被改名/移走、或在错误 cwd 执行时，Node 的 test runner 都是 rc=0（0 tests / 0 pass / 0
# fail），与「全部用例真的跑过且通过」在 shell 层完全不可区分——门会静默判绿。
# 实测：`node --test /空目录/` → rc=0，tests 0 / pass 0 / fail 0。
#
# 分类器与 Cronet 合同是 release-risk 的独立承重面，故先固定要求这些文件存在：文件枚举不能守住
# 「某一承重文件被删/改名」的情况。只枚举 *.test.mjs，未来新增同名合同仍自动纳入；目录自动发现
# 还会把 test-android-*.mjs 当作测试执行，但那些是真机脚本，必须由显式设备序列号的入口单独运行。
#
# AppImage 后处理合同同列必需（2026-09-04）：`appImageRuntimeViolations` 既是**修复方**
# （postprocess-appimage.mjs 重封前自检）又是**判分方**（verify-packaging.mjs payload 门 import
# 同一个函数），两侧共用一份判据 ⇒ 谓词退化成恒绿时没有任何别的门会红。那组用例的承重面是把
# 2026-08-24 真机上那个坏 AppDir 的形态原样回放、断言必须报满 5 项，即这条链路唯一的反向对照。
#
# CI（.github/workflows/release-risk.yml）与本机全量门单（vault SoT §12.3c）都调用本脚本，
# 而不是各自重复一份 `node --test scripts/`，以免两处判据将来各自漂移。
set -euo pipefail
cd "$(dirname "$0")/.."

required_tests=(
  scripts/classify-ci-impact.test.mjs
  scripts/fetch-cronet.test.mjs
  scripts/postprocess-appimage.test.mjs
  # Android APK 腿的产物级判据本体（scripts/verify-apk.mjs）的变异测试：它被删/改名时
  # `node --test scripts/*.test.mjs` 只会少 pass 几条，下面的下限未必抓得到，故显式列出。
  scripts/verify-apk.test.mjs
  # release 冒烟腿的产物级判据本体（scripts/assert-r8-evidence.mjs）的变异测试：它守的是
  # 「针被本仓自己的注释喂绿」与「configuration.txt 证明不了 keep 命中没命中」这两条真缺陷。
  scripts/assert-r8-evidence.test.mjs
  # 「移动端全部接线了没」这句话的退出码逻辑（scripts/wiring-verdict.mjs）的变异测试：
  # 它守的是「销账被报成门坏了」与「清零那天的提示是死循环、rc=0 到不了」这两条真缺陷。
  scripts/wiring-verdict.test.mjs
)
for file in "${required_tests[@]}"; do
  [ -f "$file" ] || {
    echo "::error::gate-node-test: 必需测试文件缺失：$file" >&2
    exit 1
  }
done

# 🔴 输出与退出码必须**分两步**取（2026-09-05 修）。旧版写成 `out="$(node --test …)"`：
#    node 非零时 `set -e` 在**赋值那一行**就把脚本掐掉，下面的 printf 一行都执行不到 ⇒
#    门 rc=1 而日志是**空文件**（实测 `wc -l` = 0）。门红的时候看不见红在哪，读的人只会
#    以为是脚本坏了或 cwd 不对，去查一个不存在的问题。
#    `|| rc=$?` 让这次赋值成为 `||` 列表的左支，errexit 按定义不介入，于是输出先落地、再据 rc 退出。
rc=0
out="$(node --test --test-reporter=tap scripts/*.test.mjs 2>&1)" || rc=$?
printf '%s\n' "$out"
if [ "$rc" -ne 0 ]; then
  echo "::error::gate-node-test: node --test 退出码 $rc —— 失败用例见上面 TAP 输出里的 'not ok' 行" >&2
  exit "$rc"
fi

# 同一形状的姊妹腿（同根因，同批一起修）：TAP 格式真的变了时 grep 匹不到，pipefail 让整条管道
# rc=1 ⇒ errexit 又会在**这一行**掐掉脚本，而下面那个 `-z` 分支正是**专为这种情况**写的报错，
# 它在旧版里永远到不了。`|| true` 把管道 rc 吞掉，让判断落回 `-z` 那一支去打它该打的话。
pass="$(printf '%s\n' "$out" | grep -E '^# pass [0-9]+$' | awk '{print $3}')" || true
if [ -z "${pass:-}" ]; then
  echo "::error::gate-node-test: 未能从输出里解析出 '# pass N' 行，TAP 格式可能变了" >&2
  exit 1
fi
# 下限随合同条数走。**加了用例就要抬这个数**，否则新用例被整批删掉时本门照绿——那正是这条
# 下限存在的理由。变更史：
#   68 → 76（2026-09-05）verify-apk 死字节判据的 8 条（正反变异 + 带的两个沿 + 两条取材面自检
#           + 算式手算对拍 + 真实尺度红绿对照）
#   76 → 86（2026-09-05）verify-apk 原生库剥符号判据的 10 条（DWARF 回来 / libbox 出处路由 /
#           `.zdebug_*` 旧拼写 / 三条取材面自检 / 正面断言有牙 / ELF32 / 节表取值口）
#   86 → 96（2026-09-05）verify-apk 判据 ABI 参数化的 8 条（parseArgs 三条：缺省不变 / 取得到 /
#           未知取值必抛；四 ABI 全绿的反向对照；换 ABI 判必须红；机器类型与位宽各一条；
#           非缺省 ABI 下 ③⑥ 仍有牙）+ gate-rust-ci-parity 的 2 条（dep-info 对差门的实质对拍
#           与它的切片自检）
#   86 → 98（2026-09-05）assert-r8-evidence 的 12 条（B07 注释污染正反回放 / keep 指向不存在的
#           类时 seeds 说话 / wry 成员没保住 / 反向对照有牙 / 空产物 / 判定面塌 / seeds 行数下限 /
#           五根针逐条删除对照 / 注释行提取）
#   98 → 103（2026-09-05）assert-r8-evidence 按**来源段**归因的 5 条（A11 正反回放：本批那个
#           keep 块被 Tauri 生成的 proguard-wry.pro 喂绿 / 外部来源的针被抄进本仓段不算数 /
#           没有段落标记时不许退化成全文匹配 / 段落切分与来源判定自检）
#   两条线在同一处各自抬过下限（96 与 103），合并后两批测试都在 ⇒ 下限按合并态实测重定。
#   113 → 124（2026-09-06）wiring-verdict 的 11 条（报告块切分 / 失败条数解析 / 三个 marker 各自
#           缺席 / 半真门没跑 / 销账不许报成门坏了 / 漂移分流不许吞掉真失败 / 无漂移 token 的红 /
#           清零→升格→rc=0 完整路径 / it.skip 那条路 / 正常态的反向对照）
#   124 → 132（2026-09-06）verify-apk 出厂权限集判据（⑦）的 8 条（真 aapt2 输出的反向对照 /
#           行首锚定不许把 `permission:` 算成请求 / 注入 QUERY_ALL_PACKAGES 必红 / 误删一条必红 /
#           自建 permission 多一条必红 / 取材面塌陷红在取材面 / 下限有牙 / 登记表 8+3 与理由自检）
if [ "$pass" -lt 132 ]; then
  echo "::error::gate-node-test: 只 pass 了 $pass 条（当前固定合同共 132 条；下限 132）—— 必需合同测试是否被误删/改名/漏跑？" >&2
  exit 1
fi
