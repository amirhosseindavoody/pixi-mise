# pixi-mise Design

Install GitHub release binaries into Pixi-managed global or local environments, with mise-like dependency discovery and platform-aware asset resolution.

## 1. Problem

Pixi installs Conda (and PyPI) packages into isolated environments. Many useful CLI tools ship only as GitHub release binaries and are not on conda-forge. Users today fall back to ad-hoc scripts, Homebrew, or tools like [mise](https://mise.jdx.dev/) / [aqua](https://aquaproj.github.io/) outside Pixi.

`pixi-mise` fills that gap as a **Pixi extension**: declare GitHub tools next to the rest of a Pixi workspace, resolve the correct OS/arch asset, and install the binary into a Pixi environment so it appears on `PATH` via normal Pixi activation / global exposure.

## 2. Goals and Non-Goals

### Goals

- Discover tool specs from project / global config (mise-inspired).
- Resolve `github:owner/repo@version` to a concrete release + platform asset.
- Score and select release assets for the host OS/arch (mise `AssetPicker`-style).
- Install extracted binaries into Pixi **local** or **global** environments.
- Ship as a Rust binary named `pixi-mise` so Pixi discovers it as `pixi mise`.
- Provide a lockfile for reproducible installs across machines/CI.
- Support common overrides: `matching`, `asset_pattern`, `bin` / `rename_exe`, checksums.

### Non-Goals (v1)

- Full mise feature parity (tasks, env var management, asdf/vfox plugins, language build backends).
- Compiling tools from source (Cargo/Go/npm backends).
- Replacing Conda packages that already exist on conda-forge.
- Publishing tools as Conda packages automatically (may be a later optional path).
- Shell hook / directory auto-switch (Pixi already owns env activation).

## 3. Prior Art

### 3.1 Mise resolution pipeline

Mise’s tool flow ([architecture](https://mise.jdx.dev/architecture.html), [dev tools](https://mise.jdx.dev/dev-tools/)):

1. **Configuration discovery** — walk up the directory tree; merge `mise.toml` / `.tool-versions` hierarchically.
2. **Tool resolution** — turn requests (`node@latest`, `github:cli/cli@2`) into concrete `ToolVersion`s.
3. **Backend selection** — core, aqua, github, http, …
4. **Dependency analysis** — install-order DAG (`depends`).
5. **Installation** — download, verify, extract; place under a versioned install path.
6. **Environment setup** — prepend install `bin` dirs to `PATH`.

Core types (mise mental model we adopt):

| Type | Role |
|------|------|
| `ToolRequest` | User spec (`github:BurntSushi/ripgrep@14`) |
| `ToolVersion` | Fully resolved version + backend metadata |
| `Toolset` | Immutable set of resolved tools for a context |
| `Backend` | Fetch/list/install implementation |

For GitHub releases specifically ([GitHub backend](https://mise.jdx.dev/dev-tools/backends/github.html)):

- List releases via GitHub API.
- Autodetect the best asset when no `asset_pattern` is set.
- Narrow candidates with `matching` / `matching_regex` while keeping platform autodetection.
- Score assets in `asset_matcher.rs` (shared by GitHub / GitLab / Forgejo).

### 3.2 Mise binary → OS resolution (AssetPicker)

Scoring dimensions (positive / negative weights):

| Signal | Typical weight | Notes |
|--------|----------------|-------|
| OS match (`linux`, `darwin`/`macos`, `windows`, …) | +100 / −100 | Wrong OS is strongly rejected |
| Arch match (`x64`/`amd64`/`x86_64`, `arm64`/`aarch64`, …) | +50 / −150 | Arch mismatch is disqualifying |
| Libc (`gnu` / `musl` / `msvc`) | +25 / −10 | Linux/Windows |
| Archive format | +10–15 | Prefer archives; zip favored on Windows |
| Preferred tool name | bonus | Prefer shortest name on ties |
| Debug/test/metadata/installers | penalties | Skip `.deb`/`.rpm`/`.msi`/checksums/sigs by default |

Selection rules:

1. Optional pre-filter: `matching` (substring) and/or `matching_regex`.
2. Score remaining assets; keep score > 0; reject arch mismatches and package/installer assets.
3. Highest score wins; ties broken by shortest filename, then lexicographic order.

Overrides users need (port from mise):

- `asset_pattern` — replaces autodetection (platform-specific patterns allowed).
- `matching` / `matching_regex` — refine multi-binary releases (e.g. `oxlint` vs `oxfmt`).
- `bin` / `rename_exe` — normalize binary names.
- `strip_components`, `bin_path`, `checksum`, `version_prefix`, `prerelease`.

### 3.3 Aqua registry (optional later)

Mise’s aqua backend reimplements aqua’s registry: templated assets (`{{.Version}}`, `{{.OS}}`, `{{.Arch}}`), `replacements`, `overrides`, `supported_envs`, `files`. That gives curated install recipes for thousands of tools. v1 can ship GitHub autodetection only; aqua registry consumption is a natural v2.

### 3.4 Pixi extensions

From [Pixi Extensions](https://pixi.prefix.dev/latest/integration/extensions/introduction/):

- Executable must be named `pixi-{command}`.
- Discovery order: `PATH`, then `pixi global` directories.
- Invoked as `pixi {command} …`; remaining args are forwarded.
- Recommended install: `pixi global install pixi-mise` (once published to a channel) or place the binary on `PATH`.
- Best practices: clap CLI, `--help`, UNIX exit codes, respect Pixi environments.

`pixi-mise` therefore exposes:

```text
pixi mise <subcommand> …
```

### 3.5 Pixi environments (install targets)

| Scope | Prefix | How binaries become usable |
|-------|--------|----------------------------|
| Local workspace | `.pixi/envs/<env>/` | Present under `$PREFIX/bin` → available in `pixi shell` / `pixi run` |
| Global | `$PIXI_HOME/envs/<env>/` | Binary in env `bin/` **and** exposed into `$PIXI_HOME/bin` via sidecar (see §9.2) |

v1 installs by placing (or symlinking) binaries into the target prefix’s `bin/` (and updating global exposures via `pixi mise global …`).

## 4. Product Shape

### 4.1 User experience

CLI verbs mirror Pixi itself (`add` / `remove` / `list` / `install` / `update` / `upgrade` / `lock` / `clean`, plus a `global` subcommand) rather than mise’s `use` / `ls` naming.

```bash
# Declare tools (workspace) — same verb as `pixi add`
pixi mise add github:BurntSushi/ripgrep@14

# Global tools — same shape as `pixi global add`
pixi mise global add github:cli/cli

# Install everything declared for this workspace
pixi mise install

# Inspect
pixi mise list
pixi mise search github:cli/cli   # remote versions (closest to `pixi search`)
pixi mise resolve                 # dry-run asset resolution (no Pixi equivalent)

# Remove
pixi mise remove github:BurntSushi/ripgrep
```

Typical local config (proposed `pixi.toml` table):

```toml
[workspace]
name = "my-project"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "win-64"]

[dependencies]
python = "3.12.*"

[tool.pixi-mise.tools]
"github:BurntSushi/ripgrep" = "14.1.1"
"github:cli/cli" = { version = "2.67.0", matching = "gh_" }

[tool.pixi-mise.tools."github:oxc-project/oxc"]
version = "apps_v1.69.0"
matching = "oxlint"
rename_exe = "oxlint"
```

Global config lives in a **sidecar** file `$PIXI_HOME/pixi-mise.toml` (not Pixi’s `pixi-global.toml`):

```toml
[tools]
"github:jdx/mise" = "latest"  # illustrative; any github: tool
"github:cli/cli" = { version = "latest", expose_as = "gh" }
```

`pixi global list` will not show these tools — use `pixi mise global list`. Both systems share `$PIXI_HOME/bin` on `PATH`.
### 4.2 Compatibility with `mise.toml`

Optionally read a subset of `mise.toml` `[tools]` entries that use the `github:` backend. Write path for v1 remains `[tool.pixi-mise]` in `pixi.toml` / `pixi-mise.toml` so Pixi-native projects have a clear home. Full bidirectional sync with mise is out of scope for v1.

## 5. Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│ CLI (pixi-mise)  clap: add/install/remove/list/global…      │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────┐
│ Config discovery                                            │
│  - walk parents for pixi.toml / pixi-mise.toml [/ mise.toml]│
│  - merge global config                                      │
│  - produce Vec<ToolRequest> + install target (local/global) │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────┐
│ Resolver                                                    │
│  ToolRequest → GitHub releases → ToolVersion                │
│  version specs: exact | prefix | latest | (semver later)    │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────┐
│ AssetMatcher / AssetPicker                                  │
│  filter → score OS/arch/libc/format → pick best asset       │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────┐
│ Installer                                                   │
│  download → verify checksum/size → extract → place bins     │
│  update lockfile + (global) exposure metadata               │
└────────────────────────────┬────────────────────────────────┘
                             │
                             ▼
┌─────────────────────────────────────────────────────────────┐
│ PixiEnvAdapter                                              │
│  locate prefix (.pixi/envs/… or $PIXI_HOME/envs/…)          │
│  write into $PREFIX/bin                                     │
│  for global: ensure expose entry / shim in $PIXI_HOME/bin   │
└─────────────────────────────────────────────────────────────┘
```

### 5.1 Rust crate layout

```text
pixi-mise/
├── Cargo.toml                 # workspace root + `pixi-mise` binary package
├── pixi.toml                  # Pixi workspace + conda package (pixi-build-rust)
├── crates/
│   ├── pixi-mise/src/         # CLI binary sources
│   ├── pixi-mise-core/        # config, types, resolve, install orchestration
│   ├── pixi-mise-github/      # GitHub API client + release listing
│   ├── pixi-mise-assets/      # AssetPicker scoring (mise-inspired)
│   └── pixi-mise-pixi/        # Pixi env/prefix + sidecar global expose
├── docs/DESIGN.md
└── README.md
```

Suggested crates.io / deps orientation:

- CLI: `clap`, `miette` / `thiserror`, `tracing`
- Async HTTP: `reqwest` + `tokio`
- Config: `toml`, `serde`
- Archives: `flate2`, `tar`, `zip`, `xz2` / `bzip2` as needed
- Checksums: `sha2`, `hex`
- Semver (later): `versions` or `semver`
- Platform: `std::env::consts` + small OS/arch normalization layer (map to Pixi platforms: `linux-64`, `osx-arm64`, `win-64`, …)

Keep scoring logic in `pixi-mise-assets` with unit tests ported from mise edge cases (multi-binary releases, musl vs gnu, wrong-arch rejection, installer skipping).

### 5.2 Core types

```rust
pub struct ToolRequest {
    pub backend: BackendKind,          // GitHub (v1)
    pub id: ToolId,                    // owner/repo
    pub version: VersionSpec,          // Latest | Exact | Prefix
    pub options: ToolOptions,
    pub source: ConfigSource,          // which file/table
}

pub struct ToolOptions {
    pub matching: Option<String>,
    pub matching_regex: Option<String>,
    pub asset_pattern: Option<String>,
    pub bin: Option<String>,
    pub rename_exe: Option<String>,
    pub strip_components: Option<u32>,
    pub bin_path: Option<String>,
    pub checksum: Option<Checksum>,
    pub version_prefix: Option<String>,
    pub prerelease: bool,
    pub expose_as: Option<String>,     // global binary name
    pub os_filter: Vec<OsArchConstraint>,
}

pub struct ToolVersion {
    pub request: ToolRequest,
    pub version: String,               // concrete, prefix stripped for display
    pub tag: String,                   // GitHub tag_name
    pub asset: ResolvedAsset,
}

pub struct ResolvedAsset {
    pub name: String,
    pub download_url: String,
    pub size: Option<u64>,
    pub digest: Option<Checksum>,
}

pub enum InstallTarget {
    Local { workspace_root: PathBuf, env: String },
    Global { env: String },            // default: sanitized tool name
}
```

## 6. Config Discovery

Inspired by mise’s hierarchical merge, but anchored on Pixi workspaces.

### Search order (local commands)

1. `./pixi.toml` → `[tool.pixi-mise]`
2. `./pixi-mise.toml` (optional dedicated file)
3. Parent directories until filesystem root (or until a `[workspace]` / `[project]` pixi root is found — **stop at workspace root** for tool lists; do not inherit another project’s tools by default)
4. Global: `$PIXI_HOME/pixi-mise.toml` (always merged for `pixi mise global …`; optionally merged as defaults for local if we add `inherit_global = true` later)

v1 recommendation: **tools are workspace-scoped**. Walking past the Pixi workspace root is not required for the MVP and avoids surprising inheritance.

### Environment selection

- Default local env: Pixi’s default environment (`default`).
- Override: `--environment <name>` / `PIXI_MISE_ENVIRONMENT`.
- Global: each tool gets its own isolated env under `$PIXI_HOME/envs/pixi-mise-<tool>` (mirrors `pixi global` isolation), unless `--environment` groups tools together.

## 7. Resolution Pipeline

```text
ToolRequest
  → authenticate GitHub (GITHUB_TOKEN / GH token / unauthenticated)
  → list releases (paginate; cache)
  → apply version_prefix / prerelease filters
  → select release matching VersionSpec
  → list assets
  → AssetMatcher::pick
  → ToolVersion (+ optional lockfile pin)
```

### Version specs (v1)

| Spec | Behavior |
|------|----------|
| `latest` | GitHub “latest” release (non-prerelease) unless `prerelease = true` |
| `1.2.3` / `v1.2.3` | Exact tag match with optional `v` normalization |
| `14` / `2.67` | Highest tag matching prefix (after stripping `version_prefix`) |

### Lockfile (`pixi-mise.lock`)

Record per tool + platform:

```toml
[[tools]]
id = "github:BurntSushi/ripgrep"
version = "14.1.1"
tag = "14.1.1"
asset = "ripgrep-14.1.1-x86_64-unknown-linux-musl.tar.gz"
url = "https://github.com/BurntSushi/ripgrep/releases/download/14.1.1/..."
checksum = "sha256:…"
platform = "linux-64"
installed_bins = ["rg"]
```

Install prefers lockfile URL/checksum when present and still valid; `pixi mise lock` / `install --update-lock` refreshes.

## 8. Asset Matching Design

Port the mise scoring model (not the full codebase). Public algorithm:

```text
candidates = assets
if matching:        candidates = filter contains(matching)
if matching_regex:  candidates = filter regex_match
if asset_pattern:   candidates = glob/template match; skip scoring (pattern wins)

for each candidate:
  score = OS + Arch + Libc + Format + PreferredName + Penalties
drop score <= 0, arch mismatches, package/installer assets
pick max score; tie → shortest name → lexicographic
```

Platform vocabulary (normalize both host and asset tokens):

| Concept | Host examples | Asset tokens recognized |
|---------|---------------|-------------------------|
| OS | linux, macos, windows | linux, gnu/musl host, darwin, macos, osx, apple, windows, win32, win64 |
| Arch | x64, arm64, x86, arm | x86_64, amd64, x64, aarch64, arm64, i686, armv7, … |
| Libc | gnu, musl, msvc | gnu, musl, msvc, pc-windows-msvc, unknown-linux-gnu |

Map host → Pixi platform string for lockfile keys: `linux-64`, `linux-aarch64`, `osx-64`, `osx-arm64`, `win-64`, …

## 9. Installation into Pixi Environments

### 9.1 Local

1. Ensure workspace env exists (`pixi install` if prefix missing — or document that users should run `pixi install` first).
2. Download asset to cache: `$PIXI_HOME/pixi-mise/cache/…` (or `XDG_CACHE_HOME`).
3. Extract to staging dir.
4. Locate binaries (`bin_path`, `bin/`, rename rules, executable bit).
5. Install into `.pixi/envs/<env>/bin/<name>` (copy or hardlink).
6. Persist metadata under `.pixi/envs/<env>/conda-meta/pixi-mise-<tool>.json` (or `.pixi/mise/`) so `list` / `remove` work without scanning heuristics.

### 9.2 Global (sidecar design)

Global GitHub tools are owned by pixi-mise, not by Pixi’s conda global manifest.

1. Create/reuse `$PIXI_HOME/envs/pixi-mise-<tool>/` (or a shared env via `--environment`).
2. Install the binary into that env’s `bin/`.
3. Expose onto `$PIXI_HOME/bin` with a symlink (copy on Windows / when linking fails).
4. Record the declared tool in the sidecar `$PIXI_HOME/pixi-mise.toml`; record install metadata under `$PIXI_HOME/mise/…` and the global lock at `$PIXI_HOME/pixi-mise.lock`.
5. Document that `$PIXI_HOME/bin` must be on `PATH` (same requirement as `pixi global`).

**Why a sidecar instead of editing `pixi-global.toml`?**

Pixi’s global manifest (`$PIXI_HOME/manifests/pixi-global.toml`) models **conda/PyPI dependencies** plus `exposed` mappings, and `pixi global sync` assumes it owns those environments. pixi-mise installs **GitHub release binaries**, which are not conda packages. Writing fake env entries into `pixi-global.toml` would risk sync conflicts and couple us to a schema without a stable “external binary provider” API.

Decision: keep `$PIXI_HOME/pixi-mise.toml` + direct `$PIXI_HOME/bin` exposure. Conda globals stay under Pixi (`pixi global …`); GitHub globals stay under pixi-mise (`pixi mise global …`). Revisit only if Pixi adds a supported extension point for non-conda global binaries.
### 9.3 Why not generate Conda packages in v1?

Generating a noarch/generic `.conda` and installing via rattler would integrate deeper with Pixi lockfiles, but adds packaging complexity (repodata, build strings, multi-platform). Direct prefix install matches mise’s model and unblocks the core UX faster. A future “export as conda package” backend remains compatible with this design.

## 10. CLI Surface

Naming follows Pixi’s built-in commands wherever the behavior matches. Aliases match Pixi where applicable (`a`, `i`, `rm`, `ls`).

### 10.1 Workspace commands

| Command | Pixi analogue | Purpose |
|---------|---------------|---------|
| `pixi mise add <tool>[@version]` | `pixi add` | Add tool to config + install |
| `pixi mise remove <tool>` | `pixi remove` | Remove from config + uninstall bins |
| `pixi mise install [tool]` | `pixi install` | Install from config / one tool |
| `pixi mise reinstall [tool]` | `pixi reinstall` | Force re-download / re-link bins |
| `pixi mise update [tool]` | `pixi update` | Re-resolve within version specs; refresh lock + env |
| `pixi mise upgrade [tool]` | `pixi upgrade` | Loosen / bump version specs in config + refresh lock |
| `pixi mise list` | `pixi list` | List configured / installed tools |
| `pixi mise search <tool>` | `pixi search` | List remote versions / releases for a tool |
| `pixi mise lock` | `pixi lock` | Rewrite lockfile from current resolution (no install) |
| `pixi mise clean cache` | `pixi clean cache` | Clear download cache |

### 10.2 Global commands

Mirror `pixi global …` instead of a `--global` flag on workspace commands:

| Command | Pixi analogue | Purpose |
|---------|---------------|---------|
| `pixi mise global add <tool>` | `pixi global add` | Add + install into global Pixi env / expose |
| `pixi mise global remove <tool>` | `pixi global remove` | Remove global tool |
| `pixi mise global install` | `pixi global install` | Install from global config |
| `pixi mise global list` | `pixi global list` | List global tools |
| `pixi mise global update [tool]` | `pixi global update` | Update global tools within specs |

### 10.3 Extension-only commands

No direct Pixi equivalent; keep for GitHub-binary workflows:

| Command | Purpose |
|---------|---------|
| `pixi mise resolve` | Show resolved assets without installing (dry-run) |
| `pixi mise which <bin>` | Print installed binary path |

Shared flags: `--environment`, `--platform` (cross-resolve for lock), `--dry-run` / `-n`, `--verbose`. Prefer Pixi-style update options (`--frozen`, `--locked`, `--no-install`) where they apply.

## 11. Auth, Cache, Verification

- Auth: `GITHUB_TOKEN`, `GH_TOKEN`, then optional `gh` CLI host token (mise-compatible idea).
- Cache downloads by URL / asset id; honor `ETag` / `If-None-Match` when practical.
- Verify `checksum` when provided; record sha256 into lockfile after first trusted install.
- Optional later: GitHub attestations / SLSA provenance (mise already does this).

## 12. Error UX

Actionable errors, modeled on mise:

- No matching asset → print host platform + scored/available asset names.
- Ambiguous multi-binary release → suggest `matching` / `matching_regex`.
- Missing Pixi env prefix → suggest `pixi install`.
- Rate-limited GitHub → suggest setting `GITHUB_TOKEN`.

## 13. Testing Strategy

- **Unit**: AssetPicker scoring fixtures (tables of asset lists → expected pick).
- **Unit**: Version prefix / latest selection against recorded GitHub JSON fixtures.
- **Integration**: install a small public release (e.g. `cli/cli` or `BurntSushi/ripgrep`) into a temp Pixi workspace in CI.
- **Snapshot**: lockfile + metadata JSON.

## 14. Implementation Phases

### Phase 0 — Skeleton ✅

- Cargo workspace, `pixi-mise` binary, clap CLI stubs for workspace + global verbs.
- Core types (`ToolRequest`, `VersionSpec`, …) and `parse_tool_spec`.
- Library crate stubs: `pixi-mise-github`, `pixi-mise-assets`, `pixi-mise-pixi` (prefix path helper).
- Document install as Pixi extension (`PATH` / future conda package) in `README.md`.

### Phase 1 — GitHub install MVP ✅

- Parse `[tool.pixi-mise.tools]` from `pixi.toml`.
- Resolve latest/exact/prefix; AssetPicker autodetection.
- Install into local env `bin/`; `add` / `install` / `list` / `remove`.

### Phase 2 — Global + lockfile ✅

- `pixi mise global …` path + `$PIXI_HOME/bin` exposure.
- `pixi-mise.lock` with checksums.
- `matching`, `asset_pattern`, `rename_exe`, `bin` (config keys partially accepted in Phase 1).

### Phase 3 — Polish ✅

- `search`, `update` / `upgrade`, `reinstall`, `clean cache`, better semver (`^` / `~` / prefix).
- Optional `mise.toml` github-tool import (`import-mise`).
- CI integration tests.

### Phase 4 — Registry ✅

- Consume aqua-registry (per-package YAML) and optional local `pixi-mise-registry.toml` for asset templates / overrides.
- Platform-specific tool filters (`os = ["linux", "macos/arm64"]`) plus aqua `supported_envs`.
- `pixi mise registry <tool> [--tag …]` to inspect resolved recipes.

## 15. Open Questions

1. **Lockfile location** — Workspace: commit `pixi-mise.lock` beside `pixi.lock`. Global: `$PIXI_HOME/pixi-mise.lock`.
2. **Upgrade policy for `latest`** — Pin on first install (lockfile); bump within constraints on `update`, loosen manifest specs on `upgrade` (Pixi semantics).
3. **Windows shims** — Copy `.exe` vs. generate `.cmd` wrappers when exposing globally (currently copy on non-Unix).
4. **Multi-env workspaces** — Should tools be per-feature/per-environment in `pixi.toml`, or always default env unless flagged?

## 16. Success Criteria

- `pixi mise add github:BurntSushi/ripgrep@14` in a Pixi workspace makes `rg` available under `pixi run rg` / `pixi shell`.
- `pixi mise global add github:cli/cli` makes `gh` available from `$PIXI_HOME/bin`.
- Same config resolves correct assets on `linux-64`, `osx-arm64`, and `win-64` without per-OS `asset_pattern` for well-named releases.
- Lockfile enables bit-for-bit reproducible CI installs when checksums are present.
