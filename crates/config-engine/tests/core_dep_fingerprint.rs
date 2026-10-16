//! 🔴 **随包核里那些「我们靠它的默认值」的依赖，版本一动就红。**
//!
//! # 为什么需要这一道（本门的第一个也是唯一的现存消费点：`sing-tun` 的 DNS 模式默认值）
//!
//! 当前契约（2026-10-10，候选适配；发布门仍关闭）：
//! - route 的 port53/hijack-dns、哨兵与 bootstrap 直连顺序不变。
//! - 已知 desktop TUN auto_route=false 显式发 typed hijack；true/缺席仍 true 且省略 dns_mode。
//! - mobile alpha.8、未知平台和非 TUN 不发新键；原 37 金样不改。
//! - 精确审计候选为 D052297f735b80bdae5b2536ef2a07d04979a0511 的
//!   sing-tun v0.9.7-0.20261009022811-5c2edb183cc9，模块 ZIP
//!   h1:VBWPwC1CM/9ULXCtl120Nh64tmjFsybhBvB3IWzF+Mc=；tun.go 的空模式
//!   只在 AutoRoute=true 返回 hijack，false 返回 disabled。后面的旧版审计是历史记录。
//! - 新版 Windows configure 去掉两处 AutoRoute guard；直接显式 hijack 会把旧 false
//!   的 SetDNS(family,nil,nil) 改为派生 server，故必须由 source owner 恢复 IPv4/IPv6 guard
//!   和 else nil，保留外部配置 bypass、family/error/Start 边界，并交真实调用代码的 mock 矩阵。
//!   EMPTY mode 也必须覆盖 true→derived / false→nil；mock 和 check 都不是设备验收。
//! - 本文件保留旧严格版本 pin。新 source/tagObject/replacement/receipt 未签收，不猜填、不放行。
//!   仅候选生成测试绿不能发布；最终版本、替换模块及 hash 从新真实 receipt 取，再重新审默认。
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
/// 按平台和 AutoRoute 审计空模式默认及 OS 副作用；true 默认必须仍为 hijack，
/// false 必须由显式模式与已签源码保持旧契约。取源码的路径（符号表被剥时仍可用）：
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
//
// 2026-09-25 随随包核 1.15.0-alpha.7 → **1.15.0-alpha.8** 复核。sing-tun 从
// `v0.9.6-0.20260922105247-aff4131a9e9e` 跳到 `v0.9.6-0.20260924073434-3077c705bbdb`
// （alpha.8 的 go.mod:57）。`compare/aff4131a9e9e...3077c705bbdb` 仍是 **diverged**（ahead 21 / behind 17），
// 故同样取两版 proxy.golang.org 模块 zip 直接对比内容（`diff -rq`：12 个文件有差、0 增 0 删）：
//   ① 默认值直读：`tun.go` 唯一改动是 `Options` 多了 `BridgeInterface []string`（:102）；
//      `tun.go:134-139` 的 `DNSModeOrDefault()` 仍是 `if o.DNSMode == "" { return DNSModeHijack }`，
//      枚举 `DNSModeHijack = "hijack"`（tun.go:66）未变；
//   ② 消费面兜底：抹掉行号与首尾空白后，非测试 `.go` 里提到 `DNSMode` 的**行集合两版逐字相同**；
//      sing-box 侧 `protocol/tun/inbound.go:215` 仍是把 `options.DNSMode` 原样透传（a8 该文件只在 :384
//      多了一行 `BridgeInterface` 赋值）；
//   ③ `StrictRoute` 的消费面（`tun.go` 声明 / `tun_linux.go` / `tun_windows.go` / `redirect_iptables.go` /
//      `redirect_nftables_rules.go`）逐文件行集合两版逐字相同，`tun_darwin.go` 仍零引用。
// 前提未变，故只更新 pin，不改机制。
const SING_TUN_PINNED: &str = "v0.9.6-0.20260924073434-3077c705bbdb";

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

// Decode only Go 1.18+ inline BuildInfo, not arbitrary plaintext in the binary.
// Format/framing comes from installed Go debug/buildinfo and cmd/go/modload.
#[derive(Debug)]
struct EmbeddedModule {
    kind: String,
    version: String,
    replacement: Option<(String, String)>,
}

