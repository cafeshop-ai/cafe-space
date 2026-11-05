import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from 'react';
import type { ApprovalDecision } from '../protocol/generated/ApprovalDecision';
import type { ApprovalRequest } from '../protocol/generated/ApprovalRequest';
import type { DeliveryMode } from '../protocol/generated/DeliveryMode';
import type { MessagePart } from '../protocol/generated/MessagePart';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { TranscriptMessage } from '../protocol/generated/TranscriptMessage';
import type { DaemonClient, DaemonView } from '../state/DaemonClient';
import { MAX_PROMPT_BYTES, canResume, cancelOutcomeOf, errorMessage, outcomeOf, stopReasonLabels, utf8Bytes, type Outcome } from '../app/copy';
import { OutcomeMark, StatePill, ToolApprovalMark } from './StatePill';
import { runtimeName, sessionHref, sessionTitle } from './SessionList';

const toolStatus = { running: '运行中', complete: '完成', error: '出错' } as const;
const oneLine = (text: string) => { const line = text.replace(/\s+/g, ' ').trim(); return line.length > 120 ? `${line.slice(0, 119)}…` : line; };

function Part({ part }: { part: MessagePart }) {
  if (part.kind === 'text') return <div className="message-text">{part.data.text}</div>;
  if (part.kind === 'thinking') return <details className="thinking"><summary>思考过程</summary><div className="message-text">{part.data.text}</div></details>;
  const tool = part.data;
  return <details className="tool-card" data-status={tool.status}>
    <summary><span className="tool-name">{tool.name}</span><span className="tool-args">{oneLine(tool.arguments)}</span><span className="tool-status">{toolStatus[tool.status]}</span></summary>
    <div className="tool-body"><h3>输入</h3><pre>{tool.arguments}</pre>{tool.output != null && <><h3>输出</h3><pre>{tool.output}</pre></>}</div>
  </details>;
}

