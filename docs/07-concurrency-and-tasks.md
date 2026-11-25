# 07 · Concurrency & Tasking

NEO's concurrency model has a single main line: **the UI thread is the only writer; everything else consumes read-only snapshots**. Snapshots are switched atomically via `arc-swap`, so the read path is lock-free; background tasks interact with the foreground through a cancellable task system with progress and backpressure; GPU resources are created and uploaded exclusively by `neo-render`, and the UI thread never blocks on `map_async` / `poll(wait)`. For the overall picture see [System Architecture](./01-system-architecture.md) §2.4, for write semantics see [Editing Core and Undo](./06-editing-core-and-undo.md), and for GPU details see [Renderer](./05-renderer-wgpu.md).

## Summary

An editor is naturally a "user performs one action at a time" model: **writes must be serialized; only reads can be parallel** (geometry building, validation, scene assembly, decoding). NEO therefore does not do fine-grained locking or multi-version merging. Instead it reduces concurrency to "one writer + several read-only snapshot consumers", pins the rules down with the type system, and makes the remaining risk explicit in a deadlock checklist.

## 1 Goals and Scope

**In scope**: thread / executor topology and CPU budget; single-writer and snapshot publication protocol; task system (cancellation / progress / priority / frame budget / backpressure / channel choices); GPU resource ownership and incremental upload; deadlock and race checklist.

**Out of scope**: transaction semantics of commands ([06](./06-editing-core-and-undo.md)), chunking strategy for parallel PBF decoding ([08](./08-io-pbf-xml-osc.md)), pipelines and shaders ([05](./05-renderer-wgpu.md)), measured frame time and throughput ([pending measurement, see docs/14-performance-and-verification.md]).

## 2 Design

### 2.1 Thread Topology

The reference machine is an i5-12400F (6 physical / 12 logical cores), 31.8 GB RAM, RTX 3070 8 GB. Allocation principle: **UI and submission never give up a core**; the compute pool takes the remaining logical cores; networking and storage each get a dedicated thread so they do not compete with Rayon for cores.

| Executor | Count | What it touches | Allowed | Strictly forbidden |
|---|---|---|---|---|
| Main/UI (winit 0.30.13 event loop) | 1 | `&mut DataSet`, egui state, GPU submission | input, command execution, scene diffing, `prepare`/`encode`, `submit` | network/disk blocking, `poll(wait)`, holding a `Guard` across frames |
| Rayon compute pool | fixed 8 | `Arc<MapSnapshot>`, read-only geometry cache | pure computation for geometry/style/validation/decoding, resumable across frames | touching `DataSet`; unbounded posting to the UI |
| wgpu Queue | single queue (driver-internal thread) | `Device`/`Queue` | writing buffers, submitting encoders, present | a second `Instance`; creating a pass or calling `submit` inside `paint` |
| Tokio runtime | 2 workers + ≤4 blocking | HTTP body, `Arc<MapSnapshot>` | download/upload/retry/progress | writing the dataset directly, long CPU computation |
| Storage worker | 1 | serialization batches, SQLite (WAL) | journal append, autosave, recovery scan | calling back into the UI, holding a lock while calling the editor |
| WASM plugin (optional) | 1 per plugin | read-only `MapQuery` view + host calls | producing `EditCommand`, queries | direct IO/network/publishing snapshots (except explicitly authorized) |

```
+---------------------- Main/UI (only writer)-----------------------+
| egui::Context::run -> NeoApp::ui -> intents -> Editor::execute   |
| MapRenderer::prepare (incremental upload) -> encode -> queue.submit |
+--+-------------+--------------+--------------+------------------+
   | submit      | publish      | adopt frame  | readback poll
   v             v              v              v
+---------+ +----------+ +-----------+ +-----------------------+
| Rayon 8 | | arc-swap | | SceneFrame| | wgpu Queue            |
| geom/   | | lock-free| | double-   | | 2 frames in flight    |
| validate| | read     | | buffered  | | (configurable 3)      |
+---------+ +----------+ +-----------+ +-----------------------+
+---------+ +----------+
| Tokio 2 | | Storage  |  <- two independent channels, not merged into the Rayon pool
+---------+ +----------+
```

