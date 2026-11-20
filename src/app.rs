use crate::tr;
use crate::{
    analyzer::{Analyzer, Report, ReportSummary},
    i18n::{Lang, fill},
    input::{self, TextInput},
    intervals::{IntervalAnalyzer, IntervalSummary, SentRequest},
    platform,
    profile::DecisionKind,
    recorder::{Recorder, RecorderStatus},
    settings::{Settings, SettingsData},
    stats::{self, DayPoint},
    store::{DayStats, DayStore, DeriveOptions, hhmm, local_date_key, now_ms},
};
use anyhow::Result;
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, TimeZone};
use gpui::{
    App, Application, Bounds, Context, Div, ElementId, Entity, FocusHandle, FontWeight, KeyBinding,
    SharedString, Stateful, Timer, TitlebarOptions, Window, WindowBounds, WindowOptions, actions,
    div, img, prelude::*, px, relative, rgb, rgba, size,
};
use std::{
    collections::BTreeMap,
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, TryRecvError},
    },
    thread,
    time::Duration,
};
use tray_icon::{
    Icon, MouseButton as TrayMouseButton, MouseButtonState, TrayIcon, TrayIconBuilder,
    TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};

const CANVAS: u32 = 0xf3f5f6;
const SURFACE: u32 = 0xffffff;
const SUBTLE: u32 = 0xe9edf0;
const INK: u32 = 0x202428;
const SECONDARY: u32 = 0x565e65;
const FAINT: u32 = 0x747c83;
const DIVIDER: u32 = 0xd7dde1;
const BRAND_PALE: u32 = 0xf6ecd7;
const ACCENT: u32 = 0xc88918;
const GREEN: u32 = 0x2f7d57;
const RED: u32 = 0xb64b4b;

actions!(
    tracebit,
    [
        ShowToday,
        ShowStats,
        ShowReport,
        ShowSettings,
        ToggleRecording,
        Dismiss,
        CloseWindow,
        FocusNext,
        FocusPrevious
    ]
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Today,
    Stats,
    Report,
    Settings,
}

struct TrayState {
    tray: TrayIcon,
    toggle: MenuItem,
    status: MenuItem,
}

struct FormState {
    base_url: Entity<TextInput>,
    api_key: Entity<TextInput>,
    model: Entity<TextInput>,
    auto_time: Entity<TextInput>,
    digest_time: Entity<TextInput>,
    screenshot_interval: Entity<TextInput>,
    search_key: Entity<TextInput>,
    heartbeat: Entity<TextInput>,
    idle: Entity<TextInput>,
    passive_idle: Entity<TextInput>,
    patterns: Entity<TextInput>,
    cap: Entity<TextInput>,
    retention: Entity<TextInput>,
}

impl FormState {
    fn new(cx: &mut Context<TraceBitWindow>, settings: &SettingsData, lang: Lang) -> Self {
        let input =
            |cx: &mut Context<TraceBitWindow>, value: String, placeholder: &str, masked: bool| {
                cx.new(|cx| TextInput::new(cx, &value, placeholder, masked))
            };
        Self {
            base_url: input(
                cx,
                settings.ai.base_url.clone(),
                "https://api.deepseek.com/v1",
                false,
            ),
            api_key: input(
                cx,
                settings.ai.api_key.clone(),
                tr!(
                    lang,
                    "sk-…，本地模型可留空",
                    "sk-… (leave empty for local)",
                    "sk-…（ローカルなら空欄）",
                    "sk-… (로컬 모델은 비움)"
                ),
                true,
            ),
            model: input(cx, settings.ai.model.clone(), "deepseek-chat", false),
            auto_time: input(cx, settings.ai.auto_report_time.clone(), "21:30", false),
            digest_time: input(cx, settings.ai.daily_digest_time.clone(), "10:30", false),
            screenshot_interval: input(
                cx,
                (settings.ai.screenshot_analysis_interval_ms / 60_000).to_string(),
                "30",
                false,
            ),
            search_key: input(
                cx,
                settings.ai.search_api_key.clone(),
                "Brave Search API Key",
                true,
            ),
            heartbeat: input(
                cx,
                (settings.recording.heartbeat_ms / 60_000).to_string(),
                "5",
                false,
            ),
            idle: input(
                cx,
                (settings.recording.idle_threshold_ms / 60_000).to_string(),
                "1",
                false,
            ),
            passive_idle: input(
                cx,
                (settings.recording.passive_idle_threshold_ms / 60_000).to_string(),
                "15",
                false,
            ),
            patterns: input(
                cx,
                settings.privacy.sensitive_patterns.join(", "),
                "密码, 网银, password",
                false,
            ),
            cap: input(
                cx,
                (settings.storage.max_snapshot_bytes / 1024_u64.pow(3)).to_string(),
                "2",
                false,
            ),
            retention: input(
                cx,
                settings.storage.retention_days.to_string(),
                "180",
                false,
            ),
        }
    }

    fn apply(&self, draft: &mut SettingsData, cx: &App) {
        let value = |input: &Entity<TextInput>| input.read(cx).value();
        draft.ai.base_url = value(&self.base_url).trim().to_owned();
        draft.ai.api_key = value(&self.api_key).trim().to_owned();
        draft.ai.model = value(&self.model).trim().to_owned();
        draft.ai.auto_report_time = {
            let value = value(&self.auto_time);
            if value.len() == 5 && value.as_bytes().get(2) == Some(&b':') {
                value
            } else {
                "21:30".to_owned()
            }
        };
        draft.ai.daily_digest_time = {
            let value = value(&self.digest_time);
            if value.len() == 5 && value.as_bytes().get(2) == Some(&b':') {
                value
            } else {
                "10:30".to_owned()
            }
        };
        draft.ai.screenshot_analysis_interval_ms = value(&self.screenshot_interval)
            .parse::<u64>()
            .unwrap_or(30)
            .clamp(5, 240)
            * 60_000;
        draft.ai.search_api_key = value(&self.search_key).trim().to_owned();
        draft.recording.heartbeat_ms = (value(&self.heartbeat)
            .parse::<f64>()
            .unwrap_or(5.0)
            .clamp(1.0, 60.0)
            * 60_000.0) as u64;
        draft.recording.idle_threshold_ms = (value(&self.idle)
            .parse::<f64>()
            .unwrap_or(1.0)
            .clamp(1.0, 30.0)
            * 60_000.0) as i64;
        draft.recording.passive_idle_threshold_ms = (value(&self.passive_idle)
            .parse::<f64>()
            .unwrap_or(15.0)
            .clamp(1.0, 120.0)
            * 60_000.0) as i64;
        draft.privacy.sensitive_patterns = value(&self.patterns)
            .split([',', '，', '\n'])
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect();
        draft.storage.max_snapshot_bytes = (value(&self.cap)
            .parse::<f64>()
            .unwrap_or(2.0)
            .clamp(1.0, 100.0)
            * 1024_f64.powi(3)) as u64;
        draft.storage.retention_days = value(&self.retention)
            .parse::<u32>()
            .unwrap_or(180)
            .clamp(7, 3650);
    }
}

pub struct TraceBitWindow {
    focus_handle: FocusHandle,
    settings: Arc<Settings>,
    store: Arc<DayStore>,
    recorder: Recorder,
    analyzer: Arc<Analyzer>,
    interval_analyzer: Arc<IntervalAnalyzer>,
    view: View,
    /// UI language mirrored from the saved settings.
    lang: Lang,
    date: NaiveDate,
    day: DayStats,
    /// Day window of the statistics page and its cached roll-up.
    stats_days: u32,
    stats: stats::PeriodStats,
    stats_key: Option<(String, u32)>,
    stats_at: Option<std::time::Instant>,
    report: Option<Report>,
    report_list: Vec<ReportSummary>,
    /// Tool decisions, mirrored from the profile so the buttons show state.
    decisions: BTreeMap<String, bool>,
    interval_summaries: Vec<IntervalSummary>,
    interval_error: Option<String>,
    interval_request: Option<SentRequest>,
    status: RecorderStatus,
    draft: SettingsData,
    form: FormState,
    generating: bool,
    testing: bool,
    report_error: Option<String>,
    notice: Option<(String, bool)>,
    notice_text: String,
    notice_since: Option<std::time::Instant>,
    lightbox: Option<usize>,
    prompt_preview: Option<String>,
    report_rx: Option<Receiver<(String, Result<Report>)>>,
    test_rx: Option<Receiver<Result<(String, String)>>>,
    auto_rx: Receiver<String>,
    interval_rx: Receiver<std::result::Result<String, String>>,
    tray: Option<TrayState>,
    quitting: Arc<AtomicBool>,
}

