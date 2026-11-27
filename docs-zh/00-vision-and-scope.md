# 00 · 愿景与范围（Vision & Scope）

> NEO —— **Next Editor for OSM**。用 Rust 原生重写的 OpenStreetMap 编辑器，目标是成为 JOSM 的完整替代，而不是一个"轻量地图查看器"。
>
> 本文回答"我们到底要造什么、不造什么、怎么算成功"；技术设计从 [01 系统架构](./01-system-architecture.md) 开始。

## 1 定位

| 维度 | 决定 | 反例（明确不做） |
|---|---|---|
| 形态 | **原生桌面应用**，单进程模块化单体 | Electron / Tauri + React 客户端；微服务 |
| 语言 | Rust（stable 1.100，MSRV 1.97） | 用 Rust 包一层 JVM |
| 渲染 | **wgpu GPU-first**，自研地图渲染器 | 只用 Java2D 式 CPU 光栅；只用 egui 画地图 |
| 数据 | 内存 SoA + 空间索引，SQLite 只做持久化/缓存 | 每帧查 SQLite；把 OSM 数据塞进通用对象树 |
| 编辑 | Command + Transaction + Undo/Redo，单写者 | GUI 直接改 store；多线程并发写对象 |
| 平台 | Windows / Linux / macOS 原生 | 只为 Windows 优化 |
| 规模 | 1M–10M+ OSM 对象可交互 | 只支持城市级小数据 |

## 2 目标（可验证）

1. **工作流等价**：一个 OSM 贡献者可以连续一周只用 NEO 完成 JOSM 的日常编辑（下载→编辑→验证→上传→冲突解决），不需要回到 JOSM，且不产生数据损坏。定义见 [13 JOSM 功能对照](./13-josm-parity.md)。
2. **规模**：千万级对象的导入、索引、平移/缩放、拾取、局部编辑都在交互预算内（目标见 [14 性能](./14-performance-and-verification.md)）。
3. **性能结构优于 JOSM**：不是"某指标更快"，而是消除结构性瓶颈——GPU 批渲染、视口决定绘制量、增量几何/索引更新、无 JVM 堆上限。
4. **可维护**：业务逻辑不依赖 GUI；渲染不依赖 UI 控件；数据模型不依赖渲染格式（ADR-0004）。
5. **可扩展**：WASM 插件 + 稳定宿主 API；样式/预设/校验规则尽量与 JOSM 资源兼容（ADR-0010、[15](./15-plugin-system.md)）。

## 3 非目标（第一版明确不做）

| 非目标 | 原因 | 替代 |
|---|---|---|
| JOSM Java 插件二进制兼容 | JOSM 插件链接内部 Java 类，无法跨语言 ABI 兼容 | WASM 能力映射（[15](./15-plugin-system.md)） |
| JVM 脚本插件（JSR-223/GraalJS） | 引入 JVM 运行时，违背原生定位 | 嵌入式脚本/WASM，标为 P2 |
| OSM API 0.7 | **0.7 只是 wiki 构想页，没有部署规范**；JOSM 自身硬编码 0.6 | 实现 API 0.6 |
| 浏览器 WASM 版本 | 数据规模与文件系统语义不符，收益低 | 桌面优先；WASM 仅作为远期实验 |
| 全量离线 OSM 星球（>100GB） | 不是编辑器场景 | 区域提取（PBF 子集） |
| 把 JOSM 的 Java 对象模型逐类翻译 | 会继承其内存/性能结构 | 重新设计 SoA 存储（[03](./03-osm-data-model-and-store.md)） |

## 4 用户与场景

- **日常贡献者**：改 POI、画建筑、修道路，需要快捷键、预设、校验、上传。
- **数据质量工作者**：Validator、Overpass/搜索、批量语义修正。
- **专业作图者**：关系（route/multipolygon）、点位精度、影像对齐、WMS/WMTS 底图。
- **插件作者**：需要稳定 API、事件、权限模型（WASM）。
- **自动化/CI 用户**：headless 导入、渲染 PNG、校验、差异对比（JOSM 有 `render/project/validate` CLI，NEO 提供对等能力）。

## 5 与 JOSM 的关系

- 行为语义以 JOSM r19678（"26.09"）为对照基线（见 [13](./13-josm-parity.md)）。
- **不复制代码**：clean-room 重写；规格先行（OSM XML/PBF/osmChange/MapCSS/预设 XSD 都是公开格式）。
- **资源隔离**：JOSM 仓库内的 presets/MapCSS/validator 规则/图标是 GPL 代码库的一部分；NEO 采用"运行时加载独立资产目录 + NOTICE/LICENSES"的合规路线（细节与逐文件许可风险见 [17 许可与治理](./17-licensing-and-governance.md)）。

## 6 成功度量（Gate 化）

阶段 1 的硬门槛（不达标不进入阶段 2，见 [16 路线图](./16-roadmap-and-risks.md) 的 G1）：

| 指标 | 目标 | 现状 |
|---|---|---|
| 百万对象：导入 + 索引 | 见 [14](./14-performance-and-verification.md) 的实测基线表 | 原型已实测，见 `out/prototype-bench.md` |
| 交互帧（视口裁剪后） | 60 FPS 目标，P95 场景构建 < 16 ms | 原型已实测 |
| 拾取 | P95 < 16 ms | 原型已实测 |
| 局部编辑（移动节点 + 关联 way + 索引） | P95 < 1 ms，且不随总对象数线性增长 | 原型已实测 |
| 千万对象 | 可导入、可平移缩放、无 OOM（31.8 GB 参考机） | 阶段 1 之后验证 |

## 7 术语

- **SoA**：Structure of Arrays，同一字段连续存放。
- **Handle**：`(index, generation)` 的内部句柄；`ObjectId` 是对外 OSM ID，二者分离。
- **快照（Snapshot）**：不可变的只读数据视图，供渲染/后台/插件读取。
- **Command**：可撤销的编辑操作。
- **SceneFrame**：CPU 侧一帧的绘制批次（顶点/索引/颜色）。
- **LOD**：按缩放/视口简化几何。
- **MapCSS**：JOSM 的样式语言（CSS 子集 + OSM 扩展）。
- **OSC**：实际指 `osmChange`（`version="0.6"`），不存在 "OSC 0.3"。
