import { Component, createContext, memo, useContext, useDeferredValue, useEffect, useId, useMemo, useRef, useState, type ComponentProps, type JSX, type ReactNode } from 'react';
import { TextMessagePartProvider } from '@assistant-ui/react';
import { MarkdownTextPrimitive, type MarkdownTextPrimitiveProps, type SyntaxHighlighterProps } from '@assistant-ui/react-markdown';
import { isTauri } from '@tauri-apps/api/core';
import { openUrl } from '@tauri-apps/plugin-opener';
import { gfm } from 'micromark-extension-gfm';
import { gfmFromMarkdown } from 'mdast-util-gfm';

/** Lists and quotes nested deeper than this render as their source text. */
export const MAX_DEPTH = 8;
const SCHEMES = new Set(['http:', 'https:', 'mailto:']);

/** The address a link may open: absolute http, https or mailto only; anything else stays text. */
export function safeUrl(href: string): string | null {
  try {
    const url = new URL(href.trim());
    return SCHEMES.has(url.protocol) ? url.href : null;
  } catch {
    return null;
  }
}

/**
 * Container nesting at a line start, or delimiter runs, that the parser and the recursive tree-to-React
 * steps would descend through. Past this the text is shown as written instead of parsed.
 */
const MAX_PARSE_NESTING = 64;
export function tooDeep(text: string) {
  if (/[*_~]{65,}|\[{65,}/.test(text)) return true;
  for (const line of text.split('\n')) {
    let markers = 0, columns = 0;
    for (let i = 0; i < line.length; i++) {
      const c = line[i];
      if (c === ' ') { columns++; continue; }
      if (c === '\t') { columns += 4 - (columns % 4); continue; }
      if (c === '>') { if (++markers > MAX_PARSE_NESTING) return true; continue; }
      const marker = /^(?:[-+*]|\d{1,9}[.)])(?=[ \t]|$)/.exec(line.slice(i, i + 11));
      if (!marker) break;
      if (++markers > MAX_PARSE_NESTING || columns > MAX_PARSE_NESTING * 2) return true;
      i += marker[0].length - 1;
    }
  }
  return false;
}

/** Long replies are parsed in pieces of about this many characters, so a streamed update re-parses only the last one. */
export const PIECE = 4096;
const LIST_ITEM = /^(?:[-+*]|\d{1,9}[.)])(?:[ \t]|$)/;
const HEADING = /^#{1,6}(?:[ \t]|$)/;
const fenceOpen = (line: string) => { const m = /^(`{3,})[^`]*$|^(~{3,})/.exec(line); return m ? m[1] ?? m[2] : ''; };
const fenceCloses = (line: string, fence: string) => {
  const m = /^ {0,3}(`{3,}|~{3,})[ \t]*$/.exec(line);
  return !!m && m[1][0] === fence[0] && m[1].length >= fence.length;
};

/**
 * Cuts only before a complete line at column 0 outside any fence that starts a new top-level block whatever came
 * before: an ATX heading, or after a blank line anything but a list item. Every earlier block is closed there, so
 * each piece parses as it would inside the whole text. Text whose meaning can cross that line (definitions, raw HTML
 * that may span blank lines, fences inside containers) stays whole. Appending text never moves an earlier cut.
 */
export function pieces(text: string): string[] {
  if (text.length <= PIECE || text.includes(']:') || /^ {0,3}</m.test(text) || /^ {1,3}(?:`{3}|~{3})/m.test(text)) return [text];
  const out: string[] = [];
  const lines = text.split('\n');
  let start = 0, offset = 0, blank = false, fence = '';
  for (let i = 0; i < lines.length - 1; i++) {
    const line = lines[i].replace(/\r$/, '');
    if (fence) {
      if (fenceCloses(line, fence)) fence = '';
      blank = false;
    } else {
      const opens = blank ? /^\S/.test(line) && !LIST_ITEM.test(line) : HEADING.test(line);
      if (opens && offset - start >= PIECE) { out.push(text.slice(start, offset)); start = offset; }
      fence = fenceOpen(line);
      blank = /^[ \t]*$/.test(line);
    }
    offset += lines[i].length + 1;
  }
  out.push(text.slice(start));
  return out;
}

