use crate::{
    categories::{is_meeting, normalize_proc},
    store::{AppStat, DayStats, Stay},
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
    #[serde(default)]
    pub replaces: Vec<String>,
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
        "dbeaver" => (
            "DBeaver Community",
            "开源替代",
            "https://github.com/dbeaver/dbeaver",
            "用同一个窗口连接 PostgreSQL、MySQL、SQL Server 等常见数据库。",
            "把查看表结构和执行 SQL 留在同一个免费客户端里，不必为每个库装一套工具。",
            "低到中，可先迁移一个最常用的连接。",
            "连接配置里可能有数据库密码，取值前先确认保存方式和访问权限。",
        ),
        "heidisql" => (
            "HeidiSQL",
            "开源替代",
            "https://github.com/HeidiSQL/HeidiSQL",
            "轻量的 Windows 数据库客户端，覆盖常见的表、查询和数据导出操作。",
            "适合以 MySQL 或 MariaDB 为主的日常查询，启动和操作都更轻。",
            "低，配置一个连接即可试用。",
            "会读写数据库；生产库上先确认账号权限，避免误执行写操作。",
        ),
        "gimp" => (
            "GIMP",
            "开源替代",
            "https://github.com/GNOME/gimp",
            "图像编辑和合成，图层、选区、滤镜等常用操作齐全。",
            "保留相近的图层编辑方式，不依赖订阅和账号。",
            "中，界面和快捷键与商业软件不同，需要一段适应期。",
            "插件来源需自行核对；处理大图时注意内存占用。",
        ),
        "krita" => (
            "Krita",
            "开源替代",
            "https://github.com/KDE/krita",
            "面向绘图和插画的绘画工具，笔刷和画布操作完整。",
            "适合以手绘、上色为主的日常工作，本地文件格式开放。",
            "中，笔刷和快捷键需要重新配置一次。",
            "笔刷和素材包来源需自行核对。",
        ),
        "inkscape" => (
            "Inkscape",
            "开源替代",
            "https://github.com/inkscape/inkscape",
            "矢量图形编辑，路径、节点和 SVG 输出都是原生能力。",
            "用 SVG 作为中间格式，和现有设计稿互通，不锁定账号。",
            "中，路径工具的操作习惯需要适应。",
            "导入外部素材前先确认授权。",
        ),
        "darktable" => (
            "darktable",
            "开源替代",
            "https://github.com/darktable-org/darktable",
            "照片管理和非破坏性调色流程。",
            "RAW 原片留在本地，调色参数独立保存，可随时回退。",
            "中到高，调色模块需要重新熟悉。",
            "目录数据库保存在本机，迁移前先备份。",
        ),
        "kdenlive" => (
            "Kdenlive",
            "开源替代",
            "https://github.com/KDE/kdenlive",
            "多轨视频剪辑，支持常用编码和代理剪辑。",
            "时间线式剪辑的操作方式接近现有习惯，项目文件可版本管理。",
            "中，编码和代理设置需要按素材调整一次。",
            "导出前确认编码参数；素材版权和授权需自行确认。",
        ),
        "shotcut" => (
            "Shotcut",
            "开源替代",
            "https://github.com/mltframework/shotcut",
            "轻量视频剪辑，直接支持大量格式，无需先转码。",
            "打开即剪，减少素材转码这一步。",
            "低到中，基础剪辑可上手即用。",
            "复杂工程请定期保存副本。",
        ),
        "penpot" => (
            "Penpot",
            "开源替代",
            "https://github.com/penpot/penpot",
            "开源的设计与原型协作工具，可自托管。",
            "设计文件和协作数据可以放在自己的服务器上，不绑定单一账号体系。",
            "中，自托管需要部署；也可先用官方托管验证。",
            "团队协作会接触设计稿，自托管前确认备份和访问控制。",
        ),
        "drawio" => (
            "draw.io Desktop",
            "开源替代",
            "https://github.com/jgraph/drawio-desktop",
            "本地运行的流程图、架构图和线框工具。",
            "图形文件保存在本地，导出格式通用，适合反复修改的图。",
            "低，现有图形习惯基本可以沿用。",
            "导入第三方图形库前确认来源。",
        ),
        "excalidraw" => (
            "Excalidraw",
            "开源替代",
            "https://github.com/excalidraw/excalidraw",
            "手绘风格的草图和白板，可本地或自托管使用。",
            "适合快速画流程和讨论稿，文件格式是开放的 JSON。",
            "低，浏览器或桌面都能用。",
            "分享链接前确认画布里没有敏感信息。",
        ),
        "rustdesk" => (
            "RustDesk",
            "开源替代",
            "https://github.com/rustdesk/rustdesk",
            "远程桌面控制，可自建中继服务。",
            "连接方式与现有远程工具一致，连接数据可以自己掌控。",
            "中，自建中继需要一台服务器；直连则接近即装即用。",
            "远程控制权限很大，务必设置强密码并限制可连接的设备。",
        ),
        "winmerge" => (
            "WinMerge",
            "开源替代",
            "https://github.com/WinMerge/winmerge",
            "文件和目录对比、合并，支持三方对比。",
            "对比操作和现有的双栏习惯一致，可直接接入版本库目录。",
            "低，选中两个目录即可开始。",
            "合并写回前先确认备份，避免覆盖他人改动。",
        ),
        "nanazip" => (
            "NanaZip",
            "开源替代",
            "https://github.com/M2Team/NanaZip",
            "Windows 右键菜单集成的现代压缩工具，兼容 7-Zip 格式。",
            "解压和压缩的操作位置不变，少一次打开主界面的步骤。",
            "低，安装后沿用右键菜单。",
            "解压来源不明的压缩包前先扫描。",
        ),
        "tabby" => (
            "Tabby",
            "开源替代",
            "https://github.com/Eugeny/tabby",
            "集成本地终端、SSH 会话和串口的终端工具。",
            "把分散在多个工具里的会话收进一个窗口，配置可导出。",
            "中，已有会话和密钥需要迁移一次。",
            "会话配置可能包含主机和密钥路径，注意文件权限。",
        ),
        "podmanDesktop" => (
            "Podman Desktop",
            "开源替代",
            "https://github.com/containers/podman-desktop",
            "无守护进程的容器管理界面，兼容常见容器工作流。",
            "镜像、容器和 Pod 的管理方式接近现有习惯，不依赖商业授权。",
            "中，首次切换需要迁移本地镜像和卷。",
            "容器会挂载本地目录，运行前确认挂载范围和镜像来源。",
        ),
        "logseq" => (
            "Logseq",
            "开源替代",
            "https://github.com/logseq/logseq",
            "本地优先的大纲笔记，页面以 Markdown 文件保存在本地。",
            "沿用现有的双链和日志写法，笔记不被单一服务锁定。",
            "中到高，已有页面需要导入并检查层级。",
            "本地目录可能含敏感笔记，同步前确认加密和备份方式。",
        ),
        "zettlr" => (
            "Zettlr",
            "开源替代",
            "https://github.com/Zettlr/Zettlr",
            "面向 Markdown 文件的写作与导出工具。",
            "文件就是普通的 .md，写作和导出流程可以沿用。",
            "低，打开现有文件夹即可。",
            "导出模板和引用格式需要按需配置。",
        ),
        "gitextensions" => (
            "Git Extensions",
            "开源替代",
            "https://github.com/gitextensions/gitextensions",
            "Windows 上的图形化 Git 客户端，支持提交、分支和差异查看。",
            "保留图形化操作习惯，仓库数据仍然只是普通 .git 目录。",
            "低到中，克隆现有仓库即可开始。",
            "改写历史的操作仍需谨慎，先确认已推送的分支。",
        ),
        "stirlingPdf" => (
            "Stirling PDF",
            "开源替代",
            "https://github.com/Stirling-Tools/Stirling-PDF",
            "可自托管的 PDF 合并、拆分、压缩和转换工具。",
            "把常用的 PDF 处理集中到一个可自托管的页面，不逐次上传到第三方。",
            "高，需要部署服务并维护。",
            "文档可能含敏感内容，部署时限制访问并确认不留日志。",
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

/// Process -> workflow role. Shared by the profile and the opportunity rules so
/// "how this person works" means the same thing everywhere.
pub fn tool_role(proc: &str) -> Option<&'static str> {
    const GROUPS: &[(&[&str], &str)] = &[
        (
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
                "hx",
                "hm",
                "windsurf",
                "trae",
                "rssfs",
            ],
            "编辑器",
        ),
        (
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
                "tabby",
            ],
            "终端",
        ),
        (
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
                "搜狗浏览器",
                "liebao",
                "maxthon",
            ],
            "浏览器",
        ),
        (
            &[
                "winword", "excel", "powerpnt", "onenote", "typora", "obsidian", "notion", "wps",
                "et", "wpp", "acrord32", "acrobat", "yuque", "logseq", "zettlr",
            ],
            "文档与笔记",
        ),
        (
            &[
                "wechat",
                "weixin",
                "wechatappex",
                "wechatstore",
                "qq",
                "tim",
                "dingtalk",
                "feishu",
                "lark",
                "telegram",
                "slack",
                "discord",
                "wxwork",
            ],
            "通讯",
        ),
        (
            &[
                "teams",
                "msteams",
                "zoom",
                "wemeetapp",
                "wemeet",
                "dyta",
                "teamshare_x",
                "voovmeeting",
                "feishu meetings",
                "webex",
            ],
            "会议",
        ),
        (
            &[
                "photoshop",
                "illustrator",
                "figma",
                "xd",
                "sketch",
                "canva",
                "axe",
                "mastergo",
                "jsdesign",
                "即时设计",
                "krita",
                "gimp",
                "inkscape",
                "lightroom",
                "darktable",
                "3dsmax",
                "maya",
            ],
            "设计",
        ),
        (
            &[
                "premiere",
                "premiere pro",
                "afterfx",
                "obs64",
                "audition",
                "kdenlive",
                "shotcut",
                "resolve",
                "jianying",
                "capcut",
            ],
            "音视频",
        ),
        (
            &[
                "postman",
                "insomnia",
                "bruno",
                "hoppscotch",
                "apifox",
                "paw",
            ],
            "API 调试",
        ),
        (
            &[
                "navicat",
                "navicat premium",
                "dbeaver",
                "heidisql",
                "ssms",
                "mysqlworkbench",
                "toad",
                "plsqldeveloper",
                "sqldeveloper",
                "redisinsight",
            ],
            "数据库",
        ),
        (
            &[
                "explorer",
                "totalcmd64",
                "doublecmd",
                "q-dir",
                "everything",
                "listary",
            ],
            "文件管理",
        ),
        (
            &[
                "teamviewer",
                "anydesk",
                "todesk",
                "sunloginclient",
                "rustdesk",
                "mstsc",
            ],
            "远程控制",
        ),
        (
            &[
                "vmware",
                "virtualbox",
                "docker desktop",
                "dockerd",
                "podman desktop",
                "podman",
                "utm",
                "parallels",
            ],
            "虚拟化",
        ),
        (
            &["winrar", "7zfm", "bandizip", "360zip", "haozip", "nanazip"],
            "压缩",
        ),
        (&["snipaste", "sharex", "flameshot", "picpick"], "截图"),
        (
            &["keepass", "keepassxc", "1password", "bitwarden", "lastpass"],
            "密码管理",
        ),
        (&["outlook", "thunderbird", "foxmail"], "邮件"),
    ];
    GROUPS
        .iter()
        .find_map(|(procs, role)| procs.contains(&proc).then_some(*role))
}

