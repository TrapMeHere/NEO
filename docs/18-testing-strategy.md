# 18 · Testing strategy (Testing Strategy)

Test sources live on `master`, alongside the program and documentation. Run the workspace tests with `cargo test --workspace`; CI runs them on every push and pull request (see `.github/workflows/ci.yml`). The retired `test` branch is no longer used.

**Abstract**: NEO's test system is built around one proposition: **"we do not have JOSM's source, so we can only claim equivalence through executable evidence"**. Therefore tests are not an appendage of quality activities but the **only evidence chain** for parity ([13](./13-josm-parity.md)) and validator rules ([12](./12-validation.md)). The system has seven layers: unit → property (proptest style) → integration → golden compatibility → end-to-end UI (egui kittest) → GPU image → performance regression, plus three special tracks: fuzz, loom concurrency, and stress. The golden layer is the core innovation: **same input → normalized object-set hash → dual-run comparison against the JOSM black-box CLI**, obtaining falsifiable equivalence under the premise of "not reading JOSM source". All performance thresholds are [to be measured, see docs/14-performance-and-verification.md].

## 1 Goals and scope

| Goal | Criterion |
|---|---|
| Data safety | No path (open/edit/save/upload/recover) produces silent data loss; round-trip fidelity is assertable |
| Falsifiable equivalence | Every P0 item in the parity matrix has at least one automated case (golden / integration / E2E) |
| Locatable regressions | Failure messages can point to "which object, which field, expected vs actual", not just a hash mismatch |
| Feedback speed | PR gate ≤ 15 minutes (excluding real-GPU and 10M fixtures); nightly full run ≤ 90 minutes |
| Controllable nondeterminism | Floating point, ordering, clock, RNG, and GPU drivers are all explicitly normalized or injected; `sleep`-style synchronization is forbidden |

**Out of scope**: porting JOSM Java unit tests (GPL code, cannot be transcribed, see [17](./17-licensing-and-governance.md) §2.2); pixel-level UI visual comparison (we are not Swing); platform installer signature verification; penetration testing (a separate security review).

## 2 Design

### 2.1 Test pyramid

| Layer | Tool | Order of magnitude | Per-run time | Gate | Main responsibility |
|---|---|---|---|---|---|
| L1 unit | `#[test]` (in-crate) | 3k+ | ms | PR | Pure functions: projection, geometry predicates, normalization, parser fragments |
| L2 property | `proptest` style | 200+ | 10–100 ms | PR | Invariants (§2.2 table); random but reproducible input space |
| L3 integration | `tests/*.rs` (workspace level) | 300+ | 100 ms–5 s | PR | Cross-crate: open→edit→save→upload (mock API) |
| L4 golden | Custom harness (§3.1) | 150+ | 0.1–3 s | PR | Normalized object-set hashes, OSC output, validation reports |
| L5 E2E UI | `egui_kittest` | 60+ | 0.2–2 s | PR | Panel interaction, shortcut sequences, undo visibility |
| L6 GPU image | Offscreen render + perceptual hash | 40+ | 0.5–3 s | PR (software adapter) / nightly (real GPU) | Style cascading, LOD, picking consistency |
| L7 performance | `criterion` 0.8 / `iai-callgrind` 0.16 | 80+ | seconds | nightly + before release | Frame time, import, validation, upload generation |
| Special: fuzz | `cargo-fuzz` | 5 targets | minutes | nightly | Parser robustness (PBF/XML/OSC/MapCSS/preset) |
| Special: concurrency | `loom` + stress | 20+ | seconds–30 min | nightly | Atomic ordering of publication/logging/pools |
| Special: memory | `dhat` 0.3 | 5+ | minutes | nightly | Allocation hotspots and leaks |

Pyramid "ratio" constraint: L1+L2 should make up ≥ 70% of all tests, and L5+L6 ≤ 10%. If the E2E count exceeds the unit count, the abstraction layers lack test seams — that is a design problem, not a testing problem.

### 2.2 Editing and format invariants that must hold

These are the core list of L2 properties; each corresponds to a class of bug that really happens:

