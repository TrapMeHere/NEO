# Binary size baseline (external fonts + external presets)

- **Date**: 2026-11-21
- **Command**: `cargo bloat --release -p neo-desktop --crates --n 40`
- **Reproduce**: `powershell -File scripts/bloat-baseline.ps1`
- **profile**: `[profile.release]` opt-level=3, lto=thin, codegen-units=1, strip=true

## Comparison

| Item | Before externalization (approx.) | After externalization |
|---|---:|---:|
| `neo-desktop.exe` | ~65 MB | **23.4 MB** |
| Fonts | compiled into the exe (~26 MB) | 26.4 MB side-by-side in `assets/fonts/` |
| Presets | compiled into the exe (~15 MB) | `schema.neo` 0.19 MB + `suggestions.neo` 6.70 MB |

Installers still need to ship `assets/`; the main program itself is about **40 MB** smaller.

## Release artifact sizes

| Item | Size |
|---|---:|
| `out/cargo/release/neo-desktop.exe` | 23.44 MB |
| `assets/presets/schema.neo` + `suggestions.neo` (external) | 0.19 + 6.70 MB |
| `assets/fonts/*` (external, total) | 26.36 MB |

## Top `.text` entries (cargo-bloat)

| Crate | Approximate size |
|---|---:|
| std | 2.6 MiB |
| naga | 1.7 MiB |
| egui | 1.1 MiB |
| wgpu_core | ~1.0 MiB |
| wgpu_hal | 0.8 MiB |
| rustls | 0.4 MiB |
| neo_desktop | 0.3 MiB |

Full table: [binary-size-bloat-crates.txt](./binary-size-bloat-crates.txt).

To slim it down further, optional features could disable `neo-api`/http (rustls, reqwest, aws_lc) or `neo-persist` (rusqlite); the graphics stack (naga/wgpu/egui) is hard to split out.
