// Dev-only layout preview (`pnpm dev`, then open /preview.html): the real App against a scripted
// in-memory daemon. It is not part of the production build and never connects anywhere.
import { createRoot } from 'react-dom/client';
import { App } from '../app/App';
import { DaemonClient } from '../state/DaemonClient';
import type { Transport } from '../services/transport';
import type { ClientRequest } from '../protocol/generated/ClientRequest';
import type { RequestResult } from '../protocol/generated/RequestResult';
import type { ServerMessage } from '../protocol/generated/ServerMessage';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { SessionView } from '../protocol/generated/SessionView';
import '../styles.css';

const capabilities = { modelControl: false, thinkingControl: false, deliveryControl: true, sessionControl: true, approvalForSession: false, toolApproval: 'never_asks' as const };
const session = (id: string, title: string, projectId: string, state: SessionSnapshot['state'], extra: Partial<SessionSnapshot> = {}): SessionSnapshot =>
  ({ id, title, projectId, runtimeId: 'pi', cwdFence: 'fence-a', capabilities, mode: 'new', state, streamId: `stream-${id}`, ...extra });
const sessions = [
  session('s1', 'Refactor the menu loader so cold starts stay under a second', 'p1', { state: 'running' }, { activeRunId: 'run-1', model: { provider: 'anthropic', id: 'claude-opus-5-5' } }),
  session('s2', '整理菜单与价格', 'p1', { state: 'ready' }, { thinkingLevel: 'medium', capabilities: { ...capabilities, thinkingControl: true } }),
  session('s3', 'Release notes', 'p2', { state: 'failed', data: { code: 'PROCESS_SPAWN_FAILED' } }),
  session('s4', '旧会话', 'gone', { state: 'stopped' }, { stopReason: 'DAEMON_RESTARTED' }),
];
const projects = [{ id: 'p1', name: '茶馆 Café menu', cwdFence: 'fence-a' }, { id: 'p2', name: 'Long project name that keeps going to test wrapping on narrow screens', cwdFence: 'fence-b' }];
const runtimes = [{ id: 'pi', name: 'Pi', version: '1.5.0', available: true, toolApproval: 'never_asks' as const }, { id: 'codex', name: 'Codex', available: false, toolApproval: 'unknown' as const }];
const longLine = 'const result = await loadMenu({ region: "east", includeSeasonal: true, cacheKey: "menu:east:2026-11", retries: 3 });';
const view: SessionView = {
  session: sessions[0], historyTruncated: true, approvals: [{ id: 'a1', summary: 'Run `npm install --save-dev vitest@4.1.12` in the project folder' }],
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
let sequence = 0;
const applied = (result: Extract<RequestResult, { status: 'applied' }>['data']['result']): RequestResult => ({ status: 'applied', data: { result } });

class PreviewTransport implements Transport {
  private onMessage: (message: ServerMessage) => void = () => {};
  async open(onMessage: (message: ServerMessage) => void) {
    this.onMessage = onMessage;
    queueMicrotask(() => onMessage({ type: 'welcome', protocolVersion: 5, daemonVersion: '0.1.0-preview' }));
  }
  async send({ requestId, request }: ClientRequest) {
    const reply = (result: RequestResult) => queueMicrotask(() => this.onMessage({ type: 'response', protocolVersion: 5, requestId, result }));
    switch (request.method) {
      case 'get_status': return reply(applied({ kind: 'health', data: { ready: true, daemonVersion: '0.1.0-preview', update: 'none' } }));
      case 'list_sessions': return reply(applied({ kind: 'sessions', data: { items: sessions, eventStreamId: 'preview', sequence: 0 } }));
      case 'subscribe_events': return reply(applied({ kind: 'subscription', data: { eventStreamId: 'preview', sequence: 0, mode: 'snapshot_required', snapshot: { update: 'none', projects, runtimes, sessions } } }));
      case 'list_runtimes': return reply(applied({ kind: 'runtimes', data: { items: runtimes } }));
      case 'get_project_root': return reply(applied({ kind: 'project_root', data: { projectId: request.params.projectId, cwdFence: projects.find(item => item.id === request.params.projectId)!.cwdFence, root: `/Users/alex/Developer/clients/${request.params.projectId}/a-rather-long-folder-name` } }));
      case 'get_session': return reply(applied({ kind: 'session', data: { session: { ...view, session: sessions.find(item => item.id === request.params.sessionId) ?? sessions[0] }, eventStreamId: 'preview', sequence } }));
      case 'set_thinking': {
        // Plays an agent without the top two levels: they come back clamped to high.
        const index = sessions.findIndex(item => item.id === request.params.sessionId);
        sessions[index] = { ...sessions[index], thinkingLevel: request.params.level === 'xhigh' || request.params.level === 'max' ? 'high' : request.params.level };
        this.onMessage({ type: 'event', protocolVersion: 5, eventStreamId: 'preview', sequence: ++sequence, event: { kind: 'session_changed', data: { session: sessions[index] } } });
        return reply(applied({ kind: 'empty' }));
      }
      default: return reply({ status: 'rejected', data: { error: { code: 'RUNTIME_UNAVAILABLE', message: '' } } });
    }
  }
  async close() {}
}

createRoot(document.getElementById('root')!).render(<App client={new DaemonClient(() => new PreviewTransport())} nativeAvailable />);
