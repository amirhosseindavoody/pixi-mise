//! Pixi environment, prefix, and global exposure helpers.
//!
//! Phase 0 placeholder — prefix discovery and bin install land in Phase 1.

#![deny(missing_docs)]

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Errors from Pixi environment integration.
#[derive(Debug, Error)]
pub enum PixiError {
    /// Feature not implemented yet.
    #[error("Pixi adapter not implemented yet")]
    NotImplemented,
    /// Local environment prefix is missing.
    #[error("Pixi environment prefix not found at {0}")]
    MissingPrefix(PathBuf),
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

/// Resolve the environment prefix path for an install target.
///
/// Not fully implemented in Phase 0.
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
        InstallTarget::Global { .. } => Err(PixiError::NotImplemented),
    }
}

/// Return `$PREFIX/bin` for a resolved prefix.
pub fn bin_dir(prefix: &Path) -> PathBuf {
    prefix.join("bin")
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
