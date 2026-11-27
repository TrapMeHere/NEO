# 05 渲染器（wgpu 30.0.2 + egui-wgpu 0.36.2）

> 状态：草案 v0.1 ｜ 日期：2026-11-15 ｜ 归属 crate：`neo-render`（消费 `neo-scene` 的 `SceneFrame`/`MapVertex`）
> 版本基准：**wgpu 30.0.2**、**egui / eframe / egui-wgpu 0.36.2**、winit 0.30.13、bytemuck 1.25
> 相关：[01-system-architecture.md](01-system-architecture.md) ｜ [02-workspace-and-dependency-rules.md](02-workspace-and-dependency-rules.md) ｜ [04-spatial-index-and-geometry.md](04-spatial-index-and-geometry.md) ｜ [11-style-presets-mapcss.md](11-style-presets-mapcss.md) ｜ [ADR-0005](adr/0005-wgpu-egui-shared-device.md) ｜ [14-performance-and-verification.md](14-performance-and-verification.md)

## 摘要

- NEO 不自建 winit 事件循环、不建第二个 `wgpu::Instance`：`eframe` 建 surface 与设备，`neo-render` 通过 `egui_wgpu::RenderState`（`adapter/instance/device/queue/target_format/renderer`）拿到**同一个** `Device`/`Queue`，用一个 `CallbackTrait` 绘制地图。
- 管线四类：**A 线段/stroke**、**B 面/fill**、**C 点/图标 instancing**、**D 文本与选中叠加**。四者共用 56 字节的 `MapVertex` 与同一组 bind group。
- 每帧只写 32 字节 camera uniform；几何 buffer 只在 `SceneFrame.version` 变化时按脏区间上传。稳定态的 GPU 侧写入量接近 0。
- 排序用画家算法（`(layer, z, kind)` 稳定序），**Phase 1 不开深度缓冲**：egui 的渲染通道把深度 `store` 设为 `Discard`（见 §2.7），任何跨 callback 的深度复用都不可靠。
- 颜色空间在启动时一次性决定：`target_format` 是 `*Srgb` 还是 gamma 空间（典型 `Bgra8Unorm`），直接决定顶点颜色与着色器输出；混用会双 gamma。
- 拾取默认走 CPU（[04](04-spatial-index-and-geometry.md) §2.10）；GPU ID buffer（`R32Uint` + 1×1 回读）只在候选 > 32 或需要「像素级精确」时启用，且**默认异步、1 帧延迟**。
- 地图文字最终走专用 glyph atlas；Phase 1 用 egui overlay 兑现同一接口（`LabelItem` 已在 `SceneFrame` 里，切换不动上层）。
- 能力探测优先：`Limits::downlevel_defaults()` 作为地板，间接绘制/timestamp/MSAA 全部运行时探测后降级。
- 所有帧时间数字【待实测，见 docs/14-performance-and-verification.md】。

## 1 目标与范围

### 1.1 目标

1. 单帧 16.6 ms（60 Hz）内稳定绘制 1M–10M 对象的可见子集，稳态无每帧重建。
2. 绘制量与「视口内对象数」成正比，与数据层总量无关（LOD 与裁剪在 `neo-scene` 完成，渲染器只做 GPU 侧合批）。
3. 与 egui 共存无撕裂、无双重 gamma、无逐帧丢设备；DPI 1.25/1.5/1.75 下无 1 px 接缝。
4. Windows(DX12)/Linux(Vulkan)/macOS(Metal) 三后端行为一致；软件适配器（WARP/llvmpipe）能降级启动而不是崩溃。
5. 顶点/批次是 POD（`bytemuck`），渲染层零 `neo-store` 依赖。

### 1.2 非目标

- 不做 3D 建筑挤出、地形、阴影（Phase 2；届时才需要深度附件与自己的 prepass）。
- 不做影像瓦片解码（`neo-io`）、不做样式求值（`neo-style`）、不做拾取的语义判定（`neo-geo`）。
- 不实现自研 UI 框架：面板、菜单、对话框全部 egui。

### 1.3 所有权边界

| 资源 | 所有者 | `neo-render` 的取得方式 | 说明 |
|---|---|---|---|
| `wgpu::Instance` / `Adapter` | `eframe` | `RenderState.instance` / `.adapter` | 不重复创建（重复实例=两块显存/两套队列） |
| `wgpu::Device` / `Queue` | `eframe` | `RenderState.device` / `.queue` | 只在 `prepare`/资源创建时用，`paint` 里禁止 `submit` |
| swapchain | `eframe` | 不直接访问 | 颜色格式只通过 `RenderState.target_format` 感知 |
| 管线、buffer、纹理、atlas | `neo-render` | 存 `Renderer::callback_resources`（`type_map`） | 见 [ADR-0005](adr/0005-wgpu-egui-shared-device.md) |
| `RendererOptions`（MSAA/深度） | 应用启动参数 | 构造 `egui_wgpu::Renderer` 时传入 | 必须与我们的管线匹配 |

