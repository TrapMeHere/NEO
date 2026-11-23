# 06 · 编辑核心与撤销（Editing Core & Undo）

NEO 的一切数据修改都经过 `Command` + `Transaction`：命令只描述意图，`EditTx` 在影子层收集操作，提交时按固定管线验证、应用、生成逆操作、写日志、发布快照。本文定义 `EditCommand`/`EditTx` 的语义、提交管线的顺序与失败语义、Undo/Redo 与上传状态的交互、ChangeSet/OSC 生成，以及 Journal 格式与崩溃恢复；线程归属见 [并发模型](./07-concurrency-and-tasks.md)，编辑器在整体中的位置见 [系统架构](./01-system-architecture.md) §2.3。

## 1 目标与范围

**在范围内**：编辑原语（增删改、节点移动、way 增删点、relation 成员）、事务原子性、Undo/Redo 栈、内存 journal 与 replay、冲突数据模型。

**不在范围**：具体工具的交互手感（`neo-tools`）、OSM API 的 HTTP 语义（[09](./09-osm-api-and-sync.md)）、校验规则内容（[12](./12-validation.md)）、SQLite schema（[10](./10-persistence-and-recovery.md)）。

## 2 架构设计

### 2.1 为什么不是直接改 store

| 若允许直接改 store，会失去 | 后果 | Command 模型如何补上 |
|---|---|---|
| 逆操作 | 无法撤销（事后重建前像需要全量 diff） | 应用阶段逐操作捕获逆操作 |
| 原子性 | 半完成状态（移动了节点但 way 未更新） | 影子层缓冲 + 全成或全败 |
| 单点验证 | 校验散落在 UI/工具里，无法在 headless 下测 | `commit()` 是唯一校验入口 |
| 日志 | 崩溃恢复只能靠全量 autosave | 提交前写 op 日志 |
| OSC 生成 | 上传 diff 需要另行对比快照 | `ChangeSet` 由提交产物累积 |
| 插件安全边界 | WASM 插件能篡改任意状态 | 插件只能返回 `EditCommand` |

直改 store 的诱惑在"性能"：多一层缓冲看似浪费。实际缓冲成本是一次 `Vec<EditOp>` push（几十字节），相对几何索引更新的量级可忽略；**具体开销【待实测，见 docs/14-performance-and-verification.md】**。

### 2.2 命令、事务、数据集三者的关系


**关键点：`apply` 不改 `DataSet`。** `EditTx` 维护 `inserts / updates / deletes` 三个叠加层，`tx.node(id)` 先查 overlay 再查 base，因此同一事务内"先建点、再建 way、再改点标签"能自洽。真正的写入发生在 `commit()`，此时才需要 `&mut DataSet`，也才可能失败（唯一能失败的写路径是句柄 generation 与索引冲突，前置验证已排除绝大多数）。

### 2.3 提交管线（12 步）

| # | 步骤 | 失败语义 | 为何在此位置 |
|---|---|---|---|
| 1 | `validate_structure`：引用完整、way 至少 2 点、relation 成员类型合法、自相交可选 | 返回 `EditError::InvalidGeometry`，**DataSet 未变** | 最早失败最便宜 |
| 2 | `journal.append_begin(txn, label)` | `JournalError` → 整个事务放弃 | 写前日志必须先于任何状态改变 |
| 3 | `apply_ops`：把 write-set 落到 `DataSet` 的 SoA 列 | 不应失败（前置已验证）；debug 下 `assert!` | — |
| 4 | `capture_inverse`：为每个 op 记录前像/后像 | 不可能失败（内存不足则 abort） | 必须在应用时捕获，事后不可重建 |
| 5 | `journal.append_commit(txn)` + `sync`（按策略） | 失败 → 用第 4 步的逆操作回滚到 txn 前 | 崩溃一致性点 |
| 6 | `dirty.mark(touched)` | — | 供增量渲染与 autosave |
| 7 | `geo.invalidate(touched, version+1)` | — | 几何缓存键含 version 区间 |
| 8 | `refs.update(touched)`（node→ways、way→relations） | — | 索引与数据同步更新 |
| 9 | `spatial.update(touched)`（rstar 增量插入/删除） | — | 拾取与可见集依赖它 |
| 10 | `version += 1`（无条件，包括 undo） | — | INV-7 |
| 11 | `publish(snapshot)`：`Arc::new` + `ArcSwap::store` | — | 订阅者从此看到新版本 |
| 12 | `notify`：selection 修剪、UI 失效、autosave 触发、插件事件、`puffin` 打点 | 订阅者异常不得回滚已提交事务 | 观察者只读 |

