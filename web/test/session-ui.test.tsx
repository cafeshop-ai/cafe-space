import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { App } from '../src/app/App';
import { DaemonClient } from '../src/state/DaemonClient';
import { applySessionEvent, MAX_MESSAGES, type SessionDetail } from '../src/state/sessionDetail';
import type { Transport } from '../src/services/transport';
import type { ClientRequest } from '../src/protocol/generated/ClientRequest';
import type { Request } from '../src/protocol/generated/Request';
import type { RequestResult } from '../src/protocol/generated/RequestResult';
import type { ServerMessage } from '../src/protocol/generated/ServerMessage';
import type { SessionSnapshot } from '../src/protocol/generated/SessionSnapshot';
import type { SessionView } from '../src/protocol/generated/SessionView';
import type { TranscriptMessage } from '../src/protocol/generated/TranscriptMessage';
import { sessionResponse } from '../src/protocol-fixtures.generated';

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function () { this.setAttribute('open', ''); };
  HTMLDialogElement.prototype.close = function () { this.removeAttribute('open'); this.dispatchEvent(new Event('close')); };
});

const fixtureView = (sessionResponse as Extract<ServerMessage, { type: 'response' }>).result as Extract<RequestResult, { status: 'applied' }>;
const baseView = (fixtureView.data.result as { kind: 'session'; data: { session: SessionView } }).data.session;
const base: SessionSnapshot = { ...baseView.session };
const text = (id: string, value: string, role: TranscriptMessage['role'] = 'assistant'): TranscriptMessage => ({ id, role, parts: [{ kind: 'text', data: { text: value } }], status: 'complete' });
const applied = (result: Extract<RequestResult, { status: 'applied' }>['data']['result']): RequestResult => ({ status: 'applied', data: { result } });

class FakeTransport implements Transport {
  sent: ClientRequest[] = [];
  onMessage: (message: ServerMessage) => void = () => {};
  onClose = () => {};
  /** Answers user operations automatically; return undefined to leave a request pending. */
  handler: (request: Request) => RequestResult | undefined = () => undefined;
  async open(onMessage: (message: ServerMessage) => void, onClose: () => void) { this.onMessage = onMessage; this.onClose = onClose; }
  async send(request: ClientRequest) {
    this.sent.push(request);
    if (!request.requestId.includes('-op-')) return;
    const result = this.handler(request.request);
    if (result) queueMicrotask(() => this.reply(request.requestId, result));
  }
  async close() { this.onClose(); }
  reply(requestId: string, result: RequestResult) { this.onMessage({ type: 'response', protocolVersion: 2, requestId, result }); }
  event(sequence: number, event: Extract<ServerMessage, { type: 'event' }>['event']) { this.onMessage({ type: 'event', protocolVersion: 2, eventStreamId: 'boot-1', sequence, event }); }
  ops(method?: Request['method']) { return this.sent.filter(item => item.requestId.includes('-op-') && (!method || item.request.method === method)); }
}

function handshake(fake: FakeTransport, sessions: SessionSnapshot[], projects = [{ id: 'project-fixture', name: '茶馆项目', cwdFence: 'cwd-fixture' }], runtimes = [{ id: 'pi', name: 'Pi', version: '1.5.0', available: true }]) {
  fake.onMessage({ type: 'welcome', protocolVersion: 2, daemonVersion: '0.1.0' });
  for (const request of [...fake.sent]) {
    if (request.request.method === 'get_status') fake.reply(request.requestId, applied({ kind: 'health', data: { ready: true, daemonVersion: '0.1.0', update: 'none' } }));
    if (request.request.method === 'list_sessions') fake.reply(request.requestId, applied({ kind: 'sessions', data: { items: sessions, eventStreamId: 'boot-1', sequence: 0 } }));
  }
  const subscribe = fake.sent.find(item => item.request.method === 'subscribe_events')!;
  fake.reply(subscribe.requestId, applied({ kind: 'subscription', data: { eventStreamId: 'boot-1', sequence: 0, mode: 'snapshot_required', snapshot: { update: 'none', projects, runtimes, sessions } } }));
}

let root: Root | undefined;
let client: DaemonClient | undefined;
afterEach(async () => {
  await act(async () => { root?.unmount(); client?.disconnect(); });
  root = undefined; client = undefined;
  document.body.replaceChildren(); window.location.hash = '';
  vi.useRealTimers();
});
async function render(fake: FakeTransport, sessions: SessionSnapshot[], hash = '') {
  window.location.hash = hash;
  client = new DaemonClient(() => fake);
  const element = document.createElement('div'); document.body.append(element);
  root = createRoot(element);
  await act(async () => root!.render(<App client={client} nativeAvailable />));
  await act(async () => handshake(fake, sessions));
  await act(async () => { await Promise.resolve(); });
}
const button = (label: string) => [...document.querySelectorAll('button')].find(item => item.textContent === label) as HTMLButtonElement | undefined;
const flush = () => act(async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); });

