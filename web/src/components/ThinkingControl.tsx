import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { ThinkingLevel } from '../protocol/generated/ThinkingLevel';
import type { DaemonClient } from '../state/DaemonClient';
import { outcomeOf, thinkingLabels, thinkingLevels, type Outcome } from '../app/copy';
import { OutcomeMark } from './StatePill';

type Fence = { sessionId: string; expectedStreamId: string; expectedCwdFence: string };
// WebKit does not focus buttons on click; keeping focus put stops the menu from closing before the click lands.
const keepFocus = (event: { preventDefault(): void }) => event.preventDefault();

/**
 * Session-header thinking level (UX §5.3). The label is always the daemon's read-back value: a request
 * never updates it, an applied change may come back clamped, and an unknown outcome is never retried.
 */
export function ThinkingControl({ client, session, fence, connected }: { client: DaemonClient; session: SessionSnapshot; fence: Fence; connected: boolean }) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  // `adjusted` is fixed once, from an authoritative read: the level the agent chose instead of the request.
  const [result, setResult] = useState<{ outcome: Outcome; adjusted?: ThinkingLevel | null } | null>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLUListElement>(null);
  const actual = session.thinkingLevel ?? null;
  const label = `思考强度：${actual ? thinkingLabels[actual] : '未报告'}`;
  // The daemon accepts a change only while the agent is idle.
  const idle = session.state.state === 'ready';
  const disabled = !connected || !idle;
  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  useEffect(() => {
    if (open) (menu.current?.querySelector<HTMLElement>('[aria-checked="true"]') ?? menu.current?.querySelector<HTMLElement>('button'))?.focus();
  }, [open]);

  if (!session.capabilities.thinkingControl) return actual ? <span className="chip" title="这个 Agent 不支持调整思考强度">{label}</span> : null;

  // While a change is in flight the trigger keeps focus but does not open.
  function openMenu() { if (!busy) setOpen(true); }
  function close() { setOpen(false); trigger.current?.focus(); }
  async function choose(level: ThinkingLevel) {
    close();
    if (level === actual) return;
    setBusy(true); setResult(null);
    const outcome = outcomeOf(await client.call({ method: 'set_thinking', params: { ...fence, level } }));
    // The reply may reach this client before or after its session_changed, so the cache cannot tell a clamp.
    // One side-effect-free read settles it without replacing the open view.
    let adjusted: ThinkingLevel | null | undefined;
    if (outcome.kind === 'done') {
      const read = await client.call({ method: 'get_session', params: { sessionId: fence.sessionId } });
      // A failed read claims nothing; the header still follows the event stream.
      if (read.status === 'applied' && read.data.result.kind === 'session') {
        const value = read.data.result.data.session.session.thinkingLevel ?? null;
        if (value !== level) adjusted = value;
      }
    }
    setBusy(false); setResult({ outcome, adjusted });
  }
  function onMenuKey(event: KeyboardEvent<HTMLUListElement>) {
    const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button')];
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = ({ ArrowDown: index + 1, ArrowUp: index - 1, Home: 0, End: -1 } as Record<string, number>)[event.key];
    if (next !== undefined) { event.preventDefault(); items.at(next % items.length)?.focus(); }
    else if (event.key === 'Escape') { event.preventDefault(); close(); }
    else if (event.key === 'Tab') setOpen(false);
  }

  return <span className="thinking-control" onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false); }}>
    <button ref={trigger} type="button" className="chip chip-button" aria-haspopup="menu" aria-expanded={open} aria-busy={busy} disabled={disabled}
      title={idle ? undefined : '会话空闲时才能调整思考强度'} onMouseDown={keepFocus} onClick={() => open ? close() : openMenu()}
      onKeyDown={event => { if (event.key === 'ArrowDown') { event.preventDefault(); openMenu(); } }}>
      {label}<span aria-hidden="true"> ▾</span>
    </button>
    {open && <ul ref={menu} role="menu" aria-label="思考强度" className="chip-menu" onKeyDown={onMenuKey} onMouseDown={keepFocus}>
      {thinkingLevels.map(level => <li key={level} role="none">
        <button type="button" role="menuitemradio" aria-checked={level === actual} tabIndex={-1} onClick={() => void choose(level)}>{thinkingLabels[level]}</button>
      </li>)}
    </ul>}
    <OutcomeMark outcome={result?.outcome ?? null} />
    {result?.adjusted !== undefined && <span className="field-note">{result.adjusted ? `Agent 把思考强度调整为“${thinkingLabels[result.adjusted]}”` : 'Agent 没有报告实际的思考强度'}</span>}
  </span>;
}
