//! # NEO 性能验证原型
//!
//! 目的（见 docs/14 与 ADR-0001 的 V 项）：在投入完整 JOSM 移植前，用**真实代码**
//! 测量关键路径，而不是靠纸面承诺。四个子命令：
//!
//! ```text
//! neo-prototype import  --pbf <file>              # 导入 + 索引，报告吞吐/内存
//! neo-prototype render  --pbf <file> [--synth N]  # 离屏 GPU 渲染 -> PNG（验证 wgpu 管线）
//! neo-prototype bench   --pbf <file> [--synth N]  # 查询/场景/局部更新/GPU 帧 的 P50/P95
//! neo-prototype gpu-info                          # 适配器信息
//! ```
//!
//! 输出既打印到 stdout，也写 `out/prototype-report.md`（可直接作为 Gate 报告素材）。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use neo_core::{LatLon, ObjectId};
use neo_geo::{Camera, HitTester, SpatialIndex, WorldBounds, WorldCache, WorldPoint};
use neo_io::{ImportOptions, SynthSpec, generate, import_pbf_parallel_with_options, import_pbf_with_report};
use neo_render::HeadlessRenderer;
use neo_scene::SceneBuilder;
use neo_store::DataSet;

mod platform;

const USAGE: &str = "\
NEO performance verification prototype

USAGE:
  neo-prototype import  --pbf <file.osm.pbf>
  neo-prototype render  --pbf <file.osm.pbf> [--width 1600] [--height 1000] [--out out/map.png]
                        [--zoom <z>] [--lat <deg>] [--lon <deg>]
  neo-prototype render  --synth <n> [--width ..] [--height ..] [--out ..]
  neo-prototype bench   --pbf <file.osm.pbf> [--iterations 30]
  neo-prototype bench   --synth <n> [--iterations 30]
  neo-prototype pbf-roundtrip --pbf <in.osm.pbf> [--out out/roundtrip.pbf]
neo-prototype pbf-roundtrip --synth <n> [--out out/roundtrip.pbf]
neo-prototype compare-import --pbf <in.osm.pbf>   # 顺序 vs 并行解码
  neo-prototype gpu-info
";

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("import") => run_import(&args[1..]),
        Some("render") => run_render(&args[1..]),
        Some("bench") => run_bench(&args[1..]),
        Some("pbf-roundtrip") => run_pbf_roundtrip(&args[1..]),
        Some("compare-import") => run_compare_import(&args[1..]),
        Some("gpu-info") => run_gpu_info(),
        Some("help") | Some("--help") | Some("-h") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => {
            print!("{USAGE}");
            bail!("unknown command: {other}")
        }
    }
}

// ---------------------------------------------------------------------------
// CLI 解析（够用即可，不引 clap）
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Opts {
    pbf: Option<PathBuf>,
    synth: Option<usize>,
    width: Option<u32>,
    height: Option<u32>,
    out: Option<PathBuf>,
    zoom: Option<f64>,
    lat: Option<f64>,
    lon: Option<f64>,
    iterations: Option<usize>,
}

fn parse_opts(args: &[String]) -> Result<Opts> {
    let mut o = Opts::default();
    let mut i = 0;
    while i < args.len() {
        let key = args[i].as_str();
        let mut value = || -> Result<String> {
            i += 1;
            if i >= args.len() {
                bail!("missing value for {key}");
            }
            Ok(args[i].clone())
        };
        match key {
            "--pbf" => o.pbf = Some(PathBuf::from(value()?)),
            "--synth" => o.synth = Some(value()?.parse().context("--synth expects a number")?),
            "--width" => o.width = Some(value()?.parse().context("--width expects u32")?),
            "--height" => o.height = Some(value()?.parse().context("--height expects u32")?),
            "--out" => o.out = Some(PathBuf::from(value()?)),
            "--zoom" => o.zoom = Some(value()?.parse().context("--zoom expects f64")?),
            "--lat" => o.lat = Some(value()?.parse().context("--lat expects f64")?),
            "--lon" => o.lon = Some(value()?.parse().context("--lon expects f64")?),
            "--iterations" => o.iterations = Some(value()?.parse().context("--iterations expects a number")?),
            other => bail!("unknown option: {other}"),
        }
        i += 1;
    }
    if o.pbf.is_none() && o.synth.is_none() {
        bail!("either --pbf <file> or --synth <n> is required");
    }
    Ok(o)
}

