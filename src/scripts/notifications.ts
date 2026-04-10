/**
 * Notification override — INJECTED via Rust at document start.
 * Replaces window.Notification constructor to forward to native notifications.
 * See src-tauri/src/lib.rs NOTIFICATION_OVERRIDE_JS constant.
 */
export {};