第 2/5 步的顺序说明"写前日志"的准确含义：journal 记录的是**操作**（`EditOp`），不是后像。因为 `EditOp` 在给定前像下是确定性的，replay 可重建状态，且记录体积远小于镜像。代价是 replay 依赖代码版本（见 §2.6 的 schema 版本门）。

### 2.4 Undo / Redo

```
 History.entries: [ e0 | e1 | e2 | e3 | e4 ]      cursor = 5 (尾部)
                                     ↑ 新事务 → 截断 cursor 之后（清空 redo），push e5
 undo: cursor-- → 取 entries[cursor].inverse → 作为普通事务提交（version+1）
 redo: cursor++ → 取 entries[cursor].forward → 作为普通事务提交（version+1）
```

| 设计点 | 规则 | 理由 |
|---|---|---|
| 撤销也是事务 | 走 §2.3 全部 12 步，version+1 | 订阅者不需要区分"正向/反向"，INV-7 |
| 逆操作来源 | `apply` 时逐 op 捕获（前像 + 被删对象的完整对象） | 事后无法重建被覆盖的标签 |
| 合并（coalescing） | 仅当 `merge_hint` 相等且间隔 < 500 ms：同类拖拽、同一标签键连续编辑 | 避免一次拖动产生 60 条记录 |
| 幂等 | `undo` 两次 = 原状态 + 两次 version++；`redo` 只在 cursor 处有效 | 保证脚本化重放可预测 |
| 容量 | 默认上限 256 MiB（可配），按对象大小估算；超限驱逐最旧条目并置 `history_truncated` | 10M 对象下前像不能无限存 |
| 与上传状态 | 条目状态 `Unuploaded / Uploaded{changeset} / DirtyAgain`；对已上传条目 undo 后置 `DirtyAgain` | 远端不会回滚，本地需重新上传（见下表） |

| 条目状态 | 触发 | 再次上传时生成 |
|---|---|---|
| `Unuploaded` | 新提交 | 创建/修改/删除元素 |
| `Uploaded{cs}` | 上传成功并标记 | —（已固化） |
| `DirtyAgain` | 对已上传条目 undo/redo，或改动了已上传对象 | 新 changeset 中的 modify/delete |
| `Orphaned` | 对象被远端删除（409 后确认） | 不出现在 OSC，改为 `RecreateObject` 候选 |

### 2.5 ChangeSet 与 OSC 生成

`ChangeSet` 是**提交产物的累积器**，与 History 解耦（History 可被截断，ChangeSet 不能丢）。

| 本地状态 | OSC 元素 | 备注 |
|---|---|---|
| 新对象（负 id） | `<create>` | 上传后由 `id_map: ObjectId → ObjectId` 映射为正 id |
| 已有对象被修改 | `<modify>` | 携带 `version`，服务端据此判冲突 |
| 已有对象被删除 | `<delete>` | 携带 `version`；服务端 `if-unused` 校验 |
| 新建后又删除 | 不出现 | 提交时即从 ChangeSet 中消解，避免噪声 |

生成时机：`ChangeSetAccumulator::diff() -> EditDiff`（`created/modified/deleted`），再交给 `neo_io::osc::OscWriter::write_osc(&diff, opts, w)`；逐对象比较 base 像与当前像，**不读 History**。`EditDiff` 的形态与写入器见 [08](./08-io-pbf-xml-osc.md) §3。冲突上传返回 409 时，服务端返回的 diff 与本地 base 像构成三方合并的输入（§2.6）。

