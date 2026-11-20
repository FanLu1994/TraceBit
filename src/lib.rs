pub mod analyzer;
pub mod app;
pub mod categories;
pub mod digest;
pub mod i18n;
pub mod input;
pub mod insights;
pub mod intervals;
pub mod llm;
pub mod platform;
pub mod profile;
pub mod recommendations;
pub mod recorder;
pub mod redact;
pub mod report_html;
pub mod secret;
pub mod settings;
pub mod stats;
pub mod store;
pub mod web_search;

use std::path::PathBuf;

/// Keep the same per-user directory as the Electron release.
pub fn user_data_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("TraceBit")
}
