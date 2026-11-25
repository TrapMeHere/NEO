# 15 · Plugin system

This document defines `neo-plugin`: it hosts third-party extensions on the **WASM Component Model** (wasmtime 50 + wit-bindgen 0.63). Plugins can only "read snapshots, submit commands, receive events, and register panels", and never touch `neo-store` directly. The core decision is to **abandon the Rust dynamic-library ABI** — Rust has no stable ABI, and `abi_stable 0.11.3` can only mitigate, not eliminate, coupling to the toolchain and dependency versions; WASM uses an explicit WIT contract to turn "compatibility" into a versionable, testable engineering problem.

## 1 Goals and scope

| # | Goal | Criterion |
|---|---|---|
| G1 | Third parties can extend NEO without recompiling it | `.wasm` works across NEO minor versions |
| G2 | Malicious/defective plugins cannot corrupt data or bring down the UI | Unauthorized access errors, killed when over budget, main process does not crash |
| G3 | Capabilities are auditable and can be granted with least privilege | manifest declaration + runtime double-check |
| G4 | JOSM plugin authors have an executable migration path | capability mapping table + equivalent implementation examples |
| G5 | Faults are isolated to a single plugin | disable/uninstall granularity = a single plugin |

**Out of scope**: JVM/Java binary compatibility (not done, not promised), plugin marketplace backend, out-of-process isolation (v1 is in-process wasmtime; high-risk plugins may optionally use subprocess mode), plugin GPU passthrough.
**Dependencies**: `neo-plugin → neo-core, neo-editor`, feature-gated by `wasm-plugins` (off by default; when off, only manifest parsing and type definitions remain, and wasmtime is not pulled in). Read-only map queries need a trait seam (see §2.7, an architectural gap pointed out in this document).

## 2 Design

### 2.1 Why not a Rust dynamic library

| Option | Compatibility | Safety | Distribution | Verdict |
|---|---|---|---|---|
| `cdylib` + hand-written `extern "C"` | Every type needs an agreed `Vec`/`String` lifetime contract; crossing the boundary is a UB risk | Arbitrary in-process code, no sandbox | One per platform per architecture | ❌ |
| `abi_stable 0.11.3` | `StableAbi` gives a stable layout, but **still binds the rustc version and the dependencies' Major versions** | No sandbox | Must be built with the same compiler as the host | ❌ |
| WASM core module | Stable, cross-language | Sandbox | Single file | ⚠️ Can only pass numbers/linear memory, needs a hand-written ABI |
| **WASM Component** | WIT interfaces + semantic versioning, type-safe | Sandbox + capability model | Single file | ✅ |

`abi_stable`'s failure modes are worth recording: it guarantees "layout stability", not "dependency singleton uniqueness". When a plugin and the host each statically link `tracing`, two sets of subscribers overwrite each other; when linking `wgpu`, two instances fight over the same `Instance`. WASM puts plugins in separate linear memories and separate instances, turning such problems from "occasional crashes" into "not allowed at compile time".

### 2.2 WIT contract and versioning

There is only one contract between host and plugin: `wit/neo-plugin.wit` (`wit-bindgen 0.63` generates bindings for both sides), versioned as `package neo:plugin@1.0.0`:

| Change | Version slot | Host behavior |
|---|---|---|
| Adding optional exports/event topics/capabilities | minor | Compatible, keeps loading |
| Adding required imports, changing field types or semantics, deleting functions | major | Refuses to load old plugins and prompts "update required" |
| Comments/docs only | patch | No effect |

