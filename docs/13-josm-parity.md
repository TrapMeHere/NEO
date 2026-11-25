# 13 · JOSM behavior parity contract (JOSM Parity)

**Abstract**: This document defines "fully replacing JOSM" as a **verifiable behavior contract**, not a feature count. The only acceptance statement is: *an active OSM mapper can work for a week using only NEO, without opening JOSM, without changing existing habits, and without corrupting any data*. To make this decidable, the document gives (a) a menu/command-level parity matrix (including 9 working modes, panels, and two-key shortcut sequences), (b) data and format parity, (c) upload and conflict workflow parity, (d) plugin capability mapping, (e) 10 "must be true" completion criteria and the 8 most easily missed behaviors, (f) an explicit "not promised" scope. JOSM-side facts are taken from [`.research/josm-inventory.md`](../.research/josm-inventory.md), baseline **r19678 "26.09" (2026-11-05)**. Rule semantics are in the [12 validator](./12-validation.md), format parsing in [08](./08-io-pbf-xml-osc.md) and [10](./10-persistence-and-recovery.md), licensing and clean-room in [17](./17-licensing-and-governance.md), and how evidence is produced in [18](./18-testing-strategy.md).

## 1 Goals and scope

### 1.1 Operational definition of "fully replacing"

| Criterion | Meaning | Counterexample (does not count as achieved) |
|---|---|---|
| Habit preservation | Default shortcuts, menu positions, and panel docking behavior stay unchanged | "All the features are there, but download moved to a new menu" |
| Data lossless | Open→edit→save→upload→re-download, semantically equivalent field by field | Saving loses `action="modify"`, and reopening treats it as unmodified |
| Closed loop | Edit→validate→upload→409→resolve→re-upload→close changeset all within NEO | On conflict, prompts "please solve it with JOSM" |
| Keyboard-first | Every action has a default shortcut and is rebindable, including two-key sequences | Features reachable only with the mouse |
| Escapable | Within a week there is no need to fall back to JOSM | "Everything works except some kinds of relations" |

Feature counting ("197 actions, we implemented 150") **is not evidence**: the denominator is meaningless, and most of JOSM's value lies in `DataSet`/`Command` semantics and resources (presets/styles/rules).

**In scope**: all user-visible behavior of JOSM core — menus, the 9 working modes, panels, formats, upload/conflicts, sessions and autosave, imagery and offsets, headless CLI equivalents; plus the **capability** mapping of 2 P0 and 8 P1 plugins.

**Out of scope**: JVM scripting plugins, Java plugin binary compatibility, OSM API 0.7 (v0.7 exists only as a concept page on the OSM wiki, with no deployment spec), see §3.7.

## 2 Design

### 2.1 Matrix fields and priority semantics

Each parity entry is a **behavioral unit**, not a UI control: one entry may correspond to several menu items (e.g. "Update data/Update selection/Update modified" share the same re-download and merge semantics).

| Column | Meaning |
|---|---|
| Item (shortcut) | JOSM's menu label and default shortcut; two-key sequences are written `A A` |
| NEO module | Primary implementation crate (`apps/neo-desktop` only does assembly) |
| P | **P0** = without it, "a week without opening JOSM" does not hold; **P1** = everyday path, its absence is complained about but can be worked around; **P2** = long tail/expert options |
| Status | **Design** = behavior contract and interfaces settled; **Skeleton** = crate boundaries and types exist, logic stubbed; **Prototype** = minimal end-to-end runnable implementation; **Not started** = no contract yet |

> **Status snapshot (2026-11-15)**: `crates/*` contains only directory placeholders with no implementation, so this table is almost entirely "Design"; "Skeleton" and "Prototype" are currently empty sets. The matrix doubles as the delivery board.
>
> **2026-11-21 update**: the data/edit/relation/validation/sync/persistence main chain has landed; for actual completeness and the gap to iD/JOSM see **[feature-parity.md](feature-parity.md)**. This document remains as the JOSM behavior contract and long-term roadmap, and is no longer used as an implementation status board.

### 2.2 Priority decision rules

1. Is it on the "edit→upload" main chain (download/edit/validate/upload/conflict)? → P0.
2. Does its absence cause **data corruption or upload failure**? → P0 (e.g. reversing a way without correcting directionality tags).
3. Is it marked expert in the JOSM docs, or does it serve only a single audience (audio/cadastre/print) → P1/P2; is it "user muscle memory as a resource" (preset tree, default styles, 9 modes) → P0.

