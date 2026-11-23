# 路线图与风险

## 阶段

| 阶段 | 内容 | 状态 |
|---|---|---|
| 1 | 技术底座：workspace、数据模型、渲染原型、性能 Gate | ✅ |
| 2 | OSM 数据核心：PBF/XML/OSC、索引、DataSet、内存账本 | ✅ |
| 3 | 编辑核心：命令/事务/undo/journal、选择/拖拽/way 操作、JOSM 风格 UI | ✅ |
| 4 | 专业编辑：校验器、预设、MapCSS、OAuth、同步、冲突策略、SQLite 快照 | ✅ |
| 5 | 完整生态：WASM 插件、GPX、影像 URL、预设覆盖、CLI、打包脚本 | ⚠️ G5 Conditional Go（见 [gate-5](../prototypes/gate-5-ecosystem.md)） |
| 6 | 极限性能：流式 LOD、indirect draw、坐标压缩、并行场景构建 | ⚠️ 未达 RC（见 [gate-6](../prototypes/gate-6-performance.md)） |

实时进度与逐轮变更见仓库根目录
[TODO.md](https://github.com/TrapMeHere/NEO/blob/master/TODO.md)。

## 阶段 4 已交付（v0.1.0 基线）

- `neo-validate`：7 条规则 + 报告 + 右侧面板（运行/点击定位）
- `neo-style`：8 个内置预设（UI 下拉填入标签）
- `neo-api`：OSM API 0.6 协议客户端（changeset / diff upload / map 下载 / 409/412 解析）+ mock 传输测试
- `neo-editor`：MergeCase/ConflictResolution/标签三方合并 + 6 类冲突测试；journal 恢复报告 `RecoveryReport`

已交付：MapCSS 子集样式、真实 HTTP 同步 UI、KeepLocal/TakeRemote 冲突策略、SQLite 快照、
changeset 端到端（mock）测试。仍待：手合并 UI、MapCSS text/icon/过滤器全集、
上传前 validator gate、真实网络人工验收。

## 当前限制

- 单数据图层（多图层 UI 已预留）
- 插件宿主只在 `neo-cli --features plugins`，桌面不链接 wasmtime
- 三平台安装包脚本未执行；10M 新路径与 1 小时会话未测
- 快捷键为内置绑定（自定义绑定待做）
- Release 仅 Windows x64（Linux/macOS 源码构建）

## 主要风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| GPU/驱动差异 | 启动失败/渲染异常 | headless 测试 + 错误对话框 + 日志 |
| 超大 PBF 内存峰值 | OOM | 增量、并行导入、内存账本（docs/03） |
| 与 JOSM 行为差异 | 用户迁移成本 | parity 矩阵（docs/13）+ 实测 |
| 许可污染（JOSM 资源） | 法律风险 | clean-room + 资产隔离（docs/17） |
| 依赖升级破坏 wgpu/egui | 编译失败 | 锁定版本（ADR-0011）+ Dependabot 分组 |

## 参与

- 认领 Issue：`good first issue` / `help wanted`
- 设计讨论：Issue + ADR
- 性能数据：附机器信息与复现命令
