//! Library root for the Messenger X cross-platform desktop client.
//!
//! Registers Tauri plugins, builds the main window programmatically (so that
//! custom `initialization_script`s and `on_navigation` hooks can be attached),
//! and wires up the IPC invoke-handler.

mod commands;
mod services;

use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// Monotonically increasing counter used to generate unique window labels for
/// popup windows spawned via `window.open()` (e.g. Messenger video/audio call
/// UI).  Wraps at `u32::MAX` but that is effectively unreachable in practice.
static POPUP_WINDOW_COUNTER: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

// ---------------------------------------------------------------------------
// JavaScript injection scripts
// ---------------------------------------------------------------------------

/// Overrides `window.Notification` and `ServiceWorkerRegistration.showNotification`
/// so that ALL browser notification calls (main-thread and SW-registration-based)
/// are forwarded to Rust via `invoke('send_notification', …)`.
///
/// **Cesta A+B+C (v1.3.13–14):**
/// - Cesta A (diagnostika): periodic 30s health-check, SW-register hook, controllerchange
///   listener, prototype-level override logging — generates detailed diagnostics in
///   `messengerx.log` so we can pinpoint whether Messenger uses `new Notification()`,
///   `registration.showNotification()`, or a pure SW push that bypasses both.
/// - Cesta B (spekulativní fix): intercepts `ServiceWorkerRegistration.prototype.showNotification`
///   at document-start (before any Messenger JS executes) so calls from the main thread
///   via `navigator.serviceWorker.ready.then(reg => reg.showNotification(…))` are
///   forwarded to Rust instead of the browser's notification stack (which is unsupported
///   in Tauri WebViews).
/// - Cesta C: hooks `navigator.permissions.query` to always return `'granted'` for
///   the `notifications` descriptor so Messenger's permission gate doesn't block calls.
///
/// **Cesta D — postMessage bridge (v1.3.15):**
/// Tauri IPC (`window.__TAURI__.core.invoke`) only works from the main frame
/// (messenger.com origin).  Cross-origin iframes (e.g. `www.fbsbx.com` which
/// hosts Messenger's App Worker proxy page) cannot call IPC directly — the
/// capability system restricts IPC to the `main` window origin.
///
/// The bridge works in two halves:
/// 1. **In any iframe** (`window !== window.top`): `forwardToRust()` sends a
///    `postMessage` with `type: '__mx_notif__'` to `window.top` instead of
///    calling IPC directly.  On init, a `__mx_frame_init__` probe is sent so
///    the log shows which cross-origin frames received the injection.
/// 2. **In the main frame** (`window === window.top`): a `'message'` listener
///    receives those relay messages and calls `forwardToRust()` — which this
///    time succeeds via IPC because we are now in the allowed origin context.
///
/// Injected **at document-start** (before the page HTML is parsed).
const NOTIFICATION_OVERRIDE_SCRIPT: &str = concat!(
    r#"
(function() {

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

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
        } catch(_) { return 'error'; }
    }

    // Core: forward any notification to Rust over IPC.
    // `source` is a short string that identifies which code path fired
    // (e.g. '[window.Notification]' or '[SW.proto]') so logs are easy to grep.
    //
    // **Cross-origin iframe path (Cesta D):**
    // When called from a cross-origin iframe (window !== window.top), Tauri IPC
    // is unavailable (capability restrictions).  Instead we relay the notification
    // to the main frame via postMessage; the main-frame listener (section 6) picks
    // it up and calls forwardToRust again — this time successfully via IPC.
    function forwardToRust(title, options, source) {
        var body   = (options && options.body)   ? String(options.body)   : '';
        var tag    = (options && options.tag)    ? String(options.tag)    : '';
        var silent = (options && options.silent) ? Boolean(options.silent) : false;

        if (window !== window.top) {
            // Cross-origin iframe: relay to main frame via postMessage.
            try {
                window.top.postMessage({
                    type: '__mx_notif__',
                    title: title, body: body, tag: tag, silent: silent, source: source
                }, '*');
            } catch(_) {}
            return;
        }

        // Main frame: forward directly over IPC.
        try {
            jlog(
                source + ' title='      + JSON.stringify(preview(title)) +
                ' body='       + JSON.stringify(preview(body)) +
                ' tag='        + JSON.stringify(preview(tag)) +
                ' silent='     + String(silent) +
                ' visibility=' + String(document.visibilityState) +
                ' hidden='     + String(document.hidden) +
                ' hasFocus='   + String(safeHasFocus())
            );
            window.__TAURI__.core.invoke('send_notification', {
                title: title, body: body, tag: tag, silent: silent
            }).then(function()  { jlog(source + ' IPC resolved'); })
              .catch(function(e){ jlog(source + ' IPC rejected: ' + String(e)); });
        } catch(e) {
            jlog(source + ' forwardToRust error: ' + String(e));
        }
    }

    // -----------------------------------------------------------------------
    // 0. Frame detection probe  [Cesta D — postMessage bridge]
    //    If the script is running inside a cross-origin iframe, announce the
    //    frame's hostname to the main frame so the log shows which iframes
    //    received the injection.  This is the first diagnostic for Cesta D.
    // -----------------------------------------------------------------------
    if (window !== window.top) {
        try {
            window.top.postMessage({
                type: '__mx_frame_init__',
                hostname: window.location.hostname
            }, '*');
        } catch(_) {}
    }

    // -----------------------------------------------------------------------
    // 1. window.Notification override
    //    Catches `new Notification(title, options)` calls from main-thread JS.
    // -----------------------------------------------------------------------
    window.Notification = function(title, options) {
        forwardToRust(title, options || {}, '[window.Notification]');
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
    if (window === window.top) { jlog('window.Notification override installed'); }

    // -----------------------------------------------------------------------
    // 2. ServiceWorkerRegistration.prototype.showNotification hook  [Cesta B]
    //    Messenger may call `registration.showNotification()` from the main
    //    thread (e.g. via `navigator.serviceWorker.ready.then(reg => …)`)
    //    instead of `new Notification()`.  Hooking the prototype at
    //    document-start ensures all current and future registration instances
    //    are intercepted before any Messenger JS executes.
    // -----------------------------------------------------------------------
    try {
        if (typeof ServiceWorkerRegistration !== 'undefined') {
            var _origProtoShow = ServiceWorkerRegistration.prototype.showNotification;
            ServiceWorkerRegistration.prototype.showNotification = function(title, options) {
                if (window === window.top) {
                    // Capture a 2-frame stack preview to identify which Messenger
                    // module triggered the call.  Phase M diagnostic: helps decide
                    // whether the Linux 3rd-toast is a duplicate main-thread
                    // SW.proto path or a separate SW push event.
                    var stackPreview = '';
                    try {
                        var s = (new Error('stack')).stack || '';
                        stackPreview = s.split('\n').slice(1, 4).join(' || ');
                        if (stackPreview.length > 360) stackPreview = stackPreview.slice(0, 357) + '...';
                    } catch(_) {}
                    jlog('[SW.proto] showNotification intercepted: title=' + JSON.stringify(preview(title)) +
                         ' stack=' + JSON.stringify(stackPreview));
                }
                forwardToRust(title, options || {}, '[SW.proto]');
                // Return a resolved Promise (matches the original API contract).
                // We do NOT call _origProtoShow because Tauri WebViews lack a
                // push-subscription endpoint, so the native call would silently
                // fail — and calling it would attempt a double-notification on
                // platforms where push notifications are partially supported.
                return Promise.resolve();
            };
            if (window === window.top) { jlog('ServiceWorkerRegistration.prototype.showNotification hooked'); }
        } else {
            if (window === window.top) { jlog('ServiceWorkerRegistration not available (no SW support in this WebView)'); }
        }
    } catch(e) {
        if (window === window.top) { jlog('SW proto hook failed: ' + String(e)); }
    }

    // -----------------------------------------------------------------------
    // 3. navigator.serviceWorker.register hook  [Cesta A — diagnostika]
    //    Logs every SW registration attempt (URL, scope) and fires when the
    //    active SW changes controller so we can see which SW script Messenger
    //    loads and whether it installs itself as the page controller.
    // -----------------------------------------------------------------------
    if (window === window.top) {
        try {
            if (navigator.serviceWorker) {
                // Log already-registered SWs from previous page load (persisted).
                navigator.serviceWorker.getRegistrations().then(function(regs) {
                    jlog(
                        '[SW.existing] count=' + regs.length +
                        (regs.length ? ' scope[0]=' + regs[0].scope : '')
                    );
                }).catch(function(e) { jlog('[SW.existing] getRegistrations error: ' + String(e)); });

                // Wrap register() to log every new SW registration.
                var _origRegister = navigator.serviceWorker.register.bind(navigator.serviceWorker);
                navigator.serviceWorker.register = function(scriptURL, options) {
                    jlog('[SW.register] called: scriptURL=' + String(scriptURL));
                    var p = _origRegister(scriptURL, options);
                    p.then(function(reg) {
                        jlog('[SW.register] resolved: scope=' + reg.scope + ' state=' + reg.active ? reg.active.state : 'no-active');
                    }).catch(function(e) { jlog('[SW.register] rejected: ' + String(e)); });
                    return p;
                };
                jlog('navigator.serviceWorker.register hooked');

                // Log whenever a new SW takes control of this page.
                navigator.serviceWorker.addEventListener('controllerchange', function() {
                    var ctrl = navigator.serviceWorker.controller;
                    jlog('[SW.controllerchange] new controller: ' + (ctrl ? ctrl.scriptURL : 'null') +
                         ' state=' + (ctrl ? ctrl.state : 'n/a'));
                });

                // Log current controller at startup.
                var ctrl0 = navigator.serviceWorker.controller;
                jlog('[SW.controller@init] ' + (ctrl0 ? ctrl0.scriptURL + ' state=' + ctrl0.state : 'none'));

            } else {
                jlog('navigator.serviceWorker not available');
            }
        } catch(e) {
            jlog('SW register hook failed: ' + String(e));
        }
    }

    // -----------------------------------------------------------------------
    // 4. navigator.permissions.query hook  [Cesta C — root-cause fix]
    //    WKWebView (macOS) and WebKitGTK (Linux) return state='denied' or
    //    state='prompt' for navigator.permissions.query({name:'notifications'}).
    //    Messenger consults this API before ever calling new Notification() —
    //    if the result is not 'granted' it skips the entire notification path.
    //    We intercept the call and return a fake PermissionStatus that always
    //    reports 'granted', so Messenger proceeds to fire new Notification().
    //
    //    Also intercepts 'microphone' and 'camera' — Messenger checks these
    //    before enabling 1:1 voice/video call UI.  On WKWebView / WebKitGTK
    //    without a prior user grant the Permissions API returns 'prompt' (or
    //    throws on older WebKit versions).  Messenger interprets anything other
    //    than 'granted' as "browser does not support calls" and shows the
    //    "Calls not supported on this browser" banner.
    //
    //    Returning 'granted' here does NOT bypass the OS permission dialog —
    //    getUserMedia() still triggers the system camera/mic prompt on first
    //    use.  We are only signalling to Messenger that the browser is capable
    //    so it enables the call UI rather than hiding it.
    // -----------------------------------------------------------------------
    try {
        if (navigator.permissions && typeof navigator.permissions.query === 'function') {
            var _origPermsQuery = navigator.permissions.query.bind(navigator.permissions);
            navigator.permissions.query = function(descriptor) {
                var name = descriptor && descriptor.name;
                if (name === 'notifications' || name === 'microphone' || name === 'camera') {
                    if (window === window.top) {
                        jlog('permissions.query({' + name + '}) intercepted -> returning granted');
                    }
                    return Promise.resolve({
                        state: 'granted',
                        status: 'granted',
                        onchange: null,
                        addEventListener: function() {},
                        removeEventListener: function() {},
                        dispatchEvent: function() { return false; }
                    });
                }
                return _origPermsQuery(descriptor);
            };
            if (window === window.top) { jlog('navigator.permissions.query hooked'); }
        } else {
            if (window === window.top) { jlog('navigator.permissions.query not available'); }
        }
    } catch(e) { if (window === window.top) { jlog('permissions.query hook failed: ' + String(e)); } }

    // -----------------------------------------------------------------------
    // 5. Periodic health-check every 30 s  [Cesta A — diagnostika]
    //    Emits a snapshot of all key notification signals to the Rust log so
    //    that even if no notification fires we can verify the override is still
    //    active and track SW state across the session.
    //    Only runs in the main frame (IPC not available from cross-origin iframes).
    // -----------------------------------------------------------------------
    if (window === window.top) {
        setInterval(function() {
            try {
                var notifPerm  = 'unknown';
                var nativeFlag = 'unknown';
                try {
                    notifPerm  = window.Notification ? window.Notification.permission : 'Notification-undef';
                    nativeFlag = window.Notification ? String(window.Notification.__native__) : 'Notification-undef';
                } catch(_) {}

                if (navigator.serviceWorker) {
                    navigator.serviceWorker.getRegistrations().then(function(regs) {
                        var ctrl = navigator.serviceWorker.controller;
                        jlog(
                            '[health] permission='  + notifPerm +
                            ' __native__='  + nativeFlag +
                            ' visibility=' + String(document.visibilityState) +
                            ' hasFocus='   + String(safeHasFocus()) +
                            ' sw_regs='    + regs.length +
                            ' sw_ctrl='    + (ctrl ? ctrl.scriptURL : 'none')
                        );
                    }).catch(function(e) { jlog('[health] getRegistrations error: ' + String(e)); });
                } else {
                    jlog(
                        '[health] permission='  + notifPerm +
                        ' __native__='  + nativeFlag +
                        ' visibility=' + String(document.visibilityState) +
                        ' hasFocus='   + String(safeHasFocus()) +
                        ' sw=unavailable'
                    );
                }
            } catch(e) { jlog('[health] error: ' + String(e)); }
        }, 30000);
    }

    // -----------------------------------------------------------------------
    // 6. postMessage listener — relay from cross-origin iframes  [Cesta D]
    //    Cross-origin iframes (e.g. fbsbx.com) cannot call Tauri IPC directly.
    //    forwardToRust() in those frames sends a postMessage to window.top
    //    (section 0 + modified forwardToRust above).  This listener, active
    //    only in the main frame, receives those relay messages and forwards
    //    them to Rust via IPC — which succeeds here because we are now in
    //    the messenger.com origin that is allowed by capabilities.
    // -----------------------------------------------------------------------
    if (window === window.top) {
        window.addEventListener('message', function(e) {
            if (!e.data || typeof e.data !== 'object') return;
            if (e.data.type === '__mx_notif__') {
                jlog('[postMessage] notification relay origin=' + String(e.origin) +
                     ' source=' + String(e.data.source) +
                     ' title=' + JSON.stringify(preview(String(e.data.title || ''))));
                forwardToRust(
                    e.data.title  || '',
                    { body: e.data.body || '', tag: e.data.tag || '', silent: !!e.data.silent },
                    '[postMessage<-' + String(e.data.source || '?') + ']'
                );
            }
            if (e.data.type === '__mx_frame_init__') {
                jlog('[postMessage] frame init from hostname=' + String(e.data.hostname));
            }
        });
        jlog('postMessage listener installed (main frame)');
    }

    if (window === window.top) { jlog('init complete (v"#,
    env!("CARGO_PKG_VERSION"),
    r#" A+B+C+D)'); }
})();
"#,
);

/// Watches the document `<title>` for an unread-count prefix like `(3)` and
/// forwards changes to Rust via `invoke('update_unread_count', …)`.
///
/// Also attempts to extract the first unread sender's name from the conversation
/// list so the native notification can show the sender instead of "New message".
///
/// For count > 0, the IPC send is deferred by ~500 ms so that React has time to
/// update the conversation DOM before `getFirstUnreadSenderName()` runs. Before
/// the deferred send fires, the title count is re-read; if it dropped to 0 the
/// send is skipped (stale), and if it changed the latest count is sent instead.
///
/// **Activity signature** (`activitySig`): when the unread count stays the same
/// (e.g. `(1) Messenger` → `(1) Messenger` after a second message in the same
/// conversation) the title observer alone cannot detect the new message.
///
/// `activitySeq` is bumped only when **all** of these hold:
/// - `count > 0`
/// - A DOM snapshot of likely unread conversation link candidates
///   (`getActivitySnapshot`) is non-empty and differs from `lastActivitySnapshot`
///
/// This avoids spamming the sequence counter on presence dots, typing indicators,
/// read receipts, and other high-frequency UI churn that produces characterData /
/// attribute mutations but does not represent a new message.
///
/// The MutationObserver is narrowed to `childList + subtree` only (no
/// `attributes` / `characterData`) and debounced ~400 ms.  It observes
/// the sidebar / conversation-list root — not the entire body.
///
/// **Unread candidate detection (composite heuristics):**
/// - `aria-label` contains "unread" (existing)
/// - numeric badge `<span>` inside container (existing)
/// - `font-weight >= 600` on a `span[dir="auto"]` inside the link (new)
/// - small colored dot element as an unread indicator (new)
///
/// **Signature format:** `"<count>:<seq>:<snapshot>"`
/// where `snapshot` is the first 40 chars of `lastActivitySnapshot`.
/// If `lastActivitySnapshot` is empty (no unread DOM candidates found yet),
/// `buildSig` returns `''` so Rust treats it the same as count=0 (no new
/// notification until a real snapshot is available).  This keeps the sig
/// stable across title oscillations (1→0→1) when the snapshot does not change,
/// preventing the `zero-bounce-sig-changed` Rust path from firing repeatedly.
///
/// **Zero-timer:** when count drops to 0, a ~7 s timer is started.  The
/// `lastActivitySnapshot` / `lastSentSig` baseline is preserved during that
/// window so transient title bounces don't erase it.  The state is only fully
/// reset when zero persists for >7 s **or** when the window is focused while
/// count=0 (user has seen/read the messages).  The IPC still sends
/// `activitySig=""` immediately when count=0 (empty snapshot also yields `""`).
///
/// Optimistic `lastSentSig` update: set immediately before `setTimeout` fires
/// to prevent duplicate scheduling for the same sig; cleared/reverted if the
/// delayed send is skipped because count dropped to 0.
///
/// Injected at document-start; waits for DOM via `DOMContentLoaded`.
const UNREAD_OBSERVER_SCRIPT: &str = r#"
(function() {
    var MX_SENDER_TITLE_PREFIX = '__MX_SENDER_V1__?';

    function isMxSenderBridgeTitle(title) {
        return typeof title === 'string' && title.indexOf(MX_SENDER_TITLE_PREFIX) === 0;
    }

    function emitSenderHintViaTitle(count, sender) {
        try {
            sender = String(sender || '').trim();
            if (count <= 0 || sender.length < 1 || sender.length > 120) return;

            // JS->Rust invoke is unavailable from remote messenger.com on some
            // desktop WebViews.  document.title still reaches Rust via Wry's
            // native on_document_title_changed hook, so we use a very short-lived
            // sentinel title as a sender-hint side channel.  The Rust title
            // handler recognizes this prefix, stores the hint, and returns early
            // without treating it as a Messenger title/count event.
            var previousTitle = document.title;
            var payload = 'count=' + encodeURIComponent(String(count))
                + '&sender=' + encodeURIComponent(sender);
            document.title = MX_SENDER_TITLE_PREFIX + payload;
            setTimeout(function() {
                try {
                    if (isMxSenderBridgeTitle(document.title)) {
                        document.title = previousTitle;
                    }
                } catch(_) {}
            }, 80);
        } catch(_) {}
    }

    function jlog(msg) {
        try {
            window.__TAURI__.core.invoke('js_log', { message: '[UnreadJS] ' + msg });
        } catch(_) {}
    }

    function getUnreadCountFromTitle() {
        var title = document.title;
        if (isMxSenderBridgeTitle(title)) {
            return (typeof prevTitleCount === 'number') ? prevTitleCount : 0;
        }
        var match = title.match(/^\((\d+)\)/);
        return match ? parseInt(match[1], 10) : 0;
    }

    // ---------------------------------------------------------------------------
    // Composite unread-signal detection for a single link element.
    // Returns an object: { isUnread: bool, hasBadge: bool, hasBold: bool, hasDot: bool }
    // ---------------------------------------------------------------------------
    function detectUnreadSignals(link) {
        var ariaLabel = (link.getAttribute('aria-label') || '').trim();
        var isUnreadViaAria = ariaLabel.toLowerCase().indexOf('unread') !== -1;

        // Signal 2: small numeric badge in parent container.
        var hasNumericBadge = false;
        var container = link.parentElement || link;
        var allSpans = container.querySelectorAll('span');
        for (var s = 0; s < allSpans.length; s++) {
            if (/^\d{1,3}$/.test(allSpans[s].textContent.trim())
                    && allSpans[s].children.length === 0) {
                hasNumericBadge = true;
                break;
            }
        }

        // Signal 3: font-weight >= 600 on a span[dir="auto"] inside the link.
        var hasBold = false;
        var autoSpans = link.querySelectorAll('span[dir="auto"]');
        for (var b = 0; b < autoSpans.length; b++) {
            var spanText = autoSpans[b].textContent.trim();
            if (spanText.length < 1 || /^\d+$/.test(spanText)) continue;
            try {
                var fw = parseInt(window.getComputedStyle(autoSpans[b]).fontWeight, 10);
                if (!isNaN(fw) && fw >= 600) { hasBold = true; break; }
            } catch(_) {}
        }

        // Signal 4: small colored dot — look for a sibling/descendant element
        // that is roughly square (≤16 px), has no text, and has a non-transparent
        // background color that is not white/black/grey (i.e. accent color dot).
        var hasDot = false;
        try {
            var dotCandidates = container.querySelectorAll('*');
            for (var d = 0; d < Math.min(dotCandidates.length, 60); d++) {
                var el = dotCandidates[d];
                if (el.textContent.trim().length > 0) continue;
                var rect = el.getBoundingClientRect();
                if (rect.width < 4 || rect.width > 18 || rect.height < 4 || rect.height > 18) continue;
                if (Math.abs(rect.width - rect.height) > 6) continue;
                try {
                    var bg = window.getComputedStyle(el).backgroundColor;
                    // Match rgb(r,g,b) or rgba(r,g,b,a) with non-trivial saturation.
                    var m = bg.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/);
                    if (m) {
                        var r = parseInt(m[1],10), g = parseInt(m[2],10), bv = parseInt(m[3],10);
                        var mn = Math.min(r,g,bv), mx = Math.max(r,g,bv);
                        // Saturation proxy: max-min > 40 and not near-white (mx>50).
                        if (mx > 50 && (mx - mn) > 40) { hasDot = true; break; }
                    }
                } catch(_) {}
            }
        } catch(_) {}

        return {
            isUnread: isUnreadViaAria || hasNumericBadge || hasBold || hasDot,
            hasAria: isUnreadViaAria,
            hasBadge: hasNumericBadge,
            hasBold: hasBold,
            hasDot: hasDot,
            ariaLabel: ariaLabel
        };
    }

    // ---------------------------------------------------------------------------
    // Extract name from a link that has been deemed unread.
    // Priority: aria-label first segment, then first plausible span[dir="auto"] text.
    // ---------------------------------------------------------------------------
    function extractNameFromLink(link, signals) {
        // From aria-label first segment.
        if (signals.hasAria && signals.ariaLabel.length >= 1) {
            var namePart = signals.ariaLabel.split(',')[0].trim();
            if (namePart.length >= 1 && namePart.length <= 80
                    && namePart.toLowerCase().indexOf('messenger') === -1) {
                return namePart;
            }
        }
        // From first non-numeric span[dir="auto"].
        var autoSpans = link.querySelectorAll('span[dir="auto"]');
        for (var j = 0; j < autoSpans.length; j++) {
            var text = autoSpans[j].textContent.trim();
            if (text.length >= 1 && text.length <= 80 && !/^\d+$/.test(text)
                    && text.toLowerCase().indexOf('messenger') === -1) {
                return text;
            }
        }
        return '';
    }

    // ---------------------------------------------------------------------------
    // getAllThreadLinks() — expanded link selector that covers all thread URL patterns.
    // De-duplicated by href to avoid processing the same link multiple times.
    // Used in getActivitySnapshot() and getFirstUnreadSenderName().
    // ---------------------------------------------------------------------------
    function getAllThreadLinks() {
        var seen = {};
        var result = [];
        var selectors = [
            'a[href*="/t/"]',
            'a[href*="/e2ee/"]',
            'a[href*="thread_fbid"]',
            '[role="link"][tabindex]'
        ];
        for (var si = 0; si < selectors.length; si++) {
            try {
                var nodes = document.querySelectorAll(selectors[si]);
                for (var ni = 0; ni < nodes.length; ni++) {
                    var node = nodes[ni];
                    var key = (node.getAttribute('href') || '') + '||' + (node.getAttribute('data-key') || ni);
                    if (!seen[key]) {
                        seen[key] = true;
                        result.push(node);
                    }
                }
            } catch(_) {}
        }
        return result;
    }

    // ---------------------------------------------------------------------------
    // Sender lookup — only returns a name when a confirmed unread signal exists.
    // Does NOT fall back to the first conversation; that risks wrong sender name.
    // Uses expanded link selectors via getAllThreadLinks().
    // ---------------------------------------------------------------------------
    function getFirstUnreadSenderName() {
        // Phase A1 telemetry: log full extraction trace per IPC call so we can
        // diagnose macOS sender-name regression (H1). Each call summarises
        // link count, candidates considered, and final result.
        var diag = { totalLinks: 0, scanned: 0, unreadHits: 0, rejected: [], picked: '' };
        try {
            var links = getAllThreadLinks();
            diag.totalLinks = links.length;
            for (var i = 0; i < Math.min(links.length, 10); i++) {
                diag.scanned++;
                var link = links[i];
                var sig = detectUnreadSignals(link);
                if (!sig.isUnread) {
                    if (diag.rejected.length < 5) {
                        diag.rejected.push('i=' + i + ' notUnread aria=' + (sig.hasAria ? 'y' : 'n')
                            + ' badge=' + (sig.hasBadge ? 'y' : 'n') + ' bold=' + (sig.hasBold ? 'y' : 'n')
                            + ' dot=' + (sig.hasDot ? 'y' : 'n'));
                    }
                    continue;
                }
                diag.unreadHits++;
                var name = extractNameFromLink(link, sig);
                if (name.length >= 1) {
                    diag.picked = name;
                    jlog('[Sender] ' + JSON.stringify(diag) + ' result="' + name + '"');
                    return name;
                } else {
                    if (diag.rejected.length < 5) {
                        diag.rejected.push('i=' + i + ' unread but extractName returned ""'
                            + ' aria=' + JSON.stringify((link.getAttribute('aria-label') || '').slice(0, 60)));
                    }
                }
            }
        } catch(e) {
            jlog('[Sender] EXCEPTION: ' + (e && e.message ? e.message : String(e)));
        }
        jlog('[Sender] ' + JSON.stringify(diag) + ' result=""');
        return '';
    }

    // ---------------------------------------------------------------------------
    // Activity snapshot — best-effort fingerprint of plausible unread DOM content.
    //
    // Returns '' when count === 0 or when no meaningful unread text can be found
    // (prevents UI-only mutations from bumping the sequence).
    //
    // Uses composite unread-signal detection (aria + badge + bold + dot).
    // Throttled diagnostics when 0 candidates found.
    // Uses getAllThreadLinks() for expanded selector coverage.
    // ---------------------------------------------------------------------------
    var _lastDiagCount = -1;
    var _lastDiagLinkCount = -1;
    var _lastDiagTs = 0;

    // ---------------------------------------------------------------------------
    // Thread mutation sequence — counts meaningful DOM additions in the active
    // thread area so that follow-up messages from the same author (no sidebar
    // change) still cause activitySig to change while the app is unfocused.
    //
    // Design:
    //  - threadMutSeq is a monotonically increasing integer, appended as "||M<n>"
    //    to the activity snapshot when > 0.
    //  - tryBumpThreadMutSeq() is called from the thread MutationObserver callback
    //    with the accumulated mutation records.  It bumps only when:
    //      1. currentCount > 0 (unread messages exist)
    //      2. cooldown since last bump has passed (THREAD_MUT_COOLDOWN_MS)
    //      3. session budget not exceeded (THREAD_MUT_MAX_PER_SESSION)
    //      4. at least one added node has meaningful text/content and is NOT a
    //         date/time label or system message label (deepHasSubstance check).
    //  - threadMutSeq and lastThreadMutSeqAt are reset in resetActivityState().
    //
    // The old getThreadFingerprint() text snapshot is replaced by this counter.
    // Date/time labels that previously created a "stable but misleading" snapshot
    // (e.g. rows=1 last="2:07") can no longer affect activitySig.
    // ---------------------------------------------------------------------------
    var THREAD_MUT_COOLDOWN_MS = 1200;
    var THREAD_MUT_MAX_PER_SESSION = 120;
    var threadMutSeq = 0;
    var lastThreadMutSeqAt = 0;

    // Diagnostics for thread root — capped to avoid log spam.
    var _threadDiagSampleCount = 0;
    var _threadDiagMaxSamples = 6;
    var _threadDiagLastTs = 0;
    var _threadDiagThrottleMs = 15000;

    function emitThreadDiag(threadRoot) {
        try {
            var now = Date.now();
            if (_threadDiagSampleCount >= _threadDiagMaxSamples) return;
            if ((now - _threadDiagLastTs) < _threadDiagThrottleMs) return;
            _threadDiagSampleCount++;
            _threadDiagLastTs = now;
            var roleMain = document.querySelectorAll('[role="main"]').length;
            var rolePresentation = document.querySelectorAll('[role="presentation"]').length;
            var msgTable = document.querySelectorAll('[data-scope="messages_table"]').length;
            var shadowHosts = 0;
            try {
                var all = (threadRoot || document.body).querySelectorAll('*');
                for (var si2 = 0; si2 < Math.min(all.length, 200); si2++) {
                    if (all[si2].shadowRoot) shadowHosts++;
                }
            } catch(_) {}
            jlog('[ThreadDiag #' + _threadDiagSampleCount + '] '
                + 'role=main:' + roleMain
                + ' role=presentation:' + rolePresentation
                + ' messages_table:' + msgTable
                + ' shadowHosts:' + shadowHosts
                + (threadRoot ? ' root=' + (threadRoot.getAttribute('data-scope') || threadRoot.getAttribute('role') || threadRoot.tagName) : ' root=none'));
        } catch(_) {}
    }

    // Returns true if text looks like a date, time, or system/event label
    // rather than an actual chat message.
    function isDateOrSystemLabelText(txt) {
        if (!txt || txt.length === 0) return true;
        // Pure time: "2:07", "8:41 AM", "12:00 PM", "Ne 8:41", "So 12:00"
        // Day-time prefix: two-letter or three-letter day abbreviation + time
        if (/^[A-Za-z\u00C0-\u024F]{1,3}\s+\d{1,2}:\d{2}/.test(txt)) return true;
        if (/^\d{1,2}:\d{2}(\s*(AM|PM))?$/.test(txt)) return true;
        // Common day/date labels: "Monday", "Yesterday", "Today", short dates
        if (/^(Today|Yesterday|Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)$/i.test(txt)) return true;
        // Short numeric date: "1/2", "12.3.", "Apr 2"
        if (/^\d{1,2}[./]\d{1,2}\.?$/.test(txt)) return true;
        if (/^[A-Za-z]{3}\s+\d{1,2}$/.test(txt)) return true;
        // Single emoji or punctuation only
        if (/^[\u2000-\uFFFF\s.,:!?]{1,3}$/.test(txt)) return true;
        // Typing indicator patterns
        if (/^[.\u2026]{1,5}$/.test(txt)) return true;
        return false;
    }

    // Returns true if an added DOM node has substantial content:
    // - Is an element with non-trivial inner text that is not a date/system label.
    // - OR is a non-empty text node child of such an element.
    function nodeHasSubstance(node) {
        try {
            if (node.nodeType === 3) {
                // Text node
                var t = node.textContent.replace(/\s+/g, ' ').trim();
                return t.length >= 2 && !isDateOrSystemLabelText(t);
            }
            if (node.nodeType !== 1) return false;
            // Element node: check its text content
            var txt = node.textContent.replace(/\s+/g, ' ').trim();
            if (txt.length < 2 || txt.length > 600) return false;
            if (isDateOrSystemLabelText(txt)) return false;
            // Skip obvious typing indicators
            if (/^[.\u2026\s]{1,5}$/.test(txt)) return false;
            return true;
        } catch(_) { return false; }
    }

    // deepHasSubstance: like nodeHasSubstance but also checks descendants up to
    // `depth` levels deep. Allows wrapper nodes to pass if their children have
    // meaningful content. Skips date/time/system labels and typing indicators.
    function deepHasSubstance(node, depth) {
        if (depth === undefined) depth = 3;
        if (nodeHasSubstance(node)) return true;
        if (depth <= 0 || node.nodeType !== 1) return false;
        try {
            var children = node.childNodes;
            for (var ci = 0; ci < Math.min(children.length, 12); ci++) {
                if (deepHasSubstance(children[ci], depth - 1)) return true;
            }
        } catch(_) {}
        return false;
    }

    // Called from the thread MutationObserver callback (after debounce).
    // Bumps threadMutSeq if conditions are met, returns true if bumped.
    function tryBumpThreadMutSeq(mutationsList, currentCount) {
        if (currentCount <= 0) {
            // jlog('[ThreadMut] skip: count=0');
            return false;
        }
        var now = Date.now();
        if ((now - lastThreadMutSeqAt) < THREAD_MUT_COOLDOWN_MS) {
            jlog('[ThreadMut] skip: cooldown remaining=' + (THREAD_MUT_COOLDOWN_MS - (now - lastThreadMutSeqAt)) + 'ms');
            return false;
        }
        if (threadMutSeq >= THREAD_MUT_MAX_PER_SESSION) {
            jlog('[ThreadMut] skip: budget exhausted (' + THREAD_MUT_MAX_PER_SESSION + ')');
            return false;
        }
        // Check for at least one meaningful added node using deepHasSubstance
        var foundSubstantial = false;
        for (var mi = 0; mi < mutationsList.length && !foundSubstantial; mi++) {
            var added = mutationsList[mi].addedNodes;
            for (var ai = 0; ai < added.length && !foundSubstantial; ai++) {
                if (deepHasSubstance(added[ai], 3)) {
                    foundSubstantial = true;
                }
            }
        }
        if (!foundSubstantial) {
            jlog('[ThreadMut] skip: no substantial added nodes in ' + mutationsList.length + ' mutations');
            return false;
        }
        threadMutSeq++;
        lastThreadMutSeqAt = now;
        jlog('[ThreadMut] bumped seq=' + threadMutSeq + ' count=' + currentCount);
        return true;
    }

    function getActivitySnapshot(count) {
        if (count === 0) return '';
        try {
            var parts = [];
            var links = getAllThreadLinks();
            var checked = 0;
            for (var i = 0; i < links.length && checked < 5; i++) {
                var link = links[i];
                var signals = detectUnreadSignals(link);
                if (!signals.isUnread) continue;
                checked++;
                // Normalise: aria-label if available, else first dir="auto" span text.
                var label = '';
                if (signals.hasAria && signals.ariaLabel.length >= 2
                        && signals.ariaLabel.length <= 200) {
                    label = signals.ariaLabel;
                } else {
                    var autoSpans = link.querySelectorAll('span[dir="auto"]');
                    for (var j = 0; j < autoSpans.length; j++) {
                        var t = autoSpans[j].textContent.replace(/\s+/g, ' ').trim();
                        if (t.length >= 2 && t.length <= 100 && !/^\d+$/.test(t)) {
                            label = t;
                            break;
                        }
                    }
                }
                if (label.length >= 2) {
                    parts.push(label.slice(0, 100));
                }
            }
            if (parts.length === 0) {
                // Throttled diagnostics: log only when count/linkCount changes or >5 s elapsed.
                var now = Date.now();
                if (count !== _lastDiagCount
                        || links.length !== _lastDiagLinkCount
                        || (now - _lastDiagTs) > 5000) {
                    _lastDiagCount = count;
                    _lastDiagLinkCount = links.length;
                    _lastDiagTs = now;
                    var diagParts = [];
                    for (var di = 0; di < Math.min(links.length, 5); di++) {
                        var dl = links[di];
                        var dhref = (dl.getAttribute('href') || '').slice(0, 40);
                        var daria = (dl.getAttribute('aria-label') || '').slice(0, 60);
                        var dspans = dl.querySelectorAll('span[dir="auto"]');
                        var dspan0 = dspans.length > 0
                            ? dspans[0].textContent.trim().slice(0, 30) : '';
                        var dsig = detectUnreadSignals(dl);
                        var dfw = '';
                        if (dspans.length > 0) {
                            try {
                                dfw = window.getComputedStyle(dspans[0]).fontWeight;
                            } catch(_) { dfw = '?'; }
                        }
                        diagParts.push('[' + di + '] href=' + dhref
                            + ' aria=' + JSON.stringify(daria)
                            + ' span0=' + JSON.stringify(dspan0)
                            + ' fw=' + dfw
                            + ' badge=' + dsig.hasBadge
                            + ' bold=' + dsig.hasBold
                            + ' dot=' + dsig.hasDot);
                    }
                    jlog('[Activity] 0 candidates count=' + count
                        + ' links=' + links.length
                        + (diagParts.length ? '\n  ' + diagParts.join('\n  ') : ''));
                }
                // Append mutation counter (if any) so same-author follow-ups
                // still change the snapshot even with 0 sidebar candidates.
                if (threadMutSeq > 0) {
                    return '||M' + threadMutSeq;
                }
                return '';
            }
            var joined = parts.join('|').slice(0, 260);
            // Append thread mutation counter when it has been bumped.
            if (threadMutSeq > 0) {
                joined = (joined + '||M' + threadMutSeq).slice(0, 300);
            }
            jlog('[Activity] snapshot candidates=' + parts.length
                + (threadMutSeq > 0 ? ' +M' + threadMutSeq : '')
                + ' snapshot=' + JSON.stringify(joined.slice(0, 80)));
            return joined;
        } catch(e) { jlog('[Activity] getActivitySnapshot failed: ' + String(e)); }
        return '';
    }

    // ---------------------------------------------------------------------------
    // Activity signature tracking.
    //
    // `activitySeq` is bumped ONLY when count > 0 AND the DOM snapshot has
    // changed from `lastActivitySnapshot`.  This prevents high-frequency UI
    // mutations (presence dots, typing, read-receipts) from spamming the counter.
    //
    // Signature format: "<count>:<seq>:<snapshot>"
    //   - snapshot: first 40 chars of lastActivitySnapshot, or '' when empty.
    //     An empty snapshot produces an empty sig (same as count=0) so that
    //     title oscillation (1→0→1) with no snapshot change does not alter
    //     the sig and cannot trigger repeated notifications.
    //   - When snapshot is non-empty and changes (new message in same count),
    //     activitySeq is incremented so the sig changes and Rust will notify.
    //
    // Zero-timer: when count drops to 0, preserve baseline for ~7 s so transient
    // title bounces don't erase it.  Reset fully only after 7 s or when focused.
    // ---------------------------------------------------------------------------
    var activitySeq = 0;
    var lastActivitySnapshot = '';
    var lastSentSig = '';
    var activityDebounceTimer = null;
    var zeroTimer = null;
    var lastRisingEdgeBucket = 0;   // set on 0→positive transition — logged only, not in sig
    var prevTitleCount = 0;         // track previous count for rising-edge logging

    function coarseBucket() {
        return Math.floor(Date.now() / 2000);
    }

    // Call this whenever count transitions from 0 → positive.
    function markRisingEdge(newCount) {
        if (prevTitleCount === 0 && newCount > 0) {
            lastRisingEdgeBucket = coarseBucket();
            jlog('[Activity] risingEdge bucket=' + lastRisingEdgeBucket);
        }
        prevTitleCount = newCount;
    }

    // Build sig. Format: "<count>:<seq>:<snapshot>"
    // Returns '' when count === 0 OR when lastActivitySnapshot is empty
    // (no unread DOM candidates yet).  An empty sig is stable across title
    // oscillations so Rust will not fire zero-bounce-sig-changed repeatedly.
    function buildSig(count) {
        if (count === 0) return '';
        if (lastActivitySnapshot.length === 0) return '';
        var sig = count + ':' + activitySeq + ':' + lastActivitySnapshot.slice(0, 40);
        return sig;
    }

    // Soft reset: clear seq/snapshot but keep lastSentSig so Rust doesn't re-notify.
    // Called after the zero-timer fires or on focused-zero.
    function resetActivityState() {
        activitySeq = 0;
        lastActivitySnapshot = '';
        lastSentSig = '';
        lastRisingEdgeBucket = 0;
        prevTitleCount = 0;
        threadMutSeq = 0;
        lastThreadMutSeqAt = 0;
        _threadDiagSampleCount = 0;
        _threadDiagLastTs = 0;
        if (activityDebounceTimer !== null) {
            clearTimeout(activityDebounceTimer);
            activityDebounceTimer = null;
        }
        if (zeroTimer !== null) {
            clearTimeout(zeroTimer);
            zeroTimer = null;
        }
        jlog('[Activity] resetActivityState — seq/snapshot/sig/threadMut cleared');
    }

    // Called when count becomes 0. Starts zero-timer but does NOT immediately
    // clear baseline so transient bounces don't lose it.
    function scheduleZeroReset() {
        if (zeroTimer !== null) return; // already pending
        zeroTimer = setTimeout(function() {
            zeroTimer = null;
            var currentCount = getUnreadCountFromTitle();
            if (currentCount === 0) {
                jlog('[Activity] zeroTimer fired — resetting baseline (zero persisted)');
                resetActivityState();
            } else {
                jlog('[Activity] zeroTimer fired but count=' + currentCount + ' — keeping baseline');
            }
        }, 7000);
        jlog('[Activity] zeroTimer scheduled (7 s)');
    }

    // Cancel the zero-timer when count goes back positive (bounce recovery).
    function cancelZeroTimer() {
        if (zeroTimer !== null) {
            clearTimeout(zeroTimer);
            zeroTimer = null;
            jlog('[Activity] zeroTimer cancelled (count went positive)');
        }
    }

    function refreshActivitySnapshot() {
        var currentCount = getUnreadCountFromTitle();
        if (currentCount === 0) return;
        var snap = getActivitySnapshot(currentCount);
        if (snap.length === 0) return;
        if (snap !== lastActivitySnapshot) {
            if (lastActivitySnapshot.length === 0) {
                // Establish a baseline only. The first non-empty snapshot often
                // appears after the count-triggered notification already fired;
                // treating that as new activity would duplicate the first alert.
                lastActivitySnapshot = snap;
                // Update lastSentSig to the baseline sig so the polling fallback
                // does not immediately fire a duplicate because sig became non-empty
                // after the count-increase notification already went out.
                lastSentSig = buildSig(currentCount);
                jlog('[Activity] baseline established count=' + currentCount
                    + ' snapshot=' + JSON.stringify(snap.slice(0, 60))
                    + ' baselineSig=' + JSON.stringify(lastSentSig));
                return;
            }
            lastActivitySnapshot = snap;
            activitySeq++;
            jlog('[Activity] activitySeq=' + activitySeq
                + ' count=' + currentCount
                + ' snapshot=' + JSON.stringify(snap.slice(0, 60)));
        }
    }

    // ---------------------------------------------------------------------------
    // Shared mutation callback path for all observers (sidebar, thread, body
    // fallback, shadow DOM).  Accumulates mutations into _pendingThreadMutations
    // and drives the debounce+bump path.
    // ---------------------------------------------------------------------------
    var _pendingThreadMutations = [];
    var _threadMutDebounceTimer = null;

    // Pre-filter for body-level fallback observer: only pass mutations that
    // are plausibly relevant (ancestors include thread/main/messages_table/
    // presentation roles, or the target itself is a thread-related role).
    function _isRelevantMutation(mutation) {
        try {
            var node = mutation.target;
            for (var depth = 0; depth < 8 && node && node !== document.body; depth++) {
                var role = node.getAttribute ? (node.getAttribute('role') || '') : '';
                var ds = node.getAttribute ? (node.getAttribute('data-scope') || '') : '';
                if (role === 'main' || role === 'presentation' || role === 'complementary'
                        || role === 'navigation' || ds === 'messages_table') {
                    return true;
                }
                node = node.parentElement;
            }
        } catch(_) {}
        return false;
    }

    function _processMutationBatch(mutationsList, fromSource) {
        // Accumulate mutations for the debounce window.
        for (var i = 0; i < mutationsList.length; i++) {
            _pendingThreadMutations.push(mutationsList[i]);
        }
        // Debounce: wait THREAD_MUT_COOLDOWN_MS before processing.
        if (_threadMutDebounceTimer !== null) {
            clearTimeout(_threadMutDebounceTimer);
        }
        _threadMutDebounceTimer = setTimeout(function() {
            _threadMutDebounceTimer = null;
            var batch = _pendingThreadMutations;
            _pendingThreadMutations = [];
            var currentCount = getUnreadCountFromTitle();
            jlog('[ThreadMut] batch size=' + batch.length + ' source=' + fromSource + ' count=' + currentCount);
            var bumped = tryBumpThreadMutSeq(batch, currentCount);
            if (bumped) {
                refreshActivitySnapshot();
            }
        }, THREAD_MUT_COOLDOWN_MS);
    }

    // Bump activitySeq when conversation-list or active-thread childList changes
    // while count > 0. Narrowed to childList+subtree only — avoids attribute /
    // characterData churn from presence dots, typing indicators, read receipts.
    // Debounced 400 ms so that a burst of React reconciliation mutations only
    // triggers one snapshot comparison.
    //
    // Observers installed:
    //  1. Sidebar (navigation/complementary) — existing conversation-list watcher.
    //  2. Thread area ([data-scope="messages_table"] or [role="main"]) — so that
    //     follow-up messages from the same author change the fingerprint even when
    //     the sidebar does not update.
    //  3. Body-level fallback observer — catches mutations outside thread/sidebar
    //     with pre-filter for relevant ancestors.
    //  4. Shadow DOM observer chaining — observes shadow roots under thread/body.
    //
    // Guards: we tag observed nodes with a JS property so we never install
    // multiple observers on the same node. The thread observer uses retry
    // (via setInterval) because Messenger SPA may mount the thread root later.

    var _threadObserverInstalled = false;
    var _threadObserverRetryTimer = null;
    var _bodyFallbackInstalled = false;
    var _installedShadowRoots = [];

    function _makeDebounced() {
        return function() {
            if (activityDebounceTimer !== null) {
                clearTimeout(activityDebounceTimer);
            }
            activityDebounceTimer = setTimeout(function() {
                activityDebounceTimer = null;
                refreshActivitySnapshot();
            }, 400);
        };
    }

    // ---------------------------------------------------------------------------
    // Shadow DOM observer chaining
    // Detect shadowRoot hosts under threadRoot/body and observe them with the
    // same mutation path. Avoid duplicate installs via _installedShadowRoots set.
    // ---------------------------------------------------------------------------
    function tryObserveShadowRoots(scopeEl) {
        if (!scopeEl) return;
        try {
            var all = scopeEl.querySelectorAll('*');
            for (var si3 = 0; si3 < Math.min(all.length, 300); si3++) {
                var host = all[si3];
                if (host.shadowRoot && _installedShadowRoots.indexOf(host) === -1) {
                    _installedShadowRoots.push(host);
                    var shadowObs = new MutationObserver(function(muts) {
                        _processMutationBatch(muts, 'shadow');
                    });
                    shadowObs.observe(host.shadowRoot, { childList: true, subtree: true });
                    jlog('[Activity] shadow observer installed on ' + (host.tagName || 'unknown'));
                }
            }
        } catch(_) {}
    }

    function tryInstallThreadObserver() {
        if (_threadObserverInstalled) return;
        try {
            var threadRoot = document.querySelector('[data-scope="messages_table"]')
                           || document.querySelector('[role="main"]');
            if (!threadRoot) return; // not mounted yet — retry will fire again
            if (threadRoot._mxThreadObserverInstalled) return; // already tagged
            threadRoot._mxThreadObserverInstalled = true;
            _threadObserverInstalled = true;
            if (_threadObserverRetryTimer !== null) {
                clearInterval(_threadObserverRetryTimer);
                _threadObserverRetryTimer = null;
            }
            emitThreadDiag(threadRoot);
            var threadObserver = new MutationObserver(function(mutationsList) {
                _processMutationBatch(mutationsList, 'thread');
            });
            threadObserver.observe(threadRoot, {
                childList: true,
                subtree: true
            });
            jlog('[Activity] thread observer installed on '
                + (threadRoot.getAttribute('data-scope') || threadRoot.getAttribute('role') || 'main'));
            // Also chain shadow DOM observers under the thread root.
            tryObserveShadowRoots(threadRoot);
        } catch(e) {
            jlog('[Activity] tryInstallThreadObserver failed: ' + String(e));
        }
    }

    // ---------------------------------------------------------------------------
    // Body-level fallback observer
    // Observes document.body childList+subtree with a pre-filter for relevant
    // thread/main/messages_table/presentation ancestors. Feeds the same pending
    // mutation/debounce path as the thread observer.
    // ---------------------------------------------------------------------------
    function tryInstallBodyFallbackObserver() {
        if (_bodyFallbackInstalled) return;
        if (!document.body) return;
        if (document.body._mxBodyFallbackInstalled) return;
        document.body._mxBodyFallbackInstalled = true;
        _bodyFallbackInstalled = true;
        var bodyObs = new MutationObserver(function(mutationsList) {
            var relevant = [];
            for (var i = 0; i < mutationsList.length; i++) {
                if (_isRelevantMutation(mutationsList[i])) {
                    relevant.push(mutationsList[i]);
                }
            }
            if (relevant.length > 0) {
                _processMutationBatch(relevant, 'body-fallback');
            }
        });
        bodyObs.observe(document.body, { childList: true, subtree: true });
        jlog('[Activity] body-level fallback observer installed');
        // Chain shadow DOM observers under body too.
        tryObserveShadowRoots(document.body);
    }

    function setupActivityObserver() {
        try {
            var listRoot = document.querySelector('[role="navigation"]')
                        || document.querySelector('[role="complementary"]')
                        || document.querySelector('nav')
                        || document.body;
            if (!listRoot) return;
            if (!listRoot._mxSidebarObserverInstalled) {
                listRoot._mxSidebarObserverInstalled = true;
                var convObserver = new MutationObserver(_makeDebounced());
                // childList + subtree only — no attributes/characterData.
                convObserver.observe(listRoot, {
                    childList: true,
                    subtree: true
                });
                jlog('[Activity] sidebar observer installed on '
                    + (listRoot.getAttribute('role') || listRoot.tagName));
            }
        } catch(e) {
            jlog('[Activity] Failed to set up sidebar activity observer: ' + String(e));
        }

        // Thread observer — attempt immediately, then retry every 2 s until mounted.
        tryInstallThreadObserver();
        if (!_threadObserverInstalled) {
            _threadObserverRetryTimer = setInterval(function() {
                tryInstallThreadObserver();
                // Also retry shadow root scan periodically.
                if (_threadObserverInstalled) {
                    tryObserveShadowRoots(document.querySelector('[data-scope="messages_table"]')
                        || document.querySelector('[role="main"]') || document.body);
                }
            }, 2000);
        }

        // Body-level fallback observer — always install in addition to thread observer.
        tryInstallBodyFallbackObserver();
    }

    // ---------------------------------------------------------------------------
    // Iframe postMessage bridge for __mx_thread_bump__
    // In non-top frames: when threadMutSeq bumps, post {type:'__mx_thread_bump__', seq}
    // to top. In top frame: receive and merge the seq then refreshActivitySnapshot.
    // Preserves existing __mx_notif__ bridge behavior.
    // ---------------------------------------------------------------------------
    if (window !== window.top) {
        // Non-top frame: hook threadMutSeq bumps by wrapping tryBumpThreadMutSeq.
        var _origTryBump = tryBumpThreadMutSeq;
        tryBumpThreadMutSeq = function(mutationsList, currentCount) {
            var bumped = _origTryBump(mutationsList, currentCount);
            if (bumped) {
                try {
                    window.top.postMessage({
                        type: '__mx_thread_bump__',
                        seq: threadMutSeq
                    }, '*');
                } catch(_) {}
            }
            return bumped;
        };
    }
    if (window === window.top) {
        window.addEventListener('message', function(e) {
            if (!e.data || typeof e.data !== 'object') return;
            if (e.data.type === '__mx_thread_bump__') {
                var remoteSeq = parseInt(e.data.seq, 10) || 0;
                if (remoteSeq > threadMutSeq) {
                    threadMutSeq = remoteSeq;
                    lastThreadMutSeqAt = Date.now();
                    jlog('[ThreadMut] iframe bump received seq=' + threadMutSeq + ' from=' + String(e.origin));
                    refreshActivitySnapshot();
                }
            }
        });
    }

    // ---------------------------------------------------------------------------
    // Send helpers
    // ---------------------------------------------------------------------------

    // Dispatch count + sender + activitySig to Rust immediately (used for count === 0).
    // For count=0: send activitySig="" immediately, but preserve baseline (zero-timer
    // will clean up after 7 s if zero persists, or on window focus).
    function sendUnreadCountNow(count) {
        try {
            var sender = '';
            var activitySig = '';
            if (count === 0) {
                // Send empty sig immediately, but keep baseline alive (zero-timer).
                // Focused zero: reset baseline right away (user saw messages).
                var focused = false;
                try { focused = typeof document.hasFocus === 'function' && document.hasFocus(); } catch(_) {}
                if (focused) {
                    resetActivityState();
                } else {
                    scheduleZeroReset();
                }
            } else {
                sender = getFirstUnreadSenderName();
                emitSenderHintViaTitle(count, sender);
                activitySig = buildSig(count);
            }
            jlog('[Unread] immediate count=' + count
                + ' sender=' + JSON.stringify(sender)
                + ' activitySig=' + JSON.stringify(activitySig));
            window.__TAURI__.core.invoke('update_unread_count',
                { count: count, sender: sender, activitySig: activitySig });
            if (count > 0) {
                lastSentSig = activitySig;
            }
        } catch(e) { jlog('[Unread] Failed to send unread count: ' + String(e)); }
    }

    // For count > 0, defer ~500 ms so React DOM has time to render the sender.
    // Optimistically records lastSentSig before setTimeout to prevent duplicate
    // scheduling for the same sig.  Reverts lastSentSig if the send is skipped.
    function sendUnreadCount(count) {
        if (count === 0) {
            sendUnreadCountNow(0);
            return;
        }
        // Cancel any pending zero-timer since count went positive again.
        cancelZeroTimer();
        // Refresh snapshot now (count > 0) so seq is up-to-date before we
        // build the optimistic sig.
        refreshActivitySnapshot();
        var optimisticSig = buildSig(count);
        // Optimistic update — prevents duplicate scheduling for same sig.
        var prevLastSentSig = lastSentSig;
        lastSentSig = optimisticSig;
        setTimeout(function() {
            var currentCount = getUnreadCountFromTitle();
            if (currentCount === 0) {
                // Count dropped — revert optimistic sig so next real event fires.
                lastSentSig = prevLastSentSig;
                jlog('[Unread] delayed send skipped (count now 0, was '
                    + count + ')');
                return;
            }
            var effectiveCount = currentCount;
            // Re-read snapshot in case more DOM settled during the 500 ms wait.
            refreshActivitySnapshot();
            var sender = getFirstUnreadSenderName();
            emitSenderHintViaTitle(effectiveCount, sender);
            var activitySig = buildSig(effectiveCount);
            // Update lastSentSig to the final sig (may differ from optimistic if
            // snapshot changed during the delay).
            lastSentSig = activitySig;
            jlog('[Unread] delayed count=' + effectiveCount
                + ' sender=' + JSON.stringify(sender)
                + ' activitySig=' + JSON.stringify(activitySig)
                + (effectiveCount !== count ? ' (count changed from ' + count + ')' : ''));
            try {
                window.__TAURI__.core.invoke('update_unread_count',
                    { count: effectiveCount, sender: sender, activitySig: activitySig });
            } catch(e) { jlog('[Unread] Failed to send unread count: ' + String(e)); }
        }, 500);
    }

    function setupObserver() {
        var lastCount = getUnreadCountFromTitle();
        prevTitleCount = lastCount;
        sendUnreadCount(lastCount);
        setupActivityObserver();
        var titleElement = document.querySelector('title');
        if (titleElement) {
            var observer = new MutationObserver(function() {
                var newCount = getUnreadCountFromTitle();
                if (newCount !== lastCount) {
                    // Track rising edge BEFORE updating lastCount.
                    markRisingEdge(newCount);
                    lastCount = newCount;
                    sendUnreadCount(newCount);
                }
            });
            observer.observe(titleElement, { childList: true, characterData: true, subtree: true });
        }
        // Polling fallback: also catches sig changes (new message, same count).
        setInterval(function() {
            var newCount = getUnreadCountFromTitle();
            if (newCount > 0) {
                refreshActivitySnapshot();
            }
            var currentSig = buildSig(newCount);
            if (newCount !== lastCount || (newCount > 0 && currentSig !== lastSentSig)) {
                markRisingEdge(newCount);
                lastCount = newCount;
                sendUnreadCount(newCount);
            }
        }, 2000);

        // When window gains focus and count=0, immediately reset baseline so
        // the next new message starts fresh.
        window.addEventListener('focus', function() {
            var fc = getUnreadCountFromTitle();
            if (fc === 0 && (lastActivitySnapshot.length > 0 || zeroTimer !== null)) {
                jlog('[Activity] window focused with count=0 — resetting baseline');
                resetActivityState();
            }
        });
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

/// Builds a JavaScript snippet that overrides `window.matchMedia` so that
/// `prefers-color-scheme` queries return the forced appearance mode.
///
/// The script first checks `sessionStorage['__mx_appearance']` for a runtime
/// override (set by the tray/menu appearance handler when switching themes),
/// then falls back to the baked-in `mode` string from Rust settings.  This
/// two-layer approach means:
///
/// - **Startup**: the baked-in mode applies (sessionStorage is empty).
/// - **Runtime switch**: the handler sets sessionStorage and reloads the page;
///   the init-script re-runs on the new load and picks up the sessionStorage
///   value, so the new theme takes effect without restarting the app.
/// - **App restart**: sessionStorage is cleared; the new baked-in mode
///   (rebuilt from updated Rust settings) applies.
///
/// Baked-in `mode` values: `"dark"`, `"light"`, or `"system"` (no override).
fn build_appearance_script(mode: &str) -> String {
    format!(
        r#"(function() {{
    try {{
        var _rt = sessionStorage.getItem('__mx_appearance');
        var _m = (_rt !== null) ? _rt : '{mode}';
        if (_m === 'dark' || _m === 'light') {{
            var _fd = (_m === 'dark');
            var _o = window.matchMedia.bind(window);
            window.matchMedia = function(q) {{
                if (typeof q === 'string' && q.indexOf('prefers-color-scheme') !== -1) {{
                    var _d = q.indexOf('dark') !== -1;
                    return {{
                        matches: _fd ? _d : !_d,
                        media: q,
                        onchange: null,
                        addListener: function() {{}},
                        removeListener: function() {{}},
                        addEventListener: function() {{}},
                        removeEventListener: function() {{}},
                        dispatchEvent: function() {{ return false; }}
                    }};
                }}
                return _o(q);
            }};
        }}
    }} catch(_e) {{}}
}})();"#,
        mode = mode
    )
}

