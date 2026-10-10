use super::*;

use crate::test_support::TestDir;

fn tmpdir() -> TestDir {
    TestDir::new("polaris-core-paths-test-")
}

// ── 路径解析纯函数 ──

#[test]
fn core_filename_is_platform_specific() {
    assert_eq!(core_filename_for("windows"), "sing-box.exe");
    assert_eq!(core_filename_for("linux"), "sing-box");
    assert_eq!(core_filename_for("macos"), "sing-box");
    // 未知 OS 回落 Unix 名（不 panic、不返空）。
    assert_eq!(core_filename_for("freebsd"), "sing-box");
}

/// **跨 crate 等值门（P4 · 拍板 2）**：核文件名在三个 crate 里各有一份字面量，`src-tauri` 是
/// 唯一同时看得见它们的地方（依赖方向 `src-tauri → polaris-helper` / `polaris-helper-client`，
/// 先例见 `runtime/proxy/platform_contracts.rs` 直调 `polaris_helper::platform::windows::*`）。
///
/// 为什么不合并成一个真值源：`polaris-helper` 是特权 daemon、`polaris-helper-client` 是装卸壳，
/// 两者都不该反向依赖 app；为一个文件名在 crate 间新开一个共享 crate 不划算。**改用门收敛**。
///
/// 漏掉这道门的后果不是编译错而是静默失效：
/// - helper 侧 `SINGBOX_BIN_NAME_WIN` 与 app 侧不一致 ⇒ `install_core_files` 校验/落盘读的是
///   另一个名字，hash 校验与真正落盘的字节脱钩；
/// - helper-client 侧 `WIN_CORE_BIN_NAME` 与 app 侧不一致 ⇒ 安装脚本烧进 ImagePath 的
///   `--singbox` 与 app 侧 `protected_core_path_in` 算出的 dest 是两个文件，reconcile 永远
///   判「受保护核不存在」⇒ 每次起核白推 80MB，而 helper exec 的是另一份。
#[test]
fn win_core_binary_names_agree_across_crates() {
    assert_eq!(
        polaris_helper::core_install::SINGBOX_BIN_NAME_WIN,
        core_filename_for("windows"),
        "helper 侧 Windows 核名与 app 侧真值源分叉"
    );
    assert_eq!(
        polaris_helper::core_install::SINGBOX_BIN_NAME,
        core_filename_for("linux"),
        "helper 侧 unix 核名与 app 侧真值源分叉"
    );
    assert_eq!(
        polaris_helper_client::manager::WIN_CORE_BIN_NAME,
        core_filename_for("windows"),
        "安装脚本 seed/ImagePath 用的核名与 app 侧真值源分叉"
    );
    assert_eq!(
        polaris_helper_client::manager::WIN_CORE_SIDECAR_NAME,
        core_sidecar_filename_for("windows").expect("windows 必有 cronet sidecar"),
        "安装脚本 seed 的 cronet 名与 app 侧真值源分叉"
    );
    // helper 的**起核前 ACL 自检**用这个名字拼出「同目录的配套 DLL」这一兜底取材对象。
    // 它必须住在 `core_install`（无 cfg）而不是 `platform::windows::coreacl`：后者的门是
    // `cfg(any(target_os = "windows", test))`，那个 `test` 只在 polaris-helper 自己的 test 编译期
    // 成立 —— src-tauri 在 Linux 上跑测试时 helper 是普通依赖（`cfg(test)` 关），整个
    // `platform::windows` 不入编译 ⇒ 放那儿的字面量**物理上进不了本门**。
    //
    // 漏掉这条腿的后果同样是静默：cronet 升版改名时上面那两条把 app 侧与安装脚本侧一起改了，
    // helper 自检这份不会 ⇒ 它去问一个不存在的路径 ⇒ 落 `unreadable` ⇒ 只 warn、起核照常 ⇒
    // 真正躺在 exec 目录里的那个 DLL（spec §2.4：exe 目录是 DLL 搜索第 7 位，先于 CWD）从此无人检查。
    assert_eq!(
        polaris_helper::core_install::CRONET_DLL_NAME_WIN,
        core_sidecar_filename_for("windows").expect("windows 必有 cronet sidecar"),
        "helper ACL 自检取材面的 cronet 名与 app 侧真值源分叉"
    );
    // 正面断言：上面四条全是「A == B」，若两侧同时被改成同一个错值（例如都写成 "sing-box"），
    // 它们仍然全绿。故再钉一次绝对值——Windows 核必须带 .exe，否则 CreateProcess 起不来。
    assert_eq!(core_filename_for("windows"), "sing-box.exe");
    assert_ne!(
        core_filename_for("windows"),
        core_filename_for("linux"),
        "两平台核名若相同，上面的 win/unix 分派全部失去意义"
    );
}

