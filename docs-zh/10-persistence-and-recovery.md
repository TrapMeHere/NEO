# 10 · 持久化与崩溃恢复（Persistence & Recovery）

本篇定义 `neo-persist`：SQLite（WAL）承载工作区元数据、索引与缓存，rkyv + memmap2 承载大对象快照，"操作日志文件 + SQLite 索引"组成崩溃恢复链。最核心的边界：**OSM 对象数据不进 SQLite 的权威路径**——权威数据永远在 `neo-store` 的内存 SoA 里，SQLite 里没有"每帧查询地图"这种事。

## 1 目标与范围

| 编号 | 目标 | 判定 |
|---|---|---|
| P1 | 崩溃后编辑不丢，恢复到一致状态 | 对象数/版本/引用完整性校验通过 |
| P2 | 重开工程秒级可用 | 只重放 `version > autosave.version` 的事务 |
| P3 | 缓存与权威数据物理隔离 | 删缓存不影响数据正确性 |
| P4 | 上传队列跨重启存活 | `changeset_queue` 持久化（[09](./09-osm-api-and-sync.md) §2.7） |
| P5 | 数据库不膨胀 | WAL 有界、密封 journal 可 GC |

**范围外**：多进程并发写同一工程（单进程独占 + 文件锁）、云端同步、数据库加密（v1 不做，仅记录风险）。
**依赖**：`neo-persist → neo-core, neo-osm, neo-store`，**不依赖** `neo-editor`——日志帧以不透明字节（`&[u8]`）传入，帧格式由 [06 §2.7](./06-editing-core-and-undo.md) 定义，本 crate 只负责"追加 + 按 seq 找回 + fsync"，不理解 `EditOp` 语义。

## 2 设计

### 2.1 职责划分：权威 / 派生 / 缓存

| 存储 | 内容 | 权威性 | 失效后果 |
|---|---|---|---|
| `neo-store` 内存 SoA | Node/Way/Relation、索引、rstar | **唯一权威** | 进程崩溃即消失，靠 journal + autosave 重建 |
| journal 文件（`.neojournal`） | 事务帧（begin/op/commit） | 权威（操作级） | 丢最近编辑 |
| autosave 快照（`.neosnap`，rkyv） | 全量镜像 + 版本 | 派生（可重建） | 重放变长，不丢数据 |
| SQLite `*.neodb` | 工程/图层/索引/队列/设置 | 权威（元数据） | 元信息丢失，OSM 数据仍可由源文件重建 |
| 缓存表 + 磁盘缓存 | imagery offsets、presets、MapCSS、tile | **缓存** | 重新下载/编译 |

**反模式（禁止）**：把 OSM 对象存进 SQLite 再逐帧 `SELECT`。渲染需要几十万到百万对象的连续内存视图，行式读取 + `rusqlite` 逐行解码进不了帧预算；且每次编辑会被 WAL 放大成页写。

### 2.2 Schema 草案

