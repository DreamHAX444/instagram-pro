# Technical Requirements Document (TRD)
## Instagram Chat History Viewer (Windows Desktop)

**Document Version:** 1.1.0  
**Status:** Approved Technical Architecture (Post-Audit Revision)  
**Target Release:** MVP (Phase 1)  
**Author:** Software Architect & Engineering Planning Agent  
**Aligned With:** `PRD.md` (Version 1.1.0)  

---

## 1. System Architecture

### 1.1 Architectural Evaluation & Recommended Stack

> [!NOTE]
> **Proposed Architecture Evaluation (Ponytail Minimalism & Windows Efficiency)**  
> In accordance with the Ponytail engineering principles (selecting the simplest, lightest, and most native solution that reliably solves the problem), we evaluated four runtime architectures for a lightweight Windows desktop application:
> 
> 1. **Electron (Node.js + Chromium):** Familiar ecosystem, but excessive resource bloat (200–350 MB idle RAM, 120+ MB installer, separate bundled browser engine). Rejected for violating the "lightweight" constraint.
> 2. **Python + PySide6/Qt:** Strong data parsing, but distribution via PyInstaller creates fragile bundles (100+ MB), and styling dynamic Instagram chat bubbles, reactions, and audio waveforms in QML/QtWidgets requires substantial custom rendering boilerplate.
> 3. **.NET 8 + WinUI 3 (C#):** Excellent native Windows performance, but cross-platform expandability is limited, and building rich media chat feeds with complex inline audio scrubbers is significantly slower to develop than modern componentized layouts.
> 4. **Tauri v2 (Rust Core + Windows Native WebView2 + TypeScript/Tailwind UI) — [PROPOSED & RECOMMENDED]:**
>    - **Lightweight Footprint:** Utilizes the Windows Evergreen WebView2 runtime already pre-installed on Windows 10/11. Standalone binary size target is ~15 MB, with working set memory targeted `[Target - Unverified Benchmark]` below 45 MB idle / 150 MB active.
>    - **Safe High-Performance Core:** Rust manages native Windows APIs (Registry startup, session-local named Mutex single-instance locking), direct file I/O, SHA-256 media hashing, streaming HTTP/custom asset protocol handlers, and embedded SQLite transactions.
>    - **Productive UI Layer:** Standard Web technologies (TypeScript, HTML5, CSS/Tailwind) enable rapid, pixel-perfect replication of Instagram's modern direct messaging interface, seamless virtualized lists, and native HTML5/WebAudio playback.

```
+---------------------------------------------------------------------------------------+
|                                    WINDOWS 10 / 11                                    |
|                                                                                       |
|   +-------------------------------------------------------------------------------+   |
|   |                        UI LAYER (WebView2 Frontend)                           |   |
|   |  - Conversation Sidebar (Search, Avatars, Unread Badges)                      |   |
|   |  - Virtualized Message Viewport (Dynamic heights, target 60fps scrolling)     |   |
|   |  - Message Bubble Components (Text, Reply Quote, Reactions, Read Receipts)    |   |
|   |  - Inline Voice Player (Web Audio API, Scrubber, Speed 1.0x/1.5x/2.0x)        |   |
|   |  - Media Lightbox (Image zoom/pan, Video inline player)                       |   |
|   |  - Call Event Cards (Voice / Video, Duration, Derived End Time)               |   |
|   +-------------------------------------------------------------------------------+   |
|                                          | Inter-Process IPC (Tauri Commands)         |
|   +--------------------------------------v----------------------------------------+   |
|   |                         CORE BACKEND ENGINE (Rust)                            |   |
|   |  +---------------------+ +----------------------+ +-------------------------+  |   |
|   |  | Import & Sync       | | Media & Storage      | | Windows OS Integration  |  |   |
|   |  | Coordinator         | | Manager              | | - Registry Startup      |  |   |
|   |  | - Checkpoint Engine | | - File Streaming     | | - Session Mutex Single- |  |   |
|   |  | - Batch Ingestion   | | - SHA-256 Validator  | |   Instance Lock         |  |   |
|   |  | - Deduplication     | | - Missing File Cache | | - Native File Dialogs   |  |   |
|   |  +---------------------+ +----------------------+ +-------------------------+  |   |
|   |  +-------------------------------------------------------------------------+  |   |
|   |  | Data-Source Adapters                                                    |  |   |
|   |  | - Meta JSON Archive Adapter (Auto-detects paths, splits, encoding fix)  |  |   |
|   |  | - File-Diff Sync Adapter (Incremental updates for personal accounts)   |  |   |
|   |  +-------------------------------------------------------------------------+  |   |
|   |  +-------------------------------------------------------------------------+  |   |
|   |  | Local Database Engine (SQLite 3.45+ with WAL mode)                      |  |   |
|   |  | - Schema Migrations | Relational Entities | Compound Indexes            |  |   |
|   |  +-------------------------------------------------------------------------+  |   |
|   +-------------------------------------------------------------------------------+   |
|           |                                                      |                    |
|   +-------v-------------------------+            +---------------v----------------+   |
|   | Local SQLite File               |            | Local Media Storage            |   |
|   | %LOCALAPPDATA%/.../chat.db      |            | %LOCALAPPDATA%/.../media/      |   |
|   +---------------------------------+            +--------------------------------+   |
+---------------------------------------------------------------------------------------+
```

### 1.2 Component Responsibilities & System Boundaries

1. **Desktop Shell & OS Integration:**
   - Enforces single-instance execution using `CreateMutexW` with a session-local namespace (`Local\InstagramChatHistoryViewer_InstanceMutex`) runnable without administrator elevation.
   - Manages Windows startup auto-launch via `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` resolving dynamically to `std::env::current_exe()`.
   - Handles graceful Windows shutdown messages (`WM_QUERYENDSESSION`, `WM_ENDSESSION`), ensuring pending SQLite transactions commit cleanly.
2. **UI Presentation Layer:**
   - Renders conversations and chat threads using window virtualization (`@tanstack/react-virtual` or equivalent lightweight windowing algorithm).
   - Manages local UI states: active conversation, lightbox open/close, audio playback state, search query filters.
   - Enforces accessible keyboard navigation, ARIA semantics, and responsive layout.
3. **Data-Source Adapters:**
   - Abstract the origin of raw data via a common `DataSourceAdapter` trait.
   - **MetaArchiveAdapter:** 
     - Recursively auto-detects conversation folders under both legacy paths (`messages/inbox/*`) and Accounts Center paths (`your_instagram_activity/messages/inbox/*`).
     - Detects and parses split multi-volume archives (`part_1.zip`, `part_2.zip`).
     - Repairs Latin-1 UTF-8 encoding flaws and normalizes file paths.
     - Maps raw array indices to chronological streaming.
   - **FileDiffSyncAdapter:** For personal accounts, compares newer DYI archive folders against existing database checkpoints to ingest only newly added messages.
4. **Import & Sync Coordinator:**
   - Manages the execution pipeline: batching records (500–1000 items), computing deterministic identity hashes, verifying file references, executing atomic database transactions, and persisting stable conversation checkpoints.
   - Dispatches progress events (e.g., `import://progress`, `import://complete`, `import://error`) to the frontend.
5. **Local Database Layer:**
   - Manages connection pooling, WAL configuration, schema creation, versioned migrations, and typed queries.
   - Strictly enforces transactional integrity and foreign keys.
6. **Media Storage Manager:**
   - Isolates imported media files in `%LOCALAPPDATA%\InstagramChatHistoryViewer\media\{conversation_id}\`.
   - Serves local media files to the WebView2 frontend via a custom protocol handler (`asset://media/...`), utilizing HTTP `206 Partial Content` (Range requests) for zero-memory audio/video streaming.
   - Computes SHA-256 hashes to prevent duplicate file storage and detect corruption.
7. **Diagnostics & Logging:**
   - Writes rolling diagnostic logs to `%LOCALAPPDATA%\InstagramChatHistoryViewer\logs\`.
   - Automatically sanitizes private message text, participant identifiers, and authorization credentials before logging.

---

## 2. Data-Source Strategy and Feasibility

### 2.1 Separation of Historical Import vs. Live Synchronization

| Capability | Historical Archive Import (Primary MVP Source) | Incremental Archive Diffing (Personal Sync) | Official Meta Graph API (Professional Only) |
|---|---|---|---|
| **Account Eligibility** | All Instagram accounts (Personal, Creator, Business) | All Instagram accounts (Personal, Creator, Business) | Professional / Creator accounts only |
| **Data Scope** | Complete export package requested by user | Newly exported messages and media | Messages created after app integration; restricted history |
| **Transport** | Offline `.zip` or uncompressed directory | Offline `.zip` or uncompressed directory | HTTPS REST endpoints + Webhooks |
| **Authentication** | None required (user-provided export) | None required (user-provided export) | OAuth 2.0 User Token (`instagram_manage_messages`) |
| **Infrastructure Req.** | Strictly local-first (no network required) | Strictly local-first (no network required) | Public HTTPS webhook callback server (out of scope for desktop MVP) |
| **Risk Profile** | Zero risk of account ban or credential theft | Zero risk of account ban or credential theft | Compliant with Developer Policies, but requires App Review |
| **Read Receipts** | **UNAVAILABLE** (omitted by Meta export) | **UNAVAILABLE** (omitted by Meta export) | Available via live messaging webhooks |
| **Parent Reply IDs** | **UNAVAILABLE** (omitted by Meta export) | **UNAVAILABLE** (omitted by Meta export) | Available on incoming webhook payload |
| **Call Records** | Available (`timestamp_ms` + `call_duration`) | Available (`timestamp_ms` + `call_duration`) | **UNAVAILABLE** (omitted from Graph API) |

> [!IMPORTANT]
> **Personal Account Live Synchronization Reality:**  
> Meta provides **zero official APIs for personal Instagram accounts**. Any third-party tool claiming "live real-time synchronization" for personal accounts relies on stolen browser cookies, web scraping, or reverse-engineered mobile endpoints—all of which violate Instagram's Terms of Service, pose credential theft hazards, and risk permanent account bans. For personal accounts, synchronization in this project is strictly defined as **offline incremental archive diffing**.

### 2.2 Data Source 1: User-Provided Instagram Data Export (Baseline Source)

#### Resilient Directory Discovery
The adapter MUST NOT assume a single rigid folder structure. It recursively inspects the import directory or ZIP archive to detect the inbox location:
```
Supported Export Structures:
1. Legacy Structure:
   <root>/messages/inbox/<conversation_name>/message_1.json
2. Accounts Center Structure (Modern):
   <root>/your_instagram_activity/messages/inbox/<conversation_name>/message_1.json
3. Split Zip Volumes:
   instagram-username-2026-10-15-part_1.zip -> inbox/conv_a/message_1.json
   instagram-username-2026-10-15-part_2.zip -> inbox/conv_a/message_2.json
```
The adapter enumerates all `message_*.json` files per conversation, sorts them by partition number, and iterates through each partition.

#### Known Quirks & Adapter Normalization Strategy
1. **The Latin-1 UTF-8 Encoding Bug:**  
   Meta encodes Unicode characters as raw Latin-1 strings. The adapter applies a mandatory string normalization routine to all parsed text fields:
   ```rust
   pub fn fix_meta_mojibake(input: &str) -> String {
       let bytes: Vec<u8> = input.chars().map(|c| c as u8).collect();
       String::from_utf8(bytes).unwrap_or_else(|_| input.to_string())
   }
   ```
2. **Audio File Formats:**  
   Voice messages are packaged as `.mp4` containers holding AAC audio, `.m4a` files, or `.aac`. The adapter normalizes MIME types and verifies existence on disk.
3. **Partitioned JSON Files & Sorting:**  
   In Meta export files, messages within each `message_N.json` are stored in reverse chronological order (newest to oldest). To achieve strict chronological ingestion, the adapter processes partitions in descending order (`message_N.json` down to `message_1.json`) and reads each array from back to front (`index = array.len() - 1` down to `0`).
4. **Call Records in JSON:**  
   Call events are represented in `message_*.json` containing `"call_duration"` (in seconds) or content descriptors ("You called...", "Video chat ended"). The start time is `timestamp_ms`. End time is a **derived value** calculated as `timestamp_ms + (call_duration * 1000)`.

---

## 3. Database Design

The local database is powered by **SQLite 3.45+**.

### 3.1 SQLite Initialization Pragma Settings
Upon opening every database connection, the application MUST execute:
```sql
PRAGMA journal_mode = WAL;          -- Write-Ahead Logging for high concurrency
PRAGMA synchronous = NORMAL;        -- Durable, prevents corruption on crash
PRAGMA foreign_keys = ON;           -- Enforce relational integrity
PRAGMA busy_timeout = 5000;         -- 5-second lock retry before returning SQLITE_BUSY
PRAGMA temp_store = MEMORY;         -- In-memory temporary tables and indexes
PRAGMA cache_size = -64000;         -- 64 MB page cache
```

### 3.2 Relational Schema (DDL)

```sql
-- Schema Migration Version Tracker
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at_ms INTEGER NOT NULL,
    description TEXT NOT NULL
);

