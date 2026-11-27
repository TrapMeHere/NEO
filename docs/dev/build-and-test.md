# Build and Test

## Prerequisites

- Rust stable (`rust-toolchain.toml` pins stable + rustfmt + clippy; MSRV 1.97)
- Windows: Visual Studio Build Tools (MSVC), Windows 10/11 SDK
- Linux: `libxkbcommon`, Wayland/X11, Vulkan drivers, etc. (`eframe` dependencies)
- Python 3 (repository scripts: dependency, i18n, write-API and documentation-link checks)
- Node.js 22. Preset data is declared with npm like iD: `@openstreetmap/id-tagging-schema` and `name-suggestion-index`. Run `npm ci` at the repository root before compiling `neo-style`

## Common commands

On Windows you can double-click `scripts/build.cmd` (it builds immediately). It installs the preset npm packages, builds `neo-desktop` (without cleaning `out/cargo`), then copies the exe and runtime dependencies (`assets/fonts`, `assets/presets`) to `out/neo-desktop/{debug|release}/`, side by side with the exe so it can run. Optional arguments: `-Release`, `-Run`, `-Workspace`.

```bash
# preset packages (before compiling neo-style)
npm ci

# build (release)
cargo build --release -p neo-desktop
# or: pwsh -File scripts/build.ps1 -Release

# run
cargo run -p neo-desktop
# or: pwsh -File scripts/build.ps1 -Run

# format / lint (CI gates)
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings

# full test suite
cargo test --workspace

# single crate
cargo test -p neo-editor
cargo test -p neo-geo picking
```

CI on `master` runs format / clippy / check / test / `--all-features` check in order, then the i18n, dependency, documentation-link and write-API scripts; `.github/workflows/test.yml` is a legacy workflow for the retired `test` branch and does not run on `master`.

## Documentation site

The documentation site is bilingual: English (`docs/`) is served at `/en/` and Chinese
(`docs-zh/`) at `/zh/`. Build both books and add the language landing page:

```bash
mdbook build          # English -> book/en
mdbook build docs-zh  # Chinese -> book/zh
cp site/index.html book/index.html
```

The fixed language switcher lives in `theme/lang-switch.js` + `theme/lang-switch.css`
(copied into `docs-zh/theme/` for the Chinese book).

## Test layers

| Layer | Location | Contents |
|---|---|---|
| Unit tests | `#[cfg(test)]` in each crate | data model, geometry, LOD, commands, i18n |
| Integration tests | `crates/*/tests/` | PBF/XML/OSC roundtrip, parallel import cancellation, G3 acceptance |
| Documentation checks | `scripts/check-*.py` | dependency direction, write API, documentation links, i18n keys |
| Desktop smoke | Manual on Windows | launch the desktop app, open a small PBF, then exercise draw/select/undo and the main panels; screenshots can be saved under `out/ui-check/` (already in `.gitignore`) |

## Performance verification

```powershell
$env:NEO_PERF_LOG = "out\perf.log"
$env:NEO_SYNTH_N = "1000000"
.\out\cargo\release\neo-desktop.exe
```

The log contains `scene build`, `gpu upload`, and `ui frame`; thresholds and baselines see
[14 Performance and Verification](../14-performance-and-verification.md).

## i18n

```bash
cargo test -p neo-i18n
python scripts/check-i18n.py
```

Adding text: edit `i18n/zh-CN.json` and `i18n/en-US.json` (`zh-TW.json` can be added later; missing entries fall back to Simplified Chinese), and use
`neo_i18n::t("key")` / `tf("key", &[("n","3")])` in code.

## Documentation site

```bash
cargo install mdbook --version 0.4.52 --locked
mdbook serve --open        # http://localhost:3000
mdbook build               # output to book/
```

## FAQ

- **Link error `LNK1104` / MSVC not found**: install the C++ workload of VS Build Tools.
- **wgpu initialization fails**: update the graphics driver; Windows requires DX12 support.
- **Fonts/presets display incorrectly**: fonts and `schema.neo`, `suggestions.neo` are external. `scripts/build.ps1` /
  `package.ps1` copy them to `out/neo-desktop/{debug|release}/assets/` (side by side with the exe). You can also set `NEO_ASSET_DIR`, or load them from the repository root's `assets/` during development. `neo.ico`
  is still embedded into the exe by `build.rs`.
- **Size baseline**: see [binary-size-baseline.md](../prototypes/binary-size-baseline.md);
  reproduce with `pwsh -File scripts/bloat-baseline.ps1`.
- **UI test cannot find the window**: make sure the desktop session is logged in and the target exe is a release build.