### 2.3 How to verify parity itself

| Means | Coverage | Frequency |
|---|---|---|
| Workflow script (§4.1) | End-to-end loop, habit fidelity | Every milestone + P0 changes |
| golden comparison ([18](./18-testing-strategy.md) §2.3) | Same input → object set/OSC semantic equivalence | Every PR |
| Dual-run against the JOSM headless CLI | Normalized differences in `render`/`validate` output | nightly (black-box invocation, no code linking) |
| Manual dogfood weekly report | Status column progress and habit breaks | Every week |

### 2.4 P0 summary (row-by-row consistent with the tables in §3)

| Area | Entries | P0 | P1 | P2 |
|---|---|---|---|---|
| Menu/commands/modes/panels (§3.1) | 55 | **35** | 14 | 6 |
| Data/formats (§3.2) | 13 | **9** | 3 | 1 |
| Upload/conflicts (§3.3) | 7 | **6** | 1 | 0 |
| Plugin capabilities (§3.4) | 8 | **2** | 5 | 1 |
| Total | 83 | **52** | 23 | 8 |

**Command-level P0 = 35** (the minimum decidable set for "a week without opening JOSM"); **full-scope P0 = 52** (including formats, upload, and plugin capabilities). Both numbers are usable; when reporting progress you must state which scope is meant.

## 3 Key matrices

### 3.1 Menu / command / mode / panel parity

**File (Alt+F) — download and upload live here, not in Data**

| Item (shortcut) | NEO module | P | Status |
|---|---|---|---|
| Open…/Open Location…/Recent (`Ctrl+O`,`Ctrl+L`) | neo-io · neo-ui | P0 | Design |
| Save/Save As (`Ctrl+S`,`Ctrl+Shift+S`) | neo-io · neo-persist | P0 | Design |
| Download Data…/Current View/Object/Parent way-relation (`Ctrl+Shift+↓`,`Ctrl+Shift+O`,`Ctrl+Alt+D`) | neo-api · neo-ui | P0 | Design |
| Update Data/Selection/Modified (`Ctrl+U`,`Ctrl+Alt+U`,`Ctrl+Alt+M`) | neo-api · neo-editor | P0 | Design |
| Upload Data/Upload Selection (`Ctrl+Shift+↑`,`Ctrl+Alt+Shift+U`) | neo-api · neo-editor | P0 | Design |
| Save Session/Save As (`Ctrl+Alt+S`,`Ctrl+Alt+Shift+S`) | neo-persist | P0 | Done (desktop buttons; shortcuts not yet wired) |
| New Layer/New Local (`Ctrl+N`,`Ctrl+Shift+N`)/Download Along (`Alt+Shift+D`)/Close changeset (`Ctrl+Alt+Q`)/Delete Layer (`Ctrl+F4`)/Offline/Exit | neo-ui · neo-api | P1 | Design |
| Export GPX… (`Ctrl+E`) | neo-io | P2 | Not started |

**Edit (Alt+E) — the core is small: undo/clipboard/merge/search/preferences**

| Item (shortcut) | NEO module | P | Status |
|---|---|---|---|
| Undo/Redo (`Ctrl+Z`,`Ctrl+Y`) | neo-editor | P0 | Design |
| Copy/Paste/Paste at Source Position/Paste Tags (`Ctrl+C`,`Ctrl+V`,`Ctrl+Alt+V`,`Ctrl+Shift+V`) | neo-editor | P0 | Design |
| Duplicate (`Ctrl+D`)/Delete (`Del`) | neo-editor | P0 | Design |
| Search… (`Ctrl+F`)/Search by preset (`Shift+F3`) | neo-style · neo-ui | P0 | Design |
| Preferences (`F12`) | neo-ui | P0 | Design |
| Purge (`Ctrl+Shift+P`, expert)/Merge Layer · Merge Selection (`Ctrl+M`,`Ctrl+Shift+M`) | neo-editor | P1 | Design |
| Copy Coordinates/Copy Server URL | neo-ui | P2 | Not started |

**View (Alt+V) and Selection (Alt+N)**

