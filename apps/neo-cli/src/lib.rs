//! `neo-cli` 的库接口，方便测试冷启动和更新检查而不必拉起进程。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::path::Path;
use std::time::Instant;

use neo_geo::{Camera, SpatialIndex};
use neo_io::{SynthSpec, XmlReadOptions, read_gpx, read_osm_xml};
use neo_scene::{SceneBuilder, StreamingScene};
use neo_store::DataSet;

/// 打开结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenReport {
    /// 节点。
    pub nodes: usize,
    /// way。
    pub ways: usize,
    /// relation。
    pub relations: usize,
    /// 来源描述。
    pub source: String,
}

/// 场景冒烟结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneReport {
    /// 第一次构建的微秒数。
    pub build_micros: u64,
    /// 顶点数。
    pub vertices: usize,
    /// 流式缓存里的构建次数。第二次同瓦片应为 1。
    pub builds: u64,
}

/// 有更新时的提示。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateOffer {
    /// 当前版本。
    pub current: String,
    /// 清单里的版本。
    pub latest: String,
    /// 下载地址。
    pub url: String,
}

/// 按扩展名打开 OSM XML、PBF 或 GPX。
pub fn open_path(path: &Path) -> anyhow::Result<OpenReport> {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("data");
    if name.to_ascii_lowercase().ends_with(".pbf") {
        let data = neo_io::import_pbf(path)?;
        return Ok(report(&data, name));
    }
    let bytes = std::fs::read(path)?;
    open_bytes(name, &bytes)
}

/// 从字节打开 GPX 或 OSM XML。PBF 走 [`open_path`]。
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

/// 生成合成数据并计数。这是安装包冷启动冒烟用的同一条导入路径。
pub fn open_synth(nodes: usize) -> anyhow::Result<OpenReport> {
    let (data, ways, relations) = neo_io::generate(&SynthSpec::with_nodes(nodes))?;
    Ok(OpenReport {
        nodes: data.node_count(),
        ways,
        relations,
        source: format!("synth:{nodes}"),
    })
}

/// 构建一帧并确认同瓦片的第二次访问不再重建。
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

/// 比较当前版本和更新清单。清单是 `{"version":"x.y.z","url":"..."}`。
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

/// 预设覆盖率 Markdown。前面两行是当前嵌入的 npm 包版本和条数。
pub fn presets_markdown() -> String {
    let mut out = format!(
        "schema {}（{} 条）\nnsi {}（{} 条品牌建议）\n\n",
        neo_style::schema_package_version(),
        neo_style::schema_presets().len(),
        neo_style::nsi_package_version(),
        neo_style::suggestion_presets().len(),
    );
    out.push_str(&neo_style::coverage_markdown());
    out
}

/// 拼一条 TMS URL，供安装包和影像面板共用。
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
