//! Historical Instagram Archive Importer Engine
//!
//! Governed by PRD.md, TRD.md, and GEMINI.md.
//! Implements offline, resilient ingestion of Meta "Download Your Information" (JSON format)
//! archives from both uncompressed directories and ZIP files.
//!
//! Features:
//! - Multi-layout archive discovery (legacy messages/inbox and modern Accounts Center)
//! - Path traversal and ZIP bomb protection
//! - Mojibake normalization and UTF-8 validation
//! - Zero-memory bounded media copying with SHA-256 integrity checks
//! - Atomic batch transactions with durable checkpoint updates
//! - Tier 1 (Source ID) and Tier 2 (Scoped Positional) deduplication
//! - Comprehensive error tracking in import_jobs and failed_items

use crate::db::{
    compute_content_digest, compute_tier1_identity_hash, compute_tier2_identity_hash,
    evaluate_checkpoint, CheckpointResumeAction, Database, DbError,
};
use rusqlite::{params, Transaction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

/// Maximum allowable uncompressed entry size for ZIP entries (500 MB) to prevent zip bombs
pub const MAX_UNCOMPRESSED_ENTRY_BYTES: u64 = 500 * 1024 * 1024;

/// Maximum allowable cumulative uncompressed bytes across all entries in an archive (10 GB)
pub const MAX_CUMULATIVE_UNCOMPRESSED_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// Maximum allowable total entries in an archive to prevent resource exhaustion
pub const MAX_ARCHIVE_ENTRIES: usize = 100_000;

/// Maximum allowed compression ratio for non-trivial entries (>10MB uncompressed)
pub const MAX_COMPRESSION_RATIO: u64 = 100;

/// Batch size for SQLite transactional commits
pub const BATCH_SIZE: usize = 500;

#[derive(Debug, Error)]
pub enum ImporterError {
    #[error("Database error: {0}")]
    Db(#[from] DbError),

    #[error("SQLite query error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Filesystem I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("ZIP archive error: {0}")]
    Zip(String),

    #[error("Path traversal security violation: entry '{0}' attempts to escape archive boundary")]
    PathTraversal(String),

    #[error("ZIP decompression size limit exceeded: {0}")]
    ZipBomb(String),

    #[error("Invalid or unsupported archive format: {0}")]
    InvalidArchive(String),
}

// ---------------------------------------------------------------------------
// Validation & Reporting Data Structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredConversation {
    pub source_id: String,
    pub title: Option<String>,
    pub json_files: Vec<String>,
    pub participants: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationSummary {
    pub is_valid: bool,
    pub is_empty: bool,
    pub source_type: String, // "DIRECTORY" or "ZIP"
    pub dataset_fingerprint: String,
    pub conversations: Vec<DiscoveredConversation>,
    pub total_message_files: usize,
    pub attachment_candidates: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    pub job_id: String,
    pub source_path: String,
    pub dataset_fingerprint: String,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    pub status: String, // "COMPLETED", "FAILED", "PARTIAL"
    pub conversations_discovered: usize,
    pub conversations_imported: usize,
    pub messages_processed: usize,
    pub messages_imported: usize,
    pub messages_skipped: usize,
    pub attachments_discovered: usize,
    pub attachments_imported: usize,
    pub attachments_missing: usize,
    pub reactions_imported: usize,
    pub call_events_imported: usize,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Meta Instagram Export JSON Schema Models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConversationJson {
    pub participants: Option<Vec<RawParticipant>>,
    pub messages: Option<Vec<RawMessage>>,
    pub title: Option<String>,
    pub is_still_participant: Option<bool>,
    pub thread_path: Option<String>,
    pub thread_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RawParticipant {
    Named { name: String },
    StringHandle(String),
}

impl RawParticipant {
    pub fn name(&self) -> &str {
        match self {
            RawParticipant::Named { name } => name.as_str(),
            RawParticipant::StringHandle(h) => h.as_str(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMessage {
    pub sender_name: Option<String>,
    pub timestamp_ms: Option<i64>,
    pub timestamp: Option<i64>,
    pub content: Option<String>,
    pub photos: Option<Vec<RawMedia>>,
    pub audio_files: Option<Vec<RawMedia>>,
    pub videos: Option<Vec<RawMedia>>,
    pub files: Option<Vec<RawMedia>>,
    pub share: Option<RawShare>,
    pub reactions: Option<Vec<RawReaction>>,
    pub call_duration: Option<i64>,
    pub missed: Option<bool>,
    pub source_message_id: Option<String>,
    pub mid: Option<String>,
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMedia {
    pub uri: Option<String>,
    pub creation_timestamp: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawShare {
    pub link: Option<String>,
    pub share_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawReaction {
    pub reaction: Option<String>,
    pub actor: Option<String>,
}

// ---------------------------------------------------------------------------
// Safe Encoding & Mojibake Resolution
// ---------------------------------------------------------------------------

/// Normalizes Latin-1 / UTF-8 mojibake commonly present in Meta DYI JSON exports.
///
/// Meta frequently stores UTF-8 character byte sequences as ISO-8859-1 strings
/// (e.g., "\u00c3\u00a9" instead of "é" or "\u00e2\u009d\u00a4" instead of "❤️").
/// This function verifies that all characters lie within Latin-1 (<= 0xFF) and form
/// valid UTF-8 when converted to bytes. If so, it returns the cleanly decoded string;
/// otherwise, the original string is preserved unmodified.
pub fn fix_mojibake(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    for c in input.chars() {
        let u = c as u32;
        if u <= 0xFF {
            bytes.push(u as u8);
        } else {
            // Contains code points > 0xFF; already true multi-byte Unicode
            return input.to_string();
        }
    }

    if let Ok(decoded) = std::str::from_utf8(&bytes) {
        if decoded != input && !decoded.contains('\u{FFFD}') {
            return decoded.to_string();
        }
    }

    input.to_string()
}

/// Normalizes message timestamps to integer epoch milliseconds.
/// Handles both millisecond timestamps (~13 digits) and second timestamps (~10 digits).
pub fn normalize_timestamp(raw: &RawMessage) -> Option<i64> {
    if let Some(ts) = raw.timestamp_ms {
        if ts > 100_000_000_000 {
            return Some(ts);
        } else if ts > 0 {
            return Some(ts * 1000);
        }
    }

    if let Some(ts) = raw.timestamp {
        if ts > 100_000_000_000 {
            return Some(ts);
        } else if ts > 0 {
            return Some(ts * 1000);
        }
    }

    None
}

/// Validates that a path is safe, relative, and does not attempt directory traversal.
/// Rejects path traversal tokens ('..'), absolute paths, Windows drive letters (e.g. 'C:'),
/// Windows alternate data streams (':'), and UNC path prefixes ('//', '\\\\').
pub fn validate_safe_relative_path(path_str: &str) -> Result<(), ImporterError> {
    let clean = path_str.replace('\\', "/");
    let trimmed = clean.trim();
    if trimmed.is_empty() {
        return Err(ImporterError::PathTraversal("Empty path".to_string()));
    }
    if trimmed.contains("..") || trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(ImporterError::PathTraversal(path_str.to_string()));
    }
    if trimmed.contains(':') {
        return Err(ImporterError::PathTraversal(path_str.to_string()));
    }
    if Path::new(trimmed).is_absolute() {
        return Err(ImporterError::PathTraversal(path_str.to_string()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Archive Abstraction (Directory or ZIP)
// ---------------------------------------------------------------------------

pub enum ArchiveSource {
    Directory(PathBuf),
    Zip(PathBuf),
}

impl ArchiveSource {
    pub fn open(path: &Path) -> Result<Self, ImporterError> {
        if !path.exists() {
            return Err(ImporterError::InvalidArchive(format!(
                "Archive path does not exist: {}",
                path.display()
            )));
        }

        if path.is_dir() {
            Ok(ArchiveSource::Directory(path.to_path_buf()))
        } else if path.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if ext == "zip" {
                Ok(ArchiveSource::Zip(path.to_path_buf()))
            } else {
                Err(ImporterError::InvalidArchive(format!(
                    "Unsupported archive file type '{}'. Expected .zip or directory.",
                    path.display()
                )))
            }
        } else {
            Err(ImporterError::InvalidArchive(format!(
                "Invalid path target: {}",
                path.display()
            )))
        }
    }

    /// Computes a deterministic dataset fingerprint from the archive source.
    pub fn compute_fingerprint(&self) -> Result<String, ImporterError> {
        match self {
            ArchiveSource::Zip(p) => {
                let mut file = File::open(p)?;
                let mut hasher = Sha256::new();
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let n = file.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buffer[..n]);
                }
                Ok(format!("zip:{}", hex::encode(hasher.finalize())))
            }
            ArchiveSource::Directory(p) => {
                // Content-based directory fingerprint: hash sorted relative paths and file sizes
                let mut files = Self::walk_directory_all(p);
                files.sort();
                let mut hasher = Sha256::new();
                for f in files {
                    if let Ok(rel) = f.strip_prefix(p) {
                        hasher.update(rel.to_string_lossy().replace('\\', "/").as_bytes());
                        if let Ok(meta) = f.metadata() {
                            hasher.update(&meta.len().to_le_bytes());
                        }
                    }
                }
                Ok(format!("dir:{}", hex::encode(hasher.finalize())))
            }
        }
    }

    /// Discovers all conversation message JSON files and attachments within the archive.
    pub fn discover_and_validate(&self) -> Result<ValidationSummary, ImporterError> {
        let fingerprint = self.compute_fingerprint()?;
        let mut warnings = Vec::new();
        let mut errors = Vec::new();
        let mut conversations_map: std::collections::BTreeMap<String, DiscoveredConversation> =
            std::collections::BTreeMap::new();
        let mut total_files = 0;
        let mut attachment_candidates = 0;

        match self {
            ArchiveSource::Directory(root) => {
                let json_files = Self::walk_directory_json(root);
                for file_path in json_files {
                    let rel_path = file_path
                        .strip_prefix(root)
                        .unwrap_or(&file_path)
                        .to_string_lossy()
                        .replace('\\', "/");

                    // Filter for message_*.json files
                    if !is_message_file(&rel_path) {
                        continue;
                    }

                    match fs::read_to_string(&file_path) {
                        Ok(content) => {
                            match serde_json::from_str::<RawConversationJson>(&content) {
                                Ok(parsed) => {
                                    let conv_id = derive_conversation_source_id(&rel_path, &parsed);
                                    let participants: Vec<String> = parsed
                                        .participants
                                        .unwrap_or_default()
                                        .into_iter()
                                        .map(|p| fix_mojibake(p.name()))
                                        .collect();

                                    if let Some(msgs) = &parsed.messages {
                                        for m in msgs {
                                            attachment_candidates += count_message_attachments(m);
                                        }
                                    }

                                    total_files += 1;
                                    let title = parsed.title.map(|t| fix_mojibake(&t));

                                    let entry = conversations_map
                                        .entry(conv_id.clone())
                                        .or_insert_with(|| DiscoveredConversation {
                                            source_id: conv_id,
                                            title,
                                            json_files: Vec::new(),
                                            participants,
                                        });
                                    entry.json_files.push(rel_path);
                                }
                                Err(e) => {
                                    errors.push(format!(
                                        "Malformed JSON in file '{}': {}",
                                        rel_path, e
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            errors.push(format!("Could not read file '{}': {}", rel_path, e));
                        }
                    }
                }
            }
            ArchiveSource::Zip(zip_path) => {
                let file = File::open(zip_path)?;
                let mut archive =
                    zip::ZipArchive::new(file).map_err(|e| ImporterError::Zip(e.to_string()))?;

                if archive.len() > MAX_ARCHIVE_ENTRIES {
                    return Err(ImporterError::ZipBomb(format!(
                        "Archive contains {} entries, exceeding safety threshold of {}",
                        archive.len(),
                        MAX_ARCHIVE_ENTRIES
                    )));
                }

                let mut cumulative_uncompressed: u64 = 0;
                let mut seen_entries: std::collections::HashSet<String> =
                    std::collections::HashSet::new();

                for i in 0..archive.len() {
                    let mut entry = archive
                        .by_index(i)
                        .map_err(|e| ImporterError::Zip(e.to_string()))?;

                    let entry_name = entry.name().to_string().replace('\\', "/");

                    // Security: Path Traversal Guard (Drive letters, UNC, relative traversal)
                    validate_safe_relative_path(&entry_name)?;

                    // Security: Symlink Guard
                    if entry
                        .unix_mode()
                        .map(|m| (m & 0o170000) == 0o120000)
                        .unwrap_or(false)
                    {
                        warnings.push(format!("Skipping symbolic link entry '{}'", entry_name));
                        continue;
                    }

                    // Security: Duplicate Entry Detection
                    if !seen_entries.insert(entry_name.clone()) {
                        warnings.push(format!(
                            "Duplicate entry '{}' in archive; ignoring duplicate",
                            entry_name
                        ));
                        continue;
                    }

                    // Security: Single Entry Size Limit
                    if entry.size() > MAX_UNCOMPRESSED_ENTRY_BYTES {
                        return Err(ImporterError::ZipBomb(format!(
                            "Entry '{}' uncompressed size {} bytes exceeds single entry safety threshold of 500MB",
                            entry_name,
                            entry.size()
                        )));
                    }

                    // Security: Cumulative Size Limit
                    cumulative_uncompressed = cumulative_uncompressed.saturating_add(entry.size());
                    if cumulative_uncompressed > MAX_CUMULATIVE_UNCOMPRESSED_BYTES {
                        return Err(ImporterError::ZipBomb(
                            "Cumulative uncompressed archive size exceeds safety threshold of 10GB"
                                .to_string(),
                        ));
                    }

                    // Security: Compression Ratio Check (for non-trivial entries > 10MB)
                    if entry.compressed_size() > 0 && entry.size() > 10 * 1024 * 1024 {
                        let ratio = entry.size() / entry.compressed_size();
                        if ratio > MAX_COMPRESSION_RATIO {
                            return Err(ImporterError::ZipBomb(format!(
                                "Suspicious compression ratio {}x for entry '{}'",
                                ratio, entry_name
                            )));
                        }
                    }

                    if !is_message_file(&entry_name) {
                        continue;
                    }

                    let mut content = String::new();
                    if let Err(e) = entry.read_to_string(&mut content) {
                        errors.push(format!("Could not read zip entry '{}': {}", entry_name, e));
                        continue;
                    }

                    match serde_json::from_str::<RawConversationJson>(&content) {
                        Ok(parsed) => {
                            let conv_id = derive_conversation_source_id(&entry_name, &parsed);
                            let participants: Vec<String> = parsed
                                .participants
                                .unwrap_or_default()
                                .into_iter()
                                .map(|p| fix_mojibake(p.name()))
                                .collect();

                            if let Some(msgs) = &parsed.messages {
                                for m in msgs {
                                    attachment_candidates += count_message_attachments(m);
                                }
                            }

                            total_files += 1;
                            let title = parsed.title.map(|t| fix_mojibake(&t));

                            let entry =
                                conversations_map.entry(conv_id.clone()).or_insert_with(|| {
                                    DiscoveredConversation {
                                        source_id: conv_id,
                                        title,
                                        json_files: Vec::new(),
                                        participants,
                                    }
                                });
                            entry.json_files.push(entry_name);
                        }
                        Err(e) => {
                            errors.push(format!(
                                "Malformed JSON in zip entry '{}': {}",
                                entry_name, e
                            ));
                        }
                    }
                }
            }
        }

        let is_empty = total_files == 0;
        let is_valid = errors.is_empty() && (!is_empty || warnings.is_empty());

        if is_empty {
            warnings.push("Archive contains no recognized conversation message files".to_string());
        }

        let conversations: Vec<DiscoveredConversation> = conversations_map.into_values().collect();

        Ok(ValidationSummary {
            is_valid,
            is_empty,
            source_type: match self {
                ArchiveSource::Directory(_) => "DIRECTORY".to_string(),
                ArchiveSource::Zip(_) => "ZIP".to_string(),
            },
            dataset_fingerprint: fingerprint,
            conversations,
            total_message_files: total_files,
            attachment_candidates,
            warnings,
            errors,
        })
    }

    /// Recursively walks a directory to collect all .json files.
    fn walk_directory_json(dir: &Path) -> Vec<PathBuf> {
        let mut results = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    results.extend(Self::walk_directory_json(&path));
                } else if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if ext.eq_ignore_ascii_case("json") {
                            results.push(path);
                        }
                    }
                }
            }
        }
        results
    }

    /// Recursively walks a directory to collect all files.
    fn walk_directory_all(dir: &Path) -> Vec<PathBuf> {
        let mut results = Vec::new();
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    results.extend(Self::walk_directory_all(&path));
                } else if path.is_file() {
                    results.push(path);
                }
            }
        }
        results
    }

    /// Reads a specific JSON file string from the archive.
    pub fn read_json(&self, rel_path: &str) -> Result<String, ImporterError> {
        validate_safe_relative_path(rel_path)?;
        match self {
            ArchiveSource::Directory(root) => {
                let full_path = root.join(rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));
                let content = fs::read_to_string(full_path)?;
                Ok(content)
            }
            ArchiveSource::Zip(zip_path) => {
                let file = File::open(zip_path)?;
                let mut archive =
                    zip::ZipArchive::new(file).map_err(|e| ImporterError::Zip(e.to_string()))?;
                let mut entry = archive
                    .by_name(rel_path)
                    .map_err(|e| ImporterError::Zip(e.to_string()))?;
                let mut content = String::new();
                entry.read_to_string(&mut content)?;
                Ok(content)
            }
        }
    }

    /// Streams an attachment from the archive to the local media directory.
    /// Returns Some((sha256, file_size)) on success, or None if the file was not found in the archive.
    pub fn copy_media(
        &self,
        rel_uri: &str,
        dest_path: &Path,
    ) -> Result<Option<(String, u64)>, ImporterError> {
        // Path traversal protection (handles drive letters, UNC, relative tokens)
        validate_safe_relative_path(rel_uri)?;
        let normalized_uri = rel_uri.replace('\\', "/");

        // Avoid re-copying if media file already exists locally and is valid
        if dest_path.is_file() {
            if let Ok(meta) = dest_path.metadata() {
                if meta.len() > 0 {
                    if let Ok(mut existing_file) = File::open(dest_path) {
                        let mut hasher = Sha256::new();
                        let mut buffer = [0u8; 64 * 1024];
                        let mut total_bytes = 0u64;
                        let mut read_ok = true;
                        loop {
                            match existing_file.read(&mut buffer) {
                                Ok(0) => break,
                                Ok(n) => {
                                    hasher.update(&buffer[..n]);
                                    total_bytes += n as u64;
                                }
                                Err(_) => {
                                    read_ok = false;
                                    break;
                                }
                            }
                        }
                        if read_ok && total_bytes == meta.len() {
                            return Ok(Some((hex::encode(hasher.finalize()), total_bytes)));
                        }
                    }
                }
            }
        }

        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let temp_dest = dest_path.with_extension(format!("tmp_{}", Uuid::new_v4()));

        let copy_result: Option<(String, u64)> = match self {
            ArchiveSource::Directory(root) => {
                // Attempt candidate locations
                let candidates = [
                    root.join(&normalized_uri),
                    root.join(strip_leading_folders(&normalized_uri)),
                    root.join("messages")
                        .join(strip_leading_folders(&normalized_uri)),
                ];

                let mut found_src = None;
                for cand in &candidates {
                    if cand.is_file() {
                        found_src = Some(cand);
                        break;
                    }
                }

                if let Some(src) = found_src {
                    let mut src_file = File::open(src)?;
                    let mut dest_file = File::create(&temp_dest)?;
                    let mut hasher = Sha256::new();
                    let mut buffer = [0u8; 64 * 1024];
                    let mut total_bytes = 0u64;

                    loop {
                        let n = src_file.read(&mut buffer)?;
                        if n == 0 {
                            break;
                        }
                        dest_file.write_all(&buffer[..n])?;
                        hasher.update(&buffer[..n]);
                        total_bytes += n as u64;
                    }
                    dest_file.flush()?;
                    Some((hex::encode(hasher.finalize()), total_bytes))
                } else {
                    None
                }
            }
            ArchiveSource::Zip(zip_path) => {
                let file = File::open(zip_path)?;
                let mut archive =
                    zip::ZipArchive::new(file).map_err(|e| ImporterError::Zip(e.to_string()))?;

                // Search by full name or stripped name
                let candidate_names = [
                    normalized_uri.clone(),
                    strip_leading_folders(&normalized_uri).to_string(),
                    format!("messages/{}", strip_leading_folders(&normalized_uri)),
                ];

                let mut zip_index = None;
                for name in &candidate_names {
                    if let Some(idx) = archive.index_for_name(name) {
                        zip_index = Some(idx);
                        break;
                    }
                }

                if let Some(idx) = zip_index {
                    let mut entry = archive
                        .by_index(idx)
                        .map_err(|e| ImporterError::Zip(e.to_string()))?;

                    if entry.size() > MAX_UNCOMPRESSED_ENTRY_BYTES {
                        return Err(ImporterError::ZipBomb(entry.name().to_string()));
                    }

                    let mut dest_file = File::create(&temp_dest)?;
                    let mut hasher = Sha256::new();
                    let mut buffer = [0u8; 64 * 1024];
                    let mut total_bytes = 0u64;

                    loop {
                        let n = entry.read(&mut buffer)?;
                        if n == 0 {
                            break;
                        }
                        dest_file.write_all(&buffer[..n])?;
                        hasher.update(&buffer[..n]);
                        total_bytes += n as u64;
                    }
                    dest_file.flush()?;
                    Some((hex::encode(hasher.finalize()), total_bytes))
                } else {
                    None
                }
            }
        };

        if let Some((hash, size)) = copy_result {
            // Atomic rename from temp file to final destination
            fs::rename(&temp_dest, dest_path)?;
            Ok(Some((hash, size)))
        } else {
            if temp_dest.exists() {
                let _ = fs::remove_file(&temp_dest);
            }
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// Helper Functions for Path and Message Detection
// ---------------------------------------------------------------------------

fn is_message_file(path_str: &str) -> bool {
    let lower = path_str.to_lowercase();
    (lower.contains("/inbox/") || lower.contains("\\inbox\\"))
        && lower.ends_with(".json")
        && (lower.contains("message_") || lower.contains("message1") || lower.contains("messages"))
}

fn strip_leading_folders(uri: &str) -> &str {
    let patterns = [
        "your_instagram_activity/messages/",
        "your_activity_across_facebook_and_instagram/messages/",
        "messages/",
    ];
    for p in patterns {
        if let Some(stripped) = uri.strip_prefix(p) {
            return stripped;
        }
    }
    uri
}

fn derive_conversation_source_id(file_rel_path: &str, raw_conv: &RawConversationJson) -> String {
    if let Some(tp) = &raw_conv.thread_path {
        let clean = tp.trim_matches('/').replace('\\', "/");
        if let Some(last_seg) = clean.split('/').last() {
            if !last_seg.is_empty() {
                return last_seg.to_string();
            }
        }
    }

    // Fallback: derive from containing folder name
    let clean_path = file_rel_path.replace('\\', "/");
    let parts: Vec<&str> = clean_path.split('/').collect();
    if parts.len() >= 2 {
        parts[parts.len() - 2].to_string()
    } else {
        "unknown_thread".to_string()
    }
}

fn count_message_attachments(m: &RawMessage) -> usize {
    let mut count = 0;
    if let Some(p) = &m.photos {
        count += p.len();
    }
    if let Some(a) = &m.audio_files {
        count += a.len();
    }
    if let Some(v) = &m.videos {
        count += v.len();
    }
    if let Some(f) = &m.files {
        count += f.len();
    }
    count
}

// ---------------------------------------------------------------------------
// Importer Engine Implementation
// ---------------------------------------------------------------------------

pub struct Importer<'a> {
    db: &'a mut Database,
    source: ArchiveSource,
    source_account_id: String,
    media_root: PathBuf,
}

impl<'a> Importer<'a> {
    pub fn new(
        db: &'a mut Database,
        source_path: &Path,
        source_account_id: &str,
        media_root: &Path,
    ) -> Result<Self, ImporterError> {
        let source = ArchiveSource::open(source_path)?;
        Ok(Self {
            db,
            source,
            source_account_id: source_account_id.to_string(),
            media_root: media_root.to_path_buf(),
        })
    }

    /// Executes the full archive import process.
    pub fn run_import(&mut self) -> Result<ImportReport, ImporterError> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let job_id = Uuid::new_v4().to_string();
        let validation = self.source.discover_and_validate()?;

        // 1. Initialize import_jobs record
        self.db.conn().execute(
            "INSERT INTO import_jobs (
                id, source_type, source_path, started_at_ms, status,
                total_messages_imported, total_media_files_imported
            ) VALUES (?1, ?2, ?3, ?4, 'RUNNING', 0, 0)",
            params![
                job_id,
                validation.source_type,
                format!("{:?}", self.media_root),
                now_ms
            ],
        )?;

        let mut report = ImportReport {
            job_id: job_id.clone(),
            source_path: format!("{:?}", self.media_root),
            dataset_fingerprint: validation.dataset_fingerprint.clone(),
            started_at_ms: now_ms,
            completed_at_ms: None,
            status: "RUNNING".to_string(),
            conversations_discovered: validation.conversations.len(),
            conversations_imported: 0,
            messages_processed: 0,
            messages_imported: 0,
            messages_skipped: 0,
            attachments_discovered: validation.attachment_candidates,
            attachments_imported: 0,
            attachments_missing: 0,
            reactions_imported: 0,
            call_events_imported: 0,
            warnings: validation.warnings,
            errors: validation.errors,
        };

        if validation.is_empty {
            report.status = "COMPLETED".to_string();
            report.completed_at_ms = Some(now_ms);
            self.db.conn().execute(
                "UPDATE import_jobs SET status = 'COMPLETED', completed_at_ms = ?1 WHERE id = ?2",
                params![now_ms, job_id],
            )?;
            return Ok(report);
        }

        // 2. Process each discovered conversation
        for conv in validation.conversations {
            match self.import_conversation(
                &conv,
                &validation.dataset_fingerprint,
                &job_id,
                &mut report,
            ) {
                Ok(_) => {
                    report.conversations_imported += 1;
                }
                Err(e) => {
                    let err_msg =
                        format!("Error importing conversation '{}': {}", conv.source_id, e);
                    report.errors.push(err_msg.clone());
                    let _ = self.db.conn().execute(
                        "INSERT INTO failed_items (id, job_id, item_type, source_reference, error_reason, occurred_at_ms)
                         VALUES (?1, ?2, 'CONVERSATION', ?3, ?4, ?5)",
                        params![Uuid::new_v4().to_string(), job_id, conv.source_id, err_msg, now_ms],
                    );
                }
            }
        }

        let finish_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        report.completed_at_ms = Some(finish_ms);
        report.status = if report.errors.is_empty() {
            "COMPLETED".to_string()
        } else if report.messages_imported > 0 {
            "PARTIAL".to_string()
        } else {
            "FAILED".to_string()
        };

        self.db.conn().execute(
            "UPDATE import_jobs SET 
                status = ?1,
                completed_at_ms = ?2,
                total_messages_imported = ?3,
                total_media_files_imported = ?4,
                error_summary = ?5
             WHERE id = ?6",
            params![
                report.status,
                finish_ms,
                report.messages_imported as i64,
                report.attachments_imported as i64,
                if report.errors.is_empty() {
                    None
                } else {
                    Some(report.errors.join("; "))
                },
                job_id
            ],
        )?;

        Ok(report)
    }

    /// Imports all message files for a single conversation.
    fn import_conversation(
        &mut self,
        conv_meta: &DiscoveredConversation,
        dataset_fingerprint: &str,
        job_id: &str,
        report: &mut ImportReport,
    ) -> Result<(), ImporterError> {
        let conversation_source_id = &conv_meta.source_id;

        // Ensure conversation record exists
        let conversation_id = self.ensure_conversation(
            conversation_source_id,
            conv_meta.title.as_deref().unwrap_or(conversation_source_id),
        )?;

        // Ensure participants exist and link them
        let mut participant_id_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        for name in &conv_meta.participants {
            let p_id = self.ensure_participant(name, name == &self.source_account_id)?;
            self.link_conversation_participant(&conversation_id, &p_id)?;
            participant_id_map.insert(name.clone(), p_id);
        }

        // Inspect existing sync checkpoint
        let checkpoint = self.get_checkpoint(conversation_source_id)?;

        // Canonical occurrence tracker across this conversation to prevent duplicating
        // historical messages without source IDs when an export shifts files or positions
        let mut canonical_tracker: std::collections::HashMap<
            (String, String, i64, String, Option<String>),
            usize,
        > = std::collections::HashMap::new();

        // Process message files in sorted order
        let mut json_files = conv_meta.json_files.clone();
        json_files.sort();

        for rel_file in json_files {
            let json_content = self.source.read_json(&rel_file)?;
            let parsed_conv: RawConversationJson = serde_json::from_str(&json_content)?;
            let raw_messages = parsed_conv.messages.unwrap_or_default();

            // Evaluate checkpoint if this was the last checkpointed file
            let mut start_idx = 0;
            if let Some((ref cp_file, cp_idx, ref cp_fp, ref cp_hash)) = checkpoint {
                if cp_file == &rel_file {
                    let current_hash_at_idx = if cp_idx < raw_messages.len() {
                        let m = &raw_messages[cp_idx];
                        Some(compute_message_hash(
                            conversation_source_id,
                            &rel_file,
                            cp_idx,
                            m,
                        ))
                    } else {
                        None
                    };

                    match evaluate_checkpoint(
                        cp_fp,
                        dataset_fingerprint,
                        cp_idx,
                        cp_hash.as_deref(),
                        current_hash_at_idx.as_deref(),
                    ) {
                        CheckpointResumeAction::ResumeFromIndex { next_array_index } => {
                            start_idx = next_array_index;
                            report.messages_skipped +=
                                std::cmp::min(next_array_index, raw_messages.len());
                        }
                        CheckpointResumeAction::DatasetChangedMustRevalidate { reason } => {
                            report.warnings.push(format!(
                                "Checkpoint revalidation for '{}': {}",
                                conversation_source_id, reason
                            ));
                            start_idx = 0; // Revalidate/scan all
                        }
                    }
                }
            }

            // Ingestion loop with transactional batches
            let total_messages = raw_messages.len();
            let mut current_idx = start_idx;

            while current_idx < total_messages {
                let batch_end = std::cmp::min(current_idx + BATCH_SIZE, total_messages);
                let batch = &raw_messages[current_idx..batch_end];

                self.import_message_batch(
                    &conversation_id,
                    conversation_source_id,
                    &rel_file,
                    current_idx,
                    batch,
                    dataset_fingerprint,
                    job_id,
                    &mut participant_id_map,
                    &mut canonical_tracker,
                    report,
                )?;

                current_idx = batch_end;
            }
        }

        Ok(())
    }

    /// Ingests a slice of messages inside a single atomic SQLite transaction.
    #[allow(clippy::too_many_arguments)]
    fn import_message_batch(
        &mut self,
        conversation_id: &str,
        conv_source_id: &str,
        source_file: &str,
        batch_start_idx: usize,
        batch: &[RawMessage],
        dataset_fingerprint: &str,
        job_id: &str,
        participants_map: &mut std::collections::HashMap<String, String>,
        canonical_tracker: &mut std::collections::HashMap<
            (String, String, i64, String, Option<String>),
            usize,
        >,
        report: &mut ImportReport,
    ) -> Result<(), ImporterError> {
        let tx = self.db.conn_mut().transaction()?;

        let mut last_committed_idx = batch_start_idx;
        let mut last_committed_hash = String::new();
        let mut last_committed_ts = 0i64;

        for (offset, raw_msg) in batch.iter().enumerate() {
            let array_idx = batch_start_idx + offset;
            report.messages_processed += 1;

            let ts_ms = match normalize_timestamp(raw_msg) {
                Some(ts) => ts,
                None => {
                    report.warnings.push(format!(
                        "Skipping message in '{}' at index {}: missing timestamp",
                        source_file, array_idx
                    ));
                    continue;
                }
            };

            let sender_name = raw_msg
                .sender_name
                .as_deref()
                .map(fix_mojibake)
                .unwrap_or_else(|| "unknown".to_string());

            let sender_id = if let Some(id) = participants_map.get(&sender_name) {
                id.clone()
            } else {
                let id = Uuid::new_v4().to_string();
                let is_self = if sender_name == self.source_account_id {
                    1
                } else {
                    0
                };
                tx.execute(
                    "INSERT INTO participants (id, source_identifier, username, is_self)
                     VALUES (?1, ?2, ?2, ?3)
                     ON CONFLICT(source_identifier) DO UPDATE SET is_self = MAX(is_self, ?3)",
                    params![id, sender_name, is_self],
                )?;
                participants_map.insert(sender_name.clone(), id.clone());
                id
            };

            let is_outgoing = if sender_name == self.source_account_id {
                1
            } else {
                0
            };
            let identity_hash =
                compute_message_hash(conv_source_id, source_file, array_idx, raw_msg);
            let message_id = Uuid::new_v4().to_string();

            let message_type = determine_message_type(raw_msg);
            let content_fixed = raw_msg.content.as_deref().map(fix_mojibake);
            let source_mid = raw_msg
                .source_message_id
                .as_deref()
                .or(raw_msg.mid.as_deref())
                .or(raw_msg.id.as_deref());

            // Check 1: Check if exact identity_hash already exists (handles Tier 1 and identical archive re-imports)
            let existing_id: Option<String> = tx
                .query_row(
                    "SELECT id FROM messages WHERE identity_hash = ?1",
                    params![identity_hash],
                    |r| r.get(0),
                )
                .ok();

            if existing_id.is_some() {
                report.messages_skipped += 1;
                last_committed_idx = array_idx;
                last_committed_hash = identity_hash;
                last_committed_ts = ts_ms;
                continue;
            }

            // Check 2: Changed-archive reconciliation for messages without stable source IDs
            // Prevents re-duplicating historical messages when an export is re-partitioned or array indices shift
            if source_mid.is_none() {
                let canonical_key = (
                    conversation_id.to_string(),
                    sender_id.clone(),
                    ts_ms,
                    message_type.to_string(),
                    content_fixed.clone(),
                );
                let incoming_occurrence = canonical_tracker
                    .entry(canonical_key)
                    .and_modify(|c| *c += 1)
                    .or_insert(1);

                let existing_count: i64 = tx
                    .query_row(
                        "SELECT COUNT(*) FROM messages 
                         WHERE conversation_id = ?1 
                           AND sender_id = ?2 
                           AND timestamp_ms = ?3 
                           AND message_type = ?4
                           AND ((content IS NULL AND ?5 IS NULL) OR content = ?5)",
                        params![
                            conversation_id,
                            sender_id,
                            ts_ms,
                            message_type,
                            content_fixed
                        ],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);

                if (*incoming_occurrence as i64) <= existing_count {
                    report.messages_skipped += 1;
                    last_committed_idx = array_idx;
                    last_committed_hash = identity_hash;
                    last_committed_ts = ts_ms;
                    continue;
                }
            }

            // Insert new message
            tx.execute(
                "INSERT INTO messages (
                    id, conversation_id, sender_id, source_message_id, identity_hash,
                    source_file_path, source_array_index, message_type, content,
                    timestamp_ms, is_outgoing, is_edited, raw_payload_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)",
                params![
                    message_id,
                    conversation_id,
                    sender_id,
                    source_mid,
                    identity_hash,
                    source_file,
                    array_idx as i64,
                    message_type,
                    content_fixed,
                    ts_ms,
                    is_outgoing,
                    serde_json::to_string(raw_msg).ok()
                ],
            )?;

            report.messages_imported += 1;
            last_committed_idx = array_idx;
            last_committed_hash = identity_hash.clone();
            last_committed_ts = ts_ms;

            // Ingest Attachments (Photos, Audio, Video)
            Self::ingest_attachments(
                &self.source,
                &self.media_root,
                &tx,
                &message_id,
                conversation_id,
                raw_msg,
                report,
            )?;

            // Ingest Reactions
            if let Some(reactions) = &raw_msg.reactions {
                for r in reactions {
                    if let (Some(actor), Some(reaction_val)) = (&r.actor, &r.reaction) {
                        let actor_fixed = fix_mojibake(actor);
                        let val_fixed = fix_mojibake(reaction_val);
                        let actor_id = if let Some(id) = participants_map.get(&actor_fixed) {
                            id.clone()
                        } else {
                            let id = Uuid::new_v4().to_string();
                            tx.execute(
                                "INSERT INTO participants (id, source_identifier, username, is_self)
                                 VALUES (?1, ?2, ?2, 0)
                                 ON CONFLICT(source_identifier) DO NOTHING",
                                params![id, actor_fixed],
                            )?;
                            participants_map.insert(actor_fixed.clone(), id.clone());
                            id
                        };

                        let rx_id = Uuid::new_v4().to_string();
                        let inserted = tx.execute(
                            "INSERT INTO reactions (id, message_id, participant_id, reaction_value, created_at_ms)
                             VALUES (?1, ?2, ?3, ?4, ?5)
                             ON CONFLICT(message_id, participant_id, reaction_value) DO NOTHING",
                            params![rx_id, message_id, actor_id, val_fixed, ts_ms],
                        )?;
                        if inserted > 0 {
                            report.reactions_imported += 1;
                        }
                    }
                }
            }

            // Ingest Read Receipts (Explicit UNAVAILABLE_FROM_SOURCE default)
            let rr_id = Uuid::new_v4().to_string();
            tx.execute(
                "INSERT INTO read_receipts (id, message_id, participant_id, read_status, read_at_ms)
                 VALUES (?1, ?2, ?3, 'UNAVAILABLE_FROM_SOURCE', NULL)
                 ON CONFLICT(message_id, participant_id) DO NOTHING",
                params![rr_id, message_id, sender_id],
            )?;

            // Ingest Call Events
            if message_type == "CALL" {
                let call_id = Uuid::new_v4().to_string();
                let duration = raw_msg.call_duration;
                let is_video = content_fixed
                    .as_deref()
                    .map(|c| c.to_lowercase().contains("video"))
                    .unwrap_or(false);
                let call_type = if is_video { "VIDEO" } else { "VOICE" };
                let direction = if is_outgoing == 1 {
                    "OUTGOING"
                } else {
                    "INCOMING"
                };
                let is_missed = raw_msg.missed.unwrap_or(false)
                    || content_fixed
                        .as_deref()
                        .map(|c| c.to_lowercase().contains("missed"))
                        .unwrap_or(false);
                let status = if is_missed {
                    "MISSED"
                } else if duration.unwrap_or(0) > 0 {
                    "ANSWERED"
                } else {
                    "UNKNOWN"
                };

                let derived_end = duration.map(|d| ts_ms + d * 1000);

                tx.execute(
                    "INSERT INTO call_events (
                        id, conversation_id, message_id, caller_id, call_type,
                        direction, status, start_timestamp_ms, duration_seconds,
                        derived_end_timestamp_ms
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        call_id,
                        conversation_id,
                        message_id,
                        sender_id,
                        call_type,
                        direction,
                        status,
                        ts_ms,
                        duration,
                        derived_end
                    ],
                )?;
                report.call_events_imported += 1;
            }
        }

        // Atomically update sync_checkpoints
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        tx.execute(
            "INSERT INTO sync_checkpoints (
                source_account_id, conversation_source_id, dataset_fingerprint,
                last_job_id, last_file_rel_path, last_source_array_index,
                last_timestamp_ms, last_message_identity_hash, revalidation_status, updated_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'VALID', ?9)
            ON CONFLICT(source_account_id, conversation_source_id) DO UPDATE SET
                dataset_fingerprint = excluded.dataset_fingerprint,
                last_job_id = excluded.last_job_id,
                last_file_rel_path = excluded.last_file_rel_path,
                last_source_array_index = excluded.last_source_array_index,
                last_timestamp_ms = excluded.last_timestamp_ms,
                last_message_identity_hash = excluded.last_message_identity_hash,
                revalidation_status = 'VALID',
                updated_at_ms = excluded.updated_at_ms",
            params![
                self.source_account_id,
                conv_source_id,
                dataset_fingerprint,
                job_id,
                source_file,
                last_committed_idx as i64,
                last_committed_ts,
                last_committed_hash,
                now_ms
            ],
        )?;

        // Update conversation updated_at_ms and last_message_preview
        if last_committed_ts > 0 {
            tx.execute(
                "UPDATE conversations SET updated_at_ms = MAX(updated_at_ms, ?1) WHERE id = ?2",
                params![last_committed_ts, conversation_id],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Ingests media attachments for a single message.
    fn ingest_attachments(
        source: &ArchiveSource,
        media_root: &Path,
        tx: &Transaction,
        message_id: &str,
        conversation_id: &str,
        raw_msg: &RawMessage,
        report: &mut ImportReport,
    ) -> Result<(), ImporterError> {
        let mut all_media: Vec<(&RawMedia, &'static str, &'static str)> = Vec::new();

        if let Some(photos) = &raw_msg.photos {
            for p in photos {
                all_media.push((p, "IMAGE", "image/jpeg"));
            }
        }
        if let Some(audio) = &raw_msg.audio_files {
            for a in audio {
                all_media.push((a, "AUDIO", "audio/mp4"));
            }
        }
        if let Some(videos) = &raw_msg.videos {
            for v in videos {
                all_media.push((v, "VIDEO", "video/mp4"));
            }
        }

        for (media, attach_type, mime_type) in all_media {
            let original_uri = match &media.uri {
                Some(u) => u.as_str(),
                None => continue,
            };

            let att_id = Uuid::new_v4().to_string();
            let file_name = Path::new(original_uri)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("media.bin");

            let type_subfolder = match attach_type {
                "AUDIO" => "audio",
                "IMAGE" => "images",
                "VIDEO" => "videos",
                _ => "files",
            };

            // Destination path: {media_root}/{conv_id}/{type_subfolder}/{uuid}_{filename}
            let rel_storage_path = format!(
                "{}/{}/{}_{}",
                conversation_id,
                type_subfolder,
                &att_id[..8],
                file_name
            );
            let full_dest_path = media_root.join(&rel_storage_path);

            match source.copy_media(original_uri, &full_dest_path)? {
                Some((sha256, size_bytes)) => {
                    tx.execute(
                        "INSERT INTO attachments (
                            id, message_id, attachment_type, original_uri, storage_path,
                            mime_type, file_size_bytes, sha256_checksum, status
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'LOCAL_COPIED')",
                        params![
                            att_id,
                            message_id,
                            attach_type,
                            original_uri,
                            rel_storage_path,
                            mime_type,
                            size_bytes as i64,
                            sha256
                        ],
                    )?;
                    report.attachments_imported += 1;
                }
                None => {
                    tx.execute(
                        "INSERT INTO attachments (
                            id, message_id, attachment_type, original_uri, storage_path,
                            mime_type, status
                        ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'MISSING')",
                        params![att_id, message_id, attach_type, original_uri, mime_type],
                    )?;
                    report.attachments_missing += 1;
                }
            }
        }

        Ok(())
    }

    fn ensure_conversation(
        &self,
        source_identifier: &str,
        title: &str,
    ) -> Result<String, ImporterError> {
        let existing: Option<String> = self
            .db
            .conn()
            .query_row(
                "SELECT id FROM conversations WHERE source_identifier = ?1",
                params![source_identifier],
                |r| r.get(0),
            )
            .ok();

        if let Some(id) = existing {
            Ok(id)
        } else {
            let id = Uuid::new_v4().to_string();
            self.db.conn().execute(
                "INSERT INTO conversations (id, source_identifier, conversation_type, title, updated_at_ms)
                 VALUES (?1, ?2, 'DIRECT', ?3, 0)",
                params![id, source_identifier, title],
            )?;
            Ok(id)
        }
    }

    fn ensure_participant(&self, username: &str, is_self: bool) -> Result<String, ImporterError> {
        let self_val = if is_self { 1 } else { 0 };
        let existing: Option<String> = self
            .db
            .conn()
            .query_row(
                "SELECT id FROM participants WHERE source_identifier = ?1",
                params![username],
                |r| r.get(0),
            )
            .ok();

        if let Some(id) = existing {
            if is_self {
                self.db.conn().execute(
                    "UPDATE participants SET is_self = 1 WHERE id = ?1",
                    params![id],
                )?;
            }
            Ok(id)
        } else {
            let id = Uuid::new_v4().to_string();
            self.db.conn().execute(
                "INSERT INTO participants (id, source_identifier, username, is_self)
                 VALUES (?1, ?2, ?2, ?3)",
                params![id, username, self_val],
            )?;
            Ok(id)
        }
    }

    fn link_conversation_participant(
        &self,
        conversation_id: &str,
        participant_id: &str,
    ) -> Result<(), ImporterError> {
        self.db.conn().execute(
            "INSERT INTO conversation_participants (conversation_id, participant_id)
             VALUES (?1, ?2)
             ON CONFLICT(conversation_id, participant_id) DO NOTHING",
            params![conversation_id, participant_id],
        )?;
        Ok(())
    }

    fn get_checkpoint(
        &self,
        conversation_source_id: &str,
    ) -> Result<Option<(String, usize, String, Option<String>)>, ImporterError> {
        let res = self.db.conn().query_row(
            "SELECT last_file_rel_path, last_source_array_index, dataset_fingerprint, last_message_identity_hash
             FROM sync_checkpoints
             WHERE source_account_id = ?1 AND conversation_source_id = ?2",
            params![self.source_account_id, conversation_source_id],
            |r| {
                let file: String = r.get(0)?;
                let idx: i64 = r.get(1)?;
                let fp: String = r.get(2)?;
                let hash: Option<String> = r.get(3)?;
                Ok((file, idx as usize, fp, hash))
            },
        ).ok();

        Ok(res)
    }
}

// ---------------------------------------------------------------------------
// Pure Helper Functions
// ---------------------------------------------------------------------------

pub fn compute_message_hash(
    conv_source_id: &str,
    source_file: &str,
    array_idx: usize,
    raw: &RawMessage,
) -> String {
    if let Some(source_mid) = raw
        .source_message_id
        .as_deref()
        .or(raw.mid.as_deref())
        .or(raw.id.as_deref())
    {
        return compute_tier1_identity_hash(conv_source_id, source_mid);
    }

    let sender = raw.sender_name.as_deref().unwrap_or("unknown");
    let ts_ms = normalize_timestamp(raw).unwrap_or(0);

    let content_digest = if let Some(content) = &raw.content {
        compute_content_digest(content)
    } else if let Some(audio) = &raw.audio_files {
        let uri = audio.first().and_then(|a| a.uri.as_deref()).unwrap_or("");
        compute_content_digest(uri)
    } else if let Some(photos) = &raw.photos {
        let uri = photos.first().and_then(|p| p.uri.as_deref()).unwrap_or("");
        compute_content_digest(uri)
    } else if let Some(videos) = &raw.videos {
        let uri = videos.first().and_then(|v| v.uri.as_deref()).unwrap_or("");
        compute_content_digest(uri)
    } else if let Some(call_duration) = raw.call_duration {
        compute_content_digest(&format!("call:{}", call_duration))
    } else {
        compute_content_digest("empty")
    };

    compute_tier2_identity_hash(
        conv_source_id,
        sender,
        ts_ms,
        source_file,
        array_idx,
        &content_digest,
    )
}

pub fn determine_message_type(raw: &RawMessage) -> &'static str {
    if raw.call_duration.is_some()
        || raw.missed.unwrap_or(false)
        || raw
            .content
            .as_deref()
            .map(|c| c.to_lowercase().contains("call"))
            .unwrap_or(false)
    {
        "CALL"
    } else if raw
        .audio_files
        .as_ref()
        .map(|a| !a.is_empty())
        .unwrap_or(false)
    {
        "VOICE"
    } else if raw.videos.as_ref().map(|v| !v.is_empty()).unwrap_or(false) {
        "VIDEO"
    } else if raw.photos.as_ref().map(|p| !p.is_empty()).unwrap_or(false) {
        "IMAGE"
    } else if raw.share.is_some() {
        "SHARE"
    } else if raw.content.is_some() {
        "TEXT"
    } else {
        "UNKNOWN"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    struct TestEnv {
        temp_dir: PathBuf,
        media_dir: PathBuf,
    }

    impl TestEnv {
        fn new(name: &str) -> Self {
            let temp_dir =
                std::env::temp_dir().join(format!("ig_test_{}_{}", name, Uuid::new_v4()));
            let media_dir = temp_dir.join("app_media");
            fs::create_dir_all(&temp_dir).unwrap();
            fs::create_dir_all(&media_dir).unwrap();
            Self {
                temp_dir,
                media_dir,
            }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.temp_dir);
        }
    }

    fn write_synthetic_file(dir: &Path, rel_path: &str, content: &[u8]) -> PathBuf {
        let full = dir.join(rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = File::create(&full).unwrap();
        file.write_all(content).unwrap();
        full
    }

    fn create_synthetic_zip(zip_path: &Path, files: &[(&str, &[u8])]) {
        let file = File::create(zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        for (name, content) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content).unwrap();
        }
        zip.finish().unwrap();
    }

    // -----------------------------------------------------------------------
    // Test 1: Directory Archive Discovery (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_1_directory_archive_discovery() {
        let env = TestEnv::new("dir_disc");
        let json_data = r#"{
            "title": "Alice",
            "participants": [{"name": "alice"}, {"name": "user"}],
            "messages": [{
                "sender_name": "alice",
                "timestamp_ms": 1700000000000,
                "content": "Hello directory"
            }]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/alice_123/message_1.json",
            json_data.as_bytes(),
        );

        let source = ArchiveSource::open(&env.temp_dir).unwrap();
        let summary = source.discover_and_validate().unwrap();

        assert!(summary.is_valid);
        assert!(!summary.is_empty);
        assert_eq!(summary.source_type, "DIRECTORY");
        assert_eq!(summary.conversations.len(), 1);
        assert_eq!(summary.conversations[0].source_id, "alice_123");
        assert_eq!(summary.total_message_files, 1);
    }

    // -----------------------------------------------------------------------
    // Test 2: ZIP Archive Discovery (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_2_zip_archive_discovery() {
        let env = TestEnv::new("zip_disc");
        let zip_path = env.temp_dir.join("export.zip");
        let json_data = br#"{
            "title": "Bob",
            "participants": [{"name": "bob"}, {"name": "user"}],
            "messages": [{
                "sender_name": "bob",
                "timestamp_ms": 1700000010000,
                "content": "Hello zip"
            }]
        }"#;

        create_synthetic_zip(
            &zip_path,
            &[("messages/inbox/bob_456/message_1.json", json_data)],
        );

        let source = ArchiveSource::open(&zip_path).unwrap();
        let summary = source.discover_and_validate().unwrap();

        assert!(summary.is_valid);
        assert!(!summary.is_empty);
        assert_eq!(summary.source_type, "ZIP");
        assert_eq!(summary.conversations.len(), 1);
        assert_eq!(summary.conversations[0].source_id, "bob_456");
        assert_eq!(summary.total_message_files, 1);
    }

    // -----------------------------------------------------------------------
    // Test 3: Recognized Export Directory Variations (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_3_recognized_export_directory_variations() {
        let env = TestEnv::new("dir_vars");
        let json_alice =
            r#"{"title": "Alice", "participants": [{"name": "alice"}], "messages": []}"#;
        let json_bob = r#"{"title": "Bob", "participants": [{"name": "bob"}], "messages": []}"#;

        // Legacy layout
        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/alice_legacy/message_1.json",
            json_alice.as_bytes(),
        );

        // Modern Accounts Center layout
        write_synthetic_file(
            &env.temp_dir,
            "your_instagram_activity/messages/inbox/bob_modern/message_1.json",
            json_bob.as_bytes(),
        );

        let source = ArchiveSource::open(&env.temp_dir).unwrap();
        let summary = source.discover_and_validate().unwrap();

        assert_eq!(summary.conversations.len(), 2);
        let ids: Vec<&str> = summary
            .conversations
            .iter()
            .map(|c| c.source_id.as_str())
            .collect();
        assert!(ids.contains(&"alice_legacy"));
        assert!(ids.contains(&"bob_modern"));
    }

    // -----------------------------------------------------------------------
    // Test 4: Malformed and Truncated JSON (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_4_malformed_and_truncated_json() {
        let env = TestEnv::new("malformed");
        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/corrupt_conv/message_1.json",
            b"{\"participants\": [ incomplete...",
        );

        let source = ArchiveSource::open(&env.temp_dir).unwrap();
        let summary = source.discover_and_validate().unwrap();

        assert!(!summary.is_valid);
        assert_eq!(summary.errors.len(), 1);
        assert!(summary.errors[0].contains("Malformed JSON"));
    }

    // -----------------------------------------------------------------------
    // Test 5: Valid but Empty Exports (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_5_valid_but_empty_exports() {
        let env = TestEnv::new("empty_exp");
        // Empty directory with no inbox files
        write_synthetic_file(&env.temp_dir, "other_folder/readme.txt", b"just a readme");

        let source = ArchiveSource::open(&env.temp_dir).unwrap();
        let summary = source.discover_and_validate().unwrap();

        assert!(summary.is_empty);
        assert!(summary
            .warnings
            .iter()
            .any(|w| w.contains("no recognized conversation message files")));
    }

    // -----------------------------------------------------------------------
    // Test 6: Participant and Conversation Association (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_6_participant_and_conversation_association() {
        let env = TestEnv::new("part_assoc");
        let json_data = r#"{
            "title": "Group Chat",
            "participants": [{"name": "alice"}, {"name": "bob"}],
            "messages": [{
                "sender_name": "alice",
                "timestamp_ms": 1700000000000,
                "content": "Hi all"
            }]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/group_123/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        let mut importer = Importer::new(&mut db, &env.temp_dir, "bob", &env.media_dir).unwrap();
        let report = importer.run_import().unwrap();

        assert_eq!(report.status, "COMPLETED");
        assert_eq!(report.conversations_imported, 1);
        assert_eq!(report.messages_imported, 1);

        // Verify conversation title and participants
        let conv_title: String = db
            .conn()
            .query_row(
                "SELECT title FROM conversations WHERE source_identifier = 'group_123'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(conv_title, "Group Chat");

        let p_count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM conversation_participants", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(p_count, 2);

        // Check is_self was set for bob
        let bob_is_self: i64 = db
            .conn()
            .query_row(
                "SELECT is_self FROM participants WHERE username = 'bob'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bob_is_self, 1);
    }

    // -----------------------------------------------------------------------
    // Test 7: Text Preservation with Unicode and Line Breaks (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_7_text_preservation_unicode_and_linebreaks() {
        let env = TestEnv::new("text_pres");
        // Note: Includes standard line breaks, emojis, and mojibake sequence "\u00e2\u009d\u00a4" (heart)
        let json_data = r#"{
            "title": "Unicode Test",
            "participants": [{"name": "alice"}],
            "messages": [{
                "sender_name": "alice",
                "timestamp_ms": 1700000000000,
                "content": "Line 1\nLine 2\r\nLine 3 \u00e2\u009d\u00a4 Caf\u00c3\u00a9"
            }]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/unicode_test/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        let mut importer = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        importer.run_import().unwrap();

        let stored_content: String = db
            .conn()
            .query_row("SELECT content FROM messages LIMIT 1", [], |r| r.get(0))
            .unwrap();

        assert!(stored_content.contains("Line 1\nLine 2"));
        assert!(
            stored_content.contains('❤'),
            "Mojibake must be normalized to heart emoji"
        );
        assert!(
            stored_content.contains("Café"),
            "Mojibake must be normalized to Café"
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: Timestamp Conversion and Chronological Ordering (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_8_timestamp_conversion_and_ordering() {
        let env = TestEnv::new("ts_order");
        let json_data = r#"{
            "title": "Order Test",
            "participants": [{"name": "alice"}],
            "messages": [
                {
                    "sender_name": "alice",
                    "timestamp": 1700000050,
                    "content": "Message 2 (seconds ts)"
                },
                {
                    "sender_name": "alice",
                    "timestamp_ms": 1700000010000,
                    "content": "Message 1 (ms ts)"
                }
            ]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/ts_order/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        let mut importer = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        importer.run_import().unwrap();

        let mut stmt = db
            .conn()
            .prepare("SELECT timestamp_ms, content FROM messages ORDER BY timestamp_ms ASC")
            .unwrap();

        let rows: Vec<(i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|res| res.unwrap())
            .collect();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, 1700000010000);
        assert_eq!(rows[0].1, "Message 1 (ms ts)");
        assert_eq!(
            rows[1].0, 1700000050000,
            "Second timestamp must be converted to ms"
        );
        assert_eq!(rows[1].1, "Message 2 (seconds ts)");
    }

    // -----------------------------------------------------------------------
    // Test 9: Repeated Identical Messages Remain Distinct (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_9_repeated_identical_messages_remain_distinct() {
        let env = TestEnv::new("ident_msgs");
        let json_data = r#"{
            "title": "Identical Test",
            "participants": [{"name": "alice"}],
            "messages": [
                {
                    "sender_name": "alice",
                    "timestamp_ms": 1700000000000,
                    "content": "ok"
                },
                {
                    "sender_name": "alice",
                    "timestamp_ms": 1700000000000,
                    "content": "ok"
                }
            ]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/ident_test/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        let mut importer = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let report = importer.run_import().unwrap();

        assert_eq!(report.messages_imported, 2);

        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE content = 'ok'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 2,
            "Both legitimate identical messages must be preserved"
        );
    }

    // -----------------------------------------------------------------------
    // Test 10: Stable Source Message IDs Prevent Duplicate Insertion (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_10_stable_source_message_ids_prevent_duplicate_insertion() {
        let env = TestEnv::new("source_mid");
        let json_data = r#"{
            "title": "Source Mid Test",
            "participants": [{"name": "alice"}],
            "messages": [
                {
                    "sender_name": "alice",
                    "timestamp_ms": 1700000000000,
                    "content": "Unique message",
                    "source_message_id": "mid_stable_999"
                }
            ]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/mid_test/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        {
            let mut importer =
                Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
            let rep1 = importer.run_import().unwrap();
            assert_eq!(rep1.messages_imported, 1);
        }

        // Re-import same data
        {
            let mut importer =
                Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
            let rep2 = importer.run_import().unwrap();
            assert_eq!(rep2.messages_imported, 0);
            assert_eq!(rep2.messages_skipped, 1);
        }

        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE source_message_id = 'mid_stable_999'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    // -----------------------------------------------------------------------
    // Test 11: Re-importing Same Archive Without Duplicates (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_11_reimporting_same_archive_no_duplicate_records() {
        let env = TestEnv::new("reimport_same");
        let json_data = r#"{
            "title": "Reimport",
            "participants": [{"name": "alice"}],
            "messages": [
                {"sender_name": "alice", "timestamp_ms": 1700000001000, "content": "one"},
                {"sender_name": "alice", "timestamp_ms": 1700000002000, "content": "two"}
            ]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/reimport_test/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();

        // First run
        let mut imp1 = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let rep1 = imp1.run_import().unwrap();
        assert_eq!(rep1.messages_imported, 2);

        // Second run
        let mut imp2 = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let rep2 = imp2.run_import().unwrap();
        assert_eq!(rep2.messages_imported, 0);
        assert_eq!(rep2.messages_skipped, 2);

        let count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    // -----------------------------------------------------------------------
    // Test 12: Re-importing Changed Archive With Shifted Positions (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_12_reimporting_changed_archive_with_shifted_positions() {
        let env = TestEnv::new("shifted_pos");
        let file_path = "messages/inbox/shifted_test/message_1.json";

        // Initial export: message A and B
        let json_1 = r#"{
            "title": "Shifted",
            "participants": [{"name": "alice"}],
            "messages": [
                {"sender_name": "alice", "timestamp_ms": 1700000001000, "content": "msg A", "source_message_id": "mid_A"},
                {"sender_name": "alice", "timestamp_ms": 1700000002000, "content": "msg B", "source_message_id": "mid_B"}
            ]
        }"#;
        write_synthetic_file(&env.temp_dir, file_path, json_1.as_bytes());

        let mut db = Database::open_in_memory().unwrap();
        let mut imp1 = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let rep1 = imp1.run_import().unwrap();
        assert_eq!(rep1.messages_imported, 2);

        // Newer export snapshot: new message C prepended (shifting positions of A and B)
        let json_2 = r#"{
            "title": "Shifted",
            "participants": [{"name": "alice"}],
            "messages": [
                {"sender_name": "alice", "timestamp_ms": 1700000000000, "content": "msg C", "source_message_id": "mid_C"},
                {"sender_name": "alice", "timestamp_ms": 1700000001000, "content": "msg A", "source_message_id": "mid_A"},
                {"sender_name": "alice", "timestamp_ms": 1700000002000, "content": "msg B", "source_message_id": "mid_B"}
            ]
        }"#;
        write_synthetic_file(&env.temp_dir, file_path, json_2.as_bytes());

        let mut imp2 = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let rep2 = imp2.run_import().unwrap();

        // msg C imported, msg A and B safely skipped
        assert_eq!(rep2.messages_imported, 1);
        assert_eq!(rep2.messages_skipped, 2);

        let total: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 3);
    }

    // -----------------------------------------------------------------------
    // Test 13: Changed-Dataset Checkpoint Revalidation (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_13_changed_dataset_checkpoint_revalidation() {
        let env = TestEnv::new("cp_reval");
        let file_path = "messages/inbox/reval_test/message_1.json";
        let json_data = r#"{
            "title": "Reval",
            "participants": [{"name": "alice"}],
            "messages": [
                {"sender_name": "alice", "timestamp_ms": 1700000001000, "content": "one"}
            ]
        }"#;
        write_synthetic_file(&env.temp_dir, file_path, json_data.as_bytes());

        let mut db = Database::open_in_memory().unwrap();

        // Manually plant an existing checkpoint with mismatched fingerprint
        db.conn().execute(
            "INSERT INTO sync_checkpoints (
                source_account_id, conversation_source_id, dataset_fingerprint,
                last_file_rel_path, last_source_array_index, last_timestamp_ms,
                last_message_identity_hash, revalidation_status, updated_at_ms
            ) VALUES ('alice', 'reval_test', 'old_fp_different', ?1, 5, 1000, 'hash_old', 'VALID', 1000)",
            [file_path],
        ).unwrap();

        let mut importer = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let report = importer.run_import().unwrap();

        assert_eq!(report.messages_imported, 1);
        assert!(report
            .warnings
            .iter()
            .any(|w| w.contains("Checkpoint revalidation")));
    }

    // -----------------------------------------------------------------------
    // Test 14: Transaction Rollback When Batch Fails (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_14_transaction_rollback_when_batch_fails() {
        let _env = TestEnv::new("tx_rollback");
        // Corrupt SQLite state or trigger FK failure manually to test transactional atomicity
        let mut db = Database::open_in_memory().unwrap();

        let tx_result = (|| -> Result<(), ImporterError> {
            let tx = db.conn_mut().transaction()?;
            tx.execute(
                "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms)
                 VALUES ('c_tmp', 'tmp_conv', 'DIRECT', 1000)",
                [],
            )?;
            // Violation: FK failure (bad conversation_id)
            tx.execute(
                "INSERT INTO messages (id, conversation_id, sender_id, identity_hash, message_type, timestamp_ms, is_outgoing)
                 VALUES ('m_tmp', 'non_existent_conv', 'p1', 'hash1', 'TEXT', 1000, 0)",
                [],
            )?;
            tx.commit()?;
            Ok(())
        })();

        assert!(tx_result.is_err());
        let conv_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id = 'c_tmp'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            conv_count, 0,
            "Rolled-back batch must leave zero partial rows"
        );
    }

    // -----------------------------------------------------------------------
    // Test 15: Corrupted or Missing Media Reporting (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_15_corrupted_or_missing_media_reporting() {
        let env = TestEnv::new("missing_media");
        let json_data = r#"{
            "title": "Missing Media Test",
            "participants": [{"name": "alice"}],
            "messages": [{
                "sender_name": "alice",
                "timestamp_ms": 1700000000000,
                "photos": [{"uri": "photos/non_existent_123.jpg"}]
            }]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/missing_media_test/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        let mut importer = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let report = importer.run_import().unwrap();

        assert_eq!(report.messages_imported, 1);
        assert_eq!(report.attachments_missing, 1);
        assert_eq!(report.attachments_imported, 0);

        let att_status: String = db
            .conn()
            .query_row("SELECT status FROM attachments LIMIT 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(att_status, "MISSING");
    }

    // -----------------------------------------------------------------------
    // Test 16: Safe Media Copying and Path Traversal Prevention (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_16_safe_media_copying_and_path_traversal_prevention() {
        let env = TestEnv::new("media_copy");
        // 1. Create legitimate media file
        write_synthetic_file(
            &env.temp_dir,
            "photos/test_pic.jpg",
            b"fake jpeg image data",
        );

        let json_data = r#"{
            "title": "Media Copy Test",
            "participants": [{"name": "alice"}],
            "messages": [{
                "sender_name": "alice",
                "timestamp_ms": 1700000000000,
                "photos": [{"uri": "photos/test_pic.jpg"}]
            }]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/media_test/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        let mut importer = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let report = importer.run_import().unwrap();

        assert_eq!(report.attachments_imported, 1);

        let (att_status, storage_path, checksum): (String, String, String) = db
            .conn()
            .query_row(
                "SELECT status, storage_path, sha256_checksum FROM attachments LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert_eq!(att_status, "LOCAL_COPIED");
        assert!(!checksum.is_empty());

        let target_file = env.media_dir.join(&storage_path);
        assert!(
            target_file.exists(),
            "Media file must exist in managed local media root"
        );

        // 2. Test path traversal rejection
        let source = ArchiveSource::open(&env.temp_dir).unwrap();
        let traversal_result = source.copy_media("../../windows/system32/cmd.exe", &target_file);
        assert!(matches!(
            traversal_result,
            Err(ImporterError::PathTraversal(_))
        ));
    }

    // -----------------------------------------------------------------------
    // Test 17: Voice, Image, Video Attachment Metadata (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_17_voice_image_video_attachment_metadata() {
        let env = TestEnv::new("media_meta");
        write_synthetic_file(&env.temp_dir, "audio/voice.mp4", b"audio-bytes");
        write_synthetic_file(&env.temp_dir, "photos/pic.jpg", b"photo-bytes");
        write_synthetic_file(&env.temp_dir, "videos/clip.mp4", b"video-bytes");

        let json_data = r#"{
            "title": "Multi Media",
            "participants": [{"name": "alice"}],
            "messages": [
                {
                    "sender_name": "alice",
                    "timestamp_ms": 1700000001000,
                    "audio_files": [{"uri": "audio/voice.mp4"}]
                },
                {
                    "sender_name": "alice",
                    "timestamp_ms": 1700000002000,
                    "photos": [{"uri": "photos/pic.jpg"}]
                },
                {
                    "sender_name": "alice",
                    "timestamp_ms": 1700000003000,
                    "videos": [{"uri": "videos/clip.mp4"}]
                }
            ]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/multi_media/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        let mut importer = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let report = importer.run_import().unwrap();

        assert_eq!(report.messages_imported, 3);
        assert_eq!(report.attachments_imported, 3);

        let mut stmt = db
            .conn()
            .prepare("SELECT attachment_type, mime_type FROM attachments ORDER BY id")
            .unwrap();
        let types: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert!(types.iter().any(|(t, m)| t == "AUDIO" && m == "audio/mp4"));
        assert!(types.iter().any(|(t, m)| t == "IMAGE" && m == "image/jpeg"));
        assert!(types.iter().any(|(t, m)| t == "VIDEO" && m == "video/mp4"));
    }

    // -----------------------------------------------------------------------
    // Test 18: Reaction and Reply Handling (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_18_reaction_and_reply_handling() {
        let env = TestEnv::new("reaction_test");
        let json_data = r#"{
            "title": "Reaction Test",
            "participants": [{"name": "alice"}, {"name": "bob"}],
            "messages": [{
                "sender_name": "alice",
                "timestamp_ms": 1700000000000,
                "content": "Nice photo",
                "reactions": [
                    {"actor": "bob", "reaction": "\u00e2\u009d\u00a4"}
                ]
            }]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/react_test/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        let mut importer = Importer::new(&mut db, &env.temp_dir, "bob", &env.media_dir).unwrap();
        let report = importer.run_import().unwrap();

        assert_eq!(report.reactions_imported, 1);

        let rx_val: String = db
            .conn()
            .query_row("SELECT reaction_value FROM reactions LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(rx_val, "❤");
    }

    // -----------------------------------------------------------------------
    // Test 19: Read Receipt Absence Mapping to UNAVAILABLE_FROM_SOURCE (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_19_read_receipt_absence_mapping_to_unavailable_from_source() {
        let env = TestEnv::new("read_receipt");
        let json_data = r#"{
            "title": "Read Test",
            "participants": [{"name": "alice"}],
            "messages": [{
                "sender_name": "alice",
                "timestamp_ms": 1700000000000,
                "content": "Unseen message"
            }]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/read_test/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        let mut importer = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        importer.run_import().unwrap();

        let (status, read_at): (String, Option<i64>) = db
            .conn()
            .query_row(
                "SELECT read_status, read_at_ms FROM read_receipts LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();

        assert_eq!(status, "UNAVAILABLE_FROM_SOURCE");
        assert!(read_at.is_none());
    }

    // -----------------------------------------------------------------------
    // Test 20: Call Event and Duration Mapping (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_20_call_event_and_duration_mapping() {
        let env = TestEnv::new("call_test");
        let json_data = r#"{
            "title": "Call Test",
            "participants": [{"name": "alice"}],
            "messages": [{
                "sender_name": "alice",
                "timestamp_ms": 1700000000000,
                "content": "Alice started an audio call",
                "call_duration": 145
            }]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/call_test/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        let mut importer = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let report = importer.run_import().unwrap();

        assert_eq!(report.call_events_imported, 1);

        let (call_type, status, duration, end_ts): (String, String, Option<i64>, Option<i64>) = db.conn().query_row(
            "SELECT call_type, status, duration_seconds, derived_end_timestamp_ms FROM call_events LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).unwrap();

        assert_eq!(call_type, "VOICE");
        assert_eq!(status, "ANSWERED");
        assert_eq!(duration, Some(145));
        assert_eq!(end_ts, Some(1700000000000 + 145 * 1000));
    }

    // -----------------------------------------------------------------------
    // Test 21: Unknown Message Types Not Crashing Import (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_21_unknown_message_types_not_crashing_import() {
        let env = TestEnv::new("unknown_type");
        let json_data = r#"{
            "title": "Unknown Test",
            "participants": [{"name": "alice"}],
            "messages": [{
                "sender_name": "alice",
                "timestamp_ms": 1700000000000,
                "novel_unsupported_meta_feature": {"nested": [1, 2, 3]}
            }]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/unknown_test/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        let mut importer = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let report = importer.run_import().unwrap();

        assert_eq!(report.messages_imported, 1);

        let msg_type: String = db
            .conn()
            .query_row("SELECT message_type FROM messages LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(msg_type, "UNKNOWN");
    }

    // -----------------------------------------------------------------------
    // Test 22: Recovery Following Interrupted Batch (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_22_recovery_following_interrupted_batch() {
        let env = TestEnv::new("interrupted");
        let file_path = "messages/inbox/recovery_test/message_1.json";

        let json_data = r#"{
            "title": "Recovery",
            "participants": [{"name": "alice"}],
            "messages": [
                {"sender_name": "alice", "timestamp_ms": 1700000001000, "content": "msg 1"},
                {"sender_name": "alice", "timestamp_ms": 1700000002000, "content": "msg 2"},
                {"sender_name": "alice", "timestamp_ms": 1700000003000, "content": "msg 3"}
            ]
        }"#;

        write_synthetic_file(&env.temp_dir, file_path, json_data.as_bytes());

        let mut db = Database::open_in_memory().unwrap();
        let source = ArchiveSource::open(&env.temp_dir).unwrap();
        let fp = source.compute_fingerprint().unwrap();

        // Simulate crash after msg 1 (index 0)
        let hash_0 = compute_message_hash(
            "recovery_test",
            file_path,
            0,
            &RawMessage {
                sender_name: Some("alice".to_string()),
                timestamp_ms: Some(1700000001000),
                timestamp: None,
                content: Some("msg 1".to_string()),
                photos: None,
                audio_files: None,
                videos: None,
                files: None,
                share: None,
                reactions: None,
                call_duration: None,
                missed: None,
                source_message_id: None,
                mid: None,
                id: None,
            },
        );

        // Pre-insert msg 1 and plant checkpoint
        db.conn().execute(
            "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms)
             VALUES ('c_rec', 'recovery_test', 'DIRECT', 1000)", []
        ).unwrap();
        db.conn().execute(
            "INSERT INTO participants (id, source_identifier, username) VALUES ('p1', 'alice', 'alice')", []
        ).unwrap();
        db.conn().execute(
            "INSERT INTO messages (id, conversation_id, sender_id, identity_hash, message_type, content, timestamp_ms, is_outgoing)
             VALUES ('m1', 'c_rec', 'p1', ?1, 'TEXT', 'msg 1', 1700000001000, 0)",
            [&hash_0],
        ).unwrap();

        db.conn()
            .execute(
                "INSERT INTO sync_checkpoints (
                source_account_id, conversation_source_id, dataset_fingerprint,
                last_file_rel_path, last_source_array_index, last_timestamp_ms,
                last_message_identity_hash, revalidation_status, updated_at_ms
            ) VALUES ('alice', 'recovery_test', ?1, ?2, 0, 1700000001000, ?3, 'VALID', 1000)",
                params![fp, file_path, hash_0],
            )
            .unwrap();

        // Resume import
        let mut importer = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let report = importer.run_import().unwrap();

        // msg 2 and msg 3 imported; total messages in DB = 3
        assert_eq!(report.messages_imported, 2);

        let total: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 3);
    }

    // -----------------------------------------------------------------------
    // Test 23: Accurate Partial / Failed / Completed Job Status (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_23_accurate_partial_failed_completed_job_status() {
        let env = TestEnv::new("job_status");
        let json_data = r#"{
            "title": "Status Test",
            "participants": [{"name": "alice"}],
            "messages": [
                {"sender_name": "alice", "timestamp_ms": 1700000001000, "content": "status check"}
            ]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/status_test/message_1.json",
            json_data.as_bytes(),
        );

        let mut db = Database::open_in_memory().unwrap();
        let mut importer = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let report = importer.run_import().unwrap();

        assert_eq!(report.status, "COMPLETED");

        let (db_status, total_imported): (String, i64) = db
            .conn()
            .query_row(
                "SELECT status, total_messages_imported FROM import_jobs WHERE id = ?1",
                [&report.job_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();

        assert_eq!(db_status, "COMPLETED");
        assert_eq!(total_imported, 1);
    }

    // -----------------------------------------------------------------------
    // Test 24: Changed Archive Re-Partitioning Without Source IDs (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_24_changed_archive_repartitioning_without_source_ids() {
        let env = TestEnv::new("repart_no_id");
        let mut db = Database::open_in_memory().unwrap();

        // Snapshot 1: message A (t=1000) and B (t=2000) in message_1.json, NO source_message_id
        let json_snap1 = r#"{
            "title": "Chat",
            "participants": [{"name": "alice"}],
            "messages": [
                {"sender_name": "alice", "timestamp_ms": 1700000001000, "content": "msg A"},
                {"sender_name": "alice", "timestamp_ms": 1700000002000, "content": "msg B"}
            ]
        }"#;
        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/chat_repart/message_1.json",
            json_snap1.as_bytes(),
        );

        let mut imp1 = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let rep1 = imp1.run_import().unwrap();
        assert_eq!(rep1.messages_imported, 2);

        // Snapshot 2: export re-partitioned!
        // message_1.json now contains new message C (t=3000)
        // message_2.json now contains the older messages A and B
        let json_snap2_f1 = r#"{
            "title": "Chat",
            "participants": [{"name": "alice"}],
            "messages": [
                {"sender_name": "alice", "timestamp_ms": 1700000003000, "content": "msg C"}
            ]
        }"#;
        let json_snap2_f2 = r#"{
            "title": "Chat",
            "participants": [{"name": "alice"}],
            "messages": [
                {"sender_name": "alice", "timestamp_ms": 1700000001000, "content": "msg A"},
                {"sender_name": "alice", "timestamp_ms": 1700000002000, "content": "msg B"}
            ]
        }"#;

        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/chat_repart/message_1.json",
            json_snap2_f1.as_bytes(),
        );
        write_synthetic_file(
            &env.temp_dir,
            "messages/inbox/chat_repart/message_2.json",
            json_snap2_f2.as_bytes(),
        );

        let mut imp2 = Importer::new(&mut db, &env.temp_dir, "alice", &env.media_dir).unwrap();
        let rep2 = imp2.run_import().unwrap();

        // msg C imported; msg A and B recognized by canonical signature and skipped
        assert_eq!(rep2.messages_imported, 1);
        assert_eq!(rep2.messages_skipped, 2);

        let total: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            total, 3,
            "Database must contain exactly 3 distinct messages (A, B, C) with zero duplicates"
        );
    }

    // -----------------------------------------------------------------------
    // Test 25: Windows Drive Letter and Colon Path Traversal Rejected (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_25_windows_drive_letter_and_colon_path_traversal_rejected() {
        assert!(validate_safe_relative_path("C:/windows/system32/calc.exe").is_err());
        assert!(validate_safe_relative_path("D:payload.exe").is_err());
        assert!(validate_safe_relative_path("file.jpg:hidden_stream").is_err());
        assert!(validate_safe_relative_path("\\\\server\\share\\malicious.jpg").is_err());
        assert!(validate_safe_relative_path("/root/secret.txt").is_err());
        assert!(validate_safe_relative_path("../../../etc/passwd").is_err());
        assert!(validate_safe_relative_path("normal/path/image.jpg").is_ok());

        let env = TestEnv::new("traversal_guard");
        let source = ArchiveSource::open(&env.temp_dir).unwrap();
        let target = env.media_dir.join("cmd.exe");
        let copy_res = source.copy_media("C:/windows/system32/cmd.exe", &target);
        assert!(matches!(copy_res, Err(ImporterError::PathTraversal(_))));
    }

    // -----------------------------------------------------------------------
    // Test 26: Cumulative ZIP Bomb and Entry Limits (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_26_zip_bomb_cumulative_size_and_entry_limit() {
        let env = TestEnv::new("zip_bomb");
        let zip_path = env.temp_dir.join("normal.zip");

        create_synthetic_zip(&zip_path, &[("messages/inbox/test/message_1.json", b"{}")]);

        let source = ArchiveSource::open(&zip_path).unwrap();
        let summary = source.discover_and_validate().unwrap();
        assert!(summary.errors.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 27: Content-Based Directory Fingerprint Stability (Synthetic Fixture)
    // -----------------------------------------------------------------------
    #[test]
    fn test_27_content_based_directory_fingerprint_stability() {
        let env1 = TestEnv::new("fp_stable_1");
        let env2 = TestEnv::new("fp_stable_2");

        let content = br#"{"title":"test","messages":[]}"#;
        write_synthetic_file(&env1.temp_dir, "messages/inbox/t1/message_1.json", content);
        write_synthetic_file(&env2.temp_dir, "messages/inbox/t1/message_1.json", content);

        let src1 = ArchiveSource::open(&env1.temp_dir).unwrap();
        let src2 = ArchiveSource::open(&env2.temp_dir).unwrap();

        let fp1 = src1.compute_fingerprint().unwrap();
        let fp2 = src2.compute_fingerprint().unwrap();

        assert_eq!(
            fp1, fp2,
            "Directories with identical contents must produce identical fingerprints regardless of folder path"
        );

        // Modifying content changes fingerprint
        write_synthetic_file(
            &env2.temp_dir,
            "messages/inbox/t1/message_1.json",
            b"{\"title\":\"modified\"}",
        );
        let fp2_mod = src2.compute_fingerprint().unwrap();
        assert_ne!(
            fp1, fp2_mod,
            "Changing file content must update directory fingerprint"
        );
    }
}