type Hast = { type: string; tagName?: string; value?: string; properties?: Record<string, unknown>; children?: Hast[]; position?: { start: { offset?: number }; end: { offset?: number } } };
const sourceBlock = (value: string): Hast => ({ type: 'element', tagName: 'p', properties: { className: ['md-source'] }, children: [{ type: 'text', value }] });
const textOf = (node: Hast): string => node.value ?? (node.children ?? []).map(textOf).join('');
const CONTAINERS = new Set(['blockquote', 'ul', 'ol']);

/** Block-level raw HTML becomes a source paragraph, and containers past MAX_DEPTH become their source text. */
function rehypeBounded() {
  return (tree: Hast, file: { value: unknown }) => {
    const source = String(file.value);
    const asWritten = (node: Hast) => {
      const start = node.position?.start.offset, end = node.position?.end.offset;
      return sourceBlock(start != null && end != null ? source.slice(start, end) : textOf(node));
    };
    const walk = (node: Hast, depth: number) => node.children?.forEach((child, i, children) => {
      if (child.type === 'raw' && node.type === 'root') children[i] = sourceBlock(child.value ?? '');
      else if (child.type === 'element') {
        const container = CONTAINERS.has(child.tagName!);
        if (container && depth >= MAX_DEPTH) children[i] = asWritten(child);
        else walk(child, depth + (container ? 1 : 0));
      }
    });
    walk(tree, 0);
  };
}

/** Only http, https and mailto survive into the tree; what was written is kept for display as text. */
const urlTransform: NonNullable<MarkdownTextPrimitiveProps['urlTransform']> = (url, key, node) => {
  if (key === 'href' || key === 'src') node.properties.dataWritten = url;
  return safeUrl(url) ?? '';
};

async function copyText(text: string) {
  try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
}
const copyNote = (copied: boolean | null) => copied === null ? '' : copied ? '已复制' : '复制失败，请手动选择';

const OpenLink = createContext<(url: string) => void>(() => {});
type Rendered<T extends keyof JSX.IntrinsicElements> = ComponentProps<T> & { node?: unknown; 'data-written'?: string };
const strip = (url: string) => url.replace(/^(?:https?:\/\/|mailto:)/, '').replace(/\/$/, '');

function Link({ href, children, 'data-written': written }: Rendered<'a'>) {
  const open = useContext(OpenLink);
  if (!href) return <span>{children}{written && <span className="md-url"> {written}</span>}</span>;
  const same = typeof children === 'string' && [href, written ?? ''].some(url => url === children || strip(url) === strip(children));
  return <span><button type="button" className="md-link" onClick={() => open(href)}>{children}</button>{!same && <span className="md-url"> {href}</span>}</span>;
}

// Images are never loaded.
const Image = ({ alt, src, 'data-written': written }: Rendered<'img'>) => <>{`[图片：${alt ?? ''}] ${src || written || ''}`}</>;

function Check({ type, checked }: Rendered<'input'>) {
  if (type !== 'checkbox') return null;
  return <><span className="md-check" aria-hidden="true">{checked ? '☑' : '☐'}</span><span className="visually-hidden">{checked ? '已完成：' : '未完成：'}</span></>;
}

function Item({ node, children, className }: Rendered<'li'>) {
  const box = (node as Hast | undefined)?.children?.find(child => child.tagName === 'input');
  return <li className={className} data-task={box ? (box.properties?.checked ? 'done' : 'open') : undefined}>{children}</li>;
}

const Table = ({ node: _, ...props }: Rendered<'table'>) => <div className="md-table" role="region" aria-label="表格" tabIndex={0}><table {...props} /></div>;

// Message headings sit below the page and session headings.
const heading = (level: number) => function Heading({ node: _, ...props }: Rendered<'h1'>) {
  const Tag = `h${Math.min(level + 2, 6)}` as 'h3';
  return <Tag {...props} />;
};

function CodeBlock({ code, language }: SyntaxHighlighterProps) {
  const [copied, setCopied] = useState<boolean | null>(null);
  const text = code.replace(/\n$/, '');
  return <div className="md-code">
    <div className="md-code-bar">
      <span>{language || '代码'}</span>
      <span className="md-copy-note" role="status">{copyNote(copied)}</span>
      <button type="button" className="quiet-button" onClick={async () => setCopied(await copyText(text))}>复制</button>
    </div>
    <pre tabIndex={0}><code>{text}</code></pre>
  </div>;
}

