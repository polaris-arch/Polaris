#!/usr/bin/env bash
# gate-android-release-behavior.sh —— Android release 路径的**行为**裁判（配置期）。
#
# ── 为什么存在（根因）────────────────────────────────────────────────────────────
#
# release 路径此前的全部判据都是源码级的：「`build.gradle.kts` 里含某串」。2026-09-05 的
# 逐行变异验收在 30 多条探针里打出 **24 条门全绿**，每一条的形状都一样 ——
# **文本里那串还在，行为已经变了**：
#
#   · `proguardFiles(...)` 的实参只喂三份规则里的一份 ⇒ 另两份整体失效，门全绿；
#   · 守卫谓词 `||` 改 `&&` ⇒ 零凭据跑 release 的 rc 由 1 变 0，守卫永久闭嘴，门全绿；
#   · 守卫体顶部插一句 `return@Action` ⇒ 被断言的串一个不少，守卫既不判也不报，门全绿；
#   · `isMinifyEnabled = true` 后面追一行 `false` ⇒ `minifyArm64ReleaseWithR8` 从任务图消失，
#     本批全部 keep / dontwarn 一次都不生效，门全绿；
#   · 逃生门读取点后面接 `|| System.getenv(...)` ⇒ 永久开关从另一扇门回来，门全绿。
#
# 判据形态选错了：要守的是构建的**行为**，判据却在断言构建脚本的**文本**。
# 本脚本换裁判 —— 跑一次 gradle 配置期，问 Gradle 自己。
#
# ── 判据都问同一个人：gradle 的配置期 ───────────────────────────────────────────
#
#   ./gradlew --offline --no-daemon :app:assembleArm64Release --dry-run     # 看 rc / 任务图 / 报错原文
#   ./gradlew --offline --no-daemon :app:printPolarisReleaseFacts           # 看 AGP 手里的事实
#
# `--dry-run` 只跑到任务图就停：`gradle.taskGraph.whenReady` 的守卫照常开口（它挂在图上，
# 不挂在执行上），而一个字节的代码都不用编。本机实测一轮约 12 秒。
# `printPolarisReleaseFacts` 住在 `app/polaris-release-facts.gradle.kts`（**不是**被审判的
# `app/build.gradle.kts`），把 AGP 已经装配好的 `proguardFiles` 清单、签名配置的 `storeFile`、
# 逃生门的三个观察值原样打出来 —— 判断在这里，构建脚本不给自己判分。
#
# ═══════════════════════════════════════════════════════════════════════════════
# 🔴 能力边界（天花板声明，2026-09-05 收官轮立；2026-09-06 终止轮逐句复核并收窄）
# —— 这一段是判据的一部分，不是免责声明
# ═══════════════════════════════════════════════════════════════════════════════
#
# 本裁判跑的是 gradle 的**配置期**（`--dry-run` 到任务图为止 + 一个只读任务）。
# 而 release 构建的另一半发生在**执行期**：任务真的跑起来、R8 真的读文件、产物真的写出来。
# 配置期看得见的与看不见的，界线是硬的：
#
# ┌── 看得见（本裁判守，逐条对应下面的 ①–⑩）────────────────────────────────────┐
# │ · 任务图里有哪些任务（`minifyArm64ReleaseWithR8` 在 release 图里 1 次 / debug 图里 0 次）│
# │ · `gradle.taskGraph.whenReady` 守卫开不开口、报的是哪一句、点不点名到变量            │
# │ · AGP 装配好的 `proguardFiles`：release **构建类型**那一份的路径、份数、逐份字节数、 │
# │   集合是否恰等（⑦）；以及 `defaultConfig` 与 `productFlavor` 两处必须为空（⑦b）——   │
# │   AGP 口径是三处合并后喂给 R8（**本轮没在 R8 输入那一层实测**，那要真跑一次构建）；  │
# │   实测到的是**取材面**：往 `defaultConfig` 挂一份 `.pro`，⑦b 之前的判据零失败 rc=0。 │
# │   两处为空时，buildType 那一份就是本仓自有规则的全集，⑦ 的恰等才名副其实            │
# │ · 逃生门的来源，**只覆盖枚举得到的这四形**：命令行 `-P` 认、`gradle.properties` 不认、│
# │   `ORG_GRADLE_PROJECT_polarisAllowUnsigned` 不认、`POLARIS_ALLOW_UNSIGNED` 不认      │
# │ · 签名的**容器侧**：`signingConfigs` 里那份 `release` 配置的 `storeFile` 指向哪儿、  │
# │   在不在盘上、读不读得动、是不是 `POLARIS_KEYSTORE_FILE` 指的那一份                  │
# └───────────────────────────────────────────────────────────────────────────────┘
#
# ┌── 看不见（**结构上**看不见，本裁判不假装守）───────────────────────────────────┐
# │ ① 任务被挂上执行期开关。`tasks.matching { … }.configureEach { onlyIf { false } }`   │
# │    或 `enabled = false`：任务**仍在任务图里**（`--dry-run` 照样打印它），只是真跑时  │
# │    被跳过。本裁判的 ⑤ 数的是任务图，数不出「它到时候会不会真跑」。                   │
# │ ② `doFirst` / `doLast` 在执行期改文件。`doFirst { proguard-rules.pro.writeText("") }`│
# │    ⇒ 配置期读到的清单与字节数全是对的，R8 到手的却是一份空规则。⑦ 量的是配置期那一刻。│
# │ ③ 任务真正执行后的**产物**：R8 吃进去的规则全文、每条 keep 命中了什么、包里还剩什么。 │
# │ ④ **签名这件事本裁判只看到容器那一层**（2026-09-06 终止轮 R4 —— 上一版把「签名」整个 │
# │    写进「看得见」，那句话过宽）。今天看得见的只有：`signingConfigs` 容器里那份配置的  │
# │    `storeFile` 指哪儿、在不在、读不读得动。看不见的是另外两层：                      │
# │    · **接线**：release 构建类型到底有没有把那份配置接上（`buildType.signingConfig`）。│
# │      ⑩ 读的是 `signingConfigs.findByName("release")`，也就是**容器**；守卫读的       │
# │      `releaseSigningConfig` 同样是容器。两条判据都不看 buildType 那一侧。            │
# │      实测（终止轮 R4）：在 release 块里那行接线之后补一句 `signingConfig = null`，    │
# │      本裁判零失败 rc=0，源码侧 12 项全绿 —— 因为源码侧数的是那行接线**出现         │
# │      几次**，而它一次没少。                                                         │
# │    · **有效性**：storeFile 是不是一个合法的密钥库、口令对不对、别名存不存在           │
# │      （那三件只有 apksigner / jarsigner / keytool 真跑才知道）。                     │
# │ ⑤ 逃生门**换一个新变量名**那一形（2026-09-06 终止轮 R3）。两侧门各守一头，中间有缝：  │
# │    · 本裁判的 ④/④b 是一张**枚举表**：只试上面列出的那四个来源，新名字它不试。         │
# │    · 源码侧那条形状断言钉死的是**读取点那一条绑定**（`val allowUnsignedRelease` 的整  │
# │      条右值逐字相等），管不到守卫**体内**别处新写的读取。                            │
# │    缝在中间：在守卫体里、逃生门分支**之前**插一句                                    │
# │    `if (System.getenv("POLARIS_XXX") == "true") return@Action`，绑定一字未动、        │
# │    枚举表里没有这个名字 ⇒ 两侧同时全绿，而这是一个从环境继承、永久生效的逃生门。      │
# │    实测（终止轮 R3）：源码侧 12 项全绿，本裁判零失败 rc=0。                          │
# │    **这一形今天没有门。** 不再加第三道门去堵它 —— 判据表的边界就写在这里。            │
# └───────────────────────────────────────────────────────────────────────────────┘
#
# ┌── 那一层归谁守（逐条点名，不许一句「归 release-smoke」了事）──────────────────┐
# │ ①②③ 归 **`.github/workflows/android.yml` 的 `release-smoke` job**：真跑一次 release  │
# │ 构建（联网拉 R8 + 全量交叉编译 + `assembleArm64Release`），判据是                    │
# │ `scripts/assert-r8-evidence.mjs` 读 R8 的 configuration.txt / seeds.txt。            │
# │                                                                                    │
# │ 🔴 **它至今一次都没跑过**（`if: inputs.release_smoke`，默认 false；且今天            │
# │    **没有任何自动调用方**会传 true —— release-risk.yml 的 android job 是一句裸       │
# │    `uses: ./.github/workflows/android.yml`，不带 `with:`。只能手动                   │
# │    `workflow_dispatch` 勾上，而本仓从未这么触发过）。①②③ 今天没有任何东西在守。      │
# │                                                                                    │
# │ ④ 的两层**都不归 release-smoke**（2026-09-06 终止轮 R5 —— 上一版那句「那一层归      │
# │ release-smoke」对签名不成立）：那条腿跑的是                                          │
# │ `./gradlew assembleArm64Release … -PpolarisAllowUnsigned=true`，走的正是逃生门，     │
# │ 出的就是 `app-arm64-release-unsigned.apk`；CI 上没有密钥库、也不该有。               │
# │ 而且全仓 `apksigner` / `jarsigner` 一次都没出现过（`keytool` 只在失败信息的教学文本   │
# │ 里）。**签名的接线与有效性今天没有任何门** —— 本机手签一次是唯一的观察面。            │
# │                                                                                    │
# │ ⑤ 不归任何人（见上）。                                                              │
# │                                                                                    │
# │ 部分补偿（只补 ①②，且只补文本这一层）：`src-tauri/tests/android_release_signing_    │
# │ wiring.rs` 的 `execution_phase_hooks_must_be_registered` 要求 `onlyIf` / `enabled` /│
# │ `doFirst` / `doLast` 在**三份**入库的 gradle 脚本里逐处登记（app 的                  │
# │ `build.gradle.kts`、事实任务、以及工程根那份）。那是**文本判据**，能被什么绕过        │
# │ 写在它自己的头注里（`buildSrc/` 那一形它就看不见）—— 别把它当行为判据用。            │
# └───────────────────────────────────────────────────────────────────────────────┘
#
# 本仓成文口径：**一条不受判据管辖的声明，比没有这句话更坏。** 上面这张表就是按这条写的：
# 能补的补进 ①–⑩，补不了的指名归属并写明它今天没跑。
#
# ── 本机怎么跑（与 CI 逐字同一条命令）──────────────────────────────────────────
#
#   JAVA_HOME=<JDK 21> ANDROID_HOME=<Android SDK> bash scripts/gate-android-release-behavior.sh
#
# 判据要 Android SDK，故 `ci.yml` 的 Rust 各 job（lint / cross / test）跑不了它（那里只有 Rust 工具链）。
# 它挂在 `.github/workflows/android.yml` 的 `apk` job —— 那条腿本来就要装 SDK/NDK 并铺
# `tauri android build` 的生成物，而本裁判的取材面（三份 `.pro` 里有两份是生成物）恰好要求
# 生成物在场。挂在 `release-smoke` 是错的：那条腿默认不跑，而这些判据必须每次都说话。
#
# 🔴 但「挂上了」不等于「跑过」（2026-09-06 终止轮实测，只读 API）：远端默认分支 `main` 上
#    **没有** `.github/workflows/android.yml`（`gh api repos/…/contents/…` 返回 404；
#    `actions/workflows` 只列得出 ci / package / release-risk / ui 四份），本分支也从未推送。
#    也就是说本裁判至今**一次都没在 CI 上跑过** —— 它今天的全部观测都来自本机手跑。
#    这不改判据本身，但「CI 上有一道门在守 release 路径」这句话今天还不成立。
#
# ── 硬要求 ──────────────────────────────────────────────────────────────────────
#
#   · gradle 跑不起来（缺 JDK / 缺 SDK / 缺生成物 / 超时）⇒ **抛并点名**，不许 skip、
#     不许当作通过。「没跑」与「跑过且通过」必须可分辨。
#   · 一律 `--offline --no-daemon`（不留守护进程，不碰网络）。
#   · 禁 kill/pkill。
#   · 每条断言取材前先自检取材面非空；rc 一律直接取，不经管道。
#
# ── 裁判自己也要有门（2026-09-05 收官轮 A1）─────────────────────────────────────
#
# 实测：把 `bad()` 里的 `FAILED=$((FAILED + 1))` 删掉（bash 语法照过）⇒ 裁判先打印
# `✗ minifyArm64ReleaseWithR8 命中 0 次`，紧接着打印「全部行为断言通过」并 **rc=0**。
# 接管四条判据的东西自己是空的。今天两层守它：
#
#   · **三通道对账**：每条失败同时走三条独立通道 —— 打印到 stdout、抄进 `$TRANSCRIPT`、
#     记进 `$LEDGER` 台账、计入 `$FAILED`。收尾时三者必须相等，不等一律 rc=2（裁判坏了，
#     不是判据红了 —— 两件事必须可分辨）。删掉其中任何一条，对账当场不平。
#   · **哨兵自检**：正式判据之前，先用 `POLARIS_GATE_SELFCHECK=fail` 把自己再跑一遍，
#     那一轮**只有一条必然失败的断言**。它的 rc 必须非零、必须打出那条 `✗`。
#     反向对照是 `POLARIS_GATE_SELFCHECK=pass`（只有一条必然通过的断言）：rc 必须为 0。
#     两轮都不碰 gradle、不碰工作树，隔离运行，约 50 毫秒。
#     `bad()` 整个被掏空时，`fail` 那轮会 rc=0 ⇒ 当场 die。
#   · **出口盖章**（2026-09-06 终止轮 R1）：上面两层都要 `finish()` **跑起来**才说话。
#     实测删掉本文件末尾那一句 `finish`、或在 `self_check` 之后插一句 `exit 0` —— 两条都
#     rc=0，而判据一条都没定罪。今天 rc 只许从 `die()` / `finish()` 两个出口出来，两者退出前
#     给 `EXIT_SEALED` 盖章，EXIT 上验章：没盖章一律改判 rc=2。实现见下方 `on_exit`。
#     🔴 它守不住的：把 `on_exit` / `trap on_exit EXIT` 本身一起删掉。那是把门拆了，
#     不是从门缝里溜过去 —— 本裁判的自守到此为止，再往上一层今天没有门。
#
# ── 临时状态：三条断言必须现场造出「另一种世界」，用完还原 ────────────────────
#
#   ③ 要往 `gen/android/gradle.properties` 写一行；⑧ 要让一份 `.pro` 暂时不在；
#   ⑨ 要往 `app/` 放一份没登记的 `.pro`。
#   三处都是 `cp` 备份 → 改 → 跑 → `cp` 还原 → `cmp` 验，并挂 trap（含中断路径）。
#   还原不到位时本脚本自己会红（末尾 `cmp` 与「探针文件不许留下」那条），而且 `cargo test`
#   那条「gradle.properties 里不许有 polarisAllowUnsigned」的断言也会红 —— 两层都能自曝。
set -uo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
GRADLE_DIR="$REPO/src-tauri/gen/android"
PROPS="$GRADLE_DIR/gradle.properties"
# ⑧ 拿它做「缺一份规则文件」的实验对象：三份里它是生成物（gitignored），
# 且不是本仓入库的那份 —— 实验失败时不会碰到入库内容。
SACRIFICIAL_RULE="$GRADLE_DIR/app/proguard-tauri.pro"
# ⑨ 「多一份没登记的规则文件」的实验对象。名字带双下划线前后缀，且只在本脚本里出现，
# 不与任何真规则文件同名；trap 负责它一定被删掉。
PROBE_RULE="$GRADLE_DIR/app/__gate_probe__.pro"