-- Conversations (Direct Messages & Group Chats)
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,                       -- Internal UUID
    source_identifier TEXT UNIQUE NOT NULL,    -- Directory name e.g. "john_doe_12345678" or Meta thread ID
    conversation_type TEXT NOT NULL,           -- 'DIRECT', 'GROUP'
    title TEXT,                                -- Group title or participant name
    created_at_ms INTEGER,                     -- Origin creation timestamp if known
    updated_at_ms INTEGER NOT NULL,            -- Last message timestamp (for sidebar sorting)
    last_message_preview TEXT,                 -- Snippet of latest message
    is_archived INTEGER NOT NULL DEFAULT 0     -- 0 = Active, 1 = Archived
);
CREATE INDEX IF NOT EXISTS idx_conversations_updated ON conversations(updated_at_ms DESC);

-- Participants
CREATE TABLE IF NOT EXISTS participants (
    id TEXT PRIMARY KEY,                       -- Internal UUID
    source_identifier TEXT UNIQUE NOT NULL,    -- Normalized username or source ID
    username TEXT NOT NULL,
    full_name TEXT,
    avatar_storage_path TEXT,                  -- Relative path to cached avatar image
    is_self INTEGER NOT NULL DEFAULT 0         -- 1 if account owner
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
    id TEXT PRIMARY KEY,                       -- Internal UUID
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    sender_id TEXT NOT NULL REFERENCES participants(id) ON DELETE RESTRICT,
    source_message_id TEXT,                    -- Source-provided ID if available
    identity_hash TEXT UNIQUE NOT NULL,        -- Deterministic SHA-256 for deduplication
    source_file_path TEXT,                     -- Relative path of source file in archive
    source_array_index INTEGER,                -- 0-based array index within source file
    message_type TEXT NOT NULL,                -- 'TEXT', 'VOICE', 'IMAGE', 'VIDEO', 'CALL', 'SHARE', 'ACTION', 'UNKNOWN'
    content TEXT,                              -- Message text content (null for pure media)
    timestamp_ms INTEGER NOT NULL,             -- UTC epoch milliseconds
    timezone_offset_minutes INTEGER,           -- Original timezone offset if provided
    is_outgoing INTEGER NOT NULL,              -- 1 = Sent by self, 0 = Received
    is_edited INTEGER NOT NULL DEFAULT 0,      -- 1 = Message was edited
    reply_to_message_id TEXT REFERENCES messages(id) ON DELETE SET NULL, -- Parent message ID (null if unlinked)
    raw_payload_json TEXT                      -- Complete raw source payload for safe archival
);
CREATE INDEX IF NOT EXISTS idx_messages_conv_time ON messages(conversation_id, timestamp_ms ASC);
CREATE INDEX IF NOT EXISTS idx_messages_type ON messages(message_type);
CREATE INDEX IF NOT EXISTS idx_messages_reply ON messages(reply_to_message_id);

