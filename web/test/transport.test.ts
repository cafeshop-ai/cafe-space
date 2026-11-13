import { describe, expect, it, vi } from 'vitest';
import type { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { TauriTransport } from '../src/services/transport';
import type { DaemonDelivery } from '../src/protocol/generated/DaemonDelivery';
import { piCommandRequests, piCommandResponses, piApprovalRequests, piApprovalResponses, piApprovalEvents, sessionResponse, desktopProbeSessionId, desktopProbeExpectedText } from '../src/protocol-fixtures.generated';

const delivery: DaemonDelivery = { kind: 'message', connectionId: '1', sequence: 1, message: { type: 'welcome', protocolVersion: 9, daemonVersion: '0.1.0' } };
const request = { type: 'request', protocolVersion: 9, requestId: 'test', request: { method: 'list_sessions' } } as const;
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

  it('preserves pending prompt outcomes and approval events with one ACK per delivery', async () => {
    const native = bridge(), transport = new TauriTransport(native), onMessage = vi.fn(), onClose = vi.fn();
    await transport.open(onMessage, onClose);
    native.emit(delivery);
    const requests = [...piCommandRequests, piApprovalRequests[0]];
    for (const frame of requests) await transport.send(frame);
    // Match the native proxy test: the original prompt is the final reply, not the first.
    const replies = [piCommandResponses[2], piCommandResponses[1], piApprovalEvents[0], piApprovalResponses[0], piApprovalEvents[1], piCommandResponses[0]];
    for (const [index, message] of replies.entries()) {
      native.emit({ kind: 'message', connectionId: '1', sequence: index + 2, message });
    }
    expect(onMessage.mock.calls.map(([message]) => message)).toEqual([delivery.message, ...replies]);
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_ack')).toEqual(
      [delivery.message, ...replies].map((_, index) => ['daemon_ack', { connectionId: '1', sequence: index + 1 }]),
    );
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_send')).toEqual(
      requests.map(frame => ['daemon_send', { connectionId: '1', frame: JSON.stringify(frame) }]),
    );
    expect(onClose).not.toHaveBeenCalled();
    native.emit({ kind: 'closed', connectionId: '1' });
    await Promise.resolve();
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_open')).toHaveLength(1);
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_send')).toHaveLength(requests.length);
    await expect(transport.send(piCommandRequests[2])).rejects.toThrow('未连接');
  });

  it('does not replay an unknown cancel when its Channel acknowledgement fails', async () => {
    const native = bridge(), transport = new TauriTransport(native), onMessage = vi.fn(), onClose = vi.fn();
    await transport.open(onMessage, onClose);
    native.emit(delivery);
    await transport.send(piCommandRequests[2]);
    native.invoke.mockImplementation(async command => {
      if (command === 'daemon_ack') throw new Error('synthetic ACK failure');
      return undefined;
    });
    const unknown = { ...delivery, sequence: 2, message: piCommandResponses[2] };
    native.emit(unknown);
    await vi.waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
    native.emit(unknown);
    expect(onMessage.mock.calls.map(([message]) => message)).toEqual([delivery.message, piCommandResponses[2]]);
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_ack')).toHaveLength(2);
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_close')).toHaveLength(1);
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_open')).toHaveLength(1);
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_send')).toEqual([
      ['daemon_send', { connectionId: '1', frame: JSON.stringify(piCommandRequests[2]) }],
    ]);
    await expect(transport.send(piCommandRequests[2])).rejects.toThrow('未连接');
  });

  it.each([false, true])('receives the final stopped snapshot before its ACK settles (rejected=%s) without replay', async rejected => {
    const native = bridge(), transport = new TauriTransport(native), onMessage = vi.fn(), onClose = vi.fn();
    await transport.open(onMessage, onClose);
    native.emit(delivery);
    const getSession = { ...request, requestId: sessionResponse.requestId,
      request: { method: 'get_session', params: { sessionId: desktopProbeSessionId } } } as const;
    await transport.send(getSession);
    let accept!: () => void, fail!: (error: Error) => void;
    const ack = new Promise<void>((resolve, reject) => { accept = resolve; fail = reject; });
    native.invoke.mockImplementation(command => command === 'daemon_ack' ? ack : Promise.resolve());
    const data = sessionResponse.result.data.result.data;
    const final: DaemonDelivery = { kind: 'message', connectionId: '1', sequence: 2, message: {
      ...sessionResponse, result: { status: 'applied', data: { result: { kind: 'session', data: { ...data, session: {
        ...data.session,
        session: { ...data.session.session, id: desktopProbeSessionId, state: { state: 'stopped' }, stopReason: 'SESSION_CLOSED' },
        messages: [{ id: 'final', role: 'assistant', status: 'complete', parts: [{ kind: 'text', data: { text: desktopProbeExpectedText } }] }],
      } } } } },
    } };
    native.emit(final);
    // Message receipt is deliberately not evidence that the native receiver has accepted its ACK.
    expect(onMessage.mock.calls.map(([message]) => message)).toEqual([delivery.message, final.message]);
    expect(onClose).not.toHaveBeenCalled();
    expect(native.invoke).toHaveBeenLastCalledWith('daemon_ack', { connectionId: '1', sequence: 2 });
    if (rejected) fail(new Error('synthetic final ACK failure')); else accept();
    await Promise.resolve();
    expect(onClose).toHaveBeenCalledTimes(rejected ? 1 : 0);
    await transport.close();
    native.emit(final);
    expect(onMessage).toHaveBeenCalledTimes(2);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_ack')).toEqual([
      ['daemon_ack', { connectionId: '1', sequence: 1 }], ['daemon_ack', { connectionId: '1', sequence: 2 }],
    ]);
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_close')).toHaveLength(1);
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_open')).toHaveLength(1);
    expect(native.invoke.mock.calls.filter(([command]) => command === 'daemon_send')).toEqual([
      ['daemon_send', { connectionId: '1', frame: JSON.stringify(getSession) }],
    ]);
    await expect(transport.send(getSession)).rejects.toThrow('未连接');
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