```wit
package neo:plugin@1.0.0;

interface types {
  enum object-kind { node, way, relation }
  record object-ref { kind: object-kind, id: u64 }
  record bbox { min-lon: s32, min-lat: s32, max-lon: s32, max-lat: s32 }  // 1e-7 degrees
  record tag { key: string, value: string }
  record object-view { ref: object-ref, version: u32, deleted: bool, tags: list<tag>,
    lat: option<s32>, lon: option<s32>, members: list<object-ref>, member-roles: list<string> }
  enum log-level { trace, debug, info, warn, error }
  variant host-error { permission-denied(string), not-found(object-ref),
    invalid-argument(string), internal(string), quota-exceeded(string) }
}

interface host-api {
  use types.{object-ref, object-view, bbox, log-level, host-error};
  snapshot-version: func() -> u64;                                   // read-only query surface
  lookup: func(refs: list<object-ref>) -> list<option<object-view>>;
  query-bbox: func(area: bbox, tag-filter: option<string>, limit: u32) -> list<object-ref>;
  parents-of: func(r: object-ref) -> list<object-ref>;
  submit-command: func(name: string, args-json: string) -> result<u64, host-error>;  // the only write path
  subscribe: func(topics: list<string>) -> result<u64, host-error>;
  register-panel: func(id: string, title: string, preferred-width: u32) -> result<u32, host-error>;
  set-panel-status: func(panel: u32, text: string) -> result<_, host-error>;
  http-request: func(url: string, method: string, body: option<list<u8>>) -> result<list<u8>, host-error>;
  read-file: func(relative-path: string) -> result<list<u8>, host-error>;
  log: func(level: log-level, message: string);
}

world neo-plugin {
  import host-api;
  export on-load: func(manifest-json: string) -> result<_, string>;
  export on-event: func(topic: string, payload-json: string) -> result<_, string>;
  export render-panel: func(panel: u32, width: u32, height: u32) -> result<list<u8>, string>;
  export on-unload: func();
}
```

`render-panel` returns a **declarative draw list** (text/buttons/tables/sliders), which the host renders with egui. Plugins get no GPU handle and no `egui::Context`, so they cannot inject arbitrary drawing or block a frame.

### 2.3 Capability model and permission list

| Capability | Allowed WIT calls | Default | Notes |
|---|---|---|---|
| `read` | `snapshot-version`/`lookup`/`query-bbox`/`parents-of` | ✅ implicit | Read-only, low privacy risk |
| `command` | `submit-command` | ❌ | Commands still go through `neo-editor` validation |
| `events` | `subscribe` + `on-event` | ❌ | Topic allowlist |
| `ui` | `register-panel`/`set-panel-status`/`render-panel` | ❌ | Max 8 panels |
| `network` | `http-request` | ❌ | Domain allowlist + HTTPS only + custom headers forbidden (to prevent token leakage) |
| `filesystem` | `read-file` | ❌ | Plugin's own directory only, read-only, `..` forbidden |
| `imagery` | Add imagery layers indirectly via `command` | ❌ | Listed separately for auditability |

**Double-check**: the manifest's `[capabilities]` determines "the permission list shown to the user at install time"; the runtime `CapabilitySet` decides "what is actually allowed". The intersection of the two is used — even if `network` is declared, if the user has not granted it, the call still returns `permission-denied`. This prevents "installing means full power".

### 2.4 Sandbox resource limits

| Dimension | Mechanism (wasmtime 50) | Default | Failure behavior |
|---|---|---|---|
| CPU | `Config::consume_fuel(true)` + `Store::set_fuel` | 1e8 fuel/event | `Trap::OutOfFuel` ⇒ record `quota-exceeded`, plugin disabled rather than retried repeatedly |
| Wall clock | `Config::epoch_interruption(true)` + `Store::set_epoch_deadline(1)` + thread `Engine::increment_epoch()` | epoch 1ms, deadline 100ms | Timeout trap |
| Memory | `ResourceLimiter` / `StoreLimitsBuilder` | 64 MiB linear memory, 10k table | Allocation failure ⇒ trap, plugin disabled |
| Files/network | Does not link WASI p2 `filesystem`/`sockets` | — | Can only go through controlled host functions |
| Re-entrancy | One instance per plugin, serialized events | — | Plugins cannot recursively call host commands |
| Compilation | `Engine::precompile_component` + `Config::cache` | Cached to `<cache>/wasm/` | Cold-start cost [to be measured, see docs/14-performance-and-verification.md] |

