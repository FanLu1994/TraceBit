use crate::{
    i18n::{Lang, fill},
    llm::{self, ChatOptions},
    recorder::SENSITIVE_TITLE,
    redact::{ai_title, redact_text},
    settings::{AiSettings, Endpoint, Settings},
    store::{DayStore, Snapshot, hhmm, local_date_key, now_ms, validate_date_key},
    tr,
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
/// Summary prompt body, in Chinese; the output language is appended per call so
/// stored summaries are written in the interface language.
const SUMMARY_PROMPT_BODY: &str = r#"你是屏幕活动摘要助手。根据一段时间内按时间顺序给出的屏幕截图（每张附带当时的前台应用和窗口标题），概括用户实际在做什么。

规则：
- 说清楚任务本身（例如“在调试某个模块的登录接口”“在整理季度报表”），而不只是应用名。
- 如果能看出卡点、等待、重复操作或频繁中断，简要指出，这对后续的效率建议最有价值。
- 只描述画面中能确认的事实；看不清的不要猜，可以说“部分画面无法确认”。
- 不要复述密码、密钥、银行卡号、私人聊天原文、人名等敏感内容；只概括活动类型。
- 不评价效率，不提供建议。"#;

fn summary_system_prompt(lang: Lang) -> String {
    let directive = fill(
        tr!(
            lang,
            "使用目标语言（{}）输出 2–4 句、约 80–180 字的纯文本，不要标题或 Markdown。",
            "Write 2–4 plain-text sentences of about 80–180 characters in the target language ({}). No headings or Markdown.",
            "対象言語（{}）で 2〜4 文、80〜180 文字程度のプレーンテキストで出力してください。見出しや Markdown は使わないでください。",
            "대상 언어({})로 2~4문장, 약 80~180자 분량의 일반 텍스트로 작성하세요. 제목이나 Markdown은 사용하지 마세요."
        ),
        &[&lang.name()],
    );
    format!("{SUMMARY_PROMPT_BODY}\n- {directive}")
}

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

/// What was sent in the latest screenshot request, kept for transparency.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SentRequest {
    pub at: i64,
    pub endpoint: String,
    pub model: String,
    pub prompt: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnalysisCursor {
    last_end: i64,
}

pub fn read_summaries(directory: &Path, date_key: &str) -> Vec<IntervalSummary> {
    if validate_date_key(date_key).is_err() {
        return Vec::new();
    }
    let mut summaries = fs::read_to_string(directory.join(format!("{date_key}.jsonl")))
        .ok()
        .into_iter()
        .flat_map(|raw| raw.lines().map(str::to_owned).collect::<Vec<_>>())
        .filter_map(|line| serde_json::from_str::<IntervalSummary>(&line).ok())
        .collect::<Vec<_>>();
    summaries.sort_by_key(|summary| summary.start);
    summaries
}

/// Screenshot analysis uses the same endpoint and model as the advice; the
/// configured model must accept image input (a multimodal model).
pub fn vision_endpoint(ai: &AiSettings, lang: Lang) -> Result<Endpoint> {
    let endpoint = ai.text_endpoint();
    if !endpoint.configured() {
        bail!(tr!(
            lang,
            "请先在设置中配置支持图片输入的多模态模型，例如 qwen2.5vl、gpt-4o-mini",
            "Configure a multimodal model that accepts image input in Settings first, such as qwen2.5vl or gpt-4o-mini.",
            "設定で画像入力に対応したマルチモーダルモデル（例：qwen2.5vl、gpt-4o-mini）を構成してください。",
            "설정에서 이미지 입력을 지원하는 멀티모달 모델(예: qwen2.5vl, gpt-4o-mini)을 먼저 구성하세요."
        ));
    }
    Ok(endpoint)
}

/// Sends a tiny generated image to verify the vision endpoint reads images.
pub fn test_vision(ai: &AiSettings, lang: Lang) -> Result<String> {
    let endpoint = vision_endpoint(ai, lang)?;
    let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        64,
        64,
        image::Rgba([200, 30, 30, 255]),
    ));
    let output = llm::chat(
        &endpoint,
        json!([{"role":"user", "content":[
            {"type":"text", "text":"这张图片主要是什么颜色？只回答颜色。"},
            {"type":"image_url", "image_url":{"url": encode_jpeg(&image)?}}
        ]}]),
        ChatOptions {
            max_tokens: 16,
            timeout_ms: 60_000,
            ..Default::default()
        },
        lang,
    )
    .with_context(|| {
        tr!(
            lang,
            "视觉模型测试失败（模型可能不支持图片输入）",
            "The vision model test failed (the model may not accept image input).",
            "ビジョンモデルのテストに失敗しました（モデルが画像入力に対応していない可能性があります）。",
            "비전 모델 테스트에 실패했습니다(모델이 이미지 입력을 지원하지 않을 수 있습니다)."
        )
    })?;
    Ok(output.model)
}

