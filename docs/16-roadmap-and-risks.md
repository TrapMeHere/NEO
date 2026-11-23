# 16 · 路线图与风险（Roadmap & Risks）

NEO 按六个阶段推进：**技术底座 → OSM 数据核心 → 基础编辑 → 专业编辑 → 完整生态 → 极限性能**，每个阶段以可量化的 Gate 收口，Gate 不通过就不进入下一阶段，而不是"先堆功能再补性能"。本文给出每阶段的交付物 / 验收 / 依赖 / 风险、Gate 的判定条款、风险登记册（R1–R8），以及一份"什么不算完成"的反幻觉清单；性能数值一律留待实测（【待实测，见 docs/14-performance-and-verification.md】）。对齐口径见 [JOSM 对齐](./13-josm-parity.md)，依赖与版本前提见 [02](./02-workspace-and-dependency-rules.md)。

## 摘要

路线图的组织原则是"**每个阶段都能独立演示、独立失败**"：宁可让阶段 1 只有 1M 对象的平移缩放与拾取，也不要先做出一堆没有性能基线的功能。Gate 是决策点而非仪式：原型指标不达标就不进阶段 2，因为数据核心、编辑与渲染的所有设计都建立在那个基线之上。本文不预置任何性能数字，只定义"测什么、怎么算通过、不通过怎么办"。

## 1 目标与范围

**在范围内**：六阶段的范围与交付物；阶段间的依赖与 Gate 判定条款；三平台与 GPU 后端的覆盖节奏；风险登记册与风险×阶段映射；"什么不算完成"的判定规则。

**不在范围内**：具体实现设计（见 04–12）；性能方法论与测量脚本（见 14）；许可与治理（见 [17](./17-licensing-and-governance.md)）；JOSM 功能逐项对照表（见 [13](./13-josm-parity.md)）。

**三条必须先校正的事实**（否则路线图会照抄错误前提）：

1. OSM API **只有 0.6**。0.7 只是 wiki 构想页，没有部署规范；因此阶段 2/4 的同步层只需实现 0.6（changeset `PUT` 创建、单请求 `application/xml` diff 上传、409/412 语义）。
2. `osmChange` 的实际版本是 `version="0.6"`，**不存在 OSC 0.3**；写侧自研 encoder，读侧同样只认 0.6。
3. **PBF 是 JOSM 核心能力，不是插件**；JOSM 的 Edit 菜单很小（几何操作在 Tools、Data 菜单零核心项、下载/上传在 File）。这决定了 NEO 的"核心 vs 插件"边界：PBF、MapCSS、Presets、校验器、会话/autosave 都属核心。

## 2 设计

### 2.1 阶段与 Gate 总览

```
 阶段1 技术底座 ──► [G1] ──► 阶段2 OSM 数据核心 ──► [G2] ──► 阶段3 基础编辑 ──► [G3]
                        │
                        └─ 原型指标不达标 ──► 不进阶段 2（回退 spike / 改方案，见 §2.8）
 阶段3 ──► [G3] ──► 阶段4 专业编辑 ──► [G4] ──► 阶段5 完整生态 ──► [G5] ──► 阶段6 极限性能
                                                                     │
                                                                     └─ 未达 JOSM 工作流等价 = 不得宣称"可替代"
```

| 阶段 | 主题 | 主交付 crate | 出口条件 |
|---|---|---|---|
| 1 | 技术底座 | `neo-core`、`neo-osm`、`neo-store`、`neo-geo`、`neo-scene`、`neo-render`、`apps/neo-prototype` | G1：原型指标达标 |
| 2 | OSM 数据核心 | `neo-io`、`neo-store`（索引/引用集）、`neo-query`、`neo-geo` | G2：10M 导入正确性 + OSC 往返 |
| 3 | 基础编辑 | `neo-editor`、`neo-tools`、`neo-ui`、`apps/neo-desktop` | G3：可自用编辑，undo/journal 自洽 |
| 4 | 专业编辑 | `neo-api`、`neo-persist`、`neo-validate`、`neo-style` | G4：下载→编辑→校验→上传→冲突闭环 |
| 5 | 完整生态 | `neo-plugin`、`neo-style`（Presets/MapCSS 全集）、`neo-ui`（对话框族） | G5：插件隔离 + 三平台安装包 + Presets 覆盖 |
| 6 | 极限性能 | `neo-scene`（流式 LOD）、`neo-render`（GPU culling）、`neo-store`（压缩） | 发布候选（RC） |

