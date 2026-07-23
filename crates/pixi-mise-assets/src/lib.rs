//! Mise-inspired asset scoring for GitHub release binaries.

#![deny(missing_docs)]

use std::sync::OnceLock;

use regex::Regex;
use thiserror::Error;

/// Errors from asset matching.
#[derive(Debug, Error)]
pub enum AssetError {
    /// No asset scored high enough for the host platform.
    #[error("no matching asset for host platform")]
    NoMatch,
    /// Invalid `matching_regex` pattern.
    #[error("invalid matching_regex: {0}")]
    InvalidRegex(String),
}

/// A release asset candidate (name + optional size / URL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetCandidate {
    /// Filename as published on the release.
    pub name: String,
    /// Size in bytes, when known.
    pub size: Option<u64>,
    /// Browser download URL, when known.
    pub download_url: Option<String>,
}

/// Host platform used for scoring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPlatform {
    /// Normalized OS (`linux`, `macos`, `windows`).
    pub os: String,
    /// Normalized arch (`x64`, `arm64`, `x86`, `arm`, …).
    pub arch: String,
    /// Optional libc (`gnu`, `musl`, `msvc`).
    pub libc: Option<String>,
}

impl HostPlatform {
    /// Detect the current host OS / arch / libc defaults.
    pub fn detect() -> Self {
        let os = match std::env::consts::OS {
            "macos" => "macos".to_string(),
            "windows" => "windows".to_string(),
            other => other.to_string(),
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64".to_string(),
            "aarch64" => "arm64".to_string(),
            "x86" => "x86".to_string(),
            "arm" => "arm".to_string(),
            other => other.to_string(),
        };
        let libc = match os.as_str() {
            "windows" => Some("msvc".to_string()),
            "linux" => Some("gnu".to_string()),
            _ => None,
        };
        Self { os, arch, libc }
    }

    /// Map host to a Pixi-style platform key (`linux-64`, `osx-arm64`, …).
    pub fn pixi_platform(&self) -> String {
        match (self.os.as_str(), self.arch.as_str()) {
            ("linux", "x64") => "linux-64".into(),
            ("linux", "arm64") => "linux-aarch64".into(),
            ("macos", "x64") => "osx-64".into(),
            ("macos", "arm64") => "osx-arm64".into(),
            ("windows", "x64") => "win-64".into(),
            ("windows", "arm64") => "win-arm64".into(),
            (os, arch) => format!("{os}-{arch}"),
        }
    }
}

/// Result of picking the best asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedAsset {
    /// Chosen asset name.
    pub name: String,
    /// Score assigned by the matcher.
    pub score: i32,
    /// Download URL when the candidate carried one.
    pub download_url: Option<String>,
    /// Size in bytes when known.
    pub size: Option<u64>,
}

