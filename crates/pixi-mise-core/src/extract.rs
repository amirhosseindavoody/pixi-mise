//! Download archive extraction helpers.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use tar::Archive;
use zip::ZipArchive;

use crate::CoreError;

/// Extract a downloaded asset into `dest_dir`, returning the extraction root.
pub fn extract_asset(archive_path: &Path, dest_dir: &Path) -> Result<(), CoreError> {
    fs::create_dir_all(dest_dir).map_err(|e| CoreError::Install(e.to_string()))?;
    let name = archive_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase();

    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let file = File::open(archive_path).map_err(|e| CoreError::Install(e.to_string()))?;
        let decoder = GzDecoder::new(file);
        let mut archive = Archive::new(decoder);
        archive
            .unpack(dest_dir)
            .map_err(|e| CoreError::Install(format!("tar.gz extract: {e}")))?;
        return Ok(());
    }

    if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        let file = File::open(archive_path).map_err(|e| CoreError::Install(e.to_string()))?;
        let decoder = xz2::read::XzDecoder::new(file);
        let mut archive = Archive::new(decoder);
        archive
            .unpack(dest_dir)
            .map_err(|e| CoreError::Install(format!("tar.xz extract: {e}")))?;
        return Ok(());
    }

    if name.ends_with(".tar.bz2") || name.ends_with(".tbz2") || name.ends_with(".tbz") {
        let file = File::open(archive_path).map_err(|e| CoreError::Install(e.to_string()))?;
        let decoder = bzip2::read::BzDecoder::new(file);
        let mut archive = Archive::new(decoder);
        archive
            .unpack(dest_dir)
            .map_err(|e| CoreError::Install(format!("tar.bz2 extract: {e}")))?;
        return Ok(());
    }

    if name.ends_with(".tar") {
        let file = File::open(archive_path).map_err(|e| CoreError::Install(e.to_string()))?;
        let mut archive = Archive::new(file);
        archive
            .unpack(dest_dir)
            .map_err(|e| CoreError::Install(format!("tar extract: {e}")))?;
        return Ok(());
    }

    if name.ends_with(".zip") {
        let file = File::open(archive_path).map_err(|e| CoreError::Install(e.to_string()))?;
        let mut archive =
            ZipArchive::new(file).map_err(|e| CoreError::Install(format!("zip open: {e}")))?;
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| CoreError::Install(format!("zip entry: {e}")))?;
            let out_path = match entry.enclosed_name() {
                Some(p) => dest_dir.join(p),
                None => continue,
            };
            if entry.name().ends_with('/') {
                fs::create_dir_all(&out_path).map_err(|e| CoreError::Install(e.to_string()))?;
            } else {
                if let Some(parent) = out_path.parent() {
                    fs::create_dir_all(parent).map_err(|e| CoreError::Install(e.to_string()))?;
                }
                let mut outfile =
                    File::create(&out_path).map_err(|e| CoreError::Install(e.to_string()))?;
                io::copy(&mut entry, &mut outfile)
                    .map_err(|e| CoreError::Install(e.to_string()))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Some(mode) = entry.unix_mode() {
                        fs::set_permissions(&out_path, fs::Permissions::from_mode(mode))
                            .map_err(|e| CoreError::Install(e.to_string()))?;
                    }
                }
            }
        }
        return Ok(());
    }

    // Bare binary / non-archive: copy as-is into dest.
    let dest = dest_dir.join(
        archive_path
            .file_name()
            .ok_or_else(|| CoreError::Install("archive has no filename".into()))?,
    );
    fs::copy(archive_path, &dest).map_err(|e| CoreError::Install(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&dest)
            .map_err(|e| CoreError::Install(e.to_string()))?
            .permissions();
        perms.set_mode(perms.mode() | 0o755);
        fs::set_permissions(&dest, perms).map_err(|e| CoreError::Install(e.to_string()))?;
    }
    Ok(())
}

/// Find likely executable binaries under `root`.
///
/// `repo_hint` is the GitHub repo name (e.g. `ripgrep`, `cli`) used to prefer
/// a matching binary when no explicit `bin` / `rename_exe` is set.
pub fn find_binaries(
    root: &Path,
    preferred: Option<&str>,
    bin_path: Option<&str>,
    repo_hint: Option<&str>,
) -> Result<Vec<PathBuf>, CoreError> {
    let search_root = if let Some(bp) = bin_path {
        root.join(bp)
    } else {
        let nested_bin = find_named_dir(root, "bin")?;
        nested_bin.unwrap_or_else(|| root.to_path_buf())
    };

    if !search_root.exists() {
        return Err(CoreError::Install(format!(
            "binary search path does not exist: {}",
            search_root.display()
        )));
    }

    let mut binaries = Vec::new();
    collect_binaries(&search_root, &mut binaries)?;

    if binaries.is_empty() && search_root != root {
        collect_binaries(root, &mut binaries)?;
    }

    if binaries.is_empty() {
        return Err(CoreError::Install(format!(
            "no executable binaries found under {}",
            root.display()
        )));
    }

    if let Some(preferred) = preferred {
        let matches = filter_by_name(&binaries, preferred);
        if !matches.is_empty() {
            return Ok(matches);
        }
    }

    // Prefer a binary named after the repo (e.g. `gh` is harder; `ripgrep` rarely
    // matches — common short names are handled below via ELF preference).
    if let Some(hint) = repo_hint {
        let matches = filter_by_name(&binaries, hint);
        if matches.len() == 1 {
            return Ok(matches);
        }
        // Well-known short names for popular tools.
        for alias in known_bin_aliases(hint) {
            let matches = filter_by_name(&binaries, alias);
            if !matches.is_empty() {
                return Ok(matches);
            }
        }
    }

    // Prefer files directly under a `bin/` directory.
    let bin_dir_bins: Vec<_> = binaries
        .iter()
        .filter(|p| {
            p.parent()
                .and_then(|parent| parent.file_name())
                .is_some_and(|n| n == "bin")
        })
        .cloned()
        .collect();
    if !bin_dir_bins.is_empty() {
        return Ok(bin_dir_bins);
    }

    // Prefer real native executables (ELF / Mach-O / PE) over scripts when
    // choosing a single binary from a flat archive.
    let mut native: Vec<_> = binaries
        .iter()
        .filter(|p| matches!(file_kind(p), Ok(FileKind::NativeExe)))
        .cloned()
        .collect();
    if !native.is_empty() {
        native.sort_by(|a, b| {
            a.components()
                .count()
                .cmp(&b.components().count())
                .then_with(|| {
                    a.file_name()
                        .map(|n| n.len())
                        .cmp(&b.file_name().map(|n| n.len()))
                })
                .then_with(|| a.cmp(b))
        });
        return Ok(vec![native.remove(0)]);
    }

    binaries.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });
    Ok(vec![binaries.remove(0)])
}

