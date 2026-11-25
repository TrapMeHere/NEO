# 12 · Validator Architecture and Rule System (Validation)

**Summary**: NEO's validator is not "a port of JOSM's 40 Java test classes" but a **parallel rule engine that takes an object dirty set as input and produces undoable fix commands as output**. Three kinds of consumers share one rule description: built-in Rust rules (needing a spatial index and cross-object graph), the `*.validator.mapcss` compatibility layer (declarative rules the community can write), and the server-capability pre-check before upload. `ValidationReport` is the single source of truth for the UI panel, the upload gate, and golden tests; every automatic fix must be expressed as a batch of `neo-editor` `EditCommand`s, and is therefore undoable by construction. The P0/P1/P2 split of rules is aligned with the union of JOSM's 12 core `.validator.mapcss` files and 40 Java tests (§3.2–3.3). For behavioral contracts see [13](./13-josm-parity.md), for the undo model see [06](./06-editing-core-and-undo.md), and all performance numbers are [pending measurement, see docs/14-performance-and-verification.md].

## 0 Built-in rules (current implementation, v0.1.0)

`neo-validate` currently has 24 rules (9 Error, 14 Warning, 1 Info; JOSM-style semantics, behavior aligned without copying code):

| Rule id | Severity | Description |
|---|---|---|
| `way_too_short` | Error | way has fewer than 2 nodes |
| `way_duplicate_node` | Warning | adjacent duplicate nodes |
| `way_untagged` | Warning | way has no tags |
| `building_not_closed` | Error | building way is not closed |
| `duplicate_way` | Warning | two ways with the same node sequence |
| `relation_empty` | Warning | relation has no tags |
| `relation_no_members` | Error | relation has no members |
| `untagged_node` | Info | isolated node with no tags |
| `duplicate_node` | Warning | nodes at exactly the same coordinates |
| `suspicious_tag_combination` | Warning | highway+building / natural=water+building / highway+railway |
| `multipolygon_bad_role` | Warning | multipolygon member role is not outer/inner/empty |
| `self_intersecting_way` | Error | way self-intersects (≤2048 nodes, O(n²)) |
| `way_too_long` | Error | way exceeds the OSM API's 2000-node limit |
| `closed_way_too_few_nodes` | Error | closed way has fewer than 4 nodes (cannot form an area) |
| `relation_type_missing` | Warning | relation has members but no `type` tag |
| `duplicate_relation_member` | Warning | relation references the same member twice |
| `invalid_layer_tag` | Warning | `layer` tag is not an integer |
| `area_not_closed` | Error | way with `area=yes` is not closed |
| `non_adjacent_duplicate_node` | Warning | the same node is referenced twice at non-adjacent positions |
| `oneway_invalid` | Warning | invalid `oneway` value |
| `empty_name` | Warning | `name` is empty |
| `layer_out_of_range` | Warning | `layer` outside the reasonable range (\|layer\| > 5) |
| `multipolygon_no_outer` | Error | multipolygon has no outer member |
| `multipolygon_outer_not_closed` | Error | multipolygon's outer way is not closed |

Not implemented yet: almost-junction, crossing ways, address/name tag rules, the MapCSS validator,
the pre-upload gate, incremental validation (by dirty set/bbox). Reference JOSM Validator's rule semantics and test data
without copying its GPL code.

## 1 Goals and Scope

**In scope**

| Item | Requirement |
|---|---|
| rule execution | at 1M–10M objects, incremental validation after an edit recomputes only the affected rule×target, without restarting a full scan |
| rule sources | built-in Rust rules + subset compatibility with unmodified JOSM core `.validator.mapcss` (12 files) |
| report | structured, localizable (object + sub-element), deduplicable, suppressible, exportable |
| fixes | each fixable issue produces one or more `EditCommand`s, merged into a single undo step |
| upload gate | errors block, warnings require confirmation; the order matches JOSM's upload hook chain |
| suppression | persisted at three levels — rule×object / rule×class / dataset — surviving sessions and autosave recovery |

**Out of scope**: porting JOSM Java test code (`data/validation/tests/*.java` is GPL-2.0+, readable only as a behavior inventory, see [17](./17-licensing-and-governance.md) §2.2); merging server-side validation and Osmose/KeepRight rules (consuming only what can be decided offline); the full MapCSS render cascade (that is `neo-style`); "zero false positives" — the goal is **the same input producing the same conclusion set**.

