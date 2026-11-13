import type { RequestResult } from '../protocol/generated/RequestResult';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { SessionStopReason } from '../protocol/generated/SessionStopReason';
import type { ThinkingLevel } from '../protocol/generated/ThinkingLevel';
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

/** The Pi package the daemon accepts at registration (adapters `pi::install`). */
export const piPackage = { name: '@earendil-works/pi-coding-agent', version: '1.5.0' };

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
  LOCAL_ONLY: '请在本机执行这项操作。',
  OWNER_SETTINGS_WRITE_UNKNOWN: '无法确认设置是否已保存。',
  OWNER_SETTINGS_UNTRUSTED: '本机设置无法安全读取，没有自动修复。',
  OWNER_SETTINGS_READ_FAILED: '本机设置无法安全读取，没有自动修复。',
  OWNER_SETTINGS_INVALID: '这个值无法使用。',
  OWNER_ENVIRONMENT_NOT_DETECTED: '守护进程的环境里已经没有这个变量。',
  OWNER_PATH_REQUIRED: 'PATH 需要非空，并且只包含绝对目录。',
  OWNER_PATH_INVALID: 'PATH 需要非空，并且只包含绝对目录。',
  PROJECT_ROOT_CHANGED: '项目目录已被替换，请在本机重新批准。',
  PROJECT_ROOT_UNAVAILABLE: '项目目录当前不可用，请确认它存在。',
  PROJECT_ROOT_UNSUPPORTED: '请选择本机上的普通文件夹：不能是网络盘、磁盘或挂载点的根目录，也不能在 Café Space 的数据目录里。',
  PROJECT_ROOT_NOT_UTF8: '这个文件夹的路径无法用 UTF-8 表示。',
  PROJECT_ROOT_ALREADY_APPROVED: '这个文件夹已经属于另一个项目。',
  PROJECT_ROOT_IDENTITY_UNAVAILABLE: '无法确认这个文件夹的身份。',
  PROJECT_IN_USE: '请先关闭这个项目里正在运行的会话。',
  PROJECT_LIMIT: '项目数量已达上限。',
  PROCESS_SPAWN_FAILED: 'Agent 进程没能启动。',
  CAPABILITY_UNSUPPORTED: '这个 Agent 不支持这项设置。',
  PI_APPROVAL_SCOPE_UNSUPPORTED: '这个 Agent 不支持“本会话内允许”。',
  PI_APPROVAL_NOT_FOUND: '这个审批已经不存在：可能已被回答，或已经失效。',
  PI_APPROVAL_PENDING: '这个审批的回复正在发送。',
  PI_CONNECTION_LOST: '与 Pi 的连接中断了。',
  PI_CANCEL_OUTCOME_UNKNOWN: '无法确认停止是否生效。',
  PI_CONFIGURATION_OUTCOME_UNKNOWN: '无法确认设置是否生效。',
  PI_STARTUP_BUSY: 'Pi 启动时已经在处理别的任务。',
  PI_COMMAND_REJECTED: 'Pi 拒绝了这个请求。',
  PI_INSTALLATION_INVALID: 'Node 程序和 Pi 包目录都要填完整路径。',
  PI_NODE_UNAVAILABLE: '找不到可运行的 Node 程序。',
  PI_PACKAGE_UNAVAILABLE: '找不到 Pi 包：目录里没有 package.json。',
  PI_PACKAGE_INVALID: 'Pi 包的 package.json 无法读取。',
  PI_VERSION_UNSUPPORTED: `需要 Pi ${piPackage.version}，这个目录里是其他版本。`,
  PI_CLI_UNAVAILABLE: 'Pi 包不完整：缺少 dist/bundle/cli.js。',
  PI_RUNTIME_UNTRUSTED: 'Pi 的登记文件无法安全读取，没有改动它。',
  PI_RUNTIME_READ_FAILED: 'Pi 的登记文件无法安全读取，没有改动它。',
  PI_RUNTIME_INVALID: 'Pi 的登记文件无法安全读取，没有改动它。',
  PI_RUNTIME_WRITE_UNKNOWN: '无法确认 Pi 是否已登记。',
  NOT_CONNECTED: '尚未连接守护进程。',
  CONNECTION_LOST: '连接中断，无法确认结果。',
  TIMEOUT: '等待回复超时，无法确认结果。',
  SEND_FAILED: '请求可能没有送达，无法确认结果。',
  STALE_VIEW: '会话记录没能同步，请重新打开。',
};
/** Families of adapter codes, so a new code still reads as a sentence; the exact code stays behind Details. */
const errorFamilies: [RegExp, string][] = [
  [/OUTCOME_UNKNOWN$/, '无法确认这个操作是否生效。'],
  [/^PI_(NODE|PACKAGE|CLI|INSTALLATION)_/, '没有找到可用的 Pi 安装。'],
  [/^PI_(RESUME_UNAVAILABLE|SESSION_IDENTITY_MISMATCH|SESSION_DIRECTORY_INVALID)$/, '无法继续这个 Pi 会话。'],
  [/^PI_(INPUT_BACKPRESSURE|COMMAND_LIMIT|COMMAND_PENDING|CANCEL_PENDING|DIALOG_LIMIT)$/, 'Pi 正忙，请稍后再试。'],
  [/^PI_RESPONSE_TIMEOUT$/, 'Pi 没有及时回复。'],
  [/^PI_CONNECTION_CLOSED$/, '与 Pi 的连接已关闭。'],
  [/^PI_UNOWNED_RUN$/, 'Pi 自己开始了一轮运行，这个会话已停止。可以继续这个会话。'],
  [/^PI_NOT_READY$/, 'Pi 还没有就绪。'],
  [/^PI_QUEUE_NOT_CLEARED$/, '稍后发送的消息没能清空，所以没有停止。'],
  [/^PI_(RECORD|RESPONSE|MESSAGE|DIALOG|DISPOSITION|COMMAND)_/, 'Pi 返回了无法识别的数据。'],
  [/^PI_/, 'Pi 出现了问题。'],
];
export const errorMessage = (code: string) =>
  errorMessages[code] ?? errorFamilies.find(([pattern]) => pattern.test(code))?.[1] ?? '操作未完成。';

