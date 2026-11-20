//! UI languages and the tiny translation helpers used across the app.
//!
//! Every user-facing literal is written as `tr!(lang, "中文", "English", "日本語",
//! "한국어")` at the call site, and content tables use [`Text`]. There is no
//! runtime catalogue to keep in sync, so a missing translation cannot compile.
//!
//! Canonical data (category ids, stored window titles, profile roles and report
//! text) stays in the language it was recorded in; display code maps the known
//! canonical values through [`crate::categories::label`] and friends instead of
//! rewriting stored data.

use std::fmt::Display;

use serde::{Deserialize, Serialize};

/// Languages the interface ships with. `Zh` is the historical default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    #[default]
    Zh,
    En,
    Ja,
    Ko,
}

impl Lang {
    pub const ALL: [Lang; 4] = [Lang::Zh, Lang::En, Lang::Ja, Lang::Ko];

    /// Stable code persisted in `settings.json`.
    pub fn code(self) -> &'static str {
        match self {
            Self::Zh => "zh",
            Self::En => "en",
            Self::Ja => "ja",
            Self::Ko => "ko",
        }
    }

    /// Name shown in the language picker, written in that language.
    pub fn label(self) -> &'static str {
        match self {
            Self::Zh => "简体中文",
            Self::En => "English",
            Self::Ja => "日本語",
            Self::Ko => "한국어",
        }
    }

    /// Language name as it is written in the *current* language, for prompts.
    pub fn name(self) -> &'static str {
        match self {
            Self::Zh => "Simplified Chinese",
            Self::En => "English",
            Self::Ja => "Japanese",
            Self::Ko => "Korean",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Zh => 0,
            Self::En => 1,
            Self::Ja => 2,
            Self::Ko => 3,
        }
    }

    /// Accepts `en`, `en-US`, `zh-CN`, `ja`, `ko` and ignores case.
    pub fn from_code(code: &str) -> Option<Self> {
        let primary = code.trim().split(['-', '_']).next()?.to_ascii_lowercase();
        match primary.as_str() {
            "zh" => Some(Self::Zh),
            "en" => Some(Self::En),
            "ja" => Some(Self::Ja),
            "ko" => Some(Self::Ko),
            _ => None,
        }
    }

    /// `setting` is the persisted `app.language`; empty means "follow Windows".
    pub fn resolve(setting: &str) -> Self {
        Self::from_code(setting).unwrap_or_else(Self::detect)
    }

    /// Windows UI language first, then the POSIX locale, then Chinese.
    pub fn detect() -> Self {
        if let Ok(value) = std::env::var("TRACEBIT_LANG")
            && let Some(lang) = Self::from_code(&value)
        {
            return lang;
        }
        if let Some(lang) = Self::from_langid(windows_ui_language()) {
            return lang;
        }
        for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(value) = std::env::var(key)
                && let Some(lang) = Self::from_code(&value)
            {
                return lang;
            }
        }
        Self::Zh
    }

    /// Primary language id of a Windows LANGID: zh 0x04, en 0x09, ja 0x11,
    /// ko 0x12. Unknown languages resolve to `None`.
    pub fn from_langid(langid: u16) -> Option<Self> {
        match langid & 0x3ff {
            0x04 => Some(Self::Zh),
            0x09 => Some(Self::En),
            0x11 => Some(Self::Ja),
            0x12 => Some(Self::Ko),
            _ => None,
        }
    }
}

#[cfg(windows)]
fn windows_ui_language() -> u16 {
    // GetUserDefaultUILanguage returns the user's display language; the low ten
    // bits are the primary language, which is all we map.
    unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() }
}

#[cfg(not(windows))]
fn windows_ui_language() -> u16 {
    0
}

/// Four translations of one string, picked with [`Lang`].
#[derive(Debug, Clone, Copy)]
pub struct Text {
    pub zh: &'static str,
    pub en: &'static str,
    pub ja: &'static str,
    pub ko: &'static str,
}

