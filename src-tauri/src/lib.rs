//! Library root for the Messenger X cross-platform desktop client.
//!
//! Registers Tauri plugins, builds the main window programmatically (so that
//! custom `initialization_script`s and `on_navigation` hooks can be attached),
//! and wires up the IPC invoke-handler.

mod commands;
mod services;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

// ---------------------------------------------------------------------------
// JavaScript injection scripts
// ---------------------------------------------------------------------------

/// Overrides `window.Notification` so that browser notification calls are
/// forwarded to Rust via `invoke('send_notification', …)`.
///
/// Injected **at document-start** (before the page HTML is parsed).
const NOTIFICATION_OVERRIDE_SCRIPT: &str = r#"
(function() {
    const OriginalNotification = window.Notification;
    window.Notification = function(title, options) {
        try {
            window.__TAURI__.core.invoke('send_notification', {
                title: title,
                body: options?.body || '',
                tag: options?.tag || '',
                silent: options?.silent || false
            });
        } catch(e) { console.error('[MessengerX] Failed to send notification:', e); }
        return {
            close: function() {},
            addEventListener: function() {},
            removeEventListener: function() {},
            onclick: null, onerror: null, onclose: null, onshow: null
        };
    };
    window.Notification.permission = 'granted';
    window.Notification.requestPermission = function(callback) {
        if (callback) callback('granted');
        return Promise.resolve('granted');
    };
    window.Notification.__native__ = true;
})();
"#;

/// Watches the document `<title>` for an unread-count prefix like `(3)` and
/// forwards changes to Rust via `invoke('update_unread_count', …)`.
///
/// Injected at document-start; waits for DOM via `DOMContentLoaded`.
const UNREAD_OBSERVER_SCRIPT: &str = r#"
(function() {
    function getUnreadCountFromTitle() {
        var title = document.title;
        var match = title.match(/^\((\d+)\)/);
        return match ? parseInt(match[1], 10) : 0;
    }
    function sendUnreadCount(count) {
        try {
            window.__TAURI__.core.invoke('update_unread_count', { count: count });
        } catch(e) { console.error('[MessengerX] Failed to send unread count:', e); }
    }
    function setupObserver() {
        var lastCount = getUnreadCountFromTitle();
        sendUnreadCount(lastCount);
        var titleElement = document.querySelector('title');
        if (titleElement) {
            var observer = new MutationObserver(function() {
                var newCount = getUnreadCountFromTitle();
                if (newCount !== lastCount) {
                    lastCount = newCount;
                    sendUnreadCount(newCount);
                }
            });
            observer.observe(titleElement, { childList: true, characterData: true, subtree: true });
        }
        setInterval(function() {
            var newCount = getUnreadCountFromTitle();
            if (newCount !== lastCount) {
                lastCount = newCount;
                sendUnreadCount(newCount);
            }
        }, 2000);
    }
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', setupObserver);
    } else {
        setupObserver();
    }
})();
"#;

/// Periodically hides Messenger "offline" / "Unable to load" dialogs so that
/// the cached content remains readable when the network is unavailable.
///
/// Injected at document-start; relies on `setInterval` to catch late dialogs.
const OFFLINE_DIALOG_HIDER_SCRIPT: &str = r#"
(function() {
    function hideOfflineDialogs() {
        var dialogs = document.querySelectorAll('[role="dialog"]');
        dialogs.forEach(function(el) {
            var text = el.textContent || '';
            if (text.includes('offline') || text.includes('Unable to load') ||
                text.includes('connection') || text.includes('internet') || text.includes('Retry')) {
                el.style.display = 'none';
                var backdrop = el.previousElementSibling;
                if (backdrop && backdrop.style) { backdrop.style.display = 'none'; }
            }
        });
    }
    setInterval(hideOfflineDialogs, 3000);
    document.addEventListener('visibilitychange', hideOfflineDialogs);
})();
"#;

/// Injects a fixed offline-mode banner and exposes
/// `window.__messenger_updateBanner(isOffline)` for Rust to call.
///
/// Injected at document-start; banner is appended to `<body>` once the DOM
/// is ready.  The `banner_text` argument is the translated banner string.
fn build_offline_banner_script(banner_text: &str) -> String {
    format!(
        r#"(function() {{
    function injectBanner() {{
        if (document.getElementById('__messenger_offline_banner__')) return;
        var banner = document.createElement('div');
        banner.id = '__messenger_offline_banner__';
        banner.style.cssText = 'display:none;position:fixed;top:0;left:0;right:0;z-index:999999;background:#f0ad4e;color:#fff;text-align:center;padding:8px 16px;font-size:13px;font-weight:500;font-family:-apple-system,BlinkMacSystemFont,Segoe UI,Roboto,sans-serif;';
        banner.textContent = '{}';
        document.body.prepend(banner);
        function updateBanner(isOffline) {{
            banner.style.display = isOffline ? 'block' : 'none';
        }}
        window.addEventListener('online',  function() {{ updateBanner(false); }});
        window.addEventListener('offline', function() {{ updateBanner(true);  }});
        if (!navigator.onLine) {{ updateBanner(true); }}
        window.__messenger_updateBanner = updateBanner;
    }}
    if (document.readyState === 'loading') {{
        document.addEventListener('DOMContentLoaded', injectBanner);
    }} else {{
        injectBanner();
    }}
}})();"#,
        banner_text
    )
}

