import { useEffect, useMemo, useRef, useState } from 'react';
import { open as pickFolder } from '@tauri-apps/plugin-dialog';
import type { CreateSessionParams } from '../protocol/generated/CreateSessionParams';
import type { ProjectSummary } from '../protocol/generated/ProjectSummary';
import type { DaemonClient, DaemonView } from '../state/DaemonClient';
import { outcomeOf, toolApprovalNotes, type Outcome } from '../app/copy';
import { OutcomeMark, ToolApprovalMark } from './StatePill';
import { agentRows, missingNote } from './Agents';
import { sessionTitle } from './SessionList';

const MAX_ROOT_READS = 100;

/** Where the session runs: a recorded folder, the home folder (first use on this computer) or a newly picked one. */
type Place = { kind: 'project'; id: string } | { kind: 'home' } | { kind: 'picked'; root: string };

/** Recorded folders, the one with the latest session activity first (M4b #110). */
function byRecentUse(projects: readonly ProjectSummary[], sessions: DaemonView['sessions']) {
  const latest = new Map<string, number>();
  for (const session of sessions ?? []) {
    const at = session.lastActivityAt ?? 0;
    if (at >= (latest.get(session.projectId) ?? -1)) latest.set(session.projectId, at);
  }
  return [...projects].sort((a, b) => (latest.get(b.id) ?? -1) - (latest.get(a.id) ?? -1));
}

/**
 * UX §5.2 in one step. The client-chosen session ID stays the same while an outcome is unknown,
 * so asking again can only confirm the same session, never start a second one.
 */
export function NewSessionSheet({ client, state, local, open, onClose, onCreated }: {
  client: DaemonClient; state: DaemonView; local: boolean; open: boolean; onClose: () => void; onCreated: (sessionId: string) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const projects = useMemo(() => byRecentUse(state.projects ?? [], state.sessions), [state.projects, state.sessions]);
  const rows = useMemo(() => agentRows(state.runtimes ?? []).sort((a, b) => Number(!!b.runtime?.available) - Number(!!a.runtime?.available)), [state.runtimes]);
  const [place, setPlace] = useState<Place | null>(null);
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
    // Opening only reads: the folder is recorded when Start is pressed, never on opening, picking or cancelling.
    setPlace(projects[0] ? { kind: 'project', id: projects[0].id } : local ? { kind: 'home' } : null);
  }, [open, client, local]);

  const projectId = place?.kind === 'project' ? place.id : null;
  const project = projects.find(item => item.id === projectId) ?? null;
  const runtime = state.runtimes?.find(item => item.id === runtimeId && item.available) ?? null;
  const candidates = (state.sessions ?? []).filter(item => item.projectId === projectId && item.runtimeId === runtimeId &&
    (item.state.state === 'stopped' || item.state.state === 'failed'));
  // Confirming an unknown create reuses its exact request, whether or not the new folder has reached the list yet.
  const ready = attempt !== null || ((project || (local && place && place.kind !== 'project')) && runtime && (mode === 'new' || candidates.some(item => item.id === sourceSessionId)));
  const changeInput = (change: () => void) => { change(); setAttempt(null); setOutcome(null); };
  const [unrecorded, setUnrecorded] = useState(false);

  async function choose() {
    const picked = await pickFolder({ directory: true, multiple: false }).catch(() => null);
    if (typeof picked !== 'string') return;
    // A folder already recorded is chosen as itself, so Start needs no extra request.
    const known = projects.find(item => state.roots[item.id]?.cwdFence === item.cwdFence && state.roots[item.id].root === picked);
    changeInput(() => { setPlace(known ? { kind: 'project', id: known.id } : { kind: 'picked', root: picked }); setSourceSessionId(null); });
  }

  async function submit() {
    if ((!attempt && (!place || !runtime)) || busy) return;
    setBusy(true); setOutcome(null); setUnrecorded(false);
    // Keep the same ID while the previous outcome is unknown; any definitive answer allows a new one.
    let params = attempt;
    if (!params) {
      let target: ProjectSummary | null = project;
      if (!target) {
        // Only this computer records a folder, once, when Start is pressed (local-session-folder contract).
        const resolved = await client.call({ method: 'resolve_local_session_folder', params: place!.kind === 'picked' ? { root: place!.root } : {} });
        if (resolved.status !== 'applied' || resolved.data.result.kind !== 'local_session_folder') {
          setBusy(false);
          const failed = outcomeOf(resolved);
          setOutcome(failed);
          setUnrecorded(failed.kind === 'unknown');
          return;
        }
        target = resolved.data.result.data.project;
      }
      params = {
        sessionId: crypto.randomUUID(), projectId: target.id, runtimeId: runtime!.id, expectedCwdFence: target.cwdFence,
        start: mode === 'new' ? { mode: 'new' } : { mode: 'resume', params: { sourceSessionId: sourceSessionId! } },
      };
      setAttempt(params);
    }
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
        <legend>文件夹</legend>
        {projects.length === 0 && !local ? <p className="field-note">在电脑上开始第一个会话后，这里可以选择它的文件夹。</p> :
          <div className="choice-list">
            {projects.map(item => <label key={item.id} className="choice">
              <input type="radio" name="project" value={item.id} checked={projectId === item.id} onChange={() => changeInput(() => { setPlace({ kind: 'project', id: item.id }); setSourceSessionId(null); })} />
              <span className="choice-title">{item.name}</span>
              {local && state.roots[item.id]?.cwdFence === item.cwdFence && <span className="choice-path">{state.roots[item.id].root}</span>}
            </label>)}
            {local && projects.length === 0 && <label className="choice">
              <input type="radio" name="project" value="home" checked={place?.kind === 'home'} onChange={() => changeInput(() => setPlace({ kind: 'home' }))} />
              <span className="choice-title">主目录</span>
            </label>}
            {place?.kind === 'picked' && <label className="choice">
              <input type="radio" name="project" value="picked" checked readOnly />
              <span className="choice-title">{place.root.split(/[\\/]/).filter(Boolean).pop() ?? place.root}</span>
              <span className="choice-path">{place.root}</span>
            </label>}
          </div>}
        {local && <div className="project-add-row"><button type="button" className="secondary-button" disabled={busy} onClick={() => void choose()}>选择其他文件夹…</button></div>}
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
        {mode === 'resume' && (candidates.length === 0 ? <p className="field-note">这个文件夹里没有这个 Agent 可以继续的会话。</p> :
          <select aria-label="要继续的会话" value={sourceSessionId ?? ''} onChange={event => changeInput(() => setSourceSessionId(event.target.value || null))}>
            <option value="">选择一个会话</option>
            {candidates.map(item => <option key={item.id} value={item.id}>{sessionTitle(item, state.runtimes)}</option>)}
          </select>)}
      </fieldset>
      <OutcomeMark outcome={outcome} />
      {outcome?.kind === 'unknown' && <p className="field-note">{unrecorded ? '没能确认文件夹是否已记录。' : '没能确认会话是否已创建。'}</p>}
      <div className="sheet-actions">
        <button type="button" className="secondary-button" onClick={() => dialog.current?.close()}>取消</button>
        <button type="submit" className="primary-button" disabled={!ready || busy || state.connection !== 'connected'}>{outcome?.kind === 'unknown' && !unrecorded ? '再次确认' : '开始'}</button>
      </div>
    </form>
  </dialog>;
}
