import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { MAX_DEPTH, PIECE, pieces, safeUrl, tooDeep } from '../src/components/Markdown';
import { ThreadProvider, Transcript, loadMarkdown } from '../src/components/SessionThread';
import type { TranscriptMessage } from '../src/protocol/generated/TranscriptMessage';

const opener = vi.hoisted(() => ({ native: false, openUrl: vi.fn(async (_url: string, _with?: string) => {}) }));
vi.mock('@tauri-apps/api/core', async original => ({ ...await original<object>(), isTauri: () => opener.native }));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: opener.openUrl }));
// Every parse builds a fresh processor, which asks for the GFM syntax once.
const parser = vi.hoisted(() => ({ parses: 0 }));
vi.mock('micromark-extension-gfm', async original => {
  const real = await original<typeof import('micromark-extension-gfm')>();
  return { ...real, gfm: () => { parser.parses++; return real.gfm(); } };
});

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
beforeAll(() => loadMarkdown());
beforeAll(() => {
  HTMLDialogElement.prototype.showModal = function () { this.setAttribute('open', ''); };
  HTMLDialogElement.prototype.close = function () { this.removeAttribute('open'); this.dispatchEvent(new Event('close')); };
});

let root: Root | undefined;
let element: HTMLElement;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = undefined;
  document.body.replaceChildren();
  vi.unstubAllGlobals();
  opener.native = false;
  opener.openUrl.mockReset();
});
/** One assistant message with this text, rendered as the session page renders it. */
const thread = (text: string) => <ThreadProvider messages={[{ id: 'm1', role: 'assistant', status: 'complete', parts: [{ kind: 'text', data: { text } }] } satisfies TranscriptMessage]} pending={[]}><Transcript agent="Pi" /></ThreadProvider>;
async function render(text: string) {
  if (!root) { element = document.createElement('div'); document.body.append(element); root = createRoot(element); }
  await act(async () => root!.render(thread(text)));
  return element.querySelector<HTMLElement>('.markdown')!;
}
const fresh = async (text: string) => { await act(async () => root?.unmount()); root = undefined; document.body.replaceChildren(); return render(text); };
const clipboard = () => {
  const writeText = vi.fn(async (_text: string) => {});
  vi.stubGlobal('navigator', { ...navigator, clipboard: { writeText } });
  return writeText;
};
const button = (label: string) => [...document.querySelectorAll('button')].find(item => item.textContent === label)!;