impl TraceBitWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        settings: Arc<Settings>,
        store: Arc<DayStore>,
        recorder: Recorder,
        analyzer: Arc<Analyzer>,
        interval_analyzer: Arc<IntervalAnalyzer>,
        auto_rx: Receiver<String>,
        interval_rx: Receiver<std::result::Result<String, String>>,
        quitting: Arc<AtomicBool>,
    ) -> Self {
        let current = settings.get();
        let date = Local::now().date_naive();
        let day = load_day(&store, &settings, &local_date_key(now_ms()));
        let report = analyzer.get_report(&local_date_key(now_ms()));
        let report_list = analyzer.list_reports();
        let decisions = analyzer
            .profile()
            .decisions
            .iter()
            .filter_map(|decision| {
                decision
                    .kind
                    .map(|kind| (decision.opportunity.clone(), kind == DecisionKind::Adopted))
            })
            .collect::<BTreeMap<_, _>>();
        let interval_summaries = interval_analyzer.list(&local_date_key(now_ms()));
        let interval_request = interval_analyzer.last_request();
        let status = recorder.status();
        let form = FormState::new(cx, &current, current.app.lang());
        let tray = create_tray(status.running && status.enabled, current.app.lang());
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let mut this = Self {
            focus_handle,
            settings,
            store,
            recorder,
            analyzer,
            interval_analyzer,
            view: View::Today,
            lang: current.app.lang(),
            date,
            day,
            stats_days: 7,
            stats: stats::PeriodStats::default(),
            stats_key: None,
            stats_at: None,
            report,
            report_list,
            decisions,
            interval_summaries,
            interval_error: None,
            interval_request,
            status,
            draft: current,
            form,
            generating: false,
            testing: false,
            report_error: None,
            notice: None,
            notice_text: String::new(),
            notice_since: None,
            lightbox: None,
            prompt_preview: None,
            report_rx: None,
            test_rx: None,
            auto_rx,
            interval_rx,
            tray,
            quitting,
        };
        this.refresh_tray();
        let weak = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                loop {
                    Timer::after(Duration::from_secs(1)).await;
                    if weak
                        .update_in(cx, |this, window, cx| this.tick(window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        this
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.expire_notice();
        self.status = self.recorder.status();
        self.poll_report_task();
        self.poll_test_task();
        self.poll_tray(window, cx);
        while let Ok(date_key) = self.auto_rx.try_recv() {
            self.notice = Some((
                tr!(
                    self.lang,
                    "工具升级建议已自动生成",
                    "Tool upgrade advice generated automatically",
                    "ツール改善の提案を自動生成しました",
                    "도구 개선 제안을 자동 생성했습니다"
                )
                .to_owned(),
                true,
            ));
            if self.date_key() == date_key {
                self.report = self.analyzer.get_report(&date_key);
            }
            self.report_list = self.analyzer.list_reports();
        }
        while let Ok(result) = self.interval_rx.try_recv() {
            match result {
                Ok(date_key) => {
                    self.interval_error = None;
                    self.interval_request = self.interval_analyzer.last_request();
                    if self.date_key() == date_key {
                        self.interval_summaries = self.interval_analyzer.list(&date_key);
                    }
                }
                Err(error) => self.interval_error = Some(error),
            }
        }
        if self.view == View::Today
            && self.date == Local::now().date_naive()
            && now_ms() % 30_000 < 1_100
        {
            self.reload_day();
        }
        self.refresh_tray();
        cx.notify();
    }

    /// Toasts float above every page and dismiss themselves, instead of
    /// appearing inline at the top of a page that may be scrolled away.
    fn expire_notice(&mut self) {
        let current = self
            .notice
            .as_ref()
            .map(|(message, _)| message.clone())
            .unwrap_or_default();
        if current != self.notice_text {
            self.notice_text = current;
            self.notice_since = self.notice.as_ref().map(|_| std::time::Instant::now());
        }
        if self
            .notice_since
            .is_some_and(|since| since.elapsed() >= Duration::from_secs(8))
        {
            self.notice = None;
            self.notice_since = None;
            self.notice_text.clear();
        }
    }

    fn poll_report_task(&mut self) {
        let result = match self.report_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(result)) => Some(result),
            Some(Err(TryRecvError::Disconnected)) => {
                self.report_rx = None;
                self.generating = false;
                Some((
                    self.date_key(),
                    Err(anyhow::anyhow!(
                        "{}",
                        tr!(
                            self.lang,
                            "建议任务意外结束",
                            "the advice task ended unexpectedly",
                            "提案タスクが予期せず終了しました",
                            "제안 작업이 예기치 않게 종료되었습니다"
                        )
                    )),
                ))
            }
            _ => None,
        };
        if let Some((date_key, result)) = result {
            self.report_rx = None;
            self.generating = false;
            match result {
                Ok(report) => {
                    self.report = Some(report);
                    self.notice = Some((
                        tr!(
                            self.lang,
                            "工具升级建议已生成",
                            "Tool upgrade advice generated",
                            "ツール改善の提案を生成しました",
                            "도구 개선 제안을 생성했습니다"
                        )
                        .to_owned(),
                        true,
                    ));
                    self.report_list = self.analyzer.list_reports();
                }
                Err(error) => {
                    self.report_error = Some(format!("{error:#}"));
                    self.notice = Some((
                        fill(
                            tr!(
                                self.lang,
                                "生成失败：{}",
                                "Generation failed: {}",
                                "生成に失敗しました：{}",
                                "생성 실패: {}"
                            ),
                            &[&format!("{error:#}")],
                        ),
                        false,
                    ));
                }
            }
            if self.date_key() != date_key {
                self.report = self.analyzer.get_report(&self.date_key());
            }
        }
    }

    fn poll_test_task(&mut self) {
        let result = match self.test_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(result)) => Some(result),
            Some(Err(TryRecvError::Disconnected)) => Some(Err(anyhow::anyhow!(
                "{}",
                tr!(
                    self.lang,
                    "连接测试意外结束",
                    "the connection test ended unexpectedly",
                    "接続テストが予期せず終了しました",
                    "연결 테스트가 예기치 않게 종료되었습니다"
                )
            ))),
            _ => None,
        };
        if let Some(result) = result {
            self.test_rx = None;
            self.testing = false;
            match result {
                Ok((_, model)) => {
                    self.notice = Some((
                        fill(
                            tr!(
                                self.lang,
                                "连接正常：{}",
                                "Connection OK: {}",
                                "接続正常：{}",
                                "연결 정상: {}"
                            ),
                            &[&model],
                        ),
                        true,
                    ))
                }
                Err(error) => {
                    self.notice = Some((
                        fill(
                            tr!(
                                self.lang,
                                "连接失败：{}",
                                "Connection failed: {}",
                                "接続に失敗しました：{}",
                                "연결 실패: {}"
                            ),
                            &[&format!("{error:#}")],
                        ),
                        false,
                    ))
                }
            }
        }
    }

    fn poll_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: TrayMouseButton::Left,
                    button_state: MouseButtonState::Down,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: TrayMouseButton::Left,
                    ..
                }
            ) {
                platform::set_window_visible(window, true);
                window.activate_window();
                cx.activate(true);
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            match event.id.0.as_str() {
                "show" => {
                    platform::set_window_visible(window, true);
                    window.activate_window();
                    cx.activate(true);
                }
                "toggle" => self.toggle_recording(),
                "report" => {
                    platform::set_window_visible(window, true);
                    window.activate_window();
                    self.view = View::Report;
                    self.reload_selected();
                }
                "quit" => {
                    self.quitting.store(true, Ordering::Release);
                    self.recorder.stop();
                    cx.quit();
                }
                _ => {}
            }
        }
    }

    fn refresh_tray(&mut self) {
        let lang = self.lang;
        if let Some(tray) = &self.tray {
            let active = self.status.enabled && self.status.running;
            let _ = tray.tray.set_tooltip(Some(if active {
                tr!(
                    lang,
                    "TraceBit — 正在记录",
                    "TraceBit — Recording",
                    "TraceBit — 記録中",
                    "TraceBit — 기록 중"
                )
            } else {
                tr!(
                    lang,
                    "TraceBit — 已暂停",
                    "TraceBit — Paused",
                    "TraceBit — 一時停止中",
                    "TraceBit — 일시정지됨"
                )
            }));
            tray.toggle.set_text(if active {
                tr!(
                    lang,
                    "暂停记录",
                    "Pause recording",
                    "記録を一時停止",
                    "기록 일시정지"
                )
            } else {
                tr!(
                    lang,
                    "恢复记录",
                    "Resume recording",
                    "記録を再開",
                    "기록 재개"
                )
            });
            tray.status.set_text(if active {
                tr!(lang, "正在记录", "Recording", "記録中", "기록 중")
            } else {
                tr!(lang, "已暂停", "Paused", "一時停止中", "일시정지됨")
            });
        }
    }

    /// The tray menu is built once, so a language change rebuilds it.
    fn rebuild_tray(&mut self) {
        let active = self.status.enabled && self.status.running;
        self.tray = None;
        self.tray = create_tray(active, self.lang);
        self.refresh_tray();
    }

    fn date_key(&self) -> String {
        self.date.format("%Y-%m-%d").to_string()
    }

    fn reload_day(&mut self) {
        self.day = load_day(&self.store, &self.settings, &self.date_key());
    }

    fn reload_selected(&mut self) {
        self.reload_day();
        self.report = self.analyzer.get_report(&self.date_key());
        self.interval_summaries = self.interval_analyzer.list(&self.date_key());
        self.interval_request = self.interval_analyzer.last_request();
        self.report_error = None;
        self.lightbox = None;
        self.prompt_preview = None;
    }

    fn toggle_prompt_preview(&mut self) {
        if self.prompt_preview.take().is_some() {
            return;
        }
        match self.analyzer.preview_prompt(&self.date_key()) {
            Ok(prompt) => self.prompt_preview = Some(prompt),
            Err(error) => {
                self.notice = Some((
                    fill(
                        tr!(
                            self.lang,
                            "无法生成预览：{}",
                            "Couldn't build the preview: {}",
                            "プレビューを作成できません：{}",
                            "미리보기를 만들 수 없습니다: {}"
                        ),
                        &[&format!("{error:#}")],
                    ),
                    false,
                ))
            }
        }
    }

    fn shift_day(&mut self, days: i64) {
        let next = self.date + ChronoDuration::days(days);
        if next <= Local::now().date_naive() {
            self.date = next;
            self.reload_selected();
        }
    }

    fn ensure_stats(&mut self) {
        let today = Local::now().date_naive();
        let key = (today.format("%Y-%m-%d").to_string(), self.stats_days);
        let stale = self
            .stats_at
            .is_none_or(|at| at.elapsed() >= Duration::from_secs(30));
        if self.stats_key.as_ref() == Some(&key) && !stale {
            return;
        }
        let recording = self.settings.get().recording;
        self.stats = stats::load(
            &self.store,
            DeriveOptions::from_settings(&recording),
            today,
            self.stats_days,
        );
        self.stats_key = Some(key);
        self.stats_at = Some(std::time::Instant::now());
    }

    fn open_today(&mut self, _: &ShowToday, _: &mut Window, cx: &mut Context<Self>) {
        self.view = View::Today;
        self.reload_selected();
        cx.notify();
    }

    fn open_stats(&mut self, _: &ShowStats, _: &mut Window, cx: &mut Context<Self>) {
        self.view = View::Stats;
        self.ensure_stats();
        cx.notify();
    }

    fn open_report(&mut self, _: &ShowReport, _: &mut Window, cx: &mut Context<Self>) {
        self.view = View::Report;
        self.reload_selected();
        cx.notify();
    }

    fn open_settings(&mut self, _: &ShowSettings, _: &mut Window, cx: &mut Context<Self>) {
        self.view = View::Settings;
        cx.notify();
    }

    fn toggle_recording_from_key(
        &mut self,
        _: &ToggleRecording,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_recording();
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if self.lightbox.take().is_some() || self.notice.take().is_some() {
            cx.notify();
        }
    }

    fn close_window(&mut self, _: &CloseWindow, window: &mut Window, _: &mut Context<Self>) {
        if self.settings.get().app.close_to_tray {
            platform::set_window_visible(window, false);
        } else {
            self.recorder.stop();
            self.quitting.store(true, Ordering::Release);
            window.remove_window();
        }
    }

    fn focus_next(&mut self, _: &FocusNext, window: &mut Window, _: &mut Context<Self>) {
        window.focus_next();
    }

    fn focus_previous(&mut self, _: &FocusPrevious, window: &mut Window, _: &mut Context<Self>) {
        window.focus_prev();
    }

    fn toggle_recording(&mut self) {
        match self.recorder.toggle() {
            Ok(status) => {
                self.status = status;
                self.draft.recording.enabled = self.status.enabled;
                self.notice = Some((
                    if self.status.running {
                        tr!(
                            self.lang,
                            "记录已开启",
                            "Recording started",
                            "記録を開始しました",
                            "기록을 시작했습니다"
                        )
                    } else {
                        tr!(
                            self.lang,
                            "记录已暂停",
                            "Recording paused",
                            "記録を一時停止しました",
                            "기록을 일시정지했습니다"
                        )
                    }
                    .to_owned(),
                    true,
                ));
            }
            Err(error) => {
                self.notice = Some((
                    fill(
                        tr!(
                            self.lang,
                            "操作失败：{}",
                            "Action failed: {}",
                            "操作に失敗しました：{}",
                            "작업 실패: {}"
                        ),
                        &[&format!("{error:#}")],
                    ),
                    false,
                ))
            }
        }
        self.refresh_tray();
    }

    fn generate_report(&mut self) {
        if self.generating {
            return;
        }
        if !has_ai(&self.settings.get()) {
            self.view = View::Report;
            self.notice = Some((
                tr!(
                    self.lang,
                    "请先配置 AI 接口地址和模型名",
                    "Set the AI endpoint and model first",
                    "先に AI の接続先とモデル名を設定してください",
                    "먼저 AI 주소와 모델명을 설정하세요"
                )
                .to_owned(),
                false,
            ));
            return;
        }
        if self.day.events == 0 {
            self.notice = Some((
                tr!(
                    self.lang,
                    "这一天还没有可分析的记录",
                    "No activity to analyze on this day",
                    "この日は分析できる記録がありません",
                    "이 날은 분석할 기록이 없습니다"
                )
                .to_owned(),
                false,
            ));
            return;
        }
        let date_key = self.date_key();
        let analyzer = self.analyzer.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let result = analyzer.generate_report(&date_key);
            let _ = tx.send((date_key, result));
        });
        self.report_rx = Some(rx);
        self.generating = true;
        self.report_error = None;
        self.prompt_preview = None;
    }

    fn save_settings_with_cx(&mut self, cx: &App) -> bool {
        let mut draft = self.draft.clone();
        self.form.apply(&mut draft, cx);
        match self.settings.replace(draft.clone()) {
            Ok(next) => {
                let previous_lang = self.lang;
                self.lang = next.lang();
                if self.lang != previous_lang {
                    self.rebuild_tray();
                }
                self.draft = next;
                if let Err(error) = platform::set_autostart(self.draft.app.autostart) {
                    self.notice = Some((
                        fill(
                            tr!(
                                self.lang,
                                "开机自启设置失败：{}",
                                "Couldn't update autostart: {}",
                                "自動起動を設定できませんでした：{}",
                                "자동 시작 설정 실패: {}"
                            ),
                            &[&format!("{error}")],
                        ),
                        false,
                    ));
                } else {
                    self.notice = Some((
                        tr!(
                            self.lang,
                            "设置已保存",
                            "Settings saved",
                            "設定を保存しました",
                            "설정을 저장했습니다"
                        )
                        .to_owned(),
                        true,
                    ));
                }
                self.interval_error = None;
                if self.draft.recording.enabled {
                    self.recorder.restart();
                } else {
                    self.recorder.stop();
                }
                self.status = self.recorder.status();
                true
            }
            Err(error) => {
                self.notice = Some((
                    fill(
                        tr!(
                            self.lang,
                            "保存失败：{}",
                            "Save failed: {}",
                            "保存に失敗しました：{}",
                            "저장 실패: {}"
                        ),
                        &[&format!("{error:#}")],
                    ),
                    false,
                ));
                false
            }
        }
    }

    fn test_ai(&mut self, cx: &App) {
        if self.testing || !self.save_settings_with_cx(cx) {
            return;
        }
        let analyzer = self.analyzer.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(analyzer.test_connection());
        });
        self.test_rx = Some(rx);
        self.testing = true;
        self.notice = Some((
            tr!(
                self.lang,
                "正在测试 AI 连接…",
                "Testing the AI connection…",
                "AI 接続をテストしています…",
                "AI 연결을 테스트하는 중…"
            )
            .to_owned(),
            true,
        ));
    }

    fn purge(&mut self, window: &Window, screenshots_only: bool) {
        let (title, message) = if screenshots_only {
            (
                tr!(
                    self.lang,
                    "清空屏幕快照",
                    "Clear snapshots",
                    "スクリーンショットを消去",
                    "스냅샷 비우기"
                ),
                tr!(
                    self.lang,
                    "将删除所有本地屏幕快照，活动记录会保留。此操作无法撤销。",
                    "Deletes all local screenshots; activity records are kept. This can't be undone.",
                    "ローカルのスクリーンショットをすべて削除します。活動記録は残ります。この操作は取り消せません。",
                    "모든 로컬 스냅샷을 삭제합니다. 활동 기록은 유지됩니다. 이 작업은 되돌릴 수 없습니다."
                ),
            )
        } else {
            (
                tr!(
                    self.lang,
                    "清空全部记录",
                    "Clear all records",
                    "すべての記録を消去",
                    "모든 기록 비우기"
                ),
                tr!(
                    self.lang,
                    "将删除所有本地活动记录和屏幕快照。此操作无法撤销。",
                    "Deletes all local activity records and screenshots. This can't be undone.",
                    "ローカルの活動記録とスクリーンショットをすべて削除します。この操作は取り消せません。",
                    "모든 로컬 활동 기록과 스냅샷을 삭제합니다. 이 작업은 되돌릴 수 없습니다."
                ),
            )
        };
        if !platform::confirm_destructive(window, title, message) {
            return;
        }
        let result = if screenshots_only {
            self.store.purge_screenshots()
        } else {
            self.store
                .purge_all()
                .and_then(|()| self.interval_analyzer.purge_all())
        };
        match result {
            Ok(()) => {
                self.notice = Some((
                    if screenshots_only {
                        tr!(
                            self.lang,
                            "快照已清空",
                            "Snapshots cleared",
                            "スクリーンショットを消去しました",
                            "스냅샷을 비웠습니다"
                        )
                    } else {
                        tr!(
                            self.lang,
                            "全部记录已清空",
                            "All records cleared",
                            "すべての記録を消去しました",
                            "모든 기록을 비웠습니다"
                        )
                    }
                    .to_owned(),
                    true,
                ));
                self.reload_selected();
                self.status = self.recorder.status();
            }
            Err(error) => {
                self.notice = Some((
                    fill(
                        tr!(
                            self.lang,
                            "清理失败：{}",
                            "Cleanup failed: {}",
                            "削除に失敗しました：{}",
                            "정리 실패: {}"
                        ),
                        &[&format!("{error}")],
                    ),
                    false,
                ))
            }
        }
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> Div {
        let lang = self.lang;
        let active = self.status.running && self.status.enabled;
        let status_title = if self.status.locked {
            tr!(
                lang,
                "锁屏暂停",
                "Paused while locked",
                "ロック中は一時停止",
                "잠금 중 일시정지"
            )
        } else if self.status.idle && active {
            tr!(lang, "空闲中", "Idle", "アイドル", "유휴")
        } else if active {
            tr!(lang, "正在记录", "Recording", "記録中", "기록 중")
        } else {
            tr!(lang, "记录已暂停", "Paused", "一時停止中", "일시정지됨")
        };
        let current_app = self
            .status
            .frontmost
            .as_ref()
            .map(|sample| sample.app.clone())
            .unwrap_or_else(|| {
                tr!(
                    lang,
                    "正常使用电脑即可自动记录",
                    "Recording starts as you use your PC",
                    "通常どおり PC を使うだけで自動記録します",
                    "PC를 평소처럼 사용하면 자동으로 기록됩니다"
                )
                .to_owned()
            });
        div()
            .flex()
            .flex_col()
            .w(px(184.))
            .h_full()
            .p_4()
            .gap_4()
            .border_r_1()
            .border_color(rgb(DIVIDER))
            .bg(rgb(SURFACE))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .child("TraceBit"),
                    )
                    .child(div().text_xs().text_color(rgb(SECONDARY)).child(tr!(
                        lang,
                        "Windows 活动记录与效率建议",
                        "Windows activity & advice",
                        "Windows の活動記録と提案",
                        "Windows 활동 기록과 제안"
                    ))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        nav_button(
                            "nav-today",
                            tr!(lang, "今日活动", "Today", "今日の活動", "오늘 활동"),
                            self.view == View::Today,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.view = View::Today;
                            this.reload_selected();
                            cx.notify();
                        })),
                    )
                    .child(
                        nav_button(
                            "nav-stats",
                            tr!(lang, "最近统计", "Recent stats", "最近の統計", "최근 통계"),
                            self.view == View::Stats,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.view = View::Stats;
                            this.ensure_stats();
                            cx.notify();
                        })),
                    )
                    .child(
                        nav_button(
                            "nav-report",
                            tr!(lang, "AI 建议", "AI advice", "AI 提案", "AI 제안"),
                            self.view == View::Report,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.view = View::Report;
                            this.reload_selected();
                            cx.notify();
                        })),
                    )
                    .child(
                        nav_button(
                            "nav-settings",
                            tr!(lang, "设置", "Settings", "設定", "설정"),
                            self.view == View::Settings,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.view = View::Settings;
                            cx.notify();
                        })),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .rounded_lg()
                    .bg(rgb(CANVAS))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size_2().rounded_full().bg(rgb(if active {
                                GREEN
                            } else {
                                FAINT
                            })))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(status_title),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(SECONDARY))
                            .child(current_app),
                    )
                    .child(
                        button(
                            "record-toggle",
                            if active {
                                tr!(
                                    lang,
                                    "暂停记录",
                                    "Pause recording",
                                    "記録を一時停止",
                                    "기록 일시정지"
                                )
                            } else {
                                tr!(
                                    lang,
                                    "开始记录",
                                    "Start recording",
                                    "記録を開始",
                                    "기록 시작"
                                )
                            },
                            !active,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_recording();
                            cx.notify();
                        })),
                    ),
            )
            .child(div().text_xs().text_color(rgb(FAINT)).child(tr!(
                lang,
                "数据只保存到这台电脑",
                "Data stays on this PC",
                "データはこの PC にのみ保存されます",
                "데이터는 이 PC에만 저장됩니다"
            )))
    }

    fn render_page_head(
        &self,
        title: &str,
        subtitle: &str,
        with_date: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut head = div().flex().items_start().justify_between().gap_4().child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.to_owned()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(SECONDARY))
                        .child(subtitle.to_owned()),
                ),
        );
        if with_date {
            head = head.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(small_button("date-prev", "‹", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.shift_day(-1);
                            cx.notify();
                        },
                    )))
                    .child(div().text_sm().child(self.date_key()))
                    .child(small_button("date-next", "›", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.shift_day(1);
                            cx.notify();
                        },
                    )))
                    .child(
                        small_button(
                            "date-today",
                            tr!(self.lang, "今天", "Today", "今日", "오늘"),
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.date = Local::now().date_naive();
                            this.reload_selected();
                            cx.notify();
                        })),
                    ),
            );
        }
        head
    }

    fn render_toast(&self, cx: &mut Context<Self>) -> Option<Div> {
        self.notice.as_ref().map(|(message, success)| {
            div()
                .absolute()
                .bottom(px(24.))
                .right(px(24.))
                .w(px(400.))
                .p_4()
                .rounded_lg()
                .shadow_lg()
                .bg(rgb(SURFACE))
                .border_1()
                .border_color(rgb(if *success { GREEN } else { RED }))
                .flex()
                .items_start()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_sm()
                        .text_color(rgb(if *success { INK } else { RED }))
                        .child(message.clone()),
                )
                .child(
                    small_button(
                        "toast-close",
                        tr!(self.lang, "关闭", "Close", "閉じる", "닫기"),
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    })),
                )
        })
    }

    fn render_today(&mut self, cx: &mut Context<Self>) -> Div {
        let lang = self.lang;
        let page = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.))
            .gap_5()
            .child(self.render_page_head(
                if self.date == Local::now().date_naive() {
                    tr!(self.lang, "今天", "Today", "今日", "오늘")
                } else {
                    tr!(
                        self.lang,
                        "每日活动",
                        "Daily activity",
                        "日別の活動",
                        "일별 활동"
                    )
                },
                &full_date(self.lang, self.date),
                true,
                cx,
            ))
            .child(self.render_status_strip());
        if self.day.events == 0 {
            return page.child(
                panel()
                    .items_center()
                    .gap_3()
                    .py_10()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(
                        if self.status.running {
                            tr!(
                                lang,
                                "记录已经开启",
                                "Recording is on",
                                "記録は有効です",
                                "기록이 켜져 있습니다"
                            )
                        } else {
                            tr!(
                                lang,
                                "这一天还没有活动记录",
                                "No activity recorded on this day",
                                "この日の活動記録はまだありません",
                                "이 날의 활동 기록이 없습니다"
                            )
                        },
                    ))
                    .child(div().text_sm().text_color(rgb(SECONDARY)).child(
                        if self.status.running {
                            tr!(
                                lang,
                                "切换应用或窗口后，时间线和快照会自动出现在这里。",
                                "Switch apps or windows and the timeline and snapshots appear here.",
                                "アプリやウィンドウを切り替えると、タイムラインとスナップショットがここに表示されます。",
                                "앱이나 창을 전환하면 타임라인과 스냅샷이 여기에 표시됩니다."
                            )
                        } else {
                            tr!(
                                lang,
                                "开启后，TraceBit 会在本机记录前台应用和去重后的屏幕快照。",
                                "When on, TraceBit records foreground apps and deduplicated screenshots locally.",
                                "有効にすると、TraceBit が前面のアプリと重複を除いたスクリーンショットをこの PC に記録します。",
                                "켜면 TraceBit가 전면 앱과 중복을 제거한 스냅샷을 이 PC에 기록합니다."
                            )
                        },
                    ))
                    .children((!self.status.running).then(|| {
                        button(
                            "empty-toggle",
                            tr!(lang, "开始记录", "Start recording", "記録を開始", "기록 시작"),
                            true,
                        )
                        .on_click(cx.listener(
                            |this, _, _, cx| {
                                this.toggle_recording();
                                cx.notify();
                            },
                        ))
                    })),
            );
        }
        page.child(self.render_timeline())
            .child(
                div()
                    .flex()
                    .w_full()
                    .min_w(px(0.))
                    .gap_5()
                    .items_start()
                    .child(self.render_apps().flex_1().min_w(px(0.)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_5()
                            .w(px(330.))
                            .child(self.render_categories())
                            .child(self.render_ai_card(cx)),
                    ),
            )
            .children(
                (self.draft.ai.screenshot_analysis || !self.interval_summaries.is_empty())
                    .then(|| self.render_interval_summaries()),
            )
            .children(
                (self.draft.recording.browser_sites || !self.day.sites.is_empty())
                    .then(|| self.render_sites()),
            )
            .child(self.render_snapshots(cx))
    }

    fn render_status_strip(&self) -> Div {
        let lang = self.lang;
        let active_hours = self.day.total_active_ms as f64 / 3_600_000.0;
        let switch_note = if active_hours > 0.2 {
            fill(
                tr!(
                    lang,
                    "{} 次/小时",
                    "{}/hour",
                    "1時間あたり{}回",
                    "시간당 {}회"
                ),
                &[&format!("{:.1}", self.day.switches as f64 / active_hours)],
            )
        } else {
            fill(
                tr!(
                    lang,
                    "{} 次切换",
                    "{} switches",
                    "{} 回の切り替え",
                    "전환 {}회"
                ),
                &[&self.day.switches],
            )
        };
        div()
            .flex()
            .w_full()
            .py_4()
            .border_b_1()
            .border_color(rgb(DIVIDER))
            .child(metric(
                tr!(
                    lang,
                    "活跃时长",
                    "Active time",
                    "アクティブ時間",
                    "활동 시간"
                ),
                &fmt_ms(lang, self.day.total_active_ms),
                &self
                    .day
                    .first_active
                    .map(|ts| {
                        fill(
                            tr!(lang, "{} 开始", "from {}", "{} から", "{}부터"),
                            &[&hhmm(ts)],
                        )
                    })
                    .unwrap_or_else(|| {
                        tr!(lang, "暂无活动", "No activity", "活動なし", "활동 없음").to_owned()
                    }),
            ))
            .child(metric(
                tr!(lang, "应用切换", "App switches", "アプリ切替", "앱 전환"),
                &self.day.switches.to_string(),
                &switch_note,
            ))
            .child(metric(
                tr!(lang, "最长专注", "Longest focus", "最長の集中", "최장 집중"),
                &self
                    .day
                    .longest_block
                    .as_ref()
                    .map(|block| fmt_ms(lang, block.ms))
                    .unwrap_or_else(|| tr!(lang, "暂无", "None", "なし", "없음").to_owned()),
                &self
                    .day
                    .longest_block
                    .as_ref()
                    .map(|block| block.app.clone())
                    .unwrap_or_else(|| {
                        tr!(lang, "不足 1 分钟", "Under 1 min", "1 分未満", "1분 미만").to_owned()
                    }),
            ))
            .child(metric(
                tr!(
                    lang,
                    "本地快照",
                    "Local snapshots",
                    "ローカル撮影",
                    "로컬 스냅샷"
                ),
                &self.day.screenshots.len().to_string(),
                &fmt_bytes(self.day.snapshot_bytes),
            ))
    }

    fn render_timeline(&self) -> Div {
        let lang = self.lang;
        let mut track = div()
            .relative()
            .h(px(44.))
            .w_full()
            .rounded_md()
            .bg(rgb(SUBTLE))
            .overflow_hidden();
        let Some(day_start) = self
            .date
            .and_hms_opt(0, 0, 0)
            .and_then(|date| Local.from_local_datetime(&date).single())
            .map(|date| date.timestamp_millis())
        else {
            return section().child(tr!(
                lang,
                "时间线不可用",
                "Timeline unavailable",
                "タイムラインを表示できません",
                "타임라인을 표시할 수 없습니다"
            ));
        };
        let day_end = day_start + 86_400_000;
        for stay in &self.day.stays {
            let start =
                ((stay.start.max(day_start) - day_start) as f32 / 86_400_000.0).clamp(0.0, 1.0);
            let end = ((stay.end.min(day_end) - day_start) as f32 / 86_400_000.0).clamp(start, 1.0);
            if end <= start {
                continue;
            }
            let tooltip = format!(
                "{} – {}  {}  {}",
                hhmm(stay.start),
                hhmm(stay.end),
                stay.app,
                crate::redact::display_title(lang, &stay.title)
            );
            track = track.child(
                div()
                    .id(SharedString::from(format!("stay-{}", stay.start)))
                    .absolute()
                    .top_0()
                    .h_full()
                    .left(relative(start))
                    .w(relative((end - start).max(0.002)))
                    .bg(rgb(category_color(stay.category())))
                    .tooltip(move |_, cx| cx.new(|_| TextTooltip(tooltip.clone())).into()),
            );
        }
        section()
            .gap_3()
            .child(panel_head(
                tr!(
                    lang,
                    "24 小时时间线",
                    "24-hour timeline",
                    "24 時間のタイムライン",
                    "24시간 타임라인"
                ),
                &fill(
                    tr!(
                        lang,
                        "{} 段活动 · {} 次长休息",
                        "{} segments · {} long breaks",
                        "{} セグメント · {} 回の長い休憩",
                        "{}개 구간 · 장시간 휴식 {}회"
                    ),
                    &[&self.day.sessions.len(), &self.day.long_idles],
                ),
            ))
            .child(track)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(rgb(FAINT))
                    .children((0..=8).map(|hour| {
                        fill(tr!(lang, "{}时", "{}h", "{}時", "{}시"), &[&(hour * 3)])
                    })),
            )
    }

    fn render_apps(&self) -> Div {
        let lang = self.lang;
        let max_ms = self
            .day
            .apps
            .iter()
            .map(|app| app.ms)
            .max()
            .unwrap_or(1)
            .max(1);
        let mut apps = self.day.apps.iter().collect::<Vec<_>>();
        apps.sort_by_key(|app| std::cmp::Reverse(app.ms));
        section()
            .gap_3()
            .child(panel_head(
                tr!(lang, "应用使用", "App usage", "アプリの使用", "앱 사용"),
                tr!(
                    lang,
                    "按活跃时长排序",
                    "By active time",
                    "アクティブ時間順",
                    "활동 시간순"
                ),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .children(apps.into_iter().take(12).map(|app| {
                        let width = app.ms as f32 / max_ms as f32;
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(DIVIDER))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(div().flex_1().min_w(px(0.)).truncate().text_sm().child(
                                        if app.app.is_empty() {
                                            app.name.clone()
                                        } else {
                                            app.app.clone()
                                        },
                                    ))
                                    .child(
                                        div()
                                            .w(px(72.))
                                            .text_xs()
                                            .text_color(rgb(category_color(&app.category)))
                                            .child(
                                                crate::categories::label(lang, &app.category)
                                                    .to_owned(),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w(px(72.))
                                            .text_sm()
                                            .text_right()
                                            .child(fmt_ms(lang, app.ms)),
                                    )
                                    .child(
                                        div()
                                            .w(px(44.))
                                            .text_xs()
                                            .text_color(rgb(SECONDARY))
                                            .text_right()
                                            .child(fill(
                                                tr!(
                                                    lang,
                                                    "{} 切换",
                                                    "{} switches",
                                                    "{} 回",
                                                    "{}회"
                                                ),
                                                &[&app.switches],
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .h(px(3.))
                                    .w_full()
                                    .bg(rgb(SUBTLE))
                                    .rounded_full()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(width))
                                            .rounded_full()
                                            .bg(rgb(category_color(&app.category))),
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .truncate()
                                    .text_xs()
                                    .text_color(rgb(FAINT))
                                    .child(
                                        app.top_titles(2)
                                            .into_iter()
                                            .map(|(title, _)| {
                                                crate::redact::display_title(lang, title)
                                            })
                                            .collect::<Vec<_>>()
                                            .join(" · "),
                                    ),
                            )
                    })),
            )
    }

    fn render_categories(&self) -> Div {
        let lang = self.lang;
        let totals = self
            .day
            .categories
            .iter()
            .map(|category| (category.name.clone(), category.ms))
            .collect::<Vec<_>>();
        section()
            .gap_3()
            .child(panel_head(
                tr!(
                    lang,
                    "时间构成",
                    "Time by category",
                    "時間の内訳",
                    "시간 구성"
                ),
                tr!(
                    lang,
                    "浏览器时间按网站归类",
                    "Browser time grouped by site",
                    "ブラウザの時間はサイト別",
                    "브라우저 시간은 사이트별"
                ),
            ))
            .children(totals.into_iter().map(|(category, ms)| {
                let fraction = ms as f32 / self.day.total_active_ms.max(1) as f32;
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_sm()
                            .child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .size_2()
                                            .rounded_full()
                                            .bg(rgb(category_color(&category))),
                                    )
                                    .child(crate::categories::label(lang, &category).to_owned()),
                            )
                            .child(fill(
                                tr!(lang, "{} · {}%", "{} · {}%", "{} · {}%", "{} · {}%"),
                                &[&fmt_ms(lang, ms), &((fraction * 100.0).round() as i32)],
                            )),
                    )
                    .child(
                        div().h(px(4.)).bg(rgb(SUBTLE)).rounded_full().child(
                            div()
                                .h_full()
                                .w(relative(fraction))
                                .rounded_full()
                                .bg(rgb(category_color(&category))),
                        ),
                    )
            }))
    }

    fn render_ai_card(&mut self, cx: &mut Context<Self>) -> Div {
        let lang = self.lang;
        let configured = has_ai(&self.settings.get());
        section()
            .gap_3()
            .child(panel_head(
                tr!(
                    lang,
                    "AI 工具升级建议",
                    "AI tool upgrades",
                    "AI ツール改善",
                    "AI 도구 개선"
                ),
                if self.report.is_some() {
                    tr!(lang, "已生成", "Generated", "生成済み", "생성됨")
                } else {
                    ""
                },
            ))
            .child(div().text_sm().text_color(rgb(SECONDARY)).child(
                if let Some(report) = &self.report {
                    report
                        .advice
                        .as_ref()
                        .map(|advice| advice.headline.clone())
                        .filter(|headline| !headline.is_empty())
                        .unwrap_or_else(|| report_excerpt(&report.markdown))
                } else if configured {
                    tr!(
                        lang,
                        "根据你实际的工作方式，找能让现有动作更省力的工具、插件和开源替代。",
                        "Based on how you actually work, finds tools, extensions and open-source alternatives that make current actions easier.",
                        "実際の仕事の進め方に合わせ、今の操作を楽にするツール・拡張・OSS 代替を探します。",
                        "실제 업무 방식에 맞춰 지금 하는 일을 더 쉽게 만드는 도구, 확장, 오픈소스 대안을 찾습니다."
                    )
                    .to_owned()
                } else {
                    tr!(
                        lang,
                        "连接模型后即可生成建议；截图分析可在设置中单独开启。",
                        "Connect a model to generate advice; screenshot analysis can be enabled separately in Settings.",
                        "モデルに接続すると提案を生成できます。スクリーンショット分析は設定で別途有効にできます。",
                        "모델을 연결하면 제안을 생성할 수 있습니다. 스냅샷 분석은 설정에서 따로 켤 수 있습니다."
                    )
                    .to_owned()
                },
            ))
            .child(
                button(
                    "today-ai-action",
                    if self.generating {
                        tr!(lang, "正在分析…", "Analyzing…", "分析中…", "분석 중…")
                    } else if self.report.is_some() {
                        tr!(
                            lang,
                            "查看完整建议",
                            "View full advice",
                            "提案をすべて見る",
                            "전체 제안 보기"
                        )
                    } else if configured {
                        tr!(
                            lang,
                            "生成今日建议",
                            "Generate today's advice",
                            "今日の提案を生成",
                            "오늘의 제안 생성"
                        )
                    } else {
                        tr!(lang, "配置 AI", "Set up AI", "AI を設定", "AI 설정")
                    },
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.report.is_some() {
                        this.view = View::Report;
                    } else {
                        this.generate_report();
                    }
                    cx.notify();
                })),
            )
    }

    fn render_interval_summaries(&self) -> Div {
        let lang = self.lang;
        let mut section = section().gap_3().child(panel_head(
            tr!(
                lang,
                "时段摘要",
                "Interval summaries",
                "時間帯の要約",
                "구간 요약"
            ),
            tr!(
                lang,
                "视觉模型按时段总结截图，作为 AI 建议的依据",
                "The vision model summarizes screenshots per interval as evidence for advice",
                "画像モデルが時間帯ごとにスクリーンショットを要約し、提案の根拠にします",
                "비전 모델이 구간별로 스냅샷을 요약해 제안의 근거로 사용합니다"
            ),
        ));
        if let Some(request) = &self.interval_request {
            section = section.child(div().text_xs().text_color(rgb(FAINT)).child(fill(
                tr!(
                    lang,
                    "上次发送：{} · {} 张截图 · {} · {}",
                    "Last sent: {} · {} screenshots · {} · {}",
                    "前回の送信：{} · スクリーンショット {} 枚 · {} · {}",
                    "마지막 전송: {} · 스냅샷 {}장 · {} · {}"
                ),
                &[
                    &hhmm(request.at),
                    &request.files.len(),
                    &request.model,
                    &request.endpoint,
                ],
            )));
        }
        if let Some(error) = &self.interval_error {
            section = section.child(div().text_sm().text_color(rgb(RED)).child(fill(
                tr!(
                    lang,
                    "时段分析失败：{}",
                    "Interval analysis failed: {}",
                    "時間帯の分析に失敗しました：{}",
                    "구간 분석 실패: {}"
                ),
                &[error],
            )));
        }
        section.children(if self.interval_summaries.is_empty() {
            vec![div().text_sm().text_color(rgb(SECONDARY)).child(tr!(
                lang,
                "完成一个分析时段后，摘要会显示在这里。",
                "Summaries appear here once an interval finishes.",
                "分析の時間帯が完了すると、要約がここに表示されます。",
                "분석 구간이 끝나면 요약이 여기에 표시됩니다."
            ))]
        } else {
            self.interval_summaries
                .iter()
                .rev()
                .map(|summary| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .py_3()
                        .border_b_1()
                        .border_color(rgb(DIVIDER))
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .text_xs()
                                .text_color(rgb(SECONDARY))
                                .child(format!("{}–{}", hhmm(summary.start), hhmm(summary.end)))
                                .child(fill(
                                    tr!(
                                        lang,
                                        "分析 {} 张截图",
                                        "{} screenshots analyzed",
                                        "スクリーンショット {} 枚を分析",
                                        "스냅샷 {}장 분석"
                                    ),
                                    &[&summary.screenshot_count],
                                )),
                        )
                        .child(div().text_sm().child(summary.summary.clone()))
                })
                .collect()
        })
    }

    fn render_sites(&self) -> Div {
        let lang = self.lang;
        section()
            .gap_3()
            .child(panel_head(
                tr!(
                    lang,
                    "浏览网站",
                    "Sites visited",
                    "訪問サイト",
                    "방문 사이트"
                ),
                tr!(
                    lang,
                    "仅记录当前浏览器域名，本机保存",
                    "Domain only, stored locally",
                    "現在のブラウザのドメインのみ、この PC に保存",
                    "현재 브라우저 도메인만 로컬 저장"
                ),
            ))
            .children(if self.day.sites.is_empty() {
                vec![div().text_sm().text_color(rgb(SECONDARY)).child(tr!(
                    lang,
                    "还没有读取到网站。",
                    "No sites captured yet.",
                    "まだサイトを取得していません。",
                    "아직 사이트를 읽지 못했습니다."
                ))]
            } else {
                self.day
                    .sites
                    .iter()
                    .take(20)
                    .map(|site| {
                        div()
                            .flex()
                            .justify_between()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(DIVIDER))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_sm().child(site.site.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(FAINT))
                                            .child(site.app.clone()),
                                    ),
                            )
                            .child(
                                div().text_xs().text_color(rgb(SECONDARY)).child(fill(
                                    tr!(
                                        lang,
                                        "{} · {} 次 · {}",
                                        "{} · {} visits · {}",
                                        "{} · {} 回 · {}",
                                        "{} · {}회 · {}"
                                    ),
                                    &[
                                        &fmt_ms(lang, site.ms),
                                        &site.visits,
                                        &site
                                            .category
                                            .as_deref()
                                            .map(|category| {
                                                crate::categories::label(lang, category).to_owned()
                                            })
                                            .unwrap_or_else(|| {
                                                tr!(
                                                    lang,
                                                    "未归类",
                                                    "Uncategorized",
                                                    "未分類",
                                                    "미분류"
                                                )
                                                .to_owned()
                                            }),
                                    ],
                                )),
                            )
                    })
                    .collect()
            })
    }

    fn render_snapshots(&self, cx: &mut Context<Self>) -> Div {
        let lang = self.lang;
        let shots = self
            .day
            .screenshots
            .iter()
            .enumerate()
            .rev()
            .take(6)
            .collect::<Vec<_>>();
        section()
            .gap_3()
            .child(panel_head(
                tr!(
                    lang,
                    "屏幕快照",
                    "Screenshots",
                    "スクリーンショット",
                    "스냅샷"
                ),
                tr!(
                    lang,
                    "只保留画面有变化的帧",
                    "Only frames that changed",
                    "変化のあったフレームのみ",
                    "화면이 바뀐 프레임만"
                ),
            ))
            .child(if shots.is_empty() {
                div()
                    .text_sm()
                    .text_color(rgb(SECONDARY))
                    .child(tr!(
                        lang,
                        "还没有快照。只有画面变化时才会保存，不会持续录屏。",
                        "No snapshots yet. They're saved only when the screen changes—no continuous recording.",
                        "スナップショットはまだありません。画面が変化したときだけ保存し、常時録画はしません。",
                        "스냅샷이 없습니다. 화면이 바뀔 때만 저장하며 계속 녹화하지 않습니다."
                    ))
            } else {
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .children(shots.into_iter().map(|(index, shot)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .id(SharedString::from(format!("shot-{index}")))
                            .tab_index(0)
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(CANVAS))
                            .focus(|style| style.border_color(rgb(ACCENT)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.lightbox = Some(index);
                                cx.notify();
                            }))
                            .child(
                                img(self.store.snapshot_path(&shot.file).unwrap_or_default())
                                    .w(px(220.))
                                    .h(px(124.))
                                    .rounded_md(),
                            )
                            .child(
                                div()
                                    .flex()
                                    .justify_between()
                                    .w(px(220.))
                                    .text_xs()
                                    .text_color(rgb(SECONDARY))
                                    .child(hhmm(shot.ts))
                                    .child(trigger_label(lang, &shot.trigger)),
                            )
                    }))
            })
    }

    fn render_stats(&mut self, cx: &mut Context<Self>) -> Div {
        self.ensure_stats();
        let lang = self.lang;
        let page = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.))
            .gap_5()
            .child(self.render_stats_head(cx));
        if self.stats.active_days == 0 {
            return page.child(
                panel()
                    .items_center()
                    .gap_3()
                    .py_10()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr!(
                                lang,
                                "这段时间还没有活动记录",
                                "No activity in this period",
                                "この期間の活動記録はありません",
                                "이 기간에 활동 기록이 없습니다"
                            )),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(SECONDARY))
                            .child(tr!(
                                lang,
                                "记录开启后，这里会按天汇总活跃时长、常用工具和活跃时段。",
                                "Once recording is on, daily active time, common tools and active hours are summarized here.",
                                "記録を有効にすると、日ごとのアクティブ時間・よく使うツール・活動時間帯をここにまとめます。",
                                "기록을 켜면 일별 활동 시간, 자주 쓰는 도구, 활동 시간대를 여기에 요약합니다."
                            )),
                    )
                    .child(
                        button(
                            "stats-empty-today",
                            tr!(
                                lang,
                                "查看今日活动",
                                "View today",
                                "今日の活動を見る",
                                "오늘 활동 보기"
                            ),
                            true,
                        )
                        .on_click(
                        cx.listener(|this, _, _, cx| {
                            this.view = View::Today;
                            this.date = Local::now().date_naive();
                            this.reload_selected();
                            cx.notify();
                        }),
                    )),
            );
        }
        page.child(self.render_stats_metrics())
            .child(self.render_stats_daily())
            .child(self.render_stats_hours())
            .child(
                div()
                    .flex()
                    .w_full()
                    .min_w(px(0.))
                    .gap_5()
                    .items_start()
                    .child(self.render_stats_apps().flex_1().min_w(px(0.)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_5()
                            .w(px(320.))
                            .child(self.render_stats_categories()),
                    ),
            )
            .children((!self.stats.sites.is_empty()).then(|| self.render_stats_sites()))
            .child(self.render_stats_days(cx))
    }

    fn render_stats_head(&mut self, cx: &mut Context<Self>) -> Div {
        let lang = self.lang;
        div()
            .flex()
            .flex_wrap()
            .items_start()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tr!(
                                lang,
                                "最近统计",
                                "Recent stats",
                                "最近の統計",
                                "최근 통계"
                            )),
                    )
                    .child(div().text_sm().text_color(rgb(SECONDARY)).child(fill(
                        tr!(
                            lang,
                            "最近 {} 天 · {} 至 {}",
                            "Last {} days · {} – {}",
                            "直近 {} 日間 · {} 〜 {}",
                            "최근 {}일 · {} ~ {}"
                        ),
                        &[
                            &self.stats_days,
                            &month_day(lang, &self.stats.start_key),
                            &month_day(lang, &self.stats.end_key),
                        ],
                    ))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .children([7u32, 14, 30].map(|days| {
                        small_button(
                            SharedString::from(format!("stats-period-{days}")),
                            &fill(tr!(lang, "{} 天", "{} days", "{} 日", "{}일"), &[&days]),
                            self.stats_days == days,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.stats_days = days;
                            this.stats_key = None;
                            this.ensure_stats();
                            cx.notify();
                        }))
                    })),
            )
    }

    fn render_stats_metrics(&self) -> Div {
        let lang = self.lang;
        let stats = &self.stats;
        let longest_ms = stats
            .longest_block
            .as_ref()
            .map(|(_, ms)| fmt_ms(lang, *ms))
            .unwrap_or_else(|| tr!(lang, "暂无", "None", "なし", "없음").to_owned());
        let longest_app = stats
            .longest_block
            .as_ref()
            .map(|(app, _)| app.clone())
            .unwrap_or_else(|| {
                tr!(lang, "不足 1 分钟", "Under 1 min", "1 分未満", "1분 미만").to_owned()
            });
        let best_note = stats
            .best_day
            .as_ref()
            .map(|(date_key, _)| {
                fill(
                    tr!(
                        lang,
                        "最活跃 {}",
                        "Busiest {}",
                        "最も活発 {}",
                        "가장 활발 {}"
                    ),
                    &[&month_day(lang, date_key)],
                )
            })
            .unwrap_or_else(|| {
                tr!(
                    lang,
                    "按有记录的天数计",
                    "Across days with records",
                    "記録のある日数で計算",
                    "기록이 있는 날 기준"
                )
                .to_owned()
            });
        div()
            .flex()
            .w_full()
            .py_4()
            .border_b_1()
            .border_color(rgb(DIVIDER))
            .child(metric(
                tr!(lang, "总活跃", "Total active", "合計アクティブ", "총 활동"),
                &fmt_ms(lang, stats.total_active_ms),
                &fill(
                    tr!(
                        lang,
                        "{} 天有记录",
                        "{} days with records",
                        "{} 日分の記録",
                        "기록 {}일"
                    ),
                    &[&stats.active_days],
                ),
            ))
            .child(metric(
                tr!(lang, "日均活跃", "Daily average", "1 日平均", "일평균"),
                &fmt_ms(lang, stats.avg_daily_ms),
                &best_note,
            ))
            .child(metric(
                tr!(lang, "应用切换", "App switches", "アプリ切替", "앱 전환"),
                &stats.total_switches.to_string(),
                &fill(
                    tr!(
                        lang,
                        "{} 次/小时",
                        "{}/hour",
                        "1時間あたり{}回",
                        "시간당 {}회"
                    ),
                    &[&format!("{:.1}", stats.switches_per_hour)],
                ),
            ))
            .child(metric(
                tr!(lang, "最长专注", "Longest focus", "最長の集中", "최장 집중"),
                &longest_ms,
                &longest_app,
            ))
            .child(metric(
                tr!(
                    lang,
                    "本地快照",
                    "Local snapshots",
                    "ローカル撮影",
                    "로컬 스냅샷"
                ),
                &stats.screenshots.to_string(),
                &fmt_bytes(stats.snapshot_bytes),
            ))
    }

    fn render_stats_daily(&self) -> Div {
        let lang = self.lang;
        let stats = &self.stats;
        let max = stats
            .daily
            .iter()
            .map(|day| day.active_ms)
            .max()
            .unwrap_or(0)
            .max(1);
        let step = stats.daily.len().div_ceil(8).max(1);
        let total = stats.daily.len();
        section()
            .gap_3()
            .child(panel_head(
                tr!(
                    lang,
                    "每日活跃",
                    "Daily activity",
                    "日別のアクティブ",
                    "일별 활동"
                ),
                &fill(
                    tr!(
                        lang,
                        "单日最高 {}",
                        "Peak {}",
                        "1 日の最高 {}",
                        "하루 최고 {}"
                    ),
                    &[&fmt_ms(lang, max)],
                ),
            ))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_1()
                    .h(px(136.))
                    .w_full()
                    .children(stats.daily.iter().map(|day: &DayPoint| {
                        let fraction = day.active_ms as f32 / max as f32;
                        let height =
                            (fraction * 120.0).max(if day.active_ms > 0 { 3.0 } else { 2.0 });
                        let color = if day.active_ms > 0 {
                            category_color(&day.category)
                        } else {
                            SUBTLE
                        };
                        let tooltip = if day.active_ms > 0 {
                            fill(
                                tr!(
                                    lang,
                                    "{} {} · {} · {} 次切换 · {}",
                                    "{} {} · {} · {} switches · {}",
                                    "{} {} · {} · 切替 {} 回 · {}",
                                    "{} {} · {} · 전환 {}회 · {}"
                                ),
                                &[
                                    &month_day(lang, &day.date_key),
                                    &weekday_name(lang, day.weekday),
                                    &fmt_ms(lang, day.active_ms),
                                    &day.switches,
                                    &day.top_app,
                                ],
                            )
                        } else {
                            fill(
                                tr!(
                                    lang,
                                    "{} {} · 无记录",
                                    "{} {} · no records",
                                    "{} {} · 記録なし",
                                    "{} {} · 기록 없음"
                                ),
                                &[
                                    &month_day(lang, &day.date_key),
                                    &weekday_name(lang, day.weekday),
                                ],
                            )
                        };
                        div()
                            .flex()
                            .flex_col()
                            .justify_end()
                            .h_full()
                            .flex_1()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .id(SharedString::from(format!("stats-bar-{}", day.date_key)))
                                    .w_full()
                                    .h(px(height))
                                    .rounded_sm()
                                    .bg(rgb(color))
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| TextTooltip(tooltip.clone())).into()
                                    }),
                            )
                    })),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .w_full()
                    .children(stats.daily.iter().enumerate().map(|(index, day)| {
                        let show = index + 1 == total || index % step == 0;
                        let label = if show {
                            NaiveDate::parse_from_str(&day.date_key, "%Y-%m-%d")
                                .map(|date| {
                                    fill(tr!(lang, "{}日", "{}", "{}日", "{}일"), &[&date.day()])
                                })
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(10.))
                            .text_color(rgb(FAINT))
                            .text_center()
                            .child(label)
                    })),
            )
    }

    fn render_stats_hours(&self) -> Div {
        let lang = self.lang;
        let stats = &self.stats;
        let max = stats.hourly.iter().copied().max().unwrap_or(0).max(1);
        section()
            .gap_3()
            .child(panel_head(
                tr!(
                    lang,
                    "活跃时段",
                    "Active hours",
                    "活動時間帯",
                    "활동 시간대"
                ),
                tr!(
                    lang,
                    "所有记录日累计，按本机时间",
                    "Summed over recorded days, local time",
                    "記録のある日を合算、ローカル時間",
                    "기록된 날 합산, 현지 시간"
                ),
            ))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_1()
                    .h(px(96.))
                    .w_full()
                    .children((0..24).map(|hour| {
                        let ms = stats.hourly[hour];
                        let fraction = ms as f32 / max as f32;
                        let height = (fraction * 80.0).max(if ms > 0 { 2.0 } else { 1.0 });
                        let tooltip = format!("{hour}:00–{}:00 · {}", hour + 1, fmt_ms(lang, ms));
                        div()
                            .flex()
                            .flex_col()
                            .justify_end()
                            .h_full()
                            .flex_1()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .id(SharedString::from(format!("stats-hour-{hour}")))
                                    .w_full()
                                    .h(px(height))
                                    .rounded_sm()
                                    .bg(rgb(if ms > 0 { ACCENT } else { SUBTLE }))
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| TextTooltip(tooltip.clone())).into()
                                    }),
                            )
                    })),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(px(10.))
                    .text_color(rgb(FAINT))
                    .children(
                        [0, 6, 12, 18, 23]
                            .map(|hour| fill(tr!(lang, "{}时", "{}h", "{}時", "{}시"), &[&hour])),
                    ),
            )
    }

    fn render_stats_apps(&self) -> Div {
        let lang = self.lang;
        let stats = &self.stats;
        let max_ms = stats.apps.first().map(|app| app.ms).unwrap_or(1).max(1);
        section()
            .gap_3()
            .child(panel_head(
                tr!(lang, "应用使用", "App usage", "アプリの使用", "앱 사용"),
                tr!(
                    lang,
                    "按周期内活跃时长排序",
                    "By active time in period",
                    "期間内のアクティブ時間順",
                    "기간 내 활동 시간순"
                ),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .children(stats.apps.iter().take(12).map(|app| {
                        let width = app.ms as f32 / max_ms as f32;
                        let color = category_color(&app.category);
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(DIVIDER))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .truncate()
                                            .text_sm()
                                            .child(app.app.clone()),
                                    )
                                    .child(div().w(px(72.)).text_xs().text_color(rgb(color)).child(
                                        crate::categories::label(lang, &app.category).to_owned(),
                                    ))
                                    .child(
                                        div()
                                            .w(px(76.))
                                            .text_right()
                                            .text_sm()
                                            .child(fmt_ms(lang, app.ms)),
                                    )
                                    .child(
                                        div()
                                            .w(px(44.))
                                            .text_right()
                                            .text_xs()
                                            .text_color(rgb(SECONDARY))
                                            .child(fill(
                                                tr!(lang, "{} 天", "{} days", "{} 日", "{}일"),
                                                &[&app.days],
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .h(px(3.))
                                    .w_full()
                                    .bg(rgb(SUBTLE))
                                    .rounded_full()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(width))
                                            .rounded_full()
                                            .bg(rgb(color)),
                                    ),
                            )
                    })),
            )
    }

    fn render_stats_categories(&self) -> Div {
        let lang = self.lang;
        let stats = &self.stats;
        let total = stats.total_active_ms.max(1);
        section()
            .gap_3()
            .child(panel_head(
                tr!(
                    lang,
                    "分类占比",
                    "Category share",
                    "カテゴリ別の割合",
                    "분류 비중"
                ),
                tr!(
                    lang,
                    "按活跃时长",
                    "By active time",
                    "アクティブ時間順",
                    "활동 시간 기준"
                ),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .children(stats.categories.iter().take(8).map(|category| {
                        let share = category.ms as f32 / total as f32;
                        let color = category_color(&category.name);
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .child(div().text_sm().child(
                                        crate::categories::label(lang, &category.name).to_owned(),
                                    ))
                                    .child(div().text_xs().text_color(rgb(SECONDARY)).child(fill(
                                        tr!(lang, "{} · {}%", "{} · {}%", "{} · {}%", "{} · {}%"),
                                        &[
                                            &fmt_ms(lang, category.ms),
                                            &format!("{:.0}", share * 100.0),
                                        ],
                                    ))),
                            )
                            .child(
                                div()
                                    .h(px(4.))
                                    .w_full()
                                    .bg(rgb(SUBTLE))
                                    .rounded_full()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(share))
                                            .rounded_full()
                                            .bg(rgb(color)),
                                    ),
                            )
                    })),
            )
    }

    fn render_stats_sites(&self) -> Div {
        let lang = self.lang;
        let max_ms = self
            .stats
            .sites
            .first()
            .map(|site| site.ms)
            .unwrap_or(1)
            .max(1);
        section()
            .gap_3()
            .child(panel_head(
                tr!(
                    lang,
                    "常去网站",
                    "Top sites",
                    "よく見るサイト",
                    "자주 가는 사이트"
                ),
                tr!(
                    lang,
                    "按浏览器停留时长排序",
                    "By time in the browser",
                    "ブラウザの滞在時間順",
                    "브라우저 체류 시간순"
                ),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .children(self.stats.sites.iter().take(8).map(|site| {
                        let width = site.ms as f32 / max_ms as f32;
                        let color = site
                            .category
                            .as_deref()
                            .map(category_color)
                            .unwrap_or(0x3f897d);
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(DIVIDER))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .truncate()
                                            .text_sm()
                                            .child(site.site.clone()),
                                    )
                                    .child(
                                        div()
                                            .w(px(80.))
                                            .truncate()
                                            .text_xs()
                                            .text_color(rgb(SECONDARY))
                                            .child(site.app.clone()),
                                    )
                                    .child(
                                        div()
                                            .w(px(44.))
                                            .text_right()
                                            .text_xs()
                                            .text_color(rgb(SECONDARY))
                                            .child(fill(
                                                tr!(lang, "{} 次", "{} visits", "{} 回", "{}회"),
                                                &[&site.visits],
                                            )),
                                    )
                                    .child(
                                        div()
                                            .w(px(76.))
                                            .text_right()
                                            .text_sm()
                                            .child(fmt_ms(lang, site.ms)),
                                    ),
                            )
                            .child(
                                div()
                                    .h(px(3.))
                                    .w_full()
                                    .bg(rgb(SUBTLE))
                                    .rounded_full()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(relative(width))
                                            .rounded_full()
                                            .bg(rgb(color)),
                                    ),
                            )
                    })),
            )
    }

    fn render_stats_days(&self, cx: &mut Context<Self>) -> Div {
        let lang = self.lang;
        section()
            .gap_3()
            .child(panel_head(
                tr!(lang, "每日明细", "Daily detail", "日別の内訳", "일별 내역"),
                tr!(
                    lang,
                    "点一行查看当天活动",
                    "Select a row to open that day",
                    "行を選ぶとその日の活動を表示",
                    "행을 선택하면 그날 활동을 봅니다"
                ),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .pb_2()
                    .border_b_1()
                    .border_color(rgb(DIVIDER))
                    .text_xs()
                    .text_color(rgb(FAINT))
                    .child(
                        div()
                            .w(px(132.))
                            .child(tr!(lang, "日期", "Date", "日付", "날짜")),
                    )
                    .child(div().flex_1().min_w(px(0.)).child(tr!(
                        lang,
                        "主要应用",
                        "Top app",
                        "主なアプリ",
                        "주요 앱"
                    )))
                    .child(div().w(px(80.)).text_right().child(tr!(
                        lang,
                        "活跃时长",
                        "Active time",
                        "アクティブ時間",
                        "활동 시간"
                    )))
                    .child(
                        div()
                            .w(px(64.))
                            .text_right()
                            .child(tr!(lang, "切换", "Switches", "切替", "전환")),
                    )
                    .child(div().w(px(80.)).text_right().child(tr!(
                        lang,
                        "主要分类",
                        "Top category",
                        "主なカテゴリ",
                        "주요 분류"
                    ))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .children(self.stats.daily.iter().rev().map(|day| {
                        let date = NaiveDate::parse_from_str(&day.date_key, "%Y-%m-%d").ok();
                        let active = day.active_ms > 0;
                        let color = category_color(&day.category);
                        div()
                            .id(SharedString::from(format!("stats-day-{}", day.date_key)))
                            .tab_index(0)
                            .flex()
                            .items_center()
                            .gap_3()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(DIVIDER))
                            .when(active, |row| {
                                row.cursor_pointer()
                                    .hover(|style| style.bg(rgb(CANVAS)))
                                    .focus(|style| style.border_color(rgb(ACCENT)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Some(date) = date {
                                            this.view = View::Today;
                                            this.date = date;
                                            this.reload_selected();
                                            cx.notify();
                                        }
                                    }))
                            })
                            .child(
                                div()
                                    .w(px(132.))
                                    .text_sm()
                                    .text_color(rgb(if active { INK } else { FAINT }))
                                    .child(fill(
                                        tr!(lang, "{} · {}", "{} · {}", "{} · {}", "{} · {}"),
                                        &[
                                            &month_day(lang, &day.date_key),
                                            &weekday_name(lang, day.weekday),
                                        ],
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_sm()
                                    .text_color(rgb(SECONDARY))
                                    .child(if active {
                                        day.top_app.clone()
                                    } else {
                                        tr!(lang, "无记录", "No records", "記録なし", "기록 없음")
                                            .to_owned()
                                    }),
                            )
                            .child(div().w(px(80.)).text_right().text_sm().child(if active {
                                fmt_ms(lang, day.active_ms)
                            } else {
                                "—".to_owned()
                            }))
                            .child(
                                div()
                                    .w(px(64.))
                                    .text_right()
                                    .text_sm()
                                    .text_color(rgb(SECONDARY))
                                    .child(day.switches.to_string()),
                            )
                            .child(
                                div()
                                    .w(px(80.))
                                    .text_right()
                                    .text_xs()
                                    .text_color(rgb(color))
                                    .child(
                                        crate::categories::label(lang, &day.category).to_owned(),
                                    ),
                            )
                    })),
            )
    }

    fn render_report(&mut self, cx: &mut Context<Self>) -> Div {
        let lang = self.lang;
        let page = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.))
            .gap_5()
            .child(self.render_page_head(
                tr!(
                    lang,
                    "AI 工具升级建议",
                    "AI tool upgrades",
                    "AI ツール改善",
                    "AI 도구 개선"
                ),
                tr!(
                    lang,
                    "学习你实际的工作方式，只推荐能让现有动作更省力的工具、配置和开源替代",
                    "Learns how you actually work and only suggests tools, settings and open-source alternatives that make current actions easier",
                    "実際の仕事の進め方を学び、今の操作を楽にするツール・設定・OSS 代替だけを提案します",
                    "실제 업무 방식을 학습해 지금 하는 일을 더 쉽게 만드는 도구, 설정, 오픈소스 대안만 제안합니다"
                ),
                true,
                cx,
            ));

        if !has_ai(&self.settings.get()) {
            return page.child(self.render_ai_setup(cx));
        }
        let mut document = panel().gap_4().child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .gap_3()
                .child(panel_head(
                    &fill(
                        tr!(lang, "{} 建议", "{} advice", "{} の提案", "{} 제안"),
                        &[&self.date_key()],
                    ),
                    &fill(
                        tr!(
                            lang,
                            "已记录 {} · {} 次切换",
                            "{} recorded · {} switches",
                            "{} を記録 · 切替 {} 回",
                            "{} 기록 · 전환 {}회"
                        ),
                        &[&fmt_ms(lang, self.day.total_active_ms), &self.day.switches],
                    ),
                ))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            button(
                                "preview-prompt",
                                if self.prompt_preview.is_some() {
                                    tr!(
                                        lang,
                                        "收起发送内容",
                                        "Hide sent content",
                                        "送信内容を閉じる",
                                        "전송 내용 접기"
                                    )
                                } else {
                                    tr!(
                                        lang,
                                        "查看发送内容",
                                        "View sent content",
                                        "送信内容を見る",
                                        "전송 내용 보기"
                                    )
                                },
                                false,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_prompt_preview();
                                cx.notify();
                            })),
                        )
                        .child(
                            button(
                                "generate-report",
                                if self.generating {
                                    tr!(lang, "正在分析…", "Analyzing…", "分析中…", "분석 중…")
                                } else if self.report.is_some() {
                                    tr!(lang, "重新生成", "Regenerate", "再生成", "다시 생성")
                                } else {
                                    tr!(
                                        lang,
                                        "生成建议",
                                        "Generate advice",
                                        "提案を生成",
                                        "제안 생성"
                                    )
                                },
                                true,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.generating {
                                    this.generate_report();
                                }
                                cx.notify();
                            })),
                        ),
                ),
        );
        if let Some(preview) = &self.prompt_preview {
            let mut block = div()
                .flex()
                .flex_col()
                .p_4()
                .rounded_md()
                .bg(rgb(CANVAS))
                .border_1()
                .border_color(rgb(DIVIDER))
                .text_xs()
                .text_color(rgb(SECONDARY))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(INK))
                        .pb_2()
                        .child(tr!(
                            lang,
                            "生成建议时会把以下内容发送给 AI 接口（不含截图、网站域名和密钥）",
                            "Generating advice sends the content below to the AI endpoint (no screenshots, site domains or keys)",
                            "提案の生成時に以下の内容を AI に送信します（スクリーンショット・サイトのドメイン・キーは含みません）",
                            "제안을 생성할 때 아래 내용을 AI로 전송합니다(스냅샷, 사이트 도메인, 키는 포함되지 않음)"
                        )),
                );
            for (index, line) in preview.lines().enumerate() {
                block = block.child(
                    div()
                        .id(SharedString::from(format!("preview-line-{index}")))
                        .child(if line.is_empty() {
                            " ".to_owned()
                        } else {
                            line.to_owned()
                        }),
                );
            }
            document = document.child(block);
        }
        if self.generating {
            document = document.child(
                div()
                    .p_8()
                    .text_center()
                    .text_color(rgb(SECONDARY))
                    .child(tr!(
                        lang,
                        "正在计算证据并请求模型，通常需要几十秒。",
                        "Computing evidence and asking the model; this usually takes tens of seconds.",
                        "根拠を計算してモデルに問い合わせています。通常は数十秒かかります。",
                        "근거를 계산하고 모델에 요청하는 중입니다. 보통 수십 초 걸립니다."
                    )),
            );
        } else if let Some(error) = &self.report_error {
            document = document.child(
                div()
                    .p_4()
                    .rounded_md()
                    .bg(rgb(CANVAS))
                    .border_1()
                    .border_color(rgb(DIVIDER))
                    .text_color(rgb(RED))
                    .child(error.clone()),
            );
        } else if let Some(report) = &self.report {
            let source = if report.source == "auto" {
                tr!(lang, "自动生成", "Auto-generated", "自動生成", "자동 생성")
            } else {
                tr!(
                    lang,
                    "手动生成",
                    "Generated manually",
                    "手動生成",
                    "직접 생성"
                )
            };
            let generated = crate::store::local_datetime(report.generated_at)
                .map(|date| date.format("%m-%d %H:%M").to_string())
                .unwrap_or_default();
            let mut meta = div()
                .flex()
                .flex_wrap()
                .gap_3()
                .text_xs()
                .text_color(rgb(FAINT))
                .child(fill(
                    tr!(lang, "模型 {}", "Model {}", "モデル {}", "모델 {}"),
                    &[&report.model],
                ))
                .child(fill(
                    tr!(lang, "{} · {}", "{} · {}", "{} · {}", "{} · {}"),
                    &[&source, &generated],
                ))
                .child(fill(
                    tr!(
                        lang,
                        "证据 {} 条",
                        "{} evidence items",
                        "根拠 {} 件",
                        "근거 {}개"
                    ),
                    &[&report.evidence.len()],
                ));
            if report.usage.prompt_tokens + report.usage.completion_tokens > 0 {
                meta = meta.child(format!(
                    "Token {} + {}",
                    report.usage.prompt_tokens, report.usage.completion_tokens
                ));
            }
            if !report.prompt_version.is_empty() {
                meta = meta.child(report.prompt_version.clone());
            }
            document = document
                .child(meta)
                .child(self.render_report_summary(report, cx));
        } else {
            document = document.child(div().p_8().text_center().text_color(rgb(SECONDARY)).child(
                tr!(
                    lang,
                    "还没有这一天的建议，点击右上角“生成建议”。",
                    "No advice for this day yet—choose “Generate advice” in the top right.",
                    "この日の提案はまだありません。右上の「提案を生成」を押してください。",
                    "이 날의 제안이 없습니다. 오른쪽 위의 “제안 생성”을 누르세요."
                ),
            ));
        }
        page.child(document).child(self.render_report_history(cx))
    }

    /// Compact in-app summary; the full report opens as an HTML page.
    fn render_report_summary(&self, report: &Report, cx: &mut Context<Self>) -> Div {
        let lang = self.lang;
        let open_button = button(
            "open-report-html",
            tr!(
                lang,
                "在浏览器中查看完整报告",
                "Open full report in browser",
                "ブラウザで完全なレポートを開く",
                "브라우저에서 전체 보고서 열기"
            ),
            true,
        )
        .on_click(cx.listener(|this, _, _, cx| {
            if let Some(report) = &this.report {
                match this.analyzer.write_html(report) {
                    Ok(path) => {
                        let _ = open::that(path);
                    }
                    Err(error) => {
                        this.notice = Some((
                            fill(
                                tr!(
                                    this.lang,
                                    "无法打开报告：{}",
                                    "Couldn't open the report: {}",
                                    "レポートを開けませんでした：{}",
                                    "보고서를 열 수 없습니다: {}"
                                ),
                                &[&format!("{error:#}")],
                            ),
                            false,
                        ));
                    }
                }
            }
            cx.notify();
        }));
        let Some(advice) = &report.advice else {
            return div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(SECONDARY))
                        .child(report_excerpt(&report.markdown)),
                )
                .child(div().flex().child(open_button));
        };
        let mut summary = div().flex().flex_col().gap_4().child(
            div()
                .p_4()
                .rounded_md()
                .bg(rgb(BRAND_PALE))
                .text_size(px(17.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(advice.headline.trim().to_owned()),
        );
        if let Some(metrics) = &report.metrics {
            summary = summary.child(
                div()
                    .flex()
                    .py_3()
                    .border_y_1()
                    .border_color(rgb(DIVIDER))
                    .child(metric(
                        tr!(lang, "活跃", "Active", "アクティブ", "활동"),
                        &fill(
                            tr!(lang, "{} 分钟", "{} min", "{} 分", "{}분"),
                            &[&format!("{:.0}", metrics.active_min)],
                        ),
                        "",
                    ))
                    .child(metric(
                        tr!(lang, "深度工作", "Deep work", "集中作業", "집중 작업"),
                        &fill(
                            tr!(lang, "{} 分钟", "{} min", "{} 分", "{}분"),
                            &[&format!("{:.0}", metrics.deep_work_min)],
                        ),
                        &fill(
                            tr!(lang, "{} 块", "{} blocks", "{} ブロック", "{}개 블록"),
                            &[&format!("{:.0}", metrics.deep_blocks)],
                        ),
                    ))
                    .child(metric(
                        tr!(
                            lang,
                            "每小时切换",
                            "Switches/hour",
                            "1 時間あたりの切替",
                            "시간당 전환"
                        ),
                        &fill(
                            tr!(lang, "{} 次", "{}/h", "{} 回", "{}회"),
                            &[&format!("{:.1}", metrics.switches_per_hour)],
                        ),
                        "",
                    ))
                    .child(metric(
                        tr!(lang, "打开通讯", "Chat checks", "チャット確認", "채팅 확인"),
                        &fill(
                            tr!(lang, "{} 次", "{}", "{} 回", "{}회"),
                            &[&format!("{:.0}", metrics.comm_checks)],
                        ),
                        "",
                    )),
            );
        }
        let mut list =
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_xs().text_color(rgb(FAINT)).child(fill(
                    tr!(
                        lang,
                        "{} 个工具升级机会",
                        "{} tool upgrades",
                        "ツール改善の候補 {} 件",
                        "도구 개선 기회 {}개"
                    ),
                    &[&advice.upgrades.len()],
                )));
        for (index, upgrade) in advice.upgrades.iter().enumerate() {
            let mut title = div()
                .flex_1()
                .flex()
                .flex_col()
                .child(div().text_sm().child(upgrade.title.trim().to_owned()));
            if !upgrade.habit.trim().is_empty() {
                title = title.child(div().text_xs().text_color(rgb(FAINT)).child(fill(
                    tr!(
                        lang,
                        "接上你现在：{}",
                        "Builds on: {}",
                        "今のやり方に接続：{}",
                        "지금 방식에 연결: {}"
                    ),
                    &[&upgrade.habit.trim()],
                )));
            }
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(22.))
                            .rounded_full()
                            .bg(rgb(if index == 0 { ACCENT } else { INK }))
                            .text_color(rgb(0xffffff))
                            .text_xs()
                            .child(format!("{}", index + 1)),
                    )
                    .child(title)
                    .child(div().text_xs().text_color(rgb(FAINT)).child(fill(
                        tr!(
                            lang,
                            "{} · 投入{} · 把握{}",
                            "{} · effort {} · confidence {}",
                            "{} · 手間 {} · 確度 {}",
                            "{} · 투입 {} · 확신 {}"
                        ),
                        &[
                            &crate::analyzer::kind_label(lang, &upgrade.kind),
                            &crate::analyzer::level_label(lang, &upgrade.effort),
                            &crate::analyzer::level_label(lang, &upgrade.confidence),
                        ],
                    ))),
            );
        }
        summary = summary.child(list);

        if let Some(profile) = &report.profile {
            let top = profile
                .tools
                .iter()
                .take(2)
                .map(|tool| tool.name.clone())
                .collect::<Vec<_>>()
                .join(" / ");
            summary = summary.child(div().text_sm().text_color(rgb(SECONDARY)).child(
                if top.is_empty() {
                    fill(
                        tr!(
                            lang,
                            "已学习 {} 天的工作方式",
                            "Learned {} days of your work style",
                            "{} 日分の仕事の進め方を学習済み",
                            "{}일치 업무 방식을 학습함"
                        ),
                        &[&profile.days],
                    )
                } else {
                    fill(
                        tr!(
                            lang,
                            "已学习 {} 天的工作方式：{}",
                            "Learned {} days of your work style: {}",
                            "{} 日分の仕事の進め方を学習済み：{}",
                            "{}일치 업무 방식을 학습함: {}"
                        ),
                        &[&profile.days, &top],
                    )
                },
            ));
        }

        if !report.recommendations.is_empty() {
            let mut candidates = div().flex().flex_col().gap_2().child(
                div().text_xs().text_color(rgb(FAINT)).child(tr!(
                    lang,
                    "工具候选 · 记录你的选择，下次报告会对比同类来回",
                    "Tool candidates · your choices are compared in the next report",
                    "ツール候補 · 選択を記録し、次回のレポートで比較します",
                    "도구 후보 · 선택을 기록하면 다음 보고서에서 비교합니다"
                )),
            );
            for item in &report.recommendations {
                let date = report.date_key.clone();
                let opportunity = item.id.clone();
                let resource = item
                    .resources
                    .first()
                    .map(|resource| resource.name.clone())
                    .unwrap_or_else(|| item.title.clone());
                let url = item
                    .resources
                    .first()
                    .map(|resource| resource.url.clone())
                    .unwrap_or_default();
                let mut row = div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().text_sm().child(resource.clone()));
                for adopted in [true, false] {
                    let selected = self
                        .decisions
                        .get(&item.id)
                        .is_some_and(|value| *value == adopted);
                    let date = date.clone();
                    let opportunity = opportunity.clone();
                    let resource = resource.clone();
                    let url = url.clone();
                    row = row.child(
                        small_button(
                            SharedString::from(format!("decide-{}-{adopted}", opportunity)),
                            if adopted {
                                tr!(lang, "已用上", "Using it", "使っている", "사용 중")
                            } else {
                                tr!(lang, "不需要", "Not needed", "不要", "불필요")
                            },
                            selected,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            match this.analyzer.decide(
                                &date,
                                &opportunity,
                                &resource,
                                &url,
                                adopted,
                            ) {
                                Ok(_) => {
                                    this.decisions.insert(opportunity.clone(), adopted);
                                    let choice = if adopted {
                                        tr!(
                                            this.lang,
                                            "已用上",
                                            "Using it",
                                            "使っている",
                                            "사용 중"
                                        )
                                    } else {
                                        tr!(
                                            this.lang,
                                            "不需要",
                                            "Not needed",
                                            "不要",
                                            "불필요"
                                        )
                                    };
                                    this.notice = Some((
                                        fill(
                                            tr!(
                                                this.lang,
                                                "已记录：{}。下次生成会自动对比同类来回。",
                                                "Saved: {}. The next report compares it.",
                                                "記録しました：{}。次回の生成で自動的に比較します。",
                                                "기록했습니다: {}. 다음 생성에서 자동으로 비교합니다."
                                            ),
                                            &[&choice],
                                        ),
                                        true,
                                    ))
                                }
                                Err(error) => {
                                    this.notice = Some((
                                        fill(
                                            tr!(
                                                this.lang,
                                                "记录失败：{}",
                                                "Couldn't save: {}",
                                                "記録に失敗しました：{}",
                                                "기록 실패: {}"
                                            ),
                                            &[&format!("{error:#}")],
                                        ),
                                        false,
                                    ))
                                }
                            }
                            cx.notify();
                        })),
                    );
                }
                candidates = candidates.child(row);
            }
            summary = summary.child(candidates);
        }
        summary.child(div().flex().child(open_button))
    }

    fn render_ai_setup(&mut self, cx: &mut Context<Self>) -> Div {
        let lang = self.lang;
        panel()
            .gap_4()
            .child(panel_head(
                tr!(
                    lang,
                    "先连接一个 AI 模型",
                    "Connect an AI model first",
                    "まず AI モデルに接続",
                    "먼저 AI 모델 연결"
                ),
                tr!(
                    lang,
                    "支持 DeepSeek、通义、Kimi、OpenAI 和本地 Ollama",
                    "Works with DeepSeek, Qwen, Kimi, OpenAI and local Ollama",
                    "DeepSeek・通義・Kimi・OpenAI・ローカル Ollama に対応",
                    "DeepSeek, Qwen, Kimi, OpenAI, 로컬 Ollama 지원"
                ),
            ))
            .child(setting_row(
                tr!(
                    lang,
                    "接口地址",
                    "Endpoint",
                    "接続先",
                    "API 주소"
                ),
                tr!(
                    lang,
                    "本地 Ollama：http://localhost:11434/v1",
                    "Local Ollama: http://localhost:11434/v1",
                    "ローカル Ollama：http://localhost:11434/v1",
                    "로컬 Ollama: http://localhost:11434/v1"
                ),
                self.form.base_url.clone(),
            ))
            .child(setting_row(
                "API Key",
                tr!(
                    lang,
                    "使用 Windows DPAPI 加密保存在本机",
                    "Encrypted with Windows DPAPI on this PC",
                    "Windows DPAPI で暗号化してこの PC に保存",
                    "Windows DPAPI로 암호화해 이 PC에 저장"
                ),
                self.form.api_key.clone(),
            ))
            .child(setting_row(
                tr!(lang, "模型名", "Model", "モデル名", "모델명"),
                tr!(
                    lang,
                    "建议多模态模型，如 qwen2.5vl、gpt-4o-mini",
                    "A multimodal model is recommended, e.g. qwen2.5vl, gpt-4o-mini",
                    "マルチモーダルモデルを推奨（例：qwen2.5vl、gpt-4o-mini）",
                    "멀티모달 모델 권장(예: qwen2.5vl, gpt-4o-mini)"
                ),
                self.form.model.clone(),
            ))
            .child(
                button(
                    "report-ai-save",
                    if self.testing {
                        tr!(lang, "正在测试…", "Testing…", "テスト中…", "테스트 중…")
                    } else {
                        tr!(
                            lang,
                            "保存并测试",
                            "Save and test",
                            "保存してテスト",
                            "저장 후 테스트"
                        )
                    },
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.test_ai(cx);
                    cx.notify();
                })),
            )
            .child(div().text_sm().text_color(rgb(SECONDARY)).child(tr!(
                lang,
                "建议只发送本地计算的脱敏证据（聊天窗口只保留应用名），不发送网站域名、API Key 或键盘输入。可以随时点击“查看发送内容”核对。",
                "Only redacted evidence computed locally is sent (chat windows keep just the app name); site domains, API keys and keystrokes never leave. Use “View sent content” to check anytime.",
                "送信するのはローカルで計算した匿名化済みの根拠のみです（チャットウィンドウはアプリ名だけ）。サイトのドメイン、API Key、キー入力は送信しません。「送信内容を見る」でいつでも確認できます。",
                "로컬에서 계산한 익명 처리된 근거만 전송합니다(채팅 창은 앱 이름만 유지). 사이트 도메인, API 키, 키보드 입력은 보내지 않습니다. “전송 내용 보기”로 언제든 확인할 수 있습니다."
            )))
    }

    fn render_report_history(&self, cx: &mut Context<Self>) -> Div {
        let lang = self.lang;
        panel()
            .gap_2()
            .child(panel_head(
                tr!(lang, "历史建议", "Past advice", "過去の提案", "지난 제안"),
                &fill(
                    tr!(lang, "{} 份", "{} reports", "{} 件", "{}개"),
                    &[&self.report_list.len()],
                ),
            ))
            .children(self.report_list.iter().take(30).map(|item| {
                let date = item.date_key.clone();
                div()
                    .id(SharedString::from(format!("history-{date}")))
                    .flex()
                    .justify_between()
                    .p_3()
                    .rounded_md()
                    .tab_index(0)
                    .border_1()
                    .border_color(rgb(SURFACE))
                    .focus(|style| style.border_color(rgb(ACCENT)))
                    .hover(|style| style.bg(rgb(CANVAS)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Ok(date) = NaiveDate::parse_from_str(&date, "%Y-%m-%d") {
                            this.date = date;
                            this.reload_selected();
                        }
                        cx.notify();
                    }))
                    .child(div().text_sm().child(item.date_key.clone()))
                    .child(div().text_xs().text_color(rgb(SECONDARY)).child(fill(
                        tr!(
                            lang,
                            "{} 分钟 · {}",
                            "{} min · {}",
                            "{} 分 · {}",
                            "{}분 · {}"
                        ),
                        &[&item.active_minutes, &item.model],
                    )))
            }))
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let lang = self.lang;
        let page = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.))
            .gap_5()
            .child(self.render_page_head(
                tr!(lang, "设置", "Settings", "設定", "설정"),
                tr!(
                    lang,
                    "修改后点击“保存设置”生效",
                    "Changes apply after you choose “Save settings”",
                    "変更は「設定を保存」で反映されます",
                    "변경 후 “설정 저장”을 눌러야 적용됩니다"
                ),
                false,
                cx,
            ));

        let language = panel()
            .gap_4()
            .child(panel_head(
                tr!(
                    lang,
                    "语言与界面",
                    "Language & interface",
                    "言語と画面",
                    "언어 및 화면"
                ),
                tr!(
                    lang,
                    "保存后立即生效；报告保留生成时的语言",
                    "Applies immediately after saving; reports keep the language they were generated in",
                    "保存後すぐに反映されます。レポートは生成時の言語のままです",
                    "저장 후 바로 적용되며, 보고서는 생성 당시 언어로 유지됩니다"
                ),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(setting_label(
                        tr!(
                            lang,
                            "界面语言",
                            "Interface language",
                            "表示言語",
                            "인터페이스 언어"
                        ),
                        tr!(
                            lang,
                            "默认跟随 Windows 显示语言",
                            "Follows the Windows display language by default",
                            "既定では Windows の表示言語に従います",
                            "기본적으로 Windows 표시 언어를 따릅니다"
                        ),
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                small_button(
                                    "lang-auto",
                                    tr!(
                                        lang,
                                        "跟随系统",
                                        "System default",
                                        "システムに従う",
                                        "시스템 설정"
                                    ),
                                    self.draft.app.language.is_empty(),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.draft.app.language.clear();
                                    cx.notify();
                                })),
                            )
                            .children(Lang::ALL.map(|option| {
                                small_button(
                                    SharedString::from(format!("lang-{}", option.code())),
                                    option.label(),
                                    self.draft.app.language == option.code(),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.draft.app.language = option.code().to_owned();
                                    cx.notify();
                                }))
                            })),
                    ),
            );

        let recording = panel()
            .gap_4()
            .child(panel_head(
                tr!(lang, "记录", "Recording", "記録", "기록"),
                tr!(
                    lang,
                    "开始和暂停也可以使用左侧固定按钮",
                    "Use the fixed button on the left to start or pause.",
                    "開始と一時停止は左側の固定ボタンでも操作できます。",
                    "시작과 일시정지는 왼쪽 고정 버튼으로도 가능합니다."
                ),
            ))
            .child(self.toggle_row(
                tr!(
                    lang,
                    "启用行为记录",
                    "Enable activity recording",
                    "行動記録を有効化",
                    "활동 기록 사용"
                ),
                tr!(
                    lang,
                    "与左侧主按钮保持同步",
                    "Stays in sync with the main button on the left",
                    "左側のメインボタンと同期します",
                    "왼쪽 기본 버튼과 동기화됩니다"
                ),
                self.draft.recording.enabled,
                cx.listener(|this, _, _, cx| {
                    this.draft.recording.enabled = !this.draft.recording.enabled;
                    cx.notify();
                }),
            ))
            .child(self.toggle_row(
                tr!(
                    lang,
                    "保存屏幕快照",
                    "Save screenshots",
                    "スクリーンショットを保存",
                    "스냅샷 저장"
                ),
                tr!(
                    lang,
                    "应用切换、定时心跳且画面变化时保存",
                    "Saved on app switches, heartbeat checks and screen changes",
                    "アプリ切替時・定期チェック時・画面変化時に保存",
                    "앱 전환, 정기 확인, 화면 변화 시 저장"
                ),
                self.draft.recording.screenshots,
                cx.listener(|this, _, _, cx| {
                    this.draft.recording.screenshots = !this.draft.recording.screenshots;
                    cx.notify();
                }),
            ))
            .child(self.toggle_row(
                tr!(
                    lang,
                    "记录浏览网站",
                    "Record visited sites",
                    "訪問サイトを記録",
                    "방문 사이트 기록"
                ),
                tr!(
                    lang,
                    "默认关闭。仅保存当前浏览器域名，不含路径和查询参数",
                    "Off by default. Only the current browser domain is saved—no paths or query strings.",
                    "既定ではオフ。現在のブラウザのドメインのみを保存し、パスやクエリは含みません。",
                    "기본은 꺼짐. 현재 브라우저 도메인만 저장하며 경로와 쿼리는 포함하지 않습니다."
                ),
                self.draft.recording.browser_sites,
                cx.listener(|this, _, _, cx| {
                    this.draft.recording.browser_sites = !this.draft.recording.browser_sites;
                    cx.notify();
                }),
            ))
            .child(setting_row(
                tr!(
                    lang,
                    "快照心跳间隔",
                    "Snapshot heartbeat",
                    "スナップショット間隔",
                    "스냅샷 주기"
                ),
                tr!(
                    lang,
                    "活跃期间定时检查画面，单位：分钟",
                    "How often to check the screen while active, in minutes",
                    "アクティブ中に画面を確認する間隔（分）",
                    "활동 중 화면 확인 주기(분)"
                ),
                self.form.heartbeat.clone(),
            ))
            .child(setting_row(
                tr!(
                    lang,
                    "空闲阈值",
                    "Idle threshold",
                    "アイドルしきい値",
                    "유휴 기준"
                ),
                tr!(
                    lang,
                    "无键鼠输入超过该分钟数后，不计入活跃时长",
                    "No keyboard or mouse input for this many minutes stops counting as active",
                    "キーボード・マウス操作がこの分数を超えるとアクティブに数えません",
                    "키보드/마우스 입력이 이 시간을 넘으면 활동으로 집계하지 않습니다"
                ),
                self.form.idle.clone(),
            ))
            .child(setting_row(
                tr!(
                    lang,
                    "会议与全屏空闲阈值",
                    "Meeting & fullscreen idle threshold",
                    "会議・全画面のしきい値",
                    "회의·전체 화면 유휴 기준"
                ),
                tr!(
                    lang,
                    "开会、看视频或放映时不碰键鼠也算活跃，单位：分钟",
                    "Meetings, videos and presentations count as active without input, in minutes",
                    "会議・動画・発表では操作がなくてもアクティブとみなします（分）",
                    "회의, 동영상, 발표 중에는 입력이 없어도 활동으로 봅니다(분)"
                ),
                self.form.passive_idle.clone(),
            ));
        let ai = panel()
            .gap_4()
            .child(panel_head(
                tr!(lang, "AI 分析", "AI analysis", "AI 分析", "AI 분석"),
                tr!(
                    lang,
                    "OpenAI 兼容接口，密钥用 Windows DPAPI 加密保存",
                    "OpenAI-compatible endpoint; keys are encrypted with Windows DPAPI",
                    "OpenAI 互換の接続先。キーは Windows DPAPI で暗号化して保存",
                    "OpenAI 호환 API, 키는 Windows DPAPI로 암호화해 저장"
                ),
            ))
            .child(setting_row(
                tr!(
                    lang,
                    "接口地址",
                    "Endpoint",
                    "接続先",
                    "API 주소"
                ),
                tr!(
                    lang,
                    "例如 https://api.deepseek.com/v1",
                    "e.g. https://api.deepseek.com/v1",
                    "例：https://api.deepseek.com/v1",
                    "예: https://api.deepseek.com/v1"
                ),
                self.form.base_url.clone(),
            ))
            .child(setting_row(
                "API Key",
                tr!(
                    lang,
                    "本地 Ollama 可以留空",
                    "Leave empty for local Ollama",
                    "ローカル Ollama なら空欄で構いません",
                    "로컬 Ollama는 비워 두세요"
                ),
                self.form.api_key.clone(),
            ))
            .child(setting_row(
                tr!(lang, "模型名", "Model", "モデル名", "모델명"),
                tr!(
                    lang,
                    "例如 qwen2.5vl、gpt-4o-mini。开启截图分析时必须支持图片输入",
                    "e.g. qwen2.5vl, gpt-4o-mini. Screenshot analysis needs image input.",
                    "例：qwen2.5vl、gpt-4o-mini。スクリーンショット分析には画像入力対応が必要です。",
                    "예: qwen2.5vl, gpt-4o-mini. 스냅샷 분석에는 이미지 입력 지원이 필요합니다."
                ),
                self.form.model.clone(),
            ))
            .child(self.toggle_row(
                tr!(
                    lang,
                    "自动分析屏幕截图",
                    "Auto-analyze screenshots",
                    "スクリーンショットを自動分析",
                    "스냅샷 자동 분석"
                ),
                tr!(
                    lang,
                    "按时段把截图交给上方模型总结，作为建议的依据。需要支持图片输入的多模态模型",
                    "Sends screenshots per interval to the model above as evidence for advice. Needs a multimodal model with image input.",
                    "時間帯ごとにスクリーンショットを上のモデルで要約し、提案の根拠にします。画像入力対応のマルチモーダルモデルが必要です。",
                    "구간별 스냅샷을 위 모델이 요약해 제안의 근거로 사용합니다. 이미지 입력을 지원하는 멀티모달 모델이 필요합니다."
                ),
                self.draft.ai.screenshot_analysis,
                cx.listener(|this, _, _, cx| {
                    this.draft.ai.screenshot_analysis = !this.draft.ai.screenshot_analysis;
                    cx.notify();
                }),
            ))
            .child(setting_row(
                tr!(
                    lang,
                    "截图分析间隔",
                    "Analysis interval",
                    "分析の間隔",
                    "분석 주기"
                ),
                tr!(
                    lang,
                    "5–240 分钟；每次最多 6 张，缩放后发送",
                    "5–240 minutes; up to 6 screenshots per run, scaled before sending",
                    "5〜240 分。1 回につき最大 6 枚、縮小して送信",
                    "5~240분, 회당 최대 6장, 축소해 전송"
                ),
                self.form.screenshot_interval.clone(),
            ))
            .child(self.toggle_row(
                tr!(
                    lang,
                    "每日自动生成",
                    "Daily auto-generate",
                    "毎日自動生成",
                    "매일 자동 생성"
                ),
                tr!(
                    lang,
                    "到点生成当天建议；错过时间或之后仍有大量活动会在次日补生成，不覆盖手动生成的建议",
                    "Generates the day's advice on schedule; missed runs or heavy later activity are caught up the next day, never overwriting manual advice",
                    "指定時刻にその日の提案を生成します。時刻を逃した場合やその後も活動が多い場合は翌日に補完し、手動生成の提案は上書きしません",
                    "정해진 시각에 그날의 제안을 생성합니다. 놓쳤거나 이후 활동이 많으면 다음 날 보완하며, 직접 생성한 제안은 덮어쓰지 않습니다"
                ),
                self.draft.ai.auto_report,
                cx.listener(|this, _, _, cx| {
                    this.draft.ai.auto_report = !this.draft.ai.auto_report;
                    cx.notify();
                }),
            ))
            .child(setting_row(
                tr!(
                    lang,
                    "自动生成时间",
                    "Generate at",
                    "生成時刻",
                    "생성 시각"
                ),
                tr!(
                    lang,
                    "24 小时制，例如 21:30",
                    "24-hour clock, e.g. 21:30",
                    "24 時間制（例：21:30）",
                    "24시간제, 예: 21:30"
                ),
                self.form.auto_time.clone(),
            ))
            .child(self.toggle_row(
                tr!(
                    lang,
                    "每日推送昨日建议",
                    "Daily digest of yesterday's advice",
                    "昨日の提案を毎日通知",
                    "어제 제안 매일 알림"
                ),
                tr!(
                    lang,
                    "到点发送系统通知：昨日结论和一句问候；昨天没有记录时不打扰",
                    "Sends a system notification with yesterday's conclusion and a short greeting; skipped when yesterday has no records",
                    "指定時刻に昨日の結論と一言をシステム通知で送ります。昨日の記録がない日は通知しません",
                    "정해진 시각에 어제의 결론과 짧은 인사를 시스템 알림으로 보냅니다. 어제 기록이 없으면 알리지 않습니다"
                ),
                self.draft.ai.daily_digest,
                cx.listener(|this, _, _, cx| {
                    this.draft.ai.daily_digest = !this.draft.ai.daily_digest;
                    cx.notify();
                }),
            ))
            .child(setting_row(
                tr!(lang, "推送时间", "Digest time", "通知時刻", "알림 시각"),
                tr!(
                    lang,
                    "24 小时制，默认 10:30",
                    "24-hour clock, 10:30 by default",
                    "24 時間制、既定は 10:30",
                    "24시간제, 기본 10:30"
                ),
                self.form.digest_time.clone(),
            ))
            .child(self.toggle_row(
                tr!(
                    lang,
                    "联网检索开源替代",
                    "Search for open-source alternatives",
                    "OSS 代替を検索",
                    "오픈소스 대안 검색"
                ),
                tr!(
                    lang,
                    "按抽象工具关键词补充候选，不发送行为记录",
                    "Adds candidates by abstract tool keywords; no activity data is sent",
                    "抽象化したツールのキーワードで候補を補います。行動記録は送信しません",
                    "추상화된 도구 키워드로 후보를 보완하며, 활동 기록은 전송하지 않습니다"
                ),
                self.draft.ai.web_search,
                cx.listener(|this, _, _, cx| {
                    this.draft.ai.web_search = !this.draft.ai.web_search;
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(setting_label(
                        tr!(
                            lang,
                            "搜索服务",
                            "Search provider",
                            "検索サービス",
                            "검색 서비스"
                        ),
                        tr!(
                            lang,
                            "DuckDuckGo 无需密钥；Brave 需要单独 API Key",
                            "DuckDuckGo needs no key; Brave needs its own API key",
                            "DuckDuckGo はキー不要、Brave は専用の API Key が必要",
                            "DuckDuckGo는 키가 필요 없고, Brave는 별도 API 키가 필요합니다"
                        ),
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                small_button(
                                    "provider-ddg",
                                    "DuckDuckGo",
                                    self.draft.ai.search_provider != "brave",
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.draft.ai.search_provider = "duckduckgo".to_owned();
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                small_button(
                                    "provider-brave",
                                    "Brave",
                                    self.draft.ai.search_provider == "brave",
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.draft.ai.search_provider = "brave".to_owned();
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            )
            .child(setting_row(
                tr!(
                    lang,
                    "搜索 API Key",
                    "Search API key",
                    "検索 API Key",
                    "검색 API 키"
                ),
                tr!(
                    lang,
                    "仅 Brave Search 需要，与 AI 密钥分开保存",
                    "Only for Brave Search, stored apart from the AI key",
                    "Brave Search のみ。AI のキーとは別に保存",
                    "Brave Search 전용, AI 키와 따로 저장"
                ),
                self.form.search_key.clone(),
            ))
            .child(
                button(
                    "ai-test",
                    if self.testing {
                        tr!(lang, "正在测试…", "Testing…", "テスト中…", "테스트 중…")
                    } else {
                        tr!(
                            lang,
                            "保存并测试连接",
                            "Save and test connection",
                            "保存して接続テスト",
                            "저장 후 연결 테스트"
                        )
                    },
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.test_ai(cx);
                    cx.notify();
                })),
            );
        let storage = panel()
            .gap_4()
            .child(panel_head(
                tr!(
                    lang,
                    "隐私与存储",
                    "Privacy & storage",
                    "プライバシーと保存",
                    "개인정보 및 저장"
                ),
                tr!(
                    lang,
                    "敏感窗口不记录标题、不截图",
                    "Sensitive windows keep no title and no screenshot",
                    "機密ウィンドウはタイトルを記録せず、撮影もしません",
                    "민감한 창은 제목을 기록하지 않고 스냅샷도 찍지 않습니다"
                ),
            ))
            .child(setting_row(
                tr!(
                    lang,
                    "敏感内容关键词",
                    "Sensitive keywords",
                    "機密キーワード",
                    "민감 키워드"
                ),
                tr!(
                    lang,
                    "用逗号分隔。前台应用、标题或网站命中时，标题写为“[敏感窗口]”，且不截图、不记录网站",
                    "Comma-separated. When an app, title or site matches, the title becomes “[敏感窗口]”, and no screenshot or site is recorded.",
                    "カンマ区切り。前面のアプリ・タイトル・サイトが一致すると、タイトルは「[敏感窗口]」になり、撮影もサイト記録も行いません。",
                    "쉼표로 구분합니다. 전면 앱, 제목, 사이트가 일치하면 제목이 “[敏感窗口]”으로 저장되고 스냅샷과 사이트는 기록하지 않습니다."
                ),
                self.form.patterns.clone(),
            ))
            .child(setting_row(
                tr!(
                    lang,
                    "快照占用上限（GB）",
                    "Snapshot limit (GB)",
                    "スナップショット上限（GB）",
                    "스냅샷 한도(GB)"
                ),
                &fill(
                    tr!(
                        lang,
                        "超出后删除最旧日期。当前 {}",
                        "Oldest dates are deleted beyond this. Currently {}",
                        "超えた分は古い日付から削除します。現在 {}",
                        "초과하면 오래된 날짜부터 삭제합니다. 현재 {}"
                    ),
                    &[&fmt_bytes(self.status.total_snapshot_bytes)],
                ),
                self.form.cap.clone(),
            ))
            .child(setting_row(
                tr!(
                    lang,
                    "数据保留天数",
                    "Keep data for (days)",
                    "保存日数",
                    "보관 일수"
                ),
                tr!(
                    lang,
                    "超期的每日记录和快照会自动清理",
                    "Older daily records and snapshots are cleaned up automatically",
                    "期限を過ぎた日別記録とスナップショットは自動で削除します",
                    "기간이 지난 일별 기록과 스냅샷은 자동으로 정리됩니다"
                ),
                self.form.retention.clone(),
            ))
            .child(self.toggle_row(
                tr!(
                    lang,
                    "开机自启",
                    "Start with Windows",
                    "Windows 起動時に開始",
                    "Windows 시작 시 실행"
                ),
                tr!(
                    lang,
                    "登录 Windows 后启动 TraceBit",
                    "Launch TraceBit after you sign in",
                    "Windows にサインインすると TraceBit を起動",
                    "Windows 로그인 후 TraceBit를 실행"
                ),
                self.draft.app.autostart,
                cx.listener(|this, _, _, cx| {
                    this.draft.app.autostart = !this.draft.app.autostart;
                    cx.notify();
                }),
            ))
            .child(self.toggle_row(
                tr!(
                    lang,
                    "关闭时最小化到托盘",
                    "Minimize to tray on close",
                    "閉じたらトレイに格納",
                    "닫을 때 트레이로 최소화"
                ),
                tr!(
                    lang,
                    "关闭窗口后继续在后台记录",
                    "Keeps recording in the background after closing the window",
                    "ウィンドウを閉じてもバックグラウンドで記録を続けます",
                    "창을 닫아도 백그라운드에서 계속 기록합니다"
                ),
                self.draft.app.close_to_tray,
                cx.listener(|this, _, _, cx| {
                    this.draft.app.close_to_tray = !this.draft.app.close_to_tray;
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgb(SECONDARY))
                            .child(self.status.data_dir.clone()),
                    )
                    .child(
                        small_button(
                            "open-data",
                            tr!(
                                lang,
                                "打开目录",
                                "Open folder",
                                "フォルダーを開く",
                                "폴더 열기"
                            ),
                            false,
                        )
                        .on_click(cx.listener(|this, _, _, _| {
                            let _ = open::that(&this.store.data_dir);
                        })),
                    )
                    .child(
                        small_button(
                            "purge-shots",
                            tr!(
                                lang,
                                "清空快照",
                                "Clear snapshots",
                                "撮影を消去",
                                "스냅샷 비우기"
                            ),
                            false,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.purge(window, true);
                            cx.notify();
                        })),
                    )
                    .child(
                        small_button(
                            "purge-all",
                            tr!(
                                lang,
                                "清空全部记录",
                                "Clear all records",
                                "すべての記録を消去",
                                "모든 기록 비우기"
                            ),
                            false,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.purge(window, false);
                            cx.notify();
                        })),
                    ),
            );
        page.child(language)
            .child(recording)
            .child(ai)
            .child(storage)
            .child(
                button(
                    "save-settings",
                    tr!(lang, "保存设置", "Save settings", "設定を保存", "설정 저장"),
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.save_settings_with_cx(cx);
                    cx.notify();
                })),
            )
    }

    fn toggle_row(
        &self,
        name: &str,
        description: &str,
        enabled: bool,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Div {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(setting_label(name, description))
            .child(
                div()
                    .id(SharedString::from(format!("toggle-{name}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .tab_index(0)
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(SURFACE))
                    .focus(|style| style.border_color(rgb(ACCENT)))
                    .on_click(on_click)
                    .child(
                        div()
                            .w(px(38.))
                            .h(px(22.))
                            .rounded_full()
                            .p(px(2.))
                            .bg(rgb(if enabled { GREEN } else { DIVIDER }))
                            .child(
                                div()
                                    .size(px(18.))
                                    .rounded_full()
                                    .bg(gpui::white())
                                    .ml(if enabled { px(16.) } else { px(0.) }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(SECONDARY))
                            .child(if enabled {
                                tr!(self.lang, "已开启", "On", "オン", "켜짐")
                            } else {
                                tr!(self.lang, "已关闭", "Off", "オフ", "꺼짐")
                            }),
                    ),
            )
    }

    fn render_lightbox(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let lang = self.lang;
        let index = self.lightbox?;
        let shot = self.day.screenshots.get(index)?;
        let path = self.store.snapshot_path(&shot.file)?;
        let title_note = shot
            .title
            .as_deref()
            .map(|title| format!(" · {}", crate::redact::display_title(lang, title)))
            .unwrap_or_default();
        let viewport = window.viewport_size();
        let viewer_width = (viewport.width - px(64.)).max(px(320.)).min(px(1120.));
        let image_width = (viewer_width - px(32.)).max(px(288.));
        let image_height = (viewport.height - px(236.)).max(px(160.)).min(px(650.));
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x000000b8))
                .p_8()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(viewer_width)
                        .bg(rgb(SURFACE))
                        .border_1()
                        .border_color(rgb(DIVIDER))
                        .rounded_lg()
                        .overflow_hidden()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_4()
                                .py_3()
                                .border_b_1()
                                .border_color(rgb(DIVIDER))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(tr!(
                                                    lang,
                                                    "屏幕快照",
                                                    "Screenshot",
                                                    "スクリーンショット",
                                                    "스냅샷"
                                                )),
                                        )
                                        .child(div().text_xs().text_color(rgb(SECONDARY)).child(
                                            fill(
                                                tr!(
                                                    lang,
                                                    "{} · {} · {}/{}{}",
                                                    "{} · {} · {}/{}{}",
                                                    "{} · {} · {}/{}{}",
                                                    "{} · {} · {}/{}{}"
                                                ),
                                                &[
                                                    &hhmm(shot.ts),
                                                    &trigger_label(lang, &shot.trigger),
                                                    &(index + 1),
                                                    &self.day.screenshots.len(),
                                                    &title_note,
                                                ],
                                            ),
                                        )),
                                )
                                .child(
                                    small_button(
                                        "lightbox-close",
                                        tr!(lang, "关闭", "Close", "閉じる", "닫기"),
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.lightbox = None;
                                            cx.notify();
                                        },
                                    )),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w_full()
                                .p_4()
                                .bg(rgb(0x17191b))
                                .child(
                                    img(path)
                                        .max_w(image_width)
                                        .max_h(image_height)
                                        .rounded_md(),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_4()
                                .py_3()
                                .border_t_1()
                                .border_color(rgb(DIVIDER))
                                .child(
                                    small_button(
                                        "lightbox-open",
                                        tr!(
                                            lang,
                                            "系统看图",
                                            "Open in viewer",
                                            "既定のビューアで開く",
                                            "기본 뷰어로 열기"
                                        ),
                                        false,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, _| {
                                            if let Some(shot) = this
                                                .lightbox
                                                .and_then(|index| this.day.screenshots.get(index))
                                                && let Some(path) =
                                                    this.store.snapshot_path(&shot.file)
                                            {
                                                let _ = open::that(path);
                                            }
                                        },
                                    )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            small_button(
                                                "lightbox-prev",
                                                tr!(lang, "上一张", "Previous", "前へ", "이전"),
                                                false,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.move_lightbox(-1);
                                                    cx.notify();
                                                }),
                                            ),
                                        )
                                        .child(
                                            small_button(
                                                "lightbox-next",
                                                tr!(lang, "下一张", "Next", "次へ", "다음"),
                                                false,
                                            )
                                            .on_click(
                                                cx.listener(|this, _, _, cx| {
                                                    this.move_lightbox(1);
                                                    cx.notify();
                                                }),
                                            ),
                                        ),
                                ),
                        ),
                ),
        )
    }

    fn move_lightbox(&mut self, delta: i64) {
        let count = self.day.screenshots.len() as i64;
        if count == 0 {
            self.lightbox = None;
            return;
        }
        let current = self.lightbox.unwrap_or(0) as i64;
        self.lightbox = Some(((current + delta + count) % count) as usize);
    }
}

