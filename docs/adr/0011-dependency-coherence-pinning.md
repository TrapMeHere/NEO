# ADR-0011：依赖一致性与钉版策略

- **状态**：accepted
- **日期**：2026-11-15
- **相关**：docs/02 §2.9、ADR-0005

## 背景

2026-11 的 crate 生态出现两处**编译期不可调和**的版本冲突，直接在"选最新版"的默认策略下会炸：

1. `egui_dock 0.21.1` 与 `egui-phosphor 0.14.0` 都声明 `egui ^0.36`，而 `egui 0.37.0` 晚于它们发布；若把 GUI 栈升到 0.37，Cargo 会同时链接两份 egui，`DockState`/`TabItem`/`Ui` 跨边界类型不匹配。
2. `geo 0.33.1` 通过 geo-types 的 `rstar_0_12` 特性绑定 `rstar 0.12`；若同时使用 `rstar 0.14`，geo 的几何类型不满足 0.14 的 `RTreeObject`，且默认特性还会拉入第二个三角化实现 `earcut 0.4.5`。

这类问题的特征：**不是 API 变化，而是 semver 区间本身互斥**；`cargo build` 之前无从发现。

## 决策

1. **GUI 栈钉在 `egui/eframe/egui-wgpu 0.36.2`**，wgpu 保持 **30.0.2**（egui-wgpu 0.36.2 仍要求 `wgpu ^30`，GPU 侧无需回退）。`egui_dock 0.21.1`、`egui-phosphor 0.14.0` 因此可共存。
2. **空间索引直接用 `rstar 0.14`**，为 NEO 自有类型（`SpatialEntry`）实现 `RTreeObject`；**不启用** geo 的 rstar 互操作。`geo` 若引入必须 `default-features = false` 并显式开启所需特性（避免第二套 earcut）。
3. **单一版本事实来源**：所有外部版本只在根 `Cargo.toml` 的 `[workspace.dependencies]` 声明一次，成员 crate 用 `{ workspace = true }`。
4. **锁文件入库**：`Cargo.lock` 提交，保证三平台/CI 解析一致。
5. **CI 断言**：
   - `cargo tree -d` 中出现 `egui`/`wgpu`/`rstar` 的重复主版本即失败；
   - `cargo metadata` 校验所有 `egui*`/`wgpu*` 成员落在锁定 minor；
   - MSRV 由 `rust-version = "1.95"` 声明（`egui_dock 0.21.1` 的 MSRV 即 1.95）。
6. **升级演练**：每次升级 GUI/GPU 依赖，只允许改 `apps/neo-desktop` 与 `neo-render` 两处适配层；`neo-ui`/`core` 等不得感知版本（ADR-0004 的接触面检验）。

## 替代方案

| 方案 | 放弃原因 |
|---|---|
| 用 egui 0.37 + 自研 docking | 丢掉成熟 dock 实现，且 phosphor 图标也需手写；收益仅"版本更新" |
| vendor/fork egui_dock 到 0.37 | 长期维护成本高，与上游修复脱节；等上游发布即可 |
| 钉 rstar 0.12 迁就 geo | 放弃 0.14 的索引改进；NEO 的索引本来就不需要 geo 类型 |
| 不锁版本，随机解析 | 三平台结果不同，CI 不可复现 |

## 后果

- 短期无法使用 egui 0.37 的新特性（需要在 UI 层绕开的地方很少）。
- `egui_dock`/`egui-phosphor` 升级到支持 0.37 的版本后，可一次性整体升级（ADR 验证项 6）。
- `rstar` 与 `geo` 之间若需要互操作，写一个 6 行的 newtype 适配器（`#[repr(transparent)]` + 转发 `envelope()`），不允许全局换版本。

## 验证

1. `cargo tree -d | grep -E '^(egui|wgpu|rstar|geo) '` 无重复主版本。
2. `cargo check --workspace` 通过（已在本机 Windows/MSVC 与 1.100.0 验证）。
3. 升级演练记录写入 `docs/14` 附录（每次依赖升级一条）。
