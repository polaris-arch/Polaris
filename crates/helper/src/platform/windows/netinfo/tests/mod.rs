use super::*;

/// v4 八位组 → 点分串（网络序即书写序）。变异：调换字节序 → 转红。
#[test]
fn v4_octets_render_dotted_quad() {
    assert_eq!(v4_octets_to_string([192, 168, 10, 5]), "192.168.10.5");
    assert_eq!(v4_octets_to_string([0, 0, 0, 0]), "0.0.0.0");
    assert_eq!(v4_octets_to_string([255, 255, 255, 255]), "255.255.255.255");
}

/// v6 十六字节 → 压缩串（`Ipv6Addr` 的标准压缩，与 `os.networkInterfaces()` 的写法同）。
#[test]
fn v6_octets_render_compressed() {
    let mut o = [0u8; 16];
    o[0] = 0xfe;
    o[1] = 0x80;
    o[15] = 0x01;
    assert_eq!(v6_octets_to_string(o), "fe80::1");
}

/// 前缀合法性：合法域 v4 `1..=32` / v6 `1..=128`；**哨兵 0 与 255 都必须被拒**。
///
/// 变异锁：
/// - 去掉校验（恒 true）→ 哨兵四条转红；
/// - 把下界放回 0（`prefix <= 32` / `prefix <= 128`）→ `prefix_is_valid(0, _)` 两条转红
///   （后果侧另有 `config-engine::builder::tun_route_exclude` 的 carve 全 skip 用例）；
/// - 把 v4 上限写成 128 → `33` 那条转红；
/// - 把下界收到 2 → `prefix_is_valid(1, _)` 两条转红。
#[test]
fn prefix_bounds_reject_sentinels() {
    assert!(prefix_is_valid(24, false));
    assert!(prefix_is_valid(32, false));
    assert!(prefix_is_valid(1, false), "下界 1 是合法前缀，别一起误杀");
    assert!(prefix_is_valid(64, true));
    assert!(prefix_is_valid(128, true));
    assert!(
        prefix_is_valid(1, true),
        "下界 1 是合法前缀，别一起误杀（v6）"
    );
    assert!(
        !prefix_is_valid(0, false),
        "哨兵 0 必须丢弃（默认路由非本机 LAN 段）"
    );
    assert!(!prefix_is_valid(0, true), "哨兵 0 必须丢弃（v6）");
    assert!(!prefix_is_valid(255, false), "哨兵 255 必须丢弃");
    assert!(!prefix_is_valid(255, true), "哨兵 255 必须丢弃（v6）");
    assert!(!prefix_is_valid(33, false), "v4 前缀不得超 32");
    assert!(!prefix_is_valid(129, true), "v6 前缀不得超 128");
}

/// 缓冲区容量换算：向上取整到 u64 槽，**容量永不缩水**（缩水 = API 往 buf 外写）。
///
/// 变异锁：把 `div_ceil` 换成整除 `/ 8` → `1 / 7 / 9` 三条转红；把槽宽写成 4 → `9` 那条转红
/// （得 3 而非 2）。u32::MAX 一条锁住无溢出。
#[test]
fn u64_cells_never_shrink_capacity() {
    assert_eq!(u64_cells_for(0), 0);
    assert_eq!(u64_cells_for(1), 1);
    assert_eq!(u64_cells_for(7), 1);
    assert_eq!(u64_cells_for(8), 1);
    assert_eq!(u64_cells_for(9), 2);
    assert_eq!(u64_cells_for(u32::MAX), 536_870_912);
    // 不变式：分配到的字节数 ≥ API 要的字节数。
    for size in [0u32, 1, 7, 8, 9, 4095, 4096, u32::MAX] {
        assert!(
            u64_cells_for(size) * 8 >= size as usize,
            "size={size} 的容量缩水了"
        );
    }
}

/// 重试预算判据：探大小与填充**共用** [`SIZE_PROBE_MAX_RETRIES`]，第 3 次用完即放弃。
///
/// 诚实说明：真 FFI（`GetAdaptersAddresses`）本机跑不到，本用例覆盖的是「第几次该放弃」这条判据，
/// 不是 FFI 行为。变异锁：把 `<` 改成 `<=`（预算多一次）→ `retries=3` 那条转红；把两条腿拆成各自
/// 预算（填充腿不再调本函数）→ 本用例照绿，但那属接线，靠 Windows 交叉编译 + 真机覆盖。
#[test]
fn overflow_retry_budget_is_shared_and_bounded() {
    assert_eq!(SIZE_PROBE_MAX_RETRIES, 3);
    assert!(should_retry_after_overflow(0));
    assert!(should_retry_after_overflow(1));
    assert!(should_retry_after_overflow(2));
    assert!(!should_retry_after_overflow(3), "预算用尽必须放弃");
    assert!(!should_retry_after_overflow(u32::MAX));
}

