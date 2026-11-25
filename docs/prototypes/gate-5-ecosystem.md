# Gate-5 Report: Full Ecosystem

- **Gate id**: G5 (docs/16 §2.6 / §2.8)
- **Evaluation date**: 2026-11-16
- **Code state**: worktree uncommitted, baseline commit `add2418`
- **Commands run this round**: `cargo test -p neo-plugin --features wasm-plugins --offline`; `cargo test -p neo-cli --features plugins --offline`; `cargo check -p neo-desktop -p neo-ui --offline`; `neo-cli presets`; `neo-cli --features plugins plugins`; `neo-cli open --synth 1000000`

## 0 Environment

| Item | Value |
|---|---|
| OS | Windows 11 10.0.26100 |
| Rust | 1.100.0 stable. The workspace `rust-version` is still 1.95 |
| wasmtime | 50.0.1 (cranelift 0.137.1). Both packages declare rustc ≥ 1.97 |
| Build | dev profile (optimized + debuginfo). No release installer was built |
| feature | `wasm-plugins` is off by default. The desktop binary does not link wasmtime |

The default build does not compile wasmtime, so the default MSRV 1.95 check is unaffected. Enabling `wasm-plugins` / `neo-cli`'s `plugins` requires rustc ≥ 1.97. The workspace's `rust-version` was not changed to 1.97 because of this.

Signing is the SHA-256 of the manifest body and entry bytes, not the Ed25519 from ADR-0007. An empty signature is only allowed under `developer_mode` and is marked `unsigned_dev`.

## 1 G5 criteria

| # | Criterion | Measured | Verdict |
|---|---|---|---|
| (a) | At least 3 JOSM plugin semantics ported to WASM and usable | `todo` (writes panel state on selection changes), `utilsplugin` (`add-node`), `buildings_tools` (rectangular `add-way` with `building=yes`). `three_josm_plugins_submit_and_apply` passes; commands are written via `Editor::execute`, and buildings can be undone. The CLI `plugins` prints the three plugins, the panel `open tasks`, and two commands | ✅ |
| (b) | Plugins cannot exceed their authority | Unauthorized commands are not queued; `../secret`, absolute paths, and backslashes are rejected before reading files; HTTP only accepts HTTPS with hosts in the allowlist. With only `tiles.example` granted, the probe module reports command count 0, `http_hits = 1`, and no `evil` in the log. File reads normalize the path first, confirm it is still under the plugin root, then `read` | ✅ |
| (c) | Timeouts or crashes do not affect the host | An infinite-loop plugin that exhausts fuel is disabled; in the same dispatch round todo still writes `open tasks`. When `memory.grow` exceeds 64 MiB the growth fails and linear memory stays at 65536 bytes | ✅ |
| (d) | Three-platform installers cold-start and open a 1M data set | The repository has a Windows zip script, an Inno script, a Linux tarball + `.desktop`, and a macOS `.app` skeleton. These scripts were not executed this round, there was no cross-compilation, and no installer cold start. The same import function opened synthetic 1M once on the local CLI: `nodes=1000000 ways=249750 relations=7813`, `neo-cli.exe` took **302 ms** (dev, single run, not a median of 5, and not the desktop installer) | ❌ Not executed |
| (e) | Presets compared against JOSM default presets, with gaps recorded | The group prefix table is in [presets-coverage.md](./presets-coverage.md). All 22 group counts are greater than 0. No item-by-item comparison against `defaultpresets.xml` was done | ⚠️ Group granularity passes, item-by-item comparison not done |

The three plugins are repository-generated WAT; their import module is only `neo`, and they do not link WASI. They replicate the three behaviors above, not JOSM plugin binaries.

The desktop "Tools → Plugins, Imagery and Presets" window lists these three names, one TMS URL, and the gaps in the coverage table. The window itself does not instantiate wasmtime.

## 2 Conclusion

**G5 = Conditional Go**

Plugin isolation (a)(b)(c) passes in wasmtime unit tests. The group coverage table has been written, and all 22 listed groups have entries.

Parts that cannot be recorded as Go: (d) three-platform installers were not built and not cold-started; (e) there is no item-by-item list against JOSM default presets. MapCSS gained `canvas` / `dashes` and warning records for `text` / `icon`, but there is still a gap to the full MapCSS set.

Risks: R7 was blocked for this round's probe by fuel, memory caps, and capability intersection. R4 keeps not copying JOSM XML. R3 only reaches group prefixes. R5 this round is Windows only.
