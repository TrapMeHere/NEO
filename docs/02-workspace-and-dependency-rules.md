# 02 · Workspace 与依赖规则（Workspace & Dependency Rules）

NEO 是 17 个 crate + 3 个 app 的单 workspace 模块化单体：分层不靠约定，靠 crate 边界、sealed trait、脚本断言三道闸强制。本文是依赖方向、feature 组合与版本一致性的**唯一事实来源**，也是 `scripts/check-deps.py` 白名单的出处。八层职责与线程模型见 [系统架构](./01-system-architecture.md)，版本冲突的证据链见 `.research/crate-api-cheatsheet.md`。

## 摘要

单 workspace 多 crate 的价值不在"整齐"，而在于**分层可以被机器拒绝**：任何"UI 直接改数据"或"渲染层依赖 store"的越界都在编译期或 CI 阶段失败，而不是三个月后以死锁的形式出现。因此本文把依赖白名单、深度表与版本钉死策略写成可执行规则，而不是风格建议。

## 1 目标与范围

**在范围内**：目录布局；每个 crate 允许 / 禁止的内部依赖；DAG 深度表；强制手段（编译器 + 脚本 + CI）；check-deps 脚本设计与失败样例；GUI 栈锁 0.36.2 的原因与升级演练；feature 组合与默认最小集；公共 API 稳定性；semver 与 MSRV。

**不在范围内**：各 crate 的内部设计与算法（见 04–12）；外部依赖的功能选型理由（见专题文档与 ADR）；发布流程与签名（见 17）。

## 2 设计

### 2.1 目录树

```
neo/
├─ Cargo.toml                 # [workspace] members = ["crates/*","apps/*"]；[workspace.dependencies] 版本唯一来源
├─ Cargo.lock                 # 入库；CI 用 --locked
├─ rust-toolchain.toml        # channel = "stable"（本机 1.100.0）；MSRV = rust-version 1.95
├─ crates/
│  ├─ neo-core/               # L1 叶子：ObjectId / Handle+generation / Sym 驻留 / 定点 Coord / 错误
│  ├─ neo-osm/                # L1 DTO：Tags / Node / Way / Relation / ChangeState（不碰存储）
│  ├─ neo-store/              # L2 SoA Node|Way|RelationStore、TagStore arena、IdIndex、
│  │                          #    CSR Referrers、ChangeTrack、BitSet 脏位图、DataSet
│  ├─ neo-query/              # L4 只读出口：MapQuery + MapSnapshot + SnapshotCell（arc-swap）
│  ├─ neo-geo/                # L3 WebMercator f64 米 / Camera / rstar 0.14 / 几何 / 拾取（只读 store）
│  ├─ neo-style/              # L4 MapCSS / Presets
│  ├─ neo-io/                 # L2 PBF / XML / osmChange 0.6（osmpbf 只读；写侧自研 encoder）
│  ├─ neo-persist/            # L2 SQLite WAL / command journal / autosave / 崩溃恢复
│  ├─ neo-scene/              # L5 可见集 / LOD / SceneFrame / MapVertex（无 GPU 句柄）
│  ├─ neo-render/             # L5 wgpu 30：设备装配 / 管线 / BufferPool / 离屏 / GPU 拾取
│  ├─ neo-editor/             # L6 Command / Transaction / Undo / Journal / Selection / 冲突
│  ├─ neo-tools/              # L6 选择 / 绘制 / 移动 / 分割 / 合并 / 简化（只产出 EditCommand）
│  ├─ neo-api/                # L1 API 0.6：changeset PUT、单请求 diff 上传、409、下载、notes
│  ├─ neo-validate/           # L4 拓扑 / tag / 几何规则（rayon 并行增量）
│  ├─ neo-plugin/             # L1 WASM 宿主（wasmtime 50）：只读 query、只返回 Command
│  └─ neo-ui/                 # L7 egui 面板 / 对话框 / 菜单 / 快捷键（不依赖 io/api/persist）
├─ apps/neo-desktop/ · neo-prototype/   # L8 eframe 入口+装配+CLI；性能原型（不含 ui/editor）
├─ benches/  tests/  docs/    # criterion 0.8 基准；跨 crate 集成测试；本文件集
├─ scripts/  assets/          # check-deps.py / check-write-api.py / i18n / UI 冒烟；图标·预设·样式
```

### 2.2 依赖白名单（唯一事实来源）

