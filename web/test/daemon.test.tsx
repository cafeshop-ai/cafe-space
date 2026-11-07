import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { App } from '../src/app/App';
import { loadSessionPage } from '../src/app/SessionPage';
import { DaemonClient } from '../src/state/DaemonClient';
import type { Transport } from '../src/services/transport';
import type { ClientRequest } from '../src/protocol/generated/ClientRequest';
import type { ServerMessage } from '../src/protocol/generated/ServerMessage';
import type { Response } from '../src/protocol/generated/Response';
import type { SessionSnapshot } from '../src/protocol/generated/SessionSnapshot';
import { sessionsResponse, unconfirmedHealthResponse, projectEvents } from '../src/protocol-fixtures.generated';

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
// A connected app fetches the session page chunk while idle; on a slow runner that cold load could land inside a test.
beforeAll(() => loadSessionPage(), 60000);
class FakeTransport implements Transport {
  sent: ClientRequest[] = [];
  onMessage: (message: ServerMessage) => void = () => {};
  onClose = () => {};
  async open(onMessage: (message: ServerMessage) => void, onClose: () => void) { this.onMessage = onMessage; this.onClose = onClose; }
  async send(request: ClientRequest) { this.sent.push(request); }
  async close() { this.onClose(); }
  emit(message: ServerMessage) { this.onMessage(message); }
}
const welcome: ServerMessage = { type: 'welcome', protocolVersion: 6, daemonVersion: '0.1.0' };
function ready(fake: FakeTransport, mode: 'snapshot_required' | 'resumed' = 'snapshot_required', highWater = 0, sessions: SessionSnapshot[] = []) {
  fake.emit(welcome);
  expect(fake.sent.map(item => item.request.method)).toEqual(['get_status', 'list_sessions']);
  for (const request of fake.sent) {
    let result: Response;
    switch (request.request.method) {
      case 'get_status': result = { kind: 'health', data: { ready: true, daemonVersion: '0.1.0', update: 'none' } }; break;
      case 'list_sessions': result = { kind: 'sessions', data: { items: sessions, eventStreamId: 'boot-1', sequence: 0 } }; break;
      default: result = { kind: 'subscription', data: { eventStreamId: 'boot-1', sequence: highWater, mode, ...(mode === 'snapshot_required' ? { snapshot: { update: 'none', projects: [], runtimes: [], sessions } } : {}) } };
    }
    fake.emit({ type: 'response', protocolVersion: 6, requestId: request.requestId, result: { status: 'applied', data: { result } } });
  }
}

let root: Root | undefined;
let client: DaemonClient | undefined;
afterEach(async () => {
  await act(async () => { root?.unmount(); client?.disconnect(); });
  root = undefined; client = undefined;
  document.body.replaceChildren(); window.location.hash = '';
  vi.useRealTimers();
});
async function render(nativeAvailable = true) {
  const fake = new FakeTransport();
  client = new DaemonClient(() => fake);
  const element = document.createElement('div'); document.body.append(element);
  root = createRoot(element);
  await act(async () => root!.render(<App client={client} nativeAvailable={nativeAvailable} />));
  return fake;
}

