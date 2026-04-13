# TASKS.md — Messenger X

## 🐛 Bugs

### [BUG-001] Cold-start offline mode shows error page instead of cached content
- **Priority:** High
- **Status:** ✅ Fixed
- **Description:** When the app is opened without internet connection, the WebView displays a native browser error page ("Safari cannot open the page" / ERR_INTERNET_DISCONNECTED) instead of loading the last cached snapshot.
- **Root cause:** `load_snapshot` IPC command exists but is never called automatically on startup. No fallback mechanism when `messenger.com` fails to load.
- **Fix (implemented):**
  1. `is_likely_online()` check at startup — if offline, loads local `index.html` instead of messenger.com
  2. Cached snapshot injected into webview via `document.write()` with offline banner
  3. Auto-reconnect timer (15s) redirects to messenger.com when connectivity is restored
  4. `SNAPSHOT_TRIGGER_SCRIPT` now guards against offline/error pages (checks `navigator.onLine` + URL)

### [BUG-002] Offline message sync between web and mobile
- **Priority:** Low
- **Status:** ⏭️ Won't Fix (Facebook-side issue)
- **Description:** Messages sent while in flaky network conditions are sometimes visible on web but not synced to mobile app. This is a Facebook server-side sync issue — Messenger X does not intercept or modify message sending in any way.

### [BUG-003] Windows SmartScreen blocks installer as unrecognized app
- **Priority:** High
- **Status:** 🔴 Open
- **Description:** NSIS installer triggers Windows SmartScreen warning ("Windows protected your PC") — SmartScreen blocks execution of the unsigned app.
- **Root cause:** The .exe is not code-signed with a valid Windows Authenticode certificate.
- **Fix:** Requires FEAT-003 (Windows Authenticode code signing). Until then, users must click "More info" → "Run anyway".
- **Related:** FEAT-003

### [BUG-004] App icon has white corners on Windows desktop shortcut
- **Priority:** Medium
- **Status:** ✅ Fixed (v0.2.3)
- **Description:** Desktop shortcut icon on Windows displays white corners instead of transparent ones.
- **Root cause:** Incorrect AND mask in BMP layers of icon.ico — AND bit was 0 for transparent pixels. Pillow's ICO save does not generate correct AND masks.
- **Fix:** Rebuilt icon.ico manually with correct AND masks; regenerated all PNG icons with alpha threshold cleanup.

### [BUG-005] Settings window unreachable
- **Priority:** High
- **Status:** ✅ Fixed (v0.2.4)
- **Description:** After launching the app, users had no way to reach the Settings window.
- **Fix:**
  - Tray context menu with "Show Window", "Settings", "Quit" (localized en/cs)
  - macOS app menu bar: Messenger X → Settings (⌘,), Quit (⌘Q); Edit → standard items
  - Keyboard shortcut Cmd+, / Ctrl+, injected into WebView via JS
  - `open_settings` IPC command + `open_settings_window()` helper

### [BUG-006] latest.json race condition in CI
- **Priority:** High
- **Status:** ✅ Fixed (v0.2.4)
- **Description:** Parallel matrix jobs each generated their own `latest.json` — first uploader won, rest got `already_exists` error. The uploaded manifest was incomplete.
- **Fix:** Three-phase workflow: (1) create-release, (2) build-tauri with `includeUpdaterJson: false`, (3) `publish-updater` job generates single complete `latest.json` after all builds.

### [BUG-007] Tray context menu not showing on Windows
- **Priority:** High
- **Status:** ✅ Fixed (v0.2.5)
- **Description:** Right-click on tray icon did not show context menu on Windows.
- **Root cause:** `on_tray_icon_event` matched ALL clicks including right-click → window stole focus → OS menu closed before rendering.
- **Fix:** Filter only `MouseButton::Left` + `MouseButtonState::Up` in the handler.

### [BUG-008] Settings window broken on Windows (MIME type enforcement)
- **Priority:** Critical
- **Status:** ✅ Fixed (v0.2.7)
- **Description:** Settings window completely non-functional on Windows — no toggles, no zoom, no updates worked.
- **Root cause:** `<script type="module" src="settings.ts">` — WebView2 (Chromium) rejects `.ts` files served as `video/mp2t` MIME type. WebKit on macOS was lenient so bug was invisible during development.
- **Fix:** Replaced with inline `<script>` using `window.__TAURI__.core.invoke` directly. Added `check_for_update()` and `install_update()` Rust IPC commands.

### [BUG-009] Settings window buttons non-functional (withGlobalTauri)
- **Priority:** Critical
- **Status:** ✅ Fixed (v0.2.8)
- **Description:** Zoom +/- buttons and "Check for updates" button render but do nothing on any platform.
- **Root cause:** `app.withGlobalTauri` not set in `tauri.conf.json` → `window.__TAURI__` is `undefined` in page-level inline scripts → `init()` returns early at the guard check → no event listeners attached.
- **Fix:** Added `"withGlobalTauri": true` to `app` section of `tauri.conf.json`. Also fixed `.update-status` CSS `transition: all` → `transition: color 0.2s`.

