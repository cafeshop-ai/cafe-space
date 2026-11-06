import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { App } from '../src/app/App';
import { loadSessionPage } from '../src/app/SessionPage';
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
import type { OwnerEnvironment } from '../src/protocol/generated/OwnerEnvironment';
import type { OwnerEnvironmentEntry } from '../src/protocol/generated/OwnerEnvironmentEntry';
import { ownerEnvironmentResponses, sessionResponse } from '../src/protocol-fixtures.generated';

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
// The session page is its own chunk; tests load it up front, as the app does while idle.
beforeAll(() => loadSessionPage(), 60000);
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
  reply(requestId: string, result: RequestResult) { this.onMessage({ type: 'response', protocolVersion: 5, requestId, result }); }
  event(sequence: number, event: Extract<ServerMessage, { type: 'event' }>['event']) { this.onMessage({ type: 'event', protocolVersion: 5, eventStreamId: 'boot-1', sequence, event }); }
  ops(method?: Request['method']) { return this.sent.filter(item => item.requestId.includes('-op-') && (!method || item.request.method === method)); }
}

function handshake(fake: FakeTransport, sessions: SessionSnapshot[], projects = [{ id: 'project-fixture', name: '茶馆项目', cwdFence: 'cwd-fixture' }], runtimes = [{ id: 'pi', name: 'Pi', version: '1.5.0', available: true, toolApproval: 'never_asks' as const }]) {
  fake.onMessage({ type: 'welcome', protocolVersion: 5, daemonVersion: '0.1.0' });
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
async function render(fake: FakeTransport, sessions: SessionSnapshot[], hash = '', native = true) {
  window.location.hash = hash;
  client = new DaemonClient(() => fake);
  const element = document.createElement('div'); document.body.append(element);
  root = createRoot(element);
  await act(async () => root!.render(<App client={client} nativeAvailable={native} />));
  // A client away from this computer connects on its own; here the test opens it.
  if (!native) await act(async () => { void client!.connect(); });
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

  it('keeps the draft while the transcript is briefly unavailable, without sending it again', async () => {
    const fake = new FakeTransport();
    let read: RequestResult = applied({ kind: 'session', data: { session: baseView, eventStreamId: 'boot-1', sequence: 0 } });
    fake.handler = request => request.method === 'get_session' ? read : request.method === 'prompt' ? { status: 'unknown', data: { error: { code: 'TIMEOUT', message: '' } } } : undefined;
    await render(fake, [base], '#/sessions/session-fixture');
    await flush();
    const input = () => document.querySelector('textarea')!;
    await act(async () => { Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(input(), '还没发出去的话'); input().dispatchEvent(new Event('input', { bubbles: true })); });
    await act(async () => { button('发送')!.click(); });
    await flush();
    expect(fake.ops('prompt')).toHaveLength(1);
    read = { status: 'rejected', data: { error: { code: 'RUNTIME_UNAVAILABLE', message: '' } } };
    await act(async () => { await client!.openSession('session-fixture'); });
    expect(document.querySelector('textarea')).toBeNull();
    expect(document.body.textContent).toContain('会话记录没有读取成功');
    read = applied({ kind: 'session', data: { session: baseView, eventStreamId: 'boot-1', sequence: 0 } });
    await act(async () => { button('重新读取')!.click(); });
    await flush();
    expect(input().value).toBe('还没发出去的话');
    expect(document.querySelector('.message[data-pending]')?.textContent).toContain('未知 — 请核对');
    expect(fake.ops('prompt')).toHaveLength(1);
  });

  it('lets each transcript echo confirm one bubble, and never brings a confirmed bubble back', async () => {
    const fake = new FakeTransport();
    let view: SessionView = { ...baseView, messages: [] };
    let sequence = 0;
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: view, eventStreamId: 'boot-1', sequence } }) :
      request.method === 'prompt' ? { status: 'dispatched', data: { operationId: 'op' } } : undefined;
    await render(fake, [base], '#/sessions/session-fixture');
    await flush();
    const input = document.querySelector('textarea')!;
    const bubbles = () => document.querySelectorAll('.message[data-pending]').length;
    for (let i = 0; i < 2; i++) {
      await act(async () => { Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!.call(input, '同一句'); input.dispatchEvent(new Event('input', { bubbles: true })); });
      await act(async () => { button('发送')!.click(); });
      await flush();
    }
    expect(bubbles()).toBe(2);
    await act(async () => fake.event(++sequence, { kind: 'message_changed', data: { sessionId: base.id, message: text('e1', '同一句', 'user') } }));
    expect(bubbles()).toBe(1);
    await act(async () => fake.event(++sequence, { kind: 'message_changed', data: { sessionId: base.id, message: text('e2', '同一句', 'user') } }));
    expect(bubbles()).toBe(0);
    // A fresh snapshot no longer holds the echoes (bounded history): the bubbles stay confirmed.
    view = { ...baseView, messages: [text('later', '之后的回复')] };
    await act(async () => { await client!.openSession('session-fixture'); });
    expect(document.body.textContent).toContain('之后的回复');
    expect(bubbles()).toBe(0);
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

describe('session inspector', () => {
  const details = () => document.querySelector<HTMLDetailsElement>('.inspector')!;
  const row = (label: string) => [...details().querySelectorAll('dl > div')].find(item => item.querySelector('dt')?.textContent === label)?.querySelector('dd')?.textContent;
  const open = (session: SessionSnapshot, roots = true) => {
    const fake = new FakeTransport();
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: { ...baseView, session }, eventStreamId: 'boot-1', sequence: 0 } }) :
      request.method === 'get_project_root' && roots ? applied({ kind: 'project_root', data: { projectId: 'project-fixture', cwdFence: 'cwd-fixture', root: '/home/alex/茶馆' } }) : undefined;
    return fake;
  };
  afterEach(() => localStorage.clear());

  it('shows the session from protocol fields, and the project path only at this computer', async () => {
    const session: SessionSnapshot = { ...base, mode: 'resume', model: { provider: 'anthropic', id: 'claude' }, capabilities: { ...base.capabilities, toolApproval: 'may_ask' } };
    const fake = open(session);
    await render(fake, [session], '#/sessions/session-fixture');
    await flush();
    expect(details().open).toBe(true);
    expect([row('Agent'), row('已装版本'), row('项目'), row('路径'), row('开始方式'), row('工具审批'), row('模型')])
      .toEqual(['Pi', '1.5.0', '茶馆项目', '/home/alex/茶馆', '继续自之前的会话', '某些工具前可能会询问', 'anthropic / claude']);
    expect(fake.ops('get_project_root').map(item => item.request)).toEqual([{ method: 'get_project_root', params: { projectId: 'project-fixture' } }]);
    await act(async () => { root?.unmount(); client?.disconnect(); });

    const remote = open(session);
    await render(remote, [session], '#/sessions/session-fixture', false);
    await flush();
    expect(row('项目')).toBe('茶馆项目');
    expect(row('路径')).toBeUndefined();
    expect(remote.ops('get_project_root')).toHaveLength(0);
  });

  it('says why a session stopped or failed, keeping the raw code, and reads a failing path once', async () => {
    const stopped: SessionSnapshot = { ...base, state: { state: 'stopped' }, stopReason: 'AGENT_EXITED' };
    let fake = open(stopped, false);
    await render(fake, [stopped], '#/sessions/session-fixture');
    await flush();
    expect(row('状态')).toBe('已停止 · Agent 已退出');
    expect(row('路径')).toBe('未读取');
    expect(fake.ops('get_project_root')).toHaveLength(1);
    await act(async () => { root?.unmount(); client?.disconnect(); });

    const failed: SessionSnapshot = { ...base, state: { state: 'failed', data: { code: 'PI_STARTUP_TIMEOUT' } } };
    fake = open(failed);
    await render(fake, [failed], '#/sessions/session-fixture');
    await flush();
    expect(row('状态')).toMatch(/^失败 · ./);
    expect(row('错误代码')).toBe('PI_STARTUP_TIMEOUT');
  });

  it('reads the path afresh on every connection and never shows one from an earlier connection', async () => {
    let path: string | null = '/home/alex/旧';
    const fake = new FakeTransport();
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: baseView, eventStreamId: 'boot-1', sequence: 0 } }) :
      request.method !== 'get_project_root' ? undefined : path ? applied({ kind: 'project_root', data: { projectId: 'project-fixture', cwdFence: 'cwd-fixture', root: path } }) :
      { status: 'rejected', data: { error: { code: 'PROJECT_ROOT_UNAVAILABLE', message: '' } } };
    const reconnect = async (hash: string) => {
      fake.sent.length = 0;
      window.location.hash = hash;
      await act(async () => { client!.disconnect(); await client!.connect(); });
      await act(async () => handshake(fake, [base]));
      await flush();
    };
    await render(fake, [base], '#/sessions/session-fixture');
    await flush();
    expect(row('路径')).toBe('/home/alex/旧');
    // Same project and fence, but the folder moved while the app was away.
    path = '/home/alex/新';
    await reconnect('#/sessions/session-fixture');
    expect(row('路径')).toBe('/home/alex/新');
    expect(fake.ops('get_project_root')).toHaveLength(1);
    path = null;
    await reconnect('#/sessions/session-fixture');
    expect(row('路径')).toBe('未读取');
    await flush();
    expect(fake.ops('get_project_root')).toHaveLength(1);
    // Approved folders in Settings are read afresh too, while the page stays open across the reconnect.
    await act(async () => { window.location.hash = '#/settings'; await new Promise(resolve => setTimeout(resolve, 0)); });
    await flush();
    path = '/home/alex/再新';
    await reconnect('#/settings');
    expect(document.querySelector('.project-row .choice-path')?.textContent).toBe('/home/alex/再新');
  });

  it('remembers whether the details are open', async () => {
    const task = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
    await render(open(base), [base], '#/sessions/session-fixture');
    await flush();
    await task();
    expect(localStorage.getItem('cafe-space.inspector.session')).toBeNull();
    await act(async () => { details().querySelector('summary')!.click(); });
    // The toggle event arrives as its own task.
    await task();
    expect(details().open).toBe(false);
    expect(localStorage.getItem('cafe-space.inspector.session')).toBe('closed');
    await act(async () => { root?.unmount(); client?.disconnect(); });

    await render(open(base), [base], '#/sessions/session-fixture');
    await flush();
    expect(details().open).toBe(false);
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

  it('drops an outcome that lands after the session moved to another generation', async () => {
    const fake = await open(base);
    await act(async () => { trigger()!.click(); });
    await act(async () => { item('高').click(); });
    server = { ...base, streamId: 'stream-2' };
    await act(async () => fake.event(1, { kind: 'session_changed', data: { session: server } }));
    await act(async () => fake.reply(fake.ops('set_thinking')[0].requestId, applied({ kind: 'empty' })));
    await flush(); await flush();
    expect(document.querySelector('.thinking-control .outcome-mark')).toBeNull();
    expect(trigger()?.getAttribute('aria-busy')).toBe('false');
  });
});

