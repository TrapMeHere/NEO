# 08 · IO 层：PBF / XML / OSC 读写

本篇定义 `neo-io` 的职责边界：把外部 OSM 序列化格式（`.osm.pbf` / `.osm` / `.osc`）转换为 `neo-store` 可批量装载的批次，以及把 Store 中的差异写回这些格式。核心结论：读侧复用 `osmpbf 0.3.8` 的 blob 级并行解码，写侧因 `osmpbf` 只读而必须自研 encoder；所有写盘路径都经过同一条"分片—批—发布"流水线，使内存占用与输入规模线性相关而非爆炸。

## 1 目标与范围

| 编号 | 目标 | 判定方式 |
|---|---|---|
| G1 | 读取 1M–10M+ 对象的 `.osm.pbf`，不整体载入内存 | 峰值 RSS ≤ 解压总量的固定倍数【待实测，见 docs/14-performance-and-verification.md】 |
| G2 | 流式读取 `.osm`（OSM XML 0.6）与 `.osc`（OsmChange） | 单遍扫描，事件级内存 |
| G3 | 写出 `.osm.pbf` / `.osm` / `.osc`，OSC 可被 OSM API 直接接受 | 服务端 `diffResult` 无 400 |
| G4 | "导出→导入"往返后语义等价 | golden test 字节/语义双比对 |
| G5 | 导入可取消，且不留半成品快照 | 返回 `Err(ImportError::Cancelled)`，不发布 Snapshot |

**范围外**：`planet.osm.pbf` 的增量 replication diff（见 [09](./09-osm-api-and-sync.md)）、Overpass QL、GPX。

**会话文件**：`.jos`（XML）与 `.joz`（ZIP）由 `neo-persist/session.rs` 实现（图层引用 + viewport + selection，容忍未知插件图层段）；格式说明见该模块文档注释。
**约束**：`neo-io` 依赖 `neo-core`、`neo-osm`、`neo-store`，**不依赖** `neo-editor`/`neo-ui`/`neo-render`；只产出只读 `Snapshot` 与 `StoreBuilder` 批次，绝不原地修改已发布快照（单写者原则，见 [03](./03-osm-data-model-and-store.md)）。

## 2 设计

### 2.1 PBF 分层与 `osmpbf 0.3.8` 的用法

PBF 的物理层级是 `Blob[] → (HeaderBlock | PrimitiveBlock)`，每个 Blob 独立解压、独立解码——这是并行的唯一天然切分点。`osmpbf` 暴露三层，我们各用一层：

| 层 | API | 用途 |
|---|---|---|
| 字节层 | `BlobReader::from_path` / `next_header_skip_blob` | 只读 blob 索引（`ByteOffset`）用于预扫描与进度估算，不解压 |
| blob 层 | `Blob::decode() -> BlobDecode` | 并行解码单元；`OsmHeader`/`OsmData`/`Unknown` 三态 |
| 元素层 | `PrimitiveBlock::elements()` / `groups()` / `dense_nodes()` | 我们**不用** `ElementReader::for_each`（串行），也不用 `par_map_reduce` |

不用 `ElementReader` 的理由：`par_map_reduce` 的 `MP: Fn(Element<'a>) -> T` + `RD: Fn(T,T)->T` 只允许"逐元素折叠为可合并值"，而导入需要**按序批量追加**到 `StoreBuilder`，顺序决定引用可解析性。故调度自建：

```text
BlobReader ──(Blob, 顺序)──▶ 有界调度器 ──(rayon)──▶ decode + 落地
```

