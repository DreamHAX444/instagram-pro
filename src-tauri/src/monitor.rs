// Local Archive Monitoring Engine
// Governed by PRD.md Section 8, TRD.md Section 6 & 7, and GEMINI.md Section 6

use crate::db::Database;
use crate::importer::{ArchiveSource, Importer};
use crate::settings::{load_settings, save_settings, AppSettings};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MonitorState {
    Disabled,
    Idle,
    Scanning,
    Importing,
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorStatus {
    pub state: MonitorState,
    pub monitored_folder: Option<String>,
    pub last_checked_at_ms: Option<i64>,
    pub message: String,
}

/// Helper to verify a file is fully written and stable (not mid-download / mid-copy).
pub fn is_file_stable(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Skip known partial download / temporary extensions
    if ext == "tmp" || ext == "crdownload" || ext == "part" || ext == "download" {
        return false;
    }

    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };

    if meta.len() == 0 {
        return false;
    }

    // Try opening file in read mode
    match fs::File::open(path) {
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Scans a monitored directory for candidate Instagram archives (.zip or extracted folders).
pub fn scan_for_candidates(folder: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if !folder.is_dir() {
        return candidates;
    }

    // Check if the monitored directory itself is an extracted Instagram export
    if folder.join("messages").join("inbox").exists()
        || folder
            .join("your_instagram_activity")
            .join("messages")
            .join("inbox")
            .exists()
    {
        candidates.push(folder.to_path_buf());
        return candidates;
    }

    if let Ok(entries) = fs::read_dir(folder) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if ext == "zip" && is_file_stable(&path) {
                    candidates.push(path);
                }
            } else if path.is_dir() {
                // Check if subdirectory is an extracted export
                if path.join("messages").join("inbox").exists()
                    || path
                        .join("your_instagram_activity")
                        .join("messages")
                        .join("inbox")
                        .exists()
                {
                    candidates.push(path);
                }
            }
        }
    }

    candidates
}

/// Executes a single check-and-import pass on the monitored folder.
/// Returns the new monitor status.
pub fn run_monitoring_check(
    db: &mut Database,
    media_root: &Path,
    account_id: &str,
) -> MonitorStatus {
    run_monitoring_check_with_settings(db, media_root, account_id, load_settings())
}

