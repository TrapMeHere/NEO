# 12 · 验证器架构与规则体系（Validation）

**摘要**：NEO 的验证器不是"移植 JOSM 的 40 个 Java Test 类"，而是一个**以对象脏集（dirty set）为输入、以可撤销修复命令为输出的并行规则引擎**。三类消费者共用一套规则描述：内置 Rust 规则（需要空间索引与跨对象图）、`*.validator.mapcss` 兼容层（社区可写的声明式规则）、上传前的服务器能力预检。`ValidationReport` 是 UI 面板、上传 gate 与 golden 测试的唯一事实来源；每条自动修复都必须表达为 `neo-editor` 的 `EditCommand` 批次，从而天然可撤销。规则 P0/P1/P2 划分与 JOSM 的 12 个核心 `.validator.mapcss` + 40 个 Java 测试并集对齐（§3.2–3.3）。行为契约见 [13](./13-josm-parity.md)，撤销模型见 [06](./06-editing-core-and-undo.md)，性能数字一律【待实测，见 docs/14-performance-and-verification.md】。

## 0 阶段 4 已实现（v0.1.0）

`neo-validate` 当前 12 条规则（JOSM 风格语义，行为对齐而非拷贝代码）：

| 规则 id | 级别 | 说明 |
|---|---|---|
| `way_too_short` | Error | way 少于 2 节点 |
| `way_duplicate_node` | Warning | 相邻重复节点 |
| `way_untagged` | Warning | way 无标签 |
| `building_not_closed` | Error | building way 未闭合 |
| `duplicate_way` | Warning | 两条 way 节点序列相同 |
| `relation_empty` | Warning | relation 无标签 |
| `relation_no_members` | Error | relation 无成员 |
| `untagged_node` | Info | 孤立且无标签的节点 |
| `duplicate_node` | Warning | 坐标完全相同的节点 |
| `suspicious_tag_combination` | Warning | highway+building / natural=water+building / highway+railway |
| `multipolygon_bad_role` | Warning | multipolygon 成员角色非 outer/inner/空 |
| `self_intersecting_way` | Error | way 自相交（≤2048 节点 O(n²)） |

未实现（阶段 4 后续）：almost-junction、crossing ways、地址/名称 tag 规则、MapCSS validator、
上传前 gate、增量校验（按 dirty/bbox）。参考 JOSM Validator 的规则语义与测试数据，
不复制其 GPL 代码。

## 1 目标与范围

**在范围内**

| 项 | 要求 |
|---|---|
| 规则执行 | 1M–10M 对象下，编辑后增量验证只重算受影响的 规则×目标，不重启全量扫描 |
| 规则来源 | 内置 Rust 规则 + 未修改的 JOSM core `.validator.mapcss`（12 个文件）子集兼容 |
| 报告 | 结构化、可定位（对象+子元素）、可去重、可抑制、可导出 |
| 修复 | 每条 fixable issue 生成一个或多个 `EditCommand`，合并为单次撤销步骤 |
| 上传 gate | error 阻断、warning 需确认；顺序与 JOSM 的 upload hook 链一致 |
| 抑制 | 按 规则×对象 / 规则×类 / 数据集 三级持久化，跨会话与 autosave 恢复 |

**不在范围内**：JOSM Java 测试的代码搬迁（`data/validation/tests/*.java` 是 GPL-2.0+，只能作行为清单阅读，见 [17](./17-licensing-and-governance.md) §2.2）；服务端校验与 Osmose/KeepRight 规则合并（只消费可离线判定的部分）；完整 MapCSS 渲染级联（那是 `neo-style`）；"零误报"——目标是**同一输入产生同一结论集合**。

范围边界上的两个判断：① **不追求规则数量对等，追求结论对等**——用 12 个核心 mapcss 的 `assertMatch`/`assertNoMatch` 当可执行兼容断言（它们是数据，不是代码），比"实现了 N 条规则"可验证得多；② **误报率是产品指标**——抑制不是补丁而是核心功能，用户一旦关掉验证器，parity 在实践中就失效。

## 2 设计

### 2.1 数据流