Two judgements at the scope boundary: ① **aim for conclusion parity, not rule-count parity** — using the `assertMatch`/`assertNoMatch` of the 12 core mapcss files as executable compatibility assertions (they are data, not code) is far more verifiable than "implemented N rules"; ② **the false-positive rate is a product metric** — suppression is a core feature, not a patch, because once users turn the validator off, parity fails in practice.

## 2 Design

### 2.1 Data Flow

```
user edit ─► neo-editor: EditTx ─► Journal(ObjectId+version) ─► Snapshot v(n) (arc-swap publish)
                                     │
                                     ▼
   RuleScheduler (UI thread, O(dirty set)): ① dependency expansion object→(rule,target) ② target expansion node→way→rel (≤2 hops)
                                      ③ spatial expansion old/new coordinate bbox⊕radius → rstar neighborhood ④ budget decision (dirty > 30% → full)
                                     ▼
   neo-validate::Engine (rayon par_iter, read-only &Snapshot): per-thread IssueBuf → sort → reduce → dedup → ReportStore
                                     ▼
   ValidatorDialog (filter/locate/fix) · upload gate (block/warn) · golden tests · suppression persistence (neo-persist)
```

### 2.2 Rule Trait and Scheduling Unit

Rules are **pure functions + explicit dependency declarations**: `deps()` is the sole basis for incremental correctness, and reading data elsewhere without declaring the dependency is a review red line.

```rust
pub trait ValidationRule: Send + Sync {
    fn id(&self) -> RuleId;                 // stable ID, used in suppression and goldens, must not be renamed after release
    fn scope(&self) -> Scope;               // Object | Reference | Global
    fn deps(&self) -> Deps;                 // determines how the dirty set expands and whether rules can run in parallel
    fn default_severity(&self) -> Severity; // Error | Warning | Other
    fn check(&self, cx: &RuleCtx<'_>, target: Target, out: &mut IssueBuf);
    fn prepare(&self, cx: &RuleCtx<'_>) -> Option<Box<dyn RuleIndex>> { None } // two-phase: duplicate detection/coastline
    fn fix(&self, cx: &RuleCtx<'_>, issue: &Issue) -> Option<FixPlan> { None }
}

pub enum Scope { Object, Reference, Global }
pub struct Deps { pub kind: DepKind, pub radius_m: f64, pub tag_keys: &'static [&'static str] }
// DepKind: None | NodesOfWay | MembersOfRelation | Closure | Neighborhood

pub struct RuleCtx<'a> {
    pub snapshot: &'a Snapshot,               // read-only, INV-2
    pub geo: &'a GeoIndex,                    // rstar 0.14 spatial index (neo-geo)
    pub presets: &'a PresetIndex,             // precompiled match expressions
    pub tags: &'a TagRuleIndex,               // selector index of the mapcss compatibility layer
    pub territory: Option<&'a TerritoryIndex>,// boundaries depended on by [inside("XX")]
    pub config: &'a ValidatorConfig,
}
```

`RuleId` and `Issue.code` are separate: the former identifies the implementation (possibly a mapcss file + line number), the latter is a stable, user-visible short code (`self_intersection`) used for suppression and documentation mapping.

### 2.3 Four Rule Categories and Execution Order

The order is determined by **cost and dependencies**, not by severity:

| Stage | Category | Typical rules | Complexity | Incremental friendliness |
|---|---|---|---|---|
| P1 | single-object geometry | self-intersection, duplicate nodes, degenerate ways, area semantics | O(n) | high (depends only on itself) |
| P2 | single-object tags | mutual exclusion, deprecated, numeric, required | O(tags) | high |
| P3 | references (1 hop) | way→nodes, relation→members | O(member count) | medium (member changes must propagate) |
| P4 | closure/global | multipolygon rings, duplicate detection, coastline | O(closure)/O(n) | low (requires index invalidation) |
| P5 | spatial neighborhood | crossing without shared node, dangling endpoints, overlapping ways | O(k·log n) | medium (must query old/new positions) |
| P6 | pre-upload check | server capabilities, changeset limits | O(upload set) | none needed (recomputed per upload) |

Within a stage, execution follows `RuleId` lexicographic order to guarantee deterministic output (a golden requirement); stages do not run in parallel (P4 index construction needs a consistent view after P1–P3 finish).

