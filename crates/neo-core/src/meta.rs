//! OSM 对象元数据。
//!
//! 完整替代 JOSM 意味着**不能为了性能丢弃** version / timestamp / changeset / uid / user /
//! visible：上传 diff、冲突解决、历史查看都依赖它们。这里把它们打包进一个 32 字节的
//! `Copy` 结构（10M 对象约 320 MB，是可以接受的上限；进一步压缩方案见 docs/03）。

use crate::sym::Sym;

/// 一个 OSM 图元的服务端元数据。
///
/// 本地新建对象使用 `version == 0`、`timestamp == 0`，`changeset` 为当前编辑会话的占位，
/// `visible == true`（被删除的本地对象用编辑器状态表达，而不是 `visible=false`）。
///
/// 字段顺序刻意按"8 字节字段在前"排列，使 `repr(C)` 下大小恰为 32 字节
/// （version/uid/user/visible 的尾部 padding 合并）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Meta {
    /// Unix 秒级时间戳（OSM 当前精度为秒）。
    pub timestamp: i64,
    /// 所属 changeset ID。
    pub changeset: i64,
    /// OSM 版本号；新建对象为 0，删除后上传版本 +1。
    pub version: u32,
    /// 最后编辑者 uid；`0` 表示未知。
    pub uid: u32,
    /// 最后编辑者用户名（驻留符号）。
    pub user: Sym,
    /// 是否可见（`false` 表示服务端已删除）。
    pub visible: bool,
}

impl Meta {
    /// 本地新建对象的元数据。
    pub const fn new_local() -> Self {
        Self {
            version: 0,
            timestamp: 0,
            changeset: 0,
            uid: 0,
            user: Sym::EMPTY,
            visible: true,
        }
    }

    /// 是否为服务端已有对象（有版本号）。
    #[inline]
    pub const fn is_server_versioned(&self) -> bool {
        self.version > 0
    }

    /// 是否为本地新建。
    #[inline]
    pub const fn is_new(&self) -> bool {
        self.version == 0
    }

    /// 单个对象元数据的字节数。
    #[inline]
    pub const fn mem_bytes() -> usize {
        core::mem::size_of::<Self>()
    }
}

impl Default for Meta {
    fn default() -> Self {
        Self::new_local()
    }
}
