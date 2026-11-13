//! The advisor: local evidence + AI reasoning -> prioritized, verifiable advice.
//!
//! Pipeline: day stats (+7 days of history) -> deterministic metrics and numbered
//! evidence (`insights`) -> screenshot interval summaries -> rule-based tool
//! candidates -> one JSON-mode model call -> validation against the evidence ->
//! locally rendered Markdown. Each report stores a measurable experiment that the
//! next report scores automatically, closing the loop.

use crate::{
    insights::{
        self, DayMetrics, Evidence, History, METRICS, build_evidence, day_metrics, known_numbers,
        metric_def,
    },
    intervals::read_summaries,
    llm::{self, ChatOptions, Usage},
    recommendations::{Recommendation, SearchSummary, build_recommendations},
    redact::redact_with_limit,
    settings::Settings,
    store::{DayStats, DayStore, DeriveOptions, hhmm, local_datetime, now_ms, validate_date_key},
    web_search::enrich_recommendations,
};
use anyhow::{Result, bail};
use chrono::{Duration as ChronoDuration, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

pub use crate::redact::redact_text;

pub const PROMPT_VERSION: &str = "advice-v2";
const HISTORY_DAYS: i64 = 7;
const SUGGESTION_TYPES: [&str; 5] = ["习惯", "日程", "环境", "工具", "自动化"];
const LEVELS: [&str; 3] = ["低", "中", "高"];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Finding {
    #[serde(alias = "text")]
    pub finding: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Suggestion {
    pub title: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub evidence: Vec<String>,
    pub why: String,
    pub actions: Vec<String>,
    pub when: String,
    pub expected: String,
    pub effort: String,
    pub confidence: String,
    pub tools: Vec<String>,
    /// Figures the model stated that do not appear in the evidence.
    pub unverified_numbers: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Experiment {
    pub hypothesis: String,
    pub action: String,
    pub metric: String,
    pub target: f64,
    /// Metric value on the day the experiment was proposed.
    pub baseline: Option<f64>,
    pub date_key: String,
}

impl Experiment {
    fn wants_increase(&self) -> bool {
        self.baseline.is_none_or(|baseline| self.target >= baseline)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ExperimentReview {
    pub experiment: Experiment,
    pub actual: Option<f64>,
    pub achieved: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Advice {
    pub headline: String,
    pub diagnosis: Vec<Finding>,
    pub suggestions: Vec<Suggestion>,
    pub experiment: Option<Experiment>,
    pub followup: String,
    pub unverified_numbers: Vec<String>,
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
    #[serde(default)]
    pub advice: Option<Advice>,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    pub metrics: Option<DayMetrics>,
    #[serde(default)]
    pub review: Option<ExperimentReview>,
    /// "manual" or "auto"; automatic runs never overwrite a manual report.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub prompt_version: String,
    /// SHA-1 of the exact prompt, to compare runs across prompt changes.
    #[serde(default)]
    pub input_hash: String,
    #[serde(default)]
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSummary {
    pub date_key: String,
    pub generated_at: i64,
    pub model: String,
    pub active_minutes: i64,
}

/// Everything the model sees, built deterministically so it can be previewed.
pub struct AdviceContext {
    pub stats: DayStats,
    pub metrics: DayMetrics,
    pub evidence: Vec<Evidence>,
    pub recommendations: Vec<Recommendation>,
    pub review: Option<ExperimentReview>,
    pub system_prompt: String,
    pub user_prompt: String,
}

pub const SYSTEM_PROMPT: &str = r#"你是一位严谨、务实的个人效率顾问。你会拿到某人一天的电脑使用证据（由本地软件计算并脱敏）、近期趋势、截图时段摘要、上一次实验的结果和可用工具候选。

你的唯一目标：找出最值得改变的 1–3 个行为模式，给出明天就能执行、能明显省时间或精力的具体建议。不要写流水账式日报，不要复述数据。

思考方法：
1. 找根因，不停在现象。组合多条证据推断原因：同一对应用高频来回且停留很短，通常是在查找或搬运信息；产出段中途频繁打开通讯，通常是消息通知在驱动注意力；会议之间留下 10–40 分钟碎片，通常是日程排布问题。
2. 排序依据 = 预期收益 × 把握 ÷ 执行成本。
3. 建议要具体到“什么时机、做什么动作、改什么设置或用什么工具”。例如“把微信设为免打扰，每天 11:30 和 17:00 集中回复”，而不是“减少分心”。
4. 改环境比靠意志力有效：至少一条建议属于 环境、工具 或 自动化 类型。
5. 如果给出了上一次实验的结果：达成就建议固化并提高一点要求；未达成就分析可能原因并调整做法，不要原样重复。
6. 证据不足以支撑的推断，confidence 设为“低”，并在 why 里说明需要验证什么。

硬性规则：
- 每条发现和建议都必须引用给出的证据编号（如 E1、S2），不得引用不存在的编号。
- 不得编造证据中没有的数字、应用或时间。估算收益时写明依据了哪个证据中的数字。
- tools 只能填写工具候选的编号；没有合适的就填空数组。不要自己写工具名或链接。
- 不评价人格，不说教，不写“多休息”“做好时间管理”这类空话。
- 简体中文。只输出一个 JSON 对象，不要 Markdown，不要代码块。"#;

fn output_contract(metrics: &DayMetrics) -> String {
    let keys = METRICS
        .iter()
        .map(|metric| {
            format!(
                "{}（{}，今天 {} {}）",
                metric.key,
                metric.label,
                metrics.get(metric.key).unwrap_or(0.0),
                metric.unit
            )
        })
        .collect::<Vec<_>>()
        .join("；");
    format!(
        r#"输出 JSON 结构（字段名必须一致）：
{{
  "headline": "一句话指出最值得改变的一件事，不超过 40 字",
  "diagnosis": [{{"finding": "根因层面的发现", "evidence": ["E1"]}}],
  "suggestions": [{{
    "title": "动词开头，不超过 20 字",
    "type": "习惯|日程|环境|工具|自动化",
    "evidence": ["E2", "S1"],
    "why": "为什么能解决根因",
    "actions": ["第一步具体动作", "第二步"],
    "when": "何时开始，或在什么触发条件下执行",
    "expected": "预期收益和推算依据",
    "effort": "低|中|高",
    "confidence": "高|中|低",
    "tools": ["工具候选编号"]
  }}],
  "experiment": {{"hypothesis": "要验证的假设", "action": "明天具体做什么", "metric": "指标键名", "target": 0}},
  "followup": "对上一次实验结果的解读和调整；没有上次实验则为空字符串"
}}
diagnosis 2–4 条；suggestions 3–5 条，按优先级排序；actions 1–4 步。
experiment 必须对应排名第一或第二的建议，metric 只能取以下键之一，target 是明天要达到的数值：{keys}"#
    )
}

fn week_label(date_key: &str) -> String {
    NaiveDate::parse_from_str(date_key, "%Y-%m-%d")
        .map(|date| {
            use chrono::Datelike;
            ["周日", "周一", "周二", "周三", "周四", "周五", "周六"]
                [date.weekday().num_days_from_sunday() as usize]
                .to_owned()
        })
        .unwrap_or_default()
}

fn fmt_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value:.1}")
    }
}

fn review_text(review: &ExperimentReview) -> String {
    let experiment = &review.experiment;
    let def = metric_def(&experiment.metric);
    let (label, unit) = def.map(|def| (def.label, def.unit)).unwrap_or(("指标", ""));
    let comparator = if experiment.wants_increase() {
        "≥"
    } else {
        "≤"
    };
    format!(
        "上次实验（{} 提出）：{}。目标 {label} {comparator} {}{unit}（当时 {}{unit}），今天实际 {}{unit}，{}。",
        experiment.date_key,
        experiment.action,
        fmt_number(experiment.target),
        experiment
            .baseline
            .map(fmt_number)
            .unwrap_or_else(|| "未知".to_owned()),
        review
            .actual
            .map(fmt_number)
            .unwrap_or_else(|| "无数据".to_owned()),
        match review.achieved {
            Some(true) => "已达成",
            Some(false) => "未达成",
            None => "无法判断",
        }
    )
}

fn build_user_prompt(
    date_key: &str,
    metrics: &DayMetrics,
    evidence: &[Evidence],
    summaries: &[Evidence],
    recommendations: &[Recommendation],
    review: Option<&ExperimentReview>,
) -> String {
    let mut lines = vec![format!(
        "日期：{date_key}（{}）。以下数据由本地软件记录并计算，窗口标题已脱敏，聊天窗口只保留应用名。",
        week_label(date_key)
    )];
    lines.push(String::new());
    lines.push("【证据】".to_owned());
    for item in evidence {
        lines.push(format!("{}：{}", item.id, item.text));
    }
    if !summaries.is_empty() {
        lines.push(String::new());
        lines.push("【截图时段摘要（视觉模型根据屏幕截图生成）】".to_owned());
        for item in summaries {
            lines.push(format!("{}：{}", item.id, item.text));
        }
    }
    lines.push(String::new());
    lines.push("【上一次实验】".to_owned());
    lines.push(review.map(review_text).unwrap_or_else(|| "无。".to_owned()));
    lines.push(String::new());
    lines.push("【工具候选】（本地规则根据证据匹配，只能按编号引用）".to_owned());
    if recommendations.is_empty() {
        lines.push("无。".to_owned());
    }
    for item in recommendations {
        lines.push(format!(
            "{}：{}。触发依据：{} 可选工具：{}。",
            item.id,
            item.title,
            item.evidence,
            item.resources
                .iter()
                .map(|resource| format!("{}（{}）", resource.name, resource.kind))
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    lines.push(String::new());
    lines.push(output_contract(metrics));
    lines.join("\n")
}

fn normalize_choice(value: &str, allowed: &[&str], fallback: &str) -> String {
    allowed
        .iter()
        .find(|choice| value.contains(**choice))
        .unwrap_or(&fallback)
        .to_string()
}

/// Numbers in factual claims that are not in the evidence. Small numbers (up to
/// 24: counts, hours, steps) are too ambiguous to flag.
fn unverified(text: &str, known: &BTreeSet<String>) -> Vec<String> {
    insights::extract_numbers(text)
        .into_iter()
        .filter(|number| number.parse::<f64>().is_ok_and(|value| value > 24.0))
        .filter(|number| !known.contains(number))
        .collect()
}

/// Drops everything that is not backed by the evidence and normalizes enums.
pub fn validate_advice(
    mut advice: Advice,
    evidence_ids: &BTreeSet<String>,
    tool_ids: &BTreeSet<String>,
    known: &BTreeSet<String>,
    metrics: &DayMetrics,
    date_key: &str,
) -> Result<Advice> {
    let keep_ids = |ids: &mut Vec<String>| {
        let mut seen = BTreeSet::new();
        ids.retain(|id| evidence_ids.contains(id.trim()) && seen.insert(id.trim().to_owned()));
        for id in ids.iter_mut() {
            *id = id.trim().to_owned();
        }
    };
    let mut flagged = Vec::new();
    advice.diagnosis.retain_mut(|finding| {
        keep_ids(&mut finding.evidence);
        flagged.extend(unverified(&finding.finding, known));
        !finding.evidence.is_empty() && !finding.finding.trim().is_empty()
    });
    advice.diagnosis.truncate(4);
    advice.suggestions.retain_mut(|suggestion| {
        keep_ids(&mut suggestion.evidence);
        suggestion
            .tools
            .retain(|tool| tool_ids.contains(tool.trim()));
        suggestion.kind = normalize_choice(&suggestion.kind, &SUGGESTION_TYPES, "习惯");
        suggestion.effort = normalize_choice(&suggestion.effort, &LEVELS, "中");
        suggestion.confidence = normalize_choice(&suggestion.confidence, &LEVELS, "中");
        suggestion.actions = suggestion
            .actions
            .iter()
            .map(|action| action.trim().to_owned())
            .filter(|action| !action.is_empty())
            .take(4)
            .collect();
        suggestion.unverified_numbers = unverified(&suggestion.why, known);
        !suggestion.evidence.is_empty()
            && !suggestion.title.trim().is_empty()
            && !suggestion.actions.is_empty()
    });
    advice.suggestions.truncate(5);
    if advice.suggestions.is_empty() {
        bail!("AI 输出中没有引用有效证据的建议，请重试或换用更强的模型");
    }
    flagged.sort();
    flagged.dedup();
    advice.unverified_numbers = flagged;
    advice.experiment = advice.experiment.take().and_then(|mut experiment| {
        let baseline = metrics.get(experiment.metric.trim())?;
        experiment.metric = experiment.metric.trim().to_owned();
        experiment.baseline = Some(baseline);
        experiment.date_key = date_key.to_owned();
        (!experiment.action.trim().is_empty() && experiment.target.is_finite())
            .then_some(experiment)
    });
    Ok(advice)
}

pub fn render_markdown(
    date_key: &str,
    advice: &Advice,
    metrics: &DayMetrics,
    evidence: &[Evidence],
    recommendations: &[Recommendation],
    search: &SearchSummary,
    review: Option<&ExperimentReview>,
) -> String {
    let mut lines = vec![
        format!("# {date_key} 效率建议"),
        String::new(),
        format!("> {}", advice.headline.trim()),
        String::new(),
        format!(
            "活跃 {} 分钟 · 深度工作 {} 分钟 · 每小时切换 {} 次 · 通讯 {} 次 · 会议 {} 分钟",
            fmt_number(metrics.active_min),
            fmt_number(metrics.deep_work_min),
            fmt_number(metrics.switches_per_hour),
            fmt_number(metrics.comm_checks),
            fmt_number(metrics.meeting_min)
        ),
    ];

    if let Some(review) = review {
        lines.extend([String::new(), "## 上次实验回顾".to_owned()]);
        lines.push(format!("- {}", review_text(review)));
        if !advice.followup.trim().is_empty() {
            lines.push(format!("- 解读：{}", advice.followup.trim()));
        }
    }

    if !advice.diagnosis.is_empty() {
        lines.extend([String::new(), "## 关键发现".to_owned()]);
        for finding in &advice.diagnosis {
            lines.push(format!(
                "- {}（依据 {}）",
                finding.finding.trim(),
                finding.evidence.join("、")
            ));
        }
        if !advice.unverified_numbers.is_empty() {
            lines.push(format!(
                "> 注意：以上发现中的数字 {} 无法在记录中核实，请谨慎参考。",
                advice.unverified_numbers.join("、")
            ));
        }
    }

    lines.extend([String::new(), "## 建议".to_owned()]);
    for (index, suggestion) in advice.suggestions.iter().enumerate() {
        lines.push(String::new());
        lines.push(format!("### {}. {}", index + 1, suggestion.title.trim()));
        lines.push(format!(
            "{} · 投入{} · 把握{} · 依据 {}",
            suggestion.kind,
            suggestion.effort,
            suggestion.confidence,
            suggestion.evidence.join("、")
        ));
        if !suggestion.why.trim().is_empty() {
            lines.push(format!("- 为什么：{}", suggestion.why.trim()));
        }
        lines.push("- 怎么做：".to_owned());
        for (step, action) in suggestion.actions.iter().enumerate() {
            lines.push(format!("    {}. {action}", step + 1));
        }
        if !suggestion.when.trim().is_empty() {
            lines.push(format!("- 何时：{}", suggestion.when.trim()));
        }
        if !suggestion.expected.trim().is_empty() {
            lines.push(format!("- 预期：{}", suggestion.expected.trim()));
        }
        for tool in &suggestion.tools {
            if let Some(item) = recommendations.iter().find(|item| &item.id == tool) {
                for resource in &item.resources {
                    lines.push(format!(
                        "- 工具：[{}]({})（{}）{} 配置成本：{} 注意：{}",
                        resource.name,
                        resource.url,
                        resource.kind,
                        resource.description,
                        resource.setup_cost,
                        resource.permissions
                    ));
                }
            }
        }
        if !suggestion.unverified_numbers.is_empty() {
            lines.push(format!(
                "> 注意：数字 {} 无法在记录中核实。",
                suggestion.unverified_numbers.join("、")
            ));
        }
    }

    if let Some(experiment) = &advice.experiment {
        let def = metric_def(&experiment.metric);
        let (label, unit) = def.map(|def| (def.label, def.unit)).unwrap_or(("指标", ""));
        lines.extend([String::new(), "## 明日实验".to_owned()]);
        lines.push(format!("- 假设：{}", experiment.hypothesis.trim()));
        lines.push(format!("- 做法：{}", experiment.action.trim()));
        lines.push(format!(
            "- 衡量：{label} 从今天的 {}{unit} {}到 {}{unit}。下一份建议会自动核对结果。",
            experiment.baseline.map(fmt_number).unwrap_or_default(),
            if experiment.wants_increase() {
                "提高"
            } else {
                "降低"
            },
            fmt_number(experiment.target)
        ));
    }

    let used = advice
        .suggestions
        .iter()
        .flat_map(|suggestion| suggestion.tools.iter())
        .collect::<BTreeSet<_>>();
    let unused = recommendations
        .iter()
        .filter(|item| !used.contains(&item.id))
        .collect::<Vec<_>>();
    if !unused.is_empty() {
        lines.extend([String::new(), "## 其他可评估的工具".to_owned()]);
        for item in unused {
            lines.push(format!("### {}", item.title));
            lines.push(format!("- 依据：{}", item.evidence));
            for resource in &item.resources {
                lines.push(format!(
                    "- [{}]({})（{}）{}",
                    resource.name, resource.url, resource.kind, resource.description
                ));
            }
        }
    }
    if search.enabled {
        lines.push(format!(
            "> 联网检索：{} 次抽象关键词查询，新增 {} 个候选，验证 {} 个链接；搜索服务不会收到行为数据。",
            search.queries, search.added, search.verified
        ));
    }

    lines.extend([String::new(), "## 依据".to_owned()]);
    for item in evidence {
        lines.push(format!("- {}：{}", item.id, item.text));
    }
    lines.push(String::new());
    lines.push("工具只供人工评估，TraceBit 不会自动下载、安装或启用。".to_owned());
    lines.join("\n")
}

#[derive(Debug)]
pub struct Analyzer {
    settings: Arc<Settings>,
    store: Arc<DayStore>,
    reports_dir: PathBuf,
    intervals_dir: Option<PathBuf>,
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
            intervals_dir: None,
        })
    }

    /// Screenshot interval summaries from this directory feed into the advice.
    pub fn with_intervals(mut self, directory: impl AsRef<Path>) -> Self {
        self.intervals_dir = Some(directory.as_ref().to_path_buf());
        self
    }

    fn options(&self) -> DeriveOptions {
        DeriveOptions::from_settings(&self.settings.get().recording)
    }

    pub fn report_file(&self, date_key: &str) -> Result<PathBuf> {
        validate_date_key(date_key)?;
        Ok(self.reports_dir.join(format!("{date_key}.json")))
    }

    /// Writes the report as a standalone HTML page next to its JSON and returns
    /// the path. Rendered from the stored JSON, so older reports work too.
    pub fn write_html(&self, report: &Report) -> Result<PathBuf> {
        validate_date_key(&report.date_key)?;
        let file = self.reports_dir.join(format!("{}.html", report.date_key));
        fs::write(&file, crate::report_html::render(report))?;
        Ok(file)
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

    fn history(&self, date_key: &str) -> Vec<DayStats> {
        let Ok(date) = NaiveDate::parse_from_str(date_key, "%Y-%m-%d") else {
            return Vec::new();
        };
        let options = self.options();
        (1..=HISTORY_DAYS)
            .filter_map(|offset| {
                let key = (date - ChronoDuration::days(offset))
                    .format("%Y-%m-%d")
                    .to_string();
                self.store
                    .day_stats(&key, options)
                    .ok()
                    .filter(|stats| stats.total_active_ms > 0)
            })
            .collect()
    }

    /// Scores the most recent earlier experiment against today's metrics.
    fn review_previous(&self, date_key: &str, metrics: &DayMetrics) -> Option<ExperimentReview> {
        let date = NaiveDate::parse_from_str(date_key, "%Y-%m-%d").ok()?;
        (1..=HISTORY_DAYS).find_map(|offset| {
            let key = (date - ChronoDuration::days(offset))
                .format("%Y-%m-%d")
                .to_string();
            let experiment = self.get_report(&key)?.advice?.experiment?;
            let actual = metrics.get(&experiment.metric);
            let achieved = actual.map(|actual| {
                if experiment.wants_increase() {
                    actual >= experiment.target
                } else {
                    actual <= experiment.target
                }
            });
            Some(ExperimentReview {
                experiment,
                actual,
                achieved,
            })
        })
    }

    fn summaries(&self, date_key: &str) -> Vec<Evidence> {
        let Some(directory) = &self.intervals_dir else {
            return Vec::new();
        };
        read_summaries(directory, date_key)
            .into_iter()
            .enumerate()
            .map(|(index, summary)| Evidence {
                id: format!("S{}", index + 1),
                kind: "interval".to_owned(),
                text: format!(
                    "{}–{} {}",
                    hhmm(summary.start),
                    hhmm(summary.end),
                    redact_with_limit(&summary.summary, 240)
                ),
            })
            .collect()
    }

    /// Builds the exact prompt without calling any model.
    pub fn build_context(&self, date_key: &str) -> Result<AdviceContext> {
        validate_date_key(date_key)?;
        let stats = self.store.day_stats(date_key, self.options())?;
        let metrics = day_metrics(&stats);
        let history_stats = self.history(date_key);
        let history_metrics = history_stats.iter().map(day_metrics).collect::<Vec<_>>();
        let history = history_stats
            .iter()
            .zip(&history_metrics)
            .map(|(stats, metrics)| History { stats, metrics })
            .collect::<Vec<_>>();
        let mut evidence = build_evidence(&stats, &metrics, &history);
        let review = self.review_previous(date_key, &metrics);
        if let Some(review) = &review {
            evidence.push(Evidence {
                id: format!("E{}", evidence.len() + 1),
                kind: "experiment".to_owned(),
                text: review_text(review),
            });
        }
        let summaries = self.summaries(date_key);
        let recommendations = build_recommendations(&stats);
        let user_prompt = build_user_prompt(
            date_key,
            &metrics,
            &evidence,
            &summaries,
            &recommendations,
            review.as_ref(),
        );
        evidence.extend(summaries);
        Ok(AdviceContext {
            stats,
            metrics,
            evidence,
            recommendations,
            review,
            system_prompt: SYSTEM_PROMPT.to_owned(),
            user_prompt,
        })
    }

    /// What would be sent to the model, for the "preview" button.
    pub fn preview_prompt(&self, date_key: &str) -> Result<String> {
        let context = self.build_context(date_key)?;
        Ok(format!(
            "【系统提示】\n{}\n\n【发送的数据】\n{}",
            context.system_prompt, context.user_prompt
        ))
    }

    pub fn generate_report(&self, date_key: &str) -> Result<Report> {
        self.generate_report_as(date_key, "manual")
    }

    pub fn generate_report_as(&self, date_key: &str, source: &str) -> Result<Report> {
        let ai = self.settings.get().ai;
        let endpoint = ai.text_endpoint();
        let context = self.build_context(date_key)?;
        if context.stats.events == 0 {
            bail!("当天没有记录数据");
        }
        if context.stats.total_active_ms < 60_000 {
            bail!("当天活跃时长不足 1 分钟，无法生成有意义的建议");
        }
        let messages = json!([
            {"role": "system", "content": context.system_prompt},
            {"role": "user", "content": context.user_prompt},
        ]);
        let options = ChatOptions {
            max_tokens: 3_000,
            timeout_ms: 180_000,
            temperature: 0.4,
            json: true,
        };
        let first = llm::chat(&endpoint, messages.clone(), options)?;
        let mut usage = first.usage.clone();
        let mut model = first.model.clone();
        let parsed = parse_advice(&first.content);
        let advice = match parsed {
            Some(advice) => advice,
            None if first.truncated => {
                bail!("AI 输出被截断，请换用上下文更长或非推理模型");
            }
            None => {
                // One repair round: show the model its own output.
                let mut repair = messages.as_array().cloned().unwrap_or_default();
                repair.push(json!({"role": "assistant", "content": first.content}));
                repair.push(json!({"role": "user", "content": "上面的输出不是符合要求结构的 JSON。请只输出那个 JSON 对象，不要任何其他文字。"}));
                let second = llm::chat(&endpoint, Value::Array(repair), options)?;
                usage.prompt_tokens += second.usage.prompt_tokens;
                usage.completion_tokens += second.usage.completion_tokens;
                model = second.model.clone();
                parse_advice(&second.content)
                    .ok_or_else(|| anyhow::anyhow!("AI 两次都没有返回有效的 JSON 建议"))?
            }
        };
        let evidence_ids = context
            .evidence
            .iter()
            .map(|item| item.id.clone())
            .collect::<BTreeSet<_>>();
        let tool_ids = context
            .recommendations
            .iter()
            .map(|item| item.id.clone())
            .collect::<BTreeSet<_>>();
        let mut known = known_numbers(context.evidence.iter().map(|item| item.text.as_str()));
        for metric in &METRICS {
            if let Some(value) = context.metrics.get(metric.key) {
                known.insert(fmt_number(value));
            }
        }
        let advice = validate_advice(
            advice,
            &evidence_ids,
            &tool_ids,
            &known,
            &context.metrics,
            date_key,
        )?;
        let enriched = enrich_recommendations(&context.recommendations, &ai);
        let markdown = render_markdown(
            date_key,
            &advice,
            &context.metrics,
            &context.evidence,
            &enriched.recommendations,
            &enriched.search,
            context.review.as_ref(),
        );
        let input_hash = format!(
            "{:x}",
            Sha1::digest(format!(
                "{}\n{}",
                context.system_prompt, context.user_prompt
            ))
        );
        let report = Report {
            date_key: date_key.to_owned(),
            markdown,
            model,
            generated_at: now_ms(),
            active_minutes: context.metrics.active_min as i64,
            switch_count: context.stats.switches,
            app_count: context.stats.apps.len(),
            recommendation_count: enriched.recommendations.len(),
            recommendations: enriched.recommendations,
            resource_search: enriched.search,
            advice: Some(advice),
            evidence: context.evidence,
            metrics: Some(context.metrics),
            review: context.review,
            source: source.to_owned(),
            prompt_version: PROMPT_VERSION.to_owned(),
            input_hash,
            usage,
        };
        fs::write(
            self.report_file(date_key)?,
            serde_json::to_vec_pretty(&report)?,
        )?;
        let _ = self.write_html(&report);
        Ok(report)
    }

    /// Whether an automatic run should (re)generate this day. Manual reports are
    /// never overwritten; an automatic report is refreshed when substantial
    /// activity happened after it was generated.
    pub fn needs_auto_report(&self, date_key: &str, not_before: i64) -> bool {
        let Ok(stats) = self.store.day_stats(date_key, self.options()) else {
            return false;
        };
        if stats.total_active_ms < 30 * 60_000 {
            return false;
        }
        match self.get_report(date_key) {
            None => true,
            Some(report) if report.source != "auto" => false,
            Some(report) => {
                report.generated_at < not_before
                    || stats
                        .last_active
                        .is_some_and(|last| last - report.generated_at > 30 * 60_000)
            }
        }
    }

    pub fn test_connection(&self) -> Result<(String, String)> {
        let ai = self.settings.get().ai;
        let output = llm::chat(
            &ai.text_endpoint(),
            json!([{"role":"user", "content":"请只回复两个字：正常"}]),
            ChatOptions {
                max_tokens: 16,
                timeout_ms: 30_000,
                ..Default::default()
            },
        )?;
        let mut model = output.model;
        if ai.screenshot_analysis {
            let vision = crate::intervals::test_vision(&ai)?;
            model = format!("{model}；视觉 {vision}");
        }
        Ok((output.content, model))
    }
}

fn parse_advice(content: &str) -> Option<Advice> {
    let value = llm::extract_json_object(content)?;
    let advice: Advice = serde_json::from_value(value).ok()?;
    (!advice.suggestions.is_empty()).then_some(advice)
}

/// Local time of day "HH:MM" on `date_key`, as epoch milliseconds.
pub fn scheduled_ts(date_key: &str, time: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(date_key, "%Y-%m-%d").ok()?;
    let time = chrono::NaiveTime::parse_from_str(time.trim(), "%H:%M").ok()?;
    use chrono::TimeZone;
    chrono::Local
        .from_local_datetime(&date.and_time(time))
        .earliest()
        .map(|date| date.timestamp_millis())
}

pub fn date_of(ts: i64) -> Option<NaiveDate> {
    local_datetime(ts).map(|date| date.date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{llm::mock, store::Event};
    use chrono::{Local, TimeZone};

    fn noon() -> (String, i64) {
        let date = Local::now().date_naive() - ChronoDuration::days(1);
        let ts = Local
            .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp_millis();
        (date.format("%Y-%m-%d").to_string(), ts)
    }

    fn seed(store: &DayStore, noon: i64) {
        for index in 0..16 {
            let code = index % 2 == 0;
            store
                .append(Event::app_focus(
                    noon + index * 3 * 60_000,
                    if code { "VS Code" } else { "PowerShell" },
                    if code { "code" } else { "powershell" },
                    if code {
                        "secret@example.com - git commit"
                    } else {
                        "git status"
                    },
                    1,
                    1_000,
                ))
                .unwrap();
        }
        store
            .append(Event::at(
                noon + 60_000,
                "site_visit",
                json!({"site":"private.example.com", "app":"Chrome", "name":"chrome"}),
            ))
            .unwrap();
        store
            .append(Event::at(noon + 48 * 60_000, "tick", json!({"i":1000})))
            .unwrap();
    }

    fn advice_json() -> String {
        json!({
            "headline": "把 git 操作留在一个窗口里",
            "diagnosis": [
                {"finding": "编辑器和终端来回切换，根因是 git 操作分散", "evidence": ["E1", "E99"]},
                {"finding": "这条没有证据", "evidence": []}
            ],
            "suggestions": [
                {
                    "title": "用 lazygit 集中处理提交",
                    "type": "工具",
                    "evidence": ["E1"],
                    "why": "每天切换 999 次",
                    "actions": ["安装 lazygit", "在终端里固定一个标签页"],
                    "when": "明早开始写代码前",
                    "expected": "减少一半切换",
                    "effort": "低",
                    "confidence": "中",
                    "tools": ["development-loop", "made-up-tool"]
                },
                {"title": "没有证据的建议", "evidence": ["E77"], "actions": ["x"]}
            ],
            "experiment": {"hypothesis": "集中 git 操作能减少切换", "action": "只在 lazygit 里提交", "metric": "switches_per_hour", "target": 5},
            "followup": ""
        })
        .to_string()
    }

    #[test]
    fn advice_end_to_end_is_grounded_redacted_and_persisted() {
        let (url, rx) = mock::serve(vec![mock::completion("mock-model", &advice_json())]);
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(DayStore::new(temp.path().join("data")).unwrap());
        let settings = Arc::new(Settings::load(temp.path().join("user")));
        settings
            .patch(&json!({"ai": {"baseUrl": url, "apiKey":"mock-key", "model":"mock-model", "webSearch":false}}))
            .unwrap();
        let (date_key, noon) = noon();
        seed(&store, noon);
        let analyzer = Analyzer::new(settings, store, temp.path().join("reports")).unwrap();
        let report = analyzer.generate_report(&date_key).unwrap();
        let request = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert!(request.contains("Bearer mock-key"));
        assert!(request.contains("response_format"));
        assert!(!request.contains("secret@example.com"));
        assert!(!request.contains("private.example.com"));
        assert!(request.contains("development-loop"));

        let advice = report.advice.as_ref().unwrap();
        assert_eq!(advice.diagnosis.len(), 1);
        assert_eq!(advice.diagnosis[0].evidence, vec!["E1"]);
        assert_eq!(advice.suggestions.len(), 1);
        assert_eq!(advice.suggestions[0].tools, vec!["development-loop"]);
        assert_eq!(advice.suggestions[0].unverified_numbers, vec!["999"]);
        let experiment = advice.experiment.as_ref().unwrap();
        assert!(experiment.baseline.is_some());
        assert_eq!(experiment.date_key, date_key);
        assert!(report.markdown.contains("## 建议"));
        assert!(report.markdown.contains("lazygit"));
        assert!(report.markdown.contains("## 明日实验"));
        assert_eq!(report.prompt_version, PROMPT_VERSION);
        assert_eq!(report.usage.prompt_tokens, 100);
        assert!(!report.input_hash.is_empty());
        assert!(analyzer.get_report(&date_key).is_some());
        let html = fs::read_to_string(temp.path().join("reports").join(format!("{date_key}.html"))).unwrap();
        assert!(html.contains("用 lazygit 集中处理提交"));
        assert!(html.contains("明日实验"));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn invalid_json_gets_one_repair_round() {
        let (url, rx) = mock::serve(vec![
            mock::completion("m", "这是一些散文，不是 JSON"),
            mock::completion("m", &advice_json()),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(DayStore::new(temp.path().join("data")).unwrap());
        let settings = Arc::new(Settings::load(temp.path().join("user")));
        settings
            .patch(&json!({"ai": {"baseUrl": url, "model":"m"}}))
            .unwrap();
        let (date_key, noon) = noon();
        seed(&store, noon);
        let analyzer = Analyzer::new(settings, store, temp.path().join("reports")).unwrap();
        let report = analyzer.generate_report(&date_key).unwrap();
        let _ = rx.recv().unwrap();
        let repair = rx.recv().unwrap();
        assert!(repair.contains("这是一些散文"));
        assert_eq!(report.usage.prompt_tokens, 200);
    }

    #[test]
    fn previous_experiment_is_scored_automatically() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(DayStore::new(temp.path().join("data")).unwrap());
        let settings = Arc::new(Settings::load(temp.path().join("user")));
        let (date_key, noon) = noon();
        seed(&store, noon);
        let analyzer = Analyzer::new(settings, store.clone(), temp.path().join("reports")).unwrap();
        let previous = (NaiveDate::parse_from_str(&date_key, "%Y-%m-%d").unwrap()
            - ChronoDuration::days(1))
        .format("%Y-%m-%d")
        .to_string();
        let report = Report {
            date_key: previous.clone(),
            markdown: String::new(),
            model: "m".to_owned(),
            generated_at: 0,
            active_minutes: 0,
            switch_count: 0,
            app_count: 0,
            recommendation_count: 0,
            recommendations: vec![],
            resource_search: SearchSummary::default(),
            advice: Some(Advice {
                experiment: Some(Experiment {
                    action: "关掉通知".to_owned(),
                    metric: "deep_work_min".to_owned(),
                    target: 20.0,
                    baseline: Some(5.0),
                    date_key: previous.clone(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            evidence: vec![],
            metrics: None,
            review: None,
            source: "manual".to_owned(),
            prompt_version: String::new(),
            input_hash: String::new(),
            usage: Usage::default(),
        };
        fs::write(
            analyzer.report_file(&previous).unwrap(),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
        let context = analyzer.build_context(&date_key).unwrap();
        let review = context.review.unwrap();
        assert_eq!(review.achieved, Some(true), "{review:?}");
        assert!(context.user_prompt.contains("上次实验"));
        assert!(context.user_prompt.contains("已达成"));
        assert!(
            context
                .evidence
                .iter()
                .any(|item| item.kind == "experiment")
        );
    }

    #[test]
    fn auto_report_never_overwrites_manual_and_refreshes_stale_auto() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(DayStore::new(temp.path().join("data")).unwrap());
        let settings = Arc::new(Settings::load(temp.path().join("user")));
        let (date_key, noon) = noon();
        seed(&store, noon);
        let analyzer = Analyzer::new(settings, store, temp.path().join("reports")).unwrap();
        assert!(analyzer.needs_auto_report(&date_key, 0));
        let mut report: Report = serde_json::from_value(json!({
            "dateKey": date_key, "markdown": "", "model": "m", "generatedAt": noon,
            "activeMinutes": 1, "switchCount": 0, "appCount": 0, "source": "manual"
        }))
        .unwrap();
        fs::write(
            analyzer.report_file(&date_key).unwrap(),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
        assert!(!analyzer.needs_auto_report(&date_key, i64::MAX));
        report.source = "auto".to_owned();
        fs::write(
            analyzer.report_file(&date_key).unwrap(),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
        // Generated at noon, activity continued until 12:48.
        assert!(analyzer.needs_auto_report(&date_key, 0));
        report.generated_at = noon + 3_600_000;
        fs::write(
            analyzer.report_file(&date_key).unwrap(),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
        assert!(!analyzer.needs_auto_report(&date_key, 0));
    }
}
