import type { EventCursor } from '../protocol/generated/EventCursor';
import type { RecoveryState } from '../protocol/generated/RecoveryState';
import type { RecoveryDetails } from '../protocol/generated/RecoveryDetails';
import type { Request } from '../protocol/generated/Request';
import type { RequestResult } from '../protocol/generated/RequestResult';
import type { RuntimeSummary } from '../protocol/generated/RuntimeSummary';
import type { ServerMessage } from '../protocol/generated/ServerMessage';
import type { SessionList } from '../protocol/generated/SessionList';
import type { ProjectSummary } from '../protocol/generated/ProjectSummary';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { SubscriptionResponse } from '../protocol/generated/SubscriptionResponse';
import type { UpdateState } from '../protocol/generated/UpdateState';
import { TauriTransport, type Transport } from '../services/transport';
import { applySessionEvent, type SessionDetail } from './sessionDetail';

export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'version_mismatch' | 'recovery';
export type { RecoveryState, RecoveryDetails };
/** Why the daemon refuses work until restored at its computer. `details` (local paths) stays null until read, or with `error` when it cannot be. */
export type Recovery = { state: RecoveryState; details: RecoveryDetails | null; error: string | null };
/** The one session whose transcript is open. `detail` is null while it loads or after it failed to load. */
export type OpenSession = { id: string; detail: SessionDetail | null; loading: boolean; error: string | null; removed: boolean };
/** A root path is only ever shown at this computer; it is kept with the fence it was read under. */
export type ProjectRootInfo = { cwdFence: string; root: string };
export type DaemonView = {
  connection: ConnectionState;
  sessions: SessionSnapshot[] | null;
  projects: ProjectSummary[] | null;
  runtimes: RuntimeSummary[] | null;
  roots: Record<string, ProjectRootInfo>;
  session: OpenSession | null;
  daemonVersion: string | null;
  daemonUpdate: UpdateState | null;
  lastUpdated: number | null;
  error: string | null;
  recovery: Recovery | null;
};

type ReadKind = 'health' | 'sessions' | 'subscription' | 'recovery_details';
const CALL_TIMEOUT_MS = 30000;
const MAX_EXPIRED = 64;
/** Outcomes the client creates itself never claim success: a lost reply is unknown, not failed. */
const unknownResult = (code: string): RequestResult => ({ status: 'unknown', data: { error: { code, message: '' } } });
const rejectedResult = (code: string): RequestResult => ({ status: 'rejected', data: { error: { code, message: '' } } });

