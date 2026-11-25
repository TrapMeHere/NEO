# 08 · IO Layer: PBF / XML / OSC Read and Write

This document defines the responsibility boundary of `neo-io`: converting external OSM serialization formats (`.osm.pbf` / `.osm` / `.osc`) into batches that `neo-store` can load in bulk, and writing Store diffs back out into these formats. Core conclusions: on the read side we reuse `osmpbf 0.3.8`'s blob-level parallel decoding; because `osmpbf` is read-only, the write side requires our own encoder; every write-to-disk path goes through the same "chunk—batch—publish" pipeline, so memory usage scales linearly with input size instead of exploding.

## 1 Goals and Scope

| # | Goal | How it is judged |
|---|---|---|
| G1 | Read a `.osm.pbf` of 1M–10M+ objects without loading it fully into memory | peak RSS ≤ a fixed multiple of the decompressed total [pending measurement, see docs/14-performance-and-verification.md] |
| G2 | Stream-read `.osm` (OSM XML 0.6) and `.osc` (OsmChange) | single pass, event-level memory |
| G3 | Write `.osm.pbf` / `.osm` / `.osc`, with OSC directly acceptable to the OSM API | server `diffResult` with no 400 |
| G4 | Semantic equivalence after an "export → import" round trip | golden tests compare both bytes and semantics |
| G5 | Import is cancellable and leaves no half-built snapshot | returns `Err(ImportError::Cancelled)`, does not publish a Snapshot |

**Out of scope**: incremental replication diffs of `planet.osm.pbf` (see [09](./09-osm-api-and-sync.md)), Overpass QL, GPX.

**Session files**: `.jos` (XML) and `.joz` (ZIP) are implemented by `neo-persist/session.rs` (layer references + viewport + selection, tolerating unknown plugin layer sections); see that module's doc comments for the format.
**Constraints**: `neo-io` depends on `neo-core`, `neo-osm`, `neo-store` and **does not depend on** `neo-editor`/`neo-ui`/`neo-render`; it only produces read-only `Snapshot` and `StoreBuilder` batches, and never modifies an already-published snapshot in place (single-writer principle, see [03](./03-osm-data-model-and-store.md)).

## 2 Design

### 2.1 PBF Layers and How `osmpbf 0.3.8` Is Used

The physical hierarchy of PBF is `Blob[] → (HeaderBlock | PrimitiveBlock)`, where each Blob is decompressed and decoded independently — this is the only natural split point for parallelism. `osmpbf` exposes three layers, and we use one of each:

| Layer | API | Purpose |
|---|---|---|
| byte layer | `BlobReader::from_path` / `next_header_skip_blob` | read-only blob index (`ByteOffset`) for pre-scanning and progress estimation, without decompression |
| blob layer | `Blob::decode() -> BlobDecode` | parallel decode unit; three states `OsmHeader`/`OsmData`/`Unknown` |
| element layer | `PrimitiveBlock::elements()` / `groups()` / `dense_nodes()` | we **do not use** `ElementReader::for_each` (serial), nor `par_map_reduce` |

