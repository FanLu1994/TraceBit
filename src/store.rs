use crate::categories::categorize;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub ts: i64,
    pub kind: String,
    #[serde(default)]
    pub data: Value,
}

impl Event {
    pub fn new(kind: impl Into<String>, data: Value) -> Self {
        Self {
            ts: now_ms(),
            kind: kind.into(),
            data,
        }
    }

    pub fn at(ts: i64, kind: impl Into<String>, data: Value) -> Self {
        Self {
            ts,
            kind: kind.into(),
            data,
        }
    }

    pub fn app_focus(ts: i64, app: &str, name: &str, title: &str, pid: u32, idle_ms: i64) -> Self {
        Self::at(
            ts,
            "app_focus",
            json!({"app": app, "name": name, "title": title, "pid": pid, "i": idle_ms}),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stay {
    pub start: i64,
    pub end: i64,
    pub app: String,
    pub name: String,
    pub title: String,
    pub titles: BTreeSet<String>,
    pub pid: u32,
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
        }
    }

    pub fn category(&self) -> &'static str {
        categorize(&self.name, &self.app, &self.title)
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
    pub titles: BTreeMap<String, u32>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusBlock {
    pub app: String,
    pub ms: i64,
    pub started_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayStats {
    pub date_key: String,
    pub events: usize,
    pub stays: Vec<Stay>,
    pub sessions: Vec<Session>,
    pub apps: Vec<AppStat>,
    pub hourly: Vec<i64>,
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

#[derive(Debug, Clone, Copy)]
pub struct DeriveOptions {
    pub idle_threshold_ms: i64,
    pub session_gap_ms: i64,
}

impl Default for DeriveOptions {
    fn default() -> Self {
        Self {
            idle_threshold_ms: 60_000,
            session_gap_ms: 600_000,
        }
    }
}

#[derive(Debug)]
struct CacheEntry {
    fingerprint: (u64, u128),
    events: Vec<Event>,
}

#[derive(Debug)]
pub struct DayStore {
    pub data_dir: PathBuf,
    pub days_dir: PathBuf,
    pub snapshots_dir: PathBuf,
    cache: Mutex<HashMap<String, CacheEntry>>,
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
        let idle_threshold_ms = options.idle_threshold_ms.max(1);
        let session_gap_ms = options.session_gap_ms.max(1);
        let mut stays = Vec::new();
        let mut current: Option<Stay> = None;
        let mut last_focus: Option<Value> = None;
        let mut last_event_ts = None;
        let mut switches = 0;
        let mut first_active = None;
        let mut last_active = None;

        for event in events {
            let ts = event.ts;
            last_event_ts = Some(ts);
            let idle_ms = event.data.get("i").and_then(Value::as_i64);
            let is_idle_now = idle_ms.is_some_and(|idle| idle >= 0 && idle >= idle_threshold_ms);
            match event.kind.as_str() {
                "app_focus" => {
                    last_focus = Some(event.data.clone());
                    if is_idle_now {
                        close_stay(&mut current, &mut stays, ts - idle_ms.unwrap_or(0).max(0));
                        continue;
                    }
                    first_active.get_or_insert(ts);
                    last_active = Some(ts);
                    let name = string_field(&event.data, "name");
                    let app = string_field(&event.data, "app");
                    let same_app = current
                        .as_ref()
                        .is_some_and(|stay| stay.name == name && stay.app == app);
                    if current.is_some() && !same_app {
                        close_stay(&mut current, &mut stays, ts);
                        switches += 1;
                    }
                    if current.is_none() {
                        current = Some(Stay::from_focus(ts, &event.data));
                    } else if let Some(stay) = current.as_mut() {
                        stay.end = ts;
                        let title = string_field(&event.data, "title");
                        if !title.is_empty() {
                            stay.title = title;
                        }
                    }
                    if let Some(stay) = current.as_mut() {
                        let title = string_field(&event.data, "title");
                        if !title.is_empty() {
                            stay.titles.insert(title);
                            if stay.titles.len() > 60 {
                                let first = stay.titles.iter().next().cloned();
                                if let Some(first) = first {
                                    stay.titles.remove(&first);
                                }
                            }
                        }
                    }
                }
                "tick" => {
                    if is_idle_now {
                        close_stay(&mut current, &mut stays, ts - idle_ms.unwrap_or(0).max(0));
                    } else {
                        first_active.get_or_insert(ts);
                        last_active = Some(ts);
                        if current.is_none()
                            && let Some(focus) = &last_focus
                        {
                            current =
                                Some(Stay::from_focus(ts - idle_ms.unwrap_or(0).max(0), focus));
                        }
                        if let Some(stay) = current.as_mut() {
                            stay.end = ts;
                        }
                    }
                }
                "lock" | "suspend" => close_stay(&mut current, &mut stays, ts),
                "unlock" | "resume" => {}
                _ => {}
            }
        }
        if let Some(ts) = last_event_ts {
            close_stay(&mut current, &mut stays, ts);
        }
        let stays = stays
            .into_iter()
            .filter(|stay| stay.end > stay.start)
            .collect::<Vec<_>>();

        let mut sessions: Vec<Session> = Vec::new();
        for stay in &stays {
            if let Some(session) = sessions.last_mut()
                && stay.start - session.end < session_gap_ms
            {
                session.end = stay.end;
                session.apps.insert(app_key(stay));
                session.active_ms += stay.end - stay.start;
                continue;
            }
            sessions.push(Session {
                start: stay.start,
                end: stay.end,
                active_ms: stay.end - stay.start,
                apps: BTreeSet::from([app_key(stay)]),
            });
        }

        let mut apps: BTreeMap<String, AppStat> = BTreeMap::new();
        for stay in &stays {
            let key = app_key(stay);
            let app = apps.entry(key.clone()).or_insert_with(|| AppStat {
                name: key,
                app: stay.app.clone(),
                category: stay.category().to_owned(),
                ms: 0,
                switches: 0,
                titles: BTreeMap::new(),
            });
            app.ms += stay.end - stay.start;
            for title in &stay.titles {
                *app.titles.entry(title.clone()).or_insert(0) += 1;
            }
        }
        for pair in stays.windows(2) {
            if app_key(&pair[0]) != app_key(&pair[1])
                && let Some(app) = apps.get_mut(&app_key(&pair[1]))
            {
                app.switches += 1;
            }
        }

        let mut hourly = vec![0_i64; 24];
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
                hourly[date.hour() as usize] += (segment_end - cursor).max(0);
                if segment_end <= cursor {
                    break;
                }
                cursor = segment_end;
            }
        }

        DayStats {
            date_key: String::new(),
            events: events.len(),
            total_active_ms: stays.iter().map(|stay| stay.end - stay.start).sum(),
            stays,
            sessions,
            apps: apps.into_values().collect(),
            hourly,
            switches,
            first_active,
            last_active,
            screenshots: Vec::new(),
            sites: Vec::new(),
            lock_events: 0,
            long_idles: 0,
            longest_idle_ms: 0,
            longest_block: None,
            snapshot_bytes: 0,
        }
    }

    pub fn day_stats(&self, date_key: &str, options: DeriveOptions) -> Result<DayStats> {
        let events = self.read_day(date_key)?;
        let mut stats = self.derive(&events, options);
        stats.date_key = date_key.to_owned();
        stats.screenshots = events
            .iter()
            .filter(|event| event.kind == "screenshot")
            .filter(|event| {
                string_field(&event.data, "file")
                    .pipe(|file| self.snapshot_path(&file))
                    .is_some_and(|path| path.exists())
            })
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
            .collect();

        let mut sites: BTreeMap<String, SiteStat> = BTreeMap::new();
        for event in events.iter().filter(|event| event.kind == "site_visit") {
            let site = string_field(&event.data, "site").to_lowercase();
            if site.is_empty()
                || site.len() > 253
                || site
                    .chars()
                    .any(|ch| ch.is_whitespace() || matches!(ch, '/' | '?' | '#'))
            {
                continue;
            }
            let item = sites.entry(site.clone()).or_insert_with(|| SiteStat {
                site,
                visits: 0,
                first_seen: event.ts,
                last_seen: event.ts,
                app: string_field(&event.data, "app"),
            });
            item.visits += 1;
            item.last_seen = event.ts;
            let app = string_field(&event.data, "app");
            if !app.is_empty() {
                item.app = app;
            }
        }
        stats.sites = {
            let mut sites = sites.into_values().collect::<Vec<_>>();
            sites.sort_by_key(|site| std::cmp::Reverse(site.last_seen));
            sites
        };
        stats.lock_events = events
            .iter()
            .filter(|event| matches!(event.kind.as_str(), "lock" | "suspend"))
            .count();
        for pair in stats.sessions.windows(2) {
            let gap = pair[1].start - pair[0].end;
            if gap >= 5 * 60_000 {
                stats.long_idles += 1;
            }
            stats.longest_idle_ms = stats.longest_idle_ms.max(gap);
        }

        let mut current: Option<FocusBlock> = None;
        let mut current_end = 0;
        for stay in &stats.stays {
            let key = app_key(stay);
            if current
                .as_ref()
                .is_some_and(|block| block.app == key && stay.start - current_end <= 60_000)
            {
                current_end = stay.end;
                if let Some(block) = current.as_mut() {
                    block.ms = current_end - block.started_at;
                }
            } else {
                replace_longest(&mut stats.longest_block, current.take());
                current = Some(FocusBlock {
                    app: key,
                    ms: stay.end - stay.start,
                    started_at: stay.start,
                });
                current_end = stay.end;
            }
        }
        replace_longest(&mut stats.longest_block, current);
        stats.snapshot_bytes = self.day_snapshot_bytes(date_key);
        Ok(stats)
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

fn app_key(stay: &Stay) -> String {
    if stay.name.is_empty() {
        stay.app.clone()
    } else {
        stay.name.clone()
    }
}

fn close_stay(current: &mut Option<Stay>, stays: &mut Vec<Stay>, end: i64) {
    if let Some(mut stay) = current.take() {
        stay.end = stay.start.max(end);
        if stay.end - stay.start < 1_000 {
            stay.end = stay.start + 1_000;
        }
        stays.push(stay);
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

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

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
        let snapshot_dir = store.snapshots_dir.join("x");
        fs::create_dir_all(&snapshot_dir).unwrap();
        fs::write(snapshot_dir.join("y.jpg"), b"test").unwrap();
        store
            .append(Event::at(
                base + 150_000,
                "screenshot",
                json!({"file":"snapshots/x/y.jpg", "w":1600, "h":900, "bytes":100, "trigger":"test", "diff":null}),
            ))
            .unwrap();
        let stats = store
            .day_stats(&local_date_key(base), DeriveOptions::default())
            .unwrap();
        assert_eq!(stats.total_active_ms, 176_000);
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
}
