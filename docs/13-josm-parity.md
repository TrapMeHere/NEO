# 13 · JOSM 行为对齐契约（JOSM Parity）

**摘要**：本文把"完整替代 JOSM"定义成**可验证的行为契约**，而不是功能计数。唯一验收语句是：*一名活跃的 OSM 制图者可以连续一周只用 NEO 工作、不打开 JOSM、不改变既有习惯、且不损坏任何数据*。为使其可判定，本文给出 (a) 菜单/命令级 parity 矩阵（含 9 种工作模式、面板、双键快捷键序列）、(b) 数据与格式 parity、(c) 上传与冲突工作流 parity、(d) 插件能力映射、(e) 10 条"必须为真"的完成判据与 8 个最易漏掉的行为、(f) 明确"不承诺"的范围。JOSM 侧事实取自 [`.research/josm-inventory.md`](../.research/josm-inventory.md)，基线 **r19678 "26.09"（2026-11-05）**。规则语义见 [12 验证器](./12-validation.md)，格式解析见 [08](./08-io-pbf-xml-osc.md) 与 [10](./10-persistence-and-recovery.md)，许可与 clean-room 见 [17](./17-licensing-and-governance.md)，证据如何产生见 [18](./18-testing-strategy.md)。

## 1 目标与范围

### 1.1 "完整替代"的操作性定义

| 判据 | 含义 | 反例（不算达成） |
|---|---|---|
| 习惯保持 | 默认快捷键、菜单位置、面板停靠行为不变 | "功能都有，但下载挪到了新菜单" |
| 数据无损 | 打开→编辑→保存→上传→重新下载，语义逐字段等价 | 存盘丢掉 `action="modify"`，重开被当成未修改 |
| 闭环完整 | 编辑→校验→上传→409→解决→重传→关闭 changeset 全在 NEO 内 | 冲突时提示"请用 JOSM 解决" |
| 键盘优先 | 每个动作都有默认快捷键且可重绑，含双键序列 | 只有鼠标可达的功能 |
| 可退出 | 一周内无需回退到 JOSM | "除了某类 relation，其他都能做" |

功能计数（"197 个 action 我们实现了 150 个"）**不是证据**：分母无意义，且 JOSM 的价值大半在 `DataSet`/`Command` 语义与资源（presets/styles/rules）里。

**在范围内**：JOSM 核心的全部用户可见行为——菜单、9 种工作模式、面板、格式、上传/冲突、会话与 autosave、影像与偏移、无头 CLI 等价物；以及 2 个 P0 与 8 个 P1 插件的**能力**映射。

**不在范围内**：JVM 脚本插件、Java 插件二进制兼容、OSM API 0.7（v0.7 仅存在于 OSM wiki 构想页，无部署规范），详见 §3.7。

## 2 设计

### 2.1 矩阵字段与优先级语义

每个 parity 条目是一个**行为单元**而非 UI 控件：一个条目可对应多个菜单项（如"更新数据/更新选中/更新已修改"共享同一套重新下载与合并语义）。

| 列 | 含义 |
|---|---|
| 项（快捷键） | JOSM 的菜单标签与默认快捷键；双键序列写作 `A A` |
| NEO 模块 | 主要实现 crate（`apps/neo-desktop` 只做装配） |
| P | **P0** = 缺少它"一周不开 JOSM"就不成立；**P1** = 日常路径，缺失会被抱怨但可绕过；**P2** = 长尾/专家选项 |
| 状态 | **设计**＝行为契约与接口已落定；**骨架**＝crate 边界与类型存在、逻辑为桩；**原型**＝端到端可运行的最小实现；**未开始**＝尚无契约 |

> **状态快照（2026-11-15）**：`crates/*` 仅有目录占位，无实现，故本表几乎全为"设计"；"骨架"与"原型"当前为空集。矩阵同时是交付看板。
>
> **2026-11-21 更新**：数据/编辑/关系/校验/同步/持久化主链已落地；真实完成度与 iD/JOSM
> 差距见 **[feature-parity.md](feature-parity.md)**。本文保留为 JOSM 行为契约与长期路线，
> 不再作为实现状态看板。