/// Returns `true` when a persisted startup URL points to a concrete Messenger
/// thread that is safe to restore on next app launch.
///
/// E2EE thread URLs (`/e2ee/t/...`) are deliberately excluded — WKWebView
/// processes e2ee threads asynchronously and emits a blank title during
/// initial load, which the crash detector misinterprets as a WebKitWebProcess
/// crash → endless reload loop.
fn is_safe_messenger_startup_url(value: &str) -> bool {
    let Ok(parsed) = url::Url::parse(value) else {
        return false;
    };

    if parsed.scheme() != "https" {
        return false;
    }

    if parsed.host_str() != Some("www.messenger.com") {
        return false;
    }

    let path = parsed.path();
    path.starts_with("/t/")
}

/// Browser compatibility shims that make Messenger treat the embedded WebView
/// as a fully capable Chrome instance for audio and video calls.
///
/// **Problem 1 — `window.chrome` missing on WKWebView / WebKitGTK:**
/// We spoof a Chrome user-agent string so Messenger serves its full web-app.
/// However, on non-Chromium WebViews (macOS WKWebView, Linux WebKitGTK) the
/// `window.chrome` object that real Chrome exposes is absent.  Messenger (and
/// other React apps) detect this to decide whether to enable the WebRTC call
/// UI.  Without the stub, 1-to-1 voice/video call buttons remain disabled even
/// though the WebView supports WebRTC.
///
/// **Problem 2 — `navigator.mediaDevices` availability:**
/// Some older WebKit versions expose `navigator.mediaDevices` as `undefined`
/// (it requires a secure context AND the browser to have opted into media
/// devices support).  We expose a thin `getUserMedia` shim that delegates to
/// `webkitGetUserMedia` (still present on older WebKit) as a fallback so that
/// Messenger's capability detection does not give up prematurely.
///
/// Note: The `navigator.permissions.query` hook in `NOTIFICATION_OVERRIDE_SCRIPT`
/// already returns `'granted'` for `'microphone'` and `'camera'` — that is the
/// primary signal Messenger checks.  This script provides a second layer for
/// platforms / versions where the property existence check fires first.
///
/// Injected **at document-start** on all platforms.  WebView2 (Windows) is
/// already Chromium and has `window.chrome`, so the guard `if (!window.chrome)`
/// makes the injection a no-op there.
const CALL_COMPAT_SCRIPT: &str = r#"(function() {
    // -----------------------------------------------------------------------
    // 1. window.chrome stub
    //    Real Chrome exposes window.chrome with at minimum a runtime/app object.
    //    Messenger uses `'chrome' in window` or `!!window.chrome` to gate the
    //    call capability check on Chromium-based browsers.  We provide a minimal
    //    stub so the check passes on WebKit-based WebViews.
    //    Guard: skip if already present (WebView2 on Windows).
    // -----------------------------------------------------------------------
    try {
        if (!window.chrome) {
            window.chrome = {
                runtime: {},
                app: { isInstalled: false }
            };
        }
    } catch(e) {}

    // -----------------------------------------------------------------------
    // 2. navigator.mediaDevices shim
    //    On older WebKit, mediaDevices can be undefined even in a secure context.
    //    Provide a shim that delegates getUserMedia to the legacy
    //    navigator.getUserMedia / webkitGetUserMedia API so that Messenger's
    //    feature detection (`navigator.mediaDevices && navigator.mediaDevices
    //    .getUserMedia`) evaluates to truthy.
    //    Guard: skip if already present (any modern WebKit / Chromium).
    // -----------------------------------------------------------------------
    try {
        if (!navigator.mediaDevices) {
            Object.defineProperty(navigator, 'mediaDevices', {
                value: {
                    getUserMedia: function(constraints) {
                        var gUM = (navigator.getUserMedia ||
                                   navigator.webkitGetUserMedia ||
                                   navigator.mozGetUserMedia ||
                                   navigator.msGetUserMedia);
                        if (!gUM) {
                            return Promise.reject(new Error('getUserMedia not available'));
                        }
                        return new Promise(function(resolve, reject) {
                            gUM.call(navigator, constraints, resolve, reject);
                        });
                    },
                    enumerateDevices: function() {
                        return Promise.resolve([]);
                    }
                },
                writable: false, configurable: true
            });
        }
    } catch(e) {}
})();
"#;

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

    // Detect Facebook's link shim: l.facebook.com/l.php?u=REAL_URL
    // (also l.messenger.com/l.php?u=REAL_URL).
    // Messenger wraps all outbound links in this shim.  When the shim URL is
    // passed to window.open() it passes isAllowedUrl() (facebook.com subdomain)
    // and falls through to _originalOpen — which WKWebView silently drops
    // because WRY has no createWebViewWith UI delegate.  We extract the real
    // destination and open it via the Tauri opener instead.
    function extractLinkShimUrl(urlStr) {
        try {
            var parsed = new URL(urlStr);
            if ((parsed.hostname === 'l.facebook.com' || parsed.hostname === 'l.messenger.com')
                    && parsed.pathname === '/l.php') {
                return parsed.searchParams.get('u') || null;
            }
        } catch(e) {}
        return null;
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
            // Check for the Facebook/Messenger link shim BEFORE isAllowedUrl,
            // because l.facebook.com IS an allowed domain but window.open()
            // with that URL would be silently dropped by WKWebView (no
            // createWebViewWith UI delegate in WRY).
            var realUrl = extractLinkShimUrl(urlStr);
            if (realUrl) {
                jlog('Link shim in window.open — routing real URL to browser: ' + realUrl);
                openExternal(realUrl);
                return null;
            }
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

        // ── Blob URL → Save-As dialog ─────────────────────────────
        if (urlStr.startsWith('blob:')) {
            e.preventDefault();
            e.stopPropagation();
            var suggestedName = el.getAttribute('download') || 'download';
            downloadSaveAs(urlStr, suggestedName);
            return;
        }

        if (!urlStr.startsWith('http://') && !urlStr.startsWith('https://')) return;
        if (isAllowedUrl(urlStr)) return; // allowed — let Tauri handle it

        e.preventDefault();
        e.stopPropagation();
        openExternal(urlStr);
    }, true /* capture phase */);

    // -----------------------------------------------------------------------
    // 2.5 Download Save-As — blob URL → native Save As dialog
    // -----------------------------------------------------------------------
    async function downloadSaveAs(blobUrl, suggestedFilename) {
        try {
            // 1. Show native "Save As" dialog (async, non-blocking).
            var path = await window.__TAURI__.core.invoke('pick_save_path', {
                suggestedFilename: suggestedFilename
            });
            if (!path) {
                jlog('download save-as: user cancelled');
                return;
            }

            // 2. Fetch the blob data from the in-memory Blob store.
            jlog('download save-as: fetching ' + blobUrl);
            var response = await fetch(blobUrl);

            // 3. If the chosen path lacks a file extension, derive one
            //    from the response Content-Type header (e.g. image/jpeg → .jpg).
            if (path.indexOf('.') <= path.lastIndexOf('/')) {
                var ct = (response.headers.get('Content-Type') || '');
                var semi = ct.indexOf(';');
                if (semi >= 0) ct = ct.substring(0, semi);
                ct = ct.trim();
                var ext = ({
                    'image/jpeg': '.jpg',  'image/png': '.png',
                    'image/gif': '.gif',   'image/webp': '.webp',
                    'image/bmp': '.bmp',   'image/svg+xml': '.svg',
                    'application/pdf': '.pdf', 'video/mp4': '.mp4',
                    'video/webm': '.webm', 'audio/mpeg': '.mp3',
                    'audio/ogg': '.ogg',
                })[ct] || '';
                if (ext) path += ext;
            }

            // 4. Convert the response to a byte array and write to disk.
            var buffer = await response.arrayBuffer();
            var data = Array.from(new Uint8Array(buffer));

            jlog('download save-as: writing ' + data.length + ' bytes to ' + path);
            await window.__TAURI__.core.invoke('write_file_bytes', {
                path: path,
                data: data
            });

            jlog('download save-as: done → ' + path);
        } catch(e) {
            jlog('download save-as error: ' + (e.message || e));
        }
    }

    jlog('window.open override + click interceptor installed');
})();
"#;

