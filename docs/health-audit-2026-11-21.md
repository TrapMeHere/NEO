# NEO Full Health Check, Fixes, and Feature Completion Report (2026-11-21)

> Scope: the entire `D:\neo` workspace (17 library crates + 3 apps). Method: read every file + layered automated scans
> (fmt / clippy / test / dependency scripts / PBF·XML roundtrip / headless rendering / desktop startup / 1M synthetic benchmark),
> then plan and land fixes by P0→P3. This document is also a **fix ledger**: each item is marked `Fixed`, `Partially fixed`, or
> `Not fixed (with follow-up plan)`. Feature completeness is covered separately in **[feature-parity.md](feature-parity.md)**.

## 1. Summary

Before the fixes:

| Item | Measured |
|---|---|
| `cargo check --workspace` | passes, 35 warnings |
| `cargo fmt --all --check` | **fails** (5 files) |
| `cargo clippy` | passes but ~60 warnings (including 48 in desktop) |
| `cargo test --workspace` | **0 `#[test]` on `master`**, only 1 doctest |
| Test directories | `tests/` contains 3 empty directories |
| CI | does not run `cargo test`; release does not run tests either; `npm ci` runs scripts under `contents: write` |
| Dependency metadata | all 20 packages publishable (no `publish = false`), repository points to the wrong organization |

After the fixes:

| Item | Measured |
|---|---|
| `cargo check --workspace` | passes, **0 warnings** |
| `cargo check --workspace --all-features` | passes (including wasmtime 50) |
| `cargo fmt --all --check` | passes |
| `cargo clippy --workspace --all-targets -- -D warnings` | **passes (0 warnings)** |
| `cargo test --workspace` | **64 tests pass** |
| `python scripts/check-{deps,i18n,doc-links,write-api}.py` | all pass |
| Desktop startup smoke | alive for 8 s, scene ~2.0 ms/39k vertices, GPU upload 0.23 ms |
| Headless rendering | 800×600 PNG, 22,708 triangles, GPU frame 3.83 ms |
| 1M synthetic benchmark | import 168 ms, z16 scene 0.819 ms, peak 557 MiB |

The biggest risk narrowed from "**silently corrupting data/losing recovery**" to "**large-scale performance and incomplete product loops**".
No change was found that required a full rollback; the only high-risk performance-related change (full WorldCache rebuild) was kept only after being rewritten as incremental updates.

## 2. Fixed P0s (data corruption / security / crashes)

### 2.1 IDs and indexes

- **Duplicate `ObjectId` overwriting the index**: `push_node/way/relation` used to `HashMap::insert` directly,
  turning the old object into an orphan that is "alive but has no ID" and exporting duplicate IDs. → The three `push_*` now return `Result`, duplicate IDs
  report `Consistency`; all import paths propagate the error.
- **Local negative ID collisions**: `next_local_id` decremented blindly, so after importing a file containing negative IDs, new objects reused IDs.
  → Skip any occupied node/way/relation ID.
- **`way_node_ids` silently dropping points**: the OSC/PBF write path `filter_map`ped away stale references. → Added
  `try_way_node_ids()`, and export now errors on stale handles; `set_way_nodes` validates handles and returns `Result`.
- **`take_dirty_*` minting future handles for dead slots**: → filter with `contains()`.

### 2.2 Journal / crash recovery (the original implementation was effectively unusable)

- **`JournalFrame::Cmd(JournalCommand)` could not be serialized**: serde's internally-tagged enum
  does not support newtype variants, so the journal **could not write a single command**, and crash recovery never actually worked.
  → Changed to `Cmd { #[serde(flatten)] command }` and added JSON roundtrip tests.
- **Wrong recovery baseline**: the old `init_editor` replayed commands onto an empty DataSet, so all edits in file-based sessions failed.
  → `neo-state.json` records the source file, and recovery loads it first; on success, `neo-recovered.osm` is written immediately as the new baseline.
- **undo/redo did not write the journal**: an undone delete would come back after restart. → Added a `Rebase` frame; after undo/redo,
  the journal is rebuilt from the current undo stack.