---

## ✨ Feature Requests

### [FEAT-001] i18n — System language localization
- **Priority:** Medium
- **Status:** ✅ Done (v0.1.4)
- **Description:** ~30 hardcoded English strings localized. Languages: English, Czech.
- **Implementation:** `sys-locale` crate, `services/locale.rs`, `get_translations()` IPC, settings UI fully localized.

### [FEAT-002] Auto-update support
- **Priority:** Medium
- **Status:** ✅ Done
- **Description:** Tauri updater plugin integrated with signing keypair, Settings UI section, CI generates `latest.json`.

### [FEAT-003] Code signing (SignPath Foundation + Apple notarization)
- **Priority:** High
- **Status:** 🔄 In Progress (čeká na external schválení)
- **Description:** Sign release builds to eliminate OS security warnings. CI je plně připraveno — stačí přidat secrets.
- **Windows:** SignPath Foundation (free for OSS) — aplikace podána 2026-04-12, čeká na schválení
- **macOS:** Apple notarization — čeká na obnovu Apple Developer Program ($99/rok)
- **Unblocks po dokončení:**
  - BUG-003 (Windows SmartScreen)
  - DIST-006 (homebrew/homebrew-cask) — viz níže
- **Related:** Resolves BUG-003

### [FEAT-004] Enhanced notifications
- **Priority:** Medium
- **Status:** ✅ Done (v0.1.4)
- **Description:** Platform-specific sounds, silent mode, grouping, tray click handler to focus main window.

