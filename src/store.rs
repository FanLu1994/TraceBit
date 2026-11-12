use crate::categories::{categorize, is_browser_process, normalize_proc, site_category};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn local_datetime(ts: i64) -> Option<DateTime<Local>> {
    Local.timestamp_millis_opt(ts).single()
}

pub fn local_date_key(ts: i64) -> String {
    local_datetime(ts)
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "1970-01-01".to_owned())
}

pub fn hhmm(ts: i64) -> String {
    local_datetime(ts)
        .map(|date| date.format("%H:%M").to_string())
        .unwrap_or_default()
}

pub fn validate_date_key(date_key: &str) -> Result<()> {
    let valid = date_key.len() == 10
        && date_key.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 4 | 7) {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        });
    if valid {
        Ok(())
    } else {
        bail!("日期格式无效")
    }
}

/// Schema version written on new events. Version 1 lines have no `v` field.
pub const EVENT_VERSION: u8 = 2;

/// Event kinds. `kind` stays a string on disk for compatibility with old files.
pub mod kinds {
    pub const APP_FOCUS: &str = "app_focus";
    pub const TICK: &str = "tick";
    pub const LOCK: &str = "lock";
    pub const UNLOCK: &str = "unlock";
    pub const SUSPEND: &str = "suspend";
    pub const RESUME: &str = "resume";
    pub const SITE_VISIT: &str = "site_visit";
    pub const SCREENSHOT: &str = "screenshot";
}

/// Stays shorter than this are Alt-Tab pass-throughs, not real use.
pub const MIN_STAY_MS: i64 = 3_000;
/// Stays of one app separated by at most this much are one continuous stay, and
/// a change of app across at most this gap is one switch.
pub const CONTIGUOUS_GAP_MS: i64 = 5_000;
const MAX_TITLES_PER_STAY: usize = 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub ts: i64,
    pub kind: String,
    #[serde(default)]
    pub data: Value,
    #[serde(default, skip_serializing_if = "is_legacy_version")]
    pub v: u8,
}

fn is_legacy_version(version: &u8) -> bool {
    *version == 0
}

impl Event {
    pub fn new(kind: impl Into<String>, data: Value) -> Self {
        Self::at(now_ms(), kind, data)
    }

    pub fn at(ts: i64, kind: impl Into<String>, data: Value) -> Self {
        Self {
            ts,
            kind: kind.into(),
            data,
            v: EVENT_VERSION,
        }
    }

    pub fn app_focus(ts: i64, app: &str, name: &str, title: &str, pid: u32, idle_ms: i64) -> Self {
        Self::at(
            ts,
            kinds::APP_FOCUS,
            json!({"app": app, "name": name, "title": title, "pid": pid, "i": idle_ms}),
        )
    }

    /// `p` marks passive context (meeting or full-screen) that uses the longer
    /// idle threshold.
    pub fn with_passive(mut self, passive: bool) -> Self {
        if passive && let Some(data) = self.data.as_object_mut() {
            data.insert("p".to_owned(), Value::Bool(true));
        }
        self
    }

    fn idle_ms(&self) -> Option<i64> {
        self.data.get("i").and_then(Value::as_i64)
    }

