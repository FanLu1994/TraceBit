//! The daily digest: one short system notification that restates yesterday's
//! advice with a greeting.
//!
//! It is deliberately not a task list. The text is built locally, is the same
//! for the whole day, and is skipped when yesterday had no activity at all.

use anyhow::Result;
use chrono::{DateTime, Local, Timelike};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

/// Stable per day, so repeated checks produce identical text.
const OPENERS: [&str; 4] = [
    "昨天的工作已经整理好了。",
    "这是昨天的记录得出的结论。",
    "昨天的时间主要花在这些地方。",
    "看看昨天有没有更省力的做法。",
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

pub fn greeting(now: DateTime<Local>) -> &'static str {
    match now.hour() {
        5..=8 => "早上好",
        9..=11 => "上午好",
        12..=13 => "中午好",
        14..=17 => "下午好",
        18..=22 => "晚上好",
        _ => "夜深了",
    }
}

fn opener(now: DateTime<Local>) -> &'static str {
    let seed = now
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
        .bytes()
        .map(u32::from)
        .sum::<u32>() as usize;
    OPENERS[seed % OPENERS.len()]
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
pub fn compose(now: DateTime<Local>, digest: Option<Digest<'_>>) -> Option<(String, String)> {
    let digest = digest?;
    let title = format!("{} · TraceBit", greeting(now));
    let opener = opener(now);
    if digest.has_report {
        let headline = truncate(digest.headline.trim(), 48);
        let top = truncate(digest.top_upgrade.trim(), 20);
        let mut body = if headline.is_empty() {
            format!("{opener}昨天的工作已经整理好了。")
        } else {
            format!("{opener}昨天：{headline}。")
        };
        if !top.is_empty() {
            body.push_str(&format!(
                "{} 个工具升级机会，例如「{top}」。",
                digest.upgrades.max(1)
            ));
        }
        body.push_str("打开 TraceBit 查看完整报告。");
        Some((title, body))
    } else if digest.has_activity {
        Some((
            title,
            format!("{opener}昨天的记录已就绪。打开 TraceBit 生成工具升级建议。"),
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
        assert_eq!(greeting(at(7, 0)), "早上好");
        assert_eq!(greeting(at(10, 30)), "上午好");
        assert_eq!(greeting(at(12, 30)), "中午好");
        assert_eq!(greeting(at(16, 0)), "下午好");
        assert_eq!(greeting(at(20, 0)), "晚上好");
        assert_eq!(greeting(at(2, 0)), "夜深了");
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
        )
        .unwrap();
        assert_eq!(title, "上午好 · TraceBit");
        assert!(body.contains("你在编辑器和终端之间完成开发"));
        assert!(body.contains("2 个工具升级机会"));
        assert!(body.contains("用 lazygit 处理提交"));
        assert!(body.contains("打开 TraceBit"));
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
        let first = compose(now, Some(digest())).unwrap();
        let second = compose(now, Some(digest())).unwrap();
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
                })
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
