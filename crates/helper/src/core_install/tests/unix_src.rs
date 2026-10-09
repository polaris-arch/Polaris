//! 源目录里摆着「不是调用者自己的普通文件」的东西时的行为（符号链接 / 命名管道 / 硬链接 /
//! 属主不符 / 中途换文件）。
//!
//! 每条的形状相同：摆好一个本该被拒的源目录 → 走生产编排 → 断言整个请求被拒，且受保护目录
//! 与安装前逐项相同。

use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::time::Duration;

/// 源目录之外的一个文件（扮演「调用者本不该能让高权限进程读到的内容」）。
fn outside_file(content: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "outside-secret", content);
    let path = dir.path().join("outside-secret");
    (dir, path)
}

/// 在独立线程里跑安装并限时：命名管道那几条若打开时阻塞，这里以超时失败而不是把测试进程挂死。
fn install_within_timeout(
    core: &Path,
    src: &Path,
    hash: &str,
) -> Result<Vec<String>, InstallResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (core, src, hash) = (core.to_path_buf(), src.to_path_buf(), hash.to_owned());
    std::thread::spawn(move || {
        let _ = tx.send(install(&core, &src, &hash));
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("安装线程 10 秒未返回：打开源文件时阻塞了")
}

/// 核的名字是一个指向源目录外文件的符号链接，且客户端给的哈希**就是**那个文件的哈希 ——
/// 跟随链接的实现会校验通过并把外面的文件装进来。
#[test]
fn a_symlink_under_the_binary_name_is_rejected() {
    let (_keep, secret) = outside_file(b"outside bytes posing as the core");
    let src = src_tempdir();
    symlink(&secret, src.path().join(SINGBOX_BIN_NAME)).unwrap();
    let hash = sha256_hex(b"outside bytes posing as the core");
    let (core, before) = make_core_dir();

    let result = install(core.path(), src.path(), &hash);

    assert!(matches!(result, Err(InstallResult::ReadSingbox(_))));
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 配套库的名字是一个指向源目录外文件的符号链接（配套文件没有哈希可比，跟随即落盘）。
#[test]
fn a_symlink_under_the_sidecar_name_is_rejected() {
    let (_keep, secret) = outside_file(b"contents of a file outside the source dir");
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME, CORE);
    symlink(&secret, src.path().join("libcronet.so")).unwrap();
    let (core, before) = make_core_dir();

    let result = install(core.path(), src.path(), &sha256_hex(CORE));

    assert!(
        matches!(&result, Err(InstallResult::Read { name, .. }) if name == "libcronet.so"),
        "{result:?}"
    );
    assert_rejected_untouched(&result, core.path(), &before);
}

#[test]
fn a_dangling_symlink_is_rejected() {
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME, CORE);
    symlink("/nonexistent/target/xyz", src.path().join("libcronet.so")).unwrap();
    let (core, before) = make_core_dir();
    let result = install(core.path(), src.path(), &sha256_hex(CORE));
    assert_rejected_untouched(&result, core.path(), &before);
}

#[test]
fn a_symlink_to_a_directory_is_rejected() {
    let target = tempfile::tempdir().unwrap();
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME, CORE);
    symlink(target.path(), src.path().join("libcronet.so")).unwrap();
    let (core, before) = make_core_dir();
    let result = install(core.path(), src.path(), &sha256_hex(CORE));
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 源目录本身是符号链接。
#[test]
fn a_symlinked_source_directory_is_rejected() {
    let (src, hash) = make_src_dir();
    let holder = tempfile::tempdir().unwrap();
    let link = holder.path().join("src-link");
    symlink(src.path(), &link).unwrap();
    let (core, before) = make_core_dir();

    let result = install(core.path(), &link, &hash);

    assert!(matches!(result, Err(InstallResult::ReadDir(_))));
    assert_rejected_untouched(&result, core.path(), &before);
    // 正面对照：同一个目录按真实路径给就装得进去。
    install(core.path(), src.path(), &hash).unwrap();
}

