//! Query-Focused Database Access Layer for Dashboard and Chat Interface
//!
//! Governed by PRD.md, TRD.md Section 5, and GEMINI.md.
//! Implements bounded pagination, scoped search, media reference validation,
//! and accurate hydration of reactions, read receipts, and call events.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::db::{Database, DbError};

/// Summary of a participant in a conversation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParticipantSummary {
    pub id: String,
    pub username: String,
    pub full_name: Option<String>,
    pub is_self: bool,
}

/// Summary of a conversation for sidebar rendering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: String,
    pub source_identifier: String,
    pub conversation_type: String,
    pub title: String,
    pub participants: Vec<ParticipantSummary>,
    pub last_message_preview: Option<String>,
    pub last_message_timestamp_ms: Option<i64>,
    pub message_count: u64,
    pub is_archived: bool,
}

/// Statistical overview of a conversation for the info drawer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationStats {
    pub id: String,
    pub title: String,
    pub conversation_type: String,
    pub participants: Vec<ParticipantSummary>,
    pub total_messages: u64,
    pub total_audio: u64,
    pub total_images: u64,
    pub total_videos: u64,
    pub total_links: u64,
    pub total_calls: u64,
    pub first_message_timestamp_ms: Option<i64>,
    pub last_message_timestamp_ms: Option<i64>,
}

/// View model for an attachment
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentView {
    pub id: String,
    pub attachment_type: String, // 'audio', 'image', 'video', 'file'
    pub original_uri: Option<String>,
    pub storage_path: Option<String>,
    pub media_url: Option<String>, // 'media://localhost/...'
    pub mime_type: String,
    pub file_size_bytes: Option<i64>,
    pub sha256_checksum: Option<String>,
    pub duration_ms: Option<i64>,
    pub status: String, // 'LOCAL_COPIED' or 'MISSING'
    pub is_available: bool,
}

/// View model for an emoji reaction
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReactionView {
    pub reaction_value: String,
    pub participant_id: String,
    pub username: String,
}

/// View model for read receipts (explicitly models unavailable vs real read status)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadReceiptView {
    pub participant_id: String,
    pub username: String,
    pub read_status: String, // 'READ' or 'UNAVAILABLE_FROM_SOURCE'
    pub read_at_ms: Option<i64>,
}

/// View model for a call event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallEventView {
    pub id: String,
    pub call_type: String, // 'VOICE' or 'VIDEO'
    pub direction: String, // 'INCOMING' or 'OUTGOING'
    pub status: String,    // 'ANSWERED', 'MISSED', 'DECLINED', etc.
    pub start_timestamp_ms: i64,
    pub duration_seconds: Option<i64>,
    pub derived_end_timestamp_ms: Option<i64>,
}

/// Quoted message preview snippet
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplyPreview {
    pub parent_message_id: Option<String>,
    pub parent_sender_username: Option<String>,
    pub snippet: Option<String>,
}

/// Complete hydrated message view model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageView {
    pub id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub sender_username: String,
    pub sender_full_name: Option<String>,
    pub is_outgoing: bool,
    pub source_message_id: Option<String>,
    pub message_type: String, // 'TEXT', 'VOICE', 'IMAGE', 'VIDEO', 'SHARE', 'CALL', 'UNKNOWN'
    pub content: Option<String>,
    pub timestamp_ms: i64,
    pub share_link: Option<String>,
    pub share_text: Option<String>,
    pub reply_to_message_id: Option<String>,
    pub reply_preview: Option<ReplyPreview>,
    pub attachments: Vec<AttachmentView>,
    pub reactions: Vec<ReactionView>,
    pub read_receipts: Vec<ReadReceiptView>,
    pub call_event: Option<CallEventView>,
}

/// Paginated message response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedMessages {
    pub conversation_id: String,
    pub total_count: u64,
    pub offset: u32,
    pub limit: u32,
    pub messages: Vec<MessageView>,
}

/// Individual item in search results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultItem {
    pub message: MessageView,
    pub conversation_title: String,
}

/// Complete search results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
    pub query: String,
    pub total_count: u64,
    pub results: Vec<SearchResultItem>,
}

/// Derives a clean fallback title when conversations.title is NULL or empty
pub fn derive_conversation_title(
    title: Option<&str>,
    participants: &[ParticipantSummary],
    conv_type: &str,
) -> String {
    if let Some(t) = title {
        let trimmed = t.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    // Filter out self if other participants exist
    let others: Vec<&ParticipantSummary> = participants.iter().filter(|p| !p.is_self).collect();
    if !others.is_empty() {
        let names: Vec<&str> = others
            .iter()
            .map(|p| p.full_name.as_deref().unwrap_or(&p.username))
            .collect();
        return names.join(", ");
    }

    if let Some(self_p) = participants.iter().find(|p| p.is_self) {
        return format!("{} (You)", self_p.username);
    }

    if conv_type.eq_ignore_ascii_case("GROUP") {
        "Group Chat".to_string()
    } else {
        "Direct Message".to_string()
    }
}

/// Validates that an external URL is safe to open in browser (http/https only)
pub fn validate_external_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Some(trimmed.to_string())
    } else {
        None
    }
}

