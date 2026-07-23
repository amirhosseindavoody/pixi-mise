//! Resolve `ToolRequest` → `ToolVersion` via GitHub + AssetPicker.

use pixi_mise_assets::{AssetCandidate, HostPlatform, PickOptions, pick_asset};
use pixi_mise_github::{GithubClient, select_release};

use crate::{CoreError, ResolvedAsset, ToolRequest, ToolVersion, VersionSpec};

/// Resolve a tool request to a concrete release asset for `host`.
pub fn resolve_tool(
    client: &GithubClient,
    request: &ToolRequest,
    host: &HostPlatform,
) -> Result<ToolVersion, CoreError> {
    if request.options.asset_pattern.is_some() {
        return Err(CoreError::NotImplemented(
            "`asset_pattern` lands in Phase 2; use AssetPicker autodetection / `matching` for now",
        ));
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

    let picked = pick_asset(
        &candidates,
        host,
        &PickOptions {
            matching: request.options.matching.clone(),
            matching_regex: request.options.matching_regex.clone(),
            preferred_name: Some(repo.clone()),
        },
    )
    .map_err(|e| {
        if matches!(e, pixi_mise_assets::AssetError::NoMatch) {
            let names: Vec<_> = candidates.iter().map(|c| c.name.as_str()).collect();
            tracing::error!(
                host_os = %host.os,
                host_arch = %host.arch,
                available = ?names,
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

    let version = display_version(&release.tag_name, request.options.version_prefix.as_deref());

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
