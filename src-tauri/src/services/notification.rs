//! Native notification dispatching service.
//! Receives notification data from JS bridge and dispatches OS-native notifications.
//!
//! ## Linux strategy
//! On Linux we first attempt to dispatch via the `notify-send` CLI tool (from
//! `libnotify-bin`), which is pre-installed on Linux Mint and Ubuntu and works
//! reliably with both Cinnamon and GNOME session managers regardless of whether
//! the application has a registered `.desktop` file. If `notify-send` is not
//! available or returns a non-zero exit code we fall back to
//! `tauri-plugin-notification` (direct D-Bus).
//!
//! ## macOS strategy
//! **Release (inside `.app` bundle):** Uses `UNUserNotificationCenter` directly —
//! the modern UserNotifications API that delivers banners to Notification Center
//! and lets us request permission and force foreground presentation.
//!
//! **Fallback:** Calling `UNUserNotificationCenter` /
//! `currentNotificationCenter()` outside a proper `.app` bundle crashes with
//! `NSInternalInconsistencyException`, so dev mode uses `/usr/bin/osascript`.
//! Release builds still use UserNotifications first, but can fall back to
//! `osascript` if the system accepts a request and then drops it before it appears
//! in delivered notifications.  The AppleScript program is passed via `argv` (not
//! a shell string), and notification text is escaped as AppleScript string
//! literals.

use std::sync::atomic::{AtomicU64, Ordering};

use tauri::AppHandle;
#[cfg(not(target_os = "macos"))]
use tauri_plugin_notification::NotificationExt;

#[cfg(target_os = "macos")]
use std::{
    ptr::NonNull,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

/// Process-global monotonically-increasing notification dispatch counter.
///
/// Every entry into [`show_notification`] obtains a fresh `call_id` and logs it
/// at all sink decision points (`[NotifSink] call_id=N site=… via=…`).  The
/// counter exists purely for log correlation: when the user reports duplicate
/// or unexpected toasts (e.g. Linux 3rd-toast diagnostic in Phase M), grepping
/// `call_id` reveals whether the OS produced one or many native notifications
/// per Rust dispatch — disambiguating Rust double-fire from
/// notify-send/tauri-plugin double-sink from external SW push paths.
static NOTIF_CALL_ID: AtomicU64 = AtomicU64::new(0);

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
use objc2_foundation::{MainThreadMarker, NSArray, NSError, NSObjectProtocol, NSString};
#[cfg(target_os = "macos")]
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationSetting,
    UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
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
            eprintln!(
                "[MessengerX][NotifyHelper][Delegate] willPresent id={:?} title={:?} sound={}",
                notification.request().identifier().to_string(),
                notification.request().content().title().to_string(),
                notification.request().content().sound().is_some(),
            );
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
/// * `site`   – Caller-origin label for diagnostic correlation (e.g.
///   `"unread-count-core"`, `"updater-installed"`, `"ipc-send-notification"`).
///   Logged together with a process-global `call_id` so duplicate-toast
///   investigations can grep a single Rust dispatch across all sink branches.
///
/// # Errors
/// Returns an error string if all available notification methods fail.
pub fn show_notification(
    app: &AppHandle,
    title: &str,
    body: &str,
    tag: &str,
    silent: bool,
    site: &str,
) -> Result<(), String> {
    let call_id = NOTIF_CALL_ID.fetch_add(1, Ordering::Relaxed) + 1;
    log::info!(
        "[MessengerX][NotifSink] call_id={call_id} site={site:?} via=entry \
         title={title:?} body_len={} tag={tag:?} silent={silent}",
        body.chars().count()
    );

    #[cfg(target_os = "macos")]
    {
        let _ = app;
        let result = show_via_user_notifications(title, body, tag, silent);
        match &result {
            Ok(()) => {
                log::info!(
                    "[MessengerX][NotifSink] call_id={call_id} site={site:?} via=macos-un \
                     result=ok"
                )
            }
            Err(e) => log::warn!(
                "[MessengerX][NotifSink] call_id={call_id} site={site:?} via=macos-un \
                 result=err err={e}"
            ),
        }
        result
    }

    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(target_os = "linux")]
        match show_via_notify_send(title, body, silent) {
            Ok(()) => {
                log::info!(
                    "[MessengerX][NotifSink] call_id={call_id} site={site:?} \
                     via=linux-notify-send result=ok"
                );
                return Ok(());
            }
            Err(e) => {
                log::warn!(
                    "[MessengerX][NotifSink] call_id={call_id} site={site:?} \
                     via=linux-notify-send result=err err={e} fallback=tauri-plugin"
                );
            }
        }

        let result = show_via_tauri_plugin(app, title, body, tag, silent);
        match &result {
            Ok(()) => log::info!(
                "[MessengerX][NotifSink] call_id={call_id} site={site:?} via=tauri-plugin \
                 result=ok"
            ),
            Err(e) => log::warn!(
                "[MessengerX][NotifSink] call_id={call_id} site={site:?} via=tauri-plugin \
                 result=err err={e}"
            ),
        }
        result
    }
}

