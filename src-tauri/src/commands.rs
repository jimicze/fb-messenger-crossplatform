//! Tauri IPC commands.
//! Defines all commands callable from the frontend via `window.__TAURI__.core.invoke()`.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, Ordering};
use tauri::{AppHandle, Manager};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Application settings persisted across restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// Whether to persist the login session across app restarts.
    pub stay_logged_in: bool,
    /// Webview zoom level in the range [0.5, 3.0] (1.0 = 100 %).
    pub zoom_level: f64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            stay_logged_in: true,
            zoom_level: 1.0,
        }
    }
}

/// A single HTML snapshot used for offline viewing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotData {
    /// Full `document.documentElement.outerHTML` of the captured page.
    pub html: String,
    /// URL of the page at the time of capture.
    pub url: String,
    /// ISO-8601 timestamp of when the snapshot was taken.
    pub timestamp: String,
}

// ---------------------------------------------------------------------------
// Zoom bounds
// ---------------------------------------------------------------------------

/// Minimum allowed zoom level (50 %).
const MIN_ZOOM: f64 = 0.5;
/// Maximum allowed zoom level (300 %).
const MAX_ZOOM: f64 = 3.0;

// ---------------------------------------------------------------------------
// Unread-count guard (prevents redundant tray updates — B5 badge-flicker fix)
// ---------------------------------------------------------------------------

/// Last known unread count; `u32::MAX` forces an update on first call.
static LAST_UNREAD_COUNT: AtomicU32 = AtomicU32::new(u32::MAX);

// ---------------------------------------------------------------------------
// IPC command implementations
// ---------------------------------------------------------------------------

/// Send a native OS notification.
///
/// Called from the injected JS `Notification` override.
#[tauri::command]
pub fn send_notification(
    title: String,
    body: String,
    tag: String,
    app: AppHandle,
) -> Result<(), String> {
    crate::services::notification::show_notification(&app, &title, &body, &tag)
}

/// Update the unread-message count badge / tray tooltip.
///
/// Guards against redundant updates: if `count` equals the last-known value
/// the function returns immediately to avoid tray-icon flicker (B5).
#[tauri::command]
pub fn update_unread_count(count: u32, app: AppHandle) -> Result<(), String> {
    // Early-exit if count is unchanged.
    if LAST_UNREAD_COUNT.load(Ordering::SeqCst) == count {
        return Ok(());
    }
    LAST_UNREAD_COUNT.store(count, Ordering::SeqCst);

    // Update tray tooltip.
    if let Some(tray) = app.tray_by_id("messengerx-tray") {
        let tooltip = if count > 0 {
            format!("Messenger X ({})", count)
        } else {
            "Messenger X".to_string()
        };
        tray.set_tooltip(Some(&tooltip))
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Load the current application settings from disk.
#[tauri::command]
pub fn get_settings(app: AppHandle) -> Result<AppSettings, String> {
    crate::services::auth::load_settings(&app)
}

/// Persist application settings to disk.
#[tauri::command]
pub fn save_settings(settings: AppSettings, app: AppHandle) -> Result<(), String> {
    crate::services::auth::save_settings(&app, &settings)
}

/// Save an HTML snapshot of the current page.
///
/// Called by the timer script injected into the webview.
/// Keeps at most 3 snapshots, rotating out the oldest.
#[tauri::command]
pub fn save_snapshot(html: String, url: String, app: AppHandle) -> Result<(), String> {
    crate::services::cache::save_snapshot(&app, html, url)
}

/// Load the most recent HTML snapshot for offline viewing.
///
/// Returns `null` (serialised as `None`) when no snapshot is available.
#[tauri::command]
pub fn load_snapshot(app: AppHandle) -> Result<Option<SnapshotData>, String> {
    crate::services::cache::load_latest_snapshot(&app)
}

/// Clear all cached snapshots and reset settings to defaults.
#[tauri::command]
pub fn clear_all_data(app: AppHandle) -> Result<(), String> {
    crate::services::cache::clear_snapshots(&app)?;
    crate::services::auth::save_settings(&app, &AppSettings::default())
}

/// Open a URL in the system default browser.
#[tauri::command]
pub fn open_external(url: String, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// Set and persist the webview zoom level.
///
/// The level is clamped to [0.5, 3.0].  The new level is saved to settings and
/// immediately applied to the main webview via `window.__messenger_setZoom`.
#[tauri::command]
pub fn set_zoom(level: f64, app: AppHandle) -> Result<(), String> {
    let clamped = level.clamp(MIN_ZOOM, MAX_ZOOM);

    // Persist.
    let mut settings = crate::services::auth::load_settings(&app).unwrap_or_default();
    settings.zoom_level = clamped;
    crate::services::auth::save_settings(&app, &settings)?;

    // Apply to the live webview.
    if let Some(webview) = app.get_webview_window("main") {
        let script = format!(
            "window.__messenger_setZoom && window.__messenger_setZoom({});",
            clamped
        );
        let result: tauri::Result<()> = webview.eval(&script);
        result.map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Get the currently persisted zoom level.
#[tauri::command]
pub fn get_zoom(app: AppHandle) -> Result<f64, String> {
    let settings = crate::services::auth::load_settings(&app).unwrap_or_default();
    Ok(settings.zoom_level)
}
