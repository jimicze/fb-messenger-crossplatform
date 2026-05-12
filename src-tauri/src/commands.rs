//! Tauri IPC commands.
//! Defines all commands callable from the frontend via `window.__TAURI__.core.invoke()`.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
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
    /// Last visited Messenger thread URL (`/t/...` or `/e2ee/t/...`).
    /// Used as the next startup URL to skip Messenger's slow root-page redirect.
    pub last_messenger_url: Option<String>,
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
            last_messenger_url: None,
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
pub(crate) static LAST_UNREAD_COUNT: AtomicU32 = AtomicU32::new(u32::MAX);

/// Unix-second timestamp of the last window-restore-from-minimized event (Linux only).
/// Written by the Linux visibility poll thread; read in `update_unread_count_core`
/// for the `came_from_background` gate.  Zero means "never restored".
pub(crate) static LAST_RESTORED_FROM_MINIMIZED_SECS: AtomicU64 = AtomicU64::new(0);

/// Unix-second timestamp of the last "empty activity_sig with count > 0" warning.
/// Used to throttle that diagnostic to at most once every
/// [`EMPTY_SIG_WARN_THROTTLE_SECS`] seconds so it does not flood the log.
static LAST_EMPTY_SIG_WARN_SECS: AtomicU32 = AtomicU32::new(0);

/// Minimum seconds between repeated "empty activity_sig while count > 0" warnings.
const EMPTY_SIG_WARN_THROTTLE_SECS: u64 = 30;

/// Notification dedupe state.
///
/// Messenger can transiently oscillate the title between `(1) Messenger` and
/// `Messenger` every few seconds while the same unread message is pending.  A
/// flat "reset on count=0" guard cannot distinguish that oscillation from a real
/// read-all event, so we keep a small state machine instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotifState {
    /// No unread notification is currently armed.
    Idle,
    /// We already fired a notification for this count/signature.
    Notified {
        /// Last count for which a notification was fired.
        count: u32,
        /// Last JS activity signature for which a notification was fired.
        sig: String,
        /// Unix timestamp (seconds) when the notification fired.
        fired_at_secs: u64,
        /// Whether the typing-indicator re-arm has already fired once for this
        /// `Notified` entry.
        ///
        /// The re-arm path (`typing-indicator-rearm`) is allowed to fire at
        /// most **once** per `Notified` entry.  Without this guard, the cycle
        /// `typing(count=0) → message(count=1)` repeats indefinitely: each
        /// re-arm creates a fresh `Notified{fired_at=now}`, which then enters
        /// `ZeroPending{prev_fired_at=now}`, and after `TYPING_REARM_SECS`
        /// have elapsed the re-arm fires again — an infinite notification spam.
        ///
        /// Reset to `false` when a genuine `count_increased` or `sig_changed`
        /// notification fires, indicating a truly new message (not a re-arm).
        typing_rearm_exhausted: bool,
    },
    /// Count just dropped to 0 after a notification.  We wait for zero to be
    /// sustained before treating it as a real read-all reset.
    ZeroPending {
        /// Count from the previous `Notified` state.
        prev_count: u32,
        /// Activity signature from the previous `Notified` state.
        prev_sig: String,
        /// Original notification fire time; retained for oscillation cooldown.
        prev_fired_at_secs: u64,
        /// When count first dropped to zero.
        zero_since_secs: u64,
        /// Whether this zero was triggered by a typing-indicator title
        /// (title lacks a `(N)` prefix and the `| Messenger` suffix).
        ///
        /// When `true`, a count return within `TYPING_REARM_SECS` may
        /// re-fire so the user is notified of the message that followed
        /// the typing indicator.
        zero_from_typing: bool,
        /// Carried forward from `Notified::typing_rearm_exhausted`.
        ///
        /// When `true`, the typing-indicator re-arm is blocked even if
        /// `zero_from_typing` is `true` and enough time has elapsed.  This
        /// prevents the infinite-spam loop described in
        /// `Notified::typing_rearm_exhausted`.
        prev_typing_rearm_exhausted: bool,
    },
}

/// Current notification dedupe state.
/// `pub(crate)` so the window-focus handler can reset it to `Idle`.
pub(crate) static NOTIF_STATE: Mutex<NotifState> = Mutex::new(NotifState::Idle);

/// Seconds count must remain zero before we treat it as a real read-all event.
/// Must be longer than Messenger's observed `(1) ↔ 0` oscillation interval (~3s).
const ZERO_SUSTAIN_SECS: u64 = 7;

/// Minimum seconds that must elapse between two `sig_changed` notifications
/// for the **same unread count**.  This prevents JS thread-mutation sequences
/// in `activitySig` from producing rapid-fire banners.
///
/// `count_increased` always bypasses this floor so genuinely new messages
/// are never delayed.  `ZeroPending` sig-changed also respects this floor
/// because the previous fire timestamp is carried over into that state.
const MIN_SIG_CHANGE_NOTIFY_SECS: u64 = 3;

/// Minimum seconds after the previous notification before a typing-indicator
/// zero-bounce may re-arm and fire a new banner.
///
/// Messenger momentarily sets the title to a locale-specific typing string
/// (e.g. "Alice is typing…", "Jouda píše!") — count=0 — then restores the
/// `(N) SENDER | Messenger` title when the message arrives.  Without re-arming
/// the user would miss the new-message notification for the same count because
/// `zero-bounce-oscillation-suppressed` blocks it.
///
/// The floor prevents rapid oscillation spam: if the typing indicator fires and
/// count bounces back before `TYPING_REARM_SECS` have elapsed since the last
/// notification, no new banner is shown.
const TYPING_REARM_SECS: u64 = 5;

/// Seconds after the last notification before the state machine re-arms
/// unconditionally, even when the unread count has not changed.
///
/// Use case: user ignores a notification (doesn't read it); after this many
/// seconds the next title-change event — regardless of count — will fire a
/// new banner.  The fired timestamp is refreshed on each re-arm so the cadence
/// is at most once per `NOTIF_REARM_SECS`, not once per event.
const NOTIF_REARM_SECS: u64 = 60;

/// Grace period after a window restore-from-minimized during which a notification
/// is fired even if `is_focused()` returns true.  Cinnamon/Muffin focuses the
/// window instantly on un-iconify, so without this gate the user would see
/// `idle-focused-skip` and miss the pending notification.
const RESTORE_GRACE_SECS: u64 = 3;

/// Returns the current Unix time in seconds (best-effort; 0 on error).
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Decision returned by the notification state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NotificationDecision {
    /// Whether the caller should dispatch a native notification.
    should_fire: bool,
    /// Whether the current `count=0` should be treated as a confirmed read-all
    /// event for badge/tray clearing.
    clear_badge: bool,
    /// Human-readable reason for diagnostics.
    reason: &'static str,
    /// Previous count used for log output.
    previous_count: u32,
    /// Whether the count increased relative to the prior notified state.
    count_increased: bool,
    /// Whether the JS activity signature changed.
    sig_changed: bool,
    /// Seconds since the last notification fire, if known.
    elapsed_since_fire: Option<u64>,
}

impl NotificationDecision {
    fn idle(reason: &'static str, clear_badge: bool) -> Self {
        Self {
            should_fire: false,
            clear_badge,
            reason,
            previous_count: 0,
            count_increased: false,
            sig_changed: false,
            elapsed_since_fire: None,
        }
    }
}

