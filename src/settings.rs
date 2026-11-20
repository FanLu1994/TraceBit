use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::RwLock,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct SettingsData {
    pub recording: RecordingSettings,
    pub privacy: PrivacySettings,
    pub storage: StorageSettings,
    pub ai: AiSettings,
    pub app: AppSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct RecordingSettings {
    pub enabled: bool,
    pub screenshots: bool,
    pub browser_sites: bool,
    pub poll_ms: u64,
    pub tick_ms: u64,
    pub title_throttle_ms: u64,
    pub focus_debounce_ms: u64,
    pub heartbeat_ms: u64,
    pub idle_threshold_ms: i64,
    /// Idle threshold while the user is plausibly watching or listening without
    /// touching input: meetings and full-screen windows (video, slides).
    pub passive_idle_threshold_ms: i64,
    pub session_gap_ms: i64,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            screenshots: true,
            browser_sites: false,
            poll_ms: 2_000,
            tick_ms: 30_000,
            title_throttle_ms: 8_000,
            focus_debounce_ms: 2_000,
            heartbeat_ms: 300_000,
            idle_threshold_ms: 60_000,
            passive_idle_threshold_ms: 900_000,
            session_gap_ms: 600_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PrivacySettings {
    pub sensitive_patterns: Vec<String>,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            sensitive_patterns: [
                "1password",
                "bitwarden",
                "lastpass",
                "dashlane",
                "keepass",
                "密码",
                "password",
                "网银",
                "银行",
                "bank",
                "支付宝",
                "alipay",
                "钱包",
                "wallet",
                "metamask",
                "inprivate",
                "incognito",
                "隐私",
                "私密",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct StorageSettings {
    pub max_snapshot_bytes: u64,
    pub retention_days: u32,
    pub jpeg_quality: u8,
    pub max_snap_width: u32,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            max_snapshot_bytes: 2 * 1024 * 1024 * 1024,
            retention_days: 180,
            jpeg_quality: 72,
            max_snap_width: 1600,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AiSettings {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub auto_report: bool,
    pub auto_report_time: String,
    /// Send yesterday's advice as a system notification once a day.
    pub daily_digest: bool,
    pub daily_digest_time: String,
    pub screenshot_analysis: bool,
    pub screenshot_analysis_interval_ms: u64,
    pub web_search: bool,
    pub search_provider: String,
    pub search_api_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify_sources: Option<bool>,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            auto_report: false,
            auto_report_time: "21:30".to_owned(),
            daily_digest: true,
            daily_digest_time: "10:30".to_owned(),
            screenshot_analysis: false,
            screenshot_analysis_interval_ms: 30 * 60_000,
            web_search: false,
            search_provider: "duckduckgo".to_owned(),
            search_api_key: String::new(),
            search_endpoint: None,
            verify_sources: None,
        }
    }
}

/// One OpenAI-compatible endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl Endpoint {
    pub fn configured(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.model.trim().is_empty()
    }
}

impl AiSettings {
    pub fn text_endpoint(&self) -> Endpoint {
        Endpoint {
            base_url: self.base_url.trim().to_owned(),
            api_key: self.api_key.trim().to_owned(),
            model: self.model.trim().to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub autostart: bool,
    pub close_to_tray: bool,
    /// `zh` / `en` / `ja` / `ko`; empty follows the Windows display language.
    pub language: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            autostart: false,
            close_to_tray: true,
            language: String::new(),
        }
    }
}

impl AppSettings {
    pub fn lang(&self) -> crate::i18n::Lang {
        crate::i18n::Lang::resolve(&self.language)
    }
}

impl SettingsData {
    pub fn lang(&self) -> crate::i18n::Lang {
        self.app.lang()
    }
}

#[derive(Debug)]
pub struct Settings {
    file: PathBuf,
    data: RwLock<SettingsData>,
}

impl Settings {
    pub fn load(user_data_path: impl AsRef<Path>) -> Self {
        let file = user_data_path.as_ref().join("settings.json");
        let data = fs::read_to_string(&file)
            .ok()
            .and_then(|raw| {
                serde_json::from_str::<Value>(&raw)
                    .and_then(|mut value| {
                        unprotect_secrets(&mut value);
                        serde_json::from_value(value)
                    })
                    .map_err(|err| {
                        eprintln!("[Settings] 读取失败，使用默认设置：{err}");
                        err
                    })
                    .ok()
            })
            .unwrap_or_default();
        Self {
            file,
            data: RwLock::new(data),
        }
    }

    pub fn get(&self) -> SettingsData {
        self.data.read().expect("settings lock").clone()
    }

    pub fn replace(&self, next: SettingsData) -> Result<SettingsData> {
        self.save(&next)?;
        *self.data.write().expect("settings lock") = next.clone();
        Ok(next)
    }

    /// Apply a partial camelCase settings object, matching the old IPC contract.
    pub fn patch(&self, partial: &Value) -> Result<SettingsData> {
        let current = self.get();
        let mut value = serde_json::to_value(&current)?;
        merge_value(&mut value, partial);
        let next: SettingsData = serde_json::from_value(value).context("设置格式无效")?;
        self.replace(next)
    }

    fn save(&self, data: &SettingsData) -> Result<()> {
        if let Some(parent) = self.file.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut value = serde_json::to_value(data)?;
        protect_secrets(&mut value)?;
        let json = serde_json::to_vec_pretty(&value)?;
        fs::write(&self.file, json).with_context(|| format!("无法保存 {}", self.file.display()))
    }
}

/// API keys are stored encrypted with Windows DPAPI (bound to the current user),
/// never as plaintext in settings.json.
const SECRET_FIELDS: [&str; 2] = ["apiKey", "searchApiKey"];

fn protect_secrets(value: &mut Value) -> Result<()> {
    let Some(ai) = value.get_mut("ai").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    for field in SECRET_FIELDS {
        let plain = ai
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        ai.insert(field.to_owned(), Value::String(String::new()));
        ai.remove(&format!("{field}Protected"));
        if !plain.is_empty() {
            ai.insert(
                format!("{field}Protected"),
                Value::String(crate::secret::protect(&plain)?),
            );
        }
    }
    Ok(())
}

fn unprotect_secrets(value: &mut Value) {
    let Some(ai) = value.get_mut("ai").and_then(Value::as_object_mut) else {
        return;
    };
    for field in SECRET_FIELDS {
        let Some(sealed) = ai
            .remove(&format!("{field}Protected"))
            .and_then(|value| value.as_str().map(str::to_owned))
        else {
            continue;
        };
        match crate::secret::unprotect(&sealed) {
            Ok(plain) => {
                ai.insert(field.to_owned(), Value::String(plain));
            }
            Err(error) => eprintln!("[Settings] 无法解密 {field}：{error:#}"),
        }
    }
}

fn merge_value(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base), Value::Object(patch)) => {
            for (key, value) in patch {
                if value.is_null() {
                    continue;
                }
                match base.get_mut(key) {
                    Some(base_value) => merge_value(base_value, value),
                    None => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_preserve_privacy_opt_ins() {
        let settings = SettingsData::default();
        assert!(settings.recording.enabled);
        assert!(settings.recording.screenshots);
        assert!(!settings.recording.browser_sites);
        assert!(!settings.ai.web_search);
        assert!(!settings.ai.auto_report);
        assert!(settings.ai.daily_digest);
        assert_eq!(settings.ai.daily_digest_time, "10:30");
        assert!(!settings.ai.screenshot_analysis);
        assert_eq!(settings.ai.screenshot_analysis_interval_ms, 30 * 60_000);
        assert_eq!(settings.storage.max_snapshot_bytes, 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn partial_json_uses_old_camel_case_contract() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Settings::load(temp.path());
        let next = settings
            .patch(&json!({
                "recording": {"browserSites": true, "heartbeatMs": 120000},
                "ai": {"model": "deepseek-chat"}
            }))
            .unwrap();
        assert!(next.recording.browser_sites);
        assert_eq!(next.recording.heartbeat_ms, 120_000);
        assert_eq!(next.recording.poll_ms, 2_000);
        assert_eq!(next.ai.model, "deepseek-chat");
        assert!(next.recording.screenshots);

        let loaded = Settings::load(temp.path()).get();
        assert_eq!(loaded, next);
    }

    #[test]
    fn language_round_trips_and_old_files_follow_the_system() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Settings::load(temp.path());
        assert_eq!(settings.get().app.language, "");
        settings.patch(&json!({"app": {"language": "ja"}})).unwrap();
        assert_eq!(
            Settings::load(temp.path()).get().lang(),
            crate::i18n::Lang::Ja
        );

        fs::write(
            temp.path().join("settings.json"),
            r#"{"recording":{"enabled":false}}"#,
        )
        .unwrap();
        let loaded = Settings::load(temp.path()).get();
        assert_eq!(loaded.app.language, "");
        assert_eq!(loaded.lang(), crate::i18n::Lang::resolve(""));
    }

    #[test]
    fn api_keys_are_not_stored_in_plaintext() {
        let temp = tempfile::tempdir().unwrap();
        let settings = Settings::load(temp.path());
        settings
            .patch(&json!({"ai": {"apiKey": "sk-plaintext-secret-123", "searchApiKey": "brave-secret"}}))
            .unwrap();
        let raw = fs::read_to_string(temp.path().join("settings.json")).unwrap();
        assert!(!raw.contains("sk-plaintext-secret-123"));
        assert!(!raw.contains("brave-secret"));
        let loaded = Settings::load(temp.path()).get();
        assert_eq!(loaded.ai.api_key, "sk-plaintext-secret-123");
        assert_eq!(loaded.ai.search_api_key, "brave-secret");
    }

    #[test]
    fn legacy_plaintext_keys_still_load() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("settings.json"),
            r#"{"ai":{"apiKey":"sk-legacy"}}"#,
        )
        .unwrap();
        assert_eq!(Settings::load(temp.path()).get().ai.api_key, "sk-legacy");
    }

    #[test]
    fn old_files_with_missing_fields_get_defaults() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("settings.json"),
            r#"{"recording":{"enabled":false},"ai":{"baseUrl":"http://localhost:11434/v1"}}"#,
        )
        .unwrap();
        let loaded = Settings::load(temp.path()).get();
        assert!(!loaded.recording.enabled);
        assert!(!loaded.recording.browser_sites);
        assert_eq!(loaded.recording.idle_threshold_ms, 60_000);
        assert_eq!(loaded.ai.base_url, "http://localhost:11434/v1");
        assert_eq!(loaded.ai.auto_report_time, "21:30");
    }
}
