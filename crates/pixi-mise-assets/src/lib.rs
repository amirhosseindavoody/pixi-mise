//! Mise-inspired asset scoring for GitHub release binaries.
//!
//! Phase 0 placeholder — AssetPicker scoring lands in Phase 1.

#![deny(missing_docs)]

use thiserror::Error;

/// Errors from asset matching.
#[derive(Debug, Error)]
pub enum AssetError {
    /// No asset scored high enough for the host platform.
    #[error("no matching asset for host platform")]
    NoMatch,
}

/// A release asset candidate (name + optional size).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetCandidate {
    /// Filename as published on the release.
    pub name: String,
    /// Size in bytes, when known.
    pub size: Option<u64>,
}

/// Host platform used for scoring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPlatform {
    /// Normalized OS (`linux`, `macos`, `windows`).
    pub os: String,
    /// Normalized arch (`x64`, `arm64`, …).
    pub arch: String,
    /// Optional libc (`gnu`, `musl`, `msvc`).
    pub libc: Option<String>,
}

/// Result of picking the best asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedAsset {
    /// Chosen asset name.
    pub name: String,
    /// Score assigned by the matcher.
    pub score: i32,
}

/// Filter and score release assets for the host platform.
///
/// Not implemented in Phase 0.
pub fn pick_asset(
    _candidates: &[AssetCandidate],
    _host: &HostPlatform,
    _matching: Option<&str>,
) -> Result<PickedAsset, AssetError> {
    Err(AssetError::NoMatch)
}
