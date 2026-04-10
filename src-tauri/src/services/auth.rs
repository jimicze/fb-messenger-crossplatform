//! Session/login persistence service.
//! Manages application settings stored as JSON in the platform app-data directory.

use crate::commands::AppSettings;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// Returns the path to the settings JSON file inside the app-data directory.
///
/// # Errors
/// Returns an error string if the app-data directory cannot be resolved.
pub fn get_settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    Ok(dir.join("settings.json"))
}

/// Load application settings from disk.
///
/// If the settings file does not exist, returns `AppSettings::default()`.
///
/// # Errors
/// Returns an error string if the file exists but cannot be read or deserialised.
pub fn load_settings(app: &AppHandle) -> Result<AppSettings, String> {
    let path = get_settings_path(app)?;
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    let data =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read settings file: {e}"))?;
    serde_json::from_str(&data).map_err(|e| format!("Failed to parse settings JSON: {e}"))
}

/// Persist application settings to disk.
///
/// Creates the parent directory tree if it does not exist.
///
/// # Errors
/// Returns an error string if the directory cannot be created or the file cannot be written.
pub fn save_settings(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let path = get_settings_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create settings directory: {e}"))?;
    }
    let data = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialise settings: {e}"))?;
    std::fs::write(&path, data).map_err(|e| format!("Failed to write settings file: {e}"))
}