```sql
PRAGMA journal_mode = WAL;          -- 崩溃安全 + 读写并发
PRAGMA synchronous  = NORMAL;       -- WAL 下已保证一致性；FULL 仅上传/危险操作前临时开
PRAGMA busy_timeout = 5000;         -- 后台 worker 与 UI 线程不互相 SQLITE_BUSY
PRAGMA foreign_keys = ON;
PRAGMA temp_store   = MEMORY;
PRAGMA wal_autocheckpoint = 512;    -- 页；约 2MiB(WAL 页 4KiB)，有界增长
PRAGMA mmap_size = 67108864;        -- 64MiB，只读缓存表受益

CREATE TABLE schema_version (version INTEGER NOT NULL PRIMARY KEY, applied_at INTEGER NOT NULL);
CREATE TABLE project (id INTEGER PRIMARY KEY, name TEXT NOT NULL, root_path TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL, last_opened_at INTEGER, settings_json TEXT NOT NULL DEFAULT '{}');
CREATE TABLE layer (id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE, name TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('osm','gpx','imagery','notes')),
  visible INTEGER NOT NULL DEFAULT 1, opacity REAL NOT NULL DEFAULT 1.0,
  z_order INTEGER NOT NULL DEFAULT 0, source_path TEXT,          -- 源文件，新建层为空
  snapshot_ver INTEGER NOT NULL DEFAULT 0);                      -- 与内存 Snapshot.version 对齐

CREATE TABLE journal_file (id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE, path TEXT NOT NULL,
  first_seq INTEGER NOT NULL, last_seq INTEGER NOT NULL, byte_len INTEGER NOT NULL,
  sealed INTEGER NOT NULL DEFAULT 0,                             -- 1 = 已被 Checkpoint 覆盖，可删
  created_at INTEGER NOT NULL);
-- 只做"帧定位索引"，帧本体在 .neojournal 里；不把 rkyv 帧塞进 BLOB
CREATE TABLE journal_index (seq INTEGER PRIMARY KEY,             -- 跨文件全局单调递增
  project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  journal_file_id INTEGER NOT NULL REFERENCES journal_file(id) ON DELETE CASCADE,
  byte_offset INTEGER NOT NULL, byte_len INTEGER NOT NULL, txn_id BLOB NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('begin','op','commit','checkpoint')),
  committed_at INTEGER NOT NULL);
CREATE INDEX idx_journal_txn ON journal_index(txn_id, seq);

CREATE TABLE autosave (id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  version INTEGER NOT NULL, path TEXT NOT NULL, byte_len INTEGER NOT NULL,
  crc32c INTEGER NOT NULL, txn_seq INTEGER NOT NULL,             -- 覆盖到的 journal seq
  created_at INTEGER NOT NULL, UNIQUE (project_id, version));
CREATE TABLE download_cache (bbox_key TEXT PRIMARY KEY,          -- 'bbox:…' 或 'tile:z/x/y'
  project_id INTEGER NOT NULL, fetched_at INTEGER NOT NULL, expires_at INTEGER, etag TEXT,
  object_count INTEGER NOT NULL, payload_path TEXT NOT NULL);    -- 压缩分块，读取时 mmap
CREATE TABLE changeset_queue (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL,
  client_uuid TEXT NOT NULL UNIQUE,                              -- 幂等键
  seq INTEGER NOT NULL, payload BLOB NOT NULL,                   -- 已序列化的 OSC 片段
  state TEXT NOT NULL CHECK (state IN ('queued','uploading','uploaded','conflicting','aborted')),
  changeset_id INTEGER, uploaded_until INTEGER,                  -- 分块续传游标
  created_at INTEGER NOT NULL);
CREATE INDEX idx_queue_pending ON changeset_queue(project_id, state, seq);

CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE kv_blob (key TEXT PRIMARY KEY, value BLOB NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE imagery_offset (id INTEGER PRIMARY KEY, imagery_id TEXT NOT NULL,
  region_key TEXT NOT NULL,                                      -- geohash-6 或 'global'
  dx REAL NOT NULL, dy REAL NOT NULL, source TEXT NOT NULL, updated_at INTEGER NOT NULL,
  UNIQUE (imagery_id, region_key));
CREATE TABLE preset_cache (id TEXT PRIMARY KEY, etag TEXT, fetched_at INTEGER, payload_path TEXT);
CREATE TABLE mapcss_cache (hash TEXT PRIMARY KEY, source_path TEXT, compiled_path TEXT, updated_at INTEGER);
```

### 2.3 Journal 的落盘位置与 fsync 策略

帧格式（magic/schema/flags/len/crc32c/seq + rkyv payload）由 [06 §2.7](./06-editing-core-and-undo.md) 定义，本文只定它落在哪里：

| 位置 | 优点 | 缺点 | 结论 |
|---|---|---|---|
| SQLite BLOB 表 | 单文件、事务天然 | 每帧一次页写 → WAL 膨胀；rkyv 对齐需拷贝；`VACUUM` 贵 | ❌ |
| 独立 append-only 文件 | 顺序写、零拷贝追加、可 mmap 扫描 | 需自建索引与 GC | ✅ |
| 纯内存 + 周期快照 | 最简单 | 崩溃丢一整个快照间隔 | ❌ |

