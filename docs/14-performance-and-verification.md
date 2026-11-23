# 14 · 性能与验证

> 本文只放**实测**数字；每条都可用 `scripts/bench.ps1` 复现。方法与判据写在前面，结果表在 §4，Gate 结论在 §5。
>
> 数据来源：`apps/neo-prototype`（同一份生产代码路径：`neo-io` 导入 → `neo-store` SoA → `neo-geo` 索引/几何 → `neo-scene` 批次 → `neo-render` wgpu 离屏渲染），由 `pwsh scripts/bench.ps1` 跑 3 轮取中位数。

## 1 测量环境

| 项 | 值 |
|---|---|
| 机器 | Intel Core i5-12400F（6C/12T） |
| 内存 | 31.8 GB |
| GPU | NVIDIA GeForce RTX 3070 8 GB |
| GPU 后端 | **Vulkan**（wgpu 30.0.2；DX12 适配器同样可用，脚本当前枚举到 Vulkan） |
| 驱动 | NVIDIA 621.14 |
| OS | Windows 11（10.0.26100），MSVC 14.43 |
| Rust | 1.100.0 stable（MSRV 1.95） |
| 编译 | `--release`（opt-level 3，thin LTO，codegen-units 1） |
| 输入数据 | Monaco（Geofabrik，0.68 MiB，41,844 节点/6,281 way/222 关系）；Bremen（20.3 MiB，**1,668,778 节点/329,676 way/5,053 关系**）；合成网格城市（1M/10M 对象） |

**测量口径**：交互类指标取 P50/P95/P99（同一次运行内的样本分位数）；吞吐类指标取 3 轮中位数；内存取进程 `PeakWorkingSetSize`（Win32 API，进程内自报）。

## 2 测量方法

`scripts/bench.ps1` 依次跑：

1. `neo-prototype gpu-info` —— 记录适配器/后端/驱动/时间戳查询支持。
2. `import --pbf` —— 真实 PBF 导入（含"缺失成员跳过"处理，bbox 提取必然出现不完整 relation）。
3. `render --pbf ... --out out/*.png` —— 完整链路：导入 → 索引 → 场景 → **wgpu 离屏渲染 → PNG 读回**（证明 GPU 管线真的工作，而不是只测 CPU）。
4. `bench --synth 1_000_000 --iterations 30` —— 六个基准：
   - `id lookup`：随机 20 万次 `ObjectId → Handle` 查找（ns/op，含 `Instant::now` 开销）；
   - `bbox query`：随机 2000 个 1200×800@z16 视口的空间索引查询（µs/op，含返回集平均 7,922 条）；
   - `scene build (fit-all)`：整城视角下全量重建场景（最坏情况）；
   - `scene build (z16)`：编辑尺度视口（1600×1000）重建场景；
   - `move node + rebuild parent ways`：移动节点 → 查父 way → 重建 bbox/索引（增量编辑路径）；
   - `pick`：随机屏幕点拾取（节点/way 命中测试）；
   - `gpu frame`：离屏 pass 编码 + 提交 + 等 GPU 完成（不含 PNG 读回）。

## 3 场景构建的两次关键优化（数据可复现）

优化前后都用同一份 1M 合成数据、同一相机、30 次迭代：

| 版本 | scene build (fit-all) P50 | 说明 |
|---|---|---|
| 初始实现 | **157.7 ms** | 每 way 堆分配 + 先建几何再 LOD + 遍历 node+way 混合树（视口覆盖全城时扫描 131 万条目） |
| + bbox 预裁剪 + 零分配 scratch | **89.5 ms** | 用索引 bbox 在生成几何前砍掉 88% 的 way；复用顶点/索引缓冲 |
| + 零分配 R-tree 遍历 + 密度 LOD + 世界坐标缓存 | **17.4 ms** | node/way 分树；亚像素 way 按 stride 抽样（绘制量≈常数/屏幕面积）；节点投影只算一次 |

同样的优化让 **z16 编辑视口** 从 ~15 ms 降到 **1.29 ms P50**（4,164 way + 12,638 节点可见）。

这三次优化对应 ADR-0009 的三个层次：**先裁剪、再复用、最后增量**。

## 4 实测结果

