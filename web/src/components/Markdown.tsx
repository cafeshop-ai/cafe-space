import { memo, useDeferredValue, useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react';
import { Lexer, type MarkedToken, type Token, type Tokens } from 'marked';

/** Lists and quotes nested deeper than this render as their source text. */
export const MAX_DEPTH = 8;
const MAX_INLINE_DEPTH = 16;
const SCHEMES = new Set(['http:', 'https:', 'mailto:']);
const NAMED = new Map([['amp', '&'], ['lt', '<'], ['gt', '>'], ['quot', '"'], ['apos', "'"], ['nbsp', ' ']]);

/** marked decodes numeric entities but leaves named ones; this decodes both, once. Unlisted names stay as written. */
export function decodeEntities(text: string) {
  return text.replace(/&(?:#(\d{1,7})|#[xX]([0-9a-fA-F]{1,6})|([a-zA-Z]+));/g, (match, dec?: string, hex?: string, name?: string) => {
    if (name) return NAMED.get(name) ?? match;
    const code = dec ? Number(dec) : parseInt(hex!, 16);
    return code > 0 && code <= 0x10ffff && (code < 0xd800 || code > 0xdfff) ? String.fromCodePoint(code) : '�';
  });
}

/** The address a link may open: absolute http, https or mailto only; anything else stays text. */
export function safeUrl(href: string): string | null {
  try {
    const url = new URL(decodeEntities(href).trim());
    return SCHEMES.has(url.protocol) ? url.href : null;
  } catch {
    return null;
  }
}

/**
 * Container nesting at a line start (quote and list markers, or indentation before a list marker) that the
 * lexer would recurse through. Past this, lexing slows sharply or overflows (1000 indented levels took seconds;
 * 5000 exhausted memory), so such text is shown as written instead of lexed.
 */
const MAX_LEX_NESTING = 64;
function tooDeep(text: string) {
  for (const line of text.split('\n')) {
    let markers = 0, columns = 0;
    for (let i = 0; i < line.length; i++) {
      const c = line[i];
      if (c === ' ') { columns++; continue; }
      if (c === '\t') { columns += 4 - (columns % 4); continue; }
      if (c === '>') { if (++markers > MAX_LEX_NESTING) return true; continue; }
      const marker = /^(?:[-+*]|\d{1,9}[.)])(?=[ \t]|$)/.exec(line.slice(i, i + 11));
      if (!marker) break;
      if (++markers > MAX_LEX_NESTING || columns > MAX_LEX_NESTING * 2) return true;
      i += marker[0].length - 1;
    }
  }
  return false;
}

export type Lexed = { tokens: Token[]; links: string };
/**
 * Lexes the whole text every time, so streamed text always ends up as a one-shot parse would (a link
 * definition can arrive after its use). Blocks whose source and the link table are unchanged keep their
 * previous object, so they are not rendered again.
 */
export function lexMarkdown(text: string, previous?: Lexed): Lexed {
  const source: Lexed = { tokens: [{ type: 'source', raw: text }], links: '' };
  if (tooDeep(text)) return source;
  const lexer = new Lexer({ gfm: true });
  let tokens: Token[];
  // Second line of defence: an overflow the scan did not foresee also shows the text as written.
  try { tokens = lexer.lex(text); } catch { return source; }
  const links = JSON.stringify(lexer.tokens.links);
  if (previous?.links === links) tokens = tokens.map((token, i) => previous.tokens[i]?.raw === token.raw ? previous.tokens[i] : token);
  return { tokens, links };
}

async function copyText(text: string) {
  try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
}
const copyNote = (copied: boolean | null) => copied === null ? '' : copied ? '已复制' : '复制失败，请手动选择';

type Open = (url: string) => void;
const sourceText = (text: string, key: number) => <p key={key} className="md-source">{text}</p>;

function link(token: Tokens.Link, open: Open, depth: number, key: number) {
  const url = safeUrl(token.href);
  const label = inline(token.tokens, open, depth + 1);
  const text = decodeEntities(token.text);
  if (!url) return <span key={key}>{label}<span className="md-url"> {decodeEntities(token.href)}</span></span>;
  const same = token.autolink || text === url || text === decodeEntities(token.href);
  return <span key={key}><button type="button" className="md-link" onClick={() => open(url)}>{label}</button>{!same && <span className="md-url"> {url}</span>}</span>;
}

function inline(tokens: Token[] | undefined, open: Open, depth: number): ReactNode {
  if (!tokens) return null;
  if (depth > MAX_INLINE_DEPTH) return tokens.map(token => token.raw).join('');
  return tokens.map((token, key) => {
    const t = token as MarkedToken;
    switch (t.type) {
      case 'text': return t.tokens ? <span key={key}>{inline(t.tokens, open, depth + 1)}</span> : decodeEntities(t.text);
      case 'escape': return decodeEntities(t.text);
      case 'codespan': return <code key={key}>{t.text}</code>;
      case 'strong': return <strong key={key}>{inline(t.tokens, open, depth + 1)}</strong>;
      case 'em': return <em key={key}>{inline(t.tokens, open, depth + 1)}</em>;
      case 'del': return <del key={key}>{inline(t.tokens, open, depth + 1)}</del>;
      case 'br': return <br key={key} />;
      case 'link': return link(t, open, depth, key);
      // Images are never loaded.
      case 'image': return `[图片：${decodeEntities(t.text)}] ${decodeEntities(t.href)}`;
      case 'checkbox': return null;
      // Raw HTML is shown as the text it is, never parsed.
      default: return token.raw;
    }
  });
}