/// Updates the notification dedupe state and returns whether to fire.
fn decide_notification(
    state: &mut NotifState,
    count: u32,
    activity_sig: &str,
    is_focused: bool,
    notifications_enabled: bool,
    is_typing_indicator: bool,
    now: u64,
) -> NotificationDecision {
    match state.clone() {
        NotifState::Idle => {
            if count == 0 {
                NotificationDecision::idle("idle-count-zero", true)
            } else if notifications_enabled && !is_focused {
                *state = NotifState::Notified {
                    count,
                    sig: activity_sig.to_owned(),
                    fired_at_secs: now,
                    typing_rearm_exhausted: false,
                };
                NotificationDecision {
                    should_fire: true,
                    clear_badge: false,
                    reason: "idle-count-positive",
                    previous_count: 0,
                    count_increased: true,
                    sig_changed: !activity_sig.is_empty(),
                    elapsed_since_fire: None,
                }
            } else {
                NotificationDecision::idle(
                    if is_focused {
                        "idle-focused-skip"
                    } else {
                        "idle-disabled-skip"
                    },
                    false,
                )
            }
        }
        NotifState::Notified {
            count: prev_count,
            sig: prev_sig,
            fired_at_secs,
            typing_rearm_exhausted: prev_exhausted,
        } => {
            if count == 0 {
                // If the window is focused the user is actively looking at the
                // app — treat count=0 as an immediate confirmed read-all and
                // skip the ZeroPending debounce.  The 7 s debounce exists only
                // to distinguish genuine reads from the transient title-flicker
                // Messenger produces when the user is NOT watching.
                if is_focused {
                    *state = NotifState::Idle;
                    return NotificationDecision {
                        should_fire: false,
                        clear_badge: true,
                        reason: "focused-read-all",
                        previous_count: prev_count,
                        count_increased: false,
                        sig_changed: false,
                        elapsed_since_fire: Some(now.saturating_sub(fired_at_secs)),
                    };
                }
                *state = NotifState::ZeroPending {
                    prev_count,
                    prev_sig,
                    prev_fired_at_secs: fired_at_secs,
                    zero_since_secs: now,
                    zero_from_typing: is_typing_indicator,
                    prev_typing_rearm_exhausted: prev_exhausted,
                };
                NotificationDecision {
                    should_fire: false,
                    clear_badge: false,
                    reason: "zero-pending-start",
                    previous_count: prev_count,
                    count_increased: false,
                    sig_changed: false,
                    elapsed_since_fire: Some(now.saturating_sub(fired_at_secs)),
                }
            } else {
                let count_increased = count > prev_count;
                // Guard against empty→nonempty sig transitions.  When the
                // current Notified state has sig="" (stored after a typing
                // indicator dropped activity_sig), the return of any real
                // signature reads as "changed" → duplicate notification.
                // Applies to both individual and group conversations.
                let sig_changed = !activity_sig.is_empty()
                    && !prev_sig.is_empty()
                    && activity_sig != prev_sig;
                let elapsed = now.saturating_sub(fired_at_secs);
                // sig_changed fires are rate-limited to prevent JS thread-mutation
                // sequences from producing rapid-fire banners.  count_increased always
                // bypasses this floor so genuine new messages are never delayed.
                let sig_under_floor =
                    sig_changed && !count_increased && elapsed < MIN_SIG_CHANGE_NOTIFY_SECS;
                // Time-based re-arm: if the user has been ignoring the notification
                // for NOTIF_REARM_SECS without reading (window never became focused
                // while count was 0), fire once more so the message isn't silently
                // missed.  The fired timestamp is updated on each re-arm, bounding
                // the repeat rate to at most once per NOTIF_REARM_SECS (anti-spam).
                let time_rearm = elapsed >= NOTIF_REARM_SECS;
                let should_fire = notifications_enabled
                    && !is_focused
                    && (count_increased || (sig_changed && !sig_under_floor) || time_rearm);
                let reason = if count_increased {
                    "count-increased"
                } else if time_rearm {
                    "time-rearm-60s"
                } else if sig_changed && sig_under_floor {
                    "sig-changed-floor-suppressed"
                } else if sig_changed {
                    "sig-changed"
                } else {
                    "same-activity-suppressed"
                };
                if should_fire {
                    // Preserve the typing_rearm_exhausted flag when the
                    // fire reason is sig-changed-only (oscillation), not a
                    // count increase.  Only a genuine count_increased or
                    // time_rearm resets it to false.  This breaks the
                    // sig_changed → typing_rearm → sig_changed infinite loop.
                    let new_exhausted = if count_increased || time_rearm {
                        false // true new message or fresh cycle
                    } else {
                        prev_exhausted // sig-only fire — carry forward
                    };
                    *state = NotifState::Notified {
                        count,
                        sig: activity_sig.to_owned(),
                        fired_at_secs: now,
                        typing_rearm_exhausted: new_exhausted,
                    };
                } else if prev_sig.is_empty() && !activity_sig.is_empty()
                    && count == prev_count
                {
                    // Empty→nonempty transition that we suppressed — store
                    // the arriving sig as a baseline so the next genuine
                    // change can be detected.
                    if let NotifState::Notified { sig, .. } = state {
                        *sig = activity_sig.to_owned();
                    }
                }
                NotificationDecision {
                    should_fire,
                    clear_badge: false,
                    reason,
                    previous_count: prev_count,
                    count_increased,
                    sig_changed,
                    elapsed_since_fire: Some(elapsed),
                }
            }
        }
        NotifState::ZeroPending {
            prev_count,
            prev_sig,
            prev_fired_at_secs,
            zero_since_secs,
            zero_from_typing,
            prev_typing_rearm_exhausted,
        } => {
            if count == 0 {
                // Same focused-read-all shortcut as in the Notified arm: if the
                // window is focused the user is watching — reset immediately.
                if is_focused {
                    *state = NotifState::Idle;
                    return NotificationDecision {
                        should_fire: false,
                        clear_badge: true,
                        reason: "focused-read-all",
                        previous_count: prev_count,
                        count_increased: false,
                        sig_changed: false,
                        elapsed_since_fire: Some(now.saturating_sub(prev_fired_at_secs)),
                    };
                }
                let zero_elapsed = now.saturating_sub(zero_since_secs);
                if zero_elapsed >= ZERO_SUSTAIN_SECS {
                    *state = NotifState::Idle;
                    NotificationDecision {
                        should_fire: false,
                        clear_badge: true,
                        reason: "zero-sustained-read-all",
                        previous_count: prev_count,
                        count_increased: false,
                        sig_changed: false,
                        elapsed_since_fire: Some(now.saturating_sub(prev_fired_at_secs)),
                    }
                } else {
                    NotificationDecision {
                        should_fire: false,
                        clear_badge: false,
                        reason: "zero-pending-wait",
                        previous_count: prev_count,
                        count_increased: false,
                        sig_changed: false,
                        elapsed_since_fire: Some(now.saturating_sub(prev_fired_at_secs)),
                    }
                }
            } else {
                let count_increased = count > prev_count;
                // sig_changed requires BOTH the current and previous sig to be
                // non-empty.  When prev_sig is "" (stored during a typing-indicator
                // transition), the return of any real signature would otherwise
                // register as "changed" → duplicate notification for the same
                // message.  This was especially visible for group chats where the
                // title oscillates between "(N) Sender | Messenger" (open tab)
                // and "Sender píše skupině GroupName" (typing indicator).
                let sig_changed = !activity_sig.is_empty()
                    && !prev_sig.is_empty()
                    && activity_sig != prev_sig;
                let elapsed = now.saturating_sub(prev_fired_at_secs);
                // Same floor as the Notified arm: sig-only fires are rate-limited.
                let sig_under_floor =
                    sig_changed && !count_increased && elapsed < MIN_SIG_CHANGE_NOTIFY_SECS;
                // Re-arm: the previous zero came from a typing indicator, enough
                // time has elapsed, and the rearm has NOT already fired once for
                // this Notified entry.  The `prev_typing_rearm_exhausted` guard
                // breaks the infinite loop: rearm → Notified{exhausted=true} →
                // ZeroPending{prev_exhausted=true} → rearm blocked.
                let typing_rearm = zero_from_typing
                    && elapsed >= TYPING_REARM_SECS
                    && !prev_typing_rearm_exhausted;
                // Time-based re-arm: same semantics as in the Notified arm — if
                // NOTIF_REARM_SECS have elapsed since the last fire and the user
                // still hasn't read the message, fire once more.
                let time_rearm = elapsed >= NOTIF_REARM_SECS;
                let should_fire = notifications_enabled
                    && !is_focused
                    && (count_increased
                        || (sig_changed && !sig_under_floor)
                        || typing_rearm
                        || time_rearm);
                let reason = if count_increased {
                    "zero-bounce-count-increased"
                } else if sig_changed && sig_under_floor {
                    "zero-bounce-sig-floor-suppressed"
                } else if sig_changed {
                    "zero-bounce-sig-changed"
                } else if typing_rearm {
                    "typing-indicator-rearm"
                } else if time_rearm {
                    "time-rearm-60s"
                } else {
                    "zero-bounce-oscillation-suppressed"
                };
                // Compute whether the new Notified entry should block future rearms:
                //   - genuine fire (count/sig/time): reset to false — new message,
                //     rearm allowed again from a fresh baseline.
                //   - typing_rearm fire:             set to true  — consumed the one
                //     allowed rearm for this Notified entry.
                //   - no fire (suppressed):          carry forward prev so exhausted
                //     flag survives the oscillation cycle.
                let new_typing_rearm_exhausted = if should_fire {
                    typing_rearm // true only when the rearm path fired
                } else {
                    prev_typing_rearm_exhausted
                };
                *state = NotifState::Notified {
                    count,
                    sig: activity_sig.to_owned(),
                    fired_at_secs: if should_fire { now } else { prev_fired_at_secs },
                    typing_rearm_exhausted: new_typing_rearm_exhausted,
                };
                NotificationDecision {
                    should_fire,
                    clear_badge: false,
                    reason,
                    previous_count: prev_count,
                    count_increased,
                    sig_changed,
                    elapsed_since_fire: Some(elapsed),
                }
            }
        }
    }
}

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
    log::info!("[MessengerX][Notification] effective_silent={effective_silent}");
    let result = crate::services::notification::show_notification(
        &app,
        &title,
        &body,
        &tag,
        effective_silent,
        "ipc-send-notification",
    );
    match &result {
        Ok(()) => log::info!("[MessengerX][Notification] send_notification finished successfully"),
        Err(e) => log::warn!("[MessengerX][Notification] send_notification failed: {e}"),
    }
    result
}