// ---------------------------------------------------------------------------
// 数据装载
// ---------------------------------------------------------------------------

struct Loaded {
    data: DataSet,
    index: SpatialIndex,
    referrers: neo_store::Referrers,
    world_cache: WorldCache,
    label: String,
    source_bytes: u64,
    import_micros: u64,
    index_micros: u64,
    referrers_micros: u64,
}

impl Loaded {
    fn bounds(&self) -> WorldBounds {
        let mut b = WorldBounds::EMPTY;
        for node in self.data.iter_nodes() {
            b.expand_point(WorldPoint::from_latlon(node.pos));
        }
        b
    }
}

fn load(opts: &Opts) -> Result<Loaded> {
    let started = Instant::now();
    let (data, label, source_bytes, import_micros) = if let Some(path) = &opts.pbf {
        let started = Instant::now();
        let (data, report) =
            import_pbf_with_report(path).with_context(|| format!("failed to import {}", path.display()))?;
        println!("{report}");
        (
            data,
            format!("pbf:{}", path.display()),
            report.input_bytes,
            started.elapsed().as_micros() as u64,
        )
    } else {
        let n = opts.synth.context("missing --synth")?;
        let started = Instant::now();
        let (data, ways, relations) =
            generate(&SynthSpec::with_nodes(n)).context("failed to generate synthetic dataset")?;
        let micros = started.elapsed().as_micros() as u64;
        println!(
            "synthetic dataset: {} nodes, {} ways, {} relations generated in {:.2}s",
            data.node_count(),
            ways,
            relations,
            micros as f64 / 1e6
        );
        (data, format!("synth:{n}"), 0, micros)
    };

    let t = Instant::now();
    let index = SpatialIndex::build(&data);
    let index_micros = t.elapsed().as_micros() as u64;

    let t = Instant::now();
    let referrers = data.build_referrers();
    let referrers_micros = t.elapsed().as_micros() as u64;

    let _ = started;
    Ok(Loaded {
        world_cache: WorldCache::build(&data),
        data,
        index,
        referrers,
        label,
        source_bytes,
        import_micros,
        index_micros,
        referrers_micros,
    })
}

// ---------------------------------------------------------------------------
// import
// ---------------------------------------------------------------------------

