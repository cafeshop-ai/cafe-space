import { useEffect, useState, type ReactNode } from 'react';
import type { OwnerEnvironment as Status } from '../protocol/generated/OwnerEnvironment';
import type { OwnerEnvironmentChange } from '../protocol/generated/OwnerEnvironmentChange';
import type { OwnerEnvironmentEntry } from '../protocol/generated/OwnerEnvironmentEntry';
import type { RequestResult } from '../protocol/generated/RequestResult';
import type { DaemonClient } from '../state/DaemonClient';
import { errorMessage, outcomeOf, type Outcome } from '../app/copy';
import { OutcomeMark } from './StatePill';

const isProxy = (name: string) => /^(https?|all)_proxy$/i.test(name);
const shown = (entry: OwnerEnvironmentEntry) => entry.display === 'set' ? '已设置' : entry.display;
function purpose(name: string) {
  if (name === 'PATH') return 'Agent 查找命令的目录；没有设置时沿用守护进程的 PATH。';
  if (isProxy(name)) return '代理：Agent 通过它访问模型。';
  if (/^no_proxy$/i.test(name)) return '不经过代理的地址。';
  if (name === 'SSH_AUTH_SOCK') return 'SSH 代理：Agent 可以用你的 SSH 密钥签名。';
  return '证书：Agent 会信任这些额外的证书。';
}
const unexpected: Outcome = { kind: 'unknown', code: 'UNEXPECTED_RESULT' };
const statusOf = (result: RequestResult) => result.status === 'applied' && result.data.result.kind === 'owner_environment' ? result.data.result.data : null;

/**
 * Owner environment for future Agent children, local only (protocol v2-local-api "Owner environment").
 * Values arrive masked and are never typed or echoed here: the only writes are adopt-by-name and remove.
 */
function useOwnerEnvironment(client: DaemonClient, connected: boolean) {
  const [status, setStatus] = useState<Status | null>(null);
  const [failure, setFailure] = useState<Outcome | null>(null);
  // One request at a time: every action is locked while a read or write is in flight, so replies cannot cross.
  const [busy, setBusy] = useState(false);
  const [last, setLast] = useState<{ name: string; outcome: Outcome } | null>(null);
  async function read() {
    setBusy(true);
    const result = await client.call({ method: 'get_owner_environment' });
    const next = statusOf(result);
    setStatus(next);
    setFailure(next ? null : result.status === 'applied' ? unexpected : outcomeOf(result));
    setBusy(false);
  }
  useEffect(() => { if (connected) void read(); }, [client, connected]);
  async function update(name: string, change: OwnerEnvironmentChange) {
    setBusy(true); setLast(null);
    const result = await client.call({ method: 'update_owner_environment', params: { name, change } });
    const next = statusOf(result);
    if (next) setStatus(next);
    const outcome = result.status === 'applied' && !next ? unexpected : outcomeOf(result);
    setLast({ name, outcome });
    // An unknown write is never replayed; reading shows what the daemon holds now.
    if (outcome.kind === 'unknown') await read();
    setBusy(false);
  }
  return { status, failure, busy, last, read, update };
}

function LastWrite({ last, status }: { last: { name: string; outcome: Outcome } | null; status: Status | null }) {
  if (!last) return null;
  return <div className="env-outcome">
    <code>{last.name}</code> <OutcomeMark outcome={last.outcome} />
    {last.outcome.kind === 'unknown' && status && <p className="field-note">没有重试；这里显示的是重新读取到的当前状态。</p>}
  </div>;
}

