# 03 · OSM 数据模型与存储

> 目标：让 1000 万级 OSM 对象在单机内存里**可编辑、可渲染、可上传**，同时保证删除/撤销下不出现"旧句柄指向新对象"的静默数据损坏。
>
> 本文与 `crates/neo-core`、`crates/neo-osm`、`crates/neo-store` 一一对应；接口名以代码为准。

## 1 目标与范围

- 定义 NEO 的**内存权威表示**（不是磁盘格式，不是渲染格式，不是 API 报文格式）。
- 给出每对象的字节账本、索引结构、编辑热路径语义。
- 明确**不负责**的内容：投影/几何（[04](./04-spatial-index-and-geometry.md)）、渲染批次（[05](./05-renderer-wgpu.md)）、Command/Undo（[06](./06-editing-core-and-undo.md)）、持久化编码（[10](./10-persistence-and-recovery.md)）。

## 2 设计

### 2.1 三个层次，刻意分离

```text
neo-osm   : Node / Way / Relation / Tags / ChangeState   ← 领域语义（DTO，IO/API/剪贴板用）
neo-store : NodeStore / WayStore / RelationStore / DataSet ← 物理存储（SoA，编辑/渲染用）
neo-core  : ObjectId / Handle / Sym / Coord / Meta        ← 原语
```

- `neo-osm` 的 DTO **不是**存储格式：它们会分配、会持有 `Tags`，只出现在边界上。
- `neo-store` 的 SoA **不是**领域模型：它不保证图元级不变量（如 relation 成员顺序），由编辑层保证。
- 这是 ADR-0004 的"三模型分离"在数据侧的具体形式。

### 2.2 外部 ID 与内部句柄

```rust
pub struct ObjectId(pub i64);          // 负数 = 本地新建（JOSM 约定）
pub struct Handle<T> { index: u32, generation: u32 }  // 8 字节，Copy
```

规则：

1. **OSM ID 绝不做数组下标**（删除/上传映射后会变，负数更不能）。
2. 句柄的 `generation` 在槽位释放时 +1；旧句柄 `Handle::index()/generation()` 必然与槽位不一致 → 失效。
3. 句柄的 `PhantomData<fn() -> T>` 让 `Handle<T>` 恒为 `Send+Sync+Copy`，与 `T` 无关。
4. `SlotAllocator`（`neo-core/src/handle.rs`）提供 `alloc/free/is_valid/generation`，三类 store 共用。

### 2.3 坐标：定点 `i32`（1e-7 度）

```rust
pub struct Coord(pub i32);            // 1 单位 = 1e-7 度 = OSM API 精度
pub struct LatLon { pub lat: Coord, pub lon: Coord }
```

| 方案 | 每坐标 | 10M 节点坐标内存 | 问题 |
|---|---|---|---|
| `f64` | 8 B | 160 MB | 浮点漂移/相等比较不可靠；PBF→上传舍入不可控 |
| `i64` nanodegree | 8 B | 160 MB | PBF 保真但内存翻倍 |
| **`i32` 1e-7** | **4 B** | **80 MB** | 与 API 精度一致；PBF 1e-9 值在导入时四舍五入 |

选择 `i32` 1e-7：经度 ±180° → ±1.8e9，落在 `i32`（±2.147e9）；纬度 ±90° → ±9e8。PBF 的 `decimicro_lat/lon` 恰好就是 1e-7 度整数，**导入零转换、零浮点**（`osmpbf` 直接提供）。

副作用与对策：

- PBF 中 1e-9 精度会在导入时按"四舍五入到 1e-7"归一；这与 OSM API 的最终精度一致。
- 若将来需要 PBF 字节级往返，增加可选的 `i64` 高精度变体（存储层已按 `Coord` 抽象，替换点集中）。
- 校验器必须拒绝越界坐标（`Coord::is_valid_lat/is_valid_lon`）。

### 2.4 元数据：32 字节 `Copy`

```rust
#[repr(C)]
pub struct Meta {
    pub version: u32,
    pub timestamp: i64,     // Unix 秒
    pub changeset: i64,
    pub uid: u32,
    pub user: Sym,          // 4 B 符号
    pub visible: bool,
}
```

- 完整保留 version/timestamp/changeset/uid/user/visible：上传 diff、冲突解决、历史查看都需要，**不能为性能丢弃**。
- 10M 对象 × 32 B = 320 MB，是内存账本里最大单项；可选压缩（changeset 用 varint arena、user 用 uid 反查）在阶段 2 评估。

### 2.5 Tag：驻留符号 + 追加式 arena

```rust
pub struct Sym(u32);                    // 4 字节，索引 0 = 空串
pub struct StringPool { ... }           // &str -> Sym, Sym -> &str
pub struct TagSetId(u32);               // 4 字节
pub struct TagStore { pairs: Vec<(Sym, Sym)>, spans: Vec<u32> }  // 区间式 arena
```

