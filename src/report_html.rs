//! Renders a saved report as one self-contained HTML page (no scripts, no
//! external assets) that opens in the default browser.

use crate::{
    analyzer::{AdoptionReview, Report, Upgrade},
    insights::metric_def,
    profile::Snapshot,
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

fn profile_html(profile: &Snapshot) -> String {
    let mut html = format!(
        r#"<section class="profile"><div class="profile-head"><h2>工作画像 · 本地学习</h2><span class="muted">{} 个记录日 · {}</span></div>"#,
        profile.days,
        esc(&profile.span)
    );
    if !profile.tools.is_empty() {
        html.push_str(r#"<div class="tools">"#);
        for tool in &profile.tools {
            let trend = if tool.trend.is_empty() {
                String::new()
            } else {
                format!(r#"<em>{}</em>"#, esc(&tool.trend))
            };
            let _ = write!(
                html,
                r#"<div class="tool-row"><span class="role">{}</span><b>{}</b><span class="mins">日均 {} 分钟 · {} 天</span>{trend}</div>"#,
                esc(&tool.role),
                esc(&tool.name),
                tool.avg_min,
                tool.days
            );
        }
        html.push_str("</div>");
    }
    if !profile.loops.is_empty() {
        html.push_str(r#"<ul class="loops">"#);
        for loop_ in &profile.loops {
            let _ = write!(
                html,
                r#"<li><b>{} ↔ {}</b><span>日均 {:.1} 次，其中一侧平均停留约 {} 秒</span></li>"#,
                esc(&loop_.a),
                esc(&loop_.b),
                loop_.per_day,
                loop_.dwell_secs()
            );
        }
        html.push_str("</ul>");
    }
    let rhythm = &profile.rhythm;
    let span = match (rhythm.start.is_empty(), rhythm.end.is_empty()) {
        (false, false) => format!("常在 {}–{} 活跃", rhythm.start, rhythm.end),
        (false, true) => format!("常在 {} 之后活跃", rhythm.start),
        _ => String::new(),
    };
    let mut facts = Vec::new();
    if !span.is_empty() {
        facts.push(span);
    }
    if !rhythm.focus_window.is_empty() {
        facts.push(format!("产出集中在 {}", rhythm.focus_window));
    }
    if rhythm.weekend_share > 0 {
        facts.push(format!("周末占 {}% 的活跃时间", rhythm.weekend_share));
    }
    if !facts.is_empty() {
        let _ = write!(html, r#"<p class="rhythm">{}</p>"#, esc(&facts.join(" · ")));
    }
    if !profile.themes.is_empty() {
        html.push_str(r#"<div class="chips">"#);
        for theme in &profile.themes {
            let _ = write!(
                html,
                r#"<span class="chip">{}<small>{} 天 · {} 分钟</small></span>"#,
                esc(&theme.label),
                theme.days,
                theme.total_min
            );
        }
        html.push_str("</div>");
    }
    if !profile.sites.is_empty() {
        html.push_str(r#"<p class="muted sites">常访问："#);
        html.push_str(
            &profile
                .sites
                .iter()
                .map(|site| format!("{}（日均 {} 分钟）", esc(&site.site), site.per_day))
                .collect::<Vec<_>>()
                .join("、"),
        );
        html.push_str("</p>");
    }
    html.push_str("</section>");
    html
}

fn upgrade_html(index: usize, upgrade: &Upgrade, report: &Report) -> String {
    let mut html = format!(
        r#"<article class="card"><header><span class="idx">{}</span><div><h3>{}</h3><div class="tags"><span class="tag">{}</span><span>投入 <b>{}</b></span><span>把握 <b>{}</b></span>{}</div></div></header>"#,
        index + 1,
        esc(&upgrade.title),
        esc(&upgrade.kind),
        esc(&upgrade.effort),
        esc(&upgrade.confidence),
        evidence_refs(&upgrade.evidence),
    );
    if !upgrade.habit.trim().is_empty() {
        let _ = write!(
            html,
            r#"<p class="habit">接上你现在：{}</p>"#,
            esc(&upgrade.habit)
        );
    }
    if !upgrade.problem.trim().is_empty() {
        let _ = write!(html, r#"<p class="problem">{}</p>"#, esc(&upgrade.problem));
    }
    if !upgrade.alternative.trim().is_empty() {
        let _ = write!(
            html,
            r#"<p class="alternative">替换：{}</p>"#,
            esc(&upgrade.alternative)
        );
    }
    html.push_str(r#"<ol class="steps">"#);
    for action in &upgrade.actions {
        let _ = write!(html, "<li>{}</li>", esc(action));
    }
    html.push_str("</ol>");
    let mut facts = String::new();
    if !upgrade.payoff.trim().is_empty() {
        let _ = write!(facts, "<dt>预期</dt><dd>{}</dd>", esc(&upgrade.payoff));
    }
    if !upgrade.risk.trim().is_empty() {
        let _ = write!(facts, "<dt>风险</dt><dd>{}</dd>", esc(&upgrade.risk));
    }
    if !facts.is_empty() {
        let _ = write!(html, r#"<dl class="facts">{facts}</dl>"#);
    }
    for tool in &upgrade.tools {
        let Some(item) = report.recommendations.iter().find(|item| &item.id == tool) else {
            continue;
        };
        for resource in &item.resources {
            html.push_str(&resource_html(resource));
        }
    }
    html.push_str(&unverified_note(&upgrade.unverified_numbers, "数字 "));
    html.push_str("</article>");
    html
}

fn resource_html(resource: &crate::recommendations::Resource) -> String {
    let name = match safe_url(&resource.url) {
        Some(url) => format!(
            r#"<a href="{url}" target="_blank" rel="noreferrer">{}</a>"#,
            esc(&resource.name)
        ),
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

fn adoption_html(adoptions: &[AdoptionReview], followup: &str) -> String {
    if adoptions.is_empty() && followup.trim().is_empty() {
        return String::new();
    }
    let mut html = String::from(r#"<section class="adoption"><h2>采纳回看</h2>"#);
    for item in adoptions {
        let _ = write!(
            html,
            r#"<div class="adoption-row"><b>{}</b><span class="muted">{} 起</span><p>{}</p><p class="muted">当时：{}</p>{}</div>"#,
            esc(&item.resource),
            esc(&item.date_key),
            if item.after.is_some() {
                "今天同类动作的观察结果"
            } else {
                "今天这类来回已经不再是候选"
            },
            esc(&item.before),
            item.after
                .as_ref()
                .map(|after| format!("<p>现在：{}</p>", esc(after)))
                .unwrap_or_default()
        );
    }
    if !followup.trim().is_empty() {
        let _ = write!(html, r#"<p class="followup">{}</p>"#, esc(followup));
    }
    html.push_str("</section>");
    html
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
        r#"<header class="top"><p class="eyebrow">TraceBit · {} 工具升级建议</p><h1>{}</h1></header>"#,
        esc(&report.date_key),
        if headline.trim().is_empty() {
            "工具升级建议".to_owned()
        } else {
            esc(&headline)
        },
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
            let _ = write!(
                body,
                r#"<div class="metric"><span>{}</span><b>{}</b><small>{}</small></div>"#,
                def.label,
                num(value),
                def.unit
            );
        }
        body.push_str("</section>");
    }

    let Some(advice) = &report.advice else {
        // Reports from older versions only have Markdown.
        let _ = write!(
            body,
            r#"<pre class="legacy">{}</pre>"#,
            esc(&report.markdown)
        );
        return page(report, &body, &generated);
    };

    if let Some(profile) = &report.profile {
        body.push_str(&profile_html(profile));
    }

    if !advice.reading.is_empty() {
        body.push_str(r#"<section><h2>你的工作方式</h2><ul class="findings">"#);
        for item in &advice.reading {
            let _ = write!(
                body,
                "<li><p>{}</p>{}<span class=\"muted\">{}</span></li>",
                esc(&item.observation),
                evidence_refs(&item.evidence),
                esc(&item.confidence)
            );
        }
        body.push_str("</ul>");
        body.push_str(&unverified_note(
            &advice.unverified_numbers,
            "以上描述中的数字 ",
        ));
        body.push_str("</section>");
    }

    body.push_str(&adoption_html(&report.adoptions, &advice.followup));

    body.push_str(r#"<section><h2>工具升级机会</h2><div class="cards">"#);
    for (index, upgrade) in advice.upgrades.iter().enumerate() {
        body.push_str(&upgrade_html(index, upgrade, report));
    }
    body.push_str("</div></section>");

    if !advice.watch.is_empty() {
        body.push_str(r#"<section class="watch"><h2>下次自动核对</h2><p class="muted">这些指标不需要你做任何事，下次生成时会自动对照。</p>"#);
        for item in &advice.watch {
            let (label, unit) = metric_def(&item.metric)
                .map(|def| (def.label, def.unit))
                .unwrap_or(("指标", ""));
            let _ = write!(
                body,
                r#"<div class="watch-row"><b>{label}</b><span class="now">今天 {}{unit}</span><p>{}</p></div>"#,
                num(item.baseline.unwrap_or_default()),
                esc(&item.reason)
            );
        }
        body.push_str("</section>");
    }

    if !advice.filtered.is_empty() {
        let _ = write!(
            body,
            r#"<p class="warn filtered">已过滤 {} 条要求改变习惯的内容；本报告只保留能让现有动作更省力的工具建议。</p>"#,
            advice.filtered.len()
        );
    }

    let used = advice
        .upgrades
        .iter()
        .flat_map(|upgrade| upgrade.tools.iter())
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
            if !item.replaces.is_empty() {
                let _ = write!(
                    body,
                    r#"<p class="muted">可替代：{}</p>"#,
                    esc(&item.replaces.join("、"))
                );
            }
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
            if report.source == "auto" {
                "自动生成"
            } else {
                "手动生成"
            }
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
<title>{date} 工具升级建议 · TraceBit</title><style>{STYLE}</style></head>
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
.profile-head{display:flex;justify-content:space-between;align-items:baseline;gap:16px;margin-bottom:14px}
.profile-head h2{margin:0}
.tools{border-top:1px solid var(--line)}
.tool-row{display:flex;align-items:baseline;gap:12px;padding:9px 0;border-bottom:1px solid var(--line)}
.tool-row .role{flex:0 0 78px;font-size:12px;color:var(--faint);letter-spacing:.04em}
.tool-row b{font-weight:600}
.tool-row .mins{color:var(--sub);font-size:13px}
.tool-row em{margin-left:auto;font-style:normal;font-size:12px;color:var(--accent)}
.loops{list-style:none;margin:16px 0 0;padding:0}
.loops li{padding:6px 0;border-bottom:1px dashed var(--line)}
.loops span{margin-left:10px;color:var(--sub);font-size:13px}
.rhythm{margin-top:16px;color:var(--sub)}
.chips{display:flex;flex-wrap:wrap;gap:8px;margin-top:14px}
.chip{display:inline-flex;flex-direction:column;background:var(--accent-pale);border-radius:6px;padding:5px 10px;font-size:13px}
.chip small{color:var(--sub);font-size:11px}
.sites{margin-top:14px}
.findings{list-style:none;margin:0;padding:0;border-top:1px solid var(--line)}
.findings li{padding:14px 0;border-bottom:1px solid var(--line);display:flex;gap:16px;justify-content:space-between;align-items:baseline}
.findings li p{flex:1}
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
.habit{margin:14px 0 0 42px;font-weight:600}
.problem{margin:6px 0 0 42px;color:var(--sub)}
.alternative{margin:6px 0 0 42px;font-size:13px;color:var(--accent)}
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
.filtered{margin-top:28px}
.watch{background:var(--accent-pale);border-radius:10px;padding:20px 22px}
.watch h2{color:var(--ink)}
.watch-row{display:grid;grid-template-columns:1fr auto;gap:4px 16px;padding:10px 0;border-top:1px solid rgba(0,0,0,.08)}
.watch-row .now{font-variant-numeric:tabular-nums;color:var(--sub);font-size:13px}
.watch-row p{grid-column:1/-1;color:var(--sub);font-size:14px}
.adoption{border:1px solid var(--line);border-radius:10px;padding:20px 22px;background:var(--paper)}
.adoption-row{padding:10px 0;border-top:1px solid var(--line)}
.adoption-row b{margin-right:10px}
.followup{margin-top:12px;padding-top:12px;border-top:1px solid var(--line);color:var(--sub)}
.evidence summary{cursor:pointer;font-size:13px;font-weight:600;color:var(--sub);letter-spacing:.06em}
.evidence dl{display:grid;grid-template-columns:auto 1fr;gap:8px 14px;margin:14px 0 0;font-size:13px;color:var(--sub)}
.evidence dt{font-family:ui-monospace,Consolas,monospace;color:var(--ink);scroll-margin-top:20px}
.evidence dt:target,.evidence dt:target+dd{color:var(--accent)}
.evidence dd{margin:0}
.legacy{white-space:pre-wrap;font:inherit;background:var(--paper);border:1px solid var(--line);border-radius:10px;padding:20px}
footer{margin-top:48px;padding-top:16px;border-top:1px solid var(--line);font-size:12px;color:var(--faint)}
footer p+p{margin-top:4px}
@media (max-width:640px){main{padding:32px 16px}.metrics{grid-template-columns:repeat(2,1fr)}.metric:nth-child(n){border-right:1px solid var(--line);border-bottom:1px solid var(--line)}.metric:nth-child(2n){border-right:0}.habit,.problem,.alternative,.steps,.facts,.tool{margin-left:0}.tool-row{flex-wrap:wrap}.tool-row .role{flex-basis:100%}}
@media print{body{background:#fff}.card,.adoption,.metrics{break-inside:avoid}}
"#;
