# 07 · 并发与任务（Concurrency & Tasking）

NEO 的并发模型只有一条主线：**UI 线程是唯一写者，其余全部是只读快照的消费者**。快照通过 `arc-swap` 原子切换，读路径无锁；后台任务通过可取消、带进度与背压的任务系统与前台交互；GPU 资源由 `neo-render` 独占创建与上传，UI 线程永不在 `map_async` / `poll(wait)` 上阻塞。整体视图见 [系统架构](./01-system-architecture.md) §2.4，写入语义见 [编辑核心与撤销](./06-editing-core-and-undo.md)，GPU 细节见 [渲染器](./05-renderer-wgpu.md)。

## 摘要

编辑器天然是"用户一次做一个动作"的模型：**写入必须串行，可并行的只有读**（几何构建、校验、场景组装、解码）。因此 NEO 不做细粒度锁、不做多版本合并，而是把并发收敛成"一个写者 + 若干只读快照消费者"，用类型系统把规则钉死，再用一张死锁清单把剩余风险显式化。

## 1 目标与范围

**在范围内**：线程 / 执行体拓扑与 CPU 预算；单写者与快照发布协议；任务系统（取消 / 进度 / 优先级 / 帧预算 / 背压 / 通道选型）；GPU 资源所有权与增量上传；死锁与竞态清单。

**不在范围内**：命令的事务语义（[06](./06-editing-core-and-undo.md)）、PBF 并行解码的分块策略（[08](./08-io-pbf-xml-osc.md)）、管线与着色器（[05](./05-renderer-wgpu.md)）、实测帧时间与吞吐（【待实测，见 docs/14-performance-and-verification.md】）。

## 2 设计

### 2.1 线程拓扑

参考机为 i5-12400F（6 物理核 / 12 逻辑核）、31.8 GB RAM、RTX 3070 8 GB。分配原则：**UI 与提交永不让核**，计算池吃满剩余逻辑核，网络与存储各留专用线程，避免与 Rayon 抢核。

| 执行体 | 数量 | 触碰什么 | 允许 | 绝对禁止 |
|---|---|---|---|---|
| Main/UI（winit 0.30.13 事件循环） | 1 | `&mut DataSet`、egui 状态、GPU 提交 | 输入、命令执行、场景差量、`prepare`/`encode`、`submit` | 网络/磁盘阻塞、`poll(wait)`、跨帧持 `Guard` |
| Rayon 计算池 | 固定 8 | `Arc<MapSnapshot>`、只读几何缓存 | 几何/样式/校验/解码的纯计算，可跨帧续算 | 触碰 `DataSet`；向 UI 无界投递 |
| wgpu Queue | 单队列（驱动内部线程） | `Device`/`Queue` | 写 buffer、提交 encoder、present | 第二个 `Instance`；`paint` 内新建 pass 或 `submit` |
| Tokio runtime | 2 worker + ≤4 blocking | HTTP body、`Arc<MapSnapshot>` | 下载/上传/重试/进度 | 直接写数据集、长 CPU 计算 |
| Storage worker | 1 | 序列化批次、SQLite（WAL） | journal 追加、autosave、恢复扫描 | 回调 UI、持锁调 editor |
| WASM 插件（可选） | 每插件 1 | 只读 `MapQuery` 视图 + 宿主调用 | 产出 `EditCommand`、查询 | 直接 IO/网络/发布快照（除显式授权） |

```
+---------------------- Main/UI（唯一写者）----------------------+
| egui::Context::run -> NeoApp::ui -> intents -> Editor::execute  |
| MapRenderer::prepare（增量上传）-> encode -> queue.submit       |
+--+-------------+--------------+--------------+-----------------+
   | submit      | publish      | adopt frame  | readback poll
   v             v              v              v
+---------+ +----------+ +-----------+ +----------------------+
| Rayon 8 | | arc-swap | | SceneFrame| | wgpu Queue           |
| 几何/校验| | 无锁读    | | 双缓冲    | | 2 帧在飞（可配 3）    |
+---------+ +----------+ +-----------+ +----------------------+
+---------+ +----------+
| Tokio 2 | | Storage  |  <- 两条独立通道，不并入 Rayon 池
+---------+ +----------+
```