-- Attachments (Voice, Image, Video, File)
CREATE TABLE IF NOT EXISTS attachments (
    id TEXT PRIMARY KEY,                       -- Internal UUID
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    attachment_type TEXT NOT NULL,             -- 'AUDIO', 'IMAGE', 'VIDEO', 'STICKER'
    original_uri TEXT,                         -- Path as referenced in export
    storage_path TEXT,                         -- Local path relative to application media root
    mime_type TEXT NOT NULL,                   -- e.g. 'audio/mp4', 'image/jpeg', 'video/mp4'
    file_size_bytes INTEGER,                   -- Verified file size
    sha256_checksum TEXT,                      -- SHA-256 for deduplication and corruption detection
    duration_ms INTEGER,                       -- Audio/video duration in milliseconds
    width INTEGER,                             -- Visual width
    height INTEGER,                            -- Visual height
    status TEXT NOT NULL                       -- 'PENDING', 'LOCAL_COPIED', 'MISSING', 'CORRUPT'
);
CREATE INDEX IF NOT EXISTS idx_attachments_msg ON attachments(message_id);
CREATE INDEX IF NOT EXISTS idx_attachments_sha256 ON attachments(sha256_checksum);

-- Message Reactions
CREATE TABLE IF NOT EXISTS reactions (
    id TEXT PRIMARY KEY,                       -- Internal UUID
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    participant_id TEXT NOT NULL REFERENCES participants(id) ON DELETE CASCADE,
    reaction_value TEXT NOT NULL,              -- Unicode emoji string
    created_at_ms INTEGER,
    UNIQUE (message_id, participant_id, reaction_value)
);
CREATE INDEX IF NOT EXISTS idx_reactions_msg ON reactions(message_id);

-- Read Receipts (Explicit Modeling of Available vs Unavailable)
CREATE TABLE IF NOT EXISTS read_receipts (
    id TEXT PRIMARY KEY,                       -- Internal UUID
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    participant_id TEXT NOT NULL REFERENCES participants(id) ON DELETE CASCADE,
    read_status TEXT NOT NULL,                 -- 'AVAILABLE', 'UNAVAILABLE_FROM_SOURCE', 'UNREAD'
    read_at_ms INTEGER,                        -- Exact viewed timestamp (null if unavailable)
    UNIQUE (message_id, participant_id)
);
CREATE INDEX IF NOT EXISTS idx_read_receipts_msg ON read_receipts(message_id);

-- Voice and Video Call Records (Mandatory)
CREATE TABLE IF NOT EXISTS call_events (
    id TEXT PRIMARY KEY,                       -- Internal UUID
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    message_id TEXT REFERENCES messages(id) ON DELETE CASCADE,
    caller_id TEXT NOT NULL REFERENCES participants(id) ON DELETE RESTRICT,
    call_type TEXT NOT NULL,                   -- 'VOICE', 'VIDEO'
    direction TEXT NOT NULL,                   -- 'INCOMING', 'OUTGOING'
    status TEXT NOT NULL,                      -- 'ANSWERED', 'MISSED', 'DECLINED', 'FAILED', 'UNKNOWN'
    start_timestamp_ms INTEGER NOT NULL,       -- Epoch milliseconds (source-provided)
    duration_seconds INTEGER,                  -- Duration in seconds (source-provided, nullable)
    derived_end_timestamp_ms INTEGER           -- Computed: start_timestamp_ms + (duration_seconds * 1000)
);
CREATE INDEX IF NOT EXISTS idx_call_events_conv ON call_events(conversation_id, start_timestamp_ms ASC);

