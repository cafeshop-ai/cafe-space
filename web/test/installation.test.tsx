import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DaemonInstallation } from '../src/components/DaemonInstallation';
import type { ConnectionState, DaemonClient, RecoveryState } from '../src/state/DaemonClient';
import type { InstallationInfo } from '../src/protocol/generated/InstallationInfo';
import type { InstallationResult } from '../src/protocol/generated/InstallationResult';
import { sessionsResponse } from '../src/protocol-fixtures.generated';

const native = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: native.invoke, Channel: class { onmessage = (_value: unknown) => {}; } }));
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
let root: Root | undefined;
let info: InstallationInfo;
const connect = vi.fn(async () => {});
const client = { connect } as unknown as DaemonClient;
const button = (label: string) => [...document.querySelectorAll('button')].find(item => item.textContent === label)!;
async function click(label: string) { await act(async () => button(label).click()); }
async function render(connection: ConnectionState = 'connected', recovery?: RecoveryState['code'], onInfo?: (value: InstallationInfo | null) => void) {
  if (!root) { const node = document.createElement('div'); document.body.append(node); root = createRoot(node); }
  const template = sessionsResponse.result.data.result.data.items[0];
  await act(async () => root!.render(<DaemonInstallation client={client} connection={connection} sessions={[{ ...template, title: '正在执行的会话', state: { state: 'running' } }]} recovery={recovery} onInfo={onInfo} show />));
}
beforeEach(() => {
  info = { bundledVersion: '0.1.0', installed: true, registered: true, updateAvailable: true, recoveryRequired: false, restoreOnly: false, rollbackAvailable: false };
  native.invoke.mockReset(); connect.mockClear();
  native.invoke.mockImplementation(async (command: string) => command === 'daemon_installation_info' ? info : 'updated');
  HTMLDialogElement.prototype.showModal = function () { this.setAttribute('open', ''); };
  HTMLDialogElement.prototype.close = function () { this.removeAttribute('open'); };
});
afterEach(async () => { await act(async () => root?.unmount()); root = undefined; document.body.replaceChildren(); });

