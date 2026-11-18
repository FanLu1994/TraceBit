//! Turns a day of activity into numbered, verifiable evidence for the advisor.
//!
//! Everything numeric is computed here, locally and deterministically. The model
//! only interprets the evidence and proposes changes; it never has to (and is
//! not allowed to) invent figures.

use crate::{
    categories::{is_communication, is_distraction, is_meeting, is_productive},
    recorder::SENSITIVE_TITLE,
    redact::{ai_title, clean_title},
    store::{CONTIGUOUS_GAP_MS, DayStats, Stay, hhmm, local_datetime},
};
use chrono::Timelike;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const MINUTE: i64 = 60_000;
/// A productive run of at least this long counts as deep work.
pub const DEEP_BLOCK_MS: i64 = 25 * MINUTE;
/// A gap longer than this ends a productive run.
const RUN_GAP_MS: i64 = 2 * MINUTE;
/// Non-productive stays up to this long interrupt a run without ending it.
const BRIEF_INTERRUPTION_MS: i64 = 2 * MINUTE;
/// A pause at least this long counts as a break.
const BREAK_MS: i64 = 5 * MINUTE;
const QUICK_CHECK_MS: i64 = MINUTE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Better {
    Higher,
    Lower,
    Neutral,
}

pub struct MetricDef {
    pub key: &'static str,
    pub label: &'static str,
    pub unit: &'static str,
    pub better: Better,
}

/// Metrics a report can watch. Keys are part of the AI output contract.
pub const METRICS: [MetricDef; 10] = [
    MetricDef {
        key: "active_min",
        label: "活跃时长",
        unit: "分钟",
        better: Better::Neutral,
    },
    MetricDef {
        key: "deep_work_min",
        label: "深度工作时长",
        unit: "分钟",
        better: Better::Higher,
    },
    MetricDef {
        key: "deep_blocks",
        label: "深度工作块",
        unit: "个",
        better: Better::Higher,
    },
    MetricDef {
        key: "switches_per_hour",
        label: "每小时应用切换",
        unit: "次",
        better: Better::Lower,
    },
    MetricDef {
        key: "comm_checks",
        label: "打开通讯应用",
        unit: "次",
        better: Better::Lower,
    },
    MetricDef {
        key: "comm_min",
        label: "通讯时长",
        unit: "分钟",
        better: Better::Neutral,
    },
    MetricDef {
        key: "meeting_min",
        label: "会议时长",
        unit: "分钟",
        better: Better::Lower,
    },
    MetricDef {
        key: "distraction_min",
        label: "娱乐与资讯时长",
        unit: "分钟",
        better: Better::Lower,
    },
    MetricDef {
        key: "late_min",
        label: "23 点后活跃",
        unit: "分钟",
        better: Better::Lower,
    },
    MetricDef {
        key: "longest_stretch_min",
        label: "最长连续未休息",
        unit: "分钟",
        better: Better::Lower,
    },
];

pub fn metric_def(key: &str) -> Option<&'static MetricDef> {
    METRICS.iter().find(|metric| metric.key == key)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct DayMetrics {
    pub active_min: f64,
    pub deep_work_min: f64,
    pub deep_blocks: f64,
    pub switches_per_hour: f64,
    pub comm_checks: f64,
    pub comm_min: f64,
    pub meeting_min: f64,
    pub distraction_min: f64,
    pub late_min: f64,
    pub longest_stretch_min: f64,
}

impl DayMetrics {
    pub fn get(&self, key: &str) -> Option<f64> {
        Some(match key {
            "active_min" => self.active_min,
            "deep_work_min" => self.deep_work_min,
            "deep_blocks" => self.deep_blocks,
            "switches_per_hour" => self.switches_per_hour,
            "comm_checks" => self.comm_checks,
            "comm_min" => self.comm_min,
            "meeting_min" => self.meeting_min,
            "distraction_min" => self.distraction_min,
            "late_min" => self.late_min,
            "longest_stretch_min" => self.longest_stretch_min,
            _ => return None,
        })
    }

