# Changelog

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- Repository baseline language switched to English: English is now the default UI language and `i18n/en-US.json` is the source language; `zh-CN`/`zh-TW` are translations (fallback chain `zh-TW → zh-CN → en-US`); documentation, code comments, scripts and CI templates are now written in English
- The desktop app now is English-first: it remembers the language chosen in the Settings menu, otherwise detects the operating system UI locale at startup (English fallback) and selects the matching translation; `NEO_LANG` still overrides both
- Tools menu regrouped by function: select/draw, measure, paths (split at point ↔ merge end-to-end, reverse, simplify), nodes (glue ↔ unglue, align, distribute, mirror in pairs), areas and other geometry
- English words that were previously left untranslated inside the Chinese UI strings (way/node/tag, etc.) are now translated into Chinese, including i18n strings and hints in code
- The tag panel is unified for single and multi selection, with no "bulk tags" title or explanatory text: multi-select shows the **union** of tags; when values differ or some objects lack a key the value column shows "……" and a dropdown lets you pick a value to write to all selected objects; edits apply on blur
- The "sync" window became **upload-only** (File → Upload…): only "upload local changes / upload selected only" and 409 conflict handling remain; the in-window bbox download button and the conflict "re-download" button were removed. Downloads now always go through the separate download window (File → Download… / toolbar cloud download, bbox selection, Overpass)
- Box selection is now **purely geometric**: it no longer depends on whether a feature is rendered; nodes hidden by the zoom LOD are still selected when they fall inside (or intersect) the box; click selection and hover still respect the render LOD
- Road links (`highway=*_link`) no longer fall back to generic grey: `motorway/trunk/primary/secondary/tertiary_link` inherit their parent class's color and text color with a narrower width (~55–60% of the parent); parent roads stay thicker
- Desktop startup no longer loads Monaco or a synthetic map; the map starts empty and the user opens a file or downloads OSM data. The "load sample data" menu was removed
- Fonts and presets are now external resources (`assets/fonts/`, `assets/presets/schema.neo`, `suggestions.neo`), no longer embedded with `include_bytes!`/`include_str!`; release bundles must ship them and `NEO_ASSET_DIR` can point to another root
- Presets use a string-table binary: the schema is loaded when the preset list is first opened, and brand suggestions are deferred until the first search

### Added

- i18n split: Chinese is now `zh-CN` (Simplified, also the `zh` fallback) and `zh-TW` (Traditional); language menu, `NEO_LANG` and the fallback chain (`zh-TW` → `zh-CN` → `en-US`) updated, and the validation script checks all three tables
- Split path at point tool: toolbar/tools menu "Split path at point"; clicking an interior node of a way splits every way through that node, in a single undo step
- Merge end-to-end paths: with two or more paths selected whose endpoints are adjacent, the tools menu "Merge end-to-end paths" merges the whole chain into one way (handles direction automatically, single undo step)
- Multi-select bulk tag editing: with several objects selected the tag panel switches to bulk mode and can add/overwrite `key=value` on all selected objects or remove tags shared by all of them; changes are committed in one transaction and undone with a single `Ctrl+Z`
- Render bridges, tunnels, embankments, cuttings and fords: `neo-style` derives structure semantics from tags (`bridge`/`tunnel`/`embankment`/`cutting`/`ford`) and provides casing plus slope-side symbols; `neo-scene` adds a casing batch (solid bridge casing, dashed tunnel casing, water surface for fords), implicit `layer` ±1 for bridges/tunnels, gold ladder hachures for embankments (ticks + toe line) and brick-red comb hachures for cuttings (shape and hue both differ, not just orientation); the default `neo.mapcss` gains matching rules and MapCSS adds `casing-color` / `casing-width` / `casing-dashes`

### Fixed

- Holed areas (building courtyards/atria) were filled at z≈15.67–16 (when 20px inner-band drawing kicks in): when a hole is not fully inside the band's inner boundary the renderer now falls back to "solid fill + punched holes", so holes are no longer covered by the band

### Removed

- Removed the "filter" feature (View → Filter…, hide selected): the `neo-ui` filter panel, the app-level `hidden` state and related checks, `neo-scene::SceneBuilder::set_hidden`, user-facing text and documentation were all removed

### Added (Phases 5–6)