| ID | Invariant | Formalization | What failure means |
|---|---|---|---|
| INV-A | Undo/redo are inverses | `redo(undo(execute(c, S))) ≡ execute(c, S)` | The command's inverse is incomplete (most common: old tag values not saved) |
| INV-B | Undo of undo equals the original state | `undo(execute(c, S)) ≡ S` | The inverse operation polluted untouched fields |
| INV-C | Failed commands are atomic | `execute(c, S)` returns `Err` ⇒ state ≡ `S` | Partial application (errors after writing half) |
| INV-D | Sequence coalescing is equivalent | `execute_batch([a,b]) ≡ execute(a); execute(b)` (except undo granularity) | Batched processing took a different write path |
| INV-E | Serialization determinism | Two `write`s of the same `Snapshot` produce equal bytes | HashMap iteration order leaked into the output |
| INV-F | XML round-trip semantic equivalence | `parse(write(parse(x))) ≡ parse(x)` | Lost fields (`action`, unknown attributes, `visible`) |
| INV-G | PBF round-trip semantic equivalence | Same as above (PBF domain) | granularity/negative id/`DenseInfo` mishandled |
| INV-H | OSC is replayable | Decode and apply the generated OSC as "the other end", obtaining the same change set | Missing `if-unused` or negative-id reference confusion |
| INV-I | Projection round-trip | `proj(unproj(p)) ≈ p` (relative error ≤ 1e-9) | Formula typos, mixed datums |
| INV-J | Selection does not leak | After operating on layer A's selection, layer B's `Selection` is unchanged | Selection made into a global set (JOSM is per-DataSet) |
| INV-K | Validation fixes are idempotent | Re-running the same rule after a fix no longer produces the same `code` | Fix condition inconsistent with the rule's detection condition |

Property-test input generators must be **structured**: not random bytes, but "randomly construct a valid DataSet" (object counts, tag combinations, topology references, negative-id ratio, and relation nesting depth are all parameters), and failing cases are automatically persisted to `proptest-regressions/`.

### 2.3 Golden compatibility test design

**Core idea**: compress "behavior" into a comparable **canonical form**, then hash it. Each of the four chains has its own golden class:

```
① Object-set golden
   fixtures/monaco.osm.pbf ─► NEO parse ─► canonical serialization ─► sha256 ─► compare
                                    └─► human-readable .osm snapshot (used for failure diff)

② Edit-sequence golden
   fixtures/base.osm + ops.yaml ([{split, way:17}, {reverse, way:23}, …])
        ─► execute command by command ─► OSC generation ─► canonical ─► sha256

③ Validation-report golden
   same input + fixed rule set ─► ValidationReport ─► sorted by (severity, code, object set)
        ─► text projection ─► sha256 (human-readable: each line "W geometry/self_intersection way/17")

④ Render golden (see §2.6, uses perceptual hashing rather than sha256)
```

harness interface (`tests/golden/harness.rs`):

```rust
pub trait GoldenCase {
    fn name(&self) -> &'static str;
    fn fixture(&self) -> Fixture;                  // PBF/XML path + optional op script
    fn produce(&self, rt: &TestRuntime) -> Artifact;
    fn norm(&self) -> NormRules;                   // normalization rules for this case (default: full set in §3.1)
}

pub struct TestRuntime {           // injection point for all nondeterminism
    pub clock: FixedClock,         // fixed timestamps, preventing golden drift due to time
    pub ids: IdAllocator,          // deterministic negative-id allocation order
    pub rng: StdRng,               // fixed seed
    pub threads: usize,            // run once each with 1 and 8; results must be identical
}
pub struct Artifact { pub canonical: Vec<u8>, pub human: String }
pub fn assert_golden(name: &str, art: &Artifact);   // compare tests/golden/<name>.sha256, print the human unified diff on failure
```

