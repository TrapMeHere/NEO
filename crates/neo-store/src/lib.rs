//! # neo-store — OSM 数据的紧凑存储层
//!
//! 目标：千万级 OSM 对象在单机内存中可编辑、可渲染、可上传。
//!
//! 核心手段（见 docs/03 与 ADR-0003）：
//!
//! * **SoA**：`Vec<i32>` 存坐标、`Vec<ObjectId>` 存 ID、`Vec<TagSetId>` 存标签引用，
//!   避免每个对象一次堆分配与指针追逐；
//! * **Arena + generation**：句柄失效检测，删除/撤销下不会静默指向错误对象；
//! * **驻留字符串**：tag 文本只存一份；
//! * **追加式引用池**：Way 的节点引用连续存放，编辑不移动其他对象；
//! * **稀疏变更跟踪**：只记录被编辑的对象；
//! * **位图脏集合**：增量重建几何/渲染/验证的依据。
//!
//! 该 crate 不依赖 GUI、GPU、网络；它只依赖 `neo-core` 与 `neo-osm`。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod bitset;
pub mod compact;
pub mod dataset;
pub mod indexes;
pub mod integrity;
pub mod node_store;
pub mod relation_store;
pub mod tag_store;
pub mod way_store;

pub use bitset::BitSet;
pub use compact::{
    decode_coords, decode_delta_varints, decode_sorted_ids, encode_coords, encode_delta_varints,
    encode_sorted_ids,
};
pub use dataset::{DataSet, MissingPolicy, NodeView, RelationView, StoreStats, WayView};
pub use indexes::{ChangeTrack, IdIndex, Referrers};
pub use integrity::{IntegrityIssue, IntegrityReport, check as check_integrity};
pub use node_store::{NodeRef, NodeSlot, NodeStore};
pub use relation_store::{AnyHandle, RelationRef, RelationSlot, RelationStore, StoredMember};
pub use tag_store::{TagSetId, TagStore};
pub use way_store::{WayRef, WaySlot, WayStore};
