# NEO feature completeness vs. iD / JOSM (2026-11-21)

> Purpose: turn "is the feature there?" from a README slogan into a verifiable matrix.
> Status definitions: **✅ Done** = UI entry point + command/transaction + round-trip/tests;
> **🟡 Partial** = backend or a single path works, missing key interactions/edge cases;
> **❌ Missing** = no implementation; **—** = explicitly out of scope / not planned.
> The iD/JOSM columns record only their **default** capabilities, without expanding the plugin market.

## 1. Conclusion

NEO already covers the main chain "open data → select/draw/edit → validate → save/export →
download/upload/conflict". This round added **relation member editing, rectangular selection,
copy/paste/duplicate, search, GPX import/export, way simplification, and node gluing**, and
expanded the validation rules from 12 to 24.

Compared with mature editors, the remaining gaps fall into four categories:

1. **Depth of geometry tools** (JOSM's orthogonalize/align/distribute/mirror/extrude/unglue/
   join areas/multipolygon, etc.);
2. **Layers and imagery** (multiple data layers, TMS/WMS imagery layers, offset calibration);
3. **Upload workflow details** (upload selection, chunking, manual merging, changeset history, notes);
4. **Ecosystem** (plugins not wired into the desktop app, MapCSS text/icon, history view, audio/cadastre/printing).

These are not "unusable", but "walls a professional mapper will hit after working for a week".
Each item is recorded in §4.

## 2. Overview matrix

Legend: ✅ done; 🟡 partial; ❌ missing; — out of scope.

