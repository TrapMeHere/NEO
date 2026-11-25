# 06 · Editing Core & Undo

All data modifications in NEO go through `Command` + `Transaction`: a command describes only intent, `EditTx` collects operations in a shadow layer, and on commit a fixed pipeline validates, applies, captures inverse operations, writes the journal, and publishes the snapshot. This document defines the semantics of `EditCommand`/`EditTx`, the commit pipeline's order and failure semantics, the interaction between Undo/Redo and upload state, ChangeSet/OSC generation, and the Journal format and crash recovery; thread ownership is in [Concurrency Model](./07-concurrency-and-tasks.md), and the editor's place in the whole is in [System Architecture](./01-system-architecture.md) §2.3.

## 1 Goals and Scope

**In scope**: editing primitives (create/update/delete, node movement, way node add/remove, relation members), transaction atomicity, the Undo/Redo stack, the in-memory journal and replay, and the conflict data model.

**Out of scope**: the interaction feel of specific tools (`neo-tools`), the HTTP semantics of the OSM API ([09](./09-osm-api-and-sync.md)), the content of validation rules ([12](./12-validation.md)), and the SQLite schema ([10](./10-persistence-and-recovery.md)).

## 2 Architecture Design

### 2.1 Why Not Mutate the Store Directly

| What would be lost if direct store mutation were allowed | Consequence | How the Command model restores it |
|---|---|---|
| Inverse operations | No undo (reconstructing the prior image after the fact needs a full diff) | Capture inverse operations per operation during apply |
| Atomicity | Half-finished state (node moved but way not updated) | Shadow-layer buffering + all-or-nothing |
| Single-point validation | Validation scattered across UI/tools, untestable headless | `commit()` is the only validation entry point |
| Journaling | Crash recovery can only rely on full autosave | Write the op log before committing |
| OSC generation | Upload diffs would need a separate snapshot comparison | `ChangeSet` is accumulated from commit products |
| Plugin safety boundary | WASM plugins could tamper with arbitrary state | Plugins can only return `EditCommand` |

The temptation of direct store mutation is "performance": an extra buffer layer looks wasteful. The actual buffering cost is one `Vec<EditOp>` push (tens of bytes), negligible relative to geometry index updates; **concrete overhead is [pending measurement, see docs/14-performance-and-verification.md]**.

### 2.2 Relationship Between Commands, Transactions, and the Dataset


**Key point: `apply` does not modify `DataSet`.** `EditTx` maintains three overlay layers, `inserts / updates / deletes`, and `tx.node(id)` checks the overlay first, then the base, so "create a node, then a way, then edit the node's tags" is self-consistent within the same transaction. The actual writes happen in `commit()`, which is when `&mut DataSet` is needed and when failure is possible (the only write paths that can fail are handle generation and index conflicts; prior validation has already ruled out the vast majority).

### 2.3 Commit Pipeline (12 Steps)

| # | Step | Failure semantics | Why at this position |
|---|---|---|---|
| 1 | `validate_structure`: references complete, way has at least 2 points, relation member types legal, optional self-intersection check | Returns `EditError::InvalidGeometry`, **DataSet unchanged** | Failing earliest is cheapest |
| 2 | `journal.append_begin(txn, label)` | `JournalError` → abandon the whole transaction | Write-ahead logging must precede any state change |
| 3 | `apply_ops`: write the write-set into `DataSet`'s SoA columns | Should not fail (pre-validated); `assert!` in debug | — |
| 4 | `capture_inverse`: record the prior/post image for every op | Cannot fail (abort on out-of-memory) | Must be captured during apply; it cannot be reconstructed later |
| 5 | `journal.append_commit(txn)` + `sync` (per policy) | Failure → roll back to pre-txn using the inverse ops from step 4 | The crash-consistency point |
| 6 | `dirty.mark(touched)` | — | For incremental rendering and autosave |
| 7 | `geo.invalidate(touched, version+1)` | — | Geometry cache keys include the version range |
| 8 | `refs.update(touched)` (node→ways, way→relations) | — | Indexes and data update together |
| 9 | `spatial.update(touched)` (incremental rstar insert/remove) | — | Picking and the visible set depend on it |
| 10 | `version += 1` (unconditionally, including undo) | — | INV-7 |
| 11 | `publish(snapshot)`: `Arc::new` + `ArcSwap::store` | — | Subscribers see the new version from here on |
| 12 | `notify`: selection pruning, UI invalidation, autosave trigger, plugin events, `puffin` points | Subscriber exceptions must not roll back a committed transaction | Observers are read-only |

