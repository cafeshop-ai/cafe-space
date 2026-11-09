import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ThreadProvider, Transcript, loadMarkdown } from '../src/components/SessionThread';
import type { TranscriptMessage } from '../src/protocol/generated/TranscriptMessage';

// The Markdown chunk arrives only when a test lets it through, and fails while `fail` is set.
const chunk = vi.hoisted(() => ({ gate: Promise.resolve(), fail: false }));
vi.mock('../src/components/Markdown', async original => {
  await chunk.gate;
  if (chunk.fail) throw new Error('chunk unavailable');
  return original();
});
const hold = () => { let release!: () => void; chunk.gate = new Promise(resolve => { release = resolve; }); return release; };

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
const source = '# 计划\n\n**先**读代码，再跑 `vitest`。\n\n  全文照原样显示。\n';
let root: Root | undefined;
afterEach(async () => {
  await act(async () => root?.unmount());
  root = undefined;
  document.body.replaceChildren();
});
async function render() {
  const element = document.createElement('div');
  document.body.append(element);
  root = createRoot(element);
  const reply = { id: 'm1', role: 'assistant', status: 'complete', parts: [{ kind: 'text', data: { text: source } }] } satisfies TranscriptMessage;
  await act(async () => root!.render(<ThreadProvider messages={[reply]} pending={[]}><Transcript agent="Pi" /></ThreadProvider>));
  return element;
}

describe('the Markdown chunk', () => {
  it('keeps a reply as its full source text when the chunk fails to load', async () => {
    chunk.fail = true;
    const release = hold();
    const view = await render();
    await act(async () => { release(); await loadMarkdown(); });
    expect(view.querySelector('.message .markdown > .md-source')?.textContent).toBe(source);
    expect(view.querySelector('h3, strong, code')).toBeNull();
  });

  it('shows the source text first, then the rendered reply in the same message', async () => {
    chunk.fail = false;
    const release = hold();
    const view = await render();
    const message = view.querySelector('.message');
    expect(view.querySelector('.message .markdown > .md-source')?.textContent).toBe(source);
    await act(async () => { release(); await loadMarkdown(); });
    expect(view.querySelector('.message')).toBe(message);
    expect(view.querySelector('.message .markdown h3')?.textContent).toBe('计划');
    expect(view.querySelector('.markdown strong')?.textContent).toBe('先');
    expect(view.querySelector('.md-source')).toBeNull();
  });
});