/// Returns `true` when the current process executable is located inside a
/// macOS `.app` bundle (i.e. a release/distribution build).
///
/// `UNUserNotificationCenter` requires a valid `.app` bundle context and will
/// crash with `NSInternalInconsistencyException` when the binary is run
/// directly (e.g. `tauri dev` / `cargo run`).  Use this guard to select the
/// appropriate notification path.
#[cfg(target_os = "macos")]
fn is_running_in_macos_app_bundle() -> bool {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS"))
        .unwrap_or(false)
}

/// Dispatch a notification directly via `UNUserNotificationCenter`, bypassing
/// all dev-mode guards (osascript fallback, subprocess delegation).
///
/// This is the entry-point for the CLI `--notify` helper mode: the binary is
/// already running **inside** the debug `.app` bundle, so the bundle context
/// is valid.  Calling this skips `show_via_user_notifications` (which would
/// re-check the bundle guard and potentially recurse into the subprocess path)
/// and goes straight to the UNUserNotificationCenter enqueue.
///
/// Exported as `pub(crate)` so that `lib.rs` can expose it as a `pub` function
/// to `main.rs` without making internal notification helpers part of the crate
/// public API.
///
/// # Errors
/// Returns `Err` if UNUserNotificationCenter initialisation or enqueue fails.
#[cfg(target_os = "macos")]
pub(crate) fn dispatch_bundle_notification(
    title: &str,
    body: &str,
    silent: bool,
) -> Result<(), String> {
    // Initialise the notification center (sets delegate, requests permission).
    // This call is idempotent — the OnceLock inside `initialize` ensures it
    // runs only once per process.
    initialize()?;

    let center = UNUserNotificationCenter::currentNotificationCenter();
    log_macos_notification_settings(&center, "helper-before-enqueue");
    log_macos_delivered_count(&center, "helper-before-enqueue");
    log_macos_pending_count(&center, "helper-before-enqueue");
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));

    if !silent {
        let sound = UNNotificationSound::defaultSound();
        content.setSound(Some(&sound));
        eprintln!("[MessengerX][NotifyHelper] sound=default (silent=false)");
        eprintln!("[MessengerX][AudioRust] dispatch_bundle_notification requesting OS sound: UNNotificationSound.defaultSound title={title:?}");
    } else {
        eprintln!("[MessengerX][NotifyHelper] sound omitted (silent=true)");
    }

    let identifier = NSString::from_str(&build_macos_notification_identifier("messenger-unread"));
    let request =
        UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);
    let enqueue_id = identifier.to_string();

    // Use a channel so we can report the enqueue result synchronously before
    // the 2 s sleep that main.rs performs after this call.
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let enqueue_handler = RcBlock::new(move |error: *mut NSError| {
        if let Some(e) = format_nserror(error) {
            eprintln!("[MessengerX][NotifyHelper] enqueue failed: {e}");
            let _ = tx.send(Err(e));
        } else {
            eprintln!("[MessengerX][NotifyHelper] enqueued: {enqueue_id}");
            let _ = tx.send(Ok(()));
        }
    });
    center.addNotificationRequest_withCompletionHandler(&request, Some(&enqueue_handler));

    // Wait up to 3 s for the completion handler.  On the main thread this
    // blocks, but in the CLI helper there is no run-loop / event queue to
    // worry about; the ObjC completion handler fires on a background thread.
    let result = match rx.recv_timeout(std::time::Duration::from_secs(3)) {
        Ok(result) => result,
        Err(_) => {
            eprintln!("[MessengerX][NotifyHelper] timed out waiting for enqueue callback");
            Ok(()) // assume it was enqueued; main.rs sleeps 2 s anyway
        }
    };

    if result.is_ok() {
        schedule_macos_helper_delivery_check(identifier.to_string());
    }

    result
}

