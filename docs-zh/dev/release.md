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
   git push origin master --tags
   ```

## GitHub Actions 流水线

| Workflow | 触发 | 内容 |
|---|---|---|
| `.github/workflows/ci.yml` | push / PR | fmt、clippy、workspace 测试、i18n、release 构建、mdBook 构建（英文 + 中文） |
| `.github/workflows/release.yml` | tag `v*` | 校验 tag = Cargo version → 测试 → 构建 Windows release → 打包 zip + sha256 → GitHub Release |
| `.github/workflows/pages.yml` | push master（docs / docs-zh / site / theme / book.toml 变更）/ 手动 | 构建中英双语 mdBook（`docs/` → `/en/`，`docs-zh/` → `/zh/`）→ GitHub Pages |

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

网站由 `docs/` + `book.toml`（英文）与 `docs-zh/` + `docs-zh/book.toml`（中文）构建（mdBook），
`site-url = "/NEO/"` 对应 `https://trapmehere.github.io/NEO/`：

- 英文：`/en/`（源 `docs/`）
- 中文：`/zh/`（源 `docs-zh/`）
- 语言入口页：`site/index.html`（按浏览器语言跳转）
- 固定右上角语言切换器：`theme/lang-switch.js` + `theme/lang-switch.css`

**首次启用（必须一次性完成）**：仓库 **Settings → Pages → Build and deployment →
Source: GitHub Actions**。未启用前 `Setup Pages` 步骤会以 `404 Not Found`
失败并使 workflow 变红——这是刻意的：不再用 `continue-on-error` 静默跳过部署。

> `actions/configure-pages` 的 `enablement: true` 可以自动建站，但它要求传入
> **非 `GITHUB_TOKEN`** 的 token（PAT 或 GitHub App），因此默认不使用。

启用后每次 push 到 `master`（涉及 docs / theme / book.toml）自动部署到
`https://trapmehere.github.io/NEO/`。

本地预览：

```bash
cargo install mdbook --version 0.4.52 --locked
mdbook build            # 英文 -> book/en
mdbook build docs-zh    # 中文 -> book/zh
cp site/index.html book/index.html
# 本地预览：python -m http.server -d book 8000  ->  http://localhost:8000/en/（或 /zh/）
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
