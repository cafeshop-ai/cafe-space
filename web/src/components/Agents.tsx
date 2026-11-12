import { useEffect, useState } from 'react';
import type { RuntimeSummary } from '../protocol/generated/RuntimeSummary';
import type { DaemonClient, DaemonView } from '../state/DaemonClient';
import { outcomeOf, piPackage, type Outcome } from '../app/copy';
import { ProxyPrompt, useOwnerEnvironment } from './OwnerEnvironment';
import { OutcomeMark, ToolApprovalMark } from './StatePill';

/** Pi and Codex are listed even when the daemon does not report them, so a missing one says what is missing. */
export type AgentRow = { id: string; name: string; runtime: RuntimeSummary | null };
export function agentRows(runtimes: readonly RuntimeSummary[]): AgentRow[] {
  const missing = [['pi', 'Pi'], ['codex', 'Codex']].filter(([id]) => !runtimes.some(item => item.id === id));
  return [...runtimes.map(runtime => ({ id: runtime.id, name: runtime.name, runtime })), ...missing.map(([id, name]) => ({ id, name, runtime: null }))];
}
/** Why this agent cannot start a session; `pathReady` is unknown away from this computer. */
export function missingNote({ id, runtime }: AgentRow, pathReady: boolean | null = null) {
  if (runtime?.available) return null;
  if (id === 'codex' && !runtime) return '这一版还不能启动 Codex。';
  if (id !== 'pi') return '未安装或未通过检测。';
  if (!runtime) return `还没登记。需要 Node 和 Pi ${piPackage.version}。`;
  return pathReady === false ? 'PATH 未就绪：Agent 需要非空、只包含绝对目录的 PATH。' : pathReady ? '登记的 Node 或 Pi 位置已经不能用了，请重新登记。' : '已登记，但检测没有通过。';
}

/** UX §5.1 step I and Settings · Agent: detection only lists what the daemon reports; nothing is sent to a model. */
export function AgentCheck({ client, state, local, proxy = false }: { client: DaemonClient; state: DaemonView; local: boolean; proxy?: boolean }) {
  const connected = state.connection === 'connected';
  // PATH readiness only explains a registered Pi that is unavailable; the first run also offers a detected proxy.
  const broken = state.runtimes?.some(item => item.id === 'pi' && !item.available);
  const env = useOwnerEnvironment(client, connected && local && (proxy || !!broken));
  const [checking, setChecking] = useState(false);
  useEffect(() => { if (connected) void client.loadRuntimes(); }, [client, connected]);
  if (!connected) return <p className="field-note">装好并连上守护进程后开始检测。</p>;
  if (!state.runtimes) return <p className="field-note">正在检测…</p>;
  const rows = agentRows(state.runtimes);
  const pathReady = env.status?.pathReady ?? null;
  const pi = rows.find(row => row.id === 'pi')!.runtime;
  async function check() {
    setChecking(true);
    await Promise.all([client.loadRuntimes(), local && (proxy || broken) && env.read()]);
    setChecking(false);
  }
  return <>
    <ul className="runtime-list">{rows.map(row => {
      const note = missingNote(row, pathReady);
      return <li key={row.id}>
        <span className="runtime-main">
          <span className="choice-title">{row.name}{row.runtime?.version && <span className="choice-version"> {row.runtime.version}</span>}</span>
          {note && <span className="field-note">{note}{row.id === 'pi' && local && pathReady === false && <> 到 <a href="#/settings">设置 · 环境</a> 处理。</>}</span>}
        </span>
        <span className="runtime-marks">{row.runtime?.available && <ToolApprovalMark value={row.runtime.toolApproval} />}
          <span className="state-pill" data-state={row.runtime?.available ? 'ready' : 'stopped'}>{row.runtime?.available ? '就绪' : row.runtime ? '不可用' : row.id === 'pi' ? '未登记' : '不可用'}</span></span>
      </li>;
    })}</ul>
    {!pi?.available && (local ? (!pi || pathReady !== false) && <PiRegistration client={client} connected={connected} /> : <p className="field-note">Pi 只能在电脑上登记。</p>)}
    {proxy && local && <ProxyPrompt env={env} connected={connected} />}
    {!state.runtimes.some(item => item.available) && <p className="field-note">至少需要一个就绪的 Agent 才能开始会话。</p>}
    <button className="quiet-button agent-recheck" disabled={checking} onClick={() => void check()}>重新检测</button>
  </>;
}

/** Explicit owner registration: the daemon only inspects the two paths, never runs or downloads anything. */
function PiRegistration({ client, connected }: { client: DaemonClient; connected: boolean }) {
  const [node, setNode] = useState('');
  const [packageRoot, setPackageRoot] = useState('');
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [busy, setBusy] = useState(false);
  const ready = node.trim() && packageRoot.trim();
  async function register() {
    if (!ready || busy) return;
    setBusy(true); setOutcome(null);
    setOutcome(outcomeOf(await client.registerPi({ node: node.trim(), packageRoot: packageRoot.trim() })));
    setBusy(false);
  }
  return <form className="project-add pi-registration" aria-label="登记 Pi" onSubmit={event => { event.preventDefault(); void register(); }}>
    <p className="field-note">还没装 Pi：装好 Node 后运行 <code>npm install -g {piPackage.name}@{piPackage.version}</code>。</p>
    <label htmlFor="pi-node">Node 程序的完整路径</label>
    <input id="pi-node" value={node} onChange={event => setNode(event.target.value)} placeholder="C:\Program Files\nodejs\node.exe 或 /usr/local/bin/node" spellCheck={false} autoComplete="off" />
    <label htmlFor="pi-package">Pi 包目录的完整路径</label>
    <input id="pi-package" value={packageRoot} onChange={event => setPackageRoot(event.target.value)} placeholder={`…/node_modules/${piPackage.name}`} spellCheck={false} autoComplete="off" />
    <p className="field-note">包目录在 <code>npm root -g</code> 显示的目录里。守护进程只检查这两个位置，不会运行它们，也不会下载。</p>
    <div className="project-add-row"><button type="submit" className="primary-button" disabled={!ready || busy || !connected}>登记 Pi</button></div>
    <OutcomeMark outcome={outcome} />
    {outcome?.kind === 'unknown' && <p className="field-note">没有重试；上面是重新读取到的状态。</p>}
  </form>;
}

export function AgentSettings(props: { client: DaemonClient; state: DaemonView; local: boolean }) {
  return <section className="project-settings" aria-labelledby="agent-title">
    <h2 id="agent-title">Agent</h2>
    <p className="field-note">只检测本机安装了哪些 Agent，不会向任何模型发送内容。</p>
    <AgentCheck {...props} />
  </section>;
}