**Why there is no dedicated render thread**: egui's `PaintCallback` runs in the paint phase on the UI thread (`egui-wgpu`'s `CallbackTrait`), and picking and hover must be in the same frame as the UI; splitting out a thread would introduce at least 1 frame of input latency and force `Device` ownership to move across threads. If measured evidence that "scene building slows down submission" appears in the future, an ADR should evaluate a triple-buffered render thread; **the current split points are concentrated in `MapRenderer::prepare` (work inside the encoder, returning a `CommandBuffer`) and `encode` (pure encoding)**, neither of which holds a dataset reference. The UI thread is actually **three roles in one** (input + commands + submission), which is exactly the cost of the single-writer model ([ADR-0006](./adr/0006-single-writer-editor.md)) and the reason §2.6 must provide a budget.

### 2.2 Single Writer and Snapshot Publication

```
      UI thread (writer)                       any reader thread
      ----------------                         -----------------
Editor::execute(cmd)
      |  EditTx (shadow layer, DataSet unchanged)
      |  commit: journal -> index/dirty bitmap -> version+1
      v
SnapshotCell::store(MapSnapshot)  ──────►  ArcSwap::store(Arc<MapSnapshot>)
      |                                            |
      |                                            +-- load()      -> Guard (valid for one frame, !Send)
      |                                            '-- load_full() -> Arc<MapSnapshot> (can be held across threads)
      v
notify(touched)                            consumers: scene assembly, validate, autosave, plugin queries
```

| Rule | Content | Enforcement / verification |
|---|---|---|
| R1 | Only `Editor` holds `&mut DataSet`; `Editor` is not `Sync` | type system |
| R2 | Single publication point: only `SnapshotCell::store` writes the `ArcSwap` | review + grep (unique call site) |
| R3 | `Guard` is thread-local and must not be stored in a struct across frames | `Guard: !Send` + review |
| R4 | Background results must carry a `base_version`; on mismatch they are discarded as `Stale` (or explicitly rebased) | `TaskOutcome::Stale` branch must be tested |
| R5 | Readers must not "repair" snapshots: read-only traits carry no `&mut` methods | `MapQuery`/`SnapshotView` have no `&mut self` |
| R6 | Old snapshots live as long as their reference count; a background `Arc` pins the old version in memory | cancellation drops it; monitor `Arc::strong_count` |

**Key limitations of the current implementation (must be acknowledged)**: `MapSnapshot` is currently `Arc<DataSet>`, so publishing a new version requires a deep copy costing O(n). The first version therefore only publishes snapshots at low-frequency points such as **import completion, batch merge, and recovery**; rendering during interactive editing takes the single-threaded incremental path (dirty set + local rebuild). The target state is COW / chunked snapshots (unmodified chunks are shared), after which the interactive path can also "publish once per transaction". Why `arc-swap` instead of `RwLock<Arc<T>>`: the read side has no atomic RMW, no writer starvation, and `load()` does not block writers; the cost is a new `Arc` allocation per `store` and the thread-local `Guard` restriction.

### 2.3 Task System

| Concept | Type | Semantics |
|---|---|---|
| Task kind | `TaskKind::{Import, Scene, Validate, Upload, Download, Autosave, Plugin}` | determines queue, concurrency limit, default priority |
| Priority | `Priority::{Interactive, Normal, Background}` | Interactive gets at most one time slice per frame; Background is polled only when the UI is idle |
| Cancellation | `CancelToken` (`Arc<AtomicBool>`) | cooperative: checked every N elements or at phase boundaries; `Drop` cancels |
| Progress | `Progress { done, total, phase }` (atomics) | lock-free on the writer side, sampled at 10 Hz by the reader |
| Handle | `TaskHandle { id, kind, priority, token, progress }` | held by the UI for display and cancellation |
| Result | `TaskOutcome::{Ready, Stale, Cancelled, Failed}` | `Stale` results from a version-check failure |

