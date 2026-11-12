import { useCallback, useEffect, useRef, useState } from 'react';
import { Channel, invoke } from '@tauri-apps/api/core';
import type { InstallationInfo } from '../protocol/generated/InstallationInfo';
import type { InstallationProgress } from '../protocol/generated/InstallationProgress';
import type { InstallationResult } from '../protocol/generated/InstallationResult';
import type { UpdateRecoveryAction } from '../protocol/generated/UpdateRecoveryAction';
import type { DaemonClient, ConnectionState, RecoveryState } from '../state/DaemonClient';
import { NoteChip } from './ChipMenu';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';

const phases: Record<InstallationProgress, string> = {
  staging: '正在校验并暂存守护进程…', registering: '正在设置当前用户的登录自启…', pending: '等待空闲后更新；当前工作会继续。',
  draining: '正在完成交接；此时不能取消。', starting: '正在等待系统启动守护进程…', recovering: '正在检查旧版兼容性并恢复二进制…',
  committed: '更新已确认。', rolled_back: '已恢复旧版守护进程。',
};
const results: Record<InstallationResult, string> = {
  installed: '守护进程已安装并由系统启动。', updated: '守护进程已更新。', unchanged: '守护进程已就绪。', cancelled: '已取消等待更新，当前工作未停止。', rolled_back: '已回到上一版守护进程。',
};
const errors: Record<string, string> = {
  INSTALL_BUSY: '此窗口已有安装或更新操作，请等待它结束。', INSTALL_LEASE_UNAVAILABLE: '无法取得安装锁。请确认没有其他窗口在更新，并检查数据目录权限。',
  INSTALL_DOWNGRADE_REFUSED: '应用内置的版本更旧，已拒绝降级。请使用匹配或更新的应用版本。', INSTALL_RECOVERY_REQUIRED: '上次更新尚未完成，请先恢复交接。',
  INSTALL_DAEMON_ALREADY_RUNNING: '已有手动启动的守护进程。请先停止它，再安装到受控路径。', INSTALL_RUNNING_BINARY_MISMATCH: '运行中的进程与已安装文件不符；没有切换二进制。',
  INSTALL_ROLLBACK_STATE_REFUSED: '旧版无法确认读取当前数据，已停止恢复。请保留数据并使用匹配版本。',
  INSTALL_IDENTITY_UNAVAILABLE: '无法确认进程属于本次安装，没有执行更新。请检查系统服务管理器。', INSTALL_PEER_MISMATCH: '进程与本次安装不符，没有执行更新。',
  INSTALL_NO_RECOVERY: '没有未完成的交接，请重新检查状态。', INSTALL_EXPLICIT_RESTORE_REQUIRED: '当前状态需要明确恢复，不能直接完成更新。', INSTALL_HANDOVER_CHANGED: '交接状态已经变化，没有继续操作。请重新检查。',
  INSTALL_START_UNCONFIRMED: '尚未确认守护进程启动。请检查系统服务管理器，再重新检查状态。', UPDATE_NOT_PENDING: '更新已开始交接，不能再取消等待。',
};

