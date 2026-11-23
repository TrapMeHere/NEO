# 17 · 许可与治理（Licensing & Governance）

**摘要**：NEO 的许可决策不是一个 `LICENSE` 文件，而是**三件事的组合**：代码许可（决定插件生态与企业采用）、资源许可（决定能否合法拿到 presets/styles/validator 规则这些"用户肌肉记忆"）、以及清洁室纪律（决定我们是否真的拥有自己写下的代码）。结论：代码取 **`Apache-2.0 OR MIT` 双许可**，资源取**运行时独立资产目录 + 逐文件许可 + NOTICE**（经由 JOSM wiki 的 CC-BY-SA/LGPL 路线再获取，而非从 GPL 仓库复制），贡献者用 **DCO** 而非 CLA，治理从 BDFL 过渡到维护者委员会并配 RFC 流程。**本文不是法律意见**：所有条款级结论须由执业律师复核后方可作为发布依据。JOSM 侧的许可事实见 [`.research/josm-inventory.md`](../.research/josm-inventory.md) §1.2–1.3，parity 目标见 [13](./13-josm-parity.md)。

## 1 目标与范围

**在范围内**：NEO 自身代码与文档的许可、第三方资源与依赖的合规、插件的许可模型、商标与命名、项目治理与流程、供应链（SBOM/审计）。

**不在范围内**：OSM 数据本身的许可判定（ODbL 的适用由 OSMF 与各数据用户处理，NEO 只做 attribution 与"不 redistributing 用户数据"）、具体司法辖区的侵权判定、商业实体设立与商标注册申请。

三条不可回避的约束：

| # | 约束 | 来源 | 后果 |
|---|---|---|---|
| C1 | JOSM 是 GPL-2.0-or-later，编译产物整体按 GPLv3 表述；插件不受该许可覆盖 | JOSM `LICENSE` | 任何 JOSM 代码进入 NEO 即传染；功能与行为不受保护 |
| C2 | JOSM 仓库里的资源（presets XML、`elemstyles.mapcss`、12 个 validator mapcss、图标、`.lang`）**无逐文件声明**，随仓库走 GPL | 仓库树 + 文件头检查 | "资源不是代码所以没关系"是错误直觉 |
| C3 | 插件许可需逐个确认，部分插件无许可文件（= 保留所有权利） | 各插件仓库 | 不能把"JOSM 有 233 个插件"当成可移植清单 |

## 2 设计

### 2.1 NEO 自身许可：决策矩阵与结论

评分 1–5（5 最好），加权求和（满分 75）。评分依据写在"理由"列，不写"感觉更好"。

| 准则（权重） | MIT OR Apache-2.0 | Apache-2.0 | MPL-2.0 | GPL-3.0-or-later |
|---|---|---|---|---|
| 与 GPL 资源隔离的合规成本（3） | 4（必须严格资产隔离） | 4 | 4 | 5（可直接内联 JOSM 资源） |
| 插件生态：第三方可否闭源（3） | 5 | 5 | 3（文件级 copyleft，插件边界需谨慎） | 1（插件必须 GPL 兼容） |
| 企业与商业采用（3） | 5 | 4（部分企业只接受 MIT） | 4 | 1 |
| 专利授权与防御（2） | 5（Apache 部分含明确专利授权） | 5 | 4 | 4 |
| 生态惯例与工具链（2） | 5（Rust 生态默认） | 4 | 2 | 2 |
| 未来再许可灵活性（2） | 5（可单方改为 GPL-3.0） | 4 | 3 | 1 |
| **加权总分** | **72** | 65 | 53 | 35 |

**结论：代码 = `Apache-2.0 OR MIT`。** 三点理由：

1. **它让"资源隔离"成为可执行的工程约束，而不是法律祈祷**。因为 NEO 本体不可能吸收 GPL 资源，任何"图省事直接复制 JOSM 文件"的捷径在法律上立刻不可行——这反而是纪律。代价是我们必须把资源当独立资产治理（§2.3），这是我们**本来就应该做**的事。
2. **插件生态需要宿主许可不传染**。WASM 插件宿主若自身是 GPL，几乎所有商业插件作者都会离开；而 MIT/Apache 宿主对插件作者零约束（插件可以是 GPL，也可以是闭源）。
3. **专利条款**。Apache-2.0 部分提供贡献者专利授权与终止条款；纯 MIT 无此保护。双许可让使用者自选，`Apache-2.0 OR MIT` 是 Rust 生态事实上的一致选择。

