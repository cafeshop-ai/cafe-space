import { useEffect, useLayoutEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from 'react';
import type { ApprovalDecision } from '../protocol/generated/ApprovalDecision';
import type { ApprovalRequest } from '../protocol/generated/ApprovalRequest';
import type { DeliveryMode } from '../protocol/generated/DeliveryMode';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { TranscriptMessage } from '../protocol/generated/TranscriptMessage';
import type { DaemonClient, DaemonView } from '../state/DaemonClient';
import { MAX_PROMPT_BYTES, canResume, cancelOutcomeOf, errorMessage, outcomeOf, stopReasonLabels, utf8Bytes, type Outcome } from '../app/copy';
import { OutcomeMark, StatePill, ToolApprovalMark } from './StatePill';
import { runtimeName, sessionHref, sessionTitle } from './SessionList';
import { DraftInput, ThreadProvider, Transcript, useDraft, type PendingPrompt } from './SessionThread';
import { ThinkingControl } from './ThinkingControl';
import { STOP_EVENT } from './CommandPalette';

/** `seen`: user messages already in the transcript at send time. `echo`: the transcript message that confirmed it. */
type Pending = PendingPrompt & { seen: ReadonlySet<string>; echo?: string };
const MAX_PENDING = 50;
const NO_MESSAGES: readonly TranscriptMessage[] = [];
const userText = (message: TranscriptMessage) => message.parts.map(part => part.kind === 'text' ? part.data.text : '').join('').trim();

/**
 * In send order, each new user message with a bubble's text confirms at most that one bubble, which then stays
 * confirmed even if the message later leaves the bounded transcript. Matching only hides; it never upgrades an outcome.
 */
function reconcile(list: Pending[], messages: readonly TranscriptMessage[]) {
  const used = new Set(list.flatMap(item => item.echo ? [item.echo] : []));
  let changed = false;
  const next = list.map(item => {
    if (item.echo) return item;
    const match = messages.find(message => message.role === 'user' && !item.seen.has(message.id) && !used.has(message.id) && userText(message) === item.text.trim());
    if (!match) return item;
    used.add(match.id);
    changed = true;
    return { ...item, echo: match.id };
  });
  return changed ? next : list;
}

function ApprovalCard({ approval, allowForSession, onAnswer, outcome }: { approval: ApprovalRequest; allowForSession: boolean; onAnswer: (decision: ApprovalDecision) => void; outcome: Outcome | null }) {
  const pending = outcome?.kind === 'sent' || outcome?.kind === 'done';
  return <li className="approval-card" aria-label="审批请求">
    <p className="approval-summary">{approval.summary}</p>
    <div className="approval-actions">
      <button className="primary-button" disabled={pending} onClick={() => onAnswer('allow_once')}>允许一次</button>
      {allowForSession && <button className="secondary-button" disabled={pending} onClick={() => onAnswer('allow_for_session')}>本会话内允许</button>}
      <button className="secondary-button" disabled={pending} onClick={() => onAnswer('deny')}>拒绝</button>
    </div>
    <OutcomeMark outcome={outcome} />
  </li>;
}

function composerBlock(session: SessionSnapshot): string | null {
  switch (session.state.state) {
    case 'starting': return 'Agent 正在启动，启动后才能发送。';
    case 'waiting_for_local_ui': return 'Agent 正在等待本机上的操作。';
    case 'stopping': return '会话正在停止。';
    case 'stopped': case 'failed': return '会话已结束。';
    case 'running': return session.capabilities.deliveryControl ? null : 'Agent 空闲后再发送，草稿会保留。';
    default: return session.streamId ? null : '会话尚未就绪。';
  }
}

type Fence = { sessionId: string; expectedStreamId: string; expectedCwdFence: string };

function Composer({ client, session, fence, connected, agent, messages, onPending, canStop, onStop, stopOutcome }: {
  client: DaemonClient; session: SessionSnapshot; fence: Fence; connected: boolean; agent: string; messages: readonly TranscriptMessage[];
  onPending: Dispatch<SetStateAction<Pending[]>>; canStop: boolean; onStop: () => void; stopOutcome: Outcome | null;
}) {
  const [draft, setDraft, clearIf] = useDraft();
  const [busy, setBusy] = useState(false);
  const block = composerBlock(session);
  const bytes = utf8Bytes(draft);
  const tooLong = bytes > MAX_PROMPT_BYTES;
  const running = session.state.state === 'running';

  async function send(delivery: DeliveryMode) {
    const content = draft;
    if (!content.trim() || tooLong || block || busy || !connected) return;
    const key = crypto.randomUUID();
    const seen = new Set(messages.filter(item => item.role === 'user').map(item => item.id));
    setBusy(true);
    // A new attempt replaces earlier bubbles whose prompt is not known to have been sent; confirmations are kept.
    onPending(list => [...list.filter(item => item.echo || item.outcome?.kind === 'sent' || item.outcome?.kind === 'done').slice(1 - MAX_PENDING), { key, text: content, outcome: null, seen }]);
    const outcome = outcomeOf(await client.call({ method: 'prompt', params: { ...fence, content, delivery } }));
    setBusy(false);
    onPending(list => list.map(item => item.key === key ? { ...item, outcome } : item));
    // A rejected or unknown prompt keeps its draft so nothing typed is lost.
    if (outcome.kind === 'sent' || outcome.kind === 'done') clearIf(content);
  }

  return <form className="composer" onSubmit={event => { event.preventDefault(); void send('follow_up'); }}>
    <label className="visually-hidden" htmlFor="composer-input">给 {agent} 的消息</label>
    <DraftInput id="composer-input" minRows={3} maxRows={12} value={draft} onChange={event => setDraft(event.target.value)}
      placeholder={block ?? `给 ${agent} 发消息（Enter 发送，Shift+Enter 换行）`} aria-describedby="composer-note" />
    <div className="composer-bar">
      <span id="composer-note" className="composer-note">{block ?? (tooLong ? `超过上限：${bytes} / ${MAX_PROMPT_BYTES} 字节` : bytes > MAX_PROMPT_BYTES * 0.8 ? `${bytes} / ${MAX_PROMPT_BYTES} 字节` : '')}</span>
      {canStop && <button type="button" className="secondary-button" disabled={!connected} onClick={onStop}>停止</button>}
      {running && session.capabilities.deliveryControl && <button type="button" className="secondary-button" disabled={!draft.trim() || tooLong || busy || !connected} onClick={() => void send('steer')}>引导</button>}
      <button type="submit" className="primary-button" disabled={!draft.trim() || tooLong || !!block || busy || !connected}>{running ? '排队' : '发送'}</button>
    </div>
    <OutcomeMark outcome={stopOutcome} />
  </form>;
}

type Props = { client: DaemonClient; state: DaemonView; sessionId: string; onNavigate: (href: string) => void };

/** Holds what must outlive a transcript that is briefly unavailable: the draft (in the runtime) and the bubbles. */
export function SessionView(props: Props) {
  const open = props.state.session?.id === props.sessionId ? props.state.session : null;
  const messages = open?.detail?.view.messages ?? NO_MESSAGES;
  const [pending, setPending] = useState<Pending[]>([]);
  useLayoutEffect(() => setPending(list => reconcile(list, messages)), [messages]);
  const shown = useMemo(() => pending.filter(item => !item.echo), [pending]);
  return <ThreadProvider messages={messages} pending={shown}><SessionBody {...props} shown={shown} setPending={setPending} /></ThreadProvider>;
}

function SessionBody({ client, state, sessionId, onNavigate, shown, setPending }: Props & { shown: readonly Pending[]; setPending: Dispatch<SetStateAction<Pending[]>> }) {
  const open = state.session?.id === sessionId ? state.session : null;
  const snapshot = open?.detail?.view.session ?? state.sessions?.find(item => item.id === sessionId) ?? null;
  const [stopOutcome, setStopOutcome] = useState<Outcome | null>(null);
  const [closeOutcome, setCloseOutcome] = useState<Outcome | null>(null);
  const [resumeOutcome, setResumeOutcome] = useState<Outcome | null>(null);
  const [answers, setAnswers] = useState<Record<string, Outcome>>({});
  const [confirmClose, setConfirmClose] = useState(false);
  const end = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const messages = open?.detail?.view.messages;
  const messageCount = messages?.length ?? 0;
  const lastMessage = messages?.at(-1);
  // Follow new output only while the reader is already at the end; never pull them away from history.
  useEffect(() => {
    const scroller = end.current?.closest('.workspace-center');
    if (!scroller) return;
    const onScroll = () => { stick.current = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80; };
    scroller.addEventListener('scroll', onScroll, { passive: true });
    return () => scroller.removeEventListener('scroll', onScroll);
  }, [!!open?.detail]);
  useLayoutEffect(() => { if (stick.current) end.current?.scrollIntoView?.({ block: 'end' }); }, [messageCount, lastMessage, shown.length]);
  const runId = open?.detail?.view.session.activeRunId ?? null;
  const fenceKey = open?.detail ? `${open.detail.view.session.streamId}\u0000${open.detail.view.session.cwdFence}` : null;
  useEffect(() => {
    // ⌘. / Ctrl+. or the command palette stops the current turn (UX §6.7).
    if (!runId || !open?.detail || state.connection !== 'connected') return;
    const session = open.detail.view.session;
    const stopTurn = (event?: Event) => {
      if (event instanceof CustomEvent && event.detail !== runId) return;
      void client.call({ method: 'cancel', params: { sessionId: session.id, expectedStreamId: session.streamId ?? '', expectedCwdFence: session.cwdFence, runId } })
        .then(result => setStopOutcome(cancelOutcomeOf(result)));
    };
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.isComposing || event.keyCode === 229 || event.key !== '.' || !(event.metaKey || event.ctrlKey)) return;
      event.preventDefault();
      stopTurn();
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener(STOP_EVENT, stopTurn);
    return () => { window.removeEventListener('keydown', onKey); window.removeEventListener(STOP_EVENT, stopTurn); };
    // The handler is rebuilt only when the run, its fences or the connection change.
  }, [client, runId, fenceKey, state.connection]);

  if (!open || (!open.detail && open.loading)) return <div className="skeleton-list" aria-label="正在读取会话" aria-busy="true"><div /><div /></div>;
  if (open.removed) return <div className="empty-state"><h2>这个会话已不存在</h2><a className="secondary-button" href="#/sessions">回到会话列表</a></div>;
  if (!open.detail || !snapshot) return <div className="empty-state">
    <h2>会话记录没有读取成功</h2><p>{errorMessage(open.error ?? '')}</p>
    <button className="secondary-button" onClick={() => void client.openSession(sessionId)}>重新读取</button>
  </div>;

  const view = open.detail.view;
  const session = snapshot;
  const agent = runtimeName(state.runtimes, session.runtimeId);
  const project = state.projects?.find(item => item.id === session.projectId) ?? null;
  const connected = state.connection === 'connected';
  const fence = { sessionId: session.id, expectedStreamId: session.streamId ?? '', expectedCwdFence: session.cwdFence };
  const canStop = !!session.activeRunId && (session.state.state === 'running' || session.state.state === 'waiting_for_local_ui');

  async function answer(approvalId: string, decision: ApprovalDecision) {
    const outcome = outcomeOf(await client.call({ method: 'respond_to_approval', params: { ...fence, approvalId, decision } }));
    setAnswers(current => ({ ...current, [approvalId]: outcome }));
  }
  async function stop() {
    if (!session.activeRunId) return;
    setStopOutcome(cancelOutcomeOf(await client.call({ method: 'cancel', params: { ...fence, runId: session.activeRunId } })));
  }
  async function close() {
    setConfirmClose(false);
    setCloseOutcome(outcomeOf(await client.call({ method: 'close_session', params: fence })));
  }
  async function resume() {
    if (!project) return;
    const sessionId = crypto.randomUUID();
    const outcome = outcomeOf(await client.call({ method: 'create_session', params: {
      sessionId, projectId: project.id, runtimeId: session.runtimeId, expectedCwdFence: project.cwdFence, start: { mode: 'resume', params: { sourceSessionId: session.id } },
    } }));
    setResumeOutcome(outcome);
    if (outcome.kind === 'done' || outcome.kind === 'sent') onNavigate(sessionHref(sessionId));
  }

  const ended = session.state.state === 'stopped' || session.state.state === 'failed';
  return <div className="session-view">
    <header className="session-header">
      <div className="session-heading">
        <h2>{sessionTitle(session, state.runtimes)}</h2>
        <StatePill session={session} />
      </div>
      <div className="session-chips">
        <span className="chip">{agent}</span>
        <span className="chip">{project?.name ?? '已移除的项目'}</span>
        {session.model && <span className="chip" title="模型">{session.model.provider} / {session.model.id}</span>}
        <ThinkingControl client={client} session={session} fence={fence} connected={connected} />
        {session.mode === 'resume' && <span className="chip">继续自之前的会话</span>}
        <ToolApprovalMark value={session.capabilities.toolApproval} />
      </div>
      {session.state.state === 'stopped' && session.stopReason && <p className="stop-reason">{stopReasonLabels[session.stopReason]}</p>}
      <div className="session-actions">
        {ended && canResume(session) && <button className="secondary-button" disabled={!connected || !project} onClick={() => void resume()}>继续这个会话</button>}
        {!ended && session.state.state !== 'stopping' && (confirmClose ?
          <span className="inline-confirm">关闭后 Agent 会停止。<button className="secondary-button" onClick={() => void close()}>关闭会话</button><button className="quiet-button" onClick={() => setConfirmClose(false)}>取消</button></span> :
          <button className="quiet-button" disabled={!connected} onClick={() => setConfirmClose(true)}>关闭会话</button>)}
        <OutcomeMark outcome={closeOutcome} /><OutcomeMark outcome={resumeOutcome} />
      </div>
    </header>
    {view.historyTruncated && <p className="history-note">更早的记录没有显示。</p>}
    {view.messages.length === 0 && view.approvals.length === 0 && shown.length === 0 ? <p className="transcript-empty">还没有对话。</p> : <Transcript agent={agent} />}
    {!!open.detail.expired?.length && <ul className="approvals" aria-label="已失效的审批">{open.detail.expired.map(approval => {
      const answered = answers[approval.id]?.kind === 'sent' || answers[approval.id]?.kind === 'done';
      return <li key={approval.id} className="approval-card" data-expired>
        <p className="approval-summary">{approval.summary}</p>
        <p className="field-note">{answered ? '已发送回复；之后这一轮结束了，对话框已关闭。' : '这个审批已失效，没有被批准：这一轮已经结束，或会话已停止。'}</p>
      </li>;
    })}</ul>}
    {view.approvals.length > 0 && <ul className="approvals">{view.approvals.map(approval =>
      <ApprovalCard key={approval.id} approval={approval} allowForSession={session.capabilities.approvalForSession} outcome={answers[approval.id] ?? null} onAnswer={decision => void answer(approval.id, decision)} />)}</ul>}
    <Composer client={client} session={session} fence={fence} connected={connected} agent={agent} messages={view.messages}
      onPending={setPending} canStop={canStop} onStop={() => void stop()} stopOutcome={stopOutcome} />
    <div ref={end} aria-hidden="true" />
  </div>;
}