- tag 文本只存一份；`highway`、`building` 等高频 key/value 让 10M 对象的标签内存从 GB 级降到百 MB 级。
- 写路径 **append-only**：编辑一个对象的 tags = 追加一条新 TagSet + 更新该对象的 `TagSetId`；旧区间暂时泄漏，`compact()` 时回收。好处是修改无引用计数、无失效风险、写入可预测。
- `Tags::content_hash()` 提供与顺序无关的内容哈希（样式/几何缓存 key）。

### 2.6 三类图元的 SoA 布局

```rust
pub struct NodeStore {
    ids:   Vec<ObjectId>,     //  8 B
    lat:   Vec<i32>,          //  4 B
    lon:   Vec<i32>,          //  4 B
    tags:  Vec<TagSetId>,     //  4 B
    meta:  Vec<Meta>,         // 32 B
    alloc: SlotAllocator,
}

pub struct WayStore {
    ids: Vec<ObjectId>, tags: Vec<TagSetId>, meta: Vec<Meta>,
    offsets: Vec<u32>, counts: Vec<u32>,          // 每 way 的引用区间
    node_refs: Vec<Handle<NodeSlot>>,             // 连续引用池
    alloc: SlotAllocator,
}

pub struct RelationStore {
    ids: Vec<ObjectId>, tags: Vec<TagSetId>, meta: Vec<Meta>,
    offsets: Vec<u32>, counts: Vec<u32>,
    members: Vec<StoredMember>,                   // role: Sym + target: AnyHandle
    alloc: SlotAllocator,
}
```

`StoredMember { role: Sym, target: AnyHandle }`，`AnyHandle = (kind, index, generation)`；成员**顺序即数据**（route/multipolygon 语义），只追加、不排序。

### 2.7 内存账本（估算，未含索引）

| 类型 | 每对象 | 10M 规模 |
|---|---|---|
| Node | 8+4+4+4+32 = **48 B** | 480 MB |
| Way（平均 5 节点） | 8+4+32+4+4 + 5×8 = **92 B** | 若 10M way → 920 MB |
| Relation（平均 4 成员） | 8+4+32+4+4 + 4×16 = **116 B** | — |
| TagSet（平均 4 tag） | 4 B/对象 + 4×8 B tag 对 | + ≈ 360 MB |
| ID 索引（FxHashMap） | ≈ 20 B/对象 | ≈ 200 MB |
| 反向引用 CSR | ≈ 4–12 B/对象 + 引用 | ≈ 100–300 MB |
| 字符串池 | 唯一文本 | 通常 < 20 MB |

> 这些是**估算**，实测见 [14](./14-performance-and-verification.md) 的"内存实测"表（原型报告 `out/prototype-bench.md` 与 PowerShell 的 PeakWorkingSet）。

### 2.8 `DataSet` 与编辑热路径

```rust
pub struct DataSet {
    pub strings: StringPool,
    pub tags: TagStore,
    pub nodes: NodeStore,
    pub ways: WayStore,
    pub relations: RelationStore,
    pub ids: IdIndex,
    pub changes: ChangeTrack,      // 稀疏：只记录被编辑对象
    version: u64,                  // 每次提交递增；渲染缓存失效依据
    next_local_id: i64,            // 负数递减
    dirty_nodes/dirty_ways: BitSet,
}
```

编辑一个节点坐标的完整语义：

```text
set_node_position(handle, pos)
  ├─ 校验经纬度范围（否则 InvalidCoordinate）
  ├─ 校验句柄 generation（否则 StaleHandle）
  ├─ 写 lat/lon 列
  ├─ dirty_nodes.set(index)        ← 渲染/拾取/验证的增量依据
  └─ version += 1                  ← SceneFrame/缓存失效
```

**不在这里做的事**：更新空间索引、重建几何、标记关联 Way。那些由 `neo-editor` 的 Command 完成（[06](./06-editing-core-and-undo.md)），因为"移动节点"对 Way/Relation/索引的影响属于编辑语义，不是存储语义。

### 2.9 索引层

| 索引 | 结构 | 用途 | 构建成本 |
|---|---|---|---|
| `IdIndex` | `FxHashMap<ObjectId, Handle>` × 3 | ID→句柄，下载/上传/跳转 | O(n) |
| `Referrers` | CSR（3 组） | node→ways、node→relations、way→relations | O(n+e) |
| `SpatialIndex` | rstar R-tree（在 `neo-geo`） | 视口裁剪、拾取 | O(n log n) |
| `dirty_*` | `BitSet` | 增量重建依据 | O(1)/对象 |
| `ChangeTrack` | 三个稀疏 `FxHashMap` | 上传 diff、脏状态 | O(脏对象) |

选择理由与替代方案见 ADR-0003。

### 2.10 视图与借用模型

```rust
DataSet::node_view(handle) -> Option<NodeView<'_>>     // { info, tag_pairs }
DataSet::way_view(handle)  -> Option<WayView<'_>>      // { info, tag_pairs, node_refs }
DataSet::relation_view(handle) -> Option<RelationView<'_>>
```

- 视图只借用 `&DataSet`，不复制数据、不持有句柄生命周期之外的引用。
- 写路径是 `&mut DataSet`，由单写者（编辑线程）持有（ADR-0006）；渲染/后台/插件读的是 `Arc` 快照或按版本校验后的结果（[07](./07-concurrency-and-tasks.md)、[15](./15-plugin-system.md)）。
- `neo-query` 把"只读查询"抽成 `MapQuery` trait，插件/验证器只依赖它，不依赖存储的写接口。

