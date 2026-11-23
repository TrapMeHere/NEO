# 11 样式、预设、过滤与搜索（MapCSS / Presets / Imagery）

> 状态：草案 v0.1 ｜ 日期：2026-11-15 ｜ 归属 crate：`neo-style`（求值与资产）、`neo-ui`（表单）、`neo-validate`（规则消费方）
> 相关：[04-spatial-index-and-geometry.md](04-spatial-index-and-geometry.md) ｜ [05-renderer-wgpu.md](05-renderer-wgpu.md) ｜ [12-validation.md](12-validation.md) ｜ [13-josm-parity.md](13-josm-parity.md) ｜ [ADR-0010](adr/0010-mapcss-compatibility.md) ｜ [14-performance-and-verification.md](14-performance-and-verification.md)

## 摘要

- 一套语法，三个消费者：JOSM 用 **extended MapCSS 0.2** 同时驱动 **地图样式**、**validator 规则**、**搜索框**；NEO 复用同一个 parser（`neo-style::mapcss`），只换求值后端（[12](12-validation.md) §3.1 的规则层）。
- 级联语义必须照抄 JOSM：**没有 CSS 的 specificity**，规则按文件顺序「后覆盖前」，`::layer` 把属性表分层再合成。任何"改良"都会让 `elemstyles.mapcss` 渲染走样。
- 兼容目标是 **未修改的 `elemstyles.mapcss` 能加载并画出同一张地图**，而不是"支持 MapCSS 0.2 规范"。差异用清单管理（§2.7），不是用 bug 修。
- 不支持的属性（`line: image`、`shield:`、`label:` 下的 `font-variant`/`text-decoration`/`text-transform`/`max-width`）在**编译期**识别，给出可读警告并降级到最近的可实现视觉效果，绝不静默丢弃。
- `defaultpresets.xml`（660 KB）同样是**兼容目标**：`group/item/chunk/reference`、`key/text/combo/multiselect/check/roles`、`type` 组合（`multipolygon` 不在 `relation` 内）全部要能吃下；XML→egui 表单走中性 `PresetForm` 描述，UI 不解析 XML。
- Filter 的语义是硬契约：`E/H/I/M` + `R/A/D/F`，自上而下求值，被过滤对象**不可选中且不参与搜索**（[13](13-josm-parity.md) §4 第 7 条）。
- 搜索框是 MapCSS 的子集 DSL：`type:`/`key=value`/`-`/`&`/`|`/正则/`preset:` + NEO 的空间谓词；语法与求值分开，`SearchQuery` 可编译成 mapcss 选择器（JOSM 的 MapCSS 模式）。
- 所有 JOSM 资源（presets/styles/rules/boundaries）**运行时加载、不编进二进制**：GPL/CC-BY-SA 资产与 NEO 代码隔离（[ADR-0010](adr/0010-mapcss-compatibility.md)）。
- 影像目录来自 ELI（Editor Layer Index），其 GitHub 许可标注为 **NOASSERTION**：不打包、不假设 CC0，运行时下载并逐条显示 attribution。

## 0 阶段 4 已实现（v0.1.0）

- `neo-style::mapcss`：MapCSS **子集**解析器 + 编译（选择器 `way/node/area/relation/*`、
  `[key]`/`[key=value]`、逗号多选择器、`|z12-` 缩放、`color/fill-color/width/opacity`、
  `#rgb/#rrggbb/#rrggbbaa`）
- 默认样式表 `assets/styles/neo.mapcss`（编译期嵌入），`SceneBuilder` 在构建几何时按
  `(tags, zoom)` 解析，命中则覆盖内置调色板/线宽，未命中回退
- 未支持：text/icon-image、eval/setting、图案、dashes、@media、MapCSS validator（后续）

## 1 目标与范围

### 1.1 目标

1. 完整覆盖 `elemstyles.mapcss` 实际使用的语法子集（选择器、运算符、伪类、`::layer`、`@supports`、territory），并让渲染结果与 JOSM 可对拍。
2. 样式求值在 10M 对象规模下可由「tag 指纹缓存 + 候选剪枝」做到增量重算。
3. `defaultpresets.xml` 零修改加载；表单生成的交互（字段类型、默认值、`match` 过滤）与 JOSM 等价。
4. Filter 与 Search 共用同一表达式求值器，保证"过滤掉的搜不到"。
5. 资产许可可审计：每个资产有来源、版本、sha256、许可标识。

### 1.2 非目标

- 不做 MapCSS 规范的"完整实现"（`line: image`、`shield:`、Kothic 挤出等明确不做，见 §2.6）。
- 不做样式编辑器 GUI（Phase 2 的 P2 项）；本阶段只做加载、编译、热重载。
- 不做法线贴图/3D 样式；`extrude` 类扩展不支持。
- 不移植 JOSM 的 40 个 Java 验证测试（属 [12](12-validation.md)）。

## 2 设计

### 2.1 三个消费者，一套前端

