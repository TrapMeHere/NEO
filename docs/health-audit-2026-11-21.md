# NEO 全身体检、修复与功能补完报告（2026-11-21）

> 范围：`D:\neo` 全 workspace（17 个库 crate + 3 个 app）。方法：逐文件通读 + 分层自动扫描
> （fmt / clippy / test / 依赖脚本 / PBF·XML 往返 / 无头渲染 / 桌面启动 / 1M 合成基准），
> 再按 P0→P3 制定方案并落地。本文同时是**修复台账**：每条标注 `已修复`、`部分修复` 或
> `未修复（含后续方案）`。功能完成度另见 **[feature-parity.md](feature-parity.md)**。

## 1. 结论摘要

修复前：

| 项 | 实测 |
|---|---|
| `cargo check --workspace` | 通过，35 条 warning |
| `cargo fmt --all --check` | **失败**（5 个文件） |
| `cargo clippy` | 通过但 ~60 条 warning（含 48 条 desktop） |
| `cargo test --workspace` | **master 上 `#[test]` = 0**，仅 1 个 doctest |
| 测试目录 | `tests/` 是 3 个空目录 |
| CI | 不跑 `cargo test`；release 也不跑测试；`npm ci` 在 `contents: write` 下执行脚本 |
| 依赖元数据 | 20 个包全部可发布（无 `publish = false`），repository 指向错误组织 |

修复后：

| 项 | 实测 |
|---|---|
| `cargo check --workspace` | 通过，**0 warning** |
| `cargo check --workspace --all-features` | 通过（含 wasmtime 50） |
| `cargo fmt --all --check` | 通过 |
| `cargo clippy --workspace --all-targets -- -D warnings` | **通过（0 warning）** |
| `cargo test --workspace` | **64 个测试通过** |
| `python scripts/check-{deps,i18n,doc-links,write-api}.py` | 全部通过 |
| 桌面启动冒烟 | 8s 存活，场景 ~2.0ms/39k 顶点，GPU 上传 0.23ms |
| 无头渲染 | 800×600 PNG，22 708 三角形，GPU 帧 3.83ms |
| 1M 合成基准 | import 168ms，z16 场景 0.819ms，峰值 557 MiB |

最大风险从"**会静默写坏数据/丢失恢复**"收敛为"**规模化性能与部分产品闭环未完成**"。
没有发现必须整体回滚的修改；唯一一次性能相关的高风险改动（WorldCache 全量重建）被
改写为增量更新后才保留。

## 2. 已修复的 P0（数据损坏 / 安全 / 崩溃）

### 2.1 ID 与索引

- **重复 `ObjectId` 覆盖索引**：`push_node/way/relation` 原来直接 `HashMap::insert`，
  旧对象变成"存活但无 ID"的孤儿，导出重复 ID。→ 三个 `push_*` 返回 `Result`，重复 ID
  报 `Consistency`；所有导入路径传播错误。
- **本地负 ID 撞号**：`next_local_id` 固定递减，导入含负 ID 的文件后新对象复用 ID。
  → 跳过任何已占用的 node/way/relation ID。
- **`way_node_ids` 静默丢点**：OSC/PBF 写路径 `filter_map` 掉失效引用。→ 新增
  `try_way_node_ids()`，导出遇失效句柄直接报错；`set_way_nodes` 校验句柄并返回 `Result`。
- **`take_dirty_*` 为死槽位铸造未来句柄**：→ 用 `contains()` 过滤。

### 2.2 Journal / 崩溃恢复（原实现实际不可用）

- **`JournalFrame::Cmd(JournalCommand)` 无法序列化**：serde 的 internally-tagged enum
  不支持 newtype variant，journal **一条命令都写不进去**，崩溃恢复从未真正工作。
  → 改为 `Cmd { #[serde(flatten)] command }` 并补 JSON 往返测试。
- **恢复基线错误**：原 `init_editor` 把命令重放到空 DataSet，文件型会话的编辑全部失败。
  → `neo-state.json` 记录源文件，恢复先加载；成功立即写 `neo-recovered.osm` 作为新基线。
- **undo/redo 不写 journal**：撤销的删除会在重启后复活。→ 新增 `Rebase` 帧，undo/redo
  后用当前 undo 栈重建 journal。