### 2.2 优先级判定规则

1. 是否在"编辑→上传"主链路上（下载/编辑/校验/上传/冲突）？→ P0。
2. 缺失是否导致**数据损坏或上传失败**？→ P0（例：反向 way 不修正方向性 tag）。
3. 是否被 JOSM 文档标为 expert、或只服务单一人群（audio/cadastre/print）→ P1/P2；是否属于"用户肌肉记忆即资源"（preset 树、默认样式、9 模式）→ P0。

### 2.3 如何验证 parity 本身

| 手段 | 覆盖 | 频率 |
|---|---|---|
| 工作流脚本（§4.1） | 端到端闭环、习惯保真 | 每里程碑 + P0 变更 |
| golden 对拍（[18](./18-testing-strategy.md) §2.3） | 同一输入 → 对象集/OSC 语义等价 | 每 PR |
| 与 JOSM 无头 CLI 双跑 | `render`/`validate` 输出的归一化差异 | nightly（黑盒调用，不链接代码） |
| 人工 dogfood 周报 | 状态列推进与习惯断裂 | 每周 |

### 2.4 P0 汇总（与 §3 各表逐行一致）

| 分区 | 条目 | P0 | P1 | P2 |
|---|---|---|---|---|
| 菜单/命令/模式/面板（§3.1） | 55 | **35** | 14 | 6 |
| 数据/格式（§3.2） | 13 | **9** | 3 | 1 |
| 上传/冲突（§3.3） | 7 | **6** | 1 | 0 |
| 插件能力（§3.4） | 8 | **2** | 5 | 1 |
| 合计 | 83 | **52** | 23 | 8 |

**命令级 P0 = 35**（"一周不开 JOSM"的最小可判定集）；**全口径 P0 = 52**（含格式、上传、插件能力）。两个数字都可用，报告进度时须注明口径。

## 3 关键矩阵

### 3.1 菜单 / 命令 / 模式 / 面板 parity

**File（Alt+F）—— 下载与上传在这里，不在 Data**

| 项（快捷键） | NEO 模块 | P | 状态 |
|---|---|---|---|
| 打开…/打开位置…/最近（`Ctrl+O`,`Ctrl+L`） | neo-io · neo-ui | P0 | 设计 |
| 保存/另存（`Ctrl+S`,`Ctrl+Shift+S`） | neo-io · neo-persist | P0 | 设计 |
| 下载数据…/当前视图/对象/父 way-relation（`Ctrl+Shift+↓`,`Ctrl+Shift+O`,`Ctrl+Alt+D`） | neo-api · neo-ui | P0 | 设计 |
| 更新数据/选中/已修改（`Ctrl+U`,`Ctrl+Alt+U`,`Ctrl+Alt+M`） | neo-api · neo-editor | P0 | 设计 |
| 上传数据/上传选中（`Ctrl+Shift+↑`,`Ctrl+Alt+Shift+U`） | neo-api · neo-editor | P0 | 设计 |
| 保存会话/另存（`Ctrl+Alt+S`,`Ctrl+Alt+Shift+S`） | neo-persist | P0 | 已完成（桌面端按钮；快捷键待接） |
| 新建图层/新建本地（`Ctrl+N`,`Ctrl+Shift+N`）/下载沿路径（`Alt+Shift+D`）/关闭 changeset（`Ctrl+Alt+Q`）/删除图层（`Ctrl+F4`）/离线/退出 | neo-ui · neo-api | P1 | 设计 |
| 导出 GPX…（`Ctrl+E`） | neo-io | P2 | 未开始 |

**Edit（Alt+E）—— 核心很小：撤销/剪贴板/合并/搜索/偏好**

