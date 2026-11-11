use super::*;

// ── 路径解析纯函数 ──

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
        polaris_helper_proto::core_payload::WIN_CORE_FILENAME,
        core_filename_for("windows"),
        "helper 侧 Windows 核名与 app 侧真值源分叉"
    );
    assert_eq!(
        polaris_helper_proto::core_payload::UNIX_CORE_FILENAME,
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
        polaris_helper_proto::core_payload::WIN_SIDECAR_FILENAME,
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