    fn set(&mut self, key: &str, value: f64) {
        match key {
            "active_min" => self.active_min = value,
            "deep_work_min" => self.deep_work_min = value,
            "deep_blocks" => self.deep_blocks = value,
            "switches_per_hour" => self.switches_per_hour = value,
            "comm_checks" => self.comm_checks = value,
            "comm_min" => self.comm_min = value,
            "meeting_min" => self.meeting_min = value,
            "distraction_min" => self.distraction_min = value,
            "late_min" => self.late_min = value,
            "longest_stretch_min" => self.longest_stretch_min = value,
            _ => {}
        }
    }

    pub fn average(days: &[DayMetrics]) -> Option<DayMetrics> {
        if days.is_empty() {
            return None;
        }
        let mut average = DayMetrics::default();
        for metric in &METRICS {
            let sum: f64 = days.iter().filter_map(|day| day.get(metric.key)).sum();
            average.set(metric.key, round1(sum / days.len() as f64));
        }
        Some(average)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub id: String,
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Run {
    pub start: i64,
    pub end: i64,
    pub productive_ms: i64,
    pub interruptions: u32,
    pub comm_interruptions: u32,
    pub main_app: String,
}

fn minutes(ms: i64) -> i64 {
    (ms as f64 / MINUTE as f64).round() as i64
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn category_of(stay: &Stay) -> &str {
    stay.category()
}

/// Productive runs: productive stays, tolerating gaps up to two minutes and
/// brief non-productive interruptions (a quick chat check does not end focus).
pub fn productive_runs(stays: &[Stay]) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut current: Option<(Run, HashMap<String, i64>)> = None;
    let finish = |current: &mut Option<(Run, HashMap<String, i64>)>, runs: &mut Vec<Run>| {
        if let Some((mut run, apps)) = current.take() {
            run.main_app = apps
                .into_iter()
                .max_by_key(|(_, ms)| *ms)
                .map(|(app, _)| app)
                .unwrap_or_default();
            runs.push(run);
        }
    };
    for stay in stays {
        let productive = is_productive(category_of(stay));
        if let Some((run, apps)) = current.as_mut() {
            let gap = stay.start - run.end;
            if gap <= RUN_GAP_MS {
                if productive {
                    run.end = stay.end;
                    run.productive_ms += stay.duration();
                    *apps.entry(stay.app.clone()).or_insert(0) += stay.duration();
                    continue;
                }
                if stay.duration() <= BRIEF_INTERRUPTION_MS {
                    run.end = stay.end;
                    run.interruptions += 1;
                    if is_communication(category_of(stay)) {
                        run.comm_interruptions += 1;
                    }
                    continue;
                }
            }
            finish(&mut current, &mut runs);
        }
        if productive {
            current = Some((
                Run {
                    start: stay.start,
                    end: stay.end,
                    productive_ms: stay.duration(),
                    interruptions: 0,
                    comm_interruptions: 0,
                    main_app: String::new(),
                },
                HashMap::from([(stay.app.clone(), stay.duration())]),
            ));
        }
    }
    finish(&mut current, &mut runs);
    runs
}

/// Continuous activity without a break of at least five minutes.
fn stretches(stays: &[Stay]) -> Vec<(i64, i64)> {
    let mut stretches: Vec<(i64, i64)> = Vec::new();
    for stay in stays {
        if let Some(last) = stretches.last_mut()
            && stay.start - last.1 < BREAK_MS
        {
            last.1 = last.1.max(stay.end);
            continue;
        }
        stretches.push((stay.start, stay.end));
    }
    stretches
}

fn is_meeting_stay(stay: &Stay) -> bool {
    category_of(stay) == "会议" || is_meeting(&stay.name, &stay.app, &stay.title)
}

/// Meeting blocks: meeting stays merged across gaps up to five minutes.
fn meeting_blocks(stays: &[Stay]) -> Vec<(i64, i64)> {
    let mut blocks: Vec<(i64, i64)> = Vec::new();
    for stay in stays.iter().filter(|stay| is_meeting_stay(stay)) {
        if let Some(last) = blocks.last_mut()
            && stay.start - last.1 <= 5 * MINUTE
        {
            last.1 = last.1.max(stay.end);
            continue;
        }
        blocks.push((stay.start, stay.end));
    }
    blocks.retain(|(start, end)| end - start >= 5 * MINUTE);
    blocks
}

fn hour_of(ts: i64) -> Option<usize> {
    local_datetime(ts).map(|date| date.hour() as usize)
}

pub fn day_metrics(stats: &DayStats) -> DayMetrics {
    let runs = productive_runs(&stats.stays);
    let deep = runs
        .iter()
        .filter(|run| run.productive_ms >= DEEP_BLOCK_MS)
        .collect::<Vec<_>>();
    let active_hours = stats.total_active_ms as f64 / 3_600_000.0;
    let category_ms = |predicate: fn(&str) -> bool| -> i64 {
        stats
            .categories
            .iter()
            .filter(|category| predicate(&category.name))
            .map(|category| category.ms)
            .sum()
    };
    let late_ms: i64 = stats
        .hourly
        .iter()
        .enumerate()
        .filter(|(hour, _)| *hour >= 23 || *hour < 5)
        .map(|(_, ms)| *ms)
        .sum();
    DayMetrics {
        active_min: minutes(stats.total_active_ms) as f64,
        deep_work_min: minutes(deep.iter().map(|run| run.productive_ms).sum()) as f64,
        deep_blocks: deep.len() as f64,
        switches_per_hour: if active_hours > 0.1 {
            round1(stats.switches as f64 / active_hours)
        } else {
            0.0
        },
        comm_checks: stats
            .stays
            .iter()
            .filter(|stay| is_communication(category_of(stay)))
            .count() as f64,
        comm_min: minutes(category_ms(is_communication)) as f64,
        meeting_min: minutes(
            meeting_blocks(&stats.stays)
                .iter()
                .map(|(start, end)| end - start)
                .sum(),
        ) as f64,
        distraction_min: minutes(category_ms(is_distraction)) as f64,
        late_min: minutes(late_ms) as f64,
        longest_stretch_min: minutes(
            stretches(&stats.stays)
                .iter()
                .map(|(start, end)| end - start)
                .max()
                .unwrap_or(0),
        ) as f64,
    }
}

/// Title key that groups "a.rs - Project - VS Code" and "b.rs - Project - VS Code"
/// differently but survives unread-count prefixes like "(3) Inbox".
pub(crate) fn title_key(title: &str) -> String {
    let title = title.trim();
    let title = title
        .strip_prefix('(')
        .and_then(|rest| rest.split_once(')'))
        .filter(|(count, _)| count.chars().all(|ch| ch.is_ascii_digit()))
        .map(|(_, rest)| rest.trim())
        .unwrap_or(title);
    title
        .split([' ', '-', '–', '—', '|', '·', '»'])
        .filter(|part| !part.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(40)
        .collect::<String>()
        .to_lowercase()
}

/// Titles that may be shown to the model, with their time, best first.
fn shareable_titles(stats: &DayStats) -> Vec<(String, String, String, i64)> {
    let mut titles: BTreeMap<(String, String), (String, i64)> = BTreeMap::new();
    for stay in &stats.stays {
        let category = category_of(stay).to_owned();
        if is_communication(&category) {
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
            let entry = titles
                .entry((stay.app.clone(), title_key(&title)))
                .or_insert_with(|| (title.clone(), 0));
            entry.1 += ms;
        }
    }
    let mut titles = titles
        .into_iter()
        .map(|((app, key), (title, ms))| (app, key, title, ms))
        .collect::<Vec<_>>();
    titles.sort_by_key(|(_, _, _, ms)| std::cmp::Reverse(*ms));
    titles
}

pub struct History<'a> {
    pub stats: &'a DayStats,
    pub metrics: &'a DayMetrics,
}

pub struct EvidenceBuilder {
    items: Vec<Evidence>,
}

impl EvidenceBuilder {
    fn push(&mut self, kind: &str, text: String) {
        let id = format!("E{}", self.items.len() + 1);
        self.items.push(Evidence {
            id,
            kind: kind.to_owned(),
            text,
        });
    }
}

fn percent_change(today: f64, baseline: f64) -> Option<f64> {
    (baseline.abs() > f64::EPSILON).then(|| (today - baseline) / baseline * 100.0)
}

/// Builds the evidence list. `history` holds earlier days (most recent first).
pub fn build_evidence(
    stats: &DayStats,
    metrics: &DayMetrics,
    history: &[History<'_>],
) -> Vec<Evidence> {
    let mut out = EvidenceBuilder { items: Vec::new() };
    let stays = &stats.stays;
    let runs = productive_runs(stays);
    let deep = runs
        .iter()
        .filter(|run| run.productive_ms >= DEEP_BLOCK_MS)
        .collect::<Vec<_>>();

    // Overview.
    out.push(
        "overview",
        format!(
            "活跃 {} 分钟（{}–{}），深度工作（连续产出 ≥25 分钟）{} 分钟、{} 块，每小时切换应用 {} 次。",
            metrics.active_min,
            stats.first_active.map(hhmm).unwrap_or_default(),
            stats.last_active.map(hhmm).unwrap_or_default(),
            metrics.deep_work_min,
            metrics.deep_blocks,
            metrics.switches_per_hour
        ),
    );

    // Category mix.
    let total = stats.total_active_ms.max(1) as f64;
    let mix = stats
        .categories
        .iter()
        .take(7)
        .map(|category| {
            format!(
                "{} {} 分钟（{}%）",
                category.name,
                minutes(category.ms),
                (category.ms as f64 / total * 100.0).round()
            )
        })
        .collect::<Vec<_>>();
    if !mix.is_empty() {
        out.push("mix", format!("时间构成：{}。", mix.join("、")));
    }

    // What the time actually went into.
    let titles = shareable_titles(stats);
    let top = titles
        .iter()
        .filter(|(_, _, _, ms)| *ms >= 3 * MINUTE)
        .take(6)
        .map(|(app, _, title, ms)| {
            let category = stays
                .iter()
                .find(|stay| &stay.app == app)
                .map(category_of)
                .unwrap_or("其他");
            format!(
                "{app}「{}」{} 分钟",
                ai_title(category, title),
                minutes(*ms)
            )
        })
        .collect::<Vec<_>>();
    if !top.is_empty() {
        out.push("content", format!("耗时最多的窗口：{}。", top.join("；")));
    }

    // Deep work blocks and when they happen.
    if deep.is_empty() {
        let longest = runs.iter().map(|run| run.productive_ms).max().unwrap_or(0);
        if metrics.active_min >= 60.0 {
            out.push(
                "deep_work",
                format!(
                    "全天没有达到 25 分钟的连续产出段；最长一段连续产出 {} 分钟。",
                    minutes(longest)
                ),
            );
        }
    } else {
        let blocks = deep
            .iter()
            .take(5)
            .map(|run| {
                format!(
                    "{}–{}（{} 分钟，{}，期间被打断 {} 次）",
                    hhmm(run.start),
                    hhmm(run.end),
                    minutes(run.productive_ms),
                    run.main_app,
                    run.interruptions
                )
            })
            .collect::<Vec<_>>();
        out.push("deep_work", format!("深度工作块：{}。", blocks.join("；")));
    }

    // Ping-pong between two apps.
    // (first app, second app) -> (transitions, dwell times on each side)
    type PairStats = (u32, Vec<i64>, Vec<i64>);
    let mut pairs: HashMap<(String, String), PairStats> = HashMap::new();
    for pair in stays.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if a.app == b.app || b.start - a.end > CONTIGUOUS_GAP_MS {
            continue;
        }
        let (first, second, first_ms, second_ms) = if a.app < b.app {
            (&a.app, &b.app, a.duration(), b.duration())
        } else {
            (&b.app, &a.app, b.duration(), a.duration())
        };
        let entry = pairs
            .entry((first.clone(), second.clone()))
            .or_insert_with(|| (0, Vec::new(), Vec::new()));
        entry.0 += 1;
        entry.1.push(first_ms);
        entry.2.push(second_ms);
    }
    let mut pairs = pairs.into_iter().collect::<Vec<_>>();
    pairs.sort_by_key(|(_, (count, _, _))| std::cmp::Reverse(*count));
    for ((first, second), (count, first_ms, second_ms)) in pairs.into_iter().take(3) {
        if count < 8 {
            break;
        }
        let median = |values: &mut Vec<i64>| {
            values.sort_unstable();
            values[values.len() / 2]
        };
        let (mut first_ms, mut second_ms) = (first_ms, second_ms);
        let (short_app, short_ms) = {
            let (fm, sm) = (median(&mut first_ms), median(&mut second_ms));
            if fm <= sm {
                (&first, fm)
            } else {
                (&second, sm)
            }
        };
        out.push(
            "ping_pong",
            format!(
                "{first} 与 {second} 之间直接来回切换 {count} 次；每次进入 {short_app} 的停留中位数 {} 秒。",
                (short_ms as f64 / 1000.0).round()
            ),
        );
    }

    // Most fragmented hours.
    let mut hour_switches = [0_u32; 24];
    for pair in stays.windows(2) {
        if pair[0].app != pair[1].app
            && pair[1].start - pair[0].end <= CONTIGUOUS_GAP_MS
            && let Some(hour) = hour_of(pair[1].start)
        {
            hour_switches[hour] += 1;
        }
    }
    let mut fragmented = (0..24)
        .filter(|hour| stats.hourly.get(*hour).copied().unwrap_or(0) >= 20 * MINUTE)
        .map(|hour| (hour, hour_switches[hour]))
        .filter(|(_, count)| *count >= 20)
        .collect::<Vec<_>>();
    fragmented.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    if !fragmented.is_empty() {
        let text = fragmented
            .iter()
            .take(2)
            .map(|(hour, count)| {
                format!(
                    "{hour} 点（活跃 {} 分钟内切换 {count} 次）",
                    minutes(stats.hourly[*hour])
                )
            })
            .collect::<Vec<_>>();
        out.push(
            "fragmentation",
            format!("最碎片化的时段：{}。", text.join("、")),
        );
    }

    // Communication interruptions.
    let comm = stays
        .iter()
        .filter(|stay| is_communication(category_of(stay)))
        .collect::<Vec<_>>();
    if comm.len() >= 8 {
        let quick = comm
            .iter()
            .filter(|stay| stay.duration() < QUICK_CHECK_MS)
            .count();
        let inside_runs: u32 = runs.iter().map(|run| run.comm_interruptions).sum();
        let mut by_hour = [0_u32; 24];
        for stay in &comm {
            if let Some(hour) = hour_of(stay.start) {
                by_hour[hour] += 1;
            }
        }
        let peak = (0..24).max_by_key(|hour| by_hour[*hour]).unwrap_or(0);
        out.push(
            "communication",
            format!(
                "打开通讯应用 {} 次，共 {} 分钟；其中 {quick} 次不足 1 分钟，{inside_runs} 次发生在产出段中途；最频繁的是 {peak} 点（{} 次）。",
                comm.len(),
                metrics.comm_min,
                by_hour[peak]
            ),
        );
    }

    // Meetings and the fragments they leave behind.
    let meetings = meeting_blocks(stays);
    if metrics.meeting_min >= 15.0 {
        let back_to_back = meetings
            .windows(2)
            .filter(|pair| pair[1].0 - pair[0].1 < 10 * MINUTE)
            .count();
        let fragments = meetings
            .windows(2)
            .map(|pair| pair[1].0 - pair[0].1)
            .filter(|gap| (10 * MINUTE..=40 * MINUTE).contains(gap))
            .collect::<Vec<_>>();
        let list = meetings
            .iter()
            .take(6)
            .map(|(start, end)| format!("{}–{}", hhmm(*start), hhmm(*end)))
            .collect::<Vec<_>>();
        out.push(
            "meetings",
            format!(
                "会议 {} 段共 {} 分钟（{}）；{back_to_back} 处首尾间隔不足 10 分钟；会议之间夹着 {} 段 10–40 分钟的碎片时间，共 {} 分钟。",
                meetings.len(),
                metrics.meeting_min,
                list.join("、"),
                fragments.len(),
                minutes(fragments.iter().sum())
            ),
        );
    }

    // Peak productive window (history + today) versus what filled it today.
    let mut productive_by_hour = [0_i64; 24];
    let mut day_count = 0;
    for day in std::iter::once(stats).chain(history.iter().map(|day| day.stats)) {
        if day.total_active_ms < 30 * MINUTE {
            continue;
        }
        day_count += 1;
        for (hour, categories) in day.hourly_categories.iter().enumerate().take(24) {
            productive_by_hour[hour] += categories
                .iter()
                .filter(|(category, _)| is_productive(category))
                .map(|(_, ms)| ms)
                .sum::<i64>();
        }
    }
    if day_count >= 1 {
        let best = (0..23)
            .max_by_key(|hour| productive_by_hour[*hour] + productive_by_hour[hour + 1])
            .unwrap_or(9);
        let window_ms = productive_by_hour[best] + productive_by_hour[best + 1];
        if window_ms >= 30 * MINUTE * day_count as i64 / 2 {
            let mut used: BTreeMap<String, i64> = BTreeMap::new();
            for hour in best..best + 2 {
                for (category, ms) in stats.hourly_categories.get(hour).into_iter().flatten() {
                    if !is_productive(category) {
                        *used.entry(category.clone()).or_insert(0) += ms;
                    }
                }
            }
            let mut used = used
                .into_iter()
                .filter(|(_, ms)| *ms >= 5 * MINUTE)
                .collect::<Vec<_>>();
            used.sort_by_key(|(_, ms)| std::cmp::Reverse(*ms));
            let span = if day_count > 1 {
                format!("近 {day_count} 天")
            } else {
                "今天".to_owned()
            };
            out.push(
                "peak_hours",
                format!(
                    "{span}产出类工作最集中在 {best}–{} 点（{}{} 分钟）；今天这两个小时里非产出活动：{}。",
                    best + 2,
                    if day_count > 1 { "日均 " } else { "共 " },
                    minutes(window_ms / day_count as i64),
                    if used.is_empty() {
                        "无".to_owned()
                    } else {
                        used.iter()
                            .map(|(category, ms)| format!("{category} {} 分钟", minutes(*ms)))
                            .collect::<Vec<_>>()
                            .join("、")
                    }
                ),
            );
        }
    }

    // Long stretches without a break.
    if let Some((start, end)) = stretches(stays)
        .into_iter()
        .max_by_key(|(start, end)| end - start)
        .filter(|(start, end)| end - start >= 120 * MINUTE)
    {
        out.push(
            "no_break",
            format!(
                "{}–{} 连续活跃 {} 分钟，中间没有 ≥5 分钟的休息。",
                hhmm(start),
                hhmm(end),
                minutes(end - start)
            ),
        );
    }

    // Distractions and when they start.
    let distractions = stays
        .iter()
        .filter(|stay| is_distraction(category_of(stay)))
        .collect::<Vec<_>>();
    if metrics.distraction_min >= 15.0 {
        let work_hours = distractions
            .iter()
            .filter(|stay| hour_of(stay.start).is_some_and(|hour| (9..18).contains(&hour)))
            .count();
        let after_focus = distractions
            .iter()
            .filter(|stay| {
                runs.iter()
                    .any(|run| stay.start >= run.end && stay.start - run.end <= 2 * MINUTE)
            })
            .count();
        out.push(
            "distraction",
            format!(
                "娱乐与资讯共 {} 分钟，进入 {} 次；其中 {work_hours} 次在 9–18 点，{after_focus} 次紧接在一段产出之后。",
                metrics.distraction_min,
                distractions.len()
            ),
        );
    }

    if metrics.late_min >= 15.0 {
        let mut late: BTreeMap<String, i64> = BTreeMap::new();
        for hour in [23, 0, 1, 2, 3, 4] {
            for (category, ms) in stats.hourly_categories.get(hour).into_iter().flatten() {
                *late.entry(category.clone()).or_insert(0) += ms;
            }
        }
        let mut late = late.into_iter().collect::<Vec<_>>();
        late.sort_by_key(|(_, ms)| std::cmp::Reverse(*ms));
        out.push(
            "late_night",
            format!(
                "23 点后仍活跃 {} 分钟，主要是：{}。",
                metrics.late_min,
                late.iter()
                    .take(3)
                    .map(|(category, ms)| format!("{category} {} 分钟", minutes(*ms)))
                    .collect::<Vec<_>>()
                    .join("、")
            ),
        );
    }

    // Recurring work: the same window shows up day after day.
    if !history.is_empty() {
        let today = titles
            .iter()
            .filter(|(_, _, _, ms)| *ms >= 5 * MINUTE)
            .take(30)
            .collect::<Vec<_>>();
        let past = history
            .iter()
            .map(|day| {
                shareable_titles(day.stats)
                    .into_iter()
                    .map(|(app, key, _, ms)| ((app, key), ms))
                    .collect::<HashMap<_, _>>()
            })
            .collect::<Vec<_>>();
        let mut recurring = Vec::new();
        for (app, key, title, ms) in today {
            let seen = past
                .iter()
                .filter_map(|day| day.get(&(app.clone(), key.clone())))
                .collect::<Vec<_>>();
            if seen.len() >= 2 && key.chars().count() >= 3 {
                let average = (seen.iter().copied().sum::<i64>() + ms) / (seen.len() as i64 + 1);
                recurring.push(format!(
                    "{app}「{}」近 {} 天中 {} 天出现，日均 {} 分钟（今天 {} 分钟）",
                    ai_title("", title),
                    history.len() + 1,
                    seen.len() + 1,
                    minutes(average),
                    minutes(*ms)
                ));
            }
        }
        if !recurring.is_empty() {
            recurring.truncate(4);
            out.push(
                "recurring",
                format!("反复出现的工作：{}。", recurring.join("；")),
            );
        }
    }

    // Trends against the recent baseline.
    let baseline_days = history
        .iter()
        .filter(|day| day.metrics.active_min >= 30.0)
        .map(|day| day.metrics.clone())
        .collect::<Vec<_>>();
    if baseline_days.len() >= 3
        && let Some(baseline) = DayMetrics::average(&baseline_days)
    {
        let mut changes = Vec::new();
        for metric in METRICS
            .iter()
            .filter(|metric| metric.better != Better::Neutral)
        {
            let (Some(today), Some(base)) = (metrics.get(metric.key), baseline.get(metric.key))
            else {
                continue;
            };
            let floor = if metric.unit == "分钟" { 10.0 } else { 3.0 };
            if (today - base).abs() < floor {
                continue;
            }
            if let Some(change) = percent_change(today, base).filter(|change| change.abs() >= 25.0)
            {
                changes.push(format!(
                    "{} {today} {}（近 {} 天均值 {base}，{}{}%）",
                    metric.label,
                    metric.unit,
                    baseline_days.len(),
                    if change > 0.0 { "+" } else { "" },
                    change.round()
                ));
            }
        }
        if !changes.is_empty() {
            out.push("trend", format!("与近期相比：{}。", changes.join("；")));
        }
    }

    out.items
}

/// Every number that appears in the evidence, used to flag figures the model
/// may have invented.
pub fn known_numbers<'a>(texts: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    let mut numbers = BTreeSet::new();
    for text in texts {
        for number in extract_numbers(text) {
            numbers.insert(number);
        }
    }
    numbers
}

pub fn extract_numbers(text: &str) -> Vec<String> {
    let mut numbers = Vec::new();
    let mut current = String::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || (ch == '.' && !current.is_empty()) {
            current.push(ch);
        } else if !current.is_empty() {
            let number = current.trim_end_matches('.').to_owned();
            let normalized = number
                .parse::<f64>()
                .map(|value| {
                    if value.fract() == 0.0 {
                        format!("{}", value as i64)
                    } else {
                        format!("{value}")
                    }
                })
                .unwrap_or(number);
            numbers.push(normalized);
            current.clear();
        }
    }
    numbers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{DeriveOptions, Event, derive_events};
    use chrono::{Local, TimeZone};

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

