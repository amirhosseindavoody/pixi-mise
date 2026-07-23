//! GitHub API client and release listing for pixi-mise.

#![deny(missing_docs)]

use std::io::Read;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

/// Errors from the GitHub client.
#[derive(Debug, Error)]
pub enum GithubError {
    /// HTTP / network failure.
    #[error("GitHub HTTP error: {0}")]
    Http(String),
    /// API returned a non-success status.
    #[error("GitHub API {status}: {message}")]
    Api {
        /// HTTP status code.
        status: u16,
        /// Response body / message.
        message: String,
    },
    /// Rate limited; suggest setting a token.
    #[error("GitHub API rate limited. Set GITHUB_TOKEN or GH_TOKEN for a higher limit.\n{message}")]
    RateLimited {
        /// Response body / message.
        message: String,
    },
    /// Requested release / asset was not found.
    #[error("{0}")]
    NotFound(String),
    /// JSON decode failure.
    #[error("failed to decode GitHub response: {0}")]
    Decode(String),
    /// I/O while downloading.
    #[error("download I/O error: {0}")]
    Io(String),
}

/// A GitHub release summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Tag name (`v1.2.3`, `14.1.1`, …).
    pub tag_name: String,
    /// Whether GitHub marked the release as a prerelease.
    pub prerelease: bool,
    /// Whether this is a draft release.
    pub draft: bool,
    /// Assets attached to the release.
    pub assets: Vec<ReleaseAsset>,
}

/// A single release asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    /// Asset filename.
    pub name: String,
    /// Browser download URL.
    pub download_url: String,
    /// Size in bytes, when known.
    pub size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    message: Option<String>,
}

/// Client for listing GitHub releases and downloading assets.
#[derive(Debug, Clone)]
pub struct GithubClient {
    http: reqwest::blocking::Client,
    token: Option<String>,
}

impl Default for GithubClient {
    fn default() -> Self {
        Self::new()
    }
}

impl GithubClient {
    /// Create a new client, picking up `GITHUB_TOKEN` / `GH_TOKEN` when set.
    pub fn new() -> Self {
        let token = std::env::var("GITHUB_TOKEN")
            .or_else(|_| std::env::var("GH_TOKEN"))
            .ok()
            .filter(|t| !t.is_empty());
        Self::with_token(token)
    }

