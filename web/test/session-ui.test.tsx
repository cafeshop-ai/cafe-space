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
  reply(requestId: string, result: RequestResult) { this.onMessage({ type: 'response', protocolVersion: 4, requestId, result }); }
  event(sequence: number, event: Extract<ServerMessage, { type: 'event' }>['event']) { this.onMessage({ type: 'event', protocolVersion: 4, eventStreamId: 'boot-1', sequence, event }); }
  ops(method?: Request['method']) { return this.sent.filter(item => item.requestId.includes('-op-') && (!method || item.request.method === method)); }
}

function handshake(fake: FakeTransport, sessions: SessionSnapshot[], projects = [{ id: 'project-fixture', name: '茶馆项目', cwdFence: 'cwd-fixture' }], runtimes = [{ id: 'pi', name: 'Pi', version: '1.5.0', available: true, toolApproval: 'never_asks' as const }]) {
  fake.onMessage({ type: 'welcome', protocolVersion: 4, daemonVersion: '0.1.0' });
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
    // A turn that settles closes its open dialog: shown as expired, never dropped or approved.
    const open = applySessionEvent(current, 'boot-1', 7, { kind: 'approval_required', data: { sessionId: base.id, approval: { id: 'a2', summary: 'git push' } } });
    if (open === 'stale' || open === 'removed') throw new Error('unexpected');
    const settled = applySessionEvent(open, 'boot-1', 8, { kind: 'session_changed', data: { session: { ...base, state: { state: 'ready' } } } });
    if (settled === 'stale' || settled === 'removed') throw new Error('unexpected');
    expect(settled.view.approvals).toEqual([]);
    expect(settled.expired?.map(item => item.id)).toEqual(['a2']);
    const resolvedLate = applySessionEvent(settled, 'boot-1', 9, { kind: 'approval_resolved', data: { sessionId: base.id, approvalId: 'a2', decision: 'deny' } });
    expect(resolvedLate !== 'stale' && resolvedLate !== 'removed' && resolvedLate.expired).toEqual([]);
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
    // The fixture session does not say whether it asks; that is never shown as safe.
    expect(document.querySelector('.session-chips .tool-approval')?.textContent).toBe('未说明是否会询问');
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

  it('shows a prompt only as a local bubble bound to its outcome until the transcript has it, and never invents an assistant message', async () => {
    const fake = new FakeTransport();
    const running: SessionSnapshot = { ...base, state: { state: 'running' }, activeRunId: 'run-1', capabilities: { ...base.capabilities, deliveryControl: true } };
    let promptResult: RequestResult | undefined;
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: { ...baseView, session: running, messages: [text('u1', '先看看', 'user')] }, eventStreamId: 'boot-1', sequence: 0 } }) :
      request.method === 'prompt' ? promptResult : undefined;
    await render(fake, [running], '#/sessions/session-fixture');
    await flush();
    const input = document.querySelector('textarea')!;
    const type = (value: string) => act(async () => { Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(input, value); input.dispatchEvent(new Event('input', { bubbles: true })); });
    const bubbles = () => [...document.querySelectorAll('.message[data-pending]')].map(item => item.textContent);
    const roles = () => [...document.querySelectorAll('.transcript .message')].map(item => item.getAttribute('data-role'));

    await type('换个方向');
    await act(async () => { button('引导')!.click(); });
    await flush();
    expect(fake.ops('prompt')[0].request).toMatchObject({ params: { content: '换个方向', delivery: 'steer' } });
    // Waiting for the outcome: the draft stays, the bubble only says it is being sent.
    expect(input.value).toBe('换个方向');
    expect(bubbles()).toEqual(['你 · 正在发送…换个方向']);
    await act(async () => fake.reply(fake.ops('prompt')[0].requestId, { status: 'dispatched', data: { operationId: 'op-1' } }));
    expect(input.value).toBe('');
    expect(bubbles()[0]).toContain('已发送');
    // A running session with a user message last still gets no placeholder reply.
    expect(roles()).toEqual(['user', 'user']);
    // Escape in the composer never stops the turn.
    await act(async () => { input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); });
    expect(fake.ops('cancel')).toHaveLength(0);
    await act(async () => fake.event(1, { kind: 'message_changed', data: { sessionId: base.id, message: text('u2', '换个方向', 'user') } }));
    expect(bubbles()).toEqual([]);
    expect(roles()).toEqual(['user', 'user']);

    promptResult = { status: 'rejected', data: { error: { code: 'STALE_STREAM', message: '' } } };
    await type('第三条');
    await act(async () => { button('排队')!.click(); });
    await flush();
    expect(fake.ops('prompt')[1].request).toMatchObject({ params: { content: '第三条', delivery: 'follow_up' } });
    expect(input.value).toBe('第三条');
    expect(bubbles()).toHaveLength(1);
    expect(bubbles()[0]).toContain('未完成');
    promptResult = { status: 'unknown', data: { error: { code: 'TIMEOUT', message: '' } } };
    await act(async () => { button('排队')!.click(); });
    await flush();
    // The retry replaces the rejected bubble; an unknown outcome keeps the draft as well.
    expect(bubbles()).toHaveLength(1);
    expect(bubbles()[0]).toContain('未知 — 请核对');
    expect(input.value).toBe('第三条');
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
    // The turn then settles: the answered dialog is shown as answered, not as unapproved.
    await act(async () => fake.event(1, { kind: 'session_changed', data: { session: { ...running, state: { state: 'ready' }, activeRunId: null } } }));
    expect(document.querySelector('.approval-card[data-expired]')?.textContent).toContain('已发送回复');
    expect(button('允许一次')).toBeUndefined();
    await act(async () => fake.event(2, { kind: 'session_changed', data: { session: running } }));
    await act(async () => { button('停止')!.click(); });
    await flush();
    expect(fake.ops('cancel')[0].request).toMatchObject({ method: 'cancel', params: { runId: 'run-1' } });
    // Dispatched means the abort was sent, not that the turn has ended.
    expect(document.body.textContent).toContain('已发送停止，等待这一轮结束');
    await act(async () => { window.dispatchEvent(new KeyboardEvent('keydown', { key: '.', ctrlKey: true })); });
    await flush();
    expect(fake.ops('cancel')).toHaveLength(2);
    expect(fake.ops('cancel')[1].request).toEqual(fake.ops('cancel')[0].request);
  });

  it('guides a first run through agent detection and approving a folder', async () => {
    const fake = new FakeTransport();
    fake.handler = request => request.method === 'list_runtimes' ? applied({ kind: 'runtimes', data: { items: [{ id: 'pi', name: 'Pi', version: '1.5.0', available: true, toolApproval: 'never_asks' as const }, { id: 'codex', name: 'Codex', available: false, toolApproval: 'unknown' as const }] } }) :
      request.method === 'add_project' ? applied({ kind: 'project', data: { id: 'p-new', name: 'cafe', cwdFence: 'f-new' } }) : undefined;
    window.location.hash = '';
    client = new DaemonClient(() => fake);
    const element = document.createElement('div'); document.body.append(element);
    root = createRoot(element);
    await act(async () => root!.render(<App client={client} nativeAvailable />));
    await act(async () => handshake(fake, [], []));
    await flush();
    const rows = [...document.querySelectorAll('.runtime-list li')].map(item => item.textContent);
    expect(rows).toEqual(['Pi 1.5.0执行工具前不会询问就绪', 'Codex不可用']);
    const input = document.querySelector('#project-path') as HTMLInputElement;
    await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, '/home/alex/cafe'); input.dispatchEvent(new Event('input', { bubbles: true })); });
    await act(async () => { button('批准文件夹')!.click(); });
    await flush();
    expect(fake.ops('add_project')[0].request).toEqual({ method: 'add_project', params: { root: '/home/alex/cafe' } });
    // The approved project arrives as an event; the page then offers New session.
    await act(async () => fake.event(1, { kind: 'project_changed', data: { project: { id: 'p-new', name: 'cafe', cwdFence: 'f-new' } } }));
    expect(document.querySelector('.first-run')).toBeNull();
    expect(button('新建会话')?.disabled).toBe(false);
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
      if (request.method === 'list_runtimes') return applied({ kind: 'runtimes', data: { items: [{ id: 'pi', name: 'Pi', version: '1.5.0', available: true, toolApproval: 'never_asks' as const }, { id: 'codex', name: 'Codex', available: false, toolApproval: 'unknown' as const }] } });
      if (request.method === 'get_project_root') return applied({ kind: 'project_root', data: { projectId: 'project-fixture', cwdFence: 'cwd-fixture', root: '/home/alex/茶馆' } });
      if (request.method === 'create_session') return createResult;
      if (request.method === 'get_session') return applied({ kind: 'session', data: { session: baseView, eventStreamId: 'boot-1', sequence: 0 } });
      return undefined;
    };
    await render(fake, []);
    await act(async () => { button('新建会话')!.click(); });
    await flush();
    expect(document.querySelector<HTMLDialogElement>('dialog.sheet')?.open).toBe(true);
    expect(document.querySelector('.choice-path')?.textContent).toBe('/home/alex/茶馆');
    const codex = document.querySelector('input[name="runtime"][value="codex"]') as HTMLInputElement;
    expect(codex.disabled).toBe(true);
    expect(document.querySelector('.choice .tool-approval')?.textContent).toBe('执行工具前不会询问');
    await act(async () => { (document.querySelector('input[name="project"]') as HTMLInputElement).click(); (document.querySelector('input[name="runtime"][value="pi"]') as HTMLInputElement).click(); });
    expect(document.querySelector('.sheet')?.textContent).toContain('不先确认');
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

