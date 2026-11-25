# 11 Styles, Presets and Search (MapCSS / Presets / Imagery)

> Status: draft v0.1 | Date: 2026-11-15 | Owning crates: `neo-style` (evaluation and assets), `neo-ui` (forms), `neo-validate` (rule consumer)
> Related: [04-spatial-index-and-geometry.md](04-spatial-index-and-geometry.md) | [05-renderer-wgpu.md](05-renderer-wgpu.md) | [12-validation.md](12-validation.md) | [13-josm-parity.md](13-josm-parity.md) | [ADR-0010](adr/0010-mapcss-compatibility.md) | [14-performance-and-verification.md](14-performance-and-verification.md)

## Summary

- One syntax, three consumers: JOSM uses **extended MapCSS 0.2** to drive **map styling**, **validator rules**, and the **search box** at the same time; NEO reuses the same parser (`neo-style::mapcss`) and swaps only the evaluation back end (the rule layer in [12](12-validation.md) §3.1).
- Cascade semantics must be copied from JOSM verbatim: **there is no CSS specificity**; rules apply in file order with "later overrides earlier", and `::layer` splits the property table into layers that are composed at the end. Any "improvement" will make `elemstyles.mapcss` render differently.
- The compatibility target is **an unmodified `elemstyles.mapcss` that loads and draws the same map**, not "supporting the MapCSS 0.2 specification". Divergences are managed as a list (§2.7), not fixed as bugs.
- Unsupported properties (`line: image`, `shield:`, and `font-variant`/`text-decoration`/`text-transform`/`max-width` under `label:`) are detected **at compile time**, reported with readable warnings and degraded to the closest achievable visual effect, never silently dropped.
- `defaultpresets.xml` (660 KB) is likewise a **compatibility target**: `group/item/chunk/reference`, `key/text/combo/multiselect/check/roles`, and `type` combinations (`multipolygon` is not inside `relation`) must all be accepted; the XML→egui form path goes through a neutral `PresetForm` description, and the UI does not parse XML.
- The search box is a subset DSL of MapCSS: `type:`/`key=value`/`-`/`&`/`|`/regex/`preset:` plus NEO's spatial predicates; syntax and evaluation are separate, and a `SearchQuery` can be compiled into a MapCSS selector (JOSM's MapCSS mode).
- All JOSM assets (presets/styles/rules/boundaries) are **loaded at runtime and not compiled into the binary**: GPL/CC-BY-SA assets are isolated from NEO code ([ADR-0010](adr/0010-mapcss-compatibility.md)).
- The imagery catalog comes from ELI (Editor Layer Index), whose GitHub license label is **NOASSERTION**: it is not bundled, CC0 is not assumed, and it is downloaded at runtime with attribution shown per entry.

## 0 Implemented in Phase 4 (v0.1.0)

- `neo-style::mapcss`: MapCSS **subset** parser + compiler (selectors `way/node/area/relation/*`,
  `[key]`/`[key=value]`, comma-separated selectors, `|z12-` zoom, `color/fill-color/width/opacity`,
  `dashes/text/text-color/font-size/icon-image/icon-width`,
  `casing-color/casing-width/casing-dashes`, `#rgb/#rrggbb/#rrggbbaa`)
- Default stylesheet `assets/styles/neo.mapcss` (embedded at compile time); `SceneBuilder` resolves it by
  `(tags, zoom)` while building geometry, overriding the built-in palette/line widths on a hit and falling back otherwise
- Structure rendering (bridges/tunnels/embankments/cuttings/fords): `neo_style::structure_facts` derives semantics from tags,
  `WayStyle` gains `casing` (outer stroke), `hachure` (slope-side symbol), `layer_bias` (bridge +1 / tunnel −1);
  embankments and cuttings are distinguished by **both shape and hue** (ladder-like slope surface vs comb-like tooth line), not only by hatch direction;
  MapCSS gains `casing-color` / `casing-width` / `casing-dashes`; `neo-scene` batches each layer in the order
  fill → casing → stroke → point
- Not supported yet: eval/setting, patterns, @media, MapCSS validator (later)

## 1 Goals and Scope

### 1.1 Goals

1. Fully cover the syntactic subset that `elemstyles.mapcss` actually uses (selectors, operators, pseudo-classes, `::layer`, `@supports`, territory) and make rendered results comparable against JOSM.
2. Style evaluation must remain amenable to incremental recomputation at 10M objects via "tag fingerprint cache + candidate pruning".
3. `defaultpresets.xml` loads with zero modifications; the interactions produced by the form (field types, defaults, `match` filtering) are equivalent to JOSM's.
4. Asset licenses are auditable: every asset has a source, version, sha256, and license identifier.