被否决的 `GPL-3.0-or-later` 的唯一真实优势是**可以合法内联 JOSM 资源并用 JOSM 的规则文件当测试语料**，节省大量资产治理工作；但它同时杀死插件生态与企业采用，且并不能让我们复制 JOSM **代码**（许可证允许，但 clean-room 的目的本来就是拿到可以自主演进的实现）。

**文档与规范**：`docs/` 取 `CC-BY-SA-4.0`（与 JOSM wiki 一致，便于双向搬运文字），代码片段在文档中按代码许可使用。

### 2.2 Clean-room 纪律

GPL 覆盖**代码表达**，不覆盖功能与行为。因此"重写"合法，但必须能证明独立性。四道闸门：

| 闸门 | 具体做法 | 可检查证据 |
|---|---|---|
| 规格先行 | 先写行为规格（本仓 `docs/`），再写实现；规格只引用公开文档、可观察行为、用户可见描述 | 每个 P0 模块都有对应文档章节 |
| 未查阅登记 | 每个模块在 `docs/cleanroom-ledger.md` 记录：行为来源、是否查阅 JOSM 源码、查阅人、复核人、日期 | ledger 覆盖率 100%（§7） |
| 人员分离 | **读过 JOSM 源码的人不得编写对应模块的实现**，只能做规格评审或测试用例 | ledger 的"查阅人 ≠ 实现者"字段 |
| 自查 | 提交时对照"高风险转录清单"（§2.2 表下）逐条确认 | PR 模板勾选项 |

**高风险转录清单**（这些是最容易被"翻译"而不是"重写"的对象）：`OrthogonalizeAction` 的迭代算法、`SimplifyWayAction` 的 Douglas-Peucker 变体及其参数、`ReverseWayTagCorrector` 的 tag 交换表、`OsmApi` 的请求拼装细节、`MapCSS` 解析器的错误恢复策略、validator 的原版 message 文案。**算法本身不受版权保护，但"逐行改写 + 改变量名"是衍生作品。**

> **本仓已经存在一处现实污染源**：`.research/josm-inventory.md` 明确记录它阅读了 `OsmApi.java`、`Command.java`、`OsmWriter.java`、`UploadDialogModel.java` 等源文件。结论：**参与撰写该文档的人不得实现 `neo-api`、`neo-editor` 的 Command 层、`neo-io` 的 XML/OSC 写入**，只能提供规格与测试断言。这不是形式主义——它是唯一能在被质疑时拿出的证据。

### 2.3 资源合规：三条路线与资产目录

JOSM 的资源是"用户肌肉记忆"，parity 必须加载它们，但**不能来自 GPL 仓库**。三条法律上不同的路线：

| 路线 | 内容 | 义务 | 采用 |
|---|---|---|---|
| A 从 `openstreetmap/josm` 或 `josm-tested.jar` 复制 | 与 JOSM 完全一致 | 该文件 GPL-2.0+（编译产物整体 GPLv3）→ 整个组合作品须 GPL 兼容并随附全部源码 | ❌ 与 §2.1 冲突 |
| B 从 JOSM wiki 再获取**同样内容**（`TaggingPresets`、`Styles`、`Rules/*` 等页面） | 同样的 XML/MapCSS 文本，wiki 许可 | 页脚：**CC-BY-SA + 2014-04 之后亦为 LGPL**。作为独立数据资产分发，附署名、许可声明与该文件自身来源，不静态并入 GPL 派生代码 | ✅ 主路线 |
| C 依据 OSM tagging 文档重写 | 自有的文件 | 只需遵守 OSM wiki 文本的 CC-BY-SA（改写与事实不受限） | ✅ 补充（尤其图标与小规则） |

路线 B 的工程化要求（缺一即回到路线 A 的风险）：