/// 命名管道：既不能卡住（无写端的只读打开默认一直等），也不能被当成空文件装进去。
#[test]
fn a_named_pipe_is_rejected_without_blocking() {
    let mode = nix::sys::stat::Mode::from_bits_truncate(0o644);

    // 配套库名字上的管道。
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME, CORE);
    nix::unistd::mkfifo(&src.path().join("libcronet.so"), mode).unwrap();
    let (core, before) = make_core_dir();
    let result = install_within_timeout(core.path(), src.path(), &sha256_hex(CORE));
    assert_eq!(
        result,
        Err(InstallResult::Read {
            name: "libcronet.so".into(),
            detail: "not a regular file".into()
        })
    );
    assert_rejected_untouched(&result, core.path(), &before);

    // 核名字上的管道。
    let src = src_tempdir();
    nix::unistd::mkfifo(&src.path().join(SINGBOX_BIN_NAME), mode).unwrap();
    // 空输入的 sha256：把管道当空文件读的实现会校验通过。
    let result = install_within_timeout(core.path(), src.path(), &sha256_hex(b""));
    assert_eq!(
        result,
        Err(InstallResult::ReadSingbox("not a regular file".into()))
    );
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 硬链接：不经符号链接，也能把别处的 inode 摆进源目录。
///
/// 本机以普通用户跑、且 `fs.protected_hardlinks=1`，造不出「链到一个自己读不了的别人的文件」
/// （那正是该开关禁止的）。等价物有两个：链到源目录外一个**可读**的自有文件（拦它的只有链接数
/// 判据），以及链到一个权限位为 000 的自有文件（读不了的文件，本进程非特权时连打开都过不去）。
/// 「别人的文件」那一半由属主判据拦，见 [`files_not_owned_by_the_caller_are_rejected`]。
#[test]
fn a_hard_link_to_a_file_outside_is_rejected() {
    let (_keep, secret) = outside_file(b"reachable only through the extra link");
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME, CORE);
    fs::hard_link(&secret, src.path().join("libcronet.so")).unwrap();
    let (core, before) = make_core_dir();

    let result = install(core.path(), src.path(), &sha256_hex(CORE));

    assert_eq!(
        result,
        Err(InstallResult::Read {
            name: "libcronet.so".into(),
            detail: "file has more than one link".into()
        })
    );
    assert_rejected_untouched(&result, core.path(), &before);

    // 核名字上的硬链接，哈希给对。
    let src = src_tempdir();
    fs::hard_link(&secret, src.path().join(SINGBOX_BIN_NAME)).unwrap();
    let hash = sha256_hex(b"reachable only through the extra link");
    let result = install(core.path(), src.path(), &hash);
    assert_eq!(
        result,
        Err(InstallResult::ReadSingbox(
            "file has more than one link".into()
        ))
    );
    assert_rejected_untouched(&result, core.path(), &before);
}

#[test]
fn a_hard_link_to_an_unreadable_file_is_rejected() {
    let (_keep, secret) = outside_file(b"mode 000");
    fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).unwrap();
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME, CORE);
    fs::hard_link(&secret, src.path().join("libcronet.so")).unwrap();
    let (core, before) = make_core_dir();
    let result = install(core.path(), src.path(), &sha256_hex(CORE));
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 属主判据：源目录与每个文件都必须属已鉴权的调用者所有。
///
/// 不提权造不出「别人的文件」，故反过来：文件都是本进程的，把**期望的调用者 uid** 换成别人
/// （本 uid + 1）—— 判据比的是这两个数，哪边不同都走同一条分支。
#[test]
fn files_not_owned_by_the_caller_are_rejected() {
    let (src, hash) = make_src_dir();
    let (core, before) = make_core_dir();
    let someone_else = me().map(|uid| uid + 1);

    let result = install_core_files(
        core.path(),
        src.path(),
        &hash,
        Platform::Linux,
        someone_else,
    );
    assert_eq!(
        result,
        Err(InstallResult::ReadDir("not owned by the caller".into()))
    );
    assert_rejected_untouched(&result, core.path(), &before);

    // 拿不到调用者身份：一律不收（不是「那就不判」）。
    let result = install_core_files(core.path(), src.path(), &hash, Platform::Linux, None);
    assert_eq!(
        result,
        Err(InstallResult::ReadDir("caller identity unavailable".into()))
    );
    assert_rejected_untouched(&result, core.path(), &before);

    // 正面对照：同一个源目录，调用者是自己就装得进去。
    install(core.path(), src.path(), &hash).unwrap();
}

