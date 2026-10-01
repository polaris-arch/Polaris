#[test]
fn windows_client_uses_native_synchronous_pipe_io() {
    let pipe = polaris_source_probe::crate_source!("windows_pipe.rs");
    let connector = polaris_source_probe::crate_source!("connector.rs");
    let production_connector = connector
        .split("\n#[cfg(test)]")
        .next()
        .expect("connector production source");
    for required in ["CreateFileW(", "WriteFile(", "ReadFile(", "OwnedHandle"] {
        assert!(
            pipe.contains(required),
            "missing native pipe anchor: {required}"
        );
    }
    assert!(pipe.contains("std::ptr::null_mut()"));
    assert!(!pipe.contains("ReadFileEx("));
    assert!(!pipe.contains("WriteFileEx("));
    assert!(!production_connector.contains("OpenOptions::new()"));
    assert!(production_connector.contains("WinPipeStream::connect"));
    assert!(production_connector.contains("ERROR_PIPE_BUSY"));
}

#[test]
fn windows_pipe_read_is_bounded_by_a_deadline() {
    // 复审 High（windows_pipe.rs:64）：裸同步 ReadFile 循环无 deadline、无取消腿 ⇒ helper 服务侧
    // 一挂住，app 主进程这次调用永不返回；而 trait 契约（transport.rs）承诺「受超时约束」。
    // cfg(windows) 代码在 Linux 宿主上没有运行期观察面（不编译），源码级门 + win-gnu clippy
    // 是这条腿在本机唯一能守住的两件事。
    let pipe = polaris_source_probe::crate_source!("windows_pipe.rs");
    // 取材自检①：锚点解析到的确实是这个文件（改名/搬家后不静默换对象）。
    assert!(
        pipe.contains("struct WinPipeStream"),
        "取材面错位：拿到的不是 windows_pipe.rs"
    );
    // 取材自检（原②：本文件无块注释 ⇒ 行注释剥离已足够，出现块注释即转红）已删：那道前置自证
    // 只在剥离实现只认行注释（旧的本地 strip_line_comments，朴素 split("//")）时才需要。现改用
    // 共享的 `mask_comments`（词法级剥注释，块注释同样处理，字符串字面量里的 `//` 不误判），
    // 该前提不再成立，判据由共享实现的文档持有。
    let code = polaris_source_probe::mask_comments(&pipe);
    // 取材自检②：剥离没有把生产代码一并吃掉。
    for anchor in ["CreateFileW(", "ReadFile(", "PeekNamedPipe("] {
        assert!(code.contains(anchor), "剥离后丢失生产锚点: {anchor}");
    }
    // 取材自检③：切点唯一，否则 nth(1) 切到的是另一段。
    assert_eq!(code.matches("fn read_until_timeout").count(), 1);
    assert_eq!(code.matches("fn set_read_timeout").count(), 1);
    assert_eq!(code.matches("fn write_all").count(), 1);

    let read_body = code
        .split("fn read_until_timeout")
        .nth(1)
        .and_then(|t| t.split("fn set_read_timeout").next())
        .expect("read_until_timeout 方法体");
    for required in [
        // deadline 挂在**可被调用方改写**的读预算上，不是写死常量
        "self.read_timeout",
        // 断言的是 deadline 的**执行力**（比较 + 返回超时），不是「出现过 deadline 这个词」：
        // 只把变量改名成 _deadline、把检查删掉的外科式变异同样必须转红。
        "Instant::now() >= deadline",
        // 非阻塞探可读量：同步 ReadFile 一旦发出就不可中断，只能读前先探
        "peek_available",
        "POLL_INTERVAL",
        "ErrorKind::TimedOut",
    ] {
        assert!(read_body.contains(required), "读路径缺超时锚点: {required}");
    }

    // ===== 位置判据：deadline 检查必须在 loop 顶（peek 之前）=====
    //
    // 只 contains 字符串分不清它守的是**哪条分支**：把检查塞回 `if avail == 0` 块内，上面那组
    // 断言仍然全绿，而「对端持续快写、不发 \n」的读就再也走不到检查点（整段读不返回 + buf 无界）。
    // 下面用两条互相独立的判据钉住位置 —— 切片（loop 顶到 peek 之间）与缩进层级（loop 体的直接
    // 层级，不是分支体的深一级），任一不成立即红；切点/唯一性先自检，取不到就 panic（fail-closed）。
    assert_eq!(
        read_body.matches("loop {").count(),
        1,
        "loop 切点不唯一，位置判据失去锚"
    );
    assert_eq!(
        read_body.matches("let avail").count(),
        1,
        "peek 切点不唯一，位置判据失去锚"
    );
    assert_eq!(
        read_body.matches("Instant::now() >= deadline").count(),
        1,
        "deadline 检查出现 {} 次：位置无法判定",
        read_body.matches("Instant::now() >= deadline").count()
    );
    // 判据①（切片）：`loop {` 与首次 peek 之间必须已经完成检查**并返回超时**。
    let loop_head = read_body
        .split_once("loop {")
        .and_then(|(_, rest)| rest.split_once("let avail"))
        .map(|(head, _)| head)
        .expect("loop 顶到 peek 之间的切片");
    assert!(
        loop_head.contains("Instant::now() >= deadline"),
        "deadline 检查不在 loop 顶（peek 之前）：对端持续快写但不发 \n 时永远走不到检查点"
    );
    assert!(
        loop_head.contains("ErrorKind::TimedOut"),
        "loop 顶只比较不返回：检查没有执行力"
    );
    // 判据②（缩进）：检查所在行 = loop 体的直接层级（rustfmt 固定 4 空格一级）。
    // 落进 `if avail == 0 {` 之类的条件块内会深一级，当场对不上。
    let indent_of = |needle: &str| -> usize {
        let line = read_body
            .lines()
            .find(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("取不到含 `{needle}` 的行"));
        line.len() - line.trim_start().len()
    };
    let loop_indent = indent_of("loop {");
    let check_indent = indent_of("Instant::now() >= deadline");
    assert_eq!(
        check_indent,
        loop_indent + 4,
        "deadline 检查缩进 {check_indent} ≠ loop 体层级 {}：它被塞进了某个条件分支内",
        loop_indent + 4
    );
    // 判据③（反向）：`avail == 0` 分支体内不得再持有 deadline 字样 —— 否则上面两条可被
    // 「两处都放一份」绕过，而真正决定行为的仍是分支内那份。
    let avail_zero_block = read_body
        .split_once("if avail == 0 {")
        .and_then(|(_, rest)| rest.split_once("continue;"))
        .map(|(block, _)| block)
        .expect("avail == 0 分支体");
    assert!(
        !avail_zero_block.contains("deadline"),
        "deadline 检查落在 avail == 0 分支内：快写不换行的对端绕过整段预算"
    );

    // 反向对照：同一套剥离 + 切片落在写路径上不成立 ⇒ 上面的绿不是「整文件恰好含这些词」。
    let write_body = code.split("fn write_all").nth(1).expect("write_all 方法体");
    assert!(
        !write_body.contains("deadline"),
        "判据不具区分力：写路径也命中 deadline"
    );
}

