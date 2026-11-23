# 发布流程（CI / Release / Pages）

## 版本号

版本唯一来源：根 `Cargo.toml` 的 `[workspace.package] version`（当前 `0.1.0`）。
遵循 SemVer；每次发布：

1. 更新 `Cargo.toml` 的 version（以及 `Cargo.lock` 中 workspace 包版本）
2. 更新 `CHANGELOG.md`（把 `Unreleased` 改为新版本 + 日期）
3. 提交：`chore(release): vX.Y.Z`
4. 打 tag 并推送：

   ```bash
   git tag -a vX.Y.Z -m "NEO vX.Y.Z"
   git push origin main --tags
   ```

## GitHub Actions 流水线

| Workflow | 触发 | 内容 |
|---|---|---|
| `.github/workflows/ci.yml` | push / PR | fmt、clippy、workspace 测试、i18n、release 构建、mdBook 构建 |
| `.github/workflows/release.yml` | tag `v*` | 校验 tag = Cargo version → 测试 → 构建 Windows release → 打包 zip + sha256 → GitHub Release |
| `.github/workflows/pages.yml` | push main（docs 变更）/ 手动 | mdBook 构建 `docs/` → GitHub Pages |

### Release 产物

- `neo-desktop-vX.Y.Z-windows-x86_64.zip`
  - `neo-desktop.exe`（GUI 子系统，无控制台）
  - `README.md`、`LICENSE`、`LICENSE-MIT`、`LICENSE-APACHE`、`NOTICE`、`CHANGELOG.md`
- `neo-desktop-vX.Y.Z-windows-x86_64.zip.sha256`

Release Notes 由 GitHub 自动生成（`generate_release_notes: true`），并在正文中说明校验方法。

### 版本校验

release workflow 会解析 `Cargo.toml` 的 workspace version，若 `v${version}` ≠ tag，
任务失败并提示先修正版本号——避免“tag 与代码版本不一致”的发布。

## GitHub Pages

网站由 `docs/` + `book.toml` 构建（mdBook）：

- 首页：`docs/introduction.md`
- 用户指南：`docs/guide/`
- 开发指南：`docs/dev/`
- 技术设计与 ADR：`docs/0*.md`、`docs/adr/`

首次启用：仓库 **Settings → Pages → Build and deployment → Source: GitHub Actions**，
之后每次 push 到 `main`（涉及 docs）自动部署到
`https://novolife.github.io/neo/`。

本地预览：

```bash
cargo install mdbook --version 0.4.52 --locked
mdbook serve --open
```

## 回滚

- 删除/标记 Release 为 pre-release；
- 重新打 patch tag（`vX.Y.Z+1`）并发布；
- 不要在已发布 tag 上强推重写。

## 本地打包（可选）

```powershell
cargo build --release -p neo-desktop
$tag = "v0.1.0"
$stage = "out/package-$tag"
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item out/cargo/release/neo-desktop.exe $stage
Copy-Item README.md,LICENSE,LICENSE-MIT,LICENSE-APACHE,NOTICE,CHANGELOG.md $stage
Compress-Archive "$stage/*" "neo-desktop-$tag-windows-x86_64.zip"
Get-FileHash "neo-desktop-$tag-windows-x86_64.zip" -Algorithm SHA256 > "neo-desktop-$tag-windows-x86_64.zip.sha256"
```
