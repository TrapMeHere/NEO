//! Deterministic synthetic dataset generator.
//!
//! Purpose:
//!
//! * Benchmarks don't depend on external downloads (Monaco is too small to represent millions/tens of millions);
//! * Regression tests are reproducible (fixed seed → fixed output);
//! * Covers the real data shapes an editor sees: nodes, roads (mostly short ways), closed building outlines,
//!   and a small number of relations (multipolygons).
//!
//! It generates a "city grid": `nodes` nodes are laid out as a `cols × rows` grid with jitter;
//! each grid point connects one way to the right and one downward (simulating roads); every `SUBDIV`
//! cells a closed building outline is generated (5 nodes); every 64 buildings produce one multipolygon relation.

use neo_core::{LatLon, Meta, NeoError, ObjectId, ObjectKind, Sym};
use neo_osm::{Node, Relation, RelationMember, Tags, Way};
use neo_store::DataSet;

/// Synthetic dataset spec.
#[derive(Clone, Copy, Debug)]
pub struct SynthSpec {
    /// Node count (rounded up to cols*rows by the grid).
    pub nodes: usize,
    /// Random seed.
    pub seed: u64,
    /// Grid spacing (meters).
    pub spacing_m: f64,
    /// Base latitude.
    pub origin_lat: f64,
    /// Base longitude.
    pub origin_lon: f64,
}

impl Default for SynthSpec {
    fn default() -> Self {
        Self {
            nodes: 100_000,
            seed: 0x4E45_4F00,
            spacing_m: 20.0,
            origin_lat: 47.3769,
            origin_lon: 8.5417,
        }
    }
}

impl SynthSpec {
    /// Spec with the given node count.
    pub fn with_nodes(nodes: usize) -> Self {
        Self {
            nodes,
            ..Default::default()
        }
    }
}

/// Generates a synthetic dataset.
///
/// Returns `(dataset, way_count, relation_count)`.
pub fn generate(spec: &SynthSpec) -> Result<(DataSet, usize, usize), NeoError> {
    let target = spec.nodes.max(4);
    // As close to a square grid as possible.
    let cols = (target as f64).sqrt().ceil() as usize;
    let cols = cols.max(2);
    let rows = target.div_ceil(cols);
    let mut rng = Rng::new(spec.seed);

    let mut ds = DataSet::new();
    let degrees_per_m_lat = 1.0 / 111_320.0;
    let degrees_per_m_lon = 1.0 / (111_320.0 * spec.origin_lat.to_radians().cos());

    // Nodes: grid + small jitter so geometry doesn't degenerate.
    let mut ids: Vec<Vec<ObjectId>> = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut row_ids = Vec::with_capacity(cols);
        for col in 0..cols {
            let jitter_x = rng.next_f64() * spec.spacing_m * 0.2;
            let jitter_y = rng.next_f64() * spec.spacing_m * 0.2;
            let lat = spec.origin_lat + (row as f64 * spec.spacing_m + jitter_y) * degrees_per_m_lat;
            let lon = spec.origin_lon + (col as f64 * spec.spacing_m + jitter_x) * degrees_per_m_lon;
            let id = ObjectId(ds.next_local_id().raw().abs());
            ds.push_node(Node {
                id,
                pos: LatLon::from_degrees(lat, lon),
                tags: Tags::new(),
                meta: Meta::new_local(),
            })?;
            row_ids.push(id);
        }
        ids.push(row_ids);
    }

    // Roads: horizontally + vertically adjacent nodes.
    // Sample every 8th row/column so the node:way ratio is close to real OSM (about 4–5:1);
    // generating a way on every grid edge would turn 1M nodes into 2M ways, usable only as a pathological stress case.
    let highway = ds.intern("highway");
    let residential = ds.intern("residential");
    let stride = 8usize;
    let mut way_count = 0usize;
    for row in 0..rows {
        for col in 0..cols {
            if row % stride == 0 && col + 1 < cols {
                let nodes = smallvec::smallvec![ids[row][col], ids[row][col + 1]];
                ds.push_way(
                    Way {
                        id: ObjectId(1_000_000 + way_count as i64),
                        nodes,
                        tags: Tags::from_pairs([(highway, residential)]),
                        meta: Meta::new_local(),
                    },
                    neo_store::MissingPolicy::Error,
                )?;
                way_count += 1;
            }
            if col % stride == 0 && row + 1 < rows {
                let nodes = smallvec::smallvec![ids[row][col], ids[row + 1][col]];
                ds.push_way(
                    Way {
                        id: ObjectId(1_000_000 + way_count as i64),
                        nodes,
                        tags: Tags::from_pairs([(highway, residential)]),
                        meta: Meta::new_local(),
                    },
                    neo_store::MissingPolicy::Error,
                )?;
                way_count += 1;
            }
        }
    }

    // Buildings: every 4 cells, a closed outline made of 4 nodes.
    let building = ds.intern("building");
    let yes = ds.intern("yes");
    let mut building_ids: Vec<ObjectId> = Vec::new();
    let mut building_ways: Vec<ObjectId> = Vec::new();
    let mut idx = 0usize;
    for row in (0..rows.saturating_sub(1)).step_by(4) {
        for col in (0..cols.saturating_sub(1)).step_by(4) {
            let ring = [
                ids[row][col],
                ids[row][col + 1],
                ids[row + 1][col + 1],
                ids[row + 1][col],
                ids[row][col],
            ];
            let id = ObjectId(2_000_000 + way_count as i64 + idx as i64);
            ds.push_way(
                Way {
                    id,
                    nodes: ring.iter().copied().collect(),
                    tags: Tags::from_pairs([(building, yes)]),
                    meta: Meta::new_local(),
                },
                neo_store::MissingPolicy::Error,
            )?;
            building_ways.push(id);
            building_ids.push(id);
            idx += 1;
        }
    }

    // Relations: one multipolygon per 8 building members (way members only, to keep the scale in check).
    let multipolygon = ds.intern("multipolygon");
    let outer_role = ds.intern("outer");
    let empty = Sym::EMPTY;
    let mut relation_count = 0usize;
    for chunk in building_ways.chunks(8) {
        if chunk.len() < 2 {
            continue;
        }
        let members = chunk
            .iter()
            .map(|id| RelationMember {
                kind: ObjectKind::Way,
                id: *id,
                role: outer_role,
            })
            .collect::<smallvec::SmallVec<[RelationMember; 4]>>();
        ds.push_relation(
            Relation {
                id: ObjectId(3_000_000 + relation_count as i64),
                members,
                tags: Tags::from_pairs([(multipolygon, empty)]),
                meta: Meta::new_local(),
            },
            neo_store::MissingPolicy::Error,
        )?;
        relation_count += 1;
    }

    let _ = &ids;
    Ok((ds, way_count, relation_count))
}

/// Minimal xorshift RNG (deterministic, no dependencies).
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}
