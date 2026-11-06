import { useState } from 'react';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { RuntimeSummary } from '../protocol/generated/RuntimeSummary';
import type { DaemonClient, DaemonView } from '../state/DaemonClient';
import { thinkingLabels } from '../app/copy';
import { ChipMenu } from './ChipMenu';
import { useProjectRoots } from './ProjectSettings';
import { StatePill } from './StatePill';

export const sessionHref = (id: string) => `#/sessions/${encodeURIComponent(id)}`;
export const runtimeName = (runtimes: RuntimeSummary[] | null, id: string) => runtimes?.find(item => item.id === id)?.name ?? id;
export const sessionTitle = (session: SessionSnapshot, runtimes: RuntimeSummary[] | null) => session.title || `${runtimeName(runtimes, session.runtimeId)} 会话`;

const AGENT_KEY = 'cafe-space.sessions.agent';
const OPEN_KEY = 'cafe-space.sessions.open';
const load = (key: string) => { try { return localStorage.getItem(key); } catch { return null; } };
const save = (key: string, value: string) => { try { localStorage.setItem(key, value); } catch { /* storage unavailable */ } };
// A collapsed project stands for its session that most needs the owner, else its newest one.
const urgency: Partial<Record<SessionSnapshot['state']['state'], number>> = { waiting_for_local_ui: 0, running: 1, starting: 1, stopping: 1, failed: 2 };
const lead = (items: SessionSnapshot[]) => items.reduce((best, item) => (urgency[item.state.state] ?? 3) < (urgency[best.state.state] ?? 3) ? item : best);

/**
 * Sessions of one Agent at a time (or all), grouped by project and collapsed by default. The daemon lists
 * sessions in creation order, so newest come first; that is not activity time, which the protocol lacks.
 */
export function SessionList({ client, state, local }: { client: DaemonClient; state: DaemonView; local: boolean }) {
  const sessions = state.sessions ?? [], projects = state.projects ?? [], runtimes = state.runtimes;
  useProjectRoots(client, state, local);
  const [chosen, setChosen] = useState(() => load(AGENT_KEY) ?? '');
  const [query, setQuery] = useState('');
  const [open, setOpen] = useState<string[]>(() => { try { return JSON.parse(load(OPEN_KEY) ?? '[]'); } catch { return []; } });
  const counts = new Map<string, number>();
  for (const session of sessions) counts.set(session.runtimeId, (counts.get(session.runtimeId) ?? 0) + 1);
  const agent = counts.has(chosen) ? chosen : counts.size === 1 ? [...counts.keys()][0] : '';
  const agentLabel = (id: string) => id ? `${runtimeName(runtimes, id)} · ${counts.get(id)}` : `全部 Agent · ${sessions.length}`;
  const needle = query.trim().toLowerCase();

  const groups = new Map<string, SessionSnapshot[]>();
  for (const session of sessions.filter(item => !agent || item.runtimeId === agent).reverse()) {
    const key = projects.some(project => project.id === session.projectId) ? session.projectId : '';
    groups.set(key, [...(groups.get(key) ?? []), session]);
  }
  const shown = [...groups].sort(([a], [b]) => +!a - +!b).map(([key, items]) => {
    const project = projects.find(item => item.id === key);
    const name = project?.name ?? '已移除项目的会话';
    const matches = !needle || name.toLowerCase().includes(needle) ? items : items.filter(item => sessionTitle(item, runtimes).toLowerCase().includes(needle));
    const root = project && state.roots[key]?.cwdFence === project.cwdFence ? state.roots[key].root : null;
    return { key, name, root, items: matches, total: items.length };
  }).filter(group => group.items.length > 0);

  function toggle(key: string) {
    const next = open.includes(key) ? open.filter(item => item !== key) : [...open, key];
    setOpen(next); save(OPEN_KEY, JSON.stringify(next));
  }
  return <>
    <div className="session-tools">
      <ChipMenu className="agent-switch" label={agentLabel(agent)} name="Agent" disabled={false} busy={false}
        options={(counts.size > 1 ? ['', ...counts.keys()] : [...counts.keys()]).map(id => ({ key: id, label: agentLabel(id), checked: id === agent }))}
        onChoose={key => { setChosen(key); save(AGENT_KEY, key); }} />
      <input type="search" className="session-search" aria-label="搜索会话" placeholder="搜索会话" value={query} onChange={event => setQuery(event.target.value)} />
    </div>
    {shown.length === 0 ? <p className="field-note">没有匹配“{query.trim()}”的会话。</p> : <div className="session-groups">
      {shown.map((group, index) => {
        const expanded = !!needle || open.includes(group.key);
        const first = lead(group.items);
        return <section key={group.key} className="session-group" aria-label={group.name}>
          <button type="button" className="group-toggle" aria-expanded={expanded} aria-controls={`session-group-${index}`} onClick={() => toggle(group.key)}>
            <span className="group-chevron" aria-hidden="true">{expanded ? '▾' : '▸'}</span>
            <span className="group-line"><span className="group-title">{group.name}</span>{group.root && <span className="group-path">{group.root}</span>}</span>
            <span className="group-count">{group.total} 个会话</span>
            {!expanded && <><span className="group-lead">{sessionTitle(first, runtimes)}</span><StatePill session={first} /></>}
          </button>
          {expanded && <ul id={`session-group-${index}`} className="session-list">
            {group.items.map(session => {
              const meta = [!agent && runtimeName(runtimes, session.runtimeId), session.model && `${session.model.provider} / ${session.model.id}`,
                session.thinkingLevel && `思考：${thinkingLabels[session.thinkingLevel]}`].filter(Boolean);
              return <li key={session.id}><a href={sessionHref(session.id)} className="session-row">
                <span className="session-row-title">{sessionTitle(session, runtimes)}</span>
                {meta.length > 0 && <span className="session-row-meta">{meta.map(part => <span key={String(part)}>{part}</span>)}</span>}
                <StatePill session={session} />
              </a></li>;
            })}
          </ul>}
        </section>;
      })}
    </div>}
  </>;
}