```
用户编辑 ─► neo-editor: EditTx ─► Journal(ObjectId+version) ─► Snapshot v(n)（arc-swap 发布）
                                     │
                                     ▼
   RuleScheduler（UI 线程，O(脏集)）：①依赖展开 object→(rule,target) ②目标展开 node→way→rel(≤2 跳)
                                      ③空间展开 旧/新坐标 bbox⊕radius → rstar 邻域 ④预算判定（脏>30%→全量）
                                     ▼
   neo-validate::Engine（rayon par_iter，只读 &Snapshot）：每线程 IssueBuf → 排序 → reduce → 去重 → ReportStore
                                     ▼
   ValidatorDialog（过滤/定位/修复） · 上传 gate（block/warn） · golden 测试 · 抑制持久化(neo-persist)
```

### 2.2 规则 trait 与调度单元

规则是**纯函数 + 显式依赖声明**：`deps()` 是增量正确性的唯一依据，未声明依赖却读取别处数据是评审红线。

```rust
pub trait ValidationRule: Send + Sync {
    fn id(&self) -> RuleId;                 // 稳定 ID，进 suppression 与 golden，发布后不得改名
    fn scope(&self) -> Scope;               // Object | Reference | Global
    fn deps(&self) -> Deps;                 // 决定脏集如何展开、规则能否并行
    fn default_severity(&self) -> Severity; // Error | Warning | Other
    fn check(&self, cx: &RuleCtx<'_>, target: Target, out: &mut IssueBuf);
    fn prepare(&self, cx: &RuleCtx<'_>) -> Option<Box<dyn RuleIndex>> { None } // 两阶段：重复检测/海岸线
    fn fix(&self, cx: &RuleCtx<'_>, issue: &Issue) -> Option<FixPlan> { None }
}

pub enum Scope { Object, Reference, Global }
pub struct Deps { pub kind: DepKind, pub radius_m: f64, pub tag_keys: &'static [&'static str] }
// DepKind: None | NodesOfWay | MembersOfRelation | Closure | Neighborhood

pub struct RuleCtx<'a> {
    pub snapshot: &'a Snapshot,               // 只读，INV-2
    pub geo: &'a GeoIndex,                    // rstar 0.14 空间索引（neo-geo）
    pub presets: &'a PresetIndex,             // 预编译 match 表达式
    pub tags: &'a TagRuleIndex,               // mapcss 兼容层的选择器索引
    pub territory: Option<&'a TerritoryIndex>,// [inside("XX")] 依赖的边界
    pub config: &'a ValidatorConfig,
}
```

`RuleId` 与 `Issue.code` 分离：前者标识实现（可为 mapcss 文件+行号），后者是用户可见稳定短码（`self_intersection`），用于抑制与文档映射。

### 2.3 四类规则与执行顺序

顺序由**代价与依赖**决定，不由严重级决定：

| 阶段 | 分类 | 典型规则 | 复杂度 | 增量友好度 |
|---|---|---|---|---|
| P1 | 单对象几何 | 自交、重复节点、退化 way、area 语义 | O(n) | 高（只依赖自身） |
| P2 | 单对象 tag | 互斥、废弃、数值、必填 | O(tags) | 高 |
| P3 | 引用（1 跳） | way→nodes、relation→members | O(成员数) | 中（成员变更需传播） |
| P4 | 闭包/全局 | multipolygon 环、重复检测、海岸线 | O(闭包)/O(n) | 低（需索引失效） |
| P5 | 空间邻域 | 交叉无节点、悬空端点、重叠 way | O(k·log n) | 中（要查旧/新位置） |
| P6 | 上传预检 | 服务器能力、changeset 上限 | O(上传集) | 不需要（每次上传重算） |

同阶段内按 `RuleId` 字典序执行以保证输出确定性（golden 要求）；跨阶段不并行（P4 索引构建需要 P1–P3 结束后的一致性视图）。

### 2.4 增量验证：反向依赖索引

```
dirty(v) = {node:42, way:17}                     ← Journal 事务对象集
 ├ 直接目标：node:42 → [T03(邻域), K15]；way:17 → [T01,T02,G04,K02]
 ├ 引用传播：node:42 ∈ way:17, way:31 → 目标 += way:17,31；way:17 ∈ rel:9 → 目标 += rel:9
 └ 空间传播：bbox(old ∪ new) ⊕ radius → rstar 查询 → way:88, node:99 → 邻域规则 T11/T09/T10
```

