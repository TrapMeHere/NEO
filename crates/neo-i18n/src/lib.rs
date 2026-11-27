//! # neo-i18n — NEO UI text
//!
//! All user-visible text lives in the repository `i18n/` directory:
//!
//! * `i18n/en-US.json` — English (default UI language, source/base language; every key must exist here)
//! * `i18n/zh-CN.json` — Simplified Chinese
//! * `i18n/zh-TW.json` — Traditional Chinese (Taiwan)
//! * `i18n/fr-FR.json` — French
//! * `i18n/de-DE.json` — German
//! * `i18n/ru-RU.json` — Russian
//! * `i18n/ar-EG.json` — Arabic (Egypt)
//! * `i18n/vi-VN.json` — Vietnamese
//! * `i18n/ja-JP.json` — Japanese
//! * `i18n/ko-KR.json` — Korean
//! * `i18n/ms-MY.json` — Malay
//!
//! English is the baseline language. At startup the desktop app uses the language saved
//! from the Language menu; without a saved choice it detects the operating system locale
//! and picks the matching language, falling back to English. `NEO_LANG` always overrides.
//!
//! All non-English tables are translations of the English base. `zh-TW` falls back to
//! `zh-CN`; every other translation falls back to English. English itself is the root and
//! returns the key when a key is missing.
//!
//! At startup the loader prefers `i18n/*.json` next to the executable and falls back to the
//! copies embedded at compile time when files are missing or broken. Use [`t`] for a single
//! string and [`tf`] for `{name}` placeholder substitution.
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
    /// Simplified Chinese.
    ZhCn,
    /// Traditional Chinese (Taiwan).
    ZhTw,
    /// French.
    Fr,
    /// German.
    De,
    /// Russian.
    Ru,
    /// Arabic (Egypt).
    ArEg,
    /// Vietnamese.
    Vi,
    /// Japanese.
    Ja,
    /// Korean.
    Ko,
    /// Malay.
    Ms,
}

/// All supported languages, in menu order (English first).
const ALL: [Lang; 11] = [
    Lang::EnUs,
    Lang::ZhCn,
    Lang::ZhTw,
    Lang::Fr,
    Lang::De,
    Lang::Ru,
    Lang::ArEg,
    Lang::Vi,
    Lang::Ja,
    Lang::Ko,
    Lang::Ms,
];

impl Lang {
    /// Parses a language/locale code (`en-US`, `zh_CN.UTF-8`, `fr-FR`, `de-DE`, `ru-RU`,
    /// `ar-EG`, `vi-VN`, `ja-JP`, `ko-KR`, `ms-MY`, ...).
    ///
    /// Only languages NEO ships translations for are recognized; anything else returns
    /// `None` so callers can fall back to the default (English). A bare `zh` is treated as
    /// Simplified Chinese, and `zh-Hant`/`zh-TW`/`zh-HK` as Traditional Chinese. Any
    /// Arabic variant maps to Arabic (Egypt) for now.
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
        let language = base.split('-').next().unwrap_or(base);
        match language {
            "en" | "english" => Some(Self::EnUs),
            "fr" | "french" => Some(Self::Fr),
            "de" | "german" => Some(Self::De),
            "ru" | "russian" => Some(Self::Ru),
            "ar" | "arabic" => Some(Self::ArEg),
            "vi" | "vietnamese" => Some(Self::Vi),
            "ja" | "japanese" => Some(Self::Ja),
            "ko" | "korean" => Some(Self::Ko),
            "ms" | "malay" => Some(Self::Ms),
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
            Self::Fr => "fr-FR",
            Self::De => "de-DE",
            Self::Ru => "ru-RU",
            Self::ArEg => "ar-EG",
            Self::Vi => "vi-VN",
            Self::Ja => "ja-JP",
            Self::Ko => "ko-KR",
            Self::Ms => "ms-MY",
        }
    }

    /// Fallback language used when the current table misses a key.
    ///
    /// `zh-TW` → `zh-CN`; every other translation falls back to English. English is the
    /// base and has no further fallback.
    pub fn fallback(self) -> Option<Self> {
        match self {
            Self::EnUs => None,
            Self::ZhCn => Some(Self::EnUs),
            Self::ZhTw => Some(Self::ZhCn),
            Self::Fr | Self::De | Self::Ru | Self::ArEg | Self::Vi | Self::Ja | Self::Ko | Self::Ms => {
                Some(Self::EnUs)
            }
        }
    }

    /// Whether this language is Chinese (Simplified or Traditional).
    pub fn is_chinese(self) -> bool {
        matches!(self, Self::ZhCn | Self::ZhTw)
    }

    /// Whether this language is written right-to-left.
    pub fn is_rtl(self) -> bool {
        matches!(self, Self::ArEg)
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::EnUs => "en-US.json",
            Self::ZhCn => "zh-CN.json",
            Self::ZhTw => "zh-TW.json",
            Self::Fr => "fr-FR.json",
            Self::De => "de-DE.json",
            Self::Ru => "ru-RU.json",
            Self::ArEg => "ar-EG.json",
            Self::Vi => "vi-VN.json",
            Self::Ja => "ja-JP.json",
            Self::Ko => "ko-KR.json",
            Self::Ms => "ms-MY.json",
        }
    }

    fn index(self) -> u8 {
        ALL.iter().position(|&lang| lang == self).unwrap_or(0) as u8
    }

    fn from_index(index: u8) -> Self {
        ALL.get(index as usize).copied().unwrap_or(Self::EnUs)
    }
}

