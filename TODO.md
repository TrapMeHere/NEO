# NEO Development Progress Ledger

> This file is the **live board for the project lead**: current phase, done/in-progress/not-started, acceptance status, next actions.
> Rule: update this file + `git commit` before ending each round; status markers: `[x]` done, `[~]` in progress, `[ ]` not started, `[!]` blocked.
> Architecture contracts: [docs/README.md](docs/README.md); roadmap and gates: [docs/16-roadmap-and-risks.md](docs/16-roadmap-and-risks.md).

## 0 Overview

| Item | Status |
|---|---|
| Current phase | **Phases 5–6 are implemented in code; gates are not all passed**. G5 = Conditional Go (plugin isolation passed; three-platform installer cold start not done). Phase 6 is not RC. See [gate-5](docs/prototypes/gate-5-ecosystem.md), [gate-6](docs/prototypes/gate-6-performance.md) |
| Last gate | G5 = **Conditional Go**; Phase 6 = **not RC**. G1's three-platform check and 10-minute stability are still outstanding |
| Target gate | Finish G5(d) three-platform installer cold start; re-measure 10M and the 1-hour session on the new streaming path |
| Code size | Rust ~19k lines (97 source files) |
| Tests | **master has restored regression tests** (core/store/editor/io/persist/style/ui, 40+ items); after the 2026-11-21 health-check fixes `cargo test --workspace` passes |
| JOSM parity P0 | **~20 of 52 achieved/partially achieved**; 2026-11-21 added relation member editing/box select/copy-paste/find/command history/GPX/simplify/glue. Real gaps in [docs/feature-parity.md](docs/feature-parity.md) |

## 1 This round (Phase 2 → 3) task board

### 1.1 Phase 2 · OSM data core (G2)

| # | Task | Status | Crate | Acceptance |
|---|---|---|---|---|
| 2.1 | OSM XML 0.6 read (node/way/relation/tag/nd/member, negative ids, action/visible) | [x] | `neo-io/xml.rs` | Unit tests + Monaco/Bremen XML round-trip |
| 2.2 | OSM XML write (server-conformant / JOSM-conformant modes) | [x] | `neo-io/xml.rs` | Round-trip semantic equivalence tests |
| 2.3 | osmChange 0.6 read (create/modify/delete, if-unused, negative-id placeholders) | [x] | `neo-io/osc.rs` | create/modify/delete parse + apply |
| 2.4 | PBF write encoder (varint + zlib, complementing read-only osmpbf) | [x] | `neo-io/pbf_write.rs` | write → read back through osmpbf with identical semantics |
| 2.5 | Referential integrity checks (dangling node/member, explicit incomplete) | [x] | `neo-store` / `neo-io` | 0 errors reported or incomplete listed |
| 2.6 | Cancellable import, no half-built state on failure (replace after the build completes) | [x] | `neo-io` | Cancellation tests + atomicity tests |
| 2.7 | 10M PBF write→read comparison (counts/integrity, measured on real hardware) | [x] | `neo-prototype` | Baseline recorded in docs/14 |
| 2.8 | Parallel PBF decode (blob-level rayon; Bremen 1.29× / Monaco 1.11×, 3× target not met) | [x] | `neo-io` | Throughput comparison (3× target) |
| 2.9 | `.jos` session read/write (XML; `.joz` = zip container) | [x] | `neo-io`/`neo-persist` | Round-trip tests |
| 2.10 | GPX read/write (P1) | [x] | `neo-io` | Round-trip unit tests (added in Phase 5) |

### 1.2 Phase 3 · Basic editing (G3)

| # | Task | Status | Crate | Acceptance |
|---|---|---|---|---|
| 3.1 | Editor commit pipeline (precheck → apply → undo record → dirty → version bump) | [x] | `neo-editor` | Unit tests |
| 3.2 | EditTx/Transaction atomicity (rollback on failure, zero side effects) | [x] | `neo-editor` | Property tests |
| 3.3 | History (undo/redo, drag coalescing, memory limit) | [x] | `neo-editor` | `undo∘redo = id` tests |
| 3.4 | Selection (node/way/relation sets, toggle/clear) | [x] | `neo-editor` | Unit tests |
| 3.5 | Journal (JSONL frames, TxBegin/Commit, replay drops half transactions) | [x] | `neo-editor` | Truncated-file replay tests |
| 3.6 | Basic commands: AddNode/AddWay/MoveNode/SetTags/Delete | [x] | `neo-editor/commands.rs` | apply+undo unit tests per command |
| 3.7 | Geometry commands: SplitWay/MergeWay/ReverseWay (incl. direction tag fixup) | [x] | `neo-editor/commands.rs` | Semantic unit tests |
| 3.8 | Index/dirty-set sync after edits (move node → parent way bbox/index) | [x] | `neo-editor` | Incremental tests |
| 3.9 | Tool set: select/draw/delete (tool state machine → commands) | [x] | `neo-tools` | Unit tests |
| 3.10 | Tag editing panel (add/remove/change tag → SetTags command) | [x] | `neo-ui` | Manual verification |
| 3.11 | Desktop integration: mode switching, shortcuts, drag-move, undo/redo, save, journal recovery | [x] | `apps/neo-desktop` | Manual smoke |
| 3.12 | One drag = 1 history entry (preview not in history; single command on release) | [x] | `neo-editor`/`neo-desktop` | Unit tests + manual |
| 3.13 | Save/export: OSM XML / osmChange (PBF export wired in after 2.4) | [x] | `neo-io` | Reopen after save is consistent |
| 3.14 | Automated acceptance scripts (random edits + undo/redo consistency, zero side effects on transaction failure, OSC write→read→write semantic equivalence) | [x] | `crates/neo-editor/tests/`, `crates/neo-io/tests/` | Runnable in CI |

