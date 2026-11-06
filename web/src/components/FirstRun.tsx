import { useEffect } from 'react';
import type { DaemonClient, DaemonView } from '../state/DaemonClient';
import { CoffeeActivityMark } from './CoffeeActivityMark';
import { ProxyPrompt } from './OwnerEnvironment';
import { ProjectAddForm, projectScopeNote } from './ProjectSettings';
import { ToolApprovalMark } from './StatePill';

/**
 * UX §5.1 steps I and II for a computer with no approved project yet. Steps III (remote access)
 * and IV arrive with M2. Detection only lists what the daemon reports; nothing is sent to a model.
 */
export function FirstRun({ client, state, local }: { client: DaemonClient; state: DaemonView; local: boolean }) {
  const connected = state.connection === 'connected';
  useEffect(() => { if (connected) void client.loadRuntimes(); }, [client, connected]);
  const runtimes = state.runtimes ?? [];
  const ready = runtimes.some(item => item.available);
  return <div className="first-run">
    <div className="first-run-intro">
      <CoffeeActivityMark />
      <p>Café Space 让你在这台电脑上使用自己的编程 Agent，之后也可以从手机继续。</p>
    </div>
    <ol className="first-run-steps">
      <li>
        <h2><span className="step-numeral">I</span> Agent</h2>
        <p className="field-note">只检测本机安装了哪些 Agent，不会向任何模型发送内容。</p>
        {runtimes.length === 0 ? <p className="field-note">{state.runtimes === null ? '正在检测…' : '没有检测到 Agent。'}</p> :
          <ul className="runtime-list">{runtimes.map(item => <li key={item.id}>
            <span className="choice-title">{item.name}{item.version && <span className="choice-version"> {item.version}</span>}</span>
            <span className="runtime-marks">{item.available && <ToolApprovalMark value={item.toolApproval} />}
              <span className="state-pill" data-state={item.available ? 'ready' : 'stopped'}>{item.available ? '就绪' : '不可用'}</span></span>
          </li>)}</ul>}
        {local && <ProxyPrompt client={client} connected={connected} />}
        {state.runtimes !== null && !ready && <p className="field-note">至少需要一个就绪的 Agent。安装 Pi 或 Codex 后重新打开这个页面。</p>}
      </li>
      <li>
        <h2><span className="step-numeral">II</span> 项目</h2>
        <p className="field-note">{projectScopeNote}</p>
        {local ? <ProjectAddForm client={client} connected={connected} /> : <p className="field-note">项目只能在电脑上批准。</p>}
      </li>
    </ol>
  </div>;
}
