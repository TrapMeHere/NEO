//! Parallel PBF import: blob-level parallel decoding + sequential assembly (stage 2.8).
//!
//! Design (docs/08 §2.4):
//!
//! 1. Read the file sequentially into `Vec<Blob>` (I/O + header parsing are cheap);
//! 2. `rayon` zlib-decompresses and protobuf-decodes each blob in parallel, producing **owned
//!    Raw DTOs** (no cross-thread sharing of borrowed `PrimitiveBlock`);
//! 3. A single thread assembles them into a `DataSet` in Node → Way → Relation order (string interning is
//!    mutable global state, preserving single-writer semantics).
//!
//! Correctness matches sequential import (verified against `tests/pbf_parallel.rs`); cancellation semantics are the same as
//! [`crate::pbf::import_pbf_with_options`].

use std::path::Path;
use std::time::Instant;

use neo_core::{LatLon, Meta, NeoError, ObjectId, ObjectKind, Sym};
use neo_osm::{Node, Relation, RelationMember, Tags, Way};
use neo_store::DataSet;
use osmpbf::{Blob, BlobReader, BlobType, Element, PrimitiveBlock};
use rayon::prelude::*;

use crate::pbf::{ImportOptions, ImportReport};

/// Raw node.
struct RawNode {
    id: i64,
    lat: i64,
    lon: i64,
    tags: Vec<(String, String)>,
    meta: Meta,
}

/// Raw way.
struct RawWay {
    id: i64,
    refs: Vec<i64>,
    tags: Vec<(String, String)>,
    meta: Meta,
}

/// Raw relation.
struct RawRelation {
    id: i64,
    members: Vec<(ObjectKind, i64, String)>,
    tags: Vec<(String, String)>,
    meta: Meta,
}

#[derive(Default)]
struct RawBlob {
    nodes: Vec<RawNode>,
    ways: Vec<RawWay>,
    relations: Vec<RawRelation>,
}

/// Parallel import (default options).
pub fn import_pbf_parallel(path: impl AsRef<Path>) -> Result<DataSet, NeoError> {
    Ok(import_pbf_parallel_with_options(path, ImportOptions::default())?.0)
}

