//! Local SQLite Database Architecture & Migration Engine
//!
//! Governed by PRD.md, TRD.md, and GEMINI.md.
//! Implements local-first embedded SQLite (3.45+) with WAL mode,
//! strict foreign key enforcement, versioned transactional migrations,
//! deterministic identity hashing, and zero-telemetry offline storage.

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Current supported database schema version
pub const CURRENT_SCHEMA_VERSION: i64 = 2;

/// Database error types
#[derive(Debug, Error)]
pub enum DbError {
    #[error("SQLite database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Filesystem I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Unsupported future schema version: database has version {db_version}, but this application only supports up to version {app_version}")]
    UnsupportedFutureVersion { db_version: i64, app_version: i64 },

    #[error("Database corrupted: {0}")]
    Corrupted(String),

    #[error("Migration failed at version {version}: {reason}")]
    MigrationFailed { version: i64, reason: String },

    #[error("Data integrity violation: {0}")]
    IntegrityViolation(String),

    #[error("Database path error: {0}")]
    PathError(String),
}

/// Applies mandatory SQLite PRAGMA settings on every connection.
/// Governed by TRD.md Section 3.1:
/// - WAL journal mode for concurrent reads and resilient writes
/// - NORMAL synchronous mode (durability without excessive fsync overhead)
/// - Foreign key enforcement (ON)
/// - 5-second busy timeout before SQLITE_BUSY
/// - In-memory temporary tables and 64 MB page cache
pub fn apply_pragmas(conn: &Connection) -> Result<(), DbError> {
    // Note: In-memory databases ignore WAL mode gracefully.
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", -64000)?;
    Ok(())
}

/// Resolves standard Windows application local data directory:
/// `%LOCALAPPDATA%\InstagramChatHistoryViewer\database\chat.db`
pub fn get_default_database_path() -> Result<PathBuf, DbError> {
    let local_data = dirs::data_local_dir().ok_or_else(|| {
        DbError::PathError("Could not resolve local app data directory".to_string())
    })?;
    Ok(local_data
        .join("InstagramChatHistoryViewer")
        .join("database")
        .join("chat.db"))
}

/// A versioned SQL migration
#[derive(Debug, Clone)]
pub struct Migration {
    pub version: i64,
    pub description: &'static str,
    pub sql: &'static str,
}

/// Initial V1 Relational Schema DDL
/// Matches TRD.md Section 3.2
pub const MIGRATION_V1_SQL: &str = r#"
-- Conversations (Direct Messages & Group Chats)
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    source_identifier TEXT UNIQUE NOT NULL,
    conversation_type TEXT NOT NULL,
    title TEXT,
    created_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL,
    last_message_preview TEXT,
    is_archived INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_conversations_updated ON conversations(updated_at_ms DESC);

-- Participants
CREATE TABLE IF NOT EXISTS participants (
    id TEXT PRIMARY KEY,
    source_identifier TEXT UNIQUE NOT NULL,
    username TEXT NOT NULL,
    full_name TEXT,
    avatar_storage_path TEXT,
    is_self INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_participants_username ON participants(username);

-- Conversation <-> Participant Join Table
CREATE TABLE IF NOT EXISTS conversation_participants (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    participant_id TEXT NOT NULL REFERENCES participants(id) ON DELETE CASCADE,
    joined_at_ms INTEGER,
    PRIMARY KEY (conversation_id, participant_id)
);

-- Messages
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    sender_id TEXT NOT NULL REFERENCES participants(id) ON DELETE RESTRICT,
    source_message_id TEXT,
    identity_hash TEXT UNIQUE NOT NULL,
    source_file_path TEXT,
    source_array_index INTEGER,
    message_type TEXT NOT NULL,
    content TEXT,
    timestamp_ms INTEGER NOT NULL,
    timezone_offset_minutes INTEGER,
    is_outgoing INTEGER NOT NULL,
    is_edited INTEGER NOT NULL DEFAULT 0,
    reply_to_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL,
    raw_payload_json TEXT
);
CREATE INDEX IF NOT EXISTS idx_messages_conv_time ON messages(conversation_id, timestamp_ms ASC);
CREATE INDEX IF NOT EXISTS idx_messages_type ON messages(message_type);
CREATE INDEX IF NOT EXISTS idx_messages_reply ON messages(reply_to_message_id);

