#![allow(clippy::too_many_lines)]

use super::*;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix_src;

const CORE: &[u8] = b"fake sing-box binary content";
const CRONET: &[u8] = b"fake libcronet";

/// 当前进程的 uid（unix）：单测里「已鉴权的调用者」就是自己。Windows 那条腿不判属主。
fn me() -> Option<u32> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        Some(nix::unistd::Uid::current().as_raw())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// 往源目录放一个文件，权限定成 0644：属主判据要求组与其他人不可写，不能随测试环境的 umask 漂。
fn put(dir: &Path, name: &str, data: &[u8]) {
    let path = dir.join(name);
    fs::write(&path, data).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    }
}

/// 源目录用的临时目录，权限定成 0755（理由同 [`put`]：`tempdir()` 建出来的目录随 umask，
/// `umask 002` 下是组可写的）。
fn src_tempdir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
}

/// 准备一个 linux 形状的临时源目录：sing-box + libcronet.so，返回 (dir, sha256_hex)。
fn make_src_dir() -> (tempfile::TempDir, String) {
    let dir = src_tempdir();
    put(dir.path(), SINGBOX_BIN_NAME, CORE);
    put(dir.path(), "libcronet.so", CRONET);
    (dir, sha256_hex(CORE))
}

/// linux 形状的生产编排。
fn install(core: &Path, src: &Path, hash: &str) -> Result<Vec<String>, InstallResult> {
    install_core_files(core, src, hash, Platform::Linux, me())
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

// ===== 收源文件 + 原子安装（跨平台部分；符号链接/管道/硬链接/属主等见 `unix_src`） =====

/// 小上限：让超限分支用几十字节的文件就走得到。
const SMALL: SrcLimits = SrcLimits {
    max_files: 2,
    max_file_bytes: 32,
    max_total_bytes: 48,
};

/// 受保护目录的全量快照：(名字, 内容) 字母序。子目录记成 `None`。
type Snapshot = Vec<(String, Option<Vec<u8>>)>;

fn snapshot(dir: &Path) -> Snapshot {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            let data = fs::symlink_metadata(e.path())
                .unwrap()
                .is_file()
                .then(|| fs::read(e.path()).unwrap());
            (name, data)
        })
        .collect();
    entries.sort();
    entries
}

/// 造一个已装着旧版的受保护目录，返回 (dir, 安装前快照)。
///
/// 预置旧核、旧配套与一个无关文件，是为了让「受保护目录没变」这句断言同时覆盖三件事：
/// 没有新文件落盘、旧文件没被换掉、清理那一步没跑。
fn make_core_dir() -> (tempfile::TempDir, Snapshot) {
    let core = tempfile::tempdir().unwrap();
    fs::write(core.path().join(SINGBOX_BIN_NAME), b"old core").unwrap();
    fs::write(core.path().join("libcronet.so"), b"old cronet").unwrap();
    fs::write(core.path().join("previous.bin"), b"previous content").unwrap();
    let before = snapshot(core.path());
    (core, before)
}

/// 生产编排的等价物，只是上限可注入（[`install_core_files`] 恒用 [`SRC_LIMITS`]）。
fn install_with(
    core: &Path,
    src: &Path,
    want_hash: &str,
    limits: SrcLimits,
) -> Result<Vec<String>, InstallResult> {
    install_received(
        core,
        receive_src(src, want_hash, Platform::Linux, me(), limits)?,
    )
}

/// 断言：整个请求被拒，且受保护目录与安装前逐项相同。
fn assert_rejected_untouched(
    result: &Result<Vec<String>, InstallResult>,
    core: &Path,
    before: &Snapshot,
) {
    assert!(result.is_err(), "请求本该被拒：{result:?}");
    assert_eq!(&snapshot(core), before, "请求被拒，受保护目录却变了");
}

/// 上限：单文件 256 MiB、总量 512 MiB。依据写在常量的文档里（现行核最大约 86 MB）。
#[test]
fn production_limits_are_pinned() {
    assert_eq!(SRC_LIMITS.max_files, 8);
    assert_eq!(SRC_LIMITS.max_file_bytes, 256 * 1024 * 1024);
    assert_eq!(SRC_LIMITS.max_total_bytes, 512 * 1024 * 1024);
}