/// Phase A diagnostic script (v1.3.23+).
///
/// Read-only instrumentation only — does **NOT** modify any Messenger behavior.
/// Logs are sent through `js_log` with the `[DiagJS]` prefix so they land in the
/// Rust log file alongside everything else.
///
/// What it captures:
/// 1. **A5 page-loaded ping** — fires once `DOMContentLoaded` plus once on
///    `load`, including `performance.now()` and a tiny set of WebView/UA
///    fingerprints. Used to confirm Win11 first-paint timing (H3).
/// 2. **A6 WebSocket proxy** — wraps `WebSocket.prototype.send` and the
///    `message` event listener registration, plus subscribes to `addEventListener`
///    so we can log incoming frames (count + size only — never payload bodies)
///    grouped per-URL. This is the read-only foundation for Phase C, and it
///    also tells us whether MQTT-over-WS is even reaching the page.
/// 3. **A6 fetch / XHR proxy** — wraps `fetch()` and `XMLHttpRequest.open()`
///    and logs only requests whose URL matches Messenger graph/messaging
///    endpoints. Method + URL + status only; no payload.
/// 4. **Focus / visibility heartbeat** — once per 30 s logs
///    `document.hasFocus()`, `document.visibilityState`, `document.hidden`.
///    Used to corroborate Linux Wayland focus-gating hypothesis (H2).
///
/// All loggers are throttled / capped so the log file does not explode.
const DIAGNOSTIC_TELEMETRY_SCRIPT: &str = concat!(
    r#"
(function() {
    var APP_VERSION = ""#,
    env!("CARGO_PKG_VERSION"),
    r#"";
    function dlog(msg) {
        try {
            window.__TAURI__.core.invoke('js_log', { message: '[DiagJS] ' + msg });
        } catch(_) {}
    }

    // -----------------------------------------------------------------------
    // 0. Early IPC availability probe (Fix 4 / Phase B diagnostics)
    //
    //    This MUST run before any other code so we know whether subsequent
    //    dlog() calls will actually reach the Rust log file.
    //
    //    On Linux VMware / AppImage with EGL/DRI3 degradation, WebKitGTK can
    //    fail to inject window.__TAURI__ into the page context even though
    //    initialization_scripts normally deliver it unconditionally.  When that
    //    happens every dlog() is silently swallowed — resulting in zero JS log
    //    lines in messengerx.log (exact symptom observed in Phase A Linux logs).
    //
    //    We emit a console.log in addition to dlog() so that the probe result
    //    is visible in the WebKit inspector even if IPC is broken.
    // -----------------------------------------------------------------------
    var _ipcAvailable = false;
    var _ipcDiag = 'unknown';
    try {
        var _tauriType  = typeof window.__TAURI__;
        var _coreType   = (_tauriType !== 'undefined') ? typeof window.__TAURI__.core   : 'n/a';
        var _invokeType = (_coreType  !== 'n/a')       ? typeof window.__TAURI__.core.invoke : 'n/a';
        _ipcAvailable = (_invokeType === 'function');
        _ipcDiag = 'tauriType=' + _tauriType
                 + ' coreType='   + _coreType
                 + ' invokeType=' + _invokeType;
    } catch(e) {
        _ipcDiag = 'probe-threw: ' + String(e);
    }
    // Always emit to console (visible in WebKit inspector regardless of IPC).
    try {
        console.log('[MessengerX][DiagJS][IPCProbe] ipcAvailable=' + _ipcAvailable + ' ' + _ipcDiag);
    } catch(_) {}
    // Also attempt dlog — will succeed only if IPC works.
    dlog('[IPCProbe] ipcAvailable=' + _ipcAvailable + ' ' + _ipcDiag);

    // -----------------------------------------------------------------------
    // 0b. Phase M trace — document-start beacon + SPA navigation wrappers.
    //
    //     `[DocStart]` is emitted at the earliest possible moment after the
    //     init script runs (before any DOMContentLoaded handlers fire).  Its
    //     `perfNow` value lets us cross-correlate with the Rust-side
    //     `[PageLoad] event=Started` log to identify whether the Win11 27 s
    //     white-screen gap lives in:
    //       (a) WebView2 → first-byte (large [DocStart] perfNow), or
    //       (b) document-start → DOMContentLoaded (small perfNow, large
    //           [PageLoaded] domcontentloaded delta), or
    //       (c) SPA hydration after DCL (visible via the history wrappers).
    //
    //     The history wrappers log every pushState/replaceState/popstate so
    //     Messenger's client-side router transitions during boot become
    //     traceable (e.g. `/` → `/t/<thread>` after auth restore).
    // -----------------------------------------------------------------------
    try {
        var _docStartPerfNow = (typeof performance !== 'undefined' && performance.now)
            ? Math.round(performance.now()) : -1;
        var _docStartUrl = (location && location.href) ? location.href.slice(0, 160) : '';
        dlog('[DocStart] perfNow=' + _docStartPerfNow + ' readyState=' + document.readyState + ' url=' + _docStartUrl);
    } catch(_) {}

    try {
        function _logHistory(kind, urlArg) {
            try {
                var perfNow = (typeof performance !== 'undefined' && performance.now)
                    ? Math.round(performance.now()) : -1;
                var resolved = '';
                try { resolved = new URL(String(urlArg || ''), location.href).href.slice(0, 160); }
                catch(_) { resolved = String(urlArg || '').slice(0, 160); }
                var current = (location && location.href) ? location.href.slice(0, 160) : '';
                dlog('[Nav][' + kind + '] perfNow=' + perfNow + ' to=' + resolved + ' from=' + current);
            } catch(_) {}
        }
        var _origPush = history.pushState;
        history.pushState = function(state, title, url) {
            _logHistory('pushState', url);
            return _origPush.apply(this, arguments);
        };
        var _origReplace = history.replaceState;
        history.replaceState = function(state, title, url) {
            _logHistory('replaceState', url);
            return _origReplace.apply(this, arguments);
        };
        window.addEventListener('popstate', function() {
            _logHistory('popstate', location.href);
        });
    } catch(_) {}

    // -----------------------------------------------------------------------
    // 1. Page-loaded ping (A5)
    // -----------------------------------------------------------------------
    var pingedDomReady = false;
    var pingedLoad = false;
    function pingPageLoaded(stage) {
        try {
            var nowMs = (typeof performance !== 'undefined' && performance.now)
                ? Math.round(performance.now()) : -1;
            var info = {
                v: APP_VERSION,
                stage: stage,
                t: nowMs,
                url: (location && location.href ? location.href.slice(0, 120) : ''),
                ua: (navigator && navigator.userAgent ? navigator.userAgent.slice(0, 80) : ''),
                visible: (document && document.visibilityState) ? document.visibilityState : '?',
                focus: (document && typeof document.hasFocus === 'function') ? !!document.hasFocus() : null
            };
            dlog('[PageLoaded] ' + JSON.stringify(info));
        } catch(_) {}
    }
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', function() {
            if (pingedDomReady) return;
            pingedDomReady = true;
            pingPageLoaded('domcontentloaded');
        });
    } else {
        pingedDomReady = true;
        pingPageLoaded('already-interactive');
    }
    window.addEventListener('load', function() {
        if (pingedLoad) return;
        pingedLoad = true;
        pingPageLoaded('load');
    });

    // -----------------------------------------------------------------------
    // 4. Focus / visibility heartbeat — done early so we have data even if
    //    other hooks fail.
    // -----------------------------------------------------------------------
    setInterval(function() {
        try {
            var info = {
                hasFocus: (typeof document.hasFocus === 'function') ? !!document.hasFocus() : null,
                visibility: document.visibilityState || '?',
                hidden: !!document.hidden,
                url: location.pathname + location.search.slice(0, 40)
            };
            dlog('[FocusHB] ' + JSON.stringify(info));
        } catch(_) {}
    }, 30000);

    // -----------------------------------------------------------------------
    // 2. WebSocket proxy (read-only) — counts frames per URL, throttled flush.
    // -----------------------------------------------------------------------
    try {
        var wsStats = Object.create(null);
        var wsFlushPending = false;
        function wsFlush() {
            wsFlushPending = false;
            try {
                var keys = Object.keys(wsStats);
                if (keys.length === 0) return;
                var summary = [];
                for (var i = 0; i < keys.length && i < 6; i++) {
                    var k = keys[i];
                    var s = wsStats[k];
                    summary.push({ url: k.slice(0, 60), out: s.out, in: s.in, bytes: s.bytes });
                }
                dlog('[WS] ' + JSON.stringify(summary));
                wsStats = Object.create(null);
            } catch(_) {}
        }
        function wsScheduleFlush() {
            if (wsFlushPending) return;
            wsFlushPending = true;
            setTimeout(wsFlush, 5000);
        }
        function wsBumpStats(url, dir, bytes) {
            try {
                var k = (url || '?').replace(/^wss?:\/\//, '').slice(0, 80);
                if (!wsStats[k]) wsStats[k] = { out: 0, in: 0, bytes: 0 };
                if (dir === 'in') wsStats[k].in++; else wsStats[k].out++;
                wsStats[k].bytes += (bytes | 0);
                wsScheduleFlush();
            } catch(_) {}
        }
        var NativeWS = window.WebSocket;
        if (NativeWS && NativeWS.prototype) {
            var origSend = NativeWS.prototype.send;
            NativeWS.prototype.send = function(data) {
                try {
                    var n = 0;
                    if (typeof data === 'string') n = data.length;
                    else if (data && data.byteLength != null) n = data.byteLength;
                    wsBumpStats(this.url, 'out', n);
                } catch(_) {}
                return origSend.apply(this, arguments);
            };
            var origAddEvt = NativeWS.prototype.addEventListener;
            NativeWS.prototype.addEventListener = function(type, listener, options) {
                if (type === 'message' && typeof listener === 'function') {
                    var url = this.url;
                    var wrapped = function(ev) {
                        try {
                            var n = 0;
                            if (ev && ev.data) {
                                if (typeof ev.data === 'string') n = ev.data.length;
                                else if (ev.data.byteLength != null) n = ev.data.byteLength;
                                else if (ev.data.size != null) n = ev.data.size;
                            }
                            wsBumpStats(url, 'in', n);
                        } catch(_) {}
                        return listener.apply(this, arguments);
                    };
                    return origAddEvt.call(this, type, wrapped, options);
                }
                return origAddEvt.apply(this, arguments);
            };
            // Also instrument onmessage setter (Messenger uses both APIs).
            try {
                var desc = Object.getOwnPropertyDescriptor(NativeWS.prototype, 'onmessage');
                if (desc && desc.set) {
                    Object.defineProperty(NativeWS.prototype, 'onmessage', {
                        configurable: true,
                        enumerable: desc.enumerable,
                        get: desc.get,
                        set: function(fn) {
                            var url = this.url;
                            if (typeof fn === 'function') {
                                var wrapped = function(ev) {
                                    try {
                                        var n = 0;
                                        if (ev && ev.data) {
                                            if (typeof ev.data === 'string') n = ev.data.length;
                                            else if (ev.data.byteLength != null) n = ev.data.byteLength;
                                            else if (ev.data.size != null) n = ev.data.size;
                                        }
                                        wsBumpStats(url, 'in', n);
                                    } catch(_) {}
                                    return fn.apply(this, arguments);
                                };
                                return desc.set.call(this, wrapped);
                            }
                            return desc.set.call(this, fn);
                        }
                    });
                }
            } catch(_) {}
            dlog('[WS] proxy installed');
        }
    } catch(e) {
        dlog('[WS] install FAILED: ' + (e && e.message ? e.message : String(e)));
    }

    // -----------------------------------------------------------------------
    // 3. fetch / XHR proxy (read-only) — only logs Messenger messaging URLs.
    // -----------------------------------------------------------------------
    try {
        var URL_RE = /(\/messaging\/|\/api\/graphql\/|\/sync\/|\/lsr\b|graphql)/i;
        var origFetch = window.fetch;
        if (typeof origFetch === 'function') {
            window.fetch = function(input, init) {
                var url = '';
                try {
                    url = (typeof input === 'string') ? input : (input && input.url) || '';
                } catch(_) {}
                var method = (init && init.method) || 'GET';
                var matched = URL_RE.test(url);
                var p = origFetch.apply(this, arguments);
                if (matched && p && typeof p.then === 'function') {
                    p.then(function(resp) {
                        try {
                            dlog('[Fetch] ' + method + ' ' + (url.length > 80 ? url.slice(0, 80) + '..' : url)
                                + ' -> ' + (resp ? resp.status : '?'));
                        } catch(_) {}
                    }, function(err) {
                        try {
                            dlog('[Fetch] ' + method + ' ' + url.slice(0, 80) + ' ERR ' + (err && err.message ? err.message : String(err)));
                        } catch(_) {}
                    });
                }
                return p;
            };
        }
        var XHRProto = window.XMLHttpRequest && window.XMLHttpRequest.prototype;
        if (XHRProto) {
            var origOpen = XHRProto.open;
            XHRProto.open = function(method, url) {
                try {
                    if (URL_RE.test(url || '')) {
                        this.__mxDiagUrl = url;
                        this.__mxDiagMethod = method;
                        var self = this;
                        this.addEventListener('loadend', function() {
                            try {
                                dlog('[XHR] ' + self.__mxDiagMethod + ' '
                                    + String(self.__mxDiagUrl).slice(0, 80) + ' -> ' + self.status);
                            } catch(_) {}
                        });
                    }
                } catch(_) {}
                return origOpen.apply(this, arguments);
            };
        }
        dlog('[HTTP] proxies installed');
    } catch(e) {
        dlog('[HTTP] install FAILED: ' + (e && e.message ? e.message : String(e)));
    }

    dlog('[Init] diagnostic telemetry v=' + APP_VERSION + ' ready');
})();
"#
);