const ZH_JSON: &str = include_str!("../../../i18n/zh-CN.json");
const ZH_TW_JSON: &str = include_str!("../../../i18n/zh-TW.json");
const EN_JSON: &str = include_str!("../../../i18n/en-US.json");
const FR_JSON: &str = include_str!("../../../i18n/fr-FR.json");
const DE_JSON: &str = include_str!("../../../i18n/de-DE.json");
const RU_JSON: &str = include_str!("../../../i18n/ru-RU.json");
const AR_JSON: &str = include_str!("../../../i18n/ar-EG.json");
const VI_JSON: &str = include_str!("../../../i18n/vi-VN.json");
const JA_JSON: &str = include_str!("../../../i18n/ja-JP.json");
const KO_JSON: &str = include_str!("../../../i18n/ko-KR.json");
const MS_JSON: &str = include_str!("../../../i18n/ms-MY.json");

static CURRENT: AtomicU8 = AtomicU8::new(0);
static ZH: OnceLock<HashMap<String, String>> = OnceLock::new();
static ZH_TW: OnceLock<HashMap<String, String>> = OnceLock::new();
static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
static FR: OnceLock<HashMap<String, String>> = OnceLock::new();
static DE: OnceLock<HashMap<String, String>> = OnceLock::new();
static RU: OnceLock<HashMap<String, String>> = OnceLock::new();
static AR: OnceLock<HashMap<String, String>> = OnceLock::new();
static VI: OnceLock<HashMap<String, String>> = OnceLock::new();
static JA: OnceLock<HashMap<String, String>> = OnceLock::new();
static KO: OnceLock<HashMap<String, String>> = OnceLock::new();
static MS: OnceLock<HashMap<String, String>> = OnceLock::new();

