//! Phase 4.1 End-to-End Runtime Integration & Verification Test Suite
//!
//! Governed by PRD.md, TRD.md Section 5, and GEMINI.md.
//! Tests the full integration chain:
//! 1. Synthetic multi-conversation archive creation (Unicode, identical messages, media, reactions, calls, corruptions)
//! 2. Archive source validation and discovery
//! 3. Initial import execution and report verification
//! 4. Database queries and hydration (conversations, messages, attachments, reactions, read receipts, calls, replies)
//! 5. Search and filtering (audio, media, links, calls)
//! 6. Re-import idempotence (zero duplicate messages created, legitimate identical messages preserved)
//! 7. Zero-memory media streaming protocol (HTTP 200, HTTP 206 Range requests, 404 missing, 403 traversal)
//! 8. Scalability & pagination benchmark on a 1,000-message thread

use instagram_chat_history_viewer_lib::db::Database;
use instagram_chat_history_viewer_lib::importer::{ArchiveSource, Importer};
use instagram_chat_history_viewer_lib::media::handle_media_request;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use uuid::Uuid;

struct TestEnvironment {
    temp_dir: PathBuf,
    archive_dir: PathBuf,
    db_path: PathBuf,
    media_dir: PathBuf,
}

impl TestEnvironment {
    fn setup() -> Self {
        let temp_dir = std::env::temp_dir().join(format!("ig_e2e_{}", Uuid::new_v4()));
        let archive_dir = temp_dir.join("archive");
        let db_path = temp_dir.join("chat.db");
        let media_dir = temp_dir.join("media");

        fs::create_dir_all(&archive_dir).unwrap();
        fs::create_dir_all(&media_dir).unwrap();

        Self {
            temp_dir,
            archive_dir,
            db_path,
            media_dir,
        }
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

fn create_e2e_archive_fixture(archive_root: &Path) {
    let inbox = archive_root
        .join("your_instagram_activity")
        .join("messages")
        .join("inbox");
    fs::create_dir_all(&inbox).unwrap();

    // -------------------------------------------------------------
    // Conversation 1: 1-on-1 with Alice (Unicode, repeated identical messages, media, reactions, reply)
    // -------------------------------------------------------------
    let conv1_dir = inbox.join("alice_smith_111");
    let conv1_audio_dir = conv1_dir.join("audio");
    let conv1_photos_dir = conv1_dir.join("photos");
    let conv1_videos_dir = conv1_dir.join("videos");
    fs::create_dir_all(&conv1_audio_dir).unwrap();
    fs::create_dir_all(&conv1_photos_dir).unwrap();
    fs::create_dir_all(&conv1_videos_dir).unwrap();

    // Create synthetic media files
    let voice_bytes = b"M4A_SYNTHETIC_VOICE_NOTE_AUDIO_DATA_BYTES_1234567890";
    fs::write(conv1_audio_dir.join("voice_note.mp4"), voice_bytes).unwrap();

    let photo_bytes = b"JPEG_SYNTHETIC_IMAGE_DATA_BYTES";
    fs::write(conv1_photos_dir.join("photo.jpg"), photo_bytes).unwrap();

    let video_bytes = b"MP4_SYNTHETIC_VIDEO_DATA_BYTES_CHUNK_ABCDEFGHIJ";
    fs::write(conv1_videos_dir.join("video.mp4"), video_bytes).unwrap();

    let conv1_json = serde_json::json!({
        "participants": [
            { "name": "Alice Smith" },
            { "name": "my_account" }
        ],
        "messages": [
            {
                "sender_name": "Alice Smith",
                "timestamp_ms": 1700000000000i64,
                "content": "Hello! Welcome to our chat \u{00f0}\u{009f}\u{008c}\u{00b8}\u{00f0}\u{009f}\u{0091}\u{008b}", // Emojis
                "reactions": [
                    { "reaction": "\u{00e2}\u{009d}\u{00a4}", "actor": "my_account" } // Heart emoji (Mojibake encoded)
                ]
            },
            // Legitimate identical repeated message (sent twice at same timestamp)
            {
                "sender_name": "my_account",
                "timestamp_ms": 1700000005000i64,
                "content": "ok"
            },
            {
                "sender_name": "my_account",
                "timestamp_ms": 1700000005000i64,
                "content": "ok"
            },
            // Voice message
            {
                "sender_name": "Alice Smith",
                "timestamp_ms": 1700000010000i64,
                "audio_files": [
                    { "uri": "your_instagram_activity/messages/inbox/alice_smith_111/audio/voice_note.mp4" }
                ]
            },
            // Image and Video message
            {
                "sender_name": "my_account",
                "timestamp_ms": 1700000020000i64,
                "photos": [
                    { "uri": "your_instagram_activity/messages/inbox/alice_smith_111/photos/photo.jpg" }
                ],
                "videos": [
                    { "uri": "your_instagram_activity/messages/inbox/alice_smith_111/videos/video.mp4" }
                ]
            },
            // Quoted reply
            {
                "sender_name": "Alice Smith",
                "timestamp_ms": 1700000030000i64,
                "content": "Thanks for the photos!",
                "reply_to_text": "ok"
            }
        ],
        "title": "Alice Smith",
        "is_still_participant": true
    });

    fs::write(
        conv1_dir.join("message_1.json"),
        serde_json::to_string_pretty(&conv1_json).unwrap(),
    )
    .unwrap();

    // -------------------------------------------------------------
    // Conversation 2: Group Chat "Travel Group" (Shared Reel, Calls)
    // -------------------------------------------------------------
    let conv2_dir = inbox.join("travel_group_222");
    fs::create_dir_all(&conv2_dir).unwrap();

    let conv2_json = serde_json::json!({
        "participants": [
            { "name": "Bob Traveler" },
            { "name": "Charlie Globetrotter" },
            { "name": "my_account" }
        ],
        "messages": [
            // Shared Reel
            {
                "sender_name": "Bob Traveler",
                "timestamp_ms": 1700000100000i64,
                "share": {
                    "link": "https://www.instagram.com/reel/C1234567890/",
                    "share_text": "Check out this amazing place in Switzerland!"
                }
            },
            // Answered Voice Call
            {
                "sender_name": "Bob Traveler",
                "timestamp_ms": 1700000200000i64,
                "call_duration": 360,
                "content": "Bob Traveler started an audio call"
            },
            // Missed Video Call (missing duration)
            {
                "sender_name": "Charlie Globetrotter",
                "timestamp_ms": 1700000300000i64,
                "content": "Charlie Globetrotter started a video chat"
            }
        ],
        "title": "Travel Group",
        "is_still_participant": true
    });

    fs::write(
        conv2_dir.join("message_1.json"),
        serde_json::to_string_pretty(&conv2_json).unwrap(),
    )
    .unwrap();

    // -------------------------------------------------------------
    // Conversation 3: Corrupted / Partial Chat (Missing media & truncated JSON)
    // -------------------------------------------------------------
    let conv3_dir = inbox.join("corrupted_chat_333");
    fs::create_dir_all(&conv3_dir).unwrap();

    let conv3_json = serde_json::json!({
        "participants": [
            { "name": "Dave Lost" },
            { "name": "my_account" }
        ],
        "messages": [
            {
                "sender_name": "Dave Lost",
                "timestamp_ms": 1700000400000i64,
                "content": "Here is a file that was deleted from export"
            },
            {
                "sender_name": "Dave Lost",
                "timestamp_ms": 1700000410000i64,
                "audio_files": [
                    { "uri": "your_instagram_activity/messages/inbox/corrupted_chat_333/audio/missing_note.mp4" }
                ]
            }
        ],
        "title": "Dave Lost",
        "is_still_participant": true
    });

    fs::write(
        conv3_dir.join("message_1.json"),
        serde_json::to_string_pretty(&conv3_json).unwrap(),
    )
    .unwrap();
    // Intentionally corrupted second message file to test error tolerance
    fs::write(
        conv3_dir.join("message_2.json"),
        b"{\"participants\": [ TRUNCATED INVALID JSON",
    )
    .unwrap();
}

#[test]
fn test_end_to_end_runtime_integration_and_audit() {
    let env = TestEnvironment::setup();
    create_e2e_archive_fixture(&env.archive_dir);

    // =============================================================
    // Step 1: Discover & Validate Archive
    // =============================================================
    let source = ArchiveSource::open(&env.archive_dir).expect("Failed to open archive source");
    let summary = source.discover_and_validate().expect("Validation failed");

    // Archive has 1 corrupted file (message_2.json in corrupted_chat_333), so validation detects the error
    assert!(
        !summary.errors.is_empty(),
        "Malformed file should be detected in validation errors"
    );
    assert_eq!(summary.conversations.len(), 3, "Expected 3 conversations");
    assert_eq!(
        summary.total_message_files, 3,
        "Expected 3 valid message json files parsed"
    );

    // =============================================================
    // Step 2: Ingest into Database via Importer
    // =============================================================
    let mut db = Database::open_at(&env.db_path).expect("Failed to open test database");
    let mut importer = Importer::new(&mut db, &env.archive_dir, "my_account", &env.media_dir)
        .expect("Failed to create importer");

    let report = importer.run_import().expect("Import failed");

    assert_eq!(report.conversations_discovered, 3);
    assert_eq!(
        report.messages_imported, 11,
        "Expected 11 successfully imported messages"
    );
    assert_eq!(
        report.attachments_imported, 3,
        "Voice, photo, and video should be copied"
    );
    assert_eq!(
        report.attachments_missing, 1,
        "Missing note should be recorded as missing"
    );
    assert_eq!(report.call_events_imported, 2, "2 calls should be recorded");
    assert!(
        !report.errors.is_empty(),
        "Malformed message_2.json should produce an error entry"
    );

    // =============================================================
    // Step 3: Verify Database Query Layer & Hydration
    // =============================================================

    // 3.1: List Conversations
    let (conversations, total_convs) = db
        .list_conversations(None, None, 10, 0)
        .expect("Failed to list conversations");
    assert_eq!(total_convs, 3);
    assert_eq!(conversations.len(), 3);

    let alice_conv = conversations
        .iter()
        .find(|c| c.title == "Alice Smith")
        .expect("Alice chat missing");
    let travel_conv = conversations
        .iter()
        .find(|c| c.title == "Travel Group")
        .expect("Travel group missing");
    let dave_conv = conversations
        .iter()
        .find(|c| c.title == "Dave Lost")
        .expect("Dave chat missing");

    assert_eq!(alice_conv.message_count, 6);
    assert_eq!(travel_conv.message_count, 3);
    assert_eq!(dave_conv.message_count, 2);

    // 3.2: Verify Conversation Statistics (Info Drawer)
    let alice_stats = db
        .get_conversation_stats(&alice_conv.id)
        .expect("Failed to get stats for Alice");
    assert_eq!(alice_stats.total_messages, 6);
    assert_eq!(alice_stats.total_audio, 1);
    assert_eq!(alice_stats.total_images, 1);
    assert_eq!(alice_stats.total_videos, 1);
    assert_eq!(alice_stats.total_calls, 0);

    let travel_stats = db
        .get_conversation_stats(&travel_conv.id)
        .expect("Failed to get stats for Travel Group");
    assert_eq!(travel_stats.total_messages, 3);
    assert_eq!(travel_stats.total_links, 1);
    assert_eq!(travel_stats.total_calls, 2);

    // 3.3: Verify Messages Hydration & Chronological Ordering
    let alice_msgs = db
        .get_messages(&alice_conv.id, 50, 0, None, &env.media_dir)
        .expect("Failed to get Alice messages");
    assert_eq!(alice_msgs.total_count, 6);
    assert_eq!(alice_msgs.messages.len(), 6);

    // Message 0: Unicode Text & Heart Reaction
    let m0 = &alice_msgs.messages[0];
    assert!(
        m0.content.as_ref().unwrap().contains("🌸👋"),
        "Unicode emojis should be preserved"
    );
    assert_eq!(m0.reactions.len(), 1);
    assert!(
        m0.reactions[0].reaction_value.starts_with('❤'),
        "Mojibake heart should be decoded to ❤"
    );
    assert_eq!(m0.reactions[0].username, "my_account");

    // Message 1 & 2: Repeated Identical Messages Preserved
    let m1 = &alice_msgs.messages[1];
    let m2 = &alice_msgs.messages[2];
    assert_eq!(m1.content.as_deref(), Some("ok"));
    assert_eq!(m2.content.as_deref(), Some("ok"));
    assert_ne!(
        m1.id, m2.id,
        "Repeated identical messages must have distinct IDs"
    );
    assert_eq!(m1.timestamp_ms, m2.timestamp_ms);

    // Message 3: Voice Note Attachment
    let m3 = &alice_msgs.messages[3];
    assert_eq!(m3.attachments.len(), 1);
    let voice_att = &m3.attachments[0];
    assert_eq!(voice_att.attachment_type, "audio");
    assert!(
        voice_att.is_available,
        "Voice note file should exist locally"
    );
    assert!(voice_att
        .media_url
        .as_ref()
        .unwrap()
        .starts_with("media://localhost/"));

    // Message 4: Photo & Video Attachments
    let m4 = &alice_msgs.messages[4];
    assert_eq!(m4.attachments.len(), 2);
    let photo_att = m4
        .attachments
        .iter()
        .find(|a| a.attachment_type == "image")
        .expect("Photo missing");
    let video_att = m4
        .attachments
        .iter()
        .find(|a| a.attachment_type == "video")
        .expect("Video missing");
    assert!(photo_att.is_available);
    assert!(video_att.is_available);

    // Message 5: Quoted Reply
    let m5 = &alice_msgs.messages[5];
    assert!(
        m5.reply_preview.is_some(),
        "Quoted reply should be hydrated"
    );
    let reply = m5.reply_preview.as_ref().unwrap();
    assert_eq!(reply.snippet.as_deref(), Some("ok"));

    // 3.4: Verify Shared Reel and Calls in Travel Group
    let travel_msgs = db
        .get_messages(&travel_conv.id, 50, 0, None, &env.media_dir)
        .expect("Failed to get Travel messages");
    assert_eq!(travel_msgs.messages.len(), 3);

    // Shared Reel
    let share_msg = &travel_msgs.messages[0];
    assert_eq!(
        share_msg.share_link.as_deref(),
        Some("https://www.instagram.com/reel/C1234567890/")
    );
    assert!(share_msg
        .share_text
        .as_ref()
        .unwrap()
        .contains("Switzerland"));

    // Answered Call
    let call_msg1 = &travel_msgs.messages[1];
    assert!(call_msg1.call_event.is_some());
    let call1 = call_msg1.call_event.as_ref().unwrap();
    assert_eq!(call1.call_type, "VOICE");
    assert_eq!(call1.status, "ANSWERED");
    assert_eq!(call1.duration_seconds, Some(360));

    // Missed Call (No duration)
    let call_msg2 = &travel_msgs.messages[2];
    assert!(call_msg2.call_event.is_some());
    let call2 = call_msg2.call_event.as_ref().unwrap();
    assert_eq!(call2.call_type, "VIDEO");
    assert_eq!(
        call2.duration_seconds, None,
        "Missing duration should be None (not fabricated)"
    );

    // 3.5: Missing Media in Dave's chat
    let dave_msgs = db
        .get_messages(&dave_conv.id, 50, 0, None, &env.media_dir)
        .expect("Failed to get Dave messages");
    let missing_msg = &dave_msgs.messages[1];
    assert_eq!(missing_msg.attachments.len(), 1);
    let missing_att = &missing_msg.attachments[0];
    assert_eq!(missing_att.status, "MISSING");
    assert!(
        !missing_att.is_available,
        "Missing attachment must report is_available = false"
    );

    // =============================================================
    // Step 4: Search and Filter Verification
    // =============================================================
    // 4.1 Keyword Search
    let search_res = db
        .search_messages("Switzerland", None, 10, 0, &env.media_dir)
        .expect("Search failed");
    assert_eq!(search_res.total_count, 1);
    assert_eq!(search_res.results[0].conversation_title, "Travel Group");

    // 4.2 Audio Filter
    let att_rows: Vec<(String, String, String)> = db
        .conn()
        .prepare("SELECT id, message_id, attachment_type FROM attachments")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    println!("Attachments in DB: {:?}", att_rows);

    let (audio_convs, audio_count) = db
        .list_conversations(None, Some("audio"), 10, 0)
        .expect("Audio filter failed");
    assert_eq!(
        audio_count, 2,
        "Alice and Dave chats have audio attachment records"
    );
    assert!(audio_convs.iter().any(|c| c.title == "Alice Smith"));
    assert!(audio_convs.iter().any(|c| c.title == "Dave Lost"));

    // 4.3 Calls Filter
    let (call_convs, call_count) = db
        .list_conversations(None, Some("calls"), 10, 0)
        .expect("Call filter failed");
    assert_eq!(call_count, 1, "Only Travel Group has call events");
    assert_eq!(call_convs[0].title, "Travel Group");

    // =============================================================
    // Step 5: Re-import Idempotence Check
    // =============================================================
    let mut importer_re = Importer::new(&mut db, &env.archive_dir, "my_account", &env.media_dir)
        .expect("Failed to create second importer");
    let re_report = importer_re.run_import().expect("Re-import failed");

    assert_eq!(
        re_report.messages_imported, 0,
        "No duplicate messages should be imported on second run"
    );
    assert_eq!(
        re_report.messages_skipped, 11,
        "All 11 existing messages should be skipped as duplicates"
    );

    // Verify total message count in database is still exactly 11
    let count_after_reimport: i64 = db
        .conn()
        .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count_after_reimport, 11,
        "Total message count must remain unchanged after re-import"
    );

    // =============================================================
    // Step 6: Zero-Memory Streaming Protocol Verification
    // =============================================================
    let voice_storage_path = voice_att.storage_path.as_ref().unwrap();

    // 6.1 Full Content Request (200 OK)
    let full_req = tauri::http::Request::builder()
        .uri(format!("media://localhost/{}", voice_storage_path))
        .body(Vec::new())
        .unwrap();
    let full_resp = handle_media_request(&full_req, &env.media_dir);
    assert_eq!(full_resp.status(), 200);
    assert_eq!(
        full_resp.headers().get("Content-Type").unwrap(),
        "audio/mp4"
    );
    assert_eq!(
        full_resp.body(),
        b"M4A_SYNTHETIC_VOICE_NOTE_AUDIO_DATA_BYTES_1234567890"
    );

    // 6.2 Range Request (206 Partial Content)
    let range_req = tauri::http::Request::builder()
        .uri(format!("media://localhost/{}", voice_storage_path))
        .header("Range", "bytes=4-19")
        .body(Vec::new())
        .unwrap();
    let range_resp = handle_media_request(&range_req, &env.media_dir);
    assert_eq!(range_resp.status(), 206);
    assert_eq!(
        range_resp.headers().get("Content-Range").unwrap(),
        "bytes 4-19/52"
    );
    assert_eq!(range_resp.headers().get("Content-Length").unwrap(), "16");
    assert_eq!(
        range_resp.body(),
        &b"M4A_SYNTHETIC_VOICE_NOTE_AUDIO_DATA_BYTES_1234567890"[4..=19]
    );

    // 6.3 Security: Path Traversal Rejection (403 Forbidden)
    let traversal_req = tauri::http::Request::builder()
        .uri("media://localhost/../../chat.db")
        .body(Vec::new())
        .unwrap();
    let traversal_resp = handle_media_request(&traversal_req, &env.media_dir);
    assert_eq!(traversal_resp.status(), 403);

    // 6.4 Missing File (404 Not Found)
    let missing_req = tauri::http::Request::builder()
        .uri("media://localhost/non_existent.mp4")
        .body(Vec::new())
        .unwrap();
    let missing_resp = handle_media_request(&missing_req, &env.media_dir);
    assert_eq!(missing_resp.status(), 404);

    // =============================================================
    // Step 7: Long Conversation Pagination & Performance Benchmark
    // =============================================================
    let long_conv_id = Uuid::new_v4().to_string();
    let sender_id = Uuid::new_v4().to_string();

    db.conn().execute(
        "INSERT INTO conversations (id, source_identifier, conversation_type, title, updated_at_ms)
         VALUES (?1, 'benchmark_conv', 'DIRECT', 'Benchmark Chat', 2000000000000)",
        rusqlite::params![long_conv_id],
    ).unwrap();

    db.conn().execute(
        "INSERT INTO participants (id, source_identifier, username, is_self) VALUES (?1, 'bench_user', 'bench_user', 0)",
        rusqlite::params![sender_id],
    ).unwrap();

    db.conn().execute(
        "INSERT INTO conversation_participants (conversation_id, participant_id) VALUES (?1, ?2)",
        rusqlite::params![long_conv_id, sender_id],
    ).unwrap();

    // Insert 1,000 synthetic messages in a transaction
    {
        let tx = db.conn_mut().transaction().unwrap();
        for i in 0..1000 {
            tx.execute(
                "INSERT INTO messages (id, conversation_id, sender_id, identity_hash, message_type, content, timestamp_ms, is_outgoing, source_array_index)
                 VALUES (?1, ?2, ?3, ?4, 'TEXT', ?5, ?6, 0, ?7)",
                rusqlite::params![
                    Uuid::new_v4().to_string(),
                    long_conv_id,
                    sender_id,
                    format!("bench_hash_{}", i),
                    format!("Benchmark message number {}", i),
                    1600000000000i64 + (i as i64 * 1000),
                    i as i64,
                ],
            ).unwrap();
        }
        tx.commit().unwrap();
    }

    // Benchmark paginated retrieval (page size = 50) at various offsets
    let start_t = Instant::now();
    let page1 = db
        .get_messages(&long_conv_id, 50, 0, None, &env.media_dir)
        .unwrap();
    let page1_duration = start_t.elapsed();

    assert_eq!(page1.total_count, 1000);
    assert_eq!(page1.messages.len(), 50);
    // Verified chronological ordering on the page
    assert!(page1.messages[0].timestamp_ms < page1.messages[49].timestamp_ms);

    let start_t500 = Instant::now();
    let page500 = db
        .get_messages(&long_conv_id, 50, 500, None, &env.media_dir)
        .unwrap();
    let page500_duration = start_t500.elapsed();

    assert_eq!(page500.messages.len(), 50);

    println!(
        "Benchmark Results (1,000 messages in SQLite): Page 1 query time: {:?}, Offset 500 query time: {:?}",
        page1_duration, page500_duration
    );

    // Both queries should complete well under 20ms on Windows SQLite WAL
    assert!(
        page1_duration.as_millis() < 50,
        "Pagination query should be fast (<50ms)"
    );
    assert!(
        page500_duration.as_millis() < 50,
        "Offset pagination query should be fast (<50ms)"
    );
}
