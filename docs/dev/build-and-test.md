# 构建与测试

## 前置

- Rust stable（`rust-toolchain.toml` 固定 stable + rustfmt + clippy；MSRV 1.95）
- Windows：Visual Studio Build Tools（MSVC），Windows 10/11 SDK
- Linux：`libxkbcommon`、Wayland/X11、Vulkan 驱动等（`eframe` 依赖）
- Python 3（仓库脚本：依赖检查、i18n、UI 冒烟）
- Node.js 22。预设数据跟 iD 一样用 npm 声明：`@openstreetmap/id-tagging-schema` 和 `name-suggestion-index`。编译 `neo-style` 之前先在仓库根目录执行 `npm ci`

## 常用命令

Windows 可直接双击 `scripts/build.cmd`（打开即构建）。它会装预设 npm 包、编 `neo-desktop`（不清理 `out/cargo`），再把 exe 与运行时依赖（`assets/fonts`、`assets/presets`）拷到 `out/neo-desktop/{debug|release}/`，与 exe 同层即可运行。可选参数：`-Release`、`-Run`、`-Workspace`。

```bash
# 预设包（编译 neo-style 之前）
npm ci

# 构建（release）
cargo build --release -p neo-desktop
# 或：pwsh -File scripts/build.ps1 -Release

# 运行
cargo run -p neo-desktop
# 或：pwsh -File scripts/build.ps1 -Run

# 格式化 / lint（CI 门禁）
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings

# 全量测试（master 已恢复测试源码）
cargo test --workspace

# 单 crate
cargo test -p neo-editor
cargo test -p neo-geo picking
```

`master` 的 CI 依次执行 format / clippy / check / test / `--all-features` check；`.github/workflows/test.yml` 保留 `test` 分支的历史基线。

## 测试分层

| 层 | 位置 | 内容 |
|---|---|---|
| 单元测试 | 各 crate `#[cfg(test)]` | 数据模型、几何、LOD、命令、i18n |
| 集成测试 | `crates/*/tests/` | PBF/XML/OSC roundtrip、并行导入取消、G3 验收 |
| 文档检查 | `scripts/check-*.py` | 依赖方向、写 API、文档链接、i18n key |
| UI 冒烟 | `test` 分支上的 `scripts/ui_check.ps1`（Windows） | 真实窗口 + 模拟鼠标/键盘 + 截图 |

```powershell
# Windows UI 冒烟：启动 → 画线 → 撤销 → 选择 → 拖拽 → 截图
powershell -ExecutionPolicy Bypass -File scripts\ui_check.ps1 start
powershell -ExecutionPolicy Bypass -File scripts\ui_check.ps1 draw
powershell -ExecutionPolicy Bypass -File scripts\ui_check.ps1 selectnode
```

截图输出在 `out/ui-check/`（已在 `.gitignore` 中）。

## 性能验证

```powershell
$env:NEO_PERF_LOG = "out\perf.log"
$env:NEO_SYNTH_N = "1000000"
.\out\cargo\release\neo-desktop.exe
```

日志包含 `scene build`、`gpu upload`、`ui frame`；阈值与基线见
[14 性能与验证](../14-performance-and-verification.md)。

## i18n

```bash
cargo test -p neo-i18n
python scripts/check-i18n.py
```

新增文本：编辑 `i18n/zh-CN.json` 与 `i18n/en-US.json`（`zh-TW.json` 可后补，缺失回退简体），代码使用
`neo_i18n::t("key")` / `tf("key", &[("n","3")])`。

## 文档站

```bash
cargo install mdbook --version 0.4.52 --locked
mdbook serve --open        # http://localhost:3000
mdbook build               # 输出到 book/
```

## 常见问题

- **链接错误 `LNK1104` / 找不到 MSVC**：安装 VS Build Tools 的 C++ 工作负载。
- **wgpu 初始化失败**：更新显卡驱动；Windows 需 DX12 支持。
- **字体/预设显示异常**：字体与 `schema.neo`、`suggestions.neo` 已外置。`scripts/build.ps1` /
  `package.ps1` 会把它们拷到 `out/neo-desktop/{debug|release}/assets/`（与 exe
  同层）。也可设 `NEO_ASSET_DIR`，或从仓库根的 `assets/` 开发加载。`neo.ico`
  仍由 `build.rs` 嵌入 exe。
- **体积基线**：见 [binary-size-baseline.md](../prototypes/binary-size-baseline.md)；
  复现 `pwsh -File scripts/bloat-baseline.ps1`。
- **UI 测试找不到窗口**：确保桌面会话已登录、目标 exe 为 release 构建。
