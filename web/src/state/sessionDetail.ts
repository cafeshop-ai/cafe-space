import type { Event } from '../protocol/generated/Event';
import type { SessionView } from '../protocol/generated/SessionView';

/** Client-side bounds match the daemon's protocol limits, so a long session cannot grow memory without limit. */
export const MAX_MESSAGES = 1000;
export const MAX_APPROVALS = 64;

/** One open session: the `get_session` view plus the event sequence it already reflects. */
export type SessionDetail = { view: SessionView; eventStreamId: string; sequence: number };

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
  switch (event.kind) {
    case 'session_changed':
      if (event.data.session.id === id) view = { ...view, session: event.data.session };
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
      if (event.data.sessionId === id) view = { ...view, approvals: view.approvals.filter(item => item.id !== event.data.approvalId) };
      break;
    default:
      break;
  }
  return { view, eventStreamId, sequence };
}
