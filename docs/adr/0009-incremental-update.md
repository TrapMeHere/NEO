# ADR-0009 · 增量更新（Incremental Update）

## 状态

- 已接受：2026-11-15。
- 可逆性：**中**。撤回意味着回到"每版本全量重建"，10M 规模下不可交互。
- 影响面：`neo-scene` 批次模型、`neo-geo` 缓存键、`neo-render` 上传路径、`neo-store` dirty 标记。
- 相关文档：[07](../07-concurrency-and-tasks.md) §2.4、[04](../04-spatial-index-and-geometry.md)、[06](../06-editing-core-and-undo.md) §2.3。

## 背景

10M 对象下，一次拖动只改变 1 个坐标和它所属的若干 way。但"每次提交重建全部场景"意味着 O(N) 几何构建 + O(N) GPU 上传：任何交互都会掉到个位数帧率，上传带宽与驱动同步还会制造尖峰。

因此增量更新不是优化项，而是**可交互性的前提**；它同时决定了缓存键的设计（否则"版本一变全部失效"等于全量重建）。

## 决策

1. **dirty 以对象为单位传播**：提交管线第 6 步 `dirty.mark(touched)` 记录对象与依赖闭包（node → 所属 way → 所在 relation 的 bbox），只标记必要批次。
2. **缓存键含版本区间**：几何缓存键为 `(ObjectId, Handle.gen, version_range)`；对象在 `[v0, v1)` 未变更即命中。
3. **场景批次带脏区间**：`SceneFrame { batches, dirty: Vec<DirtyRange> }`，`DirtyRange { buffer_id, byte_offset, len }` 直接映射 GPU 字节区间。
4. **buffer 池按容量分级复用**：不足则换更大 buffer（copy 旧数据），足够则原地 `write_buffer`。
   - 小写入直传；大写入走 staging ring + `copy_buffer_to_buffer`。
5. **回收条件**：`frame_no + in_flight < current_frame` 才允许回收槽位（`in_flight` 默认 2，可配 3）。
6. **空间索引增量维护**：dirty 对象做 rstar remove+insert；整库导入仍用 `bulk_load`（见 [08](../08-io-pbf-xml-osc.md)）。
7. **LOD 先行**：低缩放用简化几何与标签聚合，只对越阈值且入视野的批次做精细构建。
8. **每帧上传预算**：`upload_cap` 超出即把剩余 dirty 推到下一帧（逐帧追上，不制造卡顿尖峰）。

```
提交(touched) ──► dirty 闭包 ──► 失效几何缓存条目
                                   │
                                   ▼
                     SceneFrame.dirty: [(buffer, offset, len)]
                                   │
                                   ▼
              BufferPool::ensure ──► write_buffer / staging+copy
                                   │
                                   ▼
                    frame_no + in_flight < current ⇒ 可回收
```

## 替代方案

| 方案 | 优点 | 否决理由 |
|---|---|---|
| 每版本全量重建 `SceneFrame` | 实现最简单、绝无漏算 | 10M 下 O(N) 不可行；上传带宽与驱动同步造成尖峰 |
| 双缓冲整场景（A/B 交替） | 无部分更新风险 | 显存翻倍（RTX 3070 8GB 直接排除大场景） |
| CPU 端光栅缓存（tile 位图）+ 合成 | 平移廉价、无几何重建 | 缩放/样式切换需重光栅；拾取需另建映射；与 GPU-first 冲突 |
| GPU compute 剔除 + 间接绘制 | 可处理超大规模可见集 | 首版复杂度过高；保留为阶段 6 的演进方向，不作基线 |
| 只做到 layer 粒度的"整批重建" | 比全量简单 | 单节点移动仍重建整层，10M 下不可接受 |
| 每帧只重建视野内对象（无脏区间） | 无需缓存键 | 视野内仍有百万对象；平移即全量，抖动明显 |

## 后果

**正面**

- 一次编辑的代价正比于"受影响对象数"，而非数据集规模；帧时间可预测。
- 显存按需增长（池化）；拾取 id 表只需重建脏批次。
- 与 LOD 正交：低缩放时连精细几何都不构建。

**负面**

- **失效范围算错 = 画面与数据不一致**，且往往只在特定操作序列下复现；需要 property test 与 dirty 高亮调试。
- **缓存键复杂化**：`version_range` × `Handle.gen` 组合容易出现"永不失效"的泄漏条目，需定期压缩。
- **顶点布局必须稳定**：批次内对象顶点顺序不可随意重排，否则退化为整批上传。
- **显存碎片**：容量分级 + 池化带来内部碎片，需要上限与整理策略。

## 验证

1. 10M 数据集移动 1 个节点：重建批次数 ≤ 受影响 way 数 + 1，上传字节 ≤ `upload_cap`（`PrepareStats` 计数）。
2. 缓存键正确性：两版本间未变更对象的几何构建调用次数为 0（打点计数）。
3. 回收安全：`in_flight = 2` 下连续三帧写入，断言不覆盖未完成帧（`frame_no` 断言 + 校验和比对）。
4. 视觉一致性：随机 10k 次编辑后与"每次全量重建"参考实现逐像素比对（离屏渲染，容差 0）。
5. 泄漏：连续 1 小时编辑后几何缓存条目数与显存回到基线附近【阈值待实测，见 docs/14-performance-and-verification.md】。
6. LOD 稳定：阈值附近来回缩放 100 次，同批次重建次数有上限（无抖动）。

## 参考

- `egui_wgpu` 的 `CallbackTrait`（每帧提交点）：https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/
- wgpu `Queue::write_buffer` 与 staging 上传：https://docs.rs/wgpu/30.0.2/wgpu/struct.Queue.html
- rstar 的 `bulk_load` 与增量 `insert`/`remove`：https://docs.rs/rstar/0.14.0/rstar/
- earcutr（三角化）：https://docs.rs/earcutr/0.5.0/earcutr/
