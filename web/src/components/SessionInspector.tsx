import { useEffect, useState } from 'react';
import type { SessionMode } from '../protocol/generated/SessionMode';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { DaemonClient, DaemonView } from '../state/DaemonClient';
import { errorMessage, stateLabels, stopReasonLabels } from '../app/copy';
import { ToolApprovalMark } from './StatePill';

const modeLabels: Record<SessionMode, string> = { new: '新会话', resume: '继续自之前的会话', attach: '接入本机已在运行的会话' };
const OPEN_KEY = 'cafe-space.inspector.session';

/** The reader's last choice wins; otherwise open only where there is room beside the conversation. */
function initiallyOpen() {
  try {
    const saved = localStorage.getItem(OPEN_KEY);
    if (saved) return saved === 'open';
  } catch { /* storage unavailable */ }
  return window.matchMedia?.('(min-width: 1200px)').matches ?? true;
}

/** Session details from existing protocol fields only (UX §5.3 Inspector); the project path only at this computer. */
export function SessionInspector({ client, state, session, local }: { client: DaemonClient; state: DaemonView; session: SessionSnapshot; local: boolean }) {
  const [open, setOpen] = useState(initiallyOpen);
  const runtime = state.runtimes?.find(item => item.id === session.runtimeId) ?? null;
  const project = state.projects?.find(item => item.id === session.projectId) ?? null;
  const root = project && state.roots[project.id]?.cwdFence === project.cwdFence ? state.roots[project.id].root : null;
  const connected = state.connection === 'connected';
  // Read once per connection and project generation; a failed read is not retried.
  const generation = project ? `${project.id}\u0000${project.cwdFence}` : null;
  useEffect(() => {
    if (local && connected && project && root === null) void client.loadProjectRoot(project.id);
  }, [client, local, connected, generation]);

  const now = session.state;
  return <details className="inspector" open={open} onToggle={event => {
    // Rendering the default also fires a toggle; only the reader's own change is kept.
    const next = event.currentTarget.open;
    if (next === open) return;
    setOpen(next);
    try { localStorage.setItem(OPEN_KEY, next ? 'open' : 'closed'); } catch { /* storage unavailable */ }
  }}>
    <summary>会话详情</summary>
    <dl>
      <div><dt>Agent</dt><dd>{runtime?.name ?? session.runtimeId}</dd></div>
      <div><dt>已装版本</dt><dd>{runtime?.version ?? '未报告'}</dd></div>
      <div><dt>项目</dt><dd>{project?.name ?? '已移除的项目'}</dd></div>
      {local && project && <div><dt>路径</dt><dd>{root ? <code>{root}</code> : '未读取'}</dd></div>}
      <div><dt>开始方式</dt><dd>{modeLabels[session.mode]}</dd></div>
      <div><dt>工具审批</dt><dd><ToolApprovalMark value={session.capabilities.toolApproval} /></dd></div>
      <div><dt>状态</dt><dd>{stateLabels[now.state]}{now.state === 'stopped' && session.stopReason && ` · ${stopReasonLabels[session.stopReason]}`}{now.state === 'failed' && ` · ${errorMessage(now.data.code)}`}</dd></div>
      {now.state === 'failed' && <div><dt>错误代码</dt><dd><code>{now.data.code}</code></dd></div>}
      <div><dt>模型</dt><dd>{session.model ? `${session.model.provider} / ${session.model.id}` : '未报告'}</dd></div>
    </dl>
  </details>;
}
