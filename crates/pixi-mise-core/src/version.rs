//! Lightweight semver-ish parsing and comparison for version specs.

use crate::VersionSpec;

/// Parsed numeric version components (best-effort).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedVersion {
    /// Numeric segments (`1.2.3` → `[1,2,3]`).
    pub parts: Vec<u64>,
    /// Optional prerelease suffix (`1.0.0-rc.1` → `rc.1`).
    pub pre: Option<String>,
    /// Original tag (for display / exact match).
    pub original: String,
}

impl PartialOrd for ParsedVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ParsedVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let len = self.parts.len().max(other.parts.len());
        for i in 0..len {
            let a = self.parts.get(i).copied().unwrap_or(0);
            let b = other.parts.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                std::cmp::Ordering::Equal => {}
                other => return other,
            }
        }
        // Release > prerelease when numeric parts equal.
        match (&self.pre, &other.pre) {
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(a), Some(b)) => a.cmp(b),
            (None, None) => std::cmp::Ordering::Equal,
        }
    }
}

/// Parse a GitHub tag / version string into numeric parts.
pub fn parse_version(raw: &str) -> ParsedVersion {
    let original = raw.to_string();
    let trimmed = raw.trim().trim_start_matches('v');
    let (num, pre) = match trimmed.split_once('-') {
        Some((n, p)) => (n, Some(p.to_string())),
        None => match trimmed.split_once('+') {
            Some((n, _)) => (n, None),
            None => (trimmed, None),
        },
    };
    let parts: Vec<u64> = num
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    ParsedVersion {
        parts,
        pre,
        original,
    }
}

/// Normalize a tag for exact comparison (strip leading `v`).
pub fn normalize_tag(tag: &str) -> String {
    tag.trim().trim_start_matches('v').to_string()
}

/// Whether `tag` satisfies `spec` (after optional `version_prefix` strip).
pub fn tag_matches_spec(tag: &str, spec: &VersionSpec, version_prefix: Option<&str>) -> bool {
    let mut tag = tag.to_string();
    if let Some(prefix) = version_prefix
        && let Some(stripped) = tag.strip_prefix(prefix)
    {
        tag = stripped.to_string();
    }
    let norm = normalize_tag(&tag);
    match spec {
        VersionSpec::Latest => true,
        VersionSpec::Exact(exact) => {
            normalize_tag(exact) == norm || tag == *exact || format!("v{norm}") == *exact
        }
        VersionSpec::Prefix(prefix) => {
            let p = normalize_tag(prefix);
            norm == p
                || norm.starts_with(&format!("{p}."))
                || norm.starts_with(&format!("{p}-"))
                || norm.starts_with(&format!("{p}_"))
        }
        VersionSpec::Caret(req) => matches_caret(&norm, req),
        VersionSpec::Tilde(req) => matches_tilde(&norm, req),
    }
}

fn matches_caret(norm_tag: &str, req: &str) -> bool {
    let want = parse_version(&normalize_tag(req));
    let got = parse_version(norm_tag);
    if got.parts.is_empty() || want.parts.is_empty() {
        return false;
    }
    if got < want {
        return false;
    }
    // ^1.2.3 → >=1.2.3 <2.0.0 ; ^0.2.3 → >=0.2.3 <0.3.0 ; ^0.0.3 → >=0.0.3 <0.0.4
    match want.parts.as_slice() {
        [0, 0, patch, ..] => {
            got.parts.first() == Some(&0)
                && got.parts.get(1) == Some(&0)
                && got.parts.get(2).copied().unwrap_or(0) == *patch
                && got.pre.is_none()
        }
        [0, minor, ..] => {
            got.parts.first() == Some(&0) && got.parts.get(1).copied().unwrap_or(0) == *minor
        }
        [major, ..] => got.parts.first().copied().unwrap_or(0) == *major,
        [] => false,
    }
}

fn matches_tilde(norm_tag: &str, req: &str) -> bool {
    let want = parse_version(&normalize_tag(req));
    let got = parse_version(norm_tag);
    if got.parts.is_empty() || want.parts.is_empty() || got < want {
        return false;
    }
    // ~1.2.3 → >=1.2.3 <1.3.0 ; ~1.2 → >=1.2.0 <1.3.0 ; ~1 → >=1.0.0 <2.0.0
    match want.parts.len() {
        1 => got.parts.first() == want.parts.first(),
        _ => got.parts.first() == want.parts.first() && got.parts.get(1) == want.parts.get(1),
    }
}

