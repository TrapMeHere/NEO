# 09 · OSM API 0.6 与同步（API & Sync）

本篇定义 `neo-api`：面向 OSM API 0.6 的客户端与同步策略，覆盖 OAuth2 鉴权、changeset 生命周期、diff 上传、冲突处理、按需下载与离线队列。设计基调是**乐观锁 + 服务端事务不跨请求**：客户端必须自己承担"版本校验、分块、补偿"的责任，因为服务端只保证单次 `upload` 请求的原子性。

## 1 目标与范围

| 编号 | 目标 | 判定 |
|---|---|---|
| S1 | 完整替代 JOSM 上传路径：鉴权 → 建 changeset → 上传 diff → 关 changeset | `diffResult` 全量成功 |
| S2 | 冲突（409/412）可定位到具体对象并给出解决入口 | `ConflictReport` 逐对象可操作 |
| S3 | 断网/失败不丢编辑：排队 + 重放 | 重启后队列仍在（[10](./10-persistence-and-recovery.md)） |
| S4 | 下载结果满足引用完整性，缺失引用**显式建模** | `DownloadBatch::incomplete` 非空即 UI 提示 |

**范围外**：Overpass、replication diff（minute/hourly）、GPX 上传（multipart）、diary、用户偏好写入；`/api/0.6/notes` 仅做读与"新建 note"。
**依赖**：`neo-api → neo-core, neo-osm`（不依赖 store/editor；差异由上层以 `EditDiff` 传入）；网络栈 `tokio 1.54` + `reqwest 0.13.5`，错误 `thiserror 2.0`。

## 2 设计

### 2.1 端点矩阵（已核实）

