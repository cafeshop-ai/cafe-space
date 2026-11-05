import { useEffect, useId, useRef, useState, type KeyboardEvent } from 'react';
import type { DaemonView } from '../state/DaemonClient';
import { stateLabels } from '../app/copy';
import { sessionHref, sessionTitle } from './SessionList';

const mac = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);
/** Shortcut text in the platform's notation (UX §6.7: Ctrl in place of ⌘ on Windows and Linux). */
export const shortcut = (key: string) => mac ? `⌘${key}` : `Ctrl+${key}`;
/** The palette asks the open session view to stop its turn, exactly as ⌘. does. */
export const STOP_EVENT = 'cafe-space:stop-turn';
/** ⌘1 / ⌘2 / ⌘3 / ⌘, (UX §6.7). */
export const pageKeys: Record<string, string> = { '1': '#/sessions', '2': '#/rooms', '3': '#/remote', ',': '#/settings' };
const pages = [['#/sessions', '会话', '1'], ['#/rooms', '房间', '2'], ['#/remote', '远程访问', '3'], ['#/settings', '设置', ',']] as const;
const MAX_SESSIONS = 50;

type Command = { id: string; label: string; hint: string; run: () => void };
const matching = (commands: Command[], terms: string[]) => commands.filter(command => terms.every(term => command.label.toLowerCase().includes(term)));

/** ⌘K palette (UX §6.7): jump to any page or session, start a session, stop the open session's turn. */
export function CommandPalette({ open, onClose, state, onNavigate, onNewSession }: {
  open: boolean; onClose: () => void; state: DaemonView; onNavigate: (href: string) => void; onNewSession: () => void;
}) {
  const [query, setQuery] = useState('');
  // Selection follows a command, never a position: a list that changes under the reader must not retarget Enter.
  // null picks the first command of a fresh list; '' is a cleared selection that nothing matches.
  const [activeId, setActiveId] = useState<string | null>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const list = useId();
  useEffect(() => {
    const element = dialog.current;
    if (!element) return;
    if (open && !element.open) { setQuery(''); setActiveId(null); element.showModal(); }
    if (!open && element.open) element.close();
  }, [open]);

  const connected = state.connection === 'connected';
  const session = state.session?.detail?.view.session;
  const runId = session?.activeRunId;
  const canStop = connected && !!runId && (session.state.state === 'running' || session.state.state === 'waiting_for_local_ui');
  const actions: Command[] = [
    ...(connected ? [{ id: 'action:new', label: '新建会话', hint: shortcut('N'), run: onNewSession }] : []),
    // The open session view stops its own turn, exactly as ⌘. does, and only the run this command was shown for.
    ...(canStop ? [{ id: `action:stop:${JSON.stringify([session.id, session.streamId, session.cwdFence, runId])}`, label: '停止当前轮', hint: shortcut('.'), run: () => window.dispatchEvent(new CustomEvent(STOP_EVENT, { detail: runId })) }] : []),
    ...pages.map(([href, label, key]) => ({ id: `page:${href}`, label, hint: shortcut(key), run: () => onNavigate(href) })),
  ];
  const sessions = (state.sessions ?? []).map(item => ({ id: `session:${item.id}`, label: sessionTitle(item, state.runtimes), hint: stateLabels[item.state.state], run: () => onNavigate(sessionHref(item.id)) }));
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const commands = [...matching(actions, terms), ...matching(sessions, terms).slice(0, MAX_SESSIONS)];
  const first = commands[0]?.id ?? null;
  const current = commands.findIndex(command => command.id === activeId);
  // A vanished choice is cleared for good, so it never comes back pointing at a new target.
  useEffect(() => {
    if (activeId === null && first) setActiveId(first);
    else if (activeId && current < 0) setActiveId('');
  }, [activeId, first, current]);
  useEffect(() => { document.getElementById(`${list}-${current}`)?.scrollIntoView?.({ block: 'nearest' }); }, [list, current]);

  function run(command: Command | undefined) {
    if (!command) return;
    onClose();
    command.run();
  }
  function onKey(event: KeyboardEvent<HTMLInputElement>) {
    // During IME composition the arrows and Enter belong to the candidate list.
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (!commands.length) return;
      const down = event.key === 'ArrowDown';
      const next = current < 0 ? (down ? 0 : commands.length - 1) : (current + (down ? 1 : -1) + commands.length) % commands.length;
      setActiveId(commands[next].id);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      run(commands[current]);
    }
  }

  return <dialog ref={dialog} className="palette" aria-label="命令面板" onClose={onClose}>
    <input className="palette-input" role="combobox" aria-expanded="true" aria-controls={list} aria-autocomplete="list"
      aria-activedescendant={current >= 0 ? `${list}-${current}` : undefined} placeholder="搜索会话、页面或操作" value={query}
      onChange={event => { setQuery(event.target.value); setActiveId(null); }} onKeyDown={onKey} />
    {commands.length ? <ul id={list} role="listbox" aria-label="结果" className="palette-list">{commands.map((command, index) =>
      <li key={command.id} id={`${list}-${index}`} role="option" aria-selected={index === current} onMouseMove={() => setActiveId(command.id)} onClick={() => run(command)}>
        <span className="palette-label">{command.label}</span><span className="palette-hint">{command.hint}</span>
      </li>)}</ul> : <p className="palette-empty">没有匹配的结果</p>}
  </dialog>;
}
