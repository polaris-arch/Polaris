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
        .find("connect_pipe_until_stopped(h,")
        .expect("stop-aware connect 调用点消失");
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

/// SCM STOP must end admission without waiting for any client to connect.
/// The actual Windows service polls a nonblocking pipe; its native regressions
/// cover stop-before-accept, stop-between-polls and successful connection.
#[test]
fn scm_stop_is_observed_without_a_client_connection() {
    let src = polaris_source_probe::crate_source!("platform/windows/service/win.rs");
    let src = polaris_source_probe::mask_comments(&src);
    let handler = service_function(&src, "extern \"system\" fn ctrl_handler(");
    assert!(handler.contains("STOP_REQUESTED.store(true"));
    assert!(handler.contains("SERVICE_STOP_PENDING"));

    let serve = service_function(&src, "fn serve<");
    assert!(
        serve.contains("connect_pipe_until_stopped(h, || STOP_REQUESTED.load(Ordering::SeqCst))")
    );
    assert!(!serve.contains("connect_pipe(h)"));
    assert!(serve.contains("CloseHandle(h)"));

    let poll = service_function(&src, "fn connect_pipe_until_stopped(");
    let mode = poll
        .find("let mode = PIPE_NOWAIT | PIPE_READMODE_MESSAGE;")
        .unwrap();
    let apply = poll.find("SetNamedPipeHandleState(h, &mode,").unwrap();
    let stop = poll.find("if stopped()").unwrap();
    let connect = poll.find("ConnectNamedPipe(h,").unwrap();
    assert!(mode < apply && apply < stop && stop < connect);
    let connected = poll
        .split("if error == ERROR_PIPE_CONNECTED {")
        .nth(1)
        .unwrap();
    let second_stop = connected.find("if stopped()").unwrap();
    let blocking = connected
        .find("let blocking = PIPE_WAIT | PIPE_READMODE_MESSAGE;")
        .unwrap();
    assert!(second_stop < blocking);
    assert!(connected.contains("SetNamedPipeHandleState(h, &blocking,"));
    assert!(!poll.contains("PIPE_READMODE_BYTE"));
    let create = service_function(&src, "fn create_pipe_instance(");
    assert!(create.contains("PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT"));
    assert!(poll.contains("if error != ERROR_PIPE_LISTENING"));
    assert!(poll.contains("std::thread::sleep(std::time::Duration::from_millis(25))"));

    let store = src.find("STATUS_HANDLE.store(").unwrap();
    let serve_at = src.find("let _ = serve(helper.clone());").unwrap();
    assert!(store < serve_at, "STATUS_HANDLE must precede the listener");
}

// Windows-only service glue is compiled by the cross-target gate; the host tests
// above exercise native custody outcomes. These checks cover the final exit commit.
fn service_function<'a>(source: &'a str, anchor: &str) -> &'a str {
    assert_eq!(
        source.matches(anchor).count(),
        1,
        "service anchor: {anchor}"
    );
    let start = source.find(anchor).expect("service function missing");
    let masked = polaris_source_probe::mask_comments_and_strings(source);
    let open = start
        + masked[start..]
            .find('{')
            .expect("service function body missing");
    let mut depth = 0;
    for (offset, byte) in masked.as_bytes().iter().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=offset];
                }
            }
            _ => {}
        }
    }
    panic!("service function end missing: {anchor}");
}

