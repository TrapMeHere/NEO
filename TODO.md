# NEO 开发进度台账

> 这个文件是**给项目负责人看的实时看板**：当前阶段、已完成/进行中/未开始、验收状态、下一步动作。
> 规则：每轮开发结束前更新本文件 + `git commit`；状态标记：`[x]` 完成、`[~]` 进行中、`[ ]` 未开始、`[!]` 阻塞。
> 架构契约见 [docs/README.md](docs/README.md)，路线图与 Gate 见 [docs/16-roadmap-and-risks.md](docs/16-roadmap-and-risks.md)。

## 0 总览

| 项 | 状态 |
|---|---|
| 当前阶段 | **阶段 5–6 已落地代码，Gate 未全过**。G5 = Conditional Go（插件隔离通过，三平台安装包未冷启动）。阶段 6 不是 RC。详见 [gate-5](docs/prototypes/gate-5-ecosystem.md)、[gate-6](docs/prototypes/gate-6-performance.md) |
| 上一次 Gate | G5 = **Conditional Go**；阶段 6 = **未达 RC**。G1 的三平台与 10 分钟稳定性仍未补 |
| 目标 Gate | 补 G5(d) 三平台安装包冷启动；在新的流式路径上重测 10M 与 1 小时会话 |
| 代码规模 | Rust ~19k 行（97 个源文件） |
| 测试 | **master 已恢复回归测试**（core/store/editor/io/persist/style/ui 共 40+ 项）；2026-11-21 体检修复后 `cargo test --workspace` 通过 |
| JOSM parity P0 | 52 条中 **~20 条达成/部分达成**；2026-11-21 补了关系成员编辑/框选/复制粘贴/查找/命令历史/GPX/简化/粘合。真实差距见 [docs/feature-parity.md](docs/feature-parity.md) |

## 1 本轮（阶段 2 → 3）任务看板

### 1.1 阶段 2 · OSM 数据核心（G2）

| # | 任务 | 状态 | 对应 crate | 验收 |
|---|---|---|---|---|
| 2.1 | OSM XML 0.6 读取（node/way/relation/tag/nd/member、负 id、action/visible） | [x] | `neo-io/xml.rs` | 单测 + Monaco/Bremen XML 往返 |
| 2.2 | OSM XML 写出（server-conformant / JOSM-conformant 两种模式） | [x] | `neo-io/xml.rs` | 往返语义等价测试 |
| 2.3 | osmChange 0.6 读取（create/modify/delete、if-unused、负 id 占位） | [x] | `neo-io/osc.rs` | create/modify/delete 解析 + apply |
| 2.4 | PBF 写 encoder（varint + zlib，osmpbf 只读的补充） | [x] | `neo-io/pbf_write.rs` | 写→osmpbf 读回语义一致 |
| 2.5 | 引用完整性检查（悬空 node/member，显式 incomplete） | [x] | `neo-store` / `neo-io` | 报告 0 错误或列出 incomplete |
| 2.6 | 导入可取消、失败不留半成品（构建完成再替换） | [x] | `neo-io` | 取消测试 + 原子性测试 |
| 2.7 | 10M PBF 写→读对拍（计数/完整性，真机实测） | [x] | `neo-prototype` | 基线记录进 docs/14 |
| 2.8 | 并行 PBF 解码（blob 级 rayon，Bremen 1.29× / Monaco 1.11×，目标 3× 未达成） | [x] | `neo-io` | 吞吐对比（目标 3×） |
| 2.9 | `.jos` 会话读写（XML；`.joz` = zip 容器） | [x] | `neo-io`/`neo-persist` | 往返测试 |
| 2.10 | GPX 读写（P1） | [x] | `neo-io` | 往返单测（阶段 5 补上） |

### 1.2 阶段 3 · 基础编辑（G3）

| # | 任务 | 状态 | 对应 crate | 验收 |
|---|---|---|---|---|
| 3.1 | Editor 提交管线（precheck → apply → undo 记录 → dirty → 版本递增） | [x] | `neo-editor` | 单测 |
| 3.2 | EditTx/Transaction 原子性（失败回滚，零副作用） | [x] | `neo-editor` | 属性测试 |
| 3.3 | History（undo/redo、拖动合并、内存上限） | [x] | `neo-editor` | `undo∘redo = id` 测试 |
| 3.4 | Selection（节点/way/relation 集合、toggle/clear） | [x] | `neo-editor` | 单测 |
| 3.5 | Journal（JSONL 帧、TxBegin/Commit、replay 丢弃半事务） | [x] | `neo-editor` | 截断文件 replay 测试 |
| 3.6 | 基础命令：AddNode/AddWay/MoveNode/SetTags/Delete | [x] | `neo-editor/commands.rs` | 每命令 apply+undo 单测 |
| 3.7 | 几何命令：SplitWay/MergeWay/ReverseWay（含 direction tag 修正） | [x] | `neo-editor/commands.rs` | 语义单测 |
| 3.8 | 编辑后索引/脏集合同步（移动节点 → 父 way bbox/索引） | [x] | `neo-editor` | 增量测试 |
| 3.9 | 工具集：选择/绘制/删除（工具状态机 → 命令） | [x] | `neo-tools` | 单测 |
| 3.10 | 标签编辑面板（增删改 tag → SetTags 命令） | [x] | `neo-ui` | 手工验证 |
| 3.11 | 桌面端集成：模式切换、快捷键、拖拽移动、撤销/重做、保存、journal 恢复 | [x] | `apps/neo-desktop` | 手工冒烟 |
| 3.12 | 一键拖动 = 1 条历史（预览不入历史，释放时单命令） | [x] | `neo-editor`/`neo-desktop` | 单测 + 手工 |
| 3.13 | 保存/导出：OSM XML / osmChange（PBF 导出在 2.4 完成后接入） | [x] | `neo-io` | 保存后重开一致 |
| 3.14 | 自动化验收脚本（随机编辑+undo/redo 一致性、事务失败零副作用、OSC 写→读→写语义等价） | [x] | `crates/neo-editor/tests/`、`crates/neo-io/tests/` | CI 可跑 |

