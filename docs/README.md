# NEO 文档

> NEO = Next Editor for OSM：Rust 原生、GPU-first 的 OpenStreetMap 编辑器，目标完整替代 JOSM。

**快速入口**

- [项目介绍（文档站首页）](introduction.md)
- [用户指南](guide/quick-start.md)：安装、上手、界面、编辑、文件、设置、性能、FAQ
- [开发指南](dev/architecture.md)：架构、workspace、构建测试、贡献、发布、路线图
- [文档站](https://novolife.github.io/neo/)（由本目录 + `book.toml` 通过 mdBook 发布）
- [架构决策记录（ADR）](adr/)

> 本目录是**架构事实来源**；代码注释引用这里的文件与章节，两者不一致时以文档为准并修复代码。

## 阅读顺序

| 顺序 | 文档 | 内容 | 读者 |
|---|---|---|---|
| 1 | [00 愿景与范围](./00-vision-and-scope.md) | 定位、目标、非目标、成功度量 | 所有人 |
| 2 | [01 系统架构](./01-system-architecture.md) | 八层模型、线程/GPU 队列、端到端时序、不变量 | 所有人 |
| 3 | [02 Workspace 与依赖规则](./02-workspace-and-dependency-rules.md) | crate DAG、CI 强制、版本一致性、feature 策略 | 贡献者 |
| 4 | [03 数据模型与存储](./03-osm-data-model-and-store.md) | SoA/Arena/句柄、Tag 驻留、内存账本、DataSet | 数据/编辑开发者 |
| 5 | [04 空间索引与几何](./04-spatial-index-and-geometry.md) | 投影/相机坐标系、rstar、拾取、LOD | 几何/渲染开发者 |
| 6 | [05 wgpu 渲染器](./05-renderer-wgpu.md) | 管线、批次、增量上传、GPU 拾取、字体 | 渲染开发者 |
| 7 | [06 编辑核心与 Undo](./06-editing-core-and-undo.md) | Command/Transaction、Journal、冲突 | 编辑器开发者 |
| 8 | [07 并发与任务](./07-concurrency-and-tasks.md) | 单写者、快照发布、任务系统、死锁清单 | 所有人 |
| 9 | [08 IO：PBF/XML/osmChange](./08-io-pbf-xml-osc.md) | 导入管线、格式细节、写侧方案 | 数据开发者 |
| 10 | [09 OSM API 与同步](./09-osm-api-and-sync.md) | API 0.6、changeset、diff 上传 | 同步开发者 |
| 11 | [10 持久化与恢复](./10-persistence-and-recovery.md) | SQLite WAL、journal、autosave | 基础设施 |
| 12 | [11 样式、预设与 MapCSS](./11-style-presets-mapcss.md) | MapCSS 兼容、Presets、过滤器 | 样式/UI |
| 13 | [12 校验器](./12-validation.md) | 规则模型、增量校验、上传 gate | 数据质量 |
| 14 | [13 JOSM 功能对照](./13-josm-parity.md) | parity 矩阵、完成判据、易漏行为 | 产品/所有人 |
| 15 | [14 性能与验证](./14-performance-and-verification.md) | 指标、基准方法、实测基线 | 所有人 |
| 16 | [15 插件系统](./15-plugin-system.md) | WASM 宿主、权限、兼容策略 | 平台/插件作者 |
| 17 | [16 路线图与风险](./16-roadmap-and-risks.md) | 六阶段、Gate、风险登记册 | 所有人 |
| 18 | [17 许可与治理](./17-licensing-and-governance.md) | 许可选择、clean-room、资源合规 | 法务/维护者 |
| 19 | [18 测试策略](./18-testing-strategy.md) | 测试金字塔、golden、GPU 测试、CI | 所有人 |

## ADR（架构决策记录）

| 编号 | 决策 | 状态 |
|---|---|---|
| [0001](./adr/0001-rust-native-gpu-first.md) | Rust Native + GPU-first | accepted |
| [0002](./adr/0002-modular-monolith.md) | 模块化单体，而非微服务 | accepted |
| [0003](./adr/0003-soa-arena-storage.md) | SoA + Arena + generation 句柄存储 | accepted |
| [0004](./adr/0004-independent-data-render-editor-models.md) | 数据/渲染/编辑三模型分离 | accepted |
| [0005](./adr/0005-wgpu-egui-shared-device.md) | egui 与地图渲染共享 wgpu Device/Queue | accepted |
| [0006](./adr/0006-single-writer-editor.md) | 单写者编辑 + 不可变快照 | accepted |
| [0007](./adr/0007-wasm-plugins.md) | WASM 插件而非 Rust 动态库 | accepted |
| [0008](./adr/0008-command-transaction-undo.md) | Command + Transaction 作为唯一写路径 | accepted |
| [0009](./adr/0009-incremental-update.md) | 增量几何/索引/GPU 更新 | accepted |
| [0010](./adr/0010-mapcss-compatibility.md) | MapCSS 子集兼容策略 | accepted |
| [0011](./adr/0011-dependency-coherence-pinning.md) | 依赖一致性与锁版本策略 | accepted |

## 约定

- 简体中文，技术术语保留英文；文件名英文 kebab-case。
- 结构：摘要 → 目标/范围 → 设计 → 关键接口（Rust 签名）→ 数据流 → 权衡 → 风险 → 验收 → 参考。
- **不编造数字**：性能一律引用 [14](./14-performance-and-verification.md) 的实测表；未测的写 `【待实测】`。
- 外部论断必须给 URL；无法核实的标 `> 待验证`。
- 文档里的 crate 名、类型名、方法名必须与 `crates/*` 实际代码一致；改接口时同步改文档（PR 检查项）。

## 原型与 Gate 报告

- 性能验证原型：`apps/neo-prototype`（命令见根 `README.md`）。
- 实测输出：`out/prototype-bench.md`、`out/prototype-render.md`（由原型生成，可复现）。
- Gate 结论：[`prototypes/gate-1-prototype.md`](./prototypes/gate-1-prototype.md)。

## 许可

文档采用 **CC-BY-SA-4.0**；代码为 **Apache-2.0 OR MIT**。详见根目录
[LICENSE](../LICENSE) 与 [NOTICE](../NOTICE)。
