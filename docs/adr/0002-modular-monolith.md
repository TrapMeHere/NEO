# ADR-0002 · Modular Monolith（单进程模块化单体）

## 状态

- 已接受：2026-11-15。
- 可逆性：**低**。拆分或合并进程会波及所有 crate 的所有权与生命周期设计。
- 影响面：workspace 结构、`arc-swap` 快照共享、GPU 上下文所有权、插件隔离方式。
- 相关文档：[02](../02-workspace-and-dependency-rules.md)、[01](../01-system-architecture.md) §2.1/§5、[06](../06-editing-core-and-undo.md) §2.7。

## 背景

NEO 要同时满足三件互相拉扯的事：

- **零拷贝共享**：10M 对象必须在编辑、渲染、校验、持久化之间共享，不能每次都序列化；
- **全局写顺序**：编辑要可撤销、可重放、可崩溃恢复，因此写路径必须有唯一次序；
- **插件隔离**：插件可能恶意或崩溃，但不能因此破坏数据或进程。

可选形态：微服务（core daemon + 客户端）、多进程（UI/渲染/IO 分进程）、单 crate 巨石、单进程多 crate。

## 决策

采用**单进程模块化单体**：

1. 一个进程、一个地址空间；15 个库 crate + 2 个应用 crate 组成 workspace。
   - 依赖方向由 [02](../02-workspace-and-dependency-rules.md) §2.2 的白名单唯一确定（单向、无环）。
2. 进程内以 `Arc<Snapshot>` 共享只读数据：`arc-swap` 原子切换，后台任务不序列化、不拷贝全量数据。
3. 隔离靠 **crate 边界 + WASM 沙箱**，不靠进程边界：
   - 插件在 `wasmtime 50` 中运行，只能返回 `EditCommand` 或调用受限 host API；
   - host 不导出 `Snapshot::publish`，插件无法绕过验证管线。
4. GPU 上下文单一所有者（`eframe` 的 `RenderState`），`neo-render` 只借用 `&Device`/`&Queue`。

```
 ┌──────────── 一个进程 ────────────────────────────────────────┐
 │  neo-ui ──► neo-editor ──► neo-store ──► Arc<Snapshot>       │
 │                 │                            │               │
 │                 └── neo-tools                ├──► neo-scene   │
 │  neo-persist ◄──┘                            └──► neo-validate│
 │  neo-plugin (WASM 沙箱) ──EditCommand──► neo-editor           │
 └──────────────────────────────────────────────────────────────┘
```

## 替代方案

| 方案 | 优点 | 否决理由 |
|---|---|---|
| 微服务（core daemon + UI 客户端） | 崩溃隔离、可远程 | 每帧拾取/可见集重建都要过 IPC；快照传递需共享内存协议；延迟不可控 |
| 多进程（UI / 渲染 / IO） | 渲染崩溃不拖垮编辑 | 输入至少 +1 帧延迟；GPU 资源跨进程需 D3D12 shared handle / Vulkan external memory，三平台成本极高 |
| 单 crate 巨石 | 配置简单、无 `pub` 边界成本 | 编译器无法阻止 `ui → io` 之类错误依赖；模块边界只能靠自觉 |
| 插件即进程（每插件一进程） | 真隔离 | 与"插件走 Command"冲突：跨进程无法共享 `EditTx`；进程池管理成本高于 WASM |
| 动态库插件（cdylib，C ABI） | 性能最好、无沙箱开销 | ABI 不稳定、崩溃即全进程、无法做资源限额与权限控制 |

## 后果

**正面**

- 零拷贝快照 + 单一写者 ⇒ 确定的提交顺序；`cargo build` 即得完整依赖校验。
- 跨层性能剖析（`puffin`）可一次采样到底；调试只需一个进程。
- 插件与内建工具共享同一条 `EditCommand` 路径，语义不会分叉。

**负面**

- **无崩溃隔离**：驱动崩溃或 `abort` 会带走未保存编辑。缓解：journal（op log）+ 周期 autosave，把丢失窗口压到秒级（见 [10](../10-persistence-and-recovery.md)）。
- **`pub` 蔓延压力**：需要 `#[non_exhaustive]`、sealed trait、类型防火墙约束（见 [02](../02-workspace-and-dependency-rules.md) §2.4、§2.8）。
- **内存预算共享**：10M 对象 + 场景批次 + 几何缓存 + 撤销栈共用机器内存（见 [16](../16-roadmap-and-risks.md) R6）。
- **无法按团队拆仓**：所有改动进同一 workspace，CI 必须一次跑全图。

## 验证

1. `pwsh ./scripts/check-deps.ps1` 对当前树退出 0；对 `ui→io`、三元环、深度漂移三类负例退出 2（见 [02](../02-workspace-and-dependency-rules.md) §2.5）。
2. `cargo tree -p neo-render` 不含 `neo-store`/`neo-editor`/`neo-ui`；`cargo tree -p neo-ui` 不含 `neo-io`/`neo-api`/`neo-persist`。
3. 一次提交后，渲染与校验线程读到的 `Snapshot` 与写者发布的 `Arc` 满足 `Arc::ptr_eq`（无隐藏拷贝路径）。
4. 插件无法绕管：禁用 host 写 API 后，插件返回的 `EditCommand` 之外的任何尝试都不能改变 `version`（集成测试）。
5. 无跨进程通信代码：`grep -rn "TcpListener\|NamedPipe\|shared_memory" crates/` 为空（`neo-api` 的 HTTP 客户端除外）。
6. 崩溃预算：杀进程后恢复耗时与丢失事务数入基线【待实测，见 docs/14-performance-and-verification.md】。