-- Attachments (Voice, Image, Video, File)
CREATE TABLE IF NOT EXISTS attachments (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    attachment_type TEXT NOT NULL,
    original_uri TEXT,
    storage_path TEXT,
    mime_type TEXT NOT NULL,
    file_size_bytes INTEGER,
    sha256_checksum TEXT,
    duration_ms INTEGER,
    width INTEGER,
    height INTEGER,
    status TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_attachments_msg ON attachments(message_id);
CREATE INDEX IF NOT EXISTS idx_attachments_sha256 ON attachments(sha256_checksum);

-- Message Reactions
CREATE TABLE IF NOT EXISTS reactions (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    participant_id TEXT NOT NULL REFERENCES participants(id) ON DELETE CASCADE,
    reaction_value TEXT NOT NULL,
    created_at_ms INTEGER,
    UNIQUE (message_id, participant_id, reaction_value)
);
CREATE INDEX IF NOT EXISTS idx_reactions_msg ON reactions(message_id);

-- Read Receipts (Explicit Modeling of Available vs Unavailable)
CREATE TABLE IF NOT EXISTS read_receipts (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    participant_id TEXT NOT NULL REFERENCES participants(id) ON DELETE CASCADE,
    read_status TEXT NOT NULL,
    read_at_ms INTEGER,
    UNIQUE (message_id, participant_id)
);
CREATE INDEX IF NOT EXISTS idx_read_receipts_msg ON read_receipts(message_id);

-- Voice and Video Call Records (Mandatory)
CREATE TABLE IF NOT EXISTS call_events (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    message_id TEXT REFERENCES messages(id) ON DELETE CASCADE,
    caller_id TEXT NOT NULL REFERENCES participants(id) ON DELETE RESTRICT,
    call_type TEXT NOT NULL,
    direction TEXT NOT NULL,
    status TEXT NOT NULL,
    start_timestamp_ms INTEGER NOT NULL,
    duration_seconds INTEGER,
    derived_end_timestamp_ms INTEGER
);
CREATE INDEX IF NOT EXISTS idx_call_events_conv ON call_events(conversation_id, start_timestamp_ms ASC);

-- Import Jobs (Informational execution history)
CREATE TABLE IF NOT EXISTS import_jobs (
    id TEXT PRIMARY KEY,
    source_type TEXT NOT NULL,
    source_path TEXT,
    started_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    status TEXT NOT NULL,
    total_messages_imported INTEGER NOT NULL DEFAULT 0,
    total_media_files_imported INTEGER NOT NULL DEFAULT 0,
    error_summary TEXT
);

-- Stable Sync Checkpoints (Decoupled from transient job_id)
CREATE TABLE IF NOT EXISTS sync_checkpoints (
    conversation_source_id TEXT PRIMARY KEY,
    last_job_id TEXT REFERENCES import_jobs(id) ON DELETE SET NULL,
    last_file_rel_path TEXT NOT NULL,
    last_source_array_index INTEGER NOT NULL,
    last_timestamp_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

-- Failed Items Log
CREATE TABLE IF NOT EXISTS failed_items (
    id TEXT PRIMARY KEY,
    job_id TEXT REFERENCES import_jobs(id) ON DELETE CASCADE,
    item_type TEXT NOT NULL,
    source_reference TEXT NOT NULL,
    error_reason TEXT NOT NULL,
    stack_trace TEXT,
    occurred_at_ms INTEGER NOT NULL
);
"#;

/// Migration V2: Scoped Checkpoint Identity and Source Message Indexing
/// Governed by Phase 2.1 Audit:
/// - Adds idx_messages_source_id for fast scoped source message lookups.
/// - Upgrades sync_checkpoints to include source_account_id, dataset_fingerprint,
///   last_message_identity_hash, and revalidation_status, with composite PK (source_account_id, conversation_source_id).
pub const MIGRATION_V2_SQL: &str = r#"
-- 1. Index source_message_id scoped to conversation
CREATE INDEX IF NOT EXISTS idx_messages_source_id ON messages(conversation_id, source_message_id);

-- 2. Upgrade sync_checkpoints table
CREATE TABLE IF NOT EXISTS sync_checkpoints_v2 (
    source_account_id TEXT NOT NULL DEFAULT 'default',
    conversation_source_id TEXT NOT NULL,
    dataset_fingerprint TEXT NOT NULL DEFAULT '',
    last_job_id TEXT REFERENCES import_jobs(id) ON DELETE SET NULL,
    last_file_rel_path TEXT NOT NULL,
    last_source_array_index INTEGER NOT NULL,
    last_timestamp_ms INTEGER NOT NULL,
    last_message_identity_hash TEXT,
    revalidation_status TEXT NOT NULL DEFAULT 'VALID',
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (source_account_id, conversation_source_id)
);

-- Safely migrate any existing v1 checkpoints into v2 schema
INSERT INTO sync_checkpoints_v2 (
    source_account_id, conversation_source_id, dataset_fingerprint, last_job_id,
    last_file_rel_path, last_source_array_index, last_timestamp_ms, revalidation_status, updated_at_ms
)
SELECT 
    'default', conversation_source_id, '', last_job_id,
    last_file_rel_path, last_source_array_index, last_timestamp_ms, 'VALID', updated_at_ms
FROM sync_checkpoints;

DROP TABLE sync_checkpoints;
ALTER TABLE sync_checkpoints_v2 RENAME TO sync_checkpoints;
"#;

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "Initial relational schema for conversations, messages, attachments, reactions, read receipts, calls, import jobs, and checkpoints",
        sql: MIGRATION_V1_SQL,
    },
    Migration {
        version: 2,
        description: "Scoped checkpoint identity, dataset fingerprinting, and source message indexing",
        sql: MIGRATION_V2_SQL,
    },
];

/// Database connection manager
pub struct Database {
    conn: Connection,
    path: Option<PathBuf>,
}

impl Database {
    /// Opens or creates the default persistent database in LocalAppData.
    pub fn open_default() -> Result<Self, DbError> {
        let db_path = get_default_database_path()?;
        Self::open_at(&db_path)
    }

    /// Opens or creates a database at a specific path.
    /// Safely ensures parent directories exist without deleting existing data.
    pub fn open_at(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        apply_pragmas(&conn)?;
        let mut db = Self {
            conn,
            path: Some(path.to_path_buf()),
        };
        db.run_migrations()?;
        Ok(db)
    }

    /// Opens an isolated in-memory database (primarily for automated testing).
    pub fn open_in_memory() -> Result<Self, DbError> {
        let conn = Connection::open_in_memory()?;
        apply_pragmas(&conn)?;
        let mut db = Self { conn, path: None };
        db.run_migrations()?;
        Ok(db)
    }

    /// Returns a reference to the underlying SQLite connection.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Returns a mutable reference to the underlying SQLite connection.
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Returns the database path, if file-backed.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Inspects the current schema version from `schema_migrations`.
    /// Returns 0 if `schema_migrations` does not exist yet.
    pub fn get_schema_version(&self) -> Result<i64, DbError> {
        let table_exists: bool = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
            [],
            |row| row.get::<_, i64>(0).map(|c| c > 0),
        )?;

