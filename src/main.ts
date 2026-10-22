// Instagram Chat History Viewer - Main Application Controller
// Governed by PRD.md, TRD.md, GEMINI.md, and UI/UX Prompt Skill

import './style.css';
import { invoke } from '@tauri-apps/api/core';
import { openUrl } from '@tauri-apps/plugin-opener';
import { audioManager, formatDuration, PlaybackState } from './audioManager';
import {
  AppStatus,
  ConversationSummary,
  ConversationListResponse,
  ConversationStats,
  MessageView,
  PaginatedMessages,
  SearchResults,
  ValidationSummary,
  ImportReport,
  AttachmentView,
} from './types';

// SVG Icons (Clean Lucide / Instagram Style)
const ICONS = {
  chat: `<svg class="state-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path></svg>`,
  search: `<svg class="search-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg>`,
  clear: `<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>`,
  import: `<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path><polyline points="17 8 12 3 7 8"></polyline><line x1="12" y1="3" x2="12" y2="15"></line></svg>`,
  info: `<svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"></circle><line x1="12" y1="16" x2="12" y2="12"></line><line x1="12" y1="8" x2="12.01" y2="8"></line></svg>`,
  close: `<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>`,
  play: `<svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><polygon points="5 3 19 12 5 21 5 3"></polygon></svg>`,
  pause: `<svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><rect x="6" y="4" width="4" height="16"></rect><rect x="14" y="4" width="4" height="16"></rect></svg>`,
  phone: `<svg class="call-event-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6 19.79 19.79 0 0 1-3.07-8.67A2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72 12.84 12.84 0 0 0 .7 2.81 2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45 12.84 12.84 0 0 0 2.81.7A2 2 0 0 1 22 16.92z"></path></svg>`,
  video: `<svg class="call-event-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polygon points="23 7 16 12 23 17 23 7"></polygon><rect x="1" y="5" width="15" height="14" rx="2" ry="2"></rect></svg>`,
  external: `<svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"></path><polyline points="15 3 21 3 21 9"></polyline><line x1="10" y1="14" x2="21" y2="3"></line></svg>`,
  alert: `<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"></circle><line x1="12" y1="8" x2="12" y2="12"></line><line x1="12" y1="16" x2="12.01" y2="16"></line></svg>`,
  reel: `<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2"><rect x="2" y="2" width="20" height="20" rx="5" ry="5"></rect><path d="M16 11.37A4 4 0 1 1 12.63 8 4 4 0 0 1 16 11.37z"></path><line x1="17.5" y1="6.5" x2="17.51" y2="6.5"></line></svg>`,
};

// Application State
interface State {
  conversations: ConversationSummary[];
  totalConversations: number;
  activeConversationId: string | null;
  activeFilter: string; // 'all', 'audio', 'media', 'links', 'calls'
  searchQuery: string;
  isSearchingMessages: boolean;
  messages: MessageView[];
  totalMessages: number;
  messageOffset: number;
  isLoadingConversations: boolean;
  isLoadingMessages: boolean;
  isInfoDrawerOpen: boolean;
  activeStats: ConversationStats | null;
}

const state: State = {
  conversations: [],
  totalConversations: 0,
  activeConversationId: null,
  activeFilter: 'all',
  searchQuery: '',
  isSearchingMessages: false,
  messages: [],
  totalMessages: 0,
  messageOffset: 0,
  isLoadingConversations: false,
  isLoadingMessages: false,
  isInfoDrawerOpen: false,
  activeStats: null,
};

// Root DOM Element
const app = document.querySelector<HTMLDivElement>('#app')!;

// Initial Render Skeleton
function renderBaseLayout(): void {
  app.innerHTML = `
    <div class="dashboard-layout">
      <!-- Sidebar -->
      <aside class="sidebar">
        <div class="sidebar-header">
          <div class="brand">
            <div class="brand-icon">💬</div>
            <span>Instagram Chat</span>
          </div>
          <button id="import-btn" class="import-trigger-btn" title="Import Instagram Data Archive">
            ${ICONS.import}
            <span>Import</span>
          </button>
        </div>

        <div class="sidebar-search-area">
          <div class="search-input-wrapper">
            ${ICONS.search}
            <input id="search-input" class="search-input" type="text" placeholder="Search chats or messages..." autocomplete="off">
            <button id="search-clear" class="search-clear-btn" style="display: none;">${ICONS.clear}</button>
          </div>

          <div class="search-mode-tabs" style="display: flex; gap: 6px;">
            <button id="mode-chats" class="filter-pill active" style="flex: 1; text-align: center;">Chats</button>
            <button id="mode-messages" class="filter-pill" style="flex: 1; text-align: center;">Message Text</button>
          </div>

          <div class="filter-bar" id="filter-bar">
            <button class="filter-pill active" data-filter="all">All</button>
            <button class="filter-pill" data-filter="audio">Audio</button>
            <button class="filter-pill" data-filter="media">Photos & Videos</button>
            <button class="filter-pill" data-filter="links">Links</button>
            <button class="filter-pill" data-filter="calls">Calls</button>
          </div>
        </div>

        <div id="conversation-list" class="conversation-list">
          <div class="state-container">
            <div class="spinner"></div>
            <p style="margin-top: 10px; font-size: 13px;">Loading conversations...</p>
          </div>
        </div>
      </aside>

      <!-- Main Chat Area -->
      <main id="chat-area" class="chat-area">
        <div class="state-container">
          ${ICONS.chat}
          <div class="state-title">No conversation selected</div>
          <div class="state-desc">Select a chat from the left sidebar to view messages and shared media.</div>
        </div>
      </main>

      <!-- Info Drawer (Right Panel) -->
      <aside id="info-drawer" class="info-drawer" style="display: none;"></aside>
    </div>

    <!-- Modals Container -->
    <div id="modal-container"></div>
    <div id="lightbox-container"></div>
  `;

  bindSidebarEvents();
}