**文件布局**：`<project_root>/journal/000001.neojournal`，单文件上限 64MiB，达上限即旋转（`journal_file` 登记 `first_seq/last_seq`）。`journal_index` 批量写（每 512 帧或 200ms），故可能落后于文件尾部——恢复时以**文件扫描为准**、索引为加速，不一致则以文件为准重建。

**fsync 三档**（与 06 的 `journal_sync` 对齐）：`Always`（每 `TxnCommit` 后 `fdatasync`，上传前/危险操作前强制）、`Interval(d)`（默认 1s，后台线程 group commit）、`Never`（只读导入/一次性转换）。group commit 实现：写线程把"已 append 未 sync 的最大 seq"发布到 `AtomicU64`，sync 线程取走并 `fdatasync` 后回写 `synced_seq`；`Always` 请求走同一把 `Mutex<File>`，避免与后台 sync 交错。

### 2.4 Autosave 节奏与后台 worker

- 触发：`dirty`（提交管线步骤 6）后去抖，默认 **120s**；或"事务数 > N""距上次 autosave 的 journal 字节 > M"（均配置化，默认值不构成性能承诺）。
- 动作：**后台线程**对当前快照 rkyv 归档 → 临时文件 → `fsync` → 原子 `rename` → 写 `autosave` 行 → 追加一条 `Checkpoint` 帧。
- 保留：每工程最近 **3** 代（`autosave.keep`）；更旧的连同其覆盖的 journal（`sealed=1` 且 `last_seq < 最旧 autosave.txn_seq`）一起 GC。
- 崩溃检测：`<project_root>/.lock` 写 `pid` + 进程启动时间戳；启动时 pid 不存在（或启动时间不符）即判定"上次未正常退出"，直接进恢复流程。与 JOSM 的 `AutosaveTask`（pid 文件 + 保留份数）同源。
- **不阻塞编辑**：取一份 `Arc<Snapshot>`（只读不可变）交后台编码，编辑线程继续产出新快照——这是"快照只读 + 增量更新"的直接红利。

### 2.5 崩溃恢复时序

```text
启动
 ├─ 打开 project/*.neodb ─▶ schema_version：低于当前逐版本 migrate；高于则拒绝并提示
 ├─ 读 .lock ─▶ 有未清理 pid 且进程不存在 ⇒ unclean = true
 ├─ 选 autosave：ORDER BY version DESC LIMIT 3
 │    ├─ 全部 crc32c 失败 ⇒ 回退 source_path 重新导入，进只读恢复
 │    └─ 取最新可用：rkyv access → Snapshot(version = V_a)
 ├─ journal 扫描（从 autosave.txn_seq 之后）
 │    ├─ magic/len 不符 ⇒ 尾部撕裂，truncate 到最后一个完整帧，记 TruncatedTail
 │    ├─ crc32c 不符 ⇒ 同上
 │    └─ schema 不符 ⇒ 拒绝 replay，仅保留 autosave，进只读恢复模式
 ├─ 事务级重放：只重放"有 begin 且有 commit"的 txn（seq 升序），每 commit version += 1
 ├─ Editor::install_snapshot(RecoveredState)
 └─ 一致性检查（§2.6）──▶ RecoveryReport
```

### 2.6 一致性检查与修复

| 检查 | 手段 | 自动修复 |
|---|---|---|
| journal 尾部撕裂 | 帧扫描 + crc32c | ✅ 截断到最后完整帧（原件留 `.corrupt` 副本） |
| index 与文件不一致 | 比对最后索引的 `offset+len` 与文件长度 | ✅ 以文件为准重建索引尾部 |
| autosave 损坏 | crc32c + 总长 | ✅ 降级到上一代 |
| 引用完整性 | Way→Node、Relation→member 解析（复用 `RefIntegrity`） | ⚠️ 只报告，不自动删（可能是 partial data） |
| 孤儿文件 | `autosave/`、`journal/` 中未被表引用者 | ✅ 移入 `trash/`，不直接删 |
| SQLite 页损坏 | 启动 `PRAGMA quick_check`，菜单 `integrity_check` | ⚠️ 报告 + 建议导出；仅索引损坏才 `REINDEX` |
| 队列状态残留 | `state='uploading'` 但 `uploaded_until` 未更新 | ✅ 回退为 `queued`（宁可重传，不静默丢） |