## 2 设计

### 2.1 总管线

```
neo-scene::SceneFrame（POD：MapVertex[] / 索引 / RenderBatch[] / LabelItem[]）
   │  prepare（CallbackTrait::prepare，本帧 egui pass 之前）
   │  1) pools.begin_frame(version)  2) 脏区间合并 → write_buffer / staging
   │  3) camera uniform 32 B          4) 可选：编码拾取 pass（独立 encoder）
   ▼  eframe 开 egui_render 通道（color = target_format，无深度，MSAA 由 egui 定）
paint（CallbackTrait::paint）：set_viewport(viewport_in_pixels)
   ├─ B fill    draw_indexed（earcut 三角形，预乘 alpha）
   ├─ A stroke  draw_indexed（每段 4 顶点；casing 先宽后窄）
   ├─ C point   draw_indexed(0..6, 0, 0..instances)
   ├─ D glyph   draw_indexed（atlas 四边形）
   └─ D overlay 交给 egui 自己的 mesh（不抢 painter）
   ▼  eframe：egui UI mesh → present
```

`prepare` 与 `paint` 的分工是硬约束：`paint` 拿到的是 `&CallbackResources`（不可变）且已经在一个 `RenderPass<'static>` 里，**不能创建新 pass、不能 submit**；任何需要 encoder 的工作必须放进 `prepare` 并把 `CommandBuffer` 返回给 egui（egui 会在自己的命令之后提交）。

> 命名说明：[01](01-system-architecture.md) 与 [07](07-concurrency-and-tasks.md) 里该方法是 `MapRenderer::encode`，与本文的 `render` 是同一个方法——为对齐 `CallbackTrait::paint` 的语义统一改名为 `render`，三处以本文为准。

### 2.2 顶点格式与批次

```rust
// crates/neo-scene/src/frame.rs —— 无 GPU 依赖，渲染层只消费
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MapVertex {
    pub pos: [f32; 2],     // L2：投影平面米，已按 rebase_origin 重定基
    pub normal: [f32; 2],  // 单位法线（stroke 用；fill/point 为 0）
    pub across: f32,       // 四边形侧 -1.0/+1.0；fill/point 为 0
    pub half_width: f32,   // 屏幕像素（与 zoom 无关）
    pub color: [f32; 4],   // 预乘 alpha，sRGB 编码（见 §2.7）
    pub uv: [f32; 2],      // 图标/图集；fill 为 (0,0)
    pub flags: u32,        // bit0 dash, bit1 casing, bit2 textured, bit3 aa_off
    pub z: f32,            // 排序校验用（不参与深度测试）
}
const _: () = assert!(core::mem::size_of::<MapVertex>() == 56);
```

`MapVertex` 的顶点属性布局（`VertexBufferLayout` 单缓冲，步长 56）：

顶点属性布局（单缓冲，步长 56 B）：`pos` Float32x2 @0、`normal` Float32x2 @8、`across` Float32 @16、`half_width` Float32 @20、`color` Float32x4 @24、`uv` Float32x2 @40、`flags` Uint32 @48、`z` Float32 @52。

```rust
pub enum BatchKind { Fill, Stroke, Point, Glyph }
pub struct RenderBatch {
    pub kind: BatchKind,
    pub layer: LayerId,          // MapCSS ::layer
    pub z: i16,                  // z-index 合成键
    pub texture: Option<TexId>,  // 图标/图集
    pub blend: BlendMode,        // Premul | Alpha | Additive
    pub vr: Range<u32>,          // 顶点区间（同一 FrameSlot）
    pub ir: Range<u32>,          // 索引区间（Fill/A 用；Point 为空）
    pub inst: Range<u32>,        // Point/Glyph 实例区间
    pub pick_base: u32,          // 拾取 ID 基准（与 vr 平行）
    pub flags: BatchFlags,       // casing_of / overlay / dash_reset / no_pick
}

pub struct SceneFrame {
    pub version: u64,            // 几何+样式版本；变化才上传
    pub camera: CameraUniform,   // 每帧
    pub batches: Vec<RenderBatch>,
    pub labels: Vec<LabelItem>,  // 世界坐标 + 文本 + 样式（D 类消费者）
    pub stats: SceneStats,       // 顶点数/批次数/被剔除数（验收用）
}

#[repr(C)]
pub struct CameraUniform {       // 32 B，16 B 对齐
    pub scale: f32,              // 世界米 → 物理像素
    pub offset: [f32; 2],        // rebase 后的平移（像素）
    pub viewport: [f32; 2],      // 物理像素
    pub _pad: [f32; 3],
}
```

