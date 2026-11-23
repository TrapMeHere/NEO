# 01 · 系统架构（System Architecture）

NEO（Next Editor for OSM）是 JOSM 的 Rust 原生重写，核心决策是 **Rust Native + GPU-first**：单进程、模块化单体（Modular Monolith），编辑语义集中在一个可 headless 测试的 Editor Layer，渲染走自建 wgpu 30 管线。本文给出八层概念模型与真实依赖拓扑、线程/GPU 队列总览、端到端时序，以及必须成立的十条不变式。可机器校验的依赖规则见 [Workspace 与依赖规则](./02-workspace-and-dependency-rules.md)，编辑语义见 [编辑核心与撤销](./06-editing-core-and-undo.md)，线程与资源所有权见 [并发模型](./07-concurrency-and-tasks.md)。

## 1 目标与范围

**在范围内**：1M–10M+ OSM 对象的可编辑数据集；完整替代 JOSM 的编辑工作流（下载 / 编辑 / 校验 / 上传 / 冲突解决 / 插件）；Windows / Linux / macOS 原生窗口；GPU 承载地图绘制与拾取；八层职责划分与可机器校验的依赖方向。

**不在范围内**：浏览器托管版本；多进程 / 服务端拆分；多人实时协同编辑（CRDT / OT）；移动端。三者被显式否决，理由见 §5 与 [ADR-0002](./adr/0002-modular-monolith.md)。

| 维度 | 决策 | 依据 |
|---|---|---|
| 进程模型 | 单进程、多线程 | 10M 对象下共享地址空间才能零拷贝切换快照 |
| 编辑入口 | 唯一：`Command` + `Transaction` | 撤销、日志、OSC、冲突检测共同的前置条件 |
| 渲染模型 | 自建 wgpu 管线，egui 只负责 UI 层 | 10M 对象不能靠 immediate-mode 逐图元绘制 |
| 数据所有权 | `neo-store` 单写者 + 不可变快照 | 读路径无锁，后台任务只读快照 |
| 插件 | WASM（wasmtime 50），feature 门控 | 沙箱、可取消、无原生 ABI 漂移 |

**对齐基线（挪自 JOSM、最容易被误传的三点）**：

1. OSM API **只有 0.6**（0.7 只是 wiki 构想页，无部署规范）；changeset 创建是 `PUT /api/0.6/changeset/create`，diff 上传是**单个** `POST .../changeset/{id}/upload`，body 为 `application/xml`，版本冲突返回 **409**。
2. `osmChange` 的实际版本是 `version="0.6"`，**不存在** osmChange/OSC 0.3。
3. PBF 是 JOSM **核心能力**（`OsmPbfFile`），不是插件；且 JOSM 的 Edit 菜单很小（Undo/Redo/剪贴板/合并/搜索/首选项），**几何操作在 Tools 菜单**（Align / Distribute / Orthogonalize / Simplify / Reverse / Split / Combine / Unglue），**Data 菜单零核心项**（纯插件宿主），下载/上传在 File 菜单。以菜单为迁移蓝本时必须按此建表，详见 [JOSM 对齐](./13-josm-parity.md)。

## 2 设计

### 2.1 八层概念模型

概念层用于评审与心智模型，**不是**编译单元；真正的强制边界是 §2.2 的 DAG 深度（由 crate 边界落实）。

```
L8 Application    apps/neo-desktop · apps/neo-prototype   进程/入口/CLI/插件策略
L7 GUI            neo-ui                                 面板·对话框·手势 -> EditorIntent（只读快照）
L6 Editor         neo-editor · neo-tools                  Command/Transaction·Undo·Journal·冲突 ★唯一写者
L5 Rendering      neo-scene · neo-render                  可见集/LOD -> SceneFrame；wgpu 管线与 GPU 拾取
L4 OSM Domain     neo-osm · neo-style · neo-validate · neo-query   实体·Tags·MapCSS·Presets·规则·只读查询
L3 Spatial        neo-geo                                 WebMercator f64 米·rstar 0.14·几何构建·命中测试
L2 Data           neo-store · neo-io · neo-persist         SoA·DataSet·引用集·脏位图；PBF/XML/OSC；SQLite WAL
L1 Infrastructure neo-core · neo-api · neo-plugin         ID/Handle/Sym/定点 Coord/错误；API 0.6；WASM 宿主
```

