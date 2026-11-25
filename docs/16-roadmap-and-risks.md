# 16 · Roadmap and risks (Roadmap & Risks)

NEO advances in six phases: **technical foundation → OSM data core → basic editing → professional editing → full ecosystem → extreme performance**. Each phase closes with a quantifiable Gate; if a Gate does not pass, the next phase is not entered, rather than "pile up features first and add performance later". This document gives each phase's deliverables/acceptance/dependencies/risks, the Gate decision clauses, the risk register (R1–R8), and an "anti-hallucination" list of "what does not count as done"; performance numbers are always left to measurement ([to be measured, see docs/14-performance-and-verification.md]). For alignment scope see [JOSM parity](./13-josm-parity.md), and for dependency and version prerequisites see [02](./02-workspace-and-dependency-rules.md).

## Summary

The organizing principle of the roadmap is "**every phase can be demonstrated and can fail independently**": better to have phase 1 only do pan/zoom and picking of 1M objects than to first build a pile of features without a performance baseline. A Gate is a decision point, not a ceremony: if the prototype metrics miss the target, we do not enter phase 2, because all designs for the data core, editing, and rendering build on that baseline. This document does not preset any performance number; it only defines "what to measure, how passing is computed, and what to do when it fails".

## 1 Goals and scope

**In scope**: the scope and deliverables of the six phases; the dependencies between phases and the Gate decision clauses; the coverage cadence for the three platforms and GPU backends; the risk register and risk×phase mapping; the decision rules for "what does not count as done".

**Out of scope**: concrete implementation designs (see 04–12); performance methodology and measurement scripts (see 14); licensing and governance (see [17](./17-licensing-and-governance.md)); the item-by-item JOSM feature comparison table (see [13](./13-josm-parity.md)).

**Three facts that must be corrected first** (otherwise the roadmap would copy wrong premises):

1. OSM API **only has 0.6**. 0.7 is just a wiki concept page with no deployment spec; therefore the sync layer in phases 2/4 only needs to implement 0.6 (changeset `PUT` creation, single-request `application/xml` diff upload, 409/412 semantics).
2. The actual version of `osmChange` is `version="0.6"`; **OSC 0.3 does not exist**; the write side uses a custom encoder, and the read side also only recognizes 0.6.
3. **PBF is a JOSM core capability, not a plugin**; JOSM's Edit menu is very small (geometry operations are in Tools, the Data menu has zero core entries, download/upload is in File). This determines NEO's "core vs plugin" boundary: PBF, MapCSS, Presets, the validator, and sessions/autosave all belong to the core.

## 2 Design

### 2.1 Phases and Gate overview

```
 Phase 1 Technical foundation ──► [G1] ──► Phase 2 OSM data core ──► [G2] ──► Phase 3 Basic editing ──► [G3]
                        │
                        └─ prototype metrics miss target ──► do not enter phase 2 (fall back to a spike / change plan, see §2.8)
 Phase 3 ──► [G3] ──► Phase 4 Professional editing ──► [G4] ──► Phase 5 Full ecosystem ──► [G5] ──► Phase 6 Extreme performance
                                                                     │
                                                                     └─ JOSM workflow equivalence not reached = must not claim "replaceable"
```

| Phase | Theme | Main deliverable crates | Exit condition |
|---|---|---|---|
| 1 | Technical foundation | `neo-core`, `neo-osm`, `neo-store`, `neo-geo`, `neo-scene`, `neo-render`, `apps/neo-prototype` | G1: prototype metrics meet the target |
| 2 | OSM data core | `neo-io`, `neo-store` (indexes/reference sets), `neo-query`, `neo-geo` | G2: 10M import correctness + OSC round-trip |
| 3 | Basic editing | `neo-editor`, `neo-tools`, `neo-ui`, `apps/neo-desktop` | G3: self-usable editing, undo/journal self-consistent |
| 4 | Professional editing | `neo-api`, `neo-persist`, `neo-validate`, `neo-style` | G4: download→edit→validate→upload→conflict loop closed |
| 5 | Full ecosystem | `neo-plugin`, `neo-style` (full Presets/MapCSS set), `neo-ui` (dialog family) | G5: plugin isolation + three-platform installers + Presets coverage |
| 6 | Extreme performance | `neo-scene` (streaming LOD), `neo-render` (GPU culling), `neo-store` (compression) | Release candidate (RC) |

