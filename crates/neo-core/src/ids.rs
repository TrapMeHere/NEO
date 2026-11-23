//! 外部 OSM 标识与对象种类。
//!
//! 约定（与 JOSM 一致）：`ObjectId > 0` 表示服务端已有 ID；`ObjectId < 0` 表示本地新建、
//! 尚未上传的对象；`ObjectId == 0` 保留给"无 ID"。内部存储从不用 OSM ID 直接寻址，
//! 而是先经 `IdIndex` 映射为 [`crate::Handle`]，见 `neo-store`。

use core::fmt;

/// 外部 OSM 对象标识（Node / Way / Relation / Changeset 共用）。
///
/// 使用 `i64` 而不是 `i32`：OSM 的 node ID 已超过 12 位，changeset ID 仍在增长，
/// 未来任何 32 位假设都会成为数据损坏风险。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ObjectId(pub i64);

impl ObjectId {
    /// 无 ID 的占位值。
    pub const NONE: Self = Self(0);

    /// 构造。
    #[inline]
    pub const fn new(raw: i64) -> Self {
        Self(raw)
    }

    /// 原始值。
    #[inline]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// 是否为本地新建（尚未上传）对象。
    #[inline]
    pub const fn is_local(self) -> bool {
        self.0 < 0
    }

    /// 是否已有服务端 ID。
    #[inline]
    pub const fn is_server(self) -> bool {
        self.0 > 0
    }

    /// 是否为空 ID。
    #[inline]
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({})", self.0)
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for ObjectId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_i64(self.0)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for ObjectId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        i64::deserialize(d).map(Self)
    }
}

/// OSM 对象种类。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum ObjectKind {
    /// 点。
    Node = 0,
    /// 线 / 闭合面。
    Way = 1,
    /// 关系。
    Relation = 2,
    /// 变更集（只用于 API / 元数据，不在空间索引里）。
    Changeset = 3,
}

impl ObjectKind {
    /// 可编辑的图元种类（不含 changeset）。
    pub const EDITABLE: [Self; 3] = [Self::Node, Self::Way, Self::Relation];

    /// 稳定的小写名字，用于日志、XML/OSC 输出与持久化。
    #[inline]
    pub const fn type_name(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Way => "way",
            Self::Relation => "relation",
            Self::Changeset => "changeset",
        }
    }

    /// 从 `node`/`way`/`relation` 解析。
    pub fn from_type_name(name: &str) -> Option<Self> {
        match name {
            "node" => Some(Self::Node),
            "way" => Some(Self::Way),
            "relation" => Some(Self::Relation),
            "changeset" => Some(Self::Changeset),
            _ => None,
        }
    }
}

impl fmt::Display for ObjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.type_name())
    }
}
