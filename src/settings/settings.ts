/**
 * Settings window logic.
 * Communicates with Rust backend via Tauri IPC.
 */

interface AppSettings {
  stay_logged_in: boolean;
  zoom_level: number;
}

// Wait for Tauri to be available
async function initSettings(): Promise<void> {
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    
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

    // Logout button
    logoutBtn.addEventListener('click', async () => {
      if (confirm('Are you sure you want to log out and clear all data?')) {
        await invoke('clear_all_data');
        // Close settings window
        const { getCurrentWindow } = await import('@tauri-apps/api/window');
        const currentWindow = getCurrentWindow();
        await currentWindow.close();
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
