use crate::{
    categories::{is_meeting, normalize_proc},
    store::{AppStat, DayStats, Stay, local_datetime},
};
use regex::Regex;
use serde::{Deserialize, Serialize};

const MINUTE: i64 = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub description: String,
    pub benefit: String,
    pub setup_cost: String,
    pub permissions: String,
    pub source: String,
    pub verification: String,
    pub verified_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    pub id: String,
    pub title: String,
    pub evidence: String,
    pub summary: String,
    pub search_query: String,
    #[serde(skip)]
    pub score: f64,
    pub resources: Vec<Resource>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSummary {
    pub enabled: bool,
    pub provider: String,
    pub queries: u32,
    pub added: u32,
    pub verified: u32,
    pub errors: Vec<String>,
}

fn resource(id: &str) -> Option<Resource> {
    let (name, kind, url, description, benefit, setup_cost, permissions) = match id {
        "just" => (
            "just",
            "开源项目",
            "https://github.com/casey/just",
            "把重复的项目命令整理成带名称的任务入口。",
            "减少记忆命令和在编辑器、终端之间往返的成本。",
            "低，通常先整理 3 到 5 条常用命令。",
            "命令文件会执行本机命令，使用前先审阅任务内容。",
        ),
        "vscodeTasks" => (
            "VS Code Tasks",
            "内置自动化",
            "https://code.visualstudio.com/docs/editor/tasks",
            "把构建、测试和常用脚本留在编辑器内运行。",
            "减少为了固定命令而切换到终端的次数。",
            "低，可从一条最常用命令开始。",
            "任务会执行本机命令，不要运行来源不明的任务配置。",
        ),
        "lazygit" => (
            "lazygit",
            "开源项目",
            "https://github.com/jesseduffield/lazygit",
            "在终端内集中处理提交、分支、暂存和变基等 Git 操作。",
            "适合 Git 操作频繁时减少命令重复和上下文切换。",
            "中，需要熟悉基本 Git 概念和少量快捷键。",
            "会读写 Git 仓库，破坏性操作前仍需确认和备份。",
        ),
        "fetchMcp" => (
            "Fetch MCP Server",
            "MCP",
            "https://github.com/modelcontextprotocol/servers/tree/main/src/fetch",
            "让兼容的 Agent 在用户请求下读取网页并提取内容。",
            "减少复制网页正文到笔记或对话中的手工步骤。",
            "中，需要支持 MCP 的客户端和基础配置。",
            "会产生联网请求；不要抓取未授权或敏感页面，启用前核对服务器来源。",
        ),
        "agentSkills" => (
            "阅读研究 Skill 工作流",
            "Skill",
            "https://github.com/anthropics/skills",
            "参考官方 Skill 结构，把资料读取、来源整理和摘要写成可复用流程。",
            "适合反复进行资料收集和笔记整理的场景。",
            "中，需要先定义输入、输出和引用要求。",
            "Skill 能驱动宿主工具，安装前应审阅指令和允许调用的能力。",
        ),
        "searxng" => (
            "SearXNG",
            "开源项目",
            "https://github.com/searxng/searxng",
            "可自托管的元搜索服务，为研究工作流提供统一搜索入口。",
            "适合需要控制搜索数据流向的高级用户。",
            "高，需要部署和维护服务。",
            "上游搜索仍可能看到查询词，部署前检查日志和代理配置。",
        ),
        "whispercpp" => (
            "whisper.cpp",
            "开源项目",
            "https://github.com/ggml-org/whisper.cpp",
            "在本机把会议录音转写成文本，再接续摘要流程。",
            "减少手工整理逐字稿的工作量，音频不必交给云端转写服务。",
            "中，需要准备模型并确认本机性能。",
            "录音前必须取得参与者同意，音频和转写稿应按敏感资料管理。",
        ),
        "meetingSkill" => (
            "会议总结 Skill 工作流",
            "Skill",
            "https://github.com/anthropics/skills",
            "参考官方 Skill 结构，把转写稿整理成决议、行动项和负责人。",
            "把重复的报告格式和检查项固化下来。",
            "中，需要先用一份已核对纪要校准模板。",
            "转写稿可能含敏感信息，优先本地处理并限制工具权限。",
        ),
        "powerToys" => (
            "Microsoft PowerToys",
            "开源项目",
            "https://github.com/microsoft/PowerToys",
            "PowerRename 等工具可处理批量重命名和常见 Windows 操作。",
            "减少重复的文件命名和窗口整理动作。",
            "低，可先只启用一个实际需要的模块。",
            "批量修改前先备份并小范围试跑，避免误改文件名。",
        ),
        "autohotkey" => (
            "AutoHotkey",
            "开源项目",
            "https://github.com/AutoHotkey/AutoHotkey",
            "将明确、重复的 Windows 操作写成可审阅的自动化脚本。",
            "适合固定流程，不适合尚未弄清规则的一次性操作。",
            "中到高，需要编写、测试并维护脚本。",
            "脚本可模拟输入和操作文件，只运行自己理解且来源可信的脚本。",
        ),
        "bruno" => (
            "Bruno",
            "开源项目",
            "https://github.com/usebruno/bruno",
            "把 API 请求、环境和集合格式保存在本地项目中。",
            "减少重复填写请求和在不同窗口之间复制参数。",
            "低到中，可先迁移最常用的一组请求。",
            "环境文件可能含令牌，提交仓库前检查密钥和敏感地址。",
        ),
        "hoppscotch" => (
            "Hoppscotch",
            "开源项目",
            "https://github.com/hoppscotch/hoppscotch",
            "用于组织、发送和协作管理 API 请求。",
            "适合需要复用请求集合和统一调试入口的场景。",
            "低到中，可从单个项目开始。",
            "请求会接触接口和凭据，自托管或云同步前确认数据流向。",
        ),
        _ => return None,
    };
    Some(Resource {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: kind.to_owned(),
        url: url.to_owned(),
        description: description.to_owned(),
        benefit: benefit.to_owned(),
        setup_cost: setup_cost.to_owned(),
        permissions: permissions.to_owned(),
        source: "curated".to_owned(),
        verification: "内置精选，未联网复核".to_owned(),
        verified_at: None,
    })
}

