pub const CATEGORIES: [&str; 10] = [
    "开发",
    "浏览器",
    "即时通讯",
    "办公文档",
    "会议",
    "设计",
    "娱乐",
    "工具",
    "系统",
    "其他",
];

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
}
