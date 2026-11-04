import { describe, expect, it } from 'vitest';
import contract from '../../fixtures/pi-rpc/contract.json';
import traces from '../../fixtures/pi-rpc/traces.json';
import displayLimits from '../../fixtures/pi-rpc/display-limits.json';

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
    expect(contract.launchFlagReference.packageVersion).toBe(contract.packageVersion);
    expect(contract.launchFlagReference.cliSource).toBe('docs/cli.md#prompt-and-display-options');
    expect(contract.launchFlagReference.cliSha256).toMatch(/^[a-f0-9]{64}$/);
    expect(contract.launchFlagReference.securitySha256).toMatch(/^[a-f0-9]{64}$/);
    expect(contract.launchFlagReference.securityBoundary).toContain('not a network sandbox');
    expect(contract.launchFlags).not.toContain('--continue');
  });

  it('bounds display without turning an omission into success or a failed session', () => {
    expect(contract.framing.oversize).toContain('never infer a lost response or settlement');
    expect(contract.limits.displayOverflow).toContain('not session failure');
    expect(contract.limits.unknownOrImageContent).toContain('placeholder');
    expect(displayLimits.cases.find(value => value.name === 'oversized_record')).toMatchObject({ bytes: contract.framing.maxRecordBytes + 1, state: 'running', nextSettledState: 'ready' });
    expect(displayLimits.cases.find(value => value.name === 'omitted_response')?.expected).toBe('unknown_at_deadline_never_applied');
    expect(displayLimits.cases.find(value => value.name === 'invalid_bounded_json')?.expected).toBe('PI_RECORD_INVALID');
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
