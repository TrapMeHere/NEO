# 04 空间索引与几何构建

> 状态：草案 v0.1 ｜ 日期：2026-11-15 ｜ 归属 crate：`neo-geo`、`neo-scene`
> 相关：[01-system-architecture.md](01-system-architecture.md) ｜ [02-workspace-and-dependency-rules.md](02-workspace-and-dependency-rules.md) ｜ [05-renderer-wgpu.md](05-renderer-wgpu.md) ｜ [11-style-presets-mapcss.md](11-style-presets-mapcss.md) ｜ [12-validation.md](12-validation.md) ｜ [ADR-0005](adr/0005-wgpu-egui-shared-device.md) ｜ [14-performance-and-verification.md](14-performance-and-verification.md)

## 摘要

- NEO 的几何层分两段：`neo-geo`（投影、几何生成、索引、拾取，**无 GPU 依赖**）与 `neo-scene`（可见集、LOD、批次装配 `SceneFrame`/`MapVertex`，**无 GPU 依赖**）。
- 坐标栈四级：`LonLat`（WGS84 度）→ `WorldPoint`（投影平面，默认 EPSG:3857 米，f64）→ **重定基**（f64→f32，原点对齐 2^16 m 网格）→ `Screen`（物理像素）。
- 精度策略是「f64 存、f32 画、按视口重定基」：f32 在世界极值 ±2.0e7 m 处的 ulp 约 2 m，会把 19 级缩放下的一栋楼抖动到十几个像素；重定基到 ≤1e5 m 后误差降到毫米级。
- 绘制量由**视口**决定而不是由总对象数决定：对象 R-tree 只负责 `O(log n + k)` 出候选，之后全部由可见集、LOD 与批次控制，1M 与 10M 对象在同一视口下工作量同阶。
- 索引双层：**对象层**（rstar 0.14 `RTree<ObjEntry>`，Node 点 / Way 预计算 AABB / Relation 派生 bbox）+ **视口层**（屏幕空间 256 px 分桶，随可见集重建，服务 hover 与精确拾取）。
- 索引与几何都用 NEO 自有类型实现 `RTreeObject`/`PointDistance`；**不启用 geo 0.33.1 的 rstar 互操作**（见 §2.8、§5）。
- 面几何走 earcutr 0.5.0（平坦填充的正确选择），失败不 panic：降级为描边并交给 Validator 报告自相交。
- 命中有 CPU 与 GPU 两条方案，默认 CPU（已有索引），GPU ID buffer 作为密集区/多边形内部的补充，默认关闭（见 [05-renderer-wgpu.md](05-renderer-wgpu.md) §2.10）。

## 1 目标与范围

### 1.1 目标

1. 在 1M–10M+ OSM 对象的全量数据层上，视口内几何生成与拾取不随总量线性劣化。
2. 完整覆盖 JOSM 的工作流所需的几何语义：闭合判定、multipolygon/内环、`area=no`、未闭合 multipolygon、relation 成员顺序无关的 bbox。
3. 支持多投影（默认 EPSG:3857，可切 EPSG:4326 与 proj 0.32 提供的其它 CRS），且投影切换不重建索引。
4. 拾取行为与 JOSM 一致：node 优先于 way、way 优先于 area；容差以屏幕像素表达并随 zoom 变化；被 Filter 禁用的对象不可拾取（语义在 [11](11-style-presets-mapcss.md) §2.11）。
5. 为 `neo-render` 提供**已量化、可直接上传**的顶点/索引批次；渲染层不得回溯查询数据模型。

### 1.2 非目标

- 不做拓扑编辑与命令合并（属 `neo-editor`/`neo-store`），几何层只消费不可变的 `WayView`/`RelationView` 快照。
- 不做样式语义（MapCSS 求值属 `neo-style`），几何层只接收「已解析出的绘制参数」：宽度、颜色、虚线模式、是否填充。
- 不做法线贴图、挤出建模、真实 3D 建筑（Phase 2 议题，见 [05](05-renderer-wgpu.md) §2.7）。

### 1.3 分层与依赖方向

