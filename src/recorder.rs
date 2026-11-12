use crate::{
    categories::is_meeting,
    platform::{ForegroundSample, Sampler, workstation_is_locked},
    settings::Settings,
    store::{DayStore, Event, kinds, local_date_key, now_ms},
};
use anyhow::{Context, Result};
use chrono::Local;
use image::{DynamicImage, RgbaImage, imageops::FilterType};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha1::{Digest, Sha1};
use std::{
    fs,
    net::IpAddr,
    path::PathBuf,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use url::Url;

const THUMB_W: u32 = 160;
const THUMB_H: u32 = 90;
const HIST_BINS: usize = 64;
const HIST_CHANGE_THRESHOLD: f64 = 0.05;
const PIXEL_DIFF_THRESHOLD: f64 = 0.02;
const PIXEL_TOLERANCE: i16 = 30;
/// Stored instead of the real title when a window matches a sensitive pattern.
pub const SENSITIVE_TITLE: &str = "[敏感窗口]";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStatus {
    pub running: bool,
    pub enabled: bool,
    pub locked: bool,
    pub idle: bool,
    pub frontmost: Option<ForegroundSample>,
    pub poller_alive: bool,
    pub last_error: Option<String>,
    pub data_dir: String,
    pub total_snapshot_bytes: u64,
}

#[derive(Debug, Default)]
struct Inner {
    running: bool,
    locked: bool,
    idle: bool,
    frontmost: Option<ForegroundSample>,
    last_error: Option<String>,
    stop: Option<Arc<AtomicBool>>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone)]
pub struct Recorder {
    settings: Arc<Settings>,
    store: Arc<DayStore>,
    inner: Arc<Mutex<Inner>>,
}