1. **传播跳数有限**：`node→way→relation` 默认 2 跳；更远的（relation 的成员 relation）归 `Scope::Global` 按 P4 重算，避免传播爆炸。
2. **空间传播用旧/新坐标并集**：只查新位置会漏掉"移走后留下的空隙"（相邻 way 原本连通，结点移走后变悬空）。
3. **降级而非卡死**：脏目标比例 > 阈值（默认 30%）或单次 pass 预计 > 50 ms 即切全量，记 `ValidationStats::degraded`；全量耗时【待实测，见 docs/14-performance-and-verification.md】。

防抖：拖动以 120–200 ms 窗口合并脏集，拖动过程中不重建报告，只在事务边界（松手）后触发。

### 2.5 抑制（suppression）持久化

JOSM 是"per-test ignore list"，其 preference key 未核实（> 待验证，见 [`.research/josm-inventory.md`](../.research/josm-inventory.md) Appendix B.4）。NEO 不复刻其存储格式，只复刻用户可见语义，并额外解决"抑制后数据变了仍静默"：

```rust
pub struct SuppressionKey { pub rule: RuleId, pub target: ObjectKey, pub digest: u64, pub scope: SuppressScope }
// digest = 相关 tag 子集(Deps.tag_keys) + 几何类(闭合/节点数/方向)；不含坐标与时间戳
```

| 属性 | 语义 | 理由 |
|---|---|---|
| `digest` | 只哈希声明的 key 与几何类 | 拖节点不应让抑制失效，改 tag 应当失效 |
| `Object` | 仅这一条 | "这个我知道"（≈ ignore for now） |
| `Class` | 该规则对该对象类型全部静默 | "我不关心 `created_by` 的 deprecated" |
| `Dataset` | 该规则本数据集全静默（导入遗留数据） | 但报告头部须显示"已抑制 N 条" |
| 存储 | `neo-persist` SQLite `validation_suppression(dataset_fp, rule, type, id, digest, scope, created_at)` | 与 autosave 同库同事务域 |
| 可观测 | 报告含全量 issue，抑制只在**呈现与 gate** 层过滤 | 避免"抑制导致漏看 error" |

导出/导入为 JSON（`neo-io::issues`）便于换机与回归复现；**不写入 `.jos`**（会话文件不承载治理状态）。

### 2.6 报告模型、定位与自动修复

```rust
pub struct ValidationReport {
    pub generation: u64,                    // = Snapshot.version，UI 据此判断是否过期
    pub stats: ValidationStats,             // 规则数/目标数/耗时/是否降级/已抑制数
    pub issues: Arc<[Issue]>,               // 稳定排序：severity desc, code, objects
    pub by_object: HashMap<ObjectKey, SmallVec<[IssueIdx; 4]>>,
    pub counts: Counts,                     // { error, warning, other, suppressed }
}
pub struct Issue {
    pub rule: RuleId, pub code: &'static str, pub severity: Severity,
    pub objects: SmallVec<[ObjectKey; 4]>,  // 首个为主要对象
    pub locate: Locate,                     // Node | WaySegment(u32) | Member(u32)
    pub message: String, pub fixable: bool,
}
pub struct FixPlan { pub steps: Vec<Box<dyn EditCommand>>, pub label: String }
```

定位（`Locate`）是跳转的前提：`Zoom to problem` 要把 `WaySegment(3)` 变成视口中心与高亮段，`Member(7)` 要选中 Relations 面板第 8 行——只给 object id 无法实现"下一个问题"（`6` 跳转、`8`/`9` 前后切换，见 [13](./13-josm-parity.md) §3.1）。

| 修复约束 | 内容 |
|---|---|
| 单步撤销 | `Editor::execute_batch(steps)` 产生一个用户可见撤销步骤；撤销后数据逐字段等于修复前 |
| 幂等 | 修复后立即重跑同规则于同目标，不得再产出同一 `code`（`FIX-IDEMPOTENT` 不变量） |
| 预览 | UI 先在克隆的 `DataSetMut` 上 `dry_run` 展示 diff，再提交 |
| 批量 | "修复全部同类"= 多目标 FixPlan 的确定性合并（按 object id 排序） |
| 禁止 | 修复不得创建新对象种类（如自动建 relation），这类只给建议文本 |

### 2.7 上传前验证 gate