/// 回环 ifType 常量锁死（IANA 24）。变异：改值 → 转红（改了会让回环地址混进 own-lan，
/// 而 own_lan_cidr 正是靠 `is_loopback` 剔除它们）。
#[test]
fn loopback_if_type_is_iana_24() {
    assert_eq!(IF_TYPE_SOFTWARE_LOOPBACK, 24);
}

// ── D1 取材腿的纯逻辑（路由前缀规范化 / RAS 作用域位 / 宽字符名解码）──────────────────

/// 🔴 **路由前缀的 `/0` 必须留下** —— 这是本组最重要的一条。
///
/// 正面断言：`0.0.0.0/0` 与 `::/0` 都产出。它们是「某个 VPN 要走你全部流量」这件事在
/// 数据面上的**唯一**载体（`ForeignTunnelSnapshot::default_routes` 那条通道）。
///
/// 变异锁：把 [`route_prefix_len_is_valid`] 写成复用 [`prefix_is_valid`]（own_lan 那条判据，
/// 合法域 `1..=32` / `1..=128`）→ 头两条当场红。反向对照：越界长度仍必须被拒，
/// 否则「合法域放宽到全部 u8」这种改法能骗过前两条。
#[test]
fn route_prefixes_keep_the_default_route_but_reject_out_of_range_lengths() {
    assert_eq!(
        v4_route_prefix([0, 0, 0, 0], 0).as_deref(),
        Some("0.0.0.0/0"),
        "v4 默认路由被丢了 —— 抢默认路由的全隧道会整条失联"
    );
    assert_eq!(
        v6_route_prefix([0u8; 16], 0).as_deref(),
        Some("::/0"),
        "v6 默认路由被丢了"
    );
    assert_eq!(
        v4_route_prefix([10, 55, 0, 10], 32).as_deref(),
        Some("10.55.0.10/32")
    );
    let mut tailnet = [0u8; 16];
    tailnet[0] = 0xfd;
    tailnet[1] = 0x7a;
    tailnet[2] = 0x11;
    tailnet[3] = 0x5c;
    assert_eq!(
        v6_route_prefix(tailnet, 48).as_deref(),
        Some("fd7a:115c::/48"),
        "v6 地址必须走标准压缩写法，否则与 route print 的规范形比不上"
    );
    // 反向对照：越界必须被拒（不折成一个看着合理的长度）。
    assert_eq!(v4_route_prefix([10, 0, 0, 0], 33), None, "v4 前缀不得超 32");
    assert_eq!(v6_route_prefix([0u8; 16], 129), None, "v6 前缀不得超 128");
    assert!(route_prefix_len_is_valid(0, false) && route_prefix_len_is_valid(0, true));
    assert!(route_prefix_len_is_valid(32, false) && !route_prefix_len_is_valid(33, false));
    assert!(route_prefix_len_is_valid(128, true) && !route_prefix_len_is_valid(129, true));
    // 🔴 与 own_lan 那条判据的分界：同一个 0，在那边是哨兵、在这边是默认路由。
    assert!(
        !prefix_is_valid(0, false) && route_prefix_len_is_valid(0, false),
        "两条判据被合并了 —— 合并的方向无论哪一边都会坏掉一条真链路"
    );
}

/// RAS 作用域位：只认 `RASCF_AllUsers`（bit0），别的位不得渗进来。
///
/// 变异锁：把判据写成 `flags != 0` → 第三条（只带 `RASCF_GlobalCreds` bit1）转红；
/// 把常量改成别的值 → 第二条转红。作用域**不是过滤判据**（两个作用域的连接都要报），
/// 这一位只用于如实标注，故本组不断言「all_users=false 的连接被丢掉」。
#[test]
fn ras_all_users_flag_reads_only_bit_zero() {
    assert_eq!(RASCF_ALL_USERS, 0x0000_0001);
    assert!(!ras_flags_are_all_users(0), "无标志位 = 当前用户作用域");
    assert!(ras_flags_are_all_users(RASCF_ALL_USERS));
    assert!(
        !ras_flags_are_all_users(0b10),
        "RASCF_GlobalCreds（bit1）不是 all-user 作用域"
    );
    assert!(
        ras_flags_are_all_users(RASCF_ALL_USERS | 0b10),
        "同时带别的位时仍应判 all-user"
    );
}

