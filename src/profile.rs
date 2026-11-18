//! The learned work profile: a deterministic description of how this person
//! actually works, accumulated across recorded days.
//!
//! The profile never scores or judges behavior. It records the toolchain, the
//! recurring app hops, the rhythm and the recurring work items, so the advisor
//! can propose upgrades that fit habits that already exist instead of asking
//! for new ones.
//!
//! One entry per day is kept, so regenerating a report for the same day is
//! idempotent and the file stays small.

use crate::{
    categories::{is_productive, normalize_proc},
    insights::title_key,
    recommendations::tool_role,
    recorder::SENSITIVE_TITLE,
    redact::{ai_title, clean_title},
    store::{DayStats, hhmm, now_ms},
};
use anyhow::Result;
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

const MINUTE: i64 = 60_000;
/// Days kept on disk; older days are pruned on every observation.
pub const MAX_DAYS: usize = 60;
/// Two apps further apart than this are not a "hop".
const LOOP_GAP_MS: i64 = 2 * MINUTE;
const MAX_TOOLS: usize = 10;
const MAX_LOOPS: usize = 6;
const MAX_THEMES: usize = 8;
const MAX_SITES: usize = 8;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolSample {
    pub name: String,
    pub role: String,
    pub minutes: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LoopSample {
    pub a: String,
    pub b: String,
    pub transitions: u32,
    /// Total time spent on the side of the pair that is visited briefly.
    pub short_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ThemeSample {
    pub label: String,
    pub minutes: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct DayRecord {
    pub active_min: i64,
    pub first_active: Option<i64>,
    pub last_active: Option<i64>,
    /// Active minutes per local hour.
    pub hours: Vec<i64>,
    /// Productive minutes per local hour.
    pub focus_hours: Vec<i64>,
    pub tools: BTreeMap<String, ToolSample>,
    /// Keyed by `procA|procB`, sorted.
    pub loops: BTreeMap<String, LoopSample>,
    pub themes: BTreeMap<String, ThemeSample>,
    pub sites: BTreeMap<String, i64>,
}

impl DayRecord {
    pub fn from_stats(stats: &DayStats) -> Option<Self> {
        if stats.total_active_ms < MINUTE {
            return None;
        }
        let minutes = |ms: i64| (ms as f64 / MINUTE as f64).round() as i64;

        let mut tools: BTreeMap<String, ToolSample> = BTreeMap::new();
        for stay in &stats.stays {
            let spent = minutes(stay.duration());
            if spent <= 0 {
                continue;
            }
            let proc = normalize_proc(&stay.name);
            if proc.is_empty() {
                continue;
            }
            let entry = tools.entry(proc.clone()).or_insert_with(|| ToolSample {
                name: stay.app.trim().to_owned(),
                role: tool_role(&proc).unwrap_or("其他").to_owned(),
                minutes: 0,
            });
            entry.minutes += spent;
        }
        tools.retain(|_, tool| tool.minutes >= 1);

        let mut loops: BTreeMap<String, LoopSample> = BTreeMap::new();
        for pair in stats.stays.windows(2) {
            let (first, second) = (&pair[0], &pair[1]);
            if first.app == second.app || second.start - first.end > LOOP_GAP_MS {
                continue;
            }
            let (a, b, short_ms) = if first.duration() <= second.duration() {
                (&first.app, &second.app, first.duration())
            } else {
                (&second.app, &first.app, second.duration())
            };
            let (key_a, key_b) = if a <= b { (a, b) } else { (b, a) };
            let entry = loops
                .entry(format!("{key_a}|{key_b}"))
                .or_insert_with(|| LoopSample {
                    a: a.clone(),
                    b: b.clone(),
                    transitions: 0,
                    short_ms: 0,
                });
            entry.transitions += 1;
            entry.short_ms += short_ms;
        }

        let mut themes: BTreeMap<String, ThemeSample> = BTreeMap::new();
        for stay in &stats.stays {
            let category = stay.category();
            if !is_productive(category) {
                continue;
            }
            for (title, ms) in &stay.title_ms {
                if title.trim().is_empty() || title == SENSITIVE_TITLE {
                    continue;
                }
                let title = clean_title(title, &stay.app);
                if title.is_empty() {
                    continue;
                }
                let key = title_key(&title);
                if key.chars().count() < 3 {
                    continue;
                }
                let entry = themes.entry(key).or_insert_with(|| ThemeSample {
                    label: ai_title(category, &title),
                    minutes: 0,
                });
                entry.minutes += minutes(*ms);
            }
        }
        themes.retain(|_, theme| theme.minutes >= 5);

        let focus_hours = (0..24)
            .map(|hour| {
                minutes(
                    stats
                        .hourly_categories
                        .get(hour)
                        .into_iter()
                        .flatten()
                        .filter(|(category, _)| is_productive(category))
                        .map(|(_, ms)| *ms)
                        .sum(),
                )
            })
            .collect::<Vec<_>>();

        Some(Self {
            active_min: minutes(stats.total_active_ms),
            first_active: stats.first_active,
            last_active: stats.last_active,
            hours: (0..24)
                .map(|hour| minutes(stats.hourly.get(hour).copied().unwrap_or(0)))
                .collect(),
            focus_hours,
            tools,
            loops,
            themes,
            sites: stats
                .sites
                .iter()
                .filter(|site| site.ms >= MINUTE)
                .map(|site| (site.site.to_lowercase(), minutes(site.ms)))
                .collect(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecisionKind {
    Adopted,
    Dismissed,
}

impl DecisionKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Adopted => "已采纳",
            Self::Dismissed => "不需要",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Decision {
    /// Recommendation id, e.g. `development-loop` or `alternative-postman`.
    pub opportunity: String,
    pub title: String,
    pub resource: String,
    pub url: String,
    pub date_key: String,
    pub kind: Option<DecisionKind>,
    /// The evidence text at the time of the decision, for later comparison.
    pub evidence: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    pub version: u32,
    pub updated_at: i64,
    pub days: BTreeMap<String, DayRecord>,
    pub decisions: Vec<Decision>,
}

impl Profile {
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

    /// Records (or replaces) one day. Returns false for an idle day.
    pub fn observe(&mut self, date_key: &str, stats: &DayStats) -> bool {
        let Some(record) = DayRecord::from_stats(stats) else {
            return false;
        };
        self.version = 1;
        self.updated_at = now_ms();
        self.days.insert(date_key.to_owned(), record);
        while self.days.len() > MAX_DAYS {
            let oldest = self.days.keys().next().cloned();
            match oldest {
                Some(key) => {
                    self.days.remove(&key);
                }
                None => break,
            }
        }
        true
    }

    /// Keeps one decision per opportunity (latest wins).
    pub fn record(&mut self, decision: Decision) {
        self.decisions
            .retain(|item| item.opportunity != decision.opportunity);
        self.decisions.push(decision);
        self.updated_at = now_ms();
    }

    pub fn decision(&self, opportunity: &str) -> Option<&Decision> {
        self.decisions
            .iter()
            .find(|item| item.opportunity == opportunity)
    }

    pub fn adopted(&self) -> Vec<&Decision> {
        self.decisions
            .iter()
            .filter(|item| item.kind == Some(DecisionKind::Adopted))
            .collect()
    }

    pub fn dismissed(&self) -> BTreeSet<String> {
        self.decisions
            .iter()
            .filter(|item| item.kind == Some(DecisionKind::Dismissed))
            .map(|item| item.opportunity.clone())
            .collect()
    }

    /// Text for the model: what the person already told us about tools.
    pub fn decisions_for_prompt(&self) -> String {
        if self.decisions.is_empty() {
            return String::new();
        }
        let mut adopted = Vec::new();
        let mut dismissed = Vec::new();
        for decision in &self.decisions {
            let name = if decision.resource.trim().is_empty() {
                decision.title.clone()
            } else {
                decision.resource.clone()
            };
            match decision.kind {
                Some(DecisionKind::Adopted) => adopted.push(format!(
                    "{}（{} 起，来自「{}」）",
                    name, decision.date_key, decision.title
                )),
                Some(DecisionKind::Dismissed) => dismissed.push(name),
                None => {}
            }
        }
        let mut lines = Vec::new();
        if !adopted.is_empty() {
            lines.push(format!(
                "已采纳：{}。这些不要再当作新发现推荐，只在成本变化时提。",
                adopted.join("、")
            ));
        }
        if !dismissed.is_empty() {
            lines.push(format!(
                "已明确不需要：{}。不要再次推荐。",
                dismissed.join("、")
            ));
        }
        lines.join("\n")
    }

    /// Folds the retained days into a stable, human-readable picture.
    pub fn snapshot(&self) -> Snapshot {
        let days = self.days.iter().collect::<Vec<_>>();
        if days.is_empty() {
            return Snapshot::default();
        }
        let first_day = days
            .first()
            .map(|(key, _)| (*key).clone())
            .unwrap_or_default();
        let last_day = days
            .last()
            .map(|(key, _)| (*key).clone())
            .unwrap_or_default();
        let span = if self.days.len() == 1 {
            first_day.clone()
        } else {
            format!("{first_day} 至 {last_day}")
        };

        let mut tools: BTreeMap<String, (String, String, i64, usize)> = BTreeMap::new();
        for (_, day) in &days {
            for (proc, sample) in &day.tools {
                let entry = tools
                    .entry(proc.clone())
                    .or_insert_with(|| (sample.name.clone(), sample.role.clone(), 0, 0));
                entry.0 = sample.name.clone();
                entry.2 += sample.minutes;
                entry.3 += 1;
            }
        }
        let day_count = self.days.len();
        let mut tools = tools
            .into_iter()
            .map(|(proc, (name, role, total_min, present))| ToolSummary {
                name,
                role,
                days: present,
                total_min,
                avg_min: (total_min as f64 / day_count as f64).round() as i64,
                trend: trend_of(&days, &proc).to_owned(),
            })
            .filter(|tool| tool.avg_min >= 2)
            .collect::<Vec<_>>();
        tools.sort_by_key(|tool| std::cmp::Reverse(tool.total_min));
        tools.truncate(MAX_TOOLS);

        let mut loops: BTreeMap<String, LoopSummary> = BTreeMap::new();
        for (_, day) in &days {
            for sample in day.loops.values() {
                let key = if sample.a <= sample.b {
                    format!("{}|{}", sample.a, sample.b)
                } else {
                    format!("{}|{}", sample.b, sample.a)
                };
                let entry = loops.entry(key).or_insert_with(|| LoopSummary {
                    a: sample.a.clone(),
                    b: sample.b.clone(),
                    transitions: 0,
                    days: 0,
                    short_ms: 0,
                    per_day: 0.0,
                });
                entry.transitions += sample.transitions;
                entry.days += 1;
                entry.short_ms += sample.short_ms;
            }
        }
        let mut loops = loops
            .into_values()
            .map(|loop_| LoopSummary {
                per_day: (loop_.transitions as f64 / day_count as f64 * 10.0).round() / 10.0,
                a: loop_.a,
                b: loop_.b,
                days: loop_.days,
                transitions: loop_.transitions,
                short_ms: loop_.short_ms,
            })
            .filter(|loop_| loop_.per_day >= 2.0)
            .collect::<Vec<_>>();
        loops.sort_by(|a, b| b.per_day.total_cmp(&a.per_day));
        loops.truncate(MAX_LOOPS);

        let mut themes: BTreeMap<String, (String, i64, usize)> = BTreeMap::new();
        for (_, day) in &days {
            for (key, sample) in &day.themes {
                let entry = themes
                    .entry(key.clone())
                    .or_insert_with(|| (sample.label.clone(), 0, 0));
                entry.0 = sample.label.clone();
                entry.1 += sample.minutes;
                entry.2 += 1;
            }
        }
        let mut themes = themes
            .into_iter()
            .map(|(_, (label, total_min, days))| ThemeSummary {
                label,
                days,
                total_min,
            })
            .collect::<Vec<_>>();
        themes.sort_by_key(|theme| std::cmp::Reverse(theme.total_min));
        themes.truncate(MAX_THEMES);

        let mut sites: BTreeMap<String, (i64, usize)> = BTreeMap::new();
        for (_, day) in &days {
            for (site, minutes) in &day.sites {
                let entry = sites.entry(site.clone()).or_insert((0, 0));
                entry.0 += minutes;
                entry.1 += 1;
            }
        }
        let mut sites = sites
            .into_iter()
            .map(|(site, (total_min, days))| SiteSummary {
                site,
                days,
                total_min,
                per_day: (total_min as f64 / days.max(1) as f64).round() as i64,
            })
            .collect::<Vec<_>>();
        sites.sort_by_key(|site| std::cmp::Reverse(site.total_min));
        sites.truncate(MAX_SITES);

        Snapshot {
            days: day_count,
            span,
            tools,
            loops,
            themes,
            sites,
            rhythm: rhythm_of(&days),
        }
    }
}

fn trend_of(days: &[(&String, &DayRecord)], proc: &str) -> &'static str {
    if days.len() < 4 {
        return "";
    }
    let average = |slice: &[(&String, &DayRecord)]| -> f64 {
        if slice.is_empty() {
            return 0.0;
        }
        slice
            .iter()
            .map(|(_, day)| day.tools.get(proc).map(|tool| tool.minutes).unwrap_or(0) as f64)
            .sum::<f64>()
            / slice.len() as f64
    };
    let split = days.len() / 2;
    let recent = average(&days[split..]);
    let earlier = average(&days[..split]);
    if earlier < 1.0 {
        return if recent >= 5.0 { "近期出现" } else { "" };
    }
    let change = (recent - earlier) / earlier;
    if change > 0.25 {
        "近期上升"
    } else if change < -0.25 {
        "近期下降"
    } else {
        "近期稳定"
    }
}

fn median_time(values: &mut Vec<i64>) -> Option<String> {
    values.retain(|value| *value > 0);
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(hhmm(values[values.len() / 2]))
}

fn rhythm_of(days: &[(&String, &DayRecord)]) -> Rhythm {
    let count = days.len().max(1) as f64;
    let mut starts = days
        .iter()
        .filter_map(|(_, day)| day.first_active)
        .collect::<Vec<_>>();
    let mut ends = days
        .iter()
        .filter_map(|(_, day)| day.last_active)
        .collect::<Vec<_>>();
    let mut weekday_min = 0_i64;
    let mut weekend_min = 0_i64;
    for (key, day) in days {
        if NaiveDate::parse_from_str(key, "%Y-%m-%d")
            .map(|date| matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun))
            .unwrap_or(false)
        {
            weekend_min += day.active_min;
        } else {
            weekday_min += day.active_min;
        }
    }
    let total_min = (weekday_min + weekend_min).max(1);
    let mut hours = [0_i64; 24];
    let mut focus_hours = [0_i64; 24];
    for (_, day) in days {
        for hour in 0..24 {
            hours[hour] += day.hours.get(hour).copied().unwrap_or(0);
            focus_hours[hour] += day.focus_hours.get(hour).copied().unwrap_or(0);
        }
    }
    let best = hours
        .windows(2)
        .enumerate()
        .max_by_key(|(_, window)| window[0] + window[1])
        .map(|(hour, _)| hour)
        .unwrap_or(0);
    let focus_best = focus_hours
        .windows(2)
        .enumerate()
        .max_by_key(|(_, window)| window[0] + window[1])
        .map(|(hour, _)| hour)
        .unwrap_or(best);
    let active_hours = hours
        .iter()
        .filter(|minutes| **minutes as f64 / count >= 10.0)
        .count();
    let focus_avg = (focus_hours.get(focus_best).copied().unwrap_or(0)
        + focus_hours.get(focus_best + 1).copied().unwrap_or(0)) as f64
        / count;
    Rhythm {
        start: median_time(&mut starts).unwrap_or_default(),
        end: median_time(&mut ends).unwrap_or_default(),
        window: format!("{best}–{} 点", best + 2),
        focus_window: format!("{focus_best}–{} 点", focus_best + 2),
        focus_min: focus_avg.round() as i64,
        active_hours,
        weekend_share: (weekend_min as f64 / total_min as f64 * 100.0).round() as i64,
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolSummary {
    pub name: String,
    pub role: String,
    pub days: usize,
    pub total_min: i64,
    pub avg_min: i64,
    pub trend: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LoopSummary {
    pub a: String,
    pub b: String,
    pub transitions: u32,
    pub days: usize,
    pub short_ms: i64,
    pub per_day: f64,
}

impl LoopSummary {
    /// Average seconds spent on the briefly-visited side of the hop.
    pub fn dwell_secs(&self) -> i64 {
        if self.transitions == 0 {
            0
        } else {
            (self.short_ms as f64 / self.transitions as f64 / 1000.0).round() as i64
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ThemeSummary {
    pub label: String,
    pub days: usize,
    pub total_min: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct SiteSummary {
    pub site: String,
    pub days: usize,
    pub total_min: i64,
    pub per_day: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Rhythm {
    pub start: String,
    pub end: String,
    /// Widest active window.
    pub window: String,
    /// Where productive work concentrates.
    pub focus_window: String,
    pub focus_min: i64,
    pub active_hours: usize,
    pub weekend_share: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Snapshot {
    pub days: usize,
    pub span: String,
    pub tools: Vec<ToolSummary>,
    pub loops: Vec<LoopSummary>,
    pub themes: Vec<ThemeSummary>,
    pub sites: Vec<SiteSummary>,
    pub rhythm: Rhythm,
}

impl Snapshot {
    /// Prompt lines; the first element is the evidence kind.
    pub fn lines(&self) -> Vec<(&'static str, String)> {
        let mut lines = Vec::new();
        if self.days == 0 {
            return lines;
        }
        if !self.tools.is_empty() {
            lines.push((
                "tools",
                format!(
                    "主力工具（{} 个记录日）：{}。",
                    self.days,
                    self.tools
                        .iter()
                        .take(6)
                        .map(|tool| {
                            let mut text =
                                format!("{}（{}，日均 {} 分钟", tool.name, tool.role, tool.avg_min);
                            if !tool.trend.is_empty() {
                                text.push('，');
                                text.push_str(&tool.trend);
                            }
                            text.push('）');
                            text
                        })
                        .collect::<Vec<_>>()
                        .join("、")
                ),
            ));
        }
        if !self.loops.is_empty() {
            lines.push((
                "loops",
                format!(
                    "日常来回：{}。",
                    self.loops
                        .iter()
                        .take(4)
                        .map(|loop_| format!(
                            "{} ↔ {} 日均 {:.1} 次，其中一侧平均只停留 {} 秒",
                            loop_.a,
                            loop_.b,
                            loop_.per_day,
                            loop_.dwell_secs()
                        ))
                        .collect::<Vec<_>>()
                        .join("；")
                ),
            ));
        }
        let rhythm = &self.rhythm;
        let mut parts = Vec::new();
        if !rhythm.start.is_empty() {
            parts.push(if rhythm.end.is_empty() {
                format!("从 {} 开始活跃", rhythm.start)
            } else {
                format!("常在 {}–{} 活跃", rhythm.start, rhythm.end)
            });
        } else {
            parts.push(format!("最活跃时段 {}", rhythm.window));
        }
        if rhythm.active_hours > 0 {
            parts.push(format!("有 {} 个活跃小时", rhythm.active_hours));
        }
        parts.push(format!(
            "产出最集中在 {}（日均 {} 分钟）",
            rhythm.focus_window, rhythm.focus_min
        ));
        if rhythm.weekend_share > 0 {
            parts.push(format!("周末占 {}% 的活跃时间", rhythm.weekend_share));
        }
        lines.push(("rhythm", format!("工作节奏：{}。", parts.join("；"))));
        if !self.themes.is_empty() {
            lines.push((
                "themes",
                format!(
                    "反复处理的内容：{}。",
                    self.themes
                        .iter()
                        .take(5)
                        .map(|theme| format!(
                            "{}（{} 天，共 {} 分钟）",
                            theme.label, theme.days, theme.total_min
                        ))
                        .collect::<Vec<_>>()
                        .join("、")
                ),
            ));
        }
        // Site hosts are deliberately kept out of the prompt: the product
        // promise is that browsing domains never leave the machine. They are
        // still shown in the local report.
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{DeriveOptions, Event, derive_events};
    use chrono::{Local, TimeZone};
    use serde_json::json;

    fn at(hour: u32, minute: u32) -> i64 {
        let date = Local::now().date_naive();
        Local
            .from_local_datetime(&date.and_hms_opt(hour, minute, 0).unwrap())
            .single()
            .unwrap()
            .timestamp_millis()
    }

    fn focus(ts: i64, name: &str, app: &str, title: &str) -> Event {
        Event::app_focus(ts, app, name, title, 1, 0)
    }

    fn day() -> DayStats {
        let mut events = vec![focus(at(9, 0), "code", "VS Code", "main.rs - TraceBit")];
        let mut ts = at(9, 0);
        for _ in 0..6 {
            ts += 4 * 60_000;
            events.push(focus(ts, "chrome", "Chrome", "docs.rs"));
            ts += 20_000;
            events.push(focus(ts, "code", "VS Code", "main.rs - TraceBit"));
        }
        events.push(Event::at(ts + 60_000, "lock", json!({})));
        derive_events(&events, DeriveOptions::default(), None)
    }

    #[test]
    fn observation_is_idempotent_per_day_and_folds_into_a_snapshot() {
        let stats = day();
        let mut profile = Profile::default();
        assert!(profile.observe("2026-01-01", &stats));
        let first = serde_json::to_string(&profile.days).unwrap();
        assert!(profile.observe("2026-01-01", &stats));
        assert_eq!(first, serde_json::to_string(&profile.days).unwrap());
        let snapshot = profile.snapshot();
        assert_eq!(snapshot.days, 1);
        assert!(snapshot.tools.iter().any(|tool| tool.name == "VS Code"));
        assert!(
            snapshot
                .loops
                .iter()
                .any(|loop_| loop_.a == "VS Code" || loop_.b == "VS Code")
        );
        assert!(!snapshot.lines().is_empty());
        assert!(
            snapshot
                .lines()
                .iter()
                .all(|(_, text)| !text.contains("secret"))
        );
    }

    #[test]
    fn decisions_replace_and_filter() {
        let mut profile = Profile::default();
        profile.record(Decision {
            opportunity: "development-loop".to_owned(),
            title: "把命令收成入口".to_owned(),
            resource: "just".to_owned(),
            url: "https://github.com/casey/just".to_owned(),
            date_key: "2026-01-01".to_owned(),
            kind: Some(DecisionKind::Dismissed),
            evidence: "编辑器与终端之间切换 12 次。".to_owned(),
        });
        assert!(profile.dismissed().contains("development-loop"));
        profile.record(Decision {
            opportunity: "development-loop".to_owned(),
            title: "把命令收成入口".to_owned(),
            resource: "just".to_owned(),
            url: "https://github.com/casey/just".to_owned(),
            date_key: "2026-01-02".to_owned(),
            kind: Some(DecisionKind::Adopted),
            evidence: "编辑器与终端之间切换 12 次。".to_owned(),
        });
        assert!(profile.dismissed().is_empty());
        assert_eq!(profile.adopted().len(), 1);
        assert!(profile.decisions_for_prompt().contains("已采纳"));
    }

    #[test]
    fn old_days_are_pruned() {
        let stats = day();
        let mut profile = Profile::default();
        for index in 0..(MAX_DAYS + 5) {
            profile.observe(
                &format!("2026-{:02}-{:02}", index / 28 + 1, index % 28 + 1),
                &stats,
            );
        }
        assert_eq!(profile.days.len(), MAX_DAYS);
    }
}
