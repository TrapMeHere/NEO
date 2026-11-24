# NEO — Next Editor for OSM

<div align="center">

**Rust 原生、GPU-first 的 OpenStreetMap 编辑器** · 百万到千万级对象 · JOSM 风格界面

[![CI](https://github.com/TrapMeHere/NEO/actions/workflows/ci.yml/badge.svg)](https://github.com/TrapMeHere/NEO/actions/workflows/ci.yml)
[![Release](https://github.com/TrapMeHere/NEO/actions/workflows/release.yml/badge.svg)](https://github.com/TrapMeHere/NEO/releases)
[![Docs](https://github.com/TrapMeHere/NEO/actions/workflows/pages.yml/badge.svg)](https://trapmehere.github.io/NEO/)
[![License](https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.97-orange.svg)](Cargo.toml)

[文档站](https://trapmehere.github.io/NEO/) ·
[用户指南](docs/guide/quick-start.md) ·
[开发指南](docs/dev/architecture.md) ·
[Releases](https://github.com/TrapMeHere/NEO/releases) ·
[CHANGELOG](CHANGELOG.md) ·
[TODO](TODO.md)

</div>

![NEO 界面](assets/ui-josm-style.png)

## 特性

- **原生性能**：Rust + wgpu（DX12/Vulkan/Metal），1M 对象编辑视口场景重建 ~1.3 ms，
  拾取 8–22 µs（[实测](docs/14-performance-and-verification.md)）
- **完整数据核心**：SoA + Arena + 分代句柄、rstar 索引、并行 PBF 导入（~1.8M elem/s）
- **可靠编辑**：Command/Transaction/Undo/Redo、journal 崩溃恢复、引用完整性检查
- **多图层**：活动图层可编辑，停靠层只读渲染并持久化；新建/打开为图层、切换、显隐、删除
- **渲染**：wgpu 矢量地图、节点 LOD、按 OSM `layer` 分层绘制（隧道/桥/高架）、名称标签；桥梁实线外框、隧道虚线外框（隐式 layer ±1）、路堤金色梯状坡面 / 路堑砖红梳状齿线、过水路面水面
- **编辑工具**：选择/框选/绘制/删除/测距、拖拽移动、按点分割路径、合并首尾相连路径、反转/简化 way、节点粘合、对齐/等距分布/镜像/解粘/平行复制、标签表格（多选可批量改标签）
- **上传**：全量或只上传选中（自动带上新建依赖）、校验错误门禁、409 冲突处理
- **设置**：常规/语言/账户/编辑/校验/网络/存储/快捷键（可自定义重绑），持久化到 `neo-state.json`
- **关系编辑**：成员增删、排序、角色修改，操作可撤销
- **剪贴板**：复制/粘贴/创建副本（跨文档，自动重映射 ID）
- **查找**：`id:123`、`key=value` 或标签文本，结果可定位
- **校验器**：25 条 JOSM 风格规则（way 过短/过长/重复节点/非相邻重复/未闭合建筑/area 未闭合/重复 way/孤立节点/重复坐标/可疑标签/multipolygon 角色与 outer/自相交/闭合 way 节点不足/relation 缺 type/重复成员/非法 layer/layer 越界/oneway 非法/name 空 等），面板可运行并定位
- **预设**：构建期从 **id-tagging-schema** 与 **name-suggestion-index** 生成到 `assets/presets/`，运行期加载（不编进 exe）。标签面板默认可搜 schema；输入后同时搜品牌建议
- **MapCSS 样式**：默认 `neo.mapcss` + 子集解析器，按 tag/zoom 解析颜色/填充/线宽
- **OSM 账户与上传**：内置 OAuth 客户端 + PKCE，多账户保存/切换；上传 changeset（全量或只上传选中）与 409 冲突处理在「文件 → 上传…」；下载是独立窗口（框选 bbox / Overpass）
- **统一导出**：文件 → 导出… 选择 OSM XML / osmChange / PBF / 会话
- **自动保存**：journal 快照 + SQLite 快照库（WAL），启动可恢复
- **文件格式**：PBF / OSM XML / osmChange / GPX 读写，JOSM 会话 `.jos`/`.joz`
- **国际化**：中英文即时切换（`i18n/` + `neo-i18n`）；CJK/Unifont 放在旁路 `assets/fonts/`（不进 exe）
- **图层**：显示/隐藏、活动图层；非活动图层灰显 50% 且只读

> 状态：**v0.1.0 开发中**。阶段 2/3 完成，阶段 4–6 的代码已落地但 Gate 未全过（插件隔离通过，三平台安装包与 10M/1h 稳定性未复测）。详见
> [路线图](docs/16-roadmap-and-risks.md) 与 [TODO](TODO.md)。

## 下载与安装

从 [Releases](https://github.com/TrapMeHere/NEO/releases) 下载
`neo-desktop-vX.Y.Z-windows-x86_64.zip`，解压后运行 `neo-desktop.exe`。

```powershell
# 可选：校验 SHA256
Get-FileHash .\neo-desktop-v0.1.0-windows-x86_64.zip -Algorithm SHA256
```

从源码构建（Rust 1.97+，Windows 需 MSVC Build Tools + Windows SDK；预设生成需要 Node.js 22 与 `npm ci`）：

```bash
npm ci
cargo build --release -p neo-desktop
./out/cargo/release/neo-desktop
```

若机器上没有 Node，可先在有 Node 的环境执行 `node scripts/build-presets.mjs`，把
`assets/presets/schema.neo` 与 `suggestions.neo` 放进仓库再构建（build.rs 会复用已有文件）。
系统要求：Windows 10/11 x64（主要支持）；Linux/macOS 可源码构建。
更多见 [安装文档](docs/guide/installation.md)。

## 快速上手

1. 启动后地图为空。用 **文件 → 打开 PBF… / 打开 OSM XML…**，或工具栏下载 OSM 数据
2. `S` 选择、`A` 绘制、`X` 删除、`M` 测距；右键拖动平移，滚轮缩放
3. 左键拖拽移动节点/way；`Enter` 完成线；`Ctrl+Z/Y` 撤销/重做
4. `Ctrl+S` 保存 XML，或 **文件 → 导出 osmChange / PBF**
5. **帮助 → 设置…** 切换语言、查看快捷键

完整教程：[快速上手](docs/guide/quick-start.md) ·
[界面说明](docs/guide/interface.md) ·
[编辑操作](docs/guide/editing.md) ·
[FAQ](docs/guide/faq.md)

## 文档

| 入口 | 内容 |
|---|---|
| [文档站](https://trapmehere.github.io/NEO/) | 使用教程 + 开发指南 + 技术设计（mdBook） |
| [docs/introduction.md](docs/introduction.md) | 项目介绍与截图 |
| [docs/guide/](docs/guide/quick-start.md) | 安装、上手、界面、编辑、文件会话、设置、性能、FAQ |
| [docs/dev/](docs/dev/architecture.md) | 架构、workspace、构建测试、贡献、发布、路线图 |
| [docs/01](docs/01-system-architecture.md)–[18](docs/18-testing-strategy.md) | 技术设计文档 |
| [docs/adr/](docs/adr/) | 架构决策记录 |

文档站本地预览：

```bash
cargo install mdbook --version 0.4.52 --locked
mdbook serve --open
```

## 开发

```bash
npm ci
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace
cargo test --workspace
python scripts/check-deps.py
python scripts/check-i18n.py
```

测试已回到 `master`；`test` 分支保留历史测试基线，不再作为唯一测试来源。

- [贡献指南](CONTRIBUTING.md)（DCO、代码风格、clean-room 规则）
- [构建与测试](docs/dev/build-and-test.md)
- [Workspace 与 crate 职责](docs/dev/workspace.md)
- [发布流程](docs/dev/release.md)

## 项目结构

```
crates/
  neo-core      ID/坐标/句柄/Meta            neo-store    DataSet/索引/反向引用
  neo-osm       OSM 实体/变更/tag            neo-geo      相机/投影/拾取
  neo-scene     LOD/批处理/顶点              neo-render   wgpu 管线/GpuScene
  neo-io        PBF/XML/OSC                  neo-editor   命令/事务/历史/journal
  neo-tools     编辑动作                     neo-persist  会话 .jos/.joz
  neo-ui        egui 面板/主题               neo-i18n     文本目录
apps/
  neo-desktop   eframe + wgpu 桌面应用        neo-prototype 性能原型
docs/           技术设计 + 用户/开发指南（mdBook）
i18n/           zh-CN.json / en-US.json
.github/        CI / Release / Pages / 模板
```

依赖方向与不变量见 [docs/02](docs/02-workspace-and-dependency-rules.md) 与
[docs/dev/architecture.md](docs/dev/architecture.md)。

## 贡献

欢迎 Issue / PR！请先阅读 [CONTRIBUTING.md](CONTRIBUTING.md) 与
[行为准则](CODE_OF_CONDUCT.md)。安全问题请按 [SECURITY.md](SECURITY.md) 私密报告。

## 许可

- 代码：**Apache-2.0 OR MIT**（见 [LICENSE](LICENSE)、[LICENSE-MIT](LICENSE-MIT)、[LICENSE-APACHE](LICENSE-APACHE)）
- 文档：**CC-BY-SA-4.0**
- 第三方字体/图标与 OSM 数据署名：见 [NOTICE](NOTICE) 与 [assets/fonts/NOTICE.md](assets/fonts/NOTICE.md)
- OSM 数据 © OpenStreetMap contributors，ODbL 1.0
