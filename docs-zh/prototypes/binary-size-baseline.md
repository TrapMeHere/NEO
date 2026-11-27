# 二进制体积基线（外置字体 + 外置 presets）

- **日期**：2026-11-21
- **命令**：`cargo bloat --release -p neo-desktop --crates --n 40`
- **复现**：`powershell -File scripts/bloat-baseline.ps1`
- **profile**：`[profile.release]` opt-level=3, lto=thin, codegen-units=1, strip=true

## 对比

| 项 | 外置前（约） | 外置后 |
|---|---:|---:|
| `neo-desktop.exe` | ~65 MB | **23.4 MB** |
| 字体 | 编进 exe（~26 MB） | `assets/fonts/` 旁路 26.4 MB |
| 预设 | 编进 exe（~15 MB） | `schema.neo` 0.19 MB + `suggestions.neo` 6.70 MB |

安装包仍需带上 `assets/`；主程序本身小了约 **40 MB**。

## 发行物体积

| 项 | 大小 |
|---|---:|
| `out/cargo/release/neo-desktop.exe` | 23.44 MB |
| `assets/presets/schema.neo` + `suggestions.neo`（外置） | 0.19 + 6.70 MB |
| `assets/fonts/*`（外置，合计） | 26.36 MB |

## `.text` 前几名（cargo-bloat）

| Crate | 约占用 |
|---|---:|
| std | 2.6 MiB |
| naga | 1.7 MiB |
| egui | 1.1 MiB |
| wgpu_core | ~1.0 MiB |
| wgpu_hal | 0.8 MiB |
| rustls | 0.4 MiB |
| neo_desktop | 0.3 MiB |

完整表：[binary-size-bloat-crates.txt](./binary-size-bloat-crates.txt)。

下一步若要再瘦：可选 feature 关掉 `neo-api`/http（rustls、reqwest、aws_lc）或 `neo-persist`（rusqlite）；图形栈（naga/wgpu/egui）难拆。