`RecoveryReport` 必须在 UI 完整呈现：`unclean_shutdown`、`autosave_version`、`replayed_txns`、`truncated_tail_bytes`、`read_only_fallback`、`repairs`。

### 2.7 性能预算与 WAL 膨胀控制

| 手段 | 做法 | 目的 |
|---|---|---|
| 批量事务 | 索引/队列写入合并为每 512 行或 200ms 一事务 | 减少 fsync 与 WAL 帧 |
| prepared statement | `prepare_cached(sql)` | 免每次解析 SQL |
| checkpoint | `wal_autocheckpoint=512` + 关闭时 `wal_checkpoint(TRUNCATE)` | WAL 有界 |
| 短读事务 | 缓存读取不跨帧持有 `Transaction` | 长读事务阻止 checkpoint ⇒ WAL 无界（最常见事故） |
| `synchronous=FULL` 限用 | 仅上传前/危险操作前临时开 | 正常路径不为每次 commit 付两次 fsync |
| 归档不落库 | autosave 写独立文件而非 BLOB | 避免大对象撑爆 DB 与 WAL |
| 无逐帧查询 | UI/渲染只读内存快照 | 见 §2.1 反模式 |

`.neodb` 体积与工程数据量**弱相关**（只随历史条数与缓存增长），GC 后回落；具体数值【待实测，见 docs/14-performance-and-verification.md】。

## 3 关键接口

```rust
// neo-persist/src/lib.rs
pub struct DbOptions { pub busy_timeout: Duration, pub wal_autocheckpoint_pages: u32,
                       pub read_only: bool, pub mmap_size: u64 }
pub struct PersistDb { /* rusqlite::Connection + prepared stmt cache */ }
impl PersistDb {
    pub fn open(path: &Path, opts: DbOptions) -> Result<Self, PersistError>;
    pub fn migrate(&mut self) -> Result<MigrationReport, PersistError>;
    pub fn quick_check(&self) -> Result<IntegrityReport, PersistError>;
    pub fn project_upsert(&mut self, row: &ProjectRow) -> Result<i64, PersistError>;
    pub fn imagery_offset(&self, imagery: &str, region: &str) -> Result<Option<Offset>, PersistError>;
    pub fn set_imagery_offset(&mut self, o: &Offset) -> Result<(), PersistError>;
    pub fn queue_push(&mut self, item: &QueueItem) -> Result<i64, PersistError>;
    pub fn queue_claim_next(&mut self, project: i64) -> Result<Option<QueueItem>, PersistError>;
    pub fn queue_mark(&mut self, id: i64, st: QueueState, until: Option<i64>) -> Result<(), PersistError>;
}

/// 不透明帧：本 crate 不理解 EditOp，只按字节追加与找回。
pub struct FrameMeta { pub seq_hint: u64, pub txn_id: [u8; 16], pub kind: FrameKind }
pub enum FrameKind { Begin, Op, Commit, Checkpoint }
pub enum SyncPolicy { Always, Interval(Duration), Never }

pub struct JournalStore { /* File + index writer + sync 线程句柄 */ }
impl JournalStore {
    pub fn open(project: i64, root: &Path, index: Arc<Mutex<PersistDb>>) -> Result<Self, PersistError>;
    pub fn append(&mut self, frame: &[u8], meta: FrameMeta) -> Result<u64, PersistError>; // → seq
    pub fn sync(&mut self) -> std::io::Result<()>;                                        // fdatasync
    pub fn set_sync_policy(&mut self, p: SyncPolicy);
    /// 从 from_seq 扫描；visitor 返回 Break 即停止。
    pub fn scan(&mut self, from_seq: u64,
                visitor: &mut dyn FnMut(FrameRef<'_>) -> std::ops::ControlFlow<()>)
                -> Result<ScanReport, PersistError>;
    pub fn truncate_tail(&mut self, good_len: u64) -> Result<(), PersistError>;
    pub fn rotate_if_needed(&mut self, max_bytes: u64) -> Result<bool, PersistError>;
    pub fn gc(&mut self, keep_from_seq: u64) -> Result<GcReport, PersistError>;
}

pub struct AutosaveWorker { /* 后台线程 + Arc<Snapshot> 取样式 */ }
impl AutosaveWorker {
    pub fn spawn(cfg: AutosaveConfig, journal: Arc<Mutex<JournalStore>>) -> Self;
    pub fn mark_dirty(&self, snapshot_version: u64);
    pub fn force(&self) -> Result<PathBuf, PersistError>;   // 上传前调用，同步等待
    pub fn shutdown(self) -> Result<(), PersistError>;
}

pub struct RecoveryReport { pub unclean_shutdown: bool, pub autosave_version: Option<u64>,
    pub replayed_txns: u64, pub truncated_tail_bytes: u64, pub read_only_fallback: bool,
    pub repairs: Vec<RepairAction>, pub integrity: IntegrityReport }
impl RecoveryReport { pub fn is_usable(&self) -> bool }   // false ⇒ 只能导出/只读

pub struct SnapshotArchive;   // rkyv + memmap2
impl SnapshotArchive {
    pub fn write_atomic(path: &Path, snap: &Snapshot) -> Result<ArchiveMeta, PersistError>;
    pub fn open(path: &Path) -> Result<MappedSnapshot, PersistError>;  // mmap + access
}
pub struct ArchiveMeta { pub version: u64, pub byte_len: u64, pub crc32c: u32, pub schema: u16 }

#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    #[error("sqlite: {0}")] Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("schema too new: db={db}, supported={supported}")] SchemaTooNew { db: u32, supported: u32 },
    #[error("corrupt journal frame at seq {seq}")] CorruptFrame { seq: u64 },
    #[error("archive crc mismatch: {0}")] ArchiveCorrupt(PathBuf),
    #[error("project locked by pid {0}")] Locked(u32),
}
```