/// Attempt to delegate notification dispatch to the debug `.app` bundle binary.
///
/// When `npm run tauri dev` runs the binary **directly** from
/// `target/debug/messengerx` (outside a `.app`), `UNUserNotificationCenter`
/// is unavailable.  However, the Tauri debug build also produces a full `.app`
/// bundle at `target/debug/bundle/macos/Messenger X.app`.  If that bundle
/// binary exists we can spawn it with `--notify` args so it runs inside a
/// valid bundle context and can call UNUserNotificationCenter.
///
/// Returns `Ok(())` if the subprocess succeeds, `Err` otherwise (caller then
/// falls through to osascript).
///
/// Guard against infinite recursion: if the current executable already lives
/// inside a `.app` bundle this function returns `Err` immediately — the caller
/// (`show_via_user_notifications`) never reaches this path because the bundle
/// guard at the top of that function would have let it proceed directly.
#[cfg(target_os = "macos")]
fn try_delegate_to_app_bundle(title: &str, body: &str, silent: bool) -> Result<(), String> {
    use std::path::PathBuf;
    use std::process::Command;

    // Refuse to recurse: if we are already inside a .app, do nothing here.
    if is_running_in_macos_app_bundle() {
        return Err("already inside .app bundle — not delegating".to_owned());
    }

    // current_exe is expected to be  .../target/debug/messengerx
    // The .app binary lives at      .../target/debug/bundle/macos/Messenger X.app/Contents/MacOS/messengerx
    let current_exe = std::env::current_exe().map_err(|e| format!("current_exe failed: {e}"))?;

    let debug_dir = current_exe
        .parent()
        .ok_or_else(|| "current_exe has no parent directory".to_owned())?;

    let bundle_bin: PathBuf = debug_dir
        .join("bundle")
        .join("macos")
        .join("Messenger X.app")
        .join("Contents")
        .join("MacOS")
        .join("messengerx");

    if !bundle_bin.exists() {
        return Err(format!(
            "debug .app bundle binary not found at {}",
            bundle_bin.display()
        ));
    }

    // Double-check: don't exec ourselves if the paths canonicalize to the same
    // file (should not happen given the guard above, but be explicit).
    let canonical_bundle = bundle_bin
        .canonicalize()
        .unwrap_or_else(|_| bundle_bin.clone());
    let canonical_self = current_exe
        .canonicalize()
        .unwrap_or_else(|_| current_exe.clone());
    if canonical_bundle == canonical_self {
        return Err("bundle binary is the same file as current exe — skipping".to_owned());
    }

    let info_plist = bundle_bin
        .parent()
        .and_then(|macos_dir| macos_dir.parent())
        .map(|contents_dir| contents_dir.join("Info.plist"))
        .ok_or_else(|| "could not derive Info.plist path for debug .app".to_owned())?;
    let bundle_version = read_bundle_short_version(&info_plist)
        .ok_or_else(|| format!("could not read version from {}", info_plist.display()))?;
    let current_version = env!("CARGO_PKG_VERSION");
    if bundle_version != current_version {
        return Err(format!(
            "debug .app bundle version mismatch: bundle={bundle_version:?} current={current_version:?}; \
             run `npm run tauri build -- --debug` once to refresh the notification helper"
        ));
    }

    let silent_arg = if silent { "silent" } else { "not-silent" };

    log::info!(
        "[MessengerX][Notification] Delegating notification to debug .app bundle: \
         binary={} title={title:?} silent={silent}",
        bundle_bin.display()
    );

    let output = Command::new(&bundle_bin)
        .env("MESSENGERX_NOTIFY_HELPER", "1")
        .arg("--notify")
        .arg(title)
        .arg(body)
        .arg(silent_arg)
        .output()
        .map_err(|e| format!("failed to spawn bundle binary: {e}"))?;

    let exit_code = output
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout_t = stdout.trim();
    let stderr_t = stderr.trim();
    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        output.status.signal()
    };

    if output.status.success() {
        log::info!(
            "[MessengerX][Notification] debug .app bundle notification succeeded: \
             exit={exit_code} stdout={stdout_t:?} stderr={stderr_t:?}"
        );
        Ok(())
    } else {
        #[cfg(unix)]
        let signal_msg = format!(" signal={signal:?}");
        #[cfg(not(unix))]
        let signal_msg = String::new();
        Err(format!(
            "debug .app bundle binary exited non-zero: exit={exit_code}{signal_msg} \
             stdout={stdout_t:?} stderr={stderr_t:?}"
        ))
    }
}

