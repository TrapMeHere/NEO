//! Undo/Redo 历史。
//!
//! 一个条目可以是单命令或命令批（例如"画一个节点 + 连成 way"是一个原子操作）。
//! 同类相邻命令可合并（拖动连续 MoveNode → 1 条历史，G3 验收项）。
//!
//! 脏标记用「已应用状态 id」与「上次保存时的 id」比较：撤销回到保存点
//! （含撤销到空历史）后不再显示未保存；仅有 redo 栈时也不算脏。

use crate::command::{Command, CommandKind};
use crate::editor::EditContext;
use crate::journal::JournalCommand;

/// 一条历史记录。
pub enum HistoryEntry {
    /// 单命令。
    Single(Box<dyn Command>),
    /// 原子命令批（undo 逆序）。
    Batch(Vec<Box<dyn Command>>),
}

impl HistoryEntry {
    /// 标签。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Single(c) => c.label(),
            Self::Batch(cmds) => cmds.first().map(|c| c.label()).unwrap_or("batch"),
        }
    }

    fn kind(&self) -> Option<CommandKind> {
        match self {
            Self::Single(c) => Some(c.kind()),
            Self::Batch(_) => None,
        }
    }
}

/// 撤销/重做栈。
pub struct History {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    /// 与 `undo` 平行：每条入栈后对应的已应用状态 id。
    undo_state_ids: Vec<u64>,
    /// 与 `redo` 平行：该条目曾对应的已应用状态 id（redo 时写回）。
    redo_state_ids: Vec<u64>,
    /// 当前已应用状态 id（空历史为 0）。
    current_id: u64,
    /// 下一个状态 id。
    next_id: u64,
    /// 上次 `clear_dirty` 时的状态 id。
    clean_id: u64,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    /// 新建（默认上限 512 条）。
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            undo_state_ids: Vec::new(),
            redo_state_ids: Vec::new(),
            current_id: 0,
            next_id: 0,
            clean_id: 0,
            limit: 512,
        }
    }

    /// 设置内存上限（条目数）。
    pub fn with_limit(limit: usize) -> Self {
        Self {
            limit: limit.max(16),
            ..Self::new()
        }
    }

    /// 可撤销条数。
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// 撤销栈标签（最近的可撤销操作在前），用于命令历史面板。
    pub fn labels(&self) -> Vec<&'static str> {
        self.undo.iter().rev().map(HistoryEntry::label).collect()
    }

    /// 可重做条数。
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// 是否有未保存修改（当前已应用状态 ≠ 上次保存点）。
    pub fn is_dirty(&self) -> bool {
        self.current_id != self.clean_id
    }

    /// 清除脏标记（保存/上传后调用）：把当前状态标为已保存。
    pub fn clear_dirty(&mut self) {
        self.clean_id = self.current_id;
    }

    fn bump_state(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.current_id = self.next_id;
        self.current_id
    }

    /// 压入单命令；与栈顶同类可合并时合并。
    pub fn push(&mut self, cmd: Box<dyn Command>) {
        self.redo.clear();
        self.redo_state_ids.clear();
        if let Some(last) = self.undo.last_mut()
            && last.kind() == Some(cmd.kind())
            && last.kind().map(|k| k.mergeable()).unwrap_or(false)
            && let HistoryEntry::Single(last_cmd) = last
            && last_cmd.merge(cmd.as_ref())
        {
            let id = self.bump_state();
            if let Some(tip) = self.undo_state_ids.last_mut() {
                *tip = id;
            }
            return;
        }
        let id = self.bump_state();
        self.undo.push(HistoryEntry::Single(cmd));
        self.undo_state_ids.push(id);
        self.trim();
    }

    /// 压入命令批（作为一条历史）。
    pub fn push_batch(&mut self, cmds: Vec<Box<dyn Command>>) {
        if cmds.is_empty() {
            return;
        }
        self.redo.clear();
        self.redo_state_ids.clear();
        let id = self.bump_state();
        self.undo.push(HistoryEntry::Batch(cmds));
        self.undo_state_ids.push(id);
        self.trim();
    }

    fn trim(&mut self) {
        while self.undo.len() > self.limit {
            self.undo.remove(0);
            if !self.undo_state_ids.is_empty() {
                self.undo_state_ids.remove(0);
            }
        }
    }

    /// 当前已应用历史的 journal 记录（按应用顺序）。
    ///
    /// 用于 undo/redo 后的 journal 重建：`undo` 栈里的命令顺序重放即可得到
    /// 当前数据状态（redo 栈里的命令视为未应用）。
    pub fn records(&self) -> Vec<JournalCommand> {
        let mut out = Vec::new();
        for entry in &self.undo {
            match entry {
                HistoryEntry::Single(cmd) => out.push(cmd.record()),
                HistoryEntry::Batch(cmds) => {
                    out.extend(cmds.iter().map(|cmd| cmd.record()));
                }
            }
        }
        out
    }

    /// undo 一步：返回被撤销条目的标签。
    pub fn undo(
        &mut self,
        ctx: &mut EditContext<'_>,
    ) -> Result<Option<&'static str>, crate::command::CommandError> {
        let Some(entry) = self.undo.last_mut() else {
            return Ok(None);
        };
        match entry {
            HistoryEntry::Single(cmd) => {
                cmd.undo(ctx)?;
            }
            HistoryEntry::Batch(cmds) => {
                for index in (0..cmds.len()).rev() {
                    if let Err(err) = cmds[index].undo(ctx) {
                        // 回滚已经撤销的步骤（按 undo 的相反顺序 apply），保持"失败即无副作用"。
                        for cmd in &mut cmds[index + 1..] {
                            let _ = cmd.apply(ctx);
                        }
                        return Err(err);
                    }
                }
            }
        }
        let entry = self.undo.pop().expect("entry checked above");
        let state_id = self.undo_state_ids.pop().unwrap_or(0);
        let label = entry.label();
        self.redo.push(entry);
        self.redo_state_ids.push(state_id);
        self.current_id = self.undo_state_ids.last().copied().unwrap_or(0);
        Ok(Some(label))
    }

    /// redo 一步。
    pub fn redo(
        &mut self,
        ctx: &mut EditContext<'_>,
    ) -> Result<Option<&'static str>, crate::command::CommandError> {
        let Some(entry) = self.redo.last_mut() else {
            return Ok(None);
        };
        match entry {
            HistoryEntry::Single(cmd) => {
                cmd.apply(ctx)?;
            }
            HistoryEntry::Batch(cmds) => {
                for index in 0..cmds.len() {
                    if let Err(err) = cmds[index].apply(ctx) {
                        for cmd in cmds[..index].iter_mut().rev() {
                            let _ = cmd.undo(ctx);
                        }
                        return Err(err);
                    }
                }
            }
        }
        let entry = self.redo.pop().expect("entry checked above");
        let state_id = self.redo_state_ids.pop().unwrap_or_else(|| self.bump_state());
        let label = entry.label();
        self.undo.push(entry);
        self.undo_state_ids.push(state_id);
        self.current_id = state_id;
        Ok(Some(label))
    }

    /// 清空（新打开文件时）。
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.undo_state_ids.clear();
        self.redo_state_ids.clear();
        self.current_id = 0;
        self.next_id = 0;
        self.clean_id = 0;
    }

    /// 估算内存（条目数 × 粗估）。
    pub fn mem_bytes(&self) -> usize {
        (self.undo.len() + self.redo.len()) * core::mem::size_of::<usize>() * 16
    }
}

