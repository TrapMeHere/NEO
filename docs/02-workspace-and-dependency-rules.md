# 02 · Workspace & Dependency Rules

NEO is a single-workspace modular monolith of 18 crates + 6 apps: layering is not enforced by convention but by three gates — crate boundaries, sealed traits, and script assertions. This document is the **single source of truth** for dependency direction, feature combinations, and version coherence, and it is also the origin of the `scripts/check-deps.py` allowlist. The eight-layer responsibilities and thread model are in [System Architecture](./01-system-architecture.md), and the evidence chain for version conflicts is in `.research/crate-api-cheatsheet.md`.

## Summary

The value of a single workspace with many crates is not "tidiness" but the fact that **layering can be rejected by machines**: any out-of-bounds move such as "the UI mutates data directly" or "the render layer depends on store" fails at compile time or in CI, instead of surfacing three months later as a deadlock. This document therefore writes the dependency allowlist, depth table, and version pinning strategy as executable rules rather than stylistic suggestions.

## 1 Goals and Scope

**In scope**: directory layout; allowed / prohibited internal dependencies for each crate; the DAG depth table; enforcement mechanisms (compiler + scripts + CI); check-deps script design and failure examples; the reason the GUI stack is pinned to 0.36.2 and the upgrade drill; feature combinations and the minimal default set; public API stability; semver and MSRV.

**Out of scope**: the internal design and algorithms of each crate (see 04–12); functional selection rationale for external dependencies (see the topic documents and ADRs); release process and signing (see 17).

## 2 Design

### 2.1 Directory Tree

```
neo/
├─ Cargo.toml                 # [workspace] members = ["crates/*","apps/*"]; [workspace.dependencies] is the single version source
├─ Cargo.lock                 # checked in; CI uses --locked
├─ rust-toolchain.toml        # channel = "stable" (local 1.100.0); MSRV = rust-version 1.97
├─ crates/
│  ├─ neo-core/               # L1 leaf: ObjectId / Handle+generation / Sym interning / fixed-point Coord / errors
│  ├─ neo-osm/                # L1 DTO: Tags / Node / Way / Relation / ChangeState (does not touch storage)
│  ├─ neo-store/              # L2 SoA Node|Way|RelationStore, TagStore arena, IdIndex,
│  │                          #    CSR Referrers, ChangeTrack, BitSet dirty maps, DataSet
│  ├─ neo-query/              # L4 read-only exit: MapQuery + MapSnapshot + SnapshotCell (arc-swap)
│  ├─ neo-geo/                # L3 WebMercator f64 meters / Camera / rstar 0.14 / geometry / picking (read-only store)
│  ├─ neo-style/              # L4 MapCSS / Presets
│  ├─ neo-io/                 # L2 PBF / XML / osmChange 0.6 (osmpbf read-only; in-house encoder on the write side)
│  ├─ neo-persist/            # L2 SQLite WAL / command journal / autosave / crash recovery
│  ├─ neo-scene/              # L5 visible set / LOD / SceneFrame / MapVertex (no GPU handles)
│  ├─ neo-render/             # L5 wgpu 30: device setup / pipelines / BufferPool / offscreen / GPU picking
│  ├─ neo-editor/             # L6 Command / Transaction / Undo / Journal / Selection / conflicts
│  ├─ neo-tools/              # L6 select / draw / move / split / merge / simplify (produces EditCommand only)
│  ├─ neo-api/                # L1 API 0.6: changeset PUT, single-request diff upload, 409, download, notes
│  ├─ neo-validate/           # L4 topology / tag / geometry rules (rayon parallel incremental)
│  ├─ neo-plugin/             # L1 WASM host (wasmtime 50): read-only query, returns Command only
│  ├─ neo-ipc/                # L1 line-protocol IPC for the sidecar apps (imagery/sync/launcher)
│  └─ neo-ui/                 # L7 egui panels / dialogs / menus / shortcuts (does not depend on io/api/persist)
├─ apps/neo-desktop/ · neo-cli/ · neo-prototype/ · neo-imagery/ · neo-sync/ · neo-launcher/  # L8 entries, CLI, sidecars and launcher; prototype is performance-only (no ui/editor)
├─ benches/  tests/  docs/    # criterion 0.8 benchmarks; cross-crate integration tests; this document set
├─ scripts/  assets/          # check-deps.py / check-write-api.py / i18n checks; icons·presets·styles
```

