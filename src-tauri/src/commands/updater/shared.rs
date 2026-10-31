//! App 更新命令共用的窄原语。

use super::{GITHUB_FETCH_TIMEOUT_MS, MAX_GITHUB_JSON_BYTES};
use crate::runtime::http::{app_user_agent, CoreDownloader, SystemDnsLookup};
use crate::runtime::AppRuntime;
use polaris_net_stack::safe_redirect::{
    safe_redirect_fetch_until, HttpClient, SafeRedirectFetchOptions,
};
use polaris_net_stack::ssrf::DnsLookup;
use polaris_updater::github::{github_releases_api_url, GithubRelease};
use serde::de::{IgnoredAny, SeqAccess, Visitor};
use serde::Deserializer;
use std::collections::HashSet;
use std::time::{Duration, Instant};

pub(super) const GITHUB_RELEASES_PER_PAGE: usize = 100;

#[derive(Clone, Copy)]
pub(super) struct ReleaseFetchLimits {
    pub(super) max_pages: usize,
    pub(super) max_entries: usize,
    pub(super) max_bytes: usize,
}

pub(super) const RELEASE_FETCH_LIMITS: ReleaseFetchLimits = ReleaseFetchLimits {
    max_pages: 20,
    max_entries: 2_000,
    max_bytes: MAX_GITHUB_JSON_BYTES,
};

/// 构造生产下载器（真实 HTTP + 用户配置的 gh 加速前缀）。
///
/// gh 前缀取自通用 config（`ghProxyPrefix`）；读不到就空串 = 不用镜像（**只回退，不改写原址优先**）。
///
/// `max_bytes` 由调用方按「清单声明的体积」注入并封顶（见
/// [`app_update_size_limit`](super::app_update::app_update_size_limit)）：安装包流式落盘，这道闸
/// 管的是「别把盘写满」。[`crate::runtime::http::MAX_DOWNLOAD_BYTES`]（16 MiB）只是
/// [`CoreDownloader`] 的构造缺省值，容不下安装包。
pub(crate) fn updater_downloader(state: &AppRuntime, max_bytes: usize) -> CoreDownloader {
    let prefix = state
        .config()
        .get_value("ghProxyPrefix")
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    CoreDownloader::new(state.http().clone(), tokio::runtime::Handle::current())
        .with_gh_proxy(prefix)
        .with_max_bytes(max_bytes)
}

/// GitHub releases API 的**状态码 → 成因文案**（纯函数）。`None` = 2xx，调用方继续读 body。
///
/// 抽成自由函数只为**可测**：判定夹在真 HTTP 调用之后，留在 [`fetch_releases_json`] 里就只能起真
/// 网络才断言得了，而本仓禁跑触网测试 —— 那等于这段「哪个状态码是什么成因」一条断言都没有。
pub(crate) fn github_status_error(status: u16, owner: &str, repo: &str) -> Option<String> {
    match status {
        200..=299 => None,
        // 403 限流单独成因（= 上游 `fetchReleases`）：它与「403 无权限」表象相同但处置相反。
        403 => Some("GitHub API 访问频率限制 (403)，请稍后再试或配置 GitHub 加速".to_string()),
        // 404 单独成因。**它不是「该仓库还没有 release」**：本端点是 releases **列表**
        // （`/repos/{owner}/{repo}/releases`，不是 `/releases/latest`），零 release 的**可见**仓库
        // 返回的是 `200 []` —— 由 `check_app_update` 判 `NoUpdate`（见 updater crate 的
        // `check_app_update_empty_releases_is_no_update`）。所以这里的 404 只有一个含义：
        // **更新源仓库对本次（未鉴权）请求不可见** —— 不存在 / 已改名 / 或是私有仓。GitHub 对私有仓
        // 一律以 404 掩盖存在性而**不是** 403，故上面的 403 分支永远兜不到这类。
        //
        // 仍返 `Err` 而不伪装成「已是最新」：把「更新通道整条不通」显示成已最新会让它永久静默，
        // 违反本函数下方 [`fetch_releases_json`] 的反伪造契约。原文案只有裸状态码，分辨不出
        // 「URL 写错」还是「仓库不可见」，带上 owner/repo 才是可行动的诊断（两条检查腿共用本函数，
        // 不带仓名连是 app 腿还是 core 腿都看不出）。
        404 => Some(format!(
            "更新源仓库不可访问 (404)：{owner}/{repo} 不存在、已改名或为私有仓库"
        )),
        s => Some(format!("GitHub API 返回错误: {s}")),
    }
}

/// 验证数组与既有 release schema，并在解析时限制条数，避免先分配一个无界 Vec 再检查。
fn parse_release_list(body: &[u8], max_entries: usize) -> Result<Vec<GithubRelease>, String> {
    struct Releases(usize);
    impl<'de> Visitor<'de> for Releases {
        type Value = Vec<GithubRelease>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a bounded GitHub releases array")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut releases = Vec::with_capacity(self.0.min(GITHUB_RELEASES_PER_PAGE));
            while releases.len() < self.0 {
                match seq.next_element()? {
                    Some(release) => releases.push(release),
                    None => return Ok(releases),
                }
            }
            if seq.next_element::<IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::custom(
                    "GitHub releases entry limit exceeded",
                ));
            }
            Ok(releases)
        }
    }

    let mut deserializer = serde_json::Deserializer::from_slice(body);
    let releases = deserializer
        .deserialize_seq(Releases(max_entries))
        .map_err(|e| format!("解析 GitHub releases 失败: {e}"))?;
    deserializer
        .end()
        .map_err(|e| format!("解析 GitHub releases 失败: {e}"))?;
    Ok(releases)
}

