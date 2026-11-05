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
  const [result, setResult] = useState<{ requested: ThinkingLevel; outcome: Outcome } | null>(null);
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
    setBusy(false); setResult({ requested: level, outcome });
  }
  function onMenuKey(event: KeyboardEvent<HTMLUListElement>) {
    const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button')];
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = ({ ArrowDown: index + 1, ArrowUp: index - 1, Home: 0, End: -1 } as Record<string, number>)[event.key];
    if (next !== undefined) { event.preventDefault(); items.at(next % items.length)?.focus(); }
    else if (event.key === 'Escape') { event.preventDefault(); close(); }
    else if (event.key === 'Tab') setOpen(false);
  }

  const clamped = result?.outcome.kind === 'done' && actual !== result.requested;
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
    {clamped && <span className="field-note">{actual ? `Agent 把思考强度调整为“${thinkingLabels[actual]}”` : 'Agent 没有报告实际的思考强度'}</span>}
  </span>;
}
