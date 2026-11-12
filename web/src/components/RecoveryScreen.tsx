import { useRef, useState } from 'react';
import type { RequestResult } from '../protocol/generated/RequestResult';
import type { DaemonClient, Recovery } from '../state/DaemonClient';

const reasons: Record<Recovery['state']['code'], (from: number, to: number) => string> = {
  SCHEMA_MIGRATION_FAILED: (from, to) => `守护进程把数据从第 ${from} 版升级到第 ${to} 版时失败了。为了保护数据，恢复之前不能使用会话。`,
  ROLLBACK_REQUIRED: () => '升级前的数据已恢复。这一版不能使用它，请回到上一版，之后可以再升级。',
};
function outcomeText(result: RequestResult) {
  if (result.status === 'applied') return '备份已恢复，正在重新连接…';
  if (result.status === 'rejected') return result.data.error.code === 'NOT_CONNECTED' ? '连接已断开，恢复请求没有发出。请先点“重新检查”。' : `守护进程没有恢复备份（${result.data.error.code}），数据没有改动。`;
  return '不确定备份是否已经恢复。请点“重新检查”看现在的状态，确认之前不要再恢复一次。';
}

/**
 * UX §5.10: restoring the backup is an explicit request done by the daemon at its own computer; nothing is overwritten here.
 * Going back to the previous version is the controller's, offered once it has read the current data (`rollbackAvailable`, null until known); `onRollback` opens it.
 */
export function RecoveryScreen({ client, recovery: { state, details, error }, local, live, rollbackAvailable, onRollback }: {
  client: DaemonClient; recovery: Recovery; local: boolean; live: boolean; rollbackAvailable: boolean | null; onRollback?: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<RequestResult | null>(null);
  const dialog = useRef<HTMLDialogElement>(null);
  const owner = error !== 'LOCAL_ONLY';
  const restored = state.code === 'ROLLBACK_REQUIRED';
  const offer = !restored && owner && state.backup && local;
  const rollback = owner && local && rollbackAvailable && onRollback;
  const unsure = outcome !== null && outcome.status !== 'applied' && outcome.status !== 'rejected';
  async function restore() {
    dialog.current?.close();
    setBusy(true); setOutcome(null);
    const result = await client.restoreBackup(state.code);
    setBusy(false); setOutcome(result);
    // Reading the status again is safe: it now reports ROLLBACK_REQUIRED.
    if (result.status === 'applied') void client.connect();
  }
  return <section className="recovery-panel" aria-labelledby="recovery-title">
    <h2 id="recovery-title">{restored ? '需要回到上一版' : '需要恢复数据'}</h2>
    <p>{reasons[state.code](state.fromSchema, state.toSchema)}</p>
    {!owner ? <p>需要在这台电脑上处理。</p> : restored ? <>
      {!local && <p>请在这台电脑的 Café Space 应用里回到上一版。没有界面的主机可以在终端运行 cafe-space update rollback。</p>}
      {local && rollbackAvailable === false && <p>这台电脑上没有能读取这些数据的上一版。请安装上一版 Café Space。</p>}
    </> : <>
      <p>{state.backup ? '升级前留下的备份还在，可以用它恢复。' : '升级前的备份没有保留下来，现在没有可以恢复的来源。请先保留下面这些文件，不要改动，也不要删除，再把 cafe-space status --json 的输出交给维护者处理。'}</p>
      {details ? <dl className="recovery-files">
        <div><dt>数据目录</dt><dd><code>{details.dataDirectory}</code></dd></div>
        {details.backupFile && <div><dt>备份文件</dt><dd><code>{details.backupFile}</code></dd></div>}
        <div><dt>状态文件</dt><dd>{details.stateFiles.map(file => <code key={file}>{file}</code>)}</dd></div>
      </dl> : <p role="status">{error ? '没能读取涉及的文件。请点“重新检查”再试一次。' : '正在读取涉及的文件…'}</p>}
      {state.backup && !local && <p>请在这台电脑的 Café Space 应用里恢复备份。没有界面的主机可以在终端运行 cafe-space recovery restore。</p>}
      {state.backup && local && !live && <p>连接已断开，点“重新检查”后再恢复。</p>}
    </>}
    {outcome && <p role={outcome.status === 'applied' ? 'status' : 'alert'}>{outcomeText(outcome)}</p>}
    <div className="recovery-actions">
      {offer && <button className="primary-button" disabled={busy || unsure || !live} onClick={() => dialog.current?.showModal()}>{busy ? '正在恢复…' : '恢复备份'}</button>}
      {rollback && <button className={restored || !offer ? 'primary-button' : 'secondary-button'} disabled={busy} onClick={onRollback}>{restored ? '回到上一版' : '回到上一版（数据未改动）'}</button>}
      <button className="secondary-button" disabled={busy} onClick={() => void client.connect()}>重新检查</button>
    </div>
    {offer && <dialog ref={dialog} className="recovery-confirmation" aria-labelledby="restore-title">
      <h2 id="restore-title">恢复升级前的备份？</h2>
      <p>守护进程会用升级前的备份替换现在的数据。</p>
      <div className="sheet-actions">
        <button className="secondary-button" onClick={() => dialog.current?.close()}>暂不恢复</button>
        <button className="primary-button" onClick={() => void restore()}>恢复备份</button>
      </div>
    </dialog>}
  </section>;
}
