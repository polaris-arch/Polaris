#![allow(clippy::too_many_lines)]

use super::*;
use crate::core_install::SINGBOX_BIN_NAME;
use std::fs::write;
use std::os::unix::fs::PermissionsExt;
use tempfile::tempdir;

fn me() -> u32 {
    nix::unistd::Uid::current().as_raw()
}

/// 源目录：权限定成 0755（`tempdir()` 建出来的随 umask，`umask 002` 下组可写，会被属主判据拒）。
fn src_tempdir() -> tempfile::TempDir {
    let dir = tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

/// 以本进程为调用者的 install-core（单测里「已鉴权的对端」就是自己）。
fn install_core(core_dir: Option<&Path>, src_dir: &str, want_hash: &str) -> InstallResult {
    super::install_core(core_dir, src_dir, want_hash, me())
}

/// 造一个 srcDir：sing-box + libcronet.so 等配套（对照 Go 注释 :182 的真实核形态）。
fn make_src_dir(dir: &Path, singbox: &[u8], extras: &[(&str, &[u8])]) {
    // 权限显式定：属主判据要求组与其他人不可写，不能随测试环境的 umask 漂。
    let put = |name: &str, data: &[u8], mode: u32| {
        write(dir.join(name), data).unwrap();
        std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(mode)).unwrap();
    };
    // 生产核是可执行二进制。
    put(SINGBOX_BIN_NAME, singbox, 0o755);
    for (name, data) in extras {
        put(name, data, 0o644);
    }
}

/// 测试本地 sha256（与生产的 sha256_hex 独立实现，避免循环依赖断言）。
fn sha256_hex_local(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

#[test]
fn coredir_unset_returns_err() {
    // Go :184-185: coreDir == "" → "ERR coredir-unset"
    let r = install_core(None, "/tmp/src", &"a".repeat(64));
    assert_eq!(r, InstallResult::CoreDirUnset);
    assert_eq!(r.to_wire_line(), "ERR coredir-unset");
}

#[test]
fn bad_args_when_src_empty() {
    let r = install_core(Some(Path::new("/tmp/core")), "", &"a".repeat(64));
    assert_eq!(r, InstallResult::BadArgs);
}

#[test]
fn bad_args_when_hash_wrong_length() {
    // Go :187: len(wantHash) != 64 → bad-args
    let r = install_core(Some(Path::new("/tmp/core")), "/tmp/src", "abc");
    assert_eq!(r, InstallResult::BadArgs);
}

#[test]
fn bad_args_when_hash_not_hex() {
    // 64 字符但非 hex（proto crate 的 is_valid_sha256_hex 拒绝）。
    let r = install_core(Some(Path::new("/tmp/core")), "/tmp/src", &"z".repeat(64));
    assert_eq!(r, InstallResult::BadArgs);
}

#[test]
fn read_singbox_failure_when_missing() {
    // srcDir 存在但无 sing-box 文件。
    let src = src_tempdir();
    let core = tempdir().unwrap();
    let r = install_core(
        Some(core.path()),
        src.path().to_str().unwrap(),
        &"a".repeat(64),
    );
    let wire = r.to_wire_line();
    match r {
        // io::Error 详情不含 "sing-box"（是 OS 错误文本），只验证变体命中。
        InstallResult::ReadSingbox(_) => {}
        other => panic!("expected ReadSingbox, got {other:?}"),
    }
    assert!(
        wire.starts_with("ERR read-singbox"),
        "wire 应以 ERR read-singbox 开头，got {wire}"
    );
}

#[test]
fn hash_mismatch_when_content_changed() {
    let src = src_tempdir();
    let core = tempdir().unwrap();
    make_src_dir(src.path(), b"real-sing-box-binary", &[]);
    // 故意给错 hash（内容对应的真实 hash 与 wantHash 不符）。
    let r = install_core(
        Some(core.path()),
        src.path().to_str().unwrap(),
        &"0".repeat(64),
    );
    assert_eq!(r, InstallResult::HashMismatch);
    assert_eq!(r.to_wire_line(), "ERR hash-mismatch");
}

#[test]
fn successful_install_copies_singbox_and_extras() {
    let src = src_tempdir();
    let core = tempdir().unwrap();
    let sb = b"#!bin\nsing-box-binary-v1.2.3";
    let cronet = b"cronet shared lib bytes";
    make_src_dir(src.path(), sb, &[("libcronet.so", cronet)]);
    let want_hash = sha256_hex_local(sb);
    let r = install_core(Some(core.path()), src.path().to_str().unwrap(), &want_hash);
    assert_eq!(r, InstallResult::Installed);
    assert!(r.is_ok());
    assert_eq!(r.to_wire_line(), "OK installed");
    // 校验落盘内容。
    assert_eq!(
        std::fs::read(core.path().join(SINGBOX_BIN_NAME)).unwrap(),
        sb
    );
    assert_eq!(
        std::fs::read(core.path().join("libcronet.so")).unwrap(),
        cronet
    );
    // 可执行权限校验（0755）。
    let mode = std::fs::metadata(core.path().join(SINGBOX_BIN_NAME))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755, "sing-box 须 0755 可执行");
}

#[test]
fn hash_case_insensitive_match() {
    // Go strings.EqualFold 大小写不敏感。wantHash 大写也应匹配小写计算结果。
    let src = src_tempdir();
    let core = tempdir().unwrap();
    let sb = b"binary";
    make_src_dir(src.path(), sb, &[]);
    let want_upper = sha256_hex_local(sb).to_uppercase();
    let r = install_core(Some(core.path()), src.path().to_str().unwrap(), &want_upper);
    assert_eq!(r, InstallResult::Installed);
}