| 层 | crates | 一句话职责 | 允许依赖 | 硬性禁止 |
|---|---|---|---|---|
| L8 Application | `apps/neo-desktop`, `apps/neo-cli`, `apps/neo-prototype` | 装配、事件循环、CLI | 见依赖规则 §2.2 的显式列表。desktop 不含 query | 业务逻辑写进应用 crate |
| L7 GUI | `neo-ui` | 面板 / 对话框 / 菜单，意图采集 | core, osm, store, style, i18n | `io` / `api` / `persist` / render；持有 `&mut DataSet` |
| L6 Editor | `neo-editor`, `neo-tools` | 把意图变成已验证的事务 | core, osm, store, geo；tools 另加 editor | render / ui / io |
| L5 Rendering | `neo-scene`, `neo-render` | 快照 → 批次 → GPU | scene→{store, geo, style, osm, core}；render→{scene, geo, core} | render 依赖 store/editor/ui |
| L4 OSM Domain | `neo-osm`, `neo-style`, `neo-validate`, `neo-query` | 语义、规则、只读查询 | style 只依赖 core；validate 读 store；query 另读 geo | 任何 GPU/IO 类型出现在签名里 |
| L3 Spatial | `neo-geo` | 几何与索引 | core, osm, store | 持有可变 `DataSet` |
| L2 Data | `neo-store`, `neo-io`, `neo-persist` | 存储与格式 | store/io 见 §2.2；persist 当前无内部 crate | store 反向依赖任何上层 |
| L1 Infrastructure | `neo-core`, `neo-api`, `neo-plugin` | 基础类型 / 网络 / 沙箱 | core 无依赖；api 只依赖 core；plugin 依赖 core+query+editor | 依赖 UI 或渲染 |

两处**已记录的结构性依赖**，评审时不得再扩张：① `neo-validate → store`（只读规则需要引用集视图），概念层在 L4，编译深度是 3；② `neo-plugin → query + editor`（插件只能读快照、只能返回命令），深度 5。`neo-ui` 和 `neo-desktop` 都不依赖 `neo-query`。边写在 [依赖规则](./02-workspace-and-dependency-rules.md) §2.2，越界即 CI 失败。

### 2.2 真实依赖拓扑（DAG 深度）

概念层与编译深度**不同序**。强制以本表为准；任何"我属于 L4 所以可以依赖 L5"的论证无效。`depth(c) = 1 + max(depth(dep))`。

| 深度 | crates | 说明 |
|---|---|---|
| 0 | `neo-core`, `neo-i18n`, `neo-persist` | 无内部依赖。persist 当前不链接 store |
| 1 | `neo-osm`, `neo-api`, `neo-style` | osm 是 DTO；api 只依赖 core；style 只解析样式 |
| 2 | `neo-store` | SoA 与索引 |
| 3 | `neo-geo`, `neo-io`, `neo-validate`, `neo-ui` | ui 只到 store 和 style，不依赖 render/editor |
| 4 | `neo-scene`, `neo-editor`, `neo-query` | query 只被插件使用 |
| 5 | `neo-render`, `neo-tools`, `neo-plugin` | tools 经 editor 产出命令 |
| 6 | `neo-prototype`, `neo-cli`, `neo-desktop` | desktop 的依赖列表是显式的，不含 query |

CI 用 `scripts/check-deps.py` 校验允许边、无环、深度和 `publish = false`（R1、R2、R3、R6，见 [02](./02-workspace-and-dependency-rules.md) §2.5）。给 `neo-scene` 加 `neo-editor` 不会形成环，但 R3 会拒绝。

### 2.3 Editor Layer 是核心：GUI 永不直接改数据

**规则：`&mut DataSet` 在整个仓库里只出现在 `neo-editor` 的事务内部。** 其他层拿到数据内容的唯一方式是读 `Arc<MapSnapshot>`（或经 `MapQuery` 的视图）。这条规则把撤销栈、日志、OSC 生成、冲突检测、插件沙箱、headless 测试六个问题一次性收敛到同一个前置条件上。

