<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Messenger X" width="128" height="128" />
</p>

<h1 align="center">Messenger X</h1>

<p align="center">
  <strong>A lightweight, cross-platform desktop client for Facebook Messenger</strong>
</p>

<p align="center">
  <a href="https://github.com/jimicze/fb-messanger-crossplatform/releases"><img src="https://img.shields.io/github/v/release/jimicze/fb-messanger-crossplatform?style=flat-square&color=blue&label=Release" alt="Latest Release" /></a>
  <a href="https://github.com/jimicze/fb-messanger-crossplatform/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflow/status/jimicze/fb-messanger-crossplatform/release.yml?style=flat-square&label=CI" alt="CI Status" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-green?style=flat-square" alt="MIT License" /></a>
  <img src="https://img.shields.io/badge/Tauri-v2-orange?style=flat-square&logo=tauri" alt="Tauri v2" />
  <img src="https://img.shields.io/badge/Rust-2021-brown?style=flat-square&logo=rust" alt="Rust" />
  <img src="https://img.shields.io/badge/TypeScript-5-blue?style=flat-square&logo=typescript" alt="TypeScript" />
  <br />
  <a href="https://formulae.brew.sh/cask/messenger-x"><img src="https://img.shields.io/badge/Homebrew-messenger--x-FBB040?style=flat-square&logo=homebrew" alt="Homebrew" /></a>
  <img src="https://img.shields.io/badge/winget-jimicze.MessengerX-0078D4?style=flat-square&logo=windows" alt="winget" />
</p>

<p align="center">
  <a href="#-downloads">Downloads</a> •
  <a href="#-features">Features</a> •
  <a href="#%EF%B8%8F-build-from-source">Build from Source</a> •
  <a href="#-architecture">Architecture</a> •
  <a href="#-contributing">Contributing</a>
</p>

---

> **Why?** — Facebook discontinued the official Messenger desktop app. Browser tabs lack native OS integration. Messenger X fills the gap — native notifications, dock badges, persistent sessions, and offline support in a **~8 MB** binary (vs ~120 MB Electron alternatives).

## 📥 Downloads

### Package managers

```sh
# macOS — Homebrew
brew tap jimicze/tap
brew install --cask messenger-x

# Windows — winget
winget install jimicze.MessengerX
```

### Direct installers

Pick the right installer for your platform:

<table>
  <tr>
    <th>Platform</th>
    <th>Architecture</th>
    <th>Format</th>
    <th>Download</th>
  </tr>
  <tr>
    <td>🍎 <strong>macOS</strong></td>
    <td>Apple Silicon (M1+)</td>
    <td><code>.dmg</code></td>
    <td rowspan="2"><a href="https://github.com/jimicze/fb-messanger-crossplatform/releases/latest"><strong>Latest Release →</strong></a></td>
  </tr>
  <tr>
    <td>🍎 <strong>macOS</strong></td>
    <td>Intel x86_64</td>
    <td><code>.dmg</code></td>
  </tr>
  <tr>
    <td>🪟 <strong>Windows</strong></td>
    <td>x64</td>
    <td><code>.exe</code> (NSIS installer)</td>
    <td rowspan="2"><a href="https://github.com/jimicze/fb-messanger-crossplatform/releases/latest"><strong>Latest Release →</strong></a></td>
  </tr>
  <tr>
    <td>🪟 <strong>Windows</strong></td>
    <td>ARM64</td>
    <td><code>.exe</code> (NSIS installer)</td>
  </tr>
  <tr>
    <td>🐧 <strong>Linux</strong></td>
    <td>x64</td>
    <td><code>.deb</code> / <code>.rpm</code> / <code>.AppImage</code></td>
    <td rowspan="2"><a href="https://github.com/jimicze/fb-messanger-crossplatform/releases/latest"><strong>Latest Release →</strong></a></td>
  </tr>
  <tr>
    <td>🐧 <strong>Linux</strong></td>
    <td>ARM64</td>
    <td><code>.deb</code> / <code>.rpm</code> / <code>.AppImage</code></td>
  </tr>
</table>

<details>
<summary><strong>🐧 Which Linux package should I use?</strong></summary>
<br />

| Distro | Recommended format |
|--------|-------------------|
| **Debian** / **Ubuntu** / **Linux Mint** / **Pop!_OS** | `.deb` |
| **Fedora** / **RHEL** / **openSUSE** | `.rpm` |
| **Arch Linux** / **Manjaro** / **NixOS** / **any other** | `.AppImage` |