/// Update the unread-message count badge / tray tooltip.
///
/// ## Parameters
/// - `count`: current unread count extracted from the document title `(N) Messenger`.
/// - `sender`: best-effort sender name from the conversation list DOM (may be empty).
/// - `activity_sig`: opaque signature string produced by the JS activity tracker.
///   Non-empty when `count > 0` and a baseline DOM snapshot has been established.
///   Changes when a new confirmed unread conversation candidate appears in the
///   DOM snapshot while `count > 0`.  High-frequency UI-only mutations (presence
///   dots, typing indicators, read receipts) do NOT change this signature because
///   the snapshot only captures verified unread conversation link candidates.
///   Empty string when `count == 0` (JS resets activity state before sending).
///
/// ## Badge / tray flicker guard
/// The `LAST_UNREAD_COUNT` atomic is used purely to avoid redundant tray/badge updates.
/// It is updated whenever the count changes.
///
/// ## Notification decision
/// Notification dedupe is handled by `NOTIF_STATE`, a small state machine that
/// distinguishes real read-all (`count=0` sustained for `ZERO_SUSTAIN_SECS`) from
/// Messenger's transient `(1) Messenger ↔ Messenger` title oscillation.
///
/// ## On count=0
/// `count=0` first enters `ZeroPending` and clears badge/tray only after the zero
/// state is sustained.  This avoids re-firing every few seconds on title
/// oscillation while still clearing the badge after an actual read-all.
///
/// Called from JS via IPC — `is_typing_indicator` is always `false` here because
/// JS has no reliable way to detect the typing indicator title format.  Use
/// [`update_unread_count_from_title`] for the Rust-side title-change handler
/// which has full title context.
#[tauri::command]
pub fn update_unread_count(
    count: u32,
    sender: String,
    activity_sig: String,
    app: AppHandle,
) -> Result<(), String> {
    update_unread_count_core(count, sender, activity_sig, false, app)
}

/// Variant of [`update_unread_count`] called directly from the Rust-side
/// `on_document_title_changed` handler.
///
/// Unlike the IPC command, this call-site knows:
/// - `sender` — parsed from `(N) SENDER | Messenger` title format.
/// - `is_typing_indicator` — whether the title was a locale-specific typing
///   indicator (e.g. "Alice is typing…", "Jouda píše!") rather than a
///   real unread-count title.
///
/// `activity_sig` is always empty here because there is no DOM access from Rust.
pub(crate) fn update_unread_count_from_title(
    count: u32,
    sender: String,
    is_typing_indicator: bool,
    app: AppHandle,
) -> Result<(), String> {
    update_unread_count_core(count, sender, String::new(), is_typing_indicator, app)
}