export function EnvironmentSettings({ client, connected }: { client: DaemonClient; connected: boolean }) {
  const env = useOwnerEnvironment(client, connected);
  const [confirm, setConfirm] = useState<string | null>(null);
  const { status } = env;
  const locked = !connected || env.busy;
  const configured = new Set(status?.configured.map(entry => entry.name));
  const adoptable = new Set(status?.detected.filter(entry => entry.adoptable).map(entry => entry.name));
  const row = (entry: OwnerEnvironmentEntry, action: ReactNode) => <li key={entry.name} className="project-row">
    <div className="project-main">
      <span className="env-line"><code className="project-name">{entry.name}</code> <span className="env-value">{shown(entry)}</span></span>
      <span className="field-note">{purpose(entry.name)}</span>
    </div>
    <div className="project-actions">{action}</div>
  </li>;
  return <section className="project-settings" aria-labelledby="environment-title">
    <h2 id="environment-title">环境</h2>
    <p className="field-note">Agent 启动时拿到的环境变量，只影响之后新开或继续的会话。这里只显示遮掩后的值。</p>
    {!status ? env.failure ? <div className="env-outcome">
      <span>没能读取环境：{errorMessage(env.failure.code ?? '')}</span>
      <button className="secondary-button" disabled={locked} onClick={() => void env.read()}>重新读取</button>
    </div> : <p className="field-note">正在读取…</p> : <>
      <ul className="env-tools" aria-label="PATH 与工具">
        <li><span className="state-pill" data-state={status.pathReady ? 'ready' : 'failed'}>PATH {status.pathReady ? '已就绪' : '未就绪'}</span></li>
        {status.tools.map(tool => <li key={tool.name}><span className="state-pill" data-state={tool.found ? 'ready' : 'stopped'}>{tool.name} {tool.found ? '找到' : '没找到'}</span></li>)}
      </ul>
      {!status.pathReady && <p className="field-note">Agent 需要非空、只包含绝对目录的 PATH。</p>}
      <p className="field-note">只检查 PATH 里有没有这些文件，不会运行它们；找到不代表能用。</p>
      <h3>交给 Agent 的变量</h3>
      {status.configured.length === 0 ? <p className="field-note">还没有。代理、证书和 SSH 代理默认都不会交给 Agent。</p> :
        <ul className="project-list">{status.configured.map(entry => row(entry, confirm === entry.name ? <span className="inline-confirm">
          {entry.name === 'PATH' ? '移除后改用守护进程的 PATH。' : `移除后 Agent 不再拿到它${adoptable.has(entry.name) ? '' : '，这里也无法再填回它的值'}。`}
          <button className="secondary-button" disabled={locked} onClick={() => { setConfirm(null); void env.update(entry.name, { action: 'remove' }); }}>确认移除</button>
          <button className="quiet-button" onClick={() => setConfirm(null)}>取消</button>
        </span> : <button className="quiet-button" disabled={locked} onClick={() => setConfirm(entry.name)}>移除</button>))}</ul>}
      <h3>守护进程环境里检测到</h3>
      <p className="field-note">只读取守护进程自己的环境，不读取系统代理设置或登录 shell。</p>
      {status.detected.length === 0 ? <p className="field-note">没有检测到。</p> :
        <ul className="project-list">{status.detected.map(entry => row(entry, entry.adoptable ?
          <button className="secondary-button" disabled={locked} onClick={() => void env.update(entry.name, { action: 'adopt_detected' })}>
            {configured.has(entry.name) ? '改用检测到的值' : entry.name === 'PATH' ? '固定为当前值' : '交给 Agent'}</button> :
          <span className="field-note">这个值无法交给 Agent。</span>))}</ul>}
    </>}
    <LastWrite last={env.last} status={status} />
  </section>;
}

/** UX §5.1 step I: offers a detected proxy, masked, until the owner adopts it here or in Settings. */
export function ProxyPrompt({ client, connected }: { client: DaemonClient; connected: boolean }) {
  const env = useOwnerEnvironment(client, connected);
  const proxy = env.status?.detected.find(entry => isProxy(entry.name) && entry.adoptable && !env.status!.configured.some(item => item.name === entry.name));
  if (!proxy && !env.last) return null;
  return <div className="proxy-prompt">
    {proxy && <div className="env-outcome">
      <span>这台电脑配置了代理 <code>{proxy.name}</code> <span className="env-value">{shown(proxy)}</span>。Agent 需要通过它访问模型吗？</span>
      <button className="secondary-button" disabled={!connected || env.busy} onClick={() => void env.update(proxy.name, { action: 'adopt_detected' })}>使用</button>
    </div>}
    <LastWrite last={env.last} status={env.status} />
    <p className="field-note">代理、证书和 SSH 代理只有你在这里或设置里选择后才会交给 Agent。</p>
  </div>;
}
