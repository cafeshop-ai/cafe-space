// Dev-only layout preview (`pnpm dev`, then open /preview.html): the real App against a scripted
// in-memory daemon. It is not part of the production build and never connects anywhere.
import { createRoot } from 'react-dom/client';
import { App } from '../app/App';
import { DaemonClient } from '../state/DaemonClient';
import type { Transport } from '../services/transport';
import type { ClientRequest } from '../protocol/generated/ClientRequest';
import type { OwnerEnvironment } from '../protocol/generated/OwnerEnvironment';
import type { RequestResult } from '../protocol/generated/RequestResult';
import type { ServerMessage } from '../protocol/generated/ServerMessage';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { SessionView } from '../protocol/generated/SessionView';
import '../styles.css';

const capabilities = { modelControl: false, thinkingControl: false, deliveryControl: true, sessionControl: true, approvalForSession: false, toolApproval: 'never_asks' as const };
const session = (id: string, title: string, projectId: string, state: SessionSnapshot['state'], extra: Partial<SessionSnapshot> = {}): SessionSnapshot =>
  ({ id, title, projectId, runtimeId: 'pi', cwdFence: 'fence-a', capabilities, mode: 'new', state, streamId: `stream-${id}`, ...extra });
// Creation order, as the daemon lists them: the newest is last. s5 predates activity times.
const minutesAgo = (minutes: number) => Date.now() - minutes * 60_000;
const sessions = [
  session('s4', '旧会话', 'gone', { state: 'stopped' }, { stopReason: 'DAEMON_RESTARTED', lastActivityAt: new Date(2025, 2, 2).getTime(), preview: '先把旧的发布流程停掉' }),
  session('s3', '整理 11 月发布说明', 'p2', { state: 'failed', data: { code: 'PI_UNOWNED_RUN' } }, { lastActivityAt: minutesAgo(12 * 1440) }),
  session('s6', '秦淮定制弹窗提示：对接后端 SSE 订阅接口', 'p2', { state: 'ready' }, { runtimeId: 'codex', model: { provider: 'openai', id: 'gpt-5-codex' }, lastActivityAt: minutesAgo(1560), preview: '订阅断开后要自动重连吗？先只提示，不要自动重放请求' }),
  session('s5', 'FMEA 标准绑定字段核对', 'p3', { state: 'waiting_for_local_ui' }),
  session('s2', '整理菜单与价格', 'p1', { state: 'ready' }, { thinkingLevel: 'medium', model: { provider: 'openrouter', id: 'qwen/qwen3-coder' }, capabilities: { ...capabilities, thinkingControl: true, modelControl: true },
    lastActivityAt: minutesAgo(180), preview: '价格表里的季节限定要单独列一栏，并且按区域区分，东区和西区的定价策略不一样，记得把历史价格也保留下来方便对比' }),
  session('s1', '把菜单加载的冷启动降到 1 秒内', 'p1', { state: 'running' }, { activeRunId: 'run-1', model: { provider: 'anthropic', id: 'claude-opus-5-5' }, lastActivityAt: minutesAgo(0.3), preview: '帮我把菜单加载改快一点，冷启动现在要 3 秒。' }),
];
const projects = [{ id: 'p1', name: 'cafe-space', cwdFence: 'fence-a' }, { id: 'p2', name: 'deepctrls-sight', cwdFence: 'fence-b' }, { id: 'p3', name: 'catl-dehumidifier-pro-fmea-standard-bindings', cwdFence: 'fence-c' }];
const roots: Record<string, string> = { p1: 'D:\\sh\\cafe-space', p2: 'C:\\work\\nj-hospital\\deepctrls-sight', p3: 'C:\\work\\catl\\catl-dehumidifier-pro-fmea-standard-bindings' };
// Enough entries to scroll, ending with the largest identifiers the wire allows (128-byte provider, 256-byte id).
const models = [{ provider: 'openrouter', id: 'qwen/qwen3-coder' }, { provider: 'openrouter', id: 'deepseek/deepseek-chat-v3.1' }, { provider: 'local-lab', id: 'a-very-long-self-hosted-model-name-for-wrapping-checks-q4_k_m' },
  ...Array.from({ length: 20 }, (_, i) => ({ provider: 'ollama', id: `llama-${i + 1}b-instruct` })), { provider: 'p'.repeat(128), id: 'm'.repeat(256) }];