### 1.3 Known gaps (not covered this round, logged)

- [ ] Relation editing (add/remove/reorder relation members), multipolygon assembly and validation
- [ ] Conflict resolution (six MergeCase kinds), upload 409 handling → Phase 4
- [ ] MapCSS/Presets real styling → Phase 4
- [ ] Validator 43 rules → Phase 4
- [ ] SQLite persistence / proper autosave → Phase 4 (this round's Journal uses JSONL files)
- [ ] PBF parallel decode and write-side performance optimization
- [ ] Three-platform CI, Windows installer/signing, DX12 backend measurement
- [ ] 10-minute stability smoke

## 2 Current implementation inventory (code map)

| crate | Status | Main files |
|---|---|---|
| `neo-core` | ✅ done | ids/handle/units/sym/meta/error/stats |
| `neo-osm` | ✅ done | tags/element/change |
| `neo-store` | ✅ done (missing compact/COW) | node/way/relation/tag arena/indexes/bitset/dataset |
| `neo-geo` | ✅ done (missing multi-projection/relation bbox) | world/camera/spatial/geometry/picking/world_cache |
| `neo-io` | 🚧 this round | pbf (read)/xml (this round)/osc (this round)/synth |
| `neo-editor` | 🚧 this round | command/tx/history/selection/journal/commands |
| `neo-tools` | 🚧 this round | select/draw/delete controllers |
| `neo-ui` | 🚧 this round | toolbar/layers/properties + tag editor |
| `neo-scene` / `neo-render` | ✅ Phase 1 | batches/pipelines/offscreen render/MapGpu |
| `neo-query` | ✅ read-only queries | MapQuery/Snapshot |
| `neo-api` / `neo-persist` / `neo-validate` / `neo-plugin` | ✅ implemented (confirmed by the 2026-11 health check; no longer "3-line placeholders") | see each crate |
| `apps/neo-desktop` | 🚧 this round | editing integration |
| `apps/neo-prototype` | ✅ | import/render/benchmark |

## 3 Verification method (run at the end of every round)

```powershell
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
# Other check scripts: see the README "Development" section
```

## 4 Risks and decisions (this round)

| Decision | Rationale |
|---|---|
| Journal uses JSONL (serde_json) rather than rkyv | Human-readable, truncation-recoverable, no unsafe; fast enough (editing is infrequent) |
| Deleted objects stay in the store marked `ChangeState::Deleted` | Upload diff/undo/save round-trip all need "deleted objects remain visible in the changeset" |
| Relation editing/conflict resolution deferred to Phase 4 | G3 only requires "editable for our own use"; only G4 requires the upload loop |
| Tool state machine lives in `neo-tools` rather than desktop | Unit-testable and reusable by a future CLI/scripts |

## 5 Next steps (next round)

1. Finish 2.1/2.2 XML round-trip + 2.3 osmChange read
2. Finish 3.1–3.7 commands and the History/Journal tests
3. Desktop end-to-end: draw a way → change a tag → drag a node → undo/redo → save XML → reopen with an identical result
4. Run the G2/G3 automated acceptance scripts and backfill this file

---

Last updated: 2026-11-16 · Maintainer: NEO Lead Agent


## 6 Changelog (new → old)

### 2026-11-16 · Phases 5–6

- [x] `neo-plugin`: wasmtime 50 host, three plugin semantics, capability intersection, fuel / 64 MiB limits. `cargo test -p neo-plugin --features wasm-plugins` passes 11 items
- [x] GPX read/write, MapCSS warnings and style declaration editing, TMS/WMS/offset, preset group override table (measured count > 0 for all 22 groups)
- [x] CLI, desktop ecosystem window, three-platform packaging scripts (scripts not executed)
- [x] Streaming LOD, bounded tile cache, parallel scene build, coordinate delta varint, indirect draw + GPU instance-count clearing (64×64 triangles pixel-identical, then all black)
- [x] Presets switched to npm: `@openstreetmap/id-tagging-schema` 7.3.0 (1678 entries) and `name-suggestion-index` 8.0.20261101 (40778 brand suggestions). Generated into `assets/presets/`, loaded at runtime, not baked into the exe
- [x] Fonts externalized to `assets/fonts/`; `cargo bloat` baseline in `docs/prototypes/binary-size-baseline.md`
- [ ] Not done: three-platform installer cold start, new 10M path, 1-hour session, Ed25519, release profiling

### 2026-11-15 · Phases 2/3, first cut (in progress)
- [x] `neo-io/xml.rs`: OSM XML 0.6 streaming read (node/way/relation/tag/nd/member, negative ids, visible/action, entity decoding) + two write modes (Server/JOSM-conformant), 6 unit tests (incl. round-trip semantic equivalence)
- [x] `neo-editor`: single-writer `Editor`/`EditContext` transactions, `Command` contract, `History` (command batches + drag coalescing), `Selection`, `Journal` (JSONL frames + `committed_prefix` + `recover` replay)
- [x] `neo-editor/commands.rs`: AddNode / AddWay / MoveNode / SetTags / SetWayNodes / DeleteObject (referential checks), apply+undo per command; tests for `undo∘redo = id`, zero side effects on transaction failure, drag coalescing, journal replay
- [x] `neo-store`: `set_change_state_visible` (deleted objects stay in the store for upload diff/undo)
- [ ] Next round: osmChange read, PBF write encoder, neo-tools controllers, neo-ui tag panel, desktop editing wiring (draw point/line/drag/undo/save), G2/G3 acceptance scripts


### 2026-11-15 · Phases 2/3, second cut (this round)
- [x] `neo-io/osc_read.rs`: osmChange 0.6 **read and apply** (create/modify/delete, negative ids, `visible`, missing-object counts), 3 unit tests
- [x] Fixed the **coordinate parsing bug** in `neo-io/xml.rs` (XML/OSC lat/lon are decimal degrees; the original implementation parsed them as 1e-7 integers, which silently became 0)
- [x] `neo-editor/commands_geom.rs`: SplitWay / MergeWay (automatic normalization of four orientations) / ReverseWay (`oneway=yes ↔ -1` direction tag fixup), 6 unit tests
- [x] `neo-editor` fixed **redo local-ID stability** (AddNode/AddWay redo reuses the ID assigned the first time; otherwise undo→redo changes the ID)
- [x] `neo-tools`: select/draw/delete tool state machine (snap to existing nodes, draw a way point by point, delete referential checks, drag as a single history entry), 5 unit tests
- [x] `neo-ui`: **tag editing panel** (add/remove/change tag → `apply_tags` intent), toolbar (tool switch/undo/redo/open PBF+XML/save XML/export OSC/sample data/finish line), property panel extensions (way split/reverse buttons)
- [x] `apps/neo-desktop`: editing loop — Editor integration, S/A/X shortcuts, Ctrl+Z/Y/S, Enter, Delete, **drag-move nodes (preview not in history; release commits a single MoveNode)**, tag editing, save XML/export OSC, **journal recovery on startup + manual recovery button**
- [x] G3 acceptance integration test `crates/neo-editor/tests/g3_acceptance.rs`: 1000 random edits + random undo/redo stay consistent with a "replay from scratch" state; zero side effects on transaction failure; trailing half transaction in the journal is dropped
- [x] Desktop smoke: `neo-desktop.exe` survives 10s after launch (loads Monaco, creates journal, wgpu first frame)
- [x] G2 acceptance addition: `crates/neo-io/tests/osc_roundtrip.rs` (write→read→write, action grouping semantically equivalent, incl. negative IDs/deleted objects)
- [x] `neo-tools` gained `merge_ways` (automatic endpoint-adjacency normalization + failure message)
### 2026-11-15 · Phase 2 wrap-up (this round)
- [x] **PBF write encoder** (`neo-io/pbf_write.rs`): protobuf wire encoding + zlib, nodes/ways/relations, string table, sint64 delta refs; key pitfall: `osmformat.Node.id` is **sint64 (zigzag)** (Way/Relation.id is int64); a mistake makes read-back IDs negative and leaves every way dangling
- [x] **G2 acceptance**: `crates/neo-io/tests/pbf_roundtrip.rs` (write→read back with osmpbf: counts/coordinates/way order/relation member order and roles/tags/meta all equal) + compressed/uncompressed/empty datasets
- [x] **Referential integrity checks** (`neo-store/integrity.rs`): way→node, relation→member dangling detection + report; 2 unit tests
- [x] **Import cancellation semantics** (`ImportOptions.cancel`, checked every 4096 elements; cancellation returns `NeoError::Cancelled` with no half-built state): 2 tests in `tests/import_cancel.rs`
- [x] Desktop "Export PBF", prototype `pbf-roundtrip` subcommand (write→re-read→counts/integrity check)
- [ ] Next round: 10M comparison (2.7), parallel PBF decode (2.8), `.jos/.joz` (2.9); relation editing/multi-select/merge UI


### 2026-11-15 · Phase 2 parallel import + 10M comparison (this round)
- [x] **Parallel PBF decode** (`neo-io/pbf_parallel.rs`): BlobReader reads blobs sequentially → rayon decodes zlib+protobuf in parallel into Raw DTOs → a single thread assembles Node→Way→Relation; supports cancellation and reference policy
- [x] Real-hardware benchmark (release, i5-12400F / RTX 3070 / Windows 11):
  - Bremen (1,668,778 N / 329,676 W / 5,053 R, 20.3 MiB): sequential 1.163s → parallel **0.899s (1.29×)**
  - Monaco: sequential 0.028s → parallel 0.026s (1.11×)
  - Object-by-object identical to sequential import (counts/ID sets/way reference order/relations); **3× target not met**, the bottleneck is single-thread assembly (string interning + ID index insertion), recorded in docs/14
- [x] **10M synthetic PBF write→read comparison**: 10,001,406 nodes / 3,129,589 ways / 78,210 relations → 86.65 MiB PBF (2,904 blobs) → re-read counts/node references/members/tags all consistent, `check_integrity` 0 issues, memory 1,532 MiB
- [x] Prototype gained `compare-import` (sequential vs parallel) and `pbf-roundtrip --synth <n>` subcommands
- [ ] Next round: `.jos/.joz` session files (2.9), relation editing/multi-select/box select, merge UI wiring; parallel assembly (partitioned DataSet) as a follow-up optimization


### 2026-11-15 · Session files + multi-select/relation editing (Phase 2/3 close-out)
- [x] **`.jos` / `.joz` sessions** (`neo-persist/session.rs`): JOSM session XML (viewport/layer refs/active-layer/selection); `.joz` is a ZIP (session.jos + embedded layer data; unzip on read and rewrite the paths); **tolerates unknown plugin layer sections/attributes**; 3 unit tests (round-trip/unknown section/zip embedded unzip)
- [x] Desktop "Save session / Open session" (`.jos` vs `.joz` chosen by extension; restores the camera viewport and selection)
- [x] **Multi-select**: Shift+click extends the selection; batch delete (one command each; failures do not abort and the success count is reported)
- [x] **Merge UI**: with two endpoint-adjacent ways selected, a "Merge the two selected ways" button appears (`MergeWay`)
- [x] **Basic relation editing**: property panel shows relation member count/version/tags editable; deleting a relation goes through `DeleteObject` (referential checks)
- [x] Tests: `shift_click_extends_selection`, 3 session items; 111 tests pass repo-wide
- [ ] Follow-up (Phase 4+, not this goal): GPX, validator, MapCSS, presets, upload/conflicts, SQLite autosave, box select, way node batch drag


### 2026-11-15 · Mock UI manual testing (screenshot verification + 4 real bug fixes)
End-to-end UI checks on the release desktop build using PowerShell + Win32 simulated input (`scripts/ui_check.ps1`); screenshots saved under `out/ui-check/`.
**Bugs found and fixed**:
- [x] **All Chinese rendered as tofu boxes**: egui's default font lacks CJK → load system fonts (`simhei.ttf` etc.) at startup; mixed Chinese/English now renders correctly
- [x] **Operation feedback invisible**: `status` messages never reached the toolbar/left panel → `status_line()` appends the latest action ("undone/redone/start point placed…")
- [x] **UI lagged one step behind**: actions executed after the panels were drawn and did not request a repaint → undo/redo only refreshed on the next input; added `request_repaint()`
- [x] **Initial view pulled to z11 by sparse out-of-bounds ways** (Monaco only has a dense road area within 0.5° plus a few cross-border lines) → switched to a 2%–98% percentile fit, initial z15.1
**Interactions verified** (screenshot evidence): startup loads Monaco (41844/6281/222), draw tool A + click to create a way + Enter to finish, Ctrl+Z×3 / Ctrl+Y×3 undo/redo counts and status correct, S selects a node then the right-side property + tag panel, deleting a node referenced by a way cascades node removal (2-node ways require a second confirm and then delete the whole way), mouse-wheel zoom (15.49→15.82→15.49), toolbar "Sample data" button works
**Not covered (needs manual work/GUI automation)**: native file dialogs (save/open XML/OSC/PBF/session), Shift multi-select and the merge button, checkbox clicks (coordinates not aligned)


### 2026-11-15 · Rendering stutter fix (50× fewer rebuilds/uploads)
**User report**: "rendering too slow" while dragging/zooming. Measured with `scripts/ui_check.ps1` + `NEO_PERF_LOG` instrumentation (`out/ui-check/perf-*.log`):

| Scenario (synth:200000, dragging a node ~1s) | Before | After |
|---|---:|---:|
| Scene rebuilds (scene build) | 78 (once per frame, 14–15 ms each) | **2** (startup + release commit, ~15 ms each) |
| Full GPU uploads (1.86 MB each) | 78 | **2** |
| Slow frames (>8 ms) | all frames | **2 frames** |

**Root cause**: while dragging a node, `preview_move_node()` wrote to the DataSet on every mousemove → `version` was bumped → `ensure_scene`
rebuilt the whole scene + `GpuScene` re-uploaded everything (Monaco 3.9 MB / synth 200k 1.9 MB), i.e. 10–20 ms per frame.
**Fix**:
- Decouple drag preview from the DataSet: keep only `DragState.current` and draw "neighbor links + orange target point" with an egui overlay (`Editor::node_neighbors`);
- On mouse release execute `MoveNode` exactly once (one rebuild, one history entry);
- `MoveNode` now marks the node `Modified` (fixes "dirty objects = 0" and the upload diff missing objects);
- Also fixed: on double-click launch the CWD was not the repo root → Monaco was not found and it fell back to synth:200000; changed to an exe-relative path lookup;
- Added `NEO_SYNTH_N` (force synthetic data) and `NEO_PERF_LOG` (in-app performance log, avoiding shell redirection hangs);
- `scripts/ui_check.ps1` gained drag / truepan / zoomloop steps and screenshots.

Measured after the fix on Monaco: the whole "startup + drag + pan" sequence has only 5 rebuilds/5 uploads and 1 slow frame (8.6 ms at startup).


### 2026-11-15 · Mouse controls changed to "left button selects/drags elements, right button drags the background"
- Left click = select; left-drag node = move (release commits 1 history entry); left-drag empty space = no-op (reserved for future box select)
- Right-drag = pan the map; wheel = zoom at the mouse position without shifting
- Left shortcut panel and README updated in sync
- Automated verification (scripts/ui_check.ps1): drag (left-drag node, undo 1/dirty objects 1) → panright (right-drag, view pans with no edits) → emptyleft (left-drag empty space, screenshot pixel-identical to after the pan = no side effects)


### 2026-11-15 · Node rendering LOD (removed the toggle, now zoom tiers + shape distinction)
- Removed the toolbar "Show nodes" checkbox and `SceneConfig.node_zoom_threshold`; node display is now decided by zoom
- `neo-scene`: `NODE_MIN_ZOOM=13`, `NODE_ALL_ZOOM=16`, `SHARED_NODE_MIN_WAYS=2`;
  `SceneBuilder::build` gained a `&Referrers` parameter (shared-node detection)
- Shapes: regular node = small dot (hexagon approximation); shared nodes at z13–15 = small dot, z16+ = small square
- Verified (Monaco, 44-field equality tests + measured statistics):
  - z12 `nodes=0`; z13/14/15 draw only shared nodes (9135 each measured); z16 draws all visible nodes (29107)
  - Unit test `node_lod_follows_zoom` checks the three tiers' counts and the Point batch vertex count = shared×4 + regular×7
- Screenshots: `out/ui-check/21-z13-shared-nodes.png` (toolbar no longer has the checkbox), `22-z16-all-nodes.png`
- Added the `NEO_START_ZOOM` environment variable (initial zoom for screenshots/tests)


### 2026-11-15 · Fixed "map goes blank after zooming to z19 and back out"
- Root cause: the scene rebuild condition did not include zoom changes → after zooming out it still rendered batches clipped at the high zoom; and the GPU upload key was only (data version, origin),
  so a rebuild caused by zoom did not re-upload
- Fix: `ensure_scene` rebuilds when `|Δzoom| > 0.5`; `GpuScene` upload key gained camera.zoom
- Regression: `scripts/ui_check.ps1 zoomcycle` (z16→z19+→z13→z16), screenshots 23/24/25; only 10 rebuilds/uploads throughout, final image correct
- Related docs: docs/14 §13.1


### 2026-11-15 · Node LOD threshold adjustment (not shown at z16 and below)
- `NODE_MIN_ZOOM` 13 → **17**; `NODE_ALL_ZOOM` 16 → **19**
- New rule: nodes hidden at z≤16; z17–18 only shared nodes (small dots); z19+ all nodes
  (shared nodes = small squares, regular nodes = small dots)
- Updated the `node_lod_follows_zoom` unit test and the docs/05 node LOD table in sync


### 2026-11-15 · Red selection highlight + drag directly after selecting
- Selected elements draw a red highlight (node = red circle; way = red polyline outline), without rebuilding the scene at all (egui overlay)
- **No long-press needed after selecting**: left-drag anywhere (including empty space) moves the selected node/way (way = translate all its nodes);
  orange preview while dragging, and on release `execute_batch` commits once = **1 history entry + 1 rebuild**
- With nothing selected: left-drag node = move that node; left-drag empty space = no-op (reserved for box select); right-drag = pan the map
- `neo-tools::move_nodes_batch` + unit tests (one undo resets the whole batch move)
- UI automation verification: `28-select-node` (red circle), `29b-drag-from-empty` (after selecting a node, dragging from the sea still moves the selected node),
  `35-select-way-hook` (way red outline + property panel shows 1302 nodes), `36-drag-way-hook` (drag translates all 1302 nodes, undo 1)
- Added test hook `NEO_TEST_SELECT=way` (selects the way with the most nodes in the viewport, for automation)


### 2026-11-15 · Invisible nodes cannot be selected/dragged (picking shares the render LOD rules)
- Extracted `neo_scene::node_visible_at(zoom, way_ref_count)` (invisible at z<17; 17–18 shared only; ≥19 all),
  shared by `SceneBuilder` and desktop picking so "visible" and "clickable" cannot diverge
- `pick_at` filters invisible nodes: clicking a hidden node does not select it (the way under the cursor is still selectable)
- After zooming, hidden node selections are pruned automatically (`prune_hidden_node_selection`; way/relation selections kept) and the red highlight disappears in sync
- Unit test `node_visibility_rule_boundaries` (boundaries 16.99/17/18.5/19)
- Automation verification:
  - z19: `41a-node-selected-z19` (node 21911894 selected, red circle)
  - After zooming to z18: `41b-node-selection-pruned` (the node becomes invisible → selection pruned, right side shows nothing selected)
  - z16: `37-z16-pick` (clicking the original node position no longer selects the node)
- Test hook addition `NEO_TEST_SELECT=node` (selects the first pickable node in the viewport)


### 2026-11-15 · Selected features get a "draggable outer margin" (16px)
- Before: after selecting, dragging from anywhere moved the feature (too loose, easy to mis-drag)
- Now: the drag origin must fall within the selected feature's **16px screen outer margin** (node = point distance, way = polyline segment distance);
  dragging far from the feature does not move it; with no node selected, directly hitting a node still drags that node
- `app.rs` gained `selection_screen_distance` + `point_segment_distance`; the selection set only becomes draggable near the mouse
- Help text/README updated in sync


### 2026-11-15 · Selection drag outer margin (16px) verification
- Measured (`scripts/ui_check.ps1` + `NEO_TEST_SELECT=node` selecting a z19 node):
  - Press and drag near the node (≤16px): `undo 1`, `moved 1 node`, node coordinates updated ✅
  - Press and drag far away (hundreds of px of empty space): `undo 0`, no movement/edit at all ✅
- Key fix: the drag origin must use the **press origin** (`pointer.press_origin()`), not the current pointer position when the drag threshold fires;
  otherwise the pointer has already moved ~18px and falsely exceeds the 16px margin (the first implementation made "nearby drags" fail).


### 2026-11-15 · Margin also serves as the "selectable range" (unified with the drag margin, but separate per element type)
- `HitTester::pick_with_margins(camera, screen, node_margin, way_margin)`:
  - Node 16px / way 16px **independent margins**; a click within the node margin prefers the node (never mis-selects the way underneath), otherwise select the way
  - Selectable range matches the drag range (both within the 16px outer margin)
- Desktop `pick_at` switched to this interface; invisible nodes are still filtered (consistent with render LOD)
- Unit test `pick_with_margins_separates_nodes_and_ways`: clicking a long way's midpoint selects the way; a 10px offset near a node selects the node; far away nothing is hit
- UI verification: `45b-margin-click-node` (click 10px from a node → node 21911894 selected)


### 2026-11-15 · Fixed "clicking a feature does not select it, but a few pixels to the side does"
- Root cause: `pick_with_margins` did node-priority picking first, returned a hidden node, and the app layer then filtered it by LOD,
  **losing the way fallback candidate** → when all nodes are hidden at z16 and below, clicking a road within a hidden node's margin selected nothing at all
- Fix: added `HitTester::pick_with_margins_filtered(..., accept_node)` that filters hidden nodes **during picking** and falls back to the way;
  desktop `pick_at` switched to it (`node_selectable` as the filter)
- Unit test `filtered_pick_falls_back_to_way_when_node_hidden` (no filter → node; filtered → way)
- UI regression: clicking a road pixel at z16 (previously no selection `46-bug-z16-click-road`) → now correctly selects way 1424758490 (`47-fixed-z16-click-road`)


### 2026-11-15 · Removed the release console window
- `apps/neo-desktop/src/main.rs` gained `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`
- Verified: the release exe's PE Subsystem = **2 (GUI)** — double-click no longer shows a black console; the debug exe stays at 3 (console) so `cargo run` shows tracing logs
- Since there is no console, added startup-failure fallbacks: an `rfd` error dialog + `out/neo-startup-error.txt`
- Logging: in release, write to a file via `NEO_PERF_LOG=out/perf.log`; README updated


### 2026-11-15 · Auto-select before dragging (fixes "dragged but nothing selected" / "cannot drag a way without selecting it first")
- When a drag starts and the hit feature is **not selected**, first `select_only` it (red outline appears immediately), then start dragging
- Drag range = currently selected nodes + all nodes of selected ways (new `collect_selection_nodes`)
- On release keep the selection and `request_repaint()`; the red highlight shows at the moved position
- Measured:
  - z19, nothing preselected, drag a node directly → `undo 1`, `moved 1 node`, node selected with red circle (48a)
  - z16, nothing preselected, drag a way directly → `undo 1`, `moved 15 nodes`, way selected with red outline (48b)
- UI help/README/docs/05 updated


### 2026-11-15 · UI refactor to JOSM style (modernized) + Phosphor icons
- `crates/neo-ui` rewritten: top **menu bar** (File/Edit/View/Tools/Help) + **icon toolbar**, left **layer panel**,
  right **property/tag panel**, bottom **status bar** (center coordinates/zoom/selection count/undo-redo/latest action)
- Icons: open-source **Phosphor Icons** (`egui-phosphor` 0.14, MIT); font installation merged with Chinese CJK font loading in `install_fonts`
- Dark theme (`apply_modern_style`): JOSM blue accent, unified spacing/font sizes/button heights, map canvas centered
- Help window (Help → Shortcuts and About) lists shortcuts plus icon/data licenses
- Fixes: panels widened by long labels squeezed the map flat (width constraints + label truncation); long status-bar text overlapped the left metrics (truncation)
- Screenshot: `assets/ui-josm-style.png` (referenced by README)


### 2026-11-15 · exe icon + built-in full Unicode fallback fonts
- Windows exe icon: `build.rs` embeds `assets/neo.ico` via `rc.exe`; window/taskbar icon uses `assets/neo-icon.png`
  (verified: `Icon.ExtractAssociatedIcon(exe)` returns the 32×32 NEO icon)
- Built-in fallback fonts (shipped with the exe, commercially usable open-source licenses, OFL):
  - Noto Sans CJK SC (Chinese/Japanese/Korean)
  - GNU Unifont (full BMP fallback) + Unifont Upper (SMP/Plane2/3)
  - Font chain: CJK → Phosphor icons → default font/emoji → Unifont → Unifont Upper → system CJK
- Help window gained a "font coverage sample" for manual tofu checks; exe size 15.8 → **43.2 MB** (fonts ~26 MB)
- Licenses/sources: `assets/fonts/NOTICE.md` + `OFL-1.1.txt`


### 2026-11-15 · UI refactored after JOSM (left toolbar / right panel stack / bottom measure)
- Left side is now a **vertical toolbar** (select/draw/delete/measure + finish line/split/reverse/merge)
- Right side is a JOSM-style **panel stack**: layers, tags/members, selection, relations, validation results, conflicts
- **Layer info moved to the layer context menu**: right-click a layer → Info… (window shows counts/version/dirty objects/viewport/selection), zoom to layer, save as XML
- Bottom status bar: mouse lat/lon (hover), scale bar (adaptive m/km by m/px), zoom, **measure result**, selection count, undo/redo
- Added **measure tool (M)**: two clicks show a yellow line + distance label, mirrored in the status bar
- Help window/README updated; test hooks `NEO_TEST_MODE=measure|draw|delete`, `NEO_SHOW_LAYER_INFO=1`
- Automated screenshot verification: layout `54-josm-layout.png`, measure `60-measure.png`, layer info `52-layer-info.png`


### 2026-11-15 · Right panel tuned to JOSM details
- Layer rows simplified: **eye icon** (show/hide layer; hidden leaves the map canvas blank) + **data-layer active toggle** (check box) + layer name; context menu keeps info/zoom/save-as
- Tags/members: **the table shows all tags directly** (key = value); the way node / relation member list appears at the bottom of the box; the whole box stays empty when nothing is selected
- Selection box: **shows nothing when nothing is selected** (removed placeholder text)
- Relation box: **lists all relations directly** (id + name), empty when there are none
- Validation results/conflicts: only the title remains when there is no data, **no placeholder text**; removed the right-side "Recover from journal" button (already in the menu)
- Tests: `NEO_LAYER_HIDDEN=1` verifies a hidden layer; screenshots `65-layer-hidden.png` / `66-panel-final.png` / `63-right-panel-selected.png`


### 2026-11-15 · NVIDIA Overlay popup explanation
- The "press Alt+Z to open the NVIDIA info overlay" prompt at startup comes from the **NVIDIA App/GeForce Experience Overlay**, not from NEO
- NVIDIA Overlay is a driver-level global injection with no per-app off switch (confirmed on the official forums); the README FAQ gives the steps to turn it off:
  NVIDIA App → Settings → Features → Info overlay → off; or GFE → Settings → General → In-game overlay → off
- Measured: killing the `NVIDIA Overlay` process stops the popup on the next start, but NVIDIA restarts it automatically, so scripting is not recommended; the right fix is to turn off the Overlay setting


### 2026-11-15 · UI text extracted into i18n/
- Added repo-root **`i18n/`**: `zh-CN.json` (default) / `en-US.json` + `README.md`
- Added crate **`neo-i18n`**: compile-time `include_str!` embedding, `t()`/`tf()` placeholders, missing-key fallback,
  `NEO_LANG` environment variable and runtime `set_language`
- `neo-ui` (menus/toolbar/panels/status bar/help/layer info) and `neo-desktop` (status messages/errors/mode names/window title) all switched to i18n keys
- Menu gained a **Language** selector (Chinese / English), effective immediately on switch
- Verification: `cargo test -p neo-i18n` (bilingual key alignment + fallback + placeholders), `python scripts/check-i18n.py`
- Screenshots: `70-en-ui.png` (English) / `71-zh-ui.png` (Chinese)


### 2026-11-15 · Read-only layers + settings window
- Layer "active toggle" off: **GPU shader desaturates the data + 50% opacity**, not a gray overlay; no text hint; selecting/dragging/drawing/measuring disabled
  (pan/zoom still work), the selection is cleared at the same time; the layer name is grayed out
- Language and shortcuts consolidated into a **settings window** (entry: **Help → Settings…**); the top-level Language menu was removed
- Settings window: language switch (Chinese/English) + shortcut overview (incl. mouse operations), noting that shortcut customization comes in a later version
- Help window keeps "About NEO", the font coverage sample, and license info
- Test hooks: `NEO_LAYER_INACTIVE=1` (read-only layer), `NEO_SHOW_SETTINGS=1` (settings window)
- Screenshots: `74-settings.png`, `75-layer-inactive.png`, `77-inactive-select-test.png`


### 2026-11-15 · Open-source engineering: docs site + CI / Release / Pages
- Docs reorganized following open-source project conventions:
  - Root: `LICENSE` (Apache-2.0 OR MIT), `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`,
    `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `CHANGELOG.md`
  - `docs/guide/`: install, quick start, UI, editing, files and sessions, settings/i18n, performance, FAQ
  - `docs/dev/`: architecture, workspace/crates, build and test, contributing, release, roadmap
  - `docs/SUMMARY.md` + `book.toml` (mdBook) + `theme/neo.css`; technical docs/ADRs published alongside
- GitHub Actions:
  - `ci.yml`: fmt / clippy / test / i18n / deps / release build / mdBook build
  - `release.yml`: tag `v*` → verify the tag matches the Cargo version → test → Windows zip + SHA256 → GitHub Release
  - `pages.yml`: push to master (docs / theme / book.toml changes) → mdBook → GitHub Pages (`https://trapmehere.github.io/NEO/`)
- GitHub templates: issues (bug/feature), PR template, Dependabot (cargo + actions, grouped)
- Cleaned `scripts/`: keep only long-lived tools such as `ui_check.ps1` / `bench.ps1` / `check-*.py`
- README rewritten as the open-source project homepage (badges, features, downloads, doc entry points, contributing, license)


### 2026-11-15 · Phase 4 (professional editing), part 1
- **neo-validate**: 12 rules initially, 24 today (JOSM-style additions such as isolated nodes, duplicate coordinates, suspicious tag combinations, multipolygon roles, way self-intersection) + `Report` + 12 unit tests
- Right-side "Validation results" panel: run validation, error/warning counts, issue list, click to select and zoom to the object (test hook `NEO_RUN_VALIDATION=1`)
- **neo-style presets**: switched to **id-tagging-schema** (~1500 entries, Chinese/English names, ISC license) + searchable dropdown in the tag panel
- **neo-api**: OSM API 0.6 protocol client (changeset, diff upload, map download, 409/412, MockTransport) + 5 tests
- **neo-editor conflict model**: MergeCase / ConflictResolution / three-way tag merge; 6 conflict-kind tests
- **journal recovery report**: RecoveryReport (frames/applied/skipped), details shown after recovery
- Screenshot: `81-validation.png` (validation panel)
- **Autosave**: every 60s copies the journal to `out/autosave/`, keeping 3 generations (status-bar hint)
- Remaining Phase 4: MapCSS render integration, real HTTP transport + upload/download UI, conflict-resolution UI, SQLite snapshots, changeset end-to-end tests


### 2026-11-15 · Phase 4: two-column settings + OSM account OAuth
- Settings window changed to **categories on the left / content on the right** (language, OSM account, shortcuts)
- Added **OSM account**: OAuth2 + PKCE browser login, local callback listener, token storage,
  multi-account list/switch/delete/logout; `accounts.json` lives in `%APPDATA%/NEO/`
- `neo-api::oauth`: PKCE, authorization URL, token exchange/refresh, user info parsing (4 tests)
- `ReqwestTransport` (feature `http`) wired to real HTTP (30s timeout)
- Test hooks: `NEO_SHOW_SETTINGS=1`, `NEO_SETTINGS_TAB=account`
- Screenshot: `83-settings-account.png`


### 2026-11-15 · Phase 4: OSM sync UI
- **File → Sync (download/upload)…**: shows the account, the current viewport bbox, and the changeset comment
- Download: `GET /api/0.6/map` → OSM XML → replaces the current layer
- Upload: create changeset → upload osmChange → close changeset; on success shows the number of confirmed objects
- 409: conflict object list + "re-download (discard local changes)" strategy
- Network requests run on background threads so the UI does not block; `NEO_SHOW_SYNC=1` opens the window
- Screenshot: `85-sync.png`


### 2026-11-15 · Phase 4: MapCSS + SQLite snapshots + upload end-to-end test
- **MapCSS subset**: parse/compile (selectors, tag conditions, zoom, color/fill/width/opacity) + the default
  `assets/styles/neo.mapcss` wired into `SceneBuilder`; fall back to the built-in palette on a miss
- **SQLite snapshot store** `neo_persist::SnapshotStore`: journal bytes + stats (WAL, keeps 10 entries);
  autosave writes both JSONL and SQLite; when the journal is lost, startup restores from the newest snapshot
- **Upload end-to-end test**: changeset create → upload → close; 409 conflict → retry succeeds (MockTransport)
- Screenshot: `86-mapcss.png`
- Still pending: conflict KeepLocal/manual-merge UI, MapCSS text/icon, pre-upload validator gate


### 2026-11-15 · Phase 4 wrap-up: conflict KeepLocal + docs
- The sync window's 409 conflicts gained **Keep local and retry (KeepLocal)**: `Editor::set_object_version`
  aligns with the server version and re-uploads (overwriting the remote); "re-download (TakeRemote)" remains
- Docs updated: sync conflict strategy, Phase 4 status, README
- Still pending: manual-merge UI (per field/geometry), MapCSS text/icon/full filter set, pre-upload validator gate,
  manual real-network acceptance (needs a test account)


### 2026-11-15 · Fixed OAuth + unified export + box-select download
- OAuth uses a built-in client; the settings window shows no authentication configuration details
- **Unified export window**: File → Export… (also replaces the toolbar export button); formats: OSM XML / osmChange / PBF / JOSM session
- **Download window**: toolbar cloud-download icon → left-drag a bbox on the map → download that area from the OSM API and replace the layer
- Test hooks: `NEO_SHOW_EXPORT=1`, `NEO_SHOW_DOWNLOAD=1`, `NEO_DOWNLOAD_SELECT=1`
- Screenshots: `87-download.png`, `88-export.png`
