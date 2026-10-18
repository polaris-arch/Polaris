#!/usr/bin/env bash
# gate-rust.sh —— .github/workflows/ci.yml 里 Rust 门（lint / cross / test 三个并行 job）的本机镜像。
# CI 侧 2026-10-07 起并行跑；本机仍按下面的顺序串行跑同一批命令，判据逐字相同（对拍见
# scripts/gate-rust-ci-parity.test.mjs，它按步骤名取材，不依赖步骤住在哪个 job）。
#
# 起因（2026-09-02）：本机手跑门时用的命令比 CI 弱，本机全绿但取材面比 CI 窄——
# 具体漏了 clippy 的 `--workspace`/`RUSTFLAGS="-D warnings"`，doc 门漏了
# `--bins --lib --document-private-items` 与四个 `-D rustdoc::*`。这份清单不该靠人记，
# 该由脚本逐字镜像 ci.yml。**改 CI 就要改这里**——两边由 gate-rust-ci-parity.test.mjs 钉死一致，
# 改一边不改另一边，那条测试会红。
#
# 为什么 doc 门不能省 `--bins --lib`（照抄 ci.yml「Rustdoc documentation invariants」步骤的
# 头注，用自己的话说一遍）：
#   - `src-tauri` 是 bin crate（没有 lib.rs）。`cargo doc` 不加 `--bins` 时，它的
#     `runtime`/`commands` 等模块整体不进 rustdoc 默认取材面——门读不到，坏链接照样 rc=0。
#   - 反过来只给 `--bins`、不给 `--lib`，会把 `--workspace` 的目标种类收窄成只剩 bin：
#     workspace 里一堆纯 lib 的 crate（`crates/*` 里没有 `[[bin]]` 的那些）被整批排除出取材面，
#     而不是被判「零断链」。
#   两个 flag 互不包含、互不能替代，必须同时给。
#
# 已知噪声（非缺陷）：`cargo doc` 会打两条 cargo#6313 的 warning——workspace 里的 bin crate 与
# lib crate 生成同名产物时的已知上游提示。rc 仍是 0，出现属正常，不代表本门失效。
#
# 资源占位（本脚本不做，只在失败时提示）：`resources/{linux,dashboard,win,mac-arm64,mac-x64}`
# 被 .gitignore 排除，裸 checkout/worktree 下本机通常没有这些目录，tauri 的 build.rs 会在
# build script 阶段报 "resource path doesn't exist" 而让 build 门失败。CI 用同名步骤
# （见 ci.yml「Create resource placeholder dirs」）建空占位目录 + .keep 文件。那是构建前置，
# 不是门本身要做的事，本脚本不替用户建——build 门失败时会打印怎么建。
#
# 硬约束：每条门单独取退出码（不经管道，`cmd | tail` 拿到的 $? 是 tail 的不是 cmd 的），
# 全部跑完再汇总，不因某一条失败就提前退出——故不用 `set -e`。跑法与 ci.yml 里
# 「Cross-target exemptions must still be necessary」步骤同一惯用法：`set -uo pipefail`
# （不含 -e），未绑定变量仍会报错，管道里非末尾命令失败仍会被 pipefail 捕获。
set -uo pipefail
cd "$(dirname "$0")/.."

# --with-cross：额外跑 ci.yml 里那四条**只在 Linux 跑、且需要联网**的门（两条跨目标 clippy /
# 豁免反腐烂，一条随仓路径源零诊断，加一条 Android 影响面 dep-info 对差）。
# 默认关闭的理由不是它们不重要，恰恰相反——它们守的是本机根本不编译的代码
# （`#[cfg(windows)]` / `#[cfg(target_os = "macos")]` / `#[cfg(target_os = "ios")]` 块里的错误
# 在本机编译取材面之外），以及只在 android 编译面上才看得见的那半张登记表。
# 关掉是因为首次运行要 `rustup target add` 下载目标工具链，而本机门的默认口径不碰网络。
# 目标已装时零下载，跑一次约多花几分钟（逐格耗时见各门自己的注释）。
WITH_CROSS=0
for arg in "$@"; do
  case "$arg" in
    --with-cross) WITH_CROSS=1 ;;
    -h|--help)
      cat <<'USAGE'