#[test]
fn normal_service_and_console_exit_require_confirmed_custody_drain() {
    let source = polaris_source_probe::crate_source!("platform/windows/service/win.rs");
    let drain = service_function(&source, "fn drain_for_exit<");
    assert!(drain.contains("loop {"));
    assert!(drain.contains("match helper.reap_child_on_exit()"));
    assert!(drain.contains("Ok(()) => return"));
    let error = drain.split("Err(error) => {").nth(1).unwrap();
    assert!(error.contains("pending()"));
    assert!(error.contains("std::thread::sleep"));
    assert!(!error.contains("return"));
    assert!(!error.contains("SERVICE_STOPPED"));

    let service = service_function(&source, "extern \"system\" fn service_main_entry(");
    let retained = service.find("SERVICE_HELPER.set(helper.clone())").unwrap();
    let listener = service.find("serve(helper.clone())").unwrap();
    let confirmed = service.find("drain_for_exit(helper.as_ref()").unwrap();
    let stopped = service
        .find("set_status(status_handle, SERVICE_STOPPED")
        .unwrap();
    assert!(retained < listener && listener < confirmed && confirmed < stopped);
    assert!(service[confirmed..stopped].contains("SERVICE_STOP_PENDING"));

    let console = service_function(&source, "pub fn run_console(");
    let close = console
        .find("match helper_for_stop.reap_child_on_exit()")
        .unwrap();
    let stop = console.find("STOP_REQUESTED.store(true").unwrap();
    let failed = console.find("Err(error) => {").unwrap();
    assert!(close < stop && stop < failed);
    assert!(!console[failed..].contains("STOP_REQUESTED.store(true"));
    assert!(console.contains("drain_for_exit(helper.as_ref()"));

    let connection = service_function(&source, "fn handle_connection<");
    assert!(connection.contains("match helper.reap_child_on_exit()"));
    assert!(!connection.contains("std::process::exit"));
}

#[test]
fn scm_stop_fences_spawn_before_listener_stop_and_refreshes_pending_checkpoint() {
    let source = polaris_source_probe::crate_source!("platform/windows/service/win.rs");
    let control = service_function(&source, "extern \"system\" fn ctrl_handler(");
    let fence = control.find("helper.begin_shutdown()").unwrap();
    let stop = control.find("STOP_REQUESTED.store(true").unwrap();
    assert!(fence < stop);
    let status = service_function(&source, "fn set_status(");
    assert!(status.contains("if state == SERVICE_STOP_PENDING"));
    let compact_status = status.split_whitespace().collect::<String>();
    assert!(compact_status.contains(
        "status.dwCheckPoint=STOP_CHECKPOINT.fetch_add(1,Ordering::SeqCst).saturating_add(1)"
    ));
    assert!(status.contains("status.dwWaitHint = 30000"));
}

/// 写响应这条腿必须有取消守卫，且**未鉴权对端的响应不等对端**。
///
/// `FlushFileBuffers` 在命名管道服务端按定义会一直等到 client 把数据读走 ⇒ 「连上、发一帧、不读」
/// 即可无限期钉住一个 SYSTEM 服务线程 + 一个管道 HANDLE；此前该腿既无守卫、又在鉴权失败/未知命令
/// 分支同样执行（读腿早有 `IoTimeoutGuard`，同连接内两腿不对齐）。服务模块仅在 Windows 编译，
/// 故这里以源码契约在 Linux 本地守住；行为面属 Windows 真机项。档位**判据**（谁 NoWait、谁
/// WaitPeer）已搬进 `WinHelper::handle_frame`，由 `helper/tests/wire_gate.rs` 做行为门。
#[test]
fn response_write_is_bounded_and_unauthenticated_replies_never_wait_for_the_peer() {
    let src = polaris_source_probe::crate_source!("platform/windows/service/win.rs");
    // 取材自检：拿到的确实是本文件，且两个切点各唯一。
    assert!(src.contains("fn create_pipe_instance("), "取材面错位");
    assert_eq!(src.matches("fn write_response(").count(), 1);
    assert_eq!(src.matches("fn handle_connection<").count(), 1);

    // ① write_response 本体：有取消守卫 + flush 受 mode 约束（不再无条件 flush）。
    let wbody = service_function(&src, "fn write_response(");
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

    // ② 调用点分档：档位判据已搬到跨平台的 `WinHelper::handle_frame`（行为门见
    //    `windows/helper/tests/wire_gate.rs`：token 不对 / 帧不合法 ⇒ NoWait，已鉴权 ⇒ WaitPeer，
    //    含已鉴权而命令未知的 `ERR unknown`）。这里只钉住 service 层**照档位执行、不自行改档**。
    let hbody = service_function(&src, "fn handle_connection<");
    assert!(hbody.contains("helper.handle_frame_from_peer(&raw, Some(&peer))"));
    assert!(
        !hbody.contains("fn read_frame("),
        "guard borrowed the next function"
    );
    assert!(
        !hbody.contains("fn write_response("),
        "guard borrowed flush implementation"
    );
    assert!(hbody.contains("cleanup_pipe(h)"), "窗口没盖住连接处理");
    assert!(
        hbody.contains("write_response(h, reply.line.as_bytes(), reply.flush)"),
        "写回没有照 handle_frame 给的 flush 档位"
    );
    let code = polaris_source_probe::mask_comments_and_strings(hbody);
    assert!(
        !code.contains("FlushMode::"),
        "service 层自己挑了 flush 档位 —— 未鉴权对端的 NoWait 判据会被绕开"
    );
    // 反向对照：不得再有不带模式的裸调用。
    assert!(
        !hbody.contains("write_response(h, response_line.as_bytes());"),
        "仍有未分档的裸 write_response 调用"
    );
}

