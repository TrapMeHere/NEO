# 预设数据（外置）

`schema.neo` 与 `suggestions.neo` 由 `scripts/build-presets.mjs` 生成，**不编进** `neo-desktop.exe`。schema 在打开预设时加载；品牌建议在第一次搜索时加载。

```bash
npm ci
node scripts/build-presets.mjs
```

编译 `neo-style` 时也会自动生成到本目录。发行包需把本目录与 `assets/fonts/` 一起放到可执行文件旁（或设置 `NEO_ASSET_DIR`）。
