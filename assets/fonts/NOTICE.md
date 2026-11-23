点击安装内置字体

本目录包含 NEO 内置的开源字体（随可执行文件分发，作为 egui 的 fallback 字体，避免出现方框/豆腐块）：

| 文件 | 字体 | 用途 | 许可证 |
|---|---|---|---|
| `NotoSansCJKsc-Regular.otf` | Noto Sans CJK SC | 中日韩文字（含全部常用汉字，保证中文界面正常） | SIL Open Font License 1.1（OFL） |
| `unifont.otf` | GNU Unifont（Unicode BMP） | **兜底**：覆盖 BMP 几乎所有可打印码位，避免生僻字符/符号出现方框 | 双许可：SIL OFL 1.1 / GPLv2+ with GNU Font Embedding Exception（允许与自由或非自由软件一起使用） |
| `unifont_upper.otf` | GNU Unifont Upper（SMP / Plane 2 / Plane 3） | 补充平面兜底：扩展区汉字（如 𠀀 𪚥）、音乐/数学/图形符号等 | 同上 |

来源：
- Noto Sans CJK: https://github.com/notofonts/noto-cjk （SIL OFL 1.1）
- GNU Unifont: https://unifoundry.com/unifont/ （OFL 1.1 / GPLv2+ with font embedding exception）
- SIL OFL 1.1 全文见本目录 `OFL-1.1.txt`

界面图标字体使用 **Phosphor Icons**（MIT，通过 `egui-phosphor` crate）；OSM 数据 © OpenStreetMap contributors（ODbL）。

注意：字体仅用于界面渲染，不改变 OSM 数据版权。
