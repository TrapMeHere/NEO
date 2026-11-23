# ADR-0006 · 单写者编辑模型（Single-Writer Editor）

## 状态

- 已接受：2026-11-15。
- 可逆性：**中**。改动会波及 `neo-editor` 的全部公开签名与 `neo-ui` 的数据访问方式。
- 影响面：`Editor` 的非 `Sync` 约束、`SnapshotPublisher`、后台任务的版本校验、导入安装路径。
- 相关文档：[07](../07-concurrency-and-tasks.md) §2.2、[06](../06-editing-core-and-undo.md) §2.3、[ADR-0008](./0008-command-transaction-undo.md)。

## 背景

10M 对象的数据集上，任何"多线程同时改同一份数据"的方案都要回答：锁粒度、死锁顺序、撤销的全局顺序、日志的可重放顺序、渲染/校验读到哪个版本。

细粒度锁（每对象一把、分片锁）在对象数量级上开销不可控，且一旦出现"A 读 B 写"就会产生难以复现的不一致。而编辑器天然是"用户一次做一个动作"的交互模型：**写入本身是串行的，可并行的是读**（几何构建、校验、场景构建）。

## 决策

1. **UI 线程是唯一写者**：`DataSet` 的 `&mut` 只由 `Editor` 在 UI 线程持有；`Editor` 非 `Sync`、`DataSetMut` 非 `Send`，由编译器阻止跨线程。
2. **一切写入走 `EditTx`**：事务提交是唯一发布点，提交顺序即历史顺序、日志顺序、版本顺序。
3. **读路径无锁**：发布 `Arc<Snapshot>` 到 `arc-swap::ArcSwap`；后台用 `load_full()` 跨线程持有，UI 单帧用 `load()`（`Guard` 不可跨帧）。
4. **后台结果带版本校验**：`TaskOutcome::Ready { base_version, .. }` 与当前版本不符即 `Stale` 丢弃（或显式 rebase），绝不"补写"快照。
5. **两个结构性例外**：PBF/XML 导入与 autosave 恢复在 worker 上构建全新 `DataSet`，完成后 `Snapshot::publish` 一次性安装。

```
写者（UI 线程）                          读者（Rayon / Tokio / 渲染）
  Editor ──EditTx──► DataSet
     │                   │
     └── publish ────────┴──► ArcSwap<Snapshot> ──► load()/load_full()
                                  │
                                  └── 后台结果必须携带 base_version + Handle.gen
```

| 例外路径 | 写入者 | 发布方式 | 进 History |
|---|---|---|---|
| 用户编辑 | UI 线程 `EditTx` | 增量 + `version+1` | 是 |
| PBF/XML 导入 | worker（独占新 DataSet） | `Snapshot::publish` 原子替换 | 否（新会话） |
| autosave 恢复 | worker（独占新 DataSet） | `Snapshot::publish` 原子替换 | 否（历史另从 journal 重建） |

## 替代方案

| 方案 | 优点 | 否决理由 |
|---|---|---|
| 细粒度锁（每对象/分片 `RwLock`） | 理论并行度高 | 10M 对象的锁元数据与缓存行争用不可接受；死锁顺序难维护；撤销/日志丧失全局顺序 |
| 每图层一个 actor（消息传递串行化） | 逻辑清晰、易调试 | 跨图层操作（节点 + 所属 way）需两阶段提交与回滚协议，复杂度高于单写者 |
| 乐观并行编辑 + 冲突合并（MVCC） | 后台可并行写 | OSM 编辑是"用户意图"而非独立事务；本地合并语义无处安放，且与远端 409 冲突叠加后不可解释 |
| 直接用数据库（SQLite）作真相 | 事务与崩溃恢复现成 | 10M 对象下每次拾取/渲染查询都要过 SQL；内存 SoA 才是渲染与几何的可用形态 |
| 单写者但放在独立线程（命令队列） | UI 不被编辑阻塞 | 每次编辑多一次往返 + 上下文切换；undo 的即时反馈变差；仍需一条串行通道 |

## 后果

**正面**

- 提交顺序 = 历史顺序 = 日志顺序 ⇒ undo / journal / OSC 三者天然一致。
- 只有一把逻辑写锁，死锁类别消失；渲染与校验完全并行。
- 崩溃恢复只需重放 op log，不需要"多版本合并"。

**负面**

- **长事务阻塞输入**：批量粘贴/大多边形必须切分（`EditPolicy::max_ops_per_txn`，见 [06](../06-editing-core-and-undo.md) §2.8）。
- **UI 线程三职合一**：输入 + 命令执行 + GPU 提交共享帧预算（见 [07](../07-concurrency-and-tasks.md) §2.1），超预算只能降 LOD。
- **无法并行做语义修改**："给 10 万个 way 加标签"必须分事务串行，只能用几何/索引并行摊薄。
- **后台不能顺手修数据**：所有修复都要变成新事务，代价是额外版本与历史条目。

## 验证

1. 编译期：`Editor` 不实现 `Sync`、`DataSetMut` 不实现 `Send`（`static_assertions::assert_not_impl_any!`）。
2. 版本单调：任意操作序列后 `Snapshot.version` 严格递增；undo/redo 同样递增（INV-7）。
3. `Stale` 路径：构造慢任务 + 中途编辑，断言结果被丢弃且数据不变（`tests/stale_task.rs`）。
4. 发布点唯一：全仓 `Snapshot::publish` 调用点仅 `neo-store` 导出与两条安装路径。
5. 死锁清单 D1–D10 每条有测试或静态检查（锁顺序、持锁投递、渲染等待编辑等），见 [07](../07-concurrency-and-tasks.md) §2.5。
6. 压力测试：60 秒连续编辑 + 并行场景构建 + 并行校验，无死锁、无 panic、无版本回退【通过判据见 docs/14-performance-and-verification.md】。

## 参考

- `arc-swap`（`load`/`load_full`/`Guard` 语义）：https://docs.rs/arc-swap/1.9.5/arc_swap/
- Rust 的 `Send`/`Sync` 语义：https://doc.rust-lang.org/nomicon/send-and-sync.html
- `static_assertions`（编译期断言）：https://docs.rs/static_assertions/latest/static_assertions/
