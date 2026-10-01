//! install-core 的 mac wire 适配 —— 公共核心见 [`crate::core_install`]（同 crate 普通模块，三平台共享）。
//!
//! ## 本模块只剩什么
//!
//! 合并成单 crate 后，原「re-export 公共层符号」的转发块已删（同 crate 内 `crate::core_install::*`
//! 直接可达，转发纯属噪音）。本模块只留 [`to_response`] —— mac 调用点（`handler.rs` 的
//! `handle_install_core`）的就近入口。**它已不含映射判据**：那张手写表在 P4 收敛到公共
//! [`InstallResult::to_response`](crate::core_install::InstallResult::to_response) 的 wire 往返，
//! 本模块因此退化成一层命名转发（留着是为了不动 mac 调用点与它的既有单测）。
//!
//! ## mac 专属（本模块外）
//!
//! mac 的 xattr 清 quarantine + codesign adhoc 签名在 `handler.rs` 的 `handle_install_core`
//! 文件就位后触发（`helper.go:195-196`），不经本模块。

use crate::core_install::InstallResult;
use polaris_helper_proto::Response;

/// 把 [`InstallResult`] 转换成 wire [`Response`]（mac 调用点的就近入口）。
///
/// 用自由函数而非 `From` trait —— [`Response`] 是 proto crate 的类型，orphan rule 禁止
/// `impl From<InstallResult> for Response`。
///
/// **本函数不再持有映射判据**：原先这里是一张手写的 `variant → ErrorCode` 表，与公共
/// [`InstallResult::to_wire_line`](crate::core_install::InstallResult::to_wire_line) 是「结果
/// 相同、机制不同」的两份同语义映射 —— `to_wire_line` 一改只流向走 wire 往返的 Windows 侧，
/// 不流向这张表，单向漂移。现统一委托
/// [`InstallResult::to_response`](crate::core_install::InstallResult::to_response)（三平台唯一
/// 一份，走 wire 往返）。收敛前出过逐 variant 的等价收据（11/11 逐字相同，含 detail），
/// 无损性由 `wire_roundtrip_is_lossless_for_every_variant` 继续钉住。
#[must_use]
pub fn to_response(r: InstallResult) -> Response {
    r.to_response()
}

#[cfg(test)]
mod tests;
