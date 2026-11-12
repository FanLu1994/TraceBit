use crate::{
    analyzer::chat_completion,
    settings::{Settings, SettingsData},
    store::{DayStore, DeriveOptions, Snapshot, hhmm, local_date_key, now_ms, validate_date_key},
};
use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{Duration as ChronoDuration, Local, TimeZone};
use image::{DynamicImage, GenericImageView};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_SCREENSHOTS: usize = 6;
const MAX_IMAGE_EDGE: u32 = 1024;
const SUMMARY_SYSTEM_PROMPT: &str = r#"你是屏幕活动摘要助手。根据用户主动提供的一段时间内的屏幕截图，概括用户实际做了什么。

规则：
- 使用简体中文，输出 2-4 句、约 80-180 字的纯文本，不要标题或 Markdown。
- 按截图的时间顺序总结主要活动，只描述画面中能够确认的事实。
- 看不清或无法确认的内容不要猜测，可以说明“部分画面无法确认”。
- 不要复述密码、密钥、银行卡号、私人聊天原文等敏感内容；只需概括活动类型。
- 不评价效率，不提供建议，不补充截图之外的信息。"#;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IntervalSummary {
    pub start: i64,
    pub end: i64,
    pub created_at: i64,
    pub model: String,
    pub summary: String,
    pub screenshot_count: usize,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnalysisCursor {
    last_end: i64,
}

#[derive(Debug)]
pub struct IntervalAnalyzer {
    settings: Arc<Settings>,
    store: Arc<DayStore>,
    directory: PathBuf,
}

impl IntervalAnalyzer {
    pub fn new(settings: Arc<Settings>, store: Arc<DayStore>, directory: PathBuf) -> Result<Self> {
        fs::create_dir_all(&directory)?;
        Ok(Self {
            settings,
            store,
            directory,
        })
    }

    pub fn list(&self, date_key: &str) -> Vec<IntervalSummary> {
        let Ok(path) = self.summary_file(date_key) else {
            return Vec::new();
        };
        let mut summaries = fs::read_to_string(path)
            .ok()
            .into_iter()
            .flat_map(|raw| raw.lines().map(str::to_owned).collect::<Vec<_>>())
            .filter_map(|line| serde_json::from_str::<IntervalSummary>(&line).ok())
            .collect::<Vec<_>>();
        summaries.sort_by_key(|summary| summary.start);
        summaries
    }