- **半事务/坏行**：未闭合事务会被后续提交连带重放；坏行后继续读。→ `replay_plan` 校验
  `TxBegin(id)→Cmd*→TxCommit(id)` 配对；首条坏行即停止；容忍 UTF-8 BOM。
- **append 不截断 / 跨文档串档**：→ `FileJournal` 补换行、支持 `reset()`；打开/下载/
  保存/上传后清空 journal；切换文档前写 state。
- **journal 写失败被吞**：→ 记录 failure 并停止写入；UI 显示错误；每行 `sync_data()`。
- **`record()` 把自动 ID 写成 0**：→ 写实际 `applied` ID。

### 2.3 事务 / 历史 / 变更状态

- **批事务回滚不完整**：失败命令自身半截写入、回滚错误被吞、`ChangeTrack` 不恢复。
  → 批内每条命令前刷新 referrers + precheck；失败时 undo 当前命令并逆序回滚；恢复
  ChangeTrack；错误不再静默。
- **undo/redo 先弹栈后执行**：失败即丢历史。→ 成功后再移动栈；批失败回滚已撤销/重做部分。
- **undo 不恢复 `ChangeState`**：撤销后仍显示脏、上传 diff 含空修改。→ 全命令保存/恢复。
- **`MoveNode` undo 落点错误**：原来用 apply 时当前位置。→ 用构造时的 `from`。
- **`SplitWay` redo 换 ID**：→ 首次分配后写回 `new_way`。
- **`MergeWay` 自合并/丢 tag/忽略 relation 引用**：→ 拒绝 `a==b`、合并 b 独有 tag、
  b 被 relation 引用时拒绝。
- **删除 relation 不检查父 relation**：→ referrers 新增 relation→relation CSR。

### 2.4 不可信文件输入

- **`.joz` Zip Slip（可任意写文件）**：→ 只取 `file_name`，拒绝 `/ \ : ..`；限制条目数
  4096、单层 64 MiB、总量 256 MiB、session.jos 4 MiB；缺失图层不回退本机路径。
- **`.jos` 双重转义/路径**：→ 属性 `normalized_value` 解码；原子写；viewport 校验。
- **XML 时间戳**：真实 `.osm` 的 ISO-8601 被当整数解析（全部变 0）。→ 新增 RFC3339
  双向转换；同时补 `action` 读写、坐标校验、根元素/截断检查、实体解码、原子写。
- **缺失/非法 lat/lon 变 (0,0)**：→ 可见 node 缺坐标报错；`LatLon::try_from_degrees`
  拒绝 NaN/超范围；OSC/PBF/GPX 同步采用。
- **PBF**：坐标用 nanodegrees 并校验；`MissingPolicy::Error` 真正返回；写可见性时声明
  `HistoricalInformation`；blob/header 上限；delta `checked_sub`；原子写；真实 bytes；
  悬空引用报错；重复 ID 拒绝。
- **GPX**：lat/lon 有限性与范围校验；push 错误传播。
- **导出原子性**：XML/PBF/`.jos`/`.joz` 均为同目录临时文件 + rename。
- **非法 XML 码位**：`escape_xml` 丢弃 XML 1.0 禁止字符。

### 2.5 安全边界

- **OAuth state 可预测**：→ 32 字节 CSPRNG；回调校验方法/Path/Host；state 不匹配
  按 `Ignore` 继续等待（防远程打断登录）；删除 `out/oauth-debug.txt`。
- **token 泄漏面**：`HttpRequest`/`TokenSet`/`Pkce`/`Account` 的 `Debug` 全部脱敏；
  token 文件原子写、损坏时备份 `.corrupt-*`；配置目录改每用户数据目录。
- **token 过期**：`active_token()` 临期刷新并原子回写；上传/下载共用。
- **`NEO_API_URL` 可指向 http**：→ 只接受 https 或本机回环。
- **HTTP 重定向**：关闭自动重定向，避免 POST 被 301/302 改 GET、307/308 重放。
- **WASM 插件**：关闭 multi-memory；memory/table/instance 限额；网络白名单拒绝
  userinfo；http_log 上限；`read_file` 8 MiB；`.neopkg` 包/条目/解压上限；重复条目拒绝。
- **GPU**：注册 `on_uncaptured_error`，validation/device error 记录日志。
- **路径**：journal、autosave、state、恢复、同步下载、启动错误全部移到每用户数据目录。