// Bind Sidebar Event Listeners
function bindSidebarEvents(): void {
  const searchInput = document.querySelector<HTMLInputElement>('#search-input')!;
  const searchClear = document.querySelector<HTMLButtonElement>('#search-clear')!;
  const importBtn = document.querySelector<HTMLButtonElement>('#import-btn')!;
  const filterPills = document.querySelectorAll<HTMLButtonElement>('.filter-bar .filter-pill');
  const modeChats = document.querySelector<HTMLButtonElement>('#mode-chats')!;
  const modeMessages = document.querySelector<HTMLButtonElement>('#mode-messages')!;
  const filterBar = document.querySelector<HTMLElement>('#filter-bar')!;

  modeChats.addEventListener('click', () => {
    modeChats.classList.add('active');
    modeMessages.classList.remove('active');
    state.isSearchingMessages = false;
    filterBar.style.display = 'flex';
    loadConversations();
  });

  modeMessages.addEventListener('click', () => {
    modeMessages.classList.add('active');
    modeChats.classList.remove('active');
    state.isSearchingMessages = true;
    filterBar.style.display = 'none';
    loadMessageSearchResults();
  });

  let debounceTimer: number | null = null;
  searchInput.addEventListener('input', () => {
    state.searchQuery = searchInput.value.trim();
    searchClear.style.display = state.searchQuery.length > 0 ? 'flex' : 'none';

    if (debounceTimer !== null) clearTimeout(debounceTimer);
    debounceTimer = window.setTimeout(() => {
      if (state.isSearchingMessages) {
        loadMessageSearchResults();
      } else {
        loadConversations();
      }
    }, 250);
  });

  searchClear.addEventListener('click', () => {
    searchInput.value = '';
    state.searchQuery = '';
    searchClear.style.display = 'none';
    if (state.isSearchingMessages) {
      loadMessageSearchResults();
    } else {
      loadConversations();
    }
  });

  importBtn.addEventListener('click', () => {
    openImportModal();
  });

  filterPills.forEach((pill) => {
    pill.addEventListener('click', () => {
      filterPills.forEach((p) => p.classList.remove('active'));
      pill.classList.add('active');
      state.activeFilter = pill.getAttribute('data-filter') || 'all';
      loadConversations();
      if (state.activeConversationId) {
        state.messageOffset = 0;
        loadMessages(state.activeConversationId, false);
      }
    });
  });
}

// Search Message Text across All Conversations
async function loadMessageSearchResults(): Promise<void> {
  const listContainer = document.querySelector<HTMLDivElement>('#conversation-list')!;
  if (!state.searchQuery) {
    listContainer.innerHTML = `
      <div class="state-container">
        ${ICONS.search}
        <div class="state-title">Search Message Text</div>
        <div class="state-desc">Type in the search bar above to find message content across all chats.</div>
      </div>
    `;
    return;
  }

  listContainer.innerHTML = `
    <div class="state-container">
      <div class="spinner"></div>
      <p style="margin-top: 10px; font-size: 13px;">Searching messages...</p>
    </div>
  `;

  try {
    const res = await invoke<SearchResults>('search_messages', {
      query: state.searchQuery,
      conversation_id: null,
      limit: 50,
      offset: 0,
    });

    if (res.results.length === 0) {
      listContainer.innerHTML = `
        <div class="state-container">
          ${ICONS.search}
          <div class="state-title">No messages found</div>
          <div class="state-desc">No message text matches "${escapeHtml(state.searchQuery)}".</div>
        </div>
      `;
      return;
    }

    let html = `<div style="padding: 6px 12px; font-size: 12px; color: var(--text-muted);">${res.total_count} matching messages found:</div>`;
    for (const item of res.results) {
      const timeStr = formatMessageTime(item.message.timestamp_ms);
      const snippet = escapeHtml(item.message.content || 'Media message');
      html += `
        <div class="conversation-item search-result-item" data-conv-id="${item.message.conversation_id}" data-msg-id="${item.message.id}">
          <div class="avatar" style="width: 36px; height: 36px; font-size: 13px;">💬</div>
          <div class="conv-info">
            <div class="conv-top-row">
              <span class="conv-title">${escapeHtml(item.conversation_title)}</span>
              <span class="conv-time">${timeStr}</span>
            </div>
            <div class="conv-bottom-row">
              <span class="conv-preview" style="max-width: 220px;"><strong>@${escapeHtml(item.message.sender_username)}:</strong> ${snippet}</span>
            </div>
          </div>
        </div>
      `;
    }

    listContainer.innerHTML = html;

    listContainer.querySelectorAll<HTMLDivElement>('.search-result-item').forEach((item) => {
      item.addEventListener('click', async () => {
        const convId = item.getAttribute('data-conv-id');
        const msgId = item.getAttribute('data-msg-id');
        if (convId) {
          state.isSearchingMessages = false;
          document.querySelector('#mode-chats')?.classList.add('active');
          document.querySelector('#mode-messages')?.classList.remove('active');
          const filterBar = document.querySelector<HTMLElement>('#filter-bar');
          if (filterBar) filterBar.style.display = 'flex';

          await selectConversation(convId);

          if (msgId) {
            setTimeout(() => {
              const target = document.querySelector<HTMLElement>(`#msg-${msgId}`);
              if (target) {
                target.scrollIntoView({ behavior: 'smooth', block: 'center' });
                const bubble = target.querySelector<HTMLElement>('.message-bubble');
                if (bubble) {
                  bubble.classList.add('highlight-jump');
                }
              }
            }, 300);
          }
        }
      });
    });
  } catch (err) {
    listContainer.innerHTML = `
      <div class="state-container">
        ${ICONS.alert}
        <div class="state-title">Search Error</div>
        <div class="state-desc">${escapeHtml(String(err))}</div>
      </div>
    `;
  }
}