## 4 数据流与状态机

```text
EditTx commit
 ├─(06 步骤2) journal.append(Begin) / append(Op)*
 │     └─ JournalStore 顺序写 .neojournal，seq = ++global；索引行先入 buffer（512 行/200ms 刷）
 ├─(06 步骤5) journal.append(Commit) + sync(按策略)
 ├─(06 步骤6) dirty.mark() ─▶ AutosaveWorker.mark_dirty(version)
 └─ 快照发布 ─▶ 渲染/UI 立即可见（不等磁盘）

AutosaveWorker（后台，默认 120s 去抖）
 ├─ 取 Arc<Snapshot>（只读，不阻塞编辑）
 ├─ rkyv to_bytes → 临时文件 → fsync → rename → autosave 行
 └─ journal.append(Checkpoint{snapshot_version, path}) → GC(keep_from_seq)

WAL 事故模型：
长读事务 A ──────────────(跨帧存活)──────────────▶ 结束
   WAL: [帧][帧][帧]…[帧]  ← A 未结束 ⇒ checkpoint 无法回收 ⇒ WAL 无界 ⇒ 磁盘写满
正确做法：缓存读取用 μs 级短事务；跨帧/跨渲染一律读内存快照。
```

## 5 权衡与替代方案

| 决策 | 选择 | 替代 | 放弃原因 |
|---|---|---|---|
| 元数据库 | SQLite（rusqlite, WAL, bundled） | sled / redb | 需显式 SQL、成熟工具链、`bundled` 免系统依赖 |
| 日志落点 | 独立 append-only 文件 | SQLite BLOB | 每帧页写放大 + WAL 膨胀 + rkyv 对齐拷贝 |
| 大对象编码 | rkyv 0.8.18 | JSON / bincode | 零拷贝 + 体积小；bincode 无 schema 校验 |
| 大对象访问 | memmap2 只读映射 | 全量读入 `Vec` | 国省底图数百 MB，与编辑数据抢内存 |
| 崩溃检测 | pid + 启动时间戳 lock | 仅靠 WAL | WAL 只保证 DB 一致，无法告知"上次是否正常退出" |
| 自动保存 | 全量快照 + 操作日志双层 | 只有日志 / 只有快照 | 长会话重放不可控；只有快照则崩溃丢整个间隔 |
| 缓存与权威同库 | 同库不同表 + 不同文件 | 拆两个 DB | 同库便于事务/备份，靠"可删缓存表"划边界即可 |