/// Helper to fetch participants for a given conversation
pub fn fetch_participants_for_conversation(
    conn: &Connection,
    conversation_id: &str,
) -> Result<Vec<ParticipantSummary>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.username, p.full_name, p.is_self
         FROM participants p
         JOIN conversation_participants cp ON cp.participant_id = p.id
         WHERE cp.conversation_id = ?1
         ORDER BY p.is_self ASC, p.username ASC",
    )?;

    let rows = stmt.query_map(params![conversation_id], |r| {
        let is_self_int: i64 = r.get(3)?;
        Ok(ParticipantSummary {
            id: r.get(0)?,
            username: r.get(1)?,
            full_name: r.get(2)?,
            is_self: is_self_int != 0,
        })
    })?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

/// Extension methods on Database for querying
impl Database {
    /// Lists conversations with search and filter capabilities
    pub fn list_conversations(
        &self,
        query: Option<&str>,
        filter: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<ConversationSummary>, u64), DbError> {
        let conn = self.conn();
        let limit = limit.clamp(1, 100);

        // Build WHERE clause
        let mut where_clauses: Vec<String> = Vec::new();
        let mut param_values: Vec<String> = Vec::new();

        if let Some(q) = query {
            let trimmed = q.trim();
            if !trimmed.is_empty() {
                let pattern = format!("%{}%", trimmed);
                param_values.push(pattern.clone());
                param_values.push(pattern.clone());
                param_values.push(pattern);
                let idx1 = param_values.len() - 2;
                let idx2 = param_values.len() - 1;
                let idx3 = param_values.len();
                where_clauses.push(format!(
                    "(c.title LIKE ?{} OR EXISTS (
                        SELECT 1 FROM conversation_participants cp
                        JOIN participants p ON p.id = cp.participant_id
                        WHERE cp.conversation_id = c.id AND (p.username LIKE ?{} OR p.full_name LIKE ?{})
                    ))",
                    idx1, idx2, idx3
                ));
            }
        }

        if let Some(f) = filter {
            match f.to_ascii_lowercase().as_str() {
                "audio" => {
                    where_clauses.push(
                        "EXISTS (SELECT 1 FROM messages m JOIN attachments a ON a.message_id = m.id WHERE m.conversation_id = c.id AND UPPER(a.attachment_type) = 'AUDIO')".to_string()
                    );
                }
                "media" | "images" | "videos" => {
                    where_clauses.push(
                        "EXISTS (SELECT 1 FROM messages m JOIN attachments a ON a.message_id = m.id WHERE m.conversation_id = c.id AND UPPER(a.attachment_type) IN ('IMAGE', 'VIDEO'))".to_string()
                    );
                }
                "links" => {
                    where_clauses.push(
                        "EXISTS (SELECT 1 FROM messages m WHERE m.conversation_id = c.id AND (m.message_type = 'SHARE' OR m.content LIKE '%http%'))".to_string()
                    );
                }
                "calls" => {
                    where_clauses.push(
                        "EXISTS (SELECT 1 FROM call_events ce WHERE ce.conversation_id = c.id)"
                            .to_string(),
                    );
                }
                _ => {}
            }
        }

        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        // Count total
        let count_query = format!("SELECT COUNT(*) FROM conversations c {}", where_sql);
        let mut count_stmt = conn.prepare(&count_query)?;
        let count_params = rusqlite::params_from_iter(param_values.iter());
        let total_count: u64 = count_stmt.query_row(count_params, |r| r.get(0))?;

        // Query rows
        let select_query = format!(
            "SELECT c.id, c.source_identifier, c.conversation_type, c.title,
                    c.updated_at_ms, c.last_message_preview, c.is_archived,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id) as msg_count,
                    (SELECT MAX(m.timestamp_ms) FROM messages m WHERE m.conversation_id = c.id) as max_msg_ts
             FROM conversations c
             {}
             ORDER BY COALESCE((SELECT MAX(m.timestamp_ms) FROM messages m WHERE m.conversation_id = c.id), c.updated_at_ms) DESC
             LIMIT ?{} OFFSET ?{}",
            where_sql,
            param_values.len() + 1,
            param_values.len() + 2
        );

        let mut select_stmt = conn.prepare(&select_query)?;
        let mut all_params = param_values;
        all_params.push(limit.to_string());
        all_params.push(offset.to_string());

        let rows = select_stmt.query_map(rusqlite::params_from_iter(all_params.iter()), |r| {
            let id: String = r.get(0)?;
            let source_identifier: String = r.get(1)?;
            let conv_type: String = r.get(2)?;
            let raw_title: Option<String> = r.get(3)?;
            let updated_at_ms: i64 = r.get(4)?;
            let last_preview: Option<String> = r.get(5)?;
            let is_archived_int: i64 = r.get(6)?;
            let message_count: i64 = r.get(7)?;
            let max_msg_ts: Option<i64> = r.get(8)?;

            Ok((
                id,
                source_identifier,
                conv_type,
                raw_title,
                updated_at_ms,
                last_preview,
                is_archived_int != 0,
                message_count as u64,
                max_msg_ts,
            ))
        })?;

        let mut summaries = Vec::new();
        for item in rows {
            let (
                id,
                source_identifier,
                conv_type,
                raw_title,
                updated_at_ms,
                last_preview,
                is_archived,
                msg_count,
                max_msg_ts,
            ) = item?;

            let participants = fetch_participants_for_conversation(conn, &id)?;
            let title = derive_conversation_title(raw_title.as_deref(), &participants, &conv_type);
            let last_timestamp = max_msg_ts.or(Some(updated_at_ms));

            summaries.push(ConversationSummary {
                id,
                source_identifier,
                conversation_type: conv_type,
                title,
                participants,
                last_message_preview: last_preview,
                last_message_timestamp_ms: last_timestamp,
                message_count: msg_count,
                is_archived,
            });
        }

        Ok((summaries, total_count))
    }

    /// Fetches statistics and metadata for a single conversation
    pub fn get_conversation_stats(
        &self,
        conversation_id: &str,
    ) -> Result<ConversationStats, DbError> {
        let conn = self.conn();

        let (conv_type, raw_title): (String, Option<String>) = conn.query_row(
            "SELECT conversation_type, title FROM conversations WHERE id = ?1",
            params![conversation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let participants = fetch_participants_for_conversation(conn, conversation_id)?;
        let title = derive_conversation_title(raw_title.as_deref(), &participants, &conv_type);

        let total_messages: u64 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE conversation_id = ?1",
            params![conversation_id],
            |r| r.get::<_, i64>(0).map(|c| c as u64),
        )?;

        let total_audio: u64 = conn.query_row(
            "SELECT COUNT(*) FROM messages m
             JOIN attachments a ON a.message_id = m.id
             WHERE m.conversation_id = ?1 AND UPPER(a.attachment_type) = 'AUDIO'",
            params![conversation_id],
            |r| r.get::<_, i64>(0).map(|c| c as u64),
        )?;

        let total_images: u64 = conn.query_row(
            "SELECT COUNT(*) FROM messages m
             JOIN attachments a ON a.message_id = m.id
             WHERE m.conversation_id = ?1 AND UPPER(a.attachment_type) = 'IMAGE'",
            params![conversation_id],
            |r| r.get::<_, i64>(0).map(|c| c as u64),
        )?;

        let total_videos: u64 = conn.query_row(
            "SELECT COUNT(*) FROM messages m
             JOIN attachments a ON a.message_id = m.id
             WHERE m.conversation_id = ?1 AND UPPER(a.attachment_type) = 'VIDEO'",
            params![conversation_id],
            |r| r.get::<_, i64>(0).map(|c| c as u64),
        )?;

        let total_links: u64 = conn.query_row(
            "SELECT COUNT(*) FROM messages m
             WHERE m.conversation_id = ?1 AND (m.message_type = 'SHARE' OR m.content LIKE '%http%')",
            params![conversation_id],
            |r| r.get::<_, i64>(0).map(|c| c as u64),
        )?;

        let total_calls: u64 = conn.query_row(
            "SELECT COUNT(*) FROM call_events WHERE conversation_id = ?1",
            params![conversation_id],
            |r| r.get::<_, i64>(0).map(|c| c as u64),
        )?;

        let (first_ts, last_ts): (Option<i64>, Option<i64>) = conn.query_row(
            "SELECT MIN(timestamp_ms), MAX(timestamp_ms) FROM messages WHERE conversation_id = ?1",
            params![conversation_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        Ok(ConversationStats {
            id: conversation_id.to_string(),
            title,
            conversation_type: conv_type,
            participants,
            total_messages,
            total_audio,
            total_images,
            total_videos,
            total_links,
            total_calls,
            first_message_timestamp_ms: first_ts,
            last_message_timestamp_ms: last_ts,
        })
    }

    /// Fetches paginated messages for a conversation, ordered chronologically.
    /// Offset=0 retrieves the latest messages. Increasing offset pages backwards into history.
    pub fn get_messages(
        &self,
        conversation_id: &str,
        limit: u32,
        offset: u32,
        filter: Option<&str>,
        media_root: &Path,
    ) -> Result<PaginatedMessages, DbError> {
        let conn = self.conn();
        let limit = limit.clamp(1, 100);

        let mut where_clause = "m.conversation_id = ?1".to_string();
        if let Some(f) = filter {
            match f.to_ascii_lowercase().as_str() {
                "audio" => {
                    where_clause.push_str(" AND EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = m.id AND UPPER(a.attachment_type) = 'AUDIO')");
                }
                "media" | "images" | "videos" => {
                    where_clause.push_str(" AND EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = m.id AND UPPER(a.attachment_type) IN ('IMAGE', 'VIDEO'))");
                }
                "links" => {
                    where_clause
                        .push_str(" AND (m.message_type = 'SHARE' OR m.content LIKE '%http%')");
                }
                "calls" => {
                    where_clause.push_str(" AND (m.message_type = 'CALL' OR EXISTS (SELECT 1 FROM call_events ce WHERE ce.message_id = m.id))");
                }
                _ => {}
            }
        }

        // Count total matching messages
        let count_query = format!("SELECT COUNT(*) FROM messages m WHERE {}", where_clause);
        let total_count: u64 = conn.query_row(&count_query, params![conversation_id], |r| {
            r.get::<_, i64>(0).map(|c| c as u64)
        })?;

        // Query paginated messages
        // Inner query selects latest N with DESC; outer query restores chronological ASC order
        let sql = format!(
            "SELECT sub.id, sub.conversation_id, sub.sender_id, sub.source_message_id,
                    sub.message_type, sub.content, sub.timestamp_ms, sub.is_outgoing,
                    sub.reply_to_message_id, sub.raw_payload_json,
                    sub.sender_username, sub.sender_full_name
             FROM (
                 SELECT m.id, m.conversation_id, m.sender_id, m.source_message_id,
                        m.message_type, m.content, m.timestamp_ms, m.is_outgoing,
                        m.reply_to_message_id, m.raw_payload_json,
                        p.username AS sender_username, p.full_name AS sender_full_name,
                        m.source_array_index, m.rowid
                 FROM messages m
                 JOIN participants p ON p.id = m.sender_id
                 WHERE {}
                 ORDER BY m.timestamp_ms DESC, m.source_array_index DESC, m.rowid DESC
                 LIMIT ?2 OFFSET ?3
             ) sub
             ORDER BY sub.timestamp_ms ASC, sub.source_array_index ASC, sub.rowid ASC",
            where_clause
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![conversation_id, limit, offset], |r| {
            let id: String = r.get(0)?;
            let conv_id: String = r.get(1)?;
            let sender_id: String = r.get(2)?;
            let source_mid: Option<String> = r.get(3)?;
            let msg_type: String = r.get(4)?;
            let content: Option<String> = r.get(5)?;
            let ts_ms: i64 = r.get(6)?;
            let is_outgoing_int: i64 = r.get(7)?;
            let reply_to_mid: Option<String> = r.get(8)?;
            let raw_json: Option<String> = r.get(9)?;
            let sender_username: String = r.get(10)?;
            let sender_full_name: Option<String> = r.get(11)?;

            Ok((
                id,
                conv_id,
                sender_id,
                source_mid,
                msg_type,
                content,
                ts_ms,
                is_outgoing_int != 0,
                reply_to_mid,
                raw_json,
                sender_username,
                sender_full_name,
            ))
        })?;

        let mut raw_messages = Vec::new();
        for r in rows {
            raw_messages.push(r?);
        }

        if raw_messages.is_empty() {
            return Ok(PaginatedMessages {
                conversation_id: conversation_id.to_string(),
                total_count,
                offset,
                limit,
                messages: Vec::new(),
            });
        }

        // Hydrate attachments, reactions, read receipts, calls, and reply previews in batch
        let message_ids: Vec<String> = raw_messages.iter().map(|m| m.0.clone()).collect();
        let hydrated_messages =
            self.hydrate_message_views(raw_messages, &message_ids, media_root)?;

        Ok(PaginatedMessages {
            conversation_id: conversation_id.to_string(),
            total_count,
            offset,
            limit,
            messages: hydrated_messages,
        })
    }

    /// Searches messages across conversations or inside a specific conversation
    pub fn search_messages(
        &self,
        query: &str,
        conversation_id: Option<&str>,
        limit: u32,
        offset: u32,
        media_root: &Path,
    ) -> Result<SearchResults, DbError> {
        let conn = self.conn();
        let trimmed_query = query.trim();
        if trimmed_query.is_empty() {
            return Ok(SearchResults {
                query: query.to_string(),
                total_count: 0,
                results: Vec::new(),
            });
        }

        let limit = limit.clamp(1, 100);
        let pattern = format!("%{}%", trimmed_query);

        let (count_sql, select_sql) = if let Some(_cid) = conversation_id {
            (
                "SELECT COUNT(*) FROM messages m WHERE m.conversation_id = ?1 AND (m.content LIKE ?2 OR m.raw_payload_json LIKE ?2)".to_string(),
                format!(
                    "SELECT m.id, m.conversation_id, m.sender_id, m.source_message_id,
                            m.message_type, m.content, m.timestamp_ms, m.is_outgoing,
                            m.reply_to_message_id, m.raw_payload_json,
                            p.username AS sender_username, p.full_name AS sender_full_name,
                            c.title AS conv_title, c.conversation_type
                     FROM messages m
                     JOIN participants p ON p.id = m.sender_id
                     JOIN conversations c ON c.id = m.conversation_id
                     WHERE m.conversation_id = ?1 AND (m.content LIKE ?2 OR m.raw_payload_json LIKE ?2)
                     ORDER BY m.timestamp_ms DESC
                     LIMIT ?3 OFFSET ?4"
                ),
            )
        } else {
            (
                "SELECT COUNT(*) FROM messages m WHERE (m.content LIKE ?1 OR m.raw_payload_json LIKE ?1)".to_string(),
                format!(
                    "SELECT m.id, m.conversation_id, m.sender_id, m.source_message_id,
                            m.message_type, m.content, m.timestamp_ms, m.is_outgoing,
                            m.reply_to_message_id, m.raw_payload_json,
                            p.username AS sender_username, p.full_name AS sender_full_name,
                            c.title AS conv_title, c.conversation_type
                     FROM messages m
                     JOIN participants p ON p.id = m.sender_id
                     JOIN conversations c ON c.id = m.conversation_id
                     WHERE (m.content LIKE ?1 OR m.raw_payload_json LIKE ?1)
                     ORDER BY m.timestamp_ms DESC
                     LIMIT ?2 OFFSET ?3"
                ),
            )
        };

        let total_count: u64 = if let Some(cid) = conversation_id {
            conn.query_row(&count_sql, params![cid, pattern], |r| {
                r.get::<_, i64>(0).map(|c| c as u64)
            })?
        } else {
            conn.query_row(&count_sql, params![pattern], |r| {
                r.get::<_, i64>(0).map(|c| c as u64)
            })?
        };

        let mut raw_messages = Vec::new();
        let mut conv_titles_map: HashMap<String, String> = HashMap::new();

        if let Some(cid) = conversation_id {
            let mut stmt = conn.prepare(&select_sql)?;
            let rows = stmt.query_map(params![cid, pattern, limit, offset], |r| {
                let id: String = r.get(0)?;
                let conv_id: String = r.get(1)?;
                let sender_id: String = r.get(2)?;
                let source_mid: Option<String> = r.get(3)?;
                let msg_type: String = r.get(4)?;
                let content: Option<String> = r.get(5)?;
                let ts_ms: i64 = r.get(6)?;
                let is_outgoing_int: i64 = r.get(7)?;
                let reply_to_mid: Option<String> = r.get(8)?;
                let raw_json: Option<String> = r.get(9)?;
                let sender_username: String = r.get(10)?;
                let sender_full_name: Option<String> = r.get(11)?;
                let raw_conv_title: Option<String> = r.get(12)?;
                let conv_type: String = r.get(13)?;

                Ok((
                    id,
                    conv_id,
                    sender_id,
                    source_mid,
                    msg_type,
                    content,
                    ts_ms,
                    is_outgoing_int != 0,
                    reply_to_mid,
                    raw_json,
                    sender_username,
                    sender_full_name,
                    raw_conv_title,
                    conv_type,
                ))
            })?;

            for r in rows {
                let (
                    id,
                    conv_id,
                    sender_id,
                    source_mid,
                    msg_type,
                    content,
                    ts_ms,
                    is_outgoing,
                    reply_to_mid,
                    raw_json,
                    sender_username,
                    sender_full_name,
                    raw_conv_title,
                    conv_type,
                ) = r?;

                let participants = fetch_participants_for_conversation(conn, &conv_id)?;
                let conv_title =
                    derive_conversation_title(raw_conv_title.as_deref(), &participants, &conv_type);
                conv_titles_map.insert(id.clone(), conv_title);

                raw_messages.push((
                    id,
                    conv_id,
                    sender_id,
                    source_mid,
                    msg_type,
                    content,
                    ts_ms,
                    is_outgoing,
                    reply_to_mid,
                    raw_json,
                    sender_username,
                    sender_full_name,
                ));
            }
        } else {
            let mut stmt = conn.prepare(&select_sql)?;
            let rows = stmt.query_map(params![pattern, limit, offset], |r| {
                let id: String = r.get(0)?;
                let conv_id: String = r.get(1)?;
                let sender_id: String = r.get(2)?;
                let source_mid: Option<String> = r.get(3)?;
                let msg_type: String = r.get(4)?;
                let content: Option<String> = r.get(5)?;
                let ts_ms: i64 = r.get(6)?;
                let is_outgoing_int: i64 = r.get(7)?;
                let reply_to_mid: Option<String> = r.get(8)?;
                let raw_json: Option<String> = r.get(9)?;
                let sender_username: String = r.get(10)?;
                let sender_full_name: Option<String> = r.get(11)?;
                let raw_conv_title: Option<String> = r.get(12)?;
                let conv_type: String = r.get(13)?;

                Ok((
                    id,
                    conv_id,
                    sender_id,
                    source_mid,
                    msg_type,
                    content,
                    ts_ms,
                    is_outgoing_int != 0,
                    reply_to_mid,
                    raw_json,
                    sender_username,
                    sender_full_name,
                    raw_conv_title,
                    conv_type,
                ))
            })?;

            for r in rows {
                let (
                    id,
                    conv_id,
                    sender_id,
                    source_mid,
                    msg_type,
                    content,
                    ts_ms,
                    is_outgoing,
                    reply_to_mid,
                    raw_json,
                    sender_username,
                    sender_full_name,
                    raw_conv_title,
                    conv_type,
                ) = r?;

                let participants = fetch_participants_for_conversation(conn, &conv_id)?;
                let conv_title =
                    derive_conversation_title(raw_conv_title.as_deref(), &participants, &conv_type);
                conv_titles_map.insert(id.clone(), conv_title);

                raw_messages.push((
                    id,
                    conv_id,
                    sender_id,
                    source_mid,
                    msg_type,
                    content,
                    ts_ms,
                    is_outgoing,
                    reply_to_mid,
                    raw_json,
                    sender_username,
                    sender_full_name,
                ));
            }
        }

        let message_ids: Vec<String> = raw_messages.iter().map(|m| m.0.clone()).collect();
        let hydrated_messages =
            self.hydrate_message_views(raw_messages, &message_ids, media_root)?;

        let mut results = Vec::new();
        for msg in hydrated_messages {
            let title = conv_titles_map
                .get(&msg.id)
                .cloned()
                .unwrap_or_else(|| "Conversation".to_string());
            results.push(SearchResultItem {
                message: msg,
                conversation_title: title,
            });
        }

        Ok(SearchResults {
            query: query.to_string(),
            total_count,
            results,
        })
    }

    /// Internal batch hydration helper for attachments, reactions, read receipts, and calls
    fn hydrate_message_views(
        &self,
        raw_messages: Vec<(
            String,
            String,
            String,
            Option<String>,
            String,
            Option<String>,
            i64,
            bool,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
        )>,
        message_ids: &[String],
        media_root: &Path,
    ) -> Result<Vec<MessageView>, DbError> {
        let conn = self.conn();
        if message_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Hydrate attachments
        let mut attachments_by_msg: HashMap<String, Vec<AttachmentView>> = HashMap::new();
        {
            let placeholders = message_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT id, message_id, attachment_type, original_uri, storage_path,
                        mime_type, file_size_bytes, sha256_checksum, duration_ms, status
                 FROM attachments
                 WHERE message_id IN ({})
                 ORDER BY id ASC",
                placeholders
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(message_ids.iter()), |r| {
                let id: String = r.get(0)?;
                let msg_id: String = r.get(1)?;
                let attach_type: String = r.get(2)?;
                let original_uri: Option<String> = r.get(3)?;
                let storage_path: Option<String> = r.get(4)?;
                let mime_type: String = r.get(5)?;
                let file_size: Option<i64> = r.get(6)?;
                let checksum: Option<String> = r.get(7)?;
                let duration: Option<i64> = r.get(8)?;
                let status: String = r.get(9)?;

                Ok((
                    id,
                    msg_id,
                    attach_type,
                    original_uri,
                    storage_path,
                    mime_type,
                    file_size,
                    checksum,
                    duration,
                    status,
                ))
            })?;

            for r in rows {
                let (
                    id,
                    msg_id,
                    attach_type,
                    original_uri,
                    storage_path,
                    mime_type,
                    file_size,
                    checksum,
                    duration,
                    status,
                ) = r?;

                // Check physical file availability on disk
                let (is_available, media_url) = if let Some(ref rel_path) = storage_path {
                    let full_path = media_root.join(rel_path);
                    if full_path.exists() && full_path.is_file() {
                        let normalized_path = rel_path.replace('\\', "/");
                        (true, Some(format!("media://localhost/{}", normalized_path)))
                    } else {
                        (false, None)
                    }
                } else {
                    (false, None)
                };

                let view = AttachmentView {
                    id,
                    attachment_type: attach_type.to_lowercase(),
                    original_uri,
                    storage_path,
                    media_url,
                    mime_type,
                    file_size_bytes: file_size,
                    sha256_checksum: checksum,
                    duration_ms: duration,
                    status,
                    is_available,
                };
                attachments_by_msg.entry(msg_id).or_default().push(view);
            }
        }

        // Hydrate reactions
        let mut reactions_by_msg: HashMap<String, Vec<ReactionView>> = HashMap::new();
        {
            let placeholders = message_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT r.message_id, r.participant_id, r.reaction_value, p.username
                 FROM reactions r
                 JOIN participants p ON p.id = r.participant_id
                 WHERE r.message_id IN ({})
                 ORDER BY r.created_at_ms ASC",
                placeholders
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(message_ids.iter()), |r| {
                let msg_id: String = r.get(0)?;
                let participant_id: String = r.get(1)?;
                let reaction_val: String = r.get(2)?;
                let username: String = r.get(3)?;
                Ok((
                    msg_id,
                    ReactionView {
                        reaction_value: reaction_val,
                        participant_id,
                        username,
                    },
                ))
            })?;

            for r in rows {
                let (msg_id, rx) = r?;
                reactions_by_msg.entry(msg_id).or_default().push(rx);
            }
        }

        // Hydrate read receipts
        let mut read_receipts_by_msg: HashMap<String, Vec<ReadReceiptView>> = HashMap::new();
        {
            let placeholders = message_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT rr.message_id, rr.participant_id, rr.read_status, rr.read_at_ms, p.username
                 FROM read_receipts rr
                 JOIN participants p ON p.id = rr.participant_id
                 WHERE rr.message_id IN ({})",
                placeholders
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(message_ids.iter()), |r| {
                let msg_id: String = r.get(0)?;
                let participant_id: String = r.get(1)?;
                let read_status: String = r.get(2)?;
                let read_at: Option<i64> = r.get(3)?;
                let username: String = r.get(4)?;
                Ok((
                    msg_id,
                    ReadReceiptView {
                        participant_id,
                        username,
                        read_status,
                        read_at_ms: read_at,
                    },
                ))
            })?;

            for r in rows {
                let (msg_id, rr) = r?;
                read_receipts_by_msg.entry(msg_id).or_default().push(rr);
            }
        }

        // Hydrate call events
        let mut calls_by_msg: HashMap<String, CallEventView> = HashMap::new();
        {
            let placeholders = message_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT ce.id, ce.message_id, ce.call_type, ce.direction, ce.status,
                        ce.start_timestamp_ms, ce.duration_seconds, ce.derived_end_timestamp_ms
                 FROM call_events ce
                 WHERE ce.message_id IN ({})",
                placeholders
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(message_ids.iter()), |r| {
                let id: String = r.get(0)?;
                let msg_id: Option<String> = r.get(1)?;
                let call_type: String = r.get(2)?;
                let direction: String = r.get(3)?;
                let status: String = r.get(4)?;
                let start_ts: i64 = r.get(5)?;
                let duration: Option<i64> = r.get(6)?;
                let derived_end: Option<i64> = r.get(7)?;

                Ok((
                    msg_id,
                    CallEventView {
                        id,
                        call_type,
                        direction,
                        status,
                        start_timestamp_ms: start_ts,
                        duration_seconds: duration,
                        derived_end_timestamp_ms: derived_end,
                    },
                ))
            })?;

            for r in rows {
                let (msg_id, call) = r?;
                if let Some(mid) = msg_id {
                    calls_by_msg.insert(mid, call);
                }
            }
        }

        // Reply previews cache
        let mut reply_previews: HashMap<String, ReplyPreview> = HashMap::new();

        let mut hydrated = Vec::new();
        for (
            id,
            conv_id,
            sender_id,
            source_mid,
            msg_type,
            content,
            ts_ms,
            is_outgoing,
            reply_to_mid,
            raw_json,
            sender_username,
            sender_full_name,
        ) in raw_messages
        {
            // Hydrate reply preview
            let reply_preview = if let Some(ref parent_id) = reply_to_mid {
                if let Some(cached) = reply_previews.get(parent_id) {
                    Some(cached.clone())
                } else {
                    let parent_info: Result<(String, Option<String>), rusqlite::Error> = conn
                        .query_row(
                            "SELECT p.username, m.content
                         FROM messages m
                         JOIN participants p ON p.id = m.sender_id
                         WHERE m.id = ?1",
                            params![parent_id],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        );

                    let preview = if let Ok((parent_user, parent_content)) = parent_info {
                        let snippet = parent_content.map(|c| {
                            if c.chars().count() > 60 {
                                format!("{}...", c.chars().take(57).collect::<String>())
                            } else {
                                c
                            }
                        });
                        ReplyPreview {
                            parent_message_id: Some(parent_id.clone()),
                            parent_sender_username: Some(parent_user),
                            snippet,
                        }
                    } else {
                        ReplyPreview {
                            parent_message_id: Some(parent_id.clone()),
                            parent_sender_username: None,
                            snippet: Some("Original message unavailable".to_string()),
                        }
                    };
                    reply_previews.insert(parent_id.clone(), preview.clone());
                    Some(preview)
                }
            } else {
                // If reply_to_mid is None, check raw_json for inline quoted text from exports
                raw_json.as_deref().and_then(|json_str| {
                    serde_json::from_str::<serde_json::Value>(json_str)
                        .ok()
                        .and_then(|v| {
                            v.get("reply_to_text")
                                .and_then(|t| t.as_str())
                                .map(|quoted| {
                                    ReplyPreview {
                                        parent_message_id: None, // No deterministic jump available
                                        parent_sender_username: None,
                                        snippet: Some(quoted.to_string()),
                                    }
                                })
                        })
                })
            };

            // Parse share link and text if message is a share
            let (share_link, share_text) = if let Some(ref json_str) = raw_json {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                    let link = val
                        .get("share")
                        .and_then(|s| s.get("link"))
                        .and_then(|l| l.as_str())
                        .and_then(validate_external_url);
                    let text = val
                        .get("share")
                        .and_then(|s| s.get("share_text"))
                        .and_then(|t| t.as_str())
                        .map(|s| s.to_string());
                    (link, text)
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };

            let attachments = attachments_by_msg.remove(&id).unwrap_or_default();
            let reactions = reactions_by_msg.remove(&id).unwrap_or_default();
            let read_receipts = read_receipts_by_msg.remove(&id).unwrap_or_default();
            let call_event = calls_by_msg.remove(&id);

            hydrated.push(MessageView {
                id,
                conversation_id: conv_id,
                sender_id,
                sender_username,
                sender_full_name,
                is_outgoing,
                source_message_id: source_mid,
                message_type: msg_type,
                content,
                timestamp_ms: ts_ms,
                share_link,
                share_text,
                reply_to_message_id: reply_to_mid,
                reply_preview,
                attachments,
                reactions,
                read_receipts,
                call_event,
            });
        }

        Ok(hydrated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use uuid::Uuid;

    fn create_test_populated_db() -> (Database, String, String, String) {
        let mut db = Database::open_in_memory().unwrap();
        let conn = db.conn_mut();

        let conv_id = Uuid::new_v4().to_string();
        let p_self_id = Uuid::new_v4().to_string();
        let p_other_id = Uuid::new_v4().to_string();

        // Insert conversation
        conn.execute(
            "INSERT INTO conversations (id, source_identifier, conversation_type, title, created_at_ms, updated_at_ms)
             VALUES (?1, 'conv_alice_123', 'DIRECT', 'Alice Chat', 1000, 5000)",
            params![conv_id],
        ).unwrap();

        // Insert participants
        conn.execute(
            "INSERT INTO participants (id, source_identifier, username, full_name, is_self)
             VALUES (?1, 'me', 'my_account', 'My Full Name', 1)",
            params![p_self_id],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO participants (id, source_identifier, username, full_name, is_self)
             VALUES (?1, 'alice', 'alice_w', 'Alice Wonderland', 0)",
            params![p_other_id],
        )
        .unwrap();

        // Link participants
        conn.execute(
            "INSERT INTO conversation_participants (conversation_id, participant_id) VALUES (?1, ?2), (?1, ?3)",
            params![conv_id, p_self_id, p_other_id],
        ).unwrap();

        // Insert messages:
        // Msg 1: text from alice
        let m1_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO messages (id, conversation_id, sender_id, source_message_id, identity_hash, message_type, content, timestamp_ms, is_outgoing)
             VALUES (?1, ?2, ?3, 'mid_1', 'hash_1', 'TEXT', 'Hello from Alice!', 1000, 0)",
            params![m1_id, conv_id, p_other_id],
        ).unwrap();

        // Msg 2: voice note from me with attachment
        let m2_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO messages (id, conversation_id, sender_id, source_message_id, identity_hash, message_type, content, timestamp_ms, is_outgoing)
             VALUES (?1, ?2, ?3, 'mid_2', 'hash_2', 'VOICE', NULL, 2000, 1)",
            params![m2_id, conv_id, p_self_id],
        ).unwrap();

        let att_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO attachments (id, message_id, attachment_type, original_uri, storage_path, mime_type, file_size_bytes, duration_ms, status)
             VALUES (?1, ?2, 'audio', 'audio/note.mp4', 'conv_alice_123/audio/note.mp4', 'audio/mp4', 12345, 5200, 'LOCAL_COPIED')",
            params![att_id, m2_id],
        ).unwrap();

        // Reaction on Msg 1
        let rx_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO reactions (id, message_id, participant_id, reaction_value, created_at_ms)
             VALUES (?1, ?2, ?3, '❤️', 1500)",
            params![rx_id, m1_id, p_self_id],
        )
        .unwrap();

        // Read receipt on Msg 2
        let rr_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO read_receipts (id, message_id, participant_id, read_status, read_at_ms)
             VALUES (?1, ?2, ?3, 'UNAVAILABLE_FROM_SOURCE', NULL)",
            params![rr_id, m2_id, p_other_id],
        )
        .unwrap();

        // Msg 3: Call event
        let m3_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO messages (id, conversation_id, sender_id, source_message_id, identity_hash, message_type, content, timestamp_ms, is_outgoing)
             VALUES (?1, ?2, ?3, 'mid_3', 'hash_3', 'CALL', NULL, 3000, 0)",
            params![m3_id, conv_id, p_other_id],
        ).unwrap();

        let call_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO call_events (id, conversation_id, message_id, caller_id, call_type, direction, status, start_timestamp_ms, duration_seconds, derived_end_timestamp_ms)
             VALUES (?1, ?2, ?3, ?4, 'VOICE', 'INCOMING', 'ANSWERED', 3000, 120, 3120000)",
            params![call_id, conv_id, m3_id, p_other_id],
        ).unwrap();

        (db, conv_id, p_self_id, p_other_id)
    }

    #[test]
    fn test_list_conversations() {
        let (db, conv_id, _, _) = create_test_populated_db();
        let (list, count) = db.list_conversations(None, None, 10, 0).unwrap();
        assert_eq!(count, 1);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, conv_id);
        assert_eq!(list[0].title, "Alice Chat");
        assert_eq!(list[0].message_count, 3);
        assert_eq!(list[0].participants.len(), 2);
    }

    #[test]
    fn test_derive_conversation_title_fallback() {
        let p_self = ParticipantSummary {
            id: "1".into(),
            username: "me".into(),
            full_name: Some("My Name".into()),
            is_self: true,
        };
        let p_other = ParticipantSummary {
            id: "2".into(),
            username: "bob".into(),
            full_name: Some("Bob Jones".into()),
            is_self: false,
        };

        // Fallback to other participant's full name
        assert_eq!(
            derive_conversation_title(None, &[p_self.clone(), p_other.clone()], "DIRECT"),
            "Bob Jones"
        );
        // Explicit title preserved
        assert_eq!(
            derive_conversation_title(
                Some("Project Chat"),
                &[p_self.clone(), p_other.clone()],
                "GROUP"
            ),
            "Project Chat"
        );
    }

    #[test]
    fn test_get_messages_chronological_ordering() {
        let (db, conv_id, _, _) = create_test_populated_db();
        let dummy_media_root = Path::new("C:/non_existent_media_dir");
        let paginated = db
            .get_messages(&conv_id, 10, 0, None, dummy_media_root)
            .unwrap();

        assert_eq!(paginated.total_count, 3);
        assert_eq!(paginated.messages.len(), 3);

        // Strict chronological ordering
        assert_eq!(paginated.messages[0].message_type, "TEXT");
        assert_eq!(paginated.messages[0].timestamp_ms, 1000);
        assert_eq!(paginated.messages[0].reactions.len(), 1);
        assert_eq!(paginated.messages[0].reactions[0].reaction_value, "❤️");

        assert_eq!(paginated.messages[1].message_type, "VOICE");
        assert_eq!(paginated.messages[1].timestamp_ms, 2000);
        assert_eq!(paginated.messages[1].attachments.len(), 1);
        assert_eq!(
            paginated.messages[1].attachments[0].attachment_type,
            "audio"
        );
        assert_eq!(paginated.messages[1].read_receipts.len(), 1);
        assert_eq!(
            paginated.messages[1].read_receipts[0].read_status,
            "UNAVAILABLE_FROM_SOURCE"
        );

        assert_eq!(paginated.messages[2].message_type, "CALL");
        assert!(paginated.messages[2].call_event.is_some());
        let call = paginated.messages[2].call_event.as_ref().unwrap();
        assert_eq!(call.call_type, "VOICE");
        assert_eq!(call.direction, "INCOMING");
        assert_eq!(call.duration_seconds, Some(120));
    }

    #[test]
    fn test_search_messages() {
        let (db, conv_id, _, _) = create_test_populated_db();
        let dummy_media_root = Path::new("C:/non_existent_media_dir");

        let results = db
            .search_messages("Alice", None, 10, 0, dummy_media_root)
            .unwrap();
        assert_eq!(results.total_count, 1);
        assert_eq!(results.results.len(), 1);
        assert_eq!(results.results[0].message.conversation_id, conv_id);
        assert_eq!(
            results.results[0].message.content.as_deref(),
            Some("Hello from Alice!")
        );

        let no_match = db
            .search_messages("NonExistentTextXYZ", None, 10, 0, dummy_media_root)
            .unwrap();
        assert_eq!(no_match.total_count, 0);
        assert!(no_match.results.is_empty());
    }

    #[test]
    fn test_conversation_stats() {
        let (db, conv_id, _, _) = create_test_populated_db();
        let stats = db.get_conversation_stats(&conv_id).unwrap();

        assert_eq!(stats.id, conv_id);
        assert_eq!(stats.total_messages, 3);
        assert_eq!(stats.total_audio, 1);
        assert_eq!(stats.total_calls, 1);
        assert_eq!(stats.first_message_timestamp_ms, Some(1000));
        assert_eq!(stats.last_message_timestamp_ms, Some(3000));
    }

    #[test]
    fn test_external_url_validation() {
        assert_eq!(
            validate_external_url("https://instagram.com/reel/123"),
            Some("https://instagram.com/reel/123".into())
        );
        assert_eq!(
            validate_external_url("http://example.com"),
            Some("http://example.com".into())
        );
        assert_eq!(validate_external_url("javascript:alert(1)"), None);
        assert_eq!(validate_external_url("file:///etc/passwd"), None);
        assert_eq!(validate_external_url("data:text/html,malicious"), None);
    }
}
