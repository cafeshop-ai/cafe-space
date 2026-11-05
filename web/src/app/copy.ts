import type { RequestResult } from '../protocol/generated/RequestResult';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { SessionStopReason } from '../protocol/generated/SessionStopReason';
import type { ToolApproval } from '../protocol/generated/ToolApproval';

/** UX §6.1 state vocabulary. */
export const stateLabels = {
  starting: '正在启动…', ready: '空闲', running: '运行中', waiting_for_local_ui: '需要你', stopping: '正在停止', stopped: '已停止', failed: '失败',
} as const;

export const stopReasonLabels: Record<SessionStopReason, string> = {
  DAEMON_RESTARTED: '守护进程重启后已停止',
  SESSION_CLOSED: '已关闭',
  AGENT_EXITED: 'Agent 已退出',
};

/** UX §5.9: a user-closed session offers no Resume; a stop the user did not ask for does. */
export function canResume(session: SessionSnapshot) {
  if (session.state.state === 'failed') return true;
  return session.state.state === 'stopped' && session.stopReason !== 'SESSION_CLOSED';
}

/** Fixed daemon codes as plain sentences; the raw code stays available behind Details. */
const errorMessages: Record<string, string> = {
  PROJECT_NOT_APPROVED: '这个项目未批准或已被移除。',
  RUNTIME_UNAVAILABLE: '这个 Agent 目前不可用。',
  STALE_CWD_FENCE: '项目目录已经变化，请刷新后再试。',
  STALE_STREAM: '会话已经变化，请先查看它的当前状态。',
  STALE_SESSION: '会话已经变化，请先查看它的当前状态。',
  STALE_RUN: '这一轮已经结束。',
  ID_CONFLICT: '会话标识冲突，请重新新建。',
  SESSION_LIMIT: '会话数量已达上限。',
  SESSION_NOT_FOUND: '这个会话已不存在。',
  RESUME_UNAVAILABLE: '无法继续这个会话。',
  ATTACH_UNAVAILABLE: '暂不支持接入运行中的会话。',
  UPDATE_IN_PROGRESS: '守护进程正在更新，暂不接受新工作。',
  SCHEMA_MIGRATION_FAILED: '数据迁移失败，需要在本机恢复。',
  SHUTDOWN_IN_PROGRESS: '守护进程正在停止。',
  LOCAL_ONLY: '这个操作只能在本机进行。',
  PROJECT_ROOT_CHANGED: '项目目录已被替换，请在本机重新批准。',
  PROJECT_ROOT_UNAVAILABLE: '项目目录当前不可用，请确认它存在。',
  PROJECT_ROOT_UNSUPPORTED: '请选择本机上的普通文件夹：不能是网络盘、磁盘或挂载点的根目录，也不能在 Café Space 的数据目录里。',
  PROJECT_ROOT_NOT_UTF8: '这个文件夹的路径无法用 UTF-8 表示。',
  PROJECT_ROOT_ALREADY_APPROVED: '这个文件夹已经属于另一个项目。',
  PROJECT_ROOT_IDENTITY_UNAVAILABLE: '无法确认这个文件夹的身份。',
  PROJECT_IN_USE: '请先关闭这个项目里正在运行的会话。',
  PROJECT_LIMIT: '项目数量已达上限。',
  PROCESS_SPAWN_FAILED: 'Agent 进程没能启动。',
  NOT_CONNECTED: '尚未连接守护进程。',
  CONNECTION_LOST: '连接中断，无法确认结果。',
  TIMEOUT: '等待回复超时，无法确认结果。',
  SEND_FAILED: '请求可能没有送达，无法确认结果。',
  STALE_VIEW: '会话记录没能同步，请重新打开。',
};
export const errorMessage = (code: string) => errorMessages[code] ?? '操作未完成。';

/** UX §6.1 command outcomes: Sent never uses a check mark; unknown is never shown as failed. */
export type Outcome = { kind: 'sent' | 'done' | 'not_done' | 'unknown'; code?: string; label?: string };

/**
 * Stop has its own wording: `applied` means the queue was cleared and the turn had already ended, so
 * nothing was interrupted; `dispatched` means the abort was sent, not that the turn has ended.
 */
export function cancelOutcomeOf(result: RequestResult): Outcome {
  const outcome = outcomeOf(result);
  if (outcome.kind === 'done') return { ...outcome, label: '这一轮已经结束，已清空排队的输入' };
  if (outcome.kind === 'sent') return { ...outcome, label: '已发送停止，等待这一轮结束' };
  return outcome;
}
export function outcomeOf(result: RequestResult): Outcome {
  switch (result.status) {
    case 'applied': return { kind: 'done' };
    case 'dispatched': return { kind: 'sent' };
    case 'rejected': return { kind: 'not_done', code: result.data.error.code };
    case 'unknown': return { kind: 'unknown', code: result.data.error.code };
  }
}
export const outcomeLabels: Record<Outcome['kind'], string> = {
  sent: '已发送', done: '已完成', not_done: '未完成', unknown: '未知 — 请核对',
};

/** UX §5.2 and architecture D18: what an agent does before running a tool, stated truthfully. */
export const toolApprovalLabels: Record<ToolApproval, string> = {
  never_asks: '执行工具前不会询问',
  may_ask: '某些工具前可能会询问',
  unknown: '未说明是否会询问',
};
export const toolApprovalNotes: Record<ToolApproval, string> = {
  never_asks: '这个 Agent 会在项目里直接修改文件、运行命令，不先确认。',
  may_ask: '它可能在某些操作前请求批准，但不保证每个工具都会询问。',
  unknown: 'Agent 没有说明是否会询问，请按不会询问对待。',
};

/** Protocol limit for prompt content, in UTF-8 bytes. */
export const MAX_PROMPT_BYTES = 65536;
export const utf8Bytes = (text: string) => new TextEncoder().encode(text).byteLength;