/// Options that refine asset selection.
#[derive(Debug, Clone, Default)]
pub struct PickOptions {
    /// Substring filter on asset names (case-sensitive contains).
    pub matching: Option<String>,
    /// Regex filter on asset names.
    pub matching_regex: Option<String>,
    /// Explicit asset pattern (glob); when set, skips OS/arch scoring.
    ///
    /// Supports `*`, `?`, and placeholders: `{{version}}` / `{{.Version}}`,
    /// `{{os}}` / `{{.OS}}`, `{{arch}}` / `{{.Arch}}`, `{{.Format}}`,
    /// `{{trimV .Version}}`.
    pub asset_pattern: Option<String>,
    /// Prefer assets whose name starts with this tool / repo name.
    pub preferred_name: Option<String>,
    /// Concrete version string used to expand `asset_pattern` placeholders.
    ///
    /// Prefer the raw GitHub tag (may include a leading `v`) when expanding
    /// aqua-style `{{.Version}}` templates.
    pub version: Option<String>,
    /// Archive format for `{{.Format}}` (e.g. `tar.gz`, `zip`).
    pub format: Option<String>,
    /// Aqua-style replacements applied to OS/Arch template values.
    pub replacements: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssetOs {
    Linux,
    Macos,
    Windows,
    Android,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssetArch {
    X64,
    Arm64,
    X86,
    Arm,
    Riscv64,
    Loongarch64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssetLibc {
    Gnu,
    Musl,
    Msvc,
}

impl AssetOs {
    fn matches_target(self, target: &str) -> bool {
        match self {
            Self::Linux => target == "linux",
            Self::Macos => target == "macos" || target == "darwin",
            Self::Windows => target == "windows",
            Self::Android => target == "android",
        }
    }
}

impl AssetArch {
    fn matches_target(self, target: &str) -> bool {
        match self {
            Self::X64 => matches!(target, "x86_64" | "amd64" | "x64"),
            Self::Arm64 => matches!(target, "aarch64" | "arm64"),
            Self::X86 => matches!(target, "x86" | "i386" | "i686"),
            Self::Arm => target == "arm",
            Self::Riscv64 => matches!(target, "riscv64" | "riscv64gc"),
            Self::Loongarch64 => matches!(target, "loongarch64" | "loong64"),
        }
    }
}

impl AssetLibc {
    fn matches_target(self, target: &str) -> bool {
        target.split('-').any(|part| match self {
            Self::Gnu => part == "gnu" || part == "glibc",
            Self::Musl => part == "musl",
            Self::Msvc => part == "msvc",
        })
    }
}

struct Patterns {
    os: Vec<(AssetOs, Regex)>,
    arch: Vec<(AssetArch, Regex)>,
    libc: Vec<(AssetLibc, Regex)>,
}

fn patterns() -> &'static Patterns {
    static PATTERNS: OnceLock<Patterns> = OnceLock::new();
    PATTERNS.get_or_init(|| Patterns {
        os: vec![
            (
                AssetOs::Android,
                Regex::new(r"(?i)(?:\b|_)(?:android)(?:\b|_)").unwrap(),
            ),
            (
                AssetOs::Linux,
                Regex::new(r"(?i)(?:\b|_)(?:linux|manylinux(?:[0-9_]+)?|musllinux(?:[0-9_]+)?|ubuntu|debian|fedora|centos|rhel|alpine|arch)(?:\b|_|32|64|-)")
                    .unwrap(),
            ),
            (
                AssetOs::Macos,
                Regex::new(r"(?i)(?:\b|_)(?:darwin|mac(?:osx?)?|osx)(?:\b|_)").unwrap(),
            ),
            (
                AssetOs::Windows,
                Regex::new(r"(?i)(?:\b|_)(?:mingw-w64|win(?:32|64|dows)?)(?:\b|_)").unwrap(),
            ),
        ],
        arch: vec![
            (
                AssetArch::X64,
                Regex::new(r"(?i)(?:\b|_)(?:x86[_-]64|x64|amd64)(?:\b|_)").unwrap(),
            ),
            (
                AssetArch::Arm64,
                Regex::new(r"(?i)(?:\b|_)(?:aarch_?64|arm_?64)(?:\b|_)").unwrap(),
            ),
            (
                AssetArch::X86,
                Regex::new(r"(?i)(?:\b|_)(?:x86|i386|i686)(?:\b|_)").unwrap(),
            ),
            (
                AssetArch::Arm,
                Regex::new(r"(?i)(?:\b|_)arm(?:v[0-7])?(?:\b|_)").unwrap(),
            ),
            (
                AssetArch::Riscv64,
                Regex::new(r"(?i)(?:\b|_)riscv_?64(?:gc)?(?:\b|_)").unwrap(),
            ),
            (
                AssetArch::Loongarch64,
                Regex::new(r"(?i)(?:\b|_)loong(?:arch)?_?64(?:\b|_)").unwrap(),
            ),
        ],
        libc: vec![
            (
                AssetLibc::Msvc,
                Regex::new(r"(?i)(?:\b|_)(?:msvc)(?:\b|_)").unwrap(),
            ),
            (
                AssetLibc::Gnu,
                Regex::new(r"(?i)(?:\b|_)(?:gnu|glibc|manylinux(?:[0-9_]+)?)(?:\b|_)").unwrap(),
            ),
            (
                AssetLibc::Musl,
                Regex::new(r"(?i)(?:\b|_)(?:musl|musllinux(?:[0-9_]+)?)(?:\b|_)").unwrap(),
            ),
        ],
    })
}

/// Filter and score release assets for the host platform.
pub fn pick_asset(
    candidates: &[AssetCandidate],
    host: &HostPlatform,
    options: &PickOptions,
) -> Result<PickedAsset, AssetError> {
    // `asset_pattern` replaces autodetection entirely.
    if let Some(pattern) = options.asset_pattern.as_deref() {
        return pick_by_asset_pattern(candidates, host, pattern, options);
    }

    let names: Vec<&AssetCandidate> =
        if options.matching.is_none() && options.matching_regex.is_none() {
            candidates.iter().collect()
        } else {
            apply_matching_filter(candidates, options)?
        };

    let mut best: Option<(i32, &AssetCandidate)> = None;
    for candidate in names {
        let score = score_asset(&candidate.name, host, options.preferred_name.as_deref());
        if score <= 0
            || has_arch_mismatch(&candidate.name, host, options.preferred_name.as_deref())
            || is_package_or_installer_asset(&candidate.name)
        {
            continue;
        }
        match &best {
            None => best = Some((score, candidate)),
            Some((best_score, best_cand)) => {
                let better = score
                    .cmp(best_score)
                    .then_with(|| best_cand.name.len().cmp(&candidate.name.len()))
                    .then_with(|| best_cand.name.cmp(&candidate.name))
                    .is_gt();
                if better {
                    best = Some((score, candidate));
                }
            }
        }
    }

    let (score, candidate) = best.ok_or(AssetError::NoMatch)?;
    Ok(PickedAsset {
        name: candidate.name.clone(),
        score,
        download_url: candidate.download_url.clone(),
        size: candidate.size,
    })
}

fn pick_by_asset_pattern(
    candidates: &[AssetCandidate],
    host: &HostPlatform,
    pattern: &str,
    options: &PickOptions,
) -> Result<PickedAsset, AssetError> {
    let expanded = expand_asset_template(
        pattern,
        &TemplateContext {
            host,
            version: options.version.as_deref(),
            format: options.format.as_deref(),
            replacements: options.replacements.as_ref(),
        },
    );
    let mut matches: Vec<&AssetCandidate> = candidates
        .iter()
        .filter(|c| glob_match(&expanded, &c.name))
        .collect();
    if matches.is_empty() {
        tracing::debug!(pattern = %expanded, "asset_pattern matched no assets");
        return Err(AssetError::NoMatch);
    }
    matches.sort_by(|a, b| {
        a.name
            .len()
            .cmp(&b.name.len())
            .then_with(|| a.name.cmp(&b.name))
    });
    let candidate = matches[0];
    Ok(PickedAsset {
        name: candidate.name.clone(),
        score: 1000,
        download_url: candidate.download_url.clone(),
        size: candidate.size,
    })
}

/// Inputs for expanding aqua/mise asset templates.
#[derive(Debug, Clone, Copy)]
pub struct TemplateContext<'a> {
    /// Host platform (OS/arch mapped to aqua GOOS/GOARCH before replacements).
    pub host: &'a HostPlatform,
    /// Version / tag string for `{{.Version}}`.
    pub version: Option<&'a str>,
    /// Archive format for `{{.Format}}`.
    pub format: Option<&'a str>,
    /// Optional aqua `replacements` map.
    pub replacements: Option<&'a std::collections::HashMap<String, String>>,
}

/// Expand mise/aqua-style placeholders in an asset pattern (no replacements).
pub fn expand_asset_pattern(pattern: &str, host: &HostPlatform, version: Option<&str>) -> String {
    expand_asset_template(
        pattern,
        &TemplateContext {
            host,
            version,
            format: None,
            replacements: None,
        },
    )
}

/// Expand aqua-style asset templates including replacements and `{{.Format}}`.
pub fn expand_asset_template(pattern: &str, ctx: &TemplateContext<'_>) -> String {
    let mut os = match ctx.host.os.as_str() {
        "macos" => "darwin".to_string(),
        other => other.to_string(),
    };
    let mut arch = match ctx.host.arch.as_str() {
        "x64" => "amd64".to_string(),
        "arm64" => "arm64".to_string(),
        "x86" => "386".to_string(),
        other => other.to_string(),
    };
    if let Some(reps) = ctx.replacements {
        if let Some(v) = reps.get(&os) {
            os = v.clone();
        }
        if let Some(v) = reps.get(&arch) {
            arch = v.clone();
        }
    }
    let ver = ctx.version.unwrap_or("*");
    let ver_trim = ver.trim_start_matches('v');
    let format = ctx.format.unwrap_or("*");
    pattern
        .replace("{{trimV .Version}}", ver_trim)
        .replace("{{.Version}}", ver)
        .replace("{{version}}", ver_trim)
        .replace("{{.OS}}", &os)
        .replace("{{os}}", &os)
        .replace("{{.Arch}}", &arch)
        .replace("{{arch}}", &arch)
        .replace("{{.Format}}", format)
        .replace("{{format}}", format)
}

fn glob_match(pattern: &str, text: &str) -> bool {
    glob_match_rec(pattern.as_bytes(), text.as_bytes())
}

fn glob_match_rec(pattern: &[u8], text: &[u8]) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    let mut star_p: Option<usize> = None;
    let mut star_t: usize = 0;
    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == b'?' || pattern[pi] == text[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            star_p = Some(pi);
            star_t = ti;
            pi += 1;
        } else if let Some(sp) = star_p {
            pi = sp + 1;
            star_t += 1;
            ti = star_t;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }
    pi == pattern.len()
}