fn run_import(args: &[String]) -> Result<()> {
    let opts = parse_opts(args)?;
    let loaded = load(&opts)?;
    let stats = loaded.data.stats();
    println!("\n=== dataset ===");
    println!("{stats}");
    println!(
        "index: {} entries in {:.1}ms | referrers: {:.1}ms | mem est: {}",
        loaded.index.len(),
        loaded.index_micros as f64 / 1000.0,
        loaded.referrers_micros as f64 / 1000.0,
        stats.bytes
    );
    if loaded.source_bytes > 0 {
        println!(
            "source: {:.2} MiB, import {:.2}s => {:.1} MiB/s",
            loaded.source_bytes as f64 / (1024.0 * 1024.0),
            loaded.import_micros as f64 / 1e6,
            loaded.source_bytes as f64 / (1024.0 * 1024.0) / (loaded.import_micros as f64 / 1e6)
        );
    }
    println!(
        "per-object estimate: {:.0} B/object",
        stats.bytes.bytes() as f64 / (stats.nodes + stats.ways + stats.relations).max(1) as f64
    );
    if let Some(peak) = platform::peak_working_set_mib() {
        println!("peak working set: {peak:.1} MiB");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// render
// ---------------------------------------------------------------------------

fn run_render(args: &[String]) -> Result<()> {
    let opts = parse_opts(args)?;
    let width = opts.width.unwrap_or(1600);
    let height = opts.height.unwrap_or(1000);
    let out = opts
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("out/neo-render.png"));

    let mut loaded = load(&opts)?;
    let bounds = loaded.bounds();
    let mut camera = Camera::new(bounds.center(), 14.0, [width, height]);
    camera.fit_bounds(&bounds, 40.0);
    if let Some(z) = opts.zoom {
        camera.set_zoom(z);
    }
    if let (Some(lat), Some(lon)) = (opts.lat, opts.lon) {
        camera.center = WorldPoint::from_latlon(LatLon::from_degrees(lat, lon));
    }

    let builder = SceneBuilder::with_dataset(&mut loaded.data);
    let t = Instant::now();
    let scene = builder.build(
        &loaded.data,
        &loaded.index,
        &loaded.referrers,
        &loaded.world_cache,
        &camera,
    );
    let scene_micros = t.elapsed().as_micros();
    println!("\n=== scene ===");
    println!(
        "camera: center=({:.1},{:.1}) zoom={:.2} viewport={}x{}",
        camera.center.x, camera.center.y, camera.zoom, width, height
    );
    println!("scene: {}", scene.stats);
    println!("scene build: {scene_micros} us");

    let mut renderer = HeadlessRenderer::new(width, height).context("failed to create headless renderer")?;
    println!("gpu: {}", renderer.ctx.describe());

    let timing = renderer
        .render_encode_submit(&scene, &camera, [24, 26, 30, 255])
        .context("GPU frame failed")?;
    println!(
        "gpu frame: {:.2}ms total (upload {:.2}ms, {} bytes, {} draw calls, {} triangles)",
        timing.total_micros as f64 / 1000.0,
        timing.upload_micros as f64 / 1000.0,
        timing.uploaded_bytes,
        timing.draw_calls,
        timing.triangles
    );

    let t = Instant::now();
    let image = renderer
        .render_to_png(&scene, &camera, [24, 26, 30, 255], &out)
        .with_context(|| format!("failed to render PNG to {}", out.display()))?;
    let total_micros = t.elapsed().as_micros();
    println!(
        "png: {}x{} -> {} (render+readback+encode {:.1}ms)",
        image.width,
        image.height,
        out.display(),
        total_micros as f64 / 1000.0
    );
    println!(
        "non-background pixels: {}",
        count_non_background(&image.data, [24, 26, 30])
    );

    let mut report = String::new();
    report.push_str("# NEO prototype: render\n\n");
    report.push_str(&format!("- dataset: `{}`\n", loaded.label));
    report.push_str(&format!("- scene: {}\n", scene.stats));
    report.push_str(&format!("- gpu: {}\n", renderer.ctx.describe()));
    report.push_str(&format!(
        "- gpu frame: {:.2} ms, upload {:.2} ms / {} bytes, {} draw calls, {} triangles\n",
        timing.total_micros as f64 / 1000.0,
        timing.upload_micros as f64 / 1000.0,
        timing.uploaded_bytes,
        timing.draw_calls,
        timing.triangles
    ));
    report.push_str(&format!("- output: `{}`\n", out.display()));
    write_report("render", &report)?;
    Ok(())
}

fn count_non_background(rgba: &[u8], bg: [u8; 3]) -> usize {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] != bg[0] || p[1] != bg[1] || p[2] != bg[2])
        .count()
}

// ---------------------------------------------------------------------------
// bench
// ---------------------------------------------------------------------------

struct Samples {
    label: &'static str,
    unit: &'static str,
    values: Vec<f64>,
}

impl Samples {
    fn new(label: &'static str, unit: &'static str) -> Self {
        Self {
            label,
            unit,
            values: Vec::new(),
        }
    }

    fn push(&mut self, v: f64) {
        self.values.push(v);
    }

    fn report(&self) -> String {
        let mut v = self.values.clone();
        v.sort_by(|a, b| a.total_cmp(b));
        let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
        format!(
            "| {} | {} | {:.3} | {:.3} | {:.3} | {:.3} |",
            self.label,
            self.unit,
            mean,
            percentile(&v, 0.50),
            percentile(&v, 0.95),
            percentile(&v, 0.99)
        )
    }
}

fn percentile(sorted_or_unsorted: &[f64], p: f64) -> f64 {
    if sorted_or_unsorted.is_empty() {
        return 0.0;
    }
    let mut v = sorted_or_unsorted.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let idx = ((v.len() - 1) as f64 * p).round() as usize;
    v[idx.min(v.len() - 1)]
}

