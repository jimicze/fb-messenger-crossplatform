/**
 * Settings window logic.
 * Communicates with Rust backend via Tauri IPC.
 */

interface AppSettings {
  stay_logged_in: boolean;
  zoom_level: number;
}

/**
 * Apply translation strings to all visible text nodes in the settings UI.
 */
function applyTranslations(t: Record<string, string>): void {
  // Page title
  document.title = `${t.settings_title ?? 'Settings'} \u2014 Messenger X`;

  // Main heading
  const h1 = document.querySelector('h1');
  if (h1) h1.textContent = t.settings_title ?? 'Settings';

  // Section headings — match by current (English) text so this is safe to
  // call even after a previous translation pass.
  document.querySelectorAll('h2').forEach((el) => {
    switch (el.textContent?.trim()) {
      case 'Account': el.textContent = t.settings_account ?? 'Account'; break;
      case 'Display': el.textContent = t.settings_display ?? 'Display'; break;
      case 'Data':    el.textContent = t.settings_data    ?? 'Data';    break;
      case 'Updates': el.textContent = t.settings_updates  ?? 'Updates'; break;
      case 'About':   el.textContent = t.settings_about   ?? 'About';   break;
    }
  });

  // "Stay logged in" label — inside <label class="setting-row">
  const stayLoggedInLabel = document.querySelector('label.setting-row span');
  if (stayLoggedInLabel) {
    stayLoggedInLabel.textContent = t.settings_stay_logged_in ?? 'Stay logged in';
  }

  // "Zoom Level" label — inside <div class="setting-row">
  const zoomLabel = document.querySelector('div.setting-row span');
  if (zoomLabel) zoomLabel.textContent = t.settings_zoom_level ?? 'Zoom Level';

  // Logout button
  const logoutBtnEl = document.querySelector('.danger-btn');
  if (logoutBtnEl) logoutBtnEl.textContent = t.settings_logout ?? 'Log out & clear all data';

  // Hint paragraphs: first = logout hint, second = about description
  const hints = document.querySelectorAll('.hint');
  if (hints[0]) {
    hints[0].textContent =
      t.settings_logout_hint ??
      'This will clear your session, cached data, and all settings.';
  }
  if (hints[1]) {
    hints[1].textContent =
      t.settings_about_description ??
      'Cross-platform Messenger client built with Tauri.';
  }

  // Update button
  const checkUpdateBtnEl = document.getElementById('check-update-btn');
  if (checkUpdateBtnEl) {
    checkUpdateBtnEl.textContent = t.settings_check_update ?? 'Check for updates';
  }
}

