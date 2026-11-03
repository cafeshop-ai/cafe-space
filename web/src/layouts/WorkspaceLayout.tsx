import type { ReactNode } from 'react';

// Adapted from v0 WorkspaceLayout at 67237383; M0 omits the unsupported Files panel.
export function WorkspaceLayout({ header, sidebar, children }: {
  header: ReactNode; sidebar: ReactNode; children: ReactNode;
}) {
  return <div className="workspace-layout">
    <header className="workspace-toolbar">{header}</header>
    <div className="workspace-grid">
      <aside className="workspace-sidebar" aria-label="工作区导航">{sidebar}</aside>
      <main id="main-content" className="workspace-center" tabIndex={-1}>{children}</main>
    </div>
  </div>;
}