`fuel` and `epoch` must **both** be on: fuel prevents infinite loops (deterministic, billable), while epoch prevents "legal but long" calls (e.g. 100k `lookup`s).

### 2.5 Packaging, signing, and distribution

```text
cool-plugin-1.2.0.neopkg  (= zip)
├── manifest.toml          # permission list, shown at install time
├── component.wasm         # optional: precompiled component
├── assets/icon.png
└── signature.ed25519      # detached signature over the SHA-256 of manifest + component
```

```toml
schema = 1
id = "org.example.coolplugin"
name = "Cool Plugin"
version = "1.2.0"
api = "1.0"                        # host WIT version, semver-compatible range 1.x
min_neo_version = "0.1.0"
entry = "component.wasm"

[capabilities]
command = true
ui = true
events = ["edit.committed", "snapshot.published"]
network = ["https://api.example.org"]
filesystem = false
imagery = false
```

Validation order: zip integrity (listing + no path traversal) → signature → manifest deserialization and field validation → `api` compatibility → `Component::from_file` → capability negotiation → `on-load`. Unsigned plugins can only be loaded in "developer mode", and the UI must continuously show a risk marker; official distribution requires a signature (signature-verification library selection in §2.7).

### 2.6 Host lifecycle and events

```text
PluginHost::load_all()
 ├─ scan <config>/plugins/*.neopkg → verify + parse manifest
 │    (failure ⇒ LoadReport::Rejected, other plugins unaffected)
 ├─ Engine::new(Config{consume_fuel, epoch_interruption, cache})
 ├─ Component::from_file → Linker::instantiate (inject HostApiImpl's StoreData)
 └─ on-load(manifest_json) → record topic subscriptions
Event loop (UI frame boundary, not within a frame)
 ├─ collect this frame's events, dedupe/merge by topic (only the latest state per topic is dispatched, preventing event storms)
 ├─ set_fuel + set_epoch_deadline → on-event(topic, json)
 │    └─ plugins can only submit-command (queued; the real commit happens in the next edit pipeline)
 └─ panels: render-panel → declarative list → egui drawing
```

| topic | Payload points | Typical use |
|---|---|---|
| `project.*` / `layer.*` (opened, closed, added, removed, visibility) | project/layer id, kind | Initialization, layer tools |
| `selection.changed` | refs[] (>1000 gives only the count) | Context panels |
| `edit.committed` / `edit.undone` / `edit.redone` | txn id, version, changed refs[] | Automatic validation, undo awareness |
| `snapshot.published` | snapshot version | Incremental recomputation |
| `viewport.changed` | bbox, zoom | Lazy loading |
| `upload.started` / `upload.finished` / `upload.failed` | changeset id, counts, conflict summary | After-upload actions |
| `tool.activated` / `tool.deactivated` | tool id | Tool collaboration |

Events are **read-only notifications**: calling `submit-command` inside `on-event` is allowed, but it only enqueues; `EditCommand` validation, journal, and undo all still apply — plugins enjoy no privilege that bypasses the editor.

### 2.7 Version compatibility and architectural gap

The host-side implementation of `HostApi` needs three things: **the current read-only snapshot**, **a command submission entry point**, and **a panel registry**. The capabilities for the first two live in `neo-store` and `neo-editor`, while `neo-plugin` may only depend on `neo-core, neo-editor`. Possible solutions:

| Option | Description | Trade-off |
|---|---|---|
| A. Move the `MapQuery` trait down into `neo-core` | implemented by `neo-store`, `neo-plugin` depends only on the trait | ✅ Lowest cost, but query semantics sink into a leaf crate and need the 02 allowlist and concept-ownership review |
| B. Add a new `neo-query` leaf crate (trait + DTO) | Cleaner concepts | ✅ One more crate and one more dependency edge |
| C. `neo-editor` re-exports `neo-store` types | Least effort | ❌ In effect makes `neo-plugin` transitively depend on `neo-store`, breaking dependency auditability |