#[derive(Debug)]
struct EmbeddedIdentity {
    go_version: String,
    modules: std::collections::BTreeMap<String, EmbeddedModule>,
    settings: std::collections::BTreeMap<String, String>,
}

fn inline_string<'a>(bin: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], &'static str> {
    let mut len = 0u64;
    let mut terminated = false;
    for shift in (0..70).step_by(7) {
        let byte = *bin.get(*cursor).ok_or("truncated inline string length")?;
        *cursor += 1;
        if shift == 63 && byte > 1 {
            return Err("overflow inline string length");
        }
        len |= u64::from(byte & 127) << shift;
        if byte < 128 {
            terminated = true;
            break;
        }
    }
    if !terminated || len > 1024 * 1024 {
        return Err("invalid inline string length");
    }
    let end = cursor
        .checked_add(len as usize)
        .ok_or("inline string overflow")?;
    let data = bin.get(*cursor..end).ok_or("truncated inline string")?;
    *cursor = end;
    Ok(data)
}

fn embedded_identity(bin: &[u8]) -> Result<EmbeddedIdentity, &'static str> {
    const MAGIC: &[u8] = b"\xff Go buildinf:";
    const START: &[u8] = &[
        0x30, 0x77, 0xaf, 0x0c, 0x92, 0x74, 0x08, 0x02, 0x41, 0xe1, 0xc1, 0x07, 0xe6, 0xd6, 0x18,
        0xe6,
    ];
    const END: &[u8] = &[
        0xf9, 0x32, 0x43, 0x31, 0x86, 0x18, 0x20, 0x72, 0x00, 0x82, 0x42, 0x10, 0x41, 0x16, 0xd8,
        0xf2,
    ];
    let headers: Vec<_> = bin
        .windows(MAGIC.len())
        .enumerate()
        .filter_map(|(at, value)| {
            let header = bin.get(at..at + 32)?;
            (value == MAGIC
                && at.is_multiple_of(16)
                && header[14] == 8
                && header[15] & 2 == 2
                && header[15] <= 3
                && header[16..].iter().all(|byte| *byte == 0))
            .then_some(at)
        })
        .collect();
    if headers.len() != 1 {
        return Err("missing/duplicate inline Go BuildInfo");
    }
    let mut cursor = headers[0] + 32;
    let go_version = std::str::from_utf8(inline_string(bin, &mut cursor)?)
        .map_err(|_| "invalid Go version")?
        .to_owned();
    let framed = inline_string(bin, &mut cursor)?;
    if framed.len() < 33
        || !framed.starts_with(START)
        || !framed.ends_with(END)
        || framed[framed.len() - 17] != b'\n'
    {
        return Err("invalid module framing");
    }
    let info =
        std::str::from_utf8(&framed[16..framed.len() - 16]).map_err(|_| "invalid module text")?;
    let mut result = EmbeddedIdentity {
        go_version,
        modules: Default::default(),
        settings: Default::default(),
    };
    let mut previous: Option<String> = None;
    for line in info.split_terminator('\n') {
        let parts: Vec<_> = line.split('\t').collect();
        match parts[0] {
            "mod" | "dep" => {
                if !(3..=4).contains(&parts.len())
                    || parts[1].is_empty()
                    || parts[2].is_empty()
                    || result.modules.contains_key(parts[1])
                {
                    return Err("duplicate/malformed module");
                }
                result.modules.insert(
                    parts[1].to_owned(),
                    EmbeddedModule {
                        kind: parts[0].to_owned(),
                        version: parts[2].to_owned(),
                        replacement: None,
                    },
                );
                previous = Some(parts[1].to_owned());
            }
            "=>" => {
                let key = previous.take().ok_or("unbound replacement")?;
                if !(3..=4).contains(&parts.len())
                    || parts[1].is_empty()
                    || parts.get(3).is_some_and(|sum| !sum.is_empty())
                {
                    return Err("malformed local replacement");
                }
                let module = result
                    .modules
                    .get_mut(&key)
                    .ok_or("missing replacement module")?;
                if module.replacement.is_some() {
                    return Err("duplicate replacement");
                }
                module.replacement = Some((parts[1].to_owned(), parts[2].to_owned()));
            }
            "build" => {
                if parts.len() != 2 {
                    return Err("malformed build setting");
                }
                let (key, value) = parts[1].split_once('=').ok_or("invalid build setting")?;
                if key.is_empty() || result.settings.contains_key(key) {
                    return Err("duplicate build setting");
                }
                let value = if value.starts_with('"') {
                    serde_json::from_str::<String>(value)
                        .map_err(|_| "invalid quoted build setting")?
                } else {
                    value.to_owned()
                };
                result.settings.insert(key.to_owned(), value);
                previous = None;
            }
            "path" if parts.len() == 2 && !parts[1].is_empty() => {
                previous = None;
            }
            "" if line.is_empty() => {
                previous = None;
            }
            _ => return Err("unsupported BuildInfo row"),
        }
    }
    Ok(result)
}