工期只写相对顺序，不写周数：**任何"X 周完成"的承诺在没有基线数据前都是幻觉**。每阶段内部按"垂直切片"交付（一个能演示的端到端路径），而不是按 crate 横切。

### 2.2 阶段 1 · 技术底座

| 维度 | 内容 |
|---|---|
| 交付物 | workspace 骨架（16 crate + 2 app）；`scripts/check-deps.py` 与负例自测；`apps/neo-prototype` 打通"1M 对象 PBF 导入 → 渲染 → 平移缩放 → 拾取"；CI 三平台最小集；ADR-0001/0002/0004/0005/0006/0008/0009 落地；版本解析结论回填 [02](./02-workspace-and-dependency-rules.md) §2.6 |
| 可量化验收 | (a) `check-deps.py` 退出 0，3 个负例退出 2；(b) 1M 对象导入成功且对象计数与独立工具一致；(c) 平移/缩放/拾取在参考机连续运行 10 分钟无崩溃；(d) 四项基线（导入耗时、首帧时间、平移帧时间 p50/p99、峰值 RSS）测出并记录；(e) 三平台构建成功 |
| 依赖 | 无（本阶段建立一切）；输入是 [01](./01-system-architecture.md) 与 [02](./02-workspace-and-dependency-rules.md) 的架构决策 |
| 主要风险 | R1（版本漂移：`egui_dock`/`egui-phosphor` 锁 0.36、`geo`/`rstar` 互操作）、R5（GPU 后端差异）、R6（内存初测） |
| 出口 | **[G1]** 通过才允许进入阶段 2 |

### 2.3 阶段 2 · OSM 数据核心

| 维度 | 内容 |
|---|---|
| 交付物 | SoA `DataSet` 全量列（Node/Way/Relation/Tags/Meta）；`MapSnapshot`/`SnapshotCell` 与索引（id→row、node→ways、way→relations、bbox）；PBF/XML/osmChange 0.6 读写（osmpbf 只读 + 自研 encoder）；批量导入路径；增量空间索引（rstar 0.14）；10M 对象导入路径 |
| 可量化验收 | (a) 10M 对象 PBF 导入成功，对象计数 / 坐标 / 标签抽样与独立工具逐项一致；(b) OSC 往返等价（写入→读回→再写，语义级一致）；(c) 引用集完整性 0 错误（每个 way 的节点存在、每个 relation 成员存在，或显式记为 incomplete）；(d) 导入可取消且不留半成品快照；(e) 导入耗时与内存峰值入基线 |
| 依赖 | 阶段 1 的 `neo-core`/`neo-osm`/`neo-store`/`neo-geo` 骨架与 G1 基线 |
| 主要风险 | R2（负 id、可见性、version 语义偏差）、R6（10M 内存峰值）、R1（osmpbf 0.3.8 API 变化） |
| 出口 | **[G2]** |

### 2.4 阶段 3 · 基础编辑

| 维度 | 内容 |
|---|---|
| 交付物 | `Editor`（提交管线）、`History`、`EditTx`、`Selection`、`Journal`（op log + replay）；工具集：选择 / 绘制 / 移动 / 删除 / 分割 / 合并 / 标签编辑；`neo-ui` 基础面板与 `apps/neo-desktop` 主窗口；保存为 OSC/PBF |
| 可量化验收 | (a) 随机 1000 次编辑 + 随机 undo/redo 序列后，快照与"逐步重放"结果一致；(b) 属性测试：校验失败的事务零副作用（[06](./06-editing-core-and-undo.md) §7）；(c) 杀进程 100 次后 replay 无半条事务；(d) 一次拖动只产生 1 条历史（合并生效）；(e) 编辑期间帧时间与输入延迟达基线 |
| 依赖 | 阶段 2 的快照 / 索引 / 几何失效接口 |
| 主要风险 | R2（JOSM 交互语义：选择规则、快捷键、标签行行为）、R6（History 内存） |
| 出口 | **[G3]**（轻量：功能自用可编辑，不阻塞后续并行开发） |

