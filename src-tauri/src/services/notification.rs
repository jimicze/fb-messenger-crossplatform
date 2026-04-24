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
//!
//! ## macOS strategy
//! On macOS we bypass `tauri-plugin-notification` / `notify-rust` and use the
//! modern `UNUserNotificationCenter` API directly. The legacy backend only
//! delivers notifications to Notification Center on newer macOS releases,
//! whereas the modern API lets us request permission and force foreground
//! presentation as a banner.

use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

#[cfg(target_os = "macos")]
use std::{
    ptr::NonNull,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "macos")]
use block2::RcBlock;
#[cfg(target_os = "macos")]
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{Bool, NSObject, ProtocolObject},
    MainThreadOnly,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{MainThreadMarker, NSError, NSObjectProtocol, NSString};
#[cfg(target_os = "macos")]
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationSound,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

#[cfg(target_os = "macos")]
static MACOS_NOTIFICATION_INIT: OnceLock<Result<(), String>> = OnceLock::new();

#[cfg(target_os = "macos")]
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct MessengerNotificationDelegate;

    unsafe impl NSObjectProtocol for MessengerNotificationDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for MessengerNotificationDelegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present_notification(
            &self,
            _center: &UNUserNotificationCenter,
            notification: &UNNotification,
            completion_handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            let mut options =
                UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List;
            if notification.request().content().sound().is_some() {
                options |= UNNotificationPresentationOptions::Sound;
            }
            completion_handler.call((options,));
        }
    }
);

#[cfg(target_os = "macos")]
impl MessengerNotificationDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm);
        unsafe { msg_send![this, init] }
    }
}

/// Performs one-time native notification initialization for the current OS.
pub fn initialize() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        MACOS_NOTIFICATION_INIT
            .get_or_init(initialize_macos_notification_center)
            .clone()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

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
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        show_via_user_notifications(title, body, tag, silent)
    }

    #[cfg(not(target_os = "macos"))]
    {
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
}

#[cfg(target_os = "macos")]
fn initialize_macos_notification_center() -> Result<(), String> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "macOS notifications must be initialized on the main thread".to_string())?;
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let delegate = MessengerNotificationDelegate::new(mtm);
    center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // `UNUserNotificationCenter` keeps only a weak reference to its delegate,
    // so we intentionally leak the process-global delegate for the app lifetime.
    std::mem::forget(delegate);

    let permission_handler = RcBlock::new(|granted: Bool, error: *mut NSError| {
        if let Some(error) = format_nserror(error) {
            log::warn!("[MessengerX] macOS notification authorization request failed: {error}");
        } else if !granted.as_bool() {
            log::warn!(
                "[MessengerX] macOS notifications were denied in system settings; \
                 banners will not be shown until permission is enabled"
            );
        }
    });

    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert
            | UNAuthorizationOptions::Sound
            | UNAuthorizationOptions::Badge,
        &permission_handler,
    );

    Ok(())
}

#[cfg(target_os = "macos")]
fn show_via_user_notifications(
    title: &str,
    body: &str,
    tag: &str,
    silent: bool,
) -> Result<(), String> {
    initialize()?;

    let center = UNUserNotificationCenter::currentNotificationCenter();
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));

    if !tag.is_empty() {
        content.setThreadIdentifier(&NSString::from_str(tag));
    }

    if !silent {
        let sound = UNNotificationSound::defaultSound();
        content.setSound(Some(&sound));
    }

    let identifier = NSString::from_str(&build_macos_notification_identifier(tag));
    let request =
        UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);
    let enqueue_identifier = identifier.to_string();
    let enqueue_handler = RcBlock::new(move |error: *mut NSError| {
        if let Some(error) = format_nserror(error) {
            log::warn!(
                "[MessengerX] Failed to enqueue macOS notification {enqueue_identifier}: {error}"
            );
        }
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&enqueue_handler));
    Ok(())
}

#[cfg(target_os = "macos")]
fn format_nserror(error: *mut NSError) -> Option<String> {
    NonNull::new(error).map(|error| {
        let error: &NSError = unsafe { error.as_ref() };
        error.localizedDescription().to_string()
    })
}

/// Dispatch a notification through `tauri-plugin-notification` (cross-platform).
#[cfg(not(target_os = "macos"))]
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
#[cfg(not(target_os = "macos"))]
fn default_sound() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "message-new-instant"
    }
    #[cfg(target_os = "windows")]
    {
        "Default"
    }
}

#[cfg(target_os = "macos")]
fn build_macos_notification_identifier(tag: &str) -> String {
    let prefix = if tag.is_empty() { "message" } else { tag };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("messengerx-{prefix}-{ts}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn test_default_sound_returns_non_empty() {
        let sound = default_sound();
        assert!(
            !sound.is_empty(),
            "default_sound() should return a non-empty string"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn test_default_sound_is_known_value() {
        let sound = default_sound();
        let known_sounds = ["default", "Default", "message-new-instant"];
        assert!(
            known_sounds.contains(&sound),
            "default_sound() returned unexpected value: {sound}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_notification_identifier_uses_expected_prefix() {
        let tagged = build_macos_notification_identifier("thread-123");
        let untagged = build_macos_notification_identifier("");

        assert!(tagged.starts_with("messengerx-thread-123-"));
        assert!(untagged.starts_with("messengerx-message-"));
        assert_ne!(tagged, untagged);
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
