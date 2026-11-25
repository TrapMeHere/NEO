# 09 · OSM API 0.6 and Sync (API & Sync)

This document defines `neo-api`: the client and sync strategy for OSM API 0.6, covering OAuth2 authentication, changeset lifecycle, diff upload, conflict handling, on-demand downloads, and the offline queue. The design baseline is **optimistic locking + server transactions do not span requests**: the client must take on "version checking, chunking, and compensation" itself, because the server only guarantees atomicity for a single `upload` request.

## 1 Goals and Scope

| # | Goal | Judgement |
|---|---|---|
| S1 | Fully replace the JOSM upload path: authenticate → create changeset → upload diff → close changeset | `diffResult` reports full success |
| S2 | Conflicts (409/412) are localizable to a specific object and come with a resolution entry point | `ConflictReport` is actionable per object |
| S3 | Network loss / failure never loses edits: queue + replay | the queue still exists after a restart ([10](./10-persistence-and-recovery.md)) |
| S4 | Download results satisfy referential integrity; missing references are **modeled explicitly** | a non-empty `DownloadBatch::incomplete` triggers a UI notice |

**Out of scope**: Overpass, replication diffs (minute/hourly), GPX upload (multipart), diary, user-preference writes; `/api/0.6/notes` is read-only plus "create note".
**Dependencies**: `neo-api → neo-core, neo-osm` (no dependency on store/editor; diffs are passed in from above as `EditDiff`); network stack `tokio 1.54` + `reqwest 0.13.5`, errors via `thiserror 2.0`.

## 2 Design

### 2.1 Endpoint Matrix (verified)