impl Render for TraceBitWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.view {
            View::Today => self.render_today(cx),
            View::Stats => self.render_stats(cx),
            View::Report => self.render_report(cx),
            View::Settings => self.render_settings(cx),
        };
        div()
            .relative()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_today))
            .on_action(cx.listener(Self::open_stats))
            .on_action(cx.listener(Self::open_report))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::toggle_recording_from_key))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_previous))
            .flex()
            .size_full()
            .bg(rgb(CANVAS))
            .text_color(rgb(INK))
            .font_family("Microsoft YaHei UI")
            .child(self.render_sidebar(cx))
            .child(
                div()
                    .id("workspace")
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .overflow_y_scroll()
                    .p_7()
                    .child(content),
            )
            .children(self.render_lightbox(window, cx))
            .children(self.render_toast(cx))
    }
}

struct TextTooltip(String);

impl Render for TextTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().p_2().text_xs().child(self.0.clone())
    }
}

fn create_tray(active: bool, lang: Lang) -> Option<TrayState> {
    let status = MenuItem::with_id(
        "status",
        if active {
            tr!(lang, "正在记录", "Recording", "記録中", "기록 중")
        } else {
            tr!(lang, "已暂停", "Paused", "一時停止中", "일시정지됨")
        },
        false,
        None,
    );
    let toggle = MenuItem::with_id(
        "toggle",
        if active {
            tr!(
                lang,
                "暂停记录",
                "Pause recording",
                "記録を一時停止",
                "기록 일시정지"
            )
        } else {
            tr!(
                lang,
                "恢复记录",
                "Resume recording",
                "記録を再開",
                "기록 재개"
            )
        },
        true,
        None,
    );
    let show = MenuItem::with_id(
        "show",
        tr!(
            lang,
            "显示主窗口",
            "Show window",
            "メインウィンドウを表示",
            "메인 창 표시"
        ),
        true,
        None,
    );
    let report = MenuItem::with_id(
        "report",
        tr!(
            lang,
            "查看工具升级建议",
            "View tool upgrade advice",
            "ツール改善の提案を見る",
            "도구 개선 제안 보기"
        ),
        true,
        None,
    );
    let quit = MenuItem::with_id(
        "quit",
        tr!(lang, "退出", "Quit", "終了", "종료"),
        true,
        None,
    );
    let menu = Menu::new();
    menu.append_items(&[&status, &toggle, &show, &report, &quit])
        .ok()?;
    let image = image::load_from_memory(include_bytes!("../assets/tray.png")).ok()?;
    let (width, height) = (image.width(), image.height());
    let icon = Icon::from_rgba(image.into_rgba8().into_raw(), width, height).ok()?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("TraceBit")
        .with_icon(icon)
        .build()
        .ok()?;
    Some(TrayState {
        tray,
        toggle,
        status,
    })
}

