import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import { stateLabels, stopReasonLabels, errorMessage, outcomeLabels, type Outcome } from '../app/copy';

export function StatePill({ session }: { session: SessionSnapshot }) {
  const state = session.state;
  const label = state.state === 'failed' ? `${stateLabels.failed} · ${errorMessage(state.data.code)}` : stateLabels[state.state];
  const detail = state.state === 'stopped' && session.stopReason ? stopReasonLabels[session.stopReason] : undefined;
  return <span className="state-pill" data-state={state.state} title={detail}>
    {state.state === 'running' && <span className="steam" aria-hidden="true" />}
    {label}{detail && <span className="visually-hidden">，{detail}</span>}
  </span>;
}

/** One command's outcome, worded the same everywhere (UX §6.1). */
export function OutcomeMark({ outcome }: { outcome: Outcome | null }) {
  if (!outcome) return null;
  return <span className="outcome-mark" data-outcome={outcome.kind} role="status">
    <span className="outcome-icon" aria-hidden="true">{outcome.kind === 'sent' ? '◷' : outcome.kind === 'done' ? '✓' : outcome.kind === 'unknown' ? '?' : '×'}</span>
    {outcomeLabels[outcome.kind]}{outcome.kind === 'not_done' && outcome.code && ` · ${errorMessage(outcome.code)}`}
    {outcome.kind === 'unknown' && outcome.code && <span className="outcome-reason"> {errorMessage(outcome.code)}</span>}
    {outcome.code && <details className="raw-code"><summary>详情</summary><code>{outcome.code}</code></details>}
  </span>;
}