**JOSM dual-run harness (oracle)**: invoke the JOSM CLI as an **external process** (`java -jar josm-tested.jar render|validate …`), read its output files, apply the same normalization, and compare with NEO's output. This path is black-box: no linking to JOSM code, no distributing its jar, and the artifact is the run result rather than a derivative work; it is also **skipped by default** on CI (`--features josm-oracle`), running only in a dedicated nightly job.

| Comparison target | After-normalization comparison | Allowed differences |
|---|---|---|
| `render` output PNG | Perceptual-hash Hamming distance (§2.6) | Anti-aliasing, font rendering |
| `validate` report | Issue set (`code` mapping table + object set) | Message wording (only compare code and objects) |
| `project` output coordinates | Relative error ≤ 1e-9 | None |
| Object set | Canonical sha256 | None (any difference is a bug or a registered deviation) |

The code mapping table (`tests/golden/josm-code-map.toml`) is necessary: JOSM's report contains localized messages, while we compare stable codes, so a mapping from "JOSM test class/mapcss rule → NEO `code`" is needed; a missing mapping fails the test (preventing silent skipping of the comparison).

### 2.4 Round-trip fidelity: semantic equivalence when byte-level is not comparable

| Format | Byte-level round-trip possible? | Strategy |
|---|---|---|
| OSM XML | **Yes** (if we emit canonical output) | Main assertion: `write(parse(x))` and `parse(x)` have equal canonical forms; additional assertion: for an **unmodified** document, `write` output equals the input element by element after normalization |
| PBF | **No** (zlib block boundaries, string-table order, `DenseNodes` grouping are decided by the writer) | Semantic equivalence only: `parse(write(parse(x))) ≡ parse(x)`; plus a "no information lost" assertion: `version/timestamp/changeset/uid/user/visible` equal per object |
| OSC | No (block order, optional `if-unused`) | Semantic equivalence + server-perspective simulation (mock API checks diff validity) |
| `.jos`/`.joz` | `.joz` no (zip timestamps/compression level) | XML semantic equivalence after decompression; for `joz` only assert "can be read back by ourselves" |
| GPX | Partial | Semantic equivalence + preserve unknown extension elements (extras mechanism) |

**Unknown-field preservation** is the invisible prerequisite for round-trip fidelity: JOSM's private `action` attribute in XML, PBF's `optional_features`, and new attributes from future versions must all go into `extras` and be written back unchanged. Test method: artificially inject unknown attributes/elements into fixtures, and assert they remain after the round-trip (the `unknown-field-preservation` test family).

### 2.5 E2E UI tests (egui kittest)

Not aiming for "click coordinates", but for **semantic interaction**: find by widget label/`AccessKit` role, input key sequences, assert state.

| Case family | Example |
|---|---|
| Panel visibility | `Tab` cycles docked panels; `Alt+Shift+V` opens the validator panel ([13](./13-josm-parity.md) §3.1) |
| Two-key sequences | Press `A` then `A` within 300 ms → angle snapping enabled; beyond the window → still ordinary Draw |
| Undo visibility | Perform 3 edits → `Ctrl+Z`×3 → panel state matches an empty dataset |
| Conflict resolution | Inject 409 → `Apply` disabled until the four conflict dimensions are all green → clickable after resolution |
| Shortcut rebinding | Rebind `Ctrl+Shift+P` → the old key stops working, the new key works, and it persists after restart |

egui/eframe versions use the lock table in [02](./02-workspace-and-dependency-rules.md) as the single source of truth; this document does not repeat the version numbers.

### 2.6 GPU image tests

```
fixture ─► scene::build ─► renderer.encode ─► texture ─► readback ─► PNG (offscreen, no window/swapchain)
                                                                        │
              dHash 8×8 (64 bit) Hamming distance to baseline ≤ 3 ◄────┤
              on mismatch write out/cargo/gpu-diff/<case>/{new,base,diff}.png ◄─┘
```