describe('M0 workspace', () => {
  it('does not claim zero sessions or connect anywhere in browser preview', async () => {
    const fake = await render(false);
    expect(document.body.textContent).toContain('未连上');
    expect(document.body.textContent).toContain('尚未读取');
    expect(document.body.textContent).not.toContain('0 个会话');
    expect(document.body.textContent).toContain('浏览器预览不会连接云端');
    expect(fake.sent).toHaveLength(0);
  });

  it('renders connecting, real empty sessions and offline state with Chinese round-trip', async () => {
    const fake = await render();
    expect(document.body.textContent).toContain('正在连接');
    await act(async () => ready(fake));
    expect(document.body.textContent).toContain('已连接');
    expect(document.body.textContent).toContain('0 个会话');
    // No project yet: first run (UX §5.1) instead of an empty list.
    const heading = document.querySelectorAll('.first-run-steps h2')[1].textContent!;
    expect(new TextDecoder('utf-8', { fatal: true }).decode(new TextEncoder().encode(heading))).toBe('II 项目');
    expect(document.body.textContent).toContain('不会向任何模型发送内容');
    await act(async () => fake.onClose());
    expect(document.body.textContent).toContain('未连上');
    expect(document.body.textContent).toContain('当前离线');
    const button = [...document.querySelectorAll('button')].find(item => item.textContent === '新建会话')!;
    expect(button.disabled).toBe(true);
    expect(document.body.textContent).toContain('连接守护进程后可用');
  });

  it('preserves version mismatch rather than replacing it with offline', async () => {
    const fake = await render();
    await act(async () => fake.emit({ type: 'version_mismatch', daemonVersion: '9.0.0', supportedProtocolVersions: [3] }));
    expect(document.body.textContent).toContain('版本不符');
    expect(document.body.textContent).toContain('协议版本不符');
    expect(fake.sent).toHaveLength(0);
  });

  it('routes rooms and remote access to truthful milestone placeholders', async () => {
    await render(false);
    for (const [route, text] of [['rooms', '将在 M3 接入协作房间'], ['remote', '将在 M2 接入其他设备'], ['settings', '仅当前系统用户可用']] as const) {
      await act(async () => { window.location.hash = `#/${route}`; window.dispatchEvent(new HashChangeEvent('hashchange')); });
      expect(document.body.textContent).toContain(text);
      expect(document.querySelector(`a[aria-current="page"]`)?.getAttribute('href')).toBe(`#/${route}`);
    }
  });
});