| 项（快捷键） | NEO 模块 | P | 状态 |
|---|---|---|---|
| 撤销/重做（`Ctrl+Z`,`Ctrl+Y`） | neo-editor | P0 | 设计 |
| 复制/粘贴/粘贴到源位置/粘贴标签（`Ctrl+C`,`Ctrl+V`,`Ctrl+Alt+V`,`Ctrl+Shift+V`） | neo-editor | P0 | 设计 |
| 复制（Duplicate，`Ctrl+D`）/删除（`Del`） | neo-editor | P0 | 设计 |
| 搜索…（`Ctrl+F`）/按 preset 搜索（`Shift+F3`） | neo-style · neo-ui | P0 | 设计 |
| 偏好设置（`F12`） | neo-ui | P0 | 设计 |
| 清除（Purge，`Ctrl+Shift+P`，expert）/合并图层·合并选中（`Ctrl+M`,`Ctrl+Shift+M`） | neo-editor | P1 | 设计 |
| 复制坐标/复制服务器 URL | neo-ui | P2 | 未开始 |

**View（Alt+V）与 Selection（Alt+N）**

| 项（快捷键） | NEO 模块 | P | 状态 |
|---|---|---|---|
| 缩放族：放大/缩小/到数据/图层/选中/冲突/下载/问题/上一下（`+`,`-`,`1`–`6`,`8`,`9`） | neo-ui · neo-scene | P0 | 设计 |
| Map Paint 样式开关（含默认 elemstyles） | neo-style · neo-scene | P0 | 设计 |
| 下载区域边界显示 + 越界禁止编辑（hatched 语义） | neo-ui · neo-editor | P0 | 设计 |
| Expert mode 开关（**gating 菜单项与工具栏**，非装饰） | neo-ui | P0 | 设计 |
| 全选/取消全选/反选（`Ctrl+A`,`Esc`） | neo-editor | P0 | 设计 |
| 高级信息/Web 高级信息/History（`Ctrl+I`,`Ctrl+Shift+I`,`Ctrl+H`） | neo-ui · neo-api | P1 | 设计 |
| 非分支 way 序列选择（`Shift+W`） | neo-tools | P1 | 未开始 |
| 连线框/瓦片渲染/网格/视口跟随（`Ctrl+Shift+F`）/全屏（`F11`）/对话框面板（`Tab`）/共享子对象/按内部点选择 | neo-render · neo-ui | P2 | 未开始 |

**Mode（Alt+M）—— 9 种工作模式与鼠标语义**

| 模式（快捷键） | 关键鼠标/修饰语义 | NEO 模块 | P | 状态 |
|---|---|---|---|---|
| Select 矩形（`S`） | 点选/框选，拖动=移动，`Ctrl` 追加 | neo-tools | P0 | 设计 |
| Lasso 自由（`S S`） | 同上，自由手绘多边形 | neo-tools | P0 | 设计 |
| Draw（`A`；`A A` 角度捕捉） | 逐点画 way，捕捉既有节点/线段 | neo-tools | P0 | 设计 |
| Zoom 矩形 | 框选缩放，`Alt` 反向 | neo-ui | P0 | 设计 |
| Delete（`Ctrl+Del`） | 删对象**或** way 的一段（分割语义） | neo-tools | P0 | 设计 |
| Parallel（`Shift+P`） | 画的同时生成平行副本（道路复制） | neo-tools | P1 | 未开始 |
| Extrude（`X`；`X X` 双对齐） | 拉伸/调整线段 | neo-tools | P1 | 未开始 |
| Improve Way Accuracy（`W`） | 拖动既有节点、增删节点（带预览） | neo-tools | P1 | 未开始 |
| Split（`T`，r18759 起） | 在点处分割 way | neo-tools | P1 | 未开始 |

**Tools（Alt+T）—— 几何核心在这里**