## 6 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| 长读事务阻塞 checkpoint | WAL 无界、磁盘满 | 禁跨帧事务；`wal_autocheckpoint` + 关闭时 `TRUNCATE`；启动检查 WAL 大小 |
| `synchronous=NORMAL` 断电丢末尾事务 | 丢最近编辑（不损坏） | 上传/危险操作前 `force()` + `synchronous=FULL` |
| rkyv schema 演进 | 旧 autosave 读不了 | header `schema: u16` 显式拒读并降级，不承诺跨版本 |
| mmap 文件被外部改/截断 | `SIGBUS` | 打开校验 magic/len/crc；Windows 上删除被映射文件会失败，需先卸载 |
| GC 与 replay 竞争 | 删掉未重放帧 | GC 只在"最旧 autosave 覆盖点"之前、持写锁执行 |
| 多实例打开同一工程 | 互相覆盖 | `.lock` 强互斥 + `Locked`/`SchemaTooNew` 错误码 |
| 磁盘写满 | 归档/日志失败 | 写前剩余空间检查；失败即停用 autosave 并告警，绝不静默 |

## 7 验收标准

- [ ] 在 commit 前后各 100 次注入崩溃（`TerminateProcess`/`kill -9`），恢复后事务要么全含要么全不含，无中间态。
- [ ] 手工截断 `.neojournal` 末尾 N ∈ {1, 8, 100} 字节，恢复成功且 `truncated_tail_bytes > 0`。
- [ ] 损坏最新 autosave 的 crc，恢复到上一代，`repairs` 含对应项。
- [ ] 100 次随机崩溃后 `PRAGMA quick_check` 恒为 `ok`；删除全部缓存后工程仍可正常打开编辑。
- [ ] 1000 次提交后 WAL 大小有界（不随提交次数单调增长），阈值【待实测，见 docs/14-performance-and-verification.md】。
- [ ] 上传前 `force()` 后立刻断电，恢复后队列与 journal 一致（`uploading` 回退为 `queued`）。
- [ ] `cargo test -p neo-persist` 覆盖 migrate / replay / truncate / gc / lock；`clippy -D warnings` 通过。

## 8 参考

- `rusqlite 0.42`（`pragma_update`、`prepare_cached`、`transaction_with_behavior`、`busy_timeout`）：<https://docs.rs/rusqlite/0.42.0/rusqlite/>
- SQLite WAL 与 `wal_autocheckpoint`/`wal_checkpoint`：<https://www.sqlite.org/wal.html>
- `PRAGMA synchronous` 在 WAL 下的一致性保证：<https://www.sqlite.org/pragma.html#pragma_synchronous>
- `rkyv 0.8.18`（`to_bytes`/`access`/`AlignedVec`）：<https://docs.rs/rkyv/0.8.18/rkyv/>
- `memmap2 0.9.12`：<https://docs.rs/memmap2/0.9.12/memmap2/>
- Journal 帧格式 / replay 算法 / `journal_sync` 三档：[06 §2.7](./06-editing-core-and-undo.md)
- 相关：[IO 层](./08-io-pbf-xml-osc.md) · [API 与同步](./09-osm-api-and-sync.md) · [数据模型与 Store](./03-osm-data-model-and-store.md)
- > 待验证：JOSM `AutosaveTask` 的默认参数（`autosave.interval`=5min、`autosave.filesPerLayer`=1、pid 文件）取自本地源码快照，未取得可引用的永久 URL。
