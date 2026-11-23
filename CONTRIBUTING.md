# 贡献指南（Contributing）

感谢你愿意为 NEO 做贡献！NEO 是 Rust + wgpu 的 OSM 编辑器，目标是完整替代 JOSM。
请先阅读：

- [项目介绍](docs/introduction.md) 与 [架构](docs/01-system-architecture.md)
- [开发指南](docs/dev/build-and-test.md)（构建、测试、工具链）
- [许可与清洁室纪律](docs/17-licensing-and-governance.md)（**重要**）

## 行为准则

所有参与者需遵守 [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md)。

## 开始开发

```bash
git clone https://github.com/TrapMeHere/NEO.git
cd NEO
rustup show                 # 使用 rust-toolchain.toml（stable + rustfmt + clippy）
cargo build -p neo-desktop
```

程序和文档在 `master`。测试源码在 `test` 分支，把 `master` 合并进去之后再跑：

```bash
git checkout test
git merge master
cargo test --workspace
```

环境要求：Rust stable（MSRV 1.95）、Windows 10/11 或 Linux/macOS。地图渲染使用 wgpu
（DX12 / Vulkan / Metal）。

## 工作流

1. 先开 Issue 描述问题/提案；较大的改动请先写 **ADR**（`docs/adr/`）或 RFC。
2. Fork → 从 `master` 建分支：`feat/...`、`fix/...`、`docs/...`。不要把 `test` 合并回 `master`。
3. 小步提交；每个提交在 `master` 上应能 `cargo check`。测试改动只提交到 `test`。
4. 提交信息使用 Conventional Commits 风格，可选 scope：

   ```
   feat(editor): add way split at node
   fix(render): keep LOD after zoom out
   docs: document release process
   ```

5. 提交需带 **DCO 签名**（本项目管理方式，见 docs/17）：

   ```bash
   git commit -s -m "feat(ui): ..."
   ```

   即提交末尾包含 `Signed-off-by: Name <email>`。

6. 开 PR，填写模板中的检查项。`master` 的 CI 是 fmt / clippy / check / docs。功能合并进 `master` 之后，再把 `master` 合并进 `test` 并跑测试。

## 代码约定

- **数据修改只走 `neo-editor` 的命令/事务**；GUI 不得直接改 `DataSet`。
- 依赖方向遵循 [docs/02](docs/02-workspace-and-dependency-rules.md)，新增跨层依赖需先改文档与
  `scripts/check-deps.py`。
- 用户可见文本必须放进 `i18n/zh-CN.json` 与 `i18n/en-US.json`，代码用 `neo_i18n::t/tf`；
  运行 `python scripts/check-i18n.py`。
- 性能相关改动请附 benchmark 数据（见 docs/14），不编造数字。
- 渲染/拾取规则（LOD、外边距、图层只读）改动必须同时更新文档与测试。

## 测试

`master`：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo check --workspace
python scripts/check-i18n.py
python scripts/check-deps.py
python scripts/check-write-api.py
```

`test` 分支（先合并 `master`）：

```bash
cargo test --workspace
```

Windows 上的 UI 冒烟脚本在 `test` 分支（需要桌面会话）：

```powershell
powershell -ExecutionPolicy Bypass -File scripts/ui_check.ps1 start
powershell -ExecutionPolicy Bypass -File scripts/ui_check.ps1 selectnode
```

## 文档

- 用户教程/开发指南：`docs/`，由 mdBook 发布到 GitHub Pages。
- 架构事实以 `docs/01`–`docs/18` 为准；代码注释引用文档章节。
- 改接口/行为时同一 PR 内更新文档。

## PR 检查清单

- [ ] 关联 Issue / ADR
- [ ] `cargo fmt --all --check` 通过
- [ ] `cargo clippy --workspace --all-targets` 无新增警告
- [ ] `cargo check --workspace` 通过；对应测试已在 `test` 分支更新并跑过
- [ ] 新增文本已加入 zh-CN/en-US
- [ ] 文档/截图已更新（如适用）
- [ ] DCO 签名（`git commit -s`）

## 许可

提交即表示你同意以 `Apache-2.0 OR MIT` 许可贡献代码，并同意文档以 CC-BY-SA-4.0 发布。
请勿提交来自 JOSM 或其他 GPL 项目的代码/资源；参见 docs/17 的清洁室规则。
