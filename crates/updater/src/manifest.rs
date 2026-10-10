//! release 清单解析的错误类型。

use thiserror::Error;

/// 清单解析错误。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    /// JSON 解析失败（= 上游 `JSON.parse(data)` catch → `解析 GitHub 响应失败`）。
    #[error("manifest JSON parse error: {0}")]
    ParseJson(String),
}
