# 01 · System Architecture

NEO (Next Editor for OSM) is a native Rust rewrite of JOSM, and its core decision is **Rust Native + GPU-first**: a single-process, modular monolith, with editing semantics concentrated in an Editor Layer that can be tested headless, and rendering going through an in-house wgpu 30 pipeline. This document presents the eight-layer conceptual model and the real dependency topology, an overview of threads/GPU queues, the end-to-end sequencing, and the ten invariants that must hold. Machine-checkable dependency rules are in [Workspace and Dependency Rules](./02-workspace-and-dependency-rules.md), editing semantics in [Editing Core and Undo](./06-editing-core-and-undo.md), and threads and resource ownership in [Concurrency Model](./07-concurrency-and-tasks.md).

## 1 Goals and Scope

**In scope**: editable datasets of 1M–10M+ OSM objects; an editing workflow that fully replaces JOSM (download / edit / validate / upload / conflict resolution / plugins); native windows on Windows / Linux / macOS; GPU carrying map drawing and picking; an eight-layer division of responsibility with a machine-checkable dependency direction.

**Out of scope**: a browser-hosted version; multi-process / server-side splitting; multi-user real-time collaborative editing (CRDT / OT); mobile. All three are explicitly rejected; see §5 and [ADR-0002](./adr/0002-modular-monolith.md) for the rationale.

| Dimension | Decision | Rationale |
|---|---|---|
| Process model | Single process, multi-threaded | At 10M objects, only a shared address space allows zero-copy snapshot switching |
| Editing entry point | Unique: `Command` + `Transaction` | Shared prerequisite for undo, logging, OSC, and conflict detection |
| Rendering model | In-house wgpu pipeline; egui handles only the UI layer | 10M objects cannot be drawn primitive by primitive with immediate mode |
| Data ownership | `neo-store` single writer + immutable snapshots | Lock-free read path; background tasks read snapshots only |
| Plugins | WASM (wasmtime 50), feature-gated | Sandboxed, cancellable, no native ABI drift |

**Alignment baseline (three points carried over from JOSM that are most easily misreported)**:

1. OSM API is **0.6 only** (0.7 is just a wiki concept page with no deployed specification); changeset creation is `PUT /api/0.6/changeset/create`, diff upload is a **single** `POST .../changeset/{id}/upload` with an `application/xml` body, and version conflicts return **409**.
2. The actual version of `osmChange` is `version="0.6"`; osmChange/OSC 0.3 **does not exist**.
3. PBF is **core functionality** in JOSM (`OsmPbfFile`), not a plugin; also, JOSM's Edit menu is tiny (Undo/Redo/clipboard/merge/search/preferences), **geometry operations live in the Tools menu** (Align / Distribute / Orthogonalize / Simplify / Reverse / Split / Combine / Unglue), **the Data menu has zero core items** (it is purely a plugin host), and download/upload are in the File menu. When using menus as the migration blueprint, tables must be built accordingly; see [JOSM Parity](./13-josm-parity.md) for details.

## 2 Design

### 2.1 Eight-Layer Conceptual Model

Conceptual layers serve reviews and mental models and are **not** compilation units; the real enforced boundary is the DAG depth in §2.2 (implemented by crate boundaries).

```
L8 Application    apps/neo-desktop · apps/neo-prototype   process/entry/CLI/plugin policy
L7 GUI            neo-ui                                  panels·dialogs·gestures -> EditorIntent (read-only snapshot)
L6 Editor         neo-editor · neo-tools                  Command/Transaction·Undo·Journal·conflicts ★sole writer
L5 Rendering      neo-scene · neo-render                  visible set/LOD -> SceneFrame; wgpu pipeline and GPU picking
L4 OSM Domain     neo-osm · neo-style · neo-validate · neo-query   entities·Tags·MapCSS·Presets·rules·read-only queries
L3 Spatial        neo-geo                                 WebMercator f64 meters·rstar 0.14·geometry construction·hit testing
L2 Data           neo-store · neo-io · neo-persist        SoA·DataSet·reference sets·dirty bitmaps; PBF/XML/OSC; SQLite WAL
L1 Infrastructure neo-core · neo-api · neo-plugin         ID/Handle/Sym/fixed-point Coord/errors; API 0.6; WASM host
```

