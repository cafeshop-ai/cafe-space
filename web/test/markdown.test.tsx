import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { Lexer } from 'marked';
import { MAX_DEPTH, Markdown, decodeEntities, lexMarkdown, safeUrl } from '../src/components/Markdown';

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
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
});
async function render(text: string) {
  if (!root) { element = document.createElement('div'); document.body.append(element); root = createRoot(element); }
  await act(async () => root!.render(<Markdown text={text} />));
  return element;
}
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
    expect(safeUrl('HTTPS://Example.test/a?b=1&amp;c=2')).toBe('https://example.test/a?b=1&c=2');
    expect(safeUrl('mailto:alex@example.test')).toBe('mailto:alex@example.test');
    const view = await render(`${unsafe.slice(0, 3).map((href, i) => `[x${i}](${href})`).join(' ')} [ok](https://example.test/a) <https://example.test/b> www.example.test [mail](mailto:alex@example.test)`);
    expect(view.querySelector('a')).toBeNull();
    expect([...view.querySelectorAll('.md-link')].map(item => item.textContent)).toEqual(['ok', 'https://example.test/b', 'www.example.test', 'mail']);
    expect(view.textContent).toContain('x0 javascript:alert(1)');
  });

  it('decodes entities once outside code and keeps code literal', async () => {
    expect(decodeEntities('a &amp; b &lt;c&gt; &quot;q&quot; &#x3C; &#169; &amp;amp; &copy; &#0;')).toBe('a & b <c> "q" < © &amp; &copy; �');
    expect(decodeEntities('&constructor; &toString; &hasOwnProperty; &__proto__;')).toBe('&constructor; &toString; &hasOwnProperty; &__proto__;');
    const view = await render('a &amp; b &lt;c&gt; `&amp;` **&lt;b&gt;**\n\n```\n&amp; <i>\n```');
    expect(view.querySelector('p')!.textContent).toBe('a & b <c> &amp; <b>');
    expect(view.querySelector('pre code')!.textContent).toBe('&amp; <i>');
  });

  it('never loads images', async () => {
    const view = await render('![logo](https://example.test/logo.png)');
    expect(view.querySelector('img')).toBeNull();
    expect(view.textContent).toBe('[图片：logo] https://example.test/logo.png');
  });

  it('limits nesting while rendering and before lexing', async () => {
    let view = await render(`${'> '.repeat(MAX_DEPTH + 4)}深处`);
    expect(view.querySelectorAll('blockquote')).toHaveLength(MAX_DEPTH);
    expect(view.querySelector('.md-source')?.textContent).toContain('> > > > 深处');
    view = await render(Array.from({ length: MAX_DEPTH + 4 }, (_, i) => `${'  '.repeat(i)}- 第 ${i} 层`).join('\n'));
    expect(view.querySelectorAll('ul')).toHaveLength(MAX_DEPTH);
    expect(view.textContent).toContain('第 11 层');
    // Inputs that slow the lexer sharply or overflow it are shown as written without lexing.
    const lex = vi.spyOn(Lexer.prototype, 'lex');
    for (const text of ['>'.repeat(30000) + ' x', '- '.repeat(5000) + 'x', Array.from({ length: 1000 }, (_, i) => `${'  '.repeat(i)}- a`).join('\n')]) {
      expect(lexMarkdown(text).tokens).toEqual([{ type: 'source', raw: text }]);
    }
    expect(lex).not.toHaveBeenCalled();
    lex.mockRestore();
  });
});

describe('markdown rendering', () => {
  it('renders headings, plan checklists, tables and quotes', async () => {
    const view = await render('# 计划\n\n- [x] 读代码\n- [ ] 写测试\n\n| 文件 | 状态 |\n|---|:-:|\n| a.ts | 完成 |\n\n> 注意');
    expect(view.querySelector('h3')?.textContent).toBe('计划');
    expect([...view.querySelectorAll('li[data-task]')].map(item => [item.getAttribute('data-task'), item.textContent])).toEqual([['done', '☑已完成：读代码'], ['open', '☐未完成：写测试']]);
    expect(view.querySelector('.md-table th:nth-child(2)')?.getAttribute('style')).toContain('center');
    expect(view.querySelector('blockquote')?.textContent).toBe('注意');
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

describe('streaming', () => {
  const full = (text: string) => JSON.stringify(lexMarkdown(text).tokens);
  it('ends as a one-shot parse whatever the chunking, including late link definitions, fences and lists', () => {
    const text = '[手册][r] 在前\n\n```js\nconst a = 1;\n\nconst b = 2;\n```\n\n- 一\n- 二\n\n  续写\n- 三\n\n[r]: https://example.test/guide\n';
    for (const size of [1, 3, 7, 16]) {
      let lexed;
      for (let end = size; end < text.length + size; end += size) lexed = lexMarkdown(text.slice(0, end), lexed);
      expect(JSON.stringify(lexed!.tokens)).toBe(full(text));
    }
    expect(lexMarkdown(text).tokens[0]).toMatchObject({ type: 'paragraph', tokens: [{ type: 'link', href: 'https://example.test/guide' }, expect.anything()] });
  });

  it('reuses unchanged blocks so only the growing one renders again, and drops reuse when definitions change', () => {
    const head = '# 标题\n\n第一段 [r]\n\n';
    const first = lexMarkdown(`${head}第二段正在`);
    const next = lexMarkdown(`${head}第二段正在输出`, first);
    expect(next.tokens[0]).toBe(first.tokens[0]);
    expect(next.tokens[2]).toBe(first.tokens[2]);
    expect(next.tokens.at(-1)).not.toBe(first.tokens.at(-1));
    const defined = lexMarkdown(`${head}第二段正在输出\n\n[r]: https://example.test/`, next);
    expect(defined.tokens[2]).not.toBe(next.tokens[2]);
  });

  it('lexes a 64 KiB reply', async () => {
    const text = Array.from({ length: 1200 }, (_, i) => i % 12 === 0 ? `## 第 ${i} 节` : `- 第 **${i}** 项，含 \`code\` 和 [链接](https://example.test/${i})`).join('\n');
    expect(new TextEncoder().encode(text).byteLength).toBeGreaterThan(65536);
    const view = await render(text);
    expect(view.querySelectorAll('.md-link')).toHaveLength(1100);
  });
});