/// Pick the highest semver tag among candidates that match `spec`.
pub fn select_best_tag<'a>(
    tags: impl IntoIterator<Item = &'a str>,
    spec: &VersionSpec,
    version_prefix: Option<&str>,
    allow_prerelease: bool,
) -> Option<&'a str> {
    let mut best: Option<(&'a str, ParsedVersion)> = None;
    for tag in tags {
        let mut display = tag.to_string();
        if let Some(prefix) = version_prefix
            && let Some(stripped) = display.strip_prefix(prefix)
        {
            display = stripped.to_string();
        }
        let parsed = parse_version(&display);
        if !allow_prerelease && parsed.pre.is_some() {
            // Also skip if the raw tag looks like a GitHub prerelease handled elsewhere;
            // here we only filter hyphen prerelease segments.
            continue;
        }
        if !tag_matches_spec(tag, spec, version_prefix) {
            continue;
        }
        match &best {
            None => best = Some((tag, parsed)),
            Some((_, best_parsed)) if &parsed > best_parsed => best = Some((tag, parsed)),
            _ => {}
        }
    }
    best.map(|(t, _)| t)
}

/// Parse CLI/config version strings including `^` / `~` / `latest`.
pub fn parse_version_spec(raw: &str) -> VersionSpec {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("latest") {
        return VersionSpec::Latest;
    }
    if let Some(rest) = raw.strip_prefix('^') {
        return VersionSpec::Caret(rest.to_string());
    }
    if let Some(rest) = raw.strip_prefix('~') {
        return VersionSpec::Tilde(rest.to_string());
    }
    let stripped = raw.trim_start_matches('v');
    // Pure digits, or digits with a single trailing component like `14` → Prefix.
    // `14.1` and `14.1.1` → Exact (full version pin).
    if !stripped.is_empty() && stripped.chars().all(|c| c.is_ascii_digit()) {
        VersionSpec::Prefix(stripped.to_string())
    } else if stripped.contains('.')
        && stripped
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        && stripped.matches('.').count() == 1
        && stripped.split('.').nth(1).is_some_and(|p| p.len() <= 2)
    {
        // Ambiguous short forms like `1.2` — treat as Prefix so update can float patch.
        VersionSpec::Prefix(stripped.to_string())
    } else {
        VersionSpec::Exact(raw.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_semver() {
        assert!(parse_version("1.2.3") > parse_version("1.2.2"));
        assert!(parse_version("2.0.0") > parse_version("1.9.9"));
        assert!(parse_version("1.0.0") > parse_version("1.0.0-rc.1"));
        assert!(parse_version("v14.1.1") > parse_version("14.0.0"));
    }

    #[test]
    fn prefix_and_caret() {
        assert!(tag_matches_spec(
            "14.1.1",
            &VersionSpec::Prefix("14".into()),
            None
        ));
        assert!(!tag_matches_spec(
            "140.0.0",
            &VersionSpec::Prefix("14".into()),
            None
        ));
        assert!(tag_matches_spec(
            "1.3.0",
            &VersionSpec::Caret("1.2.3".into()),
            None
        ));
        assert!(!tag_matches_spec(
            "2.0.0",
            &VersionSpec::Caret("1.2.3".into()),
            None
        ));
        assert!(tag_matches_spec(
            "1.2.9",
            &VersionSpec::Tilde("1.2.3".into()),
            None
        ));
        assert!(!tag_matches_spec(
            "1.3.0",
            &VersionSpec::Tilde("1.2.3".into()),
            None
        ));
    }

    #[test]
    fn select_highest_prefix() {
        let tags = ["15.0.0", "14.1.1", "14.0.0", "13.0.0"];
        let best = select_best_tag(tags, &VersionSpec::Prefix("14".into()), None, false).unwrap();
        assert_eq!(best, "14.1.1");
    }

    #[test]
    fn parse_specs() {
        assert_eq!(parse_version_spec("latest"), VersionSpec::Latest);
        assert_eq!(parse_version_spec("14"), VersionSpec::Prefix("14".into()));
        assert_eq!(
            parse_version_spec("^1.2.3"),
            VersionSpec::Caret("1.2.3".into())
        );
        assert_eq!(
            parse_version_spec("~1.2.3"),
            VersionSpec::Tilde("1.2.3".into())
        );
        assert_eq!(
            parse_version_spec("14.1.1"),
            VersionSpec::Exact("14.1.1".into())
        );
    }
}