The ordering of steps 2/5 explains the precise meaning of "write-ahead logging": the journal records **operations** (`EditOp`), not post-images. Because `EditOp` is deterministic given the prior image, replay can reconstruct state, and the record size is far smaller than a mirror. The cost is that replay depends on the code version (see the schema version gate in §2.6).

### 2.4 Undo / Redo

```
 History.entries: [ e0 | e1 | e2 | e3 | e4 ]      cursor = 5 (tail)
                                     ↑ new transaction → truncate after cursor (clear redo), push e5
 undo: cursor-- → take entries[cursor].inverse → commit as an ordinary transaction (version+1)
 redo: cursor++ → take entries[cursor].forward → commit as an ordinary transaction (version+1)
```

| Design point | Rule | Rationale |
|---|---|---|
| Undo is also a transaction | Goes through all 12 steps of §2.3, version+1 | Subscribers need not distinguish "forward/reverse", INV-7 |
| Source of inverse ops | Captured per op during `apply` (prior image + complete object for deleted objects) | Overwritten tags cannot be reconstructed later |
| Coalescing | Only when `merge_hint` is equal and the gap is < 500 ms: same-kind drags, consecutive edits of the same tag key | Avoids 60 entries from one drag |
| Idempotence | `undo` twice = original state + two version++ increments; `redo` only valid at the cursor | Keeps scripted replay predictable |
| Capacity | Default cap 256 MiB (configurable), estimated by object size; evict oldest entries when exceeded and set `history_truncated` | Prior images of 10M objects cannot be kept unbounded |
| Upload state | Entry state `Unuploaded / Uploaded{changeset} / DirtyAgain`; undo of an uploaded entry sets `DirtyAgain` | The remote side does not roll back, so local re-upload is needed (see table below) |

| Entry state | Trigger | Generated on next upload |
|---|---|---|
| `Unuploaded` | New commit | create/modify/delete elements |
| `Uploaded{cs}` | Upload succeeded and marked | — (already fixed) |
| `DirtyAgain` | Undo/redo of an uploaded entry, or editing an uploaded object | modify/delete in a new changeset |
| `Orphaned` | Object deleted remotely (confirmed after 409) | Does not appear in the OSC; becomes a `RecreateObject` candidate |

### 2.5 ChangeSet and OSC Generation

`ChangeSet` is the **accumulator of commit products**, decoupled from History (History may be truncated; ChangeSet must not be lost).

| Local state | OSC element | Notes |
|---|---|---|
| New object (negative id) | `<create>` | After upload, mapped to a positive id by `id_map: ObjectId → ObjectId` |
| Existing object modified | `<modify>` | Carries `version`; the server judges conflicts from it |
| Existing object deleted | `<delete>` | Carries `version`; server-side `if-unused` validation |
| Created then deleted | Does not appear | Resolved out of the ChangeSet at commit time, avoiding noise |

Generation timing: `ChangeSetAccumulator::diff() -> EditDiff` (`created/modified/deleted`), then handed to `neo_io::osc::OscWriter::write_osc(&diff, opts, w)`; objects are compared base-image vs current-image, **History is not read**. The shape of `EditDiff` and the writer are in [08](./08-io-pbf-xml-osc.md) §3. When a conflicting upload returns 409, the diff returned by the server plus the local base image form the input to a three-way merge (§2.6).

### 2.6 Conflicts: Data Model and UI Handoff