## 3. 已修复的 P1（功能 / 性能）

### 3.1 渲染与场景

- **`max_vertices` 安全阀恒失效**：`stats.vertices` 只在循环后赋值。→ 每轮用三个批次
  实时顶点和判断，超限置 `clamped`。
- **每次编辑全量重建 WorldCache（10M 节点=千万次投影）**：→ WorldCache 移入 `Editor`，
  由 `EditContext` 在 add/move/remove/restore 时 O(1) 更新；SceneBuilder 只读引用。
- **resize 不重建 / 平移阈值 25% / 缩放阈值 0.5**：→ viewport 变化重建、平移 4%、
  缩放 0.25 且跨 17/19 LOD 档必重建；拾取用场景构建时 zoom。
- **闭合面填充后 return，永远没有描边**：→ fill 后继续 stroke；样式只算一次。
- **重复点产生 3 倍宽尖刺**：→ 零方向法线回退单侧、不做斜接放大。
- **RDP 递归可爆栈 / O(n²)**：→ 显式栈迭代；保留 256 点预抽稀。
- **日期变更线**：渲染顶点按相机原点 ±C 环绕（空间索引 bbox/拾取仍是后续项）。
- **相机 NaN/越界**：zoom 非有限值兜底；center clamp；退化 bbox 不再跳 z22。
- **极区数据被静默改写**：|lat|>85.05° 禁止拖动并提示。

### 3.2 编辑热路径

- **AddNode 全量重建 Referrers**：→ 新节点没有引用，不置 dirty；AddWay/删除仍重建。
- **`live_len()` O(slots)**：→ `SlotAllocator` 维护 O(1) `live` 计数。
- **版本号不随 `push_*`/`set_*` 递增**：→ 统一 `bump_version()`。
- **`Referrers` 把逻辑删除对象算作引用**：→ 跳过 `visible=false`、校验句柄、闭合 way
  去重、新增 relation→relation。
- **`TagStore::get` 越界 panic**：→ 返回空切片。
- **`is_valid_lat/lon` 对 `i32::MIN` 失效**：→ `unsigned_abs`；`approx_meters` 全程 f64。

### 3.3 同步闭环

- **上传成功不应用 diffResult**：负 ID/version 永不回写。→ `apply_diff_result()` 重映射
  ID、写回 version、清空 ChangeTrack/历史/journal，并落盘 `neo-uploaded.osm`。
- **changeset 生命周期**：失败也会尝试关闭；关闭失败不覆盖真实上传结果。
- **409 解析为空**：→ `parse_conflict_text` 提取对象与 server version。
- **delete 顺序错误**：→ relation→way→node；`if-unused="true"`；本地新建后删除不上传。
- **OSC 实体不解码/丢 user/静默丢引用**：→ 统一解码、保留 user、按 policy 报错。

### 3.4 桌面端

- 打开/下载/恢复前统一 `confirm_discard()`。
- 恢复失败保留坏 journal 为 `.unrecovered-*`；成功写基线。
- 标签编辑器在 undo/redo/删除/失败后重同步；`apply_tags` 校验当前选择。
- 同步 UI 入口、状态占位符、每用户数据目录、会话保存嵌入当前编辑。
- PBF/XML 导入移到后台线程；autosave 的 copy/read + SQLite 移到后台。
- 非活动/隐藏图层统一拦截画布、菜单、快捷键、标签、关系、剪贴板、历史回退。

## 4. 已修复的 P2 / 工程化

- 移除未使用 workspace 依赖；20 个包 `publish = false`；元数据继承；repository 改正；
  MSRV 1.97、工具链固定 1.100.0。
- `rustfmt.toml` 删除 stable 不支持的选项。
- `neo-style/build.rs` 不再每次构建都跑 node；输出损坏/缺失明确失败；支持 `NODE`。
- `neo-desktop/build.rs` PE 版本号从 `CARGO_PKG_VERSION` 生成。
- CI：fmt / clippy `-D warnings` / check / **test** / all-features；release 增加 test 门禁、
  `npm ci --ignore-scripts`、`neo-cli`、许可文本、`-ErrorAction Stop`、concurrency/timeout。
