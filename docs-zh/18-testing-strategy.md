# 18 · 测试策略（Testing Strategy）

测试源码与程序、文档一起放在 `master`。用 `cargo test --workspace` 运行工作区测试；CI 在每次 push 与 PR 上都会运行（见 `.github/workflows/ci.yml`）。已退役的 `test` 分支不再使用。

**摘要**：NEO 的测试体系围绕一个命题构建：**"我们没有 JOSM 的源码，所以只能靠可执行证据来声称等价"**。因此测试不是质量活动的附属品，而是 parity（[13](./13-josm-parity.md)）与验证器规则（[12](./12-validation.md)）的**唯一证据链**。体系分七层：单元 → 属性（proptest 风格）→ 集成 → golden 兼容性 → 端到端 UI（egui kittest）→ GPU 图像 → 性能回归，外加 fuzz、loom 并发、压力三类专项。golden 层是核心创新点：**同一输入 → 归一化对象集哈希 → 与 JOSM 黑盒 CLI 双跑对拍**，从而在"不读 JOSM 源码"的前提下获得可证伪的等价性。所有性能阈值一律【待实测，见 docs/14-performance-and-verification.md】。

## 1 目标与范围

| 目标 | 判据 |
|---|---|
| 数据安全 | 任何路径（打开/编辑/保存/上传/恢复）都不产生静默数据丢失；往返保真可断言 |
| 等价性可证伪 | parity 矩阵每条 P0 至少有一个自动化用例（golden / 集成 / E2E） |
| 回归可定位 | 失败信息能指出"哪个对象、哪个字段、期望 vs 实际"，而不是只给一个哈希不等 |
| 反馈速度 | PR 门禁 ≤ 15 分钟（不含 GPU 真机与 10M 夹具）；nightly 全量 ≤ 90 分钟 |
| 不确定性可控 | 浮点、顺序、时钟、随机数、GPU 驱动全部显式归一或注入，禁止 `sleep` 式同步 |

**不在范围内**：JOSM Java 单元测试的移植（GPL 代码，不可转写，见 [17](./17-licensing-and-governance.md) §2.2）；UI 视觉像素级比对（我们不是 Swing）；平台安装包签名验证；渗透测试（单独的安全评审）。

## 2 设计

### 2.1 测试金字塔

| 层 | 工具 | 数量级 | 单次时长 | 门禁 | 主要职责 |
|---|---|---|---|---|---|
| L1 单元 | `#[test]`（in-crate） | 3k+ | ms | PR | 纯函数：投影、几何谓词、归一化、解析器小块 |
| L2 属性 | `proptest` 风格 | 200+ | 10–100 ms | PR | 不变量（§2.2 表），输入空间随机但可复现 |
| L3 集成 | `tests/*.rs`（workspace 级） | 300+ | 100 ms–5 s | PR | 跨 crate：打开→编辑→保存→上传（mock API） |
| L4 golden | 自建 harness（§3.1） | 150+ | 0.1–3 s | PR | 归一化对象集哈希、OSC 输出、校验报告 |
| L5 E2E UI | `egui_kittest` | 60+ | 0.2–2 s | PR | 面板交互、快捷键序列、撤销可见性 |
| L6 GPU 图像 | 离屏 render + 感知哈希 | 40+ | 0.5–3 s | PR（软件适配器）/ nightly（真机） | 样式级联、LOD、拾取一致性 |
| L7 性能 | `criterion` 0.8 / `iai-callgrind` 0.16 | 80+ | 秒级 | nightly + 发布前 | 帧时间、导入、验证、上传生成 |
| 专项 fuzz | `cargo-fuzz` | 5 目标 | 分钟级 | nightly | 解析器健壮性（PBF/XML/OSC/MapCSS/preset） |
| 专项 并发 | `loom` + 压力 | 20+ | 秒–30 min | nightly | 发布/日志/池的原子顺序 |
| 专项 内存 | `dhat` 0.3 | 5+ | 分钟级 | nightly | 分配热点与泄漏 |

金字塔的"比例"约束：L1+L2 应占全部测试的 ≥ 70%，L5+L6 ≤ 10%。若 E2E 数量超过单元数量，说明抽象层缺少测试缝——这是设计问题，不是测试问题。

