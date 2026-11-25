# ADR-0008 · Command + Transaction + Undo

## Status

- Accepted: 2026-11-15.
- Reversibility: **Low**. The `EditCommand` shape is frozen; changes would affect all tools and the plugin ABI.
- Scope: all write paths of `neo-editor` / `neo-tools` / `neo-plugin`; `ChangeSet` and OSC generation.
- Related docs: [06](../06-editing-core-and-undo.md), [ADR-0004](./0004-independent-data-render-editor-models.md), [ADR-0006](./0006-single-writer-editor.md).

## Background

Editing must simultaneously satisfy five requirements: undoable (across tools and plugins), crash-recoverable, able to generate OSC, testable headless, and safely producible by WASM plugins.

If tools were allowed to modify `DataSet` directly, each of these five would need its own invented mechanism (after-the-fact diff, full snapshot, UI recording…), and they would be inconsistent with each other. JOSM's approach is `Command` + `UndoRedoHandler`, but commands directly hold and modify `DataSet`, and undo relies on each command's own `undo()`—once a plugin command writes incorrectly, history is corrupted.

## Decision

1. **Commands describe intent only**: `EditCommand::apply(&mut self, tx: &mut EditTx) -> Result<CmdOutcome, EditError>` only records `EditOp`s into the shadow layer; a command cannot bypass the transaction inside `apply`.
2. **Inverse operations are captured at apply time**: `EditTx` records the pre-image for each op (storing the full object on delete); `undo` does not re-derive it.
   - `EditCommand::undo` is reserved for non-structural side effects (selection, changeset bookkeeping, plugin-internal state);
   - The default implementation is injected by the `AutoUndo` wrapper with the captured inverse operation.
3. **The commit pipeline is a fixed 12 steps**: validate → journal begin → apply → inverse → journal commit+sync → dirty → geometry invalidation → reference sets → spatial index → version+1 → publish → notify.
   - The order is locked by integration tests; the journal always precedes publish.
4. **undo is a new transaction**: version unconditionally +1; the version number is not rolled back; subscribers do not need to distinguish forward from backward.
5. **ChangeSet is generated from "images", not from history**: `ChangeSetAccumulator` accumulates commit products; the OSC write side compares the base image with the current image.
   - History can be truncated while ChangeSet cannot.
6. **Naming isolation**: object-level merge situations are called `MergeCase` (`neo-editor`), and server 409 reports are called `ConflictKind` (`neo-api`); the two are converted by `sync_bridge` in `apps/neo-desktop`.
7. **Coalescing**: only when the same `merge_hint` + a 500 ms window + the same batch of objects; entries after upload are never merged into pre-upload entries.

```
Tool/plugin ──► Box<dyn EditCommand>
                 │ apply(&mut tx)
                 ▼
            EditTx (shadow layer: inserts/updates/deletes)
                 │ validate()  ──failure──► return with zero side effects
                 ▼ commit()
   journal(begin,ops) → DataSet → inverse → journal(commit+sync)
                 │
                 ▼ version+1 → publish(Arc<Snapshot>) → notify
```

## Alternatives

| Option | Pros | Why rejected |
|---|---|---|
| Memento: full copy before each transaction | Minimal implementation, undo absolutely correct | Copying the whole database on every edit at 10M objects is infeasible; memory peaks uncontrollable |
| Commands carry their own `undo()` and modify data directly (JOSM style) | Flexible, easy for plugins | Correctness depends on every command author; no unified inverse capture; crash recovery cannot replay |
| Immutable persistent data structures (`im`/`rpds` style) | Natural snapshots, lock-free | Rust ecosystem immature; O(log n) allocation per modification; the GPU side still needs SoA |
| Event sourcing without transaction boundaries | The log is the truth | A single event has no atomicity, so UI intermediate states leak; validation and index updates cannot be atomic |
| Log only post-images | Simple recovery (direct overwrite) | Large; cannot express "create then delete" cancellation; undo still needs pre-images |
| Store a full database snapshot per edit (SQLite transactions) | Strong crash recovery | Write amplification at 10M is unacceptable; cannot connect with GPU incremental updates |

## Consequences

**Positive**

- Undo/journal/OSC/conflict share the same commit point, so semantics cannot diverge.
- Commands can be driven in tests without a UI; plugins can only produce commands and cannot corrupt history.
- Transaction atomicity makes "validation failure = zero side effects" assertable.

**Negative**

- **Inverse operations consume memory**: batch operations pin pre-images in History, so a limit must be set and "irreversible large operations" must be allowed.
- **Replay depends on the code version**: after the `EditOp` rkyv schema changes, old logs are unreadable and must be explicitly refused (`schema: u16` gate).
- **The pipeline order is an implicit contract**: reordering steps creates a crash window where "data is newer than the log", guarded only by tests.
- **The `EditCommand: Send` constraint** (plugins produce commands on workers) means commands cannot capture `Rc`/UI handles.

## Verification

1. Property test: random command sequences + random validation-failure injection; assert that on failure `version` and content are unchanged.
2. After undo→redo, `Snapshot` is field-by-field equal except version (including way node order and relation member order).
3. Pipeline order: inject a crash "after journal commit, before publish"; after replay the state equals the committed state (`tests/commit_order.rs`).
4. Coalescing rules: a 60-frame drag = 1 history entry; gap > 500 ms = 2 entries; no merging after upload.
5. OSC equivalence: the OSC generated by the same batch of edits is semantically consistent with the equivalent JOSM operation (element sets and `version` attribute; verified against a mock server).
6. Plugin constraint: commands returned by plugins cannot call `Snapshot::publish` (the host does not export that symbol).

## References

- OsmChange (OSC) specification: https://wiki.openstreetmap.org/wiki/OsmChange
- OSM API v0.6 diff upload and 409: https://wiki.openstreetmap.org/wiki/API_v0.6#Diff_upload
- JOSM undo/command stack behavioral baseline: https://josm.openstreetmap.de/wiki/Help/Action/Undo
- rkyv 0.8 archive and schema-evolution caveats: https://docs.rs/rkyv/0.8.18/rkyv/