### 2.3 管线 A：线 / 道路 stroke

- 拓扑 `TriangleList`，**每段 4 顶点 + 6 索引**（`[0,1,2, 0,2,3]`）。顶点着色器：`p_px = affine(pos) + normal * half_width * across`，坐标与线宽都在像素空间，故线宽天然与 zoom 无关。
- 连接处（miter/bevel/round）由 `neo-geo` 生成的额外四边形补齐；round join 不生成扇面，用端点圆片 + 解析 AA。
- 抗锯齿：片元用 `cov = saturate(1.0 - (abs(across) - 1.0 + 1.0/px))` 做 1 px 覆盖带，**不依赖 MSAA**。
- 虚线：`flags.dash` + `uv.x = 累计弧长 / dash_period`，片元按 fract 取舍；`dash_reset` 表示新对象起点重置相位。
- casing：**先**画宽 casing 批次、**后**画窄 fill 批次；由 `BatchFlags::casing_of` 保证相邻，任何 z-index 变化都必须整体移动这一对（`neo-scene` 负责，渲染层只校验顺序，违序则记一条 `debug!`）。当前实现（[11](11-style-presets-mapcss.md) §0）：每层一个独立 `BatchKind::Casing` 批次，提交顺序 `fill → casing → stroke → point`，桥梁外框/隧道虚线外框/过水路面水面都走这条通道。
- 不走 `POLYGON_MODE_LINE`：它的线宽在所有后端都不可靠（多数实现固定 1 px），只作为调试开关。

### 2.4 管线 B：面 / 建筑 fill

- 输入是 earcutr 产出的 `verts + tris`（[04](04-spatial-index-and-geometry.md) §2.6），索引 `Uint32`；一个批次内可合并多个面（相邻且同一样式），减少 draw call。
- 混合用**预乘 alpha**（`wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING`）；样式里的直通 alpha 颜色在 `neo-scene` 打包时预乘一次，避免了「面叠面发黑」。
- 边缘处理两档：`msaa_samples > 1` 时靠 MSAA；`msaa_samples == 1` 时接受硬边并把该事实记进 `GpuCaps`（不做超采样后处理，代价太大）。
- 空洞/退化三角形：`AreaGeom.ok == false` 的面**不进 fill 管线**（已被 `neo-scene` 剔除并转成 stroke），渲染层不做几何修补。

### 2.5 管线 C：点 / 图标 instancing

- 6 顶点 quad 由 `vertex_index` 生成，实例缓冲步长 48 B：

```rust
#[repr(C)]
pub struct PointInstance {
    pub pos: [f32; 2],     // L2 世界
    pub size_px: [f32; 2], // 图标宽高（像素）
    pub rot: f32,          // 弧度
    pub tint: [f32; 4],    // 预乘
    pub uv_rect: [f32; 4], // atlas 子矩形
}
```

- 一次 `draw_indexed(0..6, 0, 0..n)` 画完一个图层的所有点；不同 atlas 页 = 不同批次。
- 节点手柄、选中框**不进** C：它们是 UI 语义（可 hover、可拖拽），走 D 类 overlay，避免每一帧为手柄重传实例缓冲。

### 2.6 管线 D：文本与选中叠加（含决策）

| 方案 | 实现 | 优点 | 代价 | 结论 |
|---|---|---|---|---|
| D1 egui overlay | `egui::Painter::text` 逐标签 | 零新基础设施、i18n/字体/缩放直接复用 egui | 每个标签一次 layout（egui 的 galley 缓存能缓解但不消除）；10k+ 标签不可行 | **Phase 1 出货**，标签数上限 ≤ 2k/帧 |
| D2 专用 glyph atlas | `cosmic-text` 做整形 → 自建 atlas → instanced glyph quads | 单次 draw、无 layout 抖动、可做 SDF | 需要字形缓存、换行/避让、RTL、CJK fallback 全套 | **Phase 2 目标**，`LabelItem` 接口已预留 |
| D3 混合 | 地图名走 D2，选中/量测/提示走 D1 | 各用所长 | 两套坐标系要对齐（都用 `PaintCallbackInfo` 的像素） | **最终形态** |

决策：**D3**，但按 D1 → D2 的顺序交付；`SceneFrame.labels` 的形状不随实现变化，切换只改 `LabelRenderer` 内部。

### 2.7 深度、混合、MSAA、sRGB

