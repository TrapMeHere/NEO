# ADR-0003：OSM 数据用 SoA + Arena + generation 句柄存储

- **状态**：accepted
- **日期**：2026-11-15
- **相关**：docs/03、ADR-0004、ADR-0009

## 背景

OSM 数据是"大量小对象 + 稀疏引用"的图：

- 千万级 Node（ID、坐标、tags、元数据）；
- 百万级 Way（ID、节点引用序列、tags、元数据）；
- 十万级 Relation（成员、角色、tags、元数据）；
- 编辑时高频修改单个对象，并需要快速找到"引用它的 Way/Relation"。

Java JOSM 的 `OsmPrimitive` 对象模型适合 Java 的对象管理方式，但直接翻译到 Rust 会带来：

1. 每个对象一次堆分配 + 指针追逐（缓存不友好）；
2. `Vec<Box<Node>>` 无法做连续内存的批量处理（GPU 上传、并行几何构建）；
3. 删除/撤销后，引用对象可能出现悬垂指针或"旧句柄指向新对象"的静默错误。

## 决策

采用 **SoA + Arena + 类型化 generation 句柄**：

```text
NodeStore {
    ids:   Vec<ObjectId>       // 8 B
    lat/lon: Vec<i32>          // 4+4 B（定点 1e-7 度）
    tags:  Vec<TagSetId>       // 4 B（指向 Tag arena 的一个区间）
    meta:  Vec<Meta>           // 32 B
    alloc: SlotAllocator       // generations + alive + freelist
}
```

- **句柄**：`Handle<T> { index: u32, generation: u32 }`，8 字节，`Copy`，与 `T` 的 trait bound 无关。
- **失效检测**：槽位释放时 generation +1，旧句柄必然失效；`Handle` 永不复用旧代号。
- **引用池**：Way 的节点引用放在连续 `Vec<Handle<Node>>` 中，每 way 记录 `(offset, len)`；编辑时追加新区间、旧区间在 compact 时回收。
- **Tag arena**：所有 `(Sym, Sym)` 对连续存放，TagSet 只存区间；写路径 append-only，编辑无引用计数、无失效风险。
- **字符串驻留**：tag 文本只存一份，`Sym` 4 字节。
- **反向索引**：node→ways / node→relations / way→relations 用 CSR（一次 O(n+e) 构建，查询零分配）。
- **世界坐标不进存储**：存储只保留权威经纬度定点值；WebMercator 世界坐标由 `neo-geo` 按需计算。

## 替代方案

| 方案 | 放弃原因 |
|---|---|
| `Vec<Box<Node>>` / `Vec<Node>` + 指针 | 分配次数多、缓存差、无法批量 SoA 处理；删除 O(n) |
| `slotmap`/`generational-arena` crate | 每类对象一个泛型 arena 仍是 AoS；无法按字段批量处理（如并行几何构建只读 lat/lon/tags） |
| ECS（bevy_ecs / hecs） | 通用 ECS 的 archetype/Component 开销与 OSM 的"固定三类对象"不匹配；编辑事务语义需另建 |
| `im`/`rpds` 持久化数据结构 | 单对象修改成本高（路径复制），千万级下内存膨胀 |
| SQLite 作为主存储 | 每帧/每次拾取查询数据库不可能满足交互预算；只适合持久化/缓存（docs/10） |
| 对象内嵌 `Vec<(String,String)>` | tag 文本重复存储，10M 对象下多出数百 MB 且分配次数爆炸 |

## 后果

**收益**：

- 1000 万节点 ≈ 480 MB 数据区（48 B/节点），远低于 AoS 的 100+ B/节点 + 指针；
- 几何构建、空间索引构建可对整列做顺序扫描/并行；
- 删除安全：旧 Handle 不会静默指向新对象；
- 渲染/验证/插件读取的是不可变视图（`DataSet` 的 `&` 或快照），与写路径隔离。

**代价**：

- 需要手写 arena/句柄逻辑与不变量测试（`neo-core`/`neo-store` 已覆盖）；
- 编辑后引用区间追加会造成 arena 增长，需要 `compact()`（阶段 2 实现，见 docs/03 §2.7）；
- ID→句柄需要额外哈希表（约 20 B/对象）；可用"排序数组 + 二分"在只读阶段替换；
- 跨对象不变量（如 relation 成员顺序、way 节点顺序）必须由上层命令保证，存储层不校验。

## 验证

1. `neo-store` 单测：generation 失效、槽位复用、Way 引用区间连续、内存估算（48 B/节点）。
2. 基准（docs/14）：100 万对象 SoA 构建/查询/修改的 P50/P95；内存估算 vs 进程 RSS。
3. code review 检查点：任何"把 `Vec<Box<T>>` 加回存储层"的 PR 必须引用本 ADR 说明理由。
