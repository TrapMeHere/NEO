//! # neo-editor — editing core (stage 3)
//!
//! The single entry point for all data modification:
//! `Command` → [`EditContext`] (data + spatial index + referrers) → `DataSet`.
//!
//! Design points (docs/06, ADR-0008, ADR-0009):
//! * **Single writer**: `Editor` owns data and indexes; commands execute in order;
//! * **Atomic transaction**: precheck all commands first, then apply them in sequence; on
//!   any failure undo the already-applied part in reverse order;
//! * **History**: supports command batches and drag merging (consecutive MoveNode commands
//!   merge into one entry);
//! * **Journal**: JSONL frames; replay applies only up to the last `TxCommit`, so killing
//!   the process never leaves a half transaction.
//!
//! This crate does not depend on GUI/GPU/network.

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
