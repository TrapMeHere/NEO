# 00 · Vision & Scope

> NEO — **Next Editor for OSM**. A native Rust rewrite of an OpenStreetMap editor, aiming to be a complete replacement for JOSM rather than a "lightweight map viewer".
>
> This document answers "what exactly are we building, what are we not building, and how do we measure success"; the technical design starts with [01 System Architecture](./01-system-architecture.md).

## 1 Positioning

| Dimension | Decision | Counter-example (explicitly out of scope) |
|---|---|---|
| Form | **Native desktop application**, single-process modular monolith | Electron / Tauri + React client; microservices |
| Language | Rust (stable 1.100, MSRV 1.97) | A thin Rust wrapper around the JVM |
| Rendering | **wgpu GPU-first**, in-house map renderer | Java2D-style CPU rasterization only; drawing the map with egui only |
| Data | In-memory SoA + spatial index, SQLite only for persistence/caching | Querying SQLite every frame; stuffing OSM data into a generic object tree |
| Editing | Command + Transaction + Undo/Redo, single writer | GUI mutating the store directly; multiple threads writing objects concurrently |
| Platform | Native Windows / Linux / macOS | Optimizing for Windows only |
| Scale | 1M–10M+ OSM objects remain interactive | Supporting only small city-level data |

## 2 Goals (verifiable)

1. **Workflow equivalence**: an OSM contributor can spend a full week using only NEO to complete JOSM's daily editing (download → edit → validate → upload → conflict resolution) without going back to JOSM, and without corrupting data. See [13 JOSM Parity](./13-josm-parity.md) for the definition.
2. **Scale**: importing, indexing, panning/zooming, picking, and local editing of tens of millions of objects all stay within the interaction budget (targets in [14 Performance](./14-performance-and-verification.md)).
3. **Structurally better performance than JOSM**: not "faster on some metric", but eliminating structural bottlenecks — GPU batch rendering, draw volume determined by the viewport, incremental geometry/index updates, no JVM heap ceiling.
4. **Maintainable**: business logic does not depend on the GUI; rendering does not depend on UI widgets; the data model does not depend on the render format (ADR-0004).
5. **Extensible**: WASM plugins + a stable host API; styles/presets/validation rules are as compatible as possible with JOSM resources (ADR-0010, [15](./15-plugin-system.md)).

## 3 Non-goals (explicitly out of scope for v1)

| Non-goal | Reason | Alternative |
|---|---|---|
| Binary compatibility with JOSM Java plugins | JOSM plugins link against internal Java classes; cross-language ABI compatibility is impossible | WASM capability mapping ([15](./15-plugin-system.md)) |
| JVM scripting plugins (JSR-223/GraalJS) | Introduces a JVM runtime, contradicting the native positioning | Embedded scripting/WASM, marked P2 |
| OSM API 0.7 | **0.7 is only a wiki concept page with no deployed specification**; JOSM itself hardcodes 0.6 | Implement API 0.6 |
| Browser WASM version | Data scale and filesystem semantics do not match; low payoff | Desktop first; WASM only as a long-term experiment |
| Full offline OSM planet (>100GB) | Not an editor scenario | Regional extracts (PBF subsets) |
| Translating JOSM's Java object model class by class | Would inherit its memory/performance structure | Redesign an SoA store ([03](./03-osm-data-model-and-store.md)) |

## 4 Users and scenarios

- **Everyday contributors**: editing POIs, drawing buildings, fixing roads; need shortcuts, presets, validation, and upload.
- **Data quality workers**: Validator, Overpass/search, batch semantic fixes.
- **Professional mappers**: relations (route/multipolygon), positional accuracy, imagery alignment, WMS/WMTS basemaps.
- **Plugin authors**: need a stable API, events, and a permission model (WASM).
- **Automation/CI users**: headless import, rendering PNGs, validation, diff comparison (JOSM has `render/project/validate` CLIs; NEO provides equivalent capabilities).

## 5 Relationship with JOSM

- Behavioral semantics take JOSM r19678 ("26.09") as the comparison baseline (see [13](./13-josm-parity.md)).
- **No code copying**: clean-room rewrite; specification first (OSM XML/PBF/osmChange/MapCSS/preset XSD are all public formats).
- **Resource isolation**: the presets/MapCSS/validator rules/icons in the JOSM repository are part of a GPL codebase; NEO takes the compliance route of "loading an independent asset directory at runtime + NOTICE/LICENSES" (details and per-file licensing risk in [17 Licensing & Governance](./17-licensing-and-governance.md)).

## 6 Success metrics (gated)

Hard thresholds for Phase 1 (not proceeding to Phase 2 unless met; see G1 in [16 Roadmap](./16-roadmap-and-risks.md)):

| Metric | Target | Current status |
|---|---|---|
| One million objects: import + index | See the measured baseline table in [14](./14-performance-and-verification.md) | Measured on the prototype, see `out/prototype-bench.md` |
| Interactive frames (after viewport culling) | 60 FPS target, P95 scene build < 16 ms | Measured on the prototype |
| Picking | P95 < 16 ms | Measured on the prototype |
| Local editing (move node + related ways + index) | P95 < 1 ms, and does not grow linearly with total object count | Measured on the prototype |
| Ten million objects | Importable, pannable/zoomable, no OOM (31.8 GB reference machine) | To be verified after Phase 1 |

## 7 Terminology

- **SoA**: Structure of Arrays, storing the same field contiguously.
- **Handle**: the internal `(index, generation)` handle; `ObjectId` is the external OSM ID, and the two are separate.
- **Snapshot**: an immutable, read-only view of the data for the renderer/background/plugins to read.
- **Command**: an undoable editing operation.
- **SceneFrame**: one frame's draw batches (vertices/indices/colors) on the CPU side.
- **LOD**: simplifying geometry by zoom/viewport.
- **MapCSS**: JOSM's styling language (a CSS subset + OSM extensions).
- **OSC**: actually refers to `osmChange` (`version="0.6"`); "OSC 0.3" does not exist.