## 3 关键接口

```rust
// neo-core
impl ObjectId { fn is_local(self) -> bool; fn raw(self) -> i64; }
impl<T> Handle<T> { fn index(self) -> u32; fn generation(self) -> u32; }
impl Coord { fn from_units(i32) -> Self; fn units(self) -> i32; fn from_nanodegrees(i64) -> Self; }
pub struct StringPool { fn intern(&mut self, &str) -> Sym; fn resolve(&self, Sym) -> &str; }

// neo-store
impl DataSet {
    fn new() -> Self;
    fn next_local_id(&mut self) -> ObjectId;
    fn intern(&mut self, &str) -> Sym;
    fn push_node(&mut self, Node) -> Handle<NodeSlot>;
    fn push_way(&mut self, Way, MissingPolicy) -> Result<Option<Handle<WaySlot>>>;
    fn push_relation(&mut self, Relation, MissingPolicy) -> Result<Option<Handle<RelationSlot>>>;
    fn set_node_position(&mut self, Handle<NodeSlot>, LatLon) -> Result<LatLon>;
    fn set_way_nodes(&mut self, Handle<WaySlot>, &[Handle<NodeSlot>]);
    fn node_view/way_view/relation_view(..) -> Option<..View<'_>>;
    fn take_dirty_ways/take_dirty_nodes(&mut self) -> Vec<Handle<..>>;
    fn build_referrers(&self) -> Referrers;
    fn stats(&self) -> StoreStats;   // Display: counts + memory
    fn mem_bytes(&self) -> ByteSize;
}
```

## 4 数据流

```text
PBF/XML  ──neo-io──►  DTO(Node/Way/Relation)  ──DataSet::push_*──►  SoA
                                                                     │
                              ┌──────────────────────────────────────┤
                              ▼                                      ▼
                    SpatialIndex::build                    Referrers::build
                    （视口/拾取）                            （父引用/增量失效）
                              │                                      │
                              ▼                                      ▼
                    SceneBuilder::build                    Command/Validator/Plugin
                    （SceneFrame）                          （只读查询）
```

## 5 权衡与替代

见 ADR-0003 的对照表（`Vec<Box<Object>>`、slotmap、ECS、持久化数据结构、SQLite 主存储、内嵌 String）。

补充两个**本期未做、但已留缝**的决定：

1. **快照的 COW**：`neo-query::MapSnapshot` 目前是 `Arc<DataSet>`（发布成本 O(n)）。真正高频发布需要分块 COW（`ChunkedVec<T>`，只克隆被改动的 64K 块）。接口已按"不可变快照"设计，替换实现不影响上层。
2. **arena 压缩**：`TagStore` 与 Way/Relation 的引用池是追加式，长期编辑会膨胀。`compact()` 需要同步更新所有 store 的引用，计划在阶段 2 与 autosave 一起做。

## 6 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| 追加式 arena 无界增长（长时间编辑） | 内存膨胀 | 监控 `stats()`；阶段 2 compact；autosave 重写快照 |
| ID 索引 20 B/对象在 10M 下约 200 MB | 内存压力 | 只读阶段可换"排序数组+二分"；或分片哈希 |
| 定点坐标 1e-7 与 PBF 1e-9 不等价 | 往返比较误报 | golden test 用**语义等价**（按 1e-7 归一后比较），docs/18 |
| 句柄 generation 溢出（u32） | 极端复用后误判有效 | 溢出即 panic/诊断日志；实际不可达（需同一槽位 42 亿次复用） |
| 视图/快照混用导致读到半更新 | 渲染撕裂/校验错误 | 单写者 + 快照发布（[07](./07-concurrency-and-tasks.md)） |

## 7 验收标准

1. `cargo test -p neo-core -p neo-osm -p neo-store` 全绿，覆盖：generation 失效、槽位复用、Way 引用连续、Tag append-only、内存估算。
2. 1000 万节点导入 + 索引的实测内存与时间记录在 [14](./14-performance-and-verification.md)，偏差 > 30% 需解释。
3. `NeoApp` 中不存在 `data` 之外的第二个可变 `DataSet`；`grep -r "&mut DataSet"` 只出现在编辑/导入路径。
4. 所有对外句柄都经 generation 校验（测试用旧句柄访问必须返回 `None`）。

## 8 参考

- OSM PBF 格式（node/way/relation、DenseNodes、string table）：<https://wiki.openstreetmap.org/wiki/PBF_Format>
- OSM API 0.6（1e-7 度精度、changeset 语义）：<https://wiki.openstreetmap.org/wiki/API_v0.6>
- rstar R-tree：<https://docs.rs/rstar/0.14.0/rstar/>
- SoA/AoS 与缓存：<https://en.wikipedia.org/wiki/AoS_and_SoA>
- 本项目代码：`crates/neo-core/src/{handle,units,sym,meta}.rs`、`crates/neo-store/src/{node_store,way_store,relation_store,tag_store,indexes,dataset}.rs`