impl Recorder {
    pub fn new(settings: Arc<Settings>, store: Arc<DayStore>) -> Self {
        Self {
            settings,
            store,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    pub fn running(&self) -> bool {
        self.inner.lock().expect("recorder lock").running
    }

    pub fn status(&self) -> RecorderStatus {
        let inner = self.inner.lock().expect("recorder lock");
        RecorderStatus {
            running: inner.running,
            enabled: self.settings.get().recording.enabled,
            locked: inner.locked,
            idle: inner.idle,
            frontmost: inner.frontmost.clone(),
            poller_alive: inner
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.is_finished()),
            last_error: inner.last_error.clone(),
            data_dir: self.store.data_dir.to_string_lossy().into_owned(),
            total_snapshot_bytes: self.store.total_snapshot_bytes(),
        }
    }

    pub fn start(&self) -> Result<()> {
        let mut inner = self.inner.lock().expect("recorder lock");
        if inner.running || !self.settings.get().recording.enabled {
            return Ok(());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let settings = self.settings.clone();
        let store = self.store.clone();
        let shared = self.inner.clone();
        let worker = thread::Builder::new()
            .name("tracebit-native-recorder".to_owned())
            .spawn(move || run_worker(settings, store, shared, worker_stop))
            .context("无法启动原生采集线程")?;
        inner.running = true;
        inner.locked = false;
        inner.idle = false;
        inner.last_error = None;
        inner.stop = Some(stop);
        inner.worker = Some(worker);
        Ok(())
    }

    pub fn stop(&self) {
        let (stop, worker) = {
            let mut inner = self.inner.lock().expect("recorder lock");
            inner.running = false;
            inner.idle = false;
            inner.locked = false;
            (inner.stop.take(), inner.worker.take())
        };
        if let Some(stop) = stop {
            stop.store(true, Ordering::Release);
        }
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }

    pub fn restart(&self) {
        self.stop();
        if let Err(error) = self.start() {
            self.inner.lock().expect("recorder lock").last_error = Some(error.to_string());
        }
    }

    pub fn toggle(&self) -> Result<RecorderStatus> {
        let active = self.running() && self.settings.get().recording.enabled;
        self.settings
            .patch(&json!({"recording": {"enabled": !active}}))?;
        if active {
            self.stop();
        } else {
            self.start()?;
        }
        Ok(self.status())
    }
}

#[derive(Default)]
struct Dedup {
    gray: Option<Vec<u8>>,
    histogram: Option<Vec<f64>>,
    hash: Option<String>,
}

impl Dedup {
    fn clear(&mut self) {
        self.gray = None;
        self.histogram = None;
        self.hash = None;
    }

    fn changed(&mut self, image: &RgbaImage) -> (bool, Option<f64>) {
        let thumbnail = image::imageops::resize(image, THUMB_W, THUMB_H, FilterType::Nearest);
        let gray = thumbnail
            .pixels()
            .map(|pixel| {
                let [r, g, b, _] = pixel.0;
                ((u32::from(r) * 77 + u32::from(g) * 150 + u32::from(b) * 29) >> 8) as u8
            })
            .collect::<Vec<_>>();
        let hash = format!("{:x}", Sha1::digest(&gray));
        let mut diff = None;
        if let Some(previous_hash) = &self.hash {
            if previous_hash == &hash {
                return (false, None);
            }
            let histogram = histogram(&gray);
            if hist_distance(&histogram, self.histogram.as_deref()) < HIST_CHANGE_THRESHOLD {
                let percent = pixel_diff(&gray, self.gray.as_deref(), PIXEL_TOLERANCE);
                diff = Some(percent);
                if percent < PIXEL_DIFF_THRESHOLD {
                    return (false, Some(percent));
                }
            }
        }
        self.histogram = Some(histogram(&gray));
        self.gray = Some(gray);
        self.hash = Some(hash);
        (true, diff)
    }
}

fn run_worker(
    settings: Arc<Settings>,
    store: Arc<DayStore>,
    inner: Arc<Mutex<Inner>>,
    stop: Arc<AtomicBool>,
) {
    let mut sampler = Sampler::new();
    let mut dedup = Dedup::default();
    let mut last_app = String::new();
    let mut last_title = String::new();
    let mut last_focus_at = 0_i64;
    let mut last_tick_at = now_ms();
    let mut last_heartbeat_at = last_tick_at;
    let mut last_rotation_at = last_tick_at;
    let mut last_loop_at = last_tick_at;
    let mut capture_at: Option<i64> = None;
    let mut last_site_key = String::new();
    let mut last_site_at = 0_i64;
    let mut was_locked = false;
    let mut was_idle = false;
    let mut last_date = local_date_key(last_tick_at);

    let _ = rotate(&settings, &store);
    while !stop.load(Ordering::Acquire) {
        let now = now_ms();
        if now - last_loop_at > 60_000 {
            append_event(
                &store,
                &inner,
                Event::at(last_loop_at, kinds::SUSPEND, json!({})),
            );
            append_event(&store, &inner, Event::at(now, kinds::RESUME, json!({})));
            dedup.clear();
            last_app.clear();
            last_title.clear();
        }
        last_loop_at = now;
        // Each day file must name its foreground app; otherwise work that runs
        // across midnight has no app until the next switch.
        let today = local_date_key(now);
        if today != last_date {
            last_date = today;
            last_app.clear();
            last_title.clear();
        }

        let locked = workstation_is_locked();
        if locked {
            if !was_locked {
                append_event(&store, &inner, Event::at(now, kinds::LOCK, json!({})));
                dedup.clear();
                was_locked = true;
            }
            update_state(&inner, true, was_idle, None);
            thread::sleep(Duration::from_millis(1_000));
            continue;
        }
        if was_locked {
            append_event(&store, &inner, Event::at(now, kinds::UNLOCK, json!({})));
            dedup.clear();
            was_locked = false;
            was_idle = false;
            last_app.clear();
            last_title.clear();
            last_tick_at = now;
        }

        let config = settings.get();
        let recording = config.recording;
        let mut sample = sampler.sample(recording.browser_sites);
        let sensitive = is_sensitive(&sample, &config.privacy.sensitive_patterns);
        if sensitive {
            // Redact at the source: the real title never reaches disk.
            sample.title = SENSITIVE_TITLE.to_owned();
            sample.site.clear();
        }
        // Meetings, videos and slides are watched without touching input.
        let passive = sample.fullscreen || is_meeting(&sample.name, &sample.app, &sample.title);
        let idle_threshold = if passive {
            recording
                .passive_idle_threshold_ms
                .max(recording.idle_threshold_ms)
        } else {
            recording.idle_threshold_ms
        };
        // The process name and label are tracked independently; this also catches
        // two windows whose localized labels happen to be equal.
        let identity = format!("{}\n{}", sample.app, sample.name);
        let app_changed = identity != last_app;
        let title_changed = sample.title != last_title;
        let idle = sample.idle_ms >= 0 && sample.idle_ms >= idle_threshold;

        update_state(&inner, false, idle, Some(sample.clone()));
        if app_changed
            || (title_changed && now - last_focus_at >= recording.title_throttle_ms as i64)
        {
            append_event(
                &store,
                &inner,
                Event::app_focus(
                    now,
                    &sample.app,
                    &sample.name,
                    &sample.title,
                    sample.pid,
                    sample.idle_ms,
                )
                .with_passive(passive),
            );
            last_focus_at = now;
            last_tick_at = now;
            if app_changed && !idle && recording.screenshots {
                capture_at = Some(now + recording.focus_debounce_ms as i64);
            }
            last_app = identity;
            last_title = sample.title.clone();
        } else if now - last_tick_at >= recording.tick_ms as i64 {
            if idle != was_idle || !idle {
                append_event(
                    &store,
                    &inner,
                    Event::at(now, kinds::TICK, json!({"i": sample.idle_ms})).with_passive(passive),
                );
            }
            if was_idle && !idle && recording.screenshots {
                capture_at = Some(now);
            }
            last_tick_at = now;
        }
        was_idle = idle;

        if recording.browser_sites && !idle && !sensitive {
            let site = normalize_site_host(&sample.site);
            let key = format!(
                "{}\n{site}",
                if sample.name.is_empty() {
                    &sample.app
                } else {
                    &sample.name
                }
            );
            if !site.is_empty() && (key != last_site_key || now - last_site_at >= 300_000) {
                append_event(
                    &store,
                    &inner,
                    Event::at(
                        now,
                        kinds::SITE_VISIT,
                        json!({"site":site, "app":sample.app, "name":sample.name, "pid":sample.pid, "i":sample.idle_ms}),
                    ),
                );
                last_site_key = key;
                last_site_at = now;
            }
        }

        if capture_at.is_some_and(|scheduled| scheduled <= now) {
            capture_at = None;
            if !idle
                && !sensitive
                && recording.screenshots
                && let Err(error) = capture("app_focus", &settings, &store, &sample, &mut dedup)
            {
                eprintln!("[Recorder] 截图失败：{error:#}");
            }
        }
        if now - last_heartbeat_at >= recording.heartbeat_ms.max(60_000) as i64 {
            last_heartbeat_at = now;
            if !idle
                && !sensitive
                && recording.screenshots
                && let Err(error) = capture("heartbeat", &settings, &store, &sample, &mut dedup)
            {
                eprintln!("[Recorder] 心跳截图失败：{error:#}");
            }
        }
        if now - last_rotation_at >= 30 * 60_000 {
            last_rotation_at = now;
            let _ = rotate(&settings, &store);
        }
        thread::sleep(Duration::from_millis(recording.poll_ms.clamp(250, 10_000)));
    }
    update_state(&inner, false, false, None);
}

fn append_event(store: &DayStore, inner: &Arc<Mutex<Inner>>, event: Event) {
    if let Err(error) = store.append(event) {
        inner.lock().expect("recorder lock").last_error = Some(format!("记录写入失败：{error}"));
    }
}

fn update_state(
    inner: &Arc<Mutex<Inner>>,
    locked: bool,
    idle: bool,
    frontmost: Option<ForegroundSample>,
) {
    let mut inner = inner.lock().expect("recorder lock");
    inner.locked = locked;
    inner.idle = idle;
    if frontmost.is_some() {
        inner.frontmost = frontmost;
    }
}

fn rotate(settings: &Settings, store: &DayStore) -> Result<()> {
    let storage = settings.get().storage;
    store.enforce_snapshot_cap(storage.max_snapshot_bytes)?;
    store.enforce_retention(storage.retention_days);
    Ok(())
}

fn is_sensitive(sample: &ForegroundSample, patterns: &[String]) -> bool {
    if sample.title == SENSITIVE_TITLE {
        return true;
    }
    let haystack = format!("{}\n{}\n{}", sample.app, sample.title, sample.site).to_lowercase();
    patterns.iter().any(|pattern| {
        let pattern = pattern.trim().to_lowercase();
        !pattern.is_empty() && haystack.contains(&pattern)
    })
}

fn capture(
    trigger: &str,
    settings: &Settings,
    store: &Arc<DayStore>,
    sample: &ForegroundSample,
    dedup: &mut Dedup,
) -> Result<Option<PathBuf>> {
    let config = settings.get();
    if !config.recording.screenshots || is_sensitive(sample, &config.privacy.sensitive_patterns) {
        return Ok(None);
    }
    let screens = screenshots::Screen::all().context("无法枚举显示器")?;
    // Capture the monitor that shows the foreground window, not always the
    // primary one; otherwise a window on a second screen yields an unrelated frame.
    let screen = sample
        .center
        .and_then(|(x, y)| screenshots::Screen::from_point(x, y).ok())
        .or_else(|| {
            screens
                .iter()
                .find(|screen| screen.display_info.is_primary)
                .or_else(|| screens.first())
                .copied()
        })
        .context("找不到显示器")?;
    let captured = screen.capture().context("无法截取主显示器")?;
    let (changed, diff) = dedup.changed(&captured);
    if !changed {
        return Ok(None);
    }

    let max_width = config.storage.max_snap_width.max(320);
    let width = captured.width().min(max_width);
    let height =
        ((captured.height() as u64 * width as u64) / captured.width().max(1) as u64).max(1) as u32;
    let resized = if width == captured.width() {
        captured
    } else {
        image::imageops::resize(&captured, width, height, FilterType::Triangle)
    };
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
        &mut jpeg,
        config.storage.jpeg_quality.clamp(30, 100),
    );
    DynamicImage::ImageRgba8(resized)
        .write_with_encoder(encoder)
        .context("JPEG 编码失败")?;

    let now = now_ms();
    let date_key = local_date_key(now);
    let directory = store.snapshot_dir(&date_key)?;
    fs::create_dir_all(&directory)?;
    let suffix = rand::rng().random::<u32>() & 0x00ff_ffff;
    let name = format!("{}-{suffix:06x}.jpg", Local::now().format("%H%M%S"));
    let path = directory.join(&name);
    fs::write(&path, &jpeg)?;
    let relative = PathBuf::from("snapshots").join(&date_key).join(&name);
    store.append(Event::at(
        now,
        kinds::SCREENSHOT,
        json!({
            "file": relative.to_string_lossy(),
            "w": width,
            "h": height,
            "bytes": jpeg.len(),
            "trigger": trigger,
            "diff": diff,
            "app": sample.app,
            "title": sample.title,
        }),
    ))?;
    Ok(Some(relative))
}

fn histogram(gray: &[u8]) -> Vec<f64> {
    let mut histogram = vec![0.0; HIST_BINS];
    for value in gray {
        histogram[(value >> 2) as usize] += 1.0;
    }
    let total = gray.len().max(1) as f64;
    for value in &mut histogram {
        *value /= total;
    }
    histogram
}

fn hist_distance(a: &[f64], b: Option<&[f64]>) -> f64 {
    let Some(b) = b.filter(|b| b.len() == a.len()) else {
        return 1.0;
    };
    a.iter().zip(b).map(|(a, b)| (a - b).abs()).sum::<f64>() / 2.0
}

fn pixel_diff(a: &[u8], b: Option<&[u8]>, tolerance: i16) -> f64 {
    let Some(b) = b.filter(|b| b.len() == a.len()) else {
        return 1.0;
    };
    let changed = a
        .iter()
        .zip(b)
        .filter(|(a, b)| (i16::from(**a) - i16::from(**b)).abs() > tolerance)
        .count();
    changed as f64 / a.len().max(1) as f64
}

pub fn normalize_site_host(value: &str) -> String {
    let candidate = value.trim();
    if candidate.is_empty() || candidate.len() > 4096 || candidate.chars().any(char::is_whitespace)
    {
        return String::new();
    }
    let has_scheme = candidate.find(':').is_some_and(|index| {
        candidate[..index].chars().enumerate().all(|(i, ch)| {
            ch.is_ascii_alphabetic() && i == 0
                || ch.is_ascii_alphanumeric() && i > 0
                || i > 0 && matches!(ch, '+' | '.' | '-')
        })
    });
    if has_scheme
        && !candidate
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        && !candidate
            .get(..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    {
        return String::new();
    }
    let parsed = if has_scheme {
        Url::parse(candidate)
    } else {
        Url::parse(&format!("https://{candidate}"))
    };
    let Ok(url) = parsed else {
        return String::new();
    };
    if !matches!(url.scheme(), "http" | "https") {
        return String::new();
    }
    let mut host = url.host_str().unwrap_or_default().to_lowercase();
    if host.ends_with('.') {
        host.pop();
    }
    if host.starts_with("www.") {
        host = host[4..].to_owned();
    }
    if host.is_empty()
        || host.len() > 253
        || host != "localhost" && !host.contains('.') && IpAddr::from_str(&host).is_err()
    {
        return String::new();
    }
    host
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_capture_is_host_only_and_http_only() {
        assert_eq!(
            normalize_site_host("https://www.example.com/private?token=secret"),
            "example.com"
        );
        assert_eq!(normalize_site_host("example.com/path"), "example.com");
        assert_eq!(
            normalize_site_host("http://localhost:8080/private"),
            "localhost"
        );
        assert_eq!(normalize_site_host("file:///C:/private.txt"), "");
        assert_eq!(normalize_site_host("javascript:alert(1)"), "");
        assert_eq!(normalize_site_host("not a url"), "");
    }

    #[test]
    fn dedup_rejects_identical_and_near_identical_frames() {
        let first = RgbaImage::from_pixel(64, 64, image::Rgba([30, 80, 120, 255]));
        let mut dedup = Dedup::default();
        assert!(dedup.changed(&first).0);
        assert!(!dedup.changed(&first).0);
        assert!(!dedup.changed(&first).0);
        let different = RgbaImage::from_pixel(64, 64, image::Rgba([240, 220, 20, 255]));
        assert!(dedup.changed(&different).0);
    }

    #[test]
    fn sensitive_matching_uses_app_and_title() {
        let sample = ForegroundSample {
            app: "Browser".to_owned(),
            title: "个人网银登录".to_owned(),
            ..Default::default()
        };
        assert!(is_sensitive(&sample, &["网银".to_owned()]));
        assert!(!is_sensitive(&sample, &["password".to_owned()]));
    }
}
