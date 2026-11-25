# ADR-0011: Dependency coherence and version pinning strategy

- **Status**: accepted
- **Date**: 2026-11-15
- **Related**: docs/02 §2.9, ADR-0005

## Background

In November 2026, the crate ecosystem had two **compile-time irreconcilable** version conflicts that blow up under the default "pick the latest version" strategy:

1. `egui_dock 0.21.1` and `egui-phosphor 0.14.0` both declare `egui ^0.36`, while `egui 0.37.0` was released after them; if the GUI stack is upgraded to 0.37, Cargo links two copies of egui, and `DockState`/`TabItem`/`Ui` types mismatch across the boundary.
2. `geo 0.33.1` binds `rstar 0.12` through geo-types' `rstar_0_12` feature; if `rstar 0.14` is also used, geo's geometry types do not satisfy 0.14's `RTreeObject`, and default features also pull in a second triangulation implementation, `earcut 0.4.5`.

The characteristic of such problems: **it is not an API change but mutually exclusive semver ranges**; there is no way to discover it before `cargo build`.

## Decision

1. **Pin the GUI stack to `egui/eframe/egui-wgpu 0.36.2`**, keep wgpu at **30.0.2** (egui-wgpu 0.36.2 still requires `wgpu ^30`, so no GPU-side rollback is needed). `egui_dock 0.21.1` and `egui-phosphor 0.14.0` can therefore coexist.
2. **Use `rstar 0.14` directly for the spatial index**, implementing `RTreeObject` for NEO's own type (`SpatialEntry`); **do not enable** geo's rstar interop. If `geo` must be introduced, it must be `default-features = false` with explicitly enabled required features (avoiding a second earcut).
3. **Single source of truth for versions**: every external version is declared exactly once in the root `Cargo.toml`'s `[workspace.dependencies]`, and member crates use `{ workspace = true }`.
4. **Lock file in the repository**: `Cargo.lock` is committed to ensure consistent resolution on all three platforms/CI.
5. **CI assertions**:
   - Fail if `cargo tree -d` contains duplicate major versions of `egui`/`wgpu`/`rstar`;
   - `cargo metadata` validates that all `egui*`/`wgpu*` members fall on the pinned minor version;
   - MSRV is declared by `rust-version = "1.95"` (`egui_dock 0.21.1`'s MSRV is exactly 1.95).
6. **Upgrade rehearsal**: each GUI/GPU dependency upgrade may only change the two adaptation layers `apps/neo-desktop` and `neo-render`; `neo-ui`/`core` etc. must not be aware of versions (the contact-surface test of ADR-0004).

## Alternatives

| Option | Why rejected |
|---|---|
| Use egui 0.37 + self-built docking | Lose the mature dock implementation, and phosphor icons would need hand-writing too; the only gain is "a newer version" |
| Vendor/fork egui_dock to 0.37 | High long-term maintenance cost, out of sync with upstream fixes; just wait for the upstream release |
| Pin rstar 0.12 to accommodate geo | Give up 0.14's index improvements; NEO's index does not need geo types anyway |
| No version locking, random resolution | Different results on three platforms, CI not reproducible |

## Consequences

- New egui 0.37 features cannot be used in the short term (there are few places that would need workarounds in the UI layer).
- Once `egui_dock`/`egui-phosphor` are upgraded to versions supporting 0.37, the whole stack can be upgraded in one go (ADR verification item 6).
- If interop between `rstar` and `geo` is needed, write a 6-line newtype adapter (`#[repr(transparent)]` + forwarding `envelope()`); global version switching is not allowed.

## Verification

1. `cargo tree -d | grep -E '^(egui|wgpu|rstar|geo) '` shows no duplicate major versions.
2. `cargo check --workspace` passes (verified locally on Windows/MSVC with 1.100.0).
3. Upgrade rehearsal records are written to the `docs/14` appendix (one per dependency upgrade).