| 项 | 决策 | 依据 / 注意 |
|---|---|---|
| 深度 | Phase 1 **关闭**（`RendererOptions.depth_stencil_format = None`），画家算法排序 | egui 的通道里深度 `load = Clear(1.0)`、`store = Discard`；跨 callback 复用深度不可靠 |
| 深度（Phase 2） | `Some(Depth32Float)`，管线 `DepthStencilState { depth_write_enabled: Some(true), depth_compare: Some(Less) }` | 两个字段在 wgpu 30 是 `Option`；prepass 必须放 `prepare` 并自建纹理 |
| 混合 | 预乘 alpha（fill/point/glyph）；`Additive` 仅用于高亮辉光 | `BlendState::PREMULTIPLIED_ALPHA_BLENDING` 与 egui 自身一致，避免 UI/地图重叠发黑 |
| MSAA | 启动时探测 `get_texture_format_features(fmt).flags` 是否含 `MULTISAMPLE_X4`，优先 4x，否则 1 | 管线的 `MultisampleState.count` 必须 **等于** `RendererOptions.msaa_samples.max(1)`，否则是运行时校验错误（不是编译错误） |
| sRGB | 读 `target_format.is_srgb()` 一次性分流（`GpuCaps.srgb`） | egui 建议 swapchain 用 gamma 空间（`Bgra8Unorm`）；若平台强制 `*Srgb`，我们的片元必须输出线性值，否则双 gamma 发白 |
| 颜色通路 | 样式颜色（sRGB）→ 预乘 → 顶点；着色器**不做** gamma 变换，只做 `srgb` 分支的 `linear = pow(color, 2.2)` | 只在 `GpuCaps.srgb == true` 时启用该分支；两条路径用同一份 golden 图验收 |
| DPI | `viewport_in_pixels()` 取整做 `set_viewport`，`pixels_per_point` 只用于 UI 尺寸 | 分数 DPI（1.25/1.5/1.75）不得进投影矩阵，否则 1 px 接缝 |

### 2.8 BufferPool 与增量更新

- **槽位**：`FRAMES_IN_FLIGHT = 2`。每槽独立持有 vertex/index/instance/uniform buffer，避免「上一帧还在读、这一帧已覆盖」。
- **逻辑池**：按 `BatchKey { layer, z, kind, texture }` 维护稳定分配，`RenderBatch` 的区间在 `version` 不变时**完全不变**，这是「零每帧重建」的关键。
- **脏区间上传**：`version` 变化时，把新旧批次的区间做差集 → 合并相邻区间（间隔 < 4 KiB 就并）→ 单段 < 64 KiB 用 `queue.write_buffer`，否则写入 staging buffer 后 `copy_buffer_to_buffer`（大块 `write_buffer` 会隐式分配暂存并可能阻塞）。
- **增长策略**：容量按 1.5× 增长，永不缩容；单槽超过 512 MiB 才在新槽里做一次整理（不原地搬）。
- **稳态写入量**：32 B uniform + 少量脏段；`PrepareStats { uploaded_bytes, draw_calls, batches }` 进 puffin 与验收断言。
- **禁止**：每帧 `create_buffer_init`、每帧 `create_bind_group`、`paint` 里 `queue.submit`。

### 2.9 间接绘制与降级

| 能力 | 探测 | 用途 | 缺失时 |
|---|---|---|---|
| `multi_draw_indexed_indirect` | wgpu 30 中**不再是 feature**（已并入核心），直接可用 | 一次提交几十~几百个批次 | — |
| `MULTI_DRAW_INDIRECT_COUNT` | `adapter.features().contains(..)` | GPU 侧决定绘制数（拾取 pass、动态 LOD） | CPU 端读回计数，退化为固定次数 |
| `INDIRECT_FIRST_INSTANCE` | 同上 | 合并多个实例化批次到一次调用 | 每批一次 `draw_indexed` |
| `TIMESTAMP_QUERY`（含 `_INSIDE_ENCODERS`/`_INSIDE_PASSES`） | 同上 | GPU 阶段计时 | 只用 puffin 的 CPU 区间 |
| `POLYGON_MODE_LINE` | 同上 | 仅调试线框 | 无 |

策略：**默认走 `draw_indexed` 循环**（批次数量在几十量级，间接绘制收益有限），只有在 `batches > 512`（极端样式/大量图层）或拾取 pass 时才切间接路径；两条路径共用同一 `RenderBatch` 数组，切换是纯编码差异。

### 2.10 GPU 拾取（ID buffer）

- 目标：`R32Uint` 颜色附件 + `Depth32Float`；无混合、无 AA；每个可见图元写入 `(obj_index << 8) | kind`。
- 回读只取**1×1 texel**：`copy_texture_to_buffer` 到 256 B 对齐的缓冲（`bytes_per_row = 256`，这是硬性对齐要求，不能是 4），之后 `map_async`。
- 同步成本：`device.poll(PollType::wait_indefinitely())` 会阻塞到 GPU 完成当前提交——相当于一次全流水线 flush，**绝不放在 hover 路径**。
- 异步路径：点击时 `request()` → `prepare` 里编码 → 下一帧 `try_take()`（`PollType::Poll`，非阻塞）→ 命中则更新选中。1 帧延迟对点击可接受；hover 仍用 CPU 拾取。
- 禁用条件：`neo-render` 的 `pick-readback` feature 关闭、`max_texture_dimension_2d < 视口`、或软件适配器 → 直接返回 `None`，上层自动回落 CPU（[02](02-workspace-and-dependency-rules.md) §3 的 feature 表）。

