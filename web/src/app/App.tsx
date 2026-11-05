import { useEffect, useState, useSyncExternalStore } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { WorkspaceLayout } from '../layouts/WorkspaceLayout';
import { CoffeeActivityMark } from '../components/CoffeeActivityMark';
import { CommandPalette, pageKeys, shortcut } from '../components/CommandPalette';
import { DaemonInstallation } from '../components/DaemonInstallation';
import { FirstRun } from '../components/FirstRun';
import { NewSessionSheet } from '../components/NewSessionSheet';
import { ProjectSettings } from '../components/ProjectSettings';
import { SessionList, sessionHref } from '../components/SessionList';
import { SessionView } from '../components/SessionView';
import { DaemonClient, type ConnectionState } from '../state/DaemonClient';

const routes = [
  ['sessions', '会话'], ['rooms', '房间'], ['remote', '远程访问'], ['settings', '设置'],
] as const;
const connectionLabels: Record<ConnectionState, string> = {
  disconnected: '未连上', connecting: '正在连接', connected: '已连接', version_mismatch: '版本不符',
};
// `#/sessions/<id>` opens one session; every other hash names a page.
const routeSnapshot = () => window.location.hash;
const parseRoute = (hash: string) => {
  const match = /^#\/sessions\/(.+)$/.exec(hash);
  if (match) { try { return { page: 'sessions' as const, sessionId: decodeURIComponent(match[1]) }; } catch { /* fall through to the list */ } }
  return { page: routes.find(([key]) => hash === `#/${key}`)?.[0] ?? 'sessions', sessionId: null };
};
const subscribeRoute = (listener: () => void) => {
  window.addEventListener('hashchange', listener);
  return () => window.removeEventListener('hashchange', listener);
};
const navigate = (href: string) => { window.location.hash = href.slice(1); };

