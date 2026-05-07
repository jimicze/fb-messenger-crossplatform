// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // -----------------------------------------------------------------------
    // macOS-only CLI notification helper mode.
    //
    // When `npm run tauri dev` spawns us via:
    //
    //   messengerx --notify <title> <body> [silent]
    //
    // we are running inside the debug `.app` bundle (so
    // `UNUserNotificationCenter` is available), dispatch the notification,
    // wait a few seconds for async callbacks/diagnostics, then exit — without opening any
    // window.
    //
    // This path MUST NOT delegate back to the subprocess helper; it calls
    // `UNUserNotificationCenter` directly (guarded by `is_running_in_macos_app_bundle`
    // inside the service, but we also pass an explicit flag so the service
    // skips the osascript/subprocess branch entirely).
    // -----------------------------------------------------------------------
    #[cfg(target_os = "macos")]
    {
        let args: Vec<String> = std::env::args()
            .filter(|arg| !arg.starts_with("-psn_"))
            .collect();
        let is_notify_helper = std::env::var("MESSENGERX_NOTIFY_HELPER").as_deref() == Ok("1");
        // Accept:  messengerx --notify <title> <body>
        //          messengerx --notify <title> <body> silent
        // macOS may inject `-psn_*` process-serial-number arguments when a
        // bundle binary is launched, so search rather than requiring argv[1].
        if let Some(notify_pos) = args.iter().position(|arg| arg == "--notify") {
            if args.len() <= notify_pos + 2 {
                eprintln!("[MessengerX][NotifyHelper] malformed --notify args: {args:?}; exiting");
                std::process::exit(2);
            }
            let title = &args[notify_pos + 1];
            let body = &args[notify_pos + 2];
            let silent = args
                .get(notify_pos + 3)
                .map(|s| s == "silent")
                .unwrap_or(false);

            eprintln!(
                "[MessengerX][NotifyHelper] CLI notify mode: title={title:?} silent={silent}"
            );

            match messengerx_lib::dispatch_notification_from_bundle(title, body, silent) {
                Ok(()) => {
                    eprintln!("[MessengerX][NotifyHelper] notification dispatched successfully");
                }
                Err(e) => {
                    eprintln!("[MessengerX][NotifyHelper] notification failed: {e}");
                    std::process::exit(1);
                }
            }

            // Give the async UNUserNotificationCenter completion handler and
            // delayed delivered-list diagnostic check time to fire before exit.
            std::thread::sleep(std::time::Duration::from_secs(4));
            std::process::exit(0);
        }

        // If the parent marked this as a notification-helper launch but args
        // were stripped or malformed, do NOT start the full Tauri app.  Return
        // non-zero so the parent falls back to osascript instead of hanging or
        // opening/crashing a second app instance.
        if is_notify_helper {
            eprintln!(
                "[MessengerX][NotifyHelper] helper env set but --notify args missing; args={args:?}; exiting"
            );
            std::process::exit(2);
        }
    }

    // On Linux, remove the XDG startup-notification environment variables
    // before WRY/GTK reads them.
    //
    // On X11 this prevents GTK3 from sending a `_NET_STARTUP_INFO` completion
    // message, which stops GNOME Shell from showing a "Ready 'AppName'" toast.
    //
    // On Wayland, GNOME Shell tracks startup via PID / WM_CLASS correlation
    // independently of these env vars, so this call alone is not sufficient.
    // The primary fix for Wayland is `StartupNotify=false` in the bundled
    // `.desktop` file (set via `bundle.linux.deb.desktopTemplate` in
    // tauri.conf.json), which tells GNOME Shell not to track this app's
    // startup at all.
    //
    // Removing the vars is safe: they are only consumed by GTK for startup
    // animation hints and play no role in networking, IPC, or window management.
    #[cfg(target_os = "linux")]
    suppress_gnome_startup_notification();

    messengerx_lib::run()
}