### 2.2 必须成立的编辑与格式不变量

这些是 L2 属性的核心清单，每条都对应一个真实会发生的 bug 类：

| ID | 不变量 | 形式化 | 失败意味着 |
|---|---|---|---|
| INV-A | 撤销/重做互逆 | `redo(undo(execute(c, S))) ≡ execute(c, S)` | 命令的逆不完整（最常见：漏存旧 tag 值） |
| INV-B | 撤销的撤销等于原状 | `undo(execute(c, S)) ≡ S` | 逆操作污染了未触及字段 |
| INV-C | 失败命令原子 | `execute(c, S)` 返回 `Err` ⇒ 状态 ≡ `S` | 部分应用（写一半就报错） |
| INV-D | 序列合并等价 | `execute_batch([a,b]) ≡ execute(a); execute(b)`（除撤销粒度） | 批处理走了不同的写路径 |
| INV-E | 序列化确定性 | 同一 `Snapshot` 两次 `write` 字节相等 | HashMap 迭代顺序泄漏进输出 |
| INV-F | XML 往返语义等价 | `parse(write(parse(x))) ≡ parse(x)` | 丢字段（`action`、未知属性、`visible`） |
| INV-G | PBF 往返语义等价 | 同上（PBF 域） | granularity/负 id/`DenseInfo` 处理错误 |
| INV-H | OSC 可重放 | 把生成的 OSC 以"另一端"解回并应用，得到同一变更集 | 缺 `if-unused` 或负 id 引用错乱 |
| INV-I | 投影往返 | `proj(unproj(p)) ≈ p`（误差 ≤ 1e-9 相对） | 公式笔误、基准面混用 |
| INV-J | 选择不泄漏 | 操作 A 图层选择后，B 图层的 `Selection` 不变 | 把选择做成全局集合（JOSM 是 per-DataSet） |
| INV-K | 校验修复幂等 | 修复后重跑同规则不再产同一 `code` | 修复条件与该规则判定条件不一致 |

属性测试的输入生成器必须**带结构**：不是随机构造字节，而是"随机构造合法 DataSet"（对象数、标签组合、拓扑引用、负 id 比例、relation 嵌套深度都是参数），并从失败用例自动 `proptest-regressions/` 落盘。

### 2.3 golden 兼容性测试设计

**核心思想**：把"行为"压缩成一个可比较的**规范形式**（canonical form），再对它取哈希。四条链路各有一类 golden：

```
① 对象集 golden
   fixtures/monaco.osm.pbf ─► NEO 解析 ─► canonical 序列化 ─► sha256 ─► 比对
                                    └─► 人类可读 .osm 快照（用于失败 diff）

② 编辑序列 golden
   fixtures/base.osm + ops.yaml（[{split, way:17}, {reverse, way:23}, …]）
        ─► 逐命令执行 ─► OSC 生成 ─► canonical ─► sha256

③ 校验报告 golden
   同一输入 + 固定规则集 ─► ValidationReport ─► 按 (severity, code, 对象集) 排序
        ─► 文本投影 ─► sha256（人类可读：每行 "W geometry/self_intersection way/17"）

④ 渲染 golden（见 §2.6，走感知哈希而非 sha256）
```

harness 接口（`tests/golden/harness.rs`）：

```rust
pub trait GoldenCase {
    fn name(&self) -> &'static str;
    fn fixture(&self) -> Fixture;                  // PBF/XML 路径 + 可选操作脚本
    fn produce(&self, rt: &TestRuntime) -> Artifact;
    fn norm(&self) -> NormRules;                   // 该用例的归一化规则（默认 §3.1 全集）
}

pub struct TestRuntime {           // 一切不确定性的注入点
    pub clock: FixedClock,         // 固定时间戳，杜绝 golden 因时间漂移
    pub ids: IdAllocator,          // 负 id 分配顺序确定
    pub rng: StdRng,               // 固定种子
    pub threads: usize,            // 1 与 8 各跑一次，结果必须相同
}
pub struct Artifact { pub canonical: Vec<u8>, pub human: String }
pub fn assert_golden(name: &str, art: &Artifact);   // 比对 tests/golden/<name>.sha256，失败时打印 human 的 unified diff
```

