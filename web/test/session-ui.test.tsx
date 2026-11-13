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
import type { RuntimeSummary } from '../src/protocol/generated/RuntimeSummary';
import type { ServerMessage } from '../src/protocol/generated/ServerMessage';
import type { SessionSnapshot } from '../src/protocol/generated/SessionSnapshot';
import type { SessionView } from '../src/protocol/generated/SessionView';
import type { TranscriptMessage } from '../src/protocol/generated/TranscriptMessage';
import type { OwnerEnvironment } from '../src/protocol/generated/OwnerEnvironment';
import type { OwnerEnvironmentEntry } from '../src/protocol/generated/OwnerEnvironmentEntry';
import { ownerEnvironmentResponses, sessionResponse } from '../src/protocol-fixtures.generated';
import proposal from '../../fixtures/session-activity/proposal.json';

const dialog = vi.hoisted(() => ({ open: vi.fn<(options: unknown) => Promise<string | null>>() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: dialog.open }));

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
  reply(requestId: string, result: RequestResult) { this.onMessage({ type: 'response', protocolVersion: 9, requestId, result }); }
  event(sequence: number, event: Extract<ServerMessage, { type: 'event' }>['event']) { this.onMessage({ type: 'event', protocolVersion: 9, eventStreamId: 'boot-1', sequence, event }); }
  ops(method?: Request['method']) { return this.sent.filter(item => item.requestId.includes('-op-') && (!method || item.request.method === method)); }
}