| Item | Decision |
|---|---|
| Adapter | Prefer a real GPU; when no hardware, use a software adapter (Windows **WARP**, Linux **lavapipe/lavapipe**); macOS has no software Metal backend → GPU cases on that platform are **skipped and marked** (silent passing is not allowed) |
| Skip policy | Env var `NEO_GPU_TEST=0` or adapter probing failure → `#[ignore]` semantics + an explicit skip report; the skip count must be visible in the CI summary |
| Perceptual hash | dHash 64 bit + Hamming threshold 3; also keep a "structural check": primitive bounding boxes/vertex counts/count assertions (fails earlier than pixels and localizes better) |
| Non-comparable factors | Driver differences, MSAA sample positions, font rasterization → all absorbed by "perceptual hash + structural assertions"; no per-pixel comparison |
| Picking consistency | Key addition: `resolve_pick` must agree between GPU readback and CPU-side index results (this catches real bugs better than images do) |

### 2.7 fuzz / concurrency / stress

| Special track | Target | Configuration | Criterion |
|---|---|---|---|
| fuzz | `fuzz_pbf`, `fuzz_xml`, `fuzz_osc`, `fuzz_mapcss`, `fuzz_preset` | `cargo-fuzz`, corpus extracted from fixtures + hand-written seeds; nightly `-max_total_time=600` per target | Any panic/OOM/infinite loop fails; crashes are saved to `artifacts/` automatically and an issue is opened |
| loom | Ordering of journal append and snapshot publication, `arc-swap` publication point, object pools | Only small models (3–5 structures wrapped by `loom::sync`) | INV-1/INV-6/INV-7 hold under all interleavings |
| Stress | 10M objects + 8 reader threads + 1 writer, 30 min | Self-hosted runner | No panic/deadlock; RSS growth ≤ [to be measured, see docs/14-performance-and-verification.md] |
| Memory | `dhat` 3 scenarios (import/edit/validate) | nightly | No unreleased continuous growth; hotspot allocations registered |

The **boundary** of concurrency testing: `loom` only covers "hand-written synchronization primitives and publication paths" and is not used to blow up the entire `DataSet` (state space infeasible). Other concurrency correctness relies on stress tests + `--full` comparison ([12](./12-validation.md) §2.4).

### 2.8 Performance regression

| Metric | Tool | Stability | Gate |
|---|---|---|---|
| Instructions/branches (deterministic) | `iai-callgrind` 0.16 | High (comparable on any CI machine) | PR: regression > 2% fails |
| Wall clock p50/p95 | `criterion` 0.8 | Low (affected by neighbors/frequency) | nightly: only on fixed self-hosted machines; regression > 10% warns |
| Allocation count/bytes | `dhat` 0.3 | High | nightly: regression > 15% warns |
| Frame-time breakdown | `puffin` 0.20 | Medium | Manual review before release |

CI hardware constraints must be stated clearly: shared cloud runners **only run iai-callgrind** (instruction counts are machine-independent); `criterion` only runs on self-hosted machines (e.g. i5-12400F, see [01](./01-system-architecture.md) §2.4); baselines are stored in `bench/baseline/*.json` and explicitly updated with dependency upgrades (the reason must be stated in the PR description). All thresholds are currently [to be measured, see docs/14-performance-and-verification.md]; fill in this table after the first measurement.

## 3 Key matrices

### 3.1 Normalization rules (the "constitution" of golden)

| Dimension | Rule | Rationale |
|---|---|---|
| Coordinates | Quantize to **1e-7 degrees** (= PBF `granularity` 100 nanodeg); XML output fixed to 7 decimal places | Differences below this precision are meaningless, and PBF itself cannot express them |
| Projection/floating point | Relative error ≤ 1e-9; compare with `approx` rather than `==`; output formatted as `%.12e` | GPU/CPU and different FMA paths introduce ULP-level differences |
| Element order | Sort by `(ObjectType, id)`; **normalize negative ids to `(type, creation_index)`** and number them in creation order | Negative ids are placeholders whose numeric value is meaningless, but creation order is meaningful |
| way node order | **Do not sort** | Direction is data (`oneway`, coastline, atolls) |
| relation member order | **Do not sort** | Order is data (route/multipolygon), see [13](./13-josm-parity.md) §3.6 item 2 |
| tag order | Key lexicographic order (`BTreeMap`) | Eliminates HashMap iteration order (INV-E) |
| Attribute omission | `version/timestamp/uid/user/changeset` are normalized to placeholders by default; kept only when the case asserts them | Avoids fixture timestamps polluting the hash |
| Timestamps | Injected by `FixedClock`; serialized uniformly as `1970-01-01T00:00:00Z` + `clock_offset` | Absolute times are not comparable |
| Whitespace/attribute order (XML) | Compare after parsing, no text diff; write attributes in a fixed order | A text diff would treat formatting differences as bugs |
| Compression | Decompress before comparing (PBF/zlib, `.joz` zip) | Compressor version differences |
| Float render parameters | Zoom/viewport/DPR fixed to the values declared by the case | Otherwise images are not comparable |