    #[test]
    fn deep_work_tolerates_brief_chat_checks() {
        let mut events = vec![focus(at(9, 0), "code", "VS Code", "main.rs - TraceBit")];
        // 40 minutes of coding with a 30 second chat peek every 10 minutes.
        for block in 1..=4 {
            let ts = at(9, 0) + block * 10 * MINUTE;
            events.push(focus(ts - 30_000, "wechat", "微信", "张三"));
            events.push(focus(ts, "code", "VS Code", "main.rs - TraceBit"));
        }
        events.push(Event::at(at(9, 45), "lock", serde_json::json!({})));
        let stats = derive_events(&events, DeriveOptions::default(), None);
        let metrics = day_metrics(&stats);
        assert_eq!(metrics.deep_blocks, 1.0);
        assert!(metrics.deep_work_min >= 40.0, "{metrics:?}");
        assert_eq!(metrics.comm_checks, 4.0);
        let runs = productive_runs(&stats.stays);
        assert_eq!(runs[0].comm_interruptions, 4);
    }

    #[test]
    fn evidence_flags_ping_pong_and_hides_chat_titles() {
        let mut events = Vec::new();
        let mut ts = at(14, 0);
        for _ in 0..12 {
            events.push(focus(ts, "code", "VS Code", "lib.rs - app"));
            ts += 2 * MINUTE;
            events.push(focus(ts, "chrome", "Chrome", "docs.rs"));
            ts += 20_000;
        }
        for _ in 0..8 {
            events.push(focus(ts, "wechat", "微信", "李四的私聊"));
            ts += 20_000;
            events.push(focus(ts, "code", "VS Code", "lib.rs - app"));
            ts += 3 * MINUTE;
        }
        events.push(Event::at(ts, "lock", serde_json::json!({})));
        let stats = derive_events(&events, DeriveOptions::default(), None);
        let metrics = day_metrics(&stats);
        let evidence = build_evidence(&stats, &metrics, &[]);
        let all = evidence
            .iter()
            .map(|item| item.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            evidence.iter().any(|item| item.kind == "ping_pong"),
            "{all}"
        );
        assert!(
            evidence.iter().any(|item| item.kind == "communication"),
            "{all}"
        );
        assert!(!all.contains("李四"), "{all}");
        assert!(evidence.iter().all(|item| item.id.starts_with('E')));
    }

    #[test]
    fn numbers_are_extracted_and_normalized() {
        assert_eq!(
            extract_numbers("切换 46 次，12.5 分钟，3."),
            vec!["46", "12.5", "3"]
        );
    }
}