表中每一行就是 `scripts/check-deps.py` 的 `ALLOW`。**新增任何内部边 = 同时改本文表格与脚本白名单 + 一个 ADR 或双人评审**，否则 CI 拒绝合并。

| crate | 允许的内部依赖 | 硬性禁止 | feature 门控 |
|---|---|---|---|
| `neo-core` | —（叶子） | 任何内部 crate；GUI/GPU/IO 类型 | `serde`（可选） |
| `neo-osm` | core | store / geo / render / ui | `serde` |
| `neo-store` | core, osm | 任何上层；GUI/GPU | — |
| `neo-geo` | core, osm, store | 可变 DataSet；GPU/UI | — |
| `neo-style` | core | store / osm / GPU | — |
| `neo-io` | core, osm, store | editor / ui / render | — |
| `neo-persist` | —（当前无内部 crate） | editor / ui / render | — |
| `neo-api` | core | store / osm / editor / ui | `http`（默认关） |
| `neo-query` | core, osm, store, geo | editor（避免读写循环）；render/ui | — |
| `neo-validate` | core, osm, store | editor / geo / ui / render | — |
| `neo-scene` | core, osm, store, geo, style | render / editor / ui / query | — |
| `neo-render` | core, geo, scene | **store / editor / ui** | `pick-readback` |
| `neo-editor` | core, osm, store, geo | **render / ui / io** | — |
| `neo-tools` | core, osm, store, geo, editor | store 写 API（只经 Command） | — |
| `neo-plugin` | core, query, editor | store（间接也不允许） | `wasm-plugins` |
| `neo-i18n` | （无内部依赖） | store / editor / render | — |
| `neo-ui` | core, osm, store, style, i18n | **io / api / persist / render / editor** | — |
| `neo-desktop` | core, osm, store, io, geo, scene, render, editor, persist, tools, validate, api, plugin, ui, style, i18n | **query**（快照还没接到界面） | `api`/`persist`/`wasm-plugins`/`profile` |
| `neo-prototype` | core, osm, store, io, geo, scene, render | ui / editor / api / persist | `profile` |
| `neo-cli` | core, store, io, geo, scene, style, plugin | editor / ui / api / persist | `plugins` |

两处**已记录并只允许如此**的跨层边：① `neo-validate → store`（只读规则需要引用集视图）；② `neo-plugin → query + editor`（插件只读快照、只返回命令）。`neo-query` 目前只被插件使用，界面和桌面都不依赖它。白名单等于当前 `Cargo.toml` 的直接依赖，不是预留配额。

### 2.3 DAG 与深度

`depth(c) = 0`（没有内部依赖）否则 `1 + max(depth(dep))`，按实际声明的边计算；概念层（01 §2.1）与编译深度不同序，评审以本表为准。

| depth | crates | 关键含义 |
|---|---|---|
| 0 | core, i18n, persist | 无内部依赖。persist 目前只做会话文件，不链接 store |
| 1 | osm, api, style | osm 是 DTO；api 只依赖 core；style 只解析样式 |
| 2 | store | 数据真值层 |
| 3 | geo, io, validate, ui | geo/io 读 store；validate 不读 geo；ui 只到 store 和 style |
| 4 | scene, editor, query | scene 依赖 geo+style；editor 依赖 geo；query 是插件的只读出口 |
| 5 | render, tools, plugin | render 只依赖 core+geo+scene；tools 经 editor 产出命令；plugin 读 query |
| 6 | prototype, cli, desktop | prototype 不含 ui/editor；desktop 不依赖 query |

深度按**实际声明的边**计算，必须等于本表。给 `neo-scene` 加一条 `neo-editor` 依赖不会形成环，但深度会从 4 变成至少 5，R3 拒绝，直到本表和 §2.2 一起改。

### 2.4 强制实现手段

| 闸门 | 手段 | 能抓出 | 抓不出 |
|---|---|---|---|
| G1 编译期 | crate 边界（未声明依赖不可用） | 直接越界引用 | 通过 public 重导出的间接使用 |
| G2 类型封印 | sealed trait、写 API `pub(crate)`、写权 `!Send` | "UI 拿到写权限"这类语义越界 | 全部绕过路径（需 G3 兜底） |
| G3 脚本 | `check-deps.py`：R1 白名单、R2 环、R3 深度、R6 `publish=false` | 依赖方向与深度漂移 | R4/R5 与运行时行为 |
| G4 CI 断言 | `cargo tree -p X` 不含禁用包、`Instance::new` 计数为 1、写入口调用点 grep、`--locked` | 依赖树与源码级违规 | 逻辑错误（由测试承担） |

