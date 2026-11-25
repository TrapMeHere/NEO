# Contribution Process

For the complete rules see [CONTRIBUTING.md](https://github.com/TrapMeHere/NEO/blob/master/CONTRIBUTING.md) at the repository root.

## Quick process

1. Read [Architecture](architecture.md) and [Licensing/Clean Room](../17-licensing-and-governance.md)
2. Discuss in an issue → write an ADR first for larger changes (`docs/adr/`)
3. Fork / branch `feat|fix|docs/...`
4. Commit (Conventional Commits + DCO: `git commit -s`)
5. Local checks:

   ```bash
   cargo fmt --all --check
   cargo clippy --workspace --all-targets
   cargo check --workspace
   python scripts/check-i18n.py
   # tests run on master: cargo test --workspace
   ```

6. Open a PR and tick the checklist in the template; CI must be all green

## Code requirements

- Modify data only through `neo-editor` commands; the UI emits only `UiActions`
- User-facing text goes into `i18n/`
- Performance conclusions must be backed by measurements (format in docs/14)
- Changes to rendering/picking/LOD/read-only rules must come with tests and documentation updates
- Do not commit JOSM source code/assets; follow the clean-room rules (docs/17)

## Review focus

| Dimension | Check |
|---|---|
| Correctness | Edge cases (empty data, deleting references, cancelling an import, recovery failure) |
| Performance | Does it introduce per-frame rebuilds/full traversals/O(n²)? |
| Testability | Commands unit-testable, picking testable with a synthetic camera |
| Compatibility | File formats, sessions, journal backward compatibility |
| Documentation | Are interface/behavior changes reflected in docs? |