describe('markdown safety', () => {
  it('shows raw HTML as text and never creates active elements', async () => {
    const view = await render('<div onclick="steal()">块</div>\n\n行内 <script>alert(1)</script> <img src=x onerror=alert(1)> <iframe src="https://e.test"></iframe>');
    expect(view.querySelector('script, img, iframe, div[onclick], [onerror], a')).toBeNull();
    expect(view.textContent).toContain('<div onclick="steal()">块</div>');
    expect(view.textContent).toContain('<script>alert(1)</script>');
    expect(view.textContent).toContain('<img src=x onerror=alert(1)>');
  });

  it('turns only http, https and mailto into links', async () => {
    const unsafe = ['javascript:alert(1)', 'JAVASCRIPT:alert(1)', 'java\tscript:alert(1)', 'jav&#x61;script:alert(1)', 'data:text/html,<b>x</b>', 'file:///etc/passwd', 'vbscript:msgbox(1)', '/relative', '//evil.test/x', 'tel:123'];
    for (const href of unsafe) expect(safeUrl(href)).toBeNull();
    expect(safeUrl('HTTPS://Example.test/a?b=1&c=2')).toBe('https://example.test/a?b=1&c=2');
    expect(safeUrl('mailto:alex@example.test')).toBe('mailto:alex@example.test');
    const view = await render(`${unsafe.slice(0, 4).map((href, i) => `[x${i}](${href})`).join(' ')} [ok](https://example.test/a) <https://example.test/b> www.example.test [mail](mailto:alex@example.test)`);
    expect(view.querySelector('a, [href], [src]')).toBeNull();
    expect([...view.querySelectorAll('.md-link')].map(item => item.textContent)).toEqual(['ok', 'https://example.test/b', 'www.example.test', 'mail']);
    // What an unsafe link pointed at stays readable as text, entity-encoded schemes included.
    expect(view.textContent).toContain('x0 javascript:alert(1)');
    expect(view.textContent).toContain('x3 javascript:alert(1)');
  });

  it('decodes entities once outside code, keeps unknown names as written and code literal', async () => {
    expect((await render('&quot;q&quot; &#x3C; &#169; &amp;amp; &copy; &#0; &constructor; &toString; &hasOwnProperty; &valueOf;')).textContent)
      .toBe('"q" < © &amp; © \uFFFD &constructor; &toString; &hasOwnProperty; &valueOf;');
    const view = await render('a &amp; b &lt;c&gt; `&amp;` **&lt;b&gt;**\n\n```\n&amp; <i>\n```');
    expect(view.querySelector('p')!.textContent).toBe('a & b <c> &amp; <b>');
    expect(view.querySelector('pre code')!.textContent).toBe('&amp; <i>');
  });

  it('never loads images', async () => {
    const view = await render('![logo](https://example.test/logo.png)');
    expect(view.querySelector('img')).toBeNull();
    expect(view.textContent).toBe('[图片：logo] https://example.test/logo.png');
  });

  it('limits nesting while rendering and before parsing', async () => {
    let view = await render(`${'> '.repeat(MAX_DEPTH + 4)}深处`);
    expect(view.querySelectorAll('blockquote')).toHaveLength(MAX_DEPTH);
    expect(view.querySelector('.md-source')?.textContent).toContain('> > > > 深处');
    view = await render(Array.from({ length: MAX_DEPTH + 4 }, (_, i) => `${'  '.repeat(i)}- 第 ${i} 层`).join('\n'));
    expect(view.querySelectorAll('ul')).toHaveLength(MAX_DEPTH);
    expect(view.textContent).toContain('第 11 层');
    // Inputs that would drive the parser or the recursive tree steps too deep are shown as written without parsing.
    for (const text of ['>'.repeat(30000) + ' x', `开头\r${'>'.repeat(30000)} x`, '- '.repeat(5000) + 'x', Array.from({ length: 1000 }, (_, i) => `${'  '.repeat(i)}- a`).join('\n'), `${'*'.repeat(10000)}x${'*'.repeat(10000)}`, `${'['.repeat(5000)}x`]) {
      expect(tooDeep(text)).toBe(true);
      view = await render(text);
      expect(view.querySelector('blockquote, ul, em, strong')).toBeNull();
      expect(view.querySelector('.md-source')?.textContent).toBe(text);
    }
    expect(tooDeep(`${'> '.repeat(MAX_DEPTH + 4)}深处\n${'  - '.repeat(20)}x\n***粗斜***`)).toBe(false);
  });
});

describe('markdown rendering', () => {
  it('renders headings, plan checklists, tables and quotes', async () => {
    const view = await render('# 计划\n\n- [x] 读代码\n- [ ] 写测试\n\n| 文件 | 状态 |\n|---|:-:|\n| a.ts | 完成 |\n\n> 注意');
    expect(view.querySelector('h3')?.textContent).toBe('计划');
    expect([...view.querySelectorAll('li[data-task]')].map(item => [item.getAttribute('data-task'), item.textContent!.replace(/\s+/g, '')])).toEqual([['done', '☑已完成：读代码'], ['open', '☐未完成：写测试']]);
    expect(view.querySelector('input')).toBeNull();
    expect(view.querySelector('.md-table th:nth-child(2)')?.getAttribute('style')).toContain('center');
    expect(view.querySelector('blockquote')?.textContent?.trim()).toBe('注意');
    expect((await render('~~旧~~ 新')).querySelector('del')?.textContent).toBe('旧');
  });

  it('keeps a long code line intact and copies it exactly', async () => {
    const writeText = clipboard();
    const line = `const x = "${'很长'.repeat(5000)}";`;
    const view = await render(`\`\`\`ts\n${line}\n\`\`\``);
    expect(view.querySelector('.md-code-bar')?.textContent).toContain('ts');
    expect(view.querySelector('pre code')!.textContent).toBe(line);
    await act(async () => { button('复制').click(); });
    expect(writeText).toHaveBeenCalledWith(line);
    expect(view.querySelector('.md-code .md-copy-note')?.textContent).toBe('已复制');
  });

  it('confirms the exact address before it leaves, even if the text changes underneath', async () => {
    const writeText = clipboard();
    await render('看 [文档](https://example.test/guide)');
    expect(document.querySelector('dialog')).toBeNull();
    await act(async () => { button('文档').click(); });
    expect(document.querySelector('dialog')?.textContent).toContain('https://example.test/guide');
    await render('看 [文档](https://evil.test/)');
    expect(document.querySelector('dialog')?.textContent).toContain('https://example.test/guide');
    await act(async () => { button('复制链接').click(); });
    expect(writeText).toHaveBeenCalledWith('https://example.test/guide');
    await act(async () => { button('关闭').click(); });
    expect(document.querySelector('dialog')).toBeNull();
  });
});

