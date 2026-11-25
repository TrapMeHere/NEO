# Release Process (CI / Release / Pages)

## Version number

Single source of truth for the version: the root `Cargo.toml`'s `[workspace.package] version` (currently `0.1.0`).
Follow SemVer; for each release:

1. Update the version in `Cargo.toml` (and the workspace package versions in `Cargo.lock`)
2. Update `CHANGELOG.md` (change `Unreleased` to the new version + date)
3. Commit: `chore(release): vX.Y.Z`
4. Tag and push:

   ```bash
   git tag -a vX.Y.Z -m "NEO vX.Y.Z"
   git push origin master --tags
   ```

## GitHub Actions pipelines

| Workflow | Trigger | Contents |
|---|---|---|
| `.github/workflows/ci.yml` | push / PR | fmt, clippy, workspace tests, i18n, release build, mdBook build |
| `.github/workflows/release.yml` | tag `v*` | verify tag = Cargo version → test → build Windows release → package zip + sha256 → GitHub Release |
| `.github/workflows/pages.yml` | push to master (docs / theme / book.toml changes) / manual | build mdBook from `docs/` → GitHub Pages |

### Release artifacts

- `neo-desktop-vX.Y.Z-windows-x86_64.zip`
  - `neo-desktop.exe` (GUI subsystem, no console)
  - `README.md`, `LICENSE`, `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`, `CHANGELOG.md`
- `neo-desktop-vX.Y.Z-windows-x86_64.zip.sha256`

Release Notes are generated automatically by GitHub (`generate_release_notes: true`), and the body explains how to verify the checksum.

### Version verification

The release workflow parses the workspace version from `Cargo.toml`; if `v${version}` ≠ the tag,
the job fails and prompts you to fix the version number first—avoiding releases where "the tag and the code version disagree".

## GitHub Pages

The site is built from `docs/` + `book.toml` (mdBook), with `site-url = "/NEO/"` corresponding to
`https://trapmehere.github.io/NEO/`:

- Home: `docs/introduction.md`
- User guide: `docs/guide/`
- Developer guide: `docs/dev/`
- Technical design and ADRs: `docs/0*.md`, `docs/adr/`

**First-time enablement (must be done once)**: repository **Settings → Pages → Build and deployment →
Source: GitHub Actions**. Before it is enabled, the `Setup Pages` step fails with `404 Not Found`
and turns the workflow red—this is intentional: deployment is no longer silently skipped with `continue-on-error`.

> The `enablement: true` option of `actions/configure-pages` can create the site automatically, but it requires
> a token that is **not `GITHUB_TOKEN`** (a PAT or GitHub App), so it is not used by default.

Once enabled, every push to `master` (touching docs / theme / book.toml) automatically deploys to
`https://trapmehere.github.io/NEO/`.

Local preview:

```bash
cargo install mdbook --version 0.4.52 --locked
mdbook serve --open
```

## Rollback

- Delete the Release or mark it as pre-release;
- Re-tag with a patch tag (`vX.Y.Z+1`) and release;
- Do not force-push to rewrite an already published tag.

## Local packaging (optional)

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
