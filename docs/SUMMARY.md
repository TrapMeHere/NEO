# Summary

[项目介绍](introduction.md)

# 用户指南

- [安装与运行](guide/installation.md)
- [快速上手](guide/quick-start.md)
- [界面说明](guide/interface.md)
- [编辑操作](guide/editing.md)
- [文件、会话与恢复](guide/files-and-sessions.md)
- [设置与多语言](guide/settings-and-i18n.md)
- [OSM 同步（下载 / 上传）](guide/sync.md)
- [性能与大数据](guide/performance.md)
- [常见问题（FAQ）](guide/faq.md)

# 开发指南

- [架构总览](dev/architecture.md)
- [Workspace 与 crate 职责](dev/workspace.md)
- [构建与测试](dev/build-and-test.md)
- [贡献流程](dev/contributing.md)
- [发布流程（CI / Release / Pages）](dev/release.md)
- [路线图与风险](dev/roadmap.md)

# 技术设计文档

- [00 愿景与范围](00-vision-and-scope.md)
- [01 系统架构](01-system-architecture.md)
- [02 Workspace 与依赖规则](02-workspace-and-dependency-rules.md)
- [03 OSM 数据模型与存储](03-osm-data-model-and-store.md)
- [04 空间索引与几何](04-spatial-index-and-geometry.md)
- [05 wgpu 渲染器](05-renderer-wgpu.md)
- [06 编辑核心与 Undo](06-editing-core-and-undo.md)
- [07 并发与任务](07-concurrency-and-tasks.md)
- [08 IO：PBF/XML/osmChange](08-io-pbf-xml-osc.md)
- [09 OSM API 与同步](09-osm-api-and-sync.md)
- [10 持久化与恢复](10-persistence-and-recovery.md)
- [11 样式、预设与 MapCSS](11-style-presets-mapcss.md)
- [12 校验器](12-validation.md)
- [13 JOSM 功能对照](13-josm-parity.md)
- [14 性能与验证](14-performance-and-verification.md)
- [15 插件系统](15-plugin-system.md)
- [16 路线图与风险](16-roadmap-and-risks.md)
- [17 许可与治理](17-licensing-and-governance.md)
- [18 测试策略](18-testing-strategy.md)
- [功能完成度与 iD/JOSM 对比](feature-parity.md)
- [体检与修复报告（2026-11-21）](health-audit-2026-11-21.md)

# 架构决策记录（ADR）

- [0001 Rust Native + GPU-first](adr/0001-rust-native-gpu-first.md)
- [0002 模块化单体](adr/0002-modular-monolith.md)
- [0003 SoA + Arena + 分代句柄](adr/0003-soa-arena-storage.md)
- [0004 数据/渲染/编辑三模型分离](adr/0004-independent-data-render-editor-models.md)
- [0005 egui 与地图共享 wgpu 设备](adr/0005-wgpu-egui-shared-device.md)
- [0006 单写者编辑器 + 不可变快照](adr/0006-single-writer-editor.md)
- [0007 WASM 插件](adr/0007-wasm-plugins.md)
- [0008 Command + Transaction](adr/0008-command-transaction-undo.md)
- [0009 增量更新](adr/0009-incremental-update.md)
- [0010 MapCSS 兼容策略](adr/0010-mapcss-compatibility.md)
- [0011 依赖一致性与锁版本](adr/0011-dependency-coherence-pinning.md)