-- Import Jobs (Informational execution history)
CREATE TABLE IF NOT EXISTS import_jobs (
    id TEXT PRIMARY KEY,                       -- Execution UUID
    source_type TEXT NOT NULL,                 -- 'EXPORT_ZIP', 'EXPORT_DIR', 'DIFF_UPDATE'
    source_path TEXT,
    started_at_ms INTEGER NOT NULL,
    completed_at_ms INTEGER,
    status TEXT NOT NULL,                      -- 'RUNNING', 'COMPLETED', 'FAILED', 'CANCELLED'
    total_messages_imported INTEGER NOT NULL DEFAULT 0,
    total_media_files_imported INTEGER NOT NULL DEFAULT 0,
    error_summary TEXT
);

-- Stable Sync Checkpoints (Decoupled from transient job_id)
CREATE TABLE IF NOT EXISTS sync_checkpoints (
    conversation_source_id TEXT PRIMARY KEY,   -- Stable identifier e.g. "john_doe_12345678"
    last_job_id TEXT REFERENCES import_jobs(id) ON DELETE SET NULL,
    last_file_rel_path TEXT NOT NULL,          -- e.g. "messages/inbox/conv_a/message_2.json"
    last_source_array_index INTEGER NOT NULL,  -- Physical 0-based offset in source file array
    last_timestamp_ms INTEGER NOT NULL,        -- Highest timestamp durably saved
    updated_at_ms INTEGER NOT NULL
);

-- Failed Items Log
CREATE TABLE IF NOT EXISTS failed_items (
    id TEXT PRIMARY KEY,
    job_id TEXT REFERENCES import_jobs(id) ON DELETE CASCADE,
    item_type TEXT NOT NULL,                   -- 'JSON_FILE', 'MESSAGE', 'MEDIA_FILE'
    source_reference TEXT NOT NULL,            -- File path or record ID
    error_reason TEXT NOT NULL,
    stack_trace TEXT,
    occurred_at_ms INTEGER NOT NULL
);
```

### 3.3 Deduplication Strategy & Legitimate Identical Message Preservation

> [!IMPORTANT]
> **Deterministic Identity Hash Architecture:**  
> A timestamp alone is **NEVER** used as a unique identifier. Furthermore, hashing only `(conversation, sender, timestamp, content)` can falsely discard legitimate identical messages (e.g. repeated texts sent in rapid succession or archives with truncated second-level timestamps).

The system enforces deduplication through a two-tiered identity strategy:

1. **Tier 1 (Source-Provided Unique ID):**  
   If the source provides a unique message identifier (`source_message_id`), it is used directly:
   $$\text{identity\_hash} = \text{SHA256}(\text{"MID:"} \parallel \text{source\_message\_id})$$

2. **Tier 2 (Archive Fallback with Positional Scoping):**  
   In official Meta DYI JSON archives where individual message IDs are absent, the system scopes the identity hash to the source file and array index:
   $$\text{identity\_hash} = \text{SHA256}(\text{conversation\_source\_id} \parallel \text{sender\_handle} \parallel \text{timestamp\_ms} \parallel \text{source\_file\_path} \parallel \text{source\_array\_index} \parallel \text{content\_digest})$$
   Where `content_digest` is:
   - For text: `SHA256(normalized_utf8_text)`
   - For voice/media: `SHA256(original_uri)`
   - For calls: `SHA256(call_type + direction + duration_seconds)`

**Why this guarantees data safety and idempotence:**
- **Zero Loss of Legitimate Identical Messages:** If a user sends "ok" twice in the same millisecond or second, the two messages occupy distinct array indices (`source_array_index = 4` and `source_array_index = 5`) within the source file, producing distinct identity hashes. Neither is discarded.
- **Strict Idempotency Across Re-Imports:** If the user imports the exact same archive folder twice, every message in the archive generates the exact same hash as the first run, allowing SQLite to skip or safely upsert existing rows without creating duplicates.

---

## 4. Media Storage and Playback Architecture

### 4.1 Storage Layout
All persistent media files are copied into a sandboxed application data directory:
```
%LOCALAPPDATA%\InstagramChatHistoryViewer\
├── database\
│   ├── chat.db
│   ├── chat.db-wal
│   └── chat.db-shm
├── media\
│   ├── {conversation_uuid}\
│   │   ├── audio\
│   │   │   └── {sha256_prefix}_{filename}.mp4
│   │   ├── images\
│   │   │   └── {sha256_prefix}_{filename}.jpg
│   │   └── videos\
│   │       └── {sha256_prefix}_{filename}.mp4
└── logs\
    └── app.log
```

### 4.2 Zero-Memory Streaming via Custom Protocol
The WebView2 frontend does not load raw file buffers into JavaScript memory. Instead:
1. The backend registers a secure custom protocol handler: `asset://media/{relative_path}`.
2. When the `<audio>` or `<video>` player in WebView2 requests a media URI, the Rust backend handles the request.
3. The backend opens the physical file on disk using asynchronous streaming (`tokio::fs::File`).
4. It supports standard HTTP range headers:
   - `Accept-Ranges: bytes`
   - `Range: bytes=start-end`
   - `Content-Range: bytes start-end/total`
   - Status `206 Partial Content`
5. Media bytes are streamed directly into the browser's native media pipeline, keeping the application's RAM usage bounded below 150 MB regardless of file size.

### 4.3 Missing File Detection and Retry Flow
1. If a media file referenced in the export is missing from disk, `attachments.status` is marked as `'MISSING'`.
2. The UI renders the voice or image component with a warning indicator: "Media file unavailable".
3. An action button "Locate File..." opens a native Windows File Open dialog (`rfd` crate in Rust).
4. The user selects the file; the backend verifies the file size, computes its SHA-256 hash, copies it into the proper `media/` subfolder, updates the database record to `'LOCAL_COPIED'`, and notifies the UI via an event (`media://updated`).

---

## 5. Timestamp and Event Modeling

### 5.1 Timestamp Preservation and Display Pipeline
1. **Source Storage:** Timestamps are stored as raw 64-bit integer epoch milliseconds (`timestamp_ms`). No time zone offsets are applied during storage.
2. **Display Conversion:** At render time, the client converts `timestamp_ms` into a localized date string using the standard Windows JavaScript `Intl.DateTimeFormat`.
3. **Equal Timestamps Sorting:** In-database queries strictly enforce:
   ```sql
   SELECT * FROM messages 
   WHERE conversation_id = ? 
   ORDER BY timestamp_ms ASC, source_array_index ASC, rowid ASC;
   ```
   This guarantees that messages sharing the exact same millisecond maintain an unshakeable, deterministic visual order.