```
                     ┌───────────────────────────────────────────┐
  .mapcss 文本 ──►   │ lexer → parser → AST（选择器/声明/@supports）│
                     └───────────────────────────────────────────┘
                          │                │                 │
              ┌───────────┘                │                 └────────────┐
              ▼                            ▼                              ▼
   StyleTarget::Mappaint        StyleTarget::Validator          StyleTarget::Search
   （属性 → ResolvedStyle）      （throw*/fix* 子句，见 12）      （选择器即查询）
              │                            │                              │
              ▼                            ▼                              ▼
    neo-scene → SceneFrame       ValidationReport              Vec<ObjId> 结果集
```

三个目标共用 AST 与选择器求值，但**属性集合不同**：`fill-color` 在 validator 里无意义，`throwWarning` 在渲染里无意义。`CompileOpts.target` 决定哪些声明被保留、哪些报「该目标不支持」。

### 2.2 词法与语法

```text
stylesheet  := (rule | supports_block | "meta" "{" decl* "}" | "canvas" "{" decl* "}")*
rule        := selector_list "{" decl* "}"
selector    := type_sel zoom? cond* pseudo* layer?  ( ">" link? selector | "<" selector )?
type_sel    := "*" | "node" | "way" | "relation" | "area" | "closedway"
             | "meta" | "canvas" | "setting" | "settings"
zoom        := "|z" num ( "-" num? )?            // way|z12, way|z13-15, way|z16-
cond        := "[" "!"? ( key | key op value | "/" regex "/" ) ( "?" | "?!" )? "]"
link        := "[" ( "role" | "index" ) op value "]"
layer       := "::" ( ident | "*" )
decl        := prop ":" value ( ";" value )* ";"?   // 也允许 "set: class" / "set: .class"
supports_block := "@supports" "(" name ":" value ")" "{" rule* "}"
```

