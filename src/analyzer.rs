use crate::{
    categories::{categorize, is_meeting},
    recommendations::{
        Recommendation, SearchSummary, build_recommendations, recommendations_markdown,
    },
    settings::{AiSettings, Settings, SettingsData},
    store::{AppStat, DayStats, DayStore, DeriveOptions, hhmm, validate_date_key},
    web_search::enrich_recommendations,
};
use anyhow::{Context, Result, bail};
use chrono::{Datelike, NaiveDate};
use regex::Regex;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
    time::Duration,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TitleSummary {
    pub title: String,
    pub occurrences: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkeletonApp {
    pub app: String,
    pub category: String,
    pub minutes: i64,
    pub switches: u32,
    pub top_titles: Vec<TitleSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkeletonCategory {
    pub name: String,
    pub minutes: i64,
    pub pct: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingItem {
    pub app: String,
    pub minutes: i64,
    pub sample: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSummary {
    pub minutes: i64,
    pub count: usize,
    pub items: Vec<MeetingItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusSummary {
    pub app: String,
    pub minutes: i64,
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skeleton {
    pub date: String,
    pub weekday: String,
    pub first_activity: Option<String>,
    pub last_activity: Option<String>,
    pub total_active_minutes: i64,
    pub session_count: usize,
    pub categories: Vec<SkeletonCategory>,
    pub apps: Vec<SkeletonApp>,
    pub hourly_active_minutes: Vec<i64>,
    pub context_switches: u32,
    pub switches_per_active_hour: Option<f64>,
    pub longest_focus_block: Option<FocusSummary>,
    pub meetings: MeetingSummary,
    pub long_idle_breaks: usize,
    pub longest_idle_minutes: i64,
    pub screenshot_count: usize,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub date_key: String,
    pub markdown: String,
    pub model: String,
    pub generated_at: i64,
    pub active_minutes: i64,
    pub switch_count: u32,
    pub app_count: usize,
    #[serde(default)]
    pub recommendation_count: usize,
    #[serde(default)]
    pub recommendations: Vec<Recommendation>,
    #[serde(default)]
    pub resource_search: SearchSummary,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSummary {
    pub date_key: String,
    pub generated_at: i64,
    pub model: String,
    pub active_minutes: i64,
}

pub fn redact_text(value: &str) -> String {
    static EMAIL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[\w.+-]+@[\w-]+\.[\w.]+").unwrap());
    static KEY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\b(?:sk-[A-Za-z0-9_-]{8,}|ghp_[A-Za-z0-9]{8,}|AKIA[0-9A-Z]{12,})\b").unwrap()
    });
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{16,19}\b").unwrap());
    let value = EMAIL.replace_all(value, "[邮箱]");
    let value = KEY.replace_all(&value, "[密钥]");
    let value = NUMBER.replace_all(&value, "[号码]");
    let chars = value.chars().count();
    let mut result = value.chars().take(120).collect::<String>();
    if chars > 120 {
        result.push('…');
    }
    result
}

fn summarize_titles(app: &AppStat, max: usize) -> Vec<TitleSummary> {
    static SEPARATOR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[-–—|·»]").unwrap());
    let mut entries = app
        .titles
        .iter()
        .filter(|(title, _)| !title.trim().is_empty())
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| b.1.cmp(a.1));
    let mut groups: Vec<(String, TitleSummary)> = Vec::new();
    for (title, count) in entries {
        let mut key = SEPARATOR
            .split(title)
            .next()
            .unwrap_or(title)
            .trim()
            .to_owned();
        if key.is_empty() {
            key = title.trim().to_owned();
        }
        key = key.chars().take(40).collect();
        if let Some((_, group)) = groups.iter_mut().find(|(group_key, _)| group_key == &key) {
            group.occurrences += *count;
        } else {
            groups.push((
                key,
                TitleSummary {
                    title: redact_text(title),
                    occurrences: *count,
                },
            ));
        }
    }
    groups.sort_by_key(|group| std::cmp::Reverse(group.1.occurrences));
    groups.truncate(max);
    groups.into_iter().map(|(_, group)| group).collect()
}

pub fn build_skeleton(stats: &DayStats, _settings: &SettingsData) -> Skeleton {
    let weekday = NaiveDate::parse_from_str(&stats.date_key, "%Y-%m-%d")
        .map(|date| {
            ["周日", "周一", "周二", "周三", "周四", "周五", "周六"]
                [date.weekday().num_days_from_sunday() as usize]
        })
        .unwrap_or("")
        .to_owned();
    let mut categories: BTreeMap<String, i64> = BTreeMap::new();
    let mut apps = stats
        .apps
        .iter()
        .map(|app| {
            let category = categorize(
                &app.name,
                &app.app,
                &app.titles.keys().next().cloned().unwrap_or_default(),
            )
            .to_owned();
            categories.insert(
                category.clone(),
                categories.get(&category).copied().unwrap_or(0) + app.ms,
            );
            (
                SkeletonApp {
                    app: redact_text(if app.app.is_empty() {
                        &app.name
                    } else {
                        &app.app
                    }),
                    category,
                    minutes: rounded_minutes(app.ms),
                    switches: app.switches,
                    top_titles: summarize_titles(app, 3),
                },
                app.ms,
            )
        })
        .collect::<Vec<_>>();
    apps.sort_by_key(|app| std::cmp::Reverse(app.1));
    let mut categories = categories
        .into_iter()
        .map(|(name, ms)| SkeletonCategory {
            name,
            minutes: rounded_minutes(ms),
            pct: (ms as f64 / stats.total_active_ms.max(1) as f64 * 100.0).round() as i64,
        })
        .collect::<Vec<_>>();
    categories.sort_by_key(|category| std::cmp::Reverse(category.minutes));

    let mut meeting_items: BTreeMap<String, MeetingItem> = BTreeMap::new();
    for stay in &stats.stays {
        if is_meeting(&stay.name, &stay.app, &stay.title) && stay.end - stay.start >= 60_000 {
            let key = if stay.name.is_empty() {
                &stay.app
            } else {
                &stay.name
            }
            .clone();
            let item = meeting_items.entry(key).or_insert_with(|| MeetingItem {
                app: if stay.app.is_empty() {
                    stay.name.clone()
                } else {
                    stay.app.clone()
                },
                minutes: 0,
                sample: redact_text(&stay.title),
            });
            item.minutes += rounded_minutes(stay.end - stay.start);
        }
    }
    let mut meeting_items = meeting_items.into_values().collect::<Vec<_>>();
    meeting_items.sort_by_key(|meeting| std::cmp::Reverse(meeting.minutes));
    let meeting_minutes = meeting_items.iter().map(|item| item.minutes).sum();
    let active_hours = stats.total_active_ms as f64 / 3_600_000.0;

    Skeleton {
        date: stats.date_key.clone(),
        weekday,
        first_activity: stats
            .first_active
            .map(hhmm)
            .filter(|value| !value.is_empty()),
        last_activity: stats
            .last_active
            .map(hhmm)
            .filter(|value| !value.is_empty()),
        total_active_minutes: rounded_minutes(stats.total_active_ms),
        session_count: stats.sessions.len(),
        categories,
        apps: apps.into_iter().take(14).map(|(app, _)| app).collect(),
        hourly_active_minutes: stats.hourly.iter().map(|ms| rounded_minutes(*ms)).collect(),
        context_switches: stats.switches,
        switches_per_active_hour: (active_hours > 0.1)
            .then(|| (stats.switches as f64 / active_hours * 10.0).round() / 10.0),
        longest_focus_block: stats.longest_block.as_ref().map(|block| FocusSummary {
            app: redact_text(&block.app),
            minutes: rounded_minutes(block.ms),
            started_at: hhmm(block.started_at),
        }),
        meetings: MeetingSummary {
            minutes: meeting_minutes,
            count: meeting_items.len(),
            items: meeting_items,
        },
        long_idle_breaks: stats.long_idles,
        longest_idle_minutes: rounded_minutes(stats.longest_idle_ms),
        screenshot_count: stats.screenshots.len(),
        notes: "所有标题已截断并脱敏；时间为本地时间。".to_owned(),
    }
}

fn rounded_minutes(ms: i64) -> i64 {
    (ms as f64 / 60_000.0).round() as i64
}

pub fn skeleton_markdown(skeleton: &Skeleton) -> String {
    let mut lines = vec![
        format!("日期：{}（{}）", skeleton.date, skeleton.weekday),
        format!(
            "首次活动：{}　最后活动：{}",
            skeleton.first_activity.as_deref().unwrap_or("无"),
            skeleton.last_activity.as_deref().unwrap_or("无")
        ),
        format!(
            "总活跃时长：{} 分钟，活动段数：{}",
            skeleton.total_active_minutes, skeleton.session_count
        ),
        format!(
            "应用切换次数：{}{}",
            skeleton.context_switches,
            skeleton
                .switches_per_active_hour
                .map(|rate| format!("（每小时 {rate} 次）"))
                .unwrap_or_default()
        ),
    ];
    if let Some(block) = &skeleton.longest_focus_block {
        lines.push(format!(
            "最长专注块：{} {} 分钟（始于 {}）",
            block.app, block.minutes, block.started_at
        ));
    }
    lines.extend([String::new(), "分类时长（分钟 / 占比）：".to_owned()]);
    for category in &skeleton.categories {
        lines.push(format!(
            "- {}: {} 分钟（{}%）",
            category.name, category.minutes, category.pct
        ));
    }
    lines.extend([String::new(), "应用明细（分钟）：".to_owned()]);
    for app in &skeleton.apps {
        lines.push(format!(
            "- [{}] {} — {} 分钟，切换 {} 次",
            app.category, app.app, app.minutes, app.switches
        ));
        for title in &app.top_titles {
            lines.push(format!("    · \"{}\" ×{}", title.title, title.occurrences));
        }
    }
    lines.push(String::new());
    if skeleton.meetings.count > 0 {
        lines.push(format!("会议：共 {} 分钟", skeleton.meetings.minutes));
        for meeting in &skeleton.meetings.items {
            lines.push(format!(
                "- {} {} 分钟（如 \"{}\"）",
                meeting.app, meeting.minutes, meeting.sample
            ));
        }
    } else {
        lines.push("会议：无".to_owned());
    }
    lines.extend([
        String::new(),
        format!(
            "每小时活跃分钟（0-23 点）：{}",
            skeleton
                .hourly_active_minutes
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        format!(
            "长休息（≥5分钟）次数：{}，最长休息：{} 分钟",
            skeleton.long_idle_breaks, skeleton.longest_idle_minutes
        ),
    ]);
    lines.join("\n")
}

pub const SYSTEM_PROMPT: &str = r#"你是一名注重实证的个人效率与行为分析教练。用户会给你一份由本地软件记录的、某一天电脑使用行为的结构化数据（已脱敏）。

你的任务：写一份「行为日报」，帮助用户理解自己的一天，并给出具体、可执行的省力优化建议。

硬性规则：
- 只依据记录中的事实。不得编造任何数字、应用名、时间、事件。数据里没有的就不要写。
- 语言：简体中文。语气简洁、专业、克制，不堆砌形容词，不使用 emoji。
- 直接输出 Markdown 正文，不要代码块包裹，不要开场白和结尾客套。

输出结构（严格遵守）：

# {日期} 行为日报

## 一、今日概览
3-5 句话：活跃时长、主要时间去向、一天的整体节奏（上午/下午/晚间分别做了什么类型的事）。

## 二、时间去哪儿了
- 用列表列出各分类的时长与占比（只列数据中存在的分类）。
- 指出 1-2 个最突出的时间消耗点，引用具体数字。

## 三、行为模式观察
3-5 条基于数据的观察，例如：切换频率与碎片化、长时间会议、深夜使用、某个应用反复进出、专注块长度等。每条必须引用具体数字（时长、次数、时间点）。

## 四、省力优化建议
3-6 条建议，按预期收益从高到低排序。每条格式：

**建议标题**
- 现象：（引用数据中的具体事实）
- 做法：（今天就可以执行的具体动作，不要空话）
- 预期收益：（估算能省下的时间或精力，说明依据）

要求：建议必须针对记录中真实出现的模式。禁止没有任何数据支撑的泛泛建议（如“多休息”“做好时间管理”）。如果数据中存在明显的重复性操作（如反复切换某两个应用、频繁进出同一网站/工具），优先给出消除或自动化该重复的具体方案。

## 五、明日一个实验
给出一个 24 小时内可验证的小实验（具体做法 + 如何判断有效），用于验证上面某条建议。"#;

pub fn chat_completion(
    ai: &AiSettings,
    messages: Value,
    max_tokens: u32,
    timeout_ms: u64,
) -> Result<(String, String)> {
    static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/v\d+(/|$)").unwrap());
    let mut base = ai.base_url.trim().trim_end_matches('/').to_owned();
    if base.is_empty() {
        bail!("未配置 AI 接口地址");
    }
    if !VERSION.is_match(&base) && !base.ends_with("/chat/completions") {
        base.push_str("/v1");
    }
    let url = if base.ends_with("/chat/completions") {
        base
    } else {
        format!("{base}/chat/completions")
    };
    if ai.model.trim().is_empty() {
        bail!("未配置模型名称");
    }
    let client = Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()?;
    let mut request = client.post(url).json(&json!({
        "model": ai.model,
        "messages": messages,
        "temperature": 0.3,
        "max_tokens": max_tokens,
    }));
    if !ai.api_key.is_empty() {
        request = request.bearer_auth(&ai.api_key);
    }
    let response = request.send().context("无法连接 AI 接口")?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().unwrap_or_default();
        bail!(
            "AI 接口返回 {status}：{}",
            text.chars().take(200).collect::<String>()
        );
    }
    let data: Value = response.json().context("AI 返回格式无效")?;
    let content = data
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if content.is_empty() {
        bail!("AI 返回内容为空");
    }
    let model = data
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(&ai.model)
        .to_owned();
    Ok((content.to_owned(), model))
}

#[derive(Debug)]
pub struct Analyzer {
    settings: Arc<Settings>,
    store: Arc<DayStore>,
    reports_dir: PathBuf,
}

impl Analyzer {
    pub fn new(
        settings: Arc<Settings>,
        store: Arc<DayStore>,
        reports_dir: impl AsRef<Path>,
    ) -> Result<Self> {
        let reports_dir = reports_dir.as_ref().to_path_buf();
        fs::create_dir_all(&reports_dir)?;
        Ok(Self {
            settings,
            store,
            reports_dir,
        })
    }

    pub fn build_skeleton(&self, date_key: &str) -> Result<(DayStats, Skeleton)> {
        let settings = self.settings.get();
        let stats = self.store.day_stats(
            date_key,
            DeriveOptions {
                idle_threshold_ms: settings.recording.idle_threshold_ms,
                session_gap_ms: settings.recording.session_gap_ms,
            },
        )?;
        let skeleton = build_skeleton(&stats, &settings);
        Ok((stats, skeleton))
    }

    pub fn report_file(&self, date_key: &str) -> Result<PathBuf> {
        validate_date_key(date_key)?;
        Ok(self.reports_dir.join(format!("{date_key}.json")))
    }

    pub fn get_report(&self, date_key: &str) -> Option<Report> {
        let file = self.report_file(date_key).ok()?;
        fs::read_to_string(file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
    }

    pub fn list_reports(&self) -> Vec<ReportSummary> {
        let mut reports = fs::read_dir(&self.reports_dir)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let date_key = name.strip_suffix(".json")?;
                validate_date_key(date_key).ok()?;
                let report: Report =
                    serde_json::from_str(&fs::read_to_string(entry.path()).ok()?).ok()?;
                Some(ReportSummary {
                    date_key: date_key.to_owned(),
                    generated_at: report.generated_at,
                    model: report.model,
                    active_minutes: report.active_minutes,
                })
            })
            .collect::<Vec<_>>();
        reports.sort_by(|a, b| b.date_key.cmp(&a.date_key));
        reports
    }

    pub fn generate_report(&self, date_key: &str) -> Result<Report> {
        let ai = self.settings.get().ai;
        let (stats, skeleton) = self.build_skeleton(date_key)?;
        if stats.events == 0 {
            bail!("当天没有记录数据");
        }
        if stats.total_active_ms < 60_000 {
            bail!("当天活跃时长不足 1 分钟，无法生成有意义的报告");
        }
        let user_prompt = format!(
            "以下是用户 {date_key} 的电脑使用行为记录（本地记录、已脱敏）：\n\n{}\n\n请依据以上数据写出这一天的行为日报。",
            skeleton_markdown(&skeleton)
        );
        let (text, model) = chat_completion(
            &ai,
            json!([
                {"role":"system", "content":SYSTEM_PROMPT},
                {"role":"user", "content":user_prompt}
            ]),
            2048,
            120_000,
        )?;
        let enriched = enrich_recommendations(&build_recommendations(&stats), &ai);
        let tool_markdown = recommendations_markdown(&enriched.recommendations, &enriched.search);
        let markdown = if tool_markdown.is_empty() {
            text
        } else {
            format!("{text}\n\n{tool_markdown}")
        };
        let report = Report {
            date_key: date_key.to_owned(),
            markdown,
            model,
            generated_at: crate::store::now_ms(),
            active_minutes: rounded_minutes(stats.total_active_ms),
            switch_count: stats.switches,
            app_count: stats.apps.len(),
            recommendation_count: enriched.recommendations.len(),
            recommendations: enriched.recommendations,
            resource_search: enriched.search,
        };
        fs::write(
            self.report_file(date_key)?,
            serde_json::to_vec_pretty(&report)?,
        )?;
        Ok(report)
    }

    pub fn test_connection(&self) -> Result<(String, String)> {
        chat_completion(
            &self.settings.get().ai,
            json!([{"role":"user", "content":"请只回复两个字：正常"}]),
            16,
            30_000,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Event;
    use chrono::{Local, TimeZone};
    use serde_json::json;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    #[test]
    fn redaction_removes_common_secrets() {
        let value =
            redact_text("mail secret@example.com key sk-1234567890abcdef card 1234567890123456");
        assert!(!value.contains("secret@example.com"));
        assert!(!value.contains("sk-1234567890abcdef"));
        assert!(!value.contains("1234567890123456"));
        assert!(value.contains("[邮箱]"));
        assert!(value.contains("[密钥]"));
        assert!(value.contains("[号码]"));
    }

    #[test]
    fn ai_report_end_to_end_redacts_and_persists() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut raw = Vec::new();
            let mut buffer = [0_u8; 8192];
            let expected = loop {
                let count = socket.read(&mut buffer).unwrap();
                if count == 0 {
                    break None;
                }
                raw.extend_from_slice(&buffer[..count]);
                if let Some(header_end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&raw[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if raw.len() >= header_end + 4 + length {
                        break Some(header_end + 4 + length);
                    }
                }
            };
            if let Some(expected) = expected {
                tx.send(String::from_utf8_lossy(&raw[..expected]).into_owned())
                    .unwrap();
            }
            let body = json!({
                "model":"mock-model",
                "choices":[{"message":{"content":"# 行为日报\n\n## 一、今日概览\n测试。\n\n## 四、省力优化建议\n测试建议。"}}]
            }).to_string();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });

        let temp = tempfile::tempdir().unwrap();
        let user_dir = temp.path().join("user");
        let store = Arc::new(DayStore::new(temp.path().join("data")).unwrap());
        let settings = Arc::new(Settings::load(&user_dir));
        settings.patch(&json!({"ai": {
            "baseUrl": format!("http://{address}/v1"), "apiKey":"mock-key", "model":"mock-model",
            "webSearch":false, "verifySources":false
        }})).unwrap();
        let date = Local::now().date_naive();
        let noon = Local
            .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp_millis();
        let date_key = date.format("%Y-%m-%d").to_string();
        for index in 0..8 {
            let code = index % 2 == 0;
            store
                .append(Event::app_focus(
                    noon + index * 60_000,
                    if code { "VS Code" } else { "PowerShell" },
                    if code { "code" } else { "powershell" },
                    if code {
                        "secret@example.com - git commit"
                    } else {
                        "git status"
                    },
                    if code { 1 } else { 2 },
                    1_000,
                ))
                .unwrap();
        }
        store
            .append(Event::at(
                noon + 60_000,
                "site_visit",
                json!({"site":"private.example.com", "app":"Chrome"}),
            ))
            .unwrap();
        store
            .append(Event::at(noon + 8 * 60_000, "tick", json!({"i":1000})))
            .unwrap();
        let analyzer = Analyzer::new(settings, store, temp.path().join("reports")).unwrap();
        let report = analyzer.generate_report(&date_key).unwrap();
        let request = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(request.contains("mock-model"));
        assert!(request.contains("Bearer mock-key"));
        assert!(!request.contains("secret@example.com"));
        assert!(request.contains("[邮箱]"));
        assert!(!request.contains("private.example.com"));
        assert_eq!(report.recommendation_count, 1);
        assert!(report.markdown.contains("lazygit"));
        assert!(report.markdown.contains("## 六、可工具化的操作"));
        assert!(analyzer.get_report(&date_key).is_some());
    }
}