> 注意：wgpu 30 的 `Surface::get_current_texture()` 返回 `CurrentSurfaceTexture`（`Success/Suboptimal/Timeout/Occluded/Outdated/Lost/Validation`），`Queue::present(t)` 才呈现。NEO 不拥有 surface，仅在自建离屏渲染（导出 PNG、golden 测试）时走这条路径，且必须处理 `Lost`（重建 surface 而非仅 `configure`）。

### 2.11 文字图集与 CJK 字体资产

- atlas：`R8Unorm` 1024×1024、两页起步（拉丁/符号 + CJK），按需扩容到 4 页；字形缓存键 `(font_id, px_size_bucket, glyph_id, subpixel_x)`，`px_size_bucket` 取 1/4 像素桶以避免缓存爆炸。
- 整形：`cosmic-text`（版本未核实 **> 待验证**）负责换行、双向文本、font fallback；`glyphon` 只在它已支持 wgpu 30 时才考虑，否则自建 atlas 采样路径（atlas 本身不依赖任何 crate）。
- CJK 资产策略：**不把字体编译进二进制**。启动时按平台枚举系统字体（Windows 的 `msyh`/`simhei`、Linux 的 Noto CJK、macOS 的 `PingFang`），找不到再提示用户选择；可选下载 OFL 授权的 Noto Sans CJK 到用户数据目录（体积与 GPL 隔离双重原因，见 [11](11-style-presets-mapcss.md) §2.13）。
- 标签避让在 `neo-scene`（屏幕空间网格，[04](04-spatial-index-and-geometry.md) §2.8），渲染层只画「已决定要画」的标签，不做碰撞。

### 2.12 LOD、裁剪、合批

- 裁剪：CPU 侧视锥（AABB）裁剪已在可见集完成；GPU 侧不做 per-primitive 裁剪，靠视口裁剪。
- 合批：排序键 `(layer, z, kind, texture, blend)`；相邻且同键的批次合并。稳定排序保证同 `z` 下按 `ObjId` 有序 → 同一份数据的渲染结果可复现（golden 测试依赖这一点）。
- LOD 阈值只在 `neo-scene` 判定；渲染层接收到的就是「该画什么」，避免两处阈值打架。
- 大范围平移（pan）**不触发** `version` 变化（重定基在同一 64 km 网格内），只更新 uniform；缩放跨 LOD 档才重建。

### 2.13 帧预算（16.6 ms）

> 下表是**设计预算**（分配目标），不是测量值。真实数字【待实测，见 docs/14-performance-and-verification.md】。

| 阶段 | 线程 | 预算 (ms) | 超预算动作 |
|---|---|---|---|
| 输入 + egui UI 逻辑 | 主 | 3.0 | 折叠面板/延迟布局 |
| 场景差量 + 合批 | 主 | 1.5 | 复用上一帧 `SceneFrame` |
| uniform + 脏区间上传 | 主 | 0.8 | 脏区间延后一帧（视觉无感） |
| egui tessellation + callback 编码 | 主 | 2.7 | 降标签数 |
| 主线程小计 | 主 | **8.0** | 有 8.6 ms 余量 |
| 几何重建（可跨帧） | 工作 | 4.0 | 提高 LOD 阈值、减少每帧对象数 |
| 样式求值（可跨帧） | 工作 | 2.0 | 命中 tag 指纹缓存 |
| fill / stroke / point / glyph | GPU | 2.5 / 2.5 / 0.6 / 1.2 | 合并批次、降 MSAA |
| egui composite + present | GPU | 1.5 + 0.5 | 关 dithering |

### 2.14 GPU 能力矩阵与降级

| 能力 | DX12 | Vulkan | Metal | 软件（WARP/llvmpipe） | NEO 用法 | 缺失时 |
|---|---|---|---|---|---|---|
| `Bgra8Unorm` / `Rgba8Unorm` 呈现 | ✅ | ✅ | ✅ | ✅ | 首选格式 | 回落 `*Srgb` + 线性输出分支 |
| `Depth32Float` | ✅ | ✅ | ✅ | ✅ | Phase 2 | 无深度（Phase 1 本就不需要） |
| MSAA 4x | ✅ | ✅ | ✅ | 常见缺失 | 面/图标 | `msaa_samples = 1`，面边缘硬边 |
| `R32Uint` 颜色附件 | ✅ | ✅ | ✅ | ✅ | GPU 拾取 | 关 GPU 拾取 |
| `TIMESTAMP_QUERY` | ✅ | ✅ | ✅ | 常缺 | 阶段计时 | puffin CPU 计时 |
| 间接绘制（核心） | ✅ | ✅ | ✅ | ✅ | 大批次 | 循环 draw |
| `MULTI_DRAW_INDIRECT_COUNT` | ✅ | ✅(1.2+) | ❌ | ❌ | 拾取 pass | CPU 计数 |
| `INDIRECT_FIRST_INSTANCE` | ✅ | ✅ | ✅ | ❌ | 实例合批 | 每批一次 draw |

