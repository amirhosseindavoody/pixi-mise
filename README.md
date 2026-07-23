# pixi-mise

Install GitHub release binaries into Pixi-managed global or local environments — similar to [mise](https://mise.jdx.dev/), as a [Pixi extension](https://pixi.prefix.dev/latest/integration/extensions/introduction/).

## Status

**Phase 3 (polish)** — `search`, `update` / `upgrade`, `reinstall`, `which`, `clean cache`, caret/tilde semver, `import-mise`, and CI install smoke.

See **[DESIGN.md](./docs/DESIGN.md)** for architecture, resolution pipeline, asset matching, Pixi integration, and implementation phases.

## Install as a Pixi extension

Pixi discovers extensions named `pixi-{command}` on `PATH` (then under `pixi global` directories).

### Global install from source (recommended)

This repo defines a Pixi package (`[package]` + `pixi-build-rust` in `pixi.toml`). Install it globally with Pixi so `pixi-mise` lands on `$PIXI_HOME/bin` (ensure that directory is on your `PATH`, same as other `pixi global` tools):

```bash
# From a local clone
git clone https://github.com/amirhosseindavoody/pixi-mise.git
cd pixi-mise
pixi global install --path .

# Or directly from GitHub
pixi global install --git https://github.com/amirhosseindavoody/pixi-mise.git
```

Then invoke through Pixi:

```bash
pixi mise --help
pixi mise add --help
```

### Development build (without packaging)

```bash
pixi run build
export PATH="$PWD/target/debug:$PATH"
```

Once published to a conda channel, the recommended install will be `pixi global install pixi-mise`.

## Usage

```bash
# Workspace (local Pixi env)
pixi mise add github:BurntSushi/ripgrep@14
pixi mise install
pixi mise list
pixi mise search github:BurntSushi/ripgrep
pixi mise update
pixi mise upgrade github:BurntSushi/ripgrep
pixi mise reinstall
pixi mise which rg
pixi mise lock
pixi mise import-mise
pixi mise clean cache
pixi mise remove github:BurntSushi/ripgrep

# Global ($PIXI_HOME/envs/… + expose on $PIXI_HOME/bin)
pixi mise global add github:cli/cli
pixi mise global update
pixi mise global list
pixi mise global remove github:cli/cli
```

Version specs accept `latest`, exact tags (`14.1.1`), prefixes (`14`), caret (`^1.2.3`), and tilde (`~1.2.3`). `update` re-resolves within the current spec; `upgrade` bumps the config pin to the newest Exact release.

Tools are declared in `pixi.toml`:

```toml
[tool.pixi-mise.tools]
"github:BurntSushi/ripgrep" = "14.1.1"
"github:cli/cli" = { version = "latest", matching = "gh_" }
"github:example/tool" = { version = "1.2.3", asset_pattern = "tool-{{os}}-{{arch}}.tar.gz", rename_exe = "tool" }
```

Global tools live in `$PIXI_HOME/pixi-mise.toml`:

```toml
[tools]
"github:cli/cli" = { version = "latest", matching = "gh_" }
```

Installs write `pixi-mise.lock` (workspace) or `$PIXI_HOME/pixi-mise.lock` (global) with asset URL + `sha256:…`. Use `pixi mise install --locked` to reuse locked assets.

## Development

```bash
pixi install
pixi run check
pixi run test
pixi run build
pixi run package   # conda package via pixi-build-rust → dist/
```

Workspace layout:

```text
crates/
  pixi-mise/src/      # CLI binary source (package root is Cargo.toml)
  pixi-mise-core/     # types, config, resolve, install, lockfile
  pixi-mise-github/   # GitHub API client
  pixi-mise-assets/   # AssetPicker scoring
  pixi-mise-pixi/     # Pixi prefix / metadata / global expose
```

## Why

Pixi covers Conda/PyPI well. Many CLI tools only publish GitHub release assets. `pixi-mise` brings mise-style GitHub backend resolution (config discovery, version resolve, OS/arch asset scoring) into Pixi environments so those binaries show up on the same `PATH` as the rest of your Pixi toolchain.

## Design highlights

- **Pixi extension model** — executable named `pixi-mise` → `pixi mise …`
- **Mise-inspired pipeline** — discover → resolve → pick asset → install → expose
- **AssetPicker scoring** — OS / arch / libc / archive format / penalties (mise `asset_matcher` model)
- **Local + global targets** — `.pixi/envs/<env>/bin` and `$PIXI_HOME` global exposure
- **Lockfile** — `pixi-mise.lock` with sha256 for reproducible installs
- **Rust workspace** — CLI + core + GitHub + assets + Pixi adapter crates

## License

See [LICENSE](./LICENSE).
