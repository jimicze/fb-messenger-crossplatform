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
    let webview = WebviewWindowBuilder::new(app, "main", webview_url)
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
    // 6. System tray icon with context menu.
    //    All settings are now inline CheckMenuItems instead of a separate
    //    settings window.
    // ------------------------------------------------------------------
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

        // Action items
        let check_update_item = MenuItemBuilder::with_id(
            "check_update",
            &tr.settings_check_update,
        )
        .build(app)?;

        let logout_item =
            MenuItemBuilder::with_id("logout", &tr.settings_logout).build(app)?;

        let quit_item =
            MenuItemBuilder::with_id("tray_quit", &tr.tray_quit).build(app)?;

        // --- Separators ---
        let sep1 = PredefinedMenuItem::separator(app)?;
        let sep2 = PredefinedMenuItem::separator(app)?;
        let sep3 = PredefinedMenuItem::separator(app)?;
        let sep4 = PredefinedMenuItem::separator(app)?;

        // --- Assemble tray menu ---
        let tray_menu = MenuBuilder::new(app)
            .item(&show_item)
            .item(&sep1)
            .item(&stay_logged_in_item)
            .item(&notifications_item)
            .item(&notification_sound_item)
            .item(&sep2)
            .item(&zoom_submenu)
            .item(&sep3)
            .item(&autostart_item)
            .item(&start_minimized_item)
            .item(&sep4)
            .item(&check_update_item)
            .item(&logout_item)
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
        let zoom_checks_c = zoom_checks.clone();

        // Translated strings for update-check notifications.
        let tr_update_available = tr.settings_update_available.clone();
        let tr_no_update = tr.settings_no_update.clone();
        let tr_update_error = tr.settings_update_error.clone();
        let tr_update_ready = tr.settings_update_ready.clone();

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
                        tauri::async_runtime::spawn(async move {
                            use tauri_plugin_updater::UpdaterExt;
                            match h.updater() {
                                Ok(updater) => match updater.check().await {
                                    Ok(Some(update)) => {
                                        let ver = update.version.clone();
                                        let msg = tr_avail.replace("{}", &ver);
                                        let _ = services::notification::show_notification(
                                            &h, "Messenger X", &msg, "update", false,
                                        );
                                        // macOS: auto-install blocked by Gatekeeper until
                                        // the app is notarized (FEAT-003). Skip install
                                        // to avoid a misleading "Update check failed" error.
                                        #[cfg(not(target_os = "macos"))]
                                        match update
                                            .download_and_install(|_, _| {}, || {})
                                            .await
                                        {
                                            Ok(()) => {
                                                let _ =
                                                    services::notification::show_notification(
                                                        &h,
                                                        "Messenger X",
                                                        &tr_ready,
                                                        "update",
                                                        false,
                                                    );
                                                h.restart();
                                            }
                                            Err(e) => {
                                                log::warn!(
                                                    "[MessengerX] Update install failed: {e}"
                                                );
                                                let _ =
                                                    services::notification::show_notification(
                                                        &h,
                                                        "Messenger X",
                                                        &tr_err,
                                                        "update",
                                                        false,
                                                    );
                                            }
                                        }
                                        // Suppress unused-variable warning on macOS.
                                        #[cfg(target_os = "macos")]
                                        let _ = (tr_ready, tr_err);
                                    }
                                    Ok(None) => {
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

                    // ---- Log out & clear data ----
                    "logout" => {
                        let _ = services::cache::clear_snapshots(handle);
                        let defaults = commands::AppSettings::default();
                        let _ = services::auth::save_settings(handle, &defaults);

                        // Reset all checkbox states to defaults.
                        let _ = stay_logged_in_c.set_checked(defaults.stay_logged_in);
                        let _ = notifications_c.set_checked(defaults.notifications_enabled);
                        let _ = notification_sound_c.set_checked(defaults.notification_sound);
                        let _ = start_minimized_c.set_checked(defaults.start_minimized);

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
    // 6b. macOS application menu bar (Edit menu for clipboard shortcuts,
    //     Quit via Cmd+Q).  Settings are now in the tray context menu.
    // ------------------------------------------------------------------
    #[cfg(target_os = "macos")]
    {
        let app_submenu = SubmenuBuilder::new(app, "Messenger X")
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