**为什么不设独立渲染线程**：egui 的 `PaintCallback` 在 UI 线程的 paint 阶段执行（`egui-wgpu` 的 `CallbackTrait`），拾取与 hover 必须与 UI 同帧；拆线程会引入至少 1 帧输入延迟，并让 `Device` 所有权跨线程转移。若未来出现"场景构建拖慢提交"的实测证据，再立 ADR 评估三缓冲渲染线程；**当前的可拆点收敛在 `MapRenderer::prepare`（encoder 内的工作、返回 `CommandBuffer`）与 `encode`（纯编码）两处**，它们都不持有数据集引用。UI 线程实际是**三职合一**（输入 + 命令 + 提交），这正是单写者模型的代价（[ADR-0006](./adr/0006-single-writer-editor.md)），也是 §2.6 必须给预算的原因。

### 2.2 单写者与快照发布

```
      UI 线程（写者）                          任意读者线程
      ----------------                         ----------------
Editor::execute(cmd)
      |  EditTx（影子层，DataSet 未变）
      |  commit：journal -> 索引/脏位图 -> version+1
      v
SnapshotCell::store(MapSnapshot)  ──────►  ArcSwap::store(Arc<MapSnapshot>)
      |                                            |
      |                                            +-- load()      -> Guard（单帧有效，!Send）
      |                                            '-- load_full() -> Arc<MapSnapshot>（可跨线程持有）
      v
notify(touched)                          消费：scene 组装、validate、autosave、插件查询
```

| 规则 | 内容 | 强制 / 检验 |
|---|---|---|
| R1 | 只有 `Editor` 持有 `&mut DataSet`；`Editor` 非 `Sync` | 类型系统 |
| R2 | 发布单点：只有 `SnapshotCell::store` 写 `ArcSwap` | 评审 + grep（唯一调用点） |
| R3 | `Guard` 线程局部，不得跨帧存进结构体 | `Guard: !Send` + 评审 |
| R4 | 后台结果必须带 `base_version`；不匹配即 `Stale` 丢弃（或显式 rebase） | `TaskOutcome::Stale` 分支必测 |
| R5 | 读者不得"修复"快照：只读 trait 上没有 `&mut` 方法 | `MapQuery`/`SnapshotView` 无 `&mut self` |
| R6 | 旧快照存活期由引用计数决定；后台持 `Arc` 会把旧版本钉在内存里 | 取消即 drop；监控 `Arc::strong_count` |

**当前实现的关键限制（必须承认）**：`MapSnapshot` 目前是 `Arc<DataSet>`，发布新版本需要深拷贝，成本 O(n)。所以第一版只在**导入完成、批量合并、恢复**这类低频节点发布快照；交互编辑期间的渲染走单线程增量路径（脏集合 + 局部重建）。目标态是 COW / 分块快照（未修改的块共享），落地后交互路径也能"每事务发布一次"。选 `arc-swap` 而不是 `RwLock<Arc<T>>`：读侧无原子 RMW、无写者饥饿、`load()` 不阻塞写者；代价是每次 `store` 一次新 `Arc` 分配与 `Guard` 的线程局部约束。

### 2.3 任务系统

| 概念 | 类型 | 语义 |
|---|---|---|
| 任务种类 | `TaskKind::{Import, Scene, Validate, Upload, Download, Autosave, Plugin}` | 决定队列、并发上限、默认优先级 |
| 优先级 | `Priority::{Interactive, Normal, Background}` | Interactive 每帧最多一个时间片；Background 仅在 UI 空闲时 poll |
| 取消 | `CancelToken`（`Arc<AtomicBool>`） | 协作式：每 N 个元素或阶段边界检查；`Drop` 即取消 |
| 进度 | `Progress { done, total, phase }`（原子） | 写端无锁，读端按 10 Hz 采样 |
| 句柄 | `TaskHandle { id, kind, priority, token, progress }` | UI 持有，用于显示与取消 |
| 结果 | `TaskOutcome::{Ready, Stale, Cancelled, Failed}` | `Stale` 由版本校验失败产生 |

