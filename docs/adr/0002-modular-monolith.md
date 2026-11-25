# ADR-0002 · Modular Monolith (single-process modular monolith)

## Status

- Accepted: 2026-11-15.
- Reversibility: **Low**. Splitting or merging processes would affect the ownership and lifetime design of every crate.
- Scope: workspace structure, `arc-swap` snapshot sharing, GPU context ownership, plugin isolation approach.
- Related docs: [02](../02-workspace-and-dependency-rules.md), [01](../01-system-architecture.md) §2.1/§5, [06](../06-editing-core-and-undo.md) §2.7.

## Background

NEO must satisfy three mutually competing requirements at once:

- **Zero-copy sharing**: 10M objects must be shared among editing, rendering, validation, and persistence without serializing every time;
- **Global write ordering**: edits must be undoable, replayable, and crash-recoverable, so the write path must have a single order;
- **Plugin isolation**: plugins may be malicious or crash, but must not damage data or the process.

Candidate shapes: microservices (core daemon + client), multi-process (UI/render/IO processes), single-crate monolith, single process with multiple crates.

## Decision

Adopt a **single-process modular monolith**:

1. One process, one address space; 15 library crates + 2 application crates form the workspace.
   - The dependency direction is uniquely determined by the whitelist in [02](../02-workspace-and-dependency-rules.md) §2.2 (one-way, acyclic).
2. Share read-only data within the process via `Arc<Snapshot>`: `arc-swap` switches atomically; background tasks do not serialize or copy the full data set.
3. Isolation comes from **crate boundaries + WASM sandbox**, not process boundaries:
   - Plugins run in `wasmtime 50` and can only return `EditCommand` or call a restricted host API;
   - The host does not export `Snapshot::publish`, so plugins cannot bypass the validation pipeline.
4. A single owner for the GPU context (`eframe`'s `RenderState`); `neo-render` only borrows `&Device`/`&Queue`.

```
 ┌──────────── one process ────────────────────────────────────────┐
 │  neo-ui ──► neo-editor ──► neo-store ──► Arc<Snapshot>          │
 │                 │                            │                  │
 │                 └── neo-tools                ├──► neo-scene     │
 │  neo-persist ◄──┘                            └──► neo-validate  │
 │  neo-plugin (WASM sandbox) ──EditCommand──► neo-editor          │
 └─────────────────────────────────────────────────────────────────┘
```

## Alternatives

| Option | Pros | Why rejected |
|---|---|---|
| Microservices (core daemon + UI client) | Crash isolation, remote-capable | Every-frame picking/visible-set rebuild must cross IPC; snapshot transfer needs a shared-memory protocol; latency uncontrollable |
| Multi-process (UI / render / IO) | A render crash does not take editing down | Input gains at least 1 frame of latency; GPU resources across processes need D3D12 shared handles / Vulkan external memory, extremely costly on three platforms |
| Single-crate monolith | Simple configuration, no `pub` boundary cost | The compiler cannot prevent wrong dependencies like `ui → io`; module boundaries rely on discipline |
| Plugin-as-process (one process per plugin) | True isolation | Conflicts with "plugins go through Command": `EditTx` cannot be shared across processes; process pool management costs more than WASM |
| Dynamic library plugins (cdylib, C ABI) | Best performance, no sandbox overhead | Unstable ABI, crashes take the whole process down, no resource limits or permission control |

## Consequences

**Positive**

- Zero-copy snapshots + single writer ⇒ deterministic commit order; `cargo build` also provides full dependency validation.
- Cross-layer profiling (`puffin`) can sample end to end in one go; debugging needs only one process.
- Plugins and built-in tools share the same `EditCommand` path, so semantics cannot diverge.

**Negative**

- **No crash isolation**: a driver crash or `abort` takes unsaved edits with it. Mitigation: journal (op log) + periodic autosave, shrinking the loss window to seconds (see [10](../10-persistence-and-recovery.md)).
- **Pressure toward `pub` sprawl**: needs `#[non_exhaustive]`, sealed traits, and type firewalls (see [02](../02-workspace-and-dependency-rules.md) §2.4, §2.8).
- **Shared memory budget**: 10M objects + scene batches + geometry cache + undo stack share machine memory (see [16](../16-roadmap-and-risks.md) R6).
- **No per-team repo split**: all changes go into one workspace, so CI must build the whole graph every time.

## Verification

1. `pwsh ./scripts/check-deps.ps1` exits 0 on the current tree; exits 2 for the three negative cases `ui→io`, three-node cycle, and depth drift (see [02](../02-workspace-and-dependency-rules.md) §2.5).
2. `cargo tree -p neo-render` contains no `neo-store`/`neo-editor`/`neo-ui`; `cargo tree -p neo-ui` contains no `neo-io`/`neo-api`/`neo-persist`.
3. After one commit, the `Snapshot` read by the render and validation threads and the `Arc` published by the writer satisfy `Arc::ptr_eq` (no hidden copy path).
4. Plugins cannot bypass the pipeline: with the host write API disabled, any attempt other than an `EditCommand` returned by the plugin cannot change `version` (integration test).
5. No cross-process communication code: `grep -rn "TcpListener\|NamedPipe\|shared_memory" crates/` is empty (except the `neo-api` HTTP client).
6. Crash budget: recovery time and number of lost transactions after killing the process enter the baseline [to be measured, see docs/14-performance-and-verification.md].