    pub fn purge_all(&self) -> Result<()> {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory)?;
        }
        fs::create_dir_all(&self.directory)?;
        Ok(())
    }

    /// Analyze one completed interval. The first call establishes a cursor so enabling
    /// the feature never uploads a backlog of historical screenshots.
    pub fn analyze_due(&self, now: i64) -> Result<Option<IntervalSummary>> {
        let config = self.settings.get();
        if !config.ai.screenshot_analysis {
            return Ok(None);
        }
        let interval =
            (config.ai.screenshot_analysis_interval_ms as i64).clamp(300_000, 14_400_000);
        let previous = self.cursor()?.last_end;
        if previous <= 0 {
            self.save_cursor(now)?;
            return Ok(None);
        }
        if previous + interval > now {
            return Ok(None);
        }
        // After downtime or a long disabled period, only analyze the latest complete
        // interval instead of uploading every old screenshot.
        let end = if now - previous > interval * 2 {
            now
        } else {
            previous + interval
        };
        let start = end - interval;
        let snapshots = self.snapshots_between(start, end, &config)?;
        if snapshots.is_empty() {
            self.save_cursor(end)?;
            return Ok(None);
        }
        let summary = self.analyze_interval(start, end, select_snapshots(&snapshots))?;
        self.save_cursor(end)?;
        Ok(Some(summary))
    }

    fn analyze_interval(
        &self,
        start: i64,
        end: i64,
        snapshots: Vec<Snapshot>,
    ) -> Result<IntervalSummary> {
        if snapshots.is_empty() {
            bail!("该时段没有可分析的截图");
        }
        let ai = self.settings.get().ai;
        let timeline = snapshots
            .iter()
            .enumerate()
            .map(|(index, shot)| format!("第 {} 张：{}", index + 1, hhmm(shot.ts)))
            .collect::<Vec<_>>()
            .join("；");
        let prompt = format!(
            "请分析 {} 到 {} 这一时段的屏幕截图，总结用户做了什么。{}。",
            hhmm(start),
            hhmm(end),
            timeline
        );
        let mut content = vec![json!({"type":"text", "text":prompt})];
        let mut screenshot_count = 0;
        for shot in &snapshots {
            let Some(path) = self.store.snapshot_path(&shot.file) else {
                continue;
            };
            match snapshot_data_url(&path) {
                Ok(url) => {
                    content.push(json!({"type":"image_url", "image_url":{"url":url}}));
                    screenshot_count += 1;
                }
                Err(error) => eprintln!("[IntervalAnalysis] 跳过无法读取的截图：{error:#}"),
            }
        }
        if screenshot_count == 0 {
            bail!("该时段的截图均无法读取");
        }
        let (summary, model) = chat_completion(
            &ai,
            json!([
                {"role":"system", "content":SUMMARY_SYSTEM_PROMPT},
                {"role":"user", "content":Value::Array(content)}
            ]),
            400,
            120_000,
        )?;
        let summary = IntervalSummary {
            start,
            end,
            created_at: now_ms(),
            model,
            summary,
            screenshot_count,
        };
        self.append(&summary)?;
        Ok(summary)
    }

    fn snapshots_between(
        &self,
        start: i64,
        end: i64,
        config: &SettingsData,
    ) -> Result<Vec<Snapshot>> {
        let start_date = Local
            .timestamp_millis_opt(start)
            .single()
            .context("分析开始时间无效")?
            .date_naive();
        let end_date = Local
            .timestamp_millis_opt(end)
            .single()
            .context("分析结束时间无效")?
            .date_naive();
        let options = DeriveOptions {
            idle_threshold_ms: config.recording.idle_threshold_ms,
            session_gap_ms: config.recording.session_gap_ms,
        };
        let mut snapshots = Vec::new();
        let mut date = start_date;
        while date <= end_date {
            let stats = self
                .store
                .day_stats(&date.format("%Y-%m-%d").to_string(), options)?;
            snapshots.extend(
                stats
                    .screenshots
                    .into_iter()
                    .filter(|shot| shot.ts > start && shot.ts <= end),
            );
            date += ChronoDuration::days(1);
        }
        snapshots.sort_by_key(|shot| shot.ts);
        Ok(snapshots)
    }

    fn summary_file(&self, date_key: &str) -> Result<PathBuf> {
        validate_date_key(date_key)?;
        Ok(self.directory.join(format!("{date_key}.jsonl")))
    }

    fn append(&self, summary: &IntervalSummary) -> Result<()> {
        let path = self.summary_file(&local_date_key(summary.end))?;
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        serde_json::to_writer(&mut file, summary)?;
        file.write_all(b"\n")?;
        Ok(())
    }

    fn cursor_file(&self) -> PathBuf {
        self.directory.join("cursor.json")
    }

    fn cursor(&self) -> Result<AnalysisCursor> {
        match fs::read_to_string(self.cursor_file()) {
            Ok(raw) => Ok(serde_json::from_str(&raw).unwrap_or_default()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(AnalysisCursor::default())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn save_cursor(&self, last_end: i64) -> Result<()> {
        fs::write(
            self.cursor_file(),
            serde_json::to_vec_pretty(&AnalysisCursor { last_end })?,
        )?;
        Ok(())
    }
}

fn select_snapshots(snapshots: &[Snapshot]) -> Vec<Snapshot> {
    if snapshots.len() <= MAX_SCREENSHOTS {
        return snapshots.to_vec();
    }
    (0..MAX_SCREENSHOTS)
        .map(|index| {
            let numerator = index * (snapshots.len() - 1) + (MAX_SCREENSHOTS - 1) / 2;
            snapshots[numerator / (MAX_SCREENSHOTS - 1)].clone()
        })
        .collect()
}

fn snapshot_data_url(path: &Path) -> Result<String> {
    let image = image::open(path).with_context(|| format!("无法解码 {}", path.display()))?;
    let (width, height) = image.dimensions();
    let longest_edge = width.max(height);
    let image = if longest_edge > MAX_IMAGE_EDGE {
        let scale = MAX_IMAGE_EDGE as f64 / longest_edge as f64;
        image.resize(
            (width as f64 * scale).round().max(1.0) as u32,
            (height as f64 * scale).round().max(1.0) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image
    };
    encode_jpeg(&image)
}

fn encode_jpeg(image: &DynamicImage) -> Result<String> {
    let mut bytes = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 70);
    image.write_with_encoder(encoder)?;
    Ok(format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn selection_covers_the_whole_interval() {
        let snapshots = (0..10)
            .map(|index| Snapshot {
                ts: index,
                file: format!("{index}.jpg"),
                w: 1,
                h: 1,
                bytes: 1,
                trigger: "heartbeat".to_owned(),
                diff: None,
                app: None,
                title: None,
            })
            .collect::<Vec<_>>();
        let selected = select_snapshots(&snapshots);
        assert_eq!(selected.len(), MAX_SCREENSHOTS);
        assert_eq!(selected.first().unwrap().ts, 0);
        assert_eq!(selected.last().unwrap().ts, 9);
        assert!(selected.windows(2).all(|pair| pair[0].ts < pair[1].ts));
    }

    #[test]
    fn ai_images_are_downscaled_and_reencoded() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("large.jpg");
        let image =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(1600, 800, Rgba([20, 40, 60, 255])));
        image.save(&path).unwrap();
        let url = snapshot_data_url(&path).unwrap();
        let encoded = url.strip_prefix("data:image/jpeg;base64,").unwrap();
        let bytes = STANDARD.decode(encoded).unwrap();
        let resized = image::load_from_memory(&bytes).unwrap();
        assert_eq!(resized.dimensions(), (1024, 512));
    }

    #[test]
    fn interval_analysis_sends_images_and_persists_summary() {
        use std::{io::Read, net::TcpListener, sync::mpsc, thread, time::Duration};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut raw = Vec::new();
            let mut buffer = [0_u8; 8192];
            let expected = loop {
                let count = socket.read(&mut buffer).unwrap();
                if count == 0 {
                    break None;
                }
                raw.extend_from_slice(&buffer[..count]);
                if let Some(header_end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&raw[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if raw.len() >= header_end + 4 + length {
                        break Some(header_end + 4 + length);
                    }
                }
            };
            if let Some(expected) = expected {
                tx.send(String::from_utf8_lossy(&raw[..expected]).into_owned())
                    .unwrap();
            }
            let body = json!({
                "model":"vision-mock",
                "choices":[{"message":{"content":"先处理项目代码，随后查看文档并整理记录。"}}]
            })
            .to_string();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });

        let temp = tempfile::tempdir().unwrap();
        let settings = Arc::new(Settings::load(temp.path().join("user")));
        settings
            .patch(&json!({"ai":{
                "baseUrl":format!("http://{address}/v1"),
                "model":"vision-mock"
            }}))
            .unwrap();
        let store = Arc::new(DayStore::new(temp.path().join("data")).unwrap());
        let date = Local::now().date_naive();
        let start = Local
            .from_local_datetime(&date.and_hms_opt(10, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp_millis();
        let end = start + 30 * 60_000;
        let date_key = date.format("%Y-%m-%d").to_string();
        let directory = store.snapshot_dir(&date_key).unwrap();
        fs::create_dir_all(&directory).unwrap();
        DynamicImage::ImageRgba8(ImageBuffer::from_pixel(320, 200, Rgba([80, 100, 120, 255])))
            .save(directory.join("one.jpg"))
            .unwrap();
        let snapshot = Snapshot {
            ts: start + 60_000,
            file: format!("snapshots/{date_key}/one.jpg"),
            w: 320,
            h: 200,
            bytes: 1,
            trigger: "heartbeat".to_owned(),
            diff: None,
            app: None,
            title: None,
        };
        let analyzer =
            IntervalAnalyzer::new(settings, store, temp.path().join("intervals")).unwrap();
        let summary = analyzer
            .analyze_interval(start, end, vec![snapshot])
            .unwrap();
        let request = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(request.contains("image_url"));
        assert!(request.contains("data:image/jpeg;base64,"));
        assert!(request.contains("屏幕活动摘要助手"));
        assert_eq!(summary.screenshot_count, 1);
        assert_eq!(summary.model, "vision-mock");
        assert_eq!(analyzer.list(&date_key), vec![summary]);
    }
}
