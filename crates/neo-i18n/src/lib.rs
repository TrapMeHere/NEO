//! # neo-i18n — NEO UI text
//!
//! All user-visible text lives in the repository `i18n/` directory:
//!
//! * `i18n/en-US.json` — English (default UI language, source/base language; every key must exist here)
//! * `i18n/zh-CN.json` — Simplified Chinese (translation of en-US)
//! * `i18n/zh-TW.json` — Traditional Chinese, Taiwan (translation of en-US)
//!
//! English is the baseline language. At startup the desktop app detects the operating
//! system locale and picks the matching language, falling back to English. `NEO_LANG`
//! always overrides the detection. Chinese files are translations of the English base.
//! Resolution order: `zh-TW` falls back to `zh-CN`, which falls back to English; English
//! is the root and returns the key itself when a key is missing.
//!
//! At startup the loader prefers `i18n/*.json` next to the executable and falls back to the
//! copies embedded at compile time when files are missing or broken. Use [`t`] for a single
//! string and [`tf`] for `{name}` placeholder substitution.
//!
//! The language can be switched at runtime with the `NEO_LANG` environment variable or the
//! in-app "Language" menu.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// Supported UI languages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lang {
    /// English (default UI language; source/base language).
    #[default]
    EnUs,
    /// Simplified Chinese (translation of en-US).
    ZhCn,
    /// Traditional Chinese, Taiwan (translation of en-US).
    ZhTw,
}

impl Lang {
    /// Parses a language/locale code (`en`, `en-US`, `zh`, `zh-CN`, `zh_TW.UTF-8`,
    /// `zh-Hant`, `zh-Hans`, `de-DE`, ...).
    ///
    /// Only languages NEO ships translations for are recognized; anything else returns
    /// `None` so callers can fall back to the default (English). A bare `zh` is treated as
    /// Simplified Chinese, and `zh-Hant`/`zh-TW`/`zh-HK` as Traditional Chinese.
    pub fn from_code(code: &str) -> Option<Self> {
        let normalized = code.trim().to_ascii_lowercase().replace('_', "-");
        // Drop POSIX suffixes such as "zh-cn.utf-8" or "sr-rs@latin".
        let base = normalized.split(['.', '@']).next().unwrap_or(normalized.as_str());
        if base == "zh" || base.starts_with("zh-") {
            if base.contains("hant") || base.contains("-tw") || base.contains("-hk") || base.contains("-mo") {
                return Some(Self::ZhTw);
            }
            return Some(Self::ZhCn);
        }
        if base == "en" || base.starts_with("en-") || base == "english" {
            return Some(Self::EnUs);
        }
        match base {
            "cn" | "chinese" => Some(Self::ZhCn),
            "traditional" => Some(Self::ZhTw),
            _ => None,
        }
    }

    /// Language code used when persisting/displaying the setting.
    pub fn code(self) -> &'static str {
        match self {
            Self::EnUs => "en-US",
            Self::ZhCn => "zh-CN",
            Self::ZhTw => "zh-TW",
        }
    }

    /// Fallback language used when the current table misses a key.
    ///
    /// `zh-TW` → `zh-CN` → English; English is the base and has no further fallback.
    pub fn fallback(self) -> Option<Self> {
        match self {
            Self::EnUs => None,
            Self::ZhCn => Some(Self::EnUs),
            Self::ZhTw => Some(Self::ZhCn),
        }
    }

    /// Whether this language is Chinese (Simplified or Traditional).
    pub fn is_chinese(self) -> bool {
        matches!(self, Self::ZhCn | Self::ZhTw)
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::EnUs => "en-US.json",
            Self::ZhCn => "zh-CN.json",
            Self::ZhTw => "zh-TW.json",
        }
    }

    fn index(self) -> u8 {
        match self {
            Self::EnUs => 0,
            Self::ZhCn => 1,
            Self::ZhTw => 2,
        }
    }

    fn from_index(index: u8) -> Self {
        match index {
            1 => Self::ZhCn,
            2 => Self::ZhTw,
            _ => Self::EnUs,
        }
    }
}