### 2.4 Incremental Validation: Reverse Dependency Index

```
dirty(v) = {node:42, way:17}                     ← the Journal transaction's object set
 ├ direct targets: node:42 → [T03(neighborhood), K15]; way:17 → [T01,T02,G04,K02]
 ├ reference propagation: node:42 ∈ way:17, way:31 → targets += way:17,31; way:17 ∈ rel:9 → targets += rel:9
 └ spatial propagation: bbox(old ∪ new) ⊕ radius → rstar query → way:88, node:99 → neighborhood rules T11/T09/T10
```

1. **Bounded propagation hops**: `node→way→relation` defaults to 2 hops; anything further (a relation that is a member of a relation) falls into `Scope::Global` and is recomputed as P4, avoiding propagation explosions.
2. **Spatial propagation uses the union of old/new coordinates**: querying only new positions would miss "the gap left behind after moving away" (adjacent ways were connected before, and become dangling after the node moves).
3. **Degrade rather than stall**: when the dirty-target ratio exceeds a threshold (default 30%) or a single pass is expected to exceed 50 ms, switch to a full pass and record `ValidationStats::degraded`; full-pass time [pending measurement, see docs/14-performance-and-verification.md].

Debounce: dragging coalesces the dirty set in a 120–200 ms window; the report is not rebuilt during the drag, only at transaction boundaries (mouse release).

### 2.5 Suppression Persistence

JOSM uses a "per-test ignore list" whose preference key has not been verified (> to verify, see [`.research/josm-inventory.md`](../.research/josm-inventory.md) Appendix B.4). NEO does not replicate its storage format, only its user-visible semantics, and additionally solves "still silent after the data changed":

```rust
pub struct SuppressionKey { pub rule: RuleId, pub target: ObjectKey, pub digest: u64, pub scope: SuppressScope }
// digest = relevant tag subset (Deps.tag_keys) + geometric class (closed/node count/direction); excludes coordinates and timestamps
```

| Attribute | Semantics | Rationale |
|---|---|---|
| `digest` | hashes only the declared keys and the geometric class | dragging a node should not invalidate suppression, but changing a tag should |
| `Object` | just this one issue | "I know about this one" (≈ ignore for now) |
| `Class` | silence this rule for all objects of that type | "I don't care about deprecated `created_by`" |
| `Dataset` | silence this rule for the whole dataset (imported legacy data) | but the report header must show "N suppressed" |
| storage | `neo-persist` SQLite `validation_suppression(dataset_fp, rule, type, id, digest, scope, created_at)` | same DB and transaction domain as autosave |
| observability | the report contains all issues; suppression filters only at the **presentation and gate** layers | avoids "suppression hides an error" |

Export/import as JSON (`neo-io::issues`) for moving between machines and reproducing regressions; **not written into `.jos`** (session files do not carry governance state).

### 2.6 Report Model, Localization, and Automatic Fixes

```rust
pub struct ValidationReport {
    pub generation: u64,                    // = Snapshot.version; the UI uses it to decide staleness
    pub stats: ValidationStats,             // rule count/target count/elapsed/degraded/suppressed
    pub issues: Arc<[Issue]>,               // stable order: severity desc, code, objects
    pub by_object: HashMap<ObjectKey, SmallVec<[IssueIdx; 4]>>,
    pub counts: Counts,                     // { error, warning, other, suppressed }
}
pub struct Issue {
    pub rule: RuleId, pub code: &'static str, pub severity: Severity,
    pub objects: SmallVec<[ObjectKey; 4]>,  // the first one is the primary object
    pub locate: Locate,                     // Node | WaySegment(u32) | Member(u32)
    pub message: String, pub fixable: bool,
}
pub struct FixPlan { pub steps: Vec<Box<dyn EditCommand>>, pub label: String }
```

Localization (`Locate`) is the prerequisite for navigation: `Zoom to problem` must turn `WaySegment(3)` into a viewport center and a highlighted segment, and `Member(7)` must select row 8 of the Relations panel — an object id alone cannot implement "next problem" (`6` to jump, `8`/`9` to step back and forth, see [13](./13-josm-parity.md) §3.1).

