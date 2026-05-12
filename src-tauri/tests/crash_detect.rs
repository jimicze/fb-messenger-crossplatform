//! Regression tests: assert that the CrashDetect `had_good_title` arming guard
//! (`messenger_com_navigated`) and the all-platform `page_load_stable`
//! SPA-navigation / reload guard remain in place so that normal SPA title-clears,
//! `window.location.reload()` (e.g. appearance toggle on Linux), and the
//! `loading.html` splash-page title cannot trigger a false-positive crash-
//! detection loop.
//!
//! These live in an integration-test file (not inside lib.rs) so that
//! `include_str!("../src/lib.rs")` does NOT include the assertions themselves —
//! preventing false positives where the searched string appears inside the test.

const SOURCE: &str = include_str!("../src/lib.rs");
const COMMANDS: &str = include_str!("../src/commands.rs");

// ---------------------------------------------------------------------------
// messenger_com_navigated guard (startup false-positive fix)
// ---------------------------------------------------------------------------

/// `had_good_title` must only be set when `messenger_com_navigated` is also
/// true.  Without this guard the loading.html title "Messenger X" (which
/// contains "Messenger") would arm the crash detector before any real
/// messenger.com page has loaded, causing a false-positive auto-reload on
/// every cold launch.
#[test]
fn had_good_title_is_gated_on_messenger_com_navigated() {
    assert!(
        SOURCE.contains("&& messenger_com_navigated"),
        "had_good_title arming must be guarded by && messenger_com_navigated"
    );
}

/// The `on_navigation` handler must set the `messenger_com_navigated` flag
/// when it allows a navigation to `www.messenger.com`.  If this setter is
/// removed the guard in `on_document_title_changed` will never arm and the
/// crash detector will never fire — even for real crashes.
#[test]
fn on_navigation_sets_messenger_com_navigated_for_messenger_host() {
    assert!(
        SOURCE.contains("if host == \"www.messenger.com\" {"),
        "on_navigation must have a www.messenger.com host check"
    );
    assert!(
        SOURCE.contains("messenger_com_navigated_nav"),
        "on_navigation must reference messenger_com_navigated_nav to set the flag"
    );
}

// ---------------------------------------------------------------------------
// page_load_stable guard (all-platform SPA / reload false-positive fix)
// ---------------------------------------------------------------------------

/// `had_good_title` must only be set when `page_load_stable` is also `true`.
/// Both macOS WKWebView (title="" on every SPA navigation) and Linux
/// WebKitGTK (`window.location.reload()` from the appearance toggle) transiently
/// clear the document title — without this guard either would arm the crash
/// detector prematurely.  This is an all-platform guard, not macOS-only.
#[test]
fn had_good_title_arming_is_gated_on_page_load_stable() {
    assert!(
        SOURCE.contains("&& page_load_stable.load"),
        "had_good_title arming must be guarded by && page_load_stable.load (all platforms)"
    );
}

/// The CrashDetect fire condition must also be gated on `page_load_stable` so
/// that an already-armed `had_good_title` cannot fire CrashDetect while the
/// page is still loading (title="" is normal during macOS SPA navigation and
/// Linux appearance-toggle reload).  This is an all-platform guard.
#[test]
fn crash_detect_fire_is_gated_on_page_load_stable() {
    // There must be at least two occurrences of the page_load_stable gate —
    // one for had_good_title arming and one for the CrashDetect fire condition.
    let count = SOURCE.matches("&& page_load_stable.load").count();
    assert!(
        count >= 2,
        "page_load_stable gate must appear in both had_good_title arming \
         and the CrashDetect fire condition; found {count} occurrence(s)"
    );
}

/// The `on_navigation` handler must reset `page_load_stable` to `false` for
/// all platforms when a www.messenger.com navigation starts.  This ensures a
/// new page transition is not treated as stable until `on_page_load::Finished`
/// fires, preventing false-positive CrashDetect fires on macOS (SPA navigation)
/// and Linux (appearance-toggle reload).
#[test]
fn on_navigation_resets_page_load_stable() {
    // Assert the actual reset call exists — `page_load_stable_nav` only
    // appears in the on_navigation handler, so this uniquely identifies the
    // correct store.
    let store = "page_load_stable_nav.store(false, std::sync::atomic::Ordering::Relaxed)";
    assert!(
        SOURCE.contains(store),
        "on_navigation must call page_load_stable_nav.store(false, ...) to reset stability"
    );
    // The reset must NOT be inside a macOS-only cfg! guard.  We check this by
    // finding the store call and inspecting the 300 bytes before it for a
    // cfg!(target_os = "macos") token.  This is indentation-agnostic and won't
    // false-pass if the guard is reformatted.
    let macos_guard = "cfg!(target_os = \"macos\")";
    if let Some(pos) = SOURCE.find(store) {
        let preceding = &SOURCE[pos.saturating_sub(300)..pos];
        assert!(
            !preceding.contains(macos_guard),
            "page_load_stable_nav reset must not be inside a macos-only cfg! guard"
        );
    }
}