/** UX §6.1 command outcomes: Sent never uses a check mark; unknown is never shown as failed. */
export type Outcome = { kind: 'sent' | 'done' | 'not_done' | 'unknown'; code?: string; label?: string };

/**
 * Stop has its own wording: `applied` means the queue was cleared and the turn had already ended, so
 * nothing was interrupted; `dispatched` means the abort was sent, not that the turn has ended.
 */
export function cancelOutcomeOf(result: RequestResult): Outcome {
  const outcome = outcomeOf(result);
  if (outcome.kind === 'done') return { ...outcome, label: '这一轮已经结束，已清空稍后发送的消息' };
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
  never_asks: '以你的身份自动执行工具',
  may_ask: '某些工具前可能会询问',
  unknown: '未说明是否会询问',
};
export const toolApprovalNotes: Record<ToolApproval, string> = {
  never_asks: '这个 Agent 会以你的身份在项目里直接修改文件、运行命令。',
  may_ask: '部分操作会先请求批准，其余直接执行。',
  unknown: 'Agent 没有说明是否会询问，请按自动执行对待。',
};

/** Thinking levels in protocol order, weakest first. */
export const thinkingLabels: Record<ThinkingLevel, string> = {
  off: '关闭', minimal: '极低', low: '低', medium: '中', high: '高', xhigh: '超高', max: '最大',
};
export const thinkingLevels = Object.keys(thinkingLabels) as ThinkingLevel[];

/** Protocol limit for prompt content, in UTF-8 bytes. */
export const MAX_PROMPT_BYTES = 65536;
export const utf8Bytes = (text: string) => new TextEncoder().encode(text).byteLength;