WORK="$(mktemp -d /var/tmp/polaris-release-oracle.XXXXXX)"
PROPS_BAK="$WORK/gradle.properties.bak"
RULE_BAK="$WORK/proguard-tauri.pro.bak"
# 三通道对账的另外两条通道（第三条是 $FAILED 计数器）。
TRANSCRIPT="$WORK/transcript"
LEDGER="$WORK/failures"
: >"$TRANSCRIPT"
: >"$LEDGER"

# 前提不成立（工具链缺席、取材面为空、裁判自身对账不平）一律走这里：rc=2，与「判据红了」的
# rc=1 刻意分开。「没跑」与「跑过且通过」必须可分辨 —— 本裁判在任何前提不成立时都不当作通过。
die() {
  EXIT_SEALED=die
  echo "::error::gate-android-release-behavior: $*" >&2
  echo >&2
  echo "rc=2 表示**前提不成立**（跑不起来 / 裁判自身坏了），不是判据失败（那是 rc=1）。" >&2
  exit 2
}

restore() {
  # 幂等：两份备份都在时无条件写回（改没改过都写，省得判断状态）。
  [ -f "$PROPS_BAK" ] && cp -- "$PROPS_BAK" "$PROPS"
  [ -f "$RULE_BAK" ] && cp -- "$RULE_BAK" "$SACRIFICIAL_RULE"
  # ⑨ 的探针文件从不属于工作树：中断路径上也必须消失，否则它会永久改掉 R8 的规则集。
  rm -f -- "$PROBE_RULE"
  return 0
}