### 2.2 Dependency Allowlist (Single Source of Truth)

Each row in the table is exactly one `RULES` entry in `scripts/check-deps.py`. **Adding any internal edge = changing both the table in this document and the script allowlist + an ADR or two-person review**, otherwise CI refuses the merge. The four newest workspace members (`neo-ipc`, `neo-imagery`, `neo-launcher`, `neo-sync`) are already enforced by the script but their rows are not yet in this table; see the note after the table.

| crate | Allowed internal dependencies | Hard prohibitions | Feature gating |
|---|---|---|---|
| `neo-core` | — (leaf) | Any internal crate; GUI/GPU/IO types | `serde` (optional) |
| `neo-osm` | core | store / geo / render / ui | `serde` |
| `neo-store` | core, osm | Any upper layer; GUI/GPU | — |
| `neo-geo` | core, osm, store | Mutable DataSet; GPU/UI | — |
| `neo-style` | core | store / osm / GPU | — |
| `neo-io` | core, osm, store | editor / ui / render | — |
| `neo-persist` | — (currently no internal crates) | editor / ui / render | — |
| `neo-api` | core | store / osm / editor / ui | `http` (off by default) |
| `neo-query` | core, osm, store, geo | editor (avoid read/write cycles); render/ui | — |
| `neo-validate` | core, osm, store | editor / geo / ui / render | — |
| `neo-scene` | core, osm, store, geo, style | render / editor / ui / query | — |
| `neo-render` | core, geo, scene | **store / editor / ui** | `pick-readback` |
| `neo-editor` | core, osm, store, geo | **render / ui / io** | — |
| `neo-tools` | core, osm, store, geo, editor | store write API (must go through Command) | — |
| `neo-plugin` | core, query, editor | store (not even indirectly) | `wasm-plugins` |
| `neo-i18n` | (no internal dependencies) | store / editor / render | — |
| `neo-ui` | core, osm, store, style, i18n | **io / api / persist / render / editor** | — |
| `neo-desktop` | core, osm, store, io, geo, scene, render, editor, persist, tools, validate, api, plugin, ui, style, i18n | **query** (the snapshot is not yet wired to the UI) | `api`/`persist`/`wasm-plugins`/`profile` |
| `neo-prototype` | core, osm, store, io, geo, scene, render | ui / editor / api / persist | `profile` |
| `neo-cli` | core, store, io, geo, scene, style, plugin | editor / ui / api / persist | `plugins` |

Two cross-layer edges are **documented and permitted only as such**: ① `neo-validate → store` (read-only rules need the reference-set view); ② `neo-plugin → query + editor` (plugins read snapshots only and return commands only). `neo-query` is currently used only by plugins; neither the UI nor desktop depends on it.

The four newest members are not yet listed in the table above; their allowed internal dependencies are exactly what `scripts/check-deps.py` encodes: `neo-ipc` (leaf), `neo-imagery` (ipc), `neo-sync` (core, api, io, ipc), and the launcher app `neo` (leaf). They will be folded into this table in the next revision; until then the script remains the executable source of truth for them.

### 2.3 DAG and Depth

`depth(c) = 0` (no internal dependencies), otherwise `1 + max(depth(dep))`, computed from actually declared edges; the conceptual layers (01 §2.1) and compile depth are ordered differently, and reviews follow this table.

