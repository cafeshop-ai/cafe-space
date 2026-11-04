import { describe, expect, it } from 'vitest';
import { closeSessionRequest, openSessionRequest, sessionLifecycle, sessionTransitions, migrationFailedHealthResponse, schemaMigrationFailedResponse, runtimesResponse, sessionToolApprovalEvent } from '../src/protocol-fixtures.generated';
import type { ServerMessage } from '../src/protocol';

describe('M1 session lifecycle wire fixtures', () => {
  it('keeps tool approval explicit without promising that every tool asks', () => {
    expect(runtimesResponse.protocolVersion).toBe(3);
    expect(runtimesResponse.result.data.result.data.items.map(item => item.toolApproval)).toEqual(['never_asks', 'may_ask', 'unknown']);
    expect(sessionToolApprovalEvent.protocolVersion).toBe(3);
    const capabilities = sessionToolApprovalEvent.event.data.session.capabilities;
    expect(capabilities.toolApproval).toBe('never_asks');
    expect(capabilities.approvalForSession).toBe(false);
  });
  it('keeps migration failure observable without confirming handover readiness', () => {
    const health = migrationFailedHealthResponse.result.data.result.data;
    expect(health.ready).toBe(false);
    expect(health).not.toHaveProperty('awaitingUpdateConfirmation');
    expect(health.update).toBe('draining');
    expect(schemaMigrationFailedResponse.result.status).toBe('rejected');
    expect(schemaMigrationFailedResponse.result.data.error.code).toBe('SCHEMA_MIGRATION_FAILED');
  });
  it('covers observed states, fenced open/close and fixed stopped reasons', () => {
    expect(openSessionRequest.request.method).toBe('open_session');
    expect(closeSessionRequest.request.method).toBe('close_session');
    expect(openSessionRequest.request.params).toEqual(closeSessionRequest.request.params);
    const frames: readonly ServerMessage[] = sessionLifecycle;
    expect(frames).toHaveLength(15);
    expect(Object.keys(sessionTransitions)).toHaveLength(7);
    expect(sessionTransitions.stopped).toEqual([]);
    expect(sessionTransitions.failed).toEqual([]);
    const previousState = new Map<string, keyof typeof sessionTransitions>();
    const sequence = new Map<string, number>();
    const reasons: string[] = [];
    const states: string[] = [];
    for (const frame of frames) {
      if (frame.type !== 'event' || frame.event.kind !== 'session_changed') throw Error('fixture must contain session events');
      expect(frame.sequence).toBe((sequence.get(frame.eventStreamId) ?? 0) + 1);
      sequence.set(frame.eventStreamId, frame.sequence);
      const session = frame.event.data.session;
      const previous = previousState.get(session.id);
      if (previous) expect(sessionTransitions[previous]).toContain(session.state.state);
      previousState.set(session.id, session.state.state);
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