```rust
/// Object-level "merge case". Deliberately not named ConflictKind — that name belongs to neo-api's
/// ConflictReport (see 09 §3); editor does not depend on api, and the two are separate.
pub enum MergeCase {
    VersionMismatch { local: i32, remote: i32 },
    LocalDeletedRemoteModified,
    RemoteDeletedLocalModified,
    TagDiverged { key: String },                       // same key, different values
    NodeMoved { local_delta_m: f64, remote_delta_m: f64 },
    WayNodeListDiverged { local: Vec<NodeId>, remote: Vec<NodeId> },
    RelationMemberDiverged,
    ChangesetClosed,
}

pub struct Conflict {
    pub object: ObjectRef,
    pub case: MergeCase,
    pub base: Option<ObjectImage>,     // local image before upload
    pub local: Option<ObjectImage>,    // current local image (may have been re-edited)
    pub remote: Option<ObjectImage>,   // current server image (409 response or re-download)
}

/// Same names and order as neo-api's ConflictResolution (KeepLocal | TakeRemote | MergeManually),
/// for one-to-one conversion at the app layer; the extra two entries are local actions unique to editor.
pub enum ConflictResolution {
    KeepLocal, TakeRemote,
    MergeManually { tags: BTreeMap<String, String>, geometry: GeometryChoice },
    DeleteObject, RecreateObject,
}
```

| Case | Merge rule | Which category |
|---|---|---|
| Both sides change different tag keys | Automatic merge (union) | None (no manual work needed) |
| Same key, different values | Manual choice required (local/remote/manual entry) | `TagDiverged` |
| Remote deleted, local modified | Manual choice (keep/delete/recreate as new object) | `RemoteDeletedLocalModified` |
| Both sides change a way's node list | Manual choice + visual diff | `WayNodeListDiverged` |

UI handoff shape: `ConflictQueue` is owned by `Editor`; the UI only `peek`s each frame, and after the user chooses it calls `resolve()`, which returns an **ordinary `EditCommand`** (`ResolveConflictCommand`) that goes through the §2.3 pipeline. Thus "resolving a conflict" and "editing" are completely isomorphic: undoable, journalable, and testable (without a UI).

**409 translation happens at the application layer**: `neo-api::ApiError::Conflict(ConflictReport)` contains only id/version/stage and server text, not local object images; the adapter that translates `ConflictReport` + ChangeSet base images into `Conflict` lives in `apps/neo-desktop/src/sync_bridge.rs` (the only place that depends on both api and editor). This way editor does not depend on api, api does not depend on editor, and the conflict model consists only of IDs/versions/object images.

### 2.6.1 Referential Integrity: Cascading Semantics of Delete / Merge / Split

The OSM server refuses to delete objects that are still referenced (HTTP 412 `Node … is still used by ways …`), but it allows modifying the referrers **and then deleting within the same changeset**. This is exactly what JOSM/iD do when deleting vertices. So locally we cannot "error out whenever referenced"; the referrers must be modified along with it:

| Operation | Referrer handling | Implementation |
|---|---|---|
| Delete node | Remove it from each way's node sequence; delete any way left with fewer than 2 points entirely (UI double confirmation) | `neo_editor::plan_delete` |
| Delete node / way / relation | Remove that member from relation member lists | Same as above |
| Merge way B→A | Relation members referencing B are replaced in place with A (role preserved); duplicate members deduplicated | `MergeWay::apply` |
| Split way | The new way is inserted into the relations containing the original way, right after it (same role) | `SplitWay::apply` |
| Batch-paste relations | Member relations are created before their referrers | `neo_editor::order_relations_by_dependency` |

`plan_delete` only **generates** commands, submitted by `execute_batch`: referrer fixes and deletions share one transaction and
**one** undo record. `DeleteObject::precheck` remains strict (reject when referenced) as the final safety net;
the journal records the whole batch, and replay reproduces it in the same order.

Supporting constraints:

- The minimum of 2 nodes per way is guaranteed by the planner (it never produces a 1-point way); the upper bound of 2000 is validated by the server on upload.
- Relation members must not form cycles: `SetRelationMembers::precheck` checks along the member chain (the server returns 412).
- osmChange `<delete>` entries are ordered relation → way → node, and within relations by **descending ancestor depth**
  (parents before children); otherwise child relations would be rejected by the server for still being referenced by their parents.

### 2.7 Journal: Format, Replay, and the Division of Labor with autosave


