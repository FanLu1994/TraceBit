use crate::{
    categories::{is_meeting, normalize_proc},
    i18n::{Lang, fill},
    store::{AppStat, DayStats, Stay},
    tr,
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

/// Canonical resource kinds (stored in profiles) mapped to display text.
fn kind_label(lang: Lang, kind: &'static str) -> &'static str {
    match kind {
        "开源项目" => tr!(
            lang,
            "开源项目",
            "Open source project",
            "オープンソースプロジェクト",
            "오픈 소스 프로젝트"
        ),
        "内置自动化" => tr!(
            lang,
            "内置自动化",
            "Built-in automation",
            "組み込み自動化",
            "내장 자동화"
        ),
        "开源替代" => tr!(
            lang,
            "开源替代",
            "Open source alternative",
            "オープンソース代替",
            "오픈 소스 대안"
        ),
        "Skill" => tr!(lang, "Skill", "Skill", "スキル", "스킬"),
        other => other,
    }
}

fn resource(id: &str, lang: Lang) -> Option<Resource> {
    let (name, kind, url, description, benefit, setup_cost, permissions) = match id {
        "just" => (
            "just",
            "开源项目",
            "https://github.com/casey/just",
            tr!(
                lang,
                "把重复的项目命令整理成带名称的任务入口。",
                "Turn repeated project commands into named task entry points.",
                "繰り返し使うプロジェクトのコマンドを名前付きタスクにまとめます。",
                "반복되는 프로젝트 명령을 이름 있는 작업으로 정리합니다."
            ),
            tr!(
                lang,
                "减少记忆命令和在编辑器、终端之间往返的成本。",
                "Cuts the cost of memorizing commands and hopping between editor and terminal.",
                "コマンドを覚える負担と、エディターとターミナルの往復を減らします。",
                "명령을 외우고 편집기와 터미널을 오가는 비용을 줄입니다."
            ),
            tr!(
                lang,
                "低，通常先整理 3 到 5 条常用命令。",
                "Low; start by collecting 3 to 5 common commands.",
                "低。まずよく使う 3〜5 個のコマンドをまとめるところから。",
                "낮음. 먼저 자주 쓰는 명령 3~5개를 정리하면 됩니다."
            ),
            tr!(
                lang,
                "命令文件会执行本机命令，使用前先审阅任务内容。",
                "Task files run local commands, so review their content before use.",
                "タスクファイルはローカルのコマンドを実行するため、使用前に内容を確認してください。",
                "작업 파일이 로컬 명령을 실행하므로 사용 전에 내용을 검토하세요."
            ),
        ),
        "vscodeTasks" => (
            "VS Code Tasks",
            "内置自动化",
            "https://code.visualstudio.com/docs/editor/tasks",
            tr!(
                lang,
                "把构建、测试和常用脚本留在编辑器内运行。",
                "Run builds, tests and common scripts inside the editor.",
                "ビルド、テスト、よく使うスクリプトをエディター内で実行できます。",
                "빌드, 테스트, 자주 쓰는 스크립트를 편집기 안에서 실행합니다."
            ),
            tr!(
                lang,
                "减少为了固定命令而切换到终端的次数。",
                "Fewer switches to the terminal just to run a fixed command.",
                "決まったコマンドのためにターミナルへ切り替える回数を減らします。",
                "고정된 명령을 실행하려고 터미널로 전환하는 횟수를 줄입니다."
            ),
            tr!(
                lang,
                "低，可从一条最常用命令开始。",
                "Low; start with the single most-used command.",
                "低。最もよく使うコマンド 1 つから始められます。",
                "낮음. 가장 자주 쓰는 명령 하나부터 시작할 수 있습니다."
            ),
            tr!(
                lang,
                "任务会执行本机命令，不要运行来源不明的任务配置。",
                "Tasks run local commands; never run task configs from unknown sources.",
                "タスクはローカルのコマンドを実行します。出どころ不明のタスク設定は実行しないでください。",
                "작업이 로컬 명령을 실행하므로 출처를 모르는 작업 설정은 실행하지 마세요."
            ),
        ),
        "lazygit" => (
            "lazygit",
            "开源项目",
            "https://github.com/jesseduffield/lazygit",
            tr!(
                lang,
                "在终端内集中处理提交、分支、暂存和变基等 Git 操作。",
                "Handle commits, branches, staging and rebase without leaving the terminal.",
                "コミット、ブランチ、ステージ、リベースなどの Git 操作をターミナル内でまとめて行えます。",
                "커밋, 브랜치, 스테이징, 리베이스 같은 Git 작업을 터미널 안에서 처리합니다."
            ),
            tr!(
                lang,
                "适合 Git 操作频繁时减少命令重复和上下文切换。",
                "Good when Git work is frequent: fewer repeated commands and context switches.",
                "Git 操作が多いときに、コマンドの繰り返しとコンテキストスイッチを減らせます。",
                "Git 작업이 잦을 때 명령 반복과 컨텍스트 전환을 줄여 줍니다."
            ),
            tr!(
                lang,
                "中，需要熟悉基本 Git 概念和少量快捷键。",
                "Medium; needs basic Git knowledge and a few shortcuts.",
                "中。基本的な Git の知識と少しのショートカットが必要です。",
                "보통. 기본 Git 개념과 약간의 단축키가 필요합니다."
            ),
            tr!(
                lang,
                "会读写 Git 仓库，破坏性操作前仍需确认和备份。",
                "Reads and writes the Git repo; still confirm and back up before destructive actions.",
                "Git リポジトリの読み書きを行います。破壊的な操作の前には確認とバックアップを。",
                "Git 저장소를 읽고 쓰므로 파괴적인 작업 전에는 확인과 백업이 필요합니다."
            ),
        ),
        "fetchMcp" => (
            "Fetch MCP Server",
            "MCP",
            "https://github.com/modelcontextprotocol/servers/tree/main/src/fetch",
            tr!(
                lang,
                "让兼容的 Agent 在用户请求下读取网页并提取内容。",
                "Lets a compatible agent fetch and extract web pages on request.",
                "対応するエージェントが依頼に応じて Web ページを取得し、内容を抽出できます。",
                "호환되는 에이전트가 요청 시 웹 페이지를 가져와 내용을 추출하게 합니다."
            ),
            tr!(
                lang,
                "减少复制网页正文到笔记或对话中的手工步骤。",
                "Removes the manual step of copying page text into notes or chats.",
                "Web ページ本文をノートや会話へコピーする手作業を減らします。",
                "웹 페이지 본문을 노트나 대화로 복사하는 수작업을 줄입니다."
            ),
            tr!(
                lang,
                "中，需要支持 MCP 的客户端和基础配置。",
                "Medium; needs an MCP-capable client and basic setup.",
                "中。MCP 対応クライアントと基本的な設定が必要です。",
                "보통. MCP를 지원하는 클라이언트와 기본 설정이 필요합니다."
            ),
            tr!(
                lang,
                "会产生联网请求；不要抓取未授权或敏感页面，启用前核对服务器来源。",
                "Makes network requests; don't fetch unauthorized or sensitive pages, and check the server's origin before enabling.",
                "ネットワーク通信が発生します。未許可のページや機密ページを取得せず、有効化前にサーバーの出どころを確認してください。",
                "네트워크 요청이 발생합니다. 권한 없는 페이지나 민감한 페이지를 가져오지 말고, 활성화 전에 서버 출처를 확인하세요."
            ),
        ),
        "agentSkills" => (
            tr!(
                lang,
                "阅读研究 Skill 工作流",
                "Research reading Skill workflow",
                "調査読解 Skill ワークフロー",
                "자료 조사 Skill 워크플로"
            ),
            "Skill",
            "https://github.com/anthropics/skills",
            tr!(
                lang,
                "参考官方 Skill 结构，把资料读取、来源整理和摘要写成可复用流程。",
                "Follow the official Skill structure to turn reading, source tracking and summarizing into a reusable flow.",
                "公式 Skill の構成に倣い、資料の読み込み、出典の整理、要約を再利用できる手順にします。",
                "공식 Skill 구조를 참고해 자료 읽기, 출처 정리, 요약을 재사용 가능한 흐름으로 만듭니다."
            ),
            tr!(
                lang,
                "适合反复进行资料收集和笔记整理的场景。",
                "Fits repeated research and note-organizing work.",
                "資料収集とノート整理を繰り返す場面に向いています。",
                "자료 수집과 노트 정리를 반복하는 상황에 적합합니다."
            ),
            tr!(
                lang,
                "中，需要先定义输入、输出和引用要求。",
                "Medium; define inputs, outputs and citation rules first.",
                "中。まず入力、出力、引用の要件を決める必要があります。",
                "보통. 먼저 입력, 출력, 인용 요건을 정의해야 합니다."
            ),
            tr!(
                lang,
                "Skill 能驱动宿主工具，安装前应审阅指令和允许调用的能力。",
                "Skills can drive host tools; review the instructions and allowed capabilities before installing.",
                "Skill はホストのツールを操作できます。インストール前に指示内容と呼び出せる機能を確認してください。",
                "Skill은 호스트 도구를 실행할 수 있으므로 설치 전에 지시문과 허용된 기능을 검토하세요."
            ),
        ),
        "searxng" => (
            "SearXNG",
            "开源项目",
            "https://github.com/searxng/searxng",
            tr!(
                lang,
                "可自托管的元搜索服务，为研究工作流提供统一搜索入口。",
                "A self-hostable metasearch service that gives research workflows one search entry point.",
                "セルフホスト可能なメタ検索サービスで、リサーチ作業に統一の検索窓口を提供します。",
                "자가 호스팅 가능한 메타 검색 서비스로, 리서치 작업에 통합 검색 진입점을 제공합니다."
            ),
            tr!(
                lang,
                "适合需要控制搜索数据流向的高级用户。",
                "For advanced users who need control over where search data goes.",
                "検索データの流れを管理したい上級ユーザー向けです。",
                "검색 데이터 흐름을 직접 관리하려는 고급 사용자에게 적합합니다."
            ),
            tr!(
                lang,
                "高，需要部署和维护服务。",
                "High; the service must be deployed and maintained.",
                "高。サービスの配備と運用が必要です。",
                "높음. 서비스를 배포하고 유지해야 합니다."
            ),
            tr!(
                lang,
                "上游搜索仍可能看到查询词，部署前检查日志和代理配置。",
                "Upstream search engines may still see queries; review logging and proxy settings before deploying.",
                "上流の検索エンジンには検索語が見える可能性があります。配備前にログとプロキシ設定を確認してください。",
                "상위 검색 엔진은 검색어를 볼 수 있습니다. 배포 전에 로그와 프록시 설정을 확인하세요."
            ),
        ),
        "whispercpp" => (
            "whisper.cpp",
            "开源项目",
            "https://github.com/ggml-org/whisper.cpp",
            tr!(
                lang,
                "在本机把会议录音转写成文本，再接续摘要流程。",
                "Transcribe meeting audio locally, then continue with a summary flow.",
                "会議の録音をローカルで文字起こしし、そのまま要約の工程につなげます。",
                "회의 녹음을 로컬에서 텍스트로 변환한 뒤 요약 흐름으로 이어 갑니다."
            ),
            tr!(
                lang,
                "减少手工整理逐字稿的工作量，音频不必交给云端转写服务。",
                "Less manual transcript cleanup, and audio needn't go to a cloud transcription service.",
                "文字起こしを手作業で整える負担が減り、音声をクラウドの文字起こしに渡す必要もありません。",
                "수기로 전사본을 정리하는 수고가 줄고, 오디오를 클라우드 전사 서비스에 넘길 필요가 없습니다."
            ),
            tr!(
                lang,
                "中，需要准备模型并确认本机性能。",
                "Medium; prepare a model and check local performance.",
                "中。モデルを用意し、ローカルの性能を確認する必要があります。",
                "보통. 모델을 준비하고 로컬 성능을 확인해야 합니다."
            ),
            tr!(
                lang,
                "录音前必须取得参与者同意，音频和转写稿应按敏感资料管理。",
                "Get participants' consent before recording; treat audio and transcripts as sensitive data.",
                "録音前に参加者の同意を得てください。音声と文字起こしは機密情報として扱います。",
                "녹음 전에 참가자 동의를 받아야 하며, 오디오와 전사본은 민감 자료로 관리해야 합니다."
            ),
        ),
        "meetingSkill" => (
            tr!(
                lang,
                "会议总结 Skill 工作流",
                "Meeting summary Skill workflow",
                "会議要約 Skill ワークフロー",
                "회의 요약 Skill 워크플로"
            ),
            "Skill",
            "https://github.com/anthropics/skills",
            tr!(
                lang,
                "参考官方 Skill 结构，把转写稿整理成决议、行动项和负责人。",
                "Follow the official Skill structure to turn a transcript into decisions, action items and owners.",
                "公式 Skill の構成に倣い、文字起こしを決定事項、アクション項目、担当者に整理します。",
                "공식 Skill 구조를 참고해 전사본을 결정 사항, 실행 항목, 담당자로 정리합니다."
            ),
            tr!(
                lang,
                "把重复的报告格式和检查项固化下来。",
                "Locks in the recurring report format and checklist.",
                "繰り返し使うレポート形式とチェック項目を固定できます。",
                "반복되는 보고서 형식과 점검 항목을 고정합니다."
            ),
            tr!(
                lang,
                "中，需要先用一份已核对纪要校准模板。",
                "Medium; calibrate the template with one reviewed meeting note first.",
                "中。まず確認済みの議事録 1 件でテンプレートを調整する必要があります。",
                "보통. 먼저 검토가 끝난 회의록 하나로 템플릿을 맞춰야 합니다."
            ),
            tr!(
                lang,
                "转写稿可能含敏感信息，优先本地处理并限制工具权限。",
                "Transcripts may contain sensitive data; prefer local processing and limit tool permissions.",
                "文字起こしには機密情報が含まれる可能性があります。ローカル処理を優先し、ツールの権限を制限してください。",
                "전사본에는 민감 정보가 있을 수 있으므로 로컬 처리를 우선하고 도구 권한을 제한하세요."
            ),
        ),
        "powerToys" => (
            "Microsoft PowerToys",
            "开源项目",
            "https://github.com/microsoft/PowerToys",
            tr!(
                lang,
                "PowerRename 等工具可处理批量重命名和常见 Windows 操作。",
                "PowerRename and friends handle batch renaming and common Windows actions.",
                "PowerRename などのツールで一括リネームやよく使う Windows 操作を処理できます。",
                "PowerRename 등의 도구로 일괄 이름 바꾸기와 자주 쓰는 Windows 작업을 처리합니다."
            ),
            tr!(
                lang,
                "减少重复的文件命名和窗口整理动作。",
                "Fewer repetitive file-naming and window-arranging actions.",
                "ファイル名の繰り返し入力やウィンドウ整理の手間を減らします。",
                "반복적인 파일 이름 지정과 창 정리 작업을 줄입니다."
            ),
            tr!(
                lang,
                "低，可先只启用一个实际需要的模块。",
                "Low; enable only the module you actually need at first.",
                "低。まず実際に必要なモジュールだけを有効にできます。",
                "낮음. 처음에는 실제로 필요한 모듈만 켜면 됩니다."
            ),
            tr!(
                lang,
                "批量修改前先备份并小范围试跑，避免误改文件名。",
                "Back up and test on a small set before batch changes, so filenames aren't renamed by mistake.",
                "一括変更の前にバックアップし、小さな範囲で試してファイル名の誤変更を防いでください。",
                "일괄 변경 전에 백업하고 작은 범위에서 시험해 파일 이름이 잘못 바뀌는 일을 막으세요."
            ),
        ),
        "autohotkey" => (
            "AutoHotkey",
            "开源项目",
            "https://github.com/AutoHotkey/AutoHotkey",
            tr!(
                lang,
                "将明确、重复的 Windows 操作写成可审阅的自动化脚本。",
                "Turn clear, repetitive Windows actions into reviewable automation scripts.",
                "明確で繰り返しの多い Windows 操作を、確認できる自動化スクリプトにします。",
                "명확하고 반복적인 Windows 작업을 검토 가능한 자동화 스크립트로 만듭니다."
            ),
            tr!(
                lang,
                "适合固定流程，不适合尚未弄清规则的一次性操作。",
                "Fits fixed workflows, not one-off actions whose rules aren't clear yet.",
                "定型的な手順に向き、ルールが固まっていない単発の操作には向きません。",
                "고정된 절차에 적합하며, 규칙이 아직 분명하지 않은 일회성 작업에는 맞지 않습니다."
            ),
            tr!(
                lang,
                "中到高，需要编写、测试并维护脚本。",
                "Medium to high; scripts must be written, tested and maintained.",
                "中〜高。スクリプトの作成、テスト、保守が必要です。",
                "보통~높음. 스크립트를 작성하고 시험하고 유지해야 합니다."
            ),
            tr!(
                lang,
                "脚本可模拟输入和操作文件，只运行自己理解且来源可信的脚本。",
                "Scripts can simulate input and touch files; only run ones you understand from trusted sources.",
                "スクリプトは入力の模擬やファイル操作ができます。内容を理解でき、信頼できる出どころのものだけを実行してください。",
                "스크립트는 입력을 흉내 내고 파일을 조작할 수 있으므로, 이해하고 신뢰하는 출처의 스크립트만 실행하세요."
            ),
        ),
        "bruno" => (
            "Bruno",
            "开源项目",
            "https://github.com/usebruno/bruno",
            tr!(
                lang,
                "把 API 请求、环境和集合格式保存在本地项目中。",
                "Keeps API requests, environments and collections in a local project.",
                "API リクエスト、環境、コレクションをローカルのプロジェクトに保存します。",
                "API 요청, 환경, 컬렉션을 로컬 프로젝트에 저장합니다."
            ),
            tr!(
                lang,
                "减少重复填写请求和在不同窗口之间复制参数。",
                "Less re-filling requests and copying parameters between windows.",
                "リクエストの再入力やウィンドウ間のパラメータコピーを減らします。",
                "요청을 다시 채우고 창 사이에서 파라미터를 복사하는 일을 줄입니다."
            ),
            tr!(
                lang,
                "低到中，可先迁移最常用的一组请求。",
                "Low to medium; migrate the most-used request set first.",
                "低〜中。まず最もよく使うリクエスト群を移行するところから。",
                "낮음~보통. 먼저 가장 자주 쓰는 요청 묶음을 옮기면 됩니다."
            ),
            tr!(
                lang,
                "环境文件可能含令牌，提交仓库前检查密钥和敏感地址。",
                "Environment files may hold tokens; check for keys and sensitive hosts before committing.",
                "環境ファイルにはトークンが含まれることがあります。コミット前にキーと機密アドレスを確認してください。",
                "환경 파일에 토큰이 있을 수 있으므로 저장소에 커밋하기 전에 키와 민감한 주소를 확인하세요."
            ),
        ),
        "hoppscotch" => (
            "Hoppscotch",
            "开源项目",
            "https://github.com/hoppscotch/hoppscotch",
            tr!(
                lang,
                "用于组织、发送和协作管理 API 请求。",
                "Organize, send and share API requests.",
                "API リクエストの整理、送信、共同管理に使います。",
                "API 요청을 정리하고 보내며 함께 관리합니다."
            ),
            tr!(
                lang,
                "适合需要复用请求集合和统一调试入口的场景。",
                "Fits cases that need reusable request collections and one debugging entry point.",
                "リクエストのコレクションを再利用し、デバッグの窓口を一本化したい場面に向いています。",
                "요청 컬렉션을 재사용하고 디버깅 진입점을 하나로 모아야 하는 상황에 적합합니다."
            ),
            tr!(
                lang,
                "低到中，可从单个项目开始。",
                "Low to medium; start with a single project.",
                "低〜中。1 つのプロジェクトから始められます。",
                "낮음~보통. 프로젝트 하나부터 시작할 수 있습니다."
            ),
            tr!(
                lang,
                "请求会接触接口和凭据，自托管或云同步前确认数据流向。",
                "Requests touch APIs and credentials; confirm where data goes before self-hosting or syncing.",
                "リクエストは API と認証情報に触れます。セルフホストやクラウド同期の前にデータの流れを確認してください。",
                "요청이 API와 자격 증명을 다루므로 자가 호스팅이나 클라우드 동기화 전에 데이터 흐름을 확인하세요."
            ),
        ),
        "dbeaver" => (
            "DBeaver Community",
            "开源替代",
            "https://github.com/dbeaver/dbeaver",
            tr!(
                lang,
                "用同一个窗口连接 PostgreSQL、MySQL、SQL Server 等常见数据库。",
                "Connect to PostgreSQL, MySQL, SQL Server and other common databases from one window.",
                "PostgreSQL、MySQL、SQL Server などの一般的なデータベースに同じウィンドウで接続します。",
                "PostgreSQL, MySQL, SQL Server 등 일반적인 데이터베이스에 한 창에서 접속합니다."
            ),
            tr!(
                lang,
                "把查看表结构和执行 SQL 留在同一个免费客户端里，不必为每个库装一套工具。",
                "Keep schema browsing and SQL in one free client instead of a separate tool per database.",
                "テーブル構造の確認と SQL 実行を 1 つの無料クライアントにまとめ、DB ごとに別ツールを入れる必要がありません。",
                "테이블 구조 확인과 SQL 실행을 하나의 무료 클라이언트에서 처리하고, DB마다 도구를 따로 설치할 필요가 없습니다."
            ),
            tr!(
                lang,
                "低到中，可先迁移一个最常用的连接。",
                "Low to medium; move the most-used connection first.",
                "低〜中。まず最もよく使う接続を移すところから。",
                "낮음~보통. 먼저 가장 자주 쓰는 연결을 옮기면 됩니다."
            ),
            tr!(
                lang,
                "连接配置里可能有数据库密码，取值前先确认保存方式和访问权限。",
                "Connection configs may contain database passwords; confirm storage and access before pulling data.",
                "接続設定に DB のパスワードが含まれることがあります。データを取得する前に保存方法とアクセス権を確認してください。",
                "연결 설정에 데이터베이스 비밀번호가 있을 수 있으므로 데이터를 가져오기 전에 저장 방식과 접근 권한을 확인하세요."
            ),
        ),
        "heidisql" => (
            "HeidiSQL",
            "开源替代",
            "https://github.com/HeidiSQL/HeidiSQL",
            tr!(
                lang,
                "轻量的 Windows 数据库客户端，覆盖常见的表、查询和数据导出操作。",
                "A lightweight Windows database client covering tables, queries and data export.",
                "テーブル、クエリ、データ出力といった一般的な操作を扱う軽量な Windows データベースクライアントです。",
                "테이블, 쿼리, 데이터 내보내기 등 일반적인 작업을 다루는 가벼운 Windows 데이터베이스 클라이언트입니다."
            ),
            tr!(
                lang,
                "适合以 MySQL 或 MariaDB 为主的日常查询，启动和操作都更轻。",
                "Fits everyday MySQL or MariaDB queries: lighter to start and operate.",
                "MySQL や MariaDB 中心の日常的なクエリに向き、起動も操作も軽いです。",
                "MySQL이나 MariaDB 중심의 일상 쿼리에 적합하며 실행과 조작이 가볍습니다."
            ),
            tr!(
                lang,
                "低，配置一个连接即可试用。",
                "Low; one connection is enough to try it.",
                "低。接続を 1 つ設定すれば試せます。",
                "낮음. 연결 하나만 설정하면 시험해 볼 수 있습니다."
            ),
            tr!(
                lang,
                "会读写数据库；生产库上先确认账号权限，避免误执行写操作。",
                "Reads and writes databases; check account permissions on production to avoid accidental writes.",
                "データベースの読み書きを行います。本番 DB ではアカウント権限を確認し、誤った書き込みを避けてください。",
                "데이터베이스를 읽고 씁니다. 운영 DB에서는 계정 권한을 확인해 실수로 쓰지 않도록 하세요."
            ),
        ),
        "gimp" => (
            "GIMP",
            "开源替代",
            "https://github.com/GNOME/gimp",
            tr!(
                lang,
                "图像编辑和合成，图层、选区、滤镜等常用操作齐全。",
                "Image editing and compositing with layers, selections and filters.",
                "画像編集と合成。レイヤー、選択範囲、フィルターなどの基本操作が揃っています。",
                "이미지 편집과 합성. 레이어, 선택 영역, 필터 등 기본 작업이 갖춰져 있습니다."
            ),
            tr!(
                lang,
                "保留相近的图层编辑方式，不依赖订阅和账号。",
                "Keeps a similar layer-based workflow without subscriptions or accounts.",
                "近いレイヤー編集の流れを保ちながら、サブスクリプションやアカウントに依存しません。",
                "비슷한 레이어 편집 방식을 유지하면서 구독과 계정에 의존하지 않습니다."
            ),
            tr!(
                lang,
                "中，界面和快捷键与商业软件不同，需要一段适应期。",
                "Medium; the UI and shortcuts differ from commercial tools, so allow for adjustment.",
                "中。UI とショートカットが商用ソフトと異なるため、慣れるまでの期間が必要です。",
                "보통. UI와 단축키가 상용 소프트웨어와 달라 적응 기간이 필요합니다."
            ),
            tr!(
                lang,
                "插件来源需自行核对；处理大图时注意内存占用。",
                "Check plugin sources yourself; watch memory use on large images.",
                "プラグインの出どころはご自身で確認してください。大きな画像ではメモリ使用量に注意が必要です。",
                "플러그인 출처는 직접 확인해야 하며, 큰 이미지를 다룰 때 메모리 사용량에 주의하세요."
            ),
        ),
        "krita" => (
            "Krita",
            "开源替代",
            "https://github.com/KDE/krita",
            tr!(
                lang,
                "面向绘图和插画的绘画工具，笔刷和画布操作完整。",
                "A drawing tool for illustration with full brush and canvas controls.",
                "イラスト向けの描画ツールで、ブラシとキャンバス操作が充実しています。",
                "일러스트용 드로잉 도구로 브러시와 캔버스 조작이 충실합니다."
            ),
            tr!(
                lang,
                "适合以手绘、上色为主的日常工作，本地文件格式开放。",
                "Fits day-to-day drawing and coloring, with an open local file format.",
                "手描きや彩色が中心の日常作業に向き、ローカルのファイル形式はオープンです。",
                "손으로 그리기와 채색 중심의 일상 작업에 적합하며 로컬 파일 형식이 개방되어 있습니다."
            ),
            tr!(
                lang,
                "中，笔刷和快捷键需要重新配置一次。",
                "Medium; brushes and shortcuts need a one-time setup.",
                "中。ブラシとショートカットを一度設定し直す必要があります。",
                "보통. 브러시와 단축키를 한 번 다시 설정해야 합니다."
            ),
            tr!(
                lang,
                "笔刷和素材包来源需自行核对。",
                "Check the source of brush and asset packs yourself.",
                "ブラシや素材パックの出どころはご自身で確認してください。",
                "브러시와 소재 팩의 출처는 직접 확인해야 합니다."
            ),
        ),
        "inkscape" => (
            "Inkscape",
            "开源替代",
            "https://github.com/inkscape/inkscape",
            tr!(
                lang,
                "矢量图形编辑，路径、节点和 SVG 输出都是原生能力。",
                "Vector editing where paths, nodes and SVG output are native.",
                "パス、ノード、SVG 出力を標準で扱うベクター編集ツールです。",
                "패스, 노드, SVG 출력을 기본으로 지원하는 벡터 편집 도구입니다."
            ),
            tr!(
                lang,
                "用 SVG 作为中间格式，和现有设计稿互通，不锁定账号。",
                "SVG works as the exchange format with existing designs, and no account is required.",
                "SVG を中間形式として既存のデザインとやり取りでき、アカウントに縛られません。",
                "SVG를 중간 형식으로 기존 디자인과 주고받을 수 있고 계정에 묶이지 않습니다."
            ),
            tr!(
                lang,
                "中，路径工具的操作习惯需要适应。",
                "Medium; the path tool's habits take some getting used to.",
                "中。パスツールの操作感に慣れる必要があります。",
                "보통. 패스 도구의 조작 방식에 익숙해져야 합니다."
            ),
            tr!(
                lang,
                "导入外部素材前先确认授权。",
                "Confirm licensing before importing external assets.",
                "外部素材を読み込む前にライセンスを確認してください。",
                "외부 소재를 가져오기 전에 라이선스를 확인하세요."
            ),
        ),
        "darktable" => (
            "darktable",
            "开源替代",
            "https://github.com/darktable-org/darktable",
            tr!(
                lang,
                "照片管理和非破坏性调色流程。",
                "Photo management with a non-destructive color workflow.",
                "写真管理と非破壊の色調整ワークフロー。",
                "사진 관리와 비파괴 색 보정 흐름."
            ),
            tr!(
                lang,
                "RAW 原片留在本地，调色参数独立保存，可随时回退。",
                "RAW originals stay local and edits are stored separately, so you can always roll back.",
                "RAW 原本はローカルに残り、調整パラメータは別に保存されるのでいつでも戻せます。",
                "RAW 원본은 로컬에 남고 보정 값이 따로 저장되어 언제든 되돌릴 수 있습니다."
            ),
            tr!(
                lang,
                "中到高，调色模块需要重新熟悉。",
                "Medium to high; the color modules need relearning.",
                "中〜高。色調整モジュールに慣れ直す必要があります。",
                "보통~높음. 색 보정 모듈을 다시 익혀야 합니다."
            ),
            tr!(
                lang,
                "目录数据库保存在本机，迁移前先备份。",
                "The catalog database lives on this machine; back it up before migrating.",
                "カタログデータベースはローカルに保存されます。移行前にバックアップしてください。",
                "카탈로그 데이터베이스는 로컬에 저장되므로 이전 전에 백업하세요."
            ),
        ),
        "kdenlive" => (
            "Kdenlive",
            "开源替代",
            "https://github.com/KDE/kdenlive",
            tr!(
                lang,
                "多轨视频剪辑，支持常用编码和代理剪辑。",
                "Multi-track video editing with common codecs and proxy editing.",
                "一般的なコーデックとプロキシ編集に対応したマルチトラック動画編集。",
                "일반적인 코덱과 프록시 편집을 지원하는 멀티트랙 영상 편집."
            ),
            tr!(
                lang,
                "时间线式剪辑的操作方式接近现有习惯，项目文件可版本管理。",
                "Timeline editing stays close to existing habits, and project files can be versioned.",
                "タイムライン式の編集は従来の感覚に近く、プロジェクトファイルはバージョン管理できます。",
                "타임라인 편집 방식이 기존 습관과 비슷하고 프로젝트 파일을 버전 관리할 수 있습니다."
            ),
            tr!(
                lang,
                "中，编码和代理设置需要按素材调整一次。",
                "Medium; codec and proxy settings need tuning per source material.",
                "中。コーデックとプロキシの設定を素材に合わせて一度調整する必要があります。",
                "보통. 코덱과 프록시 설정을 소재에 맞게 한 번 조정해야 합니다."
            ),
            tr!(
                lang,
                "导出前确认编码参数；素材版权和授权需自行确认。",
                "Check export settings; verify asset copyright and licensing yourself.",
                "書き出し前にエンコード設定を確認してください。素材の著作権とライセンスはご自身で確認が必要です。",
                "내보내기 전에 인코딩 설정을 확인하고, 소재 저작권과 라이선스는 직접 확인해야 합니다."
            ),
        ),
        "shotcut" => (
            "Shotcut",
            "开源替代",
            "https://github.com/mltframework/shotcut",
            tr!(
                lang,
                "轻量视频剪辑，直接支持大量格式，无需先转码。",
                "Lightweight video editing that opens many formats without transcoding first.",
                "多くの形式をそのまま開ける軽量な動画編集ツールで、事前の変換が不要です。",
                "다양한 형식을 바로 열 수 있는 가벼운 영상 편집 도구로, 미리 변환할 필요가 없습니다."
            ),
            tr!(
                lang,
                "打开即剪，减少素材转码这一步。",
                "Edit right after opening, without the transcode step.",
                "開いてすぐ編集でき、素材変換の工程を省けます。",
                "열자마자 편집할 수 있어 소재 변환 단계를 줄입니다."
            ),
            tr!(
                lang,
                "低到中，基础剪辑可上手即用。",
                "Low to medium; basic editing works right away.",
                "低〜中。基本的な編集ならすぐに使えます。",
                "낮음~보통. 기본 편집은 바로 사용할 수 있습니다."
            ),
            tr!(
                lang,
                "复杂工程请定期保存副本。",
                "Save copies of complex projects regularly.",
                "複雑なプロジェクトは定期的にコピーを保存してください。",
                "복잡한 프로젝트는 정기적으로 사본을 저장하세요."
            ),
        ),
        "penpot" => (
            "Penpot",
            "开源替代",
            "https://github.com/penpot/penpot",
            tr!(
                lang,
                "开源的设计与原型协作工具，可自托管。",
                "Open-source design and prototyping collaboration, self-hostable.",
                "オープンソースのデザイン・プロトタイプ共同作業ツールで、セルフホストも可能です。",
                "오픈 소스 디자인 및 프로토타입 협업 도구로 자가 호스팅할 수 있습니다."
            ),
            tr!(
                lang,
                "设计文件和协作数据可以放在自己的服务器上，不绑定单一账号体系。",
                "Design files and collaboration data can live on your own server, independent of one account system.",
                "デザインファイルと共同作業データを自前のサーバーに置け、特定のアカウント体系に縛られません。",
                "디자인 파일과 협업 데이터를 자체 서버에 둘 수 있고 특정 계정 체계에 묶이지 않습니다."
            ),
            tr!(
                lang,
                "中，自托管需要部署；也可先用官方托管验证。",
                "Medium; self-hosting needs deployment, or validate first on the official hosted version.",
                "中。セルフホストには配備が必要です。公式のホスティングで先に試すこともできます。",
                "보통. 자가 호스팅에는 배포가 필요하며, 공식 호스팅으로 먼저 검증할 수도 있습니다."
            ),
            tr!(
                lang,
                "团队协作会接触设计稿，自托管前确认备份和访问控制。",
                "Team collaboration touches design files; confirm backups and access control before self-hosting.",
                "チーム作業ではデザインファイルを扱います。セルフホスト前にバックアップとアクセス制御を確認してください。",
                "팀 협업에서 디자인 파일을 다루므로 자가 호스팅 전에 백업과 접근 제어를 확인하세요."
            ),
        ),
        "drawio" => (
            "draw.io Desktop",
            "开源替代",
            "https://github.com/jgraph/drawio-desktop",
            tr!(
                lang,
                "本地运行的流程图、架构图和线框工具。",
                "A locally running flowchart, architecture and wireframe tool.",
                "ローカルで動くフローチャート、構成図、ワイヤーフレームのツールです。",
                "로컬에서 실행되는 순서도, 아키텍처 다이어그램, 와이어프레임 도구입니다."
            ),
            tr!(
                lang,
                "图形文件保存在本地，导出格式通用，适合反复修改的图。",
                "Diagram files stay local and export to common formats, good for diagrams that keep changing.",
                "図のファイルはローカルに保存され、書き出し形式も汎用的なので、何度も直す図に向いています。",
                "다이어그램 파일이 로컬에 저장되고 내보내기 형식이 일반적이어서 계속 수정하는 도표에 적합합니다."
            ),
            tr!(
                lang,
                "低，现有图形习惯基本可以沿用。",
                "Low; existing diagramming habits carry over.",
                "低。これまでの図の描き方をほぼそのまま使えます。",
                "낮음. 기존 다이어그램 작업 습관을 거의 그대로 쓸 수 있습니다."
            ),
            tr!(
                lang,
                "导入第三方图形库前确认来源。",
                "Check the source before importing third-party shape libraries.",
                "サードパーティの図形ライブラリを読み込む前に出どころを確認してください。",
                "서드파티 도형 라이브러리를 가져오기 전에 출처를 확인하세요."
            ),
        ),
        "excalidraw" => (
            "Excalidraw",
            "开源替代",
            "https://github.com/excalidraw/excalidraw",
            tr!(
                lang,
                "手绘风格的草图和白板，可本地或自托管使用。",
                "Hand-drawn sketches and whiteboards, usable locally or self-hosted.",
                "手描き風のスケッチとホワイトボード。ローカルでもセルフホストでも使えます。",
                "손그림 스타일 스케치와 화이트보드로, 로컬이나 자가 호스팅에서 쓸 수 있습니다."
            ),
            tr!(
                lang,
                "适合快速画流程和讨论稿，文件格式是开放的 JSON。",
                "Good for quick flow sketches and discussion drafts; files are open JSON.",
                "流れの素早いスケッチや議論用の下書きに向き、ファイル形式はオープンな JSON です。",
                "빠른 흐름 스케치와 논의용 초안에 적합하며 파일 형식은 개방형 JSON입니다."
            ),
            tr!(
                lang,
                "低，浏览器或桌面都能用。",
                "Low; works in the browser or on the desktop.",
                "低。ブラウザでもデスクトップでも使えます。",
                "낮음. 브라우저와 데스크톱 모두에서 사용할 수 있습니다."
            ),
            tr!(
                lang,
                "分享链接前确认画布里没有敏感信息。",
                "Make sure the canvas holds no sensitive data before sharing a link.",
                "共有リンクを渡す前に、キャンバスに機密情報がないか確認してください。",
                "공유 링크를 보내기 전에 캔버스에 민감한 정보가 없는지 확인하세요."
            ),
        ),
        "rustdesk" => (
            "RustDesk",
            "开源替代",
            "https://github.com/rustdesk/rustdesk",
            tr!(
                lang,
                "远程桌面控制，可自建中继服务。",
                "Remote desktop control with an optional self-hosted relay.",
                "リモートデスクトップ操作。リレーサーバーは自前で用意できます。",
                "원격 데스크톱 제어로, 중계 서버를 직접 구축할 수 있습니다."
            ),
            tr!(
                lang,
                "连接方式与现有远程工具一致，连接数据可以自己掌控。",
                "Connects like existing remote tools, while you control the connection data.",
                "接続の流れは既存のリモートツールと同じで、接続データを自分で管理できます。",
                "연결 방식은 기존 원격 도구와 같으면서 연결 데이터를 직접 관리할 수 있습니다."
            ),
            tr!(
                lang,
                "中，自建中继需要一台服务器；直连则接近即装即用。",
                "Medium; a self-hosted relay needs a server, while direct connections work almost out of the box.",
                "中。自前のリレーにはサーバーが必要ですが、直接接続ならほぼ導入するだけで使えます。",
                "보통. 자체 중계에는 서버가 필요하지만 직접 연결은 거의 설치만 하면 됩니다."
            ),
            tr!(
                lang,
                "远程控制权限很大，务必设置强密码并限制可连接的设备。",
                "Remote control is powerful: set strong passwords and limit which devices may connect.",
                "リモート操作の権限は大きいため、強力なパスワードを設定し、接続できる端末を制限してください。",
                "원격 제어 권한이 크므로 강력한 비밀번호를 설정하고 접속 가능한 기기를 제한하세요."
            ),
        ),
        "winmerge" => (
            "WinMerge",
            "开源替代",
            "https://github.com/WinMerge/winmerge",
            tr!(
                lang,
                "文件和目录对比、合并，支持三方对比。",
                "Compare and merge files and folders, including three-way merges.",
                "ファイルとフォルダーの比較・マージ、3 方向の比較にも対応します。",
                "파일과 폴더 비교 및 병합을 지원하며 3방향 비교도 가능합니다."
            ),
            tr!(
                lang,
                "对比操作和现有的双栏习惯一致，可直接接入版本库目录。",
                "Two-pane diffing matches existing habits and can point straight at a repo folder.",
                "2 ペインの比較は従来の感覚と同じで、リポジトリのフォルダーをそのまま開けます。",
                "2단 비교 방식이 기존 습관과 같고 저장소 폴더를 바로 열 수 있습니다."
            ),
            tr!(
                lang,
                "低，选中两个目录即可开始。",
                "Low; pick two folders and start.",
                "低。2 つのフォルダーを選べば始められます。",
                "낮음. 폴더 두 개를 선택하면 시작할 수 있습니다."
            ),
            tr!(
                lang,
                "合并写回前先确认备份，避免覆盖他人改动。",
                "Confirm backups before merging back, so others' changes aren't overwritten.",
                "マージで書き戻す前にバックアップを確認し、他の人の変更を上書きしないようにしてください。",
                "병합으로 되쓸기 전에 백업을 확인해 다른 사람의 변경을 덮어쓰지 않도록 하세요."
            ),
        ),
        "nanazip" => (
            "NanaZip",
            "开源替代",
            "https://github.com/M2Team/NanaZip",
            tr!(
                lang,
                "Windows 右键菜单集成的现代压缩工具，兼容 7-Zip 格式。",
                "A modern archive tool in the Windows context menu, compatible with 7-Zip formats.",
                "Windows の右クリックメニューに統合された現代的な圧縮ツールで、7-Zip 形式に対応します。",
                "Windows 우클릭 메뉴에 통합된 현대적인 압축 도구로 7-Zip 형식과 호환됩니다."
            ),
            tr!(
                lang,
                "解压和压缩的操作位置不变，少一次打开主界面的步骤。",
                "Compress and extract from the same place, with one fewer step into a main window.",
                "圧縮と解凍の操作場所はそのままで、メイン画面を開く手間が 1 回減ります。",
                "압축과 압축 풀기 위치는 그대로이고, 메인 화면을 여는 단계가 하나 줄어듭니다."
            ),
            tr!(
                lang,
                "低，安装后沿用右键菜单。",
                "Low; keep using the context menu after installing.",
                "低。インストール後は右クリックメニューをそのまま使えます。",
                "낮음. 설치 후 우클릭 메뉴를 그대로 사용하면 됩니다."
            ),
            tr!(
                lang,
                "解压来源不明的压缩包前先扫描。",
                "Scan archives from unknown sources before extracting.",
                "出どころ不明のアーカイブは解凍前にスキャンしてください。",
                "출처를 모르는 압축 파일은 풀기 전에 검사하세요."
            ),
        ),
        "tabby" => (
            "Tabby",
            "开源替代",
            "https://github.com/Eugeny/tabby",
            tr!(
                lang,
                "集成本地终端、SSH 会话和串口的终端工具。",
                "A terminal that combines local shells, SSH sessions and serial ports.",
                "ローカルのシェル、SSH セッション、シリアルポートをまとめたターミナルです。",
                "로컬 셸, SSH 세션, 시리얼 포트를 하나로 모은 터미널입니다."
            ),
            tr!(
                lang,
                "把分散在多个工具里的会话收进一个窗口，配置可导出。",
                "Brings sessions scattered across tools into one window, with exportable config.",
                "複数のツールに散らばったセッションを 1 つのウィンドウにまとめ、設定は書き出せます。",
                "여러 도구에 흩어진 세션을 한 창으로 모으고 설정을 내보낼 수 있습니다."
            ),
            tr!(
                lang,
                "中，已有会话和密钥需要迁移一次。",
                "Medium; existing sessions and keys need a one-time migration.",
                "中。既存のセッションと鍵を一度移行する必要があります。",
                "보통. 기존 세션과 키를 한 번 이전해야 합니다."
            ),
            tr!(
                lang,
                "会话配置可能包含主机和密钥路径，注意文件权限。",
                "Session configs may contain hosts and key paths; mind file permissions.",
                "セッション設定にはホストや鍵のパスが含まれることがあります。ファイル権限に注意してください。",
                "세션 설정에 호스트와 키 경로가 있을 수 있으므로 파일 권한에 주의하세요."
            ),
        ),
        "podmanDesktop" => (
            "Podman Desktop",
            "开源替代",
            "https://github.com/containers/podman-desktop",
            tr!(
                lang,
                "无守护进程的容器管理界面，兼容常见容器工作流。",
                "A daemonless container management UI compatible with common container workflows.",
                "デーモン不要のコンテナ管理 UI で、一般的なコンテナの流れに対応します。",
                "데몬이 필요 없는 컨테이너 관리 UI로 일반적인 컨테이너 흐름과 호환됩니다."
            ),
            tr!(
                lang,
                "镜像、容器和 Pod 的管理方式接近现有习惯，不依赖商业授权。",
                "Managing images, containers and pods feels like existing habits without a commercial license.",
                "イメージ、コンテナ、Pod の管理は従来の感覚に近く、商用ライセンスに依存しません。",
                "이미지, 컨테이너, Pod 관리 방식이 기존 습관과 비슷하고 상용 라이선스에 의존하지 않습니다."
            ),
            tr!(
                lang,
                "中，首次切换需要迁移本地镜像和卷。",
                "Medium; the first switch needs local images and volumes migrated.",
                "中。初回の切り替えではローカルのイメージとボリュームを移行する必要があります。",
                "보통. 처음 전환할 때 로컬 이미지와 볼륨을 옮겨야 합니다."
            ),
            tr!(
                lang,
                "容器会挂载本地目录，运行前确认挂载范围和镜像来源。",
                "Containers mount local folders; check mount scope and image sources before running.",
                "コンテナはローカルのフォルダーをマウントします。実行前にマウント範囲とイメージの出どころを確認してください。",
                "컨테이너가 로컬 폴더를 마운트하므로 실행 전에 마운트 범위와 이미지 출처를 확인하세요."
            ),
        ),
        "logseq" => (
            "Logseq",
            "开源替代",
            "https://github.com/logseq/logseq",
            tr!(
                lang,
                "本地优先的大纲笔记，页面以 Markdown 文件保存在本地。",
                "Local-first outliner notes, with pages stored as local Markdown files.",
                "ローカル優先のアウトラインノートで、ページは Markdown ファイルとしてローカルに保存されます。",
                "로컬 우선 아웃라인 노트로, 페이지가 Markdown 파일로 로컬에 저장됩니다."
            ),
            tr!(
                lang,
                "沿用现有的双链和日志写法，笔记不被单一服务锁定。",
                "Keep your existing linking and journal habits; notes aren't locked to one service.",
                "これまでのリンクとジャーナルの書き方を保ち、ノートが特定サービスに縛られません。",
                "기존의 링크와 저널 작성 방식을 유지하고 노트가 특정 서비스에 묶이지 않습니다."
            ),
            tr!(
                lang,
                "中到高，已有页面需要导入并检查层级。",
                "Medium to high; existing pages must be imported and their hierarchy checked.",
                "中〜高。既存ページを読み込んで階層を確認する必要があります。",
                "보통~높음. 기존 페이지를 가져와 계층을 확인해야 합니다."
            ),
            tr!(
                lang,
                "本地目录可能含敏感笔记，同步前确认加密和备份方式。",
                "The local folder may hold sensitive notes; confirm encryption and backups before syncing.",
                "ローカルのフォルダーに機密ノートが含まれることがあります。同期前に暗号化とバックアップを確認してください。",
                "로컬 폴더에 민감한 노트가 있을 수 있으므로 동기화 전에 암호화와 백업 방식을 확인하세요."
            ),
        ),
        "zettlr" => (
            "Zettlr",
            "开源替代",
            "https://github.com/Zettlr/Zettlr",
            tr!(
                lang,
                "面向 Markdown 文件的写作与导出工具。",
                "A writing and export tool built around Markdown files.",
                "Markdown ファイル中心の執筆と書き出しツールです。",
                "Markdown 파일 중심의 집필 및 내보내기 도구입니다."
            ),
            tr!(
                lang,
                "文件就是普通的 .md，写作和导出流程可以沿用。",
                "Files stay plain .md, so the writing and export flow carries over.",
                "ファイルは通常の .md のままなので、執筆と書き出しの流れをそのまま使えます。",
                "파일이 일반 .md이므로 작성과 내보내기 흐름을 그대로 쓸 수 있습니다."
            ),
            tr!(
                lang,
                "低，打开现有文件夹即可。",
                "Low; just open your existing folder.",
                "低。既存のフォルダーを開くだけです。",
                "낮음. 기존 폴더를 열기만 하면 됩니다."
            ),
            tr!(
                lang,
                "导出模板和引用格式需要按需配置。",
                "Export templates and citation styles need configuring as needed.",
                "書き出しテンプレートと引用形式は必要に応じて設定が必要です。",
                "내보내기 템플릿과 인용 형식은 필요에 따라 설정해야 합니다."
            ),
        ),
        "gitextensions" => (
            "Git Extensions",
            "开源替代",
            "https://github.com/gitextensions/gitextensions",
            tr!(
                lang,
                "Windows 上的图形化 Git 客户端，支持提交、分支和差异查看。",
                "A graphical Git client for Windows with commits, branches and diffs.",
                "コミット、ブランチ、差分表示に対応した Windows 向けのグラフィカル Git クライアントです。",
                "커밋, 브랜치, 차이 보기를 지원하는 Windows용 그래픽 Git 클라이언트입니다."
            ),
            tr!(
                lang,
                "保留图形化操作习惯，仓库数据仍然只是普通 .git 目录。",
                "Keeps graphical habits while the repo stays a plain .git folder.",
                "グラフィカルな操作感を保ちつつ、リポジトリは通常の .git フォルダーのままです。",
                "그래픽 조작 습관을 유지하면서 저장소는 평범한 .git 폴더로 남습니다."
            ),
            tr!(
                lang,
                "低到中，克隆现有仓库即可开始。",
                "Low to medium; clone an existing repo to start.",
                "低〜中。既存のリポジトリをクローンすれば始められます。",
                "낮음~보통. 기존 저장소를 복제하면 시작할 수 있습니다."
            ),
            tr!(
                lang,
                "改写历史的操作仍需谨慎，先确认已推送的分支。",
                "History rewrites still need care; check which branches are already pushed.",
                "履歴を書き換える操作は慎重に。プッシュ済みのブランチを先に確認してください。",
                "기록을 다시 쓰는 작업은 여전히 신중해야 하며, 이미 푸시한 브랜치를 먼저 확인하세요."
            ),
        ),
        "stirlingPdf" => (
            "Stirling PDF",
            "开源替代",
            "https://github.com/Stirling-Tools/Stirling-PDF",
            tr!(
                lang,
                "可自托管的 PDF 合并、拆分、压缩和转换工具。",
                "A self-hostable tool for merging, splitting, compressing and converting PDFs.",
                "セルフホスト可能な PDF の結合、分割、圧縮、変換ツールです。",
                "자가 호스팅 가능한 PDF 병합, 분할, 압축, 변환 도구입니다."
            ),
            tr!(
                lang,
                "把常用的 PDF 处理集中到一个可自托管的页面，不逐次上传到第三方。",
                "Common PDF tasks in one self-hostable page instead of uploading to third parties each time.",
                "よく使う PDF 処理をセルフホスト可能な 1 つの画面にまとめ、毎回サードパーティへ送らずに済みます。",
                "자주 쓰는 PDF 처리를 자가 호스팅 가능한 한 화면에 모아 매번 서드파티에 올리지 않아도 됩니다."
            ),
            tr!(
                lang,
                "高，需要部署服务并维护。",
                "High; the service needs deployment and upkeep.",
                "高。サービスの配備と保守が必要です。",
                "높음. 서비스를 배포하고 유지해야 합니다."
            ),
            tr!(
                lang,
                "文档可能含敏感内容，部署时限制访问并确认不留日志。",
                "Documents may be sensitive; restrict access on deployment and confirm logs aren't retained.",
                "文書に機密内容が含まれることがあります。配備時にアクセスを制限し、ログが残らないことを確認してください。",
                "문서에 민감한 내용이 있을 수 있으므로 배포 시 접근을 제한하고 로그가 남지 않는지 확인하세요."
            ),
        ),
        _ => return None,
    };
    Some(Resource {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: kind_label(lang, kind).to_owned(),
        url: url.to_owned(),
        description: description.to_owned(),
        benefit: benefit.to_owned(),
        setup_cost: setup_cost.to_owned(),
        permissions: permissions.to_owned(),
        source: "curated".to_owned(),
        verification: tr!(
            lang,
            "内置精选，未联网复核",
            "Curated in app, not re-checked online",
            "アプリ内の厳選情報（オンライン再確認なし）",
            "앱 내 엄선 정보, 온라인 재확인 없음"
        )
        .to_owned(),
        verified_at: None,
    })
}

fn resources(ids: &[&str], lang: Lang) -> Vec<Resource> {
    ids.iter().filter_map(|id| resource(id, lang)).collect()
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

/// Display text for the canonical roles [`tool_role`] stores in profiles.
pub fn role_label(lang: Lang, role: &str) -> &str {
    match role {
        "编辑器" => tr!(lang, "编辑器", "Editor", "エディター", "편집기"),
        "终端" => tr!(lang, "终端", "Terminal", "ターミナル", "터미널"),
        "浏览器" => tr!(lang, "浏览器", "Browser", "ブラウザ", "브라우저"),
        "文档与笔记" => tr!(
            lang,
            "文档与笔记",
            "Documents & notes",
            "ドキュメントとノート",
            "문서 및 노트"
        ),
        "通讯" => tr!(
            lang,
            "通讯",
            "Communication",
            "コミュニケーション",
            "커뮤니케이션"
        ),
        "会议" => tr!(lang, "会议", "Meetings", "会議", "회의"),
        "设计" => tr!(lang, "设计", "Design", "デザイン", "디자인"),
        "音视频" => tr!(
            lang,
            "音视频",
            "Audio & video",
            "音声・動画",
            "오디오·비디오"
        ),
        "API 调试" => tr!(
            lang,
            "API 调试",
            "API debugging",
            "API デバッグ",
            "API 디버깅"
        ),
        "数据库" => tr!(lang, "数据库", "Database", "データベース", "데이터베이스"),
        "文件管理" => tr!(
            lang,
            "文件管理",
            "File management",
            "ファイル管理",
            "파일 관리"
        ),
        "远程控制" => tr!(
            lang,
            "远程控制",
            "Remote control",
            "リモート操作",
            "원격 제어"
        ),
        "虚拟化" => tr!(lang, "虚拟化", "Virtualization", "仮想化", "가상화"),
        "压缩" => tr!(lang, "压缩", "Compression", "圧縮", "압축"),
        "截图" => tr!(
            lang,
            "截图",
            "Screenshots",
            "スクリーンショット",
            "스크린샷"
        ),
        "密码管理" => tr!(
            lang,
            "密码管理",
            "Password manager",
            "パスワード管理",
            "비밀번호 관리"
        ),
        "邮件" => tr!(lang, "邮件", "Email", "メール", "이메일"),
        "其他" => tr!(lang, "其他", "Other", "その他", "기타"),
        other => other,
    }
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

/// Display text for the generic fallbacks used when the app name is unknown.
/// Product names (Photoshop, Figma, ...) are canonical and pass through.
fn fallback_label(lang: Lang, value: &str) -> &str {
    match value {
        "API 调试工具" => tr!(
            lang,
            "API 调试工具",
            "API debugging tool",
            "API デバッグツール",
            "API 디버깅 도구"
        ),
        "数据库客户端" => tr!(
            lang,
            "数据库客户端",
            "Database client",
            "データベースクライアント",
            "데이터베이스 클라이언트"
        ),
        "思维导图工具" => tr!(
            lang,
            "思维导图工具",
            "Mind mapping tool",
            "マインドマップツール",
            "마인드맵 도구"
        ),
        "远程控制工具" => tr!(
            lang,
            "远程控制工具",
            "Remote control tool",
            "リモート操作ツール",
            "원격 제어 도구"
        ),
        "压缩工具" => tr!(lang, "压缩工具", "Archive tool", "圧縮ツール", "압축 도구"),
        "SSH 客户端" => tr!(
            lang,
            "SSH 客户端",
            "SSH client",
            "SSH クライアント",
            "SSH 클라이언트"
        ),
        "在线笔记" => tr!(
            lang,
            "在线笔记",
            "Online notes",
            "オンラインノート",
            "온라인 노트"
        ),
        other => other,
    }
}

fn build_alternatives(stays: &[Stay], lang: Lang) -> Vec<Recommendation> {
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
                .unwrap_or_else(|| fallback_label(lang, rule.fallback).to_owned());
            let role = role_label(lang, rule.role);
            let minutes = rounded_minutes(active_ms);
            let entries = used.len();
            opportunity(
                &format!("alternative-{}", rule.procs[0]),
                lang,
                &fill(
                    tr!(
                        lang,
                        "用同类开源工具承接{}",
                        "Replace {} with a similar open-source tool",
                        "同じ用途のオープンソースツールで{}を置き換える",
                        "같은 용도의 오픈 소스 도구로 {} 대체하기"
                    ),
                    &[&label],
                ),
                fill(
                    tr!(
                        lang,
                        "{}（{}）今天活跃 {} 分钟，进入 {} 次。",
                        "{} ({}) was active {} minutes today across {} entries.",
                        "{}（{}）は本日 {} 分稼働し、{} 回開かれました。",
                        "{}({})은 오늘 {}분 활성 상태였고 {}회 열렸습니다."
                    ),
                    &[&label, &role, &minutes, &entries],
                ),
                tr!(
                    lang,
                    "同样的事情换一个本地可运行、无订阅锁定的入口，操作方式基本不变；先在一件真实任务上并行验证。",
                    "Do the same work through a local, subscription-free entry point with familiar interactions; validate it in parallel on one real task first.",
                    "同じ作業をローカルで動かせてサブスクリプションに縛られない入口に置き換えます。操作方法はほぼ変わらないので、まず実際のタスク 1 件で並行して試します。",
                    "같은 작업을 로컬에서 실행되고 구독에 묶이지 않는 진입점으로 옮깁니다. 조작 방식은 거의 그대로이니 실제 작업 하나에서 먼저 병행 검증하세요."
                ),
                &format!("open source alternative to {label}"),
                minutes as f64 / 5.0 + entries as f64,
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
    lang: Lang,
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
        resources: resources(resource_ids, lang),
    }
}

/// Turns today's observed loops into tool opportunities. Returns everything,
/// sorted by friction; the caller decides how many to keep.
pub fn build_recommendations(stats: &DayStats, lang: Lang) -> Vec<Recommendation> {
    let stays = &stats.stays;
    let apps = &stats.apps;
    let mut opportunities = build_alternatives(stays, lang);
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
            lang,
            tr!(
                lang,
                "把开发命令收成固定入口",
                "Turn dev commands into a fixed entry point",
                "開発コマンドを定番の入口にまとめる",
                "개발 명령을 고정된 진입점으로 모으기"
            ),
            fill(
                tr!(
                    lang,
                    "编辑器与终端之间切换 {} 次，相关应用活跃 {} 分钟。",
                    "Switched between editor and terminal {} times, with {} active minutes in related apps.",
                    "エディターとターミナルの切り替えが {} 回、関連アプリの稼働は {} 分です。",
                    "편집기와 터미널 사이를 {}회 전환했고 관련 앱 활동 시간은 {}분입니다."
                ),
                &[&editor_terminal_switches, &rounded_minutes(active_ms)],
            ),
            tr!(
                lang,
                "先固化重复的项目命令；如果切换主要来自 Git 操作，再使用可视化 Git 工具。",
                "Pin down the repeated project commands first; if most switching comes from Git work, add a visual Git tool.",
                "まず繰り返しのプロジェクトコマンドを固定します。切り替えの主因が Git 操作なら、視覚的な Git ツールを追加します。",
                "먼저 반복되는 프로젝트 명령을 고정하세요. 전환의 주된 원인이 Git 작업이라면 시각적 Git 도구를 추가하세요."
            ),
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
            lang,
            tr!(
                lang,
                "让网页内容直接进入笔记流程",
                "Route web content straight into your notes",
                "Web の内容をノートの流れに直接つなげる",
                "웹 콘텐츠를 노트 흐름으로 바로 연결하기"
            ),
            fill(
                tr!(
                    lang,
                    "浏览器与文档或笔记应用之间切换 {} 次，相关应用活跃 {} 分钟。",
                    "Switched between the browser and document or note apps {} times, with {} active minutes in related apps.",
                    "ブラウザとドキュメント／ノートアプリの切り替えが {} 回、関連アプリの稼働は {} 分です。",
                    "브라우저와 문서·노트 앱 사이를 {}회 전환했고 관련 앱 활동 시간은 {}분입니다."
                ),
                &[&browser_document_switches, &rounded_minutes(active_ms)],
            ),
            tr!(
                lang,
                "把网页读取、来源引用和摘要整理做成可复用流程，保留人工核对来源这一步。",
                "Make page reading, source citation and summarizing a reusable flow, keeping the manual source check.",
                "Web の読み込み、出典の引用、要約整理を再利用できる手順にし、出典の目視確認は残します。",
                "웹 읽기, 출처 인용, 요약 정리를 재사용 가능한 흐름으로 만들고 출처를 직접 확인하는 단계는 남겨 두세요."
            ),
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
            lang,
            tr!(
                lang,
                "把会议转写和纪要串成固定流程",
                "Make transcription and meeting notes one fixed flow",
                "会議の文字起こしと議事録を定型的な流れにする",
                "회의 전사와 회의록을 하나의 고정 흐름으로 만들기"
            ),
            fill(
                tr!(
                    lang,
                    "会议相关活动共 {} 分钟。",
                    "Meeting-related activity totaled {} minutes.",
                    "会議関連の稼働は合計 {} 分です。",
                    "회의 관련 활동은 총 {}분입니다."
                ),
                &[&rounded_minutes(meeting_ms)],
            ),
            tr!(
                lang,
                "在参与者同意的前提下先本地转写，再用固定模板生成决议和行动项，并人工复核。",
                "With participants' consent, transcribe locally first, then generate decisions and action items from a fixed template and review them by hand.",
                "参加者の同意を得たうえでローカルで文字起こしし、定型テンプレートから決定事項とアクション項目を作り、最後に人が確認します。",
                "참가자 동의를 받은 뒤 로컬에서 전사하고, 고정 템플릿으로 결정 사항과 실행 항목을 만든 다음 직접 검토하세요."
            ),
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
            lang,
            tr!(
                lang,
                "把重复的文件整理动作交给工具",
                "Hand repetitive file cleanup to a tool",
                "繰り返しのファイル整理をツールに任せる",
                "반복되는 파일 정리 작업을 도구에 맡기기"
            ),
            fill(
                tr!(
                    lang,
                    "文件管理相关窗口活跃 {} 分钟，与其他应用切换 {} 次。",
                    "File manager windows were active {} minutes with {} switches to other apps.",
                    "ファイル管理のウィンドウ稼働は {} 分、他のアプリとの切り替えは {} 回です。",
                    "파일 관리 창 활동 시간은 {}분이고 다른 앱과의 전환은 {}회입니다."
                ),
                &[&rounded_minutes(file_ms), &file_switches],
            ),
            tr!(
                lang,
                "批量重命名先用成熟工具；只有规则明确且反复出现的文件动作才值得写成脚本。",
                "Use a mature tool for batch renaming; only script file actions that recur with clear rules.",
                "一括リネームにはまず成熟したツールを使います。ルールが明確で繰り返すファイル操作だけをスクリプト化します。",
                "일괄 이름 바꾸기에는 먼저 성숙한 도구를 쓰세요. 규칙이 분명하고 반복되는 파일 작업만 스크립트로 만드는 것이 좋습니다."
            ),
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
            fill(
                tr!(
                    lang,
                    "检测到 {} 次 API 或接口相关窗口标题。",
                    "Found {} API-related window titles.",
                    "API やインターフェースに関連するウィンドウタイトルを {} 件検出しました。",
                    "API 또는 인터페이스 관련 창 제목을 {}건 감지했습니다."
                ),
                &[&api_title_hits],
            )
        } else {
            fill(
                tr!(
                    lang,
                    "API 调试工具活跃 {} 分钟。",
                    "API debugging tools were active {} minutes.",
                    "API デバッグツールの稼働は {} 分です。",
                    "API 디버깅 도구 활동 시간은 {}분입니다."
                ),
                &[&rounded_minutes(api_ms)],
            )
        };
        opportunities.push(opportunity(
            "api-workflow",
            lang,
            tr!(
                lang,
                "把请求集合留在本地复用",
                "Keep request collections local and reusable",
                "リクエスト集をローカルに置いて再利用する",
                "요청 컬렉션을 로컬에 두고 재사용하기"
            ),
            evidence,
            tr!(
                lang,
                "把常用请求、环境和示例响应整理成集合并纳入密钥检查，减少重复填写。",
                "Collect common requests, environments and sample responses, add a secret check, and fill things in less often.",
                "よく使うリクエスト、環境、サンプル応答をまとめて鍵の点検を含め、入力の繰り返しを減らします。",
                "자주 쓰는 요청, 환경, 예시 응답을 모아 비밀 정보 점검을 포함하고 반복 입력을 줄입니다."
            ),
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
        let recommendations = build_recommendations(&stats(stays, apps), Lang::Zh);
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

    #[test]
    fn roles_and_resources_follow_the_language() {
        assert_eq!(role_label(Lang::Zh, "编辑器"), "编辑器");
        assert_eq!(role_label(Lang::En, "编辑器"), "Editor");
        assert_eq!(role_label(Lang::Ja, "文档与笔记"), "ドキュメントとノート");
        assert_eq!(role_label(Lang::Ko, "未知角色"), "未知角色");

        let just = resource("just", Lang::En).expect("just is curated");
        assert_eq!(just.name, "just");
        assert_eq!(just.kind, "Open source project");
        assert_eq!(just.url, "https://github.com/casey/just");
        assert!(
            !just
                .description
                .chars()
                .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
        );
        assert_eq!(resource("just", Lang::Zh).unwrap().kind, "开源项目");
    }
}
