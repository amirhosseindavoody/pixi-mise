//! Aqua-registry consumption and platform filters.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use pixi_mise_assets::HostPlatform;
use pixi_mise_github::GithubClient;
use serde::Deserialize;

use crate::install::cache_root;
use crate::version::{normalize_tag, parse_version};
use crate::{CoreError, ToolId, ToolOptions};

/// Default remote aqua-registry (per-package YAML under `pkgs/`).
pub const DEFAULT_AQUA_REGISTRY_BASE: &str =
    "https://raw.githubusercontent.com/aquaproj/aqua-registry/main";

/// Registry lookup / recipe application settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrySettings {
    /// Whether to consult the registry when `asset_pattern` is unset.
    pub enabled: bool,
    /// Base URL for aqua-registry package files (no trailing slash).
    pub aqua_base_url: String,
    /// Optional local slim registry file (`pixi-mise-registry.toml`).
    pub local_path: Option<PathBuf>,
}

impl Default for RegistrySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            aqua_base_url: DEFAULT_AQUA_REGISTRY_BASE.to_string(),
            local_path: None,
        }
    }
}

/// Hints derived from a registry recipe for a concrete host + version.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RegistryHints {
    /// Expanded-ready asset template (aqua placeholders still present until pick time).
    pub asset_pattern: Option<String>,
    /// Preferred binary name from `files`.
    pub bin: Option<String>,
    /// Archive format (`tar.gz`, `zip`, `raw`, …).
    pub format: Option<String>,
    /// OS/Arch replacements.
    pub replacements: HashMap<String, String>,
    /// Whether the host is listed in `supported_envs` (true if unset).
    pub supported: bool,
    /// Human-readable source (`aqua:owner/repo`, `local:…`).
    pub source: String,
}

/// Whether the host matches a user-declared `os = [...]` filter.
///
/// Accepted entries: `linux`, `macos`, `darwin`, `windows`, `macos/arm64`,
/// `linux/amd64`, Pixi keys (`linux-64`, `osx-arm64`), or `*` / `all`.
pub fn host_matches_os_filter(host: &HostPlatform, filter: &[String]) -> bool {
    if filter.is_empty() {
        return true;
    }
    filter.iter().any(|entry| env_matches_host(entry, host))
}

/// Whether an aqua `supported_envs` entry matches the host.
pub fn env_matches_host(entry: &str, host: &HostPlatform) -> bool {
    let entry = entry.trim();
    if entry.is_empty() || entry == "*" || entry.eq_ignore_ascii_case("all") {
        return true;
    }

    let goos = host_goos(host);
    let goarch = host_goarch(host);
    let pixi = host.pixi_platform();

    // Pixi platform keys.
    if entry == pixi {
        return true;
    }
    // Friendly aliases.
    match entry {
        "macos" | "osx" if host.os == "macos" => return true,
        "linux" if host.os == "linux" => return true,
        "windows" | "win" if host.os == "windows" => return true,
        _ => {}
    }

    if let Some((os, arch)) = entry.split_once('/') {
        let os_ok = match os {
            "macos" | "osx" | "darwin" => goos == "darwin",
            "linux" => goos == "linux",
            "windows" | "win" => goos == "windows",
            other => other == goos,
        };
        let arch_ok = match arch {
            "x64" | "x86_64" | "amd64" => goarch == "amd64",
            "arm64" | "aarch64" => goarch == "arm64",
            "x86" | "386" => goarch == "386",
            other => other == goarch,
        };
        return os_ok && arch_ok;
    }

    // Bare OS or arch (aqua style: `darwin`, `amd64`).
    match entry {
        "darwin" | "linux" | "windows" => entry == goos,
        "amd64" | "arm64" | "386" | "arm" => entry == goarch,
        _ => false,
    }
}

fn host_goos(host: &HostPlatform) -> &str {
    match host.os.as_str() {
        "macos" => "darwin",
        other => other,
    }
}

fn host_goarch(host: &HostPlatform) -> &str {
    match host.arch.as_str() {
        "x64" => "amd64",
        "arm64" => "arm64",
        "x86" => "386",
        other => other,
    }
}