fn verify_desktop_embedded_identity(
    bytes: &[u8],
    source: &serde_json::Value,
    spec: &serde_json::Value,
    receipt: &serde_json::Value,
    observed_build_id: &str,
) -> Result<(), &'static str> {
    let declared = spec["sourceOwnedReplacements"]
        .as_array()
        .ok_or("missing source-owned declaration")?;
    if declared.len() != 1
        || source["sourceOwnedReplacements"] != spec["sourceOwnedReplacements"]
        || receipt["sourceOwnedReplacements"] != spec["sourceOwnedReplacements"]
        || source["schema"] != "polaris-desktop-fork-source-v1"
        || source["role"] != "desktop"
        || !matches!(source["sourceURL"].as_str(),
            Some("https://github.com/polaris-arch/polaris-box"
                | "https://github.com/polaris-arch/sing-box-polaris"))
        || source["sourceTagObject"]
            .as_str()
            .is_none_or(|value| !hex_pin(value, 40))
        || source["sourceCommit"]
            .as_str()
            .is_none_or(|value| !hex_pin(value, 40))
        || source["sourceTree"] != spec["buildTree"]
        || spec["sourceMode"] != "fork-commit-v1"
        || receipt["schema"] != "polaris-desktop-core-v1"
        || receipt["version"] != spec["version"]
        || receipt["sourceFingerprint"] != spec["sourceReceiptFingerprint"]
        || receipt["sourceReceipt"]["sourceCommit"] != source["sourceCommit"]
    {
        return Err("source-owned receipt/source identity differs");
    }
    let identity = embedded_identity(bytes)?;
    if identity.go_version
        != format!(
            "go{}",
            source["goVersion"].as_str().ok_or("missing toolchain")?
        )
    {
        return Err("embedded toolchain differs");
    }
    let roots: Vec<_> = identity
        .modules
        .iter()
        .filter(|(_, value)| value.kind == "mod")
        .collect();
    if roots.len() != 1
        || roots[0].0 != "github.com/sagernet/sing-box"
        || roots[0].1.replacement.is_some()
        || identity.settings.get("vcs.revision").map(String::as_str)
            != source["sourceCommit"].as_str()
        || identity.settings.get("vcs.modified").map(String::as_str) != Some("false")
    {
        return Err("embedded unique root/clean VCS differs");
    }
    let dep = &declared[0];
    if dep["module"] != SING_TUN_MODULE || dep["replacementPath"] != "./third_party/sing-tun" {
        return Err("source-owned module/path differs");
    }
    let actual = identity
        .modules
        .get(SING_TUN_MODULE)
        .ok_or("missing linked sing-tun")?;
    if actual.kind != "dep"
        || Some(actual.version.as_str()) != dep["version"].as_str()
        || actual
            .replacement
            .as_ref()
            .map(|(path, version)| (path.as_str(), version.as_str()))
            != Some(("./third_party/sing-tun", "(devel)"))
        || identity
            .modules
            .iter()
            .any(|(module, value)| value.replacement.is_some() && module != SING_TUN_MODULE)
    {
        return Err("embedded unique source-owned replacement differs");
    }
    let fingerprint = receipt["platformInputFingerprint"]
        .as_str()
        .ok_or("missing platform input")?;
    let build_id = format!("polaris-desktop-source-v1-{fingerprint}");
    // Go omits -ldflags from BuildInfo with -trimpath. Reuse the existing
    // consumer's static Go buildid reader; never execute the target kernel.
    if !hex_pin(fingerprint, 64) || receipt["buildID"] != build_id || observed_build_id != build_id
    {
        return Err("actual production BuildID differs");
    }
    Ok(())
}