/// Parallel import returning statistics.
pub fn import_pbf_parallel_with_options(
    path: impl AsRef<Path>,
    options: ImportOptions,
) -> Result<(DataSet, ImportReport), NeoError> {
    let path = path.as_ref();
    let input_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let started = Instant::now();

    // 1) Read blob boundaries sequentially (without parsing contents)
    let blobs: Vec<Blob> = BlobReader::from_path(path)
        .map_err(|e| NeoError::Parse(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| NeoError::Parse(e.to_string()))?;

    // 2) Parallel decoding
    let raw: Vec<RawBlob> = blobs.par_iter().map(decode_blob).collect::<Result<Vec<_>, _>>()?;

    // 3) Sequential assembly (Node → Way → Relation)
    let mut dataset = DataSet::new();
    let mut report = ImportReport {
        input_bytes,
        ..Default::default()
    };
    let mut cancelled = false;
    let mut processed: u64 = 0;

    let check_cancel = |processed: u64, cancelled: &mut bool| {
        if let Some(cancel) = &options.cancel
            && processed.is_multiple_of(4096)
            && cancel(processed)
        {
            *cancelled = true;
        }
    };

    for blob in &raw {
        for node in &blob.nodes {
            if cancelled {
                break;
            }
            let pos = LatLon::new(
                neo_core::Coord::from_nanodegrees(node.lat),
                neo_core::Coord::from_nanodegrees(node.lon),
            );
            if !pos.is_valid() {
                report.invalid += 1;
                continue;
            }
            let tag_id = intern_pairs(&mut dataset, &node.tags);
            match dataset.push_node(Node {
                id: ObjectId(node.id),
                pos,
                tags: tag_id,
                meta: node.meta,
            }) {
                Ok(_) => report.nodes += 1,
                Err(err) => {
                    tracing::warn!("node {} rejected: {err}", node.id);
                    report.invalid += 1;
                }
            }
            processed += 1;
            check_cancel(processed, &mut cancelled);
        }
    }
    if !cancelled {
        for blob in &raw {
            for way in &blob.ways {
                if cancelled {
                    break;
                }
                let tag_id = intern_pairs(&mut dataset, &way.tags);
                let nodes: smallvec::SmallVec<[ObjectId; 8]> =
                    way.refs.iter().map(|id| ObjectId(*id)).collect();
                match dataset.push_way(
                    Way {
                        id: ObjectId(way.id),
                        nodes,
                        tags: tag_id,
                        meta: way.meta,
                    },
                    options.policy,
                ) {
                    Ok(Some(_)) => report.ways += 1,
                    Ok(None) => report.skipped_ways += 1,
                    Err(err) => {
                        tracing::warn!("way {} rejected: {err}", way.id);
                        report.skipped_ways += 1;
                    }
                }
                processed += 1;
                check_cancel(processed, &mut cancelled);
            }
        }
    }
    if !cancelled {
        for blob in &raw {
            for relation in &blob.relations {
                if cancelled {
                    break;
                }
                let tag_id = intern_pairs(&mut dataset, &relation.tags);
                let members: smallvec::SmallVec<[RelationMember; 4]> = relation
                    .members
                    .iter()
                    .map(|(kind, id, role)| RelationMember {
                        kind: *kind,
                        id: ObjectId(*id),
                        role: dataset.intern(role),
                    })
                    .collect();
                match dataset.push_relation(
                    Relation {
                        id: ObjectId(relation.id),
                        members,
                        tags: tag_id,
                        meta: relation.meta,
                    },
                    options.policy,
                ) {
                    Ok(Some(_)) => report.relations += 1,
                    Ok(None) => report.skipped_relations += 1,
                    Err(err) => {
                        tracing::warn!("relation {} rejected: {err}", relation.id);
                        report.skipped_relations += 1;
                    }
                }
                processed += 1;
                check_cancel(processed, &mut cancelled);
            }
        }
    }

    if cancelled {
        return Err(NeoError::Cancelled);
    }
    report.elapsed = started.elapsed();
    Ok((dataset, report))
}

fn decode_blob(blob: &Blob) -> Result<RawBlob, NeoError> {
    let mut out = RawBlob::default();
    if !matches!(blob.get_type(), BlobType::OsmData) {
        return Ok(out);
    }
    let block: PrimitiveBlock = blob
        .to_primitiveblock()
        .map_err(|e| NeoError::Parse(e.to_string()))?;
    block.for_each_element(|element| match element {
        Element::Node(node) => out.nodes.push(RawNode {
            id: node.id(),
            lat: node.nano_lat(),
            lon: node.nano_lon(),
            tags: node.tags().map(|(k, v)| (k.to_owned(), v.to_owned())).collect(),
            meta: meta_from_info(
                node.info().version(),
                node.info().milli_timestamp(),
                node.info().changeset(),
                node.info().uid(),
                node.info().user(),
                node.info().visible(),
            ),
        }),
        Element::DenseNode(node) => {
            let meta = match node.info() {
                Some(info) => Meta {
                    version: info.version().max(0) as u32,
                    timestamp: info.milli_timestamp().div_euclid(1000),
                    changeset: info.changeset(),
                    uid: info.uid().max(0) as u32,
                    user: Sym::EMPTY,
                    visible: info.visible(),
                },
                None => Meta::new_local(),
            };
            out.nodes.push(RawNode {
                id: node.id(),
                lat: node.nano_lat(),
                lon: node.nano_lon(),
                tags: node.tags().map(|(k, v)| (k.to_owned(), v.to_owned())).collect(),
                meta,
            });
        }
        Element::Way(way) => out.ways.push(RawWay {
            id: way.id(),
            refs: way.refs().collect(),
            tags: way.tags().map(|(k, v)| (k.to_owned(), v.to_owned())).collect(),
            meta: meta_from_info(
                way.info().version(),
                way.info().milli_timestamp(),
                way.info().changeset(),
                way.info().uid(),
                way.info().user(),
                way.info().visible(),
            ),
        }),
        Element::Relation(relation) => {
            let members = relation
                .members()
                .map(|member| {
                    let kind = match member.member_type {
                        osmpbf::RelMemberType::Node => ObjectKind::Node,
                        osmpbf::RelMemberType::Way => ObjectKind::Way,
                        osmpbf::RelMemberType::Relation => ObjectKind::Relation,
                    };
                    (kind, member.member_id, member.role().unwrap_or("").to_owned())
                })
                .collect();
            out.relations.push(RawRelation {
                id: relation.id(),
                members,
                tags: relation
                    .tags()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect(),
                meta: meta_from_info(
                    relation.info().version(),
                    relation.info().milli_timestamp(),
                    relation.info().changeset(),
                    relation.info().uid(),
                    relation.info().user(),
                    relation.info().visible(),
                ),
            });
        }
    });
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn meta_from_info(
    version: Option<i32>,
    milli_timestamp: Option<i64>,
    changeset: Option<i64>,
    uid: Option<i32>,
    _user: Option<Result<&str, osmpbf::Error>>,
    visible: bool,
) -> Meta {
    Meta {
        version: version.unwrap_or(0).max(0) as u32,
        timestamp: milli_timestamp.unwrap_or(0).div_euclid(1000),
        changeset: changeset.unwrap_or(0),
        uid: uid.unwrap_or(0).max(0) as u32,
        user: Sym::EMPTY,
        visible,
    }
}

fn intern_pairs(dataset: &mut DataSet, pairs: &[(String, String)]) -> Tags {
    let converted: smallvec::SmallVec<[(Sym, Sym); 8]> = pairs
        .iter()
        .map(|(k, v)| (dataset.intern(k), dataset.intern(v)))
        .collect();
    Tags::from_pairs(converted)
}
