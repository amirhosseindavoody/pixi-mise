//! Core types, config discovery, resolve, and install orchestration.
//!
//! Phase 0: type definitions and stub APIs. Parsing / resolve / install arrive in Phase 1.

#![deny(missing_docs)]

use std::path::PathBuf;

use thiserror::Error;

pub use pixi_mise_assets as assets;
pub use pixi_mise_github as github;
pub use pixi_mise_pixi as pixi;

/// Errors from core orchestration.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Feature not implemented yet.
    #[error("{0} is not implemented yet (Phase 0 skeleton)")]
    NotImplemented(&'static str),
    /// Tool spec could not be parsed.
    #[error("invalid tool spec `{spec}`: {reason}")]
    InvalidToolSpec {
        /// Original user input.
        spec: String,
        /// Why parsing failed.
        reason: &'static str,
    },
}

/// Backend kind for a tool request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// GitHub Releases backend (`github:owner/repo`).
    Github,
}

/// Tool identity (`owner/repo` for GitHub).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolId {
    /// Owner (user or org).
    pub owner: String,
    /// Repository name.
    pub repo: String,
}

impl ToolId {
    /// Format as `owner/repo`.
    pub fn as_str(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// Version request from config / CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpec {
    /// GitHub “latest” non-prerelease (unless options say otherwise).
    Latest,
    /// Exact tag match (optional `v` normalization later).
    Exact(String),
    /// Highest tag matching this prefix.
    Prefix(String),
}

/// Optional install / resolve overrides (mise-compatible subset).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolOptions {
    /// Substring filter on asset names.
    pub matching: Option<String>,
    /// Regex filter on asset names.
    pub matching_regex: Option<String>,
    /// Explicit asset pattern (skips autodetection when set).
    pub asset_pattern: Option<String>,
    /// Preferred binary name inside the archive.
    pub bin: Option<String>,
    /// Rename installed executable.
    pub rename_exe: Option<String>,
    /// Strip leading path components when extracting.
    pub strip_components: Option<u32>,
    /// Path inside the archive that contains binaries.
    pub bin_path: Option<String>,
    /// Optional checksum string (`sha256:…`).
    pub checksum: Option<String>,
    /// Strip this prefix from tags before version matching.
    pub version_prefix: Option<String>,
    /// Allow prerelease tags.
    pub prerelease: bool,
    /// Global expose name override.
    pub expose_as: Option<String>,
}

/// Where a tool request was declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSource {
    /// Path to the config file.
    pub path: PathBuf,
    /// Human-readable table / key (e.g. `tool.pixi-mise.tools`).
    pub table: String,
}

/// User-facing tool request before resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRequest {
    /// Backend (GitHub in v1).
    pub backend: BackendKind,
    /// Tool id.
    pub id: ToolId,
    /// Requested version.
    pub version: VersionSpec,
    /// Overrides.
    pub options: ToolOptions,
    /// Config provenance.
    pub source: ConfigSource,
}

/// Fully resolved tool version + chosen asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolVersion {
    /// Original request.
    pub request: ToolRequest,
    /// Concrete version string (display).
    pub version: String,
    /// GitHub tag name.
    pub tag: String,
    /// Chosen release asset.
    pub asset: ResolvedAsset,
}

/// Resolved downloadable asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAsset {
    /// Asset filename.
    pub name: String,
    /// Download URL.
    pub download_url: String,
    /// Size in bytes, when known.
    pub size: Option<u64>,
    /// Digest, when known (`sha256:…`).
    pub digest: Option<String>,
}

/// Parse a `github:owner/repo[@version]` tool string.
pub fn parse_tool_spec(spec: &str) -> Result<(ToolId, VersionSpec), CoreError> {
    let rest = spec
        .strip_prefix("github:")
        .ok_or_else(|| CoreError::InvalidToolSpec {
            spec: spec.to_string(),
            reason: "only `github:owner/repo` is supported in v1",
        })?;

    let (id_part, version) = match rest.rsplit_once('@') {
        Some((id, ver)) => (id, parse_version_spec(ver)),
        None => (rest, VersionSpec::Latest),
    };

    let (owner, repo) = id_part
        .split_once('/')
        .ok_or_else(|| CoreError::InvalidToolSpec {
            spec: spec.to_string(),
            reason: "expected `github:owner/repo`",
        })?;

    if owner.is_empty() || repo.is_empty() {
        return Err(CoreError::InvalidToolSpec {
            spec: spec.to_string(),
            reason: "owner and repo must be non-empty",
        });
    }

    Ok((
        ToolId {
            owner: owner.to_string(),
            repo: repo.to_string(),
        },
        version,
    ))
}

fn parse_version_spec(raw: &str) -> VersionSpec {
    if raw.is_empty() || raw.eq_ignore_ascii_case("latest") {
        VersionSpec::Latest
    } else if raw.contains('.') || raw.chars().all(|c| c.is_ascii_digit() || c == 'v') {
        // Heuristic: full versions / tags → Exact; short numeric prefixes → Prefix.
        // Refined in Phase 1 against real tags.
        let stripped = raw.trim_start_matches('v');
        if stripped.chars().all(|c| c.is_ascii_digit()) && !stripped.is_empty() {
            VersionSpec::Prefix(stripped.to_string())
        } else {
            VersionSpec::Exact(raw.to_string())
        }
    } else {
        VersionSpec::Exact(raw.to_string())
    }
}

/// Crate version from Cargo.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_latest() {
        let (id, ver) = parse_tool_spec("github:cli/cli").unwrap();
        assert_eq!(id.owner, "cli");
        assert_eq!(id.repo, "cli");
        assert_eq!(ver, VersionSpec::Latest);
    }

    #[test]
    fn parse_prefix_and_exact() {
        let (_, ver) = parse_tool_spec("github:BurntSushi/ripgrep@14").unwrap();
        assert_eq!(ver, VersionSpec::Prefix("14".into()));

        let (_, ver) = parse_tool_spec("github:BurntSushi/ripgrep@14.1.1").unwrap();
        assert_eq!(ver, VersionSpec::Exact("14.1.1".into()));
    }

    #[test]
    fn reject_non_github() {
        assert!(matches!(
            parse_tool_spec("aqua:cli/cli"),
            Err(CoreError::InvalidToolSpec { .. })
        ));
    }
}
