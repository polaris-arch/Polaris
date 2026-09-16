use super::*;

#[test]
fn err_roundtrip_no_detail() {
    // 对应 Go: fmt.Fprintln(conn, "ERR auth") —— helper.go:406
    let e = Error::new(ErrorCode::Auth);
    assert_eq!(e.to_wire_line(), "ERR auth");
    let parsed = Error::parse("ERR auth").unwrap();
    assert_eq!(parsed, e);
}

#[test]
fn err_roundtrip_with_detail() {
    // 对应 Go: fmt.Fprintf(conn, "ERR start %v\n", err) —— helper.go:552
    let e = Error::with_detail(ErrorCode::Start, "exit status 1");
    assert_eq!(e.to_wire_line(), "ERR start exit status 1");
    let parsed = Error::parse("ERR start exit status 1").unwrap();
    assert_eq!(parsed.code, ErrorCode::Start);
    assert_eq!(parsed.detail, "exit status 1");
}

#[test]
fn err_detail_trimmed_on_serialize() {
    // 对齐 Go strings.TrimSpace(string(out))：尾部空白不应进 wire
    let e = Error::with_detail(ErrorCode::Dscacheutil, "  flusing failed  \n");
    assert_eq!(e.to_wire_line(), "ERR dscacheutil flusing failed");
}

#[test]
fn err_parse_unknown_token_falls_back_to_other() {
    // install-core 的 OS 错误前缀（read-singbox/readdir 等）未锁死进枚举 → Other + detail 保留完整原文
    let parsed = Error::parse("ERR read-singbox open /tmp/x: no such file").unwrap();
    assert_eq!(parsed.code, ErrorCode::Other);
    // Other 的 detail 保留完整原文（含未知 token），保 round-trip 无损
    assert_eq!(parsed.detail, "read-singbox open /tmp/x: no such file");
    // round-trip：to_wire_line 应重建原行
    assert_eq!(
        parsed.to_wire_line(),
        "ERR read-singbox open /tmp/x: no such file"
    );
}

/// `coredir-acl-weakened` 是本批新增的 token，**旧 app 的 `from_wire_token` 不认识它**。
///
/// 钉住「不认识 ≠ 崩 / 丢信息」。旧 app 走的两步全在本文件里：
/// 1. [`ErrorCode::from_wire_token`] 的 `_ => Self::Other` 兜底；
/// 2. [`Error::parse`] 对 `Other` 把 **`ERR ` 之后的完整原文**（含 token）留在 `detail`。
///
/// 故本测试断言两件事：**(a)** 未知 token 的兜底确实无损（用一个本枚举不认识的同形 token 做
/// 正向对照 —— 本枚举现在认识 `coredir-acl-weakened` 了，拿它测不到旧路径）；**(b)** 新 code
/// 产出的 wire 行去掉 `ERR ` 前缀后，恰好就是旧 app 会拿到的那段 detail，且单行无换行
/// （行协议帧的硬约束）。
#[test]
fn coredir_acl_weakened_is_lossless_for_old_apps() {
    // (a) 未知 token 的兜底路径（= 旧 app 看到本 token 时走的那条）。
    let unknown = "ERR coredir-acl-weakened-v2 C:\\x\\core: owner=S-1-5-21-1-2-3-1001";
    let parsed = Error::parse(unknown).unwrap();
    assert_eq!(parsed.code, ErrorCode::Other);
    assert_eq!(
        parsed.detail, "coredir-acl-weakened-v2 C:\\x\\core: owner=S-1-5-21-1-2-3-1001",
        "Other 必须保留完整原文（含 token），否则旧 app 连「哪个路径」都拿不到"
    );
    assert_eq!(parsed.to_wire_line(), unknown, "往返有损");

    // (b) 新 code 的 wire 行形态 == 旧 app 会读到的 detail。
    let detail =
        "C:\\ProgramData\\Polaris\\core: 非特权 SID S-1-5-32-545 被授写类权 mask=0x001201bf";
    let line = Error::with_detail(ErrorCode::CoredirAclWeakened, detail).to_wire_line();
    assert_eq!(
        line.strip_prefix("ERR ").unwrap(),
        format!("coredir-acl-weakened {detail}"),
        "旧 app 的 detail 与本行原文不一致"
    );
    assert!(
        !line.contains('\n'),
        "detail 含换行会把一帧劈成两行：{line}"
    );
    // 新 app 侧：token 已识别，detail 只留尾部自由文本。
    let new_app = Error::parse(&line).unwrap();
    assert_eq!(new_app.code, ErrorCode::CoredirAclWeakened);
    assert_eq!(new_app.detail, detail);
}

#[test]
fn err_parse_non_err_returns_none() {
    assert!(Error::parse("OK pong uid=0 v9").is_none());
    assert!(Error::parse("").is_none());
}

#[test]
fn all_known_codes_roundtrip_through_wire_token() {
    // 锁住 wire token 不漂移 —— 改名 = 与已部署 helper 断协议
    let known = [
        ErrorCode::Auth,
        ErrorCode::Peercred,
        ErrorCode::Unauthorized,
        ErrorCode::Unknown,
        ErrorCode::NoConfig,
        ErrorCode::BadArgs,
        ErrorCode::ConfigPathDenied,
        ErrorCode::LogPathDenied,
        ErrorCode::CorePathDenied,
        ErrorCode::ConfigNotOwned,
        ErrorCode::CoreMissing,
        ErrorCode::IfaceDenied,
        ErrorCode::BadGateway,
        ErrorCode::BadPort,
        ErrorCode::BadMetric,
        ErrorCode::CoredirUnset,
        ErrorCode::HashMismatch,
        ErrorCode::Enum,
        ErrorCode::Start,
        ErrorCode::Dscacheutil,
        ErrorCode::SetMetric,
        // 本表是手写名单（不是从枚举派生），此前已静默漏掉三个已存在的 code —— 顺手补齐，
        // 否则新加的 `CoredirAclWeakened` 只是落进一份本来就不全的名单里。
        ErrorCode::Ipconfig,
        ErrorCode::ResolvedDns,
        ErrorCode::SystemProxy,
        ErrorCode::CoredirAclWeakened,
    ];
    for c in known {
        let tok = c.as_wire_token();
        assert_eq!(ErrorCode::from_wire_token(tok), c, "token {tok} mismatch");
        // 其它 -> 序列化为 "unknown"（兜底，调用方构造时通常已知具体 code）
        assert_eq!(ErrorCode::from_wire_token("read-singbox"), ErrorCode::Other);
        let _ = tok; // suppress unused in case of empty
    }
}