```
UI 手势 ──> EditorIntent { Command(cmd) | Undo | Redo | Select | Download | Upload | Validate }
                  │
                  ▼  IntentSink（非阻塞；满则 UI 降级，不阻塞帧）
            Editor::execute(cmd)
                  │ EditTx::new(&mut dataset)          ← 唯一的 &mut DataSet 出现处
                  │ validate → apply（只记 EditOp）→ 捕获逆操作
                  │ commit：写 journal → 增量更新索引/脏位图 → version+1
                  ▼
        Arc<MapSnapshot> ──（arc-swap 发布）──> 渲染 / 校验 / 插件 / 后台任务（全部只读）
```

| 禁止事项 | 为什么 | 编译期 / CI 如何兜住 |
|---|---|---|
| UI 持有 `&mut DataSet` | 借用跨帧 → 死锁与帧内不一致 | `neo-ui` 无 store 写 API；`UiContext` 只持 `Guard<Arc<MapSnapshot>>` |
| 工具直接改已提交数据 | 撤销栈与数据分叉 | `neo-tools` 只返回 `Box<dyn EditCommand>` |
| 渲染层读数据集 | 帧时间被编辑路径拖住 | `neo-render` 不依赖 store/editor（`cargo tree` 断言） |
| 后台任务"顺手修数据" | 版本回退、历史不可解释 | 任务结果带 `base_version`，不符即 `Stale` 丢弃 |

### 2.4 进程 / 线程 / GPU 队列总览

```
+---------------------- process: neo-desktop (single) ----------------------+
|                                                                          |
|  Main/UI thread                      Rayon 计算池                         |
|  winit events ──> App::ui            +-- Way -> polyline / 三角化          |
|    +-- panels(snapshot.load()) 只读  +-- 样式求值 / 标签布局               |
|    +-- Editor::execute (0..n)        +-- neo-validate 增量校验             |
|    +-- SceneFrame 差量 / 合批        +-- neo-io PBF blob 解码              |
|    +-- MapRenderer::prepare/encode   +-- rstar 索引重建（churn 高时）      |
|         |                                                                |
|         v  wgpu::Queue（egui 的 paint 阶段提交，不另起渲染线程）           |
|     GPU: fill / stroke / point / glyph + pick pass + egui composite       |
|                                                                          |
|  Tokio runtime (multi-thread)         Storage worker（独占 SQLite 连接）    |
|  下载/上传/OAuth/限流/重试/notes      journal 追加/autosave/崩溃恢复扫描    |
+--------------------------------------------------------------------------+
```

| 执行体 | 拥有什么 | 允许做什么 | 禁止 |
|---|---|---|---|
| Main/UI | `Editor`、`MapRenderer`、`SnapshotCell` 的单帧 `Guard` | 输入、命令执行、场景差量、编码、提交 | 阻塞等待（`poll(wait)`、同步 HTTP、同步磁盘写） |
| Rayon 池 | 无所有权；只持 `Arc<MapSnapshot>` | 几何/样式/校验的纯计算，可跨帧续算 | 触碰 `&mut DataSet`；向 UI 投递无界消息 |
| wgpu Queue | `Device`/`Queue` 由 eframe 创建（[ADR-0005](./adr/0005-wgpu-egui-shared-device.md)） | 写 buffer、提交 encoder、present | 第二个 `Instance`；`paint` 内新建 pass 或 `submit` |
| Tokio | HTTP 客户端、取消令牌 | 下载、上传、重试、进度上报 | 直接写数据集；阻塞主线程 |
| Storage worker | SQLite 连接（WAL） | journal / autosave / 恢复，串行写入 | 与编辑线程共享连接 |

**不另起渲染线程**：egui 的 `PaintCallback` 在 UI 线程的 paint 阶段执行，地图绘制因此天然与 UI 同帧（拾取、hover、overlay 无 1 帧延迟）。可拆点收敛在 `MapRenderer::prepare`（encoder 内工作、返回 `CommandBuffer`）与 `encode`（纯编码）两处；未来若真要并行，只需把这两个函数移到独立线程并保持"每帧一次提交"的契约。**相机重基**：GPU 顶点是"相对批次原点的 f32 米坐标"，相机离原点超过 `REBASE_THRESHOLD_M`（4 km）即重基并只更新 32 B 的 camera uniform。