describe('open session reducer', () => {
  const detail: SessionDetail = { view: { ...baseView, messages: [text('m1', '你好')] }, eventStreamId: 'boot-1', sequence: 3 };
  it('replaces by id, appends new messages and ignores events it already reflects', () => {
    const replaced = applySessionEvent(detail, 'boot-1', 4, { kind: 'message_changed', data: { sessionId: base.id, message: text('m1', '你好，世界') } });
    expect(replaced !== 'stale' && replaced !== 'removed' && replaced.view.messages.map(item => item.parts[0])).toEqual([{ kind: 'text', data: { text: '你好，世界' } }]);
    const appended = applySessionEvent(detail, 'boot-1', 4, { kind: 'message_changed', data: { sessionId: base.id, message: text('m2', '第二条') } });
    expect(appended !== 'stale' && appended !== 'removed' && appended.view.messages.map(item => item.id)).toEqual(['m1', 'm2']);
    expect(applySessionEvent(detail, 'boot-1', 3, { kind: 'message_changed', data: { sessionId: base.id, message: text('m9', 'old') } })).toBe(detail);
    const other = applySessionEvent(detail, 'boot-1', 4, { kind: 'message_changed', data: { sessionId: 'other', message: text('m3', 'x') } });
    expect(other !== 'stale' && other !== 'removed' && [other.sequence, other.view.messages.length]).toEqual([4, 1]);
  });
  it('bounds history, tracks approvals, and reports stale streams and removal', () => {
    let current: SessionDetail = { ...detail, view: { ...detail.view, messages: Array.from({ length: MAX_MESSAGES }, (_, i) => text(`m${i}`, `${i}`)) } };
    const next = applySessionEvent(current, 'boot-1', 4, { kind: 'message_changed', data: { sessionId: base.id, message: text('new', 'new') } });
    if (next === 'stale' || next === 'removed') throw new Error('unexpected');
    expect(next.view.messages).toHaveLength(MAX_MESSAGES);
    expect(next.view.messages[0].id).toBe('m1');
    expect(next.view.historyTruncated).toBe(true);
    current = next;
    const asked = applySessionEvent(current, 'boot-1', 5, { kind: 'approval_required', data: { sessionId: base.id, approval: { id: 'a1', summary: 'rm -rf build' } } });
    if (asked === 'stale' || asked === 'removed') throw new Error('unexpected');
    expect(asked.view.approvals.map(item => item.id)).toEqual(['a1']);
    const resolved = applySessionEvent(asked, 'boot-1', 6, { kind: 'approval_resolved', data: { sessionId: base.id, approvalId: 'a1', decision: 'deny' } });
    expect(resolved !== 'stale' && resolved !== 'removed' && resolved.view.approvals).toEqual([]);
    expect(applySessionEvent(current, 'boot-2', 1, { kind: 'notice', data: { code: 'X', message: '' } })).toBe('stale');
    expect(applySessionEvent(current, 'boot-1', 7, { kind: 'session_removed', data: { sessionId: base.id } })).toBe('removed');
  });
});

describe('user operations', () => {
  it('rejects before connecting, resolves outcomes, and never fails the connection on a late reply', async () => {
    vi.useFakeTimers();
    const fake = new FakeTransport();
    client = new DaemonClient(() => fake);
    expect(await client.call({ method: 'list_runtimes' })).toMatchObject({ status: 'rejected', data: { error: { code: 'NOT_CONNECTED' } } });
    await client.connect(); handshake(fake, []);
    const pending = client.call({ method: 'list_runtimes' });
    await vi.advanceTimersByTimeAsync(30000);
    expect(await pending).toMatchObject({ status: 'unknown', data: { error: { code: 'TIMEOUT' } } });
    fake.reply(fake.ops('list_runtimes')[0].requestId, applied({ kind: 'runtimes', data: { items: [] } }));
    expect(client.getSnapshot().connection).toBe('connected');
    const lost = client.call({ method: 'list_runtimes' });
    await fake.close();
    expect(await lost).toMatchObject({ status: 'unknown', data: { error: { code: 'CONNECTION_LOST' } } });
  });
});