fn resources(ids: &[&str]) -> Vec<Resource> {
    ids.iter().filter_map(|id| resource(id)).collect()
}

fn proc(stay: &Stay) -> String {
    normalize_proc(&stay.name)
}

fn in_set(stay: &Stay, set: &[&str]) -> bool {
    set.contains(&proc(stay).as_str())
}

fn is_editor(stay: &Stay) -> bool {
    in_set(
        stay,
        &[
            "code",
            "cursor",
            "idea",
            "idea64",
            "webstorm64",
            "pycharm64",
            "goland64",
            "rubymine64",
            "clion64",
            "rider",
            "datagrip64",
            "studio64",
            "devenv",
            "subl",
            "sublime_text",
            "windsurf",
            "trae",
        ],
    )
}
fn is_terminal(stay: &Stay) -> bool {
    in_set(
        stay,
        &[
            "windowsterminal",
            "cmd",
            "powershell",
            "pwsh",
            "conhost",
            "wezterm",
            "alacritty",
            "hyper",
            "warp",
            "mintty",
            "putty",
        ],
    )
}
fn is_browser(stay: &Stay) -> bool {
    in_set(
        stay,
        &[
            "chrome",
            "msedge",
            "firefox",
            "brave",
            "opera",
            "vivaldi",
            "arc",
            "360se",
            "360chrome",
            "qqbrowser",
        ],
    )
}
fn is_document(stay: &Stay) -> bool {
    in_set(
        stay,
        &[
            "winword", "excel", "powerpnt", "onenote", "typora", "obsidian", "notion", "wps", "et",
            "wpp", "acrord32", "acrobat", "yuque",
        ],
    )
}
fn is_file_manager(stay: &Stay) -> bool {
    in_set(stay, &["explorer", "totalcmd64", "doublecmd", "q-dir"])
}
fn is_api_app(stay: &Stay) -> bool {
    in_set(stay, &["postman", "insomnia", "bruno", "hoppscotch"])
}

