use crate::{
    analyzer::{Analyzer, Report, ReportSummary},
    input::{self, TextInput},
    intervals::{IntervalAnalyzer, IntervalSummary, SentRequest},
    platform,
    recorder::{Recorder, RecorderStatus},
    settings::{Settings, SettingsData},
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
    fn new(cx: &mut Context<TraceBitWindow>, settings: &SettingsData) -> Self {
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
                "sk-…，本地模型可留空",
                true,
            ),
            model: input(cx, settings.ai.model.clone(), "deepseek-chat", false),
            auto_time: input(cx, settings.ai.auto_report_time.clone(), "21:30", false),
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
    date: NaiveDate,
    day: DayStats,
    report: Option<Report>,
    report_list: Vec<ReportSummary>,
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
        let interval_summaries = interval_analyzer.list(&local_date_key(now_ms()));
        let interval_request = interval_analyzer.last_request();
        let status = recorder.status();
        let form = FormState::new(cx, &current);
        let tray = create_tray(status.running && status.enabled);
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
            date,
            day,
            report,
            report_list,
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
            self.notice = Some(("效率建议已自动生成".to_owned(), true));
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
                Some((self.date_key(), Err(anyhow::anyhow!("建议任务意外结束"))))
            }
            _ => None,
        };
        if let Some((date_key, result)) = result {
            self.report_rx = None;
            self.generating = false;
            match result {
                Ok(report) => {
                    self.report = Some(report);
                    self.notice = Some(("效率建议已生成".to_owned(), true));
                    self.report_list = self.analyzer.list_reports();
                }
                Err(error) => {
                    self.report_error = Some(format!("{error:#}"));
                    self.notice = Some((format!("生成失败：{error:#}"), false));
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
            Some(Err(TryRecvError::Disconnected)) => Some(Err(anyhow::anyhow!("连接测试意外结束"))),
            _ => None,
        };
        if let Some(result) = result {
            self.test_rx = None;
            self.testing = false;
            match result {
                Ok((_, model)) => self.notice = Some((format!("连接正常：{model}"), true)),
                Err(error) => self.notice = Some((format!("连接失败：{error:#}"), false)),
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
        if let Some(tray) = &self.tray {
            let active = self.status.enabled && self.status.running;
            let _ = tray.tray.set_tooltip(Some(if active {
                "TraceBit — 正在记录"
            } else {
                "TraceBit — 已暂停"
            }));
            tray.toggle.set_text(if active {
                "暂停记录"
            } else {
                "恢复记录"
            });
            tray.status
                .set_text(if active { "正在记录" } else { "已暂停" });
        }
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
            Err(error) => self.notice = Some((format!("无法生成预览：{error:#}"), false)),
        }
    }

    fn shift_day(&mut self, days: i64) {
        let next = self.date + ChronoDuration::days(days);
        if next <= Local::now().date_naive() {
            self.date = next;
            self.reload_selected();
        }
    }

    fn open_today(&mut self, _: &ShowToday, _: &mut Window, cx: &mut Context<Self>) {
        self.view = View::Today;
        self.reload_selected();
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
                        "记录已开启"
                    } else {
                        "记录已暂停"
                    }
                    .to_owned(),
                    true,
                ));
            }
            Err(error) => self.notice = Some((format!("操作失败：{error:#}"), false)),
        }
        self.refresh_tray();
    }

    fn generate_report(&mut self) {
        if self.generating {
            return;
        }
        if !has_ai(&self.settings.get()) {
            self.view = View::Report;
            self.notice = Some(("请先配置 AI 接口地址和模型名".to_owned(), false));
            return;
        }
        if self.day.events == 0 {
            self.notice = Some(("这一天还没有可分析的记录".to_owned(), false));
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
                self.draft = next;
                if let Err(error) = platform::set_autostart(self.draft.app.autostart) {
                    self.notice = Some((format!("开机自启设置失败：{error}"), false));
                } else {
                    self.notice = Some(("设置已保存".to_owned(), true));
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
                self.notice = Some((format!("保存失败：{error:#}"), false));
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
        self.notice = Some(("正在测试 AI 连接…".to_owned(), true));
    }

    fn purge(&mut self, window: &Window, screenshots_only: bool) {
        let (title, message) = if screenshots_only {
            (
                "清空屏幕快照",
                "将删除所有本地屏幕快照，活动记录会保留。此操作无法撤销。",
            )
        } else {
            (
                "清空全部记录",
                "将删除所有本地活动记录和屏幕快照。此操作无法撤销。",
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
                        "快照已清空"
                    } else {
                        "全部记录已清空"
                    }
                    .to_owned(),
                    true,
                ));
                self.reload_selected();
                self.status = self.recorder.status();
            }
            Err(error) => self.notice = Some((format!("清理失败：{error}"), false)),
        }
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> Div {
        let active = self.status.running && self.status.enabled;
        let status_title = if self.status.locked {
            "锁屏暂停"
        } else if self.status.idle && active {
            "空闲中"
        } else if active {
            "正在记录"
        } else {
            "记录已暂停"
        };
        let current_app = self
            .status
            .frontmost
            .as_ref()
            .map(|sample| sample.app.clone())
            .unwrap_or_else(|| "正常使用电脑即可自动记录".to_owned());
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
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(SECONDARY))
                            .child("Windows 活动记录与效率建议"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        nav_button("nav-today", "今日活动", self.view == View::Today).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.view = View::Today;
                                this.reload_selected();
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        nav_button("nav-report", "AI 建议", self.view == View::Report).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.view = View::Report;
                                this.reload_selected();
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        nav_button("nav-settings", "设置", self.view == View::Settings).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.view = View::Settings;
                                cx.notify();
                            }),
                        ),
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
                                "暂停记录"
                            } else {
                                "开始记录"
                            },
                            !active,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.toggle_recording();
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(FAINT))
                    .child("数据只保存到这台电脑"),
            )
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
                        small_button("date-today", "今天", false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.date = Local::now().date_naive();
                                this.reload_selected();
                                cx.notify();
                            },
                        )),
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
                    small_button("toast-close", "关闭", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.notice = None;
                            cx.notify();
                        },
                    )),
                )
        })
    }

    fn render_today(&mut self, cx: &mut Context<Self>) -> Div {
        let page = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.))
            .gap_5()
            .child(self.render_page_head(
                if self.date == Local::now().date_naive() {
                    "今天"
                } else {
                    "每日活动"
                },
                &full_date(self.date),
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
                            "记录已经开启"
                        } else {
                            "这一天还没有活动记录"
                        },
                    ))
                    .child(div().text_sm().text_color(rgb(SECONDARY)).child(
                        if self.status.running {
                            "切换应用或窗口后，时间线和快照会自动出现在这里。"
                        } else {
                            "开启后，TraceBit 会在本机记录前台应用和去重后的屏幕快照。"
                        },
                    ))
                    .children((!self.status.running).then(|| {
                        button("empty-toggle", "开始记录", true).on_click(cx.listener(
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
        let active_hours = self.day.total_active_ms as f64 / 3_600_000.0;
        let switch_note = if active_hours > 0.2 {
            format!("{:.1} 次/小时", self.day.switches as f64 / active_hours)
        } else {
            format!("{} 次切换", self.day.switches)
        };
        div()
            .flex()
            .w_full()
            .py_4()
            .border_b_1()
            .border_color(rgb(DIVIDER))
            .child(metric(
                "活跃时长",
                &fmt_ms(self.day.total_active_ms),
                &self
                    .day
                    .first_active
                    .map(|ts| format!("{} 开始", hhmm(ts)))
                    .unwrap_or_else(|| "暂无活动".to_owned()),
            ))
            .child(metric(
                "应用切换",
                &self.day.switches.to_string(),
                &switch_note,
            ))
            .child(metric(
                "最长专注",
                &self
                    .day
                    .longest_block
                    .as_ref()
                    .map(|block| fmt_ms(block.ms))
                    .unwrap_or_else(|| "暂无".to_owned()),
                &self
                    .day
                    .longest_block
                    .as_ref()
                    .map(|block| block.app.clone())
                    .unwrap_or_else(|| "不足 1 分钟".to_owned()),
            ))
            .child(metric(
                "本地快照",
                &self.day.screenshots.len().to_string(),
                &fmt_bytes(self.day.snapshot_bytes),
            ))
    }

    fn render_timeline(&self) -> Div {
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
            return section().child("时间线不可用");
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
                stay.title
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
                "24 小时时间线",
                &format!(
                    "{} 段活动 · {} 次长休息",
                    self.day.sessions.len(),
                    self.day.long_idles
                ),
            ))
            .child(track)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(rgb(FAINT))
                    .children((0..=8).map(|hour| format!("{}时", hour * 3))),
            )
    }

    fn render_apps(&self) -> Div {
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
            .child(panel_head("应用使用", "按活跃时长排序"))
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
                                            .child(app.category.clone()),
                                    )
                                    .child(
                                        div()
                                            .w(px(72.))
                                            .text_sm()
                                            .text_right()
                                            .child(fmt_ms(app.ms)),
                                    )
                                    .child(
                                        div()
                                            .w(px(44.))
                                            .text_xs()
                                            .text_color(rgb(SECONDARY))
                                            .text_right()
                                            .child(format!("{} 切换", app.switches)),
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
                                            .map(|(title, _)| title.clone())
                                            .collect::<Vec<_>>()
                                            .join(" · "),
                                    ),
                            )
                    })),
            )
    }

    fn render_categories(&self) -> Div {
        let totals = self
            .day
            .categories
            .iter()
            .map(|category| (category.name.clone(), category.ms))
            .collect::<Vec<_>>();
        section()
            .gap_3()
            .child(panel_head("时间构成", "浏览器时间按网站归类"))
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
                                    .child(category.clone()),
                            )
                            .child(format!(
                                "{} · {}%",
                                fmt_ms(ms),
                                (fraction * 100.0).round() as i32
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
        let configured = has_ai(&self.settings.get());
        section()
            .gap_3()
            .child(panel_head(
                "AI 效率建议",
                if self.report.is_some() {
                    "已生成"
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
                    "找出今天最值得改变的行为，给出可执行的建议和明天的小实验。".to_owned()
                } else {
                    "连接模型后即可生成建议；截图分析可在设置中单独开启。".to_owned()
                },
            ))
            .child(
                button(
                    "today-ai-action",
                    if self.generating {
                        "正在分析…"
                    } else if self.report.is_some() {
                        "查看完整建议"
                    } else if configured {
                        "生成今日建议"
                    } else {
                        "配置 AI"
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
        let mut section = section().gap_3().child(panel_head(
            "时段摘要",
            "视觉模型按时段总结截图，作为 AI 建议的依据",
        ));
        if let Some(request) = &self.interval_request {
            section = section.child(div().text_xs().text_color(rgb(FAINT)).child(format!(
                "上次发送：{} · {} 张截图 · {} · {}",
                hhmm(request.at),
                request.files.len(),
                request.model,
                request.endpoint
            )));
        }
        if let Some(error) = &self.interval_error {
            section = section.child(
                div()
                    .text_sm()
                    .text_color(rgb(RED))
                    .child(format!("时段分析失败：{error}")),
            );
        }
        section.children(if self.interval_summaries.is_empty() {
            vec![
                div()
                    .text_sm()
                    .text_color(rgb(SECONDARY))
                    .child("完成一个分析时段后，摘要会显示在这里。"),
            ]
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
                                .child(format!("分析 {} 张截图", summary.screenshot_count)),
                        )
                        .child(div().text_sm().child(summary.summary.clone()))
                })
                .collect()
        })
    }

    fn render_sites(&self) -> Div {
        section()
            .gap_3()
            .child(panel_head("浏览网站", "仅记录当前浏览器域名，本机保存"))
            .children(if self.day.sites.is_empty() {
                vec![
                    div()
                        .text_sm()
                        .text_color(rgb(SECONDARY))
                        .child("还没有读取到网站。"),
                ]
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
                            .child(div().text_xs().text_color(rgb(SECONDARY)).child(format!(
                                "{} · {} 次 · {}",
                                fmt_ms(site.ms),
                                site.visits,
                                site.category.clone().unwrap_or_else(|| "未归类".to_owned())
                            )))
                    })
                    .collect()
            })
    }

    fn render_snapshots(&self, cx: &mut Context<Self>) -> Div {
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
            .child(panel_head("屏幕快照", "只保留画面有变化的帧"))
            .child(if shots.is_empty() {
                div()
                    .text_sm()
                    .text_color(rgb(SECONDARY))
                    .child("还没有快照。只有画面变化时才会保存，不会持续录屏。")
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
                                    .child(trigger_label(&shot.trigger)),
                            )
                    }))
            })
    }

    fn render_report(&mut self, cx: &mut Context<Self>) -> Div {
        let page = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.))
            .gap_5()
            .child(self.render_page_head(
                "AI 效率建议",
                "从当天记录中找出最值得改变的行为，给出可执行的建议和可验证的小实验",
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
                    &format!("{} 建议", self.date_key()),
                    &format!(
                        "已记录 {} · {} 次切换",
                        fmt_ms(self.day.total_active_ms),
                        self.day.switches
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
                                    "收起发送内容"
                                } else {
                                    "查看发送内容"
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
                                    "正在分析…"
                                } else if self.report.is_some() {
                                    "重新生成"
                                } else {
                                    "生成建议"
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
                        .child("生成建议时会把以下内容发送给 AI 接口（不含截图、网站域名和密钥）"),
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
                    .child("正在计算证据并请求模型，通常需要几十秒。"),
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
            let mut meta = div()
                .flex()
                .flex_wrap()
                .gap_3()
                .text_xs()
                .text_color(rgb(FAINT))
                .child(format!("模型 {}", report.model))
                .child(format!(
                    "{} · {}",
                    if report.source == "auto" {
                        "自动生成"
                    } else {
                        "手动生成"
                    },
                    crate::store::local_datetime(report.generated_at)
                        .map(|date| date.format("%m-%d %H:%M").to_string())
                        .unwrap_or_default()
                ))
                .child(format!("证据 {} 条", report.evidence.len()));
            if report.usage.prompt_tokens + report.usage.completion_tokens > 0 {
                meta = meta.child(format!(
                    "Token {} + {}",
                    report.usage.prompt_tokens, report.usage.completion_tokens
                ));
            }
            if !report.prompt_version.is_empty() {
                meta = meta.child(report.prompt_version.clone());
            }
            document = document.child(meta).child(self.render_report_summary(report, cx));
        } else {
            document = document.child(
                div()
                    .p_8()
                    .text_center()
                    .text_color(rgb(SECONDARY))
                    .child("还没有这一天的建议，点击右上角“生成建议”。"),
            );
        }
        page.child(document).child(self.render_report_history(cx))
    }

    /// Compact in-app summary; the full report opens as an HTML page.
    fn render_report_summary(&self, report: &Report, cx: &mut Context<Self>) -> Div {
        let open_button = button("open-report-html", "在浏览器中查看完整报告", true).on_click(
            cx.listener(|this, _, _, cx| {
                if let Some(report) = &this.report {
                    match this.analyzer.write_html(report) {
                        Ok(path) => {
                            let _ = open::that(path);
                        }
                        Err(error) => {
                            this.notice = Some((format!("无法打开报告：{error:#}"), false));
                        }
                    }
                }
                cx.notify();
            }),
        );
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
                    .child(metric("活跃", &format!("{:.0} 分钟", metrics.active_min), ""))
                    .child(metric(
                        "深度工作",
                        &format!("{:.0} 分钟", metrics.deep_work_min),
                        &format!("{:.0} 块", metrics.deep_blocks),
                    ))
                    .child(metric("每小时切换", &format!("{:.1} 次", metrics.switches_per_hour), ""))
                    .child(metric("打开通讯", &format!("{:.0} 次", metrics.comm_checks), "")),
            );
        }
        let mut list = div().flex().flex_col().gap_2().child(
            div()
                .text_xs()
                .text_color(rgb(FAINT))
                .child(format!("{} 条建议", advice.suggestions.len())),
        );
        for (index, suggestion) in advice.suggestions.iter().enumerate() {
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
                    .child(div().flex_1().text_sm().child(suggestion.title.trim().to_owned()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(FAINT))
                            .child(format!(
                                "{} · 投入{} · 把握{}",
                                suggestion.kind, suggestion.effort, suggestion.confidence
                            )),
                    ),
            );
        }
        summary = summary.child(list);
        if let Some(experiment) = &advice.experiment {
            summary = summary.child(
                div()
                    .text_sm()
                    .text_color(rgb(SECONDARY))
                    .child(format!("明日实验：{}", experiment.action.trim())),
            );
        }
        summary.child(div().flex().child(open_button))
    }

    fn render_ai_setup(&mut self, cx: &mut Context<Self>) -> Div {
        panel().gap_4().child(panel_head("先连接一个 AI 模型", "支持 DeepSeek、通义、Kimi、OpenAI 和本地 Ollama"))
            .child(setting_row("接口地址", "本地 Ollama：http://localhost:11434/v1", self.form.base_url.clone()))
            .child(setting_row("API Key", "使用 Windows DPAPI 加密保存在本机", self.form.api_key.clone()))
            .child(setting_row("模型名", "建议多模态模型，如 qwen2.5vl、gpt-4o-mini", self.form.model.clone()))
            .child(button("report-ai-save", if self.testing { "正在测试…" } else { "保存并测试" }, true).on_click(cx.listener(|this, _, _, cx| { this.test_ai(cx); cx.notify(); })))
            .child(div().text_sm().text_color(rgb(SECONDARY)).child("建议只发送本地计算的脱敏证据（聊天窗口只保留应用名），不发送网站域名、API Key 或键盘输入。可以随时点击“查看发送内容”核对。"))
    }

    fn render_report_history(&self, cx: &mut Context<Self>) -> Div {
        panel()
            .gap_2()
            .child(panel_head(
                "历史建议",
                &format!("{} 份", self.report_list.len()),
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
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(SECONDARY))
                            .child(format!("{} 分钟 · {}", item.active_minutes, item.model)),
                    )
            }))
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let page = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.))
            .gap_5()
            .child(self.render_page_head("设置", "修改后点击“保存设置”生效", false, cx));

        let recording = panel()
            .gap_4()
            .child(panel_head("记录", "开始和暂停也可以使用左侧固定按钮"))
            .child(self.toggle_row(
                "启用行为记录",
                "与左侧主按钮保持同步",
                self.draft.recording.enabled,
                cx.listener(|this, _, _, cx| {
                    this.draft.recording.enabled = !this.draft.recording.enabled;
                    cx.notify();
                }),
            ))
            .child(self.toggle_row(
                "保存屏幕快照",
                "应用切换、定时心跳且画面变化时保存",
                self.draft.recording.screenshots,
                cx.listener(|this, _, _, cx| {
                    this.draft.recording.screenshots = !this.draft.recording.screenshots;
                    cx.notify();
                }),
            ))
            .child(self.toggle_row(
                "记录浏览网站",
                "默认关闭。仅保存当前浏览器域名，不含路径和查询参数",
                self.draft.recording.browser_sites,
                cx.listener(|this, _, _, cx| {
                    this.draft.recording.browser_sites = !this.draft.recording.browser_sites;
                    cx.notify();
                }),
            ))
            .child(setting_row(
                "快照心跳间隔",
                "活跃期间定时检查画面，单位：分钟",
                self.form.heartbeat.clone(),
            ))
            .child(setting_row(
                "空闲阈值",
                "无键鼠输入超过该分钟数后，不计入活跃时长",
                self.form.idle.clone(),
            ))
            .child(setting_row(
                "会议与全屏空闲阈值",
                "开会、看视频或放映时不碰键鼠也算活跃，单位：分钟",
                self.form.passive_idle.clone(),
            ));
        let ai = panel()
            .gap_4()
            .child(panel_head(
                "AI 分析",
                "OpenAI 兼容接口，密钥用 Windows DPAPI 加密保存",
            ))
            .child(setting_row(
                "接口地址",
                "例如 https://api.deepseek.com/v1",
                self.form.base_url.clone(),
            ))
            .child(setting_row(
                "API Key",
                "本地 Ollama 可以留空",
                self.form.api_key.clone(),
            ))
            .child(setting_row(
                "模型名",
                "例如 qwen2.5vl、gpt-4o-mini。开启截图分析时必须支持图片输入",
                self.form.model.clone(),
            ))
            .child(self.toggle_row(
                "自动分析屏幕截图",
                "按时段把截图交给上方模型总结，作为建议的依据。需要支持图片输入的多模态模型",
                self.draft.ai.screenshot_analysis,
                cx.listener(|this, _, _, cx| {
                    this.draft.ai.screenshot_analysis = !this.draft.ai.screenshot_analysis;
                    cx.notify();
                }),
            ))
            .child(setting_row(
                "截图分析间隔",
                "5–240 分钟；每次最多 6 张，缩放后发送",
                self.form.screenshot_interval.clone(),
            ))
            .child(self.toggle_row(
                "每日自动生成",
                "到点生成当天建议；错过时间或之后仍有大量活动会在次日补生成，不覆盖手动生成的建议",
                self.draft.ai.auto_report,
                cx.listener(|this, _, _, cx| {
                    this.draft.ai.auto_report = !this.draft.ai.auto_report;
                    cx.notify();
                }),
            ))
            .child(setting_row(
                "自动生成时间",
                "24 小时制，例如 21:30",
                self.form.auto_time.clone(),
            ))
            .child(self.toggle_row(
                "联网搜索效率资源",
                "仅发送抽象工具关键词，不发送行为记录",
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
                        "搜索服务",
                        "DuckDuckGo 无需密钥；Brave 需要单独 API Key",
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
                "搜索 API Key",
                "仅 Brave Search 需要，与 AI 密钥分开保存",
                self.form.search_key.clone(),
            ))
            .child(
                button(
                    "ai-test",
                    if self.testing {
                        "正在测试…"
                    } else {
                        "保存并测试连接"
                    },
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.test_ai(cx);
                    cx.notify();
                })),
            );
        let storage =
            panel()
                .gap_4()
                .child(panel_head(
                    "隐私与存储",
                    "敏感窗口不记录标题、不截图",
                ))
                .child(setting_row(
                    "敏感内容关键词",
                    "用逗号分隔。前台应用、标题或网站命中时，标题写为“[敏感窗口]”，且不截图、不记录网站",
                    self.form.patterns.clone(),
                ))
                .child(setting_row(
                    "快照占用上限（GB）",
                    &format!(
                        "超出后删除最旧日期。当前 {}",
                        fmt_bytes(self.status.total_snapshot_bytes)
                    ),
                    self.form.cap.clone(),
                ))
                .child(setting_row(
                    "数据保留天数",
                    "超期的每日记录和快照会自动清理",
                    self.form.retention.clone(),
                ))
                .child(self.toggle_row(
                    "开机自启",
                    "登录 Windows 后启动 TraceBit",
                    self.draft.app.autostart,
                    cx.listener(|this, _, _, cx| {
                        this.draft.app.autostart = !this.draft.app.autostart;
                        cx.notify();
                    }),
                ))
                .child(self.toggle_row(
                    "关闭时最小化到托盘",
                    "关闭窗口后继续在后台记录",
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
                            small_button("open-data", "打开目录", false).on_click(cx.listener(
                                |this, _, _, _| {
                                    let _ = open::that(&this.store.data_dir);
                                },
                            )),
                        )
                        .child(small_button("purge-shots", "清空快照", false).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.purge(window, true);
                                cx.notify();
                            }),
                        ))
                        .child(small_button("purge-all", "清空全部记录", false).on_click(
                            cx.listener(|this, _, window, cx| {
                                this.purge(window, false);
                                cx.notify();
                            }),
                        )),
                );
        page.child(recording).child(ai).child(storage).child(
            button("save-settings", "保存设置", true).on_click(cx.listener(|this, _, _, cx| {
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
                            .child(if enabled { "已开启" } else { "已关闭" }),
                    ),
            )
    }

    fn render_lightbox(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let index = self.lightbox?;
        let shot = self.day.screenshots.get(index)?;
        let path = self.store.snapshot_path(&shot.file)?;
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
                                                .child("屏幕快照"),
                                        )
                                        .child(div().text_xs().text_color(rgb(SECONDARY)).child(
                                            format!(
                                                "{} · {} · {}/{}",
                                                hhmm(shot.ts),
                                                trigger_label(&shot.trigger),
                                                index + 1,
                                                self.day.screenshots.len()
                                            ),
                                        )),
                                )
                                .child(small_button("lightbox-close", "关闭", false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.lightbox = None;
                                        cx.notify();
                                    }),
                                )),
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
                                .child(small_button("lightbox-open", "系统看图", false).on_click(
                                    cx.listener(|this, _, _, _| {
                                        if let Some(shot) = this
                                            .lightbox
                                            .and_then(|index| this.day.screenshots.get(index))
                                            && let Some(path) = this.store.snapshot_path(&shot.file)
                                        {
                                            let _ = open::that(path);
                                        }
                                    }),
                                ))
                                .child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .child(
                                            small_button("lightbox-prev", "上一张", false)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.move_lightbox(-1);
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            small_button("lightbox-next", "下一张", false)
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.move_lightbox(1);
                                                    cx.notify();
                                                })),
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
            View::Report => self.render_report(cx),
            View::Settings => self.render_settings(cx),
        };
        div()
            .relative()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open_today))
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