### 2.6 冲突：数据模型与 UI 交接

```rust
/// 对象级"合并情形"。刻意不叫 ConflictKind——那个名字归 neo-api 的
/// ConflictReport 使用（见 09 §3）；editor 不依赖 api，两者分离。
pub enum MergeCase {
    VersionMismatch { local: i32, remote: i32 },
    LocalDeletedRemoteModified,
    RemoteDeletedLocalModified,
    TagDiverged { key: String },                       // 同键不同值
    NodeMoved { local_delta_m: f64, remote_delta_m: f64 },
    WayNodeListDiverged { local: Vec<NodeId>, remote: Vec<NodeId> },
    RelationMemberDiverged,
    ChangesetClosed,
}

pub struct Conflict {
    pub object: ObjectRef,
    pub case: MergeCase,
    pub base: Option<ObjectImage>,     // 上传前的本地像
    pub local: Option<ObjectImage>,    // 当前本地像（可能已再编辑）
    pub remote: Option<ObjectImage>,   // 服务端当前像（409 响应或重下载）
}

/// 与 neo-api 的 ConflictResolution 同名同序（KeepLocal | TakeRemote | MergeManually），
/// 便于 app 层一对一转换；额外两项是 editor 独有的本地动作。
pub enum ConflictResolution {
    KeepLocal, TakeRemote,
    MergeManually { tags: BTreeMap<String, String>, geometry: GeometryChoice },
    DeleteObject, RecreateObject,
}
```

| 情形 | 合并规则 | 落入哪类 |
|---|---|---|
| 双方改不同标签键 | 自动合并（并集） | 无（不需人工） |
| 同键不同值 | 必须人工选择（本地/远端/手填） | `TagDiverged` |
| 远端删除、本地修改 | 人工选择（保留/删除/重建为新对象） | `RemoteDeletedLocalModified` |
| way 节点列表双方都变 | 人工选择 + 可视 diff | `WayNodeListDiverged` |

UI 交接形态：`ConflictQueue` 由 `Editor` 拥有，UI 每帧只 `peek`，用户选择后调用 `resolve()`，它返回一个**普通 `EditCommand`**（`ResolveConflictCommand`），走 §2.3 管线。因此"解决冲突"与"编辑"完全同构：可撤销、可入 journal、可测试（不需要 UI）。

**409 的翻译发生在应用层**：`neo-api::ApiError::Conflict(ConflictReport)` 只含 id/version/stage 与服务端文本，不含本地对象像；把 `ConflictReport` + ChangeSet base 像翻译成 `Conflict` 的适配器放在 `apps/neo-desktop/src/sync_bridge.rs`（同时依赖 api 与 editor 的唯一位置）。这样 editor 不依赖 api、api 不依赖 editor，冲突模型只由 ID/版本/对象像构成。

### 2.7 Journal：格式、replay、与 autosave 的分工


| 记录 | 何时写 | 载荷 |
|---|---|---|
| `TxnBegin` | 步骤 2 | `txn_id, label, base_version, timestamp, author` |
| `Op` | 步骤 2 之后（随 apply 逐条写） | `EditOp` |
| `TxnCommit` | 步骤 5 | `txn_id, op_count` |
| `Checkpoint` | autosave 成功后 | `snapshot_version, autosave_path` |

