# 架构总览

NEO 是**模块化单体**（modular monolith），不是微服务。核心原则：

1. **业务逻辑不依赖 GUI**：编辑/IO/查询可在无 UI 下测试与运行。
2. **地图渲染不依赖 UI 控件**：渲染层只接收不可变 `SceneFrame`。
3. **数据模型不依赖渲染格式**：`DataSet` 是 SoA + Arena，GPU 顶点由 `neo-scene` 单独构建。
4. **单写者**：所有修改经 `neo-editor` 的 Command/Transaction，GUI 不得直接改 `DataSet`。
5. **增量**：几何/索引/GPU 上传都是增量的，平移缩放只更新 uniform。

## 分层

```
apps/neo-desktop        eframe + wgpu 集成、输入翻译、面板编排
      │
crates/neo-ui           egui 面板（纯函数：只读数据 + UiActions）
crates/neo-tools        EditController：选择/绘制/删除/拖拽/分割/反转/合并
      │
crates/neo-editor       命令、事务、历史、journal、选择集、Editor
      │
crates/neo-io  neo-persist  neo-api（阶段 5）
      │
crates/neo-scene        SceneBuilder、LOD、批处理
crates/neo-render       wgpu 管线、GpuScene、headless 渲染
crates/neo-validate / neo-query / neo-plugin / neo-style（阶段 4–6）
      │
crates/neo-store        DataSet（SoA + Arena）、索引、反向引用、完整性
crates/neo-osm          OSM 实体/变更类型/tag 模型
crates/neo-geo          世界坐标、相机、拾取、几何
crates/neo-core         ID、坐标、分代句柄、Meta、错误
crates/neo-i18n         界面文本目录（编译期嵌入）
```

## 一次编辑的数据流

```
egui 输入
  → neo-tools::EditController（pick + command）
  → neo-editor::Command（事务/历史/journal）
  → DataSet 变更（version++）
  → neo-scene::SceneBuilder（增量重建）
  → neo-render::GpuScene 上传
  → wgpu draw（egui paint callback）
```

拾取独立于渲染：`neo-geo::HitTester` 用同一份索引，按屏幕外边距选出节点/way。

## 关键不变量

- 句柄带 generation，删除后旧句柄失效（不会指向新对象）。
- `DataSet::version` 每次结构性变更递增；场景缓存以它和 (origin, zoom) 为 key。
- 被 way/relation 引用的节点不能直接删除（命令返回 `Referenced`）。
- 拖拽预览不写数据；松开鼠标才产生一次批量移动命令。
- 图层只读时：picking/拖拽/绘制/测距全部禁用，选择被清空。

## 深入阅读

- [01 系统架构](../01-system-architecture.md)
- [02 Workspace 与依赖规则](../02-workspace-and-dependency-rules.md)
- [03 数据模型与存储](../03-osm-data-model-and-store.md)
- [04 空间索引与几何](../04-spatial-index-and-geometry.md)
- [05 wgpu 渲染器](../05-renderer-wgpu.md)
- [06 编辑核心与 Undo](../06-editing-core-and-undo.md)
- [ADR 列表](../SUMMARY.md)