// Fetch and Render Conversations
async function loadConversations(): Promise<void> {
  const listContainer = document.querySelector<HTMLDivElement>('#conversation-list')!;
  state.isLoadingConversations = true;

  try {
    const res = await invoke<ConversationListResponse>('list_conversations', {
      query: state.searchQuery || null,
      filter: state.activeFilter !== 'all' ? state.activeFilter : null,
      limit: 100,
      offset: 0,
    });

    state.conversations = res.conversations;
    state.totalConversations = res.total_count;

    if (state.conversations.length === 0) {
      if (state.searchQuery) {
        listContainer.innerHTML = `
          <div class="state-container">
            ${ICONS.search}
            <div class="state-title">No results found</div>
            <div class="state-desc">No chats match "${escapeHtml(state.searchQuery)}".</div>
          </div>
        `;
      } else {
        listContainer.innerHTML = `
          <div class="state-container">
            ${ICONS.chat}
            <div class="state-title">No conversations yet</div>
            <div class="state-desc">Import your Instagram data archive (.zip or folder) to view your chats.</div>
            <button id="empty-import-btn" class="btn-primary" style="margin-top: 10px;">
              ${ICONS.import} Import Archive
            </button>
          </div>
        `;
        document.querySelector('#empty-import-btn')?.addEventListener('click', openImportModal);
      }
      return;
    }

    let html = '';
    for (const conv of state.conversations) {
      const isActive = conv.id === state.activeConversationId ? 'active' : '';
      const initial = conv.title.charAt(0).toUpperCase() || '💬';
      const timeStr = conv.last_message_timestamp_ms ? formatTimeAgo(conv.last_message_timestamp_ms) : '';
      const previewText = conv.last_message_preview || 'No messages yet';

      html += `
        <div class="conversation-item ${isActive}" data-id="${conv.id}">
          <div class="avatar">${initial}</div>
          <div class="conv-info">
            <div class="conv-top-row">
              <span class="conv-title">${escapeHtml(conv.title)}</span>
              <span class="conv-time">${timeStr}</span>
            </div>
            <div class="conv-bottom-row">
              <span class="conv-preview">${escapeHtml(previewText)}</span>
              <span class="conv-count-badge">${conv.message_count}</span>
            </div>
          </div>
        </div>
      `;
    }

    listContainer.innerHTML = html;

    listContainer.querySelectorAll<HTMLDivElement>('.conversation-item').forEach((item) => {
      item.addEventListener('click', () => {
        const id = item.getAttribute('data-id');
        if (id && id !== state.activeConversationId) {
          selectConversation(id);
        }
      });
    });
  } catch (err) {
    listContainer.innerHTML = `
      <div class="state-container">
        ${ICONS.alert}
        <div class="state-title">Database Error</div>
        <div class="state-desc">${escapeHtml(String(err))}</div>
      </div>
    `;
  } finally {
    state.isLoadingConversations = false;
  }
}

// Select a Conversation and Load Messages
async function selectConversation(id: string): Promise<void> {
  state.activeConversationId = id;
  state.messageOffset = 0;
  state.messages = [];

  // Update active state in sidebar
  document.querySelectorAll('.conversation-item').forEach((item) => {
    item.classList.toggle('active', item.getAttribute('data-id') === id);
  });

  // Stop any playing voice note when switching conversations
  audioManager.stopCurrent();

  // Load stats for info drawer
  try {
    state.activeStats = await invoke<ConversationStats>('get_conversation_stats', {
      conversation_id: id,
    });
    if (state.isInfoDrawerOpen) {
      renderInfoDrawer();
    }
  } catch (err) {
    console.error('Failed to load conversation stats:', err);
  }

  await loadMessages(id, false);
}

// Load Messages for Active Conversation
async function loadMessages(conversationId: string, appendOlder: boolean): Promise<void> {
  const chatArea = document.querySelector<HTMLDivElement>('#chat-area')!;
  state.isLoadingMessages = true;

  if (!appendOlder) {
    chatArea.innerHTML = `
      <div class="chat-header">
        <div class="chat-header-profile" id="chat-header-profile">
          <div class="avatar" style="width: 36px; height: 36px; font-size: 14px;">
            ${(state.activeStats?.title || 'C').charAt(0).toUpperCase()}
          </div>
          <div>
            <div class="chat-header-title">${escapeHtml(state.activeStats?.title || 'Conversation')}</div>
            <div class="chat-header-subtitle">${state.activeStats?.participants.length || 0} participants</div>
          </div>
        </div>
        <div class="chat-header-actions">
          <button id="info-toggle-btn" class="icon-btn" title="Conversation Details">
            ${ICONS.info}
          </button>
        </div>
      </div>
      <div id="chat-messages" class="chat-messages">
        <div class="state-container">
          <div class="spinner"></div>
          <p style="margin-top: 10px; font-size: 13px;">Loading messages...</p>
        </div>
      </div>
    `;

    document.querySelector('#info-toggle-btn')?.addEventListener('click', toggleInfoDrawer);
    document.querySelector('#chat-header-profile')?.addEventListener('click', toggleInfoDrawer);
  }

  const messagesViewport = document.querySelector<HTMLDivElement>('#chat-messages')!;

  try {
    const res = await invoke<PaginatedMessages>('get_messages', {
      conversation_id: conversationId,
      limit: 50,
      offset: state.messageOffset,
      filter: state.activeFilter !== 'all' ? state.activeFilter : null,
    });

    state.totalMessages = res.total_count;

    if (appendOlder) {
      state.messages = [...res.messages, ...state.messages];
    } else {
      state.messages = res.messages;
    }

    renderMessageStream(appendOlder);
  } catch (err) {
    messagesViewport.innerHTML = `
      <div class="state-container">
        ${ICONS.alert}
        <div class="state-title">Failed to load messages</div>
        <div class="state-desc">${escapeHtml(String(err))}</div>
      </div>
    `;
  } finally {
    state.isLoadingMessages = false;
  }
}