#[cfg(test)]
mod tests {
    use crate::commands::AddNode;
    use crate::editor::Editor;
    use neo_core::LatLon;
    use neo_store::DataSet;

    #[test]
    fn undo_to_empty_history_clears_dirty_even_with_redo() {
        let mut editor = Editor::new(DataSet::new());
        assert!(!editor.history().is_dirty());
        editor
            .execute(Box::new(AddNode::new(LatLon::from_degrees(0.0, 0.0), Vec::new())))
            .unwrap();
        assert!(editor.history().is_dirty());
        assert_eq!(editor.history().undo_len(), 1);
        editor.undo().unwrap();
        assert_eq!(editor.history().undo_len(), 0);
        assert!(editor.history().redo_len() > 0);
        assert!(
            !editor.history().is_dirty(),
            "empty undo with redo only must not look unsaved"
        );
    }

    #[test]
    fn clear_dirty_then_undo_is_dirty_until_redo_back() {
        let mut editor = Editor::new(DataSet::new());
        editor
            .execute(Box::new(AddNode::new(LatLon::from_degrees(0.0, 0.0), Vec::new())))
            .unwrap();
        editor.history_mut().clear_dirty();
        assert!(!editor.history().is_dirty());
        editor.undo().unwrap();
        assert!(editor.history().is_dirty());
        editor.redo().unwrap();
        assert!(!editor.history().is_dirty());
    }
}
