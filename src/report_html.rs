//! Renders a saved report as one self-contained HTML page (no scripts, no
//! external assets) that opens in the default browser.

use crate::{
    analyzer::{ExperimentReview, Report, Suggestion},
    insights::{Better, metric_def},
    store::local_datetime,
};
use std::{collections::BTreeSet, fmt::Write};

fn esc(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for ch in value.trim().chars() {
        match ch {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(ch),
        }
    }
    output
}

fn num(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value:.1}")
    }
}

/// Only http(s) links are emitted, so stored data cannot inject `javascript:`.
fn safe_url(url: &str) -> Option<String> {
    let url = url.trim();
    (url.starts_with("https://") || url.starts_with("http://")).then(|| esc(url))
}

fn evidence_refs(ids: &[String]) -> String {
    if ids.is_empty() {
        return String::new();
    }
    let links = ids
        .iter()
        .map(|id| format!(r##"<a href="#ev-{0}">{0}</a>"##, esc(id)))
        .collect::<Vec<_>>()
        .join("");
    format!(r#"<span class="refs">{links}</span>"#)
}

fn unverified_note(numbers: &[String], prefix: &str) -> String {
    if numbers.is_empty() {
        return String::new();
    }
    format!(
        r#"<p class="warn">{prefix}{} 无法在记录中核实，请谨慎参考。</p>"#,
        esc(&numbers.join("、"))
    )
}

fn review_html(review: &ExperimentReview, followup: &str) -> String {
    let experiment = &review.experiment;
    let (label, unit) = metric_def(&experiment.metric)
        .map(|def| (def.label, def.unit))
        .unwrap_or(("指标", ""));
    let (state, class) = match review.achieved {
        Some(true) => ("已达成", "ok"),
        Some(false) => ("未达成", "miss"),
        None => ("无法判断", "na"),
    };
    let comparator = if review
        .experiment
        .baseline
        .is_none_or(|baseline| experiment.target >= baseline)
    {
        "≥"
    } else {
        "≤"
    };
    let mut html = format!(
        r#"<section class="review"><div class="review-head"><h2>上次实验回顾</h2><span class="badge {class}">{state}</span></div>
<p class="review-action">{}</p>
<div class="review-nums"><div><span>当时</span><b>{}</b><small>{unit}</small></div><div><span>目标 {comparator}</span><b>{}</b><small>{unit}</small></div><div><span>实际</span><b>{}</b><small>{unit}</small></div></div>
<p class="muted">{label} · {} 提出</p>"#,
        esc(&experiment.action),
        experiment.baseline.map(num).unwrap_or_else(|| "—".into()),
        num(experiment.target),
        review.actual.map(num).unwrap_or_else(|| "—".into()),
        esc(&experiment.date_key),
    );
    if !followup.trim().is_empty() {
        let _ = write!(html, r#"<p class="followup">{}</p>"#, esc(followup));
    }
    html.push_str("</section>");
    html
}

fn suggestion_html(index: usize, suggestion: &Suggestion, report: &Report) -> String {
    let mut html = format!(
        r#"<article class="card"><header><span class="idx">{}</span><div><h3>{}</h3><div class="tags"><span class="tag">{}</span><span>投入 <b>{}</b></span><span>把握 <b>{}</b></span>{}</div></div></header>"#,
        index + 1,
        esc(&suggestion.title),
        esc(&suggestion.kind),
        esc(&suggestion.effort),
        esc(&suggestion.confidence),
        evidence_refs(&suggestion.evidence),
    );
    if !suggestion.why.trim().is_empty() {
        let _ = write!(html, r#"<p class="why">{}</p>"#, esc(&suggestion.why));
    }
    html.push_str(r#"<ol class="steps">"#);
    for action in &suggestion.actions {
        let _ = write!(html, "<li>{}</li>", esc(action));
    }
    html.push_str("</ol>");
    let mut facts = String::new();
    if !suggestion.when.trim().is_empty() {
        let _ = write!(facts, "<dt>何时</dt><dd>{}</dd>", esc(&suggestion.when));
    }
    if !suggestion.expected.trim().is_empty() {
        let _ = write!(facts, "<dt>预期</dt><dd>{}</dd>", esc(&suggestion.expected));
    }
    if !facts.is_empty() {
        let _ = write!(html, r#"<dl class="facts">{facts}</dl>"#);
    }
    for tool in &suggestion.tools {
        let Some(item) = report.recommendations.iter().find(|item| &item.id == tool) else {
            continue;
        };
        for resource in &item.resources {
            html.push_str(&resource_html(resource));
        }
    }
    html.push_str(&unverified_note(&suggestion.unverified_numbers, "数字 "));
    html.push_str("</article>");
    html
}

fn resource_html(resource: &crate::recommendations::Resource) -> String {
    let name = match safe_url(&resource.url) {
        Some(url) => format!(r#"<a href="{url}" target="_blank" rel="noreferrer">{}</a>"#, esc(&resource.name)),
        None => esc(&resource.name),
    };
    format!(
        r#"<div class="tool"><div class="tool-head">{name}<span>{}</span></div><p>{}</p><p class="muted">配置成本：{} · 注意：{}</p></div>"#,
        esc(&resource.kind),
        esc(&resource.description),
        esc(&resource.setup_cost),
        esc(&resource.permissions),
    )
}

/// Builds the full HTML page for a report.
pub fn render(report: &Report) -> String {
    let mut body = String::new();
    let generated = local_datetime(report.generated_at)
        .map(|date| date.format("%m-%d %H:%M").to_string())
        .unwrap_or_default();
    let headline = report
        .advice
        .as_ref()
        .map(|advice| advice.headline.clone())
        .unwrap_or_default();

    let _ = write!(
        body,
        r#"<header class="top"><p class="eyebrow">TraceBit · {} 效率建议</p><h1>{}</h1></header>"#,
        esc(&report.date_key),
        if headline.trim().is_empty() { "效率建议".to_owned() } else { esc(&headline) },
    );

    if let Some(metrics) = &report.metrics {
        body.push_str(r#"<section class="metrics">"#);
        for key in [
            "active_min",
            "deep_work_min",
            "deep_blocks",
            "switches_per_hour",
            "comm_checks",
            "distraction_min",
            "longest_stretch_min",
            "meeting_min",
        ] {
            let (Some(def), Some(value)) = (metric_def(key), metrics.get(key)) else {
                continue;
            };
            let flag = match def.better {
                Better::Lower if key == "switches_per_hour" && value > 40.0 => " bad",
                Better::Lower if key == "longest_stretch_min" && value > 90.0 => " bad",
                Better::Higher if value == 0.0 => " bad",
                _ => "",
            };
            let _ = write!(
                body,
                r#"<div class="metric{flag}"><span>{}</span><b>{}</b><small>{}</small></div>"#,
                def.label,
                num(value),
                def.unit
            );
        }
        body.push_str("</section>");
    }

    let Some(advice) = &report.advice else {
        // Reports from older versions only have Markdown.
        let _ = write!(body, r#"<pre class="legacy">{}</pre>"#, esc(&report.markdown));
        return page(report, &body, &generated);
    };

    if let Some(review) = &report.review {
        body.push_str(&review_html(review, &advice.followup));
    }

    if !advice.diagnosis.is_empty() {
        body.push_str(r#"<section><h2>关键发现</h2><ul class="findings">"#);
        for finding in &advice.diagnosis {
            let _ = write!(
                body,
                "<li><p>{}</p>{}</li>",
                esc(&finding.finding),
                evidence_refs(&finding.evidence)
            );
        }
        body.push_str("</ul>");
        body.push_str(&unverified_note(&advice.unverified_numbers, "以上发现中的数字 "));
        body.push_str("</section>");
    }

    body.push_str(r#"<section><h2>建议</h2><div class="cards">"#);
    for (index, suggestion) in advice.suggestions.iter().enumerate() {
        body.push_str(&suggestion_html(index, suggestion, report));
    }
    body.push_str("</div></section>");

    if let Some(experiment) = &advice.experiment {
        let (label, unit) = metric_def(&experiment.metric)
            .map(|def| (def.label, def.unit))
            .unwrap_or(("指标", ""));
        let increase = experiment
            .baseline
            .is_none_or(|baseline| experiment.target >= baseline);
        let _ = write!(
            body,
            r#"<section class="experiment"><h2>明日实验</h2>
<p class="hypo">{}</p><p>{}</p>
<div class="goal"><div><span>今天</span><b>{}</b><small>{unit}</small></div><div class="arrow">{}</div><div><span>目标</span><b>{}</b><small>{unit}</small></div><p>{label} · 下一份建议会自动核对结果</p></div></section>"#,
            esc(&experiment.hypothesis),
            esc(&experiment.action),
            experiment.baseline.map(num).unwrap_or_else(|| "—".into()),
            if increase { "提高到 →" } else { "降低到 →" },
            num(experiment.target),
        );
    }

    let used = advice
        .suggestions
        .iter()
        .flat_map(|suggestion| suggestion.tools.iter())
        .collect::<BTreeSet<_>>();
    let unused = report
        .recommendations
        .iter()
        .filter(|item| !used.contains(&item.id))
        .collect::<Vec<_>>();
    if !unused.is_empty() {
        body.push_str("<section><h2>其他可评估的工具</h2>");
        for item in unused {
            let _ = write!(
                body,
                r#"<div class="other"><h3>{}</h3><p class="muted">依据：{}</p>"#,
                esc(&item.title),
                esc(&item.evidence)
            );
            for resource in &item.resources {
                body.push_str(&resource_html(resource));
            }
            body.push_str("</div>");
        }
        body.push_str("</section>");
    }

    if !report.evidence.is_empty() {
        let _ = write!(
            body,
            r#"<section><details class="evidence" open><summary>依据 · {} 条</summary><dl>"#,
            report.evidence.len()
        );
        for item in &report.evidence {
            let _ = write!(
                body,
                r#"<dt id="ev-{0}">{0}</dt><dd>{1}</dd>"#,
                esc(&item.id),
                esc(&item.text)
            );
        }
        body.push_str("</dl></details></section>");
    }
    page(report, &body, &generated)
}

fn page(report: &Report, body: &str, generated: &str) -> String {
    let mut meta = vec![
        format!("模型 {}", esc(&report.model)),
        format!(
            "{} {generated}",
            if report.source == "auto" { "自动生成" } else { "手动生成" }
        ),
    ];
    if report.usage.prompt_tokens + report.usage.completion_tokens > 0 {
        meta.push(format!(
            "Token {} + {}",
            report.usage.prompt_tokens, report.usage.completion_tokens
        ));
    }
    if !report.prompt_version.is_empty() {
        meta.push(esc(&report.prompt_version));
    }
    if report.resource_search.enabled {
        meta.push(format!(
            "联网检索 {} 次，验证 {} 个链接（搜索服务不会收到行为数据）",
            report.resource_search.queries, report.resource_search.verified
        ));
    }
    format!(
        r#"<!doctype html>
<html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'">
<title>{date} 效率建议 · TraceBit</title><style>{STYLE}</style></head>
<body><main>{body}
<footer><p>{meta}</p><p>建议基于本地记录计算的证据；工具只供人工评估，TraceBit 不会自动下载、安装或启用。</p></footer>
</main></body></html>"#,
        date = esc(&report.date_key),
        meta = meta.join(" · "),
    )
}

const STYLE: &str = r#"
:root{--bg:#f4f1ea;--paper:#fffdf8;--ink:#1f2326;--sub:#5a6168;--faint:#8a9097;--line:#e3ddd0;--accent:#b7791f;--accent-pale:#f6ecd7;--green:#2f7d57;--red:#b64b4b;}
@media (prefers-color-scheme:dark){:root{--bg:#17191b;--paper:#1f2225;--ink:#ecebe7;--sub:#b2b6ba;--faint:#80868c;--line:#33383c;--accent:#e0a54a;--accent-pale:#3a2f1c;--green:#5fb487;--red:#e07a7a;}}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--ink);font:15px/1.7 "Segoe UI","Microsoft YaHei UI","PingFang SC",sans-serif;-webkit-font-smoothing:antialiased}
main{max-width:860px;margin:0 auto;padding:56px 28px 40px}
.eyebrow{margin:0 0 10px;color:var(--accent);font-size:13px;font-weight:600;letter-spacing:.04em}
h1{margin:0;font-size:30px;line-height:1.35;font-weight:700;letter-spacing:-.01em}
h2{font-size:13px;font-weight:600;color:var(--sub);letter-spacing:.06em;margin:0 0 14px}
h3{margin:0;font-size:17px;line-height:1.45}
section{margin-top:40px}
p{margin:0}
a{color:var(--accent)}
.muted{color:var(--faint);font-size:13px}
.metrics{display:grid;grid-template-columns:repeat(4,1fr);border:1px solid var(--line);border-radius:10px;background:var(--paper);overflow:hidden;margin-top:28px}
.metric{padding:14px 16px;border-right:1px solid var(--line);border-bottom:1px solid var(--line);display:flex;flex-direction:column}
.metric:nth-child(4n){border-right:0}
.metric:nth-last-child(-n+4){border-bottom:0}
.metric span{font-size:12px;color:var(--sub)}
.metric b{font-size:24px;font-weight:650;font-variant-numeric:tabular-nums;line-height:1.3}
.metric small{font-size:12px;color:var(--faint)}
.metric.bad b{color:var(--red)}
.findings{list-style:none;margin:0;padding:0;border-top:1px solid var(--line)}
.findings li{padding:14px 0;border-bottom:1px solid var(--line);display:flex;gap:16px;justify-content:space-between;align-items:baseline}
.refs{display:inline-flex;gap:4px;flex-shrink:0}
.refs a{font:12px/1 ui-monospace,Consolas,monospace;color:var(--sub);text-decoration:none;border:1px solid var(--line);border-radius:4px;padding:3px 5px}
.refs a:hover{color:var(--accent);border-color:var(--accent)}
.cards{display:flex;flex-direction:column;gap:14px}
.card{background:var(--paper);border:1px solid var(--line);border-radius:10px;padding:20px 22px}
.card header{display:flex;gap:14px;align-items:flex-start}
.idx{flex-shrink:0;width:28px;height:28px;border-radius:50%;background:var(--ink);color:var(--paper);display:grid;place-items:center;font-size:14px;font-weight:600;margin-top:1px}
.card:first-child .idx{background:var(--accent)}
.tags{display:flex;flex-wrap:wrap;gap:12px;align-items:center;margin-top:6px;font-size:13px;color:var(--sub)}
.tags b{color:var(--ink);font-weight:600}
.tag{background:var(--accent-pale);color:var(--ink);padding:1px 8px;border-radius:4px}
.why{margin:14px 0 0 42px;color:var(--sub)}
.steps{margin:12px 0 0 42px;padding-left:20px}
.steps li{padding:2px 0}
.facts{display:grid;grid-template-columns:auto 1fr;gap:6px 14px;margin:14px 0 0 42px;padding-top:12px;border-top:1px dashed var(--line);font-size:14px}
.facts dt{color:var(--faint)}
.facts dd{margin:0}
.tool{margin:12px 0 0 42px;padding:12px 14px;border-radius:8px;background:var(--bg);font-size:14px}
.other .tool{margin-left:0}
.other{margin-bottom:18px}
.tool-head{display:flex;gap:10px;align-items:baseline;font-weight:600}
.tool-head span{font-weight:400;font-size:12px;color:var(--faint)}
.warn{margin-top:12px;font-size:13px;color:var(--red)}
.experiment{background:var(--accent-pale);border-radius:10px;padding:22px 24px}
.experiment h2{color:var(--ink)}
.hypo{font-size:17px;font-weight:600;margin-bottom:6px}
.goal{display:flex;align-items:flex-end;gap:20px;flex-wrap:wrap;margin-top:18px}
.goal div,.review-nums div{display:flex;flex-direction:column}
.goal span,.review-nums span{font-size:12px;color:var(--sub)}
.goal b{font-size:32px;line-height:1.2;font-variant-numeric:tabular-nums}
.goal small,.review-nums small{font-size:12px;color:var(--faint)}
.goal .arrow{color:var(--sub);font-size:14px;padding-bottom:22px}
.goal p{flex-basis:100%;font-size:13px;color:var(--sub)}
.review{border:1px solid var(--line);border-radius:10px;padding:20px 22px;background:var(--paper)}
.review-head{display:flex;justify-content:space-between;align-items:center}
.review-head h2{margin:0}
.badge{font-size:12px;padding:2px 10px;border-radius:999px;border:1px solid currentColor}
.badge.ok{color:var(--green)}.badge.miss{color:var(--red)}.badge.na{color:var(--faint)}
.review-action{margin:10px 0 14px}
.review-nums{display:flex;gap:36px;margin-bottom:8px}
.review-nums b{font-size:22px;font-variant-numeric:tabular-nums}
.followup{margin-top:12px;padding-top:12px;border-top:1px solid var(--line);color:var(--sub)}
.evidence summary{cursor:pointer;font-size:13px;font-weight:600;color:var(--sub);letter-spacing:.06em}
.evidence dl{display:grid;grid-template-columns:auto 1fr;gap:8px 14px;margin:14px 0 0;font-size:13px;color:var(--sub)}
.evidence dt{font-family:ui-monospace,Consolas,monospace;color:var(--ink);scroll-margin-top:20px}
.evidence dt:target,.evidence dt:target+dd{color:var(--accent)}
.evidence dd{margin:0}
.legacy{white-space:pre-wrap;font:inherit;background:var(--paper);border:1px solid var(--line);border-radius:10px;padding:20px}
footer{margin-top:48px;padding-top:16px;border-top:1px solid var(--line);font-size:12px;color:var(--faint)}
footer p+p{margin-top:4px}
@media (max-width:640px){main{padding:32px 16px}.metrics{grid-template-columns:repeat(2,1fr)}.metric:nth-child(n){border-right:1px solid var(--line);border-bottom:1px solid var(--line)}.metric:nth-child(2n){border-right:0}.why,.steps,.facts,.tool{margin-left:0}}
@media print{body{background:#fff}.card,.review,.metrics{break-inside:avoid}}
"#;