/// Clock-aligned window end at or before `now` (e.g. :00 and :30 for 30 minutes),
/// counted from local midnight.
fn aligned_end(now: i64, interval: i64) -> i64 {
    let midnight = Local
        .timestamp_millis_opt(now)
        .single()
        .and_then(|date| date.date_naive().and_hms_opt(0, 0, 0))
        .and_then(|date| Local.from_local_datetime(&date).earliest())
        .map(|date| date.timestamp_millis())
        .unwrap_or(now - now.rem_euclid(interval));
    midnight + (now - midnight) / interval * interval
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

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn list(&self, date_key: &str) -> Vec<IntervalSummary> {
        read_summaries(&self.directory, date_key)
    }

    pub fn last_request(&self) -> Option<SentRequest> {
        fs::read_to_string(self.directory.join("last_request.json"))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
    }

    pub fn purge_all(&self) -> Result<()> {
        if self.directory.exists() {
            fs::remove_dir_all(&self.directory)?;
        }
        fs::create_dir_all(&self.directory)?;
        Ok(())
    }

    /// Analyze one completed, clock-aligned interval. The first call establishes a
    /// cursor so enabling the feature never uploads a backlog of old screenshots;
    /// after downtime only the latest complete interval is analyzed.
    pub fn analyze_due(&self, now: i64) -> Result<Option<IntervalSummary>> {
        let config = self.settings.get();
        if !config.ai.screenshot_analysis {
            return Ok(None);
        }
        let interval =
            (config.ai.screenshot_analysis_interval_ms as i64).clamp(300_000, 14_400_000);
        let end = aligned_end(now, interval);
        let previous = self.cursor()?.last_end;
        if previous <= 0 {
            self.save_cursor(end)?;
            return Ok(None);
        }
        if end <= previous {
            return Ok(None);
        }
        let start = previous.max(end - interval);
        let snapshots = self.snapshots_between(start, end)?;
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
        let config = self.settings.get();
        let lang = config.lang();
        if snapshots.is_empty() {
            bail!(tr!(
                lang,
                "该时段没有可分析的截图",
                "There are no analyzable screenshots in this interval.",
                "この時間帯に分析できるスクリーンショットがありません。",
                "이 시간대에 분석할 스크린샷이 없습니다."
            ));
        }
        let ai = config.ai;
        let endpoint = vision_endpoint(&ai, lang)?;
        let previous = self
            .list(&local_date_key(start))
            .into_iter()
            .rev()
            .find(|summary| summary.end <= start && start - summary.end <= 4 * 3_600_000);
        let mut prompt = fill(
            tr!(
                lang,
                "请总结 {0} 到 {1} 这一时段用户在做什么。",
                "Summarize what the user was doing between {0} and {1}.",
                "{0} から {1} までの時間帯にユーザーが何をしていたかを要約してください。",
                "{0}부터 {1}까지 이 시간대에 사용자가 무엇을 했는지 요약하세요."
            ),
            &[&hhmm(start), &hhmm(end)],
        );
        if let Some(previous) = &previous {
            prompt.push_str(&fill(
                tr!(
                    lang,
                    "上一时段（{0}–{1}）的摘要供衔接参考：{2}",
                    "The previous interval summary ({0}–{1}), for continuity: {2}",
                    "前の時間帯（{0}–{1}）の要約（参考）：{2}",
                    "이전 구간({0}–{1}) 요약(참고): {2}"
                ),
                &[
                    &hhmm(previous.start),
                    &hhmm(previous.end),
                    &redact_text(&previous.summary),
                ],
            ));
        }
        let mut content = vec![json!({"type":"text", "text":prompt})];
        let mut files = Vec::new();
        for (index, shot) in snapshots.iter().enumerate() {
            if shot.title.as_deref() == Some(SENSITIVE_TITLE) {
                continue;
            }
            let Some(path) = self.store.snapshot_path(&shot.file) else {
                continue;
            };
            match snapshot_data_url(&path, lang) {
                Ok(url) => {
                    let app = shot.app.clone().unwrap_or_default();
                    let category = crate::categories::categorize("", &app, "");
                    let title = shot
                        .title
                        .as_deref()
                        .map(|title| ai_title(category, title))
                        .unwrap_or_default();
                    content.push(json!({"type":"text", "text": fill(
                        tr!(
                            lang,
                            "第 {0} 张，{1}，前台：{2} {3}",
                            "Screenshot {0}, {1}, foreground: {2} {3}",
                            "画像 {0}、{1}、前面：{2} {3}",
                            "스크린샷 {0}, {1}, 전경: {2} {3}"
                        ),
                        &[&(index + 1), &hhmm(shot.ts), &redact_text(&app), &title]
                    )}));
                    content.push(json!({"type":"image_url", "image_url":{"url":url}}));
                    files.push(shot.file.clone());
                }
                Err(error) => eprintln!("[IntervalAnalysis] 跳过无法读取的截图：{error:#}"),
            }
        }
        if files.is_empty() {
            bail!(tr!(
                lang,
                "该时段的截图均无法读取",
                "None of the screenshots in this interval could be read.",
                "この時間帯のスクリーンショットはすべて読み込めませんでした。",
                "이 시간대의 스크린샷을 모두 읽을 수 없습니다."
            ));
        }
        let text_parts = content
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        self.record_request(&SentRequest {
            at: now_ms(),
            endpoint: endpoint.base_url.clone(),
            model: endpoint.model.clone(),
            prompt: text_parts,
            files: files.clone(),
        });
        let output = llm::chat(
            &endpoint,
            json!([
                {"role":"system", "content":summary_system_prompt(lang)},
                {"role":"user", "content":Value::Array(content)}
            ]),
            ChatOptions {
                max_tokens: 600,
                timeout_ms: 120_000,
                ..Default::default()
            },
            lang,
        )?;
        let summary = IntervalSummary {
            start,
            end,
            created_at: now_ms(),
            model: output.model,
            summary: output.content,
            screenshot_count: files.len(),
        };
        self.append(&summary)?;
        Ok(summary)
    }

    fn record_request(&self, request: &SentRequest) {
        if let Ok(json) = serde_json::to_vec_pretty(request) {
            let _ = fs::write(self.directory.join("last_request.json"), json);
        }
    }

    fn snapshots_between(&self, start: i64, end: i64) -> Result<Vec<Snapshot>> {
        let lang = self.settings.get().lang();
        let start_date = Local
            .timestamp_millis_opt(start)
            .single()
            .with_context(|| {
                tr!(
                    lang,
                    "分析开始时间无效",
                    "The analysis start time is invalid.",
                    "分析の開始時刻が無効です。",
                    "분석 시작 시간이 유효하지 않습니다."
                )
            })?
            .date_naive();
        let end_date = Local
            .timestamp_millis_opt(end)
            .single()
            .with_context(|| {
                tr!(
                    lang,
                    "分析结束时间无效",
                    "The analysis end time is invalid.",
                    "分析の終了時刻が無効です。",
                    "분석 종료 시간이 유효하지 않습니다."
                )
            })?
            .date_naive();
        let mut snapshots = Vec::new();
        let mut date = start_date;
        while date <= end_date {
            snapshots.extend(
                self.store
                    .screenshots(&date.format("%Y-%m-%d").to_string())?
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
        // Filed under the day the interval started, so a summary of 23:30–00:00
        // belongs to the day it describes.
        let path = self.summary_file(&local_date_key(summary.start))?;
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

fn snapshot_data_url(path: &Path, lang: Lang) -> Result<String> {
    let image = image::open(path).with_context(|| {
        fill(
            tr!(
                lang,
                "无法解码 {}",
                "Cannot decode {}",
                "{} をデコードできません",
                "{}을(를) 디코딩할 수 없습니다"
            ),
            &[&path.display()],
        )
    })?;
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
    use crate::llm::mock;
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
        let url = snapshot_data_url(&path, Lang::Zh).unwrap();
        let encoded = url.strip_prefix("data:image/jpeg;base64,").unwrap();
        let bytes = STANDARD.decode(encoded).unwrap();
        let resized = image::load_from_memory(&bytes).unwrap();
        assert_eq!(resized.dimensions(), (1024, 512));
    }

    #[test]
    fn windows_align_to_the_clock() {
        let date = Local::now().date_naive();
        let ts = |h, m| {
            Local
                .from_local_datetime(&date.and_hms_opt(h, m, 0).unwrap())
                .single()
                .unwrap()
                .timestamp_millis()
        };
        assert_eq!(aligned_end(ts(10, 47), 30 * 60_000), ts(10, 30));
        assert_eq!(aligned_end(ts(10, 30), 30 * 60_000), ts(10, 30));
    }

    #[test]
    fn screenshot_analysis_uses_the_main_multimodal_model() {
        let mut ai = AiSettings::default();
        assert!(vision_endpoint(&ai, Lang::Zh).is_err());
        ai.base_url = "https://api.openai.com/v1".to_owned();
        ai.model = "gpt-4o-mini".to_owned();
        let endpoint = vision_endpoint(&ai, Lang::Zh).unwrap();
        assert_eq!(endpoint.model, "gpt-4o-mini");
        assert_eq!(endpoint.base_url, "https://api.openai.com/v1");
    }

    #[test]
    fn interval_analysis_sends_images_with_context_and_persists_summary() {
        let (url, rx) = mock::serve(vec![mock::completion(
            "vision-mock",
            "先处理项目代码，随后查看文档并整理记录。",
        )]);
        let temp = tempfile::tempdir().unwrap();
        let settings = Arc::new(Settings::load(temp.path().join("user")));
        settings
            .patch(&json!({"ai":{"baseUrl":url, "apiKey":"vision-key", "model":"vision-mock"}}))
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
            app: Some("微信".to_owned()),
            title: Some("张三".to_owned()),
        };
        let analyzer =
            IntervalAnalyzer::new(settings, store, temp.path().join("intervals")).unwrap();
        let summary = analyzer
            .analyze_interval(start, end, vec![snapshot])
            .unwrap();
        let request = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert!(request.contains("image_url"));
        assert!(request.contains("data:image/jpeg;base64,"));
        assert!(request.contains("屏幕活动摘要助手"));
        assert!(request.contains("vision-mock"));
        assert!(request.contains("Bearer vision-key"));
        assert!(!request.contains("张三"));
        assert_eq!(summary.screenshot_count, 1);
        assert_eq!(summary.model, "vision-mock");
        assert_eq!(analyzer.list(&date_key), vec![summary]);
        assert_eq!(analyzer.last_request().unwrap().files.len(), 1);
    }
}