function Message({ message, agent }: { message: TranscriptMessage; agent: string }) {
  const who = message.role === 'user' ? '你' : message.role === 'assistant' ? agent : message.role === 'tool' ? '工具' : '系统';
  return <li className="message" data-role={message.role}>
    <div className="message-author">{who}{message.status === 'streaming' && <span className="message-status"> · 正在输出…</span>}{message.status === 'error' && <span className="message-status" data-error> · 出错</span>}</div>
    {message.parts.map((part, index) => <Part key={index} part={part} />)}
  </li>;
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

export function SessionView({ client, state, sessionId, onNavigate }: { client: DaemonClient; state: DaemonView; sessionId: string; onNavigate: (href: string) => void }) {
  const open = state.session?.id === sessionId ? state.session : null;
  const snapshot = open?.detail?.view.session ?? state.sessions?.find(item => item.id === sessionId) ?? null;
  const [draft, setDraft] = useState('');
  const [promptOutcome, setPromptOutcome] = useState<Outcome | null>(null);
  const [stopOutcome, setStopOutcome] = useState<Outcome | null>(null);
  const [closeOutcome, setCloseOutcome] = useState<Outcome | null>(null);
  const [resumeOutcome, setResumeOutcome] = useState<Outcome | null>(null);
  const [answers, setAnswers] = useState<Record<string, Outcome>>({});
  const [confirmClose, setConfirmClose] = useState(false);
  const [busy, setBusy] = useState(false);
  const end = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const messageCount = open?.detail?.view.messages.length ?? 0;
  const lastMessage = open?.detail?.view.messages.at(-1);
  // Follow new output only while the reader is already at the end; never pull them away from history.
  useEffect(() => {
    const scroller = end.current?.closest('.workspace-center');
    if (!scroller) return;
    const onScroll = () => { stick.current = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80; };
    scroller.addEventListener('scroll', onScroll, { passive: true });
    return () => scroller.removeEventListener('scroll', onScroll);
  }, [!!open?.detail]);
  useLayoutEffect(() => { if (stick.current) end.current?.scrollIntoView?.({ block: 'end' }); }, [messageCount, lastMessage]);
  const runId = open?.detail?.view.session.activeRunId ?? null;
  const fenceKey = open?.detail ? `${open.detail.view.session.streamId}\u0000${open.detail.view.session.cwdFence}` : null;
  useEffect(() => {
    // ⌘. / Ctrl+. stops the current turn (UX §6.7).
    if (!runId || !open?.detail) return;
    const session = open.detail.view.session;
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key !== '.' || !(event.metaKey || event.ctrlKey) || state.connection !== 'connected') return;
      event.preventDefault();
      void client.call({ method: 'cancel', params: { sessionId: session.id, expectedStreamId: session.streamId ?? '', expectedCwdFence: session.cwdFence, runId } })
        .then(result => setStopOutcome(cancelOutcomeOf(result)));
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
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
  const block = composerBlock(session);
  const bytes = utf8Bytes(draft);
  const tooLong = bytes > MAX_PROMPT_BYTES;
  const running = session.state.state === 'running';
  const canStop = !!session.activeRunId && (running || session.state.state === 'waiting_for_local_ui');

  async function send(delivery: DeliveryMode) {
    if (!draft.trim() || tooLong || block || busy || !connected) return;
    setBusy(true); setPromptOutcome(null);
    const outcome = outcomeOf(await client.call({ method: 'prompt', params: { ...fence, content: draft, delivery } }));
    setBusy(false); setPromptOutcome(outcome);
    // A rejected or unknown prompt keeps its draft so nothing typed is lost.
    if (outcome.kind === 'sent' || outcome.kind === 'done') setDraft('');
  }
  function onKey(event: KeyboardEvent<HTMLTextAreaElement>) {
    // Enter during IME composition selects a candidate; it must never send.
    if (event.key !== 'Enter' || event.shiftKey || event.nativeEvent.isComposing || event.keyCode === 229) return;
    event.preventDefault();
    void send('follow_up');
  }
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
    {view.messages.length === 0 && view.approvals.length === 0 ? <p className="transcript-empty">还没有对话。</p> :
      <ol className="transcript" aria-label="对话记录">{view.messages.map(message => <Message key={message.id} message={message} agent={agent} />)}</ol>}
    {view.approvals.length > 0 && <ul className="approvals">{view.approvals.map(approval =>
      <ApprovalCard key={approval.id} approval={approval} allowForSession={session.capabilities.approvalForSession} outcome={answers[approval.id] ?? null} onAnswer={decision => void answer(approval.id, decision)} />)}</ul>}
    <form className="composer" onSubmit={event => { event.preventDefault(); void send('follow_up'); }}>
      <label className="visually-hidden" htmlFor="composer-input">给 {agent} 的消息</label>
      <textarea id="composer-input" value={draft} rows={3} onChange={event => setDraft(event.target.value)} onKeyDown={onKey}
        placeholder={block ?? `给 ${agent} 发消息（Enter 发送，Shift+Enter 换行）`} aria-describedby="composer-note" />
      <div className="composer-bar">
        <span id="composer-note" className="composer-note">{block ?? (tooLong ? `超过上限：${bytes} / ${MAX_PROMPT_BYTES} 字节` : bytes > MAX_PROMPT_BYTES * 0.8 ? `${bytes} / ${MAX_PROMPT_BYTES} 字节` : '')}</span>
        <OutcomeMark outcome={promptOutcome} />
        {canStop && <button type="button" className="secondary-button" disabled={!connected} onClick={() => void stop()}>停止</button>}
        {running && session.capabilities.deliveryControl && <button type="button" className="secondary-button" disabled={!draft.trim() || tooLong || busy || !connected} onClick={() => void send('steer')}>引导</button>}
        <button type="submit" className="primary-button" disabled={!draft.trim() || tooLong || !!block || busy || !connected}>{running ? '排队' : '发送'}</button>
      </div>
      <OutcomeMark outcome={stopOutcome} />
    </form>
    <div ref={end} aria-hidden="true" />
  </div>;
}