**This document recommends A**, and records "`neo-plugin` lacks a read-only query seam" as an architectural gap: currently no crate in the workspace that `neo-plugin` could depend on owns spatial query and cross-reference capabilities; that seam **does not yet exist** and must be filled before implementation, otherwise `query-bbox` cannot be implemented and plugins can only see the selection set already exposed by `neo-editor`. The signature-verification library is likewise not in the dependency list and needs a separate selection (Ed25519).

### 2.8 JOSM plugin migration: equivalent capability mapping

JOSM plugins are Java bytecode that directly access the internal APIs under `org.openstreetmap.josm.*` (`MapView`/`DataSet` are referenced directly in many places), so they **cannot be ported directly**. The feasible path is "capability restatement":

| JOSM capability (typical plugin) | JOSM form | NEO equivalent | Cost |
|---|---|---|---|
| Input modes (draw/drag) | Subclass `MapMode` | `neo-tools`'s `Tool` + `command` to submit edits | Medium: interaction must be rewritten |
| Data layers/custom drawing | `Layer` + `PaintVisitor` | ❌ Custom rendering not allowed; provide a read-only "data source layer" drawn by the host | High: capability deliberately trimmed |
| Validation rules | `Validator` test sets | `neo-validate` declarative rules (plugin describes, host executes) | Low–Medium |
| Upload hooks | `UploadHook` | `upload.*` events + `command` | Low |
| Imagery sources / remote APIs | `ImageryInfo` providers / `HttpURLConnection` | `imagery` capability + config entries / `network` (domain allowlist) | Low |
| Preference panels | `PreferenceSetting` | `ui` + declarative panels | Medium |
| Scripting (JSR-223/GraalJS) | Run JS at runtime | v1 does not embed a JS engine; narrowed to small WASM plugins | High |

**On script plugins**: one class of plugins in the JOSM ecosystem is essentially "small JS/Groovy logic". NEO v1 does not embed a JS engine (not in the dependency list; adding QuickJS/GraalJS would bring new sandbox surface and size). There are two alternative paths, both requiring an explicit project: (1) declarative rules — fold "condition → action" into the `neo-validate` rule DSL, covering most tag-check scripts; (2) WIT-generated templates — a `cargo generate` template turns 20 lines of JS into 60 lines of Rust/WASM, with controllable cost and type safety. To support a GraalJS compatibility layer, the budget of running QuickJS-WASM on wasmtime would need evaluation; no promise is made at present.

## 3 Key interfaces