// Render Messages in the Viewport
function renderMessageStream(appended: boolean): void {
  const viewport = document.querySelector<HTMLDivElement>('#chat-messages');
  if (!viewport) return;

  if (state.messages.length === 0) {
    viewport.innerHTML = `
      <div class="state-container">
        ${ICONS.chat}
        <div class="state-title">No messages in this chat</div>
        <div class="state-desc">No message records match the selected filter.</div>
      </div>
    `;
    return;
  }

  const prevScrollHeight = viewport.scrollHeight;
  const hasMoreOlder = state.messages.length < state.totalMessages;

  let html = '';

  if (hasMoreOlder) {
    html += `
      <button id="load-older-btn" class="load-older-btn">
        Load older messages (${state.totalMessages - state.messages.length} remaining)
      </button>
    `;
  }

  let lastDateStr = '';

  for (const msg of state.messages) {
    const msgDate = new Date(msg.timestamp_ms);
    const dateLabel = formatDateSeparator(msgDate);

    if (dateLabel !== lastDateStr) {
      html += `<div class="date-separator">${dateLabel}</div>`;
      lastDateStr = dateLabel;
    }

    if (msg.call_event) {
      // Call Event Record
      html += renderCallEvent(msg);
    } else {
      // Standard Chat Bubble
      html += renderMessageBubble(msg);
    }
  }

  viewport.innerHTML = html;

  // Bind dynamic event listeners on message elements
  bindMessageInteractions(viewport);

  // Restore scroll position or scroll to bottom
  if (appended) {
    const newScrollHeight = viewport.scrollHeight;
    viewport.scrollTop = newScrollHeight - prevScrollHeight;
  } else {
    viewport.scrollTop = viewport.scrollHeight;
  }

  // Load older button listener
  document.querySelector('#load-older-btn')?.addEventListener('click', () => {
    state.messageOffset += 50;
    if (state.activeConversationId) {
      loadMessages(state.activeConversationId, true);
    }
  });
}

// Render Call Event Card
function renderCallEvent(msg: MessageView): string {
  const call = msg.call_event!;
  const isVideo = call.call_type === 'VIDEO';
  const icon = isVideo ? ICONS.video : ICONS.phone;
  const outcomeClass = call.status === 'ANSWERED' ? 'answered' : 'missed';

  const durationStr = call.duration_seconds !== null && call.duration_seconds !== undefined
    ? formatDuration(call.duration_seconds)
    : 'Duration unavailable';

  const timeStr = formatMessageTime(msg.timestamp_ms);

  return `
    <div class="call-event-badge" id="msg-${msg.id}">
      <span class="${outcomeClass}">${icon}</span>
      <span>${call.direction === 'OUTGOING' ? 'Outgoing' : 'Incoming'} ${isVideo ? 'video' : 'voice'} call · ${durationStr} · ${timeStr}</span>
    </div>
  `;
}

// Render Standard Message Bubble
function renderMessageBubble(msg: MessageView): string {
  const directionClass = msg.is_outgoing ? 'outgoing' : 'incoming';
  const timeStr = formatMessageTime(msg.timestamp_ms);

  let replyHtml = '';
  if (msg.reply_preview) {
    const sender = msg.reply_preview.parent_sender_username || 'Replied to message';
    const snippet = msg.reply_preview.snippet || 'Original message';
    const parentIdAttr = msg.reply_preview.parent_message_id
      ? `data-parent-id="${msg.reply_preview.parent_message_id}"`
      : '';
    replyHtml = `
      <div class="reply-quote-box" ${parentIdAttr}>
        <div class="reply-quote-sender">${escapeHtml(sender)}</div>
        <div class="reply-quote-snippet">${escapeHtml(snippet)}</div>
      </div>
    `;
  }

  let contentHtml = '';

  // 1. Text Content
  if (msg.content) {
    contentHtml += `<div class="message-text">${formatMessageTextWithLinks(msg.content)}</div>`;
  }

  // 2. Attachments
  for (const att of msg.attachments) {
    if (att.attachment_type === 'audio') {
      contentHtml += renderVoicePlayer(att);
    } else if (att.attachment_type === 'image') {
      contentHtml += renderImageAttachment(att);
    } else if (att.attachment_type === 'video') {
      contentHtml += renderVideoAttachment(att);
    }
  }

  // 3. Shared Reel / Post
  if (msg.share_link) {
    contentHtml += `
      <div class="share-card">
        <div class="share-card-header">
          ${ICONS.reel}
          <span>Shared Instagram Content</span>
        </div>
        ${msg.share_text ? `<div style="font-size: 13px;">${escapeHtml(msg.share_text)}</div>` : ''}
        <button class="share-card-btn open-external-link" data-url="${escapeHtml(msg.share_link)}">
          <span>View on Instagram</span>
          ${ICONS.external}
        </button>
      </div>
    `;
  }

  // 4. Reactions
  let reactionsHtml = '';
  if (msg.reactions.length > 0) {
    reactionsHtml = `<div class="reactions-cluster">`;
    for (const rx of msg.reactions) {
      reactionsHtml += `<div class="reaction-pill" title="Reacted by ${escapeHtml(rx.username)}">${escapeHtml(rx.reaction_value)}</div>`;
    }
    reactionsHtml += `</div>`;
  }

  // 5. Read Status
  let readReceiptHtml = '';
  const firstRead = msg.read_receipts.find((r) => r.read_status === 'READ');
  if (firstRead && firstRead.read_at_ms) {
    readReceiptHtml = `<span class="read-status-badge">· Seen ${formatTimeAgo(firstRead.read_at_ms)}</span>`;
  }

  return `
    <div class="message-row ${directionClass}" id="msg-${msg.id}">
      ${!msg.is_outgoing && msg.sender_username ? `<div class="message-sender-name">${escapeHtml(msg.sender_username)}</div>` : ''}
      <div class="message-bubble">
        ${replyHtml}
        ${contentHtml}
        ${reactionsHtml}
      </div>
      <div class="message-meta-row">
        <span>${timeStr}</span>
        ${readReceiptHtml}
      </div>
    </div>
  `;
}