**地板**：`Limits` 请求 `Limits::downlevel_defaults()`（`max_texture_dimension_2d = 2048`、`max_uniform_buffer_binding_size = 16 KiB`、`max_vertex_attributes = 16`、`max_inter_stage_shader_variables = 15`、`max_color_attachments = 4`），再与 `adapter.limits()` 取 `min` 后向上争取。`DeviceDescriptor { label, required_features, required_limits, experimental_features, memory_hints, trace }` 一次填全（wgpu 30 的 `request_device` 只收 `&DeviceDescriptor`，没有第二个参数）。

## 3 关键接口

```rust
// ── neo-render/src/caps.rs ──────────────────────────────────────────────────
pub struct GpuCaps {
    pub target_format: wgpu::TextureFormat,
    pub srgb: bool,
    pub msaa_samples: u32,             // 1 或 4
    pub limits: wgpu::Limits,
    pub has_multi_draw_count: bool,
    pub has_indirect_first_instance: bool,
    pub has_timestamps: bool,
    pub software_adapter: bool,
}
impl GpuCaps {
    pub fn probe(adapter: &wgpu::Adapter, target_format: wgpu::TextureFormat, want_msaa: u32) -> Self;
    pub fn renderer_options(&self) -> egui_wgpu::RendererOptions; // msaa_samples / depth_stencil_format
}

// ── neo-render/src/renderer.rs ──────────────────────────────────────────────
pub struct MapRenderer {
    caps: GpuCaps,
    fill: FillPipeline,
    stroke: StrokePipeline,
    point: PointPipeline,
    glyph: GlyphPipeline,
    pools: BufferPool,
    atlas: GlyphAtlas,
    picking: PickingPass,
    timestamps: Option<TimestampQueries>,
}

impl MapRenderer {
    /// 只建管线与静态资源；不持有 Instance/Device 的所有权。
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, caps: GpuCaps) -> Self;

    /// 每帧一次：上传脏区间 + 写 uniform + 可选编码拾取 pass。
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &SceneFrame,
        screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
    ) -> PrepareStats;

    /// 由 `CallbackTrait::paint` 转发；只发 draw，不建 pass、不 submit。
    pub fn render(&self, pass: &mut wgpu::RenderPass<'static>, info: egui_wgpu::PaintCallbackInfo);
}

// ── neo-render/src/buffer.rs ────────────────────────────────────────────────
pub struct FrameSlot {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub instances: wgpu::Buffer,
    pub uniform: wgpu::Buffer,
}
pub struct BufferPool { /* slots: [FrameSlot; 2], cur: usize, keys: HashMap<BatchKey, Range<u32>> */ }
impl BufferPool {
    pub fn new(device: &wgpu::Device, caps: &GpuCaps) -> Self;
    pub fn begin_frame(&mut self, version: u64) -> bool;      // true = version 变了
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &SceneFrame) -> UploadStats;
    pub fn slot(&self) -> &FrameSlot;
    pub fn trim(&mut self, budget_bytes: u64) -> bool;        // 只在跨槽整理时调用
}

// ── neo-render/src/picking.rs ───────────────────────────────────────────────
pub struct PickingPass { /* R32Uint 目标、Depth32Float、256B 对齐 readback、pending 队列 */ }
impl PickingPass {
    pub fn new(device: &wgpu::Device, caps: &GpuCaps) -> Self;
    pub fn resize(&mut self, device: &wgpu::Device, size_px: [u32; 2]);
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, frame: &SceneFrame, pool: &BufferPool) -> wgpu::CommandBuffer;
    pub fn request(&mut self, px: [u32; 2]) -> PickRequestId;                  // 异步
    pub fn try_take(&mut self, device: &wgpu::Device, id: PickRequestId) -> Option<Option<ObjId>>; // Poll，非阻塞
    pub fn take_blocking(&mut self, device: &wgpu::Device, id: PickRequestId) -> Option<ObjId>;    // PollType::wait_indefinitely()
}

```

## 4 时序