| crate / 模块 | 职责 | 允许依赖 | 明确禁止 |
|---|---|---|---|
| `neo-geo::projection` | 投影、瓦片、相机、坐标变换 | `proj` 0.32、`geo` 0.33.1（仅几何算法） | 任何 GPU/egui 类型 |
| `neo-geo::geometry` | polyline/面构造、抽稀、三角化、法线 | `earcutr` 0.5、`geo` 0.33.1 | 索引、样式 |
| `neo-geo::index` | 对象 R-tree、段索引、脏队列 | `rstar` 0.14 | `neo-store`（只收快照） |
| `neo-geo::hit` | 命中测试与优先级 | 上述三者 | GPU |
| `neo-scene::visible` | 可见集、LOD、屏幕空间分桶 | `neo-geo`、`neo-style`（只读规则结果） | wgpu |
| `neo-scene::frame` | `SceneFrame`/`RenderBatch`/`MapVertex` | `bytemuck` 1.25 | wgpu |

> 依赖方向单向：`store → editor → geo → scene → render`。`neo-render` 不得依赖 `neo-store`/`neo-editor`/`neo-ui`（见 [ADR-0005](adr/0005-wgpu-egui-shared-device.md)）。

## 2 设计

### 2.1 坐标栈

| 层 | 类型 | 单位 | 精度要求 | 负责方 | 生命周期 |
|---|---|---|---|---|---|
| L0 `LonLat` | `{lon: f64, lat: f64}` | 度（WGS84） | 存储层用 E7 定点（i32，1e-7°≈1.1 cm），与 PBF `granularity=100` 位对齐 | `neo-osm` | 永久 |
| L1 `WorldPoint` | `{x: f64, y: f64}` | 投影平面单位（3857 为米） | f64，误差 <1e-9 m | `neo-geo::projection` | 缓存 |
| L2 `Rebased` | `[f32; 2]` | 米，相对 `rebase_origin` | 重定基后 ≤1e-2 m | `neo-scene::frame` | 按 `version` |
| L3 `Screen` | `[f32; 2]` | 物理像素 | 0.5 px | `neo-render`（顶点着色器仿射） | 每帧 |

`MapVertex.pos` 承载 **L2**，不是 L3。CPU 完成投影与重定基（几何相关，只在 `version` 变化时重算），GPU 用 uniform 里的 scale/offset 完成每帧仿射。这是「CPU 做坐标变换、GPU 做批量合成」的具体切分：不是把几何搬上 GPU，也不是每帧重写顶点。

### 2.2 WebMercator 与定点精度

```
正向（R = 6378137）:
  x = R * λ                                   λ 为弧度
  y = R * asinh(tan φ) = R * ln(tan(π/4 + φ/2))
反向:
  φ = 2 * atan(exp(y / R)) - π/2
  λ = x / R
世界范围: ±20037508.342789244 m（即 ±R*π），纬度有效域 ±85.05112878°
```

精度预算（这是选型依据，不是实测性能）：

| 量 | f32 可用精度 | 结论 |
|---|---|---|
| 世界极值 2.0e7 m 处 ulp | 2^(24-23) = **2 m** | 不可直接存世界坐标 |
| 重定基后 1.0e5 m 处 ulp | 2^(16-23) ≈ **7.8 mm** | 满足 19 级（约 0.18 地面米/px） |
| f64 在 2.0e7 m 处 ulp | ≈3.7e-9 m | 投影/索引层足够 |

重定基规则：`rebase_origin = round(viewport_center / 65536 m) * 65536 m`。相机在同一个 64 km 网格内平移时 `MapVertex` 零改动；跨格才 `version += 1` 并重写顶点。这样「拖地图」在绝大多数帧里只更新一个 uniform，而不是重传顶点。

### 2.3 多投影与 proj 0.32

JOSM 支持 EPSG:3857（默认）、EPSG:4326、UTM 分区、Gauss-Krüger、Swiss LV95/LV03 等，且带一套自维护的 EPSG 定义库与按需下载的 NTv2 `.gsb` 网格。NEO 的对应策略：

| 投影族 | 用途 | 实现 | 备注 |
|---|---|---|---|
| WebMercator (3857) | 默认渲染、瓦片影像 | 手写（§2.2） | 无外部依赖、可离线；保角 |
| Geographic (4326) | 经纬度显示、调试、`project` 子命令 | 恒等 + 度→米装饰 | 非保角，线宽需按 `scale_factor` 补偿 |
| 其它 EPSG（UTM/GK/LV95/…） | JOSM 迁移用户、国家坐标系 | `proj` 0.32 + PROJ 运行时 | CRS 字符串来自配置，见下 |