#[cfg(target_os = "macos")]
fn read_bundle_short_version(info_plist: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(info_plist).ok()?;
    let key = "<key>CFBundleShortVersionString</key>";
    let key_pos = text.find(key)?;
    let after_key = &text[key_pos + key.len()..];
    let start_tag = "<string>";
    let start = after_key.find(start_tag)? + start_tag.len();
    let end = after_key[start..].find("</string>")?;
    Some(after_key[start..start + end].trim().to_owned())
}

/// macOS notification fallback via `/usr/bin/osascript`.
///
/// Called when the process is NOT running inside a `.app` bundle (i.e.
/// `npm run tauri dev` / `cargo run`), or as a last-resort release fallback if
/// `UNUserNotificationCenter` accepts a request but does not list it as
/// delivered shortly afterwards.
///
/// The generated AppleScript is passed via `argv` (not shell interpolation), and
/// notification text is escaped as AppleScript string literals.
///
/// # Errors
/// Returns `Err` if `osascript` cannot be spawned or exits non-zero.
#[cfg(target_os = "macos")]
fn show_via_osascript_fallback(
    title: &str,
    body: &str,
    silent: bool,
    reason: &'static str,
) -> Result<(), String> {
    use std::process::Command;

    let current_exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "<unknown>".to_owned());
    let pid = std::process::id();
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();

    let script = build_osascript_notification_script(title, body, silent);

    log::info!(
        "[MessengerX][Notification][OSASCRIPT FALLBACK] Dispatching macOS notification via \
         osascript: reason={reason} title={title:?} silent={silent} \
         script_len={} exe={current_exe:?} pid={pid} \
         TERM_PROGRAM={term_program:?} TERM={term:?}",
        script.len(),
    );

    let output = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|e| format!("failed to spawn osascript: {e}"))?;

    let exit_code = output
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout_trimmed = stdout.trim();
    let stderr_trimmed = stderr.trim();

    if output.status.success() {
        log::info!(
            "[MessengerX][Notification][OSASCRIPT FALLBACK] osascript succeeded: \
             exit_code={exit_code} stdout={stdout_trimmed:?} stderr={stderr_trimmed:?}"
        );
        Ok(())
    } else {
        Err(format!(
            "osascript exited with non-zero status: exit_code={exit_code} \
             stdout={stdout_trimmed:?} stderr={stderr_trimmed:?}"
        ))
    }
}

#[cfg(target_os = "macos")]
fn build_osascript_notification_script(title: &str, body: &str, _silent: bool) -> String {
    // Sound is intentionally omitted from both silent and non-silent dev-mode
    // fallback scripts.  `sound name "Funk"` is unreliable outside a proper
    // `.app` bundle context and causes osascript to fail on some macOS
    // configurations.  The System sound is still played by
    // UNUserNotificationCenter in release builds.
    let body = apple_script_string_literal(body);
    let title = apple_script_string_literal(title);
    format!("display notification {body} with title {title}")
}

