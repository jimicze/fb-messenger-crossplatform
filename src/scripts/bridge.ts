/**
 * JS ↔ Rust IPC bridge — INJECTED via Rust initialization scripts.
 * Uses window.__TAURI__.core.invoke() for frontend→backend communication.
 * Uses webview.eval() for backend→frontend communication.
 * See src-tauri/src/lib.rs for injection code.
 */
export {};
