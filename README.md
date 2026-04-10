<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Messenger X" width="128" height="128" />
</p>

<h1 align="center">Messenger X</h1>

<p align="center">
  <strong>A lightweight, cross-platform desktop client for Facebook Messenger</strong>
</p>

<p align="center">
  <a href="https://github.com/AdrianLasworkin/fb-messanger-crossplatform/releases"><img src="https://img.shields.io/github/v/release/AdrianLasworkin/fb-messanger-crossplatform?style=flat-square&color=blue&label=Release" alt="Latest Release" /></a>
  <a href="https://github.com/AdrianLasworkin/fb-messanger-crossplatform/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflow/status/AdrianLasworkin/fb-messanger-crossplatform/release.yml?style=flat-square&label=CI" alt="CI Status" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-green?style=flat-square" alt="MIT License" /></a>
  <img src="https://img.shields.io/badge/Tauri-v2-orange?style=flat-square&logo=tauri" alt="Tauri v2" />
  <img src="https://img.shields.io/badge/Rust-2021-brown?style=flat-square&logo=rust" alt="Rust" />
  <img src="https://img.shields.io/badge/TypeScript-5-blue?style=flat-square&logo=typescript" alt="TypeScript" />
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
    <td rowspan="2"><a href="https://github.com/AdrianLasworkin/fb-messanger-crossplatform/releases/latest"><strong>Latest Release →</strong></a></td>
  </tr>
  <tr>
    <td>🍎 <strong>macOS</strong></td>
    <td>Intel x86_64</td>
    <td><code>.dmg</code></td>
  </tr>
  <tr>
    <td>🪟 <strong>Windows</strong></td>
    <td>x64</td>
    <td><code>.exe</code> / <code>.msi</code></td>
    <td rowspan="2"><a href="https://github.com/AdrianLasworkin/fb-messanger-crossplatform/releases/latest"><strong>Latest Release →</strong></a></td>
  </tr>
  <tr>
    <td>🪟 <strong>Windows</strong></td>
    <td>ARM64</td>
    <td><code>.exe</code> / <code>.msi</code></td>
  </tr>
  <tr>
    <td>🐧 <strong>Linux</strong></td>
    <td>x64</td>
    <td><code>.deb</code> / <code>.rpm</code> / <code>.AppImage</code></td>
    <td rowspan="2"><a href="https://github.com/AdrianLasworkin/fb-messanger-crossplatform/releases/latest"><strong>Latest Release →</strong></a></td>
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
    <td width="50%">

#### 🌐 Native WebView Client
Loads `messenger.com` in a system WebView — no bundled browser engine. Uses your OS's native web renderer for minimal resource usage.

#### 🔔 Native Notifications
Web notifications are intercepted and forwarded to your OS notification system. Works with notification centers on all platforms.

#### 🔢 Unread Badge
Parses the unread count from the page title and displays it on your dock/taskbar icon and system tray tooltip.

#### 🔐 Persistent Sessions
Your login session persists across app restarts. Toggle "Stay logged in" in Settings, or log out and wipe all data with one click.

</td>
<td width="50%">

#### 📴 Offline Mode
Automatic HTML snapshots every 60 seconds. When you go offline, the app shows cached content with a non-intrusive banner — no blank screens.

#### 🔍 Zoom Controls
Zoom from 50% to 300% with keyboard shortcuts (`Ctrl/Cmd` + `+`/`-`/`0`). Zoom level persists across sessions.

#### 🪟 Window Management
Window size and position are saved and restored automatically. Default: 1200×800, minimum: 400×300.

#### 🔗 Smart Link Handling
Facebook/Messenger URLs stay in-app. External links open in your default system browser automatically.

</td>
  </tr>
</table>

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
│   ├─ Notifications   │   │                       │
│   ├─ Unread Observer │   Services:               │
│   ├─ Offline Banner  │   ├─ auth.rs    (session) │
│   └─ Zoom Control    │   ├─ cache.rs   (offline) │
│                      │   ├─ network.rs (monitor) │
│        invoke()      │   └─ notification.rs      │
│   ─────────────────► │                           │
│                      │   10 IPC Commands          │
│   ◄───────────────── │   ├─ send_notification    │
│        eval()        │   ├─ update_unread_count  │
│                      │   ├─ get/save_settings    │
│                      │   ├─ save/load_snapshot   │
│                      │   ├─ clear_all_data       │
│                      │   ├─ open_external        │
│                      │   └─ set/get_zoom         │
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
git clone https://github.com/AdrianLasworkin/fb-messanger-crossplatform.git
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
│   ├── scripts/                  #   JS injection stubs (actual JS in lib.rs)
│   │   ├── main.ts
│   │   ├── bridge.ts
│   │   ├── notifications.ts
│   │   ├── unread.ts
│   │   └── offline.ts
│   └── settings/                 #   Settings window
│       ├── index.html
│       ├── settings.ts
│       └── settings.css
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

This triggers builds for **all 7 targets** in parallel and creates a **draft GitHub Release** with all installers attached. Review the draft and publish when ready.

## 🛡️ Security & Privacy

- **No data collection** — Messenger X doesn't phone home or collect any telemetry
- **No API reverse-engineering** — simply wraps the official `messenger.com` web interface
- **Local persistence only** — sessions, settings, and cached snapshots are stored in your OS app data directory
- **Navigation allowlist** — only `messenger.com`, `facebook.com`, `fbcdn.net`, and `fbsbx.com` are loaded in-app; everything else opens in your system browser
- **Hardened runtime** (macOS) enabled for Gatekeeper compatibility

## 🗺️ Roadmap

- [x] **Phase 1** — Core features (WebView, notifications, badges, offline, zoom, settings)
- [ ] Cross-platform testing (Windows, Linux Mint, Fedora, Arch)
- [ ] Code signing (Apple notarization + Windows Authenticode)
- [ ] Auto-update support (Tauri updater plugin)
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