```
帧 N（稳态，version 未变）
  egui::Context::run
    │
    ├─ ui.painter().add( MapCallback { frame } )
    │
    ├─ prepare():  pools.begin_frame(v) → false（不变）
    │              upload: 仅 uniform 32 B
    │
    ├─ egui tessellate → clipped_primitives（含 MapCallback）
    │
    ├─ eframe: begin_render_pass("egui_render")
    │    color: swapchain/msaa view, load = Clear(clear_color)
    │    depth: 仅当 RendererOptions::depth_stencil_format.is_some()
    │
    ├─ renderer.render(): egui 自己的 mesh 与我们的 paint 按图元顺序执行
    │    └─ MapCallback::paint → fill → stroke → point → glyph
    │
    └─ present
```

```
click(x,y) ─► PickingPass::request(px)                 // 帧 N，返回 PickRequestId
prepare(帧 N)  encode: R32Uint pass（仅可见集）→ copy_texture_to_buffer(1×1, bytes_per_row = 256)
prepare(帧 N+1) try_take()：PollType::Poll（非阻塞）→ ObjId = id >> 8；Pending 再等 1 帧，>3 帧放弃
hover 路径永远走 CPU（neo-geo::HitTester），不触发 readback
```

## 5 权衡与替代

| 决策 | 备选 | 取此的理由 | 代价 |
|---|---|---|---|
| 共享 eframe 的 Device/Queue，用 `CallbackTrait` 绘制 | 自建 `Instance`/`Surface`/事件循环 | 单一队列、单一 present、无第二块显存；DPI/窗口/IME 全由 eframe 处理 | 受 egui 通道约束：不能改 load op、深度必须自备、MSAA 由 `RendererOptions` 决定 |
| 几何预生成 + 脏区间上传 | 每帧把可见集写进顶点缓冲 | 稳态上传≈0，CPU 与 GPU 解耦 | `version` 边界有一次上传尖峰，需分帧 |
| 画家算法（无深度） | 深度附件 + 深度测试 | 2D 地图本来就要按样式 z 排序；egui 的 depth `store = Discard` 让跨 callback 深度不可依赖 | Phase 2 若做挤出需自带 prepass |
| 解析 AA（stroke） | 全靠 MSAA | 线宽/虚线在 1x 下也干净，MSAA 关掉不掉质量 | 片元多几条指令 |
| 预乘 alpha 全链路 | 直通 alpha + 逐管线 blend state | 与 egui 一致，面叠面不发黑 | 颜色打包时多一次乘法（缓存） |
| CPU 拾取为主 | 全 GPU ID buffer | 无 flush、无延迟、复用已有索引 | 面积内部精度依赖三角化一致性 |
| `write_buffer` 小段 + staging 大段 | 全部 `write_buffer` | 避免大块写触发隐式暂存与阻塞 | 两条上传路径要各自测试 |
| 间接绘制按需启用 | 一律间接 | 几十个批次时循环 draw 更简单、可读性高 | 极端样式下需要两条路径都维护 |

## 6 风险

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| R1 | 管线与 egui 通道不匹配（format/count/depth） | 运行时校验错误，整帧黑屏 | `GpuCaps` 单一来源生成管线；启动自检 1 帧 |
| R2 | 双 gamma（`*Srgb` 平台） | 地图发白/发灰，与 UI 不一致 | 启动分流 + 两条路径的 golden 图 |
| R3 | 每帧重建 buffer 悄悄回归 | 帧时间随对象数线性上升 | `PrepareStats.uploaded_bytes` 断言（稳态 < 4 KiB）+ CI 基准 |
| R4 | 拾取 readback 阻塞主线程 | 点击时掉帧到 < 20 fps | 强制异步路径；hover 禁 readback；超时回退 |
| R5 | 软件适配器（WARP）缺 MSAA/timestamp | 启动失败或黑屏 | 能力探测 + 降级表；`Error::Wgpu` 显示可读信息而非 panic |
| R6 | 字体/atlas 缓存膨胀 | 内存与首次弹出延迟 | 尺寸桶 + LRU 页回收；CJK 延迟加载 |
| R7 | egui 版本漂移（0.36.x → 0.37） | `RenderState`/面板/回调细节变化（`App::ui` 入口在 0.36 已就位） | 只在 `apps/neo-desktop` 与 `neo-render` 两处收口（见 §8 的版本说明） |

## 7 验收标准