/// **跨 crate 等值门：helper token 的文件名。**
///
/// 同一个文件由两侧各写一份字面量：`polaris-helper-client` 的安装脚本**写**它
/// （mac `$SUPPORT/helper.token`、win `C:\ProgramData\Polaris\helper.token`），
/// `polaris-helper` 的 daemon **读**它（`token::TOKEN_FILENAME`）。两个 crate 互不依赖
/// （helper-client 是 app 侧装卸壳，helper 是特权 daemon，谁也不该反向依赖对方），
/// `src-tauri` 是唯一同时看得见它们的地方 —— 与上面那条核名门同一形态、同一理由。
///
/// 漂移后果**静默且刺眼**：脚本写 A、daemon 读 B ⇒ daemon 永远读到空 token ⇒ 每次连接回
/// `ERR auth`，而**安装本身报成功**（脚本每一步退出码都是 0）。表现是「装好了但一直未就绪」，
/// 与「helper 没起来」肉眼无法区分。
#[test]
fn helper_token_filename_agrees_across_crates() {
    assert_eq!(
        polaris_helper_client::manager::HELPER_TOKEN_FILENAME,
        polaris_helper::token::TOKEN_FILENAME,
        "安装脚本写的 token 文件名与 daemon 读的分叉 ⇒ 恒 ERR auth，而安装报成功"
    );
    // 正面断言：上面是「A == B」，两侧同时被改成同一个错值仍然全绿。故再钉一次绝对值 ——
    // 它同时是存量机器上已经躺在磁盘里的那个名字，改它等于让所有存量安装失联。
    assert_eq!(polaris_helper::token::TOKEN_FILENAME, "helper.token");
}

#[test]
fn core_sidecar_filename_matches_packaged_cronet() {
    assert_eq!(core_sidecar_filename_for("windows"), Some("libcronet.dll"));
    assert_eq!(core_sidecar_filename_for("linux"), Some("libcronet.so"));
    assert_eq!(core_sidecar_filename_for("macos"), None);
    assert_eq!(core_sidecar_filename_for("freebsd"), None);
}

// ── 旧安装残留清理 ──

/// 造一份配置根：里面是旧版本的全部内核残留形态，外加必须保留的无关数据。
fn seed_legacy_layout(root: &Path) {
    let core_update = root.join("core_update");
    std::fs::create_dir_all(&core_update).unwrap();
    std::fs::write(core_update.join("sing-box"), b"old core").unwrap();
    std::fs::write(core_update.join("sing-box.bak"), b"older core").unwrap();
    std::fs::write(core_update.join(".core-seed.json"), b"{}").unwrap();
    std::fs::write(core_update.join("libcronet.so"), b"cronet").unwrap();
    let extract = root.join("core-staged").join("extract-1-0").join("x");
    std::fs::create_dir_all(&extract).unwrap();
    std::fs::write(extract.join("sing-box"), b"staged core").unwrap();
    // 无关数据：清理不得触碰。
    std::fs::write(root.join("config.json"), b"{}").unwrap();
    std::fs::write(root.join("update-state.json"), b"{}").unwrap();
    std::fs::create_dir_all(root.join("rules")).unwrap();
    std::fs::write(root.join("rules").join("geoip-cn.srs"), b"rules").unwrap();
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn legacy_cleanup_removes_exactly_the_legacy_entries() {
    let tmp = tmpdir();
    let root = tmp.path().join("polaris");
    std::fs::create_dir_all(&root).unwrap();
    seed_legacy_layout(&root);
    // 递送暂存目录的残留形态之一：一个普通文件占了这个名字。
    std::fs::write(root.join("core-promote"), b"stray").unwrap();
    // 配置根之外、名字相同的目录：清理只认配置根的直接子项。
    let outside = tmp.path().join("core_update");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("sing-box"), b"not ours").unwrap();

    let mut removed = remove_legacy_core_state(&root);
    removed.sort();
    assert_eq!(
        removed,
        vec![
            root.join("core-promote"),
            root.join("core-staged"),
            root.join("core_update"),
        ]
    );
    assert_eq!(
        names_in(&root),
        ["config.json", "rules", "update-state.json"],
        "配置根下只应少掉三项旧残留，其余原样"
    );
    assert_eq!(
        std::fs::read(root.join("rules").join("geoip-cn.srs")).unwrap(),
        b"rules"
    );
    assert_eq!(
        std::fs::read(outside.join("sing-box")).unwrap(),
        b"not ours"
    );

    // 幂等：第二次什么都不删，也不报错。
    assert!(remove_legacy_core_state(&root).is_empty());
}