| Item (shortcut) | NEO module | P | Status |
|---|---|---|---|
| Zoom family: Zoom In/Out/To Data/Layer/Selection/Conflict/Download/Problem/Previous-Next (`+`,`-`,`1`–`6`,`8`,`9`) | neo-ui · neo-scene | P0 | Design |
| Map Paint style toggle (including default elemstyles) | neo-style · neo-scene | P0 | Design |
| Download area boundary display + no editing outside (hatched semantics) | neo-ui · neo-editor | P0 | Design |
| Expert mode toggle (**gating menu items and toolbars**, not decoration) | neo-ui | P0 | Design |
| Select All/Deselect All/Invert Selection (`Ctrl+A`,`Esc`) | neo-editor | P0 | Design |
| Advanced Info/Web Advanced Info/History (`Ctrl+I`,`Ctrl+Shift+I`,`Ctrl+H`) | neo-ui · neo-api | P1 | Design |
| Non-branching way sequence selection (`Shift+W`) | neo-tools | P1 | Not started |
| Wireframe/Tile Rendering/Grid/Viewport Follow (`Ctrl+Shift+F`)/Fullscreen (`F11`)/Dialog Panel (`Tab`)/Shared Child Objects/Select by Inner Point | neo-render · neo-ui | P2 | Not started |

**Mode (Alt+M) — 9 working modes and mouse semantics**

| Mode (shortcut) | Key mouse/modifier semantics | NEO module | P | Status |
|---|---|---|---|---|
| Select rectangle (`S`) | Click select/box select, drag = move, `Ctrl` appends | neo-tools | P0 | Design |
| Lasso free (`S S`) | Same as above, freehand polygon | neo-tools | P0 | Design |
| Draw (`A`; `A A` angle snap) | Draw a way point by point, snapping to existing nodes/segments | neo-tools | P0 | Design |
| Zoom rectangle | Box zoom, `Alt` reverses | neo-ui | P0 | Design |
| Delete (`Ctrl+Del`) | Delete an object **or** a segment of a way (split semantics) | neo-tools | P0 | Design |
| Parallel (`Shift+P`) | Generate a parallel copy while drawing (road duplication) | neo-tools | P1 | Not started |
| Extrude (`X`; `X X` double-align) | Extrude/adjust way segments | neo-tools | P1 | Not started |
| Improve Way Accuracy (`W`) | Drag existing nodes, add/delete nodes (with preview) | neo-tools | P1 | Not started |
| Split (`T`, since r18759) | Split a way at a point | neo-tools | P1 | Not started |

**Tools (Alt+T) — the geometry core lives here**

| Item (shortcut) | NEO module | P | Status |
|---|---|---|---|
| Split way (`P`)/Combine way (`C`) | neo-tools · neo-editor | P0 | Design |
| Reverse way (`R`) **+ directionality tag correction** | neo-tools | P0 | Design |
| Orthogonalize (`Q`) | neo-tools | P0 | Design |
| Merge nodes (`M`)/Join node to way (`J`)/Move node onto way (`N`)/Disconnect (`Alt+J`)/Unglue (`G`) | neo-tools | P0 | Design |
| Create/Update multipolygon (`Ctrl+B`,`Ctrl+Shift+B`) | neo-tools · neo-editor | P0 | Design |
| Simplify way (`Shift+Y`)/Align to circle·line (`O`,`L`)/Distribute nodes (`Shift+B`)/Create circle (`Shift+O`)/Merge overlapping areas (`Shift+J`) | neo-tools | P1 | Not started |
| Mirror (`Shift+M`)/Follow line (`F`, expert)/Add node (`Shift+D`)/Move node | neo-tools · neo-ui | P2 | Not started |

**Data (Alt+D) and Imagery (Alt+I)**

| Item (shortcut) | NEO module | P | Status |
|---|---|---|---|
| Data menu: **zero core entries, purely a plugin host** (occupied by reverter/buildings/utilsplugin2) | neo-plugin · neo-ui | P1 | Design |
| TMS/WMS/WMTS layers + 5 built-in imagery (Bing/Esri/Esri Clarity/Mapbox/OSM Carto) | neo-render · neo-ui | P0 | Design |
| New offset (adjustment dialog) + **per-imagery-id offset persistence** | neo-ui · neo-persist | P0 | Design |
| Get/Store imagery offsets (`Ctrl+Alt+I`, offset DB protocol, XML not JSON) | neo-api | P1 | Not started |
| Imagery preferences/layer reordering/context list filtered by bounds+zoom | neo-ui | P1 | Not started |

