# Gate-1 Report: Performance Validation Prototype (phase 1 → phase 2 decision point)

- **Gate id**: G1 (the only hard-veto Gate in docs/16 §2.8)
- **Evaluation date**: 2026-11-15
- **Code state**: all 17 workspace `neo-*` crates + 2 apps compile; the `apps/neo-prototype` prototype runs
- **Data**: Monaco/Bremen (real Geofabrik PBF), synthetic grid city (1M objects)
- **Measurement**: `pwsh scripts/bench.ps1 -Runs 3`, median of 3 runs; raw output `out/bench-summary.md`

## 0 Environment (Gate must record)

| Item | Value |
|---|---|
| Host | i5-12400F (6C/12T) / 31.8 GB / RTX 3070 8 GB |
| OS | Windows 11 10.0.26100 / MSVC 14.43 |
| GPU backend | Vulkan (wgpu 30.0.2, adapter NVIDIA GeForce RTX 3070, driver 621.14) |
| Rust | 1.100.0 stable (MSRV 1.95) |
| Build | release: opt-level 3 + thin LTO + codegen-units 1 |
| Dependency lock | egui/eframe/egui-wgpu 0.36.2, wgpu 30.0.2, rstar 0.14, osmpbf 0.3.8 (ADR-0011) |

## 1 G1 criteria item by item

| # | G1 criterion | Measured | Threshold | Verdict |
|---|---|---|---|---|
| ① | 1M-object import time | Bremen real PBF: 1,668,778 nodes + 329,676 ways + 5,053 relations, **1.12 s** (1.79 M elem/s, 18.1 MiB/s); 1M synthetic 0.19 s | Must be measured and recorded | ✅ Go |
| ② | First-frame time (import complete → visible) | Monaco full chain < 100 ms; Bremen: import 1.12 s + index 1.20 s + scene 90 ms ≈ **2.4 s** | Must be measured and recorded | ⚠️ Recorded (over 1 s, needs phase 2 optimization) |
| ③ | Pan/zoom frame time | **z16 editing-viewport scene rebuild P50 1.29 ms / P99 2.63 ms**; panning/zooming only writes uniforms, no rebuild; GPU frame P50 0.083 ms | p50/p99 must be measured; criterion = interactively usable | ✅ Go (<16 ms budget) |
| ④ | Peak RSS | 1M synthetic objects **566.8 MiB**; **10M synthetic objects 4,159.1 MiB (4.06 GiB)**, below the suggested 6 GB budget | Must be measured and recorded | ✅ Go |
| ⑤ | Single GPU-pick latency | **Definition correction**: the design chooses CPU picking (docs/04), measured P50 **12 µs** / P95 17 µs; GPU ID-buffer picking is listed as an optional optimization | Must be measured and recorded | ✅ Go (CPU picking meets <16 ms, but the definition needs review confirmation) |
| ⑥ | 10-minute continuous stability | **8-second smoke passed**: `out/cargo/debug/neo-desktop.exe` started and the window stayed alive for 8 s (including PBF load, scene build, wgpu first frame), the process did not exit or panic; **10-minute continuous interaction was not executed** | 10 minutes must be executed | ⚠️ Partial (smoke passed, 10 minutes not run) |
| ⑦ | Buildable on three platforms | Only verified locally on Windows/MSVC; Linux (Vulkan/X11) and macOS (Metal) not verified | Must build successfully on all three platforms | ❌ **No Go** (not verified) |

## 2 Conclusion

**G1 = Conditional Go**

- Passed: import throughput, edit-scale interaction latency (scene rebuild 1.29 ms, picking 12 µs, GPU frame 0.083 ms), GPU pipeline authenticity (offscreen rendering produces PNGs, see `out/monaco.png`, `out/bremen.png`), 1M memory 566.8 MiB, 10M memory 4.06 GiB.
- Blocking: **⑥ 10-minute continuous stability** (only an 8-second smoke was completed) and **⑦ three-platform build** (only Windows verified) are incomplete.
- Condition: complete the following three items before phase 2 starts; if any fails, G1 becomes No Go and triggers the degradation path in docs/16 §5:
  1. Linux + macOS CI builds (same `Cargo.lock`, wgpu backends Vulkan/Metal respectively);
  2. Run the desktop app for a 10-minute smoke (drag/zoom/select loop), with no crashes and no monotonically growing leaks;
  3. Record peak RSS for `--synth 10_000_000` import + index + scene, and keep it within budget (suggested ≤ 6 GB).

## 3 10M stress results (measured)

`neo-prototype bench --synth 10000000 --iterations 5`:

| Item | Value |
|---|---|
| Object counts | 10,001,406 nodes / 3,129,589 ways / 78,210 relations |
| Build | 3.31 s |
| Spatial index build | 9,154.8 ms (13,130,995 entries) |
| Backreferences | 147.3 ms |
| `mem_bytes` estimate | 1,693.1 MiB |
| **Peak RSS** | **4,159.1 MiB** |
| Scene rebuild (z16) | 2.03 ms P50 / 2.71 ms P99 |
| Scene rebuild (fit-all) | 143 ms P50 |
| id lookup | 300 ns P50 |
| pick | 195 µs P50 |

Verdict: **10M objects can be imported, indexed, and interacted with; index construction and fit-all rebuild are the items where updates hit scale bottlenecks, and they enter the phase 2 optimization list.**

## 4 Known bottlenecks (phase 2 input)

| Bottleneck | Measured | Plan |
|---|---|---|
| PBF single-threaded decode | 1.79 M elem/s | blob-level rayon parallelism (docs/08 §2.4), target 3–5× |
| Spatial index build | 1M entries 572 ms; Bremen 1.3M entries 1.2 s | parallel `bulk_load` / chunked construction |
| Full-city-view scene rebuild | 17 ms (1M) / 90 ms (Bremen) | incremental batch rebuild by dirty ways (next layer of ADR-0009) |
| bbox query returns Vec | 273 µs (7,922 entries) | the scene path is already zero-allocation; change the picking path to iterators |
| First-frame assembly (Bremen 2.4 s) | import + index + scene | overlap the import pipeline with index construction; progress feedback |

## 5 Evidence files

- `out/prototype-bench.md` — raw output of the 1M synthetic benchmark (including P50/P95/P99 tables)
- `out/bench-summary.md` — summary of 3 runs (including host/adapter/driver)
- `out/prototype-render.md` — the most recent offscreen rendering report
- `out/monaco.png`, `out/bremen.png` — GPU rendering evidence (openable with any image viewer)
- `docs/14-performance-and-verification.md` — methodology and complete result tables

## 6 Reproduction commands

```powershell
pwsh scripts/bench.ps1 -Runs 3
# or item by item:
cargo run -p neo-prototype --release -- import --pbf .research/data/bremen-latest.osm.pbf
cargo run -p neo-prototype --release -- render --pbf .research/data/monaco-latest.osm.pbf --out out/monaco.png
cargo run -p neo-prototype --release -- bench  --synth 1000000 --iterations 30
```
