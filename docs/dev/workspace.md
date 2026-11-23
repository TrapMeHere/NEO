# Workspace 与 crate 职责

Cargo workspace 的成员是 `crates/*` 与 `apps/*`；版本、依赖版本、lint 集中在根 `Cargo.toml`。
依赖方向由 [docs/02](../02-workspace-and-dependency-rules.md) 定义，并由
`scripts/check-deps.py` 强制检查。

## crate 一览

| crate | 职责 | 允许依赖（核心） |
|---|---|---|
| `neo-core` | ID、经纬度/世界坐标、分代句柄、Meta、错误 | 无（叶子） |
| `neo-osm` | OSM 实体、变更状态、tag 模型 | core |
| `neo-store` | `DataSet`（SoA+Arena）、rstar 索引、反向引用、完整性 | core, osm |
| `neo-geo` | 相机、投影、世界坐标、拾取、几何 | core, osm, store |
| `neo-scene` | 可见性/LOD、批处理、顶点生成 | core, osm, store, geo, style |
| `neo-render` | wgpu 管线、GpuScene、headless 渲染 | core, geo, scene（**不依赖 store/editor/ui**） |
| `neo-io` | PBF/XML/osmChange 读写、合成数据 | core, osm, store |
| `neo-editor` | Command/Transaction、History、Journal、Selection、Editor | core, osm, store, geo |
| `neo-tools` | 高层编辑动作（选择/绘制/拖拽/合并等） | core, editor |
| `neo-persist` | JOSM 会话 `.jos`/`.joz` | core, osm, store |
| `neo-ui` | egui 面板、主题、i18n 调用、UiActions | core, osm, store, geo, scene, render, editor, tools, i18n |
| `neo-i18n` | zh-CN/en-US 文本目录（编译期嵌入） | 无 |
| `neo-api` | OSM API 0.6 / 上传（阶段 5） | core, osm |
| `neo-query` | 跨层只读查询（插件/校验/任务） | core, osm, store, geo |
| `neo-validate` | 校验规则（阶段 4） | core, osm, store, geo |
| `neo-style` | MapCSS / 样式（阶段 4） | core, osm |
| `neo-plugin` | WASM 插件宿主（阶段 6） | core, query, editor |
| `apps/neo-desktop` | 桌面应用：eframe + wgpu + 面板编排 | 全部 |
| `apps/neo-prototype` | 性能原型/Gate 验证 | core, osm, store, io, geo, scene, render |

## 依赖规则

1. **禁止反向依赖**：例如 `neo-store` 不能依赖 `neo-editor`，`neo-render` 不能依赖 `neo-store`。
2. **GUI 只读**：`neo-ui` 不依赖 `neo-io`/`neo-persist`/`neo-api`，IO 由应用层执行。
3. **写接口收敛**：对 `DataSet` 的写入只能经 `neo-editor` 的命令/事务（`check-write-api.py` 检查）。
4. **新增 crate**：先更新 docs/02 的图与表，再在根 `Cargo.toml` 加依赖版本；
   `cargo check` 与 `python scripts/check-deps.py` 在 `master` 上必须通过；`cargo test` 在 `test` 分支上跑。

## 常用检查

```bash
python scripts/check-deps.py        # 依赖方向
python scripts/check-write-api.py   # 写 API 封印
python scripts/check-doc-links.py   # 文档链接
python scripts/check-i18n.py        # i18n key
```