Timelines only record relative order, never week counts: **any "done in X weeks" promise without baseline data is an illusion**. Within each phase, deliver by "vertical slice" (one demonstrable end-to-end path), not by cutting across crates.

### 2.2 Phase 1 · Technical foundation

| Dimension | Content |
|---|---|
| Deliverables | Workspace skeleton (16 crates + 2 apps); `scripts/check-deps.py` and negative-case self-tests; `apps/neo-prototype` connecting "1M object PBF import → render → pan/zoom → pick"; minimal three-platform CI; ADR-0001/0002/0004/0005/0006/0008/0009 landed; version-resolution conclusions fed back into [02](./02-workspace-and-dependency-rules.md) §2.6 |
| Quantifiable acceptance | (a) `check-deps.py` exits 0, and the 3 negative cases exit 2; (b) 1M object import succeeds and object counts match an independent tool; (c) pan/zoom/pick run continuously for 10 minutes on the reference machine without crashing; (d) the four baselines (import time, time to first frame, pan frame time p50/p99, peak RSS) are measured and recorded; (e) all three platforms build successfully |
| Dependencies | None (this phase establishes everything); the input is the architectural decisions in [01](./01-system-architecture.md) and [02](./02-workspace-and-dependency-rules.md) |
| Main risks | R1 (version drift: `egui_dock`/`egui-phosphor` locked to 0.36, `geo`/`rstar` interop), R5 (GPU backend differences), R6 (initial memory measurement) |
| Exit | **[G1]** passing is required before entering phase 2 |

### 2.3 Phase 2 · OSM data core

| Dimension | Content |
|---|---|
| Deliverables | Full SoA `DataSet` columns (Node/Way/Relation/Tags/Meta); `MapSnapshot`/`SnapshotCell` and indexes (id→row, node→ways, way→relations, bbox); PBF/XML/osmChange 0.6 read/write (osmpbf read-only + custom encoder); bulk import path; incremental spatial index (rstar 0.14); 10M object import path |
| Quantifiable acceptance | (a) 10M object PBF import succeeds, and object counts/coordinates/tag samples match an independent tool item by item; (b) OSC round-trip equivalence (write→read back→write again, semantically identical); (c) referential integrity with 0 errors (each way's nodes exist, each relation's members exist, or are explicitly recorded as incomplete); (d) import is cancellable and leaves no half-finished snapshot; (e) import time and peak memory enter the baseline |
| Dependencies | The `neo-core`/`neo-osm`/`neo-store`/`neo-geo` skeletons and the G1 baseline from phase 1 |
| Main risks | R2 (negative id, visibility, version semantic deviations), R6 (10M memory peak), R1 (osmpbf 0.3.8 API changes) |
| Exit | **[G2]** |

### 2.4 Phase 3 · Basic editing

| Dimension | Content |
|---|---|
| Deliverables | `Editor` (commit pipeline), `History`, `EditTx`, `Selection`, `Journal` (op log + replay); tool set: select / draw / move / delete / split / merge / tag editing; `neo-ui` base panels and the `apps/neo-desktop` main window; save as OSC/PBF |
| Quantifiable acceptance | (a) After 1000 random edits + a random undo/redo sequence, the snapshot matches the "step-by-step replay" result; (b) property tests: a transaction that fails validation has zero side effects ([06](./06-editing-core-and-undo.md) §7); (c) after killing the process 100 times, replay leaves no half transaction; (d) one drag produces only 1 history entry (coalescing works); (e) frame time and input latency during editing reach the baseline |
| Dependencies | The snapshot/index/geometry-invalidation interfaces from phase 2 |
| Main risks | R2 (JOSM interaction semantics: selection rules, shortcuts, tag-row behavior), R6 (History memory) |
| Exit | **[G3]** (lightweight: functionally self-usable for editing; does not block subsequent parallel development) |

