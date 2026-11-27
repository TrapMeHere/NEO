# 安装与运行

## 系统要求

| 项目 | 要求 |
|---|---|
| 操作系统 | Windows 10/11 x64（主要支持）；Linux/macOS 可源码构建 |
| 显卡 | 支持 DirectX 12（Windows）或 Vulkan/Metal，集成显卡可用 |
| 内存 | 建议 8 GB+；编辑 1000 万对象建议 16 GB+ |
| 磁盘 | release exe（代码）+ 旁路 `assets/fonts`（约 26 MB）+ `assets/presets`（约 15 MB） |

## 下载发行版（推荐）

1. 打开 <https://github.com/TrapMeHere/NEO/releases>
2. 下载最新版本的 `neo-desktop-vX.Y.Z-windows-x86_64.zip`
3. 解压到任意目录（不要放在需要管理员权限的目录）
4. 双击 `neo-desktop.exe`

发行包同时提供 `.sha256` 校验文件。可在 PowerShell 中校验：

```powershell
Get-FileHash .\neo-desktop-v0.1.0-windows-x86_64.zip -Algorithm SHA256
# 与 .sha256 文件内容对比
```

> release 版是无控制台 GUI 程序：双击不会弹出黑色命令行窗口。

## 从源码构建

前置：Rust stable（MSRV 1.97，仓库自带 `rust-toolchain.toml`）、
Windows 需安装 **Visual Studio Build Tools（MSVC）**。

```powershell
git clone https://github.com/TrapMeHere/NEO.git
cd NEO
cargo build --release -p neo-desktop
.\out\cargo\release\neo-desktop.exe
```

构建时间取决于机器，首次约 3–6 分钟（含 20+ 依赖与内置字体）。

## 首次启动

启动后地图为空。请通过 **文件 → 打开 PBF… / 打开 OSM XML…** 导入本地数据，
或用工具栏 **下载 OSM 数据** 从 OSM API 拉取。不内置示例地图。

## 卸载

NEO 是绿色程序：删除解压目录即可。编辑记录默认写入工作目录下的
`out/neo-journal.jsonl`，会话文件由用户显式保存。

## 常见安装问题

- **双击没反应**：release 版启动失败时会弹窗并写入 `out/neo-startup-error.txt`；
  请检查显卡驱动是否支持 DX12。
- **界面中文变方块**：0.1.0 已内置 CJK 字体，不应出现；若出现请提交 Issue 并附截图。
- **NVIDIA「Alt+Z」提示**：这是 NVIDIA Overlay 的全局提示，见 [FAQ](faq.md)。
