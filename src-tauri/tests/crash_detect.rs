//! Regression tests: assert that the CrashDetect `had_good_title` arming guard
//! (`messenger_com_navigated`) remains in place so that the `loading.html`
//! splash-page title cannot trigger a false-positive crash-detection loop.
//!
//! These live in an integration-test file (not inside lib.rs) so that
//! `include_str!("../src/lib.rs")` does NOT include the assertions themselves —
//! preventing false positives where the searched string appears inside the test.

const SOURCE: &str = include_str!("../src/lib.rs");

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