/// 读腿把 `ERROR_PIPE_NOT_CONNECTED`(233) 与 `ERROR_BROKEN_PIPE` 一并按 EOF 处理；写腿不归一。
///
/// 为什么要紧：旧 Windows helper 对不认识的命令会在验 token 前 NoWait 回 `ERR unknown` 并
/// `DisconnectNamedPipe`，那行常被丢掉，客户端下一次 peek/read 拿到 233。读腿不归一 ⇒ 它以
/// `ClientError::Io` 上抛，与「请求根本没写出去」同形，app 侧就分不出「连上了、写出了、0 字节」
/// 这一格（install-core 能力判定的唯一依据）。写腿若也归一，则「请求没送达」会被误读成「对端
/// 看过请求后一言不发」。cfg(windows) 代码本机不编译：源码门 + msvc 交叉 clippy 各守一半。
#[test]
fn windows_pipe_read_treats_pipe_not_connected_as_eof_but_write_does_not() {
    let pipe = polaris_source_probe::crate_source!("windows_pipe.rs");
    assert!(
        pipe.contains("struct WinPipeStream"),
        "取材面错位：拿到的不是 windows_pipe.rs"
    );
    let code = polaris_source_probe::mask_comments(&pipe);
    // 判据本体：「对端已走」集合含两个码。
    let at = code.find("fn is_peer_gone(").expect("is_peer_gone 消失");
    let end = code[at..]
        .find("\n}")
        .map(|i| at + i)
        .expect("is_peer_gone 体");
    let body = &code[at..end];
    for c in ["ERROR_BROKEN_PIPE", "ERROR_PIPE_NOT_CONNECTED"] {
        assert!(body.contains(c), "「对端已走」集合缺 {c}");
    }
    // 读腿两个出口（peek 与 ReadFile）都经它判。
    assert_eq!(code.matches("fn read_until_timeout").count(), 1);
    let read_body = code
        .split("fn read_until_timeout")
        .nth(1)
        .and_then(|t| t.split("fn set_read_timeout").next())
        .expect("read_until_timeout 方法体");
    assert_eq!(
        read_body.matches("is_peer_gone(").count(),
        2,
        "peek 与 ReadFile 两个出口必须都按「对端已走 = EOF」判"
    );
    // 读腿不得再有只认 BROKEN_PIPE 的旁路分支（改一处漏一处）。
    assert!(
        !read_body.contains("ERROR_BROKEN_PIPE"),
        "读腿里还有绕开 is_peer_gone 的裸 ERROR_BROKEN_PIPE 判断"
    );
    // 反向对照：写腿不归一。
    let write_body = code
        .split("fn write_all")
        .nth(1)
        .and_then(|t| t.split("fn shutdown").next())
        .expect("write_all 方法体");
    assert!(!write_body.contains("is_peer_gone"), "写腿也吞成 EOF 了");
    assert!(
        write_body.contains("last_os_error()"),
        "写腿失败不再如实上抛 IO 错误"
    );
}
