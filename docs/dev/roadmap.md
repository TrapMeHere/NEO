# Roadmap and Risks

## Phases

| Phase | Contents | Status |
|---|---|---|
| 1 | Technical foundation: workspace, data model, render prototype, performance gates | ✅ |
| 2 | OSM data core: PBF/XML/OSC, indexes, DataSet, memory ledger | ✅ |
| 3 | Editing core: command/transaction/undo/journal, selection/drag/way operations, JOSM-style UI | ✅ |
| 4 | Professional editing: validator, presets, MapCSS, OAuth, sync, conflict strategy, SQLite snapshots | ✅ |
| 5 | Full ecosystem: WASM plugins, GPX, imagery URLs, preset overrides, CLI, packaging scripts | ⚠️ G5 Conditional Go (see [gate-5](../prototypes/gate-5-ecosystem.md)) |
| 6 | Extreme performance: streaming LOD, indirect draw, coordinate compression, parallel scene building | ⚠️ Not at RC (see [gate-6](../prototypes/gate-6-performance.md)) |

For real-time progress and per-round changes see
[TODO.md](https://github.com/TrapMeHere/NEO/blob/master/TODO.md) at the repository root.

## Phase 4 delivered (v0.1.0 baseline)

- `neo-validate`: 7 rules + report + right-side panel (run/click to locate)
- `neo-style`: 8 built-in presets (fill tags from the UI dropdown)
- `neo-api`: OSM API 0.6 protocol client (changeset / diff upload / map download / 409/412 parsing) + mock transport tests
- `neo-editor`: MergeCase/ConflictResolution/tag three-way merge + 6 kinds of conflict tests; journal recovery report `RecoveryReport`

Delivered: MapCSS subset styles, real HTTP sync UI, KeepLocal/TakeRemote conflict strategies, SQLite snapshots,
end-to-end changeset (mock) tests. Still pending: manual merge UI, the full MapCSS text/icon set,
pre-upload validator gate, manual acceptance against the real network.

## Current limitations

- Single data layer (multi-layer UI is already reserved)
- The plugin host exists only in `neo-cli --features plugins`; the desktop app does not link wasmtime
- Three-platform installer scripts have not been run; the 10M new-path and 1-hour-session cases are untested
- Keyboard shortcuts use built-in bindings (custom bindings are still to be done)
- Release is Windows x64 only (Linux/macOS build from source)

## Main risks

| Risk | Impact | Mitigation |
|---|---|---|
| GPU/driver differences | Startup failure/render anomalies | headless tests + error dialog + logging |
| Oversized PBF memory peaks | OOM | incremental, parallel import, memory ledger (docs/03) |
| Behavioral differences from JOSM | User migration cost | parity matrix (docs/13) + measurements |
| License contamination (JOSM assets) | Legal risk | clean-room + asset isolation (docs/17) |
| Dependency upgrades breaking wgpu/egui | Compile failure | version pinning (ADR-0011) + Dependabot grouping |

## Contributing

- Claim issues: `good first issue` / `help wanted`
- Design discussions: issue + ADR
- Performance data: include machine information and reproduction commands
