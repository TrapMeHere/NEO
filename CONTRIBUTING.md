# Contributing

Thanks for contributing to NEO! NEO is a Rust + wgpu OSM editor aiming to be a complete replacement for JOSM.
Please read first:

- [Project introduction](docs/introduction.md) and [architecture](docs/01-system-architecture.md)
- [Developer guide](docs/dev/build-and-test.md) (build, test, toolchain)
- [Licensing and clean-room rules](docs/17-licensing-and-governance.md) (**important**)

## Code of Conduct

All participants must follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Getting started

```bash
git clone https://github.com/TrapMeHere/NEO.git
cd NEO
rustup show                 # uses rust-toolchain.toml (pinned 1.100.0 + rustfmt + clippy)
cargo build -p neo-desktop
```

The program, docs and tests all live on `master`. Run the workspace tests with:

```bash
cargo test --workspace
```

Requirements: stable Rust (MSRV 1.97), Windows 10/11 or Linux/macOS. Map rendering uses wgpu
(DX12 / Vulkan / Metal).

## Workflow

1. Open an issue describing the problem/proposal first; for larger changes write an **ADR** (`docs/adr/`) or RFC first.
2. Fork → branch from `master`: `feat/...`, `fix/...`, `docs/...`.
3. Commit in small steps; every commit on `master` should pass `cargo check` and `cargo test`.
4. Use Conventional Commits for commit messages, with an optional scope:

   ```
   feat(editor): add way split at node
   fix(render): keep LOD after zoom out
   docs: document release process
   ```

5. Commits must carry a **DCO sign-off** (this project's management model, see docs/17):

   ```bash
   git commit -s -m "feat(ui): ..."
   ```

   i.e. the commit trailer must contain `Signed-off-by: Name <email>`.

6. Open a PR and fill in the checklist from the template. CI on `master` runs fmt / clippy / check / tests / docs (including the mdBook build); make sure the PR's tests were added or updated in the same PR.

## Code conventions

- **Data mutations only go through `neo-editor` commands/transactions**; the GUI must never modify `DataSet` directly.
- Follow the dependency direction in [docs/02](docs/02-workspace-and-dependency-rules.md); new cross-layer dependencies require updating the docs and `scripts/check-deps.py` first.
- User-visible text must be added to `i18n/en-US.json` (the source language) and its translations in `i18n/zh-CN.json` / `i18n/zh-TW.json`; use `neo_i18n::t/tf` in code and run `python scripts/check-i18n.py`.
- Performance-related changes must include benchmark data (see docs/14); never invent numbers.
- Changes to rendering/picking rules (LOD, margins, read-only layers) must update both documentation and tests.

## Testing

On `master`:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo check --workspace
cargo test --workspace
python scripts/check-i18n.py
python scripts/check-deps.py
python scripts/check-write-api.py
```

Desktop smoke testing is manual: there is no automated UI-input script in this repository. Run
`cargo run -p neo-desktop` and verify drawing, selection, undo/redo and the main panels in the window.

## Documentation

- User tutorials/developer guides: `docs/`, published to GitHub Pages by mdBook.
- Architecture facts are authoritative in `docs/01`–`docs/18`; code comments reference doc sections.
- Update the docs in the same PR when changing interfaces or behavior.
- The repository's baseline language is **English**: English is the default UI language and the i18n source. Write documentation, code comments, commit messages and scripts in English. Chinese is provided as an i18n translation layer (`i18n/zh-CN.json`, `i18n/zh-TW.json`).

## PR checklist

- [ ] Linked issue / ADR
- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets` has no new warnings
- [ ] `cargo check --workspace` passes; corresponding tests have been added/updated and run on `master`
- [ ] New text added to `i18n/en-US.json` (source) and translations updated in `zh-CN`/`zh-TW`
- [ ] Docs/screenshots updated (when applicable)
- [ ] DCO sign-off (`git commit -s`)

## License

By submitting a contribution you agree to license code under `Apache-2.0 OR MIT` and documentation under CC-BY-SA-4.0.
Do not submit code or assets from JOSM or other GPL projects; see the clean-room rules in docs/17.