describe('thinking control', () => {
  /** What the daemon would answer to a fresh get_session. */
  let server: SessionSnapshot;
  const open = async (session: SessionSnapshot, handler: FakeTransport['handler'] = () => undefined) => {
    const fake = new FakeTransport();
    server = session;
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: { ...baseView, session: server }, eventStreamId: 'boot-1', sequence: 0 } }) : handler(request);
    await render(fake, [session], '#/sessions/session-fixture');
    await flush();
    return fake;
  };
  /** The agent applies `level` but reports `actual`; `eventFirst` picks which reaches the client first. */
  const agent = (actual: SessionSnapshot['thinkingLevel'], eventFirst: boolean, fake: () => FakeTransport): FakeTransport['handler'] => request => {
    if (request.method !== 'set_thinking') return undefined;
    server = { ...server, thinkingLevel: actual };
    if (eventFirst) fake().event(1, { kind: 'session_changed', data: { session: server } });
    return applied({ kind: 'empty' });
  };
  const trigger = () => document.querySelector<HTMLButtonElement>('.thinking-control [aria-haspopup="menu"]');
  const item = (label: string) => [...document.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')].find(each => each.textContent === label)!;
  const key = (target: Element, value: string) => act(async () => { target.dispatchEvent(new KeyboardEvent('keydown', { key: value, bubbles: true })); });
  const note = () => document.querySelector('.thinking-control .field-note')?.textContent ?? null;

  it('is a read-only chip when the agent cannot change it, and locked while the agent is busy', async () => {
    await open({ ...base, capabilities: { ...base.capabilities, thinkingControl: false } });
    expect(trigger()).toBeNull();
    const chip = [...document.querySelectorAll('.session-chips .chip')].find(each => each.textContent === '思考强度：中');
    expect(chip?.getAttribute('title')).toBe('这个 Agent 不支持调整思考强度');
    await act(async () => { root?.unmount(); client?.disconnect(); });
    await open({ ...base, state: { state: 'running' }, activeRunId: 'run-1' });
    expect(trigger()?.disabled).toBe(true);
  });

  it('sends a fenced level from the keyboard and explains a clamp when the event comes first', async () => {
    const fake: FakeTransport = await open(base, agent('high', true, () => fake));
    await act(async () => { trigger()!.click(); });
    expect(document.querySelectorAll('[role="menuitemradio"]')).toHaveLength(7);
    expect(document.activeElement).toBe(item('中'));
    await key(document.activeElement!, 'ArrowDown');
    expect(document.activeElement).toBe(item('高'));
    await key(document.activeElement!, 'Escape');
    expect(document.querySelector('[role="menu"]')).toBeNull();
    expect(document.activeElement).toBe(trigger());
    await key(trigger()!, 'ArrowDown');
    await act(async () => { item('最大').click(); });
    await flush(); await flush();
    expect(fake.ops('set_thinking')[0].request).toEqual({ method: 'set_thinking', params: { sessionId: base.id, expectedStreamId: 'stream-fixture', expectedCwdFence: 'cwd-fixture', level: 'max' } });
    expect(fake.ops('get_session')).toHaveLength(2);
    expect(trigger()?.textContent).toContain('思考强度：高');
    expect(note()).toBe('Agent 把思考强度调整为“高”');
  });

  it('judges a clamp from the authoritative read, not a cache the reply overtook', async () => {
    let fake: FakeTransport = await open(base, agent('high', false, () => fake));
    await act(async () => { trigger()!.click(); });
    await act(async () => { item('高').click(); });
    await flush(); await flush();
    // Applied as asked: the stale header value must not read as an adjustment.
    expect(trigger()?.textContent).toContain('思考强度：中');
    expect(note()).toBeNull();
    await act(async () => fake.event(1, { kind: 'session_changed', data: { session: server } }));
    expect(trigger()?.textContent).toContain('思考强度：高');
    expect(note()).toBeNull();

    await act(async () => { root?.unmount(); client?.disconnect(); });
    fake = await open(base, agent('low', false, () => fake));
    await act(async () => { trigger()!.click(); });
    await act(async () => { item('最大').click(); });
    await flush(); await flush();
    expect(note()).toBe('Agent 把思考强度调整为“低”');
  });

  it('shows an unknown outcome without updating the level, reading back or retrying', async () => {
    const fake = await open(base, request => request.method === 'set_thinking' ? { status: 'unknown', data: { error: { code: 'PI_CONFIGURATION_OUTCOME_UNKNOWN', message: '' } } } : undefined);
    await act(async () => { trigger()!.click(); });
    await act(async () => { item('低').click(); });
    await flush();
    expect(trigger()?.textContent).toContain('思考强度：中');
    expect(document.querySelector('.thinking-control .outcome-mark')?.textContent).toContain('未知 — 请核对');
    await flush();
    expect(fake.ops('set_thinking')).toHaveLength(1);
    expect(fake.ops('get_session')).toHaveLength(1);
    expect(note()).toBeNull();
    expect(trigger()?.disabled).toBe(false);
  });
});

