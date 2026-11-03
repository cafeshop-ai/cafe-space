import type { EventCursor } from '../protocol/generated/EventCursor';
import type { Request } from '../protocol/generated/Request';
import type { ServerMessage } from '../protocol/generated/ServerMessage';
import type { SessionList } from '../protocol/generated/SessionList';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { SubscriptionResponse } from '../protocol/generated/SubscriptionResponse';
import type { UpdateState } from '../protocol/generated/UpdateState';
import { TauriTransport, type Transport } from '../services/transport';

export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'version_mismatch';
export type DaemonView = {
  connection: ConnectionState;
  sessions: SessionSnapshot[] | null;
  daemonVersion: string | null;
  daemonUpdate: UpdateState | null;
  lastUpdated: number | null;
  error: string | null;
};

export class DaemonClient {
  private createTransport: () => Transport;
  private transport: Transport | null = null;
  private generation = 0;
  private cursor: EventCursor | null = null;
  private replayThrough: number | null = null;
  private timeout: ReturnType<typeof setTimeout> | null = null;
  private listeners = new Set<() => void>();
  private pending = new Map<string, 'health' | 'sessions' | 'subscription'>();
  private health = false;
  private list: SessionList | null = null;
  private subscription: SubscriptionResponse | null = null;
  private snapshot: DaemonView = {
    connection: 'disconnected', sessions: null, daemonVersion: null, daemonUpdate: null, lastUpdated: null, error: null,
  };

  constructor(createTransport: () => Transport = () => new TauriTransport()) {
    this.createTransport = createTransport;
  }

  getSnapshot = () => this.snapshot;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private update(change: Partial<DaemonView>) {
    this.snapshot = { ...this.snapshot, ...change };
    this.listeners.forEach(listener => listener());
  }
  private clearTimeout() { if (this.timeout !== null) clearTimeout(this.timeout); this.timeout = null; }

  async connect() {
    const generation = ++this.generation;
    await this.transport?.close();
    if (generation !== this.generation) return;
    this.clearTimeout();
    this.pending.clear(); this.health = false; this.list = null; this.subscription = null; this.replayThrough = null;
    this.update({ connection: 'connecting', error: null });
    const transport = this.createTransport();
    this.transport = transport;
    this.timeout = setTimeout(() => this.fail('等待守护进程超时，请确认它已启动后重新连接。'), 10000);
    try {
      await transport.open(message => {
        if (generation === this.generation) this.receive(message);
      }, () => {
        if (generation !== this.generation) return;
        this.clearTimeout();
        this.pending.clear();
        if (this.snapshot.connection !== 'version_mismatch') this.update({ connection: 'disconnected', error: this.snapshot.error ?? '守护进程连接已断开，请重新连接。' });
      });
    } catch {
      if (generation === this.generation) this.fail('无法连接守护进程，请先启动同一版本的 cafe-spaced。');
    }
  }

  disconnect() {
    ++this.generation;
    this.clearTimeout(); this.pending.clear(); this.replayThrough = null;
    this.update({ connection: 'disconnected' });
    void this.transport?.close();
    this.transport = null;
  }

  private fail(error: string) {
    this.clearTimeout(); this.pending.clear(); this.replayThrough = null;
    this.update({ connection: 'disconnected', error });
    void this.transport?.close();
  }

  private request(kind: 'health' | 'sessions' | 'subscription', request: Request) {
    const requestId = `${this.generation}-${kind}`;
    this.pending.set(requestId, kind);
    const generation = this.generation;
    void this.transport?.send({ type: 'request', protocolVersion: 2, requestId, request }).catch(() => {
      if (generation === this.generation) this.fail('读取请求未能完成，请重新连接。不会自动重放请求。');
    });
  }