/// JavaScript snippet that triggers snapshot capture and forwards the HTML to
/// Rust via `invoke('save_snapshot', …)`.  Called from the Rust snapshot timer.
///
/// Guards against offline/error states: only snapshots when the browser reports
/// online **and** the page is actually on `messenger.com` (prevents overwriting
/// a good snapshot with an error page or offline-degraded DOM).
const SNAPSHOT_TRIGGER_SCRIPT: &str = r#"
(function() {
    try {
        if (!navigator.onLine) return;
        var url = window.location.href;
        if (url.indexOf('messenger.com') === -1) return;
        var html = document.documentElement.outerHTML;
        window.__TAURI__.core.invoke('save_snapshot', { html: html, url: url });
    } catch(e) { console.error('[MessengerX] Failed to save snapshot:', e); }
})();
"#;

// ---------------------------------------------------------------------------
// Application entry point
// ---------------------------------------------------------------------------

/// Run the Tauri application.
///
/// Registers all plugins, builds the main webview window with JS injection
/// scripts and navigation policy, sets up a system-tray icon, and starts the
/// periodic snapshot timer.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            setup_app(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::send_notification,
            commands::update_unread_count,
            commands::get_settings,
            commands::save_settings,
            commands::save_snapshot,
            commands::load_snapshot,
            commands::clear_all_data,
            commands::open_external,
            commands::set_zoom,
            commands::get_zoom,
            commands::get_translations,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ---------------------------------------------------------------------------
// Setup helper (split out so the closure stays readable)
// ---------------------------------------------------------------------------