// Render Inline Voice Note Player
function renderVoicePlayer(att: AttachmentView): string {
  if (!att.is_available || !att.media_url) {
    return `
      <div class="missing-media-box">
        <div style="display: flex; align-items: center; gap: 6px;">
          ${ICONS.alert}
          <span>Voice message unavailable</span>
        </div>
        <button class="locate-btn" data-attachment-id="${att.id}">Locate File...</button>
      </div>
    `;
  }

  const isPlaying = audioManager.isPlaying(att.id);
  const playIcon = isPlaying ? ICONS.pause : ICONS.play;
  const initialDuration = att.duration_ms ? formatDuration(att.duration_ms / 1000) : '0:00';

  return `
    <div class="voice-player" data-attachment-id="${att.id}" data-url="${att.media_url}">
      <button class="voice-play-btn" data-attachment-id="${att.id}">
        ${playIcon}
      </button>
      <div class="voice-scrubber-container">
        <input type="range" class="voice-scrubber" min="0" max="100" value="0" step="0.1">
        <div class="voice-time-row">
          <span class="voice-current-time">0:00</span>
          <span class="voice-duration">${initialDuration}</span>
        </div>
      </div>
      <button class="voice-speed-btn" data-attachment-id="${att.id}" title="Playback Speed">1.0x</button>
    </div>
  `;
}

// Render Image Attachment
function renderImageAttachment(att: AttachmentView): string {
  if (!att.is_available || !att.media_url) {
    return `
      <div class="missing-media-box">
        <div style="display: flex; align-items: center; gap: 6px;">
          ${ICONS.alert}
          <span>Image file unavailable</span>
        </div>
        <button class="locate-btn" data-attachment-id="${att.id}">Locate File...</button>
      </div>
    `;
  }

  return `
    <div class="image-attachment-wrapper">
      <img class="image-attachment-img lightbox-trigger" src="${att.media_url}" alt="Shared image" loading="lazy">
    </div>
  `;
}

// Render Video Attachment
function renderVideoAttachment(att: AttachmentView): string {
  if (!att.is_available || !att.media_url) {
    return `
      <div class="missing-media-box">
        <div style="display: flex; align-items: center; gap: 6px;">
          ${ICONS.alert}
          <span>Video file unavailable</span>
        </div>
        <button class="locate-btn" data-attachment-id="${att.id}">Locate File...</button>
      </div>
    `;
  }

  return `
    <div class="video-attachment-wrapper">
      <video class="video-attachment-player" src="${att.media_url}" controls preload="metadata"></video>
    </div>
  `;
}

// Bind Interactions on Rendered Messages
function bindMessageInteractions(container: HTMLElement): void {
  // 1. Quoted Reply Jumps
  container.querySelectorAll<HTMLElement>('.reply-quote-box[data-parent-id]').forEach((box) => {
    box.addEventListener('click', () => {
      const parentId = box.getAttribute('data-parent-id');
      if (parentId) {
        const target = document.querySelector<HTMLElement>(`#msg-${parentId}`);
        if (target) {
          target.scrollIntoView({ behavior: 'smooth', block: 'center' });
          const bubble = target.querySelector<HTMLElement>('.message-bubble');
          if (bubble) {
            bubble.classList.remove('highlight-jump');
            void bubble.offsetWidth; // Trigger reflow
            bubble.classList.add('highlight-jump');
          }
        }
      }
    });
  });

  // 2. External Links
  container.querySelectorAll<HTMLButtonElement>('.open-external-link').forEach((btn) => {
    btn.addEventListener('click', async (e) => {
      e.stopPropagation();
      const url = btn.getAttribute('data-url');
      if (url) {
        try {
          await openUrl(url);
        } catch (err) {
          console.error('Failed to open URL:', err);
        }
      }
    });
  });

  // 3. Lightbox Triggers
  container.querySelectorAll<HTMLImageElement>('.lightbox-trigger').forEach((img) => {
    img.addEventListener('click', () => {
      openLightbox(img.src);
    });
  });

  // 4. Voice Player Controls
  container.querySelectorAll<HTMLElement>('.voice-player').forEach((player) => {
    const attachId = player.getAttribute('data-attachment-id')!;
    const mediaUrl = player.getAttribute('data-url')!;
    const playBtn = player.querySelector<HTMLButtonElement>('.voice-play-btn')!;
    const scrubber = player.querySelector<HTMLInputElement>('.voice-scrubber')!;
    const speedBtn = player.querySelector<HTMLButtonElement>('.voice-speed-btn')!;

    playBtn.addEventListener('click', () => {
      audioManager.toggle(attachId, mediaUrl);
    });

    scrubber.addEventListener('input', () => {
      const pct = parseFloat(scrubber.value);
      const dur = parseFloat(scrubber.getAttribute('data-raw-duration') || '0');
      if (dur > 0) {
        const target = (pct / 100) * dur;
        audioManager.seek(attachId, target);
      }
    });

    speedBtn.addEventListener('click', () => {
      const current = audioManager.getPlaybackRate();
      const next = current === 1.0 ? 1.5 : current === 1.5 ? 2.0 : 1.0;
      audioManager.setPlaybackRate(next);
      speedBtn.textContent = `${next.toFixed(1)}x`;
    });
  });

  // 5. Missing Media "Locate File..."
  container.querySelectorAll<HTMLButtonElement>('.locate-btn').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const attachId = btn.getAttribute('data-attachment-id');
      if (!attachId) return;

      btn.disabled = true;
      btn.textContent = 'Selecting file...';

      try {
        const updated = await invoke<AttachmentView | null>('locate_missing_media', {
          attachment_id: attachId,
        });

        if (updated && updated.is_available) {
          // Refresh message view
          if (state.activeConversationId) {
            loadMessages(state.activeConversationId, false);
          }
        } else {
          btn.disabled = false;
          btn.textContent = 'Locate File...';
        }
      } catch (err) {
        alert(`Failed to attach file: ${err}`);
        btn.disabled = false;
        btn.textContent = 'Locate File...';
      }
    });
  });
}