### 3.2 Three tiers of test data

| Tier | Scale | Source | Storage | Use |
|---|---|---|---|---|
| S | 100K objects (~10 MB PBF) | Generator (parameterized: density/object-type ratios/relation depth) | git (small) | PR gate, L1–L4 |
| M | 1M objects (~100–150 MB) | Generator + **real Monaco extract** | Download script (sha256 verified), not in git | nightly, L5/L6, performance |
| L | 10M objects (~1.5 GB) | Generator (including 100k relations, extremely dense road network) | Self-hosted runner local cache | nightly performance, stress |

- **Generator**: `scripts/gen-fixture --objects N --seed S --profile {city,rural,multipolygon-heavy,pt-routes}`, deterministic output (fixed seed → identical bytes), with its own self-test (`gen(seed)` twice gives equal results).
- **Monaco extract**: use Geofabrik's `monaco-latest.osm.pbf` (real data, small, containing coastline/relations/multilingual names), **download only, never commit**; write ODbL attribution into `tests/fixtures/NOTICE`, and print the attribution at test runtime.
- Synthetic data must cover "bad data": self-intersecting ways, duplicate nodes, unclosed areas, ring relations, extremely long ways (2000-node boundary), mixed negative ids, `visible=false` — otherwise the validator and upload paths never hit the edges.

### 3.3 CI matrix

| Platform | GPU backend | PR content | nightly content | Notes |
|---|---|---|---|---|
| Windows x64 | D3D12 real GPU / **WARP** | L1–L5, L6(WARP), iai | + L7 wall clock (self-hosted), stress, fuzz | WARP lets PRs run image tests too |
| Linux x64 | Vulkan **lavapipe** | L1–L6(lavapipe), iai | + fuzz, loom, stress | No GPU device needed inside the container |
| macOS arm64 | Metal | L1–L5, iai | + L7 | No software adapter → L6 **skipped and marked** |
| Cross | — | `cargo check` all targets; `cargo-deny`; `reuse lint` | Same | Gates and non-gates separated |

PR gate total time budget ≤ 15 minutes: L1–L4 sharded in parallel (by crate), L5/L6 run sequentially on a single machine, `--full` comparison only at nightly.

Current state: `.github/workflows/ci.yml` runs fmt / clippy / check / test / `--all-features` check plus the i18n, dependency, documentation-link and write-API scripts on Windows, and builds the mdBook in a separate job; the nightly/GPU/performance matrix above is the target. `.github/workflows/test.yml` is a legacy workflow that only triggers on the retired `test` branch and does not run on `master`.

### 3.4 Failure diagnosability requirements

| Layer | Must output on failure |
|---|---|
| golden | Human-readable diff of the inconsistent items (first 20 lines), and save the full artifact to `out/cargo/golden-fail/<case>/` |
| property | The shrunk minimal counterexample + random seed (directly reproducible) |
| GPU | `new.png` / `base.png` / `diff.png` triple + Hamming distance + structural assertion differences |
| integration | Full request/response record of the mock API (including the OSC request body) |
| performance | Baseline vs current median/offset, and the profile summary path |
| fuzz | Minimized input file path + reproduction command |

## 4 Process and sequence

### 4.1 PR pipeline

