# ADR-0004 · 数据 / 渲染 / 编辑三模型相互独立

## 状态

- 已接受：2026-11-15。
- 可逆性：**中**。它决定了 `neo-store` / `neo-scene` / `neo-render` / `neo-editor` 四个 crate 的划分与公开签名。
- 影响面：`SceneFrame` 的 POD 约束、类型防火墙、增量更新策略（[ADR-0009](./0009-incremental-update.md)）。
- 相关文档：[01](../01-system-architecture.md) §2.3、[02](../02-workspace-and-dependency-rules.md) §2.4、[05](../05-renderer-wgpu.md)。

## 背景

OSM 编辑器最容易腐化的地方是"让渲染直接读数据结构、让编辑直接改渲染对象"。JOSM 的 `OsmDataLayer` 把数据、绘制数据与命令混在一起，结果是：加一种渲染特性要改数据层；渲染缓存与编辑一致性靠约定；单测必须拉起 UI。

NEO 要在 10M 对象上做增量更新、并行场景构建与 GPU 拾取，必须先把"三种模型"的边界钉死，否则每一层都会向上伸手。

## 决策

系统内存在三种**互不共享可变状态**的模型，只允许单向投影：

```
 编辑模型                     数据模型                        渲染模型
 EditCommand/EditTx  ──提交──► DataSet ──publish──► Snapshot ──build_scene──► SceneFrame
 (write-set, 影子层)          (SoA, 唯一可变)      (不可变, Arc)        (POD 顶点/批次)
      ▲                                                                        │
      └────── 工具意图（EditCommand） ◄── ObjectId ◄── GPU id pass ◄────────────┘
```

| 模型 | 归属 crate | 内容 | 谁可读 | 谁可写 |
|---|---|---|---|---|
| 数据模型 | `neo-store`（实体来自 `neo-osm`） | `DataSet`（SoA 列、索引、引用集、dirty）、`Snapshot`（冻结视图） | 所有上层只读 | 仅 `EditTx`（经 `Editor`） |
| 编辑模型 | `neo-editor` / `neo-tools` | `EditCommand`、`EditTx`、`History`、`ChangeSet`、`Conflict` | `neo-ui`、`neo-plugin` | 仅编辑器自身 |
| 渲染模型 | `neo-scene` / `neo-render` | `SceneFrame`（批次、`MapVertex`、pick id 表）、GPU buffer | `neo-render` 独占 | 仅 `neo-scene` 构建、`neo-render` 上传 |

**四条硬约束**：

1. `SceneFrame` 是 POD：不含 `Arc`、不含 `Snapshot` 引用、不含 `ObjectRef` 借用；拾取 id 是 `u32`，映射表由 `SceneFrame` 自持。
2. `neo-render` 的公开签名里不出现 `neo_store`/`neo_editor` 类型（类型防火墙）。
3. `Snapshot` 没有 `&mut self` 方法；渲染持有的永远是某一版本的不可变视图。
4. 编辑模型不生产渲染数据；渲染模型不懂 OSM 语义（不知道什么是 way、标签、relation）。

## 替代方案

| 方案 | 优点 | 否决理由 |
|---|---|---|
| ECS 共享 world（实体/组件/系统） | 并行系统、数据导向 | 编辑需要"全局顺序 + 逆操作"，ECS 并行系统天然打散顺序；way→node 引用集不是组件关系 |
| 渲染直接从 `Snapshot` 逐帧绘制（无中间模型） | 无派生内存、无一致性问题 | 10M 对象下逐图元绘制不可行；无法按 layer 合批；GPU 拾取无处挂 id |
| 保留式场景图（retained scene graph） | 天然增量、结构稳定 | 场景图与数据生命周期纠缠，删除/撤销易出悬垂节点；样式切换要遍历图 |
| 编辑直接改渲染对象，反向同步回数据 | 少一次拷贝 | 数据不再单一真相；撤销/上传/校验全部失去依据 |
| 单一"上帝模型"（数据里带渲染缓存字段） | 访问最快、无需转换 | 缓存失效逻辑散落各处；序列化/持久化被缓存污染；测试无法分离 |

## 后果

**正面**

- 三模型可独立测试：headless 编辑测试、`SceneFrame` 快照测试、GPU 冒烟测试互不依赖。
- 场景构建可在 Rayon 上并行（只读 `Snapshot`），编辑与渲染互不阻塞。
- `SceneFrame` 是纯数据，可用 `bytemuck` 直传 GPU；Pick id 表使拾取不依赖几何反查。

**负面**

- **派生内存**：`DataSet` 与 `SceneFrame` 同时存在；10M 对象下必须靠 LOD 与可见集裁剪，否则内存翻倍（R6）。
- **转换成本**：每次提交要重建受影响批次；失效范围算错会出现"画面滞后于数据"（用 version 键与 dirty 区间压制）。
- **映射维护**：pick id 表随批次重建，拾取结果必须校验 `Handle.gen`，否则撤销后可能命中已删对象。

## 验证

1. 类型防火墙：`neo-render` 的 API 快照中不出现 `neo_store`/`neo_editor` 路径。
2. POD 性：对 `SceneFrame`/`MapVertex` 做 `bytemuck::Pod` 编译断言，且结构体不含 `Arc`/`&`/`String`。
3. 指针同一性：提交后渲染线程拿到的 `Snapshot` 与写者发布的 `Arc` 满足 `Arc::ptr_eq`。
4. 一致性：随机 10k 次编辑 + 场景重建后，`pick_ids` 中每个 id 都能在对应 `Snapshot` 版本解析到对象（property test）。
5. 独立性：移除 `neo-render` 依赖后，`neo-editor` 全部测试仍通过。
6. 拾取正确性：`Handle.gen` 失配时必须返回 `None`（不返回旧对象），构造删除-重建序列测试。

## 参考

- OSM 数据模型（Node/Way/Relation 语义）：https://wiki.openstreetmap.org/wiki/Elements
- JOSM 图层与绘制模型（反面教材与行为基准）：https://josm.openstreetmap.de/wiki/Help/Concepts/Object
- `bytemuck`（POD 与零拷贝上传）：https://docs.rs/bytemuck/latest/bytemuck/
- wgpu 顶点缓冲布局：https://docs.rs/wgpu/30.0.2/wgpu/struct.VertexBufferLayout.html
