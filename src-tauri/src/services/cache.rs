//! HTML snapshot caching service.
//! Saves periodic snapshots of the page for offline viewing.
//! At most `MAX_SNAPSHOTS` snapshots are retained; older ones are pruned automatically.

use crate::commands::SnapshotData;
use chrono::Utc;
use std::cmp::Reverse;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// Maximum number of snapshots to retain on disk.
const MAX_SNAPSHOTS: usize = 3;

/// Returns the directory where snapshot JSON files are stored.
///
/// # Errors
/// Returns an error string if the app-data directory cannot be resolved.
pub fn get_snapshots_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {e}"))?;
    Ok(dir.join("snapshots"))
}

/// Persist an HTML snapshot to disk and rotate old snapshots.
///
/// The snapshot is stored as a JSON file named by Unix timestamp so that
/// lexicographic order equals chronological order.
///
/// # Errors
/// Returns an error string if the directory cannot be created or the file cannot be written.
pub fn save_snapshot(app: &AppHandle, html: String, url: String) -> Result<(), String> {
    let dir = get_snapshots_dir(app)?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create snapshots directory: {e}"))?;

    let now = Utc::now();
    let snapshot = SnapshotData {
        html,
        url,
        timestamp: now.to_rfc3339(),
    };

    let filename = format!("snapshot_{}.json", now.timestamp());
    let path = dir.join(&filename);
    let data = serde_json::to_string(&snapshot)
        .map_err(|e| format!("Failed to serialise snapshot: {e}"))?;
    std::fs::write(&path, &data).map_err(|e| format!("Failed to write snapshot file: {e}"))?;

    rotate_snapshots(app)
}

/// Load the most recent snapshot from disk.
///
/// Returns `Ok(None)` when no snapshots are stored yet.
///
/// # Errors
/// Returns an error string if a snapshot file cannot be read or deserialised.
pub fn load_latest_snapshot(app: &AppHandle) -> Result<Option<SnapshotData>, String> {
    let dir = get_snapshots_dir(app)?;
    if !dir.exists() {
        return Ok(None);
    }

    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("Failed to read snapshots directory: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "json")
                .unwrap_or(false)
        })
        .collect();

    // Newest filename (largest timestamp) first.
    entries.sort_by_key(|e| Reverse(e.file_name()));

    match entries.first() {
        None => Ok(None),
        Some(entry) => {
            let data = std::fs::read_to_string(entry.path())
                .map_err(|e| format!("Failed to read snapshot file: {e}"))?;
            let snapshot: SnapshotData = serde_json::from_str(&data)
                .map_err(|e| format!("Failed to deserialise snapshot: {e}"))?;
            Ok(Some(snapshot))
        }
    }
}

/// Remove all stored snapshots from disk.
///
/// # Errors
/// Returns an error string if the directory cannot be removed.
pub fn clear_snapshots(app: &AppHandle) -> Result<(), String> {
    let dir = get_snapshots_dir(app)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("Failed to clear snapshots directory: {e}"))?;
    }
    Ok(())
}

/// Prune snapshot files so that only the newest `MAX_SNAPSHOTS` are kept.
///
/// # Errors
/// Returns an error string if the directory cannot be read.
pub fn rotate_snapshots(app: &AppHandle) -> Result<(), String> {
    let dir = get_snapshots_dir(app)?;
    if !dir.exists() {
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("Failed to read snapshots directory: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "json")
                .unwrap_or(false)
        })
        .collect();

    // Sort newest first.
    entries.sort_by_key(|e| Reverse(e.file_name()));

    // Delete everything beyond the limit.
    for entry in entries.iter().skip(MAX_SNAPSHOTS) {
        let _ = std::fs::remove_file(entry.path());
    }

    Ok(())
}
