//! Redaction applied to every piece of text that leaves the machine.

use crate::{categories::is_communication, recorder::SENSITIVE_TITLE};
use regex::Regex;
use std::sync::LazyLock;

pub const CHAT_TITLE: &str = "[聊天窗口]";

/// Removes common personal data and truncates to `max` characters.
pub fn redact_with_limit(value: &str, max: usize) -> String {
    static URL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\b(?:https?|ftp)://([a-z0-9.-]+)[^\s]*").unwrap());
    static EMAIL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[\w.+-]+@[\w-]+\.[\w.]+").unwrap());
    static KEY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"\b(?:sk-[A-Za-z0-9_-]{8,}|ghp_[A-Za-z0-9]{8,}|github_pat_[A-Za-z0-9_]{8,}|xox[bap]-[A-Za-z0-9-]{8,}|AKIA[0-9A-Z]{12,}|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]+)",
        )
        .unwrap()
    });
    static ID_CARD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{17}[\dXx]\b").unwrap());
    static CARD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{16,19}\b").unwrap());
    static PHONE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?:\+?86[-\s]?)?\b1[3-9]\d{9}\b").unwrap());
    static IPV4: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(?:25[0-5]|2[0-4]\d|1?\d?\d)(?:\.(?:25[0-5]|2[0-4]\d|1?\d?\d)){3}\b")
            .unwrap()
    });
    static USER_PATH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)([a-z]:\\users\\)[^\\/\s]+").unwrap());
    static HOME_PATH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(/home/|/Users/)[^/\s]+").unwrap());

    let value = URL.replace_all(value, "[链接:$1]");
    let value = EMAIL.replace_all(&value, "[邮箱]");
    let value = KEY.replace_all(&value, "[密钥]");
    let value = ID_CARD.replace_all(&value, "[证件号]");
    let value = CARD.replace_all(&value, "[号码]");
    let value = PHONE.replace_all(&value, "[手机号]");
    let value = IPV4.replace_all(&value, "[IP]");
    let value = USER_PATH.replace_all(&value, "${1}[用户]");
    let value = HOME_PATH.replace_all(&value, "${1}[用户]");
    let chars = value.chars().count();
    let mut result = value.chars().take(max).collect::<String>();
    if chars > max {
        result.push('…');
    }
    result
}

pub fn redact_text(value: &str) -> String {
    redact_with_limit(value, 120)
}

/// Strips window-title noise that carries no information about the task:
/// unread counters, "and N more pages", browser profile and app-name suffixes.
pub fn clean_title(title: &str, app: &str) -> String {
    static COUNTER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*[(（][^()（）]{0,30}[)）]\s*").unwrap());
    static MORE_PAGES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\s*(和另外\s*\d+\s*个页面|and \d+ more pages?).*$").unwrap());
    let normalize = |value: &str| {
        value
            .chars()
            .filter(|ch| !matches!(ch, '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{feff}'))
            .collect::<String>()
            .trim()
            .to_lowercase()
    };
    let app = normalize(app);
    let title = COUNTER.replace(title, "");
    let title = MORE_PAGES.replace(&title, "");
    let mut parts = title
        .split(" - ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    while parts.len() > 1 {
        let last = normalize(parts[parts.len() - 1]);
        let noise = last == app
            || (!last.is_empty() && app.contains(&last))
            || matches!(last.as_str(), "个人" | "personal" | "工作" | "work")
            || last.starts_with("microsoft edge")
            || last.starts_with("google chrome")
            || last.starts_with("mozilla firefox");
        if !noise {
            break;
        }
        parts.pop();
    }
    let cleaned = parts.join(" - ");
    if normalize(&cleaned) == app {
        String::new()
    } else {
        cleaned
    }
}

/// Title as it may be shown to an AI model. Chat window titles name the person
/// or group being talked to, so communication apps only reveal the app.
pub fn ai_title(category: &str, title: &str) -> String {
    if title == SENSITIVE_TITLE {
        return SENSITIVE_TITLE.to_owned();
    }
    if is_communication(category) {
        return CHAT_TITLE.to_owned();
    }
    redact_with_limit(title, 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_covers_personal_data() {
        let value = redact_with_limit(
            "mail secret@example.com key sk-1234567890abcdef card 1234567890123456 \
             phone 13812345678 id 11010519491231002X ip 192.168.1.20 \
             C:\\Users\\zhangsan\\docs https://example.com/a?token=x",
            500,
        );
        for leaked in [
            "secret@example.com",
            "sk-1234567890abcdef",
            "1234567890123456",
            "13812345678",
            "11010519491231002X",
            "192.168.1.20",
            "zhangsan",
            "token=x",
        ] {
            assert!(!value.contains(leaked), "{leaked} leaked: {value}");
        }
        assert!(value.contains("[链接:example.com]"));
    }

    #[test]
    fn title_noise_is_removed() {
        assert_eq!(
            clean_title(
                "(99+ 封私信 / 80 条消息) B站有哪些up主？ - 知乎 和另外 8 个页面 - 个人 - Microsoft\u{200b} Edge",
                "Microsoft Edge"
            ),
            "B站有哪些up主？ - 知乎"
        );
        assert_eq!(clean_title("T3 Code (Nightly)", "T3 Code (Nightly)"), "");
        assert_eq!(
            clean_title(
                "main.rs - TraceBit - Visual Studio Code",
                "Visual Studio Code"
            ),
            "main.rs - TraceBit"
        );
    }

    #[test]
    fn chat_titles_are_hidden() {
        assert_eq!(ai_title("即时通讯", "张三"), CHAT_TITLE);
        assert_eq!(ai_title("开发", "main.rs - TraceBit"), "main.rs - TraceBit");
    }
}