/// The `on_page_load` handler must set `page_load_stable` to `true` when the
/// `Finished` event fires for www.messenger.com on all platforms.  Without this
/// the crash detector would never re-arm after a navigation on any platform.
#[test]
fn on_page_load_sets_page_load_stable_true_on_finished() {
    assert!(
        SOURCE.contains("page_load_stable_pl"),
        "on_page_load must reference page_load_stable_pl to set stability flag"
    );
    // The setter must be guarded by `matches!(payload.event(), PageLoadEvent::Finished)`
    // without a platform-only cfg! wrapper — it applies to all platforms.
    assert!(
        SOURCE.contains(
            "matches!(payload.event(), PageLoadEvent::Finished)"
        ),
        "page_load_stable setter must use matches!(...Finished) (all platforms)"
    );
    // Must NOT be gated on a macOS-only condition any more.
    assert!(
        !SOURCE.contains(
            "cfg!(target_os = \"macos\") && matches!(payload.event(), PageLoadEvent::Finished)"
        ),
        "page_load_stable setter must not be macOS-only; it must apply to all platforms"
    );
    // The store call must set the flag to true.
    assert!(
        SOURCE.contains("page_load_stable_pl.store(true, std::sync::atomic::Ordering::Relaxed)"),
        "on_page_load must call page_load_stable_pl.store(true, ...) on Finished"
    );
}

/// When the max-reload limit is reached the `else` branch must reset
/// `had_good_title` to `false` (all platforms).  Without this, every subsequent
/// empty-title event — from a real crash before the recovery page sets its title
/// (Linux: GStreamer crash mid-load) or from a macOS SPA navigation — would
/// immediately re-trigger the else branch, creating an infinite
/// navigate-to-root loop.
#[test]
fn max_reloads_else_resets_had_good_title_to_break_loop() {
    // There must be at least two `had_good_title.store(false, ...)` calls:
    //   1. In the `< MAX_CRASH_RELOADS` reload branch (existing guard).
    //   2. In the `else` (max-reloads-exceeded) branch (the new cross-platform fix).
    // Asserting the count directly verifies the code reset exists in both
    // branches rather than relying on comment text.
    let count = SOURCE
        .matches("had_good_title.store(false, std::sync::atomic::Ordering::Relaxed)")
        .count();
    assert!(
        count >= 2,
        "had_good_title.store(false, ...) must appear in both the reload branch and \
         the max-reloads else branch to prevent the infinite loop; found {count} occurrence(s)"
    );
}

/// `on_document_title_changed` must recover `page_load_stable` to `true` when a
/// good Messenger title is seen but `page_load_stable` is still `false` (i.e.,
/// `on_page_load::Finished` was not received — possible on WebKitGTK where SPA
/// thread navigation may not fire the `Finished` event).  Without this recovery,
/// CrashDetect would be permanently disarmed on Linux after the first thread nav
/// because `page_load_stable` would stay `false` for the rest of the session.
#[test]
fn good_title_recovery_sets_page_load_stable_when_finished_not_received() {
    assert!(
        SOURCE.contains("&& !page_load_stable.load"),
        "on_document_title_changed must recover page_load_stable=true when a good title \
         is seen but page_load_stable is still false (on_page_load::Finished not received)"
    );
}

// ---------------------------------------------------------------------------
// Cinnamon XUrgency blink — LAST_RESTORED_FROM_MINIMIZED_SECS false-stamp fix
// ---------------------------------------------------------------------------

