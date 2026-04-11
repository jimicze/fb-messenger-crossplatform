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
    /// Tray tooltip format string with `{}` placeholder for the unread count.
    pub tray_tooltip_unread: String,

    // Loading screen
    /// Loading screen window title.
    pub loading_title: String,
    /// Loading screen body text shown while the page is loading.
    pub loading_text: String,
    /// Loading screen body text shown when offline.
    pub loading_offline: String,

    // Offline banner
    /// Text displayed in the inline offline-mode banner.
    pub offline_banner: String,

    // Settings window
    /// Settings window title and heading.
    pub settings_title: String,
    /// "Account" section heading.
    pub settings_account: String,
    /// "Stay logged in" toggle label.
    pub settings_stay_logged_in: String,
    /// "Display" section heading.
    pub settings_display: String,
    /// "Zoom Level" label.
    pub settings_zoom_level: String,
    /// "Data" section heading.
    pub settings_data: String,
    /// Logout button label.
    pub settings_logout: String,
    /// Hint text below the logout button.
    pub settings_logout_hint: String,
    /// Confirmation dialog message for logout.
    pub settings_logout_confirm: String,
    /// "About" section heading.
    pub settings_about: String,
    /// About section description text.
    pub settings_about_description: String,

    // Update section
    pub settings_updates: String,
    pub settings_check_update: String,
    pub settings_checking: String,
    pub settings_update_available: String,
    pub settings_update_downloading: String,
    pub settings_update_ready: String,
    pub settings_no_update: String,
    pub settings_update_error: String,
    pub settings_install_restart: String,

    // Tray context menu
    /// "Show Window" tray menu item label.
    pub tray_show: String,
    /// "Settings" tray menu item label.
    pub tray_settings: String,
    /// "Quit" tray menu item label.
    pub tray_quit: String,
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
        tray_tooltip_unread: "Messenger X ({})".to_string(),
        loading_title: "Messenger X".to_string(),
        loading_text: "Loading...".to_string(),
        loading_offline: "No internet connection. Waiting to reconnect\u{2026}".to_string(),
        offline_banner: "Offline Mode \u{2014} Viewing cached content".to_string(),
        settings_title: "Settings".to_string(),
        settings_account: "Account".to_string(),
        settings_stay_logged_in: "Stay logged in".to_string(),
        settings_display: "Display".to_string(),
        settings_zoom_level: "Zoom Level".to_string(),
        settings_data: "Data".to_string(),
        settings_logout: "Log out & clear all data".to_string(),
        settings_logout_hint: "This will clear your session, cached data, and all settings."
            .to_string(),
        settings_logout_confirm: "Are you sure you want to log out and clear all data?".to_string(),
        settings_about: "About".to_string(),
        settings_about_description: "Cross-platform Messenger client built with Tauri.".to_string(),
        settings_updates: "Updates".to_string(),
        settings_check_update: "Check for updates".to_string(),
        settings_checking: "Checking…".to_string(),
        settings_update_available: "Update available: v{}".to_string(),
        settings_update_downloading: "Downloading update…".to_string(),
        settings_update_ready: "Update ready — restart to apply".to_string(),
        settings_no_update: "You're up to date!".to_string(),
        settings_update_error: "Update check failed".to_string(),
        settings_install_restart: "Install & Restart".to_string(),
        tray_show: "Show Window".to_string(),
        tray_settings: "Settings".to_string(),
        tray_quit: "Quit".to_string(),
    }
}

