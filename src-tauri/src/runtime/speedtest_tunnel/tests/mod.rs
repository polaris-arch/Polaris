use super::mock_proxy::*;
use super::*;
use std::time::Duration;

// ══════════════════════════════════════════════════════════════════════
//  纯逻辑：目标解析 + 报文编解码
// ══════════════════════════════════════════════════════════════════════

#[test]
fn parses_default_endpoint_and_derives_port_path_host_header() {
    let t =
        SpeedTestTarget::parse("http://www.gstatic.com/generate_204").expect("默认端点必可解析");
    assert!(!t.https);
    assert_eq!(t.host, "www.gstatic.com");
    assert_eq!(t.port, 80);
    assert_eq!(t.path, "/generate_204");
    assert_eq!(
        t.host_header, "www.gstatic.com",
        "标准端口的 Host 头不带端口"
    );
}

#[test]
fn https_default_port_and_non_standard_port_shape() {
    let t = SpeedTestTarget::parse("https://example.com/x?a=1").expect("https 可解析");
    assert!(t.https);
    assert_eq!(t.port, 443);
    assert_eq!(t.path, "/x?a=1", "query 必须保留（自配端点常带鉴权参数）");
    assert_eq!(t.host_header, "example.com");

    let t = SpeedTestTarget::parse("https://example.com:8443/p").expect("非标端口可解析");
    assert_eq!(t.port, 8443);
    assert_eq!(
        t.host_header, "example.com:8443",
        "非标端口的 Host 头必须带端口"
    );
}

#[test]
fn rejects_non_http_schemes_and_hostless_urls() {
    for bad in [
        "socks5://1.2.3.4:1080",
        "ftp://example.com/x",
        "http://",
        "not a url",
        "",
    ] {
        assert!(
            SpeedTestTarget::parse(bad).is_none(),
            "`{bad}` 不该被当成合法测速目标"
        );
    }
}

/// 🔴 CONNECT 的 authority-form **标准端口也必须显式带 port**。
///
/// 牙：把 [`SpeedTestTarget::authority`] 改成「标准端口省略端口」→ 本测转红。
#[test]
fn connect_request_always_carries_an_explicit_port() {
    let t = SpeedTestTarget::parse("http://www.gstatic.com/generate_204").unwrap();
    assert_eq!(
        t.connect_request(None),
        "CONNECT www.gstatic.com:80 HTTP/1.1\r\nHost: www.gstatic.com:80\r\n\r\n"
    );
    let t = SpeedTestTarget::parse("https://example.com/x").unwrap();
    assert!(t
        .connect_request(None)
        .starts_with("CONNECT example.com:443 HTTP/1.1\r\n"));
}

/// 🔴 **α 批**：带凭据的 CONNECT 必须携带 `Proxy-Authorization: Basic base64(user:pass)`，且凭据
/// **只**出现在 CONNECT 这一步（隧道建成后对面是 origin，逐跳头不得带过去）。
///
/// Android 上 `probe-in-k` 要求本次起核的一次性凭据；漏发 ⇒ 内核回 407 ⇒ 整个测速池一律 -1。
/// 牙：删掉 `connect_request` 里的 `proxy_authorization_line` → 正面断言转红。
#[test]
fn connect_request_presents_the_loopback_credential_only_on_connect() {
    let user = polaris_config_engine::singbox::InboundUser {
        username: "polaris".into(),
        password: "0123456789abcdef0123456789abcdef".into(),
    };
    let t = SpeedTestTarget::parse("https://example.com/generate_204").unwrap();
    let expected = format!(
        "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\nProxy-Authorization: Basic {}\r\n\r\n",
        crate::runtime::mesh::base64_encode(b"polaris:0123456789abcdef0123456789abcdef")
    );
    assert_eq!(t.connect_request(Some(&user)), expected);
    assert!(
        !t.get_request()
            .to_ascii_lowercase()
            .contains("proxy-authorization"),
        "凭据不得跟进隧道内的 GET"
    );
    // 反向对照：无凭据 ⇒ 与改动前逐字节相同（桌面形态不变）。
    assert_eq!(
        t.connect_request(None),
        "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n"
    );
}