```
push ─► fmt/clippy ─┬─► L1 unit (sharded in parallel)
                    ├─► L2 property (sharded in parallel, fixed seed)
                    ├─► L3 integration (including mock API 409/413/429 scenarios)
                    ├─► L4 golden (object set/OSC/report)
                    ├─► L5 kittest (UI semantics)
                    ├─► L6 GPU (WARP/lavapipe or skip)
                    ├─► iai-callgrind (deterministic performance gate >2%)
                    ├─► cargo-deny / reuse lint / DCO
                    └─► coverage (no threshold on the report, trend only)
                            ▼
                        merge (≤15 min)
```

### 4.2 nightly and pre-release

```
nightly:
  ├─ L7 criterion (self-hosted machines) + dhat + puffin summary
  ├─ fuzz 5 targets × 10 min
  ├─ loom (all models) + 10M stress 30 min
  ├─ golden for M/L fixtures and incremental validation `--full` comparison ([12](./12-validation.md) §2.4)
  └─ JOSM dual-run oracle (render/validate/project, black-box CLI)

pre-release (manual + scripted):
  ├─ L1–L6 green on all platforms (macOS L6 may skip but must be recorded)
  ├─ baseline update review (state the reason for each change)
  ├─ fuzz cumulative 24 h with no new crashes
  └─ one-week workflow script ([13](./13-josm-parity.md) §4.1) passes and logs are archived
```

### 4.3 golden update process (preventing "change the test to go green")

```
Behavior change → run tests → golden mismatch
   ├─ Is the change intentional? → generate new artifacts → diff review (two people required) → update .sha256 + human snapshot → state clearly in the PR description "why the behavior changed"
   └─ Not intentional → fix the implementation
```

Golden file updates must be in the **same PR** as the behavior change, and the diff must be human-readable; batch `--bless` commits are forbidden (CI caps the number of golden files a single PR may modify, e.g. > 20 requires extra approval).

## 5 Trade-offs and alternatives

| Decision | Choice | Rejected | Rationale / cost |
|---|---|---|---|
| Equivalence evidence | golden + normalized hash | per-field assertions for every case | Per-field cases explode and are hard to maintain; cost is that failure localization needs human snapshots and diff tools |
| Comparison with JOSM | Black-box CLI dual-run (nightly) | Add JOSM as a test dependency and link its code | Linking would contaminate licensing and is technically infeasible (JVM); cost is that comparison coverage is limited by JOSM CLI capabilities |
| PBF round-trip | Semantic equivalence | Byte-level round-trip | Byte-level would treat compressor differences as bugs; cost is that "format fidelity" can only be proven down to the semantic layer (reinforced by unknown-field-preservation tests) |
| GPU tests | Perceptual hash + structural assertions | Per-pixel comparison | Per-pixel is inevitably flaky across drivers; cost is that tiny visual regressions may slip through (structural assertions as a backstop) |
| Performance gate | iai (instruction counts) in PR, wall clock only on fixed machines | Run criterion in all CI | Wall-clock noise on cloud runners makes the gate meaningless; cost is narrow real-machine coverage |
| Concurrency verification | loom limited to small models + stress | Full loom | State space infeasible; cost is that giant structures can only rely on stress and comparison |
| Fixtures | generator-first + real Monaco data | all real data | Real data is not reproducible, is large, and has complex licensing; cost is that synthetic data may miss real weird cases (compensated by "bad data" parameters) |

## 6 Risks

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| T1 | golden is treated as a "change it and it goes green" switch | Equivalence evidence becomes invalid | Updates require two-person review + count cap + PR description template |
| T2 | GPU always skipped on CI → the image layer is effectively nonexistent | Render regressions slip to the end | At least one software path, WARP/lavapipe, must run in PRs; skip counts go into the health dashboard |
| T3 | Normalization too broad (absorbing real differences) | False green | Normalization rules are a review object (§3.1 has a version number); each rule comes with "what it can absorb and what it cannot" |
| T4 | 10M fixtures not in CI → large-scale regressions found late | Performance/correctness problems at large object counts | nightly self-hosted runs; also run a "mini-scale" version with 1M fixtures in PRs |
| T5 | fuzz corpus exhaustion | Coverage stagnates | Generate seeds from real fixtures and differential tests (new vs old parser comparison); review coverage periodically |
| T6 | JOSM CLI output drifts across versions | Dual-run false positives | Pin the JOSM version (r19678) + archive results by version; upgrading JOSM is a separate PR |
| T7 | Property-test generators collapse into "only generating valid data" | Illegal-input paths are missed | There must be a set of "deliberately illegal" generation strategies (negative ids, dangling references, oversized ways) |