fn check_release_deadline(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err("GitHub releases request timeout: total deadline exceeded".to_string())
    } else {
        Ok(())
    }
}

/// 完整列表的生产分页路径，HTTP/DNS 可注入但所有请求仍经逐跳 SSRF 闸。
///
/// GitHub 每页最多 100 条，仅短页证明结束；不跟随 Link 中的任意地址。总预算覆盖全部页，
/// 上限内无法证明结束时返回 Err，不把部分清单交给版本选择器。raw 与聚合后的编码 JSON 各自
/// 不超过 16 MiB。保留已验证数组元素的原始 JSON，避免重新编码放大或丢失 metadata。
pub(super) async fn fetch_release_pages<H: HttpClient, L: DnsLookup>(
    http: &H,
    lookup: &L,
    owner: &str,
    repo: &str,
    deadline: Instant,
    limits: ReleaseFetchLimits,
) -> Result<String, String> {
    let endpoint = github_releases_api_url(owner, repo);
    let mut combined = String::from("[");
    let mut raw_bytes = 0usize;
    let mut entries = 0usize;
    let mut seen_tags = HashSet::new();
    for page in 1..=limits.max_pages {
        check_release_deadline(deadline)?;
        let remaining_bytes = limits.max_bytes.saturating_sub(raw_bytes);
        if remaining_bytes == 0 {
            return Err("GitHub releases total byte limit exceeded".to_string());
        }
        let url = format!("{endpoint}?per_page={GITHUB_RELEASES_PER_PAGE}&page={page}");
        let response = safe_redirect_fetch_until(
            SafeRedirectFetchOptions {
                fetch_impl: http,
                url: &url,
                user_agent: app_user_agent(),
                headers: Some(vec![(
                    "Accept".to_string(),
                    "application/vnd.github.v3+json".to_string(),
                )]),
                exempt_fake_ip: false,
                max_redirects: Some(5),
                timeout_ms: Some(GITHUB_FETCH_TIMEOUT_MS),
                max_body_bytes: Some(remaining_bytes),
                lookup,
            },
            deadline,
        )
        .await
        .map_err(|e| format!("请求 GitHub 失败 (page {page}): {}", e.message))?;
        if let Some(message) = github_status_error(response.status, owner, repo) {
            return Err(message);
        }
        // Recheck even if an injected/faulty transport ignores its streaming cap.
        if response.body.len() > remaining_bytes {
            return Err("GitHub releases total byte limit exceeded".to_string());
        }
        raw_bytes += response.body.len();
        let releases = parse_release_list(
            &response.body,
            GITHUB_RELEASES_PER_PAGE.min(limits.max_entries.saturating_sub(entries)),
        )?;
        let count = releases.len();
        // Offset pagination can overlap if the listing changes while fetching. Never sign off
        // a repeated page as a complete snapshot or spend the whole budget looping over it.
        if releases
            .iter()
            .any(|release| seen_tags.contains(&release.tag_name))
        {
            return Err(
                "GitHub releases pagination repeated a release; retry the check".to_string(),
            );
        }
        seen_tags.extend(releases.into_iter().map(|release| release.tag_name));
        let body = std::str::from_utf8(&response.body)
            .map_err(|_| "GitHub releases response is not UTF-8".to_string())?
            .trim();
        // parse_release_list already proved the outer array and every element's schema.
        let contents = body[1..body.len() - 1].trim();
        let separator = usize::from(entries > 0 && count > 0);
        if combined.len() + separator + contents.len() + 1 > limits.max_bytes {
            return Err("GitHub releases encoded byte limit exceeded".to_string());
        }
        if count > 0 {
            if separator != 0 {
                combined.push(',');
            }
            combined.push_str(contents);
        }
        entries += count;
        check_release_deadline(deadline)?;
        if count < GITHUB_RELEASES_PER_PAGE {
            combined.push(']');
            return Ok(combined);
        }
    }
    Err("GitHub releases page limit exceeded before the complete list was fetched".to_string())
}

/// 拉完整、有界的 App releases JSON；当前唯一业务调用点是 `update_check`。
///
/// 失败语义（反伪造）：网络/SSRF/超时/非 2xx 一律 `Err(消息)`，**绝不**把失败伪装成「已是最新」。
pub(crate) async fn fetch_releases_json(
    state: &AppRuntime,
    owner: &str,
    repo: &str,
) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_millis(GITHUB_FETCH_TIMEOUT_MS);
    let fetch = async {
        #[cfg(debug_assertions)]
        if (owner, repo) == polaris_updater::github::APP_UPDATE_REPO {
            if let Some(result) =
                crate::runtime::http::local_acceptance::releases(MAX_GITHUB_JSON_BYTES).await
            {
                // The debug fixture is one complete list, not a GitHub API page. Keep its
                // existing endpoint/auth contract, but enforce the same total-list limits.
                let body = result?;
                if body.len() > RELEASE_FETCH_LIMITS.max_bytes {
                    return Err("GitHub releases total byte limit exceeded".to_string());
                }
                parse_release_list(body.as_bytes(), RELEASE_FETCH_LIMITS.max_entries)?;
                check_release_deadline(deadline)?;
                return Ok(body);
            }
        }
        let http = state.http().clone();
        fetch_release_pages(
            http.as_ref(),
            &SystemDnsLookup,
            owner,
            repo,
            deadline,
            RELEASE_FETCH_LIMITS,
        )
        .await
    };
    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), fetch)
        .await
        .map_err(|_| "GitHub releases request timeout: total deadline exceeded".to_string())?
}