/// 🔴 隧道上的 GET 必须是 **origin-form**（absolute-form 是改前那条错路的形态）。
#[test]
fn tunnel_get_is_origin_form_not_absolute_form() {
    let t = SpeedTestTarget::parse("http://www.gstatic.com/generate_204").unwrap();
    let req = t.get_request();
    assert!(
        req.starts_with("GET /generate_204 HTTP/1.1\r\n"),
        "隧道内应发 origin-form，实得: {req:?}"
    );
    assert!(
        !req.contains("GET http://"),
        "绝不能退回 absolute-form（那是打代理的形态，打 origin 会被判 400）"
    );
    assert!(req.contains("\r\nHost: www.gstatic.com\r\n"));
}

#[test]
fn parses_status_codes_and_rejects_malformed_status_lines() {
    assert_eq!(
        parse_http_status_code(b"HTTP/1.1 204 No Content\r\n\r\n"),
        Some(204)
    );
    assert_eq!(
        parse_http_status_code(b"HTTP/1.0 200 OK\r\n\r\n"),
        Some(200)
    );
    assert_eq!(
        parse_http_status_code(b"HTTP/2 403 Forbidden\r\n\r\n"),
        Some(403)
    );
    // 畸形一律 None —— 绝不当成功（否则错误页会被记成一个漂亮的 TTFB）。
    for bad in [
        &b"NOTHTTP/1.1 200 OK\r\n\r\n"[..],
        &b"HTTP/1.1 2000 X\r\n\r\n"[..],
        &b"HTTP/1.1  \r\n\r\n"[..],
        &b"HTTP/1.1\r\n200\r\n\r\n"[..],
        &b"HTTP/x.y 200 OK\r\n\r\n"[..],
    ] {
        assert_eq!(
            parse_http_status_code(bad),
            None,
            "畸形状态行必须判失败: {bad:?}"
        );
    }
}

#[test]
fn only_2xx_is_acceptable() {
    assert!(is_acceptable_status(200) && is_acceptable_status(204) && is_acceptable_status(299));
    for code in [199u16, 300, 301, 403, 502] {
        assert!(!is_acceptable_status(code), "{code} 不该被当成功");
    }
}

/// 🔴 第二次响应必须从 `HTTP/` 锚点起判「收齐」—— 不锚定会把第一次响应的 body 残余
/// 当成第二次的响应头，值塌成 ≈0ms。
///
/// 牙：把 [`find_response_head`] 里的锚点去掉、改成直接找 `\r\n\r\n` → 本测转红。
#[test]
fn response_head_scan_anchors_on_the_status_line() {
    // 缓冲里先是上一次响应的 body 残余（含空行！），之后才是本次的响应头。
    let buf = b"leftover body\r\n\r\nHTTP/1.1 204 No Content\r\nX: 1\r\n\r\n";
    let head = find_response_head(buf).expect("锚定后应能找到本次响应头");
    assert!(
        head.starts_with(b"HTTP/1.1 204"),
        "实得: {:?}",
        String::from_utf8_lossy(head)
    );
    assert_eq!(parse_http_status_code(head), Some(204));

    // 头未收齐时必须继续等（返回 None），不能拿半截头去判状态码。
    assert!(find_response_head(b"HTTP/1.1 204 No Content\r\nX: 1\r\n").is_none());
    assert!(find_response_head(b"garbage without a status line\r\n\r\n").is_none());
}

fn http_target() -> SpeedTestTarget {
    SpeedTestTarget::parse("http://www.gstatic.com/generate_204").unwrap()
}