对应 JOSM 的 hook 链（`ApiPreconditionChecker → DiscardTags → FixData → ValidateUpload → RelationUploadOrder`）：

```
① 能力预检(S04)：way 节点 ≤2000、relation 成员 ≤32000、changeset 元素 ≤10000、bbox ≤0.25°² → 不满足即 error，不发请求
② 丢弃 tag（DiscardTags 语义）：created_by 等，只影响发出的 OSC，不改本地数据
③ FixData 语义：占位符 id 补齐、空 way 移除
④ 验证 gate：作用域 = 上传集及其 1 跳邻居（不是全数据集）→ error 阻断；warning 弹确认框（记住本次选择）
⑤ relation 上传顺序（neo-api 负责，非验证）→ ⑥ POST /api/0.6/changeset/#id/upload
```

gate 只验**上传集**是刻意的：全数据集 error 不应阻止局部上传，否则 10M 对象下永远无法上传。但"上传集会引入的新错误"必须算——删除某 way 后其父 relation 变空（P3/P4 对上传集闭包重算）。

## 3 关键接口与矩阵

### 3.1 MapCSS 兼容子集 vs 原生 Rust 规则

| 语句 / 选择器 | NEO 支持 | 说明 |
|---|---|---|
| `throwError`/`throwWarning`/`throwOther` | ✅ | 每选择器恰好一个，多则解析报错 |
| `fixAdd:"k=v"`/`fixRemove:"k"`/`fixChangeKey:"old=>new"`/`fixDeleteObject:this` | ✅ | 映射 `ChangePropertyCommand`/`DeleteCommand`（需查引用） |
| `suggestAlternative`/`group` | ✅ | group 影响面板分组，不影响 gate |
| `assertMatch`/`assertNoMatch` | ✅ 作为测试 | 解析为 golden 断言语料，`cargo test -p neo-validate` 执行 |
| 占位符 `{0.tag}` `{i.key}` `{i.value}` | ✅ | 含"伪类计入序号"语义（`:unconnected`=0、`:in-downloaded-area`=1） |
| 类型选择器 `node/way/relation/area/*` | ✅ | `area` 保留 JOSM"含未闭合 way"的反直觉语义 |
| 条件 `= != < > <= >= ^= $= *= ~= =~ !~` | ✅ | `~=` 分号列表成员、正则 `/…/` |
| 集合/空间 `∈ ⊆ ⊇ ⊈ ⊉ ⧉` / `[inside("FR")]` / `outside(...)` | ✅（`⧉` 需 `layer`；领土需 `boundaries.osm`） | 领土索引缺失 → 该类规则整体 skip 并记日志 |
| 伪类 `:closed :new :tagged :unconnected :connection` 等 | ⚠️ 子集 | `:clockwise/:anticlockwise/:open_end` P1；`:area-style` 不适用 |
| 子选择器 `relation[type=route] >[role="link"] way` | ✅ P1 | 需成员图求值 |
| `@supports (min-josm-version:N)` | ⚠️ 映射为 NEO 版本门 | `@media` 已废弃，不实现 |
| `eval()`/任意表达式、`setting()`、`prop("_match")` | ❌ | 避免嵌入表达式求值器 |

**分工原则**："单对象 tag 组合即可判定"的一律走 mapcss（社区可维护、可热重载、可来自 wiki 的 `Rules/*`）；需要空间索引、图遍历、闭包分析或跨对象状态的一律用 Rust 规则。二者产出同一 `Issue` 类型，UI 不区分来源。

### 3.2 拓扑与几何规则清单（NEO 规则 ID）

级别：`E`=Error（阻断上传）· `W`=Warning（需确认）· `O`=Other（仅列出）。优先级为交付顺序，非严重级。