# ── 出口盖章：「本裁判跑完了」这件事本身必须可判（2026-09-06 终止轮 R1）────────
#
# 三通道对账（A1）守的是「bad() 少写一条通道」，而它要 finish() **跑起来**才说话 ——
# 守不住「根本没走到 finish」。实测两条变异，两条都 rc=0：
#
#   · 删掉本文件末尾那一句 `finish` ⇒ 脚本从末尾自然落地，rc 取最后一条命令（还原自检那个
#     `if`）的 0。判据一条都没定罪，而 rc 与「全部通过」逐字相同。
#   · 在 `self_check` 之后插一句 `exit 0` ⇒ gradle 一次都不跑，rc=0。
#
# 今天把 rc 的**产生**收到两个出口：`die()`（前提不成立，rc=2）与 `finish()`（判据结论，
# rc=0/1/2）。两者都在这里盖一个章，EXIT 上验章：没盖章就走到 EXIT ⇒ 退出码是**别的东西**
# 给的，一律改判 rc=2（裁判坏了，不是判据红了 —— 与 A1 的对账不平同一档）。
EXIT_SEALED=""
on_exit() {
  local rc=$?
  restore
  [ -n "$EXIT_SEALED" ] && return 0
  echo >&2
  echo "::error::gate-android-release-behavior: **裁判没跑完** —— 退出码不是 die/finish 给的（实得 rc=$rc）。" >&2
  echo "本裁判的 rc 只许从两个出口出来：die()（前提不成立）与 finish()（判据结论），" >&2
  echo "两者都会在退出前给 EXIT_SEALED 盖章。走到这里说明中途有别的东西退出了脚本 ——" >&2
  echo "末尾那句 finish 被删掉、或某处插了一句 exit。那一形下判据一条都没定罪，" >&2
  echo "而 rc 可以是 0，与「跑完且全过」在 shell 层逐字不可分辨。" >&2
  exit 2
}
# INT/TERM 仍只还原（保持既有语义：还原完继续走，最终仍会落到 EXIT 上验章）。
trap on_exit EXIT
trap restore INT TERM

FAILED=0
# 每一行判据结论同时走两条通道：stdout（给人看）与 $TRANSCRIPT（给对账看）。
emit() { printf '%s\n' "$*"; printf '%s\n' "$*" >>"$TRANSCRIPT"; }
ok()   { emit "  ✓ $*"; }
# 失败信息里的换行先压平：三通道对账按**行**计数，一条多行的失败信息会把台账算成好几条，
# 于是「裁判坏了」与「失败信息比较长」变得不可分辨。
bad()  {
  local msg="$*"
  msg="${msg//$'\n'/ ⏎ }"
  emit "  ✗ $msg"
  printf '%s\n' "$msg" >>"$LEDGER"
  FAILED=$((FAILED + 1))
}

