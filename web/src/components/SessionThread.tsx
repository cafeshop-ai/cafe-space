// The only module that uses assistant-ui's runtime: the daemon stays the source of truth, and this adapts
// its transcript for display. Nothing here sends, edits, reloads or cancels.
import { createContext, useCallback, useContext, useEffect, useMemo, useSyncExternalStore, type ReactNode } from 'react';
import {
  AssistantRuntimeProvider, ComposerPrimitive, MessagePrimitive, ThreadPrimitive, useAui, useAuiState, useExternalStoreRuntime,
  type ReasoningMessagePartProps, type TextMessagePartProps, type ThreadMessageLike, type ToolCallMessagePartProps,
} from '@assistant-ui/react';
import type { TranscriptMessage } from '../protocol/generated/TranscriptMessage';
import type { Outcome } from '../app/copy';
import { OutcomeMark } from './StatePill';

/** A prompt the daemon has not shown in its transcript yet: only ever a local bubble bound to its outcome. */
export type PendingPrompt = { key: string; text: string; outcome: Outcome | null };
type Custom = { role: TranscriptMessage['role']; status?: TranscriptMessage['status']; pending?: true; outcome?: Outcome | null };
type ToolStatus = 'running' | 'complete' | 'error';

const toolStatus: Record<ToolStatus, string> = { running: '运行中', complete: '完成', error: '出错' };
const oneLine = (text: string) => { const line = text.replace(/\s+/g, ' ').trim(); return line.length > 120 ? `${line.slice(0, 119)}…` : line; };

function convert(item: TranscriptMessage | PendingPrompt): ThreadMessageLike {
  if (!('parts' in item)) return { id: `pending:${item.key}`, role: 'user', content: [{ type: 'text', text: item.text }], metadata: { custom: { role: 'user', pending: true, outcome: item.outcome } satisfies Custom } };
  // assistant-ui allows thinking and tools only on assistant messages; the daemon's role is kept for display.
  const role = item.role === 'user' && item.parts.every(part => part.kind === 'text') ? 'user' : 'assistant';
  return {
    id: item.id, role,
    content: item.parts.map(part => part.kind === 'text' ? { type: 'text', text: part.data.text } :
      part.kind === 'thinking' ? { type: 'reasoning', text: part.data.text } :
      { type: 'tool-call', toolCallId: part.data.id, toolName: part.data.name, args: {}, argsText: part.data.arguments, result: part.data.output ?? undefined, isError: part.data.status === 'error', artifact: part.data.status }),
    ...(role === 'assistant' && { status: item.status === 'streaming' ? { type: 'running' } : item.status === 'error' ? { type: 'incomplete', reason: 'error' } : { type: 'complete', reason: 'stop' } }),
    metadata: { custom: { role: item.role, status: item.status } satisfies Custom },
  };
}
type MarkdownModule = typeof import('./Markdown');
let markdown: MarkdownModule | null = null;
let fetching: Promise<void> | null = null;
const waiting = new Set<() => void>();
/** Fetches the Markdown renderer, a chunk of its own (web/README size budget); a failed fetch is tried again on the next request. */
export function loadMarkdown() {
  return fetching ??= import('./Markdown').then(module => { markdown = module; waiting.forEach(notify => notify()); }, () => { fetching = null; });
}
const subscribe = (notify: () => void) => { waiting.add(notify); return () => { waiting.delete(notify); }; };
/** The Markdown module once it has arrived, else null; asking for it starts the fetch. */
function useMarkdown() {
  const module = useSyncExternalStore(subscribe, () => markdown);
  useEffect(() => { if (!module) void loadMarkdown(); }, [module]);
  return module;
}
const neverSend = () => Promise.reject(new Error('Prompts go through the session view with their fences.'));

/**
 * Runs assistant-ui on the daemon's transcript. No `isRunning`, `onCancel`, `onEdit` or `onReload`: running
 * state comes from the session snapshot, and there is no optimistic assistant message, edit, branch or retry.
 */