| 项（快捷键） | NEO 模块 | P | 状态 |
|---|---|---|---|
| 分割 way（`P`）/合并 way（`C`） | neo-tools · neo-editor | P0 | 设计 |
| 反向 way（`R`）**+ 方向性 tag 修正** | neo-tools | P0 | 设计 |
| 正交化（`Q`） | neo-tools | P0 | 设计 |
| 合并节点（`M`）/连接节点到 way（`J`）/移动节点到 way（`N`）/断开（`Alt+J`）/解胶合（`G`） | neo-tools | P0 | 设计 |
| 创建/更新 multipolygon（`Ctrl+B`,`Ctrl+Shift+B`） | neo-tools · neo-editor | P0 | 设计 |
| 简化 way（`Shift+Y`）/对齐成圆·线（`O`,`L`）/分布节点（`Shift+B`）/创建圆（`Shift+O`）/合并重叠面（`Shift+J`） | neo-tools | P1 | 未开始 |
| 镜像（`Shift+M`）/跟随线段（`F`，expert）/添加节点（`Shift+D`）/移动节点 | neo-tools · neo-ui | P2 | 未开始 |

**Data（Alt+D）与 Imagery（Alt+I）**

| 项（快捷键） | NEO 模块 | P | 状态 |
|---|---|---|---|
| Data 菜单：**零核心项，纯插件宿主**（reverter/buildings/utilsplugin2 占用） | neo-plugin · neo-ui | P1 | 设计 |
| TMS/WMS/WMTS 图层 + 5 个内置影像（Bing/Esri/Esri Clarity/Mapbox/OSM Carto） | neo-render · neo-ui | P0 | 设计 |
| New offset（调整对话框）+ **每个影像 id 的偏移持久化** | neo-ui · neo-persist | P0 | 设计 |
| 获取/存储影像偏移（`Ctrl+Alt+I`，offset DB 协议，XML 非 JSON） | neo-api | P1 | 未开始 |
| 影像偏好/图层重排/按 bounds+zoom 过滤的上下文列表 | neo-ui | P1 | 未开始 |

**Windows（Alt+W）—— 面板清单**

| 面板（快捷键） | NEO 模块 | P | 状态 |
|---|---|---|---|
| Layers（`Alt+Shift+L`） | neo-ui | P0 | 设计 |
| Tags/Memberships（`Alt+Shift+P`） | neo-ui | P0 | 设计 |
| Selection（`Alt+Shift+C`） | neo-ui | P0 | 设计 |
| Relations（`Alt+Shift+R`） | neo-ui · neo-editor | P0 | 设计 |
| Conflict（JOSM 文档与 Selection 同键，NEO 需自定唯一键） | neo-ui · neo-editor | P0 | 设计 |
| Validator Results（`Alt+Shift+V`，含定位/前后跳转/修复） | neo-ui · neo-validate | P0 | 设计 |
| Filter（`Alt+Shift+F`，E/H/I/M + R/A/D/F 语义） | neo-style · neo-ui | P0 | 设计 |
| Map Paint Styles（`Alt+Shift+M`）/Command Stack（`Alt+Shift+O`）/Changeset Manager（`Ctrl+Alt+C`） | neo-ui · neo-api | P1 | 设计 |
| Notes / Mini Map / Authors / Geotagged Images（`Y`） | neo-ui | P2 | 未开始 |

**Help（Alt+H）**

| 项 | NEO 模块 | P | 状态 |
|---|---|---|---|
| 菜单项搜索（模糊找功能）/快捷键参考页 | neo-ui | P1 | 未开始 |
| 内嵌帮助/状态报告/报告 bug/关于 | neo-ui · neo-core | P2 | 未开始 |

### 3.2 数据与格式 parity

