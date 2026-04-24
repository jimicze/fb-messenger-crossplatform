//! Library root for the Messenger X cross-platform desktop client.
//!
//! Registers Tauri plugins, builds the main window programmatically (so that
//! custom `initialization_script`s and `on_navigation` hooks can be attached),
//! and wires up the IPC invoke-handler.

mod commands;
mod services;

use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
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
    function jlog(msg) {
        try {
            window.__TAURI__.core.invoke('js_log', { message: '[NotificationJS] ' + msg });
        } catch(_) {}
    }

    function preview(value) {
        var text = String(value == null ? '' : value);
        return text.length > 120 ? text.slice(0, 117) + '...' : text;
    }

    function safeHasFocus() {
        try {
            return typeof document.hasFocus === 'function' ? document.hasFocus() : 'unknown';
        } catch(_) {
            return 'error';
        }
    }

    window.Notification = function(title, options) {
        try {
            var body = options?.body || '';
            var tag = options?.tag || '';
            var silent = options?.silent || false;

            jlog(
                'constructor called: title=' + JSON.stringify(preview(title)) +
                ' body=' + JSON.stringify(preview(body)) +
                ' tag=' + JSON.stringify(preview(tag)) +
                ' silent=' + String(silent) +
                ' visibility=' + String(document.visibilityState) +
                ' hidden=' + String(document.hidden) +
                ' hasFocus=' + String(safeHasFocus())
            );

            window.__TAURI__.core.invoke('send_notification', {
                title: title,
                body: body,
                tag: tag,
                silent: silent
            }).then(function() {
                jlog('send_notification resolved');
            }).catch(function(e) {
                jlog('send_notification rejected: ' + e);
                console.error('[MessengerX] Failed to send notification:', e);
            });
        } catch(e) {
            jlog('constructor failed: ' + e);
            console.error('[MessengerX] Failed to send notification:', e);
        }
        return {
            close: function() {},
            addEventListener: function() {},
            removeEventListener: function() {},
            onclick: null, onerror: null, onclose: null, onshow: null
        };
    };
    window.Notification.permission = 'granted';
    window.Notification.requestPermission = function(callback) {
        jlog('requestPermission called');
        if (callback) callback('granted');
        return Promise.resolve('granted');
    };
    window.Notification.__native__ = true;
    jlog('window.Notification override installed');
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

/// Builds the scrollbar fix injection script with platform-specific behaviour.
///
/// **Root cause (discovered after 6 failed CSS attempts):**
/// The coloured strip in the Messenger chat area is NOT a scrollbar at all.
/// It is the `background: linear-gradient(…)` of the chat-theme container
/// (857 × 382 px) showing through the *transparent* macOS WKWebView overlay
/// scrollbar area.  Because the overlay scrollbar floats over content without
/// its own opaque track, any container background bleeds through.
///
/// `scrollbar-color` / `accent-color` CSS had zero effect because the colour
/// was never coming from a scrollbar property to begin with.
///
/// **Fix on macOS:** `scrollbar-width: none !important` hides the native
/// overlay scrollbar entirely, eliminating the transparent strip where the
/// gradient was visible.  Messenger's own custom div-based scrollbars (left
/// chat-list panel) are unaffected — they are ordinary `<div>` elements, not
/// native scrollbars.  Trackpad / scroll-wheel navigation continues to work.
///
/// **Fix on Windows / Linux:** Custom `::-webkit-scrollbar` rules replace the
/// default scrollbar rendering with a neutral grey style.  `scrollbar-width`
/// is intentionally NOT set to none on these platforms because persistent
/// scrollbars are part of the expected UI on those OSes.
fn build_scrollbar_fix_script(is_macos: bool) -> String {
    // On macOS: hide the native overlay scrollbar so it cannot reveal the
    // gradient chat-theme background behind it.
    // On other platforms: keep the scrollbar but style it neutral grey.
    let scrollbar_width_rule = if is_macos {
        "scrollbar-width: none !important;"
    } else {
        "scrollbar-color: rgba(128,128,128,0.5) transparent !important;"
    };

    format!(
        r#"(function() {{
    var STYLE_ID = '__messengerx_scrollbar_fix__';

    function createStyle() {{
        var style = document.createElement('style');
        style.id = STYLE_ID;
        style.textContent = `
            /* MessengerX — scrollbar fix.
             *
             * macOS: scrollbar-width:none hides the transparent overlay
             *   scrollbar so the chat-theme gradient behind it is invisible.
             * Win/Linux: neutral grey scrollbar-color instead of theme accent.
             * Specificity 2,0,0 beats any Messenger class selector chain.
             */
            *:not(#__msrx__):not(#__msrx__),
            *:not(#__msrx__):not(#__msrx__)::before,
            *:not(#__msrx__):not(#__msrx__)::after {{
                {scrollbar_width_rule}
            }}

            /* Chromium-based WebViews (Windows WebView2, Linux WebKitGTK). */
            ::-webkit-scrollbar {{ width: 8px; height: 8px; }}
            ::-webkit-scrollbar-track {{ background: transparent !important; }}
            ::-webkit-scrollbar-thumb {{
                background: rgba(128, 128, 128, 0.45) !important;
                border-radius: 4px;
            }}
            ::-webkit-scrollbar-thumb:hover {{
                background: rgba(128, 128, 128, 0.65) !important;
            }}
            ::-webkit-scrollbar-corner {{ background: transparent !important; }}
        `;
        return style;
    }}

    function ensureStyleLast() {{
        var parent = document.head || document.documentElement;
        var existing = document.getElementById(STYLE_ID);
        if (existing) {{
            if (existing !== parent.lastElementChild) {{
                parent.appendChild(existing);
            }}
        }} else {{
            parent.appendChild(createStyle());
        }}
    }}

    function setup() {{
        ensureStyleLast();

        /* Re-append whenever Messenger lazily loads new stylesheets. */
        new MutationObserver(function(mutations) {{
            for (var i = 0; i < mutations.length; i++) {{
                var added = mutations[i].addedNodes;
                for (var j = 0; j < added.length; j++) {{
                    var n = added[j];
                    if (n.nodeType === 1 && n.id !== STYLE_ID) {{
                        var tag = n.tagName;
                        if (tag === 'STYLE' || tag === 'LINK') {{
                            ensureStyleLast();
                            return;
                        }}
                    }}
                }}
            }}
        }}).observe(document.head || document.documentElement, {{ childList: true }});

        /* Sync color-scheme with Messenger dark/light mode so native
         * controls (e.g. input fields) use the correct contrast. */
        function syncColorScheme() {{
            var isDark = !!document.querySelector('.__fb-dark-mode');
            document.documentElement.style.setProperty(
                'color-scheme', isDark ? 'dark' : 'light', 'important'
            );
        }}
        syncColorScheme();
        new MutationObserver(syncColorScheme).observe(
            document.body || document.documentElement,
            {{ attributes: true, subtree: true, attributeFilter: ['class'] }}
        );
    }}

    if (document.readyState === 'loading') {{
        document.addEventListener('DOMContentLoaded', setup);
    }} else {{
        setup();
    }}
}})();
"#,
        scrollbar_width_rule = scrollbar_width_rule
    )
}