/// Shared implementation for [`update_unread_count`] and
/// [`update_unread_count_from_title`].
fn update_unread_count_core(
    count: u32,
    sender: String,
    activity_sig: String,
    is_typing_indicator: bool,
    app: AppHandle,
) -> Result<(), String> {
    // ------------------------------------------------------------------
    // 1. Badge / tray flicker guard — update count and early-exit tray/badge
    //    work only when the count actually changed.
    // ------------------------------------------------------------------
    let old_count = LAST_UNREAD_COUNT.load(Ordering::SeqCst);
    let count_changed = old_count != count;
    if count_changed {
        LAST_UNREAD_COUNT.store(count, Ordering::SeqCst);
    }

    // ------------------------------------------------------------------
    // 1b. Startup-baseline guard.
    //
    // On every process start (including crash-induced restarts) NOTIF_STATE
    // resets to `Idle` and LAST_UNREAD_COUNT resets to `u32::MAX`.  The
    // first title update that arrives with `count > 0` would therefore take
    // the `idle-count-positive` path and fire a spurious notification for a
    // message the user already knows about.
    //
    // Guard: when `old_count == u32::MAX` (the sentinel written at static
    // init — meaning "this process has never observed a count before") AND
    // `count > 0`, silently establish a baseline in NOTIF_STATE without
    // dispatching a notification.  This mirrors what would happen if the
    // user had already been notified: state becomes `Notified` with the
    // current count/sig and `fired_at_secs = now` so the normal
    // sig_changed / time_rearm paths work correctly from here on.
    //
    // This applies to all callers (IPC JS path + title-hook path) because
    // both converge here.
    // ------------------------------------------------------------------
    if old_count == u32::MAX && count > 0 {
        let now = now_secs();
        let mut state = NOTIF_STATE
            .lock()
            .map_err(|e| format!("notification state lock poisoned: {e}"))?;
        if matches!(*state, NotifState::Idle) {
            *state = NotifState::Notified {
                count,
                sig: activity_sig.clone(),
                fired_at_secs: now,
                typing_rearm_exhausted: false,
            };
            log::info!(
                "[MessengerX][Notification][DECISION] startup-baseline: \
                 count={count} silently baselined (no notification — pre-existing unread)"
            );
        }
        // Do NOT return — badge / tray updates below still need to run.
        // The notification section will see state=Notified and suppress the fire.
    }

    // ------------------------------------------------------------------
    // 2. Notification decision — evaluated independently of badge guard.
    // ------------------------------------------------------------------
    let settings = crate::services::auth::load_settings(&app).unwrap_or_default();
    let main_window = app.get_webview_window("main");
    let raw_focused = main_window
        .as_ref()
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    let raw_minimized = main_window.as_ref().and_then(|w| w.is_minimized().ok());
    let raw_visible_api = main_window.as_ref().and_then(|w| w.is_visible().ok());
    // Phase A: log the per-call focus probe with all three signals so we can
    // diagnose whether `is_focused()` alone is the wrong gate (H2 Wayland).
    log::debug!(
        "[MessengerX][Notification][FocusProbe] focused={raw_focused} \
         minimized={raw_minimized:?} visible_api={raw_visible_api:?}"
    );
    // Fix 3: a minimized window on Cinnamon/Muffin can still report is_focused()=true;
    // treat a minimized window as not focused regardless of the focus signal.
    let is_focused = raw_focused && !raw_minimized.unwrap_or(false);
    let now = now_secs();
    // Fix 2: if the window was just restored from minimized (Linux poll thread stamps
    // LAST_RESTORED_FROM_MINIMIZED_SECS), treat it as "not focused" for the next
    // RESTORE_GRACE_SECS so the user sees the notification banner on un-iconify.
    let restore_secs = LAST_RESTORED_FROM_MINIMIZED_SECS.load(Ordering::SeqCst);
    let came_from_background =
        restore_secs > 0 && now.saturating_sub(restore_secs) < RESTORE_GRACE_SECS;
    // Fix 4: log the came_from_background gate when active.
    if came_from_background {
        log::info!(
            "[MessengerX][Notification][DECISION] came_from_background=true \
             reason=restored-from-minimized elapsed_since_restore={}s",
            now.saturating_sub(restore_secs)
        );
    }
    // When the window was just restored from minimized, fire the notification
    // regardless of current focus so the user is alerted to pending messages.
    let effective_focused = is_focused && !came_from_background;
    let (decision, state_after) = {
        let mut state = NOTIF_STATE
            .lock()
            .map_err(|e| format!("notification state lock poisoned: {e}"))?;
        let decision = decide_notification(
            &mut state,
            count,
            &activity_sig,
            effective_focused,
            settings.notifications_enabled,
            is_typing_indicator,
            now,
        );
        (decision, state.clone())
    };

    log::info!(
        "[MessengerX][Notification][DECISION] count={} old_count={} focused={} \
         came_from_background={} effective_focused={} enabled={} \
         reason={} fire={} clear_badge={} prev_count={} count_increased={} sig_changed={} \
         elapsed={:?} typing={} activity_sig={:?} state_after={:?}",
        count,
        old_count,
        is_focused,
        came_from_background,
        effective_focused,
        settings.notifications_enabled,
        decision.reason,
        decision.should_fire,
        decision.clear_badge,
        decision.previous_count,
        decision.count_increased,
        decision.sig_changed,
        decision.elapsed_since_fire,
        is_typing_indicator,
        activity_sig,
        state_after,
    );

    // ------------------------------------------------------------------
    // 2b. Diagnostic: warn when count > 0 but activity_sig is empty.
    //     This means the JS snapshot baseline has not been established yet —
    //     sig-change notifications will not fire until it is.  Throttled to
    //     at most once per EMPTY_SIG_WARN_THROTTLE_SECS to avoid log spam.
    // ------------------------------------------------------------------
    if count > 0 && activity_sig.is_empty() {
        let last_warn = u64::from(LAST_EMPTY_SIG_WARN_SECS.load(Ordering::Relaxed));
        if now.saturating_sub(last_warn) >= EMPTY_SIG_WARN_THROTTLE_SECS {
            // Store as u32; timestamps in 2024 fit comfortably in u32 until 2106.
            LAST_EMPTY_SIG_WARN_SECS.store(now as u32, Ordering::Relaxed);
            log::info!(
                "[MessengerX][Notification][DIAG] count={} but activity_sig is empty — \
                 JS snapshot baseline not yet established; sig-change path is inactive. \
                 Notifications may still fire on count-increase. \
                 (this message throttled to once/{}s)",
                count,
                EMPTY_SIG_WARN_THROTTLE_SECS,
            );
        }
    }

    if decision.should_fire {
        let effective_silent = !settings.notification_sound;
        let locale = crate::services::locale::detect_locale();
        let tr = crate::services::locale::get_translations(&locale);
        let notif_title = if !sender.trim().is_empty() {
            sender.clone()
        } else {
            tr.notification_new_message.clone()
        };
        log::info!(
            "[MessengerX][Notification] firing notification: count {} → {}; \
             reason={} count_increased={} sig_changed={} typing={}; \
             sender={:?} activity_sig={:?} silent={}",
            decision.previous_count,
            count,
            decision.reason,
            decision.count_increased,
            decision.sig_changed,
            is_typing_indicator,
            sender,
            activity_sig,
            effective_silent,
        );
        if let Err(e) = crate::services::notification_dispatcher::dispatch(
            &app,
            decision.reason,
            sender.trim(),
            count,
            &notif_title,
            effective_silent,
        ) {
            log::warn!("[MessengerX][Notification] unread-count notification failed: {e}");
        }
    }

    // ------------------------------------------------------------------
    // 3. Tray tooltip update (only when count changed to avoid flicker).
    //    Badge and tray are now cleared unconditionally on count=0 — the
    //    cooldown in the notification path handles oscillation spam instead.
    // ------------------------------------------------------------------
    if count_changed && (count > 0 || decision.clear_badge) {
        if let Some(tray) = app.tray_by_id("messengerx-tray") {
            let tooltip = if count > 0 {
                format!("Messenger X ({})", count)
            } else {
                "Messenger X".to_string()
            };
            tray.set_tooltip(Some(&tooltip))
                .map_err(|e| e.to_string())?;
        }

        // Update macOS Dock badge label — cleared unconditionally on count=0.
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
    }

    // ------------------------------------------------------------------
    // 3b. Taskbar / dock attention request (Win11 + Linux only).
    //     On macOS the Dock badge (section 3) already draws sufficient
    //     attention; on Win11/Linux we flash the taskbar button /
    //     set the window urgency hint so the user notices the new message
    //     even when the app is in the background or minimised.
    //     The platform clears the attention signal automatically once the
    //     window is focused — no explicit teardown required.
    // ------------------------------------------------------------------
    #[cfg(not(target_os = "macos"))]
    if decision.should_fire {
        if let Some(ref w) = main_window {
            if let Err(e) = w.request_user_attention(Some(tauri::UserAttentionType::Informational))
            {
                log::warn!("[MessengerX][Attention] request_user_attention failed: {e}");
            }
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
        log::warn!("[MessengerX][Visibility] get_window_focused: main window missing");
        return false;
    };
    let Ok(is_focused) = window.is_focused() else {
        log::warn!("[MessengerX][Visibility] get_window_focused: is_focused() failed");
        return false;
    };
    let Ok(is_minimized) = window.is_minimized() else {
        log::warn!("[MessengerX][Visibility] get_window_focused: is_minimized() failed");
        return false;
    };
    // Phase A diagnostic: also probe is_visible() to detect Wayland occlusion
    // mismatches (H2). On X11 / win / mac this should track is_minimized closely.
    let visible_probe = window.is_visible().ok();
    let effective_visible = is_focused && !is_minimized;

    #[cfg(target_os = "linux")]
    {
        let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "<unset>".into());
        log::info!(
            "[MessengerX][Visibility][Linux] get_window_focused -> focused={is_focused} \
             minimized={is_minimized} visible_api={visible_probe:?} \
             effective={effective_visible} session_type={session_type:?}"
        );
    }
    #[cfg(not(target_os = "linux"))]
    log::info!(
        "[MessengerX][Visibility] get_window_focused -> focused={is_focused} \
         minimized={is_minimized} visible_api={visible_probe:?} effective={effective_visible}"
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

// ── Download save-as commands ──────────────────────────────────────────

/// Opens a native "Save As" file dialog and returns the chosen path.
/// Called from JS as the first step of a user-initiated file download.
/// The dialog is async — it does NOT block the main thread (unlike
/// `blocking_save_file()` which would deadlock inside `on_download`).
#[tauri::command]
pub async fn pick_save_path(
    app: tauri::AppHandle,
    suggested_filename: String,
) -> Result<Option<String>, String> {
    use std::sync::mpsc;
    use tauri_plugin_dialog::DialogExt;

    // Pre-increment the suggested name so the dialog itself shows
    // "download (2)", "download (3)", etc. when files already exist
    // in the Downloads folder (the default save location).
    let downloads = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
    let display_name = {
        let candidate = downloads.join(&suggested_filename);
        if !candidate.exists() {
            suggested_filename.clone()
        } else {
            let (stem, ext) = suggested_filename
                .rfind('.')
                .map(|i| (&suggested_filename[..i], &suggested_filename[i..]))
                .unwrap_or((suggested_filename.as_str(), ""));
            let mut n: u32 = 2;
            loop {
                let name = format!("{stem} ({n}){ext}");
                if !downloads.join(&name).exists() {
                    break name;
                }
                n = n.saturating_add(1);
            }
        }
    };

    let (tx, rx) = mpsc::channel();

    app.dialog()
        .file()
        .set_file_name(&display_name)
        .save_file(move |file_path| {
            let result = file_path
                .and_then(|fp| fp.into_path().ok())
                .map(|p| p.to_string_lossy().to_string());
            let _ = tx.send(result);
        });

    rx.recv()
        .map_err(|e| format!("Save dialog error: {e}"))
        .map(|opt| {
            opt.map(|path_str| {
                let p = std::path::PathBuf::from(&path_str);
                if !p.exists() {
                    return path_str;
                }
                // File already exists — auto-increment the name (fallback).
                let dir = p.parent().unwrap_or_else(|| std::path::Path::new("."));
                let base = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("download");
                let (stem, ext) = base
                    .rfind('.')
                    .map(|i| (&base[..i], &base[i..]))
                    .unwrap_or((base, ""));
                let mut n: u32 = 2;
                loop {
                    let candidate = dir.join(format!("{stem} ({n}){ext}"));
                    if !candidate.exists() {
                        return candidate.to_string_lossy().to_string();
                    }
                    n = n.saturating_add(1);
                }
            })
        })
}

/// Writes binary data to the given filesystem path and sends a system
/// notification about the completed download.
///
/// Called from JS after the user has chosen a save path and the blob data
/// has been fetched via `fetch(blobUrl) → arrayBuffer()`.
#[tauri::command]
pub fn write_file_bytes(
    path: String,
    data: Vec<u8>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    std::fs::write(&path, &data).map_err(|e| format!("File write failed: {e}"))?;

    // Extract filename for the notification.
    let filename = std::path::Path::new(&path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");

    let body = format!("Saved — {filename}");
    // Spawn on background thread — in dev mode show_notification spawns
    // the debug .app bundle subprocess (~4 s), which would beachball.
    let h = app.clone();
    std::thread::spawn(move || {
        let _ = crate::services::notification::show_notification(
            &h,
            "Messenger X",
            &body,
            "download",
            true,
            "download-save-as",
        );
    });

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────
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
            last_messenger_url: Some("https://www.messenger.com/t/123/".to_string()),
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
        assert_eq!(
            deserialized.last_messenger_url.as_deref(),
            Some("https://www.messenger.com/t/123/")
        );
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
        assert_eq!(settings.last_messenger_url, None);
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
    fn test_activity_sig_change_detection() {
        // Simulate the notification-decision logic for sig_changed branch.
        // New sig format: "<count>:<seq>:<snapshot_prefix>"
        let last_sig = "1:0:Alice".to_string();
        let new_sig_same = "1:0:Alice".to_string();
        let new_sig_changed = "1:1:Alice".to_string();
        let new_sig_new_convo = "1:1:Bob".to_string();
        let empty_sig = "".to_string();

        // Same sig → no change.
        assert!(new_sig_same.is_empty() || new_sig_same == last_sig);
        // Changed seq → should trigger.
        assert!(!new_sig_changed.is_empty() && new_sig_changed != last_sig);
        // Changed snapshot (different convo) → should trigger.
        assert!(!new_sig_new_convo.is_empty() && new_sig_new_convo != last_sig);
        // Empty sig (count=0 path) → never triggers.
        assert!(empty_sig.is_empty() || empty_sig == last_sig);
    }

    #[test]
    fn test_zero_sustain_constant_exceeds_title_oscillation() {
        // Messenger oscillation observed around ~3 seconds; require longer than that.
        const _: () = {
            assert!(ZERO_SUSTAIN_SECS > 3);
            assert!(ZERO_SUSTAIN_SECS <= 30);
        };
    }

    #[test]
    fn test_notif_state_idle_positive_fires() {
        let mut state = NotifState::Idle;
        let decision = decide_notification(&mut state, 1, "", false, true, false, 100);
        assert!(decision.should_fire);
        assert_eq!(decision.reason, "idle-count-positive");
        assert_eq!(
            state,
            NotifState::Notified {
                count: 1,
                sig: String::new(),
                fired_at_secs: 100,
                typing_rearm_exhausted: false,
            }
        );
    }

    #[test]
    fn test_notif_state_suppresses_transient_zero_bounce() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };

        let zero = decide_notification(&mut state, 0, "", false, true, false, 103);
        assert!(!zero.should_fire);
        assert!(!zero.clear_badge);
        assert_eq!(zero.reason, "zero-pending-start");

        let bounce = decide_notification(&mut state, 1, "", false, true, false, 106);
        assert!(!bounce.should_fire);
        assert_eq!(bounce.reason, "zero-bounce-oscillation-suppressed");
        assert_eq!(
            state,
            NotifState::Notified {
                count: 1,
                sig: String::new(),
                fired_at_secs: 100,
                typing_rearm_exhausted: false,
            }
        );
    }

    #[test]
    fn test_notif_state_confirms_sustained_zero_as_read_all() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };

        let _ = decide_notification(&mut state, 0, "", false, true, false, 103);
        let sustained = decide_notification(&mut state, 0, "", false, true, false, 111);
        assert!(!sustained.should_fire);
        assert!(sustained.clear_badge);
        assert_eq!(sustained.reason, "zero-sustained-read-all");
        assert_eq!(state, NotifState::Idle);
    }

    #[test]
    fn test_notif_state_fires_after_sustained_read_all() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };

        let _ = decide_notification(&mut state, 0, "", false, true, false, 103);
        let _ = decide_notification(&mut state, 0, "", false, true, false, 111);
        let next = decide_notification(&mut state, 1, "", false, true, false, 112);
        assert!(next.should_fire);
        assert_eq!(next.reason, "idle-count-positive");
    }

    #[test]
    fn test_notif_state_sig_changed_bypasses_zero_pending() {
        let mut state = NotifState::ZeroPending {
            prev_count: 1,
            prev_sig: "1:0:Alice".to_string(),
            prev_fired_at_secs: 100,
            zero_since_secs: 103,
            zero_from_typing: false,
            prev_typing_rearm_exhausted: false,
        };

        let decision = decide_notification(&mut state, 1, "1:1:Alice", false, true, false, 104);
        assert!(decision.should_fire);
        assert!(decision.sig_changed);
        assert_eq!(decision.reason, "zero-bounce-sig-changed");
    }

    /// Empty prev_sig (from typing transition) → non-empty activity_sig:
    /// should NOT fire, and should update the stored sig as a baseline
    /// so future changes can be detected against the real signature.
    #[test]
    fn test_notif_state_notified_empty_prev_sig_baseline_update() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(), // empty — from typing-indicator transition
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };

        // Same count, prev_sig is empty, activity_sig arrives non-empty.
        let decision = decide_notification(
            &mut state, 1, "1:0:Alice", false, true, false, 104,
        );
        assert!(!decision.should_fire, "empty→nonempty should NOT fire");
        assert!(!decision.sig_changed);

        // State sig should be updated to the baseline.
        match &state {
            NotifState::Notified { sig, .. } => {
                assert_eq!(sig, "1:0:Alice", "sig should be updated to baseline");
            }
            _ => panic!("expected Notified state"),
        }
    }

    /// ZeroPending with empty prev_sig and non-empty activity_sig:
    /// sig_changed must be false (guarded by empty prev_sig) so the
    /// decision does NOT fire due to the sig-change path.
    #[test]
    fn test_notif_state_zero_pending_empty_prev_sig_no_fire() {
        let mut state = NotifState::ZeroPending {
            prev_count: 1,
            prev_sig: String::new(),
            prev_fired_at_secs: 100,
            zero_since_secs: 103,
            zero_from_typing: true,
            prev_typing_rearm_exhausted: false,
        };

        let decision = decide_notification(
            &mut state, 1, "1:0:Alice", false, true, false, 104,
        );
        assert!(!decision.should_fire, "empty prev_sig should NOT trigger fire");
        assert!(
            !decision.sig_changed,
            "sig_changed must be false when prev_sig is empty"
        );
    }

    #[test]
    fn test_count_increased_detection() {
        // count > last_notified_count triggers notification.
        let last: u32 = 1;
        let increased: u32 = 2;
        let same: u32 = 1;
        let decreased: u32 = 0;
        assert!(increased > last); // count increased
        assert!(same <= last); // count same
        assert!(decreased <= last); // count decreased
    }

    // -----------------------------------------------------------------------
    // Anti-spam floor tests (MIN_SIG_CHANGE_NOTIFY_SECS)
    // -----------------------------------------------------------------------

    /// sig_changed fires after the floor has elapsed (elapsed >= MIN_SIG_CHANGE_NOTIFY_SECS).
    #[test]
    fn test_sig_changed_after_floor_fires() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: "1:0:Alice".to_string(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };
        // elapsed = 100+MIN_SIG_CHANGE_NOTIFY_SECS — exactly at the boundary → allow
        let now = 100 + MIN_SIG_CHANGE_NOTIFY_SECS;
        let decision = decide_notification(&mut state, 1, "1:1:Alice", false, true, false, now);
        assert!(decision.should_fire, "sig_changed after floor should fire");
        assert_eq!(decision.reason, "sig-changed");
    }

    /// sig_changed under floor: state.sig must stay unchanged so the same
    /// change is still detectable after the floor elapses.
    #[test]
    fn test_sig_changed_under_floor_preserves_sig() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: "1:0:Alice".to_string(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };
        let decision = decide_notification(&mut state, 1, "1:1:Alice", false, true, false, 101);
        assert!(!decision.should_fire, "under-floor sig_changed should NOT fire");
        assert_eq!(decision.reason, "sig-changed-floor-suppressed");
        // Stored sig must remain the OLD value so the next poll still sees a change.
        match &state {
            NotifState::Notified { sig, .. } => {
                assert_eq!(sig, "1:0:Alice", "sig must not be overwritten while under floor");
            }
            _ => panic!("expected Notified state"),
        }
    }

    /// sig_changed fires immediately under the floor when it should be suppressed.
    #[test]
    fn test_sig_changed_under_floor_suppressed() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: "1:0:Alice".to_string(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };
        // elapsed = MIN_SIG_CHANGE_NOTIFY_SECS - 1 → below floor → suppress
        let now = 100 + MIN_SIG_CHANGE_NOTIFY_SECS - 1;
        let decision = decide_notification(&mut state, 1, "1:1:Alice", false, true, false, now);
        assert!(
            !decision.should_fire,
            "sig_changed under floor must be suppressed"
        );
        assert_eq!(decision.reason, "sig-changed-floor-suppressed");
    }

    /// count_increased bypasses the floor and fires immediately.
    #[test]
    fn test_count_increased_bypasses_floor() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: "1:0:Alice".to_string(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };
        // elapsed = 0 — well under the floor, but count increased → must fire
        let decision = decide_notification(&mut state, 2, "2:0:Alice", false, true, false, 100);
        assert!(
            decision.should_fire,
            "count_increased must bypass sig floor"
        );
        assert_eq!(decision.reason, "count-increased");
    }

    /// Verify the empty-sig throttle constant is a reasonable positive interval.
    #[test]
    fn test_empty_sig_warn_throttle_constant_is_reasonable() {
        const _: () = {
            assert!(
                EMPTY_SIG_WARN_THROTTLE_SECS >= 10,
                "throttle should be at least 10s to avoid log spam"
            );
            assert!(
                EMPTY_SIG_WARN_THROTTLE_SECS <= 300,
                "throttle should be at most 5 minutes so issues surface quickly"
            );
        };
    }

    /// Verify the decide_notification logic still fires when activity_sig is empty
    /// (count-increase path is unaffected by sig emptiness).
    #[test]
    fn test_count_increase_fires_with_empty_sig() {
        let mut state = NotifState::Idle;
        // count > 0, empty sig — should still fire on count-increase from Idle.
        let decision = decide_notification(&mut state, 3, "", false, true, false, 200);
        assert!(
            decision.should_fire,
            "count-increase from Idle must fire even with empty activity_sig"
        );
        assert_eq!(decision.reason, "idle-count-positive");
    }

    /// Verify that an empty activity_sig does NOT trigger sig_changed (it is
    /// explicitly excluded in both Notified and ZeroPending arms).
    #[test]
    fn test_empty_sig_never_triggers_sig_changed() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: "1:0:Alice".to_string(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };
        // Same count, empty sig — must not fire.
        // Use now=159 so elapsed=59 < NOTIF_REARM_SECS=60 (time-rearm must not kick in).
        let decision = decide_notification(&mut state, 1, "", false, true, false, 159);
        assert!(
            !decision.should_fire,
            "empty activity_sig must not trigger sig_changed notification"
        );
        assert!(
            !decision.sig_changed,
            "sig_changed must be false for empty sig"
        );
    }

    // -----------------------------------------------------------------------
    // Typing-indicator re-arm tests (TYPING_REARM_SECS)
    // -----------------------------------------------------------------------

    /// After a typing-indicator zero, count returning once TYPING_REARM_SECS
    /// have elapsed since the previous notification should re-fire.
    #[test]
    fn test_typing_indicator_rearm_fires_after_cooldown() {
        // Simulate: notification fired at T=100, typing indicator at T=106,
        // message arrives at T=108 (elapsed from fired_at = 8 ≥ TYPING_REARM_SECS=5).
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };

        // Typing indicator fires — count drops to 0, is_typing_indicator=true.
        let zero = decide_notification(&mut state, 0, "", false, true, true, 106);
        assert!(!zero.should_fire);
        assert_eq!(zero.reason, "zero-pending-start");
        // Confirm zero_from_typing was stored.
        assert_eq!(
            state,
            NotifState::ZeroPending {
                prev_count: 1,
                prev_sig: String::new(),
                prev_fired_at_secs: 100,
                zero_since_secs: 106,
                zero_from_typing: true,
                prev_typing_rearm_exhausted: false,
            }
        );

        // Message arrives — count returns, elapsed = 108-100 = 8 ≥ 5 → re-arm.
        let rearm = decide_notification(&mut state, 1, "", false, true, false, 108);
        assert!(rearm.should_fire, "typing rearm must fire after cooldown");
        assert_eq!(rearm.reason, "typing-indicator-rearm");
    }

    /// Typing-indicator zero followed by a quick count return (elapsed <
    /// TYPING_REARM_SECS) must NOT re-fire — rapid oscillation guard.
    #[test]
    fn test_typing_indicator_rearm_suppressed_too_soon() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };

        // Typing indicator at T=101 (elapsed so far: 1s).
        let _ = decide_notification(&mut state, 0, "", false, true, true, 101);

        // Count returns at T=103 — elapsed = 3 < TYPING_REARM_SECS=5 → no fire.
        let d = decide_notification(&mut state, 1, "", false, true, false, 103);
        assert!(
            !d.should_fire,
            "typing rearm must be suppressed before cooldown expires"
        );
        assert_eq!(d.reason, "zero-bounce-oscillation-suppressed");
    }

    /// Regular oscillation (non-typing zero) must NOT be affected by the
    /// typing-indicator re-arm path even when enough time has elapsed.
    #[test]
    fn test_non_typing_zero_not_rearmed() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };

        // Regular zero (not typing indicator) at T=103.
        let _ = decide_notification(&mut state, 0, "", false, true, false, 103);

        // Count returns at T=110 — elapsed=10 ≥ TYPING_REARM_SECS, but
        // zero_from_typing=false so typing_rearm must NOT trigger.
        let d = decide_notification(&mut state, 1, "", false, true, false, 110);
        assert!(
            !d.should_fire,
            "non-typing zero bounce must not re-arm even after cooldown"
        );
        assert_eq!(d.reason, "zero-bounce-oscillation-suppressed");
    }

    /// After a typing-indicator re-arm fires once, a second typing→message cycle
    /// must NOT fire again — the re-arm is consumed (exhausted) per Notified entry.
    /// This guards against the infinite-spam loop observed in production logs.
    #[test]
    fn test_typing_indicator_rearm_fires_only_once() {
        // T=100: initial notification fires (Idle → Notified).
        let mut state = NotifState::Idle;
        let d = decide_notification(&mut state, 1, "", false, true, false, 100);
        assert!(d.should_fire);

        // T=106: typing indicator (count=0, is_typing=true) → ZeroPending{zero_from_typing=true,
        //        prev_typing_rearm_exhausted=false}.
        let d = decide_notification(&mut state, 0, "", false, true, true, 106);
        assert!(!d.should_fire);

        // T=112: message arrives, elapsed=12 ≥ 5 → first rearm fires.
        let d = decide_notification(&mut state, 1, "", false, true, false, 112);
        assert!(d.should_fire, "first rearm must fire");
        assert_eq!(d.reason, "typing-indicator-rearm");
        // State is now Notified{typing_rearm_exhausted=true}.

        // T=118: another typing indicator → ZeroPending{prev_typing_rearm_exhausted=true}.
        let d = decide_notification(&mut state, 0, "", false, true, true, 118);
        assert!(!d.should_fire);

        // T=124: another message, elapsed=12 ≥ 5 — but rearm is exhausted → must NOT fire.
        let d = decide_notification(&mut state, 1, "", false, true, false, 124);
        assert!(
            !d.should_fire,
            "second rearm must be blocked (exhausted=true)"
        );
        assert_eq!(d.reason, "zero-bounce-oscillation-suppressed");
    }

    /// A genuine count increase resets the exhausted flag so the NEXT typing→message
    /// cycle is allowed to re-arm again (rearm is per-message, not per-session).
    #[test]
    fn test_typing_indicator_rearm_reset_by_count_increase() {
        // T=100: initial notification.
        let mut state = NotifState::Idle;
        let d = decide_notification(&mut state, 1, "", false, true, false, 100);
        assert!(d.should_fire);

        // T=106: typing indicator.
        let _ = decide_notification(&mut state, 0, "", false, true, true, 106);

        // T=112: rearm fires (first time) → exhausted=true.
        let d = decide_notification(&mut state, 1, "", false, true, false, 112);
        assert!(d.should_fire);
        assert_eq!(d.reason, "typing-indicator-rearm");

        // T=114: genuine new message (count=2) → fires via count_increased,
        //        resets exhausted to false.
        let d = decide_notification(&mut state, 2, "", false, true, false, 114);
        assert!(d.should_fire, "count_increased must fire");
        assert_eq!(d.reason, "count-increased");
        // State is now Notified{count=2, typing_rearm_exhausted=false}.

        // T=120: typing indicator again.
        let _ = decide_notification(&mut state, 0, "", false, true, true, 120);

        // T=126: message returns — rearm is allowed again (exhausted was reset).
        let d = decide_notification(&mut state, 1, "", false, true, false, 126);
        assert!(
            d.should_fire,
            "rearm must be allowed after exhausted was reset by count_increased"
        );
        assert_eq!(d.reason, "typing-indicator-rearm");
    }

    /// Verify TYPING_REARM_SECS constant is within a sensible range.
    #[test]
    fn test_typing_rearm_constant_is_reasonable() {
        const _: () = {
            assert!(
                TYPING_REARM_SECS >= 3,
                "rearm floor must exceed oscillation interval (~3s)"
            );
            assert!(
                TYPING_REARM_SECS <= 15,
                "rearm floor must be short enough to catch real replies"
            );
        };
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

    // -----------------------------------------------------------------------
    // Fix 1: focused→Idle immediate reset tests
    // -----------------------------------------------------------------------

    /// When the window is focused and count drops to 0 from Notified, the
    /// state must reset to Idle immediately (no ZeroPending debounce).
    #[test]
    fn test_focused_read_all_from_notified_resets_immediately() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };
        // is_focused=true, count=0 → must go straight to Idle.
        let d = decide_notification(&mut state, 0, "", true, true, false, 102);
        assert!(!d.should_fire);
        assert!(d.clear_badge, "focused read-all must clear badge");
        assert_eq!(d.reason, "focused-read-all");
        assert_eq!(state, NotifState::Idle);
    }

    /// Same as above but starting from ZeroPending (window was already
    /// in zero-pending state when the user gains focus).
    #[test]
    fn test_focused_read_all_from_zero_pending_resets_immediately() {
        let mut state = NotifState::ZeroPending {
            prev_count: 1,
            prev_sig: String::new(),
            prev_fired_at_secs: 100,
            zero_since_secs: 102,
            zero_from_typing: false,
            prev_typing_rearm_exhausted: false,
        };
        let d = decide_notification(&mut state, 0, "", true, true, false, 104);
        assert!(!d.should_fire);
        assert!(d.clear_badge, "focused read-all must clear badge");
        assert_eq!(d.reason, "focused-read-all");
        assert_eq!(state, NotifState::Idle);
    }

    /// Full scenario: user gets notified, opens & reads the message
    /// (focused+count→0 → Idle), minimizes, new message arrives → fires.
    #[test]
    fn test_focused_read_then_minimize_then_new_message_fires() {
        // T=100: initial notification fires (Idle → Notified).
        let mut state = NotifState::Idle;
        let d = decide_notification(&mut state, 1, "", false, true, false, 100);
        assert!(d.should_fire);

        // T=103: user opens window (focused=true), count still 1 — focus
        // reset in lib.rs is skipped because count>0; state unchanged.
        // (No decide_notification call here — that would be a title-change
        // event from Messenger which may or may not happen on focus.)

        // T=105: user reads the message → count drops to 0, window focused.
        let d = decide_notification(&mut state, 0, "", true, true, false, 105);
        assert!(!d.should_fire);
        assert_eq!(d.reason, "focused-read-all");
        assert_eq!(state, NotifState::Idle, "must reset to Idle after focused read");

        // T=108: user minimizes; new message arrives → count=1, not focused.
        let d = decide_notification(&mut state, 1, "", false, true, false, 108);
        assert!(d.should_fire, "new message after focused read must fire notification");
        assert_eq!(d.reason, "idle-count-positive");
    }

    /// Focused read with typing_rearm_exhausted=true: state must still reset.
    #[test]
    fn test_focused_read_all_resets_even_when_typing_rearm_exhausted() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(),
            fired_at_secs: 100,
            typing_rearm_exhausted: true, // previously exhausted
        };
        let d = decide_notification(&mut state, 0, "", true, true, false, 110);
        assert!(!d.should_fire);
        assert_eq!(d.reason, "focused-read-all");
        assert_eq!(state, NotifState::Idle);

        // New message must fire without any exhausted flag interference.
        let d = decide_notification(&mut state, 1, "", false, true, false, 115);
        assert!(d.should_fire);
        assert_eq!(d.reason, "idle-count-positive");
    }

    // -----------------------------------------------------------------------
    // Fix 2: 60 s time-rearm tests
    // -----------------------------------------------------------------------

    /// After NOTIF_REARM_SECS with the same count the notification must re-fire.
    #[test]
    fn test_time_rearm_60s_fires_after_elapsed() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };
        // exactly at the boundary (elapsed == NOTIF_REARM_SECS) → fire
        let now = 100 + NOTIF_REARM_SECS;
        let d = decide_notification(&mut state, 1, "", false, true, false, now);
        assert!(d.should_fire, "time-rearm must fire at the 60 s boundary");
        assert_eq!(d.reason, "time-rearm-60s");
        // fired_at_secs must be refreshed to now.
        assert_eq!(
            state,
            NotifState::Notified {
                count: 1,
                sig: String::new(),
                fired_at_secs: now,
                typing_rearm_exhausted: false,
            }
        );
    }

    /// One second before NOTIF_REARM_SECS must still be suppressed.
    #[test]
    fn test_time_rearm_60s_suppressed_before_elapsed() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };
        let now = 100 + NOTIF_REARM_SECS - 1;
        let d = decide_notification(&mut state, 1, "", false, true, false, now);
        assert!(!d.should_fire, "time-rearm must not fire before 60 s");
        assert_eq!(d.reason, "same-activity-suppressed");
    }

    /// After a time-rearm fires, the fired_at timestamp resets; another
    /// immediate call must be suppressed (anti-spam guard).
    #[test]
    fn test_time_rearm_60s_resets_timer_after_fire() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };
        let t1 = 100 + NOTIF_REARM_SECS;
        // First re-arm fires.
        let d = decide_notification(&mut state, 1, "", false, true, false, t1);
        assert!(d.should_fire);
        assert_eq!(d.reason, "time-rearm-60s");

        // 1 second later — must be suppressed (timer reset to t1).
        let d = decide_notification(&mut state, 1, "", false, true, false, t1 + 1);
        assert!(!d.should_fire, "second immediate call must be suppressed");
        assert_eq!(d.reason, "same-activity-suppressed");
    }

    /// time-rearm must NOT fire when window is focused (user can see the message).
    #[test]
    fn test_time_rearm_60s_suppressed_when_focused() {
        let mut state = NotifState::Notified {
            count: 1,
            sig: String::new(),
            fired_at_secs: 100,
            typing_rearm_exhausted: false,
        };
        let now = 100 + NOTIF_REARM_SECS;
        // is_focused=true
        let d = decide_notification(&mut state, 1, "", true, true, false, now);
        assert!(!d.should_fire, "time-rearm must not fire when window is focused");
    }

    /// time-rearm from ZeroPending: user read the message quickly (within 7 s)
    /// but a new message arrives 60 s after the original notification.
    #[test]
    fn test_time_rearm_60s_from_zero_pending_fires() {
        // Original notif fired at T=100, user read at T=103 (ZeroPending entered
        // via unfocused path to keep the test independent of fix 1).
        let mut state = NotifState::ZeroPending {
            prev_count: 1,
            prev_sig: String::new(),
            prev_fired_at_secs: 100,
            zero_since_secs: 103,
            zero_from_typing: false,
            prev_typing_rearm_exhausted: false,
        };
        // New message arrives at T=162 (elapsed from original fire = 62 >= 60).
        let d = decide_notification(&mut state, 1, "", false, true, false, 162);
        assert!(d.should_fire, "time-rearm must fire from ZeroPending after 60 s");
        assert_eq!(d.reason, "time-rearm-60s");
    }

    /// time-rearm from ZeroPending is suppressed when < 60 s since original fire.
    #[test]
    fn test_time_rearm_60s_from_zero_pending_suppressed_before_elapsed() {
        let mut state = NotifState::ZeroPending {
            prev_count: 1,
            prev_sig: String::new(),
            prev_fired_at_secs: 100,
            zero_since_secs: 103,
            zero_from_typing: false,
            prev_typing_rearm_exhausted: false,
        };
        // Only 50 s since fire → still suppressed.
        let d = decide_notification(&mut state, 1, "", false, true, false, 150);
        assert!(!d.should_fire);
        assert_eq!(d.reason, "zero-bounce-oscillation-suppressed");
    }

    /// Verify NOTIF_REARM_SECS constant is within a sensible range.
    #[test]
    fn test_notif_rearm_constant_is_reasonable() {
        const _: () = {
            assert!(
                NOTIF_REARM_SECS >= 30,
                "rearm must be at least 30 s to avoid spam"
            );
            assert!(
                NOTIF_REARM_SECS <= 300,
                "rearm must be at most 5 min to remain useful"
            );
        };
    }
}
