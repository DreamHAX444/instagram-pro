# GEMINI.md
# Operating Manual & Governance Guidelines for Instagram Chat History Viewer

**Version:** 1.1.0  
**Status:** Mandatory Governance Contract (Post-Audit Revision)  
**Applies To:** All AI coding agents, pair programmers, and future tasks in this repository.  

---

## 1. Project Context

### 1.1 Product Mission & MVP Scope
The **Instagram Chat History Viewer** is a lightweight, Windows-focused, local-first desktop application designed to preserve, organize, search, and view personal Instagram conversation archives offline. The application presents a modern, desktop-adapted messaging dashboard strongly reminiscent of Instagram's direct messaging interface.

Key MVP Capabilities:
- Local-first embedded database (SQLite 3.45+ with Write-Ahead Logging).
- Resilient ingestion of official Meta "Download Your Information" (JSON format) archives supporting legacy and modern Accounts Center folder layouts.
- Chronological message streaming with sender attribution and line break preservation.
- **Inline Voice Message Player (Highest Priority):** Play/pause, scrubbing, duration calculation, and missing-media handling.
- Inline image/video viewers with lightbox zoom and range-request streaming.
- Micro-interactions: Emoji reaction pills, dynamic read receipts ("Seen X ago" when available, explicit unavailable fallback), and quoted reply headers.
- **Voice and Video Call Records (Mandatory):** Timeline entries capturing call type, direction, outcome, start timestamp, duration, and derived end timestamp.
- Fast keyword search, participant filtering, and attachment filtering.
- Checkpointed, recoverable ingestion with deterministic deduplication that preserves legitimate identical messages.
- Optional user-controlled auto-start on Windows login via registry integration.

### 1.2 Strict Architectural Constraints & Source Boundaries
- **Local-First & Zero Telemetry:** No personal conversation data, participant names, media files, or tokens may leave the local machine.
- **Personal Account API Reality:** Meta provides **zero official live APIs for personal Instagram accounts**. Synchronization for personal accounts consists strictly of **offline incremental archive diffing** (importing newer export packages).
- **Authorized Data Sources Only:** The application ingests official Meta data export archives and supports official Meta Graph API endpoints for eligible professional accounts. **Private browser session scraping, cookie harvesting, and reverse-engineered mobile endpoints are strictly prohibited.**
- **No Data Fabrication:** The system must never invent missing timestamps, fake read receipts, synthetic thumbnails, unrecorded call durations, or unverified parent message links.

