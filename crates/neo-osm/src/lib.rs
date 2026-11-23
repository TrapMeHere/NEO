//! # neo-osm — OSM 领域模型
//!
//! 这一层只表达 **OSM 语义**，不关心存储布局、渲染格式或 UI：
//!
//! * [`Tags`]：驻留符号对组成的 tag 集合；
//! * [`Node`] / [`Way`] / [`Relation`]：IO、API、剪贴板、命令参数使用的 DTO；
//! * [`ChangeState`] / [`ChangeAction`]：本地编辑状态与上传动作语义。
//!
//! 物理存储（SoA、句柄、索引）在 `neo-store`；两者刻意分离，见
//! `docs/03-osm-data-model-and-store.md` 与 ADR-0004。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod change;
pub mod element;
pub mod tags;

pub use change::{ChangeAction, ChangeState};
pub use element::{Element, Node, Relation, RelationMember, Way};
pub use tags::Tags;
