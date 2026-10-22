pub mod db;
pub mod importer;
pub mod media;
pub mod queries;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationListResponse {
    pub total_count: u64,
    pub conversations: Vec<queries::ConversationSummary>,
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
        phase: "Phase 4: Dashboard, Conversation Viewer & Chat Interface".to_string(),
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
    let media_dir = get_media_root_dir()?;
    let account = account_id.unwrap_or_else(|| "default_user".to_string());
    let mut importer =
        importer::Importer::new(&mut db, path, &account, &media_dir).map_err(|e| e.to_string())?;
    importer.run_import().map_err(|e| e.to_string())
}

#[tauri::command]
fn list_conversations(
    query: Option<String>,
    filter: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<ConversationListResponse, String> {
    let db = db::Database::open_default().map_err(|e| e.to_string())?;
    let (conversations, total_count) = db
        .list_conversations(
            query.as_deref(),
            filter.as_deref(),
            limit.unwrap_or(50),
            offset.unwrap_or(0),
        )
        .map_err(|e| e.to_string())?;

    Ok(ConversationListResponse {
        total_count,
        conversations,
    })
}

#[tauri::command]
fn get_conversation_stats(conversation_id: String) -> Result<queries::ConversationStats, String> {
    let db = db::Database::open_default().map_err(|e| e.to_string())?;
    db.get_conversation_stats(&conversation_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_messages(
    conversation_id: String,
    limit: Option<u32>,
    offset: Option<u32>,
    filter: Option<String>,
) -> Result<queries::PaginatedMessages, String> {
    let db = db::Database::open_default().map_err(|e| e.to_string())?;
    let media_dir = get_media_root_dir()?;
    db.get_messages(
        &conversation_id,
        limit.unwrap_or(50),
        offset.unwrap_or(0),
        filter.as_deref(),
        &media_dir,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn search_messages(
    query: String,
    conversation_id: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<queries::SearchResults, String> {
    let db = db::Database::open_default().map_err(|e| e.to_string())?;
    let media_dir = get_media_root_dir()?;
    db.search_messages(
        &query,
        conversation_id.as_deref(),
        limit.unwrap_or(50),
        offset.unwrap_or(0),
        &media_dir,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn pick_archive_file() -> Result<Option<String>, String> {
    let file = rfd::AsyncFileDialog::new()
        .add_filter("Instagram Archive ZIP", &["zip"])
        .set_title("Select Instagram Data Archive (.zip)")
        .pick_file()
        .await;

    Ok(file.map(|f| f.path().to_string_lossy().to_string()))
}

#[tauri::command]
async fn pick_archive_folder() -> Result<Option<String>, String> {
    let folder = rfd::AsyncFileDialog::new()
        .set_title("Select Extracted Instagram Archive Folder")
        .pick_folder()
        .await;

    Ok(folder.map(|f| f.path().to_string_lossy().to_string()))
}

#[tauri::command]
async fn locate_missing_media(
    attachment_id: String,
) -> Result<Option<queries::AttachmentView>, String> {
    let file = rfd::AsyncFileDialog::new()
        .set_title("Locate Missing Media File")
        .pick_file()
        .await;

    let selected_file = match file {
        Some(f) => f,
        None => return Ok(None),
    };

    let selected_path = selected_file.path();
    if !selected_path.exists() {
        return Err("Selected file does not exist".to_string());
    }

    let mut db = db::Database::open_default().map_err(|e| e.to_string())?;
    let media_root = get_media_root_dir()?;

    // Fetch existing attachment details
    let (msg_id, original_uri, attach_type): (String, Option<String>, String) = db
        .conn()
        .query_row(
            "SELECT message_id, original_uri, attachment_type FROM attachments WHERE id = ?1",
            rusqlite::params![attachment_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| format!("Attachment not found: {}", e))?;

    let conv_id: String = db
        .conn()
        .query_row(
            "SELECT conversation_id FROM messages WHERE id = ?1",
            rusqlite::params![msg_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("Parent message not found: {}", e))?;

    // Copy to destination
    let file_name = selected_path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "media.dat".to_string());

    let subfolder = match attach_type.as_str() {
        "audio" => "audio",
        "video" => "videos",
        _ => "images",
    };

    let rel_storage_path = format!(
        "{}/{}/{}_{}",
        conv_id,
        subfolder,
        uuid::Uuid::new_v4(),
        file_name
    );
    let dest_path = media_root.join(&rel_storage_path);

    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Compute checksum while copying
    let bytes = std::fs::read(selected_path).map_err(|e| e.to_string())?;
    let size_bytes = bytes.len() as i64;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let checksum = hex::encode(hasher.finalize());

    std::fs::write(&dest_path, &bytes).map_err(|e| e.to_string())?;

    let mime_type = media::get_mime_type(selected_path).to_string();

    db.conn_mut()
        .execute(
            "UPDATE attachments SET storage_path = ?1, status = 'LOCAL_COPIED', file_size_bytes = ?2, sha256_checksum = ?3, mime_type = ?4 WHERE id = ?5",
            rusqlite::params![rel_storage_path, size_bytes, checksum, mime_type, attachment_id],
        )
        .map_err(|e| e.to_string())?;

    let media_url = Some(format!(
        "media://localhost/{}",
        rel_storage_path.replace('\\', "/")
    ));

    Ok(Some(queries::AttachmentView {
        id: attachment_id,
        attachment_type: attach_type,
        original_uri,
        storage_path: Some(rel_storage_path),
        media_url,
        mime_type,
        file_size_bytes: Some(size_bytes),
        sha256_checksum: Some(checksum),
        duration_ms: None,
        status: "LOCAL_COPIED".to_string(),
        is_available: true,
    }))
}

pub fn get_media_root_dir() -> Result<PathBuf, String> {
    let dir = dirs::data_local_dir()
        .ok_or_else(|| "Could not resolve local app data directory".to_string())?
        .join("InstagramChatHistoryViewer")
        .join("media");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let media_root = get_media_root_dir().unwrap_or_else(|_| PathBuf::from("media"));
    let media_root_clone = media_root.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .register_uri_scheme_protocol("media", move |_ctx, req| {
            media::handle_media_request(&req, &media_root_clone)
        })
        .invoke_handler(tauri::generate_handler![
            ping,
            get_app_status,
            check_database,
            validate_archive_path,
            import_archive_path,
            list_conversations,
            get_conversation_stats,
            get_messages,
            search_messages,
            pick_archive_file,
            pick_archive_folder,
            locate_missing_media
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
            "Phase 4: Dashboard, Conversation Viewer & Chat Interface"
        );
    }
}