- NOTICE 修正字体外置、补 NSI BSD-3-Clause、LICENSE-ISC/BSD 文本。
- 安装包：Windows/Linux/macOS 都补 `assets/` 与许可证；资源搜索新增
  `/usr/share/neo` 与 `Contents/Resources`。
- 新增 `.gitattributes`，避免跨平台把字体/PNG 当文本。
- README/TODO/docs 与代码一致；`check-deps.py` 新增环检测与 publish 检查。

## 5. 可行性分析：哪些改动被验证、哪些被回退/改写

| 改动 | 风险 | 处理 |
|---|---|---|
| WorldCache 全量重建 → Editor 持有并增量更新 | 跨 crate API 改动大，易漏更新导致错位 | 编译期强制所有调用点；add/move/remove/restore/undo/redo/apply_diff 全更新；1M 基准与桌面渲染验证。未保留全量重建版本。 |
| Referrers 全量重建 → 只去掉 AddNode 脏标记 | 增量 CSR 改造面过大 | 未做增量 CSR；只消除"画点每点重建"的 O(k·N)。AddWay/删除仍全量重建，登记为剩余项。 |
| journal append-only → Rebase | 旧日志兼容性 | 旧帧仍可读；`MoveNode` 新字段 `#[serde(default)]`；无 Rebase 的旧日志按原语义重放。 |
| XML 严格根元素/截断校验 | 可能拒绝非标准 `.osm` 片段 | 只接受 `<osm>`，对 OSM 0.6 是正确约束；单测覆盖。 |
| 删除本地新建后对象不上传 | 可能漏删服务端对象 | 只对 `id.is_local()` 生效；服务端对象正常 `<delete>`。 |
| GPU 错误从 panic 改为日志 | 可能掩盖渲染错误 | 仍 `tracing::error!`，避免无控制台 release 直接消失。 |
| HTTP 关闭重定向 | 某些镜像/代理会 302 | OSM 官方 API 不重定向；非法跳转本就不应带 Bearer 重放。 |

## 6. 未修复 / 部分修复（含后续方案）

1. **TagStore / WayStore 只追加、无 compact**（P1 内存单调增长）。方案：事务边界统计
   hole ratio，超阈值重写 arena 并 remap `TagSetId`/way offsets；需要 `DataSet::compact()`
   与长会话测试。
2. **Referrers 结构变更仍 O(N+E)**（AddWay/删除）。方案：per-slot 可增长 `SmallVec` 或
   overlay（新增边表 + 墓碑），先 1M 测 p95 再替换。
3. **UI 线程阻塞 IO**：校验、导出、rfd 对话框仍在 `ui()` 内（导入与 autosave 已移后台）；
   查找也是全表扫描（10M 会停顿）。方案：导出/校验/查找复用 worker；rfd 改
   `AsyncFileDialog`；查找建标签倒排索引。
4. **10M 内存**：WorldCache 24 B/节点、SpatialIndex 双份 bbox、TagStore 无去重。
   方案：`Vec<WorldPoint> + BitSet`；索引去 entries；tag 内容哈希去重。
5. **日期变更线空间索引/拾取**：渲染已环绕，bbox/拾取仍近似。方案：bbox 解缠 + 双副本
   条目，或世界 x 以 0 经度为中心。
6. **极区投影**：WebMercator 不支持 |lat|>85.05；已禁止拖动避免写坏，但不可编辑。
7. **PBF**：写侧 chunk 不按字节自适应；无 DenseNodes；截断 PBF 可能"成功"；`osmpbf`
   对未知 relation member type 会 panic。方案：按 block 预算 flush、DenseNodes、
   `catch_unwind` 包单 blob 解码、上游修复/替换解码器。
8. **GPX**：`ele/time/rte` 仍丢失。方案：映射为 tag、流式读写。
9. **同步**：无分块上传、无 Ambiguous 超时语义、无手合并 UI；token 明文落盘（建议
   DPAPI/凭据管理器）；插件未链接进桌面。
10. **插件**：签名仍是清单内自哈希（无信任根）、只支持 WAT、host 调用返回数据被丢弃。
    方案：Ed25519 + 公钥内置、`load_binary`、ABI 返回 buffer。
11. **渲染**：间接绘制/cull 未接线、无 buffer 池、无 sRGB、readback 全同步。
12. **MapCSS**：`@media/@import` 未跳过、选择器逗号不识别引号、`area/closedway` 语义
    坍缩、dashes 未绘制。
