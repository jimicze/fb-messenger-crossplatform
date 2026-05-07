//! Notification dispatcher with same-content deduplication window.
//!
//! Phase M: all notification fires triggered by the unread-count title path
//! (`commands::update_unread_count_core`) go through [`dispatch`] instead of
//! calling [`crate::services::notification::show_notification`] directly.  The
//! dispatcher rejects back-to-back fires with the same `(sender, count)` key
//! within [`DEDUP_WINDOW`] so that the count-rise + typing-rearm + sig-changed
//! decision branches in `commands::update_unread_count_core` cannot
//! double-emit a single conversation event into two OS toasts in quick
//! succession.
//!
//! ## What this dispatcher does NOT do
//! - It does **not** decide whether to fire (that lives in
//!   `commands::decide_notification` /
//!   `commands::update_unread_count_core`).
//! - It does **not** translate keys or build titles (the caller already
//!   resolved the localized notification title before calling).
//! - It does **not** persist state across process restarts.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::AppHandle;

use crate::services::notification::show_notification;

/// Time window during which a second `dispatch` call carrying the same
/// `(normalized_sender, count)` key is suppressed.  Tuned to be longer than
/// the typing-indicator-rearm path roundtrip (~5s in `TYPING_REARM_SECS`)
/// but short enough that a genuine new message arriving 6+ seconds later
/// is not suppressed.
pub const DEDUP_WINDOW: Duration = Duration::from_secs(5);

/// Last successful dispatch, used by [`dispatch`] to enforce [`DEDUP_WINDOW`].
pub(crate) struct LastDispatch {
    sender: String,
    count: u32,
    fired_at: Instant,
}

static LAST_DISPATCH: Mutex<Option<LastDispatch>> = Mutex::new(None);

/// Outcome of a [`dispatch`] call.  Always logged; returned to the caller
/// for tests and any future programmatic handling.
#[derive(Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    /// The OS notification was emitted (or attempted — see `Result<(), String>`).
    Fired,
    /// Suppressed by [`DEDUP_WINDOW`] because an identical
    /// `(sender, count)` was emitted within the dedup window.
    Suppressed { age_ms: u128 },
}

/// Pure decision used by [`dispatch`] and exposed for unit tests.
///
/// Returns [`DispatchOutcome::Suppressed`] when `last` matches the incoming
/// `(sender, count)` key and was emitted within `window`.  All other cases
/// (no prior dispatch, different key, key matches but window elapsed) return
/// [`DispatchOutcome::Fired`].
pub fn decide(
    last: Option<&LastDispatch>,
    sender: &str,
    count: u32,
    now: Instant,
    window: Duration,
) -> DispatchOutcome {
    if let Some(prev) = last {
        if prev.sender == sender && prev.count == count {
            let age = now.saturating_duration_since(prev.fired_at);
            if age <= window {
                return DispatchOutcome::Suppressed {
                    age_ms: age.as_millis(),
                };
            }
        }
    }
    DispatchOutcome::Fired
}

/// Dispatch a notification, deduplicating against the last successful fire.
///
/// `sender` may be empty (generic "new message" fallback); the dedup key
/// uses the verbatim sender string, so `""` deduplicates against `""`.
///
/// `title` is the already-localized notification title built by the caller.
/// `site` is forwarded to [`show_notification`] for log correlation.
///
/// Returns the [`DispatchOutcome`] alongside any `show_notification` error.
/// Suppressed fires return `Ok(DispatchOutcome::Suppressed { .. })` because
/// no underlying sink was invoked.
pub fn dispatch(
    app: &AppHandle,
    site: &str,
    sender: &str,
    count: u32,
    title: &str,
    silent: bool,
) -> Result<DispatchOutcome, String> {
    let now = Instant::now();
    let outcome = match LAST_DISPATCH.lock() {
        Ok(guard) => decide(guard.as_ref(), sender, count, now, DEDUP_WINDOW),
        Err(e) => {
            log::warn!("[MessengerX][NotifDispatch] mutex poisoned; bypassing dedup: {e}");
            DispatchOutcome::Fired
        }
    };

    match outcome {
        DispatchOutcome::Suppressed { age_ms } => {
            log::info!(
                "[MessengerX][NotifDispatch] suppressed site={site:?} sender={sender:?} \
                 count={count} age_ms={age_ms} window_ms={}",
                DEDUP_WINDOW.as_millis()
            );
            return Ok(DispatchOutcome::Suppressed { age_ms });
        }
        DispatchOutcome::Fired => {
            log::info!(
                "[MessengerX][NotifDispatch] firing site={site:?} sender={sender:?} \
                 count={count} title={title:?} silent={silent}"
            );
        }
    }

    let result = show_notification(app, title, "", "messenger-unread", silent, site);

    // Update last-dispatch baseline only on a successful sink delivery so
    // that a failed sink does not silently suppress the next retry.
    if result.is_ok() {
        if let Ok(mut guard) = LAST_DISPATCH.lock() {
            *guard = Some(LastDispatch {
                sender: sender.to_string(),
                count,
                fired_at: now,
            });
        }
    }

    result.map(|()| DispatchOutcome::Fired)
}