### 2.5 Phase 4 · Professional editing

| Dimension | Content |
|---|---|
| Deliverables | `neo-api` (API 0.6 client, changeset, diff upload, 409/412, notes); `neo-persist` (SQLite WAL, journal file, autosave, crash recovery); `neo-validate` (built-in rules + MapCSS validator); `neo-style` (MapCSS rendering styles, Presets v1); conflict resolution UI; download/upload round-trip |
| Quantifiable acceptance | (a) The full workflow runs end to end: download area → edit → validate → upload → trigger 409 → resolve conflict → re-upload succeeds; (b) all six `MergeCase` types have integration tests ([06](./06-editing-core-and-undo.md) §2.6); (c) after power loss/process kill, `RecoveryReport` is visible and `replayed_txns > 0`; (d) validation-rule recall on a known error set aligns with JOSM (compared rule by rule, with differences registered); (e) autosave period and retained generations match configuration |
| Dependencies | The `Editor`/`History`/`Journal` interfaces from phase 3; the contracts in [09](./09-osm-api-and-sync.md) / [10](./10-persistence-and-recovery.md) / [12](./12-validation.md) |
| Main risks | R2 (conflict semantics differing from JOSM), R3 (MapCSS/Presets compatibility), R4 (JOSM presets asset licensing), R1 (admission of out-of-list dependencies such as reqwest) |
| Exit | **[G4]** |

### 2.6 Phase 5 · Full ecosystem

| Dimension | Content |
|---|---|
| Deliverables | `neo-plugin` (wasmtime 50 host, plugin ABI v1, permission model, resource limits); the full Presets/MapCSS set and style editor; imagery layers (WMS/TMS) and the offset database; more import/export formats; three-platform installers and auto-update; CLI |
| Quantifiable acceptance | (a) At least 3 real JOSM plugin semantics are ported to WASM plugins and usable; (b) plugins cannot exceed permissions (cannot write files/network/publish snapshots unless explicitly authorized); (c) plugin timeout/crash does not affect the main process (sandbox isolation test); (d) three-platform installers cold-start and open a 1M dataset; (e) the Presets coverage item-by-item comparison against JOSM default presets is complete (missing items recorded) |
| Dependencies | API/persistence/validation from phase 4; the plugin ABI depends on the `EditCommand` shape ([ADR-0008](./adr/0008-command-transaction-undo.md)) |
| Main risks | R7 (plugin security), R4 (GPL: must not copy JOSM code/tainted assets), R3 (Presets compatibility), R8 (staffing and long-term maintenance) |
| Exit | **[G5]** |

### 2.7 Phase 6 · Extreme performance

| Dimension | Content |
|---|---|
| Deliverables | 10M+ object interaction (streaming scene, LOD tiers, GPU culling/indirect draw); memory compression (string interning, coordinate quantization, index compression); multithreaded scene building; performance profiling integration (puffin 0.20); long-session stability |
| Quantifiable acceptance | (a) Pan/zoom/pick at 10M objects reach the baseline (frame time, p99 input latency); (b) peak memory below budget and no monotonic growth over a 1-hour session; (c) cold-start time meets the target; (d) visuals match the reference implementation (full rebuild) (golden images); (e) three-platform performance differences are recorded and explained |
| Dependencies | The full feature surface from phase 5 (performance cannot be bought by cutting features) |
| Main risks | R5 (backend differences: DX12/Vulkan/Metal), R6 (memory peak), R8 (long-term maintenance) |
| Exit | Release candidate (Release Candidate) |

### 2.8 Milestone Gates

**G1 (phase 1 → 2) is the only "hard veto" Gate**: if the prototype metrics miss the target, we do not enter phase 2, because without a baseline it is impossible to judge whether later designs regress.