// `?first` plays a computer with no session or project, `?first=install` one without the daemon (pair it with a
// native stub whose installation info says not registered); `?agents=none|broken|path` plays Pi missing, failing detection or PATH.
const search = new URLSearchParams(window.location.search);
const first = search.get('first');
const agents = search.get('agents');
const pi = (available: boolean) => ({ id: 'pi', name: 'Pi', version: '1.5.0', available, toolApproval: 'never_asks' as const });
let runtimes = agents === 'none' ? [] : agents ? [pi(false)] : [pi(true), { id: 'codex', name: 'Codex', version: '0.9.0', available: true, toolApproval: 'unknown' as const }];
const detected = (name: string, display = 'set', adoptable = true) => ({ name, display, source: 'daemon_environment' as const, adoptable });
// SSH_AUTH_SOCK plays an unknown write; SSL_CERT_FILE cannot be adopted.
const environment: OwnerEnvironment = {
  detected: [detected('PATH'), detected('HTTPS_PROXY', 'http://•••:•••@proxy.corp-network-with-a-long-name.example.invalid:3128'), detected('NO_PROXY'), detected('SSH_AUTH_SOCK'), detected('SSL_CERT_FILE', 'set', false)],
  configured: [{ name: 'NODE_EXTRA_CA_CERTS', display: 'set', source: 'owner_override', adoptable: false }], pathReady: agents !== 'path',
  tools: [{ name: 'git', found: true }, { name: 'node', found: true }, { name: 'npm', found: false }, { name: 'pwsh', found: true }],
};
const longLine = 'const result = await loadMenu({ region: "east", includeSeasonal: true, cacheKey: "menu:east:2026-11", retries: 3 });';
const view: SessionView = {
  session: sessions[5], historyTruncated: true, approvals: [{ id: 'a1', summary: 'Run `npm install --save-dev vitest@4.1.12` in the project folder' }],
  messages: [
    { id: 'm1', role: 'user', status: 'complete', parts: [{ kind: 'text', data: { text: '帮我把菜单加载改快一点，冷启动现在要 3 秒。' } }] },
    { id: 'm2', role: 'assistant', status: 'complete', parts: [
      { kind: 'thinking', data: { text: 'Read the loader, then profile the cold path.' } },
      { kind: 'tool_call', data: { id: 't1', name: 'read', arguments: '{"path":"src/menu/loader.ts"}', output: `${longLine}\n`.repeat(6), status: 'complete' } },
      { kind: 'tool_call', data: { id: 't2', name: 'bash', arguments: '{"command":"npm test -- --run menu"}', status: 'running' } },
      { kind: 'text', data: { text: `### 计划\n\n- [x] 读取 loader\n- [ ] 按区域缓存\n\n加载器在渲染前会请求所有区域，详见 [性能说明](https://example.test/perf)。\n\n\`\`\`ts\n${longLine}\n\`\`\`\n\n| 区域 | 冷启动 |\n|---|---:|\n| east | 3.1 s |\n| west | 2.8 s |\n\n> 先缓存，再懒加载其余区域。<b>raw</b>` } },
    ] },
    { id: 'm3', role: 'assistant', status: 'streaming', parts: [{ kind: 'text', data: { text: '[RPC record omitted: 1048577 bytes exceeds display limit; transcript may be incomplete]' } }] },
  ],
};
// `?recovery`, `?recovery=no-backup`, `?recovery=remote` or `?recovery=unsure` (restore outcome unknown) plays a daemon whose
// migration failed; an applied restore, or `?recovery=restored`, leaves it at ROLLBACK_REQUIRED.
let recovery = search.get('recovery');
const dataDirectory = 'C:\\Users\\owner-with-a-long-name\\AppData\\Local\\cafe-space';
let sequence = 0;
const applied = (result: Extract<RequestResult, { status: 'applied' }>['data']['result']): RequestResult => ({ status: 'applied', data: { result } });

