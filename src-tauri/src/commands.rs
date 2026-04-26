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
#[serde(default)]
pub struct AppSettings {
    /// Whether to persist the login session across app restarts.
    pub stay_logged_in: bool,
    /// Webview zoom level in the range [0.6, 1.2] (1.0 = 100 %).
    pub zoom_level: f64,
    /// Whether native OS notifications are enabled.
    pub notifications_enabled: bool,
    /// Whether notification sounds are enabled (false = silent).
    pub notification_sound: bool,
    /// Whether the app auto-starts at system login.
    pub autostart: bool,
    /// Whether the app starts minimized to the system tray.
    pub start_minimized: bool,
    /// Whether to automatically check for updates in the background (once per month).
    pub auto_update: bool,
    /// Unix timestamp (seconds) of the last update check; `None` if never checked.
    pub last_update_check_secs: Option<u64>,
    /// Appearance mode: `"system"` (follow OS), `"dark"`, or `"light"`.
    pub appearance: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            stay_logged_in: true,
            zoom_level: 1.0,
            notifications_enabled: true,
            notification_sound: true,
            autostart: false,
            start_minimized: false,
            auto_update: true,
            last_update_check_secs: None,
            appearance: "system".to_string(),
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

/// Minimum allowed zoom level (60 %).
const MIN_ZOOM: f64 = 0.6;
/// Maximum allowed zoom level (120 %).
const MAX_ZOOM: f64 = 1.2;

// ---------------------------------------------------------------------------
// Unread-count guard (prevents redundant tray updates — B5 badge-flicker fix)
// ---------------------------------------------------------------------------

/// Last known unread count; `u32::MAX` forces an update on first call.
static LAST_UNREAD_COUNT: AtomicU32 = AtomicU32::new(u32::MAX);

/// Last unread count for which we already sent a notification.
/// Prevents duplicate notifications when the count stays the same across multiple calls.
static LAST_NOTIFIED_COUNT: AtomicU32 = AtomicU32::new(0);

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
    silent: bool,
    app: AppHandle,
) -> Result<(), String> {
    let settings = crate::services::auth::load_settings(&app).unwrap_or_default();
    log::info!(
        "[MessengerX][Notification] send_notification called: title={title:?} body_len={} tag={tag:?} silent={} notifications_enabled={} notification_sound={}",
        body.chars().count(),
        silent,
        settings.notifications_enabled,
        settings.notification_sound
    );
    if !settings.notifications_enabled {
        log::info!(
            "[MessengerX][Notification] send_notification skipped: notifications disabled in settings"
        );
        return Ok(());
    }
    // Force silent if user disabled notification sounds.
    let effective_silent = silent || !settings.notification_sound;
    log::info!(
        "[MessengerX][Notification] effective_silent={effective_silent}"
    );
    let result = crate::services::notification::show_notification(
        &app,
        &title,
        &body,
        &tag,
        effective_silent,
    );
    match &result {
        Ok(()) => log::info!(
            "[MessengerX][Notification] send_notification finished successfully"
        ),
        Err(e) => log::warn!(
            "[MessengerX][Notification] send_notification failed: {e}"
        ),
    }
    result
}