| Queue | Capacity | Policy when full | Rationale |
|---|---|---|---|
| `scene` | 1 (overwrite) | a new request replaces the old one (keep only the latest view + version) | the scene only ever cares about the latest state |
| `import` | 1 | reject the second | single-writer installation, inherently mutually exclusive |
| `validate` | 1 | coalesce (reuse same version + same region) | results cached by `(version, bbox)` |
| `upload` | 1 | reject | changeset order matters |
| `download` | 8 | queue | chunk downloads can run concurrently |
| `autosave` / `plugin` | 1 / 1 per plugin | coalesce / reject | idempotent; prevents one plugin from starving everything |

**Channel choices**: UI ↔ worker always uses `crossbeam-channel 0.5` (bounded) — purely synchronous, `select!`, stable latency, and it does not leak a tokio dependency into `neo-ui`; the few edges needing `send_timeout` / `drain` semantics (such as batch flushing on shutdown) use `flume 0.12`; one-off result delivery reuses the same submission channel (one fewer channel = one fewer place for ordering bugs). `std::sync::mpsc` is not used (no `try_send` timeout, no MPMC), and `tokio::sync::mpsc` is not used as the UI channel (the UI is not async).

**Backpressure**: all channels are bounded; a failed `try_send` means degradation (progress dropped, scene request overwritten, import rejected), and **blocking send while holding a lock is not allowed** — this is also a deadlock defense (§2.5 D4). **The frame budget** is carried by a struct (fields are design targets, values are to be measured):

```rust
pub struct FrameBudget {
    pub frame: Duration,        // target frame time (derived from refresh rate)
    pub ui: Duration,           // egui logic
    pub scene_adopt: Duration,  // adopt a ready SceneFrame
    pub upload: Duration,       // buffer upload
    pub task_poll: Duration,    // total time slice per frame for polling background tasks
}
```

Degradation order when over budget: ① lower the scene LOD; ② postpone non-Interactive tasks; ③ reuse the previous frame's `SceneFrame` (visually lagging but no dropped frames).

### 2.4 GPU Resource Ownership

| Resource | Creator | Update | Destroyer | Lifetime |
|---|---|---|---|---|
| `Device` / `Queue` / `Instance` | `eframe` (`egui_wgpu::RenderState`) | — | `eframe` | process |
| `RenderPipeline` / `BindGroupLayout` / `Sampler` | `MapRenderer::new` (once at startup) | — | `MapRenderer::drop` | session |
| Vertex / index buffer | `BufferPool` | `write_buffer` inside `prepare`, or staging + copy | pool reuse / eviction | reused across frames |
| uniform buffer | ring at startup (2–3 slots) | `write_buffer` into the current slot each frame | process | aligned with frames-in-flight |
| Text atlas | allocated on first need | per-glyph `write_texture` | whole replacement on rebuild | growing |
| Picking texture + staging | `MapRenderer` (on demand) | GPU rendering | session | 2 staging buffers rotating |

**Ownership invariants**:

1. `neo-render` does not own `Device`, it only borrows `&Device`/`&Queue`; it must not create a second `Instance` bypassing eframe ([ADR-0005](./adr/0005-wgpu-egui-shared-device.md)).
2. Buffer creation / writing / recycling all happen in `prepare`/`collect` on the UI thread; background tasks only produce `SceneFrame` (CPU data) and **never touch the GPU API**.
3. `FRAMES_IN_FLIGHT = 2`: each slot owns its own vertex/index/instance/uniform buffers; a slot may be recycled only when `frame_no + in_flight < current`.
4. Incremental upload: `≤ 64 KiB` goes through `write_buffer`, anything larger goes through staging + `copy_buffer_to_buffer` (a large `write_buffer` implicitly allocates staging and may block); in steady state only the 32 B camera uniform is written per frame.
5. **The UI thread never blocks on `map_async`**: picking is `request()` (encoded in the current frame) → `try_take()` in the next frame (`PollType::Poll`). `poll(wait_indefinitely())` is allowed only on export / test paths; it amounts to a full pipeline flush.
6. While holding a `BufferView` you must not `unmap`/`destroy`/`poll`; `get_mapped_range` returns a `Result`, and an error is handled as "no picking result this frame"; the readback row pitch must be a multiple of 256 B.