    /// Create a client with an explicit token (or none).
    pub fn with_token(token: Option<String>) -> Self {
        let http = reqwest::blocking::Client::builder()
            .user_agent(format!("pixi-mise/{}", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(120))
            .build()
            .expect("failed to build HTTP client");
        Self { http, token }
    }

    /// List releases for `owner/repo` (paginated, up to a reasonable limit).
    pub fn list_releases(&self, owner: &str, repo: &str) -> Result<Vec<Release>, GithubError> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let url = format!(
                "https://api.github.com/repos/{owner}/{repo}/releases?per_page=100&page={page}"
            );
            let body = self.get_json(&url)?;
            let batch: Vec<ApiRelease> =
                serde_json::from_str(&body).map_err(|e| GithubError::Decode(e.to_string()))?;
            if batch.is_empty() {
                break;
            }
            let batch_len = batch.len();
            for r in batch {
                if r.draft {
                    continue;
                }
                all.push(Release {
                    tag_name: r.tag_name,
                    prerelease: r.prerelease,
                    draft: r.draft,
                    assets: r
                        .assets
                        .into_iter()
                        .map(|a| ReleaseAsset {
                            name: a.name,
                            download_url: a.browser_download_url,
                            size: a.size,
                        })
                        .collect(),
                });
            }
            if batch_len < 100 || page >= 10 {
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    /// Fetch GitHub's "latest" release (non-prerelease).
    pub fn latest_release(&self, owner: &str, repo: &str) -> Result<Release, GithubError> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/releases/latest");
        let body = self.get_json(&url)?;
        let r: ApiRelease =
            serde_json::from_str(&body).map_err(|e| GithubError::Decode(e.to_string()))?;
        Ok(Release {
            tag_name: r.tag_name,
            prerelease: r.prerelease,
            draft: r.draft,
            assets: r
                .assets
                .into_iter()
                .map(|a| ReleaseAsset {
                    name: a.name,
                    download_url: a.browser_download_url,
                    size: a.size,
                })
                .collect(),
        })
    }

    /// Download `url` into `dest`, returning bytes written.
    pub fn download(&self, url: &str, dest: &mut dyn std::io::Write) -> Result<u64, GithubError> {
        let mut request = self.http.get(url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let mut response = request
            .send()
            .map_err(|e| GithubError::Http(e.to_string()))?;
        let status = response.status();
        if status.as_u16() == 403 || status.as_u16() == 429 {
            let message = response.text().unwrap_or_default();
            return Err(GithubError::RateLimited { message });
        }
        if !status.is_success() {
            let message = response.text().unwrap_or_default();
            return Err(GithubError::Api {
                status: status.as_u16(),
                message,
            });
        }
        let mut buf = [0u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            let n = response
                .read(&mut buf)
                .map_err(|e| GithubError::Io(e.to_string()))?;
            if n == 0 {
                break;
            }
            dest.write_all(&buf[..n])
                .map_err(|e| GithubError::Io(e.to_string()))?;
            written += n as u64;
        }
        Ok(written)
    }

    fn get_json(&self, url: &str) -> Result<String, GithubError> {
        let mut request = self
            .http
            .get(url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .map_err(|e| GithubError::Http(e.to_string()))?;
        let status = response.status();
        let body = response
            .text()
            .map_err(|e| GithubError::Http(e.to_string()))?;
        if status.as_u16() == 403 || status.as_u16() == 429 {
            return Err(GithubError::RateLimited { message: body });
        }
        if status.as_u16() == 404 {
            let msg = serde_json::from_str::<ApiErrorBody>(&body)
                .ok()
                .and_then(|b| b.message)
                .unwrap_or(body);
            return Err(GithubError::NotFound(msg));
        }
        if !status.is_success() {
            return Err(GithubError::Api {
                status: status.as_u16(),
                message: body,
            });
        }
        Ok(body)
    }
}

/// Select a release matching `latest` / exact tag / prefix.
pub fn select_release<'a>(
    releases: &'a [Release],
    latest: Option<&'a Release>,
    want_latest: bool,
    exact: Option<&str>,
    prefix: Option<&str>,
    allow_prerelease: bool,
) -> Result<&'a Release, GithubError> {
    let usable = |r: &&Release| allow_prerelease || !r.prerelease;

    if want_latest {
        if let Some(r) = latest.filter(usable) {
            return Ok(r);
        }
        return releases
            .iter()
            .find(usable)
            .ok_or_else(|| GithubError::NotFound("no suitable latest release".into()));
    }

    if let Some(exact) = exact {
        let normalized = exact.trim_start_matches('v');
        return releases
            .iter()
            .find(|r| {
                usable(r)
                    && (r.tag_name == exact
                        || r.tag_name.trim_start_matches('v') == normalized
                        || format!("v{normalized}") == r.tag_name)
            })
            .ok_or_else(|| GithubError::NotFound(format!("no release matching tag `{exact}`")));
    }

    if let Some(prefix) = prefix {
        let normalized = prefix.trim_start_matches('v');
        // Prefer the first release whose tag (sans optional v) starts with prefix as a version
        // boundary: `14` matches `14.1.1` / `v14.0.0` but not `140.0.0`.
        let matched = releases.iter().find(|r| {
            if !usable(r) {
                return false;
            }
            let tag = r.tag_name.trim_start_matches('v');
            tag == normalized
                || tag.starts_with(&format!("{normalized}."))
                || tag.starts_with(&format!("{normalized}-"))
                || tag.starts_with(&format!("{normalized}_"))
        });
        return matched.ok_or_else(|| {
            GithubError::NotFound(format!("no release matching prefix `{prefix}`"))
        });
    }

    Err(GithubError::NotFound("empty version selector".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rel(tag: &str, pre: bool) -> Release {
        Release {
            tag_name: tag.into(),
            prerelease: pre,
            draft: false,
            assets: vec![],
        }
    }

    #[test]
    fn select_exact_with_v_normalization() {
        let releases = vec![rel("v2.67.0", false), rel("v2.66.0", false)];
        let got = select_release(&releases, None, false, Some("2.67.0"), None, false).unwrap();
        assert_eq!(got.tag_name, "v2.67.0");
    }

    #[test]
    fn select_prefix() {
        let releases = vec![
            rel("15.0.0", false),
            rel("14.1.1", false),
            rel("14.0.0", false),
            rel("13.0.0", false),
        ];
        let got = select_release(&releases, None, false, None, Some("14"), false).unwrap();
        assert_eq!(got.tag_name, "14.1.1");
    }

    #[test]
    fn prefix_does_not_match_longer_major() {
        let releases = vec![rel("140.0.0", false), rel("14.1.0", false)];
        let got = select_release(&releases, None, false, None, Some("14"), false).unwrap();
        assert_eq!(got.tag_name, "14.1.0");
    }
}