/// Hooks `HTMLAudioElement.prototype.play` to log every audio playback event
/// initiated by the Messenger page (notification sounds, ringtones, call alerts,
/// etc.) to the Rust log file via `js_log`.
///
/// Log prefix: `[AudioJS]`.  Correlate with `[AudioRust]` Rust-side log lines
/// (emitted in `show_via_notify_send` / `show_via_tauri_plugin` / macOS
/// `show_via_user_notifications` when `silent=false`) to detect whether OS sound
/// is played twice: once by Messenger's own `<audio>` element and once by the
/// native notification sound hint we request.
const AUDIO_HOOK_SCRIPT: &str = concat!(
    r#"
(function() {
    var APP_VERSION = ""#,
    env!("CARGO_PKG_VERSION"),
    r#"";

    function alog(msg) {
        try {
            window.__TAURI__.core.invoke('js_log', { message: '[AudioJS] ' + msg });
        } catch(_) {}
    }

    // Intercept every HTMLAudioElement.play() call made by Messenger.
    // Reports: src (truncated), readyState, paused, muted so we can tell
    // whether it is a real notification sound or a prefetch / silent preload.
    try {
        var _origPlay = HTMLAudioElement.prototype.play;
        HTMLAudioElement.prototype.play = function() {
            try {
                var src = this.src || this.currentSrc || '(no-src)';
                if (src.length > 120) { src = src.slice(0, 117) + '...'; }
                alog('[play] src=' + JSON.stringify(src)
                    + ' readyState=' + this.readyState
                    + ' paused=' + this.paused
                    + ' muted=' + this.muted
                    + ' volume=' + (Math.round(this.volume * 100) / 100)
                    + ' v=' + APP_VERSION);
            } catch(_) {}
            return _origPlay.apply(this, arguments);
        };
        alog('[init] HTMLAudioElement.prototype.play hooked v=' + APP_VERSION);
    } catch(e) {
        alog('[init] hook FAILED: ' + (e && e.message ? e.message : String(e)));
    }
})();
"#
);

/// JavaScript snippet that captures failed media-resource loads (`<img>`,
/// `<video>`, `<audio>`, `<source>`) at the `window` capture phase and forwards
/// each unique failed URL to the Rust log file via `js_log`.
///
/// This is passive diagnostic telemetry: it fires only when a media element
/// fails to load (network error, blocked CDN domain, etc.) and deduplicates
/// within the same page-load so the log is not flooded on retry loops.
///
/// Use-case: Linux users with DNS-level ad-blockers on their router may have
/// certain CDN domains blocked.  GIFs and videos served from different CDN
/// endpoints for DMs vs group chats may be affected differently.  The logged
/// URL tells us (and the user) exactly which domain is being blocked without
/// requiring them to open browser DevTools.
///
/// Log prefix: `[MediaErrorJS]`.
const MEDIA_ERROR_LOGGER_SCRIPT: &str = concat!(
    r#"
(function() {
    var APP_VERSION = ""#,
    env!("CARGO_PKG_VERSION"),
    r#"";

    function mlog(msg) {
        try {
            window.__TAURI__.core.invoke('js_log', { message: '[MediaErrorJS] ' + msg });
        } catch(_) {}
    }

    // Dedup set: only the first failure per URL per page-load is logged so
    // that retry loops don't flood the log file.  Keyed on the full raw URL
    // so that long URLs differing only in their query string are not collapsed.
    var _seen = new Set();

    try {
        window.addEventListener('error', function(e) {
            try {
                var t = e.target;
                if (!(t instanceof HTMLImageElement  ||
                      t instanceof HTMLVideoElement  ||
                      t instanceof HTMLAudioElement  ||
                      t instanceof HTMLSourceElement)) {
                    return;
                }
                // t.src / t.currentSrc covers <img>, <video>, <audio>, and
                // <source> inside <video>/<audio>.  For <source> inside
                // <picture>, the browser uses srcset rather than src, so fall
                // back to the first URL token in srcset when src is empty.
                var _srcset = t.srcset && t.srcset.split(',')[0].trim().split(/\s+/)[0];
                var src = t.src || t.currentSrc || _srcset || '(no-src)';
                // Dedup on the full raw URL before sanitising — prevents
                // collisions between long URLs that share the same first N chars.
                if (_seen.has(src)) { return; }
                _seen.add(src);
                // Log only scheme+host+path; strip query string and fragment so
                // that CDN signatures/tokens are not captured in log files that
                // users share in bug reports.
                 var display = src;
                 try { var u = new URL(src); display = u.origin + u.pathname; } catch(_) {
                     // src is a relative URL or opaque string — strip query/fragment
                     // with a regex so tokens can't leak even when URL parsing fails.
                     display = src.replace(/[?#].*$/, '');
                 }
                if (display.length > 200) { display = display.slice(0, 197) + '...'; }
                mlog('failed <' + t.tagName.toLowerCase() + '> src=' +
                     JSON.stringify(display) + ' v=' + APP_VERSION);
            } catch(_) {}
        }, true /* capture phase — catches errors from same-origin frames; cross-origin iframes excluded by browser SOP */);
        mlog('listener registered v=' + APP_VERSION);
    } catch(e) {
        mlog('register FAILED: ' + (e && e.message ? e.message : String(e)));
    }
})();
"#
);

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

/// JavaScript snippet injected via `eval()` during logout.  Performs a
/// best-effort client-side clear of JS-accessible cookies (including
/// subdomain and path variants), localStorage, and sessionStorage, then
/// navigates to `https://www.messenger.com`.
///
/// **Limitations** — This cannot remove `HttpOnly` cookies (commonly used
/// for auth tokens) and only affects cookies for the *current* origin.
/// A full session drop may require a server-side logout flow in addition
/// to this client-side clear.
///
/// * Cookie names are trimmed after splitting on `;`.
/// * Empty cookie names are skipped (guards against `document.cookie === ''`).
/// * Cookie expiry uses RFC1123 GMT format for cross-WebView compatibility.
/// * The `while` loop stops before the TLD (`hostParts.length > 1`).
/// * Path clearing includes the current pathname and all parent prefixes
///   (e.g. `/messages/t/123`, `/messages/t`, `/messages`, `/`).
/// * Any storage error is caught so the redirect always runs in `finally`.
const LOGOUT_CLEAR_SCRIPT: &str = r#"
(function() {
    try {
        var pathSegments = window.location.pathname.split('/').filter(Boolean);
        document.cookie.split(';').forEach(function(c) {
            var eq = c.indexOf('=');
            var name = (eq > -1 ? c.substring(0, eq) : c).trim();
            if (!name) return;
            var hostParts = window.location.hostname.split('.');
            while (hostParts.length > 1) {
                var domain = hostParts.join('.');
                // Clear for root path and all parent path prefixes
                // (both with and without trailing slash, since they are
                // distinct cookie identities).
                for (var i = pathSegments.length; i >= 0; i--) {
                    var p = '/' + pathSegments.slice(0, i).join('/');
                    document.cookie = name + '=;expires=Thu, 01 Jan 1970 00:00:00 GMT;path=' + p + ';domain=' + domain;
                    document.cookie = name + '=;expires=Thu, 01 Jan 1970 00:00:00 GMT;path=' + p + ';domain=.' + domain;
                    if (i > 0) {
                        var pSlash = p + '/';
                        document.cookie = name + '=;expires=Thu, 01 Jan 1970 00:00:00 GMT;path=' + pSlash + ';domain=' + domain;
                        document.cookie = name + '=;expires=Thu, 01 Jan 1970 00:00:00 GMT;path=' + pSlash + ';domain=.' + domain;
                    }
                }
                hostParts.shift();
            }
            // Current origin (no domain attribute).
            for (var i = pathSegments.length; i >= 0; i--) {
                var p = '/' + pathSegments.slice(0, i).join('/');
                document.cookie = name + '=;expires=Thu, 01 Jan 1970 00:00:00 GMT;path=' + p;
                if (i > 0) {
                    document.cookie = name + '=;expires=Thu, 01 Jan 1970 00:00:00 GMT;path=' + p + '/';
                }
            }
        });
    } catch (e) {
        console.error('[MessengerX] Logout cookie clear failed:', e);
    } finally {
        try { localStorage.clear(); } catch (e) {
            console.error('[MessengerX] Logout localStorage clear failed:', e);
        }
        try { sessionStorage.clear(); } catch (e) {
            console.error('[MessengerX] Logout sessionStorage clear failed:', e);
        }
        try { sessionStorage.setItem('__mx_appearance', 'system'); } catch (e) {
            console.error('[MessengerX] Logout appearance reset failed:', e);
        }
        window.location.href = 'https://www.messenger.com';
    }
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

    // Fix 3 (Linux IPC): wrap the top-level window.__TAURI__ access in try/catch.
    //
    // On VMware / AppImage / EGL-degraded WebKitGTK (Linux), window.__TAURI__ can
    // be undefined even though initialization_scripts normally inject the Tauri IPC
    // binding unconditionally.  Without this guard the bare property access at the
    // top level of the IIFE throws a TypeError that silently aborts the script —
    // preventing the "visibility override installed" log line and potentially leaving
    // Messenger with a permanently-wrong visibilityState after a re-navigation.
    //
    // The .catch() already handles async failures; this try/catch handles the
    // synchronous case where window.__TAURI__ itself (or .core) is undefined.
    try {
        window.__TAURI__.core.invoke('get_window_focused')
            .then(function(focused) {
                jlog('initial get_window_focused=' + String(focused));
                setVisible(focused, 'ipc-resync');
            })
            .catch(function(e) {
                jlog('get_window_focused IPC rejected: ' + String(e));
            });
    } catch(e) {
        // IPC not yet available (window.__TAURI__ undefined or .core missing).
        // The flag stays at its conservative default (_hidden=false → visible),
        // which is the safer fallback: Messenger will attempt notifications rather
        // than silently suppressing them.  The Rust-side WindowEvent::Focused handler
        // and the is_minimized() poll thread will update the flag via
        // window.__messengerx_set_visible() once IPC becomes available.
        jlog('get_window_focused sync error (IPC unavailable at init): ' + String(e));
    }

    jlog('visibility override installed');
})();"#;

/// Linux: suppress the GNOME MPRIS "Now Playing" media-player widget.
///
/// WebKitGTK implements the W3C Media Session API and automatically
/// publishes any `navigator.mediaSession.metadata` (title, artist, …) to
/// D-Bus via its MPRIS interface.  GNOME Shell subscribes to this interface
/// and displays a media-player control widget in the notification shade that
/// shows Messenger conversation titles / contact names.
///
/// This script replaces `navigator.mediaSession` with a no-op proxy at
/// document-start, before Messenger's scripts run.  The replacement is
/// API-compatible (all properties and methods exist) so Messenger does not
/// throw errors — it simply cannot propagate metadata to D-Bus.
///
/// Override strategy (belt-and-suspenders):
///   1. Define a frozen no-op object as the replacement.
///   2. Try `Object.defineProperty` on `Navigator.prototype` first —
///      in WebKitGTK the attribute is defined on the prototype, so this
///      intercepts all `navigator.mediaSession` accesses immediately.
///      Using `configurable: false` prevents page code from re-overriding.
///   3. Fall back to the `navigator` instance if the prototype override
///      throws (e.g. already non-configurable on the prototype).
///   4. Log a console.warn if both paths fail so future debugging is easier.
///
/// Audio and video calls are unaffected: WebRTC / Web Audio API handle the
/// actual media routing and do not go through MediaSession.
#[cfg(target_os = "linux")]
const MEDIA_SESSION_SUPPRESS_SCRIPT: &str = r#"(function() {
    'use strict';
    var _noop = Object.freeze({
        metadata: null,
        playbackState: 'none',
        setActionHandler: function() {},
        setPositionState: function() {},
        setCameraActive: function() {},
        setMicrophoneActive: function() {},
    });
    var _descriptor = {
        get: function() { return _noop; },
        set: function() { /* drop assignment — prevent re-override by page code */ },
        configurable: false,
        enumerable: false,
    };
    var _overridden = false;
    // Strategy 1: override on the prototype (preferred — covers all instances).
    try {
        Object.defineProperty(Navigator.prototype, 'mediaSession', _descriptor);
        _overridden = true;
    } catch (_protoErr) {
        // Prototype override failed; fall through to instance override.
    }
    // Strategy 2: override on the navigator instance as a fallback.
    if (!_overridden) {
        try {
            Object.defineProperty(navigator, 'mediaSession', _descriptor);
            _overridden = true;
        } catch (_instanceErr) {
            // Both overrides failed.
        }
    }
    if (!_overridden) {
        // eslint-disable-next-line no-console
        console.warn('[MX] mediaSession override failed — GNOME MPRIS widget may still appear');
    }
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

/// Linux/AppImage startup guard:
/// prefer local/in-process backends that avoid common host integration issues,
/// while still allowing bundled GIO modules (such as glib-networking's TLS
/// backend) to load normally inside the AppImage.
#[cfg(target_os = "linux")]
const LINUX_APPIMAGE_ENV_OVERRIDES: &[(&str, &str)] = &[
    // Skip GVFS (host module) and use plain local file backend.
    ("GIO_USE_VFS", "local"),
    // Avoid loading host dconf backend when bundled GLib is older/newer.
    ("GSETTINGS_BACKEND", "memory"),
    // Force GTK3's built-in file chooser instead of the XDG Desktop Portal.
    // Without this, WebKitGTK on Wayland inside an AppImage uses the portal
    // (`org.freedesktop.portal.FileChooser`), which rejects requests from
    // AppImages that have no portal access → `<input type="file">` silently
    // fails and no file picker appears (Issue #27 Bug B).
    ("GTK_USE_PORTAL", "0"),
];

#[cfg(target_os = "linux")]
fn configure_linux_runtime_env() {
    let is_appimage =
        std::env::var_os("APPIMAGE").is_some() || std::env::var_os("APPDIR").is_some();
    if !is_appimage {
        return;
    }

    for (key, value) in LINUX_APPIMAGE_ENV_OVERRIDES {
        // Respect an explicit user/system override (e.g. launch script) while
        // still applying safe defaults for untouched environments.
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, value);
        }
    }
}

