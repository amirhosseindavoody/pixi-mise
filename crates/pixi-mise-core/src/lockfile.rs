//! `pixi-mise.lock` read/write and sha256 helpers.
//!
//! Schema mirrors [`pixi.lock`](https://pixi.prefix.dev/latest/workspace/lock_file/):
//! a versioned YAML document with per-environment package refs and a deduplicated
//! `packages` table keyed by download URL.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::CoreError;

/// Current `pixi-mise.lock` schema version.
pub const LOCKFILE_VERSION: u32 = 1;

/// One locked tool install (flattened view used by resolve / install).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockEntry {
    /// Tool id (`github:owner/repo`).
    pub id: String,
    /// Concrete version string.
    pub version: String,
    /// GitHub tag name.
    pub tag: String,
    /// Asset filename.
    pub asset: String,
    /// Download URL.
    pub url: String,
    /// Checksum (`sha256:…`), when known.
    pub checksum: Option<String>,
    /// Pixi platform key (`linux-64`, …).
    pub platform: String,
    /// Binaries installed into the prefix.
    pub installed_bins: Vec<String>,
}

/// Platform entry (pixi.lock-style).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformEntry {
    /// Platform name (`linux-64`, `osx-arm64`, …).
    pub name: String,
}

/// Reference to a package in an environment (like pixi's `conda: <url>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRef {
    /// Download URL for the GitHub release asset.
    pub github: String,
}

/// Deduplicated package record (like pixi's `packages:` entries).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRecord {
    /// Download URL (identity key, same as [`PackageRef::github`]).
    pub github: String,
    /// Tool id (`github:owner/repo`).
    pub id: String,
    /// Concrete version string.
    pub version: String,
    /// GitHub tag name.
    pub tag: String,
    /// Asset filename.
    pub asset: String,
    /// Pixi platform / subdir (`linux-64`, …).
    pub subdir: String,
    /// Bare hex SHA-256 (no `sha256:` prefix), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Asset size in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Binaries installed into the prefix.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bins: Vec<String>,
}

/// Per-environment package lists keyed by platform.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvironmentLock {
    /// `platform → [package refs]`.
    #[serde(default)]
    pub packages: BTreeMap<String, Vec<PackageRef>>,
}

/// Full lockfile document (pixi.lock-inspired YAML).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lockfile {
    /// Schema version.
    pub version: u32,
    /// Platforms present in this lockfile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub platforms: Vec<PlatformEntry>,
    /// Environment → packages-per-platform.
    #[serde(default)]
    pub environments: BTreeMap<String, EnvironmentLock>,
    /// Deduplicated package metadata.
    #[serde(default)]
    pub packages: Vec<PackageRecord>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self {
            version: LOCKFILE_VERSION,
            platforms: Vec::new(),
            environments: BTreeMap::new(),
            packages: Vec::new(),
        }
    }
}

impl Lockfile {
    /// Path to the workspace lockfile beside `pixi.toml`.
    pub fn workspace_path(workspace_root: &Path) -> PathBuf {
        workspace_root.join("pixi-mise.lock")
    }

    /// Path to the global lockfile under `$PIXI_HOME`.
    pub fn global_path(pixi_home: &Path) -> PathBuf {
        pixi_home.join("pixi-mise.lock")
    }