const ZH_JSON: &str = include_str!("../../../i18n/zh-CN.json");
const ZH_TW_JSON: &str = include_str!("../../../i18n/zh-TW.json");
const EN_JSON: &str = include_str!("../../../i18n/en-US.json");

static CURRENT: AtomicU8 = AtomicU8::new(0);
static ZH: OnceLock<HashMap<String, String>> = OnceLock::new();
static ZH_TW: OnceLock<HashMap<String, String>> = OnceLock::new();
static EN: OnceLock<HashMap<String, String>> = OnceLock::new();

fn embedded(lang: Lang) -> &'static str {
    match lang {
        Lang::EnUs => EN_JSON,
        Lang::ZhCn => ZH_JSON,
        Lang::ZhTw => ZH_TW_JSON,
    }
}

fn load_table(lang: Lang) -> HashMap<String, String> {
    for dir in i18n_search_dirs() {
        let path = dir.join(lang.file_name());
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(table) = serde_json::from_str::<HashMap<String, String>>(&text)
            && !table.is_empty()
        {
            return table;
        }
    }
    serde_json::from_str(embedded(lang)).unwrap_or_else(|err| {
        panic!("embedded i18n {} is invalid JSON: {err}", lang.file_name());
    })
}

fn i18n_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        dirs.push(parent.join("i18n"));
        // During development, climb from out/neo-desktop/... or out/cargo/... to the repo root.
        if let Some(root) = parent.parent().and_then(|p| p.parent()) {
            dirs.push(root.join("i18n"));
        }
        if let Some(root) = parent.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
            dirs.push(root.join("i18n"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("i18n"));
    }
    dirs
}

fn catalog(lang: Lang) -> &'static HashMap<String, String> {
    match lang {
        Lang::EnUs => EN.get_or_init(|| load_table(Lang::EnUs)),
        Lang::ZhCn => ZH.get_or_init(|| load_table(Lang::ZhCn)),
        Lang::ZhTw => ZH_TW.get_or_init(|| load_table(Lang::ZhTw)),
    }
}

/// Current UI language (English by default until the app detects the system locale).
pub fn current_language() -> Lang {
    Lang::from_index(CURRENT.load(Ordering::Relaxed))
}

/// Switches the UI language (takes effect immediately; panels re-read text every frame).
pub fn set_language(lang: Lang) {
    CURRENT.store(lang.index(), Ordering::Relaxed);
}

/// Language from environment variables, if any is set and recognized.
///
/// Checks POSIX locale variables in priority order. Windows/macOS GUI applications
/// usually do not set them; the desktop app performs an OS-level detection and calls
/// [`set_language`] directly.
pub fn language_from_env() -> Option<Lang> {
    for name in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
        if let Ok(value) = std::env::var(name)
            && let Some(lang) = Lang::from_code(&value)
        {
            return Some(lang);
        }
    }
    None
}

/// Initializes the language from `NEO_LANG`, then from POSIX locale variables.
///
/// Keeps the default (English) when nothing matches.
pub fn init_from_env() {
    if let Ok(code) = std::env::var("NEO_LANG")
        && let Some(lang) = Lang::from_code(&code)
    {
        set_language(lang);
        return;
    }
    if let Some(lang) = language_from_env() {
        set_language(lang);
    }
}