### 1.3 已知缺口（本轮不覆盖，登记）

- [ ] 关系编辑（relation 成员增删排序）、multipolygon 组装与校验
- [ ] 冲突解决（MergeCase 六类）、上传 409 处理 → 阶段 4
- [ ] MapCSS/Presets 真实样式 → 阶段 4
- [ ] 校验器 43 条规则 → 阶段 4
- [ ] SQLite 持久化 / autosave 正式实现 → 阶段 4（本轮 Journal 用 JSONL 文件）
- [ ] PBF 并行解码与写侧性能优化
- [ ] 三平台 CI、Windows 安装包/签名、DX12 后端实测
- [ ] 10 分钟稳定性冒烟

## 2 当前实现清单（代码地图）

| crate | 状态 | 主要文件 |
|---|---|---|
| `neo-core` | ✅ 完成 | ids/handle/units/sym/meta/error/stats |
| `neo-osm` | ✅ 完成 | tags/element/change |
| `neo-store` | ✅ 完成（缺 compact/COW） | node/way/relation/tag arena/indexes/bitset/dataset |
| `neo-geo` | ✅ 完成（缺多投影/关系 bbox） | world/camera/spatial/geometry/picking/world_cache |
| `neo-io` | 🚧 本轮 | pbf(读)/xml(本轮)/osc(本轮)/synth |
| `neo-editor` | 🚧 本轮 | command/tx/history/selection/journal/commands |
| `neo-tools` | 🚧 本轮 | select/draw/delete 控制器 |
| `neo-ui` | 🚧 本轮 | 工具栏/图层/属性 + 标签编辑器 |
| `neo-scene` / `neo-render` | ✅ 阶段 1 | 批次/管线/离屏渲染/MapGpu |
| `neo-query` | ✅ 只读查询 | MapQuery/Snapshot |
| `neo-api` / `neo-persist` / `neo-validate` / `neo-plugin` | ✅ 已实现（2026-11 体检确认，不再“3 行占位”） | 见各 crate |
| `apps/neo-desktop` | 🚧 本轮 | 编辑集成 |
| `apps/neo-prototype` | ✅ | 导入/渲染/基准 |

## 3 验证方法（每轮结束必跑）

```powershell
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
# 其余检查脚本见 README “开发”段
```

## 4 风险与决策记录（本轮）

| 决策 | 理由 |
|---|---|
| Journal 用 JSONL（serde_json）而非 rkyv | 可读、可截断恢复、无需 unsafe；性能足够（编辑频率低） |
| 删除对象保留在 store 中并标记 `ChangeState::Deleted` | 上传 diff/撤销/保存 round-trip 都需要"被删对象仍可见于变更集" |
| 关系编辑/冲突解决推迟到阶段 4 | G3 只要求"自用可编辑"，G4 才要求上传闭环 |
| 工具状态机放 `neo-tools` 而非 desktop | 可单测、可被未来 CLI/脚本复用 |

## 5 下一步（下一轮）

1. 完成 2.1/2.2 XML 往返 + 2.3 osmChange 读取
2. 完成 3.1–3.7 命令与 History/Journal 测试
3. 桌面端端到端：绘制一条 way → 改 tag → 拖动节点 → undo/redo → 保存 XML → 重开一致
4. 跑 G2/G3 自动化验收脚本并回填本文件

---

最后更新：2026-11-16 · 维护者：NEO Lead Agent


## 6 变更日志（新→旧）

### 2026-11-16 · 阶段 5–6

- [x] `neo-plugin`：wasmtime 50 宿主、三个插件语义、能力求交、fuel / 64 MiB 上限。`cargo test -p neo-plugin --features wasm-plugins` 11 项通过
- [x] GPX 读写、MapCSS 警告与样式声明编辑、TMS/WMS/offset、预设分组覆盖表（22 组计数都大于 0）
- [x] CLI、桌面生态窗口、三平台打包脚本（脚本未执行）
- [x] 流式 LOD、有界瓦片缓存、并行场景构建、坐标差分 varint、indirect draw + GPU 清零实例数（64×64 三角形像素一致，随后全黑）
- [x] 预设改为 npm：`@openstreetmap/id-tagging-schema` 7.3.0（1678 条）与 `name-suggestion-index` 8.0.20261101（40778 条品牌建议）。生成到 `assets/presets/`，运行时加载，不进 exe
- [x] 字体外置到 `assets/fonts/`；`cargo bloat` 基线见 `docs/prototypes/binary-size-baseline.md`
- [ ] 未做：三平台安装包冷启动、10M 新路径、1 小时会话、Ed25519、release 剖析