Why not `ElementReader`: `par_map_reduce`'s `MP: Fn(Element<'a>) -> T` + `RD: Fn(T,T)->T` only permits "folding elements into a mergeable value", whereas import needs **ordered batch appends** into `StoreBuilder`, and order determines reference resolvability. Hence we build the scheduler ourselves:

```text
BlobReader ──(Blob, in order)──▶ bounded scheduler ──(rayon)──▶ decode + materialize
```

`required_features` must be validated before loading ([PBF wiki](https://wiki.openstreetmap.org/wiki/PBF_Format)): `OsmSchema-V0.6` and `DenseNodes` must be supported; `HistoricalInformation` and `LocationsOnWays` are "recognized but degraded" — the former keeps deleted-state objects with `visible=false` (for later upload deletion), the latter obtains optional node coordinates via `Way::node_locations()`.

### 2.2 DenseNodes, the String Table, and Coordinate Precision

`DenseNodes` is the primary node encoding: three delta arrays `id`/`lat`/`lon` plus a shared flat `keys_vals` array (both keys and values are string-table indices, and `0` terminates that node's tags). `osmpbf`'s `DenseNodeIter` already performs the accumulation (`dids/dlats/dlons` + `cid/clat/clon`), so we only handle precision and offset:

```text
nano_lat = lat_offset + granularity * Δaccumulated lat   (granularity defaults to 100)
lat_deg  = 1e-9 * nano_lat
```

**Precision decision**: internal coordinates are uniformly `i32` decimicrodegrees (10⁻⁷°), matching API 0.6's 7-decimal serialization and `osmpbf`'s `decimicro_lat() = (nano/100) as i32`. The risk lies in a non-default `granularity`:

| Case | Handling |
|---|---|
| `% 100 == 0` (including the default 100) | exact conversion, no warning |
| `< 100` and not evenly divisible | keep `i64` nanodegrees temporarily, round to 10⁻⁷° on load, count into `ImportReport::precision_loss_objects` |
| offset causes `i32` overflow | reject the blob with `Err(ImportError::CoordinateOverflow)` (>180° is treated as corruption) |

Why not "nanodegrees (`i64`) for the whole database": that costs 16 B per node instead of 8 B, adding nearly 100 MB resident for tens of millions of nodes; why not `f64`: it is not fixed-point at 10⁻⁷° and round trips show 1e-9 drift, breaking golden byte stability.

The **string table** is a per-`PrimitiveBlock` `Vec<Vec<u8>>`; UTF-8 is **not guaranteed**, so keys and values always go through the `Result` path of `str_from_stringtable`, and invalid UTF-8 records `ImportError::InvalidUtf8 { index }` instead of panicking. There is no deduplication across blocks; deduplication happens in `neo-core`'s interning layer (the temporary intern table for one import is bounded, dropping entries by LRU when it overflows, so we do not keep a whole-planet tag vocabulary alive for 10M objects).

### 2.3 Element Types and Ordering Constraints

PBF hard constraint (verified): **a single `PrimitiveGroup` must never mix element types**, the default is roughly 8k entities per group, and the file level does not guarantee that "all Nodes precede all Ways". Import therefore does not rely on file order:

1. **Read in a single pass**, writing into three separate slots by type (nodes / ways / relations);
2. Way/Relation members are written as raw `i64` ids, **without immediate handle resolution**;
3. After reading, the "resolution phase" builds the `id → Handle` map, reference sets, and indexes once in `StoreBuilder::finish()`;
4. Missing references are recorded in `RefIntegrity` without erroring (partial data is legitimate, see [09](./09-osm-api-and-sync.md) §2.6).

The cost is that Way geometry is invisible during loading (no draw-while-reading); the benefit is that throughput is independent of file order. `Sort.Type_then_ID` is used only as an **optimization hint**: when present, the resolution phase can binary-search over ranges to skip id segments that do not exist.

### 2.4 Pipeline, Backpressure, and Memory Limits

```text
 ┌───────────────┐   Blob(≤32MiB decompressed)  ┌─────────────────────┐
 │ reader thread │ ────bounded(8)───▶           │ rayon decode pool   │
 │ BlobReader    │                              │ zlib + protobuf     │
 └───────────────┘                              └──────────┬──────────┘
                                                          │ ElementBatch
                                              ┌───────────▼───────────┐
                                              │ main thread: aggregate│
                                              │ StoreBuilder.append(  │
                                              │ batches, in order)    │
                                              └───────────┬───────────┘
                                                          ▼ finish(): id map → ref set → rstar bulk_load
                                              Snapshot::publish (read-only, Arc)