### 2.5 阶段 4 · 专业编辑

| 维度 | 内容 |
|---|---|
| 交付物 | `neo-api`（API 0.6 客户端、changeset、diff 上传、409/412、notes）；`neo-persist`（SQLite WAL、journal 文件、autosave、崩溃恢复）；`neo-validate`（内置规则 + MapCSS validator）；`neo-style`（MapCSS 渲染样式、过滤器、Presets v1）；冲突解决 UI；下载/上传往返 |
| 可量化验收 | (a) 完整工作流端到端跑通：下载区域 → 编辑 → 校验 → 上传 → 触发 409 → 解决冲突 → 重传成功；(b) 六类 `MergeCase` 全部有集成测试（[06](./06-editing-core-and-undo.md) §2.6）；(c) 断电/杀进程后 `RecoveryReport` 可见且 `replayed_txns > 0`；(d) 校验规则对已知错误集的召回与 JOSM 对齐（按规则逐条比对，差异登记）；(e) autosave 周期与保留代数符合配置 |
| 依赖 | 阶段 3 的 `Editor`/`History`/`Journal` 接口；[09](./09-osm-api-and-sync.md) / [10](./10-persistence-and-recovery.md) / [12](./12-validation.md) 的契约 |
| 主要风险 | R2（冲突语义与 JOSM 差异）、R3（MapCSS/Presets 兼容）、R4（JOSM presets 资产许可）、R1（reqwest 等清单外依赖准入） |
| 出口 | **[G4]** |

### 2.6 阶段 5 · 完整生态

| 维度 | 内容 |
|---|---|
| 交付物 | `neo-plugin`（wasmtime 50 宿主、插件 ABI v1、权限模型、资源限额）；Presets/MapCSS 全集与样式编辑器；影像层（WMS/TMS）与 offset 数据库；更多导入导出格式；三平台安装包与自动更新；CLI |
| 可量化验收 | (a) 至少 3 个真实 JOSM 插件语义被移植为 WASM 插件并可用；(b) 插件无法越权（不能写文件/网络/发布快照，除非显式授权）；(c) 插件超时/崩溃不影响主进程（沙箱隔离测试）；(d) 三平台安装包可冷启动并打开 1M 数据集；(e) Presets 覆盖率与 JOSM 默认预设逐项对照表完成（缺失项有记录） |
| 依赖 | 阶段 4 的 API/持久化/校验；插件 ABI 依赖 `EditCommand` 形态（[ADR-0008](./adr/0008-command-transaction-undo.md)） |
| 主要风险 | R7（插件安全）、R4（GPL：不得拷贝 JOSM 代码/受污染资产）、R3（Presets 兼容）、R8（人力与长期维护） |
| 出口 | **[G5]** |

### 2.7 阶段 6 · 极限性能

| 维度 | 内容 |
|---|---|
| 交付物 | 10M+ 对象交互（流式场景、LOD 分级、GPU culling/indirect draw）；内存压缩（字符串 interning、坐标量化、索引压缩）；多线程场景构建；性能剖析集成（puffin 0.20）；长时间会话稳定性 |
| 可量化验收 | (a) 10M 对象下平移/缩放/拾取达基线（帧时间、p99 输入延迟）；(b) 内存峰值低于预算且 1 小时会话无单调增长；(c) 冷启动时间达标；(d) 与参考实现（全量重建）画面一致（golden 图）；(e) 三平台性能差异有记录与解释 |
| 依赖 | 阶段 5 的完整功能面（性能不能靠砍功能换取） |
| 主要风险 | R5（后端差异：DX12/Vulkan/Metal）、R6（内存峰值）、R8（长期维护） |
| 出口 | 发布候选（Release Candidate） |

### 2.8 里程碑 Gate

**G1（阶段 1 → 2）是唯一的"硬否决" Gate**：原型指标不达标就不进入阶段 2，因为没有基线就无法判断后续设计是否退化。