export class DaemonClient {
  private createTransport: () => Transport;
  private transport: Transport | null = null;
  private generation = 0;
  private cursor: EventCursor | null = null;
  private replayThrough: number | null = null;
  private timeout: ReturnType<typeof setTimeout> | null = null;
  private listeners = new Set<() => void>();
  private pending = new Map<string, ReadKind>();
  private health = false;
  private list: SessionList | null = null;
  private subscription: SubscriptionResponse | null = null;
  private calls = new Map<string, { resolve: (result: RequestResult) => void; timer: ReturnType<typeof setTimeout> }>();
  private expired: string[] = [];
  private callCounter = 0;
  private snapshot: DaemonView = {
    connection: 'disconnected', sessions: null, projects: null, runtimes: null, roots: {}, session: null, daemonVersion: null, daemonUpdate: null, lastUpdated: null, error: null, recovery: null,
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
    // A root may change without a new fence while away, so each connection reads paths afresh.
    this.update({ connection: 'connecting', error: null, roots: {}, recovery: null });
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
        this.settleCalls();
        const recovery = this.snapshot.recovery;
        if (recovery && !recovery.details && !recovery.error) this.update({ recovery: { ...recovery, error: 'CONNECTION_LOST' } });
        if (this.snapshot.connection !== 'version_mismatch') this.update({ connection: 'disconnected', error: this.snapshot.error ?? '守护进程连接已断开，请重新连接。' });
      });
    } catch {
      if (generation === this.generation) this.fail('无法连接守护进程，请先启动同一版本的 cafe-spaced。');
    }
  }

  disconnect() {
    ++this.generation;
    this.clearTimeout(); this.pending.clear(); this.replayThrough = null;
    this.settleCalls();
    this.update({ connection: 'disconnected' });
    void this.transport?.close();
    this.transport = null;
  }

  private fail(error: string) {
    ++this.generation;
    this.clearTimeout(); this.pending.clear(); this.replayThrough = null;
    this.settleCalls();
    this.update({ connection: 'disconnected', error });
    void this.transport?.close();
  }

  /** The daemon admits no work until restored at its computer, whatever `update` says; read which local files are involved. */
  private enterRecovery(state: RecoveryState) {
    this.clearTimeout();
    this.timeout = setTimeout(() => this.readRecovery(unknownResult('TIMEOUT')), 10000);
    this.update({ connection: 'recovery', recovery: { state, details: null, error: null }, error: null });
    this.request('recovery_details', { method: 'get_recovery_details' });
  }

  /** The connection stays open in recovery for an explicit restore; the screen stays until the next connection. */
  private readRecovery(result: RequestResult) {
    this.clearTimeout();
    const read = result.status === 'applied' ? result.data.result : null;
    const details = read?.kind === 'recovery_details' ? read.data : null;
    const error = details ? null : result.status === 'rejected' || result.status === 'unknown' ? result.data.error.code : 'UNEXPECTED_RESULT';
    this.update({ recovery: { ...this.snapshot.recovery!, details, error } });
  }

  /** A request in flight when the connection ends may still have taken effect, so its outcome is unknown. */
  private settleCalls() {
    for (const [, call] of this.calls) { clearTimeout(call.timer); call.resolve(unknownResult('CONNECTION_LOST')); }
    this.calls.clear(); this.expired = [];
  }

  /**
   * Sends one user operation and resolves with the daemon's outcome. It never retries: callers decide,
   * with the same client-chosen IDs, whether to ask again. A timeout or lost connection resolves `unknown`.
   */
  call(request: Request): Promise<RequestResult> {
    return this.snapshot.connection === 'connected' ? this.send(request) : Promise.resolve(rejectedResult('NOT_CONNECTED'));
  }

  /** Asks the daemon to put back the backup it kept before the failed migration. Sent once and never retried. */
  restoreBackup(code: RecoveryState['code']): Promise<RequestResult> {
    return this.snapshot.connection === 'recovery' ? this.send({ method: 'restore_backup', params: { confirm: true, code } }) : Promise.resolve(rejectedResult('NOT_CONNECTED'));
  }

  private send(request: Request): Promise<RequestResult> {
    const transport = this.transport;
    if (!transport) return Promise.resolve(rejectedResult('NOT_CONNECTED'));
    const requestId = `${this.generation}-op-${++this.callCounter}`;
    const generation = this.generation;
    return new Promise(resolve => {
      const timer = setTimeout(() => {
        if (!this.calls.delete(requestId)) return;
        // A late reply to this ID is ignored rather than treated as a protocol error.
        this.expired.push(requestId);
        if (this.expired.length > MAX_EXPIRED) this.expired.shift();
        resolve(unknownResult('TIMEOUT'));
      }, CALL_TIMEOUT_MS);
      this.calls.set(requestId, { resolve, timer });
      transport.send({ type: 'request', protocolVersion: 8, requestId, request }).catch(() => {
        const call = this.calls.get(requestId);
        if (!call || generation !== this.generation) return;
        clearTimeout(call.timer); this.calls.delete(requestId);
        resolve(unknownResult('SEND_FAILED'));
      });
    });
  }

  /** Reads an approved project's root. The daemon answers this only on a local owner-only connection. */
  async loadProjectRoot(projectId: string) {
    const result = await this.call({ method: 'get_project_root', params: { projectId } });
    if (result.status !== 'applied' || result.data.result.kind !== 'project_root') return;
    const { cwdFence, root } = result.data.result.data;
    this.update({ roots: { ...this.snapshot.roots, [projectId]: { cwdFence, root } } });
  }

  async loadRuntimes() {
    const result = await this.call({ method: 'list_runtimes' });
    if (result.status === 'applied' && result.data.result.kind === 'runtimes') this.update({ runtimes: result.data.result.data.items });
  }

  /** Opens one session's transcript. Events after the view's sequence are then applied to it in order. */
  async openSession(id: string) {
    const current = this.snapshot.session;
    if (current?.id !== id || !current.detail) this.update({ session: { id, detail: null, loading: true, error: null, removed: false } });
    // Reading is side-effect free, so one more read is safe when the first view is already behind.
    for (let attempt = 0; attempt < 2; attempt++) {
      const result = await this.call({ method: 'get_session', params: { sessionId: id } });
      if (this.snapshot.session?.id !== id) return;
      if (result.status !== 'applied' || result.data.result.kind !== 'session') {
        const code = result.status === 'rejected' || result.status === 'unknown' ? result.data.error.code : 'UNEXPECTED_RESULT';
        this.update({ session: { id, detail: null, loading: false, error: code, removed: code === 'SESSION_NOT_FOUND' } });
        return;
      }
      const { session, eventStreamId, sequence } = result.data.result.data;
      // A view older than events already applied here would silently miss them.
      if (this.cursor && eventStreamId === this.cursor.eventStreamId && sequence >= this.cursor.sequence) {
        this.update({ session: { id, detail: { view: session, eventStreamId, sequence }, loading: false, error: null, removed: false } });
        return;
      }
    }
    this.update({ session: { id, detail: null, loading: false, error: 'STALE_VIEW', removed: false } });
  }

  closeSessionView() { if (this.snapshot.session) this.update({ session: null }); }

  private request(kind: ReadKind, request: Request) {
    const requestId = `${this.generation}-${kind}`;
    this.pending.set(requestId, kind);
    const generation = this.generation;
    void this.transport?.send({ type: 'request', protocolVersion: 8, requestId, request }).catch(() => {
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
      const call = this.calls.get(message.requestId);
      if (call) {
        clearTimeout(call.timer); this.calls.delete(message.requestId);
        call.resolve(message.result);
        return;
      }
      if (this.expired.includes(message.requestId)) return;
      const kind = this.pending.get(message.requestId);
      // While recovering, the daemon refuses session reads; only the recovery details matter.
      if (kind && this.snapshot.connection === 'recovery') { this.pending.delete(message.requestId); if (kind === 'recovery_details') this.readRecovery(message.result); return; }
      if (!kind || message.result.status !== 'applied' || message.result.data.result.kind !== kind) {
        this.fail('守护进程返回了无法确认的读取结果，请重新连接。'); return;
      }
      this.pending.delete(message.requestId);
      const result = message.result.data.result;
      const recovery = result.kind === 'health' ? result.data.recovery : null;
      if (recovery) { this.enterRecovery(recovery); return; }
      if (result.kind === 'health' && result.data.awaitingUpdateConfirmation) { this.fail('上次连接：更新未完成，等待交接确认。请查看当前恢复入口。'); return; }
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
      this.update({ connection: catchingUp ? 'connecting' : 'connected', sessions: resumed ? this.snapshot.sessions : subscription.snapshot!.sessions, projects: resumed ? this.snapshot.projects : subscription.snapshot!.projects, runtimes: resumed ? this.snapshot.runtimes : subscription.snapshot!.runtimes, daemonUpdate: resumed ? this.snapshot.daemonUpdate : subscription.snapshot!.update, lastUpdated: catchingUp ? this.snapshot.lastUpdated : Date.now(), error: null });
      // A fresh snapshot may follow a gap the open transcript never saw; read it again.
      if (!resumed && this.snapshot.session) void this.openSession(this.snapshot.session.id);
      return;
    }
    if (message.type === 'event') {
      const replaying = this.snapshot.connection === 'connecting' && this.replayThrough !== null;
      if (!this.cursor || (!replaying && this.snapshot.connection !== 'connected') || message.eventStreamId !== this.cursor.eventStreamId || message.sequence !== this.cursor.sequence + 1) {
        this.cursor = null; this.fail('事件流已变化，重新连接后将读取完整快照。'); return;
      }
      let sessions = this.snapshot.sessions ?? [];
      let projects = this.snapshot.projects ?? [];
      const event = message.event;
      if (event.kind === 'project_changed') {
        const exists = projects.some(item => item.id === event.data.project.id);
        projects = exists ? projects.map(item => item.id === event.data.project.id ? event.data.project : item) : [...projects, event.data.project];
      }
      if (event.kind === 'project_removed') projects = projects.filter(item => item.id !== event.data.projectId);
      if (event.kind === 'session_changed') {
        const exists = sessions.some(item => item.id === event.data.session.id);
        sessions = exists ? sessions.map(item => item.id === event.data.session.id ? event.data.session : item) : [...sessions, event.data.session];
      }
      if (event.kind === 'session_removed') sessions = sessions.filter(item => item.id !== event.data.sessionId);
      const daemonUpdate = event.kind === 'update_changed' ? event.data.update : this.snapshot.daemonUpdate;
      this.cursor = { eventStreamId: message.eventStreamId, sequence: message.sequence };
      const caughtUp = replaying && message.sequence === this.replayThrough;
      if (caughtUp) { this.replayThrough = null; this.clearTimeout(); }
      let session = this.snapshot.session;
      let reload = false;
      if (session?.detail) {
        const next = applySessionEvent(session.detail, message.eventStreamId, message.sequence, event);
        if (next === 'removed') session = { ...session, detail: null, removed: true };
        else if (next === 'stale') { session = { ...session, detail: null, loading: true }; reload = true; }
        else if (next !== session.detail) session = { ...session, detail: next };
      }
      this.update({ sessions, projects, session, daemonUpdate, lastUpdated: Date.now(), ...(caughtUp ? { connection: 'connected' } as const : {}) });
      if (reload && session) void this.openSession(session.id);
    }
  }
}