### 2.5 Deadlock and Race Checklist

| ID | Risk | Typical scenario | Mitigation |
|---|---|---|---|
| D1 | Lock-order inversion | the editing thread takes `geo` then `store`, while the render path takes them in the opposite order | global order `store → geo → scene`; across modules only one lock may be taken "outward" |
| D2 | Sending to the UI while holding a lock | `send` to the UI queue while holding a `parking_lot::Mutex` that the UI is waiting on | take the data, release the guard, then send |
| D3 | Rendering waits for editing | `prepare` takes a snapshot while the writer holds the store lock and waits for the GPU | the writer never waits for the GPU (only `submit`); rendering does not touch `DataSet` |
| D4 | Full channels causing mutual waits | the UI waits for a worker result while the worker blocks on a full progress channel | progress uses overwrite-style / `try_send`; blocking send only for one-off results |
| D5 | poll while holding a `BufferView` | `poll` and `unmap` interleaved in readback code | scoped: `drop(view); buffer.unmap();`, enforced by `ReadbackGuard` |
| D6 | Blocking inside async | writing a large file with `std::fs` or `fsync` in a Tokio worker | disk goes through the Storage worker or `spawn_blocking`; chunk large files |
| D7 | `Guard` across await / across frames | `let g = cell.load();` followed by `await` or stored in a field | `Guard: !Send` blocks it at compile time; only store `Arc` (`load_full`) across frames |
| D8 | Old snapshots pinned | a background task holds `Arc<MapSnapshot>` and does not drop it after cancellation | tasks hold it via RAII, cancellation drops it; warn when `strong_count` exceeds a threshold |
| D9 | Double publication | both import and edit paths call `SnapshotCell::store` | unique publication point (R2); check for uncommitted transactions before installation |
| D10 | Busy-waiting / synchronous readback | hand-written `loop { if ready { break } }`, or `wait_indefinitely` on the hover path | busy-waiting is banned: the UI only `Poll`s; hover uses CPU picking, GPU picking only for clicks |

### 2.6 Priority and CPU Budget Recommendations

| Thread | Recommended priority | Implementation |
|---|---|---|
| UI | normal (no lowering) | default |
| Rayon | below-normal | custom `ThreadPoolBuilder::spawn_handler` |
| Tokio worker | normal | `runtime::Builder::worker_threads(2)` |
| Storage | below-normal | `std::thread::Builder` + platform wrapper |
| Render submission | follows the UI (normal) | no dedicated thread |

The budget is allocated over 12 logical cores: UI 1 + Rayon 8 + Tokio 2 + Storage 1 = 12. When import and validation run at the same time, the Rayon pool queues internally by `Priority` instead of spawning more threads; a fixed thread count is the key to preventing Rayon and Tokio from competing for cores (the default `num_cpus` would compute 12 and crowd out the UI and networking). These are **configuration limits, not measured throughput**.

## 3 Key Interfaces