### 2.5 单写者与"数据集安装"

除用户编辑外，只有两条结构性写入路径，两者都**独占**一份新 `DataSet`，完成后一次性安装：

| 路径 | 写入者 | 发布方式 | 进 History |
|---|---|---|---|
| 用户编辑 | UI 线程 `EditTx` | 增量 + `version+1` | 是 |
| PBF/XML 导入 | worker（独占新 DataSet） | `SnapshotCell::store` 整体替换 | 否（新会话） |
| autosave 恢复 | Storage worker | 同上 | 否（历史从 journal 重建） |
| 区域下载合并 | `neo-editor` 的 `MergeCommand` | 走命令管线 + `version+1` | 是 |

版本语义：`version` 在**每次提交**后严格递增，undo/redo 也递增（不回落），因此"结果与版本"永远单调对应；后台任务用 `base_version` 校验，不符就丢结果而不是打补丁。

### 2.6 关键不变式

| 编号 | 不变式 | 实现手段 | 不做会怎样 |
|---|---|---|---|
| INV-1 | 单一写者：任一时刻只有一个线程持 `&mut DataSet` | `Editor` 非 `Sync`，写 API 收在 editor 内 | 数据竞争（编译器挡不住的那部分只能靠类型设计） |
| INV-2 | 快照不可变：发布后的 `Arc<MapSnapshot>` 永不修改 | `MapSnapshot` 无 `&mut self` 方法 | 渲染撕裂、索引与数据不一致 |
| INV-3 | 渲染只读：`neo-render` 不依赖 store/editor | crate 白名单 + CI forbidden-edge | 帧时间被编辑路径拖死 |
| INV-4 | UI 不持可变引用，也不跨帧存 `Guard` | `arc_swap::load()` 作用域限定在单次绘制 | 借用活过 `ui()` → UI 锁死 |
| INV-5 | 一切修改走 Command，无绕过 `EditTx` 的字段写入 | store 写 API `pub(crate)` + sealed trait（见 02 §3） | 撤销栈与数据分叉 |
| INV-6 | 写前日志：事务落盘（或入内存 journal）后才发布快照 | `Editor::commit` 中 journal 在 publish 之前 | 崩溃后数据与历史不一致 |
| INV-7 | 撤销产生**新**版本，不回滚版本号 | `version = version + 1` 无条件递增 | 基于旧版本的结论被误用 |
| INV-8 | 句柄带 generation，过期句柄返回 `Err` | `Handle { index, generation }`，释放时 `generation+1` | 悬挂引用 / 幽灵对象 |
| INV-9 | UI 线程不做阻塞等待 | 代码评审 + `pollster::block_on` 仅限启动路径 | 卡帧、丢输入 |
| INV-10 | 锁顺序：store → geo cache → scene，不得反向；持锁不投递 UI 消息 | `parking_lot` 封装 + 评审清单 | 死锁（清单见 07 §2.5） |

## 3 关键接口

以下为跨层契约签名。`neo-core`/`neo-osm`/`neo-store`/`neo-geo`/`neo-query` 已在骨架代码中落地；`neo-scene`/`neo-render`/`neo-editor`/`neo-ui` 的 `lib.rs` 目前是占位注释，签名按本表对齐（差异在对应 PR 同步）。