| ID | 检测 | JOSM 对应 | 级别 | 增量范围 | 修复 | P |
|---|---|---|---|---|---|---|
| VAL-T01 | way 自交 | `SelfIntersectingWay`/geometry | E | 单 way | 提示在交点分割（两步） | P0 |
| VAL-T02 | way 内重复节点 | `DuplicatedWayNodes` | E | 单 way | 去重节点，保留 tag | P0 |
| VAL-T03 | 同坐标重复节点 | `DuplicateNode` | W | 邻域 ~1e-7° | 合并节点并重定向引用 | P1 |
| VAL-T04 | area 未闭合 | `UnclosedWays`/geometry | W | 单 way | 追加首节点闭合 | P0 |
| VAL-T05 | 完全重复 way/relation | `DuplicateWay`/`DuplicateRelation` | W | 全量哈希桶 | 合并或删除其一 | P1 |
| VAL-T06 | relation 成员缺失/未加载 | `RelationChecker` | E | 单 relation+成员 | 触发"下载不完整成员" | P0 |
| VAL-T07 | 成员重复/角色非法 | relation.mapcss/`RelationChecker` | W | 单 relation | 去重成员 | P1 |
| VAL-T08 | multipolygon 环不闭合或嵌套错 | `MultipolygonTest`/`CycleDetector` | E | relation 闭包 | 重建成员顺序 | P0 |
| VAL-T09 | 悬空端点 | `UnconnectedWays`/`:unconnected` | W | 邻域 | 无（仅提示连接） | P1 |
| VAL-T10 | way 重叠 | `OverlappingWays` | W | 邻域 | 无 | P2 |
| VAL-T11 | 交叉但无共享节点 | `CrossingWays` | W | 邻域 | 在交点插入共享节点 | P0 |
| VAL-T12 | 海岸线不连续 | `Coastlines` | E | 全量有序链 | 无 | P1 |
| VAL-T13 | way 接入 area 内部 | `WayConnectedToArea` | W | 邻域 | 无 | P2 |
| VAL-G01 | 尖角 | `SharpAngles` | W | 单 way | 无 | P2 |
| VAL-G02 | 超长线段 | `LongSegment` | W | 单 way | 无 | P2 |
| VAL-G03 | 建筑非直角 | `RightAngleBuildingTest` | W | 单 way | 局部 orthogonalize | P1 |
| VAL-G04 | area 语义与闭合/方向矛盾（`area=no`、闭合但线性） | geometry.mapcss | W | 单 way | 增删 `area` tag | P0 |
| VAL-G05 | 退化几何（<2 节点、零长 way） | —（JOSM 分散） | E | 单 way | 删除 | P0 |
| VAL-G06 | 坐标越出下载 bbox | `:in-downloaded-area` | W | 单对象 | 无 | P1 |
| VAL-G07 | 环方向（outer/inner）反向 | `:clockwise` 系 | W | relation 闭包 | 反转 way | P1 |

### 3.3 tag 与语义规则清单

| ID | 检测 | JOSM 对应 | 级别 | 增量范围 | 修复 | P |
|---|---|---|---|---|---|---|
| VAL-K01 | presets 必填 key 缺失 | `TagChecker`/unnecessary | W | 单对象+presets | 无（打开 preset） | P0 |
| VAL-K02 | 互斥 tag 组合 | `combinations.mapcss` | E | 单对象 | 移除一侧 | P0 |
| VAL-K03 | 废弃 tag | `deprecated.mapcss` | W | 单对象 | `fixChangeKey`/`fixAdd` | P0 |
| VAL-K04 | 数值/单位格式 | `numeric.mapcss` | W | 单对象 | 归一化（`50 km/h`→`50`） | P1 |
| VAL-K05 | 分号多值重复/空段 | `multiple.mapcss` | W | 单对象 | 去重 | P1 |
| VAL-K06 | 冗余/隐含 tag | `unnecessary.mapcss` | O | 单对象 | 删除 | P2 |
| VAL-K07 | wikipedia/wikidata 链接格式 | `wikipedia.mapcss` | W | 单对象 | 补语言前缀 | P1 |
| VAL-K08 | 地址不完整（housenumber/street 互依） | `addresses.mapcss` | W | 单对象 | 无 | P0 |
| VAL-K09 | `conditional:*` 语法 | `ConditionalKeys` | O | 单对象 | 无 | P2 |
| VAL-K10 | `opening_hours` 语法 | `OpeningHourTest` | O | 单对象 | 无 | P2 |
| VAL-K11 | lanes/`turn:lanes` 数量一致 | `Lanes`/highway.mapcss | W | 单 way | 无 | P2 |
| VAL-K12 | name 与 `name:xx` 不一致、相似名 | `NameMismatch`/`SimilarNamedWays` | O | 邻域 | 无 | P2 |
| VAL-K13 | religion/denomination 组合 | `religion.mapcss` | O | 单对象 | 无 | P2 |
| VAL-K14 | 国家特定规则 `inside("XX")` | `territories.mapcss` | W | 单对象+领土 | 视规则 | P1 |
| VAL-K15 | 未打 tag 的节点 | `UntaggedNode` | W | 单对象 | 删除 | P0 |
| VAL-S01 | highway+building 等语义冲突 | `combinations.mapcss` | E | 单对象 | 无 | P0 |
| VAL-S02 | 转弯限制结构 | `TurnrestrictionTest` | E | 单 relation | 无 | P1 |
| VAL-S03 | 公交线路 schema v2 | `PublicTransportRouteTest`/pt_assistant | W | 单 relation | 无 | P1 |
| VAL-S04 | 上传前服务器能力预检 | `ApiCapabilitiesTest` | E | 上传集 | 无（阻断） | P0 |
| VAL-S05 | 方向性 tag 与 way 方向不一致 | `ReverseWayTagCorrector` 语义 | W | 单 way | 翻转方向性 tag | P1 |
| VAL-S06 | route 成员顺序异常 | `WronglyOrderedWays` | W | 单 relation | 重排成员 | P2 |
| VAL-S07 | 电力线跨接 | `PowerLines` | W | 邻域 | 无 | P2 |
| VAL-S08 | 栏杆/入口组合 | `BarriersEntrances` | W | 邻域 | 无 | P2 |

