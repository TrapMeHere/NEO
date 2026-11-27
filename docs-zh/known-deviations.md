# 与 JOSM 的已知差异

本页记录 NEO 与 JOSM 之间**已知且可接受的行为差异**。它不是 bug 列表：条目可能是已记录的限制、有意的范围取舍，或仍在测量中的差异。每条都要给出原因；只有行为与 JOSM 一致后才会删除。

相关文档：[11 样式、预设与 MapCSS](11-style-presets-mapcss.md) §2.7 · [12 校验器](12-validation.md) · [13 JOSM 功能对照](13-josm-parity.md) · [ADR-0010](adr/0010-mapcss-compatibility.md)

## MapCSS 与渲染

- 不执行 `:area-style`。
- `::*` 在编译期展开，而不是运行期迭代（语义等价，但 `is_prop_set` 的时序细节仍待对比）。
- 文本排版使用 NEO 的 atlas/egui，字体度量与 Java AWT 不同（行高、字距），因此标签避让结果会在像素级不同。
- `@media` 版本语义按“兼容级别”映射，而不是按 JOSM 构建号。
- 明确不支持的属性通过 `unsupported()` 上报并展示给用户，而不是静默丢弃（[ADR-0010](adr/0010-mapcss-compatibility.md) D4）。

## 校验器

- MapCSS validator 兼容层尚未实现。内置规则是对 JOSM 行为的 Rust 重实现，而不是移植其 Java 测试类，因此规则覆盖与文案可能不同（见 [12 校验器](12-validation.md) §0）。

## 维护状态

本清单由人工维护，随实现推进持续增补；当前允许的偏差数量见相关文档中的验收标准。