G2 的最小实现（签名公开，但外部 crate 无法构造写许可，因此只有 editor 能发起写）：

```rust
mod private { pub struct Seal; }
pub struct WriteSeal(private::Seal);                              // 外部无法构造
pub trait WriteSealSource { fn seal(&self) -> WriteSeal; }        // impl 只在 editor 内部
```

### 2.5 `scripts/check-deps.py` 设计

**输入**：`cargo metadata --format-version 1 --no-deps`（不拉远端索引，可离线）。**输出**：违规行带 `R1`/`R2`/`R3`/`R6`；通过时 `OK: N packages, DAG + depth + publish rules hold`。**退出码**：`0` 通过 / `2` 违规。脚本当前执行 R1、R2、R3、R6；下表里的 R4、R5 仍是设计，还没有进脚本。

| 规则 | 内容 | 失败示例 |
|---|---|---|
| R1 | 每条内部边必须出现在 §2.2 白名单 | `EDGE neo-ui -> neo-persist (R1: not in allowlist)` |
| R2 | 白名单图必须无环（三色 DFS） | `CYCLE neo-store -> neo-geo -> neo-store (R2)` |
| R3 | 深度必须等于 §2.3 表；漂移需同 PR 改表 | `DEPTH neo-scene expected 4 got 6 (R3)` |
| R4 | `dev-dependencies` 只允许同深度或更浅 | `EDGE neo-geo -> neo-editor (dev, R4: points upward)` |
| R5 | `[features]` 名必须与 §2.7 表一致 | `FEATURE neo-store/api-cache not declared (R5)` |
| R6 | 每个内部 crate 必须 `publish = false` | `PUBLISH neo-query is publishable (R6)` |

```python
#!/usr/bin/env python3
"""校验 workspace 内部依赖边。白名单 = docs/02 §2.2 表格，逐行对应（唯一事实来源）。"""
import json, subprocess, sys
from collections import defaultdict

ALLOW = {   # 18 个 crate 的允许边：逐行照抄 §2.2 表格（新增边必须同 PR 改两处）
    ...     # 内容从略；实现时按表填入，含 ui->query、plugin->query 两条已记录边
}
DEPTH = {...}   # 见 §2.3 表：18 个 crate 的期望深度，逐行照抄

def main() -> int:
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"], text=True))
    pkg = {p["name"]: p for p in meta["packages"]}
    errs, edges, devs = [], defaultdict(list), defaultdict(list)
    for name, allowed in ALLOW.items():
        if name not in pkg:
            errs.append(f"MISSING {name}"); continue
        if pkg[name].get("publish") != []:
            errs.append(f"PUBLISH {name} is publishable (R6)")
        for dep in pkg[name]["dependencies"]:
            d = dep["name"]
            if d not in DEPTH:                 # 外部依赖不在本规则范围
                continue
            (devs if dep["kind"] == "dev" else edges[name]).append(d)
            if d not in allowed:
                errs.append(f"EDGE {name} -> {d} (R1: not in allowlist)")

    color = defaultdict(int)                   # 0 白 / 1 灰 / 2 黑；递归找环
    def visit(n, path):
        color[n] = 1
        for d in edges[n]:
            if color[d] == 1: errs.append("CYCLE " + " -> ".join(path + [d]) + " (R2)")
            elif color[d] == 0: visit(d, path + [d])
        color[n] = 2
    for n in ALLOW:
        if color[n] == 0: visit(n, [n])

    def depth(n): return 0 if not edges[n] else 1 + max(depth(d) for d in edges[n])
    for n, want in DEPTH.items():
        if depth(n) != want: errs.append(f"DEPTH {n} expected {want} got {depth(n)} (R3)")
    for n, ds in devs.items():
        for d in ds:
            if DEPTH.get(d, 0) > DEPTH[n]: errs.append(f"EDGE {n} -> {d} (dev, R4)")
    if errs:
        print("\n".join(errs)); return 2
    print(f"check-deps: OK ({len(edges)} crates)")
    return 0

if __name__ == "__main__":
    sys.exit(main())
```

**失败形态**（CI 把原文贴在 PR 里）：

```
$ python scripts/check-deps.py
EDGE neo-ui -> neo-persist (R1: not in allowlist)
DEPTH neo-scene expected 4 got 6 (R3)
exit code 2
```