/// 三个按名字引用的常量与协议层的真值是同一份。
#[test]
fn name_constants_come_from_the_proto_allowlist() {
    assert_eq!(
        SINGBOX_BIN_NAME,
        core_payload::core_filename(Platform::Linux)
    );
    assert_eq!(SINGBOX_BIN_NAME, core_payload::core_filename(Platform::Mac));
    assert_eq!(
        SINGBOX_BIN_NAME_WIN,
        core_payload::core_filename(Platform::Win)
    );
    assert_eq!(
        Some(CRONET_DLL_NAME_WIN),
        core_payload::sidecar_filename(Platform::Win)
    );
}

/// 正向：只含核与配套库的常规文件装得进去，内容逐字节相同，不留 `.new` / `.bak`。
#[test]
fn install_copies_the_core_and_its_sidecar_byte_for_byte() {
    let (src, hash) = make_src_dir();
    let core = tempfile::tempdir().unwrap();

    let names = install(core.path(), src.path(), &hash).unwrap();

    assert_eq!(names, vec!["libcronet.so", SINGBOX_BIN_NAME]);
    assert_eq!(
        snapshot(core.path()),
        vec![
            ("libcronet.so".to_owned(), Some(CRONET.to_vec())),
            (SINGBOX_BIN_NAME.to_owned(), Some(CORE.to_vec())),
        ]
    );
    #[cfg(unix)]
    for name in &names {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(core.path().join(name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "{name} 权限位");
    }
}

/// 覆盖安装：旧核、旧配套都换成新的，备份不留。
#[test]
fn install_replaces_an_existing_version_and_leaves_no_backup() {
    let (src, hash) = make_src_dir();
    let (core, _) = make_core_dir();
    install(core.path(), src.path(), &hash).unwrap();
    assert_eq!(
        snapshot(core.path()),
        vec![
            ("libcronet.so".to_owned(), Some(CRONET.to_vec())),
            (SINGBOX_BIN_NAME.to_owned(), Some(CORE.to_vec())),
        ],
        "旧文件应被换掉，`previous.bin` 应被清理，`.new` / `.bak` 不应留下"
    );
}

#[test]
fn install_mkdirs_core_dir() {
    // core_dir 不存在时应自动建（helper.go:152-154 MkdirAll）
    let (src, hash) = make_src_dir();
    let parent = tempfile::tempdir().unwrap();
    let core = parent.path().join("nested").join("core");
    install(&core, src.path(), &hash).unwrap();
    assert!(core.join(SINGBOX_BIN_NAME).exists());
}

#[test]
fn hash_is_compared_case_insensitively() {
    // helper.go:146: EqualFold 大小写不敏感
    let (src, hash) = make_src_dir();
    let core = tempfile::tempdir().unwrap();
    install(core.path(), src.path(), &hash.to_uppercase()).unwrap();
}

/// 哈希不符：受保护目录零改动 —— 旧核还是旧核，写到一半的 `.new` 不留。
///
/// 比对是边写边算的，所以这条同时在验「没通过比对的字节不会变成生效文件，也不会留在目录里」。
#[test]
fn hash_mismatch_rejects_the_request_and_changes_nothing() {
    let (src, _) = make_src_dir();
    let (core, before) = make_core_dir();
    let result = install(core.path(), src.path(), &"0".repeat(64));
    assert_eq!(result, Err(InstallResult::HashMismatch));
    assert_rejected_untouched(&result, core.path(), &before);
}

#[test]
fn missing_binary_is_a_read_singbox_error() {
    let src = src_tempdir();
    let (core, before) = make_core_dir();
    let result = install(core.path(), src.path(), &"a".repeat(64));
    assert!(matches!(result, Err(InstallResult::ReadSingbox(_))));
    assert_rejected_untouched(&result, core.path(), &before);
}

#[test]
fn missing_src_dir_is_a_readdir_error() {
    let (core, before) = make_core_dir();
    let result = install(
        core.path(),
        Path::new("/nonexistent/src/dir/xyz"),
        &"a".repeat(64),
    );
    assert!(matches!(result, Err(InstallResult::ReadDir(_))));
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 核与配套库的名字由平台定：Windows 只认 `sing-box.exe` + `libcronet.dll`，
/// 别的平台的名字在这次请求里就是白名单外的名字。
#[test]
fn the_platform_decides_which_names_are_received() {
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME_WIN, b"windows core");
    put(src.path(), "libcronet.dll", b"cronet payload");
    let win_hash = sha256_hex(b"windows core");

    let core = tempfile::tempdir().unwrap();
    let names =
        install_core_files(core.path(), src.path(), &win_hash, Platform::Win, me()).unwrap();
    assert_eq!(names, vec!["libcronet.dll", SINGBOX_BIN_NAME_WIN]);
    assert_eq!(
        fs::read(core.path().join(SINGBOX_BIN_NAME_WIN)).unwrap(),
        b"windows core"
    );
    assert_eq!(
        fs::read(core.path().join("libcronet.dll")).unwrap(),
        b"cronet payload"
    );

    // 同一个源目录按 linux 来收：两个名字都在白名单外，整单拒掉。
    let (other, before) = make_core_dir();
    let result = install(other.path(), src.path(), &win_hash);
    assert_eq!(
        result,
        Err(InstallResult::ReadDir("entry name not allowed".into()))
    );
    assert_rejected_untouched(&result, other.path(), &before);

    // macOS 没有配套库：核旁边多一个 `libcronet.so` 也拒。
    let (mac_src, hash) = make_src_dir();
    let result = install_core_files(other.path(), mac_src.path(), &hash, Platform::Mac, me());
    assert_eq!(
        result,
        Err(InstallResult::ReadDir("entry name not allowed".into()))
    );
    assert_rejected_untouched(&result, other.path(), &before);
}

/// 白名单外的名字：拒掉**整个请求**（核与合法配套一个都不落），而不是跳过那一个文件。
///
/// 白名单是精确名：带版本后缀、别的扩展名、首尾多一个点、大小写不同的都不是。
#[test]
fn a_name_outside_the_allowlist_rejects_the_whole_request() {
    for bad in [
        "extra.txt",
        "sing-box.bak",
        "sing-box.new",
        ".core-seed.json",
        "libcronet",
        "libcronet.so.119",
        "libcronet.dylib",
        "libcronet.dll",
        ".libcronet.so",
        "libcronet..so",
        "libcronet.so copy",
        "LIBCRONET.SO.1",
    ] {
        let (src, hash) = make_src_dir();
        put(src.path(), bad, b"unwanted");
        let (core, before) = make_core_dir();
        let result = install(core.path(), src.path(), &hash);
        assert_eq!(
            result,
            Err(InstallResult::ReadDir("entry name not allowed".into())),
            "{bad:?}"
        );
        assert_rejected_untouched(&result, core.path(), &before);
    }
}

/// 名字校验的纯逻辑：重复的名字、不是合法 UTF-8 的名字都拒整单。
///
/// 重复名在真实文件系统上造不出来（它只在目录被并发改动时由 `readdir` 报出），故直接喂枚举结果。
#[test]
fn enumerated_names_must_be_allowlisted_and_unique() {
    let os = |names: &[&str]| names.iter().map(OsString::from).collect::<Vec<_>>();

    assert_eq!(
        checked_names(os(&["sing-box", "libcronet.so"]), Platform::Linux).unwrap(),
        vec!["libcronet.so", "sing-box"]
    );
    assert_eq!(
        checked_names(
            os(&["sing-box", "libcronet.so", "sing-box"]),
            Platform::Linux
        ),
        Err(InstallResult::ReadDir("duplicate entry name".into()))
    );
    assert_eq!(
        checked_names(os(&["libcronet.so", "libcronet.so"]), Platform::Linux),
        Err(InstallResult::ReadDir("duplicate entry name".into()))
    );
    assert_eq!(
        checked_names(os(&["sing-box", "other"]), Platform::Linux),
        Err(InstallResult::ReadDir("entry name not allowed".into()))
    );

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        // 末尾多一个非法字节：有损转换后会「看起来像」白名单里的名字，必须按原始字节判。
        let invalid = std::ffi::OsStr::from_bytes(b"libcronet.so\xff").to_owned();
        assert!(invalid.to_str().is_none(), "前置条件：不是合法 UTF-8");
        assert_eq!(
            checked_names(vec![OsString::from("sing-box"), invalid], Platform::Linux),
            Err(InstallResult::ReadDir("entry name not allowed".into()))
        );
    }
}