fn sum_ms(stays: &[Stay], predicate: impl Fn(&Stay) -> bool) -> i64 {
    stays
        .iter()
        .filter(|stay| predicate(stay))
        .map(|stay| (stay.end - stay.start).max(0))
        .sum()
}

fn cross_transitions(
    stays: &[Stay],
    left: impl Fn(&Stay) -> bool,
    right: impl Fn(&Stay) -> bool,
) -> usize {
    stays
        .windows(2)
        .filter(|pair| left(&pair[0]) && right(&pair[1]) || right(&pair[0]) && left(&pair[1]))
        .count()
}

fn involving_transitions(stays: &[Stay], predicate: impl Fn(&Stay) -> bool) -> usize {
    stays
        .windows(2)
        .filter(|pair| predicate(&pair[0]) || predicate(&pair[1]))
        .count()
}

fn matching_title_count(apps: &[AppStat], pattern: &Regex) -> u32 {
    apps.iter()
        .flat_map(|app| &app.titles)
        .filter(|(title, _)| pattern.is_match(title))
        .map(|(_, occurrences)| (*occurrences).max(1))
        .sum()
}

fn rounded_minutes(ms: i64) -> i64 {
    (ms as f64 / MINUTE as f64).round() as i64
}

fn opportunity(
    id: &str,
    title: &str,
    evidence: String,
    summary: &str,
    search_query: &str,
    score: f64,
    resource_ids: &[&str],
) -> Recommendation {
    Recommendation {
        id: id.to_owned(),
        title: title.to_owned(),
        evidence,
        summary: summary.to_owned(),
        search_query: search_query.to_owned(),
        score,
        resources: resources(resource_ids),
    }
}

