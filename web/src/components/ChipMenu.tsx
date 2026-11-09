import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';

export type ChipOption = { key: string; label: string; checked: boolean };
// WebKit does not focus buttons on click; keeping focus put stops the menu from closing before the click lands.
const keepFocus = (event: { preventDefault(): void }) => event.preventDefault();

/** A chip that explains itself on tap: a title tooltip never shows on touch screens. `float` shows the note as a popover. */
export function NoteChip({ className, note, value, float, children }: { className: string; note: string; value?: string; float?: boolean; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open || !float) return;
    // WebKit does not focus a tapped button, so only a tap elsewhere can close the popover.
    const away = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener('pointerdown', away);
    return () => document.removeEventListener('pointerdown', away);
  }, [open, float]);
  return <span ref={root} className="chip-control">
    <button type="button" className={className} data-value={value} aria-expanded={open} onClick={() => setOpen(!open)}
      onKeyDown={event => { if (event.key === 'Escape' && open) { event.preventDefault(); setOpen(false); } }}>
      {children}<span aria-hidden="true"> ⓘ</span>
    </button>
    {open && <span className={float ? 'chip-menu chip-menu-note' : 'field-note'} role="note">{note}</span>}
  </span>;
}

/**
 * A session-header chip with a radio menu (UX §5.3). `note` stands in for the menu while there is nothing to choose;
 * `reason`, shown on tap, while the menu cannot be used at all.
 */
export function ChipMenu({ className, label, name, reason, disabled, busy, options, note, onOpen, onChoose, children }: {
  className: string; label: string; name: string; reason?: string; disabled: boolean; busy: boolean;
  options: ChipOption[]; note?: string | null; onOpen?: () => void; onChoose: (key: string) => void; children?: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLUListElement>(null);
  const shown = open && !reason && options.length > 0;
  // Whatever is open closes when the chip gains or loses its use.
  useEffect(() => setOpen(false), [disabled, reason]);
  useEffect(() => {
    if (shown) (menu.current?.querySelector<HTMLElement>('[aria-checked="true"]') ?? menu.current?.querySelector<HTMLElement>('button'))?.focus();
  }, [shown]);

  // While a change is in flight the trigger keeps focus but does not open.
  function openMenu() { if (!busy) { setOpen(true); if (!reason) onOpen?.(); } }
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
    <button ref={trigger} type="button" className="chip chip-button" aria-haspopup={reason ? undefined : 'menu'} aria-expanded={open} aria-busy={busy} disabled={disabled}
      onMouseDown={keepFocus} onClick={() => open ? close() : openMenu()}
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
    </ul> : open && !reason && note && <p className="chip-menu chip-menu-note" role="status">{note}</p>}
    {open && reason && <span className="field-note" role="note">{reason}</span>}
    {children}
  </span>;
}