| 格式/能力 | 契约要点 | NEO 模块 | P | 状态 |
|---|---|---|---|---|
| OSM XML 0.6 读 | `bounds`、负 id、**JOSM 私有 `action="modify\|delete"`** | neo-io | P0 | 设计 |
| OSM XML 写 | 两种模式：server-conformant 与 JOSM-conformant（`visible`/`action`/`changeset`） | neo-io | P0 | 设计 |
| PBF 读 | `DenseNodes`、`granularity`/`date_granularity`、组内不混类型、`HistoricalInformation` 拒绝或显式降级 | neo-io | P0 | 设计 |
| OsmChange 0.6 读/写 | 外层 `osmChange version="0.6"`、create/modify/delete、`if-unused`、负 id 占位（**无 0.3 规范**） | neo-io | P0 | 设计 |
| `.jos`/`.joz` 会话 | XML 与 ZIP 两种容器；容忍未知插件图层段 | neo-persist | P0 | 已完成（最小兼容：图层引用/viewport/selection；imagery 参数段忽略） |
| autosave | 默认 300 s、`filesPerLayer=1`、启动恢复提示、删除图层备份 | neo-persist | P0 | 设计 |
| presets XML | 未修改的 `defaultpresets.xml`（660 KB）可加载：`group/item/chunk/reference`、`key/text/combo/multiselect/roles`、`type` 组合（`multipolygon` 不属于 `relation`） | neo-style | P0 | 设计 |
| MapCSS 渲染 | 未修改的 `elemstyles.mapcss` 可加载：`::layer`、`z-index`、伪类全集、`∈` 系运算符 | neo-style · neo-scene | P0 | 设计 |
| validator mapcss | 12 个核心文件可加载并产出等价 issue（[12](./12-validation.md) §3.1） | neo-validate | P0 | 设计 |
| PBF 写 | 导出 `.osm.pbf`（JOSM 核心只读，写成插件能力；NEO 内置更合理） | neo-io | P1 | 未开始 |
| GPX / 投影 | GPX 读写；EPSG 定义、NTv2 `.gsb`（**有独立使用条款**） | neo-io · neo-geo | P1 | 未开始 |
| 影像偏移 XML | 查询串式 `get?lat=&lon=`/`store?…`，返回 `<offset>` XML | neo-api | P1 | 未开始 |
| 翻译 `.lang` | 48 个 `.lang` 的 key 兼容；缺失回退英文 | neo-core | P2 | 未开始 |

### 3.3 上传与冲突工作流 parity

| 项 | 契约要点 | P | 状态 |
|---|---|---|---|
| changeset 生命周期 | 懒创建、上传期间复用、关闭（`Ctrl+Alt+Q` 关闭全部打开的） | P0 | 设计 |
| changeset 标签 | `comment`（≤255、过短校验、`Ctrl+Enter` 提交）+ `source`（可自动获取）+ **从 comment 提取 `hashtags`**（默认开） | P0 | 设计 |
| hook 链顺序 | `ApiPreconditionChecker → DiscardTags → FixData → ValidateUpload → RelationUploadOrder`（新 relation 最后） | P0 | 设计 |
| `diffResult` | 逐元素 `<node old_id new_id new_version>`；把负占位 id 改写为真实 id | P0 | 设计 |
| 409 事务语义 | 服务器**在第一个错误处中止整个事务**，故须重新下载受影响对象后合并，不能重试；另一种 409 = changeset 已关闭 | P0 | 设计 |
| 冲突解决 UI | 四维度（Properties/Tags/Nodes/Members）三列（My/Merged/Their）+ 列表冲突的 **Freeze** + 全绿才可 `Apply Resolution` | P0 | 设计 |
| 服务端限制 | 与上传同批处理：`capabilities`（waynodes 2000 / members 32000 / 点集 10000）、413（bbox 超限）、429（限流）分别给出可操作提示 | P1 | 设计 |

### 3.4 插件 parity 策略

JOSM 插件**无法**字节码兼容：它们编译依赖 `OsmPrimitive`/`MapMode`/`Command`/`MainApplication`/`Config` 等 Java 内部类型。NEO 的"插件兼容"只能意味着**能力重实现 + 等价扩展 API**（WASM，见 [15](./15-plugin-system.md)）。