fn load_day(store: &DayStore, settings: &Settings, date_key: &str) -> DayStats {
    let recording = settings.get().recording;
    store
        .day_stats(date_key, DeriveOptions::from_settings(&recording))
        .unwrap_or_else(|error| {
            eprintln!("[Store] {error:#}");
            store.derive(&[], DeriveOptions::default())
        })
}

fn nav_button(id: impl Into<ElementId>, label: &str, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_2()
        .rounded_md()
        .tab_index(0)
        .text_sm()
        .bg(rgb(if selected { BRAND_PALE } else { SURFACE }))
        .text_color(rgb(if selected { INK } else { SECONDARY }))
        .border_1()
        .border_color(rgb(if selected { BRAND_PALE } else { SURFACE }))
        .focus(|style| style.border_color(rgb(ACCENT)))
        .hover(|style| style.bg(rgb(BRAND_PALE)))
        .child(label.to_owned())
}

fn button(id: impl Into<ElementId>, label: &str, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .justify_center()
        .items_center()
        .h(px(40.))
        .px_4()
        .rounded_md()
        .tab_index(0)
        .text_sm()
        .bg(rgb(if primary { INK } else { SURFACE }))
        .text_color(rgb(if primary { 0xffffff } else { INK }))
        .border_1()
        .border_color(rgb(if primary { INK } else { DIVIDER }))
        .focus(|style| style.border_color(rgb(ACCENT)))
        .active(|style| style.opacity(0.85))
        .child(label.to_owned())
}