describe('native installation controls', () => {
  it('does not install or change startup until the user explicitly chooses installation', async () => {
    info = { ...info, installed: false, registered: false, updateAvailable: false };
    await render('disconnected');
    expect(native.invoke.mock.calls.every(([command]) => command === 'daemon_installation_info')).toBe(true);
    await click('安装并启用登录自启');
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_install')).toHaveLength(1);
    expect(connect).toHaveBeenCalledTimes(1);
  });

  it('lists affected sessions and requires confirmation before the immediate update', async () => {
    await render(); await click('立即更新');
    expect(document.querySelector('dialog')?.open).toBe(true);
    expect(document.querySelector('dialog')?.textContent).toContain('正在执行的会话');
    expect(native.invoke.mock.calls.some(([command]) => command === 'daemon_update')).toBe(false);
    await click('暂不更新');
    expect(document.querySelector('dialog')?.open).toBe(false);
    expect(native.invoke.mock.calls.some(([command]) => command === 'daemon_update')).toBe(false);
    await click('立即更新'); await click('停止当前工作并更新');
    const calls = native.invoke.mock.calls.filter(([command]) => command === 'daemon_update');
    expect(calls).toHaveLength(1); expect(calls[0][1].mode).toBe('now');
    expect(Object.keys(calls[0][1]).sort()).toEqual(['events', 'mode']);
    expect(connect).toHaveBeenCalledTimes(1);
  });

  it('keeps the pending lease until native cancellation is confirmed, without replaying work', async () => {
    let finish: (result: InstallationResult) => void = () => { throw new Error('update not started'); };
    native.invoke.mockImplementation((command: string, args: { events?: { onmessage: (phase: string) => void } } = {}) => {
      if (command === 'daemon_installation_info') return Promise.resolve(info);
      if (command === 'daemon_update') { args.events!.onmessage('pending'); return new Promise<InstallationResult>(resolve => { finish = resolve; }); }
      return Promise.resolve();
    });
    await render(); await click('空闲时更新');
    expect(native.invoke.mock.calls.find(([command]) => command === 'daemon_update')![1].mode).toBe('when_idle');
    expect(button('取消等待更新')).toBeDefined();
    await click('取消等待更新');
    expect(document.body.textContent).not.toContain('已取消等待更新');
    await act(async () => finish('cancelled'));
    expect(document.body.textContent).toContain('已取消等待更新，当前工作未停止');
    expect(connect).not.toHaveBeenCalled();
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_update')).toHaveLength(1);
  });

  it('offers explicit finish and rollback without automatically choosing a version', async () => {
    info.recoveryRequired = true;
    await render('disconnected');
    expect(native.invoke.mock.calls.some(([command]) => command === 'daemon_recover')).toBe(false);
    await click('完成更新');
    expect(native.invoke.mock.calls.find(([command]) => command === 'daemon_recover')![1].action).toBe('finish');
    await click('回到上一版');
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_recover')[1][1].action).toBe('rollback');
  });

  it('shows refused state compatibility without offering database replacement or reconnecting', async () => {
    info.recoveryRequired = true; info.restoreOnly = true;
    native.invoke.mockImplementation(async (command: string) => {
      if (command === 'daemon_installation_info') return info;
      throw 'INSTALL_ROLLBACK_STATE_REFUSED';
    });
    await render();
    expect(button('完成更新')).toBeUndefined();
    expect(document.body.textContent).toContain('state.sqlite3');
    await click('回到上一版');
    expect(document.querySelector('[role="alert"]')?.textContent).toContain('不会覆盖数据库');
    expect(connect).not.toHaveBeenCalled();
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_recover')).toHaveLength(1);
  });

  it('under a failed migration holds rollback until the previous version has read the current data, and says why on a tap', async () => {
    info.recoveryRequired = true;
    await render('disconnected', 'SCHEMA_MIGRATION_FAILED');
    expect(button('完成更新')).toBeUndefined();
    const blocked = button('回到上一版 ⓘ');
    expect(blocked.getAttribute('aria-disabled')).toBe('true');
    await act(async () => blocked.click());
    expect(document.getElementById(blocked.getAttribute('aria-describedby')!)?.textContent).toBe('上一版还不能读取现在的数据，请先按恢复页处理');
    expect(native.invoke.mock.calls.some(([command]) => command === 'daemon_recover')).toBe(false);
  });

  it('goes back with the data untouched once the previous version has read it, with no handover needed', async () => {
    info.rollbackAvailable = true;
    await render('recovery', 'SCHEMA_MIGRATION_FAILED');
    expect(button('完成更新')).toBeUndefined();
    expect(button('回到上一版').className).toBe('secondary-button');
    await click('回到上一版');
    expect(native.invoke.mock.calls.find(([command]) => command === 'daemon_recover')![1].action).toBe('rollback');
  });

  it('once the backup is restored offers rollback as the only way on, and reports what it read', async () => {
    info.recoveryRequired = true; info.rollbackAvailable = true;
    const onInfo = vi.fn();
    await render('recovery', 'ROLLBACK_REQUIRED', onInfo);
    expect(onInfo).toHaveBeenLastCalledWith(info);
    expect(button('完成更新')).toBeUndefined();
    expect(document.body.textContent).not.toContain('请选择完成更新');
    expect(button('回到上一版').className).toBe('primary-button');
    await click('回到上一版');
    expect(native.invoke.mock.calls.find(([command]) => command === 'daemon_recover')![1].action).toBe('rollback');
  });

  it('says so when no previous version can take the restored data', async () => {
    await render('recovery', 'ROLLBACK_REQUIRED');
    expect(document.body.textContent).toContain('这台电脑上没有可以回到的上一版');
    expect([...document.querySelector('.installation-actions')!.querySelectorAll('button')].map(item => item.textContent)).toEqual(['重新检查状态']);
  });

  it('reads the controller again when the daemon reports recovery, and reports a failed read as unknown', async () => {
    const onInfo = vi.fn();
    await render('connecting', undefined, onInfo);
    const reads = () => native.invoke.mock.calls.filter(([command]) => command === 'daemon_installation_info').length;
    const before = reads();
    await render('recovery', 'ROLLBACK_REQUIRED', onInfo);
    expect(reads()).toBe(before + 1);
    native.invoke.mockRejectedValue('INSTALL_LEASE_UNAVAILABLE');
    await click('重新检查状态');
    expect(onInfo).toHaveBeenLastCalledWith(null);
  });
});