```
assets/                          ← 唯一允许存放第三方内容的地方；不参与 cargo build
├── MANIFEST.toml                ← 逐文件：来源 URL、上游版本、SPDX、sha256、获取日期、是否修改
├── NOTICE                       ← 汇总署名（JOSM 贡献者、OSM 贡献者、字体/图标作者）
├── LICENSES/
│   ├── LGPL-2.1-or-later.txt
│   ├── CC-BY-SA-4.0.txt
│   ├── OFL-1.1.txt
│   └── Apache-2.0.txt
├── presets/defaultpresets.xml   ← 未修改；若修改则在 MANIFEST 标注 derived 并整体 CC-BY-SA
├── styles/elemstyles.mapcss
├── validator/{addresses,combinations,deprecated,geometry,highway,multiple,
│              numeric,relation,religion,territories,unnecessary,wikipedia}.validator.mapcss
├── boundaries.osm               ← territory 选择器依赖；ODbL 内容，需 OSM 署名
├── icons/                       ← 自绘或 CC0/Apache-2.0/ISC 集，逐文件标注
└── fonts/                       ← OFL-1.1（如 Noto Sans / JetBrains Mono）
```

硬规则：

1. **资产不进二进制**：资源以运行时目录加载（`--assets-dir`，默认随安装包解压到用户数据区）。这样"独立资产"不只是说法，而是链接边界上的事实。
2. **未修改优先**：改动即产生 CC-BY-SA 的"改编"，该文件必须继续 CC-BY-SA——可以接受，但要让 MANIFEST 显式标注 `modified = true`，否则我们会误以为它是 LGPL-only。
3. **图标是独立的坑**：presets XML 用 `icon="presets/..."` 等路径引用图标名，**图标名是数据契约**（引用关系不构成对图像的复制），但**图像文件本身不是**。做法：自绘一套 JOSM 同名图标（或使用 CC0/Apache-2.0 图标集映射到同名），缺失时回退通用图标并统计覆盖率。1385 SVG + 884 PNG 不可能一次画完，按 P0 preset 的引用频次做前 200 个。
4. **ELI 不可假定 CC0**：GitHub 对 Editor Layer Index 报 `NOASSERTION`。因此**不打包 ELI**；内置影像条目逐条核实 attribution 与使用条款，其余由用户在偏好里自行启用（自行接受条款），默认关闭。
5. **NTv2 `.gsb` 网格有独立使用条款**（JOSM 自己在 `nodist/data/projection/CHENyx06-Distribution.pdf` 里放了说明）——投影 parity 若要含 NTv2，必须单独评审。
6. **`.lang` 翻译**：48 个文件的贡献者条款未经核实（> 待验证）；NEO 初期只用英文 + 自建翻译流程（Crowdin/Weblate），不从 JOSM 复制译文。
7. **REUSE 合规**：每个资产文件配 `SPDX-FileCopyrightText`/`SPDX-License-Identifier` 头或同名 `.license` 旁文件，CI 跑 `reuse lint`。

### 2.4 插件许可模型

WASM 插件与宿主是**边界清晰的独立程序**（无链接、无共享地址空间、仅经文档化 ABI 交互），因此宿主许可（MIT/Apache）不对插件施加约束；反之，把 GPL 插件**移植**成 WASM 后它仍是 GPL，且不得把 NEO 的 MIT 代码抄进去再宣称闭源（MIT 允许，但移植后的作品整体仍是 GPL）。

插件 manifest 必填许可与权限：

```toml
[plugin]
id            = "org.example.buildingtools"
version       = "0.3.1"
license       = "GPL-3.0-or-later"     # SPDX 表达式；缺失即拒绝加载
source_url    = "https://…"            # 对应源码位置（GPL 插件的分发义务）

[permissions]                           # 默认全拒，逐项申请
network       = ["api.openstreetmap.org"]   # 域名白名单，非通配
fs_scope      = ["$DATA/plugins/org.example.buildingtools"]
editor        = ["read", "write"]       # 写必须经 Command，故仍可撤销
ui            = ["panel", "menu:data"]  # Data 菜单是插件宿主，见 13 §3.1
limits        = { fuel = 50_000_000, memory_mb = 256, wall_ms = 2_000 }
```

治理要求：

| 要求 | 理由 |
|---|---|
| `license` 必填且为 SPDX 表达式 | 无许可文件的上游插件 = 保留所有权利，不能移植；我们在移植前必须逐项确认 |
| 分发 GPL 插件时必须提供其源码 | GPL 义务随插件走，宿主不代为履行；插件市场需提供源码链接字段 |
| 权限最小化 + 用户可见 | 与许可无关但同属"生态可信度"；也是沙箱的技术前提（[15](./15-plugin-system.md)） |
| 不提供 JOSM 插件二进制加载路径 | 技术上不可行（依赖 Java 内部类型），法律上也无必要 |

