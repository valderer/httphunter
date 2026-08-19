use std::{io::Read, net::SocketAddr, sync::Arc};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    capture::{HttpSession, MemoryStore},
    system_proxy::SystemProxyController,
};

#[derive(Clone)]
struct AppState {
    store: Arc<MemoryStore>,
    system_proxy: SystemProxyController,
}

pub async fn run(
    listen: SocketAddr,
    store: Arc<MemoryStore>,
    system_proxy: SystemProxyController,
) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/", get(dashboard))
        .route("/health", get(health))
        .route("/control/status", get(control_status))
        .route("/control/start", post(start_capture))
        .route("/control/stop", post(stop_capture))
        .route("/sessions", get(list_sessions).delete(clear_sessions))
        .route("/sessions/:id", get(get_session))
        .route("/export/har", get(export_har))
        .with_state(AppState {
            store,
            system_proxy,
        });
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, "local API listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn dashboard() -> Html<&'static str> {
    Html(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>http-hunter</title>
  <style>
    :root { color-scheme: dark; font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif; }
    body { height: 100vh; margin: 0; overflow: hidden; background: #0d1117; color: #c9d1d9; font-size: 14px; }
    header { min-height: 52px; box-sizing: border-box; padding: 10px 20px; background: #161b22; border-bottom: 1px solid #30363d; display: flex; align-items: center; gap: 12px; }
    .header-spacer { flex: 1 1 auto; }
    h1 { margin: 0; color: #f0f6fc; font-size: 16px; font-weight: 600; }
    main { display: grid; grid-template-columns: minmax(340px, 0.85fr) minmax(380px, 1.15fr); height: calc(100vh - 52px); }
    section { min-width: 0; overflow: auto; }
    #list { display: flex; flex-direction: column; box-sizing: border-box; background: #0d1117; border-right: 1px solid #30363d; overflow: hidden; }
    .toolbar { flex: 0 0 auto; padding: 9px 12px; background: #161b22; display: flex; gap: 7px; flex-wrap: wrap; border-bottom: 1px solid #30363d; }
    .toolbar-spacer { flex: 1 1 auto; }
    dialog { width: min(420px, calc(100vw - 32px)); padding: 0; color: #c9d1d9; background: #161b22; border: 1px solid #30363d; border-radius: 8px; box-shadow: 0 16px 48px rgba(1, 4, 9, 0.7); }
    dialog::backdrop { background: rgba(1, 4, 9, 0.65); }
    .filter-dialog-header, .filter-dialog-footer { display: flex; align-items: center; gap: 8px; padding: 12px 16px; }
    .filter-dialog-header { border-bottom: 1px solid #30363d; }
    .filter-dialog-header h2 { margin: 0; color: #f0f6fc; font-size: 14px; font-weight: 600; }
    .filter-dialog-header button { margin-left: auto; min-width: 28px; padding: 4px 8px; }
    .filter-dialog-body { padding: 16px; display: grid; gap: 12px; }
    .filter-field { display: grid; gap: 6px; }
    .filter-field label { color: #8b949e; font-size: 12px; }
    .filter-options { display: grid; gap: 10px; padding-top: 4px; }
    .filter-options label { display: flex; align-items: center; gap: 8px; color: #c9d1d9; }
    .filter-options input { margin: 0; }
    .filter-dialog-footer { justify-content: flex-end; border-top: 1px solid #30363d; }
    .filter-dialog-footer .primary { color: #fff; background: #1f6feb; border-color: #388bfd; }
    .filter-dialog-footer .primary:hover { background: #388bfd; }
    .request-table { min-height: 0; flex: 1 1 auto; display: flex; flex-direction: column; background: #161b22; }
    .request-header { flex: 0 0 auto; width: calc(100% - 12px); }
    .request-scroll { min-height: 0; flex: 1 1 auto; overflow-y: scroll; scrollbar-gutter: stable; }
    .request-scroll::-webkit-scrollbar { width: 12px; }
    .request-scroll::-webkit-scrollbar-track { background: #161b22; border-left: 1px solid #21262d; }
    .request-scroll::-webkit-scrollbar-thumb { background: #30363d; border: 3px solid #161b22; border-radius: 6px; }
    .request-scroll::-webkit-scrollbar-thumb:hover { background: #484f58; }
    input, button, select { box-sizing: border-box; background: #0d1117; color: #c9d1d9; border: 1px solid #30363d; border-radius: 6px; padding: 6px 9px; font: inherit; font-size: 13px; }
    input::placeholder { color: #7d8590; }
    input:focus, button:focus-visible, a:focus-visible { outline: 2px solid #1f6feb; outline-offset: -1px; }
    button { background: #21262d; color: #c9d1d9; box-shadow: 0 1px 0 rgba(27, 31, 36, 0.1); }
    button:hover { background: #30363d; border-color: #8b949e; }
    button, a { cursor: pointer; text-decoration: none; }
    a { color: #58a6ff; }
    a:hover { text-decoration: underline; }
    table { width: 100%; border-collapse: collapse; table-layout: fixed; font-size: 12px; }
    th, td { text-align: left; padding: 8px 9px; border-bottom: 1px solid #21262d; white-space: nowrap; }
    thead { background: #161b22; }
    thead th { color: #8b949e; font-weight: 500; font-size: 11px; text-transform: uppercase; }
    tr { cursor: pointer; }
    tr:hover { background: #161b22; }
    .request-row.selected { background: #1c2d41; box-shadow: inset 3px 0 0 #1f6feb; }
    .request-row.selected:hover { background: #233b55; }
    .request-row.new-request { animation: request-arrival 500ms ease-out both; }
    @keyframes request-arrival {
      0% { opacity: 1; background: #1f6feb66; }
      100% { opacity: 1; background: transparent; }
    }
    @media (prefers-reduced-motion: reduce) {
      .request-row.new-request { animation: none; }
      .capture-toggle.running::before { animation: none; }
    }
    .status-2xx { color: #3fb950; }
    .status-3xx { color: #d29922; }
    .status-4xx, .status-5xx { color: #f85149; }
    .capture-toggle { min-width: 92px; display: inline-flex; align-items: center; justify-content: center; gap: 6px; border-color: #6e1717; background: #2d1214; color: #ffb4b0; font-weight: 600; }
    .capture-toggle::before { content: ''; width: 7px; height: 7px; border-radius: 50%; background: currentColor; }
    .capture-toggle:hover { border-color: #f85149; background: #3d1719; }
    .capture-toggle.running { border-color: #238636; background: #12261e; color: #7ee787; }
    .capture-toggle.running:hover { border-color: #3fb950; background: #173326; }
    .capture-toggle.running::before { animation: capture-indicator 1.2s ease-in-out infinite; }
    @keyframes capture-indicator { 0%, 100% { opacity: 0.45; box-shadow: 0 0 0 0 rgba(63, 185, 80, 0); } 50% { opacity: 1; box-shadow: 0 0 0 5px rgba(63, 185, 80, 0.22); } }
    .capture-toggle:disabled { opacity: 0.6; cursor: wait; }
    .tree-root td { background: #161b22; color: #f0f6fc; font-weight: 600; padding-top: 9px; padding-bottom: 8px; }
    .tree-host td { background: #0d1117; color: #c9d1d9; font-weight: 600; padding-top: 7px; padding-bottom: 7px; }
    .tree-toggle { color: inherit; border: 0; background: transparent; padding: 0; margin-right: 6px; font: inherit; }
    .tree-count { color: #8b949e; font-size: 12px; font-weight: 400; }
    .request-row .url-cell { padding-left: 24px; }
    #list th:nth-child(1), #list td:nth-child(1) { width: 48px; text-align: center; }
    #list th:nth-child(1), #list td:nth-child(1) { padding-right: 2px; }
    #list th:nth-child(2), #list td:nth-child(2) { padding-left: 3px; }
    #list th:nth-child(3), #list td:nth-child(3) { width: 42px; text-align: center; }
    #list th:nth-child(4), #list td:nth-child(4) { width: 51px; text-align: center; }
    #list th:nth-child(5), #list td:nth-child(5) { width: 51px; text-align: center; }
    #list th:nth-child(3), #list td:nth-child(3), #list th:nth-child(4), #list td:nth-child(4), #list th:nth-child(5), #list td:nth-child(5) { box-sizing: border-box; padding-left: 2px; padding-right: 2px; font-size: 12px; }
    .url-cell { overflow: hidden; text-overflow: ellipsis; }
    .url-host { color: #c9d1d9; font-weight: 500; }
    .url-path { color: #8b949e; display: block; overflow: hidden; text-overflow: ellipsis; }
    .type-cell { color: #8b949e; overflow: hidden; text-overflow: ellipsis; }
    .size-cell { color: #8b949e; text-align: right; }
    .method-status { display: grid; gap: 2px; font-size: 11px; }
    .method-status .method { color: #c9d1d9; font-weight: 600; }
    #detail { padding: 16px 20px; background: #0d1117; }
    .summary { display: grid; grid-template-columns: 112px 1fr; gap: 7px 12px; margin-bottom: 18px; }
    .summary-label { color: #8b949e; }
    .summary-value { overflow-wrap: anywhere; }
    .detail-actions, .body-tools { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
    .detail-actions { margin: -4px 0 14px; }
    .detail-actions button, .body-tools button { padding: 5px 7px; font-size: 12px; }
    .body-tools { margin-bottom: 8px; }
    .body-search { flex: 1 1 150px; min-width: 0; padding: 5px 7px; font-size: 12px; }
    .search-count { color: #8b949e; font-size: 12px; min-width: 42px; }
    mark { background: #bb8009; color: #fff8c5; border-radius: 2px; padding: 0 1px; }
    #notice { position: fixed; right: 18px; bottom: 18px; opacity: 0; transform: translateY(6px); pointer-events: none; background: #1f6feb; color: #fff; border: 1px solid #58a6ff; border-radius: 6px; padding: 8px 10px; font-size: 13px; box-shadow: 0 8px 24px rgba(1, 4, 9, 0.35); transition: opacity 150ms ease, transform 150ms ease; }
    #notice.visible { opacity: 1; transform: translateY(0); }
    .tabs { display: flex; gap: 2px; border-bottom: 1px solid #30363d; margin-bottom: 14px; flex-wrap: wrap; }
    .tab { color: #8b949e; border: 0; border-bottom: 2px solid transparent; border-radius: 0; background: transparent; box-shadow: none; padding: 8px 10px; }
    .tab:hover { color: #c9d1d9; background: #161b22; border-color: transparent; }
    .tab.active { color: #f0f6fc; border-bottom-color: #f78166; }
    .panel { display: none; }
    .panel.active { display: block; }
    .headers { width: 100%; border-collapse: collapse; font-size: 13px; }
    .headers th, .headers td { white-space: normal; vertical-align: top; overflow-wrap: anywhere; }
    .headers th { width: 30%; color: #8b949e; font-weight: 500; }
    pre { white-space: pre-wrap; overflow-wrap: anywhere; background: #161b22; border: 1px solid #30363d; padding: 14px; border-radius: 6px; margin: 0; line-height: 1.5; }
    .body { min-height: 180px; max-height: 82vh; overflow: auto; }
    .binary-body { border: 1px solid #30363d; border-radius: 6px; background: #161b22; }
    .binary-body summary { padding: 10px 12px; color: #d29922; cursor: pointer; user-select: none; }
    .binary-body[open] summary { border-bottom: 1px solid #30363d; }
    .binary-body pre { border: 0; border-radius: 0 0 6px 6px; }
    .syntax-key { color: #79c0ff; }
    .syntax-string, .syntax-attr-value { color: #a5d6ff; }
    .syntax-number { color: #79c0ff; }
    .syntax-literal { color: #ff7b72; }
    .binary { color: #d29922; }
    .muted { color: #8b949e; }
  </style>
</head>
<body>
  <header>
    <h1>HTTP-Hunter</h1>
    <span class="muted">capture console</span>
    <span class="header-spacer"></span>
    <a href="/export/har" download="capture.har">Download HAR</a>
    <button onclick="refreshUi()">Refresh UI</button>
  </header>
  <main>
    <section id="list">
      <div class="toolbar">
        <button id="capture-toggle" class="capture-toggle" onclick="toggleCapture()" disabled title="Toggle capture and system proxy">Stopped</button>
        <span class="toolbar-spacer"></span>
        <button onclick="openFilters()">Filters</button>
        <button onclick="clearSessions()">Clear</button>
      </div>
      <div class="request-table">
        <table class="request-header">
          <thead><tr><th>Req</th><th>Host / Path</th><th>Type</th><th>Size</th><th>Time</th></tr></thead>
        </table>
        <div class="request-scroll">
          <table>
            <tbody id="rows"></tbody>
          </table>
        </div>
    </section>
    <section id="detail">
      <p class="muted">Select a request to inspect it.</p>
    </section>
  </main>
  <dialog id="filters-dialog" aria-labelledby="filters-title">
    <div class="filter-dialog-header">
      <h2 id="filters-title">Filters</h2>
      <button onclick="closeFilters()" title="Close filters">Close</button>
    </div>
    <div class="filter-dialog-body">
      <div class="filter-field"><label for="host">Host</label><input id="host" placeholder="example.com"></div>
      <div class="filter-field"><label for="method">Method</label><input id="method" placeholder="GET"></div>
      <div class="filter-field"><label for="status">Status</label><input id="status" inputmode="numeric" placeholder="200"></div>
      <div class="filter-options">
        <label><input id="hide-static" type="checkbox"> Hide static resources</label>
        <label><input id="group-host" type="checkbox"> Group by domain</label>
      </div>
    </div>
    <div class="filter-dialog-footer">
      <button onclick="resetFilters()">Reset</button>
      <button class="primary" onclick="applyFilters()">Apply</button>
    </div>
  </dialog>
  <div id="notice" role="status"></div>
  <script>
    let sessions = [];
    const collapsed = new Set();
    let captureEnabled = false;
    let selectedSessionId = null;
    let activeTab = 'overview';
    let selectedSession = null;
    let bodyModes = { request: 'pretty', response: 'pretty' };
    let bodyViews = {};
    const knownSessionIds = new Set();
    let newSessionIds = new Set();
    let hasLoadedSessions = false;
    let isLoadingSessions = false;
    function refreshUi() {
      window.location.replace(window.location.pathname + '?refresh=' + Date.now());
    }
    function openFilters() {
      document.getElementById('filters-dialog').showModal();
    }
    function closeFilters() {
      document.getElementById('filters-dialog').close();
    }
    async function applyFilters() {
      closeFilters();
      await loadSessions();
    }
    async function resetFilters() {
      for (const id of ['host', 'method', 'status']) document.getElementById(id).value = '';
      document.getElementById('hide-static').checked = false;
      document.getElementById('group-host').checked = false;
      await loadSessions();
    }
    async function refreshCaptureStatus() {
      const response = await fetch('/control/status');
      if (!response.ok) return;
      const status = await response.json();
      captureEnabled = status.capture_enabled;
      const button = document.getElementById('capture-toggle');
      button.textContent = captureEnabled ? 'Capturing' : 'Stopped';
      button.classList.toggle('running', captureEnabled);
      button.disabled = false;
    }
    async function toggleCapture() {
      const button = document.getElementById('capture-toggle');
      button.disabled = true;
      try {
        const endpoint = captureEnabled ? '/control/stop' : '/control/start';
        const response = await fetch(endpoint, { method: 'POST' });
        if (!response.ok) throw new Error(await response.text());
        await refreshCaptureStatus();
      } catch (error) {
        alert('Unable to change capture state: ' + error.message);
        button.disabled = false;
      }
    }
    function query() {
      const params = new URLSearchParams();
      for (const id of ['host', 'method', 'status']) {
        const value = document.getElementById(id).value.trim();
        if (value) params.set(id, value);
      }
      return params.toString() ? '?' + params.toString() : '';
    }
    async function loadSessions() {
      if (isLoadingSessions) return;
      isLoadingSessions = true;
      try {
        const response = await fetch('/sessions' + query());
        if (!response.ok) return;
        const loadedSessions = await response.json();
        newSessionIds = hasLoadedSessions
          ? new Set(loadedSessions.filter(session => !knownSessionIds.has(session.id)).map(session => session.id))
          : new Set();
        loadedSessions.forEach(session => knownSessionIds.add(session.id));
        hasLoadedSessions = true;
        sessions = loadedSessions;
        renderRows();
      } finally {
        isLoadingSessions = false;
      }
    }
    function renderRows() {
      const rows = document.getElementById('rows');
      const previousTops = new Map(
        [...rows.querySelectorAll('.request-row[data-session-id]')]
          .map(row => [row.dataset.sessionId, row.getBoundingClientRect().top])
      );
      const hideStatic = document.getElementById('hide-static').checked;
      const groupHost = document.getElementById('group-host').checked;
      const visible = sessions
        .map((session, index) => ({ session, index }))
        .reverse()
        .filter(item => !hideStatic || !isStatic(item.session));
      rows.innerHTML = groupHost ? renderTree(visible) : renderFlat(visible);
      animateMovedRows(rows, previousTops);
    }
    function renderFlat(visible) {
      return visible.map(({session: s, index: i}) => requestRow(s, i, true)).join('');
    }
    function renderTree(visible) {
      const domains = new Map();
      for (const item of visible) {
        const { host } = splitUrl(item.session.request.url);
        const domain = registrableDomain(host);
        if (!domains.has(domain)) domains.set(domain, new Map());
        const hosts = domains.get(domain);
        if (!hosts.has(host)) hosts.set(host, []);
        hosts.get(host).push(item);
      }
      return [...domains.entries()]
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([domain, hosts]) => {
          const domainKey = 'domain:' + domain;
          const domainItems = [...hosts.values()].flat();
          const domainClosed = collapsed.has(domainKey);
          const domainRow = treeRow('tree-root', domainKey, domain, domainItems.length, domainClosed);
          if (domainClosed) return domainRow;
          const hostRows = [...hosts.entries()]
            .sort(([a], [b]) => a.localeCompare(b))
            .map(([host, items]) => {
              const hostKey = 'host:' + host;
              const hostClosed = collapsed.has(hostKey);
              const label = host === domain ? '@ apex host' : host;
              const hostRow = treeRow('tree-host', hostKey, label, items.length, hostClosed);
              return hostClosed ? hostRow : hostRow + items.map(({session, index}) => requestRow(session, index, false)).join('');
            }).join('');
          return domainRow + hostRows;
        }).join('');
    }
    function treeRow(className, key, label, count, isClosed) {
      return `<tr class="${className}"><td colspan="5"><button class="tree-toggle" onclick="toggleTree('${escapeJs(key)}')">${isClosed ? '▸' : '▾'}</button>${escapeHtml(label)} <span class="tree-count">${count} requests</span></td></tr>`;
    }
    function requestRow(s, i, showHost) {
        const statusClass = 'status-' + String(s.response.status).charAt(0) + 'xx';
        const parsed = splitUrl(s.request.url);
        const selectedClass = s.id === selectedSessionId ? ' selected' : '';
        const newClass = newSessionIds.has(s.id) ? ' new-request' : '';
        return `<tr class="request-row${selectedClass}${newClass}" data-session-id="${escapeHtml(s.id)}" onclick="showSession(${i})">
          <td><span class="method-status"><span class="method">${escapeHtml(s.request.method)}</span><span class="${statusClass}">${s.response.status}</span></span></td>
          <td class="url-cell" title="${escapeHtml(s.request.url)}">${showHost ? `<span class="url-host">${escapeHtml(parsed.host)}</span>` : ''}<span class="url-path">${escapeHtml(parsed.path)}</span></td>
          <td class="type-cell" title="${escapeHtml(resourceType(s))}">${escapeHtml(compactType(s))}</td>
          <td class="size-cell">${formatBytes(s.response.body.size)}</td>
          <td>${formatDuration(s.duration_ms)}</td>
        </tr>`;
    }
    function toggleTree(key) {
      if (collapsed.has(key)) collapsed.delete(key); else collapsed.add(key);
      renderRows();
    }
    function animateMovedRows(rows, previousTops) {
      requestAnimationFrame(() => {
        const movedRows = [];
        rows.querySelectorAll('.request-row[data-session-id]').forEach(row => {
          const previousTop = previousTops.get(row.dataset.sessionId);
          if (previousTop === undefined) return;
          const offset = previousTop - row.getBoundingClientRect().top;
          if (Math.abs(offset) < 1) return;
          row.style.transition = 'none';
          row.style.transform = `translateY(${offset}px)`;
          movedRows.push(row);
        });
        if (movedRows.length === 0) return;
        requestAnimationFrame(() => {
          movedRows.forEach(row => {
            row.style.transition = 'transform 240ms ease-out';
            row.style.transform = '';
            window.setTimeout(() => { row.style.transition = ''; }, 250);
          });
        });
      });
    }
    function splitUrl(url) {
      try {
        const parsed = new URL(url);
        return { host: parsed.host, path: parsed.pathname + parsed.search };
      } catch (_) {
        return { host: '(unknown)', path: url };
      }
    }
    function registrableDomain(host) {
      const labels = host.toLowerCase().split('.').filter(Boolean);
      if (labels.length <= 2 || /^\d+\.\d+\.\d+\.\d+$/.test(host) || host.includes(':')) return host;
      const compoundSuffixes = new Set(['com.cn', 'net.cn', 'org.cn', 'gov.cn', 'edu.cn', 'co.uk', 'org.uk', 'ac.uk', 'com.au', 'net.au', 'org.au', 'co.jp']);
      const suffix = labels.slice(-2).join('.');
      return compoundSuffixes.has(suffix) && labels.length >= 3 ? labels.slice(-3).join('.') : labels.slice(-2).join('.');
    }
    function resourceType(session) {
      const mime = (session.response.mime_type || '').split(';')[0].toLowerCase();
      if (mime) return mime;
      const path = splitUrl(session.request.url).path.toLowerCase();
      if (/\.(png|jpg|jpeg|gif|svg|webp|ico)(\?|$)/.test(path)) return 'image';
      if (/\.css(\?|$)/.test(path)) return 'text/css';
      if (/\.js(\?|$)/.test(path)) return 'javascript';
      return 'other';
    }
    function compactType(session) {
      const type = resourceType(session).toLowerCase();
      if (type.includes('json')) return 'JSON';
      if (type.includes('html')) return 'HTML';
      if (type.includes('javascript')) return 'JS';
      if (type.includes('css')) return 'CSS';
      if (type.startsWith('image/') || type === 'image') return 'IMG';
      if (type.includes('font')) return 'FONT';
      if (type.startsWith('text/')) return 'TEXT';
      return '—';
    }
    function isStatic(session) {
      const type = resourceType(session);
      return type.startsWith('image/') || type.startsWith('video/') || type.startsWith('audio/') || type === 'application/octet-stream' || type === 'text/css' || type.includes('javascript') || type.includes('font') || type === 'text/html' && session.request.url.endsWith('/favicon.ico');
    }
    function formatBytes(size) {
      if (size < 1024 * 1024) return (size / 1024).toFixed(1) + 'K';
      return (size / 1024 / 1024).toFixed(1) + 'M';
    }
    function formatDuration(milliseconds) {
      if (milliseconds < 1000) return milliseconds + 'ms';
      if (milliseconds < 60 * 1000) return (milliseconds / 1000).toFixed(2) + 's';
      return (milliseconds / (60 * 1000)).toFixed(2) + 'm';
    }
    function showSession(index) {
      const s = sessions[index];
      selectedSessionId = s.id;
      selectedSession = s;
      renderRows();
      const statusClass = 'status-' + String(s.response.status).charAt(0) + 'xx';
      const parsedUrl = new URL(s.request.url);
      document.getElementById('detail').innerHTML = `
        <div class="detail-actions">
          <button onclick="copyUrl()">Copy URL</button>
          <button onclick="copyCurl()">Copy cURL</button>
          <button onclick="copyResponseBody()">Copy response</button>
          <button onclick="saveResponseBody()">Save response</button>
        </div>
        <div class="summary">
          <div class="summary-label">Method</div><div class="summary-value"><strong>${escapeHtml(s.request.method)}</strong></div>
          <div class="summary-label">Status</div><div class="summary-value ${statusClass}"><strong>${s.response.status}</strong></div>
          <div class="summary-label">Host / Path</div><div class="summary-value">${escapeHtml(parsedUrl.host + parsedUrl.pathname + parsedUrl.search)}</div>
          <div class="summary-label">MIME</div><div class="summary-value">${escapeHtml(s.response.mime_type || 'unknown')}</div>
          <div class="summary-label">Encoding</div><div class="summary-value">${escapeHtml(headerValue(s.response.headers, 'content-encoding') || 'identity')} ${headerValue(s.response.headers, 'content-encoding') ? '· decoded for display' : ''}</div>
          <div class="summary-label">Duration</div><div class="summary-value">${formatDuration(s.duration_ms)}</div>
          <div class="summary-label">Transfer size</div><div class="summary-value">${formatBytes(s.response.body.size)}</div>
          <div class="summary-label">Client</div><div class="summary-value">${escapeHtml(s.client)}</div>
        </div>
        <div class="tabs">
          <button class="tab ${activeTab === 'overview' ? 'active' : ''}" data-tab="overview" onclick="setTab('overview')">Overview</button>
          <button class="tab ${activeTab === 'params' ? 'active' : ''}" data-tab="params" onclick="setTab('params')">Params</button>
          <button class="tab ${activeTab === 'request-headers' ? 'active' : ''}" data-tab="request-headers" onclick="setTab('request-headers')">Request Headers</button>
          <button class="tab ${activeTab === 'request-body' ? 'active' : ''}" data-tab="request-body" onclick="setTab('request-body')">Request Body</button>
          <button class="tab ${activeTab === 'response-headers' ? 'active' : ''}" data-tab="response-headers" onclick="setTab('response-headers')">Response Headers</button>
          <button class="tab ${activeTab === 'response-body' ? 'active' : ''}" data-tab="response-body" onclick="setTab('response-body')">Response Body</button>
          <button class="tab ${activeTab === 'raw' ? 'active' : ''}" data-tab="raw" onclick="setTab('raw')">Raw JSON</button>
        </div>
        <div id="panel-overview" class="panel ${activeTab === 'overview' ? 'active' : ''}">
          <div class="summary">
            <div class="summary-label">Started</div><div class="summary-value">${escapeHtml(s.started_at)}</div>
            <div class="summary-label">Completed</div><div class="summary-value">${escapeHtml(s.completed_at)}</div>
            <div class="summary-label">Request size</div><div class="summary-value">${s.request.body.size} bytes</div>
            <div class="summary-label">Response size</div><div class="summary-value">${s.response.body.size} bytes</div>
          </div>
        </div>
        <div id="panel-params" class="panel ${activeTab === 'params' ? 'active' : ''}">${paramsTable(parsedUrl)}</div>
        <div id="panel-request-headers" class="panel ${activeTab === 'request-headers' ? 'active' : ''}">${headersTable(s.request.headers)}</div>
        <div id="panel-request-body" class="panel ${activeTab === 'request-body' ? 'active' : ''}">${bodyPanel(s.request.body, '', 'request')}</div>
        <div id="panel-response-headers" class="panel ${activeTab === 'response-headers' ? 'active' : ''}">${headersTable(s.response.headers)}</div>
        <div id="panel-response-body" class="panel ${activeTab === 'response-body' ? 'active' : ''}">${bodyPanel(s.response.body, s.response.mime_type || '', 'response')}</div>
        <div id="panel-raw" class="panel ${activeTab === 'raw' ? 'active' : ''}"><pre>${escapeHtml(JSON.stringify(s, null, 2))}</pre></div>`;
    }
    function setTab(name) {
      activeTab = name;
      document.querySelectorAll('.panel').forEach(panel => panel.classList.toggle('active', panel.id === 'panel-' + name));
      document.querySelectorAll('.tab').forEach(tab => tab.classList.toggle('active', tab.dataset.tab === name));
    }
    function headersTable(headers) {
      if (!headers || headers.length === 0) return '<p class="muted">No headers</p>';
      return '<table class="headers"><thead><tr><th>Name</th><th>Value</th></tr></thead><tbody>' +
        headers.map(h => `<tr><th>${escapeHtml(h.name)}</th><td>${escapeHtml(h.value)}</td></tr>`).join('') +
        '</tbody></table>';
    }
    function paramsTable(url) {
      const params = [...url.searchParams.entries()];
      if (!params.length) return '<p class="muted">No query parameters</p>';
      return '<table class="headers"><thead><tr><th>Name</th><th>Value</th></tr></thead><tbody>' +
        params.map(([name, value]) => `<tr><th>${escapeHtml(name)}</th><td>${escapeHtml(value)}</td></tr>`).join('') +
        '</tbody></table>';
    }
    function headerValue(headers, name) {
      const header = headers.find(header => header.name.toLowerCase() === name.toLowerCase());
      return header ? header.value : '';
    }
    async function copyUrl() {
      if (selectedSession) await copyText(selectedSession.request.url, 'URL copied');
    }
    async function copyCurl() {
      if (!selectedSession) return;
      const request = selectedSession.request;
      const parts = ['curl', '-X', shellQuote(request.method), shellQuote(request.url)];
      request.headers.forEach(header => parts.push('-H', shellQuote(header.name + ': ' + header.value)));
      if (request.body.encoding === 'utf8' && request.body.text) {
        parts.push('--data-raw', shellQuote(request.body.text));
      }
      await copyText(parts.join(' '), 'cURL copied');
    }
    async function copyResponseBody() {
      if (!selectedSession) return;
      const body = selectedSession.response.body;
      const content = body.encoding === 'utf8' ? (body.text || '') : (body.base64 || '');
      await copyText(content, body.encoding === 'utf8' ? 'Response copied' : 'Base64 response copied');
    }
    function saveResponseBody() {
      if (!selectedSession) return;
      const response = selectedSession.response;
      const body = response.body;
      const content = body.encoding === 'utf8' ? (body.text || '') : base64Bytes(body.base64 || '');
      const type = body.encoding === 'utf8' ? (response.mime_type || 'text/plain') : 'application/octet-stream';
      const extension = body.encoding === 'base64' ? 'bin' : (response.mime_type || '').includes('json') ? 'json' : 'txt';
      const link = document.createElement('a');
      link.href = URL.createObjectURL(new Blob([content], { type }));
      link.download = 'httphunter-response.' + extension;
      link.click();
      URL.revokeObjectURL(link.href);
      showNotice('Response saved');
    }
    async function copyText(value, message) {
      try {
        await navigator.clipboard.writeText(value);
        showNotice(message);
      } catch (_) {
        const input = document.createElement('textarea');
        input.value = value;
        document.body.appendChild(input);
        input.select();
        document.execCommand('copy');
        input.remove();
        showNotice(message);
      }
    }
    function shellQuote(value) {
      return "'" + String(value).replace(/'/g, "'\"'\"'") + "'";
    }
    function base64Bytes(base64) {
      const binary = atob(base64);
      return Uint8Array.from(binary, character => character.charCodeAt(0));
    }
    function showNotice(message) {
      const notice = document.getElementById('notice');
      notice.textContent = message;
      notice.classList.add('visible');
      window.clearTimeout(showNotice.timer);
      showNotice.timer = window.setTimeout(() => notice.classList.remove('visible'), 1400);
    }
    function bodyPanel(body, mime, kind) {
      if (!body || body.size === 0) return '<p class="muted">Empty body</p>';
      if (body.encoding === 'base64') {
        return `<details class="binary-body"><summary>Binary body · ${body.size} bytes · Base64 encoded</summary><pre class="body">${escapeHtml(body.base64 || '')}</pre></details>`;
      }
      const text = body.text || '';
      const isJson = (mime || '').toLowerCase().includes('json') || looksLikeJson(text);
      const pretty = isJson ? prettyJson(text) : text;
      const displayText = bodyModes[kind] === 'raw' ? text : pretty;
      bodyViews[kind] = { text: displayText, mime, isJson };
      return `<div class="body-tools">
        <input class="body-search" placeholder="Search body" oninput="searchBody('${kind}', this.value)">
        ${isJson ? `<button onclick="setBodyMode('${kind}', 'pretty')">Pretty</button><button onclick="setBodyMode('${kind}', 'raw')">Raw</button>` : ''}
        <span id="${kind}-search-count" class="search-count"></span>
      </div><pre id="${kind}-body-content" class="body">${highlightBody(displayText, mime, isJson)}</pre>`;
    }
    function prettyJson(text) {
      try { return JSON.stringify(JSON.parse(text), null, 2); }
      catch (_) { return text; }
    }
    function setBodyMode(kind, mode) {
      bodyModes[kind] = mode;
      if (selectedSession) showSession(sessions.findIndex(session => session.id === selectedSession.id));
    }
    function searchBody(kind, query) {
      const content = document.getElementById(kind + '-body-content');
      const count = document.getElementById(kind + '-search-count');
      const view = bodyViews[kind];
      if (!content || !view) return;
      const text = view.text;
      const normalized = query.trim();
      if (!normalized) { content.innerHTML = highlightBody(text, view.mime, view.isJson); count.textContent = ''; return; }
      const escaped = escapeHtml(text).replace(new RegExp(escapeRegExp(escapeHtml(normalized)), 'gi'), match => `<mark>${match}</mark>`);
      const matches = text.toLowerCase().split(normalized.toLowerCase()).length - 1;
      content.innerHTML = escaped;
      count.textContent = matches + ' match' + (matches === 1 ? '' : 'es');
    }
    function escapeRegExp(value) { return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'); }
    function highlightBody(text, mime, isJson) {
      if (isJson) return highlightJson(text);
      return escapeHtml(text);
    }
    function highlightJson(text) {
      const token = /"(?:\\.|[^"\\])*"|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|\b(?:true|false|null)\b/g;
      let result = '';
      let cursor = 0;
      for (const match of text.matchAll(token)) {
        const value = match[0];
        const index = match.index;
        result += escapeHtml(text.slice(cursor, index));
        const after = text.slice(index + value.length);
        const className = value.startsWith('"')
          ? (/^\s*:/.test(after) ? 'syntax-key' : 'syntax-string')
          : (/^(true|false|null)$/.test(value) ? 'syntax-literal' : 'syntax-number');
        result += `<span class="${className}">${escapeHtml(value)}</span>`;
        cursor = index + value.length;
      }
      return result + escapeHtml(text.slice(cursor));
    }
    function looksLikeJson(text) {
      const trimmed = text.trim();
      return (trimmed.startsWith('{') && trimmed.endsWith('}')) || (trimmed.startsWith('[') && trimmed.endsWith(']'));
    }
    async function clearSessions() {
      if (!confirm('Clear all in-memory sessions?')) return;
      await fetch('/sessions', { method: 'DELETE' });
      await loadSessions();
      document.getElementById('detail').innerHTML = '<p class="muted">Sessions cleared.</p>';
    }
    function escapeHtml(value) {
      return String(value).replace(/[&<>'"]/g, ch => ({'&':'&amp;','<':'&lt;','>':'&gt;',"'":'&#39;','"':'&quot;'}[ch]));
    }
    function escapeJs(value) {
      return String(value).replace(/\\/g, '\\\\').replace(/'/g, "\\'");
    }
    refreshCaptureStatus();
    loadSessions();
    setInterval(() => {
      if (captureEnabled) loadSessions();
    }, 2000);
  </script>
</body>
</html>"##,
    )
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

#[derive(Serialize)]
struct CaptureStatus {
    capture_enabled: bool,
    network_service: String,
}

async fn control_status(State(state): State<AppState>) -> Json<CaptureStatus> {
    Json(CaptureStatus {
        capture_enabled: state.store.is_enabled(),
        network_service: state.system_proxy.network_service().to_owned(),
    })
}

async fn start_capture(State(state): State<AppState>) -> Result<Json<CaptureStatus>, ApiError> {
    state
        .system_proxy
        .enable()
        .await
        .map_err(|error| ApiError::Operation(error.to_string()))?;
    state.store.set_enabled(true);
    tracing::info!(service = %state.system_proxy.network_service(), "capture started and system proxy enabled");
    Ok(Json(CaptureStatus {
        capture_enabled: true,
        network_service: state.system_proxy.network_service().to_owned(),
    }))
}

async fn stop_capture(State(state): State<AppState>) -> Result<Json<CaptureStatus>, ApiError> {
    state.store.set_enabled(false);
    state
        .system_proxy
        .disable()
        .await
        .map_err(|error| ApiError::Operation(error.to_string()))?;
    tracing::info!(service = %state.system_proxy.network_service(), "capture stopped and system proxy disabled");
    Ok(Json(CaptureStatus {
        capture_enabled: false,
        network_service: state.system_proxy.network_service().to_owned(),
    }))
}

async fn list_sessions(
    State(state): State<AppState>,
    Query(filter): Query<SessionFilter>,
) -> Json<Vec<ApiSession>> {
    let sessions = state
        .store
        .list()
        .await
        .into_iter()
        .filter(|session| filter.matches(session))
        .map(ApiSession::from)
        .collect();
    Json(sessions)
}

async fn clear_sessions(State(state): State<AppState>) -> StatusCode {
    state.store.clear().await;
    StatusCode::NO_CONTENT
}

async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiSession>, ApiError> {
    let id = Uuid::parse_str(&id).map_err(|_| ApiError::NotFound)?;
    state
        .store
        .get(id)
        .await
        .map(ApiSession::from)
        .map(Json)
        .ok_or(ApiError::NotFound)
}

#[derive(Debug, Serialize)]
struct ApiSession {
    id: Uuid,
    started_at: chrono::DateTime<chrono::Utc>,
    completed_at: chrono::DateTime<chrono::Utc>,
    client: SocketAddr,
    request: ApiRequest,
    response: ApiResponse,
    duration_ms: i64,
}

#[derive(Debug, Serialize)]
struct ApiRequest {
    method: String,
    url: String,
    headers: Vec<crate::capture::HeaderEntry>,
    body: BodyView,
}

#[derive(Debug, Serialize)]
struct ApiResponse {
    status: u16,
    headers: Vec<crate::capture::HeaderEntry>,
    mime_type: Option<String>,
    body: BodyView,
}

#[derive(Debug, Serialize)]
struct BodyView {
    size: usize,
    encoding: &'static str,
    text: Option<String>,
    base64: Option<String>,
}

impl From<HttpSession> for ApiSession {
    fn from(session: HttpSession) -> Self {
        let response_body =
            BodyView::from_response_bytes(&session.response.body, &session.response.headers);
        Self {
            id: session.id,
            started_at: session.started_at,
            completed_at: session.completed_at,
            client: session.client,
            request: ApiRequest {
                method: session.request.method,
                url: session.request.url,
                headers: session.request.headers,
                body: BodyView::from_bytes(&session.request.body),
            },
            response: ApiResponse {
                status: session.response.status,
                headers: session.response.headers,
                mime_type: session.response.mime_type,
                body: response_body,
            },
            duration_ms: session.duration_ms,
        }
    }
}

impl BodyView {
    fn from_bytes(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(text) => Self {
                size: bytes.len(),
                encoding: "utf8",
                text: Some(text.to_owned()),
                base64: None,
            },
            Err(_) => Self {
                size: bytes.len(),
                encoding: "base64",
                text: None,
                base64: Some(STANDARD.encode(bytes)),
            },
        }
    }

    fn from_response_bytes(bytes: &[u8], headers: &[crate::capture::HeaderEntry]) -> Self {
        let displayed_bytes = if has_gzip_content_encoding(headers) {
            decompress_gzip(bytes).unwrap_or_else(|| bytes.to_vec())
        } else {
            bytes.to_vec()
        };
        let mut body = Self::from_bytes(&displayed_bytes);
        // Keep the captured transfer size visible in the request list and overview.
        body.size = bytes.len();
        body
    }
}

fn has_gzip_content_encoding(headers: &[crate::capture::HeaderEntry]) -> bool {
    headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("content-encoding")
            && header
                .value
                .split(',')
                .any(|encoding| encoding.trim().eq_ignore_ascii_case("gzip"))
    })
}

fn decompress_gzip(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = GzDecoder::new(bytes);
    let mut decompressed = Vec::new();
    decoder.read_to_end(&mut decompressed).ok()?;
    Some(decompressed)
}

async fn export_har(State(state): State<AppState>) -> Json<Value> {
    let entries = state
        .store
        .list()
        .await
        .into_iter()
        .map(session_to_har)
        .collect::<Vec<_>>();
    Json(json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "httphunter", "version": env!("CARGO_PKG_VERSION")},
            "entries": entries
        }
    }))
}

fn session_to_har(session: HttpSession) -> Value {
    let request_headers = session
        .request
        .headers
        .iter()
        .map(|header| json!({"name": header.name.clone(), "value": header.value.clone()}))
        .collect::<Vec<_>>();
    let response_headers = session
        .response
        .headers
        .iter()
        .map(|header| json!({"name": header.name.clone(), "value": header.value.clone()}))
        .collect::<Vec<_>>();
    let request_body = String::from_utf8_lossy(&session.request.body).into_owned();
    let response_body = String::from_utf8_lossy(&session.response.body).into_owned();
    let method = session.request.method.clone();
    let url = session.request.url.clone();
    let request_body_size = session.request.body.len();
    let response_status = session.response.status;
    let response_body_size = session.response.body.len();
    let duration_ms = session.duration_ms;
    let response_mime_type = session.response.mime_type.clone().unwrap_or_default();

    json!({
        "startedDateTime": session.started_at.to_rfc3339(),
        "time": duration_ms,
        "request": {
            "method": method,
            "url": url,
            "httpVersion": "HTTP/1.1",
            "headers": request_headers,
            "queryString": [],
            "cookies": [],
            "headersSize": -1,
            "bodySize": request_body_size,
            "postData": {"mimeType": "", "text": request_body}
        },
        "response": {
            "status": response_status,
            "statusText": "",
            "httpVersion": "HTTP/1.1",
            "headers": response_headers,
            "cookies": [],
            "content": {
                "size": response_body_size,
                "mimeType": response_mime_type,
                "text": response_body
            },
            "redirectURL": "",
            "headersSize": -1,
            "bodySize": response_body_size
        },
        "cache": {},
        "timings": {"send": 0, "wait": duration_ms, "receive": 0}
    })
}

#[derive(Debug)]
enum ApiError {
    NotFound,
    Operation(String),
}

#[derive(Debug, Default, Deserialize)]
struct SessionFilter {
    host: Option<String>,
    method: Option<String>,
    status: Option<u16>,
    url: Option<String>,
}

impl SessionFilter {
    fn matches(&self, session: &HttpSession) -> bool {
        if let Some(host) = &self.host {
            if !session
                .request
                .url
                .to_ascii_lowercase()
                .contains(&host.to_ascii_lowercase())
            {
                return false;
            }
        }
        if let Some(method) = &self.method {
            if !session.request.method.eq_ignore_ascii_case(method) {
                return false;
            }
        }
        if let Some(status) = self.status {
            if session.response.status != status {
                return false;
            }
        }
        if let Some(url) = &self.url {
            if !session.request.url.contains(url) {
                return false;
            }
        }
        true
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "session not found").into_response(),
            Self::Operation(message) => {
                (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
            }
        }
    }
}