/// Performs one-time application setup inside the Tauri `setup` hook.
fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    // ------------------------------------------------------------------
    // 1. Load persisted settings so we can bake the zoom level into the
    //    initialization script before the window is created.
    // ------------------------------------------------------------------
    let settings = services::auth::load_settings(app.handle()).unwrap_or_default();
    let zoom_level = settings.zoom_level;

    // ------------------------------------------------------------------
    // 1b. Detect system locale and load translations.
    // ------------------------------------------------------------------
    let locale = services::locale::detect_locale();
    let tr = services::locale::get_translations(&locale);

    // Build a zoom-control init script that (a) defines __messenger_setZoom
    // and (b) applies the saved zoom once the body is available.
    let zoom_init_script = format!(
        r#"(function() {{
    window.__messenger_setZoom = function(level) {{
        document.body.style.zoom = level;
    }};
    var __initial_zoom = {zoom};
    function applyZoom() {{
        if (document.body) {{ document.body.style.zoom = __initial_zoom; }}
    }}
    if (document.readyState === 'loading') {{
        document.addEventListener('DOMContentLoaded', applyZoom);
    }} else {{
        applyZoom();
    }}
}})();"#,
        zoom = zoom_level
    );

    // Build the offline banner script with the translated banner text.
    let offline_banner_script = build_offline_banner_script(&tr.offline_banner);

    // Clone translated strings needed inside the offline fallback thread.
    let offline_banner_text = tr.offline_banner.clone();
    let loading_offline_text = tr.loading_offline.clone();

    // ------------------------------------------------------------------
    // 2. Clone the app handle for the navigation callback closure.
    //    The closure is `Send + Sync + 'static`, so it needs an owned
    //    handle that can outlive the setup function.
    // ------------------------------------------------------------------
    let nav_app_handle = app.handle().clone();

    // ------------------------------------------------------------------
    // 3. Determine initial URL based on connectivity.
    //    If offline, load a local fallback page instead of messenger.com.
    // ------------------------------------------------------------------
    let is_online = services::network::is_likely_online();
    let webview_url = if is_online {
        WebviewUrl::External(url::Url::parse("https://www.messenger.com")?)
    } else {
        log::info!("[MessengerX] Offline at startup — loading local fallback page");
        WebviewUrl::App("index.html".into())
    };

    // ------------------------------------------------------------------
    // 4. Build the main window programmatically.
    // ------------------------------------------------------------------
    let webview = WebviewWindowBuilder::new(app, "main", webview_url)
        .title("Messenger X")
        .inner_size(1200.0, 800.0)
        .min_inner_size(400.0, 300.0)
        .resizable(true)
        // Inject all JS at document-start.
        .initialization_script(NOTIFICATION_OVERRIDE_SCRIPT)
        .initialization_script(UNREAD_OBSERVER_SCRIPT)
        .initialization_script(OFFLINE_DIALOG_HIDER_SCRIPT)
        .initialization_script(&offline_banner_script)
        .initialization_script(&zoom_init_script)
        // Spoof a desktop Chrome UA so Messenger serves its full web-app.
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
         AppleWebKit/537.36 (KHTML, like Gecko) \
         Chrome/120.0.0.0 Safari/537.36",
        )
        // Navigation policy: allow Messenger / Facebook CDN domains;
        // open everything else in the system browser.
        .on_navigation(move |url| {
            let scheme = url.scheme();
            // Pass through non-HTTP schemes (blob:, data:, about:, tauri:, etc.).
            if scheme != "http" && scheme != "https" {
                return true;
            }

            let host = url.host_str().unwrap_or("");
            const ALLOWED: &[&str] = &["messenger.com", "facebook.com", "fbcdn.net", "fbsbx.com"];
            let is_allowed = ALLOWED
                .iter()
                .any(|&d| host == d || host.ends_with(&format!(".{d}")));

            if !is_allowed {
                let url_str = url.to_string();
                let handle = nav_app_handle.clone();
                // Open in system browser on a background thread so we don't
                // block the navigation callback.
                std::thread::spawn(move || {
                    use tauri_plugin_opener::OpenerExt;
                    if let Err(e) = handle.opener().open_url(&url_str, None::<&str>) {
                        log::warn!("[MessengerX] Failed to open external URL {url_str}: {e}");
                    }
                });
                return false;
            }

            true
        })
        .build()?;

    // ------------------------------------------------------------------
    // 5. Offline fallback: inject cached content or show offline message.
    //    Also starts a reconnect timer that redirects to messenger.com
    //    once connectivity is restored.
    // ------------------------------------------------------------------
    if !is_online {
        let fallback_handle = app.handle().clone();
        let fallback_webview = webview.clone();
        std::thread::spawn(move || {
            // Wait for the local page to render.
            std::thread::sleep(std::time::Duration::from_secs(1));

            // Try to load cached snapshot and inject it.
            let has_cache = if let Ok(Some(snapshot)) =
                services::cache::load_latest_snapshot(&fallback_handle)
            {
                let html_json = serde_json::to_string(&snapshot.html).unwrap_or_default();
                let inject = format!(
                    r#"(function(){{
    document.open();
    document.write({html});
    document.close();
    var b=document.createElement('div');
    b.id='__messenger_offline_banner__';
    b.style.cssText='display:block;position:fixed;top:0;left:0;right:0;z-index:999999;background:#f0ad4e;color:#fff;text-align:center;padding:8px 16px;font-size:13px;font-weight:500;font-family:-apple-system,BlinkMacSystemFont,Segoe UI,Roboto,sans-serif;';
    b.textContent='{banner}';
    if(document.body)document.body.prepend(b);
}})()"#,
                    html = html_json,
                    banner = offline_banner_text,
                );
                fallback_webview.eval(&inject).is_ok()
            } else {
                false
            };

            if !has_cache {
                // No cached snapshot — update the loading screen.
                let offline_msg = serde_json::to_string(&loading_offline_text).unwrap_or_default();
                let script = format!(
                    "if(document.querySelector('.loading p')){{document.querySelector('.loading p').textContent={};}}",
                    offline_msg
                );
                let _ = fallback_webview.eval(&script);
            }

            // Start reconnect timer: check every 15 s and redirect when online.
            let _ = fallback_webview.eval(
                r#"(function(){
    setInterval(function(){
        var img=new Image();
        img.onload=function(){window.location.href='https://www.messenger.com';};
        img.onerror=function(){};
        img.src='https://static.xx.fbcdn.net/rsrc.php/v4/yJ/r/bWHaFYtfBCe.png?'+Date.now();
    },15000);
})()"#,
            );
        });
    }

    // ------------------------------------------------------------------
    // 6. System tray icon.
    // ------------------------------------------------------------------
    if let Some(icon) = app.default_window_icon() {
        let tray_app_handle = app.handle().clone();
        match tauri::tray::TrayIconBuilder::with_id("messengerx-tray")
            .icon(icon.clone())
            .tooltip(&tr.tray_tooltip)
            .on_tray_icon_event(move |_tray, event| {
                if let tauri::tray::TrayIconEvent::Click { .. } = event {
                    if let Some(window) = tray_app_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                }
            })
            .build(app)
        {
            Ok(_tray) => {}
            Err(e) => log::warn!("[MessengerX] Failed to create tray icon: {e}"),
        }
    }

    // ------------------------------------------------------------------
    // 7. Periodic snapshot timer (every 60 seconds).
    //    The JS itself guards against offline/error pages (see
    //    `SNAPSHOT_TRIGGER_SCRIPT`).
    // ------------------------------------------------------------------
    let snapshot_webview = webview.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
        if let Err(e) = snapshot_webview.eval(SNAPSHOT_TRIGGER_SCRIPT) {
            log::warn!("[MessengerX] Failed to trigger snapshot: {e}");
        }
    });

    Ok(())
}