// Audio Manager Global Subscription (updates active audio UI in real time)
audioManager.subscribe((attachId: string, playState: PlaybackState, currentTime: number, duration: number) => {
  const player = document.querySelector<HTMLElement>(`.voice-player[data-attachment-id="${attachId}"]`);
  if (!player) return;

  const playBtn = player.querySelector<HTMLButtonElement>('.voice-play-btn');
  const scrubber = player.querySelector<HTMLInputElement>('.voice-scrubber');
  const curTimeEl = player.querySelector<HTMLElement>('.voice-current-time');
  const durEl = player.querySelector<HTMLElement>('.voice-duration');

  if (playBtn) {
    playBtn.innerHTML = playState === 'playing' ? ICONS.pause : ICONS.play;
  }

  if (curTimeEl) {
    curTimeEl.textContent = formatDuration(currentTime);
  }

  if (durEl && duration > 0) {
    durEl.textContent = formatDuration(duration);
  }

  if (scrubber && duration > 0) {
    scrubber.setAttribute('data-raw-duration', String(duration));
    const pct = (currentTime / duration) * 100;
    scrubber.value = String(pct);
  }
});

// Toggle Conversation Info Drawer
function toggleInfoDrawer(): void {
  state.isInfoDrawerOpen = !state.isInfoDrawerOpen;
  const drawer = document.querySelector<HTMLElement>('#info-drawer')!;

  if (state.isInfoDrawerOpen) {
    renderInfoDrawer();
    drawer.style.display = 'flex';
  } else {
    drawer.style.display = 'none';
  }
}

// Render Conversation Info Drawer Content
function renderInfoDrawer(): void {
  const drawer = document.querySelector<HTMLElement>('#info-drawer');
  if (!drawer || !state.activeStats) return;

  const stats = state.activeStats;

  let participantsHtml = '';
  for (const p of stats.participants) {
    const initial = p.username.charAt(0).toUpperCase() || 'U';
    participantsHtml += `
      <div class="participant-item">
        <div class="avatar" style="width: 32px; height: 32px; font-size: 13px;">${initial}</div>
        <div style="min-width: 0;">
          <div style="font-weight: 600; font-size: 13px;">${escapeHtml(p.full_name || p.username)}</div>
          <div style="font-size: 11px; color: var(--text-muted);">@${escapeHtml(p.username)} ${p.is_self ? '(You)' : ''}</div>
        </div>
      </div>
    `;
  }

  drawer.innerHTML = `
    <div class="info-drawer-header">
      <div class="info-drawer-title">Details</div>
      <button id="close-info-btn" class="icon-btn" title="Close Details">${ICONS.close}</button>
    </div>
    <div class="info-drawer-content">
      <div>
        <div class="info-section-title">Participants (${stats.participants.length})</div>
        <div>${participantsHtml}</div>
      </div>

      <div>
        <div class="info-section-title">Conversation Statistics</div>
        <div class="stat-grid">
          <div class="stat-card">
            <span class="stat-num">${stats.total_messages}</span>
            <span class="stat-label">Messages</span>
          </div>
          <div class="stat-card">
            <span class="stat-num">${stats.total_audio}</span>
            <span class="stat-label">Voice Notes</span>
          </div>
          <div class="stat-card">
            <span class="stat-num">${stats.total_images}</span>
            <span class="stat-label">Photos</span>
          </div>
          <div class="stat-card">
            <span class="stat-num">${stats.total_videos}</span>
            <span class="stat-label">Videos</span>
          </div>
          <div class="stat-card">
            <span class="stat-num">${stats.total_links}</span>
            <span class="stat-label">Reels & Links</span>
          </div>
          <div class="stat-card">
            <span class="stat-num">${stats.total_calls}</span>
            <span class="stat-label">Calls</span>
          </div>
        </div>
      </div>

      ${
        stats.first_message_timestamp_ms && stats.last_message_timestamp_ms
          ? `
        <div>
          <div class="info-section-title">History Span</div>
          <div style="font-size: 12px; color: var(--text-secondary); line-height: 1.6;">
            <div><strong>First:</strong> ${new Date(stats.first_message_timestamp_ms).toLocaleDateString()}</div>
            <div><strong>Latest:</strong> ${new Date(stats.last_message_timestamp_ms).toLocaleDateString()}</div>
          </div>
        </div>
      `
          : ''
      }
    </div>
  `;

  document.querySelector('#close-info-btn')?.addEventListener('click', toggleInfoDrawer);
}