/// 子目录不再被跳过：名字不在白名单的按名字拒，名字合法的按类型拒。
#[test]
fn a_directory_entry_rejects_the_whole_request() {
    let (src, hash) = make_src_dir();
    fs::create_dir(src.path().join("subdir")).unwrap();
    let (core, before) = make_core_dir();
    let result = install(core.path(), src.path(), &hash);
    assert_rejected_untouched(&result, core.path(), &before);

    // 配套库的名字被一个目录占着。
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME, CORE);
    fs::create_dir(src.path().join("libcronet.so")).unwrap();
    let result = install(core.path(), src.path(), &sha256_hex(CORE));
    assert!(
        matches!(&result, Err(InstallResult::Read { name, .. }) if name == "libcronet.so"),
        "{result:?}"
    );
    assert_rejected_untouched(&result, core.path(), &before);

    // 核的名字被一个目录占着。
    let src = src_tempdir();
    fs::create_dir(src.path().join(SINGBOX_BIN_NAME)).unwrap();
    let result = install(core.path(), src.path(), &"a".repeat(64));
    assert!(matches!(result, Err(InstallResult::ReadSingbox(_))));
    assert_rejected_untouched(&result, core.path(), &before);
}

#[test]
fn too_many_entries_rejects_the_whole_request() {
    let (src, hash) = make_src_dir();
    let core = tempfile::tempdir().unwrap();
    // 正面对照：两个条目恰在上限内。
    install_with(core.path(), src.path(), &hash, SMALL).unwrap();

    put(src.path(), "third", b"c");
    let (core, before) = make_core_dir();
    let result = install_with(core.path(), src.path(), &hash, SMALL);
    assert_eq!(
        result,
        Err(InstallResult::ReadDir("too many entries".into()))
    );
    assert_rejected_untouched(&result, core.path(), &before);
}

