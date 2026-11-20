pub const CATEGORIES: [&str; 11] = [
    "开发",
    "浏览器",
    "即时通讯",
    "办公文档",
    "会议",
    "设计",
    "资讯社交",
    "娱乐",
    "工具",
    "系统",
    "其他",
];

/// Display name for a canonical category id. Stored data and all matching logic
/// stay Chinese; only the display layer goes through here. Unknown ids (for
/// example a category written by a newer version) pass through unchanged.
pub fn label(lang: crate::i18n::Lang, category: &str) -> &str {
    use crate::tr;
    match category {
        "开发" => tr!(lang, "开发", "Development", "開発", "개발"),
        "浏览器" => tr!(lang, "浏览器", "Browser", "ブラウザ", "브라우저"),
        "即时通讯" => tr!(lang, "即时通讯", "Messaging", "メッセージ", "메신저"),
        "办公文档" => tr!(lang, "办公文档", "Documents", "ドキュメント", "문서"),
        "会议" => tr!(lang, "会议", "Meetings", "会議", "회의"),
        "设计" => tr!(lang, "设计", "Design", "デザイン", "디자인"),
        "资讯社交" => tr!(lang, "资讯社交", "News & social", "ソーシャル", "소셜"),
        "娱乐" => tr!(lang, "娱乐", "Entertainment", "エンタメ", "엔터테인먼트"),
        "工具" => tr!(lang, "工具", "Utilities", "ユーティリティ", "유틸리티"),
        "系统" => tr!(lang, "系统", "System", "システム", "시스템"),
        "其他" => tr!(lang, "其他", "Other", "その他", "기타"),
        other => other,
    }
}

/// Categories that represent producing work rather than consuming or coordinating.
pub fn is_productive(category: &str) -> bool {
    matches!(category, "开发" | "办公文档" | "设计")
}

pub fn is_communication(category: &str) -> bool {
    category == "即时通讯"
}

pub fn is_distraction(category: &str) -> bool {
    matches!(category, "娱乐" | "资讯社交")
}

pub fn is_browser_process(proc_name: &str) -> bool {
    proc_category(&normalize_proc(proc_name)) == Some("浏览器")
}

/// Maps a browser host to a work category so browser time is not one opaque bucket.
/// Matching is on the registrable suffix, so `docs.github.com` matches `github.com`.
pub fn site_category(host: &str) -> Option<&'static str> {
    let host = host.trim().trim_end_matches('.').to_lowercase();
    if host.is_empty() {
        return None;
    }
    if host == "localhost"
        || host.starts_with("127.")
        || host.starts_with("192.168.")
        || host.starts_with("10.")
    {
        return Some("开发");
    }
    let groups: [(&[&str], &str); 8] = [
        (
            &[
                "chatgpt.com",
                "openai.com",
                "claude.ai",
                "gemini.google.com",
                "kimi.com",
                "kimi.moonshot.cn",
                "chat.deepseek.com",
                "doubao.com",
                "tongyi.aliyun.com",
                "yiyan.baidu.com",
                "perplexity.ai",
                "poe.com",
            ],
            "工具",
        ),
        (
            &[
                "github.com",
                "gitlab.com",
                "gitee.com",
                "bitbucket.org",
                "stackoverflow.com",
                "stackexchange.com",
                "docs.rs",
                "crates.io",
                "npmjs.com",
                "pypi.org",
                "developer.mozilla.org",
                "rust-lang.org",
                "python.org",
                "go.dev",
                "learn.microsoft.com",
                "vercel.com",
                "netlify.app",
                "cloudflare.com",
                "aws.amazon.com",
                "console.cloud.google.com",
                "aliyun.com",
                "huggingface.co",
                "csdn.net",
                "juejin.cn",
                "cnblogs.com",
                "segmentfault.com",
                "v2ex.com",
            ],
            "开发",
        ),
        (
            &[
                "docs.google.com",
                "notion.so",
                "notion.site",
                "yuque.com",
                "feishu.cn",
                "larksuite.com",
                "office.com",
                "sharepoint.com",
                "kdocs.cn",
                "docs.qq.com",
                "shimo.im",
                "confluence",
                "atlassian.net",
                "wps.cn",
                "overleaf.com",
            ],
            "办公文档",
        ),
        (
            &[
                "meet.google.com",
                "zoom.us",
                "meeting.tencent.com",
                "teams.microsoft.com",
                "webex.com",
            ],
            "会议",
        ),
        (
            &[
                "mail.google.com",
                "outlook.live.com",
                "outlook.office.com",
                "mail.qq.com",
                "mail.163.com",
                "web.whatsapp.com",
                "web.telegram.org",
                "slack.com",
                "discord.com",
                "wx.qq.com",
            ],
            "即时通讯",
        ),
        (
            &[
                "figma.com",
                "canva.com",
                "mastergo.com",
                "js.design",
                "dribbble.com",
                "behance.net",
            ],
            "设计",
        ),
        (
            &[
                "bilibili.com",
                "youtube.com",
                "youku.com",
                "iqiyi.com",
                "v.qq.com",
                "netflix.com",
                "twitch.tv",
                "douyin.com",
                "huya.com",
                "douyu.com",
                "music.163.com",
                "spotify.com",
                "mgtv.com",
                "taobao.com",
                "jd.com",
                "tmall.com",
                "pinduoduo.com",
                "steampowered.com",
            ],
            "娱乐",
        ),
        (
            &[
                "weibo.com",
                "zhihu.com",
                "x.com",
                "twitter.com",
                "reddit.com",
                "douban.com",
                "xiaohongshu.com",
                "facebook.com",
                "instagram.com",
                "news.ycombinator.com",
                "36kr.com",
                "toutiao.com",
                "sina.com.cn",
                "163.com",
                "qq.com",
            ],
            "资讯社交",
        ),
    ];
    groups.into_iter().find_map(|(suffixes, category)| {
        suffixes
            .iter()
            .any(|suffix| {
                if suffix.contains('.') {
                    host == *suffix || host.ends_with(&format!(".{suffix}"))
                } else {
                    host.contains(suffix)
                }
            })
            .then_some(category)
    })
}

