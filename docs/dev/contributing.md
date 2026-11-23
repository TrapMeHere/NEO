# 贡献流程

完整规则见仓库根目录 [CONTRIBUTING.md](https://github.com/novolife/neo/blob/main/CONTRIBUTING.md)。

## 快速流程

1. 阅读 [架构](architecture.md) 与 [许可/清洁室](../17-licensing-and-governance.md)
2. Issue 讨论 → 较大改动先写 ADR（`docs/adr/`）
3. Fork / 分支 `feat|fix|docs/...`
4. 提交（Conventional Commits + DCO：`git commit -s`）
5. 本地检查：

   ```bash
   cargo fmt --all --check
   cargo clippy --workspace --all-targets
   cargo check --workspace
   python scripts/check-i18n.py
   # 测试在 test 分支：git merge master 之后 cargo test --workspace
   ```

6. 开 PR，按模板勾选检查项；CI 必须全绿

## 代码要求

- 只经 `neo-editor` 命令修改数据；UI 只发 `UiActions`
- 用户文本进 `i18n/`
- 性能结论必须有实测（docs/14 的格式）
- 渲染/拾取/LOD/只读规则的改动必须带测试与文档更新
- 不提交 JOSM 源码/资源；遵守 clean-room 规则（docs/17）

## Review 关注点

| 维度 | 检查 |
|---|---|
| 正确性 | 边界（空数据、删除引用、取消导入、恢复失败） |
| 性能 | 是否引入每帧重建/全量遍历/O(n²) |
| 可测试性 | 命令可单测、拾取可用合成相机测试 |
| 兼容性 | 文件格式、会话、journal 向后兼容 |
| 文档 | 接口/行为变更是否同步 docs |
