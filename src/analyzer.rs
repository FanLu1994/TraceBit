//! The advisor: learned work profile + local evidence + AI reasoning -> tool upgrades.
//!
//! Pipeline: persistent work profile -> day stats (+7 days of history) ->
//! deterministic metrics and numbered evidence (`insights`) -> screenshot
//! interval summaries -> rule-based tool candidates (including open-source
//! alternatives) -> one JSON-mode model call -> validation against the evidence
//! and the "no behavior change" guard -> locally rendered Markdown. Each report
//! also records metrics worth watching next time; the next report compares them
//! on its own, and tools the person marked as adopted are re-measured.

use crate::{
    insights::{
        self, DayMetrics, Evidence, History, METRICS, build_evidence, day_metrics, known_numbers,
        metric_def,
    },
    intervals::read_summaries,
    llm::{self, ChatOptions, Usage},
    profile::{Decision, DecisionKind, Profile, Snapshot as ProfileSnapshot},
    recommendations::{Recommendation, SearchSummary, build_recommendations},
    redact::redact_with_limit,
    settings::Settings,
    store::{DayStats, DayStore, DeriveOptions, hhmm, local_datetime, now_ms, validate_date_key},
    web_search::enrich_recommendations,
};
use anyhow::{Result, bail};
use chrono::{Duration as ChronoDuration, NaiveDate};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

pub use crate::redact::redact_text;

pub const PROMPT_VERSION: &str = "advice-v3";
const HISTORY_DAYS: i64 = 7;
/// How many tool opportunities a report keeps.
const MAX_OPPORTUNITIES: usize = 4;
const UPGRADE_TYPES: [&str; 5] = ["工具", "替代", "自动化", "配置", "集成"];
const LEVELS: [&str; 3] = ["低", "中", "高"];

/// A statement about how this person works, taken from the evidence.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Observation {
    #[serde(alias = "finding")]
    pub observation: String,
    pub evidence: Vec<String>,
    pub confidence: String,
}

/// A concrete tool or configuration change that makes an existing action cheaper.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Upgrade {
    pub title: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub evidence: Vec<String>,
    /// The existing habit or action this attaches to.
    pub habit: String,
    /// What that action costs today, according to the evidence.
    pub problem: String,
    pub actions: Vec<String>,
    pub payoff: String,
    pub risk: String,
    /// The tool this one replaces, if any.
    pub alternative: String,
    pub effort: String,
    pub confidence: String,
    pub tools: Vec<String>,
    /// Figures the model stated that do not appear in the evidence.
    pub unverified_numbers: Vec<String>,
}

/// A metric worth watching next time. Purely observational: no action needed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Watch {
    pub metric: String,
    pub reason: String,
    pub baseline: Option<f64>,
}

/// Before/after for a tool the person marked as adopted.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct AdoptionReview {
    pub title: String,
    pub resource: String,
    pub date_key: String,
    pub before: String,
    pub after: Option<String>,
}