describe('opening links in the desktop app', () => {
  const dialog = () => document.querySelector('dialog');
  it('opens only the confirmed snapshot, once, with the default app', async () => {
    opener.native = true;
    await render('看 [文档](https://example.test/guide)');
    await act(async () => { button('文档').click(); });
    expect(dialog()?.textContent).toContain('确认后用默认的浏览器打开。');
    await act(async () => { button('关闭').click(); });
    expect(opener.openUrl).not.toHaveBeenCalled();
    await act(async () => { button('文档').click(); });
    // The reply keeps streaming and its link changes; the sheet still opens what was confirmed.
    await render('看 [文档](https://evil.test/)');
    await act(async () => { button('用浏览器打开').click(); });
    expect(opener.openUrl.mock.calls).toEqual([['https://example.test/guide']]);
    expect(dialog()).toBeNull();
  });

  it('reports a failure without retrying and keeps the address copyable', async () => {
    opener.native = true;
    opener.openUrl.mockRejectedValueOnce(new Error('no handler'));
    const writeText = clipboard();
    await render('[写信](mailto:alex@example.test)');
    await act(async () => { button('写信').click(); });
    expect(dialog()?.textContent).toContain('确认后用默认的邮件应用打开。');
    await act(async () => { button('用邮件应用打开').click(); });
    expect(dialog()?.querySelector('[role="alert"]')?.textContent).toBe('没能打开邮件应用，可以复制地址后手动打开。');
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 20)); });
    expect(opener.openUrl).toHaveBeenCalledTimes(1);
    await act(async () => { button('复制链接').click(); });
    expect(writeText).toHaveBeenCalledWith('mailto:alex@example.test');
  });

  it('offers only copying outside the desktop app', async () => {
    await render('[文档](https://example.test/guide)');
    await act(async () => { button('文档').click(); });
    expect([...dialog()!.querySelectorAll('button')].map(item => item.textContent)).toEqual(['关闭', '复制链接']);
    expect(opener.openUrl).not.toHaveBeenCalled();
  });
});

// jsdom lays out every element synchronously and slow CI hosts take seconds; these check results, not speed.
const SLOW = 30_000;
const LONG = Array.from({ length: 1200 }, (_, i) => i % 12 === 0 ? `## 第 ${i} 节` : `- 第 **${i}** 项，含 \`code\` 和 [链接](https://example.test/${i})`).join('\n');
const FILLER = `${'x'.repeat(200)}\n\n`.repeat(60);

describe('streaming', () => {
  it('ends as a one-shot parse whatever the chunking, including late link definitions, fences and lists', async () => {
    const text = '[手册][r] 在前\n\n```js\nconst a = 1;\n\nconst b = 2;\n```\n\n- 一\n- 二\n\n  续写\n- 三\n\n[r]: https://example.test/guide\n';
    const once = (await fresh(text)).innerHTML;
    expect(once).toContain('md-link');
    for (const size of [1, 3, 7, 16]) {
      await fresh(text.slice(0, size));
      for (let end = size; end < text.length + size; end += size) await render(text.slice(0, end));
      expect(element.querySelector('.markdown')!.innerHTML).toBe(once);
    }
  }, SLOW);

  it('keeps the elements of finished blocks while the reply grows, and links a reference once it is defined', async () => {
    const head = '# 标题\n\n第一段 [r]\n\n';
    const view = await render(`${head}第二段正在`);
    const [title, first] = [view.querySelector('h3'), view.querySelector('p')];
    await render(`${head}第二段正在输出`);
    expect(view.querySelector('h3')).toBe(title);
    expect(view.querySelector('p')).toBe(first);
    expect(view.textContent).toContain('第二段正在输出');
    await render(`${head}第二段正在输出\n\n[r]: https://example.test/`);
    expect(view.querySelector('p .md-link')?.textContent).toBe('r');
  });

  it('renders a 64 KiB reply', async () => {
    expect(new TextEncoder().encode(LONG).byteLength).toBeGreaterThan(65536);
    const view = await render(LONG);
    expect(view.querySelectorAll('.md-link')).toHaveLength(1100);
  }, SLOW);
});