fn apply_matching_filter<'a>(
    candidates: &'a [AssetCandidate],
    options: &PickOptions,
) -> Result<Vec<&'a AssetCandidate>, AssetError> {
    let regex = options
        .matching_regex
        .as_deref()
        .map(|pat| Regex::new(pat).map_err(|e| AssetError::InvalidRegex(e.to_string())))
        .transpose()?;

    Ok(candidates
        .iter()
        .filter(|c| {
            if let Some(m) = options.matching.as_deref()
                && !c.name.contains(m)
            {
                return false;
            }
            if let Some(re) = &regex
                && !re.is_match(&c.name)
            {
                return false;
            }
            true
        })
        .collect())
}

fn platform_part<'a>(asset: &'a str, preferred_name: Option<&str>) -> &'a str {
    let Some(preferred_name) = preferred_name else {
        return asset;
    };
    let Some(prefix) = asset.get(..preferred_name.len()) else {
        return asset;
    };
    if !prefix.eq_ignore_ascii_case(preferred_name) {
        return asset;
    }
    let rest = &asset[preferred_name.len()..];
    match rest.chars().next() {
        None => rest,
        Some(c) if c == '-' || c == '_' || c == '.' || c.is_ascii_digit() => rest,
        _ => asset,
    }
}

fn score_asset(asset: &str, host: &HostPlatform, preferred_name: Option<&str>) -> i32 {
    let mut score = 0;
    score += score_os_match(asset, host, preferred_name);
    score += score_arch_match(asset, host, preferred_name);
    if host.os == "linux" || host.os == "windows" {
        score += score_libc_match(asset, host, preferred_name);
    }
    score += score_format_preferences(asset, host);
    score += score_preferred_name_match(asset, preferred_name);
    score += score_build_penalties(asset, host);
    score
}

