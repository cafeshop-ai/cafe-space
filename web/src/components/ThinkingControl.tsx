import { useState } from 'react';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { ThinkingLevel } from '../protocol/generated/ThinkingLevel';
import type { DaemonClient } from '../state/DaemonClient';
import { outcomeOf, thinkingLabels, thinkingLevels, type Outcome } from '../app/copy';
import { ChipMenu, NoteChip } from './ChipMenu';
import { OutcomeMark } from './StatePill';

export type Fence = { sessionId: string; expectedStreamId: string; expectedCwdFence: string };

/**
 * Session-header thinking level (UX §5.3). The label is always the daemon's read-back value: a request
 * never updates it, an applied change may come back clamped, and an unknown outcome is never retried.
 * Mounted once per session generation, like ModelControl.
 */
export function ThinkingControl({ client, session, fence, connected }: { client: DaemonClient; session: SessionSnapshot; fence: Fence; connected: boolean }) {
  const [busy, setBusy] = useState(false);
  // `adjusted` is fixed once, from an authoritative read: the level the agent chose instead of the request.
  const [result, setResult] = useState<{ outcome: Outcome; adjusted?: ThinkingLevel | null } | null>(null);
  const actual = session.thinkingLevel ?? null;
  const label = `思考强度：${actual ? thinkingLabels[actual] : '未报告'}`;
  // The daemon accepts a change only while the agent is idle.
  const idle = session.state.state === 'ready';

  if (!session.capabilities.thinkingControl) return actual ? <NoteChip className="chip chip-button" note="这个 Agent 不支持调整思考强度">{label}</NoteChip> : null;

  async function choose(level: ThinkingLevel) {
    if (level === actual) return;
    setBusy(true); setResult(null);
    const outcome = outcomeOf(await client.call({ method: 'set_thinking', params: { ...fence, level } }));
    const adjusted = outcome.kind === 'done' ? await readBack(client, fence, view => view.thinkingLevel ?? null, value => value === level) : undefined;
    setBusy(false); setResult({ outcome, adjusted });
  }

  return <ChipMenu className="thinking-control" label={label} name="思考强度" reason={idle ? undefined : '会话空闲时才能调整思考强度'}
    disabled={!connected} busy={busy} onChoose={key => void choose(key as ThinkingLevel)}
    options={thinkingLevels.map(level => ({ key: level, label: thinkingLabels[level], checked: level === actual }))}>
    <OutcomeMark outcome={result?.outcome ?? null} />
    {result?.adjusted !== undefined && <span className="field-note">{result.adjusted ? `Agent 把思考强度调整为“${thinkingLabels[result.adjusted]}”` : 'Agent 没有报告实际的思考强度'}</span>}
  </ChipMenu>;
}

/**
 * The reply to a change may reach this client before or after its session_changed, so the cache cannot tell a
 * clamp. One side-effect-free read settles it without replacing the open view; a failed read, or one of another
 * session generation, claims nothing. Resolves to the value the agent chose instead of the request, or undefined.
 */
export async function readBack<T>(client: DaemonClient, fence: Fence, pick: (session: SessionSnapshot) => T, asked: (value: T) => boolean): Promise<T | undefined> {
  const read = await client.call({ method: 'get_session', params: { sessionId: fence.sessionId } });
  if (read.status !== 'applied' || read.data.result.kind !== 'session') return undefined;
  const view = read.data.result.data.session.session;
  if (view.id !== fence.sessionId || (view.streamId ?? '') !== fence.expectedStreamId || view.cwdFence !== fence.expectedCwdFence) return undefined;
  const value = pick(view);
  return asked(value) ? undefined : value;
}