# ── 收尾：先给裁判自己对账，再给判据结论定 rc ────────────────────────────────
#
# 顺序是刻意的：「裁判坏了」必须比「判据红了」先说话，也必须用不同的 rc（2 vs 1）——
# 两者混在一起的话，一个坏掉的裁判可以用一条真失败把自己藏起来。
finish() {
  EXIT_SEALED=finish
  local printed ledger
  printed=$(command grep -c '^  ✗ ' "$TRANSCRIPT")
  [ -n "$printed" ] || printed=0
  ledger=$(command grep -c '' "$LEDGER")
  [ -n "$ledger" ] || ledger=0

  if [ "$printed" -ne "$FAILED" ] || [ "$ledger" -ne "$FAILED" ]; then
    echo >&2
    echo "::error::gate-android-release-behavior: **裁判自身坏了**（三通道对账不平）：" >&2
    echo "  打印出来的 ✗ ：$printed 条" >&2
    echo "  台账里记下的 ：$ledger 条" >&2
    echo "  计数器 FAILED：$FAILED" >&2
    echo >&2
    echo "三条通道由 bad() 一处同时写，不平就说明 bad() 被改过（少写一条通道）。" >&2
    echo "这不是判据红了，是判据的裁决**带不动退出码** —— rc=2 与判据失败的 rc=1 刻意分开。" >&2
    [ "${SELFCHECK:-}" = "" ] || rm -rf -- "$WORK"
    exit 2
  fi

  echo
  # 自检轮不碰 gradle，也就没有 gradle 日志可留 —— 报一个不存在的路径同样是不实之词。
  if [ -n "${SELFCHECK:-}" ]; then
    rm -rf -- "$WORK"
    if [ "$FAILED" -eq 0 ]; then
      echo "gate-android-release-behavior: 全部行为断言通过（裁判自检轮，未跑 gradle）"
      exit 0
    fi
    echo "::error::gate-android-release-behavior: $FAILED 条行为断言失败（裁判自检轮）"
    exit 1
  fi
  if [ "$FAILED" -eq 0 ]; then
    echo "gate-android-release-behavior: 全部行为断言通过（日志在 $WORK）"
    rm -rf -- "$WORK"
    exit 0
  fi
  echo "::error::gate-android-release-behavior: $FAILED 条行为断言失败；gradle 日志留在 $WORK"
  exit 1
}

# ══ 裁判自检：正式判据之前，先证明「✗ 一定带得动 rc」════════════════════════════
#
# 隔离模式：不碰 gradle、不碰工作树，只跑一条必然失败（或必然通过）的哨兵断言，
# 走的是与真判据**同一段** bad()/ok()/finish() 代码。
SELFCHECK_FAIL_LABEL="哨兵断言：这一条必然失败（裁判自检用，不是真判据）"
SELFCHECK_PASS_LABEL="哨兵断言：这一条必然通过（裁判自检的反向对照）"
SELFCHECK="${POLARIS_GATE_SELFCHECK:-}"
case "$SELFCHECK" in
  fail) bad "$SELFCHECK_FAIL_LABEL"; finish ;;
  pass) ok  "$SELFCHECK_PASS_LABEL"; finish ;;
  "") ;;
  *) die "POLARIS_GATE_SELFCHECK 只认 fail / pass，实得：$SELFCHECK" ;;
esac

self_check() {
  local rc
  POLARIS_GATE_SELFCHECK=fail bash "$0" >"$WORK/selfcheck-fail.log" 2>&1
  rc=$?
  case "$rc" in
    1) ;;
    0) die "裁判自检（fail 轮）rc=0 —— 一条必然失败的断言没能把退出码带成非零。\
失败通道与 rc 脱节 ⇒ 本裁判此刻接管的判据全部没有信息量。日志：$WORK/selfcheck-fail.log" ;;
    2) die "裁判自检（fail 轮）rc=2 —— 三通道对账不平，裁判自身坏了（bad() 少写了一条通道）。\
原文：$(command grep -E '打印出来的|台账里记下的|计数器' "$WORK/selfcheck-fail.log" | tr '\n' ' ')" ;;
    *) die "裁判自检（fail 轮）rc=$rc，期望 1。日志：$WORK/selfcheck-fail.log" ;;
  esac
  command grep -q -F -- "✗ $SELFCHECK_FAIL_LABEL" "$WORK/selfcheck-fail.log" || die \
    "裁判自检（fail 轮）rc 对了，但那条 ✗ 一个字都没打出来 —— 失败通道与退出码脱节了。"
  command grep -q -F -- "全部行为断言通过" "$WORK/selfcheck-fail.log" && die \
    "裁判自检（fail 轮）在失败之后仍然打印了「全部行为断言通过」—— 收尾逻辑与计数脱节。"

  # 反向对照：没有它，上面三条会被一个「永远 rc≠0」的坏收尾满足。
  POLARIS_GATE_SELFCHECK=pass bash "$0" >"$WORK/selfcheck-pass.log" 2>&1
  rc=$?
  [ "$rc" -eq 0 ] || die \
    "裁判自检（pass 轮）rc=$rc，期望 0。只有一条必然通过的断言却判成了失败 ⇒ \
本裁判恒红，红也没有信息量。日志：$WORK/selfcheck-pass.log"
  command grep -q -F -- "全部行为断言通过" "$WORK/selfcheck-pass.log" || die \
    "裁判自检（pass 轮）rc=0 却没打印通过行 —— 收尾逻辑没走到。"
  echo "══ ⓪ 裁判自检：失败通道带得动 rc（fail 轮 rc=1）、通过通道也走得通（pass 轮 rc=0）══"
}
self_check

# ══ 前置：工具链必须在场，缺了当场死 ══════════════════════════════════════════
[ -x "$GRADLE_DIR/gradlew" ] || die "找不到 $GRADLE_DIR/gradlew —— Android 工程没铺出来。"
if [ -n "${JAVA_HOME:-}" ]; then
  [ -x "$JAVA_HOME/bin/java" ] || die "JAVA_HOME=$JAVA_HOME 下没有 bin/java。"
  export PATH="$JAVA_HOME/bin:$PATH"