**负例自测**（CI job `rules-selftest`）：a) 给 `neo-ui` 加 `neo-io` 边；b) 造三元环；c) 删一个已声明 feature——断言分别以 `2` 退出且消息含 `R1`/`R2`/`R5`。不允许在没有负例的情况下合并白名单。**落地状态**：`scripts/check-deps.py` 已入库并由 CI 执行 R1（白名单）、R2（环）、R6（publish=false）；R3（深度表）、R4（dev 边）、R5（feature 表）仍需补齐。`cargo fmt` / `cargo clippy -- -D warnings` / `cargo test` 已在 CI 门禁中。

### 2.6 版本一致性策略（为何 GUI 栈锁 0.36.2）

| 事实（crates.io 依赖 JSON + 源码） | 后果 | 做法 |
|---|---|---|
| `egui_dock` 0.21.1（最新）要求 `egui ^0.36`；`egui-phosphor` 0.14.0 同样 `^0.36` | 上 0.37 会同时链接 `egui 0.36` 与 `0.37`：`DockState::show_inside` 的 `Ui` 类型不匹配（编译错），字体装进错误的 `Context`（运行时图标 tofu，静默） | GUI 三点（egui / eframe / egui-wgpu）全部钉 `0.36.2` |
| `egui-wgpu` 0.36.2 要求 `wgpu ^30.0`、`winit ^0.30.13`、`bytemuck ^1.25` | 降 egui **不会**降 wgpu | wgpu 保持 `30.0.2` |
| `geo` 0.33.1 经 `geo-types` 的 `rstar_0_12` 绑 `rstar 0.12` | 与 `rstar 0.14` 并存两棵树；`geo::Polygon` 不满足 `rstar 0.14::RTreeObject` | 关闭 geo 的 rstar 互操作，`neo-geo` 自写 newtype `RTreeObject` |
| `osmpbf` 0.3.8 的 `rust-zlib`/`zlib`/`zlib-ng` 互斥（源码 `compile_error!`） | 同时开两个直接编译失败，错误不直观 | 只允许默认 `rust-zlib`；切换需在 PR 与本表同时记录 |

**做法**：版本只在 `[workspace.dependencies]` 声明一次，crate 内一律 `{ workspace = true }`；`Cargo.lock` 入库、CI 用 `--locked`；**禁止** `cargo tree -d` 出现两份 egui/wgpu/winit/rstar 大版本；`rust-version` 统一为 **1.97**（由 `wasmtime 50` 决定），`rust-toolchain.toml` 固定到 1.100.0；CI 跑默认集与 `--all-features` 两种组合。

**升级演练**（升级点收敛在 `apps/neo-desktop` 与 `neo-render`）：① 读两边 CHANGELOG 的破坏性变更；② spike 只动 GUI 三点（必要时连 wgpu）；③ `cargo tree -d` 断言无重复大版本；④ 修入口层（`eframe::App` 形态、`RenderState` 字段、`Surface` 呈现）；⑤ golden 图 + 拾取一致率回归；⑥ 回填本节、[ADR-0005](./adr/0005-wgpu-egui-shared-device.md) 与 [16 风险 R1](./16-roadmap-and-risks.md)。锁文件变更需在 PR 贴 `git diff Cargo.lock` 摘要。

### 2.7 Feature 策略

默认构建是**最小可玩集**：能开文件、能编辑、能渲染、能保存，不含网络、数据库持久化与 WASM。理由：默认构建必须三平台零 C 依赖（`rusqlite bundled` 与 `wasmtime` 都带 C/CMake 负担），且 CI 要快。

| feature | 归属 crate | 默认 | 拉入 | 关闭后的行为 |
|---|---|---|---|---|
| `default` | 各 crate 组合 | on | core/osm/store/query/geo/style/scene/render/editor/tools/ui/desktop | — |
| `api` | `neo-api` + desktop | off | tokio 1.54、reqwest | 菜单灰化，Upload 返回 `Unsuitable` |
| `persist` | `neo-persist` + desktop | off | rusqlite 0.42（bundled）、rkyv | 仅内存 journal，禁用 autosave |
| `wasm-plugins` | `neo-plugin`；`neo-cli` 的 `plugins` 转发 | off | wasmtime 50 | 桌面不链接 wasmtime。CLI 的 `plugins` 子命令直接拒绝 |
| `validate` | `neo-validate` + ui | **on** | — | 校验面板空，上传前不阻塞 |
| `proj-db` | `neo-geo` | on | `proj` 数据库 | 仅 EPSG:3857/4326 |
| `pick-readback` | `neo-render` | on | — | 拾取退回 CPU（`neo-geo`） |
| `profile` | `neo-scene`、`neo-render` | off | puffin 0.20 | 场景构建与上传不打点 |