| Record | When written | Payload |
|---|---|---|
| `TxnBegin` | Step 2 | `txn_id, label, base_version, timestamp, author` |
| `Op` | After step 2 (written one by one during apply) | `EditOp` |
| `TxnCommit` | Step 5 | `txn_id, op_count` |
| `Checkpoint` | After a successful autosave | `snapshot_version, autosave_path` |

| Decision | Choice | Rationale |
|---|---|---|
| Encoding | **rkyv 0.8.18** (`Archive + Serialize`) | Zero-copy reads, small size; bincode has no version validation and evolves fragilely |
| Alignment | Payload is written to an `AlignedVec` before hitting disk; the read side does `AlignedVec::from_slice` then `access` per record | rkyv relative pointers require the overall base address to be aligned |
| Version gate | Header `schema: u16`; mismatch refuses to load | rkyv schema evolution is not automatic |
| Integrity | Per-frame `crc32c` + magic validation | On crash the last half-frame is necessarily torn; validation locates the truncation point |
| fsync policy | `always` / `interval(1s)` / `never`; default `interval` + forced `always` before upload | `always` means one fsync per transaction, too slow on large datasets [pending measurement, see docs/14-performance-and-verification.md] |
| Division of labor with SQLite | journal = operation log; SQLite = object image cache + autosave | One is operation-oriented, the other image-oriented; no duplication |

**Ownership split (avoiding a reverse editor↔persist dependency)**: frame format, `JournalRecord`, `EditOp` rkyv archiving, and the replay algorithm all belong to `neo-editor`; file creation/rotation/indexing/GC/fsync belong to `neo-persist::JournalStore`, which only receives `(FrameKind, &[u8])` and returns a `seq`. The wiring point is `apps/neo-desktop`: adapt `Arc<Mutex<JournalStore>>` into a `JournalSink` and inject it into `Editor`. This way `neo-persist` depends on `store` but not on `editor` ([02](./02-workspace-and-dependency-rules.md) §2.2); details in [10](./10-persistence-and-recovery.md) §2.3.

**Replay algorithm**:


## 3 Key Interfaces

