//! # neo-i18n — NEO 界面文本
//!
//! 所有用户可见文本集中存放在仓库根目录的 `i18n/` 目录：
//!
//! * `i18n/zh-CN.json` — 简体中文（默认，也是 `zh` 的回退表）
//! * `i18n/zh-TW.json` — 繁體中文（臺灣）
//! * `i18n/en-US.json` — English
//!
//! 语言解析：`zh`（不区分地区）按回退规则归到简体中文；`zh-TW`/`zh-Hant` 指向繁體。
//! 某语言缺 key 时按 [`Lang::fallback`] 回退（zh-TW → zh，即简体表；英文缺 key 也回退
//! 到简体），再缺则返回 key 本身（便于发现漏翻）。
//!
//! 启动时优先读取可执行文件旁的 `i18n/*.json`；缺失或损坏时回退到编译期嵌入的副本。
//! 使用 [`t`] 取单条文本，[`tf`] 支持 `{name}` 占位符。
//!
//! 语言可在运行时切换：`NEO_LANG=zh-TW` 环境变量，或界面「语言 / Language」菜单。
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// 支持的语言。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lang {
    /// 简体中文（默认；也是 `zh` 与其它语言的回退表）。
    #[default]
    ZhCn,
    /// 繁體中文（臺灣）。
    ZhTw,
    /// English。
    EnUs,
}

impl Lang {
    /// 从语言代码解析（`zh` / `zh-CN` / `zh-TW` / `zh-Hant` / `en-US` / `en`）
    ///。
    /// `zh` 不带地区时按回退规则归到简体中文；下划线写法同样接受。
    pub fn from_code(code: &str) -> Option<Self> {
        match code.trim().to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" | "zh_cn" | "cn" | "chinese" | "zh-hans" | "zh_hans" => Some(Self::ZhCn),
            "zh-tw" | "zh_tw" | "tw" | "zh-hk" | "zh_hk" | "zh-hant" | "zh_hant" | "traditional" => {
                Some(Self::ZhTw)
            }
            "en" | "en-us" | "en_us" | "english" => Some(Self::EnUs),
            _ => None,
        }
    }

    /// 语言代码（写回设置 / 展示用）。
    pub fn code(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::ZhTw => "zh-TW",
            Self::EnUs => "en-US",
        }
    }

    /// 缺 key 时的回退语言。
    ///
    /// 繁體与英文都回退到简体（`zh`）；简体本身没有更底的回退。
    pub fn fallback(self) -> Option<Self> {
        match self {
            Self::ZhCn => None,
            Self::ZhTw | Self::EnUs => Some(Self::ZhCn),
        }
    }

    /// 是否为中文（简体或繁體）。
    pub fn is_chinese(self) -> bool {
        matches!(self, Self::ZhCn | Self::ZhTw)
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN.json",
            Self::ZhTw => "zh-TW.json",
            Self::EnUs => "en-US.json",
        }
    }

    fn index(self) -> u8 {
        match self {
            Self::ZhCn => 0,
            Self::ZhTw => 1,
            Self::EnUs => 2,
        }
    }

    fn from_index(index: u8) -> Self {
        match index {
            2 => Self::EnUs,
            1 => Self::ZhTw,
            _ => Self::ZhCn,
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
        Lang::ZhCn => ZH_JSON,
        Lang::ZhTw => ZH_TW_JSON,
        Lang::EnUs => EN_JSON,
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
        // 开发时从 out/neo-desktop/... 或 out/cargo/... 回退到仓库根。
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
        Lang::ZhCn => ZH.get_or_init(|| load_table(Lang::ZhCn)),
        Lang::ZhTw => ZH_TW.get_or_init(|| load_table(Lang::ZhTw)),
        Lang::EnUs => EN.get_or_init(|| load_table(Lang::EnUs)),
    }
}

/// 当前语言。
pub fn current_language() -> Lang {
    Lang::from_index(CURRENT.load(Ordering::Relaxed))
}

/// 设置当前语言（立即生效；面板每帧重新取文本）。
pub fn set_language(lang: Lang) {
    CURRENT.store(lang.index(), Ordering::Relaxed);
}

/// 按 `NEO_LANG` 环境变量初始化（无该变量时保持默认中文）。
pub fn init_from_env() {
    if let Ok(code) = std::env::var("NEO_LANG")
        && let Some(lang) = Lang::from_code(&code)
    {
        set_language(lang);
    }
}

/// 取一条界面文本；缺失时按 [`Lang::fallback`] 回退，再缺则返回键名。
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

/// 带 `{name}` 占位符的文本替换。
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

/// 诊断：返回将尝试的 i18n 目录（测试用）。
pub fn search_dirs_for_test() -> Vec<PathBuf> {
    i18n_search_dirs()
}

/// 判断路径是否像语言表。
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
    fn chinese_variants_are_parsed_and_zh_is_the_fallback() {
        assert_eq!(Lang::from_code("zh"), Some(Lang::ZhCn));
        assert_eq!(Lang::from_code("zh_CN"), Some(Lang::ZhCn));
        assert_eq!(Lang::from_code("zh-TW"), Some(Lang::ZhTw));
        assert_eq!(Lang::from_code("zh_tw"), Some(Lang::ZhTw));
        assert_eq!(Lang::from_code("zh-Hant"), Some(Lang::ZhTw));
        assert_eq!(Lang::from_code("zh-Hans"), Some(Lang::ZhCn));
        assert_eq!(Lang::from_code("en-US"), Some(Lang::EnUs));
        assert_eq!(Lang::from_code("fr"), None);
        // 繁體与英文缺失时都回退到 zh（简体表）。
        assert_eq!(Lang::ZhTw.fallback(), Some(Lang::ZhCn));
        assert_eq!(Lang::EnUs.fallback(), Some(Lang::ZhCn));
        assert_eq!(Lang::ZhCn.fallback(), None);
        assert!(Lang::ZhTw.is_chinese() && Lang::ZhCn.is_chinese());
        assert!(!Lang::EnUs.is_chinese());
    }

    #[test]
    fn language_codes_and_files_are_unique() {
        let langs = [Lang::ZhCn, Lang::ZhTw, Lang::EnUs];
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
    fn traditional_chinese_covers_every_simplified_key_with_same_placeholders() {
        let simplified = table(ZH_JSON);
        let traditional = table(ZH_TW_JSON);
        assert!(simplified.len() > 300, "简体表异常：{} 条", simplified.len());
        assert_eq!(
            simplified.keys().collect::<BTreeSet<_>>(),
            traditional.keys().collect::<BTreeSet<_>>(),
            "zh-TW.json 的 key 集合必须与 zh-CN.json 完全一致"
        );
        for (key, value) in &simplified {
            let other = &traditional[key];
            assert_eq!(
                placeholders(value),
                placeholders(other),
                "`{key}` 的占位符在简体/繁體之间不一致"
            );
        }
    }

    #[test]
    fn english_covers_every_simplified_key() {
        let simplified = table(ZH_JSON);
        let english = table(EN_JSON);
        assert_eq!(
            simplified.keys().collect::<BTreeSet<_>>(),
            english.keys().collect::<BTreeSet<_>>(),
            "en-US.json 的 key 集合必须与 zh-CN.json 完全一致"
        );
    }

    #[test]
    fn switching_to_traditional_uses_the_traditional_table() {
        set_language(Lang::ZhTw);
        assert_eq!(current_language(), Lang::ZhTw);
        assert_eq!(t("menu.file"), "檔案");
        set_language(Lang::ZhCn);
        assert_eq!(current_language(), Lang::ZhCn);
        assert_eq!(t("menu.file"), "文件");
    }
}
