//! 🔴 **随包核里那些「我们靠它的默认值」的依赖，版本一动就红。**
//!
//! # 为什么需要这一道（本门的第一个也是唯一的现存消费点：`sing-tun` 的 DNS 模式默认值）
//!
//! 本仓的 DNS 截获**完全建立在一个上游默认值上**，而那个默认值我们从不写进配置：
//!
//! - `route.rules` 里一条 `port:[53] + action:"hijack-dns"`（`builder/route.rs`），
//!   配一个哨兵 IP `CONTROLLED_TUN_DNS_IP = 8.8.8.8`（`user_config/dns_constants.rs`，
//!   **故意排除出 `BOOTSTRAP_DIRECT_DNS_IPS`**，否则被直连放行就逃逸了劫持）。
//! - tun inbound 里**不发** `dns_mode` ⇒ 用的是 sing-tun 的默认值。
//!   定版源码（`sing-tun@v0.9.0-beta.4` `tun.go:127-132`）：
//!   `func (o *Options) DNSModeOrDefault() string { if o.DNSMode == "" { return DNSModeHijack } … }`
//!
//! 即：**默认是 `hijack`，我们整套 DNS 分流就靠它**。上游哪天把默认改成 `native`，
//! 生成的配置一个字节都不变、`sing-box check` 照样 rc=0、全仓单测照样全绿，而用户侧的表现是
//! **能上网但分流失效**（DNS 不再进规则引擎 ⇒ FakeIP / 国内外分流 / dns-race 全部旁路）。
//! 这正是本仓反复吃亏的那类「看起来正常」的失败。
//!
//! # 为什么是「钉依赖版本」而不是「把 `dns_mode: hijack` 写进配置」
//!
//! 写进配置看着更直接，但代价在金样：`fixtures/config-snapshot.json` 是 **上游 4.2.6 冻结的
//! parity 基线**（`golden_config_snapshot.rs` 开头明文「只重生、不手改」），而 上游侧不发这个键。
//! 加一个 Polaris-only 的键 ⇒ 每个 TUN 场景当场 delta，且**永远无法靠重生对齐**（重生一次回来一次）。
//! 为一个假想风险换掉一道真门的可信度，不划算。
//!
//! 于是判据挪到**依赖指纹**上：核一 bump，`sing-tun` 版本串必变 ⇒ 本门红 ⇒ 人被迫回去看一眼
//! `DNSModeOrDefault` 的默认值还是不是 `hijack`，确认后才更新下面的常量。
//! **判据独立于被检查的那个值** —— 这是 `28f5d46` 修 cronet go.mod 对拍时立下的同一条：
//! 同一类坏味道曾出现在 Cronet：把待发现的依赖版本再写进 manifest 当新鲜度判据。现在 Cronet 版本
//! 只从随包 sing-box tag 的 `go.mod` 解析，manifest 只保留库本体 SHA-256 pin。
//!
//! # 射程（自曝，别把绿读大）
//!
//! - 本门**不**验默认值本身是什么 —— 二进制里读不到（Go 符号表被剥，`go tool nm` 报
//!   `no symbol section`）。它只保证「依赖没换过」，换了就把人拦下来。
//! - 覆盖面 = **盘上 present 的每一份核**（linux / win / mac-arm64 / mac-x64），逐份比对 modinfo
//!   版本串，失败信息点明是哪个平台那一格。平台名单不写在本文件里，从
//!   `support/core_locator.rs` 的 `CORE_MATRIX` 派生 —— 与 `core_build_matrix` 同一个枚举。
//!   🔴 2026-09-16 前本门只读**当前构建目标那一份**（走 `core_or_skip`），而文档里写的是
//!   「四份一致」：那句「一致」是历次升核时人工核的，不是门在查。实测过这道缝 ——
//!   只重拉了 linux 核就改 `SING_TUN_PINNED`，盘上另外三份仍是旧版旧 pin，本门照样绿。
//! - 盘上缺哪个平台就少看哪个平台（打 warn，`eprintln!` 同样归 libtest 捕获）；一份都没有时跳过，
//!   `POLARIS_REQUIRE_KERNEL_GATE=1` 时缺一即红（与 `core_build_matrix` 共用
//!   `require_all_present`，打包腿强制生效，`ci_step_still_wired` 守着）。
//! - 🔴 **「跳过」是静默的**：下面那句 `eprintln!` 归 libtest 捕获，**只在测试失败时才回放**。
//!   实测本门首跑的 CI ubuntu 腿（不拉核）日志里只有 `bundled_core_still_uses_the_pinned_sing_tun
//!   ... ok`，提示语零命中 ⇒ **那条绿只说明「编得过 + 提取器单测过」，没有比对过任何版本串**。
//!   真正的比对只发生在打包腿（核已 fetch + 硬化开关）。本地要看见提示语得显式 `-- --nocapture`。

