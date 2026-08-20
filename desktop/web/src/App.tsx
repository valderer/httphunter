import { type ReactNode, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

type CertificateInfo = {
  path: string;
  exists: boolean;
};

type CaptureRuntimeStatus = {
  running: boolean;
  listen: string;
  mitm_enabled: boolean;
  capture_enabled: boolean;
};

type SystemProxyStatus = {
  supported: boolean;
  enabled: boolean;
  network_service: string;
};

type MobileCaptureStatus = {
  enabled: boolean;
  listen: string;
  lan_addresses: string[];
};

type HeaderEntry = {
  name: string;
  value: string;
};

type EditableRequest = {
  method: string;
  url: string;
  headers: HeaderEntry[];
  body: number[];
};

type PendingIntercept = { id: string; request: EditableRequest };

type MockRule = {
  id: string;
  enabled: boolean;
  method: string;
  url_pattern: string;
  status: number;
  headers: HeaderEntry[];
  body: number[];
};

type ReplayResult = { session: HttpSession; error: string | null };

type SessionSummary = {
  id: string;
  started_at: string;
  method: string;
  url: string;
  host: string;
  path: string;
  status: number;
  mime_type: string | null;
  response_size: number;
  duration_ms: number;
};

type HttpSession = {
  id: string;
  started_at: string;
  completed_at: string;
  client: string;
  request: {
    method: string;
    url: string;
    headers: HeaderEntry[];
    body: number[];
  };
  response: {
    status: number;
    mime_type: string | null;
    headers: HeaderEntry[];
    body: number[];
  };
  duration_ms: number;
};

type DetailTab = 'overview' | 'request-headers' | 'request-body' | 'response-headers' | 'response-body' | 'raw';

type SessionFilters = {
  host: string;
  method: string;
  status: string;
  hideStatic: boolean;
};

const emptyFilters: SessionFilters = { host: '', method: '', status: '', hideStatic: false };
const filtersStorageKey = 'httphunter.session-filters';

function loadStoredFilters(): SessionFilters {
  try {
    const stored = JSON.parse(localStorage.getItem(filtersStorageKey) ?? '{}') as Partial<SessionFilters>;
    return {
      host: typeof stored.host === 'string' ? stored.host : '',
      method: typeof stored.method === 'string' ? stored.method : '',
      status: typeof stored.status === 'string' ? stored.status : '',
      hideStatic: stored.hideStatic === true,
    };
  } catch {
    return emptyFilters;
  }
}

function formatSize(size: number) {
  if (size < 1024) return `${size} B`;
  return `${(size / 1024).toFixed(size >= 10 * 1024 ? 0 : 1)}k`;
}

function formatListSize(size: number) {
  if (size === 0) return '0K';
  return `${(size / 1024).toFixed(size >= 10 * 1024 ? 0 : 1)}K`;
}

function formatDuration(duration: number) {
  if (duration < 1000) return `${duration} ms`;
  if (duration < 60_000) return `${(duration / 1000).toFixed(1)} s`;
  return `${(duration / 60_000).toFixed(1)} min`;
}

function resourceType(mimeType: string | null) {
  if (!mimeType) return '-';
  return mimeType.split(';', 1)[0].replace('application/', '').replace('text/', '');
}

function statusClass(status: number) {
  return `status-${Math.floor(status / 100)}xx`;
}

function isStaticResource(session: SessionSummary) {
  const mimeType = session.mime_type?.split(';', 1)[0].toLowerCase() ?? '';
  if (mimeType.startsWith('image/') || mimeType.startsWith('video/') || mimeType.startsWith('audio/')) return true;
  if (mimeType === 'application/octet-stream' || mimeType === 'text/css' || mimeType.includes('javascript') || mimeType.includes('font')) return true;
  return /\.(avif|css|eot|gif|ico|jpe?g|js|m4a|mp3|mp4|ogg|otf|png|svg|ttf|wav|webm|webp|woff2?)(\?|$)/i.test(session.path);
}

export function App() {
  const [capture, setCapture] = useState<CaptureRuntimeStatus | null>(null);
  const [systemProxy, setSystemProxy] = useState<SystemProxyStatus | null>(null);
  const [mobileCapture, setMobileCapture] = useState<MobileCaptureStatus | null>(null);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [selected, setSelected] = useState<HttpSession | null>(null);
  const [certificate, setCertificate] = useState<CertificateInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [changingCapture, setChangingCapture] = useState(false);
  const [generatingCertificate, setGeneratingCertificate] = useState(false);
  const [detailTab, setDetailTab] = useState<DetailTab>('overview');
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [mobileCaptureOpen, setMobileCaptureOpen] = useState(false);
  const [changingMobileCapture, setChangingMobileCapture] = useState(false);
  const [interceptEnabled, setInterceptEnabled] = useState(false);
  const [pendingIntercepts, setPendingIntercepts] = useState<PendingIntercept[]>([]);
  const [mockRules, setMockRules] = useState<MockRule[]>([]);
  const [mockOpen, setMockOpen] = useState(false);
  const [mockDraft, setMockDraft] = useState<MockRule | null>(null);
  const [importingMock, setImportingMock] = useState(false);
  const [editor, setEditor] = useState<{ kind: 'replay' | 'intercept'; id?: string; request: EditableRequest } | null>(null);
  const [replaying, setReplaying] = useState(false);
  const [filters, setFilters] = useState<SessionFilters>(loadStoredFilters);
  const [draftFilters, setDraftFilters] = useState<SessionFilters>(loadStoredFilters);

  const visibleSessions = sessions.filter((session) => {
    const host = filters.host.trim().toLowerCase();
    const method = filters.method.trim().toUpperCase();
    const status = filters.status.trim();
    return (!host || session.host.toLowerCase().includes(host))
      && (!method || session.method.toUpperCase().includes(method))
      && (!status || String(session.status).startsWith(status))
      && (!filters.hideStatic || !isStaticResource(session));
  });
  const activeFilterCount = Number(Boolean(filters.host.trim())) + Number(Boolean(filters.method.trim())) + Number(Boolean(filters.status.trim())) + Number(filters.hideStatic);

  useEffect(() => {
    void refreshCapture();
    invoke<CertificateInfo>('certificate_info').then(setCertificate).catch(showError);
  }, []);

  useEffect(() => {
    if (!capture?.running) return undefined;
    const timer = window.setInterval(() => void refreshCapture(), 1000);
    return () => window.clearInterval(timer);
  }, [capture?.running]);

  useEffect(() => {
    localStorage.setItem(filtersStorageKey, JSON.stringify(filters));
  }, [filters]);

  function showError(value: unknown) {
    setError(String(value));
  }

  async function refreshCapture() {
    try {
      const [status, summaries, proxyStatus, mobileStatus, intercept, pending, rules] = await Promise.all([
        invoke<CaptureRuntimeStatus>('capture_status'),
        invoke<SessionSummary[]>('list_sessions'),
        invoke<SystemProxyStatus>('system_proxy_status'),
        invoke<MobileCaptureStatus>('mobile_capture_status'),
        invoke<boolean>('intercept_enabled'),
        invoke<PendingIntercept[]>('pending_intercepts'),
        invoke<MockRule[]>('list_mock_rules'),
      ]);
      setCapture(status);
      setSessions([...summaries].reverse());
      setSystemProxy(proxyStatus);
      setMobileCapture(mobileStatus);
      setInterceptEnabled(intercept);
      setPendingIntercepts(pending);
      setMockRules(rules);
    } catch (value) {
      showError(value);
    }
  }

  async function changeCapture(command: 'start_capture' | 'stop_capture') {
    setChangingCapture(true);
    setError(null);
    try {
      setCapture(await invoke<CaptureRuntimeStatus>(command));
      await refreshCapture();
    } catch (value) {
      showError(value);
    } finally {
      setChangingCapture(false);
      void refreshCapture();
    }
  }

  async function selectSession(id: string) {
    try {
      const session = await invoke<HttpSession | null>('get_session', { id });
      setSelected(session);
    } catch (value) {
      showError(value);
    }
  }

  async function clearSessions() {
    try {
      await invoke('clear_sessions');
      setSessions([]);
      setSelected(null);
    } catch (value) {
      showError(value);
    }
  }

  function openFilters() {
    setDraftFilters(filters);
    setFiltersOpen(true);
  }

  function applyFilters() {
    setFilters(draftFilters);
    setFiltersOpen(false);
  }

  function resetFilters() {
    setDraftFilters(emptyFilters);
    setFilters(emptyFilters);
  }

  async function generateCertificate() {
    setGeneratingCertificate(true);
    setError(null);
    try {
      setCertificate(await invoke<CertificateInfo>('generate_certificate'));
    } catch (value) {
      showError(value);
    } finally {
      setGeneratingCertificate(false);
    }
  }

  async function changeMobileCapture(enabled: boolean) {
    setChangingMobileCapture(true);
    setError(null);
    try {
      setMobileCapture(await invoke<MobileCaptureStatus>('set_mobile_capture', { enabled }));
      await refreshCapture();
    } catch (value) {
      showError(value);
    } finally {
      setChangingMobileCapture(false);
      void refreshCapture();
    }
  }

  async function toggleIntercept() {
    try {
      const enabled = !interceptEnabled;
      await invoke('set_intercept_enabled', { enabled });
      setInterceptEnabled(enabled);
    } catch (value) { showError(value); }
  }

  async function saveMock(rule: MockRule) {
    try {
      await invoke<MockRule>('save_mock_rule', { rule });
      setMockRules(await invoke<MockRule[]>('list_mock_rules'));
    } catch (value) { showError(value); }
  }

  async function importSelectedAsMock() {
    if (!selected) return;
    setImportingMock(true);
    setError(null);
    try {
      const rule = await invoke<MockRule>('save_mock_rule', {
        rule: {
          id: '',
          enabled: true,
          method: selected.request.method,
          url_pattern: selected.request.url,
          status: selected.response.status,
          headers: selected.response.headers,
          body: selected.response.body,
        },
      });
      setMockRules((rules) => [...rules, rule]);
      setMockDraft(rule);
      setMockOpen(true);
    } catch (value) {
      showError(value);
    } finally {
      setImportingMock(false);
    }
  }

  async function removeMock(id: string) {
    try {
      await invoke('delete_mock_rule', { id });
      setMockRules(await invoke<MockRule[]>('list_mock_rules'));
    } catch (value) { showError(value); }
  }

  async function submitEditor(action: 'forward' | 'drop' | 'replay', request: EditableRequest) {
    setReplaying(true);
    try {
      if (action === 'replay') {
        const result = await invoke<ReplayResult>('replay_request', { request });
        setSelected(result.session);
        setEditor(null);
        if (result.error) showError(result.error);
      } else if (editor?.id) {
        await invoke('resolve_intercept', {
          id: editor.id,
          resolution: { action: action === 'forward' ? 'Forward' : 'Drop', request },
        });
        setEditor(null);
      }
      await refreshCapture();
    } catch (value) { showError(value); } finally { setReplaying(false); }
  }

  return (
    <main className="capture-app">
      <header className="app-header">
        <div className="brand"><h1>httphunter</h1><span>Capture console</span><span className={`system-proxy ${systemProxy?.enabled ? 'enabled' : ''}`}>Proxy {systemProxy?.enabled ? 'on' : 'off'}</span></div>
        <div className="header-actions">
          <button className={interceptEnabled ? 'intercept-active' : ''} onClick={() => void toggleIntercept()} title="Pause matching decrypted requests before forwarding">Intercept{pendingIntercepts.length ? ` (${pendingIntercepts.length})` : ''}</button>
          <button className={mockRules.some((rule) => rule.enabled) ? 'mock-active' : ''} onClick={() => { setMockDraft(null); setMockOpen(true); }} title="Configure mocked responses">Mock{mockRules.length ? ` (${mockRules.length})` : ''}</button>
          <button
            className={`capture-toggle ${capture?.running ? 'running' : 'stopped'}`}
            disabled={changingCapture || certificate?.exists === false}
            onClick={() => void changeCapture(capture?.running ? 'stop_capture' : 'start_capture')}
            title={capture?.running ? 'Stop capture and disable the system proxy' : 'Start capture and enable the system proxy'}
          >
            <i /> {changingCapture ? 'Working...' : capture?.running ? 'Capturing' : 'Stopped'}
          </button>
          <button className={mobileCapture?.enabled ? 'mobile-capture-active' : ''} onClick={() => setMobileCaptureOpen(true)} title="Configure capture from phones and other Wi-Fi devices">Mobile</button>
        </div>
      </header>

      {!certificate?.exists && (
        <section className="certificate-notice">
          <span>Local CA is required for HTTPS inspection.</span>
          <button disabled={generatingCertificate} onClick={() => void generateCertificate()}>
            {generatingCertificate ? 'Generating...' : 'Generate local CA'}
          </button>
        </section>
      )}

      {error && <section className="error-notice"><span>{error}</span><button onClick={() => setError(null)}>Dismiss</button></section>}

      <section className="capture-layout">
        <aside className="session-pane">
          <div className="session-toolbar">
            <span>{activeFilterCount ? `${visibleSessions.length} / ${sessions.length} requests` : `${sessions.length} requests`}</span>
            <span className="toolbar-spacer" />
            <button onClick={openFilters} className={activeFilterCount ? 'filters-active' : ''}>Filters{activeFilterCount ? ` (${activeFilterCount})` : ''}</button>
            <button onClick={() => void clearSessions()} disabled={sessions.length === 0}>Clear</button>
          </div>
          <div className="session-table-wrap">
            <table className="session-table">
              <thead><tr><th>Req</th><th>Host / Path</th><th>Type</th><th>Size</th></tr></thead>
              <tbody>
                {visibleSessions.map((session) => (
                  <tr
                    key={session.id}
                    className={selected?.id === session.id ? 'selected' : ''}
                    onClick={() => void selectSession(session.id)}
                  >
                    <td><div className="method-status"><b>{session.method}</b><span className={statusClass(session.status)}>{session.status}</span></div></td>
                    <td className="url-cell"><b>{session.host || session.url}</b><span>{session.path}</span></td>
                    <td title={session.mime_type ?? undefined}>{resourceType(session.mime_type)}</td>
                    <td>{formatListSize(session.response_size)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            {!sessions.length && <p className="empty-list">Start capturing to see requests.</p>}
            {!!sessions.length && !visibleSessions.length && <p className="empty-list">No requests match the active filters.</p>}
          </div>
        </aside>

        <section className="detail-pane">
          {selected ? (
            <>
              <div className="detail-heading">
                <div className="detail-meta"><b>{selected.request.method}</b><span className={statusClass(selected.response.status)}>{selected.response.status}</span><button className="replay-button" onClick={() => setEditor({ kind: 'replay', request: selected.request })}>Replay</button><button className="mock-import-button" disabled={importingMock} onClick={() => void importSelectedAsMock()} title="Create an enabled Mock rule from this request and response">{importingMock ? 'Importing...' : 'Import Mock'}</button></div>
                <h2>{selected.request.url}</h2>
              </div>
              <nav className="detail-tabs" aria-label="Request details">
                <DetailTabButton active={detailTab === 'overview'} onClick={() => setDetailTab('overview')}>Overview</DetailTabButton>
                <DetailTabButton active={detailTab === 'request-headers'} onClick={() => setDetailTab('request-headers')} title="Request Headers">Req Headers</DetailTabButton>
                <DetailTabButton active={detailTab === 'request-body'} onClick={() => setDetailTab('request-body')} title="Request Body">Req Body</DetailTabButton>
                <DetailTabButton active={detailTab === 'response-headers'} onClick={() => setDetailTab('response-headers')} title="Response Headers">Res Headers</DetailTabButton>
                <DetailTabButton active={detailTab === 'response-body'} onClick={() => setDetailTab('response-body')} title="Response Body">Res Body</DetailTabButton>
                <DetailTabButton active={detailTab === 'raw'} onClick={() => setDetailTab('raw')}>Raw JSON</DetailTabButton>
              </nav>
              <DetailPanel tab={detailTab} session={selected} />
            </>
          ) : (
            <div className="empty-detail"><p>Select a request to inspect it.</p><span>Proxy: {capture?.listen ?? '127.0.0.1:8080'} · HTTPS MITM {capture?.mitm_enabled ? 'enabled' : 'disabled'} · System proxy {systemProxy?.enabled ? 'on' : 'off'}</span></div>
          )}
        </section>
      </section>

      {filtersOpen && (
        <div className="filter-backdrop" role="presentation" onMouseDown={() => setFiltersOpen(false)}>
          <section className="filter-dialog" role="dialog" aria-modal="true" aria-labelledby="filters-title" onMouseDown={(event) => event.stopPropagation()}>
            <header className="filter-dialog-header"><h2 id="filters-title">Filters</h2><button onClick={() => setFiltersOpen(false)} title="Close filters">Close</button></header>
            <div className="filter-dialog-body">
              <FilterField label="Host"><input value={draftFilters.host} onChange={(event) => setDraftFilters({ ...draftFilters, host: event.target.value })} placeholder="example.com" autoFocus /></FilterField>
              <FilterField label="Method"><input value={draftFilters.method} onChange={(event) => setDraftFilters({ ...draftFilters, method: event.target.value })} placeholder="GET" /></FilterField>
              <FilterField label="Status"><input value={draftFilters.status} onChange={(event) => setDraftFilters({ ...draftFilters, status: event.target.value.replace(/\D/g, '') })} inputMode="numeric" placeholder="200" /></FilterField>
              <label className="filter-checkbox"><input type="checkbox" checked={draftFilters.hideStatic} onChange={(event) => setDraftFilters({ ...draftFilters, hideStatic: event.target.checked })} />Hide static resources</label>
            </div>
            <footer className="filter-dialog-footer"><button onClick={resetFilters}>Reset</button><button className="filter-apply" onClick={applyFilters}>Apply</button></footer>
          </section>
        </div>
      )}

      {mobileCaptureOpen && (
        <div className="filter-backdrop" role="presentation" onMouseDown={() => setMobileCaptureOpen(false)}>
          <section className="mobile-dialog" role="dialog" aria-modal="true" aria-labelledby="mobile-capture-title" onMouseDown={(event) => event.stopPropagation()}>
            <header className="filter-dialog-header"><h2 id="mobile-capture-title">Mobile capture</h2><button onClick={() => setMobileCaptureOpen(false)} title="Close mobile capture settings">Close</button></header>
            <div className="mobile-dialog-body">
              <label className="mobile-switch">
                <input
                  type="checkbox"
                  checked={mobileCapture?.enabled ?? false}
                  disabled={changingMobileCapture}
                  onChange={(event) => void changeMobileCapture(event.target.checked)}
                />
                <span>Allow Wi-Fi devices</span>
              </label>
              {mobileCapture?.enabled && (
                <>
                  <div className="mobile-proxy-values">
                    <span>Server</span><b>{mobileCapture.lan_addresses[0] ?? 'Not detected'}</b>
                    <span>Port</span><b>8080</b>
                  </div>
                  {mobileCapture.lan_addresses.length > 1 && <p className="mobile-note">Available addresses: {mobileCapture.lan_addresses.join(', ')}</p>}
                  <p className="mobile-note">Set this server and port as the phone Wi-Fi manual proxy. HTTPS inspection also requires trusting the Local CA on that phone.</p>
                  <p className="mobile-note">Only private-network clients are accepted. This does not change the Mac system proxy.</p>
                </>
              )}
              {!mobileCapture?.enabled && <p className="mobile-note">When enabled, httphunter listens on this computer's Wi-Fi network so a phone can use it as a proxy.</p>}
            </div>
          </section>
        </div>
      )}

      {pendingIntercepts.length > 0 && !editor && (
        <InterceptQueue pending={pendingIntercepts} onOpen={(pending) => setEditor({ kind: 'intercept', id: pending.id, request: pending.request })} />
      )}

      {editor && <RequestEditor editor={editor} busy={replaying} onClose={() => setEditor(null)} onSubmit={submitEditor} />}
      {mockOpen && <MockDialog rules={mockRules} initialRule={mockDraft} onClose={() => { setMockOpen(false); setMockDraft(null); }} onSave={saveMock} onDelete={removeMock} />}
    </main>
  );
}

function FilterField({ label, children }: { label: string; children: ReactNode }) {
  return <label className="filter-field"><span>{label}</span>{children}</label>;
}

function InterceptQueue({ pending, onOpen }: { pending: PendingIntercept[]; onOpen: (pending: PendingIntercept) => void }) {
  return (
    <div className="intercept-queue">
      <span>{pending.length} request{pending.length === 1 ? '' : 's'} paused</span>
      <button onClick={() => onOpen(pending[0])}>Review</button>
    </div>
  );
}

function RequestEditor({ editor, busy, onClose, onSubmit }: {
  editor: { kind: 'replay' | 'intercept'; id?: string; request: EditableRequest };
  busy: boolean;
  onClose: () => void;
  onSubmit: (action: 'forward' | 'drop' | 'replay', request: EditableRequest) => void;
}) {
  const [request, setRequest] = useState(editor.request);
  const [headersText, setHeadersText] = useState(headersToText(editor.request.headers));
  const [bodyText, setBodyText] = useState(decodeUtf8(editor.request.body) ?? '');
  const update = () => ({ ...request, headers: textToHeaders(headersText), body: Array.from(new TextEncoder().encode(bodyText)) });
  const title = editor.kind === 'replay' ? 'Replay request' : 'Intercepted request';
  return (
    <div className="filter-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="request-editor" role="dialog" aria-modal="true" aria-labelledby="request-editor-title" onMouseDown={(event) => event.stopPropagation()}>
        <header className="filter-dialog-header"><h2 id="request-editor-title">{title}</h2><button onClick={onClose}>Close</button></header>
        <div className="request-editor-body">
          <div className="editor-method-url"><input value={request.method} onChange={(event) => setRequest({ ...request, method: event.target.value.toUpperCase() })} aria-label="Method" /><input value={request.url} onChange={(event) => setRequest({ ...request, url: event.target.value })} aria-label="URL" /></div>
          <FilterField label="Headers"><textarea value={headersText} onChange={(event) => setHeadersText(event.target.value)} spellCheck={false} placeholder="content-type: application/json" /></FilterField>
          <FilterField label="Body"><textarea className="request-body-editor" value={bodyText} onChange={(event) => setBodyText(event.target.value)} spellCheck={false} /></FilterField>
        </div>
        <footer className="filter-dialog-footer">
          {editor.kind === 'intercept' && <button className="drop-button" disabled={busy} onClick={() => onSubmit('drop', update())}>Drop</button>}
          <button className="filter-apply" disabled={busy} onClick={() => onSubmit(editor.kind === 'replay' ? 'replay' : 'forward', update())}>{busy ? 'Sending...' : editor.kind === 'replay' ? 'Send' : 'Forward'}</button>
        </footer>
      </section>
    </div>
  );
}

function MockDialog({ rules, initialRule, onClose, onSave, onDelete }: { rules: MockRule[]; initialRule: MockRule | null; onClose: () => void; onSave: (rule: MockRule) => void; onDelete: (id: string) => void }) {
  const blank = (): MockRule => ({ id: '', enabled: true, method: '', url_pattern: '', status: 200, headers: [{ name: 'content-type', value: 'application/json; charset=utf-8' }], body: [] });
  const [draft, setDraft] = useState<MockRule>(blank);
  const [headersText, setHeadersText] = useState(headersToText(draft.headers));
  const [bodyText, setBodyText] = useState('');
  const edit = (rule: MockRule) => { setDraft(rule); setHeadersText(headersToText(rule.headers)); setBodyText(decodeUtf8(rule.body) ?? ''); };
  useEffect(() => { if (initialRule) edit(initialRule); }, [initialRule?.id]);
  const save = () => { onSave({ ...draft, headers: textToHeaders(headersText), body: Array.from(new TextEncoder().encode(bodyText)) }); setDraft(blank()); setHeadersText(headersToText(blank().headers)); setBodyText(''); };
  return (
    <div className="filter-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="mock-dialog" role="dialog" aria-modal="true" aria-labelledby="mock-title" onMouseDown={(event) => event.stopPropagation()}>
        <header className="filter-dialog-header"><h2 id="mock-title">Mock responses</h2><button onClick={onClose}>Close</button></header>
        <div className="mock-dialog-body">
          <div className="mock-rule-list">
            {rules.map((rule) => <div className="mock-rule" key={rule.id}><button onClick={() => edit(rule)}>{rule.enabled ? 'On' : 'Off'} · {rule.method || 'Any'} · {rule.url_pattern}</button><button className="remove-button" onClick={() => onDelete(rule.id)}>Remove</button></div>)}
            {!rules.length && <p className="mobile-note">No rules yet.</p>}
          </div>
          <label className="filter-checkbox"><input type="checkbox" checked={draft.enabled} onChange={(event) => setDraft({ ...draft, enabled: event.target.checked })} />Enabled</label>
          <div className="mock-match"><input value={draft.method} placeholder="Method (any)" onChange={(event) => setDraft({ ...draft, method: event.target.value.toUpperCase() })} /><input value={draft.url_pattern} placeholder="URL contains, e.g. /api/user" onChange={(event) => setDraft({ ...draft, url_pattern: event.target.value })} /><input value={draft.status} type="number" min="100" max="599" onChange={(event) => setDraft({ ...draft, status: Number(event.target.value) || 200 })} /></div>
          <FilterField label="Response headers"><textarea value={headersText} onChange={(event) => setHeadersText(event.target.value)} placeholder={'content-type: application/json; charset=utf-8\ncontent-encoding: gzip (optional)'} spellCheck={false} /></FilterField>
          <FilterField label="Response body"><textarea className="request-body-editor" value={bodyText} onChange={(event) => setBodyText(event.target.value)} spellCheck={false} /></FilterField>
        </div>
        <footer className="filter-dialog-footer"><button onClick={() => { setDraft(blank()); setHeadersText(headersToText(blank().headers)); setBodyText(''); }}>New</button><button className="filter-apply" onClick={save} disabled={!draft.url_pattern.trim()}>Save rule</button></footer>
      </section>
    </div>
  );
}

function headersToText(headers: HeaderEntry[]) { return headers.map((header) => `${header.name}: ${header.value}`).join('\n'); }

function textToHeaders(value: string) {
  return value.split('\n').flatMap((line) => {
    const separator = line.indexOf(':');
    if (separator <= 0) return [];
    return [{ name: line.slice(0, separator).trim(), value: line.slice(separator + 1).trim() }];
  });
}

function DetailTabButton({ active, onClick, title, children }: { active: boolean; onClick: () => void; title?: string; children: string }) {
  return <button className={`detail-tab ${active ? 'active' : ''}`} aria-selected={active} onClick={onClick} title={title}>{children}</button>;
}

function DetailPanel({ tab, session }: { tab: DetailTab; session: HttpSession }) {
  if (tab === 'request-headers') return <HeaderTable headers={session.request.headers} />;
  if (tab === 'response-headers') return <HeaderTable headers={session.response.headers} />;
  if (tab === 'request-body') return <BodyPanel body={session.request.body} mimeType={headerValue(session.request.headers, 'content-type')} />;
  if (tab === 'response-body') return <BodyPanel body={session.response.body} mimeType={session.response.mime_type} />;
  if (tab === 'raw') return <pre className="raw-panel">{JSON.stringify(session, null, 2)}</pre>;
  const queryParams = [...new URL(session.request.url).searchParams.entries()];
  const paramsValue = queryParams.length
    ? <span className="overview-params">{queryParams.map(([name, value], index) => <span key={`${name}-${index}`}>{name}={value}</span>)}</span>
    : '-';
  const requestContentType = headerValue(session.request.headers, 'content-type') ?? '-';
  const origin = headerValue(session.request.headers, 'origin');
  const referer = headerValue(session.request.headers, 'referer') ?? headerValue(session.request.headers, 'referrer');
  const responseContentType = headerValue(session.response.headers, 'content-type') ?? session.response.mime_type ?? '-';
  const responseCharset = contentCharset(responseContentType);
  const responseEncoding = headerValue(session.response.headers, 'content-encoding');
  const redirectTo = headerValue(session.response.headers, 'location');
  const cacheStatus = headerValue(session.response.headers, 'cf-cache-status')
    ?? headerValue(session.response.headers, 'x-cache')
    ?? headerValue(session.response.headers, 'cache-control');
  const server = headerValue(session.response.headers, 'server');
  const setCookieCount = session.response.headers.filter((header) => header.name.toLowerCase() === 'set-cookie').length;
  return (
    <div className="tab-overview">
      <OverviewSection title="Request">
        <OverviewRow label="Client" value={session.client} />
        <OverviewRow label="Started" value={new Date(session.started_at).toLocaleString()} />
        <OverviewRow label="Content type" value={requestContentType} />
        {origin && <OverviewRow label="Origin" value={origin} />}
        {referer && <OverviewRow label="Referer" value={referer} />}
        <OverviewRow label="Size" value={formatSize(session.request.body.length)} />
        <OverviewRow label="Params" value={paramsValue} />
      </OverviewSection>
      <OverviewSection title="Response">
        <OverviewRow label="Completed" value={new Date(session.completed_at).toLocaleString()} />
        <OverviewRow label="Duration" value={formatDuration(session.duration_ms)} />
        <OverviewRow label="Content type" value={responseContentType} />
        {responseCharset && <OverviewRow label="Charset" value={responseCharset} />}
        <OverviewRow label="Response encoding" value={responseEncoding ? `${responseEncoding} · decoded for display` : 'identity'} />
        <OverviewRow label="Size" value={formatSize(session.response.body.length)} />
        {redirectTo && <OverviewRow label="Redirect to" value={redirectTo} />}
        {cacheStatus && <OverviewRow label="Cache" value={cacheStatus} />}
        {server && <OverviewRow label="Server" value={server} />}
        {setCookieCount > 0 && <OverviewRow label="Set-Cookie" value={`${setCookieCount} cookie${setCookieCount === 1 ? '' : 's'}`} />}
      </OverviewSection>
    </div>
  );
}

function OverviewSection({ title, children }: { title: string; children: ReactNode }) {
  return <section className="overview-section"><h3>{title}</h3><div className="overview-rows">{children}</div></section>;
}

function OverviewRow({ label, value }: { label: string; value: ReactNode }) {
  return <div><span>{label}</span><b>{value}</b></div>;
}

function HeaderTable({ headers }: { headers: HeaderEntry[] }) {
  return (
    <section className="headers-section">
      <table className="headers-table"><tbody>{headers.map((header, index) => (
        <tr key={`${header.name}-${index}`}><th>{header.name}</th><td>{header.value}</td></tr>
      ))}</tbody></table>
    </section>
  );
}

function BodyPanel({ body, mimeType }: { body: number[]; mimeType: string | null }) {
  if (!body.length) return <EmptyPanel text="Empty body" />;
  const text = decodeUtf8(body);
  const isText = isTextualMimeType(mimeType) || (text !== null && isLikelyText(text));
  if (!isText || text === null) return <EmptyPanel text={`Binary body · ${formatSize(body.length)}`} />;
  const formattedJson = formatJson(text);
  return <pre className="body-panel">{formattedJson === null ? text : <JsonSyntax value={formattedJson} />}</pre>;
}

function decodeUtf8(body: number[]) {
  try { return new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array(body)); } catch { return null; }
}

function isTextualMimeType(mimeType: string | null) {
  const value = mimeType?.toLowerCase() ?? '';
  return value.startsWith('text/')
    || value.includes('json')
    || value.includes('javascript')
    || value.includes('xml')
    || value.includes('x-www-form-urlencoded')
    || value.includes('graphql');
}

function isLikelyText(value: string) {
  const inspected = value.slice(0, 1024);
  if (!inspected) return true;
  let controlCharacters = 0;
  for (const character of inspected) {
    const code = character.charCodeAt(0);
    if (code < 32 && character !== '\n' && character !== '\r' && character !== '\t') controlCharacters += 1;
  }
  return controlCharacters / inspected.length < 0.02;
}

function formatJson(text: string) {
  try { return JSON.stringify(JSON.parse(text), null, 2); } catch { return null; }
}

function JsonSyntax({ value }: { value: string }) {
  const tokens: ReactNode[] = [];
  const pattern = /"(?:\\.|[^"\\])*"|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|\b(?:true|false|null)\b/g;
  let cursor = 0;
  let index = 0;

  for (const match of value.matchAll(pattern)) {
    const token = match[0];
    const position = match.index ?? cursor;
    if (position > cursor) tokens.push(value.slice(cursor, position));
    const following = value.slice(position + token.length);
    const className = token.startsWith('"')
      ? (/^\s*:/.test(following) ? 'syntax-key' : 'syntax-string')
      : (/^(true|false|null)$/.test(token) ? 'syntax-literal' : 'syntax-number');
    tokens.push(<span className={className} key={`${position}-${index}`}>{token}</span>);
    cursor = position + token.length;
    index += 1;
  }
  if (cursor < value.length) tokens.push(value.slice(cursor));
  return <>{tokens}</>;
}

function headerValue(headers: HeaderEntry[], name: string) {
  return headers.find((header) => header.name.toLowerCase() === name.toLowerCase())?.value ?? null;
}

function contentCharset(contentType: string) {
  return contentType.match(/(?:^|;)\s*charset\s*=\s*([^;\s]+)/i)?.[1] ?? null;
}

function EmptyPanel({ text }: { text: string }) {
  return <p className="empty-panel">{text}</p>;
}