### 2026-11-15 · 阶段 2/3 第一刀（进行中）
- [x] `neo-io/xml.rs`：OSM XML 0.6 流式读取（node/way/relation/tag/nd/member、负 id、visible/action、实体解码）+ 两种写出模式（Server/JOSM-conformant），6 个单测（含往返语义等价）
- [x] `neo-editor`：`Editor`/`EditContext` 单写者事务、`Command` 契约、`History`（命令批 + 拖动合并）、`Selection`、`Journal`（JSONL 帧 + `committed_prefix` + `recover` replay）
- [x] `neo-editor/commands.rs`：AddNode / AddWay / MoveNode / SetTags / SetWayNodes / DeleteObject（引用检查），每命令 apply+undo；`undo∘redo = id`、事务失败零副作用、拖动合并、journal replay 均有测试
- [x] `neo-store`：`set_change_state_visible`（删除对象保留在 store 中，供上传 diff/撤销）
- [ ] 待下一轮：osmChange 读取、PBF 写 encoder、neo-tools 控制器、neo-ui 标签面板、desktop 编辑接线（画点/画线/拖动/撤销/保存）、G2/G3 验收脚本


### 2026-11-15 · 阶段 2/3 第二刀（本轮）
- [x] `neo-io/osc_read.rs`：osmChange 0.6 **读取与应用**（create/modify/delete、负 id、`visible`、缺失对象计数），3 个单测
- [x] 修复 `neo-io/xml.rs` 的**坐标解析 bug**（XML/OSC 的 lat/lon 是十进制度，原实现按 1e-7 整数解析导致静默变 0）
- [x] `neo-editor/commands_geom.rs`：SplitWay / MergeWay（四种方向自动归一）/ ReverseWay（`oneway=yes ↔ -1` 方向 tag 修正），6 个单测
- [x] `neo-editor` 修复 **redo 的本地 ID 稳定性**（AddNode/AddWay 重做复用首次分配的 ID，否则 undo→redo 会换 ID）
- [x] `neo-tools`：选择/绘制/删除工具状态机（吸附已有节点、逐点画 way、删除引用检查、拖动单历史），5 个单测
- [x] `neo-ui`：**标签编辑面板**（增删改 tag → `apply_tags` 意图）、工具栏（工具切换/撤销/重做/打开 PBF+XML/保存 XML/导出 OSC/示例/完成线）、属性面板扩展（way 的分割/反转按钮）
- [x] `apps/neo-desktop`：编辑闭环 —— Editor 集成、S/A/X 快捷键、Ctrl+Z/Y/S、Enter、Delete、**拖拽移动节点（预览不入历史，释放提交单条 MoveNode）**、标签编辑、保存 XML/导出 OSC、**journal 启动恢复 + 手动恢复按钮**
- [x] G3 验收集成测试 `crates/neo-editor/tests/g3_acceptance.rs`：随机 1000 次编辑 + 随机 undo/redo 与"从头重放"状态一致；事务失败零副作用；journal 尾部半事务被丢弃
- [x] 桌面冒烟：`neo-desktop.exe` 启动 10s 存活（加载 Monaco、创建 journal、wgpu 首帧）
- [x] G2 验收补充：`crates/neo-io/tests/osc_roundtrip.rs`（写→读→再写，动作分组语义等价，含负 ID/删除对象）
- [x] `neo-tools` 补 `merge_ways`（端点相邻自动归一 + 失败信息）
### 2026-11-15 · 阶段 2 收尾（本轮）
- [x] **PBF 写 encoder**（`neo-io/pbf_write.rs`）：protobuf wire 编码 + zlib、节点/Way/Relation、string table、sint64 delta refs；关键坑：`osmformat.Node.id` 是 **sint64(zigzag)**（Way/Relation.id 是 int64），写错会让读回 ID 变负、way 全部悬空
- [x] **G2 验收**：`crates/neo-io/tests/pbf_roundtrip.rs`（写→osmpbf 读回：计数/坐标/way 顺序/relation 成员顺序与角色/tags/meta 全等）+ 压缩/非压缩/空数据集
- [x] **引用完整性检查**（`neo-store/integrity.rs`）：way→node、relation→member 悬空检测 + 报告；2 个单测
- [x] **导入取消语义**（`ImportOptions.cancel`，每 4096 元素检查一次，取消返回 `NeoError::Cancelled` 且无半成品）：`tests/import_cancel.rs` 2 个测试
- [x] 桌面端「导出 PBF」、原型 `pbf-roundtrip` 子命令（写→重读→计数/完整性校验）
- [ ] 待下一轮：10M 对拍（2.7）、并行 PBF 解码（2.8）、`.jos/.joz`（2.9）；关系编辑/多选/合并 UI


### 2026-11-15 · 阶段 2 并行导入 + 10M 对拍（本轮）
- [x] **并行 PBF 解码**（`neo-io/pbf_parallel.rs`）：BlobReader 顺序取 blob → rayon 并行 zlib+protobuf 解码为 Raw DTO → 单线程按 Node→Way→Relation 装配；支持取消与引用策略
- [x] 真机基准（release，i5-12400F / RTX 3070 / Windows 11）：
  - Bremen（1,668,778 N / 329,676 W / 5,053 R，20.3 MiB）：顺序 1.163s → 并行 **0.899s（1.29×）**
  - Monaco：顺序 0.028s → 并行 0.026s（1.11×）
  - 与顺序导入逐对象一致（计数/ID 集合/way 引用顺序/relation）；**目标 3× 未达成**，瓶颈是单线程装配（string interning + ID 索引插入），记录于 docs/14
