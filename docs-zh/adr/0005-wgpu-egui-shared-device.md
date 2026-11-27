# ADR-0005：与 egui 共享同一个 wgpu 设备（不另建渲染后端）

- **状态**：已接受（Accepted）
- **日期**：2026-11-15
- **决策范围**：`apps/neo-desktop`（启动）、`neo-render`（全部绘制）、`neo-ui`（paint 顺序）
- **版本基线**：egui / eframe / egui-wgpu **0.36.2**、wgpu **30.0.2**、winit 0.30.13
- **相关**：[05-renderer-wgpu.md](../05-renderer-wgpu.md) ｜ [01-system-architecture.md](../01-system-architecture.md) ｜ [ADR-0010](0010-mapcss-compatibility.md)

## 背景

NEO 的界面（面板、菜单、对话框、属性表）用 egui；地图本身有 1M–10M 对象的矢量批次，必须有自己的 wgpu 管线、buffer pool、文字图集与拾取通道。两条路线：

1. **共享** eframe 创建的 `Instance`/`Device`/`Queue`，通过 `egui_wgpu::CallbackTrait` 在 egui 的渲染通道里绘制地图；
2. **自建** 第二个 `Instance` + `Surface` + 事件循环（或离屏纹理合成），地图与 UI 各自 present。

这不是品味问题，而是被几个硬事实约束的：同一进程两个 `wgpu::Instance` 意味着两套适配器枚举、两块 swapchain、两个 present 时序；而 winit 的窗口事件、DPI 变化、IME、无障碍都已经被 eframe 接管，自己再接管一次会出现输入延迟与焦点竞争。

同时，共享方案的代价是**必须服从 egui 的渲染通道**，而这些约束是运行时错误而非编译错误（最典型的：管线 `MultisampleState.count` / `ColorTargetState.format` / `DepthStencilState.format` 与通道不一致时，只有在 draw 时才报校验错误）。

## 决策

**采用方案 1**，并接受下列具体约束：

| # | 约束 | 依据 |
|---|---|---|
| D1 | 只使用 `eframe::egui_wgpu::RenderState` 提供的 `adapter` / `instance` / `device` / `queue` / `target_format` / `renderer`；**不创建第二个 `Instance`** | 单队列、单 present；`RenderState` 是 `Clone` 且字段全公开 |
| D2 | 地图绘制实现 `egui_wgpu::CallbackTrait`：`prepare(&self, &Device, &Queue, &ScreenDescriptor, &mut CommandEncoder, &mut CallbackResources) -> Vec<CommandBuffer>` 与 `paint(&self, PaintCallbackInfo, &mut RenderPass<'static>, &CallbackResources)` | `paint` 里已在通道内，禁止新建 pass / `submit`；需要 encoder 的工作放 `prepare` 并把 `CommandBuffer` 返回给 egui |
| D3 | 持久 GPU 资源（管线、`BufferPool`、atlas、拾取纹理）存 `Renderer::callback_resources`（`type_map`），用 `get_mut` / 否则 `insert` | 生命周期与 `Renderer` 绑定；不要跨帧持有 `Renderer` 写锁（会与 paint 回调里的读锁死锁） |
| D4 | 所有管线的颜色格式取 `RenderState::target_format`；`RendererOptions` 里显式设置 `msaa_samples` 与（如需）`depth_stencil_format` | egui 默认无深度且深度 `store = Discard`；MSAA 由 egui 统一 resolve |
| D5 | 顶点与颜色在**预乘 alpha** 下与 egui 混用同一套混合状态 | 与 egui 自身一致；避免 UI 与地图重叠处的暗边 |
| D6 | 每帧只写 32 B 的 camera uniform；几何 buffer 仅在 `SceneFrame.version` 变化时按脏区间上传 | 稳态零重传，见 [05](../05-renderer-wgpu.md) §2.8 |
| D7 | 拾取回读绝不阻塞 UI 线程；`PollType::wait_indefinitely()` 只允许出现在导出/测试路径 | 一次等待 = 全流水线 flush，会让 hover 掉帧（[07](../07-concurrency-and-tasks.md) §D10） |
| D8 | 自建离屏渲染（导出 PNG、golden 图）也要用同一个 `Device`，并显式处理 `CurrentSurfaceTexture::Lost` / 256 B row pitch | wgpu 30 的 `get_current_texture` 返回枚举、`Queue::present(tex)` 才呈现 |

## 替代方案（已评估并放弃）