| Gate | Criteria | Decision method | What to do if it fails |
|---|---|---|---|
| **G1** | ① 1M object import time; ② time to first frame (import complete→visible); ③ pan/zoom frame time (p50/p99); ④ peak RSS; ⑤ single GPU pick latency; ⑥ 10-minute continuous stability; ⑦ three-platform buildability | Run the fixed script of `apps/neo-prototype` on the unified reference machine (i5-12400F / 31.8 GB / RTX 3070 8 GB / MSVC 14.43), take the median of 5 runs, record commit and driver version | One 2-week spike fix is allowed before re-evaluation; if it still fails, change the plan per the fallback path in §5 (e.g. downgrade the GUI stack, switch picking to CPU), and feed the conclusion back into [02](./02-workspace-and-dependency-rules.md) §2.6 and R1 here |
| G2 | 10M import correctness (counts/sampling/reference set), OSC round-trip, cancellation semantics | Comparison with an independent tool + property tests | Re-evaluate after fixing; correctness problems must not be carried into the editing phase |
| G3 | Self-usable editing: undo/redo consistency, crash replay, one history entry per drag | Property tests + process-kill script | Allowed to enter phase 4 with gaps (parallel development), but the gaps must be registered |
| G4 | JOSM workflow equivalence (download→edit→validate→upload→conflict), crash recovery, six conflict types | End-to-end script + manual checklist (against the parity definition in [13](./13-josm-parity.md)) | Fill in the missing flows; must not claim "replaceable" externally until reached |
| G5 | Plugin isolation, three-platform installers, Presets coverage | Sandbox attack tests + installer smoke test | Narrow plugin permissions or delay the release |

> All quantitative thresholds (except "the requirement to measure it at all") are filled in from measured baselines at Gate review and written into `docs/prototypes/gate-*.md`; **this document does not preset numbers**. Verdicts always use medians, not best results, and must record hardware and driver versions.

### 2.9 Three-platform and GPU backend coverage

| Phase | Windows (DX12) | Linux (Vulkan) | macOS (Metal) | Notes |
|---|---|---|---|---|
| 1 | Must test | Must test (build + smoke) | Must test (build + smoke) | G1 requires three-platform buildability; Windows is the functional baseline |
| 2 | Must test | Must test | Must test | Import correctness is identical on all three platforms |
| 3 | Must test | Must test | Must test | Input/IME differences surface at this phase |
| 4 | Must test | Must test | Must test | Packaging C dependencies for SQLite/wasmtime is solved at this phase |
| 5 | Must test (release) | Must test (release) | Must test (release) | Installers and signing |
| 6 | Baseline machine | Baseline machine | Reference machine (Metal, non-NVIDIA) | Cross-backend performance differences registered as R5 |

WARP / llvmpipe software adapters only provide a "can start, can prompt" fallback (`pick-readback` disabled path, see [02](./02-workspace-and-dependency-rules.md) §2.7), with no performance promise.

## 3 Key interfaces

Gate decisions do not enter the runtime, but need an executable, archivable structure (an xtask-style tool in `tools/gate/`, not belonging to any runtime crate):

```rust
// tools/gate/src/lib.rs
pub enum Phase { Foundation = 1, DataCore, BasicEditing, ProEditing, Ecosystem, Performance }

pub struct Criterion {
    pub id: &'static str,          // "g1.import_1m_secs" / "g1.pan_frame_p99_ms"
    pub metric: Metric,
    pub unit: Unit,
    pub budget: Option<f64>,       // None = measure only this time, no verdict (first baseline)
    pub comparison: Comparison,    // Less | Greater | Range
}

pub enum Metric {
    ImportSeconds, FirstFrameMillis, FrameTimeP50, FrameTimeP99,
    PeakRssBytes, PickLatencyMillis, CrashFreeMinutes, CorrectnessChecks,
}

pub struct Measurement { pub id: &'static str, pub value: f64, pub at: SystemTime,
                         pub commit: String, pub adapter: String }

pub struct GateReport {
    pub phase: Phase,
    pub measurements: Vec<Measurement>,
    pub verdicts: Vec<Verdict>,
}

pub enum Verdict { Pass(Criterion), Fail { criterion: Criterion, measured: f64 } }

pub trait Gate { fn evaluate(&self, report: &GateReport) -> GateOutcome; }
pub enum GateOutcome { Go, NoGo { blocking: Vec<&'static str> }, Inconclusive }
```

