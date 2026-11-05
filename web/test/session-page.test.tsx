import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import { DaemonClient } from '../src/state/DaemonClient';
import { SessionPage } from '../src/app/SessionPage';

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
const chunk = vi.hoisted(() => ({ fail: true }));
vi.mock('../src/components/SessionView', async importOriginal => {
  if (chunk.fail) throw new Error('chunk unavailable');
  return importOriginal();
});

it('says the session page did not load and loads it again on retry', async () => {
  vi.spyOn(console, 'error').mockImplementation(() => {});
  const client = new DaemonClient(() => { throw new Error('unused'); });
  const state = { ...client.getSnapshot(), session: { id: 's1', detail: null, loading: false, error: null, removed: true } };
  const element = document.createElement('div'); document.body.append(element);
  const root = createRoot(element);
  // Loading the real chunk (assistant-ui included) takes a while under the test transformer.
  const settle = async (text: string) => { for (let i = 0; i < 100 && !element.textContent?.includes(text); i++) await act(async () => { await new Promise(resolve => setTimeout(resolve, 50)); }); };
  await act(async () => root.render(<SessionPage client={client} state={state} sessionId="s1" onNavigate={() => {}} />));
  await settle('会话页没能加载');
  expect(element.textContent).toContain('会话页没能加载');
  chunk.fail = false;
  await act(async () => { [...element.querySelectorAll('button')].find(item => item.textContent === '重试')!.click(); });
  await settle('这个会话已不存在');
  expect(element.textContent).toContain('这个会话已不存在');
  await act(async () => root.unmount());
});