fn filter_by_name(binaries: &[PathBuf], name: &str) -> Vec<PathBuf> {
    let want = name.to_lowercase();
    binaries
        .iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.trim_end_matches(".exe").eq_ignore_ascii_case(&want))
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

fn known_bin_aliases(repo: &str) -> &'static [&'static str] {
    match repo {
        "ripgrep" => &["rg"],
        "cli" => &["gh"],
        "fd" | "fd-find" => &["fd"],
        "bat" => &["bat"],
        "exa" => &["exa"],
        "eza" => &["eza"],
        "bottom" => &["btm"],
        "procs" => &["procs"],
        "sd" => &["sd"],
        "xsv" => &["xsv"],
        "hyperfine" => &["hyperfine"],
        "jq" => &["jq"],
        "yq" => &["yq"],
        _ => &[],
    }
}

fn find_named_dir(root: &Path, name: &str) -> Result<Option<PathBuf>, CoreError> {
    if root.join(name).is_dir() {
        return Ok(Some(root.join(name)));
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| CoreError::Install(e.to_string()))?;
        for entry in entries {
            let entry = entry.map_err(|e| CoreError::Install(e.to_string()))?;
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == name) {
                    return Ok(Some(path));
                }
                if path.parent() == Some(root) {
                    stack.push(path);
                }
            }
        }
    }
    Ok(None)
}

fn collect_binaries(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), CoreError> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = match fs::read_dir(&current) {
            Ok(e) => e,
            Err(e) => return Err(CoreError::Install(e.to_string())),
        };
        for entry in entries {
            let entry = entry.map_err(|e| CoreError::Install(e.to_string()))?;
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if matches!(
                    name,
                    "share"
                        | "docs"
                        | "doc"
                        | "man"
                        | "completions"
                        | "autocomplete"
                        | "complete"
                        | ".git"
                ) {
                    continue;
                }
                stack.push(path);
            } else if is_likely_binary(&path)? {
                out.push(path);
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    NativeExe,
    Script,
    Other,
}

fn file_kind(path: &Path) -> Result<FileKind, CoreError> {
    let mut file = File::open(path).map_err(|e| CoreError::Install(e.to_string()))?;
    let mut magic = [0u8; 4];
    let n = file.read(&mut magic).unwrap_or(0);
    if n >= 4 && magic == [0x7f, b'E', b'L', b'F'] {
        return Ok(FileKind::NativeExe);
    }
    // Mach-O thin / fat
    if n >= 4
        && matches!(
            u32::from_be_bytes(magic),
            0xfeedface | 0xfeedfacf | 0xcafebabe | 0xcffaedfe | 0xcefaedfe
        )
    {
        return Ok(FileKind::NativeExe);
    }
    // PE / MZ
    if n >= 2 && magic[0] == b'M' && magic[1] == b'Z' {
        return Ok(FileKind::NativeExe);
    }
    if n >= 2 && magic[0] == b'#' && magic[1] == b'!' {
        return Ok(FileKind::Script);
    }
    Ok(FileKind::Other)
}

fn is_likely_binary(path: &Path) -> Result<bool, CoreError> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    if name.starts_with("license")
        || name.starts_with("copying")
        || name.starts_with("changelog")
        || name.starts_with("readme")
        || name.starts_with("notice")
        || name.starts_with("authors")
        || name.starts_with("credits")
        || name == "unlicense"
        || name.ends_with(".md")
        || name.ends_with(".txt")
        || name.ends_with(".1")
        || name.ends_with(".fish")
        || name.ends_with(".zsh")
        || name.ends_with(".bash")
        || name.ends_with(".ps1")
        || name.ends_with(".bat")
        || name.ends_with(".cmd")
        || name.ends_with(".json")
        || name.ends_with(".yml")
        || name.ends_with(".yaml")
        || name.ends_with(".toml")
        || name.ends_with(".sbom")
        || name.ends_with(".pdf")
        || name.ends_with(".html")
    {
        return Ok(false);
    }

    match file_kind(path)? {
        FileKind::NativeExe | FileKind::Script => return Ok(true),
        FileKind::Other => {}
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(path).map_err(|e| CoreError::Install(e.to_string()))?;
        if meta.permissions().mode() & 0o111 != 0 && !name.contains('.') {
            return Ok(true);
        }
    }

    #[cfg(not(unix))]
    {
        if name.ends_with(".exe") {
            return Ok(true);
        }
    }

    Ok(false)
}