| 队列 | 容量 | 满载策略 | 理由 |
|---|---|---|---|
| `scene` | 1（覆盖式） | 新请求替换旧请求（只留最新 view + version） | 场景永远只关心最新状态 |
| `import` | 1 | 拒绝第二个 | 单写者安装，天然互斥 |
| `validate` | 1 | 合并（同版本同区域复用） | 结果按 `(version, bbox)` 缓存 |
| `upload` | 1 | 拒绝 | changeset 顺序敏感 |
| `download` | 8 | 排队 | 分块下载可并发 |
| `autosave` / `plugin` | 1 / 每插件 1 | 合并 / 拒绝 | 幂等；防单插件饿死全局 |

**通道选型**：UI ↔ worker 一律 `crossbeam-channel 0.5`（bounded）——纯同步、`select!`、延迟稳定，且不把 tokio 依赖泄漏进 `neo-ui`；需要 `send_timeout` / `drain` 语义的少数边（如关闭时的批量冲刷）用 `flume 0.12`；一次性结果回传复用同一条提交通道（少一条通道 = 少一处顺序问题）。不用 `std::sync::mpsc`（无 `try_send` 超时、无 MPMC），也不用 `tokio::sync::mpsc` 做 UI 通道（UI 非 async）。

**背压**：所有通道 bounded；`try_send` 失败即降级（进度丢弃、场景请求覆盖、导入拒绝），**不允许在持锁状态下阻塞 send**——这同时是死锁防线（§2.5 D4）。**帧预算**由结构体承载（字段为设计目标，数值待实测）：

```rust
pub struct FrameBudget {
    pub frame: Duration,        // 目标帧时间（由刷新率推导）
    pub ui: Duration,           // egui 逻辑
    pub scene_adopt: Duration,  // 采用已就绪的 SceneFrame
    pub upload: Duration,       // buffer 上传
    pub task_poll: Duration,    // 每帧 poll 后台任务的总时间片
}
```

超预算的降级顺序：① 场景降 LOD；② 推迟非 Interactive 任务；③ 复用上一帧 `SceneFrame`（画面滞后但不掉帧）。

### 2.4 GPU 资源所有权

| 资源 | 创建者 | 更新 | 销毁者 | 生命周期 |
|---|---|---|---|---|
| `Device` / `Queue` / `Instance` | `eframe`（`egui_wgpu::RenderState`） | — | `eframe` | 进程级 |
| `RenderPipeline` / `BindGroupLayout` / `Sampler` | `MapRenderer::new`（启动一次） | — | `MapRenderer::drop` | 会话级 |
| 顶点 / 索引 buffer | `BufferPool` | `prepare` 内 `write_buffer` 或 staging + copy | pool 回收 / 淘汰 | 帧间复用 |
| uniform buffer | 启动时 ring（2–3 槽） | 每帧 `write_buffer` 到当前槽 | 进程级 | 与 frame-in-flight 对齐 |
| 文字图集 | 首次需要时分配 | 逐字形 `write_texture` | 重建时整体替换 | 增长式 |
| 拾取纹理 + staging | `MapRenderer`（按需） | GPU 渲染 | 会话级 | 2 个 staging 轮换 |

**所有权不变量**：

1. `neo-render` 不拥有 `Device`，只借 `&Device`/`&Queue`；不能绕过 eframe 创建第二个 `Instance`（[ADR-0005](./adr/0005-wgpu-egui-shared-device.md)）。
2. buffer 的创建 / 写 / 回收都发生在 UI 线程的 `prepare`/`collect`；后台任务只产出 `SceneFrame`（CPU 数据），**永不触碰 GPU API**。
3. `FRAMES_IN_FLIGHT = 2`：每槽独立持有 vertex/index/instance/uniform buffer，`frame_no + in_flight < current` 才允许回收。
4. 增量上传：`≤ 64 KiB` 走 `write_buffer`，更大走 staging + `copy_buffer_to_buffer`（大块 `write_buffer` 会隐式分配暂存并可能阻塞）；稳态每帧只写 32 B camera uniform。
5. **UI 线程绝不阻塞在 `map_async`**：拾取是 `request()`（当帧编码）→ 下一帧 `try_take()`（`PollType::Poll`）。`poll(wait_indefinitely())` 只允许出现在导出 / 测试路径，它等于一次全流水线 flush。
6. 持有 `BufferView` 期间不得 `unmap`/`destroy`/`poll`；`get_mapped_range` 返回 `Result`，出错按"本帧无拾取结果"处理；回读行 pitch 必须是 256 B 的倍数。

