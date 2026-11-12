use crate::{
    analyzer::{Analyzer, Report, ReportSummary},
    input::{self, TextInput},
    platform,
    recorder::{Recorder, RecorderStatus},
    settings::{Settings, SettingsData},
    store::{DayStats, DayStore, DeriveOptions, hhmm, local_date_key, now_ms},
};
use anyhow::Result;
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, TimeZone};
use gpui::{
    App, Application, Bounds, Context, Div, ElementId, Entity, FontWeight, SharedString, Stateful,
    Timer, TitlebarOptions, Window, WindowBounds, WindowOptions, div, img, prelude::*, px,
    relative, rgb, rgba, size,
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

const CANVAS: u32 = 0xf4f2ed;
const SURFACE: u32 = 0xfffefb;
const SUBTLE: u32 = 0xebe8e1;
const INK: u32 = 0x24231f;
const SECONDARY: u32 = 0x6f6b62;
const FAINT: u32 = 0x979186;
const DIVIDER: u32 = 0xdedad1;
const BRAND_PALE: u32 = 0xf4e7c8;
const GREEN: u32 = 0x2f7d57;
const RED: u32 = 0xb64b4b;

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
    search_key: Entity<TextInput>,
    heartbeat: Entity<TextInput>,
    idle: Entity<TextInput>,
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
    settings: Arc<Settings>,
    store: Arc<DayStore>,
    recorder: Recorder,
    analyzer: Arc<Analyzer>,
    view: View,
    date: NaiveDate,
    day: DayStats,
    report: Option<Report>,
    report_list: Vec<ReportSummary>,
    status: RecorderStatus,
    draft: SettingsData,
    form: FormState,
    generating: bool,
    testing: bool,
    report_error: Option<String>,
    notice: Option<(String, bool)>,
    lightbox: Option<usize>,
    report_rx: Option<Receiver<(String, Result<Report>)>>,
    test_rx: Option<Receiver<Result<(String, String)>>>,
    auto_rx: Receiver<String>,
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
        auto_rx: Receiver<String>,
        quitting: Arc<AtomicBool>,
    ) -> Self {
        let current = settings.get();
        let date = Local::now().date_naive();
        let day = load_day(&store, &settings, &local_date_key(now_ms()));
        let report = analyzer.get_report(&local_date_key(now_ms()));
        let report_list = analyzer.list_reports();
        let status = recorder.status();
        let form = FormState::new(cx, &current);
        let tray = create_tray(status.running && status.enabled);
        let mut this = Self {
            settings,
            store,
            recorder,
            analyzer,
            view: View::Today,
            date,
            day,
            report,
            report_list,
            status,
            draft: current,
            form,
            generating: false,
            testing: false,
            report_error: None,
            notice: None,
            lightbox: None,
            report_rx: None,
            test_rx: None,
            auto_rx,
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
        self.status = self.recorder.status();
        self.poll_report_task();
        self.poll_test_task();
        self.poll_tray(window, cx);
        while let Ok(date_key) = self.auto_rx.try_recv() {
            self.notice = Some(("今日日报已自动生成".to_owned(), true));
            if self.date_key() == date_key {
                self.report = self.analyzer.get_report(&date_key);
            }
            self.report_list = self.analyzer.list_reports();
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

    fn poll_report_task(&mut self) {
        let result = match self.report_rx.as_ref().map(|rx| rx.try_recv()) {
            Some(Ok(result)) => Some(result),
            Some(Err(TryRecvError::Disconnected)) => {
                self.report_rx = None;
                self.generating = false;
                Some((self.date_key(), Err(anyhow::anyhow!("日报任务意外结束"))))
            }
            _ => None,
        };
        if let Some((date_key, result)) = result {
            self.report_rx = None;
            self.generating = false;
            match result {
                Ok(report) => {
                    self.report = Some(report);
                    self.notice = Some(("行为日报已生成".to_owned(), true));
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
        self.report_error = None;
        self.lightbox = None;
    }

    fn shift_day(&mut self, days: i64) {
        let next = self.date + ChronoDuration::days(days);
        if next <= Local::now().date_naive() {
            self.date = next;
            self.reload_selected();
        }
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
        let ai = self.settings.get().ai;
        if ai.base_url.trim().is_empty() || ai.model.trim().is_empty() {
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

    fn purge(&mut self, screenshots_only: bool) {
        let result = if screenshots_only {
            self.store.purge_screenshots()
        } else {
            self.store.purge_all()
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
                            .child("看见时间，少费点力"),
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
                        nav_button("nav-report", "AI 日报", self.view == View::Report).on_click(
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

    fn render_notice(&self) -> Option<Div> {
        self.notice.as_ref().map(|(message, success)| {
            div()
                .p_3()
                .rounded_md()
                .text_sm()
                .bg(rgb(if *success { 0xe4efe7 } else { 0xf5e4e2 }))
                .text_color(rgb(if *success { GREEN } else { RED }))
                .child(message.clone())
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
            .children(self.render_notice())
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
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(DIVIDER))
            .rounded_lg()
            .p_4()
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
            return panel().child("时间线不可用");
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
        panel()
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
        panel()
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
                                        app.titles
                                            .keys()
                                            .take(2)
                                            .cloned()
                                            .collect::<Vec<_>>()
                                            .join(" · "),
                                    ),
                            )
                    })),
            )
    }

    fn render_categories(&self) -> Div {
        let mut totals = BTreeMap::new();
        for app in &self.day.apps {
            *totals.entry(app.category.clone()).or_insert(0) += app.ms;
        }
        let mut totals = totals.into_iter().collect::<Vec<_>>();
        totals.sort_by_key(|total| std::cmp::Reverse(total.1));
        panel()
            .gap_3()
            .child(panel_head("时间构成", ""))
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
        panel()
            .gap_3()
            .child(panel_head(
                "AI 行为日报",
                if self.report.is_some() {
                    "已生成"
                } else {
                    ""
                },
            ))
            .child(div().text_sm().text_color(rgb(SECONDARY)).child(
                if let Some(report) = &self.report {
                    report_excerpt(&report.markdown)
                } else if configured {
                    "把今天的记录整理成行为复盘和具体省力建议。".to_owned()
                } else {
                    "连接模型后即可生成日报。标题会脱敏，截图不会发送。".to_owned()
                },
            ))
            .child(
                button(
                    "today-ai-action",
                    if self.generating {
                        "正在生成…"
                    } else if self.report.is_some() {
                        "查看完整日报"
                    } else if configured {
                        "生成今日日报"
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

    fn render_sites(&self) -> Div {
        panel()
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
                                "{} 次 · {}",
                                site.visits,
                                hhmm(site.last_seen)
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
        panel()
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
                            .cursor_pointer()
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
                "AI 行为日报",
                "看清时间去向，拿到今天就能用的省力建议",
                true,
                cx,
            ))
            .children(self.render_notice());
        if !has_ai(&self.settings.get()) {
            return page.child(self.render_ai_setup(cx));
        }
        let mut document = panel().gap_4().child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .child(panel_head(
                    &format!("{} 报告", self.date_key()),
                    &format!(
                        "已记录 {} · {} 次切换",
                        fmt_ms(self.day.total_active_ms),
                        self.day.switches
                    ),
                ))
                .child(
                    button(
                        "generate-report",
                        if self.generating {
                            "正在生成…"
                        } else if self.report.is_some() {
                            "重新生成"
                        } else {
                            "生成日报"
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
        );
        if self.generating {
            document = document.child(
                div()
                    .p_8()
                    .text_center()
                    .text_color(rgb(SECONDARY))
                    .child("正在整理记录并请求模型，通常需要几十秒。"),
            );
        } else if let Some(error) = &self.report_error {
            document = document.child(
                div()
                    .p_4()
                    .rounded_md()
                    .bg(rgb(0xf5e4e2))
                    .text_color(rgb(RED))
                    .child(error.clone()),
            );
        } else if let Some(report) = &self.report {
            document = document.child(
                div()
                    .flex()
                    .gap_3()
                    .text_xs()
                    .text_color(rgb(FAINT))
                    .child(format!("模型 {}", report.model))
                    .child(format!("活跃 {} 分钟", report.active_minutes))
                    .child(format!("工具化机会 {} 项", report.recommendation_count)),
            );
            for (index, line) in report.markdown.lines().enumerate() {
                document = document.child(render_markdown_line(index, line));
            }
        } else {
            document = document.child(
                div()
                    .p_8()
                    .text_center()
                    .text_color(rgb(SECONDARY))
                    .child("还没有这一天的日报，点击右上角“生成日报”。"),
            );
        }
        page.child(document).child(self.render_report_history(cx))
    }

    fn render_ai_setup(&mut self, cx: &mut Context<Self>) -> Div {
        panel().gap_4().child(panel_head("先连接一个 AI 模型", "支持 DeepSeek、通义、Kimi、OpenAI 和本地 Ollama"))
            .child(setting_row("接口地址", "本地 Ollama：http://localhost:11434/v1", self.form.base_url.clone()))
            .child(setting_row("API Key", "密钥只保存在本机设置文件中", self.form.api_key.clone()))
            .child(setting_row("模型名", "例如 deepseek-chat、qwen-plus、gpt-4o-mini", self.form.model.clone()))
            .child(button("report-ai-save", if self.testing { "正在测试…" } else { "保存并测试" }, true).on_click(cx.listener(|this, _, _, cx| { this.test_ai(cx); cx.notify(); })))
            .child(div().text_sm().text_color(rgb(SECONDARY)).child("发送：活跃时长、切换、应用名和脱敏标题。不会发送：截图、网站域名、API Key、键盘输入内容。"))
    }

    fn render_report_history(&self, cx: &mut Context<Self>) -> Div {
        panel()
            .gap_2()
            .child(panel_head(
                "历史日报",
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
                    .cursor_pointer()
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
            .child(self.render_page_head("设置", "采集、隐私、存储和模型都集中在这里", false, cx))
            .children(self.render_notice());
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
            ));
        let ai = panel()
            .gap_4()
            .child(panel_head("AI 分析", "OpenAI 兼容接口，密钥只保存在本机"))
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
                "例如 deepseek-chat、qwen-plus",
                self.form.model.clone(),
            ))
            .child(self.toggle_row(
                "每日自动生成",
                "到点分析当天记录并发送系统通知",
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
                    "敏感窗口不截图，快照永远不会发送给 AI",
                ))
                .child(setting_row(
                    "敏感内容关键词",
                    "用逗号分隔。前台应用或标题命中时跳过截图",
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
                            cx.listener(|this, _, _, cx| {
                                this.purge(true);
                                cx.notify();
                            }),
                        ))
                        .child(small_button("purge-all", "清空全部记录", false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.purge(false);
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
                    .cursor_pointer()
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

    fn render_lightbox(&self, cx: &mut Context<Self>) -> Option<Div> {
        let index = self.lightbox?;
        let shot = self.day.screenshots.get(index)?;
        let path = self.store.snapshot_path(&shot.file)?;
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x000000cc))
                .p_8()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .items_center()
                        .child(img(path).max_w(px(1000.)).max_h(px(650.)).rounded_lg())
                        .child(
                            div()
                                .flex()
                                .gap_3()
                                .items_center()
                                .child(small_button("lightbox-prev", "‹ 上一张", false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.move_lightbox(-1);
                                        cx.notify();
                                    }),
                                ))
                                .child(div().text_sm().text_color(gpui::white()).child(format!(
                                    "{} · {} · {}/{}",
                                    hhmm(shot.ts),
                                    trigger_label(&shot.trigger),
                                    index + 1,
                                    self.day.screenshots.len()
                                )))
                                .child(small_button("lightbox-next", "下一张 ›", false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.move_lightbox(1);
                                        cx.notify();
                                    }),
                                ))
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
                                .child(small_button("lightbox-close", "关闭", true).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.lightbox = None;
                                        cx.notify();
                                    }),
                                )),
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.view {
            View::Today => self.render_today(cx),
            View::Report => self.render_report(cx),
            View::Settings => self.render_settings(cx),
        };
        div()
            .relative()
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
            .children(self.render_lightbox(cx))
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
    let report = MenuItem::with_id("report", "生成今日日报", true, None);
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
        .day_stats(
            date_key,
            DeriveOptions {
                idle_threshold_ms: recording.idle_threshold_ms,
                session_gap_ms: recording.session_gap_ms,
            },
        )
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
        .cursor_pointer()
        .text_sm()
        .bg(rgb(if selected { BRAND_PALE } else { SURFACE }))
        .text_color(rgb(if selected { INK } else { SECONDARY }))
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
        .cursor_pointer()
        .text_sm()
        .bg(rgb(if primary { INK } else { SURFACE }))
        .text_color(rgb(if primary { 0xffffff } else { INK }))
        .border_1()
        .border_color(rgb(if primary { INK } else { DIVIDER }))
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
        .cursor_pointer()
        .text_xs()
        .bg(rgb(if selected { BRAND_PALE } else { SURFACE }))
        .border_1()
        .border_color(rgb(DIVIDER))
        .child(label.to_owned())
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
                .text_base()
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
        .border_r_1()
        .border_color(rgb(DIVIDER))
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

fn render_markdown_line(index: usize, line: &str) -> Stateful<Div> {
    let trimmed = line.trim();
    let (size, weight) = if trimmed.starts_with("# ") {
        (22., FontWeight::BOLD)
    } else if trimmed.starts_with("## ") {
        (18., FontWeight::SEMIBOLD)
    } else if trimmed.starts_with("### ") {
        (15., FontWeight::SEMIBOLD)
    } else {
        (14., FontWeight::NORMAL)
    };
    let text = clean_inline(trimmed.trim_start_matches('#').trim_start());
    let url = markdown_url(trimmed);
    let mut element = div()
        .id(SharedString::from(format!("report-line-{index}")))
        .text_size(px(size))
        .font_weight(weight)
        .py_1();
    if trimmed.starts_with('>') {
        element = element
            .p_3()
            .rounded_md()
            .bg(rgb(CANVAS))
            .text_color(rgb(SECONDARY));
    }
    if let Some(url) = url {
        element = element
            .cursor_pointer()
            .text_color(rgb(0x4277a8))
            .on_click(move |_, _, _| {
                let _ = open::that(&url);
            });
    }
    element.child(if text.is_empty() {
        " ".to_owned()
    } else {
        text
    })
}

fn clean_inline(value: &str) -> String {
    let mut output = value.replace("**", "").replace('`', "");
    while let Some(start) = output.find('[') {
        let Some(middle) = output[start..].find("](") else {
            break;
        };
        let middle = start + middle;
        let Some(end) = output[middle + 2..].find(')') else {
            break;
        };
        let end = middle + 2 + end;
        let name = output[start + 1..middle].to_owned();
        let url = output[middle + 2..end].to_owned();
        output.replace_range(start..=end, &format!("{name}（{url}）"));
    }
    output
}

fn markdown_url(value: &str) -> Option<String> {
    let start = value.find("](https://")? + 2;
    let end = value[start..].find(')')? + start;
    Some(value[start..end].to_owned())
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
    let user_dir = crate::user_data_dir();
    fs::create_dir_all(&user_dir)?;
    let settings = Arc::new(Settings::load(&user_dir));
    let store = Arc::new(DayStore::new(user_dir.join("data"))?);
    let recorder = Recorder::new(settings.clone(), store.clone());
    let analyzer = Arc::new(Analyzer::new(
        settings.clone(),
        store.clone(),
        user_dir.join("reports"),
    )?);
    if settings.get().app.autostart {
        let _ = platform::set_autostart(true);
    }
    if settings.get().recording.enabled {
        recorder.start()?;
    }
    let (auto_tx, auto_rx) = std::sync::mpsc::channel();
    spawn_auto_report(settings.clone(), analyzer.clone(), auto_tx);
    let quitting = Arc::new(AtomicBool::new(false));
    let recorder_for_quit = recorder.clone();
    Application::new().run(move |cx| {
        input::bind_keys(cx);
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
                        auto_rx,
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

fn spawn_auto_report(
    settings: Arc<Settings>,
    analyzer: Arc<Analyzer>,
    sender: std::sync::mpsc::Sender<String>,
) {
    thread::spawn(move || {
        let mut attempted = String::new();
        loop {
            thread::sleep(Duration::from_secs(30));
            let ai = settings.get().ai;
            if !ai.auto_report || ai.base_url.is_empty() || ai.model.is_empty() {
                continue;
            }
            let now = Local::now();
            let date_key = now.format("%Y-%m-%d").to_string();
            if now.format("%H:%M").to_string() != ai.auto_report_time || attempted == date_key {
                continue;
            }
            attempted = date_key.clone();
            match analyzer.generate_report(&date_key) {
                Ok(_) => {
                    platform::show_notification(
                        "TraceBit 日报已生成",
                        &format!("{date_key} 的行为日报已就绪"),
                    );
                    let _ = sender.send(date_key);
                }
                Err(error) => eprintln!("[AutoReport] {error:#}"),
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