/// A1：`ConvertInterfaceLuidToAlias` 的哪些码算「接口已消失」（跳过该行）、哪些仍必须整表报错。
///
/// 两侧都写死，缺一边就没牙：
/// - **正面**：`1168` / `87` 必须判 true —— 否则接口热插时整张路由表被丢弃，起核峰值下
///   用户拿到的是「探测失败」而不是冲突提示（本条修的就是这个）；
/// - **反面**：其余码必须判 false —— 判据若写成 `code != 0` 恒 true，「读法塌了」这一格会被
///   悄悄吞成「少几行」，而少掉的那几行可能正是唯一那条隧道宣告。
///
/// 变异锁：判据改成恒 true → `ERROR_ACCESS_DENIED` 等四条转红；只留 `ERROR_NOT_FOUND` 一格 →
/// `ERROR_INVALID_PARAMETER` 那条转红。常量本身另有 `win_impl` 的 `const _` 对着 windows-sys 钉死。
#[test]
fn only_the_interface_gone_codes_skip_a_route_row() {
    assert_eq!(ERROR_NOT_FOUND, 1168);
    assert_eq!(ERROR_INVALID_PARAMETER, 87);
    assert!(luid_alias_error_means_interface_gone(ERROR_NOT_FOUND));
    assert!(luid_alias_error_means_interface_gone(
        ERROR_INVALID_PARAMETER
    ));
    // 反面对照：这些是「读法塌了」，必须让整次枚举失败。
    assert!(
        !luid_alias_error_means_interface_gone(0),
        "NO_ERROR 不是失败"
    );
    assert!(
        !luid_alias_error_means_interface_gone(5),
        "ERROR_ACCESS_DENIED 不是「接口没了」"
    );
    assert!(
        !luid_alias_error_means_interface_gone(122),
        "ERROR_INSUFFICIENT_BUFFER 不是「接口没了」"
    );
    assert!(!luid_alias_error_means_interface_gone(u32::MAX));
}

/// A2：RAS 两步取缓冲的下一轮容量 —— 按 API 回写的字节数算，但**每轮必须真的变大**。
///
/// 正面断言：`cb` 说要 3 条就开 3 条；不足一个结构体大小也至少开 1 条（向上取整）。
/// 反面断言（本条的要害）：`cb` 没变大时仍必须 `> current` —— 否则重试在原地空转，
/// 预算烧完后一次本来能成的枚举被判失败，RAS 那一族隧道整族失联。
///
/// 变异锁：去掉 `.max(current + 1)` → 后两条转红；把 `div_ceil` 换成整除 → 第二条转红。
#[test]
fn ras_buffer_growth_is_monotonic() {
    let size = 40usize; // 用一个固定的“结构体大小”跑判据，与真 RASCONNW 布局无关。
    assert_eq!(ras_entries_for(120, size, 1), 3);
    assert_eq!(ras_entries_for(121, size, 1), 4, "不足一个槽也要向上取整");
    assert_eq!(ras_entries_for(0, size, 1), 2, "cb=0 时仍必须推进");
    assert_eq!(
        ras_entries_for(40, size, 1),
        2,
        "API 回写的大小没变大时，重试必须自己往前走一格，否则原地空转烧完预算"
    );
    for current in [1usize, 2, 7, 64] {
        assert!(
            ras_entries_for(40, size, current) > current,
            "current={current} 时没有增长 —— 重试不推进"
        );
    }
}

/// 定长 WCHAR 缓冲按 NUL 截断；无 NUL 时吃满整个缓冲。
///
/// 变异锁：去掉截断（整段 `from_utf16_lossy`）→ 第一条转红（尾巴上的 `\0` 会被带进 String，
/// 而下游是拿它与路由表接口别名**逐字**比的，带尾巴 = RAS 那一族整族失联）。
#[test]
fn wide_buffers_are_cut_at_the_nul() {
    let mut buf = [0u16; 257];
    for (slot, ch) in buf.iter_mut().zip("PolarisProbeL2TP".encode_utf16()) {
        *slot = ch;
    }
    assert_eq!(wide_nul_terminated_to_string(&buf), "PolarisProbeL2TP");
    // 中文连接名（RAS 连接名可以是任何 UTF-16 串）。
    let cn: Vec<u16> = "公司 VPN\0残留".encode_utf16().collect();
    assert_eq!(wide_nul_terminated_to_string(&cn), "公司 VPN");
    // 无 NUL：吃满缓冲，不 panic、不截半个码点。
    let full: Vec<u16> = "abc".encode_utf16().collect();
    assert_eq!(wide_nul_terminated_to_string(&full), "abc");
    assert_eq!(wide_nul_terminated_to_string(&[]), "");
    assert_eq!(wide_nul_terminated_to_string(&[0u16; 8]), "");
}