- [x] **10M 合成数据 PBF 写→读对拍**：10,001,406 节点 / 3,129,589 ways / 78,210 relations → 86.65 MiB PBF（2,904 blobs）→ 重读计数/节点引用/成员/tag 全部一致，`check_integrity` 0 issues，内存 1,532 MiB
- [x] 原型新增 `compare-import`（顺序 vs 并行）与 `pbf-roundtrip --synth <n>` 子命令
- [ ] 待下一轮：`.jos/.joz` 会话文件（2.9）、关系编辑/多选/框选、合并 UI 接线；并行装配（分区 DataSet）作为后续优化


### 2026-11-15 · 会话文件 + 多选/关系编辑（阶段 2/3 收口）
- [x] **`.jos` / `.joz` 会话**（`neo-persist/session.rs`）：JOSM 会话 XML（viewport/layer 引用/active-layer/selection），`.joz` 为 ZIP（session.jos + 嵌入图层数据，读取时解压并改写路径）；**容忍未知插件图层段/属性**；3 个单测（往返/未知段/zip 嵌入解压）
- [x] 桌面端「保存会话 / 打开会话」（`.jos` 与 `.joz` 自动按扩展名选择；恢复相机视口与选择集）
- [x] **多选**：Shift+点击追加选择；批量删除（逐个命令，失败不中断并报告成功数）
- [x] **合并 UI**：选中两条端点相邻 way 后出现「合并选中的两条 way」按钮（`MergeWay`）
- [x] **关系编辑基础**：属性面板显示 relation 成员数/版本/tags 可编辑，删除关系走 `DeleteObject`（引用检查）
- [x] 测试：`shift_click_extends_selection`、session 3 项；全仓 111 测试通过
- [ ] 后续（阶段 4+，非本目标）：GPX、校验器、MapCSS、预设、上传/冲突、SQLite autosave、框选、way 节点批量拖拽


### 2026-11-15 · UI 模拟手测（截图验证 + 4 个真实缺陷修复）
用 PowerShell + Win32 模拟输入（`scripts/ui_check.ps1`）对 release 版桌面程序做端到端 UI 检查，截图存 `out/ui-check/`。
**发现并修复的缺陷**：
- [x] **中文全部显示为方块（tofu）**：egui 默认字体不含 CJK → 启动时加载系统字体（`simhei.ttf` 等），中英文混排正常
- [x] **操作反馈不可见**：`status` 消息从未进入工具栏/左面板 → `status_line()` 追加最近动作（"已撤销/已重做/已放置起点…"）
- [x] **UI 滞后一步**：动作在面板绘制后才执行且未请求重绘 → 撤销/重做后界面要等下个输入才刷新；已 `request_repaint()`
- [x] **初始视图被稀疏越界 way 拉到 z11**（Monaco 只有 0.5° 内的道路密集区 + 少量跨境线）→ 改 2%~98% 分位点拟合，初始 z15.1
**验证通过的交互**（截图证据）：启动加载 Monaco（41844/6281/222）、绘制工具 A+点击建 way+Enter 结束、Ctrl+Z×3 / Ctrl+Y×3 撤销重做计数与状态正确、S 选择节点后右侧属性+标签编辑面板、删除被引用节点被正确拒绝、鼠标滚轮缩放（15.49→15.82→15.49）、工具栏「示例数据」按钮生效
**未覆盖（需人工/GUI 自动化补）**：原生文件对话框（保存/打开 XML/OSC/PBF/会话）、Shift 多选与合并按钮、checkbox 点击（坐标未对准）


### 2026-11-15 · 渲染卡顿修复（50× 减少重建/上传）
**用户反馈**：拖动/缩放时"渲染太慢"。用 `scripts/ui_check.ps1` + `NEO_PERF_LOG` 打点实测（`out/ui-check/perf-*.log`）：

| 场景（synth:200000，拖拽节点 ~1s） | 修复前 | 修复后 |
|---|---:|---:|
| 场景重建次数（scene build） | 78 次（每帧一次，14–15 ms/次） | **2 次**（启动 + 释放提交，各 ~15 ms） |
| GPU 全量上传次数（1.86 MB/次） | 78 次 | **2 次** |
| 慢帧（>8 ms） | 全部帧 | **2 帧** |

**根因**：拖动节点时 `preview_move_node()` 每个 mousemove 都写入 DataSet → `version` 递增 → `ensure_scene`
整场重建 + `GpuScene` 全量重传（Monaco 3.9 MB / synth 200k 1.9 MB），即每帧 10–20 ms。
**修复**：
- 拖动预览与 DataSet 解耦：仅保存 `DragState.current`，用 egui 叠层画"邻居连线 + 橙色目标点"（`Editor::node_neighbors`）；
- 释放鼠标时只执行一次 `MoveNode`（一次重建、一条历史）；
- `MoveNode` 现在会把节点标记 `Modified`（修复"脏对象=0"、上传 diff 漏对象）；
- 顺带修复：双击启动时 CWD 不是仓库根 → 找不到 Monaco 而回退 synth:200000；改成 exe 相对路径查找；
- 新增 `NEO_SYNTH_N`（强制合成数据）与 `NEO_PERF_LOG`（应用内写性能日志，避免 shell 重定向卡死）；
- `scripts/ui_check.ps1` 增加 drag / truepan / zoomloop 步骤与截图。

实测修复后 Monaco：整段"启动+拖拽+平移"仅 5 次重建/5 次上传、1 个慢帧（启动 8.6 ms）。