pub fn build_recommendations(stats: &DayStats) -> Vec<Recommendation> {
    let stays = &stats.stays;
    let apps = &stats.apps;
    let mut opportunities = Vec::new();

    let editor_terminal_switches = cross_transitions(stays, is_editor, is_terminal);
    if editor_terminal_switches >= 4 {
        let git_pattern = Regex::new(
            r"(?i)(?:^|\W)(?:git|github|gitlab|commit|merge|pull request)(?:\W|$)|提交|合并|分支",
        )
        .unwrap();
        let git_evidence = matching_title_count(apps, &git_pattern) > 0;
        let ids: &[&str] = if git_evidence {
            &["lazygit", "just", "vscodeTasks"]
        } else {
            &["just", "vscodeTasks"]
        };
        let active_ms = sum_ms(stays, |stay| is_editor(stay) || is_terminal(stay));
        opportunities.push(opportunity(
            "development-loop",
            "把开发命令收成固定入口",
            format!(
                "编辑器与终端之间切换 {editor_terminal_switches} 次，相关应用活跃 {} 分钟。",
                rounded_minutes(active_ms)
            ),
            "先固化重复的项目命令；如果切换主要来自 Git 操作，再使用可视化 Git 工具。",
            "open source command runner Git GUI Windows",
            editor_terminal_switches as f64,
            ids,
        ));
    }

    let browser_document_switches = cross_transitions(stays, is_browser, is_document);
    if browser_document_switches >= 4 {
        let active_ms = sum_ms(stays, |stay| is_browser(stay) || is_document(stay));
        opportunities.push(opportunity(
            "research-loop",
            "减少资料复制和整理步骤",
            format!("浏览器与文档或笔记应用之间切换 {browser_document_switches} 次，相关应用活跃 {} 分钟。", rounded_minutes(active_ms)),
            "将网页读取、来源引用和摘要整理做成可复用流程，保留人工核对来源这一步。",
            "open source research assistant MCP fetch Skill",
            browser_document_switches as f64 * 1.2,
            &["fetchMcp", "agentSkills", "searxng"],
        ));
    }

    let meeting_ms = sum_ms(stays, |stay| is_meeting(&stay.name, &stay.app, &stay.title));
    if meeting_ms >= 30 * MINUTE {
        opportunities.push(opportunity(
            "meeting-notes",
            "把会议转写和纪要拆开自动化",
            format!("会议相关活动共 {} 分钟。", rounded_minutes(meeting_ms)),
            "在参与者同意的前提下先本地转写，再用固定模板生成决议和行动项，并人工复核。",
            "open source local meeting transcription summary",
            rounded_minutes(meeting_ms) as f64 / 4.0,
            &["whispercpp", "meetingSkill"],
        ));
    }

    let file_ms = sum_ms(stays, is_file_manager);
    let file_switches = involving_transitions(stays, is_file_manager);
    let file_title_hits = matching_title_count(
        apps,
        &Regex::new(r"(?i)重命名|批量|整理文件|rename|batch rename|copy files|move files").unwrap(),
    );
    if file_ms >= 15 * MINUTE && (file_switches >= 4 || file_title_hits >= 2) {
        opportunities.push(opportunity(
            "file-workflow",
            "自动化明确的文件整理动作",
            format!(
                "文件管理相关窗口活跃 {} 分钟，与其他应用切换 {file_switches} 次。",
                rounded_minutes(file_ms)
            ),
            "批量重命名先用成熟工具；只有规则明确且反复出现的文件动作才值得写成脚本。",
            "open source Windows batch rename file automation",
            file_switches as f64 + rounded_minutes(file_ms) as f64 / 5.0,
            &["powerToys", "autohotkey"],
        ));
    }

    let api_title_hits = matching_title_count(
        apps,
        &Regex::new(r"(?i)\b(?:api|openapi|swagger|graphql|postman)\b|接口调试|接口文档").unwrap(),
    );
    let api_ms = sum_ms(stays, is_api_app);
    if api_title_hits >= 2 || api_ms >= 10 * MINUTE {
        let evidence = if api_title_hits >= 2 {
            format!("检测到 {api_title_hits} 次 API 或接口相关窗口标题。")
        } else {
            format!("API 调试工具活跃 {} 分钟。", rounded_minutes(api_ms))
        };
        opportunities.push(opportunity(
            "api-workflow",
            "复用 API 请求和调试环境",
            evidence,
            "把常用请求、环境和示例响应整理成集合并纳入密钥检查，减少重复填写。",
            "open source API client request collection",
            api_title_hits as f64 * 2.0 + rounded_minutes(api_ms) as f64 / 5.0,
            &["bruno", "hoppscotch"],
        ));
    }

    opportunities.sort_by(|a, b| b.score.total_cmp(&a.score));
    opportunities.truncate(3);
    opportunities
}

