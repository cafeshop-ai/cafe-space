import { describe, expect, it, vi } from 'vitest';
import type { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { TauriTransport } from '../src/services/transport';
import type { DaemonDelivery } from '../src/protocol/generated/DaemonDelivery';

const delivery: DaemonDelivery = { kind: 'message', connectionId: '1', sequence: 1, message: { type: 'welcome', protocolVersion: 5, daemonVersion: '0.1.0' } };
const request = { type: 'request', protocolVersion: 5, requestId: 'test', request: { method: 'list_sessions' } } as const;
function bridge() {
  const invoke = vi.fn<typeof tauriInvoke>().mockResolvedValue('1');
  const channel = { onmessage: (_delivery: DaemonDelivery) => {} };
  return { invoke, channel: () => channel, emit: (value: DaemonDelivery) => channel.onmessage(value) };
}

describe('Tauri transport boundaries', () => {
  it('handles a Channel delivery before open resolves and acknowledges every frame', async () => {
    const native = bridge();
    native.invoke.mockImplementation(async command => {
      if (command === 'daemon_open') native.emit(delivery);
      return '1';
    });
    const transport = new TauriTransport(native), onMessage = vi.fn(), onClose = vi.fn();
    await transport.open(onMessage, onClose);
    expect(onMessage).toHaveBeenCalledWith(delivery.message);
    expect(native.invoke).toHaveBeenCalledWith('daemon_ack', { connectionId: '1', sequence: 1 });
    await transport.send(request);
    expect(native.invoke).toHaveBeenCalledWith('daemon_send', { connectionId: '1', frame: JSON.stringify(request) });
    expect(JSON.stringify(native.invoke.mock.calls)).not.toContain('token');
    expect(JSON.stringify(native.invoke.mock.calls)).not.toContain('endpoint');
    await transport.close();
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('ignores another connection and closes on a duplicate sequence', async () => {
    const native = bridge(), transport = new TauriTransport(native), onMessage = vi.fn(), onClose = vi.fn();
    await transport.open(onMessage, onClose);
    native.emit({ ...delivery, connectionId: 'stale' });
    expect(onMessage).not.toHaveBeenCalled();
    native.emit(delivery); native.emit(delivery);
    await Promise.resolve();
    expect(onMessage).toHaveBeenCalledTimes(1);
    expect(onClose).toHaveBeenCalledTimes(1);
    await expect(transport.send(request)).rejects.toThrow('未连接');
  });

  it('closes a late native connection after the UI has unmounted', async () => {
    const native = bridge(); let resolve!: (id: string) => void;
    native.invoke.mockImplementation(command => command === 'daemon_open' ? new Promise<string>(done => { resolve = done; }) : Promise.resolve());
    const transport = new TauriTransport(native);
    const opening = transport.open(vi.fn(), vi.fn());
    await transport.close(); resolve('1'); await opening;
    expect(native.invoke).toHaveBeenCalledWith('daemon_close', { connectionId: '1' });
  });

  it('rejects oversized UTF-8 frames before invoking native send', async () => {
    const native = bridge(), transport = new TauriTransport(native);
    await transport.open(vi.fn(), vi.fn());
    await expect(transport.send({ ...request, requestId: '中'.repeat(100000) })).rejects.toThrow('帧大小');
    expect(native.invoke.mock.calls.some(([command]) => command === 'daemon_send')).toBe(false);
    await transport.close();
  });
});