```rust
// crates/neo-core/src/task.rs —— task description and results (no IO, no GPU dependencies)
pub enum Priority { Interactive, Normal, Background }
pub enum TaskKind { Import, Scene, Validate, Upload, Download, Autosave, Plugin }

pub struct CancelToken(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl CancelToken {
    pub fn cancel(&self);
    pub fn check(&self) -> Result<(), Cancelled>;   // cooperative checkpoint: returns Err so the task exits early
}

pub struct Progress { /* done: AtomicU64, total: AtomicU64, phase: AtomicU8 */ }
impl Progress {
    pub fn inc(&self, n: u64);
    pub fn snapshot(&self) -> ProgressSnapshot;      // (done, total, phase)
}

pub struct TaskHandle { pub id: TaskId, pub kind: TaskKind, pub priority: Priority,
                        pub token: CancelToken, pub progress: std::sync::Arc<Progress> }

pub trait TaskSink: Send + Sync {
    fn submit(&self, task: TaskDesc) -> Result<TaskHandle, TaskRejected>;  // non-blocking
    fn poll(&self, time_budget: std::time::Duration) -> Vec<TaskOutcome>;   // called every frame
}

/// Result write-back with version checking: the only "submission" path for background tasks
pub enum TaskOutcome {
    Ready { task: TaskId, base_version: u64, payload: TaskPayload },
    Stale { task: TaskId, base_version: u64, current: u64 },
    Cancelled(TaskId),
    Failed { task: TaskId, error: TaskError },
}

// crates/neo-query/src/lib.rs —— snapshot publication (lock-free read path)
pub struct SnapshotCell { /* ArcSwap<MapSnapshot> */ }
impl SnapshotCell {
    pub fn load(&self) -> arc_swap::Guard<Arc<MapSnapshot>>;   // valid for one frame
    pub fn load_full(&self) -> Arc<MapSnapshot>;               // across threads / frames
    pub fn store(&self, snapshot: MapSnapshot);                // the only publication point
}

// crates/neo-render/src/gpu.rs —— resource ownership and incremental upload
pub struct GpuCtx<'a> { pub device: &'a wgpu::Device, pub queue: &'a wgpu::Queue, pub frame_no: u64 }

pub struct BufferPool { /* free: Vec<PooledBuffer>, in_flight: u8, upload_cap: usize */ }
impl BufferPool {
    pub fn ensure(&mut self, id: BufferId, len: u64, ctx: &GpuCtx<'_>) -> &wgpu::Buffer;
    pub fn upload(&mut self, id: BufferId, range: std::ops::Range<u64>, data: &[u8], ctx: &GpuCtx<'_>);
    pub fn collect(&mut self, completed_frame: u64);   // reclaim completed slots at the end of each frame
}

pub struct PickReadback { /* 2 staging buffers rotating */ }
impl PickReadback {
    pub fn request(&mut self, ctx: &GpuCtx<'_>, rect: [u32; 4]);
    pub fn try_take(&mut self, device: &wgpu::Device) -> Option<PickResult>;   // never poll(wait)
}
```

## 4 Data Flow and Timing

### 4.1 One Scene Rebuild (across threads)

```
UI: edit commit (version=n) or camera change
      +-- cache hit (version unchanged and camera at the same level) --> reuse SceneFrame
      '-- spawn SceneTask{ version=n, view, token } --> Rayon
              visible set (viewport culling + LOD) -> geometry (incremental for dirty ways / two full-rebuild tiers)
              '-- token.check() every 2000 objects (cancellable)
      <-- TaskOutcome::Ready { base_version: n, payload: SceneFrame }
      next frame: poll (within budget) -> base_version == current ? adopt (double-buffer swap) : Stale (discard)
      adopt -> MapRenderer::prepare (upload dirty ranges) -> encode (paint phase)
```

### 4.2 Picking (GPU readback, 1 frame latency)

```
click -> EditorIntent::Select -> hit test (CPU: screen radius -> projected plane distance)
            '-- when precision is needed: PickReadback::request() (encode a 1x1 readback this frame, bytes_per_row = 256)
next frame: try_take(PollType::Poll) -> Some(id) ? update Selection : keep the CPU result
```

hover always uses CPU picking (`neo-geo::HitTester`); GPU picking is used only for clicks, avoiding per-frame readbacks.

### 4.3 Import and Cancellation

```
Ctrl+O -> ImportTask{token} -> Rayon decode -> new DataSet (owned exclusively by the worker)
             |  progress.inc(batch)
             '-- cancel: token.check() -> Err -> discard the half-built DataSet (not entered into History)
      <-- Ready{ payload: Installed(DataSet) } -> UI: SnapshotCell::store (one-shot install) -> full scene rebuild
```

Import does **not** go through per-entity `Command`s and does not enter History; it builds a complete `DataSet` on the worker and installs it in one shot, the only structural exception to the single-writer model ([ADR-0006](./adr/0006-single-writer-editor.md)).

## 5 Trade-offs and Alternatives