describe('command palette', () => {
  const ctrl = (key: string) => act(async () => { window.dispatchEvent(new KeyboardEvent('keydown', { key, ctrlKey: true, bubbles: true })); });
  const input = () => document.querySelector<HTMLInputElement>('.palette-input')!;
  const open = () => document.querySelector('.palette')?.hasAttribute('open');
  const options = () => [...document.querySelectorAll('.palette-list [role="option"]')].map(item => item.querySelector('.palette-label')!.textContent);
  const type = (value: string) => act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input(), value); input().dispatchEvent(new Event('input', { bubbles: true })); });
  const key = (value: string, init: KeyboardEventInit = {}) => act(async () => { input().dispatchEvent(new KeyboardEvent('keydown', { key: value, bubbles: true, ...init })); });

  it('opens from Ctrl+K or the toolbar, filters, and jumps to a session from the keyboard', async () => {
    await render(new FakeTransport(), [{ ...base, id: 's1', title: '整理菜单' }, { ...base, id: 's2', title: '发布说明' }]);
    await ctrl('k');
    expect(open()).toBe(true);
    expect(options()).toEqual(['新建会话', '会话', '房间', '远程访问', '设置', '整理菜单', '发布说明']);
    expect(document.querySelector('.palette-list [role="option"] .palette-hint')?.textContent).toBe('Ctrl+N');
    await type('发布');
    expect(options()).toEqual(['发布说明']);
    // Enter during IME composition selects a candidate and must not run anything.
    await key('Enter', { isComposing: true });
    expect(window.location.hash).toBe('');
    await key('Enter');
    expect(window.location.hash).toBe('#/sessions/s2');
    expect(open()).toBe(false);
    await act(async () => { document.querySelector<HTMLButtonElement>('.palette-button')!.click(); });
    expect(open()).toBe(true);
    expect(input().value).toBe('');
    await ctrl('k');
    expect(open()).toBe(false);
  });

  it('moves with the arrows, runs actions, and switches pages with Ctrl+1–3 and Ctrl+,', async () => {
    await render(new FakeTransport(), [base]);
    await ctrl('k');
    await key('ArrowDown'); await key('ArrowDown'); await key('ArrowUp'); await key('ArrowUp'); await key('ArrowUp');
    const selected = document.querySelector('[role="option"][aria-selected="true"]')!;
    expect(selected.textContent).toContain('Fixture session');
    expect(input().getAttribute('aria-activedescendant')).toBe(selected.id);
    await type('新建');
    await key('Enter');
    expect(open()).toBe(false);
    expect(document.querySelector<HTMLDialogElement>('dialog.sheet')?.open).toBe(true);
    for (const [shortcut, hash] of [[',', '#/settings'], ['2', '#/rooms'], ['3', '#/remote'], ['1', '#/sessions']]) {
      await ctrl(shortcut);
      expect(window.location.hash).toBe(hash);
    }
  });

  it('offers stop only for the open session’s running turn and sends the same fenced cancel as Ctrl+.', async () => {
    const fake = new FakeTransport();
    const running: SessionSnapshot = { ...base, state: { state: 'running' }, activeRunId: 'run-1' };
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: { ...baseView, session: running }, eventStreamId: 'boot-1', sequence: 0 } }) :
      request.method === 'cancel' ? { status: 'dispatched', data: { operationId: 'x' } } : undefined;
    await render(fake, [running], '#/sessions/session-fixture');
    await flush();
    await ctrl('k');
    await type('停止');
    expect(options()).toEqual(['停止当前轮']);
    await key('Enter');
    await flush();
    expect(fake.ops('cancel')[0].request).toEqual({ method: 'cancel', params: { sessionId: base.id, expectedStreamId: 'stream-fixture', expectedCwdFence: 'cwd-fixture', runId: 'run-1' } });
    expect(document.body.textContent).toContain('已发送停止，等待这一轮结束');
    await act(async () => fake.event(1, { kind: 'session_changed', data: { session: { ...running, state: { state: 'ready' }, activeRunId: null } } }));
    await ctrl('k');
    await type('停止');
    expect(options()).toEqual([]);
    expect(document.querySelector('.palette-empty')?.textContent).toBe('没有匹配的结果');
  });

  it('keeps the chosen command when the list changes under it, and never retargets Enter', async () => {
    const fake = new FakeTransport();
    const running: SessionSnapshot = { ...base, state: { state: 'running' }, activeRunId: 'run-1' };
    let current = base, sequence = 0;
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: { ...baseView, session: current }, eventStreamId: 'boot-1', sequence } }) : undefined;
    await render(fake, [base], '#/sessions/session-fixture');
    await flush();
    const selected = () => document.querySelector('[role="option"][aria-selected="true"] .palette-label')?.textContent;
    await ctrl('k');
    await key('ArrowDown');
    expect(selected()).toBe('会话');
    await act(async () => fake.event(1, { kind: 'session_changed', data: { session: running } }));
    expect(options().slice(0, 3)).toEqual(['新建会话', '停止当前轮', '会话']);
    expect(selected()).toBe('会话');
    await key('Enter');
    expect(window.location.hash).toBe('#/sessions');
    expect(fake.ops('cancel')).toHaveLength(0);

    current = running; sequence = 1;
    await act(async () => { window.location.hash = '#/sessions/session-fixture'; await new Promise(resolve => setTimeout(resolve)); });
    await flush();
    await ctrl('k');
    await key('ArrowDown');
    expect(selected()).toBe('停止当前轮');
    await act(async () => fake.event(2, { kind: 'session_changed', data: { session: { ...running, state: { state: 'ready' }, activeRunId: null } } }));
    expect(selected()).toBeUndefined();
    expect(input().hasAttribute('aria-activedescendant')).toBe(false);
    await key('Enter');
    expect(open()).toBe(true);
    expect(window.location.hash).toBe('#/sessions/session-fixture');
    // A later run brings back a stop command, but the cleared choice must not land on it.
    await act(async () => fake.event(3, { kind: 'session_changed', data: { session: { ...running, activeRunId: 'run-2' } } }));
    expect(options()[1]).toBe('停止当前轮');
    expect(selected()).toBeUndefined();
    await key('Enter');
    await key('ArrowDown'); await key('ArrowDown');
    expect(selected()).toBe('停止当前轮');
    // The run is replaced without a settled state in between.
    await act(async () => fake.event(4, { kind: 'session_changed', data: { session: { ...running, activeRunId: 'run-3' } } }));
    expect(selected()).toBeUndefined();
    await key('Enter');
    await flush();
    expect(fake.ops('cancel')).toHaveLength(0);
  });

  it('leaves the arrows and shortcuts to the IME while it composes', async () => {
    await render(new FakeTransport(), [base]);
    const composing = (target: EventTarget, init: KeyboardEventInit) => {
      const event = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, isComposing: true, ...init });
      act(() => { target.dispatchEvent(event); });
      return event.defaultPrevented;
    };
    expect(composing(window, { key: 'k', ctrlKey: true })).toBe(false);
    expect(open()).toBe(false);
    await ctrl('k');
    const before = input().getAttribute('aria-activedescendant');
    expect(composing(input(), { key: 'ArrowDown' })).toBe(false);
    expect(composing(input(), { key: 'ArrowUp' })).toBe(false);
    expect(input().getAttribute('aria-activedescendant')).toBe(before);
    expect(open()).toBe(true);
  });
});

describe('error wording', () => {
  it('gives every daemon and adapter code a sentence without hiding the code', async () => {
    const { errorMessage } = await import('../src/app/copy');
    expect(errorMessage('PI_APPROVAL_SCOPE_UNSUPPORTED')).toContain('本会话内允许');
    expect(errorMessage('PI_COMMAND_OUTCOME_UNKNOWN')).toBe('无法确认这个操作是否生效。');
    expect(errorMessage('PI_NODE_UNAVAILABLE')).toBe('没有找到可用的 Pi 安装。');
    expect(errorMessage('PI_RECORD_INVALID')).toBe('Pi 返回了无法识别的数据。');
    expect(errorMessage('PI_SOMETHING_NEW')).toBe('Pi 出现了问题。');
    expect(errorMessage('UNKNOWN_CODE')).toBe('操作未完成。');
  });
});
