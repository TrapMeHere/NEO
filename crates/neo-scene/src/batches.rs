//! 渲染批次与场景帧（CPU 侧，不含任何 GPU 类型）。
//!
//! 分层原则（ADR-0004）：`neo-scene` 只产出"顶点 + 索引 + 颜色"，`neo-render` 才把
//! 它变成 wgpu buffer。这样更换渲染后端不需要改场景装配，测试也不需要 GPU。

use bytemuck::{Pod, Zeroable};
use neo_core::{ObjectId, ObjectKind};
use neo_geo::{Camera, WorldPoint};

/// 地图顶点（12 字节，可直接映射到 GPU）。
///
/// `position` 是**相对批次原点（`SceneFrame::origin`）的 WebMercator 米坐标**（f32）。
/// 用相对坐标而非经纬度：经纬度在 f32 下精度不足，而相对米坐标在 4 km 重基范围内
/// 的误差远小于 1 像素（见 docs/04 §2.1）。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct MapVertex {
    /// 相对批次原点的位置（米）。
    pub position: [f32; 2],
    /// RGBA，非预乘。
    pub color: [u8; 4],
}

const _: () = assert!(
    core::mem::size_of::<MapVertex>() == 12,
    "MapVertex must stay 12 bytes to keep GPU bandwidth low"
);

impl MapVertex {
    /// 构造。
    #[inline]
    pub fn new(position: [f32; 2], color: [u8; 4]) -> Self {
        Self { position, color }
    }
}

/// 批次类型，对应渲染管线的混合/拓扑策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BatchKind {
    /// 面填充（建筑、水体）。
    #[default]
    Fill,
    /// 线描边（道路、铁路、边界）。
    Stroke,
    /// 点标记（节点）。
    Point,
}

/// 一个 draw batch：同类、同材质的三角形集合。
#[derive(Clone, Debug, Default)]
pub struct RenderBatch {
    /// 批次类型。
    pub kind: BatchKind,
    /// 顶点。
    pub vertices: Vec<MapVertex>,
    /// 三角形索引。
    pub indices: Vec<u32>,
}

impl RenderBatch {
    /// 新建空批次。
    pub fn new(kind: BatchKind) -> Self {
        Self {
            kind,
            vertices: Vec::new(),
            indices: Vec::new(),
        }
    }

    /// 预分配。
    pub fn with_capacity(kind: BatchKind, vertices: usize, indices: usize) -> Self {
        Self {
            kind,
            vertices: Vec::with_capacity(vertices),
            indices: Vec::with_capacity(indices),
        }
    }

    /// 是否为空。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// 顶点数。
    #[inline]
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// 三角形数。
    #[inline]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// 添加一个独立三角形。
    #[inline]
    pub fn push_triangle(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], color: [u8; 4]) {
        let base = self.vertices.len() as u32;
        self.vertices.push(MapVertex::new(a, color));
        self.vertices.push(MapVertex::new(b, color));
        self.vertices.push(MapVertex::new(c, color));
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }

    /// 添加一批"顶点数组 + 局部索引"的几何。
    pub fn push_indexed(&mut self, positions: &[[f32; 2]], local_indices: &[u32], color: [u8; 4]) {
        let base = self.vertices.len() as u32;
        self.vertices
            .extend(positions.iter().map(|p| MapVertex::new(*p, color)));
        self.indices.extend(local_indices.iter().map(|i| base + *i));
    }

    /// 清空（保留容量，便于复用于下一帧）。
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
    }
}

/// 没有要素色时的注记兜底色。正常情况下名称跟要素颜色走。
pub const NAME_COLOR: [u8; 4] = [34, 34, 34, 255];

/// 注记颜色。样式写了 `text-color` 就用它，否则跟要素颜色。
/// `lift` 把面状要素的线色往白色提一档，避免贴在填充上。
pub fn name_color(explicit: Option<[u8; 4]>, feature: [u8; 4], lift: bool) -> [u8; 4] {
    if let Some(color) = explicit {
        return [color[0], color[1], color[2], 255];
    }
    if !lift {
        return [feature[0], feature[1], feature[2], 255];
    }
    let mix = |channel: u8| ((u16::from(channel) * 2 + 255 * 3) / 5) as u8;
    [mix(feature[0]), mix(feature[1]), mix(feature[2]), 255]
}

/// 注记相对几何的锚法。绘制时再换成屏幕锚点。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LabelPlace {
    /// 面：字心在面积质心，水平书写。
    Center,
    /// 线：字心在长度中点，`angle` 为屏幕顺时针弧度，已翻成正向。
    Along {
        /// 可读走向，范围 (-π/2, π/2]。
        angle: f32,
    },
    /// 点：字的左侧中点在符号右侧，`gap_px` 是离开符号中心的距离。
    Right {
        /// 字的左缘到符号中心的屏幕距离。
        gap_px: f32,
    },
}