| Layer | crates | One-line responsibility | Allowed dependencies | Hard prohibitions |
|---|---|---|---|---|
| L8 Application | `apps/neo-desktop`, `apps/neo-cli`, `apps/neo-prototype` | Wiring, event loop, CLI | The explicit list in dependency rules §2.2; desktop does not include query | Business logic written into application crates |
| L7 GUI | `neo-ui` | Panels / dialogs / menus, intent capture | core, osm, store, style, i18n | `io` / `api` / `persist` / render; holding `&mut DataSet` |
| L6 Editor | `neo-editor`, `neo-tools` | Turning intents into validated transactions | core, osm, store, geo; tools additionally editor | render / ui / io |
| L5 Rendering | `neo-scene`, `neo-render` | Snapshot → batches → GPU | scene→{store, geo, style, osm, core}; render→{scene, geo, core} | render depending on store/editor/ui |
| L4 OSM Domain | `neo-osm`, `neo-style`, `neo-validate`, `neo-query` | Semantics, rules, read-only queries | style depends only on core; validate reads store; query additionally reads geo | Any GPU/IO type appearing in signatures |
| L3 Spatial | `neo-geo` | Geometry and indexing | core, osm, store | Holding a mutable `DataSet` |
| L2 Data | `neo-store`, `neo-io`, `neo-persist` | Storage and formats | store/io see §2.2; persist currently has no internal crates | store depending in reverse on any upper layer |
| L1 Infrastructure | `neo-core`, `neo-api`, `neo-plugin` | Base types / networking / sandbox | core has no dependencies; api depends only on core; plugin depends on core+query+editor | Depending on UI or rendering |

Two **documented structural dependencies** may not expand further at review time: ① `neo-validate → store` (read-only rules need the reference-set view); conceptually this is L4, while its compile depth is 3; ② `neo-plugin → query + editor` (plugins may only read snapshots and only return commands), depth 5. Neither `neo-ui` nor `neo-desktop` depends on `neo-query`. The edges are written in [Dependency Rules](./02-workspace-and-dependency-rules.md) §2.2; crossing the line fails CI.

### 2.2 Real Dependency Topology (DAG Depth)

Conceptual layers and compile depth are **ordered differently**. This table is authoritative; any argument of the form "I am L4 so I may depend on L5" is invalid. `depth(c) = 1 + max(depth(dep))`.

| Depth | crates | Notes |
|---|---|---|
| 0 | `neo-core`, `neo-i18n`, `neo-persist` | No internal dependencies. persist currently does not link store |
| 1 | `neo-osm`, `neo-api`, `neo-style` | osm is DTOs; api depends only on core; style only parses styles |
| 2 | `neo-store` | SoA and indexes |
| 3 | `neo-geo`, `neo-io`, `neo-validate`, `neo-ui` | ui reaches only store and style, not render/editor |
| 4 | `neo-scene`, `neo-editor`, `neo-query` | query is used only by plugins |
| 5 | `neo-render`, `neo-tools`, `neo-plugin` | tools produce commands through editor |
| 6 | `neo-prototype`, `neo-cli`, `neo-desktop` | desktop's dependency list is explicit and does not include query |

CI uses `scripts/check-deps.py` to check allowed edges, acyclicity, depth, and `publish = false` (R1, R2, R3, R6; see [02](./02-workspace-and-dependency-rules.md) §2.5). Adding `neo-editor` to `neo-scene` would not create a cycle, but R3 would reject it.

### 2.3 The Editor Layer Is the Core: the GUI Never Mutates Data Directly

**Rule: `&mut DataSet` appears anywhere in the repository only inside a transaction in `neo-editor`.** The only way other layers obtain data content is to read `Arc<MapSnapshot>` (or a view through `MapQuery`). This rule collapses six problems — the undo stack, journaling, OSC generation, conflict detection, the plugin sandbox, and headless testing — onto a single shared precondition.