### [FEAT-005] Package manager distribution (winget, Homebrew)
- **Priority:** Medium
- **Status:** ✅ Done (v1.3.1+)
- **Description:** Publish to platform-native package managers with CI auto-update.
- **winget:** Initial PR submitted to `microsoft/winget-pkgs` (#358865, v1.3.5), CLA signed, awaiting merge; CI Phase 5 (`update-winget`) auto-submits PRs for each new release after merge
- **Homebrew:** Tap `jimicze/homebrew-tap` live — `brew tap jimicze/tap && brew install --cask messenger-x`; CI Phase 4 (`update-homebrew`) auto-updates Cask formula on every release

### [FEAT-006] Notification settings
- **Priority:** Medium
- **Status:** ✅ Done (v0.2.8)
- **Description:** Added notification controls to Settings window.
- **Implementation:**
  - `AppSettings` extended with `notifications_enabled: bool` and `notification_sound: bool` (default `true`)
  - `send_notification` gating: if `!notifications_enabled` → early return; if `!notification_sound` → force `silent: true`
  - Settings UI: new "Notifications" section with two toggles
  - i18n: 3 new translation keys (en + cs)

### [FEAT-007] Auto-start at login + minimize to tray
- **Priority:** Medium
- **Status:** ✅ Done (v0.2.8)
- **Description:** App starts automatically at system boot, sits in tray, collects notifications in background.
- **Implementation:**
  - `tauri-plugin-autostart` integrated (macOS Login Items, Windows Registry, Linux `.desktop` in `~/.config/autostart/`)
  - `AppSettings` extended with `autostart: bool` and `start_minimized: bool` (default `false`)
  - Wrapper IPC commands: `set_autostart(enabled)` and `is_autostart_enabled()`
  - Main window: `.visible(!settings.start_minimized)` in builder
  - Settings UI: new "Startup" section with two toggles; autostart queries OS state on load
  - i18n: 3 new translation keys (en + cs)
  - Capabilities: added `autostart:default` permission

---

## 🚀 Distribuce

### [DIST-001] Flathub (Linux Flatpak)
- **Priority:** High
- **Effort:** M (2–4 h)
- **Status:** ❌ Zrušeno
- **Description:** Zrušeno — `flatpak/` adresář odstraněn z repozitáře.

### [DIST-002] Snap Store (Ubuntu/Snapcraft)
- **Priority:** Low
- **Effort:** S (1–2 h)
- **Status:** 📋 Planned
- **Description:** Publikovat na Snap Store — předinstalovaný na Ubuntu, dostupný na dalších distribucích. Snap je alternativa k Flatpak s automatickými aktualizacemi.
- **Kroky:**
  1. Vytvořit `snap/snapcraft.yaml` (grade: stable, confinement: strict, base: core22)
  2. Zaregistrovat snap název `messenger-x` na snapcraft.io
  3. Přidat CI krok: `snapcraft pack` + `snapcraft upload` po releasu
- **Install (po schválení):** `sudo snap install messenger-x`
- **Prerekvizity:** Účet na snapcraft.io

### [DIST-003] AUR (Arch Linux)
- **Priority:** Low
- **Effort:** S (1 h)
- **Status:** 📋 Planned
- **Description:** Publikovat AUR balíček pro Arch Linux a Arch-based distribuce (Manjaro, EndeavourOS). AUR je populární u power userů.
- **Kroky:**
  1. Vytvořit AUR repozitář `messenger-x-bin` (binární balíček z AppImage/tar.gz)
  2. Napsat `PKGBUILD` stahující AppImage z GitHub Releases
  3. Přidat CI krok: auto-update `PKGBUILD` + `.SRCINFO` po releasu (podobně jako Homebrew)
- **Install (po schválení):** `yay -S messenger-x-bin`
- **Prerekvizity:** AUR účet na aur.archlinux.org

### [DIST-004] Microsoft Store
- **Priority:** Low
- **Effort:** XL (1–2 dny + čekání na review)
- **Status:** 📋 Planned
- **Description:** Publikovat na Microsoft Store — eliminuje SmartScreen warning pro Store verzi, velká viditelnost. Alternativa/doplněk k FEAT-003 (code signing).
- **Kroky:**
  1. Zaregistrovat se jako vývojář na Partner Center ($19 jednorázově)
  2. Zabalit NSIS installer do MSIX (`msixpackagingtool` nebo `tauri bundle --target msix`)
  3. Nakonfigurovat `tauri.conf.json`: `bundle.windows.wix` nebo `bundle.windows.nsis` + Store-specifická konfigurace
  4. Podat aplikaci přes Partner Center, projít certifikaci
- **Prerekvizity:** $19 Developer Account; FEAT-003 (Windows signing) doporučeno ale ne striktně nutné

---

## ✨ Funkce / UX

### [FEAT-008] Dock badge (macOS) + taskbar overlay (Windows)
- **Priority:** Medium
- **Effort:** M (3–5 h)
- **Status:** 📋 Planned
- **Description:** Zobrazit počet nepřečtených zpráv přímo na ikoně v docku (macOS) a jako overlay badge na taskbaru (Windows). Aktuálně se unread count zobrazuje jen v tray tooltipu.
- **macOS:** `NSApp.dockTile.badgeLabel` přes Tauri plugin nebo custom Rust kód s `objc` crate
- **Windows:** `ITaskbarList3::SetOverlayIcon()` Win32 API — Tauri zatím nemá nativní podporu, nutný custom Rust kód přes `windows-rs` crate
- **Linux:** Tray tooltip (žádný standardní badge API)
- **Prerekvizity:** Žádné

### [FEAT-009] Deep links (`messenger://`)
- **Priority:** Low
- **Effort:** S (2–3 h)
- **Status:** 📋 Planned
- **Description:** Registrovat `messenger://` URL schéma → kliknutí na Messenger odkaz v prohlížeči otevře/přenese focus na app.
- **Implementace:** `tauri-plugin-deep-link` (Tauri v2 plugin); registrace URL schématu v `tauri.conf.json`; handler otevře okno + naviguje na správnou konverzaci
- **Prerekvizity:** Žádné

### [FEAT-010] Tray rychlé akce
- **Priority:** Low
- **Effort:** S (1–2 h)
- **Status:** 📋 Planned
- **Description:** Přidat kontextové akce přímo do tray menu bez nutnosti otevřít hlavní okno.
- **Možné akce:** "Mark all as read" (JS inject do WebView), "Open last conversation"
- **Prerekvizity:** Žádné

---

## 🏗️ Infrastruktura

### [INFRA-001] GitHub community files
- **Priority:** Low
- **Effort:** XS (30 min)
- **Status:** 📋 Planned
- **Description:** Přidat standardní GitHub community soubory pro open-source projekt.
- **Soubory:**
  - `.github/ISSUE_TEMPLATE/bug_report.md`
  - `.github/ISSUE_TEMPLATE/feature_request.md`
  - `.github/CONTRIBUTING.md`
  - `.github/PULL_REQUEST_TEMPLATE.md`
- **Prerekvizity:** Žádné

### [INFRA-002] AUR CI auto-update
- **Priority:** Low
- **Effort:** S (1–2 h)
- **Status:** 📋 Planned
- **Description:** CI Phase 6 nebo samostatný workflow — po každém releasu automaticky aktualizuje `PKGBUILD` v AUR repozitáři (verze + SHA256 AppImage).
- **Prerekvizity:** DIST-003 (AUR balíček musí existovat)

---

## ⏳ Čeká na externí schválení

| Task | Čeká na | Odhadovaný čas |
|------|---------|----------------|
| FEAT-003 macOS notarization | Apple Developer Program obnova ($99/rok) | — |
| FEAT-003 Windows signing | SignPath Foundation schválení (podáno 2026-04-12) | týdny |
| BUG-003 SmartScreen fix | FEAT-003 Windows signing | — |
| DIST-005 winget auto-update | Merge winget PR #358865 | dny–týdny |
| DIST-006 homebrew/homebrew-cask | FEAT-003 Apple notarization | — |

### Poznámka k Homebrew
Aktuálně je Cask v custom tapu `jimicze/homebrew-tap` — vyžaduje `brew tap jimicze/tap` před instalací.
Po Apple notarizaci (FEAT-003) bude možné podat PR do hlavního `homebrew/homebrew-cask` repozitáře.
Teprve pak bude app discoverable přes `brew search` a instalovatelná jedním příkazem:
```bash
brew install --cask messenger-x
```