#[path = "support/core_locator.rs"]
mod core_locator;

use core_locator::{present_cores, repo_root, require_all_present};

/// 缺核信息里用来说明「是哪道门没跑全」的名字。
const GATE_NAME: &str = "依赖指纹门未完整执行";

/// 随包核当前使用的 `sing-tun` 版本。
///
/// 🔴 **改这个常量之前，先做这件事**：去读该版本 `tun.go` 的 `DNSModeOrDefault()`，
/// 确认 `o.DNSMode == ""` 时返回的仍是 `DNSModeHijack`。取源码的路径（符号表被剥时仍可用）：
///
/// ```text
/// go version -m <随包核>                     # 读出 sing-tun 的确切版本（伪版本带 commit）
/// curl -sfL https://proxy.golang.org/github.com/sagernet/sing-tun/@v/<版本>.zip
/// ```
///
/// 若默认值变了 —— **不要**只更新这里就放行：本仓整套 DNS 分流依赖它，得先决定是显式下发
/// `dns_mode` 还是改机制，详见 vault `polaris-singbox-1.14-adoption-matrix-2026-07-30` 的 I3 判定块。
// 2026-08-29 随 1.14.0-rc.2 复核。RC2 的 go.mod 仍钉同一版 sing-tun；放行前已按本常量
// 文档的指引核对：
// https://raw.githubusercontent.com/SagerNet/sing-tun/v0.9.0-beta.2/tun.go 的
// `DNSModeOrDefault()`（tun.go:127-132）仍是 `if o.DNSMode == "" { return DNSModeHijack }` ——
// 前提未变，故只更新 pin，不改机制。
//
// 2026-08-31 随 1.14.0 正式版复核。随包核的 sing-tun 从 `v0.9.0-beta.2` 跳到 `v0.9.0-beta.4`
// （盘上四份二进制 linux / win / mac-arm64 / mac-x64 的 modinfo 版本串一致）。复核结论：
// **两个 tag 下的 `tun.go` 是同一个 git blob**（`e7701222182f80ae0d3e66cb976ad7f72de93d81`），
// 逐字无差异 ⇒ `DNSModeOrDefault()`（tun.go:127-132）仍是
// `if o.DNSMode == "" { return DNSModeHijack }`，枚举 `DNSModeHijack = "hijack"`（tun.go:63-67）
// 亦未变。**上面模块文档里引用的那段 beta.2 函数体，逐字等同于 beta.4 的**，故未重抄。
// 另做了全仓 blob 对差兜底（防「默认值没动但别处把它绕过去了」）：beta.2 → beta.4 整个仓
// 只有 `stack_system.go` / `stack_system_nat.go` / `tun_linux.go` 三个文件变了，其中仅
// `tun_linux.go` 提到 DNS 模式，两处 `t.options.DNSMode != DNSModeDisabled` 逐字相同（仅行号 +2）。
// 取证命令（只读，不需要 Go 工具链，比本常量文档里的 `go version -m` + `curl` 那条更省事）：
//   gh api repos/SagerNet/sing-tun/git/trees/<tag> --jq '.tree[]|select(.path=="tun.go").sha'
//   gh api -H 'Accept: application/vnd.github.raw' 'repos/SagerNet/sing-tun/contents/tun.go?ref=<tag>'
//   gh api 'repos/SagerNet/sing-tun/git/trees/<tag>?recursive=1' \
//     --jq '.tree[]|select(.type=="blob")|"\(.sha) \(.path)"'
// 前提未变，故只更新 pin，不改机制。
//
// 2026-09-08 随随包核 1.14.0 → **1.15.0-alpha.2** 复核。sing-tun 从 `v0.9.0-beta.4` 跳到
// `v0.9.1-0.20260902150540-98e457e39c90`（1.15.0-alpha.2 的 go.mod:58）。逐条按本常量文档的指引核对：
//   ① 直读新版函数体：`tun.go:128-133` 仍是 `if o.DNSMode == "" { return DNSModeHijack }`，
//      枚举 `DNSModeHijack = "hijack"`（tun.go:64-66）未变；
//   ② 全仓对差兜底（防「默认值没动但别处把它绕过去了」）：beta.4 → 该 commit 共 21 个文件变动，
//      其中 `tun.go` 只改了 **1 行**且与 DNS/路由模式无关；提到 `DNSMode` 的新增行仅
//      `redirect_iptables.go` 一处 `dnsHijack := options.DNSModeOrDefault() == DNSModeHijack`
//      —— 那是 Linux redirect/auto_redirect 路径（本仓不开启），是**读取**该默认而非改动它。
//   ③ 顺带确认 `StrictRoute` 的消费面仍只有 `tun.go`(声明) / `tun_linux.go` / `tun_windows.go` /
//      `redirect_iptables.go` / `redirect_nftables_rules.go`，**`tun_darwin.go` 一次都没有**
//      ⇒ macOS 上 `strict_route` 仍是 no-op（这条是 UI 侧「mac 禁用该开关」的判据来源）。
// 前提未变，故只更新 pin，不改机制。
//
// 2026-09-13 随随包核 1.15.0-alpha.2 → **1.15.0-alpha.3** 复核。sing-tun 从
// `v0.9.1-0.20260902150540-98e457e39c90` 跳到 `v0.9.4-0.20260912075549-869f0a4d76af`
// （alpha.3 的 go.mod:58；盘上四份二进制 linux / win / mac-arm64 / mac-x64 的 modinfo 版本串一致）。
// 取两版 proxy.golang.org 模块 zip 逐条核对：
//   ① 直读新版函数体：`tun.go:133-138` 仍是 `if o.DNSMode == "" { return DNSModeHijack }`，
//      枚举 `DNSModeHijack = "hijack"`（tun.go:64-66）未变；`tun.go` 本身只多了 `MultiQueue` 字段
//      与两段 darwin 注释改写，与 DNS/路由模式无关；
//   ② 全仓对差兜底：31 个文件改动 + 39 个新增条目（主体是新 go 栈 `stack_go*.go` 及其测试）、0 删除。
//      抹掉行号后，非测试 `.go` 里提到 `DNSMode` 的**行集合两版逐字相同**（tun.go / tun_linux.go /
//      tun_windows.go / redirect_iptables.go / redirect_nftables_rules.go /
//      redirect_route_bypass_android.go）；新增的 `stack_go*.go` 零引用 ⇒ 默认值与消费面都没被绕开。
//      这也说明 DNS 模式与「用哪个栈」无关 —— 本轮 Polaris 不再下发 `stack`（改走上游新 go 栈）不改变本门前提；
//   ③ `StrictRoute` 的消费面仍是 `tun.go`(声明) / `tun_linux.go` / `tun_windows.go` /
//      `redirect_iptables.go` / `redirect_nftables_rules.go`，`tun_darwin.go` 仍零引用。
// 前提未变，故只更新 pin，不改机制。
//
// 2026-09-16 随随包核 1.15.0-alpha.3 → **1.15.0-alpha.4** 复核。sing-tun 从
// `v0.9.4-0.20260912075549-869f0a4d76af` 跳到 `v0.9.4-0.20260914145202-3a0d3878577a`
// （alpha.4 的 go.mod:58）。逐条按本常量文档的指引核对：
//   ① 默认值直读：`tun.go:133-138` 的 `DNSModeOrDefault()` 仍是 `if o.DNSMode == "" { return DNSModeHijack }`
//      （`gh api -H 'Accept: application/vnd.github.raw' 'repos/SagerNet/sing-tun/contents/tun.go?ref=3a0d3878577a'`）；
//   ② 全仓对差兜底：**1 个 commit、40 个文件**，改动面全部在新 go 栈（`stack_go*.go`，主体是新增
//      `stack_go_tcp_{ack,bbr,cong,cubic,rate}.go` 共约 2900 行拥塞控制）与 Windows 内部实现
//      （`internal/afd/*`、`internal/wintun/session_windows.go`、`tun_windows.go`）加
//      `gtcpip/header/checksum.go`、`stack.go`。**`tun.go` 不在改动清单里** ⇒ DNSMode 的声明与默认值
//      一行没动；`tun_windows.go` 的 patch 里 grep `DNSMode|StrictRoute` **零命中** ⇒ 两个符号的消费面
//      在本次唯一被改到的平台文件里也没变；
//   ③ `StrictRoute` 的消费面（`tun.go` 声明 / `tun_linux.go` / `redirect_iptables.go` /
//      `redirect_nftables_rules.go`）本轮全部未被改动，`tun_darwin.go` 仍零引用。
// 前提未变，故只更新 pin，不改机制。
//
// 2026-09-24 随随包核 1.15.0-alpha.4 → **1.15.0-alpha.7** 复核。sing-tun 从
// `v0.9.4-0.20260914145202-3a0d3878577a` 跳到 `v0.9.6-0.20260922105247-aff4131a9e9e`
// （alpha.7 的 go.mod:57；盘上四份二进制 linux / win / mac-arm64 / mac-x64 的 modinfo 版本串一致）。
// 🔴 本次 `compare/3a0d3878577a...aff4131a9e9e` 的 status 是 **diverged**（ahead 19 / behind 8，
// 上游 rebase 过），三点 diff 报 117 个文件含 `tun.go` —— 那不是 A→B 的差集，故改取两个 commit 的
// 源码 tarball 直接对比内容：
//   ① 默认值直读：两版 `tun.go` **逐字节相同**；`tun.go:133-138` 的 `DNSModeOrDefault()` 仍是
//      `if o.DNSMode == "" { return DNSModeHijack }`，枚举 `DNSModeHijack = "hijack"`（tun.go:64-66）未变；
//   ② 消费面兜底：抹掉行号后，非测试 `.go` 里提到 `DNSMode` 的**行集合两版逐字相同**；
//      sing-box 侧 `protocol/tun/inbound.go:215` 仍是把 `options.DNSMode` 原样透传（空值不改写）；
//   ③ `StrictRoute` 的消费面（`tun.go` 声明 / `tun_linux.go` / `tun_windows.go` / `redirect_iptables.go` /
//      `redirect_nftables_rules.go`）行集合两版逐字相同，`tun_darwin.go` 仍零引用。
// 前提未变，故只更新 pin，不改机制。
const SING_TUN_PINNED: &str = "v0.9.6-0.20260922105247-aff4131a9e9e";

