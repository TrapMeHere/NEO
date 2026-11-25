# Known Deviations from JOSM

This page tracks **known, accepted behavioral differences** between NEO and JOSM. It is not a bug
list: an entry may be a documented limitation, a deliberate scope decision, or a difference that is
still being measured. Each entry needs a reason, and entries are removed only when the behavior
matches JOSM.

Related: [11 Styles, Presets and MapCSS](11-style-presets-mapcss.md) §2.7 · [12 Validator](12-validation.md) · [13 JOSM Parity](13-josm-parity.md) · [ADR-0010](adr/0010-mapcss-compatibility.md)

## MapCSS and rendering

- `:area-style` is not evaluated.
- `::*` is expanded at compile time rather than iterated at runtime (semantically equivalent, but
  the timing details of `is_prop_set` still need comparison).
- Text layout uses NEO's atlas/egui, and font metrics differ from Java AWT (line height, letter
  spacing), so label avoidance results will differ at the pixel level.
- `@media` version semantics are mapped by "compatibility level" rather than JOSM build numbers.
- Explicitly unsupported properties are reported through `unsupported()` and shown to the user
  instead of being silently dropped ([ADR-0010](adr/0010-mapcss-compatibility.md) D4).

## Validator

- The MapCSS validator compatibility layer is not implemented yet. Built-in rules are a Rust
  reimplementation with aligned behavior, not a port of JOSM's Java test classes, so rule coverage
  and wording can differ (see [12 Validator](12-validation.md) §0).

## Status

The list is maintained by hand and extended as implementation proceeds; see the acceptance criteria
in the linked documents for the current allowed number of deviations.
