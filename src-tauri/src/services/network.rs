//! Network monitoring service.
//! For this implementation, network state changes are detected via the browser's
//! `online`/`offline` events (injected at document-end) and communicated back to
//! Rust via Tauri IPC.  This module exposes a lightweight utility that can be
//! extended with native monitoring in the future.

/// Returns `true` when a basic TCP connection to `8.8.8.8:53` (Google DNS) can
/// be established within a short timeout, giving a quick offline/online hint.
///
/// This is intentionally best-effort; the primary connectivity signal is the
/// browser `navigator.onLine` flag propagated via JS events.
pub fn is_likely_online() -> bool {
    use std::net::TcpStream;
    use std::time::Duration;
    TcpStream::connect_timeout(
        &"8.8.8.8:53".parse().expect("valid socket address"),
        Duration::from_secs(2),
    )
    .is_ok()
}