impl AdoptionReview {
    pub fn text(&self) -> String {
        match &self.after {
            Some(after) => format!(
                "你在 {} 记录了「{}」。当时：{} 现在：{}",
                self.date_key, self.resource, self.before, after
            ),
            None => format!(
                "你在 {} 记录了「{}」。当时：{} 今天这类来回已经不再是候选，说明它要么已经解决，要么当天没有发生。",
                self.date_key, self.resource, self.before
            ),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Advice {
    /// One sentence describing how this person works, not a request.
    pub headline: String,
    #[serde(alias = "diagnosis")]
    pub reading: Vec<Observation>,
    #[serde(alias = "suggestions")]
    pub upgrades: Vec<Upgrade>,
    pub watch: Vec<Watch>,
    pub followup: String,
    pub unverified_numbers: Vec<String>,
    /// Upgrades dropped by the local "no behavior pressure" guard.
    pub filtered: Vec<String>,
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
    /// The accumulated picture of how this person works.
    #[serde(default)]
    pub profile: Option<ProfileSnapshot>,
    /// Before/after for tools the person marked as adopted.
    #[serde(default)]
    pub adoptions: Vec<AdoptionReview>,
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
    pub profile: ProfileSnapshot,
    pub adoptions: Vec<AdoptionReview>,
    pub system_prompt: String,
    pub user_prompt: String,
}

pub const SYSTEM_PROMPT: &str = r#"你是一位工具升级顾问。你会拿到某人真实工作方式的长期画像、今天的电脑使用证据（由本地软件计算并脱敏）、截图时段摘要、可用工具候选，以及他之前对工具的决定。

你的任务不是纠正他的习惯，而是让他已经在做的事更省力。

思考方法：
1. 先读懂这个人怎么工作：他用什么工具、在哪些工具之间来回、在什么时段产出、反复处理哪些项目。写进 reading，用陈述句描述事实，不评价。
2. 再针对具体动作找升级：这个动作今天花了他多少时间和步骤？有没有同类工具、插件、快捷键、命令、配置或自动化，能让同一个动作少几步、少几次跳转、少几次手工搬运？
3. 排序依据 = 每天发生的次数 × 每次浪费的步骤 × 上手成本。

硬性规则（违反即作废）：
- 不得要求他改变习惯。禁止出现这类建议：减少/戒掉/少用/限制使用、定时集中回复、关闭通知、早睡、多休息、保持专注、自律、番茄钟、时间管理。不要评价他的作息、注意力或意志力。
- 每条 upgrade 必须落到可安装、可配置或可执行的东西上：具体工具、插件、快捷键、命令、模板、脚本或设置项。只讲道理而不给出具体东西的，不要写。
- habit 必须写清楚它接上他现在哪个动作（例如“你每天已经在终端里跑 git 命令”），不要假设他会形成新流程。
- 优先开源、可本地运行、无账号锁定的选择；能替代他现在用的付费或受限工具时，在 alternative 写明替代对象。
- 必须引用证据编号（E 开头是今天，P 开头是长期画像，S 开头是截图摘要）。不得编造证据里没有的数字、应用或时间；payoff 里出现的数字必须来自证据。
- tools 只能填工具候选的编号；候选里没有合适的就填空数组，不要自己写工具名或链接。
- 已采纳的工具不要当作新发现重复推荐；已标记不需要的工具不要再次推荐。
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
  "headline": "一句话描述他实际怎么工作（陈述句，不是要求），不超过 40 字",
  "reading": [{{"observation": "关于他工作方式的观察", "evidence": ["P1", "E2"], "confidence": "高|中|低"}}],
  "upgrades": [{{
    "title": "具体工具或做法，动词开头，不超过 20 字",
    "type": "工具|替代|自动化|配置|集成",
    "evidence": ["E1"],
    "habit": "它接上你现在做的哪件事",
    "problem": "这件事现在的时间和步骤成本（引用证据里的数字）",
    "actions": ["第一步具体动作", "第二步"],
    "payoff": "省下的时间或步骤，及依据",
    "risk": "风险与注意",
    "alternative": "它替代的现有工具名；没有就空字符串",
    "effort": "低|中|高",
    "confidence": "高|中|低",
    "tools": ["工具候选编号"]
  }}],
  "watch": [{{"metric": "指标键名", "reason": "为什么值得下次观察"}}],
  "followup": "对上次采纳情况的说明；没有则空字符串"
}}
reading 2–4 条；upgrades 2–4 条，按优先级排序；actions 1–4 步；watch 1–2 条，metric 只能取以下键之一：{keys}"#
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

#[allow(clippy::too_many_arguments)]
fn build_user_prompt(
    date_key: &str,
    metrics: &DayMetrics,
    evidence: &[Evidence],
    summaries: &[Evidence],
    profile: &[Evidence],
    recommendations: &[Recommendation],
    decisions: &str,
    adoptions: &[AdoptionReview],
) -> String {
    let mut lines = vec![format!(
        "日期：{date_key}（{}）。以下内容由本地软件记录、计算并脱敏，聊天窗口只保留应用名。",
        week_label(date_key)
    )];
    if !profile.is_empty() {
        lines.push(String::new());
        lines.push("【长期工作画像】（P 开头，本机累计记录）".to_owned());
        for item in profile {
            lines.push(format!("{}：{}", item.id, item.text));
        }
    }
    lines.push(String::new());
    lines.push("【今天的证据】（E 开头）".to_owned());
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
    if !decisions.trim().is_empty() {
        lines.push(String::new());
        lines.push("【他之前对工具的决定】".to_owned());
        lines.push(decisions.trim().to_owned());
    }
    if !adoptions.is_empty() {
        lines.push(String::new());
        lines.push("【采纳回看】".to_owned());
        for item in adoptions {
            lines.push(format!("- {}", item.text()));
        }
    }
    lines.push(String::new());
    lines.push("【工具候选】（本地规则按今天的真实动作匹配，只能按编号引用）".to_owned());
    if recommendations.is_empty() {
        lines.push("无。".to_owned());
    }
    for item in recommendations {
        let mut line = format!(
            "{}：{}。触发依据：{} 可选工具：{}。",
            item.id,
            item.title,
            item.evidence,
            item.resources
                .iter()
                .map(|resource| format!("{}（{}）", resource.name, resource.kind))
                .collect::<Vec<_>>()
                .join("、")
        );
        if !item.replaces.is_empty() {
            line.push_str(&format!(" 可替代：{}。", item.replaces.join("、")));
        }
        lines.push(line);
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

/// Wording that tells the person to change behavior instead of changing tools.
/// Upgrades matching it are dropped and reported, so the promise is enforced
/// locally rather than only in the prompt.
static BEHAVIOR_PRESSURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"戒掉|戒除|减少(?:使用|打开|查看|刷|玩)|少用|少看|不要(?:再)?(?:看|刷|玩)|定时(?:集中)?(?:回复|查看|处理)|关闭(?:消息)?通知|免打扰|早睡|多休息|番茄钟|自律|时间管理|提高专注|管理注意力",
    )
    .unwrap()
});