  private receive(message: ServerMessage) {
    if (message.type === 'version_mismatch') {
      this.clearTimeout(); this.pending.clear();
      this.update({ connection: 'version_mismatch', daemonVersion: message.daemonVersion, error: '协议版本不符，请使用配套版本的桌面应用与守护进程。' });
      void this.transport?.close();
      return;
    }
    if (message.type === 'welcome') {
      if (this.pending.size || this.snapshot.connection !== 'connecting') { this.fail('守护进程重复发送握手，请重新连接。'); return; }
      this.update({ daemonVersion: message.daemonVersion });
      this.request('health', { method: 'get_status' });
      this.request('sessions', { method: 'list_sessions' });
      return;
    }
    if (message.type === 'response') {
      const kind = this.pending.get(message.requestId);
      if (!kind || message.result.status !== 'applied' || message.result.data.result.kind !== kind) {
        this.fail('守护进程返回了无法确认的读取结果，请重新连接。'); return;
      }
      this.pending.delete(message.requestId);
      const result = message.result.data.result;
      if (result.kind === 'health') this.health = result.data.ready;
      if (result.kind === 'sessions') this.list = result.data;
      if (result.kind === 'subscription') this.subscription = result.data;
      if (this.pending.size) return;
      if (!this.subscription && this.health && this.list) {
        // Subscribe after the list so its high-water cannot precede the confirmed list cursor.
        this.request('subscription', { method: 'subscribe_events', params: { after: this.cursor } });
        return;
      }
      const list = this.list, subscription = this.subscription;
      if (!this.health || !list || !subscription || list.eventStreamId !== subscription.eventStreamId || list.sequence > subscription.sequence ||
        (subscription.mode === 'snapshot_required' && !subscription.snapshot) ||
        (subscription.mode === 'resumed' && (!this.cursor || this.snapshot.sessions === null || this.cursor.eventStreamId !== subscription.eventStreamId || this.cursor.sequence > subscription.sequence))) {
        this.cursor = null; this.fail('会话快照未就绪，请重新连接。'); return;
      }
      const resumed = subscription.mode === 'resumed';
      const catchingUp = resumed && this.cursor !== null && this.cursor.sequence < subscription.sequence;
      this.replayThrough = catchingUp ? subscription.sequence : null;
      // A resumed response announces H; it does not mean after+1..H have been applied.
      if (!resumed) this.cursor = { eventStreamId: subscription.eventStreamId, sequence: subscription.sequence };
      if (!catchingUp) this.clearTimeout();
      this.update({ connection: catchingUp ? 'connecting' : 'connected', sessions: resumed ? this.snapshot.sessions : subscription.snapshot!.sessions, daemonUpdate: resumed ? this.snapshot.daemonUpdate : subscription.snapshot!.update, lastUpdated: catchingUp ? this.snapshot.lastUpdated : Date.now(), error: null });
      return;
    }
    if (message.type === 'event') {
      const replaying = this.snapshot.connection === 'connecting' && this.replayThrough !== null;
      if (!this.cursor || (!replaying && this.snapshot.connection !== 'connected') || message.eventStreamId !== this.cursor.eventStreamId || message.sequence !== this.cursor.sequence + 1) {
        this.cursor = null; this.fail('事件流已变化，重新连接后将读取完整快照。'); return;
      }
      let sessions = this.snapshot.sessions ?? [];
      const event = message.event;
      if (event.kind === 'session_changed') {
        const exists = sessions.some(item => item.id === event.data.session.id);
        sessions = exists ? sessions.map(item => item.id === event.data.session.id ? event.data.session : item) : [...sessions, event.data.session];
      }
      if (event.kind === 'session_removed') sessions = sessions.filter(item => item.id !== event.data.sessionId);
      const daemonUpdate = event.kind === 'update_changed' ? event.data.update : this.snapshot.daemonUpdate;
      this.cursor = { eventStreamId: message.eventStreamId, sequence: message.sequence };
      const caughtUp = replaying && message.sequence === this.replayThrough;
      if (caughtUp) { this.replayThrough = null; this.clearTimeout(); }
      this.update({ sessions, daemonUpdate, lastUpdated: Date.now(), ...(caughtUp ? { connection: 'connected' } as const : {}) });
    }
  }
}