**约束**：feature 只能"加能力"，不得改变默认语义；增删必须同 PR 更新本表与 R5；resolver 固定 `"3"`；**禁止用 feature 表达权限**（权限靠 sealed trait + 调用点断言，见 §2.4）。

### 2.8 公共 API 稳定性

内部 crate 一律 `publish = false`，版本号由 workspace 统一继承（`0.1.x`），**不对外承诺 semver**；但跨 crate API 仍被 16 个消费者使用，等价于内部 semver：

| 规则 | 做法 | 反例 |
|---|---|---|
| 不漂漏实现细节 | 公开字段私有 + 构造函数；`#[non_exhaustive]` 用于枚举与配置结构 | `pub struct Snapshot { pub nodes: Vec<Node> }` |
| 枚举演进 | `ObjectKind`、`NeoError`、`EditError`、`EditorIntent`、`HostCall` 全部 `#[non_exhaustive]` | 下游 `match` 穷尽 → 加变体即破坏 |
| 可扩展点 | sealed trait（`WriteSealSource`、`GeometrySink`、`ElementSink`） | 让第三方实现 `EditCommand`（插件走 WASM ABI） |
| 类型化 ID | `ObjectId` / `Handle<T>` newtype，禁止暴露裸 `i64`/`u32` | 用 `i64` 传 way id，静默互换 node/way |
| 错误收敛 | 每 crate 一个 `thiserror` 枚举，跨层 `From` 收敛到 `NeoError`/`EditError` | `anyhow` 出现在库 crate 公开签名 |
| 版本化 ABI / 布局 | 插件 ABI 与 journal 各带 `u32` 版本号；禁止在公共 API 暴露 SoA 字段布局 | 无版本号静默误读；下游按偏移索引，重构即全仓编译失败 |

## 3 关键接口

```rust
// crates/neo-store/src/seal.rs —— 写许可封印（§2.4 G2）
pub struct WriteSeal(/* private */);
pub trait WriteSealSource { fn seal(&self) -> WriteSeal; }

// crates/neo-query/src/lib.rs —— 跨层只读查询（插件 / 验证器 / 后台任务 / UI 共用）
pub trait MapQuery { /* 签名见 01 §3 */ }
pub struct SnapshotCell { /* ArcSwap<MapSnapshot> */ }
impl SnapshotCell {
    pub fn load(&self) -> arc_swap::Guard<Arc<MapSnapshot>>;  // 单帧有效
    pub fn store(&self, snapshot: MapSnapshot);               // 唯一发布点
}

// crates/neo-ui/src/lib.rs —— UI 只依赖这些，不依赖 DataSet 写接口
pub struct UiContext<'a> {
    pub snapshot: arc_swap::Guard<Arc<MapSnapshot>>,  // 不得存入结构体字段
    pub intents: &'a dyn IntentSink,
}

// crates/neo-core/src/error.rs —— 跨层错误收敛点（#[non_exhaustive]）
pub enum NeoError {
    InvalidCoordinate(String), StaleHandle { kind: ObjectKind, index: u32 },
    DanglingReference { kind: ObjectKind, id: ObjectId, referrer_kind: ObjectKind, referrer: ObjectId },
    Parse(String), Consistency(String), Io(#[from] std::io::Error),
}
```

```toml
[workspace.dependencies]           # 版本只在此处出现一次；crate 内一律 { workspace = true }
neo-query = { path = "crates/neo-query" }
egui      = "0.36.2"               # GUI 三点同版本（§2.6）
wgpu      = "30.0"                 # egui-wgpu 0.36.2 要求 ^30
# neo-render 的内部边只允许三条：neo-core / neo-geo / neo-scene
```

## 4 数据流

```
改 Cargo.toml -> cargo build（crate 边界 / 类型防火墙失败即发现）
   -> pre-commit: python scripts/check-deps.py（R1..R6）
         ├─ 违规：打印 EDGE/CYCLE/DEPTH/FEATURE 行，exit 2
         └─ 通过：exit 0
   -> CI `deps` + `rules-selftest`：白名单只允许与 docs/02 §2.2 同 PR 修改后放行
   -> CI `check`/`test-min`/`test-full`/`msrv`：feature 组合可构建、MSRV 不漂移
   -> merge：表格与脚本白名单必须同一次 PR 变更（§7）
```

## 5 权衡与替代方案