/// 反向对照：没有任何残留的配置根，清理必须零动作（否则上一条的「删掉三项」可能只是
/// 「见什么删什么」）。
#[test]
fn legacy_cleanup_is_a_noop_on_a_clean_root() {
    let tmp = tmpdir();
    let root = tmp.path().join("polaris");
    std::fs::create_dir_all(root.join("rules")).unwrap();
    std::fs::write(root.join("config.json"), b"{}").unwrap();
    // 名字只是前缀相近的条目不算残留。
    std::fs::create_dir_all(root.join("core_update.keep")).unwrap();
    std::fs::write(root.join("core-staged.txt"), b"x").unwrap();

    assert!(remove_legacy_core_state(&root).is_empty());
    assert_eq!(
        names_in(&root),
        [
            "config.json",
            "core-staged.txt",
            "core_update.keep",
            "rules"
        ]
    );
    // 配置根本身不存在同样是零动作。
    assert!(remove_legacy_core_state(&tmp.path().join("missing")).is_empty());
}

#[cfg(unix)]
#[test]
fn legacy_cleanup_unlinks_symlinks_without_touching_their_targets() {
    use std::os::unix::fs::symlink;

    let tmp = tmpdir();
    let root = tmp.path().join("polaris");
    std::fs::create_dir_all(&root).unwrap();
    // 配置根之外的受害者：一个目录、一个文件。
    let victim_dir = tmp.path().join("victim-dir");
    std::fs::create_dir_all(victim_dir.join("nested")).unwrap();
    std::fs::write(victim_dir.join("keep.txt"), b"keep").unwrap();
    std::fs::write(victim_dir.join("nested").join("deep.txt"), b"deep").unwrap();
    let victim_file = tmp.path().join("victim-file");
    std::fs::write(&victim_file, b"file").unwrap();

    // 顶层条目本身是链接（指目录、指文件）。
    symlink(&victim_dir, root.join("core_update")).unwrap();
    symlink(&victim_file, root.join("core-promote")).unwrap();
    // 真目录里面夹着链接（指目录、指文件、断链）。
    let staged = root.join("core-staged");
    std::fs::create_dir_all(&staged).unwrap();
    symlink(&victim_dir, staged.join("link-to-dir")).unwrap();
    symlink(&victim_file, staged.join("link-to-file")).unwrap();
    symlink(tmp.path().join("nowhere"), staged.join("dangling")).unwrap();

    let removed = remove_legacy_core_state(&root);
    assert_eq!(removed.len(), 3, "三个顶层条目都应被删掉：{removed:?}");
    assert!(names_in(&root).is_empty());

    assert_eq!(names_in(&victim_dir), ["keep.txt", "nested"]);
    assert_eq!(std::fs::read(victim_dir.join("keep.txt")).unwrap(), b"keep");
    assert_eq!(
        std::fs::read(victim_dir.join("nested").join("deep.txt")).unwrap(),
        b"deep"
    );
    assert_eq!(std::fs::read(&victim_file).unwrap(), b"file");
}

/// 配置根自己是一个链接时，被清理的仍然只是它名下的三项，链接目标里的其它内容不动。
#[cfg(unix)]
#[test]
fn legacy_cleanup_through_a_symlinked_root_stays_inside_it() {
    use std::os::unix::fs::symlink;

    let tmp = tmpdir();
    let real = tmp.path().join("real-root");
    std::fs::create_dir_all(&real).unwrap();
    seed_legacy_layout(&real);
    let link = tmp.path().join("polaris");
    symlink(&real, &link).unwrap();

    let removed = remove_legacy_core_state(&link);
    assert_eq!(removed.len(), 2);
    assert_eq!(
        names_in(&real),
        ["config.json", "rules", "update-state.json"]
    );
}

#[test]
fn direct_child_rejects_names_that_leave_the_config_root() {
    let root = Path::new("/cfg/polaris");
    assert_eq!(
        direct_child(root, "core_update"),
        Some(root.join("core_update"))
    );
    for escaping in [
        "",
        ".",
        "..",
        "../core_update",
        "core_update/..",
        "core_update/sing-box",
        "core_update/",
        "/core_update",
        "/etc",
    ] {
        assert_eq!(
            direct_child(root, escaping),
            None,
            "{escaping:?} 不是单个普通路径分量"
        );
    }
    // 清理名单里的每一项都必须过这道判据，否则那一项在运行期会被静默跳过。
    for name in LEGACY_CORE_STATE_NAMES {
        assert!(direct_child(root, name).is_some(), "{name}");
    }
}

/// 清理名单的正面钉死：少一项 = 那类残留永远留在用户盘上，且没有任何别的测试会红。
#[test]
fn legacy_cleanup_list_is_pinned() {
    assert_eq!(
        LEGACY_CORE_STATE_NAMES,
        ["core_update", "core-staged", "core-promote"]
    );
}
