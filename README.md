# pixi-mise

Install GitHub release binaries into Pixi-managed global or local environments — similar to [mise](https://mise.jdx.dev/), as a [Pixi extension](https://pixi.prefix.dev/latest/integration/extensions/introduction/).

## Status

Design phase. See **[DESIGN.md](./DESIGN.md)** for architecture, resolution pipeline, asset matching, Pixi integration, and implementation phases.

## Intended usage

Once implemented, the Rust binary ships as `pixi-mise` and is invoked through Pixi:

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