fi
command -v java >/dev/null 2>&1 || die "PATH 上没有 java，且 JAVA_HOME 未指向一个 JDK。AGP 8.11 要 JDK 17–21。"
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
[ -n "$SDK" ] && [ -d "$SDK" ] || die "ANDROID_HOME / ANDROID_SDK_ROOT 没指向一个存在的 Android SDK。"
export ANDROID_HOME="$SDK"
# 三份 `.pro` 里有两份是 `tauri android build` 的生成物：不在场时下面 ⑦⑧ 的取材面是空的，
# 「三份都在」会退化成对空清单恒真 —— 故在这里当场死，而不是让断言去撞。
for rel in \
  app/proguard-rules.pro \
  app/proguard-tauri.pro \
  app/src/main/java/com/polaris2/app/generated/proguard-wry.pro
do
  [ -s "$GRADLE_DIR/$rel" ] || die "$rel 不在场或为 0 字节 —— 先跑一次 \`tauri android build --debug --apk --target aarch64\` 把生成物铺出来。"
done
# ⑨ 的探针文件必须从「不在场」出发：上一轮中断留下来的话，⑦ 的集合恰等会红在一个假原因上。
[ -e "$PROBE_RULE" ] && die "$PROBE_RULE 已经在场 —— 上一轮的探针没清干净，先删掉它再跑。"

cp -- "$PROPS" "$PROPS_BAK" || die "备份 gradle.properties 失败"
cp -- "$SACRIFICIAL_RULE" "$RULE_BAK" || die "备份 proguard-tauri.pro 失败"

# ══ 跑一轮 gradle；rc 直接取，日志落盘供断言取材 ═══════════════════════════════
#
# 每一轮都从**同一个干净环境**出发：把四个签名环境变量与全部 ORG_GRADLE_PROJECT_* 摘掉，
# 免得跑这条门的人自己机器上的凭据把「零凭据」这个前提悄悄改掉。
RC=0
gradle_run() {
  local log="$1"; shift
  local -a clean_env=(env
    -u POLARIS_KEYSTORE_FILE -u POLARIS_KEYSTORE_PASSWORD -u POLARIS_KEY_PASSWORD
    -u POLARIS_KEY_ALIAS -u POLARIS_ALLOW_UNSIGNED
    -u ORG_GRADLE_PROJECT_polarisAllowUnsigned)
  ( cd "$GRADLE_DIR" && "${clean_env[@]}" "$@" ) >"$log" 2>&1
  RC=$?
  return 0
}

# ⑩ 专用：凭据**齐全**那条分支。密钥库用的是一个现造的普通文件（不是密钥，也不需要是）——
# 配置期从不读它的内容，只问「在不在、读不读得动、是不是本工程认的那一份」。
# 口令/别名是三个显式的占位串，写在这里就是要让人一眼看出它们不是秘密。
FAKE_KEYSTORE="$WORK/not-a-real-keystore.bin"
FAKE_SECRET="gate-probe-placeholder-not-a-secret"
gradle_run_signed() {
  local log="$1" keystore="$2"; shift 2
  ( cd "$GRADLE_DIR" && env \
      -u POLARIS_ALLOW_UNSIGNED -u ORG_GRADLE_PROJECT_polarisAllowUnsigned \
      POLARIS_KEYSTORE_FILE="$keystore" \
      POLARIS_KEYSTORE_PASSWORD="$FAKE_SECRET" \
      POLARIS_KEY_PASSWORD="$FAKE_SECRET" \
      POLARIS_KEY_ALIAS="$FAKE_SECRET" \
      "$@" ) >"$log" 2>&1
  RC=$?
  return 0
}

# 取材前自检：日志为空 = gradle 根本没说话，下面全部断言都在空面上作证。
count_in() { # <file> <needle> -> stdout 次数
  local f="$1" needle="$2" n
  [ -s "$f" ] || die "取材面为空：$f 一个字节都没有 —— gradle 没跑起来。"
  n=$(command grep -c -F -- "$needle" "$f")
  [ -n "$n" ] || n=0
  printf '%s' "$n"
}
expect_rc() { # <label> <期望 0|nonzero>
  local label="$1" want="$2"
  if [ "$want" = 0 ]; then
    [ "$RC" -eq 0 ] && ok "$label（rc=$RC）" || bad "$label：期望 rc=0，实得 rc=$RC"
  else
    [ "$RC" -ne 0 ] && ok "$label（rc=$RC）" || bad "$label：期望 rc≠0，实得 rc=$RC"
  fi
}
expect_has()    { local n; n=$(count_in "$2" "$3"); [ "$n" -gt 0 ] && ok "$1" || bad "$1：$2 里找不到 \`$3\`"; }
expect_absent() { local n; n=$(count_in "$2" "$3"); [ "$n" -eq 0 ] && ok "$1" || bad "$1：$2 里出现了 \`$3\`（$n 次），本不该有"; }
expect_count()  { local n; n=$(count_in "$2" "$3"); [ "$n" -eq "$4" ] && ok "$1（$n 次）" || bad "$1：\`$3\` 命中 $n 次，期望 $4 次"; }

DRY=(./gradlew --offline --no-daemon :app:assembleArm64Release --dry-run)
DRY_DEBUG=(./gradlew --offline --no-daemon :app:assembleArm64Debug --dry-run)
FACTS=(./gradlew --offline --no-daemon :app:printPolarisReleaseFacts)

# 三条“指名到点”的针。它们是失败信息的原文片段，不是变量名 —— 守的是「说了什么」。
MSG_REFUSE="Android release 签名凭据缺失"
MSG_HATCH="已生效，本次 release 产物"
MSG_RULES="release 的 R8 规则文件缺席"
MSG_EXTRA="release 的 R8 规则集里有没登记的规则文件"
MSG_STORE="storeFile 不是本工程认的那一份"

# 事实行的前缀（含制表符）：`POLARIS_FACT<TAB>proguardFile<TAB><路径><TAB><字节数>`。
FACT_FILE_PREFIX=$'POLARIS_FACT\tproguardFile\t'

echo "══ ① 零凭据 + 不带逃生门 ⇒ 必须硬失败并点名 ══"
gradle_run "$WORK/1-bare.log" "${DRY[@]}"
expect_rc "零凭据跑 release 被拒绝" nonzero
expect_has "失败信息点名了「拒绝静默产出未签名包」" "$WORK/1-bare.log" "$MSG_REFUSE"
for v in POLARIS_KEYSTORE_PASSWORD POLARIS_KEY_PASSWORD POLARIS_KEY_ALIAS; do
  expect_has "失败信息逐条点名 $v" "$WORK/1-bare.log" "$v"
done
# 反向对照：这一轮不该出现逃生门的警告 —— 证明上面那几条不是被一段「什么都印」的输出喂绿的。
expect_absent "本轮没有逃生门警告（反向对照）" "$WORK/1-bare.log" "$MSG_HATCH"

