import { describe, expect, it, vi } from 'vitest';
import { runHostProbe } from '../src/probe/host';
import { DaemonClient } from '../src/state/DaemonClient';
import type { Transport } from '../src/services/transport';
import type { ApprovalRequest } from '../src/protocol/generated/ApprovalRequest';
import type { ClientRequest, Event, RequestResult, Response, ServerMessage, SessionSnapshot, TranscriptMessage } from '../src/protocol';
import { sessionResponse, desktopProbeSessionId, desktopProbeCancelPrompt, desktopProbeConfirmationPrompt, desktopProbeConfirmationSummary, desktopProbeExpectedText } from '../src/protocol-fixtures.generated';

class Peer implements Transport {
  sent: ClientRequest[] = [];
  onMessage: (frame: ServerMessage) => void = () => {};
  onClose = () => {};
  sequence = 0;
  messages: TranscriptMessage[] = [];
  cancelled = false;
  approvals: ApprovalRequest[] = [];
  confirmationCount = 0;
  selected = false;
  session: SessionSnapshot = {
    ...sessionResponse.result.data.result.data.session.session, id: desktopProbeSessionId,
    capabilities: { ...sessionResponse.result.data.result.data.session.session.capabilities, approvalForSession: false, toolApproval: 'never_asks' },
  };
  constructor(private order: 'before' | 'after' | 'unknown' | 'bad-readback' | 'busy-readback' | 'cancel-unknown' | 'cancel-busy-readback' | 'wrong-run-readback' | 'cancel-no-settle' | 'confirmation-unknown' | 'confirmation-bad-readback' | 'confirmation-stale-readback' | 'confirmation-no-settle' | 'confirmation-no-resolution') {}
  async open(onMessage: (frame: ServerMessage) => void, onClose: () => void) {
    this.onMessage = onMessage; this.onClose = onClose;
    queueMicrotask(() => onMessage({ type: 'welcome', protocolVersion: 5, daemonVersion: '0.1.0' }));
  }
  async close() { this.onClose(); }
  event(event: Event) { this.onMessage({ type: 'event', protocolVersion: 5, eventStreamId: 'probe', sequence: ++this.sequence, event }); }
  async send(frame: ClientRequest) { this.sent.push(frame); queueMicrotask(() => this.respond(frame)); }
  running(runId: string) {
    this.session = { ...this.session, state: { state: 'running' }, activeRunId: runId };
    this.event({ kind: 'session_changed', data: { session: this.session } });
  }
  ready() {
    this.session = { ...this.session, state: { state: 'ready' }, activeRunId: undefined };
    this.event({ kind: 'session_changed', data: { session: this.session } });
  }
  confirmation() {
    this.confirmationCount++;
    this.selected = false;
    this.running(`confirmation-${this.confirmationCount}`);
    this.approvals = [{ id: String(this.confirmationCount).padStart(32, '0'), summary: desktopProbeConfirmationSummary }];
    this.event({ kind: 'approval_required', data: { sessionId: this.session.id, approval: this.approvals[0] } });
  }
  respond(frame: ClientRequest) {
    let response: Response;
    const reply = (result: RequestResult) => this.onMessage({ type: 'response', protocolVersion: 5, requestId: frame.requestId, result });
    const observed = () => {
      this.running('synthetic-run');
      this.messages = [{ id: 'synthetic-message', role: 'assistant', status: 'complete', parts: [{ kind: 'text', data: { text: desktopProbeExpectedText } }] }];
      this.event({ kind: 'message_changed', data: { sessionId: this.session.id, message: this.messages[0] } });
      this.ready();
    };
    switch (frame.request.method) {
      case 'get_status': response = { kind: 'health', data: { ready: true, daemonVersion: '0.1.0', update: 'none' } }; break;
      case 'list_sessions': response = { kind: 'sessions', data: { items: [this.session], eventStreamId: 'probe', sequence: this.sequence } }; break;
      case 'subscribe_events': response = { kind: 'subscription', data: { eventStreamId: 'probe', sequence: this.sequence, mode: 'snapshot_required', snapshot: { update: 'none', projects: [], runtimes: [], sessions: [this.session] } } }; break;
      case 'get_session': {
        let session = this.session;
        if ((this.order === 'busy-readback' && this.messages.length) || (this.order === 'cancel-busy-readback' && this.cancelled)) {
          session = { ...session, state: { state: 'running' }, activeRunId: 'unfinished' };
        } else if (this.order === 'wrong-run-readback' && session.state.state === 'running') {
          session = { ...session, activeRunId: 'another-run' };
        }
        response = { kind: 'session', data: { eventStreamId: 'probe', sequence: this.sequence, session: {
          session, messages: this.order === 'bad-readback' && this.messages.length ? [] : this.messages, historyTruncated: false,
          approvals: this.order === 'confirmation-bad-readback' && this.approvals.length ? [{ ...this.approvals[0], id: 'another-dialog' }]
            : this.order === 'confirmation-stale-readback' && this.selected ? [{ id: 'still-pending', summary: desktopProbeConfirmationSummary }] : this.approvals,
        } } }; break;
      }
      case 'prompt':
        if (this.order === 'unknown') { reply({ status: 'unknown', data: { error: { code: 'PI_COMMAND_OUTCOME_UNKNOWN', message: '' } } }); return; }
        if (frame.request.params.content === desktopProbeConfirmationPrompt) {
          if (this.order !== 'after') this.confirmation();
          reply({ status: 'dispatched', data: { operationId: 'synthetic-confirmation' } });
          if (this.order === 'after') queueMicrotask(() => this.confirmation());
          return;
        }
        if (frame.request.params.content === desktopProbeCancelPrompt) {
          if (this.order !== 'after') this.running('synthetic-cancel-run');
          reply({ status: 'dispatched', data: { operationId: 'synthetic-busy' } });
          if (this.order === 'after') queueMicrotask(() => this.running('synthetic-cancel-run'));
          return;
        }
        if (this.order !== 'after') observed();
        reply({ status: 'dispatched', data: { operationId: 'synthetic-operation' } });
        if (this.order === 'after') queueMicrotask(observed);
        return;
      case 'cancel':
        if (this.order === 'cancel-unknown') { reply({ status: 'unknown', data: { error: { code: 'PI_COMMAND_OUTCOME_UNKNOWN', message: '' } } }); return; }
        this.cancelled = true;
        if (this.order !== 'after' && this.order !== 'cancel-no-settle') this.ready();
        reply({ status: 'dispatched', data: { operationId: 'synthetic-cancel' } });
        if (this.order === 'after') queueMicrotask(() => this.ready());
        return;
      case 'respond_to_approval': {
        const { approvalId, decision } = frame.request.params;
        const resolved = () => {
          this.approvals = [];
          if (this.order !== 'confirmation-no-resolution') this.event({ kind: 'approval_resolved', data: { sessionId: this.session.id, approvalId, decision } });
          if (this.order !== 'confirmation-no-settle') this.ready();
        };
        this.selected = true;
        if (this.order !== 'after') resolved();
        if (this.order === 'confirmation-unknown') { reply({ status: 'unknown', data: { error: { code: 'PI_COMMAND_OUTCOME_UNKNOWN', message: '' } } }); return; }
        reply({ status: 'dispatched', data: { operationId: 'synthetic-selection' } });
        if (this.order === 'after') queueMicrotask(resolved);
        return;
      }
      case 'close_session':
        this.session = { ...this.session, state: { state: 'stopped' }, stopReason: 'SESSION_CLOSED' };
        this.event({ kind: 'session_changed', data: { session: this.session } });
        reply({ status: 'dispatched', data: { operationId: 'synthetic-close' } }); return;
      default: throw new Error('unexpected probe operation');
    }
    reply({ status: 'applied', data: { result: response } });
  }
}