**Windows (Alt+W) — panel list**

| Panel (shortcut) | NEO module | P | Status |
|---|---|---|---|
| Layers (`Alt+Shift+L`) | neo-ui | P0 | Design |
| Tags/Memberships (`Alt+Shift+P`) | neo-ui | P0 | Design |
| Selection (`Alt+Shift+C`) | neo-ui | P0 | Design |
| Relations (`Alt+Shift+R`) | neo-ui · neo-editor | P0 | Design |
| Conflict (JOSM docs share the same key as Selection; NEO needs its own unique key) | neo-ui · neo-editor | P0 | Design |
| Validator Results (`Alt+Shift+V`, including locate/previous-next jump/fix) | neo-ui · neo-validate | P0 | Design |
| Filter (`Alt+Shift+F`, E/H/I/M + R/A/D/F semantics) | — | — | Removed (not doing it) |
| Map Paint Styles (`Alt+Shift+M`)/Command Stack (`Alt+Shift+O`)/Changeset Manager (`Ctrl+Alt+C`) | neo-ui · neo-api | P1 | Design |
| Notes / Mini Map / Authors / Geotagged Images (`Y`) | neo-ui | P2 | Not started |

**Help (Alt+H)**

| Item | NEO module | P | Status |
|---|---|---|---|
| Menu item search (fuzzy find features)/shortcut reference page | neo-ui | P1 | Not started |
| Embedded help/status report/report bug/About | neo-ui · neo-core | P2 | Not started |

### 3.2 Data and format parity

| Format/capability | Contract points | NEO module | P | Status |
|---|---|---|---|---|
| OSM XML 0.6 read | `bounds`, negative ids, **JOSM private `action="modify\|delete"`** | neo-io | P0 | Design |
| OSM XML write | Two modes: server-conformant and JOSM-conformant (`visible`/`action`/`changeset`) | neo-io | P0 | Design |
| PBF read | `DenseNodes`, `granularity`/`date_granularity`, no mixed types within a group, `HistoricalInformation` rejected or explicitly downgraded | neo-io | P0 | Design |
| OsmChange 0.6 read/write | Outer `osmChange version="0.6"`, create/modify/delete, `if-unused`, negative id placeholders (**no 0.3 spec**) | neo-io | P0 | Design |
| `.jos`/`.joz` sessions | XML and ZIP containers; tolerate unknown plugin layer sections | neo-persist | P0 | Done (minimal compatibility: layer references/viewport/selection; imagery parameter sections ignored) |
| autosave | Default 300 s, `filesPerLayer=1`, restore prompt on startup, delete layer backups | neo-persist | P0 | Design |
| presets XML | The unmodified `defaultpresets.xml` (660 KB) can be loaded: `group/item/chunk/reference`, `key/text/combo/multiselect/roles`, `type` combinations (`multipolygon` is not a `relation`) | neo-style | P0 | Design |
| MapCSS rendering | The unmodified `elemstyles.mapcss` can be loaded: `::layer`, `z-index`, the full pseudo-class set, `∈`-family operators | neo-style · neo-scene | P0 | Design |
| validator mapcss | The 12 core files can be loaded and produce equivalent issues ([12](./12-validation.md) §3.1) | neo-validate | P0 | Design |
| PBF write | Export `.osm.pbf` (JOSM core is read-only, so it is a plugin capability there; building it into NEO is more reasonable) | neo-io | P1 | Not started |
| GPX / projection | GPX read/write; EPSG definitions, NTv2 `.gsb` (**has separate terms of use**) | neo-io · neo-geo | P1 | Not started |
| Imagery offset XML | Query-string style `get?lat=&lon=`/`store?…`, returns `<offset>` XML | neo-api | P1 | Not started |
| Translation `.lang` | Key compatibility for the 48 `.lang` files; missing ones fall back to English | neo-core | P2 | Not started |

### 3.3 Upload and conflict workflow parity

