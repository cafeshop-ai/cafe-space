import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { App } from '../src/app/App';
import { loadSessionPage } from '../src/app/SessionPage';
import { DaemonClient, type Recovery, type RecoveryDetails } from '../src/state/DaemonClient';
import { RecoveryScreen } from '../src/components/RecoveryScreen';
import type { InstallationInfo } from '../src/protocol/generated/InstallationInfo';
import type { RecoveryCode } from '../src/protocol/generated/RecoveryCode';
import type { RequestResult } from '../src/protocol/generated/RequestResult';
import type { Transport } from '../src/services/transport';
import type { ClientRequest } from '../src/protocol/generated/ClientRequest';
import type { ServerMessage } from '../src/protocol/generated/ServerMessage';
import type { Response } from '../src/protocol/generated/Response';
import type { SessionSnapshot } from '../src/protocol/generated/SessionSnapshot';
import { sessionsResponse, unconfirmedHealthResponse, projectEvents } from '../src/protocol-fixtures.generated';

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
// Outside the desktop app every native call is refused; a test that needs the controller answers for it.
const notNative = async (..._args: unknown[]): Promise<unknown> => { throw Error('not native'); };
const native = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', async original => ({ ...await original<typeof import('@tauri-apps/api/core')>(), invoke: native.invoke }));
native.invoke.mockImplementation(notNative);
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
const welcome: ServerMessage = { type: 'welcome', protocolVersion: 9, daemonVersion: '0.1.0' };
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
    fake.emit({ type: 'response', protocolVersion: 9, requestId: request.requestId, result: { status: 'applied', data: { result } } });
  }
}