fn small_button(id: impl Into<ElementId>, label: &str, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(32.))
        .px_3()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .tab_index(0)
        .text_xs()
        .bg(rgb(if selected { BRAND_PALE } else { SURFACE }))
        .border_1()
        .border_color(rgb(DIVIDER))
        .focus(|style| style.border_color(rgb(ACCENT)))
        .child(label.to_owned())
}

fn section() -> Div {
    div().flex().flex_col().w_full().min_w(px(0.))
}

fn panel() -> Div {
    div()
        .flex()
        .flex_col()
        .bg(rgb(SURFACE))
        .border_1()
        .border_color(rgb(DIVIDER))
        .rounded_lg()
        .p_5()
}

fn panel_head(title: &str, hint: &str) -> Div {
    div()
        .flex()
        .justify_between()
        .items_center()
        .child(
            div()
                .text_size(px(18.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.to_owned()),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(FAINT))
                .child(hint.to_owned()),
        )
}

fn metric(label: &str, value: &str, note: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .gap_1()
        .px_4()
        .child(
            div()
                .text_xs()
                .text_color(rgb(SECONDARY))
                .child(label.to_owned()),
        )
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child(value.to_owned()),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(FAINT))
                .child(note.to_owned()),
        )
}

fn setting_label(name: &str, description: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .gap_1()
        .max_w(px(520.))
        .child(div().text_sm().child(name.to_owned()))
        .child(
            div()
                .text_xs()
                .text_color(rgb(SECONDARY))
                .child(description.to_owned()),
        )
}