pub fn recommendations_markdown(
    recommendations: &[Recommendation],
    search: &SearchSummary,
) -> String {
    if recommendations.is_empty() {
        return String::new();
    }
    let mut lines = vec!["## 六、可工具化的操作".to_owned(), String::new()];
    if search.enabled {
        lines.push(format!("> 本地规则结合联网检索：执行 {} 次抽象关键词查询，新增 {} 个待审核候选，验证 {} 个来源链接。搜索服务不会收到窗口标题、网站域名或行为统计。", search.queries, search.added, search.verified));
    } else {
        lines.push("> 以下来自本地行为规则和内置精选资源，未进行联网搜索。".to_owned());
    }
    if let Some(error) = search.errors.first() {
        lines.push(format!("> 联网检索提示：{error}。已保留本地精选结果。"));
    }
    lines.push(String::new());
    for opportunity in recommendations {
        lines.push(format!("### {}", opportunity.title));
        lines.push(format!("- 操作依据：{}", opportunity.evidence));
        lines.push(format!("- 优化方向：{}", opportunity.summary));
        for resource in &opportunity.resources {
            let checked = resource
                .verified_at
                .and_then(local_datetime)
                .map(|date| format!("，检查于 {}", date.format("%Y-%m-%d %H:%M:%S")))
                .unwrap_or_default();
            lines.push(format!(
                "- [{}]({})（{}）：{} 预期收益：{} 配置成本：{} 权限与隐私：{} 来源状态：{}{}。",
                resource.name,
                resource.url,
                resource.kind,
                resource.description,
                resource.benefit,
                resource.setup_cost,
                resource.permissions,
                resource.verification,
                checked
            ));
        }
        lines.push(String::new());
    }
    lines.push("以上资源只供人工评估，TraceBit 不会自动下载、安装或启用。".to_owned());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn stay(name: &str, app: &str, title: &str, start: i64, minutes: i64) -> Stay {
        Stay {
            start,
            end: start + minutes * MINUTE,
            app: app.to_owned(),
            name: name.to_owned(),
            title: title.to_owned(),
            titles: BTreeSet::from([title.to_owned()]),
            pid: 1,
        }
    }

    fn stats(stays: Vec<Stay>, apps: Vec<AppStat>) -> DayStats {
        DayStats {
            date_key: "2026-01-01".to_owned(),
            events: 1,
            stays,
            sessions: vec![],
            apps,
            hourly: vec![0; 24],
            switches: 0,
            total_active_ms: 1,
            first_active: None,
            last_active: None,
            screenshots: vec![],
            sites: vec![],
            lock_events: 0,
            long_idles: 0,
            longest_idle_ms: 0,
            longest_block: None,
            snapshot_bytes: 0,
        }
    }

    #[test]
    fn recommendations_have_numeric_evidence_and_safe_resources() {
        let mut stays = Vec::new();
        let mut ts = 0;
        for _ in 0..4 {
            stays.push(stay("code", "VS Code", "git commit - project", ts, 3));
            ts += 3 * MINUTE;
            stays.push(stay("powershell", "PowerShell", "git status", ts, 3));
            ts += 3 * MINUTE;
        }
        for _ in 0..3 {
            stays.push(stay("chrome", "Chrome", "research", ts, 3));
            ts += 3 * MINUTE;
            stays.push(stay("obsidian", "Obsidian", "notes", ts, 3));
            ts += 3 * MINUTE;
        }
        stays.push(stay("zoom", "Zoom", "Zoom Meeting", ts, 40));
        let apps = vec![AppStat {
            name: "code".to_owned(),
            app: "VS Code".to_owned(),
            category: "开发".to_owned(),
            ms: 1,
            switches: 0,
            titles: BTreeMap::from([("secret@example.com git commit".to_owned(), 3)]),
        }];
        let recommendations = build_recommendations(&stats(stays, apps));
        assert!(
            recommendations
                .iter()
                .any(|item| item.id == "development-loop")
        );
        assert!(
            recommendations
                .iter()
                .any(|item| item.id == "research-loop")
        );
        assert!(
            recommendations
                .iter()
                .any(|item| item.id == "meeting-notes")
        );
        for item in &recommendations {
            assert!(item.evidence.chars().any(|ch| ch.is_ascii_digit()));
            assert!(
                item.resources
                    .iter()
                    .all(|resource| resource.url.starts_with("https://")
                        && !resource.permissions.is_empty())
            );
        }
        let markdown = recommendations_markdown(&recommendations, &SearchSummary::default());
        assert!(markdown.contains("## 六、可工具化的操作"));
        assert!(!markdown.contains("secret@example.com"));
    }
}
