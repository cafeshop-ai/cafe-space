import { describe, expect, it } from 'vitest';
import contract from '../../fixtures/pi-rpc/contract.json';
import traces from '../../fixtures/pi-rpc/traces.json';

describe('shared Pi 1.5 RPC adapter contract', () => {
  it('pins the runtime and keeps offline verification separate from production readiness', () => {
    expect(contract.package).toBe('@earendil-works/pi-coding-agent');
    expect(contract.packageVersion).toBe('1.5.0');
    expect(contract.scope).toContain('production runtime remains disabled');
    expect(contract.framing.delimiter).toBe('LF');
    expect(contract.framing.unicodeSeparatorsAreText).toBe(true);
    expect(contract.approvals.allowForSession).toBe(false);
    expect(contract.approvals.selectInputEditor).toContain('no guessed value');
    expect(contract.launchFlags).toContain('--no-approve');
    expect(contract.launchFlags).not.toContain('--continue');
  });

  it('keeps acceptance, retries, agent_end and final settlement distinct', () => {
    for (const trace of traces) {
      for (const step of trace.steps) {
        const record = 'receive' in step ? step.receive : undefined;
        if (record?.type === 'agent_end') expect(step.state).toBe('running');
        if (record?.type === 'agent_settled') expect(step.state).toBe('ready');
      }
    }
    const first = traces.find(trace => trace.name === 'first_turn_and_retry')!;
    expect(first.steps.some(step => 'receive' in step && step.receive?.type === 'message_update'
      && 'assistantMessageEvent' in step.receive && step.receive.assistantMessageEvent?.delta?.includes('\u2028')
      && step.receive.assistantMessageEvent?.delta?.includes('\u2029'))).toBe(true);
    expect(traces.find(trace => trace.name === 'handled_prompt')?.steps.at(-1)?.state).toBe('ready');
  });

  it('cancels queued work before abort and does not guess generic UI permissions', () => {
    const cancel = traces.find(trace => trace.name === 'cancel_clear_queue_then_abort')!;
    expect(cancel.steps.flatMap(step => 'send' in step && step.send ? [step.send.type] : [])).toEqual(['get_state', 'clear_queue', 'abort']);
    const ui = traces.find(trace => trace.name === 'confirm_and_local_ui')!;
    const selection = ui.steps.find(step => 'receive' in step && step.receive?.type === 'extension_ui_request' && 'method' in step.receive && step.receive.method === 'select');
    expect(selection?.state).toBe('waiting_for_local_ui');
  });
});
