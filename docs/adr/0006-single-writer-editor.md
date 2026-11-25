# ADR-0006 · Single-Writer Editor

## Status

- Accepted: 2026-11-15.
- Reversibility: **Medium**. Changes would affect all public signatures of `neo-editor` and how `neo-ui` accesses data.
- Scope: the non-`Sync` constraint on `Editor`, `SnapshotPublisher`, version validation of background tasks, the import install path.
- Related docs: [07](../07-concurrency-and-tasks.md) §2.2, [06](../06-editing-core-and-undo.md) §2.3, [ADR-0008](./0008-command-transaction-undo.md).

## Background

On a data set of 10M objects, any scheme that "has multiple threads modify the same data concurrently" must answer: lock granularity, deadlock order, global undo order, replayable log order, and which version rendering/validation reads.

Fine-grained locking (one lock per object, sharded locks) has uncontrollable overhead at the object-count scale, and once "A reads while B writes" appears it produces hard-to-reproduce inconsistency. An editor, however, naturally follows the interaction model of "the user performs one action at a time": **writes themselves are serial, while reads can be parallel** (geometry building, validation, scene building).

## Decision

1. **The UI thread is the only writer**: `&mut` to `DataSet` is held only by `Editor` on the UI thread; `Editor` is non-`Sync` and `DataSetMut` is non-`Send`, so the compiler prevents cross-thread access.
2. **All writes go through `EditTx`**: transaction commit is the only publication point; commit order is history order, log order, and version order.
3. **The read path is lock-free**: publish `Arc<Snapshot>` into `arc-swap::ArcSwap`; background code holds it across threads with `load_full()`, while a single UI frame uses `load()` (`Guard` cannot cross frames).
4. **Background results carry version validation**: if `TaskOutcome::Ready { base_version, .. }` does not match the current version, it is discarded as `Stale` (or explicitly rebased); never "patch-write" a snapshot.
5. **Two structural exceptions**: PBF/XML import and autosave recovery build a brand-new `DataSet` on a worker, then install it in one shot with `Snapshot::publish`.

```
Writer (UI thread)                       Readers (Rayon / Tokio / rendering)
  Editor ──EditTx──► DataSet
     │                   │
     └── publish ────────┴──► ArcSwap<Snapshot> ──► load()/load_full()
                                  │
                                  └── background results must carry base_version + Handle.gen
```

| Exception path | Writer | Publication | Enters History |
|---|---|---|---|
| User edit | UI thread `EditTx` | Incremental + `version+1` | Yes |
| PBF/XML import | worker (exclusive new DataSet) | `Snapshot::publish` atomic replace | No (new session) |
| autosave recovery | worker (exclusive new DataSet) | `Snapshot::publish` atomic replace | No (history rebuilt separately from the journal) |

## Alternatives

| Option | Pros | Why rejected |
|---|---|---|
| Fine-grained locking (per-object/sharded `RwLock`) | High theoretical parallelism | Lock metadata and cache-line contention for 10M objects are unacceptable; deadlock order is hard to maintain; undo/log lose global order |
| One actor per layer (message-passing serialization) | Clear logic, easy to debug | Cross-layer operations (a node + its ways) need two-phase commit and rollback protocols, more complex than a single writer |
| Optimistic parallel editing + conflict merging (MVCC) | Background writes can be parallel | OSM editing is "user intent", not independent transactions; local merge semantics have nowhere to live, and stacking them with remote 409 conflicts is unexplainable |
| Use a database (SQLite) directly as truth | Transactions and crash recovery come for free | At 10M objects every pick/render query would go through SQL; the in-memory SoA is the only usable form for rendering and geometry |
| Single writer on a dedicated thread (command queue) | UI is not blocked by editing | Every edit costs a round trip + context switch; immediate undo feedback gets worse; a serial channel is still needed |

## Consequences

**Positive**

- Commit order = history order = log order ⇒ undo / journal / OSC are naturally consistent.
- Only one logical write lock, so the deadlock category disappears; rendering and validation are fully parallel.
- Crash recovery only needs to replay the op log, with no "multi-version merging".

**Negative**

- **Long transactions block input**: bulk paste/large polygons must be split (`EditPolicy::max_ops_per_txn`, see [06](../06-editing-core-and-undo.md) §2.8).
- **UI thread triples as one**: input + command execution + GPU submit share the frame budget (see [07](../07-concurrency-and-tasks.md) §2.1); exceeding the budget can only lower LOD.
- **Cannot make semantic modifications in parallel**: "add a tag to 100k ways" must be serialized into transactions, and only geometry/index parallelism can amortize it.
- **Background cannot opportunistically fix data**: every fix must become a new transaction, at the cost of extra versions and history entries.

## Verification

1. Compile time: `Editor` does not implement `Sync`, `DataSetMut` does not implement `Send` (`static_assertions::assert_not_impl_any!`).
2. Monotonic version: after any operation sequence, `Snapshot.version` strictly increases; undo/redo also increase it (INV-7).
3. `Stale` path: construct a slow task + mid-way edit, assert the result is discarded and the data unchanged (`tests/stale_task.rs`).
4. Single publication point: across the repo, `Snapshot::publish` is called only by the `neo-store` export and the two install paths.
5. Each deadlock item D1–D10 has a test or static check (lock order, holding a lock while posting, rendering waiting for editing, etc.), see [07](../07-concurrency-and-tasks.md) §2.5.
6. Stress test: 60 seconds of continuous editing + parallel scene building + parallel validation, with no deadlock, no panic, and no version rollback [acceptance criteria see docs/14-performance-and-verification.md].

## References

- `arc-swap` (`load`/`load_full`/`Guard` semantics): https://docs.rs/arc-swap/1.9.5/arc_swap/
- Rust's `Send`/`Sync` semantics: https://doc.rust-lang.org/nomicon/send-and-sync.html
- `static_assertions` (compile-time assertions): https://docs.rs/static_assertions/latest/static_assertions/