/// The poll thread must guard `LAST_RESTORED_FROM_MINIMIZED_SECS` stamps with
/// `was_minimized`.  Without this guard, Cinnamon/Muffin XUrgency blinks
/// (triggered by `request_user_attention()`) produce rapid not-focused→focused
/// transitions that re-stamp the restore timestamp on every blink cycle.  Each
/// re-stamp resets the 3-second `RESTORE_GRACE_SECS` window, holding
/// `came_from_background=true` for seconds and allowing `sig_changed` to fire
/// duplicate notifications from a single group message (observed: 4 banners).
///
/// The fix: only stamp when `was_minimized` is true so that ordinary
/// background→focus transitions (including urgency blinks, which never involve
/// a minimize) do not count as "restored from minimized".
#[test]
fn blink_stamp_requires_was_minimized() {
    assert!(
        SOURCE.contains("is_visible && !was_visible && was_minimized"),
        "LAST_RESTORED_FROM_MINIMIZED_SECS must only be stamped when was_minimized \
         is true; stamping on any not-visible→visible edge (including Cinnamon \
         XUrgency blinks) causes 4× duplicate notifications from a single message"
    );
}

/// The poll thread must keep `was_minimized` up to date after every poll
/// cycle — not only when visibility changes.  Without this, a
/// minimize→unminimize (without focus) followed by a focus gain would see
/// `was_minimized=true` stale from a much earlier cycle and may incorrectly
/// stamp or miss the stamp.
#[test]
fn was_minimized_is_updated_every_poll_cycle() {
    assert!(
        SOURCE.contains("was_minimized = is_minimized;"),
        "was_minimized must be assigned from is_minimized each poll iteration \
         (not only on visibility changes) so the stamp guard stays accurate"
    );
}

// ---------------------------------------------------------------------------
// post_crash_proxy_block — must be cleared after successful page reload
// ---------------------------------------------------------------------------

/// After a WebKit crash, `post_crash_proxy_block` is set to `true` to
/// temporarily block the fbsbx.com maw_proxy_page that triggers a GStreamer
/// NULL-pointer deref on ARM64 Linux.  The block must be cleared once the
/// reloaded Messenger page finishes loading successfully (`on_page_load::Finished`
/// for www.messenger.com), otherwise GIF picker and video thumbnails stay broken
/// for the rest of the session.
#[test]
fn post_crash_proxy_block_cleared_on_page_load_finished() {
    // Use a formatting-agnostic position-based check rather than a literal
    // newline + indentation match that rustfmt could reflow.
    //
    // Strategy: `rfind` gives the *last* occurrence of the variable name,
    // which is the `.store(false …)` call-site (not the clone or the load).
    // Then verify:
    //   (a) `.store(false` appears within 200 bytes forward of that position.
    //   (b) `Finished` appears within 300 bytes *before* that position —
    //       confirming the clear lives inside the PageLoadEvent::Finished branch.
    let token = "post_crash_proxy_block_pl";
    let pos = SOURCE
        .rfind(token)
        .expect("post_crash_proxy_block_pl not found in lib.rs SOURCE");

    let forward = &SOURCE[pos..SOURCE.len().min(pos + 200)];
    assert!(
        forward.contains(".store(false"),
        "post_crash_proxy_block must be cleared (store false) in on_page_load::Finished \
         for www.messenger.com; if it is never cleared GIF/video loading is permanently \
         broken after the first crash"
    );

    let preceding = &SOURCE[pos.saturating_sub(300)..pos];
    assert!(
        preceding.contains("Finished"),
        "post_crash_proxy_block clear must be inside the on_page_load::Finished branch"
    );
}

// ---------------------------------------------------------------------------
// Startup notification baseline — pre-existing unread must not re-notify
// ---------------------------------------------------------------------------

/// On every process restart (including crash-induced ones) NOTIF_STATE resets
/// to `Idle`.  Without a startup-baseline guard, the first title update with
/// `count > 0` takes the `idle-count-positive` branch and fires a spurious
/// notification for a message the user already received.
///
/// The fix: when `old_count == u32::MAX` (the static sentinel — "this process
/// has never seen a count before") and `count > 0`, silently advance
/// NOTIF_STATE to `Notified` without dispatching so the pre-existing unread is
/// treated as already-notified.
#[test]
fn startup_baseline_suppresses_pre_existing_unread_notification() {
    assert!(
        COMMANDS.contains("old_count == u32::MAX && count > 0"),
        "update_unread_count_core must baseline the first positive count after \
         boot (old_count == u32::MAX sentinel) to avoid re-notifying for \
         pre-existing unread messages on every crash restart"
    );
}
