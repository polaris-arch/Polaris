use super::*;
use crate::traits::{MockFailOp, MockFs, StdFs};

// 已知内容的标准 SHA256（用 echo -n | sha256sum 验证）：
//   sha256(b"")     = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
//   sha256(b"hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const HELLO_SHA: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

#[test]
fn sha256_known_vectors() {
    assert_eq!(sha256_hex(b""), EMPTY_SHA);
    assert_eq!(sha256_hex(b"hello"), HELLO_SHA);
    // 大小写：hex::encode 恒输出小写。
    assert!(sha256_hex(b"hello")
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
}

#[test]
fn is_valid_sha256_hex_variants() {
    assert!(is_valid_sha256_hex(&"a".repeat(64)));
    assert!(is_valid_sha256_hex(&"ABCDEF0123456789".repeat(4))); // 大小写混用
    assert!(!is_valid_sha256_hex("abc")); // 太短
    assert!(!is_valid_sha256_hex(&"z".repeat(64))); // 非 hex
    assert!(!is_valid_sha256_hex(&"a".repeat(63))); // 63 字符
}

/// 「真算出来的摘要」喂进单点判据：匹配 / 大小写 / 不符。
#[test]
fn computed_digest_match_is_case_insensitive() {
    let actual = sha256_hex(b"hello");
    // 匹配（小写期望）。
    assert!(verify_hex_digest(&actual, HELLO_SHA).is_ok());
    // 匹配（大写期望——大小写不敏感）。
    assert!(verify_hex_digest(&actual, &HELLO_SHA.to_uppercase()).is_ok());
    // 不匹配：载荷须带齐期望值与实际值。
    assert_eq!(
        verify_hex_digest(&actual, EMPTY_SHA),
        Err(VerifyError::HashMismatch {
            expected: EMPTY_SHA.to_string(),
            actual: HELLO_SHA.to_string(),
        })
    );
}

#[test]
fn invalid_expected_hash_is_rejected_before_comparison() {
    let actual = sha256_hex(b"hello");
    // 非 64 字符 hex。
    assert_eq!(
        verify_hex_digest(&actual, "abc"),
        Err(VerifyError::InvalidExpectedHash(3))
    );
    // 非 hex（64 字符但含 z）。
    assert_eq!(
        verify_hex_digest(&actual, &"z".repeat(64)),
        Err(VerifyError::InvalidExpectedHash(64))
    );
}

/// 🟡 **摘要判定是单点，且两个变体必须可分辨（二者处置相反）。**
///
/// 生产的 `update_download` 腿此前手搓 `!is_valid_sha256_hex(..) || !eq_ignore_ascii_case(..)`，
/// 把「发布方 digest 写坏了」与「包被截断/篡改」压成一个 bool ⇒ 用户被引导去反复重下一个
/// 永远不会好的包。本条钉住：判据分变体，且流式入口与单点结论**逐字一致**。
///
/// **变异探针**：删掉 [`verify_hex_digest`] 的 `InvalidExpectedHash` 早退（非法 hex 落进
/// `eq_ignore_ascii_case` → 报成 HashMismatch）⇒ 第 1 条转红；把
/// [`Sha256Stream::verify`] 改回自己手搓比较 ⇒ 一致性断言转红。
#[test]
fn digest_verdict_is_single_sourced_and_splits_by_variant() {
    // 格式非法 ≠ 摘要不符。
    assert_eq!(
        verify_hex_digest(HELLO_SHA, "not-a-hash"),
        Err(VerifyError::InvalidExpectedHash(10))
    );
    assert!(matches!(
        verify_hex_digest(HELLO_SHA, EMPTY_SHA),
        Err(VerifyError::HashMismatch { .. })
    ));
    assert!(verify_hex_digest(HELLO_SHA, HELLO_SHA).is_ok());
    // 大小写不敏感（实际值侧与期望值侧都是）。
    assert!(verify_hex_digest(&HELLO_SHA.to_uppercase(), HELLO_SHA).is_ok());
    assert!(verify_hex_digest(HELLO_SHA, &HELLO_SHA.to_uppercase()).is_ok());

    // 流式入口与单点的结论（含错误变体与其载荷）必须逐字相同。
    for expected in [HELLO_SHA, EMPTY_SHA, "not-a-hash"] {
        let by_stream = {
            let mut s = Sha256Stream::new();
            s.update(b"hello");
            s.verify(expected)
        };
        let by_hex = verify_hex_digest(HELLO_SHA, expected);
        assert_eq!(
            by_stream, by_hex,
            "Sha256Stream::verify 与单点判据分叉了（expected={expected}）"
        );
    }
}

/// 增量 hash 与整包 hash **必须**给出同一个摘要，且与分片方式无关。
///
/// 这是流式腿不必把整包攒进内存再算摘要的全部依据：分片一变结论就变的话，
/// 「本地算出来的摘要」与「发布方公布的摘要」永远对不上，且只在真机大包上才暴露。
#[test]
fn incremental_sha256_equals_one_shot_for_any_chunking() {
    let payload: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
    let one_shot = sha256_hex(&payload);
    // 分片长度刻意取互质/极端值（1 字节、素数、超过总长）。
    for chunk in [1usize, 7, 997, 4096, payload.len(), payload.len() * 2] {
        let mut s = Sha256Stream::new();
        for part in payload.chunks(chunk.max(1)) {
            s.update(part);
        }
        assert_eq!(s.len(), payload.len() as u64, "累计字节数必须等于喂入总量");
        assert_eq!(s.finish(), one_shot, "分片大小 {chunk} 改变了摘要");
    }
    // 空输入与 `sha256_hex(b"")` 同口径。
    let empty = Sha256Stream::new();
    assert!(empty.is_empty());
    assert_eq!(Sha256Stream::new().finish(), EMPTY_SHA);
}

/// `Sha256Stream::verify` 的判定：匹配 / 大小写 / 不符（载荷逐字）/ 期望值非法。
#[test]
fn stream_verify_semantics() {
    let mut ok = Sha256Stream::new();
    ok.update(b"hel");
    ok.update(b"lo");
    assert!(ok.verify(HELLO_SHA).is_ok());
    // 大小写不敏感。
    let mut upper = Sha256Stream::new();
    upper.update(b"hello");
    assert!(upper.verify(&HELLO_SHA.to_uppercase()).is_ok());
    // 不符 → HashMismatch，且 actual 是喂入字节的真实摘要。
    let mut bad = Sha256Stream::new();
    bad.update(b"hello");
    assert_eq!(
        bad.verify(EMPTY_SHA),
        Err(VerifyError::HashMismatch {
            expected: EMPTY_SHA.to_string(),
            actual: HELLO_SHA.to_string(),
        })
    );
    // 期望 hex 格式非法 → InvalidExpectedHash（**不能**降级成「没校验」放行）。
    let mut invalid = Sha256Stream::new();
    invalid.update(b"hello");
    assert_eq!(
        invalid.verify("abc"),
        Err(VerifyError::InvalidExpectedHash(3))
    );
}

#[test]
fn sha256_reader_hex_matches_one_shot() {
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 97) as u8).collect();
    let got = sha256_reader_hex(std::io::Cursor::new(payload.clone())).unwrap();
    assert_eq!(got, sha256_hex(&payload));
    assert_eq!(
        sha256_reader_hex(std::io::Cursor::new(Vec::new())).unwrap(),
        EMPTY_SHA
    );
}