| 能力（JOSM 插件） | 映射到 NEO | P | 许可需单独确认 |
|---|---|---|---|
| 建筑绘制（`buildings_tools`：矩形/圆形、`Alt+R`/`Alt+Z`/`Ctrl+Alt+B`、合并地址点） | neo-tools 核心能力（建筑描图是主活动，不能是可选插件） | P0 | 是 |
| 杂项工具集（`utilsplugin2`：More Tools、多对象打 tag `Ctrl+T`、对齐 way） | neo-tools "More Tools" 模块 | P0 | 是 |
| `terracer` 排屋 / `FastDraw` 鼠标快画 / `measurement` 长度·角度·面积 | neo-tools | P1 | 是 |
| `pt_assistant` 公交 schema v2 校验与修复 | neo-validate 规则包（[12](./12-validation.md) VAL-S03） | P1 | 是 |
| `reverter` 回滚 changeset | neo-api + neo-editor（依赖 changeset 下载与冲突解决） | P1 | 是 |
| `reltoolbox` relation/multipolygon 编辑面板 | neo-ui + neo-editor | P1 | 是 |
| `PicLayer` 图片底图对齐 / `imagery_offset_db` 偏移共享 | neo-render + neo-api | P1 | 是 |
| `scripting`/`todo`/`turnlanes`/`CommandLine`/`o5m`/`print`/`opendata` 长尾 | 按需；**绝不承诺 `scripting`** | P2 | 是 |

**结论**：口径是"P0 两族能力进核心 + P1 八项能力进能力包"，而不是"支持 233 个 JOSM 插件"。

### 3.5 10 条"必须为真"的完成判据

| # | 判据 |
|---|---|
| 1 | 往返无损：`.osm/.osc/.osm.pbf/.gpx/.jos/.joz` 打开→编辑→保存→上传→重下，语义逐字段等价（版本、负 id、`action`、visible/删除态） |
| 2 | 完整 编辑→上传→冲突 闭环（comment/source/hashtag、hook 顺序、`diffResult`、409→重下→四维度冲突→重传、关闭 changeset、上传选中） |
| 3 | Tools 几何工具达到 JOSM 语义（split/combine/reverse 含 tag 修正/simplify/align/distribute/orthogonalize/mirror/follow line/join/move onto/unglue/join areas/multipolygon） |
| 4 | 9 种工作模式及其修饰键与捕捉行为 |
| 5 | 未修改的 `defaultpresets.xml` + `elemstyles.mapcss` + 12 个 `.validator.mapcss` 可加载，产出**可识别为同一张地图、同一批警告**，修复可用 |
| 6 | 面板齐备且可停靠，快捷键同 JOSM |
| 7 | 键盘优先：默认快捷键集 + 双键序列 + 全部可重绑，无仅鼠标可达的动作 |
| 8 | 影像：TMS/WMS/WMTS + 5 内置、偏移调整、每影像持久化、偏移库取/存 |
| 9 | 会话与恢复：混合图层类型存/取、autosave 恢复提示 |
| 10 | 无头：`render`/`project`/`validate` 等价 CLI 可用于脚本 |

### 3.6 8 个最容易漏掉的行为

| # | 行为 | 漏掉的后果 |
|---|---|---|
| 1 | **反向 way 必须修正方向性 tag**（`oneway=yes→-1`、`:forward`/`:backward` 互换、`incline` 变号） | 静默制造错误数据，比崩溃更糟 |
| 2 | **relation 成员顺序是数据**，不是展示顺序 | 路线/多边形被重排，语义被改 |
| 3 | **area ≠ closed way**（`area=no`、闭合但线性的 way；MapCSS `area` 刻意匹配未闭合 way） | 误判面积对象，样式与规则双错 |
| 4 | **全局撤销**：跨图层单一栈（命令持有自己的 dataset），切图层不清栈，含可撤销的 SelectCommand；上传后要正确阻断/警告 | 撤销行为与预期不符 |
| 5 | **偏移按影像 id 持久化**（id 由 URL 派生），与偏移库无关也要存活重启 | 每次重新对齐影像 |
| 6 | **validator 抑制持久化**："暂时忽略"与"整类忽略"不同，跨会话有效 | 每次启动都吵，用户关掉验证器 |
| 7 | **Filter 语义**：每行 E/H/I/M（R 替换·A 增·D 删·F 找），自上而下求值，被过滤对象不可选中且**不参与搜索**，`Ctrl+Alt+E`+数字行开关 | 过滤变成"看不见但能选中"，出现幽灵操作 |
| 8 | **Expert mode 是功能门控**（Purge、复制坐标、Mirror、Follow Line、Add/Move Node、Changeset Manager、Jump To Position、Tiled Rendering…） | 隐藏错东西 → 高级用户以为功能消失 |