echo "══ ② 命令行逃生门 ⇒ 放行、出警告、R8 仍在任务图里 ══"
gradle_run "$WORK/2-hatch.log" "${DRY[@]}" -PpolarisAllowUnsigned=true
expect_rc "带 -PpolarisAllowUnsigned=true 放行" 0
expect_has "逃生门打印了未签名警告" "$WORK/2-hatch.log" "$MSG_HATCH"
expect_absent "放行时不再报「凭据缺失」（反向对照）" "$WORK/2-hatch.log" "$MSG_REFUSE"

echo "══ ⑤ release 任务图里 R8 恰好跑一次 ══"
expect_count "minifyArm64ReleaseWithR8 在任务图里" "$WORK/2-hatch.log" "minifyArm64ReleaseWithR8" 1
expect_has "任务图确实是 release 那条（取材自检）" "$WORK/2-hatch.log" ":app:assembleArm64Release"

echo "══ ⑤b ARMv7 与 universal 同走完整 release/R8 任务图 ══"
gradle_run "$WORK/5b-arm-flavors.log" ./gradlew --offline --no-daemon \
  :app:assembleArmRelease :app:assembleUniversalRelease --dry-run \
  -PpolarisAllowUnsigned=true -PtargetList=aarch64,armv7 -ParchList=arm64,arm -PabiList=arm64-v8a,armeabi-v7a
expect_rc "ARMv7/universal release 任务图可解析" 0
expect_count "ARMv7 的 R8 在图中恰好一次" "$WORK/5b-arm-flavors.log" "minifyArmReleaseWithR8" 1
expect_count "universal 的 R8 在图中恰好一次" "$WORK/5b-arm-flavors.log" "minifyUniversalReleaseWithR8" 1

echo "══ ②b 命令行来源的正向对照：这条路确实把开关送到了逃生门手里 ══"
gradle_run "$WORK/2b-facts.log" "${FACTS[@]}" -PpolarisAllowUnsigned=true
expect_rc "事实任务跑得动" 0
expect_has "命令行属性表里有它" "$WORK/2b-facts.log" "hatchCmdlineKeys	polarisAllowUnsigned"
expect_has "逃生门确实被打开" "$WORK/2b-facts.log" "hatchOpen	true"

echo "══ ③ gradle.properties 写一行 ⇒ 仍然必须硬失败 ══"
printf '\npolarisAllowUnsigned=true\n' >> "$PROPS"
gradle_run "$WORK/3-props.log" "${DRY[@]}"
expect_rc "gradle.properties 打不开逃生门" nonzero
expect_has "仍然报「凭据缺失」" "$WORK/3-props.log" "$MSG_REFUSE"
expect_absent "没有出逃生门警告" "$WORK/3-props.log" "$MSG_HATCH"
# 正向对照：证明这一行**确实被 gradle 读到了**（否则 rc≠0 只是「什么都没发生」）。
gradle_run "$WORK/3b-facts.log" "${FACTS[@]}"
expect_has "gradle 确实读到了这一行（正向对照）" "$WORK/3b-facts.log" "hatchAllPropertyKeys	polarisAllowUnsigned"
expect_has "但它没进命令行属性表" "$WORK/3b-facts.log" "hatchCmdlineKeys	<none>"
expect_has "逃生门保持关闭" "$WORK/3b-facts.log" "hatchOpen	false"
cp -- "$PROPS_BAK" "$PROPS"

echo "══ ④ ORG_GRADLE_PROJECT_ 环境变量 ⇒ 仍然必须硬失败 ══"
( cd "$GRADLE_DIR" && env -u POLARIS_KEYSTORE_FILE -u POLARIS_KEYSTORE_PASSWORD \
    -u POLARIS_KEY_PASSWORD -u POLARIS_KEY_ALIAS -u POLARIS_ALLOW_UNSIGNED \
    ORG_GRADLE_PROJECT_polarisAllowUnsigned=true "${DRY[@]}" ) >"$WORK/4-env.log" 2>&1
RC=$?
expect_rc "ORG_GRADLE_PROJECT_ 打不开逃生门" nonzero
expect_has "仍然报「凭据缺失」" "$WORK/4-env.log" "$MSG_REFUSE"
expect_absent "没有出逃生门警告" "$WORK/4-env.log" "$MSG_HATCH"
( cd "$GRADLE_DIR" && env -u POLARIS_KEYSTORE_FILE -u POLARIS_KEYSTORE_PASSWORD \
    -u POLARIS_KEY_PASSWORD -u POLARIS_KEY_ALIAS -u POLARIS_ALLOW_UNSIGNED \
    ORG_GRADLE_PROJECT_polarisAllowUnsigned=true "${FACTS[@]}" ) >"$WORK/4b-facts.log" 2>&1
RC=$?
expect_rc "事实任务跑得动" 0
expect_has "gradle 确实收到了这个属性（正向对照）" "$WORK/4b-facts.log" "hatchAllPropertyKeys	polarisAllowUnsigned"
expect_has "逃生门保持关闭" "$WORK/4b-facts.log" "hatchOpen	false"

echo "══ ④b 别的环境变量也不是逃生门的来源 ══"
# 验收员的 M01 变异就是给读取点接一句 `|| System.getenv("POLARIS_ALLOW_UNSIGNED") == "true"`。
# 这条断言只覆盖**这一个名字**。上一版这里写着「新变量名由源码侧那条形状断言守，两条一起
# 就封住了这一形」——**那句话过宽**（2026-09-06 终止轮 R3）：源码侧钉死的是
# `val allowUnsignedRelease` **那一条绑定**的整条右值，管不到守卫**体内**别处新写的读取。
# 两侧各守一头，中间那条缝（在逃生门分支之前插一句
# `if (System.getenv("POLARIS_XXX") == "true") return@Action`）**今天没有门**，
# 实测两侧同时全绿。登记在头注天花板表的「看不见 ⑤」，本处不复写。
( cd "$GRADLE_DIR" && env -u POLARIS_KEYSTORE_FILE -u POLARIS_KEYSTORE_PASSWORD \
    -u POLARIS_KEY_PASSWORD -u POLARIS_KEY_ALIAS \
    POLARIS_ALLOW_UNSIGNED=true "${DRY[@]}" ) >"$WORK/4c-env.log" 2>&1
RC=$?
expect_rc "POLARIS_ALLOW_UNSIGNED 环境变量打不开逃生门" nonzero
expect_has "仍然报「凭据缺失」" "$WORK/4c-env.log" "$MSG_REFUSE"