| Gate | 判定项 | 判定方法 | 不通过怎么办 |
|---|---|---|---|
| **G1** | ① 1M 对象导入耗时；② 首帧时间（导入完成→可见）；③ 平移/缩放帧时间（p50/p99）；④ 峰值 RSS；⑤ GPU 拾取单次延迟；⑥ 连续 10 分钟稳定性；⑦ 三平台可构建 | 统一在参考机（i5-12400F / 31.8 GB / RTX 3070 8 GB / MSVC 14.43）上跑 `apps/neo-prototype` 的固定脚本，取 5 次中位数，记录 commit 与驱动版本 | 允许一次 2 周 spike 修复后复评；仍不通过则按 §5 的降级路径改方案（如 GUI 栈降级、拾取改 CPU），并把结论回填 [02](./02-workspace-and-dependency-rules.md) §2.6 与本文 R1 |
| G2 | 10M 导入正确性（计数/抽样/引用集）、OSC 往返、取消语义 | 与独立工具对比 + 属性测试 | 修复后复评；正确性问题不得带病进入编辑阶段 |
| G3 | 可自用编辑：undo/redo 一致性、崩溃 replay、单次拖动一条历史 | 属性测试 + 杀进程脚本 | 允许带缺口进入阶段 4（并行开发），但缺口必须登记 |
| G4 | JOSM 工作流等价（下载→编辑→校验→上传→冲突）、崩溃恢复、六类冲突 | 端到端脚本 + 手工清单（对照 [13](./13-josm-parity.md) 的 parity 定义） | 补齐缺失流程；未达"可替代"不得对外宣称 |
| G5 | 插件隔离、三平台安装包、Presets 覆盖率 | 沙箱攻击测试 + 安装包冒烟 | 收敛插件权限或推迟发布 |

> 所有量化阈值（除"必须测出这一要求本身"）在 Gate 评审时由实测基线填入，并写入 `docs/prototypes/gate-*.md`；**本文不预置数字**。判定一律取中位数而非最好成绩，且必须记录硬件与驱动版本。

### 2.9 三平台与 GPU 后端覆盖

| 阶段 | Windows (DX12) | Linux (Vulkan) | macOS (Metal) | 说明 |
|---|---|---|---|---|
| 1 | 必测 | 必测（构建 + 冒烟） | 必测（构建 + 冒烟） | G1 要求三平台可构建；Windows 为功能基准 |
| 2 | 必测 | 必测 | 必测 | 导入正确性三平台一致 |
| 3 | 必测 | 必测 | 必测 | 输入/IME 差异在此阶段暴露 |
| 4 | 必测 | 必测 | 必测 | SQLite/wasmtime 的 C 依赖打包在此阶段解决 |
| 5 | 必测（发布） | 必测（发布） | 必测（发布） | 安装包与签名 |
| 6 | 基准机 | 基准机 | 参考机（Metal 非 NVIDIA） | 跨后端性能差异登记为 R5 |

WARP / llvmpipe 软件适配器只做"能启动、能提示"的兜底（`pick-readback` 关闭路径见 [02](./02-workspace-and-dependency-rules.md) §2.7），不承诺性能。

## 3 关键接口

Gate 判定不进运行时，但需要可执行、可存档的结构（`tools/gate/` 的 xtask 风格工具，不属于任何运行时 crate）：

```rust
// tools/gate/src/lib.rs
pub enum Phase { Foundation = 1, DataCore, BasicEditing, ProEditing, Ecosystem, Performance }

pub struct Criterion {
    pub id: &'static str,          // "g1.import_1m_secs" / "g1.pan_frame_p99_ms"
    pub metric: Metric,
    pub unit: Unit,
    pub budget: Option<f64>,       // None = 本次只测不判（首次基线）
    pub comparison: Comparison,    // Less | Greater | Range
}

pub enum Metric {
    ImportSeconds, FirstFrameMillis, FrameTimeP50, FrameTimeP99,
    PeakRssBytes, PickLatencyMillis, CrashFreeMinutes, CorrectnessChecks,
}

pub struct Measurement { pub id: &'static str, pub value: f64, pub at: SystemTime,
                         pub commit: String, pub adapter: String }

pub struct GateReport {
    pub phase: Phase,
    pub measurements: Vec<Measurement>,
    pub verdicts: Vec<Verdict>,
}

pub enum Verdict { Pass(Criterion), Fail { criterion: Criterion, measured: f64 } }

pub trait Gate { fn evaluate(&self, report: &GateReport) -> GateOutcome; }
pub enum GateOutcome { Go, NoGo { blocking: Vec<&'static str> }, Inconclusive }
```