用法: scripts/gate-rust.sh [--with-cross]

  默认        跑 ci.yml 的 5 条常规 Rust 门（不联网）
  --with-cross 额外跑四条要交叉目标 / NDK 的门：对 ci.yml 跨目标步骤登记的每个目标跑
               clippy；随仓路径源（vendor/）在这些目标上零诊断；用 dep-info 对差查 Android
               影响面登记表完备不完备；检查
               scripts/cross-target-exempt.json 的豁免有没有腐烂。目标清单见下方 cross-clippy
               门本体（**本用法文本里刻意不重复三元组字面量**：gate-rust-ci-parity 的对拍门按
               整份文件抠三元组，写在这里会让「循环没跟上 ci.yml」的漂移照样判绿）。
               首次运行会 `rustup target add` 下载目标工具链（联网）。
USAGE
      exit 0 ;;
    *) echo "未知参数: $arg（用 --help 看用法）" >&2; exit 2 ;;
  esac
done

# ci.yml 把 NDK 解析做成一个独立步骤（"Resolve Android NDK"，结果写进 GITHUB_ENV 给后面几步共用）。
# 本脚本的对应物就是这个函数——`bash -c` 子壳之间不共享变量，故 export -f 后由用得着的门自己调。
# 口径与 ci.yml 同源：CI 取 runner 预装的 ANDROID_NDK_LATEST_HOME，本机等价物是 ANDROID_HOME/ndk
# 下版本号最高的**稳定**版。预发布版当场排除（实测 NDK 30 beta 会让 btls-sys 的 bindgen 报
# "Unversioned target triples are not supported"；成因写在 ci.yml 同名步的 🔴 段，本处不复述）。
# 找不到就返回非零并说清看过哪里，不静默回落成「没有 NDK 也往下走」——那会让调用方倒在
# 「找不到 C 工具链」上，报错点离真因两跳远。
#
# 已知重复：下面 cross-clippy / cross-exempt 两门的 `bash -c` 块里还各留着一份内联副本。
# 本批不收编它们：跑一次 --with-cross 要 `rustup target add`（联网），而本批的口径不碰网络
# ⇒ 改了验不了，不把未验证过的改动塞进两道已经在守事的门。
resolve_ndk() {
  local ah="${ANDROID_HOME:-$HOME/Android/Sdk}"
  local ndk="${ANDROID_NDK_HOME:-}" d rev
  if [ -z "$ndk" ]; then
    while read -r d; do
      rev=$(grep "^Pkg.Revision" "$ah/ndk/$d/source.properties" 2>/dev/null | cut -d= -f2 | tr -d " ")
      case "$rev" in *-*) continue ;; esac
      ndk="$ah/ndk/$d"
    done < <(find "$ah/ndk" -mindepth 1 -maxdepth 1 -type d -printf "%f\n" 2>/dev/null | sort -V)
  fi
  [ -n "$ndk" ] || { echo "找不到稳定 NDK（看过 $ah/ndk，环境里也没有 ANDROID_NDK_HOME）" >&2; return 1; }
  [ -d "$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin" ] || {
    echo "NDK 里没有 toolchain 目录：$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin" >&2
    return 1
  }
  printf '%s\n' "$ndk"
}
export -f resolve_ndk

gate_names=()
gate_rcs=()

run_gate() {
  local name="$1"
  shift
  echo
  echo "── ${name} ──"
  "$@"
  local rc=$?
  gate_names+=("$name")
  gate_rcs+=("$rc")
  echo "${name} rc=${rc}"
}

run_gate fmt cargo fmt --all -- --check
run_gate clippy env RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets -- -D warnings
run_gate rustdoc env RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links -D rustdoc::invalid_html_tags -D rustdoc::private_intra_doc_links -D rustdoc::redundant_explicit_links" cargo doc --no-deps --workspace --bins --lib --document-private-items
run_gate build cargo build --workspace --verbose
# ── 本机专属：禁起核（陈先生决策 2026-09-25）──
# 本机硬约束「不许起 sing-box run」，而 `cargo test --workspace` 里有真起核的门（盘上有随包核就真起）。
# 决策：本机跳过起核用例，覆盖交给 CI。判定与 `run` 的拼接只在
# crates/config-engine/tests/support/kernel_run.rs 一处（源码级门 kernel_run_single_entry.rs 钉死）。
# 只在非 CI（GITHUB_ACTIONS 未设）时 export；ci.yml / package.yml / release-risk.yml 不设，照常起核。
# 不改下面那条 `run_gate test` 行本身：它与 ci.yml 由 gate-rust-ci-parity.test.mjs 逐字对拍。
# 自曝：cargo test 吞掉通过用例的 stderr，故跳过记账写进文件、由本脚本汇总打印「跳过 N 个」；
# 若本机 shell 里还设着 POLARIS_REQUIRE_KERNEL_GATE=1，helper 会当场红（二者互斥）。
kernel_skip_log=""
if [ -z "${GITHUB_ACTIONS:-}" ]; then
  export POLARIS_NO_KERNEL_RUN=1
  kernel_skip_log="$(mktemp "${TMPDIR:-/var/tmp}/polaris-kernel-run-skip.XXXXXX")"
  export POLARIS_KERNEL_RUN_SKIP_LOG="$kernel_skip_log"