describe('fake Host WebView probe driver', () => {
  it.each(['before', 'after'] as const)('checks authoritative transcript and stopped readback with events %s dispatch', async order => {
    const peer = new Peer(order), client = new DaemonClient(() => peer);
    try {
      await runHostProbe(client);
      expect(peer.sent.map(frame => frame.request.method)).toEqual(['get_status', 'list_sessions', 'subscribe_events', 'get_session', 'prompt', 'get_session', 'prompt', 'get_session', 'cancel', 'get_session', 'prompt', 'get_session', 'respond_to_approval', 'get_session', 'prompt', 'get_session', 'respond_to_approval', 'get_session', 'close_session', 'get_session']);
      expect(peer.sent.find(frame => frame.request.method === 'cancel')?.request).toEqual({ method: 'cancel', params: { sessionId: desktopProbeSessionId, expectedStreamId: 'stream-fixture', expectedCwdFence: 'cwd-fixture', runId: 'synthetic-cancel-run' } });
      expect(peer.sent.filter(frame => frame.request.method === 'respond_to_approval').map(frame => frame.request)).toEqual(
        ['allow_once', 'deny'].map((decision, index) => ({ method: 'respond_to_approval', params: { sessionId: desktopProbeSessionId, expectedStreamId: 'stream-fixture', expectedCwdFence: 'cwd-fixture', approvalId: String(index + 1).padStart(32, '0'), decision } })),
      );
      expect(client.getSnapshot().session?.detail?.view.session.state.state).toBe('stopped');
      expect(client.getSnapshot().connection).toBe('connected');
    } finally { client.disconnect(); }
  });
  it.each([
    ['cancel', false, 'M1_HOST_PROBE_ASSERTION_FAILED'],
    ['cancel', true, 'M1_HOST_PROBE_DISCONNECTED'],
    ['respond_to_approval', false, 'M1_HOST_PROBE_ASSERTION_FAILED'],
    ['respond_to_approval', true, 'M1_HOST_PROBE_DISCONNECTED'],
  ] as const)('fails closed when %s disconnects (dispatched=%s) without replay or automatic reconnect', async (method, dispatched, error) => {
    vi.useFakeTimers();
    const peer = new Peer('before'), createTransport = vi.fn(() => peer), client = new DaemonClient(createTransport);
    const call = vi.spyOn(client, 'call');
    const respond = peer.respond.bind(peer);
    vi.spyOn(peer, 'respond').mockImplementation(frame => {
      if (frame.request.method !== method) { respond(frame); return; }
      if (dispatched) peer.onMessage({ type: 'response', protocolVersion: 5, requestId: frame.requestId,
        result: { status: 'dispatched', data: { operationId: 'synthetic-disconnect' } } });
      setTimeout(() => peer.onClose(), 1);
    });
    try {
      const failed = expect(runHostProbe(client)).rejects.toThrow(error);
      await vi.advanceTimersByTimeAsync(0);
      expect(client.getSnapshot().connection).toBe('connected');
      expect(client.getSnapshot().session?.detail?.view.session.state.state).toBe('running');
      expect(peer.sent.at(-1)?.request.method).toBe(method);
      await vi.advanceTimersByTimeAsync(1);
      await failed;
      expect(vi.getTimerCount()).toBe(0);
      const outcome = call.mock.results[call.mock.calls.findIndex(([request]) => request.method === method)].value;
      await expect(outcome).resolves.toEqual(dispatched
        ? { status: 'dispatched', data: { operationId: 'synthetic-disconnect' } }
        : { status: 'unknown', data: { error: { code: 'CONNECTION_LOST', message: '' } } });
      const sent = [...peer.sent];
      await vi.advanceTimersByTimeAsync(30001);
      expect(peer.sent).toEqual(sent);
      expect(peer.sent.filter(frame => frame.request.method === method)).toHaveLength(1);
      expect(peer.sent.filter(frame => frame.request.method === 'prompt')).toHaveLength(method === 'cancel' ? 2 : 3);
      expect(peer.sent.filter(frame => frame.request.method === 'close_session')).toHaveLength(0);
      expect(createTransport).toHaveBeenCalledTimes(1);
      expect(client.getSnapshot().connection).toBe('disconnected');
      expect(client.getSnapshot().session?.detail?.view.session.state.state).toBe('running');
      expect(client.getSnapshot().session?.detail?.view.approvals).toHaveLength(method === 'cancel' ? 0 : 1);
    } finally { client.disconnect(); vi.useRealTimers(); vi.restoreAllMocks(); }
  });
  it.each([['confirmation-unknown', 1], ['confirmation-stale-readback', 1], ['confirmation-bad-readback', 0]] as const)('fails on %s without replaying a selection or closing', async (order, selections) => {
    const peer = new Peer(order), client = new DaemonClient(() => peer);
    try {
      await expect(runHostProbe(client)).rejects.toThrow('M1_HOST_PROBE_ASSERTION_FAILED');
      expect(peer.sent.filter(frame => frame.request.method === 'prompt')).toHaveLength(3);
      expect(peer.sent.filter(frame => frame.request.method === 'respond_to_approval')).toHaveLength(selections);
      expect(peer.sent.filter(frame => frame.request.method === 'close_session')).toHaveLength(0);
    } finally { client.disconnect(); }
  });
  it.each(['confirmation-no-settle', 'confirmation-no-resolution'] as const)('fails on %s without replaying a dispatched selection', async order => {
    vi.useFakeTimers();
    const peer = new Peer(order), client = new DaemonClient(() => peer);
    try {
      const failed = expect(runHostProbe(client)).rejects.toThrow('M1_HOST_PROBE_TIMEOUT');
      await vi.advanceTimersByTimeAsync(10001);
      await failed;
      expect(peer.sent.filter(frame => frame.request.method === 'respond_to_approval')).toHaveLength(1);
      expect(peer.sent.filter(frame => frame.request.method === 'close_session')).toHaveLength(0);
    } finally { client.disconnect(); vi.useRealTimers(); }
  });
  it.each([['cancel-unknown', 1], ['cancel-busy-readback', 1], ['wrong-run-readback', 0]] as const)('fails on %s without replaying cancel or claiming close', async (order, cancelCount) => {
    const peer = new Peer(order), client = new DaemonClient(() => peer);
    try {
      await expect(runHostProbe(client)).rejects.toThrow('M1_HOST_PROBE_ASSERTION_FAILED');
      expect(peer.sent.filter(frame => frame.request.method === 'prompt')).toHaveLength(2);
      expect(peer.sent.filter(frame => frame.request.method === 'cancel')).toHaveLength(cancelCount);
      expect(peer.sent.filter(frame => frame.request.method === 'close_session')).toHaveLength(0);
    } finally { client.disconnect(); }
  });
  it('does not treat dispatched cancel as settlement or replay it on timeout', async () => {
    vi.useFakeTimers();
    const peer = new Peer('cancel-no-settle'), client = new DaemonClient(() => peer);
    try {
      const failed = expect(runHostProbe(client)).rejects.toThrow('M1_HOST_PROBE_TIMEOUT');
      await vi.advanceTimersByTimeAsync(10001);
      await failed;
      expect(peer.sent.filter(frame => frame.request.method === 'cancel')).toHaveLength(1);
      expect(peer.sent.filter(frame => frame.request.method === 'close_session')).toHaveLength(0);
      expect(client.getSnapshot().session?.detail?.view.session.state.state).toBe('running');
    } finally { client.disconnect(); vi.useRealTimers(); }
  });
  it.each(['unknown', 'bad-readback', 'busy-readback'] as const)('fails closed on %s without a second prompt or a close success', async order => {
    const peer = new Peer(order), client = new DaemonClient(() => peer);
    try {
      await expect(runHostProbe(client)).rejects.toThrow('M1_HOST_PROBE_ASSERTION_FAILED');
      expect(peer.sent.filter(frame => frame.request.method === 'prompt')).toHaveLength(1);
      expect(peer.sent.filter(frame => frame.request.method === 'close_session')).toHaveLength(0);
    } finally { client.disconnect(); }
  });
});
