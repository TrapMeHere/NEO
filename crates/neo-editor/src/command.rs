//! 命令契约。

use std::any::Any;

use neo_core::{ObjectId, ObjectKind};
use neo_store::{DataSet, Referrers};

use crate::editor::EditContext;
use crate::journal::JournalCommand;

/// 编辑错误。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CommandError {
    /// 对象不存在。
    #[error("{kind} {id} not found")]
    NotFound {
        /// 对象种类。
        kind: ObjectKind,
        /// 对象 ID。
        id: ObjectId,
    },
    /// 对象仍被引用，不能删除。
    #[error("{kind} {id} is still referenced by {what}")]
    Referenced {
        /// 对象种类。
        kind: ObjectKind,
        /// 对象 ID。
        id: ObjectId,
        /// 引用者描述（way 数 / relation 等）。
        what: String,
    },
    /// 参数非法。
    #[error("invalid edit: {0}")]
    Invalid(String),
    /// 存储错误。
    #[error("store error: {0}")]
    Store(String),
}

impl From<neo_core::NeoError> for CommandError {
    fn from(value: neo_core::NeoError) -> Self {
        Self::Store(value.to_string())
    }
}

/// 命令类型。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum CommandKind {
    /// 添加节点。
    AddNode,
    /// 添加 way。
    AddWay,
    /// 添加 relation。
    AddRelation,
    /// 移动节点。
    MoveNode,
    /// 修改标签。
    SetTags,
    /// 修改 way 节点序列。
    SetWayNodes,
    /// 修改 relation 成员。
    SetRelationMembers,
    /// 删除对象。
    Delete,
    /// 分割 way。
    SplitWay,
    /// 合并 way。
    MergeWay,
    /// 反转 way。
    ReverseWay,
}

impl CommandKind {
    /// 是否可与同类命令合并（拖动合并）。
    pub fn mergeable(self) -> bool {
        matches!(self, Self::MoveNode | Self::SetTags)
    }
}

/// 可撤销编辑命令。
pub trait Command: Send {
    /// 类型。
    fn kind(&self) -> CommandKind;
    /// 标签。
    fn label(&self) -> &'static str;
    /// 预检查（无副作用）。
    fn precheck(&self, _data: &DataSet, _referrers: &Referrers) -> Result<(), CommandError> {
        Ok(())
    }
    /// 正向执行。
    fn apply(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError>;
    /// 逆操作。
    fn undo(&mut self, ctx: &mut EditContext<'_>) -> Result<(), CommandError>;
    /// 合并更新命令。
    fn merge(&mut self, _newer: &dyn Command) -> bool {
        false
    }
    /// journal 记录。
    fn record(&self) -> JournalCommand;
    /// 向下转型（用于合并）。
    fn as_any(&self) -> &dyn Any;
}