### 2.5 死锁与竞态清单

| ID | 风险 | 典型场景 | 规避 |
|---|---|---|---|
| D1 | 锁顺序反转 | 编辑线程先拿 `geo` 再拿 `store`，渲染路径相反 | 全局顺序 `store → geo → scene`；跨模块只允许"向外一把" |
| D2 | 持锁投递 UI | 在 `parking_lot::Mutex` 保护下 `send` 到 UI 队列，而 UI 正等该锁 | 先取数据、释放 guard、再 send |
| D3 | 渲染等待编辑 | `prepare` 取快照时写者正持 store 锁并等 GPU | 写者永不等待 GPU（只 `submit`）；渲染不碰 `DataSet` |
| D4 | 通道满导致互等 | UI 等 worker 结果，worker 阻塞在满的进度通道 | 进度用覆盖式 / `try_send`；阻塞 send 只用于一次性结果 |
| D5 | 持 `BufferView` 时 poll | 回读代码里 `poll` 与 `unmap` 交错 | 作用域化：`drop(view); buffer.unmap();`，由 `ReadbackGuard` 强制 |
| D6 | 异步里做阻塞 | Tokio worker 里 `std::fs` 写大文件或 `fsync` | 磁盘走 Storage worker 或 `spawn_blocking`；大文件分块 |
| D7 | `Guard` 跨 await / 跨帧 | `let g = cell.load();` 后 `await` 或存字段 | `Guard: !Send` 编译期挡住；跨帧只存 `Arc`（`load_full`） |
| D8 | 旧快照被钉住 | 后台持 `Arc<MapSnapshot>` 但取消后未 drop | 任务 RAII 持有，取消即 drop；`strong_count` 超阈值告警 |
| D9 | 双重发布 | 导入与编辑两条路径都调 `SnapshotCell::store` | 发布点唯一（R2）；安装前检查无未提交事务 |
| D10 | 忙等 / 同步回读 | 手写 `loop { if ready { break } }`，或 hover 路径 `wait_indefinitely` | 禁用忙等：UI 只 `Poll`；hover 用 CPU 拾取，GPU 拾取只用于点击 |

### 2.6 优先级与 CPU 预算建议

| 线程 | 建议优先级 | 实现 |
|---|---|---|
| UI | normal（不降） | 默认 |
| Rayon | below-normal | `ThreadPoolBuilder::spawn_handler` 自定义 |
| Tokio worker | normal | `runtime::Builder::worker_threads(2)` |
| Storage | below-normal | `std::thread::Builder` + 平台封装 |
| 渲染提交 | 跟随 UI（normal） | 无独立线程 |

预算按逻辑核 12 分配：UI 1 + Rayon 8 + Tokio 2 + Storage 1 = 12。同时有导入与校验时，Rayon 池内部按 `Priority` 排队，而不是再开线程；固定线程数是防止 Rayon 与 Tokio 抢核的关键（默认 `num_cpus` 会算出 12 而挤掉 UI 与网络）。这些是**配置上限而非实测吞吐**。

## 3 关键接口