fn has_role(stay: &Stay, role: &str) -> bool {
    tool_role(&proc(stay)).is_some_and(|value| value == role)
}

fn is_editor(stay: &Stay) -> bool {
    has_role(stay, "编辑器")
}
fn is_terminal(stay: &Stay) -> bool {
    has_role(stay, "终端")
}
fn is_browser(stay: &Stay) -> bool {
    has_role(stay, "浏览器")
}
fn is_document(stay: &Stay) -> bool {
    has_role(stay, "文档与笔记")
}
fn is_file_manager(stay: &Stay) -> bool {
    has_role(stay, "文件管理")
}
fn is_api_app(stay: &Stay) -> bool {
    has_role(stay, "API 调试")
}

/// Existing tool -> open-source tool that keeps the same interaction.
struct AlternativeRule {
    procs: &'static [&'static str],
    fallback: &'static str,
    role: &'static str,
    resources: &'static [&'static str],
}

const ALTERNATIVES: &[AlternativeRule] = &[
    AlternativeRule {
        procs: &["postman", "insomnia", "apifox"],
        fallback: "API 调试工具",
        role: "API 调试",
        resources: &["bruno", "hoppscotch"],
    },
    AlternativeRule {
        procs: &[
            "navicat",
            "navicat premium",
            "plsqldeveloper",
            "sqldeveloper",
            "ssms",
        ],
        fallback: "数据库客户端",
        role: "数据库",
        resources: &["dbeaver", "heidisql"],
    },
    AlternativeRule {
        procs: &["photoshop"],
        fallback: "Photoshop",
        role: "设计",
        resources: &["gimp", "krita"],
    },
    AlternativeRule {
        procs: &["illustrator"],
        fallback: "Illustrator",
        role: "设计",
        resources: &["inkscape"],
    },
    AlternativeRule {
        procs: &["lightroom"],
        fallback: "Lightroom",
        role: "设计",
        resources: &["darktable"],
    },
    AlternativeRule {
        procs: &["premiere", "premiere pro"],
        fallback: "Premiere",
        role: "音视频",
        resources: &["kdenlive", "shotcut"],
    },
    AlternativeRule {
        procs: &["figma"],
        fallback: "Figma",
        role: "设计",
        resources: &["penpot"],
    },
    AlternativeRule {
        procs: &["xmind", "mindmaster"],
        fallback: "思维导图工具",
        role: "设计",
        resources: &["drawio", "excalidraw"],
    },
    AlternativeRule {
        procs: &["visio"],
        fallback: "Visio",
        role: "设计",
        resources: &["drawio"],
    },
    AlternativeRule {
        procs: &["teamviewer", "anydesk", "todesk", "sunloginclient"],
        fallback: "远程控制工具",
        role: "远程控制",
        resources: &["rustdesk"],
    },
    AlternativeRule {
        procs: &["bcompare", "beyondcompare"],
        fallback: "Beyond Compare",
        role: "文件管理",
        resources: &["winmerge"],
    },
    AlternativeRule {
        procs: &["winrar", "bandizip", "360zip", "haozip"],
        fallback: "压缩工具",
        role: "压缩",
        resources: &["nanazip"],
    },
    AlternativeRule {
        procs: &["xshell", "securecrt", "mobaxterm"],
        fallback: "SSH 客户端",
        role: "终端",
        resources: &["tabby"],
    },
    AlternativeRule {
        procs: &["docker desktop"],
        fallback: "Docker Desktop",
        role: "虚拟化",
        resources: &["podmanDesktop"],
    },
    AlternativeRule {
        procs: &["typora"],
        fallback: "Typora",
        role: "文档与笔记",
        resources: &["zettlr"],
    },
    AlternativeRule {
        procs: &["sourcetree"],
        fallback: "SourceTree",
        role: "编辑器",
        resources: &["gitextensions", "lazygit"],
    },
    AlternativeRule {
        procs: &["notion", "yuque"],
        fallback: "在线笔记",
        role: "文档与笔记",
        resources: &["logseq"],
    },
    AlternativeRule {
        procs: &["acrobat"],
        fallback: "Acrobat",
        role: "文档与笔记",
        resources: &["stirlingPdf"],
    },
];