/// 走完整条腿（建隧道 + 两次 GET，第二次计时）—— 与生产同一条代码路径，只是预算可注入。
///
/// 生产是**两段**预算（冷建链 / 复用请求，见 `commands::speedtest::measure_warm_ttfb`）；本模块的门
/// 测的是**线级报文形态与 socket 生命周期**，不测分段边界（那条在 `commands::speedtest` 的假时钟门里
/// ——真 socket 与假时钟不能共存）。故这里两段注入同一个值，语义等价于原来的单一 `total`。
async fn measure(port: u16, target: &SpeedTestTarget, budget: Duration) -> Option<u32> {
    measure_detailed(port, target, budget).await.ok()
}

/// 同 [`measure`]，但保留失败的阶段与成因。
async fn measure_detailed(
    port: u16,
    target: &SpeedTestTarget,
    budget: Duration,
) -> crate::commands::speedtest::Measured {
    crate::commands::speedtest::measure_warm_ttfb(
        budget,
        budget,
        open_tunnel(port, None, target, &|_| {}),
    )
    .await
}

/// 🔴 **结构事实门**：本腿说的是 CONNECT + origin-form GET，不是 absolute-form。
///
/// 牙：把 [`open_tunnel`] 换回「经 reqwest 本机代理发 absolute-form」→ mock 代理收到的首个请求行
/// 不再是 `CONNECT ...`（而是 `GET http://... HTTP/1.1`）→ 本测转红。把 [`SpeedTestTarget::get_request`]
/// 改回 absolute-form → 第二/三条断言转红。
#[tokio::test]
async fn tunnel_speaks_connect_then_origin_form_gets() {
    let (port, observed) = spawn_mock_proxy(Script {
        connect_reply: Some(OK_204),
        gets: vec![GetReply::ok(), GetReply::ok()],
    })
    .await;
    let out = measure(port, &http_target(), Duration::from_secs(5)).await;
    assert!(out.is_some(), "mock 代理按脚本回 204，应出值");

    let lines = observed.lock().unwrap().request_lines.clone();
    assert_eq!(
        lines.first().map(String::as_str),
        Some("CONNECT www.gstatic.com:80 HTTP/1.1"),
        "首个请求行必须是带显式端口的 CONNECT，实得 {lines:?}"
    );
    assert_eq!(lines.len(), 3, "CONNECT + 两次 GET，实得 {lines:?}");
    for line in &lines[1..] {
        assert_eq!(
            line, "GET /generate_204 HTTP/1.1",
            "隧道内必须是 origin-form，实得 {line:?}"
        );
    }
}

/// 🔴 **计的是第二次 GET**：把 500ms 延迟放在**第一次**应答上，测得值必须远小于它。
///
/// 牙：把计时核改成「量第一次 GET」→ 测得值 ≈500ms → 本测转红。
#[tokio::test]
async fn measures_the_second_get_not_the_first() {
    let (port, _) = spawn_mock_proxy(Script {
        connect_reply: Some(OK_204),
        gets: vec![GetReply::delayed(500), GetReply::ok()],
    })
    .await;
    let out = measure(port, &http_target(), Duration::from_secs(5))
        .await
        .expect("两次应答都正常，应出值");
    assert!(
        out < 250,
        "第一次（暖身）承担了 500ms 延迟，measured 只应量第二次的往返；实得 {out}ms"
    );
}

/// 🔴 **非 2xx 一律判失败**（隧道建立与目标响应两处各一条）。
///
/// 牙：把 [`is_acceptable_status`] 放宽成「有状态码就算成功」→ 两条断言全红。
#[tokio::test]
async fn non_2xx_is_never_counted_as_success() {
    // ① CONNECT 被拒（502 = 出站拨不通）→ 隧道建不成 → None。
    let (port, _) = spawn_mock_proxy(Script {
        connect_reply: Some("HTTP/1.1 502 Bad Gateway\r\n\r\n"),
        gets: vec![],
    })
    .await;
    assert_eq!(
        measure(port, &http_target(), Duration::from_secs(5)).await,
        None,
        "CONNECT 非 2xx 必须判失败"
    );

    // ② 目标对 measured 那次回 403（CF-Workers 节点的典型形态）→ None。
    let (port, _) = spawn_mock_proxy(Script {
        connect_reply: Some(OK_204),
        gets: vec![
            GetReply::ok(),
            GetReply::raw("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n"),
        ],
    })
    .await;
    assert_eq!(
        measure(port, &http_target(), Duration::from_secs(5)).await,
        None,
        "measured 那次非 2xx 必须判失败（错误页绝不当成功记 TTFB）"
    );
}