**JOSM 双跑 harness（oracle）**：以**外部进程**方式调用 JOSM CLI（`java -jar josm-tested.jar render|validate …`），读取其输出文件并套用同一套归一化，再与 NEO 的输出比较。这条路径是黑盒的：不链接 JOSM 代码、不分发其 jar、产物是运行结果而非衍生作品；同时 CI 上 **默认 skip**（`--features josm-oracle`），只在 nightly 的专门 job 里跑。

| 对拍对象 | 归一化后比较 | 允许差异 |
|---|---|---|
| `render` 输出 PNG | 感知哈希汉明距离（§2.6） | 抗锯齿、字体渲染 |
| `validate` 报告 | issue 集合（`code` 映射表 + 对象集） | 文案措辞（只比 code 与对象） |
| `project` 输出坐标 | 相对误差 ≤ 1e-9 | 无 |
| 对象集 | canonical sha256 | 无（有差异即视为 bug 或登记偏差） |

code 映射表（`tests/golden/josm-code-map.toml`）是必要的：JOSM 的报告里是本地化 message，我们比的是稳定 code，因此需要一张"JOSM 测试类/mapcss 规则 → NEO `code`"的映射，映射缺失即测试失败（防止悄悄跳过对拍）。

### 2.4 往返保真：字节级不可比时用语义等价

| 格式 | 能否字节级往返 | 策略 |
|---|---|---|
| OSM XML | **能**（若我们输出规范化） | 主断言：`write(parse(x))` 与 `parse(x)` 的 canonical 相等；附加断言：对**未修改**文档，`write` 输出与输入在规范化后逐元素相等 |
| PBF | **不能**（zlib 压缩块、字符串表顺序、`DenseNodes` 分块由写出器决定） | 只做语义等价：`parse(write(parse(x))) ≡ parse(x)`；另加"信息不丢"断言：`version/timestamp/changeset/uid/user/visible` 逐对象相等 |
| OSC | 不能（块顺序、可选 `if-unused`） | 语义等价 + 服务器视角模拟（mock API 校验 diff 合法性） |
| `.jos`/`.joz` | `.joz` 不能（zip 时间戳/压缩级别） | 解压后 XML 语义等价；`joz` 只断言"可被自己读回" |
| GPX | 部分 | 语义等价 + 保留未知扩展元素（extras 机制） |

**未知字段保留**是往返保真的隐形前提：XML 的 JOSM 私有 `action` 属性、PBF 的 `optional_features`、以及未来版本的新属性，都必须进 `extras` 并原样写回。测试手段：给 fixture 人为注入未知属性/未知元素，断言往返后仍在（`unknown-field-preservation` 测试族）。

### 2.5 E2E UI 测试（egui kittest）

不追求"点击坐标"，而是**语义交互**：按控件标签/`AccessKit` 角色查找、按键序列输入、断言状态。

| 用例族 | 例 |
|---|---|
| 面板可见性 | `Tab` 循环停靠面板；`Alt+Shift+V` 打开验证器面板（[13](./13-josm-parity.md) §3.1） |
| 双键序列 | `A` 后 300 ms 内再 `A` → 角度捕捉开启；超过窗口 → 仍是普通 Draw |
| 撤销可见性 | 执行 3 次编辑 → `Ctrl+Z`×3 → 面板状态与空数据集一致 |
| 冲突解决 | 注入 409 → 冲突面板四维度全绿前 `Apply` 禁用 → 解决后可点 |
| 快捷键重绑 | 改绑 `Ctrl+Shift+P` → 旧键失效、新键生效，重启后保留 |

egui/eframe 版本以 [02](./02-workspace-and-dependency-rules.md) 的锁定表为唯一事实来源，本文不重复声明版本号。

### 2.6 GPU 图像测试

```
fixture ─► scene::build ─► renderer.encode ─► texture ─► readback ─► PNG（离屏，无窗口/swapchain）
                                                                        │
              dHash 8×8（64 bit）与基线汉明距离 ≤ 3 ◄───────────────────┤
              不一致时落盘 out/cargo/gpu-diff/<case>/{new,base,diff}.png ◄─┘
```

