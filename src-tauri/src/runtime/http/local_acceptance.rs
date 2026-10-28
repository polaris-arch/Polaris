//! ACC01 Q3 transport for explicit debug-only local update windows.
//! Production release selection, size/hash checks, staging and installation remain shared.

use std::sync::Arc;
use std::time::Duration;

use crate::runtime::http::{DownloadProgressFn, DownloadSinkFactory, StreamedDownload};
use polaris_updater::traits::DownloadError;

#[derive(Clone)]
struct Source {
    origin: reqwest::Url,
    token: String,
}

fn parse_source(origin: &str, token: &str) -> Result<Source, String> {
    let url = reqwest::Url::parse(origin).map_err(|_| "Invalid ACC01 update origin")?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_none_or(|port| port < 1024)
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || token.len() != 64
        || !token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("ACC01 requires an exact loopback origin and window token".into());
    }
    Ok(Source {
        origin: url,
        token: token.to_string(),
    })
}

fn configured() -> Result<Option<Source>, String> {
    let origin = std::env::var("POLARIS_ACC01_UPDATE_ORIGIN");
    let token = std::env::var("POLARIS_ACC01_UPDATE_TOKEN");
    match (origin, token) {
        (Err(std::env::VarError::NotPresent), Err(std::env::VarError::NotPresent)) => Ok(None),
        (Ok(origin), Ok(token)) => parse_source(&origin, &token).map(Some),
        _ => Err("Incomplete ACC01 local update configuration".into()),
    }
}

fn client(timeout: Duration) -> Result<reqwest::Client, String> {
    super::install_ring_provider();
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .map_err(|_| "ACC01 local client unavailable".into())
}

fn asset_path(url: &str) -> Result<&str, String> {
    let path = url
        .strip_prefix("https://github.com/polaris-arch/Polaris/releases/download/")
        .ok_or("ACC01 only accepts App release asset URLs")?;
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
    {
        return Err("Invalid ACC01 release asset path".into());
    }
    Ok(path)
}

pub(crate) async fn releases(max_bytes: usize) -> Option<Result<String, String>> {
    let source = match configured() {
        Ok(None) => return None,
        Ok(Some(source)) => source,
        Err(error) => return Some(Err(error)),
    };
    Some(
        async {
            let client = client(Duration::from_secs(30))?;
            let endpoint = source
                .origin
                .join("releases.json")
                .map_err(|_| "Invalid local endpoint")?;
            let mut response = client
                .get(endpoint)
                .bearer_auth(source.token)
                .send()
                .await
                .map_err(|_| "ACC01 local metadata request failed")?;
            if response.status() != reqwest::StatusCode::OK {
                return Err("ACC01 local metadata was refused".into());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| "ACC01 metadata truncated")?
            {
                if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
                    return Err("ACC01 metadata exceeds size bound".into());
                }
                bytes.extend_from_slice(&chunk);
            }
            String::from_utf8(bytes).map_err(|_| "ACC01 metadata is not UTF-8".into())
        }
        .await,
    )
}

pub(crate) fn download(
    url: &str,
    max_bytes: usize,
    sink: Arc<DownloadSinkFactory>,
    progress: Arc<DownloadProgressFn>,
    handle: &tokio::runtime::Handle,
) -> Option<Result<StreamedDownload, DownloadError>> {
    let source = match configured() {
        Ok(None) => return None,
        Ok(Some(source)) => source,
        Err(error) => return Some(Err(DownloadError::Other(error))),
    };
    let result = handle.block_on(async {
        let path = asset_path(url)?;
        let endpoint = source
            .origin
            .join(&format!("assets/{path}"))
            .map_err(|_| "Invalid local asset endpoint")?;
        let mut response = client(Duration::from_secs(600))?
            .get(endpoint)
            .bearer_auth(source.token)
            .send()
            .await
            .map_err(|_| "ACC01 local download failed")?;
        if response.status() != reqwest::StatusCode::OK {
            return Err("ACC01 local asset was refused".into());
        }
        let expected = response
            .content_length()
            .ok_or("ACC01 asset size unavailable")?;
        if expected == 0 || expected > max_bytes as u64 {
            return Err("ACC01 asset exceeds size bound".into());
        }
        let mut target = sink().map_err(|_| "ACC01 staging file unavailable")?;
        let mut hash = polaris_updater::verify::Sha256Stream::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "ACC01 asset truncated")?
        {
            if chunk.len() as u64 > expected.saturating_sub(hash.len()) {
                return Err("ACC01 asset exceeds declared size".into());
            }
            target
                .write_all(&chunk)
                .map_err(|_| "ACC01 staging write failed")?;
            hash.update(&chunk);
            progress(hash.len(), Some(expected));
        }
        target.flush().map_err(|_| "ACC01 staging flush failed")?;
        if hash.len() != expected {
            return Err("ACC01 asset size mismatch".into());
        }
        Ok(StreamedDownload {
            bytes: hash.len(),
            sha256_hex: hash.finish(),
        })
    });
    Some(result.map_err(DownloadError::Other))
}

#[cfg(test)]
mod tests;