#[cfg(target_os = "macos")]
fn apple_script_string_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' | '\t' => out.push(' '),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(target_os = "macos")]
fn log_macos_notification_settings(center: &UNUserNotificationCenter, context: &'static str) {
    let handler = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        let settings = unsafe { settings.as_ref() };
        let authorization_status = settings.authorizationStatus();
        let alert_setting = settings.alertSetting();
        let notification_center_setting = settings.notificationCenterSetting();
        let sound_setting = settings.soundSetting();
        let badge_setting = settings.badgeSetting();
        let alert_style = settings.alertStyle();
        log::info!(
            "[MessengerX][Notification][DIAG] macOS settings ({context}): \
             authorizationStatus={} alertSetting={} notificationCenterSetting={} \
             soundSetting={} badgeSetting={} alertStyle={}",
            authorization_status.0,
            alert_setting.0,
            notification_center_setting.0,
            sound_setting.0,
            badge_setting.0,
            alert_style.0,
        );
        if authorization_status == UNAuthorizationStatus::Denied
            || authorization_status == UNAuthorizationStatus::NotDetermined
            || alert_setting == UNNotificationSetting::Disabled
        {
            log::warn!(
                "[MessengerX][Notification][DIAG] macOS system settings may block banners; \
                 enable notifications for Messenger X in System Settings → Notifications"
            );
        }
    });
    center.getNotificationSettingsWithCompletionHandler(&handler);
}

#[cfg(target_os = "macos")]
fn log_macos_delivered_count(center: &UNUserNotificationCenter, context: &'static str) {
    let handler = RcBlock::new(move |notifications: NonNull<NSArray<UNNotification>>| {
        let notifications = unsafe { notifications.as_ref() };
        let count = notifications.len();
        log::info!(
            "[MessengerX][Notification][DIAG] macOS delivered notifications ({context}): count={count}"
        );
        eprintln!("[MessengerX][NotifyHelper][DIAG] delivered ({context}): count={count}");
        for (i, notification) in notifications.to_vec().iter().enumerate().take(5) {
            let id = notification.request().identifier().to_string();
            let title = notification.request().content().title().to_string();
            log::info!(
                "[MessengerX][Notification][DIAG]   delivered[{i}] id={id:?} title={title:?}"
            );
            eprintln!(
                "[MessengerX][NotifyHelper][DIAG]   delivered[{i}] id={id:?} title={title:?}"
            );
        }
    });
    center.getDeliveredNotificationsWithCompletionHandler(&handler);
}

#[cfg(target_os = "macos")]
fn log_macos_pending_count(center: &UNUserNotificationCenter, context: &'static str) {
    let handler = RcBlock::new(move |requests: NonNull<NSArray<UNNotificationRequest>>| {
        let requests = unsafe { requests.as_ref() };
        let count = requests.len();
        log::info!(
                "[MessengerX][Notification][DIAG] macOS pending notification requests ({context}): count={count}"
            );
        eprintln!("[MessengerX][NotifyHelper][DIAG] pending ({context}): count={count}");
        for (i, request) in requests.to_vec().iter().enumerate().take(5) {
            let id = request.identifier().to_string();
            let title = request.content().title().to_string();
            log::info!("[MessengerX][Notification][DIAG]   pending[{i}] id={id:?} title={title:?}");
            eprintln!("[MessengerX][NotifyHelper][DIAG]   pending[{i}] id={id:?} title={title:?}");
        }
    });
    center.getPendingNotificationRequestsWithCompletionHandler(&handler);
}

/// Spawn a background thread that checks, after ~2 seconds, whether the
/// notification with `identifier` appears in the delivered list.
///
/// Designed for the CLI `--notify` helper context where the parent process
/// captures stderr; uses both `eprintln!` and `log::info!` so both channels
/// receive the result.
#[cfg(target_os = "macos")]
fn schedule_macos_helper_delivery_check(identifier: String) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let expected = identifier.clone();
        let handler = RcBlock::new(move |notifications: NonNull<NSArray<UNNotification>>| {
            let notifications = unsafe { notifications.as_ref() };
            let count = notifications.len();
            let found = notifications
                .to_vec()
                .iter()
                .any(|n| n.request().identifier().to_string() == expected);
            if found {
                log::info!(
                    "[MessengerX][NotifyHelper][DIAG] delivery-check: identifier={expected:?} FOUND in delivered (count={count})"
                );
                eprintln!(
                    "[MessengerX][NotifyHelper][DIAG] delivery-check: identifier={expected:?} FOUND in delivered (count={count})"
                );
            } else {
                log::warn!(
                    "[MessengerX][NotifyHelper][DIAG] delivery-check: identifier={expected:?} NOT FOUND in delivered after 2s (count={count})"
                );
                eprintln!(
                    "[MessengerX][NotifyHelper][DIAG] delivery-check: identifier={expected:?} NOT FOUND in delivered after 2s (count={count})"
                );
            }
        });
        center.getDeliveredNotificationsWithCompletionHandler(&handler);
    });
}

