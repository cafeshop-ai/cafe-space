import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';

export type ChipOption = { key: string; label: string; checked: boolean };
// WebKit does not focus buttons on click; keeping focus put stops the menu from closing before the click lands.
const keepFocus = (event: { preventDefault(): void }) => event.preventDefault();

/** A session-header chip with a radio menu (UX §5.3). `note` stands in for the menu while there is nothing to choose. */
export function ChipMenu({ className, label, name, title, disabled, busy, options, note, onOpen, onChoose, children }: {
  className: string; label: string; name: string; title?: string; disabled: boolean; busy: boolean;
  options: ChipOption[]; note?: string | null; onOpen?: () => void; onChoose: (key: string) => void; children?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLUListElement>(null);
  const shown = open && options.length > 0;
  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  useEffect(() => {
    if (shown) (menu.current?.querySelector<HTMLElement>('[aria-checked="true"]') ?? menu.current?.querySelector<HTMLElement>('button'))?.focus();
  }, [shown]);

  // While a change is in flight the trigger keeps focus but does not open.
  function openMenu() { if (!busy) { setOpen(true); onOpen?.(); } }
  function close() { setOpen(false); trigger.current?.focus(); }
  function onMenuKey(event: KeyboardEvent<HTMLUListElement>) {
    const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button')];
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    const next = ({ ArrowDown: index + 1, ArrowUp: index - 1, Home: 0, End: -1 } as Record<string, number>)[event.key];
    if (next !== undefined) { event.preventDefault(); items.at(next % items.length)?.focus(); }
    else if (event.key === 'Escape') { event.preventDefault(); close(); }
    else if (event.key === 'Tab') setOpen(false);
  }

  return <span className={`chip-control ${className}`} onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false); }}>
    <button ref={trigger} type="button" className="chip chip-button" aria-haspopup="menu" aria-expanded={open} aria-busy={busy} disabled={disabled}
      title={title} onMouseDown={keepFocus} onClick={() => open ? close() : openMenu()}
      onKeyDown={event => {
        if (event.key === 'ArrowDown') { event.preventDefault(); openMenu(); }
        else if (event.key === 'Escape' && open) { event.preventDefault(); setOpen(false); }
      }}>
      {label}<span aria-hidden="true"> ▾</span>
    </button>
    {shown ? <ul ref={menu} role="menu" aria-label={name} className="chip-menu" onKeyDown={onMenuKey} onMouseDown={keepFocus}>
      {options.map(option => <li key={option.key} role="none">
        <button type="button" role="menuitemradio" aria-checked={option.checked} tabIndex={-1} onClick={() => { close(); onChoose(option.key); }}>{option.label}</button>
      </li>)}
    </ul> : open && note && <p className="chip-menu chip-menu-note" role="status">{note}</p>}
    {children}
  </span>;
}
