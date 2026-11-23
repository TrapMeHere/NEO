# 预设覆盖：JOSM 分组 × id-tagging-schema 前缀

- **日期**：2026-11-16
- **来源**：`neo-cli presets`（`neo_style::coverage_markdown`）
- **对照方式**：22 个公开分组名对 schema id 前缀计数。不读取 JOSM 的 `defaultpresets.xml`（GPL，R4）。

这张表回答的是“每个分组里至少有没有预设”，不是“JOSM 默认预设的每一条都有同名条目”。

| JOSM 分组 | schema 前缀 | 条数 |
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

缺失分组（计数为 0）：（无）

22 个列出的分组都至少有 1 条。未落入这些前缀的 schema 条目不在本表里，也不算已与 JOSM 逐条对齐。