</details>

## ✨ Features

<table>
  <tr>
    <td width="50%" valign="top">

#### 🌐 Native WebView Client
Loads `messenger.com` in a system WebView — no bundled browser engine. Uses your OS's native web renderer for minimal resource usage.

#### 🔔 Native Notifications
Web notifications are intercepted and forwarded to your OS notification system. Works with notification centers on all platforms.

#### 🔢 Unread Badge
Parses the unread count from the page title and displays it on your dock/taskbar icon and system tray tooltip.

#### 🔐 Persistent Sessions
Your login session persists across app restarts. Toggle "Stay logged in" in Settings, or log out and wipe all data with one click.

</td>
<td width="50%" valign="top">

#### 📴 Offline Mode
Automatic HTML snapshots every 60 seconds. When you go offline, the app shows cached content with a non-intrusive banner — no blank screens.

#### 🔄 Auto-Updates
Built-in update checker in Settings. One-click download, install, and restart — no manual re-downloading needed.

#### 🌍 Localization
Automatically detects your system language. Currently supports English and Czech, with easy extensibility for more languages.

#### 🔍 Zoom Controls
Zoom from 50% to 300%. Zoom level persists across sessions.

#### 🪟 Window Management
Window size and position are saved and restored. Default: 1200×800, minimum: 400×300.

#### 🔗 Smart Link Handling
Facebook/Messenger URLs stay in-app. External links open in your default system browser automatically.

</td>
  </tr>
</table>

<details>
<summary><strong>Platform notes — notifications &amp; startup</strong></summary>
<br />

| Platform | Notification system | Requirements / notes |
|----------|---------------------|----------------------|
| **macOS** | `UNUserNotificationCenter` | Grant permission in **System Settings → Notifications → Messenger X**. Notification sound plays on delivery. |
| **Windows** | WinRT toast | Grant permission in **Settings → System → Notifications**. Focus Assist / Do Not Disturb suppresses toasts while active. |
| **Linux (GNOME)** | `libnotify` (`notify-send`) with plugin fallback | Install `libnotify-bin` for best results. MPRIS media widget is suppressed automatically. |
| **Linux (KDE / XFCE / other)** | Native notification plugin | Works out of the box on most desktop environments. |

**Windows startup note**: Messenger X reads your Windows proxy settings from the registry and
passes the correct flag to WebView2, so the 20–30 second blank-window delay that occurred on
networks without a proxy server is fully eliminated as of v1.3.38.

</details>

## 🏗️ Architecture

```
┌─────────────────────────────────────────────────┐
│                  Messenger X                     │
├──────────────────────┬──────────────────────────┤
│   Frontend (TS)      │   Backend (Rust)          │
│                      │                           │
│   messenger.com      │   Tauri v2 Framework      │
│   loaded in WebView  │   ├─ Plugin: Notification │
│                      │   ├─ Plugin: Opener        │
│   JS Injection:      │   ├─ Plugin: Window-State  │
│   ├─ Notifications   │   ├─ Plugin: Updater       │
│   ├─ Unread Observer │   ├─ Plugin: Autostart     │
│   ├─ Offline Banner  │   │                       │
│   └─ Zoom Control    │   Services:               │
│                      │   ├─ auth.rs    (session) │
│   All settings live  │   ├─ cache.rs   (offline) │
│   in the native      │   ├─ locale.rs  (i18n)    │
│   tray context menu  │   ├─ network.rs (monitor) │
│                      │   └─ notification.rs      │
│        invoke()      │   10 IPC Commands          │
│   ─────────────────► │   ├─ send_notification    │
│                      │   ├─ update_unread_count  │
│   ◄───────────────── │   ├─ get/save_settings    │
│        eval()        │   ├─ save/load_snapshot   │
│                      │   ├─ clear_all_data       │
│                      │   ├─ open_external        │
│                      │   ├─ set/get_zoom         │
│                      │   └─ check/install_update │
├──────────────────────┴──────────────────────────┤
│             System WebView                       │
│   macOS: WebKit · Windows: WebView2              │
│   Linux: WebKitGTK                               │
└─────────────────────────────────────────────────┘
```

**Key design decisions:**
- **No framework** — vanilla TypeScript, zero frontend dependencies
- **System WebView** — no bundled Chromium, keeps the binary at ~8 MB
- **JS injection from Rust** — all scripts are injected at document-start via `initialization_script()`, not loaded from files
- **Programmatic window creation** — main window is built in Rust `setup()`, not `tauri.conf.json`, to enable custom navigation hooks and script injection

