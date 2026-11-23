//! 编辑状态与上传动作语义。
//!
//! 两个概念必须分开，混用是 JOSM 重构最常见的语义 bug：
//!
//! * [`ChangeState`] 是**本地数据状态**（相对上次下载/上传的基线）；
//! * [`ChangeAction`] 是**上传到 OSM API 的动作**（对应 osmChange 的 create/modify/delete）。
//!
//! 一个 `Added` 对象上传时是 `create`；`Deleted` 对象若从未上传过则根本不该出现在
//! diff 里（本地新建后又被删除）。

use neo_core::{ObjectId, ObjectKind};

/// 本地数据状态。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ChangeState {
    /// 与基线一致。
    #[default]
    Unchanged,
    /// 本地新建，尚未有服务端版本。
    Added,
    /// 在服务端版本基础上修改。
    Modified,
    /// 本地删除（服务端仍存在，上传时 delete）。
    Deleted,
}

impl ChangeState {
    /// 是否需要在下次上传时提交。
    #[inline]
    pub const fn is_dirty(self) -> bool {
        !matches!(self, Self::Unchanged)
    }

    /// 是否存在服务端对象（上传时需要 version 才能 modify/delete）。
    #[inline]
    pub const fn has_server_object(self) -> bool {
        matches!(self, Self::Modified | Self::Deleted)
    }
}

/// osmChange 动作。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChangeAction {
    /// `<create>`。
    Create,
    /// `<modify>`。
    Modify,
    /// `<delete>`。
    Delete,
}

impl ChangeAction {
    /// 由本地状态与是否有服务端版本推导动作；无需上传时返回 `None`。
    pub fn from_state(state: ChangeState) -> Option<Self> {
        match state {
            ChangeState::Unchanged => None,
            ChangeState::Added => Some(Self::Create),
            ChangeState::Modified => Some(Self::Modify),
            ChangeState::Deleted => Some(Self::Delete),
        }
    }

    /// osmChange 元素名。
    #[inline]
    pub const fn xml_name(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Modify => "modify",
            Self::Delete => "delete",
        }
    }
}

/// 一次变更的标识（用于 journal / diff / 冲突定位）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ChangeRef {
    /// 对象种类。
    pub kind: ObjectKind,
    /// 对象 ID。
    pub id: ObjectId,
    /// 动作。
    pub action: ChangeAction,
}
