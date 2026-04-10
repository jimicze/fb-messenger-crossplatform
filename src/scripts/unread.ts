/**
 * Unread count observer — INJECTED via Rust at document end.
 * Uses MutationObserver + 2s polling on document.title to detect unread count.
 * Regex: /^\((\d+)\)/ extracts count from title like "(3) Messenger".
 * See src-tauri/src/lib.rs UNREAD_OBSERVER_JS constant.
 */
export {};