| depth | crates | Key meaning |
|---|---|---|
| 0 | core, i18n, persist | No internal dependencies. persist currently only handles session files and does not link store |
| 1 | osm, api, style | osm is DTOs; api depends only on core; style only parses styles |
| 2 | store | The data truth layer |
| 3 | geo, io, validate, ui | geo/io read store; validate does not read geo; ui reaches only store and style |
| 4 | scene, editor, query | scene depends on geo+style; editor depends on geo; query is the plugin's read-only exit |
| 5 | render, tools, plugin | render depends only on core+geo+scene; tools produce commands through editor; plugin reads query |
| 6 | prototype, cli, desktop | prototype has no ui/editor; desktop does not depend on query |

Depth is computed from **actually declared edges** and must equal this table. Adding a `neo-editor` dependency to `neo-scene` would not create a cycle, but the depth would go from 4 to at least 5, and R3 rejects it until this table and §2.2 are changed together.

The four newest members are not in the table above yet; the depth values recorded in `scripts/check-deps.py` are: `neo-ipc` 0, `neo-imagery` 1, `neo-sync` 4, and the launcher app `neo` 6.

### 2.4 Enforcement Mechanisms

| Gate | Mechanism | Catches | Does not catch |
|---|---|---|---|
| G1 Compile time | crate boundaries (undeclared dependencies are unavailable) | Direct out-of-bounds references | Indirect use through public re-exports |
| G2 Type sealing | sealed trait, write APIs `pub(crate)`, write capability `!Send` | Semantic overreach such as "the UI got write permission" | All bypass paths (needs G3 as a backstop) |
| G3 Script | `check-deps.py`: R1 allowlist, R2 cycles, R3 depth, R6 `publish=false` | Dependency direction and depth drift | R4/R5 and runtime behavior |
| G4 CI assertions | `cargo tree -p X` free of banned packages, `Instance::new` count is 1, grep for write-entry call sites, `--locked` | Dependency tree and source-level violations | Logic errors (covered by tests) |

Minimal implementation of G2 (the signature is public, but external crates cannot construct write permits, so only editor can initiate writes):

```rust
mod private { pub struct Seal; }
pub struct WriteSeal(private::Seal);                              // cannot be constructed externally
pub trait WriteSealSource { fn seal(&self) -> WriteSeal; }        // impl only inside editor
```

### 2.5 `scripts/check-deps.py` Design

**Input**: `cargo metadata --format-version 1 --no-deps` (no remote index fetch, works offline). **Output**: violation lines tagged `R1`/`R2`/`R3`/`R6`; on success `OK: N packages, DAG + depth + publish rules hold`. **Exit code**: `0` pass / `2` violation. The script currently enforces R1, R2, R3, and R6; R4 and R5 in the table below are still design and not yet in the script.

| Rule | Content | Failure example |
|---|---|---|
| R1 | Every internal edge must appear in the §2.2 allowlist | `EDGE neo-ui -> neo-persist (R1: not in allowlist)` |
| R2 | The allowlist graph must be acyclic (three-color DFS) | `CYCLE neo-store -> neo-geo -> neo-store (R2)` |
| R3 | Depth must equal the §2.3 table; drift requires changing the table in the same PR | `DEPTH neo-scene expected 4 got 6 (R3)` |
| R4 | `dev-dependencies` may only be at the same depth or shallower | `EDGE neo-geo -> neo-editor (dev, R4: points upward)` |
| R5 | `[features]` names must match the §2.7 table | `FEATURE neo-store/api-cache not declared (R5)` |
| R6 | Every internal crate must have `publish = false` | `PUBLISH neo-query is publishable (R6)` |

