# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，
版本号遵循 [Semantic Versioning](https://semver.org/lang/zh-CN/)。

## [Unreleased]

### Added

- 按点分割路径工具：左侧工具栏/工具菜单「按点分割路径」，点击路径上的内部节点即分割经过它的所有路径，一次撤销
- 合并首尾相连路径：选中两条及以上端点相邻的路径，工具菜单「合并首尾相连路径」把整条链合成一条（自动处理方向，一次撤销）
- 多选批量修改标签：选中多个对象时标签面板切换为批量模式，可对全部选中对象添加/覆盖 `key=value`、删除所有对象共有的标签；改动通过一次事务提交，`Ctrl+Z` 一步撤销
- 渲染桥梁、隧道、路堤、路堑、过水路面：`neo-style` 从标签解析结构物语义（`bridge`/`tunnel`/`embankment`/`cutting`/`ford`）并给出外描边与坡侧符号，`neo-scene` 新增 casing 批次（桥实线外框、隧道虚线外框、过水路面水面）、桥 +1 / 隧道 −1 的隐式 `layer`、路堤金色梯状坡面（刻线 + 外缘坡脚线）与路堑砖红梳状齿线（形状与色相双重区分，不只靠朝向）；默认 `neo.mapcss` 补对应规则，MapCSS 新增 `casing-color` / `casing-width` / `casing-dashes`

### Fixed

- 挖洞面（建筑内院/中庭）在 z≈15.67–16 附近（刚进入 20px 内圈条带绘制）时洞被条带填实：现在只要洞没有完整落在条带内边界以内，就改画“实心 + 挖洞”，洞不会再被条带覆盖

### Removed

- 删除“过滤器”功能（视图 → 过滤器…、隐藏选中）：`neo-ui` 过滤器面板、应用层 `hidden` 状态与相关判断、`neo-scene::SceneBuilder::set_hidden`、对应用户可见文案与文档一并移除

### Changed

- 工具菜单按功能分组重排：选择/绘制、测距、路径（按点分割↔合并首尾相连、反转、简化）、节点（粘合↔解粘、对齐、等距分布、镜像成对）、面与其它几何
- 中文文案里的英文单词（way/node/tag 等）统一译为中文，包括 i18n 与代码内的提示文案
- 标签面板单选/多选统一，不再有“批量标签”标题或说明文案：多选按**并集**展示标签，值不一致或部分对象缺少该 key 时值列显示「……」，点击下拉选值并写入全部选中对象；编辑失焦即生效
- “同步”窗口改为**纯上传**（文件 → 上传…）：只保留上传本地修改 / 只上传选中与上传 409 冲突处理；窗口内的 bbox 下载按钮和冲突“重新下载”按钮移除。下载统一走独立的下载窗口（文件 → 下载… / 工具栏云下载，框选 bbox、Overpass）
- 框选改为**纯几何选择**：不再区分要素是否被渲染，缩放 LOD 隐藏的节点等只要落在框内（或与框相交）都会被选中；点选/悬停仍按渲染 LOD
- 道路连接路（`highway=*_link`）不再落到通用灰色：`motorway/trunk/primary/secondary/tertiary_link` 沿用所属等级的颜色与文字色，线宽收窄（约为本体的 55–60%），本体道路保持较粗
- 桌面启动不再加载 Monaco 或合成地图；默认为空，需用户打开文件或下载 OSM 数据。已移除「加载示例数据」菜单
- 字体与预设改为外置资源（`assets/fonts/`、`assets/presets/schema.neo`、`suggestions.neo`），不再 `include_bytes!`/`include_str!` 进 exe。发行包需一并分发；可用 `NEO_ASSET_DIR` 指定根目录
- 预设用字符串表二进制：schema 在第一次打开预设时加载，品牌建议留到第一次搜索

### Added (Phase 5–6)

- `neo-plugin`：wasmtime 50 core module 宿主（feature `wasm-plugins`，默认关闭）。三个插件语义：todo 面板、utilsplugin 加节点、buildings_tools 矩形建筑。命令走 `Editor::execute`。fuel、64 MiB 内存上限、能力求交。签名为 SHA-256，不是 Ed25519
- `neo-cli`：打开 OSM XML / GPX / 合成数据、场景瓦片缓存、预设覆盖表、TMS URL、本地更新清单比较。`plugins` feature 跑内置 WASM 演示
- `neo-io`：GPX 1.1 读写（航点 → 节点，轨迹点 → way）
- `neo-style`：MapCSS `canvas` / `dashes`，未知的 text/icon 属性记入警告；声明编辑；TMS/WMS URL 与 offset 数据库；22 个 JOSM 分组的 schema 前缀覆盖表
- `neo-scene`：LOD 抽样、有界瓦片缓存、rayon 并行构建。`profile` feature 默认关闭
- `neo-store`：坐标与有序 ID 的 zigzag + LEB128 差分
- `neo-render`：可选 `draw_indexed_indirect`；compute shader 把 `instance_count` 写成 0
- 打包脚本：`scripts/package.ps1`、`packaging/windows`、`packaging/linux`、`packaging/macos`。本轮没有执行，也没有打出安装包
- 桌面「工具」菜单打开生态窗口（插件名、TMS 示例、预设缺口）。窗口不加载 WASM

### Added (Phase 4, 进行中)

- `neo-validate`：12 条 JOSM 风格校验规则 + `Report` + 右侧校验面板（运行/统计/点击定位）
- `neo-style`：预设改为 npm 包 `@openstreetmap/id-tagging-schema` 与 `name-suggestion-index`，编译期调用 `buildIDPresets`。品牌建议只在搜索时出现；写入前删除过期的 `*:wikipedia`
- `neo-api`：OSM API 0.6 协议客户端（changeset 创建/关闭、diff 上传、map 下载、409/412 解析）+ MockTransport 测试
- `neo-editor`：`MergeCase` / `ConflictResolution` / 三方标签合并；6 类冲突测试
- `neo-editor`：`RecoveryReport`（frames/applied/skipped），恢复状态可观测
- `neo-style`：MapCSS 子集解析/编译 + 默认样式表接入 SceneBuilder
- `neo-persist`：SQLite WAL 快照库（journal 字节 + 统计，保留 10 条）
- `neo-api`：上传 changeset 端到端测试（含 409 后重试）
- 同步冲突：KeepLocal（版本对齐后重试）与 TakeRemote（重新下载）两种策略
- 固定 OAuth 客户端与回调；统一导出窗口（XML/osmChange/PBF/会话）；地图框选 bbox 下载 OSM 数据

### Planned

- 关系编辑增强、框选、校验器规则补齐
- 三平台安装包冷启动、10M 流式场景重测、1 小时会话内存曲线
- Ed25519 插件签名、MapCSS text/icon 真正绘制、桌面内嵌 wasmtime

## [0.1.0] - 2026-11-15

第一个可运行的 Windows 桌面版本（阶段 2/3 完成）。

### Added

- **数据核心**：SoA + Arena + 分代句柄的 OSM 数据模型、Tag 驻留、rstar 空间索引、
  CSR 反向引用、世界坐标/相机/拾取（`neo-core`、`neo-store`、`neo-geo`）。
- **IO**：PBF（含并行导入）、OSM XML、osmChange 读写；10M 对象导入与导出测试。
- **编辑核心**：Command/Transaction/Undo/Redo、选择集、journal（JSONL）恢复。
- **渲染**：wgpu 管线、批处理、增量上传、节点 LOD（z≤16 隐藏 / z17–18 共享节点点 /
  z19 全节点）、16px 拾取与拖拽外边距。
- **桌面 UI**：JOSM 风格布局（菜单、工具栏、竖排工具箱、右侧图层/标签/选择/关系面板、
  底部经纬度+比例尺+测距）、深色主题、Phosphor 图标。
- **图层**：显示/隐藏、活动图层；非活动图层 shader 灰度 + 50% 不透明度、只读。
- **设置**：语言（中/英）与快捷键一览；运行时 `NEO_LANG`。
- **i18n**：`i18n/zh-CN.json`、`en-US.json` + `neo-i18n` crate（编译期嵌入）。
- **测距工具**（M）：两点连线显示 m/km。
- **打包**：exe 图标（winres）、内置 Noto Sans CJK SC / GNU Unifont / Unifont Upper 兜底字体。
- **工程化**：GitHub Actions（CI / release / Pages）、mdBook 文档站、issue/PR 模板。

### Changed

- 界面由早期单面板改为 JOSM 风格布局；语言/快捷键合并到「设置」窗口。
- 渲染性能修复：拖拽不再每帧重建场景；缩放回退不再出现空白地图。

### Fixed

- 隐藏节点截胡 way 点击 → 拾取阶段过滤并回退到 way。
- 拖拽与选中解耦，避免每帧重建。
- 启动不再弹出控制台窗口（release GUI 子系统）。

[Unreleased]: https://github.com/TrapMeHere/NEO/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/TrapMeHere/NEO/releases/tag/v0.1.0