// Lightbox Modal for Full Image Zoom
function openLightbox(imgSrc: string): void {
  const container = document.querySelector<HTMLDivElement>('#lightbox-container')!;
  container.innerHTML = `
    <div class="lightbox-backdrop">
      <button class="lightbox-close-btn" id="lightbox-close">${ICONS.close}</button>
      <img class="lightbox-img" src="${imgSrc}" alt="Expanded preview">
    </div>
  `;

  const closeBtn = document.querySelector<HTMLButtonElement>('#lightbox-close')!;
  const backdrop = document.querySelector<HTMLDivElement>('.lightbox-backdrop')!;

  const close = () => {
    container.innerHTML = '';
    window.removeEventListener('keydown', onKeyDown);
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Escape') close();
  };

  closeBtn.addEventListener('click', close);
  backdrop.addEventListener('click', (e) => {
    if (e.target === backdrop) close();
  });
  window.addEventListener('keydown', onKeyDown);
}

// Import Archive Modal
function openImportModal(): void {
  const container = document.querySelector<HTMLDivElement>('#modal-container')!;

  container.innerHTML = `
    <div class="modal-backdrop">
      <div class="modal-card">
        <div class="modal-header">
          <div style="font-weight: 600; font-size: 16px;">Import Instagram Data Archive</div>
          <button id="modal-close" class="icon-btn">${ICONS.close}</button>
        </div>
        <div class="modal-body">
          <p style="font-size: 13px; color: var(--text-secondary); line-height: 1.5;">
            Select an official Meta data export (<strong>.zip</strong> archive or extracted folder).
            Your data is processed 100% locally on this computer.
          </p>

          <div style="display: flex; gap: 8px;">
            <button id="pick-zip-btn" class="btn-secondary" style="flex: 1; display: flex; align-items: center; justify-content: center; gap: 6px;">
              ${ICONS.import} Select ZIP File
            </button>
            <button id="pick-folder-btn" class="btn-secondary" style="flex: 1; display: flex; align-items: center; justify-content: center; gap: 6px;">
              📁 Select Folder
            </button>
          </div>

          <div style="display: flex; flex-direction: column; gap: 6px;">
            <label style="font-size: 12px; color: var(--text-muted);">Selected Path:</label>
            <input id="archive-path-input" class="search-input" type="text" placeholder="C:\\path\\to\\instagram_export.zip" style="height: 38px;">
          </div>

          <div id="validation-summary-card" style="display: none;"></div>
          <div id="import-progress-card" style="display: none;"></div>
        </div>
        <div class="modal-footer">
          <button id="validate-btn" class="btn-secondary">Validate Source</button>
          <button id="start-import-btn" class="btn-primary" disabled>Start Import</button>
        </div>
      </div>
    </div>
  `;

  const closeBtn = document.querySelector<HTMLButtonElement>('#modal-close')!;
  const pickZipBtn = document.querySelector<HTMLButtonElement>('#pick-zip-btn')!;
  const pickFolderBtn = document.querySelector<HTMLButtonElement>('#pick-folder-btn')!;
  const pathInput = document.querySelector<HTMLInputElement>('#archive-path-input')!;
  const validateBtn = document.querySelector<HTMLButtonElement>('#validate-btn')!;
  const startImportBtn = document.querySelector<HTMLButtonElement>('#start-import-btn')!;
  const validationCard = document.querySelector<HTMLDivElement>('#validation-summary-card')!;
  const progressCard = document.querySelector<HTMLDivElement>('#import-progress-card')!;

  const closeModal = () => {
    container.innerHTML = '';
  };

  closeBtn.addEventListener('click', closeModal);

  pickZipBtn.addEventListener('click', async () => {
    try {
      const selected = await invoke<string | null>('pick_archive_file');
      if (selected) {
        pathInput.value = selected;
        validateSource(selected);
      }
    } catch (err) {
      alert(`Error opening file picker: ${err}`);
    }
  });

  pickFolderBtn.addEventListener('click', async () => {
    try {
      const selected = await invoke<string | null>('pick_archive_folder');
      if (selected) {
        pathInput.value = selected;
        validateSource(selected);
      }
    } catch (err) {
      alert(`Error opening folder picker: ${err}`);
    }
  });

  validateBtn.addEventListener('click', () => {
    const val = pathInput.value.trim();
    if (val) validateSource(val);
  });

  async function validateSource(archivePath: string): Promise<void> {
    validateBtn.disabled = true;
    validateBtn.textContent = 'Validating...';
    validationCard.style.display = 'block';
    validationCard.innerHTML = `<div style="font-size: 13px; color: var(--text-muted);"><div class="spinner" style="display: inline-block; vertical-align: middle; margin-right: 6px;"></div>Inspecting archive structure...</div>`;

    try {
      const summary = await invoke<ValidationSummary>('validate_archive_path', {
        archive_path: archivePath,
      });

      if (summary.is_valid) {
        validationCard.innerHTML = `
          <div style="background-color: rgba(16, 185, 129, 0.15); border: 1px solid var(--color-success); border-radius: 8px; padding: 12px; font-size: 13px;">
            <div style="font-weight: 600; color: #34d399; margin-bottom: 4px;">✓ Valid Instagram Archive Found</div>
            <div><strong>Format:</strong> ${summary.archive_format}</div>
            <div><strong>Conversations found:</strong> ${summary.total_conversations_found}</div>
            <div><strong>Message JSON files:</strong> ${summary.total_message_files_found}</div>
            ${summary.detected_account_id ? `<div><strong>Detected Account:</strong> @${summary.detected_account_id}</div>` : ''}
          </div>
        `;
        startImportBtn.disabled = false;
      } else {
        validationCard.innerHTML = `
          <div style="background-color: rgba(239, 68, 68, 0.15); border: 1px solid var(--color-danger); border-radius: 8px; padding: 12px; font-size: 13px; color: #fca5a5;">
            <div style="font-weight: 600; margin-bottom: 4px;">⚠️ Invalid Archive Structure</div>
            <div>No supported Instagram messages found at this location.</div>
          </div>
        `;
        startImportBtn.disabled = true;
      }
    } catch (err) {
      validationCard.innerHTML = `
        <div style="background-color: rgba(239, 68, 68, 0.15); border: 1px solid var(--color-danger); border-radius: 8px; padding: 12px; font-size: 13px; color: #fca5a5;">
          <div style="font-weight: 600;">Validation Failed:</div>
          <div>${escapeHtml(String(err))}</div>
        </div>
      `;
      startImportBtn.disabled = true;
    } finally {
      validateBtn.disabled = false;
      validateBtn.textContent = 'Validate Source';
    }
  }

  startImportBtn.addEventListener('click', async () => {
    const archivePath = pathInput.value.trim();
    if (!archivePath) return;

    startImportBtn.disabled = true;
    validateBtn.disabled = true;
    pickZipBtn.disabled = true;
    pickFolderBtn.disabled = true;

    progressCard.style.display = 'block';
    progressCard.innerHTML = `
      <div style="background-color: var(--bg-card); border-radius: 8px; padding: 14px; display: flex; flex-direction: column; gap: 8px;">
        <div style="display: flex; align-items: center; gap: 8px; font-weight: 600; font-size: 13px;">
          <div class="spinner"></div>
          <span>Importing messages and media into SQLite...</span>
        </div>
        <p style="font-size: 12px; color: var(--text-muted);">Please wait while conversations, voice notes, photos, and reactions are ingested.</p>
      </div>
    `;

    try {
      const report = await invoke<ImportReport>('import_archive_path', {
        archive_path: archivePath,
        account_id: null,
      });

      progressCard.innerHTML = `
        <div style="background-color: rgba(16, 185, 129, 0.15); border: 1px solid var(--color-success); border-radius: 8px; padding: 14px; font-size: 13px;">
          <div style="font-weight: 600; color: #34d399; font-size: 14px; margin-bottom: 6px;">
            🎉 Import ${report.status === 'SUCCESS' ? 'Complete' : 'Completed with Notes'}
          </div>
          <div style="line-height: 1.6;">
            <div><strong>Conversations imported:</strong> ${report.conversations_processed}</div>
            <div><strong>Messages imported:</strong> ${report.messages_imported}</div>
            <div><strong>Duplicate messages skipped:</strong> ${report.messages_skipped}</div>
            <div><strong>Media files stored:</strong> ${report.attachments_imported}</div>
            <div><strong>Missing media noted:</strong> ${report.attachments_missing}</div>
            <div><strong>Call events recorded:</strong> ${report.call_events_imported}</div>
          </div>
        </div>
      `;

      // Refresh sidebar conversation list
      await loadConversations();

      startImportBtn.textContent = 'Done';
      startImportBtn.disabled = false;
      startImportBtn.onclick = closeModal;
    } catch (err) {
      progressCard.innerHTML = `
        <div style="background-color: rgba(239, 68, 68, 0.15); border: 1px solid var(--color-danger); border-radius: 8px; padding: 14px; font-size: 13px; color: #fca5a5;">
          <div style="font-weight: 600; margin-bottom: 4px;">❌ Import Failed</div>
          <div>${escapeHtml(String(err))}</div>
        </div>
      `;
      startImportBtn.disabled = false;
      validateBtn.disabled = false;
    }
  });
}