P0 小计 **15 条**：T01/T02/T04/T06/T08/T11、G04/G05、K01/K02/K03/K08/K15、S01/S04——"关闭验证器就无法安全上传"的最小集；其余 25 条为体验与社区一致性。

## 4 流程与时序

### 4.1 拖动 → 增量验证（帧内不阻塞）

```
UI         Journal      Scheduler                        rayon pool        ReportStore
│ 拖动节点   │            │                                 │                  │
├──────────►│ txn(Move)  │                                 │                  │
│           ├ dirty={node42}（120ms 防抖）                  │                  │
│           ├───────────►│ 依赖展开 → target={node42,way17,way31,rel9,nbhd}
│           │            ├────────────────────────────────►│ par_iter(规则×目标)
│           │            │                                 │ IssueBuf→reduce→sort
│           │            ├─────────────────────────────────┼─────────────────►│ v(n)
│◄──────────┴────────────┤ 报告就绪(generation)             │                  │
│ ValidatorDialog 仅重绘差异行                                              │
```

无脏集时不调度（`dirty.is_empty()` 直接返回），这是空闲态零开销的保证。

### 4.2 修复与撤销

```
用户点"修复" → rule.fix(cx,issue) → FixPlan{steps}
  → Editor::execute_batch(steps)（单事务、单撤销步骤）→ journal 记录（可 autosave/恢复）
  → publish Snapshot(v+1) → Scheduler(dirty=被触达对象) → 报告刷新
  → Ctrl+Z → 数据逐字段回到修复前 → 原 issue 重现
```

### 4.3 上传 gate 判定

```
upload() ─► ①能力预检 ─┬ fail → 阻断，报 S04，不建 changeset
                       └ ok ─► ④gate(上传集闭包) ─┬ error>0 → 阻断（可跳转/修复）
                                                   ├ warning>0 → 确认框（默认取消）
                                                   └ clean → ⑤顺序化 ─► ⑥POST
```

## 5 权衡与替代方案

| 决策 | 选择 | 被否决项 | 理由与代价 |
|---|---|---|---|
| 规则语言 | Rust trait + mapcss 兼容层双轨 | 只做 mapcss（像 JOSM 尽量下推） | 空间/图类规则在 mapcss 里无法表达（JOSM 也只能用 Java 写）；代价是两套测试与文档 |
| 增量模型 | 反向依赖索引 + 空间传播 | 每次全量（简单、永远正确） | 10M 下全量不可接受；代价是索引失效 bug 表现为"漏报"，故必须有 `--full` 与 nightly 全量对拍 |
| 抑制粒度 | 带 `digest` 的三级抑制 | 照抄 per-test ignore list | JOSM 语义细节未核实，且"抑制后数据变了仍静默"是真实痛点；代价是 digest 必须严格限定在 `tag_keys` |
| gate 作用域 | 上传集闭包 | 全数据集 | 否则大范围 error 永久阻塞局部上传；代价是既有错误改由状态栏常驻计数提醒 |
| 修复执行 | 生成 EditCommand 走 Editor | 直接改 store（快） | 绕过 INV-5 就失去撤销与 journal；代价是批量修复要构造大量命令对象（用对象池缓解） |
| 规则来源 | 内置 Rust + 运行时可加载 mapcss | 编译期烘焙全部规则 | 规则是**资源**，涉及 GPL/wiki 许可隔离，必须运行时可替换（[17](./17-licensing-and-governance.md) §2.3） |