/// Look up registry hints for `id` @ `tag` on `host`.
///
/// Prefers a local slim registry when configured / present, then aqua-registry.
/// Returns `Ok(None)` when disabled or the package is unknown (fail open).
pub fn lookup_registry_hints(
    client: &GithubClient,
    settings: &RegistrySettings,
    id: &ToolId,
    tag: &str,
    host: &HostPlatform,
) -> Result<Option<RegistryHints>, CoreError> {
    if !settings.enabled {
        return Ok(None);
    }

    if let Some(path) = settings.local_path.as_ref()
        && path.is_file()
        && let Some(hints) = lookup_local_registry(path, id, tag, host)?
    {
        return Ok(Some(hints));
    } else if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("pixi-mise-registry.toml");
        if candidate.is_file()
            && let Some(hints) = lookup_local_registry(&candidate, id, tag, host)?
        {
            return Ok(Some(hints));
        }
    }

    match fetch_aqua_package(client, &settings.aqua_base_url, id)? {
        Some(pkg) => Ok(Some(hints_from_aqua_package(&pkg, tag, host, id)?)),
        None => Ok(None),
    }
}

/// Apply registry hints onto tool options without clobbering explicit user overrides.
pub fn merge_registry_hints(options: &mut ToolOptions, hints: &RegistryHints) {
    if options.asset_pattern.is_none() {
        options.asset_pattern = hints.asset_pattern.clone();
    }
    if options.bin.is_none() {
        options.bin = hints.bin.clone();
    }
    if options.registry_format.is_none() {
        options.registry_format = hints.format.clone();
    }
    if options.registry_replacements.is_none() && !hints.replacements.is_empty() {
        options.registry_replacements = Some(hints.replacements.clone());
    }
}

fn lookup_local_registry(
    path: &Path,
    id: &ToolId,
    tag: &str,
    host: &HostPlatform,
) -> Result<Option<RegistryHints>, CoreError> {
    let text = fs::read_to_string(path).map_err(|e| CoreError::Config(e.to_string()))?;
    let doc: LocalRegistry = toml::from_str(&text)
        .map_err(|e| CoreError::Config(format!("parse {}: {e}", path.display())))?;
    let key = id.github_spec();
    let Some(pkg) = doc
        .packages
        .iter()
        .find(|p| p.id == key || p.id == id.as_str())
    else {
        return Ok(None);
    };

    let supported = pkg
        .supported_envs
        .as_ref()
        .map(|envs| envs.iter().any(|e| env_matches_host(e, host)))
        .unwrap_or(true);

    let _ = tag; // local recipes are version-agnostic in v1
    Ok(Some(RegistryHints {
        asset_pattern: pkg.asset.clone(),
        bin: pkg.bin.clone(),
        format: pkg.format.clone(),
        replacements: pkg.replacements.clone().unwrap_or_default(),
        supported,
        source: format!("local:{}", path.display()),
    }))
}

