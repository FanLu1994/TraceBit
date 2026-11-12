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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub autostart: bool,
    pub close_to_tray: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            autostart: false,
            close_to_tray: true,
        }
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
                serde_json::from_str(&raw)
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
        let json = serde_json::to_vec_pretty(data)?;
        fs::write(&self.file, json).with_context(|| format!("无法保存 {}", self.file.display()))
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