fn verify_release_dependency(
    bytes: &[u8],
    declared: &serde_json::Value,
) -> Result<(), &'static str> {
    let identity = embedded_identity(bytes)?;
    let actual = identity
        .modules
        .get(SING_TUN_MODULE)
        .ok_or("missing published sing-tun")?;
    if declared["module"] != SING_TUN_MODULE
        || declared["version"].as_str() != Some(actual.version.as_str())
        || actual.kind != "dep"
        || actual
            .replacement
            .as_ref()
            .map(|(path, version)| (path.as_str(), version.as_str()))
            != Some(("./third_party/sing-tun", "(devel)"))
    {
        return Err("published sing-tun dependency/replacement differs");
    }
    Ok(())
}

fn hex_pin(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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

    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("src-tauri/core-manifest.json")).unwrap(),
    )
    .unwrap();
    let desktop = &manifest["desktopSourceBuild"];
    let owned_mode = desktop.get("sourceOwnedReplacements").is_some();
    for (core, path) in &present {
        let bytes =
            std::fs::read(path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()));

        if manifest.get("desktopRelease").is_some() {
            // The existing release verifier checks the exact public contract, App
            // adapter/receipt, original bytes, full module graph and actual BuildID.
            // It observes native Mac signatures on Mac; no foreign core is run.
            let inspected = std::process::Command::new("node")
                .args(["--input-type=module", "-e", "import fs from 'node:fs'; import {loadReleaseContract,verifyPackagedRelease,releaseSingTunDependency} from './scripts/desktop-core/release-consumer.mjs'; const root=process.cwd(), m=JSON.parse(fs.readFileSync('src-tauri/core-manifest.json')), key=process.argv[1], core=process.argv[2], c=loadReleaseContract(root,m); verifyPackagedRelease(root,m,key,core); console.log(JSON.stringify(releaseSingTunDependency(c.platforms[key])));"])
                .arg(core.key)
                .arg(path)
                .current_dir(repo_root())
                .output()
                .expect("Node release dependency verifier required");
            assert!(
                inspected.status.success(),
                "{}: published dependency verification failed: {}",
                core.key,
                String::from_utf8_lossy(&inspected.stderr)
            );
            let declared: serde_json::Value = serde_json::from_slice(&inspected.stdout)
                .expect("Release dependency row must be JSON");
            verify_release_dependency(&bytes, &declared)
                .unwrap_or_else(|error| panic!("{}: {error}", core.key));
            continue;
        }
        if owned_mode {
            let source: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(
                    repo_root().join("scripts/desktop-core/source-manifest.json"),
                )
                .unwrap(),
            )
            .unwrap();
            let metadata = repo_root().join("resources/.source-receipts");
            let receipt: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(metadata.join(format!("{}.json", core.key))).unwrap(),
            )
            .unwrap();
            let bundle: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(metadata.join("bundle.json")).unwrap(),
            )
            .unwrap();
            assert!(
                bundle["platforms"]
                    .as_object()
                    .is_some_and(|platforms| platforms.len() == 4),
                "strict four-core receipt inventory required"
            );
            assert_eq!(receipt["platform"], core.key);
            assert_eq!(receipt["candidate"], bundle["candidate"]);
            let context = &receipt["producerContext"];
            assert_eq!(context["repository"], "polaris-arch/Polaris");
            assert_eq!(context["repositoryId"], "1347028220");
            assert_eq!(context["candidate"], bundle["candidate"]);
            for field in ["runId", "runAttempt"] {
                assert!(
                    context[field]
                        .as_str()
                        .is_some_and(|v| v.parse::<u64>().is_ok_and(|n| n > 0)),
                    "producer context missing: {field}"
                );
            }
            if std::env::var("GITHUB_ACTIONS").as_deref() == Ok("true") {
                for (field, env) in [
                    ("runId", "GITHUB_RUN_ID"),
                    ("runAttempt", "GITHUB_RUN_ATTEMPT"),
                    ("workflowRef", "GITHUB_WORKFLOW_REF"),
                    ("workflowHead", "GITHUB_SHA"),
                    ("ref", "GITHUB_REF"),
                    ("event", "GITHUB_EVENT_NAME"),
                ] {
                    assert_eq!(
                        context[field].as_str(),
                        std::env::var(env).ok().as_deref(),
                        "current producer context differs: {field}"
                    );
                }
            }
            if bundle["schema"] == "polaris-desktop-validation-bundle-v2" {
                assert_eq!(bundle["mode"], "source-owned-all-current-v1");
                let mut origin = context.clone();
                origin["kind"] = "current-native".into();
                assert_eq!(
                    bundle["origins"][core.key], origin,
                    "old native attempt cannot be relabeled"
                );
            } else {
                assert_eq!(bundle["schema"], "polaris-desktop-bundle-v1");
            }
            let buildid = std::process::Command::new("go")
                .args(["tool", "buildid"])
                .arg(path)
                .env("GOTOOLCHAIN", "local")
                .env("GOWORK", "off")
                .env("GOENV", "off")
                .output()
                .expect("static Go buildid reader required; target core is not executed");
            assert!(
                buildid.status.success(),
                "{}: static BuildID read failed",
                core.key
            );
            let actual_build_id = std::str::from_utf8(&buildid.stdout)
                .expect("Go BuildID must be UTF-8")
                .trim();
            verify_desktop_embedded_identity(&bytes, &source, desktop, &receipt, actual_build_id)
                .unwrap_or_else(|error| panic!("{}: {error}", core.key));
            continue;
        }
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
             桌面 TUN auto_route=false 显式下发 typed dns_mode:hijack；true/缺省保持 true 且省略该键，\
             仍依赖精确审计的默认值。mobile alpha.8、未知平台和非 TUN 不发新键。\n\
             ⇒ 按平台/AutoRoute 复核新版 DNSModeOrDefault() 与调用副作用；Windows 必须先签收\
             IPv4/IPv6 AutoRoute guards、else SetDNS(family,nil,nil) 及实际调用 mock 矩阵，\
             保留 external bypass/family/error/Start 边界。源码/tagObject/replacement/hash/receipt\
             未定不得猜填，不能只更新 SING_TUN_PINNED 放行；实际门仍保持严格四核版本检查。\n",
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