`required_features` 必须在装载前校验（[PBF wiki](https://wiki.openstreetmap.org/wiki/PBF_Format)）：`OsmSchema-V0.6`、`DenseNodes` 必须支持；`HistoricalInformation`、`LocationsOnWays` 属"能识别但降级"——前者保留 `visible=false` 的删除态对象（供后续上传删除），后者经 `Way::node_locations()` 取可选节点坐标。

### 2.2 DenseNodes、string table 与坐标精度

`DenseNodes` 是节点主编码：`id`/`lat`/`lon` 三组 delta 数组 + 共享 `keys_vals` 扁平数组（键值均为 string table 索引，`0` 表示该节点标签结束）。`osmpbf` 的 `DenseNodeIter` 已完成累加（`dids/dlats/dlons` + `cid/clat/clon`），我们只需处理精度与偏移：

```text
nano_lat = lat_offset + granularity * Δ累加lat      (granularity 默认 100)
lat度    = 1e-9 * nano_lat
```

**精度决策**：内部坐标统一 `i32` decimicrodegree（10⁻⁷°），与 API 0.6 的 7 位小数序列化、以及 `osmpbf` 的 `decimicro_lat() = (nano/100) as i32` 一致。风险在非默认 `granularity`：

| 情形 | 处理 |
|---|---|
| `% 100 == 0`（含默认 100） | 精确转换，无告警 |
| `< 100` 且非整除 | 以 `i64` nanodegree 暂存，装载时四舍五入到 10⁻⁷°，计入 `ImportReport::precision_loss_objects` |
| 偏移导致溢出 `i32` | 拒绝该 blob，`Err(ImportError::CoordinateOverflow)`（>180° 视为损坏） |

不选"全库 nanodegree (i64)"：坐标 16B/节点而非 8B，千万级多出近百 MB 常驻；不选 `f64`：10⁻⁷° 上非定点，往返出现 1e-9 漂移，破坏 golden 字节稳定。

**string table** 是每 `PrimitiveBlock` 独立的 `Vec<Vec<u8>>`，UTF-8 **不保证**，键值一律走 `str_from_stringtable` 的 `Result` 路径，非法 UTF-8 记 `ImportError::InvalidUtf8 { index }` 而非 panic。跨 block 不去重；去重在 `neo-core` 的 interning 层（一次导入的临时 intern 表有上限，超出按 LRU 丢弃，避免为 10M 对象保留全网 tag 词表）。

### 2.3 元素类型与顺序约束

PBF 硬约束（已核实）：**一个 `PrimitiveGroup` 绝不能混装不同类型**，默认约 8k 实体/group，且文件级不保证"全部 Node 先于全部 Way"。故导入不依赖文件顺序：

1. **单遍读取**，按类型写入三个独立分槽（nodes / ways / relations）；
2. Way/Relation 成员写成 `i64` 原始 id，**不做即时句柄解析**；
3. 读取完成后进入"解析期"，`StoreBuilder::finish()` 一次构建 `id → Handle` 映射、引用集与索引；
4. 缺失引用记入 `RefIntegrity`，不报错（partial data 合法，见 [09](./09-osm-api-and-sync.md) §2.6）。

代价是 Way 几何在装载期不可见（无法边读边画），收益是吞吐不受文件顺序影响。`Sort.Type_then_ID` 仅作**优化提示**：存在时解析期可用范围二分跳过不存在的 id 段。

### 2.4 流水线、背压与内存上限

```text
 ┌──────────────┐  Blob(≤32MiB 解压)  ┌────────────────────┐
 │ 读取线程(1)   │ ────bounded(8)───▶ │ rayon 解码池(N=核数) │
 │ BlobReader   │                    │ zlib + protobuf 解析 │
 └──────────────┘                    └─────────┬──────────┘
                                               │ ElementBatch
                                    ┌──────────▼───────────┐
                                    │ 主线程 聚合 StoreBuilder│
                                    │ append(批量、有序)     │
                                    └──────────┬───────────┘
                                               ▼ finish(): id 映射 → 引用集 → rstar bulk_load
                                    Snapshot::publish (只读, Arc)
```

背压有两个闸门，任一先到即阻塞上游：**槽位闸门** `channel::bounded(capacity)`（默认 `2 × rayon 线程数`，防 blob 排队）；**字节闸门** `ByteBudget`（默认 = 压缩文件大小 × 4，可配），每解码一个 blob 先 `reserve(block_bytes)`、消费后 `release`，防止多个 16MiB 解压块同时驻留。

`StoreBuilder` 用**预分配 + 1.5x 几何增长**（按 `HeaderBlock::bbox()` 与文件大小估算量级，仅用于 `with_capacity`）；Way 的节点索引单独用 `Vec<u32>` + 偏移表，避免 per-Way `Vec` 分配（1M Way × 24B 头开销 = 24MB 纯浪费）。

### 2.5 XML（`.osm`）流式读取

`quick-xml 0.42` 单遍事件循环：`read_event_into(&mut buf)`（复用缓冲，零分配），`Event::Start/Empty/End` 分派到 `start_node/flush_node`，忽略 `bounds`/`note`/`meta`，`buf.clear()` 放循环尾（借用检查器决定）：

```rust
loop {
    match reader.read_event_into(&mut buf)? {
        Event::Start(e) if e.name() == b"node"  => start_node(e.attributes())?,
        Event::Empty(e) if e.name() == b"node"  => flush_node()?,   // 无标签节点
        Event::Empty(e) if e.name() == b"tag"   => push_tag(e.attributes())?,
        Event::End(e)   if e.name() == b"node"  => flush_node()?,
        Event::Eof => break,
        _ => {}
    }
    buf.clear();
}
```

- **安全**：不解析 DTD/外部实体；显式拒绝 `Event::DocType`（`quick-xml` 默认不解析 DTD，但 XXE 与 billion-laughs 需上层兜底）。
- **属性语义**：`id` 必填；`version` 缺失 ⇒ 0（未上传）；`visible="false"` ⇒ 删除态；`action="delete"` 只出现在 `.osc`，出现在 `.osm` 中视为错误；`changeset`/`timestamp`/`uid`/`user` 进元数据。
- **上限**：单文件尺寸与元素数由 `ImportOptions` 限定，超限 `Err(ImportError::TooLarge)`——拒绝误拖入的 planet 级文件，而非性能优化。

### 2.6 OSC（OsmChange）读写

`.osc` 是 `create` / `modify` / `delete` 三段式，**动作作用于整个 primitive**，不存在标签级补丁，因此 `modify` 必须携带全量标签。删除块可带 `if-unused`（值被忽略），语义为"若被引用则跳过而不使整个 diff 失败"（[OsmChange](https://wiki.openstreetmap.org/wiki/OsmChange)）。

> **待验证**：任务描述中的"OSC 0.3"在公开规范中不存在。`osmChange` 的版本属性为 `version="0.6"`（JOSM `OsmChangeBuilder.DEFAULT_API_VERSION = "0.6"`，wiki 示例为 `<osmChange version="0.6">`）。NEO 按 0.6 实现；若"0.3"指内部格式号，需另行确认来源。

往返保真（round-trip）要求：

| 维度 | 要求 |
|---|---|
| 属性顺序 | 固定 `id, version, changeset, lat, lon, visible`；不要求等同输入，但要求**同输入两次导出字节一致**（确定性） |
| 浮点 | 坐标以整数 decimicro 直接 `{:.7}` 格式化，不经 `f64`，杜绝 `-0.0000000` 与末位抖动 |
| 转义 | `&<>"'` 与 `\r\n\t` 走统一 `escape_xml`；非 ASCII 原样 UTF-8，不转 `&#x…;` |
| 空元素 | `<node …/>` 自闭合 |
| 缩进 | 无缩进、`\n` 结尾，便于 diff 与 golden 比对 |

**golden test 思路**：`tests/fixtures/osc/*.osc` 与 `.expected.osc` 成对入库，跑 `parse → write` 做**字节相等**断言；另有 `semantic_roundtrip`（`parse → write → parse → 结构相等`）覆盖属性缺省、非法 UTF-8、`if-unused`。CI 三平台强制 LF（Windows CRLF 是假失败来源）。

### 2.7 写出：PBF / XML / OSC

**PBF 写出的现实**：`osmpbf 0.3.8` 是**只读**库（crate 自述 "A fast reader for the OpenStreetMap PBF file format"，源码中无 encoder），`osmium`/`libosmium` 是 C++ 依赖。故 NEO 自研最小 encoder，因为写侧只需**前向**编码：

- 手写 varint/zigzag/length-delimited 写出器（约 300 行，`bytes` 缓冲），按 `.proto` 权威 schema 生成 `BlobHeader`/`Blob`/`PrimitiveBlock`/`PrimitiveGroup`/`DenseNodes`/`Way`/`Relation`，不引入 protobuf 运行时依赖；
- 复用 `osmpbf` 自带的 `fileformat.proto` / `osmformat.proto` 作为字段号与默认值的**唯一事实来源**（避免手抄错字段号导致文件不可读）；
- 输出前用 `osmpbf` 反向读回自检；CI 对每次写出做"写→读→语义比对"，这是 encoder 唯一可信的验收方式；
- 压缩默认 zlib（与生态一致），`raw` 仅测试用；单 blob 解压后 < 16MiB（规范建议值），据此切分 DenseNodes 批次。

XML/OSC 写出共用 `XmlSink` trait，PBF 与 XML 只是三个 `impl`。

### 2.8 rstar 构建时机与 rkyv+mmap 的适用边界

**不边导入边插 rstar**：`RTree::insert` 单点插入且频繁重平衡；`RTree::bulk_load(Vec<T>)` 是一次性 O(n) 打包，并可利用 Hilbert/STR 排序获得更优聚集。故索引构建固定在 `StoreBuilder::finish()` 内、快照发布之前（构建期无旧快照可显示，但这是"打开文件"而非"增量编辑"场景，可接受）。

**rkyv + memmap2 归档**（`rkyv 0.8.18` 的 `to_bytes`/`access`，`memmap2 0.9.12` 只读映射）：

| 场景 | 适用 | 理由 |
|---|---|---|
| 只读大底图（国/省级抽取） | ✅ | 归档后零反序列化，`access` 直接借用内存；OS 页缓存按需换入 |
| 会话快照/自动保存 | ✅ | 写一次、极少读；崩溃恢复只需 `access` + 校验 |
| 频繁增量编辑的活跃层 | ❌ | 归档不可变，任何一次编辑都要整份重写；编辑态必须用 SoA |
| 跨版本兼容 | ❌ | 布局绑定类型定义与字节序；用 `schema_version` 显式拒旧归档，不承诺兼容 |
| 网络传输/跨机共享 | ⚠️ | 需固定字节序与指针宽度；仅同架构可用 |

对齐：`rkyv` 要求按类型对齐（通常 8/16B），`memmap2` 的文件内偏移不受控，故归档头写 `align_pad` 把根对象推到 16B 边界。

## 3 关键接口

```rust
// neo-io/src/lib.rs
pub struct ImportOptions {
    pub bbox: Option<BboxFilter>, pub threads: Option<usize>,   // None = rayon 默认
    pub bytes_budget: Option<u64>,                             // None = 压缩大小 * 4
    pub max_blob_in_flight: usize, pub keep_deleted: bool, pub precision: PrecisionPolicy,
}
pub struct ImportReport {
    pub nodes: u64, pub ways: u64, pub relations: u64, pub deleted: u64,
    pub precision_loss_objects: u64, pub ref_integrity: RefIntegrity, pub duration: Duration,
}
pub struct RefIntegrity {
    pub missing_nodes: u64, pub missing_members: u64,
    pub dangling_refs: Vec<(ObjectId, ObjectId)>,   // 采样上限 1024，其余只计数
}

pub struct BboxFilter { /* min/max lon/lat: i32 decimicro */ }
impl BboxFilter {
    pub fn new(min: LonLat, max: LonLat) -> Self;
    pub fn contains_node(&self, p: LonLat) -> bool;
    pub fn accepts_way(&self, node_hits: usize, envelope: Envelope) -> bool;
    pub fn accepts_relation(&self, member_hits: usize) -> bool;
}

/// 导入的消费者抽象：PBF/XML/OSC 三个前端共用一个后端。
pub trait ElementSink {
    fn begin(&mut self, header: &ImportHeader) -> Result<()>;
    fn nodes(&mut self, batch: &NodeBatch) -> Result<()>;
    fn ways(&mut self, batch: &WayBatch) -> Result<()>;
    fn relations(&mut self, batch: &RelationBatch) -> Result<()>;
    fn end(&mut self) -> Result<ImportReport>;
}

pub struct ImportPipeline { /* BlobReader 工厂 + rayon 池 + 预算 */ }
impl ImportPipeline {
    pub fn new(opts: ImportOptions) -> Self;
    pub fn run_pbf<P: AsRef<Path>, S: ElementSink>(&self, path: P, sink: &mut S) -> Result<ImportReport>;
    pub fn run_xml<P: AsRef<Path>, S: ElementSink>(&self, path: P, sink: &mut S) -> Result<ImportReport>;
    pub fn cancel(&self);                    // 幂等；已投递 batch 不再追加
}
pub struct StoreBuilderSink<'a> { /* &'a mut StoreBuilder */ }
impl ElementSink for StoreBuilderSink<'_> { /* ... */ }

pub enum ExportFormat { Pbf(PbfWriteOptions), Xml(XmlWriteOptions), Osc(OscWriteOptions) }
pub struct PbfWriteOptions { pub compression: Compression, pub max_block_entities: usize }
pub enum Compression { None, Zlib }
pub struct XmlWriteOptions { pub pretty: bool, pub visible_attr: bool }
pub struct OscWriteOptions { pub changeset: Option<i64>, pub if_unused: bool }

pub struct Exporter;
impl Exporter {
    pub fn write<S: SnapshotView, W: Write>(fmt: ExportFormat, snap: &S, w: W) -> Result<WriteReport>;
    pub fn write_osc<W: Write>(diff: &EditDiff, opts: OscWriteOptions, w: W) -> Result<WriteReport>;
}

pub struct OscWriter<W: Write> { /* 内部维护 current_mode 以合并连续同动作块 */ }
impl<W: Write> OscWriter<W> {
    pub fn new(w: W, opts: OscWriteOptions) -> Self;
    pub fn start(&mut self) -> Result<()>;
    pub fn write_created(&mut self, e: ElementView<'_>) -> Result<()>;
    pub fn write_modified(&mut self, e: ElementView<'_>) -> Result<()>;
    pub fn write_deleted(&mut self, e: ElementView<'_>) -> Result<()>;
    pub fn finish(&mut self) -> Result<WriteReport>;
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("pbf decode: {0}")] Pbf(String),
    #[error("xml at {line}: {msg}")] Xml { line: u64, msg: String },
    #[error("unsupported required feature: {0}")] UnsupportedFeature(String),
    #[error("coordinate out of range")] CoordinateOverflow,
    #[error("invalid utf-8 in string table at {index}")] InvalidUtf8 { index: u32 },
    #[error("input too large: {0}")] TooLarge(String),
    #[error("cancelled")] Cancelled,
}
```

## 4 数据流与时序

```text
UI/CLI        ImportPipeline      BlobReader      rayon pool       StoreBuilder
  │ start           │                 │               │                │
  │────────────────▶│ open + header   │               │                │
  │                 │── required_feat▶│               │                │
  │                 │◀─ HeaderBlock ──│               │                │
  │◀─ progress(0%) ─│                 │               │                │
  │                 │─ reserve(bytes) ──────────────────────────────▶ │
  │                 │── next_blob ────▶│               │                │
  │                 │                 │── Blob ──────▶│ decode         │
  │                 │                 │               │─ NodeBatch ───▶│ append
  │                 │◀─ release(bytes) ────────────────────────────── │
  │◀─ progress(n%) ─│        … 循环至 EOF …           │                │
  │                 │──── finish() ─────────────────────────────────▶ │ id 映射
  │                 │                 │               │                │ 引用集
  │                 │                 │               │                │ rstar bulk_load
  │                 │◀────────── Snapshot (Arc, 只读) ────────────────│
  │◀─ ImportReport ─│                 │               │                │
  │ cancel()        │                 │               │                │
  │────────────────▶│ 置 flag；丢弃 pending blobs；不调 finish()        │
  │◀─ Err(Cancelled)│                 │               │                │

.osc ─▶ OscReader ─▶ EditDiff{created/modified/deleted}
                        ├─▶ Exporter::write_osc（字节 golden 比对）
                        └─▶ Editor::apply_batch（本地预览）
```

## 5 权衡与替代方案

| 决策 | 选择 | 替代 | 放弃原因 |
|---|---|---|---|
| 并行粒度 | blob 级 | 元素级 | 元素级无法共享 string table；delta 编码使其不可随机切分 |
| 高层 API | 自建调度 + `BlobReader` | `ElementReader::par_map_reduce` | reduce 语义不支持背压与有序批量追加 |
| 坐标表示 | `i32` decimicro | `f64` / `i64` nano | `f64` 往返不定点；`i64` 内存翻倍 |
| PBF 写出 | 自研 encoder | `osmium` FFI | 引入 C++ 构建链与跨平台分发负担；`osmpbf` 无写侧 |
| XML 解析 | `quick-xml` 事件 | `serde-xml-rs`/DOM | DOM 与 planet 级输入不兼容；serde 派生处理不了"同名不同上下文" |
| 索引时机 | 导入末 `bulk_load` | 逐条 `insert` | 插入期重平衡 + 随机写显著劣于一次性打包 |
| 归档 | rkyv+mmap（只读底图） | 每帧查 SQLite | 见 [10](./10-persistence-and-recovery.md) §2.1 |

## 6 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| 恶意/损坏 blob（头大小、`raw_size` 撒谎） | OOM / panic | 依赖 `osmpbf` 的 64KiB/32MiB 硬限；再做 `raw_size` 与预算比对 |
| 非默认 `granularity` 数据 | 静默精度损失 | 导入报告计数 + `PrecisionPolicy::Reject` 严格模式 |
| string table 非法 UTF-8 | 导入中断 | 记错误 + 可配"跳过并计数"降级 |
| 自研 encoder 字段号错误 | 输出文件不可读 | 写→读自检 + CI 往返；禁止手改 `.proto` 副本 |
| mmap 归档被外部截断/改写 | SIGBUS / UB | 打开校验 magic + `schema_version` + 总长 + CRC；`unsafe` 集中在 `archive` 单模块 |
| 三平台换行/编码差异 | golden test 假失败 | fixture 强制 LF；写侧显式 `\n` |
| 取消竞态（正在写的 batch） | 半成品 | `finish()`/`cancel()` 互斥（`AtomicBool` + 保护 store 所有权的 `Mutex`） |

## 7 验收标准

- [ ] `cargo test -p neo-io` 覆盖三格式导入（含 `visible=false`、无标签节点、缺失引用）、导出、往返。
- [ ] 用 `osmpbf` 反向读取 NEO 写出的 `.osm.pbf`，计数与标签完全一致（encoder 验收）。
- [ ] 写出 OSC 的每个元素带 `changeset` 与 `version`（create 除外）；上传返回 `diffResult` 而非 400/409。
- [ ] `--bbox` 导入的结果中，所有保留 Way 至少一节点在框内（或按 `accepts_way` 策略显式记录）。
- [ ] 取消导入后：无快照发布、无临时文件残留、`StoreBuilder` 内存释放（`Drop` 断言 + 峰值采样）。
- [ ] 1M / 10M 对象导入的时间与峰值 RSS 记录在 [14](./14-performance-and-verification.md)，图表由 `benches/` 生成，本文不引用具体数值。
- [ ] `clippy -p neo-io -- -D warnings` 通过；`unsafe` 仅在 `archive`/`mmap` 模块且各带 `// SAFETY:` 说明。

## 8 参考

- PBF 格式（BlobHeader 32KiB/64KiB、Blob 16MiB/32MiB、`granularity` 默认 100、`date_granularity` 默认 1000、`lat_offset`、PrimitiveGroup 单类型、`required_features`、`Sort.Type_then_ID`）：<https://wiki.openstreetmap.org/wiki/PBF_Format>
- `osmpbf 0.3.8`（只读；`BlobReader`/`PrimitiveBlock`/`DenseNodeIter`）：<https://docs.rs/osmpbf/0.3.8/osmpbf/>
- `quick-xml 0.42`（`from_reader`、`read_event_into`、`config_mut`）：<https://docs.rs/quick-xml/0.42.0/quick_xml/>
- OsmChange（`create`/`modify`/`delete`、`if-unused`、`version="0.6"`）：<https://wiki.openstreetmap.org/wiki/OsmChange>
- OSM API v0.6（7 位小数坐标、`version`/`changeset` 语义）：<https://wiki.openstreetmap.org/wiki/API_v0.6>
- `rstar 0.14` `bulk_load`：<https://docs.rs/rstar/0.14.0/rstar/struct.RTree.html#method.bulk_load>
- `rkyv 0.8.18`：<https://docs.rs/rkyv/0.8.18/rkyv/> · `memmap2 0.9.12`：<https://docs.rs/memmap2/0.9.12/memmap2/>
- 相关：[数据模型与 Store](./03-osm-data-model-and-store.md) · [API 与同步](./09-osm-api-and-sync.md) · [持久化与恢复](./10-persistence-and-recovery.md) · [性能与验证](./14-performance-and-verification.md)