```rust
// neo-geo/src/projection/proj_bridge.rs（签名以 proj 0.32 文档为准）
let ctx = proj::Proj::new_known_crs("EPSG:4326", "EPSG:32633", None)?; // > 待验证：proj 0.32 具体构造函数名
let p: (f64, f64) = ctx.convert((lon, lat))?;
```

- **PROJ 运行时**（libproj / proj.db）是外部资产，随安装包分发而不是编进二进制；缺失时降级为「仅 3857/4326」并把投影选择项灰显，绝不 panic。
- NTv2 `.gsb` 网格有独立使用条款（JOSM 侧同样按需下载），NEO 默认不下载、不打包，只提示用户手动放置。**> 待验证**：`.gsb` 的再分发条款与 PROJ 默认查找路径。
- 投影切换不重建索引：索引存 `WorldPoint`（当前投影），换投影时后台按新投影重算 AABB，用双缓冲交换，期间旧模型继续渲染（见 §2.8 重建策略）。

### 2.4 相机、缩放与瓦片（`z/x/y`）

```rust
世界宽度 W = 40075016.685578488 m
px_per_world_m = 2^zoom * 256.0 / W              // 256 px 瓦片基准
world_m_per_px = W / (2^zoom * 256.0)
ground_m_per_px = world_m_per_px * cos(lat)      // Mercator 比例因子 1/cos φ 的反向
瓦片 z/x/y: n = 2^z; x = floor((lon+180)/360 * n)
      y = floor((1 - asinh(tan φ)/π) / 2 * n)
```

- `zoom` 是 f64 连续量（滚轮可停在小数），只在跨越 **整数级 ±0.01** 时切 LOD（§2.9），避免滚动时抖动式重算。
- 视口查询用 `view_bounds()`：屏幕矩形反投影成投影平面 AABB，再按 **1 个瓦片（≤256 m 或当前 LOD 阈值）外扩**，抵消屏幕空间线宽与标签的外溢。
- 瓦片编号仅用于影像层与 `bounds` 对齐；矢量对象不按瓦片切分（对象是跨瓦片的），这点与栅格地图库不同。

### 2.5 Way → polyline

1. **取引用**：从 `store` 拿成员 node 快照（缺失节点 → 记 `incomplete`，polyline 断开成多段，不猜）。
2. **投影**：逐点 `to_world`，写入 `Vec<[f64;2]>`；对 3857 走手写快路径，其它投影走 proj。
3. **抽稀（屏幕空间）**：Douglas–Peucker，epsilon = `0.5 px * world_m_per_px`，在**投影平面**上做（不在经纬度上做，否则中纬度误差被 cos φ 放大）。抽稀结果与 zoom 绑定：LOD 档位变化才重抽。
4. **闭合判定**：`closed = pts.len() >= 4 && first == last`（按索引 1e-9 比较，不按绝对距离）；闭合 **不等于** 面：还需 `area != 0`、`area=no` 未置位、且不是 `natural=coastline` 之类的线性特征。JOSM 的 MapCSS `area` 选择器甚至匹配未闭合 way，语义差异清单见 [11](11-style-presets-mapcss.md) §2.5。
5. **弧长与虚线相位**：预计算 `cum_len`，虚线在片元着色器里按弧长取模，避免 CPU 切段。
6. **节流**：几何重建按**帧预算**切块——每次最多处理 `min(dirty.len(), 2000)` 个对象，剩余留到下一帧；`dirty` 集合按 `ObjId` 去重（拖动共享节点会一次产生上百个 Way 脏标记）。

### 2.6 multipolygon 组装、内环与三角化

```
relation(type=multipolygon|boundary)
   │ 按 role 分类: outer / inner / (空 role 视为 outer, 与 JOSM 一致)
   ▼
环拼接 (ring assembly): 首尾相接的 way 链 → 闭合环
   │ 未闭合 → :unclosed_multipolygon 语义, 只描边不填充
   ▼
嵌套深度 (nesting depth): 环之间的包含次数 → 偶数=外环, 奇数=内环
   │ (支持 relation 成员里嵌套 relation: 递归展开, 深度上限 8 防环)
   ▼
earcutr: 平铺 [outer ring, hole0, hole1, …] + hole_indices
   │ dims = 2, 环必须闭合、不自交
   ▼
AreaGeom { verts, tris: Vec<u32>, rings, ok }
```

