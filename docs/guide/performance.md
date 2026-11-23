# 性能与大数据

NEO 的目标是百万到千万级 OSM 对象仍然可编辑。以下数据来自
[14 性能与验证](../14-performance-and-verification.md) 的实测（Windows 11 / Rust 1.100）。

## 实测基线（摘要）

| 场景 | 数据 | 结果 |
|---|---|---|
| 导入 Bremen | 1.67M 节点 / 33 万 way | 1.12 s（1.79 M elem/s） |
| 场景重建（z16 编辑视口，1M 对象） | 4,164 way + 12,638 节点 | 1.29 ms P50 |
| GPU 帧（缓存命中） | 1M 对象 | 0.083 ms P50 |
| 拾取 | 节点/way | 8–22 µs |
| 单节点移动 + 父 way/索引更新 | — | 17 µs |

全城视角（fit-all）重建约 17–18 ms，是一次性开销（数据变化/大幅跳转才发生），
不是每帧成本。

## 渲染优化要点

- 节点/way 分开建 R-tree，视口 bbox 预裁剪，避免全量遍历
- 密度 LOD：亚像素 way 按 stride 抽样，绘制量接近“每屏常数”
- 节点只在必要缩放显示（z17–18 共享节点；z19 全节点）
- 几何增量上传；平移/缩放只更新 32 字节相机 uniform
- 拖拽只更新 egui 叠层预览，松开才写数据并重建一次

## 自己复现

```powershell
# 性能日志
$env:NEO_PERF_LOG = "out\perf.log"
.\out\cargo\release\neo-desktop.exe

# 合成数据规模（节点数）
$env:NEO_SYNTH_N = "1000000"
```

日志包含 `scene build`、`gpu upload`、`ui frame` 等时间戳，可与 docs/14 的表格对比。

## 大数据使用建议

- 1000 万对象建议 16 GB+ 内存、独立显卡或较新的集显
- 导入超大 PBF 前关闭其它内存占用大的程序
- 编辑时保持较高缩放（z16+）；全城视角主要用于导航
- 用 osmChange 导出增量，避免反复写全量 XML

## 与 JOSM 的差异

NEO 的编辑视口重建与 GPU 绘制是毫秒级；JOSM 基于 Java2D，1M 对象通常需要数十秒导入，
交互明显更慢。完整对照见 [13 JOSM parity](../13-josm-parity.md)。