fn score_os_match(asset: &str, host: &HostPlatform, preferred_name: Option<&str>) -> i32 {
    let asset = platform_part(asset, preferred_name);
    let mut mismatch = false;
    for (os, pattern) in &patterns().os {
        if pattern.is_match(asset) {
            if os.matches_target(&host.os) {
                return 100;
            }
            mismatch = true;
        }
    }
    if mismatch {
        return -100;
    }
    let lower = asset.to_lowercase();
    if (lower.ends_with(".msi") || lower.ends_with(".exe")) && host.os != "windows" {
        return -100;
    }
    0
}

fn score_arch_match(asset: &str, host: &HostPlatform, preferred_name: Option<&str>) -> i32 {
    let asset = platform_part(asset, preferred_name);
    for (arch, pattern) in &patterns().arch {
        if pattern.is_match(asset) {
            return if arch.matches_target(&host.arch) {
                50
            } else if *arch == AssetArch::X86 && AssetArch::X64.matches_target(&host.arch) {
                5
            } else {
                -150
            };
        }
    }
    0
}

fn has_arch_mismatch(asset: &str, host: &HostPlatform, preferred_name: Option<&str>) -> bool {
    score_arch_match(asset, host, preferred_name) < 0
}

fn score_libc_match(asset: &str, host: &HostPlatform, preferred_name: Option<&str>) -> i32 {
    let Some(target_libc) = host.libc.as_deref() else {
        return 0;
    };
    let asset = platform_part(asset, preferred_name);
    for (libc, pattern) in &patterns().libc {
        if pattern.is_match(asset) {
            return if libc.matches_target(target_libc) {
                25
            } else {
                -10
            };
        }
    }
    0
}

