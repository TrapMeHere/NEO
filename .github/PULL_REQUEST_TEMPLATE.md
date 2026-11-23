## Summary / 概述

<!-- 这个 PR 做了什么？关联 Issue / ADR： -->

Closes #

## Type / 类型

- [ ] feat: 新功能
- [ ] fix: 缺陷修复
- [ ] perf: 性能
- [ ] refactor: 重构（无行为变化）
- [ ] docs: 文档
- [ ] chore: 构建/CI/依赖

## Test plan / 验证

<!-- 命令、截图、性能数据；UI 改动请附 out/ui-check 截图 -->

```bash
cargo check --workspace
python scripts/check-i18n.py
# 测试在 test 分支：git checkout test && git merge master && cargo test --workspace
```

## Checklist

- [ ] 只通过 `neo-editor` 命令修改数据（UI 未直接写 `DataSet`）
- [ ] 新增/修改的用户文本已加入 `i18n/zh-CN.json` 与 `en-US.json`
- [ ] `cargo fmt --all --check` 通过
- [ ] `cargo clippy --workspace --all-targets` 无新增警告
- [ ] 接口/行为变化已同步文档（`docs/`）
- [ ] 提交包含 DCO 签名（`git commit -s`）
- [ ] 未引入 JOSM/GPL 代码或资源（见 `docs/17`）