fn build_alternatives(stays: &[Stay]) -> Vec<Recommendation> {
    let mut matched = ALTERNATIVES
        .iter()
        .filter_map(|rule| {
            let used = stays
                .iter()
                .filter(|stay| rule.procs.contains(&proc(stay).as_str()))
                .collect::<Vec<_>>();
            let active_ms = used.iter().map(|stay| stay.duration()).sum::<i64>();
            (active_ms >= 20 * MINUTE).then_some((rule, used, active_ms))
        })
        .collect::<Vec<_>>();
    // Two heaviest tools only: a report should not become a migration checklist.
    matched.sort_by_key(|(_, _, active_ms)| std::cmp::Reverse(*active_ms));
    matched.truncate(2);
    matched
        .into_iter()
        .map(|(rule, used, active_ms)| {
            let label = used
                .iter()
                .find(|stay| !stay.app.trim().is_empty())
                .map(|stay| stay.app.trim().to_owned())
                .unwrap_or_else(|| rule.fallback.to_owned());
            opportunity(
                &format!("alternative-{}", rule.procs[0]),
                &format!("用同类开源工具承接{label}"),
                format!(
                    "{label}（{}）今天活跃 {} 分钟，进入 {} 次。",
                    rule.role,
                    rounded_minutes(active_ms),
                    used.len()
                ),
                "同样的事情换一个本地可运行、无订阅锁定的入口，操作方式基本不变；先在一件真实任务上并行验证。",
                &format!("open source alternative to {label}"),
                rounded_minutes(active_ms) as f64 / 5.0 + used.len() as f64,
                &[label.as_str()],
                rule.resources,
            )
        })
        .collect()
}

