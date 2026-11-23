# 文件、会话与恢复

## 支持格式

| 格式 | 读 | 写 | 说明 |
|---|---|---|---|
| PBF（`.osm.pbf`） | ✅ | ✅ | 主格式；并行导入 |
| OSM XML（`.osm` / `.xml`） | ✅ | ✅ | 可读 JOSM 文件 |
| osmChange（`.osc`） | ✅ | ✅ | 只含变更，适合审阅/上传 |
| JOSM 会话（`.jos` / `.joz`） | ✅ | ✅ | 压缩包为 `.joz` |

## 打开文件

- **文件 → 打开 PBF… / 打开 OSM XML…**
- 大文件导入会显示进度，可取消；导入失败会在状态栏显示错误。
- 同一时间只保留一个数据图层；打开新文件会替换当前数据（请先保存）。

## 保存与导出

- **保存 XML（Ctrl+S）**：写当前数据集为 OSM XML
- **导出 osmChange**：只写本次会话的增删改（`create/modify/delete`）
- **导出 PBF**：写完整数据集为 PBF
- 保存前若存在未保存修改，工具栏显示「未保存」

## 会话文件

**文件 → 保存会话…** 生成 `.jos`（XML）或 `.joz`（ZIP，含数据快照元信息）：

- 保留视口（中心/缩放）、选择、图层信息与文件引用
- 打开会话时若数据图层引用的文件不可用，会提示错误
- 会话不是自动保存；请显式保存

## 自动保存（journal 快照）

程序每 60 秒把 journal 复制到 `out/autosave/neo-NNNN.jsonl`（保留 3 代），同时把 journal 字节
与统计写入 `out/autosave/neo-snapshots.sqlite`（保留 10 条，SQLite WAL）。状态栏显示
「已自动保存 journal 快照 #N」；若 journal 丢失，启动时会尝试从 SQLite 最近快照恢复。

## Journal（自动恢复）

所有编辑在提交历史时同步写入 **JSONL journal**（默认 `out/neo-journal.jsonl`）：

1. 程序异常退出后重新启动；
2. **编辑 → 从 journal 恢复**；
3. NEO 重放命令，把 journal 改名备份为 `.recovered`，并重新打开 journal。

限制：

- journal 只记录**命令**，依赖同一份基线数据；基线文件变化后可能无法恢复；
- 恢复失败会保留原 journal，不会删除。

## 数据安全建议

- 编辑前先备份源 PBF/XML；
- 重要编辑及时 `Ctrl+S` 或导出 osmChange；
- 定期查看 `out/neo-journal.jsonl` 是否增长（说明有未落盘编辑）。

## 环境变量（测试/运维）

| 变量 | 作用 |
|---|---|
| `NEO_PERF_LOG` | 写性能日志到文件 |
| `NEO_SYNTH_N` | 仅开发/性能测试：启动时生成 N 个节点的合成数据（正式产品默认为空图） |
| `NEO_START_ZOOM` | 指定初始缩放 |
| `NEO_TEST_SELECT` | 启动即选中 node/way（测试） |
| `NEO_LAYER_HIDDEN` | 启动时隐藏图层 |
| `NEO_LAYER_INACTIVE` | 启动即为只读灰显 |
| `NEO_SHOW_LAYER_INFO` / `NEO_SHOW_SETTINGS` | 启动即打开信息/设置窗口 |
