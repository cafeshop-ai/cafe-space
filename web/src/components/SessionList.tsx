import type { ProjectSummary } from '../protocol/generated/ProjectSummary';
import type { RuntimeSummary } from '../protocol/generated/RuntimeSummary';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import { StatePill } from './StatePill';

export const sessionHref = (id: string) => `#/sessions/${encodeURIComponent(id)}`;
export const runtimeName = (runtimes: RuntimeSummary[] | null, id: string) => runtimes?.find(item => item.id === id)?.name ?? id;
export const sessionTitle = (session: SessionSnapshot, runtimes: RuntimeSummary[] | null) => session.title || `${runtimeName(runtimes, session.runtimeId)} 会话`;

/** UX §5: sessions grouped by project, in project order; sessions of a removed project stay visible as history. */
export function SessionList({ sessions, projects, runtimes }: { sessions: SessionSnapshot[]; projects: ProjectSummary[]; runtimes: RuntimeSummary[] | null }) {
  const groups: { key: string; name: string; items: SessionSnapshot[] }[] = projects
    .map(project => ({ key: project.id, name: project.name, items: sessions.filter(item => item.projectId === project.id) }))
    .filter(group => group.items.length > 0);
  const orphaned = sessions.filter(item => !projects.some(project => project.id === item.projectId));
  if (orphaned.length) groups.push({ key: '', name: '已移除的项目', items: orphaned });
  return <div className="session-groups">
    {groups.map(group => <section key={group.key} className="session-group" aria-label={group.name}>
      <h2 className="group-title">{group.name}</h2>
      <ul className="session-list">
        {group.items.map(session => <li key={session.id}>
          <a href={sessionHref(session.id)} className="session-row">
            <span className="session-row-title">{sessionTitle(session, runtimes)}</span>
            <span className="session-row-agent">{runtimeName(runtimes, session.runtimeId)}</span>
            <StatePill session={session} />
          </a>
        </li>)}
      </ul>
    </section>)}
  </div>;
}
