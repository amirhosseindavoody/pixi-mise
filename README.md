# pixi-mise

Install GitHub release binaries into Pixi-managed global or local environments — similar to [mise](https://mise.jdx.dev/), as a [Pixi extension](https://pixi.prefix.dev/latest/integration/extensions/introduction/).

## Status

**Phase 1 (GitHub install MVP)** — Workspace `add` / `install` / `list` / `remove` resolve GitHub releases, pick a platform asset, and install binaries into `.pixi/envs/<env>/bin`. Global installs and lockfiles arrive in Phase 2.

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

## Intended usage

```bash
pixi mise add github:BurntSushi/ripgrep@14
pixi mise install
pixi mise list
pixi mise remove github:BurntSushi/ripgrep

# Global (Phase 2)
pixi mise global add github:cli/cli
```

Tools are declared in `pixi.toml`:

```toml
[tool.pixi-mise.tools]
"github:BurntSushi/ripgrep" = "14.1.1"
"github:cli/cli" = { version = "latest", matching = "gh_" }
```

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
  pixi-mise-core/     # types, config, resolve, install
  pixi-mise-github/   # GitHub API client
  pixi-mise-assets/   # AssetPicker scoring
  pixi-mise-pixi/     # Pixi prefix / metadata / bin install
```

## Why

Pixi covers Conda/PyPI well. Many CLI tools only publish GitHub release assets. `pixi-mise` brings mise-style GitHub backend resolution (config discovery, version resolve, OS/arch asset scoring) into Pixi environments so those binaries show up on the same `PATH` as the rest of your Pixi toolchain.

## Design highlights

- **Pixi extension model** — executable named `pixi-mise` → `pixi mise …`
- **Mise-inspired pipeline** — discover → resolve → pick asset → install → expose
- **AssetPicker scoring** — OS / arch / libc / archive format / penalties (mise `asset_matcher` model)
- **Local + global targets** — `.pixi/envs/<env>/bin` and `$PIXI_HOME` global exposure
- **Rust workspace** — CLI + core + GitHub + assets + Pixi adapter crates

## License

See [LICENSE](./LICENSE).