- `neo-plugin`: wasmtime host for 50 core modules (feature `wasm-plugins`, off by default). Three plugin behaviors: todo panel, utilsplugin add node, buildings_tools rectangular building. Commands go through `Editor::execute`. Fuel limits, 64 MiB memory cap, capability intersection. Signatures use SHA-256, not Ed25519
- `neo-cli`: opens OSM XML / GPX / synthetic data, and supports a scene tile cache, preset override tables, TMS URLs and local update-manifest comparison. The `plugins` feature runs a built-in WASM demo
- `neo-io`: GPX 1.1 read/write (waypoints → nodes, track points → ways)
- `neo-style`: MapCSS `canvas` / `dashes`; unknown text/icon properties recorded as warnings; declarations editor; TMS/WMS URLs and offset database; schema prefix overrides for 22 JOSM groups
- `neo-scene`: LOD sampling, bounded tile cache, rayon-parallel build. The `profile` feature is off by default
- `neo-store`: zigzag + LEB128 delta encoding for coordinates and ordered IDs
- `neo-render`: optional `draw_indexed_indirect`; a compute shader writes `instance_count` as 0
- Packaging scripts: `scripts/package.ps1`, `packaging/windows`, `packaging/linux`, `packaging/macos`. Not executed this round; no installers were produced
- Desktop Tools menu opens an ecosystem window (plugin names, TMS examples, preset gaps); the window does not load WASM

### Added (Phase 4, in progress)

- `neo-validate`: 24 JOSM-style validation rules + `Report` + right-side validation panel (run/stats/click to locate)
- `neo-style`: presets now come from the npm packages `@openstreetmap/id-tagging-schema` and `name-suggestion-index`, generated at build time via `buildIDPresets`. Brand suggestions appear only while searching; stale `*:wikipedia` tags are removed before writing
- `neo-api`: OSM API 0.6 client (changeset create/close, diff upload, map download, 409/412 parsing) + MockTransport tests
- `neo-editor`: `MergeCase` / `ConflictResolution` / three-way tag merge; tests for 6 conflict classes
- `neo-editor`: `RecoveryReport` (frames/applied/skipped), observable recovery state
- `neo-style`: MapCSS subset parse/compile + default stylesheet wired into SceneBuilder
- `neo-persist`: SQLite WAL snapshot store (journal bytes + stats, keeps 10 snapshots)
- `neo-api`: end-to-end changeset upload tests (including retry after 409)
- Sync conflicts: `KeepLocal` (retry after version alignment) and `TakeRemote` (re-download) strategies
- Built-in OAuth client and callback; unified export window (XML/osmChange/PBF/session); bbox download of OSM data from the map

### Planned

- Relation editing improvements, box selection, more validator rules
- Three-platform installer cold start, 10M streaming scene re-test, 1-hour session memory curve
- Ed25519 plugin signatures, real MapCSS text/icon drawing, embedded wasmtime in the desktop app

## [0.1.0] - 2026-11-15

First runnable Windows desktop version (phases 2/3 complete).

### Added

- **Data core**: SoA + Arena + generational-handle OSM data model, tag interning, rstar spatial index,
  CSR referrers, world coordinates/camera/picking (`neo-core`, `neo-store`, `neo-geo`).
- **IO**: PBF (with parallel import), OSM XML, osmChange read/write; 10M-object import/export tests.
- **Editing core**: Command/Transaction/Undo/Redo, selection sets, journal (JSONL) recovery.
- **Rendering**: wgpu pipelines, batching, incremental upload, node LOD (hidden at z≤16 / shared node points at z17–18 /
  all nodes at z19), 16px picking and drag margin.
- **Desktop UI**: JOSM-style layout (menus, toolbar, vertical toolbox, right-side layer/tags/selection/relation panels,
  bottom lat/lon + scale + measure readout), dark theme, Phosphor icons.
- **Layers**: show/hide, active layer; inactive layers are shader-greyed and 50% opaque, read-only.
- **Settings**: language (Chinese/English) and shortcut overview; runtime `NEO_LANG`.
- **i18n**: `i18n/zh-CN.json`, `en-US.json` + `neo-i18n` crate (compile-time embedding).
- **Measure tool** (M): two-point line showing m/km.
- **Packaging**: exe icon (winres), built-in Noto Sans CJK SC / GNU Unifont / Unifont Upper fallback fonts.
- **Engineering**: GitHub Actions (CI / release / Pages), mdBook docs site, issue/PR templates.

### Changed

- The UI moved from the earlier single panel to a JOSM-style layout; language/shortcuts moved into the Settings window.
- Rendering performance fixes: dragging no longer rebuilds the scene every frame; zooming back out no longer shows a blank map.

### Fixed

- Hidden nodes intercepted way clicks; they are now filtered out during picking and the way is picked instead.
- Dragging decoupled from selection to avoid per-frame rebuilds.
- Startup no longer opens a console window (release GUI subsystem).

[Unreleased]: https://github.com/TrapMeHere/NEO/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/TrapMeHere/NEO/releases/tag/v0.1.0
