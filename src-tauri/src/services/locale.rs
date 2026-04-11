//! Locale detection and translation service.
//! Detects the system language and provides translated strings.

/// Supported application locales.
const SUPPORTED_LOCALES: &[&str] = &["en", "cs"];

/// Default fallback locale.
const DEFAULT_LOCALE: &str = "en";

/// Detects the system locale and returns the best matching supported locale code.
///
/// Uses the `sys_locale` crate to read the OS language setting, then matches
/// against supported locales. Falls back to `"en"` if no match is found.
pub fn detect_locale() -> String {
    let sys = sys_locale::get_locale().unwrap_or_else(|| DEFAULT_LOCALE.to_string());
    // sys might be "cs-CZ", "en-US", etc. — extract the language prefix.
    let lang = sys
        .split('-')
        .next()
        .or_else(|| sys.split('_').next())
        .unwrap_or(DEFAULT_LOCALE);

    if SUPPORTED_LOCALES.contains(&lang) {
        lang.to_string()
    } else {
        DEFAULT_LOCALE.to_string()
    }
}

/// Translation strings for the application UI.
///
/// Each field corresponds to a user-visible string in the native UI
/// (tray, loading screen, offline banner, settings window, etc.).
#[derive(Debug, Clone)]
pub struct Translations {
    // Tray
    /// Default tray tooltip (no unread messages).
    pub tray_tooltip: String,

    // Loading screen
    /// Loading screen body text shown when offline.
    pub loading_offline: String,

    // Offline banner
    /// Text displayed in the inline offline-mode banner.
    pub offline_banner: String,

    // Settings window
    /// "Stay logged in" toggle label.
    pub settings_stay_logged_in: String,
    /// "Zoom Level" label.
    pub settings_zoom_level: String,
    /// Logout button label.
    pub settings_logout: String,

    // Update section
    /// "Check for updates" menu item label.
    pub settings_check_update: String,
    /// Update available notification message with `{}` placeholder for version.
    pub settings_update_available: String,
    /// "Update ready" notification message.
    pub settings_update_ready: String,
    /// "No update available" notification message.
    pub settings_no_update: String,
    /// "Update check failed" notification message.
    pub settings_update_error: String,

    // Tray context menu
    /// "Show Window" tray menu item label.
    pub tray_show: String,
    /// "Quit" tray menu item label.
    pub tray_quit: String,

    // Notifications section
    /// "Enable notifications" toggle label.
    pub settings_notifications_enabled: String,
    /// "Notification sound" toggle label.
    pub settings_notification_sound: String,

    // Startup section
    /// "Start at login" toggle label.
    pub settings_autostart: String,
    /// "Start minimized" toggle label.
    pub settings_start_minimized: String,
}

/// Returns the translation strings for the given locale code.
///
/// Falls back to English if the locale is not supported.
pub fn get_translations(locale: &str) -> Translations {
    match locale {
        "cs" => czech(),
        _ => english(),
    }
}

/// English translation strings.
fn english() -> Translations {
    Translations {
        tray_tooltip: "Messenger X".to_string(),
        loading_offline: "No internet connection. Waiting to reconnect\u{2026}".to_string(),
        offline_banner: "Offline Mode \u{2014} Viewing cached content".to_string(),
        settings_stay_logged_in: "Stay logged in".to_string(),
        settings_zoom_level: "Zoom Level".to_string(),
        settings_logout: "Log out & clear all data".to_string(),
        settings_check_update: "Check for updates".to_string(),
        settings_update_available: "Update available: v{}".to_string(),
        settings_update_ready: "Update ready — restart to apply".to_string(),
        settings_no_update: "You're up to date!".to_string(),
        settings_update_error: "Update check failed".to_string(),
        tray_show: "Show Window".to_string(),
        tray_quit: "Quit".to_string(),
        settings_notifications_enabled: "Enable notifications".to_string(),
        settings_notification_sound: "Notification sound".to_string(),
        settings_autostart: "Start at login".to_string(),
        settings_start_minimized: "Start minimized to tray".to_string(),
    }
}

