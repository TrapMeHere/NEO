//! # neo-io — OSM 数据读写
//!
//! * [`pbf`]：PBF 导入（`osmpbf` 0.3.8，只读；并行解码设计见 docs/08）；
//! * [`osc`]：osmChange 0.6 输出（上传 diff 的唯一格式，注意不是 "OSC 0.3"）；
//! * [`synth`]：确定性合成数据集，用于性能基准与回归（不依赖外部数据）。
//!
//! 写 PBF 需要自研 encoder（osmpbf 不提供写侧），见 docs/08 §2.7；本期先把
//! XML/osmChange 往返做对，PBF 写作为阶段 2 的独立任务。
//!
//! 依赖方向：`neo-core`/`neo-osm`/`neo-store` → 本 crate；本 crate 不做渲染、不碰 UI。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod gpx;
pub mod osc;
pub mod osc_read;
pub mod pbf;
pub mod pbf_parallel;
pub mod pbf_write;
pub mod synth;
pub mod time;
pub mod xml;

pub use gpx::{GpxImportReport, read_gpx, write_gpx};
pub use osc::{
    ChangeSet, collect_changeset, collect_changeset_selected, stamp_osmchange_changeset, write_changeset,
};
pub use osc_read::{OscApplyReport, read_osm_change, read_osm_change_file};
pub use pbf::{ImportOptions, ImportReport, import_pbf, import_pbf_with_options, import_pbf_with_report};
pub use pbf_parallel::{import_pbf_parallel, import_pbf_parallel_with_options};
pub use pbf_write::{PbfWriteOptions, PbfWriteReport, write_pbf, write_pbf_file};
pub use synth::{SynthSpec, generate};
pub use xml::{
    XmlImportReport, XmlReadOptions, XmlWriteMode, read_osm_xml, read_osm_xml_file, write_osm_xml,
    write_osm_xml_file,
};