/// 被钉的依赖模块路径。
const SING_TUN_MODULE: &str = "github.com/sagernet/sing-tun";

/// 从 Go 二进制内嵌的 modinfo 里提某个依赖的版本串。
///
/// modinfo 是纯文本、制表符分隔、换行结尾，形如：
/// `dep\tgithub.com/sagernet/sing-tun\tv0.8.12-…\th1:LhMorA53…=\n`
///
/// 不走 `go version -m`：CI 的 Rust 腿**没有 Go 工具链**（`ci.yml` 开头明载），
/// 挂一个本机有、CI 没有的外部命令等于把本门在 CI 上变成静默跳过。
///
/// 前后都用 `\t` 定界 ⇒ 模块名是**精确匹配**，`…/sing-tun-extra` 这类前缀撞名不会命中。
fn extract_dep_version(bin: &[u8], module: &str) -> Option<String> {
    let needle = format!("dep\t{module}\t").into_bytes();
    let at = bin
        .windows(needle.len())
        .position(|w| w == needle.as_slice())?;
    let rest = &bin[at + needle.len()..];
    let end = rest.iter().position(|b| *b == b'\t' || *b == b'\n')?;
    std::str::from_utf8(&rest[..end]).ok().map(str::to_owned)
}

#[test]
fn bundled_core_still_uses_the_pinned_sing_tun() {
    let present = present_cores();
    let complete = require_all_present(&present, GATE_NAME);
    if present.is_empty() {
        eprintln!(
            "⚠ 跳过依赖指纹门：盘上一份随包核都没有（`.gitignore` 的 /resources/*）。\
             跑 `node scripts/fetch-core.mjs` 后本门自动生效；\
             打包腿带 POLARIS_REQUIRE_KERNEL_GATE=1 强制生效。"
        );
        return;
    }
    if !complete {
        eprintln!("⚠ 依赖指纹门只看到部分平台，未覆盖的平台本轮没有被检查。");
    }

    for (core, path) in &present {
        let bytes =
            std::fs::read(path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));

        // 提不出来 = 本门失效（Go 换了 modinfo 编码 / 核被 strip 得更狠 / 拿到的不是 Go 二进制）。
        // 这种情况必须**红**而不是跳过 —— 否则本门会从「守着依赖」静静退化成「永远绿」。
        let actual = extract_dep_version(&bytes, SING_TUN_MODULE).unwrap_or_else(|| {
            panic!(
                "在 {} 里找不到 `{SING_TUN_MODULE}` 的 modinfo 条目 —— \
                 本门已失效（不是「依赖没变」），先修门再谈结论",
                path.display()
            )
        });

        assert_eq!(
            actual, SING_TUN_PINNED,
            "\n随包核 **{}**（{}）的 sing-tun 版本变了：{SING_TUN_PINNED} → {actual}\n\
             ⚠ 只有这一格不符时，其余平台那几份仍是 pin 住的版本 —— 那更可能是\
             「升核只重拉了部分平台」，而不是上游换了依赖：先看 `node scripts/fetch-core.mjs`\
             是不是漏了 `--platform`（不传该 flag = 全平台）。\n\
             本仓的 DNS 截获依赖 sing-tun 的 `DNSModeOrDefault()` 默认值为 `hijack`\
             （tun inbound 刻意不下发 `dns_mode`）。默认值一变，生成的配置一字节不动、\
             `sing-box check` 照样 rc=0，而用户侧表现是「能上网但分流失效」。\n\
             ⇒ 先读新版 `tun.go` 的 `DNSModeOrDefault()` 确认默认仍是 `DNSModeHijack`，\
             再更新 core_dep_fingerprint.rs 的 SING_TUN_PINNED。取源码路径见该常量的文档注释。\n",
            core.key, core.rel
        );
    }
}