```
UI gesture ──> EditorIntent { Command(cmd) | Undo | Redo | Select | Download | Upload | Validate }
                  │
                  ▼  IntentSink (non-blocking; when full the UI degrades, never blocking the frame)
            Editor::execute(cmd)
                  │ EditTx::new(&mut dataset)           ← the only place &mut DataSet appears
                  │ validate → apply (records EditOp only) → capture inverse ops
                  │ commit: write journal → incremental index/dirty-bitmap update → version+1
                  ▼
        Arc<MapSnapshot> ──(published via arc-swap)──> rendering / validation / plugins / background tasks (all read-only)
```

| Prohibition | Why | How compile time / CI catches it |
|---|---|---|
| UI holding `&mut DataSet` | Borrow spanning frames → deadlock and intra-frame inconsistency | `neo-ui` has no store write API; `UiContext` holds only `Guard<Arc<MapSnapshot>>` |
| Tools mutating already-committed data | Undo stack and data diverge | `neo-tools` only returns `Box<dyn EditCommand>` |
| Render layer reading the dataset | Frame time is dragged down by the editing path | `neo-render` does not depend on store/editor (`cargo tree` assertion) |
| Background tasks "fixing data along the way" | Version rollback, inexplicable history | Task results carry `base_version`; mismatches are dropped as `Stale` |

### 2.4 Process / Thread / GPU Queue Overview

```
+---------------------- process: neo-desktop (single) ----------------------+
|                                                                          |
|  Main/UI thread                      Rayon compute pool                 |
|  winit events ──> App::ui            +-- Way -> polyline / triangulation  |
|    +-- panels(snapshot.load()) ro    +-- style evaluation / label layout  |
|    +-- Editor::execute (0..n)        +-- neo-validate incremental checks   |
|    +-- SceneFrame diff / batching    +-- neo-io PBF blob decoding          |
|    +-- MapRenderer::prepare/encode   +-- rstar index rebuild (heavy churn) |
|         |                                                                |
|         v  wgpu::Queue (submitted in egui's paint phase; no render thread) |
|     GPU: fill / stroke / point / glyph + pick pass + egui composite       |
|                                                                          |
|  Tokio runtime (multi-thread)         Storage worker (exclusive SQLite conn) |
|  download/upload/OAuth/rate-limit/retry/notes   journal append/autosave/crash-recovery scan |
+--------------------------------------------------------------------------+
```

| Executor | Owns | May do | Prohibited |
|---|---|---|---|
| Main/UI | Single-frame `Guard` of `Editor`, `MapRenderer`, `SnapshotCell` | Input, command execution, scene diffing, encoding, submission | Blocking waits (`poll(wait)`, synchronous HTTP, synchronous disk writes) |
| Rayon pool | No ownership; holds only `Arc<MapSnapshot>` | Pure geometry/style/validation computation, resumable across frames | Touching `&mut DataSet`; posting unbounded messages to the UI |
| wgpu Queue | `Device`/`Queue` created by eframe ([ADR-0005](./adr/0005-wgpu-egui-shared-device.md)) | Writing buffers, submitting encoders, presenting | A second `Instance`; creating a pass or calling `submit` inside `paint` |
| Tokio | HTTP client, cancellation tokens | Download, upload, retry, progress reporting | Writing the dataset directly; blocking the main thread |
| Storage worker | SQLite connection (WAL) | journal / autosave / recovery, serialized writes | Sharing a connection with the editing thread |

**No separate render thread**: egui's `PaintCallback` executes during the paint phase on the UI thread, so map drawing is naturally in the same frame as the UI (picking, hover, and overlays have no 1-frame latency). The seams for splitting it out are concentrated in `MapRenderer::prepare` (works inside the encoder, returns a `CommandBuffer`) and `encode` (pure encoding); if real parallelism is ever needed, moving these two functions to a dedicated thread while keeping the "one submission per frame" contract suffices. **Camera rebasing**: GPU vertices are "f32 meter coordinates relative to the batch origin"; once the camera is more than `REBASE_THRESHOLD_M` (4 km) from the origin it rebases and only updates the 32 B camera uniform.