let root: Root | undefined;
let client: DaemonClient | undefined;
afterEach(async () => {
  await act(async () => { root?.unmount(); client?.disconnect(); });
  root = undefined; client = undefined;
  document.body.replaceChildren(); window.location.hash = '';
  native.invoke.mockReset().mockImplementation(notNative);
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
    expect(document.body.textContent).toContain('请从桌面应用连接本机守护进程');
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
    expect(document.querySelectorAll('.first-run-steps h2')[0].textContent).toBe('I Agent');
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
    for (const [route, text] of [['rooms', '将在 M3 接入协作房间'], ['remote', '将在 M2 接入其他设备'], ['settings', '守护进程版本']] as const) {
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
      type: 'event', protocolVersion: 9, eventStreamId: 'boot-1', sequence, event: { kind: 'session_changed', data: { session } },
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
    fake.emit({ type: 'response', protocolVersion: 9, requestId: fake.sent[1].requestId, result: { status: 'applied', data: { result: { kind: 'sessions', data: { items: [], eventStreamId: 'boot-1', sequence: 0 } } } } });
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
    second.emit({ type: 'event', protocolVersion: 9, eventStreamId: 'boot-1', sequence: 2, event: { kind: 'session_removed', data: { sessionId: 'absent' } } });
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
    first.emit({ type: 'event', protocolVersion: 9, eventStreamId: 'boot-1', sequence: 1, event: { kind: 'update_changed', data: { update: 'pending' } } });
    expect(client.getSnapshot().daemonUpdate).toBe('pending');
    client.disconnect(); await client.connect(); ready(second, 'resumed', 2);
    expect(client.getSnapshot().connection).toBe('connecting');
    expect(client.getSnapshot().daemonUpdate).toBe('pending');
    second.emit({ type: 'event', protocolVersion: 9, eventStreamId: 'boot-1', sequence: 2, event: { kind: 'update_changed', data: { update: 'none' } } });
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
    second.emit({ type: 'event', protocolVersion: 9, eventStreamId: 'boot-1', sequence: 1, event: { kind: 'session_changed', data: { session: { ...a, title: '已更新' } } } });
    expect(client.getSnapshot().connection).toBe('connecting');
    expect(client.getSnapshot().sessions?.map(item => item.id)).toEqual(['a', 'b']);
    expect(client.getSnapshot().sessions?.[0].title).toBe('已更新');
    second.emit({ type: 'event', protocolVersion: 9, eventStreamId: 'boot-1', sequence: 2, event: { kind: 'session_removed', data: { sessionId: 'b' } } });
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
    fake.emit({ type: 'response', protocolVersion: 9, requestId: fake.sent[1].requestId, result: { status: 'applied', data: { result: { kind: 'sessions', data: { items: [], eventStreamId: 'boot-1', sequence: 1 } } } } });
    expect(fake.sent).toHaveLength(2);
    fake.emit({ type: 'response', protocolVersion: 9, requestId: fake.sent[0].requestId, result: { status: 'applied', data: { result: { kind: 'health', data: { ready: true, daemonVersion: '0.1.0', update: 'pending' } } } } });
    expect(fake.sent[2].request.method).toBe('subscribe_events');
    expect(client.getSnapshot().connection).toBe('connecting');
    expect(client.getSnapshot().daemonUpdate).toBeNull();
    fake.emit({ type: 'response', protocolVersion: 9, requestId: fake.sent[2].requestId, result: { status: 'applied', data: { result: { kind: 'subscription', data: { eventStreamId: 'boot-1', sequence: 1, mode: 'snapshot_required', snapshot: { update: 'pending', projects: [], runtimes: [], sessions: [] } } } } } });
    expect(client.getSnapshot().connection).toBe('connected');
    expect(client.getSnapshot().daemonUpdate).toBe('pending');
  });

  it('does not mark ready until all three initial responses are confirmed', async () => {
    const fake = new FakeTransport(); client = new DaemonClient(() => fake);
    await client.connect(); fake.emit(welcome);
    fake.emit({ type: 'response', protocolVersion: 9, requestId: fake.sent[0].requestId, result: { status: 'rejected', data: { error: { code: 'NOT_READY', message: 'not ready' } } } });
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

describe('first run without the daemon (M4a)', () => {
  it('opens on the installation panel instead of a connection error, then guides the steps once connected', async () => {
    let info: InstallationInfo = { bundledVersion: '0.1.0', installed: false, registered: false, updateAvailable: false, recoveryRequired: false, restoreOnly: false, rollbackAvailable: false };
    native.invoke.mockImplementation(async (command: unknown) => { if (command === 'daemon_installation_info') return info; throw Error('not native'); });
    const fake = new FakeTransport();
    let installed = false;
    client = new DaemonClient(() => installed ? fake : Object.assign(new FakeTransport(), { open: async () => { throw Error('ENOENT'); } }));
    const element = document.createElement('div'); document.body.append(element);
    root = createRoot(element);
    await act(async () => root!.render(<App client={client} nativeAvailable />));
    await act(async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); });
    const page = () => document.querySelector('.page-content')!;
    const buttons = () => [...page().querySelectorAll('button')].map(item => item.textContent);
    expect(page().textContent).toContain('开始之前，先安装守护进程');
    expect(page().textContent).not.toContain('无法连接守护进程');
    expect(page().textContent).toContain('装好并连上守护进程后开始检测');
    expect(page().textContent).toContain('装好并连上守护进程后批准');
    expect(buttons()).toContain('安装并启用登录自启');
    expect(buttons()).not.toContain('重新连接');
    expect(page().querySelector('.first-run-intro')!.compareDocumentPosition(page().querySelector('.installation-panel')!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // Installed and started: the same page now detects agents and asks for a folder.
    info = { ...info, installed: true, registered: true };
    installed = true;
    await act(async () => { void client!.connect(); });
    await act(async () => ready(fake));
    expect(document.querySelector('.connection-status')!.textContent).toBe('已连接');
    expect(page().querySelector('.installation-panel')).toBeNull();
    expect([...page().querySelectorAll('.runtime-list .choice-title')].map(item => item.textContent)).toEqual(['Pi', 'Codex']);
    expect(page().querySelector('#project-path')).not.toBeNull();
  });
});

describe('recovery screen (UX §5.10, wire 8)', () => {
  const details: RecoveryDetails = { dataDirectory: 'C:\\Users\\owner\\AppData\\Local\\cafe-space', backupFile: 'C:\\Users\\owner\\AppData\\Local\\cafe-space\\state.sqlite3.v5.k3x9.bak', stateFiles: ['C:\\Users\\owner\\AppData\\Local\\cafe-space\\state.sqlite3', 'C:\\Users\\owner\\AppData\\Local\\cafe-space\\handover.json'] };
  const text = () => document.body.textContent!;
  const button = (label: string) => [...document.querySelectorAll('button')].find(item => item.textContent === label);
  const click = (label: string) => act(async () => button(label)!.click());
  const confirm = () => act(async () => [...document.querySelectorAll<HTMLButtonElement>('.recovery-confirmation button')].find(item => item.textContent === '恢复备份')!.click());
  beforeAll(() => {
    HTMLDialogElement.prototype.showModal = function () { this.setAttribute('open', ''); };
    HTMLDialogElement.prototype.close = function () { this.removeAttribute('open'); };
  });
  /** A daemon whose migration failed: health names it while `update` still says draining, and session reads are refused. Returns the details request. */
  function recover(fake: FakeTransport, backup = true, code: RecoveryCode = 'SCHEMA_MIGRATION_FAILED') {
    fake.emit(welcome);
    const [health, sessions] = fake.sent.slice(-2);
    const data = { ready: false, daemonVersion: '0.1.0', update: 'draining' as const, recovery: { code, fromSchema: 5, toSchema: 6, backup } };
    fake.emit({ type: 'response', protocolVersion: 9, requestId: health.requestId, result: { status: 'applied', data: { result: { kind: 'health', data } } } });
    fake.emit({ type: 'response', protocolVersion: 9, requestId: sessions.requestId, result: { status: 'rejected', data: { error: { code: 'SCHEMA_MIGRATION_FAILED', message: '' } } } });
    return fake.sent.at(-1)!;
  }
  const answer = (fake: FakeTransport, request: ClientRequest, result: RequestResult) =>
    fake.emit({ type: 'response', protocolVersion: 9, requestId: request.requestId, result });
  const read = (value: RecoveryDetails): RequestResult => ({ status: 'applied', data: { result: { kind: 'recovery_details', data: value } } });
  const restores = (fake: FakeTransport) => fake.sent.filter(item => item.request.method === 'restore_backup');
  const actions = () => [...document.querySelectorAll('.recovery-actions button')].map(item => item.textContent);
  const installed = (rollbackAvailable: boolean): InstallationInfo => ({ bundledVersion: '0.1.0', installed: true, registered: true, updateAvailable: false, recoveryRequired: false, restoreOnly: false, rollbackAvailable });

  it('lists the files and restores the backup only after an explicit confirmation, then checks again', async () => {
    const fake = await render();
    const request = await act(async () => recover(fake));
    expect(request.request).toEqual({ method: 'get_recovery_details' });
    expect(text()).toContain('需要恢复');
    expect(text()).toContain('从第 5 版升级到第 6 版时失败');
    expect(text()).toContain('正在读取涉及的文件');
    expect(text()).not.toContain('先连接本机守护进程');
    await act(async () => answer(fake, request, read(details)));
    expect([...document.querySelectorAll('.recovery-files code')].map(item => item.textContent)).toEqual([details.dataDirectory, details.backupFile, ...details.stateFiles]);
    expect([...document.querySelectorAll('.recovery-actions button')].map(item => item.textContent)).toEqual(['恢复备份', '重新检查']);
    expect(client!.getSnapshot().connection).toBe('recovery');
    await click('恢复备份');
    expect(document.querySelector('.recovery-confirmation')?.hasAttribute('open')).toBe(true);
    await click('暂不恢复');
    expect(restores(fake)).toHaveLength(0);
    await click('恢复备份'); await confirm();
    expect(fake.sent.at(-1)!.request).toEqual({ method: 'restore_backup', params: { confirm: true, code: 'SCHEMA_MIGRATION_FAILED' } });
    expect(button('正在恢复…')?.disabled).toBe(true);
    await act(async () => answer(fake, fake.sent.at(-1)!, { status: 'applied', data: { result: { kind: 'empty' } } }));
    expect(document.querySelector('.recovery-panel')).toBeNull();
    expect(text()).toContain('正在连接');
    expect(restores(fake)).toHaveLength(1);
  });

  it('after a restore offers only the rollback, which it opens in settings', async () => {
    native.invoke.mockImplementation(async (command: string) => command === 'daemon_installation_info' ? installed(true) : notNative());
    const fake = await render();
    await act(async () => answer(fake, recover(fake, true, 'ROLLBACK_REQUIRED'), read(details)));
    expect(text()).toContain('需要回到上一版');
    expect(text()).toContain('升级前的数据已恢复。这一版不能使用它，请回到上一版，之后可以再升级。');
    expect(document.querySelector('.connection-status')?.textContent).toBe('需要回退');
    expect(document.querySelector('.recovery-files, .recovery-confirmation')).toBeNull();
    expect(actions()).toEqual(['回到上一版', '重新检查']);
    await click('回到上一版');
    await act(async () => window.dispatchEvent(new HashChangeEvent('hashchange')));
    expect(window.location.hash).toBe('#/settings');
    expect(actions()).toEqual(['重新检查']);
    expect([...document.querySelectorAll('.installation-actions button')].map(item => item.textContent)).toContain('回到上一版');
    expect(native.invoke.mock.calls.some(([command]) => command === 'daemon_recover')).toBe(false);
    expect(restores(fake)).toHaveLength(0);
  });

  it('offers going back with the data untouched once the previous version has read it, and says when there is none', async () => {
    const element = document.createElement('div'); document.body.append(element);
    root = createRoot(element);
    const onRollback = vi.fn();
    const failed = { state: { code: 'SCHEMA_MIGRATION_FAILED', fromSchema: 5, toSchema: 6, backup: true }, details, error: null } as const;
    const restored = { ...failed, state: { ...failed.state, code: 'ROLLBACK_REQUIRED' } } as const;
    const screen = (recovery: Recovery, rollbackAvailable: boolean | null) => act(async () =>
      root!.render(<RecoveryScreen client={new DaemonClient(() => new FakeTransport())} recovery={recovery} local live rollbackAvailable={rollbackAvailable} onRollback={onRollback} />));
    await screen(failed, null);
    expect(actions()).toEqual(['恢复备份', '重新检查']);
    await screen(failed, true);
    expect(actions()).toEqual(['恢复备份', '回到上一版（数据未改动）', '重新检查']);
    expect(button('回到上一版（数据未改动）')!.className).toBe('secondary-button');
    await click('回到上一版（数据未改动）');
    expect(onRollback).toHaveBeenCalledTimes(1);
    await screen({ ...failed, state: { ...failed.state, backup: false } }, true);
    expect(button('回到上一版（数据未改动）')!.className).toBe('primary-button');
    await screen(restored, null);
    expect(actions()).toEqual(['重新检查']);
    expect(text()).not.toContain('请安装上一版');
    await screen(restored, false);
    expect(text()).toContain('这台电脑上没有能读取这些数据的上一版。请安装上一版 Café Space。');
    expect(actions()).toEqual(['重新检查']);
  });

  it('reports a refused or unknown restore as it is and never sends it again by itself', async () => {
    const fake = await render();
    await act(async () => answer(fake, recover(fake), read(details)));
    await click('恢复备份'); await confirm();
    await act(async () => answer(fake, fake.sent.at(-1)!, { status: 'rejected', data: { error: { code: 'RECOVERY_BACKUP_INVALID', message: '' } } }));
    expect(document.querySelector('[role="alert"]')?.textContent).toBe('守护进程没有恢复备份（RECOVERY_BACKUP_INVALID），数据没有改动。');
    expect(button('恢复备份')?.disabled).toBe(false);
    await click('恢复备份'); await confirm();
    await act(async () => answer(fake, fake.sent.at(-1)!, { status: 'unknown', data: { error: { code: 'TIMEOUT', message: '' } } }));
    expect(document.querySelector('[role="alert"]')?.textContent).toContain('不确定备份是否已经恢复');
    expect(button('恢复备份')?.disabled).toBe(true);
    expect(restores(fake)).toHaveLength(2);
    expect(text()).toContain('需要恢复数据');
  });

  it('only gives the reason where the files cannot be read, and without a backup offers no restore', async () => {
    const fake = await render();
    const request = await act(async () => recover(fake));
    await act(async () => answer(fake, request, { status: 'rejected', data: { error: { code: 'LOCAL_ONLY', message: '' } } }));
    expect(text()).toContain('需要在这台电脑上处理');
    expect(document.querySelector('.recovery-files, .recovery-confirmation')).toBeNull();
    expect(button('恢复备份')).toBeUndefined();
    await act(async () => client!.connect());
    const again = await act(async () => recover(fake, false));
    await act(async () => answer(fake, again, read({ ...details, backupFile: null })));
    expect(text()).toContain('备份没有保留下来');
    expect(text()).toContain('cafe-space status --json');
    expect(text()).not.toContain('备份文件');
    expect(button('恢复备份')).toBeUndefined();
    expect(document.querySelectorAll('.recovery-files code')).toHaveLength(3);
    await act(async () => client!.connect());
    await act(async () => answer(fake, recover(fake, true, 'ROLLBACK_REQUIRED'), { status: 'rejected', data: { error: { code: 'LOCAL_ONLY', message: '' } } }));
    expect(text()).toContain('需要在这台电脑上处理');
    expect(button('回到上一版')).toBeUndefined();
  });

  it('points to the app or the CLI when this computer is not using the desktop app', async () => {
    const element = document.createElement('div'); document.body.append(element);
    root = createRoot(element);
    const recovery = { state: { code: 'SCHEMA_MIGRATION_FAILED', fromSchema: 5, toSchema: 6, backup: true }, details, error: null } as const;
    await act(async () => root!.render(<RecoveryScreen client={new DaemonClient(() => new FakeTransport())} recovery={recovery} local={false} live rollbackAvailable={null} />));
    expect(text()).toContain('请在这台电脑的 Café Space 应用里恢复备份');
    expect(text()).toContain('cafe-space recovery restore');
    expect(button('恢复备份')).toBeUndefined();
    await act(async () => root!.render(<RecoveryScreen client={new DaemonClient(() => new FakeTransport())} recovery={{ ...recovery, state: { ...recovery.state, code: 'ROLLBACK_REQUIRED' } }} local={false} live rollbackAvailable={true} onRollback={() => {}} />));
    expect(text()).toContain('请在这台电脑的 Café Space 应用里回到上一版');
    expect(text()).toContain('cafe-space update rollback');
    expect(button('回到上一版')).toBeUndefined();
  });

  it('keeps the recovery screen when its files cannot be read in time or the connection drops', async () => {
    vi.useFakeTimers();
    const fake = await render();
    await act(async () => { recover(fake); vi.advanceTimersByTime(10000); });
    expect(client!.getSnapshot()).toMatchObject({ connection: 'recovery', error: null, recovery: { details: null, error: 'TIMEOUT' } });
    await act(async () => { await client!.connect(); recover(fake); fake.onClose(); });
    expect(client!.getSnapshot()).toMatchObject({ connection: 'disconnected', recovery: { details: null, error: 'CONNECTION_LOST' } });
    expect(text()).toContain('需要恢复数据');
    expect(text()).toContain('连接已断开，点“重新检查”后再恢复');
    expect(button('恢复备份')?.disabled).toBe(true);
    expect(text()).not.toContain('守护进程安装与更新');
  });
});