### 5.2 Independent Event Models
- **Call Events:** Store source start time and duration. `derived_end_timestamp_ms` is computed and explicitly designated as derived.
- **Read Receipts:** Explicitly represent `UNAVAILABLE_FROM_SOURCE` for all archive-derived messages.
- **Replies:** Stored with `reply_to_message_id = NULL` when parent IDs are omitted by the export. Quoted preview snippets are parsed and displayed as static context without claiming deterministic jump navigation.

---

## 6. Checkpoint-Based Incremental Synchronization

### 6.1 The Recoverable Ingestion Algorithm

```
  [Start Import Session]
         │
         ▼
  1. Initialize Job Record (import_jobs with status = 'RUNNING')
         │
         ▼
  2. Inspect Stable Checkpoints (sync_checkpoints WHERE conversation_source_id = ?)
         │
         ▼
  3. Locate Resumption Offset (Seek to last_file_rel_path, skip to last_source_array_index)
         │
         ▼
  4. Batch Enumeration (Read batch of 500 records from JSON stream)
         │
         ▼
  5. Compute Identity Hashes (Tier 1 MID or Tier 2 Scoped Hash)
         │
         ▼
  6. Copy Media Attachments (Verify SHA-256, write to AppData media folder)
         │
         ▼
  7. Open SQLite Transaction (BEGIN IMMEDIATE)
         │
         ▼
  8. Insert Entities (conversations, participants, messages, attachments, calls)
         │
         ▼
  9. Update Sync Checkpoint (Atomically advance file path, array index, timestamp)
         │
         ▼
  10. Commit Transaction (COMMIT)
         │
         ▼
  11. Emit Progress Event to UI (IPC event: import://progress)
         │
         ├── More records? ──► [Loop back to Step 4]
         │
         ▼
  12. Finalize Session (Mark import_jobs status = 'COMPLETED')
```

1. **Job Record Creation:** Create a new tracking row in `import_jobs` with a new UUID.
2. **Stable Checkpoint Query:** Query `sync_checkpoints` using `conversation_source_id`. If a checkpoint exists, load `last_file_rel_path` and `last_source_array_index`. Notice that this query does **NOT** filter by `job_id`, ensuring full recovery after a previous crash.
3. **Stream Positioning:** Open the JSON files. Skip any partition files already fully processed. For the partially processed file, seek directly past `last_source_array_index`.
4. **Batch Processing:** Read a batch of 500 items.
5. **Hash & Media Ingestion:** Compute scoped `identity_hash`. Stream media files to `%LOCALAPPDATA%\...\media\`.
6. **Atomic Transaction:** Execute `BEGIN IMMEDIATE TRANSACTION;`.
7. **Entity Writes:** Insert/update `conversations`, `participants`, `conversation_participants`, `messages`, `attachments`, `reactions`, and `call_events`.
8. **Atomic Checkpoint Advancement:** Update or insert into `sync_checkpoints` setting `last_file_rel_path`, `last_source_array_index`, and `last_timestamp_ms` to the latest item in this batch, along with `last_job_id = current_job_id`.
9. **Commit:** Execute `COMMIT;`. Invariant: Checkpoint advancement and message writes are committed in the exact same SQLite transaction.
10. **Progress Event:** Emit `import://progress` to UI.

### 6.2 Crash Recovery Invariant
If a crash occurs during file copy or transaction insertion, SQLite automatically rolls back any uncommitted writes upon next startup. The checkpoint table retains the last durably committed batch. The subsequent import run reads the checkpoint and resumes without skipping a single record and without creating duplicates.

---

## 7. Windows Startup and Lifecycle

### 7.1 Startup Registration via Registry
- When enabled by user, the application writes:
  - **Key:** `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`
  - **Name:** `InstagramChatHistoryViewer`
  - **Type:** `REG_SZ`
  - **Value:** `"{current_exe_path}" --minimized` (resolved dynamically via `std::env::current_exe()`).
- When disabled, the key is removed using `RegDeleteValueW`.
- Operates entirely within `HKCU` without requiring administrative rights.

### 7.2 Single-Instance Enforcement for Standard Users
To prevent elevation issues on standard Windows user accounts, the application uses a **session-local** named mutex:
```rust
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Threading::*;

// Use Local\ namespace to run without SeCreateGlobalPrivilege / admin rights
let mutex_name = b"Local\\InstagramChatHistoryViewer_InstanceMutex\0";
unsafe {
    let handle = CreateMutexA(std::ptr::null(), TRUE, mutex_name.as_ptr());
    if GetLastError() == ERROR_ALREADY_EXISTS {
        eprintln!("Another instance is already running in this user session.");
        std::process::exit(0);
    }
}
```

### 7.3 Graceful Shutdown Handling
Captures `WM_QUERYENDSESSION` and `WM_ENDSESSION` to flush SQLite WAL buffers (`PRAGMA wal_checkpoint(TRUNCATE)`) and release mutexes before process exit.

---

## 8. Security and Privacy Architecture

1. **Strictly Local-First Operation:** Zero outgoing network requests for conversation data.
2. **Credential Management (DPAPI):** If optional Meta Graph API credentials are used, tokens are encrypted with Windows DPAPI (`CryptProtectData`) before storage in Windows Credential Manager.
3. **PII Redaction in Diagnostics:**
   ```rust
   fn sanitize_log_message(msg: &str) -> String {
       let re_token = regex::Regex::new(r"EAAB[a-zA-Z0-9]+").unwrap();
       re_token.replace_all(msg, "[REDACTED_ACCESS_TOKEN]").to_string()
   }
   ```
