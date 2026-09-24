#![allow(clippy::too_many_lines)]

use super::*;

/// 准备一个临时源目录：sing-box + 一个配套文件，返回 (dir, sha256_hex)。
fn make_src_dir() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let sb_content = b"fake sing-box binary content";
    fs::write(dir.path().join(SINGBOX_BIN_NAME), sb_content).unwrap();
    // 给 sing-box 可执行权限（unix）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            dir.path().join(SINGBOX_BIN_NAME),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    fs::write(dir.path().join("libcronet.dylib"), b"fake libcronet").unwrap();

    let hash = sha256_hex(sb_content);
    (dir, hash)
}

// ===== sha256_hex（合并自 linux 自实现 + mac 内联） =====

#[test]
fn sha256_hex_known_vector() {
    // sha256(b"") = e3b0c4...
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    // sha256(b"abc") = ba7816...
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn sha256_hex_lowercase() {
    // Go hex.EncodeToString 输出小写
    let h = sha256_hex(b"X");
    assert_eq!(h, h.to_lowercase());
    assert!(!h.chars().any(|c| c.is_ascii_uppercase()));
}

// ===== verify_singbox_hash（移植自 mac 单测） =====

#[test]
fn verify_singbox_hash_matches() {
    // helper.go:144-146: sha256 匹配
    let (dir, hash) = make_src_dir();
    let result = verify_singbox_hash(dir.path(), &hash, SINGBOX_BIN_NAME);
    assert!(result.is_ok());
    assert!(!result.unwrap().is_empty());
}

#[test]
fn verify_singbox_hash_case_insensitive() {
    // Go EqualFold 大小写不敏感
    let (dir, hash) = make_src_dir();
    let upper = hash.to_uppercase();
    assert!(verify_singbox_hash(dir.path(), &upper, SINGBOX_BIN_NAME).is_ok());
}

#[test]
fn verify_singbox_hash_mismatch() {
    // helper.go:146: hash 不符
    let (dir, _hash) = make_src_dir();
    let wrong = "0".repeat(64);
    let result = verify_singbox_hash(dir.path(), &wrong, SINGBOX_BIN_NAME);
    assert_eq!(result.unwrap_err(), InstallResult::HashMismatch);
}

/// `bin_name` 选的是**哪个文件**被读、被校验 —— 两个名字各读各的，不串。
#[test]
fn verify_singbox_hash_reads_the_named_binary() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(SINGBOX_BIN_NAME), b"unix core").unwrap();
    fs::write(dir.path().join(SINGBOX_BIN_NAME_WIN), b"windows core").unwrap();
    let unix_hash = sha256_hex(b"unix core");
    let win_hash = sha256_hex(b"windows core");

    // 正面：各自读到自己那份（返回的字节即后续复用的已校验字节）。
    assert_eq!(
        verify_singbox_hash(dir.path(), &unix_hash, SINGBOX_BIN_NAME).unwrap(),
        b"unix core"
    );
    assert_eq!(
        verify_singbox_hash(dir.path(), &win_hash, SINGBOX_BIN_NAME_WIN).unwrap(),
        b"windows core"
    );
    // 反面对照：拿 .exe 的 hash 去校 `sing-box` 必不符 —— 证明上面两条不是「碰巧都过」。
    assert_eq!(
        verify_singbox_hash(dir.path(), &win_hash, SINGBOX_BIN_NAME).unwrap_err(),
        InstallResult::HashMismatch
    );
}

#[test]
fn verify_singbox_read_fails() {
    // helper.go:142: 读失败 → ERR read-singbox
    let dir = tempfile::tempdir().unwrap(); // 空目录，无 sing-box
    let result = verify_singbox_hash(dir.path(), &"a".repeat(64), SINGBOX_BIN_NAME);
    assert!(matches!(result, Err(InstallResult::ReadSingbox(_))));
}

// ===== list_src_files（移植自 mac 单测） =====

#[test]
fn list_src_files_excludes_dirs() {
    // helper.go:158: if e.IsDir() { continue }
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("sing-box"), b"x").unwrap();
    fs::write(dir.path().join("libcronet.dylib"), b"y").unwrap();
    fs::create_dir(dir.path().join("subdir")).unwrap();
    let names = list_src_files(dir.path()).unwrap();
    assert!(names.contains(&"sing-box".to_owned()));
    assert!(names.contains(&"libcronet.dylib".to_owned()));
    assert!(!names.contains(&"subdir".to_owned()), "目录应被排除");
}

#[test]
fn list_src_files_sorted() {
    // Go os.ReadDir 返回已排序
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("zzz"), b"1").unwrap();
    fs::write(dir.path().join("aaa"), b"2").unwrap();
    fs::write(dir.path().join("mmm"), b"3").unwrap();
    let names = list_src_files(dir.path()).unwrap();
    assert_eq!(names, vec!["aaa", "mmm", "zzz"]);
}