```rust
// neo-plugin/src/lib.rs
pub struct PluginHostConfig { pub plugin_dir: PathBuf, pub cache_dir: PathBuf,
    pub fuel_per_event: u64, pub epoch_deadline_ms: u64, pub memory_limit_bytes: usize,
    pub max_panels: usize, pub developer_mode: bool }

pub struct PluginHost { /* Engine + per-plugin Store/Instance + permission table */ }
impl PluginHost {
    pub fn new(cfg: PluginHostConfig) -> Result<Self, PluginError>;
    pub fn load_all(&mut self) -> Result<Vec<LoadReport>, PluginError>;
    pub fn load(&mut self, pkg: &Path, granted: CapabilitySet) -> Result<PluginId, PluginError>;
    pub fn dispatch(&mut self, events: &[HostEvent]) -> DispatchReport;
    pub fn drain_commands(&mut self) -> Vec<PendingCommand>;   // hand off to the edit pipeline
    pub fn panels(&self) -> Vec<PanelDescriptor>;
    pub fn shutdown(self);
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PluginManifest { pub schema: u32, pub id: String, pub name: String, pub version: String,
    pub api: String, pub entry: String, pub min_neo_version: Option<String>,
    pub capabilities: ManifestCaps, pub limits: ManifestLimits }
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ManifestCaps { pub command: bool, pub ui: bool, pub events: Vec<String>,
    pub network: Vec<String>, pub filesystem: bool, pub imagery: bool }

#[derive(Debug, Clone, Default)]
pub struct CapabilitySet { /* bitflags style; manifest ∩ user grants */ }
impl CapabilitySet {
    pub fn grants(&self, need: Capability) -> bool;
    pub fn intersect(self, declared: &ManifestCaps) -> Self;
}

/// The host's entire visible surface: read-only query + command submission + UI registration.
/// The implementation lives at the app layer (apps/neo-desktop), so as not to break neo-plugin's dependency set.
pub trait HostApi: Send {
    fn snapshot_version(&self) -> u64;
    fn lookup(&self, refs: &[ObjectRef]) -> Vec<Option<ObjectView>>;
    fn query_bbox(&self, area: Bbox, filter: Option<&str>, limit: u32) -> Vec<ObjectRef>;
    fn parents_of(&self, r: ObjectRef) -> Vec<ObjectRef>;
    fn submit_command(&mut self, name: &str, args_json: &str) -> Result<u64, HostError>;
    fn http_request(&mut self, url: &str, method: &str, body: Option<&[u8]>) -> Result<Vec<u8>, HostError>;
    fn read_file(&mut self, rel: &str) -> Result<Vec<u8>, HostError>;
    fn log(&mut self, level: LogLevel, msg: &str);
}

pub enum LoadReport {
    Loaded { id: PluginId, name: String, caps: CapabilitySet },
    Rejected { path: PathBuf, reason: RejectReason },
    Disabled { id: PluginId, reason: DisableReason },   // after the budget is exceeded
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("manifest: {0}")] Manifest(String),
    #[error("incompatible api: plugin={plugin}, host={host}")] ApiIncompatible { plugin: String, host: String },
    #[error("signature invalid")] SignatureInvalid,
    #[error("capability denied: {0}")] CapabilityDenied(String),
    #[error("plugin trapped: {0}")] Trapped(String),
    #[error("quota exceeded: {0}")] QuotaExceeded(String),
}
```

## 4 Data flow and sequence

```text
UI      PluginHost         Manifest/signature  wasmtime Engine     HostApi(app layer)
 │ install │                  │                 │                    │
 │────────▶│ open .neopkg → unzip + verify ────▶│                    │
 │         │◀─ ok / SignatureInvalid             │                    │
 │         │── api compatible (1.x) → Component::from_file ─────────▶│
 │         │── Linker::instantiate ─────────────▶│ separate Store/memory
 │         │── on-load(manifest_json) ──────────▶│ fuel/epoch set     │
 │◀ perm list│ (manifest ∩ user grants)           │                    │
 │ selection change │── dispatch([selection.changed]) ───▶│ on-event  │
 │         │                                    │─ lookup → snapshot ▶│
 │         │                                    │─ submit-command ──▶│ queued
 │         │◀── PendingCommand (submitted next frame) ──│              │
 │         │── drain_commands → neo-editor pipeline ────────────────▶│
 │         │── render-panel → declarative list → egui drawing        │
```

Budget exceeded: an infinite loop in `on-event` ⇒ fuel exhausted ⇒ `Trap::OutOfFuel` ⇒ record `quota-exceeded`; after 3 consecutive occurrences the plugin is automatically `Disabled` (can be manually reset); other plugins and their own instances in that frame are unaffected.

## 5 Trade-offs and alternatives

