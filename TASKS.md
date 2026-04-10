# TASKS.md — Messenger X

## 🐛 Bugs

### [BUG-001] Cold-start offline mode shows error page instead of cached content
- **Priority:** High
- **Status:** ✅ Fixed
- **Description:** When the app is opened without internet connection, the WebView displays a native browser error page ("Safari cannot open the page" / ERR_INTERNET_DISCONNECTED) instead of loading the last cached snapshot.
- **Root cause:** `load_snapshot` IPC command exists but is never called automatically on startup. No fallback mechanism when `messenger.com` fails to load.
- **Additional issue:** The 60-second snapshot timer doesn't check `navigator.onLine` — it can overwrite the last good snapshot with an offline/error page snapshot.
- **Fix (implemented):**
  1. `is_likely_online()` check at startup — if offline, loads local `index.html` instead of messenger.com
  2. Cached snapshot injected into webview via `document.write()` with offline banner
  3. Auto-reconnect timer (15s) redirects to messenger.com when connectivity is restored
  4. `SNAPSHOT_TRIGGER_SCRIPT` now guards against offline/error pages (checks `navigator.onLine` + URL)

### [BUG-002] Offline message sync between web and mobile
- **Priority:** Low
- **Status:** ⏭️ Won't Fix (Facebook-side issue)
- **Description:** Messages sent while in flaky network conditions are sometimes visible on web but not synced to mobile app. This is a Facebook server-side sync issue — Messenger X does not intercept or modify message sending in any way.

---

## ✨ Feature Requests

### [FEAT-001] i18n — System language localization
- **Priority:** Medium
- **Status:** 📋 Planned
- **Description:** The app has ~30 hardcoded English strings in native UI (tray tooltip, loading screen, offline banner, settings window, confirmation dialogs, NSIS installer). These should adapt to the system language.
- **Affected areas:**
  - Rust: tray tooltip (lib.rs, commands.rs)
  - HTML: loading screen (index.html), settings window (settings/index.html)
  - JS: offline banner (injected in lib.rs), confirm dialog (settings.ts)
  - Installer: NSIS languages (tauri.conf.json)
- **Approach:**
  1. Detect system locale via `sys_locale` crate (Rust) + `navigator.language` (JS)
  2. Create JSON translation files (`locales/en.json`, `locales/cs.json`, etc.)
  3. Expose current locale via IPC command
  4. Frontend loads translations at startup
- **Languages to support initially:** English, Czech

### [FEAT-002] Auto-update support
- **Priority:** Medium
- **Status:** 📋 Planned
- **Description:** Integrate Tauri updater plugin for automatic update checks and in-app update flow.

### [FEAT-003] Code signing
- **Priority:** Medium
- **Status:** 📋 Planned
- **Description:** Apple notarization + Windows Authenticode signing for release builds.

### [FEAT-004] System notification styles (banners, alerts, sounds)
- **Priority:** Medium
- **Status:** 📋 Planned
- **Description:** Currently notifications are forwarded to the OS via `tauri-plugin-notification` with just title + body. Add support for richer notification features:
  - **Notification style:** Respect OS banner vs. alert preference
  - **Sound:** Play the system default or a custom Messenger sound
  - **Grouping:** Group notifications by conversation (use `tag` field already captured)
  - **Actions:** Add quick-reply / mark-as-read action buttons (where OS supports it)
  - **App icon in notification:** Ensure the Messenger X icon shows in notification center
  - **Click handling:** Clicking a notification should focus the app and navigate to the relevant conversation
- **Affected areas:**
  - Rust: `services/notification.rs`, `commands.rs` (`send_notification`)
  - JS: `NOTIFICATION_OVERRIDE_SCRIPT` in `lib.rs` (capture more fields from `Notification` options)
  - Tauri: may need additional notification plugin capabilities
