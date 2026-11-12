import type { DaemonClient, DaemonView } from '../state/DaemonClient';
import { AgentCheck } from './Agents';
import { CoffeeActivityMark } from './CoffeeActivityMark';
import { ProjectAddForm, projectScopeNote } from './ProjectSettings';

export function FirstRunIntro({ install }: { install?: boolean }) {
  return <div className="first-run-intro">
    <CoffeeActivityMark />
    <div>
      <p>Café Space 让你在这台电脑上使用自己的编程 Agent，之后也可以从手机继续。</p>
      {install && <p>开始之前，先安装守护进程：它在后台运行 Agent，关闭窗口也不会停止。</p>}
    </div>
  </div>;
}

/**
 * UX §5.1 steps I and II, shown until this computer has a project and a ready agent. Steps III (remote access)
 * and IV arrive with M2. Without the daemon, the installation panel comes first and both steps wait.
 */
export function FirstRun({ client, state, local, intro = true }: { client: DaemonClient; state: DaemonView; local: boolean; intro?: boolean }) {
  const connected = state.connection === 'connected';
  const projects = state.projects ?? [];
  return <div className="first-run">
    {intro && <FirstRunIntro />}
    <ol className="first-run-steps">
      <li>
        <h2><span className="step-numeral">I</span> Agent</h2>
        <p className="field-note">只检测本机安装了哪些 Agent，不会向任何模型发送内容。</p>
        <AgentCheck client={client} state={state} local={local} proxy />
      </li>
      <li>
        <h2><span className="step-numeral">II</span> 项目</h2>
        <p className="field-note">{projectScopeNote}</p>
        {!connected ? <p className="field-note">装好并连上守护进程后批准。</p> :
          projects.length ? <p className="field-note">已批准：{projects.map(item => item.name).join('、')}。更多项目在 <a href="#/settings">设置 · 项目</a> 里批准。</p> :
          local ? <ProjectAddForm client={client} connected={connected} /> : <p className="field-note">项目只能在电脑上批准。</p>}
      </li>
    </ol>
  </div>;
}
