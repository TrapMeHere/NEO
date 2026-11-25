//! # neo-io — OSM data reading and writing
//!
//! * [`pbf`]: PBF import (`osmpbf` 0.3.8, read-only; parallel decoding design in docs/08);
//! * [`osc`]: osmChange 0.6 output (the only format for upload diffs; note it is not "OSC 0.3");
//! * [`synth`]: deterministic synthetic datasets for performance benchmarks and regressions (no external data).
//!
//! Writing PBF needs a self-built encoder (osmpbf offers no write side), see docs/08 §2.7; this iteration gets
//! XML/osmChange round-trips right first, and PBF writing is a separate stage-2 task.
//!
//! Dependency direction: `neo-core`/`neo-osm`/`neo-store` → this crate; this crate does no rendering and never touches the UI.

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
