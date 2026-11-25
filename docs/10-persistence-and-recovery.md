# 10 · Persistence and Crash Recovery (Persistence & Recovery)

This document defines `neo-persist`: SQLite (WAL) holds workspace metadata, indexes, and caches; rkyv + memmap2 hold large-object snapshots; and the "operation-log files + SQLite index" form the crash-recovery chain. The most important boundary: **OSM object data does not enter SQLite's authoritative path** — the authoritative data always lives in `neo-store`'s in-memory SoA, and there is no such thing in SQLite as "query the map every frame".

## 1 Goals and Scope

| # | Goal | Judgement |
|---|---|---|
| P1 | Edits survive a crash and recovery reaches a consistent state | object count / versions / referential integrity checks pass |
| P2 | Reopening a project is usable within seconds | only replay transactions with `version > autosave.version` |
| P3 | Caches are physically isolated from authoritative data | deleting caches does not affect data correctness |
| P4 | The upload queue survives restarts | `changeset_queue` is persisted ([09](./09-osm-api-and-sync.md) §2.7) |
| P5 | The database does not bloat | bounded WAL, sealed journals are GC-able |

**Out of scope**: multiple processes concurrently writing the same project (single-process exclusivity + file lock), cloud sync, database encryption (not in v1; risk noted only).
**Dependencies**: `neo-persist → neo-core, neo-osm, neo-store`, and **not** `neo-editor` — journal frames are passed in as opaque bytes (`&[u8]`), the frame format is defined by [06 §2.7](./06-editing-core-and-undo.md), and this crate is responsible only for "append + find by seq + fsync", without understanding `EditOp` semantics.

## 2 Design

### 2.1 Division of Responsibility: Authoritative / Derived / Cached

| Storage | Content | Authority | Consequence of loss |
|---|---|---|---|
| `neo-store` in-memory SoA | Node/Way/Relation, indexes, rstar | **sole authority** | lost when the process crashes; rebuilt from journal + autosave |
| journal file (`.neojournal`) | transaction frames (begin/op/commit) | authoritative (operation level) | loses recent edits |
| autosave snapshot (`.neosnap`, rkyv) | full image + version | derived (rebuildable) | replay gets longer, no data loss |
| SQLite `*.neodb` | project/layers/indexes/queue/settings | authoritative (metadata) | metadata lost, OSM data still rebuildable from source files |
| cache tables + disk cache | imagery offsets, presets, MapCSS, tiles | **cache** | re-download/recompile |

**Anti-pattern (forbidden)**: storing OSM objects in SQLite and then `SELECT`ing them frame by frame. Rendering needs a contiguous in-memory view of hundreds of thousands to millions of objects; row-based reads with row-by-row `rusqlite` decoding cannot fit the frame budget; and every edit would be amplified into page writes by the WAL.

### 2.2 Draft Schema

