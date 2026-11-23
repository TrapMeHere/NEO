# ADR-0010：MapCSS 兼容策略（不发明新样式格式）

- **状态**：已接受（Accepted）
- **日期**：2026-11-15
- **决策范围**：`neo-style`（parser、级联、资产加载）、`neo-validate`（规则消费）、`neo-ui`（样式/预设面板）
- **相关**：[11-style-presets-mapcss.md](../11-style-presets-mapcss.md) ｜ [12-validation.md](../12-validation.md) ｜ [13-josm-parity.md](../13-josm-parity.md) ｜ [17-licensing-and-governance.md](../17-licensing-and-governance.md)

## 背景

JOSM 用**同一套 extended MapCSS 0.2** 驱动三个子系统：地图样式（`elemstyles.mapcss`）、validator 规则（12 个 `.validator.mapcss`）、搜索框（MapCSS 选择器模式）。再加上 `defaultpresets.xml`（660 KB）与 `boundaries.osm`，这些文件就是"用户肌肉记忆"本身。

三个可选方向：

1. **设计 NEO 自己的样式格式**（TOML/JSON DSL），把 JOSM 资源离线转换；
2. **完整实现 MapCSS 规范**（含 `line: image`、`shield:`、Kothic `extrude` 等）；
3. **实现 JOSM 实际用到的子集 + 差异清单**，并让未修改的官方资源可直接加载。

方向 1 的问题：社区样式与社区 validator 规则（wiki 上的 `Rules/*` 页面）立刻全部失效，而"社区可写的声明式规则"正是 JOSM 生态的关键资产；转换器还会把"渲染差异"变成"转换 bug"，无法对拍。方向 2 的问题：规范里我们根本不打算实现的部分（沿线贴图、路牌、挤出）会变成永久的半成品代码，而 JOSM 自己也不支持其中一部分。

另一个硬约束是许可：这些资源是 GPL-2.0+（部分 wiki 镜像为 CC-BY-SA + LGPL），它们与 NEO 代码必须**分开分发、分开加载**，否则整个二进制被拉进 GPL。

## 决策

**采用方向 3**，并明确以下边界：

| # | 决策 | 说明 |
|---|---|---|
| D1 | 一套 parser，三个 `StyleTarget`（`Mappaint` / `Validator` / `Search`） | 选择器求值共享；属性集合按目标裁剪并报"该目标不支持" |
| D2 | **照抄 JOSM 的级联语义**：规则按文件顺序后覆盖前，`::layer` 分层后合成，`::*` 展开到所有层；**不引入 CSS specificity** | `elemstyles` 的结果依赖文件顺序；加 specificity = 渲染走样 |
| D3 | 支持 JOSM 实际使用的选择器与运算符全集：`area`/`closedway`、`> <` 子父选择器、`>[role=]/[index=]`、`|z13-15`、`::layer`、`@supports`、`inside()/outside()`、`[k=*k2]` 解引用、`∈ ⊆ ⊇ ⊈ ⊉ ⧉` | 逐项见 [11](../11-style-presets-mapcss.md) §2.3–2.5 |
| D4 | 明确不支持并**编译期降级**：`line: image`、`shield:`、`label:` 下的 `font-variant`/`text-decoration`/`text-transform`/`max-width`、Kothic `extrude`、`:area-style` | 不静默丢弃：进 `unsupported()` 并展示给用户，同时登记 `docs/known-deviations.md` |
| D5 | `defaultpresets.xml` 零修改加载；`<chunk>/<reference>` 在加载期展开，`match` 表达式预编译 | `tagging-preset.xsd` 是唯一规范来源；660 KB 不允许逐对象解析 |
| D6 | 所有 JOSM 资源**运行时加载、不编进二进制**（禁 `include_str!`），资产目录独立且带 `manifest.json`（来源/版本/sha256/license） | GPL/CC-BY-SA 隔离；可热重载；用户可替换 |
| D7 | 影像目录消费 ELI，**不打包**；逐条展示 attribution/terms（ELI 许可标注 NOASSERTION） | 不假设 CC0，不做"一键全选" |
| D8 | 差异用清单管理，不以"修 bug"的方式悄悄改变语义 | 每条差异必须有理由与对拍证据 |