```python
#!/usr/bin/env python3
"""Validate internal workspace dependency edges. The allowlist = the docs/02 §2.2 table, one row each (single source of truth)."""
import json, subprocess, sys
from collections import defaultdict

RULES = {   # allowed edges for the workspace members: copied row by row from the §2.2 table (new edges must change both places in the same PR)
    ...     # omitted here; fill in from the table when implementing, including the two recorded edges ui->query and plugin->query
}
DEPTH = {...}   # see the §2.3 table: expected depth for each workspace member, copied row by row

def main() -> int:
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"], text=True))
    pkg = {p["name"]: p for p in meta["packages"]}
    errs, edges, devs = [], defaultdict(list), defaultdict(list)
    for name, allowed in RULES.items():
        if name not in pkg:
            errs.append(f"MISSING {name}"); continue
        if pkg[name].get("publish") != []:
            errs.append(f"PUBLISH {name} is publishable (R6)")
        for dep in pkg[name]["dependencies"]:
            d = dep["name"]
            if d not in DEPTH:                 # external dependencies are outside these rules
                continue
            (devs if dep["kind"] == "dev" else edges[name]).append(d)
            if d not in allowed:
                errs.append(f"EDGE {name} -> {d} (R1: not in allowlist)")

    color = defaultdict(int)                   # 0 white / 1 gray / 2 black; recursive cycle detection
    def visit(n, path):
        color[n] = 1
        for d in edges[n]:
            if color[d] == 1: errs.append("CYCLE " + " -> ".join(path + [d]) + " (R2)")
            elif color[d] == 0: visit(d, path + [d])
        color[n] = 2
    for n in RULES:
        if color[n] == 0: visit(n, [n])

    def depth(n): return 0 if not edges[n] else 1 + max(depth(d) for d in edges[n])
    for n, want in DEPTH.items():
        if depth(n) != want: errs.append(f"DEPTH {n} expected {want} got {depth(n)} (R3)")
    for n, ds in devs.items():
        for d in ds:
            if DEPTH.get(d, 0) > DEPTH[n]: errs.append(f"EDGE {n} -> {d} (dev, R4)")
    if errs:
        print("\n".join(errs)); return 2
    print(f"check-deps: OK ({len(edges)} crates)")
    return 0

if __name__ == "__main__":
    sys.exit(main())
```

**Failure form** (CI pastes the raw output into the PR):

```
$ python scripts/check-deps.py
EDGE neo-ui -> neo-persist (R1: not in allowlist)
DEPTH neo-scene expected 4 got 6 (R3)
exit code 2
```

**Negative self-tests** (run them locally before changing the allowlist; there is no dedicated CI job for this yet): a) add a `neo-io` edge to `neo-ui`; b) construct a three-node cycle; c) delete an already-declared feature — assert that each exits with `2` and that the message contains `R1`/`R2`/`R5`. Merging an allowlist without negative examples is not allowed. **Implementation status**: `scripts/check-deps.py` is checked in and executed by CI for R1 (allowlist), R2 (cycles), R3 (depth table), and R6 (publish=false); R4 (dev edges) and R5 (feature table) still need to be added. `cargo fmt` / `cargo clippy -- -D warnings` / `cargo test` are already CI gates.

### 2.6 Version Coherence Strategy (Why the GUI Stack Is Pinned to 0.36.2)

| Fact (crates.io dependency JSON + source) | Consequence | Approach |
|---|---|---|
| `egui_dock` 0.21.1 (latest) requires `egui ^0.36`; `egui-phosphor` 0.14.0 likewise `^0.36` | Moving to 0.37 would link both `egui 0.36` and `0.37`: the `Ui` type in `DockState::show_inside` mismatches (compile error), and fonts are installed into the wrong `Context` (runtime icon tofu, silent) | All three GUI crates (egui / eframe / egui-wgpu) pinned to `0.36.2` |
| `egui-wgpu` 0.36.2 requires `wgpu ^30.0`, `winit ^0.30.13`, `bytemuck ^1.25` | Downgrading egui does **not** downgrade wgpu | wgpu stays at `30.0.2` |
| `geo` 0.33.1 binds `rstar 0.12` through the `rstar_0_12` feature of `geo-types` | Two trees coexist with `rstar 0.14`; `geo::Polygon` does not satisfy `rstar 0.14::RTreeObject` | Disable geo's rstar interop; `neo-geo` writes its own newtype `RTreeObject` |
| `osmpbf` 0.3.8's `rust-zlib`/`zlib`/`zlib-ng` are mutually exclusive (source `compile_error!`) | Enabling two at once fails compilation directly, with an unintuitive error | Only the default `rust-zlib` is allowed; switching must be recorded in the PR and in this table at the same time |