```rust
// neo-geo/src/geometry/tess.rs
let mut verts: Vec<f64> = Vec::new();     // 扁平 [x0,y0, x1,y1, …]
let mut holes: Vec<usize> = Vec::new();   // 每个孔环起始的**顶点下标**（不是坐标下标）
verts.extend_from_slice(outer_flat);
for hole in holes_flat { holes.push(verts.len() / 2); verts.extend_from_slice(hole); }
let idx: Vec<usize> = earcutr::earcut(&verts, &holes, 2)?;  // 顶点下标 → u32 索引
```

- earcutr 0.5 的 `earcut` 返回 `Result<Vec<usize>, Error>`，但**只覆盖极少失败路径**：自交环会静默产出退化三角形，所以 NEO 在调用前做 `ring_is_simple()`（`geo::Intersects` 自检，仅对面积 > 阈值且首次生成的环做，结果按 `(rel_id, version)` 缓存）。
- 失败/退化回退：`ok = false` → 该关系只走 stroke 管线（描边），并向 `neo-validate` 投递一条 `MultipolygonTest` 级别的几何问题；渲染不 panic、不留空洞色块。
- `area=no` 的闭合 way 走 polyline 而非 fill；roundabout 是线不是面，别把「闭合」当「面积」。
- 环方向（顺时针/逆时针）**不做归一化后写回数据**：方向是 OSM 数据的语义（`:clockwise`/`:anticlockwise` 伪类需要它），只在三角化入口按需翻转副本。

### 2.7 线宽扩展与屏幕空间抗锯齿

- `MapVertex` 携带 `normal`（段方向的法线，投影平面单位向量）、`across ∈ {-1, +1}`、`half_width`（**像素**）。顶点着色器：`pos_px = affine(pos_world) + normal * half_width * across`，于是线宽与 zoom 无关，GPU 侧只做一次乘加。
- 连接（join）：miter（默认，限 `miter_limit = 2.0`，超限退 bevel）/ bevel / round。round join 由「端点补一个半径 = half_width 的圆片」实现，不生成扇面几何。
- 转折处抗锯齿：片元着色器用 `abs(across) - 1.0 + 1.0/px` 的解析覆盖率做 alpha 衰减（1 px 过渡带）。因此线段**不依赖 MSAA**；MSAA 只用于面与图标的斜边（见 [05](05-renderer-wgpu.md) §2.7）。
- casing（道路描边）不是一条「更宽的线 + 一条窄线叠画」那么简单：它要两趟绘制且 z 序相邻，因此 `RenderBatch` 显式带 `casing_of` 关系，由 `neo-scene` 保证相邻批次，避免被别的对象的 z-index 插队。

### 2.8 双层空间索引

**对象层**（`rstar::RTree<ObjEntry>`，NEO 自有类型，见下）：

| 对象 | envelope | 说明 |
|---|---|---|
| Node | `AABB::from_point([x, y])` | 单点；聚簇（同一坐标多点）不合并，由渲染层合批 |
| Way | 预计算 AABB（顶点包围盒） | 解析时算一次，节点移动后重算并 `upsert` |
| Relation | 成员 bbox 的并集（**派生、缓存**） | 成员增删/移动 → 失效重算；嵌套 relation 递归但带上限 |
| Segment | 每段一条 `AABB::from_corners` | 独立段树，仅用于线拾取（`PointDistance` 段距离） |

```rust
impl rstar::RTreeObject for ObjEntry {
    type Envelope = rstar::AABB<[f64; 2]>;
    fn envelope(&self) -> Self::Envelope { self.env }
}
impl rstar::PointDistance for SegmentEntry {
    fn distance_2(&self, p: &[f64; 2]) -> f64 { /* 夹紧投影到线段的平方距离 */ }
}
```

**视口层**（屏幕空间分桶，随 `VisibleSet` 重建）：

- 网格 256×256 px；每个 cell 存该区域内的 `ObjId` 列表（按绘制顺序）。
- 用途：hover 拾取的 O(1) 起步、标签避让的邻域查询、`hit_all` 的候选收窄。
- 只在 `version` 或相机变化时重建，随可见集同批次产出，不单独维护。

**bulk_load 时机与增量策略**：