| 选择 | 替代方案 | 为什么选它 | 代价 |
|---|---|---|---|
| 单 workspace（多 crate） | 单 crate + `mod`；或多 repo | crate 边界是编译器强制的；重构原子、CI 一次跑全图 | 改边要动多处 `Cargo.toml`；`pub` 蔓延风险 |
| 白名单脚本 | 只用 `cargo` 的环检测 | Cargo 只防环，不防"合法但错误"的方向（`ui→io` 无环但违反分层） | 需人工维护白名单（用负例自测压住腐化） |
| 权限靠类型 + CI | 权限靠 feature | feature 全局合并，不能按调用方区分 | 写入口仍是 `pub`，需要 `check-write-api.py` 兜底 |
| 版本集中在 `[workspace.dependencies]` | 各 crate 自行声明 | 一次升级全线生效，避免同一 crate 两个版本 | 某 crate 需要旧版本时冲突显性化（正是想要的效果） |

## 6 风险

| # | 风险 | 触发信号 | 缓解 |
|---|---|---|---|
| B1 | `egui`/`egui_dock`（或 `geo`/`rstar`）解析冲突拖住里程碑 | 首次 `cargo build` 报两份 egui | §2.6 的结论与证据；决策点在原型 Gate（16 §2.8） |
| B2 | 白名单被"顺手加一行"绕过 | PR 同时改脚本与文档但无 ADR | 白名单变更需 ADR 或双人评审；负例自测不可删 |
| B3 | 清单外依赖（bytemuck / HTTP / CLI / 字形）无治理 | 新 `[dependencies]` 无人审 | 走"依赖准入"小 ADR；登记在 16 风险 R1 |
| B4 | 全 feature CI 时长失控导致被跳过 | `test-full` 经常超时或被禁用 | 全量只跑 Windows+Linux 每日一次，PR 走最小集 |
| B5 | `no_std` 目标腐化（`neo-core` 引入 `std`） | `no_std` job 失败 | core 的 CI 单独加 `--no-default-features` 编译 |

## 7 验收标准

1. `python scripts/check-deps.py` 对当前树退出 `0`；违规时退出 `2` 并给出含 `R1`/`R2`/`R6` 的消息。
2. `cargo tree -p neo-render` 不含 `neo-store`/`neo-editor`/`neo-ui`；`cargo tree -p neo-ui` 不含 `neo-io`/`neo-api`/`neo-persist`。
3. `cargo build --workspace --no-default-features --features desktop` 在 Windows/Linux/macOS 均成功。
4. `cargo build --workspace --all-features` 成功，且 `wasmtime`/`rusqlite` 不进入默认构建产物（对比二进制体积与 `cargo tree`）。
5. `scripts/check-write-api.py` 对写许可的构造点只命中 `neo-editor`。
6. 依赖图深度与 §2.3 表零偏差（R3 通过）；新增边同 PR 更新本表，PR 描述含 `git diff Cargo.lock` 摘要。

## 8 参考

- Cargo workspaces 与 `[workspace.dependencies]`：https://doc.rust-lang.org/cargo/reference/workspaces.html
- Cargo feature 解析与合并（resolver v3）：https://doc.rust-lang.org/cargo/reference/features.html
- `cargo metadata` JSON 格式（脚本输入契约）：https://doc.rust-lang.org/cargo/commands/cargo-metadata.html
- Rust API Guidelines（`#[non_exhaustive]`、sealed trait）：https://rust-lang.github.io/api-guidelines/
- semver 与 0.x 兼容规则：https://semver.org/
- `egui_dock` 0.21.1 的依赖声明（要求 `egui ^0.36`）：https://crates.io/api/v1/crates/egui_dock/0.21.1/dependencies
- `egui-phosphor` 0.14.0：https://crates.io/crates/egui-phosphor/0.14.0
- `egui-wgpu` 0.36.2（要求 `wgpu ^30.0`）：https://crates.io/crates/egui-wgpu/0.36.2
- `geo` 0.33.1 的 rstar 依赖：https://crates.io/crates/geo/0.33.1
- `osmpbf` 0.3.8（zlib feature 互斥）：https://docs.rs/osmpbf/0.3.8/osmpbf/
- 版本核对记录（含源码与依赖 JSON 证据）：`.research/crate-api-cheatsheet.md`

> 待验证：当前树是否已做到 `cargo tree -d` 零重复（需一次完整 `cargo build` 后确认），结论回填本节与 [16](./16-roadmap-and-risks.md) 风险 R1。