### 4.1 导入（单线程顺序解码，真实 PBF）

| 数据 | 对象数 | 导入耗时 | 吞吐 | 索引构建 | 反向引用 | 峰值 RSS |
|---|---|---|---|---|---|---|
| Monaco | 41,844 N / 6,281 W / 222 R | 0.03 s | 1.70 M elem/s（23.8 MiB/s） | 18.3 ms | 0.6 ms | 26.2 MiB |
| Bremen | 1,668,778 N / 329,676 W / 5,053 R | **1.12 s** | **1.79 M elem/s**（18.1 MiB/s） | **~1.2 s** | ~14 ms | 【见 10M/内存表】 |

- Bremen 有 958 个 relation 因 bbox 提取不完整被跳过（预期行为，记录在 `ImportReport.skipped_relations`）。
- 内存估算：Bremen 数据集 `mem=274 MiB`（SoA + tag arena + 索引估算），1M 合成数据 138.7 MiB。
- PBF 解码目前是单线程；`osmpbf` 的 blob 级并行（`par_map_reduce`）是阶段 2 任务（docs/08 §2.4），预期可再用满 12 线程。

### 4.2 交互（1M 合成对象，30 次迭代，P50/P95/P99）

| 指标 | mean | P50 | P95 | P99 | 预算（阶段 1） |
|---|---|---|---|---|---|
| id lookup | 164 ns | 100 ns | 300 ns | 700 ns | < 1 µs |
| bbox query（z16 视口） | 272 µs | 273 µs | 417 µs | 516 µs | < 1 ms |
| **scene build（z16 编辑视口）** | 1.44 ms | **1.29 ms** | 2.33 ms | 2.63 ms | **< 16 ms ✅** |
| scene build（fit-all 最坏） | 18.6 ms | 17.4 ms | 19.5 ms | 52.2 ms | 参考值（重建只在数据变化/大幅跳转时发生） |
| move node + 重建父 way + 索引 | 17.8 µs | 17 µs | 27 µs | 42 µs | < 1 ms ✅ |
| pick（节点/way） | 8.1 µs | 12 µs | 17 µs | 22 µs | < 16 ms ✅ |
| **GPU frame（缓存命中，encode+submit+wait）** | 0.245 ms | **0.083 ms** | 0.38 ms | 4.52 ms | 60 FPS = 16.6 ms ✅ |

关键结论：

1. **编辑视口下 60 FPS 有充足余量**：场景重建 1.29 ms + GPU 0.08 ms ≈ 1.4 ms/帧。
2. **平移/缩放不重建几何**：只写 32 字节相机 uniform（docs/05），所以交互帧成本 ≈ GPU 帧 + UI；场景重建只在"数据变化 / 跳转超过 25% 视口 / 离开批次原点 4 km"时发生。
3. 全城视角（fit-all）重建 17 ms：由密度 LOD 控制绘制量（28,600 way / 57k 三角形），属于一次性开销，不是每帧成本。

### 4.3 GPU（真实 wgpu 离屏渲染）

| 场景 | 绘制内容 | 上传 | GPU 帧（含等待） | PNG |
|---|---|---|---|---|
| Monaco @ z11，1600×1000 | 659 way / 4,622 三角形 / 2 draw calls | 128 KB / 0.11 ms | 4.95 ms（首次含管线初始化） | 8.0 ms（渲染+读回+编码） |
| Bremen @ z9.21，1920×1200 | 20,237 way / 142,300 三角形 / 2 draw calls | 3.99 MB / 0.88 ms | 4.39 ms | 11.9 ms |
| 1M 合成 @ z16，1600×1000 | 4,164 way + 12,638 节点 / 33,604 三角形 | 缓存命中（0 B） | 0.083 ms（P50） | — |

- 同一帧内 node/way/fill 三个批次 → **2–3 个 draw call**，而不是每对象一个。
- 相机平移/缩放时几何不重传：`upload 0 B` 是预期结果（缓存命中）。
- 支持 `TIMESTAMP_QUERY`（可做 GPU 计时），本阶段先用 CPU 侧 submit+wait 测量。

### 4.4 内存（1M 合成对象）

