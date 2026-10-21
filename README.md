# Instagram Chat History Viewer (`instagram-pro`)

A lightweight, Windows-focused, local-first desktop application designed to preserve, organize, search, and view personal Instagram conversation archives offline.

## Core Architecture
- **Desktop Shell:** Tauri v2 with Windows Evergreen WebView2
- **Backend:** Rust (embedded SQLite, local media streaming via custom protocol, session-local single instance mutex)
- **Frontend:** TypeScript + Vite
- **Storage:** Local-first SQLite database (WAL mode) with sandboxed `%LOCALAPPDATA%` media storage
- **Privacy:** 100% offline, zero telemetry, zero external cloud synchronization

## Governing Documentation
- [`PRD.md`](PRD.md) — Product Requirements Document (MVP scope, user workflows, acceptance criteria)
- [`TRD.md`](TRD.md) — Technical Requirements Document (architecture, schemas, deduplication, sync algorithms, traceability matrix)
- [`GEMINI.md`](GEMINI.md) — Operating Manual & Governance Guidelines (mandatory skills, checklists, rules of engagement)

## Development Setup

### Prerequisites
- [Node.js](https://nodejs.org/) (v20+ recommended)
- [Rust](https://rustup.rs/) (`rustc`, `cargo`)
- [Visual Studio C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) (MSVC toolchain)
- Microsoft Edge WebView2 Runtime (pre-installed on Windows 10/11)

### Getting Started
```bash
# 1. Install frontend dependencies
npm install

# 2. Run frontend in development mode
npm run dev

# 3. Build frontend bundle
npm run build

# 4. Launch native desktop application (requires Rust toolchain)
npm run tauri dev
```

## Project Status
- **Phase 0:** Requirements Analysis & Documentation Foundation (`PRD.md`, `TRD.md`, `GEMINI.md` v1.1.0) — **Complete**
- **Phase 1:** Architecture Scaffolding & Initial Setup (Tauri v2 + Vite/TS scaffold) — **Complete**
- **Phase 2:** Local Database Engine & Ingestion Pipeline — **Pending**