### 1.3 Governing Documentation Hierarchy
1. [`PRD.md`](file:///c:/Users/ZNS/Documents/instagram_pro/PRD.md): Product Requirements Authority. Dictates what features exist, user workflows, and verifiable acceptance criteria.
2. [`TRD.md`](file:///c:/Users/ZNS/Documents/instagram_pro/TRD.md): Technical Architecture Authority. Dictates data models, SQLite schemas, system boundaries, streaming protocols, and testing strategies.
3. [`GEMINI.md`](file:///c:/Users/ZNS/Documents/instagram_pro/GEMINI.md): Agent Governance & Operational Manual. Dictates mandatory skills, pre-task checklists, coding standards, and definition of done.

---

## 2. Mandatory Skill Gate: Ponytail Skill

> [!CRITICAL]
> **Every task MUST use the Ponytail Skill. This is mandatory without exception.**

### 2.1 Universal Applicability
The Ponytail Skill is not optional, nor is it reserved only for refactoring. It applies to **EVERY** task executed in this repository, including but not limited to:
- Requirements analysis and scope definition.
- Repository inspection and architectural planning.
- Database design, DDL migrations, and query authoring.
- Backend services, data adapters, and synchronization logic.
- UI components, layout styling, and accessibility.
- Testing, debugging, chaos engineering, and benchmark verification.
- Documentation updates and code reviews.

### 2.2 Skill Location & Invocation Protocol
- **Skill Location:** `C:\Users\ZNS\.gemini\config\plugins\ponytail\skills\ponytail\SKILL.md` (and related commands `ponytail-audit`, `ponytail-debt`, `ponytail-help`, `ponytail-review`).
- **Pre-Task Requirement:** Before performing any task, the agent MUST read and apply the instructions from the Ponytail Skill.
- **Blocker Escalation:** If the Ponytail Skill is unavailable, corrupted, or fails to load, **STOP IMMEDIATELY AND REPORT THE BLOCKER TO THE USER**. Do not attempt to emulate or claim compliance without the actual skill.
- Merely stating that the skill was used without adhering to its principles does not satisfy this requirement.

### 2.3 The Ponytail Ladder (Hierarchy of Laziness)
Always stop at the first rung that holds:
1. **Does this need to exist at all?** If speculative or unrequested, skip it. (YAGNI)
2. **Already in this codebase?** Reuse existing helpers, patterns, or utilities. Do not duplicate.
3. **Standard Library does it?** Use the platform's standard library before pulling in third-party packages.
4. **Native platform feature covers it?** Use OS native capabilities (e.g., SQLite WAL mode, Windows Registry for auto-start, HTML5 Audio for playback, Windows session mutex for single-instance).
5. **Already-installed dependency solves it?** Never add a new library when a few lines or existing dependencies suffice.
6. **Can it be one line?** Make it one line.
7. **Only then:** Write the minimum necessary clean code.

### 2.4 Core Ponytail Principles
- **Boring Over Clever:** Write transparent, predictable code that requires no mental gymnastics.
- **Fewest Files Possible:** Keep diffs compact and changes localized.
- **Never Simplify Away Essentials:** Never cut corners on input validation at trust boundaries, data-loss prevention, cryptographic integrity (SHA-256), accessibility, or explicitly requested features.
- **Ceiling Annotations:** Mark any deliberate simplification with a ceiling comment:  
  `// ponytail: <current shortcut>, <future upgrade path if ceiling hit>`

---

## 3. UI/UX Skill Gate: UI/UX Prompt Skill

> [!IMPORTANT]
> **The UI/UX Prompt Skill is MANDATORY for all interface-related tasks.**

### 3.1 Applicability
Whenever a task involves designing, implementing, modifying, styling, or reviewing any of the following interface elements, the agent MUST invoke the UI/UX Prompt Skill (`ui-ux-pro-max`):
- Application window shell, title bar, and system navigation.
- Conversation sidebar, search input, and participant list.
- Message timeline, virtualized scroll viewport, and message bubbles.
- **Inline voice message player, waveforms, and scrubbers.**
- Image/video thumbnails, carousel grids, and lightbox modals.
- Reaction badges, read receipts ("Seen" indicators), and reply context headers.
- Shared Instagram Reel / Post cards.
- Voice and video call timeline event cards.
- Loading spinners, empty states, error notices, and missing-media placeholders.

### 3.2 Dual-Skill Mandate
When undertaking a UI task, **BOTH** the Ponytail Skill and the UI/UX Prompt Skill apply:
- **Ponytail** enforces minimal code, standard native elements, zero-dependency audio, and bounded memory.
- **UI/UX Prompt Skill** ensures aesthetic polish, pixel-perfect Instagram alignment, design system consistency, dark/light themes, and WCAG 2.1 AA accessibility.

### 3.3 Skill Location & Reference
- **Skill Path:** `C:\Users\ZNS\.gemini\config\plugins\ui-ux-pro-max\skills\ui-ux-pro-max\SKILL.md` (and agent `ui_ux_master_agent`).
- If the skill cannot be located, report the blocker before modifying UI components.

---

## 4. Mandatory Pre-Task Checklist

Every agent beginning any development or maintenance task in this repository MUST follow this 10-step sequence without exception:

1. **Read Requirements:** Check [`PRD.md`](file:///c:/Users/ZNS/Documents/instagram_pro/PRD.md) for relevant functional requirements (`FR-xxx`) and acceptance criteria (`AC-xxx`).
2. **Read Architecture & Technical Constraints:** Check [`TRD.md`](file:///c:/Users/ZNS/Documents/instagram_pro/TRD.md) for data schemas, component boundaries, and testing patterns.
3. **Invoke Ponytail Skill:** Review `C:\Users\ZNS\.gemini\config\plugins\ponytail\skills\ponytail\SKILL.md` and apply the ladder.
4. **Invoke UI/UX Skill (If Applicable):** For any interface task, load `C:\Users\ZNS\.gemini\config\plugins\ui-ux-pro-max\skills\ui-ux-pro-max\SKILL.md`.
5. **Inspect Existing Files & State:** Examine current directory contents, schemas, and tests before writing new code.
6. **Define Bounded Scope:** State the exact files to be created or modified; avoid speculative edits.
7. **Execute Minimal Implementation:** Write the cleanest, smallest working implementation adhering to Ponytail.
8. **Execute Automated Validation:** Run unit tests, database checks, or linting. Observe actual execution output.
9. **Review Git Diff:** Inspect changes to ensure no extraneous files, dead code, or unintended formatting changes were introduced.
10. **Report Results:** Report modified files, executed tests, observed results, requirement coverage, and remaining debt.

---

## 5. Documentation Authority & Conflict Resolution

### 5.1 Authority Hierarchy
- **Product Scope Authority:** [`PRD.md`](file:///c:/Users/ZNS/Documents/instagram_pro/PRD.md) is the final arbiter of functional capabilities, user workflows, and MVP inclusions/exclusions.
- **Technical Architecture Authority:** [`TRD.md`](file:///c:/Users/ZNS/Documents/instagram_pro/TRD.md) is the final arbiter of data schemas, database migrations, file storage, network boundaries, and protocol handlers.
- **Agent Governance Authority:** [`GEMINI.md`](file:///c:/Users/ZNS/Documents/instagram_pro/GEMINI.md) is the final arbiter of AI developer actions, skill enforcement, and process gates.

### 5.2 Conflict Resolution Protocol
If a discrepancy is discovered between documents:
1. **Halt & Document:** Do not silently favor one document over another or delete requirements.
2. **Identify Mismatch:** Detail the exact contradiction, citing file paths and section numbers.
3. **Propose Minimal Resolution:** Propose the simplest, lowest-friction reconciliation aligned with MVP goals.
4. **Await User Authorization:** Do not edit the governing documents unless specifically instructed by the user or executing an explicitly authorized documentation maintenance phase.

---

## 6. Engineering & Implementation Rules

### 6.1 Database & Data Integrity
- **Relational Integrity:** Always ensure `PRAGMA foreign_keys = ON;` is enabled on every SQLite connection.
- **Durable Transactions:** Ingestion must occur in explicit batches (500–1000 items) wrapped in `BEGIN IMMEDIATE ... COMMIT;`.
- **Decoupled Checkpoints:** Checkpoints in `sync_checkpoints` must be keyed by `conversation_source_id` (independent of transient execution `job_id` UUIDs). Advancing checkpoints MUST occur in the exact same transaction as message persistence.
- **Deduplication with Identical Message Preservation:** Never use timestamp alone as a unique identifier. Use `source_message_id` when present. For archive exports without IDs, use a scoped hash:
  $$\text{SHA256}(\text{conversation\_source\_id} \parallel \text{sender} \parallel \text{timestamp\_ms} \parallel \text{source\_file\_path} \parallel \text{array\_index} \parallel \text{content\_digest})$$
  This guarantees identical repeated messages within a file are never silently discarded, while re-importing the same archive remains 100% idempotent.
- **Graceful Nullability & Explicit Fallbacks:** Fields absent from source exports (read receipts, call durations, parent reply IDs) must be modeled as nullable or use explicit state enums (`UNAVAILABLE_FROM_SOURCE`).
- **Forward-Only Migrations:** All schema changes must be implemented as numbered, idempotent SQL migration scripts tracked in `schema_migrations`.

### 6.2 Media Management & Playback
- **Bitstream Preservation:** Audio and video files must be preserved in their native bitstream without lossy re-encoding.
- **Bounded RAM Consumption:** Audio and video playback must utilize HTTP Range / Byte-stream requests (`asset://` custom protocol) directly from disk. Never buffer entire files into memory.
- **Integrity Checksums:** Media files must be indexed with SHA-256 checksums to detect disk corruption and skip redundant copies.
- **Explicit Missing State:** When an audio or video file cannot be found on disk, the UI must render an explicit "Media unavailable" warning and allow the user to locate the missing file manually.

### 6.3 Windows Platform & Lifecycle Integration
- **Unprivileged Execution:** The application must run under standard Windows user accounts without administrative elevation.
- **Session-Local Mutex:** Enforce single-instance locking using `Local\InstagramChatHistoryViewer_InstanceMutex` (never `Global\`, which requires admin rights).
- **Dynamic Startup Path:** Write the registry startup entry (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`) using the dynamically resolved `std::env::current_exe()`.
- **Zero Credentials in Repository:** Never commit API keys, personal access tokens, or account passwords to Git.
- **DPAPI Encryption:** When storing tokens for official Meta Graph API integrations, always encrypt them via Windows DPAPI before persisting to Windows Credential Manager.
- **Redacted Diagnostics:** Application log files must sanitize user message content, participant names, and access tokens before flushing to disk.

### 6.4 Empirical Performance Verification
- All performance, memory, startup time, and scroll framerate figures are **`[Target - Unverified Benchmark]`** until empirically measured on physical Windows test hardware via `TS-BENCH`. Never claim performance targets as verified facts without test evidence.

---

## 7. UI/UX Implementation Rules

### 7.1 Visual Hierarchy & Component Clarity
- **Instagram DM Visual Language:** Match Instagram's clean aesthetic: rounded bubbles (18px radius), subtle incoming grey/dark bubble background, distinctive outgoing blue/gradient bubble background.
- **Component Distinctiveness:** Ensure every message type is instantly recognizable:
  - Text messages: Clean typographic hierarchy with readable line height (1.4).
  - Voice notes: Distinctive audio waveform/bar, elapsed/total duration, prominent play/pause button.
  - Image/Video: Rounded media cards with aspect-ratio preservation and play overlays.
  - Reactions: Compact emoji pills overlapping the bottom corner of the parent bubble.
  - Quoted Replies: Subtle inset preview bar with author name and truncated text snippet; click-to-scroll enabled ONLY when parent message ID exists.
  - Shared Reels/Posts: Card format displaying Instagram icon, "Shared a Reel / Post" headline, and external browser link.
  - Call Records: Centered, subdued timeline badges with distinct icons (phone for voice, camera for video; green for answered, red/grey for missed), displaying start time, duration, and derived end time.

### 7.2 State Coverage Mandate
Every UI component must explicitly handle and display all five lifecycle states:
1. **Loading State:** Subtle skeleton placeholders during conversation switching.
2. **Empty State:** Helpful onboarding graphic when no conversations or messages exist.
3. **Active/Populated State:** Smooth virtualized message stream.
4. **Missing/Corrupted Data State:** Non-intrusive warning badges (e.g., "Audio file missing").
5. **Error / Unavailable State:** Actionable error prompts or explicit "Unavailable from source" notices.

---

## 8. Definition of Done (DoD)

A development task is considered complete **ONLY** when all of the following conditions are met:

- [ ] **Skills Utilized:** The Ponytail Skill was loaded and followed. If UI work was involved, the UI/UX Prompt Skill was loaded and followed.
- [ ] **Scope Enforced:** Only files strictly necessary for the authorized task were modified. No unsolicited refactoring or speculative features were added.
- [ ] **Requirements Traceability:** The implementation satisfies the targeted `FR-xxx` or `NFR-xxx` requirement in `PRD.md` and aligns with `TRD.md`.
- [ ] **Tests Executed & Verified:** Automated tests covering the change were run locally and passed. Actual test output was observed.
- [ ] **No Regressions:** Existing functionality and database constraints remain intact.
- [ ] **Clear Failure Handling:** Error states, missing media, and edge cases are handled explicitly without application crashes.
- [ ] **Truthful Reporting:** Final report clearly states modified files, test commands run, verification output, and any remaining open questions.

---

## 9. Current Phase Restriction (Phase 0.1)

> [!WARNING]
> **Phase 0.1 Documentation Boundary:**  
> Phase 0.1 is strictly limited to establishing and correcting the documentation foundation:
> 1. `PRD.md` (v1.1.0)
> 2. `TRD.md` (v1.1.0)
> 3. `GEMINI.md` (v1.1.0)
> 
> **Do not write application source code, scaffold directory trees, initialize package managers, install dependencies, or advance to Phase 1 until explicitly directed by the user.**
