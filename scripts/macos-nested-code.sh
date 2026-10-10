#!/usr/bin/env bash
# macos-nested-code.sh —— `.app` 里 `Contents/Resources` 之下的 Mach-O 逐个下判断。
#
# 用法：
#   bash scripts/macos-nested-code.sh seal   <Polaris.app> <源内核文件>   # 封印整个 bundle **之前**跑
#   bash scripts/macos-nested-code.sh verify <Polaris.app> <源内核文件>   # 对最终产物跑；只读
#   <源内核文件> = 本次打包取用的那份 `resources/mac-<arch>/sing-box`（必填，没有缺省值）。
#
# 为什么需要它：
#   内核 `sing-box` 与 `polaris-helper` 住在 `Contents/Resources/_up_/resources/mac-<arch>/`，应用直接
#   执行包内这一份内核。Apple 的资源规则把 `Resources/` 下的文件按**资源**封进 bundle 的哈希表，不当
#   嵌套代码逐个签；照此 bundle 那一次 `codesign --deep` 封印与 `--verify --deep` 都不替这两个文件
#   各自的签名作证（这一条取自规则文本，本仓没有在 runner 上单独实测）。所以这里逐个判断，不借 bundle
#   的封印替它们作证。
#
# ── 三类文件，三种处置（按文件名分流；分流点只有下面 `case "$base"` 一处）──
#
#   1. 内核 `sing-box` —— **只校验，任何模式下都不签、不改写**。
#      内核字节要与源码构建回执里的哈希对得上，打包链上任何一次改写（含补签）都会让它对不上。
#      判据两条，都必须成立：
#        a. 与 <源内核文件> 逐字节相同（`cmp`）。这是「打包过程没有改写内核」的正面断言 —— 不是靠
#           「本脚本没调签名命令」反推：bundle 封印、dmg 转换等别的步骤同样在射程内。
#        b. 自身签名有效（`codesign --verify --strict`）。唯一例外：`file` 报告为**纯 x86_64** 且
#           codesign 明确报告「根本没有签名」（不是「签名无效」）时放行，并打一条 notice 如实记录。
#           依据：Intel 上执行 Mach-O 不要求代码签名；arm64 上没有有效签名的 Mach-O 一执行就被系统
#           终止，故 arm64（含 universal）没有这条例外。「有签名但验不过」在任何架构上都判失败。
#      内核验不过 ⇒ 打包失败，不在这里修：签名属于内核构建产物那一步（前置说明见
#      `scripts/core-patches/README.md` 的「macOS code signature of the bundled core」一节）。
#
#   2. `polaris-helper` —— 本仓 Rust 产物，不在「不得改写」之内。
#      seal 模式：验不过就 ad-hoc 补签并点名，补签后再验一次；verify 模式：只验，验不过即失败。
#      所以最终产物里 helper 的签名由「seal 补签 + verify 复验」这一对保证。
#      点名的级别：纯 x86_64 且「根本没有签名」打 notice，其余（arm64 / universal 没有签名、任何架构上
#      签名无效）打 warning。前者是每次打包都会出现的预期形态 —— 链接器只给 arm64 产物自动加 ad-hoc
#      签名，x86_64 产物出链接器时就是没有签名的（内核那一侧的同一事实见上面 1b）；每次都响的 warning
#      只会让真正异常的那一次没人看。后者说明构建链上有东西不对，照旧打 warning。
#
#   3. 其它 Mach-O（包内容白名单目前不允许出现）—— 只校验，验不过即失败，不替来历不明的文件签名。
#
# 正面断言：扫完必须确实见过 `sing-box` 与 `polaris-helper` 两个 Mach-O。目录不对、文件不是
#   Mach-O、`find` 一个都没给出来，都会让「逐个验」变成「一个都没验」而照样 rc=0。
#
# 符号链接：`find -type f` 不跟随链接，链接本身不算文件 ⇒ 指向包外的链接不会被当成包内文件去验；
#   `sing-box` 若是一条链接，结果是「没有扫到」而失败。
#
# 这里验不了的（只能在 macOS runner / 真机上成立）：真 `codesign` 对 linker-signed ad-hoc 签名的
#   判定、`--deep` 是否触碰 `Resources/` 下的 Mach-O（由判据 1a 在 runner 上间接回答）、带
#   `com.apple.quarantine` 时系统对包内未签名 x86_64 内核的放行与否。
set -euo pipefail

fail() {
  echo "::error::macos-nested-code: $1" >&2
  exit "${2:-1}"
}

mode="${1:-}"
app="${2:-}"
core_ref="${3:-}"
case "$mode" in
  seal | verify) ;;
  *) fail "用法 macos-nested-code.sh <seal|verify> <App.app> <源内核文件>（收到模式 '${mode}'）" 2 ;;
esac
# 以 `-` 开头的相对路径会被后面的命令当成选项。
case "$app" in -*) app="./$app" ;; esac
case "$core_ref" in -*) core_ref="./$core_ref" ;; esac
res="$app/Contents/Resources"
if [ -z "$app" ] || [ ! -d "$res" ]; then
  fail "不是 app bundle（缺 Contents/Resources）：${app}" 2
fi
if [ -z "$core_ref" ] || [ ! -f "$core_ref" ] || [ -L "$core_ref" ]; then
  fail "第三个参数必须是源内核文件（普通文件，非链接）：'${core_ref}'" 2
fi
for tool in find file codesign cmp mktemp node; do
  command -v "$tool" >/dev/null 2>&1 || fail "找不到命令 ${tool}（本脚本只在带 Xcode 命令行工具的 macOS 上有意义）" 2
done

