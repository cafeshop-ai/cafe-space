import type { Recovery } from '../state/DaemonClient';

const reasons: Record<Recovery['state']['code'], (from: number, to: number) => string> = {
  SCHEMA_MIGRATION_FAILED: (from, to) => `守护进程把数据从第 ${from} 版升级到第 ${to} 版时失败了。为了保护数据，恢复之前不能使用会话。`,
};

/** UX §5.10: the only action offered is an explicit restore at the daemon's computer; nothing is downgraded or overwritten here. */
export function RecoveryScreen({ recovery: { state, details, error }, local, onRecheck }: { recovery: Recovery; local: boolean; onRecheck: () => void }) {
  const here = local && error !== 'LOCAL_ONLY';
  return <section className="recovery-panel" aria-labelledby="recovery-title">
    <h2 id="recovery-title">需要恢复数据</h2>
    <p>{reasons[state.code](state.fromSchema, state.toSchema)}</p>
    {!here ? <p>需要在这台电脑上处理。</p> : <>
      <p>{state.backup ? '升级前留下的备份还在，可以用它恢复。' : '升级前的备份没有保留下来，现在没有可以恢复的来源。请先保留下面这些文件，不要改动，也不要删除，再把 cafe-space status --json 的输出交给维护者处理。'}</p>
      {details ? <dl className="recovery-files">
        <div><dt>数据目录</dt><dd><code>{details.dataDirectory}</code></dd></div>
        {details.backupFile && <div><dt>备份文件</dt><dd><code>{details.backupFile}</code></dd></div>}
        <div><dt>状态文件</dt><dd>{details.stateFiles.map(file => <code key={file}>{file}</code>)}</dd></div>
      </dl> : <p role="status">{error ? '没能读取涉及的文件。请点“重新检查”再试一次。' : '正在读取涉及的文件…'}</p>}
      {state.backup && <>
        <p>在这台电脑的终端里运行下面的命令。它会用备份替换现在的数据，运行前会请你确认。</p>
        <pre className="recovery-command"><code>cafe-space recovery restore</code></pre>
        <p>恢复完成后，点“重新检查”。</p>
      </>}
    </>}
    <button className="secondary-button" onClick={onRecheck}>重新检查</button>
  </section>;
}
