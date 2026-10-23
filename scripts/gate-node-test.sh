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
  # Release native stripping must retain runtime metadata and exact AAR source bytes.
  scripts/strip-android-release-native.test.mjs
  # release 冒烟腿的产物级判据本体（scripts/assert-r8-evidence.mjs）的变异测试：它守的是
  # 「针被本仓自己的注释喂绿」与「configuration.txt 证明不了 keep 命中没命中」这两条真缺陷。
  scripts/assert-r8-evidence.test.mjs
  # 「移动端全部接线了没」这句话的退出码逻辑（scripts/wiring-verdict.mjs）的变异测试：
  # 它守的是「销账被报成门坏了」与「清零那天的提示是死循环、rc=0 到不了」这两条真缺陷。
  scripts/wiring-verdict.test.mjs
  # PC 发布冻门的策略测试不能被删/改名后静默少跑。
  scripts/assert-pc-runtime-release.test.mjs
  # Desktop source graph predicates may not disappear from the host fixture gate.
  scripts/build-desktop-core.test.mjs
  scripts/desktop-core-ci-wiring.test.mjs
  # 内核从安装目录直接运行后的两条打包侧判据：被删/改名时安装器照样编得过、dmg 照样出得来。
  scripts/nsis-core-sweep.test.mjs
  scripts/macos-nested-code.test.mjs
  scripts/macos-open-guide.test.mjs
  scripts/helper-installer-target-scope.test.mjs
  # 卸载钩子里唯一提权执行的那一段：脚本随命令行进入提权进程、删除前逐项校验、退出码被消费。
  scripts/nsis-helper-cleanup.test.mjs
  scripts/release-assets.test.mjs
  scripts/mobile-source-unit.test.mjs
  scripts/apple-ci-impact.test.mjs
  scripts/ci-workflow-efficiency.test.mjs
  scripts/package-acceptance.test.mjs
)
for file in "${required_tests[@]}"; do
  [ -f "$file" ] || {
    echo "::error::gate-node-test: 必需测试文件缺失：$file" >&2
    exit 1
  }
done

# Stream TAP directly without retaining a second complete log in the shell.
# pipefail keeps the runner exit status authoritative; the terminal count adds
# coverage protection and never substitutes for successful test execution.
node --test --test-reporter=tap scripts/*.test.mjs 2>&1 | (
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
  if [ "$passed" -lt 501 ]; then
    echo "::error::gate-node-test: only $passed passes; mandatory minimum is 501" >&2
    exit 1
  fi
)
