//! PBF import.
//!
//! Key facts about OSM PBF (docs/08 has the full description):
//!
//! * Coordinates are stored as **nanodegrees** (1e-9 degrees); `osmpbf` directly provides `decimicro_lat/lon`
//!   (1e-7 degrees), which lines up exactly with NEO's [`neo_core::Coord`], so import never goes through f64;
//! * The element order in a file guarantees Nodes come before Ways/Relations, so a single pass suffices;
//! * `DenseNodes` uses delta encoding + string table, already wrapped by `osmpbf`;
//! * PBF is a **read-only** format (osmpbf has no write side); see the self-built encoder plan in docs/08 §2.7.
//!
//! The current implementation is a **single-threaded sequential import** (correctness first); the parallel
//! pipeline (blob-level parallel decoding + partitioned merge) landed in stage 2, see docs/08 §2.4. Benchmarks report sequential import throughput separately.

use std::path::Path;
use std::time::{Duration, Instant};

use neo_core::{LatLon, Meta, ObjectId, ObjectKind, Sym};
use neo_osm::{Node, Relation, RelationMember, Tags, Way};
use neo_store::{DataSet, MissingPolicy};
use osmpbf::{Element, ElementReader};
use tracing::{debug, warn};

/// Import statistics.
#[derive(Clone, Debug, Default)]
pub struct ImportReport {
    /// Node count.
    pub nodes: u64,
    /// Way count.
    pub ways: u64,
    /// Relation count.
    pub relations: u64,
    /// Ways skipped due to missing node references.
    pub skipped_ways: u64,
    /// Relations skipped due to missing members.
    pub skipped_relations: u64,
    /// Objects rejected due to invalid coordinates or duplicate ids.
    pub invalid: u64,
    /// Input file size in bytes.
    pub input_bytes: u64,
    /// Import duration.
    pub elapsed: Duration,
}

impl ImportReport {
    /// Elements imported per second.
    pub fn elements_per_second(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        (self.nodes + self.ways + self.relations) as f64 / secs
    }

    /// Input throughput (MiB/s).
    pub fn mib_per_second(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs <= 0.0 {
            return 0.0;
        }
        self.input_bytes as f64 / (1024.0 * 1024.0) / secs
    }
}

impl core::fmt::Display for ImportReport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "PBF import: {} nodes, {} ways, {} relations ({} / {} skipped), {:.1} MiB in {:.2}s = {:.0} elem/s, {:.1} MiB/s",
            self.nodes,
            self.ways,
            self.relations,
            self.skipped_ways,
            self.skipped_relations,
            self.input_bytes as f64 / (1024.0 * 1024.0),
            self.elapsed.as_secs_f64(),
            self.elements_per_second(),
            self.mib_per_second(),
        )
    }
}

/// Imports PBF into a new [`DataSet`].
pub fn import_pbf(path: impl AsRef<Path>) -> Result<DataSet, neo_core::NeoError> {
    Ok(import_pbf_with_report(path)?.0)
}

/// PBF import options.
pub struct ImportOptions {
    /// Policy for missing references (bbox extraction suggests `Skip`, integrity checks suggest `Error`).
    pub policy: MissingPolicy,
    /// Cancellation callback: called once every 4096 elements; returning `true` aborts and returns
    /// [`neo_core::NeoError::Cancelled`], **without returning a half-built dataset**.
    pub cancel: Option<Box<dyn Fn(u64) -> bool + Send + Sync>>,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            policy: MissingPolicy::Skip,
            cancel: None,
        }
    }
}

/// Imports PBF and returns statistics (default options).
pub fn import_pbf_with_report(path: impl AsRef<Path>) -> Result<(DataSet, ImportReport), neo_core::NeoError> {
    import_pbf_with_options(path, ImportOptions::default())
}

