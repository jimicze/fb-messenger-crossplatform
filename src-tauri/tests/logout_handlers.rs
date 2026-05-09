//! Integration test: assert both logout handlers (tray / macOS menu) keep
//! snapshot clearing and LOGOUT_CLEAR_SCRIPT evaluation in sync.
//!
//! These checks live in an integration test (not unit tests inside lib.rs)
//! so that `include_str!("../src/lib.rs")` does NOT include the assertions
//! themselves — preventing false positives where the test passes because
//! the searched string appears inside the test code.

const SOURCE: &str = include_str!("../src/lib.rs");

/// Both logout handlers must call `services::cache::clear_snapshots`.
#[test]
fn both_logout_handlers_clear_snapshots() {
    assert!(
        SOURCE.contains("services::cache::clear_snapshots(handle)"),
        "tray logout must clear snapshots"
    );
    assert!(
        SOURCE.contains("services::cache::clear_snapshots(&h)"),
        "macOS logout must clear snapshots"
    );
}

/// Both logout handlers must evaluate `LOGOUT_CLEAR_SCRIPT` and
/// log errors instead of silently dropping them.
#[test]
fn both_logout_handlers_log_eval_errors() {
    assert!(
        SOURCE.contains("Failed to eval logout clear script"),
        "logout handlers must log eval errors"
    );
    assert!(
        SOURCE.contains("if let Err(e) = wv.eval(LOGOUT_CLEAR_SCRIPT)"),
        "logout handlers must handle eval Result"
    );
}