fn setting_row(name: &str, description: &str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(setting_label(name, description))
        .child(div().w(px(360.)).child(control))
}

fn has_ai(settings: &SettingsData) -> bool {
    !settings.ai.base_url.trim().is_empty() && !settings.ai.model.trim().is_empty()
}

const MONTHS_EN: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn month_text(lang: Lang, month: u32) -> String {
    match lang {
        Lang::En => MONTHS_EN[(month - 1) as usize].to_owned(),
        _ => month.to_string(),
    }
}

fn full_date(lang: Lang, date: NaiveDate) -> String {
    let weekday = weekday_name(lang, date.weekday().num_days_from_sunday());
    fill(
        tr!(
            lang,
            "{}月{}日 · {}",
            "{} {} · {}",
            "{}月{}日 · {}",
            "{}월 {}일 · {}"
        ),
        &[&month_text(lang, date.month()), &date.day(), &weekday],
    )
}

fn fmt_ms(lang: Lang, ms: i64) -> String {
    let minutes = (ms as f64 / 60_000.0).round() as i64;
    if minutes < 60 {
        return fill(tr!(lang, "{}分钟", "{} min", "{}分", "{}분"), &[&minutes]);
    }
    let hours = minutes / 60;
    let rest = minutes % 60;
    if rest == 0 {
        fill(tr!(lang, "{}小时", "{} hr", "{}時間", "{}시간"), &[&hours])
    } else {
        fill(
            tr!(
                lang,
                "{}小时{}分",
                "{} hr {} min",
                "{}時間{}分",
                "{}시간 {}분"
            ),
            &[&hours, &rest],
        )
    }
}