| 项 | 决策 |
|---|---|
| 适配器 | 优先真实 GPU；无硬件时用软件适配器（Windows **WARP**、Linux **lavapipe/lavapipe**）；macOS 无软件 Metal 后端 → 该平台 GPU 用例 **skip 并标记**（不许静默通过） |
| 跳过策略 | 环境变量 `NEO_GPU_TEST=0` 或适配器探测失败 → `#[ignore]` 语义 + 明确的 skip 报告；CI 汇总里 skip 数必须可见 |
| 感知哈希 | dHash 64 bit + 汉明阈值 3；另留一个"结构校验"：图元包围盒/顶点数/count 断言（比像素更早失败、更好定位） |
| 不可比因素 | 驱动差异、MSAA 采样位置、字体栅格化 → 全部由"感知哈希 + 结构断言"吸收，不做逐像素比对 |
| 拾取一致性 | 关键补充：`resolve_pick` 在 GPU 回读与 CPU 侧索引结果必须一致（这项比图像更能抓真 bug） |

### 2.7 fuzz / 并发 / 压力

| 专项 | 目标 | 配置 | 判据 |
|---|---|---|---|
| fuzz | `fuzz_pbf`、`fuzz_xml`、`fuzz_osc`、`fuzz_mapcss`、`fuzz_preset` | `cargo-fuzz`，语料由 fixtures 提取 + 手工种子；nightly 每目标 `-max_total_time=600` | 任何 panic/OOM/无限循环即失败；崩溃自动存 `artifacts/` 并开 issue |
| loom | journal 追加与快照发布的顺序、`arc-swap` 发布点、对象池 | 仅对小模型（`loom::sync` 包装的 3–5 个结构） | 所有交错下满足 INV-1/INV-6/INV-7 |
| 压力 | 10M 对象 + 8 读线程 + 1 写者，30 min | 自托管 runner | 无 panic/死锁；RSS 增长 ≤ 【待实测，见 docs/14-performance-and-verification.md】 |
| 内存 | `dhat` 3 个场景（导入/编辑/验证） | nightly | 无未释放的持续增长；热点分配登记 |

并发测试的**边界**：`loom` 只覆盖"手写的同步原语与发布路径"，不用它炸整个 `DataSet`（状态空间不可行）。其余并发正确性靠压力测试 + `--full` 对拍（[12](./12-validation.md) §2.4）。

### 2.8 性能回归

| 指标 | 工具 | 稳定性 | 门禁 |
|---|---|---|---|
| 指令数/分支（确定性） | `iai-callgrind` 0.16 | 高（CI 任意机器可比） | PR：回归 > 2% 失败 |
| 墙钟 p50/p95 | `criterion` 0.8 | 低（受邻居/频率影响） | nightly：仅自托管固定机型，回归 > 10% 告警 |
| 分配次数/字节 | `dhat` 0.3 | 高 | nightly：回归 > 15% 告警 |
| 帧时间分解 | `puffin` 0.20 | 中 | 发布前人工复核 |

CI 硬件约束必须写清：共享云 runner **只跑 iai-callgrind**（指令数与机器无关），`criterion` 只在自托管机型（如 i5-12400F，见 [01](./01-system-architecture.md) §2.4）上跑；基线存 `bench/baseline/*.json` 并随依赖升级显式更新（更新需在 PR 描述里写明原因）。所有阈值当前均为【待实测，见 docs/14-performance-and-verification.md】，首次测量后回填本表。

## 3 关键矩阵

### 3.1 归一化规则（golden 的"宪法"）