export function DaemonInstallation({ client, connection, sessions, recovery, onInfo, show }: {
  client: DaemonClient; connection: ConnectionState; sessions: readonly SessionSnapshot[] | null; recovery?: RecoveryState['code'];
  onInfo?: (info: InstallationInfo | null) => void; show: boolean;
}) {
  const [info, setInfo] = useState<InstallationInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [phase, setPhase] = useState<InstallationProgress | null>(null);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [cancelling, setCancelling] = useState(false);
  const dialog = useRef<HTMLDialogElement>(null);
  const mounted = useRef(false);
  const generation = useRef(0);
  const report = useRef(onInfo);
  report.current = onInfo;
  const refresh = useCallback(async () => {
    const current = ++generation.current;
    try { const value = await invoke<InstallationInfo>('daemon_installation_info'); if (mounted.current && current === generation.current) { setInfo(value); report.current?.(value); } }
    catch { if (mounted.current && current === generation.current) { setError('无法读取安装状态。请重新检查，或查看系统服务管理器。'); report.current?.(null); } }
  }, []);
  useEffect(() => { mounted.current = true; void refresh(); return () => { mounted.current = false; }; }, [refresh]);
  useEffect(() => { if (connection === 'connected' || connection === 'disconnected' || connection === 'recovery') void refresh(); }, [connection, refresh]);

  async function run(action: 'install' | 'start' | 'update' | 'recover', mode?: 'now' | 'when_idle' | UpdateRecoveryAction) {
    if (busy) return;
    setBusy(true); setError(''); setMessage(''); setCancelling(false); setPhase(action === 'start' ? 'starting' : action === 'recover' ? 'recovering' : 'staging');
    try {
      const events = new Channel<InstallationProgress>();
      events.onmessage = value => { if (mounted.current) setPhase(value); };
      const result = await invoke<InstallationResult>(`daemon_${action}`, { events, ...(mode ? action === 'recover' ? { action: mode } : { mode } : {}) });
      if (!mounted.current) return;
      setMessage(results[result]);
      if (result !== 'cancelled') await client.connect();
    } catch (cause) {
      if (mounted.current) setError(typeof cause === 'string' && errors[cause] ? errors[cause] : '操作未完成。请重新检查安装状态；有恢复入口时，先恢复交接。');
    } finally {
      if (mounted.current) { setBusy(false); setPhase(null); setCancelling(false); await refresh(); }
    }
  }
  async function cancel() {
    setCancelling(true);
    try { await invoke('daemon_cancel_update'); }
    catch (cause) { setError(typeof cause === 'string' && errors[cause] ? errors[cause] : '无法确认取消结果，请等待交接状态。'); setCancelling(false); }
  }
  if (!show && !busy) return null;
  const connected = connection === 'connected';
  const affected = sessions?.filter(session => !['stopped', 'failed'].includes(session.state.state));
  return <section className="installation-panel" aria-labelledby="installation-title" aria-busy={busy}>
    <h2 id="installation-title">守护进程安装与更新</h2>
    {!info && !error && <p role="status">正在检查安装状态…</p>}
    {info && <>
      <p>{recovery ? info.rollbackAvailable ? '上一版已核对过能读取现在的数据，可以回到上一版。' : recovery === 'ROLLBACK_REQUIRED' ? '这台电脑上没有可以回到的上一版。' : '升级没有完成，守护进程不接收新工作。'
        : info.recoveryRequired ? info.restoreOnly ? '恢复未完成。涉及 state.sqlite3、handover.json 和 bin 下的 .prev 文件。数据不兼容时，请恢复兼容备份或使用匹配版本。' : '更新未完成，守护进程尚未恢复接收新工作。请选择完成更新或回到上一版。' : !info.registered ? '将随应用附带的守护进程安装到当前用户的数据目录，并启用登录自启。' : info.updateAvailable ? `应用内置守护进程 ${info.bundledVersion} 与当前安装不同。` : `已安装应用内置守护进程 ${info.bundledVersion}。`}</p>
      <div className="installation-actions">
        {/* While the daemon reports recovery, rollback waits for the previous binary to have read the current data, and finishing would run the failed migration again (v2-local-api "Recovery state"). */}
        {recovery ? info.rollbackAvailable ? <button className={recovery === 'ROLLBACK_REQUIRED' ? 'primary-button' : 'secondary-button'} disabled={busy} onClick={() => void run('recover', 'rollback')}>回到上一版</button>
          : recovery === 'SCHEMA_MIGRATION_FAILED' && <NoteChip className="secondary-button" blocked note="上一版还不能读取现在的数据，请先按恢复页处理">回到上一版</NoteChip>
        : info.recoveryRequired ? <>
          {!info.restoreOnly && <button className="primary-button" disabled={busy} onClick={() => void run('recover', 'finish')}>完成更新</button>}
          <button className={info.restoreOnly ? "primary-button" : "secondary-button"} disabled={busy} onClick={() => void run('recover', 'rollback')}>回到上一版</button>
        </> : !info.registered ? <>
          <button className="primary-button" disabled={busy || connection !== 'disconnected'} onClick={() => void run('install')}>安装并启用登录自启</button>
          {connected && <span>请先停止手动启动的进程，再安装。</span>}
        </> : !connected ? <>
          <button className="primary-button" disabled={busy || connection !== 'disconnected'} onClick={() => void run('start')}>启动守护进程</button>
          <button className="secondary-button" disabled={busy || connection === 'connecting'} onClick={() => void client.connect()}>重新连接</button>
        </> : info.updateAvailable && <>
          <button className="primary-button" disabled={busy} onClick={() => void run('update', 'when_idle')}>空闲时更新</button>
          <button className="secondary-button" disabled={busy || sessions === null} onClick={() => dialog.current?.showModal()}>立即更新</button>
        </>}
        {busy && phase === 'pending' && <button className="secondary-button" disabled={cancelling} onClick={() => void cancel()}>{cancelling ? '正在请求取消…' : '取消等待更新'}</button>}
        {!busy && <button className="quiet-button" onClick={() => { setError(''); void refresh(); }}>重新检查状态</button>}
      </div>
    </>}
    {busy && phase && <p role="status">{phases[phase]}</p>}
    {!busy && message && <p role="status">{message}</p>}
    {error && <p className="connection-error" role="alert">{error}</p>}
    {!info && error && <button className="secondary-button" disabled={busy} onClick={() => { setError(''); void refresh(); }}>重新检查状态</button>}
    <dialog ref={dialog} className="update-confirmation" aria-labelledby="update-now-title">
      <h2 id="update-now-title">立即更新守护进程？</h2>
      <p>立即更新会停止当前工作，正在进行的操作可能中断。</p>
      {affected?.length ? <><p>受影响的会话：</p><ul>{affected.map(session => <li key={session.id}>{session.title || session.id}</li>)}</ul></> : <p>{sessions === null ? '尚未读取会话，请先重新连接。' : '当前没有活动会话。'}</p>}
      <div className="installation-actions">
        <button className="secondary-button" onClick={() => dialog.current?.close()}>暂不更新</button>
        <button className="primary-button" disabled={busy || !connected || sessions === null} onClick={() => { dialog.current?.close(); void run('update', 'now'); }}>停止当前工作并更新</button>
      </div>
    </dialog>
  </section>;
}