### 2026-11-15 · 鼠标操作改为「左键选择/拖动元素，右键拖动背景」
- 左键点击 = 选择；左键拖动节点 = 移动（松开提交 1 条历史）；左键拖空地 = 无操作（预留给后续框选）
- 右键拖动 = 平移地图；滚轮 = 以鼠标位置缩放不变
- 左侧快捷键面板与 README 同步更新
- 自动化验证（scripts/ui_check.ps1）：drag（左键拖节点，撤销 1/脏对象 1）→ panright（右键拖动，视图平移且无编辑）→ emptyleft（左键拖空地，截图与平移后逐像素一致 = 无副作用）


### 2026-11-15 · 节点渲染 LOD（去掉开关，按 zoom 分档 + 形状区分）
- 移除工具栏「显示节点」复选框与 `SceneConfig.node_zoom_threshold`，节点显示改由 zoom 决定
- `neo-scene`：`NODE_MIN_ZOOM=13`、`NODE_ALL_ZOOM=16`、`SHARED_NODE_MIN_WAYS=2`；
  `SceneBuilder::build` 增加 `&Referrers` 参数（共享节点判定）
- 形状：普通节点 = 小圆点（六边形近似）；共享节点在 z13–15 = 小圆点，z16+ = 小方框
- 验证（Monaco，44 个字段全等测试 + 实测统计）：
  - z12 `nodes=0`；z13/14/15 只画共享节点（实测均 9135 个）；z16 画全部可见节点（29107 个）
  - 单测 `node_lod_follows_zoom` 校验三档计数与 Point 批次顶点数 = 共享×4 + 普通×7
- 截图：`out/ui-check/21-z13-shared-nodes.png`（工具栏已无复选框）、`22-z16-all-nodes.png`
- 新增 `NEO_START_ZOOM` 环境变量（截图/测试时指定初始 zoom）


### 2026-11-15 · 修复「放大到 z19 再缩小后地图变空」
- 根因：场景重建条件不含 zoom 变化 → 缩小后仍渲染高倍率时裁剪过的批次；且 GPU 上传 key 只有 (data version, origin)，
  缩放重建不会重传
- 修复：`ensure_scene` 在 `|Δzoom| > 0.5` 时重建；`GpuScene` 上传 key 增加 camera.zoom
- 回归：`scripts/ui_check.ps1 zoomcycle`（z16→z19+→z13→z16），截图 23/24/25；全程仅 10 次重建/上传，最终画面正常
- 相关文档：docs/14 §13.1


### 2026-11-15 · 节点 LOD 阈值调整（z16 及以下不显示）
- `NODE_MIN_ZOOM` 13 → **17**；`NODE_ALL_ZOOM` 16 → **19**
- 新规则：z≤16 不显示节点；z17–18 只显示共享节点（小圆点）；z19+ 显示全部节点
  （共享节点 = 小方框，普通节点 = 小圆点）
- 同步更新单测 `node_lod_follows_zoom` 与 docs/05 节点 LOD 表格


### 2026-11-15 · 选中红色高亮 + 选中后直接拖动
- 选中元素绘制红色高亮（节点 = 红圈；way = 红色折线边框），完全不重建场景（egui 叠层）
- **选中后无需长按**：左键在任意位置（含空白处）拖拽即移动选中的节点/way（way = 整体平移其全部节点）；
  拖动中橙色预览，松开时 `execute_batch` 一次提交 = **1 条历史 + 1 次重建**
- 未选中时：左键拖节点 = 移动该节点；左键拖空白 = 无操作（留给框选）；右键拖动 = 平移地图
- `neo-tools::move_nodes_batch` + 单测（批量移动后 undo 一次全部复位）
- UI 自动化验证：`28-select-node`（红圈）、`29b-drag-from-empty`（选中节点后从海面拖动仍移动选中节点）、
  `35-select-way-hook`（way 红色边框 + 属性面板 1302 节点）、`36-drag-way-hook`（拖动整体平移 1302 节点，撤销 1）
- 新增测试钩子 `NEO_TEST_SELECT=way`（选中视口内节点最多的 way，便于自动化验证）


### 2026-11-15 · 不可见节点禁止选中/拖动（拾取与渲染共用 LOD 规则）
- 抽出 `neo_scene::node_visible_at(zoom, way_ref_count)`（z<17 不可见；17–18 仅共享节点；≥19 全部），
  `SceneBuilder` 与桌面拾取逻辑共用，避免"看得见/点得到"不一致
- `pick_at` 过滤不可见节点：节点被隐藏时点击不会选中它（光标下的 way 仍可被选中）
- 缩放后自动清理已隐藏的节点选择（`prune_hidden_node_selection`；way/relation 选择保留），红色高亮同步消失
- 单测 `node_visibility_rule_boundaries`（16.99/17/18.5/19 边界）
- 自动化验证：
  - z19：`41a-node-selected-z19`（node 21911894 选中，红圈）
  - 缩放到 z18 后：`41b-node-selection-pruned`（该节点变为不可见 → 选择被清理，右侧显示未选择）
  - z16：`37-z16-pick`（点击原节点位置不再选中节点）
- 测试钩子补充 `NEO_TEST_SELECT=node`（选中视口内第一个可拾取节点）


### 2026-11-15 · 选中要素加"可拖动外边距"（16px）
- 之前：选中后从任意位置拖拽都会移动（过于宽松，容易误拖）
- 现在：拖动起点必须落在选中要素的**屏幕外边距 16px** 内（节点=点距离，way=折线线段距离）；
  远离要素拖动不会移动；未选中节点时直接命中节点仍可拖动该节点
