pub mod db;
pub mod importer;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AppStatus {
    pub app_name: String,
    pub version: String,
    pub architecture: String,
    pub target_os: String,
    pub local_first_mode: bool,
    pub phase: String,
    pub database_ready: bool,
}

#[tauri::command]
fn ping(message: String) -> Result<String, String> {
    Ok(format!("Pong from Rust backend: received '{}'", message))
}

#[tauri::command]
fn get_app_status() -> Result<AppStatus, String> {
    let db_ready = db::Database::open_default().is_ok();
    Ok(AppStatus {
        app_name: "Instagram Chat History Viewer".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        architecture: std::env::consts::ARCH.to_string(),
        target_os: std::env::consts::OS.to_string(),
        local_first_mode: true,
        phase: "Phase 3: Historical Instagram Archive Importer".to_string(),
        database_ready: db_ready,
    })
}

#[tauri::command]
fn check_database() -> Result<String, String> {
    let db = db::Database::open_default().map_err(|e| e.to_string())?;
    db.check_integrity().map_err(|e| e.to_string())?;
    let version = db.get_schema_version().map_err(|e| e.to_string())?;
    Ok(format!("SQLite Database OK (Schema Version: {})", version))
}

#[tauri::command]
fn validate_archive_path(archive_path: String) -> Result<importer::ValidationSummary, String> {
    let trimmed = archive_path.trim();
    if trimmed.is_empty() {
        return Err("Archive path cannot be empty".to_string());
    }
    let path = std::path::Path::new(trimmed);
    let source = importer::ArchiveSource::open(path).map_err(|e| e.to_string())?;
    source.discover_and_validate().map_err(|e| e.to_string())
}

#[tauri::command]
fn import_archive_path(
    archive_path: String,
    account_id: Option<String>,
) -> Result<importer::ImportReport, String> {
    let trimmed = archive_path.trim();
    if trimmed.is_empty() {
        return Err("Archive path cannot be empty".to_string());
    }
    let path = std::path::Path::new(trimmed);
    let mut db = db::Database::open_default().map_err(|e| e.to_string())?;
    let media_dir = dirs::data_local_dir()
        .ok_or_else(|| "Could not resolve local app data directory".to_string())?
        .join("InstagramChatHistoryViewer")
        .join("media");
    let account = account_id.unwrap_or_else(|| "default_user".to_string());
    let mut importer =
        importer::Importer::new(&mut db, path, &account, &media_dir).map_err(|e| e.to_string())?;
    importer.run_import().map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            ping,
            get_app_status,
            check_database,
            validate_archive_path,
            import_archive_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ping_command() {
        let response = ping("test payload".to_string());
        assert!(response.is_ok());
        assert_eq!(
            response.unwrap(),
            "Pong from Rust backend: received 'test payload'"
        );
    }

    #[test]
    fn test_app_status_command() {
        let status = get_app_status();
        assert!(status.is_ok());
        let status = status.unwrap();
        assert_eq!(status.app_name, "Instagram Chat History Viewer");
        assert!(status.local_first_mode);
        assert_eq!(
            status.phase,
            "Phase 3: Historical Instagram Archive Importer"
        );
    }
}
