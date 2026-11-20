//! OpenAI-compatible chat client shared by the advice report and the
//! screenshot summaries.

use crate::{i18n::Lang, settings::Endpoint};
use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use reqwest::{StatusCode, blocking::Client};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::LazyLock, thread, time::Duration};

#[derive(Debug, Clone, Copy)]
pub struct ChatOptions {
    pub max_tokens: u32,
    pub timeout_ms: u64,
    pub temperature: f32,
    /// Ask for `response_format: json_object`; silently dropped when the
    /// provider rejects it.
    pub json: bool,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            max_tokens: 2048,
            timeout_ms: 120_000,
            temperature: 0.3,
            json: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct ChatOutput {
    pub content: String,
    pub model: String,
    pub usage: Usage,
    /// The provider stopped because of `max_tokens`.
    pub truncated: bool,
}

/// Uses the configured URL as given. Only a bare host gets `/v1`, because that
/// is what every OpenAI-compatible server (including Ollama) expects; any path
/// the user typed (`/v1beta/openai`, `/api/paas/v4`, ...) is respected.
pub fn completions_url(base_url: &str, lang: Lang) -> Result<String> {
    use crate::tr;
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        bail!(tr!(
            lang,
            "未配置 AI 接口地址",
            "The AI endpoint URL is not configured",
            "AI エンドポイントの URL が未設定です",
            "AI 엔드포인트 주소가 설정되지 않았습니다"
        ));
    }
    if base.ends_with("/chat/completions") {
        return Ok(base.to_owned());
    }
    let url = url::Url::parse(base).context(tr!(
        lang,
        "AI 接口地址无效",
        "Invalid AI endpoint URL",
        "AI エンドポイントの URL が無効です",
        "AI 엔드포인트 주소가 올바르지 않습니다"
    ))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!(tr!(
            lang,
            "AI 接口地址必须以 http:// 或 https:// 开头",
            "The AI endpoint URL must start with http:// or https://",
            "AI エンドポイントの URL は http:// または https:// で始まる必要があります",
            "AI 엔드포인트 주소는 http:// 또는 https://로 시작해야 합니다"
        ));
    }
    if url.path().trim_matches('/').is_empty() {
        Ok(format!("{base}/v1/chat/completions"))
    } else {
        Ok(format!("{base}/chat/completions"))
    }
}

/// Reasoning models may inline their chain of thought.
pub fn strip_reasoning(content: &str) -> String {
    static THINK: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<think>.*?</think>").unwrap());
    THINK.replace_all(content, "").trim().to_owned()
}

fn describe_status(status: StatusCode, body: &str, lang: Lang) -> String {
    use crate::tr;
    let detail = body.chars().take(200).collect::<String>();
    let hint = match status.as_u16() {
        401 | 403 => tr!(
            lang,
            "API Key 无效或没有该模型的权限",
            "The API key is invalid or has no access to this model",
            "API キーが無効か、このモデルへの権限がありません",
            "API 키가 유효하지 않거나 이 모델에 대한 권한이 없습니다"
        ),
        404 => tr!(
            lang,
            "接口地址或模型名不存在",
            "The endpoint path or model name does not exist",
            "エンドポイントまたはモデル名が存在しません",
            "엔드포인트 또는 모델 이름이 존재하지 않습니다"
        ),
        413 => tr!(
            lang,
            "请求内容过大",
            "The request is too large",
            "リクエストが大きすぎます",
            "요청 내용이 너무 큽니다"
        ),
        429 => tr!(
            lang,
            "请求过于频繁或额度不足",
            "Too many requests, or the quota is exhausted",
            "リクエストが多すぎるか、利用枠が不足しています",
            "요청이 너무 많거나 할당량이 부족합니다"
        ),
        500..=599 => tr!(
            lang,
            "服务端暂时不可用",
            "The service is temporarily unavailable",
            "サーバーが一時的に利用できません",
            "서버를 일시적으로 사용할 수 없습니다"
        ),
        _ => tr!(
            lang,
            "请求被拒绝",
            "The request was rejected",
            "リクエストが拒否されました",
            "요청이 거부되었습니다"
        ),
    };
    crate::i18n::fill(
        tr!(
            lang,
            "AI 接口返回 {0}（{1}）：{2}",
            "AI endpoint returned {0} ({1}): {2}",
            "AI エンドポイントが {0} を返しました（{1}）：{2}",
            "AI 엔드포인트가 {0}를 반환했습니다 ({1}): {2}"
        ),
        &[&status, &hint, &detail],
    )
}