| 时机 | 操作 | 说明 |
|---|---|---|
| 初次解析/导入完成 | `RTree::bulk_load(Vec<ObjEntry>)` | `O(n log n)`，比逐条 `insert` 快得多；**唯一允许的整树构建路径** |
| 单次编辑（含 undo/redo） | `remove(&old)` + `insert(new)` | 每次重平衡；脏集合按帧批量刷 |
| 下载区域合并 | 按键排序后批量 `insert` | 新 id 密集，排序后插入可减少分裂 |
| churn > 10% 或单帧脏 > 50k | 后台 `rayon` 重建 + 双缓冲 swap | 重建期间读旧树；swap 在帧边界，指针交换后旧树延迟 1 帧释放 |
| 投影切换 | 同上重建（换 CRS 后所有坐标失效） | 复用同一重建通道 |

> 关键约束：`geo` 0.33.1 通过 `geo-types` 的 `rstar_0_12` feature 绑定 **rstar 0.12**，而 NEO 用 **rstar 0.14**。因此 `neo-geo` **不启用** geo 的 rstar 互操作（`geo-types` 的 rstar feature 关闭），`geo` 只当纯几何算法库用；需要把 geo 类型喂给索引时用 newtype 适配（`struct GeoAabb(geo::Rect<f64>)` 手写 `RTreeObject`），不做全局类型别名。

### 2.9 LOD 策略

| zoom | 绘制内容 | 抽稀 epsilon | 备注 |
|---|---|---|---|
| 0–7 | `highway ∈ {motorway,trunk,primary}` 骨架、`place` 点、大面积 water/landuse | 2.0 px | 小面直接丢弃（<4 px² 不可见） |
| 8–12 | + secondary/tertiary/rail/waterway | 1.0 px | 建筑合并为 footprint 色块 |
| 13–15 | 全部 way 样式 | 0.5 px | 建筑单独填充 |
| 16–18 | 全顶点、POI 图标、名称标注 | 0（仅去重复点） | 标签开始避让 |
| ≥19 | 全细节 + 编辑手柄/节点标记（overlay） | 0 | 手柄属 UI 层，不入 `SceneFrame` |

LOD 判定集中在一处（`Lod::for_zoom`），避免样式、几何、渲染各自抄一份阈值。`VisibleStats` 暴露 `objects / vertices / batches / culled_lod` 便于验收。

### 2.10 命中测试

1. **屏幕半径 → 投影平面距离**：`r_world = radius_px / px_per_world_m`。注意这**不是**地面米：地面容差 = `r_world * cos(lat)`。JOSM 在投影空间里判距，NEO 保持一致，并在 UI 上把容差表述为「像素」而非「米」。
2. **候选**：`locate_in_envelope_intersecting(AABB::from_point(p).expanded(r_world))`。
3. **精确距离**：`PointDistance` 的段树用 `nearest_neighbor_iter_with_distance_2` 排序；node 用欧氏距离。
4. **优先级**（同距离时，与 JOSM 心智一致）：`node` > `way`（段） > `closed way` 面积内部 > `relation` 成员。面积内部命中只在没有更近的线/点、且该样式允许拾取面积时生效。
5. **多边形内部**：`geo` 的 `PreparedGeometry`（`algorithm::indexed::prepared_geometry`）一次构建多次查询，比每次 `Contains` 快；仍先用对象 AABB 粗筛。
6. **容差随 zoom 变化**：`radius_px` 固定 8 px（可配置），因为它是屏幕量，zoom 变化时容差自然收敛；不要用固定的地面米容差——低 zoom 下会命中一片对象。
7. **Filter 交互**：被 Filter 禁用/隐藏的对象**不可拾取**（与 JOSM 一致），过滤在候选阶段做而不是命中后做，否则「最上层的不可选对象」会盖住下面可选的。

| 方案 | 精度 | 成本 | 结论 |
|---|---|---|---|
| CPU（索引 + 解析距离） | 线/点亚像素，面积需 `Contains` | 与候选数同阶 | **默认**，hover 与点击都用它 |
| GPU ID buffer（R32Uint） | 与像素完全一致，含三角化误差 | 一次 pass + readback 同步 | 密集区/面积内部兜底，默认关（细节见 [05](05-renderer-wgpu.md) §2.10） |
| 混合 | CPU 先出候选，GPU 只在候选 > 32 时介入 | 两套结果需一致 | Phase 2，先做一致性回归再启用 |