| Item | Contract points | P | Status |
|---|---|---|---|
| changeset lifecycle | Lazy creation, reused during upload, close (`Ctrl+Alt+Q` closes all open ones) | P0 | Design |
| changeset tags | `comment` (≤255, too-short validation, `Ctrl+Enter` submits) + `source` (can be auto-obtained) + **extract `hashtags` from the comment** (on by default) | P0 | Design |
| Hook chain order | `ApiPreconditionChecker → DiscardTags → FixData → ValidateUpload → RelationUploadOrder` (new relations last) | P0 | Design |
| `diffResult` | Per-element `<node old_id new_id new_version>`; rewrite negative placeholder ids to real ids | P0 | Design |
| 409 transaction semantics | The server **aborts the whole transaction at the first error**, so the affected objects must be re-downloaded and then merged; the upload cannot simply be retried; another kind of 409 = changeset already closed | P0 | Design |
| Conflict resolution UI | Four dimensions (Properties/Tags/Nodes/Members), three columns (My/Merged/Their) + **Freeze** for list conflicts + `Apply Resolution` only when everything is green | P0 | Design |
| Server limits | Handled in the same batch as upload: `capabilities` (waynodes 2000 / members 32000 / node set 10000), 413 (bbox over limit), and 429 (rate limiting) each get an actionable message | P1 | Design |

### 3.4 Plugin parity strategy

JOSM plugins **cannot** be bytecode-compatible: they compile against Java internal types such as `OsmPrimitive`/`MapMode`/`Command`/`MainApplication`/`Config`. NEO's "plugin compatibility" can only mean **capability reimplementation + an equivalent extension API** (WASM, see [15](./15-plugin-system.md)).

| Capability (JOSM plugin) | Mapped to NEO | P | License needs separate confirmation |
|---|---|---|---|
| Building drawing (`buildings_tools`: rectangle/circle, `Alt+R`/`Alt+Z`/`Ctrl+Alt+B`, merge address points) | neo-tools core capability (building tracing is a primary activity, so it cannot be an optional plugin) | P0 | Yes |
| Misc tool set (`utilsplugin2`: More Tools, tag multiple objects `Ctrl+T`, align ways) | neo-tools "More Tools" module | P0 | Yes |
| `terracer` terraced houses / `FastDraw` quick mouse drawing / `measurement` length·angle·area | neo-tools | P1 | Yes |
| `pt_assistant` public transport schema v2 validation and fixing | neo-validate rule pack ([12](./12-validation.md) VAL-S03) | P1 | Yes |
| `reverter` roll back a changeset | neo-api + neo-editor (depends on changeset download and conflict resolution) | P1 | Yes |
| `reltoolbox` relation/multipolygon editing panel | neo-ui + neo-editor | P1 | Yes |
| `PicLayer` image basemap alignment / `imagery_offset_db` offset sharing | neo-render + neo-api | P1 | Yes |
| `scripting`/`todo`/`turnlanes`/`CommandLine`/`o5m`/`print`/`opendata` long tail | As needed; **never promise `scripting`** | P2 | Yes |

**Conclusion**: the scope is "two P0 capability families into the core + eight P1 capabilities into capability packs", not "supporting 233 JOSM plugins".

### 3.5 The 10 "must be true" completion criteria

| # | Criterion |
|---|---|
| 1 | Round-trip lossless: `.osm/.osc/.osm.pbf/.gpx/.jos/.joz` open→edit→save→upload→re-download, semantically equivalent field by field (version, negative id, `action`, visible/deleted state) |
| 2 | Complete edit→upload→conflict loop (comment/source/hashtag, hook order, `diffResult`, 409→re-download→four-dimension conflict→re-upload, close changeset, upload selection) |
| 3 | Tools geometry tools reach JOSM semantics (split/combine/reverse including tag correction/simplify/align/distribute/orthogonalize/mirror/follow line/join/move onto/unglue/join areas/multipolygon) |
| 4 | The 9 working modes with their modifier keys and snapping behavior |
| 5 | The unmodified `defaultpresets.xml` + `elemstyles.mapcss` + 12 `.validator.mapcss` files can load and produce **something recognizable as the same map and the same set of warnings**, with working fixes |
| 6 | Panels complete and dockable, shortcuts identical to JOSM |
| 7 | Keyboard-first: default shortcut set + two-key sequences + all rebindable, with no mouse-only actions |
| 8 | Imagery: TMS/WMS/WMTS + 5 built-ins, offset adjustment, per-imagery persistence, offset DB get/store |
| 9 | Sessions and recovery: mixed layer types save/load, autosave restore prompt |
| 10 | Headless: a `render`/`project`/`validate` equivalent CLI usable for scripting |