```rust
// crates/neo-editor/src/command.rs
pub trait EditCommand: Send {
    /// Records operations into the tx shadow layer only; must not touch DataSet directly.
    fn apply(&mut self, tx: &mut EditTx) -> Result<CmdOutcome, EditError>;
    /// Inverse operations. The default implementation is injected by the AutoUndo wrapper (replaying the captured inverses);
    /// only commands with non-structural side effects (selection/changeset/plugin state) need a hand-written version.
    fn undo(&mut self, tx: &mut EditTx) -> Result<(), EditError>;
    fn label(&self) -> CmdLabel;
    fn merge_hint(&self) -> MergeHint { MergeHint::None }
    fn touched(&self) -> &[ObjectRef] { &[] }
}

/// Why `apply` takes `&mut self`:
/// 1) commands are replayed on redo, needing a resettable internal cursor;
/// 2) coalescing must modify an existing command's final state;
/// 3) inverse caches (prior images, deleted objects) hang off the command, avoiding a second traversal.
/// Why `undo` also returns `Result`: undo may fail due to an expired handle or a missing dependency,
/// and must be able to report the error to the user while preserving history consistency (on failure the cursor does not advance).

pub struct CmdOutcome {
    pub label: CmdLabel,
    pub touched: Vec<ObjectRef>,
    pub created: Vec<ObjectRef>,
    pub deleted: Vec<ObjectRef>,
    pub notes: Vec<std::borrow::Cow<'static, str>>,
}

// crates/neo-editor/src/tx.rs
pub struct EditTx<'a> { /* base: &'a mut DataSet, ops: Vec<EditOp>, policy: &'a EditPolicy */ }

impl EditTx<'_> {
    pub fn base_version(&self) -> u64;
    // Read-through: check the overlay first, then the base, so writes within the same transaction are immediately visible
    pub fn node(&self, id: NodeId) -> Option<NodeView<'_>>;
    pub fn way(&self, id: WayId) -> Option<WayView<'_>>;
    pub fn way_node_ids(&self, id: WayId) -> Option<Vec<NodeId>>;

    pub fn insert_node(&mut self, node: Node) -> NodeId;                 // allocates a negative id automatically
    pub fn insert_node_with_id(&mut self, id: NodeId, node: Node) -> Result<(), EditError>;
    pub fn set_position(&mut self, id: NodeId, pos: LatLon) -> Result<(), EditError>;
    pub fn set_tags(&mut self, id: ObjectId, diff: TagDiff) -> Result<(), EditError>;
    pub fn delete(&mut self, id: ObjectId) -> Result<(), EditError>;
    pub fn insert_way_node(&mut self, way: WayId, idx: usize, node: NodeId) -> Result<(), EditError>;
    pub fn remove_way_node(&mut self, way: WayId, idx: usize) -> Result<(), EditError>;
    pub fn set_relation_members(&mut self, rel: RelationId, m: Vec<Member>) -> Result<(), EditError>;
    pub fn note(&mut self, msg: impl Into<std::borrow::Cow<'static, str>>);
    pub fn ops(&self) -> &[EditOp];
}

// crates/neo-editor/src/context.rs
pub struct EditorContext {
    pub version: u64,
    pub selection: Selection,
    pub changeset: ChangeSetAccumulator,
    pub geo: GeoCacheHandle,
    pub journal: Box<dyn JournalSink + Send>,
    pub policy: EditPolicy,
}

// crates/neo-editor/src/selection.rs
pub struct Selection {
    nodes: rustc_hash::FxHashSet<NodeId>,
    ways: rustc_hash::FxHashSet<WayId>,
    relations: rustc_hash::FxHashSet<RelationId>,
    primary: Option<ObjectRef>,
}
impl Selection {
    pub fn toggle(&mut self, id: ObjectRef);
    pub fn set_primary(&mut self, id: Option<ObjectRef>);
    pub fn snapshot(&self) -> SelectionSnapshot;          // for read-only use by background tasks
    pub fn prune(&mut self, snap: &Snapshot) -> usize;    // returns the number removed
    pub fn contains(&self, id: ObjectRef) -> bool;
}

// crates/neo-editor/src/history.rs
pub struct History { entries: Vec<HistoryEntry>, cursor: usize, bytes: usize, cap_bytes: usize }
impl History {
    pub fn push(&mut self, e: HistoryEntry) -> Option<HistoryEntry>;   // returns the evicted entry
    pub fn can_undo(&self) -> bool;
    pub fn can_redo(&self) -> bool;
    pub fn undo(&mut self, tx: &mut EditTx) -> Result<CmdLabel, EditError>;
    pub fn redo(&mut self, tx: &mut EditTx) -> Result<CmdLabel, EditError>;
    pub fn mark_uploaded(&mut self, up_to: TxnId, changeset: i64);
    pub fn set_cap(&mut self, bytes: usize);
}

// crates/neo-editor/src/journal.rs —— editor owns "what to record", persist owns "where to write it"
pub fn encode_record(rec: &JournalRecord, out: &mut rkyv::util::AlignedVec) -> Result<FrameHeader, JournalError>;
pub fn decode_record(bytes: &[u8]) -> Result<JournalRecord, JournalError>;

/// Byte-level persistence interface: implemented by neo-persist::JournalStore (with feature = "persist"),
/// or by editor's built-in MemoryJournal / FileJournal when persist is absent.
pub trait JournalSink: Send {
    fn append_frame(&mut self, kind: FrameKind, payload: &[u8]) -> Result<u64, JournalError>; // -> seq
    fn sync(&mut self, mode: SyncMode) -> Result<(), JournalError>;
    fn truncate_tail(&mut self, good_seq: u64) -> Result<(), JournalError>;
}

pub trait JournalSource {
    fn scan(&mut self, from_seq: u64, visit: &mut dyn FnMut(FrameKind, &[u8]) -> std::ops::ControlFlow<()>)
        -> Result<ScanReport, JournalError>;
}
pub fn replay(src: &mut dyn JournalSource, base: Option<&Snapshot>) -> Result<ReplayOutcome, JournalError>;

// crates/neo-editor/src/conflict.rs
pub trait ConflictQueue: Send {
    fn push(&mut self, c: Conflict);
    fn peek(&self) -> Option<&Conflict>;
    fn resolve(&mut self, r: ConflictResolution) -> Result<Box<dyn EditCommand>, EditError>;
    fn len(&self) -> usize;
}
```

