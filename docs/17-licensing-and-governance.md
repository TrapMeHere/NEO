# 17 · Licensing and governance (Licensing & Governance)

**Abstract**: NEO's licensing decision is not a `LICENSE` file but **a combination of three things**: code licensing (which determines the plugin ecosystem and enterprise adoption), asset licensing (which determines whether we can legally obtain "user muscle memory" such as presets/styles/validator rules), and clean-room discipline (which determines whether we actually own the code we write). Conclusion: code uses the **`Apache-2.0 OR MIT` dual license**, assets use a **runtime-separate asset directory + per-file license + NOTICE** (re-obtained through the JOSM wiki's CC-BY-SA/LGPL route, not copied from the GPL repository), contributors use the **DCO** rather than a CLA, and governance transitions from BDFL to a maintainer committee with an RFC process. **This document is not legal advice**: all clause-level conclusions must be reviewed by a practicing lawyer before they can serve as a basis for release. JOSM-side licensing facts are in [`.research/josm-inventory.md`](../.research/josm-inventory.md) §1.2–1.3, and the parity goal is in [13](./13-josm-parity.md).

## 1 Goals and scope

**In scope**: licensing of NEO's own code and documentation, compliance for third-party assets and dependencies, the plugin license model, trademarks and naming, project governance and processes, supply chain (SBOM/audit).

**Out of scope**: determination of the license of OSM data itself (ODbL applicability is handled by the OSMF and each data user; NEO only does attribution and "does not redistribute user data"), infringement determination in specific jurisdictions, establishment of a commercial entity, and trademark registration applications.

Three unavoidable constraints:

| # | Constraint | Source | Consequence |
|---|---|---|---|
| C1 | JOSM is GPL-2.0-or-later, and compiled artifacts are presented as GPLv3 overall; plugins are not covered by that license | JOSM `LICENSE` | Any JOSM code entering NEO is contagious; functionality and behavior are not protected |
| C2 | Assets in the JOSM repository (presets XML, `elemstyles.mapcss`, 12 validator mapcss files, icons, `.lang`) **have no per-file declaration** and follow the repository's GPL | Repository tree + file-header check | "Assets are not code, so it doesn't matter" is the wrong intuition |
| C3 | Plugin licenses must be confirmed individually; some plugins have no license file (= all rights reserved) | Each plugin repository | Cannot treat "JOSM has 233 plugins" as a portable list |

## 2 Design

### 2.1 NEO's own license: decision matrix and conclusion

Scored 1–5 (5 is best), weighted sum (max 75). The basis for each score is written in the "Rationale" column, not "it feels better".

| Criterion (weight) | MIT OR Apache-2.0 | Apache-2.0 | MPL-2.0 | GPL-3.0-or-later |
|---|---|---|---|---|
| Compliance cost of isolating from GPL assets (3) | 4 (strict asset isolation required) | 4 | 4 | 5 (can inline JOSM assets directly) |
| Plugin ecosystem: can third parties be closed-source (3) | 5 | 5 | 3 (file-level copyleft; plugin boundary needs care) | 1 (plugins must be GPL-compatible) |
| Enterprise and commercial adoption (3) | 5 | 4 (some enterprises only accept MIT) | 4 | 1 |
| Patent grant and defense (2) | 5 (the Apache part includes an explicit patent grant) | 5 | 4 | 4 |
| Ecosystem convention and tooling (2) | 5 (Rust ecosystem default) | 4 | 2 | 2 |
| Future relicensing flexibility (2) | 5 (can unilaterally change to GPL-3.0) | 4 | 3 | 1 |
| **Weighted total** | **72** | 65 | 53 | 35 |

**Conclusion: code = `Apache-2.0 OR MIT`.** Three reasons:

1. **It makes "asset isolation" an executable engineering constraint rather than a legal prayer.** Because NEO itself cannot absorb GPL assets, any shortcut such as "just copy the JOSM file to save effort" is immediately infeasible legally — which is itself discipline. The cost is that we must govern assets as independent assets (§2.3), which is something we **should have done anyway**.
2. **The plugin ecosystem needs the host license to be non-contagious.** If the WASM plugin host itself were GPL, almost all commercial plugin authors would leave; an MIT/Apache host imposes zero constraints on plugin authors (plugins can be GPL or closed-source).
3. **Patent clause.** The Apache-2.0 part provides a contributor patent grant and termination clauses; pure MIT has no such protection. Dual licensing lets users choose, and `Apache-2.0 OR MIT` is the de facto consistent choice in the Rust ecosystem.