fn run_bench(args: &[String]) -> Result<()> {
    let opts = parse_opts(args)?;
    let iterations = opts.iterations.unwrap_or(30);
    let mut loaded = load(&opts)?;
    let stats = loaded.data.stats();
    println!("\n=== dataset ===");
    println!("{stats}");
    println!(
        "index={} entries | import={:.1}ms index={:.1}ms referrers={:.1}ms",
        loaded.index.len(),
        loaded.import_micros as f64 / 1000.0,
        loaded.index_micros as f64 / 1000.0,
        loaded.referrers_micros as f64 / 1000.0
    );

    let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);
    let bounds = loaded.bounds();
    let (bw, bh) = bounds.size();
    let node_ids: Vec<ObjectId> = loaded.data.iter_nodes().map(|n| n.id).collect::<Vec<_>>();
    if node_ids.is_empty() {
        bail!("dataset has no nodes");
    }

    // --- 1. ID 查找 ---
    let mut lookup = Samples::new("id lookup", "ns/op");
    for _ in 0..200_000 {
        let id = node_ids[rng.next_usize(node_ids.len())];
        let t = Instant::now();
        let found = loaded.data.node_handle(id).is_some();
        lookup.push(t.elapsed().as_nanos() as f64);
        std::hint::black_box(found);
    }

    // --- 2. 空间查询（视口 bbox） ---
    let mut bbox = Samples::new("bbox query (1200x800 @ z16)", "µs/op");
    let mut bbox_hits = 0usize;
    for _ in 0..2000 {
        let cx = bounds.min.x + rng.next_f64() * bw;
        let cy = bounds.min.y + rng.next_f64() * bh;
        let camera = Camera::new(WorldPoint::new(cx, cy), 16.0, [1200, 800]);
        let vb = camera.visible_bounds();
        let t = Instant::now();
        let hits = loaded.index.query(vb);
        bbox.push(t.elapsed().as_micros() as f64);
        bbox_hits += hits.len();
    }

    // --- 3. 场景构建 ---
    let builder = SceneBuilder::with_dataset(&mut loaded.data);
    let center = bounds.center();
    let mut scene_camera = Camera::new(center, 16.0, [1600, 1000]);
    scene_camera.fit_bounds(&bounds, 20.0);
    if scene_camera.zoom > 18.0 {
        scene_camera.set_zoom(18.0);
    }
    let mut scene_build = Samples::new("scene build (fit-all, worst case)", "ms/frame");
    let mut scene_stats = String::new();
    for _ in 0..iterations {
        let t = Instant::now();
        let frame = builder.build(
            &loaded.data,
            &loaded.index,
            &loaded.referrers,
            &loaded.world_cache,
            &scene_camera,
        );
        scene_build.push(t.elapsed().as_secs_f64() * 1000.0);
        scene_stats = frame.stats.to_string();
        std::hint::black_box(frame.total_vertices());
    }

    // 编辑尺度的视口（z16 局部）：交互时的真实重建成本。
    let mut scene_build_z16 = Samples::new("scene build (z16 viewport)", "ms/frame");
    let mut scene_stats_z16 = String::new();
    let scene16_camera = Camera::new(bounds.center(), 16.0, [1600, 1000]);
    for _ in 0..iterations {
        let t = Instant::now();
        let frame = builder.build(
            &loaded.data,
            &loaded.index,
            &loaded.referrers,
            &loaded.world_cache,
            &scene16_camera,
        );
        scene_build_z16.push(t.elapsed().as_secs_f64() * 1000.0);
        scene_stats_z16 = frame.stats.to_string();
        std::hint::black_box(frame.total_vertices());
    }

    // --- 4. 局部更新：移动节点 + 重建关联 way 的 bbox/索引 ---
    let mut incremental = Samples::new("move node + rebuild parent ways", "µs/op");
    let mut update_count = 0usize;
    for _ in 0..iterations.max(200) {
        let node = loaded
            .data
            .iter_nodes()
            .nth(rng.next_usize(node_ids.len()))
            .unwrap();
        let parents = loaded.referrers.ways_of_node(node.handle).to_vec();
        if parents.is_empty() {
            continue;
        }
        let t = Instant::now();
        let new_pos = LatLon::new(
            neo_core::Coord::from_units(node.pos.lat.units() + 100),
            neo_core::Coord::from_units(node.pos.lon.units() + 100),
        );
        loaded.data.set_node_position(node.handle, new_pos)?;
        for parent in &parents {
            loaded.index.update_way(&loaded.data, *parent);
        }
        loaded.index.update_node(node.handle, new_pos);
        incremental.push(t.elapsed().as_micros() as f64);
        update_count += 1;
    }

    // --- 5. 命中测试 ---
    let tester = HitTester::new(&loaded.data, &loaded.index);
    let mut pick = Samples::new("pick (node/way hover)", "µs/op");
    for _ in 0..2000 {
        let screen = [rng.next_f64() as f32 * 1600.0, rng.next_f64() as f32 * 1000.0];
        let t = Instant::now();
        let hit = tester.pick(&scene_camera, screen, 6.0);
        pick.push(t.elapsed().as_micros() as f64);
        std::hint::black_box(hit.is_some());
    }

    // --- 6. GPU 帧（若可用） ---
    let mut gpu = Samples::new("gpu frame (encode+submit+wait)", "ms/frame");

    let mut gpu_desc = String::from("unavailable");
    let scene = builder.build(
        &loaded.data,
        &loaded.index,
        &loaded.referrers,
        &loaded.world_cache,
        &scene_camera,
    );
    let gpu_note: String = match HeadlessRenderer::new(1600, 1000) {
        Ok(mut renderer) => {
            gpu_desc = renderer.ctx.describe();
            for _ in 0..iterations {
                let timing = renderer.render_encode_submit(&scene, &scene_camera, [24, 26, 30, 255])?;
                gpu.push(timing.total_micros as f64 / 1000.0);
            }
            format!(
                "{} draw calls, {} triangles, upload {:.1}ms/{}B",
                scene.batches.len(),
                scene.total_triangles(),
                renderer.gpu_scene.upload_stats().micros as f64 / 1000.0,
                renderer.gpu_scene.upload_stats().bytes
            )
        }
        Err(e) => {
            format!("skipped: {e}")
        }
    };

    // --- 报告 ---
    let mut md = String::new();
    md.push_str("# NEO prototype benchmark report\n\n");
    md.push_str(&format!("- dataset: `{}`\n", loaded.label));
    md.push_str(&format!("- stats: {}\n", stats));
    md.push_str(&format!(
        "- import: {:.1} ms, index build: {:.1} ms, referrers: {:.1} ms\n",
        loaded.import_micros as f64 / 1000.0,
        loaded.index_micros as f64 / 1000.0,
        loaded.referrers_micros as f64 / 1000.0
    ));
    md.push_str(&format!(
        "- cpu threads: {}\n",
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0)
    ));
    md.push_str(&format!("- gpu: {gpu_desc}\n"));
    md.push_str(&format!("- scene (fit-all): {scene_stats}\n"));
    md.push_str(&format!("- scene (z16): {scene_stats_z16}\n"));
    if let Some(peak) = platform::peak_working_set_mib() {
        md.push_str(&format!("- peak working set: {peak:.1} MiB\n"));
    }
    md.push_str(&format!(
        "- bbox query avg hits: {:.1}\n",
        bbox_hits as f64 / 2000.0
    ));
    md.push_str(&format!(
        "- incremental updates measured: {update_count} (each: 1 node + {} parent ways avg)\n",
        if update_count > 0 { 1 } else { 0 }
    ));
    md.push_str(&format!("- gpu note: {gpu_note}\n"));
    md.push_str("\n| benchmark | unit | mean | p50 | p95 | p99 |\n|---|---|---|---|---|---|\n");
    for s in [
        &lookup,
        &bbox,
        &scene_build,
        &scene_build_z16,
        &incremental,
        &pick,
        &gpu,
    ] {
        md.push_str(&s.report());
        md.push('\n');
    }
    println!("\n===benchmark===\n{md}");
    write_report("bench", &md)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// pbf-roundtrip：PBF 写 encoder 的端到端验证
