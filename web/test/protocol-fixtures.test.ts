import { describe, expect, it } from 'vitest';
import { closeSessionRequest, openSessionRequest, sessionLifecycle, sessionTransitions, migrationFailedHealthResponse, schemaMigrationFailedResponse, runtimesResponse, sessionToolApprovalEvent, ownerEnvironmentRequests, ownerEnvironmentResponses, modelListRequests, modelListResponses, piCommandRequests, piCommandResponses, piApprovalRequests, piApprovalResponses, piApprovalEvents, runtimeRegistrationRequests, runtimeRegistrationResponses } from '../src/protocol-fixtures.generated';
import type { ServerMessage } from '../src/protocol';

describe('M1 session lifecycle wire fixtures', () => {
  it('keeps tool approval explicit without promising that every tool asks', () => {
    expect(runtimesResponse.protocolVersion).toBe(9);
    expect(runtimesResponse.result.data.result.data.items.map(item => item.toolApproval)).toEqual(['never_asks', 'may_ask', 'unknown']);
    expect(sessionToolApprovalEvent.protocolVersion).toBe(9);
    const capabilities = sessionToolApprovalEvent.event.data.session.capabilities;
    expect(capabilities.toolApproval).toBe('never_asks');
    expect(capabilities.approvalForSession).toBe(false);
  });
  it('carries the owner environment requests and their three outcomes at v5', () => {
    expect(ownerEnvironmentRequests.map(item => item.request.method)).toEqual(['get_owner_environment', 'update_owner_environment', 'update_owner_environment', 'update_owner_environment']);
    expect(ownerEnvironmentResponses.map(item => item.result.status)).toEqual(['applied', 'rejected', 'unknown']);
    expect([...ownerEnvironmentRequests, ...ownerEnvironmentResponses].every(item => item.protocolVersion === 9)).toBe(true);
  });
  it('shares fenced model queries, empty lists and truthful outcomes at v5', () => {
    expect(modelListRequests[0].request).toEqual({ method: 'list_models', params: { sessionId: 'synthetic-session', expectedStreamId: 'synthetic-stream', expectedCwdFence: 'synthetic-cwd' } });
    expect(modelListResponses.map(frame => frame.result.status)).toEqual(['applied', 'applied', 'rejected', 'rejected', 'unknown']);
    expect(modelListResponses[0].result.data.result.data.items).toEqual([{ provider: 'synthetic', id: 'tea' }, { provider: 'synthetic', id: 'coffee' }]);
    expect(modelListResponses[1].result.data.result.data.items).toEqual([]);
    expect([...modelListRequests, ...modelListResponses].every(frame => frame.protocolVersion === 9)).toBe(true);
  });
  it('shares prompt/cancel shapes without treating acceptance as completion', () => {
    expect(piCommandRequests.map(frame => frame.request.method)).toEqual(['prompt', 'prompt', 'cancel']);
    expect(piCommandRequests[0].request.params.delivery).toBe('steer');
    expect(piCommandRequests[1].request.params.delivery).toBe('follow_up');
    expect(piCommandRequests[0].request.params.content).toBe('Synthetic tea 茶\u2028line\u2029end');
    expect(piCommandRequests[2].request.params.runId).toBe('synthetic-run');
    expect(piCommandResponses.map(frame => frame.result.status)).toEqual(['dispatched', 'rejected', 'unknown', 'applied']);
    expect(piCommandResponses[0].result.data).toEqual({ operationId: 'synthetic-operation' });
    expect(piCommandResponses[2].result.data.error.code).toBe('PI_CANCEL_OUTCOME_UNKNOWN');
    expect(piCommandResponses[3].result.data.result).toEqual({ kind: 'empty' });
    expect([...piCommandRequests, ...piCommandResponses].every(frame => frame.protocolVersion === 9)).toBe(true);
  });
  it('shares explicit confirmation decisions and does not call a selection applied', () => {
    expect(piApprovalRequests.map(frame => frame.request.params.decision)).toEqual(['allow_once', 'deny', 'allow_for_session']);
    expect(piApprovalResponses.map(frame => frame.result.status)).toEqual(['dispatched', 'rejected', 'unknown']);
    expect(piApprovalResponses[1].result.data.error.code).toBe('PI_APPROVAL_SCOPE_UNSUPPORTED');
    expect(piApprovalEvents[0].event.data.approval.id).toBe(piApprovalEvents[1].event.data.approvalId);
    expect(piApprovalEvents[1].event.data.decision).toBe('allow_once');
    expect([...piApprovalRequests, ...piApprovalResponses, ...piApprovalEvents].every(frame => frame.protocolVersion === 9)).toBe(true);
  });
  it('returns local-only Pi candidates, including an empty result', () => {
    expect(runtimeRegistrationRequests.at(-1)?.request).toEqual({ method: 'discover_pi_runtime' });
    const discoveries = runtimeRegistrationResponses.filter(frame => frame.requestId === 'runtime-discovery');
    expect(discoveries.map(frame => frame.result.status)).toEqual(['applied', 'applied', 'rejected']);
    for (const [index, expected] of [
      { node: '/synthetic-private/node', packageRoot: '/synthetic-private/pi' },
      { node: null, packageRoot: null },
    ].entries()) {
      const result = discoveries[index].result;
      if (result.status !== 'applied' || result.data.result.kind !== 'pi_runtime_discovery') throw Error('candidate fixture must be applied');
      expect(result.data.result.data).toEqual(expected);
    }
    expect(discoveries[2].result.status === 'rejected' && discoveries[2].result.data.error.code).toBe('LOCAL_ONLY');
    expect([...runtimeRegistrationRequests, ...runtimeRegistrationResponses].every(frame => frame.protocolVersion === 9)).toBe(true);
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