### 2.5 商标与命名

| 对象 | 规则 | 具体做法 |
|---|---|---|
| "OpenStreetMap" / "OSM" | OSMF 的注册/普通商标；描述性使用允许，暗示官方认可不允许 | 产品主标只用 **NEO**；副标题用描述性措辞"an editor for OpenStreetMap data"；**不**把 "OSM" 作为产品名首个词（避免 "OSM Editor NEO" 这类） |
| OSM 徽标 | 不得使用 | 不用 OSM logo 做应用图标或启动画面 |
| JOSM 名称/徽标 | 不得使用其徽标；名称可作**事实性指称** | 文档中写"行为对齐 JOSM r19678"并附免责声明："NEO 与 JOSM 项目及 OpenStreetMap Foundation 无隶属关系，未获其背书" |
| 数据署名 | ODbL 要求 | 地图角落常驻 "© OpenStreetMap contributors"；导出的图片/文件同样带署名；"produced work" 声明见 §3 |
| 域名/包名 | 不与官方混淆 | 不使用 `openstreetmap.*`、`josm.*` 命名空间 |

> 待验证：OSMF 商标政策的条款级措辞与"基于 OpenStreetMap"的允许边界，需律师与 OSMF 确认后方可定稿产品名与宣传语。

### 2.6 治理

| 阶段 | 触发条件 | 结构 | 决策方式 |
|---|---|---|---|
| Phase 0 BDFL（当前） | 项目启动 | 1 名所有者 + 模块维护者 | 所有者裁决，RFC 记录 |
| Phase 1 委员会 | ≥3 名外部贡献者或首个 stable 发布 | 3–5 人维护者委员会（含 1 名 release manager、1 名 security officer） | 惰性共识 + 投票，重大变更需 RFC |
| Phase 2 基金会托管（可选） | 出现商业依赖或商标争议 | 独立法人/基金会 | 章程化 |

流程要素：

- **RFC**：`docs/rfc/NNNN-title.md`，模板含动机/规格/替代方案/兼容性影响/许可影响；公示 ≥7 天，惰性共识（无实质反对即通过）；架构级决策落 `docs/adr/`（本仓已有 `docs/adr/`）。
- **发布节奏**：nightly（每次合入）、**每月一个 stable**（对齐 JOSM 的月度 tested 节奏，便于 parity 对拍）、每年一个 LTS（会话/格式兼容承诺）。
- **安全响应**：私密 GHSA 通道 + `security@` 邮箱；确认后 90 天披露；依赖漏洞每周 `cargo-audit`；解析器崩溃类问题按安全级别处理（见 [18](./18-testing-strategy.md) §2.8 的 fuzz）。
- **贡献者**：DCO 1.1（`git commit -s`）而非 CLA——理由见 §5。DCO 无法覆盖"未来改许可"的需求，因此若将来要做 GPL 双轨，须提前引入 CLA 并取得**全部**历史贡献者同意（这是选择 DCO 的已知代价）。

## 3 关键矩阵

### 3.1 SPDX 一览（发布制品 × 许可）

| 制品 | SPDX | 位置 | 分发方式 | 备注 |
|---|---|---|---|---|
| `crates/*`, `apps/*` | `Apache-2.0 OR MIT` | workspace | 二进制 + 源码 | 专利授权来自 Apache 分支 |
| `docs/` | `CC-BY-SA-4.0` | 仓库 | 随仓库 | 便于与 wiki 双向搬运 |
| presets / styles / validator 规则 | `LGPL-2.1-or-later`（源自 wiki，另 CC-BY-SA-4.0） | `assets/` | 独立资产目录 | 运行时加载，不编译 |
| 领土边界 `boundaries.osm` | `ODbL-1.0` | `assets/` | 独立资产 | 需 OSM 署名 |
| 图标 | `CC0-1.0` / `Apache-2.0` / `ISC` 逐文件 | `assets/icons/` | 独立资产 | 自绘或兼容集 |
| 字体 | `OFL-1.1` | `assets/fonts/` | 独立资产 | 保留字体名条款，不得单售字体 |
| 测试 fixture（合成） | `CC0-1.0` | `tests/fixtures/` | 随仓库 | 生成器可复现 |
| 测试 fixture（Monaco 提取） | `ODbL-1.0` | 不入 git（下载脚本） | 按需下载 | 需署名，见 [18](./18-testing-strategy.md) §3.4 |
| 插件（第三方） | 各自 SPDX | 插件市场 | 独立分发 | manifest 必填 |