/// Imports PBF with options (supports cancellation and reference policy).
pub fn import_pbf_with_options(
    path: impl AsRef<Path>,
    options: ImportOptions,
) -> Result<(DataSet, ImportReport), neo_core::NeoError> {
    let path = path.as_ref();
    let input_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let started = Instant::now();

    let reader = ElementReader::from_path(path).map_err(|e| neo_core::NeoError::Parse(e.to_string()))?;
    let mut dataset = DataSet::new();
    let mut report = ImportReport {
        input_bytes,
        ..Default::default()
    };

    let mut cancelled = false;
    let mut first_error: Option<neo_core::NeoError> = None;
    let mut seen = 0u64;
    reader
        .for_each(|element| {
            seen += 1;
            if seen.is_multiple_of(4096)
                && let Some(cancel) = &options.cancel
                && cancel(seen)
            {
                cancelled = true;
                return;
            }
            if cancelled {
                return;
            }
            match element {
                Element::Node(node) => {
                    let id = ObjectId(node.id());
                    let Some(pos) = node_position(node.nano_lat(), node.nano_lon()) else {
                        report.invalid += 1;
                        if options.policy == MissingPolicy::Error {
                            first_error = Some(neo_core::NeoError::InvalidCoordinate(format!(
                                "node {} has invalid coordinates",
                                id.0
                            )));
                        }
                        return;
                    };
                    let tags = intern_tags(&mut dataset, node.tags());
                    let meta = meta_from_info(
                        node.info().version(),
                        node.info().milli_timestamp(),
                        node.info().changeset(),
                        node.info().uid(),
                        node.info().user(),
                        node.info().visible(),
                        &mut dataset,
                    );
                    let visible = meta.visible;
                    match dataset.push_node(Node { id, pos, tags, meta }) {
                        Ok(_) => {}
                        Err(err) => {
                            if options.policy == MissingPolicy::Error {
                                first_error = Some(err);
                                report.invalid += 1;
                                return;
                            }
                            warn!("node {} rejected: {err}", id.0);
                            report.invalid += 1;
                            return;
                        }
                    }
                    if !visible {
                        dataset.set_change_state(ObjectKind::Node, id, neo_osm::ChangeState::Deleted);
                    }
                    report.nodes += 1;
                }
                Element::DenseNode(node) => {
                    let id = ObjectId(node.id());
                    let Some(pos) = node_position(node.nano_lat(), node.nano_lon()) else {
                        report.invalid += 1;
                        if options.policy == MissingPolicy::Error {
                            first_error = Some(neo_core::NeoError::InvalidCoordinate(format!(
                                "node {} has invalid coordinates",
                                id.0
                            )));
                        }
                        return;
                    };
                    let tags = intern_tags(&mut dataset, node.tags());
                    // osmpbf 0.3.8: DenseNode's optional info lives in `dense.rs`, and its field accessors return
                    // values directly (i32/i64/Result<&str>), unlike the Info of Node/Way/Relation.
                    let meta = match node.info() {
                        Some(info) => Meta {
                            version: info.version().max(0) as u32,
                            timestamp: info.milli_timestamp().div_euclid(1000),
                            changeset: info.changeset(),
                            uid: info.uid().max(0) as u32,
                            user: match info.user() {
                                Ok(name) => dataset.intern(name),
                                Err(_) => Sym::EMPTY,
                            },
                            visible: info.visible(),
                        },
                        None => Meta::new_local(),
                    };
                    let visible = meta.visible;
                    if let Err(err) = dataset.push_node(Node { id, pos, tags, meta }) {
                        if options.policy == MissingPolicy::Error {
                            first_error = Some(err);
                            report.invalid += 1;
                            return;
                        }
                        warn!("dense node {} rejected: {err}", id.0);
                        report.invalid += 1;
                        return;
                    }
                    if !visible {
                        dataset.set_change_state(ObjectKind::Node, id, neo_osm::ChangeState::Deleted);
                    }
                    report.nodes += 1;
                }
                Element::Way(way) => {
                    let id = ObjectId(way.id());
                    let tags = intern_tags(&mut dataset, way.tags());
                    let meta = meta_from_info(
                        way.info().version(),
                        way.info().milli_timestamp(),
                        way.info().changeset(),
                        way.info().uid(),
                        way.info().user(),
                        way.info().visible(),
                        &mut dataset,
                    );
                    // Important: PBF way references are **delta encoded**; `raw_refs()` returns raw delta slices,
                    // so `refs()` (which accumulates inside the iterator) must be used to get absolute node ids.
                    // This is the easiest trap in osmpbf 0.3.x (contrary to the docs/old examples).
                    let nodes = way.refs().map(ObjectId::new).collect();
                    let dto = Way {
                        id,
                        nodes,
                        tags,
                        meta,
                    };
                    match dataset.push_way(dto, options.policy) {
                        Ok(Some(_)) => report.ways += 1,
                        Ok(None) => report.skipped_ways += 1,
                        Err(err) => {
                            if options.policy == MissingPolicy::Error {
                                first_error = Some(err);
                                report.invalid += 1;
                                return;
                            }
                            warn!("way {} rejected: {err}", id.0);
                            report.skipped_ways += 1;
                        }
                    }
                }
                Element::Relation(rel) => {
                    let id = ObjectId(rel.id());
                    let tags = intern_tags(&mut dataset, rel.tags());
                    let meta = meta_from_info(
                        rel.info().version(),
                        rel.info().milli_timestamp(),
                        rel.info().changeset(),
                        rel.info().uid(),
                        rel.info().user(),
                        rel.info().visible(),
                        &mut dataset,
                    );
                    let mut members = smallvec::SmallVec::<[RelationMember; 4]>::new();
                    for member in rel.members() {
                        let kind = match member.member_type {
                            osmpbf::RelMemberType::Node => ObjectKind::Node,
                            osmpbf::RelMemberType::Way => ObjectKind::Way,
                            osmpbf::RelMemberType::Relation => ObjectKind::Relation,
                        };
                        let role = member.role().unwrap_or("");
                        members.push(RelationMember {
                            kind,
                            id: ObjectId(member.member_id),
                            role: dataset.intern(role),
                        });
                    }
                    let dto = Relation {
                        id,
                        members,
                        tags,
                        meta,
                    };
                    match dataset.push_relation(dto, options.policy) {
                        Ok(Some(_)) => report.relations += 1,
                        Ok(None) => report.skipped_relations += 1,
                        Err(err) => {
                            if options.policy == MissingPolicy::Error {
                                first_error = Some(err);
                                report.invalid += 1;
                                return;
                            }
                            warn!("relation {} rejected: {err}", id.0);
                            report.skipped_relations += 1;
                        }
                    }
                }
            }
        })
        .map_err(|e| neo_core::NeoError::Parse(e.to_string()))?;

    if let Some(err) = first_error {
        return Err(err);
    }
    if cancelled {
        return Err(neo_core::NeoError::Cancelled);
    }

    report.elapsed = started.elapsed();
    debug!(?report, "PBF import finished");
    Ok((dataset, report))
}

