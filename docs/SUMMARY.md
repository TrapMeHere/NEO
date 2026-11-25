# Summary

[Project Introduction](introduction.md)

# User Guide

- [Installation and Running](guide/installation.md)
- [Quick Start](guide/quick-start.md)
- [Interface Overview](guide/interface.md)
- [Editing Operations](guide/editing.md)
- [Files, Sessions and Recovery](guide/files-and-sessions.md)
- [Settings and Languages](guide/settings-and-i18n.md)
- [OSM Upload](guide/sync.md)
- [Performance and Large Data](guide/performance.md)
- [FAQ](guide/faq.md)

# Developer Guide

- [Architecture Overview](dev/architecture.md)
- [Workspace and Crate Responsibilities](dev/workspace.md)
- [Build and Test](dev/build-and-test.md)
- [Contribution Workflow](dev/contributing.md)
- [Release Workflow (CI / Release / Pages)](dev/release.md)
- [Roadmap and Risks](dev/roadmap.md)

# Technical Design Documents

- [00 Vision & Scope](00-vision-and-scope.md)
- [01 System Architecture](01-system-architecture.md)
- [02 Workspace and Dependency Rules](02-workspace-and-dependency-rules.md)
- [03 OSM Data Model and Store](03-osm-data-model-and-store.md)
- [04 Spatial Index and Geometry](04-spatial-index-and-geometry.md)
- [05 wgpu Renderer](05-renderer-wgpu.md)
- [06 Editing Core and Undo](06-editing-core-and-undo.md)
- [07 Concurrency and Tasks](07-concurrency-and-tasks.md)
- [08 IO: PBF/XML/osmChange](08-io-pbf-xml-osc.md)
- [09 OSM API and Sync](09-osm-api-and-sync.md)
- [10 Persistence and Recovery](10-persistence-and-recovery.md)
- [11 Styles, Presets and MapCSS](11-style-presets-mapcss.md)
- [12 Validator](12-validation.md)
- [Known Deviations from JOSM](known-deviations.md)
- [13 JOSM Parity](13-josm-parity.md)
- [14 Performance and Verification](14-performance-and-verification.md)
- [15 Plugin System](15-plugin-system.md)
- [16 Roadmap and Risks](16-roadmap-and-risks.md)
- [17 Licensing and Governance](17-licensing-and-governance.md)
- [18 Testing Strategy](18-testing-strategy.md)
- [Feature Completeness vs iD/JOSM](feature-parity.md)
- [Health Audit and Fix Report (2026-11-21)](health-audit-2026-11-21.md)

# Architecture Decision Records (ADR)

- [0001 Rust Native + GPU-first](adr/0001-rust-native-gpu-first.md)
- [0002 Modular Monolith](adr/0002-modular-monolith.md)
- [0003 SoA + Arena + Generational Handles](adr/0003-soa-arena-storage.md)
- [0004 Separation of Data/Render/Editing Models](adr/0004-independent-data-render-editor-models.md)
- [0005 egui and Map Share a wgpu Device](adr/0005-wgpu-egui-shared-device.md)
- [0006 Single-Writer Editor + Immutable Snapshots](adr/0006-single-writer-editor.md)
- [0007 WASM Plugins](adr/0007-wasm-plugins.md)
- [0008 Command + Transaction](adr/0008-command-transaction-undo.md)
- [0009 Incremental Updates](adr/0009-incremental-update.md)
- [0010 MapCSS Compatibility Strategy](adr/0010-mapcss-compatibility.md)
- [0011 Dependency Coherence and Version Pinning](adr/0011-dependency-coherence-pinning.md)