pub fn chat(
    endpoint: &Endpoint,
    messages: Value,
    options: ChatOptions,
    lang: Lang,
) -> Result<ChatOutput> {
    use crate::tr;
    let url = completions_url(&endpoint.base_url, lang)?;
    if endpoint.model.trim().is_empty() {
        bail!(tr!(
            lang,
            "未配置模型名称",
            "The model name is not configured",
            "モデル名が設定されていません",
            "모델 이름이 설정되지 않았습니다"
        ));
    }
    let client = Client::builder()
        .timeout(Duration::from_millis(options.timeout_ms))
        .build()?;
    let mut json_mode = options.json;
    let mut attempt: u64 = 0;
    let response: Value = loop {
        attempt += 1;
        let mut body = json!({
            "model": endpoint.model,
            "messages": messages,
            "temperature": options.temperature,
            "max_tokens": options.max_tokens,
        });
        if json_mode {
            body["response_format"] = json!({"type": "json_object"});
        }
        let mut request = client.post(&url).json(&body);
        if !endpoint.api_key.is_empty() {
            request = request.bearer_auth(&endpoint.api_key);
        }
        let retry_delay = Duration::from_millis((if cfg!(test) { 10 } else { 2_000 }) * attempt);
        let response = match request.send() {
            Ok(response) => response,
            Err(error) if attempt < 3 && (error.is_connect() || error.is_timeout()) => {
                thread::sleep(retry_delay);
                continue;
            }
            Err(error) => {
                return Err(anyhow!(error).context(tr!(
                    lang,
                    "无法连接 AI 接口",
                    "Cannot reach the AI endpoint",
                    "AI エンドポイントに接続できません",
                    "AI 엔드포인트에 연결할 수 없습니다"
                )));
            }
        };
        let status = response.status();
        if status.is_success() {
            break response.json().context(tr!(
                lang,
                "AI 返回格式无效",
                "The AI response format is invalid",
                "AI の応答形式が不正です",
                "AI 응답 형식이 올바르지 않습니다"
            ))?;
        }
        let text = response.text().unwrap_or_default();
        if status == StatusCode::BAD_REQUEST && json_mode {
            // Not every compatible server supports response_format.
            json_mode = false;
            continue;
        }
        if attempt < 3 && (status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()) {
            thread::sleep(retry_delay);
            continue;
        }
        bail!(describe_status(status, &text, lang));
    };
    let choice = response.pointer("/choices/0").cloned().unwrap_or_default();
    let raw = choice
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let content = strip_reasoning(raw);
    let truncated = choice.get("finish_reason").and_then(Value::as_str) == Some("length");
    if content.is_empty() {
        if truncated {
            bail!(tr!(
                lang,
                "AI 输出在思考阶段就达到长度上限，请换用非推理模型或更大的输出额度",
                "The model hit its length limit while still reasoning; use a non-reasoning model or raise the output budget",
                "推論の段階で出力の上限に達しました。非推論モデルに替えるか、出力枠を増やしてください",
                "추론 단계에서 출력 한도에 도달했습니다. 비추론 모델로 바꾸거나 출력량을 늘리세요"
            ));
        }
        bail!(tr!(
            lang,
            "AI 返回内容为空",
            "The AI returned an empty response",
            "AI の応答が空でした",
            "AI 응답이 비어 있습니다"
        ));
    }
    let usage = Usage {
        prompt_tokens: response
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        completion_tokens: response
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    };
    Ok(ChatOutput {
        content,
        model: response
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or(&endpoint.model)
            .to_owned(),
        usage,
        truncated,
    })
}

/// Extracts the first JSON object from a reply that may be wrapped in a code
/// fence or surrounded by prose.
pub fn extract_json_object(content: &str) -> Option<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(content.trim())
        && value.is_object()
    {
        return Some(value);
    }
    let start = content.find('{')?;
    let end = content.rfind('}')?;
    (end > start)
        .then(|| serde_json::from_str::<Value>(&content[start..=end]).ok())
        .flatten()
        .filter(Value::is_object)
}

#[cfg(test)]
pub mod mock {
    //! Minimal HTTP server returning canned chat completions.
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    /// Serves one response per body; returns the base URL and received requests.
    pub fn serve(bodies: Vec<String>) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for body in bodies {
                let Ok((mut socket, _)) = listener.accept() else {
                    return;
                };
                let mut raw = Vec::new();
                let mut buffer = [0_u8; 16384];
                let expected = loop {
                    let count = socket.read(&mut buffer).unwrap_or(0);
                    if count == 0 {
                        break None;
                    }
                    raw.extend_from_slice(&buffer[..count]);
                    if let Some(header_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
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
                    let _ = tx.send(String::from_utf8_lossy(&raw[..expected]).into_owned());
                }
                let _ = write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            }
        });
        (format!("http://{address}/v1"), rx)
    }

    pub fn completion(model: &str, content: &str) -> String {
        serde_json::json!({
            "model": model,
            "choices": [{"message": {"content": content}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 50}
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_respect_user_paths() {
        assert_eq!(
            completions_url("http://localhost:11434", Lang::Zh).unwrap(),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            completions_url("https://api.deepseek.com/v1/", Lang::Zh).unwrap(),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            completions_url(
                "https://generativelanguage.googleapis.com/v1beta/openai",
                Lang::Zh
            )
            .unwrap(),
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        );
        assert_eq!(
            completions_url("https://open.bigmodel.cn/api/paas/v4", Lang::Zh).unwrap(),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        assert!(completions_url("", Lang::Zh).is_err());
    }

    #[test]
    fn reasoning_and_fences_are_removed() {
        assert_eq!(strip_reasoning("<think>a\nb</think>\n答案"), "答案");
        let value = extract_json_object("```json\n{\"a\":1}\n```").unwrap();
        assert_eq!(value["a"], 1);
        assert!(extract_json_object("no json").is_none());
    }

    #[test]
    fn truncation_is_reported() {
        let body = json!({
            "choices": [{"message": {"content": "{\"a\":"}, "finish_reason": "length"}]
        })
        .to_string();
        let (url, _rx) = mock::serve(vec![body]);
        let endpoint = Endpoint {
            base_url: url,
            api_key: String::new(),
            model: "m".to_owned(),
        };
        let output = chat(&endpoint, json!([]), ChatOptions::default(), Lang::Zh).unwrap();
        assert!(output.truncated);
    }
}