| Fix constraint | Content |
|---|---|
| single-step undo | `Editor::execute_batch(steps)` produces one user-visible undo step; after undo the data equals the pre-fix state field by field |
| idempotent | rerunning the same rule on the same target immediately after a fix must not produce the same `code` again (`FIX-IDEMPOTENT` invariant) |
| preview | the UI first `dry_run`s on a cloned `DataSetMut` to show the diff, then commits |
| batch | "fix all of this kind" = deterministic merge of FixPlans across targets (ordered by object id) |
| forbidden | a fix must not create new kinds of objects (e.g. auto-creating a relation); such cases only get suggestion text |

### 2.7 Pre-Upload Validation Gate

Corresponding to JOSM's hook chain (`ApiPreconditionChecker → DiscardTags → FixData → ValidateUpload → RelationUploadOrder`):

```
① capability pre-check (S04): way nodes ≤2000, relation members ≤32000, changeset elements ≤10000, bbox ≤0.25°² → if unmet, error and send no request
② discard tags (DiscardTags semantics): created_by etc., affecting only the OSC sent, not local data
③ FixData semantics: fill placeholder ids, remove empty ways
④ validation gate: scope = the upload set and its 1-hop neighbors (not the whole dataset) → errors block; warnings raise a confirmation dialog (remembering this choice)
⑤ relation upload order (neo-api's responsibility, not validation) → ⑥ POST /api/0.6/changeset/#id/upload
```

It is deliberate that the gate validates only the **upload set**: a whole-dataset error should not block a local upload, otherwise uploading with 10M objects would never be possible. But "new errors the upload set would introduce" must be counted — deleting a way can leave its parent relation empty (P3/P4 recompute the closure of the upload set).

## 3 Key Interfaces and Matrices

### 3.1 MapCSS Compatible Subset vs Native Rust Rules

| Statement / selector | NEO support | Notes |
|---|---|---|
| `throwError`/`throwWarning`/`throwOther` | ✅ | exactly one per selector; more is a parse error |
| `fixAdd:"k=v"`/`fixRemove:"k"`/`fixChangeKey:"old=>new"`/`fixDeleteObject:this` | ✅ | mapped to `ChangePropertyCommand`/`DeleteCommand` (reference check needed) |
| `suggestAlternative`/`group` | ✅ | group affects panel grouping, not the gate |
| `assertMatch`/`assertNoMatch` | ✅ as tests | parsed into golden assertion corpora, executed by `cargo test -p neo-validate` |
| placeholders `{0.tag}` `{i.key}` `{i.value}` | ✅ | includes "pseudo-classes count in the ordinal" semantics (`:unconnected`=0, `:in-downloaded-area`=1) |
| type selectors `node/way/relation/area/*` | ✅ | `area` keeps JOSM's counter-intuitive "includes unclosed ways" semantics |
| conditions `= != < > <= >= ^= $= *= ~= =~ !~` | ✅ | `~=` semicolon list membership, regex `/…/` |
| set/spatial `∈ ⊆ ⊇ ⊈ ⊉ ⧉` / `[inside("FR")]` / `outside(...)` | ✅ (`⧉` needs `layer`; territory needs `boundaries.osm`) | missing territory index → the whole class of rules is skipped and logged |
| pseudo-classes `:closed :new :tagged :unconnected :connection` etc. | ⚠️ subset | `:clockwise/:anticlockwise/:open_end` P1; `:area-style` not applicable |
| child selectors `relation[type=route] >[role="link"] way` | ✅ P1 | requires member-graph evaluation |
| `@supports (min-josm-version:N)` | ⚠️ mapped to an NEO version gate | `@media` is deprecated and not implemented |
| `eval()`/arbitrary expressions, `setting()`, `prop("_match")` | ❌ | avoids embedding an expression evaluator |