/// Executes a single check-and-import pass with explicit settings (avoids global env races in tests).
pub fn run_monitoring_check_with_settings(
    db: &mut Database,
    media_root: &Path,
    account_id: &str,
    mut settings: AppSettings,
) -> MonitorStatus {
    let now_ms = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    if !settings.monitor_folder_enabled {
        return MonitorStatus {
            state: MonitorState::Disabled,
            monitored_folder: settings.monitored_folder_path,
            last_checked_at_ms: Some(now_ms),
            message: "Local archive monitoring is disabled in Settings.".to_string(),
        };
    }

    let folder_str = match settings.monitored_folder_path.as_deref() {
        Some(f) if !f.trim().is_empty() => f.trim(),
        _ => {
            return MonitorStatus {
                state: MonitorState::Error("No folder selected for monitoring.".to_string()),
                monitored_folder: None,
                last_checked_at_ms: Some(now_ms),
                message: "No folder has been configured. Choose a folder in Settings.".to_string(),
            };
        }
    };

    let folder_path = Path::new(folder_str);
    if !folder_path.exists() || !folder_path.is_dir() {
        return MonitorStatus {
            state: MonitorState::Error(format!(
                "Folder '{}' does not exist or is not a directory.",
                folder_str
            )),
            monitored_folder: Some(folder_str.to_string()),
            last_checked_at_ms: Some(now_ms),
            message: "Configured folder is missing or inaccessible.".to_string(),
        };
    }

    let candidates = scan_for_candidates(folder_path);
    if candidates.is_empty() {
        return MonitorStatus {
            state: MonitorState::Idle,
            monitored_folder: Some(folder_str.to_string()),
            last_checked_at_ms: Some(now_ms),
            message: "Watching folder: No new archives found.".to_string(),
        };
    }

    // Inspect candidates
    for candidate in &candidates {
        let candidate_name = candidate
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "export".to_string());

        let source = match ArchiveSource::open(candidate) {
            Ok(s) => s,
            Err(e) => {
                settings.last_import_error =
                    Some(format!("Error opening '{}': {}", candidate_name, e));
                let _ = save_settings(&settings);
                continue;
            }
        };

        let fingerprint = match source.compute_fingerprint() {
            Ok(fp) => fp,
            Err(e) => {
                settings.last_import_error =
                    Some(format!("Error fingerprinting '{}': {}", candidate_name, e));
                let _ = save_settings(&settings);
                continue;
            }
        };

        // Skip if exact archive was already imported
        if settings.last_imported_archive_fingerprint.as_deref() == Some(&fingerprint) {
            continue;
        }

        // Validate archive candidate
        let summary = match source.discover_and_validate() {
            Ok(s) => s,
            Err(e) => {
                settings.last_import_error =
                    Some(format!("Invalid archive '{}': {}", candidate_name, e));
                let _ = save_settings(&settings);
                continue;
            }
        };

        if summary.conversations.is_empty() {
            continue;
        }

        // Perform import
        let mut importer = match Importer::new(db, candidate, account_id, media_root) {
            Ok(imp) => imp,
            Err(e) => {
                settings.last_import_error = Some(format!("Failed to initialize importer: {}", e));
                let _ = save_settings(&settings);
                continue;
            }
        };

        match importer.run_import() {
            Ok(report) => {
                let status_str = if report.errors.is_empty() {
                    "SUCCESS"
                } else {
                    "COMPLETED_WITH_NOTES"
                };

                settings.last_imported_archive_fingerprint = Some(fingerprint);
                settings.last_imported_archive_name = Some(candidate_name.clone());
                settings.last_imported_at_ms = Some(now_ms);
                settings.last_import_status = Some(status_str.to_string());
                settings.last_import_error = None;
                let _ = save_settings(&settings);

                return MonitorStatus {
                    state: MonitorState::Idle,
                    monitored_folder: Some(folder_str.to_string()),
                    last_checked_at_ms: Some(now_ms),
                    message: format!(
                        "Successfully imported '{}' ({} conversations, {} messages).",
                        candidate_name, report.conversations_imported, report.messages_imported
                    ),
                };
            }
            Err(e) => {
                settings.last_import_error =
                    Some(format!("Import failed on '{}': {}", candidate_name, e));
                settings.last_import_status = Some("FAILED".to_string());
                let _ = save_settings(&settings);
            }
        }
    }

    MonitorStatus {
        state: MonitorState::Idle,
        monitored_folder: Some(folder_str.to_string()),
        last_checked_at_ms: Some(now_ms),
        message: "Watching folder: Up to date.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_file_stable_rejects_temp_extensions() {
        assert!(!is_file_stable(Path::new("download.crdownload")));
        assert!(!is_file_stable(Path::new("archive.part")));
        assert!(!is_file_stable(Path::new("temp.tmp")));
        assert!(!is_file_stable(Path::new("archive.download")));
    }

    #[test]
    fn test_scan_for_candidates_detects_zip_and_dirs() {
        let temp_dir =
            std::env::temp_dir().join(format!("test_monitor_scan_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();
        let folder = &temp_dir;

        // 1. Regular text file - should be ignored
        fs::write(folder.join("notes.txt"), b"some text").unwrap();

        // 2. Temp file - should be ignored
        fs::write(folder.join("export.crdownload"), b"temp data").unwrap();

        // 3. Stable zip file - should be detected
        fs::write(folder.join("instagram_export.zip"), b"fake_zip_bytes").unwrap();

        // 4. Extracted directory structure - should be detected
        let conv_dir = folder
            .join("extracted_export")
            .join("messages")
            .join("inbox");
        fs::create_dir_all(&conv_dir).unwrap();

        let candidates = scan_for_candidates(folder);
        assert_eq!(candidates.len(), 2, "Should detect zip and extracted dir");
        let names: Vec<String> = candidates
            .iter()
            .map(|c| c.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.contains(&"instagram_export.zip".to_string()));
        assert!(names.contains(&"extracted_export".to_string()));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_monitoring_disabled_returns_disabled_state() {
        let temp_dir =
            std::env::temp_dir().join(format!("test_monitor_disabled_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("test.db");
        let media_path = temp_dir.join("media");

        let mut db = Database::open_at(&db_path).unwrap();
        let status = run_monitoring_check_with_settings(
            &mut db,
            &media_path,
            "test_user",
            AppSettings::default(),
        );
        assert_eq!(status.state, MonitorState::Disabled);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_monitoring_missing_folder_reports_error() {
        let temp_dir =
            std::env::temp_dir().join(format!("test_monitor_missing_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();
        let db_path = temp_dir.join("test.db");
        let media_path = temp_dir.join("media");

        let s = AppSettings {
            monitor_folder_enabled: true,
            monitored_folder_path: Some(r"C:\NonExistent_Folder_12345".to_string()),
            ..Default::default()
        };

        let mut db = Database::open_at(&db_path).unwrap();
        let status = run_monitoring_check_with_settings(&mut db, &media_path, "test_user", s);
        match status.state {
            MonitorState::Error(err) => assert!(err.contains("does not exist")),
            other => panic!("Expected Error state, got {:?}", other),
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