**Approach**: versions are declared only once in `[workspace.dependencies]`, and crates always use `{ workspace = true }`; `Cargo.lock` is checked in and CI uses `--locked`; `cargo tree -d` is **forbidden** from showing two major versions of egui/wgpu/winit/rstar; `rust-version` is unified to **1.97** (determined by `wasmtime 50`), with `rust-toolchain.toml` fixed at 1.100.0; CI runs both the default set and `--all-features`.

**Upgrade drill** (upgrade points are concentrated in `apps/neo-desktop` and `neo-render`): ① read the breaking changes in both CHANGELOGs; ② the spike touches only the three GUI crates (plus wgpu if necessary); ③ assert with `cargo tree -d` that there are no duplicate major versions; ④ fix the entry layer (`eframe::App` shape, `RenderState` fields, `Surface` presentation); ⑤ regress golden images + picking consistency; ⑥ write back to this section, [ADR-0005](./adr/0005-wgpu-egui-shared-device.md), and [16 Risk R1](./16-roadmap-and-risks.md). Lockfile changes require pasting a `git diff Cargo.lock` summary in the PR.

### 2.7 Feature Strategy

The default build is the **minimal playable set**: it can open files, edit, render, and save, without networking, database persistence, or WASM. Rationale: the default build must have zero C dependencies on all three platforms (`rusqlite bundled` and `wasmtime` both carry C/CMake burdens), and CI must be fast.

| feature | Owning crate | Default | Pulls in | Behavior when disabled |
|---|---|---|---|---|
| `default` | combination of crates | on | core/osm/store/query/geo/style/scene/render/editor/tools/ui/desktop | — |
| `api` | `neo-api` + desktop | off | tokio 1.54, reqwest | Menu items grayed out; Upload returns `Unsuitable` |
| `persist` | `neo-persist` + desktop | off | rusqlite 0.42 (bundled), rkyv | In-memory journal only; autosave disabled |
| `wasm-plugins` | `neo-plugin`; forwarded by `neo-cli`'s `plugins` | off | wasmtime 50 | Desktop does not link wasmtime. The CLI's `plugins` subcommand is rejected outright |
| `validate` | `neo-validate` + ui | **on** | — | Validation panel empty; upload is not blocked beforehand |
| `proj-db` | `neo-geo` | on | `proj` database | Only EPSG:3857/4326 |
| `pick-readback` | `neo-render` | on | — | Picking falls back to CPU (`neo-geo`) |
| `profile` | `neo-scene`, `neo-render` | off | puffin 0.20 | Scene build and upload are not instrumented |

**Constraints**: features may only "add capability" and must not change default semantics; additions/removals must update this table and R5 in the same PR; the resolver is fixed at `"3"`; **features must not express permissions** (permissions rely on sealed traits + call-site assertions, see §2.4).

### 2.8 Public API Stability

Internal crates are all `publish = false`, with versions inherited uniformly from the workspace (`0.1.x`), and **no semver promise is made externally**; however, cross-crate APIs are still used by 16 consumers, which is equivalent to an internal semver:

