//! Pixi environment, prefix, and binary install helpers.

#![deny(missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors from Pixi environment integration.
#[derive(Debug, Error)]
pub enum PixiError {
    /// Local environment prefix is missing.
    #[error(
        "Pixi environment prefix not found at {}\nRun `pixi install` in the workspace first.",
        .0.display()
    )]
    MissingPrefix(PathBuf),
    /// I/O failure.
    #[error("I/O error: {0}")]
    Io(String),
    /// Feature not implemented yet.
    #[error("{0} is not implemented yet")]
    NotImplemented(&'static str),
}

/// Where binaries should be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallTarget {
    /// Workspace-local Pixi env under `.pixi/envs/<env>/`.
    Local {
        /// Workspace root containing `pixi.toml`.
        workspace_root: PathBuf,
        /// Environment name (default: `default`).
        env: String,
    },
    /// Global Pixi env under `$PIXI_HOME/envs/<env>/`.
    Global {
        /// Environment name.
        env: String,
    },
}

/// Metadata recorded for an installed tool (list / remove).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledToolMeta {
    /// Tool id (`github:owner/repo`).
    pub id: String,
    /// Concrete version string.
    pub version: String,
    /// GitHub tag name.
    pub tag: String,
    /// Chosen asset filename.
    pub asset: String,
    /// Download URL used.
    pub url: String,
    /// Host / lock platform key.
    pub platform: String,
    /// Binary names installed into `$PREFIX/bin`.
    pub installed_bins: Vec<String>,
}

/// Resolve the environment prefix path for an install target.
pub fn resolve_prefix(target: &InstallTarget) -> Result<PathBuf, PixiError> {
    match target {
        InstallTarget::Local {
            workspace_root,
            env,
        } => {
            let prefix = workspace_root.join(".pixi").join("envs").join(env);
            if prefix.is_dir() {
                Ok(prefix)
            } else {
                Err(PixiError::MissingPrefix(prefix))
            }
        }
        InstallTarget::Global { .. } => Err(PixiError::NotImplemented("global install (Phase 2)")),
    }
}

/// Return `$PREFIX/bin` for a resolved prefix.
pub fn bin_dir(prefix: &Path) -> PathBuf {
    prefix.join("bin")
}

/// Ensure `$PREFIX/bin` exists.
pub fn ensure_bin_dir(prefix: &Path) -> Result<PathBuf, PixiError> {
    let dir = bin_dir(prefix);
    fs::create_dir_all(&dir).map_err(|e| PixiError::Io(e.to_string()))?;
    Ok(dir)
}