function CodeBlock({ code, lang }: { code: string; lang?: string }) {
  const [copied, setCopied] = useState<boolean | null>(null);
  return <div className="md-code">
    <div className="md-code-bar">
      <span>{lang?.split(/\s/)[0] || '代码'}</span>
      <span className="md-copy-note" role="status">{copyNote(copied)}</span>
      <button type="button" className="quiet-button" onClick={async () => setCopied(await copyText(code))}>复制</button>
    </div>
    <pre tabIndex={0}><code>{code}</code></pre>
  </div>;
}

function block(token: Token, open: Open, depth: number, key: number): ReactNode {
  const t = token as MarkedToken;
  switch (t.type) {
    case 'space': case 'def': case 'checkbox': return null;
    case 'paragraph': return <p key={key}>{inline(t.tokens, open, 0)}</p>;
    case 'text': return <div key={key}>{t.tokens ? inline(t.tokens, open, 0) : decodeEntities(t.text)}</div>;
    case 'heading': {
      // Message headings sit below the page and session headings.
      const Heading = `h${Math.min(t.depth + 2, 6)}` as 'h3';
      return <Heading key={key}>{inline(t.tokens, open, 0)}</Heading>;
    }
    case 'hr': return <hr key={key} />;
    case 'code': return <CodeBlock key={key} code={t.escaped ? decodeEntities(t.text) : t.text} lang={t.lang} />;
    case 'html': return sourceText(t.text, key);
    case 'blockquote':
      if (depth >= MAX_DEPTH) return sourceText(t.raw, key);
      return <blockquote key={key}>{t.tokens.map((child, i) => block(child, open, depth + 1, i))}</blockquote>;
    case 'list': {
      if (depth >= MAX_DEPTH) return sourceText(t.raw, key);
      const List = t.ordered ? 'ol' : 'ul';
      return <List key={key} start={t.ordered && t.start !== '' && t.start !== 1 ? t.start : undefined}>{t.items.map((item, i) =>
        <li key={i} data-task={item.task ? (item.checked ? 'done' : 'open') : undefined}>
          {item.task && <><span className="md-check" aria-hidden="true">{item.checked ? '☑' : '☐'}</span><span className="visually-hidden">{item.checked ? '已完成：' : '未完成：'}</span></>}
          <div className="md-item">{item.tokens.map((child, j) => block(child, open, depth + 1, j))}</div>
        </li>)}</List>;
    }
    case 'table': {
      const align = (cell: Tokens.TableCell) => cell.align ? { textAlign: cell.align } : undefined;
      return <div key={key} className="md-table" role="region" aria-label="表格" tabIndex={0}><table>
        <thead><tr>{t.header.map((cell, i) => <th key={i} style={align(cell)}>{inline(cell.tokens, open, 0)}</th>)}</tr></thead>
        <tbody>{t.rows.map((row, r) => <tr key={r}>{row.map((cell, i) => <td key={i} style={align(cell)}>{inline(cell.tokens, open, 0)}</td>)}</tr>)}</tbody>
      </table></div>;
    }
    default: return sourceText(token.raw, key);
  }
}

/** A top-level block re-renders only when its token object changes. */
const Block = memo(function Block({ token, open }: { token: Token; open: Open }) { return block(token, open, 0, 0); });

/** The address is shown in full before anything leaves the app. */
function LinkSheet({ url, onClose }: { url: string; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useId();
  const [copied, setCopied] = useState<boolean | null>(null);
  useEffect(() => { dialog.current?.showModal(); }, []);
  return <dialog ref={dialog} className="sheet" aria-labelledby={title} onClose={onClose}>
    <h2 id={title}>外部链接</h2>
    <p className="field-note">这个链接来自 Agent 的回复，可能指向任何网站，请先核对完整地址。</p>
    <p className="md-sheet-url"><code>{url}</code></p>
    <p className="field-note">{url.startsWith('mailto:') ? '目前还不能从这里打开邮件应用，可以复制地址。' : '目前还不能从这里直接打开，可以复制后粘贴到浏览器。'}</p>
    <div className="sheet-actions">
      <span className="md-copy-note" role="status">{copyNote(copied)}</span>
      <button type="button" className="secondary-button" onClick={() => dialog.current?.close()}>关闭</button>
      <button type="button" className="primary-button" onClick={async () => setCopied(await copyText(url))}>复制链接</button>
    </div>
  </dialog>;
}

/** Assistant text as a safe Markdown subset: React elements only, no raw HTML, no loaded resources. */
export function Markdown({ text }: { text: string }) {
  // While a long reply streams, React may skip intermediate texts instead of lexing every one.
  const deferred = useDeferredValue(text);
  const cache = useRef<Lexed>(undefined);
  const lexed = useMemo(() => (cache.current = lexMarkdown(deferred, cache.current)), [deferred]);
  const [url, setUrl] = useState<string | null>(null);
  return <div className="markdown">
    {lexed.tokens.map((token, key) => <Block key={key} token={token} open={setUrl} />)}
    {url && <LinkSheet url={url} onClose={() => setUrl(null)} />}
  </div>;
}