- `app.rs` 新增 `selection_screen_distance` + `point_segment_distance`，鼠标附近才进入选中集合拖动
- 帮助文案/README 同步


### 2026-11-15 · 选中拖动外边距（16px）验证
- 实测（`scripts/ui_check.ps1` + `NEO_TEST_SELECT=node` 选中 z19 节点）：
  - 在节点附近（≤16px）按下拖动：`撤销 1`、`移动 1 个节点`、节点坐标更新 ✅
  - 在远处（数百 px 外空白处）按下拖动：`撤销 0`、无任何移动/编辑 ✅
- 关键修复：拖拽起点必须用 **按下原点**（`pointer.press_origin()`）而不是拖拽阈值触发时的当前指针位置，
  否则指针已移动 ~18px 会误判超出 16px 外边距（首次实现曾导致"附近也拖不动"）。


### 2026-11-15 · 外边距同时作为"可选择范围"（与拖动边距统一，但按元素类型分开）
- `HitTester::pick_with_margins(camera, screen, node_margin, way_margin)`：
  - 节点 16px / way 16px **独立边距**；节点边距内点击优先选节点（不会误选压着的 way），否则选 way
  - 可选范围与拖动范围一致（都在 16px 外边距内）
- 桌面 `pick_at` 改用该接口；不可见节点仍被过滤（与渲染 LOD 一致）
- 单测 `pick_with_margins_separates_nodes_and_ways`：长 way 中点选 way；节点附近 10px 偏移选节点；远处不命中
- UI 验证：`45b-margin-click-node`（点击距节点 10px → 选中 node 21911894）


### 2026-11-15 · 修复"点击要素本身选不中、点旁边几像素反而能选中"
- 根因：`pick_with_margins` 先做节点优先拾取，返回隐藏节点后应用层再按 LOD 过滤掉，
  **way 的回退候选已经丢失** → z16 及以下节点全部隐藏时，点击压在隐藏节点边距内的道路直接无选中
- 修复：新增 `HitTester::pick_with_margins_filtered(..., accept_node)`，在**拾取阶段**过滤隐藏节点并回退 way；
  桌面 `pick_at` 改用该接口（`node_selectable` 作为过滤条件）
- 单测 `filtered_pick_falls_back_to_way_when_node_hidden`（不过滤→节点；过滤→way）
- UI 回归：z16 点击道路像素（之前无选中 `46-bug-z16-click-road`）→ 现在正确选中 way 1424758490（`47-fixed-z16-click-road`）


### 2026-11-15 · 去掉 release 版控制台窗口
- `apps/neo-desktop/src/main.rs` 增加 `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`
- 验证：release exe 的 PE Subsystem = **2 (GUI)** —— 双击不再弹出黑色控制台；debug exe 仍为 3 (console)，方便 `cargo run` 看 tracing 日志
- 因为没有控制台，补充启动失败兜底：`rfd` 错误对话框 + `out/neo-startup-error.txt`
- 日志：release 下用 `NEO_PERF_LOG=out/perf.log` 写文件；README 已更新


### 2026-11-15 · 拖动前自动选中（修复"拖了但没选中" / "way 不先选中就不能拖"）
- 按下拖动时若命中的要素**未选中**，先 `select_only` 把它设为选中（红框立即出现），再开始拖动
- 拖动 range = 当前选中节点 + 选中 way 的全部节点（新增 `collect_selection_nodes`）
- 松开后保留选中状态并 `request_repaint()`，红色高亮显示在移动后的位置
- 实测：
  - z19 无预选、直接拖节点 → `撤销 1`、`移动 1 个节点`、node 选中红圈（48a）
  - z16 无预选、直接拖 way → `撤销 1`、`移动 15 个节点`、way 选中红框（48b）
- UI 帮助/README/docs/05 更新


### 2026-11-15 · 界面重构为 JOSM 风格（现代化）+ Phosphor 图标
- `crates/neo-ui` 重写：顶部**菜单栏**（文件/编辑/视图/工具/帮助）+ **图标工具栏**、左侧**图层面板**、
  右侧**属性/标签面板**、底部**状态栏**（中心坐标/缩放/选中数/撤销重做/最近操作）
- 图标：开源 **Phosphor Icons**（`egui-phosphor` 0.14，MIT）；字体安装与中文 CJK 字体合并到 `install_fonts`
- 深色主题（`apply_modern_style`）：JOSM 蓝强调色、统一间距/字号/按钮高度，地图画布居中
- 帮助窗口（帮助 → 快捷键与关于）列出快捷键与图标/数据授权
- 修复：面板被长标签撑宽导致地图被挤扁（宽度约束 + 标签截断）；状态栏长文本与左侧指标重叠（截断）
- 截图：`assets/ui-josm-style.png`（README 引用）


### 2026-11-15 · exe 图标 + 内置全 Unicode fallback 字体
- Windows exe 图标：`build.rs` 用 `rc.exe` 嵌入 `assets/neo.ico`；窗口/任务栏图标用 `assets/neo-icon.png`
  （验证：`Icon.ExtractAssociatedIcon(exe)` 得到 32×32 NEO 图标）
- 内置 fallback 字体（随 exe 分发，商业可用开源许可，OFL）：
  - Noto Sans CJK SC（中文/日文/韩文）
  - GNU Unifont（BMP 全量兜底）+ Unifont Upper（SMP/Plane2/3）
  - 字体链：CJK → Phosphor 图标 → 默认字体/emoji → Unifont → Unifont Upper → 系统 CJK
