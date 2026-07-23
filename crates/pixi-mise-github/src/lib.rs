//! GitHub API client and release listing for pixi-mise.
//!
//! Phase 0 placeholder — HTTP client and pagination land in Phase 1.

#![deny(missing_docs)]

use thiserror::Error;

/// Errors from the GitHub client.
#[derive(Debug, Error)]
pub enum GithubError {
    /// Feature not implemented yet.
    #[error("GitHub client not implemented yet")]
    NotImplemented,
}

/// A GitHub release summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Tag name (`v1.2.3`, `14.1.1`, …).
    pub tag_name: String,
    /// Whether GitHub marked the release as a prerelease.
    pub prerelease: bool,
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

/// Client for listing GitHub releases.
///
/// Not implemented in Phase 0.
#[derive(Debug, Default, Clone)]
pub struct GithubClient;

impl GithubClient {
    /// Create a new client (auth wiring comes in Phase 1).
    pub fn new() -> Self {
        Self
    }

    /// List releases for `owner/repo`.
    pub fn list_releases(&self, _owner: &str, _repo: &str) -> Result<Vec<Release>, GithubError> {
        Err(GithubError::NotImplemented)
    }
}