| Decision | Choice | Alternative | Why rejected |
|---|---|---|---|
| Plugin format | WASM Component | `cdylib`/`abi_stable` | No stable ABI, dependency singleton conflicts, no sandbox |
| Isolation level | In-process wasmtime | Subprocess + IPC | Every query needs a serialization round trip, and interactive panel latency is unacceptable; kept as an optional mode for high-risk plugins |
| UI extension | Declarative draw list | Expose `egui::Ui` | Arbitrary drawing can break layout/block frames; declarative is restrictable and auditable |
| Resource limits | fuel + epoch + memory cap | Timeout only / fuel only | Timeout alone is not billable; fuel alone cannot stop "legal long calls" |
| Network | Domain allowlist + custom headers forbidden | Allow everything | Prevents plugins from stealing OAuth tokens (tokens only exist in the host HTTP layer) |
| Signing | Detached Ed25519 | No signature | Plugins can submit edits, so the supply-chain attack surface is too large |

## 6 Risks

| Risk | Impact | Mitigation |
|---|---|---|
| WIT contract evolution breaks old plugins | Plugins fail en masse | semver rules + dual-version `Linker` in parallel; CI keeps v1 golden plugins |
| fuel budget too tight | Normal plugins are wrongly killed | Per-topic tiers + user-adjustable + `Disabled` is recoverable |
| Event storms | Frame-time jitter | Frame-boundary dispatch + same-topic coalescing + once per plugin per frame |
| High-frequency command submission | Edit history pollution | Command rate limit (default 20/s) + everything undoable |
| wasm cold start | Slow first open | `precompile_component` + compilation cache + lazy instantiation |
| Signing key stolen / plugin reads sensitive data | No revocation / privacy leak | manifest `revoked_after` + local revocation list; `read` only exposes snapshot content, notes/GPX layers need additional capabilities |

## 7 Acceptance criteria

- [ ] Load the WIT `1.0` sample plugin: read a snapshot, query by bbox, submit one `AddNode`, render one panel.
- [ ] A plugin without declared `command` calling `submit-command` returns `permission-denied` and produces no edits.
- [ ] An infinite-loop plugin is trapped within 100ms, the host frame loop does not block, and other plugins keep working.
- [ ] Requesting more than 64MiB of memory is throttled and the host does not OOM; `read-file("../x")` returns `permission-denied`.
- [ ] Unsigned plugins are rejected with `developer_mode=false`; with `true` they load, but the UI shows a risk marker.
- [ ] A plugin with `api = "2.0"` is rejected with a readable error.
- [ ] With `wasm-plugins` off, `neo-plugin` does not pull in wasmtime (`cargo tree` assertion); `cargo test -p neo-plugin` and `clippy -D warnings` pass.

## 8 References

- wasmtime 50 component model (`Component::from_file`, `Linker::instantiate`, `bindgen!`): <https://docs.rs/wasmtime/50.0.1/wasmtime/component/>
- wasmtime resource limits (`consume_fuel`, `epoch_interruption`, `increment_epoch`, `set_fuel`, `set_epoch_deadline`, `ResourceLimiter`): <https://docs.rs/wasmtime/50.0.1/wasmtime/struct.Config.html>
- Precompilation and compilation cache (`Engine::precompile_component`, `Config::cache`): <https://docs.rs/wasmtime/50.0.1/wasmtime/struct.Engine.html#method.precompile_component>
- WIT language and package versioning: <https://component-model.bytecodealliance.org/design/wit.html>
- `wit-bindgen 0.63`: <https://docs.rs/wit-bindgen/0.63.0/wit_bindgen/>
- JOSM plugin manifest fields (`Plugin-Mainversion`, `Plugin-Class`, `Plugin-Version`, `Plugin-Description`, `Plugin-Stage`, etc.): <https://wiki.openstreetmap.org/wiki/JOSM/Plugins>
- Related: [Workspace and dependency rules](./02-workspace-and-dependency-rules.md) · [Editing core and undo](./06-editing-core-and-undo.md) · [System architecture](./01-system-architecture.md) · [ADR-0007](./adr/0007-wasm-plugins.md)