fn node_position(nano_lat: i64, nano_lon: i64) -> Option<LatLon> {
    let pos = LatLon::new(
        neo_core::Coord::from_nanodegrees(nano_lat),
        neo_core::Coord::from_nanodegrees(nano_lon),
    );
    pos.is_valid().then_some(pos)
}

fn intern_tags<'a, I>(dataset: &mut DataSet, tags: I) -> Tags
where
    I: Iterator<Item = (&'a str, &'a str)>,
{
    let mut pairs: smallvec::SmallVec<[(Sym, Sym); 6]> = smallvec::SmallVec::new();
    for (k, v) in tags {
        let key = dataset.intern(k);
        let value = dataset.intern(v);
        pairs.push((key, value));
    }
    Tags::from_pairs(pairs)
}

#[allow(clippy::too_many_arguments)]
fn meta_from_info(
    version: Option<i32>,
    milli_timestamp: Option<i64>,
    changeset: Option<i64>,
    uid: Option<i32>,
    user: Option<Result<&str, osmpbf::Error>>,
    visible: bool,
    dataset: &mut DataSet,
) -> Meta {
    let user_sym = match user {
        Some(Ok(name)) => dataset.intern(name),
        _ => Sym::EMPTY,
    };
    Meta {
        version: version.unwrap_or(0).max(0) as u32,
        timestamp: milli_timestamp.unwrap_or(0).div_euclid(1000),
        changeset: changeset.unwrap_or(0),
        uid: uid.unwrap_or(0).max(0) as u32,
        user: user_sym,
        visible,
    }
}