- **Half transactions / bad lines**: an unclosed transaction would be replayed together with later commits, and reading continued after a bad line. → `replay_plan` validates
  `TxBegin(id)→Cmd*→TxCommit(id)` pairing; stops at the first bad line; tolerates a UTF-8 BOM.
- **append did not truncate / sessions crossed documents**: → `FileJournal` now emits newlines and supports `reset()`; opening/downloading/
  saving/uploading clears the journal; state is written before switching documents.
- **Journal write failures were swallowed**: → record the failure and stop writing; the UI shows the error; `sync_data()` per line.
- **`record()` wrote automatic IDs as 0**: → write the actual `applied` ID.

### 2.3 Transactions / history / change state

- **Batch transaction rollback was incomplete**: a failed command's own partial writes, swallowed rollback errors, and `ChangeTrack` not restored.
  → Refresh referrers + precheck before each command in the batch; on failure undo the current command and roll back in reverse order; restore
  ChangeTrack; errors are no longer silent.
- **undo/redo popped the stack before executing**: a failure lost history. → Move the stack only after success; on batch failure roll back the undone/redone parts.
- **undo did not restore `ChangeState`**: after undo the data still showed as dirty, and the upload diff contained empty modifications. → Save/restore for all commands.
- **`MoveNode` undo used the wrong position**: it used the position at apply time. → Use the `from` captured at construction time.
- **`SplitWay` redo changed the ID**: → write back `new_way` after first allocation.
- **`MergeWay` self-merge/lost tags/ignored relation references**: → reject `a==b`, merge tags unique to b, and reject
  when b is referenced by a relation.
- **Deleting a relation did not check parent relations**: → added a relation→relation CSR to referrers.

### 2.4 Untrusted file input

- **`.joz` Zip Slip (arbitrary file write)**: → only take `file_name`, reject `/ \ : ..`; limit entries to
  4096, a single layer to 64 MiB, the total to 256 MiB, and session.jos to 4 MiB; missing layers no longer fall back to local paths.
- **`.jos` double escaping/paths**: → `normalized_value` decoding; atomic writes; viewport validation.
- **XML timestamps**: real `.osm` ISO-8601 timestamps were parsed as integers (all became 0). → Added RFC3339
  two-way conversion; also added `action` reading/writing, coordinate validation, root-element/truncation checks, entity decoding, atomic writes.
- **Missing/invalid lat/lon became (0,0)**: → visible nodes without coordinates now error; `LatLon::try_from_degrees`
  rejects NaN/out-of-range; adopted by OSC/PBF/GPX synchronization.
- **PBF**: coordinates use nanodegrees with validation; `MissingPolicy::Error` actually returns; visibility writes declare
  `HistoricalInformation`; blob/header limits; delta `checked_sub`; atomic writes; real bytes;
  errors on dangling references; duplicate IDs rejected.
- **GPX**: lat/lon finiteness and range validation; push errors propagated.
- **Export atomicity**: XML/PBF/`.jos`/`.joz` all use a temporary file in the same directory + rename.
- **Invalid XML code points**: `escape_xml` drops characters forbidden by XML 1.0.

### 2.5 Security boundaries

- **Predictable OAuth state**: → 32-byte CSPRNG; callback validates method/Path/Host; a state mismatch
  keeps waiting as `Ignore` (to prevent remote login interruption); removed `out/oauth-debug.txt`.
- **Token leak surface**: `Debug` for `HttpRequest`/`TokenSet`/`Pkce`/`Account` is fully redacted;
  token files are written atomically and backed up as `.corrupt-*` when corrupted; the config directory moved to the per-user data directory.
- **Token expiry**: `active_token()` refreshes near expiry and writes back atomically; shared by upload/download.
- **`NEO_API_URL` could point to http**: → only https or local loopback is accepted.
- **HTTP redirects**: automatic redirects disabled, preventing POST from being changed to GET by 301/302 or replayed by 307/308.
- **WASM plugins**: multi-memory disabled; memory/table/instance limits; the network allowlist rejects
  userinfo; http_log cap; `read_file` 8 MiB; `.neopkg` package/entry/extraction limits; duplicate entries rejected.
- **GPU**: registered `on_uncaptured_error`; validation/device errors are logged.
- **Paths**: journal, autosave, state, recovery, sync downloads, and startup errors all moved to the per-user data directory.

