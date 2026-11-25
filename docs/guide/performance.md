# Performance and Large Data

NEO aims to keep millions to tens of millions of OSM objects editable. The following data comes from
measurements in [14 Performance and Verification](../14-performance-and-verification.md) (Windows 11 / Rust 1.100).

## Measured baseline (summary)

| Scenario | Data | Result |
|---|---|---|
| Import Bremen | 1.67M nodes / 330k ways | 1.12 s (1.79 M elem/s) |
| Scene rebuild (z16 editing viewport, 1M objects) | 4,164 ways + 12,638 nodes | 1.29 ms P50 |
| GPU frame (cache hit) | 1M objects | 0.083 ms P50 |
| Picking | node/way | 8–22 µs |
| Single node move + parent way/index update | — | 17 µs |

A full-city view (fit-all) rebuild takes about 17–18 ms; this is a one-time cost (only on data change/major jumps),
not a per-frame cost.

## Key rendering optimizations

- Separate R-trees for nodes/ways, viewport bbox pre-culling to avoid full traversals
- Density LOD: sub-pixel ways are sampled by stride, keeping the drawn amount close to "constant per screen"
- Nodes shown only at the necessary zoom (z17–18 shared nodes; z19 all nodes)
- Incremental geometry upload; panning/zoom only updates the 32-byte camera uniform
- Dragging only updates the egui overlay preview; data is written and rebuilt once on release

## Reproducing it yourself

```powershell
# performance log
$env:NEO_PERF_LOG = "out\perf.log"
.\out\cargo\release\neo-desktop.exe

# synthetic data size (node count)
$env:NEO_SYNTH_N = "1000000"
```

The log contains timestamps for `scene build`, `gpu upload`, `ui frame`, etc., and can be compared with the table in docs/14.

## Recommendations for large data

- For 10 million objects, 16 GB+ of memory and a discrete GPU or a recent integrated GPU are recommended
- Close other memory-hungry programs before importing a very large PBF
- Keep a relatively high zoom (z16+) while editing; the full-city view is mainly for navigation
- Export increments with osmChange to avoid repeatedly writing full XML

## Differences from JOSM

NEO's editing-viewport rebuild and GPU drawing are on the millisecond scale; JOSM is based on Java2D, and 1M objects typically take tens of seconds to import,
with interactions noticeably slower. For a full comparison see [13 JOSM parity](../13-josm-parity.md).