13. **CI/发布**：Action 未 pin SHA、未签名/无 SBOM/provenance、无 cargo-deny、无三平台
    matrix。
14. **测试覆盖**：64 个测试集中在核心回归；UI/网络/GPU/10M 仍无自动化。

## 7. 验证记录

### 7.1 编译 / 静态检查

```
cargo check --workspace                          # 0 warning
cargo check --workspace --all-features           # 通过（wasmtime 50）
cargo fmt --all --check                          # 通过
cargo clippy --workspace --all-targets -- -D warnings  # 通过
cargo test --workspace                           # 64 passed
python scripts/check-deps.py                     # OK: 20 packages, DAG + publish
python scripts/check-i18n.py                     # 339 keys, zh/en 502
python scripts/check-doc-links.py                # 393 links
python scripts/check-write-api.py                # 60 files
```

### 7.2 运行冒烟

- 桌面：8 秒存活；`scene build ~2.0ms / 39k verts`；`gpu upload 0.23ms`；
  journal 正确创建在用户数据目录。
- journal 恢复：手工写入 `TxBegin/AddNode/TxCommit` → 启动后 journal 轮转为
  `*.recovered-*`、生成 `neo-recovered.osm` 与 `neo-state.json`；二次启动以该文件为基线。
- CLI：`scene --nodes 5000` → `vertices=2061 build=143µs`；presets 覆盖率正常。
- 无头渲染：`render --synth 20000` → 800×600 PNG，22 708 triangles，3.83ms。
- 200k 合成基准：import 25.0ms，index 120.9ms，z16 0.82ms，fit-all 12.3ms，
  move 9.37µs，pick 2.05µs，peak 246.3 MiB。
- 1M 合成基准：dataset 126.6 MiB，import 168.3ms，index 584.8ms，referrers 13.2ms，
  z16 0.819ms，fit-all 21.6ms，move 16.8µs，pick 8.9µs，peak 557.4 MiB。

### 7.3 性能结论

- 场景构建没有退化：z16 视野 0.8ms；编辑节点不再触发 10M 次投影，WorldCache 增量是净收益。
- 全量重建仍发生在 AddWay/删除（Referrers）与每次编辑后的视口场景重建；10M 下仍是瓶颈。
- 没有发现因本轮修改导致的性能回退；唯一有风险的全量缓存改动已被增量方案替代。

## 8. 关键文件索引

| 主题 | 文件 |
|---|---|
| ID/坐标/句柄 | `crates/neo-core/src/{ids,units,handle,sym}.rs` |
| 数据集/索引/引用 | `crates/neo-store/src/{dataset,indexes,tag_store,node_store,way_store,relation_store}.rs` |
| 命令/事务/历史/journal | `crates/neo-editor/src/{editor,commands,commands_geom,history,journal}.rs` |
| XML/OSC/PBF/时间戳 | `crates/neo-io/src/{xml,osc,osc_read,pbf,pbf_parallel,pbf_write,time}.rs` |
| 相机/几何/缓存 | `crates/neo-geo/src/{camera,geometry,world_cache,world}.rs` |
| 场景构建 | `crates/neo-scene/src/builder.rs` |
| MapCSS | `crates/neo-style/src/{mapcss,lib,schema,assets}.rs` |
| 会话/快照 | `crates/neo-persist/src/{session,snapshot}.rs` |
| API/OAuth | `crates/neo-api/src/{lib,oauth}.rs` |
| 插件沙箱 | `crates/neo-plugin/src/{wasm_host,gate,package,capability,apply}.rs` |
| 桌面 | `apps/neo-desktop/src/{app,accounts,main,build}.rs` |
| CI/发布 | `.github/workflows/{ci,release}.yml` |
| 回归测试 | `crates/neo-editor/tests/regressions.rs`、`crates/neo-io/tests/regressions.rs`、`crates/neo-persist/tests/zip_safety.rs`、各 crate `#[cfg(test)]` |

## 9. 下一步（按 ROI 排序）

1. 用 Bremen/Monaco 真机数据重跑 XML/OSC/PBF 往返与桌面打开。
2. 10M PBF 导入 + 编辑 + 1 小时会话，观测 arena 增长与 referrers 重建耗时。
3. 把校验/导出移出 UI 线程。
4. 实现 arena compact 与 Referrers 增量，再重测 10M 编辑。
5. 同步补分块/Ambiguous/手合并；token 交给 OS 凭据库。
6. CI 补 Action pin、cargo-deny、三平台 check、签名/SBOM。