describe('model control', () => {
  const A = { provider: 'openrouter', id: 'qwen/qwen3-coder' }, B = { provider: 'openrouter', id: 'deepseek/v3' }, C = { provider: 'lab', id: 'tiny' };
  const withModel: SessionSnapshot = { ...base, model: A, capabilities: { ...base.capabilities, modelControl: true } };
  const fence = { sessionId: base.id, expectedStreamId: 'stream-fixture', expectedCwdFence: 'cwd-fixture' };
  let server: SessionSnapshot;
  const open = async (session: SessionSnapshot, handler: FakeTransport['handler'] = () => undefined, native = true) => {
    const fake = new FakeTransport();
    server = session;
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: { ...baseView, session: server }, eventStreamId: 'boot-1', sequence: 0 } }) : handler(request);
    await render(fake, [session], '#/sessions/session-fixture', native);
    await flush();
    return fake;
  };
  const models = (...items: typeof A[]): RequestResult => applied({ kind: 'models', data: { items } });
  const trigger = () => document.querySelector<HTMLButtonElement>('.model-control [aria-haspopup="menu"]');
  const items = () => [...document.querySelectorAll<HTMLButtonElement>('.model-control [role="menuitemradio"]')];
  const item = (label: string) => items().find(each => each.textContent === label)!;
  const note = (selector = '.field-note') => document.querySelector(`.model-control ${selector}`)?.textContent ?? null;
  const toggle = () => act(async () => { trigger()!.click(); });

  it('is a read-only chip when the agent cannot switch, or away from this computer', async () => {
    let fake = await open({ ...withModel, capabilities: { ...base.capabilities, modelControl: false } });
    expect(trigger()).toBeNull();
    const chip = () => [...document.querySelectorAll('.session-chips .chip')].find(each => each.textContent === '模型：openrouter / qwen/qwen3-coder');
    expect(chip()?.getAttribute('title')).toBe('这个 Agent 不支持切换模型');
    await act(async () => { root?.unmount(); client?.disconnect(); });
    fake = await open(withModel, () => models(A), false);
    expect(trigger()).toBeNull();
    expect(chip()?.getAttribute('title')).toBe('只能在这台电脑上切换模型');
    expect(fake.ops('list_models')).toHaveLength(0);
  });

  it('offers only the configured list and judges the switch from the authoritative read', async () => {
    const fake: FakeTransport = await open(withModel, request => {
      if (request.method === 'list_models') return models(A, B, C);
      if (request.method !== 'set_model') return undefined;
      // The agent takes another model than asked, and its event arrives before the reply.
      server = { ...server, model: B };
      fake.event(1, { kind: 'session_changed', data: { session: server } });
      return applied({ kind: 'empty' });
    });
    expect(trigger()?.textContent).toContain('模型：openrouter / qwen/qwen3-coder');
    await toggle(); await flush();
    expect(fake.ops('list_models')[0].request).toEqual({ method: 'list_models', params: fence });
    expect(items().map(each => each.textContent)).toEqual(['openrouter / qwen/qwen3-coder', 'openrouter / deepseek/v3', 'lab / tiny']);
    expect(document.activeElement).toBe(item('openrouter / qwen/qwen3-coder'));
    await act(async () => { document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); });
    expect(items()).toHaveLength(0);
    // The list read once stays for this session generation.
    await toggle(); await flush();
    expect(fake.ops('list_models')).toHaveLength(1);
    await act(async () => { item('lab / tiny').click(); });
    await flush(); await flush();
    expect(fake.ops('set_model')[0].request).toEqual({ method: 'set_model', params: { ...fence, provider: 'lab', modelId: 'tiny' } });
    expect(fake.ops('get_session')).toHaveLength(2);
    expect(trigger()?.textContent).toContain('模型：openrouter / deepseek/v3');
    expect(note()).toBe('Agent 把模型调整为“openrouter / deepseek/v3”');
  });

  it('says so when the list is empty or unread, and reads again only when reopened', async () => {
    let answer: RequestResult = models();
    let fake = await open(withModel, request => request.method === 'list_models' ? answer : undefined);
    await toggle(); await flush();
    expect(items()).toHaveLength(0);
    expect(note('.chip-menu-note')).toBe('Agent 没有报告可选模型。');
    await act(async () => { root?.unmount(); client?.disconnect(); });

    answer = { status: 'rejected', data: { error: { code: 'PI_COMMAND_REJECTED', message: '' } } };
    fake = await open(withModel, request => request.method === 'list_models' ? answer : undefined);
    await toggle(); await flush();
    expect(note('.chip-menu-note')).toBe('没能读取可选模型：Pi 拒绝了这个请求。');
    await flush();
    expect(fake.ops('list_models')).toHaveLength(1);
    answer = models(A, B);
    await toggle(); await toggle(); await flush();
    expect(fake.ops('list_models')).toHaveLength(2);
    expect(items()).toHaveLength(2);
  });

  it('drops a list read for an earlier session generation', async () => {
    const fake = await open(withModel);
    await toggle();
    expect(note('.chip-menu-note')).toBe('正在读取可选模型…');
    const first = fake.ops('list_models')[0];
    server = { ...withModel, streamId: 'stream-2' };
    await act(async () => fake.event(1, { kind: 'session_changed', data: { session: server } }));
    // The next generation starts with the menu closed and nothing read.
    expect(note('.chip-menu-note')).toBeNull();
    await toggle(); await flush();
    const second = fake.ops('list_models')[1];
    expect(second.request).toEqual({ method: 'list_models', params: { ...fence, expectedStreamId: 'stream-2' } });
    // The earlier reply lands while the current read is still out.
    await act(async () => fake.reply(first.requestId, models(A, B, C)));
    await flush();
    expect(items()).toHaveLength(0);
    expect(note('.chip-menu-note')).toBe('正在读取可选模型…');
    await act(async () => fake.reply(second.requestId, models(B)));
    expect(items().map(each => each.textContent)).toEqual(['openrouter / deepseek/v3']);
    expect(fake.ops('list_models')).toHaveLength(2);
  });

  it('shows an unknown switch without reading back or retrying', async () => {
    const fake = await open(withModel, request => request.method === 'list_models' ? models(A, B) :
      request.method === 'set_model' ? { status: 'unknown', data: { error: { code: 'PI_CONFIGURATION_OUTCOME_UNKNOWN', message: '' } } } : undefined);
    await toggle(); await flush();
    await act(async () => { item('openrouter / deepseek/v3').click(); });
    await flush(); await flush();
    expect(trigger()?.textContent).toContain('模型：openrouter / qwen/qwen3-coder');
    expect(document.querySelector('.model-control .outcome-mark')?.textContent).toContain('未知 — 请核对');
    expect(fake.ops('set_model')).toHaveLength(1);
    expect(fake.ops('get_session')).toHaveLength(1);
    expect(note()).toBeNull();
  });

  it('drops a switch outcome that lands after the session moved to another generation', async () => {
    const fake = await open(withModel, request => request.method === 'list_models' ? models(A, B, C) : undefined);
    await toggle(); await flush();
    await act(async () => { item('lab / tiny').click(); });
    expect(trigger()?.getAttribute('aria-busy')).toBe('true');
    server = { ...withModel, streamId: 'stream-2' };
    await act(async () => fake.event(1, { kind: 'session_changed', data: { session: server } }));
    await act(async () => fake.reply(fake.ops('set_model')[0].requestId, applied({ kind: 'empty' })));
    await flush(); await flush();
    expect(document.querySelector('.model-control .outcome-mark')).toBeNull();
    expect(note()).toBeNull();
    expect(trigger()?.getAttribute('aria-busy')).toBe('false');
  });

  it('claims no adjustment from a read of another generation', async () => {
    const fake = await open(withModel, request => {
      if (request.method === 'list_models') return models(A, B, C);
      if (request.method !== 'set_model') return undefined;
      server = { ...server, streamId: 'stream-3', model: B };
      return applied({ kind: 'empty' });
    });
    await toggle(); await flush();
    await act(async () => { item('lab / tiny').click(); });
    await flush(); await flush();
    expect(fake.ops('get_session')).toHaveLength(2);
    expect(document.querySelector('.model-control .outcome-mark')?.textContent).toContain('已完成');
    expect(note()).toBeNull();
  });
});

