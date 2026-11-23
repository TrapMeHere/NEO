//! # neo-i18n — NEO 界面文本
//!
//! 所有用户可见文本集中存放在仓库根目录的 `i18n/` 目录：
//!
//! * `i18n/zh-CN.json` — 简体中文（默认）
//! * `i18n/en-US.json` — English
//!
//! 启动时优先读取可执行文件旁的 `i18n/*.json`；缺失或损坏时回退到编译期嵌入的副本。
//! 使用 [`t`] 取单条文本，[`tf`] 支持 `{name}` 占位符；缺失键回退到默认语言，
//! 再缺失则返回键名本身（便于发现漏翻）。
//!
//! 语言可在运行时切换：`NEO_LANG=en-US` 环境变量，或界面「语言 / Language」菜单。

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// 支持的语言。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lang {
    /// 简体中文（默认）。
    #[default]
    ZhCn,
    /// English。
    EnUs,
}

impl Lang {
    /// 从环境变量值解析（`zh-CN` / `zh` / `en-US` / `en`）。
    pub fn from_code(code: &str) -> Option<Self> {
        match code.trim().to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" | "zh_cn" | "cn" | "chinese" => Some(Self::ZhCn),
            "en" | "en-us" | "en_us" | "english" => Some(Self::EnUs),
            _ => None,
        }
    }

    /// 语言代码。
    pub fn code(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::EnUs => "en-US",
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN.json",
            Self::EnUs => "en-US.json",
        }
    }

    fn index(self) -> u8 {
        match self {
            Self::ZhCn => 0,
            Self::EnUs => 1,
        }
    }
}

const ZH_JSON: &str = include_str!("../../../i18n/zh-CN.json");
const EN_JSON: &str = include_str!("../../../i18n/en-US.json");

static CURRENT: AtomicU8 = AtomicU8::new(0);
static ZH: OnceLock<HashMap<String, String>> = OnceLock::new();
static EN: OnceLock<HashMap<String, String>> = OnceLock::new();

fn embedded(lang: Lang) -> &'static str {
    match lang {
        Lang::ZhCn => ZH_JSON,
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
        Lang::EnUs => EN.get_or_init(|| load_table(Lang::EnUs)),
    }
}

/// 当前语言。
pub fn current_language() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        0 => Lang::ZhCn,
        _ => Lang::EnUs,
    }
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

/// 取一条界面文本；缺失时回退到默认语言，再缺失则返回键名。
pub fn t(key: &'static str) -> &'static str {
    if let Some(value) = catalog(current_language()).get(key) {
        return value.as_str();
    }
    if let Some(value) = catalog(Lang::ZhCn).get(key) {
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