export function ThreadProvider({ messages, pending, children }: { messages: readonly TranscriptMessage[]; pending: readonly PendingPrompt[]; children: ReactNode }) {
  const items = useMemo(() => [...messages, ...pending], [messages, pending]);
  const runtime = useExternalStoreRuntime<TranscriptMessage | PendingPrompt>({ messages: items, convertMessage: convert, onNew: neverSend, unstable_capabilities: { copy: false } });
  return <AssistantRuntimeProvider runtime={runtime}>{children}</AssistantRuntimeProvider>;
}

const AgentName = createContext('Agent');
const useCustom = () => useAuiState(state => state.message.metadata.custom) as Custom;

function Text({ text }: TextMessagePartProps) {
  const renderer = useMarkdown();
  if (useCustom().role !== 'assistant') return <div className="message-text">{text}</div>;
  // Until the Markdown chunk arrives, a reply is its source text in the box Markdown renders into.
  return renderer ? <renderer.MarkdownText text={text} /> : <div className="markdown"><p className="md-source">{text}</p></div>;
}
const Reasoning = ({ text }: ReasoningMessagePartProps) => <details className="thinking"><summary>思考过程</summary><div className="message-text">{text}</div></details>;
function ToolCard({ toolName, argsText, result, artifact }: ToolCallMessagePartProps) {
  const status = artifact as ToolStatus;
  return <details className="tool-card" data-status={status}>
    <summary><span className="tool-name">{toolName}</span><span className="tool-args">{oneLine(argsText)}</span><span className="tool-status">{toolStatus[status]}</span></summary>
    <div className="tool-body"><h3>输入</h3><pre>{argsText}</pre>{typeof result === 'string' && <><h3>输出</h3><pre>{result}</pre></>}</div>
  </details>;
}
const parts = { Text, Reasoning, tools: { Fallback: ToolCard } };

function Message() {
  const agent = useContext(AgentName);
  const { role, status, pending, outcome } = useCustom();
  const who = role === 'user' ? '你' : role === 'assistant' ? agent : role === 'tool' ? '工具' : '系统';
  return <MessagePrimitive.Root asChild>
    <li className="message" data-role={role} data-pending={pending ? '' : undefined}>
      <div className="message-author">{who}
        {status === 'streaming' && <span className="message-status"> · 正在输出…</span>}
        {status === 'error' && <span className="message-status" data-error> · 出错</span>}
        {pending && !outcome && <span className="message-status"> · 正在发送…</span>}
      </div>
      <MessagePrimitive.Parts components={parts} />
      {pending && <OutcomeMark outcome={outcome ?? null} />}
    </li>
  </MessagePrimitive.Root>;
}
const messageComponents = { Message };

export function Transcript({ agent }: { agent: string }) {
  return <AgentName.Provider value={agent}>
    <ol className="transcript" aria-label="对话记录"><ThreadPrimitive.Messages components={messageComponents} /></ol>
  </AgentName.Provider>;
}

/** The composer's text lives in assistant-ui; the session view decides when it is sent or cleared. */
export function useDraft(): [string, (text: string) => void, (sent: string) => void] {
  const aui = useAui();
  const text = useAuiState(state => state.composer.text);
  const set = useCallback((next: string) => aui.composer.setText(next), [aui]);
  // Clears only the text that was sent, never what was typed after it.
  const clearIf = useCallback((sent: string) => { if (aui.composer.getState().text === sent) aui.composer.setText(''); }, [aui]);
  return [text, set, clearIf];
}

/** Enter submits the surrounding form (never during IME composition); Escape, paste and focus stay native. */
export function DraftInput(props: Omit<ComposerPrimitive.Input.Props, 'submitMode' | 'submitOnEnter'>) {
  return <ComposerPrimitive.Input submitMode="enter" cancelOnEscape={false} addAttachmentOnPaste={false}
    unstable_focusOnRunStart={false} unstable_focusOnScrollToBottom={false} unstable_focusOnThreadSwitched={false} {...props} />;
}
