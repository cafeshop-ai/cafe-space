import { useEffect, useId, useRef, useState } from 'react';
import type { ApprovalDecision } from '../protocol/generated/ApprovalDecision';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { DaemonClient, DaemonView, SessionWrite } from '../state/DaemonClient';
import { errorMessage, outcomeOf, type Outcome } from '../app/copy';
import { sessionHref, sessionTitle } from './SessionList';
import { OutcomeMark } from './StatePill';

const SEEN_KEY = 'cafe-space.needs-you.seen-failures';
const MAX_SEEN = 256;
const writeLabels: Record<SessionWrite, string> = {
  prompt: '发送消息', cancel: '停止', close_session: '关闭会话', respond_to_approval: '回复审批', set_model: '切换模型', set_thinking: '调整思考强度',
};
function readSeen(): string[] {
  try { const value: unknown = JSON.parse(localStorage.getItem(SEEN_KEY) ?? '[]'); return Array.isArray(value) ? value.filter(item => typeof item === 'string') : []; } catch { return []; }
}

/**
 * UX §6.4, local part: approvals, failed sessions and unknown outcomes, by urgency. Items leave on their own once
 * resolved; a failure leaves once its session has been opened here. The count is the only badge in the app.
 */
export function NeedsYou({ client, state, openId }: { client: DaemonClient; state: DaemonView; openId: string | null }) {
  const [open, setOpen] = useState(false);
  const [seen, setSeen] = useState(readSeen);
  const [answers, setAnswers] = useState<Record<string, Outcome>>({});
  const root = useRef<HTMLSpanElement>(null);
  const panelId = useId();
  const sessions = state.sessions ?? [];
  const connected = state.connection === 'connected';
  const failedOpen = sessions.some(item => item.id === openId && item.state.state === 'failed');
  useEffect(() => {
    if (!openId || !failedOpen || seen.includes(openId)) return;
    const next = [...seen, openId].slice(-MAX_SEEN);
    setSeen(next);
    try { localStorage.setItem(SEEN_KEY, JSON.stringify(next)); } catch { /* the item then shows again after a restart */ }
  }, [openId, failedOpen, seen]);
  useEffect(() => {
    if (!open) return;
    const away = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener('pointerdown', away);
    return () => document.removeEventListener('pointerdown', away);
  }, [open]);

  const approvals = sessions.flatMap(session => (state.approvals[session.id] ?? []).map(approval => ({ session, approval })));
  // Waiting at the computer with no request known here yet, e.g. right after reconnecting.
  const waiting = sessions.filter(session => session.state.state === 'waiting_for_local_ui' && !state.approvals[session.id]?.length);
  const failed = sessions.filter(session => session.state.state === 'failed' && !seen.includes(session.id));
  const unknown = sessions.filter(session => state.unknown[session.id]);
  const count = approvals.length + waiting.length + failed.length + unknown.length;

  async function answer(session: SessionSnapshot, approvalId: string, decision: ApprovalDecision) {
    const fence = { sessionId: session.id, expectedStreamId: session.streamId ?? '', expectedCwdFence: session.cwdFence };
    const outcome = outcomeOf(await client.call({ method: 'respond_to_approval', params: { ...fence, approvalId, decision } }));
    setAnswers(current => ({ ...current, [approvalId]: outcome }));
  }
  const link = (session: SessionSnapshot) => <a href={sessionHref(session.id)} onClick={() => setOpen(false)}>{sessionTitle(session, state.runtimes)}</a>;

  return <span ref={root} className="needs-you-control">
    <button type="button" className="quiet-button needs-you" aria-expanded={open} aria-controls={open ? panelId : undefined} onClick={() => setOpen(!open)}
      onKeyDown={event => { if (event.key === 'Escape' && open) { event.preventDefault(); setOpen(false); } }}>
      待处理{count > 0 && <span className="needs-you-count" aria-label={`${count} 项`}>{count}</span>}
    </button>
    {open && <div id={panelId} className="needs-you-panel" role="dialog" aria-label="待处理"
      onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); setOpen(false); root.current?.querySelector('button')?.focus(); } }}>
      {count === 0 ? <p className="needs-you-empty">现在没有要你处理的事</p> : <>
        {approvals.length + waiting.length > 0 && <section aria-label="等你审批">
          <h2>等你审批</h2>
          <ul>
            {approvals.map(({ session, approval }) => <li key={approval.id}>
              {link(session)}
              <p className="needs-you-summary">{approval.summary}</p>
              <div className="needs-you-actions">
                <button type="button" className="secondary-button" disabled={!connected} onClick={() => void answer(session, approval.id, 'allow_once')}>允许一次</button>
                <button type="button" className="quiet-button" disabled={!connected} onClick={() => void answer(session, approval.id, 'deny')}>拒绝</button>
                <OutcomeMark outcome={answers[approval.id] ?? null} />
              </div>
            </li>)}
            {waiting.map(session => <li key={session.id}>{link(session)}<p className="needs-you-summary">在会话里等你处理。</p></li>)}
          </ul>
        </section>}
        {failed.length > 0 && <section aria-label="失败的会话">
          <h2>失败的会话</h2>
          <ul>{failed.map(session => <li key={session.id}>{link(session)}
            <p className="needs-you-summary">{session.state.state === 'failed' && errorMessage(session.state.data.code)}</p></li>)}</ul>
        </section>}
        {unknown.length > 0 && <section aria-label="结果未知">
          <h2>结果未知</h2>
          <ul>{unknown.map(session => <li key={session.id}>{link(session)}
            <p className="needs-you-summary">{writeLabels[state.unknown[session.id].method]}：没能确认结果，请打开会话核对。</p></li>)}</ul>
        </section>}
      </>}
    </div>}
  </span>;
}