/// Default `PIXI_HOME` (`~/.pixi` when unset).
pub fn pixi_home() -> PathBuf {
    if let Ok(home) = std::env::var("PIXI_HOME") {
        return PathBuf::from(home);
    }
    dirs_home()
        .map(|h| h.join(".pixi"))
        .unwrap_or_else(|| PathBuf::from(".pixi"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Directory for pixi-mise install metadata inside a workspace.
pub fn mise_meta_dir(workspace_root: &Path, env: &str) -> PathBuf {
    workspace_root.join(".pixi").join("mise").join(env)
}

fn meta_path(workspace_root: &Path, env: &str, tool_id: &str) -> PathBuf {
    let safe = tool_id.replace([':', '/'], "--");
    mise_meta_dir(workspace_root, env).join(format!("{safe}.json"))
}

/// Write install metadata for a tool.
pub fn write_tool_meta(
    workspace_root: &Path,
    env: &str,
    meta: &InstalledToolMeta,
) -> Result<PathBuf, PixiError> {
    let dir = mise_meta_dir(workspace_root, env);
    fs::create_dir_all(&dir).map_err(|e| PixiError::Io(e.to_string()))?;
    let path = meta_path(workspace_root, env, &meta.id);
    let json = serde_json::to_string_pretty(meta).map_err(|e| PixiError::Io(e.to_string()))?;
    fs::write(&path, json).map_err(|e| PixiError::Io(e.to_string()))?;
    Ok(path)
}

/// Read install metadata for a tool, if present.
pub fn read_tool_meta(
    workspace_root: &Path,
    env: &str,
    tool_id: &str,
) -> Result<Option<InstalledToolMeta>, PixiError> {
    let path = meta_path(workspace_root, env, tool_id);
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path).map_err(|e| PixiError::Io(e.to_string()))?;
    let meta = serde_json::from_str(&text).map_err(|e| PixiError::Io(e.to_string()))?;
    Ok(Some(meta))
}

/// List all installed tool metadata for an environment.
pub fn list_tool_meta(
    workspace_root: &Path,
    env: &str,
) -> Result<Vec<InstalledToolMeta>, PixiError> {
    let dir = mise_meta_dir(workspace_root, env);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| PixiError::Io(e.to_string()))? {
        let entry = entry.map_err(|e| PixiError::Io(e.to_string()))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|e| PixiError::Io(e.to_string()))?;
        match serde_json::from_str::<InstalledToolMeta>(&text) {
            Ok(meta) => out.push(meta),
            Err(e) => tracing::warn!(?path, error = %e, "skipping invalid metadata"),
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Remove install metadata for a tool.
pub fn remove_tool_meta(workspace_root: &Path, env: &str, tool_id: &str) -> Result<(), PixiError> {
    let path = meta_path(workspace_root, env, tool_id);
    if path.is_file() {
        fs::remove_file(&path).map_err(|e| PixiError::Io(e.to_string()))?;
    }
    Ok(())
}

/// Install a binary file into `$PREFIX/bin/<name>` (copy; set executable bit on Unix).
pub fn install_binary(prefix: &Path, src: &Path, bin_name: &str) -> Result<PathBuf, PixiError> {
    let dest_dir = ensure_bin_dir(prefix)?;
    let dest = dest_dir.join(bin_name);
    if dest.exists() {
        fs::remove_file(&dest).map_err(|e| PixiError::Io(e.to_string()))?;
    }
    fs::copy(src, &dest).map_err(|e| PixiError::Io(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&dest)
            .map_err(|e| PixiError::Io(e.to_string()))?
            .permissions();
        perms.set_mode(perms.mode() | 0o755);
        fs::set_permissions(&dest, perms).map_err(|e| PixiError::Io(e.to_string()))?;
    }
    Ok(dest)
}

/// Remove installed binaries by name from `$PREFIX/bin`.
pub fn remove_binaries(prefix: &Path, bins: &[String]) -> Result<(), PixiError> {
    let dir = bin_dir(prefix);
    for bin in bins {
        let path = dir.join(bin);
        if path.is_file() {
            fs::remove_file(&path).map_err(|e| PixiError::Io(e.to_string()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("pixi-mise-pixi-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn meta_roundtrip() {
        let root = temp_dir();
        let meta = InstalledToolMeta {
            id: "github:cli/cli".into(),
            version: "2.67.0".into(),
            tag: "v2.67.0".into(),
            asset: "gh_2.67.0_linux_amd64.tar.gz".into(),
            url: "https://example.com".into(),
            platform: "linux-64".into(),
            installed_bins: vec!["gh".into()],
        };
        write_tool_meta(&root, "default", &meta).unwrap();
        let loaded = read_tool_meta(&root, "default", "github:cli/cli")
            .unwrap()
            .unwrap();
        assert_eq!(loaded, meta);
        let listed = list_tool_meta(&root, "default").unwrap();
        assert_eq!(listed.len(), 1);
        remove_tool_meta(&root, "default", "github:cli/cli").unwrap();
        assert!(
            read_tool_meta(&root, "default", "github:cli/cli")
                .unwrap()
                .is_none()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_prefix_errors() {
        let root = temp_dir();
        let err = resolve_prefix(&InstallTarget::Local {
            workspace_root: root.clone(),
            env: "default".into(),
        })
        .unwrap_err();
        assert!(matches!(err, PixiError::MissingPrefix(_)));
        let _ = fs::remove_dir_all(&root);
    }
}