**Division principle**: anything decidable from "a single object's tag combination" goes through mapcss (community-maintainable, hot-reloadable, can come from the wiki's `Rules/*`); anything needing a spatial index, graph traversal, closure analysis, or cross-object state uses a Rust rule. Both produce the same `Issue` type, and the UI does not distinguish the source.

### 3.2 Topology and Geometry Rule Inventory (NEO rule IDs)

Severity: `E`=Error (blocks upload) · `W`=Warning (requires confirmation) · `O`=Other (list only). Priority is delivery order, not severity.

| ID | Detection | JOSM counterpart | Severity | Incremental scope | Fix | P |
|---|---|---|---|---|---|---|
| VAL-T01 | way self-intersection | `SelfIntersectingWay`/geometry | E | single way | suggest splitting at the intersection (two steps) | P0 |
| VAL-T02 | duplicate nodes within a way | `DuplicatedWayNodes` | E | single way | deduplicate nodes, keep tags | P0 |
| VAL-T03 | duplicate nodes at the same coordinates | `DuplicateNode` | W | neighborhood ~1e-7° | merge nodes and redirect references | P1 |
| VAL-T04 | area not closed | `UnclosedWays`/geometry | W | single way | append the first node to close it | P0 |
| VAL-T05 | fully duplicate way/relation | `DuplicateWay`/`DuplicateRelation` | W | full hash buckets | merge or delete one | P1 |
| VAL-T06 | relation member missing/not loaded | `RelationChecker` | E | single relation+members | trigger "download incomplete members" | P0 |
| VAL-T07 | duplicate members / illegal role | relation.mapcss/`RelationChecker` | W | single relation | deduplicate members | P1 |
| VAL-T08 | multipolygon ring unclosed or wrongly nested | `MultipolygonTest`/`CycleDetector` | E | relation closure | rebuild member order | P0 |
| VAL-T09 | dangling endpoints | `UnconnectedWays`/`:unconnected` | W | neighborhood | none (hint to connect only) | P1 |
| VAL-T10 | overlapping ways | `OverlappingWays` | W | neighborhood | none | P2 |
| VAL-T11 | crossing without a shared node | `CrossingWays` | W | neighborhood | insert a shared node at the intersection | P0 |
| VAL-T12 | discontinuous coastline | `Coastlines` | E | full ordered chains | none | P1 |
| VAL-T13 | way connecting into the interior of an area | `WayConnectedToArea` | W | neighborhood | none | P2 |
| VAL-G01 | sharp angles | `SharpAngles` | W | single way | none | P2 |
| VAL-G02 | overly long segments | `LongSegment` | W | single way | none | P2 |
| VAL-G03 | building not right-angled | `RightAngleBuildingTest` | W | single way | local orthogonalize | P1 |
| VAL-G04 | area semantics contradict closure/direction (`area=no`, closed but linear) | geometry.mapcss | W | single way | add/remove the `area` tag | P0 |
| VAL-G05 | degenerate geometry (<2 nodes, zero-length way) | — (scattered in JOSM) | E | single way | delete | P0 |
| VAL-G06 | coordinates outside the downloaded bbox | `:in-downloaded-area` | W | single object | none | P1 |
| VAL-G07 | ring direction (outer/inner) reversed | `:clockwise` family | W | relation closure | reverse the way | P1 |

### 3.3 Tag and Semantic Rule Inventory

| ID | Detection | JOSM counterpart | Severity | Incremental scope | Fix | P |
|---|---|---|---|---|---|---|
| VAL-K01 | required preset key missing | `TagChecker`/unnecessary | W | single object+presets | none (open the preset) | P0 |
| VAL-K02 | mutually exclusive tag combination | `combinations.mapcss` | E | single object | remove one side | P0 |
| VAL-K03 | deprecated tag | `deprecated.mapcss` | W | single object | `fixChangeKey`/`fixAdd` | P0 |
| VAL-K04 | numeric/unit format | `numeric.mapcss` | W | single object | normalize (`50 km/h`→`50`) | P1 |
| VAL-K05 | duplicate values/empty segments in semicolon lists | `multiple.mapcss` | W | single object | deduplicate | P1 |
| VAL-K06 | redundant/implied tags | `unnecessary.mapcss` | O | single object | delete | P2 |
| VAL-K07 | wikipedia/wikidata link format | `wikipedia.mapcss` | W | single object | add the language prefix | P1 |
| VAL-K08 | incomplete address (housenumber/street interdependence) | `addresses.mapcss` | W | single object | none | P0 |
| VAL-K09 | `conditional:*` syntax | `ConditionalKeys` | O | single object | none | P2 |
| VAL-K10 | `opening_hours` syntax | `OpeningHourTest` | O | single object | none | P2 |
| VAL-K11 | lanes/`turn:lanes` count consistency | `Lanes`/highway.mapcss | W | single way | none | P2 |
| VAL-K12 | name inconsistent with `name:xx`, similar names | `NameMismatch`/`SimilarNamedWays` | O | neighborhood | none | P2 |
| VAL-K13 | religion/denomination combination | `religion.mapcss` | O | single object | none | P2 |
| VAL-K14 | country-specific rules `inside("XX")` | `territories.mapcss` | W | single object+territory | depends on the rule | P1 |
| VAL-K15 | untagged node | `UntaggedNode` | W | single object | delete | P0 |
| VAL-S01 | semantic conflicts such as highway+building | `combinations.mapcss` | E | single object | none | P0 |
| VAL-S02 | turn-restriction structure | `TurnrestrictionTest` | E | single relation | none | P1 |
| VAL-S03 | public transport route schema v2 | `PublicTransportRouteTest`/pt_assistant | W | single relation | none | P1 |
| VAL-S04 | pre-upload server capability check | `ApiCapabilitiesTest` | E | upload set | none (blocks) | P0 |
| VAL-S05 | directional tags inconsistent with way direction | `ReverseWayTagCorrector` semantics | W | single way | flip the directional tag | P1 |
| VAL-S06 | abnormal route member order | `WronglyOrderedWays` | W | single relation | reorder members | P2 |
| VAL-S07 | power-line crossing | `PowerLines` | W | neighborhood | none | P2 |
| VAL-S08 | barrier/entrance combination | `BarriersEntrances` | W | neighborhood | none | P2 |

P0 totals **15 rules**: T01/T02/T04/T06/T08/T11, G04/G05, K01/K02/K03/K08/K15, S01/S04 — the minimal set for which "turning the validator off means you cannot upload safely"; the other 28 are for experience and community consistency.

## 4 Flow and Timing

### 4.1 Drag → Incremental Validation (non-blocking within a frame)

```
UI         Journal      Scheduler                        rayon pool        ReportStore
│ drag node  │            │                                 │                  │
├──────────►│ txn(Move)  │                                 │                  │
│           ├ dirty={node42} (120ms debounce)               │                  │
│           ├───────────►│ dependency expansion → target={node42,way17,way31,rel9,nbhd}
│           │            ├────────────────────────────────►│ par_iter(rule×target)
│           │            │                                 │ IssueBuf→reduce→sort
│           │            ├─────────────────────────────────┼─────────────────►│ v(n)
│◄──────────┴────────────┤ report ready(generation)         │                  │
│ ValidatorDialog repaints only changed rows                                  │
```

When there is no dirty set nothing is scheduled (early return on `dirty.is_empty()`), which guarantees zero overhead when idle.

### 4.2 Fix and Undo

```
user clicks "fix" → rule.fix(cx,issue) → FixPlan{steps}
  → Editor::execute_batch(steps) (single transaction, single undo step) → journal record (autosave/recovery capable)
  → publish Snapshot(v+1) → Scheduler(dirty = touched objects) → report refresh
  → Ctrl+Z → data returns field by field to the pre-fix state → the original issue reappears
```

### 4.3 Upload Gate Decision

```
upload() ─► ① capability pre-check ─┬ fail → block, report S04, do not create a changeset
                                    └ ok ─► ④ gate(upload-set closure) ─┬ error>0 → block (can jump/fix)
                                                                        ├ warning>0 → confirmation dialog (cancel by default)
                                                                        └ clean → ⑤ order ─► ⑥ POST
```

## 5 Trade-offs and Alternatives

| Decision | Choice | Rejected option | Rationale and cost |
|---|---|---|---|
| rule language | two tracks: Rust traits + mapcss compatibility layer | mapcss only (push as much down as JOSM does) | spatial/graph rules cannot be expressed in mapcss (JOSM also has to write them in Java); the cost is two test suites and two sets of docs |
| incremental model | reverse dependency index + spatial propagation | full pass every time (simple, always correct) | a full pass at 10M is unacceptable; the cost is that index-invalidation bugs manifest as "missed reports", so `--full` and nightly full comparisons are mandatory |
| suppression granularity | three levels with `digest` | copy the per-test ignore list | JOSM's semantic details are unverified, and "still silent after data changes" is a real pain point; the cost is that digest must be strictly limited to `tag_keys` |
| gate scope | upload-set closure | whole dataset | otherwise a wide-area error permanently blocks local uploads; the cost is that pre-existing errors are surfaced via a persistent status-bar counter |
| fix execution | generate EditCommands through the Editor | modify the store directly (fast) | bypassing INV-5 loses undo and the journal; the cost is that batch fixes construct many command objects (mitigated with an object pool) |
| rule sources | built-in Rust + runtime-loadable mapcss | bake all rules in at compile time | rules are **assets**, with GPL/wiki licensing isolation, and must be replaceable at runtime ([17](./17-licensing-and-governance.md) §2.3) |

## 6 Risks

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| R1 | missing incremental dependency declaration → silently missed reports | problems discovered only after upload (worst case) | mandatory `deps()` + mutation tests asserting "deliberately changing a tag/coordinate must trigger recomputation" + nightly full comparison |
| R2 | mapcss compatibility-layer semantic deviations (pseudo-class ordinals, `area` selector) | different conclusions for the same rule | use the asserts of the 12 core files as a corpus; record deviations in the known-deviations table rather than "fixing bugs" |
| R3 | suppression hiding errors | data damage | suppression filters only at the presentation layer; the report retains everything; the gate always looks at unsuppressed issues |
| R4 | inconsistent reports during rule hot reload | UI flicker, mistaken fix clicks | reload produces a new `generation` and old reports become read-only; a failed reload keeps the previous version |
| R5 | territories depend on `boundaries.osm` (mixed GPL/ODbL) | licensing and size | go through the runtime asset directory and allow absence; absence is recorded as skip rather than error ([17](./17-licensing-and-governance.md) §2.3) |
| R6 | a full pass exceeds the frame budget at 10M | stutter | full passes run in the background only when idle/before upload/on explicit request, with cancellable progress |

## 7 Acceptance Criteria

1. **Conclusion parity**: all `assertMatch`/`assertNoMatch` in the 12 core `.validator.mapcss` files pass, and deviations are registered in the known-deviations table with a reason for each.
2. **Incremental correctness**: on a 1M-object dataset, 10k random edits, where the issue set of step-by-step incremental validation is exactly equal to "full validation every time" (compared by normalized `(code, object set)`), with 0 differences.
3. **Determinism**: two validation reports on the same `Snapshot.version` are byte-identical (including ordering), and identical at concurrency 1 and 8.
4. **Undoable**: for every fixable rule class, the dataset after `fix→undo` equals the pre-fix state field by field, and the undo stack depth increases by 1, not +N.
5. **Idempotent**: rerunning the same rule after a fix no longer produces the same `code` (property test with 10^4 cases).
6. **Suppression**: still effective after a restart; modifying relevant tags of a suppressed object invalidates the suppression automatically and raises the warning again.
7. **Gate**: an upload containing errors issues no HTTP write request at all (mock API asserts zero requests); warnings alone require explicit confirmation.
8. **Performance**: p95 from a single-node drag to report refresh [pending measurement, see docs/14-performance-and-verification.md]; idle CPU at 10M objects < 0.5% (nothing scheduled without a dirty set).

## 8 References

- JOSM validator MapCSS rule syntax: https://josm.openstreetmap.de/wiki/Help/Validator/MapCSSTagChecker
- Community rule-set index (`Rules/*`, the preferred distribution channel): https://josm.openstreetmap.de/wiki/Rules
- MapCSS implementation and the full operator set (`∈ ⊆ ⧉`, pseudo-classes, `::layer`): https://josm.openstreetmap.de/wiki/Help/Styles/MapCSSImplementation
- JOSM validator preferences (suppression details): https://josm.openstreetmap.de/wiki/Help/Preferences/Validator
  > To verify: the preference key and persistence semantics of the suppression/ignore list; primary sources have not been re-checked.
- OSM API v0.6 capability limits (waynodes/relationmembers/changeset): https://wiki.openstreetmap.org/wiki/API_v0.6
- Conflict dialog and upload hooks (409 semantics): https://josm.openstreetmap.de/wiki/Help/Dialog/Conflict
- `rstar` 0.14 (R\* tree neighborhood queries) and `rayon` (`par_iter`/`fold`/`reduce`): https://docs.rs/rstar/0.14.0/rstar/ · https://docs.rs/rayon/latest/rayon/
- Primary factual basis (12 core rule files, 40 test classes, hook chain): [`.research/josm-inventory.md`](../.research/josm-inventory.md)