// Native DNS cache work must not recreate the console-process shutdown failure.
#[test]
fn windows_native_dns_wait_is_bounded_without_a_subprocess() {
    const {
        assert!(FLUSH_DNS_TIMEOUT_MS > 0 && FLUSH_DNS_TIMEOUT_MS < 5_000);
    }
    let source = polaris_source_probe::crate_source!("platform/windows/winproc/win.rs");
    let source = polaris_source_probe::mask_comments_and_strings(&source);
    let body = source
        .split("    fn flush_dns(")
        .nth(1)
        .unwrap()
        .split("    fn enable_ip_forwarding(")
        .next()
        .unwrap();
    assert!(body.contains("admission.launch(polaris_system_integration::windows_dns::begin_flush)"));
    assert!(body.contains("pending.wait(") && body.contains("FLUSH_DNS_TIMEOUT_MS"));
    for forbidden in [
        "Command::",
        ".spawn(",
        ".output(",
        "Stdio::",
        "WaitForSingleObject",
        "child.kill",
        "child.wait",
    ] {
        assert!(
            !body.contains(forbidden),
            "DNS cache work recreated a child: {forbidden}"
        );
    }
}

#[test]
fn parse_request_ping_status() {
    assert!(matches!(
        parse_request("ping", &[][..]),
        Some(Request::Ping)
    ));
    assert!(matches!(
        parse_request("status", &[][..]),
        Some(Request::Status)
    ));
}

/// stop 的受管 pid 身份行可选：有则解出，无（旧客户端）则 `None`。
///
/// 变异：把 `STOP` 分支退回不读身份行的 `Request::Stop { pid: None }` → 首条转红
/// （身份判据从此永远拿不到 want = 形同虚设）。
#[test]
fn parse_request_stop_reads_optional_pid() {
    assert_eq!(
        parse_request("stop", &["4242"]).unwrap(),
        Request::Stop { pid: Some(4242) }
    );
    assert_eq!(
        parse_request("stop", &[][..]).unwrap(),
        Request::Stop { pid: None },
        "旧客户端不发身份行 → None → 沿用「停当前受管核」"
    );
}

#[test]
fn parse_request_freeport() {
    let r = parse_request("freeport", &["9090"]).unwrap();
    assert_eq!(r, Request::FreePort { port: 9090 });
}

#[test]
fn parse_request_freeport_bad_port_returns_none() {
    assert!(parse_request("freeport", &["abc"]).is_none());
}

#[test]
fn parse_request_start_full() {
    let id = "a".repeat(64);
    let r = parse_request(
        "start-native-pinned-birth-safe",
        &[&id, "/c/cfg.json", "/l/log.txt", "1", "4242"],
    )
    .unwrap();
    let Request::NativeStartPinned(p) = r else {
        panic!()
    };
    assert_eq!(p.id.as_str(), id);
    assert_eq!(p.common.cfg, "/c/cfg.json");
    assert_eq!(p.common.log, "/l/log.txt");
    assert!(p.common.fwd);
    assert_eq!(p.common.parent_pid, Some(4242));
}

#[test]
fn parse_request_start_without_ppid() {
    let id = "a".repeat(64);
    let r = parse_request(
        "start-native-pinned-birth-safe",
        &[&id, "/c/cfg.json", "", "0"],
    )
    .unwrap();
    let Request::NativeStartPinned(p) = r else {
        panic!()
    };
    assert_eq!(p.id.as_str(), id);
    assert_eq!(p.common.parent_pid, None);
}

#[test]
fn parse_request_route_add() {
    let r = parse_request("route-add", &["polaris-tun0", "10.0.0.0/8,172.16.0.0/12"]).unwrap();
    let Request::RouteAdd(rp) = r else { panic!() };
    assert_eq!(rp.iface, "polaris-tun0");
    assert_eq!(rp.cidrs, vec!["10.0.0.0/8", "172.16.0.0/12"]);
}