// ===== atomic_install_files（移植自 mac 单测） =====

#[test]
fn atomic_install_writes_all_files() {
    // helper.go:156-178: 逐文件原子写入 + chmod 0755
    let (src, hash) = make_src_dir();
    let core = tempfile::tempdir().unwrap();
    let sb_data = verify_singbox_hash(src.path(), &hash, SINGBOX_BIN_NAME).unwrap();
    let names = list_src_files(src.path()).unwrap();
    atomic_install_files(src.path(), core.path(), &names, &sb_data, SINGBOX_BIN_NAME).unwrap();

    // 验证两文件都就位
    assert!(core.path().join(SINGBOX_BIN_NAME).exists());
    assert!(core.path().join("libcronet.dylib").exists());
    // sing-box 内容与源一致（堵 TOCTOU：用的是已校验字节）
    let installed = fs::read(core.path().join(SINGBOX_BIN_NAME)).unwrap();
    assert_eq!(installed, b"fake sing-box binary content");
    // 权限 0755（unix）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(core.path().join(SINGBOX_BIN_NAME))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}

#[test]
fn atomic_install_leaves_no_tmp_files() {
    // .new 文件应已 rename 掉，不留残留
    let (src, hash) = make_src_dir();
    let core = tempfile::tempdir().unwrap();
    let sb_data = verify_singbox_hash(src.path(), &hash, SINGBOX_BIN_NAME).unwrap();
    let names = list_src_files(src.path()).unwrap();
    atomic_install_files(src.path(), core.path(), &names, &sb_data, SINGBOX_BIN_NAME).unwrap();
    // 不应有 .new 文件残留
    let entries: Vec<_> = fs::read_dir(core.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!entries.iter().any(|n| n.ends_with(".new")));
}

/// 「复用已校验字节」的判等必须用传入的 `bin_name`，不能写死 [`SINGBOX_BIN_NAME`]。
///
/// 磁盘上那份 `sing-box.exe` 与传入的 `sb_data` **刻意不同**（模拟校验后被换掉）：判等对上 →
/// 落盘的是 `sb_data`；判等漏掉（写死 `"sing-box"`，`sing-box.exe` 不等于它）→ 走 else 分支回头
/// 读盘，落盘的是磁盘那份 ⇒ hash 校验与实际写入的字节脱钩。漏改不会编译错，只会静默失效。
#[test]
fn atomic_install_reuses_verified_bytes_for_the_named_binary() {
    let src = tempfile::tempdir().unwrap();
    let core = tempfile::tempdir().unwrap();
    fs::write(
        src.path().join(SINGBOX_BIN_NAME_WIN),
        b"swapped-after-verify",
    )
    .unwrap();
    fs::write(src.path().join("libcronet.dll"), b"cronet payload").unwrap();
    let verified: &[u8] = b"verified bytes";

    let names = list_src_files(src.path()).unwrap();
    atomic_install_files(
        src.path(),
        core.path(),
        &names,
        verified,
        SINGBOX_BIN_NAME_WIN,
    )
    .unwrap();

    assert_eq!(
        fs::read(core.path().join(SINGBOX_BIN_NAME_WIN)).unwrap(),
        verified,
        "sing-box.exe 未复用已校验字节 ⇒ 判等没用上 bin_name，TOCTOU 那层空了"
    );
    // 正面对照：配套文件确实走了「回头读盘」那条腿 —— 否则上面那条相等可能只是「什么都写 sb_data」。
    assert_eq!(
        fs::read(core.path().join("libcronet.dll")).unwrap(),
        b"cronet payload"
    );
}

/// **`bin_name` 恒第一个被处理**（S2）：它撞锁失败时，一组文件里零个被改动。
///
/// 用「目标已被一个目录占住」制造 rename 硬失败（`rename(2)` 对「文件 → 已存在的目录」返 EISDIR），
/// 因为 Windows 上真正的失败源（运行中的 exe 持有句柄）在 Linux 上造不出来。
///
/// 钉的是**顺序本身**，不是「sort 被调用」：`libcronet.dll` 字母序在 `sing-box.exe` 之前，
/// 按 `names` 原序走时它会先 rename 成功，于是受保护目录留下「新 DLL + 旧 exe」的持久化版本错配
/// ——「`.new` + rename 保证半成品不会变成生效文件」这句只对**单个文件**成立，对一组文件不成立。
#[test]
fn atomic_install_processes_the_named_binary_first() {
    let src = tempfile::tempdir().unwrap();
    let core = tempfile::tempdir().unwrap();
    fs::write(src.path().join(SINGBOX_BIN_NAME_WIN), b"new core").unwrap();
    fs::write(src.path().join("libcronet.dll"), b"new cronet").unwrap();
    let names = list_src_files(src.path()).unwrap();
    assert_eq!(
        names,
        vec!["libcronet.dll", SINGBOX_BIN_NAME_WIN],
        "前置条件：配套 DLL 的字母序必须在核之前，否则本条什么都没验"
    );
    // 让 `sing-box.exe` 的 rename 必失败（目标是一个目录 → EISDIR）。
    fs::create_dir(core.path().join(SINGBOX_BIN_NAME_WIN)).unwrap();

    let err = atomic_install_files(
        src.path(),
        core.path(),
        &names,
        b"new core",
        SINGBOX_BIN_NAME_WIN,
    )
    .unwrap_err();

    assert!(
        matches!(&err, InstallResult::Rename { name, .. } if name == SINGBOX_BIN_NAME_WIN),
        "第一个失败的不是核：{err:?}"
    );
    assert!(
        !core.path().join("libcronet.dll").exists(),
        "核撞锁中止前，配套 DLL 已经被换成新版 ⇒ 受保护目录留下「新 DLL + 旧 exe」的版本错配"
    );
    // 半成品也不许留（tmp 在失败腿上被清掉）。
    assert!(!core.path().join("libcronet.dll.new").exists());
    assert!(!core
        .path()
        .join(format!("{SINGBOX_BIN_NAME_WIN}.new"))
        .exists());
    // 正面对照：没有那个障碍时两个文件都装得进去（上面的「没装」不是「什么都做不到」）。
    let clean = tempfile::tempdir().unwrap();
    atomic_install_files(
        src.path(),
        clean.path(),
        &names,
        b"new core",
        SINGBOX_BIN_NAME_WIN,
    )
    .unwrap();
    assert_eq!(
        fs::read(clean.path().join(SINGBOX_BIN_NAME_WIN)).unwrap(),
        b"new core"
    );
    assert_eq!(
        fs::read(clean.path().join("libcronet.dll")).unwrap(),
        b"new cronet"
    );
}

#[test]
fn atomic_install_mkdirs_core_dir() {
    // core_dir 不存在时应自动建（helper.go:152-154 MkdirAll）
    let (src, hash) = make_src_dir();
    let parent = tempfile::tempdir().unwrap();
    let core = parent.path().join("nested").join("core");
    let sb_data = verify_singbox_hash(src.path(), &hash, SINGBOX_BIN_NAME).unwrap();
    let names = list_src_files(src.path()).unwrap();
    atomic_install_files(src.path(), &core, &names, &sb_data, SINGBOX_BIN_NAME).unwrap();
    assert!(core.join(SINGBOX_BIN_NAME).exists());
}

// ===== prune_extra_files（移植自 mac 单测） =====

#[test]
fn prune_extra_files_removes_old() {
    // helper.go:179-192: 清理不在 keep_names 的旧文件
    let core = tempfile::tempdir().unwrap();
    // 模拟旧残留：旧版 sing-box + 旧配套
    fs::write(core.path().join("sing-box"), b"old").unwrap();
    fs::write(core.path().join("libcronet_old.dylib"), b"old dylib").unwrap();
    fs::write(core.path().join("stale.bin"), b"stale").unwrap();
    // 新 src 只有 sing-box + libcronet.dylib
    let keep = vec!["sing-box".to_owned(), "libcronet.dylib".to_owned()];
    prune_extra_files(core.path(), &keep);
    // 旧残留应被删
    assert!(!core.path().join("stale.bin").exists());
    assert!(!core.path().join("libcronet_old.dylib").exists());
    // sing-box 保留（在 keep 中）
    assert!(core.path().join("sing-box").exists());
}

#[test]
fn prune_extra_files_missing_dir_is_noop() {
    // core_dir 不存在 → 静默 no-op（best-effort）
    prune_extra_files(Path::new("/nonexistent/xyz/core"), &[]);
}

// ===== install_core_files 完整流程（移植自 mac 单测） =====

#[test]
fn install_core_files_full_flow() {
    // 完整流程：校验 → 枚举 → 写入 → 清理
    let (src, hash) = make_src_dir();
    let core = tempfile::tempdir().unwrap();
    // 预放一个旧残留
    fs::write(core.path().join("stale.bin"), b"stale").unwrap();

    let result = install_core_files(core.path(), src.path(), &hash, SINGBOX_BIN_NAME);
    assert!(result.is_ok());
    // 旧残留被清
    assert!(!core.path().join("stale.bin").exists());
    // 新文件就位
    assert!(core.path().join(SINGBOX_BIN_NAME).exists());
    assert!(core.path().join("libcronet.dylib").exists());
}

#[test]
fn install_core_files_coredir_unset() {
    // helper.go:135: coreDir 空 → ERR coredir-unset
    let (src, hash) = make_src_dir();
    let result = install_core_files(Path::new(""), src.path(), &hash, SINGBOX_BIN_NAME);
    assert_eq!(result.unwrap_err(), InstallResult::CoreDirUnset);
}

#[test]
fn install_core_files_bad_args_empty_src() {
    // helper.go:137: srcDir 空 → ERR bad-args
    let core = tempfile::tempdir().unwrap();
    let result = install_core_files(
        core.path(),
        Path::new(""),
        &"a".repeat(64),
        SINGBOX_BIN_NAME,
    );
    assert_eq!(result.unwrap_err(), InstallResult::BadArgs);
}

#[test]
fn install_core_files_bad_args_short_hash() {
    // helper.go:137: wantHash 长度 != 64 → ERR bad-args
    let (src, _hash) = make_src_dir();
    let core = tempfile::tempdir().unwrap();
    let result = install_core_files(core.path(), src.path(), "abc", SINGBOX_BIN_NAME);
    assert_eq!(result.unwrap_err(), InstallResult::BadArgs);
}

#[test]
fn install_core_files_bad_args_non_hex_hash() {
    // 64 字符但非 hex
    let (src, _hash) = make_src_dir();
    let core = tempfile::tempdir().unwrap();
    let result = install_core_files(core.path(), src.path(), &"z".repeat(64), SINGBOX_BIN_NAME);
    assert_eq!(result.unwrap_err(), InstallResult::BadArgs);
}

// ===== to_wire_line（锁住 wire 协议，对照 Go 源各 return 分支） =====

#[test]
fn to_wire_line_for_all_variants() {
    assert_eq!(InstallResult::Installed.to_wire_line(), "OK installed");
    assert_eq!(
        InstallResult::CoreDirUnset.to_wire_line(),
        "ERR coredir-unset"
    );
    assert_eq!(InstallResult::BadArgs.to_wire_line(), "ERR bad-args");
    assert_eq!(
        InstallResult::HashMismatch.to_wire_line(),
        "ERR hash-mismatch"
    );
    assert_eq!(
        InstallResult::ReadSingbox("e".into()).to_wire_line(),
        "ERR read-singbox e"
    );
    assert_eq!(
        InstallResult::ReadDir("e".into()).to_wire_line(),
        "ERR readdir e"
    );
    assert_eq!(
        InstallResult::Mkdir("e".into()).to_wire_line(),
        "ERR mkdir e"
    );
    assert_eq!(
        InstallResult::Read {
            name: "libcronet.so".into(),
            detail: "perm".into()
        }
        .to_wire_line(),
        "ERR read libcronet.so perm"
    );
    assert_eq!(
        InstallResult::Write {
            name: "x".into(),
            detail: "full".into()
        }
        .to_wire_line(),
        "ERR write x full"
    );
    assert_eq!(
        InstallResult::Rename {
            name: "y".into(),
            detail: "busy".into()
        }
        .to_wire_line(),
        "ERR rename y busy"
    );
    assert_eq!(InstallResult::Busy.to_wire_line(), "ERR busy");
}

/// `ERR busy` 是 Polaris 新增的 token，旧 app 的 `from_wire_token` 不认识它。
///
/// 钉住「不认识 ≠ 崩/丢信息」：归 [`polaris_helper_proto::ErrorCode::Other`] 且 detail 保原文，
/// 再序列化回去逐字无损（Other 的 `to_wire_line` 直接拼 detail）。
#[test]
fn busy_wire_line_round_trips_through_proto() {
    let line = InstallResult::Busy.to_wire_line();
    assert_eq!(line, "ERR busy");
    let parsed = polaris_helper_proto::Error::parse(&line).expect("ERR 行必可解析");
    assert_eq!(parsed.code, polaris_helper_proto::ErrorCode::Other);
    assert_eq!(parsed.detail, "busy");
    assert_eq!(parsed.to_wire_line(), line);
}

#[test]
fn is_ok_predicate() {
    assert!(InstallResult::Installed.is_ok());
    assert!(!InstallResult::BadArgs.is_ok());
    assert!(!InstallResult::HashMismatch.is_ok());
    assert!(!InstallResult::Busy.is_ok());
}

// ===== is_valid_core_dir（移植自 mac 单测） =====

#[test]
fn is_valid_core_dir_checks() {
    let parent = tempfile::tempdir().unwrap();
    let core = parent.path().join("core");
    assert!(is_valid_core_dir(&core), "父目录存在 + core 非空 → 有效");
    assert!(!is_valid_core_dir(Path::new("")), "空路径无效");
    assert!(
        !is_valid_core_dir(Path::new("/nonexistent-parent-xyz/core")),
        "父目录不存在 → 无效"
    );
}