```rust
// crates/neo-core/src/task.rs —— 任务描述与结果（无 IO、无 GPU 依赖）
pub enum Priority { Interactive, Normal, Background }
pub enum TaskKind { Import, Scene, Validate, Upload, Download, Autosave, Plugin }

pub struct CancelToken(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl CancelToken {
    pub fn cancel(&self);
    pub fn check(&self) -> Result<(), Cancelled>;   // 协作检查点：返回 Err 让任务提前退出
}

pub struct Progress { /* done: AtomicU64, total: AtomicU64, phase: AtomicU8 */ }
impl Progress {
    pub fn inc(&self, n: u64);
    pub fn snapshot(&self) -> ProgressSnapshot;      // (done, total, phase)
}

pub struct TaskHandle { pub id: TaskId, pub kind: TaskKind, pub priority: Priority,
                        pub token: CancelToken, pub progress: std::sync::Arc<Progress> }

pub trait TaskSink: Send + Sync {
    fn submit(&self, task: TaskDesc) -> Result<TaskHandle, TaskRejected>;  // 非阻塞
    fn poll(&self, time_budget: std::time::Duration) -> Vec<TaskOutcome>;   // 每帧调用
}

/// 带版本校验的结果回写：后台任务唯一的"提交"方式
pub enum TaskOutcome {
    Ready { task: TaskId, base_version: u64, payload: TaskPayload },
    Stale { task: TaskId, base_version: u64, current: u64 },
    Cancelled(TaskId),
    Failed { task: TaskId, error: TaskError },
}

// crates/neo-query/src/lib.rs —— 快照发布（读路径无锁）
pub struct SnapshotCell { /* ArcSwap<MapSnapshot> */ }
impl SnapshotCell {
    pub fn load(&self) -> arc_swap::Guard<Arc<MapSnapshot>>;   // 单帧有效
    pub fn load_full(&self) -> Arc<MapSnapshot>;               // 跨线程 / 跨帧
    pub fn store(&self, snapshot: MapSnapshot);                // 唯一发布点
}

// crates/neo-render/src/gpu.rs —— 资源所有权与增量上传
pub struct GpuCtx<'a> { pub device: &'a wgpu::Device, pub queue: &'a wgpu::Queue, pub frame_no: u64 }

pub struct BufferPool { /* free: Vec<PooledBuffer>, in_flight: u8, upload_cap: usize */ }
impl BufferPool {
    pub fn ensure(&mut self, id: BufferId, len: u64, ctx: &GpuCtx<'_>) -> &wgpu::Buffer;
    pub fn upload(&mut self, id: BufferId, range: std::ops::Range<u64>, data: &[u8], ctx: &GpuCtx<'_>);
    pub fn collect(&mut self, completed_frame: u64);   // 每帧末回收已完成槽位
}

pub struct PickReadback { /* 2 个 staging buffer 轮换 */ }
impl PickReadback {
    pub fn request(&mut self, ctx: &GpuCtx<'_>, rect: [u32; 4]);
    pub fn try_take(&mut self, device: &wgpu::Device) -> Option<PickResult>;   // 绝不 poll(wait)
}
```

## 4 数据流与时序

### 4.1 一次场景重建（跨线程）

```
UI: 编辑提交（version=n）或相机变化
      +-- 命中缓存（version 未变且相机同档）--> 复用 SceneFrame
      '-- spawn SceneTask{ version=n, view, token } --> Rayon
              可见集（视口裁剪 + LOD）-> 几何（脏 way 增量 / 全量两档）
              '-- 每 2000 个对象 token.check()（可取消）
      <-- TaskOutcome::Ready { base_version: n, payload: SceneFrame }
      next frame: poll（预算内）-> base_version == current ? adopt（双缓冲交换） : Stale（丢弃）
      adopt -> MapRenderer::prepare（脏区间上传）-> encode（paint 阶段）
```

### 4.2 拾取（GPU 回读，1 帧延迟）

```
click -> EditorIntent::Select -> 命中测试（CPU：屏幕半径 -> 投影平面距离）
            '-- 需要精确时：PickReadback::request()（当帧编码 1x1 回读，bytes_per_row = 256）
next frame: try_take(PollType::Poll) -> Some(id) ? 更新 Selection : 沿用 CPU 结果
```

hover 一律走 CPU 拾取（`neo-geo::HitTester`）；GPU 拾取只用于点击，避免每帧回读。

### 4.3 导入与取消

```
Ctrl+O -> ImportTask{token} -> Rayon 解码 -> 新 DataSet（worker 独占）
             |  progress.inc(batch)
             '-- 取消：token.check() -> Err -> 丢弃半成品 DataSet（不进 History）
      <-- Ready{ payload: Installed(DataSet) } -> UI: SnapshotCell::store（一次性安装）-> 场景全量重建
```

导入**不经过逐条 Command**，也不进 History；它在 worker 上构建完整 `DataSet` 后一次性安装，这是单写者模型的唯一结构性例外（[ADR-0006](./adr/0006-single-writer-editor.md)）。

## 5 权衡与替代方案