fn embedded(lang: Lang) -> &'static str {
    match lang {
        Lang::EnUs => EN_JSON,
        Lang::ZhCn => ZH_JSON,
        Lang::ZhTw => ZH_TW_JSON,
        Lang::Fr => FR_JSON,
        Lang::De => DE_JSON,
        Lang::Ru => RU_JSON,
        Lang::ArEg => AR_JSON,
        Lang::Vi => VI_JSON,
        Lang::Ja => JA_JSON,
        Lang::Ko => KO_JSON,
        Lang::Ms => MS_JSON,
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
        Lang::Fr => FR.get_or_init(|| load_table(Lang::Fr)),
        Lang::De => DE.get_or_init(|| load_table(Lang::De)),
        Lang::Ru => RU.get_or_init(|| load_table(Lang::Ru)),
        Lang::ArEg => AR.get_or_init(|| load_table(Lang::ArEg)),
        Lang::Vi => VI.get_or_init(|| load_table(Lang::Vi)),
        Lang::Ja => JA.get_or_init(|| load_table(Lang::Ja)),
        Lang::Ko => KO.get_or_init(|| load_table(Lang::Ko)),
        Lang::Ms => MS.get_or_init(|| load_table(Lang::Ms)),
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

/// All supported languages (English first).
pub fn all_languages() -> &'static [Lang] {
    &ALL
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
        assert_eq!(Lang::EnUs.fallback(), None);
        assert_eq!(Lang::ZhCn.fallback(), Some(Lang::EnUs));
        assert_eq!(Lang::ZhTw.fallback(), Some(Lang::ZhCn));
        for lang in ALL {
            if !lang.is_chinese() && lang != Lang::EnUs {
                assert_eq!(lang.fallback(), Some(Lang::EnUs));
            }
        }
        assert!(Lang::ZhTw.is_chinese() && Lang::ZhCn.is_chinese());
        assert!(!Lang::EnUs.is_chinese());
        assert!(Lang::ArEg.is_rtl());
        assert!(!Lang::Fr.is_rtl());
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
        assert_eq!(Lang::from_code("fr-FR"), Some(Lang::Fr));
        assert_eq!(Lang::from_code("fr_CA.UTF-8"), Some(Lang::Fr));
        assert_eq!(Lang::from_code("de-DE"), Some(Lang::De));
        assert_eq!(Lang::from_code("de_AT"), Some(Lang::De));
        assert_eq!(Lang::from_code("ru-RU"), Some(Lang::Ru));
        assert_eq!(Lang::from_code("ar-EG"), Some(Lang::ArEg));
        assert_eq!(Lang::from_code("ar"), Some(Lang::ArEg));
        assert_eq!(Lang::from_code("vi-VN"), Some(Lang::Vi));
        assert_eq!(Lang::from_code("ja-JP"), Some(Lang::Ja));
        assert_eq!(Lang::from_code("ko-KR"), Some(Lang::Ko));
        assert_eq!(Lang::from_code("ms-MY"), Some(Lang::Ms));
        assert_eq!(Lang::from_code("fr"), Some(Lang::Fr));
        assert_eq!(Lang::from_code("xx-YY"), None);
        assert_eq!(Lang::from_code("C"), None);
    }

    #[test]
    fn language_codes_and_files_are_unique() {
        let codes: BTreeSet<&str> = ALL.iter().map(|lang| lang.code()).collect();
        let files: BTreeSet<&str> = ALL.iter().map(|lang| lang.file_name()).collect();
        assert_eq!(codes.len(), ALL.len());
        assert_eq!(files.len(), ALL.len());
        for lang in ALL {
            assert!(looks_like_table(Path::new(lang.file_name())));
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
            assert_eq!(Lang::from_index(lang.index()), lang);
        }
    }

    #[test]
    fn translations_are_subsets_of_the_english_base_with_same_placeholders() {
        let english = table(EN_JSON);
        assert!(english.len() > 300, "English base looks empty: {}", english.len());
        let locales: [(&str, &str); 10] = [
            ("zh-CN", ZH_JSON),
            ("zh-TW", ZH_TW_JSON),
            ("fr-FR", FR_JSON),
            ("de-DE", DE_JSON),
            ("ru-RU", RU_JSON),
            ("ar-EG", AR_JSON),
            ("vi-VN", VI_JSON),
            ("ja-JP", JA_JSON),
            ("ko-KR", KO_JSON),
            ("ms-MY", MS_JSON),
        ];
        for (locale, json) in locales {
            let translation = table(json);
            assert!(
                translation.keys().all(|key| english.contains_key(key)),
                "{locale}.json contains keys missing from en-US.json"
            );
            for (key, value) in &translation {
                if let Some(base) = english.get(key) {
                    assert_eq!(
                        placeholders(base),
                        placeholders(value),
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
        set_language(Lang::Fr);
        assert_eq!(current_language(), Lang::Fr);
    }
}