describe('owner environment', () => {
  const fixture = (JSON.parse(JSON.stringify(ownerEnvironmentResponses[0])) as { result: { data: { result: { data: OwnerEnvironment } } } }).result.data.result.data;
  const status = (patch: Partial<OwnerEnvironment> = {}) => applied({ kind: 'owner_environment', data: { ...fixture, ...patch } });
  const adopted: OwnerEnvironmentEntry = { name: 'HTTPS_PROXY', display: 'http://•••:•••@proxy.invalid:3128', source: 'owner_override', adoptable: false };
  const section = () => document.querySelector<HTMLElement>('section[aria-labelledby="environment-title"]');
  const rows = (name: string) => [...section()!.querySelectorAll('.project-row')].filter(row => row.querySelector('code')?.textContent === name);
  const press = (row: Element, label?: string) => act(async () => { [...row.querySelectorAll('button')].find(item => !label || item.textContent === label)!.click(); });

  it('shows masked status and changes a variable only by name, never with a value', async () => {
    const fake = new FakeTransport();
    fake.handler = request => request.method === 'get_owner_environment' ? status() : request.method !== 'update_owner_environment' ? undefined :
      status({ configured: request.params.change.action === 'adopt_detected' ? [...fixture.configured, adopted] : [] });
    await render(fake, [], '#/settings');
    await flush();
    const text = section()!.textContent!;
    expect(text).toContain('PATH 已就绪');
    expect(text).toContain('npm 没找到');
    expect(rows('SSH_AUTH_SOCK')[0].textContent).toContain('已设置');
    expect(rows('HTTPS_PROXY')[0].textContent).toContain('http://•••:•••@proxy.invalid:3128');
    expect(section()!.querySelector('input, textarea')).toBeNull();
    await press(rows('HTTPS_PROXY')[0]);
    await flush();
    expect(fake.ops('update_owner_environment')[0].request).toEqual({ method: 'update_owner_environment', params: { name: 'HTTPS_PROXY', change: { action: 'adopt_detected' } } });
    expect(rows('HTTPS_PROXY').map(row => row.querySelector('button')?.textContent)).toEqual(['移除', '改用检测到的值']);
    expect(section()!.querySelector('.outcome-mark')?.getAttribute('data-outcome')).toBe('done');
    // Removing asks first, and warns only when the value cannot be adopted again from here.
    await press(rows('HTTPS_PROXY')[0]);
    expect(rows('HTTPS_PROXY')[0].textContent).not.toContain('无法再填回');
    await press(rows('HTTPS_PROXY')[0], '取消');
    await press(rows('NODE_EXTRA_CA_CERTS')[0]);
    expect(rows('NODE_EXTRA_CA_CERTS')[0].textContent).toContain('无法再填回');
    expect(fake.ops('update_owner_environment')).toHaveLength(1);
    await press(rows('NODE_EXTRA_CA_CERTS')[0], '确认移除');
    await flush();
    expect(fake.ops('update_owner_environment')[1].request).toEqual({ method: 'update_owner_environment', params: { name: 'NODE_EXTRA_CA_CERTS', change: { action: 'remove' } } });
    expect(section()!.textContent).toContain('默认都不会交给 Agent');
    expect(fake.ops('get_owner_environment')).toHaveLength(1);
  });

  it('reads the status after an unknown write instead of replaying it', async () => {
    const fake = new FakeTransport();
    fake.handler = request => request.method === 'get_owner_environment' ? status(fake.ops('get_owner_environment').length > 1 ? { configured: [adopted] } : {}) :
      request.method === 'update_owner_environment' ? { status: 'unknown', data: { error: { code: 'OWNER_SETTINGS_WRITE_UNKNOWN', message: '' } } } : undefined;
    await render(fake, [], '#/settings');
    await flush();
    await press(rows('HTTPS_PROXY')[0]);
    await flush(); await flush();
    expect(fake.ops('update_owner_environment')).toHaveLength(1);
    expect(fake.ops('get_owner_environment')).toHaveLength(2);
    expect(section()!.querySelector('.outcome-mark')?.getAttribute('data-outcome')).toBe('unknown');
    expect(section()!.textContent).toContain('无法确认设置是否已保存');
    expect(section()!.textContent).toContain('没有重试');
    expect(rows('HTTPS_PROXY')).toHaveLength(2);
  });

  it('says when the status cannot be read, reads again only when asked, and locks actions while reading', async () => {
    const fake = new FakeTransport();
    let read: RequestResult | undefined = { status: 'rejected', data: { error: { code: 'OWNER_SETTINGS_UNTRUSTED', message: '' } } };
    fake.handler = request => request.method === 'get_owner_environment' ? read : undefined;
    await render(fake, [], '#/settings');
    await flush();
    expect(section()!.textContent).toContain('本机设置无法安全读取');
    expect(fake.ops('get_owner_environment')).toHaveLength(1);
    read = status({ pathReady: false, tools: fixture.tools.map(tool => ({ ...tool, found: false })) });
    await press(section()!, '重新读取');
    await flush();
    expect(fake.ops('get_owner_environment')).toHaveLength(2);
    expect(section()!.textContent).toContain('PATH 未就绪');
    expect(section()!.textContent).toContain('只包含绝对目录');
    expect(section()!.textContent).toContain('git 没找到');
    // A reconnect reads afresh; until that answer arrives nothing can be changed.
    read = undefined;
    fake.sent.length = 0;
    await act(async () => { client!.disconnect(); await client!.connect(); });
    await act(async () => handshake(fake, []));
    await flush();
    expect(fake.ops('get_owner_environment')).toHaveLength(1);
    expect(rows('HTTPS_PROXY')[0].querySelector('button')!.disabled).toBe(true);
  });

  it('is not offered away from this computer, in Settings or on a first run', async () => {
    const fake = new FakeTransport();
    fake.handler = request => request.method === 'get_owner_environment' ? status() : undefined;
    await render(fake, [], '#/settings', false);
    await flush();
    expect(section()).toBeNull();
    await act(async () => { window.location.hash = ''; window.dispatchEvent(new HashChangeEvent('hashchange')); });
    fake.sent.length = 0;
    await act(async () => { client!.disconnect(); await client!.connect(); });
    await act(async () => handshake(fake, [], []));
    await flush();
    expect(document.querySelector('.first-run')).not.toBeNull();
    expect(document.querySelector('.proxy-prompt')).toBeNull();
    expect(fake.ops('get_owner_environment')).toHaveLength(0);
  });

  it('offers a detected proxy, masked, on first run until it is adopted', async () => {
    const fake = new FakeTransport();
    fake.handler = request => request.method === 'get_owner_environment' ? status() : request.method === 'update_owner_environment' ? status({ configured: [adopted] }) : undefined;
    client = new DaemonClient(() => fake);
    const element = document.createElement('div'); document.body.append(element);
    root = createRoot(element);
    await act(async () => root!.render(<App client={client} nativeAvailable />));
    await act(async () => handshake(fake, [], []));
    await flush();
    const prompt = () => document.querySelector('.proxy-prompt');
    expect(prompt()?.textContent).toContain('HTTPS_PROXY http://•••:•••@proxy.invalid:3128');
    await press(prompt()!, '使用');
    await flush();
    expect(fake.ops('update_owner_environment')[0].request).toEqual({ method: 'update_owner_environment', params: { name: 'HTTPS_PROXY', change: { action: 'adopt_detected' } } });
    expect(prompt()?.textContent).not.toContain('Agent 需要通过它访问模型吗');
    expect(prompt()?.querySelector('.outcome-mark')?.getAttribute('data-outcome')).toBe('done');
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