echo "══ ⑥ debug 侧不受影响：跑得通，且任务图里没有 R8 ══"
gradle_run "$WORK/6-debug.log" "${DRY_DEBUG[@]}"
expect_rc "debug 侧照常放行" 0
expect_has "任务图确实是 debug 那条（取材自检）" "$WORK/6-debug.log" ":app:assembleArm64Debug"
expect_count "debug 任务图里没有 R8" "$WORK/6-debug.log" "WithR8" 0
expect_absent "debug 侧不该被签名守卫拦" "$WORK/6-debug.log" "$MSG_REFUSE"

echo "══ ⑦ R8 实际吃进的规则集：集合恰等（多一份少一份都红）、且逐份非空 ══"
gradle_run "$WORK/7-facts.log" "${FACTS[@]}"
expect_rc "事实任务跑得动" 0
expect_has "逃生门默认关闭" "$WORK/7-facts.log" "hatchOpen	false"
expect_has "命令行属性表为空（默认态自检）" "$WORK/7-facts.log" "hatchCmdlineKeys	<none>"
expect_has "release 资源裁剪已开启" "$WORK/7-facts.log" $'POLARIS_FACT\treleaseShrinkResources\ttrue'
for variant in arm64Release armRelease universalRelease; do
  expect_has "$variant 排除 x86/x86_64 原生库" "$WORK/7-facts.log" \
    $'POLARIS_FACT\treleaseJniExcludes\t'"$variant"$'\t**/x86/*.so,**/x86_64/*.so'
done


# 把 AGP 手里那份清单原样取下来（每行一个绝对路径），下面按**集合**判，不按份数判。
#
# 上一版判的是 `listed >= 4`。那是 membership 不是闭合：往 app/ 放一份没人登记的
# `extra.pro`（内容 `-keep class ** { *; }` = 把整包混淆关掉），listed 变成 5，
# 「≥ 4」照样绿。规则集是集合，多一份与少一份是同一类事故。
command grep -F -- "$FACT_FILE_PREFIX" "$WORK/7-facts.log" >"$WORK/7-listed.txt"
listed=$(command grep -c '' "$WORK/7-listed.txt")
[ "$listed" -gt 0 ] || die "事实任务一行 proguardFile 都没打出来 —— 取材面是空的，⑦ 全部断言作废。"
cut -f3 <"$WORK/7-listed.txt" | sort >"$WORK/7-actual-paths.txt"
{
  for rel in \
    app/proguard-rules.pro \
    app/proguard-tauri.pro \
    app/src/main/java/com/polaris2/app/generated/proguard-wry.pro
  do
    printf '%s\n' "$GRADLE_DIR/$rel"
  done
} | sort >"$WORK/7-expected-paths.txt"
# AGP 默认档的路径带着 AGP 版本号（`…/proguard-android-optimize.txt-8.11.0`），
# 不能写死；把它从实得集合里摘出来单独判，剩下的必须与登记的三份**逐字相等**。
default_hits=$(command grep -c -F -- "proguard-android-optimize.txt" "$WORK/7-actual-paths.txt")
[ -n "$default_hits" ] || default_hits=0
command grep -v -F -- "proguard-android-optimize.txt" "$WORK/7-actual-paths.txt" >"$WORK/7-actual-own.txt"
if [ "$default_hits" -eq 1 ]; then
  ok "AGP 默认档在清单里，且恰好一份"
else
  bad "AGP 默认档在清单里出现 $default_hits 次（应恰好 1 次）—— 0 次 = native 方法、@JavascriptInterface、enum values/valueOf 这批全局兜底规则整体缺席"
fi
if diff -u "$WORK/7-expected-paths.txt" "$WORK/7-actual-own.txt" >"$WORK/7-setdiff.txt" 2>&1; then
  ok "AGP 手里的本仓规则文件集合与登记的三份**恰等**"
else
  bad "AGP 手里的本仓规则文件集合与登记的三份不等（- 期望 / + 实得）：$(cat "$WORK/7-setdiff.txt")"
fi
for rel in \
  app/proguard-rules.pro \
  app/proguard-tauri.pro \
  app/src/main/java/com/polaris2/app/generated/proguard-wry.pro