### 3.2 依赖许可策略（供应链）

| 策略 | 工具 | 门禁 |
|---|---|---|
| 允许列表 | `cargo-deny` `[licenses]`：`MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, MPL-2.0, CC0-1.0` | 出现 GPL/AGPL/SSPL 即 CI 失败（MPL 需按 crate 评审） |
| 漏洞 | `cargo-audit`（每周 + 每次 lockfile 变更） | 已知可利用漏洞阻断发布 |
| SBOM | `cargo-sbom` / CycloneDX 输出随 release 附件 | 每次 stable 发布必须附 |
| 归属 | `cargo-about` 生成 `THIRD-PARTY.md` | 与 release 一同分发 |

## 4 流程与时序

### 4.1 贡献者合规流水线

```
PR 提交（含 Signed-off-by）
   │
   ├─ ① DCO 检查（每 commit 一个 -s，作者与提交者一致）
   ├─ ② REUSE lint（每个新文件有 SPDX 头；assets 有 .license 旁文件）
   ├─ ③ cargo-deny（许可允许列表 + 漏洞 + 重复版本）
   ├─ ④ clean-room 声明：PR 模板必答
   │     · 本变更是否参考了 JOSM 源码？ → 是：必须由未查阅者重写或改为规格
   │     · 是否是 AI 辅助生成？ → 是：填 `Assisted-by:` trailer（模型/用途）
   │     · 新增资源？ → 附 MANIFEST 条目（来源 URL + SPDX + sha256）
   └─ ⑤ 人工评审（模块维护者）+ ledger 更新
   ▼
  merge（squash 保留 Signed-off-by）
```

### 4.2 发布合规检查（每次 stable 必做）

```
[ ] reuse lint 100%              [ ] cargo-deny 通过（含 advisory）
[ ] SBOM + THIRD-PARTY.md 生成    [ ] assets/MANIFEST.toml 无缺项
[ ] 安装包内不含 assets/ 之外的第三方文件
[ ] 二进制符号扫描：不含 GPL 声明（SBOM 反查）
[ ] cleanroom-ledger 覆盖全部 P0 模块
[ ] 商标使用复查（应用名/图标/关于页/商店页文案）
[ ] 地图署名与导出文件署名抽查
```

## 5 权衡与替代方案

| 决策 | 选择 | 被否决项 | 理由 / 代价 |
|---|---|---|---|
| 代码许可 | `Apache-2.0 OR MIT` | GPL-3.0（可内联 JOSM 资源，省掉整个资产治理） | 省下的工作量 < 失去的插件生态与企业采用；代价是资产治理成为长期成本 |
| 资源来源 | wiki 再获取 + 运行时目录 | 从 GPL 仓库复制 | 后者使 NEO 成为 GPL 衍生作品，直接否决双许可方案 |
| 贡献者协议 | DCO | CLA | DCO 零摩擦、易被个人贡献者接受；代价是**无法单方面改许可**，将来若要做 GPL 双轨需全员重新授权 |
| 图标 | 自绘/兼容集 + 名称映射 | 复用 JOSM 图标 | 复用需 GPL；代价是前 200 个图标的重绘工作量与视觉差异 |
| AI 政策 | 允许 boilerplate 与测试脚手架，禁止核心算法 | 全面允许 / 全面禁止 | 全面允许有许可与可版权性风险；全面禁止不现实。代价是需要人工判断边界与 trailer 记录 |
| 翻译 | 自建管线 | 复用 48 个 `.lang` | 上游译文贡献者条款未核实；代价是初期本地化覆盖低 |