fn pushes_behavior(text: &str) -> bool {
    BEHAVIOR_PRESSURE.is_match(text)
}

/// Drops everything that is not backed by the evidence and normalizes enums.
pub fn validate_advice(
    mut advice: Advice,
    evidence_ids: &BTreeSet<String>,
    tool_ids: &BTreeSet<String>,
    known: &BTreeSet<String>,
    metrics: &DayMetrics,
) -> Result<Advice> {
    let keep_ids = |ids: &mut Vec<String>| {
        let mut seen = BTreeSet::new();
        ids.retain(|id| evidence_ids.contains(id.trim()) && seen.insert(id.trim().to_owned()));
        for id in ids.iter_mut() {
            *id = id.trim().to_owned();
        }
    };
    let mut flagged = Vec::new();
    advice.reading.retain_mut(|item| {
        keep_ids(&mut item.evidence);
        flagged.extend(unverified(&item.observation, known));
        item.observation = item.observation.trim().to_owned();
        item.confidence = normalize_choice(&item.confidence, &LEVELS, "中");
        !item.evidence.is_empty() && !item.observation.is_empty()
    });
    advice.reading.truncate(4);

    let mut filtered = Vec::new();
    advice.upgrades.retain_mut(|upgrade| {
        keep_ids(&mut upgrade.evidence);
        upgrade.tools.retain(|tool| tool_ids.contains(tool.trim()));
        upgrade.title = upgrade.title.trim().to_owned();
        upgrade.kind = normalize_choice(&upgrade.kind, &UPGRADE_TYPES, "工具");
        upgrade.effort = normalize_choice(&upgrade.effort, &LEVELS, "中");
        upgrade.confidence = normalize_choice(&upgrade.confidence, &LEVELS, "中");
        upgrade.actions = upgrade
            .actions
            .iter()
            .map(|action| action.trim().to_owned())
            .filter(|action| !action.is_empty())
            .take(4)
            .collect();
        // Titles and imperative steps are where a behavior demand would show
        // up; problem/payoff legitimately describe the current cost.
        if pushes_behavior(&format!("{} {}", upgrade.title, upgrade.actions.join(" "))) {
            filtered.push(upgrade.title.clone());
            return false;
        }
        upgrade.unverified_numbers =
            unverified(&format!("{} {}", upgrade.problem, upgrade.payoff), known);
        !upgrade.evidence.is_empty() && !upgrade.title.is_empty() && !upgrade.actions.is_empty()
    });
    advice.upgrades.truncate(4);
    if advice.upgrades.is_empty() {
        if !filtered.is_empty() {
            bail!(
                "AI 只给出了要求改变习惯的建议（已过滤 {} 条），请重试或换用更强的模型",
                filtered.len()
            );
        }
        bail!("AI 输出中没有引用有效证据的工具升级，请重试或换用更强的模型");
    }
    advice.filtered = filtered;

    advice.watch.retain_mut(|watch| {
        let key = watch.metric.trim().to_owned();
        let Some(baseline) = metrics.get(&key) else {
            return false;
        };
        watch.metric = key;
        watch.reason = watch.reason.trim().to_owned();
        watch.baseline = Some(baseline);
        !watch.reason.is_empty()
    });
    advice.watch.truncate(2);

    flagged.sort();
    flagged.dedup();
    advice.unverified_numbers = flagged;
    Ok(advice)
}