> **Important correction**: the `POST /api/0.6/changeset/create` from the task description is actually **`PUT`** ([API v0.6 §2.2.2](https://wiki.openstreetmap.org/wiki/API_v0.6)). Confusing it with `POST changeset/{id}/upload` yields an immediate 405.

| Purpose | Method and path | Auth | Key constraints |
|---|---|---|---|
| Service capabilities | `GET /api/0.6/capabilities` | read, no auth | `area maximum="0.25"`, `waynodes=2000`, `relationmembers=32000`, `changesets maximum_elements=10000`, `note_area=25` |
| Create changeset | `PUT /api/0.6/changeset/create` | `write_api` | body `<osm><changeset><tag k="comment" v="…"/></changeset></osm>`, response is a plain-text id |
| Update tags | `PUT /api/0.6/changeset/{id}` | `write_api` | append `comment`/`created_by` |
| Close | `PUT /api/0.6/changeset/{id}/close` | `write_api` | uploading after close ⇒ 409 |
| Upload diff | `POST /api/0.6/changeset/{id}/upload` | `write_api` | the body is OsmChange XML (`application/xml`), **not multipart** |
| Download by bbox | `GET /api/0.6/map?bbox=left,bottom,right,top` | read, no auth | area ≤ 0.25 square degrees; over **50,000 nodes** ⇒ 400 |
| Multi-object fetch | `GET /api/0.6/{nodes\|ways\|relations}?{type}=id,…` | read, no auth | the primary means of completing missing members |
| Referrers | `GET /api/0.6/{type}/{id}/relations` | read, no auth | recursive parents |
| Full object | `GET /api/0.6/{way\|relation}/{id}/full` | read, no auth | brings along nodes and members |
| notes | `GET /api/0.6/notes?bbox=…`, `POST /api/0.6/notes` | read / `write_notes` | area ≤ 25 square degrees |

The upload body is **not** `multipart/form-data` (multipart is required only for GPX upload to `/api/0.6/gpx`); `Content-Type` may be omitted (it defaults to XML/UTF-8), and request-body gzip (`Content-Encoding: gzip`) has been supported since 2018. The `diffResult` response has one line per uploaded element, and an element uploaded multiple times appears multiple times:

```xml
<diffResult generator="OpenStreetMap Server" version="0.6">
  <node old_id="-1" new_id="123456" new_version="1"/>
  <way  old_id="-2" new_id="123457" new_version="2"/>
</diffResult>
```

A `create` line contains `new_id`+`new_version`; a `modify` line has the same input `new_id` and an incremented `new_version`; a `delete` line has **no `new_version`**. The remapping from placeholder ids (negative) to real ids can **only** rely on this response — it is the sole authority for aligning the local Store with the server.

### 2.2 Authentication (OAuth 2.0)

OAuth 1.0a and HTTP Basic were retired in June 2024, leaving only OAuth 2.0. Required scopes:

| scope | Permission name | Purpose |
|---|---|---|
| `read_prefs` | `allow_read_prefs` | user preferences, `user/details` |
| `write_api` | `allow_write_api` | map edits + changeset comments |
| `write_notes` | `allow_write_notes` | writing notes |
| `consume_messages` | `allow_consume_messages` | site messages (optional) |

Scopes in `/permissions` are uniformly prefixed with `allow_` — permission comparison must normalize them, otherwise everything is always judged "unauthorized". **Token storage**: access/refresh tokens go into the OS credential store (Windows Credential Manager / macOS Keychain / libsecret), and **never** into SQLite or the configuration file; SQLite stores only `account_id` and a scope digest. A failed refresh degrades to read-only mode: the upload button is disabled and shows the reason.

> To verify: the exact OAuth2 authorization/token endpoint paths (usually `https://www.openstreetmap.org/oauth2/authorize` and `/oauth2/token`) could not be verified online this time (`web_fetch` failed). Re-check <https://wiki.openstreetmap.org/wiki/OAuth> before implementing, and confirm whether PKCE is required.

### 2.3 HTTP Client: Connection Pool, Rate Limiting, Retries, Cancellation

- **Connection pool**: a single `reqwest::Client` reused globally (HTTP/2 multiplexing, keep-alive), `pool_max_idle_per_host(4)`, `timeout(30s)` + `connect_timeout(5s)`.
- **Concurrency**: downloads default to **2** (aligned with the OSM usage policy; JOSM also implements `MAX_DOWNLOAD_THREADS = 2`); uploads are always **1** (sequential uploads within one changeset, avoiding bbox contention and ordering confusion).
- **Rate limiting**: token bucket (default 2 req/s, burst 4); on 429, back off per `Retry-After`, temporarily halve the bucket capacity, and recover slowly after success.
- **Retries**: automatic retries only for idempotent requests (GET, `capabilities`) and failures that "definitely did not commit" (connection-establishment failure, TLS errors); `upload` is retried **only when the request was never sent** (`reqwest`'s `is_connect()`), otherwise it returns `Ambiguous` for the caller to verify — never risking duplicate object creation.
- **Backoff**: exponential + jitter, base 500ms, cap 60s, at most 5 attempts (the same order of magnitude as JOSM's `DEFAULT_MAX_NUM_RETRIES = 5`).
- **Cancellation and progress**: our own `CancelToken` (`Arc<AtomicBool>` + `tokio::sync::Notify`, without pulling in `tokio-util`); every `await` point is wrapped in `tokio::select!`, and after cancellation the request is dropped (HTTP/1.1 closes the connection, HTTP/2 sends RST_STREAM). Progress uses `watch::Receiver<Progress>`, advancing by "objects downloaded / elements uploaded".

### 2.4 Changeset Lifecycle

```text
 ┌────────┐  create(PUT)   ┌────────┐  upload(cls)  ┌───────────┐  close  ┌────────┐
 │  Idle  │───────────────▶│  Open  │──────────────▶│ Uploading │────────▶│ Closed │
 └────────┘                └────────┘               └───────────┘         └────────┘
      ▲                        │  ▲ append tags / stay Open on failure
      └── discard (not created)┘  └── auto-close on idle timeout
```

1. **Lazy creation**: `PUT changeset/create` happens only when the user first clicks "upload", avoiding empty changesets polluting history.
2. **Tags**: `comment` (mandatory; upload is disabled when empty), `created_by=NEO/<version>`, `host`, `locale`, plus optional `source`, `imagery_used`. `created_by` is a community convention rather than an API requirement, but its absence invites criticism.
3. **Auto-close**: when `Open` and idle past a threshold (default 30 minutes, configurable), close in the background, avoiding the server's 24h auto-close leading to "upload to a closed changeset ⇒ 409".
4. **Pre-check against limits**: before uploading, compare `capabilities.changesets.maximum_elements` (currently 10000) with the number of elements already uploaded and require splitting into multiple changesets if exceeded — do not rely on discovering it via 413.
5. **Close failure**: 409 (already closed) counts as success (idempotent); other errors keep `Open` and notify, and the next upload reuses the same changeset.

### 2.5 Upload Pipeline

```text
EditDiff(local diff)
 │ 1. validate: no modify with version=0; delete dependency order (relations before ways before nodes)
 ▼
UploadPlan{changeset tags, chunks[]}
 │ 2. conflict precheck (on by default): multi_fetch the current version of all modify/delete
 │    mismatch ⇒ produce ConflictReport directly (saving a round trip that would certainly 409)
 ▼
for chunk in chunks:                          ← sequential, single concurrency
 │ 3. serialize: OscWriter output (changeset/version mandatory, create exempt from version)
 │ 4. POST upload (gzip allowed)
 │    ├─ 200 + diffResult ⇒ 5
 │    ├─ 409 ⇒ version mismatch / changeset closed / not the owner (text/plain)
 │    ├─ 412 ⇒ missing node or still referenced ("Way #1 requires the nodes with id in (…)")
 │    └─ 429/5xx ⇒ backoff and retry (only when it can be determined the request had no effect)
 ▼
5. apply diffResult: placeholder id → real id; version overwrite; write back to the Store via the Editor
 │
6. all succeeded ⇒ close; any failure ⇒ stay Open + ConflictReport
```

**The conflict between chunking and atomicity** (the most important trade-off in this design): the server guarantees that a **single** `upload` is "all applied or none applied", but **not across requests**.

| Approach | Atomicity | Cost |
|---|---|---|
| single request uploading the whole changeset | ✅ server transaction | limited by `maximum_elements`=10000 and request-body size |
| multiple requests in chunks | ❌ partial success | requires client-side compensation |
| multiple changesets | ❌ split semantics | multiple changesets appear in history, unfriendly to the community |

**Decision**: prefer a single request (covering the vast majority of sessions); chunk only when the element count > `maximum_elements` or the request body exceeds a threshold (default 8MiB, to avoid proxy truncation), and then it must: (1) persist `uploaded_until` immediately after each chunk succeeds, so a crash leads to **resuming rather than re-uploading**; (2) stop after the first failed chunk and stay `Open`, with the UI showing "N chunks uploaded, M remaining"; (3) on giving up, offer "compensating upload" — generate a **reverse OSC** for the already-uploaded elements (modify back to old values / delete newly created objects) as a new request. Compensation is **best effort**, not a transaction rollback: the server has no revert endpoint, the reverse diff is itself a legitimate edit, and if someone changes things meanwhile it will 409 too.

**Failure semantics** (must be precise, no vagueness allowed):

| Failure point | Server state | Client action |
|---|---|---|
| request never sent / connection failed | unchanged | safe to retry |
| sent but no response (timeout) | **unknown** | mark `Ambiguous`, `GET` the version to check before deciding |
| 409 (version updated by someone else) | unchanged (whole request rolled back) | produce `ConflictReport` |
| 409 (changeset already closed) | unchanged | create a new changeset and re-upload |
| 412 (missing/used reference) | unchanged | download missing members, or use `if-unused` for deletes |

### 2.6 Download Strategy and Referential Integrity

| Strategy | Implementation | Applicable to |
|---|---|---|
| by bbox | `GET /map?bbox=`, area ≤ 0.25 square degrees, nodes ≤ 50k | primary download |
| tiling | split into tiles ≤ 0.25 square degrees, two concurrent | city-level areas |
| along a path/track | geometry envelope + corridor buffer (default 100m) generating a sequence of elongated bboxes, deduplicated, downloaded chunk by chunk | mapping along roads |
| parent recursion | `GET {type}/{id}/relations` walking upward, depth 3 by default | avoiding broken relations |
| member completion | `GET ways?ways=…` / `nodes?nodes=…` batch fetch | incomplete members |
| full object | `GET {way,relation}/{id}/full` | deep links to a single object |

**Handling a Way with missing Nodes under partial data** (explicit, never silently dropped):

1. A Way may reference a nonexistent `ObjectId`, resolved in the Store as `Handle::MISSING` (a generation=0 sentinel);
2. The render layer skips missing nodes and draws a break marker at the gap (**does not** connect them with a straight line, avoiding fabricated geometry);
3. `neo-validate` produces an `incomplete_way` warning and **upload is hard-blocked**: any Way referencing a missing node is filtered out, and the `ConflictReport` explains why — the server would reject the whole diff with 412, so blocking locally is far cheaper;
4. The user may choose "complete and retry": automatic multi_fetch, and if that still fails, mark it `deleted` (the server semantics being invisible).

> Detail: `GET /map` only returns roads that "have a node inside the box"; a Way that intersects the box but has all its nodes outside it **will not** be returned. Ways at the box boundary therefore often show up in "missing node" form — a common source of partial data, and the UI copy must explain it clearly.

### 2.7 Offline Queue

Pending diffs produced by editing land in SQLite's `changeset_queue` (DDL in [10](./10-persistence-and-recovery.md) §2.2): `client_uuid` is the idempotency key (repeated enqueues of the same edit keep only one); transitions of `state ∈ {queued, uploading, uploaded, conflicting, aborted}` must commit in the same transaction as `uploaded_until`; replay follows `seq` and **never reorders across changesets** (create-before-reference dependencies are not commutative); while offline the UI shows "N items unsynced", and a second edit to the same object is merged into the same payload.

## 3 Key Interfaces

```rust
// neo-api/src/lib.rs
pub struct ApiConfig { pub base_url: reqwest::Url,      // default https://api.openstreetmap.org
    pub user_agent: String,                             // mandatory: NEO/0.1 (+https://…)
    pub max_download_concurrency: usize,                // default 2
    pub request_timeout: Duration, pub max_retries: u32, pub gzip_upload: bool, pub offline: bool }

pub struct OsmApiClient { /* reqwest::Client + AuthProvider + RateLimiter + CancelToken */ }
impl OsmApiClient {
    pub fn new(cfg: ApiConfig, auth: Arc<dyn AuthProvider>) -> Result<Self, ApiError>;
    pub async fn capabilities(&self) -> Result<Capabilities, ApiError>;
    pub async fn map_bbox(&self, bbox: Bbox) -> Result<DownloadBatch, ApiError>;
    pub async fn multi_fetch(&self, ids: &ObjectIdSet) -> Result<DownloadBatch, ApiError>;
    pub async fn full(&self, id: ObjectId) -> Result<DownloadBatch, ApiError>;
    pub async fn parents_of(&self, id: ObjectId) -> Result<Vec<ObjectId>, ApiError>;
    pub async fn notes(&self, bbox: Bbox) -> Result<Vec<Note>, ApiError>;
    pub async fn create_changeset(&self, meta: &ChangesetMeta) -> Result<i64, ApiError>;
    pub async fn update_changeset(&self, id: i64, meta: &ChangesetMeta) -> Result<(), ApiError>;
    pub async fn close_changeset(&self, id: i64) -> Result<(), ApiError>;
    pub async fn upload_diff(&self, cs: i64, osc: &[u8]) -> Result<DiffResult, ApiError>;
    pub fn cancel_token(&self) -> CancelToken;
    pub fn progress(&self) -> tokio::sync::watch::Receiver<Progress>;
}

pub struct Capabilities { pub max_area_sq_deg: f64,      // 0.25
    pub max_note_area_sq_deg: f64,                      // 25
    pub max_way_nodes: u32,                             // 2000
    pub max_relation_members: u32,                      // 32000
    pub max_changeset_elements: u32,                    // 10000
    pub database_status: ServiceStatus, pub api_status: ServiceStatus }

#[async_trait::async_trait]
pub trait AuthProvider: Send + Sync {
    async fn access_token(&self) -> Result<secrecy::SecretString, ApiError>;
    async fn scopes(&self) -> Result<Vec<Scope>, ApiError>;
    fn is_read_only(&self) -> bool;
}

pub struct ChangesetManager { /* state machine + idle timer + queue write-back */ }
impl ChangesetManager {
    pub fn new(client: Arc<OsmApiClient>, queue: Arc<dyn QueueStore>) -> Self;
    pub async fn ensure_open(&mut self, meta: &ChangesetMeta) -> Result<i64, ApiError>;
    pub async fn upload(&mut self, plan: UploadPlan) -> Result<UploadOutcome, ApiError>;
    pub async fn close(&mut self) -> Result<(), ApiError>;
    pub fn state(&self) -> ChangesetState;
}

pub struct UploadPlan { pub changeset_meta: ChangesetMeta, pub chunks: Vec<UploadChunk>, // usually exactly 1
    pub precheck_versions: bool, pub compensation: CompensationPolicy }  // None | InverseOsc
pub struct UploadChunk { pub osc: Vec<u8>, pub element_count: usize, pub kind: ChunkKind }

pub struct ConflictReport { pub kind: ConflictKind, pub changeset_closed: Option<ConflictSummaryLine>,
    pub per_object: Vec<ObjectConflict>, pub message: String }   // the server's raw text/plain message
pub struct ObjectConflict { pub id: ObjectId, pub stage: ConflictStage,  // Precheck | Upload409 | Upload412
    pub local_version: i32, pub remote_version: Option<i32>, pub resolution: ConflictResolution }

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized: {0}")] Auth(String),
    #[error("rate limited, retry after {0:?}")] RateLimited(Duration),
    #[error("conflict: {0}")] Conflict(Box<ConflictReport>),
    #[error("payload rejected: {0}")] Rejected(String),        // 400/412/413
    #[error("ambiguous outcome: request may have been applied")] Ambiguous,
    #[error("cancelled")] Cancelled,
    #[error("network: {0}")] Transport(#[from] reqwest::Error),
    #[error("decode: {0}")] Decode(String),
}
```

## 4 Data Flow and Timing

```text
User   ChangesetManager   OsmApiClient        Server           Store/Editor
 │ upload    │                 │                │                 │
 │──────────▶│ ensure_open     │                │                 │
 │           │── PUT create ──▶│───────────────▶│ id=555          │
 │           │ precheck: multi_fetch current versions             │
 │           │────────────────▶│───────────────▶│                 │
 │           │◀── node 42 local=3 remote=5 ──────│                 │
 │◀─ conflict panel (TakeRemote / Merge / Skip)  │                 │
 │ choose TakeRemote for 42  │                │                 │
 │──────────▶│ recompute diff (discard local tag edits on 42)     │
 │           │── POST upload (OsmChange)──────▶│ txn: all-or-nothing
 │           │◀──── diffResult(old→new) ────────│                 │
 │           │ apply id/version remap ───────────────────────────▶│
 │           │── PUT close ──▶│───────────────▶│                 │
 │◀─ done ───│                 │                │                 │

timeout ⇒ Ambiguous:
client ──POST upload──▶ [timeout] ──▶ no retry
client ──GET /api/0.6/node/42─▶ remote_version == local expectation + 1 ?
    ├─ yes ⇒ treat as succeeded, apply version, continue with the next chunk
    └─ no  ⇒ treat as not applied, retry a limited number of times (flagging the risk)
```

## 5 Trade-offs and Alternatives

| Decision | Choice | Alternative | Why rejected |
|---|---|---|---|
| upload granularity | single request by default | always chunk | chunking sacrifices atomicity just to dodge the 10k limit |
| conflict detection | precheck then upload | rely on 409 only | the precheck saves a round trip that is certain to fail; it can be disabled |
| retry policy | no automatic retry for non-idempotent requests | retry everything | `upload` may already have taken effect, so retrying would create duplicate objects |
| token storage | OS credential store | SQLite | plaintext token leakage is unacceptable |
| cancellation mechanism | our own `CancelToken` | `tokio-util::CancellationToken` | `tokio-util` is not in the dependency list; don't add a dependency for one bool + Notify |
| download concurrency | 2 | scale by core count | violates the OSM usage policy and invites 429s/bans |
| missing references | explicit `MISSING` + upload blocking | silently drop the Way | silent dropping manufactures "invisible data corruption" |

## 6 Risks

| Risk | Impact | Mitigation |
|---|---|---|
| changeset auto-closed by the server after 24h | upload 409 | auto-close after 30 minutes idle; create a new one and re-upload on 409 |
| crash in the middle of chunked upload | partial data already committed | transactionally persist `uploaded_until`, resume from the next chunk after recovery |
| proxy/gateway truncates a large request body | 400 or half an XML document | request-body cap of 8MiB, chunk beyond that; gzip reduces the chance |
| expired token / insufficient scope | 403, user confusion | validate scopes before upload, and give actionable hints for missing ones |
| server in read-only maintenance (`status=readonly`) | all writes fail | cache `capabilities` for 5 minutes; disable upload entry points while `readonly` |
| reverse compensation fails | data half-changed | record the compensation result separately and tell the user to "manually verify changeset #id" |
| version checks based on a local increment | false conflict detection | trust only versions returned by the server |

## 7 Acceptance Criteria

- [ ] Against the [OSM development server](https://master.apis.dev.openstreetmap.org) (not production): create changeset → create node/way → upload → close → download and compare.
- [ ] Assert that the `upload` request body is OsmChange XML (`application/xml`) and **contains no** multipart boundary.
- [ ] Construct a scenario where the version is 1 behind and get `ApiError::Conflict`, with `per_object` containing that object and `remote_version`.
- [ ] Construct a Way-missing-node scenario and confirm the upload is blocked locally with a readable 412 explanation.
- [ ] After network loss the queue is not lost; on recovery it replays in `seq` order and `client_uuid` deduplication works.
- [ ] 50k nodes / 0.25 square degrees precheck: an over-limit request is rejected before being sent (without spending a 400).
- [ ] Tokens never appear in any log/`tracing` field (`secrecy` wrapper + integration test).
- [ ] Upload/download throughput and 429 rate are recorded in [14](./14-performance-and-verification.md) [pending measurement, see docs/14-performance-and-verification.md].
- [ ] `cargo test -p neo-api` + `clippy -- -D warnings` pass.

## 8 References

- The full API v0.6 endpoint set, error codes, capabilities fields (`area maximum`, `waynodes`, `changesets maximum_elements`, the 50'000-node cap): <https://wiki.openstreetmap.org/wiki/API_v0.6>
- OsmChange format (`create`/`modify`/`delete`, `if-unused`): <https://wiki.openstreetmap.org/wiki/OsmChange>
- OAuth 2.0 and scopes: <https://wiki.openstreetmap.org/wiki/OAuth> (endpoint details `> to verify`)
- Development server (for acceptance): <https://master.apis.dev.openstreetmap.org>
- `reqwest 0.13`: <https://docs.rs/reqwest/0.13.5/reqwest/> · `tokio 1.54`: <https://docs.rs/tokio/1.54.0/tokio/>
- Related: [IO Layer](./08-io-pbf-xml-osc.md) · [Persistence and Recovery](./10-persistence-and-recovery.md) · [Data Model and Store](./03-osm-data-model-and-store.md)