/// Czech translation strings.
fn czech() -> Translations {
    Translations {
        tray_tooltip: "Messenger X".to_string(),
        loading_offline: "\u{017d}\u{00e1}dn\u{00e9} p\u{0159}ipojen\u{00ed} k internetu. \u{010c}ek\u{00e1}m na obnoven\u{00ed}\u{2026}".to_string(),
        offline_banner: "Offline re\u{017e}im \u{2014} Zobrazuji ulo\u{017e}en\u{00fd} obsah"
            .to_string(),
        settings_stay_logged_in: "Z\u{016f}stat p\u{0159}ihl\u{00e1}\u{0161}en/a".to_string(),
        settings_zoom_level: "\u{00da}rove\u{0148} p\u{0159}ibl\u{00ed}\u{017e}en\u{00ed}"
            .to_string(),
        settings_logout: "Odhl\u{00e1}sit se a smazat v\u{0161}echna data".to_string(),
        settings_check_update: "Zkontrolovat aktualizace".to_string(),
        settings_update_available: "Dostupn\u{00e1} aktualizace: v{}".to_string(),
        settings_update_ready: "Aktualizace p\u{0159}ipravena \u{2014} restartujte pro aplikaci".to_string(),
        settings_no_update: "M\u{00e1}te nejnov\u{011b}j\u{0161}\u{00ed} verzi!".to_string(),
        settings_update_error: "Kontrola aktualizac\u{00ed} selhala".to_string(),
        tray_show: "Zobrazit okno".to_string(),
        tray_quit: "Ukon\u{010d}it".to_string(),
        settings_notifications_enabled: "Povolit ozn\u{00e1}men\u{00ed}".to_string(),
        settings_notification_sound: "Zvuk ozn\u{00e1}men\u{00ed}".to_string(),
        settings_autostart: "Spustit p\u{0159}i p\u{0159}ihl\u{00e1}\u{0161}en\u{00ed}".to_string(),
        settings_start_minimized: "Spustit minimalizovan\u{011b}".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_translations_english_returns_all_fields() {
        let t = get_translations("en");
        assert_eq!(t.tray_tooltip, "Messenger X");
        assert_eq!(t.settings_check_update, "Check for updates");
        assert!(!t.offline_banner.is_empty());
        assert!(!t.loading_offline.is_empty());
    }

    #[test]
    fn test_get_translations_czech_returns_all_fields() {
        let t = get_translations("cs");
        assert_ne!(t.settings_check_update, "Check for updates"); // Czech should differ
        assert!(!t.tray_tooltip.is_empty());
        assert!(!t.offline_banner.is_empty());
        assert!(!t.settings_check_update.is_empty());
    }

    #[test]
    fn test_unknown_locale_falls_back_to_english() {
        let en = get_translations("en");
        let unknown = get_translations("xx");
        assert_eq!(en.offline_banner, unknown.offline_banner);
        assert_eq!(en.settings_check_update, unknown.settings_check_update);
    }

    #[test]
    fn test_detect_locale_returns_supported() {
        let locale = detect_locale();
        assert!(
            SUPPORTED_LOCALES.contains(&locale.as_str()),
            "detect_locale() returned unsupported locale: {locale}"
        );
    }

    #[test]
    fn test_all_translations_have_no_empty_strings() {
        for lang in SUPPORTED_LOCALES {
            let t = get_translations(lang);
            let fields = [
                &t.tray_tooltip,
                &t.loading_offline,
                &t.offline_banner,
                &t.settings_stay_logged_in,
                &t.settings_zoom_level,
                &t.settings_logout,
                &t.settings_check_update,
                &t.settings_update_available,
                &t.settings_update_ready,
                &t.settings_no_update,
                &t.settings_update_error,
                &t.tray_show,
                &t.tray_quit,
                &t.settings_notifications_enabled,
                &t.settings_notification_sound,
                &t.settings_autostart,
                &t.settings_start_minimized,
            ];
            for (i, field) in fields.iter().enumerate() {
                assert!(
                    !field.is_empty(),
                    "Locale '{lang}': translation field index {i} is empty"
                );
            }
        }
    }

    #[test]
    fn test_update_available_template_has_placeholder() {
        for lang in SUPPORTED_LOCALES {
            let t = get_translations(lang);
            assert!(
                t.settings_update_available.contains("{}"),
                "Locale '{lang}': settings_update_available missing {{}} placeholder"
            );
        }
    }
}