## ⚙️ Build from Source

### Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| **Node.js** | 18+ | Frontend tooling |
| **Rust** | stable (1.70+) | Backend compilation |
| **Tauri CLI** | 2.x | Build orchestration |

<details>
<summary><strong>🐧 Linux: Additional system dependencies</strong></summary>

```bash
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libappindicator3-dev \
  librsvg2-dev \
  patchelf \
  libgtk-3-dev \
  libglib2.0-dev \
  libssl-dev \
  libdbus-1-dev
```

</details>

### Build

```bash
# Clone
git clone https://github.com/jimicze/fb-messanger-crossplatform.git
cd fb-messanger-crossplatform

# Install dependencies
npm install

# Development (hot-reload)
npm run tauri dev

# Release build
npm run tauri build

# Debug build
npm run tauri build -- --debug
```

### Verify

```bash
# Type-check frontend
npx tsc --noEmit

# Lint Rust
cargo clippy          # from src-tauri/

# Run Rust tests
cargo test            # from src-tauri/
```

## 📁 Project Structure

```
fb-messanger-crossplatform/
├── src/                          # TypeScript frontend
│   ├── index.html                #   Loading splash screen
│   └── scripts/                  #   JS injection stubs (actual JS in lib.rs)
│       ├── main.ts
│       ├── bridge.ts
│       ├── notifications.ts
│       ├── unread.ts
│       └── offline.ts
├── src-tauri/                    # Rust backend
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/             #   Tauri v2 permission model
│   ├── icons/                    #   App icons (all platforms)
│   └── src/
│       ├── main.rs               #   Entry point
│       ├── lib.rs                #   App setup, JS injection, navigation
│       ├── commands.rs           #   10 IPC command handlers
│       └── services/
│           ├── auth.rs           #   Session & settings persistence
│           ├── cache.rs          #   HTML snapshot management
│           ├── locale.rs         #   i18n (en, cs)
│           ├── network.rs        #   Network monitoring
│           └── notification.rs   #   Native notification dispatch
└── .github/workflows/
    └── release.yml               # Cross-platform CI/CD
```

## 🚀 Release Process

Releases are automated via GitHub Actions. To create a new release:

```bash
# Tag and push
git tag v1.0.0
git push origin v1.0.0
```

This triggers builds for **6 parallel jobs** (macOS arm64 + x64, Windows x64 + arm64, Linux x64 + arm64), signs update artifacts, publishes `latest.json` for auto-updates, bumps the Homebrew tap, and submits a winget PR — all automatically.

## 🛡️ Security & Privacy

- **No data collection** — Messenger X doesn't phone home or collect any telemetry
- **No API reverse-engineering** — simply wraps the official `messenger.com` web interface
- **Local persistence only** — sessions, settings, and cached snapshots are stored in your OS app data directory
- **Navigation allowlist** — only `messenger.com`, `facebook.com`, `fbcdn.net`, and `fbsbx.com` are loaded in-app; everything else opens in your system browser
- **Hardened runtime** (macOS) enabled for Gatekeeper compatibility
- **Windows code signing** — provided by [SignPath Foundation](https://signpath.org) (free code signing for open-source projects)

## 🗺️ Roadmap

- [x] **Phase 1** — Core features (WebView, notifications, badges, offline, zoom, settings)
- [x] **i18n** — System language detection, English + Czech localization
- [x] **Enhanced notifications** — Platform-specific sounds, silent mode, tray click handler
- [x] **Auto-updates** — Built-in update checker & installer via Tauri updater plugin
- [ ] Code signing — [SignPath Foundation](https://signpath.org) (Windows) + Apple notarization (macOS)
- [x] **Package managers** — Homebrew tap (`brew install --cask messenger-x`) + winget (`winget install jimicze.MessengerX`)
- [ ] Cross-platform testing (Windows, Linux Mint, Fedora, Arch)
- [ ] Keyboard shortcuts customization
- [ ] Multiple account support

## 🤝 Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Ensure `cargo clippy` and `npx tsc --noEmit` pass
4. Commit your changes
5. Open a Pull Request

## 📄 License

This project is licensed under the **MIT License** — see the [LICENSE](LICENSE) file for details.

---

<p align="center">
  Built with ❤️ using <a href="https://v2.tauri.app">Tauri v2</a> · <a href="https://www.rust-lang.org">Rust</a> · <a href="https://www.typescriptlang.org">TypeScript</a>
</p>
