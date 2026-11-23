//! # neo-editor — 编辑核心（阶段 3）
//!
//! 所有数据修改的唯一入口：`Command` → [`EditContext`]（数据 + 空间索引 + 反查）→ `DataSet`。
//!
//! 设计要点（docs/06、ADR-0008、ADR-0009）：
//! * **单写者**：`Editor` 持有数据与索引，命令顺序执行；
//! * **原子事务**：先 precheck 全部命令，再依次 apply；任一失败逆序 undo 已应用部分；
//! * **History**：支持命令批与拖动合并（连续 MoveNode 合并为一条）；
//! * **Journal**：JSONL 帧，replay 只应用到最后一个 `TxCommit`，杀进程不产生半事务。
//!
//! 本 crate 不依赖 GUI/GPU/网络。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod command;
pub mod commands;
pub mod commands_geom;
pub mod conflict;
pub mod delete_plan;
pub mod editor;
pub mod history;
pub mod journal;
pub mod order;
pub mod selection;

pub use command::{Command, CommandError, CommandKind};
pub use commands::{
    AddNode, AddRelation, AddWay, DeleteObject, MoveNode, RelationMemberSpec, SetRelationMembers, SetTags,
    SetWayNodes, from_journal,
};
pub use commands_geom::{MergeWay, ReverseWay, SplitWay, merged_nodes, reverse_direction_tags};
pub use conflict::{ConflictResolution, MergeCase, ObjectImage, TagMerge, classify, merge_tags, resolve};
pub use delete_plan::{DeletePlan, plan_delete};
pub use editor::{DiffMapping, EditContext, Editor, MergeStats, RecoveryReport, world_of};
pub use history::History;
pub use journal::{
    FileJournal, Journal, JournalCommand, JournalFrame, JournalSink, MemoryJournal, ReplayPlan,
    committed_prefix, read_journal, read_journal_report, replay_plan,
};
pub use order::order_relations_by_dependency;
pub use selection::Selection;