/// Overrides `window.open()` to intercept external links that Messenger opens
/// in a new tab/window.  Messenger is a React SPA that calls `window.open()`
/// for external URLs rather than navigating the main frame, so the Tauri
/// `on_navigation` callback never fires for those.
///
/// Also installs a capture-phase `click` listener on `document` as a fallback
/// for external `<a href>` links that Messenger doesn't route through
/// `window.open()` (e.g. direct anchor tags in chat).
///
/// All debug messages are forwarded to the Rust log file via `js_log` so they
/// are visible in `messengerx.log` (not just in the WebKit inspector).
///
/// Injected **at document-start** (before the page HTML is parsed).
const WINDOW_OPEN_OVERRIDE_SCRIPT: &str = r#"
(function() {
    const ALLOWED_DOMAINS = [
        'messenger.com', 'facebook.com', 'fbcdn.net', 'fbsbx.com',
        // Google domains required for the Facebook login reCAPTCHA flow.
        // Facebook redirects to accounts.google.com / recaptcha.google.com
        // during login verification — these must render inside the WebView.
        'google.com', 'gstatic.com', 'recaptcha.net'
    ];

    function jlog(msg) {
        try { window.__TAURI__.core.invoke('js_log', { message: msg }); } catch(_) {}
    }

    function isAllowedUrl(url) {
        try {
            var host = new URL(url).hostname;
            return ALLOWED_DOMAINS.some(function(d) {
                return host === d || host.endsWith('.' + d);
            });
        } catch(e) {
            return true; // non-parseable URL — let it through
        }
    }

    function openExternal(urlStr) {
        jlog('External URL — routing to system browser: ' + urlStr);
        try {
            window.__TAURI__.core.invoke('open_external', { url: urlStr });
        } catch(e) {
            jlog('Failed to open external URL via invoke: ' + e);
        }
    }

    // -----------------------------------------------------------------------
    // 1. window.open() override
    // -----------------------------------------------------------------------
    var _originalOpen = window.open;
    window.open = function(url, target, features) {
        var urlStr = url ? String(url) : '';
        jlog('window.open intercepted: url=' + urlStr + ' target=' + target);

        if (urlStr && (urlStr.startsWith('http://') || urlStr.startsWith('https://'))) {
            if (!isAllowedUrl(urlStr)) {
                openExternal(urlStr);
                return null;
            }
        }
        return _originalOpen.call(this, url, target, features);
    };

    // -----------------------------------------------------------------------
    // 2. Capture-phase <a href> click interceptor (fallback for anchor tags)
    // -----------------------------------------------------------------------
    document.addEventListener('click', function(e) {
        var el = e.target;
        // Walk up the DOM to find the nearest <a> ancestor.
        while (el && el.tagName !== 'A') { el = el.parentElement; }
        if (!el) return;
        var href = el.getAttribute('href');
        if (!href) return;

        // Resolve to absolute URL.
        var urlStr;
        try { urlStr = new URL(href, window.location.href).toString(); } catch(_) { return; }
        jlog('click interceptor: href=' + urlStr);

        if (!urlStr.startsWith('http://') && !urlStr.startsWith('https://')) return;
        if (isAllowedUrl(urlStr)) return; // allowed — let Tauri handle it

        e.preventDefault();
        e.stopPropagation();
        openExternal(urlStr);
    }, true /* capture phase */);

    jlog('window.open override + click interceptor installed');
})();
"#;

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
// Helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Linux-only: Visibility API shim
// ---------------------------------------------------------------------------
/// On WebKitGTK (Linux) `document.visibilityState` stays `'visible'` even when
/// the window is minimised or obscured, because GTK widget visibility is not
/// coupled to window iconification.  As a result, Messenger never fires
/// `new Notification()` – it assumes the user is watching the UI.
///
/// This script overrides the Visibility API and `document.hasFocus()` with a
/// flag that Rust controls via `window.__messengerx_set_visible(bool)`.
/// Two Rust-side paths drive the flag at runtime (see section 4d of
/// `setup_webview`):
///
/// * **`WindowEvent::Focused`** – fast path, works on most WMs (GNOME/Mutter,
///   KDE/KWin) that emit a focus-lost event when the window is iconified.
/// * **`is_minimized()` poll** – 500 ms background thread that catches WMs
///   (e.g. Linux Mint/Muffin) that do NOT emit a `Focused(false)` event on
///   iconification.  The setter is idempotent, so double-calling is harmless.
///
/// The setter also dispatches synthetic `focus` / `blur` events alongside
/// `visibilitychange` because Messenger's notification gate appears to consult
/// both focus and visibility signals on Linux.
///
/// On **every main-frame page load** the script also calls `get_window_focused`
/// via the Tauri IPC bridge to immediately resync `_hidden` from the actual OS
/// window state. This is necessary because `initialization_script` runs on
/// every navigation (not just the first load), so baking a startup preference
/// into the initial value would corrupt the flag after any re-navigation (e.g.
/// a logout that loads the login page) when the window is already focused.

/// Poll interval (ms) for the Linux background thread that derives Messenger's
/// effective visibility from `is_focused()` + `is_minimized()`.
/// 500 ms gives a worst-case detection latency that is imperceptible to users
/// while keeping CPU overhead negligible (two Tauri window-state queries per
/// half-second).
#[cfg(target_os = "linux")]
const WINDOW_STATE_POLL_INTERVAL_MS: u64 = 500;

#[cfg(target_os = "linux")]
const VISIBILITY_OVERRIDE_SCRIPT: &str = r#"(function() {
    var _hidden = false;

    function jlog(msg) {
        try {
            window.__TAURI__.core.invoke('js_log', { message: '[VisibilityJS] ' + msg });
        } catch(_) {}
    }

    function dispatchFocusChange(visible) {
        var type = visible ? 'focus' : 'blur';
        window.dispatchEvent(new Event(type));
        document.dispatchEvent(new Event(type));
    }

    function setVisible(visible, source) {
        source = source || 'unknown';
        if (_hidden === !visible) {
            jlog('setVisible no-op: source=' + source + ' visible=' + String(visible));
            return;
        }
        _hidden = !visible;
        jlog(
            'setVisible changed: source=' + source +
            ' visible=' + String(visible) +
            ' hidden=' + String(_hidden)
        );
        document.dispatchEvent(new Event('visibilitychange'));
        dispatchFocusChange(visible);
    }

    Object.defineProperty(document, 'visibilityState', {
        get: function() { return _hidden ? 'hidden' : 'visible'; },
        configurable: true
    });
    Object.defineProperty(document, 'hidden', {
        get: function() { return _hidden; },
        configurable: true
    });

    try {
        Object.defineProperty(document, 'hasFocus', {
            value: function() { return !_hidden; },
            configurable: true
        });
    } catch(_) {
        document.hasFocus = function() { return !_hidden; };
    }

    window.__messengerx_set_visible = setVisible;
    window.__TAURI__.core.invoke('get_window_focused')
        .then(function(focused) {
            jlog('initial get_window_focused=' + String(focused));
            setVisible(focused, 'ipc-resync');
        })
        .catch(function(e) {
            jlog('get_window_focused failed: ' + e);
        });
    jlog('visibility override installed');
})();"#;

/// Persists the current Unix timestamp (seconds) as `last_update_check_secs`
/// in the user's settings file.
///
/// Called after every update check (manual or automatic) so that the
/// 30-day automatic-check window is correctly tracked.
fn save_check_timestamp(handle: &tauri::AppHandle) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut s = services::auth::load_settings(handle).unwrap_or_default();
    s.last_update_check_secs = Some(now);
    let _ = services::auth::save_settings(handle, &s);
}

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
        .plugin(
            tauri_plugin_log::Builder::new()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("messengerx".to_string()),
                    },
                ))
                .max_file_size(500_000)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
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
            commands::check_for_update,
            commands::install_update,
            commands::set_autostart,
            commands::is_autostart_enabled,
            commands::js_log,
            commands::get_window_focused,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // macOS: clicking the Dock icon while all windows are hidden should
            // bring the main window back (applicationShouldHandleReopen).
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } = event
            {
                if !has_visible_windows {
                    if let Some(window) = app_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }
        });
}

