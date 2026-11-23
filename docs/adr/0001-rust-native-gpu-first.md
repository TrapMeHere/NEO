# ADR-0001 · Rust Native + GPU-first

- **状态**：已接受（Accepted），2026-11-15
- **可逆性**：**低**。它决定进程模型、渲染所有权、UI 框架与 16 个 crate 的依赖 DAG，改动的代价接近重写。
- **影响面**：`apps/neo-desktop`、`neo-ui`、`neo-render`、`neo-scene`、`neo-store`；全部性能相关文档。
- **相关**：[01 系统架构](../01-system-architecture.md) ｜ [02 依赖规则](../02-workspace-and-dependency-rules.md) ｜ [05 渲染器](../05-renderer-wgpu.md) ｜ [16 路线图与风险](../16-roadmap-and-risks.md) ｜ [ADR-0005](0005-wgpu-egui-shared-device.md)

## 状态

Accepted。若后续证据（原型 Gate 指标、后端差异、维护成本）推翻其中任一子决策，另立 ADR 记录替代方案，不静默修改本文。

## 背景

NEO 的目标是**完整替代 JOSM 的编辑工作流**，数据规模 1M–10M+ OSM 对象，原生运行在 Windows / Linux / macOS。JOSM 是 Java + Swing：功能完备、生态成熟，但堆内存与逐图元绘制模型在大数据集上有天花板，启动与输入延迟也受 JVM 影响。

摆在面前的是三个独立但互相牵制的选择：

1. **语言与运行时**：继续 JVM（Java/Kotlin）、.NET，还是 Rust 原生？
2. **绘制模型**：沿用 CPU immediate-mode（Swing/egui 自带绘制），还是自建 GPU 管线？
3. **UI 外壳**：Tauri/Electron（WebView）、Qt/GTK（C++ 绑定）、还是 Rust 原生 immediate-mode（egui）？

三者不能分开选：选 WebView 就等于放弃自建 wgpu 管线（需要独立窗口 handle 嵌入，三平台方案不统一）；选自建管线就意味着 UI 层必须与同一个 `Device`/`Queue` 共享帧；选 Rust 原生则意味着不能复用 JOSM 的 GPL 代码（静态链接或改写都会传染许可），语义兼容只能靠行为对齐。

## 决策

**Rust Native + GPU-first，单进程模块化单体。**

| # | 子决策 | 具体内容 |
|---|---|---|
| D1 | 语言与运行时 | Rust stable（本机 1.100.0，MSRV 1.95），无 GC、无 JVM；`#![forbid(unsafe_code)]` 作为库 crate 默认 |
| D2 | 进程模型 | 单进程、多线程；UI 线程是唯一写者，其余只读快照（[ADR-0006](0006-single-writer-editor.md)） |
| D3 | UI 栈 | egui / eframe / egui-wgpu **0.36.2**（三点同版本）；版本由 `egui_dock 0.21.1` 与 `egui-phosphor 0.14.0` 的 `egui ^0.36` 约束 |
| D4 | 渲染 | 自研 wgpu **30.0.2** 地图渲染器，与 egui 共享 `Device`/`Queue`，通过 `egui_wgpu::CallbackTrait` 在 egui 的渲染通道内绘制（[ADR-0005](0005-wgpu-egui-shared-device.md)） |
| D5 | 不以"移植 Java 类"方式重构 | 不做 JOSM 类的一对一翻译，不做 JNI/转译桥；只按行为等价重写（parity 以工作流定义，见 [13](../13-josm-parity.md)） |
| D6 | 不用 Tauri / Electron | 不使用 WebView 作为地图承载层，也不把数据面放进 JS/IPC |

## 替代方案

| 方案 | 优点 | 否决理由 |
|---|---|---|
| Java/Kotlin 继续演进（复刻 JOSM 架构） | 可直接参考现有实现与插件生态 | 堆内存与 GC 峰值不可控；10M 对象下 `HashSet<Node>` 式模型无法达标；GPU 管线要跨 JNI，收益被抵消 |
| Tauri + React（WebView 前端） | Web 生态、前端人才、UI 迭代快 | WebView 无法承载自建 wgpu 管线（三平台嵌入方案不统一）；10M 图元经 IPC/JS 桥不可行 |
| Electron | 同上，且更成熟 | 同上，外加 100 MB+ 运行时与内存开销 |
| Qt（`qmetaobject` / `cxx` 绑定） | Dock、无障碍、IME 成熟；原生观感 | LGPL/商业双许可与静态链接合规成本；C++ 对象生命周期跨越 Rust 借用检查，FFI 成为长期 bug 源 |
| GTK（`gtk-rs`） | Linux 原生感最好 | Windows 上观感与 IME 明显落后；三平台一致性差 |
| 纯 CPU 渲染（egui `Mesh` 直接画地图） | 零新基础设施 | 每帧重新 tessellate 百万级线段，CPU 与内存都不可行 |
| 直接复用 JOSM 代码（JNI / 转译 / 改写） | 语义免费对齐 | GPL-2.0+ 传染；JVM 启动与堆模型与 GPU-first 冲突；等于把性能问题搬进新进程 |
| 浏览器 WASM 版（顺带产物） | 传播成本低 | wasm32 4 GB 地址空间装不下 10M 对象 + 索引；文件系统受限 |

