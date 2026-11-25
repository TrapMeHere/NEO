# Preset coverage: JOSM groups × id-tagging-schema prefixes

- **Date**: 2026-11-16
- **Source**: `neo-cli presets` (`neo_style::coverage_markdown`)
- **Comparison method**: count schema ids by prefix for the 22 public group names. JOSM's `defaultpresets.xml` is not read (GPL, R4).

This table answers "does each group have at least some presets", not "does every JOSM default preset have an entry with the same name".

| JOSM group | schema prefix | Count |
|---|---|---:|
| Highways | `highway/` | 109 |
| Waterways | `waterway/` | 20 |
| Railways | `railway/` | 34 |
| Power | `power/` | 31 |
| Buildings | `building/` | 68 |
| Amenities | `amenity/` | 317 |
| Man made | `man_made/` | 107 |
| Natural | `natural/` | 66 |
| Leisure | `leisure/` | 128 |
| Shops | `shop/` | 187 |
| Tourism | `tourism/` | 42 |
| Emergency | `emergency/` | 22 |
| Barriers | `barrier/` | 45 |
| Aerialways | `aerialway/` | 14 |
| Aeroways | `aeroway/` | 13 |
| Historic | `historic/` | 24 |
| Office | `office/` | 55 |
| Craft | `craft/` | 52 |
| Healthcare | `healthcare/` | 25 |
| Telecom | `telecom/` | 2 |
| Seamarks | `seamark/` | 7 |
| Pipelines | `pipeline/` | 2 |

Missing groups (count 0): (none)

All 22 listed groups have at least 1 entry. Schema entries outside these prefixes are not in this table and are not counted as item-by-item aligned with JOSM.