## 6 风险

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| L1 | "同一内容"论证被质疑（wiki 版与仓库版被认定实质相同但许可不同） | 被迫 GPL 化或下架资源 | 只从 wiki 页面获取、记录 URL+日期+sha256；不改用仓库文件；律师复核留档 |
| L2 | wiki 版本落后于 JOSM 发行版 → parity 断言失败 | 被误判为 bug | MANIFEST 记录上游版本；对拍时同时比对 JOSM 实际加载结果与 wiki 版本差异（[18](./18-testing-strategy.md) §2.4） |
| L3 | AI 辅助代码复现了 JOSM 片段 | 传染风险 + 无法主张版权 | 禁止 AI 生成核心算法；`Assisted-by:` trailer；可疑片段重写 |
| L4 | 贡献者污染（读过 JOSM 源码的人写了对应实现） | 整个模块的独立性可被质疑 | ledger + 人员分离 + 代码评审；本仓 inventory 的读者被显式排除在 `neo-api`/`Command`/`neo-io` 写入模块之外 |
| L5 | 商标投诉（产品名含 OSM/JOSM 暗示） | 被迫改名 | 主标 NEO；文案送律师与 OSMF 预审；商店页加免责声明 |
| L6 | 插件许可雷区（上游无 LICENSE = 保留所有权利） | 移植即侵权 | 每族开工前许可确认；`license` 必填校验 |
| L7 | ELI / NTv2 / 字体条款 | 打包即违规 | 不打包 ELI；NTv2 单独评审；字体只用 OFL/Apache |
| L8 | 依赖引入 GPL（传递依赖） | 传染 | `cargo-deny` 允许列表 + 每次 lockfile 变更审计 |

## 7 验收标准

1. `reuse lint` 在干净工作区 100% 通过；`assets/` 下每个文件在 `MANIFEST.toml` 有且仅有一条记录（数量断言相等）。
2. `cargo-deny` 的许可检查对 workspace 全部 crate 与依赖通过，输出中不存在 GPL/AGPL/SSPL。
3. 发布二进制/安装包的 SBOM 中不含任何 GPL 组件；`assets/` 以外的第三方文件数为 0（脚本断言）。
4. `docs/cleanroom-ledger.md` 覆盖全部 P0 模块（[13](./13-josm-parity.md) §2.4 的 52 条），每条含"查阅人 / 实现者 / 复核人"，且查阅人 ≠ 实现者。
5. 最近 50 个合并提交中 Signed-off-by 覆盖率 100%，AI 辅助提交均带 `Assisted-by:`。
6. 应用名、图标、"关于"页、商店文案通过商标复查清单；地图与导出文件均含 OSM 署名。
7. 任一插件加载时若 manifest 缺 `license` 或申请未声明权限，加载被拒绝（负例测试通过）。

## 8 参考

- JOSM `LICENSE`（GPL-2.0-or-later，含 Apache-2.0 混合与插件豁免表述）：https://raw.githubusercontent.com/JOSM/josm/master/LICENSE
- JOSM wiki 页脚许可声明（CC-BY-SA + 2014-04 起 LGPL）：https://josm.openstreetmap.de/wiki/TaggingPresets
- JOSM 规则集分发（`Rules/*` wiki 路线）：https://josm.openstreetmap.de/wiki/Rules
- GPL-2.0 全文：https://www.gnu.org/licenses/old-licenses/gpl-2.0.html
- Apache-2.0 全文：https://www.apache.org/licenses/LICENSE-2.0
- MIT 全文（SPDX 文本）：https://spdx.org/licenses/MIT.html
- CC-BY-SA 4.0：https://creativecommons.org/licenses/by-sa/4.0/legalcode
- LGPL-2.1：https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html
- SIL OFL 1.1（字体，含保留字体名条款）：https://openfontlicense.org/
- ODbL 1.0 与 OSM 署名指引：https://www.openstreetmap.org/copyright
- OSMF 商标政策：https://wiki.osmfoundation.org/wiki/Trademark_Policy
  > 待验证：条款级措辞与"基于 OpenStreetMap"的允许边界，须律师/OSMF 确认。
- DCO 1.1：https://developercertificate.org/
- REUSE 3.x 规范：https://reuse.software/spec/
- `cargo-deny`：https://embarkstudios.github.io/cargo-deny/
- `cargo-audit`：https://rustsec.org/
- 一手许可事实（资源清单、图标/字体/ELI/NTv2 的许可状态）：[`.research/josm-inventory.md`](../.research/josm-inventory.md) §1.2–1.3