## 3 关键接口

```rust
// ── neo-geo/src/projection.rs ───────────────────────────────────────────────
pub struct LonLat { pub lon: f64, pub lat: f64 }
pub struct WorldPoint { pub x: f64, pub y: f64 }
pub struct Aabb { pub min: [f64; 2], pub max: [f64; 2] }

pub enum ProjectionKind {
    WebMercator,                                   // EPSG:3857，默认，无外部依赖
    Geographic,                                    // EPSG:4326，调试/显示
    Proj { epsg: u32, crs: String },               // proj 0.32 后端
}

pub trait Projection: Send + Sync {
    fn kind(&self) -> ProjectionKind;
    fn to_world(&self, ll: LonLat) -> WorldPoint;
    fn to_lonlat(&self, w: WorldPoint) -> LonLat;
    fn is_conformal(&self) -> bool;                // 保角性：决定线宽/圆半径补偿
    fn scale_factor(&self, ll: LonLat) -> f64;     // 局部比例：投影米 → 地面米
    fn tile_scheme(&self) -> Option<TileScheme>;   // 仅 WebMercator
}

// ── neo-geo/src/camera.rs ───────────────────────────────────────────────────
pub struct Camera {
    pub center: LonLat,
    pub zoom: f64,             // 连续缩放级别
    pub viewport_px: [u32; 2], // 物理像素
    pub ppp: f32,              // pixels_per_point（分数，不要取整）
}
impl Camera {
    pub fn px_per_world_m(&self) -> f64;          // 2^zoom * 256 / W
    pub fn world_m_per_px(&self) -> f64;
    pub fn ground_m_per_px(&self) -> f64;         // 乘 cos(lat)
    pub fn view_bounds(&self, margin_px: f32) -> Aabb;
    pub fn rebase_origin(&self) -> WorldPoint;    // 2^16 m 网格对齐
    pub fn to_screen(&self, w: WorldPoint) -> [f32; 2];   // 仅 UI/调试
    pub fn screen_to_lonlat(&self, px: [f32; 2]) -> LonLat;
    pub fn screen_to_world(&self, px: [f32; 2]) -> WorldPoint;
}

// ── neo-geo/src/geometry.rs ─────────────────────────────────────────────────
pub struct Polyline { pub pts: Vec<[f64; 2]>, pub closed: bool, pub cum_len: Vec<f32>, pub aabb: Aabb }
pub struct AreaGeom { pub verts: Vec<[f64; 2]>, pub tris: Vec<u32>, pub rings: Vec<Ring>, pub ok: bool }

pub struct GeometryBuilder { /* earcutr 复用缓冲、抽稀工作区 */ }
impl GeometryBuilder {
    pub fn build_way(&mut self, way: WayView<'_>, cam: &Camera, lod: Lod) -> Polyline;
    pub fn build_area(&mut self, mp: MultiPolygonView<'_>) -> AreaGeom;
    pub fn simplify_screen(&self, pts: &[[f64; 2]], px_eps: f32, world_m_per_px: f64) -> Vec<[f64; 2]>;
    pub fn is_closed_area(&self, pts: &[[f64; 2]], tags: &TagView<'_>) -> bool; // closed ≠ area
}

// ── neo-geo/src/index.rs ────────────────────────────────────────────────────
pub struct ObjEntry { pub id: ObjId, pub env: Aabb, pub osm_id: i64, pub derived: bool }

pub struct SpatialIndex { /* rstar::RTree<ObjEntry> + 段树 + 脏队列 + 重建状态 */ }
impl SpatialIndex {
    pub fn new() -> Self;
    pub fn bulk_load(&mut self, entries: Vec<ObjEntry>);
    pub fn upsert(&mut self, e: ObjEntry);
    pub fn remove(&mut self, id: ObjId);
    pub fn flush_dirty(&mut self, budget: usize) -> DirtyStats;
    pub fn rebuild_if_needed(&mut self, force: bool) -> Option<RebuildHandle>; // 后台 + swap
    pub fn candidates(&self, area: Aabb) -> impl Iterator<Item = ObjId> + '_;
    pub fn nearest_segments(&self, p: [f64; 2], max_d: f64) -> impl Iterator<Item = (ObjId, f64)> + '_;
    pub fn stats(&self) -> IndexStats;
}

// ── neo-geo/src/hit.rs ──────────────────────────────────────────────────────
pub struct HitOpts { pub radius_px: f32, pub allow_area: bool, pub layers: LayerMask, pub skip_filtered: bool }
pub enum HitCandidate {
    Node(ObjId),
    WaySegment { way: ObjId, seg: u32, t: f32, dist: f64 },
    Area(ObjId),
}

pub struct HitTester<'a> { pub index: &'a SpatialIndex }
impl<'a> HitTester<'a> {
    pub fn hit(&self, cam: &Camera, px: [f32; 2], opts: HitOpts) -> Option<HitCandidate>;
    pub fn hit_all(&self, cam: &Camera, px: [f32; 2], opts: HitOpts) -> Vec<HitCandidate>;
}

// ── neo-scene/src/visible.rs ────────────────────────────────────────────────
pub struct VisibleSet { pub objects: Vec<ObjId>, pub batches: Vec<RenderBatch>, pub stats: VisibleStats }
impl VisibleSet {
    pub fn rebuild(index: &SpatialIndex, cam: &Camera, style: &StyleSheet, filter: &FilterSet) -> Self;
    pub fn bucket_at(&self, px: [f32; 2]) -> &[ObjId];   // 屏幕空间索引，256 px 网格
}
```