| 决策 | 选择 | 理由 |
|---|---|---|
| 编码 | **rkyv 0.8.18**（`Archive + Serialize`） | 零拷贝读、体积小；bincode 无版本校验且演进脆弱 |
| 对齐 | payload 先写 `AlignedVec` 再落盘；读侧逐条 `AlignedVec::from_slice` 后 `access` | rkyv 相对指针要求整体基址对齐 |
| 版本门 | header `schema: u16`，不匹配即拒绝加载 | rkyv 的 schema 演进不自动 |
| 完整性 | 每帧 `crc32c` + magic 校验 | 崩溃时最后半帧必然撕裂，靠校验定位截断点 |
| fsync 策略 | `always` / `interval(1s)` / `never`，默认 `interval` + 上传前强制 `always` | `always` 每事务一次 fsync 在大数据集上过慢【待实测，见 docs/14-performance-and-verification.md】 |
| 与 SQLite 的分工 | journal = 操作日志；SQLite = 对象镜像缓存 + autosave | 一个面向操作、一个面向镜像，不重复 |

**所有权切分（避免 editor↔persist 反向依赖）**：帧格式、`JournalRecord`、`EditOp` 的 rkyv 归档与 replay 算法都归 `neo-editor`；文件创建/轮转/索引/GC/fsync 归 `neo-persist::JournalStore`，它只接收 `(FrameKind, &[u8])` 并回传 `seq`。装配点在 `apps/neo-desktop`：把 `Arc<Mutex<JournalStore>>` 适配成 `JournalSink` 注入 `Editor`。这样 `neo-persist` 依赖 `store` 而不依赖 `editor`（[02](./02-workspace-and-dependency-rules.md) §2.2）；细节见 [10](./10-persistence-and-recovery.md) §2.3。

**Replay 算法**：


## 3 关键接口