#[test]
fn parse_request_uninstall_iface_metric() {
    assert!(matches!(
        parse_request("uninstall", &[][..]),
        Some(Request::Uninstall)
    ));
    let r = parse_request("iface-metric", &["polaris-tun0", "999"]).unwrap();
    let Request::IfaceMetric { iface, metric } = r else {
        panic!()
    };
    assert_eq!(iface, "polaris-tun0");
    assert_eq!(metric, 999);
}

#[test]
fn parse_request_unknown_returns_none() {
    assert!(parse_request("bogus", &[][..]).is_none());
}

/// D4：`flush-dns` 必须在解码侧被认出来（无参数行）。
///
/// 变异：删掉 `FLUSH_DNS` 那一格 → 本条转红。分派层实现了新分支却漏了解码，serve 循环会在
/// `parse_request` 返 `None` 时直接回 `ERR unknown`，新分支一辈子够不着。
#[test]
fn parse_request_flush_dns() {
    assert!(matches!(
        parse_request("flush-dns", &[][..]),
        Some(Request::FlushDns)
    ));
}

/// P4：`install-core` 必须在解码侧被认出来（src 行 + wantHash 行）。
///
/// 变异：删掉 `INSTALL_CORE` 那一格 → 本条转红（= 批三真机缺陷的形态：分派有、解码无 ⇒ 0 字节 / 233）。
#[test]
fn parse_request_retired_mutators_are_unknown() {
    for (command, args) in [
        ("install-core", vec!["/retired/source", "not-a-pin"]),
        ("start", vec!["cfg", "", "0"]),
        ("start-native-birth-safe", vec!["cfg", "", "0"]),
        ("start-reap-safe", vec!["binary", "cfg", "", "0"]),
        ("install-core", Vec::new()),
    ] {
        assert_eq!(parse_request(command, &args), None);
    }
}

/// 切行：行1 token、行2 命令、其余参数；`\r\n` 与 `\n` 同剥；凑不齐两行 ⇒ None。
#[test]
fn split_frame_cuts_token_command_and_args() {
    assert_eq!(
        split_frame("tok\r\nroute-add\r\npolaris-tun0\r\n10.0.0.0/8\r\n"),
        Some(Frame {
            token: "tok",
            command: "route-add",
            args: vec!["polaris-tun0", "10.0.0.0/8"],
        })
    );
    assert_eq!(
        split_frame("tok\nping\n"),
        Some(Frame {
            token: "tok",
            command: "ping",
            args: vec![],
        })
    );
    // 空参数行必须保留（install-core 缺 src 时是空行，不能被吞掉把 hash 顶到 src 位）。
    assert_eq!(
        split_frame("tok\ninstall-core\n\nHASH\n").map(|f| f.args),
        Some(vec!["", "HASH"])
    );
    assert_eq!(split_frame("only-token\n"), None);
    assert_eq!(split_frame(""), None);
}

#[test]
fn native_cache_command_has_its_own_strict_capability_name() {
    assert_eq!(
        parse_request("flush-dns-native", &[]),
        Some(Request::WindowsFlushDnsNative)
    );
    assert!(parse_request("flush-dns-native", &["unexpected"]).is_none());
    assert_eq!(
        Request::WindowsFlushDnsNative.command_name(),
        "flush-dns-native"
    );
    assert_eq!(Request::FlushDns.command_name(), "flush-dns");
}

#[test]
fn config_confinement_rejects_traversal_and_windows_aliases() {
    let base = r"C:\registered";
    for path in [
        r"C:\registered\..\outside.json",
        r"C:\registered\.\file.json",
        r"C:\registered\sub\..\file.json",
        r"C:\registered\file.json:stream",
        r"C:\registered\sub.\file.json",
        r"C:\registered\NUL.json",
        r"C:\registered\COM1",
        r"C:registered\file.json",
        r"\\?\C:\registered\file.json",
        r"\\server\share\file.json",
        r"\registered\file.json",
    ] {
        assert!(!cfg_allowed(path, base), "accepted alias {path}");
    }
    assert!(cfg_allowed(r"C:\registered\配置.json", base));
}