/// Test-only helper: clear the dedup baseline between unit tests.
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    if let Ok(mut guard) = LAST_DISPATCH.lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last(sender: &str, count: u32, fired_at: Instant) -> LastDispatch {
        LastDispatch {
            sender: sender.to_string(),
            count,
            fired_at,
        }
    }

    #[test]
    fn no_prior_dispatch_always_fires() {
        let now = Instant::now();
        assert_eq!(
            decide(None, "Alice", 1, now, DEDUP_WINDOW),
            DispatchOutcome::Fired
        );
    }

    #[test]
    fn same_key_inside_window_is_suppressed() {
        let now = Instant::now();
        let prev = last("Alice", 1, now - Duration::from_secs(2));
        match decide(Some(&prev), "Alice", 1, now, DEDUP_WINDOW) {
            DispatchOutcome::Suppressed { age_ms } => {
                assert!((1_900..=2_100).contains(&age_ms), "age_ms={age_ms}");
            }
            other => panic!("expected Suppressed, got {other:?}"),
        }
    }

    #[test]
    fn same_key_outside_window_fires() {
        let now = Instant::now();
        let prev = last("Alice", 1, now - DEDUP_WINDOW - Duration::from_secs(1));
        assert_eq!(
            decide(Some(&prev), "Alice", 1, now, DEDUP_WINDOW),
            DispatchOutcome::Fired
        );
    }

    #[test]
    fn different_sender_fires_immediately() {
        let now = Instant::now();
        let prev = last("Alice", 1, now);
        assert_eq!(
            decide(Some(&prev), "Bob", 1, now, DEDUP_WINDOW),
            DispatchOutcome::Fired
        );
    }

    #[test]
    fn different_count_fires_immediately() {
        let now = Instant::now();
        let prev = last("Alice", 1, now);
        assert_eq!(
            decide(Some(&prev), "Alice", 2, now, DEDUP_WINDOW),
            DispatchOutcome::Fired
        );
    }

    #[test]
    fn empty_sender_dedups_against_empty_sender() {
        let now = Instant::now();
        let prev = last("", 1, now - Duration::from_secs(1));
        match decide(Some(&prev), "", 1, now, DEDUP_WINDOW) {
            DispatchOutcome::Suppressed { .. } => {}
            other => panic!("expected Suppressed for empty-sender match, got {other:?}"),
        }
    }

    #[test]
    fn empty_sender_does_not_dedup_against_named_sender() {
        let now = Instant::now();
        let prev = last("Alice", 1, now);
        assert_eq!(
            decide(Some(&prev), "", 1, now, DEDUP_WINDOW),
            DispatchOutcome::Fired
        );
    }

    #[test]
    fn dedup_window_is_within_phase_m_design_bounds() {
        // Must be longer than TYPING_REARM_SECS (5s) only marginally — long
        // enough to absorb count-rise + immediate sig-changed re-fire, short
        // enough that a deliberate second message 6s later still notifies.
        assert!(
            DEDUP_WINDOW >= Duration::from_secs(3),
            "dedup window must absorb back-to-back same-event fires"
        );
        assert!(
            DEDUP_WINDOW <= Duration::from_secs(10),
            "dedup window must not silently swallow real second messages"
        );
    }

    #[test]
    fn reset_helper_clears_baseline() {
        // Smoke test for the test-only helper itself.
        reset_for_tests();
        let guard = LAST_DISPATCH.lock().unwrap();
        assert!(guard.is_none());
    }
}