- 帮助窗口新增"字体覆盖示例"，用于人工检查是否还有方框；exe 体积 15.8 → **43.2 MB**（字体约 26 MB）
- 许可证/来源：`assets/fonts/NOTICE.md` + `OFL-1.1.txt`


### 2026-11-15 · 界面按 JOSM 重构（左侧工具栏 / 右侧面板堆叠 / 底部测距）
- 左侧改为**竖排工具栏**（选择/绘制/删除/测距 + 完成线/分割/反转/合并）
- 右侧改为 JOSM 式**面板堆叠**：图层、标签/成员、选择、关系、校验结果、冲突
- **图层信息移到图层右键菜单**：右键图层 → 信息…（窗口显示计数/版本/脏对象/视口/选中）、缩放至图层、另存 XML
- 底部状态栏：鼠标经纬度（悬停）、比例尺（按 m/px 自适应 m/km）、缩放、**测距结果**、选中数、撤销/重做
- 新增**测距工具（M）**：两次点击显示黄色连线 + 距离标签，状态栏同步显示
- 帮助窗口/README 更新；测试钩子 `NEO_TEST_MODE=measure|draw|delete`、`NEO_SHOW_LAYER_INFO=1`
- 自动化截图验证：布局 `54-josm-layout.png`、测距 `60-measure.png`、图层信息 `52-layer-info.png`


### 2026-11-15 · 右侧面板按 JOSM 细节调整
- 图层行精简：**眼睛图标**（显示/隐藏图层，隐藏后地图画布留空）+ **数据图层活动开关**（对勾方框）+ 图层名；右键菜单保留信息/缩放/另存
- 标签/成员：**表格直接显示全部标签**（key = value），way 节点/relation 成员列表显示在框底部；无选择时整个框留空
- 选择框：**无选择时不显示任何内容**（去掉占位文字）
- 关系框：**直接列出全部 relation**（id + name），为空时留空
- 校验结果/冲突：无数据时只保留标题，**不显示占位文字**；移除右侧"从 journal 恢复"按钮（菜单里已有）
- 测试：`NEO_LAYER_HIDDEN=1` 验证隐藏图层；截图 `65-layer-hidden.png` / `66-panel-final.png` / `63-right-panel-selected.png`


### 2026-11-15 · NVIDIA Overlay 弹窗说明
- 启动时出现的「点击 Alt+Z，打开 NVIDIA 信息浮窗」来自 **NVIDIA App/GeForce Experience Overlay**，不是 NEO 弹窗
- NVIDIA Overlay 为驱动级全局注入，无 per-app 关闭开关（官方论坛确认）；README FAQ 给出关闭步骤：
  NVIDIA App → 设置 → 功能 → 信息浮窗 → 关闭；或 GFE → 设置 → 常规 → 游戏内覆盖 → 关闭
- 已实测：杀 `NVIDIA Overlay` 进程后启动不再弹出，但 NVIDIA 会自动重启进程，故不推荐脚本化；正确做法是关闭 Overlay 设置


### 2026-11-15 · 界面文本抽离到 i18n/
- 新增仓库根目录 **`i18n/`**：`zh-CN.json`（默认）/ `en-US.json` + `README.md`
- 新增 crate **`neo-i18n`**：编译期 `include_str!` 嵌入、`t()`/`tf()` 占位符、缺失回退、
  `NEO_LANG` 环境变量与运行时 `set_language`
- `neo-ui`（菜单/工具栏/面板/状态栏/帮助/图层信息）与 `neo-desktop`（状态消息/错误/模式名/窗口标题）全部改为 i18n key
- 菜单新增 **语言 / Language**（中文 / English），切换即时生效
- 校验：`cargo test -p neo-i18n`（双语 key 对齐 + 回退 + 占位符）、`python scripts/check-i18n.py`
- 截图：`70-en-ui.png`（English）/ `71-zh-ui.png`（中文）


### 2026-11-15 · 图层只读 + 设置窗口
- 图层「活动开关」取消后：**GPU shader 对数据去色 + 50% 不透明度**，不是灰遮罩；不显示文字提示；禁止选择/拖动/绘制/测距
  （平移/缩放可用），同时清空选择；图层名置灰
- 语言与快捷键集中到 **设置窗口**（入口：**帮助 → 设置…**）；顶层「语言 / Language」菜单移除
- 设置窗口：语言切换（中文/English）+ 快捷键一览（含鼠标操作），并注明快捷键自定义在后续版本
- 帮助窗口保留「关于 NEO」、字体覆盖示例与许可证信息
- 测试钩子：`NEO_LAYER_INACTIVE=1`（只读图层）、`NEO_SHOW_SETTINGS=1`（设置窗口）
- 截图：`74-settings.png`、`75-layer-inactive.png`、`77-inactive-select-test.png`


### 2026-11-15 · 开源工程化：文档站 + CI / Release / Pages
- 文档按开源项目范式重组：
  - 根目录：`LICENSE`（Apache-2.0 OR MIT）、`LICENSE-MIT`、`LICENSE-APACHE`、`NOTICE`、
    `CONTRIBUTING.md`、`CODE_OF_CONDUCT.md`、`SECURITY.md`、`CHANGELOG.md`
  - `docs/guide/`：安装、快速上手、界面、编辑、文件与会话、设置/i18n、性能、FAQ
  - `docs/dev/`：架构、workspace/crate、构建测试、贡献、发布、路线图
  - `docs/SUMMARY.md` + `book.toml`（mdBook）+ `theme/neo.css`，技术文档/ADR 一并上线
