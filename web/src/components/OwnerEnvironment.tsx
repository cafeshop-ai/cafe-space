import { useEffect, useState, type ReactNode } from 'react';
import type { OwnerEnvironment as Status } from '../protocol/generated/OwnerEnvironment';
import type { OwnerEnvironmentChange } from '../protocol/generated/OwnerEnvironmentChange';
import type { OwnerEnvironmentEntry } from '../protocol/generated/OwnerEnvironmentEntry';
import type { RequestResult } from '../protocol/generated/RequestResult';
import type { DaemonClient } from '../state/DaemonClient';
import { errorMessage, outcomeOf, type Outcome } from '../app/copy';
import { OutcomeMark } from './StatePill';

const shown = (entry: OwnerEnvironmentEntry) => entry.display === 'set' ? '已设置' : entry.display;
function purpose(name: string) {
  const upper = name.toUpperCase();
  if (upper === 'PATH') return 'Agent 查找命令的目录。';
  if (/^(HTTPS?|ALL)_PROXY$/.test(upper)) return '代理：Agent 通过它访问模型。';
  if (upper === 'NO_PROXY') return '不经过代理的地址。';
  if (upper === 'SSH_AUTH_SOCK') return 'SSH 代理：Agent 可以用你的 SSH 密钥签名。';
  if (upper === 'NODE_EXTRA_CA_CERTS' || upper === 'SSL_CERT_FILE') return '证书：Agent 用它校验 HTTPS 连接。';
  if (upper === 'NODE_OPTIONS') return 'Node 的启动参数。';
  if (upper.endsWith('_API_KEY')) return 'API 密钥。';
  if (upper.endsWith('_TOKEN')) return '访问令牌。';
  return null;
}
const bytes = (text: string) => new TextEncoder().encode(text).length;
const unsendable = /[\p{Cc}\p{Cs}]/u;
/** Mirrors the daemon's checks: a name or value it cannot accept fails the whole message, not just this write. */
function problem(name: string, value: string) {
  if (/^CAFE_SPACE_/i.test(name)) return 'CAFE_SPACE_ 开头的变量名是保留的。';
  if (name.includes('=') || unsendable.test(name)) return '变量名不能包含 = 或控制字符。';
  if (bytes(name) > 256) return '变量名太长了。';
  if (unsendable.test(value)) return '值不能包含控制字符。';
  if (bytes(value) > 65536) return '值太长了。';
  return null;
}
const unexpected: Outcome = { kind: 'unknown', code: 'UNEXPECTED_RESULT' };
const statusOf = (result: RequestResult) => result.status === 'applied' && result.data.result.kind === 'owner_environment' ? result.data.result.data : null;

/**
 * Owner environment for future Agent children, local only (protocol v2-local-api "Owner environment", D20).
 * Agents inherit the daemon's environment; overrides win. Values arrive masked; a typed value is sent once and never shown.
 */
export function useOwnerEnvironment(client: DaemonClient, connected: boolean) {
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
    return outcome;
  }
  return { status, failure, busy, last, read, update };
}

function LastWrite({ last }: { last: { name: string; outcome: Outcome } | null }) {
  if (!last) return null;
  return <div className="env-outcome">
    <code>{last.name}</code> <OutcomeMark outcome={last.outcome} />
  </div>;
}

export function EnvironmentSettings({ client, connected }: { client: DaemonClient; connected: boolean }) {
  const env = useOwnerEnvironment(client, connected);
  const [confirm, setConfirm] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [value, setValue] = useState('');
  const { status } = env;
  const locked = !connected || env.busy;
  const invalid = problem(name, value);
  // Windows names and Unix proxy aliases match in any case.
  const configured = new Set(status?.configured.map(entry => entry.name.toUpperCase()));
  const row = (entry: OwnerEnvironmentEntry, action: ReactNode, note = purpose(entry.name)) => <li key={entry.name} className="project-row">
    <div className="project-main">
      <span className="env-line"><code className="project-name">{entry.name}</code> <span className="env-value">{shown(entry)}</span></span>
      {note && <span className="field-note">{note}</span>}
    </div>
    <div className="project-actions">{action}</div>
  </li>;
  async function save() {
    if ((await env.update(name, { action: 'set', value })).kind === 'done') { setName(''); setValue(''); }
  }
  return <section className="project-settings" aria-labelledby="environment-title">
    <h2 id="environment-title">环境</h2>
    <p className="field-note">Agent 启动时继承守护进程的环境变量，覆盖优先。</p>
    {!status ? env.failure ? <div className="env-outcome">
      <span>没能读取环境：{errorMessage(env.failure.code ?? '')}</span>
      <button className="secondary-button" disabled={locked} onClick={() => void env.read()}>重新读取</button>
    </div> : <p className="field-note">正在读取…</p> : <>
      <ul className="env-tools" aria-label="PATH 与工具">
        <li><span className="state-pill" data-state={status.pathReady ? 'ready' : 'failed'}>PATH {status.pathReady ? '已就绪' : '未就绪'}</span></li>
        {status.tools.map(tool => <li key={tool.name}><span className="state-pill" data-state={tool.found ? 'ready' : 'stopped'}>{tool.name} {tool.found ? '找到' : '没找到'}</span></li>)}
      </ul>
      <h3>守护进程环境里检测到</h3>
      {status.detected.length === 0 ? <p className="field-note">没有检测到。</p> :
        <ul className="project-list">{status.detected.map(entry => row(entry, entry.adoptable &&
          <button className="secondary-button" disabled={locked} onClick={() => void env.update(entry.name, { action: 'adopt_detected' })}>
            {configured.has(entry.name.toUpperCase()) ? '改用检测到的值' : '固定为当前值'}</button>))}</ul>}
      <h3>覆盖</h3>
      {status.configured.length === 0 ? <p className="field-note">还没有。</p> :
        <ul className="project-list">{status.configured.map(entry => row(entry, confirm === entry.name ? <span className="inline-confirm">
          移除后改用守护进程环境里的值。
          <button className="secondary-button" disabled={locked} onClick={() => { setConfirm(null); void env.update(entry.name, { action: 'remove' }); }}>确认移除</button>
          <button className="quiet-button" onClick={() => setConfirm(null)}>取消</button>
        </span> : <button className="quiet-button" disabled={locked} onClick={() => setConfirm(entry.name)}>移除</button>))}</ul>}
      <form className="project-add" onSubmit={event => { event.preventDefault(); void save(); }}>
        <label htmlFor="env-name">设置变量</label>
        <div className="project-add-row">
          <input id="env-name" value={name} onChange={event => setName(event.target.value)} placeholder="变量名" spellCheck={false} autoComplete="off" />
          <input type="password" aria-label="值" value={value} onChange={event => setValue(event.target.value)} placeholder="值" autoComplete="off" />
          <button type="submit" className="primary-button" disabled={locked || !name || !value || !!invalid}>保存</button>
        </div>
        {invalid && <p className="field-note">{invalid}</p>}
      </form>
    </>}
    <LastWrite last={env.last} />
  </section>;
}