### 3.6 The 8 most easily missed behaviors

| # | Behavior | Consequence of missing it |
|---|---|---|
| 1 | **Reversing a way must correct directionality tags** (`oneway=yes→-1`, swap `:forward`/`:backward`, flip the sign of `incline`) | Silently creates bad data, which is worse than a crash |
| 2 | **relation member order is data**, not display order | Routes/polygons get reordered and their semantics change |
| 3 | **area ≠ closed way** (`area=no`, closed but linear ways; MapCSS `area` deliberately matches unclosed ways) | Area objects are misclassified; both styling and rules go wrong |
| 4 | **Global undo**: a single stack across layers (each command holds its own dataset), switching layers does not clear the stack, includes the undoable SelectCommand; after upload it must block/warn correctly | Undo behavior diverges from expectations |
| 5 | **Offsets persist per imagery id** (the id is derived from the URL) and must survive a restart even without the offset DB | Re-aligning the imagery every time |
| 6 | **validator suppression persists**: "ignore for now" and "ignore this whole class" are different, and both work across sessions | Noisy at every startup, so the user turns the validator off |
| 7 | **Filter semantics** (removed, not doing it: NEO no longer hides objects, and selection does not depend on render state) | — |
| 8 | **Expert mode is feature gating** (Purge, Copy Coordinates, Mirror, Follow Line, Add/Move Node, Changeset Manager, Jump To Position, Tiled Rendering…) | Hides the wrong things → advanced users think the feature has disappeared |

### 3.7 The explicit "not promised" scope

| Not promised | Rationale |
|---|---|
| Java plugin binary compatibility | Depends on JOSM Java internal types; a Rust host cannot load them, so capability reimplementation is the only option (§3.4) |
| JVM scripting plugins (`scripting`: JSR-223/GraalVM) | No realistic counterpart; the replacement is an optional embedded scripting engine + documented API, and it is **not listed in parity** |
| OSM API 0.7 | Only a wiki concept page, with no deployment spec; JOSM itself hardcodes `0.6` |
| "OSC 0.3" | No authoritative spec; the reality is `osmChange version="0.6"` |
| Pixel-perfect UI consistency | Uses egui instead of Swing; aligns behavior and shortcuts, not control appearance |
| Distributing a GPL version that shares resources with JOSM | Take the wiki CC-BY-SA+LGPL re-acquisition route ([17](./17-licensing-and-governance.md) §2.3) |

## 4 Process and sequence

### 4.1 The "one-week workflow test" script

```
Day 0  Install NEO (without a JOSM shortcut), import existing preferences (shortcuts/imagery offsets/suppressions)
Day 1  Download bbox → trace 20 buildings → orthogonalize → add addresses → upload (comment+hashtag)
Day 2  Trace a river (non-branching selection) → split/combine/reverse (including oneway fixes) → upload
Day 3  Edit a public transport relation (member order, roles) → validate → upload; hit 409 → four-dimension conflict resolution → re-upload
Day 4  Import a local .osm + reference GPX/imagery → adjust offsets → save session (.joz) → restart → recover
Day 5  Create errors (self-intersection/duplicate nodes/unclosed area) → fix 3 / suppress 2 → restart and confirm the suppressions persist
Day 6  Complete one full edit entirely by keyboard: multi-select/box select + tool switching + property editing; upload selection (partial) → close changeset
Day 7  Headless validate/render produces a report and an image; reproduce today's state on another machine with the `.joz` session
```

Record three kinds of events every day: **habit breaks** ("I instinctively pressed a key and nothing happened / the wrong thing happened"), **data doubts** ("not sure whether saving is lossless"), **rollback urges** ("this needs JOSM"). Any P0-level break counts as that milestone not being achieved.

### 4.2 Advancing parity

```
Entry/behavior change ─► matrix (P+status=Design) ─► implementation+tests ─► Skeleton ─► Prototype
                                              │
        nightly dual-run comparison vs JOSM CLI ─────────►┴─► golden/workflow pass ─► Achieved
```

The **only basis** for upgrading a status is tests passing, not code being merged; "achieved" is signed by CI artifacts (golden hashes, workflow logs).