| 维度 | 规则 | 理由 |
|---|---|---|
| 坐标 | 量化到 **1e-7 度**（= PBF `granularity` 100 nanodeg）；XML 输出固定 7 位小数 | 低于此精度的差异无意义，且 PBF 本身不可表达 |
| 投影/浮点 | 相对误差 ≤ 1e-9；比较用 `approx` 而非 `==`；输出按 `%.12e` 格式化 | GPU/CPU 与不同 FMA 路径会带来 ULP 级差异 |
| 元素顺序 | 按 `(ObjectType, id)` 排序；**负 id 归一化为 `(type, creation_index)`** 并按创建顺序编号 | 负 id 是占位符，其数值本身无意义，但创建顺序有意义 |
| way 节点顺序 | **不排序** | 方向是数据（`oneway`、海岸线、环礁） |
| relation 成员顺序 | **不排序** | 顺序是数据（route/multipolygon），见 [13](./13-josm-parity.md) §3.6 第 2 条 |
| tag 顺序 | key 字典序（`BTreeMap`） | 消除 HashMap 迭代顺序（INV-E） |
| 属性省略 | `version/timestamp/uid/user/changeset` 默认归一为占位；仅当用例断言它们时保留 | 避免夹具时间戳污染哈希 |
| 时间戳 | 由 `FixedClock` 注入；序列化时统一 `1970-01-01T00:00:00Z` + `clock_offset` | 绝对时间不可比 |
| 空白/属性顺序（XML） | 解析后比较，不做文本 diff；写出时属性按固定顺序 | 文本 diff 会把格式化差异当 bug |
| 压缩 | 先解压再比较（PBF/zlib、`.joz` zip） | 压缩器版本差异 |
| 浮点渲染参数 | 缩放/视口/DPR 固定为用例声明值 | 否则图像不可比 |

### 3.2 测试数据三档

| 档 | 规模 | 来源 | 存放 | 用途 |
|---|---|---|---|---|
| S | 100K 对象（约 10 MB PBF） | 生成器（参数化：密度/对象类型比例/relation 深度） | git（小） | PR 门禁、L1–L4 |
| M | 1M 对象（约 100–150 MB） | 生成器 + **真实 Monaco 提取** | 下载脚本（sha256 校验），不入 git | nightly、L5/L6、性能 |
| L | 10M 对象（约 1.5 GB） | 生成器（含 10 万 relation、极密路网） | 自托管 runner 本地缓存 | nightly 性能、压力 |

- **生成器**：`scripts/gen-fixture --objects N --seed S --profile {city,rural,multipolygon-heavy,pt-routes}`，输出确定性（固定 seed → 相同字节），并有自测（`gen(seed)` 两次结果相等）。
- **Monaco 提取**：用 Geofabrik 的 `monaco-latest.osm.pbf`（真实数据、体积小、含海岸线/relation/多语言 name），**只下载不提交**，ODbL 署名写入 `tests/fixtures/NOTICE`，测试运行时打印署名。
- 合成数据必须覆盖"坏数据"：自交 way、重复节点、未闭合 area、环形 relation、超长 way（2000 节点边界）、负 id 混合、`visible=false`——否则验证器与上传路径永远测不到边界。

### 3.3 CI 矩阵

| 平台 | GPU 后端 | PR 内容 | nightly 内容 | 备注 |
|---|---|---|---|---|
| Windows x64 | D3D12 真机 / **WARP** | L1–L5、L6(WARP)、iai | + L7 墙钟（自托管）、压力、fuzz | WARP 让 PR 也能跑图像测试 |
| Linux x64 | Vulkan **lavapipe** | L1–L6(lavapipe)、iai | + fuzz、loom、压力 | 容器内无需 GPU 设备 |
| macOS arm64 | Metal | L1–L5、iai | + L7 | 无软件适配器 → L6 **skip 并标记** |
| 交叉 | — | `cargo check` 全 target；`cargo-deny`；`reuse lint` | 同左 | 门禁与非门禁分离 |

PR 门禁总时长预算 ≤ 15 分钟：L1–L4 并行分片（按 crate），L5/L6 单机顺序跑，`--full` 对拍只在 nightly。

### 3.4 失败可诊断性要求

| 层 | 失败时必须输出 |
|---|---|
| golden | 不一致项的人类可读 diff（前 20 行），并保存完整产物到 `out/cargo/golden-fail/<case>/` |
| 属性 | `proptest` 的收缩后最小反例 + 随机种子（可直接复现） |
| GPU | `new.png` / `base.png` / `diff.png` 三件套 + 汉明距离 + 结构断言差异 |
| 集成 | mock API 的完整请求/响应记录（含 OSC 请求体） |
| 性能 | 基线 vs 当前的中位数/偏移量，以及 profile 摘要路径 |
| fuzz | 最小化后的输入文件路径 + 复现命令 |

## 4 流程与时序

### 4.1 PR 流水线