| Area | Capability | NEO | iD | JOSM | Notes |
|---|---|---|---|---|---|
| Data | OSM XML read/write | ✅ | ✅ | ✅ | Includes ISO timestamps, action, visible, negative IDs |
| Data | PBF read/write | ✅ | 🟡 (read-focused) | ✅ | Self-built encoder on the write side; no DenseNodes |
| Data | osmChange read/write | ✅ | ✅ | ✅ | delete follows dependency order, if-unused, negative ID folding |
| Data | GPX read/write | ✅ | 🟡 | ✅ | No ele/time/rte semantics |
| Data | `.jos/.joz` sessions | ✅ | — | ✅ | Safe zip extraction, external layer files |
| Data | Multiple data layers | ✅ | 🟡 | ✅ | Active edit + docked read-only rendering/switch/show-hide/new/open/persist/full-layer copy |
| Data | Cross-layer copy | 🟡 | 🟡 | ✅ | Full-layer copy from a docked layer to the active layer (ID remapping); object-level copy not done |
| Edit | Select/multi-select/box select | ✅ | ✅ | ✅ | This round adds rectangular selection and Shift to accumulate |
| Edit | Draw way/close | ✅ | ✅ | ✅ | Double-click/Enter to finish |
| Edit | Delete (reference check) | ✅ | ✅ | ✅ | Refused when referenced; references must be handled first |
| Edit | Move node/whole way | ✅ | ✅ | ✅ | Drag batch move = one history entry |
| Edit | Tag table/presets/NSI | ✅ | ✅ | ✅ | schema + brand suggestion search; multi-select can add/delete tags in bulk |
| Edit | Split/merge/reverse way | ✅ | ✅ | ✅ | Split supports selected nodes |
| Edit | Simplify way | ✅ | — | ✅ | RDP, tolerance in screen pixels |
| Edit | Glue nodes | ✅ | ✅ | ✅ | New this round; way/relation references migrate in sync |
| Edit | Copy/paste/duplicate | ✅ | ✅ | ✅ | New this round; cross-document string clipboard |
| Edit | Orthogonalize/align/distribute/mirror | ✅ | 🟡 | ✅ | Align/distribute/mirror/unglue/parallel/orthogonalize; join areas/extrude missing |
| Edit | Unglue/parallel way | ✅ | — | ✅ | Implemented this round |
| Edit | Polygon assembly/multipolygon relations | 🟡 | 🟡 | ✅ | multipolygon outer/inner + earcut holes; unclosed members are not stitched |
| Edit | Relation member add/remove/order/role | ✅ | ✅ | ✅ | New this round; no manual merge |
| Edit | Undo/redo | ✅ | ✅ | ✅ | Command batches, coalescing, journal rebase |
| Search | Find by ID/tag/text | ✅ | ✅ | ✅ | New this round; up to 500 results |
| Search | History/command stack panel | ✅ | — | ✅ | Right panel shows undoable commands; click to undo to that step |
| Validation | Built-in rules | 🟡 24 rules | 🟡 | ✅ 100+ | Added since the 12-rule baseline: way too long, closed way with too few nodes, relation missing `type`, duplicate relation members, invalid `layer`, unclosed area, non-adjacent duplicate nodes, invalid `oneway`, empty `name`, `layer` out of range, multipolygon no-outer/outer unclosed |
| Validation | Configurable/ignore/auto-fix | ❌ | 🟡 | ✅ | |
| Rendering | Vector map/node LOD | ✅ | ✅ | ✅ | Bucketed and sorted by OSM `layer`; node LOD; no full text/icon/shield |
| Rendering | MapCSS subset | 🟡 | — (own styling) | ✅ | fill/color/width/opacity/zoom/dashes/text/text-color/font-size/icon-image(PNG) |
| Rendering | Imagery layers (TMS/WMS) | ✅ | ✅ | ✅ | TMS tiles + WMS single image, async, opacity, X/Y offset, offset DB import/export/apply |
| Sync | OAuth2+PKCE/multiple accounts | ✅ | ✅ | ✅ | Token refresh wired up |
| Sync | bbox download | ✅ | ✅ | ✅ | 0.25° cap is not chunked |
| Sync | Upload changeset/diff | ✅ | ✅ | ✅ | diffResult written back, change cleanup |
| Sync | 409 conflict | 🟡 | ✅ | ✅ | KeepLocal/re-download + take remote/merge tags; no position/member field-level selection |
| Sync | Upload selection/chunking | 🟡 | 🟡 | ✅ | Upload selection + dependency-group chunking/partial success; oversized mixed changes refuse chunking |
| Persistence | journal crash recovery | ✅ | 🟡 | ✅ | Recovery writes a new baseline; no fsync of the directory |
| Persistence | Autosave/snapshot | ✅ | ✅ | ✅ | journal + SQLite snapshot |
| Sync | Notes | ✅ | ✅ | 🟡 | List/create/comment/close/locate in the current area |
| Platform | Chinese UI/i18n | ✅ | ✅ | ✅ | zh/en |
| Platform | Settings/About | ✅ | ✅ | ✅ | General/edit/validation/network/storage/accounts/shortcuts (rebindable, persisted) |
| Platform | Plugins | ✅ | — | ✅ | Install/list/enable/delete + WAT/WASM execution; no signature trust root |
| Platform | CLI/headless | ✅ | — | ✅ | CLI/prototype |
| Platform | Three-platform release | 🟡 | ✅ | ✅ | Windows is the primary target; Linux/mac scripts have assets |

## 3. New/improved this round (verifiable)

| Capability | Entry point | Implementation |
|---|---|---|
| Relation member editing | Right-side member panel: ↑ ↓ ✕, role editing, add to selection | `SetRelationMembers` command + undo/journal; batch refresh of reverse lookup tables |
| Rectangular selection | With the select tool, drag a rectangle on empty space; Shift accumulates | Spatial index query + select a way if any node falls inside the rectangle |
| Copy/paste/duplicate | Edit menu / Ctrl+C, Ctrl+V, Ctrl+D | String clipboard, cross-document; remaps node/way/relation IDs within the batch |
| Search | Edit → Find… / Ctrl+F | `id:123`, `key=value`, text; click to select and center |
| GPX | File → Import/Export GPX… | Imported through command batches, keeping a single writer |
| Way simplification | Tools → Simplify way | RDP, 1.5 px tolerance at the current zoom |
| Node gluing | Tools → Glue nodes | Transaction: merge tags, update way/relation references → delete redundant nodes |
| Geometry tools | Tools menu | Align, evenly distribute, mirror horizontally/vertically, unglue, parallel-copy way |
| Upload selection | Upload window | Upload only selected objects, automatically including new dependencies |
| Settings/About | Help → Settings/About | 7 settings pages; About drops the font example and keeps only version/license |
| Imagery layers | View → Imagery layers… | TMS URL template, async tiles, cache, opacity, persistence |
| Command history | Right-side "Command history" panel | Shows undoable commands; clicking undoes to before that step |
| Validation rules +5 | Validation panel | way>2000, closed way<4, relation missing type, duplicate members, invalid layer |
| Read-only/hidden layers | Layers panel | Canvas, menus, shortcuts, tags, and delete are all intercepted consistently |
| Background import/autosave | Open file/60s | PBF/XML import and autosave moved off the UI thread |

