# NEO Documentation

> NEO = Next Editor for OSM: a native Rust, GPU-first OpenStreetMap editor aiming to fully replace JOSM.

**Quick links**

- [Project introduction (documentation site home)](introduction.md)
- [User guide](guide/quick-start.md): installation, getting started, interface, editing, files, settings, performance, FAQ
- [Developer guide](dev/architecture.md): architecture, workspace, build and test, contribution, release, roadmap
- [Documentation site](https://trapmehere.github.io/NEO/) (published from this directory + `book.toml` via mdBook)
- [Architecture Decision Records (ADR)](adr/)

> This directory is the **source of truth for architecture**; code comments reference the files and sections here. If the two disagree, the documentation wins and the code must be fixed.

## Reading order

| Order | Document | Content | Audience |
|---|---|---|---|
| 1 | [00 Vision & Scope](./00-vision-and-scope.md) | Positioning, goals, non-goals, success metrics | Everyone |
| 2 | [01 System Architecture](./01-system-architecture.md) | Eight-layer model, thread/GPU queues, end-to-end sequencing, invariants | Everyone |
| 3 | [02 Workspace & Dependency Rules](./02-workspace-and-dependency-rules.md) | Crate DAG, CI enforcement, version coherence, feature strategy | Contributors |
| 4 | [03 Data Model & Store](./03-osm-data-model-and-store.md) | SoA/Arena/handles, tag interning, memory ledger, DataSet | Data/editing developers |
| 5 | [04 Spatial Index & Geometry](./04-spatial-index-and-geometry.md) | Projection/camera coordinate systems, rstar, picking, LOD | Geometry/rendering developers |
| 6 | [05 wgpu Renderer](./05-renderer-wgpu.md) | Pipelines, batches, incremental uploads, GPU picking, fonts | Rendering developers |
| 7 | [06 Editing Core & Undo](./06-editing-core-and-undo.md) | Command/Transaction, Journal, conflicts | Editor developers |
| 8 | [07 Concurrency & Tasks](./07-concurrency-and-tasks.md) | Single writer, snapshot publication, task system, deadlock checklist | Everyone |
| 9 | [08 IO: PBF/XML/osmChange](./08-io-pbf-xml-osc.md) | Import pipeline, format details, write-side plan | Data developers |
| 10 | [09 OSM API & Sync](./09-osm-api-and-sync.md) | API 0.6, changesets, diff upload | Sync developers |
| 11 | [10 Persistence & Recovery](./10-persistence-and-recovery.md) | SQLite WAL, journal, autosave | Infrastructure |
| 12 | [11 Styles, Presets & MapCSS](./11-style-presets-mapcss.md) | MapCSS compatibility, Presets | Styling/UI |
| 13 | [12 Validator](./12-validation.md) | Rule model, incremental validation, upload gate | Data quality |
| 14 | [13 JOSM Parity](./13-josm-parity.md) | Parity matrix, completion criteria, easily missed behaviors | Product/everyone |
| 15 | [14 Performance & Verification](./14-performance-and-verification.md) | Metrics, benchmarking methodology, measured baselines | Everyone |
| 16 | [15 Plugin System](./15-plugin-system.md) | WASM host, permissions, compatibility strategy | Platform/plugin authors |
| 17 | [16 Roadmap & Risks](./16-roadmap-and-risks.md) | Six phases, Gates, risk register | Everyone |
| 18 | [17 Licensing & Governance](./17-licensing-and-governance.md) | License choice, clean-room, resource compliance | Legal/maintainers |
| 19 | [18 Testing Strategy](./18-testing-strategy.md) | Test pyramid, golden tests, GPU tests, CI | Everyone |

## ADRs (Architecture Decision Records)

| Number | Decision | Status |
|---|---|---|
| [0001](./adr/0001-rust-native-gpu-first.md) | Rust Native + GPU-first | accepted |
| [0002](./adr/0002-modular-monolith.md) | Modular monolith rather than microservices | accepted |
| [0003](./adr/0003-soa-arena-storage.md) | SoA + Arena + generation-handle storage | accepted |
| [0004](./adr/0004-independent-data-render-editor-models.md) | Separation of data/render/editing models | accepted |
| [0005](./adr/0005-wgpu-egui-shared-device.md) | egui and map rendering share the wgpu Device/Queue | accepted |
| [0006](./adr/0006-single-writer-editor.md) | Single-writer editing + immutable snapshots | accepted |
| [0007](./adr/0007-wasm-plugins.md) | WASM plugins rather than Rust dynamic libraries | accepted |
| [0008](./adr/0008-command-transaction-undo.md) | Command + Transaction as the only write path | accepted |
| [0009](./adr/0009-incremental-update.md) | Incremental geometry/index/GPU updates | accepted |
| [0010](./adr/0010-mapcss-compatibility.md) | MapCSS subset compatibility strategy | accepted |
| [0011](./adr/0011-dependency-coherence-pinning.md) | Dependency coherence and version pinning strategy | accepted |

## Conventions

- English prose, technical terms kept in English; file names in English kebab-case.
- Structure: summary → goals/scope → design → key interfaces (Rust signatures) → data flow → tradeoffs → risks → acceptance → references.
- **Do not invent numbers**: performance figures always reference the measured tables in [14](./14-performance-and-verification.md); unmeasured items are written as `[pending measurement]`.
- External claims must include a URL; anything unverifiable is marked `> to be verified`.
- Crate names, type names, and method names in the docs must match the actual code in `crates/*`; when an interface changes, update the docs too (a PR checklist item).

## Prototype and Gate reports

- Performance verification prototype: `apps/neo-prototype` (commands in the root `README.md`).
- Measured output: `out/prototype-bench.md`, `out/prototype-render.md` (generated by the prototype, reproducible).
- Gate conclusion: [`prototypes/gate-1-prototype.md`](./prototypes/gate-1-prototype.md).

## License

Documentation is under **CC-BY-SA-4.0**; code is under **Apache-2.0 OR MIT**. See the root
[LICENSE](../LICENSE) and [NOTICE](../NOTICE) for details.