describe('sessions UI', () => {
  it('groups sessions by project and keeps sessions of a removed project as history', async () => {
    const fake = new FakeTransport();
    await render(fake, [{ ...base, id: 's1', title: '整理菜单' }, { ...base, id: 's2', projectId: 'gone', title: '旧会话', state: { state: 'stopped' }, stopReason: 'SESSION_CLOSED' }]);
    const groups = [...document.querySelectorAll('.session-group')].map(item => item.getAttribute('aria-label'));
    expect(groups).toEqual(['茶馆项目', '已移除的项目']);
    expect(document.querySelector('a[href="#/sessions/s1"]')?.textContent).toContain('整理菜单');
    expect(document.body.textContent).toContain('空闲');
  });

  it('renders the transcript, applies live messages, and sends fenced prompts without losing rejected drafts', async () => {
    const fake = new FakeTransport();
    const view: SessionView = { ...baseView, messages: [text('u1', '帮我看看', 'user'), { id: 'a1', role: 'assistant', status: 'complete', parts: [
      { kind: 'thinking', data: { text: '先读文件' } }, { kind: 'tool_call', data: { id: 't1', name: 'read', arguments: '{"path":"README.md"}', output: '# 茶', status: 'complete' } }, { kind: 'text', data: { text: '看完了。' } },
    ] }] };
    let promptResult: RequestResult = { status: 'rejected', data: { error: { code: 'STALE_STREAM', message: '' } } };
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: view, eventStreamId: 'boot-1', sequence: 0 } }) :
      request.method === 'prompt' ? promptResult : undefined;
    await render(fake, [base], '#/sessions/session-fixture');
    await flush();
    expect(fake.ops('get_session')[0].request).toEqual({ method: 'get_session', params: { sessionId: 'session-fixture' } });
    expect(document.querySelector('.thinking summary')?.textContent).toBe('思考过程');
    expect(document.querySelector('.tool-card .tool-name')?.textContent).toBe('read');
    expect(document.body.textContent).toContain('看完了。');
    await act(async () => fake.event(1, { kind: 'message_changed', data: { sessionId: base.id, message: text('a2', '还有一件事') } }));
    expect(document.body.textContent).toContain('还有一件事');

    const input = document.querySelector('textarea')!;
    await act(async () => { Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(input, '请继续'); input.dispatchEvent(new Event('input', { bubbles: true })); });
    // Enter while an IME is composing must not send.
    await act(async () => { input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, isComposing: true })); });
    expect(fake.ops('prompt')).toHaveLength(0);
    await act(async () => { input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); });
    await flush();
    expect(fake.ops('prompt')[0].request).toEqual({ method: 'prompt', params: { sessionId: base.id, expectedStreamId: 'stream-fixture', expectedCwdFence: 'cwd-fixture', content: '请继续', delivery: 'follow_up' } });
    expect(input.value).toBe('请继续');
    expect(document.body.textContent).toContain('未完成');
    promptResult = { status: 'dispatched', data: { operationId: 'op-1' } };
    await act(async () => { button('发送')!.click(); });
    await flush();
    expect(input.value).toBe('');
    expect(document.body.textContent).toContain('已发送');
  });

  it('offers steer, queue and stop only as the session allows, and answers approvals', async () => {
    const fake = new FakeTransport();
    const running: SessionSnapshot = { ...base, state: { state: 'running' }, activeRunId: 'run-1', capabilities: { ...base.capabilities, deliveryControl: false, approvalForSession: false } };
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: { ...baseView, session: running, approvals: [{ id: 'ap1', summary: 'npm install' }] }, eventStreamId: 'boot-1', sequence: 0 } }) :
      request.method === 'respond_to_approval' || request.method === 'cancel' ? { status: 'dispatched', data: { operationId: 'x' } } : undefined;
    await render(fake, [running], '#/sessions/session-fixture');
    await flush();
    expect(document.body.textContent).toContain('Agent 空闲后再发送');
    expect(button('引导')).toBeUndefined();
    expect(button('本会话内允许')).toBeUndefined();
    await act(async () => { button('允许一次')!.click(); });
    await flush();
    expect(fake.ops('respond_to_approval')[0].request).toEqual({ method: 'respond_to_approval', params: { sessionId: base.id, expectedStreamId: 'stream-fixture', expectedCwdFence: 'cwd-fixture', approvalId: 'ap1', decision: 'allow_once' } });
    await act(async () => { button('停止')!.click(); });
    await flush();
    expect(fake.ops('cancel')[0].request).toMatchObject({ method: 'cancel', params: { runId: 'run-1' } });
  });

  it('manages approved folders only through fenced local requests', async () => {
    const fake = new FakeTransport();
    let update: RequestResult = { status: 'rejected', data: { error: { code: 'PROJECT_ROOT_CHANGED', message: '' } } };
    fake.handler = request => {
      if (request.method === 'get_project_root') return applied({ kind: 'project_root', data: { projectId: 'project-fixture', cwdFence: 'cwd-fixture', root: '/home/alex/茶馆' } });
      if (request.method === 'add_project') return { status: 'rejected', data: { error: { code: 'PROJECT_ROOT_UNSUPPORTED', message: '' } } };
      if (request.method === 'update_project') return update;
      if (request.method === 'remove_project') return applied({ kind: 'empty' });
      return undefined;
    };
    await render(fake, [], '#/settings');
    await flush();
    expect(document.querySelector('.project-row .choice-path')?.textContent).toBe('/home/alex/茶馆');
    const input = document.querySelector('#project-path') as HTMLInputElement;
    await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, '  /mnt/nas  '); input.dispatchEvent(new Event('input', { bubbles: true })); });
    await act(async () => { button('批准文件夹')!.click(); });
    await flush();
    expect(fake.ops('add_project')[0].request).toEqual({ method: 'add_project', params: { root: '/mnt/nas' } });
    expect(document.body.textContent).toContain('不能是网络盘');
    expect(input.value).toBe('  /mnt/nas  ');

    await act(async () => { button('改名')!.click(); });
    const name = document.querySelector('#name-project-fixture') as HTMLInputElement;
    await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(name, '新名字'); name.dispatchEvent(new Event('input', { bubbles: true })); });
    await act(async () => { button('保存')!.click(); });
    await flush();
    expect(fake.ops('update_project')[0].request).toEqual({ method: 'update_project', params: { projectId: 'project-fixture', expectedCwdFence: 'cwd-fixture', name: '新名字', root: null } });
    update = applied({ kind: 'project', data: { id: 'project-fixture', name: '茶馆项目', cwdFence: 'cwd-next' } });
    await act(async () => { button('重新批准')!.click(); });
    await flush();
    expect(fake.ops('update_project')[1].request).toEqual({ method: 'update_project', params: { projectId: 'project-fixture', expectedCwdFence: 'cwd-fixture', name: null, root: '/home/alex/茶馆' } });

    await act(async () => { button('移除')!.click(); });
    expect(fake.ops('remove_project')).toHaveLength(0);
    await act(async () => { button('移除项目')!.click(); });
    await flush();
    expect(fake.ops('remove_project')[0].request).toEqual({ method: 'remove_project', params: { projectId: 'project-fixture', expectedCwdFence: 'cwd-fixture' } });
    expect(fake.ops('get_project_root')).toHaveLength(1);
  });

  it('creates a session with one stable ID until its outcome is known', async () => {
    const fake = new FakeTransport();
    let createResult: RequestResult = { status: 'unknown', data: { error: { code: 'TIMEOUT', message: '' } } };
    fake.handler = request => {
      if (request.method === 'list_runtimes') return applied({ kind: 'runtimes', data: { items: [{ id: 'pi', name: 'Pi', version: '1.5.0', available: true }, { id: 'codex', name: 'Codex', available: false }] } });
      if (request.method === 'get_project_root') return applied({ kind: 'project_root', data: { projectId: 'project-fixture', cwdFence: 'cwd-fixture', root: '/home/alex/茶馆' } });
      if (request.method === 'create_session') return createResult;
      if (request.method === 'get_session') return applied({ kind: 'session', data: { session: baseView, eventStreamId: 'boot-1', sequence: 0 } });
      return undefined;
    };
    await render(fake, []);
    await act(async () => { button('新建会话')!.click(); });
    await flush();
    expect(document.querySelector('dialog')?.open).toBe(true);
    expect(document.querySelector('.choice-path')?.textContent).toBe('/home/alex/茶馆');
    const codex = document.querySelector('input[name="runtime"][value="codex"]') as HTMLInputElement;
    expect(codex.disabled).toBe(true);
    await act(async () => { (document.querySelector('input[name="project"]') as HTMLInputElement).click(); (document.querySelector('input[name="runtime"][value="pi"]') as HTMLInputElement).click(); });
    await act(async () => { button('开始')!.click(); });
    await flush();
    expect(document.body.textContent).toContain('未知 — 请核对');
    const first = fake.ops('create_session')[0].request;
    createResult = applied({ kind: 'session', data: { session: baseView, eventStreamId: 'boot-1', sequence: 0 } });
    await act(async () => { button('再次确认')!.click(); });
    await flush();
    const second = fake.ops('create_session')[1].request;
    expect(second).toEqual(first);
    expect(first).toMatchObject({ method: 'create_session', params: { projectId: 'project-fixture', runtimeId: 'pi', expectedCwdFence: 'cwd-fixture', start: { mode: 'new' } } });
    expect(window.location.hash).toBe(`#/sessions/${(first as Extract<Request, { method: 'create_session' }>).params.sessionId}`);
  });
});