4. **Data Deletion:** "Delete All Local Data" button removes `%LOCALAPPDATA%\InstagramChatHistoryViewer\`, leaving zero residual data.

---

## 9. Testing Strategy

### 9.1 Test Suites

| Suite ID | Focus | Tools / Frameworks |
|---|---|---|
| **TS-UNIT** | Identity hashing, encoding fixes, timestamp formatting, DDL schemas | Rust `cargo test`, Vitest (Frontend) |
| **TS-DB** | Relational constraints, WAL mode, deduplication idempotency | SQLite in-memory and temporary file tests |
| **TS-IMPORT** | Archive folder auto-discovery, split archives, checkpoint recovery | Test fixtures with synthetic Meta JSON exports |
| **TS-MEDIA** | Audio streaming, Range requests, SHA-256 verification, missing media | Mock file server & audio file fixtures |
| **TS-UI** | Virtualized list rendering, conversation search, lightbox, audio player | Playwright / Vitest Component Testing |
| **TS-CHAOS** | Process termination during import, crash recovery | Scripted process kill and database validation |
| **TS-BENCH** | Performance benchmarks (startup, scrolling FPS, search latency, memory) | Windows Performance Toolkit, Criterion.rs |

### 9.2 Critical Test Cases

1. **`TC-VOICE-01` (Voice Playback & Scrubbing):** Load AAC `.mp4` audio note. Verify play, pause, time updates, scrubber repositioning, and duration accuracy.
2. **`TC-VOICE-02` (Missing Audio Handling):** Reference a non-existent audio file. Verify attachment marked `'MISSING'`, warning badge rendered, no crash.
3. **`TC-TIME-01` (Timestamp Conversion):** Input epoch milliseconds. Verify rendered text matches local system clock representation.
4. **`TC-TIME-02` (Identical Timestamp Stability):** Insert 5 messages with identical timestamps. Verify consistent query order across 100 queries using array index secondary sort.
5. **`TC-READ-01` (Read Receipts Fallback):** For DYI archive imports, verify read status is `UNAVAILABLE_FROM_SOURCE` and no synthetic "Seen" label appears.
6. **`TC-REACT-01` (Reactions Dedup):** Ingest a reaction `❤️` from participant X twice. Verify reaction count remains 1.
7. **`TC-REPLY-01` (Quoted Fallback):** Verify reply renders quote text; click-to-scroll is disabled when parent ID is absent.
8. **`TC-CALL-01` (Voice Call Duration & Derived End):** Verify call event parses duration, computes derived end time, and renders badge.
9. **`TC-CALL-02` (Missed Video Call):** Parse missed video call event; verify status `'MISSED'`.
10. **`TC-SYNC-01` (Deduplication Safety):** Ingest archive with identical consecutive short messages. Verify all distinct messages are preserved. Re-importing same archive must result in zero duplicate additions.
11. **`TC-SYNC-02` (Crash Recovery Resumption):** Kill process at batch 3 of 10. Re-open and verify import resumes from checkpoint by `conversation_source_id` without data loss.
12. **`TC-WIN-01` (Non-Elevated Mutex):** Verify `Local\` mutex succeeds on a standard unprivileged Windows account.
13. **`TC-BENCH-01` (Resource Measurements):** Measure cold start, virtualized list scroll FPS, search latency, and memory working set on physical hardware.

---

## 10. Comprehensive Requirements Traceability Matrix

| PRD Requirement ID | Category | Technical Component | Data Model Entity | Test Case ID | Feasibility & Dependencies |
|---|---|---|---|---|---|
| `FR-CONV-001` | Conversations | Sidebar Viewport | `conversations` | `TS-UI-01` | Fully feasible from local SQLite |
| `FR-CONV-002` | Conversations | Conversation Item | `conversations`, `participants` | `TS-UI-02` | Available in export JSON |
| `FR-CONV-003` | Conversations | Participant Service | `participants` | `TS-DB-01` | Available in export JSON |
| `FR-CONV-004` | Conversations | Sidebar Search Bar | `conversations` | `TS-UI-01` | Client-side reactive filter |
| `FR-CONV-005` | Conversations | Message Timeline | `messages` compound index | `TS-DB-02` | `ORDER BY timestamp_ms ASC` |
| `FR-TXT-001` | Text Messages | Message Bubble | `messages.content` | `TS-UNIT-01` | Handled with UTF-8 encoding fix |
| `FR-TXT-002` | Text Messages | Bubble Alignment | `messages.is_outgoing` | `TS-UI-03` | Evaluated against self participant |
| `FR-TXT-003` | Text Messages | Source ID Tracker | `messages.source_message_id` | `TS-DB-02` | Preserved when provided by source |
| `FR-TXT-004` | Text Messages | Localized Time Hover | `messages.timestamp_ms` | `TC-TIME-01` | Converted via JavaScript Intl |
| `FR-TXT-005` | Text Messages | Edited Badge | `messages.is_edited` | `TS-UI-03` | Handled when flag exists in source |
| `FR-TXT-006` | Text Messages | Clipboard Handler | Windows Clipboard API | `TS-UI-03` | Standard browser/OS clipboard |
| `FR-TXT-007` | Text Messages | Message Classifier | `messages.message_type` | `TS-UNIT-01` | Ingestion type normalizer |
| `FR-VOICE-001` | Voice Messages | Media Storage Mgr | `attachments` | `TS-MEDIA-01` | Local bitstream file copy |
| `FR-VOICE-002` | Voice Messages | Inline Voice Player | `attachments` | `TC-VOICE-01` | High Priority MVP target |
| `FR-VOICE-003` | Voice Messages | Audio Controls | Web Audio / HTML5 | `TC-VOICE-01` | Standard browser audio API |
| `FR-VOICE-004` | Voice Messages | Duration Parser | `attachments.duration_ms` | `TC-VOICE-01` | Parsed from file headers |
| `FR-VOICE-005` | Voice Messages | Path Resolution | `attachments.storage_path` | `TS-MEDIA-02` | Sandboxed to AppData |
| `FR-VOICE-006` | Voice Messages | Missing State UI | `attachments.status` | `TC-VOICE-02` | Explicit warning state |
| `FR-VOICE-007` | Voice Messages | Locate File Dialog | Native File Dialog (`rfd`) | `TC-VOICE-02` | Native Windows file picker |
| `FR-VOICE-008` | Voice Messages | Playback Mutex | Frontend Audio Manager | `TC-VOICE-01` | Singleton playing instance |
| `FR-VOICE-009` | Voice Messages | Speed Selector | HTML5 PlaybackRate | `TC-VOICE-01` | Supports 1.0x, 1.5x, 2.0x |
| `FR-IMG-001` | Images | Thumbnail Grid | `attachments` | `TS-UI-04` | Local custom protocol |
| `FR-IMG-002` | Images | Lightbox Modal | Frontend Dialog | `TS-UI-04` | DOM modal with keyboard escape |
| `FR-IMG-003` | Images | Carousel / Multi-Img | `attachments` join | `TS-UI-04` | Rendered inside parent container |
| `FR-IMG-004` | Images | Image Preservation | Media Storage Manager | `TS-MEDIA-01` | Verified non-zero file copy |
| `FR-IMG-005` | Images | Missing Image UI | `attachments.status` | `TS-UI-04` | Explicit missing placeholder |
| `FR-VID-001` | Videos | Video Poster Card | `attachments` | `TS-MEDIA-03` | Poster frame + play overlay |
| `FR-VID-002` | Videos | Inline Video Player | HTML5 Video Player | `TS-MEDIA-03` | Range request streaming |
| `FR-VID-003` | Videos | Video Duration | `attachments.duration_ms` | `TS-MEDIA-03` | Extracted from video metadata |
| `FR-VID-004` | Videos | Storage Integrity | `attachments.sha256_checksum` | `TS-MEDIA-01` | SHA-256 verified |
| `FR-VID-005` | Videos | Missing Video UI | `attachments.status` | `TS-UI-04` | Explicit missing placeholder |
| `FR-TIME-001` | Timestamps | Storage Pipeline | `messages.timestamp_ms` | `TC-TIME-01` | Integer 64-bit UTC storage |
| `FR-TIME-002` | Timestamps | Timezone Display | Frontend Date Formatter | `TC-TIME-01` | Converted to OS timezone |
| `FR-TIME-003` | Timestamps | Sorting Engine | `messages` compound index | `TC-TIME-02` | Strict `timestamp_ms ASC` |
| `FR-TIME-004` | Timestamps | Equal Time Stability | Secondary sort `source_array_index` | `TC-TIME-02` | Deterministic ordering |
| `FR-TIME-005` | Timestamps | Date Separators | Virtualized Stream Grouper | `TS-UI-03` | Inline day dividers |
| `FR-READ-001` | Read Receipts | Read State Model | `read_receipts.read_status` | `TC-READ-01` | Explicit enum state modeling |
| `FR-READ-002` | Read Receipts | Outgoing Seen Badge | `read_receipts` | `TC-READ-01` | Rendered only when available |
| `FR-READ-003` | Read Receipts | Seen Timestamp Hover| `read_receipts.read_at_ms` | `TC-READ-01` | Hover inspection |
| `FR-READ-004` | Read Receipts | Relative Dynamic Time| Frontend Time Ago Calc | `TC-READ-01` | e.g. "Seen 15m ago" |
| `FR-READ-005` | Read Receipts | Archive Fallback | `read_receipts.read_status` | `TC-READ-01` | Defaults to `UNAVAILABLE_FROM_SOURCE` |
| `FR-READ-006` | Read Receipts | Zero Hallucination | Ingestion Validator | `TC-READ-01` | Never inferred from activity |
| `FR-REACT-001` | Reactions | Reaction Badges | `reactions` table | `TC-REACT-01` | Emoji pill pinned to bubble |
| `FR-REACT-002` | Reactions | Emoji Fidelity | Normalization pipeline | `TC-REACT-01` | Latin-1 mojibake fix applied |
| `FR-REACT-003` | Reactions | Reactor Tooltip | `reactions.participant_id` | `TC-REACT-01` | Shows handle on hover |
| `FR-REACT-004` | Reactions | Multi-Reactor Group | SQL Aggregation | `TC-REACT-01` | Aggregates duplicate emojis |
| `FR-REACT-005` | Reactions | Reaction Dedup | `reactions` UNIQUE key | `TC-REACT-01` | Re-imports remain idempotent |
| `FR-REPLY-001` | Replies | Parent Linker | `messages.reply_to_message_id`| `TC-REPLY-01` | Linked only when ID in source |
| `FR-REPLY-002` | Replies | Quoted Preview Bar | Message Bubble Header | `TC-REPLY-01` | Displays author & quote snippet |
| `FR-REPLY-003` | Replies | Click-to-Scroll | Virtualizer Scroll To | `TC-REPLY-01` | Enabled only when parent linked |
| `FR-REPLY-004` | Replies | Archive Absence Note| Message Bubble Header | `TC-REPLY-01` | Shows preview text without link |
| `FR-REPLY-005` | Replies | Missing Parent State| Message Bubble Header | `TC-REPLY-01` | Shows "Original unavailable" |
| `FR-REEL-001` | Shared Content | Content Classifier | Ingestion Regex / Flags | `TS-UI-05` | Recognizes `instagram.com/reel` |
| `FR-REEL-002` | Shared Content | Reel Card Component | Message Bubble Card | `TS-UI-05` | Formatted card with icon |
| `FR-REEL-003` | Shared Content | URL Preservation | `messages.raw_payload_json` | `TS-UI-05` | Preserves original web link |
| `FR-REEL-004` | Shared Content | Default Browser Launch| Windows ShellExecute | `TS-UI-05` | Opens external browser safely |
| `FR-REEL-005` | Shared Content | Fallback Graphic | Card Placeholder | `TS-UI-05` | No fake thumbnails fabricated |
| `FR-CALL-001` | Call Records | Call Event Classifier| Ingestion Engine | `TC-CALL-01` | Mandatory MVP timeline entry |
| `FR-CALL-002` | Call Records | Voice vs Video | `call_events.call_type` | `TC-CALL-01` | Distinct handset / camera icons |
| `FR-CALL-003` | Call Records | Direction Classifier| `call_events.direction` | `TC-CALL-01` | Incoming vs Outgoing |
| `FR-CALL-004` | Call Records | Status Classifier | `call_events.status` | `TC-CALL-02` | Answered, Missed, Declined |
| `FR-CALL-005` | Call Records | Duration Formatter | `call_events.duration_seconds`| `TC-CALL-01` | e.g. "Voice call — 14m 22s" |
| `FR-CALL-006` | Call Records | Derived End Time | `derived_end_timestamp_ms` | `TC-CALL-01` | Labeled explicitly as derived |
| `FR-CALL-007` | Call Records | Caller Attribution | `call_events.caller_id` | `TC-CALL-01` | Attribution to participant |
| `FR-CALL-008` | Call Records | Centered Timeline UI| Timeline Event Badge | `TC-CALL-01` | Subdued centered badge |
| `FR-CALL-009` | Call Records | Unavailable Duration| Badge Fallback Renderer | `TC-CALL-01` | Shows "Duration unavailable" |
| `FR-CALL-010` | Call Records | No Audio Claim | Call Component Validator | `TC-CALL-01` | Never claims call is playable |
| `FR-CALL-011` | Call Records | Unique Event ID | `call_events.id` | `TC-CALL-01` | UUID primary key |
| `FR-CALL-012` | Call Records | Call Event Dedup | Ingestion Deduplicator | `TC-CALL-01` | Idempotent call ingestion |
| `FR-CALL-013` | Call Records | Call Filter | Search & Filter Engine | `TS-DB-03` | Filter by call type |
| `FR-MISC-001` | Other Messages | Sticker / GIF Viewer| Image Attachment Streamer| `TS-UI-04` | Inline image render |
| `FR-MISC-002` | Other Messages | Autolinker | UI Text Component | `TS-UI-03` | Auto-detects URLs |
| `FR-MISC-003` | Other Messages | System Event Badges | Timeline Notice Badge | `TS-UI-03` | Centered event text |
| `FR-MISC-004` | Other Messages | Raw Payload Fallback| Fallback Bubble Component| `TS-UNIT-01` | Never drops unknown records |
| `FR-MISC-005` | Other Messages | Multi-Attachment Box| Message Container | `TS-UI-04` | Grouped attachments in bubble |
| `FR-SRCH-001` | Search | Full-Text Search | SQLite Query (`LIKE`/FTS5)| `TS-DB-03` | Global and thread-level search |
| `FR-SRCH-002` | Search | Keyword Highlight | Frontend Text Matcher | `TS-UI-03` | Highlights match in yellow/bold |
| `FR-SRCH-003` | Search | Sender Filter | SQL Filter Query | `TS-DB-03` | Filter by `sender_id` |
| `FR-SRCH-004` | Search | Date Range Filter | SQL Filter Query | `TS-DB-03` | `timestamp_ms BETWEEN a AND b` |
| `FR-SRCH-005` | Search | Attachment Filter | SQL Join Filter | `TS-DB-03` | Filter by attachment type |
| `FR-SYNC-001` | Import & Sync | Auto-Discovery Parser| `MetaArchiveAdapter` | `TS-IMPORT-01`| Handles legacy, modern, splits |
| `FR-SYNC-002` | Import & Sync | Progress Emitter | Tauri IPC Progress Event | `TS-IMPORT-01`| Live progress modal |
| `FR-SYNC-003` | Import & Sync | Stable Checkpoints | `sync_checkpoints` table | `TC-SYNC-02` | Keyed by `conversation_source_id`|
| `FR-SYNC-004` | Import & Sync | Scoped Deduplication| `identity_hash` Tier 1 & 2 | `TC-SYNC-01` | Preserves identical messages |
| `FR-SYNC-005` | Import & Sync | Media Local Copy | Media Storage Manager | `TS-MEDIA-01` | Copies to sandboxed AppData |
| `FR-SYNC-006` | Import & Sync | Error Log Report | `failed_items` table | `TS-IMPORT-01`| Post-import error summary |
| `FR-SYNC-007` | Import & Sync | Personal Diff Sync | `FileDiffSyncAdapter` | `TS-IMPORT-01`| Offline incremental archive diff |
| `FR-SYNC-008` | Import & Sync | Completeness Summary| Summary Dialog Component | `TS-IMPORT-01`| Reports missing/unavailable data |
| `FR-SYNC-009` | Import & Sync | Windows Startup Key | Win32 `RegSetValueExW` | `TS-UNIT-02` | `HKCU\...\Run` toggle |
| `FR-SYNC-010` | Import & Sync | Graceful Cancellation| Atomic Transaction Guard| `TS-IMPORT-01`| Preserves committed batches |
| `NFR-PERF-001` | Performance | Cold Start Time | Application Bootstrapper | `TC-BENCH-01` | `[Target - Unverified Benchmark]` |
| `NFR-PERF-002` | Performance | 60 FPS Scrolling | Virtualized Viewport | `TC-BENCH-01` | `[Target - Unverified Benchmark]` |
| `NFR-PERF-003` | Performance | Search Latency | SQLite Query Optimizer | `TC-BENCH-01` | `[Target - Unverified Benchmark]` |
| `NFR-MEM-001` | Memory Usage | Baseline RAM | Memory Profiler | `TC-BENCH-01` | `[Target - Unverified Benchmark]` |
| `NFR-MEM-002` | Memory Usage | Media Streaming | HTTP 206 Custom Protocol | `TS-MEDIA-03` | Never loads file into JS heap |
| `NFR-SCALE-001`| Scalability | Large Archive Cap. | SQLite WAL Configuration | `TC-BENCH-01` | Tested up to 500k messages |
| `NFR-SCALE-002`| Scalability | Index Efficiency | SQLite Database Analyzer | `TS-DB-02` | Index overhead target < 35% |
| `NFR-REL-001` | Reliability | WAL Crash Resilience| SQLite WAL Mode | `TS-CHAOS-01` | Durable on power loss |
| `NFR-REL-002` | Reliability | Atomic Transactions | SQLite Transaction Guard | `TC-SYNC-02` | 500-1000 batch rollback safety |
| `NFR-REL-003` | Reliability | Standard User Mutex | Session Mutex `Local\` | `TC-WIN-01` | Runs without admin rights |
| `NFR-DATA-001` | Data Integrity | Media Checksums | SHA-256 Engine | `TS-MEDIA-01` | Prevents disk corruption |
| `NFR-DATA-002` | Data Integrity | Foreign Keys | SQLite `PRAGMA foreign_keys`| `TS-DB-01` | Enforces relational integrity |
| `NFR-DATA-003` | Data Integrity | Sandboxed AppData | Windows Path Resolver | `TS-UNIT-02` | Standard `%LOCALAPPDATA%` path |
| `NFR-SEC-001` | Security | Local-First Zero Telemetry| Network Activity Monitor| `TS-UNIT-02` | Zero external network calls |
| `NFR-SEC-002` | Security | Credential Encryption | Windows DPAPI / Credential Mgr| `TS-UNIT-02` | Encrypts optional tokens |
| `NFR-SEC-003` | Security | PII Log Redaction | Logging Middleware | `TS-UNIT-02` | Redacts messages and tokens |
| `NFR-SEC-004` | Security | Purge All Data | Data Purge Service | `TS-UNIT-02` | One-click complete deletion |
| `NFR-A11Y-001` | Accessibility | Keyboard Navigation | UI Focus Management | `TS-UI-03` | Tab, Enter, Space, Escape, Ctrl+F|
| `NFR-A11Y-002` | Accessibility | Screen Reader Markup| Semantic HTML / ARIA | `TS-UI-03` | WCAG 2.1 AA screen reader tags |
| `NFR-A11Y-003` | Accessibility | High Contrast Themes| Tailwind Theme Tokens | `TS-UI-03` | Dark/Light mode >4.5:1 contrast |
| `NFR-MAINT-001`| Maintainability| Decoupled Services | Rust Module Architecture | `TS-UNIT-01` | Clean traits and boundaries |
| `NFR-MAINT-002`| Maintainability| Automated Test Suite| CI Test Runner | `TS-UNIT-01` | Automated regression prevention |