// ---------------------------------------------------------------------------
// Public helper: macOS notification dispatch from inside .app bundle
// (used by the CLI `--notify` path in main.rs)
// ---------------------------------------------------------------------------

/// Dispatch a native notification using `UNUserNotificationCenter`.
///
/// This is a **macOS-only** public entry-point intended for the CLI helper mode
/// (`messengerx --notify <title> <body> [silent]`) where the binary runs inside
/// the debug `.app` bundle and therefore has a valid bundle context.
///
/// It calls `services::notification::dispatch_bundle_notification` directly —
/// **without** going through the osascript / subprocess delegation paths — so
/// the helper process does not recursively spawn itself.
///
/// # Errors
/// Returns `Err` if `UNUserNotificationCenter` initialisation fails or the
/// enqueue call returns an error.
#[cfg(target_os = "macos")]
pub fn dispatch_notification_from_bundle(
    title: &str,
    body: &str,
    silent: bool,
) -> Result<(), String> {
    services::notification::dispatch_bundle_notification(title, body, silent)
}

// ---------------------------------------------------------------------------
// Application entry point
// ---------------------------------------------------------------------------

/// Phase A diagnostic — log the runtime platform environment once at startup.
///
/// Per-platform fields:
///  - **Linux**: `XDG_SESSION_TYPE`, `WAYLAND_DISPLAY`, `XDG_CURRENT_DESKTOP`,
///    `DESKTOP_SESSION`, `DBUS_SESSION_BUS_ADDRESS`, presence of `notify-send`
///    on `$PATH`. Used to validate H2 (Wayland focus gating) and the
///    notify-send transport path.
///  - **Windows**: OS version via `cmd /c ver` and WebView2 runtime version
///    via the EdgeUpdate registry key. Used for H3/H4.
///  - **macOS**: kernel version via `uname -r`.
fn log_platform_environment() {
    let pkg_version = env!("CARGO_PKG_VERSION");
    log::info!("[MessengerX][Env] starting v{pkg_version}");

    #[cfg(target_os = "linux")]
    {
        let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "<unset>".into());
        let wayland_display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "<unset>".into());
        let current_desktop =
            std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "<unset>".into());
        let desktop_session = std::env::var("DESKTOP_SESSION").unwrap_or_else(|_| "<unset>".into());
        let dbus_addr = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .map(|v| {
                let trimmed: String = v.chars().take(80).collect();
                if v.len() > 80 {
                    format!("{trimmed}…(truncated)")
                } else {
                    trimmed
                }
            })
            .unwrap_or_else(|_| "<unset>".into());
        let appimage = std::env::var("APPIMAGE").ok();
        let gtk_use_portal = std::env::var("GTK_USE_PORTAL").unwrap_or_else(|_| "<unset>".into());
        log::info!(
            "[MessengerX][Env][Linux] session_type={session_type:?} wayland_display={wayland_display:?} \
             current_desktop={current_desktop:?} desktop_session={desktop_session:?} \
             dbus_addr={dbus_addr:?} appimage={appimage:?} GTK_USE_PORTAL={gtk_use_portal:?}"
        );

        let notify_send_check = std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v notify-send 2>/dev/null && notify-send --version 2>/dev/null | head -n1")
            .output();
        match notify_send_check {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                log::info!(
                    "[MessengerX][Env][Linux] notify-send probe: status={:?} stdout={stdout:?} stderr={stderr:?}",
                    out.status.code()
                );
            }
            Err(e) => {
                log::warn!("[MessengerX][Env][Linux] notify-send probe spawn failed: {e}");
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        let os_info = std::process::Command::new("cmd")
            .args(["/c", "ver"])
            .output();
        match os_info {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                log::info!("[MessengerX][Env][Windows] os_version={stdout:?}");
            }
            Err(e) => {
                log::warn!("[MessengerX][Env][Windows] `cmd /c ver` failed: {e}");
            }
        }
        let webview2 = std::process::Command::new("reg")
            .args([
                "query",
                "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\EdgeUpdate\\Clients\\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}",
                "/v",
                "pv",
            ])
            .output();
        if let Ok(out) = webview2 {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            log::info!("[MessengerX][Env][Windows] webview2_runtime_query={stdout:?}");
        }
        // Proxy args are logged by resolve_webview2_proxy_args() at builder
        // construction time, so no separate diagnostic is needed here.
    }

    #[cfg(target_os = "macos")]
    {
        let uname = std::process::Command::new("uname").arg("-r").output();
        if let Ok(out) = uname {
            let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
            log::info!("[MessengerX][Env][macOS] kernel={stdout:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// Windows proxy helpers
// ---------------------------------------------------------------------------

/// Reads the Windows Internet Options proxy settings from the registry and
/// returns the appropriate Chromium command-line flag string to pass to
/// [`WebviewWindowBuilder::additional_browser_args`].
///
/// Decision table (evaluated top-to-bottom, first match wins):
///
/// | AutoConfigURL | ProxyEnable | ProxyServer | AutoDetect | Chromium flag              |
/// |---------------|-------------|-------------|------------|----------------------------|
/// | non-empty     | any         | any         | any        | `--proxy-pac-url=<url>`    |
/// | empty         | 1           | non-empty   | any        | `--proxy-server=<host:port>` |
/// | empty         | 0 / absent  | absent      | 1          | `--no-proxy-server`        |
/// | empty         | 0 / absent  | absent      | 0          | `--no-proxy-server`        |
///
/// `--disable-background-networking` is always appended.
///
/// ## Why `--no-proxy-server` for both AutoDetect=0 and AutoDetect=1
///
/// Even when `AutoDetect=0` (IE "Automatically detect settings" unchecked),
/// Chromium's network process still calls `WinHttpGetProxyForUrl()` with the
/// `WINHTTP_AUTOPROXY_AUTO_DETECT` flag as part of its system-proxy resolver.
/// On networks without a WPAD server this times out after **~27 seconds**
/// before falling back to DIRECT.  Because the user has no proxy configured at
/// all we can safely tell Chromium to go DIRECT immediately via
/// `--no-proxy-server`, bypassing all WinHTTP proxy discovery.
#[cfg(target_os = "windows")]
fn resolve_webview2_proxy_args() -> String {
    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    // Single `reg query` call to read all values from the key at once.
    let reg_output = std::process::Command::new("reg")
        .args(["query", KEY])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();

    let mut auto_detect: u32 = 0;
    let mut proxy_enable: u32 = 0;
    let mut proxy_server = String::new();
    let mut auto_config_url = String::new();

    for line in reg_output.lines() {
        // Each data line is indented and looks like:
        //   "    ValueName    REG_DWORD    0x1"
        //   "    ValueName    REG_SZ    127.0.0.1:8080"
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("AutoDetect") {
            let rest = rest.trim();
            if let Some(hex) = rest
                .strip_prefix("REG_DWORD")
                .map(|s| s.trim().trim_start_matches("0x"))
            {
                auto_detect = u32::from_str_radix(hex, 16).unwrap_or(0);
            }
        } else if let Some(rest) = trimmed.strip_prefix("ProxyEnable") {
            let rest = rest.trim();
            if let Some(hex) = rest
                .strip_prefix("REG_DWORD")
                .map(|s| s.trim().trim_start_matches("0x"))
            {
                proxy_enable = u32::from_str_radix(hex, 16).unwrap_or(0);
            }
        } else if let Some(rest) = trimmed.strip_prefix("ProxyServer") {
            let rest = rest.trim();
            if let Some(val) = rest.strip_prefix("REG_SZ") {
                proxy_server = val.trim().to_string();
            }
        } else if let Some(rest) = trimmed.strip_prefix("AutoConfigURL") {
            let rest = rest.trim();
            if let Some(val) = rest.strip_prefix("REG_SZ") {
                auto_config_url = val.trim().to_string();
            }
        }
    }

    log::info!(
        "[MessengerX][Proxy] auto_detect={auto_detect} proxy_enable={proxy_enable} \
         proxy_server={proxy_server:?} auto_config_url={auto_config_url:?}"
    );

    let proxy_flag = if !auto_config_url.is_empty() {
        // Explicit PAC URL — hand it straight to Chromium; no WPAD discovery needed.
        format!("--proxy-pac-url={auto_config_url}")
    } else if proxy_enable == 1 && !proxy_server.is_empty() {
        // Manual proxy server — bypass all auto-detection.
        format!("--proxy-server={proxy_server}")
    } else if auto_detect == 1 {
        // WPAD auto-detect is on but there is no actual proxy server configured.
        // Skip discovery to avoid the ~27 s WPAD timeout on networks without a
        // WPAD server.  Direct connection is the correct behaviour here.
        "--no-proxy-server".to_string()
    } else {
        // No proxy configured at all — but Chromium's WinHTTP proxy resolver
        // still probes for WPAD via WinHttpGetProxyForUrl() even when
        // AutoDetect=0, causing a ~27 s timeout on networks without a WPAD
        // server.  Bypass all discovery and go DIRECT immediately.
        "--no-proxy-server".to_string()
    };

    log::info!("[MessengerX][Proxy] webview2_proxy_flag={proxy_flag:?}");

    if proxy_flag.is_empty() {
        "--disable-background-networking".to_string()
    } else {
        format!("{proxy_flag} --disable-background-networking")
    }
}

/// How long a previously consumed sender hint stays available for the
/// typing-indicator-rearm fallback after the original count-rise dispatch
/// already consumed the live hint.  Long enough to span a typical typing
/// pause + reply burst, short enough to avoid attributing a later message
/// from a different conversation to the previous sender.
const SENDER_HINT_RETAIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Cache of sender names extracted from the JS DOM scraper and pushed to
/// Rust via the `__MX_SENDER_V1__` document.title side-channel.
///
/// See the field-level docs at the [`SenderHintCache::default`]-constructed
/// instance in `setup_app` for the lifecycle.
#[derive(Default, Debug)]
struct SenderHintCache {
    /// Fresh hint pending consumption by the next count-rise notification.
    /// Cleared (`take`) by the worker as soon as it matches `count`.
    live: Option<(u32, String, std::time::Instant)>,
    /// Last sender that was successfully consumed from `live`, kept for
    /// [`SENDER_HINT_RETAIN`] so the typing-indicator-rearm path can
    /// re-attribute the rearm fire to the same author when no fresh
    /// `live` hint is available.  `Instant` is the time of consumption,
    /// not the time the hint was originally produced.
    retained: Option<(String, std::time::Instant)>,
}

/// Opens the Messenger X log file on Linux.
///
/// Every Linux desktop ships with a text editor registered for `text/plain`,
/// so `xdg-open` on a `.txt` file is the primary — and usually sufficient —
/// approach.  `.log` files often have no MIME handler registered, so a
/// `/tmp/messengerx-log.txt` symlink is used instead of the real path.
///
/// All spawns strip `LD_LIBRARY_PATH`/`LD_PRELOAD` — the AppImage runtime
/// injects these with stale paths, causing child processes to fail silently
/// (same root cause as the `notify-send` fix in v1.4.1).
///
/// Strategy — first success wins:
///   1. **`xdg-open` on `/tmp/messengerx-log.txt` symlink** — opens in the
///      user's default text editor (gedit, kate, mousepad, xed, VS Code, …)
///      regardless of which DE or editor they have installed.
///   2. **Terminal + `less`** — fallback if `xdg-open` is absent or fails.
///      `x-terminal-emulator` (Debian/Ubuntu/Mint update-alternatives standard)
///      then `xterm` (X11 base, near-universal), then others.
///   3. **`xdg-open` on the log directory** — file-manager last resort.
#[cfg(target_os = "linux")]
fn open_log_on_linux(log_dir: &std::path::Path, log_file: &std::path::Path) {
    let file_exists = log_file.exists();
    let mut opened = false;

    if file_exists {
        // -------------------------------------------------------------------
        // 1.  xdg-open via .txt symlink — opens in the default text editor.
        //
        // Every Linux desktop distro registers a text editor for text/plain.
        // .log files often have no handler, so we symlink to .txt so that
        // xdg-open picks the correct MIME type and launches the editor the
        // user already has (gedit, kate, mousepad, xed, VS Code, etc.).
        // AppImage env vars are stripped so xdg-open loads system libraries.
        // -------------------------------------------------------------------
        let tmp_link = std::env::temp_dir().join("messengerx-log.txt");
        let _ = std::fs::remove_file(&tmp_link);
        let target = if std::os::unix::fs::symlink(log_file, &tmp_link).is_ok() {
            log::info!("[MessengerX] view_logs: created .txt symlink");
            tmp_link.as_path()
        } else {
            log::warn!("[MessengerX] view_logs: symlink failed, using .log directly");
            log_file
        };
        match std::process::Command::new("xdg-open")
            .arg(target)
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("LD_PRELOAD")
            .spawn()
        {
            Ok(_) => {
                log::info!("[MessengerX] view_logs: xdg-open on {}", target.display());
                opened = true;
            }
            Err(e) => {
                log::warn!("[MessengerX] view_logs: xdg-open failed: {e}");
                let _ = std::fs::remove_file(&tmp_link);
            }
        }

        // -------------------------------------------------------------------
        // 2.  Terminal + less — fallback when xdg-open is absent or fails.
        //
        // x-terminal-emulator is the Debian/Ubuntu/Mint update-alternatives
        // entry pointing to the user's configured default terminal.
        // xterm is in the X11 base and is available on virtually every Linux
        // desktop installation.  The log file path is passed as a direct arg
        // (Command::arg handles spaces; no sh -c quoting needed).
        // -------------------------------------------------------------------
        if !opened {
            let terminals: &[(&str, &[&str])] = &[
                ("x-terminal-emulator", &["-e", "less"]),
                ("xterm",               &["-e", "less"]),
                ("gnome-terminal",      &["--", "less"]),
                ("xfce4-terminal",      &["-e", "less"]),
                ("konsole",             &["-e", "less"]),
                ("mate-terminal",       &["-e", "less"]),
                ("lxterminal",          &["-e", "less"]),
                ("tilix",               &["-e", "less"]),
            ];
            for (term, pre) in terminals {
                match std::process::Command::new(term)
                    .args(*pre)
                    .arg(log_file)
                    .env_remove("LD_LIBRARY_PATH")
                    .env_remove("LD_PRELOAD")
                    .spawn()
                {
                    Ok(_) => {
                        log::info!("[MessengerX] view_logs: opened in terminal {term}");
                        opened = true;
                        break;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => log::warn!("[MessengerX] view_logs: {term} error: {e}"),
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // 3.  Open the log directory — file manager, absolute last resort.
    // -----------------------------------------------------------------------
    if !opened {
        let _ = std::fs::create_dir_all(log_dir);
        match std::process::Command::new("xdg-open")
            .arg(log_dir)
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("LD_PRELOAD")
            .spawn()
        {
            Ok(_) => log::info!("[MessengerX] view_logs: opened log directory in file manager"),
            Err(e) => log::warn!("[MessengerX] view_logs: xdg-open dir failed: {e}"),
        }
    }
}

/// Run the Tauri application.
///
/// Registers all plugins, builds the main webview window with JS injection
/// scripts and navigation policy, sets up a system-tray icon, and starts the
/// periodic snapshot timer.
pub fn run() {
    #[cfg(target_os = "linux")]
    configure_linux_runtime_env();

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("messengerx".to_string()),
                    }),
                ])
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
            commands::pick_save_path,
            commands::write_file_bytes,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Phase A diagnostic: log RunEvent::Ready timing for first-paint
            // correlation (H3 Win11 white screen).
            if let tauri::RunEvent::Ready = event {
                log::info!("[MessengerX][Boot] RunEvent::Ready fired");
            }

            // Windows 11 (especially on some ARM devices) can ignore a focus
            // request if it happens too early during setup. Apply the
            // startup foreground workaround after the runtime reports Ready,
            // but only for a window that setup has already decided should be
            // visible.
            #[cfg(target_os = "windows")]
            if let tauri::RunEvent::Ready = event {
                if let Some(window) = app_handle.get_webview_window("main") {
                    if matches!(window.is_visible(), Ok(true)) {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                }
            }

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
    // Phase A diagnostic: mark setup_app entry so we can correlate first-paint
    // delay with Rust-side cost (H3 Win11 white screen).
    let setup_started = std::time::Instant::now();
    log::info!("[MessengerX][Boot] setup_app start");
    // Phase A diagnostic: log_platform_environment must run AFTER the log plugin
    // is initialized (inside setup_app), not from run() where the logger is not yet
    // active. This was the bug that caused all [Env] lines to be silently dropped.
    log_platform_environment();

    if let Err(e) = services::notification::initialize() {
        log::warn!("[MessengerX] Failed to initialize native notifications: {e}");
    }

    // ------------------------------------------------------------------
    // 1. Load persisted settings so we can bake the zoom level into the
    //    initialization script before the window is created.
    // ------------------------------------------------------------------
    // Phase M trace: measure load_settings cost (sync std::fs, can be slow on
    // first launch on Win11 if appdata dir needs to be created on a slow disk).
    let load_settings_start = std::time::Instant::now();
    let settings = services::auth::load_settings(app.handle()).unwrap_or_default();
    log::info!(
        "[MessengerX][Boot][Trace] load_settings elapsed={}ms last_url_present={} \
         start_minimized={} appearance={:?} zoom={}",
        load_settings_start.elapsed().as_millis(),
        settings.last_messenger_url.is_some(),
        settings.start_minimized,
        settings.appearance,
        settings.zoom_level,
    );
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
    let zoom_init_script =
        "(function() { window.__messenger_setZoom = function() {}; })();".to_string();

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
    // Phase K: crash detection state.
    //
    // If `title` becomes "" after a successful load, this almost certainly
    // means the WebKitWebProcess has crashed.  On Linux/VMware this is
    // triggered by `GStreamer element autoaudiosink not found` → NULL
    // pointer dereference inside WebKit's media pipeline.
    //
    // `had_good_title` is set to `true` the first time we see a title that
    // looks like a real Messenger page ("Messenger", "(N) Messenger", …).
    // When we then observe `title=""` we log a WARN and schedule an auto-
    // reload (up to MAX_CRASH_RELOADS times per session).
    // ------------------------------------------------------------------
    let had_good_title = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let crash_reload_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    // Set to `true` by `on_navigation` the first time a navigation to a
    // `www.messenger.com` URL is allowed through the policy callback.
    // (`on_navigation` is a policy hook that may also return `false` to
    // cancel; it is NOT a navigation-committed signal.)
    // `had_good_title` must NOT be set from the `loading.html` splash page
    // title ("Messenger X") — that would cause a false-positive CrashDetect
    // when the page title briefly clears during the initial SPA navigation on
    // macOS WKWebView.
    let messenger_com_navigated =
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The document title briefly becomes `""` during certain events:
    //   • macOS WKWebView: on EVERY SPA navigation (not only after a real
    //     WebKit crash) — thread-to-thread navigation clears the title.
    //   • Linux WebKitGTK: on `window.location.reload()` (e.g. appearance
    //     toggle) AND on real WebKitWebProcess crashes.
    // To avoid false-positive CrashDetect fires on both platforms we track
    // whether the page has finished loading via `page_load_stable`:
    //   • Reset to `false` in `on_navigation` for www.messenger.com
    //     (navigation has started, page is not yet stable).
    //   • Set to `true` in `on_page_load::Finished` for www.messenger.com
    //     (DOMContentLoaded-equivalent; page is now stable).
    // `had_good_title` may only be set when `page_load_stable` is `true`,
    // and CrashDetect may only fire when it is `true` (all platforms).
    // Real crashes that happen AFTER the page has fully loaded still fire
    // correctly because `page_load_stable` is `true` at that point.
    let page_load_stable =
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // ------------------------------------------------------------------
    // Sender-hint cache.
    //
    // `live`     – Fresh hint emitted by the JS side-channel
    //              (`__MX_SENDER_V1__?count=…&sender=…` document.title sentinel).
    //              Stored by the title-change handler, consumed by the
    //              `count > 0` worker when a notification is being dispatched.
    //              Taken via `Option::take` so the same hint cannot satisfy
    //              two unrelated count-rises.
    // `retained` – Last successfully consumed sender, kept for
    //              `SENDER_HINT_RETAIN` (~10 s) so the typing-indicator-rearm
    //              path (count drops to 0 from typing, then returns) can
    //              re-use the same author when no fresh `live` hint has been
    //              re-emitted by JS for the same conversation.  Without this
    //              the rearm fire falls back to the generic "Nová zpráva"
    //              title even when the originating sender is still known.
    // ------------------------------------------------------------------
    let pending_sender_hint =
        std::sync::Arc::new(std::sync::Mutex::new(SenderHintCache::default()));
    // Set to `true` after a confirmed WebKitWebProcess crash (title became ""
    // after a good load).  When active on Linux, the `on_navigation` handler
    // blocks the fbsbx.com maw_proxy_page that is known to trigger the
    // GStreamer NULL-pointer deref, giving the reloaded page a chance to
    // finish loading without immediately crashing again.
    let post_crash_proxy_block = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    // ------------------------------------------------------------------
    // 3. Determine initial URL based on connectivity.
    //    If offline, load a local fallback page instead of messenger.com.
    // ------------------------------------------------------------------
    // Phase M trace: is_likely_online does a TcpStream::connect_timeout(2s)
    // — if DNS or routing is degraded this can dominate boot time on Win11.
    let online_check_start = std::time::Instant::now();
    let is_online = services::network::is_likely_online();
    log::info!(
        "[MessengerX][Boot][Trace] is_likely_online elapsed={}ms result={is_online}",
        online_check_start.elapsed().as_millis()
    );
    // Phase M trace: surface the URL-resolution decision so the persisted-URL
    // safe-check verdict (Commit-A diagnostic ground truth) is visible without
    // re-running the binary in dev.  Empty `last_url_raw` means first-launch.
    let last_url_raw = settings.last_messenger_url.as_deref().unwrap_or("");
    let last_url_safe = !last_url_raw.is_empty() && is_safe_messenger_startup_url(last_url_raw);
    let startup_url = settings
        .last_messenger_url
        .as_deref()
        .filter(|u| is_safe_messenger_startup_url(u))
        .unwrap_or("https://www.messenger.com/");
    log::info!(
        "[MessengerX][Boot][Trace] startup_url decision: last_raw={last_url_raw:?} \
         safe={last_url_safe} chosen={startup_url:?}"
    );
    // When online, start with the local loading.html so the window shows a
    // spinner immediately (instead of a blank white screen) while WebView2
    // initialises its network stack.  After build() we navigate to the real
    // messenger.com URL in a background thread.  WebView2 keeps the previous
    // page visible until ContentLoading fires for the new navigation, so the
    // spinner stays on-screen for the entire ~27 s network-init gap.
    // When offline, go straight to index.html (the offline fallback) as before.
    let (webview_url, navigate_after_build) = if is_online {
        log::info!("[MessengerX][Boot] initial_url={startup_url} (via loading.html splash)");
        let target_url = url::Url::parse(startup_url)?;
        (WebviewUrl::App("loading.html".into()), Some(target_url))
    } else {
        log::info!("[MessengerX] Offline at startup — loading local fallback page");
        (WebviewUrl::App("index.html".into()), None)
    };

    // Build the scrollbar-fix script with platform-specific behaviour.
    // On macOS the native overlay scrollbar is hidden (scrollbar-width:none)
    // because the transparent scrollbar track was revealing the chat-theme
    // gradient behind it. On other platforms we use custom webkit-scrollbar.
    let scrollbar_fix_script = build_scrollbar_fix_script(cfg!(target_os = "macos"));

    // Build the appearance override script (overrides window.matchMedia so
    // that prefers-color-scheme queries return the forced mode).
    let appearance_script = build_appearance_script(&settings.appearance);

    // ------------------------------------------------------------------
    // 4. Build the main window programmatically.
    // ------------------------------------------------------------------
    // Phase M trace: WebviewWindowBuilder construction starts here.  The
    // `.build()?` call below is the synchronous handoff to the OS WebView
    // (WebView2 on Windows, WebKitGTK on Linux, WKWebView on macOS) — on
    // Win11 first-run this can include WebView2 runtime initialization,
    // which historically dominates the 27 s white-screen gap.
    let webview_build_started = std::time::Instant::now();
    let builder = WebviewWindowBuilder::new(app, "main", webview_url)
        .title("Messenger X")
        .inner_size(1200.0, 800.0)
        .min_inner_size(400.0, 300.0)
        .resizable(true)
        .visible(!settings.start_minimized)
        // Inject all JS at document-start.
        .initialization_script(NOTIFICATION_OVERRIDE_SCRIPT)
        .initialization_script(UNREAD_OBSERVER_SCRIPT)
        .initialization_script(DIAGNOSTIC_TELEMETRY_SCRIPT)
        .initialization_script(AUDIO_HOOK_SCRIPT)
        .initialization_script(MEDIA_ERROR_LOGGER_SCRIPT)
        .initialization_script(OFFLINE_DIALOG_HIDER_SCRIPT)
        .initialization_script(&offline_banner_script)
        .initialization_script(&zoom_init_script)
        .initialization_script(&scrollbar_fix_script)
        .initialization_script(&appearance_script)
        .initialization_script(CALL_COMPAT_SCRIPT)
        .initialization_script(WINDOW_OPEN_OVERRIDE_SCRIPT);

    // On Linux, inject the Visibility API shim so that Rust can push the real
    // focus state and Messenger correctly fires desktop notifications when the
    // window is not active.  On each page load the shim immediately resyncs
    // _hidden from the actual OS window-focus state via IPC (get_window_focused)
    // so that re-navigations (e.g. logout) never inherit a stale startup value.
    #[cfg(target_os = "linux")]
    let builder = builder.initialization_script(VISIBILITY_OVERRIDE_SCRIPT);

    // On Linux, suppress the GNOME MPRIS media-player widget that would
    // otherwise display Messenger conversation titles / contact names in the
    // notification shade.  See MEDIA_SESSION_SUPPRESS_SCRIPT doc comment.
    #[cfg(target_os = "linux")]
    let builder = builder.initialization_script(MEDIA_SESSION_SUPPRESS_SCRIPT);

    // On Windows, pass Chromium flags directly via the WebviewWindowBuilder
    // `additional_browser_args` API.  This is the *only* reliable way to reach
    // WebView2's AdditionalBrowserArguments setting — the env-var approach
    // (WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS) is silently overridden by WRY
    // when it calls CreateCoreWebView2EnvironmentWithOptions.
    //
    // `resolve_webview2_proxy_args()` reads the Windows Internet Options proxy
    // settings from the registry and picks the correct Chromium proxy flag:
    //   - PAC URL configured → --proxy-pac-url=<url>
    //   - Manual proxy      → --proxy-server=<host:port>
    //   - WPAD only (no actual proxy server) → --no-proxy-server
    //                         (avoids the ~27 s WPAD discovery timeout)
    //   - No proxy at all   → (no proxy flag)
    // Always appends --disable-background-networking.
    #[cfg(target_os = "windows")]
    let proxy_args = resolve_webview2_proxy_args();
    #[cfg(target_os = "windows")]
    let builder = builder.additional_browser_args(&proxy_args);

    // Pre-clone the app handle for the download-completion notification.
    // `nav_app_handle` is moved into the `on_navigation` closure below.
    let dl_app_handle = nav_app_handle.clone();
    // Pre-clone for the `on_new_window` popup handler.
    let popup_app_handle = nav_app_handle.clone();

    let webview = builder
        // Spoof a desktop Chrome UA so Messenger serves its full web-app.
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
         AppleWebKit/537.36 (KHTML, like Gecko) \
         Chrome/136.0.0.0 Safari/537.36",
        )
        // ------------------------------------------------------------------
        // Phase G: Rust-side title-change unread-count detection.
        //
        // On Windows (WebView2) and Linux (WebKitGTK), Tauri v2 blocks
        // invoke() from remote origins (https://www.messenger.com) at the
        // WebView layer regardless of the capabilities `"remote"` block —
        // that block is designed for iOS/Android, not desktop WebViews.
        //
        // This handler fires whenever document.title changes (via Wry's
        // cross-platform title-changed hook) — no IPC, no origin checks.
        // Messenger sets the title to "(N) Messenger" for N unread messages
        // and back to "Messenger" when all are read.  Parsing that title
        // gives us the unread count without JS→Rust IPC at all.
        //
        // On macOS (where invoke() already works) this also fires but
        // NOTIF_STATE deduplication suppresses double-notifications when
        // the same count is reported by both paths.
        // ------------------------------------------------------------------
        .on_document_title_changed({
            let had_good_title = had_good_title.clone();
            let crash_reload_count = crash_reload_count.clone();
            let pending_sender_hint = pending_sender_hint.clone();
            let post_crash_proxy_block = post_crash_proxy_block.clone();
            let messenger_com_navigated = messenger_com_navigated.clone();
            let page_load_stable = page_load_stable.clone();
            move |webview_window, title| {
                const SENDER_HINT_PREFIX: &str = "__MX_SENDER_V1__?";
                if let Some(query) = title.strip_prefix(SENDER_HINT_PREFIX) {
                    let mut count_hint = None;
                    let mut sender_hint = None;
                    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
                        match key.as_ref() {
                            "count" => count_hint = value.parse::<u32>().ok(),
                            "sender" => {
                                let s = value.trim();
                                if !s.is_empty() && s.len() <= 120 {
                                    sender_hint = Some(s.to_string());
                                }
                            }
                            _ => {}
                        }
                    }

                    if let (Some(count), Some(sender)) = (count_hint, sender_hint) {
                        match pending_sender_hint.lock() {
                            Ok(mut hint) => {
                                hint.live =
                                    Some((count, sender.clone(), std::time::Instant::now()));
                                log::info!(
                                    "[MessengerX][SenderHint] stored count={} sender={:?}",
                                    count,
                                    sender,
                                );
                            }
                            Err(e) => log::warn!(
                                "[MessengerX][SenderHint] mutex poisoned; dropping hint: {e}"
                            ),
                        }
                    } else {
                        log::warn!(
                            "[MessengerX][SenderHint] malformed title payload: {:?}",
                            title
                        );
                    }

                    return;
                }

                // Parse "(N)" prefix — same logic as JS getUnreadCountFromTitle().
                let count: u32 = title
                    .trim_start()
                    .strip_prefix('(')
                    .and_then(|s| s.split_once(')'))
                    .and_then(|(n, _)| n.parse::<u32>().ok())
                    .unwrap_or(0);

                // Do NOT extract sender from "(N) SENDER | Messenger" — SENDER is the
                // *currently open* conversation, not the author of the new message.
                // Example: user has "Karel Novák" open; "Jouda" sends a message →
                // title becomes "(1) Karel Novák | Messenger" → wrong sender.
                // A DOM-derived sender hint may arrive shortly after the title
                // count via the `__MX_SENDER_V1__` sentinel side channel above.
                // We intentionally do NOT parse the title's `SENDER | Messenger`
                // segment here (Phase J: that segment is the open conversation,
                // not necessarily the message author).  The spawned update below
                // waits briefly for a verified DOM hint before firing.

                // Detect typing indicator: count==0 AND title is not the plain
                // "Messenger" (all-read) AND has no "(N)" prefix AND the title is
                // not empty AND does not contain " | Messenger" (which indicates a
                // background conversation tab title like "Alice | Messenger" —
                // Bug 2 false positive) AND the title is not blank (Bug 3: empty
                // title from WebKitWebProcess crash must not be classified as typing).
                let is_typing_indicator = count == 0
                    && !title.trim().is_empty()
                    && title.trim() != "Messenger"
                    && !title.trim_start().starts_with('(')
                    && !title.contains(" | Messenger");

                log::info!(
                    "[MessengerX][TitleChange] title={:?} parsed_count={} is_typing={} sender_hint_wait={}",
                    title,
                    count,
                    is_typing_indicator,
                    count > 0,
                );
                let app = webview_window.app_handle().clone();
                let sender_hint = pending_sender_hint.clone();
                std::thread::spawn(move || {
                    let sender = if count > 0 {
                        let deadline = std::time::Instant::now()
                            + std::time::Duration::from_millis(1_200);
                        let resolved = loop {
                            let mut peek_retained: Option<String> = None;
                            match sender_hint.lock() {
                                Ok(mut hint) => {
                                    if let Some((hint_count, sender, stored_at)) = hint.live.take()
                                    {
                                        let age = stored_at.elapsed();
                                        if hint_count == count
                                            && age <= std::time::Duration::from_secs(3)
                                        {
                                            log::info!(
                                                "[MessengerX][SenderHint] using DOM sender hint count={} sender={:?} age={}ms",
                                                count,
                                                sender,
                                                age.as_millis(),
                                            );
                                            // Promote to retained so a typing-
                                            // indicator-rearm fire (same conv,
                                            // count drops to 0 then returns)
                                            // can re-attribute the fire to the
                                            // same author without waiting for
                                            // a fresh JS hint that may never
                                            // arrive (DOM unchanged).
                                            hint.retained =
                                                Some((sender.clone(), std::time::Instant::now()));
                                            break sender;
                                        }

                                        log::info!(
                                            "[MessengerX][SenderHint] dropping stale hint count={} for parsed_count={} age={}ms",
                                            hint_count,
                                            count,
                                            age.as_millis(),
                                        );
                                    }

                                    // No live hint — peek retained for fallback
                                    // (do not consume yet; only used if the wait
                                    // deadline elapses with no live hint).
                                    if let Some((sender, consumed_at)) = &hint.retained {
                                        if consumed_at.elapsed() <= SENDER_HINT_RETAIN {
                                            peek_retained = Some(sender.clone());
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::warn!(
                                        "[MessengerX][SenderHint] mutex poisoned; using generic sender: {e}"
                                    );
                                    break String::new();
                                }
                            }

                            if std::time::Instant::now() >= deadline {
                                if let Some(sender) = peek_retained {
                                    log::info!(
                                        "[MessengerX][SenderHint] using retained sender (typing-rearm fallback) sender={:?} count={}",
                                        sender,
                                        count,
                                    );
                                    break sender;
                                }
                                break String::new();
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        };
                        resolved
                    } else {
                        String::new()
                    };

                    if let Err(e) = crate::commands::update_unread_count_from_title(
                        count,
                        sender,
                        is_typing_indicator,
                        app,
                    ) {
                        log::warn!("[MessengerX][TitleChange] update_unread_count error: {e}");
                    }
                });

                // ---------------------------------------------------------
                // Phase K: WebKitWebProcess crash detection + auto-reload.
                //
                // A "good" title contains "Messenger", meaning the SPA has
                // loaded successfully at least once this session.  If the
                // title subsequently becomes empty ("") the WebKitWebProcess
                // has almost certainly crashed (on Linux/VMware this is
                // caused by a NULL-pointer deref inside GStreamer's audio
                // pipeline when autoaudiosink is not found).
                //
                // Guard: had_good_title is reset before each reload so that
                // a series of back-to-back crashes doesn't bypass the count.
                //
                // ---------------------------------------------------------
                // CrashDetect: `had_good_title` arming and fire condition.
                //
                // `page_load_stable` (all platforms) gates both:
                //   • Setting `had_good_title = true` — prevents arming
                //     before the page has fully loaded after any navigation.
                //   • The CrashDetect fire — prevents false positives while
                //     the page is still loading (title="" is normal mid-load).
                //
                // This handles two distinct cases uniformly:
                //   • macOS WKWebView: title="" during EVERY SPA navigation
                //     (not just after a real crash) — the gate prevents
                //     false-positive CrashDetect fires during normal thread
                //     navigation.
                //   • Linux WebKitGTK: title="" during `window.location.reload()`
                //     (e.g. appearance toggle) — the gate prevents a false-
                //     positive because `on_navigation` resets `page_load_stable`
                //     to `false` before the reload and `on_page_load::Finished`
                //     re-arms it once the page is stable again.
                // ---------------------------------------------------------
                // Only count a title as "good" once a real messenger.com
                // navigation has been allowed.  The loading.html splash page
                // title ("Messenger X") also contains "Messenger", so without
                // this guard it would arm the crash detector — causing a false-
                // positive CrashDetect when the title briefly clears during the
                // initial navigation.
                //
                // Recovery: if `on_page_load::Finished` was not received (e.g.,
                // WebKitGTK SPA thread navigation may not fire Finished), a good
                // Messenger title re-arms `page_load_stable` so CrashDetect is
                // not permanently disarmed on Linux after the first SPA nav.
                if title.contains("Messenger")
                    && !title.trim().is_empty()
                    && messenger_com_navigated
                        .load(std::sync::atomic::Ordering::Relaxed)
                    && !page_load_stable.load(std::sync::atomic::Ordering::Relaxed)
                {
                    page_load_stable.store(true, std::sync::atomic::Ordering::Relaxed);
                    log::info!(
                        "[MessengerX][CrashDetect] page_load_stable=true \
                         (good-title recovery — on_page_load::Finished not received)"
                    );
                }
                if title.contains("Messenger")
                    && !title.trim().is_empty()
                    && messenger_com_navigated
                        .load(std::sync::atomic::Ordering::Relaxed)
                    && page_load_stable.load(std::sync::atomic::Ordering::Relaxed)
                {
                    had_good_title.store(true, std::sync::atomic::Ordering::Relaxed);
                }

                const MAX_CRASH_RELOADS: u32 = 3;
                // `page_load_stable` must be true (page fully loaded) before
                // CrashDetect fires.  This prevents false positives while the
                // page is loading after any navigation or reload on all platforms.
                if title.trim().is_empty()
                    && had_good_title.load(std::sync::atomic::Ordering::Relaxed)
                    && page_load_stable.load(std::sync::atomic::Ordering::Relaxed)
                {
                    let prev_count =
                        crash_reload_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if prev_count < MAX_CRASH_RELOADS {
                        log::warn!(
                            "[MessengerX][CrashDetect] WebKitWebProcess crash suspected \
                             (title became empty after successful load). \
                             Scheduling auto-reload {}/{MAX_CRASH_RELOADS}.",
                            prev_count + 1,
                        );
                        // Reset so the next reload doesn't immediately re-trigger.
                        had_good_title.store(false, std::sync::atomic::Ordering::Relaxed);
                        // Activate the post-crash fbsbx proxy block so that the
                        // maw_proxy_page navigation (which triggers the GStreamer
                        // NULL-pointer deref) is suppressed on the reloaded page.
                        post_crash_proxy_block.store(true, std::sync::atomic::Ordering::Relaxed);
                        let wv = webview_window.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_secs(2));
                            match url::Url::parse("https://www.messenger.com/") {
                                Ok(u) => {
                                    if let Err(e) = wv.navigate(u) {
                                        log::warn!(
                                            "[MessengerX][CrashDetect] navigate() failed: {e}"
                                        );
                                    }
                                }
                                Err(e) => {
                                    log::warn!("[MessengerX][CrashDetect] URL parse failed: {e}");
                                }
                            }
                        });
                    } else {
                        // Reset had_good_title (all platforms) so that
                        // subsequent empty-title events — whether from a
                        // real crash before the recovery page sets its
                        // title (Linux: GStreamer crash mid-load) or from
                        // a macOS SPA navigation — do not immediately
                        // re-trigger this else branch, creating an
                        // infinite navigate-to-root loop.
                        had_good_title.store(false, std::sync::atomic::Ordering::Relaxed);
                        log::warn!(
                            "[MessengerX][CrashDetect] Max crash reloads ({MAX_CRASH_RELOADS}) \
                              reached — navigating to messenger.com root to restore usability."
                        );
                        // Navigate to root so the user is not left with a
                        // blank / crashed window.  We do not increment the
                        // reload counter here — this is a recovery navigation,
                        // not another crash-triggered reload.
                        let wv = webview_window.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_secs(2));
                            match url::Url::parse("https://www.messenger.com/") {
                                Ok(u) => {
                                    if let Err(e) = wv.navigate(u) {
                                        log::warn!(
                                            "[MessengerX][CrashDetect] recovery navigate() \
                                             failed: {e}"
                                        );
                                    }
                                }
                                Err(e) => {
                                    log::warn!(
                                        "[MessengerX][CrashDetect] recovery URL parse \
                                         failed: {e}"
                                    );
                                }
                            }
                        });
                    }
                }
            }
        })
        // ------------------------------------------------------------------
        // Phase K: page-load timing hook.
        //
        // Logs `Started` (first byte / navigation committed) and `Finished`
        // (DOMContentLoaded equivalent) with milliseconds since app launch.
        // This lets us pinpoint which phase accounts for the 27-second white-
        // screen gap observed on Win11 (WebView2 boot timing diagnostic).
        //
         // `Finished` for www.messenger.com additionally marks
         // `page_load_stable = true` so the CrashDetect gate is re-armed
         // only after the page has fully settled (all platforms).
         //
         // On `Finished` we also clear `post_crash_proxy_block` so that the
         // fbsbx.com maw_proxy_page block (set on crash) is lifted once the
         // reloaded Messenger page has successfully finished loading.  Without
         // this clear the block is session-permanent: GIF picker and video
         // thumbnails break for the rest of the session after any crash.
         // ------------------------------------------------------------------
         .on_page_load({
             let page_load_stable_pl = page_load_stable.clone();
             let post_crash_proxy_block_pl = post_crash_proxy_block.clone();
             move |_webview_window, payload| {
                 use tauri::webview::PageLoadEvent;
                 let event_str = match payload.event() {
                     PageLoadEvent::Started => "Started",
                     PageLoadEvent::Finished => "Finished",
                 };
                 log::info!(
                     "[MessengerX][PageLoad] event={event_str} url={} t={}ms",
                     payload.url(),
                     setup_started.elapsed().as_millis(),
                 );
                 // All platforms: re-arm the crash-detect stable flag once the
                 // page has finished loading for messenger.com.  Also clear the
                 // post-crash proxy block so GIF/video paths are restored.
                 if matches!(payload.event(), PageLoadEvent::Finished) {
                     let host = payload.url().host_str().unwrap_or("");
                     if host == "www.messenger.com" {
                         page_load_stable_pl.store(true, std::sync::atomic::Ordering::Relaxed);
                         log::info!(
                             "[MessengerX][CrashDetect] page_load_stable=true (Finished, \
                              www.messenger.com)"
                         );
                         if post_crash_proxy_block_pl
                             .load(std::sync::atomic::Ordering::Relaxed)
                         {
                             post_crash_proxy_block_pl
                                 .store(false, std::sync::atomic::Ordering::Relaxed);
                             log::info!(
                                 "[MessengerX][CrashDetect] post_crash_proxy_block cleared \
                                  (page loaded successfully after crash)"
                             );
                         }
                     }
                 }
             }
         })
        // Navigation policy: allow Messenger / Facebook CDN domains;
        // open everything else in the system browser.
        .on_navigation({
            let post_crash_proxy_block = post_crash_proxy_block.clone();
            let messenger_com_navigated_nav = messenger_com_navigated.clone();
            let page_load_stable_nav = page_load_stable.clone();
            move |url| {
                let scheme = url.scheme();
                // Pass through non-HTTP schemes (blob:, data:, about:, tauri:, etc.).
                if scheme != "http" && scheme != "https" {
                    return true;
                }

                let host = url.host_str().unwrap_or("");

                // Pass through Tauri's own local asset server (tauri.localhost on
                // Windows / WebView2).  It uses the http scheme, so the scheme
                // check above does not catch it, and it must never be treated as
                // an external URL or opened in the system browser.
                if host == "tauri.localhost" || host == "localhost" {
                    return true;
                }
                log::info!(
                    "[MessengerX] on_navigation: host={host} url={url} t={}ms",
                    setup_started.elapsed().as_millis()
                );

                // Arm crash detection once a www.messenger.com navigation has
                // been allowed by the policy callback.  This prevents the
                // loading.html splash title ("Messenger X") from falsely
                // arming `had_good_title` before the SPA has actually loaded.
                if host == "www.messenger.com" {
                    messenger_com_navigated_nav
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    // Mark the page as not yet stable so that a transient
                    // title="" during the load (macOS SPA navigation or Linux
                    // window.location.reload()) doesn't trigger CrashDetect.
                    // Stability is restored by on_page_load::Finished.
                    page_load_stable_nav.store(false, std::sync::atomic::Ordering::Relaxed);
                }

                // Post-crash fbsbx proxy block (Linux only).
                //
                // After a confirmed WebKitWebProcess crash the maw_proxy_page on
                // www.fbsbx.com is the likely trigger of the GStreamer NULL-pointer
                // deref (autoaudiosink not found → media pipeline teardown crash).
                // Blocking this one navigation on the reloaded page suppresses the
                // crash loop while still allowing all other fbsbx.com content.
                // The block is only active when `post_crash_proxy_block` was set
                // true by CrashDetect — it is never active during normal startup.
                if cfg!(target_os = "linux")
                    && post_crash_proxy_block.load(std::sync::atomic::Ordering::Relaxed)
                    && host == "www.fbsbx.com"
                    && url.path().starts_with("/maw_proxy_page")
                {
                    log::warn!(
                        "[MessengerX][CrashDetect] Blocking post-crash fbsbx maw_proxy_page \
                     navigation to suppress GStreamer crash loop: {url}"
                    );
                    return false;
                }

                // Handle Facebook's link shim: l.facebook.com/l.php?u=EXTERNAL_URL
                // Messenger wraps all external links in this shim — so `on_navigation`
                // sees a facebook.com URL instead of the real destination.  We extract
                // the actual URL from the `u` query param and open it in the system browser.
                if (host == "l.facebook.com" || host == "l.messenger.com") && url.path() == "/l.php"
                {
                    if let Some(actual_url) = url
                        .query_pairs()
                        .find(|(k, _)| k == "u")
                        .map(|(_, v)| v.into_owned())
                    {
                        log::info!(
                            "[MessengerX] Link shim detected — opening real URL: {actual_url}"
                        );
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
                    // No `u` param — this is likely a Messenger OAuth / login
                    // cookie redirect (NOT an external link shim).  Let it
                    // navigate inside the WebView so the login flow can complete.
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

                // Persist the last concrete Messenger thread URL and restore it
                // on the next startup.  Win11/WebView2 currently spends ~27s on
                // the root `https://www.messenger.com/` SPA redirect before it
                // navigates to the thread; starting directly at the last thread
                // avoids that blank-window phase.
                if host == "www.messenger.com"
                    && (url.path().starts_with("/t/") || url.path().starts_with("/e2ee/t/"))
                {
                    // Skip e2ee thread URLs for persistence — on restart they
                    // cause empty-title → crash-detect → reload loops because
                    // WKWebView processes e2ee threads asynchronously and emits
                    // a blank title during initial load, which the crash detector
                    // misinterprets as a WebKitWebProcess crash.
                    if !url.path().starts_with("/e2ee/") {
                        let handle = nav_app_handle.clone();
                        let url_str = url.to_string();
                        std::thread::spawn(move || {
                            let mut s = services::auth::load_settings(&handle).unwrap_or_default();
                            if s.last_messenger_url.as_deref() == Some(url_str.as_str()) {
                                return;
                            }
                            s.last_messenger_url = Some(url_str.clone());
                            if let Err(e) = services::auth::save_settings(&handle, &s) {
                                log::warn!("[MessengerX][Boot] Failed to persist last URL: {e}");
                            } else {
                                log::info!("[MessengerX][Boot] persisted last URL: {url_str}");
                            }
                        });
                    }
                }

                // Phase M trace: explicit accept verdict so the count of
                // accept-vs-block decisions during boot can be tallied
                // straight from the log without inferring from absence.
                log::debug!(
                    "[MessengerX][Boot][Trace] on_navigation accept host={host} t={}ms",
                    setup_started.elapsed().as_millis()
                );
                true
            }
        })
        // ------------------------------------------------------------------
        // Issue #27 Fix A: Download handler with reveal-on-finish.
        //
        // Without this handler WKWebView/WebKitGTK/WebView2 silently save
        // files to a platform default directory with no UI.  We intercept
        // every download:
        //   • Requested — set destination to the system Downloads directory
        //     so the file goes to a predictable, user-accessible location.
        //     Add a numeric suffix if the path already exists (avoids
        //     NSURLErrorDomain -3000 on repeated downloads of the same file).
        //   • Finished  — send a silent system notification with the saved
        //     filename so the user knows the download completed.
        //
        // IMPORTANT: on macOS, `on_download` runs on the MAIN THREAD.
        // DO NOT call `blocking_save_file()` here — it deadlocks WKWebView's
        // download completion handler (nested NSSavePanel modal run loop).
        //
        // macOS note: `DownloadEvent::Finished.path` is always `None` due to
        // WKWebView API constraints.  We store the final path in `Arc<Mutex>`
        // from the Requested handler and use it in Finished.
        // ------------------------------------------------------------------
        .on_download({
            let download_dest: std::sync::Arc<
                std::sync::Mutex<Option<std::path::PathBuf>>,
            > = std::sync::Arc::new(std::sync::Mutex::new(None));
            let notify_handle = dl_app_handle.clone();
            move |_webview, event| {
                match event {
                    tauri::webview::DownloadEvent::Requested { url, destination } => {
                        // WRY pre-fills `destination` with the HTTP
                        // Content-Disposition `suggestedFilename` (macOS) or
                        // a path derived from the URL (Windows/Linux).
                        // Extract the filename stem from it; fall back to
                        // deriving from the URL path if the destination is
                        // empty or has no meaningful filename.
                        let fallback_name = url
                            .path_segments()
                            .and_then(|mut seg| seg.next_back())
                            .filter(|s| !s.is_empty())
                            .unwrap_or("download")
                            .to_string();

                        let base_name = destination
                            .file_name()
                            .and_then(|n| n.to_str())
                            .filter(|n| !n.is_empty() && *n != "download")
                            .unwrap_or(&fallback_name)
                            .to_string();

                        let downloads = dirs::download_dir().unwrap_or_else(|| {
                            std::env::temp_dir()
                        });

                        // Append a numeric suffix if the file already exists
                        // (e.g. "photo.jpg" → "photo (2).jpg").
                        let save_path = {
                            let mut candidate = downloads.join(&base_name);
                            if candidate.exists() {
                                let (stem, ext) = base_name
                                    .rfind('.')
                                    .map(|i| {
                                        (&base_name[..i], &base_name[i..])
                                    })
                                    .unwrap_or((base_name.as_str(), ""));
                                let mut n: u32 = 2;
                                loop {
                                    candidate = downloads.join(format!(
                                        "{stem} ({n}){ext}"
                                    ));
                                    if !candidate.exists() {
                                        break;
                                    }
                                    n = n.saturating_add(1);
                                }
                            }
                            candidate
                        };

                        log::info!(
                            "[MessengerX][Download] Requested url={url} \
                             base={base_name:?} path={save_path:?}"
                        );

                        *destination = save_path.clone();
                        if let Ok(mut guard) = download_dest.lock() {
                            *guard = Some(save_path);
                        }
                        true
                    }
                    tauri::webview::DownloadEvent::Finished { url, path, success } => {
                        let stored = download_dest.lock().ok().and_then(|g| g.clone());
                        let effective_path = path.or(stored);
                        let filename = effective_path
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .and_then(|n| n.to_str())
                            .unwrap_or("download");

                        log::info!(
                            "[MessengerX][Download] Finished url={url} \
                             file={filename} success={success}"
                        );

                        if success {
                            let body =
                                format!("Saved to Downloads — {filename}");
                            let h = notify_handle.clone();
                            // Spawn on a background thread — on macOS dev
                            // mode show_notification spawns the debug .app
                            // bundle as a subprocess (~4 s), which would
                            // beachball if called on the main thread here.
                            std::thread::spawn(move || {
                                let _ = crate::services::notification::show_notification(
                                    &h,
                                    "Messenger X",
                                    &body,
                                    "download",
                                    true,
                                    "download-finished",
                                );
                            });
                        }
                        true
                    }
                _ => true,
            }
        }
        })
        // ------------------------------------------------------------------
        // on_new_window — allow Messenger/Facebook popup windows (e.g. video
        // and audio call UI) to open as native Tauri windows.
        //
        // Background: WRY does not install a `createWebViewWith` UI delegate by
        // default, so any `window.open()` call that survives the JS
        // WINDOW_OPEN_OVERRIDE_SCRIPT and reaches the native layer is silently
        // dropped on WKWebView.  For Messenger call UI the site opens the call
        // widget in a popup on messenger.com / facebook.com — which our JS
        // override correctly allows through to `_originalOpen`.  Without this
        // handler those calls are lost and no video/audio call window appears.
        //
        // Security: only messenger.com / facebook.com / fbcdn.net URLs are
        // allowed.  Anything else is denied here; the JS override already
        // routes external URLs to the system browser before they even reach
        // this native handler.
        //
        // Cross-platform notes:
        //   macOS  — `.window_features(features)` propagates the parent's
        //            `WKWebViewConfiguration`, which shares the session store
        //            (cookies + storage) so the popup is authenticated.
        //   Linux  — `.window_features(features)` sets `related_view` so the
        //            new WebKitGTK WebView shares the same WebContext.
        //   Windows — `.window_features(features)` ties the popup to the same
        //            WebView2 environment (shared cookies / session).
        // ------------------------------------------------------------------
        .on_new_window({
            move |url, features| {
                let host = url.host_str().unwrap_or("");
                let allowed = host == "messenger.com"
                    || host.ends_with(".messenger.com")
                    || host == "facebook.com"
                    || host.ends_with(".facebook.com")
                    || host == "fbcdn.net"
                    || host.ends_with(".fbcdn.net");

                if !allowed {
                    log::info!(
                        "[MessengerX][Popup] Denied popup for non-FB url={}",
                        url.as_str()
                    );
                    return tauri::webview::NewWindowResponse::Deny;
                }

                let label = format!(
                    "popup-{}",
                    POPUP_WINDOW_COUNTER
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                );

                log::info!(
                    "[MessengerX][Popup] Opening popup label={label} url={}",
                    url.as_str()
                );

                // Use the URL directly so the new window loads the call page.
                let target_url = tauri::WebviewUrl::External(url.clone());
                let builder = tauri::WebviewWindowBuilder::new(
                    &popup_app_handle,
                    &label,
                    target_url,
                )
                // Propagates parent's WKWebViewConfiguration / WebContext /
                // WebView2 environment so cookies and session are shared.
                .window_features(features)
                .title("Messenger X")
                .inner_size(960.0, 640.0)
                .resizable(true)
                // Keep the title bar in sync with what the page reports.
                .on_document_title_changed(|window, title| {
                    let _ = window.set_title(&title);
                });

                match builder.build() {
                    Ok(window) => tauri::webview::NewWindowResponse::Create { window },
                    Err(e) => {
                        log::warn!(
                            "[MessengerX][Popup] Failed to build popup window label={label}: {e}"
                        );
                        tauri::webview::NewWindowResponse::Deny
                    }
                }
            }
        })
        .build()?;

    log::info!(
        "[MessengerX][Boot] webview window built (online={is_online}, t={}ms)",
        setup_started.elapsed().as_millis()
    );
    log::info!(
        "[MessengerX][Boot][Trace] WebviewWindowBuilder.build elapsed={}ms",
        webview_build_started.elapsed().as_millis()
    );

    // ------------------------------------------------------------------
    // 4a-loading. If we started with the local loading.html splash page,
    //   navigate to the real messenger.com URL now in a background thread.
    //
    //   A short delay (100 ms) gives the WebView time to paint the spinner
    //   before we trigger the network navigation.  Without the delay, the
    //   navigate() call races with the initial paint and the spinner may
    //   never be visible.  100 ms is imperceptible to the user but long
    //   enough for the compositor to flush the first frame.
    // ------------------------------------------------------------------
    if let Some(target_url) = navigate_after_build {
        let navigate_webview = webview.clone();
        log::info!(
            "[MessengerX][Boot] scheduling navigate to {target_url} after 100ms splash delay"
        );
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if let Err(e) = navigate_webview.navigate(target_url) {
                log::warn!("[MessengerX][Boot] post-build navigate failed: {e}");
            }
        });
    }

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

    // Apply initial window chrome theme (title bar / decorations) based on
    // the saved appearance setting.  `None` = follow OS (system default).
    {
        let chrome_theme = match settings.appearance.as_str() {
            "dark" => Some(tauri::Theme::Dark),
            "light" => Some(tauri::Theme::Light),
            _ => None,
        };
        if let Err(e) = webview.set_theme(chrome_theme) {
            log::warn!("[MessengerX] Failed to apply initial window theme: {e}");
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
    // 4c-2. Cross-platform: reset notification guard when the window gains
    //       focus AND the unread count is 0 (messages already read).
    //
    //       Guard: we only reset when count == 0.  If count > 0 the user
    //       has NOT yet read the pending messages; resetting to Idle here
    //       would cause a duplicate notification on the next polling tick.
    //
    //       This also filters spurious WebView2 focus events on Windows
    //       (H2-variant), where the runtime fires Focused(true) ~8x per
    //       99 s without any user interaction.  Those events arrive while
    //       count is still > 0, so the count guard silently skips them
    //       and prevents erroneous notification re-fires.
    //
    //       When count == 0 and the window is truly focused (user has seen
    //       the messages), resetting to Idle is correct: it ensures the
    //       NEXT new message always triggers a notification immediately
    //       rather than waiting for the 7-second ZeroPending→Idle timer.
    // ------------------------------------------------------------------
    {
        webview.on_window_event(move |event| {
            if let tauri::WindowEvent::Focused(true) = event {
                use std::sync::atomic::Ordering;
                let current_count = crate::commands::LAST_UNREAD_COUNT.load(Ordering::SeqCst);
                if current_count > 0 && current_count != u32::MAX {
                    // Messages still unread — do NOT reset; a real focus would be
                    // handled via the ZeroPending→Idle path once count drops to 0.
                    log::debug!(
                        "[MessengerX][Notification] Focused(true) with count={} — skip reset (messages unread or spurious event)",
                        current_count
                    );
                    return;
                }
                if let Ok(mut state) = crate::commands::NOTIF_STATE.lock() {
                    if *state == crate::commands::NotifState::Idle {
                        // Already idle — no-op, avoid log spam from repeated events.
                        return;
                    }
                    *state = crate::commands::NotifState::Idle;
                }
                log::info!(
                    "[MessengerX][Notification] Window gained focus (count=0) — notification state reset to Idle"
                );
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
                log::info!("[MessengerX][Visibility] WindowEvent::Focused({focused})");
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
            // Track minimized state separately so that LAST_RESTORED_FROM_MINIMIZED_SECS
            // is only stamped on a genuine minimize→restore transition.  Without this,
            // Cinnamon/Muffin XUrgency blinks (triggered by request_user_attention) flip
            // is_focused true/false rapidly; the poll thread would see a not-visible→visible
            // edge on every blink and keep re-stamping, holding came_from_background=true
            // across many seconds and allowing sig_changed to fire 4+ duplicate notifications
            // from a single group message.
            let mut was_minimized = initially_minimized;
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
                    // Fix 1: stamp the restore-from-minimized timestamp so that
                    // update_unread_count_core can detect the came_from_background
                    // condition within RESTORE_GRACE_SECS.
                    // Guard: only stamp when the window was actually minimized before
                    // this transition.  Cinnamon XUrgency blinks produce rapid
                    // not-focused→focused edges WITHOUT a preceding minimize; stamping
                    // on those would hold came_from_background=true through the entire
                    // blink sequence and let sig_changed fire duplicate notifications.
                    if is_visible && !was_visible && was_minimized {
                        let restore_ts = crate::commands::now_secs();
                        crate::commands::LAST_RESTORED_FROM_MINIMIZED_SECS
                            .store(restore_ts, std::sync::atomic::Ordering::SeqCst);
                        log::info!(
                            "[MessengerX][Visibility] Window restored from minimized — \
                             stamped LAST_RESTORED_FROM_MINIMIZED_SECS={}",
                            restore_ts
                        );
                    }
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
                // Always update was_minimized so the next iteration knows the true
                // prior minimized state regardless of whether visibility changed.
                was_minimized = is_minimized;
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
            app.autolaunch().is_enabled().unwrap_or(settings.autostart)
        };

        // --- Build menu items ---

        // Non-interactive version label at the top of the menu.
        let version_str = format!("v{}", app.package_info().version);
        let version_item = MenuItemBuilder::with_id("app_version", &version_str)
            .enabled(false)
            .build(app)?;

        let show_item = MenuItemBuilder::with_id("tray_show", &tr.tray_show).build(app)?;

        // Toggles
        let stay_logged_in_item =
            CheckMenuItemBuilder::with_id("stay_logged_in", &tr.settings_stay_logged_in)
                .checked(settings.stay_logged_in)
                .build(app)?;

        let notifications_item = CheckMenuItemBuilder::with_id(
            "notifications_enabled",
            &tr.settings_notifications_enabled,
        )
        .checked(settings.notifications_enabled)
        .build(app)?;

        let notification_sound_item =
            CheckMenuItemBuilder::with_id("notification_sound", &tr.settings_notification_sound)
                .checked(settings.notification_sound)
                .build(app)?;

        // Appearance submenu with radio-like CheckMenuItems (System/Dark/Light)
        let appearance_system_item =
            CheckMenuItemBuilder::with_id("appearance_system", &tr.settings_appearance_system)
                .checked(settings.appearance == "system")
                .build(app)?;

        let appearance_dark_item =
            CheckMenuItemBuilder::with_id("appearance_dark", &tr.settings_appearance_dark)
                .checked(settings.appearance == "dark")
                .build(app)?;

        let appearance_light_item =
            CheckMenuItemBuilder::with_id("appearance_light", &tr.settings_appearance_light)
                .checked(settings.appearance == "light")
                .build(app)?;

        let appearance_submenu = SubmenuBuilder::new(app, &tr.settings_appearance)
            .item(&appearance_system_item)
            .item(&appearance_dark_item)
            .item(&appearance_light_item)
            .build()?;

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
        let autostart_item = CheckMenuItemBuilder::with_id("autostart", &tr.settings_autostart)
            .checked(autostart_checked)
            .build(app)?;

        let start_minimized_item =
            CheckMenuItemBuilder::with_id("start_minimized", &tr.settings_start_minimized)
                .checked(settings.start_minimized)
                .build(app)?;

        let auto_update_item =
            CheckMenuItemBuilder::with_id("auto_update", &tr.settings_auto_update)
                .checked(settings.auto_update)
                .build(app)?;

        // Action items
        let check_update_item =
            MenuItemBuilder::with_id("check_update", &tr.settings_check_update).build(app)?;

        let view_logs_item =
            MenuItemBuilder::with_id("view_logs", &tr.settings_view_logs).build(app)?;

        let clear_logs_item =
            MenuItemBuilder::with_id("clear_logs", &tr.settings_clear_logs).build(app)?;

        let logout_item = MenuItemBuilder::with_id("logout", &tr.settings_logout).build(app)?;

        let quit_item = MenuItemBuilder::with_id("tray_quit", &tr.tray_quit).build(app)?;

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
            .item(&appearance_submenu)
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
        let appearance_system_c = appearance_system_item.clone();
        let appearance_dark_c = appearance_dark_item.clone();
        let appearance_light_c = appearance_light_item.clone();

        // Translated strings for update-check notifications.
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

                    // ---- Appearance (radio-like behaviour) ----
                    _ if id.starts_with("appearance_") => {
                        let mode = &id["appearance_".len()..]; // "system", "dark", or "light"
                        // Enforce radio: uncheck all, check selected.
                        let _ = appearance_system_c.set_checked(mode == "system");
                        let _ = appearance_dark_c.set_checked(mode == "dark");
                        let _ = appearance_light_c.set_checked(mode == "light");
                        // Persist.
                        let mut s =
                            services::auth::load_settings(handle).unwrap_or_default();
                        s.appearance = mode.to_string();
                        let _ = services::auth::save_settings(handle, &s);
                        // Apply: store in sessionStorage + reload so the
                        // initialization_script picks up the new value.
                        if let Some(wv) = handle.get_webview_window("main") {
                            let script = format!(
                                "sessionStorage.setItem('__mx_appearance','{}');\
                                 window.location.reload();",
                                mode
                            );
                            if let Err(e) = wv.eval(&script) {
                                log::warn!("[MessengerX][Appearance] eval failed: {e}");
                            }
                            // Update window chrome theme (title bar) immediately.
                            let theme = match mode {
                                "dark" => Some(tauri::Theme::Dark),
                                "light" => Some(tauri::Theme::Light),
                                _ => None,
                            };
                            let _ = wv.set_theme(theme);
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
                                                                    "updater-installed",
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
                                                                    "updater-install-failed",
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
                                            "updater-up-to-date",
                                        );
                                    }
                                    Err(e) => {
                                        log::warn!(
                                            "[MessengerX] Update check failed: {e}"
                                        );
                                        let _ = services::notification::show_notification(
                                            &h, "Messenger X", &tr_err, "update", false,
                                            "updater-check-failed",
                                        );
                                    }
                                },
                                Err(e) => {
                                    log::warn!("[MessengerX] Updater init failed: {e}");
                                    let _ = services::notification::show_notification(
                                        &h, "Messenger X", &tr_err, "update", false,
                                        "updater-init-failed",
                                    );
                                }
                            }
                        });
                    }

                    // ---- View Logs ----
                    "view_logs" => {
                        if let Ok(log_dir) = handle.path().app_log_dir() {
                            let log_file = log_dir.join("messengerx.log");
                            log::info!(
                                "[MessengerX] view_logs: path={} exists={}",
                                log_file.display(),
                                log_file.exists()
                            );
                            // On Linux (AppImage): use open_log_on_linux() which
                            // strips LD_LIBRARY_PATH/LD_PRELOAD before spawning.
                            // Step 1: xdg-open on a .txt symlink (text editor).
                            // Step 2: terminal + less fallback.
                            // Step 3: xdg-open on log directory (file manager).
                            #[cfg(target_os = "linux")]
                            open_log_on_linux(&log_dir, &log_file);

                            #[cfg(not(target_os = "linux"))]
                            {
                                use tauri_plugin_opener::OpenerExt;
                                let p = if log_file.exists() {
                                    log_file.to_string_lossy().into_owned()
                                } else {
                                    let _ = std::fs::create_dir_all(&log_dir);
                                    log_dir.to_string_lossy().into_owned()
                                };
                                if let Err(e) =
                                    handle.opener().open_path(p, None::<&str>)
                                {
                                    log::warn!(
                                        "[MessengerX] view_logs: opener failed: {e}"
                                    );
                                }
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
                        if let Err(e) = services::cache::clear_snapshots(handle) {
                            log::warn!("[MessengerX] Failed to clear snapshots during logout: {e}");
                        }
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

                        // Reset appearance to system.
                        let _ = appearance_system_c.set_checked(true);
                        let _ = appearance_dark_c.set_checked(false);
                        let _ = appearance_light_c.set_checked(false);
                        if let Some(wv) = handle.get_webview_window("main") {
                            let _ = wv.set_theme(None);
                        }

                        // Disable autostart.
                        {
                            use tauri_plugin_autostart::ManagerExt;
                            let _ = handle.autolaunch().disable();
                        }
                        let _ = autostart_c.set_checked(false);

                        // Best-effort client-side clear of JS-accessible cookies
                        // (including path variants), localStorage, and sessionStorage
                        // before navigating to messenger.com.  HttpOnly cookies and
                        // cookies for other origins are not affected by this script.
                        if let Some(wv) = handle.get_webview_window("main") {
                            if let Err(e) = wv.eval(LOGOUT_CLEAR_SCRIPT) {
                                log::warn!("[MessengerX] Failed to eval logout clear script: {e}");
                            }
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
            app.autolaunch().is_enabled().unwrap_or(settings.autostart)
        };

        // --- Build all menu items ---

        let show_item = MenuItemBuilder::with_id("tray_show", &tr.tray_show).build(app)?;

        let stay_logged_in_item =
            CheckMenuItemBuilder::with_id("stay_logged_in", &tr.settings_stay_logged_in)
                .checked(settings.stay_logged_in)
                .build(app)?;

        let notifications_item = CheckMenuItemBuilder::with_id(
            "notifications_enabled",
            &tr.settings_notifications_enabled,
        )
        .checked(settings.notifications_enabled)
        .build(app)?;

        let notification_sound_item =
            CheckMenuItemBuilder::with_id("notification_sound", &tr.settings_notification_sound)
                .checked(settings.notification_sound)
                .build(app)?;

        // Appearance submenu with radio-like CheckMenuItems (System/Dark/Light)
        let appearance_system_item =
            CheckMenuItemBuilder::with_id("appearance_system", &tr.settings_appearance_system)
                .checked(settings.appearance == "system")
                .build(app)?;

        let appearance_dark_item =
            CheckMenuItemBuilder::with_id("appearance_dark", &tr.settings_appearance_dark)
                .checked(settings.appearance == "dark")
                .build(app)?;

        let appearance_light_item =
            CheckMenuItemBuilder::with_id("appearance_light", &tr.settings_appearance_light)
                .checked(settings.appearance == "light")
                .build(app)?;

        let appearance_submenu = SubmenuBuilder::new(app, &tr.settings_appearance)
            .item(&appearance_system_item)
            .item(&appearance_dark_item)
            .item(&appearance_light_item)
            .build()?;

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

        let autostart_item = CheckMenuItemBuilder::with_id("autostart", &tr.settings_autostart)
            .checked(autostart_checked)
            .build(app)?;

        let start_minimized_item =
            CheckMenuItemBuilder::with_id("start_minimized", &tr.settings_start_minimized)
                .checked(settings.start_minimized)
                .build(app)?;

        let auto_update_item =
            CheckMenuItemBuilder::with_id("auto_update", &tr.settings_auto_update)
                .checked(settings.auto_update)
                .build(app)?;

        let check_update_item =
            MenuItemBuilder::with_id("check_update", &tr.settings_check_update).build(app)?;

        let view_logs_item =
            MenuItemBuilder::with_id("view_logs", &tr.settings_view_logs).build(app)?;

        let clear_logs_item =
            MenuItemBuilder::with_id("clear_logs", &tr.settings_clear_logs).build(app)?;

        let logout_item = MenuItemBuilder::with_id("logout", &tr.settings_logout).build(app)?;

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
            .item(&appearance_submenu)
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
        let appearance_system_c = appearance_system_item.clone();
        let appearance_dark_c = appearance_dark_item.clone();
        let appearance_light_c = appearance_light_item.clone();

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

                // ---- Appearance (radio-like behaviour) ----
                _ if id.starts_with("appearance_") => {
                    let mode = &id["appearance_".len()..]; // "system", "dark", or "light"
                                                           // Enforce radio: uncheck all, check selected.
                    let _ = appearance_system_c.set_checked(mode == "system");
                    let _ = appearance_dark_c.set_checked(mode == "dark");
                    let _ = appearance_light_c.set_checked(mode == "light");
                    // Persist.
                    let mut s = services::auth::load_settings(&h).unwrap_or_default();
                    s.appearance = mode.to_string();
                    let _ = services::auth::save_settings(&h, &s);
                    // Apply: store in sessionStorage + reload so the
                    // initialization_script picks up the new value.
                    if let Some(wv) = h.get_webview_window("main") {
                        let script = format!(
                            "sessionStorage.setItem('__mx_appearance','{}');\
                             window.location.reload();",
                            mode
                        );
                        if let Err(e) = wv.eval(&script) {
                            log::warn!("[MessengerX][Appearance] eval failed: {e}");
                        }
                        // Update window chrome theme (title bar) immediately.
                        let theme = match mode {
                            "dark" => Some(tauri::Theme::Dark),
                            "light" => Some(tauri::Theme::Light),
                            _ => None,
                        };
                        let _ = wv.set_theme(theme);
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
                                    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
                                    h2.dialog()
                                        .message(&body)
                                        .title(&tr_dlg_title)
                                        .buttons(MessageDialogButtons::OkCancelCustom(
                                            tr_install, tr_later,
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
                                        &h2,
                                        "Messenger X",
                                        &tr_none,
                                        "update",
                                        false,
                                        "updater-up-to-date",
                                    );
                                }
                                Err(e) => {
                                    log::warn!("[MessengerX] Update check failed: {e}");
                                    let _ = services::notification::show_notification(
                                        &h2,
                                        "Messenger X",
                                        &tr_err,
                                        "update",
                                        false,
                                        "updater-check-failed",
                                    );
                                }
                            },
                            Err(e) => {
                                log::warn!("[MessengerX] Updater init failed: {e}");
                                let _ = services::notification::show_notification(
                                    &h2,
                                    "Messenger X",
                                    &tr_err,
                                    "update",
                                    false,
                                    "updater-init-failed",
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
                        log::info!(
                            "[MessengerX] view_logs: path={} exists={}",
                            log_file.display(),
                            log_file.exists()
                        );
                        let p = if log_file.exists() {
                            log_file.to_string_lossy().into_owned()
                        } else {
                            let _ = std::fs::create_dir_all(&log_dir);
                            log_dir.to_string_lossy().into_owned()
                        };
                        if let Err(e) = h.opener().open_path(p, None::<&str>) {
                            log::warn!("[MessengerX] view_logs: opener failed: {e}");
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
                                    log::warn!(
                                        "[MessengerX] Failed to delete {}: {e}",
                                        path.display()
                                    );
                                } else {
                                    log::info!("[MessengerX] Deleted log file: {}", path.display());
                                }
                            }
                        }
                    }
                }

                // ---- Log out & clear data ----
                "logout" => {
                    if let Err(e) = services::cache::clear_snapshots(&h) {
                        log::warn!("[MessengerX] Failed to clear snapshots during logout: {e}");
                    }
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

                    // Reset appearance to system.
                    let _ = appearance_system_c.set_checked(true);
                    let _ = appearance_dark_c.set_checked(false);
                    let _ = appearance_light_c.set_checked(false);
                    if let Some(wv) = h.get_webview_window("main") {
                        let _ = wv.set_theme(None);
                    }

                    // Disable autostart.
                    {
                        use tauri_plugin_autostart::ManagerExt;
                        let _ = h.autolaunch().disable();
                    }
                    let _ = autostart_c.set_checked(false);

                    // Best-effort client-side clear of JS-accessible cookies
                    // (including path variants), localStorage, and sessionStorage
                    // before navigating to messenger.com.  HttpOnly cookies and
                    // cookies for other origins are not affected by this script.
                    if let Some(wv) = h.get_webview_window("main") {
                        if let Err(e) = wv.eval(LOGOUT_CLEAR_SCRIPT) {
                            log::warn!("[MessengerX] Failed to eval logout clear script: {e}");
                        }
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
                                use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
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
                                                                "updater-startup-installed",
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
                                                                "updater-startup-install-failed",
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
                            log::warn!("[MessengerX] Startup updater init failed: {e}");
                        }
                    }
                });
            });
        }
    }

    log::info!(
        "[MessengerX][Boot] setup_app complete (t={}ms)",
        setup_started.elapsed().as_millis()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    mod sender_hint_cache {
        use super::super::{SenderHintCache, SENDER_HINT_RETAIN};
        use std::time::{Duration, Instant};

        #[test]
        fn default_cache_is_empty() {
            let c = SenderHintCache::default();
            assert!(c.live.is_none());
            assert!(c.retained.is_none());
        }

        #[test]
        fn live_take_then_retained_promotion_round_trip() {
            let mut c = SenderHintCache {
                live: Some((1, "Alice".into(), Instant::now())),
                ..Default::default()
            };
            // Simulate worker promotion path.
            let taken = c.live.take().expect("live present");
            assert_eq!(taken.1, "Alice");
            c.retained = Some((taken.1.clone(), Instant::now()));
            assert!(c.live.is_none());
            let (sender, _) = c.retained.as_ref().expect("retained set");
            assert_eq!(sender, "Alice");
        }

        #[test]
        fn retained_ttl_is_long_enough_to_cover_typing_pause() {
            // Phase M design: a typical typing pause + reply burst should
            // remain inside the retain window so the rearm fire reuses the
            // sender.  Guard against accidental shrinkage to e.g. 1 s.
            assert!(
                SENDER_HINT_RETAIN >= Duration::from_secs(5),
                "retain window must cover typical typing pause (>= 5s)"
            );
            // Also guard against accidental growth that would risk
            // misattributing a later message from a different conversation.
            assert!(
                SENDER_HINT_RETAIN <= Duration::from_secs(30),
                "retain window must stay short enough to avoid cross-conv attribution"
            );
        }

        #[test]
        fn retained_expiry_check_uses_consumed_at_age() {
            // Stamp consumed_at far in the past — outside retain window.
            let stale = Instant::now() - SENDER_HINT_RETAIN - Duration::from_secs(1);
            let c = SenderHintCache {
                retained: Some(("Bob".into(), stale)),
                ..Default::default()
            };
            let (_sender, consumed_at) = c.retained.as_ref().unwrap();
            assert!(
                consumed_at.elapsed() > SENDER_HINT_RETAIN,
                "stale retained entry must be detected as expired"
            );
        }
    }

    mod windows_startup_activation {
        const SOURCE: &str = include_str!("lib.rs");

        #[test]
        fn windows_startup_activation_runs_in_ready_event() {
            assert!(
                SOURCE.contains("#[cfg(target_os = \"windows\")]")
                    && SOURCE.contains("if let tauri::RunEvent::Ready = event"),
                "Windows startup activation must remain in RunEvent::Ready"
            );
        }

        #[test]
        fn startup_minimized_preference_stays_guarded_via_initial_visibility() {
            assert!(
                SOURCE.contains(".visible(!settings.start_minimized)"),
                "main window creation must keep start_minimized visibility guard"
            );
            assert!(
                SOURCE.contains("matches!(window.is_visible(), Ok(true))"),
                "Windows Ready activation must only run for initially visible windows"
            );
        }
    }

    #[cfg(target_os = "linux")]
    mod linux_runtime {
        use super::super::{configure_linux_runtime_env, LINUX_APPIMAGE_ENV_OVERRIDES};
        use std::env;
        use std::sync::{Mutex, OnceLock};

        fn env_lock() -> &'static Mutex<()> {
            static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
            ENV_LOCK.get_or_init(|| Mutex::new(()))
        }

        fn managed_env_keys() -> Vec<&'static str> {
            let mut keys: Vec<&'static str> = LINUX_APPIMAGE_ENV_OVERRIDES
                .iter()
                .map(|(key, _)| *key)
                .collect();
            keys.push("APPIMAGE");
            keys
        }

        struct EnvRestoreGuard {
            original: Vec<(&'static str, Option<String>)>,
        }

        impl EnvRestoreGuard {
            fn capture(keys: &[&'static str]) -> Self {
                Self {
                    original: keys.iter().map(|key| (*key, env::var(key).ok())).collect(),
                }
            }
        }

        impl Drop for EnvRestoreGuard {
            fn drop(&mut self) {
                for (key, value) in &self.original {
                    match value {
                        Some(value) => unsafe { env::set_var(key, value) },
                        None => unsafe { env::remove_var(key) },
                    }
                }
            }
        }

        #[test]
        fn appimage_runtime_overrides_are_skipped_outside_appimage() {
            let _guard = env_lock().lock().unwrap();
            let keys = managed_env_keys();
            let _restore = EnvRestoreGuard::capture(&keys);

            for key in &keys {
                unsafe { env::remove_var(key) };
            }

            configure_linux_runtime_env();

            for (key, _) in LINUX_APPIMAGE_ENV_OVERRIDES.iter() {
                assert!(
                    env::var(key).is_err(),
                    "configure_linux_runtime_env() must not set {key} outside AppImage"
                );
            }
        }

        #[test]
        fn appimage_runtime_overrides_preserve_existing_operator_values() {
            let _guard = env_lock().lock().unwrap();
            let keys = managed_env_keys();
            let _restore = EnvRestoreGuard::capture(&keys);

            for key in &keys {
                unsafe { env::remove_var(key) };
            }
            unsafe { env::set_var("APPIMAGE", "/tmp/MessengerX.AppImage") };

            let preserved_key = LINUX_APPIMAGE_ENV_OVERRIDES[0].0;
            unsafe { env::set_var(preserved_key, "operator-provided-value") };

            configure_linux_runtime_env();

            for (key, expected_value) in LINUX_APPIMAGE_ENV_OVERRIDES.iter() {
                let actual_value = env::var(key)
                    .unwrap_or_else(|_| panic!("expected {key} to be set in AppImage"));

                if *key == preserved_key {
                    assert_eq!(
                        actual_value, "operator-provided-value",
                        "configure_linux_runtime_env() must preserve an existing {key} value"
                    );
                } else {
                    assert_eq!(
                        actual_value, *expected_value,
                        "configure_linux_runtime_env() must populate {key} from LINUX_APPIMAGE_ENV_OVERRIDES when it is unset"
                    );
                }
            }
        }
    }

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

    // -----------------------------------------------------------------------
    // Logout script regression tests
    // -----------------------------------------------------------------------
    mod logout_script {
        use super::super::LOGOUT_CLEAR_SCRIPT;

        /// The script must attempt to clear cookies (the core privacy action).
        #[test]
        fn clears_cookies() {
            assert!(
                LOGOUT_CLEAR_SCRIPT.contains("document.cookie"),
                "logout script must touch document.cookie"
            );
        }

        /// The script must clear localStorage and sessionStorage.
        #[test]
        fn clears_storage() {
            assert!(
                LOGOUT_CLEAR_SCRIPT.contains("localStorage.clear()"),
                "logout script must clear localStorage"
            );
            assert!(
                LOGOUT_CLEAR_SCRIPT.contains("sessionStorage.clear()"),
                "logout script must clear sessionStorage"
            );
        }

        /// The script must reset the appearance override key in sessionStorage
        /// so the next login page shows the correct theme.
        #[test]
        fn resets_appearance_key() {
            assert!(
                LOGOUT_CLEAR_SCRIPT.contains("__mx_appearance"),
                "logout script must reset __mx_appearance"
            );
        }

        /// The script must navigate to messenger.com after clearing.
        #[test]
        fn navigates_to_messenger() {
            assert!(
                LOGOUT_CLEAR_SCRIPT.contains("https://www.messenger.com"),
                "logout script must redirect to messenger.com"
            );
        }

        /// Cookie expiry must use the RFC1123 GMT format (not UTC) for
        /// cross-WebView compatibility.
        #[test]
        fn cookie_expiry_uses_gmt() {
            assert!(
                LOGOUT_CLEAR_SCRIPT.contains("00:00:00 GMT"),
                "cookie expiry must use GMT format"
            );
            assert!(
                !LOGOUT_CLEAR_SCRIPT.contains("00:00:00 UTC"),
                "cookie expiry must not use UTC format"
            );
        }

        /// The cookie loop must guard against empty cookie names so that
        /// `document.cookie === ''` does not generate invalid assignments.
        #[test]
        fn skips_empty_cookie_names() {
            assert!(
                LOGOUT_CLEAR_SCRIPT.contains("if (!name) return"),
                "logout script must skip empty cookie names"
            );
        }

        /// The script must clear cookies for the current pathname and all
        /// parent path prefixes (e.g. `/messages/t/123`, `/messages/t`,
        /// `/messages`, `/`), not just `path=/`.  It must also try the
        /// trailing-slash variant of each prefix (e.g. `/messages/` is a
        /// distinct cookie identity from `/messages`).
        #[test]
        fn clears_cookie_path_variants() {
            assert!(
                LOGOUT_CLEAR_SCRIPT.contains("pathSegments"),
                "logout script must compute path segments"
            );
            assert!(
                LOGOUT_CLEAR_SCRIPT.contains("window.location.pathname"),
                "logout script must read current pathname"
            );
            assert!(
                LOGOUT_CLEAR_SCRIPT.contains("var pSlash = p + '/'"),
                "logout script must clear trailing-slash path variants"
            );
        }

        /// The cookie-domain loop must stop before the TLD to avoid
        /// assigning cookies to e.g. `domain=com`.
        #[test]
        fn stops_before_tld() {
            assert!(
                LOGOUT_CLEAR_SCRIPT.contains("hostParts.length > 1"),
                "cookie loop must stop before TLD"
            );
        }

        /// The script must be wrapped in an IIFE so that `var` bindings
        /// do not leak onto the global `window` object.
        #[test]
        fn wrapped_in_iife() {
            assert!(
                LOGOUT_CLEAR_SCRIPT.trim_start().starts_with("(function()"),
                "logout script must be wrapped in an IIFE"
            );
            assert!(
                LOGOUT_CLEAR_SCRIPT.trim_end().ends_with(")();"),
                "logout script IIFE must end with )();"
            );
        }
    }

}