### 2.5 Single Writer and "Dataset Installation"

Apart from user editing, there are only two structural write paths, and both **exclusively own** a new `DataSet`, installing it in one shot when done:

| Path | Writer | Publication | Enters History |
|---|---|---|---|
| User editing | UI thread `EditTx` | Incremental + `version+1` | Yes |
| PBF/XML import | worker (exclusive new DataSet) | `SnapshotCell::store` wholesale replacement | No (new session) |
| autosave recovery | Storage worker | Same as above | No (history rebuilt from journal) |
| Regional download merge | `neo-editor`'s `MergeCommand` | Goes through the command pipeline + `version+1` | Yes |

Version semantics: `version` strictly increases after **every commit**, and undo/redo increment it too (it never goes back down), so "result and version" always correspond monotonically; background tasks validate with `base_version` and drop results on mismatch instead of patching.

### 2.6 Key Invariants

| Id | Invariant | Implementation | If violated |
|---|---|---|---|
| INV-1 | Single writer: only one thread at any time holds `&mut DataSet` | `Editor` is not `Sync`; write APIs are confined to editor | Data races (the part the compiler cannot catch must rely on type design) |
| INV-2 | Snapshots are immutable: a published `Arc<MapSnapshot>` is never modified | `MapSnapshot` has no `&mut self` methods | Render tearing, index/data inconsistency |
| INV-3 | Rendering is read-only: `neo-render` does not depend on store/editor | Crate whitelist + CI forbidden-edge | Frame time is strangled by the editing path |
| INV-4 | The UI holds no mutable references and no `Guard` across frames | `arc_swap::load()` is scoped to a single draw | Borrow outliving `ui()` → UI locks up |
| INV-5 | All mutations go through Command, with no field writes bypassing `EditTx` | store write APIs are `pub(crate)` + sealed trait (see 02 §3) | Undo stack and data diverge |
| INV-6 | Write-ahead logging: the snapshot is published only after the transaction is persisted (or goes into the in-memory journal) | In `Editor::commit`, the journal comes before publish | Data and history diverge after a crash |
| INV-7 | Undo produces a **new** version; it does not roll the version number back | `version = version + 1` increments unconditionally | Conclusions based on an old version are misused |
| INV-8 | Handles carry a generation; expired handles return `Err` | `Handle { index, generation }`, `generation+1` on release | Dangling references / ghost objects |
| INV-9 | The UI thread never performs blocking waits | Code review + `pollster::block_on` limited to startup paths | Dropped frames, lost input |
| INV-10 | Lock order: store → geo cache → scene, never reversed; do not post UI messages while holding a lock | `parking_lot` wrappers + review checklist | Deadlocks (checklist in 07 §2.5) |

## 3 Key Interfaces

These are the cross-layer contract signatures. `neo-core`/`neo-osm`/`neo-store`/`neo-geo`/`neo-query` are already implemented in the skeleton code; the `lib.rs` of `neo-scene`/`neo-render`/`neo-editor`/`neo-ui` currently contains placeholder comments, and signatures align with this table (differences are synchronized in the corresponding PR).