## 3. Fixed P1s (functionality / performance)

### 3.1 Rendering and scenes

- **`max_vertices` safety valve never worked**: `stats.vertices` was only assigned after the loop. → Use the live vertex sum of the three batches
  each round, and set `clamped` when over the limit.
- **Every edit rebuilt the entire WorldCache (10M nodes = tens of millions of projections)**: → WorldCache moved into `Editor`,
  updated in O(1) by `EditContext` on add/move/remove/restore; SceneBuilder holds a read-only reference.
- **resize did not rebuild / pan threshold 25% / zoom threshold 0.5**: → rebuild on viewport change, pan 4%,
  zoom 0.25 and always rebuild when crossing the 17/19 LOD steps; picking uses the zoom at scene-build time.
- **Closed-area fill returned early, so there was never a stroke**: → continue to stroke after fill; style is computed only once.
- **Duplicate points produced 3×-wide spikes**: → zero-direction normals fall back to one side and skip miter scaling.
- **RDP recursion could overflow the stack / was O(n²)**: → explicit-stack iteration; keep 256-point pre-decimation.
- **Date line**: render vertices wrap around the camera origin by ±C (spatial-index bbox/picking are still follow-up items).
- **Camera NaN/out of range**: fallback for non-finite zoom; center clamp; degenerate bbox no longer jumps to z22.
- **Polar data silently rewritten**: dragging is forbidden at |lat|>85.05° with a notice.

### 3.2 Editing hot paths

- **AddNode rebuilt Referrers entirely**: → a new node has no references, so it is not marked dirty; AddWay/delete still rebuild.
- **`live_len()` was O(slots)**: → `SlotAllocator` maintains an O(1) `live` counter.
- **Version number did not increment with `push_*`/`set_*`**: → unified `bump_version()`.
- **`Referrers` counted logically deleted objects as references**: → skip `visible=false`, validate handles, deduplicate closed ways,
  and add relation→relation.
- **`TagStore::get` panicked out of bounds**: → return an empty slice.
- **`is_valid_lat/lon` failed for `i32::MIN`**: → `unsigned_abs`; `approx_meters` uses f64 throughout.

### 3.3 Sync loop

- **Upload success did not apply diffResult**: negative IDs/versions were never written back. → `apply_diff_result()` remaps
  IDs, writes versions back, clears ChangeTrack/history/journal, and flushes `neo-uploaded.osm` to disk.
- **changeset lifecycle**: failures also attempt to close; a close failure does not overwrite the real upload result.
- **409 parsing returned empty**: → `parse_conflict_text` extracts the object and server version.
- **Wrong delete order**: → relation→way→node; `if-unused="true"`; locally created then deleted objects are not uploaded.
- **OSC entities not decoded/lost user/silently dropped references**: → unified decoding, preserve user, report errors according to policy.

### 3.4 Desktop

- Unified `confirm_discard()` before open/download/recovery.
- Recovery failure keeps the bad journal as `.unrecovered-*`; success writes the baseline.
- The tag editor resyncs after undo/redo/delete/failure; `apply_tags` validates the current selection.
- Sync UI entry points, status placeholders, per-user data directory, session saving embeds current edits.
- PBF/XML import moved to a background thread; autosave copy/read + SQLite moved to the background.
- Inactive/hidden layers uniformly block the canvas, menus, shortcuts, tags, relations, clipboard, and history rollback.

## 4. Fixed P2s / engineering

- Removed unused workspace dependencies; 20 packages `publish = false`; metadata inheritance; repository corrected;
  MSRV 1.97, toolchain pinned to 1.100.0.
- `rustfmt.toml` removed options unsupported on stable.
- `neo-style/build.rs` no longer runs node on every build; corrupted/missing output fails explicitly; supports `NODE`.
- `neo-desktop/build.rs` PE version generated from `CARGO_PKG_VERSION`.
- CI: fmt / clippy `-D warnings` / check / **test** / all-features; release adds a test gate,
  `npm ci --ignore-scripts`, `neo-cli`, license texts, `-ErrorAction Stop`, concurrency/timeout.