/// Pending-source release lock, not a claim about OS behavior. Remove/update only
/// together with independently signed corrected source/dependency receipt/tests.
#[test]
fn candidate_d_dns_dependency_has_not_been_release_authorized() {
    let candidate = b"dep\tgithub.com/sagernet/sing-tun\tv0.9.7-0.20261009022811-5c2edb183cc9\th1:VBWPwC1CM/9ULXCtl120Nh64tmjFsybhBvB3IWzF+Mc=\n";
    let version = extract_dep_version(candidate, SING_TUN_MODULE).unwrap();
    assert_ne!(
        version, SING_TUN_PINNED,
        "unmodified D must remain blocked pending Windows source fix"
    );
    let source: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo_root().join("scripts/desktop-core/source-manifest.json"))
            .unwrap(),
    )
    .unwrap();
    if source.get("sourceOwnedReplacements").is_none() {
        assert_eq!(source["sourceCommit"], "052297f735b80bdae5b2536ef2a07d04979a0511", "source changed: re-audit exact dependency/default/Windows contract before changing this lock");
    }
}

fn framed_identity_fixture(info: &str) -> Vec<u8> {
    fn append(bytes: &mut Vec<u8>, data: &[u8]) {
        let mut size = data.len();
        while size >= 128 {
            bytes.push((size as u8 & 127) | 128);
            size >>= 7;
        }
        bytes.push(size as u8);
        bytes.extend_from_slice(data);
    }
    let mut bytes = vec![0u8; 32];
    bytes[..14].copy_from_slice(b"\xff Go buildinf:");
    bytes[14] = 8;
    bytes[15] = 2;
    append(&mut bytes, b"go1.25.5");
    let mut framed = vec![
        0x30, 0x77, 0xaf, 0x0c, 0x92, 0x74, 0x08, 0x02, 0x41, 0xe1, 0xc1, 0x07, 0xe6, 0xd6, 0x18,
        0xe6,
    ];
    framed.extend_from_slice(info.as_bytes());
    framed.extend_from_slice(&[
        0xf9, 0x32, 0x43, 0x31, 0x86, 0x18, 0x20, 0x72, 0x00, 0x82, 0x42, 0x10, 0x41, 0x16, 0xd8,
        0xf2,
    ]);
    append(&mut bytes, &framed);
    bytes
}

