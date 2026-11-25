## Summary

<!-- What does this PR do? Linked issues / ADRs: -->

Closes #

## Type

- [ ] feat: new feature
- [ ] fix: bug fix
- [ ] perf: performance
- [ ] refactor: refactor (no behavior change)
- [ ] docs: documentation
- [ ] chore: build/CI/dependencies

## Test plan

<!-- Commands, screenshots, performance numbers; attach out/ui-check screenshots for UI changes -->

```bash
cargo check --workspace
python scripts/check-i18n.py
# Tests run on the test branch: git checkout test && git merge master && cargo test --workspace
```

## Checklist

- [ ] Data is only modified through `neo-editor` commands (the UI never writes `DataSet` directly)
- [ ] New/changed user-facing text is added to `i18n/zh-CN.json` and `en-US.json`
- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets` adds no new warnings
- [ ] API/behavior changes are reflected in the docs (`docs/`)
- [ ] Commits carry the DCO sign-off (`git commit -s`)
- [ ] No JOSM/GPL code or assets are introduced (see `docs/17`)
