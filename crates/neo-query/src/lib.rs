//! # neo-query — 只读地图查询能力
//!
//! 存在的理由（架构缺口修复，见 ADR-0007 与 docs/15）：
//!
//! * 插件、验证器、后台任务需要"按 ID 读对象 / 按 bbox 查对象 / 查父引用"；
//! * 但它们**不允许**持有 `&mut DataSet`，也不应该直接依赖 `neo-store` 的写接口；
//! * 于是把只读能力收敛成一个 trait + 一个不可变快照视图。
//!
//! ```text
//! 编辑线程（单写者）           后台 / 插件（多读者）
//! ┌──────────────┐  快照发布   ┌────────────────────┐
//! │ &mut DataSet │ ──────────► │ Arc<MapSnapshot>   │
//! └──────────────┘  arc-swap   │  → QueryView<'_>   │
//!                              └────────────────────┘
//! ```
//!
//! 注意：本 crate 只做**查询**。任何修改必须回到 `neo-editor` 的 Command 通道。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::sync::Arc;

use neo_core::{ObjectId, ObjectKind};
use neo_geo::{SpatialIndex, WorldBounds, WorldPoint};
use neo_osm::{Node, Relation, Way};
use neo_store::{DataSet, Referrers};

/// 只读地图查询接口。
///
/// 所有实现都必须是"读时不改变可见状态"的；后台任务可以安全地并发调用。
pub trait MapQuery {
    /// 底层数据集（只读）。
    fn data(&self) -> &DataSet;

    /// 按 ID 读节点 DTO。
    fn node(&self, id: ObjectId) -> Option<Node>;
    /// 按 ID 读 way DTO。
    fn way(&self, id: ObjectId) -> Option<Way>;
    /// 按 ID 读 relation DTO。
    fn relation(&self, id: ObjectId) -> Option<Relation>;

    /// 引用某节点的 way。
    fn parent_ways(&self, node: ObjectId) -> Vec<ObjectId>;
    /// 引用某对象的 relation。
    fn parent_relations(&self, kind: ObjectKind, id: ObjectId) -> Vec<ObjectId>;

    /// 包围盒内的节点（世界坐标）。
    fn nodes_in(&self, bounds: WorldBounds) -> Vec<ObjectId>;
    /// 包围盒内的 way。
    fn ways_in(&self, bounds: WorldBounds) -> Vec<ObjectId>;

    /// 最近的节点（在 `max_meters` 内）。
    fn nearest_node(&self, point: WorldPoint, max_meters: f64) -> Option<(ObjectId, f64)>;
}

/// 一个查询视图：数据集 + 可选的空间索引/反向引用。
///
/// 索引与引用可以缺省（例如刚导入还没建），此时相关查询返回空集，而不是 panic。
pub struct QueryView<'a> {
    data: &'a DataSet,
    index: Option<&'a SpatialIndex>,
    referrers: Option<&'a Referrers>,
}

impl<'a> QueryView<'a> {
    /// 仅数据集（无空间查询能力）。
    pub fn new(data: &'a DataSet) -> Self {
        Self {
            data,
            index: None,
            referrers: None,
        }
    }

    /// 数据集 + 空间索引。
    pub fn with_index(data: &'a DataSet, index: &'a SpatialIndex) -> Self {
        Self {
            data,
            index: Some(index),
            referrers: None,
        }
    }

    /// 完整能力。
    pub fn with_all(data: &'a DataSet, index: &'a SpatialIndex, referrers: &'a Referrers) -> Self {
        Self {
            data,
            index: Some(index),
            referrers: Some(referrers),
        }
    }
}