pub fn normalize_proc(name: &str) -> String {
    name.trim()
        .trim_end_matches(".exe")
        .trim_end_matches(".EXE")
        .to_lowercase()
}

fn proc_category(proc: &str) -> Option<&'static str> {
    Some(match proc {
        "code" | "cursor" | "idea" | "idea64" | "webstorm64" | "pycharm64" | "goland64"
        | "rubymine64" | "clion64" | "rider" | "datagrip64" | "studio64" | "devenv" | "subl"
        | "sublime_text" | "hx" | "hm" | "windowsterminal" | "cmd" | "powershell" | "pwsh"
        | "conhost" | "wezterm" | "alacritty" | "hyper" | "warp" | "mintty" | "putty"
        | "windsurf" | "trae" | "rssfs" => "开发",
        "chrome" | "msedge" | "firefox" | "brave" | "opera" | "vivaldi" | "arc" | "360se"
        | "360chrome" | "qqbrowser" | "搜狗浏览器" | "liebao" | "maxthon" => "浏览器",
        "wechat" | "weixin" | "wechatappex" | "wechatstore" | "qq" | "tim" | "dingtalk"
        | "feishu" | "lark" | "telegram" | "slack" | "discord" | "wexin" | "wxwork" => "即时通讯",
        "winword" | "excel" | "powerpnt" | "onenote" | "typora" | "obsidian" | "notion" | "wps"
        | "et" | "wpp" | "acrord32" | "acrobat" | "yuque" => "办公文档",
        "teams" | "msteams" | "zoom" | "wemeetapp" | "wemeet" | "dyta" | "teamshare_x"
        | "voovmeeting" | "feishu meetings" | "webex" => "会议",
        "photoshop" | "illustrator" | "figma" | "xd" | "blender" | "afterfx" | "premiere"
        | "premiere pro" | "sketch" | "canva" | "axe" | "mastergo" | "jsdesign" | "即时设计" => {
            "设计"
        }
        "steam" | "spotify" | "cloudmusic" | "qqmusic" | "kugou" | "kwmusic" | "bilibili"
        | "gamebar" | "mpc-hc64" | "potplayer" | "potplayermini64" => "娱乐",
        "totalcmd64" | "everything" | "listary" | "snipaste" | "picpick" | "bandizip" | "7zfm"
        | "winrar" | "notepad++" | "notepad++64" | "calc" | "calculatorapp" | "mspaint"
        | "gimp" | "taskmgr" => "工具",
        "systemsettings"
        | "explorer"
        | "applicationframehost"
        | "searchhost"
        | "textinputhost"
        | "shellexperiencehost"
        | "startmenuexperiencehost"
        | "dwm"
        | "logonui"
        | "regedit"
        | "sihost"
        | "ctfmon"
        | "backgroundtaskhost" => "系统",
        _ => return None,
    })
}

const TITLE_MEETING_PATTERNS: [&str; 8] = [
    "会议",
    "腾讯会议",
    "zoom meeting",
    "meeting",
    "视频会议",
    "线上会议",
    "webex",
    "google meet",
];
const TITLE_DEV_PATTERNS: [&str; 8] = [
    "vscode",
    "visual studio code",
    "— vscode",
    ".git",
    "git ",
    "terminal",
    "powershell",
    "cmd.exe",
];
const TITLE_DOC_PATTERNS: [&str; 14] = [
    ".docx",
    ".xlsx",
    ".pptx",
    ".doc",
    ".xls",
    ".ppt",
    "word",
    "excel",
    "powerpoint",
    "typora",
    "obsidian",
    "notion",
    "语雀",
    "飞书文档",
];
const TITLE_ENT_PATTERNS: [&str; 15] = [
    "bilibili",
    "哔哩哔哩",
    "youtube",
    "哔",
    "网易云",
    "spotify",
    "steam",
    "斗鱼",
    "虎牙",
    "twitch",
    "爱奇艺",
    "腾讯视频",
    "优酷",
    "芒果tv",
    "netflix",
];