/// 对端在响应头收齐前关闭（上游的 `early-close`）→ `None`，绝不伪造数值。
#[tokio::test]
async fn early_close_yields_none() {
    let (port, _) = spawn_mock_proxy(Script {
        connect_reply: Some(OK_204),
        gets: vec![GetReply::raw("HTTP/1.1 204 No")], // 半截响应头后 mock 退出 → 客户端读到 EOF
    })
    .await;
    assert_eq!(
        measure(port, &http_target(), Duration::from_secs(5)).await,
        None
    );
}

/// 🔴 **超时路径必须真把 socket 关掉**（而不是把它挂在一个还在跑的 tokio 任务里）。
///
/// mock 代理收到 CONNECT 后**什么都不回**；客户端应在注入的短总超时后放弃，且服务端随即读到
/// EOF —— 那就是 fd 已归还的线级证据。
///
/// 牙：把 [`open_tunnel`] 的 I/O 挪进 `tokio::spawn`（只 await join handle）→ 超时只丢 handle、
/// 任务与 socket 继续存活 → 服务端读不到 EOF → 本测转红。
#[tokio::test]
async fn timeout_closes_the_socket_not_leaks_it() {
    let (port, observed) = spawn_mock_proxy(Script {
        connect_reply: None,
        gets: vec![],
    })
    .await;
    let out = measure(port, &http_target(), Duration::from_millis(300)).await;
    assert_eq!(out, None, "隧道建不成必须返回 None（上层记 -1）");

    // 给 mock 一点时间观察到 EOF（客户端 drop 是同步的，这里只是让出调度）。
    for _ in 0..50 {
        if observed.lock().unwrap().saw_client_close {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        observed.lock().unwrap().saw_client_close,
        "总超时后客户端必须把隧道 socket 关掉（否则大订阅并发时 fd 会累积）"
    );
}

/// 🔴 **https 腿真的在隧道上做 TLS 握手**（不是悄悄退回明文）。
///
/// 不起真 TLS server：让 mock 代理接下 CONNECT 后把客户端发来的**首个数据块**抓下来，断言它是
/// 一条 TLS ClientHello 记录、且 SNI 里带目标主机名。握手随后当然会失败（对端不是 TLS server）
/// —— 不影响，本门只看已抓到的字节。**不触碰宿主网络。**
///
/// 牙：把 [`open_tunnel`] 里的 `target.https` 分支删掉（一律走明文）→ 首个数据块变成
/// `GET /x HTTP/1.1` → 本测转红。
#[tokio::test]
async fn https_target_performs_tls_handshake_inside_the_tunnel() {
    let (port, observed) = spawn_mock_proxy(Script {
        connect_reply: Some(OK_204),
        gets: vec![],
    })
    .await;
    let target = SpeedTestTarget::parse("https://speed.example.com/probe").unwrap();
    let out = measure(port, &target, Duration::from_millis(800)).await;
    assert_eq!(out, None, "对端不是真 TLS server，握手必失败 → None");

    let first = observed.lock().unwrap().first_tunnel_bytes.clone();
    assert!(
        first.len() > 5 && first[0] == 0x16 && first[1] == 0x03,
        "隧道内首个数据块应是 TLS handshake 记录（0x16 0x03..），实得 {:02x?}",
        &first[..first.len().min(16)]
    );
    assert!(
        find_subslice(&first, b"speed.example.com").is_some(),
        "ClientHello 应带 SNI = 目标主机名（否则真 https 端点会拿错证书/被拒）"
    );
}

/// 🔵 **边界门**：不校验证书的 verifier 只许留在本模块。
///
/// 泄漏到订阅拉取 / 内核下载 / 更新 / 解锁检测任何一条链上都是真实的安全降级（那些链路要读并
/// **信任**内容，而测速只看状态码与时刻）。独立函数只是让泄漏「需要显式复制一段代码」，本门让它
/// **直接转红**。
///
/// 牙：在 `src-tauri/src/` 任何其它文件里写 `with_custom_certificate_verifier(` → 本门转红。
#[test]
fn dangerous_tls_verifier_stays_in_this_module() {
    const NEEDLE: &str = "with_custom_certificate_verifier";
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let module_root = root.join("runtime").join("speedtest_tunnel");
    let mut offenders = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("读 src 目录") {
            let path = entry.expect("目录项").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            // 豁免面按**模块归属**判，不按文件名判：本模块 = `speedtest_tunnel.rs`
            // ∪ `speedtest_tunnel/` 目录下的一切（含本测试文件 `speedtest_tunnel/tests/mod.rs`）。
            // 旧写法 `ends_with("speedtest_tunnel.rs")` 在测试外移后失效 —— 本用例自身的源码
            // 里就写着这个针，文件名却变成了 `mod.rs`，于是门被自己的取材面绊倒。
            let owned_by_this_module =
                path.starts_with(&module_root) || path.ends_with("speedtest_tunnel.rs");
            if path.extension().is_some_and(|e| e == "rs")
                && !owned_by_this_module
                && std::fs::read_to_string(&path)
                    .is_ok_and(|src| src.contains(&format!("{NEEDLE}(")))
            {
                offenders.push(path.display().to_string());
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "不校验证书的 TLS verifier 只允许出现在测速隧道模块（其它链路要信任内容）。越界:\n{}",
        offenders.join("\n")
    );
}

// ══════════════════════════════════════════════════════════════════════════
// 预热响应体的有界消费：定界判据（纯逻辑）+ 线级行为（回环 mock 代理）。
// ══════════════════════════════════════════════════════════════════════════

/// 响应体定界：没有响应体 / 定长 / chunked / 不可复用四种，头名与取值大小写不敏感。
#[test]
fn body_framing_follows_status_and_headers() {
    for (head, expected) in [
        ("HTTP/1.1 204 No Content\r\n\r\n", BodyFraming::Empty),
        (
            "HTTP/1.1 304 Not Modified\r\nETag: x\r\n\r\n",
            BodyFraming::Empty,
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n",
            BodyFraming::Length(5),
        ),
        (
            "HTTP/1.1 200 OK\r\ncontent-length:0\r\n\r\n",
            BodyFraming::Length(0),
        ),
        (
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, Chunked\r\n\r\n",
            BodyFraming::Chunked,
        ),
        // 没有可判定的长度：响应体只能靠关连接定界。
        (
            "HTTP/1.1 200 OK\r\nServer: x\r\n\r\n",
            BodyFraming::NotReusable,
        ),
        // 对端声明要关连接：有长度也不可复用，204 也一样。
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: Close\r\n\r\n",
            BodyFraming::NotReusable,
        ),
        (
            "HTTP/1.1 204 No Content\r\nConnection: keep-alive, close\r\n\r\n",
            BodyFraming::NotReusable,
        ),
        // 解析不出的长度不猜。
        (
            "HTTP/1.1 200 OK\r\nContent-Length: five\r\n\r\n",
            BodyFraming::NotReusable,
        ),
    ] {
        assert_eq!(body_framing(head.as_bytes()), expected, "{head:?}");
    }
}

/// chunked 扫描：收齐 / 还要再读 / 畸形或单块超界。
#[test]
fn chunked_scan_distinguishes_done_more_and_bad() {
    for (body, expected) in [
        ("5\r\nhello\r\n0\r\n\r\n", ChunkScan::Done),
        (
            "5;ext=1\r\nhello\r\n0\r\nTrailer: x\r\n\r\n",
            ChunkScan::Done,
        ),
        ("0\r\n\r\n", ChunkScan::Done),
        ("", ChunkScan::More),
        ("5\r\nhel", ChunkScan::More),
        ("5\r\nhello\r\n", ChunkScan::More),
        ("5\r\nhello\r\n0\r\n", ChunkScan::More), // 末块之后的空行还没到
        ("zz\r\n", ChunkScan::Bad),
        ("5\r\nhelloXX0\r\n\r\n", ChunkScan::Bad), // 块数据之后不是 CRLF
        ("10001\r\n", ChunkScan::Bad),             // 单块 65537 字节，超过上界
    ] {
        assert_eq!(scan_chunked(body.as_bytes()), expected, "{body:?}");
    }
}

fn warmup_failure(
    kind: crate::commands::speedtest::FailKind,
) -> crate::commands::speedtest::Measured {
    Err(crate::commands::speedtest::MeasureFailure::new(
        crate::commands::speedtest::FailPhase::Warmup,
        kind,
    ))
}

/// 预热响应带响应体（定长与 chunked 各一）：排干净之后再计时，照常出值。
/// 响应体里故意放一个完整的假响应头 —— 不排的话，第二次的判定会锚到它上面。
#[tokio::test]
async fn warmup_body_is_drained_before_the_measured_get() {
    for first in [
        "HTTP/1.1 200 OK\r\nContent-Length: 19\r\n\r\nHTTP/1.1 500 X\r\n\r\n.",
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n12\r\nHTTP/1.1 500 X\r\n\r\n\r\n0\r\n\r\n",
    ] {
        let (port, observed) = spawn_mock_proxy(Script {
            connect_reply: Some(OK_204),
            gets: vec![GetReply::raw(first), GetReply::ok()],
        })
        .await;
        let out = measure_detailed(port, &http_target(), Duration::from_secs(5)).await;
        assert!(out.is_ok(), "响应体可排干净时应出值，实得 {out:?}（{first:?}）");
        assert_eq!(observed.lock().unwrap().request_lines.len(), 3);
    }
}

/// 预热响应的**响应头与响应体分段到达**：排空的读循环必须把后到的那段读完，再发第二次 GET。
///
/// 后半段里放一个完整的假响应头，并且它到得比第二次 GET 的真响应早。不排（或没读完就返回）的话，
/// 第二次 GET 的判定会落在这个假头上，得到计时阶段的 500 —— 关掉排空，本测转红。
/// 三种切法：定长体整段后到；chunked 的块长行被读边界切开；chunked 的块数据被切开。
#[tokio::test]
async fn a_warmup_body_arriving_in_a_later_segment_is_drained() {
    for (head, tail) in [
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 19\r\n\r\n",
            "HTTP/1.1 500 X\r\n\r\n.",
        ),
        (
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1",
            "2\r\nHTTP/1.1 500 X\r\n\r\n\r\n0\r\n\r\n",
        ),
        (
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n12\r\nHT",
            "TP/1.1 500 X\r\n\r\n\r\n0\r\n\r\n",
        ),
    ] {
        let (port, observed) = spawn_mock_proxy(Script {
            connect_reply: Some(OK_204),
            gets: vec![GetReply::split(head, tail), GetReply::ok()],
        })
        .await;
        let out = measure_detailed(port, &http_target(), Duration::from_secs(5)).await;
        assert!(
            out.is_ok(),
            "后到的响应体应被排干净，实得 {out:?}（{head:?}）"
        );
        assert_eq!(observed.lock().unwrap().request_lines.len(), 3);
    }
}