/// 清理：受管目录里凡不是本次装入的**常规文件**都删；符号链接与子目录原样留着，
/// 链接指向的目标不受影响。
#[test]
fn install_removes_every_unlisted_regular_file_and_nothing_else() {
    let src = src_tempdir();
    let core = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let target = outside.path().join("link-target");
    write(&target, b"outside the managed dir").unwrap();

    // 本次会装入的两个名字（预置旧内容）。
    write(core.path().join(SINGBOX_BIN_NAME), b"OLD singbox").unwrap();
    write(core.path().join("libcronet.so"), b"OLD cronet").unwrap();
    // 不属于本次安装的常规文件：旧配套（名字在白名单内）、任意名字、上次中断留下的临时件。
    for stale in ["libcronet.so.119", "libstale.so", "helper.bin", "x.new"] {
        write(core.path().join(stale), b"stale").unwrap();
    }
    // 不是常规文件的两样。
    std::os::unix::fs::symlink(&target, core.path().join("a-link")).unwrap();
    std::fs::create_dir(core.path().join("a-dir")).unwrap();
    write(core.path().join("a-dir").join("inner"), b"inner").unwrap();

    let sb = b"NEW singbox";
    make_src_dir(src.path(), sb, &[("libcronet.so", b"NEW cronet")]);
    let r = install_core(
        Some(core.path()),
        src.path().to_str().unwrap(),
        &sha256_hex_local(sb),
    );
    assert_eq!(r, InstallResult::Installed);

    let mut left: Vec<String> = std::fs::read_dir(core.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(
        left,
        vec!["a-dir", "a-link", "libcronet.so", SINGBOX_BIN_NAME]
    );
    assert_eq!(
        std::fs::read(core.path().join(SINGBOX_BIN_NAME)).unwrap(),
        b"NEW singbox"
    );
    assert_eq!(
        std::fs::read(core.path().join("libcronet.so")).unwrap(),
        b"NEW cronet"
    );
    // 链接本身还在、仍是链接，目标原样；子目录里的东西没被碰。
    assert!(std::fs::symlink_metadata(core.path().join("a-link"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read(&target).unwrap(), b"outside the managed dir");
    assert_eq!(
        std::fs::read(core.path().join("a-dir").join("inner")).unwrap(),
        b"inner"
    );
}

/// 受管目录路径本身是符号链接时不清理（按目录 fd 打开，不跟随）。
#[test]
fn prune_does_not_follow_a_symlinked_core_dir() {
    let real = tempdir().unwrap();
    write(real.path().join("stale.bin"), b"stale").unwrap();
    let holder = tempdir().unwrap();
    let link = holder.path().join("core-link");
    std::os::unix::fs::symlink(real.path(), &link).unwrap();

    prune_unlisted_regular_files(&link, &[]);
    assert!(real.path().join("stale.bin").exists());
    // 正面对照：给真实路径就删。
    prune_unlisted_regular_files(real.path(), &[]);
    assert!(!real.path().join("stale.bin").exists());
}

/// 被拒的请求不触发清理：受管目录里的旧文件原样留着。
#[test]
fn rejected_install_leaves_the_core_dir_untouched() {
    let src = src_tempdir();
    let core = tempdir().unwrap();
    write(core.path().join("previous.bin"), b"previous").unwrap();
    let sb = b"bin";
    make_src_dir(src.path(), sb, &[]);
    // srcDir 里的子目录不再被跳过：白名单外的名字，整单拒掉。
    std::fs::create_dir(src.path().join("subdir")).unwrap();
    let r = install_core(
        Some(core.path()),
        src.path().to_str().unwrap(),
        &sha256_hex_local(sb),
    );
    assert_eq!(r.to_wire_line(), "ERR readdir entry name not allowed");
    let left: Vec<String> = std::fs::read_dir(core.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, vec!["previous.bin"]);
}

#[test]
fn read_dir_failure_when_src_missing() {
    let core = tempdir().unwrap();
    // 先打开源目录，再谈其中的文件：srcDir 不存在 → readdir。
    let r = install_core(
        Some(core.path()),
        "/nonexistent/src/dir/xyz",
        &"a".repeat(64),
    );
    match r {
        InstallResult::ReadDir(_) => {}
        other => panic!("expected ReadDir for missing srcDir, got {other:?}"),
    }
}

/// 源文件属主判据接在 linux 入口上：调用者 uid 不是文件属主 → 整单拒，受管目录不动。
#[test]
fn install_refuses_sources_not_owned_by_the_caller() {
    let src = src_tempdir();
    let core = tempdir().unwrap();
    write(core.path().join("previous.bin"), b"previous").unwrap();
    let sb = b"bin";
    make_src_dir(src.path(), sb, &[("libcronet.so", b"lib")]);
    let r = super::install_core(
        Some(core.path()),
        src.path().to_str().unwrap(),
        &sha256_hex_local(sb),
        me() + 1,
    );
    assert_eq!(r.to_wire_line(), "ERR readdir not owned by the caller");
    let left: Vec<String> = std::fs::read_dir(core.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, vec!["previous.bin"]);
}

#[test]
fn wire_line_for_all_outcomes() {
    // 锁住 wire 形态（对照 Go 源各 return 分支的字符串；委托公共 to_wire_line，输出应逐字同）。
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
}

/// 静态断言：is_ok 仅对 Installed 真。
#[test]
fn is_ok_predicate() {
    assert!(InstallResult::Installed.is_ok());
    assert!(!InstallResult::BadArgs.is_ok());
    assert!(!InstallResult::HashMismatch.is_ok());
}