- NOTICE corrected for external fonts, added NSI BSD-3-Clause, LICENSE-ISC/BSD texts.
- Installers: Windows/Linux/macOS all ship `assets/` and licenses; resource search adds
  `/usr/share/neo` and `Contents/Resources`.
- Added `.gitattributes` to avoid treating fonts/PNGs as text on different platforms.
- README/TODO/docs consistent with code; `check-deps.py` adds cycle detection and publish checks.

## 5. Feasibility analysis: which changes were validated, which were reverted/rewritten

| Change | Risk | Handling |
|---|---|---|
| WorldCache full rebuild → owned by Editor with incremental updates | Large cross-crate API change, easy to miss updates and cause misalignment | Forced all call sites at compile time; add/move/remove/restore/undo/redo/apply_diff all update; verified with the 1M benchmark and desktop rendering. The full-rebuild version was not kept. |
| Referrers full rebuild → only remove the AddNode dirty mark | An incremental CSR refactor is too broad | No incremental CSR; only eliminated the O(k·N) "rebuild per drawn point". AddWay/delete still rebuild fully, registered as remaining work. |
| journal append-only → Rebase | Old log compatibility | Old frames are still readable; the new `MoveNode` field is `#[serde(default)]`; old logs without Rebase replay with the original semantics. |
| Strict XML root-element/truncation validation | May reject non-standard `.osm` snippets | Only accepts `<osm>`, which is a correct constraint for OSM 0.6; covered by unit tests. |
| Do not upload locally created then deleted objects | May miss deleting server objects | Only applies to `id.is_local()`; server objects get a normal `<delete>`. |
| GPU errors changed from panic to logging | May hide rendering errors | Still `tracing::error!`, avoiding disappearing silently in a console-less release. |
| HTTP redirects disabled | Some mirrors/proxies use 302 | The official OSM API does not redirect; illegal redirects should never replay a Bearer token anyway. |

## 6. Not fixed / partially fixed (with follow-up plans)

1. **TagStore / WayStore are append-only, no compact** (P1 monotonic memory growth). Plan: count
   hole ratio at transaction boundaries, rewrite the arena and remap `TagSetId`/way offsets above a threshold; requires `DataSet::compact()`
   and long-session tests.
2. **Referrers structural changes are still O(N+E)** (AddWay/delete). Plan: per-slot growable `SmallVec` or
   overlay (new edge table + tombstones); measure p95 at 1M first, then replace.
3. **UI thread blocks on IO**: validation, export, and rfd dialogs are still inside `ui()` (import and autosave have moved to the background);
   find is also a full table scan (10M would stall). Plan: reuse workers for export/validation/find; change rfd to
   `AsyncFileDialog`; build a tag inverted index for find.
4. **10M memory**: WorldCache 24 B/node, SpatialIndex duplicate bboxes, TagStore without dedup.
   Plan: `Vec<WorldPoint> + BitSet`; remove entries from the index; content-hash dedup for tags.
5. **Date-line spatial index/picking**: rendering wraps already, but bbox/picking remain approximate. Plan: bbox unwrapping + dual-copy
   entries, or center world x on longitude 0.
6. **Polar projection**: WebMercator does not support |lat|>85.05; dragging is forbidden to avoid corrupting data, but editing is impossible.
7. **PBF**: the write side does not adapt chunk size by bytes; no DenseNodes; a truncated PBF may "succeed"; `osmpbf`
   panics on unknown relation member types. Plan: flush by block budget, DenseNodes,
   wrap single-blob decoding in `catch_unwind`, upstream fix/replace the decoder.
8. **GPX**: `ele/time/rte` are still lost. Plan: map to tags, streaming read/write.
9. **Sync**: no chunked upload, no Ambiguous timeout semantics, no manual merge UI; tokens stored in plaintext on disk (recommend
   DPAPI/Credential Manager); plugins are not linked into the desktop app.
10. **Plugins**: signatures are still self-hashes inside the manifest (no trust root), only WAT is supported, and data returned by host calls is discarded.
    Plan: Ed25519 + built-in public keys, `load_binary`, ABI-returned buffers.
11. **Rendering**: indirect draw/cull not wired, no buffer pool, no sRGB, readback fully synchronous.
12. **MapCSS**: `@media/@import` not skipped, selector commas do not recognize quotes, `area/closedway` semantics
    collapse, dashes not drawn.