| 项 | 数值 |
|---|---|
| SoA + tag arena + ID 索引（`DataSet::mem_bytes`） | 138.7 MiB |
| 节点世界坐标缓存（`WorldCache`，16 B/节点） | ~16 MiB |
| 空间索引（rstar 两棵树 + entries 映射） | ~120 MiB（条目 1,312,250） |
| **进程峰值 RSS** | **566.8 MiB** |
| 每对象估算 | ~139 B（SoA 部分） |

> 峰值 RSS 远高于 `mem_bytes` 估算的原因：rstar 节点块、场景批次（顶点/索引）、PNG 缓冲与分配器碎片。10M 对象的峰值见 §4.5（若未跑则标 `【待实测】`）。

### 4.5 10M 对象压力（实测）

`neo-prototype bench --synth 10000000 --iterations 5`（合成网格城市，node:way ≈ 3.2:1）：

| 项 | 数值 |
|---|---|
| 对象数 | **10,001,406 节点 / 3,129,589 way / 78,210 关系** |
| 构建（生成） | 3.31 s |
| 空间索引构建 | 9,154.8 ms（13,130,995 条目） |
| 反向引用构建 | 147.3 ms |
| `DataSet::mem_bytes` 估算 | 1,693.1 MiB |
| **进程峰值 RSS** | **4,159.1 MiB（4.06 GiB）** ✅（阶段 1 建议预算 ≤ 6 GB） |
| 场景重建（z16 编辑视口） | **2.03 ms P50** / 2.71 ms P99 |
| 场景重建（fit-all 最坏） | 143 ms P50（97,800 way 抽样绘制 / 195,600 三角形） |
| id lookup | 300 ns P50 |
| pick | 195 µs P50 |
| GPU 帧（缓存命中） | 0.23 ms P50 |

结论：

- **千万对象可导入、可索引、可交互**；编辑视口 2 ms 的场景重建成本随规模基本不变（因为绘制量由视口与 LOD 决定，而不是由总量决定）——这正是"绘制量由视口决定"设计目标的实测证据。
- 主要随规模变差的项是**索引构建（9.2 s）**与**fit-all 重建（143 ms）**，两者都在阶段 2 的优化清单里。
- 内存 4.06 GiB 中约 1.7 GiB 是 SoA + tag arena + ID 索引，其余是 rstar 节点块、世界坐标缓存、场景批次与分配器碎片。

## 5 Gate-1 判定（详见 `docs/prototypes/gate-1-prototype.md`）

| G1 判定项 | 结果 | 判定 |
|---|---|---|
| ① 1M 对象导入 | Bremen 真实 PBF 1.79M elem/s；1M 合成 0.19 s | ✅ |
| ② 首帧时间（导入→可见） | Monaco 全链路 < 100 ms；Bremen 导入 1.12 s + 索引 1.2 s + 场景 90 ms | ⚠️ 需阶段 2 优化索引构建/并行解码 |
| ③ 平移/缩放帧时间 | 场景重建 z16 = 1.29 ms P50 / 2.63 ms P99；平移不重建 | ✅ |
| ④ 峰值 RSS（1M） | 566.8 MiB；**10M = 4,159.1 MiB（4.06 GiB）**，低于 6 GB 建议预算 | ✅ |
| ⑤ GPU 拾取延迟 | 当前拾取在 CPU：12 µs P50（1M）/ 195 µs P50（10M）；设计选择，docs/04；GPU ID buffer 为可选优化 | ✅（口径修正） |
| ⑥ 连续 10 分钟稳定性 | 8 秒 GUI 冒烟通过（窗口存活、PBF 加载 + 场景 + wgpu 首帧无崩溃）；10 分钟连续交互未跑 | ⚠️ 部分 |
| ⑦ 三平台可构建 | 仅 Windows 本机验证；Linux/macOS 需 CI | ❌ 未验证 |

**结论：G1 = Conditional Go（有条件通过）**。编辑尺度交互、GPU 管线、1M/10M 内存均已达标；阻塞项只剩"三平台构建 + 10 分钟稳定性"，必须在进入阶段 2 前补齐（见 gate 报告）。

## 6 已知瓶颈与下一步（按收益排序）