#[test]
fn source_owned_embedded_identity_rejects_plaintext_and_broken_framing() {
    let info = "path\tgithub.com/sagernet/sing-box/cmd/sing-box\nmod\tgithub.com/sagernet/sing-box\tv0.0.0-fixture\ndep\tgithub.com/sagernet/sing-tun\tv0.9.7-fixture\n=>\t./third_party/sing-tun\t(devel)\n\nbuild\tvcs.modified=false\n";
    let bytes = framed_identity_fixture(info);
    assert_eq!(
        embedded_identity(&bytes).unwrap().modules[SING_TUN_MODULE]
            .replacement
            .as_ref()
            .unwrap()
            .0,
        "./third_party/sing-tun"
    );
    assert!(embedded_identity(info.as_bytes()).is_err());
    assert!(embedded_identity(&bytes[..bytes.len() - 1]).is_err());
    let mut bad = bytes.clone();
    *bad.last_mut().unwrap() ^= 1;
    assert!(embedded_identity(&bad).is_err());
    let mut duplicate = bytes.clone();
    while !duplicate.len().is_multiple_of(16) {
        duplicate.push(0);
    }
    duplicate.extend_from_slice(&bytes);
    assert!(embedded_identity(&duplicate).is_err());
    for extra in [
        "dep\tgithub.com/sagernet/sing-tun\tv9\n",
        "build\tvcs.modified=true\n",
        "=>\t./arbitrary\t(devel)\n",
    ] {
        assert!(embedded_identity(&framed_identity_fixture(&format!("{info}{extra}"))).is_err());
    }
}