    /// Load a lockfile, or empty if missing.
    ///
    /// Accepts the current YAML schema. Legacy TOML `[[tools]]` files are migrated
    /// into the new shape (written back on the next [`Self::save`]).
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path).map_err(|e| CoreError::Config(e.to_string()))?;
        let trimmed = text.trim_start();
        // Skip generated comment headers.
        let body = trimmed
            .lines()
            .skip_while(|l| l.starts_with('#') || l.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");

        if body.trim_start().starts_with("version:")
            || body.trim_start().starts_with("environments:")
            || body.trim_start().starts_with("packages:")
        {
            return serde_yaml::from_str(&body)
                .map_err(|e| CoreError::Config(format!("parse lockfile: {e}")));
        }

        // Legacy TOML `[[tools]]` migration.
        if body.contains("[[tools]]") || body.trim_start().starts_with("tools") {
            return migrate_legacy_toml(&body);
        }

        // Prefer YAML; fall back to legacy TOML parse.
        match serde_yaml::from_str::<Self>(&body) {
            Ok(lock) => Ok(lock),
            Err(yaml_err) => migrate_legacy_toml(&body).map_err(|e| {
                CoreError::Config(format!("parse lockfile (yaml: {yaml_err}; legacy: {e})"))
            }),
        }
    }

    /// Write the lockfile as YAML (pixi.lock-like).
    pub fn save(&self, path: &Path) -> Result<(), CoreError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| CoreError::Config(e.to_string()))?;
        }
        let mut sorted = self.clone();
        sorted.normalize();
        let rendered = serde_yaml::to_string(&sorted)
            .map_err(|e| CoreError::Config(format!("serialize lockfile: {e}")))?;
        let header = "# This file is automatically @generated by pixi-mise.\n\
                      # It is not intended for manual editing.\n";
        fs::write(path, format!("{header}{rendered}"))
            .map_err(|e| CoreError::Config(e.to_string()))?;
        Ok(())
    }

    /// Sort platforms / packages / refs for stable diffs.
    fn normalize(&mut self) {
        self.version = LOCKFILE_VERSION;
        self.platforms.sort_by(|a, b| a.name.cmp(&b.name));
        self.platforms.dedup_by(|a, b| a.name == b.name);
        self.packages.sort_by(|a, b| {
            a.id.cmp(&b.id)
                .then_with(|| a.subdir.cmp(&b.subdir))
                .then_with(|| a.github.cmp(&b.github))
        });
        for env in self.environments.values_mut() {
            for refs in env.packages.values_mut() {
                refs.sort_by(|a, b| a.github.cmp(&b.github));
                refs.dedup_by(|a, b| a.github == b.github);
            }
        }
    }

    /// Find a locked entry for `env` + `id` + `platform`.
    pub fn find(&self, env: &str, id: &str, platform: &str) -> Option<LockEntry> {
        let refs = self
            .environments
            .get(env)
            .and_then(|e| e.packages.get(platform))?;
        for r in refs {
            if let Some(pkg) = self.packages.iter().find(|p| p.github == r.github)
                && pkg.id == id
            {
                return Some(package_to_entry(pkg));
            }
        }
        None
    }

    /// Find any package with `id` + `platform` (any environment).
    pub fn find_by_id_platform(&self, id: &str, platform: &str) -> Option<LockEntry> {
        self.packages
            .iter()
            .find(|p| p.id == id && p.subdir == platform)
            .map(package_to_entry)
    }

    /// Upsert a package into `env` for its platform.
    pub fn upsert(&mut self, env: &str, entry: LockEntry) {
        let url = entry.url.clone();
        let platform = entry.platform.clone();

        if !self.platforms.iter().any(|p| p.name == platform) {
            self.platforms.push(PlatformEntry {
                name: platform.clone(),
            });
        }

        let record = PackageRecord {
            github: url.clone(),
            id: entry.id.clone(),
            version: entry.version.clone(),
            tag: entry.tag.clone(),
            asset: entry.asset.clone(),
            subdir: platform.clone(),
            sha256: entry.checksum.as_deref().map(to_bare_sha256),
            size: None,
            bins: entry.installed_bins.clone(),
        };

        if let Some(existing) = self.packages.iter_mut().find(|p| p.github == url) {
            *existing = record;
        } else {
            self.packages.push(record);
        }

        let env_lock = self.environments.entry(env.to_string()).or_default();
        let refs = env_lock.packages.entry(platform).or_default();
        if !refs.iter().any(|r| r.github == url) {
            refs.push(PackageRef { github: url });
        }
    }

    /// Remove all packages with the given tool id (all envs / platforms).
    pub fn remove_id(&mut self, id: &str) {
        let urls: Vec<String> = self
            .packages
            .iter()
            .filter(|p| p.id == id)
            .map(|p| p.github.clone())
            .collect();
        self.packages.retain(|p| p.id != id);
        for env in self.environments.values_mut() {
            for refs in env.packages.values_mut() {
                refs.retain(|r| !urls.contains(&r.github));
            }
            env.packages.retain(|_, refs| !refs.is_empty());
        }
        self.environments.retain(|_, e| !e.packages.is_empty());
        let used_platforms: std::collections::BTreeSet<_> =
            self.packages.iter().map(|p| p.subdir.as_str()).collect();
        self.platforms
            .retain(|p| used_platforms.contains(p.name.as_str()));
    }
}

fn package_to_entry(pkg: &PackageRecord) -> LockEntry {
    LockEntry {
        id: pkg.id.clone(),
        version: pkg.version.clone(),
        tag: pkg.tag.clone(),
        asset: pkg.asset.clone(),
        url: pkg.github.clone(),
        checksum: pkg.sha256.as_deref().map(to_digest),
        platform: pkg.subdir.clone(),
        installed_bins: pkg.bins.clone(),
    }
}

fn to_bare_sha256(s: &str) -> String {
    s.strip_prefix("sha256:")
        .unwrap_or(s)
        .trim()
        .to_ascii_lowercase()
}

fn to_digest(s: &str) -> String {
    if s.starts_with("sha256:") {
        s.to_string()
    } else {
        format!("sha256:{}", s.trim().to_ascii_lowercase())
    }
}