const components: MarkdownTextPrimitiveProps['components'] = {
  a: Link, img: Image, input: Check, li: Item, table: Table, SyntaxHighlighter: CodeBlock,
  h1: heading(1), h2: heading(2), h3: heading(3), h4: heading(4), h5: heading(5), h6: heading(6),
};
/** GFM for reading only (tables, task lists, strikethrough, autolinks, footnotes); nothing here writes Markdown back. */
function remarkGfmRead(this: { data(): unknown }) {
  const data = this.data() as { micromarkExtensions?: unknown[]; fromMarkdownExtensions?: unknown[] };
  (data.micromarkExtensions ??= []).push(gfm());
  (data.fromMarkdownExtensions ??= []).push(gfmFromMarkdown());
}
const remarkPlugins = [remarkGfmRead];
const rehypePlugins = [rehypeBounded];
const remarkRehypeOptions = { footnoteLabel: '脚注', footnoteBackLabel: '回到正文' };

/** A tree the renderer still cannot handle is shown as written. */
class Fallback extends Component<{ text: string; children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() { return { failed: true }; }
  render() { return this.state.failed ? <div className="markdown"><p className="md-source">{this.props.text}</p></div> : this.props.children; }
}

/** The address is shown in full before anything leaves the app; only the confirm button opens it, once per press. */
function LinkSheet({ url, onClose }: { url: string; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useId();
  const [copied, setCopied] = useState<boolean | null>(null);
  const [opening, setOpening] = useState<'idle' | 'busy' | 'failed'>('idle');
  const native = isTauri();
  const mail = url.startsWith('mailto:');
  const app = mail ? '邮件应用' : '浏览器';
  useEffect(() => { dialog.current?.showModal(); }, []);
  async function open() {
    setOpening('busy');
    // Handed to the system's default app; a failure is reported and never retried.
    try { await openUrl(url); dialog.current?.close(); } catch { setOpening('failed'); }
  }
  return <dialog ref={dialog} className="sheet" aria-labelledby={title} onClose={onClose}>
    <h2 id={title}>外部链接</h2>
    <p className="field-note">这个链接来自 Agent 的回复，可能指向任何网站，请先核对完整地址。</p>
    <p className="md-sheet-url"><code>{url}</code></p>
    <p className="field-note">{native ? `确认后用默认的${app}打开。` : mail ? '目前还不能从这里打开邮件应用，可以复制地址。' : '目前还不能从这里直接打开，可以复制后粘贴到浏览器。'}</p>
    {opening === 'failed' && <p className="field-note" role="alert">没能打开{app}，可以复制地址后手动打开。</p>}
    <div className="sheet-actions">
      <span className="md-copy-note" role="status">{copyNote(copied)}</span>
      <button type="button" className="secondary-button" onClick={() => dialog.current?.close()}>关闭</button>
      <button type="button" className={native ? 'secondary-button' : 'primary-button'} onClick={async () => setCopied(await copyText(url))}>复制链接</button>
      {native && <button type="button" className="primary-button" disabled={opening === 'busy'} onClick={() => void open()}>用{app}打开</button>}
    </div>
  </dialog>;
}

const Bare = ({ children }: { children?: ReactNode }) => <>{children}</>;

/** An unchanged piece is not parsed again; every piece but the first is laid out in a background render. */
const Piece = memo(function Piece({ text, first }: { text: string; first: boolean }) {
  const shown = useDeferredValue(text, first ? text : '');
  if (!shown) return null;
  return <TextMessagePartProvider text={shown}>
    <MarkdownTextPrimitive containerComponent={Bare} smooth={false} components={components} remarkPlugins={remarkPlugins}
      rehypePlugins={rehypePlugins} remarkRehypeOptions={remarkRehypeOptions} urlTransform={urlTransform} />
  </TextMessagePartProvider>;
});

/** Assistant text part as safe Markdown: React elements only, raw HTML as text, no loaded resources. */
export function MarkdownText({ text }: { text: string }) {
  const [url, setUrl] = useState<string | null>(null);
  const deep = useMemo(() => tooDeep(text), [text]);
  const parts = useMemo(() => deep ? [] : pieces(text), [deep, text]);
  return <OpenLink.Provider value={setUrl}>
    {deep ? <div className="markdown"><p className="md-source">{text}</p></div> : <Fallback text={text}>
      <div className="markdown">{parts.map((part, i) => <Piece key={i} text={part} first={i === 0} />)}</div>
    </Fallback>}
    {url && <LinkSheet url={url} onClose={() => setUrl(null)} />}
  </OpenLink.Provider>;
}
