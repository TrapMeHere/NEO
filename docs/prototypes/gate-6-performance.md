# Gate-6 Report: Extreme Performance

- **Gate id**: phase 6 exit, release candidate (docs/16 §2.7)
- **Evaluation date**: 2026-11-16
- **Code state**: worktree uncommitted, baseline commit `add2418`
- **Commands run this round**: the tests relevant to this phase in `cargo test -p neo-scene -p neo-store -p neo-render --offline`; `neo-cli scene --nodes 50000`; `neo-cli open --synth 1000000`

The existing 10M interaction numbers are in [docs/14](../14-performance-and-verification.md). They are full scene rebuilds from before phase 6, not this round's streaming LOD / indirect draw. This document does not count those numbers as results of the new path.

## 0 Environment

| Item | Value |
|---|---|
| OS | Windows 11 10.0.26100 |
| Rust | 1.100.0 |
| Build | dev profile (optimized + debuginfo), not release LTO |
| GPU tests | `indirect_matches_direct_and_gpu_cull_drops_draw` passes, without being skipped via `NoAdapter`. `gpu-info` was not re-run this round |

The `profile` feature (puffin 0.20) is off by default. It was not enabled this round, and there are no frame-profiling records.

## 1 Criteria

| # | Criterion | Measured | Verdict |
|---|---|---|---|
| (a) | 10M+ pan / zoom / picking | The new path was not re-tested at 10M. On dev, `scene --nodes 50000`: 1431 vertices, build **2655 µs**, second build of the same tile `stream_builds=1`. LOD unit tests: Overview stride 8, City stride 2, Street/Edit stride 1; parallel and serial builds produce the same vertex counts | ❌ 10M not tested |
| (b) | Peak memory below budget, no monotonic growth over a 1-hour session | Repeated `intern("highway")` does not increase `StringPool::mem_bytes`. Coordinates use zigzag + LEB128 separately for latitude and longitude; a three-point sample compresses to under 8 bytes/point and decodes back. Tile cache capacity has an upper bound (`cache_stays_bounded`). The 1-hour session was not run | ⚠️ Primitives pass, long session not tested |
| (c) | Cold start | No release installer cold start. CLI synthetic 1M single run 302 ms, see [gate-5](./gate-5-ecosystem.md) | ❌ Not tested |
| (d) | Indirect and direct frames match, GPU cull drops draws | 64×64 red triangle: direct and `draw_indexed_indirect` are byte-for-byte identical in RGBA; after the compute shader writes the indirect `instance_count` to 0, the next frame is entirely black. Test duration 0.67 s (including adapter initialization) | ✅ For this one image |
| (e) | Three-platform performance differences recorded | Only this Windows machine. Linux / macOS were not run | ❌ Not tested |

GPU culling writes the whole batch's instance count to 0; it is not per-vertex frustum computation. CPU frustum counting is in `neo-scene::cull` and is not connected to the `instance_count` at upload time. Default drawing is still `draw_indexed` (`GpuScene.indirect` defaults to false).

## 2 Conclusion

**Phase 6 is not a release candidate.**

What is landed and reproducible: bounded tile cache, coordinate/ID delta varint, non-growing string interning, multithreaded scene building consistent with serial, and on one triangle indirect matching direct pixels with GPU zeroing the instance count stopping drawing.

The acceptance items not landed are the new-path frame time at 10M, the 1-hour memory curve, release cold start, and the three-platform comparison. These remain untested and do not borrow the old docs/14 baseline to pad the numbers.
