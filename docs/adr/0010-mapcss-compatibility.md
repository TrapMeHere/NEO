# ADR-0010: MapCSS compatibility strategy (do not invent a new style format)

- **Status**: Accepted
- **Date**: 2026-11-15
- **Decision scope**: `neo-style` (parser, cascade, asset loading), `neo-validate` (rule consumption), `neo-ui` (style/preset panels)
- **Related**: [11-style-presets-mapcss.md](../11-style-presets-mapcss.md) | [12-validation.md](../12-validation.md) | [13-josm-parity.md](../13-josm-parity.md) | [17-licensing-and-governance.md](../17-licensing-and-governance.md)

## Background

JOSM uses **the same extended MapCSS 0.2** to drive three subsystems: map styles (`elemstyles.mapcss`), validator rules (12 `.validator.mapcss`), and the search box (MapCSS selector mode). Add to that `defaultpresets.xml` (660 KB) and `boundaries.osm`, and these files are "the user's muscle memory" itself.

Three possible directions:

1. **Design NEO's own style format** (TOML/JSON DSL), converting JOSM assets offline;
2. **Fully implement the MapCSS spec** (including `line: image`, `shield:`, Kothic `extrude`, etc.);
3. **Implement the subset JOSM actually uses + a deviation list**, and make unmodified official assets load directly.

Problems with direction 1: community styles and community validator rules (the `Rules/*` pages on the wiki) would immediately all break, and "community-writable declarative rules" are a key asset of the JOSM ecosystem; a converter would also turn "rendering differences" into "converter bugs", making pixel-by-pixel comparison impossible. Problems with direction 2: the parts of the spec we never intend to implement (along-line textures, shields, extrusion) would become permanent half-finished code, and JOSM itself does not support some of them either.

Another hard constraint is licensing: these assets are GPL-2.0+ (some wiki mirrors are CC-BY-SA + LGPL), and they must be **distributed separately and loaded separately** from NEO code, otherwise the whole binary gets pulled into GPL.

## Decision

**Adopt direction 3**, with the following boundaries made explicit:

| # | Decision | Notes |
|---|---|---|
| D1 | One parser, three `StyleTarget`s (`Mappaint` / `Validator` / `Search`) | Selector evaluation is shared; the property set is trimmed per target and reports "not supported by this target" |
| D2 | **Copy JOSM's cascade semantics**: rules override earlier ones in file order, `::layer` is layered then composed, `::*` expands to all layers; **do not introduce CSS specificity** | `elemstyles` results depend on file order; adding specificity = rendering distortion |
| D3 | Support the full set of selectors and operators JOSM actually uses: `area`/`closedway`, `> <` child-parent selectors, `>[role=]/[index=]`, `\|z13-15`, `::layer`, `@supports`, `inside()/outside()`, `[k=*k2]` dereference, `∈ ⊆ ⊇ ⊈ ⊉ ⧉` | Item by item see [11](../11-style-presets-mapcss.md) §2.3–2.5 |
| D4 | Explicitly unsupported with **compile-time degradation**: `line: image`, `shield:`, `font-variant`/`text-decoration`/`text-transform`/`max-width` under `label:`, Kothic `extrude`, `:area-style` | Not silently dropped: goes into `unsupported()` and is shown to the user, and registered in `docs/known-deviations.md` |
| D5 | `defaultpresets.xml` loads with zero modification; `<chunk>/<reference>` are expanded at load time, and `match` expressions are precompiled | `tagging-preset.xsd` is the single source of truth; 660 KB cannot be parsed per object |
| D6 | All JOSM assets are **loaded at runtime, not compiled into the binary** (`include_str!` forbidden), and the asset directory is separate with a `manifest.json` (source/version/sha256/license) | GPL/CC-BY-SA isolation; hot reload; user-replaceable |
| D7 | The imagery catalog consumes ELI and is **not bundled**; attribution/terms are shown per entry (ELI licensing marked NOASSERTION) | Do not assume CC0, do not do "select all with one click" |
| D8 | Deviations are managed as a list and are not silently changed by "fixing bugs" | Each deviation must have a reason and comparison evidence |

## Alternatives (evaluated and rejected)

| Option | Pros | Why rejected |
|---|---|---|
| Own style format (TOML/JSON) | Simple parsing, type safety, no compatibility burden | All community styles/rules break; must maintain a converter and cannot compare pixel by pixel |
| Fully implement the MapCSS 0.2 spec | "Spec compatible" looks nicer | We do not implement `line: image`/`shield:` etc.; JOSM itself does not support some of them either; poor return on investment |
| Reuse JOSM's Java implementation directly (JNI/JVM embedding) | 100% semantic parity | GPL contagion + requires a JVM + conflicts with the "native Rust rewrite" goal |
| Implement only the validator MapCSS (rendering uses its own format) | Rules compatible, rendering free | One map with two style semantics: the world the user sees differs from the world the rules understand |

## Consequences

**Positive**

- Official styles, presets, and validator rules **load without modification**, with the lowest migration cost, and the community ecosystem remains usable.
- The same AST serves both the rendering and validation consumers (the early-planned filter has been removed), so semantic drift cannot occur.
- Licensing isolation is clear: code (MIT/Apache dual license pending) and assets (GPL/CC-BY-SA) are physically separated, independently replaceable, and hot-reloadable.

**Negative / costs**

- The parser must tolerate JOSM's historical baggage (regexes in comments, relative values `+2`, `::*`, deprecated `@media`).
- "Unsupported items" are a long-lived list requiring UI presentation and documentation maintenance.
- Text layout/font metrics differ from Java AWT, so label positions will differ at the pixel level; this must be written into the deviation list rather than fixed as a bug.
- Evaluation performance depends on tag fingerprint caching + key inverted-index pruning, otherwise 10M objects × thousands of rules is infeasible.

**Constraints (encoded in CI)**

1. No asset file content constants can be grepped in the binary (`include_str!`/`include_bytes!` allowlist).
2. The build fails when `assets/manifest.json` lacks `license`/`sha256`.
3. Adding an "unsupported item" must update both `docs/known-deviations.md` and the `unsupported()` test expectations.

## Verification

- After loading `elemstyles.mapcss`, the 12 core `.validator.mapcss`, and `defaultpresets.xml`, the error count is 0 (warnings allowed), and `unsupported()` corresponds one-to-one with the deviation list.
- The reference data set is compared against JOSM screenshots in a 3×3 grid at zoom 10/13/16/19; deviation entries ≤ 5 and each has a reason.
- All `assertMatch` / `assertNoMatch` in the 12 core rule files pass (as executable corpora, see [12](../12-validation.md) §7).
- Cascade cases (later overrides earlier, `::layer` ordering, `::*` expansion, `+2` relative values, casing pairing) all pass.
- Box selection selects by pure geometry: it does not distinguish whether a feature is rendered (objects hidden by zoom LOD are still within the box-selection range), while click/hover still follow render LOD.

## References

- JOSM MapCSS implementation (selectors/operators/`::layer`/unsupported items): <https://josm.openstreetmap.de/wiki/Help/Styles/MapCSSImplementation>
- MapCSS tag checker (rule clauses and `{n}` placeholders): <https://josm.openstreetmap.de/wiki/Help/Validator/MapCSSTagChecker>
- Tagging presets: <https://josm.openstreetmap.de/wiki/TaggingPresets>
- Editor Layer Index licensing note (NOASSERTION): <https://github.com/osmlab/editor-layer-index>
- To be verified: the complete set of URL placeholders in `maps.xsd`; the official JOSM semantics of `in`/`bbox` in the search box