## 5 Trade-offs and alternatives

| Decision | Choice | Rejected option | Rationale / cost |
|---|---|---|---|
| parity definition | Workflow contract (a week without opening JOSM) | Feature counting, control-by-control comparison | Counting is unfalsifiable; the cost is needing real-human dogfood, which is expensive and not fully automatable |
| Counterintuitive behavior | **Copy it** (download in File, geometry in Tools, empty Data menu) | "Take the opportunity to reorganize into a more sensible menu" | Habit is contract; the cost is that in self-review the menu always feels unreasonable |
| Plugin compatibility | Capability reimplementation + WASM API | Attempt Java interop/transpilation | Compile-time dependencies on Java internal types make it unsolvable; the cost is that 233 plugins are covered by only 10 families |
| Scripting | Optional embedded scripting (no promise of JSR-223 equivalence) | Embedded JVM | An embedded JVM violates the core selling point "no JVM startup"; the API only targets 0.6 (0.7 has no spec to align with, and a reserved abstraction layer is pure liability) |

## 6 Risks

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| J1 | Matrix granularity is too coarse, and P0 mixes in items that "could actually be omitted" | Delivery is diluted | Every P0 must have a case in the §4.1 script; without a case it is downgraded |
| J2 | Two-key sequences get simplified into single-key mappings | Habit breaks (`A A` stops working) | Design the shortcut engine as a "sequence state machine" |
| J3 | Counterintuitive behaviors get "well-intentioned" fixes | User misoperation | The matrix explicitly lists counterintuitive items and asserts menu positions |
| J4 | Resource version drift (wiki presets/styles lag behind the current JOSM release); the week-long test relies on real humans and cannot be reproduced in CI | Differences misjudged as bugs; regressions found late | Asset manifests record origin and sha256 with comparison assertions; turn Day1/2/3/5/7 of §4.1 into end-to-end tests |
| J5 | Plugin capability pack licenses not confirmed item by item | Compliance blocks | Complete license confirmation before starting each family ([17](./17-licensing-and-governance.md) §2.4) |

## 7 Acceptance criteria

1. All **35 command-level P0** items in §3.1 have status "Achieved", each with at least one automated or scripted case passing; the **17 P0** items in §3.2 / §3.3 / §3.4 (9+6+2) are likewise achieved, and none of the full-scope **52 P0** items is "Not started".
2. The §4.1 script has been run once on each of **two unrelated mappers**, with habit breaks ≤ 2 and all of them registered as known deviations.
3. Every P0 item with counterintuitive semantics has a corresponding line-item check, otherwise review rejects it.
4. Dual-run comparison against r19678: for the same PBF input, the `validate` issue set and `render` output have zero difference after normalization (rendering uses a perceptual-hash threshold, see [18](./18-testing-strategy.md) §2.6).
5. Regression of any P0 status (Achieved → other) blocks the release.

## 8 References

- JOSM menu index: https://josm.openstreetmap.de/wiki/Help/Menu
- File (download/upload location): https://josm.openstreetmap.de/wiki/Help/Menu/File
- Tools (geometry core): https://josm.openstreetmap.de/wiki/Help/Menu/Tools
- Mode (9 modes and shortcuts): https://josm.openstreetmap.de/wiki/Help/Menu/Mode
- Data (zero core entries, plugin host): https://josm.openstreetmap.de/wiki/Help/Menu/Data
- Windows (panels and key conflicts): https://josm.openstreetmap.de/wiki/Help/Menu/Windows
- Conflict dialog (four dimensions/three columns/Freeze): https://josm.openstreetmap.de/wiki/Help/Dialog/Conflict
- Command-line options (`render`/`project`/`validate`): https://josm.openstreetmap.de/wiki/Help/CommandLineOptions
  > To verify: the `validate` subcommand option table (first-hand docs only give the usage block).
- Plugin list (233, including `Plugin-Mainversion`): https://josm.openstreetmap.de/plugin
- OSM API v0.6 (changeset/409/diff upload): https://wiki.openstreetmap.org/wiki/API_v0.6
- OsmChange format: https://wiki.openstreetmap.org/wiki/OsmChange
- API 0.7 status (concept page): https://wiki.openstreetmap.org/wiki/API_v0.7
- First-hand facts summary: [`.research/josm-inventory.md`](../.research/josm-inventory.md)
