# ADR-0009 · Incremental Update

## Status

- Accepted: 2026-11-15.
- Reversibility: **Medium**. Reverting means going back to "full rebuild per version", which is non-interactive at the 10M scale.
- Scope: `neo-scene` batch model, `neo-geo` cache keys, `neo-render` upload path, `neo-store` dirty marks.
- Related docs: [07](../07-concurrency-and-tasks.md) §2.4, [04](../04-spatial-index-and-geometry.md), [06](../06-editing-core-and-undo.md) §2.3.

## Background

At 10M objects, one drag changes only 1 coordinate and the few ways it belongs to. But "rebuild the entire scene on every commit" means O(N) geometry building + O(N) GPU upload: any interaction drops to single-digit frame rates, and upload bandwidth plus driver synchronization create spikes.

So incremental update is not an optimization but a **prerequisite for interactivity**; it also determines the cache-key design (otherwise "version changes invalidate everything" equals a full rebuild).

## Decision

1. **Dirty propagates per object**: commit pipeline step 6, `dirty.mark(touched)`, records objects and the dependency closure (node → owning ways → bbox of containing relations), marking only the necessary batches.
2. **Cache keys include version ranges**: the geometry cache key is `(ObjectId, Handle.gen, version_range)`; an object hits if it is unchanged over `[v0, v1)`.
3. **Scene batches carry dirty ranges**: `SceneFrame { batches, dirty: Vec<DirtyRange> }`, where `DirtyRange { buffer_id, byte_offset, len }` maps directly to GPU byte ranges.
4. **The buffer pool is reused by capacity tier**: if too small, swap in a larger buffer (copying old data); if large enough, `write_buffer` in place.
   - Small writes go direct; large writes use a staging ring + `copy_buffer_to_buffer`.
5. **Reclaim condition**: a slot may be reclaimed only when `frame_no + in_flight < current_frame` (`in_flight` defaults to 2, configurable to 3).
6. **Incremental spatial index maintenance**: dirty objects are removed and re-inserted via rstar; full imports still use `bulk_load` (see [08](../08-io-pbf-xml-osc.md)).
7. **LOD first**: low zoom uses simplified geometry and label aggregation, and only batches that cross a threshold and enter the view are built in detail.
8. **Per-frame upload budget**: once `upload_cap` is exceeded, the remaining dirty work is pushed to the next frame (catching up frame by frame without creating stutter spikes).

```
commit(touched) ──► dirty closure ──► invalidate geometry cache entries
                                   │
                                   ▼
                     SceneFrame.dirty: [(buffer, offset, len)]
                                   │
                                   ▼
              BufferPool::ensure ──► write_buffer / staging+copy
                                   │
                                   ▼
                    frame_no + in_flight < current ⇒ reclaimable
```

## Alternatives

| Option | Pros | Why rejected |
|---|---|---|
| Full `SceneFrame` rebuild per version | Simplest implementation, never misses anything | O(N) at 10M is infeasible; upload bandwidth and driver synchronization create spikes |
| Double-buffer the whole scene (A/B alternation) | No partial-update risk | VRAM doubles (RTX 3070 8GB rules out large scenes outright) |
| CPU-side raster cache (tile bitmaps) + compositing | Cheap panning, no geometry rebuild | Zoom/style switches require re-rasterizing; picking needs a separate mapping; conflicts with GPU-first |
| GPU compute culling + indirect draw | Handles very large visible sets | Too complex for the first version; reserved as the phase 6 evolution direction, not a baseline |
| Only "whole-layer rebuild" at layer granularity | Simpler than full rebuild | Moving a single node still rebuilds the whole layer; unacceptable at 10M |
| Rebuild only objects in view each frame (no dirty ranges) | No cache keys needed | There are still millions of objects in view; panning means full rebuild, with obvious jitter |

## Consequences

**Positive**

- The cost of one edit is proportional to "the number of affected objects", not the data set size; frame times are predictable.
- VRAM grows on demand (pooling); the pick id table only needs to rebuild dirty batches.
- Orthogonal to LOD: at low zoom, not even detailed geometry is built.

**Negative**

- **Miscalculating the invalidation scope = the picture is inconsistent with the data**, and it often reproduces only under specific operation sequences; needs property tests and dirty-highlight debugging.
- **Cache keys get complex**: the `version_range` × `Handle.gen` combination easily produces leak entries that "never expire", requiring periodic compaction.
- **Vertex layout must be stable**: object vertex order within a batch cannot be rearranged arbitrarily, otherwise it degrades to a full-batch upload.
- **VRAM fragmentation**: capacity tiers + pooling bring internal fragmentation, requiring caps and compaction strategy.

## Verification

1. Move 1 node in a 10M data set: the number of rebuilt batches ≤ number of affected ways + 1, and uploaded bytes ≤ `upload_cap` (`PrepareStats` counters).
2. Cache-key correctness: the geometry-building call count is 0 for objects unchanged between two versions (instrumented counter).
3. Reclaim safety: with `in_flight = 2`, write on three consecutive frames and assert that unfinished frames are not overwritten (`frame_no` assertion + checksum comparison).
4. Visual consistency: after 10k random edits, compare pixel by pixel against a "full rebuild every time" reference implementation (offscreen rendering, tolerance 0).
5. Leaks: after 1 hour of continuous editing, geometry cache entry count and VRAM return near the baseline [threshold to be measured, see docs/14-performance-and-verification.md].
6. LOD stability: zoom back and forth across the threshold 100 times; the rebuild count for the same batch has an upper bound (no jitter).

## References

- `egui_wgpu`'s `CallbackTrait` (per-frame submission point): https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/
- wgpu `Queue::write_buffer` and staging uploads: https://docs.rs/wgpu/30.0.2/wgpu/struct.Queue.html
- rstar's `bulk_load` and incremental `insert`/`remove`: https://docs.rs/rstar/0.14.0/rstar/
- earcutr (triangulation): https://docs.rs/earcutr/0.5.0/earcutr/