### 1.2 Non-Goals

- Not a "complete implementation" of the MapCSS specification (`line: image`, `shield:`, Kothic extrusions, etc. are explicitly out of scope, see §2.6).
- No style-editor GUI (P2 in Phase 2); this phase covers loading, compiling, and hot reloading only.
- No normal mapping/3D styling; `extrude`-style extensions are unsupported.
- Not porting JOSM's 40 Java validation tests (that belongs to [12](12-validation.md)).

## 2 Design

### 2.1 Three Consumers, One Front End

```
                     ┌───────────────────────────────────────────┐
  .mapcss text ──►   │ lexer → parser → AST (selectors/decls/@supports) │
                     └───────────────────────────────────────────┘
                          │                │                 │
              ┌───────────┘                │                 └────────────┐
              ▼                            ▼                              ▼
   StyleTarget::Mappaint        StyleTarget::Validator          StyleTarget::Search
   (properties → ResolvedStyle) (throw*/fix* clauses, see 12)    (selector is the query)
              │                            │                              │
              ▼                            ▼                              ▼
    neo-scene → SceneFrame       ValidationReport              Vec<ObjId> result set
```

The three targets share the AST and selector evaluation but **have different property sets**: `fill-color` is meaningless in a validator, and `throwWarning` is meaningless in rendering. `CompileOpts.target` decides which declarations are retained and which report "not supported for this target".

### 2.2 Lexical and Grammar

```text
stylesheet  := (rule | supports_block | "meta" "{" decl* "}" | "canvas" "{" decl* "}")*
rule        := selector_list "{" decl* "}"
selector    := type_sel zoom? cond* pseudo* layer?  ( ">" link? selector | "<" selector )?
type_sel    := "*" | "node" | "way" | "relation" | "area" | "closedway"
             | "meta" | "canvas" | "setting" | "settings"
zoom        := "|z" num ( "-" num? )?            // way|z12, way|z13-15, way|z16-
cond        := "[" "!"? ( key | key op value | "/" regex "/" ) ( "?" | "?!" )? "]"
link        := "[" ( "role" | "index" ) op value "]"
layer       := "::" ( ident | "*" )
decl        := prop ":" value ( ";" value )* ";"?   // "set: class" / "set: .class" is also allowed
supports_block := "@supports" "(" name ":" value ")" "{" rule* "}"
```

