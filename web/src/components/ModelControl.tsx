import { useState } from 'react';
import type { ModelInfo } from '../protocol/generated/ModelInfo';
import type { SessionSnapshot } from '../protocol/generated/SessionSnapshot';
import type { DaemonClient } from '../state/DaemonClient';
import { errorMessage, outcomeOf, type Outcome } from '../app/copy';
import { ChipMenu, NoteChip } from './ChipMenu';
import { OutcomeMark } from './StatePill';
import { readBack, type Fence } from './ThinkingControl';

const named = (model: ModelInfo) => `${model.provider} / ${model.id}`;
const same = (a: ModelInfo | null, b: ModelInfo) => a?.provider === b.provider && a.id === b.id;
// Either part may contain any character, so a key built by joining them could be ambiguous.
const keyOf = (model: ModelInfo) => JSON.stringify([model.provider, model.id]);
/** One configured-model read: no items yet while it is out. */
type Listing = { items?: ModelInfo[]; failure?: Outcome };

/**
 * Session-header model (UX §5.3). Choices come only from the agent's configured list for the current session
 * generation, read when the menu opens; the label is always the daemon's read-back, never the request. It is
 * mounted once per generation, so a late reply for an earlier one changes nothing here.
 */
export function ModelControl({ client, session, fence, connected, local }: { client: DaemonClient; session: SessionSnapshot; fence: Fence; connected: boolean; local: boolean }) {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ outcome: Outcome; adjusted?: ModelInfo | null } | null>(null);
  const [list, setList] = useState<Listing | null>(null);
  const actual = session.model ?? null;
  const label = `模型：${actual ? named(actual) : '未报告'}`;
  // The daemon answers either request only while the agent is idle.
  const idle = session.state.state === 'ready';

  if (!session.capabilities.modelControl) return actual ? <NoteChip className="chip chip-button" note="这个 Agent 不支持切换模型">{label}</NoteChip> : null;
  // The configured list is read only on a connection at this computer.
  if (!local) return actual ? <NoteChip className="chip chip-button" note="只能在这台电脑上切换模型">{label}</NoteChip> : null;

  // A list read once stays; a failed read is tried again only when the menu is opened again.
  async function load() {
    if (list && !list.failure) return;
    setList({});
    const read = await client.call({ method: 'list_models', params: fence });
    setList(read.status === 'applied' && read.data.result.kind === 'models' ? { items: read.data.result.data.items }
      : { failure: read.status === 'applied' ? { kind: 'unknown', code: 'UNEXPECTED_RESULT' } : outcomeOf(read) });
  }
  async function choose(key: string) {
    const model = list?.items?.find(item => keyOf(item) === key);
    if (!model || same(actual, model)) return;
    setBusy(true); setResult(null);
    const outcome = outcomeOf(await client.call({ method: 'set_model', params: { ...fence, provider: model.provider, modelId: model.id } }));
    const adjusted = outcome.kind === 'done' ? await readBack(client, fence, view => view.model ?? null, value => same(value, model)) : undefined;
    setBusy(false); setResult({ outcome, adjusted });
  }

  const note = !list ? null : list.failure ? `没能读取可选模型：${errorMessage(list.failure.code ?? '')}` : !list.items ? '正在读取可选模型…' : 'Agent 没有报告可选模型。';
  return <ChipMenu className="model-control" label={label} name="模型" reason={idle ? undefined : '会话空闲时才能切换模型'}
    disabled={!connected} busy={busy} onOpen={() => void load()} onChoose={key => void choose(key)} note={note}
    options={(list?.items ?? []).map(model => ({ key: keyOf(model), label: named(model), checked: same(actual, model) }))}>
    <OutcomeMark outcome={result?.outcome ?? null} />
    {result?.adjusted !== undefined && <span className="field-note">{result.adjusted ? `Agent 把模型调整为“${named(result.adjusted)}”` : 'Agent 没有报告实际使用的模型'}</span>}
  </ChipMenu>;
}