```rust
// crates/neo-core/src/ids.rs —— typed IDs; bare i64 must not cross crates
pub struct ObjectId(pub i64);                  // <0 locally created / 0 empty / >0 already exists on the server
pub enum ObjectKind { Node, Way, Relation, Changeset }

// crates/neo-core/src/handle.rs —— slotted handle with generation (independent of whether T is Send)
pub struct Handle<T> { /* index: u32, generation: u32 */ }
impl<T> Handle<T> {
    pub const fn new(index: u32, generation: u32) -> Self;
    pub const fn index(self) -> u32;
    pub const fn generation(self) -> u32;
}

// crates/neo-core/src/units.rs —— fixed-point coordinates, 1e-7 degrees (OSM API precision)
pub struct Coord(pub i32);
pub struct LatLon { pub lat: Coord, pub lon: Coord }
// crates/neo-store/src/dataset.rs —— the single source of truth owned by the single writer
pub struct DataSet { /* SoA + IdIndex + TagStore + ChangeTrack + dirty bitmaps */ }
impl DataSet {
    pub fn new() -> Self;
    pub fn version(&self) -> u64;
    pub fn push_node(&mut self, node: Node) -> Handle<NodeSlot>;
    pub fn push_way(&mut self, way: Way, policy: MissingPolicy) -> Result<Option<Handle<WaySlot>>>;
    pub fn push_relation(&mut self, r: Relation, policy: MissingPolicy) -> Result<Option<Handle<RelationSlot>>>;
    pub fn node_view(&self, h: Handle<NodeSlot>) -> Option<NodeView<'_>>;
    pub fn set_node_position(&mut self, h: Handle<NodeSlot>, pos: LatLon) -> Result<LatLon>;
    pub fn take_dirty_ways(&mut self) -> Vec<Handle<WaySlot>>;
}

// crates/neo-query/src/lib.rs —— read-only exit point for plugins/validators/background tasks/UI (no write interface)
pub trait MapQuery {
    fn data(&self) -> &DataSet;
    fn node(&self, id: ObjectId) -> Option<Node>;
    fn way(&self, id: ObjectId) -> Option<Way>;
    fn relation(&self, id: ObjectId) -> Option<Relation>;
    fn parent_ways(&self, node: ObjectId) -> Vec<ObjectId>;
    fn parent_relations(&self, kind: ObjectKind, id: ObjectId) -> Vec<ObjectId>;
    fn nodes_in(&self, bounds: WorldBounds) -> Vec<ObjectId>;
    fn ways_in(&self, bounds: WorldBounds) -> Vec<ObjectId>;
    fn nearest_node(&self, point: WorldPoint, max_meters: f64) -> Option<(ObjectId, f64)>;
}
pub struct SnapshotCell { /* arc_swap::ArcSwap<MapSnapshot> */ }
impl SnapshotCell {
    pub fn load(&self) -> arc_swap::Guard<Arc<MapSnapshot>>;   // valid for one frame; must not be kept across frames
    pub fn store(&self, snapshot: MapSnapshot);                // the only publication point
}

// crates/neo-editor/src/lib.rs —— the only write entry point (target signatures)
pub struct Editor { /* DataSet + History + Journal + Selection */ }
impl Editor {
    pub fn execute(&mut self, cmd: Box<dyn EditCommand>) -> Result<CommitReport, EditError>;
    pub fn undo(&mut self) -> Result<CommitReport, EditError>;
    pub fn redo(&mut self) -> Result<CommitReport, EditError>;
    pub fn snapshot(&self) -> Arc<MapSnapshot>;
    pub fn publish_if_dirty(&mut self) -> Option<Arc<MapSnapshot>>;
}

// crates/neo-ui/src/bridge.rs —— the UI only expresses intents
pub enum EditorIntent {
    Command(Box<dyn EditCommand>), Undo, Redo, Select(SelectionOp),
    Download(DownloadRequest), Upload(UploadRequest), Validate(ValidateRequest),
}
pub trait IntentSink: Send + Sync {
    fn submit(&self, intent: EditorIntent) -> IntentId;
    fn try_submit(&self, intent: EditorIntent) -> Result<IntentId, Full>;
}

// crates/neo-geo/src/camera.rs —— vertices are "f32 meter coordinates relative to the batch origin"
pub const REBASE_THRESHOLD_M: f64;   // 4 km; beyond this, rebase and update only the 32 B uniform

// crates/neo-scene/src/frame.rs —— the only input shape for the render layer (no GPU handles, no editor dependency)
pub struct ViewParams { pub center: WorldPoint, pub zoom: f64, pub viewport_px: [u32; 2], pub dpr: f32 }
pub struct SceneFrame { pub version: u64, pub batches: Vec<Batch>, pub labels: Vec<LabelItem> }
pub fn build_scene(
    data: &DataSet,
    index: Option<&SpatialIndex>,
    style: &StyleSheet,
    view: &ViewParams,
) -> SceneFrame;

// crates/neo-render/src/pass.rs —— no store/editor types appear here
pub struct MapRenderer { /* Device/Queue passed in from outside (ADR-0005) */ }
impl MapRenderer {
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &SceneFrame) -> PrepareStats;
    pub fn encode(&mut self, pass: &mut wgpu::RenderPass<'_>);
    pub fn resolve_pick(&self, cursor: [f32; 2]) -> Option<ObjectId>;
}
```