fn score_format_preferences(asset: &str, host: &HostPlatform) -> i32 {
    let lower = asset.to_lowercase();
    if lower.ends_with(".zip") {
        return if host.os == "windows" { 15 } else { 5 };
    }
    if is_archive(&lower) {
        return 10;
    }
    if lower.ends_with(".phar") || lower.ends_with(".jar") || lower.ends_with(".pyz") {
        return 10;
    }
    0
}

fn is_archive(lower: &str) -> bool {
    lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || lower.ends_with(".tar.xz")
        || lower.ends_with(".txz")
        || lower.ends_with(".tar.bz2")
        || lower.ends_with(".tbz2")
        || lower.ends_with(".tar.zst")
        || lower.ends_with(".tzst")
        || lower.ends_with(".tar")
        || lower.ends_with(".gz")
        || lower.ends_with(".xz")
        || lower.ends_with(".bz2")
        || lower.ends_with(".7z")
}

fn score_build_penalties(asset: &str, host: &HostPlatform) -> i32 {
    let mut penalty = 0;
    let asset = asset.to_lowercase();
    if asset.contains("debug") || asset.contains("test") {
        penalty -= 20;
    }
    if asset.ends_with(".artifactbundle") || asset.contains(".artifactbundle.") {
        penalty -= 30;
    }
    if asset.contains(".app.") && host.os != "macos" {
        penalty -= 100;
    }
    if asset.ends_with(".vsix") {
        penalty -= 100;
    }
    if asset.ends_with(".asc")
        || asset.ends_with(".sig")
        || asset.ends_with(".sign")
        || asset.ends_with(".sha256")
        || asset.ends_with(".sha512")
        || asset.ends_with(".sha1")
        || asset.ends_with(".md5")
        || asset.ends_with(".json")
        || asset.ends_with(".txt")
        || asset.ends_with(".xml")
        || asset.ends_with(".sbom")
        || asset.ends_with(".spdx")
        || asset.ends_with(".intoto")
        || asset.ends_with(".attestation")
        || asset.ends_with(".pem")
        || asset.ends_with(".cert")
        || asset.ends_with(".cer")
        || asset.ends_with(".crt")
        || asset.ends_with(".key")
        || asset.ends_with(".pub")
        || asset.ends_with(".manifest")
    {
        penalty -= 100;
    }
    if asset.contains("release-info") || asset.contains("changelog") {
        penalty -= 50;
    }
    penalty
}

fn score_preferred_name_match(asset: &str, preferred_name: Option<&str>) -> i32 {
    let Some(preferred_name) = preferred_name else {
        return 0;
    };
    if asset_matches_preferred_name(asset, preferred_name) {
        20
    } else {
        0
    }
}

fn asset_matches_preferred_name(asset: &str, preferred_name: &str) -> bool {
    let stem = asset_name_stem(asset);
    let preferred = preferred_name
        .rsplit('/')
        .next()
        .unwrap_or(preferred_name)
        .to_lowercase();
    if stem == preferred {
        return true;
    }
    stem.starts_with(&format!("{preferred}-"))
        || stem.starts_with(&format!("{preferred}_"))
        || stem.starts_with(&format!("{preferred}."))
}

fn asset_name_stem(asset: &str) -> String {
    let mut name = asset.to_lowercase();
    for ext in [
        ".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst", ".tgz", ".txz", ".tbz2", ".tzst", ".tar",
        ".zip", ".gz", ".xz", ".bz2", ".7z", ".exe",
    ] {
        if let Some(stripped) = name.strip_suffix(ext) {
            name = stripped.to_string();
            break;
        }
    }
    name
}

