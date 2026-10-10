//! polaris-updater — 应用自更新的纯逻辑。
//!
//! - [`state`]：弹窗更新 UI 状态机。
//! - [`version`]：semver 比较。
//! - [`github`]：GitHub release JSON → 应用更新信息 + 平台资产选择。
//! - [`manifest`]：release 清单解析的错误类型。
//! - [`verify`]：SHA256 校验 + 「临时文件 → rename 就位」落位。
//! - [`popup`]：mini 更新弹窗会话。
//! - [`core_build`]：内核版本行解析（只读展示用）。
//!
//! ## 纯逻辑纪律（不触碰宿主网络/FS）
//!
//! 文件系统操作经 trait 抽象（[`traits::UpdateFs`]），测试注入 mock；**绝不在本 crate 内发起
//! 真实 HTTP**。SHA256 校验、rename 编排、版本比较均为纯函数，可直接单测。

#![forbid(unsafe_code)]
// clippy 零警告（对齐移植纪律）：workspace 无集中 [workspace.lints]，与同级 crate（helper-linux /
// net-stack 等）同口径——只 forbid(unsafe_code)，clippy::all 默认级零警告即达标（不 escalate 到
// pedantic/nursery，避免与同级 crate 的 lint 严格度漂移）。

pub mod core_build;
pub mod github;
pub mod manifest;
pub mod popup;
pub mod state;
pub mod traits;
pub mod verify;
pub mod version;

pub use core_build::extract_version_token;
pub use github::{
    check_app_update, check_app_update_release_only, find_suitable_update_asset,
    github_releases_api_url, parse_asset_digest, resolve_current_app_release, strip_v,
    AppUpdateCheck, AppUpdateInfo, AssetArch, AssetPlatform, GithubAsset, GithubRelease,
    ANDROID_APK_SUFFIX, ANDROID_ARMV7_APK_SUFFIX, ANDROID_UNIVERSAL_APK_SUFFIX, APP_UPDATE_REPO,
};
pub use manifest::ManifestError;
pub use popup::{
    popup_height_for, PopupAction, PopupBootstrap, PopupSession, PopupTransport, UpdateErr,
    UpdateErrCode, UpdatePopupState, DONE_AUTO_CLOSE_MS, NO_UPDATE_AUTO_CLOSE_MS, POPUP_WIDTH,
};
pub use state::{PopupPhase, UpdateState, UpdateStateError};
pub use traits::UpdateFs;
pub use verify::{sha256_hex, VerifyError};
pub use version::ParseVersionError;