## 4 Data Flow and Sequencing

**Steady-state frame loop**: `input -> egui::Context::run -> App::ui`, executing in order `panels(snapshot.load())` (read-only, `Guard` does not leave its scope), `intents.drain() -> Editor::execute`, `build_scene` (when dirty), `MapRenderer::prepare/encode`, and finally submission via egui-wgpu's paint callback. Each frame has a hard budget (a design target, not a measured value): UI logic + scene build (resumable across frames) + upload + submit; when over budget, follow [07](./07-concurrency-and-tasks.md) §2.6 to drop LOD or defer upload by one frame — "catching up frames" must never block input.

### 4.1 End-to-End: Open PBF → Edit → Upload

```
User      neo-ui       neo-io        neo-store/geo     worker        neo-render      neo-api
 |          |             |                |               |              |              |
 | Ctrl+O ->| OpenFile    |                |               |              |              |
 |          |------------>| (spawn)        |               |              |              |
 |          |             |--osmpbf------->|               |              |              |
 |          |             |  blob decode   | SoA + index    |              |              |
 |          |             |<---------------| publish v=1    |              |              |
 |          |<------------| ImportReport   |               |              |              |
 |<-first   |--build_scene(v=1)--------------------------->| prepare/encode|              |
 | drag --->| MoveCommand (intent)           |               |              |              |
 |          |--> Editor::execute: validate->apply->inverse->journal->publish v=2 |          |
 |<-repaint |--build_scene(v=2) (dirty ways only)---------->| dirty range upload |           |
 | Ctrl+U ->| UploadRequest                  |               |              |              |
 |          |------------>| OSC 0.6 generation (<osmChange version="0.6">)------->| PUT changeset|
 |          |             |                |               |              | POST upload  |
 |          |<------------| ChangeSet      |               |              | 200 / 409    |
 |          |  409 --> ConflictQueue --> ConflictDialog --> ResolveCommand --> resend         |
```

| Stage | Executor | Artifact | Failure semantics |
|---|---|---|---|
| Open | worker + `neo-io` | `MapSnapshot(v=1)` installed | Corrupt file → notify and keep the old dataset, do not overwrite |
| Edit | UI thread `neo-editor` | `Snapshot(v+1)` + journal transaction | Validation failure → transaction not committed, UI shows the reason |
| Scene | UI thread (resumable across frames) + Rayon geometry | `SceneFrame(v)` | Over budget → lower LOD (without blocking the UI) |
| Upload | Tokio + `neo-api` | changeset id or a 409 conflict set | Conflict → enqueued for the user to resolve; network failure → exponential backoff retry |

> Upload failure semantics must be precise: a request that was never sent can safely be retried; one that was sent but timed out is "unknown state", so `GET` the version first to verify; 409 (version updated by someone else) rolls back the whole request and produces a conflict report; 412 (missing reference) requires a supplementary download, or `if-unused` for deletions. The full state machine is in [OSM API and Sync](./09-osm-api-and-sync.md) §2.5.

## 5 Tradeoffs and Alternatives