// ---------------------------------------------------------------------------

fn run_pbf_roundtrip(args: &[String]) -> Result<()> {
    let opts = parse_opts(args)?;
    let out = opts
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("out/roundtrip.pbf"));

    let (data, label) = if let Some(pbf) = &opts.pbf {
        (
            neo_io::import_pbf(pbf).context("import source pbf")?,
            pbf.display().to_string(),
        )
    } else {
        let n = opts.synth.context("either --pbf or --synth is required")?;
        let started = std::time::Instant::now();
        let (data, ways, relations) =
            generate(&SynthSpec::with_nodes(n)).context("failed to generate synthetic dataset")?;
        println!(
            "synthetic: {} nodes, {} ways, {} relations in {:.2}s",
            data.node_count(),
            ways,
            relations,
            started.elapsed().as_secs_f64()
        );
        (data, format!("synth:{n}"))
    };
    println!("source: {label}");
    let before = data.stats();
    let report =
        neo_io::write_pbf_file(&data, &out, neo_io::PbfWriteOptions::default()).context("write pbf")?;
    let back = neo_io::import_pbf(&out).context("re-import written pbf")?;
    let after = back.stats();
    let integrity = neo_store::check_integrity(&back);

    println!("{report}");
    println!("before: {before}");
    println!("after : {after}");
    println!("integrity: {integrity}");
    anyhow::ensure!(
        before.nodes == after.nodes && before.ways == after.ways && before.relations == after.relations,
        "object counts changed after PBF roundtrip"
    );
    println!("OK: PBF roundtrip preserved object counts ({})", out.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// gpu-info
// ---------------------------------------------------------------------------

fn run_compare_import(args: &[String]) -> Result<()> {
    let opts = parse_opts(args)?;
    let pbf = opts.pbf.context("--pbf is required")?;

    let t0 = std::time::Instant::now();
    let (sequential, seq_report) = import_pbf_with_report(&pbf).context("sequential import")?;
    let seq_time = t0.elapsed();

    let t1 = std::time::Instant::now();
    let (parallel, par_report) =
        import_pbf_parallel_with_options(&pbf, ImportOptions::default()).context("parallel import")?;
    let par_time = t1.elapsed();

    let seq_elems = seq_report.nodes + seq_report.ways + seq_report.relations;
    let par_elems = par_report.nodes + par_report.ways + par_report.relations;
    println!("file: {}", pbf.display());
    println!(
        "sequential: {:.3}s  {:.0} elem/s  ({} nodes, {} ways, {} relations)",
        seq_time.as_secs_f64(),
        seq_elems as f64 / seq_time.as_secs_f64(),
        sequential.node_count(),
        sequential.way_count(),
        sequential.relation_count()
    );
    println!(
        "parallel  : {:.3}s  {:.0} elem/s  ({} nodes, {} ways, {} relations)",
        par_time.as_secs_f64(),
        par_elems as f64 / par_time.as_secs_f64(),
        parallel.node_count(),
        parallel.way_count(),
        parallel.relation_count()
    );
    println!(
        "speedup   : {:.2}x",
        seq_time.as_secs_f64() / par_time.as_secs_f64()
    );
    anyhow::ensure!(
        sequential.node_count() == parallel.node_count()
            && sequential.way_count() == parallel.way_count()
            && sequential.relation_count() == parallel.relation_count(),
        "parallel import differs from sequential import"
    );
    println!("OK: parallel import equals sequential import");
    Ok(())
}

fn run_gpu_info() -> Result<()> {
    let ctx = neo_render::GpuContext::new_headless().context("no GPU adapter")?;
    println!("{}", ctx.describe());
    let limits = ctx.adapter.limits();
    println!("max_texture_dimension_2d: {}", limits.max_texture_dimension_2d);
    println!("max_buffer_size: {}", limits.max_buffer_size);
    println!(
        "max_storage_buffer_binding_size: {}",
        limits.max_storage_buffer_binding_size
    );
    println!("timestamp_query: {}", ctx.timestamp_query_supported);
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn write_report(kind: &str, body: &str) -> Result<()> {
    let dir = Path::new("out");
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("prototype-{kind}.md"));
    std::fs::write(&path, body)?;
    println!("report written to {}", path.display());
    Ok(())
}

struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() % bound as u64) as usize
    }
}

/// 让编译器不要优化掉计时循环。
#[allow(dead_code)]
fn black_box<T>(v: T) -> T {
    std::hint::black_box(v)
}

/// 标准库 Duration 的便捷格式化。
#[allow(dead_code)]
fn fmt_duration(d: Duration) -> String {
    format!("{:.3}ms", d.as_secs_f64() * 1000.0)
}
