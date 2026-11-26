//! Android release 签名密钥不入库：仓库根 `.gitignore` 必须覆盖密钥类文件。
//!
//! `.gitignore` 那条断言的期望值从 gradle 里那行默认密钥库路径**反推**扩展名，不在本文件
//! 另写一份字面量：改路径与改忽略模式必须一起改，只改一边就红。
// 源码文本门：三平台源码逐字一致（`.gitattributes` eol=lf），只在必跑的 Linux 腿执行一次。
#![cfg(target_os = "linux")]

use std::collections::BTreeSet;

/// Android 应用工程的 Gradle 脚本（相对 workspace 根）。
const GRADLE_APP: &str = "src-tauri/gen/android/app/build.gradle.kts";
/// 仓库根的 `.gitignore`（射程覆盖全仓，故密钥模式写在这里而不是 `gen/android/` 那份）。
const GITIGNORE: &str = ".gitignore";

/// 必须被仓库根 `.gitignore` 覆盖的密钥类文件模式。
const KEY_MATERIAL_IGNORES: &[&str] = &[
    "*.jks",
    "*.keystore",
    "keystore.properties",
    "key.properties",
];

/// 剥掉 `#` 行注释（ProGuard 与 `.gitignore` 的唯一注释形态），保行结构。
fn strip_hash_comments(src: &str) -> String {
    src.lines()
        .map(|line| match line.find('#') {
            Some(i) => line[..i].to_string(),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 读 `build.gradle.kts` 并剥注释；哨兵串挑的是这个文件独有的那行本地 aar 装配。
fn gradle_source() -> String {
    polaris_source_probe::mask_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(GRADLE_APP),
        GRADLE_APP,
        "implementation(files(\"libs/libbox.aar\"))",
    ))
}

/// 密钥类文件被仓库根 `.gitignore` 覆盖 —— 扩展名从 gradle 那条默认路径反推。
#[test]
fn key_material_can_never_enter_the_repository() {
    let gradle = gradle_source();
    let at = gradle.find("/.polaris/android/").unwrap_or_else(|| {
        panic!(
            "{GRADLE_APP}：找不到默认密钥库路径 `/.polaris/android/…` —— \
             约定位置改了。改路径就要一并改 `.gitignore` 的模式，本门正是为此存在。"
        )
    });
    let rest = &gradle[at..];
    let end = rest
        .find('"')
        .unwrap_or_else(|| panic!("{GRADLE_APP}：默认密钥库路径的字符串没有收尾引号。"));
    let default_path = &rest[..end];
    let (_, extension) = default_path.rsplit_once('.').unwrap_or_else(|| {
        panic!("{GRADLE_APP}：默认密钥库路径 `{default_path}` 没有扩展名，反推不出忽略模式。")
    });

    let ignore = strip_hash_comments(&polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(GITIGNORE),
        GITIGNORE,
        "!/resources/data/",
    ));
    let entries: BTreeSet<&str> = ignore
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    let derived = format!("*.{extension}");
    assert!(
        entries.contains(derived.as_str()),
        "{GITIGNORE}：没有忽略 `{derived}`，而 gradle 的默认密钥库正是 `{default_path}`。\
         签名密钥一旦入库，撤销它的成本是重新签一版并让所有已装用户手动重装。"
    );
    for pattern in KEY_MATERIAL_IGNORES {
        assert!(
            entries.contains(pattern),
            "{GITIGNORE}：没有忽略 `{pattern}`。口令按裁定只走环境变量、绝不落盘，\
             但「不落盘」这件事需要一条守卫兜住手滑 —— 这就是那条守卫。"
        );
    }
}