1. **PBF 并行解码**（阶段 2）：单线程 1.79M elem/s；blob 级 rayon 并行预期 3–5×。
2. **空间索引构建**（1M 1.3M 条目 572 ms；Bremen 1.2 s）：可并行 bulk_load / 分块构建。
3. **全城视角场景重建 17–90 ms**：真正的修复是"按脏 way 增量重建批次"（ADR-0009 的下一层），以及把几何缓存做成 per-way 可失效条目。
4. **bbox query 273 µs**：返回 7,922 条目的 Vec 分配是主要成本；场景路径已改为零分配迭代器，拾取路径仍是小范围查询。
5. **PBF 写侧**：`osmpbf` 只读，需要自研 varint encoder（docs/08）。

## 7 复现

```powershell
# 一次性准备（README 有完整说明）
# .research/data/monaco-latest.osm.pbf / bremen-latest.osm.pbf
pwsh scripts/bench.ps1 -Runs 3
# 原始输出：out/bench-summary.md、out/prototype-bench.md、out/*.png
```

## 8 参考

- wgpu 30 计时与 `PollType`：<https://docs.rs/wgpu/30.0.2/wgpu/>
- rstar 0.14 bulk_load / 迭代器：<https://docs.rs/rstar/0.14.0/rstar/>
- osmpbf 0.3.8：<https://docs.rs/osmpbf/0.3.8/osmpbf/>
- 机器与数据来源：Geofabrik extracts <https://download.geofabrik.de/>


## 9 G2 / G3 验收结果（2026-11-15，第二轮）

自动化验收（`cargo test --workspace`，98 个测试全绿；本节 3 项为 G3 要求）：

| G3 判定项 | 测试 | 结果 |
|---|---|---|
| 随机 1000 次编辑 + 随机 undo/redo 后与"逐步重放"一致 | `crates/neo-editor/tests/g3_acceptance.rs::random_edits_with_undo_redo_match_replay` | ✅ 状态摘要（节点 ID/坐标/tag 数）逐位相等，且每步 `history.undo_len == 已应用操作数` |
| 事务失败零副作用 | `...::failed_batch_rolls_back_and_keeps_history_clean` | ✅ 批量第二条失败 → 第一条回滚、历史不增长、坐标回到原值 |
| journal replay 丢弃半条事务 | `...::journal_prefix_replay_drops_unfinished_transaction` + `journal::tests::committed_prefix_drops_unfinished_transaction` | ✅ 尾部未提交 MoveNode 未生效 |
| OSC 写→读→再写语义等价（G2） | `crates/neo-io/tests/osc_roundtrip.rs::osc_write_read_write_is_semantically_stable` | ✅ create/modify/delete 分组 ID 集合一致（含负 ID 与删除对象） |
| 一次拖动只产生 1 条历史 | `neo-tools::tests::drag_end_is_single_history_entry` + 桌面拖拽实现（预览不入历史） | ✅ |
| 原子性/撤销单测 | `neo-editor` 15 个单测（AddNode/AddWay/Move/SetTags/SetWayNodes/Delete/Split/Merge/Reverse + undo/redo identity + merge） | ✅ |

G2 相关（部分）：

| G2 判定项 | 状态 |
|---|---|
| OSM XML 0.6 读 + 两种写模式 + 往返语义等价 | ✅ `neo-io/xml.rs` 6 个单测 |
| osmChange 0.6 读 + 写 + apply | ✅ `osc.rs`（写）+ `osc_read.rs`（读/应用，3 个单测） |
| PBF 读（osmpbf） | ✅ Monaco/Bremen 实测（docs/14 §4） |
| **PBF 写 encoder** | ✅ `neo-io/pbf_write.rs`（wire 编码 + zlib；写→osmpbf 读回全等，3 个集成测试 + 原型 `pbf-roundtrip`） |
| 引用完整性 / 导入取消 | ✅ `neo-store/integrity.rs`（2 单测）、`ImportOptions.cancel`（2 集成测试） |
| 10M 对拍 / 并行解码 | ❌ 待实现（TODO 2.7–2.8） |

复现：

```powershell
cargo test -p neo-editor --test g3_acceptance
cargo test --workspace
```


## 10 并行导入与 10M PBF 往返（2026-11-15）

### 10.1 顺序 vs 并行 PBF 导入（release）