#[cfg(target_os = "macos")]
fn schedule_macos_delivery_check(identifier: String, title: String, body: String, silent: bool) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let expected_identifier = identifier.clone();
        let fallback_title = title.clone();
        let fallback_body = body.clone();
        let handler = RcBlock::new(move |notifications: NonNull<NSArray<UNNotification>>| {
            let notifications = unsafe { notifications.as_ref() };
            let delivered_count = notifications.len();
            let mut found = false;
            for notification in notifications.to_vec() {
                let delivered_identifier = notification.request().identifier().to_string();
                if delivered_identifier == expected_identifier {
                    found = true;
                    break;
                }
            }
            if found {
                log::info!(
                    "[MessengerX][Notification][DIAG] macOS delivered notification present: \
                     id={expected_identifier} delivered_count={delivered_count}"
                );
            } else {
                log::warn!(
                    "[MessengerX][Notification][DIAG] macOS notification was accepted by \
                     UNUserNotificationCenter but is not present in delivered notifications \
                     after 2s: id={expected_identifier} delivered_count={delivered_count}; \
                     falling back to osascript"
                );
                if let Err(e) = show_via_osascript_fallback(
                    &fallback_title,
                    &fallback_body,
                    silent,
                    "unusernotificationcenter-not-delivered",
                ) {
                    log::warn!(
                        "[MessengerX][Notification][DIAG] osascript delivery fallback failed: {e}"
                    );
                }
            }
        });
        center.getDeliveredNotificationsWithCompletionHandler(&handler);
    });
}

#[cfg(target_os = "macos")]
fn initialize_macos_notification_center() -> Result<(), String> {
    // `UNUserNotificationCenter::currentNotificationCenter()` requires a valid
    // macOS `.app` bundle context.  When the binary runs directly from
    // `target/debug/` (i.e. `tauri dev` / `cargo run`) it crashes with
    // NSInternalInconsistencyException because `bundleProxyForCurrentProcess`
    // is nil.  Dev-mode notifications are handled via osascript instead.
    if !is_running_in_macos_app_bundle() {
        log::info!(
            "[MessengerX][Notification] Skipping UNUserNotificationCenter init: \
             not running inside .app bundle (dev mode — osascript fallback will be used)"
        );
        return Ok(());
    }

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
    log_macos_notification_settings(&center, "after-authorization-request");

    Ok(())
}

