# Workspace and crate responsibilities

The Cargo workspace members are `crates/*` and `apps/*`; versions, dependency versions, and lints are centralized in the root `Cargo.toml`.
The dependency direction is defined by [docs/02](../02-workspace-and-dependency-rules.md) and enforced by
`scripts/check-deps.py`.

## Crate overview

| crate | Responsibility | Allowed dependencies (core) |
|---|---|---|
| `neo-core` | IDs, lat/lon/world coordinates, generational handles, Meta, errors | none (leaf) |
| `neo-osm` | OSM entities, change state, tag model | core |
| `neo-store` | `DataSet` (SoA+Arena), rstar index, backreferences, integrity | core, osm |
| `neo-geo` | camera, projection, world coordinates, picking, geometry | core, osm, store |
| `neo-scene` | visibility/LOD, batching, vertex generation | core, osm, store, geo, style |
| `neo-render` | wgpu pipeline, GpuScene, headless rendering | core, geo, scene (**does not depend on store/editor/ui**) |
| `neo-io` | PBF/XML/osmChange read/write, synthetic data | core, osm, store |
| `neo-editor` | Command/Transaction, History, Journal, Selection, Editor | core, osm, store, geo |
| `neo-tools` | high-level edit actions (select/draw/drag/merge, etc.) | core, editor |
| `neo-persist` | JOSM sessions `.jos`/`.joz` | core, osm, store |
| `neo-ui` | egui panels, theme, i18n calls, UiActions | core, osm, store, geo, scene, render, editor, tools, i18n |
| `neo-i18n` | zh-CN/zh-TW/en-US text catalogs (embedded at compile time, zh as fallback) | none |
| `neo-api` | OSM API 0.6 / upload (phase 5) | core, osm |
| `neo-query` | cross-layer read-only queries (plugins/validation/tasks) | core, osm, store, geo |
| `neo-validate` | validation rules (phase 4) | core, osm, store, geo |
| `neo-style` | MapCSS / styles (phase 4) | core, osm |
| `neo-plugin` | WASM plugin host (phase 6) | core, query, editor |
| `apps/neo-desktop` | desktop app: eframe + wgpu + panel orchestration | all |
| `apps/neo-prototype` | performance prototype/Gate verification | core, osm, store, io, geo, scene, render |

## Dependency rules

1. **No reverse dependencies**: e.g., `neo-store` must not depend on `neo-editor`, and `neo-render` must not depend on `neo-store`.
2. **GUI is read-only**: `neo-ui` does not depend on `neo-io`/`neo-persist`/`neo-api`; IO is performed by the application layer.
3. **Write interface convergence**: writes to `DataSet` may only go through `neo-editor` commands/transactions (checked by `check-write-api.py`).
4. **Adding a crate**: first update the diagram and table in docs/02, then add the dependency version in the root `Cargo.toml`;
   `cargo check` and `python scripts/check-deps.py` must pass on `master`; `cargo test` also runs on `master`.

## Common checks

```bash
python scripts/check-deps.py        # dependency direction
python scripts/check-write-api.py   # write API sealing
python scripts/check-doc-links.py   # documentation links
python scripts/check-i18n.py        # i18n keys
```