    fn passive(&self) -> bool {
        self.data.get("p").and_then(Value::as_bool).unwrap_or(false)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stay {
    pub start: i64,
    pub end: i64,
    pub app: String,
    pub name: String,
    pub title: String,
    pub titles: BTreeSet<String>,
    /// Milliseconds spent per window title inside this stay.
    #[serde(default)]
    pub title_ms: BTreeMap<String, i64>,
    /// Milliseconds spent per browser host inside this stay.
    #[serde(default)]
    pub site_ms: BTreeMap<String, i64>,
    /// Dominant category, including browser time reassigned by site.
    #[serde(default)]
    pub category: String,
    pub pid: u32,
    #[serde(skip)]
    pub(crate) title_at: i64,
    #[serde(skip)]
    pub(crate) site: String,
    #[serde(skip)]
    pub(crate) site_at: i64,
}

impl Stay {
    fn from_focus(start: i64, data: &Value) -> Self {
        let title = string_field(data, "title");
        Self {
            start,
            end: start,
            app: nonempty(string_field(data, "app"), string_field(data, "name"), "?"),
            name: string_field(data, "name"),
            titles: if title.is_empty() {
                BTreeSet::new()
            } else {
                BTreeSet::from([title.clone()])
            },
            title,
            pid: data.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32,
            title_at: start,
            site_at: start,
            ..Default::default()
        }
    }

    pub fn category(&self) -> &str {
        if self.category.is_empty() {
            categorize(&self.name, &self.app, &self.title)
        } else {
            &self.category
        }
    }

    pub fn duration(&self) -> i64 {
        (self.end - self.start).max(0)
    }

    fn switch_title(&mut self, ts: i64, title: String) {
        if title.is_empty() || title == self.title {
            return;
        }
        self.credit_title(ts);
        self.title_at = ts;
        self.titles.insert(title.clone());
        self.title = title;
        if self.titles.len() > MAX_TITLES_PER_STAY {
            // Evict the title with the least time, never the current one.
            let evict = self
                .titles
                .iter()
                .filter(|title| **title != self.title)
                .min_by_key(|title| self.title_ms.get(*title).copied().unwrap_or(0))
                .cloned();
            if let Some(evict) = evict {
                self.titles.remove(&evict);
                self.title_ms.remove(&evict);
            }
        }
    }

    fn switch_site(&mut self, ts: i64, site: String) {
        if site == self.site {
            return;
        }
        self.credit_site(ts);
        self.site = site;
        self.site_at = ts;
    }

    fn credit_title(&mut self, until: i64) {
        let ms = (until - self.title_at.max(self.start)).max(0);
        if !self.title.is_empty() && ms > 0 {
            *self.title_ms.entry(self.title.clone()).or_insert(0) += ms;
        }
    }

    fn credit_site(&mut self, until: i64) {
        let ms = (until - self.site_at.max(self.start)).max(0);
        if !self.site.is_empty() && ms > 0 {
            *self.site_ms.entry(self.site.clone()).or_insert(0) += ms;
        }
    }

    fn close(mut self, end: i64) -> Self {
        self.end = self.start.max(end);
        let end = self.end;
        self.credit_title(end);
        self.credit_site(end);
        self.title_at = end;
        self.site_at = end;
        self
    }

    fn absorb(&mut self, next: Stay) {
        self.end = self.end.max(next.end);
        self.title = next.title;
        self.titles.extend(next.titles);
        for (title, ms) in next.title_ms {
            *self.title_ms.entry(title).or_insert(0) += ms;
        }
        for (site, ms) in next.site_ms {
            *self.site_ms.entry(site).or_insert(0) += ms;
        }
    }

    /// Time per category. Browser time with a known host goes to that host's
    /// category; the rest stays with the stay's own category.
    fn category_split(&self) -> Vec<(String, i64)> {
        let base = categorize(&self.name, &self.app, &self.title).to_owned();
        let total = self.duration();
        let mut split: BTreeMap<String, i64> = BTreeMap::new();
        let mut assigned = 0;
        if is_browser_process(&self.name) {
            for (site, ms) in &self.site_ms {
                if let Some(category) = site_category(site) {
                    let ms = (*ms).min(total - assigned).max(0);
                    *split.entry(category.to_owned()).or_insert(0) += ms;
                    assigned += ms;
                }
            }
        }
        if total - assigned > 0 {
            *split.entry(base).or_insert(0) += total - assigned;
        }
        let mut split = split.into_iter().collect::<Vec<_>>();
        split.sort_by_key(|(_, ms)| std::cmp::Reverse(*ms));
        split
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub start: i64,
    pub end: i64,
    pub active_ms: i64,
    pub apps: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStat {
    pub name: String,
    pub app: String,
    pub category: String,
    pub ms: i64,
    pub switches: u32,
    /// Number of stays in which each title appeared.
    pub titles: BTreeMap<String, u32>,
    /// Milliseconds per title.
    #[serde(default)]
    pub title_ms: BTreeMap<String, i64>,
}

impl AppStat {
    /// Titles ordered by time spent, longest first.
    pub fn top_titles(&self, max: usize) -> Vec<(&String, i64)> {
        let mut titles = self
            .title_ms
            .iter()
            .map(|(title, ms)| (title, *ms))
            .collect::<Vec<_>>();
        titles.sort_by_key(|(_, ms)| std::cmp::Reverse(*ms));
        titles.truncate(max);
        titles
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryStat {
    pub name: String,
    pub ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub ts: i64,
    pub file: String,
    pub w: u32,
    pub h: u32,
    pub bytes: u64,
    pub trigger: String,
    pub diff: Option<f64>,
    pub app: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteStat {
    pub site: String,
    pub visits: u32,
    pub first_seen: i64,
    pub last_seen: i64,
    pub app: String,
    #[serde(default)]
    pub ms: i64,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusBlock {
    pub app: String,
    pub ms: i64,
    pub started_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayStats {
    pub date_key: String,
    pub events: usize,
    pub stays: Vec<Stay>,
    pub sessions: Vec<Session>,
    pub apps: Vec<AppStat>,
    pub hourly: Vec<i64>,
    /// Active milliseconds per category for each local hour.
    #[serde(default)]
    pub hourly_categories: Vec<BTreeMap<String, i64>>,
    /// Single source of truth for category totals (UI and AI both use it).
    #[serde(default)]
    pub categories: Vec<CategoryStat>,
    pub switches: u32,
    pub total_active_ms: i64,
    pub first_active: Option<i64>,
    pub last_active: Option<i64>,
    pub screenshots: Vec<Snapshot>,
    pub sites: Vec<SiteStat>,
    pub lock_events: usize,
    pub long_idles: usize,
    pub longest_idle_ms: i64,
    pub longest_block: Option<FocusBlock>,
    pub snapshot_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeriveOptions {
    pub idle_threshold_ms: i64,
    pub passive_idle_threshold_ms: i64,
    pub session_gap_ms: i64,
}

impl Default for DeriveOptions {
    fn default() -> Self {
        Self {
            idle_threshold_ms: 60_000,
            passive_idle_threshold_ms: 900_000,
            session_gap_ms: 600_000,
        }
    }
}

impl DeriveOptions {
    pub fn from_settings(recording: &crate::settings::RecordingSettings) -> Self {
        Self {
            idle_threshold_ms: recording.idle_threshold_ms,
            passive_idle_threshold_ms: recording
                .passive_idle_threshold_ms
                .max(recording.idle_threshold_ms),
            session_gap_ms: recording.session_gap_ms,
        }
    }

    fn threshold(&self, passive: bool) -> i64 {
        if passive {
            self.passive_idle_threshold_ms.max(self.idle_threshold_ms)
        } else {
            self.idle_threshold_ms
        }
        .max(1)
    }
}

type Fingerprint = (u64, u128);

#[derive(Debug)]
struct CacheEntry {
    fingerprint: Fingerprint,
    events: Vec<Event>,
}

#[derive(Debug)]
struct StatsEntry {
    key: (Fingerprint, Option<Fingerprint>, DeriveOptions),
    stats: DayStats,
}

#[derive(Debug)]
pub struct DayStore {
    pub data_dir: PathBuf,
    pub days_dir: PathBuf,
    pub snapshots_dir: PathBuf,
    cache: Mutex<HashMap<String, CacheEntry>>,
    stats_cache: Mutex<HashMap<String, StatsEntry>>,
}

impl DayStore {
    pub fn new(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let days_dir = data_dir.join("days");
        let snapshots_dir = data_dir.join("snapshots");
        fs::create_dir_all(&days_dir)?;
        fs::create_dir_all(&snapshots_dir)?;
        Ok(Self {
            data_dir,
            days_dir,
            snapshots_dir,
            cache: Mutex::new(HashMap::new()),
            stats_cache: Mutex::new(HashMap::new()),
        })
    }

    pub fn append(&self, event: Event) -> Result<String> {
        let date_key = local_date_key(event.ts);
        let file = self.day_file(&date_key)?;
        let mut line = serde_json::to_vec(&event)?;
        line.push(b'\n');
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .with_context(|| format!("无法写入 {}", file.display()))?
            .write_all(&line)?;
        let mut cache = self.cache.lock().expect("store cache lock");
        if let Some(entry) = cache.get_mut(&date_key) {
            entry.events.push(event);
            if let Ok(metadata) = fs::metadata(&file) {
                entry.fingerprint = fingerprint(&metadata);
            }
        }
        Ok(date_key)
    }

    pub fn day_file(&self, date_key: &str) -> Result<PathBuf> {
        validate_date_key(date_key)?;
        Ok(self.days_dir.join(format!("{date_key}.jsonl")))
    }

    pub fn snapshot_dir(&self, date_key: &str) -> Result<PathBuf> {
        validate_date_key(date_key)?;
        Ok(self.snapshots_dir.join(date_key))
    }

    pub fn snapshot_path(&self, relative: &str) -> Option<PathBuf> {
        let path = Path::new(relative);
        if path.is_absolute()
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return None;
        }
        Some(self.data_dir.join(path))
    }

    fn day_fingerprint(&self, date_key: &str) -> Option<Fingerprint> {
        let file = self.day_file(date_key).ok()?;
        fs::metadata(file)
            .ok()
            .map(|metadata| fingerprint(&metadata))
    }

    pub fn read_day(&self, date_key: &str) -> Result<Vec<Event>> {
        let file = self.day_file(date_key)?;
        let Ok(metadata) = fs::metadata(&file) else {
            return Ok(Vec::new());
        };
        let fingerprint = fingerprint(&metadata);
        if let Some(entry) = self.cache.lock().expect("store cache lock").get(date_key)
            && entry.fingerprint == fingerprint
        {
            return Ok(entry.events.clone());
        }
        let raw =
            fs::read_to_string(&file).with_context(|| format!("无法读取 {}", file.display()))?;
        let events = raw
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                (!line.is_empty())
                    .then(|| serde_json::from_str(line).ok())
                    .flatten()
            })
            .collect::<Vec<Event>>();
        let mut cache = self.cache.lock().expect("store cache lock");
        if cache.len() >= 40 {
            cache.clear();
        }
        cache.insert(
            date_key.to_owned(),
            CacheEntry {
                fingerprint,
                events: events.clone(),
            },
        );
        Ok(events)
    }

    pub fn list_days(&self) -> Vec<String> {
        let mut days = fs::read_dir(&self.days_dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".jsonl")
                    .filter(|date| validate_date_key(date).is_ok())
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>();
        days.sort();
        days
    }

    pub fn derive(&self, events: &[Event], options: DeriveOptions) -> DayStats {
        derive_events(events, options, None)
    }

    /// The foreground window that was active when `date_key` began, if the
    /// previous day ended without a lock or suspend. Without it, activity that
    /// continues across midnight has no app until the next switch.
    fn carry_over_focus(&self, date_key: &str) -> Option<Event> {
        let previous = previous_date_key(date_key)?;
        let events = self.read_day(&previous).ok()?;
        events
            .iter()
            .rev()
            .find(|event| {
                matches!(
                    event.kind.as_str(),
                    kinds::APP_FOCUS | kinds::LOCK | kinds::SUSPEND
                )
            })
            .filter(|event| event.kind == kinds::APP_FOCUS)
            .cloned()
    }

    pub fn day_stats(&self, date_key: &str, options: DeriveOptions) -> Result<DayStats> {
        validate_date_key(date_key)?;
        let key = (
            self.day_fingerprint(date_key).unwrap_or_default(),
            previous_date_key(date_key).and_then(|previous| self.day_fingerprint(&previous)),
            options,
        );
        let cached = self
            .stats_cache
            .lock()
            .expect("stats cache lock")
            .get(date_key)
            .filter(|entry| entry.key == key)
            .map(|entry| entry.stats.clone());
        let mut stats = match cached {
            Some(stats) => stats,
            None => {
                let events = self.read_day(date_key)?;
                let seed = self.carry_over_focus(date_key);
                let mut stats = derive_events(&events, options, seed.as_ref());
                stats.date_key = date_key.to_owned();
                stats.screenshots = screenshot_events(&events);
                let mut cache = self.stats_cache.lock().expect("stats cache lock");
                if cache.len() >= 40 {
                    cache.clear();
                }
                cache.insert(
                    date_key.to_owned(),
                    StatsEntry {
                        key,
                        stats: stats.clone(),
                    },
                );
                stats
            }
        };
        // Files can be removed by rotation without touching the day log, so the
        // existence filter runs on every call, using one directory listing.
        let (files, bytes) = self.snapshot_files(date_key);
        stats.screenshots.retain(|shot| files.contains(&shot.file));
        stats.snapshot_bytes = bytes;
        Ok(stats)
    }

    /// Screenshots of a day without deriving activity.
    pub fn screenshots(&self, date_key: &str) -> Result<Vec<Snapshot>> {
        let events = self.read_day(date_key)?;
        let (files, _) = self.snapshot_files(date_key);
        let mut shots = screenshot_events(&events);
        shots.retain(|shot| files.contains(&shot.file));
        Ok(shots)
    }

    /// Relative paths (as stored in events) and total bytes of one day's snapshots.
    fn snapshot_files(&self, date_key: &str) -> (HashSet<String>, u64) {
        let mut files = HashSet::new();
        let mut bytes = 0;
        let Ok(dir) = self.snapshot_dir(date_key) else {
            return (files, bytes);
        };
        for entry in fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            if let Ok(metadata) = entry.metadata() {
                bytes += metadata.len();
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = PathBuf::from("snapshots").join(date_key).join(&name);
            files.insert(relative.to_string_lossy().into_owned());
            files.insert(format!("snapshots/{date_key}/{name}"));
        }
        (files, bytes)
    }

    pub fn list_snapshot_days(&self) -> Vec<String> {
        let mut days = fs::read_dir(&self.snapshots_dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                validate_date_key(&name).ok().map(|_| name)
            })
            .collect::<Vec<_>>();
        days.sort();
        days
    }

    pub fn day_snapshot_bytes(&self, date_key: &str) -> u64 {
        self.snapshot_dir(date_key)
            .ok()
            .and_then(|dir| fs::read_dir(dir).ok())
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .map(|metadata| metadata.len())
            .sum()
    }

    pub fn total_snapshot_bytes(&self) -> u64 {
        self.list_snapshot_days()
            .iter()
            .map(|day| self.day_snapshot_bytes(day))
            .sum()
    }

    pub fn enforce_snapshot_cap(&self, max_bytes: u64) -> Result<(u64, Vec<String>)> {
        let target = max_bytes.saturating_mul(80) / 100;
        let mut total = self.total_snapshot_bytes();
        let mut deleted = Vec::new();
        if total <= max_bytes {
            return Ok((total, deleted));
        }
        for day in self.list_snapshot_days() {
            if total <= target {
                break;
            }
            let day_bytes = self.day_snapshot_bytes(&day);
            if let Ok(dir) = self.snapshot_dir(&day) {
                let _ = fs::remove_dir_all(dir);
            }
            total = total.saturating_sub(day_bytes);
            deleted.push(day);
        }
        Ok((total, deleted))
    }

    pub fn enforce_retention(&self, retention_days: u32) -> Vec<String> {
        let cutoff = local_date_key(now_ms() - i64::from(retention_days) * 86_400_000);
        let mut removed = Vec::new();
        for day in self.list_days() {
            if day < cutoff {
                if let Ok(file) = self.day_file(&day) {
                    let _ = fs::remove_file(file);
                }
                self.cache.lock().expect("store cache lock").remove(&day);
                self.stats_cache
                    .lock()
                    .expect("stats cache lock")
                    .remove(&day);
                removed.push(day);
            }
        }
        for day in self.list_snapshot_days() {
            if day < cutoff
                && let Ok(dir) = self.snapshot_dir(&day)
            {
                let _ = fs::remove_dir_all(dir);
            }
        }
        removed
    }

    pub fn purge_screenshots(&self) -> Result<()> {
        if self.snapshots_dir.exists() {
            fs::remove_dir_all(&self.snapshots_dir)?;
        }
        fs::create_dir_all(&self.snapshots_dir)?;
        Ok(())
    }

    pub fn purge_all(&self) -> Result<()> {
        for dir in [&self.days_dir, &self.snapshots_dir] {
            if dir.exists() {
                fs::remove_dir_all(dir)?;
            }
            fs::create_dir_all(dir)?;
        }
        self.cache.lock().expect("store cache lock").clear();
        self.stats_cache.lock().expect("stats cache lock").clear();
        Ok(())
    }
}

fn fingerprint(metadata: &fs::Metadata) -> (u64, u128) {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    (metadata.len(), modified)
}

fn string_field(data: &Value, key: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn optional_string_field(data: &Value, key: &str) -> Option<String> {
    data.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn nonempty(app: String, name: String, fallback: &str) -> String {
    if !app.is_empty() {
        app
    } else if !name.is_empty() {
        name
    } else {
        fallback.to_owned()
    }
}

pub fn app_key(stay: &Stay) -> String {
    if stay.name.is_empty() {
        stay.app.clone()
    } else {
        stay.name.clone()
    }
}

fn previous_date_key(date_key: &str) -> Option<String> {
    let date = chrono::NaiveDate::parse_from_str(date_key, "%Y-%m-%d").ok()?;
    Some((date - Duration::days(1)).format("%Y-%m-%d").to_string())
}

fn screenshot_events(events: &[Event]) -> Vec<Snapshot> {
    events
        .iter()
        .filter(|event| event.kind == kinds::SCREENSHOT)
        .map(|event| Snapshot {
            ts: event.ts,
            file: string_field(&event.data, "file"),
            w: event.data.get("w").and_then(Value::as_u64).unwrap_or(0) as u32,
            h: event.data.get("h").and_then(Value::as_u64).unwrap_or(0) as u32,
            bytes: event.data.get("bytes").and_then(Value::as_u64).unwrap_or(0),
            trigger: string_field(&event.data, "trigger"),
            diff: event.data.get("diff").and_then(Value::as_f64),
            app: optional_string_field(&event.data, "app"),
            title: optional_string_field(&event.data, "title"),
        })
        .filter(|shot| !shot.file.is_empty())
        .collect()
}

fn valid_site(site: &str) -> bool {
    !site.is_empty()
        && site.len() <= 253
        && !site
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '/' | '?' | '#'))
}

/// Turns the raw event log of one day into activity statistics.
///
/// `seed` is the focus event active at the start of the day (carried over from
/// the previous day); it only applies until the first focus event of the day.
pub fn derive_events(events: &[Event], options: DeriveOptions, seed: Option<&Event>) -> DayStats {
    let session_gap_ms = options.session_gap_ms.max(1);
    let mut raw: Vec<Stay> = Vec::new();
    let mut current: Option<Stay> = None;
    let mut last_focus: Option<Value> = seed.map(|event| event.data.clone());
    let mut last_site_by_proc: HashMap<String, String> = HashMap::new();
    let mut site_visits: BTreeMap<String, SiteStat> = BTreeMap::new();
    let mut lock_events = 0;

    let close = |current: &mut Option<Stay>, raw: &mut Vec<Stay>, end: i64| {
        if let Some(stay) = current.take() {
            raw.push(stay.close(end));
        }
    };
    let open = |ts: i64, data: &Value, sites: &HashMap<String, String>| {
        let mut stay = Stay::from_focus(ts, data);
        if let Some(site) = sites.get(&normalize_proc(&stay.name)) {
            stay.site = site.clone();
            stay.site_at = ts;
        }
        stay
    };

    for event in events {
        let ts = event.ts;
        let idle_ms = event.idle_ms().unwrap_or(0).max(0);
        let is_idle_now = event
            .idle_ms()
            .is_some_and(|idle| idle >= 0 && idle >= options.threshold(event.passive()));
        match event.kind.as_str() {
            kinds::APP_FOCUS => {
                last_focus = Some(event.data.clone());
                if is_idle_now {
                    close(&mut current, &mut raw, ts - idle_ms);
                    continue;
                }
                let name = string_field(&event.data, "name");
                let app = string_field(&event.data, "app");
                let same_app = current
                    .as_ref()
                    .is_some_and(|stay| stay.name == name && stay.app == app);
                if same_app {
                    if let Some(stay) = current.as_mut() {
                        stay.end = ts;
                        stay.switch_title(ts, string_field(&event.data, "title"));
                    }
                } else {
                    close(&mut current, &mut raw, ts);
                    current = Some(open(ts, &event.data, &last_site_by_proc));
                }
            }
            kinds::TICK => {
                if is_idle_now {
                    close(&mut current, &mut raw, ts - idle_ms);
                } else {
                    if current.is_none()
                        && let Some(focus) = &last_focus
                    {
                        current = Some(open(ts - idle_ms, focus, &last_site_by_proc));
                    }
                    if let Some(stay) = current.as_mut() {
                        stay.end = ts;
                    }
                }
            }
            kinds::SITE_VISIT => {
                let site = string_field(&event.data, "site").to_lowercase();
                if !valid_site(&site) {
                    continue;
                }
                let proc = normalize_proc(&string_field(&event.data, "name"));
                last_site_by_proc.insert(proc.clone(), site.clone());
                if let Some(stay) = current.as_mut()
                    && normalize_proc(&stay.name) == proc
                {
                    stay.end = stay.end.max(ts);
                    stay.switch_site(ts, site.clone());
                }
                let app = string_field(&event.data, "app");
                let item = site_visits.entry(site.clone()).or_insert_with(|| SiteStat {
                    site: site.clone(),
                    visits: 0,
                    first_seen: ts,
                    last_seen: ts,
                    app: app.clone(),
                    ms: 0,
                    category: site_category(&site).map(str::to_owned),
                });
                item.visits += 1;
                item.last_seen = ts;
                if !app.is_empty() {
                    item.app = app;
                }
            }
            kinds::LOCK | kinds::SUSPEND => {
                lock_events += 1;
                close(&mut current, &mut raw, ts);
                last_focus = None;
            }
            _ => {}
        }
    }
    if let Some(stay) = current.take() {
        let end = stay.end;
        raw.push(stay.close(end));
    }

    // Drop Alt-Tab pass-throughs, then merge what they split apart.
    let mut stays: Vec<Stay> = Vec::new();
    for stay in raw
        .into_iter()
        .filter(|stay| stay.duration() >= MIN_STAY_MS)
    {
        if let Some(previous) = stays.last_mut()
            && app_key(previous) == app_key(&stay)
            && previous.app == stay.app
            && stay.start - previous.end <= CONTIGUOUS_GAP_MS
        {
            previous.absorb(stay);
            continue;
        }
        stays.push(stay);
    }
    let mut category_totals: BTreeMap<String, i64> = BTreeMap::new();
    for stay in &mut stays {
        let split = stay.category_split();
        for (category, ms) in &split {
            *category_totals.entry(category.clone()).or_insert(0) += ms;
        }
        stay.category = split
            .first()
            .map(|(category, _)| category.clone())
            .unwrap_or_else(|| categorize(&stay.name, &stay.app, &stay.title).to_owned());
    }

    let is_switch = |pair: &[Stay]| {
        app_key(&pair[0]) != app_key(&pair[1]) && pair[1].start - pair[0].end <= CONTIGUOUS_GAP_MS
    };
    let switches = stays.windows(2).filter(|pair| is_switch(pair)).count() as u32;

    let mut sessions: Vec<Session> = Vec::new();
    for stay in &stays {
        if let Some(session) = sessions.last_mut()
            && stay.start - session.end < session_gap_ms
        {
            session.end = stay.end;
            session.apps.insert(app_key(stay));
            session.active_ms += stay.duration();
            continue;
        }
        sessions.push(Session {
            start: stay.start,
            end: stay.end,
            active_ms: stay.duration(),
            apps: BTreeSet::from([app_key(stay)]),
        });
    }

    let mut apps: BTreeMap<String, AppStat> = BTreeMap::new();
    for stay in &stays {
        let key = app_key(stay);
        let app = apps.entry(key.clone()).or_insert_with(|| AppStat {
            name: key,
            app: stay.app.clone(),
            category: categorize(&stay.name, &stay.app, &stay.title).to_owned(),
            ms: 0,
            switches: 0,
            titles: BTreeMap::new(),
            title_ms: BTreeMap::new(),
        });
        app.ms += stay.duration();
        for title in &stay.titles {
            *app.titles.entry(title.clone()).or_insert(0) += 1;
        }
        for (title, ms) in &stay.title_ms {
            *app.title_ms.entry(title.clone()).or_insert(0) += ms;
        }
    }
    for pair in stays.windows(2).filter(|pair| is_switch(pair)) {
        if let Some(app) = apps.get_mut(&app_key(&pair[1])) {
            app.switches += 1;
        }
    }

    let mut hourly = vec![0_i64; 24];
    let mut hourly_categories = vec![BTreeMap::<String, i64>::new(); 24];
    for stay in &stays {
        let mut cursor = stay.start;
        while cursor < stay.end {
            let Some(date) = local_datetime(cursor) else {
                break;
            };
            let hour_start = date
                .with_minute(0)
                .and_then(|date| date.with_second(0))
                .and_then(|date| date.with_nanosecond(0));
            let Some(hour_start) = hour_start else {
                break;
            };
            let next_hour = (hour_start + Duration::hours(1)).timestamp_millis();
            let segment_end = stay.end.min(next_hour);
            if segment_end <= cursor {
                break;
            }
            let hour = date.hour() as usize;
            hourly[hour] += segment_end - cursor;
            *hourly_categories[hour]
                .entry(stay.category.clone())
                .or_insert(0) += segment_end - cursor;
            cursor = segment_end;
        }
    }

    for stay in &stays {
        for (site, ms) in &stay.site_ms {
            if let Some(item) = site_visits.get_mut(site) {
                item.ms += ms;
            }
        }
    }
    let mut sites = site_visits.into_values().collect::<Vec<_>>();
    sites.sort_by(|a, b| b.ms.cmp(&a.ms).then(b.last_seen.cmp(&a.last_seen)));

    let mut categories = category_totals
        .into_iter()
        .filter(|(_, ms)| *ms > 0)
        .map(|(name, ms)| CategoryStat { name, ms })
        .collect::<Vec<_>>();
    categories.sort_by_key(|category| std::cmp::Reverse(category.ms));

    let mut long_idles = 0;
    let mut longest_idle_ms = 0;
    for pair in sessions.windows(2) {
        let gap = pair[1].start - pair[0].end;
        if gap >= 5 * 60_000 {
            long_idles += 1;
        }
        longest_idle_ms = longest_idle_ms.max(gap);
    }

    let mut longest_block: Option<FocusBlock> = None;
    let mut block: Option<FocusBlock> = None;
    let mut block_end = 0;
    for stay in &stays {
        let key = app_key(stay);
        if block
            .as_ref()
            .is_some_and(|block| block.app == key && stay.start - block_end <= 60_000)
        {
            block_end = stay.end;
            if let Some(block) = block.as_mut() {
                block.ms = block_end - block.started_at;
            }
        } else {
            replace_longest(&mut longest_block, block.take());
            block = Some(FocusBlock {
                app: key,
                ms: stay.duration(),
                started_at: stay.start,
            });
            block_end = stay.end;
        }
    }
    replace_longest(&mut longest_block, block);

    DayStats {
        date_key: String::new(),
        events: events.len(),
        total_active_ms: stays.iter().map(Stay::duration).sum(),
        first_active: stays.first().map(|stay| stay.start),
        last_active: stays.last().map(|stay| stay.end),
        stays,
        sessions,
        apps: apps.into_values().collect(),
        hourly,
        hourly_categories,
        categories,
        switches,
        screenshots: Vec::new(),
        sites,
        lock_events,
        long_idles,
        longest_idle_ms,
        longest_block,
        snapshot_bytes: 0,
    }
}

fn replace_longest(longest: &mut Option<FocusBlock>, candidate: Option<FocusBlock>) {
    if let Some(candidate) = candidate
        && longest
            .as_ref()
            .is_none_or(|current| candidate.ms > current.ms)
    {
        *longest = Some(candidate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn append_sample(store: &DayStore, base: i64) {
        store
            .append(Event::app_focus(base, "Code", "code", "a.js", 1, 1_000))
            .unwrap();
        store
            .append(Event::app_focus(
                base + 60_000,
                "Code",
                "code",
                "b.js",
                1,
                1_000,
            ))
            .unwrap();
        store
            .append(Event::app_focus(
                base + 120_000,
                "Chrome",
                "chrome",
                "x.com",
                2,
                1_000,
            ))
            .unwrap();
        store
            .append(Event::at(
                base + 125_000,
                "site_visit",
                json!({"site":"example.com", "app":"Chrome", "name":"chrome", "pid":2, "i":1000}),
            ))
            .unwrap();
        store
            .append(Event::at(base + 180_000, "tick", json!({"i": 5000})))
            .unwrap();
        store
            .append(Event::at(base + 240_000, "tick", json!({"i": 65000})))
            .unwrap();
        store
            .append(Event::at(base + 300_000, "tick", json!({"i": 1000})))
            .unwrap();
    }

    #[test]
    fn old_smoke_store_derivation_matches() {
        let temp = tempfile::tempdir().unwrap();
        let store = DayStore::new(temp.path()).unwrap();
        let base = now_ms() - 500_000;
        append_sample(&store, base);
        let day = local_date_key(base);
        let snapshot_dir = store.snapshot_dir(&day).unwrap();
        fs::create_dir_all(&snapshot_dir).unwrap();
        fs::write(snapshot_dir.join("y.jpg"), b"test").unwrap();
        store
            .append(Event::at(
                base + 150_000,
                "screenshot",
                json!({"file":format!("snapshots/{day}/y.jpg"), "w":1600, "h":900, "bytes":100, "trigger":"test", "diff":null}),
            ))
            .unwrap();
        let stats = store
            .day_stats(&local_date_key(base), DeriveOptions::default())
            .unwrap();
        // The 1 s blip after the idle period is a transient, not activity.
        assert_eq!(stats.total_active_ms, 175_000);
        assert_eq!(stats.apps.len(), 2);
        assert_eq!(stats.switches, 1);
        assert_eq!(stats.screenshots.len(), 1);
        assert_eq!(stats.sites.len(), 1);
        assert_eq!(stats.sites[0].site, "example.com");
        assert_eq!(stats.longest_block.unwrap().ms, 120_000);
    }

    #[test]
    fn lock_closes_activity_and_unlock_does_not_invent_time() {
        let temp = tempfile::tempdir().unwrap();
        let store = DayStore::new(temp.path()).unwrap();
        let base = now_ms() - 100_000;
        store
            .append(Event::app_focus(base, "Code", "code", "a", 1, 0))
            .unwrap();
        store
            .append(Event::at(base + 10_000, "tick", json!({"i":0})))
            .unwrap();
        store
            .append(Event::at(base + 20_000, "lock", json!({})))
            .unwrap();
        store
            .append(Event::at(base + 50_000, "unlock", json!({})))
            .unwrap();
        let stats = store.derive(
            &store.read_day(&local_date_key(base)).unwrap(),
            DeriveOptions::default(),
        );
        assert_eq!(stats.total_active_ms, 20_000);
        assert_eq!(stats.stays.len(), 1);
    }

    #[test]
    fn jsonl_tolerates_a_torn_last_line() {
        let temp = tempfile::tempdir().unwrap();
        let store = DayStore::new(temp.path()).unwrap();
        let day = local_date_key(now_ms());
        store
            .append(Event::at(now_ms(), "lock", json!({})))
            .unwrap();
        OpenOptions::new()
            .append(true)
            .open(store.day_file(&day).unwrap())
            .unwrap()
            .write_all(br#"{"ts": bad"#)
            .unwrap();
        assert_eq!(store.read_day(&day).unwrap().len(), 1);
    }

    #[test]
    fn paths_cannot_escape_data_dir() {
        let temp = tempfile::tempdir().unwrap();
        let store = DayStore::new(temp.path()).unwrap();
        assert!(store.snapshot_path("snapshots/2026-01-01/a.jpg").is_some());
        assert!(store.snapshot_path("../settings.json").is_none());
        assert!(store.snapshot_path("C:/Windows/system.ini").is_none());
        assert!(store.day_file("../../bad").is_err());
    }

    fn ts_on(date: chrono::NaiveDate, hour: u32, minute: u32, second: u32) -> i64 {
        Local
            .from_local_datetime(&date.and_hms_opt(hour, minute, second).unwrap())
            .single()
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn work_across_midnight_keeps_its_app() {
        let temp = tempfile::tempdir().unwrap();
        let store = DayStore::new(temp.path()).unwrap();
        let today = Local::now().date_naive() - Duration::days(1);
        let yesterday = today - Duration::days(1);
        store
            .append(Event::app_focus(
                ts_on(yesterday, 23, 50, 0),
                "Code",
                "code",
                "a.rs",
                1,
                0,
            ))
            .unwrap();
        store
            .append(Event::at(
                ts_on(yesterday, 23, 59, 30),
                kinds::TICK,
                json!({"i": 0}),
            ))
            .unwrap();
        for minute in [0, 1, 2] {
            store
                .append(Event::at(
                    ts_on(today, 0, minute, 30),
                    kinds::TICK,
                    json!({"i": 0}),
                ))
                .unwrap();
        }
        let key = today.format("%Y-%m-%d").to_string();
        let stats = store.day_stats(&key, DeriveOptions::default()).unwrap();
        assert_eq!(stats.stays.len(), 1);
        assert_eq!(stats.stays[0].name, "code");
        assert!(stats.total_active_ms >= 2 * 60_000);
    }

    #[test]
    fn alt_tab_blips_are_merged_and_titles_get_time() {
        let date = Local::now().date_naive();
        let base = ts_on(date, 10, 0, 0);
        let events = vec![
            Event::app_focus(base, "Code", "code", "a.rs", 1, 0),
            Event::app_focus(base + 60_000, "Code", "code", "b.rs", 1, 0),
            Event::app_focus(base + 180_000, "Explorer", "explorer", "", 2, 0),
            Event::app_focus(base + 181_000, "Code", "code", "b.rs", 1, 0),
            Event::at(base + 300_000, kinds::LOCK, json!({})),
        ];
        let stats = derive_events(&events, DeriveOptions::default(), None);
        assert_eq!(stats.stays.len(), 1, "{:?}", stats.stays);
        assert_eq!(stats.switches, 0);
        let code = &stats.apps[0];
        assert_eq!(code.title_ms.get("a.rs"), Some(&60_000));
        assert!(code.title_ms.get("b.rs").copied().unwrap_or(0) >= 230_000);
        assert_eq!(code.top_titles(1)[0].0, "b.rs");
    }

    #[test]
    fn meetings_and_fullscreen_use_the_passive_threshold() {
        let date = Local::now().date_naive();
        let base = ts_on(date, 14, 0, 0);
        let events = vec![
            Event::app_focus(base, "Zoom", "zoom", "Zoom Meeting", 1, 0).with_passive(true),
            Event::at(base + 5 * 60_000, kinds::TICK, json!({"i": 5 * 60_000})).with_passive(true),
            Event::at(base + 10 * 60_000, kinds::TICK, json!({"i": 10 * 60_000}))
                .with_passive(true),
            Event::at(base + 10 * 60_000, kinds::LOCK, json!({})),
        ];
        let stats = derive_events(&events, DeriveOptions::default(), None);
        assert_eq!(stats.total_active_ms, 10 * 60_000);
        // Same input without the passive flag: idle after one minute.
        let plain = events
            .iter()
            .cloned()
            .map(|mut event| {
                if let Some(data) = event.data.as_object_mut() {
                    data.remove("p");
                }
                event
            })
            .collect::<Vec<_>>();
        assert!(derive_events(&plain, DeriveOptions::default(), None).total_active_ms < 2 * 60_000);
    }

    #[test]
    fn browser_time_is_split_by_site_category() {
        let date = Local::now().date_naive();
        let base = ts_on(date, 9, 0, 0);
        let visit = |ts: i64, site: &str| {
            Event::at(
                ts,
                kinds::SITE_VISIT,
                json!({"site": site, "app": "Chrome", "name": "chrome"}),
            )
        };
        let events = vec![
            Event::app_focus(base, "Chrome", "chrome", "GitHub", 1, 0),
            visit(base + 1_000, "github.com"),
            visit(base + 20 * 60_000, "bilibili.com"),
            Event::at(base + 30 * 60_000, kinds::TICK, json!({"i": 0})),
            Event::at(base + 30 * 60_000 + 1, kinds::LOCK, json!({})),
        ];
        let stats = derive_events(&events, DeriveOptions::default(), None);
        let category = |name: &str| {
            stats
                .categories
                .iter()
                .find(|category| category.name == name)
                .map(|category| category.ms)
                .unwrap_or(0)
        };
        assert!(category("开发") >= 19 * 60_000);
        assert!(category("娱乐") >= 9 * 60_000);
        assert_eq!(stats.stays[0].category(), "开发");
        assert_eq!(stats.sites[0].site, "github.com");
        assert!(stats.sites[0].ms >= 19 * 60_000);
        assert!(stats.hourly_categories[9].contains_key("开发"));
    }

    #[test]
    fn new_events_carry_a_schema_version() {
        let line = serde_json::to_string(&Event::at(1, kinds::LOCK, json!({}))).unwrap();
        assert!(line.contains("\"v\":2"));
        let legacy: Event = serde_json::from_str(r#"{"ts":1,"kind":"lock","data":{}}"#).unwrap();
        assert_eq!(legacy.v, 0);
    }
}