## 4 Data Flow and Sequencing

### 4.1 Full Sequence of One Commit

```
UI ──execute──► Editor ──EditTx──► DataSet ──► Journal ──► Snapshot
                 │ apply(ops)        │ SoA writes  │ persist    │ publish
                 │ validate          │             │            │
                 └─ failure ⇒ zero side effects (DataSet unchanged, returns Err)
                  success ⇒ inverse → version+1 → notify(UI / rendering / autosave)
```

## 5 Tradeoffs and Alternatives

| Choice | Alternative | Why this one | Cost |
|---|---|---|---|
| Collect in a shadow layer then apply all at once | Write DataSet immediately + roll back on failure | Validation has zero side effects; tool code stays simple | The overlay read path adds one lookup; aggregate queries must consult both layers |
| Operation log (op log) | Image log (post-image) | Smaller, and can express "create then delete" resolution | Replay depends on the code schema; cross-version conversion needs a converter |
| Capture inverses during apply | Recompute during undo | The only correct approach (prior images are already overwritten) | Memory grows with transaction size; a capacity cap is required |
| `EditTx::insert_node` allocates negative ids automatically | Caller supplies a temporary id | Avoids id conflicts; centralizes id-space management | Negative id allocation order must be journaled to be reproducible |

## 6 Risks

| # | Risk | Trigger signal | Mitigation |
|---|---|---|---|
| C1 | Inverse-operation memory blowup (undo after a batch operation on 1 million objects) | History bytes approach the cap, evictions become frequent | Per-transaction ops cap (`EditPolicy`) + confirmation required for large operations |
| C2 | Replay incompatible with the current code schema | After an upgrade, startup warns "log version too new" | Schema version gate + run an "old log replay" regression on every release |
| C3 | Commit pipeline order changed (journal after publish) | Data newer than the log after a crash | Lock the pipeline order in an integration test (`tests/commit_order.rs`) |
| C4 | Three-way merge silently loses tags | Users report "tags disappeared" | Automatic merging only allows "different keys"; same keys are always manual |

## 7 Acceptance Criteria

1. For any transaction where `apply` succeeds but `validate` fails, `DataSet`'s `version` and content change by zero (property test: random commands + randomly injected validation failures).
2. `undo` followed by `redo` yields a `Snapshot` identical field by field except for version, covering create/delete/modify/way node add-remove/relation members.
3. The commit pipeline order is locked by tests: simulating a crash "after commit, before publish", replay yields a state consistent with the committed state.
4. Kill-process tests (SIGKILL / `TerminateProcess`) with 100 random injections never produce "half a transaction" [pass rate pending measurement, see docs/14-performance-and-verification.md].
5. Dragging the same object continuously for 60 frames produces only 1 history entry; two drags more than 500 ms apart produce 2.
6. 409 conflict integration tests: construct all 6 classes of `MergeCase`; the OSC produced after `resolve` is accepted by the server (mock).
7. During a batch operation on 100k objects, History memory stays under the configured cap, and after eviction the most recent 100 entries can still be undone.

## 8 References

- OsmChange (OSC) specification: https://wiki.openstreetmap.org/wiki/OsmChange
- OSM API v0.6 upload and 409 conflict semantics: https://wiki.openstreetmap.org/wiki/API_v0.6
- JOSM conflict resolution flow (behavioral alignment baseline): https://josm.openstreetmap.de/wiki/Help/ConflictResolution
- JOSM's public description of undo/command stacks: https://josm.openstreetmap.de/wiki/Help/Action/Undo
- rkyv 0.8 archiving and alignment requirements: https://docs.rs/rkyv/0.8.18/rkyv/
- arc-swap and snapshot switching: https://docs.rs/arc-swap/1.9.5/arc_swap/
- Project-internal evidence records (command/conflict/JOSM semantics inventory): `.research/josm-inventory.md`, `.research/pages/conflict.txt`