export function App({ client: suppliedClient, nativeAvailable = isTauri() }: { client?: DaemonClient; nativeAvailable?: boolean }) {
  const [client] = useState(() => suppliedClient ?? new DaemonClient());
  const state = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const { page, sessionId } = parseRoute(useSyncExternalStore(subscribeRoute, routeSnapshot));
  const [sheetOpen, setSheetOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  useEffect(() => {
    if (nativeAvailable) void client.connect();
    return () => client.disconnect();
  }, [client, nativeAvailable]);
  const connectedNow = state.connection === 'connected';
  useEffect(() => {
    if (sessionId && connectedNow) void client.openSession(sessionId);
    if (!sessionId) client.closeSessionView();
  }, [client, sessionId, connectedNow]);
  useEffect(() => {
    // ⌘K, ⌘N, ⌘1–3 and ⌘, (UX §6.7); Ctrl on Windows and Linux. ⌘. belongs to the open session view.
    const onKey = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.shiftKey || event.altKey) return;
      const key = event.key.toLowerCase();
      if (key === 'k') { event.preventDefault(); setPaletteOpen(open => !open); }
      else if (key === 'n' && connectedNow) { event.preventDefault(); setSheetOpen(true); }
      else if (pageKeys[key]) { event.preventDefault(); navigate(pageKeys[key]); }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [connectedNow]);
  useEffect(() => {
    if (nativeAvailable && state.connection === 'connected' && state.sessions?.length === 0) {
      // The native command is inert outside an explicitly enabled debug-only WebView probe.
      void invoke('m0_finish').catch(() => {});
    }
  }, [nativeAvailable, state.connection, state.sessions]);

  const connected = connectedNow;
  const title = routes.find(([key]) => key === page)![1];
  return <>
    <a className="skip-link" href="#main-content" onClick={event => { event.preventDefault(); document.getElementById('main-content')?.focus(); }}>跳到内容</a>
    <WorkspaceLayout header={<>
      <a className="brand" href="#/sessions">Café Space</a>
      <button className="quiet-button palette-button" aria-keyshortcuts="Meta+K Control+K" onClick={() => setPaletteOpen(true)}>搜索<kbd>{shortcut('K')}</kbd></button>
      <button className="quiet-button needs-you" disabled title="M1 接入 Agent 审批后可用">待处理</button>
      <span className="connection-status" data-state={state.connection} role="status">
        <span className="status-dot" aria-hidden="true" />{connectionLabels[state.connection]}
      </span>
    </>} sidebar={<>
      <div className="sidebar-caption">本机工作区</div>
      <nav className="workspace-navigation" aria-label="主导航">
        {routes.map(([key, label]) => <a key={key} href={`#/${key}`} aria-current={page === key ? 'page' : undefined}>
          {label}{key === 'sessions' && <span className="nav-count" aria-label="会话数量">{state.sessions?.length ?? '—'}</span>}
        </a>)}
      </nav>
      <div className="sidebar-footnote">M1 · 本机 Agent</div>
    </>}>
      <section className="page-content" aria-labelledby="page-title">
        <div className="page-heading"><h1 id="page-title">{sessionId ? <a href="#/sessions" className="back-link">{title}</a> : title}</h1>
          {page === 'sessions' && !sessionId && <span className="session-count">{state.sessions === null ? '尚未读取' : `${state.sessions.length} 个会话`}</span>}
          {page === 'sessions' && !sessionId && connected && !!state.sessions?.length && <button className="primary-button heading-action" onClick={() => setSheetOpen(true)}>新建会话</button>}
        </div>
        {nativeAvailable && <DaemonInstallation client={client} connection={state.connection} sessions={state.sessions} show={page === 'settings' || state.connection === 'disconnected' || state.connection === 'version_mismatch'} />}
        {state.error && <p className="connection-error" role="alert">{state.error}</p>}
        {state.lastUpdated !== null && !connected && <p className="last-updated">上次更新 {new Date(state.lastUpdated).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })} · 当前离线</p>}
        {page === 'sessions' && <>
          {!connected && <div className="connection-notice">
            <p>{nativeAvailable ? (state.connection === 'connecting' ? '正在等待守护进程握手与会话同步。' : '先连接本机守护进程，再读取会话。') : '请从桌面应用连接本机守护进程；浏览器预览不会连接云端。'}</p>
            {nativeAvailable && state.connection !== 'connecting' && <button className="secondary-button" onClick={() => void client.connect()}>重新连接</button>}
          </div>}
          {sessionId ? <SessionView key={sessionId} client={client} state={state} sessionId={sessionId} onNavigate={navigate} /> :
            state.connection === 'connecting' && state.sessions === null ? <div className="skeleton-list" aria-label="正在读取会话" aria-busy="true"><div /><div /><div /></div> :
            state.sessions?.length === 0 && connected && state.projects?.length === 0 ? <FirstRun client={client} state={state} local={nativeAvailable} /> :
            state.sessions?.length === 0 ? <div className="empty-state">
              <CoffeeActivityMark />
              <h2>这里还没有会话</h2>
              <p>从本机 Agent 开始一段新会话。</p>
              <button className="primary-button" disabled={!connected} aria-describedby="new-session-limit" onClick={() => setSheetOpen(true)}>新建会话</button>
              {!connected && <small id="new-session-limit">连接守护进程后可用</small>}
            </div> : state.sessions && <SessionList sessions={state.sessions} projects={state.projects ?? []} runtimes={state.runtimes} />}
        </>}
        {page === 'rooms' && <div className="empty-state"><CoffeeActivityMark /><h2>房间尚未接入</h2><p>将在 M3 接入协作房间。</p></div>}
        {page === 'remote' && <div className="empty-state"><CoffeeActivityMark /><h2>远程访问尚未接入</h2><p>将在 M2 接入其他设备。</p></div>}
        {page === 'settings' && nativeAvailable && <ProjectSettings client={client} state={state} />}
        {page === 'settings' && <dl className="settings-list">
          <div><dt>本机连接</dt><dd>仅当前系统用户可用的 socket / named pipe</dd></div>
          <div><dt>守护进程版本</dt><dd>{state.daemonVersion ?? '尚未读取'}</dd></div>
          <div><dt>窗口与守护进程</dt><dd>关闭窗口不会停止守护进程。</dd></div>
        </dl>}
      </section>
    </WorkspaceLayout>
    {/* Before the sheet: the palette must close before the sheet it may open takes focus. */}
    <CommandPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} state={state} onNavigate={navigate} onNewSession={() => setSheetOpen(true)} />
    <NewSessionSheet client={client} state={state} local={nativeAvailable} open={sheetOpen} onClose={() => setSheetOpen(false)}
      onCreated={id => { setSheetOpen(false); navigate(sessionHref(id)); }} />
  </>;
}