/// 🟡 **提升路径只 rename，不读内容** —— 且 tmp 必须与 dest 同目录。
///
/// **变异探针**：把 [`promote_staged`] 改回 `atomic_replace(fs, dest, &fs.read(tmp)?)`
/// ⇒ 「dest 落位后 tmp 必须消失且目录里只剩 dest」仍绿，但 `promote_staged` 就重新
/// 需要一次全量读 —— 故另配 `MockFailOp::Read` 注入：本函数**一次 read 都不能有**。
#[test]
fn promote_staged_renames_without_reading_the_payload() {
    let tmpdir = tempfile::tempdir().unwrap();
    let dest = StdFs.join(tmpdir.path(), "update.pkg");
    let tmp = tmp_name(&dest);
    assert_eq!(
        tmp.parent(),
        dest.parent(),
        "tmp 必须与 dest 同目录（跨卷 rename 不是原子操作）"
    );
    std::fs::write(&tmp, b"streamed-bytes").unwrap();

    // 注入「read 一律失败」：若实现里还藏着一次读回内存，本条立刻转红。
    let mut fs = MockFs::new(tmpdir.path());
    fs.fail_next(MockFailOp::Read);
    promote_staged(&fs, &tmp, &dest).unwrap();

    assert_eq!(std::fs::read(&dest).unwrap(), b"streamed-bytes");
    assert_eq!(
        StdFs.list_files(tmpdir.path()).unwrap(),
        vec!["update.pkg".to_string()],
        "提升后不得留 tmp 残件"
    );
}

