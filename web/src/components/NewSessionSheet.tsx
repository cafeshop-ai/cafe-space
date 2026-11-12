import { useEffect, useMemo, useRef, useState } from 'react';
import type { CreateSessionParams } from '../protocol/generated/CreateSessionParams';
import type { DaemonClient, DaemonView } from '../state/DaemonClient';
import { outcomeOf, toolApprovalNotes, type Outcome } from '../app/copy';
import { OutcomeMark, ToolApprovalMark } from './StatePill';
import { agentRows, missingNote } from './Agents';
import { sessionTitle } from './SessionList';

const MAX_ROOT_READS = 100;

/**
 * UX §5.2 in one step. The client-chosen session ID stays the same while an outcome is unknown,
 * so asking again can only confirm the same session, never start a second one.
 */
export function NewSessionSheet({ client, state, local, open, onClose, onCreated }: {
  client: DaemonClient; state: DaemonView; local: boolean; open: boolean; onClose: () => void; onCreated: (sessionId: string) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const projects = state.projects ?? [];
  const rows = useMemo(() => agentRows(state.runtimes ?? []).sort((a, b) => Number(!!b.runtime?.available) - Number(!!a.runtime?.available)), [state.runtimes]);
  const [projectId, setProjectId] = useState<string | null>(null);
  const [runtimeId, setRuntimeId] = useState<string | null>(null);
  const [mode, setMode] = useState<'new' | 'resume'>('new');
  const [sourceSessionId, setSourceSessionId] = useState<string | null>(null);
  const [attempt, setAttempt] = useState<CreateSessionParams | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const element = dialog.current;
    if (!element) return;
    if (open && !element.open) element.showModal();
    if (!open && element.open) element.close();
  }, [open]);
  useEffect(() => {
    if (!open) return;
    void client.loadRuntimes();
    // Paths are read only at this computer; a remote client shows labels alone.
    if (local) for (const project of projects.slice(0, MAX_ROOT_READS)) {
      if (state.roots[project.id]?.cwdFence !== project.cwdFence) void client.loadProjectRoot(project.id);
    }
    // Roots are read once per opening; a later fence change hides the stale path until the next opening.
  }, [open, client, local]);

  const project = projects.find(item => item.id === projectId) ?? null;
  const runtime = state.runtimes?.find(item => item.id === runtimeId && item.available) ?? null;
  const candidates = (state.sessions ?? []).filter(item => item.projectId === projectId && item.runtimeId === runtimeId &&
    (item.state.state === 'stopped' || item.state.state === 'failed'));
  const ready = project && runtime && (mode === 'new' || candidates.some(item => item.id === sourceSessionId));
  const changeInput = (change: () => void) => { change(); setAttempt(null); setOutcome(null); };

  async function submit() {
    if (!project || !runtime || busy) return;
    // Keep the same ID while the previous outcome is unknown; any definitive answer allows a new one.
    const params: CreateSessionParams = attempt ?? {
      sessionId: crypto.randomUUID(), projectId: project.id, runtimeId: runtime.id, expectedCwdFence: project.cwdFence,
      start: mode === 'new' ? { mode: 'new' } : { mode: 'resume', params: { sourceSessionId: sourceSessionId! } },
    };
    setAttempt(params); setBusy(true); setOutcome(null);
    const result = await client.call({ method: 'create_session', params });
    setBusy(false);
    const next = outcomeOf(result);
    setOutcome(next);
    if (next.kind === 'unknown') return;
    setAttempt(null);
    if (next.kind === 'done' || next.kind === 'sent') onCreated(params.sessionId);
  }

  return <dialog ref={dialog} className="sheet" aria-labelledby="new-session-title" onClose={onClose}>
    <form method="dialog" onSubmit={event => { event.preventDefault(); void submit(); }}>
      <h2 id="new-session-title">新建会话</h2>
      <fieldset>
        <legend>项目</legend>
        {projects.length === 0 ? <p className="field-note">还没有批准的项目。{local ?
          <>在 <a href="#/settings" onClick={() => dialog.current?.close()}>设置 · 项目</a> 里批准一个文件夹，或在本机运行 <code>cafe-space project add &lt;目录&gt;</code>。</> :
          '请在电脑上批准项目。'}</p> :
          <div className="choice-list">{projects.map(item => <label key={item.id} className="choice">
            <input type="radio" name="project" value={item.id} checked={projectId === item.id} onChange={() => changeInput(() => { setProjectId(item.id); setSourceSessionId(null); })} />
            <span className="choice-title">{item.name}</span>
            {local && state.roots[item.id]?.cwdFence === item.cwdFence && <span className="choice-path">{state.roots[item.id].root}</span>}
          </label>)}</div>}
      </fieldset>
      <fieldset>
        <legend>Agent</legend>
        {!state.runtimes ? <p className="field-note">正在检测…</p> :
          <div className="choice-list">{rows.map(({ id, name, runtime: item }, index) => <label key={id} className="choice" data-disabled={!item?.available || undefined}>
            <input type="radio" name="runtime" value={id} disabled={!item?.available} checked={runtimeId === id} onChange={() => changeInput(() => { setRuntimeId(id); setSourceSessionId(null); })} />
            <span className="choice-title">{name}{item?.version && <span className="choice-version"> {item.version}</span>}</span>
            <span className="choice-note">{item?.available ? <ToolApprovalMark value={item.toolApproval} plain /> : <>{missingNote(rows[index])}
              {local && id === 'pi' && <> <a href="#/settings" onClick={() => dialog.current?.close()}>到设置处理</a></>}</>}</span>
          </label>)}</div>}
        {runtime && <p className="field-note">{toolApprovalNotes[runtime.toolApproval]}</p>}
      </fieldset>
      <fieldset>
        <legend>开始方式</legend>
        <div className="choice-list">
          <label className="choice"><input type="radio" name="start" checked={mode === 'new'} onChange={() => changeInput(() => setMode('new'))} /><span className="choice-title">新会话</span></label>
          <label className="choice"><input type="radio" name="start" checked={mode === 'resume'} onChange={() => changeInput(() => setMode('resume'))} /><span className="choice-title">继续之前的会话</span></label>
          <label className="choice" data-disabled><input type="radio" name="start" disabled /><span className="choice-title">接入运行中的会话</span><span className="choice-path">暂不支持</span></label>
        </div>
        {mode === 'resume' && (candidates.length === 0 ? <p className="field-note">这个项目里没有这个 Agent 可以继续的会话。</p> :
          <select aria-label="要继续的会话" value={sourceSessionId ?? ''} onChange={event => changeInput(() => setSourceSessionId(event.target.value || null))}>
            <option value="">选择一个会话</option>
            {candidates.map(item => <option key={item.id} value={item.id}>{sessionTitle(item, state.runtimes)}</option>)}
          </select>)}
      </fieldset>
      <OutcomeMark outcome={outcome} />
      {outcome?.kind === 'unknown' && <p className="field-note">没能确认会话是否已创建。</p>}
      <div className="sheet-actions">
        <button type="button" className="secondary-button" onClick={() => dialog.current?.close()}>取消</button>
        <button type="submit" className="primary-button" disabled={!ready || busy || state.connection !== 'connected'}>{outcome?.kind === 'unknown' ? '再次确认' : '开始'}</button>
      </div>
    </form>
  </dialog>;
}