fn create_tray(active: bool) -> Option<TrayState> {
    let status = MenuItem::with_id(
        "status",
        if active { "正在记录" } else { "已暂停" },
        false,
        None,
    );
    let toggle = MenuItem::with_id(
        "toggle",
        if active {
            "暂停记录"
        } else {
            "恢复记录"
        },
        true,
        None,
    );
    let show = MenuItem::with_id("show", "显示主窗口", true, None);
    let report = MenuItem::with_id("report", "查看效率建议", true, None);
    let quit = MenuItem::with_id("quit", "退出", true, None);
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

fn full_date(date: NaiveDate) -> String {
    let weekday = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"]
        [date.weekday().num_days_from_sunday() as usize];
    format!("{}月{}日 · {weekday}", date.month(), date.day())
}

fn fmt_ms(ms: i64) -> String {
    let minutes = (ms as f64 / 60_000.0).round() as i64;
    if minutes < 60 {
        format!("{minutes}分钟")
    } else {
        let hours = minutes / 60;
        let rest = minutes % 60;
        if rest == 0 {
            format!("{hours}小时")
        } else {
            format!("{hours}小时{rest}分")
        }
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

fn trigger_label(trigger: &str) -> String {
    match trigger {
        "app_focus" => "切换",
        "heartbeat" => "定时",
        "resume" => "返回",
        "manual" => "手动",
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
        .expect("无法创建 TraceBit 窗口");
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
        loop {
            if !first {
                thread::sleep(Duration::from_secs(60));
            }
            first = false;
            let ai = settings.get().ai;
            if !ai.auto_report || !ai.text_endpoint().configured() {
                continue;
            }
            let now = Local::now();
            let today = now.format("%Y-%m-%d").to_string();
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
                            "TraceBit 效率建议已生成",
                            &format!("{date_key} 的效率建议已就绪"),
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
    let store = Arc::new(DayStore::new(root.join("data"))?);
    let recorder = Recorder::new(settings, store.clone());
    recorder.start()?;
    thread::sleep(Duration::from_secs(8));
    let status = recorder.status();
    recorder.stop();
    let stats = store.day_stats(&local_date_key(now_ms()), DeriveOptions::default())?;
    println!(
        "前台应用：{:?}",
        status.frontmost.as_ref().map(|sample| &sample.app)
    );
    println!(
        "原生事件：{}，截图：{}",
        stats.events,
        stats.screenshots.len()
    );
    if stats.events == 0 {
        anyhow::bail!("未采集到原生事件");
    }
    if stats.screenshots.is_empty() {
        anyhow::bail!("未生成屏幕快照");
    }
    fs::remove_dir_all(root)?;
    println!("原生采集 smoke 通过");
    Ok(())
}
