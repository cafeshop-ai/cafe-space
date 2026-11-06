import { describe, expect, it } from 'vitest';
import { runHostProbe } from '../src/probe/host';
import { DaemonClient } from '../src/state/DaemonClient';
import type { Transport } from '../src/services/transport';
import type { ClientRequest, Event, RequestResult, Response, ServerMessage, SessionSnapshot, TranscriptMessage } from '../src/protocol';
import { sessionResponse, desktopProbeSessionId, desktopProbeExpectedText } from '../src/protocol-fixtures.generated';

class Peer implements Transport {
  sent: ClientRequest[] = [];
  onMessage: (frame: ServerMessage) => void = () => {};
  onClose = () => {};
  sequence = 0;
  messages: TranscriptMessage[] = [];
  session: SessionSnapshot = {
    ...sessionResponse.result.data.result.data.session.session, id: desktopProbeSessionId,
    capabilities: { ...sessionResponse.result.data.result.data.session.session.capabilities, approvalForSession: false, toolApproval: 'never_asks' },
  };
  constructor(private order: 'before' | 'after' | 'unknown' | 'bad-readback' | 'busy-readback') {}
  async open(onMessage: (frame: ServerMessage) => void, onClose: () => void) {
    this.onMessage = onMessage; this.onClose = onClose;
    queueMicrotask(() => onMessage({ type: 'welcome', protocolVersion: 5, daemonVersion: '0.1.0' }));
  }
  async close() { this.onClose(); }
  event(event: Event) { this.onMessage({ type: 'event', protocolVersion: 5, eventStreamId: 'probe', sequence: ++this.sequence, event }); }
  async send(frame: ClientRequest) { this.sent.push(frame); queueMicrotask(() => this.respond(frame)); }
  respond(frame: ClientRequest) {
    let response: Response;
    const reply = (result: RequestResult) => this.onMessage({ type: 'response', protocolVersion: 5, requestId: frame.requestId, result });
    const observed = () => {
      this.session = { ...this.session, state: { state: 'running' }, activeRunId: 'synthetic-run' };
      this.event({ kind: 'session_changed', data: { session: this.session } });
      this.messages = [{ id: 'synthetic-message', role: 'assistant', status: 'complete', parts: [{ kind: 'text', data: { text: desktopProbeExpectedText } }] }];
      this.event({ kind: 'message_changed', data: { sessionId: this.session.id, message: this.messages[0] } });
      this.session = { ...this.session, state: { state: 'ready' }, activeRunId: undefined };
      this.event({ kind: 'session_changed', data: { session: this.session } });
    };
    switch (frame.request.method) {
      case 'get_status': response = { kind: 'health', data: { ready: true, daemonVersion: '0.1.0', update: 'none' } }; break;
      case 'list_sessions': response = { kind: 'sessions', data: { items: [this.session], eventStreamId: 'probe', sequence: this.sequence } }; break;
      case 'subscribe_events': response = { kind: 'subscription', data: { eventStreamId: 'probe', sequence: this.sequence, mode: 'snapshot_required', snapshot: { update: 'none', projects: [], runtimes: [], sessions: [this.session] } } }; break;
      case 'get_session': response = { kind: 'session', data: { eventStreamId: 'probe', sequence: this.sequence, session: {
        session: this.order === 'busy-readback' && this.messages.length ? { ...this.session, state: { state: 'running' }, activeRunId: 'unfinished' } : this.session,
        messages: this.order === 'bad-readback' && this.messages.length ? [] : this.messages, historyTruncated: false, approvals: [],
      } } }; break;
      case 'prompt':
        if (this.order === 'unknown') { reply({ status: 'unknown', data: { error: { code: 'PI_COMMAND_OUTCOME_UNKNOWN', message: '' } } }); return; }
        if (this.order !== 'after') observed();
        reply({ status: 'dispatched', data: { operationId: 'synthetic-operation' } });
        if (this.order === 'after') queueMicrotask(observed);
        return;
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
      expect(peer.sent.map(frame => frame.request.method)).toEqual(['get_status', 'list_sessions', 'subscribe_events', 'get_session', 'prompt', 'get_session', 'close_session', 'get_session']);
      expect(client.getSnapshot().session?.detail?.view.session.state.state).toBe('stopped');
      expect(client.getSnapshot().connection).toBe('connected');
    } finally { client.disconnect(); }
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