// Wait for Tauri to be available
async function initSettings(): Promise<void> {
  try {
    const { invoke } = await import('@tauri-apps/api/core');

    // Load and apply translations first so the UI is localised before
    // any user interaction.
    let translations: Record<string, string> = {};
    try {
      translations = await invoke<Record<string, string>>('get_translations');
      applyTranslations(translations);
    } catch (e) {
      console.error('[Settings] Failed to load translations:', e);
    }

    const stayLoggedInCheckbox = document.getElementById('stay-logged-in') as HTMLInputElement;
    const zoomLevelDisplay = document.getElementById('zoom-level') as HTMLSpanElement;
    const zoomInBtn = document.getElementById('zoom-in') as HTMLButtonElement;
    const zoomOutBtn = document.getElementById('zoom-out') as HTMLButtonElement;
    const logoutBtn = document.getElementById('logout-btn') as HTMLButtonElement;

    // Load current settings
    const settings: AppSettings = await invoke('get_settings');
    stayLoggedInCheckbox.checked = settings.stay_logged_in;
    zoomLevelDisplay.textContent = `${Math.round(settings.zoom_level * 100)}%`;

    // Stay logged in toggle
    stayLoggedInCheckbox.addEventListener('change', async () => {
      const currentSettings: AppSettings = await invoke('get_settings');
      currentSettings.stay_logged_in = stayLoggedInCheckbox.checked;
      await invoke('save_settings', { settings: currentSettings });
    });

    // Zoom controls
    zoomInBtn.addEventListener('click', async () => {
      const currentSettings: AppSettings = await invoke('get_settings');
      const newZoom = Math.min(3.0, currentSettings.zoom_level + 0.1);
      await invoke('set_zoom', { level: newZoom });
      zoomLevelDisplay.textContent = `${Math.round(newZoom * 100)}%`;
    });

    zoomOutBtn.addEventListener('click', async () => {
      const currentSettings: AppSettings = await invoke('get_settings');
      const newZoom = Math.max(0.5, currentSettings.zoom_level - 0.1);
      await invoke('set_zoom', { level: newZoom });
      zoomLevelDisplay.textContent = `${Math.round(newZoom * 100)}%`;
    });

    // Logout button — use translated confirmation message when available.
    logoutBtn.addEventListener('click', async () => {
      const confirmMsg =
        translations.settings_logout_confirm ??
        'Are you sure you want to log out and clear all data?';
      if (confirm(confirmMsg)) {
        await invoke('clear_all_data');
        // Close settings window
        const { getCurrentWindow } = await import('@tauri-apps/api/window');
        const currentWindow = getCurrentWindow();
        await currentWindow.close();
      }
    });

    // --- Update section ---
    const checkUpdateBtn = document.getElementById('check-update-btn') as HTMLButtonElement;
    const installUpdateBtn = document.getElementById('install-update-btn') as HTMLButtonElement;
    const updateStatus = document.getElementById('update-status') as HTMLDivElement;

    let pendingUpdate: any = null;

    checkUpdateBtn.addEventListener('click', async () => {
      checkUpdateBtn.disabled = true;
      updateStatus.className = 'update-status';
      updateStatus.textContent = translations.settings_checking ?? 'Checking…';

      try {
        const { check } = await import('@tauri-apps/plugin-updater');
        const update = await check();

        if (update) {
          pendingUpdate = update;
          const versionText = (translations.settings_update_available ?? 'Update available: v{}')
            .replace('{}', update.version);
          updateStatus.textContent = versionText;
          updateStatus.className = 'update-status info';
          installUpdateBtn.style.display = 'block';
          installUpdateBtn.textContent =
            translations.settings_install_restart ?? 'Install & Restart';
          checkUpdateBtn.textContent =
            translations.settings_check_update ?? 'Check for updates';
          checkUpdateBtn.disabled = false;
        } else {
          updateStatus.textContent =
            translations.settings_no_update ?? "You're up to date!";
          updateStatus.className = 'update-status success';
          checkUpdateBtn.textContent =
            translations.settings_check_update ?? 'Check for updates';
          checkUpdateBtn.disabled = false;
        }
      } catch (e) {
        console.error('[Settings] Update check failed:', e);
        updateStatus.textContent =
          translations.settings_update_error ?? 'Update check failed';
        updateStatus.className = 'update-status error';
        checkUpdateBtn.textContent =
          translations.settings_check_update ?? 'Check for updates';
        checkUpdateBtn.disabled = false;
      }
    });

    installUpdateBtn.addEventListener('click', async () => {
      if (!pendingUpdate) return;
      installUpdateBtn.disabled = true;
      checkUpdateBtn.disabled = true;
      updateStatus.textContent =
        translations.settings_update_downloading ?? 'Downloading update…';
      updateStatus.className = 'update-status info';

      try {
        await pendingUpdate.downloadAndInstall();
        updateStatus.textContent =
          translations.settings_update_ready ?? 'Update ready — restart to apply';
        updateStatus.className = 'update-status success';
        const { relaunch } = await import('@tauri-apps/plugin-process');
        await relaunch();
      } catch (e) {
        console.error('[Settings] Update install failed:', e);
        updateStatus.textContent =
          translations.settings_update_error ?? 'Update check failed';
        updateStatus.className = 'update-status error';
        installUpdateBtn.disabled = false;
        checkUpdateBtn.disabled = false;
      }
    });

  } catch (error) {
    console.error('[Settings] Failed to initialize:', error);
  }
}

// Initialize when DOM is ready
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', () => initSettings());
} else {
  initSettings();
}
