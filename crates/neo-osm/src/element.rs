//! OSM 图元 DTO。
//!
//! 这些结构是 IO / API / 剪贴板 / 命令边界上的**值对象**，不是存储格式。
//! 存储层（`neo-store`）用 SoA 保存同样的信息，二者可以互相转换。

use neo_core::{LatLon, Meta, ObjectId, ObjectKind, Sym};
use smallvec::SmallVec;

use crate::tags::Tags;

/// 一个 Node。
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// OSM ID（负数为本地新建）。
    pub id: ObjectId,
    /// 经纬度（定点 1e-7 度）。
    pub pos: LatLon,
    /// 标签。
    pub tags: Tags,
    /// 元数据。
    pub meta: Meta,
}

/// 一个 Way，`nodes` 顺序即几何顺序。
#[derive(Clone, Debug, PartialEq)]
pub struct Way {
    /// OSM ID。
    pub id: ObjectId,
    /// 节点序列（顺序敏感；重复节点是合法但通常无效的数据，由 validator 报错）。
    pub nodes: SmallVec<[ObjectId; 8]>,
    /// 标签。
    pub tags: Tags,
    /// 元数据。
    pub meta: Meta,
}

/// Relation 成员。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelationMember {
    /// 目标对象种类。
    pub kind: ObjectKind,
    /// 目标对象 ID。
    pub id: ObjectId,
    /// 成员角色（驻留符号，可能是空串）。
    pub role: Sym,
}

/// 一个 Relation。
///
/// 成员顺序是**数据的一部分**（route / multipolygon 的顺序有语义），编辑与上传
/// 都必须保序，见 docs/13 的"易漏行为"。
#[derive(Clone, Debug, PartialEq)]
pub struct Relation {
    /// OSM ID。
    pub id: ObjectId,
    /// 成员列表。
    pub members: SmallVec<[RelationMember; 4]>,
    /// 标签。
    pub tags: Tags,
    /// 元数据。
    pub meta: Meta,
}

/// 任意 OSM 图元。
#[derive(Clone, Debug, PartialEq)]
pub enum Element {
    /// 点。
    Node(Node),
    /// 线 / 面。
    Way(Way),
    /// 关系。
    Relation(Relation),
}

impl Element {
    /// 对象 ID。
    #[inline]
    pub fn id(&self) -> ObjectId {
        match self {
            Self::Node(n) => n.id,
            Self::Way(w) => w.id,
            Self::Relation(r) => r.id,
        }
    }

    /// 对象种类。
    #[inline]
    pub fn kind(&self) -> ObjectKind {
        match self {
            Self::Node(_) => ObjectKind::Node,
            Self::Way(_) => ObjectKind::Way,
            Self::Relation(_) => ObjectKind::Relation,
        }
    }

    /// 标签。
    #[inline]
    pub fn tags(&self) -> &Tags {
        match self {
            Self::Node(n) => &n.tags,
            Self::Way(w) => &w.tags,
            Self::Relation(r) => &r.tags,
        }
    }

    /// 元数据。
    #[inline]
    pub fn meta(&self) -> &Meta {
        match self {
            Self::Node(n) => &n.meta,
            Self::Way(w) => &w.meta,
            Self::Relation(r) => &r.meta,
        }
    }
}