/// 响应体没给够对端就关了连接：预热阶段的传输错，不发第二次 GET。
#[tokio::test]
async fn a_peer_closing_mid_body_is_a_warmup_transport_error() {
    let (port, observed) = spawn_mock_proxy(Script {
        connect_reply: Some(OK_204),
        gets: vec![
            GetReply::split("HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc", "def").hanging_up(),
        ],
    })
    .await;
    let out = measure_detailed(port, &http_target(), Duration::from_secs(5)).await;
    assert_eq!(
        out,
        warmup_failure(crate::commands::speedtest::FailKind::Transport)
    );
    assert_eq!(observed.lock().unwrap().request_lines.len(), 2);
}

/// 响应体迟迟不来：在冷段预算处以预热超时收口，不无限等。
#[tokio::test]
async fn a_body_that_never_arrives_times_out_in_the_warmup_phase() {
    let (port, observed) = spawn_mock_proxy(Script {
        connect_reply: Some(OK_204),
        gets: vec![GetReply::raw(
            "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc",
        )],
    })
    .await;
    let started = std::time::Instant::now();
    let out = measure_detailed(port, &http_target(), Duration::from_millis(300)).await;
    assert_eq!(
        out,
        warmup_failure(crate::commands::speedtest::FailKind::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(observed.lock().unwrap().request_lines.len(), 2);
}

/// 预热响应的连接不可复用（声明关连接 / 没有可判定的长度 / 超过上界）：判预热失败，
/// 且**不发第二次 GET**。
#[tokio::test]
async fn an_unreusable_warmup_response_fails_without_a_second_get() {
    for first in [
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
        "HTTP/1.1 200 OK\r\nServer: x\r\n\r\nbody until close",
        "HTTP/1.1 200 OK\r\nContent-Length: 65537\r\n\r\n",
    ] {
        let (port, observed) = spawn_mock_proxy(Script {
            connect_reply: Some(OK_204),
            gets: vec![GetReply::raw(first), GetReply::ok()],
        })
        .await;
        let out = measure_detailed(port, &http_target(), Duration::from_secs(5)).await;
        assert_eq!(
            out,
            warmup_failure(crate::commands::speedtest::FailKind::Rejected),
            "{first:?}"
        );
        assert_eq!(
            observed.lock().unwrap().request_lines.len(),
            2,
            "CONNECT + 一次 GET；预热失败后不得再发"
        );
    }
}

/// 预热那次回非 2xx：判预热失败并带出状态码，不发第二次 GET。
#[tokio::test]
async fn a_non_2xx_warmup_fails_with_its_status_on_the_wire() {
    let (port, observed) = spawn_mock_proxy(Script {
        connect_reply: Some(OK_204),
        gets: vec![
            GetReply::raw("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n"),
            GetReply::ok(),
        ],
    })
    .await;
    let out = measure_detailed(port, &http_target(), Duration::from_secs(5)).await;
    assert_eq!(
        out,
        warmup_failure(crate::commands::speedtest::FailKind::HttpStatus(403))
    );
    assert_eq!(observed.lock().unwrap().request_lines.len(), 2);
}

/// 源端口在 socket 连上代理口、**还没发出任何字节**时交给 `announce`：那一刻代理还没收到 CONNECT。
/// 交出去的就是代理一侧看到的对端端口，且只交一次。
///
/// 多线程运行时：`announce` 里那段阻塞等待期间，代理任务要能在另一个线程上跑 —— 单线程下它根本
/// 没机会读，「没看到请求行」就成了恒真。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_source_port_is_announced_before_any_byte_is_sent() {
    let (port, observed) = spawn_mock_proxy(Script {
        connect_reply: Some(OK_204),
        gets: vec![],
    })
    .await;
    let announced = std::sync::Mutex::new(Vec::new());
    let _tunnel = open_tunnel(port, None, &http_target(), &|source_port| {
        // 回环上给代理留足读到 CONNECT 的时间：字节若已发出，这里一定看得到请求行。
        std::thread::sleep(Duration::from_millis(50));
        let seen_requests = observed.lock().unwrap().request_lines.len();
        announced.lock().unwrap().push((source_port, seen_requests));
    })
    .await
    .expect("隧道建成");
    let peer_port = observed.lock().unwrap().peer_port.expect("代理接到了连接");
    assert_eq!(*announced.lock().unwrap(), vec![(peer_port, 0)]);
    assert_eq!(
        observed.lock().unwrap().request_lines.len(),
        1,
        "对照：登记之后 CONNECT 确实发出去了"
    );
}
