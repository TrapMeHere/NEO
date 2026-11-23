//! GPX 1.1 读写。
//!
//! 航点写成 node（`gpx=waypoint`），每条轨迹段写成一条 way（`gpx=track`）。
//! 这是阶段 5 的附加交换格式，不替代 OSM XML。

use neo_core::{LatLon, Meta, NeoError, ObjectId};
use neo_osm::{Node, Tags, Way};
use neo_store::{DataSet, MissingPolicy};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// 一次 GPX 导入的计数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpxImportReport {
    /// 航点数。
    pub waypoints: usize,
    /// 轨迹点数。
    pub track_points: usize,
    /// 轨迹条数。
    pub tracks: usize,
}

/// 从 GPX 文本追加到数据集。
pub fn read_gpx(xml: &str, data: &mut DataSet) -> Result<GpxImportReport, NeoError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut report = GpxImportReport::default();
    let mut in_track = false;
    let mut track: Vec<ObjectId> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let local = local_name(e.name().as_ref());
                if local == "trk" {
                    flush_track(data, &mut track, &mut report)?;
                    in_track = true;
                } else if local == "wpt" || local == "trkpt" {
                    let (lat, lon) = lat_lon(&e).map_err(NeoError::Parse)?;
                    let pos = LatLon::try_from_degrees(lat, lon)
                        .ok_or_else(|| NeoError::Parse(format!("invalid GPX point {lat},{lon}")))?;
                    let id = data.next_local_id();
                    let kind = if local == "wpt" { "waypoint" } else { "trackpoint" };
                    let mut tags = Tags::new();
                    let (k, v) = data.intern_tag("gpx", kind);
                    tags.set(k, v);
                    data.push_node(Node {
                        id,
                        pos,
                        tags,
                        meta: Meta::new_local(),
                    })?;
                    if local == "wpt" {
                        report.waypoints += 1;
                    } else {
                        report.track_points += 1;
                        track.push(id);
                    }
                }
            }
            Ok(Event::End(e)) => {
                let local = local_name(e.name().as_ref());
                if local == "trkseg" || local == "trk" {
                    flush_track(data, &mut track, &mut report)?;
                    if local == "trk" {
                        in_track = false;
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(NeoError::Parse(err.to_string())),
            _ => {}
        }
        buf.clear();
    }
    if in_track {
        flush_track(data, &mut track, &mut report)?;
    }
    Ok(report)
}

/// 把数据集写成 GPX。不在任何 way 里的节点是航点，每条 way 是一条轨迹。
pub fn write_gpx(data: &DataSet) -> String {
    let mut used = std::collections::HashSet::new();
    let mut tracks = String::new();
    for way in data.iter_ways() {
        let ids = data.way_node_ids(way.handle);
        if ids.len() < 2 {
            continue;
        }
        tracks.push_str("  <trk><trkseg>\n");
        for id in &ids {
            used.insert(*id);
            if let Some(handle) = data.node_handle(*id)
                && let Some(pos) = data.position(handle)
            {
                let (lat, lon) = pos.to_degrees();
                tracks.push_str(&format!("    <trkpt lat=\"{lat}\" lon=\"{lon}\"></trkpt>\n"));
            }
        }
        tracks.push_str("  </trkseg></trk>\n");
    }
    let mut wpts = String::new();
    for node in data.iter_nodes() {
        if used.contains(&node.id) {
            continue;
        }
        let (lat, lon) = node.pos.to_degrees();
        wpts.push_str(&format!("  <wpt lat=\"{lat}\" lon=\"{lon}\"></wpt>\n"));
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"neo\">\n{wpts}{tracks}</gpx>\n"
    )
}

fn flush_track(
    data: &mut DataSet,
    track: &mut Vec<ObjectId>,
    report: &mut GpxImportReport,
) -> Result<(), NeoError> {
    if track.len() >= 2 {
        let mut tags = Tags::new();
        let (k, v) = data.intern_tag("gpx", "track");
        tags.set(k, v);
        let id = data.next_local_id();
        let nodes = std::mem::take(track);
        data.push_way(
            Way {
                id,
                nodes: nodes.into(),
                tags,
                meta: Meta::new_local(),
            },
            MissingPolicy::Error,
        )?;
        report.tracks += 1;
    } else {
        track.clear();
    }
    Ok(())
}

fn local_name(qname: &str) -> String {
    qname.rsplit(':').next().unwrap_or(qname).to_owned()
}

fn lat_lon(e: &BytesStart<'_>) -> Result<(f64, f64), String> {
    let mut lat = None;
    let mut lon = None;
    for attr in e.attributes().with_checks(false).flatten() {
        let key = local_name(attr.key.as_ref());
        let value = attr.value.as_ref();
        if key == "lat" {
            lat = Some(
                value
                    .parse::<f64>()
                    .map_err(|_| format!("invalid lat {value:?}"))?,
            );
        } else if key == "lon" {
            lon = Some(
                value
                    .parse::<f64>()
                    .map_err(|_| format!("invalid lon {value:?}"))?,
            );
        }
    }
    match (lat, lon) {
        (Some(lat), Some(lon)) if lat.is_finite() && lon.is_finite() => Ok((lat, lon)),
        (Some(_), Some(_)) => Err("gpx point has non-finite lat/lon".into()),
        _ => Err("gpx point is missing lat/lon".into()),
    }
}