/// 正向对照：证明提取器真的会「提不到」和「提错不了」，否则上面那条断言可能只是碰巧绿。
#[test]
fn the_extractor_has_teeth() {
    let ok = b"...\ndep\tgithub.com/sagernet/sing-tun\tv1.2.3\th1:AAAA=\ndep\tother\tv9\n";
    assert_eq!(
        extract_dep_version(ok, SING_TUN_MODULE).as_deref(),
        Some("v1.2.3")
    );

    // 依赖不在 → None（而不是返回旁边那条的版本）
    let absent = b"...\ndep\tgithub.com/sagernet/sing-box\tv1.14.0\th1:BBBB=\n";
    assert_eq!(extract_dep_version(absent, SING_TUN_MODULE), None);

    // 前缀撞名不得命中：`sing-tun-extra` 与 `sing-tun` 只差后缀，靠尾部 `\t` 定界分开
    let confusable = b"...\ndep\tgithub.com/sagernet/sing-tun-extra\tv7.7.7\th1:CCCC=\n";
    assert_eq!(extract_dep_version(confusable, SING_TUN_MODULE), None);

    // 截断（版本串后既无 \t 也无 \n）→ None，不返回半截版本
    let truncated = b"dep\tgithub.com/sagernet/sing-tun\tv1.2.3";
    assert_eq!(extract_dep_version(truncated, SING_TUN_MODULE), None);
}

/// 打包腿上本门必须真的在跑 —— 否则「缺核强制红」那条开关形同虚设。
#[test]
fn ci_step_still_wired() {
    let wf = repo_root().join(".github/workflows/package.yml");
    let raw =
        std::fs::read_to_string(&wf).unwrap_or_else(|e| panic!("读不到 {}: {e}", wf.display()));
    // 匹配 **run 命令本身**而不是裸词：本文件在 package.yml 里还留了一段说明注释，
    // 注释里就写着 `core_dep_fingerprint`。用裸词做判据的话，把整个 step 删掉、注释留下，
    // 本断言照样绿 —— 实测过，这正是「绿没有信息量」的一个新鲜样本。
    assert!(
        raw.contains("--test core_dep_fingerprint"),
        "package.yml 里找不到 `--test core_dep_fingerprint` —— 打包腿没在跑依赖指纹门，\
         缺核时它会静静跳过而没人知道"
    );
}
