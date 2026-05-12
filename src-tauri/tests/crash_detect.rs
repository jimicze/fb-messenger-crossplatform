//! Regression tests: assert that the CrashDetect `had_good_title` arming guard
//! (`messenger_com_navigated`) and the macOS `page_load_stable` SPA-navigation
//! guard remain in place so that normal SPA title-clears and the `loading.html`
//! splash-page title cannot trigger a false-positive crash-detection loop.
//!
//! These live in an integration-test file (not inside lib.rs) so that
//! `include_str!("../src/lib.rs")` does NOT include the assertions themselves —
//! preventing false positives where the searched string appears inside the test.

const SOURCE: &str = include_str!("../src/lib.rs");

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
// page_load_stable guard (macOS SPA false-positive fix)
// ---------------------------------------------------------------------------

/// On macOS, `had_good_title` may only be set when `page_load_stable` is also
/// `true`.  WKWebView emits title="" during every SPA navigation (not just
/// after a real crash), so without this guard any mid-session thread navigation
/// would trigger CrashDetect.
#[test]
fn had_good_title_arming_is_gated_on_page_load_stable_on_macos() {
    assert!(
        SOURCE.contains("|| page_load_stable.load"),
        "had_good_title arming must include a page_load_stable gate for macOS"
    );
}

/// The CrashDetect fire condition must also be gated on `page_load_stable` on
/// macOS so that an already-armed `had_good_title` cannot fire CrashDetect
/// during a normal SPA navigation (where the title briefly becomes "").
#[test]
fn crash_detect_fire_is_gated_on_page_load_stable_on_macos() {
    // There must be at least two occurrences of the page_load_stable gate —
    // one for had_good_title arming and one for the CrashDetect fire condition.
    let count = SOURCE.matches("|| page_load_stable.load").count();
    assert!(
        count >= 2,
        "page_load_stable gate must appear in both had_good_title arming \
         and the CrashDetect fire condition; found {count} occurrence(s)"
    );
}

/// The `on_navigation` handler must reset `page_load_stable` to `false` for
/// macOS when a www.messenger.com navigation starts.  This ensures a new SPA
/// transition is not treated as stable until `on_page_load::Finished` fires.
#[test]
fn on_navigation_resets_page_load_stable_on_macos() {
    // Assert the actual reset call exists — `page_load_stable_nav` only
    // appears in the on_navigation handler, so this uniquely identifies the
    // correct store. Checking for the full call rather than just the variable
    // name avoids a false positive where the variable is referenced but the
    // store is absent.
    assert!(
        SOURCE.contains("page_load_stable_nav.store(false, std::sync::atomic::Ordering::Relaxed)"),
        "on_navigation must call page_load_stable_nav.store(false, ...) to reset stability"
    );
    // The reset must be guarded by `if cfg!(target_os = "macos")` so that the
    // condition evaluates to `false` at runtime on Linux/Windows (where
    // WebKitGTK/WebView2 do not clear the title during normal SPA navigation);
    // the optimizer then removes the dead branch.  We check both parts
    // independently to avoid whitespace-sensitive multi-line assertions.
    assert!(
        SOURCE.contains("if cfg!(target_os = \"macos\") {"),
        "page_load_stable_nav reset must be guarded by if cfg!(target_os = \"macos\")"
    );
}

/// The `on_page_load` handler must set `page_load_stable` to `true` when the
/// `Finished` event fires for www.messenger.com on macOS.  Without this the
/// crash detector would never re-arm after a navigation.
#[test]
fn on_page_load_sets_page_load_stable_true_on_finished() {
    assert!(
        SOURCE.contains("page_load_stable_pl"),
        "on_page_load must reference page_load_stable_pl to set stability flag"
    );
    // The setter must be guarded by `cfg!(target_os = "macos") &&
    // matches!(payload.event(), PageLoadEvent::Finished)`.  This combined
    // condition is unique to the on_page_load handler and avoids matching
    // any of the many other `#[cfg(target_os = "macos")]` attributes
    // elsewhere in lib.rs.
    assert!(
        SOURCE.contains(
            "cfg!(target_os = \"macos\") && matches!(payload.event(), PageLoadEvent::Finished)"
        ),
        "page_load_stable setter must use cfg!(target_os = \"macos\") && matches!(...Finished)"
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
