import { useEffect, useRef, useState } from 'react';
import type { ProjectSummary } from '../protocol/generated/ProjectSummary';
import type { DaemonClient, DaemonView } from '../state/DaemonClient';
import { errorMessage, outcomeOf, type Outcome } from '../app/copy';
import { OutcomeMark } from './StatePill';

/**
 * Approved project folders, managed only at this computer (UX §5, M1-2). The daemon rejects these
 * requests from any remote connection, so this panel is shown only in the local desktop app.
 */
export function ProjectSettings({ client, state }: { client: DaemonClient; state: DaemonView }) {
  const projects = state.projects ?? [];
  const connected = state.connection === 'connected';
  const [path, setPath] = useState('');
  const [addOutcome, setAddOutcome] = useState<Outcome | null>(null);
  const [busy, setBusy] = useState(false);
  // Each project generation is read at most once, so a failing read cannot become a request loop.
  const attempted = useRef(new Set<string>());
  useEffect(() => {
    if (!connected) return;
    for (const project of projects) {
      const key = `${project.id}\u0000${project.cwdFence}`;
      if (state.roots[project.id]?.cwdFence === project.cwdFence || attempted.current.has(key)) continue;
      attempted.current.add(key);
      void client.loadProjectRoot(project.id);
    }
  }, [client, connected, projects, state.roots]);

  async function add() {
    if (!path.trim() || busy) return;
    setBusy(true); setAddOutcome(null);
    const outcome = outcomeOf(await client.call({ method: 'add_project', params: { root: path.trim() } }));
    setBusy(false); setAddOutcome(outcome);
    if (outcome.kind === 'done') setPath('');
  }

  return <section className="project-settings" aria-labelledby="projects-title">
    <h2 id="projects-title">项目</h2>
    <p className="field-note">Agent 只能在这里批准的文件夹里开始会话。这只决定会话从哪里开始，不是沙箱：Agent 能做什么由它自己的权限决定。</p>
    {projects.length > 0 && <ul className="project-list">{projects.map(project =>
      <ProjectRow key={project.id} client={client} project={project} root={state.roots[project.id]?.cwdFence === project.cwdFence ? state.roots[project.id].root : null} connected={connected} />)}</ul>}
    <form className="project-add" onSubmit={event => { event.preventDefault(); void add(); }}>
      <label htmlFor="project-path">文件夹的完整路径</label>
      <div className="project-add-row">
        <input id="project-path" value={path} onChange={event => setPath(event.target.value)} placeholder="/Users/alex/projects/cafe 或 C:\\Users\\alex\\cafe" spellCheck={false} autoComplete="off" />
        <button type="submit" className="primary-button" disabled={!path.trim() || busy || !connected}>批准文件夹</button>
      </div>
      <OutcomeMark outcome={addOutcome} />
    </form>
  </section>;
}

function ProjectRow({ client, project, root, connected }: { client: DaemonClient; project: ProjectSummary; root: string | null; connected: boolean }) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(project.name);
  const [confirmRemove, setConfirmRemove] = useState(false);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const fence = { projectId: project.id, expectedCwdFence: project.cwdFence };

  async function rename() {
    if (!name.trim() || name === project.name) { setEditing(false); return; }
    const next = outcomeOf(await client.call({ method: 'update_project', params: { ...fence, name: name.trim(), root: null } }));
    setOutcome(next);
    if (next.kind === 'done') setEditing(false);
  }
  // Re-approving the same path accepts the folder's current identity and rotates the fence.
  async function reapprove() {
    if (!root) return;
    setOutcome(outcomeOf(await client.call({ method: 'update_project', params: { ...fence, name: null, root } })));
  }
  async function remove() {
    setConfirmRemove(false);
    setOutcome(outcomeOf(await client.call({ method: 'remove_project', params: fence })));
  }

  const changed = outcome?.kind === 'not_done' && outcome.code === 'PROJECT_ROOT_CHANGED';
  return <li className="project-row">
    <div className="project-main">
      {editing ? <form onSubmit={event => { event.preventDefault(); void rename(); }} className="project-rename">
        <label className="visually-hidden" htmlFor={`name-${project.id}`}>项目名称</label>
        <input id={`name-${project.id}`} value={name} onChange={event => setName(event.target.value)} autoFocus />
        <button type="submit" className="secondary-button" disabled={!connected}>保存</button>
        <button type="button" className="quiet-button" onClick={() => { setName(project.name); setEditing(false); }}>取消</button>
      </form> : <span className="project-name">{project.name}</span>}
      <span className="choice-path">{root ?? '正在读取路径…'}</span>
    </div>
    <div className="project-actions">
      {!editing && <button className="quiet-button" disabled={!connected} onClick={() => setEditing(true)}>改名</button>}
      {changed && <button className="secondary-button" disabled={!connected || !root} onClick={() => void reapprove()}>重新批准</button>}
      {confirmRemove ? <span className="inline-confirm">移除后不能再在这里开始会话，历史会保留。
        <button className="secondary-button" onClick={() => void remove()}>移除项目</button>
        <button className="quiet-button" onClick={() => setConfirmRemove(false)}>取消</button></span> :
        <button className="quiet-button" disabled={!connected} onClick={() => setConfirmRemove(true)}>移除</button>}
    </div>
    <OutcomeMark outcome={outcome} />
    {changed && <p className="field-note">{errorMessage('PROJECT_ROOT_CHANGED')}确认这个路径现在就是你要的文件夹后，再重新批准。</p>}
  </li>;
}