/// 属主判据落在**每个文件**上，不只是目录：目录过了、文件属主不符照样拒。
///
/// 绕开目录那道判据的办法是直接问 `SrcDir`：用自己的 uid 打开目录，再把记下的调用者换掉。
#[test]
fn each_file_is_checked_against_the_caller_not_just_the_directory() {
    let (src, _) = make_src_dir();
    let mut dir = SrcDir::open(src.path(), me()).unwrap();
    dir.open_regular(SINGBOX_BIN_NAME).unwrap();

    dir.set_owner_uid_for_test(me().unwrap() + 1);
    assert_eq!(
        dir.open_regular(SINGBOX_BIN_NAME).unwrap_err().to_string(),
        "not owned by the caller"
    );
}

/// 「组与其他人不可写」：目录或文件对别人可写，摆进去的内容就不一定出自调用者之手。
#[test]
fn group_or_world_writable_sources_are_rejected() {
    let (core, before) = make_core_dir();

    for mode in [0o775, 0o757] {
        let (src, hash) = make_src_dir();
        fs::set_permissions(src.path(), fs::Permissions::from_mode(mode)).unwrap();
        let result = install(core.path(), src.path(), &hash);
        assert_eq!(
            result,
            Err(InstallResult::ReadDir("writable by group or others".into())),
            "目录 {mode:o}"
        );
        assert_rejected_untouched(&result, core.path(), &before);
    }

    for mode in [0o664, 0o646] {
        let (src, hash) = make_src_dir();
        fs::set_permissions(
            src.path().join("libcronet.so"),
            fs::Permissions::from_mode(mode),
        )
        .unwrap();
        let result = install(core.path(), src.path(), &hash);
        assert_eq!(
            result,
            Err(InstallResult::Read {
                name: "libcronet.so".into(),
                detail: "writable by group or others".into()
            }),
            "文件 {mode:o}"
        );
        assert_rejected_untouched(&result, core.path(), &before);

        let (src, hash) = make_src_dir();
        fs::set_permissions(
            src.path().join(SINGBOX_BIN_NAME),
            fs::Permissions::from_mode(mode),
        )
        .unwrap();
        let result = install(core.path(), src.path(), &hash);
        assert_eq!(
            result,
            Err(InstallResult::ReadSingbox(
                "writable by group or others".into()
            )),
            "核 {mode:o}"
        );
        assert_rejected_untouched(&result, core.path(), &before);
    }
}