describe('long replies', () => {
  it('cut only where a top-level block starts, and growing never moves an earlier cut', () => {
    const parts = pieces(LONG);
    expect(parts.length).toBeGreaterThan(8);
    expect(parts.join('')).toBe(LONG);
    expect(parts.slice(1).every(part => part.startsWith('## '))).toBe(true);
    expect(parts.every(part => part.length < PIECE * 2)).toBe(true);
    for (let end = 1; end <= LONG.length; end += 997) {
      const grown = pieces(LONG.slice(0, end));
      expect(grown.slice(0, -1)).toEqual(parts.slice(0, grown.length - 1));
    }
  });

  it('never cut inside a fence or before a list item, and stay whole where a cut could change the meaning', () => {
    const fence = `~~~~\n${'code\n\nmore\n'.repeat(400)}~~~\n~~~~~\n`;
    const parts = pieces(`${FILLER}${fence}\n- a\n\n- b\n\n${FILLER}`);
    expect(parts.length).toBeGreaterThan(2);
    expect(parts.some(part => part.includes(fence))).toBe(true);
    expect(parts.some(part => /^(?:code|more|- )/.test(part))).toBe(false);
    expect(pieces(FILLER).length).toBeGreaterThan(1);
    for (const tail of ['[r]: https://example.test/', '[^1]: 注', '<!--\n\nx\n\n-->', '- a\n  ```\n  b\n  ```'])
      expect(pieces(`${FILLER}${tail}\n${FILLER}`)).toHaveLength(1);
    // A lone CR ends a line too, so the fence it opens is seen.
    expect(pieces(`${'x'.repeat(PIECE)}\nlead\r\`\`\`\n# 代码\n\`\`\`\n`)).toHaveLength(1);
    expect(pieces(FILLER.replace(/\n/g, '\r')).length).toBeGreaterThan(1);
  });

  it('render the pieces exactly as the whole text, with any line ending', async () => {
    const section = (i: number) => `## 第 ${i} 节\n\n段落 **${i}** 有 [链接](https://example.test/${i}) 和 \`code\`。\n续行\n\n- 一\n- 二\n\n  续写\n- 三\n\n` +
      `3. 从三开始\n4. 下一项\n\n> 引用\n继续\n\n| a | b |\n| - | - |\n| ${i} | x |\n\n\`\`\`js\nconst a = ${i};\n\n# 不是标题\n\`\`\`\n\n标题\n===\n\n***\n- [x] 完成\n- [ ] 未完成\nlead\r\`\`\`\n# 代码 [链接](https://example.test/)\n\`\`\`\n`;
    const flat = (html: string) => html.replace(/>\s+</g, '><');
    for (const end of ['\n', '\r\n', '\r']) {
      const mixed = Array.from({ length: 24 }, (_, i) => section(i)).join('\n').replace(/\n/g, end);
      expect(pieces(mixed).length).toBeGreaterThan(1);
      const cut = flat((await fresh(mixed)).innerHTML);
      // An unused definition renders nothing and keeps the text whole.
      expect(cut).toBe(flat((await fresh(`${mixed}${end}[unused]: https://example.test/${end}`)).innerHTML));
    }
  }, SLOW);

  it('link an early reference once its definition arrives', async () => {
    const head = `[手册][r] 在前\n\n${FILLER}`;
    expect(pieces(head).length).toBeGreaterThan(1);
    await fresh(head);
    expect(element.querySelector('.markdown p')!.textContent).toBe('[手册][r] 在前');
    await render(`${head}[r]: https://example.test/guide\n`);
    expect(element.querySelector('.markdown p .md-link')?.textContent).toBe('手册');
  });

  it('parse each piece once, and while streaming only the piece that grew', async () => {
    const start = parser.parses;
    await fresh(LONG);
    expect(parser.parses - start).toBe(pieces(LONG).length);
    const grown = parser.parses;
    await render(`${LONG}\n- 追加`);
    expect(parser.parses - grown).toBe(1);
    expect(element.querySelectorAll('.md-link')).toHaveLength(1100);
  }, SLOW);

  it('commit only the first piece at once and lay out the rest in the background', async () => {
    element = document.createElement('div'); document.body.append(element); root = createRoot(element);
    let now = 0;
    // Read between the urgent commit and the background render that act would flush at its end.
    act(() => { flushSync(() => root!.render(thread(LONG))); now = element.querySelectorAll('.md-link').length; });
    expect(now).toBeGreaterThan(0);
    expect(now).toBeLessThan(1100);
    expect(element.querySelectorAll('.md-link')).toHaveLength(1100);
  }, SLOW);
});