pub fn render_markdown(
    date_key: &str,
    advice: &Advice,
    metrics: &DayMetrics,
    evidence: &[Evidence],
    recommendations: &[Recommendation],
    search: &SearchSummary,
    adoptions: &[AdoptionReview],
) -> String {
    let mut lines = vec![
        format!("# {date_key} 工具升级建议"),
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

    if !adoptions.is_empty() {
        lines.extend([String::new(), "## 采纳回看".to_owned()]);
        for item in adoptions {
            lines.push(format!("- {}", item.text()));
        }
        if !advice.followup.trim().is_empty() {
            lines.push(format!("- 说明：{}", advice.followup.trim()));
        }
    }

    if !advice.reading.is_empty() {
        lines.extend([String::new(), "## 你的工作方式".to_owned()]);
        for item in &advice.reading {
            lines.push(format!(
                "- {}（依据 {}）",
                item.observation.trim(),
                item.evidence.join("、")
            ));
        }
        if !advice.unverified_numbers.is_empty() {
            lines.push(format!(
                "> 注意：以上描述中的数字 {} 无法在记录中核实，请谨慎参考。",
                advice.unverified_numbers.join("、")
            ));
        }
    }

    lines.extend([String::new(), "## 工具升级机会".to_owned()]);
    for (index, upgrade) in advice.upgrades.iter().enumerate() {
        lines.push(String::new());
        lines.push(format!("### {}. {}", index + 1, upgrade.title.trim()));
        lines.push(format!(
            "{} · 投入{} · 把握{} · 依据 {}",
            upgrade.kind,
            upgrade.effort,
            upgrade.confidence,
            upgrade.evidence.join("、")
        ));
        if !upgrade.habit.trim().is_empty() {
            lines.push(format!("- 接上你现在：{}", upgrade.habit.trim()));
        }
        if !upgrade.problem.trim().is_empty() {
            lines.push(format!("- 现状：{}", upgrade.problem.trim()));
        }
        if !upgrade.alternative.trim().is_empty() {
            lines.push(format!("- 替换：{}", upgrade.alternative.trim()));
        }
        lines.push("- 怎么开始：".to_owned());
        for (step, action) in upgrade.actions.iter().enumerate() {
            lines.push(format!("    {}. {action}", step + 1));
        }
        if !upgrade.payoff.trim().is_empty() {
            lines.push(format!("- 预期：{}", upgrade.payoff.trim()));
        }
        if !upgrade.risk.trim().is_empty() {
            lines.push(format!("- 风险：{}", upgrade.risk.trim()));
        }
        for tool in &upgrade.tools {
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
        if !upgrade.unverified_numbers.is_empty() {
            lines.push(format!(
                "> 注意：数字 {} 无法在记录中核实。",
                upgrade.unverified_numbers.join("、")
            ));
        }
    }

    if !advice.watch.is_empty() {
        lines.extend([String::new(), "## 下次会自动核对".to_owned()]);
        lines.push("这些指标不需要你做任何事，下次生成时会自动对照。".to_owned());
        for watch in &advice.watch {
            let def = metric_def(&watch.metric);
            let (label, unit) = def.map(|def| (def.label, def.unit)).unwrap_or(("指标", ""));
            lines.push(format!(
                "- {label}：今天 {}{unit}。{}",
                fmt_number(watch.baseline.unwrap_or_default()),
                watch.reason.trim()
            ));
        }
    }

    if !advice.filtered.is_empty() {
        lines.extend([
            String::new(),
            format!(
                "> 已过滤 {} 条要求改变习惯的内容；这份报告只保留能让现有动作更省力的工具建议。",
                advice.filtered.len()
            ),
        ]);
    }

    let used = advice
        .upgrades
        .iter()
        .flat_map(|upgrade| upgrade.tools.iter())
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
            if !item.replaces.is_empty() {
                lines.push(format!("- 可替代：{}", item.replaces.join("、")));
            }
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

    /// Score-free picture of how this person works, stored next to the reports.
    fn profile_path(&self) -> PathBuf {
        self.reports_dir
            .parent()
            .map(|parent| parent.join("profile.json"))
            .unwrap_or_else(|| self.reports_dir.join("profile.json"))
    }

    pub fn profile(&self) -> Profile {
        Profile::load(&self.profile_path())
    }

    fn digest_path(&self) -> PathBuf {
        self.reports_dir
            .parent()
            .map(|parent| parent.join("digest.json"))
            .unwrap_or_else(|| self.reports_dir.join("digest.json"))
    }

    /// Local date whose digest was already delivered.
    pub fn last_digest(&self) -> Option<String> {
        crate::digest::DigestState::load(&self.digest_path()).last_sent
    }

    pub fn mark_digest_sent(&self, date_key: &str) -> Result<()> {
        validate_date_key(date_key)?;
        let path = self.digest_path();
        let mut state = crate::digest::DigestState::load(&path);
        state.last_sent = Some(date_key.to_owned());
        state.save(&path)
    }

    /// Whether a day has enough recorded activity to be worth a notification.
    pub fn has_activity(&self, date_key: &str) -> bool {
        self.store
            .day_stats(date_key, self.options())
            .is_ok_and(|stats| stats.total_active_ms >= 30 * 60_000)
    }

    /// Records this day in the profile and persists it.
    pub fn observe_profile(&self, date_key: &str) -> Result<Profile> {
        validate_date_key(date_key)?;
        let stats = self.store.day_stats(date_key, self.options())?;
        let mut profile = self.profile();
        profile.observe(date_key, &stats);
        profile.save(&self.profile_path())?;
        Ok(profile)
    }

    /// Learns every recorded day in the recent past. Runs once when the app
    /// starts, so the profile is useful immediately instead of after N reports.
    /// Days already in the profile are not recomputed.
    pub fn backfill_profile(&self, days: i64) -> usize {
        let mut profile = self.profile();
        let today = chrono::Local::now().date_naive();
        let mut learned = 0;
        for offset in 0..days.max(0) {
            let key = (today - ChronoDuration::days(offset))
                .format("%Y-%m-%d")
                .to_string();
            if profile.days.contains_key(&key) {
                continue;
            }
            let Ok(stats) = self.store.day_stats(&key, self.options()) else {
                continue;
            };
            if stats.total_active_ms < 5 * 60_000 {
                continue;
            }
            if profile.observe(&key, &stats) {
                learned += 1;
            }
        }
        if learned > 0 {
            let _ = profile.save(&self.profile_path());
        }
        learned
    }

    /// Marks a tool candidate as adopted or not needed. The next report compares
    /// the same workflow automatically; nothing else is required from the user.
    pub fn decide(
        &self,
        date_key: &str,
        opportunity: &str,
        resource: &str,
        url: &str,
        adopted: bool,
    ) -> Result<Profile> {
        validate_date_key(date_key)?;
        let stats = self.store.day_stats(date_key, self.options())?;
        let recommendations = build_recommendations(&stats);
        let Some(item) = recommendations.iter().find(|item| item.id == opportunity) else {
            bail!("找不到这个工具候选");
        };
        let mut profile = self.profile();
        profile.record(Decision {
            opportunity: item.id.clone(),
            title: item.title.clone(),
            resource: resource.to_owned(),
            url: url.to_owned(),
            date_key: date_key.to_owned(),
            kind: Some(if adopted {
                DecisionKind::Adopted
            } else {
                DecisionKind::Dismissed
            }),
            evidence: item.evidence.clone(),
        });
        profile.save(&self.profile_path())?;
        Ok(profile)
    }

    /// Before / after for the most recent adopted tools.
    fn review_adoptions(
        &self,
        profile: &Profile,
        recommendations: &[Recommendation],
    ) -> Vec<AdoptionReview> {
        let mut reviews = profile
            .adopted()
            .into_iter()
            .rev()
            .take(2)
            .map(|decision| AdoptionReview {
                title: decision.title.clone(),
                resource: if decision.resource.trim().is_empty() {
                    decision.title.clone()
                } else {
                    decision.resource.clone()
                },
                date_key: decision.date_key.clone(),
                before: decision.evidence.clone(),
                after: recommendations
                    .iter()
                    .find(|item| item.id == decision.opportunity)
                    .map(|item| item.evidence.clone()),
            })
            .collect::<Vec<_>>();
        reviews.reverse();
        reviews
    }

    fn profile_evidence(snapshot: &ProfileSnapshot) -> Vec<Evidence> {
        snapshot
            .lines()
            .into_iter()
            .enumerate()
            .map(|(index, (kind, text))| Evidence {
                id: format!("P{}", index + 1),
                kind: kind.to_owned(),
                text,
            })
            .collect()
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

        // Today is folded in memory so a preview shows exactly what generation
        // would send; generation stores the same state right after.
        let mut profile = self.profile();
        profile.observe(date_key, &stats);
        let snapshot = profile.snapshot();
        let profile_evidence = Self::profile_evidence(&snapshot);

        let summaries = self.summaries(date_key);
        let all_recommendations = build_recommendations(&stats);
        let adoptions = self.review_adoptions(&profile, &all_recommendations);
        let dismissed = profile.dismissed();
        let mut recommendations = all_recommendations
            .into_iter()
            .filter(|item| !dismissed.contains(&item.id))
            .collect::<Vec<_>>();
        recommendations.truncate(MAX_OPPORTUNITIES);
        let user_prompt = build_user_prompt(
            date_key,
            &metrics,
            &evidence,
            &summaries,
            &profile_evidence,
            &recommendations,
            &profile.decisions_for_prompt(),
            &adoptions,
        );
        evidence.extend(summaries);
        evidence.extend(profile_evidence);
        Ok(AdviceContext {
            stats,
            metrics,
            evidence,
            recommendations,
            profile: snapshot,
            adoptions,
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
        // Learn this day even if the model call fails: the profile is just an
        // observation, not a generated artifact.
        self.observe_profile(date_key)?;
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
        let advice = validate_advice(advice, &evidence_ids, &tool_ids, &known, &context.metrics)?;
        let enriched = enrich_recommendations(&context.recommendations, &ai);
        let markdown = render_markdown(
            date_key,
            &advice,
            &context.metrics,
            &context.evidence,
            &enriched.recommendations,
            &enriched.search,
            &context.adoptions,
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
            profile: Some(context.profile),
            adoptions: context.adoptions,
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
    (!advice.upgrades.is_empty()).then_some(advice)
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

    fn analyzer_fixture(
        temp: &tempfile::TempDir,
    ) -> (Arc<Settings>, Arc<DayStore>, Analyzer, String, i64) {
        let store = Arc::new(DayStore::new(temp.path().join("data")).unwrap());
        let settings = Arc::new(Settings::load(temp.path().join("user")));
        let (date_key, noon) = noon();
        seed(&store, noon);
        let analyzer =
            Analyzer::new(settings.clone(), store.clone(), temp.path().join("reports")).unwrap();
        (settings, store, analyzer, date_key, noon)
    }

    fn advice_json() -> String {
        json!({
            "headline": "你在编辑器和终端之间完成开发，来回主要来自 git 操作",
            "reading": [
                {"observation": "开发工作集中在 VS Code 和 PowerShell 之间", "evidence": ["E1", "E99"], "confidence": "高"},
                {"observation": "没有证据的观察", "evidence": [], "confidence": "低"}
            ],
            "upgrades": [
                {
                    "title": "用 lazygit 处理提交",
                    "type": "工具",
                    "evidence": ["E1"],
                    "habit": "你每天在终端里跑 git 命令",
                    "problem": "每天来回切换 999 次",
                    "actions": ["安装 lazygit", "在终端里固定一个标签页"],
                    "payoff": "同样的提交在原窗口完成",
                    "risk": "改写历史的操作仍需确认",
                    "alternative": "",
                    "effort": "低",
                    "confidence": "中",
                    "tools": ["development-loop", "made-up-tool"]
                },
                {
                    "title": "定时集中回复消息",
                    "type": "配置",
                    "evidence": ["E1"],
                    "habit": "你已经被打断很多次",
                    "problem": "打断太多",
                    "actions": ["关闭通知"],
                    "payoff": "",
                    "risk": "",
                    "alternative": "",
                    "effort": "低",
                    "confidence": "中",
                    "tools": []
                },
                {"title": "没有证据的建议", "evidence": ["E77"], "actions": ["x"]}
            ],
            "watch": [
                {"metric": "switches_per_hour", "reason": "观察工具是否让来回变少"},
                {"metric": "not_a_metric", "reason": "x"}
            ],
            "followup": ""
        })
        .to_string()
    }

    #[test]
    fn advice_end_to_end_is_grounded_redacted_and_persisted() {
        let (url, rx) = mock::serve(vec![mock::completion("mock-model", &advice_json())]);
        let temp = tempfile::tempdir().unwrap();
        let (settings, _store, analyzer, date_key, _) = analyzer_fixture(&temp);
        settings
            .patch(&json!({"ai": {"baseUrl": url, "apiKey":"mock-key", "model":"mock-model", "webSearch":false}}))
            .unwrap();
        let report = analyzer.generate_report(&date_key).unwrap();
        let request = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert!(request.contains("Bearer mock-key"));
        assert!(request.contains("response_format"));
        assert!(!request.contains("secret@example.com"));
        assert!(!request.contains("private.example.com"));
        assert!(request.contains("development-loop"));
        assert!(request.contains("长期工作画像"));

        let advice = report.advice.as_ref().unwrap();
        assert_eq!(advice.reading.len(), 1);
        assert_eq!(advice.reading[0].evidence, vec!["E1"]);
        assert_eq!(advice.upgrades.len(), 1);
        assert_eq!(advice.upgrades[0].tools, vec!["development-loop"]);
        assert_eq!(advice.upgrades[0].unverified_numbers, vec!["999"]);
        assert_eq!(advice.filtered, vec!["定时集中回复消息"]);
        assert_eq!(advice.watch.len(), 1);
        assert_eq!(advice.watch[0].metric, "switches_per_hour");
        assert!(advice.watch[0].baseline.is_some());
        assert!(report.markdown.contains("## 工具升级机会"));
        assert!(report.markdown.contains("## 你的工作方式"));
        assert!(report.markdown.contains("lazygit"));
        assert!(report.markdown.contains("已过滤"));
        assert_eq!(report.prompt_version, PROMPT_VERSION);
        assert_eq!(report.usage.prompt_tokens, 100);
        assert!(!report.input_hash.is_empty());
        let profile = report.profile.as_ref().unwrap();
        assert_eq!(profile.days, 1);
        assert!(profile.tools.iter().any(|tool| tool.name == "VS Code"));
        assert!(analyzer.get_report(&date_key).is_some());
        assert!(temp.path().join("profile.json").is_file());
        let html = fs::read_to_string(temp.path().join("reports").join(format!("{date_key}.html")))
            .unwrap();
        assert!(html.contains("用 lazygit 处理提交"));
        assert!(html.contains("工具升级机会"));
        assert!(html.contains("工作方式"));
        assert!(!html.contains("<script"));
    }

    #[test]
    fn guard_matches_behavior_requests_not_cost_descriptions() {
        assert!(pushes_behavior("定时集中回复消息"));
        assert!(pushes_behavior("安装插件 关闭通知"));
        assert!(!pushes_behavior("把常用命令写成任务入口"));
        assert!(!pushes_behavior("现状是每天来回切换 12 次"));
    }

    #[test]
    fn invalid_json_gets_one_repair_round() {
        let (url, rx) = mock::serve(vec![
            mock::completion("m", "这是一些散文，不是 JSON"),
            mock::completion("m", &advice_json()),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let (settings, _store, analyzer, date_key, _) = analyzer_fixture(&temp);
        settings
            .patch(&json!({"ai": {"baseUrl": url, "model":"m"}}))
            .unwrap();
        let report = analyzer.generate_report(&date_key).unwrap();
        let _ = rx.recv().unwrap();
        let repair = rx.recv().unwrap();
        assert!(repair.contains("这是一些散文"));
        assert_eq!(report.usage.prompt_tokens, 200);
    }

    #[test]
    fn adopted_and_dismissed_tools_change_the_next_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let (_settings, _store, analyzer, date_key, _) = analyzer_fixture(&temp);
        let profile = analyzer
            .decide(
                &date_key,
                "development-loop",
                "lazygit",
                "https://github.com/jesseduffield/lazygit",
                true,
            )
            .unwrap();
        assert_eq!(profile.adopted().len(), 1);
        let context = analyzer.build_context(&date_key).unwrap();
        assert!(!context.adoptions.is_empty());
        assert!(context.user_prompt.contains("采纳回看"));
        assert!(context.user_prompt.contains("lazygit"));
        assert!(context.user_prompt.contains("已采纳"));

        analyzer
            .decide(&date_key, "development-loop", "lazygit", "", false)
            .unwrap();
        let context = analyzer.build_context(&date_key).unwrap();
        assert!(
            context
                .recommendations
                .iter()
                .all(|item| item.id != "development-loop")
        );
        assert!(context.user_prompt.contains("不要再次推荐"));
    }

    #[test]
    fn regeneration_does_not_duplicate_the_profile_day() {
        let temp = tempfile::tempdir().unwrap();
        let (_settings, _store, analyzer, date_key, _) = analyzer_fixture(&temp);
        analyzer.observe_profile(&date_key).unwrap();
        analyzer.observe_profile(&date_key).unwrap();
        let profile = analyzer.profile();
        assert_eq!(profile.days.len(), 1);
        let snapshot = profile.snapshot();
        assert_eq!(snapshot.days, 1);
        assert!(snapshot.rhythm.active_hours > 0);
    }

    #[test]
    fn auto_report_never_overwrites_manual_and_refreshes_stale_auto() {
        let temp = tempfile::tempdir().unwrap();
        let (_settings, _store, analyzer, date_key, noon) = analyzer_fixture(&temp);
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