The only real advantage of the rejected `GPL-3.0-or-later` is that it **can legally inline JOSM assets and use JOSM's rule files as test corpora**, saving a lot of asset-governance work; but it also kills the plugin ecosystem and enterprise adoption, and it does not let us copy JOSM **code** (the license allows it, but the whole point of clean-room is to get an implementation that can evolve independently).

**Docs and specs**: `docs/` uses `CC-BY-SA-4.0` (consistent with the JOSM wiki, facilitating two-way text movement), and code snippets in docs are used under the code license.

### 2.2 Clean-room discipline

GPL covers **code expression**, not functionality and behavior. So "rewriting" is legal, but independence must be demonstrable. Four gates:

| Gate | Concrete practice | Checkable evidence |
|---|---|---|
| Specs first | Write the behavior spec first (this repo's `docs/`), then the implementation; specs reference only public docs, observable behavior, and user-visible descriptions | Every P0 module has a corresponding doc section |
| Non-consultation record | For each module, record in `docs/cleanroom-ledger.md`: behavior sources, whether JOSM source was consulted, who consulted, who reviewed, date | ledger coverage 100% (§7) |
| Personnel separation | **Anyone who has read JOSM source must not write the implementation of the corresponding module**, and may only do spec review or test cases | ledger's "consultant ≠ implementer" field |
| Self-check | On commit, confirm item by item against the "high-risk transcription checklist" (below the §2.2 table) | PR template checkbox |

**High-risk transcription checklist** (these are the things most easily "translated" rather than "rewritten"): `OrthogonalizeAction`'s iterative algorithm, `SimplifyWayAction`'s Douglas-Peucker variant and its parameters, `ReverseWayTagCorrector`'s tag swap table, `OsmApi`'s request assembly details, the `MapCSS` parser's error-recovery strategy, and the validator's original message wording. **Algorithms themselves are not protected by copyright, but "line-by-line rewrite + renaming variables" is a derivative work.**

> **This repository already has one real contamination source**: `.research/josm-inventory.md` explicitly records that it read source files such as `OsmApi.java`, `Command.java`, `OsmWriter.java`, and `UploadDialogModel.java`. Conclusion: **anyone involved in writing that document must not implement `neo-api`, the Command layer of `neo-editor`, or XML/OSC writing in `neo-io`**, and may only provide specs and test assertions. This is not formalism — it is the only evidence we can present if challenged.

### 2.3 Asset compliance: three routes and the asset directory

JOSM's assets are "user muscle memory"; parity must load them, but **they must not come from the GPL repository**. There are three legally distinct routes:

| Route | Content | Obligation | Adopted |
|---|---|---|---|
| A Copy from `openstreetmap/josm` or `josm-tested.jar` | Identical to JOSM | That file is GPL-2.0+ (compiled artifacts overall GPLv3) → the whole combined work must be GPL-compatible with all sources shipped | ❌ Conflicts with §2.1 |
| B Re-obtain **the same content** from the JOSM wiki (`TaggingPresets`, `Styles`, `Rules/*` pages, etc.) | The same XML/MapCSS text, under the wiki license | Footer: **CC-BY-SA + also LGPL after 2014-04**. Distributed as an independent data asset with attribution, license statement, and the file's own source, not statically merged into GPL-derived code | ✅ Main route |
| C Rewrite based on OSM tagging documentation | Our own files | Only need to comply with the CC-BY-SA of OSM wiki text (rewriting and facts are unconstrained) | ✅ Supplementary (especially icons and small rules) |

Engineering requirements for route B (missing any one falls back to the risk of route A):

```
assets/                          ← the only place where third-party content may live; not part of cargo build
├── MANIFEST.toml                ← per file: source URL, upstream version, SPDX, sha256, retrieval date, whether modified
├── NOTICE                       ← aggregated attribution (JOSM contributors, OSM contributors, font/icon authors)
├── LICENSES/
│   ├── LGPL-2.1-or-later.txt
│   ├── CC-BY-SA-4.0.txt
│   ├── OFL-1.1.txt
│   └── Apache-2.0.txt
├── presets/defaultpresets.xml   ← unmodified; if modified, mark derived in MANIFEST and license the whole file CC-BY-SA
├── styles/elemstyles.mapcss
├── validator/{addresses,combinations,deprecated,geometry,highway,multiple,
│              numeric,relation,religion,territories,unnecessary,wikipedia}.validator.mapcss
├── boundaries.osm               ← required by the territory selector; ODbL content, needs OSM attribution
├── icons/                       ← self-drawn or CC0/Apache-2.0/ISC set, annotated per file
└── fonts/                       ← OFL-1.1 (e.g. Noto Sans / JetBrains Mono)
```

Hard rules:

1. **Assets do not go into the binary**: assets are loaded from a runtime directory (`--assets-dir`, by default unpacked with the installer into the user data area). This way "independent assets" is not just a claim but a fact at the linkage boundary.
2. **Unmodified preferred**: any change creates a CC-BY-SA "adaptation", and that file must remain CC-BY-SA — acceptable, but the MANIFEST must explicitly mark `modified = true`; otherwise we would mistakenly think it is LGPL-only.
3. **Icons are a separate pitfall**: presets XML reference icon names via paths such as `icon="presets/..."`; **icon names are a data contract** (the reference relationship does not constitute copying the image), but **the image files themselves are not**. Approach: draw a JOSM-compatible icon set under the same names ourselves (or map a CC0/Apache-2.0 icon set to the same names), fall back to a generic icon when missing, and track coverage. 1385 SVG + 884 PNG cannot all be drawn at once; do the first 200 by reference frequency in P0 presets.
4. **ELI must not be assumed CC0**: GitHub reports `NOASSERTION` for the Editor Layer Index. Therefore **do not bundle ELI**; verify attribution and terms for built-in imagery entries one by one, and let users enable the rest themselves in preferences (accepting the terms themselves), off by default.
5. **NTv2 `.gsb` grids have separate terms of use** (JOSM itself includes a note in `nodist/data/projection/CHENyx06-Distribution.pdf`) — if projection parity is to include NTv2, it must be reviewed separately.
6. **`.lang` translations**: the contributor terms of the 48 files are unverified (> to verify); NEO initially uses only English + its own translation pipeline (Crowdin/Weblate), and does not copy translations from JOSM.
7. **REUSE compliance**: each asset file gets an `SPDX-FileCopyrightText`/`SPDX-License-Identifier` header or a same-named `.license` sidecar file, and CI runs `reuse lint`.

### 2.4 Plugin license model

WASM plugins and the host are **independent programs with clear boundaries** (no linking, no shared address space, only interaction through a documented ABI), so the host license (MIT/Apache) imposes no constraints on plugins; conversely, after a GPL plugin is **ported** to WASM it is still GPL, and one must not copy NEO's MIT code into it and then claim it is closed-source (MIT allows that, but the ported work as a whole is still GPL).

Plugin manifests must include the license and permissions:

```toml
[plugin]
id            = "org.example.buildingtools"
version       = "0.3.1"
license       = "GPL-3.0-or-later"     # SPDX expression; missing = refuse to load
source_url    = "https://…"            # corresponding source location (distribution obligation for GPL plugins)

[permissions]                           # deny all by default, request item by item
network       = ["api.openstreetmap.org"]   # domain allowlist, not wildcards
fs_scope      = ["$DATA/plugins/org.example.buildingtools"]
editor        = ["read", "write"]       # writes must go through Command, so they remain undoable
ui            = ["panel", "menu:data"]  # the Data menu is the plugin host, see 13 §3.1
limits        = { fuel = 50_000_000, memory_mb = 256, wall_ms = 2_000 }
```

Governance requirements:

| Requirement | Rationale |
|---|---|
| `license` is mandatory and must be an SPDX expression | An upstream plugin without a license file = all rights reserved, cannot be ported; we must confirm each one before porting |
| When distributing a GPL plugin its source must be provided | The GPL obligation travels with the plugin; the host does not fulfill it on its behalf; the plugin marketplace needs a source-link field |
| Least privilege + user-visible | Unrelated to licensing but part of "ecosystem trustworthiness"; also a technical prerequisite for the sandbox ([15](./15-plugin-system.md)) |
| No binary loading path for JOSM plugins | Technically infeasible (depends on Java internal types), and legally unnecessary |

### 2.5 Trademarks and naming

| Object | Rule | Concrete practice |
|---|---|---|
| "OpenStreetMap" / "OSM" | Registered/common-law trademarks of the OSMF; descriptive use is allowed, implying official endorsement is not | The primary product mark uses only **NEO**; the subtitle uses descriptive wording "an editor for OpenStreetMap data"; do **not** put "OSM" as the first word of the product name (avoid things like "OSM Editor NEO") |
| OSM logo | Must not be used | Do not use the OSM logo for the app icon or splash screen |
| JOSM name/logo | Its logo must not be used; the name may be used for **factual reference** | In docs write "behavior-aligned with JOSM r19678" with a disclaimer: "NEO is not affiliated with or endorsed by the JOSM project or the OpenStreetMap Foundation" |
| Data attribution | Required by ODbL | Always show "© OpenStreetMap contributors" in a map corner; exported images/files carry the attribution too; the "produced work" statement is in §3 |
| Domain/package names | No confusion with official projects | Do not use the `openstreetmap.*` or `josm.*` namespaces |

> To verify: the clause-level wording of the OSMF trademark policy and the permitted boundary of "based on OpenStreetMap" must be confirmed by a lawyer and the OSMF before finalizing the product name and promotional copy.

### 2.6 Governance

| Phase | Trigger | Structure | Decision-making |
|---|---|---|---|
| Phase 0 BDFL (current) | Project start | 1 owner + module maintainers | Owner decides, RFCs recorded |
| Phase 1 Committee | ≥3 external contributors or the first stable release | 3–5 person maintainer committee (including 1 release manager, 1 security officer) | Lazy consensus + voting; major changes require an RFC |
| Phase 2 Foundation-hosted (optional) | Commercial dependency or trademark dispute arises | Independent legal entity/foundation | Bylaws |

Process elements:

- **RFC**: `docs/rfc/NNNN-title.md`, template with motivation/spec/alternatives/compatibility impact/license impact; public comment ≥7 days, lazy consensus (passes without substantive objection); architecture-level decisions go to `docs/adr/` (this repo already has `docs/adr/`).
- **Release cadence**: nightly (every merge), **one stable per month** (aligned with JOSM's monthly tested cadence, facilitating parity comparison), one LTS per year (session/format compatibility promise).
- **Security response**: private GHSA channel + `security@` mailbox; disclosure 90 days after confirmation; `cargo-audit` for dependency vulnerabilities weekly; parser-crash classes handled at security severity (see the fuzz section in [18](./18-testing-strategy.md) §2.8).
- **Contributors**: DCO 1.1 (`git commit -s`) rather than a CLA — rationale in §5. The DCO cannot cover the need to "relicense in the future", so if a GPL dual-track is ever desired, a CLA must be introduced in advance and **all** past contributors must agree (this is the known cost of choosing the DCO).

## 3 Key matrices

### 3.1 SPDX overview (release artifacts × license)

| Artifact | SPDX | Location | Distribution | Notes |
|---|---|---|---|---|
| `crates/*`, `apps/*` | `Apache-2.0 OR MIT` | workspace | binary + source | Patent grant comes from the Apache branch |
| `docs/` | `CC-BY-SA-4.0` | repository | with repo | Facilitates two-way movement with the wiki |
| presets / styles / validator rules | `LGPL-2.1-or-later` (from the wiki, additionally CC-BY-SA-4.0) | `assets/` | separate asset directory | Loaded at runtime, not compiled |
| Territory boundaries `boundaries.osm` | `ODbL-1.0` | `assets/` | separate asset | Needs OSM attribution |
| Icons | `CC0-1.0` / `Apache-2.0` / `ISC` per file | `assets/icons/` | separate asset | Self-drawn or compatible set |
| Fonts | `OFL-1.1` | `assets/fonts/` | separate asset | Reserved font name clause; may not sell the fonts separately |
| Test fixtures (synthetic) | `CC0-1.0` | `tests/fixtures/` | with repo | Generator reproducible |
| Test fixtures (Monaco extract) | `ODbL-1.0` | not in git (download script) | download on demand | Needs attribution, see [18](./18-testing-strategy.md) §3.4 |
| Plugins (third-party) | their own SPDX | plugin marketplace | separate distribution | Mandatory in manifest |

### 3.2 Dependency license policy (supply chain)

| Policy | Tool | Gate |
|---|---|---|
| Allowlist | `cargo-deny` `[licenses]`: `MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, MPL-2.0, CC0-1.0` | CI fails on GPL/AGPL/SSPL (MPL needs per-crate review) |
| Vulnerabilities | `cargo-audit` (weekly + on every lockfile change) | Known exploitable vulnerabilities block release |
| SBOM | `cargo-sbom` / CycloneDX output attached to the release | Required for every stable release |
| Attribution | `cargo-about` generates `THIRD-PARTY.md` | Distributed with the release |

## 4 Process and sequence

### 4.1 Contributor compliance pipeline

```
PR submit (with Signed-off-by)
   │
   ├─ ① DCO check (one -s per commit, author and committer match)
   ├─ ② REUSE lint (every new file has an SPDX header; assets have .license sidecar files)
   ├─ ③ cargo-deny (license allowlist + vulnerabilities + duplicate versions)
   ├─ ④ clean-room declaration: mandatory in the PR template
   │     · Does this change reference JOSM source? → Yes: must be rewritten by someone who has not consulted it, or turned into a spec
   │     · Is it AI-assisted? → Yes: fill in the `Assisted-by:` trailer (model/purpose)
   │     · New assets? → attach a MANIFEST entry (source URL + SPDX + sha256)
   └─ ⑤ human review (module maintainer) + ledger update
   ▼
  merge (squash preserves Signed-off-by)
```

### 4.2 Release compliance check (required for every stable)

```
[ ] reuse lint 100%              [ ] cargo-deny passes (including advisories)
[ ] SBOM + THIRD-PARTY.md generated    [ ] assets/MANIFEST.toml has no missing entries
[ ] The installer contains no third-party files outside assets/
[ ] Binary symbol scan: no GPL declarations (cross-checked against the SBOM)
[ ] cleanroom-ledger covers all P0 modules
[ ] Trademark usage review (app name/icon/About page/store listing copy)
[ ] Spot-check map attribution and exported-file attribution
```

## 5 Trade-offs and alternatives

| Decision | Choice | Rejected | Rationale / cost |
|---|---|---|---|
| Code license | `Apache-2.0 OR MIT` | GPL-3.0 (can inline JOSM assets, saving all asset governance) | Saved effort < lost plugin ecosystem and enterprise adoption; cost is that asset governance becomes a long-term cost |
| Asset source | wiki re-acquisition + runtime directory | copy from the GPL repository | The latter makes NEO a GPL derivative and directly kills the dual-license option |
| Contributor agreement | DCO | CLA | DCO is zero-friction and easily accepted by individual contributors; cost is that **the license cannot be changed unilaterally**, so a future GPL dual-track would require re-authorization from everyone |
| Icons | self-drawn/compatible set + name mapping | reuse JOSM icons | Reuse requires GPL; cost is the redraw effort for the first 200 icons and visual differences |
| AI policy | allow boilerplate and test scaffolding, forbid core algorithms | fully allow / fully forbid | Fully allowing has licensing and copyrightability risks; fully forbidding is unrealistic. Cost is needing human judgment on boundaries and trailer records |
| Translation | own pipeline | reuse the 48 `.lang` files | Upstream translation contributor terms are unverified; cost is low localization coverage initially |

## 6 Risks

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| L1 | The "same content" argument is challenged (wiki version and repository version deemed substantively identical but under different licenses) | Forced to go GPL or take assets down | Only obtain from wiki pages, record URL+date+sha256; never switch to repository files; keep a lawyer-reviewed record |
| L2 | Wiki version lags the JOSM release → parity assertions fail | Misjudged as bugs | MANIFEST records upstream version; during comparison also compare JOSM's actual loaded result against the wiki version differences ([18](./18-testing-strategy.md) §2.4) |
| L3 | AI-assisted code reproduces JOSM fragments | Contagion risk + cannot claim copyright | Forbid AI generation for core algorithms; `Assisted-by:` trailer; rewrite suspicious fragments |
| L4 | Contributor contamination (someone who read JOSM source wrote the corresponding implementation) | The independence of the whole module can be questioned | ledger + personnel separation + code review; readers of this repo's inventory are explicitly excluded from writing `neo-api`/`Command`/`neo-io` modules |
| L5 | Trademark complaints (product name implying OSM/JOSM) | Forced rename | Primary mark NEO; copy pre-reviewed by a lawyer and the OSMF; disclaimers on store listings |
| L6 | Plugin license minefield (upstream has no LICENSE = all rights reserved) | Porting is infringement | License confirmation before starting each family; mandatory `license` validation |
| L7 | ELI / NTv2 / font terms | Bundling violates them | Do not bundle ELI; review NTv2 separately; only use OFL/Apache fonts |
| L8 | Dependencies introduce GPL (transitive dependencies) | Contagion | `cargo-deny` allowlist + audit on every lockfile change |

## 7 Acceptance criteria

1. `reuse lint` passes 100% on a clean working tree; every file under `assets/` has exactly one record in `MANIFEST.toml` (count equality assertion).
2. The `cargo-deny` license check passes for all workspace crates and dependencies, with no GPL/AGPL/SSPL in the output.
3. The SBOM of released binaries/installers contains no GPL components; the number of third-party files outside `assets/` is 0 (script assertion).
4. `docs/cleanroom-ledger.md` covers all P0 modules (the 52 items in [13](./13-josm-parity.md) §2.4), each with "consultant / implementer / reviewer", and consultant ≠ implementer.
5. Among the last 50 merged commits, Signed-off-by coverage is 100%, and all AI-assisted commits carry `Assisted-by:`.
6. The app name, icon, About page, and store copy pass the trademark review checklist; maps and exported files all carry OSM attribution.
7. When any plugin manifest lacks `license` or requests an undeclared permission, loading is refused (negative tests pass).

## 8 References

- JOSM `LICENSE` (GPL-2.0-or-later, including the Apache-2.0 mixing and plugin exemption wording): https://raw.githubusercontent.com/JOSM/josm/master/LICENSE
- JOSM wiki footer license statement (CC-BY-SA + LGPL since 2014-04): https://josm.openstreetmap.de/wiki/TaggingPresets
- JOSM rule set distribution (`Rules/*` wiki route): https://josm.openstreetmap.de/wiki/Rules
- GPL-2.0 full text: https://www.gnu.org/licenses/old-licenses/gpl-2.0.html
- Apache-2.0 full text: https://www.apache.org/licenses/LICENSE-2.0
- MIT full text (SPDX text): https://spdx.org/licenses/MIT.html
- CC-BY-SA 4.0: https://creativecommons.org/licenses/by-sa/4.0/legalcode
- LGPL-2.1: https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html
- SIL OFL 1.1 (fonts, with reserved font name clause): https://openfontlicense.org/
- ODbL 1.0 and OSM attribution guidelines: https://www.openstreetmap.org/copyright
- OSMF trademark policy: https://wiki.osmfoundation.org/wiki/Trademark_Policy
  > To verify: clause-level wording and the permitted boundary of "based on OpenStreetMap" must be confirmed by a lawyer/OSMF.
- DCO 1.1: https://developercertificate.org/
- REUSE 3.x spec: https://reuse.software/spec/
- `cargo-deny`: https://embarkstudios.github.io/cargo-deny/
- `cargo-audit`: https://rustsec.org/
- First-hand licensing facts (asset inventory, license status of icons/fonts/ELI/NTv2): [`.research/josm-inventory.md`](../.research/josm-inventory.md) §1.2–1.3
