//! Polaris 桌面可执行入口 —— **薄壳**：除进程属性外不含任何业务逻辑。
//!
//! 应用装配（18 个 `mod`、插件注册、`setup`、command 注册表、`RunEvent` 循环）全部在
//! `lib.rs`，桌面 bin 与移动端 `cdylib` 共用同一份（理由见 `lib.rs` 模块文档）。
//! 本文件只做桌面 bin target 独有的两件事：`windows_subsystem` 属性、调 `run()`。

// GUI 子系统属性只对 **bin target** 有意义（lib 上写它是 no-op），故它留在这里而不随装配下沉。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    polaris_lib::run();
}