## 后果

**正面**

- 单进程单地址空间：快照切换零拷贝，拾取与渲染同帧，无 IPC 延迟。
- 无 GC：帧时间可预测；内存布局（SoA）可显式控制，10M 对象才有可行路径。
- 自建 wgpu 管线：批次化、实例化、GPU 拾取、文字图集都可按需设计，不被 UI 框架的 immediate-mode 限制。
- 许可干净：MIT/Apache-2.0 双许可，不与 GPL 资产混用；JOSM 只作行为参考。
- 依赖面可控：`neo-render` 只依赖 `wgpu` + `egui-wgpu` + `bytemuck`，不依赖 store/editor/ui。

**负面 / 代价**

- **生态自负**：Dock、表格、属性编辑器、i18n、无障碍都要自己在 egui 上搭；`egui_dock` 只是 Dock 的起点。
- **GUI 栈锁版本**：0.36.2 是为兼容 `egui_dock`/`egui-phosphor` 而选的；0.36 → 0.37 是破坏性升级（入口 trait 形态、`RenderState` 字段变化），升级需专门演练（[02](../02-workspace-and-dependency-rules.md) §2.6）。
- **受 egui 通道约束**：不能改 load op、不能跨 callback 复用深度、颜色格式必须与 swapchain 一致；管线参数错误是**运行时**校验错误而非编译错误。
- **单进程无崩溃域**：插件/驱动崩溃会带走全部未保存编辑，只能靠 journal + autosave + WASM 沙箱兜底。
- **不自研渲染线程**：UI 三职合一（输入 + 命令 + 提交），帧预算更紧；极端场景只能降 LOD。

## 验证

1. **可构建**：三平台 `cargo build --workspace --no-default-features --features desktop` 成功（最小编译集）。
2. **单一设备**：全仓源码中 `Instance::new` / `new_without_display_handle` 出现次数为 1（grep 断言）；`cargo tree -p neo-render` 不含 `neo-store`/`neo-editor`/`neo-ui`。
3. **原型基线**：`apps/neo-prototype` 在参考机跑通"1M 对象导入 → 渲染 → 平移缩放 → 拾取"，四项基线（导入耗时、首帧、帧时间 p50/p99、峰值 RSS）测出并记录；不达标则触发 [16](../16-roadmap-and-risks.md) §2.8 的 G1 否决流程。
4. **版本一致性**：`cargo tree -d` 中 egui / wgpu / winit / rstar 无重复大版本；`Cargo.lock` 变更出现在 PR 描述中。
5. **许可**：`neo-render`/`neo-ui` 的依赖树不含 GPL 传染型许可（审计脚本 + 人工核对，见 [17](../17-licensing-and-governance.md)）。
6. **行为等价的可否证性**：parity 清单（[13](../13-josm-parity.md)）里每条都是可执行检查或手工步骤，不存在"架构上应该一致"这类无法证伪的表述。

## 参考

- JOSM 项目与源码（GPL-2.0+，仅行为参考）：https://josm.openstreetmap.de/
- JOSM 许可说明：https://josm.openstreetmap.de/wiki/Source
- eframe 0.36.2：https://docs.rs/eframe/0.36.2/eframe/
- egui-wgpu 0.36.2（`CallbackTrait` / `RenderState`）：https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/
- wgpu 30.0.2：https://docs.rs/wgpu/30.0.2/wgpu/
- `egui_dock` 0.21.1 的 `egui ^0.36` 约束：https://crates.io/api/v1/crates/egui_dock/0.21.1/dependencies
- Qt 许可（LGPL/商业）：https://www.qt.io/licensing
- WebGPU 规范：https://www.w3.org/TR/webgpu/

> 待验证：0.37 线相对 0.36.2 的 `eframe::App`/面板 API 差异（`App::ui` 入口在 0.36 已就位，需在升级演练时逐条核对）；egui 的无障碍（accesskit）覆盖度是否满足桌面编辑器的最低要求。