/// Whether a paid API client is used long enough to be replaced outright, so
/// the generic API-collection rule does not suggest the same tools twice.
fn covers_api(stays: &[Stay]) -> bool {
    sum_ms(stays, |stay| {
        ["postman", "insomnia", "apifox"].contains(&proc(stay).as_str())
    }) >= 20 * MINUTE
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

#[allow(clippy::too_many_arguments)]
fn opportunity(
    id: &str,
    title: &str,
    evidence: String,
    summary: &str,
    search_query: &str,
    score: f64,
    replaces: &[&str],
    resource_ids: &[&str],
) -> Recommendation {
    Recommendation {
        id: id.to_owned(),
        title: title.to_owned(),
        evidence,
        summary: summary.to_owned(),
        search_query: search_query.to_owned(),
        replaces: replaces.iter().map(|value| (*value).to_owned()).collect(),
        score,
        resources: resources(resource_ids),
    }
}

/// Turns today's observed loops into tool opportunities. Returns everything,
/// sorted by friction; the caller decides how many to keep.
pub fn build_recommendations(stats: &DayStats) -> Vec<Recommendation> {
    let stays = &stats.stays;
    let apps = &stats.apps;
    let mut opportunities = build_alternatives(stays);
    let covers_api = covers_api(stays);

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
            &[],
            ids,
        ));
    }

    let browser_document_switches = cross_transitions(stays, is_browser, is_document);
    if browser_document_switches >= 4 {
        let active_ms = sum_ms(stays, |stay| is_browser(stay) || is_document(stay));
        opportunities.push(opportunity(
            "research-loop",
            "让网页内容直接进入笔记流程",
            format!("浏览器与文档或笔记应用之间切换 {browser_document_switches} 次，相关应用活跃 {} 分钟。", rounded_minutes(active_ms)),
            "把网页读取、来源引用和摘要整理做成可复用流程，保留人工核对来源这一步。",
            "open source research assistant MCP fetch Skill",
            browser_document_switches as f64 * 1.2,
            &[],
            &["fetchMcp", "agentSkills", "searxng"],
        ));
    }

    let meeting_ms = sum_ms(stays, |stay| is_meeting(&stay.name, &stay.app, &stay.title));
    if meeting_ms >= 30 * MINUTE {
        opportunities.push(opportunity(
            "meeting-notes",
            "把会议转写和纪要串成固定流程",
            format!("会议相关活动共 {} 分钟。", rounded_minutes(meeting_ms)),
            "在参与者同意的前提下先本地转写，再用固定模板生成决议和行动项，并人工复核。",
            "open source local meeting transcription summary",
            rounded_minutes(meeting_ms) as f64 / 4.0,
            &[],
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
            "把重复的文件整理动作交给工具",
            format!(
                "文件管理相关窗口活跃 {} 分钟，与其他应用切换 {file_switches} 次。",
                rounded_minutes(file_ms)
            ),
            "批量重命名先用成熟工具；只有规则明确且反复出现的文件动作才值得写成脚本。",
            "open source Windows batch rename file automation",
            file_switches as f64 + rounded_minutes(file_ms) as f64 / 5.0,
            &[],
            &["powerToys", "autohotkey"],
        ));
    }

    let api_title_hits = matching_title_count(
        apps,
        &Regex::new(r"(?i)\b(?:api|openapi|swagger|graphql|postman)\b|接口调试|接口文档").unwrap(),
    );
    let api_ms = sum_ms(stays, is_api_app);
    if api_title_hits >= 2 || (api_ms >= 10 * MINUTE && !covers_api) {
        let evidence = if api_title_hits >= 2 {
            format!("检测到 {api_title_hits} 次 API 或接口相关窗口标题。")
        } else {
            format!("API 调试工具活跃 {} 分钟。", rounded_minutes(api_ms))
        };
        opportunities.push(opportunity(
            "api-workflow",
            "把请求集合留在本地复用",
            evidence,
            "把常用请求、环境和示例响应整理成集合并纳入密钥检查，减少重复填写。",
            "open source API client request collection",
            api_title_hits as f64 * 2.0 + rounded_minutes(api_ms) as f64 / 5.0,
            &[],
            &["bruno", "hoppscotch"],
        ));
    }

    opportunities.sort_by(|a, b| b.score.total_cmp(&a.score));
    opportunities
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
            ..Default::default()
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
            ..Default::default()
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
        ts += 40 * MINUTE;
        // A paid API client used long enough to match an open-source alternative.
        stays.push(stay("postman", "Postman", "Collections", ts, 25));
        let apps = vec![AppStat {
            name: "code".to_owned(),
            app: "VS Code".to_owned(),
            category: "开发".to_owned(),
            ms: 1,
            switches: 0,
            titles: BTreeMap::from([("secret@example.com git commit".to_owned(), 3)]),
            title_ms: BTreeMap::new(),
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
        let alternative = recommendations
            .iter()
            .find(|item| item.id == "alternative-postman")
            .expect("long Postman use should surface an open-source alternative");
        assert_eq!(alternative.replaces, vec!["Postman"]);
        assert!(
            alternative
                .resources
                .iter()
                .any(|resource| resource.name.contains("Bruno"))
        );
        assert!(!recommendations.iter().any(|item| item.id == "api-workflow"));
        for item in &recommendations {
            assert!(item.evidence.chars().any(|ch| ch.is_ascii_digit()));
            assert!(
                item.resources
                    .iter()
                    .all(|resource| resource.url.starts_with("https://")
                        && !resource.permissions.is_empty())
            );
        }
    }
}