| Lexical point | Rule |
|---|---|
| Comments | only `/* … */`; a `/…/` regex inside a comment ends the comment early (the same trap as JOSM, [documentation](https://josm.openstreetmap.de/wiki/Help/Styles/MapCSSImplementation) warns about it explicitly) |
| Quotes | keys/values containing spaces, `:`, or non-ASCII must be `"…"`; `"` and `\` can be escaped |
| Colors | named colors, `#RGB`/`#RRGGBB`/`#RRGGBBAA`, `rgb()`, `rgba()`, `hsb_color()`, optional `name#hex` (user-overridable in color preferences) |
| Numeric values | a relative value has a leading `+` (e.g. `width: +2`), meaning "the same layer's default value + 2", not an absolute value |
| Expressions | bare expressions are allowed (JOSM does not force `eval(...)`); NEO accepts both and compiles them to the same AST |

### 2.3 Selectors and Pseudo-Classes

| Selector | Semantics | NEO priority |
|---|---|---|
| `*` | any object | P0 |
| `node` / `way` / `relation` | primitive type | P0 |
| `area` | any area: a closed way **or** a multipolygon; **also matches unclosed ways** (hence usually combined with `:closed`) | P0 |
| `closedway` | closed way | P0 |
| `meta` | style preamble (`title/icon/version/description/author/link/min-josm-version`) | P0 |
| `canvas` | background (`fill-color`, `default-points`, `default-lines`) | P0 |
| `setting` / `settings` | style-defined custom settings (values read by the `setting()` function) | P1 |
| `parent > child` | child selector (`>`) | P0 (needed by `elemstyles`) |
| `parent < child` | **JOSM-proprietary** parent selector (`<`); properties apply to the object on the right | P1 |
| `>[role=…]` / `>[index=1]` | link selector: member role, index (negative counts from the end) | P1 |
| `|z12` / `|z13-15` / `|z16-` | zoom selector; meaningful only on the right side of `>` | P0 |

Full pseudo-class set (the JOSM-verified list; `!` negates): `:closed`, `:closed2`, `:completely_downloaded`, `:new`, `:connection`, `:unconnected`, `:tagged` (JOSM has an exclusion list of "not counted as tags": `source*`, `source_ref`, `note`, `comment`, `converted_by`, `created_by`, `watch*`, `fixme`, `FIXME`, `description`, `attribution`), `:area-style`, `:righthandtraffic`, `:clockwise`, `:anticlockwise`, `:unclosed_multipolygon`, `:open_end`, `:in-downloaded-area`, `:selected`, `:highlighted`, `:modified`.

- P0 (required for rendering): `closed`, `closed2`, `tagged`, `new`, `modified`, `connection`, `unconnected`, `clockwise`, `anticlockwise`.
- P1 (mostly validation/hints): `completely_downloaded`, `in-downloaded-area`, `righthandtraffic`, `unclosed_multipolygon`, `open_end`, `selected`, `highlighted`.
- `:area-style` is **not applicable** (JOSM uses it internally for "this object was given an area style"; NEO's evaluation order differs, so it is explicitly unsupported and goes into the deviation list).
- Pseudo-class ordinals affect the validator's `{n}` placeholders (`:unconnected` = `{0}`) and must be produced in order by the same parser ([12](12-validation.md) §3.1).

Condition operators (`[k op v]`):

| Operator | Semantics |
|---|---|
| `=` / `!=` | exact match (**case-sensitive**) |
| `<` `>` `<=` `>=` | numeric comparison |
| `^=` / `$=` / `*=` | prefix / suffix / substring |
| `~=` | member of a semicolon-separated list (`vending~=stamps`) |
| `=~` / `!~` | regex (`/…/`; inline flags such as `(?i)` are allowed) |
| `∈` `⊆` | "the left object belongs to the right selector's set": `*[amenity=parking] ∈ area[amenity=parking]` |
| `⊇` | reverse containment (slower; mostly for search rather than rendering) |
| `⊈` / `⊉` | negation of the above; pair with `:closed2` to avoid misjudging unclosed areas |
| `⧉` | intersection/overlap; **respects the `layer` tag** |
| `[k]` / `[!k]` / `[/^addr:/]` | key exists / does not exist / key-name regex |
| `[k?]` / `[k?!]` | truthy (`yes`/`true`/`1`) and falsy |

- **Dereference** `[k1=*k2]`: compares the value of `k1` against the value of `k2` (`[k=~*/re/]` treats the value of the key referenced by `k` as a regex); it expands at compile time into a second lookup, with no string concatenation.
- **territory** `inside("FR")` / `outside("FR,DE")`: for `inside` the commas are **OR**, for `outside` they are **AND**; supports `FR`/`FX`/`EU`/sub-national codes such as `US-FL`; data comes from `boundaries.osm`, evaluation is expensive (use sparingly in styles, commonly in validators).
- The left side of a child selector carries no zoom/layer; `prop()`/`is_prop_set()` are only valid on the right side of `>`; `area` combined with `:closed` must be written explicitly.

### 2.4 Cascade and Priorities (copy JOSM, no "improvements")

JOSM's algorithm is: **walk every rule in file order; when a selector matches, write into the property table (later overrides earlier)**; `::layer` splits the property table into separate layers that are composed at the end. **There is no notion of specificity.**

```
for rule in rules_in_file_order:            # later overrides earlier
    if selector.matches(obj, ctx):
        layer = rule.layer or "default"     # ::name; ::* writes into all layers
        props[layer].apply(rule.decls)
compose(props) -> ResolvedStyle             # sort layers by z-index, apply one by one
```

| Sort key (high to low) | Description |
|---|---|
| `major-z-index` | coarse ordering of object classes (point/line/area) |
| `z-index` | ordering within `::layer` (JOSM default **0**, Halcyon uses 5) |
| `object-z-index` | fine adjustment for a single object within the same layer |
| layer declaration order | tiebreaker: stable and reproducible (golden images depend on it) |

- `::*` is syntactic sugar for "write into/initialize **all** layers"; `elemstyles` uses it for global defaults (e.g. `*::* { … }`); NEO expands it at compile time into "one per declared layer", avoiding a runtime loop over layers.
- Relative values such as `+2` are resolved during composition (taking the current value of `default` on the same layer and adding to it), so "declaration order + layer" information must be preserved and cannot be folded into constants at compile time.
- Multi-file layering (the user activates several styles): concatenated in load order, with `source: origin.file + rule.index` participating in diagnostics and hot-reload targeting.

### 2.5 Property and Value Model

| Group | Representative properties | NEO mapping |
|---|---|---|
| line | `width`, `color`, `opacity`, `dashes`, `linecap`, `linejoin`, `repeat-image*` | `StrokeSpec` (`repeat-image*` goes through icon textures, P2) |
| casing | `casing-*`, `left-casing-*`, `right-casing-*` | separate `StrokeSpec`, bound by `BatchFlags::casing_of` ([05](05-renderer-wgpu.md) §2.3) |
| area | `fill-color`, `fill-image`, `fill-opacity` (default **0.2** since r11700), `fill-extent` | `FillSpec`; the `fill-opacity` default is easily overlooked and must be copied |
| point/icon | `symbol-shape`, `symbol-size`, `symbol-fill-color`, `icon-image`, `icon-width` | `IconSpec` (pipeline C instances) |
| text | `text`, `text-color/-halo-*/-offset-*/-anchor-*/-rotation`, `font-family/-size/-weight/-style`, `text-position` | `TextSpec` (pipeline D, [05](05-renderer-wgpu.md) §2.6) |
| evaluation functions | `tag()`, `prop()`, `is_prop_set()`, `parent_tag()`, `concat()`, `eval()`, `setting()`, `tr()`… | compiled into a closure tree, executed against a whitelist, **no** arbitrary side effects allowed |

The implementation boundary of `eval()`: whitelisted functions + literals + arithmetic/boolean/comparison, never a general-purpose script. `JOSM_pref()`/`JOSM_search()` return NEO settings/search results (the latter triggers recursive evaluation, marked P2 and disabled by default).

### 2.6 Unsupported Items and Deviation List

| JOSM capability | Status | NEO strategy |
|---|---|---|
| `line: image` (repeated texture along a line) | ❌ unsupported (JOSM does not support it either; the [documentation](https://josm.openstreetmap.de/wiki/Help/Styles/MapCSSImplementation) says so explicitly) | compile-time warning, degrade to a solid-color line |
| `shield:` family (road shields) | ❌ unsupported | degrade to `text:` + rounded background (P2 of Phase 2) |
| `font-variant` / `text-decoration` / `text-transform` / `max-width` under `label:` | ❌ unsupported | warning; `text-transform` may be implemented as an option in NEO (P2) |
| `@media (min-josm-version: …)` | ⚠️ accepted but **`@supports` is recommended** (JOSM has deprecated `@media`) | `@media` evaluates as "at the josm-version compatibility level NEO claims", mapping rules in §2.7 |
| `:area-style` | ❌ not applicable | entry 1 of the deviation list |
| regex flavor | Java regex vs Rust `regex` | rules not using lookaround/backreference are equivalent; on hitting an unsupported construct, warn and degrade to "no match" (not a panic) |

> Principle: **unknown property = warning + keep the original text**. `elemstyles.mapcss` contains dozens of properties we do not implement yet; silently dropping them turns "looks slightly different" into an untraceable puzzle. Keeping the original text lets `neo-ui` list "the Nth property of this rule is inactive".

### 2.7 Compatibility Targets for `elemstyles.mapcss`

| Dimension | Target | Acceptance method |
|---|---|---|
| parsing | zero `StyleError`s (warnings allowed, errors not) | CI load + count assertion |
| selectors | every selector used by `elemstyles` compiles (including `area`, `::layer`, `:closed`, zoom) | coverage report: un-hit selector kinds = 0 |
| rendering | manual comparison with JOSM screenshots on the same dataset and zoom (9-cell grid sampling) | deviations recorded in `docs/known-deviations.md` ≤ 5 |
| z-order | `major-z-index`/`z-index` ordering matches JOSM (default 0) | stacking-order comparison cases |
| performance | style evaluation does not enter main-thread hot spots at 10M objects | budget in [05](05-renderer-wgpu.md) §2.13 |

Known deviations (initial list, extended as implementation proceeds, recorded in `docs/known-deviations.md`):

1. `:area-style` is not evaluated.
2. `::*` is expanded at compile time rather than iterated at runtime (semantically equivalent, but the timing details of `is_prop_set` need comparison).
3. Text layout uses NEO's atlas/egui, and font metrics differ from Java AWT (line height, letter spacing), so label avoidance results will differ at the pixel level.
4. `@media` version semantics are mapped by "compatibility level" rather than JOSM build numbers.

### 2.8 Evaluation Pipeline from Style to `SceneFrame`

```
① compile (startup/hot reload, one-time)
   .mapcss ──lexer──► tokens ──parser──► AST ──compile──► StyleSheet
                                                          ├ rules: Vec<CompiledRule>
                                                          ├ key_index: HashMap<Key, SmallVec<RuleId>>  ← pruning
                                                          ├ layers / meta / canvas
                                                          └ unsupported: Vec<UnsupportedProperty>

② evaluate (once per object, result cached)
   tags ──► TagFingerprint(u64)  ──► LRU<TagFingerprint, SmallVec<StyleRef>>
                │ miss
                ├─ candidates = key_index[each present key] ∪ rules without key conditions
                ├─ matches(obj, ctx) one by one          ← selector evaluation (incl. pseudo-classes/territory)
                └─ matched set → cascade compose → ResolvedStyle

③ pack (neo-scene)
   ResolvedStyle + Polyline/AreaGeom ──► MapVertex / RenderBatch / LabelItem
```

| Invalidation condition | Recompute scope |
|---|---|
| an object's tags change | that object's fingerprint cache entry |
| selection/highlight/modification state changes | maintain `pseudo → BTreeSet<ObjId>` and recompute only objects matched by rules containing that pseudo-class |
| style hot reload | rebuild the whole table + clear all caches (switching styles is low-frequency anyway) |
| zoom crosses a level | affects only geometry/LOD; style matches are unchanged (unless the rule contains a zoom selector → cache bucketed by level) |

`TagFingerprint` = a 64-bit hash of the sorted `(key, value)` sequence (including type bits), with full comparison as the collision fallback. This layer is the precondition for "10M objects × thousands of rules" being viable.

### 2.9 Presets XML

```text
<presets xmlns="http://josm.openstreetmap.de/tagging-preset-1.0" author version description shortdescription link icon baselanguage>
  <group name icon>                                  ← tree node
    <item name type preset_name_label icon name_template match_expression>
      <label text/> <optional text/> <space/> <separator/> <item_separator/> <link wiki|href/>
      <key key value match/>                          ← value omitted = delete that key
      <text key/> <combo key values delimiter …/> <multiselect … delimiter=";"/>
      <check key default="on|off"/> <list_entry value/> <checkgroup columns>…</checkgroup>
      <roles><role key text requisite count type member_expression regexp/></roles>
      <preset_link preset_name text/>  <reference ref="surface"/>   ← reference to <chunk id="surface">
    </item>
  </group>
  <chunk id="surface">…</chunk>                        ← reusable fragment
</presets>
```

| Constraint | Description |
|---|---|
| `type` | `node`, `way`, `closedway`, `multipolygon`, `relation`, combinable with commas; **`multipolygon` is not included in `relation`** (a classic bug source) |
| schema | `tagging-preset.xsd` is the parser's sole normative source; with a 660 KB sample file, `match` expressions must be **precompiled** (not parsing XML text per object every time) |
| i18n | attribute-suffix localization (`de.description`) and `baselanguage`; `<label text>` goes through `tr()` |
| references | `<chunk>`/`<reference>` are expanded into a tree at load time (keeping the chunk id for diagnostics), **not** a runtime include |
| validation | `assertMatch`/`assertNoMatch` belong only to validator rules, not presets; presets self-check via the xsd |

Mapping from XML to egui forms (`PresetForm` is a neutral description; only `neo-ui` knows about egui):

| XML element | `PresetForm` control | Interaction |
|---|---|---|
| `<text key>` | `TextField` (single-line/multi-line) | empty value = delete that key |
| `<combo key values editable delimiter>` | `ComboBox` (editable + comma/semicolon splitting) | `values_from` takes values dynamically from another key (P1) |
| `<multiselect>` | `ChipGroup` (delimiter defaults to `;`, `\` escapes) | order preserved |
| `<check key default>` | `Checkbox` | three-state mapping of `on/off` and "absent" |
| `<key key value>` | no control, written directly | a missing `value` means **remove** |
| `<roles>` | member-role table (role/type/count/requisite) | relation-editor constraints |
| `<reference>` | expanded to the same fragment (shared state) | edits affect all reference points |

### 2.10 Reusing Validator Rules

- The same `StyleSheet` front end; `StyleTarget::Validator` retains only `throwError|throwWarning|throwOther` (**exactly one**), `fixAdd`/`fixRemove`/`fixChangeKey`/`fixDeleteObject`/`suggestAlternative`/`group`/`assertMatch`/`assertNoMatch`.
- The numbering of `{0}`/`{1}` comes from **selector order (including pseudo-classes)**, produced uniformly by the parser so `neo-validate` does not have to parse again.
- Rule sources: the 12 core `.validator.mapcss` files + community `Rules/*`; the division of labor with Rust rules, severity levels, and suppression semantics **all follow [12](12-validation.md)**; this document is only responsible for "the same AST".

### 2.11 Imagery Catalog, ELI, and Layer Configuration

| Item | Fact | NEO strategy |
|---|---|---|
| catalog source | JOSM fetches `maps.xml` from `josm.openstreetmap.de`, generated from **ELI (Editor Layer Index)** | consume ELI's JSON/`maps.xml` directly, cached in the user data directory with ETag/version |
| license | the ELI repository's GitHub metadata is labeled **NOASSERTION / Other** → **CC0 must not be assumed** | not bundled into the installer; downloaded at runtime; the UI shows each entry's `attribution` and `terms`; export an "imagery I used" list |
| built-in entries | JOSM ships 5 (Bing, Esri World Imagery, Esri Clarity Beta, Mapbox Satellite, OSM Carto) | do not preselect entries requiring commercial keys; offer keyless ones such as `OSM Carto` plus "add URL" |
| layer types | TMS / WMS / WMTS (separate layer types in JOSM session files) | unified `ImageryLayer { url_template, kind, min_zoom, max_zoom, tile_size, format, attribution }` |
| URL templates | TMS `{z}/{x}/{y}`; WMS `SERVICE=WMS&REQUEST=GetMap&…&BBOX={bbox}&WIDTH={width}&HEIGHT={height}&SRS={proj}`; WMTS `TileMatrix={z}&TileRow={y}&TileCol={x}`; subdomain rotation `{switch:a,b,c}` | the template variable table is fixed with the implementation; **the complete placeholder set > to verify** (per `maps.xsd`) |
| offsets | the offset database is **XML** (not JSON), endpoint `GET {server}get?lat=…&lon=…`, returning `<offset>`; local offsets are persisted per imagery | local x/y offsets + manual adjustment only; the online offset database is P1 (default server `https://offsets.textual.ru/`; note this is a third-party service) |

### 2.12 Filter Semantics (removed)

Filters were implemented in v0.1.0 (hiding matching objects) but have been removed entirely because they conflicted with the product decision that "selection does not depend on render state"; box selection is now a pure geometric selection, while click/hover still follow the render LOD.

### 2.13 Search DSL

| Syntax | Semantics | Source |
|---|---|---|
| `type:node` / `type:way` / `type:relation` | type | JOSM-verified |
| `key=value` | exact match (case-sensitive) | verified |
| `key:value` | substring match (case-insensitive, e.g. `name:Herrengasse`) | verified |
| `"addr:street"="Baker Street"` | keys/values containing `:` or spaces must be quoted | verified |
| `-expr` | negation (e.g. `-highway:`) | verified |
| `expr1 expr2` / `expr1 & expr2` | AND (juxtaposition means AND) | verified |
| `expr1 \| expr2` | OR | verified |
| `key=~/regex/`, `/^addr:/` | regex (JOSM uses Java regex; NEO uses the `regex` crate; construction differences in §2.6) | verified |
| `id:`, `user:`, `timestamp:a/b`, `version:`, `changeset:` | metadata predicates | verified (example includes `user:Steve`) |
| `preset:"Craft/Beekeeper"` | match by preset | verified |
| the whole string handed to a MapCSS selector | JOSM search box's "MapCSS selector" mode | verified |
| `in <bbox>` / `bbox:` | spatial range predicate | **> to verify**: JOSM's official search page does not list `in`/`bbox`; NEO implements it as an extension and marks it as such |

Evaluation order: `DSL parse → type/key index pruning → predicate evaluation → return Vec<ObjId>`. Spatial predicates use the object R-tree from [04](04-spatial-index-and-geometry.md); the precedence of `-` with `&`/`|` follows "`-` highest, `&` next, `|` lowest", with explicit grouping via parentheses.

### 2.14 i18n and Asset Loading

| Asset | Loading method | License/isolation |
|---|---|---|
| `elemstyles.mapcss`, presets, the 12 validator rules, `boundaries.osm` | read at runtime from the assets directory or the user directory (hot reload) | GPL/CC-BY-SA assets are physically separated from code; no `include_str!`, not in the binary |
| translations | JOSM's `.lang` (48 files) coexist with NEO's own keys; look up the NEO table first, then the asset table | `.lang` ships with the asset directory |
| fonts | system font enumeration (CJK see [05](05-renderer-wgpu.md) §2.11), optional OFL font download | not in the binary |
| catalog manifest | `assets/manifest.json`: source URL, version, sha256, license identifier | CI verifies sha256; a missing license field fails the build |

Key spaces of `tr()`: NEO's own UI uses the `neo.*` prefix, and JOSM assets use the original English strings (aligned with `.lang`), so the two translation systems never override each other.

## 3 Key Interfaces

```rust
// ── neo-style/src/mapcss/mod.rs ─────────────────────────────────────────────
pub enum StyleTarget { Mappaint, Validator, Search }
pub struct CompileOpts { pub target: StyleTarget, pub settings: StyleSettings, pub claimed_josm_version: u32 }

pub struct StyleSheet { /* rules, key_index, layers, meta, canvas, unsupported */ }
impl StyleSheet {
    pub fn compile(src: &str, opts: &CompileOpts) -> Result<Self, Vec<StyleError>>;
    pub fn extend(&mut self, src: &str, origin: StyleOrigin) -> Result<(), Vec<StyleError>>; // multi-file layering
    pub fn apply(&self, obj: &StyleObject<'_>, ctx: &StyleCtx<'_>) -> ResolvedStyle;         // cascade + layer composition
    pub fn candidates(&self, tags: &TagView<'_>, kind: GeomKind) -> RuleCandidates<'_>;      // key-index pruning
    pub fn meta(&self) -> &StyleMeta;
    pub fn canvas(&self) -> &CanvasStyle;
    pub fn unsupported(&self) -> &[UnsupportedProperty];   // exported for the UI and known-deviations
    pub fn fingerprint(&self) -> u64;                      // hot-reload detection
}

pub struct StyleObject<'a> {
    pub kind: GeomKind,
    pub tags: TagView<'a>,
    pub pseudo: PseudoSet,
    pub geom: Option<GeomFacts>,   // closed, area, direction, unclosed multipolygon
}
pub struct StyleCtx<'a> {
    pub zoom: f64,
    pub territory: Option<&'a TerritoryDb>,   // inside()/outside()
    pub settings: &'a StyleSettings,          // setting()
    pub layer_seq: &'a [LayerId],
}
pub struct ResolvedStyle {
    pub fill: Option<FillSpec>,
    pub stroke: Vec<StrokeSpec>,     // main stroke + casing(left/right)
    pub icon: Option<IconSpec>,
    pub text: Option<TextSpec>,
    pub z: ZKey,                     // (major_z, z, object_z)
    pub layer: LayerId,
}

// ── neo-style/src/preset.rs ─────────────────────────────────────────────────
pub struct PresetCatalog { /* precompiled match expressions + name/type indexes */ }
impl PresetCatalog {
    pub fn load_xml(bytes: &[u8], i18n: &dyn I18n) -> Result<Self, Vec<PresetError>>;
    pub fn extend_xml(&mut self, bytes: &[u8], i18n: &dyn I18n) -> Result<(), Vec<PresetError>>;
    pub fn matches_for(&self, kind: PresetType, tags: &TagView<'_>) -> Vec<&PresetItem>;
    pub fn item_by_name(&self, name: &str) -> Option<&PresetItem>;
    pub fn roots(&self) -> &[PresetGroup];
    pub fn form(&self, item: &PresetItem) -> PresetForm;   // neutral description, independent of egui
}

// ── neo-editor/src/search.rs ────────────────────────────────────────────────
pub struct SearchQuery { /* AST */ }
impl SearchQuery {
    pub fn parse(src: &str) -> Result<Self, Vec<SearchError>>;
    pub fn eval(&self, snapshot: &Snapshot, index: &SpatialIndex, bbox: Option<Aabb>) -> Vec<ObjId>;
    pub fn to_mapcss_selector(&self) -> Option<String>;   // JOSM's MapCSS search mode
}

```

## 4 Data Flow

```
startup / hot reload: assets/*.mapcss ─► StyleSheet::compile + extend ─► neo-scene
             defaultpresets.xml ─► PresetCatalog::load_xml (chunk/reference expansion + match precompilation)
             validator/*.mapcss ─► StyleSheet::compile(Validator) ─► neo-validate
             boundaries.osm ─► TerritoryDb; ELI/maps.xml ─► ImageryCatalog (cache + attribution)
per frame: ① data snapshot ─► ② style evaluation (worker thread, fingerprint cache) ─► ③ pack SceneFrame ─► neo-render (only ③ on the main thread)
editing: change tag ─► fingerprint invalidated ─► recompute only that object; change selection ─► recompute only :selected/:highlighted-related
```

```
one search
  parse("type:way highway & -service") ──► AST
     ├─ key index: candidates = ways containing highway
     ├─ predicate evaluation (tags + regex + spatial)
     └─ result Vec<ObjId> ──► selection set (undoable as a standalone command)
```

## 5 Trade-offs and Alternatives

| Decision | Alternative | Why this choice | Cost |
|---|---|---|---|
| one parser, three targets | write a separate validator parser | JOSM's `.validator.mapcss` shares selector syntax with styles; two parsers would inevitably drift | the parser needs `StyleTarget` branches, enlarging the test surface |
| copy JOSM's "later overrides earlier" | introduce CSS specificity | `elemstyles` depends on file order; adding specificity would change rendering results | rule order becomes a contract; reordering breaks compatibility |
| detect unsupported properties at compile time and degrade | fail wholesale on any error | the 663 KB official stylesheet inevitably contains properties we have not implemented; failing wholesale = unusable | must maintain the `unsupported` list and UI hints |
| tag fingerprint cache + key inverted-index pruning | full match of every object against every rule | 10M × thousands of rules is infeasible | cache-invalidation logic is a new bug surface |
| load assets at runtime | compile into the binary | GPL/CC-BY-SA isolation + hot updates + user-swappable | one more IO and validation at startup; must degrade gracefully when assets are missing |
| neutral `PresetForm` for presets | UI directly manipulates the XML DOM | egui is decoupled from XML and form generation is unit-testable | one more mapping layer |
| no bundled imagery, online catalog | ship a built-in snapshot | licensing (NOASSERTION) and freshness | first startup needs network; offline requires a cache |

## 6 Risks

| # | Risk | Impact | Mitigation |
|---|---|---|---|
| R1 | GPL/CC-BY-SA assets accidentally compiled into the binary | license contamination, the whole package would need GPL | CI checks an allowlist for `include_str!`/`include_bytes!`; asset directory separate + manifest |
| R2 | MapCSS semantic deviations (especially `::*`, `:area-style`, z-order) | rendering does not match JOSM | register each in `known-deviations.md`; 9-cell screenshot comparison |
| R3 | style evaluation slows down the main thread | dropped frames | fingerprint cache + worker thread + the frame budget from `05`; when over budget lower LOD, not styles |
| R4 | 660 KB of presets + large match expressions make startup/matching slow | preset panel stutters | precompile matches into predicates at load time; index by type; finish asynchronously after startup |
| R5 | regex flavor differences (lookaround/backref) | some rule never matches | report "unsupported regex construct" at compile time, degrade to no-match with a hint |
| R6 | ELI catalog entries change license/terms | users misled into using restricted imagery | show attribution/terms per entry; no "select all" shortcut |
| R7 | mixing `.lang` with NEO's own i18n causes cross-contamination | garbled UI copy | key-prefix isolation (`neo.*` vs original text), fixed load order |

## 7 Acceptance Criteria

1. **Parsing**: loading `elemstyles.mapcss`, the 12 `.validator.mapcss` files, and `defaultpresets.xml` yields 0 `StyleError`/`PresetError`; `unsupported()` entries correspond one-to-one with `docs/known-deviations.md`.
2. **Render comparison**: on the reference dataset at zoom 10/13/16/19, compare 9-cell screenshots against JOSM; deviation entries ≤ 5, each with a stated reason.
3. **Cascade**: construct 20 cases covering "later overrides earlier", `::layer` ordering, `::*` expansion, relative `+2`, and paired casing; all pass.
4. **Presets**: `group/item/chunk/reference` expand correctly; `type` combinations (including the difference between `multipolygon` and `relation`) cases pass; `match` filtering on 10k objects takes ≤ [pending measurement, see docs/14-performance-and-verification.md] per run.
5. **Search**: each syntax in the §2.13 table has at least 2 cases; `-`/`&`/`|` precedence cases pass.
6. **Licensing**: `assets/manifest.json` covers all assets, and CI fails when license/sha256 is missing; no asset file string constants can be found in the binary by grep.

## 8 References

- JOSM's MapCSS implementation (selectors, operators, `::layer`, `@supports`, list of unsupported items): <https://josm.openstreetmap.de/wiki/Help/Styles/MapCSSImplementation>
- MapCSS tag checker (`throw*/fix*/assertMatch`, `{n}` placeholders): <https://josm.openstreetmap.de/wiki/Help/Validator/MapCSSTagChecker>
- Tagging presets (XML structure, `chunk`/`reference`, field types): <https://josm.openstreetmap.de/wiki/TaggingPresets>
- Search syntax (`type:`, `key=value`, `-`, `&`, `|`, regex, `preset:`, MapCSS mode): <https://josm.openstreetmap.de/wiki/Help/Action/Search>
- Editor Layer Index (license labeled NOASSERTION; needs case-by-case confirmation): <https://github.com/osmlab/editor-layer-index>
- JOSM imagery offset database (XML + query-string endpoint): <https://josm.openstreetmap.de/wiki/Plugin/ImageryOffsetDb>
- To verify: the complete URL placeholder set of `maps.xsd` (`{switch:…}` etc.) **> to verify**; the official semantics of the search-box `in`/`bbox` keywords in JOSM **> to verify**; per-file provenance of icon assets **> to verify**.