```
push ─► fmt/clippy ─┬─► L1 单元（分片并行）
                    ├─► L2 属性（分片并行，种子固定）
                    ├─► L3 集成（含 mock API 409/413/429 场景）
                    ├─► L4 golden（对象集/OSC/报告）
                    ├─► L5 kittest（UI 语义）
                    ├─► L6 GPU（WARP/lavapipe 或 skip）
                    ├─► iai-callgrind（确定性性能门禁 >2%）
                    ├─► cargo-deny / reuse lint / DCO
                    └─► 覆盖率（报告不设阈值，仅趋势）
                            ▼
                        合并（≤15 min）
```

### 4.2 nightly 与发布前

```
nightly:
  ├─ L7 criterion（自托管机型）+ dhat + puffin 摘要
  ├─ fuzz 5 目标 × 10 min
  ├─ loom（全部模型）+ 10M 压力 30 min
  ├─ M/L 档夹具的 golden 与增量验证 `--full` 对拍（[12](./12-validation.md) §2.4）
  └─ JOSM 双跑 oracle（render/validate/project，黑盒 CLI）

release 前（人工 + 脚本）:
  ├─ 全平台 L1–L6 全绿（macOS L6 允许 skip，需记录）
  ├─ baseline 更新评审（每条变化写明原因）
  ├─ fuzz 累计 24 h 无新崩溃
  └─ 一周工作流脚本（[13](./13-josm-parity.md) §4.1）跑通并归档日志
```

### 4.3 golden 更新流程（防止"改测试就绿"）

```
行为变更 → 跑测试 → golden 不一致
   ├─ 变更是有意的？ → 生成新产物 → diff 评审（必须两人）→ 更新 .sha256 + 人类快照 → PR 说明写清"为什么行为变了"
   └─ 变更非有意 → 修实现
```

golden 文件的更新必须与**行为变更同一 PR**，且 diff 要人可读；禁止批量 `--bless` 一把提交（CI 对单 PR 修改 golden 数量设上限，例如 > 20 需额外批准）。

## 5 权衡与替代方案

| 决策 | 选择 | 被否决项 | 理由 / 代价 |
|---|---|---|---|
| 等价性证据 | golden + 归一化哈希 | 逐字段断言每个用例 | 逐字段用例数爆炸且难维护；代价是失败定位需要人类快照与 diff 工具 |
| 与 JOSM 对比 | 黑盒 CLI 双跑（nightly） | 引入 JOSM 作为测试依赖并链接其代码 | 链接会传染许可且技术上不可行（JVM）；代价是对拍覆盖受 JOSM CLI 能力限制 |
| PBF 往返 | 语义等价 | 字节级往返 | 字节级会把压缩器差异当 bug；代价是"格式保真"只能证明到语义层（用未知字段保留测试补强） |
| GPU 测试 | 感知哈希 + 结构断言 | 逐像素比对 | 逐像素在跨驱动时必然 flaky；代价是可能漏掉细小视觉回归（用结构断言兜底） |
| 性能门禁 | PR 用 iai（指令数），墙钟只在固定机型 | 全 CI 跑 criterion | 云 runner 的墙钟噪声会让门禁失去意义；代价是真机覆盖窄 |
| 并发验证 | loom 限定小模型 + 压力 | 全量 loom | 状态空间不可行；代价是巨型结构只能靠压力与对拍 |
| 夹具 | 生成器为主 + Monaco 真实数据 | 全用真实数据 | 真实数据不可复现、体积大、许可复杂；代价是合成数据可能不覆盖真实怪例（用"坏数据"参数补） |