| 词法要点 | 规则 |
|---|---|
| 注释 | 仅 `/* … */`；注释里出现 `/…/` 正则会提前结束（JOSM 同样的坑，[文档](https://josm.openstreetmap.de/wiki/Help/Styles/MapCSSImplementation) 明确警告） |
| 引号 | 键/值含空格、`:`、非 ASCII 时必须 `"…"`；`"` 与 `\` 可转义 |
| 颜色 | 具名色、`#RGB`/`#RRGGBB`/`#RRGGBBAA`、`rgb()`、`rgba()`、`hsb_color()`、可选 `name#hex`（可在颜色偏好里被用户覆盖） |
| 数值 | 相对值前置 `+`（如 `width: +2`）表示"同层默认值 + 2"，不是绝对值 |
| 表达式 | 允许裸表达式（JOSM 不强制 `eval(...)`），NEO 两种都接受，编译成同一 AST |

### 2.3 选择器与伪类

| 选择器 | 语义 | NEO 优先级 |
|---|---|---|
| `*` | 任意对象 | P0 |
| `node` / `way` / `relation` | 原语类型 | P0 |
| `area` | 任何面积：闭合 way **或** multipolygon；**也匹配未闭合 way**（所以常配 `:closed`） | P0 |
| `closedway` | 闭合 way | P0 |
| `meta` | 样式前言（`title/icon/version/description/author/link/min-josm-version`） | P0 |
| `canvas` | 背景（`fill-color`、`default-points`、`default-lines`） | P0 |
| `setting` / `settings` | 样式自定义设置（`setting()` 函数取值） | P1 |
| `parent > child` | 子选择器（`>`） | P0（`elemstyles` 需要） |
| `parent < child` | **JOSM 专有**父选择器（`<`），属性作用于右侧对象 | P1 |
| `>[role=…]` / `>[index=1]` | 链接选择器：成员 role、下标（负数从尾数） | P1 |
| `|z12` / `|z13-15` / `|z16-` | zoom 选择器；只对 `>` 右侧有意义 | P0 |

伪类全集（JOSM 已核实清单，`!` 可否定）：`:closed`、`:closed2`、`:completely_downloaded`、`:new`、`:connection`、`:unconnected`、`:tagged`（JOSM 有一份"不算 tag"的排除表：`source*`、`source_ref`、`note`、`comment`、`converted_by`、`created_by`、`watch*`、`fixme`、`FIXME`、`description`、`attribution`）、`:area-style`、`:righthandtraffic`、`:clockwise`、`:anticlockwise`、`:unclosed_multipolygon`、`:open_end`、`:in-downloaded-area`、`:selected`、`:highlighted`、`:modified`。

- P0（渲染必需）：`closed`、`closed2`、`tagged`、`new`、`modified`、`connection`、`unconnected`、`clockwise`、`anticlockwise`。
- P1（校验/提示为主）：`completely_downloaded`、`in-downloaded-area`、`righthandtraffic`、`unclosed_multipolygon`、`open_end`、`selected`、`highlighted`。
- `:area-style` **不适用**（JOSM 内部用于"该对象被给了面积样式"，NEO 的求值顺序不同，明确不支持并进差异清单）。
- 伪类序号影响 validator 的 `{n}` 占位符（`:unconnected` = `{0}`），必须由同一 parser 保序产出（[12](12-validation.md) §3.1）。

条件运算符（`[k op v]`）：

| 运算符 | 语义 |
|---|---|
| `=` / `!=` | 精确匹配（**大小写敏感**） |
| `<` `>` `<=` `>=` | 数值比较 |
| `^=` / `$=` / `*=` | 前缀 / 后缀 / 子串 |
| `~=` | 分号列表成员（`vending~=stamps`） |
| `=~` / `!~` | 正则（`/…/`，`(?i)` 内联标志可用） |
| `∈` `⊆` | 「左侧对象属于右侧选择器集合」：`*[amenity=parking] ∈ area[amenity=parking]` |
| `⊇` | 反向包含（较慢，多用于搜索而非渲染） |
| `⊈` / `⊉` | 上述的否定；配 `:closed2` 避免未闭合面误判 |
| `⧉` | 相交/重叠；**尊重 `layer` tag** |
| `[k]` / `[!k]` / `[/^addr:/]` | 键存在 / 不存在 / 键名正则 |
| `[k?]` / `[k?!]` | 真值（`yes`/`true`/`1`）与假值 |

- **解引用** `[k1=*k2]`：用 `k1` 的值去比对 `k2` 的值（`[k=~*/re/]` 则把 `k` 所指键的值当正则）；编译期展开成第二次取值，不做字符串拼接。
- **territory** `inside("FR")` / `outside("FR,DE")`：`inside` 的逗号是 **OR**、`outside` 的逗号是 **AND**，支持 `FR`/`FX`/`EU`/次国家码 `US-FL`；数据来自 `boundaries.osm`，求值昂贵（样式慎用、validator 常用）。
- 子选择器左侧不带 zoom/layer，`prop()`/`is_prop_set()` 只在 `>` 右侧有效；`area` 与 `:closed` 的组合必须显式写。

### 2.4 级联与优先级（照抄 JOSM，不做"改良"）

JOSM 的算法是：**按文件顺序遍历每条规则；选择器命中就写入属性表（后者覆盖前者）**；`::layer` 把属性表分成独立的层，最后合成。**没有 specificity 概念**。

```
for rule in rules_in_file_order:            # 后覆盖前
    if selector.matches(obj, ctx):
        layer = rule.layer or "default"     # ::name；::* 写入所有层
        props[layer].apply(rule.decls)
compose(props) -> ResolvedStyle             # 按 z-index 排序层，逐层叠加
```

| 排序键（从高到低） | 说明 |
|---|---|
| `major-z-index` | 对象类别层（点/线/面）的粗排序 |
| `z-index` | `::layer` 内排序（JOSM 默认 **0**，Halcyon 是 5） |
| `object-z-index` | 同层内单对象微调 |
| 层声明序号 | 兜底：稳定且可复现（golden 图依赖） |

- `::*` 是"对**所有**层写入/初始化"的语法糖，`elemstyles` 用它做全局默认（如 `*::* { … }`）；NEO 在编译期把它展开成"每个已声明层各一条"，避免运行时遍历层。
- 相对值 `+2` 在合成期解析（取同层 `default` 的当前值再加），因此必须保留"声明顺序 + 层"信息，不能在编译期折叠成常量。
- 多文件叠加（用户激活多个样式）：按加载顺序拼接，`source: origin.file + rule.index` 参与诊断与热重载定位。

### 2.5 属性与值模型

| 组 | 代表属性 | NEO 映射 |
|---|---|---|
| 线 | `width`、`color`、`opacity`、`dashes`、`linecap`、`linejoin`、`repeat-image*` | `StrokeSpec`（`repeat-image*` 走图标纹理，P2） |
| casing | `casing-*`、`left-casing-*`、`right-casing-*` | 独立 `StrokeSpec`，`BatchFlags::casing_of` 绑定（[05](05-renderer-wgpu.md) §2.3） |
| 面 | `fill-color`、`fill-image`、`fill-opacity`（默认 **0.2**，r11700 起）、`fill-extent` | `FillSpec`；`fill-opacity` 默认值易被忽略，必须照抄 |
| 点/图标 | `symbol-shape`、`symbol-size`、`symbol-fill-color`、`icon-image`、`icon-width` | `IconSpec`（C 管线实例） |
| 文本 | `text`、`text-color/-halo-*/-offset-*/-anchor-*/-rotation`、`font-family/-size/-weight/-style`、`text-position` | `TextSpec`（D 管线，[05](05-renderer-wgpu.md) §2.6） |
| 求值函数 | `tag()`、`prop()`、`is_prop_set()`、`parent_tag()`、`concat()`、`eval()`、`setting()`、`tr()`… | 编译成闭包树，白名单执行，**不允许**任意副作用 |

`eval()` 的实现边界：白名单函数 + 字面量 + 四则/布尔/比较，绝不做通用脚本。`JOSM_pref()`/`JOSM_search()` 返回 NEO 的设置值/搜索结果（后者会触发求值递归，标记为 P2 并默认禁用）。

### 2.6 不支持项与差异清单

| JOSM 能力 | 状态 | NEO 策略 |
|---|---|---|
| `line: image`（沿线重复贴图） | ❌ 不支持（JOSM 也不支持，[文档](https://josm.openstreetmap.de/wiki/Help/Styles/MapCSSImplementation) 明确） | 编译期警告，降级为纯色线 |
| `shield:` 系（路牌） | ❌ 不支持 | 降级为 `text:` + 圆角背景（Phase 2 的 P2） |
| `label:` 下的 `font-variant` / `text-decoration` / `text-transform` / `max-width` | ❌ 不支持 | 警告；`text-transform` 在 NEO 里可选项实现（P2） |
| `@media (min-josm-version: …)` | ⚠️ 接受但**建议用 `@supports`**（JOSM 已弃用 `@media`） | `@media` 求值为"按 NEO 声称的 josm-version 兼容级别"，映射规则见 §2.7 |
| `:area-style` | ❌ 不适用 | 差异清单第 1 条 |
| 正则风味 | Java regex vs Rust `regex` | 不用 lookaround/backreference 的规则可等价；命中不支持的构造时警告并退化为"不匹配"（不是 panic） |

> 原则：**未知属性 = 警告 + 保留原始文本**。`elemstyles.mapcss` 里有几十个我们暂不实现的属性，静默丢弃会让"看起来差一点"变成无法定位的谜题；保留原文使 `neo-ui` 能列出"这条规则的第 N 个属性未生效"。

### 2.7 对 `elemstyles.mapcss` 的兼容目标

| 维度 | 目标 | 验收方式 |
|---|---|---|
| 解析 | 零 `StyleError`（警告允许，错误不允许） | CI 加载 + 计数断言 |
| 选择器 | `elemstyles` 用到的全部选择器可编译（含 `area`、`::layer`、`:closed`、zoom） | 覆盖率报告：未命中的 selector 种类 = 0 |
| 渲染 | 同数据集、同 zoom 下与 JOSM 截图人工对拍（网格取样 9 宫格） | `docs/known-deviations.md` 记录的差异 ≤ 5 条 |
| z 序 | `major-z-index`/`z-index` 排序与 JOSM 一致（默认 0） | 叠放顺序对比用例 |
| 性能 | 10M 对象下样式求值不进主线程热点 | 预算见 [05](05-renderer-wgpu.md) §2.13 |

已知差异（初始清单，随实现增补，进 `docs/known-deviations.md`）：

1. `:area-style` 不求值。
2. `::*` 在编译期展开而非运行时遍历（语义等价，但 `is_prop_set` 的时序细节需对拍）。
3. 文本排版用 NEO 的 atlas/egui，字体度量与 Java AWT 不同（行高、字距），标签避让结果会有像素级差异。
4. `@media` 的版本语义按"兼容级别"映射而非 JOSM 构建号。

### 2.8 样式 → `SceneFrame` 的求值管线

```
① 编译（启动/热重载，一次性）
   .mapcss ──lexer──► tokens ──parser──► AST ──compile──► StyleSheet
                                                          ├ rules: Vec<CompiledRule>
                                                          ├ key_index: HashMap<Key, SmallVec<RuleId>>  ← 剪枝
                                                          ├ layers / meta / canvas
                                                          └ unsupported: Vec<UnsupportedProperty>

② 求值（每对象一次，结果缓存）
   tags ──► TagFingerprint(u64)  ──► LRU<TagFingerprint, SmallVec<StyleRef>>
                │ 未命中
                ├─ 候选 = key_index[每个存在的 key] ∪ 无 key 条件的规则
                ├─ 逐条 matches(obj, ctx)          ← 选择器求值（含伪类/territory）
                └─ 命中集合 → 级联合成 → ResolvedStyle

③ 打包（neo-scene）
   ResolvedStyle + Polyline/AreaGeom ──► MapVertex / RenderBatch / LabelItem
```

| 失效条件 | 重算范围 |
|---|---|
| 对象 tag 变化 | 该对象的指纹缓存条目 |
| 选中/高亮/修改状态变化 | 维护 `pseudo → BTreeSet<ObjId>`，只重算含该伪类的规则命中对象 |
| 样式热重载 | 整表重建 + 全缓存清空（样式切换本来就是低频） |
| zoom 跨档 | 只影响几何/LOD，样式命中不变（除非规则含 zoom 选择器 → 按档位分桶缓存） |
| Filter 变化 | 不影响样式，只影响可见集与可选中性 |

`TagFingerprint` = 排序后的 `(key, value)` 序列的 64 位哈希（含类型位），碰撞用全量比对兜底。这一层是「10M 对象 × 数千规则」能成立的前提。

### 2.9 Presets XML

```text
<presets xmlns="http://josm.openstreetmap.de/tagging-preset-1.0" author version description shortdescription link icon baselanguage>
  <group name icon>                                  ← 树节点
    <item name type preset_name_label icon name_template match_expression>
      <label text/> <optional text/> <space/> <separator/> <item_separator/> <link wiki|href/>
      <key key value match/>                          ← value 省略 = 删除该 key
      <text key/> <combo key values delimiter …/> <multiselect … delimiter=";"/>
      <check key default="on|off"/> <list_entry value/> <checkgroup columns>…</checkgroup>
      <roles><role key text requisite count type member_expression regexp/></roles>
      <preset_link preset_name text/>  <reference ref="surface"/>   ← 引用 <chunk id="surface">
    </item>
  </group>
  <chunk id="surface">…</chunk>                        ← 可复用片段
</presets>
```

| 约束 | 说明 |
|---|---|
| `type` | `node`、`way`、`closedway`、`multipolygon`、`relation`，可逗号组合；**`multipolygon` 不包含在 `relation` 里**（经典 bug 源） |
| schema | `tagging-preset.xsd` 是解析器的唯一规范来源，样本文件 660 KB → `match` 表达式必须**预编译**（不是每次逐对象解析 XML 文本） |
| 多语言 | 属性后缀式本地化（`de.description`）与 `baselanguage`；`<label text>` 走 `tr()` |
| 引用 | `<chunk>`/`<reference>` 在加载期展开成树（保留 chunk id 以便诊断），**不是**运行时 include |
| 校验 | `assertMatch`/`assertNoMatch` 只属于 validator 规则，presets 没有；presets 的自检靠 xsd |

XML → egui 表单的映射（`PresetForm` 是中性描述，`neo-ui` 才认识 egui）：

| XML 元素 | `PresetForm` 控件 | 交互 |
|---|---|---|
| `<text key>` | `TextField`（单行/多行） | 空值 = 删除该 key |
| `<combo key values editable delimiter>` | `ComboBox`（可编辑 + 逗号/分号拆分） | `values_from` 从另一个 key 动态取值（P1） |
| `<multiselect>` | `ChipGroup`（分隔符默认 `;`，`\` 转义） | 顺序保留 |
| `<check key default>` | `Checkbox` | `on/off` 与"不存在"三态映射 |
| `<key key value>` | 无控件、直接写入 | `value` 缺省表示**移除** |
| `<roles>` | 成员角色表格（role/type/count/requisite） | relation 编辑器的约束 |
| `<reference>` | 展开为同一片段（共享状态） | 修改同时影响所有引用点 |

### 2.10 Validator 规则复用

- 同一 `StyleSheet` 前端，`StyleTarget::Validator` 只保留 `throwError|throwWarning|throwOther`（**恰好一个**）、`fixAdd`/`fixRemove`/`fixChangeKey`/`fixDeleteObject`/`suggestAlternative`/`group`/`assertMatch`/`assertNoMatch`。
- `{0}`/`{1}` 的编号来自**选择器顺序（含伪类）**，由 parser 统一产出，避免 `neo-validate` 自己再解析一遍。
- 规则来源：12 个核心 `.validator.mapcss` + 社区 `Rules/*`；与 Rust 规则的分工、严重级别与抑制语义**全部以 [12](12-validation.md) 为准**，本文只负责"同一份 AST"。

### 2.11 Imagery 目录、ELI 与图层配置

| 项 | 事实 | NEO 策略 |
|---|---|---|
| 目录来源 | JOSM 从 `josm.openstreetmap.de` 取 `maps.xml`，由 **ELI（Editor Layer Index）** 生成 | 直接消费 ELI 的 JSON/`maps.xml`，缓存到用户数据目录，带 ETag/版本 |
| 许可 | ELI 仓库 GitHub 元数据标注 **NOASSERTION / Other** → **不能假设 CC0** | 不打包进安装包；运行时下载；UI 展示每条目的 `attribution` 与 `terms`；导出「我用了哪些影像」清单 |
| 内置条目 | JOSM 内置 5 条（Bing、Esri World Imagery、Esri Clarity Beta、Mapbox Satellite、OSM Carto） | 不预置需要商业 key 的条目；提供 `OSM Carto` 这类无 key 项 + "添加 URL" |
| 图层类型 | TMS / WMS / WMTS（JOSM 会话文件里是独立图层类型） | 统一 `ImageryLayer { url_template, kind, min_zoom, max_zoom, tile_size, format, attribution }` |
| URL 模板 | TMS `{z}/{x}/{y}`；WMS `SERVICE=WMS&REQUEST=GetMap&…&BBOX={bbox}&WIDTH={width}&HEIGHT={height}&SRS={proj}`；WMTS `TileMatrix={z}&TileRow={y}&TileCol={x}`；子域轮转 `{switch:a,b,c}` | 模板变量表随实现固定；**完整占位符集合 > 待验证**（以 `maps.xsd` 为准） |
| 偏移 | 偏移库是 **XML**（不是 JSON），端点 `GET {server}get?lat=…&lon=…`，返回 `<offset>`；本地偏移按影像持久化 | 只做本地 x/y 偏移 + 手动调整；在线偏移库 P1（默认服务器 `https://offsets.textual.ru/`，注意这是第三方服务） |

### 2.12 Filter 语义

| 列 | 含义 | 影响 |
|---|---|---|
| **E**（Enable） | 该行是否启用 | 关闭 = 该行不参与 |
| **H**（Hide） | 匹配对象**完全隐藏**（不是灰显） | 灰显 = disable（不可选但仍可见）；hide = 不可见 |
| **Text** | 过滤表达式（与搜索同一 DSL） | 见 §2.13 |
| **I**（Inverse） | 反转：只显示匹配对象 | 与 H 组合等价于"只隐藏不匹配的" |
| **M**（Mode） | 与**上面各行**的组合方式 | `R` 替换（忽略上面所有行）/`A` 追加（各行独立，最常用）/`D` 移除（取消上面的过滤）/`F` 在结果中再查找 |

硬契约（与 JOSM 一致，[13](13-josm-parity.md) §4 第 7 条）：

1. 自上而下求值，`M` 决定与上面结果的合成。
2. 被过滤对象**不可选中**（选择工具、框选、`Ctrl+A` 后不可被选中）；例外极少，NEO 不复制那些例外。
3. 被过滤对象**不出现在搜索结果里**（"过滤掉的搜不到"）。
4. 过滤不改变数据：仍会上传、仍参与校验、仍计入统计。UI 显示"Filter active / N objects hidden"。
5. 快捷键 `Ctrl+Alt+E` + `[1-9,0,A-Z]` 按行号切换 E 列。

### 2.13 搜索 DSL

| 语法 | 语义 | 来源 |
|---|---|---|
| `type:node` / `type:way` / `type:relation` | 类型 | JOSM 已核实 |
| `key=value` | 精确匹配（大小写敏感） | 已核实 |
| `key:value` | 子串匹配（大小写不敏感，如 `name:Herrengasse`） | 已核实 |
| `"addr:street"="Baker Street"` | 键/值含 `:` 或空格必须加引号 | 已核实 |
| `-expr` | 取反（如 `-highway:`） | 已核实 |
| `expr1 expr2` / `expr1 & expr2` | AND（并列即 AND） | 已核实 |
| `expr1 \| expr2` | OR | 已核实 |
| `key=~/regex/`、`/^addr:/` | 正则（JOSM 用 Java regex；NEO 用 `regex` crate，构造差异见 §2.6） | 已核实 |
| `id:`、`user:`、`timestamp:a/b`、`version:`、`changeset:` | 元数据谓词 | 已核实（filter 示例含 `user:Steve`） |
| `preset:"Craft/Beekeeper"` | 按预设匹配 | 已核实 |
| 整串交给 MapCSS 选择器 | JOSM 搜索框的 "MapCSS selector" 模式 | 已核实 |
| `in <bbox>` / `bbox:` | 空间范围谓词 | **> 待验证**：JOSM 官方搜索页未列出 `in`/`bbox`，NEO 作为扩展实现并标注 |

求值顺序：`DSL parse → 类型/键索引剪枝 → 谓词求值 → 排除 Filter 命中 → 返回 Vec<ObjId>`。空间谓词用 [04](04-spatial-index-and-geometry.md) 的对象 R-tree；`-` 与 `&`/`|` 的优先级遵循"`-` 最强、`&` 次之、`|` 最弱"，括号显式分组。

### 2.14 i18n 与资产加载

| 资产 | 加载方式 | 许可/隔离 |
|---|---|---|
| `elemstyles.mapcss`、presets、12 个 validator 规则、`boundaries.osm` | 运行时从 assets 目录或用户目录读取（热重载） | GPL/CC-BY-SA 资产与代码物理分离；不 `include_str!`、不进二进制 |
| 翻译 | JOSM 的 `.lang`（48 个文件）与 NEO 自有 key 并存；先查 NEO 表，再查资产表 | `.lang` 随资产目录分发 |
| 字体 | 系统字体枚举（CJK 见 [05](05-renderer-wgpu.md) §2.11），可选下载 OFL 字库 | 不进二进制 |
| 目录清单 | `assets/manifest.json`：来源 URL、版本、sha256、license 标识 | CI 校验 sha256；缺 license 字段直接失败 |

`tr()` 的键空间：NEO 自有 UI 用 `neo.*` 前缀，JOSM 资产用原始英文串（与 `.lang` 对齐），避免两套翻译体系互相覆盖。

## 3 关键接口

```rust
// ── neo-style/src/mapcss/mod.rs ─────────────────────────────────────────────
pub enum StyleTarget { Mappaint, Validator, Search }
pub struct CompileOpts { pub target: StyleTarget, pub settings: StyleSettings, pub claimed_josm_version: u32 }

pub struct StyleSheet { /* rules, key_index, layers, meta, canvas, unsupported */ }
impl StyleSheet {
    pub fn compile(src: &str, opts: &CompileOpts) -> Result<Self, Vec<StyleError>>;
    pub fn extend(&mut self, src: &str, origin: StyleOrigin) -> Result<(), Vec<StyleError>>; // 多文件叠加
    pub fn apply(&self, obj: &StyleObject<'_>, ctx: &StyleCtx<'_>) -> ResolvedStyle;         // 级联 + 层合成
    pub fn candidates(&self, tags: &TagView<'_>, kind: GeomKind) -> RuleCandidates<'_>;      // key 索引剪枝
    pub fn meta(&self) -> &StyleMeta;
    pub fn canvas(&self) -> &CanvasStyle;
    pub fn unsupported(&self) -> &[UnsupportedProperty];   // 供 UI 与 known-deviations 导出
    pub fn fingerprint(&self) -> u64;                      // 热重载判定
}

pub struct StyleObject<'a> {
    pub kind: GeomKind,
    pub tags: TagView<'a>,
    pub pseudo: PseudoSet,
    pub geom: Option<GeomFacts>,   // 闭合、面积、方向、未闭合 multipolygon
}
pub struct StyleCtx<'a> {
    pub zoom: f64,
    pub territory: Option<&'a TerritoryDb>,   // inside()/outside()
    pub settings: &'a StyleSettings,          // setting()
    pub layer_seq: &'a [LayerId],
}
pub struct ResolvedStyle {
    pub fill: Option<FillSpec>,
    pub stroke: Vec<StrokeSpec>,     // 主线 + casing(left/right)
    pub icon: Option<IconSpec>,
    pub text: Option<TextSpec>,
    pub z: ZKey,                     // (major_z, z, object_z)
    pub layer: LayerId,
}

