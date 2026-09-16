use super::*;

// ===== iface_allowed（逐字对照 Go helper.go:50-60）=====

#[test]
fn iface_allowed_matches_go_source() {
    // Go ifaceAllowed：polaris- 前缀 + rest 每字符须 [a-z0-9-]，长度 ≤ 24
    assert!(iface_allowed("polaris-ts"));
    assert!(iface_allowed("polaris-wg"));
    assert!(iface_allowed("polaris-tun0"));
    assert!(iface_allowed("polaris-abc-123"));
    assert!(iface_allowed("polaris-")); // rest 空时 Go 源返回 true（for 循环不执行）
                                        // 拒绝
    assert!(!iface_allowed("polaris-ABC")); // 大写
    assert!(!iface_allowed("polaris_abc")); // 下划线
    assert!(!iface_allowed("en0"));
    assert!(!iface_allowed(""));
    // 超长拒绝（Go: len(s) > 24）
    assert!(!iface_allowed(&format!("polaris-{}", "a".repeat(30))));
}

// ===== cfg_allowed（对照 Go helper.go:86-93）=====

#[test]
fn cfg_allowed_empty_confdir_fails_closed() {
    assert!(!cfg_allowed("C:\\anything\\cfg.json", ""));
    assert!(!cfg_allowed("/anywhere", ""));
}

#[test]
fn cfg_allowed_inside_confdir() {
    let conf = r"C:\Users\polaris\AppData\Roaming\polaris\config";
    assert!(cfg_allowed(
        r"C:\Users\polaris\AppData\Roaming\polaris\config\c.json",
        conf
    ));
    // 注：cfg_allowed(conf, conf) 在 Go 源也返回 false —— base = Clean(conf)+"\\"，clean=Clean(conf)
    //（无尾 \），HasPrefix 失配。即「配置文件必须严格位于 confDir 之内，不能是 confDir 本身」。
    assert!(!cfg_allowed(conf, conf));
}

#[test]
fn cfg_allowed_case_insensitive_windows_path() {
    // Go: strings.ToLower 前缀比对 —— Windows 路径大小写不敏感
    let conf = r"C:\Users\Polaris\config";
    assert!(cfg_allowed(r"c:\users\polaris\config\c.json", conf));
}

#[test]
fn cfg_allowed_rejects_outside_confdir() {
    let conf = r"C:\Users\polaris\config";
    assert!(!cfg_allowed(r"C:\Windows\System32\evil.json", conf));
    assert!(!cfg_allowed(r"C:\Users\polaris\config-other\c.json", conf)); // 前缀相似但不匹配
}

#[test]
fn cfg_allowed_handles_redundant_separators() {
    // normalize 合并连续分隔符 —— 防 `conf\\cfg` 与 `conf\cfg` 失配
    let conf = r"C:\polaris\config";
    assert!(cfg_allowed(r"C:\polaris\config\\c.json", conf));
    assert!(cfg_allowed(r"C:/polaris/config/c.json", conf)); // 正斜杠
}

// ===== parse_port（对照 Go helper.go:298-307）=====

#[test]
fn parse_port_valid() {
    assert_eq!(parse_port("9090"), Some(9090));
    assert_eq!(parse_port("1"), Some(1));
    assert_eq!(parse_port("65535"), Some(65535));
    assert_eq!(parse_port("  8080  "), Some(8080)); // Go: strings.TrimSpace(readLine(r))
}

#[test]
fn parse_port_rejects_bad() {
    // Go: strings.IndexFunc(非数字) >= 0 → bad-port
    assert_eq!(parse_port(""), None);
    assert_eq!(parse_port("abc"), None);
    assert_eq!(parse_port("80a"), None);
    assert_eq!(parse_port("12.5"), None);
    assert_eq!(parse_port("0"), None); // Go: pnum <= 0
    assert_eq!(parse_port("65536"), None); // Go: pnum > 65535
    assert_eq!(parse_port("-1"), None); // 负号非数字
}