- GitHub Actions：
  - `ci.yml`：fmt / clippy / test / i18n / deps / release 构建 / mdBook 构建
  - `release.yml`：tag `v*` → 校验 tag 与 Cargo 版本一致 → 测试 → Windows zip + SHA256 → GitHub Release
  - `pages.yml`：push master（docs / theme / book.toml 变更）→ mdBook → GitHub Pages（`https://trapmehere.github.io/NEO/`）
- GitHub 模板：issue（bug/feature）、PR 模板、Dependabot（cargo + actions，分组）
- 清理 `scripts/`：只保留 `ui_check.ps1` / `bench.ps1` / `check-*.py` 等长期工具
- README 重写为开源项目首页（徽章、特性、下载、文档入口、贡献、许可）


### 2026-11-15 · 阶段 4（专业编辑）第一部分
- **neo-validate**：12 条规则（增加 JOSM 风格：孤立节点、重复坐标、可疑标签组合、multipolygon 角色、way 自相交）+ `Report` + 12 个单元测试
- 右侧「校验结果」面板：运行校验、错误/警告统计、问题列表、点击选中并定位对象（测试钩子 `NEO_RUN_VALIDATION=1`）
- **neo-style 预设**：改用 **id-tagging-schema**（约 1500 条，中英文名，ISC license）+ 标签面板可搜索下拉
- **neo-api**：OSM API 0.6 协议客户端（changeset、diff upload、map 下载、409/412、MockTransport）+ 5 个测试
- **neo-editor 冲突模型**：MergeCase / ConflictResolution / 三方标签合并；6 类冲突测试
- **journal 恢复报告**：RecoveryReport（frames/applied/skipped），恢复后状态显示明细
- 截图：`81-validation.png`（校验面板）
- **自动保存**：每 60s 复制 journal 到 `out/autosave/`，保留 3 代（状态栏提示）
- 阶段 4 剩余：MapCSS 渲染集成、真实 HTTP 传输 + 上传/下载 UI、冲突解决 UI、SQLite 快照、changeset 端到端测试


### 2026-11-15 · 阶段 4：设置双栏 + OSM 账户 OAuth
- 设置窗口改为**左侧分类 / 右侧内容**（语言、OSM 账户、快捷键）
- 新增 **OSM 账户**：OAuth2 + PKCE 浏览器登录、本地回调监听、token 保存、
  多账户列表/切换/删除/退出；`accounts.json` 位于 `%APPDATA%/NEO/`
- `neo-api::oauth`：PKCE、授权 URL、token 交换/刷新、用户信息解析（4 个测试）
- `ReqwestTransport`（feature `http`）接入真实 HTTP（30s 超时）
- 测试钩子：`NEO_SHOW_SETTINGS=1`、`NEO_SETTINGS_TAB=account`
- 截图：`83-settings-account.png`


### 2026-11-15 · 阶段 4：OSM 同步 UI
- **文件 → 同步（下载/上传）…**：显示账户、当前视口 bbox、changeset 注释
- 下载：`GET /api/0.6/map` → OSM XML → 替换当前图层
- 上传：创建 changeset → osmChange 上传 → 关闭 changeset；成功显示确认对象数
- 409：冲突对象列表 + 「重新下载（放弃本地修改）」策略
- 后台线程执行网络请求，UI 不阻塞；`NEO_SHOW_SYNC=1` 打开窗口
- 截图：`85-sync.png`


### 2026-11-15 · 阶段 4：MapCSS + SQLite 快照 + 上传端到端测试
- **MapCSS 子集**：解析/编译（选择器、tag 条件、zoom、color/fill/width/opacity）+ 默认
  `assets/styles/neo.mapcss` 接入 `SceneBuilder`；未命中回退内置调色板
- **SQLite 快照库** `neo_persist::SnapshotStore`：journal 字节 + 统计（WAL，保留 10 条）；
  自动保存同时写 JSONL 与 SQLite；journal 丢失时启动从最近快照恢复
- **上传端到端测试**：changeset 创建 → upload → close；409 冲突 → 重试成功（MockTransport）
- 截图：`86-mapcss.png`
- 仍待：冲突 KeepLocal/手合并 UI、MapCSS text/icon、上传前 validator gate


### 2026-11-15 · 阶段 4 收尾：冲突 KeepLocal + 文档
- 同步窗口 409 冲突新增 **保留本地并重试（KeepLocal）**：`Editor::set_object_version`
  对齐服务端版本后重传（覆盖远端）；保留「重新下载（TakeRemote）」
- 文档更新：同步冲突策略、阶段 4 状态、README
- 仍待：手合并 UI（按字段/几何）、MapCSS text/icon/过滤器全集、上传前 validator gate、
  真实网络人工验收（需要测试账号）


### 2026-11-15 · 固定 OAuth + 统一导出 + 框选下载
- OAuth 使用内置客户端；设置窗口不展示任何认证配置细节
- **统一导出窗口**：文件 → 导出…（也替换工具栏导出按钮），格式：OSM XML / osmChange / PBF / JOSM 会话
- **下载窗口**：工具栏云下载图标 → 在地图上左键框选 bbox → 从 OSM API 下载该区域并替换图层
- 测试钩子：`NEO_SHOW_EXPORT=1`、`NEO_SHOW_DOWNLOAD=1`、`NEO_DOWNLOAD_SELECT=1`
- 截图：`87-download.png`、`88-export.png`