fn weekday_name(lang: Lang, index: u32) -> &'static str {
    match index % 7 {
        0 => tr!(lang, "周日", "Sun", "日", "일"),
        1 => tr!(lang, "周一", "Mon", "月", "월"),
        2 => tr!(lang, "周二", "Tue", "火", "화"),
        3 => tr!(lang, "周三", "Wed", "水", "수"),
        4 => tr!(lang, "周四", "Thu", "木", "목"),
        5 => tr!(lang, "周五", "Fri", "金", "금"),
        _ => tr!(lang, "周六", "Sat", "土", "토"),
    }
}

fn month_day(lang: Lang, date_key: &str) -> String {
    match NaiveDate::parse_from_str(date_key, "%Y-%m-%d") {
        Ok(date) => fill(
            tr!(lang, "{}月{}日", "{} {}", "{}月{}日", "{}월 {}일"),
            &[&month_text(lang, date.month()), &date.day()],
        ),
        Err(_) => date_key.to_owned(),
    }
}

fn fmt_bytes(bytes: u64) -> String {
    if bytes < 1024_u64.pow(3) {
        format!("{:.1} MB", bytes as f64 / 1024_f64.powi(2))
    } else {
        format!("{:.2} GB", bytes as f64 / 1024_f64.powi(3))
    }
}

