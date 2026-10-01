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

/// 已知 code 名单 + 各自的 wire token 金标（**唯一**一处，宏同时展开成数组与穷举 `match`）。
///
/// 早先名单是手写数组，加变体时 `as_wire_token` 的穷举 `match` 会编译错逼人补 token，名单却不会红
/// ⇒ 曾静默漏掉 `Ipconfig`/`ResolvedDns`/`SystemProxy` 三个。现在同一份条目既生成
/// [`KNOWN_CODES`]，又生成**无 `_` 臂**的 [`golden_wire_token`]：新增变体而不来这里登记 ⇒
/// `match` 不穷举 ⇒ **编译错**（E0004）；而要让它编译过，唯一的写法就是往宏调用里加一条，
/// 数组也就跟着有了它 —— 名单与枚举全集在构造上绑死，不靠人记得同步两处。
///
/// token 用字面量写死，不从 `as_wire_token` 取：判据自己喂自己就锁不住「两侧一起改名」这种
/// 协议破坏（与已部署 helper 断协议）。
macro_rules! known_codes {
    ($($variant:ident => $token:literal,)*) => {
        const KNOWN_CODES: &[ErrorCode] = &[$(ErrorCode::$variant,)*];

        /// 金标 token；[`ErrorCode::Other`] 无自己的 token（序列化兜底为 `unknown`），返 `None`。
        fn golden_wire_token(code: ErrorCode) -> Option<&'static str> {
            match code {
                $(ErrorCode::$variant => Some($token),)*
                ErrorCode::Other => None,
            }
        }
    };
}

known_codes! {
    Auth => "auth",
    Peercred => "peercred",
    Unauthorized => "unauthorized",
    Unknown => "unknown",
    NoConfig => "no-config",
    BadArgs => "bad-args",
    ConfigPathDenied => "config-path-denied",
    LogPathDenied => "log-path-denied",
    CorePathDenied => "core-path-denied",
    ConfigNotOwned => "config-not-owned",
    CoreMissing => "core-missing",
    IfaceDenied => "iface-denied",
    BadGateway => "bad-gateway",
    BadPort => "bad-port",
    BadMetric => "bad-metric",
    CoredirUnset => "coredir-unset",
    HashMismatch => "hash-mismatch",
    Enum => "enum",
    Start => "start",
    Dscacheutil => "dscacheutil",
    Ipconfig => "ipconfig",
    ResolvedDns => "resolved-dns",
    SystemProxy => "system-proxy",
    SetMetric => "set-metric",
    CoredirAclWeakened => "coredir-acl-weakened",
}

#[test]
fn all_known_codes_roundtrip_through_wire_token() {
    // 锁住 wire token 不漂移 —— 改名 = 与已部署 helper 断协议
    for &c in KNOWN_CODES {
        let golden = golden_wire_token(c).expect("KNOWN_CODES 里不该有 Other");
        assert_eq!(c.as_wire_token(), golden, "{c:?} 的 wire token 漂移");
        assert_eq!(
            ErrorCode::from_wire_token(golden),
            c,
            "token {golden} 解析不回 {c:?}"
        );
    }
    // Other 不在名单里：它没有自己的 token，序列化兜底为 `unknown`，未知 token 解析回它。
    assert_eq!(golden_wire_token(ErrorCode::Other), None);
    assert_eq!(ErrorCode::Other.as_wire_token(), "unknown");
    assert_eq!(ErrorCode::from_wire_token("read-singbox"), ErrorCode::Other);
}