```sql
PRAGMA journal_mode = WAL;          -- crash safety + concurrent read/write
PRAGMA synchronous  = NORMAL;       -- consistency is already guaranteed under WAL; FULL only temporarily before uploads/dangerous operations
PRAGMA busy_timeout = 5000;         -- so the background worker and the UI thread do not hit SQLITE_BUSY against each other
PRAGMA foreign_keys = ON;
PRAGMA temp_store   = MEMORY;
PRAGMA wal_autocheckpoint = 512;    -- pages; about 2MiB (WAL page 4KiB), bounded growth
PRAGMA mmap_size = 67108864;        -- 64MiB, benefits the read-only cache tables

CREATE TABLE schema_version (version INTEGER NOT NULL PRIMARY KEY, applied_at INTEGER NOT NULL);
CREATE TABLE project (id INTEGER PRIMARY KEY, name TEXT NOT NULL, root_path TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL, last_opened_at INTEGER, settings_json TEXT NOT NULL DEFAULT '{}');
CREATE TABLE layer (id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE, name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('osm','gpx','imagery','notes')),
  visible INTEGER NOT NULL DEFAULT 1, opacity REAL NOT NULL DEFAULT 1.0,
  z_order INTEGER NOT NULL DEFAULT 0, source_path TEXT,          -- source file; empty for a new layer
  snapshot_ver INTEGER NOT NULL DEFAULT 0);                      -- aligned with the in-memory Snapshot.version

CREATE TABLE journal_file (id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE, path TEXT NOT NULL,
  first_seq INTEGER NOT NULL, last_seq INTEGER NOT NULL, byte_len INTEGER NOT NULL,
  sealed INTEGER NOT NULL DEFAULT 0,                             -- 1 = already covered by a checkpoint, safe to delete
  created_at INTEGER NOT NULL);
-- only a "frame locator index"; the frames themselves live in .neojournal; rkyv frames are not stuffed into BLOBs
CREATE TABLE journal_index (seq INTEGER PRIMARY KEY,             -- globally monotonically increasing across files
  project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  journal_file_id INTEGER NOT NULL REFERENCES journal_file(id) ON DELETE CASCADE,
  byte_offset INTEGER NOT NULL, byte_len INTEGER NOT NULL, txn_id BLOB NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('begin','op','commit','checkpoint')),
  committed_at INTEGER NOT NULL);
CREATE INDEX idx_journal_txn ON journal_index(txn_id, seq);

CREATE TABLE autosave (id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  version INTEGER NOT NULL, path TEXT NOT NULL, byte_len INTEGER NOT NULL,
  crc32c INTEGER NOT NULL, txn_seq INTEGER NOT NULL,             -- journal seq covered up to
  created_at INTEGER NOT NULL, UNIQUE (project_id, version));
CREATE TABLE download_cache (bbox_key TEXT PRIMARY KEY,          -- 'bbox:…' or 'tile:z/x/y'
  project_id INTEGER NOT NULL, fetched_at INTEGER NOT NULL, expires_at INTEGER, etag TEXT,
  object_count INTEGER NOT NULL, payload_path TEXT NOT NULL);    -- compressed chunks, mmap on read
CREATE TABLE changeset_queue (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL,
  client_uuid TEXT NOT NULL UNIQUE,                              -- idempotency key
  seq INTEGER NOT NULL, payload BLOB NOT NULL,                   -- serialized OSC fragment
  state TEXT NOT NULL CHECK (state IN ('queued','uploading','uploaded','conflicting','aborted')),
  changeset_id INTEGER, uploaded_until INTEGER,                  -- chunked-resume cursor
  created_at INTEGER NOT NULL);
CREATE INDEX idx_queue_pending ON changeset_queue(project_id, state, seq);

CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE kv_blob (key TEXT PRIMARY KEY, value BLOB NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE imagery_offset (id INTEGER PRIMARY KEY, imagery_id TEXT NOT NULL,
  region_key TEXT NOT NULL,                                      -- geohash-6 or 'global'
  dx REAL NOT NULL, dy REAL NOT NULL, source TEXT NOT NULL, updated_at INTEGER NOT NULL,
  UNIQUE (imagery_id, region_key));
CREATE TABLE preset_cache (id TEXT PRIMARY KEY, etag TEXT, fetched_at INTEGER, payload_path TEXT);
CREATE TABLE mapcss_cache (hash TEXT PRIMARY KEY, source_path TEXT, compiled_path TEXT, updated_at INTEGER);
```

### 2.3 Journal Location on Disk and fsync Policy

The frame format (magic/schema/flags/len/crc32c/seq + rkyv payload) is defined by [06 §2.7](./06-editing-core-and-undo.md); this document only defines where it lands:

| Location | Pros | Cons | Verdict |
|---|---|---|---|
| SQLite BLOB table | single file, transactional for free | one page write per frame → WAL bloat; rkyv alignment requires copying; `VACUUM` is expensive | ❌ |
| separate append-only file | sequential writes, zero-copy appends, mmap-scannable | requires a self-built index and GC | ✅ |
| pure in-memory + periodic snapshots | simplest | a crash loses an entire snapshot interval | ❌ |