fn is_package_or_installer_asset(asset: &str) -> bool {
    let asset = asset.to_lowercase();
    asset.split('.').any(|extension| {
        matches!(
            extension,
            "apk"
                | "appx"
                | "appxbundle"
                | "deb"
                | "dmg"
                | "mpkg"
                | "msi"
                | "msix"
                | "msixbundle"
                | "pkg"
                | "rpm"
        )
    })
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

    fn candidates(names: &[&str]) -> Vec<AssetCandidate> {
        names
            .iter()
            .map(|n| AssetCandidate {
                name: (*n).into(),
                size: None,
                download_url: None,
            })
            .collect()
    }

    #[test]
    fn picks_linux_musl_ripgrep() {
        let assets = candidates(&[
            "ripgrep-14.1.1-aarch64-apple-darwin.tar.gz",
            "ripgrep-14.1.1-x86_64-apple-darwin.tar.gz",
            "ripgrep-14.1.1-x86_64-pc-windows-msvc.zip",
            "ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz",
            "ripgrep-14.1.1-arm-unknown-linux-gnueabihf.tar.gz",
        ]);
        let picked = pick_asset(
            &assets,
            &linux_x64(),
            &PickOptions {
                preferred_name: Some("ripgrep".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            picked.name,
            "ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz"
        );
    }

    #[test]
    fn picks_macos_arm64() {
        let assets = candidates(&[
            "tool-linux-amd64.tar.gz",
            "tool-darwin-arm64.tar.gz",
            "tool-windows-amd64.zip",
        ]);
        let picked = pick_asset(&assets, &macos_arm64(), &PickOptions::default()).unwrap();
        assert_eq!(picked.name, "tool-darwin-arm64.tar.gz");
    }

    #[test]
    fn rejects_wrong_arch() {
        let assets = candidates(&["tool-linux-arm64.tar.gz", "tool-linux-amd64.deb"]);
        let err = pick_asset(&assets, &linux_x64(), &PickOptions::default()).unwrap_err();
        assert!(matches!(err, AssetError::NoMatch));
    }

    #[test]
    fn skips_deb_rpm_installers() {
        let assets = candidates(&[
            "quickhook-1.6.2-linux-amd64.deb",
            "quickhook-1.6.2-linux-amd64.rpm",
            "quickhook-1.6.2-linux-amd64.tar.gz",
        ]);
        let picked = pick_asset(&assets, &linux_x64(), &PickOptions::default()).unwrap();
        assert_eq!(picked.name, "quickhook-1.6.2-linux-amd64.tar.gz");
    }

    #[test]
    fn matching_narrows_candidates() {
        let assets = candidates(&[
            "oxlint-x86_64-unknown-linux-gnu.tar.gz",
            "oxfmt-x86_64-unknown-linux-gnu.tar.gz",
        ]);
        let picked = pick_asset(
            &assets,
            &linux_x64(),
            &PickOptions {
                matching: Some("oxlint".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(picked.name, "oxlint-x86_64-unknown-linux-gnu.tar.gz");
    }

    #[test]
    fn prefers_shortest_name_on_tie() {
        let assets = candidates(&[
            "tool-lsp-linux-x64.tar.gz",
            "tool-linux-x64.tar.gz",
            "tool-mcp-linux-x64.tar.gz",
        ]);
        let picked = pick_asset(&assets, &linux_x64(), &PickOptions::default()).unwrap();
        assert_eq!(picked.name, "tool-linux-x64.tar.gz");
    }

    #[test]
    fn asset_pattern_skips_scoring() {
        let assets = candidates(&[
            "app-linux-amd64.tar.gz",
            "app-custom-weird-name.zip",
            "app-darwin-arm64.tar.gz",
        ]);
        let picked = pick_asset(
            &assets,
            &linux_x64(),
            &PickOptions {
                asset_pattern: Some("app-custom-*.zip".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(picked.name, "app-custom-weird-name.zip");
    }

    #[test]
    fn asset_pattern_expands_placeholders() {
        let expanded = expand_asset_pattern(
            "gh_{{.Version}}_linux_{{.Arch}}.tar.gz",
            &linux_x64(),
            Some("2.67.0"),
        );
        assert_eq!(expanded, "gh_2.67.0_linux_amd64.tar.gz");
    }

    #[test]
    fn aqua_template_applies_replacements_and_trimv() {
        let mut reps = std::collections::HashMap::new();
        reps.insert("darwin".into(), "macOS".into());
        reps.insert("amd64".into(), "x86_64".into());
        reps.insert("linux".into(), "unknown-linux-musl".into());
        let expanded = expand_asset_template(
            "gh_{{trimV .Version}}_{{.OS}}_{{.Arch}}.{{.Format}}",
            &TemplateContext {
                host: &linux_x64(),
                version: Some("v2.67.0"),
                format: Some("tar.gz"),
                replacements: Some(&reps),
            },
        );
        assert_eq!(expanded, "gh_2.67.0_unknown-linux-musl_x86_64.tar.gz");
    }

    #[test]
    fn pixi_platform_mapping() {
        assert_eq!(linux_x64().pixi_platform(), "linux-64");
        assert_eq!(macos_arm64().pixi_platform(), "osx-arm64");
    }
}