impl Text {
    pub const fn new(
        zh: &'static str,
        en: &'static str,
        ja: &'static str,
        ko: &'static str,
    ) -> Self {
        Self { zh, en, ja, ko }
    }

    pub fn get(self, lang: Lang) -> &'static str {
        pick(lang, [self.zh, self.en, self.ja, self.ko])
    }

    pub fn fill(self, lang: Lang, args: &[&dyn Display]) -> String {
        fill(self.get(lang), args)
    }
}

/// Selects one of four translations. Keep the order `zh, en, ja, ko`.
pub fn pick(lang: Lang, texts: [&'static str; 4]) -> &'static str {
    texts[lang.index()]
}

/// Four translations at the call site. Keeps the source Chinese readable in
/// diffs; the other three languages sit next to it.
///
/// ```ignore
/// div().child(tr!(lang, "设置", "Settings", "設定", "설정"))
/// ```
#[macro_export]
macro_rules! tr {
    ($lang:expr, $zh:expr, $en:expr, $ja:expr, $ko:expr $(,)?) => {
        $crate::i18n::pick($lang, [$zh, $en, $ja, $ko])
    };
}

/// Replaces `{}` placeholders in order and `{0}`, `{1}` by position. `{{` and
/// `}}` are literal braces. Missing arguments leave the placeholder in place so
/// a mistake shows up in the UI instead of panicking.
pub fn fill(pattern: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut chars = pattern.chars().peekable();
    let mut next = 0usize;
    while let Some(ch) = chars.next() {
        match ch {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut index = String::new();
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == '}' {
                        closed = true;
                        break;
                    }
                    index.push(inner);
                }
                if !closed {
                    out.push('{');
                    out.push_str(&index);
                    continue;
                }
                let slot = if index.trim().is_empty() {
                    let slot = next;
                    next += 1;
                    Some(slot)
                } else {
                    index.trim().parse::<usize>().ok()
                };
                match slot.and_then(|slot| args.get(slot)) {
                    Some(arg) => out.push_str(&arg.to_string()),
                    None => {
                        out.push('{');
                        out.push_str(&index);
                        out.push('}');
                    }
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_accept_regions() {
        for lang in Lang::ALL {
            assert_eq!(Lang::from_code(lang.code()), Some(lang));
        }
        assert_eq!(Lang::from_code("en-US"), Some(Lang::En));
        assert_eq!(Lang::from_code("ZH_cn"), Some(Lang::Zh));
        assert_eq!(Lang::from_code("ja_JP"), Some(Lang::Ja));
        assert_eq!(Lang::from_code("fr"), None);
        assert_eq!(Lang::from_code(""), None);
    }

    #[test]
    fn windows_langids_map_to_supported_languages() {
        assert_eq!(Lang::from_langid(0x0804), Some(Lang::Zh));
        assert_eq!(Lang::from_langid(0x0409), Some(Lang::En));
        assert_eq!(Lang::from_langid(0x0411), Some(Lang::Ja));
        assert_eq!(Lang::from_langid(0x0412), Some(Lang::Ko));
        assert_eq!(Lang::from_langid(0x0407), None);
    }

    #[test]
    fn pick_and_text_follow_the_language() {
        assert_eq!(pick(Lang::En, ["中", "en", "日", "한"]), "en");
        let text = Text::new("中", "en", "日", "한");
        assert_eq!(text.get(Lang::Ko), "한");
    }

    #[test]
    fn fill_supports_ordered_positional_and_escaped_braces() {
        assert_eq!(
            fill("{} 天 · {} 至 {}", &[&7, &"1 日", &"7 日"]),
            "7 天 · 1 日 至 7 日"
        );
        assert_eq!(fill("{1} and {0}", &[&"a", &"b"]), "b and a");
        assert_eq!(fill("literal {{}} {}", &[&1]), "literal {} 1");
        assert_eq!(fill("keep {}", &[]), "keep {}");
    }
}
