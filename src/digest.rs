//! The daily digest: one short system notification that restates yesterday's
//! advice with a greeting.
//!
//! It is deliberately not a task list. The text is built locally, is the same
//! for the whole day, and is skipped when yesterday had no activity at all.

use crate::{
    i18n::{Lang, Text, fill},
    tr,
};
use anyhow::Result;
use chrono::{DateTime, Local, Timelike};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

/// Stable per day, so repeated checks produce identical text.
const OPENERS: [Text; 4] = [
    Text::new(
        "昨天的工作已经整理好了。",
        "Yesterday's work is ready to review.",
        "昨日の作業をまとめました。",
        "어제 작업을 정리했습니다.",
    ),
    Text::new(
        "这是昨天的记录得出的结论。",
        "Here is what yesterday's records show.",
        "昨日の記録から得られた結論です。",
        "어제 기록에서 나온 결론입니다.",
    ),
    Text::new(
        "昨天的时间主要花在这些地方。",
        "Here is where yesterday's time went.",
        "昨日の時間は主にここに使われました。",
        "어제 시간은 주로 여기에 쓰였습니다.",
    ),
    Text::new(
        "看看昨天有没有更省力的做法。",
        "See whether yesterday had an easier way.",
        "昨日にもっと楽なやり方がなかったか見てみましょう。",
        "어제 더 편한 방법이 있었는지 확인해 보세요.",
    ),
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct DigestState {
    /// Local date (`YYYY-MM-DD`) whose digest was already delivered.
    pub last_sent: Option<String>,
}

impl DigestState {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}

pub fn greeting(now: DateTime<Local>, lang: Lang) -> &'static str {
    match now.hour() {
        5..=8 => tr!(
            lang,
            "早上好",
            "Good morning",
            "おはようございます",
            "좋은 아침입니다"
        ),
        9..=11 => tr!(
            lang,
            "上午好",
            "Good morning",
            "おはようございます",
            "좋은 아침입니다"
        ),
        12..=13 => tr!(lang, "中午好", "Good afternoon", "こんにちは", "안녕하세요"),
        14..=17 => tr!(
            lang,
            "下午好",
            "Good afternoon",
            "こんにちは",
            "좋은 오후입니다"
        ),
        18..=22 => tr!(
            lang,
            "晚上好",
            "Good evening",
            "こんばんは",
            "좋은 저녁입니다"
        ),
        _ => tr!(lang, "夜深了", "Late night", "夜更けです", "늦은 밤입니다"),
    }
}

fn opener(now: DateTime<Local>, lang: Lang) -> &'static str {
    let seed = now
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
        .bytes()
        .map(u32::from)
        .sum::<u32>() as usize;
    OPENERS[seed % OPENERS.len()].get(lang)
}

/// Everything the digest needs from yesterday's report.
pub struct Digest<'a> {
    pub headline: &'a str,
    pub top_upgrade: &'a str,
    pub upgrades: usize,
    pub has_report: bool,
    /// Yesterday had enough activity to be worth mentioning.
    pub has_activity: bool,
}

/// Returns the notification title and body, or `None` when there is nothing
/// worth sending.
pub fn compose(
    now: DateTime<Local>,
    digest: Option<Digest<'_>>,
    lang: Lang,
) -> Option<(String, String)> {
    let digest = digest?;
    let title = format!("{} · TraceBit", greeting(now, lang));
    let opener = opener(now, lang);
    if digest.has_report {
        let headline = truncate(digest.headline.trim(), 48);
        let top = truncate(digest.top_upgrade.trim(), 20);
        let mut body = if headline.is_empty() {
            format!(
                "{opener}{}",
                tr!(
                    lang,
                    "昨天的工作已经整理好了。",
                    "Yesterday's work is ready.",
                    "昨日の作業は整理済みです。",
                    "어제 작업이 정리되었습니다."
                )
            )
        } else {
            fill(
                tr!(
                    lang,
                    "{0}昨天：{1}。",
                    "{0} Yesterday: {1}.",
                    "{0}昨日：{1}。",
                    "{0} 어제: {1}."
                ),
                &[&opener, &headline],
            )
        };
        if !top.is_empty() {
            body.push_str(&fill(
                tr!(
                    lang,
                    "{0} 个工具升级机会，例如「{1}」。",
                    "{0} tool upgrade opportunities, e.g. “{1}”.",
                    "ツール改善の提案が {0} 件、例えば「{1}」。",
                    "도구 개선 제안 {0}건, 예: “{1}”."
                ),
                &[&digest.upgrades.max(1), &top],
            ));
        }
        body.push_str(tr!(
            lang,
            "打开 TraceBit 查看完整报告。",
            "Open TraceBit to see the full report.",
            "TraceBit を開いてレポート全体を確認できます。",
            "TraceBit을 열어 전체 보고서를 확인하세요."
        ));
        Some((title, body))
    } else if digest.has_activity {
        Some((
            title,
            fill(
                tr!(
                    lang,
                    "{0}昨天的记录已就绪。打开 TraceBit 生成工具升级建议。",
                    "{0} Yesterday's records are ready. Open TraceBit to generate tool upgrade advice.",
                    "{0}昨日の記録ができました。TraceBit を開いてツール改善の提案を生成してください。",
                    "{0} 어제 기록이 준비되었습니다. TraceBit을 열어 도구 개선 제안을 생성하세요."
                ),
                &[&opener],
            ),
        ))
    } else {
        None
    }
}

