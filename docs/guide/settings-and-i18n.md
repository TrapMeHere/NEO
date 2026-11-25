# Settings and languages

## Settings window

Entry: **Help → Settings…** (gear icon). It contains:

### Language

- **English** (default UI and source language), **Simplified Chinese** and **Traditional Chinese**
- On startup NEO detects the operating system's UI locale and picks the matching translation;
  unsupported locales stay on English
- Changes apply immediately, no restart required
- The choice is remembered in `neo-state.json`; without a saved choice the app follows the OS locale
- The language can also be set with an environment variable, which overrides both the saved choice
  and auto-detection: `NEO_LANG=en-US` / `NEO_LANG=zh-CN` / `NEO_LANG=zh-TW`; a bare `NEO_LANG=zh`
  means Simplified Chinese
- Missing translations fall back through Traditional Chinese → Simplified Chinese → English

### Shortcuts

Lists the current built-in bindings: `S/A/X/M`, `Enter`, `Ctrl+Z/Y/S`, `Delete`, plus mouse actions.
In 0.1.0 the shortcut list is read-only; configurable bindings are planned for a later release.

![Settings window](../assets/ui-settings.png)

## OSM account (OAuth2 + PKCE)

Choose **OSM account** on the left of the settings window:

NEO ships a built-in OAuth client (authorization code + PKCE), so there is no client_id or secret to fill in:

1. Click "Sign in with browser"
2. After you authorize in the browser, the app receives the callback on a local port, exchanges the authorization code for a token, and stores it
3. Signed-in accounts are listed and can be switched/deleted

Signing out/deleting only affects the local record; it does not revoke the authorization server-side.

## About window

**Help → About NEO**: version, font coverage samples (to check for missing glyphs),
and icon/font/data license notes.

## Fonts

The UI prefers system CJK fonts and then loads OFL fonts from the external `assets/fonts/`
directory (not embedded in the exe):

| Font | Coverage | License |
|---|---|---|
| Noto Sans CJK SC | Chinese/Japanese/Korean | SIL OFL 1.1 |
| GNU Unifont | full Unicode BMP fallback | OFL 1.1 / GPLv2+ font embedding exception |
| GNU Unifont Upper | SMP / Plane 2 / 3 | same as above |
| Phosphor Icons | UI icons | MIT |

Full license texts: `assets/fonts/OFL-1.1.txt`, `assets/fonts/NOTICE.md`.

## For developers: i18n

All user-visible text is in the repository's `i18n/` directory:

```
i18n/
├── en-US.json   # English: default UI and source/base language (all keys must exist here)
├── zh-CN.json   # Simplified Chinese: translation of en-US
├── zh-TW.json   # Traditional Chinese: translation of en-US
└── README.md
```

- Code: `use neo_i18n::{t, tf};` → `t("menu.file")`, `tf("status.selected", &[("n", "3")])`
- Compile-time `include_str!` embedding, so a release build works without external files; if an
  `i18n/` directory sits next to the executable, its tables override the embedded ones at runtime
- Add new text to `i18n/en-US.json` first (the source); `zh-CN.json` / `zh-TW.json` may lag and
  fall back along the chain `zh-TW` → `zh-CN` → `en-US` at runtime
- The desktop app detects the OS locale at startup (`apps/neo-desktop/src/app/locale.rs`); `NEO_LANG`
  overrides it
- `cargo test -p neo-i18n` checks key/placeholder consistency; `python scripts/check-i18n.py`
  checks that every code key exists in the source table and that translations do not invent keys
- Adding a language: add a JSON file + register it in the `Lang` enum in
  `crates/neo-i18n/src/lib.rs` and implement its `fallback()` (tables are compiled in; a
  directory-scanning loader can be added later if many languages are needed)