// Utility Helpers
function escapeHtml(str: string): string {
  const map: Record<string, string> = {
    '&': '&amp;',
    '<': '&lt;',
    '>': '&gt;',
    '"': '&quot;',
    "'": '&#039;',
  };
  return str.replace(/[&<>"']/g, (m) => map[m]);
}

function formatMessageTextWithLinks(text: string): string {
  const escaped = escapeHtml(text);
  // Match standard http/https URLs and wrap in clickable buttons
  const urlRegex = /(https?:\/\/[^\s]+)/g;
  return escaped.replace(urlRegex, (url) => {
    return `<a href="${url}" class="open-external-link" data-url="${url}" style="color: #60a5fa; text-decoration: underline; cursor: pointer;">${url}</a>`;
  });
}

function formatMessageTime(timestampMs: number): string {
  const date = new Date(timestampMs);
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function formatDateSeparator(date: Date): string {
  const now = new Date();
  const isToday = date.toDateString() === now.toDateString();

  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  const isYesterday = date.toDateString() === yesterday.toDateString();

  if (isToday) return 'Today';
  if (isYesterday) return 'Yesterday';

  return date.toLocaleDateString([], {
    month: 'short',
    day: 'numeric',
    year: date.getFullYear() !== now.getFullYear() ? 'numeric' : undefined,
  });
}

function formatTimeAgo(timestampMs: number): string {
  const elapsed = Math.floor((Date.now() - timestampMs) / 1000);
  if (elapsed < 60) return 'just now';
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)}m ago`;
  if (elapsed < 86400) return `${Math.floor(elapsed / 3600)}h ago`;
  if (elapsed < 604800) return `${Math.floor(elapsed / 86400)}d ago`;

  const date = new Date(timestampMs);
  return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
}

// Application Entry Point
async function init(): Promise<void> {
  renderBaseLayout();

  try {
    const status = await invoke<AppStatus>('get_app_status');
    console.log('App Status:', status);

    const dbCheck = await invoke<string>('check_database');
    console.log('DB Integrity:', dbCheck);

    await loadConversations();
  } catch (err) {
    console.error('Initialization error:', err);
    const listContainer = document.querySelector<HTMLDivElement>('#conversation-list');
    if (listContainer) {
      listContainer.innerHTML = `
        <div class="state-container">
          ${ICONS.alert}
          <div class="state-title">Startup Failed</div>
          <div class="state-desc">${escapeHtml(String(err))}</div>
        </div>
      `;
    }
  }
}

init();