判定结果写入 `docs/prototypes/gate-<phase>.md`（markdown 表格 + 原始 JSON），随 commit 一起入库，**不写在 PR 评论里**。

## 4 数据流

```
阶段开始
   │
   ├─ 定义 Criterion（id / metric / budget: None 表示首次只测不判）
   ▼
在参考机跑固定脚本（release + 固定驱动 + 固定数据集）
   │
   ├─ 产出 Measurement[]（含 commit 与 adapter 名）
   ▼
GateReport ──► Gate::evaluate ──► Go / NoGo / Inconclusive
   │                                │
   │                                └─ NoGo：允许一次 spike，仍 NoGo 则按 §5 改方案
   ▼
写 docs/prototypes/gate-<phase>.md 并回填本文与相关专题的"待验证"条目
   │
   ▼
进入下一阶段（或推迟该阶段，先补缺口）
```

## 5 权衡与替代方案

| 选择 | 替代方案 | 为什么选它 | 代价 |
|---|---|---|---|
| 阶段 1 先做原型基座 | 先做功能（编辑/上传）再补性能 | GPU-first 的架构假设不成立时，后期改动是伤筋动骨的 | 前 1–2 个月没有"可用的编辑器"可演示 |
| 六阶段垂直演进 | 按 crate 横向并行开发 | 每阶段都有端到端可验证路径，Gate 能真实否决 | 某些 crate 会在阶段 3 才真正被用上 |
| G1 硬否决 | 所有 Gate 都软性（记录即通过） | 唯一一个"错了就全盘重来"的决策点值得硬性 | 一次 2 周 spike 的时间成本 |
| 不预置性能数字 | 先定目标数字再实现 | 编造数字会误导后续所有设计评审 | 早期文档显得"没有目标"，靠 Gate 补 |
| Windows 为功能基准 | 三平台同步推进 | 参考机与主要用户群都在 Windows；macOS/Linux 先保证可构建 | Linux/macOS 的体验问题暴露较晚（阶段 3 集中修） |
| 不做浏览器版 | 顺带做 wasm 构建 | 10M 对象超出 wasm32 地址空间，投入产出比不成立 | 失去"网页里试一下"的传播能力 |

## 6 风险

### 6.1 登记册

| # | 风险 | 触发信号 | 影响 | 缓解 | 责任阶段 |
|---|---|---|---|---|---|
| R1 | 版本漂移（GUI 栈 / geo-rstar / 清单外依赖） | `cargo build` 报两份 egui；`cargo tree -d` 非空 | 里程碑被解析问题拖住 | 锁 0.36.2 + `Cargo.lock` 入库 + 升级演练（02 §2.6）；决策点在 G1 | 1、全程 |
| R2 | JOSM 语义差异（选择规则、快捷键、冲突、负 id/可见性） | 用户对照测试发现"JOSM 这样做，NEO 不这样" | 迁移失败：能用但用不惯 | parity 按工作流而非功能数定义（[13](./13-josm-parity.md)）；差异逐条登记 | 3、4 |
| R3 | MapCSS / Presets 兼容缺口 | 真实样式/预设渲染或行为不一致 | 无法使用现有 OSM 样式生态 | 兼容层 + 差异清单；先覆盖 Top-N 样式（[11](./11-style-presets-mapcss.md)） | 4、5 |
| R4 | GPL 资源合规（JOSM 代码 / presets / 图标 / 样式） | 引入来源不明资产；直接粘贴 Java 代码 | 许可传染，整仓不可分发 | 只做行为对齐，不拷代码；资产逐个记录来源与许可（[17](./17-licensing-and-governance.md)） | 全程 |
| R5 | GPU 后端差异（DX12 / Vulkan / Metal / 软件适配器） | 某后端黑屏、颜色偏差、MSAA 缺失、拾取失效 | 三平台体验不一致 | 能力探测 + 降级路径（05 §2.14）；golden 图跨后端比对 | 1、6 |
| R6 | 内存峰值（10M 对象 + 历史 + 快照并存） | 峰值 RSS 超预算；1 小时会话单调增长 | OOM、崩溃、数据丢失风险 | 紧凑布局 + 内存估算 API + 峰值为 G1/G2 指标；压缩列阶段 6 | 1、2、6 |
| R7 | 插件安全（WASM 逃逸、资源耗尽、越权 IO） | 插件可写文件/网络；单插件吃满 CPU | 用户数据风险、信任崩塌 | wasmtime 50 沙箱 + 能力白名单 + fuel/内存上限；攻防测试（[15](./15-plugin-system.md)） | 5 |
| R8 | 长期维护（上游破坏性升级、人力） | 每季度大量时间耗在升级适配 | 项目停滞 | 升级窗口固定；关键依赖隔离在少数 crate；文档与 ADR 同步 | 全程 |