/// 不是合法 UTF-8 的文件名（真文件系统上的）。macOS 的文件系统不让建这种名字，故只在 linux 跑。
#[cfg(target_os = "linux")]
#[test]
fn a_non_utf8_file_name_is_rejected() {
    use std::os::unix::ffi::OsStrExt;
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME, CORE);
    let name = std::ffi::OsStr::from_bytes(b"libcronet.so\xff");
    fs::write(src.path().join(name), b"x").unwrap();
    let (core, before) = make_core_dir();
    let result = install(core.path(), src.path(), &sha256_hex(CORE));
    assert_eq!(
        result,
        Err(InstallResult::ReadDir("entry name not allowed".into()))
    );
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 含路径分隔符的名字。`/` 进不了目录项；反斜杠在 unix 上是合法文件名字符、在 Windows 上是分隔符，
/// 这里能真造出来的就是它。
#[test]
fn a_name_containing_a_separator_is_rejected() {
    let (src, hash) = make_src_dir();
    put(src.path(), r"libcronet.so\b", b"x");
    let (core, before) = make_core_dir();
    let result = install(core.path(), src.path(), &hash);
    assert_eq!(
        result,
        Err(InstallResult::ReadDir("entry name not allowed".into()))
    );
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 单文件上限接在生产编排上：稀疏文件只占元数据，判的是 `fstat` 的大小，不会真去读 256 MiB。
#[test]
fn a_file_over_the_production_size_limit_is_rejected() {
    let (src, hash) = make_src_dir();
    let big = fs::OpenOptions::new()
        .write(true)
        .open(src.path().join("libcronet.so"))
        .unwrap();
    big.set_len(SRC_LIMITS.max_file_bytes + 1).unwrap();
    let (core, before) = make_core_dir();
    let result = install(core.path(), src.path(), &hash);
    assert_eq!(
        result,
        Err(InstallResult::Read {
            name: "libcronet.so".into(),
            detail: "size over limit".into()
        })
    );
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 落盘的是收下时那个句柄读到的字节：收下之后把源路径上的文件整个换掉，落盘的仍是原来的内容，
/// 且 sha256 比的也是原来的内容（换上去的那份哈希对不上，却不影响结果）。
///
/// 换法是「新文件 rename 到原名上」—— 路径指向了另一个 inode，这正是按路径回头再开一次会读到
/// 别的东西的情形。核与配套各换一个。
#[test]
fn bytes_written_are_the_ones_received_even_if_the_paths_are_swapped_afterwards() {
    let (src, hash) = make_src_dir();
    let received = receive_src(src.path(), &hash, Platform::Linux, me(), SRC_LIMITS).unwrap();

    for name in [SINGBOX_BIN_NAME, "libcronet.so"] {
        put(src.path(), "replacement", b"swapped in after receipt");
        fs::rename(src.path().join("replacement"), src.path().join(name)).unwrap();
        // 前置条件：路径上现在确实是另一份内容。
        assert_eq!(
            fs::read(src.path().join(name)).unwrap(),
            b"swapped in after receipt"
        );
    }

    let core = tempfile::tempdir().unwrap();
    install_received(core.path(), received).unwrap();

    assert_eq!(fs::read(core.path().join(SINGBOX_BIN_NAME)).unwrap(), CORE);
    assert_eq!(fs::read(core.path().join("libcronet.so")).unwrap(), CRONET);
}

/// 落盘的字节就是参与哈希的字节：收下之后把核**原地**改掉（同一个 inode，句柄读到的就是新
/// 内容）—— 写出去的会是改过的字节，所以哈希必须对不上，且受保护目录零改动。
///
/// 这条钉的是「哈希算的是写出去的那一批」：若哈希另算一遍（比如收的时候先算好），这里会把
/// 没校验过的字节装进去。
#[test]
fn bytes_changed_in_place_after_receipt_fail_the_hash_and_change_nothing() {
    let (src, hash) = make_src_dir();
    let received = receive_src(src.path(), &hash, Platform::Linux, me(), SRC_LIMITS).unwrap();

    put(
        src.path(),
        SINGBOX_BIN_NAME,
        b"tampered in place!!!!!!!!!!!",
    );

    let (core, before) = make_core_dir();
    let result = install_received(core.path(), received);
    assert_eq!(result, Err(InstallResult::HashMismatch));
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 配套文件在收下之后被原地加长到越过上限：落盘阶段按实际字节数再卡一次，
/// 且此时已写好的核的 `.new` 也一并清掉 —— 没有任何文件生效。
#[test]
fn a_sidecar_grown_past_the_limit_after_receipt_leaves_nothing_behind() {
    let src = src_tempdir();
    put(src.path(), SINGBOX_BIN_NAME, b"core");
    put(src.path(), "libcronet.so", &[0u8; 8]);
    let received = receive_src(
        src.path(),
        &sha256_hex(b"core"),
        Platform::Linux,
        me(),
        SMALL,
    )
    .unwrap();

    put(src.path(), "libcronet.so", &[0u8; 33]);

    let (core, before) = make_core_dir();
    let result = install_received(core.path(), received);
    assert_eq!(
        result,
        Err(InstallResult::Read {
            name: "libcronet.so".into(),
            detail: "size over limit".into()
        })
    );
    assert_rejected_untouched(&result, core.path(), &before);
}

/// 受保护目录里 `<name>.new` 的位置上若已有一个符号链接，写入不顺着它落到别处。
#[test]
fn a_planted_tmp_symlink_is_not_written_through() {
    let (_keep, victim) = outside_file(b"victim content");
    let (src, hash) = make_src_dir();
    let core = tempfile::tempdir().unwrap();
    symlink(&victim, core.path().join(format!("{SINGBOX_BIN_NAME}.new"))).unwrap();

    install(core.path(), src.path(), &hash).unwrap();

    assert_eq!(fs::read(&victim).unwrap(), b"victim content");
    let installed = core.path().join(SINGBOX_BIN_NAME);
    assert!(fs::symlink_metadata(&installed).unwrap().is_file());
    assert_eq!(fs::read(&installed).unwrap(), CORE);
}

/// 上一条在生产编排上成立有两道原因（安装开头收拾残留、新建临时件时不沿用既有对象），
/// 这里单独钉后一道：直接在一个已有符号链接的位置上新建临时件。
#[test]
fn creating_the_tmp_file_replaces_an_existing_link_instead_of_following_it() {
    use std::io::Write;
    let (_keep, victim) = outside_file(b"victim content");
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().join("sing-box.new");
    symlink(&victim, &tmp).unwrap();

    let mut out = create_private(&tmp).unwrap();
    out.write_all(b"new bytes").unwrap();
    drop(out);

    assert_eq!(fs::read(&victim).unwrap(), b"victim content");
    assert!(fs::symlink_metadata(&tmp).unwrap().is_file());
    assert_eq!(fs::read(&tmp).unwrap(), b"new bytes");
}

/// 顺序：写入期间 `.new` 只有本进程读得到，**比对通过之后**才放开权限。
///
/// 在写阶段与放开权限之间停下来看：哈希对得上，文件已写完，但权限仍是 0600；随后 `seal`
/// 才把它改成 0755。哈希对不上时写阶段直接失败，`.new` 不留 —— 两条合起来就是「没通过比对的
/// 字节从未以他人可读的形态出现过」。把放开权限挪到比对之前，第一组断言会红。
#[test]
fn tmp_files_stay_private_until_the_hash_has_been_checked() {
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let (src, hash) = make_src_dir();

    let core = tempfile::tempdir().unwrap();
    let received = receive_src(src.path(), &hash, Platform::Linux, me(), SRC_LIMITS).unwrap();
    let mut staged = write_tmp_files(core.path(), &received).unwrap();
    assert_eq!(staged.len(), 2, "前置条件：核与配套库各一个 `.new`");
    for item in &staged {
        assert!(item.file.is_some(), "{}：写阶段不该关句柄", item.name);
        assert_eq!(mode(&item.tmp), 0o600, "{}：比对通过后、放开前", item.name);
    }
    for item in &mut staged {
        seal(item).unwrap();
        assert_eq!(mode(&item.tmp), 0o755, "{}：放开后", item.name);
    }

    // 哈希对不上：写阶段失败，目录里一个 `.new` 都不剩。
    let core = tempfile::tempdir().unwrap();
    let received = receive_src(
        src.path(),
        &"0".repeat(64),
        Platform::Linux,
        me(),
        SRC_LIMITS,
    )
    .unwrap();
    assert_eq!(
        write_tmp_files(core.path(), &received).err(),
        Some(InstallResult::HashMismatch)
    );
    assert!(snapshot(core.path()).is_empty());
}
