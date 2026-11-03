import { Channel, invoke, type InvokeArgs } from '@tauri-apps/api/core';
import type { ClientRequest } from '../protocol/generated/ClientRequest';
import type { ServerMessage } from '../protocol/generated/ServerMessage';
import type { DaemonDelivery } from '../protocol/generated/DaemonDelivery';

export interface Transport {
  open(onMessage: (message: ServerMessage) => void, onClose: () => void): Promise<void>;
  send(request: ClientRequest): Promise<void>;
  close(): Promise<void>;
}

type Bridge = {
  invoke: (command: string, args?: InvokeArgs) => Promise<unknown>;
  channel: () => Pick<Channel<DaemonDelivery>, 'onmessage'>;
};

export class TauriTransport implements Transport {
  private bridge: Bridge;
  private id: string | null = null;
  private opening: Promise<unknown> | null = null;
  private closed = false;
  private sequence = 0;
  private onClose: (() => void) | null = null;

  constructor(bridge: Bridge = { invoke, channel: () => new Channel<DaemonDelivery>() }) {
    this.bridge = bridge;
  }

  async open(onMessage: (message: ServerMessage) => void, onClose: () => void) {
    if (this.opening) throw new Error('连接已经开始。');
    this.onClose = onClose;
    const events = this.bridge.channel();
    events.onmessage = (delivery) => {
      if (this.closed || (this.id !== null && delivery.connectionId !== this.id)) return;
      this.id = delivery.connectionId;
      if (delivery.kind === 'closed') { void this.close(); return; }
      if (delivery.sequence !== this.sequence + 1) { void this.close(); return; }
      this.sequence = delivery.sequence;
      try {
        onMessage(delivery.message);
        void this.bridge.invoke('daemon_ack', {
          connectionId: delivery.connectionId, sequence: delivery.sequence,
        }).catch(() => this.close());
      } catch { void this.close(); }
    };
    this.opening = this.bridge.invoke('daemon_open', { events });
    try {
      const id = await this.opening;
      if (typeof id !== 'string' || !/^[1-9][0-9]{0,19}$/.test(id)) throw new Error('原生连接标识无效。');
      if (this.closed) { await this.bridge.invoke('daemon_close', { connectionId: id }).catch(() => {}); return; }
      if (this.id !== null && this.id !== id) { await this.close(); throw new Error('连接标识不一致。'); }
      this.id = id;
    } catch (error) { await this.close(); throw error; }
  }

  async send(request: ClientRequest) {
    if (this.closed || this.id === null) throw new Error('守护进程未连接。');
    const frame = JSON.stringify(request);
    if (new TextEncoder().encode(frame).byteLength > 262144) throw new Error('请求超过帧大小上限。');
    await this.bridge.invoke('daemon_send', { connectionId: this.id, frame });
  }

  async close() {
    if (this.closed) return;
    this.closed = true;
    const id = this.id;
    this.id = null;
    this.onClose?.();
    if (id !== null) await this.bridge.invoke('daemon_close', { connectionId: id }).catch(() => {});
    // If open is still pending, its continuation closes the late-created native connection.
  }
}