describe('daemon initialization and cursors', () => {
  it('replaces activity metadata on live events, replay and fresh snapshots', async () => {
    const peers = [new FakeTransport(), new FakeTransport(), new FakeTransport()];
    const [first, second, third] = peers;
    client = new DaemonClient(() => peers.shift()!);
    const base: SessionSnapshot = sessionsResponse.result.data.result.data.items[0];
    await client.connect();
    ready(first, 'snapshot_required', 0, [{ ...base, lastActivityAt: 100, preview: 'old' }]);
    const emit = (peer: FakeTransport, sequence: number, session: SessionSnapshot) => peer.emit({
      type: 'event', protocolVersion: 6, eventStreamId: 'boot-1', sequence, event: { kind: 'session_changed', data: { session } },
    });
    emit(first, 1, { ...base, lastActivityAt: 101, preview: 'new' });
    expect(client.getSnapshot().sessions?.[0]).toEqual({ ...base, lastActivityAt: 101, preview: 'new' });
    client.disconnect(); await client.connect(); ready(second, 'resumed', 2);
    expect(client.getSnapshot().connection).toBe('connecting');
    emit(second, 2, { ...base, lastActivityAt: 102 });
    expect(client.getSnapshot().connection).toBe('connected');
    expect(client.getSnapshot().sessions?.[0]).toEqual({ ...base, lastActivityAt: 102 });
    expect(client.getSnapshot().sessions?.[0]).not.toHaveProperty('preview');
    client.disconnect(); await client.connect(); ready(third, 'snapshot_required', 3, [base]);
    expect(client.getSnapshot().sessions?.[0]).toEqual(base);
    expect(client.getSnapshot().sessions?.[0]).not.toHaveProperty('lastActivityAt');
  });
  it('retains projects across reconnect and applies fence rotation/removal in sequence', async () => {
    const first = new FakeTransport(), second = new FakeTransport();
    const transports = [first, second];
    client = new DaemonClient(() => transports.shift()!);
    await client.connect(); ready(first);
    expect(client.getSnapshot().projects).toEqual([]);
    first.emit({ ...projectEvents[0], eventStreamId: 'boot-1' });
    expect(client.getSnapshot().projects?.[0].cwdFence).toBe('cwd-fixture');
    client.disconnect(); await client.connect(); ready(second, 'resumed', 3);
    expect(client.getSnapshot().projects?.[0].cwdFence).toBe('cwd-fixture');
    second.emit({ ...projectEvents[1], eventStreamId: 'boot-1' });
    expect(client.getSnapshot().projects?.[0].cwdFence).toBe('cwd-next');
    second.emit({ ...projectEvents[2], eventStreamId: 'boot-1' });
    expect(client.getSnapshot().projects).toEqual([]);
    expect(client.getSnapshot().connection).toBe('connected');
  });
  it('explains an unconfirmed handover and ignores already-buffered replies after failure', async () => {
    const fake = new FakeTransport(); client = new DaemonClient(() => fake);
    await client.connect(); fake.emit(welcome);
    fake.emit({ ...unconfirmedHealthResponse, requestId: fake.sent[0].requestId });
    fake.emit({ type: 'response', protocolVersion: 6, requestId: fake.sent[1].requestId, result: { status: 'applied', data: { result: { kind: 'sessions', data: { items: [], eventStreamId: 'boot-1', sequence: 0 } } } } });
    expect(client.getSnapshot().connection).toBe('disconnected');
    expect(client.getSnapshot().error).toContain('等待交接确认');
    expect(client.getSnapshot().sessions).toBeNull();
    expect(fake.sent).toHaveLength(2);
  });
  it('subscribes from a snapshot, resumes its cursor, and resets on a gap without replay', async () => {
    const first = new FakeTransport(), second = new FakeTransport(), third = new FakeTransport();
    const transports = [first, second, third];
    client = new DaemonClient(() => transports.shift()!);
    await client.connect(); ready(first);
    expect(first.sent[2].request).toEqual({ method: 'subscribe_events', params: { after: null } });
    client.disconnect(); await client.connect(); ready(second, 'resumed');
    expect(second.sent[2].request).toEqual({ method: 'subscribe_events', params: { after: { eventStreamId: 'boot-1', sequence: 0 } } });
    second.emit({ type: 'event', protocolVersion: 6, eventStreamId: 'boot-1', sequence: 2, event: { kind: 'session_removed', data: { sessionId: 'absent' } } });
    expect(client.getSnapshot().connection).toBe('disconnected');
    expect(client.getSnapshot().error).toContain('完整快照');
    await client.connect(); ready(third);
    expect(third.sent[2].request).toEqual({ method: 'subscribe_events', params: { after: null } });
    expect([...first.sent, ...second.sent, ...third.sent].every(item => ['get_status', 'list_sessions', 'subscribe_events'].includes(item.request.method))).toBe(true);
  });

  it('retains the update baseline during resumed replay instead of replacing it with current health', async () => {
    const first = new FakeTransport(), second = new FakeTransport();
    const transports = [first, second];
    client = new DaemonClient(() => transports.shift()!);
    await client.connect(); ready(first);
    expect(client.getSnapshot().daemonUpdate).toBe('none');
    first.emit({ type: 'event', protocolVersion: 6, eventStreamId: 'boot-1', sequence: 1, event: { kind: 'update_changed', data: { update: 'pending' } } });
    expect(client.getSnapshot().daemonUpdate).toBe('pending');
    client.disconnect(); await client.connect(); ready(second, 'resumed', 2);
    expect(client.getSnapshot().connection).toBe('connecting');
    expect(client.getSnapshot().daemonUpdate).toBe('pending');
    second.emit({ type: 'event', protocolVersion: 6, eventStreamId: 'boot-1', sequence: 2, event: { kind: 'update_changed', data: { update: 'none' } } });
    expect(client.getSnapshot().connection).toBe('connected');
    expect(client.getSnapshot().daemonUpdate).toBe('none');
  });

  it('keeps the applied baseline and after cursor while two replayed events catch up to H', async () => {
    const first = new FakeTransport(), second = new FakeTransport(), third = new FakeTransport();
    const transports = [first, second, third];
    client = new DaemonClient(() => transports.shift()!);
    const template = sessionsResponse.result.data.result.data.items[0];
    const a: SessionSnapshot = { ...template, id: 'a', title: '原有会话' };
    const b: SessionSnapshot = { ...template, id: 'b', title: '第二个会话' };
    await client.connect(); ready(first, 'snapshot_required', 0, [a, b]);
    client.disconnect(); await client.connect();
    // This list is stale/empty; resumed replay must start from the already applied sessions.
    ready(second, 'resumed', 2, []);
    expect(client.getSnapshot().connection).toBe('connecting');
    expect(client.getSnapshot().sessions?.map(item => item.id)).toEqual(['a', 'b']);
    second.emit({ type: 'event', protocolVersion: 6, eventStreamId: 'boot-1', sequence: 1, event: { kind: 'session_changed', data: { session: { ...a, title: '已更新' } } } });
    expect(client.getSnapshot().connection).toBe('connecting');
    expect(client.getSnapshot().sessions?.map(item => item.id)).toEqual(['a', 'b']);
    expect(client.getSnapshot().sessions?.[0].title).toBe('已更新');
    second.emit({ type: 'event', protocolVersion: 6, eventStreamId: 'boot-1', sequence: 2, event: { kind: 'session_removed', data: { sessionId: 'b' } } });
    expect(client.getSnapshot().connection).toBe('connected');
    expect(client.getSnapshot().error).toBeNull();
    expect(client.getSnapshot().sessions?.map(item => item.id)).toEqual(['a']);
    client.disconnect(); await client.connect(); ready(third, 'resumed', 2);
    expect(third.sent[2].request).toEqual({ method: 'subscribe_events', params: { after: { eventStreamId: 'boot-1', sequence: 2 } } });
  });

  it('refuses resumed without a retained baseline and requires a snapshot', async () => {
    const fake = new FakeTransport(); client = new DaemonClient(() => fake);
    await client.connect(); ready(fake, 'resumed');
    expect(client.getSnapshot().connection).toBe('disconnected');
    expect(client.getSnapshot().sessions).toBeNull();
  });

  it('waits for health and list before subscribing and never claims ready before the snapshot', async () => {
    const fake = new FakeTransport(); client = new DaemonClient(() => fake);
    await client.connect(); fake.emit(welcome);
    expect(fake.sent.map(item => item.request.method)).toEqual(['get_status', 'list_sessions']);
    fake.emit({ type: 'response', protocolVersion: 6, requestId: fake.sent[1].requestId, result: { status: 'applied', data: { result: { kind: 'sessions', data: { items: [], eventStreamId: 'boot-1', sequence: 1 } } } } });
    expect(fake.sent).toHaveLength(2);
    fake.emit({ type: 'response', protocolVersion: 6, requestId: fake.sent[0].requestId, result: { status: 'applied', data: { result: { kind: 'health', data: { ready: true, daemonVersion: '0.1.0', update: 'pending' } } } } });
    expect(fake.sent[2].request.method).toBe('subscribe_events');
    expect(client.getSnapshot().connection).toBe('connecting');
    expect(client.getSnapshot().daemonUpdate).toBeNull();
    fake.emit({ type: 'response', protocolVersion: 6, requestId: fake.sent[2].requestId, result: { status: 'applied', data: { result: { kind: 'subscription', data: { eventStreamId: 'boot-1', sequence: 1, mode: 'snapshot_required', snapshot: { update: 'pending', projects: [], runtimes: [], sessions: [] } } } } } });
    expect(client.getSnapshot().connection).toBe('connected');
    expect(client.getSnapshot().daemonUpdate).toBe('pending');
  });

  it('does not mark ready until all three initial responses are confirmed', async () => {
    const fake = new FakeTransport(); client = new DaemonClient(() => fake);
    await client.connect(); fake.emit(welcome);
    fake.emit({ type: 'response', protocolVersion: 6, requestId: fake.sent[0].requestId, result: { status: 'rejected', data: { error: { code: 'NOT_READY', message: 'not ready' } } } });
    expect(client.getSnapshot().connection).toBe('disconnected');
    expect(client.getSnapshot().sessions).toBeNull();
  });

  it('times out a silent daemon without automatic reconnect', async () => {
    vi.useFakeTimers();
    const fake = new FakeTransport(); const factory = vi.fn(() => fake);
    client = new DaemonClient(factory); await client.connect();
    await vi.advanceTimersByTimeAsync(10000);
    expect(client.getSnapshot().connection).toBe('disconnected');
    expect(client.getSnapshot().error).toContain('超时');
    expect(factory).toHaveBeenCalledTimes(1);
  });
});