// ── neo-style/src/preset.rs ─────────────────────────────────────────────────
pub struct PresetCatalog { /* 预编译 match 表达式 + name/type 索引 */ }
impl PresetCatalog {
    pub fn load_xml(bytes: &[u8], i18n: &dyn I18n) -> Result<Self, Vec<PresetError>>;
    pub fn extend_xml(&mut self, bytes: &[u8], i18n: &dyn I18n) -> Result<(), Vec<PresetError>>;
    pub fn matches_for(&self, kind: PresetType, tags: &TagView<'_>) -> Vec<&PresetItem>;
    pub fn item_by_name(&self, name: &str) -> Option<&PresetItem>;
    pub fn roots(&self) -> &[PresetGroup];
    pub fn form(&self, item: &PresetItem) -> PresetForm;   // 与 egui 无关的中性描述
}

// ── neo-style/src/filter.rs ─────────────────────────────────────────────────
pub enum FilterMode { Replace, Add, Remove, Find }
pub struct FilterRow { pub enabled: bool, pub hide: bool, pub inverse: bool, pub mode: FilterMode, pub expr: SearchQuery }
pub struct FilterSet { /* rows + 求值缓存 + pseudo 集合 */ }
impl FilterSet {
    pub fn rows(&self) -> &[FilterRow];
    pub fn evaluate(&mut self, snapshot: &Snapshot, index: &SpatialIndex) -> FilterOutcome;
    pub fn is_visible(&self, id: ObjId) -> bool;
    pub fn is_selectable(&self, id: ObjId) -> bool;
    pub fn hidden_count(&self) -> usize;
}

// ── neo-editor/src/search.rs ────────────────────────────────────────────────
pub struct SearchQuery { /* AST */ }
impl SearchQuery {
    pub fn parse(src: &str) -> Result<Self, Vec<SearchError>>;
    pub fn eval(&self, snapshot: &Snapshot, index: &SpatialIndex, filters: &FilterSet, bbox: Option<Aabb>) -> Vec<ObjId>;
    pub fn to_mapcss_selector(&self) -> Option<String>;   // JOSM 的 MapCSS 搜索模式
}

```

## 4 数据流

```
启动 / 热重载：assets/*.mapcss ─► StyleSheet::compile + extend ─► neo-scene
             defaultpresets.xml ─► PresetCatalog::load_xml（chunk/reference 展开 + match 预编译）
             validator/*.mapcss ─► StyleSheet::compile(Validator) ─► neo-validate
             boundaries.osm ─► TerritoryDb；ELI/maps.xml ─► ImageryCatalog（缓存 + attribution）
每帧：① 数据快照 ─► ② 样式求值（工作线程，指纹缓存）─► ③ 打包 SceneFrame ─► neo-render（仅 ③ 在主线程）
编辑：改 tag ─► 指纹失效 ─► 只重算该对象；改选择 ─► 只重算 :selected/:highlighted 相关；Filter 变 ─► 只重建可见集
```

```
一次搜索
  parse("type:way highway & -service") ──► AST
     ├─ 键索引：候选 = ways 且含 highway
     ├─ 谓词求值（标签 + 正则 + 空间）
     ├─ 减去 FilterSet 命中（不可见/不可选）
     └─ 结果 Vec<ObjId> ──► 选中集（可撤销为独立命令）
```

## 5 权衡与替代

| 决策 | 备选 | 取此的理由 | 代价 |
|---|---|---|---|
| 一套 parser 三个目标 | validator 另写一套 | JOSM 的 `.validator.mapcss` 与样式共享选择器语法；两套必然漂移 | parser 需要 `StyleTarget` 分支，测试面变大 |
| 照抄 JOSM 的"后覆盖前" | 引入 CSS specificity | `elemstyles` 依赖文件顺序；加 specificity 会改变渲染结果 | 规则顺序成为契约，重排即破坏兼容 |
| 编译期识别不支持属性并降级 | 遇错整体失败 | 663 KB 的官方样式里必然有我们没实现的属性；整体失败 = 不可用 | 需要维护 `unsupported` 清单与 UI 提示 |
| tag 指纹缓存 + key 倒排剪枝 | 逐对象逐规则全量匹配 | 10M × 数千规则不可行 | 缓存失效逻辑是新 bug 面 |
| 资产运行时加载 | 编进二进制 | GPL/CC-BY-SA 隔离 + 可热更新 + 用户可换 | 启动要多一次 IO 与校验；缺资产要优雅降级 |
| Presets 中性 `PresetForm` | UI 直接操作 XML DOM | egui 与 XML 解耦，可单测表单生成 | 多一层映射代码 |
| Filter 与 Search 共用求值器 | 各写一套 | 避免"过滤掉的能搜到" | 求值器要同时满足两种性能画像 |
| 影像不打包、在线目录 | 内置一份快照 | 许可（NOASSERTION）与时效性 | 首次启动需联网；离线需缓存 |

## 6 风险

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| R1 | GPL/CC-BY-SA 资产被误编进二进制 | 许可污染，整包需 GPL | CI 检查 `include_str!`/`include_bytes!` 白名单；资产目录独立 + manifest |
| R2 | MapCSS 语义偏差（尤其 `::*`、`:area-style`、z 序） | 与 JOSM 渲染不一致 | `known-deviations.md` 逐条登记；9 宫格截图对拍 |
| R3 | 样式求值拖慢主线程 | 掉帧 | 指纹缓存 + 工作线程 + `05` 的帧预算；超预算降 LOD 不降样式 |
| R4 | 660 KB presets + 大 match 表达式导致启动/匹配慢 | 打开预设面板卡顿 | 加载期预编译 match 为谓词；按 type 建索引；启动后异步完成 |
| R5 | 正则风味差异（lookaround/backref） | 某条规则永不匹配 | 编译期报"不支持的正则构造"，降级为不匹配并提示 |
| R6 | ELI 目录条目许可/条款变化 | 用户被误导使用受限影像 | 逐条展示 attribution/terms；不做"一键全选" |
| R7 | `.lang` 与 NEO 自有 i18n 混用导致串味 | UI 文案错乱 | 键前缀隔离（`neo.*` vs 原文），加载顺序固定 |

## 7 验收标准

1. **解析**：`elemstyles.mapcss`、12 个 `.validator.mapcss`、`defaultpresets.xml` 三者加载后 `StyleError`/`PresetError` 均为 0；`unsupported()` 条目与 `docs/known-deviations.md` 一一对应。
2. **渲染对拍**：参考数据集在 zoom 10/13/16/19 四档与 JOSM 截图九宫格比对，差异条目 ≤ 5 且每条有理由。
3. **级联**：构造 20 条用例覆盖"后覆盖前"、`::layer` 排序、`::*` 展开、相对 `+2`、casing 成对，全部通过。
4. **Presets**：`group/item/chunk/reference` 展开正确；`type` 组合（含 `multipolygon` 与 `relation` 的差异）用例通过；`match` 过滤在 10k 对象上单次 ≤ 【待实测，见 docs/14-performance-and-verification.md】。
5. **Filter**：被过滤对象在（框选、`Ctrl+A`、搜索、hover）四条路径上均不可命中；`E/H/I/M`+`R/A/D/F` 组合 8 例与 JOSM 行为一致。
6. **搜索**：§2.13 表中每条语法至少 2 个用例；`-`/`&`/`|` 优先级用例通过；filtered 排除用例通过。
7. **许可**：`assets/manifest.json` 覆盖全部资产，缺 license/sha256 时 CI 失败；二进制中 grep 不到任何资产文件的字符串常量。

## 8 参考

- JOSM MapCSS 实现（选择器、运算符、`::layer`、`@supports`、不支持项清单）：<https://josm.openstreetmap.de/wiki/Help/Styles/MapCSSImplementation>
- MapCSS tag checker（`throw*/fix*/assertMatch`、`{n}` 占位符）：<https://josm.openstreetmap.de/wiki/Help/Validator/MapCSSTagChecker>
- Tagging presets（XML 结构、`chunk`/`reference`、字段类型）：<https://josm.openstreetmap.de/wiki/TaggingPresets>
- Filter 面板（E/H/I/M 与 R/A/D/F、被过滤对象不可选、不参与搜索）：<https://josm.openstreetmap.de/wiki/Help/Dialog/Filter>
- 搜索语法（`type:`、`key=value`、`-`、`&`、`|`、正则、`preset:`、MapCSS 模式）：<https://josm.openstreetmap.de/wiki/Help/Action/Search>
- Editor Layer Index（许可标注 NOASSERTION，需逐条确认）：<https://github.com/osmlab/editor-layer-index>
- JOSM 影像偏移库（XML + 查询串端点）：<https://josm.openstreetmap.de/wiki/Plugin/ImageryOffsetDb>
- 待验证：`maps.xsd` 的完整 URL 占位符集合（`{switch:…}` 等）**> 待验证**；搜索框 `in`/`bbox` 关键字在 JOSM 中的正式语义 **> 待验证**；图标资产逐文件 provenance **> 待验证**。
