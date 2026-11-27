# ADR-0007 · 用 WASM Component 作插件 ABI

- **状态**：已接受（Accepted），2026-11-15
- **决策者**：架构组
- **影响面**：`neo-plugin`（新）、`neo-editor`（命令入口）、`neo-ui`（面板注册）、`neo-api`（令牌隔离）
- **相关**：[15 插件系统](../15-plugin-system.md) · [02 Workspace 与依赖规则](../02-workspace-and-dependency-rules.md) · [06 编辑核心与撤销](../06-editing-core-and-undo.md)

## 背景

NEO 要替代 JOSM 工作流，就必须允许第三方扩展：JOSM 的插件生态（校验器、影像源、导入器、绘制工具）是它长期存活的真实原因之一。若 NEO 不提供插件机制，"完整替代 JOSM"这个目标不成立。

问题在于**用什么作为 ABI**。候选有三类：

1. **Rust 动态库**（`cdylib` + 手写 `extern "C"`）；
2. **`abi_stable 0.11.3`** 的 `StableAbi` 动态库；
3. **WASM**（core module 或 Component Model）。

Rust 至今没有稳定 ABI。`abi_stable` 解决的是"结构体布局稳定"，但解决不了三件事：(a) 插件必须由**同一 rustc 版本**编译，否则 `std` 的 `TypeId`/trait object 布局可能不兼容；(b) 插件与宿主各自静态链接同一 crate 时，全局单例（`tracing` subscriber、`wgpu::Instance`、`rayon` 线程池）会**各有一份**，出现"日志丢失""GPU 设备冲突"这类难查故障；(c) 插件代码与宿主同进程同权限，一个 `unsafe` 或死循环就能拖垮编辑器。对一个要处理用户未上传编辑、持有 OAuth 令牌的桌面应用，这三条都不可接受。

WASM 的代价是明确的：需要一份跨边界契约（WIT）、需要宿主把能力逐个显式开放、跨边界调用有序列化成本。但这些代价是**一次性的、可测试的、可版本化的**；而 ABI 漂移的代价是持续的、不可预测的。

## 决策

插件的唯一 ABI 是 **WASM Component Model**，运行时为 `wasmtime 50.0`，契约由 `wit-bindgen 0.63` 生成的 WIT world `neo:plugin@1.0.0` 定义。具体条款：

1. **契约版本化**：`package neo:plugin@x.y.z` 用语义化版本；新增可选导出走 minor，改类型或删函数走 major。宿主可同时链接多个 major 版本。
2. **能力白名单**：插件 `manifest.toml` 声明所需能力（`command`/`ui`/`events`/`network`/`filesystem`/`imagery`），运行时按"声明 ∩ 用户授权"放行；`read`（只读快照查询）是唯一隐式能力。
3. **写路径唯一**：插件不能触碰 `neo-store`，只能 `submit-command` 入队，由 `neo-editor` 的提交管线执行——因此插件的每次修改都可校验、可入 journal、可撤销。
4. **UI 声明式**：插件返回绘制列表，由宿主用 egui 渲染；不暴露 `egui::Context`，防止任意绘制与帧阻塞。
5. **资源硬限**：`consume_fuel` + `epoch_interruption` + 内存上限（默认 64 MiB）+ 单实例串行派发；超限即 trap 并禁用该插件，不影响其他插件。
6. **C ABI 桥是例外通道**：极少数必须访问本地库的插件（如专有格式 SDK）可走独立的 C ABI 子进程桥，需单独 ADR，**不**作为默认路径。
7. **签名**：正式分发要求 `.neopkg` 内含对 `manifest.toml` + `component.wasm` 的 detached 签名；未签名仅开发者模式可加载。

## 替代方案

| 方案 | 为什么被否决 |
|---|---|
| `cdylib` + 手写 `extern "C"` | 每个类型都要手工约定 `Vec`/`String` 生命周期；跨边界即 UB 风险；无沙箱 |
| `abi_stable 0.11.3` | 只解决布局，不解决 rustc 版本耦合与依赖单例冲突；仍无沙箱；每平台每架构一份二进制 |
| 子进程 + 自定义 IPC | 隔离最好，但每次查询都要序列化往返，交互式面板（选择变化即刷新）延迟不可接受；作为高危插件的可选模式保留 |
| 内嵌 JS 引擎（JSR-223/GraalJS 等价） | 引入第二个沙箱面与显著体积；脚本类需求先由 `neo-validate` 声明式规则覆盖 |
| 不做插件 | 直接放弃"替代 JOSM 工作流"目标 |

## 后果

**正面**

- 插件二进制跨平台、跨 NEO minor 版本可用；分发是单文件 zip。
- 供应链与权限可审计：安装时展示权限清单，运行时二次校验。
- 插件崩溃、死循环、超内存都被隔离在自身实例内；宿主数据不可能被直接破坏。
- 强制走命令管线，插件产生的编辑自动获得 undo/journal/冲突检测能力。

**负面 / 成本**

- 需要维护 WIT 契约与向后兼容测试（golden 插件 + CI 矩阵）。
- 跨边界调用的数据形态受限（无引用传递、无零拷贝），批量查询必须设计成"一次多取"。
- 插件作者需要 Rust（或任意 WASM 源语言）+ 组件模型工具链，学习曲线高于写 Java 插件。
- 宿主需要在 `apps/neo-desktop` 实现 `HostApi`（读快照/提交命令/HTTP/文件），该实现天然跨层，是新的耦合点。

**必须补齐的前置条件**

- `neo-plugin` 的合法依赖集（`neo-core`, `neo-editor`）中**没有**只读空间查询能力（查询与索引在 `neo-store`）。需按 [15 §2.7](../15-plugin-system.md) 的方案 A 把 `MapQuery` trait 下沉到 `neo-core`（或新增 `neo-query` 叶子 crate），并在 [02](../02-workspace-and-dependency-rules.md) 白名单中登记。**此项未完成前，插件只能看到选择集，`query-bbox` 无法实现。**
- 签名验签库未在依赖清单内，需单列选型（Ed25519）。

## 验证

| 验证项 | 方法 | 通过标准 |
|---|---|---|
| 契约兼容 | 保留 v1 golden 插件，CI 用新宿主加载 | 加载成功且行为不变 |
| 权限隔离 | 未声明 `command` 的插件提交命令 | 返回 `permission-denied`，`neo-editor` 无任何事务 |
| 资源隔离 | 死循环 / 超内存 / 超时插件 | trap 后宿主帧循环正常，同进程其他插件继续工作 |
| 数据安全 | 插件尝试直接改 store | 编译期不可能（无此导入），运行期无接口 |
| 版本门 | `api = "2.0"` 的插件 | 拒绝加载 + 可读错误 |
| 可分发性 | 在一台无 Rust 工具链的机器上安装 `.neopkg` | 加载并运行成功 |
| 性能 | 事件派发与面板渲染开销 | 【待实测，见 docs/14-performance-and-verification.md】 |