/// Looks up one UI string; falls back through [`Lang::fallback`] and then returns the key.
pub fn t(key: &'static str) -> &'static str {
    let lang = current_language();
    if let Some(value) = catalog(lang).get(key) {
        return value.as_str();
    }
    if let Some(fallback) = lang.fallback()
        && let Some(value) = catalog(fallback).get(key)
    {
        return value.as_str();
    }
    key
}

/// Replaces `{name}` placeholders in a UI string.
///
/// ```
/// # use neo_i18n::tf;
/// let text = tf("selection.more", &[("n", "3")]);
/// assert!(text.contains('3'));
/// ```
pub fn tf(key: &'static str, args: &[(&str, &str)]) -> String {
    let mut text = t(key).to_owned();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

/// Diagnostics: returns the i18n directories that would be tried (used by tests).
pub fn search_dirs_for_test() -> Vec<PathBuf> {
    i18n_search_dirs()
}

/// Whether a path looks like a language table.
pub fn looks_like_table(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn table(json: &str) -> HashMap<String, String> {
        serde_json::from_str(json).expect("valid locale JSON")
    }

    fn placeholders(text: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            let Some(close) = rest[open..].find('}') else {
                break;
            };
            out.insert(rest[open + 1..open + close].to_owned());
            rest = &rest[open + close + 1..];
        }
        out
    }

    #[test]
    fn default_language_is_english() {
        assert_eq!(Lang::default(), Lang::EnUs);
        assert_eq!(current_language(), Lang::EnUs);
        assert_eq!(Lang::EnUs.fallback(), None);
        assert_eq!(Lang::ZhCn.fallback(), Some(Lang::EnUs));
        assert_eq!(Lang::ZhTw.fallback(), Some(Lang::ZhCn));
        assert!(Lang::ZhTw.is_chinese() && Lang::ZhCn.is_chinese());
        assert!(!Lang::EnUs.is_chinese());
    }

    #[test]
    fn locale_codes_are_parsed() {
        assert_eq!(Lang::from_code("en"), Some(Lang::EnUs));
        assert_eq!(Lang::from_code("en-US"), Some(Lang::EnUs));
        assert_eq!(Lang::from_code("en_GB.UTF-8"), Some(Lang::EnUs));
        assert_eq!(Lang::from_code("zh"), Some(Lang::ZhCn));
        assert_eq!(Lang::from_code("zh_CN.UTF-8"), Some(Lang::ZhCn));
        assert_eq!(Lang::from_code("zh-Hans-CN"), Some(Lang::ZhCn));
        assert_eq!(Lang::from_code("zh-TW"), Some(Lang::ZhTw));
        assert_eq!(Lang::from_code("zh_tw.utf8"), Some(Lang::ZhTw));
        assert_eq!(Lang::from_code("zh-Hant"), Some(Lang::ZhTw));
        assert_eq!(Lang::from_code("zh-HK"), Some(Lang::ZhTw));
        assert_eq!(Lang::from_code("fr-FR"), None);
        assert_eq!(Lang::from_code("C"), None);
    }

    #[test]
    fn language_codes_and_files_are_unique() {
        let langs = [Lang::EnUs, Lang::ZhCn, Lang::ZhTw];
        let codes: BTreeSet<&str> = langs.iter().map(|lang| lang.code()).collect();
        let files: BTreeSet<&str> = langs.iter().map(|lang| lang.file_name()).collect();
        assert_eq!(codes.len(), langs.len());
        assert_eq!(files.len(), langs.len());
        for lang in langs {
            assert!(looks_like_table(Path::new(lang.file_name())));
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
            assert_eq!(Lang::from_index(lang.index()), lang);
        }
    }

    #[test]
    fn translations_are_subsets_of_the_english_base_with_same_placeholders() {
        let english = table(EN_JSON);
        let simplified = table(ZH_JSON);
        let traditional = table(ZH_TW_JSON);
        assert!(english.len() > 300, "English base looks empty: {}", english.len());
        // Translations may temporarily miss keys (they fall back), but must never invent keys.
        assert!(
            simplified.keys().all(|key| english.contains_key(key)),
            "zh-CN.json contains keys missing from the en-US.json base"
        );
        assert!(
            traditional.keys().all(|key| english.contains_key(key)),
            "zh-TW.json contains keys missing from the en-US.json base"
        );
        for (key, value) in &english {
            for (locale, other) in [("zh-CN", &simplified), ("zh-TW", &traditional)] {
                if let Some(other) = other.get(key) {
                    assert_eq!(
                        placeholders(value),
                        placeholders(other),
                        "`{key}` has inconsistent placeholders between en-US and {locale}"
                    );
                }
            }
        }
    }

    #[test]
    fn switching_languages_uses_the_matching_table() {
        set_language(Lang::ZhTw);
        assert_eq!(current_language(), Lang::ZhTw);
        assert_eq!(t("menu.file"), "檔案");
        set_language(Lang::ZhCn);
        assert_eq!(current_language(), Lang::ZhCn);
        assert_eq!(t("menu.file"), "文件");
        set_language(Lang::EnUs);
        assert_eq!(current_language(), Lang::EnUs);
        assert_eq!(t("menu.file"), "File");
    }
}