| Rule | Approach | Counter-example |
|---|---|---|
| Do not leak implementation details | Public fields private + constructors; `#[non_exhaustive]` for enums and config structs | `pub struct Snapshot { pub nodes: Vec<Node> }` |
| Enum evolution | `ObjectKind`, `NeoError`, `EditError`, `EditorIntent`, `HostCall` are all `#[non_exhaustive]` | Downstream exhaustive `match` → adding a variant breaks it |
| Extension points | sealed trait (`WriteSealSource`, `GeometrySink`, `ElementSink`) | Letting third parties implement `EditCommand` (plugins go through the WASM ABI) |
| Typed IDs | `ObjectId` / `Handle<T>` newtypes; exposing bare `i64`/`u32` is forbidden | Passing a way id as `i64`, silently swapping node/way |
| Error convergence | One `thiserror` enum per crate; cross-layer `From` converges to `NeoError`/`EditError` | `anyhow` appearing in a library crate's public signature |
| Versioned ABI / layout | Plugin ABI and journal each carry a `u32` version number; SoA field layout must not be exposed in public APIs | Silent misreads without a version number; downstream indexing by offset, so refactors break the whole build |

## 3 Key Interfaces

```rust
// crates/neo-store/src/seal.rs —— write permit sealing (§2.4 G2)
pub struct WriteSeal(/* private */);
pub trait WriteSealSource { fn seal(&self) -> WriteSeal; }

// crates/neo-query/src/lib.rs —— cross-layer read-only queries (shared by plugins / validators / background tasks / UI)
pub trait MapQuery { /* signatures in 01 §3 */ }
pub struct SnapshotCell { /* ArcSwap<MapSnapshot> */ }
impl SnapshotCell {
    pub fn load(&self) -> arc_swap::Guard<Arc<MapSnapshot>>;  // valid for one frame
    pub fn store(&self, snapshot: MapSnapshot);               // the only publication point
}

// crates/neo-ui/src/lib.rs —— the UI depends only on these, not on the DataSet write interface
pub struct UiContext<'a> {
    pub snapshot: arc_swap::Guard<Arc<MapSnapshot>>,  // must not be stored in struct fields
    pub intents: &'a dyn IntentSink,
}

// crates/neo-core/src/error.rs —— cross-layer error convergence point (#[non_exhaustive])
pub enum NeoError {
    InvalidCoordinate(String), StaleHandle { kind: ObjectKind, index: u32 },
    DanglingReference { kind: ObjectKind, id: ObjectId, referrer_kind: ObjectKind, referrer: ObjectId },
    Parse(String), Consistency(String), Io(#[from] std::io::Error),
}
```

```toml
[workspace.dependencies]           # versions appear only here; crates always use { workspace = true }
neo-query = { path = "crates/neo-query" }
egui      = "0.36.2"               # all three GUI crates on the same version (§2.6)
wgpu      = "30.0"                 # egui-wgpu 0.36.2 requires ^30
# neo-render's internal edges are restricted to three: neo-core / neo-geo / neo-scene
```

## 4 Data Flow

```
Change Cargo.toml -> cargo build (crate boundary / type firewall fails immediately)
   -> pre-commit: python scripts/check-deps.py (R1..R6)
         ├─ violation: print EDGE/CYCLE/DEPTH/FEATURE lines, exit 2
         └─ pass: exit 0
   -> CI `test` job runs `scripts/check-deps.py` along with fmt/clippy/check/test: allowlist changes are only allowed in the same PR as docs/02 §2.2
   -> CI `test` job also checks the default build and `--all-features` (wasmtime requires Rust >= 1.97); the `docs` job builds the mdBook
   -> merge: the table and the script allowlist must change in the same PR (§7)
```

## 5 Tradeoffs and Alternatives

| Choice | Alternative | Why this one | Cost |
|---|---|---|---|
| Single workspace (many crates) | Single crate + `mod`; or multiple repos | Crate boundaries are compiler-enforced; refactors are atomic, CI covers the whole graph at once | Changing an edge touches several `Cargo.toml` files; risk of `pub` sprawl |
| Allowlist script | Rely only on cargo's cycle detection | Cargo only prevents cycles, not "legal but wrong" directions (`ui→io` has no cycle but violates layering) | The allowlist must be maintained by hand (negative self-tests hold back rot) |
| Permissions via types + CI | Permissions via features | Features are merged globally and cannot distinguish callers | The write entry is still `pub`, so `check-write-api.py` is needed as a backstop |
| Versions centralized in `[workspace.dependencies]` | Each crate declares its own | One upgrade applies across the line, avoiding two versions of the same crate | When one crate needs an old version, the conflict becomes explicit (exactly the desired effect) |