### 3.7 明确"不承诺"的范围

| 不承诺 | 理由 |
|---|---|
| Java 插件二进制兼容 | 依赖 JOSM Java 内部类型，Rust 宿主无法加载，只能能力重实现（§3.4） |
| JVM 脚本插件（`scripting`：JSR-223/GraalVM） | 无现实对应；替代是自选嵌入式脚本 + 文档化 API，且**不列入 parity** |
| OSM API 0.7 | 仅 wiki 构想页，无部署规范；JOSM 自身硬编码 `0.6` |
| "OSC 0.3" | 无权威规范；实际是 `osmChange version="0.6"` |
| 逐像素 UI 一致 | 用 egui 而非 Swing；对齐行为与快捷键，不对齐控件外观 |
| 与 JOSM 共享资源的 GPL 版本分发 | 走 wiki CC-BY-SA+LGPL 再获取路线（[17](./17-licensing-and-governance.md) §2.3） |

## 4 流程与时序

### 4.1 "一周工作流测试"脚本

```
Day 0  安装 NEO（不装 JOSM 快捷方式），导入既有 preferences（快捷键/影像偏移/抑制）
Day 1  下载 bbox → 描 20 栋楼 → 正交化 → 打地址 → 上传（comment+hashtag）
Day 2  追一条河（非分支选择）→ 分割/合并/反向（含 oneway 修正）→ 上传
Day 3  编辑公交 relation（成员顺序、角色）→ 校验 → 上传；遇 409 → 四维度冲突解决 → 重传
Day 4  导入本地 .osm + 参照 GPX/影像 → 偏移调整 → 存会话(.joz) → 重启 → 恢复
Day 5  制造错误（自交/重复节点/未闭合 area）→ 修 3 条 / 抑制 2 条 → 重启确认抑制仍在
Day 6  Filter 组合（R/A/D/F 各一次）+ 面板切换 + 全程键盘完成一次完整编辑；上传选中（局部）→ 关闭 changeset
Day 7  无头 validate/render 出报告与出图；用 `.joz` 会话在另一台机器复现今天的状态
```

每日记录三类事件：**习惯断裂**（"我下意识按了某键，没反应/做错事"）、**数据疑虑**（"不确定存盘是否保真"）、**回退冲动**（"这个得开 JOSM"）。任一 P0 级断裂即视为该里程碑未达成。

### 4.2 parity 推进

```
条目/行为变更 ─► 矩阵（P+状态=设计）─► 实现+测试 ─► 骨架 ─► 原型
                                              │
        nightly 与 JOSM CLI 双跑对拍 ─────────►┴─► golden/工作流通过 ─► 达成
```

状态升级的**唯一依据是测试通过**，不是代码合并；"达成"由 CI 工件（golden 哈希、工作流日志）签名。

## 5 权衡与替代方案

| 决策 | 选择 | 被否决项 | 理由 / 代价 |
|---|---|---|---|
| parity 定义 | 工作流契约（一周不开 JOSM） | 功能计数、逐控件对照 | 计数不可证伪；代价是需要真人 dogfood，成本高且不可完全自动化 |
| 反直觉行为 | **照抄**（下载在 File、几何在 Tools、Data 空菜单） | "顺便整理成更合理的菜单" | 习惯即契约；代价是自审时永远觉得菜单不合理 |
| 插件兼容 | 能力重实现 + WASM API | 尝试 Java 互操作/转译 | 编译期依赖 Java 内部类型，无解；代价是 233 个插件只覆盖 10 族 |
| 脚本 | 自选嵌入式脚本（不承诺 JSR-223 等价） | 内嵌 JVM | 内嵌 JVM 违背"无 JVM 启动"的核心卖点；API 只做 0.6（0.7 无规范可对齐，预留抽象层是纯负债） |

