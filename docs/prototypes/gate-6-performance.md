# Gate-6 报告：极限性能

- **Gate id**：阶段 6 出口，发布候选（docs/16 §2.7）
- **评估日期**：2026-11-16
- **代码状态**：工作区未提交，基线 commit `add2418`
- **本轮跑过的命令**：`cargo test -p neo-scene -p neo-store -p neo-render --offline` 中与本阶段相关的用例；`neo-cli scene --nodes 50000`；`neo-cli open --synth 1000000`

10M 交互的既有数字在 [docs/14](../14-performance-and-verification.md)。那是阶段 6 之前的全量场景重建，不是本轮的流式 LOD / indirect draw。本文件不把那些数字算成新路径的成绩。

## 0 环境

| 项 | 值 |
|---|---|
| OS | Windows 11 10.0.26100 |
| Rust | 1.100.0 |
| 构建 | dev profile（optimized + debuginfo），不是 release LTO |
| GPU 测试 | `indirect_matches_direct_and_gpu_cull_drops_draw` 通过，没有走 `NoAdapter` 跳过。本轮没有重跑 `gpu-info` |

`profile` feature（puffin 0.20）默认关闭。本轮没有打开它，也没有帧剖析记录。

## 1 判定项

| # | 判定项 | 实测 | 判定 |
|---|---|---|---|
| (a) | 10M+ 平移 / 缩放 / 拾取 | 新路径未在 10M 上重测。dev 上 `scene --nodes 50000`：顶点 1431，构建 **2655 µs**，第二次同瓦片 `stream_builds=1`。LOD 单测：Overview 步长 8、City 步长 2、Street/Edit 步长 1；并行构建与串行的顶点数一致 | ❌ 10M 未测 |
| (b) | 内存峰值低于预算，1 小时会话不单调增长 | 重复 `intern("highway")` 不增加 `StringPool::mem_bytes`。坐标按纬度和经度分别做 zigzag + LEB128，三点样例压缩后小于 8 字节/点并可以解回。瓦片缓存容量有上界（`cache_stays_bounded`）。1 小时会话没有跑 | ⚠️ 原语通过，长会话未测 |
| (c) | 冷启动 | 没有 release 安装包冷启动。CLI 合成 1M 单次 302 ms，见 [gate-5](./gate-5-ecosystem.md) | ❌ 未测 |
| (d) | indirect 与 direct 画面一致，GPU cull 丢掉绘制 | 64×64 红色三角形：direct 与 `draw_indexed_indirect` 的 RGBA 逐字节相同；compute shader 把 indirect 的 `instance_count` 写成 0 后，下一帧全黑。测试耗时 0.67 s（含适配器初始化） | ✅ 这一张图 |
| (e) | 三平台性能差异有记录 | 只有这台 Windows。Linux / macOS 没有跑 | ❌ 未测 |

GPU 剔除是把整批的实例数写成 0，不是对每个顶点做视锥计算。CPU 视锥计数在 `neo-scene::cull`，没有接到上传时的 `instance_count` 上。默认绘制仍是 `draw_indexed`（`GpuScene.indirect` 默认 false）。

## 2 结论

**阶段 6 不是发布候选。**

已经落地并能复现的是：有界瓦片缓存、坐标/ID 差分 varint、字符串驻留不增长、多线程场景构建与串行一致、一张三角形上 indirect 与 direct 像素一致且 GPU 清零实例数后不再绘制。

没有落地的验收是 10M 新路径的帧时间、1 小时内存曲线、release 冷启动，以及三平台对比。这些保持未测，不借用 docs/14 的旧基线充数。
