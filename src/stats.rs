//! Period statistics for the "最近统计" page.
//!
//! The page is a read-only roll-up of the same [`DayStats`] the daily view
//! uses, so every number here agrees with 今日活动 and with the AI report.

use std::collections::BTreeMap;

use chrono::{Datelike, Duration as ChronoDuration, NaiveDate};

use crate::store::{AppStat, DayStats, DayStore, DeriveOptions};

/// Day windows offered by the page selector.
pub const PERIODS: [u32; 3] = [7, 14, 30];

/// One bar of the daily chart. Days without activity stay in the list so the
/// axis keeps a continuous run of dates.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DayPoint {
    pub date_key: String,
    /// `Weekday::num_days_from_sunday`, kept as a number so the struct can
    /// derive `Default`.
    pub weekday: u32,
    pub active_ms: i64,
    pub switches: u32,
    pub top_app: String,
    pub category: String,
}

/// An application across the whole period.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppTotal {
    /// Display label (usually the friendly app name).
    pub app: String,
    pub name: String,
    pub category: String,
    pub ms: i64,
    pub switches: u32,
    /// Number of recorded days the app appeared on.
    pub days: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CategoryTotal {
    pub name: String,
    pub ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SiteTotal {
    pub site: String,
    pub app: String,
    pub ms: i64,
    pub visits: u32,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeriodStats {
    pub days: u32,
    pub start_key: String,
    pub end_key: String,
    /// Oldest first, one entry per calendar day in the window.
    pub daily: Vec<DayPoint>,
    pub active_days: u32,
    pub total_active_ms: i64,
    pub avg_daily_ms: i64,
    pub total_switches: u32,
    pub switches_per_hour: f64,
    /// `(app, ms)` of the longest uninterrupted focus block.
    pub longest_block: Option<(String, i64)>,
    /// `(date_key, ms)` of the most active day.
    pub best_day: Option<(String, i64)>,
    pub screenshots: usize,
    pub snapshot_bytes: u64,
    pub hourly: [i64; 24],
    /// Sorted by time, longest first.
    pub apps: Vec<AppTotal>,
    /// Sorted by time, longest first.
    pub categories: Vec<CategoryTotal>,
    /// Sorted by time, longest first.
    pub sites: Vec<SiteTotal>,
}

/// Load and aggregate the last `days` days ending on `end`.
pub fn load(store: &DayStore, options: DeriveOptions, end: NaiveDate, days: u32) -> PeriodStats {
    let days = days.clamp(1, 365);
    let mut recorded = Vec::new();
    let mut daily = Vec::with_capacity(days as usize);
    for offset in (0..days).rev() {
        let date = end - ChronoDuration::days(offset as i64);
        let date_key = date.format("%Y-%m-%d").to_string();
        match store.day_stats(&date_key, options) {
            Ok(stats) if stats.total_active_ms > 0 => {
                daily.push(point(&stats));
                recorded.push(stats);
            }
            Ok(_) => daily.push(DayPoint {
                date_key,
                weekday: date.weekday().num_days_from_sunday(),
                ..DayPoint::default()
            }),
            Err(_) => daily.push(DayPoint {
                date_key,
                weekday: date.weekday().num_days_from_sunday(),
                ..DayPoint::default()
            }),
        }
    }
    summarize(days, daily, &recorded)
}

fn point(stats: &DayStats) -> DayPoint {
    let top_app = stats
        .apps
        .iter()
        .max_by_key(|app| app.ms)
        .map(display_app)
        .unwrap_or_default();
    let category = stats
        .categories
        .iter()
        .max_by_key(|category| category.ms)
        .map(|category| category.name.clone())
        .unwrap_or_default();
    DayPoint {
        date_key: stats.date_key.clone(),
        weekday: NaiveDate::parse_from_str(&stats.date_key, "%Y-%m-%d")
            .map(|date| date.weekday().num_days_from_sunday())
            .unwrap_or(0),
        active_ms: stats.total_active_ms,
        switches: stats.switches,
        top_app,
        category,
    }
}

fn display_app(app: &AppStat) -> String {
    if app.app.trim().is_empty() {
        app.name.clone()
    } else {
        app.app.clone()
    }
}

#[derive(Default)]
struct AppGroup {
    name: String,
    ms: i64,
    switches: u32,
    days: u32,
    categories: BTreeMap<String, i64>,
}

#[derive(Default)]
struct SiteGroup {
    app: String,
    ms: i64,
    visits: u32,
    categories: BTreeMap<String, i64>,
}

fn summarize(days: u32, daily: Vec<DayPoint>, recorded: &[DayStats]) -> PeriodStats {
    let mut hourly = [0i64; 24];
    let mut apps: BTreeMap<String, AppGroup> = BTreeMap::new();
    let mut categories: BTreeMap<String, i64> = BTreeMap::new();
    let mut sites: BTreeMap<String, SiteGroup> = BTreeMap::new();
    let mut longest: Option<(String, i64)> = None;
    let mut screenshots = 0usize;
    let mut snapshot_bytes = 0u64;
    let mut total_active_ms = 0i64;
    let mut total_switches = 0u32;

    for stats in recorded {
        total_active_ms += stats.total_active_ms;
        total_switches = total_switches.saturating_add(stats.switches);
        screenshots += stats.screenshots.len();
        snapshot_bytes += stats.snapshot_bytes;
        for (hour, ms) in stats.hourly.iter().enumerate().take(24) {
            hourly[hour] += *ms;
        }
        if let Some(block) = &stats.longest_block
            && longest.as_ref().is_none_or(|(_, ms)| block.ms > *ms)
        {
            longest = Some((block.app.clone(), block.ms));
        }
        for app in &stats.apps {
            if app.ms <= 0 {
                continue;
            }
            let label = display_app(app);
            let group = apps.entry(label.clone()).or_insert_with(|| AppGroup {
                name: app.name.clone(),
                ..AppGroup::default()
            });
            group.ms += app.ms;
            group.switches = group.switches.saturating_add(app.switches);
            group.days += 1;
            *group.categories.entry(app.category.clone()).or_insert(0) += app.ms;
        }
        for category in &stats.categories {
            *categories.entry(category.name.clone()).or_insert(0) += category.ms;
        }
        for site in &stats.sites {
            if site.ms <= 0 && site.visits == 0 {
                continue;
            }
            let group = sites.entry(site.site.clone()).or_insert_with(|| SiteGroup {
                app: site.app.clone(),
                ..SiteGroup::default()
            });
            group.ms += site.ms;
            group.visits = group.visits.saturating_add(site.visits);
            if let Some(category) = &site.category {
                *group.categories.entry(category.clone()).or_insert(0) += site.ms.max(1);
            }
        }
    }

    let mut apps: Vec<AppTotal> = apps
        .into_iter()
        .map(|(label, group)| AppTotal {
            app: label,
            name: group.name,
            category: dominant(&group.categories),
            ms: group.ms,
            switches: group.switches,
            days: group.days,
        })
        .collect();
    apps.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.app.cmp(&b.app)));

    let mut categories: Vec<CategoryTotal> = categories
        .into_iter()
        .map(|(name, ms)| CategoryTotal { name, ms })
        .collect();
    categories.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.name.cmp(&b.name)));

    let mut sites: Vec<SiteTotal> = sites
        .into_iter()
        .map(|(site, group)| SiteTotal {
            site,
            app: group.app,
            ms: group.ms,
            visits: group.visits,
            category: group
                .categories
                .iter()
                .max_by_key(|(_, ms)| **ms)
                .map(|(name, _)| name.clone()),
        })
        .collect();
    sites.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.site.cmp(&b.site)));

    let active_days = recorded.len() as u32;
    let avg_daily_ms = if active_days > 0 {
        total_active_ms / i64::from(active_days)
    } else {
        0
    };
    let switches_per_hour = if total_active_ms > 0 {
        f64::from(total_switches) * 3_600_000.0 / total_active_ms as f64
    } else {
        0.0
    };
    let best_day = daily
        .iter()
        .filter(|day| day.active_ms > 0)
        .max_by_key(|day| day.active_ms)
        .map(|day| (day.date_key.clone(), day.active_ms));

    PeriodStats {
        days,
        start_key: daily
            .first()
            .map(|day| day.date_key.clone())
            .unwrap_or_default(),
        end_key: daily
            .last()
            .map(|day| day.date_key.clone())
            .unwrap_or_default(),
        daily,
        active_days,
        total_active_ms,
        avg_daily_ms,
        total_switches,
        switches_per_hour,
        longest_block: longest,
        best_day,
        screenshots,
        snapshot_bytes,
        hourly,
        apps,
        categories,
        sites,
    }
}