**File layout**: `<project_root>/journal/000001.neojournal`, with a 64MiB per-file cap; reaching the cap triggers rotation (`journal_file` records `first_seq/last_seq`). `journal_index` is written in batches (every 512 frames or 200ms), so it may lag behind the file tail — during recovery **the file scan is authoritative** and the index only accelerates it; on inconsistency, the index is rebuilt from the file.

**Three fsync modes** (aligned with 06's `journal_sync`): `Always` (`fdatasync` after every `TxnCommit`, forced before uploads/dangerous operations), `Interval(d)` (default 1s, background group commit), `Never` (read-only import/one-off conversion). Group commit implementation: the writer thread publishes "the largest appended-but-unsynced seq" into an `AtomicU64`; the sync thread takes it, does `fdatasync`, then writes back `synced_seq`; `Always` requests go through the same `Mutex<File>` to avoid interleaving with the background sync.

### 2.4 Autosave Cadence and Background Worker

- Trigger: debounced after `dirty` (commit-pipeline step 6), default **120s**; or "transaction count > N" / "journal bytes since the last autosave > M" (both configurable; the defaults are not a performance promise).
- Action: a **background thread** rkyv-archives the current snapshot → temporary file → `fsync` → atomic `rename` → write the `autosave` row → append a `Checkpoint` frame.
- Retention: the most recent **3** generations per project (`autosave.keep`); older ones are GC'd together with the journal they cover (`sealed=1` and `last_seq < oldest autosave.txn_seq`).
- Crash detection: `<project_root>/.lock` records `pid` + process start timestamp; if the pid does not exist at startup (or the start time does not match), the previous run did not exit normally and recovery proceeds directly. Same lineage as JOSM's `AutosaveTask` (pid file + number of retained copies).
- **Does not block editing**: take an `Arc<Snapshot>` (read-only, immutable) and hand it to background encoding while the editing thread keeps producing new snapshots — a direct dividend of "read-only snapshots + incremental updates".

### 2.5 Crash Recovery Sequence

```text
startup
 ├─ open project/*.neodb ─▶ schema_version: below current ⇒ migrate version by version; above ⇒ refuse and notify
 ├─ read .lock ─▶ stale pid present and process does not exist ⇒ unclean = true
 ├─ pick autosave: ORDER BY version DESC LIMIT 3
 │    ├─ all crc32c checks fail ⇒ fall back to re-import from source_path, enter read-only recovery
 │    └─ take the newest usable: rkyv access → Snapshot(version = V_a)
 ├─ journal scan (from after autosave.txn_seq)
 │    ├─ magic/len mismatch ⇒ torn tail, truncate to the last complete frame, record TruncatedTail
 │    ├─ crc32c mismatch ⇒ same as above
 │    └─ schema mismatch ⇒ refuse replay, keep only the autosave, enter read-only recovery mode
 ├─ transaction-level replay: replay only transactions that have both begin and commit (ascending seq), version += 1 per commit
 ├─ Editor::install_snapshot(RecoveredState)
 └─ consistency check (§2.6) ──▶ RecoveryReport
```

### 2.6 Consistency Checks and Repair

| Check | Method | Automatic repair |
|---|---|---|
| torn journal tail | frame scan + crc32c | ✅ truncate to the last complete frame (keep a `.corrupt` copy of the original) |
| index/file inconsistency | compare the last indexed `offset+len` with the file length | ✅ rebuild the index tail from the file |
| corrupt autosave | crc32c + total length | ✅ fall back to the previous generation |
| referential integrity | Way→Node, Relation→member resolution (reusing `RefIntegrity`) | ⚠️ report only, never auto-delete (could be partial data) |
| orphan files | entries under `autosave/`, `journal/` not referenced by any table | ✅ move to `trash/`, not deleted directly |
| corrupt SQLite pages | `PRAGMA quick_check` at startup, menu `integrity_check` | ⚠️ report + suggest export; `REINDEX` only when the index is corrupt |
| stale queue state | `state='uploading'` but `uploaded_until` not updated | ✅ revert to `queued` (prefer re-uploading over silently losing) |

`RecoveryReport` must be fully presented in the UI: `unclean_shutdown`, `autosave_version`, `replayed_txns`, `truncated_tail_bytes`, `read_only_fallback`, `repairs`.

### 2.7 Performance Budget and WAL Bloat Control

| Means | Practice | Purpose |
|---|---|---|
| batched transactions | index/queue writes coalesced into one transaction per 512 rows or 200ms | fewer fsyncs and WAL frames |
| prepared statements | `prepare_cached(sql)` | avoid parsing SQL every time |
| checkpoint | `wal_autocheckpoint=512` + `wal_checkpoint(TRUNCATE)` on shutdown | bounded WAL |
| short read transactions | cache reads never hold a `Transaction` across frames | long read transactions block checkpoints ⇒ unbounded WAL (the most common incident) |
| restricted `synchronous=FULL` | enabled temporarily only before uploads/dangerous operations | the normal path does not pay two fsyncs per commit |
| archives not stored in the DB | autosave writes separate files rather than BLOBs | avoids large objects blowing up the DB and WAL |
| no per-frame queries | UI/rendering read only the in-memory snapshot | see the anti-pattern in §2.1 |

`.neodb` size is only **weakly correlated** with project data volume (it grows only with history entries and caches) and shrinks after GC; concrete numbers [pending measurement, see docs/14-performance-and-verification.md].

## 3 Key Interfaces

```rust
// neo-persist/src/lib.rs
pub struct DbOptions { pub busy_timeout: Duration, pub wal_autocheckpoint_pages: u32,
                       pub read_only: bool, pub mmap_size: u64 }
pub struct PersistDb { /* rusqlite::Connection + prepared stmt cache */ }
impl PersistDb {
    pub fn open(path: &Path, opts: DbOptions) -> Result<Self, PersistError>;
    pub fn migrate(&mut self) -> Result<MigrationReport, PersistError>;
    pub fn quick_check(&self) -> Result<IntegrityReport, PersistError>;
    pub fn project_upsert(&mut self, row: &ProjectRow) -> Result<i64, PersistError>;
    pub fn imagery_offset(&self, imagery: &str, region: &str) -> Result<Option<Offset>, PersistError>;
    pub fn set_imagery_offset(&mut self, o: &Offset) -> Result<(), PersistError>;
    pub fn queue_push(&mut self, item: &QueueItem) -> Result<i64, PersistError>;
    pub fn queue_claim_next(&mut self, project: i64) -> Result<Option<QueueItem>, PersistError>;
    pub fn queue_mark(&mut self, id: i64, st: QueueState, until: Option<i64>) -> Result<(), PersistError>;
}

/// Opaque frames: this crate does not understand EditOp, it only appends bytes and finds them again.
pub struct FrameMeta { pub seq_hint: u64, pub txn_id: [u8; 16], pub kind: FrameKind }
pub enum FrameKind { Begin, Op, Commit, Checkpoint }
pub enum SyncPolicy { Always, Interval(Duration), Never }

pub struct JournalStore { /* File + index writer + sync thread handle */ }
impl JournalStore {
    pub fn open(project: i64, root: &Path, index: Arc<Mutex<PersistDb>>) -> Result<Self, PersistError>;
    pub fn append(&mut self, frame: &[u8], meta: FrameMeta) -> Result<u64, PersistError>; // → seq
    pub fn sync(&mut self) -> std::io::Result<()>;                                        // fdatasync
    pub fn set_sync_policy(&mut self, p: SyncPolicy);
    /// Scan from from_seq; the visitor stops when it returns Break.
    pub fn scan(&mut self, from_seq: u64,
                visitor: &mut dyn FnMut(FrameRef<'_>) -> std::ops::ControlFlow<()>)
                -> Result<ScanReport, PersistError>;
    pub fn truncate_tail(&mut self, good_len: u64) -> Result<(), PersistError>;
    pub fn rotate_if_needed(&mut self, max_bytes: u64) -> Result<bool, PersistError>;
    pub fn gc(&mut self, keep_from_seq: u64) -> Result<GcReport, PersistError>;
}

pub struct AutosaveWorker { /* background thread + Arc<Snapshot> snapshot taking */ }
impl AutosaveWorker {
    pub fn spawn(cfg: AutosaveConfig, journal: Arc<Mutex<JournalStore>>) -> Self;
    pub fn mark_dirty(&self, snapshot_version: u64);
    pub fn force(&self) -> Result<PathBuf, PersistError>;   // called before upload, waits synchronously
    pub fn shutdown(self) -> Result<(), PersistError>;
}

pub struct RecoveryReport { pub unclean_shutdown: bool, pub autosave_version: Option<u64>,
    pub replayed_txns: u64, pub truncated_tail_bytes: u64, pub read_only_fallback: bool,
    pub repairs: Vec<RepairAction>, pub integrity: IntegrityReport }
impl RecoveryReport { pub fn is_usable(&self) -> bool }   // false ⇒ export/read-only only

pub struct SnapshotArchive;   // rkyv + memmap2
impl SnapshotArchive {
    pub fn write_atomic(path: &Path, snap: &Snapshot) -> Result<ArchiveMeta, PersistError>;
    pub fn open(path: &Path) -> Result<MappedSnapshot, PersistError>;  // mmap + access
}
pub struct ArchiveMeta { pub version: u64, pub byte_len: u64, pub crc32c: u32, pub schema: u16 }

#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    #[error("sqlite: {0}")] Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("schema too new: db={db}, supported={supported}")] SchemaTooNew { db: u32, supported: u32 },
    #[error("corrupt journal frame at seq {seq}")] CorruptFrame { seq: u64 },
    #[error("archive crc mismatch: {0}")] ArchiveCorrupt(PathBuf),
    #[error("project locked by pid {0}")] Locked(u32),
}
```

## 4 Data Flow and State Machine

```text
EditTx commit
 ├─(06 step 2) journal.append(Begin) / append(Op)*
 │     └─ JournalStore writes .neojournal sequentially, seq = ++global; index rows first go to a buffer (flushed every 512 rows/200ms)
 ├─(06 step 5) journal.append(Commit) + sync(per policy)
 ├─(06 step 6) dirty.mark() ─▶ AutosaveWorker.mark_dirty(version)
 └─ snapshot published ─▶ immediately visible to rendering/UI (without waiting for disk)

AutosaveWorker (background, 120s debounce by default)
 ├─ take Arc<Snapshot> (read-only, does not block editing)
 ├─ rkyv to_bytes → temporary file → fsync → rename → autosave row
 └─ journal.append(Checkpoint{snapshot_version, path}) → GC(keep_from_seq)

WAL incident model:
long read transaction A ──────────────(alive across frames)──────────────▶ end
   WAL: [frame][frame][frame]…[frame]  ← A has not ended ⇒ checkpoints cannot reclaim ⇒ unbounded WAL ⇒ disk full
correct practice: cache reads use μs-level short transactions; anything crossing frames/rendering reads the in-memory snapshot.
```

## 5 Trade-offs and Alternatives

| Decision | Choice | Alternative | Why rejected |
|---|---|---|---|
| metadata DB | SQLite (rusqlite, WAL, bundled) | sled / redb | needs explicit SQL, a mature toolchain, and `bundled` avoids system dependencies |
| journal location | separate append-only file | SQLite BLOB | per-frame page-write amplification + WAL bloat + rkyv alignment copying |
| large-object encoding | rkyv 0.8.18 | JSON / bincode | zero-copy + small size; bincode has no schema validation |
| large-object access | memmap2 read-only mapping | read fully into a `Vec` | country/province basemaps are hundreds of MB and would compete for memory with editing data |
| crash detection | pid + start-timestamp lock | rely on WAL alone | the WAL only guarantees DB consistency; it cannot tell you "did the last run exit normally" |
| autosave | full snapshot + operation log, two layers | log only / snapshot only | replay over long sessions is uncontrollable; snapshots alone lose a whole interval on crash |
| caches in the same DB as authoritative data | same DB, different tables + different files | split into two DBs | one DB simplifies transactions/backups; a "deletable cache tables" boundary suffices |

## 6 Risks

| Risk | Impact | Mitigation |
|---|---|---|
| long read transactions block checkpoints | unbounded WAL, disk full | ban cross-frame transactions; `wal_autocheckpoint` + `TRUNCATE` on shutdown; check WAL size at startup |
| `synchronous=NORMAL` loses the last transactions on power loss | loses recent edits (no corruption) | `force()` + `synchronous=FULL` before uploads/dangerous operations |
| rkyv schema evolution | old autosaves unreadable | explicit refusal to read via the header `schema: u16` and degrade; no cross-version promise |
| mmap file modified/truncated externally | `SIGBUS` | validate magic/len/crc on open; on Windows, deleting a mapped file fails, so it must be unmapped first |
| GC racing with replay | deleting not-yet-replayed frames | GC runs only before "the oldest autosave coverage point" and under the write lock |
| multiple instances opening the same project | overwriting each other | strong `.lock` exclusivity + `Locked`/`SchemaTooNew` error codes |
| disk full | archive/journal failures | check free space before writing; on failure disable autosave and warn, never silently |

## 7 Acceptance Criteria

- [ ] Inject a crash 100 times before and after commit (`TerminateProcess`/`kill -9`); after recovery, transactions are either fully present or fully absent, with no intermediate state.
- [ ] Manually truncate the tail of `.neojournal` by N ∈ {1, 8, 100} bytes; recovery succeeds and `truncated_tail_bytes > 0`.
- [ ] Corrupt the crc of the newest autosave; fall back to the previous generation, with `repairs` containing the corresponding entry.
- [ ] After 100 random crashes `PRAGMA quick_check` is always `ok`; after deleting all caches the project still opens and edits normally.
- [ ] After 1000 commits the WAL size is bounded (does not grow monotonically with commit count), threshold [pending measurement, see docs/14-performance-and-verification.md].
- [ ] Power is cut immediately after `force()` before upload; after recovery the queue and journal are consistent (`uploading` reverted to `queued`).
- [ ] `cargo test -p neo-persist` covers migrate / replay / truncate / gc / lock; `clippy -D warnings` passes.

## 8 References

- `rusqlite 0.42` (`pragma_update`, `prepare_cached`, `transaction_with_behavior`, `busy_timeout`): <https://docs.rs/rusqlite/0.42.0/rusqlite/>
- SQLite WAL and `wal_autocheckpoint`/`wal_checkpoint`: <https://www.sqlite.org/wal.html>
- Consistency guarantees of `PRAGMA synchronous` under WAL: <https://www.sqlite.org/pragma.html#pragma_synchronous>
- `rkyv 0.8.18` (`to_bytes`/`access`/`AlignedVec`): <https://docs.rs/rkyv/0.8.18/rkyv/>
- `memmap2 0.9.12`: <https://docs.rs/memmap2/0.9.12/memmap2/>
- Journal frame format / replay algorithm / three `journal_sync` modes: [06 §2.7](./06-editing-core-and-undo.md)
- Related: [IO Layer](./08-io-pbf-xml-osc.md) · [API and Sync](./09-osm-api-and-sync.md) · [Data Model and Store](./03-osm-data-model-and-store.md)
- > To verify: the default parameters of JOSM's `AutosaveTask` (`autosave.interval`=5min, `autosave.filesPerLayer`=1, pid file) come from a local source snapshot; no citable permanent URL was obtained.