```

Backpressure has two gates, and whichever fills first blocks the upstream: the **slot gate** `channel::bounded(capacity)` (default `2 × rayon thread count`, preventing blob pile-up); and the **byte gate** `ByteBudget` (default = compressed file size × 4, configurable); each decoded blob does `reserve(block_bytes)` first and `release` after consumption, preventing several 16MiB decompressed blocks from being resident at once.

`StoreBuilder` uses **preallocation + 1.5x geometric growth** (magnitude estimated from `HeaderBlock::bbox()` and file size, used only for `with_capacity`); a Way's node indices use a separate `Vec<u32>` plus an offset table, avoiding a per-Way `Vec` allocation (1M Ways × 24 B header overhead = 24 MB of pure waste).

### 2.5 Streaming XML (`.osm`) Reading

`quick-xml 0.42` in a single-pass event loop: `read_event_into(&mut buf)` (reusing the buffer, zero allocation), with `Event::Start/Empty/End` dispatched to `start_node/flush_node`, ignoring `bounds`/`note`/`meta`, and `buf.clear()` at the end of the loop (as required by the borrow checker):

```rust
loop {
    match reader.read_event_into(&mut buf)? {
        Event::Start(e) if e.name() == b"node"  => start_node(e.attributes())?,
        Event::Empty(e) if e.name() == b"node"  => flush_node()?,   // node without tags
        Event::Empty(e) if e.name() == b"tag"   => push_tag(e.attributes())?,
        Event::End(e)   if e.name() == b"node"  => flush_node()?,
        Event::Eof => break,
        _ => {}
    }
    buf.clear();
}
```

- **Security**: no DTD / external entity parsing; `Event::DocType` is explicitly rejected (`quick-xml` does not parse DTDs by default, but XXE and billion-laughs still need an upper-layer guard).
- **Attribute semantics**: `id` is mandatory; missing `version` ⇒ 0 (not uploaded); `visible="false"` ⇒ deleted state; `action="delete"` appears only in `.osc`, and appearing in `.osm` is treated as an error; `changeset`/`timestamp`/`uid`/`user` go into metadata.
- **Limits**: single-file size and element count are bounded by `ImportOptions`, exceeding them yields `Err(ImportError::TooLarge)` — a guard against accidentally dragging in a planet-level file, not a performance optimization.

### 2.6 OSC (OsmChange) Reading and Writing

`.osc` has three sections `create` / `modify` / `delete`; **actions apply to a whole primitive**, and tag-level patches do not exist, so `modify` must carry the full tag set. A delete block may carry `if-unused` (the value is ignored), meaning "skip this one if it is referenced, without failing the whole diff" ([OsmChange](https://wiki.openstreetmap.org/wiki/OsmChange)).

> **To verify**: the "OSC 0.3" in the task description does not exist in any public specification. The `osmChange` version attribute is `version="0.6"` (JOSM `OsmChangeBuilder.DEFAULT_API_VERSION = "0.6"`, wiki examples use `<osmChange version="0.6">`). NEO implements 0.6; if "0.3" refers to an internal format number, its source needs separate confirmation.

Round-trip fidelity requirements:

| Dimension | Requirement |
|---|---|
| Attribute order | fixed as `id, version, changeset, lat, lon, visible`; it need not match the input, but **two exports of the same input must be byte-identical** (determinism) |
| Floating point | coordinates are formatted directly with `{:.7}` from integer decimicros, never through `f64`, eliminating `-0.0000000` and last-digit jitter |
| Escaping | `&<>"'` and `\r\n\t` go through a shared `escape_xml`; non-ASCII is emitted as raw UTF-8, not as `&#x…;` |
| Empty elements | `<node …/>` self-closing |
| Indentation | none, terminated by `\n`, for easy diffs and golden comparisons |

**Golden test approach**: `tests/fixtures/osc/*.osc` and `.expected.osc` are checked in as pairs, and `parse → write` is asserted for **byte equality**; a separate `semantic_roundtrip` (`parse → write → parse → structural equality`) covers attribute defaults, invalid UTF-8, and `if-unused`. CI enforces LF on all three platforms (Windows CRLF is a source of false failures).

### 2.7 Writing: PBF / XML / OSC

**The reality of PBF writing**: `osmpbf 0.3.8` is a **read-only** library (the crate describes itself as "A fast reader for the OpenStreetMap PBF file format", with no encoder in the source), and `osmium`/`libosmium` are C++ dependencies. NEO therefore builds a minimal encoder, because the write side only needs **forward** encoding:

- a hand-written varint/zigzag/length-delimited writer (about 300 lines, `bytes` buffer), generating `BlobHeader`/`Blob`/`PrimitiveBlock`/`PrimitiveGroup`/`DenseNodes`/`Way`/`Relation` from the authoritative `.proto` schema, with no protobuf runtime dependency;
- reusing `osmpbf`'s bundled `fileformat.proto` / `osmformat.proto` as the **single source of truth** for field numbers and defaults (avoiding hand-copied wrong field numbers that produce unreadable files);
- before output, self-check by reading back through `osmpbf`; CI performs a "write → read → semantic comparison" for every output, which is the only trustworthy acceptance method for an encoder;
- zlib compression by default (matching the ecosystem), with `raw` only for tests; decompressed size per blob is < 16MiB (the specification's suggested value), which is how DenseNodes batches are split.

XML/OSC writing share the `XmlSink` trait; PBF and XML are just three `impl`s.

### 2.8 When the rstar Index Is Built, and the Applicability Limits of rkyv+mmap

**Do not insert into rstar while importing**: `RTree::insert` is a single-point insert that rebalances frequently; `RTree::bulk_load(Vec<T>)` is a one-shot O(n) packing that can exploit Hilbert/STR ordering for better clustering. Index construction is therefore fixed inside `StoreBuilder::finish()` and before snapshot publication (during construction there is no old snapshot to display, but this is an "open a file" rather than "incremental editing" scenario, so that is acceptable).

**rkyv + memmap2 archives** (`rkyv 0.8.18`'s `to_bytes`/`access`, `memmap2 0.9.12` read-only mapping):

| Scenario | Applicable | Reason |
|---|---|---|
| read-only large basemap (country/province extracts) | ✅ | zero deserialization after archiving, `access` borrows the memory directly; OS page cache pages in on demand |
| session snapshots / autosaves | ✅ | written once, rarely read; crash recovery only needs `access` + validation |
| active layers with frequent incremental edits | ❌ | archives are immutable, so any single edit requires rewriting the whole thing; edit state must use SoA |
| cross-version compatibility | ❌ | layout is bound to the type definitions and byte order; use `schema_version` to explicitly reject old archives, with no compatibility promise |
| network transfer / cross-machine sharing | ⚠️ | requires a fixed byte order and pointer width; only usable on the same architecture |

Alignment: `rkyv` requires type alignment (usually 8/16 B), while `memmap2` file offsets are uncontrolled, so the archive header writes `align_pad` to push the root object to a 16 B boundary.

## 3 Key Interfaces

```rust
// neo-io/src/lib.rs
pub struct ImportOptions {
    pub bbox: Option<BboxFilter>, pub threads: Option<usize>,   // None = rayon default
    pub bytes_budget: Option<u64>,                             // None = compressed size * 4
    pub max_blob_in_flight: usize, pub keep_deleted: bool, pub precision: PrecisionPolicy,
}
pub struct ImportReport {
    pub nodes: u64, pub ways: u64, pub relations: u64, pub deleted: u64,
    pub precision_loss_objects: u64, pub ref_integrity: RefIntegrity, pub duration: Duration,
}
pub struct RefIntegrity {
    pub missing_nodes: u64, pub missing_members: u64,
    pub dangling_refs: Vec<(ObjectId, ObjectId)>,   // sampled up to 1024, the rest are only counted
}

pub struct BboxFilter { /* min/max lon/lat: i32 decimicro */ }
impl BboxFilter {
    pub fn new(min: LonLat, max: LonLat) -> Self;
    pub fn contains_node(&self, p: LonLat) -> bool;
    pub fn accepts_way(&self, node_hits: usize, envelope: Envelope) -> bool;
    pub fn accepts_relation(&self, member_hits: usize) -> bool;
}

/// Consumer abstraction for import: the PBF/XML/OSC front ends share one back end.
pub trait ElementSink {
    fn begin(&mut self, header: &ImportHeader) -> Result<()>;
    fn nodes(&mut self, batch: &NodeBatch) -> Result<()>;
    fn ways(&mut self, batch: &WayBatch) -> Result<()>;
    fn relations(&mut self, batch: &RelationBatch) -> Result<()>;
    fn end(&mut self) -> Result<ImportReport>;
}

pub struct ImportPipeline { /* BlobReader factory + rayon pool + budget */ }
impl ImportPipeline {
    pub fn new(opts: ImportOptions) -> Self;
    pub fn run_pbf<P: AsRef<Path>, S: ElementSink>(&self, path: P, sink: &mut S) -> Result<ImportReport>;
    pub fn run_xml<P: AsRef<Path>, S: ElementSink>(&self, path: P, sink: &mut S) -> Result<ImportReport>;
    pub fn cancel(&self);                    // idempotent; batches already submitted are no longer appended
}
pub struct StoreBuilderSink<'a> { /* &'a mut StoreBuilder */ }
impl ElementSink for StoreBuilderSink<'_> { /* ... */ }

pub enum ExportFormat { Pbf(PbfWriteOptions), Xml(XmlWriteOptions), Osc(OscWriteOptions) }
pub struct PbfWriteOptions { pub compression: Compression, pub max_block_entities: usize }
pub enum Compression { None, Zlib }
pub struct XmlWriteOptions { pub pretty: bool, pub visible_attr: bool }
pub struct OscWriteOptions { pub changeset: Option<i64>, pub if_unused: bool }

pub struct Exporter;
impl Exporter {
    pub fn write<S: SnapshotView, W: Write>(fmt: ExportFormat, snap: &S, w: W) -> Result<WriteReport>;
    pub fn write_osc<W: Write>(diff: &EditDiff, opts: OscWriteOptions, w: W) -> Result<WriteReport>;
}

pub struct OscWriter<W: Write> { /* keeps current_mode internally to merge consecutive same-action blocks */ }
impl<W: Write> OscWriter<W> {
    pub fn new(w: W, opts: OscWriteOptions) -> Self;
    pub fn start(&mut self) -> Result<()>;
    pub fn write_created(&mut self, e: ElementView<'_>) -> Result<()>;
    pub fn write_modified(&mut self, e: ElementView<'_>) -> Result<()>;
    pub fn write_deleted(&mut self, e: ElementView<'_>) -> Result<()>;
    pub fn finish(&mut self) -> Result<WriteReport>;
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("pbf decode: {0}")] Pbf(String),
    #[error("xml at {line}: {msg}")] Xml { line: u64, msg: String },
    #[error("unsupported required feature: {0}")] UnsupportedFeature(String),
    #[error("coordinate out of range")] CoordinateOverflow,
    #[error("invalid utf-8 in string table at {index}")] InvalidUtf8 { index: u32 },
    #[error("input too large: {0}")] TooLarge(String),
    #[error("cancelled")] Cancelled,
}
```

## 4 Data Flow and Timing

```text
UI/CLI        ImportPipeline      BlobReader      rayon pool       StoreBuilder
  │ start           │                 │               │                │
  │────────────────▶│ open + header   │               │                │
  │                 │── required_feat▶│               │                │
  │                 │◀─ HeaderBlock ──│               │                │
  │◀─ progress(0%) ─│                 │               │                │
  │                 │─ reserve(bytes) ──────────────────────────────▶ │
  │                 │── next_blob ────▶│               │                │
  │                 │                 │── Blob ──────▶│ decode         │
  │                 │                 │               │─ NodeBatch ───▶│ append
  │                 │◀─ release(bytes) ────────────────────────────── │
  │◀─ progress(n%) ─│        … loop until EOF …       │                │
  │                 │──── finish() ─────────────────────────────────▶ │ id map
  │                 │                 │               │                │ ref set
  │                 │                 │               │                │ rstar bulk_load
  │                 │◀────────── Snapshot (Arc, read-only) ───────────│
  │◀─ ImportReport ─│                 │               │                │
  │ cancel()        │                 │               │                │
  │────────────────▶│ set flag; drop pending blobs; skip finish()       │
  │◀─ Err(Cancelled)│                 │               │                │

.osc ─▶ OscReader ─▶ EditDiff{created/modified/deleted}
                        ├─▶ Exporter::write_osc (byte-level golden comparison)
                        └─▶ Editor::apply_batch (local preview)
```

## 5 Trade-offs and Alternatives

| Decision | Choice | Alternative | Why rejected |
|---|---|---|---|
| parallelism granularity | blob level | element level | element level cannot share the string table; delta encoding makes it non-randomly splittable |
| high-level API | self-built scheduling + `BlobReader` | `ElementReader::par_map_reduce` | reduce semantics do not support backpressure or ordered batch appends |
| coordinate representation | `i32` decimicro | `f64` / `i64` nano | `f64` round trips are not fixed-point; `i64` doubles memory |
| PBF writing | own encoder | `osmium` FFI | brings a C++ build chain and cross-platform distribution burden; `osmpbf` has no write side |
| XML parsing | `quick-xml` events | `serde-xml-rs`/DOM | DOM is incompatible with planet-level input; serde derives cannot handle "same name in different contexts" |
| index timing | `bulk_load` at the end of import | per-item `insert` | rebalancing during inserts + random writes is significantly worse than one-shot packing |
| archiving | rkyv+mmap (read-only basemap) | SQLite query per frame | see [10](./10-persistence-and-recovery.md) §2.1 |

## 6 Risks

| Risk | Impact | Mitigation |
|---|---|---|
| malicious/corrupt blobs (header size, lying `raw_size`) | OOM / panic | rely on `osmpbf`'s hard limits of 64KiB/32MiB; additionally compare `raw_size` against the budget |
| data with non-default `granularity` | silent precision loss | counted in the import report + `PrecisionPolicy::Reject` strict mode |
| invalid UTF-8 in the string table | import aborts | record the error + configurable "skip and count" degradation |
| wrong field numbers in our encoder | unreadable output file | write→read self-check + CI round trip; hand-editing the `.proto` copies is forbidden |
| mmap archive truncated/rewritten externally | SIGBUS / UB | validate magic + `schema_version` + total length + CRC on open; `unsafe` confined to the `archive` module |
| newline/encoding differences across three platforms | golden test false failures | fixtures forced to LF; the write side emits explicit `\n` |
| cancellation races (a batch being written) | half-built result | `finish()`/`cancel()` are mutually exclusive (`AtomicBool` + a `Mutex` guarding store ownership) |

## 7 Acceptance Criteria

- [ ] `cargo test -p neo-io` covers import of the three formats (including `visible=false`, nodes without tags, and missing references), export, and round trips.
- [ ] Reading NEO-written `.osm.pbf` back with `osmpbf` yields exactly matching counts and tags (encoder acceptance).
- [ ] Every element written into OSC carries `changeset` and `version` (except create); upload returns `diffResult` rather than 400/409.
- [ ] In `--bbox` import results, every retained Way has at least one node inside the box (or is explicitly recorded per the `accepts_way` policy).
- [ ] After cancelling an import: no snapshot published, no temporary files left behind, `StoreBuilder` memory freed (`Drop` assertion + peak sampling).
- [ ] Time and peak RSS for 1M / 10M object imports are recorded in [14](./14-performance-and-verification.md), with charts generated by `benches/`; this document does not cite specific numbers.
- [ ] `clippy -p neo-io -- -D warnings` passes; `unsafe` appears only in the `archive`/`mmap` modules, each with a `// SAFETY:` note.

## 8 References

- PBF format (BlobHeader 32KiB/64KiB, Blob 16MiB/32MiB, default `granularity` 100, default `date_granularity` 1000, `lat_offset`, single-type PrimitiveGroup, `required_features`, `Sort.Type_then_ID`): <https://wiki.openstreetmap.org/wiki/PBF_Format>
- `osmpbf 0.3.8` (read-only; `BlobReader`/`PrimitiveBlock`/`DenseNodeIter`): <https://docs.rs/osmpbf/0.3.8/osmpbf/>
- `quick-xml 0.42` (`from_reader`, `read_event_into`, `config_mut`): <https://docs.rs/quick-xml/0.42.0/quick_xml/>
- OsmChange (`create`/`modify`/`delete`, `if-unused`, `version="0.6"`): <https://wiki.openstreetmap.org/wiki/OsmChange>
- OSM API v0.6 (7-decimal coordinates, `version`/`changeset` semantics): <https://wiki.openstreetmap.org/wiki/API_v0.6>
- `rstar 0.14` `bulk_load`: <https://docs.rs/rstar/0.14.0/rstar/struct.RTree.html#method.bulk_load>
- `rkyv 0.8.18`: <https://docs.rs/rkyv/0.8.18/rkyv/> · `memmap2 0.9.12`: <https://docs.rs/memmap2/0.9.12/memmap2/>
- Related: [Data Model and Store](./03-osm-data-model-and-store.md) · [API and Sync](./09-osm-api-and-sync.md) · [Persistence and Recovery](./10-persistence-and-recovery.md) · [Performance and Verification](./14-performance-and-verification.md)