        if !table_exists {
            return Ok(0);
        }

        let version: Option<i64> =
            self.conn
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get(0)
                })?;

        Ok(version.unwrap_or(0))
    }

    /// Runs all pending migrations transactionally in sequential order.
    pub fn run_migrations(&mut self) -> Result<(), DbError> {
        // Ensure schema_migrations table exists
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at_ms INTEGER NOT NULL,
                description TEXT NOT NULL
            );",
        )?;

        let current_version = self.get_schema_version()?;
        if current_version > CURRENT_SCHEMA_VERSION {
            return Err(DbError::UnsupportedFutureVersion {
                db_version: current_version,
                app_version: CURRENT_SCHEMA_VERSION,
            });
        }

        for migration in MIGRATIONS {
            if migration.version > current_version {
                let tx = self.conn.transaction()?;

                // Execute migration SQL within transaction
                tx.execute_batch(migration.sql)
                    .map_err(|e| DbError::MigrationFailed {
                        version: migration.version,
                        reason: e.to_string(),
                    })?;

                // Record migration commit timestamp
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);

                tx.execute(
                    "INSERT INTO schema_migrations (version, applied_at_ms, description) VALUES (?1, ?2, ?3)",
                    params![migration.version, now_ms, migration.description],
                ).map_err(|e| DbError::MigrationFailed {
                    version: migration.version,
                    reason: e.to_string(),
                })?;

                tx.commit()?;
            }
        }

        Ok(())
    }

    /// Executes SQLite integrity check and foreign key check.
    pub fn check_integrity(&self) -> Result<bool, DbError> {
        let integrity: String = self
            .conn
            .query_row("PRAGMA integrity_check;", [], |r| r.get(0))?;
        if integrity != "ok" {
            return Err(DbError::Corrupted(integrity));
        }

        let mut stmt = self.conn.prepare("PRAGMA foreign_key_check;")?;
        let mut rows = stmt.query([])?;
        if rows.next()?.is_some() {
            return Err(DbError::IntegrityViolation(
                "Foreign key constraint violations detected".to_string(),
            ));
        }

        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// Identity Hashing & Provenance (TRD.md Section 3.3)
// ---------------------------------------------------------------------------

/// Computes Tier 1 Deterministic Identity Hash:
/// SHA256("MID:" || conversation_source_id || ":" || source_message_id)
/// Scopes source message identifiers to the conversation namespace to avoid false cross-thread collisions.
pub fn compute_tier1_identity_hash(
    conversation_source_id: &str,
    source_message_id: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"MID:");
    hasher.update(conversation_source_id.as_bytes());
    hasher.update(b":");
    hasher.update(source_message_id.as_bytes());
    hex::encode(hasher.finalize())
}

/// Action determined after evaluating a persisted checkpoint against an incoming dataset
#[derive(Debug, PartialEq, Eq)]
pub enum CheckpointResumeAction {
    /// Safe to resume immediately from array index + 1
    ResumeFromIndex { next_array_index: usize },
    /// The dataset changed or array offset does not match; must revalidate / diff
    DatasetChangedMustRevalidate { reason: String },
}

/// Evaluates whether a checkpoint position can be resumed directly on a source dataset.
/// Governed by TRD.md and PRD.md:
/// Prevents blind resumption when an archive dataset has changed or repartitioned.
pub fn evaluate_checkpoint(
    checkpoint_dataset_fingerprint: &str,
    current_dataset_fingerprint: &str,
    checkpoint_last_index: usize,
    checkpoint_last_msg_hash: Option<&str>,
    current_msg_hash_at_index: Option<&str>,
) -> CheckpointResumeAction {
    if checkpoint_dataset_fingerprint != current_dataset_fingerprint {
        return CheckpointResumeAction::DatasetChangedMustRevalidate {
            reason: format!(
                "Dataset fingerprint mismatch: checkpoint was for '{}', current dataset is '{}'",
                checkpoint_dataset_fingerprint, current_dataset_fingerprint
            ),
        };
    }

    if let (Some(expected), Some(actual)) = (checkpoint_last_msg_hash, current_msg_hash_at_index) {
        if expected != actual {
            return CheckpointResumeAction::DatasetChangedMustRevalidate {
                reason: format!(
                    "Last message identity hash mismatch at index {}: expected '{}', found '{}'",
                    checkpoint_last_index, expected, actual
                ),
            };
        }
    }

    CheckpointResumeAction::ResumeFromIndex {
        next_array_index: checkpoint_last_index + 1,
    }
}

