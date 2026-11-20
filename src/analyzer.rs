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
    i18n::{Lang, fill},
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
    tr,
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
const UPGRADE_TYPES: [&str; 5] = ["tool", "alternative", "automation", "config", "integration"];
const LEVELS: [&str; 3] = ["low", "medium", "high"];
/// Chinese enum values written by reports generated before the contract moved
/// to language-neutral keys. Accepted on read and mapped to the new keys.
const LEGACY_KINDS: [(&str, &str); 5] = [
    ("工具", "tool"),
    ("替代", "alternative"),
    ("自动化", "automation"),
    ("配置", "config"),
    ("集成", "integration"),
];
const LEGACY_LEVELS: [(&str, &str); 3] = [("低", "low"), ("中", "medium"), ("高", "high")];

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
    pub fn text(&self, lang: Lang) -> String {
        match &self.after {
            Some(after) => fill(
                tr!(
                    lang,
                    "你在 {} 记录了「{}」。当时：{} 现在：{}",
                    "On {} you recorded “{}”. Then: {} Now: {}",
                    "{} に「{}」を記録しました。当時：{} 現在：{}",
                    "{}에 “{}”을(를) 기록했습니다. 당시: {} 지금: {}"
                ),
                &[&self.date_key, &self.resource, &self.before, after],
            ),
            None => fill(
                tr!(
                    lang,
                    "你在 {} 记录了「{}」。当时：{} 今天这类来回已经不再是候选，说明它要么已经解决，要么当天没有发生。",
                    "On {} you recorded “{}”. Then: {} Today this kind of back-and-forth is no longer a candidate, so it was either resolved or did not happen.",
                    "{} に「{}」を記録しました。当時：{} 今日はこの種の往復が候補に上がっていません。解決済みか、その日は発生しなかったと考えられます。",
                    "{}에 “{}”을(를) 기록했습니다. 당시: {} 오늘은 이런 왕복이 더 이상 후보가 아니며, 이미 해결되었거나 그날 발생하지 않았다는 뜻입니다."
                ),
                &[&self.date_key, &self.resource, &self.before],
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

/// Prompt body, in Chinese. The one line that depends on the interface
/// language is appended by [`system_prompt`] so the JSON contract stays fixed.
const PROMPT_BODY: &str = r#"你是一位工具升级顾问。你会拿到某人真实工作方式的长期画像、今天的电脑使用证据（由本地软件计算并脱敏）、截图时段摘要、可用工具候选，以及他之前对工具的决定。

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
- 已采纳的工具不要当作新发现重复推荐；已标记不需要的工具不要再次推荐。"#;

fn system_prompt(lang: Lang) -> String {
    let directive = fill(
        tr!(
            lang,
            "所有面向用户的文字都用目标语言（{}）撰写。只输出一个 JSON 对象，不要 Markdown，不要代码块。",
            "Write every user-facing string in the target language ({}). Only output one JSON object, with no Markdown and no code blocks.",
            "ユーザー向けの文字列はすべて対象言語（{}）で書いてください。JSON オブジェクトを 1 つだけ出力し、Markdown もコードブロックも使わないでください。",
            "사용자에게 보이는 문자열은 모두 대상 언어({})로 작성하세요. JSON 객체 하나만 출력하고 Markdown과 코드 블록은 사용하지 마세요."
        ),
        &[&lang.name()],
    );
    format!("{PROMPT_BODY}\n- {directive}")
}

fn output_contract(metrics: &DayMetrics, lang: Lang) -> String {
    let keys = METRICS
        .iter()
        .map(|metric| {
            format!(
                "{}（{}，今天 {} {}）",
                metric.key,
                metric.label(lang),
                metrics.get(metric.key).unwrap_or(0.0),
                metric.unit(lang)
            )
        })
        .collect::<Vec<_>>()
        .join("；");
    format!(
        r#"输出 JSON 结构（字段名必须一致）：
{{
  "headline": "一句话描述他实际怎么工作（陈述句，不是要求），不超过 40 字",
  "reading": [{{"observation": "关于他工作方式的观察", "evidence": ["P1", "E2"], "confidence": "low|medium|high"}}],
  "upgrades": [{{
    "title": "具体工具或做法，动词开头，不超过 20 字",
    "type": "tool|alternative|automation|config|integration",
    "evidence": ["E1"],
    "habit": "它接上你现在做的哪件事",
    "problem": "这件事现在的时间和步骤成本（引用证据里的数字）",
    "actions": ["第一步具体动作", "第二步"],
    "payoff": "省下的时间或步骤，及依据",
    "risk": "风险与注意",
    "alternative": "它替代的现有工具名；没有就空字符串",
    "effort": "low|medium|high",
    "confidence": "low|medium|high",
    "tools": ["工具候选编号"]
  }}],
  "watch": [{{"metric": "指标键名", "reason": "为什么值得下次观察"}}],
  "followup": "对上次采纳情况的说明；没有则空字符串"
}}
reading 2–4 条；upgrades 2–4 条，按优先级排序；actions 1–4 步；watch 1–2 条，metric 只能取以下键之一：{keys}
输出语言：{lang_name}；字段名与 type、effort、confidence 的取值保持上面的英文键名。"#,
        lang_name = lang.name()
    )
}

fn week_label(date_key: &str, lang: Lang) -> String {
    NaiveDate::parse_from_str(date_key, "%Y-%m-%d")
        .map(|date| {
            use chrono::Datelike;
            let names = [
                tr!(lang, "周日", "Sun", "日", "일"),
                tr!(lang, "周一", "Mon", "月", "월"),
                tr!(lang, "周二", "Tue", "火", "화"),
                tr!(lang, "周三", "Wed", "水", "수"),
                tr!(lang, "周四", "Thu", "木", "목"),
                tr!(lang, "周五", "Fri", "金", "금"),
                tr!(lang, "周六", "Sat", "土", "토"),
            ];
            names[date.weekday().num_days_from_sunday() as usize].to_owned()
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

/// A date-key error reaches the UI, so it is reported in the interface
/// language while the stored format check stays in `store`.
fn check_date(date_key: &str, lang: Lang) -> Result<()> {
    validate_date_key(date_key).map_err(|_| {
        anyhow::anyhow!(tr!(
            lang,
            "日期格式无效",
            "Invalid date format.",
            "日付の形式が正しくありません。",
            "날짜 형식이 올바르지 않습니다."
        ))
    })
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
    lang: Lang,
) -> String {
    let mut lines = vec![format!(
        "日期：{date_key}（{}）。以下内容由本地软件记录、计算并脱敏，聊天窗口只保留应用名。",
        week_label(date_key, lang)
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
            lines.push(format!("- {}", item.text(lang)));
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
    lines.push(output_contract(metrics, lang));
    lines.join("\n")
}

fn normalize_choice(
    value: &str,
    allowed: &[&str],
    legacy: &[(&str, &str)],
    fallback: &str,
) -> String {
    let value = value.trim();
    if let Some((_, key)) = legacy.iter().find(|(label, _)| value.contains(*label)) {
        return (*key).to_owned();
    }
    allowed
        .iter()
        .find(|choice| value.contains(**choice))
        .unwrap_or(&fallback)
        .to_string()
}

/// Display label for an upgrade kind. Accepts the language-neutral keys and the
/// Chinese values written by older reports.
pub fn kind_label(lang: Lang, kind: &str) -> &'static str {
    match kind.trim() {
        "alternative" | "替代" => tr!(lang, "替代", "Alternative", "代替", "대체"),
        "automation" | "自动化" => tr!(lang, "自动化", "Automation", "自動化", "자동화"),
        "config" | "配置" => tr!(lang, "配置", "Config", "設定", "설정"),
        "integration" | "集成" => tr!(lang, "集成", "Integration", "連携", "연동"),
        _ => tr!(lang, "工具", "Tool", "ツール", "도구"),
    }
}

/// Display label for an effort/confidence level. Accepts the language-neutral
/// keys and the Chinese values written by older reports.
pub fn level_label(lang: Lang, level: &str) -> &'static str {
    match level.trim() {
        "low" | "低" => tr!(lang, "低", "Low", "低", "낮음"),
        "high" | "高" => tr!(lang, "高", "High", "高", "높음"),
        _ => tr!(lang, "中", "Medium", "中", "보통"),
    }
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
    Regex::new(concat!(
        r"(?i)",
        r"戒掉|戒除|减少(?:使用|打开|查看|刷|玩)|少用|少看|不要(?:再)?(?:看|刷|玩)",
        r"|定时(?:集中)?(?:回复|查看|处理)|关闭(?:消息)?通知|免打扰|早睡|多休息",
        r"|番茄钟|自律|时间管理|提高专注|管理注意力",
        // English: only advice about the person's habits, never about steps.
        r"|\b(?:quit|stop)\s+(?:using|opening|checking|scrolling)\b",
        r"|\breduce\s+(?:your\s+)?(?:screen\s+time|usage|app\s+switching)\b",
        r"|\buse\s+less\b|\bavoid\s+using\b|\b(?:do\s+not|don't)\s+use\b",
        r"|\bturn\s+off\s+(?:the\s+)?notifications\b|\bdo\s+not\s+disturb\b",
        r"|\bsleep\s+earlier\b|\bgo\s+to\s+bed\s+earlier\b|\btake\s+(?:more\s+)?breaks\b",
        r"|\bpomodoro\b|\bself[- ]discipline\b|\btime\s+management\b",
        r"|\bmanage\s+(?:your\s+)?attention\b|\bimprove\s+(?:your\s+)?focus\b",
        // Japanese: limit to usage/time/notification objects so "手順を減らす" stays valid.
        r"|(?:使用|利用|画面|時間|確認|通知|アプリ)[^\n]{0,8}?(?:減ら|控え)",
        r"|やめましょう|やめてください|使わないで|見ないで|通知を(?:オフ|切)",
        r"|早寝|早く寝|休憩を(?:増や|と)|ポモドーロ|自己管理|時間管理|集中力を(?:高め|上げ)",
        // Korean: same rule; "단계를 줄이는" is a tool improvement, not pressure.
        r"|(?:사용|화면|시간|확인|알림|앱)[^\n]{0,8}?(?:줄이|줄여)",
        r"|그만\s*(?:사용|보|쓰)|사용하지\s*마|보지\s*마|알림을\s*(?:끄|꺼)",
        r"|일찍\s*자|휴식을\s*(?:늘|취)|뽀모도로|자기\s*관리|시간\s*관리|집중력을\s*(?:높|올)",
    ))
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
    lang: Lang,
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
        item.confidence = normalize_choice(&item.confidence, &LEVELS, &LEGACY_LEVELS, "medium");
        !item.evidence.is_empty() && !item.observation.is_empty()
    });
    advice.reading.truncate(4);

    let mut filtered = Vec::new();
    advice.upgrades.retain_mut(|upgrade| {
        keep_ids(&mut upgrade.evidence);
        upgrade.tools.retain(|tool| tool_ids.contains(tool.trim()));
        upgrade.title = upgrade.title.trim().to_owned();
        upgrade.kind = normalize_choice(&upgrade.kind, &UPGRADE_TYPES, &LEGACY_KINDS, "tool");
        upgrade.effort = normalize_choice(&upgrade.effort, &LEVELS, &LEGACY_LEVELS, "medium");
        upgrade.confidence =
            normalize_choice(&upgrade.confidence, &LEVELS, &LEGACY_LEVELS, "medium");
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
            bail!(fill(
                tr!(
                    lang,
                    "AI 只给出了要求改变习惯的建议（已过滤 {} 条），请重试或换用更强的模型",
                    "The AI only suggested behavior changes ({} filtered out). Try again or use a stronger model.",
                    "AI は習慣の変更を求める提案しか出しませんでした（{} 件を除外）。再試行するか、より強力なモデルを使ってください。",
                    "AI가 습관 변경을 요구하는 제안만 내놓았습니다({}개 제외됨). 다시 시도하거나 더 강력한 모델을 사용하세요."
                ),
                &[&filtered.len()]
            ));
        }
        bail!(tr!(
            lang,
            "AI 输出中没有引用有效证据的工具升级，请重试或换用更强的模型",
            "The AI output contained no tool upgrade backed by valid evidence. Try again or use a stronger model.",
            "AI の出力に、有効な根拠を引用したツール改善がありませんでした。再試行するか、より強力なモデルを使ってください。",
            "AI 출력에 유효한 근거를 인용한 도구 개선이 없습니다. 다시 시도하거나 더 강력한 모델을 사용하세요."
        ));
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

#[allow(clippy::too_many_arguments)]
pub fn render_markdown(
    date_key: &str,
    advice: &Advice,
    metrics: &DayMetrics,
    evidence: &[Evidence],
    recommendations: &[Recommendation],
    search: &SearchSummary,
    adoptions: &[AdoptionReview],
    lang: Lang,
) -> String {
    let list_sep = tr!(lang, "、", ", ", "、", ", ");
    let mut lines = vec![
        format!(
            "# {date_key} {}",
            tr!(
                lang,
                "工具升级建议",
                "Tool upgrade advice",
                "ツール改善の提案",
                "도구 개선 제안"
            )
        ),
        String::new(),
        format!("> {}", advice.headline.trim()),
        String::new(),
        fill(
            tr!(
                lang,
                "活跃 {} 分钟 · 深度工作 {} 分钟 · 每小时切换 {} 次 · 通讯 {} 次 · 会议 {} 分钟",
                "Active {} min · Deep work {} min · Switches {} per hour · Chat {} times · Meetings {} min",
                "アクティブ {} 分 · 集中作業 {} 分 · 1時間あたりの切り替え {} 回 · チャット {} 回 · 会議 {} 分",
                "활동 {}분 · 집중 작업 {}분 · 시간당 전환 {}회 · 채팅 {}회 · 회의 {}분"
            ),
            &[
                &fmt_number(metrics.active_min),
                &fmt_number(metrics.deep_work_min),
                &fmt_number(metrics.switches_per_hour),
                &fmt_number(metrics.comm_checks),
                &fmt_number(metrics.meeting_min),
            ],
        ),
    ];

    if !adoptions.is_empty() {
        lines.extend([
            String::new(),
            format!(
                "## {}",
                tr!(
                    lang,
                    "采纳回看",
                    "Adoption review",
                    "採用後の振り返り",
                    "채택 후 점검"
                )
            ),
        ]);
        for item in adoptions {
            lines.push(format!("- {}", item.text(lang)));
        }
        if !advice.followup.trim().is_empty() {
            lines.push(fill(
                tr!(lang, "- 说明：{}", "- Note: {}", "- メモ：{}", "- 설명: {}"),
                &[&advice.followup.trim()],
            ));
        }
    }

    if !advice.reading.is_empty() {
        lines.extend([
            String::new(),
            format!(
                "## {}",
                tr!(
                    lang,
                    "你的工作方式",
                    "How you work",
                    "あなたの働き方",
                    "업무 방식"
                )
            ),
        ]);
        for item in &advice.reading {
            lines.push(fill(
                tr!(
                    lang,
                    "- {}（依据 {}）",
                    "- {} (evidence {})",
                    "- {}（根拠 {}）",
                    "- {} (근거 {})"
                ),
                &[&item.observation.trim(), &item.evidence.join(list_sep)],
            ));
        }
        if !advice.unverified_numbers.is_empty() {
            lines.push(fill(
                tr!(
                    lang,
                    "> 注意：以上描述中的数字 {} 无法在记录中核实，请谨慎参考。",
                    "> Note: the numbers {} above cannot be verified in the records; use them with care.",
                    "> 注意：上記の記述にある数値 {} は記録から確認できません。参考程度にしてください。",
                    "> 참고: 위 설명의 숫자 {}는 기록에서 확인할 수 없으니 주의해서 참고하세요."
                ),
                &[&advice.unverified_numbers.join(list_sep)],
            ));
        }
    }

    lines.extend([
        String::new(),
        format!(
            "## {}",
            tr!(
                lang,
                "工具升级机会",
                "Tool opportunities",
                "ツール改善の機会",
                "도구 개선 기회"
            )
        ),
    ]);
    for (index, upgrade) in advice.upgrades.iter().enumerate() {
        lines.push(String::new());
        lines.push(format!("### {}. {}", index + 1, upgrade.title.trim()));
        lines.push(fill(
            tr!(
                lang,
                "{} · 投入{} · 把握{} · 依据 {}",
                "{} · Effort: {} · Confidence: {} · Evidence: {}",
                "{}・工数{}・確度{}・根拠 {}",
                "{} · 투입 {} · 확신 {} · 근거 {}"
            ),
            &[
                &kind_label(lang, &upgrade.kind),
                &level_label(lang, &upgrade.effort),
                &level_label(lang, &upgrade.confidence),
                &upgrade.evidence.join(list_sep),
            ],
        ));
        if !upgrade.habit.trim().is_empty() {
            lines.push(fill(
                tr!(
                    lang,
                    "- 接上你现在：{}",
                    "- Attaches to what you already do: {}",
                    "- 現在の動作につながる点：{}",
                    "- 지금 하는 일과 연결되는 지점: {}"
                ),
                &[&upgrade.habit.trim()],
            ));
        }
        if !upgrade.problem.trim().is_empty() {
            lines.push(fill(
                tr!(
                    lang,
                    "- 现状：{}",
                    "- Current cost: {}",
                    "- 現状：{}",
                    "- 현재: {}"
                ),
                &[&upgrade.problem.trim()],
            ));
        }
        if !upgrade.alternative.trim().is_empty() {
            lines.push(fill(
                tr!(
                    lang,
                    "- 替换：{}",
                    "- Replaces: {}",
                    "- 置き換え：{}",
                    "- 대체: {}"
                ),
                &[&upgrade.alternative.trim()],
            ));
        }
        lines.push(format!(
            "- {}",
            tr!(
                lang,
                "怎么开始：",
                "How to start:",
                "始め方：",
                "시작 방법:"
            )
        ));
        for (step, action) in upgrade.actions.iter().enumerate() {
            lines.push(format!("    {}. {action}", step + 1));
        }
        if !upgrade.payoff.trim().is_empty() {
            lines.push(fill(
                tr!(
                    lang,
                    "- 预期：{}",
                    "- Expected: {}",
                    "- 期待できる効果：{}",
                    "- 기대 효과: {}"
                ),
                &[&upgrade.payoff.trim()],
            ));
        }
        if !upgrade.risk.trim().is_empty() {
            lines.push(fill(
                tr!(
                    lang,
                    "- 风险：{}",
                    "- Risk: {}",
                    "- リスク：{}",
                    "- 위험: {}"
                ),
                &[&upgrade.risk.trim()],
            ));
        }
        for tool in &upgrade.tools {
            if let Some(item) = recommendations.iter().find(|item| &item.id == tool) {
                for resource in &item.resources {
                    lines.push(fill(
                        tr!(
                            lang,
                            "- 工具：[{}]({})（{}）{} 配置成本：{} 注意：{}",
                            "- Tool: [{}]({}) ({}) {} Setup cost: {} Notes: {}",
                            "- ツール：[{}]({})（{}）{} 設定コスト：{} 注意：{}",
                            "- 도구: [{}]({}) ({}) {} 설정 비용: {} 주의: {}"
                        ),
                        &[
                            &resource.name,
                            &resource.url,
                            &resource.kind,
                            &resource.description,
                            &resource.setup_cost,
                            &resource.permissions,
                        ],
                    ));
                }
            }
        }
        if !upgrade.unverified_numbers.is_empty() {
            lines.push(fill(
                tr!(
                    lang,
                    "> 注意：数字 {} 无法在记录中核实。",
                    "> Note: the numbers {} cannot be verified in the records.",
                    "> 注意：数値 {} は記録から確認できません。",
                    "> 참고: 숫자 {}는 기록에서 확인할 수 없습니다."
                ),
                &[&upgrade.unverified_numbers.join(list_sep)],
            ));
        }
    }

    if !advice.watch.is_empty() {
        lines.extend([
            String::new(),
            format!(
                "## {}",
                tr!(
                    lang,
                    "下次会自动核对",
                    "Checked automatically next time",
                    "次回に自動で照合",
                    "다음에 자동 확인"
                )
            ),
        ]);
        lines.push(tr!(
            lang,
            "这些指标不需要你做任何事，下次生成时会自动对照。",
            "Nothing to do here: these metrics are compared automatically when the next report is generated.",
            "ここで何かをする必要はありません。次回の生成時に自動で照合します。",
            "여기서 따로 할 일은 없습니다. 다음 보고서를 만들 때 자동으로 비교합니다."
        ).to_owned());
        for watch in &advice.watch {
            let def = metric_def(&watch.metric);
            let (label, unit) = def
                .map(|def| (def.label(lang), def.unit(lang)))
                .unwrap_or((tr!(lang, "指标", "Metric", "指標", "지표"), ""));
            lines.push(fill(
                tr!(
                    lang,
                    "- {0}：今天 {1}{2}。{3}",
                    "- {0}: today {1}{2}. {3}",
                    "- {0}：今日 {1}{2}。{3}",
                    "- {0}: 오늘 {1}{2}. {3}"
                ),
                &[
                    &label,
                    &fmt_number(watch.baseline.unwrap_or_default()),
                    &unit,
                    &watch.reason.trim(),
                ],
            ));
        }
    }

    if !advice.filtered.is_empty() {
        lines.extend([
            String::new(),
            fill(
                tr!(
                    lang,
                    "> 已过滤 {} 条要求改变习惯的内容；这份报告只保留能让现有动作更省力的工具建议。",
                    "> Filtered out {} behavior-change suggestions; this report keeps only tool advice that makes existing actions cheaper.",
                    "> 習慣の変更を求める内容を {} 件除外しました。このレポートには、今の動作を楽にするツールの提案だけを残しています。",
                    "> 습관 변경을 요구하는 내용 {}건을 제외했습니다. 이 보고서에는 지금 하는 일을 더 쉽게 만드는 도구 제안만 남겼습니다."
                ),
                &[&advice.filtered.len()],
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
        lines.extend([
            String::new(),
            format!(
                "## {}",
                tr!(
                    lang,
                    "其他可评估的工具",
                    "Other tools to consider",
                    "その他の検討候補",
                    "그 밖의 검토 도구"
                )
            ),
        ]);
        for item in unused {
            lines.push(format!("### {}", item.title));
            lines.push(fill(
                tr!(
                    lang,
                    "- 依据：{}",
                    "- Evidence: {}",
                    "- 根拠：{}",
                    "- 근거: {}"
                ),
                &[&item.evidence],
            ));
            if !item.replaces.is_empty() {
                lines.push(fill(
                    tr!(
                        lang,
                        "- 可替代：{}",
                        "- Can replace: {}",
                        "- 代替できるもの：{}",
                        "- 대체 가능: {}"
                    ),
                    &[&item.replaces.join(list_sep)],
                ));
            }
            for resource in &item.resources {
                lines.push(fill(
                    tr!(
                        lang,
                        "- [{}]({})（{}）{}",
                        "- [{}]({}) — {}: {}",
                        "- [{}]({})（{}）{}",
                        "- [{}]({}) — {}: {}"
                    ),
                    &[
                        &resource.name,
                        &resource.url,
                        &resource.kind,
                        &resource.description,
                    ],
                ));
            }
        }
    }

    if search.enabled {
        lines.push(fill(
            tr!(
                lang,
                "> 联网检索：{} 次抽象关键词查询，新增 {} 个候选，验证 {} 个链接；搜索服务不会收到行为数据。",
                "> Web search: {} abstract keyword queries, {} new candidates, {} links verified; the search service never receives behavior data.",
                "> Web 検索：抽象キーワードで {} 回検索し、候補を {} 件追加、{} 件のリンクを確認しました。検索サービスに行動データは送信されません。",
                "> 웹 검색: 추상 키워드 {}회 조회, 후보 {}개 추가, 링크 {}개 확인. 검색 서비스에는 행동 데이터가 전달되지 않습니다."
            ),
            &[&search.queries, &search.added, &search.verified],
        ));
    }

    lines.extend([
        String::new(),
        format!("## {}", tr!(lang, "依据", "Evidence", "根拠", "근거")),
    ]);
    for item in evidence {
        lines.push(fill(
            tr!(lang, "- {}：{}", "- {}: {}", "- {}：{}", "- {}: {}"),
            &[&item.id, &item.text],
        ));
    }
    lines.push(String::new());
    lines.push(tr!(
        lang,
        "工具只供人工评估，TraceBit 不会自动下载、安装或启用。",
        "Tools are for human review only; TraceBit never downloads, installs, or enables anything automatically.",
        "ツールは人が評価するためのものであり、TraceBit が自動でダウンロード・インストール・有効化することはありません。",
        "도구는 사람이 검토하기 위한 것이며, TraceBit이 자동으로 다운로드·설치·활성화하지 않습니다."
    ).to_owned());
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
        let lang = self.settings.get().lang();
        check_date(date_key, lang)?;
        Ok(self.reports_dir.join(format!("{date_key}.json")))
    }

    /// Writes the report as a standalone HTML page next to its JSON and returns
    /// the path. Rendered from the stored JSON, so older reports work too.
    pub fn write_html(&self, report: &Report) -> Result<PathBuf> {
        let lang = self.settings.get().lang();
        check_date(&report.date_key, lang)?;
        let file = self.reports_dir.join(format!("{}.html", report.date_key));
        fs::write(&file, crate::report_html::render(report, lang))?;
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
        let lang = self.settings.get().lang();
        check_date(date_key, lang)?;
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
        let lang = self.settings.get().lang();
        check_date(date_key, lang)?;
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
        let lang = self.settings.get().lang();
        check_date(date_key, lang)?;
        let stats = self.store.day_stats(date_key, self.options())?;
        let recommendations = build_recommendations(&stats, lang);
        let Some(item) = recommendations.iter().find(|item| item.id == opportunity) else {
            bail!(tr!(
                lang,
                "找不到这个工具候选",
                "This tool candidate was not found.",
                "このツール候補が見つかりません。",
                "이 도구 후보를 찾을 수 없습니다."
            ));
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

    fn profile_evidence(snapshot: &ProfileSnapshot, lang: Lang) -> Vec<Evidence> {
        snapshot
            .lines(lang)
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
        let lang = self.settings.get().lang();
        check_date(date_key, lang)?;
        let stats = self.store.day_stats(date_key, self.options())?;
        let metrics = day_metrics(&stats);
        let history_stats = self.history(date_key);
        let history_metrics = history_stats.iter().map(day_metrics).collect::<Vec<_>>();
        let history = history_stats
            .iter()
            .zip(&history_metrics)
            .map(|(stats, metrics)| History { stats, metrics })
            .collect::<Vec<_>>();
        let mut evidence = build_evidence(&stats, &metrics, &history, lang);

        // Today is folded in memory so a preview shows exactly what generation
        // would send; generation stores the same state right after.
        let mut profile = self.profile();
        profile.observe(date_key, &stats);
        let snapshot = profile.snapshot(lang);
        let profile_evidence = Self::profile_evidence(&snapshot, lang);

        let summaries = self.summaries(date_key);
        let all_recommendations = build_recommendations(&stats, lang);
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
            &profile.decisions_for_prompt(lang),
            &adoptions,
            lang,
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
            system_prompt: system_prompt(lang),
            user_prompt,
        })
    }

    /// What would be sent to the model, for the "preview" button.
    pub fn preview_prompt(&self, date_key: &str) -> Result<String> {
        let lang = self.settings.get().lang();
        let context = self.build_context(date_key)?;
        Ok(fill(
            tr!(
                lang,
                "【系统提示】\n{}\n\n【发送的数据】\n{}",
                "[System prompt]\n{}\n\n[Data sent]\n{}",
                "【システムプロンプト】\n{}\n\n【送信データ】\n{}",
                "[시스템 프롬프트]\n{}\n\n[전송 데이터]\n{}"
            ),
            &[&context.system_prompt, &context.user_prompt],
        ))
    }

    pub fn generate_report(&self, date_key: &str) -> Result<Report> {
        self.generate_report_as(date_key, "manual")
    }

    pub fn generate_report_as(&self, date_key: &str, source: &str) -> Result<Report> {
        let config = self.settings.get();
        let lang = config.lang();
        let ai = config.ai;
        let endpoint = ai.text_endpoint();
        // Learn this day even if the model call fails: the profile is just an
        // observation, not a generated artifact.
        self.observe_profile(date_key)?;
        let context = self.build_context(date_key)?;
        if context.stats.events == 0 {
            bail!(tr!(
                lang,
                "当天没有记录数据",
                "No activity was recorded for this day.",
                "この日の記録がありません。",
                "이 날 기록된 데이터가 없습니다."
            ));
        }
        if context.stats.total_active_ms < 60_000 {
            bail!(tr!(
                lang,
                "当天活跃时长不足 1 分钟，无法生成有意义的建议",
                "Less than a minute of activity was recorded, so no meaningful advice can be generated.",
                "この日のアクティブ時間が 1 分未満のため、意味のある提案を作成できません。",
                "이 날 활동 시간이 1분 미만이라 의미 있는 제안을 만들 수 없습니다."
            ));
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
        let first = llm::chat(&endpoint, messages.clone(), options, lang)?;
        let mut usage = first.usage.clone();
        let mut model = first.model.clone();
        let parsed = parse_advice(&first.content);
        let advice = match parsed {
            Some(advice) => advice,
            None if first.truncated => {
                bail!(tr!(
                    lang,
                    "AI 输出被截断，请换用上下文更长或非推理模型",
                    "The AI output was truncated. Use a model with a larger context or a non-reasoning model.",
                    "AI の出力が途中で切れました。より長いコンテキストのモデルか、非推論モデルを使ってください。",
                    "AI 출력이 잘렸습니다. 더 긴 컨텍스트를 가진 모델이나 비추론 모델을 사용하세요."
                ));
            }
            None => {
                // One repair round: show the model its own output.
                let mut repair = messages.as_array().cloned().unwrap_or_default();
                repair.push(json!({"role": "assistant", "content": first.content}));
                repair.push(json!({"role": "user", "content": tr!(
                    lang,
                    "上面的输出不是符合要求结构的 JSON。请只输出那个 JSON 对象，不要任何其他文字。",
                    "The output above is not JSON in the required shape. Output only that JSON object, with no other text.",
                    "上記の出力は要求された構造の JSON ではありません。その JSON オブジェクトだけを出力し、他の文字は入れないでください。",
                    "위 출력은 요구된 구조의 JSON이 아닙니다. 해당 JSON 객체만 출력하고 다른 텍스트는 넣지 마세요."
                )}));
                let second = llm::chat(&endpoint, Value::Array(repair), options, lang)?;
                usage.prompt_tokens += second.usage.prompt_tokens;
                usage.completion_tokens += second.usage.completion_tokens;
                model = second.model.clone();
                parse_advice(&second.content).ok_or_else(|| {
                    anyhow::anyhow!(tr!(
                        lang,
                        "AI 两次都没有返回有效的 JSON 建议",
                        "The AI did not return valid JSON advice after two attempts.",
                        "AI が 2 回とも有効な JSON の提案を返しませんでした。",
                        "AI가 두 번 모두 유효한 JSON 제안을 반환하지 않았습니다."
                    ))
                })?
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
            lang,
        )?;
        let enriched = enrich_recommendations(&context.recommendations, &ai, lang);
        let markdown = render_markdown(
            date_key,
            &advice,
            &context.metrics,
            &context.evidence,
            &enriched.recommendations,
            &enriched.search,
            &context.adoptions,
            lang,
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
        let lang = self.settings.get().lang();
        let ai = self.settings.get().ai;
        let output = llm::chat(
            &ai.text_endpoint(),
            json!([{"role":"user", "content":"请只回复两个字：正常"}]),
            ChatOptions {
                max_tokens: 16,
                timeout_ms: 30_000,
                ..Default::default()
            },
            lang,
        )?;
        let mut model = output.model;
        if ai.screenshot_analysis {
            let vision = crate::intervals::test_vision(&ai, lang)?;
            model = fill(
                tr!(
                    lang,
                    "{0}；视觉 {1}",
                    "{0}; vision {1}",
                    "{0}、ビジョン {1}",
                    "{0}; 비전 {1}"
                ),
                &[&model, &vision],
            );
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
        // Chinese assertions in these tests rely on the stored report and
        // evidence being generated in Chinese.
        settings.patch(&json!({"app": {"language": "zh"}})).unwrap();
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
    fn guard_covers_every_output_language_without_blocks_on_steps() {
        for text in [
            "Stop using Slack for status updates",
            "Reduce your screen time during deep work",
            "Turn off notifications while writing",
            "通知をオフにしましょう",
            "使用時間を減らす",
            "알림을 끄세요",
            "사용 시간을 줄이세요",
        ] {
            assert!(pushes_behavior(text), "should block: {text}");
        }
        for text in [
            "Reduce steps with a task runner",
            "One keystroke opens the command palette",
            "手順を減らすタスクランナー",
            "단계를 줄이는 설정",
        ] {
            assert!(!pushes_behavior(text), "should keep: {text}");
        }
    }

    #[test]
    fn enums_normalize_from_both_key_sets_and_labels_follow_language() {
        assert_eq!(
            normalize_choice("工具", &UPGRADE_TYPES, &LEGACY_KINDS, "tool"),
            "tool"
        );
        assert_eq!(
            normalize_choice("automation", &UPGRADE_TYPES, &LEGACY_KINDS, "tool"),
            "automation"
        );
        assert_eq!(
            normalize_choice("高", &LEVELS, &LEGACY_LEVELS, "medium"),
            "high"
        );
        assert_eq!(
            normalize_choice("medium", &LEVELS, &LEGACY_LEVELS, "medium"),
            "medium"
        );
        assert_eq!(kind_label(Lang::En, "工具"), "Tool");
        assert_eq!(kind_label(Lang::Zh, "integration"), "集成");
        assert_eq!(level_label(Lang::Ko, "low"), "낮음");
        assert_eq!(level_label(Lang::Ja, "high"), "高");
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
        let snapshot = profile.snapshot(Lang::Zh);
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