#[derive(Debug, Deserialize)]
struct LegacyLockfile {
    #[serde(default)]
    tools: Vec<LegacyTool>,
}

#[derive(Debug, Deserialize)]
struct LegacyTool {
    id: String,
    version: String,
    tag: String,
    asset: String,
    url: String,
    #[serde(default)]
    checksum: Option<String>,
    platform: String,
    #[serde(default)]
    installed_bins: Vec<String>,
}

fn migrate_legacy_toml(text: &str) -> Result<Lockfile, CoreError> {
    let legacy: LegacyLockfile = toml::from_str(text)
        .map_err(|e| CoreError::Config(format!("parse legacy lockfile: {e}")))?;
    let mut lock = Lockfile::default();
    for tool in legacy.tools {
        lock.upsert(
            "default",
            LockEntry {
                id: tool.id,
                version: tool.version,
                tag: tool.tag,
                asset: tool.asset,
                url: tool.url,
                checksum: tool.checksum,
                platform: tool.platform,
                installed_bins: tool.installed_bins,
            },
        );
    }
    Ok(lock)
}

/// Compute `sha256:<hex>` for a file.
pub fn sha256_file(path: &Path) -> Result<String, CoreError> {
    let mut file = fs::File::open(path).map_err(|e| CoreError::Install(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| CoreError::Install(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

/// Verify a file against an expected `sha256:…` digest.
pub fn verify_sha256(path: &Path, expected: &str) -> Result<(), CoreError> {
    let actual = sha256_file(path)?;
    let expected = to_digest(expected);
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(CoreError::Install(format!(
            "checksum mismatch for {}: expected {expected}, got {actual}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn lockfile_roundtrip_yaml() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("pixi-mise-lock-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pixi-mise.lock");
        let mut lock = Lockfile::default();
        lock.upsert(
            "default",
            LockEntry {
                id: "github:cli/cli".into(),
                version: "2.67.0".into(),
                tag: "v2.67.0".into(),
                asset: "gh_2.67.0_linux_amd64.tar.gz".into(),
                url: "https://example.com/gh.tgz".into(),
                checksum: Some("sha256:abc".into()),
                platform: "linux-64".into(),
                installed_bins: vec!["gh".into()],
            },
        );
        lock.upsert(
            "test",
            LockEntry {
                id: "github:cli/cli".into(),
                version: "2.67.0".into(),
                tag: "v2.67.0".into(),
                asset: "gh_2.67.0_linux_amd64.tar.gz".into(),
                url: "https://example.com/gh.tgz".into(),
                checksum: Some("sha256:abc".into()),
                platform: "linux-64".into(),
                installed_bins: vec!["gh".into()],
            },
        );
        lock.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("version: 1"));
        assert!(text.contains("environments:"));
        assert!(text.contains("packages:"));
        assert!(text.contains("github: https://example.com/gh.tgz"));
        assert!(text.contains("sha256: abc"));
        assert_eq!(
            text.matches("github: https://example.com/gh.tgz").count(),
            3
        ); // 2 refs + 1 package

        let loaded = Lockfile::load(&path).unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.packages.len(), 1);
        assert!(
            loaded
                .find("default", "github:cli/cli", "linux-64")
                .is_some()
        );
        assert!(loaded.find("test", "github:cli/cli", "linux-64").is_some());
        assert_eq!(
            loaded
                .find("default", "github:cli/cli", "linux-64")
                .unwrap()
                .checksum
                .as_deref(),
            Some("sha256:abc")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrates_legacy_toml() {
        let legacy = r#"
[[tools]]
id = "github:BurntSushi/ripgrep"
version = "14.1.1"
tag = "14.1.1"
asset = "ripgrep.tar.gz"
url = "https://example.com/rg.tgz"
checksum = "sha256:deadbeef"
platform = "linux-64"
installed_bins = ["rg"]
"#;
        let lock = migrate_legacy_toml(legacy).unwrap();
        let entry = lock
            .find("default", "github:BurntSushi/ripgrep", "linux-64")
            .unwrap();
        assert_eq!(entry.version, "14.1.1");
        assert_eq!(entry.checksum.as_deref(), Some("sha256:deadbeef"));
    }

    #[test]
    fn sha256_stable() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("pixi-mise-sha-{nanos}"));
        {
            let mut f = fs::File::create(&path).unwrap();
            f.write_all(b"hello").unwrap();
        }
        let digest = sha256_file(&path).unwrap();
        assert_eq!(
            digest,
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        verify_sha256(&path, &digest).unwrap();
        verify_sha256(
            &path,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        )
        .unwrap();
        let _ = fs::remove_file(&path);
    }
}