/// 场景中的文字标签（由 UI 叠加绘制，不进入 GPU 顶点批次）。
#[derive(Clone, Debug, PartialEq)]
pub struct SceneLabel {
    /// 世界锚点。面和线是字心，点是符号中心。
    pub pos: WorldPoint,
    /// 相对几何的摆放。
    pub place: LabelPlace,
    /// 文本。
    pub text: String,
    /// RGBA 颜色。
    pub color: [u8; 4],
    /// 字号（屏幕点）。
    pub size_px: f32,
    /// 越大越优先占住位置。面用世界面积，线和点用较小的常数。
    pub rank: f32,
    /// 自动换行宽度（屏幕点）。
    pub wrap_px: f32,
}

/// 场景中的图标（由 UI 叠加绘制）。
#[derive(Clone, Debug, PartialEq)]
pub struct SceneIcon {
    /// 世界坐标。
    pub pos: WorldPoint,
    /// 相对 `assets/` 的图标路径。
    pub image: String,
    /// 屏幕宽度（点）。
    pub width_px: f32,
    /// 图标颜色。图片图标忽略它，`poi:` 图标用它填底。
    pub color: [u8; 4],
}

/// 一个面的填充。够大时是贴边内圈，屏幕上放不下内圈时是整面实心。
#[derive(Clone, Debug)]
pub struct AreaBand {
    /// 面是闭合 way，或 multipolygon 关系。
    pub kind: ObjectKind,
    /// 对象 ID。
    pub id: ObjectId,
    /// 外环（世界坐标）。
    pub outer: Vec<WorldPoint>,
    /// 洞。
    pub holes: Vec<Vec<WorldPoint>>,
    /// 从边界向内的起点（米）。内圈外缘贴着面，这里是 0。
    pub inset_m: f64,
    /// 圈宽（米）。实心填充时不使用。
    pub width_m: f64,
    /// 当前缩放下这个面放不下一圈内边，画成实心，点击整个内部都算选中。
    pub solid: bool,
}

/// 一帧场景。
///
/// `version` 与 `DataSet::version()` 对应；相机平移/缩放**不需要重建** `SceneFrame`
/// （GPU 用 uniform 变换），只有在数据变化或需要原点重基时才重建。
#[derive(Clone, Debug)]
pub struct SceneFrame {
    /// 构建自的数据版本。
    pub version: u64,
    /// 构建时的相机（渲染时以当前相机 uniform 覆盖平移/缩放）。
    pub camera: Camera,
    /// 顶点坐标的原点（世界坐标米）。
    pub origin: WorldPoint,
    /// 批次列表。
    pub batches: Vec<RenderBatch>,
    /// 文字标签。
    pub labels: Vec<SceneLabel>,
    /// 图标。
    pub icons: Vec<SceneIcon>,
    /// 面的内圈，供点击命中。几何已经画进 `batches`。
    pub area_bands: Vec<AreaBand>,
    /// 统计。
    pub stats: SceneStats,
}

impl SceneFrame {
    /// 空场景。
    pub fn empty(version: u64, camera: Camera) -> Self {
        Self {
            version,
            origin: camera.center,
            camera,
            batches: Vec::new(),
            labels: Vec::new(),
            icons: Vec::new(),
            area_bands: Vec::new(),
            stats: SceneStats::default(),
        }
    }

    /// 总顶点数。
    pub fn total_vertices(&self) -> usize {
        self.batches.iter().map(|b| b.vertices.len()).sum()
    }

    /// 总三角形数。
    pub fn total_triangles(&self) -> usize {
        self.batches.iter().map(|b| b.indices.len() / 3).sum()
    }

    /// 相机是否需要原点重基。
    pub fn needs_rebase(&self, camera: &Camera) -> bool {
        camera.needs_rebase(self.origin)
    }

    /// 场景是否为空。
    pub fn is_empty(&self) -> bool {
        self.batches.iter().all(|b| b.is_empty())
    }
}

/// 场景构建统计（用于状态栏与性能基准）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneStats {
    /// 实际绘制的 way 数。
    pub ways: usize,
    /// 实际绘制的节点数。
    pub nodes: usize,
    /// 被 LOD/裁剪跳过的 way 数。
    pub skipped: usize,
    /// 顶点总数。
    pub vertices: usize,
    /// 三角形总数。
    pub triangles: usize,
    /// 构建耗时（微秒）。
    pub build_micros: u64,
    /// 是否因为顶点上限被截断。
    pub clamped: bool,
}

impl core::fmt::Display for SceneStats {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "ways={} nodes={} skipped={} verts={} tris={} build={}us{}",
            self.ways,
            self.nodes,
            self.skipped,
            self.vertices,
            self.triangles,
            self.build_micros,
            if self.clamped { " CLAMPED" } else { "" }
        )
    }
}