#[cfg(target_os = "macos")]
fn show_via_user_notifications(
    title: &str,
    body: &str,
    tag: &str,
    silent: bool,
) -> Result<(), String> {
    // UNUserNotificationCenter crashes outside a proper .app bundle (dev mode).
    // Strategy (dev mode, outside .app):
    //   1. Try to delegate to the debug .app bundle binary via --notify subprocess.
    //      The subprocess runs inside the bundle context and can call
    //      UNUserNotificationCenter directly; it does NOT recurse back here.
    //   2. If the bundle binary is missing or fails, fall back to osascript.
    // Release .app builds continue through to UNUserNotificationCenter below.
    if !is_running_in_macos_app_bundle() {
        match try_delegate_to_app_bundle(title, body, silent) {
            Ok(()) => return Ok(()),
            Err(e) => {
                log::info!(
                    "[MessengerX][Notification] bundle delegation unavailable ({e}); \
                     falling back to osascript"
                );
            }
        }
        return show_via_osascript_fallback(title, body, silent, "dev-mode-outside-app-bundle");
    }

    log::info!(
        "[MessengerX][Notification] Trying macOS UNUserNotificationCenter: title={title:?} body_len={} tag={tag:?} silent={silent}",
        body.chars().count()
    );

    initialize()?;

    let center = UNUserNotificationCenter::currentNotificationCenter();
    log_macos_notification_settings(&center, "before-enqueue");

    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));

    if !silent {
        let sound = UNNotificationSound::defaultSound();
        content.setSound(Some(&sound));
        log::info!(
            "[MessengerX][AudioRust] UNUserNotificationCenter requesting OS sound: \
             UNNotificationSound.defaultSound title={title:?}"
        );
    }

    let identifier = NSString::from_str(&build_macos_notification_identifier(tag));
    let request =
        UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None);
    let enqueue_identifier = identifier.to_string();
    let delivery_check_identifier = enqueue_identifier.clone();
    let delivery_check_title = title.to_owned();
    let delivery_check_body = body.to_owned();
    let enqueue_handler = RcBlock::new(move |error: *mut NSError| {
        if let Some(error) = format_nserror(error) {
            log::warn!(
                "[MessengerX][Notification] Failed to enqueue macOS notification {enqueue_identifier}: {error}"
            );
        } else {
            log::info!(
                "[MessengerX][Notification] macOS notification enqueued: {enqueue_identifier}"
            );
            schedule_macos_delivery_check(
                delivery_check_identifier.clone(),
                delivery_check_title.clone(),
                delivery_check_body.clone(),
                silent,
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
    log::info!(
        "[MessengerX][Notification] Trying tauri-plugin-notification: title={title:?} body_len={} tag={tag:?} silent={silent}",
        body.chars().count()
    );

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
        let sound = default_sound();
        log::info!(
            "[MessengerX][Notification][Sound] tauri-plugin-notification sound={sound:?} \
             (Phase A diag — Win11 H4: plugin .sound() may not produce <audio> in toast XML)"
        );
        log::info!(
            "[MessengerX][AudioRust] tauri-plugin requesting OS sound: \
             sound={sound:?} title={title:?}"
        );
        builder = builder.sound(sound);
    } else {
        log::info!("[MessengerX][Notification][Sound] silent=true — no sound requested");
        builder = builder.silent();
    }

    match builder.show() {
        Ok(()) => {
            log::info!(
                "[MessengerX][Notification] tauri-plugin-notification delivered successfully"
            );
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            log::warn!("[MessengerX][Notification] tauri-plugin-notification failed: {msg}");
            Err(msg)
        }
    }
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

    log::info!(
        "[MessengerX][Notification] Trying notify-send: title={title:?} body_len={} \
         silent={silent} (LD_LIBRARY_PATH+LD_PRELOAD stripped for AppImage compat)",
        body.chars().count()
    );

    let mut cmd = Command::new("notify-send");

    // When running as an AppImage, the launcher injects LD_LIBRARY_PATH (and
    // sometimes LD_PRELOAD) pointing at bundled (older) GLib inside the image.
    // The system `notify-send` binary resolves libnotify.so.4 from the system
    // lib path, but libnotify.so.4 in turn requires symbols (e.g.
    // `g_once_init_leave_pointer`) from the *system* GLib.  With AppImage's
    // LD_LIBRARY_PATH active the dynamic linker picks the older bundled GLib
    // instead → symbol lookup error → exit code 127 → no notification banner.
    //
    // Fix: strip those variables from the child environment so the system
    // binary uses only system libraries.  This is safe because `notify-send`
    // is a standalone tool with no need for any AppImage-internal libraries.
    cmd.env_remove("LD_LIBRARY_PATH");
    cmd.env_remove("LD_PRELOAD");

    cmd.arg("--app-name=Messenger X")
        .arg("--urgency=normal")
        .arg("--expire-time=5000")
        .arg("--icon=message-im");

    // Request sound playback via libcanberra hint when not silenced.
    if !silent {
        cmd.arg("--hint=string:sound-name:message-new-instant");
        log::info!(
            "[MessengerX][AudioRust] notify-send requesting OS sound: \
             sound-name=message-new-instant (libcanberra hint) title={title:?}"
        );
    }

    cmd.arg(title).arg(body);

    let output = cmd
        .output()
        .map_err(|e| format!("failed to spawn notify-send: {e}"))?;

    let exit_code = output
        .status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "unknown".to_owned());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout_t = stdout.trim();
    let stderr_t = stderr.trim();

    if output.status.success() {
        log::info!(
            "[MessengerX][Notification] notify-send exited successfully: \
             exit_code={exit_code} stdout={stdout_t:?} stderr={stderr_t:?}"
        );
        Ok(())
    } else {
        Err(format!(
            "notify-send exited with non-zero status: exit_code={exit_code} \
             stdout={stdout_t:?} stderr={stderr_t:?}"
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
    // Always use a unique nanosecond timestamp so every notification arrives as
    // a fresh banner rather than silently replacing one already in Notification
    // Center.  A stable identifier caused macOS to re-use the existing
    // delivered record, suppressing the sound/banner after the first delivery.
    static LAST_ID_NANOS: AtomicU64 = AtomicU64::new(0);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let ts = loop {
        let prev = LAST_ID_NANOS.load(Ordering::Relaxed);
        let candidate = now.max(prev.saturating_add(1));
        if LAST_ID_NANOS
            .compare_exchange(prev, candidate, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            break candidate;
        }
    };
    if !tag.is_empty() {
        format!("messengerx-{tag}-{ts}")
    } else {
        format!("messengerx-message-{ts}")
    }
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
        let tagged1 = build_macos_notification_identifier("thread-123");
        let tagged2 = build_macos_notification_identifier("thread-123");
        let untagged = build_macos_notification_identifier("");

        // Tagged notifications always start with the tag-scoped prefix.
        assert!(
            tagged1.starts_with("messengerx-thread-123-"),
            "expected 'messengerx-thread-123-' prefix, got: {tagged1}"
        );
        // Each call produces a unique identifier (fresh banner, not a replacement).
        assert_ne!(
            tagged1, tagged2,
            "tagged identifiers should be unique across calls"
        );
        // Untagged notifications still get a unique timestamp suffix.
        assert!(untagged.starts_with("messengerx-message-"));
        assert_ne!(tagged1, untagged);
    }

    /// Verify that `is_running_in_macos_app_bundle` returns a bool without
    /// panicking (value depends on test runner environment, not asserted).
    #[cfg(target_os = "macos")]
    #[test]
    fn test_bundle_guard_does_not_panic() {
        let _ = is_running_in_macos_app_bundle();
    }

    /// Verify that the osascript `Command` for the dev fallback is assembled
    /// correctly — does NOT spawn the binary, only inspects debug output.
    #[cfg(target_os = "macos")]
    #[test]
    fn test_osascript_dev_fallback_args_assembled_without_panic() {
        use std::process::Command;

        let title = "Test \"title\"";
        let body = "Test body\\line\nnext";

        // Non-silent variant: sound is omitted from the dev fallback for reliability.
        let script_sound = build_osascript_notification_script(title, body, false);
        let mut cmd_sound = Command::new("/usr/bin/osascript");
        cmd_sound.arg("-e").arg(&script_sound);
        let debug_sound = format!("{cmd_sound:?}");
        assert!(
            debug_sound.contains("osascript"),
            "command should reference osascript"
        );
        assert!(
            debug_sound.contains("display notification"),
            "should contain AppleScript verb"
        );
        assert!(
            !debug_sound.contains("sound name"),
            "dev fallback should omit sound name for reliability"
        );
        assert!(
            script_sound.contains("\\\"title\\\""),
            "quotes should be escaped"
        );
        assert!(
            script_sound.contains("body\\\\line next"),
            "backslash/newline should be escaped/sanitized"
        );

        // Silent variant must also NOT include `sound name`.
        let script_silent = build_osascript_notification_script(title, body, true);
        let mut cmd_silent = Command::new("/usr/bin/osascript");
        cmd_silent.arg("-e").arg(&script_silent);
        let debug_silent = format!("{cmd_silent:?}");
        assert!(
            !debug_silent.contains("sound name"),
            "silent should omit sound name"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_read_bundle_short_version_from_xml_plist() {
        let unique = format!(
            "messengerx-test-{}-{}.plist",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::write(
            &path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>Messenger X</string>
  <key>CFBundleShortVersionString</key>
  <string>1.3.17</string>
</dict>
</plist>"#,
        )
        .expect("write temp plist");

        let version = read_bundle_short_version(&path).expect("read version");
        let _ = std::fs::remove_file(&path);
        assert_eq!(version, "1.3.17");
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
