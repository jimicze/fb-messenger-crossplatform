//! Native notification dispatching service.
//! Receives notification data from JS bridge and dispatches OS-native notifications.
//!
//! ## Linux strategy
//! On Linux we first attempt to dispatch via the `notify-send` CLI tool (from
//! `libnotify-bin`), which is pre-installed on Linux Mint and Ubuntu and works
//! reliably with both Cinnamon and GNOME session managers regardless of whether
//! the application has a registered `.desktop` file.  If `notify-send` is not
//! available or returns a non-zero exit code we fall back to
//! `tauri-plugin-notification` (direct D-Bus).

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
/// Returns an error string if all available notification methods fail.
pub fn show_notification(
    app: &AppHandle,
    title: &str,
    body: &str,
    tag: &str,
    silent: bool,
) -> Result<(), String> {
    // On Linux try notify-send first; fall back to tauri-plugin-notification.
    #[cfg(target_os = "linux")]
    match show_via_notify_send(title, body, silent) {
        Ok(()) => return Ok(()),
        Err(e) => {
            log::warn!(
                "notify-send unavailable or failed ({e}); \
                 falling back to tauri-plugin-notification"
            );
        }
    }

    show_via_tauri_plugin(app, title, body, tag, silent)
}

/// Dispatch a notification through `tauri-plugin-notification` (cross-platform).
fn show_via_tauri_plugin(
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

/// Dispatch a notification via the `notify-send` CLI tool (Linux only).
///
/// Uses `libnotify`'s command-line interface which is pre-installed on most
/// Linux distributions and works reliably with both Cinnamon and GNOME,
/// regardless of whether a `.desktop` file has been registered for the app.
///
/// Sound playback is requested via the `sound-name` hint supported by
/// libcanberra-based sound themes (e.g. Linux Mint / Cinnamon).
///
/// # Errors
/// Returns `Err` if the `notify-send` binary is not found on `$PATH` or
/// if the process exits with a non-zero status code.
#[cfg(target_os = "linux")]
fn show_via_notify_send(title: &str, body: &str, silent: bool) -> Result<(), String> {
    use std::process::Command;

    let mut cmd = Command::new("notify-send");
    cmd.arg("--app-name=Messenger X")
        .arg("--urgency=normal")
        .arg("--expire-time=5000")
        .arg("--icon=message-im");

    // Request sound playback via libcanberra hint when not silenced.
    if !silent {
        cmd.arg("--hint=string:sound-name:message-new-instant");
    }

    cmd.arg(title).arg(body);

    let status = cmd.status().map_err(|e| format!("failed to spawn notify-send: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "notify-send exited with non-zero status: {}",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "unknown".to_owned())
        ))
    }
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

    /// Verify that the notify-send command is constructed without panicking
    /// (does not actually invoke the binary — only tests argument assembly).
    #[cfg(target_os = "linux")]
    #[test]
    fn test_notify_send_args_assembled_without_panic() {
        use std::process::Command;
        let mut cmd = Command::new("notify-send");
        cmd.arg("--app-name=Messenger X")
            .arg("--urgency=normal")
            .arg("--expire-time=5000")
            .arg("--icon=message-im")
            .arg("--hint=string:sound-name:message-new-instant")
            .arg("Test title")
            .arg("Test body");
        // We only verify the Command was constructed — we do not spawn it.
        let prog = format!("{cmd:?}");
        assert!(prog.contains("notify-send"));
        assert!(prog.contains("Messenger X"));
    }
}