/// True when today's digest time has passed and today has not been delivered.
pub fn is_due(now_ms: i64, scheduled_ms: i64, last_sent: Option<&str>, today: &str) -> bool {
    now_ms >= scheduled_ms && last_sent != Some(today)
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    value
        .chars()
        .take(max.saturating_sub(1))
        .chain(std::iter::once('…'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32, minute: u32) -> DateTime<Local> {
        let date = Local::now().date_naive();
        Local
            .from_local_datetime(&date.and_hms_opt(hour, minute, 0).unwrap())
            .single()
            .unwrap()
    }

    #[test]
    fn greetings_follow_time_of_day() {
        assert_eq!(greeting(at(7, 0), Lang::Zh), "早上好");
        assert_eq!(greeting(at(10, 30), Lang::Zh), "上午好");
        assert_eq!(greeting(at(12, 30), Lang::Zh), "中午好");
        assert_eq!(greeting(at(16, 0), Lang::Zh), "下午好");
        assert_eq!(greeting(at(20, 0), Lang::Zh), "晚上好");
        assert_eq!(greeting(at(2, 0), Lang::Zh), "夜深了");
    }

    #[test]
    fn digest_restates_yesterdays_advice() {
        let now = at(10, 30);
        let (title, body) = compose(
            now,
            Some(Digest {
                headline: "你在编辑器和终端之间完成开发",
                top_upgrade: "用 lazygit 处理提交",
                upgrades: 2,
                has_report: true,
                has_activity: true,
            }),
            Lang::Zh,
        )
        .unwrap();
        assert_eq!(title, "上午好 · TraceBit");
        assert!(body.contains("你在编辑器和终端之间完成开发"));
        assert!(body.contains("2 个工具升级机会"));
        assert!(body.contains("用 lazygit 处理提交"));
        assert!(body.contains("打开 TraceBit"));
    }

    #[test]
    fn digest_follows_the_interface_language() {
        let (title, body) = compose(
            at(10, 30),
            Some(Digest {
                headline: "You move between the editor and the terminal",
                top_upgrade: "Handle commits with lazygit",
                upgrades: 2,
                has_report: true,
                has_activity: true,
            }),
            Lang::En,
        )
        .unwrap();
        assert_eq!(title, "Good morning · TraceBit");
        assert!(body.contains("2 tool upgrade opportunities"));
        assert!(body.contains("Open TraceBit"));
    }

    #[test]
    fn digest_is_stable_for_the_same_day() {
        let now = at(10, 30);
        let digest = || Digest {
            headline: "固定结论",
            top_upgrade: "固定工具",
            upgrades: 1,
            has_report: true,
            has_activity: true,
        };
        let first = compose(now, Some(digest()), Lang::Zh).unwrap();
        let second = compose(now, Some(digest()), Lang::Zh).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn digest_skips_empty_days_and_falls_back_to_activity() {
        assert!(
            compose(
                at(10, 30),
                Some(Digest {
                    headline: "",
                    top_upgrade: "",
                    upgrades: 0,
                    has_report: false,
                    has_activity: false,
                }),
                Lang::Zh,
            )
            .is_none()
        );
        let (_, body) = compose(
            at(10, 30),
            Some(Digest {
                headline: "",
                top_upgrade: "",
                upgrades: 0,
                has_report: false,
                has_activity: true,
            }),
            Lang::Zh,
        )
        .unwrap();
        assert!(body.contains("记录已就绪"));
    }

    #[test]
    fn due_once_per_day_after_the_time() {
        let today = "2026-11-14";
        assert!(!is_due(999, 1000, None, today));
        assert!(is_due(1000, 1000, None, today));
        assert!(!is_due(2000, 1000, Some(today), today));
        assert!(is_due(2000, 1000, Some("2026-11-13"), today));
    }

    #[test]
    fn state_roundtrips() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("digest.json");
        let mut state = DigestState::load(&path);
        assert!(state.last_sent.is_none());
        state.last_sent = Some("2026-11-14".to_owned());
        state.save(&path).unwrap();
        assert_eq!(DigestState::load(&path), state);
    }
}