| 方案 | 优点 | 放弃理由 |
|---|---|---|
| 自建 `Instance` + `Surface` + 自管 winit 循环 | 完全控制 present 时机与 load op | 输入/DPI/IME/无障碍要与 eframe 争所有权；两套 present 会互相撕裂；显存与适配器枚举翻倍 |
| 地图渲染到离屏纹理，再作为 egui 纹理贴上来 | 不需要 `CallbackTrait`，与 egui 解耦 | 多一次全屏合成（带宽 = 视口像素 × 4 B × 帧率），且拾取/视口/DPI 要自己对齐；10M 对象场景下这笔带宽不可忽略 |
| 用 `egui` 的 `Mesh` 直接画地图（不走 wgpu） | 零新基础设施 | 每帧重新 tessellate 百万级线段，CPU 与内存都不可行（egui 的 mesh 是 immediate 的） |
| 等 egui 提供多 viewport 的官方 3D 集成 | 未来更干净 | 现状未提供；`CallbackTrait` 是官方支持的扩展点，已有稳定签名 |

## 后果

**正面**

- 单设备单队列，UI 与地图共享同一个 present，无撕裂、无第二块 swapchain。
- DPI/缩放/窗口生命周期由 eframe 处理；我们只需把 `PaintCallbackInfo::viewport_in_pixels()` 当视口、把 `pixels_per_point` 当 UI 尺寸。
- 拾取、hover、选择高亮与 UI 同帧，无 1 帧输入延迟。
- 依赖面小：`neo-render` 只依赖 `wgpu` + `egui-wgpu` + `bytemuck`，不依赖 `neo-store`/`neo-editor`/`neo-ui`。

**负面 / 代价**

- 受 egui 通道约束：不能改 load op、不能跨 callback 复用深度、MSAA 由 `RendererOptions` 决定、颜色格式必须与 swapchain 完全一致（含 sRGB 分支）。
- 管线创建参数与运行期通道必须由**单一来源**（`GpuCaps`）派生，否则会出现只在某些后端触发的校验错误。
- 帧内顺序被 egui 的图元顺序决定：必须把地图 callback 加在正确的 `ui` 层级，才能保证覆盖层（选中框、手柄）画在地图之上。
- egui 版本升级是**破坏性**的：`eframe::App` 的入口在 0.36 已是 `ui()`（`update` 被移除），0.36 → 0.37 又带来 `RenderState`/面板 API 变化；升级点必须收敛在 `apps/neo-desktop` 与 `neo-render` 两处。

**约束（必须写进 CI）**

1. `cargo tree -p neo-render` 不含 `neo-store` / `neo-editor` / `neo-ui`。
2. 全仓只允许出现一个 `wgpu::Instance`（源码 grep `Instance::new` / `new_without_display_handle`）。
3. 管线创建只允许经 `MapRenderer::new`，禁止在 `paint` 中创建资源。

## 验证

- **启动自检**：首帧校验每条管线的 `format` / `sample_count` / `depth` 与当前通道一致（不一致立刻 panic，带可读信息），替代"跑几分钟才黑屏"。
- **稳态断言**：静置与平移 60 帧，`PrepareStats.uploaded_bytes ≤ 128 B/帧`（只有 uniform）。
- **颜色**：`Bgra8Unorm` 与 `Bgra8UnormSrgb` 两种 `target_format` 下同一 golden 图像素差 ≤ 2/255。
- **拾取**：任何帧都不调用 `PollType::wait_*`（源码 grep + 单测）；GPU 与 CPU 拾取一致率 ≥ 99%。
- **降级**：WARP / 无 MSAA / 无 timestamp 组合可启动、可交互、无 panic。
- 帧时间验收数字见 [14](../14-performance-and-verification.md)【待实测】。

## 参考

- `egui-wgpu` 0.36.2 的 `CallbackTrait` / `RenderState` / `RendererOptions`：<https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/>
- egui 渲染通道的深度 `Clear(1.0)` / `StoreOp::Discard` 与 MSAA resolve：`egui-wgpu` 的 `paint_and_update_textures`
- wgpu 30 破坏性变更（`Maintain` 移除 → `PollType`、`DeviceDescriptor`、`multiview_mask`）：<https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md>
- `egui_dock` 0.21.1 要求 `egui ^0.36`（本项目因此钉 0.36.2）：<https://crates.io/api/v1/crates/egui_dock/0.21.1/dependencies>
