//! Library interface of `neo-cli`, so cold-start and update checks can be tested without spawning the process.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::path::Path;
use std::time::Instant;

use neo_geo::{Camera, SpatialIndex};
use neo_io::{SynthSpec, XmlReadOptions, read_gpx, read_osm_xml};
use neo_scene::{SceneBuilder, StreamingScene};
use neo_store::DataSet;

/// Result of opening a data source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenReport {
    /// Nodes.
    pub nodes: usize,
    /// Ways.
    pub ways: usize,
    /// Relations.
    pub relations: usize,
    /// Source description.
    pub source: String,
}

/// Scene smoke-test result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneReport {
    /// Microseconds of the first build.
    pub build_micros: u64,
    /// Vertex count.
    pub vertices: usize,
    /// Build count in the streaming cache. A second access to the same tile should be 1.
    pub builds: u64,
}

/// Notification when an update is available.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateOffer {
    /// Current version.
    pub current: String,
    /// Version from the manifest.
    pub latest: String,
    /// Download URL.
    pub url: String,
}

/// Opens OSM XML, PBF, or GPX by file extension.
pub fn open_path(path: &Path) -> anyhow::Result<OpenReport> {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("data");
    if name.to_ascii_lowercase().ends_with(".pbf") {
        let data = neo_io::import_pbf(path)?;
        return Ok(report(&data, name));
    }
    let bytes = std::fs::read(path)?;
    open_bytes(name, &bytes)
}

/// Opens GPX or OSM XML from bytes. PBF goes through [`open_path`].
pub fn open_bytes(name: &str, bytes: &[u8]) -> anyhow::Result<OpenReport> {
    let mut data = DataSet::new();
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".pbf") {
        anyhow::bail!("pbf must be opened from a path");
    } else if lower.ends_with(".gpx") {
        read_gpx(&String::from_utf8_lossy(bytes), &mut data)?;
    } else {
        read_osm_xml(&mut data, std::io::Cursor::new(bytes), XmlReadOptions::default())?;
    }
    Ok(report(&data, name))
}

/// Generates synthetic data and counts it. This is the same import path used by the installer cold-start smoke test.
pub fn open_synth(nodes: usize) -> anyhow::Result<OpenReport> {
    let (data, ways, relations) = neo_io::generate(&SynthSpec::with_nodes(nodes))?;
    Ok(OpenReport {
        nodes: data.node_count(),
        ways,
        relations,
        source: format!("synth:{nodes}"),
    })
}

/// Builds one frame and confirms that a second access to the same tile does not rebuild.
pub fn scene_bench(nodes: usize) -> anyhow::Result<SceneReport> {
    let (mut data, _, _) = neo_io::generate(&SynthSpec::with_nodes(nodes))?;
    let builder = SceneBuilder::with_dataset(&mut data);
    let index = SpatialIndex::build(&data);
    let referrers = data.build_referrers();
    let world_cache = neo_geo::WorldCache::build(&data);
    let camera = Camera::from_latlon(neo_core::LatLon::from_degrees(47.3769, 8.5417), 16.0, [800, 600]);
    let mut streaming = StreamingScene::new(8);
    let started = Instant::now();
    let frame = streaming.frame(&builder, &data, &index, &referrers, &world_cache, &camera);
    let build_micros = started.elapsed().as_micros() as u64;
    let _ = streaming.frame(&builder, &data, &index, &referrers, &world_cache, &camera);
    Ok(SceneReport {
        build_micros,
        vertices: frame.total_vertices(),
        builds: streaming.builds,
    })
}

/// Compares the current version with the update manifest. The manifest is `{"version":"x.y.z","url":"..."}`.
pub fn check_update(current: &str, manifest_json: &str) -> anyhow::Result<Option<UpdateOffer>> {
    let value: serde_json::Value = serde_json::from_str(manifest_json)?;
    let latest = value
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("update manifest has no version"))?
        .to_owned();
    let url = value.get("url").and_then(|v| v.as_str()).unwrap_or("").to_owned();
    if version_tuple(&latest) > version_tuple(current) {
        Ok(Some(UpdateOffer {
            current: current.to_owned(),
            latest,
            url,
        }))
    } else {
        Ok(None)
    }
}

/// Preset coverage Markdown. The first two lines are the versions and counts of the currently embedded npm packages.
pub fn presets_markdown() -> String {
    let mut out = format!(
        "schema {} ({} entries)\nnsi {} ({} brand suggestions)\n\n",
        neo_style::schema_package_version(),
        neo_style::schema_presets().len(),
        neo_style::nsi_package_version(),
        neo_style::suggestion_presets().len(),
    );
    out.push_str(&neo_style::coverage_markdown());
    out
}

/// Builds one TMS URL, shared by the installer and the imagery panel.
pub fn imagery_example(z: u32, x: u32, y: u32) -> String {
    neo_style::tms_url("https://tile.openstreetmap.org/{z}/{x}/{y}.png", z, x, y)
}

fn report(data: &DataSet, source: &str) -> OpenReport {
    OpenReport {
        nodes: data.node_count(),
        ways: data.way_count(),
        relations: data.relation_count(),
        source: source.to_owned(),
    }
}

fn version_tuple(text: &str) -> (u64, u64, u64) {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}