## 4 数据流

```
逻辑编辑 / 下载 / undo
        │  Command（增量）
        ▼
  neo-store  ──►  dirty(objects) ──►  VisibleSet 失效
        │
        ├─►  neo-geo::geometry   投影 → 抽稀 → 闭合判定 → 法线 → 三角化
        │        └─ Polyline / AreaGeom（投影平面 f64 + 屏幕量参数）
        ├─►  neo-geo::index      upsert/remove；churn>10% 触发后台重建
        ▼
  neo-scene::visible  (cam, style, filter) ──► VisibleSet { objects, batches, 分桶 }
        │  重定基 f64→f32，LOD 过滤，按 (layer, z-index, casing) 排序合批
        ▼
  neo-scene::frame    SceneFrame { MapVertex[], 索引, RenderBatch[], LabelBatch }
        ▼
  neo-render          BufferPool 上传（`version` 变化才传）→ egui paint callback
```

一帧内的判定顺序（避免「每帧全量重建」）：

```
frame:
  input ──► Camera 更新（f64）
     │
     ├─ zoom 跨 LOD 档 或 rebase 跨网格 ──► version+1 ──► 几何重建（分帧预算）
     ├─ style/filter 变更 ────────────────► version+1 ──► 只重算受影响的 tag 指纹桶
     │
     ├─ 索引查询 view_bounds(margin) ──► 候选（O(log n + k)）
     ├─ LOD 丢弃 + 合批 ──► RenderBatch[]
     └─ 仅当 `version` 变化：upload；每帧：uniform 更新（camera scale/offset）
```

## 5 权衡与替代

| 决策 | 备选 | 取此的理由 | 代价 |
|---|---|---|---|
| 自有类型 + rstar 0.14 | 用 geo 的 rstar 0.12 互操作 | 单一 rstar 版本；`PointDistance` 可对段树定制 | 需手写 4 个 trait impl；不能用 `geo::algorithm::rstar` 的现成封装 |
| f64 世界 + f32 重定基 | 全 f64 顶点 | 顶点带宽减半，且 19 级下误差 <0.1 px | 多一层 rebase 逻辑与跨格重建 |
| 按 `version` 重建几何 | 每帧全量重投影 | CPU 与 zoom 变化解耦，pan 几乎零成本 | `version` 边界有一次性成本尖峰，需分帧预算 |
| 屏幕空间分桶 | 直接用对象 R-tree 拾取 | hover 每帧都查，分桶把常数降到 O(1) | 额外内存（约 8 B/可见对象），随可见集重建 |
| earcutr 而非 spade/Delaunay | `geo` 的 `spade` 特性 | 平坦 2D 填充不需要良好形状的三角形，earcut 更省 | 自交输入静默退化，需自检 |
| CPU 拾取默认开启 | 全 GPU ID buffer | 无同步成本、无 1 帧延迟、无需 extra pass | 面积内部判定依赖三角化一致性 |
| geo 关闭默认特性（`earcut`/`spade`/`multithreading`） | `geo = "0.33.1"` 默认 | 默认会拉 **earcut 0.4.5**（与 earcutr 0.5 重复）与 rstar 0.12 | 只按需开 `Contains`/`Simplify` 所需特性，升级时需复核 |

