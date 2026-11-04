import { describe, expect, it } from 'vitest';
import { closeSessionRequest, openSessionRequest, sessionLifecycle } from '../src/protocol-fixtures.generated';
import type { ServerMessage } from '../src/protocol';

describe('M1 session lifecycle wire fixtures', () => {
  it('covers observed states, fenced open/close and fixed stopped reasons', () => {
    expect(openSessionRequest.request.method).toBe('open_session');
    expect(closeSessionRequest.request.method).toBe('close_session');
    expect(openSessionRequest.request.params).toEqual(closeSessionRequest.request.params);
    const frames: readonly ServerMessage[] = sessionLifecycle;
    expect(frames).toHaveLength(15);
    const sequence = new Map<string, number>();
    const reasons: string[] = [];
    const states: string[] = [];
    for (const frame of frames) {
      if (frame.type !== 'event' || frame.event.kind !== 'session_changed') throw Error('fixture must contain session events');
      expect(frame.sequence).toBe((sequence.get(frame.eventStreamId) ?? 0) + 1);
      sequence.set(frame.eventStreamId, frame.sequence);
      const session = frame.event.data.session;
      states.push(session.state.state);
      if (session.stopReason) {
        expect(session.state.state).toBe('stopped');
        expect(session.activeRunId).toBeUndefined();
        reasons.push(session.stopReason);
      }
      expect(JSON.parse(JSON.stringify(frame))).toEqual(frame);
    }
    expect(states.slice(0, 6)).toEqual(['starting', 'ready', 'running', 'ready', 'stopping', 'stopped']);
    expect(reasons).toEqual(['SESSION_CLOSED', 'DAEMON_RESTARTED', 'AGENT_EXITED']);
  });
});