## 10. 功能补完（同日第二轮）

对照 iD/JOSM 补了专业用户最常撞到的交互缺口，详见 **[feature-parity.md](feature-parity.md)**：

- **关系编辑**：成员增删/上下移/角色编辑/加入选中；新增 `SetRelationMembers` /
  `AddRelation`（含 undo 与 journal）；关系列表可点击选中。
- **框选**：空白处拖矩形，Shift 累加；空间索引 + 任意节点在框内选 way。
- **复制/粘贴/副本**：Ctrl+C/V/D，跨文档字符串剪贴板；复制 way 自动带上节点。
- **查找**：Ctrl+F，`id:123` / `key=value` / 文本；结果点击选中并定位。
- **命令历史**：右侧显示可撤销命令，点击撤销到该步之前。
- **GPX**：文件菜单导入/导出；导入走命令批，保持单写者。
- **几何**：简化、粘合、对齐、等距分布、水平/垂直镜像、解粘、平行复制、正交化、
  合并闭合区域（multipolygon）、移动节点到 way 上、挤出建筑轮廓。
- **过滤器**：按类型/`id:`/`key=value`/文本隐藏未匹配对象，或隐藏选中；场景与拾取同步。
- **上传门禁/选中上传**：错误阻止上传；只上传选中对象，自动带上新建依赖。
- **多图层**：活动图层可编辑，停靠图层只读渲染；新建空图层、打开为图层、切换、显隐、
  删除；停靠层保存到 `layers/*.osm` 并在启动时恢复；整层复制到活动图层（ID 重映射）。
- **Changeset/偏移库**：View → Changeset 查看器加载服务端 changeset；影像偏移库 JSON
  导入/导出、按当前中心应用最近偏移。
- **快捷键**：设置 → 快捷键可逐项重绑（按下新组合键，Esc 取消），立即生效并写入
  `neo-state.json`；支持 Ctrl/Shift/Alt + 键，恢复默认。
- **冲突**：409 后保留本地/重下载；逐对象采用远端/合并标签/合并几何。
- **multipolygon**：`type=multipolygon/boundary` 的 outer/inner 成员组装，earcut 真正挖洞；
  未闭合成员不自动拼接。
- **Notes**：当前区域 notes 列表、新建、评论、关闭、点击定位；读公开，写需登录。
- **插件管理**：安装 `.neopkg`、列表、启用/禁用、删除；启用时 `PluginHost` 加载
  WAT/WASM、按清单能力授权、执行 on_load 命令并走 Editor 事务。签名仍是清单内自哈希。
- **影像**：TMS 瓦片 + WMS 单图（异步、PNG、透明度、X/Y 偏移、持久化）；只接受
  https/本机回环。偏移数据库共享未做。
- **校验规则**：由 17 条扩到 25 条，新增 area 未闭合、非相邻重复节点、oneway 非法、
  空 name、layer 越界、multipolygon 无 outer/outer 未闭合；UI 规则名全部走 i18n。
- **layer 排序**：场景按 OSM `layer`（-5..5，非法值按 0）分桶，逐层升序提交
  fill→stroke→point；隧道/桥/高架不再依赖遍历顺序。
- **MapCSS/标签**：`dashes`、`text`（标签键）、`text-color`、`font-size`、`icon-image`
  （assets 下 PNG，16px 默认/icon-width）真正绘制；SVG/远程图标未支持。
- **设置**：常规（语言/自动保存）、编辑（简化容差）、校验（门禁）、网络（API 地址）、
  存储（数据目录/清理自动保存）、账户、快捷键；设置持久化到 `neo-state.json`。
- **关于窗口**：移除字体示例，只保留版本与许可信息。
- **校验**：+5 条规则，共 17 条。
- **只读保护**：非活动/隐藏图层同时拦截菜单、快捷键、标签、关系、剪贴板、历史回退。

验证：`cargo test --workspace` **64 passed**；clippy `-D warnings`、fmt、all-features 全通过；
桌面冒烟 8s 存活（场景 2.0ms/39k 顶点，GPU 上传 0.23ms）。
