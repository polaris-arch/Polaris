#!/usr/bin/env bash
# macos-nested-code.sh —— `.app` 里 `Contents/Resources` 之下的 Mach-O 逐个下判断。
#
# 用法：
#   bash scripts/macos-nested-code.sh <Polaris.app> <源内核文件>
#   <源内核文件> = 本次打包取用的那份 `resources/mac-<arch>/sing-box`（必填，没有缺省值）。
#   封印整个 bundle 前和打开最终产物后各做一次只读核验。
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
#   1. 内核 `sing-box` —— **只校验，不签、不改写**。
#      内核字节要与源码构建回执里的哈希对得上，打包链上任何一次改写（含补签）都会让它对不上。
#      判据两条，都必须成立：
#        a. 与 <源内核文件> 逐字节相同（`cmp`）。这是「打包过程没有改写内核」的正面断言 —— 不是靠
#           「本脚本没调签名命令」反推：bundle 封印、dmg 转换等别的步骤同样在射程内。
#        b. 通过发布核契约验证。在 macOS 上，发布核验证会用 `codesign --verify --strict` 观察实际签名，
#           并与契约中的签名状态和 CDHash 核对。纯 x86_64 且明确未签名时放行并留 notice。
#           依据：Intel 上执行 Mach-O 不要求代码签名；arm64 上没有有效签名的 Mach-O 一执行就被系统
#           终止，故 arm64（含 universal）没有这条例外。「有签名但验不过」在任何架构上都判失败。
#      内核验不过 ⇒ 打包失败，不在这里修：签名属于内核发布产物那一步（fetch-core 消费时由
#      `scripts/desktop-core/source-graph.mjs` 的 observeMacCodeSignature 观测并记入收据）。
#
#   2. `polaris-helper` —— 编译 App 之前已签并冻结。这里只校验源/载体
#      字节与签名；验不过即拒绝，不在包后补签，也不改写安装身份。
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

[ "$#" -eq 2 ] || fail "用法 macos-nested-code.sh <App.app> <源内核文件>" 2
app="$1"
core_ref="$2"
# 以 `-` 开头的相对路径会被后面的命令当成选项。
case "$app" in -*) app="./$app" ;; esac
case "$core_ref" in -*) core_ref="./$core_ref" ;; esac
res="$app/Contents/Resources"
if [ -z "$app" ] || [ ! -d "$res" ]; then
  fail "不是 app bundle（缺 Contents/Resources）：${app}" 2
fi
if [ -z "$core_ref" ] || [ ! -f "$core_ref" ] || [ -L "$core_ref" ]; then
  fail "第二个参数必须是源内核文件（普通文件，非链接）：'${core_ref}'" 2
fi
for tool in find file codesign cmp mktemp node; do
  command -v "$tool" >/dev/null 2>&1 || fail "找不到命令 ${tool}（本脚本只在带 Xcode 命令行工具的 macOS 上有意义）" 2
done

# 文件清单先落盘再读：`find` 中途失败（某个子目录读不了）时这里就失败，而不是带着半份清单继续。
list="$(mktemp "${TMPDIR:-/tmp}/polaris-nested-code.XXXXXX")"
trap 'rm -f "$list"' EXIT
find "$res" -type f -print0 >"$list" || fail "枚举 ${res} 失败"

verify_signature() {
  local detail
  if ! detail="$(codesign --verify --strict "$1" 2>&1)"; then
    echo "$detail" >&2
    fail "$2"
  fi
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

check_core() {
  local f="$1" rel="$2" signature
  if ! cmp -s "$f" "$core_ref"; then
    fail "包内内核与源内核字节不同：${rel} ≠ ${core_ref} —— 打包链上有步骤改写了内核"
  fi
  if ! signature="$(node --input-type=module - "$core_ref" "$f" "$script_dir" <<'NODE'
import { readFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const [reference, binary, scripts] = process.argv.slice(2);
const key = basename(dirname(reference));
const root = resolve(dirname(dirname(reference)), '..');
const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json'), 'utf8'));
const { verifyPackagedRelease } = await import(pathToFileURL(join(scripts, 'desktop-core/release-consumer.mjs')));
const { macCodeSignature } = verifyPackagedRelease(root, manifest, key, binary);
// The verifier matches the actual signature to this receipt only on Darwin.
if (process.platform !== 'darwin' || !macCodeSignature) {
  throw Error('Packaged Core signature verification requires macOS and a Mac release');
}
console.log(macCodeSignature.state);
NODE
  )"; then
    fail "包内内核未通过发布核验：${rel}"
  fi
  case "$signature" in
    signed) ;;
    unsigned)
      echo "::notice::macos-nested-code: ${rel} 是没有签名的 x86_64 内核，按原字节放行（Intel 上执行不要求代码签名）"
      core_unsigned=$((core_unsigned + 1))
      ;;
    *) fail "包内内核缺少已核验的签名状态：${rel}" ;;
  esac
}

# helper bytes were signed and frozen before compiling the App's installation identity.
check_helper() {
  local f="$1" rel="$2" reference
  reference="$(dirname "$core_ref")/polaris-helper"
  cmp -s "$f" "$reference" || fail "包内 helper 与 App 编译前冻结的 helper 不同：${rel}"
  verify_signature "$f" "helper 的签名必须在 App 编译前完成：${rel}"
}

count=0
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
      check_core "$f" "$rel"
      core_seen=$((core_seen + 1))
      ;;
    polaris-helper)
      check_helper "$f" "$rel"
      helper_seen=$((helper_seen + 1))
      ;;
    *)
      verify_signature "$f" "未登记的 Mach-O 没有有效签名：${rel} —— 本脚本不补签任何 Mach-O"
      ;;
  esac
done <"$list"

if [ "$core_seen" -lt 1 ]; then
  fail "Contents/Resources 下没有扫到 Mach-O「sing-box」（共扫到 ${count} 个 Mach-O）—— 逐个验没有验到它"
fi
if [ "$helper_seen" -lt 1 ]; then
  fail "Contents/Resources 下没有扫到 Mach-O「polaris-helper」（共扫到 ${count} 个 Mach-O）—— 逐个验没有验到它"
fi

# Verify the exact pre-compilation installation metadata and helper/core bytes.
core_key="$(basename "$(dirname "$core_ref")")"
node --input-type=module - "$res/_up_/resources/$core_key" "$(dirname "$core_ref")" "$core_key" "$script_dir" <<'NODE'
import { pathToFileURL } from 'node:url';
import { join } from 'node:path';
const [directory, source, platform, scripts] = process.argv.slice(2);
const { verifySelectedCoreManifest } = await import(pathToFileURL(join(scripts, 'lib/core-identity.mjs')));
verifySelectedCoreManifest(directory, source, platform);
NODE

echo "macos-nested-code: Contents/Resources 下 ${count} 个 Mach-O 已逐个判定；内核 ${core_seen} 份与源内核逐字节相同（其中未签名的 x86_64 ${core_unsigned} 份，其余签名有效），polaris-helper ${helper_seen} 份签名有效"
