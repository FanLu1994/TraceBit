pub mod analyzer;
pub mod app;
pub mod categories;
pub mod input;
pub mod platform;
pub mod recommendations;
pub mod recorder;
pub mod settings;
pub mod store;
pub mod web_search;

use std::path::PathBuf;

/// Keep the same per-user directory as the Electron release.
pub fn user_data_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("TraceBit")
}