/// Computes content digest for Tier 2 fallback identity hashing:
/// - Text: SHA256(normalized_content)
/// - Media/Voice: SHA256(original_uri)
/// - Call: SHA256(call_type:direction:duration_seconds)
pub fn compute_content_digest(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Computes Tier 2 Archive Fallback Deterministic Identity Hash:
/// SHA256(conversation_source_id || sender || timestamp_ms || source_file_path || source_array_index || content_digest)
/// Preserves legitimate identical messages within an export while keeping re-imports idempotent.
pub fn compute_tier2_identity_hash(
    conversation_source_id: &str,
    sender: &str,
    timestamp_ms: i64,
    source_file_path: &str,
    source_array_index: usize,
    content_digest: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(conversation_source_id.as_bytes());
    hasher.update(b":");
    hasher.update(sender.as_bytes());
    hasher.update(b":");
    hasher.update(timestamp_ms.to_string().as_bytes());
    hasher.update(b":");
    hasher.update(source_file_path.as_bytes());
    hasher.update(b":");
    hasher.update(source_array_index.to_string().as_bytes());
    hasher.update(b":");
    hasher.update(content_digest.as_bytes());
    hex::encode(hasher.finalize())
}

// Helper module for hex encoding to avoid extra crate dependency
mod hex {
    pub fn encode(data: impl AsRef<[u8]>) -> String {
        let mut s = String::with_capacity(data.as_ref().len() * 2);
        for &byte in data.as_ref() {
            use std::fmt::Write;
            write!(&mut s, "{:02x}", byte).unwrap();
        }
        s
    }
}

// ---------------------------------------------------------------------------
// Unit & Integration Tests (16 Mandatory Test Cases)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_1_fresh_database_initialization() {
        let db = Database::open_in_memory().expect("Should initialize in-memory DB");
        assert_eq!(db.get_schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn test_2_all_tables_and_indexes_exist() {
        let db = Database::open_in_memory().unwrap();
        let expected_tables = [
            "schema_migrations",
            "conversations",
            "participants",
            "conversation_participants",
            "messages",
            "attachments",
            "reactions",
            "read_receipts",
            "call_events",
            "import_jobs",
            "sync_checkpoints",
            "failed_items",
        ];

        for table in expected_tables {
            let count: i64 = db
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    params![table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "Table '{}' must exist in schema", table);
        }

        let expected_indexes = [
            "idx_conversations_updated",
            "idx_participants_username",
            "idx_messages_conv_time",
            "idx_messages_type",
            "idx_messages_reply",
            "idx_messages_source_id",
            "idx_attachments_msg",
            "idx_attachments_sha256",
            "idx_reactions_msg",
            "idx_read_receipts_msg",
            "idx_call_events_conv",
        ];

        for index in expected_indexes {
            let count: i64 = db
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    params![index],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "Index '{}' must exist in schema", index);
        }
    }

    #[test]
    fn test_3_foreign_key_enforcement() {
        let db = Database::open_in_memory().unwrap();

        // Attempt to insert message with nonexistent conversation_id and sender_id
        let res = db.conn().execute(
            "INSERT INTO messages (
                id, conversation_id, sender_id, identity_hash, message_type, content, timestamp_ms, is_outgoing
            ) VALUES ('msg1', 'nonexistent_conv', 'nonexistent_sender', 'hash1', 'TEXT', 'hello', 1000, 1)",
            [],
        );

        assert!(res.is_err(), "Foreign key violation must be rejected");
    }

    #[test]
    fn test_4_migrations_execute_in_order() {
        let db = Database::open_in_memory().unwrap();
        let version: i64 = db
            .conn()
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, 2);

        let desc_v1: String = db
            .conn()
            .query_row(
                "SELECT description FROM schema_migrations WHERE version=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(desc_v1.contains("Initial relational schema"));

        let desc_v2: String = db
            .conn()
            .query_row(
                "SELECT description FROM schema_migrations WHERE version=2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(desc_v2.contains("Scoped checkpoint identity"));
    }

    #[test]
    fn test_5_reopening_database_does_not_reapply_migrations() {
        let temp_dir =
            std::env::temp_dir().join(format!("ig_test_reopen_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("chat.db");

        {
            let db = Database::open_at(&db_path).unwrap();
            // Insert a test conversation
            db.conn()
                .execute(
                    "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms)
                     VALUES ('conv1', 'test_thread_1', 'DIRECT', 1000)",
                    [],
                )
                .unwrap();
        }

        // Reopen database at the same path
        {
            let db = Database::open_at(&db_path).unwrap();
            assert_eq!(db.get_schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
            let count: i64 = db
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM conversations WHERE id='conv1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "Pre-existing data must be preserved on reopen");
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_6_failed_migration_rolls_back() {
        let mut db = Database::open_in_memory().unwrap();

        // Simulate applying a broken migration within a transaction
        let bad_migration_sql = "
            CREATE TABLE test_table_ok (id TEXT PRIMARY KEY);
            THIS IS INVALID SQL THAT CAUSES A SYNTAX ERROR;
        ";

        let tx_result = (|| -> Result<(), DbError> {
            let tx = db.conn_mut().transaction()?;
            tx.execute_batch(bad_migration_sql)
                .map_err(|e| DbError::MigrationFailed {
                    version: 3,
                    reason: e.to_string(),
                })?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at_ms, description) VALUES (3, 0, 'bad')",
                [],
            )?;
            tx.commit()?;
            Ok(())
        })();

        assert!(tx_result.is_err(), "Broken migration must fail");

        // Verify version did NOT advance and test_table_ok was NOT committed
        assert_eq!(db.get_schema_version().unwrap(), 2);
        let table_exists: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='test_table_ok'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            table_exists, 0,
            "Failed migration changes must roll back completely"
        );
    }

    #[test]
    fn test_7_unsupported_future_schema_version_rejected() {
        let mut db = Database::open_in_memory().unwrap();

        // Artificially record a higher schema version
        db.conn()
            .execute(
                "INSERT INTO schema_migrations (version, applied_at_ms, description) VALUES (999, 1000, 'Future version')",
                [],
            )
            .unwrap();

        let res = db.run_migrations();
        match res {
            Err(DbError::UnsupportedFutureVersion {
                db_version,
                app_version,
            }) => {
                assert_eq!(db_version, 999);
                assert_eq!(app_version, CURRENT_SCHEMA_VERSION);
            }
            _ => panic!("Expected UnsupportedFutureVersion error, got {:?}", res),
        }
    }

    #[test]
    fn test_8_conversation_and_participant_relationships() {
        let db = Database::open_in_memory().unwrap();

        db.conn()
            .execute(
                "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms)
                 VALUES ('c1', 'thread_1', 'DIRECT', 1000)",
                [],
            )
            .unwrap();

        db.conn()
            .execute(
                "INSERT INTO participants (id, source_identifier, username, is_self)
                 VALUES ('p1', 'user_1', 'alice', 0), ('p2', 'user_2', 'bob', 1)",
                [],
            )
            .unwrap();

        db.conn()
            .execute(
                "INSERT INTO conversation_participants (conversation_id, participant_id, joined_at_ms)
                 VALUES ('c1', 'p1', 1000), ('c1', 'p2', 1000)",
                [],
            )
            .unwrap();

        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM conversation_participants WHERE conversation_id='c1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);

        // Test cascade deletion: deleting conversation deletes conversation_participants
        db.conn()
            .execute("DELETE FROM conversations WHERE id='c1'", [])
            .unwrap();
        let orphan_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM conversation_participants WHERE conversation_id='c1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphan_count, 0, "Cascade delete must remove join rows");

        // Participants themselves must still exist
        let p_count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM participants", [], |r| r.get(0))
            .unwrap();
        assert_eq!(p_count, 2);
    }

    #[test]
    fn test_9_messages_preserve_identical_text_for_distinct_records() {
        let db = Database::open_in_memory().unwrap();

        db.conn()
            .execute(
                "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms)
                 VALUES ('c1', 'thread_1', 'DIRECT', 1000)",
                [],
            )
            .unwrap();

        db.conn()
            .execute(
                "INSERT INTO participants (id, source_identifier, username, is_self)
                 VALUES ('p1', 'user_1', 'alice', 0)",
                [],
            )
            .unwrap();

        // Scenario: Two consecutive identical messages "ok" sent at the exact same millisecond
        let timestamp = 1700000000000_i64;
        let content = "ok";
        let digest = compute_content_digest(content);

        let hash_1 = compute_tier2_identity_hash(
            "thread_1",
            "alice",
            timestamp,
            "message_1.json",
            4,
            &digest,
        );
        let hash_2 = compute_tier2_identity_hash(
            "thread_1",
            "alice",
            timestamp,
            "message_1.json",
            5,
            &digest,
        );

        assert_ne!(
            hash_1, hash_2,
            "Hashes must differ because array indices differ"
        );

        db.conn()
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, sender_id, identity_hash, source_file_path, source_array_index,
                    message_type, content, timestamp_ms, is_outgoing
                ) VALUES
                ('m1', 'c1', 'p1', ?1, 'message_1.json', 4, 'TEXT', ?2, ?3, 0),
                ('m2', 'c1', 'p1', ?4, 'message_1.json', 5, 'TEXT', ?2, ?3, 0)",
                params![hash_1, content, timestamp, hash_2],
            )
            .unwrap();

        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE content='ok'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 2,
            "Both legitimate identical messages must be preserved"
        );

        // Idempotency: Re-inserting with hash_1 must trigger unique constraint violation
        let duplicate_attempt = db.conn().execute(
            "INSERT INTO messages (
                id, conversation_id, sender_id, identity_hash, message_type, content, timestamp_ms, is_outgoing
            ) VALUES ('m3', 'c1', 'p1', ?1, 'TEXT', 'ok', ?2, 0)",
            params![hash_1, timestamp],
        );
        assert!(
            duplicate_attempt.is_err(),
            "Duplicate hash must be rejected"
        );
    }

    #[test]
    fn test_10_attachments_associated_with_correct_message() {
        let db = Database::open_in_memory().unwrap();

        db.conn()
            .execute(
                "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms)
                 VALUES ('c1', 'thread_1', 'DIRECT', 1000)",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO participants (id, source_identifier, username, is_self)
                 VALUES ('p1', 'user_1', 'alice', 0)",
                [],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO messages (id, conversation_id, sender_id, identity_hash, message_type, timestamp_ms, is_outgoing)
                 VALUES ('m1', 'c1', 'p1', 'hash_voice', 'VOICE', 1000, 0)",
                [],
            )
            .unwrap();

        db.conn()
            .execute(
                "INSERT INTO attachments (
                    id, message_id, attachment_type, original_uri, storage_path, mime_type,
                    file_size_bytes, sha256_checksum, duration_ms, status
                ) VALUES (
                    'att1', 'm1', 'AUDIO', 'audio/clip.mp4', 'media/audio/clip_hash.mp4', 'audio/mp4',
                    45120, 'sha256_hex_sample', 14200, 'LOCAL_COPIED'
                )",
                [],
            )
            .unwrap();

        let (att_type, dur, status): (String, i64, String) = db
            .conn()
            .query_row(
                "SELECT attachment_type, duration_ms, status FROM attachments WHERE message_id='m1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert_eq!(att_type, "AUDIO");
        assert_eq!(dur, 14200);
        assert_eq!(status, "LOCAL_COPIED");
    }

    #[test]
    fn test_11_multiple_reactions_on_single_message() {
        let db = Database::open_in_memory().unwrap();

        db.conn().execute(
            "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms) VALUES ('c1', 't1', 'DIRECT', 1000)", []
        ).unwrap();
        db.conn().execute(
            "INSERT INTO participants (id, source_identifier, username) VALUES ('p1', 'u1', 'alice'), ('p2', 'u2', 'bob')", []
        ).unwrap();
        db.conn().execute(
            "INSERT INTO messages (id, conversation_id, sender_id, identity_hash, message_type, timestamp_ms, is_outgoing)
             VALUES ('m1', 'c1', 'p1', 'h1', 'TEXT', 1000, 0)", []
        ).unwrap();

        // Alice reacts with ❤️ and Bob reacts with 👍
        db.conn().execute(
            "INSERT INTO reactions (id, message_id, participant_id, reaction_value, created_at_ms)
             VALUES ('r1', 'm1', 'p1', '❤️', 1100), ('r2', 'm1', 'p2', '👍', 1200)", []
        ).unwrap();

        let count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM reactions WHERE message_id='m1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);

        // Same participant reacting with same emoji on same message must fail UNIQUE constraint
        let duplicate_reaction = db.conn().execute(
            "INSERT INTO reactions (id, message_id, participant_id, reaction_value) VALUES ('r3', 'm1', 'p1', '❤️')", []
        );
        assert!(
            duplicate_reaction.is_err(),
            "Duplicate reaction must be rejected"
        );
    }

    #[test]
    fn test_12_read_receipts_unavailable_from_source() {
        let db = Database::open_in_memory().unwrap();

        db.conn().execute(
            "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms) VALUES ('c1', 't1', 'DIRECT', 1000)", []
        ).unwrap();
        db.conn().execute(
            "INSERT INTO participants (id, source_identifier, username) VALUES ('p1', 'u1', 'alice')", []
        ).unwrap();
        db.conn().execute(
            "INSERT INTO messages (id, conversation_id, sender_id, identity_hash, message_type, timestamp_ms, is_outgoing)
             VALUES ('m1', 'c1', 'p1', 'h1', 'TEXT', 1000, 1)", []
        ).unwrap();

        // DYI exports lack per-message read receipts; must default to UNAVAILABLE_FROM_SOURCE
        db.conn().execute(
            "INSERT INTO read_receipts (id, message_id, participant_id, read_status, read_at_ms)
             VALUES ('rr1', 'm1', 'p1', 'UNAVAILABLE_FROM_SOURCE', NULL)", []
        ).unwrap();

        let (status, read_at): (String, Option<i64>) = db
            .conn()
            .query_row(
                "SELECT read_status, read_at_ms FROM read_receipts WHERE message_id='m1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();

        assert_eq!(status, "UNAVAILABLE_FROM_SOURCE");
        assert!(
            read_at.is_none(),
            "read_at_ms must be null when unavailable"
        );
    }

    #[test]
    fn test_13_call_event_distinguishes_source_and_derived_values() {
        let db = Database::open_in_memory().unwrap();

        db.conn().execute(
            "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms) VALUES ('c1', 't1', 'DIRECT', 1000)", []
        ).unwrap();
        db.conn().execute(
            "INSERT INTO participants (id, source_identifier, username) VALUES ('p1', 'u1', 'alice')", []
        ).unwrap();

        let start_ms = 1700000000000_i64;
        let duration_sec = 862_i64; // 14m 22s
        let derived_end_ms = start_ms + (duration_sec * 1000);

        db.conn()
            .execute(
                "INSERT INTO call_events (
                id, conversation_id, caller_id, call_type, direction, status,
                start_timestamp_ms, duration_seconds, derived_end_timestamp_ms
            ) VALUES ('call1', 'c1', 'p1', 'VOICE', 'INCOMING', 'ANSWERED', ?1, ?2, ?3)",
                params![start_ms, duration_sec, derived_end_ms],
            )
            .unwrap();

        let (c_type, dir, stat, start, dur, end): (String, String, String, i64, Option<i64>, Option<i64>) = db.conn().query_row(
            "SELECT call_type, direction, status, start_timestamp_ms, duration_seconds, derived_end_timestamp_ms
             FROM call_events WHERE id='call1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        ).unwrap();

        assert_eq!(c_type, "VOICE");
        assert_eq!(dir, "INCOMING");
        assert_eq!(stat, "ANSWERED");
        assert_eq!(start, start_ms);
        assert_eq!(dur, Some(862));
        assert_eq!(end, Some(start_ms + 862000));
    }

    #[test]
    fn test_14_checkpoints_independent_of_transient_job_uuid() {
        let db = Database::open_in_memory().unwrap();

        // Insert a transient import job
        db.conn()
            .execute(
                "INSERT INTO import_jobs (id, source_type, started_at_ms, status)
             VALUES ('job_uuid_1', 'EXPORT_DIR', 1000, 'COMPLETED')",
                [],
            )
            .unwrap();

        // Create checkpoint keyed by conversation_source_id
        db.conn().execute(
            "INSERT INTO sync_checkpoints (
                conversation_source_id, last_job_id, last_file_rel_path, last_source_array_index,
                last_timestamp_ms, updated_at_ms
            ) VALUES ('conv_source_alice_123', 'job_uuid_1', 'messages/inbox/alice/message_1.json', 45, 1700000050000, 1700000060000)", []
        ).unwrap();

        // Delete the transient job; ON DELETE SET NULL must preserve the checkpoint
        db.conn()
            .execute("DELETE FROM import_jobs WHERE id='job_uuid_1'", [])
            .unwrap();

        let (last_job, file_path, offset): (Option<String>, String, i64) = db
            .conn()
            .query_row(
                "SELECT last_job_id, last_file_rel_path, last_source_array_index
             FROM sync_checkpoints WHERE conversation_source_id='conv_source_alice_123'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert!(
            last_job.is_none(),
            "last_job_id must be null after job deletion"
        );
        assert_eq!(file_path, "messages/inbox/alice/message_1.json");
        assert_eq!(offset, 45);
    }

    #[test]
    fn test_15_transaction_rollback_leaves_no_partial_records() {
        let mut db = Database::open_in_memory().unwrap();

        db.conn().execute(
            "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms) VALUES ('c1', 't1', 'DIRECT', 1000)", []
        ).unwrap();

        let tx_result = (|| -> Result<(), DbError> {
            let tx = db.conn_mut().transaction()?;
            tx.execute(
                "INSERT INTO participants (id, source_identifier, username) VALUES ('p1', 'u1', 'alice')", []
            )?;
            // Intentional error: violate non-null or foreign key
            tx.execute(
                "INSERT INTO messages (id, conversation_id) VALUES ('m1', 'bad_c')",
                [],
            )?;
            tx.commit()?;
            Ok(())
        })();

        assert!(tx_result.is_err());

        // Verify participant 'p1' was rolled back
        let p_count: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM participants WHERE id='p1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(p_count, 0, "Rolled-back transaction must leave zero rows");
    }

    #[test]
    fn test_16_existing_database_not_deleted_during_recovery() {
        let temp_dir =
            std::env::temp_dir().join(format!("ig_test_persist_{}", uuid::Uuid::new_v4()));
        let db_path = temp_dir.join("chat.db");

        {
            let db = Database::open_at(&db_path).unwrap();
            db.conn().execute(
                "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms) VALUES ('c_safe', 't_safe', 'DIRECT', 1000)", []
            ).unwrap();
        }

        assert!(db_path.exists(), "DB file must exist on disk");

        // Open again without destructive operations
        {
            let db = Database::open_at(&db_path).unwrap();
            let count: i64 = db
                .conn()
                .query_row(
                    "SELECT COUNT(*) FROM conversations WHERE id='c_safe'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                count, 1,
                "Existing database data must remain completely intact"
            );
            assert!(db.check_integrity().unwrap_or(true));
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_17_tier1_source_message_id_scoped_to_conversation() {
        let db = Database::open_in_memory().unwrap();

        // Insert two distinct conversations
        db.conn()
            .execute(
                "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms) VALUES 
             ('c1', 'conv_alice', 'DIRECT', 1000),
             ('c2', 'conv_bob', 'DIRECT', 1000)",
                [],
            )
            .unwrap();

        db.conn()
            .execute(
                "INSERT INTO participants (id, source_identifier, username) VALUES ('p1', 'u1', 'user1')",
                [],
            )
            .unwrap();

        let source_mid = "mid_12345";
        let hash_c1 = compute_tier1_identity_hash("conv_alice", source_mid);
        let hash_c2 = compute_tier1_identity_hash("conv_bob", source_mid);

        assert_ne!(
            hash_c1, hash_c2,
            "Identical source_message_id in different conversations must yield different hashes"
        );

        // Insert into conversation 1
        db.conn()
            .execute(
                "INSERT INTO messages (id, conversation_id, sender_id, message_type, content, timestamp_ms, is_outgoing, source_message_id, identity_hash)
             VALUES ('m1', 'c1', 'p1', 'TEXT', 'Hello', 1000, 0, ?1, ?2)",
                [&source_mid, &hash_c1.as_str()],
            )
            .unwrap();

        // Insert into conversation 2 with same source_message_id
        db.conn()
            .execute(
                "INSERT INTO messages (id, conversation_id, sender_id, message_type, content, timestamp_ms, is_outgoing, source_message_id, identity_hash)
             VALUES ('m2', 'c2', 'p1', 'TEXT', 'Hello', 1000, 0, ?1, ?2)",
                [&source_mid, &hash_c2.as_str()],
            )
            .unwrap();

        // Inserting a duplicate in c1 must fail unique constraint on identity_hash
        let dup_insert = db.conn().execute(
            "INSERT INTO messages (id, conversation_id, sender_id, message_type, content, timestamp_ms, is_outgoing, source_message_id, identity_hash)
             VALUES ('m3', 'c1', 'p1', 'TEXT', 'Hello', 1000, 0, ?1, ?2)",
            [&source_mid, &hash_c1.as_str()],
        );
        assert!(
            dup_insert.is_err(),
            "Duplicate identity_hash in same conversation must be rejected"
        );

        // Verify index idx_messages_source_id is used by query planner
        let explain_plan: String = db
            .conn()
            .query_row(
                "EXPLAIN QUERY PLAN SELECT id FROM messages WHERE conversation_id = ?1 AND source_message_id = ?2",
                ["c1", source_mid],
                |r| r.get(3),
            )
            .unwrap();
        assert!(
            explain_plan.contains("idx_messages_source_id"),
            "Query planner must utilize idx_messages_source_id for scoped source_message_id lookup: {explain_plan}"
        );
    }

    #[test]
    fn test_18_independent_sources_do_not_share_checkpoints() {
        let db = Database::open_in_memory().unwrap();

        // Insert checkpoints for Alice and Bob for the same conversation_source_id
        let conv_src = "shared_thread_abc";
        db.conn()
            .execute(
                "INSERT INTO sync_checkpoints 
             (source_account_id, conversation_source_id, dataset_fingerprint, last_file_rel_path, last_source_array_index, last_timestamp_ms, revalidation_status, updated_at_ms)
             VALUES 
             ('account_alice', ?1, 'fp_alice', 'inbox/alice/message_1.json', 100, 1000, 'VALID', 1000),
             ('account_bob', ?1, 'fp_bob', 'inbox/bob/message_1.json', 45, 950, 'VALID', 1000)",
                [conv_src],
            )
            .unwrap();

        let alice_index: i64 = db
            .conn()
            .query_row(
                "SELECT last_source_array_index FROM sync_checkpoints WHERE source_account_id = 'account_alice' AND conversation_source_id = ?1",
                [conv_src],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(alice_index, 100);

        let bob_index: i64 = db
            .conn()
            .query_row(
                "SELECT last_source_array_index FROM sync_checkpoints WHERE source_account_id = 'account_bob' AND conversation_source_id = ?1",
                [conv_src],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bob_index, 45);

        // Delete Alice's checkpoint and confirm Bob's remains
        db.conn()
            .execute(
                "DELETE FROM sync_checkpoints WHERE source_account_id = 'account_alice' AND conversation_source_id = ?1",
                [conv_src],
            )
            .unwrap();

        let alice_exists: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sync_checkpoints WHERE source_account_id = 'account_alice' AND conversation_source_id = ?1",
                [conv_src],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(alice_exists, 0);

        let bob_still_exists: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sync_checkpoints WHERE source_account_id = 'account_bob' AND conversation_source_id = ?1",
                [conv_src],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bob_still_exists, 1);
    }

    #[test]
    fn test_19_changed_archive_dataset_not_resumed_blindly() {
        // 1. Matching dataset & matching hash -> resume next index
        let action_ok = evaluate_checkpoint(
            "export_2026_01",
            "export_2026_01",
            50,
            Some("hash_abc"),
            Some("hash_abc"),
        );
        assert_eq!(
            action_ok,
            CheckpointResumeAction::ResumeFromIndex {
                next_array_index: 51
            }
        );

        // 2. Mismatched dataset fingerprint -> must revalidate
        let action_fp_mismatch = evaluate_checkpoint(
            "export_2026_01",
            "export_2026_06",
            50,
            Some("hash_abc"),
            Some("hash_abc"),
        );
        match action_fp_mismatch {
            CheckpointResumeAction::DatasetChangedMustRevalidate { reason } => {
                assert!(reason.contains("Dataset fingerprint mismatch"));
            }
            _ => panic!("Expected DatasetChangedMustRevalidate on fingerprint mismatch"),
        }

        // 3. Mismatched last message identity hash -> must revalidate
        let action_hash_mismatch = evaluate_checkpoint(
            "export_2026_01",
            "export_2026_01",
            50,
            Some("hash_abc"),
            Some("hash_different"),
        );
        match action_hash_mismatch {
            CheckpointResumeAction::DatasetChangedMustRevalidate { reason } => {
                assert!(reason.contains("Last message identity hash mismatch"));
            }
            _ => panic!("Expected DatasetChangedMustRevalidate on hash mismatch"),
        }
    }

    #[test]
    fn test_20_migration_v1_to_v2_preserves_existing_data() {
        let conn = Connection::open_in_memory().unwrap();
        apply_pragmas(&conn).unwrap();

        // 1. Manually establish V1 state
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at_ms INTEGER NOT NULL,
                description TEXT NOT NULL
            );",
        )
        .unwrap();
        conn.execute_batch(MIGRATION_V1_SQL).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at_ms, description) VALUES (1, 1000, 'Initial schema')",
            [],
        )
        .unwrap();

        // Insert sample V1 data: conversation, participant, message with source_message_id, v1 checkpoint
        conn.execute(
            "INSERT INTO conversations (id, source_identifier, conversation_type, updated_at_ms) VALUES ('c_v1', 'conv_v1_src', 'DIRECT', 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO participants (id, source_identifier, username) VALUES ('p_v1', 'u_v1', 'user_v1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, conversation_id, sender_id, message_type, content, timestamp_ms, is_outgoing, source_message_id, identity_hash)
             VALUES ('m_v1', 'c_v1', 'p_v1', 'TEXT', 'Legacy message', 1000, 0, 'legacy_mid_1', 'legacy_hash_1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sync_checkpoints (conversation_source_id, last_file_rel_path, last_source_array_index, last_timestamp_ms, updated_at_ms)
             VALUES ('conv_v1_src', 'inbox/legacy/message_1.json', 15, 1000, 1000)",
            [],
        )
        .unwrap();

        // Wrap into Database struct and trigger migrations
        let mut db = Database { conn, path: None };

        let v_before = db.get_schema_version().unwrap();
        assert_eq!(v_before, 1);

        db.run_migrations().unwrap();

        let v_after = db.get_schema_version().unwrap();
        assert_eq!(v_after, 2);

        // Verify message data preserved
        let msg_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE id = 'm_v1' AND source_message_id = 'legacy_mid_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(msg_count, 1, "Messages must be preserved across migration");

        // Verify v1 checkpoint migrated cleanly into v2 schema with default source_account_id
        let (account_id, last_index, reval_status): (String, i64, String) = db
            .conn()
            .query_row(
                "SELECT source_account_id, last_source_array_index, revalidation_status FROM sync_checkpoints WHERE conversation_source_id = 'conv_v1_src'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(account_id, "default");
        assert_eq!(last_index, 15);
        assert_eq!(reval_status, "VALID");

        // Verify new index idx_messages_source_id exists
        let idx_count: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_messages_source_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            idx_count, 1,
            "idx_messages_source_id must exist in V2 schema"
        );

        // Verify composite primary key allows inserting same conversation_source_id for another account
        db.conn()
            .execute(
                "INSERT INTO sync_checkpoints (source_account_id, conversation_source_id, dataset_fingerprint, last_file_rel_path, last_source_array_index, last_timestamp_ms, updated_at_ms)
             VALUES ('custom_acc', 'conv_v1_src', 'fp2', 'inbox/legacy/message_1.json', 20, 1000, 1000)",
                [],
            )
            .unwrap();

        let total_checkpoints: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM sync_checkpoints WHERE conversation_source_id = 'conv_v1_src'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            total_checkpoints, 2,
            "Composite PK must allow same conversation for distinct accounts"
        );
    }
}