## 4. Gap list vs. JOSM/iD (by priority)

### P0: walls a professional user hits within a week

1. **Multiple data layers + active layer semantics**: the active layer is editable, docked layers
   are rendered read-only; create an empty layer, open as layer, switch, show/hide, delete; docked
   layers are saved to `%LOCALAPPDATA%/NEO/layers/*.osm` and restored automatically on startup.
2. **Imagery layers TMS/WMS + offset calibration**: TMS tiles and WMS single images are both wired
   up (async, cache/single image, opacity, X/Y pixel offset, persistence); sharing of the offset
   database is not done.
3. **Upload selection/chunking/after-upload cleanup**: upload selection is implemented; changes are
   automatically chunked at 4000 elements, new objects are split by reference groups, and partial
   success applies the uploaded mappings while keeping the remaining changes. Oversized mixed
   create+modify changes refuse chunking (to avoid negative-ID reference errors across diffs);
   413 backoff/after-upload cleanup is not done.
4. **Per-field manual conflict merging**: per-object "take remote", "merge tags", "merge geometry"
   (take remote positions/nodes/members, keep local tags) + "keep local" are implemented; per-field
   checkboxes are still missing.
5. **Orthogonalize/align/distribute/mirror**: align, evenly distribute, mirror horizontally/vertically,
   unglue, and parallel copy are implemented; JOSM's orthogonalize / join areas / extrude / move node
   onto way are still missing.
6. **multipolygon assembly and holes**: outer/inner members of `type=multipolygon/boundary` are
   assembled and holes are really carved out with earcut; unclosed members are not automatically
   stitched for now.

### P1: daily complaints but workable around

7. Command stack/history panel (a basic version is implemented: shows undoable commands and rolls
   back on click; lacks a redo tree/coalescing display).
8. Search is still a full-table scan on the UI thread; the first search over 10M objects will stall;
   a background worker + inverted index is needed.
9. Pre-upload validator gate (implemented; can be disabled in settings).
10. Notes and changesets: Notes list/create/comment/close/locate are done; the changeset viewer can
    load server-side changesets (user/time/comments/tags/change count); the user profile page is not done.
11. MapCSS `text`/`icon-image`/`dashes`: `dashes`, `text` (label key), `text-color`, `font-size`,
    `icon-image` (PNG under assets) are drawn; SVG/remote icons are unsupported.
12. Plugins wired into the desktop app (install/list/enable/delete + WAT/WASM execution; on enable,
    load the capabilities declared in the manifest and run the on_load command; signing is still a
    self-hash inside the manifest).
13. Audio/cadastre/printing/OSM data checks (JOSM's long tail).
14. Installers and signing for the three platforms.

### P2: expert/long tail

15. Parallel way, follow line, extrude, join areas, unglue, move node onto way (parallel, unglue,
    orthogonalize, join areas, move onto, extrude are implemented; follow line is still missing).
16. Custom shortcuts/preset editing/style editor (the settings window has been expanded to pages for
    general/edit/validation/network/storage etc.).
17. Incremental validation of large files, LOD caching, GPU chunking.
18. 10M memory reduction (arena compact, tag dedup, index dedup).

## 5. Suggested acceptance walkthrough (next round)

- With real city data (Bremen/Monaco), walk through: open → box select → copy/paste → relation
  member editing → simplify/glue → validate → upload → 409 → re-download;
- Open NEO-exported `.osm/.osc` in JOSM and compare tags/version/timestamp/action;
- Round-trip tags for 100 common features using iD's `id-tagging-schema` presets;
- Record the undo step count and journal recovery result for each operation.

> This document is a living document: when an item is finished, flip its status to ✅ and leave a
> record in CHANGELOG/TODO, to avoid the drift of "README says done, code has nothing" happening again.