## 6 Risks

| # | Risk | Trigger signal | Mitigation |
|---|---|---|---|
| B1 | A resolution conflict in `egui`/`egui_dock` (or `geo`/`rstar`) stalls the milestone | First `cargo build` reports two egui versions | The conclusions and evidence in §2.6; decision point at the prototype Gate (16 §2.8) |
| B2 | The allowlist is bypassed by "just adding a line" | A PR changes both the script and the docs without an ADR | Allowlist changes require an ADR or two-person review; negative self-tests may not be deleted |
| B3 | Dependencies outside the checklist (bytemuck / HTTP / CLI / glyphs) are ungoverned | New `[dependencies]` with no review | A small "dependency admission" ADR; registered as risk R1 in 16 |
| B4 | Full-feature CI runtime balloons and gets skipped | The `--all-features` check frequently times out or is disabled | Run the full set only on Windows+Linux once a day; PRs use the minimal set |
| B5 | The `no_std` target rots (`neo-core` pulling in `std`) | The `no_std` job fails | Add a separate `--no-default-features` compile for core in CI |

## 7 Acceptance Criteria

1. `python scripts/check-deps.py` exits `0` against the current tree; on violation it exits `2` with messages containing `R1`/`R2`/`R6`.
2. `cargo tree -p neo-render` does not contain `neo-store`/`neo-editor`/`neo-ui`; `cargo tree -p neo-ui` does not contain `neo-io`/`neo-api`/`neo-persist`.
3. `cargo build --workspace --no-default-features --features desktop` succeeds on Windows/Linux/macOS.
4. `cargo build --workspace --all-features` succeeds, and `wasmtime`/`rusqlite` do not enter the default build artifacts (compare binary size and `cargo tree`).
5. `scripts/check-write-api.py` only matches `neo-editor` as a construction site for write permits.
6. The dependency graph depth deviates from the §2.3 table by zero (R3 passes); new edges update this table in the same PR, and the PR description includes a `git diff Cargo.lock` summary.

## 8 References

- Cargo workspaces and `[workspace.dependencies]`: https://doc.rust-lang.org/cargo/reference/workspaces.html
- Cargo feature resolution and merging (resolver v3): https://doc.rust-lang.org/cargo/reference/features.html
- `cargo metadata` JSON format (script input contract): https://doc.rust-lang.org/cargo/commands/cargo-metadata.html
- Rust API Guidelines (`#[non_exhaustive]`, sealed traits): https://rust-lang.github.io/api-guidelines/
- semver and 0.x compatibility rules: https://semver.org/
- Dependency declaration of `egui_dock` 0.21.1 (requires `egui ^0.36`): https://crates.io/api/v1/crates/egui_dock/0.21.1/dependencies
- `egui-phosphor` 0.14.0: https://crates.io/crates/egui-phosphor/0.14.0
- `egui-wgpu` 0.36.2 (requires `wgpu ^30.0`): https://crates.io/crates/egui-wgpu/0.36.2
- rstar dependency of `geo` 0.33.1: https://crates.io/crates/geo/0.33.1
- `osmpbf` 0.3.8 (mutually exclusive zlib features): https://docs.rs/osmpbf/0.3.8/osmpbf/
- Version verification records (including source and dependency JSON evidence): `.research/crate-api-cheatsheet.md`

> To be verified: whether the current tree achieves zero duplicates in `cargo tree -d` (requires confirmation after a full `cargo build`); fill the conclusion back into this section and into [16](./16-roadmap-and-risks.md) risk R1.