fn category_color(category: &str) -> u32 {
    match category {
        "开发" => 0x4277a8,
        "浏览器" => 0x3f897d,
        "即时通讯" => 0xb37b24,
        "办公文档" => 0x8a6aa8,
        "会议" => 0xb95b5b,
        "设计" => 0xb86e52,
        "娱乐" => 0x64864f,
        "资讯社交" => 0x5f7f99,
        "工具" => 0x7c8285,
        "系统" => 0x8c8c86,
        _ => 0x92928b,
    }
}

fn trigger_label(lang: Lang, trigger: &str) -> String {
    match trigger {
        "app_focus" => tr!(lang, "切换", "Switch", "切替", "전환"),
        "heartbeat" => tr!(lang, "定时", "Timer", "定時", "정기"),
        "resume" => tr!(lang, "返回", "Resume", "復帰", "복귀"),
        "manual" => tr!(lang, "手动", "Manual", "手動", "수동"),
        _ => trigger,
    }
    .to_owned()
}

fn report_excerpt(markdown: &str) -> String {
    markdown
        .chars()
        .filter(|ch| !"#*>`[]()_-\n\r".contains(*ch))
        .take(116)
        .collect()
}

pub fn run() -> Result<()> {
    #[cfg(windows)]
    let _instance = platform::single_instance()?;
    #[cfg(windows)]
    if _instance.is_none() {
        return Ok(());
    }
    let user_dir = crate::user_data_dir();
    fs::create_dir_all(&user_dir)?;
    let settings = Arc::new(Settings::load(&user_dir));
    let store = Arc::new(DayStore::new(user_dir.join("data"))?);
    let recorder = Recorder::new(settings.clone(), store.clone());
    let analyzer = Arc::new(
        Analyzer::new(settings.clone(), store.clone(), user_dir.join("reports"))?
            .with_intervals(user_dir.join("intervals")),
    );
    let interval_analyzer = Arc::new(IntervalAnalyzer::new(
        settings.clone(),
        store.clone(),
        user_dir.join("intervals"),
    )?);
    if settings.get().app.autostart {
        let _ = platform::set_autostart(true);
    }
    if settings.get().recording.enabled {
        recorder.start()?;
    }
    let (auto_tx, auto_rx) = std::sync::mpsc::channel();
    spawn_auto_report(settings.clone(), analyzer.clone(), auto_tx);
    let (interval_tx, interval_rx) = std::sync::mpsc::channel();
    spawn_interval_analysis(settings.clone(), interval_analyzer.clone(), interval_tx);
    let quitting = Arc::new(AtomicBool::new(false));
    let recorder_for_quit = recorder.clone();
    Application::new().run(move |cx| {
        input::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("ctrl-1", ShowToday, None),
            KeyBinding::new("ctrl-4", ShowStats, None),
            KeyBinding::new("ctrl-2", ShowReport, None),
            KeyBinding::new("ctrl-3", ShowSettings, None),
            KeyBinding::new("ctrl-,", ShowSettings, None),
            KeyBinding::new("ctrl-shift-r", ToggleRecording, None),
            KeyBinding::new("ctrl-w", CloseWindow, None),
            KeyBinding::new("escape", Dismiss, None),
            KeyBinding::new("tab", FocusNext, None),
            KeyBinding::new("shift-tab", FocusPrevious, None),
        ]);
        let bounds = Bounds::centered(None, size(px(1240.), px(840.)), cx);
        let lang = settings.get().lang();
        let settings_for_close = settings.clone();
        let recorder_for_close = recorder.clone();
        let quitting_for_close = quitting.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.), px(620.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("TraceBit".into()),
                    ..Default::default()
                }),
                app_id: Some("TraceBit.Desktop".to_owned()),
                ..Default::default()
            },
            move |window, cx| {
                platform::set_window_icon(window);
                window.on_window_should_close(cx, move |window, _| {
                    if quitting_for_close.load(Ordering::Acquire) {
                        return true;
                    }
                    if settings_for_close.get().app.close_to_tray {
                        platform::set_window_visible(window, false);
                        false
                    } else {
                        recorder_for_close.stop();
                        true
                    }
                });
                cx.new(|cx| {
                    TraceBitWindow::new(
                        window,
                        cx,
                        settings.clone(),
                        store.clone(),
                        recorder.clone(),
                        analyzer.clone(),
                        interval_analyzer.clone(),
                        auto_rx,
                        interval_rx,
                        quitting.clone(),
                    )
                })
            },
        )
        .expect(tr!(
            lang,
            "无法创建 TraceBit 窗口",
            "Couldn't create the TraceBit window",
            "TraceBit ウィンドウを作成できません",
            "TraceBit 창을 만들 수 없습니다"
        ));
        cx.activate(true);
        let recorder = recorder_for_quit.clone();
        cx.on_app_quit(move |_| {
            recorder.stop();
            async {}
        })
        .detach();
    });
    Ok(())
}

/// Generates advice automatically: today once the scheduled time has passed, and
/// yesterday if it was missed (machine asleep, app started late) or its automatic
/// report predates substantial later activity. Manual reports are never replaced.
fn spawn_auto_report(
    settings: Arc<Settings>,
    analyzer: Arc<Analyzer>,
    sender: std::sync::mpsc::Sender<String>,
) {
    thread::spawn(move || {
        let mut retry_after: BTreeMap<String, std::time::Instant> = BTreeMap::new();
        let mut first = true;
        // Learn recent days once, independent of whether AI is configured.
        let _ = analyzer.backfill_profile(30);
        let mut last_observed: Option<std::time::Instant> = None;
        loop {
            if !first {
                thread::sleep(Duration::from_secs(60));
            }
            first = false;
            let now = Local::now();
            let today = now.format("%Y-%m-%d").to_string();
            // Keep today's profile entry fresh without recomputing every minute.
            if last_observed.is_none_or(|at| at.elapsed() >= Duration::from_secs(600)) {
                last_observed = Some(std::time::Instant::now());
                let _ = analyzer.observe_profile(&today);
            }
            let current = settings.get();
            let lang = current.app.lang();
            let ai = current.ai;
            // Daily digest: yesterday's advice plus a greeting, once a day. It
            // runs whether or not automatic generation is enabled.
            if ai.daily_digest {
                let today_key = today.clone();
                if let Some(scheduled) =
                    crate::analyzer::scheduled_ts(&today_key, &ai.daily_digest_time)
                    && crate::digest::is_due(
                        now.timestamp_millis(),
                        scheduled,
                        analyzer.last_digest().as_deref(),
                        &today_key,
                    )
                {
                    let yesterday = (now - ChronoDuration::days(1))
                        .format("%Y-%m-%d")
                        .to_string();
                    let mut report = analyzer.get_report(&yesterday);
                    if report.is_none()
                        && ai.auto_report
                        && ai.text_endpoint().configured()
                        && analyzer.needs_auto_report(&yesterday, scheduled)
                    {
                        report = analyzer.generate_report_as(&yesterday, "auto").ok();
                    }
                    let has_activity = analyzer.has_activity(&yesterday);
                    let digest = match &report {
                        Some(report) => {
                            let advice = report.advice.as_ref();
                            crate::digest::Digest {
                                headline: advice
                                    .map(|advice| advice.headline.as_str())
                                    .unwrap_or(""),
                                top_upgrade: advice
                                    .and_then(|advice| advice.upgrades.first())
                                    .map(|upgrade| upgrade.title.as_str())
                                    .unwrap_or(""),
                                upgrades: advice.map(|advice| advice.upgrades.len()).unwrap_or(0),
                                has_report: true,
                                has_activity,
                            }
                        }
                        None => crate::digest::Digest {
                            headline: "",
                            top_upgrade: "",
                            upgrades: 0,
                            has_report: false,
                            has_activity,
                        },
                    };
                    if let Some((title, body)) = crate::digest::compose(now, Some(digest), lang) {
                        platform::show_notification(&title, &body);
                    }
                    let _ = analyzer.mark_digest_sent(&today_key);
                }
            }
            if !ai.auto_report || !ai.text_endpoint().configured() {
                continue;
            }
            let yesterday = (now - ChronoDuration::days(1))
                .format("%Y-%m-%d")
                .to_string();
            let mut due = Vec::new();
            if let Some(scheduled) = crate::analyzer::scheduled_ts(&yesterday, &ai.auto_report_time)
                && analyzer.needs_auto_report(&yesterday, scheduled)
            {
                due.push(yesterday);
            }
            if let Some(scheduled) = crate::analyzer::scheduled_ts(&today, &ai.auto_report_time)
                && now.timestamp_millis() >= scheduled
                && analyzer
                    .get_report(&today)
                    .is_none_or(|report| report.source == "auto" && report.generated_at < scheduled)
                && analyzer.needs_auto_report(&today, scheduled)
            {
                due.push(today);
            }
            for date_key in due {
                if retry_after
                    .get(&date_key)
                    .is_some_and(|until| *until > std::time::Instant::now())
                {
                    continue;
                }
                match analyzer.generate_report_as(&date_key, "auto") {
                    Ok(_) => {
                        retry_after.remove(&date_key);
                        platform::show_notification(
                            tr!(
                                lang,
                                "TraceBit 工具升级建议已生成",
                                "TraceBit tool upgrade advice is ready",
                                "TraceBit のツール改善提案ができました",
                                "TraceBit 도구 개선 제안이 준비되었습니다"
                            ),
                            &fill(
                                tr!(
                                    lang,
                                    "{} 的工具升级建议已就绪",
                                    "Advice for {} is ready",
                                    "{} のツール改善提案ができました",
                                    "{} 도구 개선 제안이 준비되었습니다"
                                ),
                                &[&date_key],
                            ),
                        );
                        let _ = sender.send(date_key);
                    }
                    Err(error) => {
                        eprintln!("[AutoReport] {error:#}");
                        retry_after.insert(
                            date_key,
                            std::time::Instant::now() + Duration::from_secs(30 * 60),
                        );
                    }
                }
            }
        }
    });
}

fn spawn_interval_analysis(
    settings: Arc<Settings>,
    analyzer: Arc<IntervalAnalyzer>,
    sender: std::sync::mpsc::Sender<std::result::Result<String, String>>,
) {
    thread::spawn(move || {
        let mut retry_after = None::<std::time::Instant>;
        let mut previous_config = String::new();
        loop {
            thread::sleep(Duration::from_secs(30));
            let ai = settings.get().ai;
            let config = format!(
                "{}|{}|{}|{}",
                ai.base_url, ai.model, ai.screenshot_analysis, ai.screenshot_analysis_interval_ms
            );
            if config != previous_config {
                retry_after = None;
                previous_config = config;
            }
            if !ai.screenshot_analysis || !ai.text_endpoint().configured() {
                continue;
            }
            if retry_after.is_some_and(|until| until > std::time::Instant::now()) {
                continue;
            }
            match analyzer.analyze_due(now_ms()) {
                Ok(Some(summary)) => {
                    retry_after = None;
                    let _ = sender.send(Ok(local_date_key(summary.end)));
                }
                Ok(None) => retry_after = None,
                Err(error) => {
                    eprintln!("[IntervalAnalysis] {error:#}");
                    let _ = sender.send(Err(format!("{error:#}")));
                    retry_after = Some(std::time::Instant::now() + Duration::from_secs(900));
                }
            }
        }
    });
}

pub fn native_smoke() -> Result<()> {
    let root = std::env::temp_dir().join(format!("tracebit-native-smoke-{}", now_ms()));
    fs::create_dir_all(&root)?;
    let settings = Arc::new(Settings::load(&root));
    let lang = settings.get().lang();
    let store = Arc::new(DayStore::new(root.join("data"))?);
    let recorder = Recorder::new(settings, store.clone());
    recorder.start()?;
    thread::sleep(Duration::from_secs(8));
    let status = recorder.status();
    recorder.stop();
    let stats = store.day_stats(&local_date_key(now_ms()), DeriveOptions::default())?;
    println!(
        "{}",
        fill(
            tr!(
                lang,
                "前台应用：{}",
                "Foreground app: {}",
                "前面のアプリ：{}",
                "전면 앱: {}"
            ),
            &[&format!(
                "{:?}",
                status.frontmost.as_ref().map(|sample| &sample.app)
            )],
        )
    );
    println!(
        "{}",
        fill(
            tr!(
                lang,
                "原生事件：{}，截图：{}",
                "Native events: {}, screenshots: {}",
                "ネイティブイベント：{}、スクリーンショット：{}",
                "네이티브 이벤트: {}, 스냅샷: {}"
            ),
            &[&stats.events, &stats.screenshots.len()],
        )
    );
    if stats.events == 0 {
        anyhow::bail!(
            "{}",
            tr!(
                lang,
                "未采集到原生事件",
                "no native events were captured",
                "ネイティブイベントを取得できませんでした",
                "네이티브 이벤트를 수집하지 못했습니다"
            )
        );
    }
    if stats.screenshots.is_empty() {
        anyhow::bail!(
            "{}",
            tr!(
                lang,
                "未生成屏幕快照",
                "no screenshots were saved",
                "スクリーンショットが保存されませんでした",
                "스냅샷이 저장되지 않았습니다"
            )
        );
    }
    fs::remove_dir_all(root)?;
    println!(
        "{}",
        tr!(
            lang,
            "原生采集 smoke 通过",
            "native capture smoke passed",
            "ネイティブ取得の smoke に合格しました",
            "네이티브 수집 smoke 통과"
        )
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_helpers_follow_the_language() {
        assert_eq!(fmt_ms(Lang::Zh, 90 * 60_000), "1小时30分");
        assert_eq!(fmt_ms(Lang::En, 90 * 60_000), "1 hr 30 min");
        assert_eq!(fmt_ms(Lang::Ja, 120 * 60_000), "2時間");
        assert_eq!(fmt_ms(Lang::Ko, 45 * 60_000), "45분");

        let date = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        assert_eq!(full_date(Lang::Zh, date), "11月20日 · 周五");
        assert_eq!(full_date(Lang::En, date), "Nov 20 · Fri");
        assert_eq!(full_date(Lang::Ja, date), "11月20日 · 金");
        assert_eq!(full_date(Lang::Ko, date), "11월 20일 · 금");
        assert_eq!(month_day(Lang::En, "2026-11-05"), "Nov 5");
        assert_eq!(weekday_name(Lang::Ko, 0), "일");
    }
}