/// Czech translation strings.
fn czech() -> Translations {
    Translations {
        tray_tooltip: "Messenger X".to_string(),
        tray_tooltip_unread: "Messenger X ({})".to_string(),
        loading_title: "Messenger X".to_string(),
        loading_text: "Na\u{010d}ít\u{00e1}n\u{00ed}...".to_string(),
        loading_offline: "\u{017d}\u{00e1}dn\u{00e9} p\u{0159}ipojen\u{00ed} k internetu. \u{010c}ek\u{00e1}m na obnoven\u{00ed}\u{2026}".to_string(),
        offline_banner: "Offline re\u{017e}im \u{2014} Zobrazuji ulo\u{017e}en\u{00fd} obsah"
            .to_string(),
        settings_title: "Nastaven\u{00ed}".to_string(),
        settings_account: "\u{00da}\u{010d}et".to_string(),
        settings_stay_logged_in: "Z\u{016f}stat p\u{0159}ihl\u{00e1}\u{0161}en/a".to_string(),
        settings_display: "Zobrazen\u{00ed}".to_string(),
        settings_zoom_level: "\u{00da}rove\u{0148} p\u{0159}ibl\u{00ed}\u{017e}en\u{00ed}"
            .to_string(),
        settings_data: "Data".to_string(),
        settings_logout: "Odhl\u{00e1}sit se a smazat v\u{0161}echna data".to_string(),
        settings_logout_hint:
            "T\u{00ed}m se vyma\u{017e}e va\u{0161}e relace, mezipam\u{011b}\u{0165} a ve\u{0161}ker\u{00e1} nastaven\u{00ed}."
                .to_string(),
        settings_logout_confirm:
            "Opravdu se chcete odhl\u{00e1}sit a smazat v\u{0161}echna data?".to_string(),
        settings_about: "O aplikaci".to_string(),
        settings_about_description:
            "Multiplatformn\u{00ed} Messenger klient postaven\u{00fd} na Tauri.".to_string(),
        settings_updates: "Aktualizace".to_string(),
        settings_check_update: "Zkontrolovat aktualizace".to_string(),
        settings_checking: "Kontroluji\u{2026}".to_string(),
        settings_update_available: "Dostupn\u{00e1} aktualizace: v{}".to_string(),
        settings_update_downloading: "Stahov\u{00e1}n\u{00ed} aktualizace\u{2026}".to_string(),
        settings_update_ready: "Aktualizace p\u{0159}ipravena \u{2014} restartujte pro aplikaci".to_string(),
        settings_no_update: "M\u{00e1}te nejnov\u{011b}j\u{0161}\u{00ed} verzi!".to_string(),
        settings_update_error: "Kontrola aktualizac\u{00ed} selhala".to_string(),
        settings_install_restart: "Nainstalovat a restartovat".to_string(),
        tray_show: "Zobrazit okno".to_string(),
        tray_settings: "Nastaven\u{00ed}".to_string(),
        tray_quit: "Ukon\u{010d}it".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_translations_english_returns_all_fields() {
        let t = get_translations("en");
        assert_eq!(t.tray_tooltip, "Messenger X");
        assert_eq!(t.settings_title, "Settings");
        assert_eq!(t.settings_updates, "Updates");
        assert_eq!(t.settings_check_update, "Check for updates");
        assert_eq!(t.settings_install_restart, "Install & Restart");
        assert!(!t.offline_banner.is_empty());
        assert!(!t.loading_text.is_empty());
    }

    #[test]
    fn test_get_translations_czech_returns_all_fields() {
        let t = get_translations("cs");
        assert_ne!(t.settings_title, "Settings"); // Czech should differ
        assert!(!t.tray_tooltip.is_empty());
        assert!(!t.offline_banner.is_empty());
        assert!(!t.settings_updates.is_empty());
        assert!(!t.settings_check_update.is_empty());
    }

    #[test]
    fn test_unknown_locale_falls_back_to_english() {
        let en = get_translations("en");
        let unknown = get_translations("xx");
        assert_eq!(en.settings_title, unknown.settings_title);
        assert_eq!(en.offline_banner, unknown.offline_banner);
        assert_eq!(en.settings_updates, unknown.settings_updates);
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
                &t.tray_tooltip_unread,
                &t.loading_title,
                &t.loading_text,
                &t.loading_offline,
                &t.offline_banner,
                &t.settings_title,
                &t.settings_account,
                &t.settings_stay_logged_in,
                &t.settings_display,
                &t.settings_zoom_level,
                &t.settings_data,
                &t.settings_logout,
                &t.settings_logout_hint,
                &t.settings_logout_confirm,
                &t.settings_about,
                &t.settings_about_description,
                &t.settings_updates,
                &t.settings_check_update,
                &t.settings_checking,
                &t.settings_update_available,
                &t.settings_update_downloading,
                &t.settings_update_ready,
                &t.settings_no_update,
                &t.settings_update_error,
                &t.settings_install_restart,
                &t.tray_show,
                &t.tray_settings,
                &t.tray_quit,
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