| 数据 | 元素数 | 顺序导入 | 并行导入 | 加速比 |
|---|---:|---:|---:|---:|
| Bremen（20.3 MiB） | 2,003,507 | 1.163 s | **0.899 s** | 1.29× |
| Monaco（0.7 MiB） | 48,347 | 0.028 s | 0.026 s | 1.11× |

结论：blob 级并行解码有效但**未达 3× 目标**。Amdahl：单线程装配（string interning + ID HashMap 插入）
占绝对大头；下一步若需要更高吞吐应做"分区 DataSet + 合并"或按 id 分片并行 interning。

复现：`neo-prototype compare-import --pbf .research/data/bremen-latest.osm.pbf`

### 10.2 10M 合成数据 PBF 写→读对拍

```
synthetic: 10,001,406 nodes, 2,503,908 ways, 78,210 relations in 2.60s
PBF written: 10,001,406 nodes, 3,129,589 ways, 78,210 relations, 86,651,971 bytes in 2904 blobs
before: nodes=10,001,406 ways=3,129,589 relations=78,210 node_refs=8,136,221 members=625,680 tags=3,207,799 mem=1532.1 MiB
after : nodes=10,001,406 ways=3,129,589 relations=78,210 node_refs=8,136,221 members=625,680 tags=3,207,799 mem=1532.1 MiB
integrity: 10,001,406 nodes, 3,129,589 ways, 78,210 relations, 0 issues
OK: PBF roundtrip preserved object counts
```

写侧 encoder + 读侧 osmpbf 在 10M 规模上语义一致（坐标/引用/成员/tag 计数），格式正确性闭环。

复现：`neo-prototype pbf-roundtrip --synth 10000000 --out out/10m-roundtrip.pbf`


## 11 会话文件与多选（2026-11-15）

| 验收项 | 测试/证据 | 结果 |
|---|---|---|
| `.jos` 往返（viewport/layers/selection） | `neo-persist::session::tests::jos_roundtrip` | ✅ |
| 未知插件图层段被容忍 | `...::unknown_elements_and_attributes_are_ignored` | ✅ |
| `.joz` ZIP 嵌入图层 + 读取解压 | `...::joz_embeds_and_extracts_layers` | ✅ |
| Shift+点击多选 | `neo-tools::multi_select_tests::shift_click_extends_selection` | ✅ |

桌面端已接入：保存/打开会话（`.jos`/`.joz`）、批量删除、合并两条 way、relation 属性/标签编辑。


## 12 桌面 UI 模拟输入手测（2026-11-15）

工具：`scripts/ui_check.ps1`（Win32 `SetCursorPos`/`mouse_event`/`keybd_event` + `CopyFromScreen` 截图；窗口按标题
`NEO — Next Editor for OSM` 定位、按 DPI 换算坐标）。截图：`out/ui-check/01-initial.png` … `13-zoom-back.png`。

| 场景 | 结果 |
|---|---|
| 启动 + Monaco PBF（41844 N / 6281 W / 222 R）+ wgpu 首帧 | ✅ z15.1（修复前 z11） |
| 中文字体/工具栏/左右面板/状态栏 | ✅（修复前 tofu 方块） |
| A 绘制：点击建节点/way（吸附近似节点）、Enter 结束线 | ✅ 节点+2 / way+1 / 脏 3 / 撤销 4 |
| Ctrl+Z×3 / Ctrl+Y×3 | ✅ 撤销 1 重做 3 ↔ 撤销 4 重做 0，消息与版本号正确 |
| S 选择节点 → 右侧属性（id/lat/lon/version）+ Tags 编辑面板 | ✅ |
| 删除被 way 引用的节点 | ✅ 级联删除：先把它从各 way 摘掉，2 点 way 会先提示再整条删除（`plan_delete`） |
| 同时选中节点与其 way 删除 | ✅ 一批提交、一次撤销（修复前逐条 execute 且节点先检查 → 永远 "0/2"） |
| 工具栏「示例数据」、滚轮缩放 | ✅ 重置为 v1；z15.49→15.82→15.49 |
| 原生文件对话框 / Shift 多选 / 合并按钮 | ⚠️ 未覆盖（需人工或更精确的 GUI 自动化） |