```rust
// crates/neo-core/src/ids.rs —— 类型化 ID，禁止裸 i64 跨 crate 传递
pub struct ObjectId(pub i64);                  // <0 本地新建 / 0 空 / >0 服务端已有
pub enum ObjectKind { Node, Way, Relation, Changeset }

// crates/neo-core/src/handle.rs —— 带 generation 的槽位句柄（与 T 是否 Send 无关）
pub struct Handle<T> { /* index: u32, generation: u32 */ }
impl<T> Handle<T> {
    pub const fn new(index: u32, generation: u32) -> Self;
    pub const fn index(self) -> u32;
    pub const fn generation(self) -> u32;
}

// crates/neo-core/src/units.rs —— 定点坐标，1e-7 度（OSM API 精度）
pub struct Coord(pub i32);
pub struct LatLon { pub lat: Coord, pub lon: Coord }
// crates/neo-store/src/dataset.rs —— 单写者拥有的唯一真值
pub struct DataSet { /* SoA + IdIndex + TagStore + ChangeTrack + 脏位图 */ }
impl DataSet {
    pub fn new() -> Self;
    pub fn version(&self) -> u64;
    pub fn push_node(&mut self, node: Node) -> Handle<NodeSlot>;
    pub fn push_way(&mut self, way: Way, policy: MissingPolicy) -> Result<Option<Handle<WaySlot>>>;
    pub fn push_relation(&mut self, r: Relation, policy: MissingPolicy) -> Result<Option<Handle<RelationSlot>>>;
    pub fn node_view(&self, h: Handle<NodeSlot>) -> Option<NodeView<'_>>;
    pub fn set_node_position(&mut self, h: Handle<NodeSlot>, pos: LatLon) -> Result<LatLon>;
    pub fn take_dirty_ways(&mut self) -> Vec<Handle<WaySlot>>;
}

// crates/neo-query/src/lib.rs —— 插件/验证器/后台任务/UI 的只读出口（无写接口）
pub trait MapQuery {
    fn data(&self) -> &DataSet;
    fn node(&self, id: ObjectId) -> Option<Node>;
    fn way(&self, id: ObjectId) -> Option<Way>;
    fn relation(&self, id: ObjectId) -> Option<Relation>;
    fn parent_ways(&self, node: ObjectId) -> Vec<ObjectId>;
    fn parent_relations(&self, kind: ObjectKind, id: ObjectId) -> Vec<ObjectId>;
    fn nodes_in(&self, bounds: WorldBounds) -> Vec<ObjectId>;
    fn ways_in(&self, bounds: WorldBounds) -> Vec<ObjectId>;
    fn nearest_node(&self, point: WorldPoint, max_meters: f64) -> Option<(ObjectId, f64)>;
}
pub struct SnapshotCell { /* arc_swap::ArcSwap<MapSnapshot> */ }
impl SnapshotCell {
    pub fn load(&self) -> arc_swap::Guard<Arc<MapSnapshot>>;   // 单帧有效，不得跨帧保存
    pub fn store(&self, snapshot: MapSnapshot);                // 唯一发布点
}

// crates/neo-editor/src/lib.rs —— 唯一写入口（目标签名）
pub struct Editor { /* DataSet + History + Journal + Selection */ }
impl Editor {
    pub fn execute(&mut self, cmd: Box<dyn EditCommand>) -> Result<CommitReport, EditError>;
    pub fn undo(&mut self) -> Result<CommitReport, EditError>;
    pub fn redo(&mut self) -> Result<CommitReport, EditError>;
    pub fn snapshot(&self) -> Arc<MapSnapshot>;
    pub fn publish_if_dirty(&mut self) -> Option<Arc<MapSnapshot>>;
}

// crates/neo-ui/src/bridge.rs —— UI 只表达意图
pub enum EditorIntent {
    Command(Box<dyn EditCommand>), Undo, Redo, Select(SelectionOp),
    Download(DownloadRequest), Upload(UploadRequest), Validate(ValidateRequest),
}
pub trait IntentSink: Send + Sync {
    fn submit(&self, intent: EditorIntent) -> IntentId;
    fn try_submit(&self, intent: EditorIntent) -> Result<IntentId, Full>;
}

// crates/neo-geo/src/camera.rs —— 顶点为"相对批次原点的 f32 米坐标"
pub const REBASE_THRESHOLD_M: f64;   // 4 km；超过即重基并只更新 32 B uniform

// crates/neo-scene/src/frame.rs —— 渲染层的唯一输入形态（无 GPU 句柄、无编辑器依赖）
pub struct ViewParams { pub center: WorldPoint, pub zoom: f64, pub viewport_px: [u32; 2], pub dpr: f32 }
pub struct SceneFrame { pub version: u64, pub batches: Vec<Batch>, pub labels: Vec<LabelItem> }
pub fn build_scene(
    data: &DataSet,
    index: Option<&SpatialIndex>,
    style: &StyleSheet,
    view: &ViewParams,
) -> SceneFrame;

// crates/neo-render/src/pass.rs —— 不出现任何 store/editor 类型
pub struct MapRenderer { /* Device/Queue 由外部传入（ADR-0005） */ }
impl MapRenderer {
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &SceneFrame) -> PrepareStats;
    pub fn encode(&mut self, pass: &mut wgpu::RenderPass<'_>);
    pub fn resolve_pick(&self, cursor: [f32; 2]) -> Option<ObjectId>;
}
```