fn fetch_aqua_package(
    client: &GithubClient,
    base_url: &str,
    id: &ToolId,
) -> Result<Option<AquaPackage>, CoreError> {
    let cache_path = registry_cache_path(id);
    if cache_path.is_file()
        && let Ok(text) = fs::read_to_string(&cache_path)
        && let Ok(file) = parse_aqua_registry_yaml(&text)
        && let Some(pkg) = file.packages.into_iter().next()
    {
        return Ok(Some(pkg));
    }

    let url = format!(
        "{}/pkgs/{}/{}/registry.yaml",
        base_url.trim_end_matches('/'),
        id.owner,
        id.repo
    );
    let text = match client.get_text(&url) {
        Ok(t) => t,
        Err(pixi_mise_github::GithubError::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if let Some(parent) = cache_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&cache_path, &text);

    let file = parse_aqua_registry_yaml(&text)?;
    Ok(file.packages.into_iter().next())
}

fn registry_cache_path(id: &ToolId) -> PathBuf {
    cache_root()
        .join("registry")
        .join("aqua")
        .join(&id.owner)
        .join(format!("{}.yaml", id.repo))
}

fn parse_aqua_registry_yaml(text: &str) -> Result<AquaRegistryFile, CoreError> {
    serde_yaml::from_str(text).map_err(|e| CoreError::Config(format!("parse aqua registry: {e}")))
}

fn hints_from_aqua_package(
    base: &AquaPackage,
    tag: &str,
    host: &HostPlatform,
    id: &ToolId,
) -> Result<RegistryHints, CoreError> {
    if base.type_.as_deref().is_some_and(|t| t != "github_release") {
        return Ok(RegistryHints {
            supported: true,
            source: format!("aqua:{}/{} (skipped non-github_release)", id.owner, id.repo),
            ..RegistryHints::default()
        });
    }

    let selected = select_version_package(base, tag);
    let mut effective = merge_aqua_package(base, &selected);
    apply_platform_overrides(&mut effective, host);

    let supported = effective
        .supported_envs
        .as_ref()
        .map(|envs| envs.iter().any(|e| env_matches_host(e, host)))
        .unwrap_or(true);

    let bin = effective
        .files
        .as_ref()
        .and_then(|files| files.first())
        .map(|f| f.name.clone());

    Ok(RegistryHints {
        asset_pattern: effective.asset.clone(),
        bin,
        format: effective.format.clone(),
        replacements: effective.replacements.clone().unwrap_or_default(),
        supported,
        source: format!("aqua:{}/{}", id.owner, id.repo),
    })
}

fn select_version_package(base: &AquaPackage, tag: &str) -> AquaPackage {
    let version = normalize_tag(tag);
    if let Some(overrides) = &base.version_overrides {
        for ov in overrides {
            if constraint_matches(ov.version_constraint.as_deref(), &version, tag) {
                return ov.clone();
            }
        }
    }
    base.clone()
}

fn merge_aqua_package(base: &AquaPackage, ov: &AquaPackage) -> AquaPackage {
    AquaPackage {
        type_: ov.type_.clone().or_else(|| base.type_.clone()),
        repo_owner: ov.repo_owner.clone().or_else(|| base.repo_owner.clone()),
        repo_name: ov.repo_name.clone().or_else(|| base.repo_name.clone()),
        asset: ov.asset.clone().or_else(|| base.asset.clone()),
        format: ov.format.clone().or_else(|| base.format.clone()),
        replacements: ov
            .replacements
            .clone()
            .or_else(|| base.replacements.clone()),
        overrides: ov.overrides.clone().or_else(|| base.overrides.clone()),
        format_overrides: ov
            .format_overrides
            .clone()
            .or_else(|| base.format_overrides.clone()),
        files: ov.files.clone().or_else(|| base.files.clone()),
        supported_envs: ov
            .supported_envs
            .clone()
            .or_else(|| base.supported_envs.clone()),
        version_constraint: ov.version_constraint.clone(),
        version_overrides: None,
        rosetta2: ov.rosetta2.or(base.rosetta2),
        windows_arm_emulation: ov.windows_arm_emulation.or(base.windows_arm_emulation),
    }
}

fn apply_platform_overrides(pkg: &mut AquaPackage, host: &HostPlatform) {
    let goos = host_goos(host).to_string();
    let goarch = host_goarch(host).to_string();

    if let Some(format_overrides) = pkg.format_overrides.clone() {
        for fo in format_overrides {
            if fo.goos.as_deref() == Some(goos.as_str()) {
                pkg.format = Some(fo.format);
            }
        }
    }

    let Some(overrides) = pkg.overrides.clone() else {
        return;
    };
    for ov in overrides {
        let os_ok = ov.goos.as_deref().is_none_or(|o| o == goos);
        let arch_ok = ov.goarch.as_deref().is_none_or(|a| a == goarch);
        if !(os_ok && arch_ok) {
            continue;
        }
        if let Some(asset) = ov.asset {
            pkg.asset = Some(asset);
        }
        if let Some(format) = ov.format {
            pkg.format = Some(format);
        }
        if let Some(files) = ov.files {
            pkg.files = Some(files);
        }
        if let Some(reps) = ov.replacements {
            let mut merged = pkg.replacements.clone().unwrap_or_default();
            merged.extend(reps);
            pkg.replacements = Some(merged);
        }
    }
}

/// Evaluate a subset of aqua `version_constraint` expressions.
pub fn constraint_matches(expr: Option<&str>, normalized_version: &str, raw_tag: &str) -> bool {
    let Some(expr) = expr.map(str::trim).filter(|s| !s.is_empty()) else {
        return true;
    };
    if expr == "true" {
        return true;
    }
    if expr == "false" {
        return false;
    }

    let ver = parse_version(normalized_version);

    if let Some(rest) = expr.strip_prefix("Version == ") {
        let want = rest.trim().trim_matches('"').trim_matches('\'');
        let want_norm = normalize_tag(want);
        return want_norm == normalized_version
            || want == raw_tag
            || want == normalized_version
            || format!("v{normalized_version}") == want;
    }

    if let Some(inner) = expr
        .strip_prefix("semver(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .or_else(|| {
            expr.strip_prefix("semver('")
                .and_then(|s| s.strip_suffix("')"))
        })
    {
        let inner = inner.trim();
        if let Some(bound) = inner.strip_prefix("<=") {
            let bound = parse_version(bound.trim());
            return ver <= bound;
        }
        if let Some(bound) = inner.strip_prefix(">=") {
            let bound = parse_version(bound.trim());
            return ver >= bound;
        }
        if let Some(bound) = inner.strip_prefix('<') {
            let bound = parse_version(bound.trim());
            return ver < bound;
        }
        if let Some(bound) = inner.strip_prefix('>') {
            let bound = parse_version(bound.trim());
            return ver > bound;
        }
        if let Some(bound) = inner.strip_prefix("==") {
            let bound = parse_version(bound.trim());
            return ver == bound;
        }
    }

    // Unknown expression — do not match (except we already handled true/false).
    tracing::debug!(expr, "unsupported aqua version_constraint; skipping");
    false
}

