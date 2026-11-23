//! 确定性合成数据集生成器。
//!
//! 用途：
//!
//! * 基准测试不依赖外部下载（Monaco 太小，不能代表百万/千万级）；
//! * 回归测试可复现（固定 seed → 固定输出）；
//! * 覆盖编辑器真实的数据形状：节点、道路（多为短 way）、闭合建筑面、
//!   少量 relation（multipolygon）。
//!
//! 生成的是"城市网格"：`nodes` 个节点铺成 `cols × rows` 网格并加抖动；
//! 每个网格点向右/向下各连一条 way（模拟道路）；每隔 `SUBDIV` 个格子生成一个
//! 闭合建筑轮廓（5 个节点）；每 64 条建筑生成一个 multipolygon relation。

use neo_core::{LatLon, Meta, NeoError, ObjectId, ObjectKind, Sym};
use neo_osm::{Node, Relation, RelationMember, Tags, Way};
use neo_store::DataSet;

/// 合成数据集规格。
#[derive(Clone, Copy, Debug)]
pub struct SynthSpec {
    /// 节点数（会被按网格向上取整到 cols*rows）。
    pub nodes: usize,
    /// 随机种子。
    pub seed: u64,
    /// 网格间距（米）。
    pub spacing_m: f64,
    /// 基准纬度。
    pub origin_lat: f64,
    /// 基准经度。
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
    /// 指定节点数的规格。
    pub fn with_nodes(nodes: usize) -> Self {
        Self {
            nodes,
            ..Default::default()
        }
    }
}

/// 生成合成数据集。
///
/// 返回 `(dataset, way_count, relation_count)`。
pub fn generate(spec: &SynthSpec) -> Result<(DataSet, usize, usize), NeoError> {
    let target = spec.nodes.max(4);
    // 尽量接近正方形网格。
    let cols = (target as f64).sqrt().ceil() as usize;
    let cols = cols.max(2);
    let rows = target.div_ceil(cols);
    let mut rng = Rng::new(spec.seed);

    let mut ds = DataSet::new();
    let degrees_per_m_lat = 1.0 / 111_320.0;
    let degrees_per_m_lon = 1.0 / (111_320.0 * spec.origin_lat.to_radians().cos());

    // 节点：网格 + 小幅抖动，保证几何不退化。
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

    // 道路：横向 + 纵向相邻节点。
    // 每 8 行/列采样一次，使 node:way 比例接近真实 OSM（约 4–5:1）；
    // 若每个网格边都生成 way，1M 节点会产生 2M way，只能作为"病态图形"压测。
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

    // 建筑：每隔 4 个格子，用 4 个节点组成闭合轮廓。
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

    // Relation：每 8 个建筑组成员一个 multipolygon（为控制规模，仅引用 way 成员）。
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

/// 极简 xorshift RNG（确定性、无依赖）。
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
