//! OS locale detection for the initial UI language.
//!
//! NEO is English-first: English is the default UI language and the source language of
//! the i18n tables. At startup the app asks the operating system for its UI locale and
//! switches to Simplified/Traditional Chinese when the system locale matches a shipped
//! translation, otherwise it stays on English. `NEO_LANG` always wins.

use neo_i18n::Lang;

/// Language to use for this run: `NEO_LANG`, then the OS locale, then English.
pub(crate) fn startup_language() -> Lang {
    if let Ok(code) = std::env::var("NEO_LANG")
        && let Some(lang) = Lang::from_code(&code)
    {
        return lang;
    }
    if let Some(locale) = os_locale()
        && let Some(lang) = Lang::from_code(&locale)
    {
        return lang;
    }
    if let Some(lang) = neo_i18n::language_from_env() {
        return lang;
    }
    Lang::default()
}

/// The operating system's preferred UI locale, if it can be read.
fn os_locale() -> Option<String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
        // LOCALE_NAME_MAX_LENGTH is 85 on Windows.
        let mut buffer = [0u16; 85];
        // SAFETY: `buffer` is a valid writable array of `u16` and the API is told its length;
        // it writes at most that many UTF-16 code units including the terminating NUL.
        let written = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
        if written <= 0 {
            None
        } else {
            let end = (written as usize).saturating_sub(1);
            Some(String::from_utf16_lossy(&buffer[..end]))
        }
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "macos")]
        {
            // Finder-launched apps do not inherit POSIX locale variables.
            if let Ok(output) = std::process::Command::new("defaults")
                .args(["read", "-g", "AppleLocale"])
                .output()
                && output.status.success()
            {
                let value = String::from_utf8_lossy(&output.stdout);
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_owned());
                }
            }
        }
        None
    }
}