13. **CI/release**: Actions not pinned to SHA, unsigned/no SBOM/provenance, no cargo-deny, no three-platform
    matrix.
14. **Test coverage**: 64 tests concentrated on core regressions; UI/network/GPU/10M still have no automation.

## 7. Verification record

### 7.1 Compilation / static checks

```
cargo check --workspace                          # 0 warnings
cargo check --workspace --all-features           # passes (wasmtime 50)
cargo fmt --all --check                          # passes
cargo clippy --workspace --all-targets -- -D warnings  # passes
cargo test --workspace                           # 64 passed
python scripts/check-deps.py                     # OK: 20 packages, DAG + publish
python scripts/check-i18n.py                     # 339 keys, zh/en 502
python scripts/check-doc-links.py                # 393 links
python scripts/check-write-api.py                # 60 files
```

### 7.2 Runtime smoke

- Desktop: alive for 8 seconds; `scene build ~2.0ms / 39k verts`; `gpu upload 0.23ms`;
  journal correctly created in the user data directory.
- journal recovery: manually write `TxBegin/AddNode/TxCommit` → after startup the journal rotates to
  `*.recovered-*`, `neo-recovered.osm` and `neo-state.json` are generated; the second startup uses that file as the baseline.
- CLI: `scene --nodes 5000` → `vertices=2061 build=143µs`; preset coverage normal.
- Headless rendering: `render --synth 20000` → 800×600 PNG, 22,708 triangles, 3.83ms.
- 200k synthetic benchmark: import 25.0ms, index 120.9ms, z16 0.82ms, fit-all 12.3ms,
  move 9.37µs, pick 2.05µs, peak 246.3 MiB.
- 1M synthetic benchmark: dataset 126.6 MiB, import 168.3ms, index 584.8ms, referrers 13.2ms,
  z16 0.819ms, fit-all 21.6ms, move 16.8µs, pick 8.9µs, peak 557.4 MiB.

### 7.3 Performance conclusions

- Scene building did not regress: z16 viewport 0.8ms; editing a node no longer triggers 10M projections, and WorldCache incremental updates are a net win.
- Full rebuilds still happen on AddWay/delete (Referrers) and on viewport scene rebuild after every edit; still a bottleneck at 10M.
- No performance regression caused by this round's changes was found; the only risky full-cache change was replaced by an incremental approach.

## 8. Key file index

| Topic | File |
|---|---|
| IDs/coordinates/handles | `crates/neo-core/src/{ids,units,handle,sym}.rs` |
| Data set/indexes/references | `crates/neo-store/src/{dataset,indexes,tag_store,node_store,way_store,relation_store}.rs` |
| Commands/transactions/history/journal | `crates/neo-editor/src/{editor,commands,commands_geom,history,journal}.rs` |
| XML/OSC/PBF/timestamps | `crates/neo-io/src/{xml,osc,osc_read,pbf,pbf_parallel,pbf_write,time}.rs` |
| Camera/geometry/cache | `crates/neo-geo/src/{camera,geometry,world_cache,world}.rs` |
| Scene building | `crates/neo-scene/src/builder.rs` |
| MapCSS | `crates/neo-style/src/{mapcss,lib,schema,assets}.rs` |
| Sessions/snapshots | `crates/neo-persist/src/{session,snapshot}.rs` |
| API/OAuth | `crates/neo-api/src/{lib,oauth}.rs` |
| Plugin sandbox | `crates/neo-plugin/src/{wasm_host,gate,package,capability,apply}.rs` |
| Desktop | `apps/neo-desktop/src/{app,accounts,main,build}.rs` |
| CI/release | `.github/workflows/{ci,release}.yml` |
| Regression tests | `crates/neo-editor/tests/regressions.rs`, `crates/neo-io/tests/regressions.rs`, `crates/neo-persist/tests/zip_safety.rs`, `#[cfg(test)]` in each crate |

## 9. Next steps (ordered by ROI)