| Rejected option | Reason for rejection | What it costs us |
|---|---|---|
| Microservices / multi-process (core daemon + UI client) | At 10M objects, cross-process requires serialization or a shared-memory protocol; per-frame picking and visible-set rebuild both go over IPC, making latency uncontrollable; sharing a GPU context across processes needs platform-specific code | No independent crash domain; plugin isolation can only rely on the WASM sandbox rather than process boundaries |
| Tauri + React (WebView front end) | A WebView cannot host an in-house wgpu pipeline (it requires embedding an independent window handle, and the three platforms differ); a primitive list of 10M objects over an IPC/JS bridge is infeasible | We give up the Web ecosystem's widget richness and existing front-end talent |
| Qt / GTK (C++ bindings such as `qmetaobject`/`gtk-rs`) | Qt's dual licensing (LGPL/commercial) and static-linking compliance cost; C++ object lifetimes cross Rust's borrow checker, making the FFI layer a long-term bug source; GTK's look and IME support on Windows lag noticeably | We give up mature multi-window docking, accessibility, and long-term IME polish (accessibility relies on accesskit, whose coverage is weaker than Qt's) |
| Reusing JOSM code directly (Java interop / transpilation) | JOSM is GPL-2.0+, so static linking or rewriting is license-contagious; JVM startup and heap model are unsuitable for GPU-first | Semantic compatibility can only come from behavioral alignment (see risk R2 in [16](./16-roadmap-and-risks.md)) |

## 6 Risks

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| A1 | The conceptual eight layers and the DAG depth are ordered differently, and this is misread as "depending upward is allowed" | Circular dependencies; impossible to split at compile time | §2.2 table takes precedence; CI forbidden-edge assertions |
| A2 | `neo-ui` obtains a store write path indirectly through `neo-tools` | INV-5 is bypassed | Write APIs are `pub(crate)`; tools only return `Box<dyn EditCommand>` |
| A3 | Single process has no crash domain; a plugin/driver crash loses all unsaved edits | Data loss | journal + autosave (10/07) + WASM sandbox resource limits |

## 7 Acceptance Criteria

1. The dependency graph exported by `cargo metadata` matches the §2.2 table edge by edge (`scripts/check-deps.py` exits 0), and negative tests for at least 3 forbidden edges pass.
2. `cargo tree -p neo-render` does not contain `neo-store`/`neo-editor`/`neo-ui`; `cargo tree -p neo-ui` does not contain `neo-io`/`neo-api`/`neo-persist`.
3. A single interaction dragging a map node produces **exactly one** history entry, the `Snapshot` version increments by exactly 1, the journal has one complete transaction, and the geometry dirty set contains only the moved node and the ways that contain it.
4. During a 1M-object import, the UI thread's longest block is [pending measurement, see docs/14-performance-and-verification.md], and no input is lost (record input latency p99 over 60 s).
5. When an upload encounters 1 conflict, the UI receives a `ConflictQueue` entry, and resolving them one by one allows a successful retry (integration test `tests/upload_conflict.rs`).
6. After a forced process kill (SIGKILL) and restart, journal replay recovers to the last committed transaction, and uncommitted transactions do not appear.

## 8 References

- JOSM project home page and source (GPL-2.0+, behavioral alignment reference only, no code copying): https://josm.openstreetmap.de/
- JOSM menu baseline (actual items in File/Edit/Tools/Data): https://josm.openstreetmap.de/wiki/Help/Menu
- OSM API v0.6 (changeset, upload, 409/412 semantics): https://wiki.openstreetmap.org/wiki/API_v0.6
- OsmChange format (`version="0.6"`): https://wiki.openstreetmap.org/wiki/OsmChange
- OSM PBF format: https://wiki.openstreetmap.org/wiki/PBF_Format
- egui / eframe / egui-wgpu 0.36.2: https://docs.rs/egui/0.36.2/egui/
- wgpu 30.0.2: https://docs.rs/wgpu/30.0.2/wgpu/
- arc-swap (`load`/`load_full`/`Guard` semantics): https://docs.rs/arc-swap/1.9.5/arc_swap/
- rstar 0.14 (`RTreeObject`/`PointDistance`): https://docs.rs/rstar/0.14/rstar/
- JOSM license (GPL-2.0+): https://josm.openstreetmap.de/wiki/Source
- Version verification records (including source and dependency JSON evidence): `.research/crate-api-cheatsheet.md`

> To be verified: `accesskit`'s accessibility coverage on the three platforms; the `App` entry point of `eframe 0.36.2` is `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame)`, and `update()` was removed in 0.36 (verified from the crate source; see [ADR-0005](./adr/0005-wgpu-egui-shared-device.md)). Both items need to be filled back into this section after the first `cargo build`.
