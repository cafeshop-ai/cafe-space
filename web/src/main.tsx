import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './styles.css';

const root = document.getElementById('root');
if (!root) throw new Error('Missing application root');

createRoot(root).render(
  <StrictMode>
    <main aria-labelledby="title">
      <h1 id="title">Café Space</h1>
      <p>守护进程连接与会话界面将在 M0-4 接入。</p>
    </main>
  </StrictMode>,
);
