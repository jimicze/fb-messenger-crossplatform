## v1.4.0 — Windows startup lag eliminated · notification engine overhaul · Linux stability

This release consolidates 20 patch releases (v1.3.19–v1.3.38) into a curated milestone update.
The three headline changes are: the Windows 27-second startup lag is gone, the cross-platform
notification engine was completely overhauled, and Linux received a round of stability fixes.

### ✨ What's new

- **Notification engine overhaul** — Unified dispatcher with a deduplication window, smart sender
  cache (avoids re-fetching the sender name on typing-indicator re-arms), per-call diagnostic IDs
  for log correlation, and Win11 boot-timing trace to pinpoint startup gaps.
- **60-second notification re-arm** — If a conversation stays unread for 60 seconds and the window
  is not focused, the notification fires again so messages are not silently buried.
- **Typing-indicator re-arm** — A notification re-fires after 5 seconds when the sender is still
  typing and the user has not focused the window.
- **Linux crash recovery** — The renderer auto-reloads after a GStreamer / fbsbx crash (up to
  3 retries) so the app recovers without user intervention.
- **Windows ARM64 CI** — Official ARM64 Windows builds added to the release matrix.
- **`npm run artifacts:pull`** — Download CI debug bundles directly from the command line.

### 🐛 Bug fixes

#### Windows
- **27-second startup lag eliminated** — WebView2 was invoking the Windows WPAD proxy-discovery
  stack (`WinHttpGetProxyForUrl`) on every launch, timing out after ~27 s on networks without a
  WPAD server.  Messenger X now reads the Windows Internet Options registry key and passes the
  correct Chromium flag (`--no-proxy-server`, `--proxy-server=…`, or `--proxy-pac-url=…`)
  directly to WebView2, bypassing all WPAD discovery when no proxy is configured.  The fix applies
  regardless of whether "Automatically detect settings" is on or off in Windows Proxy Settings.
- **Splash screen** — A `loading.html` page is shown during WebView2 initialisation so the window
  is never blank on first launch.
- **Startup foreground** — Main window is brought to the foreground on launch to prevent the
  initial-load lag where the window appeared behind other apps.
- **Notification delivery** — Notifications now fire from the Rust title-change handler on all
  platforms, eliminating the dependency on JS↔Rust IPC that was blocked on WebView2.

#### Linux
- **MPRIS media widget suppressed** — `navigator.mediaSession` is overridden with a no-op proxy
  (`configurable: false`) so Messenger's audio/video activity no longer spawns a GNOME media
  player widget in the system tray.
- **Startup notification suppressed** — `StartupNotify=false` in the `.desktop` template
  eliminates the spurious GNOME "application is starting" toast on every launch.
- **AppImage startup guard** — Guards against host GIO module ABI conflicts that caused the
  AppImage to crash immediately on some distributions.
- **Regression fix** — `suppress_gnome_startup_notification()` was accidentally deleted in
  v1.3.35 and is now restored.

#### Notifications (all platforms)
- **Wrong sender name removed** — The window title's `SENDER | Messenger` segment is the
  *currently open* conversation, not the author of the incoming message.  Sender extraction from
  the title was removed; the notification now shows the correct sender (from DOM hints) or a
  generic fallback.
- **`focused-read-all`** — When the window is focused and the unread count drops to 0, the dock/
  taskbar badge clears immediately without waiting for a timer.
- **Typing-indicator false positives fixed** — Titles like `Alice | Messenger` (background tab)
  and empty crash titles are no longer mis-classified as typing indicators.

#### CI / Infrastructure
- **CI version-bump race fixed** — The bump job now fetches and rebases before pushing, eliminating
  a rare race that caused the push to fail when another CI run landed at the same time.
- **GitHub remote URL corrected** — Typo `fb-messanger-crossplatform` → `fb-messenger-crossplatform`.

