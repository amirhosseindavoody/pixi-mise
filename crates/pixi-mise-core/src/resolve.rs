//! Resolve `ToolRequest` → `ToolVersion` via GitHub + AssetPicker.

use pixi_mise_assets::{AssetCandidate, HostPlatform, PickOptions, pick_asset};
use pixi_mise_github::{GithubClient, select_release};

use crate::lockfile::LockEntry;
use crate::{CoreError, ResolvedAsset, ToolRequest, ToolVersion, VersionSpec};

/// Resolve a tool request to a concrete release asset for `host`.
///
/// When `locked` is provided for this tool+platform, that asset is used as-is
/// (URL / name / checksum) without re-querying GitHub asset selection.
pub fn resolve_tool(
    client: &GithubClient,
    request: &ToolRequest,
    host: &HostPlatform,
) -> Result<ToolVersion, CoreError> {
    resolve_tool_with_lock(client, request, host, None)
}

/// Resolve using an optional lock entry for the current platform.
pub fn resolve_tool_with_lock(
    client: &GithubClient,
    request: &ToolRequest,
    host: &HostPlatform,
    locked: Option<&LockEntry>,
) -> Result<ToolVersion, CoreError> {
    if let Some(entry) = locked
        && entry.id == request.id.github_spec()
        && entry.platform == host.pixi_platform()
    {
        return Ok(ToolVersion {
            request: request.clone(),
            version: entry.version.clone(),
            tag: entry.tag.clone(),
            asset: ResolvedAsset {
                name: entry.asset.clone(),
                download_url: entry.url.clone(),
                size: None,
                digest: entry.checksum.clone(),
            },
        });
    }

    let owner = &request.id.owner;
    let repo = &request.id.repo;

    let (want_latest, exact, prefix) = match &request.version {
        VersionSpec::Latest => (true, None, None),
        VersionSpec::Exact(v) => (false, Some(v.as_str()), None),
        VersionSpec::Prefix(v) => (false, None, Some(v.as_str())),
    };

    let latest = if want_latest {
        match client.latest_release(owner, repo) {
            Ok(r) => Some(r),
            Err(pixi_mise_github::GithubError::NotFound(_)) => None,
            Err(e) => return Err(e.into()),
        }
    } else {
        None
    };

    let releases = if want_latest && latest.is_some() {
        Vec::new()
    } else {
        client.list_releases(owner, repo)?
    };

    let release = select_release(
        &releases,
        latest.as_ref(),
        want_latest,
        exact,
        prefix,
        request.options.prerelease,
    )?;

    let candidates: Vec<AssetCandidate> = release
        .assets
        .iter()
        .map(|a| AssetCandidate {
            name: a.name.clone(),
            size: a.size,
            download_url: Some(a.download_url.clone()),
        })
        .collect();

    let version = display_version(&release.tag_name, request.options.version_prefix.as_deref());

    let picked = pick_asset(
        &candidates,
        host,
        &PickOptions {
            matching: request.options.matching.clone(),
            matching_regex: request.options.matching_regex.clone(),
            asset_pattern: request.options.asset_pattern.clone(),
            preferred_name: Some(repo.clone()),
            version: Some(version.clone()),
        },
    )
    .map_err(|e| {
        if matches!(e, pixi_mise_assets::AssetError::NoMatch) {
            let names: Vec<_> = candidates.iter().map(|c| c.name.as_str()).collect();
            tracing::error!(
                host_os = %host.os,
                host_arch = %host.arch,
                available = ?names,
                pattern = ?request.options.asset_pattern,
                "no matching asset"
            );
        }
        e
    })?;

    let download_url = picked.download_url.ok_or_else(|| {
        CoreError::Install(format!(
            "selected asset `{}` has no download URL",
            picked.name
        ))
    })?;

    Ok(ToolVersion {
        request: request.clone(),
        version,
        tag: release.tag_name.clone(),
        asset: ResolvedAsset {
            name: picked.name,
            download_url,
            size: picked.size,
            digest: request.options.checksum.clone(),
        },
    })
}

fn display_version(tag: &str, version_prefix: Option<&str>) -> String {
    let mut v = tag.to_string();
    if let Some(prefix) = version_prefix
        && let Some(stripped) = v.strip_prefix(prefix)
    {
        v = stripped.to_string();
    }
    v.trim_start_matches('v').to_string()
}