### 6.2 风险与阶段的映射

| 阶段 | 主要风险（按优先级） |
|---|---|
| 1 | R1、R5、R6 |
| 2 | R2、R6、R1 |
| 3 | R2、R6 |
| 4 | R2、R3、R4、R1 |
| 5 | R7、R4、R3、R8 |
| 6 | R5、R6、R8 |

## 7 验收标准

### 7.1 判定规则

1. 每个 Gate 必须有 `GateReport`（含 commit、adapter、5 次中位数），并写入 `docs/prototypes/gate-<phase>.md`；口头结论无效。
2. Gate 的 `NoGo` 必须列出阻塞项 id；`Inconclusive`（测量失败/环境异常）不得当作通过。
3. 任一阶段声称"完成"时，其验收项必须**全部**可复现（脚本或手工清单），且缺项以"未完成"计入，而不是"已知限制"。
4. 性能类判定取中位数；成绩不得取自单次最好值，也不得跨硬件比较。
5. 风险登记册的每条风险在对应阶段结束时有明状态（已缓解 / 接受 / 转移 / 关闭）。

### 7.2 什么不算完成（反幻觉清单）

- 能导入 1M 但导入期间 UI 冻结超过一帧预算 → 不算 G1 通过。
- 有 undo 按钮但拖动一次产生 N 条历史 → 不算编辑可用。
- 上传成功一次但在 409 下无法解释原因 → 不算同步闭环。
- OSC 写出 `<osmChange version="0.6">` 但缺 `changeset` 属性 / 顺序不对 → 不算格式兼容。
- MapCSS 能渲染 demo 样式但真实 OSM Carto 出图错位 → 不算样式兼容。
- 插件跑通 hello world 但可无限循环不超时 → 不算插件可用。
- 三平台"能编译"但没有安装包/冷启动验证 → 不算跨平台支持。
- 10M 对象"能打开"但平移掉到交互不可用 → 不算性能达标。
- 只测了 Windows、Linux/macOS 记为"应该没问题" → 不算三平台。
- 文档写了阈值但没有实测记录 → 不算验收。

## 8 参考

- OSM API v0.6：https://wiki.openstreetmap.org/wiki/API_v0.6
- OsmChange（`version="0.6"`）：https://wiki.openstreetmap.org/wiki/OsmChange
- OSM PBF 格式：https://wiki.openstreetmap.org/wiki/PBF_Format
- JOSM 菜单基线（File/Edit/Tools/Data 实际项）：https://josm.openstreetmap.de/wiki/Help/Menu
- JOSM 项目与许可（GPL-2.0+）：https://josm.openstreetmap.de/wiki/Source
- egui / eframe 0.36.2：https://docs.rs/eframe/0.36.2/eframe/
- wgpu 30.0.2：https://docs.rs/wgpu/30.0.2/wgpu/
- wasmtime 50：https://docs.rs/wasmtime/50.0.0/wasmtime/
- criterion 0.8（基准框架）：https://docs.rs/criterion/0.8/criterion/
- puffin 0.20（帧剖析）：https://docs.rs/puffin/0.20/puffin/
- 版本与依赖证据链：`.research/crate-api-cheatsheet.md`；JOSM 能力/许可清单：`.research/josm-inventory.md`

> 待验证：各阶段工期（无历史速度数据，拒绝估算）；三平台基线数字（需 G1 实测）；`docs/prototypes/` 目录尚未建立，Gate 报告模板随后续 commit 落地。