/// rename 失败 → 删 tmp 残件后抛出，**dest 保持原样**（不出现半截态）。
#[test]
fn promote_staged_cleans_the_tmp_when_rename_fails() {
    let tmpdir = tempfile::tempdir().unwrap();
    let dest = StdFs.join(tmpdir.path(), "update.pkg");
    std::fs::write(&dest, b"old-and-complete").unwrap();
    let tmp = tmp_name(&dest);
    std::fs::write(&tmp, b"partial").unwrap();

    let mut fs = MockFs::new(tmpdir.path());
    fs.fail_next(MockFailOp::Rename);
    let err = promote_staged(&fs, &tmp, &dest).unwrap_err();
    assert!(err.to_string().contains("Rename"));

    assert!(!tmp.exists(), "rename 失败必须删掉 tmp 残件");
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        b"old-and-complete",
        "落位失败不得动 dest"
    );
}

/// 生产落位的两步：写进 [`tmp_name`] 给的唯一临时名，再 [`promote_staged`] 提升。
fn write_then_promote(fs: &dyn UpdateFs, dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = tmp_name(dest);
    fs.write(&tmp, bytes)?;
    promote_staged(fs, &tmp, dest)
}

/// 🟡 **变异锁：tmp 名必须每次调用都不同。**
///
/// 把 [`tmp_name`] 改回固定的 `{dest}.polaris-new` ⇒ 本条转红。它是
/// [`concurrent_landing_never_yields_a_torn_dest`] 的判据来源：并发写同一个 tmp 才是撕裂的成因。
#[test]
fn tmp_name_is_unique_per_call() {
    let dest = Path::new("/tmp/whatever/core.bin");
    let a = tmp_name(dest);
    let b = tmp_name(dest);
    assert_ne!(
        a, b,
        "固定 tmp 名 ⇒ 并发原子替换会把半截文件 rename 成 dest"
    );
    for p in [&a, &b] {
        let s = p.to_string_lossy().into_owned();
        assert!(
            s.starts_with("/tmp/whatever/core.bin.polaris-new-"),
            "tmp 必须与 dest **同目录**（跨目录 rename 不是原子操作），实得 {s}"
        );
    }
}

/// 🟡 **并发原子替换绝不产生撕裂的 dest。**
///
/// 复现的是生产里的默认形态：`autoDownloadUpdate` 的后台下载腿与用户在 mini 弹窗点「更新」
/// 触发的下载腿写**同一个** dest。用固定 tmp 名时，两条腿的 write(truncate) 与 rename 交错
/// 会把长度不对的半截文件搬成 dest（且先完成方已报 success/verified）。
///
/// **变异探针**：把 `tmp_name` 改回固定名 ⇒ 读侧断言（dest 内容必须是某一方的完整载荷）转红。
#[test]
fn concurrent_landing_never_yields_a_torn_dest() {
    let tmpdir = tempfile::tempdir().unwrap();
    let dest = StdFs.join(tmpdir.path(), "update.pkg");
    // 三份**长度各异**的载荷：撕裂一定表现为「长度对不上任何一份」或「内容混杂」。
    let payloads: Vec<Vec<u8>> = (0..3u8)
        .map(|i| vec![b'a' + i; 4096 * (usize::from(i) + 1)])
        .collect();
    // 先落一份合法内容，读侧从第一拍起就有东西可读。
    write_then_promote(&StdFs, &dest, &payloads[0]).unwrap();

    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut writers = Vec::new();
    for p in payloads.clone() {
        let dest = dest.clone();
        writers.push(std::thread::spawn(move || {
            for _ in 0..60 {
                write_then_promote(&StdFs, &dest, &p).unwrap();
            }
        }));
    }
    let reader = {
        let (dest, stop, payloads) = (dest.clone(), stop.clone(), payloads.clone());
        std::thread::spawn(move || {
            let mut reads = 0u32;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok(got) = std::fs::read(&dest) {
                    assert!(
                        payloads.contains(&got),
                        "dest 出现了撕裂内容（长度 {}）—— 并发原子替换把半截文件 rename 成了 dest",
                        got.len()
                    );
                    reads += 1;
                }
            }
            reads
        })
    };
    for w in writers {
        w.join().unwrap();
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(reader.join().unwrap() > 0, "读侧一次都没读到 → 断言没生效");
    // 收尾：只剩 dest，无 tmp 残件。
    assert_eq!(
        StdFs.list_files(tmpdir.path()).unwrap(),
        vec!["update.pkg".to_string()]
    );
}