## 7 Acceptance criteria

1. **All invariants green**: each of INV-A…INV-K in §2.2 has at least one property test, 0 failures under `--proptest-cases=10000`, and counterexamples are reproducible.
2. **Round-trip fidelity**: S/M fixtures are semantically equivalent on the XML, PBF, and OSC paths; unknown-field-preservation tests pass.
3. **golden coverage**: each of the 52 P0 items in the parity matrix has at least one golden or integration case; missing items are counted by a script and block release.
4. **JOSM comparison**: nightly's `render`/`validate`/`project` comparisons have zero difference after normalization (rendering by perceptual-hash threshold); differences are either fixed or registered in the known-deviations table with a reason.
5. **UI**: 60+ kittest cases cover two-key sequences, conflict panel gating, and shortcut rebinding.
6. **GPU**: both software paths, WARP and lavapipe, pass in CI; macOS skips are clearly visible (not silent skipping).
7. **Performance**: iai regressions > 2% block; wall-clock and memory baselines are backfilled into §2.8 after the first measurement, then hardware drift is reviewed quarterly.
8. **fuzz**: the five targets complete 24 h cumulative with no crashes, no OOM, and no timeouts; the corpus is committed.
9. **Pipeline duration**: PR gate p95 ≤ 15 minutes, nightly ≤ 90 minutes (exceeding it requires optimization, not loosening the timeout).

## 8 References

- `proptest` (shrinking and regression-file semantics) · `cargo-fuzz` (libFuzzer frontend) · `loom` (concurrency interleaving model checking): https://docs.rs/proptest/latest/proptest/ · https://rust-fuzz.github.io/book/cargo-fuzz.html · https://docs.rs/loom/latest/loom/
- `criterion` 0.8 (statistics and baselines) · `iai-callgrind` 0.16 (instruction-count benchmarking, deterministic): https://docs.rs/criterion/0.8.0/criterion/ · https://docs.rs/iai-callgrind/0.16.0/iai_callgrind/
- `dhat` 0.3 (allocation profiling) · `puffin` 0.20 (frame profiling) · `egui_kittest` (UI semantic testing): https://docs.rs/dhat/0.3.0/dhat/ · https://docs.rs/puffin/0.20.0/puffin/ · https://docs.rs/egui_kittest/latest/egui_kittest/
- wgpu offscreen rendering and `readback` (`ImageCopyTexture`/`map_async`): https://docs.rs/wgpu/30.0.2/wgpu/
- Windows WARP (D3D12 software adapter): https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/warp
- Mesa lavapipe (software Vulkan): https://docs.mesa3d.org/drivers/llvmpipe.html
- OSM PBF format (granularity / DenseNodes, which determine normalization precision): https://wiki.openstreetmap.org/wiki/PBF_Format
- OsmChange and `if-unused`: https://wiki.openstreetmap.org/wiki/OsmChange
- Geofabrik Monaco extract (real small fixture, ODbL): https://download.geofabrik.de/europe/monaco.html
- OSM copyright and attribution (fixture distribution obligation): https://www.openstreetmap.org/copyright
- JOSM CLI (`render`/`project`/`validate`, as a black-box oracle): https://josm.openstreetmap.de/wiki/Help/CommandLineOptions
  > To verify: the `validate` subcommand's option table; first-hand docs only give the usage block.
- First-hand fact basis (JOSM r19678, 40 validation test classes, autosave/session defaults): [`.research/josm-inventory.md`](../.research/josm-inventory.md)