修复的 UI 缺陷：CJK 字体缺失、状态消息不可见、动作后未请求重绘（UI 滞后一步）、初始视图分位点拟合。


## 13 桌面渲染卡顿：根因与修复（2026-11-15）

**现象**：拖动节点/缩放时明显卡顿（"渲染太慢"）。

**诊断方法**：`NEO_PERF_LOG=out/ui-check/perf.log RUST_LOG=neo_desktop=debug,neo_render=debug` + `scripts/ui_check.ps1` 模拟输入，
日志每帧记录 `scene build` / `gpu upload` / `ui frame`。

**根因**：节点拖拽预览把每个 mousemove 都写回 `DataSet`（`preview_move_node`），`data.version()` 每帧递增，
`ensure_scene` 因此每帧整场重建（Monaco 7–8 ms / 200k synth 14–15 ms），`GpuScene` 按 version 全量重传
（Monaco 3.9 MiB / synth 200k 1.9 MiB），叠加 UI 帧耗时后约 20–40 ms/帧。

| synth:200000，拖拽节点约 1s | 修复前 | 修复后 |
|---|---:|---:|
| scene build 次数 | 78 | **2** |
| gpu upload 次数 / 字节 | 78 / 1.86 MiB×78 | **2 / 3.7 MiB** |
| ui frame > 8 ms | 全部 | **2** |

**修复**：
1. 预览不写数据：`DragState.current: LatLon` + egui 叠加层画邻居连线/目标点（`Editor::node_neighbors`）；
2. 释放时单次 `MoveNode`（单条历史、单次重建 + 上传）；
3. `MoveNode` 标记 `ChangeState::Modified`（脏对象计数与上传 diff 正确）；
4. 双击启动 CWD 不是仓库根导致加载 synth 的问题：示例数据按 exe 相对路径查找；
5. 新增 `NEO_SYNTH_N` / `NEO_PERF_LOG` 测试钩子。

复现：`powershell -File scripts\ui_check.ps1 start|drag|truepan`，日志见 `out/ui-check/perf-*.log`。


### 13.1 追加：放大到 z19 再缩小后地图变空（2026-11-15）

**现象**：只做缩放、没有任何编辑，放大到 z19+ 再缩小，地图只剩几条线（视口裁剪结果停留在高倍率）。

**根因**：`ensure_scene` 的**重建触发条件只有**「数据版本变化 / 原点重基 >4km / 平移超过 25% 视口」，
缩放变化不会触发重建；而放大到 z19 时的高倍率视口（或此时轻微平移触发的重建）把场景裁剪到很小范围，
缩小回 z16 后仍渲染这份旧批次 → 只剩少量元素。另外 `GpuScene` 的上传缓存 key 只有
(data version, origin)，即使重建，缩放后 origin/version 未变也会**跳过重传**。

**修复**：
1. `ensure_scene` 增加触发条件：`|scene.camera.zoom - camera.zoom| > 0.5` → 重建（LOD/裁剪随缩放更新）；
2. `GpuScene` 上传 key 增加 `camera.zoom`（几何含 LOD，缩放重建后必须重传）；
3. 自动化回归：`scripts/ui_check.ps1 zoomcycle`（z16 → 滚轮放大到 z19+ → 缩小到 z13 → 再放大到 z16），
   截图 `23-zoomed-in-z19.png` / `24-zoomed-out-z12.png` / `25-zoomed-in-z16-again.png`，
   全程 10 次重建/10 次上传，最终地图与缩放前一致。


### 13.2 选中要素的可拖动外边距（2026-11-15）

选中要素只有在鼠标位于其**屏幕外边距 16px** 内时才可拖动：节点按点距离、way 按折线线段距离；
判定使用**按下原点**（`pointer.press_origin()`），避免 egui 拖拽阈值（~6px）触发时指针已移动导致误判。
远离要素的拖动不会移动任何对象。

自动化验证（`NEO_TEST_SELECT=node` 选中 z19 节点，红圈位置由截图定位）：

| 起点 | 结果 |
|---|---|
| 距离节点 ≤16px | 撤销 1、`移动 1 个节点`、坐标更新 ✅ |
| 距离节点数百 px（空白区域） | 撤销 0、无移动 ✅ |