## 替代方案（已评估并放弃）

| 方案 | 优点 | 放弃理由 |
|---|---|---|
| 自有样式格式（TOML/JSON） | 解析简单、类型安全、无需兼容负担 | 社区样式/规则全部失效；必须维护转换器且无法逐像素对拍 |
| 完整实现 MapCSS 0.2 规范 | "规范兼容"更漂亮 | 规范里 `line: image`/`shield:` 等我们不做；JOSM 自身也不支持一部分；投入产出比差 |
| 直接复用 JOSM 的 Java 实现（JNI/JVM 嵌入） | 语义 100% 一致 | GPL 传染 + 需要 JVM + 与"Rust 原生重写"的目标冲突 |
| 只做 validator 的 mapcss（渲染走自有格式） | 规则兼容、渲染自由 | 一张地图两套样式语义，用户看到的世界与规则理解的世界不一致 |

## 后果

**正面**

- 官方样式、预设、validator 规则**未修改即可加载**，迁移成本最低，社区生态可用。
- 同一份 AST 服务三个消费者，不会出现"过滤掉的能搜到"这类语义漂移。
- 许可隔离清晰：代码（MIT/Apache 双许可待定）与资源（GPL/CC-BY-SA）物理分离，可独立替换与热重载。

**负面 / 代价**

- parser 必须容忍 JOSM 的历史包袱（注释里的正则、相对值 `+2`、`::*`、已弃用的 `@media`）。
- "不支持项"是长期存在的清单，需要 UI 呈现与文档维护。
- 文本排版/字体度量与 Java AWT 不同，标签位置会有像素级差异，必须写进差异清单而不是当 bug 修。
- 求值性能要靠 tag 指纹缓存 + key 倒排剪枝，否则 10M 对象 × 数千规则不可行。

**约束（写进 CI）**

1. 二进制中 grep 不到任何资源文件的内容常量（`include_str!`/`include_bytes!` 白名单）。
2. `assets/manifest.json` 缺 `license`/`sha256` 时构建失败。
3. 新增"不支持项"必须同时更新 `docs/known-deviations.md` 与 `unsupported()` 的测试期望。

## 验证

- `elemstyles.mapcss`、12 个核心 `.validator.mapcss`、`defaultpresets.xml` 加载后错误数为 0（警告允许），`unsupported()` 与差异清单一一对应。
- 参考数据集在 zoom 10/13/16/19 与 JOSM 截图九宫格对拍，差异条目 ≤ 5 且每条有理由。
- 12 个核心规则文件里的 `assertMatch` / `assertNoMatch` 全部通过（作为可执行语料，见 [12](../12-validation.md) §7）。
- 级联用例（后覆盖前、`::layer` 排序、`::*` 展开、`+2` 相对值、casing 成对）全部通过。
- Filter 硬契约：被过滤对象在框选、`Ctrl+A`、搜索、hover 四条路径上都不可命中（[13](../13-josm-parity.md) §4 第 7 条）。

## 参考

- JOSM MapCSS 实现（选择器/运算符/`::layer`/不支持项）：<https://josm.openstreetmap.de/wiki/Help/Styles/MapCSSImplementation>
- MapCSS tag checker（规则子句与 `{n}` 占位符）：<https://josm.openstreetmap.de/wiki/Help/Validator/MapCSSTagChecker>
- Tagging presets：<https://josm.openstreetmap.de/wiki/TaggingPresets>
- Editor Layer Index 许可标注（NOASSERTION）：<https://github.com/osmlab/editor-layer-index>
- 待验证：`maps.xsd` 的完整 URL 占位符集合；搜索框 `in`/`bbox` 的 JOSM 正式语义
