import type { DaemonClient, DaemonView } from '../state/DaemonClient';
import { AgentCheck } from './Agents';
import { CoffeeActivityMark } from './CoffeeActivityMark';

export function FirstRunIntro() {
  return <div className="first-run-intro">
    <CoffeeActivityMark />
    <p>Café Space 让你在这台电脑上使用自己的编程 Agent，之后也可以从手机继续。</p>
  </div>;
}

/**
 * UX §5.1 step I, shown until this computer has a ready agent. Steps II (remote access) and III
 * arrive with M2. Without the daemon, the installation panel comes first and this step waits.
 */
export function FirstRun({ client, state, local, intro = true }: { client: DaemonClient; state: DaemonView; local: boolean; intro?: boolean }) {
  return <div className="first-run">
    {intro && <FirstRunIntro />}
    <ol className="first-run-steps">
      <li>
        <h2><span className="step-numeral">I</span> Agent</h2>
        <AgentCheck client={client} state={state} local={local} />
      </li>
    </ol>
  </div>;
}
