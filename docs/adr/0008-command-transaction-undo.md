# ADR-0008 · Command + Transaction + Undo

## 状态

- 已接受：2026-11-15。
- 可逆性：**低**。`EditCommand` 形状已冻结；改动会波及所有工具与插件 ABI。
- 影响面：`neo-editor` / `neo-tools` / `neo-plugin` 的全部写入路径；`ChangeSet` 与 OSC 生成。
- 相关文档：[06](../06-editing-core-and-undo.md)、[ADR-0004](./0004-independent-data-render-editor-models.md)、[ADR-0006](./0006-single-writer-editor.md)。

## 背景

编辑需要同时满足五项：可撤销（含跨工具、跨插件）、可崩溃恢复、可生成 OSC、可 headless 测试、可被 WASM 插件安全产出。

若允许工具直接改 `DataSet`，这五项都要各自发明机制（事后 diff、全量快照、UI 录制……），且彼此不一致。JOSM 的做法是 `Command` + `UndoRedoHandler`，但命令直接持有并修改 `DataSet`、撤销依赖命令自带的 `undo()`——一旦某个插件命令写错，历史即损坏。

## 决策

1. **命令只描述意图**：`EditCommand::apply(&mut self, tx: &mut EditTx) -> Result<CmdOutcome, EditError>` 只向影子层记录 `EditOp`；命令无法在 `apply` 里绕过事务。
2. **逆操作在应用时捕获**：`EditTx` 为每个 op 记录前像（删除时存完整对象）；`undo` 不重新推导。
   - `EditCommand::undo` 保留给非结构副作用（选区、changeset 记账、插件内部状态）；
   - 默认实现由 `AutoUndo` 包装器注入捕获到的逆操作。
3. **提交管线固定 12 步**：验证 → journal begin → apply → inverse → journal commit+sync → dirty → 几何失效 → 引用集 → 空间索引 → version+1 → publish → notify。
   - 顺序由集成测试锁定；journal 永远先于 publish。
4. **undo 是一次新事务**：version 无条件 +1，不回滚版本号；订阅者不需要区分正反向。
5. **ChangeSet 由"像"生成，不由历史生成**：`ChangeSetAccumulator` 累积提交产物；OSC 写侧比较 base 像与当前像。
   - History 可被截断而 ChangeSet 不会。
6. **命名隔离**：对象级合并情形叫 `MergeCase`（`neo-editor`），服务端 409 报告叫 `ConflictKind`（`neo-api`）；两者由 `apps/neo-desktop` 的 `sync_bridge` 转换。
7. **合并（coalescing）**：仅当同一 `merge_hint` + 500ms 窗口 + 同一批对象；上传后的条目不再合并到上传前。

```
工具/插件 ──► Box<dyn EditCommand>
                 │ apply(&mut tx)
                 ▼
            EditTx（影子层：inserts/updates/deletes）
                 │ validate()  ──失败──► 零副作用返回
                 ▼ commit()
   journal(begin,ops) → DataSet → inverse → journal(commit+sync)
                 │
                 ▼ version+1 → publish(Arc<Snapshot>) → notify
```

## 替代方案

| 方案 | 优点 | 否决理由 |
|---|---|---|
| Memento：每事务前全量拷贝 | 实现极简，undo 绝对正确 | 10M 对象下每次编辑拷贝整库不可行；内存峰值不可控 |
| 命令自带 `undo()` 且直接改数据（JOSM 式） | 灵活、插件易写 | 正确性依赖每个命令作者；无统一逆向捕获；崩溃恢复无法重放 |
| 不可变持久数据结构（`im`/`rpds` 风格） | 天然快照、无锁 | Rust 生态不成熟；每次修改 O(log n) 分配；GPU 侧仍需 SoA |
| 事件溯源但无事务边界 | 日志即真相 | 单条事件无原子性，UI 中间态会泄漏；校验与索引更新无法原子 |
| 只记录 post-image 的日志 | 恢复简单（直接覆盖） | 体积大；无法表达"创建后删除"的消解；undo 仍需前像 |
| 每次编辑都存整库快照（SQLite 事务） | 崩溃恢复强 | 10M 下写放大不可接受；与 GPU 增量更新无法衔接 |

## 后果

**正面**

- 撤销/日志/OSC/冲突四条路径共享同一提交点，语义不会分叉。
- 命令可在无 UI 的测试中驱动；插件只能产出命令，无法破坏历史。
- 事务原子性使"验证失败零副作用"可被断言。

**负面**

- **逆操作占内存**：批量操作会把前像钉在 History 中，必须设上限并允许"不可撤销的大操作"。
- **replay 依赖代码版本**：`EditOp` 的 rkyv schema 变更后旧日志不可读，必须显式拒读（`schema: u16` 门）。
- **管线顺序是隐式契约**：重排步骤会造成"数据比日志新"的崩溃窗口，只能靠测试守。
- **`EditCommand: Send` 约束**（插件在 worker 产出命令）意味着命令不能捕获 `Rc`/UI 句柄。

## 验证

1. 属性测试：随机命令序列 + 随机验证失败注入，断言失败时 `version` 与内容零变化。
2. undo→redo 后 `Snapshot` 除 version 外逐字段相等（含 way 节点顺序、relation 成员顺序）。
3. 管线顺序：在"journal commit 后、publish 前"注入崩溃，replay 后状态等于已提交状态（`tests/commit_order.rs`）。
4. 合并规则：60 帧拖拽 = 1 条历史；间隔 > 500ms = 2 条；上传后不可合并。
5. OSC 等价性：同一批编辑生成的 OSC 与 JOSM 对等操作语义一致（元素集合与 `version` 属性；mock 服务端校验）。
6. 插件约束：插件返回的命令无法调用 `Snapshot::publish`（host 不导出该符号）。

## 参考

- OsmChange（OSC）规范：https://wiki.openstreetmap.org/wiki/OsmChange
- OSM API v0.6 diff upload 与 409：https://wiki.openstreetmap.org/wiki/API_v0.6#Diff_upload
- JOSM 撤销/命令栈行为基准：https://josm.openstreetmap.de/wiki/Help/Action/Undo
- rkyv 0.8 归档与 schema 演进注意事项：https://docs.rs/rkyv/0.8.18/rkyv/