## 6 风险

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| R1 | 精度不足：重定基漏做或 origin 未对齐 | 高 zoom 下几何抖动/开裂 | 单元测试断言「极值坐标 + 重定基后误差 < 1 cm」；debug 断言坐标范围 |
| R2 | earcutr 对自交环静默产坏三角形 | 面出现伪影/穿透 | 首次生成自检 + 失败回退描边 + Validator 报告 |
| R3 | 索引重建与渲染线程竞争 | 掉帧、偶发卡顿 | 双缓冲 + 帧边界 swap；重建在 rayon 池外单独 1 线程，避免抢渲染 |
| R4 | relation 派生 bbox 失效不全 | 大 relation 整块消失 | 成员变动统一走 `invalidate_relation()`；每日一致性校验（bbox ⊇ 成员 bbox） |
| R5 | 多投影下 AABB 非矩形语义（投影后仍是 AABB，但比例不同） | 容差在东西/南北向不等 | `is_conformal()` 为 false 时按主轴统一容差，UI 说明 |
| R6 | JOSM 语义漂移（`area` 匹配未闭合 way、未闭合 multipolygon） | 与 JOSM 视觉不一致 | 语义差异清单进 [11](11-style-presets-mapcss.md) §2.5，并做对照渲染回归 |
| R7 | proj 运行时缺失/`.gsb` 网格缺失 | 投影选项不可用或精度降级 | 启动探测 + 明确降级提示，禁止 panic |

## 7 验收标准

1. **精度**：在 EPSG:3857 极值附近（±180°, ±85°）与参考机目标城市各取 1e4 个点，`to_world → 重定基 → 回投影` 往返误差 < 1 cm；渲染位置与 CPU `to_screen` 偏差 < 0.5 px（19 级）。
2. **索引**：1M / 10M 对象两档数据集，`view_bounds` 查询候选返回正确（与暴力扫描逐项比对）；`bulk_load` 后 `size()` 与输入一致；1000 次随机 `upsert`/`remove` 后仍与暴力扫描一致。
3. **几何**：闭合判定、`area=no`、未闭合 multipolygon、内含 2 层孔洞的 relation 各 20 例与 JOSM 渲染截图人工比对通过。
4. **LOD**：在 0/8/13/16/19 五档 zoom 下，`VisibleStats.vertices` 随 zoom 单调（允许 ±10% 抖动），且**不随总对象数**从 1M 到 10M 显著增长。
5. **拾取**：与 JOSM 同名操作的命中对象一致率 ≥ 95%（200 次人工点击回归）；filtered 对象命中率 = 0。
6. **耗时**：索引查询、几何重建、拾取三项在参考机（i5-12400F / 32 GB / RTX 3070 / Windows）上的单次耗时【待实测，见 docs/14-performance-and-verification.md】。

## 8 参考

- OSM PBF 的坐标精度（`granularity=100`、`lat = 1e-9*(lat_offset + granularity*lat)`）：<https://wiki.openstreetmap.org/wiki/PBF_Format>
- Web Mercator 定义与世界范围：<https://epsg.io/3857>
- JOSM 地图样式语义（`area` 匹配未闭合 way）：<https://josm.openstreetmap.de/wiki/Help/Styles/MapCSSImplementation>
- rstar 0.14 trait 与方法（`bulk_load`、`locate_in_envelope_intersecting`、`PointDistance`）：<https://docs.rs/rstar/0.14.0/rstar/>
- earcutr 0.5.0 的 `earcut(vertices, hole_indices, dims)`：<https://docs.rs/earcutr/0.5.0/earcutr/>
- geo 0.33.1 `Contains`/`PreparedGeometry`/`Simplify`：<https://docs.rs/geo/0.33.1/geo/>
- JOSM 投影/`--projection` 与 `.gsb` 网格：<https://josm.openstreetmap.de/wiki/Help/CommandLineOptions>
- 以下未在本仓库 vendor 中核实：`proj` 0.32 的构造 API 与线程安全性 **> 待验证**；PROJ `proj.db`/`.gsb` 的分发条款 **> 待验证**。
