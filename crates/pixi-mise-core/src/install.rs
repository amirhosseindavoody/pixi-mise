//! Download, extract, and install resolved tools into a Pixi prefix.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use pixi_mise_assets::HostPlatform;
use pixi_mise_github::GithubClient;
use pixi_mise_pixi::{
    InstallTarget, InstalledToolMeta, install_binary, remove_binaries, resolve_prefix,
    write_tool_meta,
};

use crate::extract::{extract_asset, find_binaries};
use crate::{CoreError, ToolVersion};

/// Result of installing a tool.
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    /// Resolved tool version.
    pub tool: ToolVersion,
    /// Installed binary names under `$PREFIX/bin`.
    pub installed_bins: Vec<String>,
    /// Prefix used for the install.
    pub prefix: PathBuf,
}

/// Install a resolved tool into a local Pixi environment.
pub fn install_tool(
    client: &GithubClient,
    tool: &ToolVersion,
    workspace_root: &Path,
    env: &str,
    host: &HostPlatform,
    dry_run: bool,
) -> Result<InstallOutcome, CoreError> {
    let target = InstallTarget::Local {
        workspace_root: workspace_root.to_path_buf(),
        env: env.to_string(),
    };
    let prefix = resolve_prefix(&target)?;

    if dry_run {
        return Ok(InstallOutcome {
            tool: tool.clone(),
            installed_bins: Vec::new(),
            prefix,
        });
    }

    let cache_dir = cache_dir_for(&tool.request.id.owner, &tool.request.id.repo, &tool.tag);
    fs::create_dir_all(&cache_dir).map_err(|e| CoreError::Install(e.to_string()))?;
    let archive_path = cache_dir.join(&tool.asset.name);

    if !archive_path.is_file() {
        tracing::info!(
            url = %tool.asset.download_url,
            dest = %archive_path.display(),
            "downloading asset"
        );
        let mut file =
            File::create(&archive_path).map_err(|e| CoreError::Install(e.to_string()))?;
        client.download(&tool.asset.download_url, &mut file)?;
    } else {
        tracing::debug!(path = %archive_path.display(), "using cached asset");
    }

    let staging = cache_dir.join("staging");
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|e| CoreError::Install(e.to_string()))?;
    }
    fs::create_dir_all(&staging).map_err(|e| CoreError::Install(e.to_string()))?;
    extract_asset(&archive_path, &staging)?;

    let preferred = tool.request.options.bin.as_deref().or(tool
        .request
        .options
        .rename_exe
        .as_deref());
    let binaries = find_binaries(
        &staging,
        preferred,
        tool.request.options.bin_path.as_deref(),
        Some(tool.request.id.repo.as_str()),
    )?;

    let mut installed_bins = Vec::new();
    for bin_src in &binaries {
        let default_name = bin_src
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| CoreError::Install("binary has no filename".into()))?;
        let install_name = tool
            .request
            .options
            .rename_exe
            .as_deref()
            .unwrap_or(default_name);
        let dest = install_binary(&prefix, bin_src, install_name)?;
        tracing::info!(bin = %dest.display(), "installed binary");
        installed_bins.push(install_name.to_string());
    }

    // Remove previously installed bins for this tool that are no longer present.
    if let Ok(Some(old)) =
        pixi_mise_pixi::read_tool_meta(workspace_root, env, &tool.request.id.github_spec())
    {
        let stale: Vec<String> = old
            .installed_bins
            .into_iter()
            .filter(|b| !installed_bins.contains(b))
            .collect();
        if !stale.is_empty() {
            remove_binaries(&prefix, &stale)?;
        }
    }

    let meta = InstalledToolMeta {
        id: tool.request.id.github_spec(),
        version: tool.version.clone(),
        tag: tool.tag.clone(),
        asset: tool.asset.name.clone(),
        url: tool.asset.download_url.clone(),
        platform: host.pixi_platform(),
        installed_bins: installed_bins.clone(),
    };
    write_tool_meta(workspace_root, env, &meta)?;

    // Best-effort cleanup of staging (keep downloaded archive in cache).
    let _ = fs::remove_dir_all(&staging);

    Ok(InstallOutcome {
        tool: tool.clone(),
        installed_bins,
        prefix,
    })
}

fn cache_dir_for(owner: &str, repo: &str, tag: &str) -> PathBuf {
    let base = if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        PathBuf::from(xdg)
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".cache")
    } else {
        PathBuf::from(".cache")
    };
    let safe_tag = tag.replace('/', "_");
    base.join("pixi-mise").join(owner).join(repo).join(safe_tag)
}
