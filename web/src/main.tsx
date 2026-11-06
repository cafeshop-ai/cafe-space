import { createRoot } from 'react-dom/client';
import { App } from './app/App';
import { invoke } from '@tauri-apps/api/core';
import './styles.css';

const root = document.getElementById('root');
if (!root) throw new Error('Missing application root');

if ((window as Window & { __CAFE_SPACE_M1_PROBE__?: boolean }).__CAFE_SPACE_M1_PROBE__ === true) {
  void import('./probe/host').then(async ({ runHostProbe }) => {
    await runHostProbe();
    await invoke('m1_finish', { success: true });
  }).catch(() => invoke('m1_finish', { success: false }));
} else {
  createRoot(root).render(<App />);
}