fi
run_gate test cargo test --workspace --no-fail-fast
if [ -n "$kernel_skip_log" ]; then
  kernel_skip_n=$(wc -l < "$kernel_skip_log")
  echo "⛔ 本机禁起核（POLARIS_NO_KERNEL_RUN=1）：跳过 ${kernel_skip_n} 个起核用例（覆盖交给 CI）"
  sort "$kernel_skip_log" | sed 's/^/   · /'
  rm -f "$kernel_skip_log"
fi

if [ "$WITH_CROSS" = 1 ]; then
  run_gate cross-clippy bash -c '
    set -euo pipefail
    rustup target add x86_64-pc-windows-msvc x86_64-apple-darwin aarch64-linux-android aarch64-apple-ios
    # android 那一格要 C 工具链（zstd-sys 的 cc-rs、btls-sys 的 CMake 子构建），另三个不要。
    # 判据、三点实测与「预发布 NDK 不进门」的成因，全在 ci.yml 的 "Resolve Android NDK" 步注释里，
    # 本处不复述（复述两份必然漂）。CI 取 runner 预装的 ANDROID_NDK_LATEST_HOME，
    # 本机等价物是 ANDROID_HOME/ndk 下版本号最高的**稳定**版。
    AH="${ANDROID_HOME:-$HOME/Android/Sdk}"
    NDK="${ANDROID_NDK_HOME:-}"
    if [ -z "$NDK" ]; then
      while read -r d; do
        rev=$(grep "^Pkg.Revision" "$AH/ndk/$d/source.properties" 2>/dev/null | cut -d= -f2 | tr -d " ")
        case "$rev" in *-*) continue ;; esac
        NDK="$AH/ndk/$d"
      done < <(find "$AH/ndk" -mindepth 1 -maxdepth 1 -type d -printf "%f\n" 2>/dev/null | sort -V)
    fi
    [ -n "$NDK" ] || { echo "找不到稳定 NDK —— android 那一格无从跑" >&2; exit 1; }
    NDK_BIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
    [ -d "$NDK_BIN" ] || { echo "NDK 里没有 toolchain 目录: $NDK_BIN" >&2; exit 1; }
    export ANDROID_NDK_HOME="$NDK" NDK_HOME="$NDK" ANDROID_NDK_ROOT="$NDK"
    echo "NDK: $NDK"
    # 覆盖面从 workspace 成员**推导**再减去豁免表，不手写清单——新建 crate 默认被覆盖。
    # 豁免**按 target** 生效（成因见 ci.yml 同名步的 🔴 段：只读 keys[] 会把只该在某一个 target
    # 上豁免的包从所有 target 一起摘掉）。
    mapfile -t ALL < <(cargo metadata --no-deps --format-version 1 | jq -r ".packages[].name" | sort)
    # 豁免还分两档（scope）：缺省/all = 整包跳过；tests = 只跳测试目标，该包仍跑 clippy -D warnings。
    # 成因见 ci.yml 同名步的 🔴 段（polaris-helper 在 android 上「编不过」与「真·未使用」是两类，
    # 整包豁免会把后者一起关掉）。
    for t in x86_64-pc-windows-msvc x86_64-apple-darwin aarch64-linux-android aarch64-apple-ios; do
      mapfile -t EXEMPT < <(jq -r --arg t "$t" "to_entries[] | select(.value.targets | index(\$t)) | select((.value.scope // \"all\") != \"tests\") | .key" scripts/cross-target-exempt.json)
      mapfile -t LIBONLY < <(jq -r --arg t "$t" "to_entries[] | select(.value.targets | index(\$t)) | select((.value.scope // \"all\") == \"tests\") | .key" scripts/cross-target-exempt.json)
      TARGETS=()
      for p in "${ALL[@]}"; do
        skip=0
        for e in "${EXEMPT[@]}" "${LIBONLY[@]}"; do [ "$p" = "$e" ] && skip=1; done
        [ "$skip" = 0 ] && TARGETS+=("-p" "$p")
      done
      echo "--- $t: 全目标覆盖 $(( ${#TARGETS[@]} / 2 )) 个包; 仅 lib/bins: ${LIBONLY[*]:-无}"
      # 「静默零执行」自曝：派生坏掉 / 豁免写宽了在这里红，而不是循环跑 0 个包也绿。
      # 下限**逐 target 给**，不是全局一个数（2026-09-06 加 ios 腿时改）：ios 实测要豁免 3 个包，
      # 另三个 target 各 2 个。把全局值降到 17 去迁就 ios，等于给另外三个 target 白送一格
      # 「没人会发现的豁免额度」—— 而那正是本断言存在的理由。每个数都是该 target 当天实测的
      # 派生数（20 个包 − 该 target 的豁免数）；新建 crate 只让实到值变大，再豁免一个包就在这里红。
      case "$t" in
        aarch64-apple-ios) FLOOR=17 ;;
        *) FLOOR=18 ;;
      esac
      test "$(( ${#TARGETS[@]} / 2 ))" -ge "$FLOOR"
      # 判据是 clippy -D warnings 不是 cargo check：实测同一份带 field_reassign_with_default 的
      # cfg(windows) 代码，cargo check 同 target rc=0（绿）而 clippy rc=101。
      # PATH 只在 android 那一格前置：NDK bin 里有 clang/ld/llvm-*，全局前置会影响宿主编译。
      extra=""
      [ "$t" = aarch64-linux-android ] && extra="$NDK_BIN:"
      PATH="${extra}${PATH}" cargo clippy --target "$t" --all-targets "${TARGETS[@]}" -- -D warnings
      if [ "${#LIBONLY[@]}" -gt 0 ]; then
        LIB_ARGS=()
        for p in "${LIBONLY[@]}"; do LIB_ARGS+=("-p" "$p"); done
        PATH="${extra}${PATH}" cargo clippy --target "$t" "${LIB_ARGS[@]}" -- -D warnings
      fi
    done
    # polaris 在 android 上被豁免出 clippy 循环（既有 lint 债，理由在豁免表），但编译面不能跟着丢：
    # 「改坏 Android 交叉编译」正是本门要抓的，而它只住在这个包里。check 不 deny warnings。
    # 🔴 **ios 上没有这条对应的补救，而且补不了**（2026-09-06 实测）：polaris 在 ios 上不是 lint 债，
    # 是 `objc2-exception-helper` / `zstd-sys` 两个 build script 直接倒在
    # `error occurred in cc-rs: failed to find tool "xcrun"` —— 那是**编译之前**的阶段（cc-rs 要 Apple
    # SDK），不是链接期，也不是 check 能绕过去的（`cargo check` 照样跑 build script）。
    # ⇒ src-tauri 那 12 处穷举 match 的 iOS 臂、以及 CFG_REGISTRY 记的 37 条 cfg 债，在**这台宿主上**
    # 一行都编不到。ios 这条腿今天守住的是另外 17 个纯 Rust 包的 iOS 编译面，别把它读成「整仓已验」。
    PATH="$NDK_BIN:$PATH" cargo check --target aarch64-linux-android -p polaris
  '
  # ── ci.yml「Vendored path sources stay diagnostic-free」的本机镜像 ──
  # 判据本体在被调脚本里，两侧调的是同一条命令（gate-rust-ci-parity.test.mjs 逐字对拍，并把这里的
  # 目标清单与上面 cross-clippy 的清单钉成同一份）。排在 cross-clippy 之后：目标由那一格
  # `rustup target add`。
  run_gate cross-vendored bash scripts/check-vendored-path-sources.sh x86_64-pc-windows-msvc x86_64-apple-darwin aarch64-linux-android aarch64-apple-ios
  # ── ci.yml「Android impact face must be registered (dep-info 对差)」的本机镜像 ──
  #
  # 它守的是：`ANDROID_IMPACT_SCOPES`（scripts/classify-ci-impact.mjs）是 android.yml 的**唯一**
  # 触发面，新增一个 Android 专属源文件而不登记 ⇒ APK 腿永远不为它跑。判据本体与它的射程/已知
  # 上限全在 scripts/check-android-only-face.mjs 的头注里，本处不复述。
  #
  # 逐字对拍抠不出来（多行块，不是一条 run:），故登记在 gate-rust-ci-parity.test.mjs 的
  # MIRRORED_BUT_NOT_VERBATIM 里，由那边一条**针对性**对拍钉住实质：两条 cargo check 的选择器
  # 两侧一致且互相对称、checker 的调用形态两侧一致。只登记名字不补跑法的话，本机对这件事的
  # 检出力恒为 0，而「MIRRORED」这个名字会说谎。
  #
  # 为什么挂在 --with-cross 下、而不是常规五门：它要 aarch64-linux-android 目标（首次得
  # `rustup target add`，联网）与 NDK 的 C 工具链——与上面两条门同一批前置。次序照抄 ci.yml：
  # 跨目标 clippy 之后、豁免反腐烂之前；上一格结尾那条 `cargo check --target … -p polaris`
  # 已把 android 侧的 check 单元编热，故本格 android 侧接近零成本。
  #
  # ⏱ 耗时（2026-09-05 本机实测，不是估的）：android 侧 59s；**host 侧才是本格的实质代价**——
  # 它与本脚本已有的 clippy（走 clippy-driver）、build（出 rlib）、test（test 构型）指纹**都不
  # 共享**，是一趟真的增量编译：冷跑约 95s，编热后 16s。看它停在 "Checking …" 一分多钟是正常的，
  # 不是卡住。
  #
  # 两条 cargo 的 rc 必须真的拦住这一步：dep-info 是类型检查**之前**产出的（rustc 对有类型错误
  # 的代码照样写出正确 `.d`），「.d 是对的」与「代码编得过」是两件事 ⇒ errexit 全程有效。
  # JSON 一律 `>` 重定向、不接管道（管道会让 cargo 自己的 rc 失真）。
  run_gate android-face bash -c '
    set -euo pipefail
    NDK="$(resolve_ndk)"
    export ANDROID_NDK_HOME="$NDK" NDK_HOME="$NDK" ANDROID_NDK_ROOT="$NDK"
    OUT="$(mktemp -d)"
    trap "rm -rf $OUT" EXIT
    PATH="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH" cargo check --workspace --target aarch64-linux-android --message-format=json-render-diagnostics > "$OUT/android-units.json"
    cargo check --workspace --message-format=json-render-diagnostics > "$OUT/host-units.json"
    node scripts/check-android-only-face.mjs --android "$OUT/android-units.json" --host "$OUT/host-units.json"
  '
  run_gate cross-exempt bash -c '
    set -uo pipefail
    rot=0
    # 豁免腐烂检查必须与主循环量同一件事：少了 NDK，android 上的豁免会因为「找不到 C 工具链」
    # 而失败，于是本步得出「豁免仍必要」——理由却是假的。
    AH="${ANDROID_HOME:-$HOME/Android/Sdk}"
    NDK="${ANDROID_NDK_HOME:-}"
    if [ -z "$NDK" ]; then
      while read -r d; do
        rev=$(grep "^Pkg.Revision" "$AH/ndk/$d/source.properties" 2>/dev/null | cut -d= -f2 | tr -d " ")
        case "$rev" in *-*) continue ;; esac
        NDK="$AH/ndk/$d"
      done < <(find "$AH/ndk" -mindepth 1 -maxdepth 1 -type d -printf "%f\n" 2>/dev/null | sort -V)
    fi
    if [ -n "$NDK" ]; then
      export ANDROID_NDK_HOME="$NDK" NDK_HOME="$NDK" ANDROID_NDK_ROOT="$NDK"
      export PATH="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH"
    fi
    for e in $(jq -r "keys[]" scripts/cross-target-exempt.json); do
      for t in $(jq -r --arg e "$e" ".[\$e].targets[]" scripts/cross-target-exempt.json); do
        if cargo clippy --target "$t" --all-targets -p "$e" -- -D warnings >/dev/null 2>&1; then
          echo "豁免已腐烂：$e 现在能过 $t —— 从 scripts/cross-target-exempt.json 删掉它" >&2
          rot=1
        fi
      done
    done
    exit $rot
  '
fi

build_rc=0
for i in "${!gate_names[@]}"; do
  [ "${gate_names[$i]}" = build ] && build_rc="${gate_rcs[$i]}"
done
if [ "$build_rc" -ne 0 ]; then
  cat >&2 <<'EOF'

build 门失败：如果报错含 "resource path doesn't exist"，是本机缺 .gitignore 排除的
resources/{linux,dashboard,win,mac-arm64,mac-x64} 占位目录（不是代码问题）。建齐后重跑：
  for d in dashboard linux win mac-arm64 mac-x64; do mkdir -p "resources/$d" && touch "resources/$d/.keep"; done
EOF
fi

echo
echo "===== gate-rust 汇总 ====="
overall_rc=0
for i in "${!gate_names[@]}"; do
  printf '%-8s rc=%s\n' "${gate_names[$i]}" "${gate_rcs[$i]}"
  [ "${gate_rcs[$i]}" -ne 0 ] && overall_rc=1
done
exit "$overall_rc"