1. **稳态零重传**：静置 60 帧、仅平移时 `PrepareStats.uploaded_bytes` 每帧 ≤ 128 B（uniform），`draw_calls` 稳定。
2. **帧时间**：参考机（i5-12400F / 32 GB / RTX 3070 / Windows，DX12）在 1M 与 10M 对象两档、16 级缩放下，主线程与 GPU 时间【待实测，见 docs/14-performance-and-verification.md】；验收门槛为设计预算表 ±20%。
3. **视觉一致**：同一数据集与 [11](11-style-presets-mapcss.md) 的样式，在 DX12/Vulkan/Metal 三后端截图 SSIM ≥ 0.99（允许 MSAA/字体差异 1 档）。
4. **颜色**：`Bgra8Unorm` 与 `Bgra8UnormSrgb` 两种 `target_format` 下，同一张 golden 图的像素差 ≤ 2/255。
5. **拾取**：GPU 与 CPU 结果在 1000 次随机点上的一致率 ≥ 99%（差异必须能归因到三角化 vs 解析几何）；readback 关闭时功能不缺失。
6. **健壮性**：WARP 与 `msaa_samples = 1`、无 timestamp 的组合下可启动、可交互、无 panic。
7. **依赖纪律**：`cargo tree -p neo-render` 不含 `neo-store`/`neo-editor`/`neo-ui`（[02](02-workspace-and-dependency-rules.md) §7 第 2 条）。

## 8 参考

- egui-wgpu `CallbackTrait` / `RenderState` / `RendererOptions`（**0.36.2**，本仓库以该版本为准）：<https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/>
- `egui_dock` 0.21.1 依赖 `egui ^0.36`（因此不能上 0.37）：<https://crates.io/api/v1/crates/egui_dock/0.21.1/dependencies>
- wgpu 30 的破坏性变更（`Maintain` 移除、`DeviceDescriptor`、`multiview_mask`）：<https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md>
- `Limits::downlevel_defaults` / 特性表：<https://docs.rs/wgpu-types/30.0.2/wgpu_types/struct.Limits.html>
- WebGPU 规范（`copyTextureToBuffer` 的 256 B `bytesPerRow` 对齐）：<https://www.w3.org/TR/webgpu/#dom-gpucommandencoder-copytexturetobuffer>
- 以下未核实：`cosmic-text`/`glyphon` 的当前版本与对 wgpu 30 的支持 **> 待验证**；WARP 的具体能力矩阵 **> 待验证**（[01](01-system-architecture.md) §8 同样标注）。


## 节点 LOD（2026-11-15 起）

节点图标不是无条件绘制，而是随 zoom 分三档（`neo-scene::builder::{NODE_MIN_ZOOM, NODE_ALL_ZOOM}`）：

| zoom | 绘制内容 | 形状 |
|---|---|---|
| ≤ 16（z16 及以下） | 不绘制节点 | — |
| 17 ～ 18 | 仅**共享节点**（被 ≥2 条 way 引用，`Referrers::ways_of_node().len() >= 2`） | 小圆点（六边形近似，7 顶点/6 三角） |
| ≥ 19 | 全部节点 | 共享节点 = 小方框（4 顶点/2 三角）；普通节点 = 小圆点 |

共享判定依赖 `Referrers`（CSR 反查表），因此 `SceneBuilder::build` 的签名需要传入 `&Referrers`；
桌面端直接复用 `Editor` 已维护的反查表，无额外预处理成本。工具栏不再有"显示节点"开关，
行为完全由 zoom 决定，避免用户手动切换造成密集区噪声。

**拾取与渲染共用同一规则** `neo_scene::node_visible_at(zoom, way_ref_count)`：
不可见的节点不能被点击选中、也不能拖动；缩放后应用层会自动清理已变为不可见的节点选择（way 选择保留）。


## 选中/拖动的屏幕外边距

| 用途 | 节点 | way |
|---|---|---|
| 点击选择（`NODE_PICK_MARGIN_PX` / `WAY_PICK_MARGIN_PX`） | 16 px | 16 px |
| 选中后拖动（`SELECTION_DRAG_MARGIN_PX`） | 16 px | 16 px（点到折线距离） |

两种元素的边距**独立计算**（`HitTester::pick_with_margins`）：鼠标在节点边距内选节点（即使节点压在 way 上也不会误选 way）；
否则在 way 边距内选 way。判定使用鼠标**按下原点**，避免 egui 拖拽阈值触发时指针已移动导致误判。
对不可见节点（按 zoom LOD）不会进入拾取流程：`pick_with_margins_filtered` 在**拾取阶段**过滤隐藏节点，
并回退到其下方的 way —— 否则隐藏节点会截胡 way 的点击，出现"点要素本身选不中、点旁边几像素反而能选中"的问题。


## 拖动与选中的关系

拖动必须以选中为前提，**按下时自动完成选中**：

1. 左键按下命中未选中的节点/way → 立即把它设为当前选中（红框出现），随即进入拖动；
2. 拖动已选中的要素（或其外边距内）→ 一起拖动整个选中集合（多选时）；
3. 松开后提交 **1 条历史**（批量 `MoveNode`），选中状态保留、红框显示在移动后的位置。

因此"点击拖动"一步完成"选中 + 移动"，不会出现"拖了但没选中"的状态。
