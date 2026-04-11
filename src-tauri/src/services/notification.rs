//! Native notification dispatching service.
//! Receives notification data from JS bridge and dispatches OS-native notifications.

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

/// Display a native OS notification with platform-appropriate sound.
///
/// # Arguments
/// * `app`    – Tauri application handle used to access the notification plugin.
/// * `title`  – Notification title text.
/// * `body`   – Notification body text.
/// * `tag`    – Conversation tag for deduplication (used as group identifier).
/// * `silent` – If `true`, suppress the notification sound.
///
/// # Errors
/// Returns an error string if the notification plugin fails to show the notification.
pub fn show_notification(
    app: &AppHandle,
    title: &str,
    body: &str,
    tag: &str,
    silent: bool,
) -> Result<(), String> {
    let mut builder = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .auto_cancel();

    // Group notifications by conversation tag for better organization.
    if !tag.is_empty() {
        builder = builder.group(tag);
    }

    // Play notification sound unless explicitly silenced.
    if !silent {
        builder = builder.sound(default_sound());
    } else {
        builder = builder.silent();
    }

    builder.show().map_err(|e| e.to_string())
}

/// Returns the platform-appropriate default notification sound name.
///
/// - **macOS**: `"default"` plays the system notification sound.
/// - **Linux**: `"message-new-instant"` uses the XDG sound theme.
/// - **Windows**: `"Default"` plays the default Windows notification sound.
fn default_sound() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "default"
    }
    #[cfg(target_os = "linux")]
    {
        "message-new-instant"
    }
    #[cfg(target_os = "windows")]
    {
        "Default"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_sound_returns_non_empty() {
        let sound = default_sound();
        assert!(
            !sound.is_empty(),
            "default_sound() should return a non-empty string"
        );
    }

    #[test]
    fn test_default_sound_is_known_value() {
        let sound = default_sound();
        let known_sounds = ["default", "Default", "message-new-instant"];
        assert!(
            known_sounds.contains(&sound),
            "default_sound() returned unexpected value: {sound}"
        );
    }
}