## 4 数据流与时序

**稳态帧循环**：`input -> egui::Context::run -> App::ui`，依次执行 `panels(snapshot.load())`（只读，`Guard` 不出作用域）、`intents.drain() -> Editor::execute`、`build_scene`（脏时）、`MapRenderer::prepare/encode`，最后由 egui-wgpu 的 paint callback 提交。每帧有硬预算（设计目标，非实测值）：UI 逻辑 + 场景构建（可跨帧续算）+ 上传 + 提交；超预算按 [07](./07-concurrency-and-tasks.md) §2.6 丢 LOD 或延后一帧上传，不允许"补帧"阻塞输入。

### 4.1 端到端：打开 PBF → 编辑 → 上传

```
User      neo-ui       neo-io        neo-store/geo     worker        neo-render      neo-api
 |          |             |                |               |              |              |
 | Ctrl+O ->| OpenFile    |                |               |              |              |
 |          |------------>| (spawn)        |               |              |              |
 |          |             |--osmpbf------->|               |              |              |
 |          |             |  blob 解码     | SoA + 索引     |              |              |
 |          |             |<---------------| publish v=1    |              |              |
 |          |<------------| ImportReport   |               |              |              |
 |<-- 首帧  |--build_scene(v=1)--------------------------->| prepare/encode|              |
 | drag --->| MoveCommand (意图)            |               |              |              |
 |          |--> Editor::execute: validate->apply->inverse->journal->publish v=2 |          |
 |<-- 重绘  |--build_scene(v=2)（仅脏 way）--------------->| 脏区间上传    |              |
 | Ctrl+U ->| UploadRequest                  |               |              |              |
 |          |------------>| OSC 0.6 生成（<osmChange version="0.6">）------->| PUT changeset|
 |          |             |                |               |              | POST upload  |
 |          |<------------| ChangeSet      |               |              | 200 / 409    |
 |          |  409 --> ConflictQueue --> ConflictDialog --> ResolveCommand --> 重传         |
```

| 阶段 | 执行体 | 产物 | 失败语义 |
|---|---|---|---|
| 打开 | worker + `neo-io` | `MapSnapshot(v=1)` 安装 | 文件损坏 → 提示并保留旧数据集，不覆盖 |
| 编辑 | UI 线程 `neo-editor` | `Snapshot(v+1)` + journal 事务 | 校验失败 → 事务不提交，UI 显示原因 |
| 场景 | UI 线程（可跨帧）+ Rayon 几何 | `SceneFrame(v)` | 超预算 → 降 LOD（不阻塞 UI） |
| 上传 | Tokio + `neo-api` | changeset id 或 409 冲突集 | 冲突 → 入队等用户解决；网络失败 → 指数退避重试 |

> 上传失败语义必须精确：请求未发出可安全重试；已发出但超时属于"状态未知"，要先 `GET` 核对 version；409（version 被他人更新）整请求回滚并产出冲突报告；412（引用缺失）补下载或对删除用 `if-unused`。完整状态机见 [OSM API 与同步](./09-osm-api-and-sync.md) §2.5。

## 5 权衡与替代方案