```rust
// crates/neo-editor/src/command.rs
pub trait EditCommand: Send {
    /// 只记录操作到 tx 的影子层；不得直接触碰 DataSet。
    fn apply(&mut self, tx: &mut EditTx) -> Result<CmdOutcome, EditError>;
    /// 逆操作。默认实现由 AutoUndo 包装器注入（重放捕获到的逆操作）；
    /// 仅当命令有非结构副作用（selection/changeset/插件状态）时才需要手写。
    fn undo(&mut self, tx: &mut EditTx) -> Result<(), EditError>;
    fn label(&self) -> CmdLabel;
    fn merge_hint(&self) -> MergeHint { MergeHint::None }
    fn touched(&self) -> &[ObjectRef] { &[] }
}

/// 为什么 `apply` 取 `&mut self`：
/// 1) 命令会被 redo 重放，需要可重置的内部游标；
/// 2) 合并（coalescing）要修改既有命令的终态；
/// 3) 逆操作缓存（前像、被删对象）挂在命令上，避免二次遍历。
/// 为什么 `undo` 也返回 `Result`：undo 可能因句柄过期或依赖缺失失败，
/// 必须能向用户报错并保留 history 一致性（失败则 cursor 不前移）。

pub struct CmdOutcome {
    pub label: CmdLabel,
    pub touched: Vec<ObjectRef>,
    pub created: Vec<ObjectRef>,
    pub deleted: Vec<ObjectRef>,
    pub notes: Vec<std::borrow::Cow<'static, str>>,
}

// crates/neo-editor/src/tx.rs
pub struct EditTx<'a> { /* base: &'a mut DataSet, ops: Vec<EditOp>, policy: &'a EditPolicy */ }

impl EditTx<'_> {
    pub fn base_version(&self) -> u64;
    // 读穿透：先查 overlay 再查 base，因此同一事务内的写入立刻可见
    pub fn node(&self, id: NodeId) -> Option<NodeView<'_>>;
    pub fn way(&self, id: WayId) -> Option<WayView<'_>>;
    pub fn way_node_ids(&self, id: WayId) -> Option<Vec<NodeId>>;

    pub fn insert_node(&mut self, node: Node) -> NodeId;                 // 自动分配负 id
    pub fn insert_node_with_id(&mut self, id: NodeId, node: Node) -> Result<(), EditError>;
    pub fn set_position(&mut self, id: NodeId, pos: LatLon) -> Result<(), EditError>;
    pub fn set_tags(&mut self, id: ObjectId, diff: TagDiff) -> Result<(), EditError>;
    pub fn delete(&mut self, id: ObjectId) -> Result<(), EditError>;
    pub fn insert_way_node(&mut self, way: WayId, idx: usize, node: NodeId) -> Result<(), EditError>;
    pub fn remove_way_node(&mut self, way: WayId, idx: usize) -> Result<(), EditError>;
    pub fn set_relation_members(&mut self, rel: RelationId, m: Vec<Member>) -> Result<(), EditError>;
    pub fn note(&mut self, msg: impl Into<std::borrow::Cow<'static, str>>);
    pub fn ops(&self) -> &[EditOp];
}

// crates/neo-editor/src/context.rs
pub struct EditorContext {
    pub version: u64,
    pub selection: Selection,
    pub changeset: ChangeSetAccumulator,
    pub geo: GeoCacheHandle,
    pub journal: Box<dyn JournalSink + Send>,
    pub policy: EditPolicy,
}

// crates/neo-editor/src/selection.rs
pub struct Selection {
    nodes: rustc_hash::FxHashSet<NodeId>,
    ways: rustc_hash::FxHashSet<WayId>,
    relations: rustc_hash::FxHashSet<RelationId>,
    primary: Option<ObjectRef>,
}
impl Selection {
    pub fn toggle(&mut self, id: ObjectRef);
    pub fn set_primary(&mut self, id: Option<ObjectRef>);
    pub fn snapshot(&self) -> SelectionSnapshot;          // 供后台任务只读使用
    pub fn prune(&mut self, snap: &Snapshot) -> usize;    // 返回被移除的数量
    pub fn contains(&self, id: ObjectRef) -> bool;
}

// crates/neo-editor/src/history.rs
pub struct History { entries: Vec<HistoryEntry>, cursor: usize, bytes: usize, cap_bytes: usize }
impl History {
    pub fn push(&mut self, e: HistoryEntry) -> Option<HistoryEntry>;   // 返回被驱逐项
    pub fn can_undo(&self) -> bool;
    pub fn can_redo(&self) -> bool;
    pub fn undo(&mut self, tx: &mut EditTx) -> Result<CmdLabel, EditError>;
    pub fn redo(&mut self, tx: &mut EditTx) -> Result<CmdLabel, EditError>;
    pub fn mark_uploaded(&mut self, up_to: TxnId, changeset: i64);
    pub fn set_cap(&mut self, bytes: usize);
}

// crates/neo-editor/src/journal.rs —— editor 拥有"记录什么"，persist 拥有"写到哪"
pub fn encode_record(rec: &JournalRecord, out: &mut rkyv::util::AlignedVec) -> Result<FrameHeader, JournalError>;
pub fn decode_record(bytes: &[u8]) -> Result<JournalRecord, JournalError>;

/// 字节级落盘接口：由 neo-persist::JournalStore 实现（feature = "persist" 时），
/// 无 persist 时用 editor 自带的 MemoryJournal / FileJournal。
pub trait JournalSink: Send {
    fn append_frame(&mut self, kind: FrameKind, payload: &[u8]) -> Result<u64, JournalError>; // -> seq
    fn sync(&mut self, mode: SyncMode) -> Result<(), JournalError>;
    fn truncate_tail(&mut self, good_seq: u64) -> Result<(), JournalError>;
}

pub trait JournalSource {
    fn scan(&mut self, from_seq: u64, visit: &mut dyn FnMut(FrameKind, &[u8]) -> std::ops::ControlFlow<()>)
        -> Result<ScanReport, JournalError>;
}
pub fn replay(src: &mut dyn JournalSource, base: Option<&Snapshot>) -> Result<ReplayOutcome, JournalError>;

// crates/neo-editor/src/conflict.rs
pub trait ConflictQueue: Send {
    fn push(&mut self, c: Conflict);
    fn peek(&self) -> Option<&Conflict>;
    fn resolve(&mut self, r: ConflictResolution) -> Result<Box<dyn EditCommand>, EditError>;
    fn len(&self) -> usize;
}
```

## 4 数据流与时序

### 4.1 一次提交的完整时序

