//! Download, extract, and install resolved tools into a Pixi prefix.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use pixi_mise_assets::HostPlatform;
use pixi_mise_github::GithubClient;
use pixi_mise_pixi::{
    InstallTarget, InstalledToolMeta, expose_binary, install_binary, remove_binaries,
    resolve_prefix, unexpose_binaries, write_tool_meta,
};

use crate::extract::{extract_asset, find_binaries};
use crate::lockfile::{LockEntry, Lockfile, sha256_file, verify_sha256};
use crate::{CoreError, ToolVersion};

/// Result of installing a tool.
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    /// Resolved tool version.
    pub tool: ToolVersion,
    /// Installed binary names under `$PREFIX/bin`.
    pub installed_bins: Vec<String>,
    /// Names exposed on `$PIXI_HOME/bin` (global installs).
    pub exposed_bins: Vec<String>,
    /// Prefix used for the install.
    pub prefix: PathBuf,
    /// Recorded sha256 digest (`sha256:…`).
    pub checksum: Option<String>,
}

/// Install a resolved tool into a local or global Pixi environment.
pub fn install_tool(
    client: &GithubClient,
    tool: &ToolVersion,
    target: &InstallTarget,
    host: &HostPlatform,
    dry_run: bool,
) -> Result<InstallOutcome, CoreError> {
    let prefix = resolve_prefix(target)?;

    if dry_run {
        return Ok(InstallOutcome {
            tool: tool.clone(),
            installed_bins: Vec::new(),
            exposed_bins: Vec::new(),
            prefix,
            checksum: tool.asset.digest.clone(),
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

    if let Some(expected) = tool.asset.digest.as_deref() {
        verify_sha256(&archive_path, expected)?;
    }

    let checksum = match &tool.asset.digest {
        Some(d) => Some(d.clone()),
        None => Some(sha256_file(&archive_path)?),
    };

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

    // When `bin` is set, only install that binary (find_binaries already filtered).
    // When `rename_exe` is set with a single binary, rename it.
    let mut installed_bins = Vec::new();
    for bin_src in &binaries {
        let default_name = bin_src
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| CoreError::Install("binary has no filename".into()))?;
        let install_name = if binaries.len() == 1 {
            tool.request
                .options
                .rename_exe
                .as_deref()
                .unwrap_or(default_name)
        } else {
            default_name
        };
        let dest = install_binary(&prefix, bin_src, install_name)?;
        tracing::info!(bin = %dest.display(), "installed binary");
        installed_bins.push(install_name.to_string());
    }

    let meta_root = target.meta_root();
    let env = target.env_name();
    let tool_id = tool.request.id.github_spec();

    if let Ok(Some(old)) = pixi_mise_pixi::read_tool_meta(&meta_root, env, &tool_id) {
        let stale: Vec<String> = old
            .installed_bins
            .into_iter()
            .filter(|b| !installed_bins.contains(b))
            .collect();
        if !stale.is_empty() {
            remove_binaries(&prefix, &stale)?;
        }
        if matches!(target, InstallTarget::Global { .. }) && !old.exposed_bins.is_empty() {
            let stale_expose: Vec<String> = old
                .exposed_bins
                .into_iter()
                .filter(|b| {
                    let still = installed_bins.contains(b)
                        || tool
                            .request
                            .options
                            .expose_as
                            .as_deref()
                            .is_some_and(|e| e == b);
                    !still
                })
                .collect();
            if !stale_expose.is_empty() {
                unexpose_binaries(&stale_expose)?;
            }
        }
    }

    let mut exposed_bins = Vec::new();
    if matches!(target, InstallTarget::Global { .. }) {
        for bin in &installed_bins {
            let expose_name = tool
                .request
                .options
                .expose_as
                .as_deref()
                .filter(|_| installed_bins.len() == 1)
                .unwrap_or(bin);
            let src = prefix.join("bin").join(bin);
            let link = expose_binary(&src, expose_name)?;
            tracing::info!(expose = %link.display(), "exposed binary");
            exposed_bins.push(expose_name.to_string());
        }
    }

    let meta = InstalledToolMeta {
        id: tool_id.clone(),
        version: tool.version.clone(),
        tag: tool.tag.clone(),
        asset: tool.asset.name.clone(),
        url: tool.asset.download_url.clone(),
        platform: host.pixi_platform(),
        installed_bins: installed_bins.clone(),
        exposed_bins: exposed_bins.clone(),
    };
    write_tool_meta(&meta_root, env, &meta)?;

    // Update lockfile next to the install scope.
    let (lock_path, lock_env) = match target {
        InstallTarget::Local {
            workspace_root,
            env,
        } => (Lockfile::workspace_path(workspace_root), env.as_str()),
        InstallTarget::Global { env } => (
            Lockfile::global_path(&pixi_mise_pixi::pixi_home()),
            env.as_str(),
        ),
    };
    let mut lock = Lockfile::load(&lock_path)?;
    lock.upsert(
        lock_env,
        LockEntry {
            id: tool_id,
            version: tool.version.clone(),
            tag: tool.tag.clone(),
            asset: tool.asset.name.clone(),
            url: tool.asset.download_url.clone(),
            checksum: checksum.clone(),
            platform: host.pixi_platform(),
            installed_bins: installed_bins.clone(),
        },
    );
    lock.save(&lock_path)?;

    let _ = fs::remove_dir_all(&staging);

    Ok(InstallOutcome {
        tool: tool.clone(),
        installed_bins,
        exposed_bins,
        prefix,
        checksum,
    })
}

/// Convenience wrapper for local workspace installs (Phase 1 API).
pub fn install_tool_local(
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
    install_tool(client, tool, &target, host, dry_run)
}

fn cache_dir_for(owner: &str, repo: &str, tag: &str) -> PathBuf {
    let safe_tag = tag.replace('/', "_");
    cache_root().join(owner).join(repo).join(safe_tag)
}

/// Root directory for downloaded release assets.
pub fn cache_root() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        PathBuf::from(xdg).join("pixi-mise")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".cache").join("pixi-mise")
    } else {
        PathBuf::from(".cache").join("pixi-mise")
    }
}

/// Delete the entire pixi-mise download cache.
pub fn clear_cache() -> Result<PathBuf, CoreError> {
    let root = cache_root();
    if root.is_dir() {
        fs::remove_dir_all(&root).map_err(|e| CoreError::Install(e.to_string()))?;
    }
    Ok(root)
}

/// Remove a cached asset so the next install re-downloads it.
pub fn invalidate_cached_asset(
    owner: &str,
    repo: &str,
    tag: &str,
    asset: &str,
) -> Result<(), CoreError> {
    let dir = cache_dir_for(owner, repo, tag);
    let archive = dir.join(asset);
    if archive.is_file() {
        fs::remove_file(&archive).map_err(|e| CoreError::Install(e.to_string()))?;
    }
    let staging = dir.join("staging");
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    Ok(())
}
