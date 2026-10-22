import './style.css';
import { invoke } from '@tauri-apps/api/core';

interface AppStatus {
  app_name: string;
  version: string;
  architecture: string;
  target_os: string;
  local_first_mode: boolean;
  phase: string;
  database_ready: boolean;
}

const app = document.querySelector<HTMLDivElement>('#app')!;

app.innerHTML = `
  <div class="container">
    <div class="app-icon">💬</div>
    <h1>Instagram Chat History Viewer</h1>
    <p class="subtitle">Architecture Scaffold & Initial Setup (Phase 1)</p>
    
    <div class="status-card">
      <div class="status-row">
        <span class="status-label">Environment Target</span>
        <span class="status-value">Windows Desktop (WebView2)</span>
      </div>
      <div class="status-row">
        <span class="status-label">Backend Engine</span>
        <span class="status-value">Tauri v2 + Rust</span>
      </div>
      <div class="status-row">
        <span class="status-label">Local-First Storage</span>
        <span class="status-value status-badge">Active (Zero Telemetry)</span>
      </div>
      <div class="status-row">
        <span class="status-label">Implementation Phase</span>
        <span class="status-value">Phase 1: Scaffolding</span>
      </div>
    </div>

    <div class="action-box">
      <button id="ping-btn" type="button">Test Rust Backend IPC</button>
      <button id="status-btn" type="button">Check App Status</button>
    </div>

    <div id="output" class="backend-output">
      Click a button to verify frontend-to-Rust command boundary.
    </div>
  </div>
`;

const pingBtn = document.querySelector<HTMLButtonElement>('#ping-btn')!;
const statusBtn = document.querySelector<HTMLButtonElement>('#status-btn')!;
const outputDiv = document.querySelector<HTMLDivElement>('#output')!;

pingBtn.addEventListener('click', async () => {
  outputDiv.textContent = 'Sending IPC message to Rust backend...';
  try {
    const response = await invoke<string>('ping', { message: 'Ping from TypeScript frontend' });
    outputDiv.textContent = `Backend Response: ${response}`;
  } catch (err) {
    outputDiv.textContent = `Browser Fallback Mode: ${String(err)} (Tauri IPC active in native window)`;
  }
});

statusBtn.addEventListener('click', async () => {
  outputDiv.textContent = 'Querying system status from Rust...';
  try {
    const status = await invoke<AppStatus>('get_app_status');
    outputDiv.textContent = `Status: ${status.app_name} v${status.version} (${status.target_os}/${status.architecture}) - Phase: ${status.phase}`;
  } catch (err) {
    outputDiv.textContent = `Browser Fallback Mode: ${String(err)} (Tauri IPC active in native window)`;
  }
});