fn contains_any(haystack: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| haystack.contains(pattern))
}

fn categorize_by_label(label: &str) -> Option<&'static str> {
    let label = label.to_lowercase();
    let groups: [(&[&str], &str); 8] = [
        (
            &[
                "code",
                "visual studio",
                "webstorm",
                "pycharm",
                "goland",
                "idea",
                "terminal",
                "powershell",
            ],
            "开发",
        ),
        (
            &["chrome", "edge", "firefox", "browser", "浏览器"],
            "浏览器",
        ),
        (
            &[
                "微信", "wechat", "飞书", "feishu", "lark", "qq", "dingtalk", "钉钉", "telegram",
                "slack", "discord",
            ],
            "即时通讯",
        ),
        (
            &[
                "word",
                "excel",
                "powerpoint",
                "typora",
                "obsidian",
                "notion",
                "wps",
                "语雀",
            ],
            "办公文档",
        ),
        (&["zoom", "会议", "meeting", "teams"], "会议"),
        (
            &[
                "photoshop",
                "illustrator",
                "figma",
                "blender",
                "premiere",
                "after effects",
                "设计",
            ],
            "设计",
        ),
        (
            &[
                "spotify",
                "网易云音乐",
                "qq音乐",
                "bilibil",
                "哔哩哔哩",
                "steam",
                "视频",
            ],
            "娱乐",
        ),
        (
            &["explorer", "设置", "文件资源管理器", "任务管理器"],
            "系统",
        ),
    ];
    groups
        .into_iter()
        .find_map(|(patterns, category)| contains_any(&label, patterns).then_some(category))
}

pub fn categorize(proc_name: &str, app_label: &str, title: &str) -> &'static str {
    let proc = normalize_proc(proc_name);
    if let Some(category) = proc_category(&proc) {
        return category;
    }
    // Unreal Engine games ship as "<Name>-Win64-Shipping.exe".
    if proc.ends_with("-shipping") {
        return "娱乐";
    }
    let haystack = format!("{app_label} {title}").to_lowercase();
    if contains_any(&haystack, &TITLE_MEETING_PATTERNS)
        && matches!(
            categorize_by_label(app_label),
            Some("浏览器" | "即时通讯") | None
        )
    {
        return "会议";
    }
    if let Some(category) = categorize_by_label(app_label) {
        return category;
    }
    if contains_any(&haystack, &TITLE_DEV_PATTERNS) {
        return "开发";
    }
    if contains_any(&haystack, &TITLE_DOC_PATTERNS) {
        return "办公文档";
    }
    if contains_any(&haystack, &TITLE_ENT_PATTERNS) {
        return "娱乐";
    }
    "其他"
}

pub fn is_meeting(proc_name: &str, app_label: &str, title: &str) -> bool {
    let proc = normalize_proc(proc_name);
    if matches!(
        proc.as_str(),
        "zoom"
            | "wemeetapp"
            | "wemeet"
            | "dyta"
            | "teamshare_x"
            | "voovmeeting"
            | "webex"
            | "teams"
            | "msteams"
    ) {
        return true;
    }
    contains_any(
        &format!("{app_label} {title}").to_lowercase(),
        &TITLE_MEETING_PATTERNS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Lang;

    #[test]
    fn labels_localize_every_canonical_id() {
        assert_eq!(label(Lang::En, "开发"), "Development");
        assert_eq!(label(Lang::Ja, "即时通讯"), "メッセージ");
        assert_eq!(label(Lang::Ko, "娱乐"), "엔터테인먼트");
        for category in CATEGORIES {
            assert_eq!(label(Lang::Zh, category), category);
            assert!(!label(Lang::En, category).is_empty());
            assert!(
                !label(Lang::En, category)
                    .chars()
                    .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch)),
                "{category} has no English label"
            );
        }
        assert_eq!(label(Lang::En, "custom"), "custom");
    }

    #[test]
    fn process_and_fallback_categories_work() {
        assert_eq!(categorize("Code.exe", "", ""), "开发");
        assert_eq!(categorize("msedge", "", "Zoom Meeting"), "浏览器");
        assert_eq!(
            categorize("custom", "Custom Browser", "Zoom Meeting"),
            "会议"
        );
        assert_eq!(categorize("unknown", "", "notes.docx"), "办公文档");
        assert!(is_meeting("chrome", "Chrome", "Weekly Meeting"));
    }

    #[test]
    fn sites_map_by_suffix_not_substring() {
        assert_eq!(site_category("docs.github.com"), Some("开发"));
        assert_eq!(site_category("www.bilibili.com"), Some("娱乐"));
        assert_eq!(site_category("mail.qq.com"), Some("即时通讯"));
        assert_eq!(site_category("notgithub.com"), None);
        assert_eq!(site_category("localhost"), Some("开发"));
        assert!(is_browser_process("msedge.exe"));
        assert_eq!(categorize("X6Game-Win64-Shipping", "", ""), "娱乐");
    }
}
