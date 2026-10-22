// TypeScript interfaces matching Rust IPC types exactly
// Governed by PRD.md, TRD.md Section 5, and GEMINI.md

export interface AppStatus {
  app_name: string;
  version: string;
  architecture: string;
  target_os: string;
  local_first_mode: boolean;
  phase: string;
  database_ready: boolean;
}

export interface ParticipantSummary {
  id: string;
  username: string;
  full_name: string | null;
  is_self: boolean;
}

export interface ConversationSummary {
  id: string;
  source_identifier: string;
  conversation_type: string;
  title: string;
  participants: ParticipantSummary[];
  last_message_preview: string | null;
  last_message_timestamp_ms: number | null;
  message_count: number;
  is_archived: boolean;
}

export interface ConversationListResponse {
  total_count: number;
  conversations: ConversationSummary[];
}

export interface ConversationStats {
  id: string;
  title: string;
  conversation_type: string;
  participants: ParticipantSummary[];
  total_messages: number;
  total_audio: number;
  total_images: number;
  total_videos: number;
  total_links: number;
  total_calls: number;
  first_message_timestamp_ms: number | null;
  last_message_timestamp_ms: number | null;
}

export interface AttachmentView {
  id: string;
  attachment_type: string; // 'audio', 'image', 'video', 'file'
  original_uri: string | null;
  storage_path: string | null;
  media_url: string | null; // 'media://localhost/...'
  mime_type: string;
  file_size_bytes: number | null;
  sha256_checksum: string | null;
  duration_ms: number | null;
  status: string; // 'LOCAL_COPIED' or 'MISSING'
  is_available: boolean;
}

export interface ReactionView {
  reaction_value: string;
  participant_id: string;
  username: string;
}

export interface ReadReceiptView {
  participant_id: string;
  username: string;
  read_status: string; // 'READ' or 'UNAVAILABLE_FROM_SOURCE'
  read_at_ms: number | null;
}

export interface CallEventView {
  id: string;
  call_type: string; // 'VOICE' or 'VIDEO'
  direction: string; // 'INCOMING' or 'OUTGOING'
  status: string; // 'ANSWERED', 'MISSED', etc.
  start_timestamp_ms: number;
  duration_seconds: number | null;
  derived_end_timestamp_ms: number | null;
}

export interface ReplyPreview {
  parent_message_id: string | null;
  parent_sender_username: string | null;
  snippet: string | null;
}

export interface MessageView {
  id: string;
  conversation_id: string;
  sender_id: string;
  sender_username: string;
  sender_full_name: string | null;
  is_outgoing: boolean;
  source_message_id: string | null;
  message_type: string; // 'TEXT', 'VOICE', 'IMAGE', 'VIDEO', 'SHARE', 'CALL', 'UNKNOWN'
  content: string | null;
  timestamp_ms: number;
  share_link: string | null;
  share_text: string | null;
  reply_to_message_id: string | null;
  reply_preview: ReplyPreview | null;
  attachments: AttachmentView[];
  reactions: ReactionView[];
  read_receipts: ReadReceiptView[];
  call_event: CallEventView | null;
}

export interface PaginatedMessages {
  conversation_id: string;
  total_count: number;
  offset: number;
  limit: number;
  messages: MessageView[];
}

export interface SearchResultItem {
  message: MessageView;
  conversation_title: string;
}

export interface SearchResults {
  query: string;
  total_count: number;
  results: SearchResultItem[];
}

export interface DiscoveredConversation {
  source_id: string;
  title: string | null;
  json_files: string[];
  participants: string[];
}

export interface ValidationSummary {
  is_valid: boolean;
  is_empty: boolean;
  source_type: string;
  dataset_fingerprint: string;
  conversations: DiscoveredConversation[];
  total_message_files: number;
  attachment_candidates: number;
  warnings: string[];
  errors: string[];
}

export interface ImportReport {
  job_id: string;
  source_path: string;
  dataset_fingerprint: string;
  started_at_ms: number;
  completed_at_ms: number | null;
  status: string;
  conversations_discovered: number;
  conversations_imported: number;
  messages_processed: number;
  messages_imported: number;
  messages_skipped: number;
  attachments_discovered: number;
  attachments_imported: number;
  attachments_missing: number;
  call_events_imported: number;
  reactions_imported: number;
  warnings: string[];
  errors: string[];
}

export interface AppSettings {
  autostart_enabled: boolean;
  monitor_folder_enabled: boolean;
  monitored_folder_path: string | null;
  last_imported_archive_fingerprint: string | null;
  last_imported_archive_name: string | null;
  last_imported_at_ms: number | null;
  last_import_status: string | null;
  last_import_error: string | null;
}

export type MonitorState =
  | 'Disabled'
  | 'Idle'
  | 'Scanning'
  | 'Importing'
  | { Error: string };

export interface MonitorStatus {
  state: MonitorState;
  monitored_folder: string | null;
  last_checked_at_ms: number | null;
  message: string;
}