function handshake(fake: FakeTransport, sessions: SessionSnapshot[], projects = [{ id: 'project-fixture', name: '茶馆项目', cwdFence: 'cwd-fixture' }], runtimes: RuntimeSummary[] = [{ id: 'pi', name: 'Pi', version: '1.5.0', available: true, toolApproval: 'never_asks' }]) {
  fake.onMessage({ type: 'welcome', protocolVersion: 9, daemonVersion: '0.1.0' });
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
    // One title row: a small back arrow, the session title as the page title, state, chips and actions.
    expect(document.querySelector('.page-heading')).toBeNull();
    const heading = document.querySelector('.session-heading')!;
    expect(heading.querySelector('.back-link')?.getAttribute('href')).toBe('#/sessions');
    expect(heading.querySelector('.back-link')?.getAttribute('aria-label')).toBe('返回会话列表');
    expect(heading.querySelector('h1#page-title')?.textContent).toBe(document.querySelector('.page-content')!.querySelector('h1')!.textContent);
    expect(heading.querySelector('.state-pill')).not.toBeNull();
    expect(heading.querySelector('.session-chips')).not.toBeNull();
    expect(heading.querySelector('.session-actions')?.textContent).toContain('关闭会话');
    // The fixture session does not say whether it asks; that is never shown as safe, and the advice opens on a tap.
    const approval = document.querySelector<HTMLButtonElement>('.session-chips button.tool-approval')!;
    expect(approval.textContent).toBe('未说明是否会询问 ⓘ');
    expect(document.querySelector('.session-chips [role="note"]')).toBeNull();
    expect(approval.hasAttribute('aria-describedby')).toBe(false);
    await act(async () => { approval.click(); });
    expect(approval.getAttribute('aria-expanded')).toBe('true');
    // A screen reader hears the advice on the button itself, not only as text beside it.
    expect(document.getElementById(approval.getAttribute('aria-describedby')!)?.textContent).toBe('Agent 没有说明是否会询问，请按自动执行对待。');
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
    await act(async () => { button('插话')!.click(); });
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
    await act(async () => { button('稍后发送')!.click(); });
    await flush();
    expect(fake.ops('prompt')[1].request).toMatchObject({ params: { content: '第三条', delivery: 'follow_up' } });
    expect(input.value).toBe('第三条');
    expect(bubbles()).toHaveLength(1);
    expect(bubbles()[0]).toContain('未完成');
    promptResult = { status: 'unknown', data: { error: { code: 'TIMEOUT', message: '' } } };
    await act(async () => { button('稍后发送')!.click(); });
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
    expect(button('插话')).toBeUndefined();
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
    expect(rows).toEqual(['Pi 1.5.0以你的身份自动执行工具 ⓘ就绪', 'Codex未安装或未通过检测。不可用']);
    const input = document.querySelector('#project-path') as HTMLInputElement;
    // The system folder picker only fills the field; a cancelled or failed picker changes nothing.
    dialog.open.mockRejectedValueOnce(new Error('denied'));
    await act(async () => { button('选择文件夹…')!.click(); });
    expect(input.value).toBe('');
    dialog.open.mockResolvedValueOnce('/home/alex/cafe');
    await act(async () => { button('选择文件夹…')!.click(); });
    expect(dialog.open).toHaveBeenLastCalledWith({ directory: true, multiple: false });
    expect(input.value).toBe('/home/alex/cafe');
    expect(fake.ops('add_project')).toHaveLength(0);
    await act(async () => { button('批准文件夹')!.click(); });
    await flush();
    expect(fake.ops('add_project')[0].request).toEqual({ method: 'add_project', params: { root: '/home/alex/cafe' } });
    // The approved project arrives as an event; the page then offers New session.
    await act(async () => fake.event(1, { kind: 'project_changed', data: { project: { id: 'p-new', name: 'cafe', cwdFence: 'f-new' } } }));
    expect(document.querySelector('.first-run')).toBeNull();
    expect(button('新建会话')?.disabled).toBe(false);
  });

  describe('agents', () => {
    const pi = (available: boolean): RuntimeSummary => ({ id: 'pi', name: 'Pi', version: '1.5.0', available, toolApproval: 'never_asks' });
    const rows = () => [...document.querySelectorAll('.runtime-list li')].map(item => item.textContent);
    const type = (selector: string, value: string) => act(async () => {
      const input = document.querySelector(selector) as HTMLInputElement;
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value); input.dispatchEvent(new Event('input', { bubbles: true }));
    });
    const environment = (pathReady: boolean) => applied({ kind: 'owner_environment', data: { detected: [], configured: [], pathReady, tools: [] } });
    async function start(fake: FakeTransport, runtimes: RuntimeSummary[], projects: { id: string; name: string; cwdFence: string }[] = [], { hash = '', native = true } = {}) {
      window.location.hash = hash;
      client = new DaemonClient(() => fake);
      const element = document.createElement('div'); document.body.append(element);
      root = createRoot(element);
      await act(async () => root!.render(<App client={client} nativeAvailable={native} />));
      if (!native) await act(async () => { void client!.connect(); });
      await act(async () => handshake(fake, [], projects, runtimes));
      await flush();
    }

    it('prefills both paths from local discovery without registering Pi automatically', async () => {
      const fake = new FakeTransport();
      fake.handler = request => request.method === 'discover_pi_runtime' ? applied({ kind: 'pi_runtime_discovery', data: { node: 'C:\\Program Files\\nodejs\\node.exe', packageRoot: 'C:\\npm\\node_modules\\@earendil-works\\pi-coding-agent' } }) : undefined;
      await start(fake, []);
      expect((document.querySelector('#pi-node') as HTMLInputElement).value).toBe('C:\\Program Files\\nodejs\\node.exe');
      expect((document.querySelector('#pi-package') as HTMLInputElement).value).toBe('C:\\npm\\node_modules\\@earendil-works\\pi-coding-agent');
      expect(fake.ops('discover_pi_runtime')).toHaveLength(1);
      expect(fake.ops('register_pi_runtime')).toHaveLength(0);
      expect(document.querySelector('.pi-registration')!.textContent).not.toContain('npm install');
    });

    it('shows the install command only after discovery misses Node or the Pi package', async () => {
      const fake = new FakeTransport();
      await start(fake, []);
      const form = () => document.querySelector('.pi-registration')!.textContent;
      expect(form()).not.toContain('npm install');
      fake.reply(fake.ops('discover_pi_runtime')[0].requestId, applied({ kind: 'pi_runtime_discovery', data: { node: 'C:\\nodejs\\node.exe', packageRoot: null } }));
      await flush();
      expect(form()).toContain('还没装 Pi：装好 Node 后运行 npm install -g @earendil-works/pi-coding-agent@1.5.0');
      expect((document.querySelector('#pi-node') as HTMLInputElement).value).toBe('C:\\nodejs\\node.exe');
    });

    it('lists a missing Pi and Codex, registers Pi from two paths, and keeps the first run until an agent is ready', async () => {
      const fake = new FakeTransport();
      let register: RequestResult = { status: 'rejected', data: { error: { code: 'PI_VERSION_UNSUPPORTED', message: '' } } };
      fake.handler = request => request.method === 'register_pi_runtime' ? register : undefined;
      await start(fake, [], [{ id: 'p1', name: '茶馆项目', cwdFence: 'f1' }]);
      expect(rows()).toEqual(['Pi还没登记。需要 Node 和 Pi 1.5.0。未登记', 'Codex这一版还不能启动 Codex。不可用']);
      const page = document.querySelector('.first-run')!;
      expect(page.textContent).toContain('至少需要一个就绪的 Agent 才能开始会话');
      expect(page.textContent).toContain('已批准：茶馆项目');
      expect(page.textContent).not.toMatch(/只|不会|不代表/);
      expect(page.textContent).not.toContain('npm root');
      expect(page.querySelector('#project-path')).toBeNull();
      expect(button('登记 Pi')!.disabled).toBe(true);
      expect(fake.ops('discover_pi_runtime')).toHaveLength(1);
      await type('#pi-node', ' C:\\nodejs\\node.exe ');
      const discovery = fake.ops('discover_pi_runtime')[0];
      fake.reply(discovery.requestId, applied({ kind: 'pi_runtime_discovery', data: { node: 'C:\\detected\\node.exe', packageRoot: 'C:\\detected\\node_modules\\@earendil-works\\pi-coding-agent' } }));
      await flush();
      expect((document.querySelector('#pi-node') as HTMLInputElement).value).toBe(' C:\\nodejs\\node.exe ');
      expect((document.querySelector('#pi-package') as HTMLInputElement).value).toBe('C:\\detected\\node_modules\\@earendil-works\\pi-coding-agent');
      await type('#pi-package', 'C:\\npm\\node_modules\\@earendil-works\\pi-coding-agent\t');
      await act(async () => { button('登记 Pi')!.click(); });
      await flush();
      expect(fake.ops('register_pi_runtime')[0].request).toEqual({ method: 'register_pi_runtime', params: { node: 'C:\\nodejs\\node.exe', packageRoot: 'C:\\npm\\node_modules\\@earendil-works\\pi-coding-agent' } });
      expect(document.querySelector('.pi-registration .outcome-mark')!.textContent).toContain('需要 Pi 1.5.0，这个目录里是其他版本');
      register = applied({ kind: 'runtimes', data: { items: [pi(true)] } });
      await act(async () => { button('登记 Pi')!.click(); });
      await flush();
      // A project and a ready agent end the first run on the empty Sessions page.
      expect(document.querySelector('.first-run')).toBeNull();
      expect(button('新建会话')?.disabled).toBe(false);
    });

    it('reads the list back after an unknown registration instead of sending it again', async () => {
      const fake = new FakeTransport();
      fake.handler = request => request.method === 'register_pi_runtime' ? { status: 'unknown', data: { error: { code: 'PI_RUNTIME_WRITE_UNKNOWN', message: '' } } } :
        request.method === 'list_runtimes' ? applied({ kind: 'runtimes', data: { items: [] } }) : undefined;
      await start(fake, []);
      const reads = fake.ops('list_runtimes').length;
      await type('#pi-node', '/usr/bin/node');
      await type('#pi-package', '/usr/lib/node_modules/@earendil-works/pi-coding-agent');
      await act(async () => { button('登记 Pi')!.click(); });
      await flush();
      expect(fake.ops('register_pi_runtime')).toHaveLength(1);
      expect(fake.ops('list_runtimes')).toHaveLength(reads + 1);
      expect(document.querySelector('.pi-registration .outcome-mark')?.getAttribute('data-outcome')).toBe('unknown');
    });

    it('says why a registered Pi is unavailable, and offers registration again only when PATH is not the cause', async () => {
      const fake = new FakeTransport();
      let ready = false;
      fake.handler = request => request.method === 'get_owner_environment' ? environment(ready) : undefined;
      await start(fake, [pi(false)], [], { hash: '#/settings' });
      const section = () => document.querySelector('section[aria-labelledby="agent-title"]')!;
      expect(section().querySelector('.runtime-list li')!.textContent).toContain('PATH 未就绪');
      expect(section().querySelector('a[href="#/settings"]')).not.toBeNull();
      expect(section().querySelector('.pi-registration')).toBeNull();
      ready = true;
      await act(async () => { [...section().querySelectorAll('button')].find(item => item.textContent === '重新检测')!.click(); });
      await flush();
      expect(section().querySelector('.runtime-list li')!.textContent).toContain('登记的 Node 或 Pi 位置已经不能用了');
      expect(section().querySelector('.pi-registration')).not.toBeNull();
    });

    it('lists missing agents in the new-session sheet with the reason, and registers only at this computer', async () => {
      await start(new FakeTransport(), [], [{ id: 'p1', name: '茶馆项目', cwdFence: 'f1' }]);
      await act(async () => { window.dispatchEvent(new KeyboardEvent('keydown', { key: 'n', ctrlKey: true })); });
      const choices = [...document.querySelectorAll('.sheet fieldset')[1].querySelectorAll('.choice')];
      expect(choices.map(item => item.textContent)).toEqual(['Pi还没登记。需要 Node 和 Pi 1.5.0。 到设置处理', 'Codex这一版还不能启动 Codex。']);
      expect(choices.every(item => item.querySelector('input')!.disabled)).toBe(true);
      await act(async () => { root!.unmount(); client!.disconnect(); });
      document.body.replaceChildren();
      const remote = new FakeTransport();
      await start(remote, [pi(false)], [], { native: false });
      expect(document.querySelector('.first-run')!.textContent).toContain('请在电脑上登记 Pi');
      expect(document.querySelector('#pi-node')).toBeNull();
      expect(remote.ops('discover_pi_runtime')).toHaveLength(0);
    });
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
    expect(document.querySelector('.choice .tool-approval')?.textContent).toBe('以你的身份自动执行工具');
    await act(async () => { (document.querySelector('input[name="project"]') as HTMLInputElement).click(); (document.querySelector('input[name="runtime"][value="pi"]') as HTMLInputElement).click(); });
    expect(document.querySelector('.sheet')?.textContent).toContain('以你的身份在项目里直接修改文件');
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

describe('session resume requests', () => {
  const stopped: SessionSnapshot = { ...base, state: { state: 'stopped' }, stopReason: 'AGENT_EXITED' };
  const resumeButton = () => document.querySelector<HTMLButtonElement>('.session-actions button')!;
  const open = (result: () => RequestResult | undefined) => {
    const fake = new FakeTransport();
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: { ...baseView, session: stopped }, eventStreamId: 'boot-1', sequence: 0 } }) :
      request.method === 'create_session' ? result() : undefined;
    return fake;
  };

  it.each(['TIMEOUT', 'CONNECTION_LOST'])('retains the same Resume attempt after %s without automatic retry', async code => {
    let result: RequestResult = { status: 'unknown', data: { error: { code, message: '' } } };
    const fake = open(() => result);
    await render(fake, [stopped], '#/sessions/session-fixture');
    await act(async () => { resumeButton().click(); });
    await flush();
    const first = fake.ops('create_session')[0].request as Extract<Request, { method: 'create_session' }>;
    expect(first).toMatchObject({ method: 'create_session', params: { projectId: 'project-fixture', runtimeId: 'pi', expectedCwdFence: 'cwd-fixture', start: { mode: 'resume', params: { sourceSessionId: base.id } } } });
    expect(window.location.hash).toBe('#/sessions/session-fixture');
    expect(document.querySelector('.session-actions [data-outcome="unknown"]')).not.toBeNull();
    expect(fake.ops('create_session')).toHaveLength(1);
    expect(resumeButton().textContent).toBe('再次确认');
    result = { status: 'dispatched', data: { operationId: 'resume-1' } };
    await act(async () => { resumeButton().click(); });
    await flush();
    expect(fake.ops('create_session')).toHaveLength(2);
    expect(fake.ops('create_session')[1].request).toEqual(first);
    expect(window.location.hash).toBe(`#/sessions/${first.params.sessionId}`);
  });

  it('keeps Resume unknown after a client timeout and ignores its late reply', async () => {
    const fake = open(() => undefined);
    await render(fake, [stopped], '#/sessions/session-fixture');
    vi.useFakeTimers();
    await act(async () => { resumeButton().click(); });
    const first = fake.ops('create_session')[0];
    await act(async () => { await vi.advanceTimersByTimeAsync(29999); });
    expect(resumeButton().disabled).toBe(true);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(document.querySelector('.session-actions [data-outcome="unknown"]')?.textContent).toContain('TIMEOUT');
    expect(resumeButton().disabled).toBe(false);
    await act(async () => fake.reply(first.requestId, { status: 'dispatched', data: { operationId: 'late-resume' } }));
    expect(client!.getSnapshot().connection).toBe('connected');
    expect(window.location.hash).toBe('#/sessions/session-fixture');
    expect(resumeButton().textContent).toBe('再次确认');
    expect(fake.ops('create_session')).toHaveLength(1);
    await act(async () => { resumeButton().click(); });
    expect(fake.ops('create_session')).toHaveLength(2);
    expect(fake.ops('create_session')[1].request).toEqual(first.request);
  });

  it('preserves Resume params across transport loss and reconnect without replaying creation', async () => {
    const fake = open(() => undefined);
    await render(fake, [stopped], '#/sessions/session-fixture');
    await act(async () => { resumeButton().click(); });
    const first = fake.ops('create_session')[0];
    const oldOnMessage = fake.onMessage;
    await act(async () => { await fake.close(); });
    expect(client!.getSnapshot().connection).toBe('disconnected');
    expect(document.querySelector('.session-actions [data-outcome="unknown"]')?.textContent).toContain('CONNECTION_LOST');
    expect(resumeButton().disabled).toBe(true);
    await act(async () => { resumeButton().click(); });
    expect(fake.ops('create_session')).toHaveLength(1);
    // Reuse the fake socket, but keep only this connection's requests for its handshake.
    fake.sent = [];
    await act(async () => { button('重新连接')!.click(); });
    await act(async () => handshake(fake, [stopped], [{ id: 'project-fixture', name: '茶馆项目', cwdFence: 'cwd-rotated' }]));
    await flush();
    expect(client!.getSnapshot().connection).toBe('connected');
    expect(client!.getSnapshot().projects?.[0].cwdFence).toBe('cwd-rotated');
    expect(resumeButton().disabled).toBe(false);
    expect(resumeButton().textContent).toBe('再次确认');
    expect(fake.ops('create_session')).toHaveLength(0);
    await act(async () => oldOnMessage({ type: 'response', protocolVersion: 9, requestId: first.requestId, result: { status: 'dispatched', data: { operationId: 'old-resume' } } }));
    expect(window.location.hash).toBe('#/sessions/session-fixture');
    expect(document.querySelector('.session-actions [data-outcome="unknown"]')?.textContent).toContain('CONNECTION_LOST');
    await act(async () => { resumeButton().click(); });
    expect(fake.ops('create_session')).toHaveLength(1);
    const confirmation = fake.ops('create_session')[0];
    expect(confirmation.requestId).not.toBe(first.requestId);
    expect(confirmation.request).toEqual(first.request);
    expect(confirmation.request).toMatchObject({ params: { expectedCwdFence: 'cwd-fixture' } });
    await act(async () => fake.reply(confirmation.requestId, { status: 'dispatched', data: { operationId: 'resume-confirmed' } }));
    const params = (first.request as Extract<Request, { method: 'create_session' }>).params;
    expect(window.location.hash).toBe(`#/sessions/${params.sessionId}`);
  });

  it.each(['applied', 'dispatched'] as const)('does not navigate on a %s Resume reply after leaving the session page', async status => {
    const fake = open(() => undefined);
    await render(fake, [stopped], '#/sessions/session-fixture');
    await act(async () => { resumeButton().click(); });
    const first = fake.ops('create_session')[0];
    const params = (first.request as Extract<Request, { method: 'create_session' }>).params;
    await act(async () => { window.location.hash = '#/sessions'; window.dispatchEvent(new HashChangeEvent('hashchange')); });
    expect(document.querySelector('.session-view')).toBeNull();
    expect(client!.getSnapshot().session).toBeNull();
    const result: RequestResult = status === 'dispatched' ? { status, data: { operationId: 'late-resume' } } :
      applied({ kind: 'session', data: { session: { ...baseView, session: { ...base, id: params.sessionId } }, eventStreamId: 'boot-1', sequence: 0 } });
    await act(async () => fake.reply(first.requestId, result));
    expect(window.location.hash).toBe('#/sessions');
    expect(client!.getSnapshot().connection).toBe('connected');
    expect(fake.ops('create_session')).toHaveLength(1);
    expect(fake.ops('cancel')).toHaveLength(0);
    expect(fake.ops('close_session')).toHaveLength(0);
  });

  it('blocks duplicate pending Resume clicks and permits a new ID after a definitive rejection', async () => {
    const fake = open(() => undefined);
    await render(fake, [stopped], '#/sessions/session-fixture');
    await act(async () => { resumeButton().click(); resumeButton().click(); });
    expect(fake.ops('create_session')).toHaveLength(1);
    expect(resumeButton().disabled).toBe(true);
    const first = fake.ops('create_session')[0];
    await act(async () => fake.reply(first.requestId, { status: 'rejected', data: { error: { code: 'STALE_CWD_FENCE', message: '' } } }));
    expect(resumeButton().disabled).toBe(false);
    expect(document.querySelector('.session-actions [data-outcome="not_done"]')).not.toBeNull();
    await act(async () => { resumeButton().click(); });
    const second = fake.ops('create_session')[1];
    expect(second.request).not.toEqual(first.request);
    expect(second.request).toMatchObject({ method: 'create_session', params: { start: { mode: 'resume', params: { sourceSessionId: base.id } } } });
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
      .toEqual(['Pi', '1.5.0', '茶馆项目', '/home/alex/茶馆', '继续自之前的会话', '某些工具前可能会询问 ⓘ', 'anthropic / claude']);
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

  it('says on a tap why it cannot change: the agent cannot, or is busy until it is idle again', async () => {
    await open({ ...base, capabilities: { ...base.capabilities, thinkingControl: false } });
    expect(trigger()).toBeNull();
    const chip = [...document.querySelectorAll<HTMLButtonElement>('.session-chips button.chip')].find(each => each.textContent === '思考强度：中 ⓘ')!;
    expect(chip.getAttribute('aria-expanded')).toBe('false');
    await act(async () => { chip.click(); });
    expect(chip.parentElement?.querySelector('[role="note"]')?.textContent).toBe('这个 Agent 不支持调整思考强度');
    await act(async () => { root?.unmount(); client?.disconnect(); });
    const fake = await open({ ...base, state: { state: 'running' }, activeRunId: 'run-1' });
    const busy = document.querySelector<HTMLButtonElement>('.thinking-control .chip-button')!;
    expect(busy.disabled).toBe(false);
    expect(trigger()).toBeNull();
    await act(async () => { busy.click(); });
    expect(document.querySelector('[role="menuitemradio"]')).toBeNull();
    expect(document.getElementById(busy.getAttribute('aria-describedby')!)?.textContent).toBe('会话空闲时才能调整思考强度');
    // Once idle, the reason goes and the menu stays shut until asked for.
    await act(async () => fake.event(1, { kind: 'session_changed', data: { session: base } }));
    expect(document.querySelector('.thinking-control [role="note"]')).toBeNull();
    expect(trigger()?.getAttribute('aria-expanded')).toBe('false');
    expect(trigger()?.hasAttribute('aria-describedby')).toBe(false);
    expect(document.querySelector('[role="menuitemradio"]')).toBeNull();
    expect(fake.ops('set_thinking')).toHaveLength(0);
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

  it('says on a tap why it cannot switch: the agent cannot, it is away from this computer, or the agent is busy', async () => {
    let fake = await open({ ...withModel, capabilities: { ...base.capabilities, modelControl: false } });
    expect(trigger()).toBeNull();
    const reason = async () => {
      const chip = [...document.querySelectorAll<HTMLButtonElement>('.session-chips button.chip')].find(each => each.textContent === '模型：openrouter / qwen/qwen3-coder ⓘ')!;
      await act(async () => { chip.click(); });
      return chip.parentElement?.querySelector('[role="note"]')?.textContent;
    };
    expect(await reason()).toBe('这个 Agent 不支持切换模型');
    await act(async () => { root?.unmount(); client?.disconnect(); });
    fake = await open(withModel, () => models(A), false);
    expect(trigger()).toBeNull();
    expect([...document.querySelectorAll('.session-chips span.chip')].map(each => each.textContent)).toContain('模型：openrouter / qwen/qwen3-coder');
    expect(fake.ops('list_models')).toHaveLength(0);
    await act(async () => { root?.unmount(); client?.disconnect(); });
    // Busy, the chip reads nothing: a list asked for now could not be used.
    fake = await open({ ...withModel, state: { state: 'running' }, activeRunId: 'run-1' }, () => models(A));
    await act(async () => { document.querySelector<HTMLButtonElement>('.model-control .chip-button')!.click(); });
    expect(note('[role="note"]')).toBe('会话空闲时才能切换模型');
    expect(items()).toHaveLength(0);
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
    expect(section()!.textContent).toContain('还没有。');
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
    expect(fake.ops('get_owner_environment')).toHaveLength(0);
  });

  const firstRun = async (fake: FakeTransport) => {
    client = new DaemonClient(() => fake);
    const element = document.createElement('div'); document.body.append(element);
    root = createRoot(element);
    await act(async () => root!.render(<App client={client} nativeAvailable />));
    await act(async () => handshake(fake, [], []));
    await flush();
  };

  it('leaves detected proxies to inheritance: no first-run prompt, and Settings only pins or restores them', async () => {
    const fake = new FakeTransport();
    fake.handler = request => request.method === 'get_owner_environment' ? status() : request.method === 'update_owner_environment' ? status({ configured: [...fixture.configured, adopted] }) : undefined;
    await firstRun(fake);
    expect(document.querySelector('.first-run')!.textContent).not.toContain('HTTPS_PROXY');
    expect(fake.ops('get_owner_environment')).toHaveLength(0);
    await act(async () => { window.location.hash = '#/settings'; window.dispatchEvent(new HashChangeEvent('hashchange')); });
    await flush();
    expect(rows('HTTPS_PROXY')[0].querySelector('button')!.textContent).toBe('固定为当前值');
    await press(rows('HTTPS_PROXY')[0]);
    await flush();
    await press(rows('HTTPS_PROXY')[0], '移除');
    expect(rows('HTTPS_PROXY')[0].textContent).toContain('移除后改用守护进程环境里的值。');
  });
});

describe('session list', () => {
  afterEach(() => localStorage.clear());
  const pi = { id: 'pi', name: 'Pi', version: '1.5.0', available: true, toolApproval: 'never_asks' as const };
  const codex = { id: 'codex', name: 'Codex', available: true, toolApproval: 'unknown' as const };
  const projects = [{ id: 'p1', name: '茶馆项目', cwdFence: 'f1' }, { id: 'p2', name: '后台', cwdFence: 'f2' }];
  const item = (id: string, projectId: string, state: SessionSnapshot['state'], extra: Partial<SessionSnapshot> = {}): SessionSnapshot => ({ ...base, id, projectId, title: id, state, model: null, thinkingLevel: null, ...extra });
  // Creation order, as the daemon lists them: the newest is last. None has an activity time, so the list orders them by ID.
  const sessions = [item('f-失败', 'p1', { state: 'failed', data: { code: 'PROCESS_SPAWN_FAILED' } }), item('e-后台旧', 'p2', { state: 'ready' }),
    item('a-空闲', 'p1', { state: 'ready' }), item('g-后台新', 'p2', { state: 'ready' }, { runtimeId: 'codex' }),
    item('c-最新', 'p1', { state: 'ready' }, { model: { provider: 'anthropic', id: 'claude-opus-5-5' }, thinkingLevel: 'medium' }), item('b-旧会话', 'gone', { state: 'stopped' })];
  async function mount(fake: FakeTransport, list: SessionSnapshot[], runtimes = [pi, codex], native = true) {
    fake.handler = request => request.method === 'get_project_root' ? applied({ kind: 'project_root', data: { projectId: request.params.projectId, cwdFence: request.params.projectId === 'p1' ? 'f1' : 'f2', root: `/home/alex/${request.params.projectId}` } }) : undefined;
    client = new DaemonClient(() => fake);
    const element = document.createElement('div'); document.body.append(element);
    root = createRoot(element);
    await act(async () => root!.render(<App client={client} nativeAvailable={native} />));
    if (!native) await act(async () => { void client!.connect(); });
    await act(async () => handshake(fake, list, projects, runtimes));
    await flush();
  }
  const groups = () => [...document.querySelectorAll('.session-group')];
  const group = (name: string) => groups().find(item => item.getAttribute('aria-label') === name)!;
  const links = () => [...document.querySelectorAll('.session-row')].map(item => decodeURIComponent(item.getAttribute('href')!.replace('#/sessions/', '')));
  const toggle = (name: string) => act(async () => { group(name).querySelector<HTMLButtonElement>('.group-toggle')!.click(); });
  const agentMenu = async () => {
    const trigger = document.querySelector<HTMLButtonElement>('.agent-switch .chip-button')!;
    if (trigger.getAttribute('aria-expanded') !== 'true') await act(async () => { trigger.click(); });
    return [...document.querySelectorAll('.agent-switch [role="menuitemradio"]')] as HTMLButtonElement[]; };
  const search = (text: string) => act(async () => { const input = document.querySelector('.session-search') as HTMLInputElement; Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, text); input.dispatchEvent(new Event('input', { bubbles: true })); });

  it('groups by project, collapsed, each standing for the session that most needs the owner', async () => {
    const fake = new FakeTransport();
    await mount(fake, sessions.map(session => ({ ...session, runtimeId: 'pi' })), [pi]);
    expect(groups().map(item => item.getAttribute('aria-label'))).toEqual(['茶馆项目', '后台', '已移除项目的会话']);
    expect(links()).toEqual([]);
    // A failed session outranks idle ones; otherwise the first in list order stands for the project.
    expect(groups().map(item => item.querySelector('.group-lead')?.textContent)).toEqual(['f-失败', 'e-后台旧', 'b-旧会话']);
    // The list may cut a failure reason short, so its pill carries the whole reason as a tooltip.
    expect(group('茶馆项目').querySelector('.state-pill')?.getAttribute('title')).toBe('失败 · Agent 进程没能启动');
    expect(group('后台').querySelector('.state-pill')?.hasAttribute('title')).toBe(false);
    expect(group('茶馆项目').querySelector('.group-count')?.textContent).toBe('3 个会话');
    expect(group('茶馆项目').querySelector('.group-path')?.textContent).toBe('/home/alex/p1');
    expect(document.querySelector('.agent-switch .chip-button')?.textContent).toBe('Pi · 6 ▾');
    expect((await agentMenu()).map(option => option.textContent)).toEqual(['Pi · 6']);
    await toggle('茶馆项目');
    expect(group('茶馆项目').querySelector('.group-toggle')?.getAttribute('aria-expanded')).toBe('true');
    expect(group('茶馆项目').querySelector('.group-lead')).toBeNull();
    expect(links()).toEqual(['a-空闲', 'c-最新', 'f-失败']);
    expect([...document.querySelector('.session-row-meta')!.children].map(part => part.textContent)).toEqual(['anthropic / claude-opus-5-5', '思考：中']);
    // The open project stays open when the list comes back.
    await act(async () => { root!.unmount(); client!.disconnect(); });
    await mount(new FakeTransport(), sessions.map(session => ({ ...session, runtimeId: 'pi' })), [pi]);
    expect(links()).toHaveLength(3);
  });

  it('shows one Agent at a time, remembers it, and names the Agent only when showing all', async () => {
    const fake = new FakeTransport();
    await mount(fake, sessions);
    expect(document.querySelector('.agent-switch .chip-button')?.textContent).toBe('全部 Agent · 6 ▾');
    expect((await agentMenu()).map(option => option.textContent)).toEqual(['全部 Agent · 6', 'Pi · 5', 'Codex · 1']);
    await toggle('后台');
    expect([...group('后台').querySelectorAll('.session-row-meta')].map(item => item.textContent)).toEqual(['Pi', 'Codex']);
    await act(async () => { (await agentMenu())[2].click(); });
    expect(groups().map(item => item.getAttribute('aria-label'))).toEqual(['后台']);
    expect(links()).toEqual(['g-后台新']);
    expect(document.querySelector('.session-row-meta')).toBeNull();
    expect(localStorage.getItem('cafe-space.sessions.agent')).toBe('codex');
    // A remembered Agent without sessions falls back to all.
    await act(async () => { root!.unmount(); client!.disconnect(); });
    await mount(new FakeTransport(), sessions.filter(session => session.runtimeId !== 'codex'), [pi, codex]);
    expect(document.querySelector('.agent-switch .chip-button')?.textContent).toBe('Pi · 5 ▾');
  });

  it('searches titles and project names and opens every match', async () => {
    const fake = new FakeTransport();
    await mount(fake, sessions);
    await search('最新');
    expect(groups().map(item => item.getAttribute('aria-label'))).toEqual(['茶馆项目']);
    expect(group('茶馆项目').querySelector('.group-toggle')?.getAttribute('aria-expanded')).toBe('true');
    expect(links()).toEqual(['c-最新']);
    // A project name matches every session in it.
    await search('茶馆');
    expect(links()).toEqual(['a-空闲', 'c-最新', 'f-失败']);
    await search('不存在');
    expect(document.body.textContent).toContain('没有匹配“不存在”的会话。');
    await search('');
    expect(links()).toEqual([]);
  });

  it('reads project paths only at this computer', async () => {
    const fake = new FakeTransport();
    await mount(fake, sessions, [pi, codex], false);
    expect(groups()).toHaveLength(3);
    expect(document.querySelector('.group-path')).toBeNull();
    expect(fake.ops('get_project_root')).toHaveLength(0);
  });

  it.each(['null', '{}', '0', '"p1"', '[1,"p1"]', 'not json'])('starts collapsed from an unusable stored open list (%s) and writes a valid one', async stored => {
    localStorage.setItem('cafe-space.sessions.open', stored);
    await mount(new FakeTransport(), sessions);
    expect(groups()).toHaveLength(3);
    expect(links()).toEqual([]);
    await toggle('茶馆项目');
    expect(links()).toHaveLength(3);
    expect(localStorage.getItem('cafe-space.sessions.open')).toBe('["p1"]');
  });

  describe('activity', () => {
    const now = new Date(2026, 10, 7, 15, 0).getTime();
    const before = (minutes: number) => now - minutes * 60_000;
    const ready = { state: 'ready' } as const;
    const times = () => [...document.querySelectorAll('.session-row')].map(row => row.querySelector('time')?.textContent ?? null);
    const previews = () => [...document.querySelectorAll('.session-row')].map(row => row.querySelector('.session-row-preview')?.textContent ?? null);
    const clock = () => vi.useFakeTimers({ now, toFake: ['Date', 'setInterval', 'clearInterval'] });

    it('orders projects and sessions by latest activity, ties and unknown times by ID', async () => {
      clock();
      // Creation order: m- is older than z-; r- is the most recent but its project is gone.
      const list = [item('u-旧未知', 'p1', ready), item('k-后台', 'p2', ready, { lastActivityAt: before(5) }),
        item('m-同刻', 'p1', ready, { lastActivityAt: before(90) }), item('z-同刻', 'p1', ready, { lastActivityAt: before(90) }),
        item('n-新未知', 'p1', ready), item('b-越界', 'p1', ready, { lastActivityAt: 9e15 }), item('r-移除', 'gone', ready, { lastActivityAt: now - 10_000 })];
      const fake = new FakeTransport();
      await mount(fake, list);
      expect(groups().map(item => item.getAttribute('aria-label'))).toEqual(['后台', '茶馆项目', '已移除项目的会话']);
      expect(groups().map(item => item.querySelector('.group-count')?.textContent)).toEqual(['1 个会话 · 5 分钟前', '5 个会话 · 1 小时前', '1 个会话 · 刚刚']);
      expect(group('茶馆项目').querySelector('.group-lead')?.textContent).toBe('m-同刻');
      await toggle('茶馆项目');
      expect(links()).toEqual(['m-同刻', 'z-同刻', 'b-越界', 'n-新未知', 'u-旧未知']);
      expect(times()).toEqual(['1 小时前', '1 小时前', null, null, null]);
      expect(group('茶馆项目').querySelector('time')?.getAttribute('dateTime')).toBe(new Date(before(90)).toISOString());
      // New activity moves a session and its project to the top; the clock moves relative times on.
      await act(async () => fake.event(1, { kind: 'session_changed', data: { session: { ...list[0], lastActivityAt: now } } }));
      expect(groups().map(item => item.getAttribute('aria-label'))).toEqual(['茶馆项目', '后台', '已移除项目的会话']);
      expect(links()).toEqual(['u-旧未知', 'm-同刻', 'z-同刻', 'b-越界', 'n-新未知']);
      await act(async () => { vi.advanceTimersByTime(120_000); });
      expect(times()[0]).toBe('2 分钟前');
      expect(group('后台').querySelector('.group-count')?.textContent).toBe('1 个会话 · 7 分钟前');
      // The list's one clock stops when the list goes away.
      expect(vi.getTimerCount()).toBe(1);
      await act(async () => { window.location.hash = '#/settings'; window.dispatchEvent(new HashChangeEvent('hashchange')); });
      expect(document.querySelector('.session-groups')).toBeNull();
      expect(vi.getTimerCount()).toBe(0);
    });

    it('treats a time that is not a positive in-range number as unknown', async () => {
      clock();
      const bad = ['1793952000000', NaN, Infinity, 0, 8.64e15 + 1] as unknown as number[];
      await mount(new FakeTransport(), [...bad.map((value, index) => item(`b${index}`, 'p1', ready, { lastActivityAt: value })), item('z-已知', 'p1', ready, { lastActivityAt: before(3) })]);
      expect(group('茶馆项目').querySelector('.group-count')?.textContent).toBe('6 个会话 · 3 分钟前');
      await toggle('茶馆项目');
      expect(links()).toEqual(['z-已知', 'b0', 'b1', 'b2', 'b3', 'b4']);
      expect(times()).toEqual(['3 分钟前', null, null, null, null, null]);
    });

    it('follows the shared sorting golden', async () => {
      await mount(new FakeTransport(), proposal.sorting.input.map(({ id, lastActivityAt }) => item(id, 'p1', ready, { lastActivityAt })));
      await toggle('茶馆项目');
      expect(links()).toEqual(proposal.sorting.expectedIds);
    });

    it('words times from just now to a date, treating a clock ahead of this one as just now', async () => {
      clock();
      const at = [now + 3_600_000, before(59), new Date(2026, 10, 7, 0, 30).getTime(), new Date(2026, 10, 6, 23, 0).getTime(),
        new Date(2026, 2, 2, 9, 0).getTime(), new Date(2025, 11, 31, 9, 0).getTime()];
      await mount(new FakeTransport(), at.map((time, index) => item(`s${index}`, 'p1', ready, { lastActivityAt: time })));
      await toggle('茶馆项目');
      expect(times()).toEqual(['刚刚', '59 分钟前', '14 小时前', '昨天', '3月2日', '2025年12月31日']);
    });

    it('shows the last thing said as plain text, finds it by search, and drops it when a snapshot omits it', async () => {
      const said = item('a-会话', 'p1', ready, { preview: '<b>先别发布</b> 等我看完' });
      const fake = new FakeTransport();
      await mount(fake, [item('b-会话', 'p1', ready), said]);
      expect(group('茶馆项目').querySelector('.group-lead .group-lead-preview')?.textContent).toBe('<b>先别发布</b> 等我看完');
      await search('先别发布');
      expect(links()).toEqual(['a-会话']);
      expect(previews()).toEqual(['<b>先别发布</b> 等我看完']);
      expect(document.querySelector('.session-row b')).toBeNull();
      const { preview: _preview, ...cleared } = said;
      await act(async () => fake.event(1, { kind: 'session_changed', data: { session: cleared } }));
      expect(document.body.textContent).toContain('没有匹配“先别发布”的会话。');
      await search('');
      await toggle('茶馆项目');
      expect(previews()).toEqual([null, null]);
    });
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
    // Shortcut hints are marked so a touch screen can hide them; a session's state is not one.
    const hints = [...document.querySelectorAll<HTMLElement>('.palette-hint')];
    expect(hints.filter(hint => hint.dataset.shortcut).map(hint => hint.textContent)).toEqual(['Ctrl+N', 'Ctrl+1', 'Ctrl+2', 'Ctrl+3', 'Ctrl+,']);
    expect(hints.filter(hint => !hint.dataset.shortcut)).toHaveLength(2);
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

  it('closes without Escape for touch screens: 取消, or a tap that begins and ends on the backdrop', async () => {
    await render(new FakeTransport(), [base]);
    const palette = document.querySelector<HTMLDialogElement>('.palette')!;
    const tap = (down: Element, up: Element) => act(async () => { down.dispatchEvent(new Event('pointerdown', { bubbles: true })); up.dispatchEvent(new MouseEvent('click', { bubbles: true })); });
    const show = () => act(async () => { document.querySelector<HTMLButtonElement>('.palette-button')!.click(); });
    await show();
    await act(async () => { [...palette.querySelectorAll('button')].find(each => each.textContent === '取消')!.click(); });
    expect(open()).toBe(false);
    await show();
    // A drag from the field that ends outside is not a tap on the backdrop.
    await tap(input(), palette);
    expect(open()).toBe(true);
    await tap(palette, palette);
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

describe('needs you (UX §6.4)', () => {
  afterEach(() => localStorage.clear());
  const panel = () => document.querySelector('.needs-you-panel');
  const inbox = () => document.querySelector<HTMLButtonElement>('.needs-you')!;
  const inPanel = (label: string) => [...panel()!.querySelectorAll('button')].filter(item => item.textContent === label);

  it('says there is nothing to do, without a badge, and puts the panel away on a tap elsewhere or Escape', async () => {
    await render(new FakeTransport(), [base]);
    expect(inbox().textContent).toBe('待处理');
    await act(async () => { inbox().click(); });
    expect(inbox().getAttribute('aria-expanded')).toBe('true');
    expect(panel()?.textContent).toBe('现在没有要你处理的事');
    await act(async () => { inbox().dispatchEvent(new Event('pointerdown', { bubbles: true })); });
    expect(panel()).not.toBeNull();
    await act(async () => { document.body.dispatchEvent(new Event('pointerdown', { bubbles: true })); });
    expect(panel()).toBeNull();
    await act(async () => { inbox().click(); });
    await act(async () => { panel()!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); });
    expect(panel()).toBeNull();
  });

  it('forgets approvals on a fresh snapshot and reads only sessions still waiting', async () => {
    const waiting: SessionSnapshot = { ...base, id: 's-wait', title: '等审批', state: { state: 'waiting_for_local_ui' } };
    const fake = new FakeTransport();
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: { ...baseView, session: waiting, approvals: [{ id: 'ap1', summary: 'npm install' }] }, eventStreamId: 'boot-1', sequence: 0 } }) : undefined;
    await render(fake, [waiting]);
    await flush();
    expect(inbox().textContent).toBe('待处理1');
    await act(async () => fake.onClose());
    fake.sent = [];
    await act(async () => { void client!.connect(); });
    await act(async () => handshake(fake, [{ ...waiting, state: { state: 'ready' } }]));
    await flush();
    expect(fake.ops('get_session')).toHaveLength(0);
    expect(inbox().textContent).toBe('待处理');
  });

  it('lists approvals, failures and unknown outcomes by urgency, and each leaves once handled', async () => {
    const waiting: SessionSnapshot = { ...base, id: 's-wait', title: '等审批', state: { state: 'waiting_for_local_ui' }, streamId: 'stream-w' };
    const failed: SessionSnapshot = { ...base, id: 's-fail', title: '失败了', state: { state: 'failed', data: { code: 'PI_UNOWNED_RUN' } } };
    const running: SessionSnapshot = { ...base, id: 's-run', title: '在跑', state: { state: 'running' }, activeRunId: 'run-1' };
    const fake = new FakeTransport();
    let seq = 0;
    let deny: RequestResult = { status: 'unknown', data: { error: { code: 'TIMEOUT', message: '' } } };
    const views: Record<string, SessionView> = {
      's-wait': { ...baseView, session: waiting, approvals: [{ id: 'ap1', summary: 'npm install' }] },
      's-run': { ...baseView, session: running, approvals: [{ id: 'ap2', summary: 'rm -rf dist' }] },
      's-fail': { ...baseView, session: failed, approvals: [] },
    };
    fake.handler = request => request.method === 'get_session' ? applied({ kind: 'session', data: { session: views[request.params.sessionId], eventStreamId: 'boot-1', sequence: seq } }) :
      request.method === 'respond_to_approval' ? (request.params.decision === 'deny' ? deny : { status: 'dispatched', data: { operationId: 'op-a' } }) : undefined;
    const event = (value: Parameters<FakeTransport['event']>[1]) => act(async () => fake.event(++seq, value));
    await render(fake, [waiting, failed, running]);
    await flush();
    // A fresh snapshot carries no approvals: only the waiting session is read.
    expect(fake.ops('get_session').map(item => item.request)).toEqual([{ method: 'get_session', params: { sessionId: 's-wait' } }]);
    expect(inbox().textContent).toBe('待处理2');
    expect(inbox().querySelector('.needs-you-count')?.getAttribute('aria-label')).toBe('2 项');
    await event({ kind: 'approval_required', data: { sessionId: 's-run', approval: { id: 'ap2', summary: 'rm -rf dist' } } });
    await act(async () => { inbox().click(); });
    expect([...panel()!.querySelectorAll('h2')].map(item => item.textContent)).toEqual(['等你审批', '失败的会话']);
    expect([...panel()!.querySelectorAll('li')].map(item => item.textContent)).toEqual([
      '等审批npm install允许一次拒绝', '在跑rm -rf dist允许一次拒绝', '失败了Pi 自己开始了一轮运行，这个会话已停止。可以继续这个会话。',
    ]);
    expect(panel()!.querySelector('a')?.getAttribute('href')).toBe('#/sessions/s-wait');

    await act(async () => { inPanel('允许一次')[0].click(); });
    await flush();
    expect(fake.ops('respond_to_approval')[0].request).toEqual({ method: 'respond_to_approval', params: { sessionId: 's-wait', expectedStreamId: 'stream-w', expectedCwdFence: base.cwdFence, approvalId: 'ap1', decision: 'allow_once' } });
    expect(panel()!.querySelector('[data-outcome="sent"]')).not.toBeNull();
    await event({ kind: 'approval_resolved', data: { sessionId: 's-wait', approvalId: 'ap1', decision: 'allow_once' } });
    await event({ kind: 'session_changed', data: { session: { ...waiting, state: { state: 'running' } } } });
    expect(inbox().textContent).toBe('待处理2');

    // An unknown answer for a session that is not open is listed until that session is opened.
    await act(async () => { inPanel('拒绝')[0].click(); });
    await flush();
    expect([...panel()!.querySelectorAll('h2')].map(item => item.textContent)).toEqual(['等你审批', '失败的会话', '结果未知']);
    expect(panel()!.querySelector('[aria-label="结果未知"] li')?.textContent).toBe('在跑回复审批：没能确认结果，请打开会话核对。');
    expect(inbox().textContent).toBe('待处理3');
    await act(async () => { panel()!.querySelector<HTMLAnchorElement>('[aria-label="结果未知"] a')!.click(); });
    // jsdom announces fragment navigation in a later task.
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
    await flush();
    expect(window.location.hash).toBe('#/sessions/s-run');
    expect(panel()).toBeNull();
    expect(inbox().textContent).toBe('待处理2');
    // On the open session its own page shows the outcome, so the inbox does not repeat it.
    await act(async () => { inbox().click(); });
    await act(async () => { inPanel('拒绝')[0].click(); });
    await flush();
    expect(client!.getSnapshot().unknown).toEqual({});

    // A failure leaves once its session has been opened here, also after a restart.
    await act(async () => { window.location.hash = '#/sessions/s-fail'; window.dispatchEvent(new HashChangeEvent('hashchange')); });
    await flush();
    expect(JSON.parse(localStorage.getItem('cafe-space.needs-you.seen-failures')!)).toEqual(['s-fail']);
    expect(inbox().textContent).toBe('待处理1');
    deny = { status: 'dispatched', data: { operationId: 'op-d' } };
    await event({ kind: 'session_changed', data: { session: { ...running, state: { state: 'ready' } } } });
    expect(inbox().textContent).toBe('待处理');
  });
});

describe('error wording', () => {
  it('gives every daemon and adapter code a sentence without hiding the code', async () => {
    const { errorMessage } = await import('../src/app/copy');
    expect(errorMessage('PI_APPROVAL_SCOPE_UNSUPPORTED')).toContain('本会话内允许');
    expect(errorMessage('PI_COMMAND_OUTCOME_UNKNOWN')).toBe('无法确认这个操作是否生效。');
    expect(errorMessage('PI_NODE_UNAVAILABLE')).toBe('找不到可运行的 Node 程序。');
    expect(errorMessage('PI_PACKAGE_SOMETHING_NEW')).toBe('没有找到可用的 Pi 安装。');
    expect(errorMessage('PI_RECORD_INVALID')).toBe('Pi 返回了无法识别的数据。');
    expect(errorMessage('PI_SOMETHING_NEW')).toBe('Pi 出现了问题。');
    expect(errorMessage('UNKNOWN_CODE')).toBe('操作未完成。');
  });
});
