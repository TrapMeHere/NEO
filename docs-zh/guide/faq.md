# 常见问题（FAQ）

### 启动时弹出 NVIDIA「点击 Alt+Z，打开 NVIDIA 信息浮窗」

这是 **NVIDIA App / GeForce Experience 的 Overlay** 在检测到 GPU 渲染应用后弹出的提示，
不是 NEO 的窗口，也与 NEO 代码无关。NVIDIA Overlay 是驱动级全局注入，**没有按应用单独关闭的
官方开关**。

关闭方法（二选一）：

1. **NVIDIA App**：右上角 设置（齿轮）→ 功能 → **NVIDIA 信息浮窗 / Overlay** → 关闭；
2. **GeForce Experience**：设置 → 常规 → **游戏内覆盖** → 关闭。

只想在用 NEO 时关闭：先关 Overlay → 启动 NEO，用完再打开。参考
[NVIDIA 开发者论坛讨论](https://forums.developer.nvidia.com/t/how-to-prevent-in-game-overlay-from-triggering/256026)。

### 界面出现方框/豆腐块

0.1.0 已内置 Noto Sans CJK SC + GNU Unifont（含 Upper）作为 fallback，正常情况下不应出现。
若仍出现，请在 **帮助 → 关于 NEO** 的「字体覆盖示例」截图并提交 Issue。

### 地图一片空白 / 缩放后消失

- 隐藏了图层：检查右侧图层面板的**眼睛图标**；
- 节点 LOD：z ≤ 16 不显示节点是预期行为；
- 缩放回退空白是 0.1.0 已修复的 bug（场景重建触发条件 + GPU 上传 key），
  若仍复现请附上缩放级别与操作步骤。

### 节点点不中 / 点了节点却选中了 way

- 当前缩放低于节点显示阈值时节点不可拾取（按 `z` 提示缩放）；
- 节点与 way 各有 16px 外边距，优先节点；隐藏节点会回退到下方 way。

### 图层为什么变灰且不能编辑？

图层前的「活动」勾选框被取消：数据以灰度 + 50% 不透明度显示并只读。
重新勾选即可编辑；多图层时只有活动图层可编辑。

### 快捷键没反应

- 输入框（标签编辑）聚焦时快捷键被禁用，点击地图后重试；
- 窗口必须在前台；Windows 下其它程序（如 JOSM）抢占焦点时按键不会发送到 NEO。

### 中文标签/名称显示正常吗？

正常。数据本身 UTF-8；界面字体链覆盖 CJK，标签表格直接显示原始 Unicode。

### 编辑丢失了怎么办？

先看 `out/neo-journal.jsonl` 是否存在，再用 **编辑 → 从 journal 恢复**。
journal 依赖同一份基线数据；若基线文件已变，恢复可能失败，但不会删除 journal。

### 支持上传到 OSM API 吗？

0.1.0 尚未实现上传（阶段 5）。当前可导出 `.osc` 供 JOSM/其它工具上传。

### 有校验器和插件吗？

0.1.0 右侧「校验结果 / 冲突」为占位；插件系统（WASM）在阶段 6。

### 支持 Linux / macOS 吗？

代码是跨平台的（wgpu + eframe），但 0.1.0 的 release 仅提供 Windows x64；
Linux/macOS 可按 [安装文档](installation.md) 从源码构建。

### 数据版权

OSM 数据 © OpenStreetMap contributors，ODbL 1.0。NEO 不分发地图数据；
用户导入或下载的数据由用户自行遵守 ODbL。详见仓库 `NOTICE`。
