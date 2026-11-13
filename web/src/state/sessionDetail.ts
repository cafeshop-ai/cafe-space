import type { ApprovalRequest } from '../protocol/generated/ApprovalRequest';
import type { Event } from '../protocol/generated/Event';
import type { SessionState } from '../protocol/generated/SessionState';
import type { SessionView } from '../protocol/generated/SessionView';

/** Client-side bounds match the daemon's protocol limits, so a long session cannot grow memory without limit. */
export const MAX_MESSAGES = 1000;
export const MAX_APPROVALS = 64;

/**
 * One open session: the `get_session` view plus the event sequence it already reflects.
 * `expired` holds approvals the agent closed without a choice; it exists only in this client.
 */
export type SessionDetail = { view: SessionView; eventStreamId: string; sequence: number; expired?: ApprovalRequest[] };

/** No confirmation dialog survives these states: the daemon closes it without allowing or denying. */
export const closesApprovals = (state: SessionState) =>
  state.state === 'ready' || state.state === 'stopping' || state.state === 'stopped' || state.state === 'failed';

/** Pending approvals of every session (UX §6.4), kept from the same ordered events as the open view. */
export type PendingApprovals = Record<string, ApprovalRequest[]>;
export function applyApprovalEvent(pending: PendingApprovals, event: Event): PendingApprovals {
  const without = (id: string) => { const { [id]: _, ...rest } = pending; return rest; };
  switch (event.kind) {
    case 'approval_required': {
      const list = pending[event.data.sessionId] ?? [];
      if (list.some(item => item.id === event.data.approval.id)) return pending;
      return { ...pending, [event.data.sessionId]: [...list, event.data.approval].slice(-MAX_APPROVALS) };
    }
    case 'approval_resolved': {
      const list = pending[event.data.sessionId];
      return list ? { ...pending, [event.data.sessionId]: list.filter(item => item.id !== event.data.approvalId) } : pending;
    }
    case 'session_changed':
      return pending[event.data.session.id] && closesApprovals(event.data.session.state) ? without(event.data.session.id) : pending;
    case 'session_removed':
      return pending[event.data.sessionId] ? without(event.data.sessionId) : pending;
    default:
      return pending;
  }
}

/**
 * Applies one ordered event to an open session view.
 * - `stale`: the view can no longer be trusted (another event stream); fetch it again.
 * - `removed`: the session no longer exists.
 * - otherwise the (possibly unchanged) detail, with its sequence advanced.
 * Events at or below the view's sequence are already reflected in it and are ignored.
 */
export function applySessionEvent(detail: SessionDetail, eventStreamId: string, sequence: number, event: Event): SessionDetail | 'stale' | 'removed' {
  if (eventStreamId !== detail.eventStreamId) return 'stale';
  if (sequence <= detail.sequence) return detail;
  const id = detail.view.session.id;
  let view = detail.view;
  let expired = detail.expired;
  switch (event.kind) {
    case 'session_changed':
      if (event.data.session.id !== id) break;
      view = { ...view, session: event.data.session };
      // UX §6.3: an unanswered request never disappears silently; it is shown as expired, never as approved.
      if (closesApprovals(event.data.session.state) && view.approvals.length) {
        expired = [...(expired ?? []), ...view.approvals].slice(-MAX_APPROVALS);
        view = { ...view, approvals: [] };
      }
      break;
    case 'session_removed':
      if (event.data.sessionId === id) return 'removed';
      break;
    case 'message_changed': {
      if (event.data.sessionId !== id) break;
      const message = event.data.message;
      const index = view.messages.findIndex(item => item.id === message.id);
      let messages = index >= 0 ? view.messages.map((item, i) => i === index ? message : item) : [...view.messages, message];
      let historyTruncated = view.historyTruncated;
      if (messages.length > MAX_MESSAGES) { messages = messages.slice(messages.length - MAX_MESSAGES); historyTruncated = true; }
      view = { ...view, messages, historyTruncated };
      break;
    }
    case 'approval_required':
      if (event.data.sessionId !== id || view.approvals.some(item => item.id === event.data.approval.id)) break;
      // More than the protocol allows means the view is inconsistent; fetch it again instead of dropping a request.
      if (view.approvals.length >= MAX_APPROVALS) return 'stale';
      view = { ...view, approvals: [...view.approvals, event.data.approval] };
      break;
    case 'approval_resolved':
      if (event.data.sessionId !== id) break;
      view = { ...view, approvals: view.approvals.filter(item => item.id !== event.data.approvalId) };
      if (expired) expired = expired.filter(item => item.id !== event.data.approvalId);
      break;
    default:
      break;
  }
  return expired === undefined ? { view, eventStreamId, sequence } : { view, eventStreamId, sequence, expired };
}