/// Update the unread-message count badge / tray tooltip.
///
/// Guards against redundant updates: if `count` equals the last-known value
/// the function returns immediately to avoid tray-icon flicker (B5).
///
/// When the count **increases** and the main window is not focused a generic
/// "New message" native notification is sent so the user is alerted even when
/// Messenger's own JS `Notification` API is never triggered by WKWebView.
#[tauri::command]
pub fn update_unread_count(count: u32, app: AppHandle) -> Result<(), String> {
    // Read old count BEFORE updating so we can detect an increase.
    let old_count = LAST_UNREAD_COUNT.load(Ordering::SeqCst);

    // Early-exit if count is unchanged.
    if old_count == count {
        return Ok(());
    }
    LAST_UNREAD_COUNT.store(count, Ordering::SeqCst);

    // -----------------------------------------------------------------------
    // Fire a native notification when unread count increases while the window
    // is not focused.  This is our primary notification path because Messenger
    // on WKWebView never calls window.Notification() (confirmed v1.3.15).
    // -----------------------------------------------------------------------
    // Treat u32::MAX sentinel (initial value) as "no previous messages".
    let real_old = if old_count == u32::MAX { 0 } else { old_count };
    if count > real_old {
        let settings = crate::services::auth::load_settings(&app).unwrap_or_default();
        if settings.notifications_enabled {
            let is_focused = app
                .get_webview_window("main")
                .and_then(|w| w.is_focused().ok())
                .unwrap_or(false);
            let last_notified = LAST_NOTIFIED_COUNT.load(Ordering::SeqCst);
            if !is_focused && count > last_notified {
                LAST_NOTIFIED_COUNT.store(count, Ordering::SeqCst);
                let effective_silent = !settings.notification_sound;
                let locale = crate::services::locale::detect_locale();
                let tr = crate::services::locale::get_translations(&locale);
                log::info!(
                    "[MessengerX][Notification] unread count increased {} → {}; sending notification (silent={})",
                    real_old,
                    count,
                    effective_silent,
                );
                if let Err(e) = crate::services::notification::show_notification(
                    &app,
                    &tr.notification_new_message,
                    "",
                    "messenger-unread",
                    effective_silent,
                ) {
                    log::warn!("[MessengerX][Notification] unread-count notification failed: {e}");
                }
            }
        }
    }

    // When the user reads messages (count drops to 0) reset the notified counter
    // so the next increase fires again.
    if count == 0 {
        LAST_NOTIFIED_COUNT.store(0, Ordering::SeqCst);
    }

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

    // Update macOS Dock badge label.
    #[cfg(target_os = "macos")]
    if let Some(webview) = app.get_webview_window("main") {
        let label = if count > 0 {
            Some(count.to_string())
        } else {
            None
        };
        if let Err(e) = webview.set_badge_label(label) {
            log::warn!("[MessengerX][Badge] Failed to set dock badge: {e}");
        }
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
/// The level is clamped to [0.6, 1.2].  The new level is saved to settings and
/// immediately applied to the main webview via the native `set_zoom` API.
/// Unlike CSS `body.style.zoom`, the native API scales the entire viewport so
/// the page layout fills the window correctly at all zoom levels.
#[tauri::command]
pub fn set_zoom(level: f64, app: AppHandle) -> Result<(), String> {
    let clamped = level.clamp(MIN_ZOOM, MAX_ZOOM);

    // Persist.
    let mut settings = crate::services::auth::load_settings(&app).unwrap_or_default();
    settings.zoom_level = clamped;
    crate::services::auth::save_settings(&app, &settings)?;

    // Apply native zoom to the live webview.
    if let Some(webview) = app.get_webview_window("main") {
        webview.set_zoom(clamped).map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Get the currently persisted zoom level.
#[tauri::command]
pub fn get_zoom(app: AppHandle) -> Result<f64, String> {
    let settings = crate::services::auth::load_settings(&app).unwrap_or_default();
    Ok(settings.zoom_level)
}

/// Check whether a newer version is available from the update endpoint.
///
/// Returns `Some(version_string)` if an update is available, `None` otherwise.
/// Used by the settings window which cannot use ES module imports directly.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    Ok(update.map(|u| u.version))
}

/// Download and install the available update (if any).
///
/// Should be called after `check_for_update` returns `Some(_)`.
/// After this returns successfully the caller should trigger a relaunch.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app
        .updater()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    if let Some(update) = update {
        update
            .download_and_install(|_chunk, _total| {}, || {})
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Log a message from the WebView JavaScript context to the Rust log file.
///
/// This allows JS debug output to appear in `messengerx.log` via `log::info!`,
/// since `console.log` only goes to the WebKit inspector and never to the file.
#[tauri::command]
pub fn js_log(message: String) {
    log::info!("[MessengerX][JS] {}", message);
}

/// Return whether the main window should be treated as visible by Messenger.
///
/// On Linux this is stricter than plain OS focus: the window must be focused
/// and not minimized. The Visibility API shim calls this on every main-frame
/// page load so that the JS `document.visibilityState` / `document.hasFocus()`
/// overrides are synchronised from the real OS window state instead of from a
/// baked-in start-up preference. Without this resync, re-navigations (e.g.
/// logout -> login page) could reinitialise the shim with a stale state.
#[tauri::command]
pub fn get_window_focused(app: AppHandle) -> bool {
    let Some(window) = app.get_webview_window("main") else {
        log::warn!(
            "[MessengerX][Visibility] get_window_focused: main window missing"
        );
        return false;
    };
    let Ok(is_focused) = window.is_focused() else {
        log::warn!(
            "[MessengerX][Visibility] get_window_focused: is_focused() failed"
        );
        return false;
    };
    let Ok(is_minimized) = window.is_minimized() else {
        log::warn!(
            "[MessengerX][Visibility] get_window_focused: is_minimized() failed"
        );
        return false;
    };
    let effective_visible = is_focused && !is_minimized;
    log::info!(
        "[MessengerX][Visibility] get_window_focused -> focused={} minimized={} visible={}",
        is_focused,
        is_minimized,
        effective_visible
    );
    effective_visible
}

/// Enable or disable auto-start at system login.
///
/// Wraps the autostart plugin so the settings window can call it via `invoke`.
#[tauri::command]
pub fn set_autostart(enabled: bool, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let autolaunch = app.autolaunch();
    if enabled {
        autolaunch.enable().map_err(|e| e.to_string())
    } else {
        autolaunch.disable().map_err(|e| e.to_string())
    }
}

/// Check whether auto-start is currently enabled.
#[tauri::command]
pub fn is_autostart_enabled(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_settings_default_values() {
        let settings = AppSettings::default();
        assert!(settings.stay_logged_in);
        assert!((settings.zoom_level - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_app_settings_serialization_roundtrip() {
        let settings = AppSettings {
            stay_logged_in: false,
            zoom_level: 1.5,
            notifications_enabled: false,
            notification_sound: false,
            autostart: true,
            start_minimized: true,
            auto_update: false,
            last_update_check_secs: Some(1_000_000),
            appearance: "dark".to_string(),
        };
        let json = serde_json::to_string(&settings).expect("serialize");
        let deserialized: AppSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(!deserialized.stay_logged_in);
        assert!((deserialized.zoom_level - 1.5).abs() < f64::EPSILON);
        assert!(!deserialized.notifications_enabled);
        assert!(!deserialized.notification_sound);
        assert!(deserialized.autostart);
        assert!(deserialized.start_minimized);
        assert_eq!(deserialized.appearance, "dark");
    }

    #[test]
    fn test_app_settings_backward_compat_missing_fields() {
        // Old settings JSON with only original fields — new fields should default.
        let json = r#"{"stay_logged_in": true, "zoom_level": 1.2}"#;
        let settings: AppSettings = serde_json::from_str(json).expect("deserialize");
        assert!(settings.stay_logged_in);
        assert!((settings.zoom_level - 1.2).abs() < f64::EPSILON);
        // New fields use Default values:
        assert!(settings.notifications_enabled);
        assert!(settings.notification_sound);
        assert!(!settings.autostart);
        assert!(!settings.start_minimized);
        // New appearance field defaults to "system"
        assert_eq!(settings.appearance, "system");
    }

    #[test]
    fn test_snapshot_data_serialization_roundtrip() {
        let snapshot = SnapshotData {
            html: "<html><body>Test</body></html>".to_string(),
            url: "https://www.messenger.com".to_string(),
            timestamp: "2026-04-11T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&snapshot).expect("serialize");
        let deserialized: SnapshotData = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.html, snapshot.html);
        assert_eq!(deserialized.url, snapshot.url);
        assert_eq!(deserialized.timestamp, snapshot.timestamp);
    }

    #[test]
    fn test_snapshot_data_handles_html_special_chars() {
        let snapshot = SnapshotData {
            html: r#"<div class="test">Hello "world" & <script>alert('xss')</script></div>"#
                .to_string(),
            url: "https://www.messenger.com/t/123".to_string(),
            timestamp: "2026-04-11T12:00:00+02:00".to_string(),
        };
        let json = serde_json::to_string(&snapshot).expect("serialize");
        let deserialized: SnapshotData = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.html, snapshot.html);
    }

    #[test]
    fn test_zoom_clamp_logic() {
        // Verify the clamping constants are sensible (checked at compile-time via type system)
        const _: () = {
            assert!(MIN_ZOOM > 0.0);
        };
        const _: () = {
            assert!(MIN_ZOOM < 1.0);
        };
        const _: () = {
            assert!(MAX_ZOOM > 1.0);
        };
        const _: () = {
            assert!(MAX_ZOOM <= 2.0);
        };

        // Test clamping behavior
        let too_low = 0.1_f64.clamp(MIN_ZOOM, MAX_ZOOM);
        assert!((too_low - MIN_ZOOM).abs() < f64::EPSILON);

        let too_high = 10.0_f64.clamp(MIN_ZOOM, MAX_ZOOM);
        assert!((too_high - MAX_ZOOM).abs() < f64::EPSILON);

        let normal = 1.1_f64.clamp(MIN_ZOOM, MAX_ZOOM);
        assert!((normal - 1.1).abs() < f64::EPSILON);
    }
}