Verdict results are written into `docs/prototypes/gate-<phase>.md` (Markdown tables + raw JSON) and committed together, **not written in PR comments**.

## 4 Data flow

```
Phase start
   │
   ├─ define Criterion (id / metric / budget: None means measure only the first time)
   ▼
Run the fixed script on the reference machine (release + fixed driver + fixed dataset)
   │
   ├─ produce Measurement[] (including commit and adapter name)
   ▼
GateReport ──► Gate::evaluate ──► Go / NoGo / Inconclusive
   │                                │
   │                                └─ NoGo: one spike allowed; if still NoGo, change plan per §5
   ▼
Write docs/prototypes/gate-<phase>.md and feed back the "to verify" items in this document and related topics
   │
   ▼
Enter the next phase (or delay that phase and fill the gaps first)
```

## 5 Trade-offs and alternatives

| Choice | Alternative | Why chosen | Cost |
|---|---|---|---|
| Phase 1 builds the prototype foundation first | Build features (editing/upload) first and add performance later | If the GPU-first architectural assumption does not hold, later changes are bone-deep | No "usable editor" to demonstrate in the first 1–2 months |
| Six-phase vertical evolution | Horizontal parallel development by crate | Every phase has an end-to-end verifiable path, and Gates can really veto | Some crates only become truly used in phase 3 |
| G1 hard veto | All Gates soft (record and pass) | The only decision point where "wrong means starting over completely" is worth being hard | One 2-week spike time cost |
| No preset performance numbers | Set target numbers before implementation | Fabricated numbers mislead every subsequent design review | Early docs look "aimless"; Gates compensate |
| Windows as the functional baseline | Push all three platforms simultaneously | The reference machine and main user group are on Windows; macOS/Linux first only guarantee buildability | Linux/macOS experience problems surface later (fixed together at phase 3) |
| No browser version | Add a wasm build along the way | 10M objects exceed the wasm32 address space; the ROI does not hold | Lose the "try it in a web page" distribution ability |

## 6 Risks

### 6.1 Register

| # | Risk | Trigger signal | Impact | Mitigation | Responsible phase |
|---|---|---|---|---|---|
| R1 | Version drift (GUI stack / geo-rstar / out-of-list dependencies) | `cargo build` reports two copies of egui; `cargo tree -d` is non-empty | Milestones get stuck on resolution problems | Lock 0.36.2 + commit `Cargo.lock` + upgrade drills (02 §2.6); decision point at G1 | 1, throughout |
| R2 | JOSM semantic differences (selection rules, shortcuts, conflicts, negative id/visibility) | Users comparing side by side find "JOSM does this, NEO does not" | Migration failure: usable but not habitual | parity defined by workflow rather than feature count ([13](./13-josm-parity.md)); differences registered one by one | 3, 4 |
| R3 | MapCSS / Presets compatibility gaps | Real styles/presets render or behave inconsistently | Cannot use the existing OSM style ecosystem | Compatibility layer + difference list; cover Top-N styles first ([11](./11-style-presets-mapcss.md)) | 4, 5 |
| R4 | GPL asset compliance (JOSM code / presets / icons / styles) | Assets of unknown origin introduced; Java code pasted directly | License contagion; the whole repo becomes undistributable | Only align behavior, never copy code; record origin and license for each asset ([17](./17-licensing-and-governance.md)) | Throughout |
| R5 | GPU backend differences (DX12 / Vulkan / Metal / software adapters) | A backend shows a black screen, color shift, missing MSAA, or broken picking | Inconsistent experience across three platforms | Capability detection + fallback paths (05 §2.14); golden-image comparison across backends | 1, 6 |
| R6 | Memory peak (10M objects + history + snapshots coexisting) | Peak RSS over budget; monotonic growth over a 1-hour session | OOM, crashes, data-loss risk | Compact layout + memory-estimation API + peak as a G1/G2 metric; compression columns in phase 6 | 1, 2, 6 |
| R7 | Plugin security (WASM escape, resource exhaustion, unauthorized IO) | Plugins can write files/network; a single plugin maxes out CPU | User data risk, trust collapse | wasmtime 50 sandbox + capability allowlist + fuel/memory caps; attack tests ([15](./15-plugin-system.md)) | 5 |
| R8 | Long-term maintenance (upstream breaking upgrades, staffing) | A lot of time each quarter spent on upgrade adaptation | Project stalls | Fixed upgrade windows; key dependencies isolated in a few crates; docs and ADRs kept in sync | Throughout |

