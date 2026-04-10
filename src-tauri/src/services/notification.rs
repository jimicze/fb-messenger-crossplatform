//! Native notification dispatching service.
//! Receives notification data from JS bridge and dispatches OS-native notifications.

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// Display a native OS notification.
///
/// # Arguments
/// * `app`   – Tauri application handle used to access the notification plugin.
/// * `title` – Notification title text.
/// * `body`  – Notification body text.
/// * `_tag`  – Optional tag (used for deduplication on some platforms; currently unused).
///
/// # Errors
/// Returns an error string if the notification plugin fails to show the notification.
pub fn show_notification(
    app: &AppHandle,
    title: &str,
    body: &str,
    _tag: &str,
) -> Result<(), String> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}
