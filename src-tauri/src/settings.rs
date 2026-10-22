// Application Settings Persistence
// Governed by PRD.md Section 8, TRD.md Section 7, and GEMINI.md Section 6.3

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSettings {
    pub autostart_enabled: bool,
    pub monitor_folder_enabled: bool,
    pub monitored_folder_path: Option<String>,
    pub last_imported_archive_fingerprint: Option<String>,
    pub last_imported_archive_name: Option<String>,
    pub last_imported_at_ms: Option<i64>,
    pub last_import_status: Option<String>, // "SUCCESS", "COMPLETED_WITH_NOTES", "FAILED"
    pub last_import_error: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            autostart_enabled: false,
            monitor_folder_enabled: false,
            monitored_folder_path: None,
            last_imported_archive_fingerprint: None,
            last_imported_archive_name: None,
            last_imported_at_ms: None,
            last_import_status: None,
            last_import_error: None,
        }
    }
}

pub fn get_settings_file_path() -> Result<PathBuf, String> {
    if let Ok(override_path) = std::env::var("INSTAGRAM_VIEWER_SETTINGS_PATH") {
        return Ok(PathBuf::from(override_path));
    }

    let local_data = dirs::data_local_dir()
        .ok_or_else(|| "Could not resolve local app data directory".to_string())?;
    Ok(local_data
        .join("InstagramChatHistoryViewer")
        .join("settings.json"))
}

pub fn load_settings_from(path: &Path) -> AppSettings {
    if !path.exists() {
        return AppSettings::default();
    }

    match fs::read_to_string(path) {
        Ok(content) => serde_json::from_str::<AppSettings>(&content).unwrap_or_default(),
        Err(_) => AppSettings::default(),
    }
}

pub fn save_settings_to(settings: &AppSettings, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let json_bytes = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(path, json_bytes).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_settings() -> AppSettings {
    match get_settings_file_path() {
        Ok(path) => load_settings_from(&path),
        Err(_) => AppSettings::default(),
    }
}

pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let path = get_settings_file_path()?;
    save_settings_to(settings, &path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_default_values() {
        let s = AppSettings::default();
        assert!(!s.autostart_enabled);
        assert!(!s.monitor_folder_enabled);
        assert_eq!(s.monitored_folder_path, None);
        assert_eq!(s.last_imported_at_ms, None);
    }

    #[test]
    fn test_settings_save_and_load_roundtrip() {
        let temp_dir = std::env::temp_dir().join(format!("test_settings_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let settings_path = temp_dir.join("test_settings.json");

        let initial = load_settings_from(&settings_path);
        assert_eq!(initial, AppSettings::default());

        let custom = AppSettings {
            autostart_enabled: true,
            monitor_folder_enabled: true,
            monitored_folder_path: Some(r"C:\Users\ZNS\Downloads\InstagramExports".to_string()),
            last_imported_archive_fingerprint: Some("fp_123456".to_string()),
            last_imported_archive_name: Some("instagram_export.zip".to_string()),
            last_imported_at_ms: Some(1700000000000),
            last_import_status: Some("SUCCESS".to_string()),
            last_import_error: None,
        };

        save_settings_to(&custom, &settings_path).expect("Failed to save settings");
        let loaded = load_settings_from(&settings_path);
        assert_eq!(loaded, custom);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
