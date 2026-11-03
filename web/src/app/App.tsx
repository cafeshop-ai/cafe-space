import { useEffect, useState, useSyncExternalStore } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { WorkspaceLayout } from '../layouts/WorkspaceLayout';
import { CoffeeActivityMark } from '../components/CoffeeActivityMark';
import { DaemonClient, type ConnectionState } from '../state/DaemonClient';

const routes = [
  ['sessions', '会话'], ['rooms', '房间'], ['remote', '远程访问'], ['settings', '设置'],
] as const;
const connectionLabels: Record<ConnectionState, string> = {
  disconnected: '未连上', connecting: '正在连接', connected: '已连接', version_mismatch: '版本不符',
};
const sessionLabels = { starting: '正在启动', ready: '就绪', running: '运行中', waiting_for_local_ui: '等待本机操作', stopping: '正在停止', stopped: '已停止', failed: '失败' };
const routeSnapshot = () => routes.find(([key]) => window.location.hash === `#/${key}`)?.[0] ?? 'sessions';
const subscribeRoute = (listener: () => void) => {
  window.addEventListener('hashchange', listener);
  return () => window.removeEventListener('hashchange', listener);
};

export function App({ client: suppliedClient, nativeAvailable = isTauri() }: { client?: DaemonClient; nativeAvailable?: boolean }) {
  const [client] = useState(() => suppliedClient ?? new DaemonClient());
  const state = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const page = useSyncExternalStore(subscribeRoute, routeSnapshot);
  useEffect(() => {
    if (nativeAvailable) void client.connect();
    return () => client.disconnect();
  }, [client, nativeAvailable]);
  useEffect(() => {
    if (nativeAvailable && state.connection === 'connected' && state.sessions?.length === 0) {
      // The native command is inert outside an explicitly enabled debug-only WebView probe.
      void invoke('m0_finish').catch(() => {});
    }
  }, [nativeAvailable, state.connection, state.sessions]);

  const connected = state.connection === 'connected';
  const title = routes.find(([key]) => key === page)![1];
  return <>
    <a className="skip-link" href="#main-content" onClick={event => { event.preventDefault(); document.getElementById('main-content')?.focus(); }}>跳到内容</a>
    <WorkspaceLayout header={<>
      <a className="brand" href="#/sessions">Café Space</a>
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
      <div className="sidebar-footnote">M0 · 基础验证</div>
    </>}>
      <section className="page-content" aria-labelledby="page-title">
        <div className="page-heading"><h1 id="page-title">{title}</h1>
          {page === 'sessions' && <span className="session-count">{state.sessions === null ? '尚未读取' : `${state.sessions.length} 个会话`}</span>}
        </div>
        {state.error && <p className="connection-error" role="alert">{state.error}</p>}
        {state.lastUpdated !== null && !connected && <p className="last-updated">上次更新 {new Date(state.lastUpdated).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })} · 当前离线</p>}
        {page === 'sessions' && <>
          {!connected && <div className="connection-notice">
            <p>{nativeAvailable ? (state.connection === 'connecting' ? '正在等待守护进程握手与会话快照。' : '先连接本机守护进程，再读取会话。') : '请从桌面应用连接本机守护进程；浏览器预览不会连接云端。'}</p>
            {nativeAvailable && state.connection !== 'connecting' && <button className="secondary-button" onClick={() => void client.connect()}>重新连接</button>}
          </div>}
          {state.connection === 'connecting' && state.sessions === null ? <div className="skeleton-list" aria-label="正在读取会话" aria-busy="true"><div /><div /><div /></div> :
            state.sessions?.length === 0 ? <div className="empty-state">
              <CoffeeActivityMark />
              <h2>这里还没有会话</h2>
              <p>从本机 Agent 开始一段新会话。</p>
              <button className="primary-button" disabled aria-describedby="new-session-limit">新建会话</button>
              <small id="new-session-limit">M1 接入 Agent 后可用</small>
            </div> : state.sessions && <ul className="session-list">{state.sessions.map(session => <li key={session.id}><strong>{session.title || session.id}</strong><span>{sessionLabels[session.state.state]}</span></li>)}</ul>}
        </>}
        {page === 'rooms' && <div className="empty-state"><CoffeeActivityMark /><h2>房间尚未接入</h2><p>将在 M3 接入协作房间。</p></div>}
        {page === 'remote' && <div className="empty-state"><CoffeeActivityMark /><h2>远程访问尚未接入</h2><p>将在 M2 接入其他设备。</p></div>}
        {page === 'settings' && <dl className="settings-list">
          <div><dt>本机连接</dt><dd>仅当前系统用户可用的 socket / named pipe</dd></div>
          <div><dt>守护进程版本</dt><dd>{state.daemonVersion ?? '尚未读取'}</dd></div>
          <div><dt>窗口与守护进程</dt><dd>关闭窗口不会停止守护进程。</dd></div>
        </dl>}
      </section>
    </WorkspaceLayout>
  </>;
}