// ===== is_locked_singbox / eq_ignore_ascii_case_path（对照 winproc.go:173-178）=====

#[test]
fn is_locked_singbox_case_insensitive() {
    let bin = r"C:\Program Files\Polaris\sing-box.exe";
    assert!(is_locked_singbox(bin, bin));
    assert!(is_locked_singbox(
        r"c:\program files\polaris\sing-box.exe",
        bin
    ));
    assert!(!is_locked_singbox(r"C:\other\evil.exe", bin));
    assert!(!is_locked_singbox("", bin));
    assert!(!is_locked_singbox(bin, ""));
}

// ===== filepath_base（对照 winproc.go:248-254）=====

#[test]
fn filepath_base_matches_go_source() {
    assert_eq!(filepath_base(r"C:\dir\file.exe"), "file.exe");
    assert_eq!(filepath_base(r"C:\dir\file.exe\"), "file.exe"); // 去尾分隔符
    assert_eq!(filepath_base("file.exe"), "file.exe");
    assert_eq!(filepath_base("/usr/bin/sing-box"), "sing-box");
    assert_eq!(filepath_base(r"a/b\c"), "c"); // 混合分隔符
}

// ===== filepath_dir（起核子进程 CWD 的推导）=====

#[test]
fn filepath_dir_takes_parent_of_windows_paths() {
    // 生产形态：cfg 在 conf_dir 里（cfg_allowed 已保证），父目录就是要设的 CWD。
    assert_eq!(
        filepath_dir(r"C:\ProgramData\Polaris\config.json"),
        Some(r"C:\ProgramData\Polaris")
    );
    assert_eq!(
        filepath_dir("/etc/polaris/config.json"),
        Some("/etc/polaris")
    );
    assert_eq!(filepath_dir(r"a/b\c.json"), Some("a/b")); // 混合分隔符
}

#[test]
fn filepath_dir_keeps_the_separator_at_a_root() {
    // `"C:"` 在 Win32 = 「C 盘的当前目录」，不是 C 盘根 —— 尾分隔符丢不得。
    assert_eq!(filepath_dir(r"C:\config.json"), Some(r"C:\"));
    assert_eq!(filepath_dir(r"\config.json"), Some(r"\"));
    assert_eq!(filepath_dir("/config.json"), Some("/"));
}

#[test]
fn filepath_dir_returns_none_without_a_separator() {
    // 无父目录 → 调用方保持继承父进程 CWD 的旧行为，而不是拿一个空串去 chdir。
    assert_eq!(filepath_dir("config.json"), None);
    assert_eq!(filepath_dir(""), None);
}

#[test]
fn filepath_dir_does_not_split_multibyte_characters() {
    // 分隔符前一字节按 b':' 比对：中文目录名的 UTF-8 续字节恒 ≥ 0x80，不得被误判成盘符根。
    assert_eq!(
        filepath_dir(r"C:\用户\配置\config.json"),
        Some(r"C:\用户\配置")
    );
}

// ===== local_port_from_net_order（对照 winproc.go:294-298）=====

#[test]
fn local_port_from_net_order_known_ports() {
    // 端口 9090 = 0x2382。网络序首字节=0x23（存进 32 位低字节），次字节=0x82。
    // GetExtendedTcpTable 的 LocalPort u32 低 16 位 = 0x8223（小端：低字节0x23在前? 需对照 Go 源注释）。
    // Go 源：b0 = byte(p)（低字节）作网络序高位，b1 = byte(p>>8) 作网络序低位。
    // 故若想还原端口 9090（0x2382）：网络序两字节 [0x23, 0x82]，存进 u32 使 byte(p)=0x23 → p=0x23 + 0x82<<8=0x8223。
    let p: u32 = 0x8223; // byte(p)=0x23（网络序高位）, byte(p>>8)=0x82
    assert_eq!(local_port_from_net_order(p), 9090);
    // 端口 80 = 0x0050 → 网络序 [0x00, 0x50] → u32 = 0x5000
    assert_eq!(local_port_from_net_order(0x5000), 80);
    // 端口 443 = 0x01BB → 网络序 [0x01, 0xBB] → u32 = 0xBB01
    assert_eq!(local_port_from_net_order(0xBB01), 443);
}

// ===== filter_listen_pids（对照 winproc.go collect 闭包过滤逻辑）=====

#[test]
fn filter_listen_pids_dedupes() {
    let rows = vec![
        ListenEntry {
            pid: 100,
            port: 9090,
        },
        ListenEntry {
            pid: 200,
            port: 9090,
        },
        ListenEntry {
            pid: 100,
            port: 9090,
        }, // 重复 pid → 去重
        ListenEntry { pid: 300, port: 80 }, // 非目标端口
    ];
    assert_eq!(filter_listen_pids(&rows, 9090), vec![100, 200]);
    assert!(filter_listen_pids(&rows, 9999).is_empty());
}

// ===== normalize_path =====

#[test]
fn normalize_path_collapses_separators() {
    assert_eq!(normalize_path(r"C:\\dir\\file"), r"C:\dir\file");
    assert_eq!(normalize_path(r"C:/dir/file"), r"C:\dir\file");
    assert_eq!(normalize_path(r"C:\dir\file\\"), r"C:\dir\file");
    // 根保留（C:\ 不被去成 C:）
    assert_eq!(normalize_path(r"C:\\"), r"C:\");
}

/// 首实例带 `FILE_FLAG_FIRST_PIPE_INSTANCE`，后续实例不带 —— 位值层面。
#[test]
fn only_the_first_pipe_instance_claims_the_name() {
    const FIRST_BIT: u32 = 0x0008_0000;
    const DUPLEX: u32 = 0x0000_0003;
    assert_eq!(
        pipe_open_mode(true) & FIRST_BIT,
        FIRST_BIT,
        "首实例必须带独占 flag"
    );
    assert_eq!(
        pipe_open_mode(false) & FIRST_BIT,
        0,
        "后续实例带了独占 flag 就恒 ERROR_ACCESS_DENIED"
    );
    // 两条都必须保住双向：只读句柄写不了响应（W1 修过的老坑）。
    assert_eq!(pipe_open_mode(true) & DUPLEX, DUPLEX);
    assert_eq!(pipe_open_mode(false) & DUPLEX, DUPLEX);
    // 正向对照：两者确实不同，否则上面几条可能被一个「恒相同」的实现同时满足。
    assert_ne!(pipe_open_mode(true), pipe_open_mode(false));
}

/// accept 循环真的把 `first` 接进去了 —— 纯函数测不到接线。
///
/// `service/win.rs` 整模块 `cfg(windows)`，Linux 上不编译，但 `crate_source!` 读的是磁盘文本，
/// 不受 cfg 影响 ⇒ 这道门在本机也能跑。
#[test]
fn the_accept_loop_only_claims_the_name_once() {
    let src = polaris_source_probe::crate_source!("platform/windows/service/win.rs");
    let at = src.find("fn serve<").expect("serve 消失，门失去判据");
    let end = src[at..]
        .find("\nfn create_pipe_instance")
        .map_or(src.len(), |i| at + i);
    let body = &src[at..end];
    assert!(
        body.contains("create_pipe_instance(&pipe_name_w, &sddl_w, first)"),
        "accept 循环没有把 first 传给 create_pipe_instance"
    );
    assert!(body.contains("let mut first = true;"), "缺少首实例标志");
    let clear = body
        .find("first = false;")
        .expect("first 从未被清掉 —— 每个实例都会带独占 flag");
    let connect = body
        .find("connect_pipe(h)")
        .expect("connect_pipe 调用点消失");
    assert!(
        clear < connect,
        "first 必须在 connect_pipe **之前**清掉：否则连接失败那条腿回头重建时仍带独占 flag"
    );
    // 判据自检：窗口里必须真有 CreateNamedPipe 之外的循环体，否则上面几条可能落在空串上。
    assert!(body.contains("STOP_REQUESTED"), "窗口没盖住 accept 循环");
}

/// 响应必须在断开命名管道前刷到 client。
///
/// Windows 的同步 `WriteFile` 只把响应放进内核缓冲；紧跟 `DisconnectNamedPipe` 会丢弃 client
/// 尚未读走的字节，表现为同包 app/helper 仍稳定报 ERROR_PIPE_NOT_CONNECTED(233)。服务模块仅在
/// Windows 编译，这里用源码契约让 Linux 本地门也能守住 `write → flush → disconnect` 次序。
#[test]
fn response_is_flushed_before_pipe_disconnect() {
    let src = polaris_source_probe::crate_source!("platform/windows/service/win.rs");
    let write = src
        .find("let ok = WriteFile(")
        .expect("响应 WriteFile 消失");
    let flush = src[write..]
        .find("FlushFileBuffers(h)")
        .map(|offset| write + offset)
        .expect("响应未 FlushFileBuffers，disconnect 会丢未读字节");
    let disconnect = src[flush..]
        .find("DisconnectNamedPipe(h)")
        .map(|offset| flush + offset)
        .expect("命名管道清理出口消失");

    assert!(write < flush, "必须先写响应再 flush");
    assert!(flush < disconnect, "必须先 flush 再断开管道");
}

/// SCM STOP 必须能**打断**阻塞中的 `ConnectNamedPipe`，而不只是置个标志。
///
/// 与 [`the_accept_loop_only_claims_the_name_once`] 同款：`service/win.rs` 整模块
/// `cfg(windows)` 不参与 Linux 编译，但 `crate_source!` 读磁盘文本不受 cfg 影响。
/// 这条只能是源码级 —— 行为要真机（组 E）验，编译由 CI 的 windows 交叉 check 兜。
#[test]
fn scm_stop_wakes_the_blocked_accept_loop() {
    let src = polaris_source_probe::crate_source!("platform/windows/service/win.rs");

    // ① ctrl_handler 的 STOP 分支：置标志之外必须唤醒 + 上报中间态。
    let at = src
        .find("extern \"system\" fn ctrl_handler(")
        .expect("ctrl_handler 消失");
    let end = src[at..]
        .find("\n/// 唤醒阻塞在")
        .map_or(src.len(), |i| at + i);
    let body = &src[at..end];
    assert!(
        body.contains("STOP_REQUESTED.store(true"),
        "STOP 分支不再置停止标志"
    );
    assert!(
        body.contains("wake_accept_loop()"),
        "STOP 只置了标志没唤醒 —— accept 循环仍阻塞在 ConnectNamedPipe，要等下一个客户端才退"
    );
    assert!(
        body.contains("SERVICE_STOP_PENDING"),
        "STOP 不上报中间态 —— SCM 记录仍是 RUNNING，`sc stop` 直接返回成功而进程还在跑"
    );

    // ② 唤醒实现必须真去连自己的管道（不是空函数 / 不是 sleep）。
    let wat = src
        .find("fn wake_accept_loop()")
        .expect("wake_accept_loop 消失");
    let wend = src[wat..].find("\n}").map_or(src.len(), |i| wat + i);
    let wbody = &src[wat..wend];
    assert!(wbody.contains("CreateFileW("), "唤醒没有真发起连接");
    assert!(wbody.contains("PIPE_NAME"), "唤醒连的不是本服务的管道名");
    assert!(
        wbody.contains("OPEN_EXISTING"),
        "应当只连已存在的管道，不得创建"
    );

    // ③ 次序：STATUS_HANDLE 必须在进 serve **之前**存好，否则 ctrl_handler 读到 0、
    //    上报那步静默跳过。
    let store = src
        .find("STATUS_HANDLE.store(")
        .expect("STATUS_HANDLE 没有被写入，ctrl_handler 永远拿不到句柄");
    let serve_at = src
        .find("let _ = serve(helper.clone());")
        .expect("serve 调用点消失");
    assert!(store < serve_at, "STATUS_HANDLE 存得比 serve 还晚");
}

/// 写响应这条腿必须有取消守卫，且**未鉴权对端的响应不等对端**。
///
/// `FlushFileBuffers` 在命名管道服务端按定义会一直等到 client 把数据读走 ⇒ 「连上、发一帧、不读」
/// 即可无限期钉住一个 SYSTEM 服务线程 + 一个管道 HANDLE；此前该腿既无守卫、又在鉴权失败/未知命令
/// 分支同样执行（读腿早有 `IoTimeoutGuard`，同连接内两腿不对齐）。服务模块仅在 Windows 编译，
/// 故这里以源码契约在 Linux 本地守住；行为面属 Windows 真机项。
#[test]
fn response_write_is_bounded_and_unauthenticated_replies_never_wait_for_the_peer() {
    let src = polaris_source_probe::crate_source!("platform/windows/service/win.rs");
    // 取材自检：拿到的确实是本文件，且两个切点各唯一。
    assert!(src.contains("fn create_pipe_instance("), "取材面错位");
    assert_eq!(src.matches("fn write_response(").count(), 1);
    assert_eq!(src.matches("fn handle_connection<").count(), 1);

    // ① write_response 本体：有取消守卫 + flush 受 mode 约束（不再无条件 flush）。
    let wat = src.find("fn write_response(").expect("write_response 消失");
    let wend = src[wat..]
        .find("\n/// 断开并关闭管道实例")
        .map_or(src.len(), |i| wat + i);
    let wbody = &src[wat..wend];
    assert!(
        wbody.contains("IoTimeoutGuard::arm("),
        "写路径无取消守卫：FlushFileBuffers 可被不读数据的对端无限期钉住"
    );
    assert!(
        wbody.contains("mode == FlushMode::WaitPeer"),
        "flush 仍是无条件执行 —— 未鉴权对端照样能让服务线程等它"
    );
    // 守卫必须在 WriteFile **之前**装上（装晚了那次同步 IO 已经挂住，没人再去 CancelIoEx）。
    let arm = wbody.find("IoTimeoutGuard::arm(").expect("守卫锚点");
    let write = wbody.find("let ok = WriteFile(").expect("WriteFile 锚点");
    assert!(arm < write, "守卫必须在同步 WriteFile 之前装上");

    // ② 调用点分档：未鉴权/未识别的三条响应一律 NoWait，正常响应 WaitPeer。
    let hat = src
        .find("fn handle_connection<")
        .expect("handle_connection 消失");
    let hend = src[hat..]
        .find("\n/// 读一个请求帧")
        .map_or(src.len(), |i| hat + i);
    let hbody = &src[hat..hend];
    assert!(hbody.contains("cleanup_pipe(h)"), "窗口没盖住连接处理");
    assert_eq!(
        hbody.matches("FlushMode::NoWait").count(),
        3,
        "未鉴权响应共 3 条（帧不合法 / 未知命令 / 鉴权失败），少一条就是留了个钉住点"
    );
    // 鉴权失败分支本身：切到下一 arm 之前，只能是 NoWait。
    let auth = hbody
        .find("HandleOutcome::AuthFailed")
        .expect("鉴权失败分支消失");
    let respond = hbody
        .find("HandleOutcome::Respond")
        .expect("正常响应分支消失");
    assert!(auth < respond, "分支顺序变了，下面的切片会取错");
    let auth_arm = &hbody[auth..respond];
    assert!(
        auth_arm.contains("FlushMode::NoWait"),
        "鉴权失败分支仍等对端读走响应"
    );
    assert!(
        !auth_arm.contains("FlushMode::WaitPeer"),
        "鉴权失败分支等对端"
    );
    // 反向对照：正常（已鉴权）响应必须仍然 flush —— 否则 DisconnectNamedPipe 丢字节，
    // 回到 ERROR_PIPE_NOT_CONNECTED(233) 那个老坑。
    assert!(
        hbody.contains("FlushMode::WaitPeer"),
        "已鉴权响应也不 flush 了 ⇒ client 稳定收 233"
    );
    // 反向对照：不得再有不带模式的裸调用。
    assert!(
        !hbody.contains("write_response(h, response_line.as_bytes());"),
        "仍有未分档的裸 write_response 调用"
    );
}

// ===== D4 flush-dns 命令构造与结果判据 =====

/// 命令 shape：System32 **绝对**路径 + `/flushdns` + `CREATE_NO_WINDOW`。
///
/// 三样都是承重的：绝对路径绕开缺 System32 的 PATH（裸命令报「不是内部或外部命令」，与权限无关）；
/// CREATE_NO_WINDOW 让 session-0 的 SYSTEM 服务不弹控制台窗。位值与 `windows-sys` 常量的一致性由
/// `winproc::win` 的编译期断言钉住（本机编不到那格，交叉门编得到）。
#[test]
fn flush_dns_command_shape() {
    let c = flush_dns_command(Some(r"C:\Windows"));
    assert_eq!(c.program, r"C:\Windows\System32\ipconfig.exe");
    assert_eq!(c.args, vec!["/flushdns"]);
    assert_eq!(c.creation_flags, CREATE_NO_WINDOW);
    assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
}

/// `%SystemRoot%` 非默认盘/带尾分隔符/缺失三种取材都要落在同一形态上。
#[test]
fn flush_dns_command_normalizes_the_system_root() {
    assert_eq!(
        flush_dns_command(Some(r"D:\Win11\")).program,
        r"D:\Win11\System32\ipconfig.exe"
    );
    // env 读不到（非 Windows 宿主 / 变量被清空）→ 回落 C:\Windows，绝不退化成裸 `ipconfig`。
    for missing in [None, Some(""), Some("   ")] {
        assert_eq!(
            flush_dns_command(missing).program,
            r"C:\Windows\System32\ipconfig.exe",
            "{missing:?}"
        );
    }
}

/// flush-dns 子进程的硬超时：取值有界、错误串说得出「是被掐的」。
///
/// 两条边界都钉（理由见 [`FLUSH_DNS_TIMEOUT_MS`] 的文档）：
/// - `< 5000`：必须小于 app 侧 `HELPER_FLUSH_TIMEOUT`，否则先到点的是 app，helper 这头仍扣着
///   线程与管道实例，本次加固等于白做；
/// - `> 0`：为 0 等于「不等，直接杀」，`ipconfig` 一次都跑不完。
///
/// 错误串必须与 [`flush_dns_result`] 的失败串**可区分** —— 前者是「Dnscache 挂住了」，
/// 后者是「刷缓存被拒」，两者的修法不同，折成一句等于让排查从这里开始瞎猜。
#[test]
fn flush_dns_timeout_is_bounded_and_says_it_was_killed() {
    // 两条边界都是常量比常量 ⇒ 写成**编译期**断言（clippy 的 `assertions_on_constants` 也正是
    // 让它落到 const 求值里）。比运行期 assert 更强：改坏了在 `cargo build` 就红，不必等跑到这条测试。
    const _: () = assert!(
        FLUSH_DNS_TIMEOUT_MS < 5_000,
        "必须小于 app 侧 5s 的 IPC 超时，否则先到点的是 app"
    );
    const _: () = assert!(FLUSH_DNS_TIMEOUT_MS > 0, "0 等于不等待，ipconfig 跑不完");
    let msg = flush_dns_timeout_error(FLUSH_DNS_TIMEOUT_MS);
    assert!(msg.contains("3000"), "错误串要带上等了多久：{msg}");
    assert!(msg.contains("强制终止"), "错误串没说清是被掐的：{msg}");
    let rejected = flush_dns_result(Some(1), "Could not flush the DNS Resolver Cache", "")
        .expect_err("非零退出必须是 Err");
    assert_ne!(msg, rejected, "超时串与「跑完了但被拒」不得同形");
    assert!(
        !rejected.contains("强制终止"),
        "跑完了的失败不该说成被掐：{rejected}"
    );
}

/// **接线门**（B2）：Windows flush-dns 生产腿必须走**有界等待**，不得退回裸 `.output()`。
///
/// `winproc/win.rs` 整块 `cfg(windows)`，Linux 上不编译 —— 但 `crate_source!` 读的是磁盘文本，
/// 不受 cfg 影响，与本文件里那几条 `service/win.rs` 的门同款。
///
/// 变异锁：把这条腿改回 `.output()` → 前两条转红（`.output()` 自己等到天荒地老，超时判据消失）；
/// 只 wait 不杀 → 第三条转红（子进程留成孤儿，句柄挂到 helper 退出）。
#[test]
fn windows_flush_dns_waits_with_a_bound_and_kills_on_timeout() {
    let src = polaris_source_probe::mask_comments(&polaris_source_probe::crate_source!(
        "platform/windows/winproc/win.rs"
    ));
    let at = src
        .find("    fn flush_dns(&self) -> Result<(), String> {")
        .expect("flush_dns 锚点消失，门失去判据");
    let end = src[at..]
        .find("\n    fn enable_ip_forwarding")
        .map_or(src.len(), |i| at + i);
    let body = &src[at..end];
    // 切点自检：窗口必须真盖住这条腿（否则下面几条可能落在一段不相干的文本上）。
    assert!(
        body.contains("flush_dns_command("),
        "切片没盖住 flush-dns 生产腿"
    );
    assert!(
        body.contains("WaitForSingleObject("),
        "flush-dns 没有上界 —— ipconfig 卡在 DNS Client 上时这条连接线程与管道实例被一起扣住"
    );
    assert!(
        body.contains("FLUSH_DNS_TIMEOUT_MS"),
        "超时值没走 logic 里那条判据 —— 判据与执行分家，改一边不动另一边"
    );
    assert!(
        body.contains("child.kill()") && body.contains("child.wait()"),
        "超时后必须杀 + 收割，否则 ipconfig 成孤儿、句柄挂到 helper 退出"
    );
    assert!(
        !body.contains(".output()"),
        "裸 .output() 无上界 —— 本条修的就是它"
    );
}

/// rc=0 → Ok；非零 → Err，且 **stdout 必须在错误串里**（ipconfig 的失败文字只在 stdout）。
#[test]
fn flush_dns_result_folds_exit_and_carries_stdout() {
    assert_eq!(
        flush_dns_result(Some(0), "Successfully flushed", ""),
        Ok(())
    );

    let err = flush_dns_result(
        Some(1),
        "Could not flush the DNS Resolver Cache: Function failed during execution.",
        "",
    )
    .expect_err("非零退出必须是 Err");
    assert!(
        err.contains("Could not flush the DNS Resolver Cache"),
        "{err}"
    );
    assert!(err.contains("exit 1"), "{err}");

    // stderr 也不丢（两条流都并进来，顺序 stdout 在前）。
    let both = flush_dns_result(Some(1), "out-text", "err-text").expect_err("非零退出必须是 Err");
    assert!(
        both.contains("out-text") && both.contains("err-text"),
        "{both}"
    );
    assert!(
        both.find("out-text") < both.find("err-text"),
        "stdout 应在前：{both}"
    );

    // 两条流都空 → 错误串仍要说得出发生了什么，不能是空串。
    let silent = flush_dns_result(Some(1), "", "   ").expect_err("非零退出必须是 Err");
    assert!(
        silent.contains("exit 1") && silent.contains("no output"),
        "{silent}"
    );

    // 被信号带走（无退出码）同样是失败，且不得伪装成 exit 0。
    let signaled = flush_dns_result(None, "", "").expect_err("无退出码必须是 Err");
    assert!(signaled.contains("terminated by signal"), "{signaled}");
}