> **重要更正**：任务描述中的 `POST /api/0.6/changeset/create` 实际是 **`PUT`**（[API v0.6 §2.2.2](https://wiki.openstreetmap.org/wiki/API_v0.6)）。与 `POST changeset/{id}/upload` 混淆会直接得到 405。

| 用途 | 方法与路径 | 鉴权 | 关键约束 |
|---|---|---|---|
| 服务能力 | `GET /api/0.6/capabilities` | 读免鉴权 | `area maximum="0.25"`、`waynodes=2000`、`relationmembers=32000`、`changesets maximum_elements=10000`、`note_area=25` |
| 建 changeset | `PUT /api/0.6/changeset/create` | `write_api` | body `<osm><changeset><tag k="comment" v="…"/></changeset></osm>`，响应为纯文本 id |
| 改标签 | `PUT /api/0.6/changeset/{id}` | `write_api` | 追加 `comment`/`created_by` |
| 关闭 | `PUT /api/0.6/changeset/{id}/close` | `write_api` | 关闭后再上传 ⇒ 409 |
| 上传 diff | `POST /api/0.6/changeset/{id}/upload` | `write_api` | body 即 OsmChange XML（`application/xml`），**非 multipart** |
| 按框下载 | `GET /api/0.6/map?bbox=left,bottom,right,top` | 读免鉴权 | 面积 ≤ 0.25 平方度；超 **50,000 节点** ⇒ 400 |
| 多对象取回 | `GET /api/0.6/{nodes\|ways\|relations}?{type}=id,…` | 读免鉴权 | 补全缺失成员的主手段 |
| 引用者 | `GET /api/0.6/{type}/{id}/relations` | 读免鉴权 | parent 递归 |
| 完整对象 | `GET /api/0.6/{way\|relation}/{id}/full` | 读免鉴权 | 连带节点与成员 |
| notes | `GET /api/0.6/notes?bbox=…`、`POST /api/0.6/notes` | 读 / `write_notes` | 面积 ≤ 25 平方度 |

上传体**不是** `multipart/form-data`（multipart 是 GPX 上传 `/api/0.6/gpx` 的要求）；`Content-Type` 可省略（默认 XML/UTF-8），2018 年起支持请求体 gzip（`Content-Encoding: gzip`）。响应 `diffResult` 每个上传元素一行，同一元素出现多次则输出多次：

```xml
<diffResult generator="OpenStreetMap Server" version="0.6">
  <node old_id="-1" new_id="123456" new_version="1"/>
  <way  old_id="-2" new_id="123457" new_version="2"/>
</diffResult>
```

`create` 行含 `new_id`+`new_version`；`modify` 行 `new_id` 同输入、`new_version` 递增；`delete` 行**无 `new_version`**。占位 id（负数）→ 真实 id 的重映射**只能**依赖此响应，这是本地 Store 与服务器对齐的唯一权威。

### 2.2 鉴权（OAuth 2.0）

OAuth 1.0a 与 HTTP Basic 已于 2024 年 6 月下线，只剩 OAuth 2.0。所需 scope：

| scope | 权限名 | 用途 |
|---|---|---|
| `read_prefs` | `allow_read_prefs` | 用户偏好、`user/details` |
| `write_api` | `allow_write_api` | 改地图 + changeset 评论 |
| `write_notes` | `allow_write_notes` | notes 写入 |
| `consume_messages` | `allow_consume_messages` | 站内信（可选） |

scope 在 `/permissions` 中统一加 `allow_` 前缀——比较权限时必须归一，否则永远判"未授权"。**令牌存储**：access/refresh token 进 OS 凭据库（Windows Credential Manager / macOS Keychain / libsecret），**绝不**进 SQLite 或配置文件；SQLite 只存 `account_id` 与 scope 摘要。刷新失败降级为只读模式，上传按钮禁用并显示原因。

> 待验证：OAuth2 授权/令牌端点精确路径（通常 `https://www.openstreetmap.org/oauth2/authorize` 与 `/oauth2/token`）本次未能联网核实（`web_fetch` 失败）。实现前需按 <https://wiki.openstreetmap.org/wiki/OAuth> 复核，并确认 PKCE 是否必需。

### 2.3 HTTP 客户端：连接池、限流、重试、取消

- **连接池**：单个 `reqwest::Client` 全局复用（HTTP/2 多路复用、keep-alive），`pool_max_idle_per_host(4)`，`timeout(30s)` + `connect_timeout(5s)`。
- **并发**：下载默认 **2**（对齐 OSM 使用策略；JOSM 亦以 `MAX_DOWNLOAD_THREADS = 2` 实现）；上传恒为 **1**（同一 changeset 顺序上传，避免 bbox 竞争与顺序错乱）。
- **限流**：令牌桶（默认 2 req/s、突发 4）；命中 429 按 `Retry-After` 退避，并把桶容量临时减半、成功后缓慢恢复。
- **重试**：仅幂等请求（GET、`capabilities`）与"明确未提交"的失败（连接建立失败、TLS 错误）自动重试；`upload` **只在请求未发出时**（`reqwest` 的 `is_connect()`）重试，否则返回 `Ambiguous` 让上层查证，绝不冒重复创建对象的险。
- **退避**：指数 + 抖动，基数 500ms，上限 60s，最多 5 次（与 JOSM `DEFAULT_MAX_NUM_RETRIES = 5` 同量级）。
- **取消与进度**：自建 `CancelToken`（`Arc<AtomicBool>` + `tokio::sync::Notify`，不引入 `tokio-util`）；所有 `await` 点插入 `tokio::select!`，取消后请求被 drop（HTTP/1.1 关连接，HTTP/2 RST_STREAM）。进度用 `watch::Receiver<Progress>`，按"已下载对象数/已上传元素数"推进。

### 2.4 changeset 生命周期

```text
 ┌────────┐  create(PUT)   ┌────────┐  upload(cls)  ┌───────────┐  close  ┌────────┐
 │  Idle  │───────────────▶│  Open  │──────────────▶│ Uploading │────────▶│ Closed │
 └────────┘                └────────┘               └───────────┘         └────────┘
      ▲                        │  ▲ 追加标签 / 失败保持 Open
      └── 丢弃（未建）──────────┘  └── 空闲超时自动关闭
```

1. **延迟创建**：用户首次点"上传"时才 `PUT changeset/create`，避免空 changeset 污染历史。
2. **标签**：`comment`（必填，空则禁用上传）、`created_by=NEO/<version>`、`host`、`locale`，可选 `source`、`imagery_used`。`created_by` 是社区规范而非 API 要求，缺失会被质疑。
3. **自动关闭**：`Open` 且空闲超阈值（默认 30 分钟，可配）时后台 close，避免服务端 24h 自动关闭导致"上传到已关闭 changeset ⇒ 409"。
4. **超限预检**：上传前用 `capabilities.changesets.maximum_elements`（当前 10000）与已上传元素数比对，超出即要求分 changeset——不要依赖 413 才发现。
5. **关闭失败**：409（已关闭）视为成功（幂等）；其它错误保留 `Open` 并提示，下次上传复用同一 changeset。

### 2.5 上传流水线

```text
EditDiff(本地差异)
 │ 1. 校验：无 version=0 的 modify；delete 依赖序（先 relation 后 way 后 node）
 ▼
UploadPlan{changeset 标签, chunks[]}
 │ 2. 冲突预检（默认开）：multi_fetch 所有 modify/delete 的当前 version
 │    不一致 ⇒ 直接产出 ConflictReport（省掉一次必然失败的 409）
 ▼
for chunk in chunks:                          ← 顺序、单并发
 │ 3. 序列化：OscWriter 输出（changeset/version 必填，create 免 version）
 │ 4. POST upload（可 gzip）
 │    ├─ 200 + diffResult ⇒ 5
 │    ├─ 409 ⇒ version 不匹配 / changeset 已关闭 / 非本人（text/plain）
 │    ├─ 412 ⇒ 缺失节点或被引用（"Way #1 requires the nodes with id in (…)"）
 │    └─ 429/5xx ⇒ 退避重试（仅当可判定请求未生效）
 ▼
5. 应用 diffResult：占位 id → 真实 id；version 覆盖；经 Editor 写回 Store
 │
6. 全部成功 ⇒ close；任一失败 ⇒ 保持 Open + ConflictReport
```

**分块与原子性的矛盾**（本设计最重要的取舍）：服务端对**单次** `upload` 保证"全部应用或全不应用"，但**不跨请求**。

| 方案 | 原子性 | 代价 |
|---|---|---|
| 单请求上传整个 changeset | ✅ 服务端事务 | 受 `maximum_elements`=10000 与请求体大小限制 |
| 多请求分块 | ❌ 部分成功 | 需客户端补偿 |
| 多 changeset | ❌ 语义割裂 | 历史里出现多个 changeset，社区不友好 |

**决策**：优先单请求（覆盖绝大多数会话）；仅当元素数 > `maximum_elements` 或请求体超阈值（默认 8MiB，避免代理截断）才分块，且必须：(1) 每块成功后立即持久化 `uploaded_until`，崩溃后**续传而非重传**；(2) 失败块之后不再继续，保持 `Open`，UI 显示"已上传 N 块，剩余 M 块"；(3) 放弃时提供"补偿上传"——对已上传元素生成**逆向 OSC**（modify 回旧值 / delete 新建对象）作为新请求。补偿是**尽力而为**，不是事务回滚：服务端无 revert 端点，逆向 diff 本身也是合法编辑，期间若有人改动同样会 409。

**失败语义**（必须准确，不允许含糊）：

| 失败点 | 服务端状态 | 客户端动作 |
|---|---|---|
| 请求未发出 / 连接失败 | 未变更 | 安全重试 |
| 已发出但无响应（超时） | **未知** | 标记 `Ambiguous`，先 `GET` 核对 version 再决定 |
| 409（version 被他人更新） | 未变更（整请求回滚） | 产出 `ConflictReport` |
| 409（changeset 已关闭） | 未变更 | 新建 changeset 重传 |
| 412（引用缺失/被引用） | 未变更 | 补下载缺失成员或对删除用 `if-unused` |

### 2.6 下载策略与引用完整性

| 策略 | 实现 | 适用 |
|---|---|---|
| 按框 | `GET /map?bbox=`，面积 ≤ 0.25 平方度、节点 ≤ 50k | 主下载 |
| 分片 | 切成 ≤ 0.25 平方度的瓦片，双并发 | 城市级区域 |
| 沿路径/轨迹 | 几何包络 + 走廊缓冲（默认 100m）生成细长 bbox 序列，去重后逐块下载 | 沿道路测绘 |
| parent 递归 | `GET {type}/{id}/relations` 逐层上溯，默认深度 3 | 避免破坏关系 |
| 成员补全 | `GET ways?ways=…` / `nodes?nodes=…` 批量取回 | incomplete members |
| 完整对象 | `GET {way,relation}/{id}/full` | 单对象深链 |

**partial data 下 Way 缺 Node 的处理**（显式，不静默丢弃）：

1. Way 允许引用不存在的 `ObjectId`，Store 中解析为 `Handle::MISSING`（generation=0 哨兵）；
2. 渲染层跳过缺失节点、在断口处画断口标记（**不**连成直线，避免伪造几何）；
3. `neo-validate` 产出 `incomplete_way` 警告，**上传被硬阻断**：过滤任何引用缺失节点的 Way，并在 `ConflictReport` 说明原因——服务端会以 412 拒绝整个 diff，本地拦截代价低得多；
4. 用户可选择"补全并重试"：自动 multi_fetch，仍失败则标 `deleted`（服务端语义为不可见）。

> 细节：`GET /map` 只返回"框内有节点"的道路；与框相交但节点全在框外的 Way **不会**返回。框边界的 Way 因此常以"缺节点"形态出现，这是 partial data 的常见来源，UI 文案必须解释清楚。

### 2.7 离线队列

编辑产生的待上传差异落入 SQLite 的 `changeset_queue`（DDL 见 [10](./10-persistence-and-recovery.md) §2.2）：`client_uuid` 为幂等键（同一编辑重复入队只留一份）；`state ∈ {queued, uploading, uploaded, conflicting, aborted}` 的迁移必须与 `uploaded_until` 在同一事务提交；重放按 `seq` 且**跨 changeset 不重排**（先建后引的依赖不可交换）；离线期间 UI 显示"未同步 N 项"，同一对象上的第二个编辑合并进同一 payload。

## 3 关键接口

```rust
// neo-api/src/lib.rs
pub struct ApiConfig { pub base_url: reqwest::Url,      // 默认 https://api.openstreetmap.org
    pub user_agent: String,                             // 必填：NEO/0.1 (+https://…)
    pub max_download_concurrency: usize,                // 默认 2
    pub request_timeout: Duration, pub max_retries: u32, pub gzip_upload: bool, pub offline: bool }

pub struct OsmApiClient { /* reqwest::Client + AuthProvider + RateLimiter + CancelToken */ }
impl OsmApiClient {
    pub fn new(cfg: ApiConfig, auth: Arc<dyn AuthProvider>) -> Result<Self, ApiError>;
    pub async fn capabilities(&self) -> Result<Capabilities, ApiError>;
    pub async fn map_bbox(&self, bbox: Bbox) -> Result<DownloadBatch, ApiError>;
    pub async fn multi_fetch(&self, ids: &ObjectIdSet) -> Result<DownloadBatch, ApiError>;
    pub async fn full(&self, id: ObjectId) -> Result<DownloadBatch, ApiError>;
    pub async fn parents_of(&self, id: ObjectId) -> Result<Vec<ObjectId>, ApiError>;
    pub async fn notes(&self, bbox: Bbox) -> Result<Vec<Note>, ApiError>;
    pub async fn create_changeset(&self, meta: &ChangesetMeta) -> Result<i64, ApiError>;
    pub async fn update_changeset(&self, id: i64, meta: &ChangesetMeta) -> Result<(), ApiError>;
    pub async fn close_changeset(&self, id: i64) -> Result<(), ApiError>;
    pub async fn upload_diff(&self, cs: i64, osc: &[u8]) -> Result<DiffResult, ApiError>;
    pub fn cancel_token(&self) -> CancelToken;
    pub fn progress(&self) -> tokio::sync::watch::Receiver<Progress>;
}

pub struct Capabilities { pub max_area_sq_deg: f64,      // 0.25
    pub max_note_area_sq_deg: f64,                      // 25
    pub max_way_nodes: u32,                             // 2000
    pub max_relation_members: u32,                      // 32000
    pub max_changeset_elements: u32,                    // 10000
    pub database_status: ServiceStatus, pub api_status: ServiceStatus }

#[async_trait::async_trait]
pub trait AuthProvider: Send + Sync {
    async fn access_token(&self) -> Result<secrecy::SecretString, ApiError>;
    async fn scopes(&self) -> Result<Vec<Scope>, ApiError>;
    fn is_read_only(&self) -> bool;
}

pub struct ChangesetManager { /* 状态机 + 空闲计时 + 队列写回 */ }
impl ChangesetManager {
    pub fn new(client: Arc<OsmApiClient>, queue: Arc<dyn QueueStore>) -> Self;
    pub async fn ensure_open(&mut self, meta: &ChangesetMeta) -> Result<i64, ApiError>;
    pub async fn upload(&mut self, plan: UploadPlan) -> Result<UploadOutcome, ApiError>;
    pub async fn close(&mut self) -> Result<(), ApiError>;
    pub fn state(&self) -> ChangesetState;
}

pub struct UploadPlan { pub changeset_meta: ChangesetMeta, pub chunks: Vec<UploadChunk>, // 通常恰 1 个
    pub precheck_versions: bool, pub compensation: CompensationPolicy }  // None | InverseOsc
pub struct UploadChunk { pub osc: Vec<u8>, pub element_count: usize, pub kind: ChunkKind }

pub struct ConflictReport { pub kind: ConflictKind, pub changeset_closed: Option<ConflictSummaryLine>,
    pub per_object: Vec<ObjectConflict>, pub message: String }   // 服务端 text/plain 原文
pub struct ObjectConflict { pub id: ObjectId, pub stage: ConflictStage,  // Precheck | Upload409 | Upload412
    pub local_version: i32, pub remote_version: Option<i32>, pub resolution: ConflictResolution }

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized: {0}")] Auth(String),
    #[error("rate limited, retry after {0:?}")] RateLimited(Duration),
    #[error("conflict: {0}")] Conflict(Box<ConflictReport>),
    #[error("payload rejected: {0}")] Rejected(String),        // 400/412/413
    #[error("ambiguous outcome: request may have been applied")] Ambiguous,
    #[error("cancelled")] Cancelled,
    #[error("network: {0}")] Transport(#[from] reqwest::Error),
    #[error("decode: {0}")] Decode(String),
}
```

## 4 数据流与时序

```text
用户   ChangesetManager   OsmApiClient        服务端          Store/Editor
 │ 上传      │                 │                │                 │
 │──────────▶│ ensure_open     │                │                 │
 │           │── PUT create ──▶│───────────────▶│ id=555          │
 │           │ precheck: multi_fetch 现网 version│                 │
 │           │────────────────▶│───────────────▶│                 │
 │           │◀── node 42 local=3 remote=5 ──────│                 │
 │◀─ 冲突面板（TakeRemote / Merge / Skip）        │                 │
 │ 选 TakeRemote for 42      │                │                 │
 │──────────▶│ 重算 diff（丢弃本地 42 的标签改动）                │
 │           │── POST upload（OsmChange）──────▶│ 事务：全成功/全回滚│
 │           │◀──── diffResult(old→new) ────────│                 │
 │           │ 应用 id/version 重映射 ───────────────────────────▶│
 │           │── PUT close ──▶│───────────────▶│                 │
 │◀─ 完成 ───│                 │                │                 │

超时 ⇒ Ambiguous：
client ──POST upload──▶ [超时] ──▶ 不重试
client ──GET /api/0.6/node/42─▶ remote_version == 本地预期+1 ?
    ├─ 是 ⇒ 视为已成功，应用 version，继续下一块
    └─ 否 ⇒ 视为未生效，有限次重试（标记风险）
```

## 5 权衡与替代方案

| 决策 | 选择 | 替代 | 放弃原因 |
|---|---|---|---|
| 上传粒度 | 默认单请求 | 总是分块 | 分块牺牲原子性，只为绕开 10k 限制 |
| 冲突检测 | 先预检再上传 | 只靠 409 | 预检省一次必然失败的往返；可关 |
| 重试策略 | 非幂等请求不自动重试 | 全量重试 | `upload` 可能已生效，重试会重复创建对象 |
| 令牌存储 | OS 凭据库 | SQLite | 明文令牌泄露不可接受 |
| 取消机制 | 自建 `CancelToken` | `tokio-util::CancellationToken` | 依赖清单无 `tokio-util`，不为一个布尔 + Notify 增依赖 |
| 并发下载 | 2 | 按核数放大 | 违背 OSM 使用策略，易被 429/封禁 |
| 缺失引用 | 显式 `MISSING` + 上传阻断 | 静默丢弃 Way | 静默丢弃制造"看不见的数据损坏" |

## 6 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| changeset 被服务端 24h 自动关闭 | 上传 409 | 空闲 30 分钟自动 close；409 时新建重传 |
| 分块中途崩溃 | 部分数据已入库 | `uploaded_until` 事务化，恢复后从下一块续传 |
| 代理/网关截断大请求体 | 400 或半截 XML | 请求体上限 8MiB，超出即分块；gzip 降低概率 |
| 令牌过期/scope 不足 | 403，用户困惑 | 上传前校验 scope，缺失项给可操作提示 |
| 服务端只读维护（`status=readonly`） | 写入全失败 | `capabilities` 缓存 5 分钟，`readonly` 时禁用上传入口 |
| 逆向补偿失败 | 数据半改 | 补偿结果单独记录并提示"需人工核对 changeset #id" |
| 版本判断用本地自增 | 误判冲突 | 版本只信服务端返回值 |

## 7 验收标准

- [ ] 对 [OSM 开发服务器](https://master.apis.dev.openstreetmap.org)（非生产库）完成：建 changeset → 新建节点/道路 → 上传 → 关闭 → 再下载比对。
- [ ] `upload` 请求体断言为 OsmChange XML（`application/xml`），**不含** multipart 边界。
- [ ] 构造 version 落后 1 的场景，得到 `ApiError::Conflict`，`per_object` 含该对象与 `remote_version`。
- [ ] 构造 Way 缺节点场景，本地即阻断上传并给出 412 的可读原因。
- [ ] 断网后队列不丢；恢复后按 `seq` 重放，`client_uuid` 去重生效。
- [ ] 50k 节点/0.25 平方度预检：超限请求在发出前被拒（不消耗一次 400）。
- [ ] 令牌不出现在任何日志/`tracing` 字段（`secrecy` 包装 + 集成测试）。
- [ ] 上传/下载吞吐与 429 触发率记录在 [14](./14-performance-and-verification.md)【待实测，见 docs/14-performance-and-verification.md】。
- [ ] `cargo test -p neo-api` + `clippy -- -D warnings` 通过。

## 8 参考

- API v0.6 全量端点、错误码、capabilities 字段（`area maximum`、`waynodes`、`changesets maximum_elements`、50'000 节点上限）：<https://wiki.openstreetmap.org/wiki/API_v0.6>
- OsmChange 格式（`create`/`modify`/`delete`、`if-unused`）：<https://wiki.openstreetmap.org/wiki/OsmChange>
- OAuth 2.0 与 scope：<https://wiki.openstreetmap.org/wiki/OAuth>（端点细节 `> 待验证`）
- 开发服务器（验收用）：<https://master.apis.dev.openstreetmap.org>
- `reqwest 0.13`：<https://docs.rs/reqwest/0.13.5/reqwest/> · `tokio 1.54`：<https://docs.rs/tokio/1.54.0/tokio/>
- 相关：[IO 层](./08-io-pbf-xml-osc.md) · [持久化与恢复](./10-persistence-and-recovery.md) · [数据模型与 Store](./03-osm-data-model-and-store.md)
