# i18n — NEO UI text

All **user-visible text** lives in this directory; the code no longer hardcodes Chinese or English UI copy:

| File | Language | Notes |
|---|---|---|
| `en-US.json` | English | **Default UI and source/base language**: every key used by the code must exist here |
| `zh-CN.json` | Simplified Chinese | Translation of `en-US` |
| `zh-TW.json` | Traditional Chinese (Taiwan) | Translation of `en-US`; may lag behind and fall back to `zh-CN` |

JSON is a flat `"key": "text"` mapping in UTF-8. The `neo-i18n` crate embeds the files at
**compile time** with `include_str!`, so a release exe does not need to ship them (the
packaging scripts still copy them next to the executable so translations can be updated
without rebuilding).

## Runtime fallback

`t()` resolves keys in this order:

1. current language table;
2. fallback chain `zh-TW → zh-CN → en-US`;
3. the key itself (so missing translations are visible).

This means a new English string shows up in the Chinese UI as English until it is
translated, and Traditional Chinese falls back to Simplified Chinese first.

## Using it in code

```rust
use neo_i18n::{t, tf};

ui.label(t("panel.layers"));                       // plain lookup
let text = tf("status.selected", &[("n", "3")]);   // {n} placeholder
```

- `t(key)`: one UI string, with the fallback chain described above.
- `tf(key, &[("name", "value")])`: replaces `{name}` placeholders.
- English is the default UI language. At startup the desktop app reads the OS locale and
  selects the matching translation, otherwise it stays on English; `NEO_LANG` always
  overrides it (`NEO_LANG=en-US`, `zh-CN`, `zh-TW`; a bare `zh` means Simplified).
- The language can also be switched at runtime from the in-app **Language** menu.
- Adding text: add the key to `i18n/en-US.json` first (source). Translations in
  `zh-CN.json` / `zh-TW.json` are optional at first — the runtime falls back.

## Checks

```powershell
python scripts\check-i18n.py   # every code key must exist in en-US; zh tables may lag but must not
                               # invent keys; placeholders must match; prints zh coverage
# cargo test -p neo-i18n: language parsing, fallback chain, placeholder consistency
```

## Conventions

- Key naming is `area.object.action`: `menu.file.save_xml`, `panel.layers`, `app.error.import`.
- Icons are composed in code (e.g. `format!("{} {}", ph::FOLDER_OPEN, t("menu.file.open_pbf"))`);
  translation files contain plain text only.
- Do not put font coverage samples or OSM data names (e.g. `Monaco`) into the tables.
- The repository baseline language is English: docs, comments, scripts and commit messages
  are written in English; the `zh-*` files are translations.