### 6.2 Risk-to-phase mapping

| Phase | Main risks (by priority) |
|---|---|
| 1 | R1, R5, R6 |
| 2 | R2, R6, R1 |
| 3 | R2, R6 |
| 4 | R2, R3, R4, R1 |
| 5 | R7, R4, R3, R8 |
| 6 | R5, R6, R8 |

## 7 Acceptance criteria

### 7.1 Decision rules

1. Every Gate must have a `GateReport` (including commit, adapter, median of 5 runs) written to `docs/prototypes/gate-<phase>.md`; verbal conclusions are invalid.
2. A Gate `NoGo` must list the blocking item ids; `Inconclusive` (measurement failure/environment anomaly) must not be treated as passing.
3. When any phase claims "done", its acceptance items must **all** be reproducible (script or manual checklist), and missing items count as "not done", not as "known limitations".
4. Performance verdicts use medians; results must not be taken from a single best run, nor compared across hardware.
5. Every risk in the register has an explicit state at the end of its phase (mitigated / accepted / transferred / closed).

### 7.2 What does not count as done (anti-hallucination checklist)

- Can import 1M but the UI freezes for more than one frame budget during import → not a G1 pass.
- Has an undo button but one drag produces N history entries → editing not usable.
- Upload succeeds once but cannot explain the reason under 409 → sync loop not closed.
- Writes `<osmChange version="0.6">` but is missing the `changeset` attribute / has the wrong order → not format-compatible.
- MapCSS renders the demo style but real OSM Carto output is misaligned → not style-compatible.
- Plugin runs hello world but can loop forever without timing out → plugin not usable.
- Three platforms "compile" but there is no installer/cold-start verification → not cross-platform support.
- 10M objects "can open" but panning drops to unusable interaction → performance not met.
- Only tested Windows and marked Linux/macOS as "should be fine" → not three platforms.
- Docs state thresholds but there is no measurement record → not acceptance.

## 8 References

- OSM API v0.6: https://wiki.openstreetmap.org/wiki/API_v0.6
- OsmChange (`version="0.6"`): https://wiki.openstreetmap.org/wiki/OsmChange
- OSM PBF format: https://wiki.openstreetmap.org/wiki/PBF_Format
- JOSM menu baseline (actual File/Edit/Tools/Data items): https://josm.openstreetmap.de/wiki/Help/Menu
- JOSM project and license (GPL-2.0+): https://josm.openstreetmap.de/wiki/Source
- egui / eframe 0.36.2: https://docs.rs/eframe/0.36.2/eframe/
- wgpu 30.0.2: https://docs.rs/wgpu/30.0.2/wgpu/
- wasmtime 50: https://docs.rs/wasmtime/50.0.0/wasmtime/
- criterion 0.8 (benchmark framework): https://docs.rs/criterion/0.8/criterion/
- puffin 0.20 (frame profiling): https://docs.rs/puffin/0.20/puffin/
- Version and dependency evidence chain: `.research/crate-api-cheatsheet.md`; JOSM capability/license inventory: `.research/josm-inventory.md`

> To verify: phase timelines (no historical velocity data; estimates refused); three-platform baseline numbers (need G1 measurements); the `docs/prototypes/` directory has not been created yet, and the Gate report template will land with subsequent commits.
