# pixi-mise

Install GitHub release binaries into Pixi-managed global or local environments — similar to [mise](https://mise.jdx.dev/), as a [Pixi extension](https://pixi.prefix.dev/latest/integration/extensions/introduction/).

## Status

**Phase 0 (skeleton)** — Cargo workspace and `pixi-mise` CLI stubs. Subcommands parse and print a not-implemented message; GitHub resolve/install arrives in Phase 1.

See **[DESIGN.md](./docs/DESIGN.md)** for architecture, resolution pipeline, asset matching, Pixi integration, and implementation phases.

## Install as a Pixi extension

Pixi discovers extensions named `pixi-{command}` on `PATH` (then under `pixi global` directories). After building:

```bash
# From this repo (Pixi-managed Rust toolchain)
pixi run build
export PATH="$PWD/target/debug:$PATH"

# Or copy / symlink onto PATH
ln -s "$PWD/target/debug/pixi-mise" ~/.local/bin/pixi-mise
```

Then invoke through Pixi:

```bash
pixi mise --help
pixi mise add --help
```

Once published to a conda channel, the recommended install will be `pixi global install pixi-mise`.

## Intended usage

Once Phase 1+ is implemented:

```bash
pixi mise add github:BurntSushi/ripgrep@14
pixi mise global add github:cli/cli
pixi mise install
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
```

Workspace layout:

```text
crates/
  pixi-mise/          # binary → pixi-mise
  pixi-mise-core/     # types, config, orchestration
  pixi-mise-github/   # GitHub API (stub)
  pixi-mise-assets/   # AssetPicker scoring (stub)
  pixi-mise-pixi/     # Pixi prefix / global (partial)
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