class PreviewTransport implements Transport {
  private onMessage: (message: ServerMessage) => void = () => {};
  async open(onMessage: (message: ServerMessage) => void) {
    if (first === 'install') throw Error('not installed');
    this.onMessage = onMessage;
    queueMicrotask(() => onMessage({ type: 'welcome', protocolVersion: 9, daemonVersion: '0.1.0-preview' }));
  }
  async send({ requestId, request }: ClientRequest) {
    const reply = (result: RequestResult) => queueMicrotask(() => this.onMessage({ type: 'response', protocolVersion: 9, requestId, result }));
    if (recovery !== null) {
      const backup = recovery !== 'no-backup';
      const health = { ready: false, daemonVersion: '0.1.0-preview', update: 'draining' as const, recovery: { code: recovery === 'restored' ? 'ROLLBACK_REQUIRED' as const : 'SCHEMA_MIGRATION_FAILED' as const, fromSchema: 5, toSchema: 6, backup } };
      const details = { dataDirectory, backupFile: backup ? `${dataDirectory}\\state.sqlite3.v5.k3x9q2mf7a.bak` : null, stateFiles: [`${dataDirectory}\\state.sqlite3`, `${dataDirectory}\\state.sqlite3-wal`, `${dataDirectory}\\handover.json`] };
      if (request.method === 'get_status') return reply(applied({ kind: 'health', data: health }));
      if (request.method === 'restore_backup') {
        if (recovery === 'remote') return reply({ status: 'rejected', data: { error: { code: 'LOCAL_ONLY', message: '' } } });
        if (!backup || recovery === 'restored') return reply({ status: 'rejected', data: { error: { code: 'RESTORE_NOT_AVAILABLE', message: '' } } });
        if (recovery === 'unsure') return reply({ status: 'unknown', data: { error: { code: 'TIMEOUT', message: '' } } });
        recovery = 'restored';
        return reply(applied({ kind: 'empty' }));
      }
      if (recovery !== 'remote' && request.method === 'get_recovery_details') return reply(applied({ kind: 'recovery_details', data: details }));
      return reply({ status: 'rejected', data: { error: { code: recovery === 'remote' && request.method !== 'list_sessions' ? 'LOCAL_ONLY' : 'SCHEMA_MIGRATION_FAILED', message: '' } } });
    }
    switch (request.method) {
      case 'get_status': return reply(applied({ kind: 'health', data: { ready: true, daemonVersion: '0.1.0-preview', update: 'none' } }));
      case 'list_sessions': return reply(applied({ kind: 'sessions', data: { items: first === null ? sessions : [], eventStreamId: 'preview', sequence: 0 } }));
      case 'subscribe_events': return reply(applied({ kind: 'subscription', data: { eventStreamId: 'preview', sequence: 0, mode: 'snapshot_required', snapshot: { update: 'none', projects: first === null ? projects : [], runtimes, sessions: first === null ? sessions : [] } } }));
      case 'discover_pi_runtime': return reply(applied({ kind: 'pi_runtime_discovery', data: { node: 'C:\\Program Files\\nodejs\\node.exe', packageRoot: 'C:\\Users\\owner\\AppData\\Roaming\\npm\\node_modules\\@earendil-works\\pi-coding-agent' } }));
      case 'register_pi_runtime':
        if (!request.params.packageRoot.endsWith('pi-coding-agent')) return reply({ status: 'rejected', data: { error: { code: 'PI_PACKAGE_UNAVAILABLE', message: '' } } });
        runtimes = [pi(true)];
        return reply(applied({ kind: 'runtimes', data: { items: runtimes } }));
      case 'list_runtimes': return reply(applied({ kind: 'runtimes', data: { items: runtimes } }));
      case 'get_project_root': return reply(applied({ kind: 'project_root', data: { projectId: request.params.projectId, cwdFence: projects.find(item => item.id === request.params.projectId)!.cwdFence, root: roots[request.params.projectId] } }));
      case 'get_session': return reply(applied({ kind: 'session', data: { session: { ...view, session: sessions.find(item => item.id === request.params.sessionId) ?? sessions[0] }, eventStreamId: 'preview', sequence } }));
      case 'get_owner_environment': return reply(applied({ kind: 'owner_environment', data: environment }));
      case 'update_owner_environment': {
        const { name, change } = request.params;
        if (name === 'SSH_AUTH_SOCK') return reply({ status: 'unknown', data: { error: { code: 'OWNER_SETTINGS_WRITE_UNKNOWN', message: '' } } });
        const found = environment.detected.find(item => item.name === name);
        environment.configured = [...environment.configured.filter(item => item.name !== name), ...(change.action === 'adopt_detected' && found ? [{ ...found, source: 'owner_override' as const, adoptable: false }] : [])];
        return reply(applied({ kind: 'owner_environment', data: environment }));
      }
      case 'list_models': return reply(applied({ kind: 'models', data: { items: models } }));
      // Plays an agent without the top two thinking levels (they come back clamped to high) whose long-named model falls back to the first.
      case 'set_thinking': return this.change(request.params.sessionId, { thinkingLevel: request.params.level === 'xhigh' || request.params.level === 'max' ? 'high' : request.params.level }, reply);
      case 'set_model': return this.change(request.params.sessionId, { model: request.params.provider === 'local-lab' ? models[0] : { provider: request.params.provider, id: request.params.modelId } }, reply);
      default: return reply({ status: 'rejected', data: { error: { code: 'RUNTIME_UNAVAILABLE', message: '' } } });
    }
  }
  private change(sessionId: string, patch: Partial<SessionSnapshot>, reply: (result: RequestResult) => void) {
    const index = sessions.findIndex(item => item.id === sessionId);
    sessions[index] = { ...sessions[index], ...patch, lastActivityAt: Date.now() };
    this.onMessage({ type: 'event', protocolVersion: 9, eventStreamId: 'preview', sequence: ++sequence, event: { kind: 'session_changed', data: { session: sessions[index] } } });
    reply(applied({ kind: 'empty' }));
  }
  async close() {}
}

createRoot(document.getElementById('root')!).render(<App client={new DaemonClient(() => new PreviewTransport())} nativeAvailable />);