#[test]
fn source_owned_embedded_identity_requires_exact_root_replacement_vcs_and_build_identity() {
    let commit = "c".repeat(40);
    let fp = "f".repeat(64);
    let source_fp = "a".repeat(64);
    let declaration = serde_json::json!([{ "module": SING_TUN_MODULE, "version": "v0.9.7-fixture", "replacementPath": "./third_party/sing-tun" }]);
    let source = serde_json::json!({ "schema": "polaris-desktop-fork-source-v1", "role": "desktop", "sourceURL": "https://github.com/polaris-arch/polaris-box",
      "sourceTagObject": "b".repeat(40), "sourceCommit": commit, "sourceTree": "d".repeat(40), "goVersion": "1.25.5", "sourceOwnedReplacements": declaration });
    let spec = serde_json::json!({ "sourceMode": "fork-commit-v1", "sourceOwnedReplacements": declaration, "buildTree": "d".repeat(40), "version": "1.0.0.polaris.1", "sourceReceiptFingerprint": source_fp });
    let receipt = serde_json::json!({ "schema": "polaris-desktop-core-v1", "version": "1.0.0.polaris.1", "sourceOwnedReplacements": declaration, "sourceFingerprint": source_fp,
      "sourceReceipt": {"sourceCommit": commit}, "platformInputFingerprint": fp, "buildID": format!("polaris-desktop-source-v1-{fp}") });
    let info = format!("path\tgithub.com/sagernet/sing-box/cmd/sing-box\nmod\tgithub.com/sagernet/sing-box\tv0.0.0-fixture\ndep\t{SING_TUN_MODULE}\tv0.9.7-fixture\n=>\t./third_party/sing-tun\t(devel)\n\nbuild\tvcs.revision={commit}\nbuild\tvcs.modified=false\nbuild\t-trimpath=true\n");
    let actual_build_id = format!("polaris-desktop-source-v1-{fp}");
    verify_desktop_embedded_identity(
        &framed_identity_fixture(&info),
        &source,
        &spec,
        &receipt,
        &actual_build_id,
    )
    .unwrap();
    let mut migrated_source = source.clone();
    migrated_source["sourceURL"] = serde_json::json!("https://github.com/polaris-arch/sing-box-polaris");
    verify_desktop_embedded_identity(
        &framed_identity_fixture(&info), &migrated_source, &spec, &receipt, &actual_build_id,
    ).unwrap();
    for url in ["https://github.com/polaris-arch/sing-box-polaris/", "https://github.com/polaris-arch/sing-box-polaris-other"] {
        migrated_source["sourceURL"] = serde_json::json!(url);
        assert!(verify_desktop_embedded_identity(
            &framed_identity_fixture(&info), &migrated_source, &spec, &receipt, &actual_build_id,
        ).is_err());
    }
    for changed in [
        info.replace("sing-box\tv0", "sing-box-extra\tv0"),
        info.replace("(devel)", "v1"),
        info.replace("./third_party/sing-tun", "/absolute/sing-tun"),
        info.replace("vcs.modified=false", "vcs.modified=true"),
        info.replace(&commit, &"e".repeat(40)),
        info.replace("v0.9.7-fixture", "v0.9.8-fixture"),
        format!("{info}mod\texample.com/workspace\tv0.0.0\n"),
        format!("{info}dep\texample.com/extra\tv1\n=>\t./extra\t(devel)\n"),
    ] {
        assert!(verify_desktop_embedded_identity(
            &framed_identity_fixture(&changed),
            &source,
            &spec,
            &receipt,
            &actual_build_id
        )
        .is_err());
    }
    for id in [
        format!("polaris-dns-technical-v1-{fp}"),
        String::new(),
        format!("polaris-desktop-source-v1-{}", "e".repeat(64)),
    ] {
        assert!(verify_desktop_embedded_identity(
            &framed_identity_fixture(&info),
            &source,
            &spec,
            &receipt,
            &id
        )
        .is_err());
    }
    let mut wrong_version = receipt.clone();
    wrong_version["version"] = "candidate".into();
    assert!(verify_desktop_embedded_identity(
        &framed_identity_fixture(&info),
        &source,
        &spec,
        &wrong_version,
        &actual_build_id
    )
    .is_err());
    let mut technical = receipt.clone();
    technical["schema"] = "polaris-dns-technical-native-v1".into();
    assert!(verify_desktop_embedded_identity(
        &framed_identity_fixture(&info),
        &source,
        &spec,
        &technical,
        &actual_build_id
    )
    .is_err());
}

#[test]
fn published_dependency_still_rejects_wrong_version_path_and_missing_modinfo() {
    let declared = serde_json::json!({"module": SING_TUN_MODULE, "version": "v0.9.7-fixture"});
    let info = "path\tgithub.com/sagernet/sing-box/cmd/sing-box\nmod\tgithub.com/sagernet/sing-box\tv0.0.0-fixture\ndep\tgithub.com/sagernet/sing-tun\tv0.9.7-fixture\n=>\t./third_party/sing-tun\t(devel)\n\nbuild\tvcs.modified=false\n";
    assert!(verify_release_dependency(&framed_identity_fixture(info), &declared).is_ok());
    for bad in [
        info.replace("v0.9.7-fixture", "v0.9.8-fixture"),
        info.replace("./third_party/sing-tun", "./other"),
        info.replace("(devel)", "v1.0.0"),
        info.replace(
            "github.com/sagernet/sing-tun",
            "github.com/sagernet/sing-tun-extra",
        ),
    ] {
        assert!(verify_release_dependency(&framed_identity_fixture(&bad), &declared).is_err());
    }
    assert!(verify_release_dependency(info.as_bytes(), &declared).is_err());
}