do
  line=$(command grep -F -- "$FACT_FILE_PREFIX$GRADLE_DIR/$rel	" "$WORK/7-facts.log")
  if [ -z "$line" ]; then
    bad "⑦ $rel 不在 AGP 装配好的 proguardFiles 清单里 —— 这份规则整体不生效，而构建照常绿"
    continue
  fi
  size=${line##*	}
  if [ "$size" -gt 0 ] 2>/dev/null; then
    ok "⑦ $rel 进了 R8（$size 字节）"
  else
    bad "⑦ $rel 在清单里但是空文件（$size 字节）—— 规则文本一条都没有"
  fi
done

echo "══ ⑦b R8 输入集的另外两处来源必须为空（defaultConfig / productFlavor）══"
#
# ⑦ 的「集合恰等」判的是 release **构建类型**那一份清单，而 AGP 是把
# `defaultConfig` + 选中的 `productFlavor` + `buildType` 三处**合并**后喂给 R8 的
# （AGP 口径；本轮没去 R8 的输入那一层实测，那要真跑一次 release 构建）。
# 实测（2026-09-06 终止轮 R2）：往 `defaultConfig` 上挂一份 `../__probe__.pro`
# —— 它在 app/ 的 fileTree 扫描面之外，守卫那条「多一份没登记的规则文件」也看不见它 ——
# 本裁判 ⑦ 零失败、rc=0，事实表里连它的影子都没有。取材面只有三分之一，恰等就名不副实。
#
# 今天把另外两处也摊开，并钉死为空：两处为空 ⇒ buildType 那一份**就是**本仓自有规则的全集，
# ⑦ 的恰等才等于「R8 到手的就是登记的那三份」。
# 🔴 这两条是**空集断言**，不是白名单：真要给 defaultConfig / flavor 挂规则，
#    这条会红，那时该做的是把登记面搬过去（连同 ⑦ 的期望集），不是把这条改宽。
expect_has "defaultConfig 上没挂 proguardFiles" "$WORK/7-facts.log" \
  $'POLARIS_FACT\tproguardFilesDefaultConfig\t<none>'
expect_has "五个 productFlavor 上都没挂 proguardFiles" "$WORK/7-facts.log" \
  $'POLARIS_FACT\tproguardFilesFlavors\t<none>'

echo "══ ⑧ 少一份规则文件 ⇒ 必须点名那一份并硬失败 ══"
rm -f -- "$SACRIFICIAL_RULE"
gradle_run "$WORK/8-missing.log" "${DRY[@]}" -PpolarisAllowUnsigned=true
expect_rc "缺规则文件时拒绝构建" nonzero
expect_has "失败信息说的是「规则文件缺席」" "$WORK/8-missing.log" "$MSG_RULES"
expect_has "并点名缺的是哪一份" "$WORK/8-missing.log" "proguard-tauri.pro"
# 反向对照：这一轮带着逃生门，所以红的原因必须是规则文件，不是签名。
expect_absent "红的原因不是签名（反向对照）" "$WORK/8-missing.log" "$MSG_REFUSE"
cp -- "$RULE_BAK" "$SACRIFICIAL_RULE"

echo "══ ⑨ 多一份没登记的 .pro ⇒ 事实表必须跟着变，且构建必须点名硬失败 ══"
#
# 这一节同时是两条判据：
#   · **闭合**（收官轮 A9）：`-keep class ** { *; }` 一行就把整包混淆关掉，而「三份都在」
#     这条 membership 判定对它完全是瞎的。集合恰等在 ⑦，硬失败在这里。
#   · **事实任务读的是不是 AGP 那份清单**（收官轮 A2）：事实任务已经搬出被审判的
#     `build.gradle.kts`，但「它读的到底是 AGP 装配好的清单，还是一份写死的常量」
#     不能靠读代码来答。差分实验答：AGP 的扫描面变了，它打出来的清单必须跟着变。
#     打不出这一行 = 它读的是别的东西，⑦ 的全部结论随之作废。
printf -- '-keep class ** { *; }\n' >"$PROBE_RULE"
gradle_run "$WORK/9-facts.log" "${FACTS[@]}"
expect_rc "多一份规则文件时事实任务仍跑得动" 0
expect_has "事实表跟着 AGP 的扫描面变了（证明它读的就是 AGP 装配好的清单，不是写死的常量）" \
  "$WORK/9-facts.log" "$FACT_FILE_PREFIX$PROBE_RULE	"
gradle_run "$WORK/9-dry.log" "${DRY[@]}" -PpolarisAllowUnsigned=true
expect_rc "多一份没登记的规则文件时拒绝构建" nonzero
expect_has "失败信息说的是「有没登记的规则文件」" "$WORK/9-dry.log" "$MSG_EXTRA"
expect_has "并点名多出来的是哪一份" "$WORK/9-dry.log" "__gate_probe__.pro"
expect_absent "红的原因不是签名（反向对照）" "$WORK/9-dry.log" "$MSG_REFUSE"
expect_absent "红的原因也不是缺规则文件（反向对照）" "$WORK/9-dry.log" "$MSG_RULES"
rm -f -- "$PROBE_RULE"

echo "══ ⑩ 凭据在场那条分支：storeFile 必须存在、可读、且就是本工程认的那一份 ══"
#
# 这条分支此前**零观测**：裁判每一轮都跑在零凭据下，`storeFile` 指向哪儿没有任何东西看一眼。
# 最坏的一形不是指向空气（AGP 执行期还会报一句 `Keystore file not set`），而是指向**另一份
# 存在的**密钥库：构建全绿、包也签得出来，只是签它的不是本工程的密钥。
#
# 🔴 这里用的不是密钥：`$FAKE_KEYSTORE` 是现造的一个普通文件，口令/别名是三个写死的占位串。
#    配置期从不读密钥库的内容 —— 它只被 `isFile` / `canRead` / 路径比对碰到。
#    「这份密钥库是不是合法、口令对不对」不在本裁判射程内（见头注天花板表第 ④ 条）。
printf 'not a keystore; created by gate-android-release-behavior.sh as a path probe\n' >"$FAKE_KEYSTORE"
gradle_run_signed "$WORK/10-facts.log" "$FAKE_KEYSTORE" "${FACTS[@]}"
expect_rc "凭据齐全时事实任务跑得动" 0
expect_has "AGP 手里的 storeFile 就是 POLARIS_KEYSTORE_FILE 指的那一份，且存在、可读" \
  "$WORK/10-facts.log" $'POLARIS_FACT\tsigningStoreFile\t'"$FAKE_KEYSTORE"$'\ttrue\ttrue'
gradle_run_signed "$WORK/10-dry.log" "$FAKE_KEYSTORE" "${DRY[@]}"
expect_rc "凭据齐全时 release 放行" 0
expect_absent "凭据齐全时不再报「凭据缺失」" "$WORK/10-dry.log" "$MSG_REFUSE"
expect_absent "凭据齐全时不该走逃生门（反向对照：这一轮的绿不是逃生门给的）" "$WORK/10-dry.log" "$MSG_HATCH"
expect_absent "storeFile 与本工程认的那一份一致（没报错位）" "$WORK/10-dry.log" "$MSG_STORE"

echo "══ ⑩b 反向对照：密钥库不在盘上时，凭据齐不齐都必须硬失败并点名那条路径 ══"
# 没有这条，⑩ 的「存在且可读」可能是被一条恒真的判定喂绿的。
MISSING_KEYSTORE="$WORK/there-is-no-such-keystore.jks"
[ -e "$MISSING_KEYSTORE" ] && die "反向对照的前提被破坏：$MISSING_KEYSTORE 居然在场。"
gradle_run_signed "$WORK/10c-dry.log" "$MISSING_KEYSTORE" "${DRY[@]}"
expect_rc "密钥库不在盘上时拒绝构建" nonzero
expect_has "失败信息点名了「密钥库文件不存在」" "$WORK/10c-dry.log" "密钥库文件不存在"
expect_has "并点名是哪一条路径" "$WORK/10c-dry.log" "$MISSING_KEYSTORE"
gradle_run_signed "$WORK/10c-facts.log" "$MISSING_KEYSTORE" "${FACTS[@]}"
expect_rc "事实任务仍跑得动" 0
expect_has "此时 AGP 手里的 storeFile 是空的（空壳配置，正向对照）" \
  "$WORK/10c-facts.log" $'POLARIS_FACT\tsigningStoreFile\t<none>\tfalse\tfalse'

echo "══ 还原自检 ══"
if cmp -s -- "$PROPS_BAK" "$PROPS"; then ok "gradle.properties 已还原"; else bad "gradle.properties 没还原干净"; fi
if cmp -s -- "$RULE_BAK" "$SACRIFICIAL_RULE"; then ok "proguard-tauri.pro 已还原"; else bad "proguard-tauri.pro 没还原干净"; fi
if [ -e "$PROBE_RULE" ]; then bad "⑨ 的探针文件还留在树上：$PROBE_RULE"; else ok "⑨ 的探针文件已清掉"; fi

finish