## 6 风险

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| T1 | golden 被当作"改一下就绿"的开关 | 等价性证据失效 | 更新需双人评审 + 数量上限 + PR 说明模板 |
| T2 | CI 上 GPU 永远 skip → 图像层形同虚设 | 渲染回归漏到最后 | 至少 WARP/lavapipe 一条软件路径必须在 PR 跑；skip 数纳入健康度看板 |
| T3 | 归一化过宽（把真差异吸收掉） | 假绿 | 归一化规则是评审对象（§3.1 有版本号）；每条规则附带"它能吸收什么、不能吸收什么" |
| T4 | 10M 夹具不在 CI → 大规模回归发现晚 | 大对象数下的性能/正确性问题 | nightly 自托管跑；另用 1M 夹具在 PR 跑一次"迷你规模"版本 |
| T5 | fuzz 语料枯竭 | 覆盖率停滞 | 从真实夹具与差分测试（新旧解析器对拍）生成种子；覆盖率定期审查 |
| T6 | JOSM CLI 输出随版本漂移 | 双跑误报 | 固定 JOSM 版本（r19678）+ 结果按版本存档；升级 JOSM 时单独立 PR |
| T7 | 属性测试的生成器塌缩成"只生成合法数据" | 漏掉非法输入路径 | 必须有一组"故意非法"的生成策略（负 id、悬空引用、超限 way） |

## 7 验收标准

1. **不变量全绿**：§2.2 的 INV-A…INV-K 各至少一个属性测试，`--proptest-cases=10000` 下 0 失败，反例可复现。
2. **往返保真**：S/M 档夹具在 XML、PBF、OSC 三条路径上往返语义等价；未知字段保留测试通过。
3. **golden 覆盖**：parity 矩阵 52 条 P0 每条至少一个 golden 或集成用例；缺失条目由脚本统计并阻断发布。
4. **JOSM 对拍**：nightly 的 `render`/`validate`/`project` 三项对拍在归一化后差异为 0（渲染按感知哈希阈值）；差异要么修，要么登记进已知偏差表并附理由。
5. **UI**：60+ kittest 用例覆盖双键序列、冲突面板门控、快捷键重绑。
6. **GPU**：WARP 与 lavapipe 两条软件路径在 CI 通过；macOS skip 明确可见（不是静默跳过）。
7. **性能**：iai 回归 > 2% 阻断；墙钟与内存基线首次测量后回填 §2.8，此后每季度复核硬件漂移。
8. **fuzz**：五个目标累计 24 h 无崩溃、无 OOM、无超时；语料入库。
9. **流水线时长**：PR 门禁 p95 ≤ 15 分钟，nightly ≤ 90 分钟（超出需优化而不是放宽超时）。

## 8 参考

- `proptest`（收缩与回归文件语义）· `cargo-fuzz`（libFuzzer 前端）· `loom`（并发交错模型检查）：https://docs.rs/proptest/latest/proptest/ · https://rust-fuzz.github.io/book/cargo-fuzz.html · https://docs.rs/loom/latest/loom/
- `criterion` 0.8（统计与基线）· `iai-callgrind` 0.16（指令数基准，确定性）：https://docs.rs/criterion/0.8.0/criterion/ · https://docs.rs/iai-callgrind/0.16.0/iai_callgrind/
- `dhat` 0.3（分配剖析）· `puffin` 0.20（帧剖析）· `egui_kittest`（UI 语义测试）：https://docs.rs/dhat/0.3.0/dhat/ · https://docs.rs/puffin/0.20.0/puffin/ · https://docs.rs/egui_kittest/latest/egui_kittest/
- wgpu 离屏渲染与 `readback`（`ImageCopyTexture`/`map_async`）：https://docs.rs/wgpu/30.0.2/wgpu/
- Windows WARP（D3D12 软件适配器）：https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/warp
- Mesa lavapipe（软件 Vulkan）：https://docs.mesa3d.org/drivers/llvmpipe.html
- OSM PBF 格式（granularity / DenseNodes，决定归一化精度）：https://wiki.openstreetmap.org/wiki/PBF_Format
- OsmChange 与 `if-unused`：https://wiki.openstreetmap.org/wiki/OsmChange
- Geofabrik Monaco 提取（真实小夹具，ODbL）：https://download.geofabrik.de/europe/monaco.html
- OSM 版权与署名（夹具分发义务）：https://www.openstreetmap.org/copyright
- JOSM CLI（`render`/`project`/`validate`，作为黑盒 oracle）：https://josm.openstreetmap.de/wiki/Help/CommandLineOptions
  > 待验证：`validate` 子命令的选项表，一手文档仅给出用法块。
- 一手事实基础（JOSM r19678、40 个验证测试类、autosave/会话默认值）：[`.research/josm-inventory.md`](../.research/josm-inventory.md)