# 文件清单先落盘再读：`find` 中途失败（某个子目录读不了）时这里就失败，而不是带着半份清单继续。
list="$(mktemp "${TMPDIR:-/tmp}/polaris-nested-code.XXXXXX")"
trap 'rm -f "$list"' EXIT
find "$res" -type f -print0 >"$list" || fail "枚举 ${res} 失败"

# 结果经全局变量回传（bash 3.2 没有 nameref）：state = signed | unsigned | invalid。
state=""
detail=""
sig_state() {
  if detail="$(codesign --verify --strict "$1" 2>&1)"; then
    state="signed"
  else
    case "$detail" in
      *"code object is not signed at all"*) state="unsigned" ;;
      *) state="invalid" ;;
    esac
  fi
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

BUILD_SIDE="应在内核构建产物那一步签名，而不是在打包时补（见 scripts/core-patches/README.md「macOS code signature of the bundled core」）"

# 内核：只读。本函数及它调用的 sig_state 里没有任何签名命令。
check_core() {
  local f="$1" kind="$2" rel="$3"
  if ! cmp -s "$f" "$core_ref"; then
    fail "包内内核与源内核字节不同：${rel} ≠ ${core_ref} —— 打包链上有步骤改写了内核"
  fi
  sig_state "$f"
  case "$state" in
    signed) ;;
    unsigned)
      case "$kind" in
        *arm64* | *universal*) fail "内核没有签名：${rel}（${kind}）—— arm64 上无有效签名即无法执行。${BUILD_SIDE}" ;;
        *x86_64*)
          echo "::notice::macos-nested-code: ${rel} 是没有签名的 x86_64 内核，按原字节放行（Intel 上执行不要求代码签名）"
          core_unsigned=$((core_unsigned + 1))
          ;;
        *) fail "内核没有签名，且认不出架构：${rel}（${kind}）。${BUILD_SIDE}" ;;
      esac
      ;;
    *)
      echo "$detail" >&2
      fail "内核带有无效签名：${rel}。${BUILD_SIDE}"
      ;;
  esac
  # The adjacent source receipt binds the observed state/CDHash and final bytes.
  # Its upstream/source pins are checked by the existing packaging consumer.
  node --input-type=module - "$core_ref" "$f" "$script_dir" <<'NODE'
import { readFileSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';
const [reference, binary, scripts] = process.argv.slice(2);
const key = basename(dirname(reference));
const receipt = JSON.parse(readFileSync(join(dirname(dirname(reference)), '.source-receipts', `${key}.json`), 'utf8'));
const { verifyMacCodeSignatureReceipt } = await import(pathToFileURL(join(scripts, 'desktop-core/source-graph.mjs')));
verifyMacCodeSignatureReceipt(binary, receipt, key);
NODE
}

# helper：seal 模式下允许 ad-hoc 补签（本仓产物）。
check_helper() {
  local f="$1" kind="$2" rel="$3" level="warning"
  sig_state "$f"
  [ "$state" = "signed" ] && return 0
  if [ "$mode" = "verify" ]; then
    echo "$detail" >&2
    fail "最终产物里的 polaris-helper 没有有效签名：${rel}"
  fi
  # 分支顺序与 check_core 相同：universal 的 `file` 输出里同时有 x86_64 与 arm64，必须先被前一支接住。
  case "${state}:${kind}" in
    unsigned:*arm64* | unsigned:*universal*) ;;
    unsigned:*x86_64*) level="notice" ;;
  esac
  echo "::${level}::macos-nested-code: ${rel} 没有有效签名（${state}），ad-hoc 补签"
  codesign --force --sign - --timestamp=none "$f"
  sig_state "$f"
  if [ "$state" != "signed" ]; then
    echo "$detail" >&2
    fail "补签后仍验不过：${rel}"
  fi
  resigned=$((resigned + 1))
}

count=0
resigned=0
core_seen=0
core_unsigned=0
helper_seen=0
app_len=${#app}
while IFS= read -r -d '' f; do
  # 相对 bundle 的路径按长度截取，不走 `${f#"$app"/}`：路径里的 `*`、`[` 不该被当成通配，
  # 引号嵌在展开式里的写法在 bash 3.2 与新版本之间也不一致。
  rel="${f:$((app_len + 1))}"
  base="${f##*/}"
  if ! kind="$(file -b "$f")"; then
    fail "file 无法判断文件类型：${rel}"
  fi
  case "$kind" in
    Mach-O*) ;;
    *) continue ;;
  esac
  count=$((count + 1))
  case "$base" in
    sing-box)
      check_core "$f" "$kind" "$rel"
      core_seen=$((core_seen + 1))
      ;;
    polaris-helper)
      check_helper "$f" "$kind" "$rel"
      helper_seen=$((helper_seen + 1))
      ;;
    *)
      sig_state "$f"
      if [ "$state" != "signed" ]; then
        echo "$detail" >&2
        fail "未登记的 Mach-O 没有有效签名：${rel} —— 本脚本只替 polaris-helper 补签"
      fi
      ;;
  esac
done <"$list"

if [ "$core_seen" -lt 1 ]; then
  fail "Contents/Resources 下没有扫到 Mach-O「sing-box」（共扫到 ${count} 个 Mach-O）—— 逐个验没有验到它"
fi
if [ "$helper_seen" -lt 1 ]; then
  fail "Contents/Resources 下没有扫到 Mach-O「polaris-helper」（共扫到 ${count} 个 Mach-O）—— 逐个验没有验到它"
fi

echo "macos-nested-code(${mode}): Contents/Resources 下 ${count} 个 Mach-O 已逐个判定；内核 ${core_seen} 份与源内核逐字节相同（其中未签名的 x86_64 ${core_unsigned} 份，其余签名有效），polaris-helper ${helper_seen} 份签名有效（本次补签 ${resigned} 份）"