fn dominant(totals: &BTreeMap<String, i64>) -> String {
    totals
        .iter()
        .max_by_key(|(_, ms)| **ms)
        .map(|(name, _)| name.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{AppStat, CategoryStat, FocusBlock, SiteStat};

    fn app(label: &str, category: &str, ms: i64, switches: u32) -> AppStat {
        AppStat {
            name: label.to_lowercase(),
            app: label.to_owned(),
            category: category.to_owned(),
            ms,
            switches,
            titles: Default::default(),
            title_ms: Default::default(),
        }
    }

    fn day(key: &str, apps: Vec<AppStat>) -> DayStats {
        let mut categories: BTreeMap<String, i64> = BTreeMap::new();
        for app in &apps {
            *categories.entry(app.category.clone()).or_insert(0) += app.ms;
        }
        DayStats {
            date_key: key.to_owned(),
            total_active_ms: apps.iter().map(|app| app.ms).sum(),
            switches: apps.iter().map(|app| app.switches).sum(),
            apps,
            hourly: vec![0; 24],
            categories: categories
                .into_iter()
                .map(|(name, ms)| CategoryStat { name, ms })
                .collect(),
            ..DayStats::default()
        }
    }

    fn window(end: NaiveDate, days: u32, recorded: &[DayStats]) -> Vec<DayPoint> {
        (0..days)
            .rev()
            .map(|offset| {
                let date = end - ChronoDuration::days(i64::from(offset));
                let date_key = date.format("%Y-%m-%d").to_string();
                recorded
                    .iter()
                    .find(|stats| stats.date_key == date_key)
                    .map(point)
                    .unwrap_or(DayPoint {
                        date_key,
                        weekday: date.weekday().num_days_from_sunday(),
                        ..DayPoint::default()
                    })
            })
            .collect()
    }

    #[test]
    fn merges_apps_and_picks_dominant_category() {
        let end = NaiveDate::from_ymd_opt(2026, 5, 20).unwrap();
        let first = day(
            "2026-05-19",
            vec![
                app("Code", "开发", 60 * 60_000, 4),
                app("Chrome", "浏览器", 20 * 60_000, 2),
            ],
        );
        let second = day(
            "2026-05-20",
            vec![
                app("Code", "开发", 30 * 60_000, 3),
                app("Slack", "即时通讯", 10 * 60_000, 1),
            ],
        );
        let stats = summarize(
            2,
            window(end, 2, &[first.clone(), second.clone()]),
            &[first, second],
        );

        assert_eq!(stats.days, 2);
        assert_eq!(stats.active_days, 2);
        assert_eq!(stats.start_key, "2026-05-19");
        assert_eq!(stats.end_key, "2026-05-20");
        assert_eq!(stats.total_active_ms, 120 * 60_000);
        assert_eq!(stats.avg_daily_ms, 60 * 60_000);
        assert_eq!(stats.apps[0].app, "Code");
        assert_eq!(stats.apps[0].ms, 90 * 60_000);
        assert_eq!(stats.apps[0].days, 2);
        assert_eq!(stats.apps[0].switches, 7);
        assert_eq!(stats.apps[0].category, "开发");
        assert_eq!(stats.categories[0].name, "开发");
        assert_eq!(stats.categories[0].ms, 90 * 60_000);
        assert_eq!(stats.daily[1].top_app, "Code");
        assert_eq!(stats.daily[1].category, "开发");
        assert_eq!(stats.best_day.as_ref().unwrap().0, "2026-05-19");
        assert!((stats.switches_per_hour - 5.0).abs() < 0.001);
    }

    #[test]
    fn empty_days_keep_the_axis_and_are_not_counted() {
        let end = NaiveDate::from_ymd_opt(2026, 5, 20).unwrap();
        let stats = summarize(3, window(end, 3, &[]), &[]);

        assert_eq!(stats.daily.len(), 3);
        assert_eq!(stats.active_days, 0);
        assert_eq!(stats.total_active_ms, 0);
        assert_eq!(stats.avg_daily_ms, 0);
        assert!(stats.apps.is_empty());
        assert!(stats.best_day.is_none());
    }

    #[test]
    fn sums_hourly_rhythm_sites_and_longest_block() {
        let end = NaiveDate::from_ymd_opt(2026, 5, 20).unwrap();
        let mut first = day("2026-05-19", vec![app("Code", "开发", 45 * 60_000, 2)]);
        first.hourly[9] = 40 * 60_000;
        first.hourly[10] = 5 * 60_000;
        first.longest_block = Some(FocusBlock {
            app: "Code".to_owned(),
            ms: 40 * 60_000,
            started_at: 0,
        });
        first.sites.push(SiteStat {
            site: "docs.rs".to_owned(),
            visits: 3,
            first_seen: 0,
            last_seen: 0,
            app: "Chrome".to_owned(),
            ms: 12 * 60_000,
            category: Some("浏览器".to_owned()),
        });
        let mut second = day("2026-05-20", vec![app("Code", "开发", 20 * 60_000, 1)]);
        second.hourly[9] = 20 * 60_000;
        second.longest_block = Some(FocusBlock {
            app: "Code".to_owned(),
            ms: 20 * 60_000,
            started_at: 0,
        });

        let stats = summarize(
            2,
            window(end, 2, &[first.clone(), second.clone()]),
            &[first, second],
        );

        assert_eq!(stats.hourly[9], 60 * 60_000);
        assert_eq!(stats.hourly[10], 5 * 60_000);
        assert_eq!(stats.hourly[11], 0);
        assert_eq!(stats.longest_block, Some(("Code".to_owned(), 40 * 60_000)));
        assert_eq!(stats.sites.len(), 1);
        assert_eq!(stats.sites[0].site, "docs.rs");
        assert_eq!(stats.sites[0].visits, 3);
        assert_eq!(stats.sites[0].category.as_deref(), Some("浏览器"));
    }

    #[test]
    fn load_reads_recorded_days_and_pads_the_rest() {
        use crate::store::{Event, kinds};
        use chrono::{Local, TimeZone};
        use serde_json::json;

        let temp = tempfile::tempdir().unwrap();
        let store = DayStore::new(temp.path()).unwrap();
        let today = Local::now().date_naive();
        let yesterday = today - ChronoDuration::days(1);
        let base = Local
            .from_local_datetime(&yesterday.and_hms_opt(3, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp_millis();
        for event in [
            Event::app_focus(base, "Code", "code", "a.rs", 10, 0),
            Event::at(base + 600_000, kinds::TICK, json!({"i": 0})),
            Event::app_focus(base + 1_200_000, "Chrome", "chrome", "docs", 11, 0),
            Event::at(base + 1_800_000, kinds::TICK, json!({"i": 0})),
            Event::at(base + 2_400_000, kinds::LOCK, json!({})),
        ] {
            store.append(event).unwrap();
        }

        let stats = load(&store, DeriveOptions::default(), today, 7);
        assert_eq!(stats.daily.len(), 7);
        assert_eq!(stats.end_key, today.format("%Y-%m-%d").to_string());
        assert_eq!(
            stats.daily[5].date_key,
            yesterday.format("%Y-%m-%d").to_string()
        );
        assert_eq!(stats.active_days, 1);
        assert!(stats.total_active_ms > 0);
        assert!(stats.apps.iter().any(|app| app.app == "Code"));
    }
}