#[test]
fn an_oversized_sidecar_is_rejected_when_received() {
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME, b"core");
    put(src.path(), "libcronet.so", &[0u8; 32]);
    let hash = sha256_hex(b"core");
    // 正面对照：恰等于上限的文件收得下。
    receive_src(src.path(), &hash, Platform::Linux, me(), SMALL).unwrap();

    put(src.path(), "libcronet.so", &[0u8; 33]);
    // 断在**收**这一步：落盘阶段另有一道按实际字节数的上限，只看最终结果分不出是哪道拦的。
    assert_eq!(
        receive_src(src.path(), &hash, Platform::Linux, me(), SMALL).unwrap_err(),
        InstallResult::Read {
            name: "libcronet.so".into(),
            detail: "size over limit".into()
        }
    );
}

/// 超限的核在收的时候就被拒（不开始往受保护目录里写）。
#[test]
fn an_oversized_binary_is_rejected_when_received() {
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME, &[7u8; 33]);
    assert_eq!(
        receive_src(
            src.path(),
            &sha256_hex(&[7u8; 33]),
            Platform::Linux,
            me(),
            SMALL
        )
        .unwrap_err(),
        InstallResult::ReadSingbox("size over limit".into())
    );
}

#[test]
fn total_size_over_the_limit_rejects_the_whole_request() {
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME, &[7u8; 16]);
    put(src.path(), "libcronet.so", &[0u8; 32]);
    let hash = sha256_hex(&[7u8; 16]);
    let core = tempfile::tempdir().unwrap();
    // 正面对照：16 + 32 恰等于总量上限。
    install_with(core.path(), src.path(), &hash, SMALL).unwrap();

    put(src.path(), SINGBOX_BIN_NAME, &[7u8; 17]);
    let (core, before) = make_core_dir();
    let result = install_with(core.path(), src.path(), &sha256_hex(&[7u8; 17]), SMALL);
    assert_eq!(
        result,
        Err(InstallResult::ReadDir("size over limit".into()))
    );
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 流式拷贝：写出去的字节与算进哈希的字节是同一批，超限即停。
#[test]
fn copy_hashing_writes_exactly_what_it_hashes_and_stops_at_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    // 跨过一个缓冲块（64 KiB），确认分块读写不丢不重。
    let data: Vec<u8> = (0..150_000u32).map(|i| (i % 251) as u8).collect();
    fs::write(dir.path().join("in"), &data).unwrap();
    let source = File::open(dir.path().join("in")).unwrap();

    let mut out = create_private(&dir.path().join("out")).unwrap();
    let (len, hash) = copy_hashing(&source, &mut out, data.len() as u64)
        .ok()
        .expect("恰等于上限应通过");
    drop(out);
    assert_eq!(len, data.len() as u64);
    assert_eq!(hash, sha256_hex(&data));
    assert_eq!(fs::read(dir.path().join("out")).unwrap(), data);

    let source = File::open(dir.path().join("in")).unwrap();
    let mut out = create_private(&dir.path().join("out2")).unwrap();
    assert!(matches!(
        copy_hashing(&source, &mut out, data.len() as u64 - 1),
        Err(CopyError::Read(_))
    ));
}