#[derive(Debug, Clone, Deserialize)]
struct LocalRegistry {
    #[serde(default)]
    packages: Vec<LocalPackage>,
}

#[derive(Debug, Clone, Deserialize)]
struct LocalPackage {
    id: String,
    asset: Option<String>,
    bin: Option<String>,
    format: Option<String>,
    #[serde(default)]
    replacements: Option<HashMap<String, String>>,
    #[serde(default)]
    supported_envs: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct AquaRegistryFile {
    #[serde(default)]
    packages: Vec<AquaPackage>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct AquaPackage {
    #[serde(rename = "type", default)]
    type_: Option<String>,
    #[serde(default)]
    repo_owner: Option<String>,
    #[serde(default)]
    repo_name: Option<String>,
    #[serde(default)]
    asset: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    replacements: Option<HashMap<String, String>>,
    #[serde(default)]
    overrides: Option<Vec<AquaOverride>>,
    #[serde(default)]
    format_overrides: Option<Vec<AquaFormatOverride>>,
    #[serde(default)]
    files: Option<Vec<AquaFile>>,
    #[serde(default)]
    supported_envs: Option<Vec<String>>,
    #[serde(default)]
    version_constraint: Option<String>,
    #[serde(default)]
    version_overrides: Option<Vec<AquaPackage>>,
    #[serde(default)]
    rosetta2: Option<bool>,
    #[serde(default)]
    windows_arm_emulation: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct AquaOverride {
    #[serde(default)]
    goos: Option<String>,
    #[serde(default)]
    goarch: Option<String>,
    #[serde(default)]
    asset: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    files: Option<Vec<AquaFile>>,
    #[serde(default)]
    replacements: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
struct AquaFormatOverride {
    goos: Option<String>,
    format: String,
}

#[derive(Debug, Clone, Deserialize)]
struct AquaFile {
    name: String,
    #[serde(default)]
    #[allow(dead_code)]
    src: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux_x64() -> HostPlatform {
        HostPlatform {
            os: "linux".into(),
            arch: "x64".into(),
            libc: Some("gnu".into()),
        }
    }

    fn macos_arm64() -> HostPlatform {
        HostPlatform {
            os: "macos".into(),
            arch: "arm64".into(),
            libc: None,
        }
    }

    #[test]
    fn os_filter_matches_aliases() {
        assert!(host_matches_os_filter(
            &linux_x64(),
            &["linux".into(), "macos/arm64".into()]
        ));
        assert!(!host_matches_os_filter(
            &linux_x64(),
            &["macos".into(), "windows".into()]
        ));
        assert!(host_matches_os_filter(
            &macos_arm64(),
            &["macos/arm64".into()]
        ));
        assert!(host_matches_os_filter(
            &macos_arm64(),
            &["osx-arm64".into()]
        ));
    }

    #[test]
    fn supported_envs_bare_arch() {
        assert!(env_matches_host("amd64", &linux_x64()));
        assert!(!env_matches_host("arm64", &linux_x64()));
        assert!(env_matches_host("darwin", &macos_arm64()));
        assert!(env_matches_host("linux/amd64", &linux_x64()));
    }

    #[test]
    fn semver_constraints() {
        assert!(constraint_matches(
            Some("semver(\">= 1.0.0\")"),
            "1.2.3",
            "v1.2.3"
        ));
        assert!(constraint_matches(
            Some("semver(\"<= 14.1.1\")"),
            "14.1.1",
            "14.1.1"
        ));
        assert!(!constraint_matches(
            Some("semver(\"<= 13.0.0\")"),
            "14.1.1",
            "14.1.1"
        ));
        assert!(constraint_matches(Some("true"), "9.9.9", "v9.9.9"));
        assert!(!constraint_matches(Some("false"), "9.9.9", "v9.9.9"));
        assert!(constraint_matches(
            Some("Version == \"v1.2.3\""),
            "1.2.3",
            "v1.2.3"
        ));
    }

    #[test]
    fn parses_cli_aqua_fixture_and_selects_latest() {
        let yaml = r#"
packages:
  - type: github_release
    repo_owner: cli
    repo_name: cli
    files:
      - name: gh
    version_constraint: "false"
    version_overrides:
      - version_constraint: semver("<= 2.20.0")
        asset: old_{{trimV .Version}}.tar.gz
        format: tar.gz
      - version_constraint: "true"
        asset: gh_{{trimV .Version}}_{{.OS}}_{{.Arch}}.{{.Format}}
        format: zip
        replacements:
          darwin: macOS
        overrides:
          - goos: linux
            format: tar.gz
"#;
        let file = parse_aqua_registry_yaml(yaml).unwrap();
        let pkg = &file.packages[0];
        let hints = hints_from_aqua_package(
            pkg,
            "v2.67.0",
            &linux_x64(),
            &ToolId {
                owner: "cli".into(),
                repo: "cli".into(),
            },
        )
        .unwrap();
        assert_eq!(
            hints.asset_pattern.as_deref(),
            Some("gh_{{trimV .Version}}_{{.OS}}_{{.Arch}}.{{.Format}}")
        );
        assert_eq!(hints.format.as_deref(), Some("tar.gz"));
        assert_eq!(hints.bin.as_deref(), Some("gh"));
        assert_eq!(
            hints.replacements.get("darwin").map(String::as_str),
            Some("macOS")
        );
        assert!(hints.supported);
    }

    #[test]
    fn local_registry_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "pixi-mise-reg-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pixi-mise-registry.toml");
        fs::write(
            &path,
            r#"
[[packages]]
id = "github:cli/cli"
asset = "gh_{{trimV .Version}}_{{.OS}}_{{.Arch}}.tar.gz"
bin = "gh"
format = "tar.gz"
supported_envs = ["linux", "darwin"]
"#,
        )
        .unwrap();
        let hints = lookup_local_registry(
            &path,
            &ToolId {
                owner: "cli".into(),
                repo: "cli".into(),
            },
            "v2.0.0",
            &linux_x64(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(hints.bin.as_deref(), Some("gh"));
        assert!(hints.supported);
        let _ = fs::remove_dir_all(&dir);
    }
}