| Choice | Alternative | Why it was chosen | Cost |
|---|---|---|---|
| Single writer + read-only snapshots | fine-grained locking / MVCC | no lock-ordering protocol, no deadlock classes, undo/journal/OSC naturally share one order | long transactions block input; batch modifications can only be serialized as separate transactions |
| `arc-swap` | `RwLock<Arc<T>>` | no atomic RMW on the read side, no writer starvation, `load` does not block writers | a new `Arc` per store; `Guard` cannot cross threads/frames |
| Submit GPU work on the UI thread | dedicated render thread + triple buffering | picking/hover/overlay are in the same frame as the UI, no 1-frame latency | the UI has three roles in one, making the frame budget tighter |
| `crossbeam-channel` as the default | `flume` everywhere | synchronous, stable latency, mature `select!`, no async dependency | no `send_timeout`/`drain`; a few edges must switch to `flume` |
| Cooperative cancellation | preemptive (kill the thread) | safe, composable, no lock leaks | long functions must place their own checkpoints; a missing checkpoint makes cancellation ineffective |
| Fixed Rayon thread count | default `num_cpus` | does not eat UI/network cores, predictable behavior | large machines are underutilized (tunable via a feature) |

## 6 Risks

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| C1 | Snapshot deep copy (currently `Arc<DataSet>`) is unusable on the interactive path | O(n) per publication | low-frequency publication + single-threaded incremental path; COW/chunked snapshots are a Phase 6 goal |
| C2 | Missing checkpoints make import "cancellation ineffective" | the user cannot interrupt | shared `check_batch()` helper + property tests with random cancellation points |
| C3 | Misuse of `wait_indefinitely` on the readback path | one full pipeline flush per click → dropped frames | source grep assertion + `ReadbackGuard` wrapper |
| C4 | Rayon and Tokio competing for cores causes UI jitter | increased input latency | fixed thread counts; below-normal priority; reduce concurrency during import |
| C5 | Old snapshots pinned by background tasks | memory peak rises | cancellation drops the snapshot; `strong_count` warning; autosave serializes instead of holding a snapshot |
| C6 | Plugin tasks starve other tasks | validation/scene latency | per-plugin queue + time-slice cap + total budget |

## 7 Acceptance Criteria

1. 60 seconds of continuous editing + parallel scene building + parallel validation + one import: no deadlock, no panic, `version` monotonically increasing.
2. Each of D1–D10 has a corresponding test or static assertion (lock-order comment check, `Instance::new` count, `wait_indefinitely` grep).
3. The `TaskOutcome::Stale` path has a test: slow task + concurrent edit → the result is discarded and the data is unchanged (`tests/stale_task.rs`).
4. Cancelling an import takes effect within 1 frame and the half-built `DataSet` is freed (no `Arc` leak, `strong_count` returns to baseline).
5. The picking readback path contains zero occurrences of `poll(wait)` (grep + unit test); a click's picking result is returned within 1 frame.
6. Steady-state upload is ≤ 128 B per frame (uniform only), asserted via `PrepareStats.uploaded_bytes` ([pending measurement, see docs/14-performance-and-verification.md]).

## 8 References

- `arc-swap` (`load`/`load_full`/`Guard` semantics): https://docs.rs/arc-swap/1.9.5/arc_swap/
- Rust `Send`/`Sync` semantics: https://doc.rust-lang.org/nomicon/send-and-sync.html
- `crossbeam-channel` 0.5: https://docs.rs/crossbeam-channel/0.5/crossbeam_channel/
- `flume` 0.12: https://docs.rs/flume/0.12/flume/
- Tokio runtime and `spawn_blocking`: https://docs.rs/tokio/1.54/tokio/
- Rayon `ThreadPoolBuilder`: https://docs.rs/rayon/1.12/rayon/
- wgpu 30 `PollType` (`Maintain` was removed): https://docs.rs/wgpu/30.0.2/wgpu/enum.PollType.html
- wgpu 30 `Buffer::map_async` / `get_mapped_range`: https://docs.rs/wgpu/30.0.2/wgpu/struct.Buffer.html

> To verify: whether `Guard: !Send` holds in all `arc-swap` versions (semantics per the 1.9 docs; re-check on upgrade); the concrete shape of COW chunked snapshots (a Phase 6 decision); a portable platform wrapper for thread priorities (Windows `SetThreadPriority` / Linux `nice`, no measured comparison yet).