1. Re-run XML/OSC/PBF roundtrip and desktop open with real Bremen/Monaco data.
2. 10M PBF import + edit + 1-hour session, observing arena growth and referrers rebuild time.
3. Move validation/export off the UI thread.
4. Implement arena compact and incremental Referrers, then re-measure 10M editing.
5. Sync: add chunking/Ambiguous/manual merge; hand tokens to the OS credential store.
6. CI: add Action pins, cargo-deny, three-platform check, signing/SBOM.

## 10. Feature completion (second round the same day)

Compared with iD/JOSM, filled the interaction gaps professional users hit most; see **[feature-parity.md](feature-parity.md)**:

- **Relation editing**: add/remove members, move up/down, edit roles, add to selection; added `SetRelationMembers` /
  `AddRelation` (with undo and journal); relation list items are clickable to select.
- **Box selection**: drag a rectangle on empty space, Shift accumulates; spatial index + any node inside selects the way.
- **Copy/paste/duplicate**: Ctrl+C/V/D, cross-document string clipboard; copying a way includes its nodes automatically.
- **Find**: Ctrl+F, `id:123` / `key=value` / text; results are clickable to select and locate.
- **Command history**: the right side shows undoable commands; click to undo to before that step.
- **GPX**: import/export in the File menu; imports go through command batches, preserving the single writer.
- **Geometry**: simplify, glue, align, distribute, horizontal/vertical mirror, unglue, parallel copy, orthogonalize,
  merge closed areas (multipolygon), move a node onto a way, extrude building outlines.
- **Filters**: hide unmatched objects by type/`id:`/`key=value`/text, or hide the selection; scene and picking stay in sync.
- **Upload gate/upload selection**: errors block upload; upload only selected objects with new dependencies included automatically.
- **Multiple layers**: the active layer is editable, docked layers are read-only rendering; create empty layer, open as layer, switch, show/hide,
  delete; docked layers are saved to `layers/*.osm` and restored at startup; whole-layer copy into the active layer (ID remapping).
- **Changeset/offset library**: View → Changeset viewer loads server changesets; imagery offset library JSON
  import/export, apply the nearest offset at the current center.
- **Shortcuts**: Settings → Shortcuts allows rebinding each item (press the new combination, Esc cancels), effective immediately and written to
  `neo-state.json`; supports Ctrl/Shift/Alt + key, restore defaults.
- **Conflicts**: after 409, keep local/re-download; per-object take remote/merge tags/merge geometry.
- **multipolygon**: assemble outer/inner members of `type=multipolygon/boundary`, earcut actually cuts holes;
  unclosed members are not stitched automatically.
- **Notes**: notes list for the current area, create, comment, close, click to locate; public read, login required to write.
- **Plugin management**: install `.neopkg`, list, enable/disable, delete; when enabled, `PluginHost` loads
  WAT/WASM, authorizes per manifest capabilities, executes the on_load command through an Editor transaction. Signing is still the manifest self-hash.
- **Imagery**: TMS tiles + WMS single image (async, PNG, opacity, X/Y offset, persistence); only accepts
  https/local loopback. The offset database is not shared.
- **Validation rules**: expanded from 17 to 25, adding unclosed area, non-adjacent duplicate nodes, illegal oneway,
  empty name, layer out of range, multipolygon without outer/unclosed outer; all UI rule names go through i18n.
- **Layer ordering**: the scene buckets by OSM `layer` (-5..5, invalid values treated as 0) and submits fill→stroke→point
  in ascending layer order; tunnels/bridges/viaducts no longer depend on traversal order.
- **MapCSS/tags**: `dashes`, `text` (tag key), `text-color`, `font-size`, `icon-image`
  (PNG under assets, 16px default/icon-width) are actually drawn; SVG/remote icons unsupported.
- **Settings**: General (language/autosave), Editing (simplification tolerance), Validation (gate), Network (API address),
  Storage (data directory/clean autosaves), Account, Shortcuts; settings persist to `neo-state.json`.
- **About window**: removed font samples, keeping only version and license info.
- **Validation**: +5 rules, 17 total.
- **Read-only protection**: inactive/hidden layers also block menus, shortcuts, tags, relations, clipboard, and history rollback.

Verification: `cargo test --workspace` **64 passed**; clippy `-D warnings`, fmt, all-features all pass;
desktop smoke alive for 8 s (scene 2.0ms/39k vertices, GPU upload 0.23ms).