| 被否决方案 | 否决理由 | 我们付出的代价 |
|---|---|---|
| 微服务 / 多进程（core daemon + UI 客户端） | 10M 对象跨进程需序列化或共享内存协议；每帧拾取与可见集重建都过 IPC，延迟不可控；GPU 上下文跨进程共享要平台特化代码 | 无独立崩溃域；插件隔离只能靠 WASM 沙箱而非进程边界 |
| Tauri + React（WebView 前端） | WebView 无法承载自建 wgpu 管线（需独立窗口 handle 嵌入，三平台方案不统一）；10M 对象的图元列表经 IPC/JS 桥不可行 | 放弃 Web 生态的控件丰富度与既有前端人才 |
| Qt / GTK（C++ 绑定，如 `qmetaobject`/`gtk-rs`） | Qt 双许可（LGPL/商业）与静态链接合规成本；C++ 对象生命周期跨越 Rust 借用检查，FFI 层成为长期 bug 源；GTK 在 Windows 上的观感与输入法差距明显 | 放弃成熟的多窗口 docking、无障碍与 IME 长期打磨（无障碍依赖 accesskit，覆盖度弱于 Qt） |
| 直接复用 JOSM 代码（Java 互操作 / 转译） | JOSM 为 GPL-2.0+，静态链接或改写会传染许可；JVM 启动与堆模型不适合 GPU-first | 语义兼容只能靠行为对齐（见 [16](./16-roadmap-and-risks.md) 风险 R2） |

## 6 风险

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| A1 | 概念八层与 DAG 深度不同序，被误读为"允许向上依赖" | 循环依赖、编译期无法拆分 | §2.2 表格优先；CI forbidden-edge 断言 |
| A2 | `neo-ui` 通过 `neo-tools` 间接拿到 store 写途径 | INV-5 被绕过 | 写 API `pub(crate)`；tools 只返回 `Box<dyn EditCommand>` |
| A3 | 单进程无崩溃域，插件/驱动崩溃丢全部未保存编辑 | 数据丢失 | journal + autosave（10/07）+ WASM 沙箱资源限制 |

## 7 验收标准

1. `cargo metadata` 导出的依赖图与 §2.2 表逐边一致（`scripts/check-deps.py` 退出码 0），且至少 3 条 forbidden-edge 负例测试通过。
2. `cargo tree -p neo-render` 不含 `neo-store`/`neo-editor`/`neo-ui`；`cargo tree -p neo-ui` 不含 `neo-io`/`neo-api`/`neo-persist`。
3. 拖动地图节点的单次交互产生**恰好一条**历史记录，`Snapshot` 版本恰好 +1，journal 有一条完整事务，几何脏集合只含被移动节点及其所属 way。
4. 导入 1M 对象期间 UI 线程最长阻塞【待实测，见 docs/14-performance-and-verification.md】，且输入不丢（记录 60 s 内 input latency p99）。
5. 上传遇到 1 处冲突时，UI 收到 `ConflictQueue` 条目，逐条解决后可重试成功（集成测试 `tests/upload_conflict.rs`）。
6. 强制杀进程（SIGKILL）后重启，journal replay 恢复到最后一个已提交事务，未提交事务不出现。

## 8 参考

- JOSM 项目主页与源码（GPL-2.0+，仅作行为对齐参考，不复制代码）：https://josm.openstreetmap.de/
- JOSM 菜单基线（File/Edit/Tools/Data 的实际项）：https://josm.openstreetmap.de/wiki/Help/Menu
- OSM API v0.6（changeset、上传、409/412 语义）：https://wiki.openstreetmap.org/wiki/API_v0.6
- OsmChange 格式（`version="0.6"`）：https://wiki.openstreetmap.org/wiki/OsmChange
- OSM PBF 格式：https://wiki.openstreetmap.org/wiki/PBF_Format
- egui / eframe / egui-wgpu 0.36.2：https://docs.rs/egui/0.36.2/egui/
- wgpu 30.0.2：https://docs.rs/wgpu/30.0.2/wgpu/
- arc-swap（`load`/`load_full`/`Guard` 语义）：https://docs.rs/arc-swap/1.9.5/arc_swap/
- rstar 0.14（`RTreeObject`/`PointDistance`）：https://docs.rs/rstar/0.14/rstar/
- JOSM 许可（GPL-2.0+）：https://josm.openstreetmap.de/wiki/Source
- 版本核对记录（含源码与依赖 JSON 证据）：`.research/crate-api-cheatsheet.md`

> 待验证：`accesskit` 在三平台的无障碍覆盖度；`eframe 0.36.2` 的 `App` 入口是 `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame)`，`update()` 在 0.36 已移除（已从 crate 源码核实；见 [ADR-0005](./adr/0005-wgpu-egui-shared-device.md)）。两处都需在首次 `cargo build` 后回填本节。