```
UI ──execute──► Editor ──EditTx──► DataSet ──► Journal ──► Snapshot
                 │ apply(ops)        │ SoA 写入    │ 落盘       │ publish
                 │ validate          │             │            │
                 └─ 失败 ⇒ 零副作用（DataSet 未改，返回 Err）
                 成功 ⇒ inverse → version+1 → notify(UI / 渲染 / autosave)
```

## 5 权衡与替代方案

| 选择 | 替代 | 为什么选它 | 代价 |
|---|---|---|---|
| 影子层收集后统一 apply | 立即写 DataSet + 失败回滚 | 验证零副作用，工具代码简单 | overlay 读路径多一次查表；聚合查询需同时看两层 |
| 操作日志（op log） | 镜像日志（post-image） | 体积小、可表达"创建再删除"消解 | replay 依赖代码 schema，跨版本需转换器 |
| 逆操作在 apply 时捕获 | undo 时重新计算 | 唯一正确的方式（前像已被覆盖） | 内存随事务大小增长，需要容量上限 |
| `EditTx::insert_node` 自动分配负 id | 由调用方传入临时 id | 避免 id 冲突；集中管理 id 空间 | 负 id 的分配顺序必须写入日志才能复现 |

## 6 风险

| # | 风险 | 触发信号 | 缓解 |
|---|---|---|---|
| C1 | 逆操作内存爆炸（批量操作 100 万对象后 undo） | History bytes 逼近 cap，驱逐频繁 | 单事务 ops 上限（`EditPolicy`）+ 大操作要求确认 |
| C2 | replay 与当前代码 schema 不兼容 | 升级后启动提示"日志版本过新" | schema 版本门 + 每次发布跑"旧日志 replay"回归 |
| C3 | 提交管线顺序被改（journal 落后于 publish） | 崩溃后数据比日志新 | 管线顺序写成集成测试（`tests/commit_order.rs`） |
| C4 | 三方合并静默丢标签 | 用户报告"标签消失" | 自动合并只允许"不同键"；同键一律人工 |

## 7 验收标准

1. 任一 `apply` 成功但 `validate` 失败的事务，`DataSet` 的 `version` 与内容零变化（属性测试：随机命令 + 随机注入验证失败）。
2. `undo` 后再 `redo` 得到除 version 外逐字段相同的 `Snapshot`，覆盖 create/delete/modify/way 增删点/relation 成员。
3. 提交管线顺序由测试锁定：模拟"commit 后、publish 前"崩溃，replay 后状态与已提交状态一致。
4. 杀进程测试（SIGKILL / `TerminateProcess`）100 次随机注入，绝不出现"半个事务"【通过率待实测，见 docs/14-performance-and-verification.md】。
5. 对同一对象连续拖拽 60 帧只产生 1 条历史；间隔 > 500 ms 的两次拖拽产生 2 条。
6. 409 冲突集成测试：构造 6 类 `MergeCase`，`resolve` 后的 OSC 能被服务端（mock）接受。
7. 10 万对象批量操作时 History 内存不超过配置上限，驱逐后仍可 undo 最近 100 条。

## 8 参考

- OsmChange（OSC）规范：https://wiki.openstreetmap.org/wiki/OsmChange
- OSM API v0.6 上传与 409 冲突语义：https://wiki.openstreetmap.org/wiki/API_v0.6
- JOSM 冲突解决流程（行为对齐基准）：https://josm.openstreetmap.de/wiki/Help/ConflictResolution
- JOSM 撤销/命令栈的公开说明：https://josm.openstreetmap.de/wiki/Help/Action/Undo
- rkyv 0.8 归档与对齐要求：https://docs.rs/rkyv/0.8.18/rkyv/
- arc-swap 与快照切换：https://docs.rs/arc-swap/1.9.5/arc_swap/
- 项目内部取证记录（命令/冲突/JOSM 语义清单）：`.research/josm-inventory.md`、`.research/pages/conflict.txt`