impl MapQuery for QueryView<'_> {
    fn data(&self) -> &DataSet {
        self.data
    }

    fn node(&self, id: ObjectId) -> Option<Node> {
        let handle = self.data.node_handle(id)?;
        let view = self.data.node_view(handle)?;
        Some(Node {
            id: view.info.id,
            pos: view.info.pos,
            tags: self.data.tags_of(view.info.tags),
            meta: view.info.meta,
        })
    }

    fn way(&self, id: ObjectId) -> Option<Way> {
        let handle = self.data.way_handle(id)?;
        let view = self.data.way_view(handle)?;
        let nodes = view
            .node_refs
            .iter()
            .filter_map(|h| self.data.nodes.id(*h))
            .collect();
        Some(Way {
            id: view.info.id,
            nodes,
            tags: self.data.tags_of(view.info.tags),
            meta: view.info.meta,
        })
    }

    fn relation(&self, id: ObjectId) -> Option<Relation> {
        let handle = self.data.relation_handle(id)?;
        let view = self.data.relation_view(handle)?;
        let members = view
            .members
            .iter()
            .filter_map(|m| {
                let (kind, target_id) = match m.target.kind {
                    ObjectKind::Node => (ObjectKind::Node, self.data.nodes.id(m.target.as_node()?)),
                    ObjectKind::Way => (ObjectKind::Way, self.data.ways.id(m.target.as_way()?)),
                    ObjectKind::Relation => (
                        ObjectKind::Relation,
                        self.data.relations.id(m.target.as_relation()?),
                    ),
                    ObjectKind::Changeset => return None,
                };
                Some(neo_osm::RelationMember {
                    kind,
                    id: target_id?,
                    role: m.role,
                })
            })
            .collect();
        Some(Relation {
            id: view.info.id,
            members,
            tags: self.data.tags_of(view.info.tags),
            meta: view.info.meta,
        })
    }

    fn parent_ways(&self, node: ObjectId) -> Vec<ObjectId> {
        let Some(referrers) = self.referrers else {
            return Vec::new();
        };
        let Some(handle) = self.data.node_handle(node) else {
            return Vec::new();
        };
        referrers
            .ways_of_node(handle)
            .iter()
            .filter_map(|h| self.data.ways.id(*h))
            .collect()
    }

    fn parent_relations(&self, kind: ObjectKind, id: ObjectId) -> Vec<ObjectId> {
        let Some(referrers) = self.referrers else {
            return Vec::new();
        };
        let handles: Vec<neo_core::Handle<neo_store::RelationSlot>> = match kind {
            ObjectKind::Node => self
                .data
                .node_handle(id)
                .map(|h| referrers.relations_of_node(h).to_vec())
                .unwrap_or_default(),
            ObjectKind::Way => self
                .data
                .way_handle(id)
                .map(|h| referrers.relations_of_way(h).to_vec())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        handles
            .iter()
            .filter_map(|h| self.data.relations.id(*h))
            .collect()
    }

    fn nodes_in(&self, bounds: WorldBounds) -> Vec<ObjectId> {
        let Some(index) = self.index else {
            return Vec::new();
        };
        index
            .query_nodes(bounds)
            .iter()
            .filter_map(|e| e.as_node())
            .filter_map(|h| self.data.nodes.id(h))
            .collect()
    }

    fn ways_in(&self, bounds: WorldBounds) -> Vec<ObjectId> {
        let Some(index) = self.index else {
            return Vec::new();
        };
        index
            .query_ways(bounds)
            .iter()
            .filter_map(|e| e.as_way())
            .filter_map(|h| self.data.ways.id(h))
            .collect()
    }

    fn nearest_node(&self, point: WorldPoint, max_meters: f64) -> Option<(ObjectId, f64)> {
        let index = self.index?;
        let bounds = WorldBounds::from_point(point).expanded_by(max_meters);
        index
            .query_nodes(bounds)
            .into_iter()
            .filter_map(|e| {
                let handle = e.as_node()?;
                let pos = self.data.nodes.position(handle)?;
                let id = self.data.nodes.id(handle)?;
                Some((id, WorldPoint::from_latlon(pos).dist(point)))
            })
            .filter(|(_, d)| *d <= max_meters)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }
}

/// 不可变地图快照（可跨线程持有、可原子发布）。
///
/// 当前实现用 `Arc<DataSet>`；**注意**：编辑线程提交事务后发布新快照需要深拷贝
/// 数据集，成本 O(n)。第一版原型只在"导入完成/大批量变更"时发布快照，交互编辑
/// 的渲染走单线程增量路径。真正的 COW/分块快照方案见 docs/07 与 ADR-0006。
#[derive(Clone)]
pub struct MapSnapshot {
    data: Arc<DataSet>,
    index: Option<Arc<SpatialIndex>>,
    referrers: Option<Arc<Referrers>>,
}

impl MapSnapshot {
    /// 仅数据集。
    pub fn new(data: Arc<DataSet>) -> Self {
        Self {
            data,
            index: None,
            referrers: None,
        }
    }

    /// 数据集 + 已构建的索引。
    pub fn with_index(data: Arc<DataSet>, index: Arc<SpatialIndex>) -> Self {
        Self {
            data,
            index: Some(index),
            referrers: None,
        }
    }

    /// 数据集 + 索引 + 反向引用。
    pub fn with_all(data: Arc<DataSet>, index: Arc<SpatialIndex>, referrers: Arc<Referrers>) -> Self {
        Self {
            data,
            index: Some(index),
            referrers: Some(referrers),
        }
    }

    /// 取得只读查询视图。
    pub fn view(&self) -> QueryView<'_> {
        QueryView {
            data: &self.data,
            index: self.index.as_deref(),
            referrers: self.referrers.as_deref(),
        }
    }

    /// 底层数据集 Arc。
    pub fn data(&self) -> &Arc<DataSet> {
        &self.data
    }

    /// 快照版本（等于数据版本）。
    pub fn version(&self) -> u64 {
        self.data.version()
    }
}

/// 用 `arc_swap` 发布/读取快照的容器（读路径无锁）。
///
/// 写者（编辑线程）在事务提交后 `store(new_snapshot)`；任意多个读者 `load()` 得到
/// 一个稳定视图，不会看到半更新状态。
pub struct SnapshotCell {
    cell: arc_swap::ArcSwap<MapSnapshot>,
}

impl SnapshotCell {
    /// 用初始快照构造。
    pub fn new(initial: MapSnapshot) -> Self {
        Self {
            cell: arc_swap::ArcSwap::from_pointee(initial),
        }
    }

    /// 读取当前快照。
    pub fn load(&self) -> arc_swap::Guard<Arc<MapSnapshot>> {
        self.cell.load()
    }

    /// 发布新快照。
    pub fn store(&self, snapshot: MapSnapshot) {
        self.cell.store(Arc::new(snapshot));
    }
}

impl Default for SnapshotCell {
    fn default() -> Self {
        Self::new(MapSnapshot::new(Arc::new(DataSet::new())))
    }
}