| 选择 | 替代方案 | 为什么选它 | 代价 |
|---|---|---|---|
| 单写者 + 只读快照 | 细粒度锁 / MVCC | 无锁顺序协议、无死锁类别、undo/journal/OSC 天然同序 | 长事务阻塞输入；批量修改只能分事务串行 |
| `arc-swap` | `RwLock<Arc<T>>` | 读侧无原子 RMW、无写者饥饿、`load` 不阻塞写者 | 每次 store 新 `Arc`；`Guard` 不能跨线程/跨帧 |
| UI 线程内提交 GPU | 独立渲染线程 + 三缓冲 | 拾取/hover/overlay 与 UI 同帧，无 1 帧延迟 | UI 三职合一，帧预算更紧 |
| `crossbeam-channel` 为主 | 全用 `flume` | 同步、延迟稳定、`select!` 成熟、不引入 async | 无 `send_timeout`/`drain`，少数边要切 `flume` |
| 协作式取消 | 抢占式（杀线程） | 安全、可组合、无锁泄漏 | 长函数必须自己埋检查点，漏埋即取消失效 |
| 固定 Rayon 线程数 | 默认 `num_cpus` | 不吃掉 UI/网络核，行为可预测 | 大机器算力用不满（可用 feature 调） |

## 6 风险

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| C1 | 快照深拷贝（当前 `Arc<DataSet>`）在交互路径不可用 | 每次发布 O(n) | 低频发布 + 单线程增量；COW/分块快照列为阶段 6 目标 |
| C2 | 检查点漏埋导致导入"取消不生效" | 用户无法中断 | 统一 `check_batch()` 工具 + 随机取消点属性测试 |
| C3 | 回读路径误用 `wait_indefinitely` | 每次点击一次全流水线 flush → 掉帧 | 源码 grep 断言 + `ReadbackGuard` 封装 |
| C4 | Rayon 与 Tokio 抢核导致 UI 抖动 | 输入延迟升高 | 固定线程数；below-normal 优先级；导入期降并发 |
| C5 | 旧快照被后台任务钉住 | 内存峰值上升 | 取消即 drop；`strong_count` 告警；autosave 用序列化而非持快照 |
| C6 | 插件任务饿死其他任务 | 校验/场景延迟 | 每插件独立队列 + 时间片上限 + 总预算 |

## 7 验收标准

1. 60 秒连续编辑 + 并行场景构建 + 并行校验 + 一次导入：无死锁、无 panic、`version` 单调递增。
2. D1–D10 每条都有对应测试或静态断言（锁顺序注释检查、`Instance::new` 计数、`wait_indefinitely` grep）。
3. `TaskOutcome::Stale` 路径有测试：慢任务 + 中途编辑 → 结果被丢弃且数据不变（`tests/stale_task.rs`）。
4. 取消导入在 1 帧内生效，半成品 `DataSet` 被释放（`Arc` 不泄漏，`strong_count` 回落）。
5. 拾取回读路径 `poll(wait)` 出现次数为 0（grep + 单测）；点击拾取结果 1 帧内返回。
6. 稳态每帧上传 ≤ 128 B（仅 uniform），`PrepareStats.uploaded_bytes` 断言【待实测，见 docs/14-performance-and-verification.md】。

## 8 参考

- `arc-swap`（`load`/`load_full`/`Guard` 语义）：https://docs.rs/arc-swap/1.9.5/arc_swap/
- Rust `Send`/`Sync` 语义：https://doc.rust-lang.org/nomicon/send-and-sync.html
- `crossbeam-channel` 0.5：https://docs.rs/crossbeam-channel/0.5/crossbeam_channel/
- `flume` 0.12：https://docs.rs/flume/0.12/flume/
- Tokio 运行时与 `spawn_blocking`：https://docs.rs/tokio/1.54/tokio/
- Rayon `ThreadPoolBuilder`：https://docs.rs/rayon/1.12/rayon/
- wgpu 30 `PollType`（`Maintain` 已移除）：https://docs.rs/wgpu/30.0.2/wgpu/enum.PollType.html
- wgpu 30 `Buffer::map_async` / `get_mapped_range`：https://docs.rs/wgpu/30.0.2/wgpu/struct.Buffer.html

> 待验证：`Guard: !Send` 是否在所有 `arc-swap` 版本都成立（1.9 文档语义，升级时复核）；COW 分块快照的具体形态（阶段 6 决策）；线程优先级的平台可移植封装（Windows `SetThreadPriority` / Linux `nice`，尚无实测对比）。