// ---------------------------------------------------------------------------
// Setup helper (split out so the closure stays readable)
// ---------------------------------------------------------------------------

/// Performs one-time application setup inside the Tauri `setup` hook.
fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    if let Err(e) = services::notification::initialize() {
        log::warn!("[MessengerX] Failed to initialize native notifications: {e}");
    }

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

    // Build a zoom-control init script that defines __messenger_setZoom
    // as a no-op stub for backward compatibility (the actual zoom is now
    // handled via the native webview.set_zoom() API, see below).
    // NOTE: We no longer use CSS body.style.zoom because it causes layout
    // issues — the page content doesn't fill the viewport at zoom < 100%.
    let zoom_init_script = "(function() { window.__messenger_setZoom = function() {}; })();"
        .to_string();

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

    // Build the scrollbar-fix script with platform-specific behaviour.
    // On macOS the native overlay scrollbar is hidden (scrollbar-width:none)
    // because the transparent scrollbar track was revealing the chat-theme
    // gradient behind it. On other platforms we use custom webkit-scrollbar.
    let scrollbar_fix_script = build_scrollbar_fix_script(cfg!(target_os = "macos"));

    // ------------------------------------------------------------------
    // 4. Build the main window programmatically.
    // ------------------------------------------------------------------
    let builder = WebviewWindowBuilder::new(app, "main", webview_url)
        .title("Messenger X")
        .inner_size(1200.0, 800.0)
        .min_inner_size(400.0, 300.0)
        .resizable(true)
        .visible(!settings.start_minimized)
        // Inject all JS at document-start.
        .initialization_script(NOTIFICATION_OVERRIDE_SCRIPT)
        .initialization_script(UNREAD_OBSERVER_SCRIPT)
        .initialization_script(OFFLINE_DIALOG_HIDER_SCRIPT)
        .initialization_script(&offline_banner_script)
        .initialization_script(&zoom_init_script)
        .initialization_script(&scrollbar_fix_script)
        .initialization_script(WINDOW_OPEN_OVERRIDE_SCRIPT);

    // On Linux, inject the Visibility API shim so that Rust can push the real
    // focus state and Messenger correctly fires desktop notifications when the
    // window is not active.  On each page load the shim immediately resyncs
    // _hidden from the actual OS window-focus state via IPC (get_window_focused)
    // so that re-navigations (e.g. logout) never inherit a stale startup value.
    #[cfg(target_os = "linux")]
    let builder = builder.initialization_script(VISIBILITY_OVERRIDE_SCRIPT);

    let webview = builder
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
            log::info!("[MessengerX] on_navigation: host={host} url={url}");

            // Handle Facebook's link shim: l.facebook.com/l.php?u=EXTERNAL_URL
            // Messenger wraps all external links in this shim — so `on_navigation`
            // sees a facebook.com URL instead of the real destination.  We extract
            // the actual URL from the `u` query param and open it in the system browser.
            if (host == "l.facebook.com" || host == "l.messenger.com")
                && url.path() == "/l.php"
            {
                if let Some(actual_url) = url
                    .query_pairs()
                    .find(|(k, _)| k == "u")
                    .map(|(_, v)| v.into_owned())
                {
                    log::info!("[MessengerX] Link shim detected — opening real URL: {actual_url}");
                    let handle = nav_app_handle.clone();
                    std::thread::spawn(move || {
                        use tauri_plugin_opener::OpenerExt;
                        if let Err(e) = handle.opener().open_url(&actual_url, None::<&str>) {
                            log::warn!(
                                "[MessengerX] Failed to open link-shim URL {actual_url}: {e}"
                            );
                        }
                    });
                    return false;
                }
            }

            // Google domains are required for the Facebook login reCAPTCHA flow:
            // Facebook redirects to accounts.google.com / recaptcha.google.com
            // during login verification.  These pages must render inside the
            // WebView (where session cookies exist) — opening them in the system
            // browser breaks the flow because the browser has no session context.
            const ALLOWED: &[&str] = &[
                "messenger.com",
                "facebook.com",
                "fbcdn.net",
                "fbsbx.com",
                // Google auth & reCAPTCHA (needed for FB login verification)
                "google.com",
                "gstatic.com",
                "recaptcha.net",
            ];
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
    // 4b. Apply persisted zoom level via the native WebView API.
    //     This scales the entire viewport (not just body content), so the
    //     page layout fills the window correctly at all zoom levels.
    // ------------------------------------------------------------------
    if (zoom_level - 1.0).abs() > f64::EPSILON {
        if let Err(e) = webview.set_zoom(zoom_level) {
            log::warn!("[MessengerX] Failed to apply initial zoom {zoom_level}: {e}");
        }
    }

    // ------------------------------------------------------------------
    // 4c. macOS close-button behaviour: hide the window instead of quitting.
    //     On macOS, clicking the red traffic-light close button is expected
    //     to hide the window (app stays in the Dock / app menu).  The user
    //     quits explicitly via Messenger X → Quit (⌘Q).
    //     Windows and Linux keep the default behaviour (close = quit).
    // ------------------------------------------------------------------
    #[cfg(target_os = "macos")]
    {
        let close_webview = webview.clone();
        webview.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = close_webview.hide();
            }
        });
    }

    // ------------------------------------------------------------------
    // 4d. Linux: push real OS-focus state into the Visibility API shim.
    //     WebKitGTK does not update document.visibilityState when the GTK
    //     window is iconified, so Messenger never fires notifications.
    //
    //     Two complementary paths cover different WM behaviours:
    //     (a) WindowEvent::Focused – fires immediately on WMs that emit a
    //         focus-change event when the window loses or gains focus (e.g.
    //         GNOME/Mutter, KDE/KWin).
    //     (b) window-state poll – a 500 ms background thread that derives the
    //         effective notification-visible state from `is_focused()` AND
    //         `!is_minimized()`. This covers WMs (e.g. Linux Mint / Muffin)
    //         that do NOT reliably emit a Focused(false) event on iconify.
    //         __messengerx_set_visible is idempotent, so double-calling from
    //         both paths is harmless.
    // ------------------------------------------------------------------
    #[cfg(target_os = "linux")]
    {
        // Path (a): focus events (fast path, most WMs).
        let vis_webview = webview.clone();
        webview.on_window_event(move |event| {
            if let tauri::WindowEvent::Focused(focused) = event {
                log::info!(
                    "[MessengerX][Visibility] WindowEvent::Focused({focused})"
                );
                let js = if *focused {
                    "window.__messengerx_set_visible?.(true, 'window-event-focused')"
                } else {
                    "window.__messengerx_set_visible?.(false, 'window-event-blurred')"
                };
                if let Err(e) = vis_webview.eval(js) {
                    log::warn!(
                        "[MessengerX][Visibility] Failed to eval focus-event visibility sync: {e}"
                    );
                }
            }
        });

        // Path (b): derive effective visibility from focused + minimized state.
        // This covers WMs that skip Focused events on iconify/backgrounding
        // (e.g. Linux Mint / Muffin). The loop exits automatically when the
        // window is destroyed (window-state query returns Err), which Tauri
        // guarantees on app shutdown before the process exits, so no JoinHandle
        // or explicit cancellation token is needed.
        let poll_webview = webview.clone();
        std::thread::spawn(move || {
            let Ok(initially_focused) = poll_webview.is_focused() else {
                log::info!(
                    "[MessengerX][Visibility] Poll thread exiting before start: is_focused() unavailable"
                );
                return;
            };
            let Ok(initially_minimized) = poll_webview.is_minimized() else {
                log::info!(
                    "[MessengerX][Visibility] Poll thread exiting before start: is_minimized() unavailable"
                );
                return;
            };
            let mut was_visible = initially_focused && !initially_minimized;
            log::info!(
                "[MessengerX][Visibility] Poll thread started: focused={} minimized={} visible={}",
                initially_focused,
                initially_minimized,
                was_visible
            );

            loop {
                std::thread::sleep(std::time::Duration::from_millis(
                    WINDOW_STATE_POLL_INTERVAL_MS,
                ));
                let Ok(is_focused) = poll_webview.is_focused() else {
                    log::info!(
                        "[MessengerX][Visibility] Poll thread exiting: is_focused() unavailable"
                    );
                    break;
                };
                let Ok(is_minimized) = poll_webview.is_minimized() else {
                    log::info!(
                        "[MessengerX][Visibility] Poll thread exiting: is_minimized() unavailable"
                    );
                    break;
                };
                let is_visible = is_focused && !is_minimized;
                if is_visible != was_visible {
                    was_visible = is_visible;
                    log::info!(
                        "[MessengerX][Visibility] Poll state changed: focused={} minimized={} visible={}",
                        is_focused,
                        is_minimized,
                        is_visible
                    );
                    let js = if is_visible {
                        "window.__messengerx_set_visible?.(true, 'poll-focused-unminimized')"
                    } else {
                        "window.__messengerx_set_visible?.(false, 'poll-unfocused-or-minimized')"
                    };
                    if let Err(e) = poll_webview.eval(js) {
                        log::warn!(
                            "[MessengerX][Visibility] Failed to eval poll-based visibility sync: {e}"
                        );
                    }
                }
            }
        });
    }

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
    // 6. System tray icon with context menu (Windows / Linux only).
    //    On macOS the tray icon is omitted — all settings live in the
    //    native app menu bar instead (see section 6b below).
    // ------------------------------------------------------------------
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        use tauri::menu::CheckMenuItemBuilder;

        // Load current autostart state from the OS (may differ from saved).
        let autostart_checked = {
            use tauri_plugin_autostart::ManagerExt;
            app.autolaunch()
                .is_enabled()
                .unwrap_or(settings.autostart)
        };

        // --- Build menu items ---

        // Non-interactive version label at the top of the menu.
        let version_str = format!("v{}", app.package_info().version);
        let version_item = MenuItemBuilder::with_id("app_version", &version_str)
            .enabled(false)
            .build(app)?;

        let show_item =
            MenuItemBuilder::with_id("tray_show", &tr.tray_show).build(app)?;

        // Toggles
        let stay_logged_in_item = CheckMenuItemBuilder::with_id(
            "stay_logged_in",
            &tr.settings_stay_logged_in,
        )
        .checked(settings.stay_logged_in)
        .build(app)?;

        let notifications_item = CheckMenuItemBuilder::with_id(
            "notifications_enabled",
            &tr.settings_notifications_enabled,
        )
        .checked(settings.notifications_enabled)
        .build(app)?;

        let notification_sound_item = CheckMenuItemBuilder::with_id(
            "notification_sound",
            &tr.settings_notification_sound,
        )
        .checked(settings.notification_sound)
        .build(app)?;

        // Zoom submenu with radio-like CheckMenuItems (60%-120%)
        let zoom_levels: &[(u32, &str)] = &[
            (60, "60%"),
            (70, "70%"),
            (80, "80%"),
            (90, "90%"),
            (100, "100%"),
            (110, "110%"),
            (120, "120%"),
        ];
        let current_zoom_pct = (settings.zoom_level * 100.0).round() as u32;
        let mut zoom_checks: Vec<tauri::menu::CheckMenuItem<tauri::Wry>> = Vec::new();
        for &(pct, label) in zoom_levels {
            let item = CheckMenuItemBuilder::with_id(format!("zoom_{pct}"), label)
                .checked(pct == current_zoom_pct)
                .build(app)?;
            zoom_checks.push(item);
        }

        let mut zoom_builder = SubmenuBuilder::new(app, &tr.settings_zoom_level);
        for item in &zoom_checks {
            zoom_builder = zoom_builder.item(item);
        }
        let zoom_submenu = zoom_builder.build()?;

        // Startup toggles
        let autostart_item = CheckMenuItemBuilder::with_id(
            "autostart",
            &tr.settings_autostart,
        )
        .checked(autostart_checked)
        .build(app)?;

        let start_minimized_item = CheckMenuItemBuilder::with_id(
            "start_minimized",
            &tr.settings_start_minimized,
        )
        .checked(settings.start_minimized)
        .build(app)?;

        let auto_update_item = CheckMenuItemBuilder::with_id(
            "auto_update",
            &tr.settings_auto_update,
        )
        .checked(settings.auto_update)
        .build(app)?;

        // Action items
        let check_update_item = MenuItemBuilder::with_id(
            "check_update",
            &tr.settings_check_update,
        )
        .build(app)?;

        let view_logs_item =
            MenuItemBuilder::with_id("view_logs", &tr.settings_view_logs).build(app)?;

        let clear_logs_item =
            MenuItemBuilder::with_id("clear_logs", &tr.settings_clear_logs).build(app)?;

        let logout_item =
            MenuItemBuilder::with_id("logout", &tr.settings_logout).build(app)?;

        let quit_item =
            MenuItemBuilder::with_id("tray_quit", &tr.tray_quit).build(app)?;

        // --- Separators ---
        let sep1 = PredefinedMenuItem::separator(app)?;
        let sep2 = PredefinedMenuItem::separator(app)?;
        let sep3 = PredefinedMenuItem::separator(app)?;
        let sep4 = PredefinedMenuItem::separator(app)?;
        let sep5 = PredefinedMenuItem::separator(app)?;
        let sep6 = PredefinedMenuItem::separator(app)?;

        // --- Assemble tray menu ---
        let tray_menu = MenuBuilder::new(app)
            .item(&version_item)
            .item(&sep1)
            .item(&show_item)
            .item(&sep2)
            .item(&stay_logged_in_item)
            .item(&notifications_item)
            .item(&notification_sound_item)
            .item(&sep3)
            .item(&zoom_submenu)
            .item(&sep4)
            .item(&autostart_item)
            .item(&start_minimized_item)
            .item(&auto_update_item)
            .item(&sep5)
            .item(&check_update_item)
            .item(&view_logs_item)
            .item(&clear_logs_item)
            .item(&sep6)
            .item(&logout_item)
            .separator()
            .item(&quit_item)
            .build()?;

        // --- Clone items for the menu-event closure ---
        let tray_click_handle = app.handle().clone();
        let tray_menu_handle = app.handle().clone();

        let stay_logged_in_c = stay_logged_in_item.clone();
        let notifications_c = notifications_item.clone();
        let notification_sound_c = notification_sound_item.clone();
        let autostart_c = autostart_item.clone();
        let start_minimized_c = start_minimized_item.clone();
        let auto_update_c = auto_update_item.clone();
        let zoom_checks_c = zoom_checks.clone();

        // Translated strings for update-check notifications.
        let tr_update_available = tr.settings_update_available.clone();
        let tr_no_update = tr.settings_no_update.clone();
        let tr_update_error = tr.settings_update_error.clone();
        let tr_update_ready = tr.settings_update_ready.clone();
        // Translated strings for the update confirmation dialog.
        let tr_update_dialog_title = tr.settings_update_dialog_title.clone();
        let tr_update_dialog_body = tr.settings_update_dialog_body.clone();
        let tr_update_install_btn = tr.settings_update_install_btn.clone();
        let tr_update_later_btn = tr.settings_update_later_btn.clone();

        match tauri::tray::TrayIconBuilder::with_id("messengerx-tray")
            .icon(icon.clone())
            .tooltip(&tr.tray_tooltip)
            .menu(&tray_menu)
            .show_menu_on_left_click(false)
            .on_tray_icon_event(move |_tray, event| {
                use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    if let Some(window) = tray_click_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                }
            })
            .on_menu_event(move |_tray, event| {
                let handle = &tray_menu_handle;
                let id = event.id().as_ref();

                match id {
                    // ---- Show window ----
                    "tray_show" => {
                        if let Some(window) = handle.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }

                    // ---- Toggle: stay logged in ----
                    "stay_logged_in" => {
                        if let Ok(checked) = stay_logged_in_c.is_checked() {
                            let mut s =
                                services::auth::load_settings(handle).unwrap_or_default();
                            s.stay_logged_in = checked;
                            let _ = services::auth::save_settings(handle, &s);
                        }
                    }

                    // ---- Toggle: notifications enabled ----
                    "notifications_enabled" => {
                        if let Ok(checked) = notifications_c.is_checked() {
                            let mut s =
                                services::auth::load_settings(handle).unwrap_or_default();
                            s.notifications_enabled = checked;
                            let _ = services::auth::save_settings(handle, &s);
                        }
                    }

                    // ---- Toggle: notification sound ----
                    "notification_sound" => {
                        if let Ok(checked) = notification_sound_c.is_checked() {
                            let mut s =
                                services::auth::load_settings(handle).unwrap_or_default();
                            s.notification_sound = checked;
                            let _ = services::auth::save_settings(handle, &s);
                        }
                    }

                    // ---- Toggle: autostart ----
                    "autostart" => {
                        if let Ok(checked) = autostart_c.is_checked() {
                            use tauri_plugin_autostart::ManagerExt;
                            let autolaunch = handle.autolaunch();
                            if checked {
                                let _ = autolaunch.enable();
                            } else {
                                let _ = autolaunch.disable();
                            }
                            let mut s =
                                services::auth::load_settings(handle).unwrap_or_default();
                            s.autostart = checked;
                            let _ = services::auth::save_settings(handle, &s);
                        }
                    }

                    // ---- Toggle: start minimized ----
                    "start_minimized" => {
                        if let Ok(checked) = start_minimized_c.is_checked() {
                            let mut s =
                                services::auth::load_settings(handle).unwrap_or_default();
                            s.start_minimized = checked;
                            let _ = services::auth::save_settings(handle, &s);
                        }
                    }

                    // ---- Toggle: auto-update ----
                    "auto_update" => {
                        if let Ok(checked) = auto_update_c.is_checked() {
                            let mut s =
                                services::auth::load_settings(handle).unwrap_or_default();
                            s.auto_update = checked;
                            let _ = services::auth::save_settings(handle, &s);
                        }
                    }

                    // ---- Zoom (radio-like behaviour) ----
                    _ if id.starts_with("zoom_") => {
                        if let Ok(pct) = id[5..].parse::<u32>() {
                            let level = pct as f64 / 100.0;
                            // Enforce radio: uncheck all, check selected.
                            for zitem in &zoom_checks_c {
                                let _ = zitem.set_checked(zitem.id().as_ref() == id);
                            }
                            // Apply native zoom.
                            if let Some(wv) = handle.get_webview_window("main") {
                                let _ = wv.set_zoom(level);
                            }
                            // Persist.
                            let mut s =
                                services::auth::load_settings(handle).unwrap_or_default();
                            s.zoom_level = level;
                            let _ = services::auth::save_settings(handle, &s);
                        }
                    }

                    // ---- Check for updates ----
                    "check_update" => {
                        let h = handle.clone();
                        let tr_avail = tr_update_available.clone();
                        let tr_none = tr_no_update.clone();
                        let tr_err = tr_update_error.clone();
                        let tr_ready = tr_update_ready.clone();
                        let tr_dlg_title = tr_update_dialog_title.clone();
                        let tr_dlg_body = tr_update_dialog_body.clone();
                        let tr_install = tr_update_install_btn.clone();
                        let tr_later = tr_update_later_btn.clone();
                        tauri::async_runtime::spawn(async move {
                            use tauri_plugin_updater::UpdaterExt;
                            match h.updater() {
                                Ok(updater) => match updater.check().await {
                                    Ok(Some(update)) => {
                                        let ver = update.version.clone();
                                        let body = tr_dlg_body.replace("{}", &ver);
                                        save_check_timestamp(&h);
                                        // Show confirmation dialog before installing.
                                        use tauri_plugin_dialog::{
                                            DialogExt, MessageDialogButtons,
                                        };
                                        let h2 = h.clone();
                                        let tr_ready2 = tr_ready.clone();
                                        let tr_err2 = tr_err.clone();
                                        h.dialog()
                                            .message(&body)
                                            .title(&tr_dlg_title)
                                            .buttons(MessageDialogButtons::OkCancelCustom(
                                                tr_install,
                                                tr_later,
                                            ))
                                            .show(move |confirmed| {
                                                if confirmed {
                                                    tauri::async_runtime::spawn(async move {
                                                        match update
                                                            .download_and_install(
                                                                |_, _| {},
                                                                || {},
                                                            )
                                                            .await
                                                        {
                                                            Ok(()) => {
                                                                let _ = services::notification::show_notification(
                                                                    &h2,
                                                                    "Messenger X",
                                                                    &tr_ready2,
                                                                    "update",
                                                                    false,
                                                                );
                                                                h2.restart();
                                                            }
                                                            Err(e) => {
                                                                log::warn!(
                                                                    "[MessengerX] Update install failed: {e}"
                                                                );
                                                                let _ = services::notification::show_notification(
                                                                    &h2,
                                                                    "Messenger X",
                                                                    &tr_err2,
                                                                    "update",
                                                                    false,
                                                                );
                                                            }
                                                        }
                                                    });
                                                } else {
                                                    log::info!(
                                                        "[MessengerX] Update v{ver} deferred by user"
                                                    );
                                                }
                                            });
                                    }
                                    Ok(None) => {
                                        save_check_timestamp(&h);
                                        let _ = services::notification::show_notification(
                                            &h, "Messenger X", &tr_none, "update", false,
                                        );
                                    }
                                    Err(e) => {
                                        log::warn!(
                                            "[MessengerX] Update check failed: {e}"
                                        );
                                        let _ = services::notification::show_notification(
                                            &h, "Messenger X", &tr_err, "update", false,
                                        );
                                    }
                                },
                                Err(e) => {
                                    log::warn!("[MessengerX] Updater init failed: {e}");
                                    let _ = services::notification::show_notification(
                                        &h, "Messenger X", &tr_err, "update", false,
                                    );
                                }
                            }
                        });
                    }

                    // ---- View Logs ----
                    "view_logs" => {
                        use tauri_plugin_opener::OpenerExt;
                        if let Ok(log_dir) = handle.path().app_log_dir() {
                            let log_file = log_dir.join("messengerx.log");
                            if log_file.exists() {
                                let _ = handle.opener().open_path(
                                    log_file.to_string_lossy().into_owned(),
                                    None::<&str>,
                                );
                            } else {
                                let _ = std::fs::create_dir_all(&log_dir);
                                let _ = handle.opener().open_path(
                                    log_dir.to_string_lossy().into_owned(),
                                    None::<&str>,
                                );
                            }
                        }
                    }

                    // ---- Clear Logs ----
                    "clear_logs" => {
                        if let Ok(log_dir) = handle.path().app_log_dir() {
                            for name in &["messengerx.log", "messengerx.log.old"] {
                                let path = log_dir.join(name);
                                if path.exists() {
                                    if let Err(e) = std::fs::remove_file(&path) {
                                        log::warn!("[MessengerX] Failed to delete {}: {e}", path.display());
                                    } else {
                                        log::info!("[MessengerX] Deleted log file: {}", path.display());
                                    }
                                }
                            }
                        }
                    }

                    // ---- Log out & clear data ----
                    "logout" => {
                        let defaults = crate::commands::AppSettings::default();
                        let _ = services::auth::save_settings(handle, &defaults);

                        // Reset all checkbox states to defaults.
                        let _ = stay_logged_in_c.set_checked(defaults.stay_logged_in);
                        let _ = notifications_c.set_checked(defaults.notifications_enabled);
                        let _ = notification_sound_c.set_checked(defaults.notification_sound);
                        let _ = start_minimized_c.set_checked(defaults.start_minimized);
                        let _ = auto_update_c.set_checked(defaults.auto_update);

                        // Reset zoom to 100 %.
                        for zitem in &zoom_checks_c {
                            let _ =
                                zitem.set_checked(zitem.id().as_ref() == "zoom_100");
                        }
                        if let Some(wv) = handle.get_webview_window("main") {
                            let _ = wv.set_zoom(1.0);
                        }

                        // Disable autostart.
                        {
                            use tauri_plugin_autostart::ManagerExt;
                            let _ = handle.autolaunch().disable();
                        }
                        let _ = autostart_c.set_checked(false);

                        // Navigate to messenger.com to clear session/cookies.
                        if let Some(wv) = handle.get_webview_window("main") {
                            let _ = wv.eval(
                                "window.location.href = 'https://www.messenger.com';",
                            );
                            let _ = wv.show();
                            let _ = wv.set_focus();
                        }
                    }

                    // ---- Quit ----
                    "tray_quit" => {
                        handle.exit(0);
                    }

                    _ => {}
                }
            })
            .build(app)
        {
            Ok(_tray) => {}
            Err(e) => log::warn!("[MessengerX] Failed to create tray icon: {e}"),
        }
    }

    // ------------------------------------------------------------------
    // 6b. macOS application menu bar — full settings menu.
    //     All items that would be in the tray on Windows / Linux are
    //     placed here instead.  No tray icon is used on macOS.
    // ------------------------------------------------------------------
    #[cfg(target_os = "macos")]
    {
        use tauri::menu::CheckMenuItemBuilder;

        // Load current autostart state from the OS (may differ from saved).
        let autostart_checked = {
            use tauri_plugin_autostart::ManagerExt;
            app.autolaunch()
                .is_enabled()
                .unwrap_or(settings.autostart)
        };

        // --- Build all menu items ---

        let show_item =
            MenuItemBuilder::with_id("tray_show", &tr.tray_show).build(app)?;

        let stay_logged_in_item = CheckMenuItemBuilder::with_id(
            "stay_logged_in",
            &tr.settings_stay_logged_in,
        )
        .checked(settings.stay_logged_in)
        .build(app)?;

        let notifications_item = CheckMenuItemBuilder::with_id(
            "notifications_enabled",
            &tr.settings_notifications_enabled,
        )
        .checked(settings.notifications_enabled)
        .build(app)?;

        let notification_sound_item = CheckMenuItemBuilder::with_id(
            "notification_sound",
            &tr.settings_notification_sound,
        )
        .checked(settings.notification_sound)
        .build(app)?;

        // Zoom submenu with radio-like CheckMenuItems (60%–120%)
        let zoom_levels: &[(u32, &str)] = &[
            (60, "60%"),
            (70, "70%"),
            (80, "80%"),
            (90, "90%"),
            (100, "100%"),
            (110, "110%"),
            (120, "120%"),
        ];
        let current_zoom_pct = (settings.zoom_level * 100.0).round() as u32;
        let mut zoom_checks: Vec<tauri::menu::CheckMenuItem<tauri::Wry>> = Vec::new();
        for &(pct, label) in zoom_levels {
            let item = CheckMenuItemBuilder::with_id(format!("zoom_{pct}"), label)
                .checked(pct == current_zoom_pct)
                .build(app)?;
            zoom_checks.push(item);
        }

        let mut zoom_builder = SubmenuBuilder::new(app, &tr.settings_zoom_level);
        for item in &zoom_checks {
            zoom_builder = zoom_builder.item(item);
        }
        let zoom_submenu = zoom_builder.build()?;

        let autostart_item = CheckMenuItemBuilder::with_id(
            "autostart",
            &tr.settings_autostart,
        )
        .checked(autostart_checked)
        .build(app)?;

        let start_minimized_item = CheckMenuItemBuilder::with_id(
            "start_minimized",
            &tr.settings_start_minimized,
        )
        .checked(settings.start_minimized)
        .build(app)?;

        let auto_update_item = CheckMenuItemBuilder::with_id(
            "auto_update",
            &tr.settings_auto_update,
        )
        .checked(settings.auto_update)
        .build(app)?;

        let check_update_item =
            MenuItemBuilder::with_id("check_update", &tr.settings_check_update).build(app)?;

        let view_logs_item =
            MenuItemBuilder::with_id("view_logs", &tr.settings_view_logs).build(app)?;

        let clear_logs_item =
            MenuItemBuilder::with_id("clear_logs", &tr.settings_clear_logs).build(app)?;

        let logout_item =
            MenuItemBuilder::with_id("logout", &tr.settings_logout).build(app)?;

        // Non-interactive version label shown at the top of the app submenu.
        let version_str = format!("v{}", app.package_info().version);
        let version_item = MenuItemBuilder::with_id("app_version_macos", &version_str)
            .enabled(false)
            .build(app)?;

        // --- Separators ---
        let sep1 = PredefinedMenuItem::separator(app)?;
        let sep2 = PredefinedMenuItem::separator(app)?;
        let sep3 = PredefinedMenuItem::separator(app)?;
        let sep4 = PredefinedMenuItem::separator(app)?;
        let sep5 = PredefinedMenuItem::separator(app)?;
        let sep6 = PredefinedMenuItem::separator(app)?;

        // --- Assemble app submenu ---
        let app_submenu = SubmenuBuilder::new(app, "Messenger X")
            .item(&version_item)
            .item(&sep1)
            .item(&show_item)
            .item(&sep2)
            .item(&stay_logged_in_item)
            .item(&notifications_item)
            .item(&notification_sound_item)
            .item(&sep3)
            .item(&zoom_submenu)
            .item(&sep4)
            .item(&autostart_item)
            .item(&start_minimized_item)
            .item(&auto_update_item)
            .item(&sep5)
            .item(&check_update_item)
            .item(&view_logs_item)
            .item(&clear_logs_item)
            .item(&sep6)
            .item(&logout_item)
            .separator()
            .quit()
            .build()?;

        let edit_submenu = SubmenuBuilder::new(app, "Edit")
            .undo()
            .redo()
            .separator()
            .cut()
            .copy()
            .paste()
            .select_all()
            .build()?;

        let app_menu = MenuBuilder::new(app)
            .items(&[&app_submenu, &edit_submenu])
            .build()?;

        app.set_menu(app_menu)?;

        // --- Clone items for the menu-event closure ---
        let h = app.handle().clone();
        let stay_logged_in_c = stay_logged_in_item.clone();
        let notifications_c = notifications_item.clone();
        let notification_sound_c = notification_sound_item.clone();
        let autostart_c = autostart_item.clone();
        let start_minimized_c = start_minimized_item.clone();
        let auto_update_c = auto_update_item.clone();
        let zoom_checks_c = zoom_checks.clone();

        // Translated strings for update-check notifications and dialog.
        let tr_no_update = tr.settings_no_update.clone();
        let tr_update_error = tr.settings_update_error.clone();
        // Translated strings for the update confirmation dialog.
        let tr_update_dialog_title = tr.settings_update_dialog_title.clone();
        let tr_update_dialog_body = tr.settings_update_dialog_body.clone();
        let tr_update_install_btn = tr.settings_update_install_btn.clone();
        let tr_update_later_btn = tr.settings_update_later_btn.clone();

        app.on_menu_event(move |_app, event| {
            let id = event.id().as_ref();

            match id {
                // ---- Show window ----
                "tray_show" => {
                    if let Some(window) = h.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                }

                // ---- Toggle: stay logged in ----
                "stay_logged_in" => {
                    if let Ok(checked) = stay_logged_in_c.is_checked() {
                        let mut s = services::auth::load_settings(&h).unwrap_or_default();
                        s.stay_logged_in = checked;
                        let _ = services::auth::save_settings(&h, &s);
                    }
                }

                // ---- Toggle: notifications enabled ----
                "notifications_enabled" => {
                    if let Ok(checked) = notifications_c.is_checked() {
                        let mut s = services::auth::load_settings(&h).unwrap_or_default();
                        s.notifications_enabled = checked;
                        let _ = services::auth::save_settings(&h, &s);
                    }
                }

                // ---- Toggle: notification sound ----
                "notification_sound" => {
                    if let Ok(checked) = notification_sound_c.is_checked() {
                        let mut s = services::auth::load_settings(&h).unwrap_or_default();
                        s.notification_sound = checked;
                        let _ = services::auth::save_settings(&h, &s);
                    }
                }

                // ---- Toggle: autostart ----
                "autostart" => {
                    if let Ok(checked) = autostart_c.is_checked() {
                        use tauri_plugin_autostart::ManagerExt;
                        let autolaunch = h.autolaunch();
                        if checked {
                            let _ = autolaunch.enable();
                        } else {
                            let _ = autolaunch.disable();
                        }
                        let mut s = services::auth::load_settings(&h).unwrap_or_default();
                        s.autostart = checked;
                        let _ = services::auth::save_settings(&h, &s);
                    }
                }

                // ---- Toggle: start minimized ----
                "start_minimized" => {
                    if let Ok(checked) = start_minimized_c.is_checked() {
                        let mut s = services::auth::load_settings(&h).unwrap_or_default();
                        s.start_minimized = checked;
                        let _ = services::auth::save_settings(&h, &s);
                    }
                }

                // ---- Toggle: auto-update ----
                "auto_update" => {
                    if let Ok(checked) = auto_update_c.is_checked() {
                        let mut s = services::auth::load_settings(&h).unwrap_or_default();
                        s.auto_update = checked;
                        let _ = services::auth::save_settings(&h, &s);
                    }
                }

                // ---- Zoom (radio-like behaviour) ----
                _ if id.starts_with("zoom_") => {
                    if let Ok(pct) = id[5..].parse::<u32>() {
                        let level = pct as f64 / 100.0;
                        // Enforce radio: uncheck all, check selected.
                        for zitem in &zoom_checks_c {
                            let _ = zitem.set_checked(zitem.id().as_ref() == id);
                        }
                        // Apply native zoom.
                        if let Some(wv) = h.get_webview_window("main") {
                            let _ = wv.set_zoom(level);
                        }
                        // Persist.
                        let mut s = services::auth::load_settings(&h).unwrap_or_default();
                        s.zoom_level = level;
                        let _ = services::auth::save_settings(&h, &s);
                    }
                }

                // ---- Check for updates ----
                "check_update" => {
                    let h2 = h.clone();
                    let tr_none = tr_no_update.clone();
                    let tr_err = tr_update_error.clone();
                    let tr_dlg_title = tr_update_dialog_title.clone();
                    let tr_dlg_body = tr_update_dialog_body.clone();
                    let tr_install = tr_update_install_btn.clone();
                    let tr_later = tr_update_later_btn.clone();
                    tauri::async_runtime::spawn(async move {
                        use tauri_plugin_updater::UpdaterExt;
                        match h2.updater() {
                            Ok(updater) => match updater.check().await {
                                Ok(Some(update)) => {
                                    let ver = update.version.clone();
                                    let body = tr_dlg_body.replace("{}", &ver);
                                    save_check_timestamp(&h2);
                                    // Show confirmation dialog.
                                    // On macOS, auto-install is blocked by Gatekeeper until
                                    // notarization is implemented (FEAT-003); the dialog
                                    // informs the user and the install branch is a no-op.
                                    use tauri_plugin_dialog::{
                                        DialogExt, MessageDialogButtons,
                                    };
                                    h2.dialog()
                                        .message(&body)
                                        .title(&tr_dlg_title)
                                        .buttons(MessageDialogButtons::OkCancelCustom(
                                            tr_install,
                                            tr_later,
                                        ))
                                        .show(move |confirmed| {
                                            if confirmed {
                                                // Gatekeeper blocks install without notarization.
                                                log::info!(
                                                    "[MessengerX] Update v{ver} \
                                                     acknowledged (macOS — install \
                                                     requires notarization, FEAT-003)"
                                                );
                                            } else {
                                                log::info!(
                                                    "[MessengerX] Update v{ver} deferred by user"
                                                );
                                            }
                                        });
                                }
                                Ok(None) => {
                                    save_check_timestamp(&h2);
                                    let _ = services::notification::show_notification(
                                        &h2, "Messenger X", &tr_none, "update", false,
                                    );
                                }
                                Err(e) => {
                                    log::warn!("[MessengerX] Update check failed: {e}");
                                    let _ = services::notification::show_notification(
                                        &h2, "Messenger X", &tr_err, "update", false,
                                    );
                                }
                            },
                            Err(e) => {
                                log::warn!("[MessengerX] Updater init failed: {e}");
                                let _ = services::notification::show_notification(
                                    &h2, "Messenger X", &tr_err, "update", false,
                                );
                            }
                        }
                    });
                }

                // ---- View Logs ----
                "view_logs" => {
                    use tauri_plugin_opener::OpenerExt;
                    if let Ok(log_dir) = h.path().app_log_dir() {
                        let log_file = log_dir.join("messengerx.log");
                        if log_file.exists() {
                            let _ = h.opener().open_path(
                                log_file.to_string_lossy().into_owned(),
                                None::<&str>,
                            );
                        } else {
                            let _ = std::fs::create_dir_all(&log_dir);
                            let _ = h.opener().open_path(
                                log_dir.to_string_lossy().into_owned(),
                                None::<&str>,
                            );
                        }
                    }
                }

                // ---- Clear Logs ----
                "clear_logs" => {
                    if let Ok(log_dir) = h.path().app_log_dir() {
                        for name in &["messengerx.log", "messengerx.log.old"] {
                            let path = log_dir.join(name);
                            if path.exists() {
                                if let Err(e) = std::fs::remove_file(&path) {
                                    log::warn!("[MessengerX] Failed to delete {}: {e}", path.display());
                                } else {
                                    log::info!("[MessengerX] Deleted log file: {}", path.display());
                                }
                            }
                        }
                    }
                }

                // ---- Log out & clear data ----
                "logout" => {
                    let _ = services::cache::clear_snapshots(&h);
                    let defaults = commands::AppSettings::default();
                    let _ = services::auth::save_settings(&h, &defaults);

                    // Reset all checkbox states to defaults.
                    let _ = stay_logged_in_c.set_checked(defaults.stay_logged_in);
                    let _ = notifications_c.set_checked(defaults.notifications_enabled);
                    let _ = notification_sound_c.set_checked(defaults.notification_sound);
                    let _ = start_minimized_c.set_checked(defaults.start_minimized);
                    let _ = auto_update_c.set_checked(defaults.auto_update);

                    // Reset zoom to 100 %.
                    for zitem in &zoom_checks_c {
                        let _ = zitem.set_checked(zitem.id().as_ref() == "zoom_100");
                    }
                    if let Some(wv) = h.get_webview_window("main") {
                        let _ = wv.set_zoom(1.0);
                    }

                    // Disable autostart.
                    {
                        use tauri_plugin_autostart::ManagerExt;
                        let _ = h.autolaunch().disable();
                    }
                    let _ = autostart_c.set_checked(false);

                    // Navigate to messenger.com to clear session/cookies.
                    if let Some(wv) = h.get_webview_window("main") {
                        let _ = wv.eval(
                            "window.location.href = 'https://www.messenger.com';",
                        );
                        let _ = wv.show();
                        let _ = wv.set_focus();
                    }
                }

                _ => {}
            }
        });
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

    // ------------------------------------------------------------------
    // 8. Startup auto-update check (once every ≥ 30 days).
    //    Only runs if `auto_update` is enabled in user settings.
    //    A 5-second startup delay ensures the main window is fully
    //    initialised before the dialog may appear.
    // ------------------------------------------------------------------
    if settings.auto_update {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let should_check = match settings.last_update_check_secs {
            None => true,
            Some(last) => now_secs.saturating_sub(last) >= 30 * 24 * 3600,
        };
        if should_check {
            let startup_handle = app.handle().clone();
            let tr_dlg_title_su = tr.settings_update_dialog_title.clone();
            let tr_dlg_body_su = tr.settings_update_dialog_body.clone();
            let tr_install_su = tr.settings_update_install_btn.clone();
            let tr_later_su = tr.settings_update_later_btn.clone();
            // Install-path strings — only needed on non-macOS where
            // `download_and_install` is actually called.
            #[cfg(not(target_os = "macos"))]
            let tr_ready_su = tr.settings_update_ready.clone();
            #[cfg(not(target_os = "macos"))]
            let tr_err_su = tr.settings_update_error.clone();
            std::thread::spawn(move || {
                // Give the app time to finish initialising.
                std::thread::sleep(std::time::Duration::from_secs(5));
                tauri::async_runtime::spawn(async move {
                    use tauri_plugin_updater::UpdaterExt;
                    match startup_handle.updater() {
                        Ok(updater) => match updater.check().await {
                            Ok(Some(update)) => {
                                let ver = update.version.clone();
                                let body = tr_dlg_body_su.replace("{}", &ver);
                                save_check_timestamp(&startup_handle);
                                use tauri_plugin_dialog::{
                                    DialogExt, MessageDialogButtons,
                                };
                                // Windows / Linux: show dialog and install if confirmed.
                                #[cfg(not(target_os = "macos"))]
                                {
                                    let h_install = startup_handle.clone();
                                    let tr_ready_c = tr_ready_su;
                                    let tr_err_c = tr_err_su;
                                    startup_handle
                                        .dialog()
                                        .message(&body)
                                        .title(&tr_dlg_title_su)
                                        .buttons(MessageDialogButtons::OkCancelCustom(
                                            tr_install_su,
                                            tr_later_su,
                                        ))
                                        .show(move |confirmed| {
                                            if confirmed {
                                                tauri::async_runtime::spawn(async move {
                                                    match update
                                                        .download_and_install(|_, _| {}, || {})
                                                        .await
                                                    {
                                                        Ok(()) => {
                                                            let _ = services::notification::show_notification(
                                                                &h_install,
                                                                "Messenger X",
                                                                &tr_ready_c,
                                                                "update",
                                                                false,
                                                            );
                                                            h_install.restart();
                                                        }
                                                        Err(e) => {
                                                            log::warn!(
                                                                "[MessengerX] Startup update install failed: {e}"
                                                            );
                                                            let _ = services::notification::show_notification(
                                                                &h_install,
                                                                "Messenger X",
                                                                &tr_err_c,
                                                                "update",
                                                                false,
                                                            );
                                                        }
                                                    }
                                                });
                                            } else {
                                                log::info!(
                                                    "[MessengerX] Startup update v{ver} \
                                                     deferred by user"
                                                );
                                            }
                                        });
                                }
                                // macOS: show informational dialog only — auto-install
                                // is blocked by Gatekeeper until notarization (FEAT-003).
                                #[cfg(target_os = "macos")]
                                startup_handle
                                    .dialog()
                                    .message(&body)
                                    .title(&tr_dlg_title_su)
                                    .buttons(MessageDialogButtons::OkCancelCustom(
                                        tr_install_su,
                                        tr_later_su,
                                    ))
                                    .show(move |confirmed| {
                                        if confirmed {
                                            log::info!(
                                                "[MessengerX] Startup update v{ver} \
                                                 acknowledged (macOS — install requires \
                                                 notarization, FEAT-003)"
                                            );
                                        } else {
                                            log::info!(
                                                "[MessengerX] Startup update v{ver} \
                                                 deferred by user"
                                            );
                                        }
                                    });
                            }
                            Ok(None) => {
                                save_check_timestamp(&startup_handle);
                                log::debug!("[MessengerX] Startup check: no update available");
                            }
                            Err(e) => {
                                log::warn!("[MessengerX] Startup update check failed: {e}");
                            }
                        },
                        Err(e) => {
                            log::warn!(
                                "[MessengerX] Startup updater init failed: {e}"
                            );
                        }
                    }
                });
            });
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // The visibility-override script is Linux-only; gate the tests accordingly.
    #[cfg(target_os = "linux")]
    mod visibility_script {
        use super::super::VISIBILITY_OVERRIDE_SCRIPT;

        /// The script must default `_hidden` to `false` so that a normal
        /// foreground load has the correct state immediately, without waiting
        /// for the async IPC round-trip to complete.
        #[test]
        fn default_state_is_visible() {
            assert!(
                VISIBILITY_OVERRIDE_SCRIPT.contains("var _hidden = false"),
                "script must default _hidden to false (safe for foreground startup)"
            );
        }

        /// The script must expose the `__messengerx_set_visible` setter so that
        /// the Rust `WindowEvent::Focused` handler can update the flag at runtime.
        #[test]
        fn exposes_set_visible_function() {
            assert!(
                VISIBILITY_OVERRIDE_SCRIPT.contains("__messengerx_set_visible"),
                "script must define __messengerx_set_visible"
            );
        }

        /// The `visibilityState` property getter and the `visibilitychange` event
        /// dispatch must be present in the script.
        #[test]
        fn overrides_visibility_api_properties() {
            assert!(
                VISIBILITY_OVERRIDE_SCRIPT.contains("visibilityState"),
                "script must override visibilityState"
            );
            assert!(
                VISIBILITY_OVERRIDE_SCRIPT.contains("visibilitychange"),
                "script must dispatch visibilitychange event"
            );
        }

        /// Messenger may consult focus signals in addition to visibility,
        /// so the script must override `document.hasFocus()` and dispatch
        /// synthetic `focus` / `blur` events when the state changes.
        #[test]
        fn overrides_focus_api_properties() {
            assert!(
                VISIBILITY_OVERRIDE_SCRIPT.contains("hasFocus"),
                "script must override document.hasFocus"
            );
            assert!(
                VISIBILITY_OVERRIDE_SCRIPT.contains("'focus'"),
                "script must dispatch a synthetic focus event"
            );
            assert!(
                VISIBILITY_OVERRIDE_SCRIPT.contains("'blur'"),
                "script must dispatch a synthetic blur event"
            );
        }

        /// On every page load the script must invoke `get_window_focused` via
        /// the Tauri IPC bridge to resync `_hidden` from the real OS window
        /// state.  This prevents re-navigations (e.g. logout → login page)
        /// from inheriting a stale start-up preference.
        #[test]
        fn resyncs_from_ipc_on_page_load() {
            assert!(
                VISIBILITY_OVERRIDE_SCRIPT.contains("get_window_focused"),
                "script must invoke get_window_focused to resync state on each navigation"
            );
        }
    }
}