## 6 风险

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| J1 | 矩阵粒度过粗，P0 里混入"其实可以没有"的项 | 交付被稀释 | 每条 P0 必须在 §4.1 脚本中有用例，无用例则降级 |
| J2 | 双键序列被简化成单键映射 | 习惯断裂（`A A` 失效） | 快捷键引擎按"序列状态机"设计 |
| J3 | 反直觉行为被"好心"修正 | 用户误操作 | 矩阵显式列出反直觉项并写菜单位置断言 |
| J4 | 资源版本漂移（wiki 版 presets/styles 落后于当次 JOSM 发行）；一周测试依赖真人、CI 无法复现 | 差异被误判为 bug；回归发现晚 | 资产 manifest 记来源与 sha256 并对拍断言；把 §4.1 的 Day1/2/3/5/7 做成端到端测试 |
| J5 | 插件能力包许可未逐项确认 | 合规阻断 | 每族开工前完成许可确认（[17](./17-licensing-and-governance.md) §2.4） |

## 7 验收标准

1. §3.1 的 **35 条命令级 P0** 状态全为"达成"，每条至少一个自动化或脚本化用例通过；§3.2 / §3.3 / §3.4 的 **17 条 P0**（9+6+2）同样达成，全口径 **52 条 P0** 无"未开始"。
2. §4.1 脚本在**两位互不相关的制图者**身上各跑通一次，习惯断裂 ≤ 2 且均登记为已知偏差。
3. 含反直觉语义的每条 P0 都有一行覆盖检查，否则评审拒绝。
4. 与 r19678 双跑对拍：同一 PBF 输入的 `validate` issue 集合与 `render` 输出归一化后差异为 0（渲染用感知哈希阈值，见 [18](./18-testing-strategy.md) §2.6）。
5. 任一 P0 状态回退（达成 → 其他）即发布阻断。

## 8 参考

- JOSM 菜单索引：https://josm.openstreetmap.de/wiki/Help/Menu
- File（下载/上传位置）：https://josm.openstreetmap.de/wiki/Help/Menu/File
- Tools（几何核心）：https://josm.openstreetmap.de/wiki/Help/Menu/Tools
- Mode（9 模式与快捷键）：https://josm.openstreetmap.de/wiki/Help/Menu/Mode
- Data（零核心项、插件宿主）：https://josm.openstreetmap.de/wiki/Help/Menu/Data
- Windows（面板与键位冲突）：https://josm.openstreetmap.de/wiki/Help/Menu/Windows
- 冲突对话框（四维度/三列/Freeze）：https://josm.openstreetmap.de/wiki/Help/Dialog/Conflict
- Filter（E/H/I/M 与 R/A/D/F）：https://josm.openstreetmap.de/wiki/Help/Dialog/Filter
- 命令行选项（`render`/`project`/`validate`）：https://josm.openstreetmap.de/wiki/Help/CommandLineOptions
  > 待验证：`validate` 子命令选项表（一手文档只给用法块）。
- 插件清单（233 个，含 `Plugin-Mainversion`）：https://josm.openstreetmap.de/plugin
- OSM API v0.6（changeset/409/差异上传）：https://wiki.openstreetmap.org/wiki/API_v0.6
- OsmChange 格式：https://wiki.openstreetmap.org/wiki/OsmChange
- API 0.7 现状（构想页）：https://wiki.openstreetmap.org/wiki/API_v0.7
- 一手事实汇总：[`.research/josm-inventory.md`](../.research/josm-inventory.md)