/// 写入中的 `.new` 只有本进程读得到（它装着还没通过比对的字节，而受保护目录对所有人可读）。
#[cfg(unix)]
#[test]
fn the_tmp_file_is_private_while_being_written() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("sing-box.new");
    let _out = create_private(&tmp).unwrap();
    assert_eq!(
        fs::metadata(&tmp).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

/// **核恒第一个就位**（S2）：它撞锁失败时，一组文件里零个被改动，且配套库连碰都没碰。
///
/// 用「目标已被一个目录占住」制造就位硬失败，因为 Windows 上真正的失败源（运行中的 exe 持有
/// 句柄）在 Linux 上造不出来。
///
/// 就位失败有回滚兜底，所以只看「目录没变」分不出顺序。钉顺序的办法是让**两个**文件的就位都
/// 必败，看报出来的是谁：`libcronet.dll` 字母序在 `sing-box.exe` 之前，按字母序走时先撞上的
/// 会是它。
#[test]
fn install_puts_the_binary_in_place_first() {
    let src = src_tempdir();
    let core = tempfile::tempdir().unwrap();
    put(src.path(), SINGBOX_BIN_NAME_WIN, b"new core");
    put(src.path(), "libcronet.dll", b"new cronet");
    let hash = sha256_hex(b"new core");
    // 两个正式名都被目录占着：谁先就位谁先失败。
    fs::create_dir(core.path().join(SINGBOX_BIN_NAME_WIN)).unwrap();
    fs::create_dir(core.path().join("libcronet.dll")).unwrap();
    let before = snapshot(core.path());

    let result = install_core_files(core.path(), src.path(), &hash, Platform::Win, me());

    assert!(
        matches!(&result, Err(InstallResult::Rename { name, .. }) if name == SINGBOX_BIN_NAME_WIN),
        "第一个去就位的不是核：{result:?}"
    );
    assert_rejected_untouched(&result, core.path(), &before);

    // 只有核的正式名被占：同样是核先失败，配套库没有被换成新版。
    fs::remove_dir(core.path().join("libcronet.dll")).unwrap();
    fs::write(core.path().join("libcronet.dll"), b"old cronet").unwrap();
    let before = snapshot(core.path());
    let result = install_core_files(core.path(), src.path(), &hash, Platform::Win, me());
    assert!(
        matches!(&result, Err(InstallResult::Rename { name, .. }) if name == SINGBOX_BIN_NAME_WIN),
        "{result:?}"
    );
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 就位中途失败 → 回滚：已经就位的核换回旧核，结果与安装前逐项相同（要么全新要么全旧）。
///
/// 让第二个文件（配套库）的就位失败：它的正式名被一个目录占着。此时核已经 rename 过去了。
#[test]
fn a_failure_midway_through_placement_rolls_everything_back() {
    let (src, hash) = make_src_dir();

    // 有旧核：回滚 = 把备份换回去。
    let core = tempfile::tempdir().unwrap();
    fs::write(core.path().join(SINGBOX_BIN_NAME), b"old core").unwrap();
    fs::create_dir(core.path().join("libcronet.so")).unwrap();
    let before = snapshot(core.path());
    let result = install(core.path(), src.path(), &hash);
    assert!(
        matches!(&result, Err(InstallResult::Rename { name, .. }) if name == "libcronet.so"),
        "{result:?}"
    );
    assert_rejected_untouched(&result, core.path(), &before);

    // 没有旧核：回滚 = 把刚就位的核删掉。
    let core = tempfile::tempdir().unwrap();
    fs::create_dir(core.path().join("libcronet.so")).unwrap();
    let before = snapshot(core.path());
    let result = install(core.path(), src.path(), &hash);
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 上一次安装被打断留下的东西，在下一次安装开头收拾掉。
#[test]
fn leftovers_of_an_interrupted_install_are_cleaned_up_first() {
    // 只剩备份、正式名缺了（打断在回滚途中）：把备份换回去。
    let core = tempfile::tempdir().unwrap();
    fs::write(core.path().join("sing-box.bak"), b"old core").unwrap();
    fs::write(core.path().join("libcronet.so.new"), b"half").unwrap();
    recover_interrupted(core.path(), Platform::Linux);
    assert_eq!(
        snapshot(core.path()),
        vec![(SINGBOX_BIN_NAME.to_owned(), Some(b"old core".to_vec()))]
    );

    // 正式名与备份都在（打断在就位之后、删备份之前）：留正式名，删备份。
    let core = tempfile::tempdir().unwrap();
    fs::write(core.path().join(SINGBOX_BIN_NAME), b"current").unwrap();
    fs::write(core.path().join("sing-box.bak"), b"older").unwrap();
    fs::write(core.path().join("sing-box.new"), b"half").unwrap();
    fs::write(core.path().join("libcronet.so.bak"), b"older cronet").unwrap();
    fs::write(core.path().join("libcronet.so"), b"current cronet").unwrap();
    recover_interrupted(core.path(), Platform::Linux);
    assert_eq!(
        snapshot(core.path()),
        vec![
            ("libcronet.so".to_owned(), Some(b"current cronet".to_vec())),
            (SINGBOX_BIN_NAME.to_owned(), Some(b"current".to_vec())),
        ]
    );

    // 接在生产编排上：带着残留的目录照常装得进去，残留一个不剩。
    let (src, hash) = make_src_dir();
    let core = tempfile::tempdir().unwrap();
    fs::write(
        core.path().join("sing-box.new"),
        b"half-written leftover that is longer than the real core content",
    )
    .unwrap();
    fs::write(core.path().join("sing-box.bak"), b"old core").unwrap();
    fs::write(core.path().join("libcronet.so.bak"), b"old cronet").unwrap();
    install(core.path(), src.path(), &hash).unwrap();
    assert_eq!(
        snapshot(core.path()),
        vec![
            ("libcronet.so".to_owned(), Some(CRONET.to_vec())),
            (SINGBOX_BIN_NAME.to_owned(), Some(CORE.to_vec())),
        ]
    );

    // 残留收拾完之后这次请求被拒（哈希不符）：恢复出来的旧核留着，不因为「反正要装新的」而丢。
    let core = tempfile::tempdir().unwrap();
    fs::write(core.path().join("sing-box.bak"), b"old core").unwrap();
    let result = install(core.path(), src.path(), &"0".repeat(64));
    assert_eq!(result, Err(InstallResult::HashMismatch));
    assert_eq!(
        snapshot(core.path()),
        vec![(SINGBOX_BIN_NAME.to_owned(), Some(b"old core".to_vec()))]
    );
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

    let result = install(core.path(), src.path(), &hash);
    assert!(result.is_ok());
    // 旧残留被清
    assert!(!core.path().join("stale.bin").exists());
    // 新文件就位
    assert!(core.path().join(SINGBOX_BIN_NAME).exists());
    assert!(core.path().join("libcronet.so").exists());
}

#[test]
fn install_core_files_coredir_unset() {
    // helper.go:135: coreDir 空 → ERR coredir-unset
    let (src, hash) = make_src_dir();
    let result = install(Path::new(""), src.path(), &hash);
    assert_eq!(result.unwrap_err(), InstallResult::CoreDirUnset);
}

#[test]
fn install_core_files_bad_args_empty_src() {
    // helper.go:137: srcDir 空 → ERR bad-args
    let core = tempfile::tempdir().unwrap();
    let result = install(core.path(), Path::new(""), &"a".repeat(64));
    assert_eq!(result.unwrap_err(), InstallResult::BadArgs);
}

#[test]
fn install_core_files_bad_args_short_hash() {
    // helper.go:137: wantHash 长度 != 64 → ERR bad-args
    let (src, _hash) = make_src_dir();
    let core = tempfile::tempdir().unwrap();
    let result = install(core.path(), src.path(), "abc");
    assert_eq!(result.unwrap_err(), InstallResult::BadArgs);
}

#[test]
fn install_core_files_bad_args_non_hex_hash() {
    // 64 字符但非 hex
    let (src, _hash) = make_src_dir();
    let core = tempfile::tempdir().unwrap();
    let result = install(core.path(), src.path(), &"z".repeat(64));
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
