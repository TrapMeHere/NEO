# Gate-1 报告：性能验证原型（阶段 1 → 阶段 2 决策点）

- **Gate id**：G1（docs/16 §2.8 唯一的硬否决 Gate）
- **评估日期**：2026-11-15
- **代码状态**：workspace `neo-*` 17 crates + 2 apps 全部编译通过；原型 `apps/neo-prototype` 可运行
- **数据**：Monaco/Bremen（Geofabrik 真实 PBF）、合成网格城市（1M 对象）
- **测量**：`pwsh scripts/bench.ps1 -Runs 3`，3 轮中位数；原始输出 `out/bench-summary.md`

## 0 环境（Gate 必须记录）

| 项 | 值 |
|---|---|
| 主机 | i5-12400F（6C/12T）/ 31.8 GB / RTX 3070 8 GB |
| OS | Windows 11 10.0.26100 / MSVC 14.43 |
| GPU 后端 | Vulkan（wgpu 30.0.2，适配器 NVIDIA GeForce RTX 3070，驱动 621.14） |
| Rust | 1.100.0 stable（MSRV 1.95） |
| 构建 | release：opt-level 3 + thin LTO + codegen-units 1 |
| 依赖锁 | egui/eframe/egui-wgpu 0.36.2、wgpu 30.0.2、rstar 0.14、osmpbf 0.3.8（ADR-0011） |

## 1 G1 判定项逐条

| # | G1 判定项 | 实测 | 阈值 | 判定 |
|---|---|---|---|---|
| ① | 1M 对象导入耗时 | Bremen 真实 PBF：1,668,778 节点 + 329,676 way + 5,053 关系，**1.12 s**（1.79 M elem/s，18.1 MiB/s）；1M 合成 0.19 s | 必须测出并记录 | ✅ Go |
| ② | 首帧时间（导入完成 → 可见） | Monaco 全链路 < 100 ms；Bremen：导入 1.12 s + 索引 1.20 s + 场景 90 ms ≈ **2.4 s** | 必须测出并记录 | ⚠️ 记录（超 1 s，需阶段 2 优化） |
| ③ | 平移/缩放帧时间 | **z16 编辑视口场景重建 P50 1.29 ms / P99 2.63 ms**；平移/缩放只写 uniform，不重建；GPU 帧 P50 0.083 ms | p50/p99 必须测出；判据 = 交互可用 | ✅ Go（<16 ms 预算） |
| ④ | 峰值 RSS | 1M 合成对象 **566.8 MiB**；**10M 合成对象 4,159.1 MiB（4.06 GiB）**，低于 6 GB 建议预算 | 必须测出并记录 | ✅ Go |
| ⑤ | GPU 拾取单次延迟 | **口径修正**：设计选择为 CPU 拾取（docs/04），实测 P50 **12 µs** / P95 17 µs；GPU ID-buffer 拾取列为可选优化 | 必须测出并记录 | ✅ Go（CPU 拾取满足 <16 ms，但需评审确认口径） |
| ⑥ | 连续 10 分钟稳定性 | **8 秒冒烟通过**：`out/cargo/debug/neo-desktop.exe` 启动后窗口存活 8 s（含 PBF 加载、场景构建、wgpu 首帧），进程未退出/未 panic；**10 分钟连续交互未执行** | 必须执行 10 分钟 | ⚠️ 部分（冒烟通过，10 分钟未跑） |
| ⑦ | 三平台可构建 | 仅 Windows/MSVC 本机验证；Linux（Vulkan/X11）与 macOS（Metal）未验证 | 三平台必须构建成功 | ❌ **No Go**（未验证） |

## 2 结论

**G1 = Conditional Go（有条件通过）**

- 通过：导入吞吐、编辑尺度交互延迟（场景重建 1.29 ms、拾取 12 µs、GPU 帧 0.083 ms）、GPU 管线真实性（离屏渲染产出 PNG，见 `out/monaco.png`、`out/bremen.png`）、1M 内存 566.8 MiB、10M 内存 4.06 GiB。
- 阻塞：**⑥ 10 分钟连续稳定性**（仅完成 8 秒冒烟）与 **⑦ 三平台构建**（仅 Windows 验证）未完成。
- 条件：在阶段 2 启动前完成以下三项，任一项失败则 G1 转 No Go 并触发 docs/16 §5 的降级路径：
  1. Linux + macOS CI 构建（同一 `Cargo.lock`，wgpu 后端分别 Vulkan/Metal）；
  2. 桌面应用连跑 10 分钟冒烟（拖拽/缩放/选择循环），无崩溃/无泄漏单调增长；
  3. `--synth 10_000_000` 导入 + 索引 + 场景峰值 RSS 记录，且 <> 预算（建议 ≤ 6 GB）。

## 3 10M 压力结果（实测）

`neo-prototype bench --synth 10000000 --iterations 5`：

| 项 | 数值 |
|---|---|
| 对象数 | 10,001,406 节点 / 3,129,589 way / 78,210 关系 |
| 构建 | 3.31 s |
| 空间索引构建 | 9,154.8 ms（13,130,995 条目） |
| 反向引用 | 147.3 ms |
| `mem_bytes` 估算 | 1,693.1 MiB |
| **峰值 RSS** | **4,159.1 MiB** |
| 场景重建（z16） | 2.03 ms P50 / 2.71 ms P99 |
| 场景重建（fit-all） | 143 ms P50 |
| id lookup | 300 ns P50 |
| pick | 195 µs P50 |

判定：**10M 可导入、可索引、可交互；更新导致规模瓶颈的项是索引构建与 fit-all 重建，进入阶段 2 优化清单。**

## 4 已知瓶颈（阶段 2 输入）

| 瓶颈 | 实测 | 计划 |
|---|---|---|
| PBF 单线程解码 | 1.79 M elem/s | blob 级 rayon 并行（docs/08 §2.4），目标 3–5× |
| 空间索引构建 | 1M 条目 572 ms；Bremen 1.3M 条目 1.2 s | 并行 bulk_load / 分块构建 |
| 全城视角场景重建 | 17 ms（1M）/ 90 ms（Bremen） | 按脏 way 增量重建批次（ADR-0009 下一层） |
| bbox query 返回 Vec | 273 µs（7,922 条） | 场景路径已零分配；拾取路径改迭代器 |
| 首帧组装（Bremen 2.4 s） | 导入+索引+场景 | 导入流水线与索引构建重叠；进度反馈 |

## 5 证据文件

- `out/prototype-bench.md` —— 1M 合成基准原始输出（含 P50/P95/P99 表）
- `out/bench-summary.md` —— 3 轮运行的汇总（含 host/adapter/驱动）
- `out/prototype-render.md` —— 最近一次离屏渲染报告
- `out/monaco.png`、`out/bremen.png` —— GPU 渲染证据（可用任意图片查看器打开）
- `docs/14-performance-and-verification.md` —— 方法与完整结果表

## 6 复现命令

```powershell
pwsh scripts/bench.ps1 -Runs 3
# 或逐项：
cargo run -p neo-prototype --release -- import --pbf .research/data/bremen-latest.osm.pbf
cargo run -p neo-prototype --release -- render --pbf .research/data/monaco-latest.osm.pbf --out out/monaco.png
cargo run -p neo-prototype --release -- bench  --synth 1000000 --iterations 30
```