## 6 风险

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| R1 | 增量依赖漏声明 → 静默漏报 | 上传后才发现问题（最坏情形） | `deps()` 必填 + "故意改 tag/坐标必须触发重算"的变异测试 + nightly 全量对拍 |
| R2 | mapcss 兼容层语义偏差（伪类序号、`area` 选择器） | 同一规则不同结论 | 用 12 个核心文件的 assert 做语料；差异进已知偏差表而非"修 bug" |
| R3 | 抑制隐藏 error | 数据损坏 | 抑制只在呈现层过滤，报告保留全量；gate 永远看未抑制 issue |
| R4 | 规则热重载期报告不一致 | UI 闪烁、误点修复 | 重载生成新 `generation`，旧报告只读；重载失败保留上一版 |
| R5 | territories 依赖 `boundaries.osm`（GPL/ODbL 混合） | 许可与体积 | 走运行时资产目录且可缺失；缺失记 skip 而非报错（[17](./17-licensing-and-governance.md) §2.3） |
| R6 | 全量 pass 在 10M 上超帧预算 | 卡顿 | 全量只在 idle/上传前/显式请求时后台执行，进度可取消 |

## 7 验收标准

1. **结论对等**：12 个核心 `.validator.mapcss` 中所有 `assertMatch`/`assertNoMatch` 全部通过，差异登记进已知偏差表且每条有理由。
2. **增量正确性**：1M 对象数据集上随机 10k 次编辑，逐步增量验证的 issue 集合与"每次全量"结果完全相等（按 `(code,对象集)` 归一化比较），0 差异。
3. **确定性**：同一 `Snapshot.version` 上两次验证报告字节级相等（含排序），并发度 1 与 8 时亦相等。
4. **可撤销**：每类 fixable 规则 `修复→撤销` 后数据集逐字段等于修复前，撤销栈深度 +1 而非 +N。
5. **幂等**：修复后重跑同规则不再产出同一 `code`（属性测试 10^4 例）。
6. **抑制**：重启后仍生效；修改被抑制对象的相关 tag 后抑制自动失效并重新告警。
7. **gate**：含 error 的上传不产生任何 HTTP write 请求（mock API 断言零请求）；仅 warning 时需显式确认。
8. **性能**：单节点拖动到报告刷新 p95 【待实测，见 docs/14-performance-and-verification.md】；10M 对象空闲态 CPU < 0.5%（无脏集不调度）。

## 8 参考

- JOSM 验证器 MapCSS 规则语法：https://josm.openstreetmap.de/wiki/Help/Validator/MapCSSTagChecker
- 社区规则集索引（`Rules/*`，首选分发渠道）：https://josm.openstreetmap.de/wiki/Rules
- MapCSS 实现与运算符全集（`∈ ⊆ ⧉`、伪类、`::layer`）：https://josm.openstreetmap.de/wiki/Help/Styles/MapCSSImplementation
- JOSM 验证器偏好（抑制细节）：https://josm.openstreetmap.de/wiki/Help/Preferences/Validator
  > 待验证：抑制/忽略列表的 preference key 与持久化语义，一手资料未复核。
- OSM API v0.6 能力上限（waynodes/relationmembers/changeset）：https://wiki.openstreetmap.org/wiki/API_v0.6
- 冲突对话框与上传 hook（409 语义）：https://josm.openstreetmap.de/wiki/Help/Dialog/Conflict
- `rstar` 0.14（R\* 树邻域查询）与 `rayon`（`par_iter`/`fold`/`reduce`）：https://docs.rs/rstar/0.14.0/rstar/ · https://docs.rs/rayon/latest/rayon/
- 一手事实基础（12 个核心规则文件、40 个测试类、hook 链）：[`.research/josm-inventory.md`](../.research/josm-inventory.md)
