import { describe, expect, it } from 'vitest';
import contract from '../../fixtures/acp/contract.json';

describe('shared private ACP v1 contract', () => {
  it('pins the official SDK and never treats the synthetic stage as production acceptance', () => {
    expect(contract.sdkVersion).toBe('5.0.0');
    expect(contract.packageVersion).toBe('2.5.0');
    expect(contract.protocolVersion).toBe(1);
    expect(contract.sdkProcessFeature).toBe(false);
    expect(contract.scope).toContain('no production admission');
    expect(contract.capabilities).toMatchObject({steer: false, queueOwner: 'host', allowForSession: false, clientFilesystem: false, clientTerminal: false});
  });
  it('keeps native identity, content and correlation in the private adapter', () => {
    expect(contract.created.result.sessionId).toBe(contract.prompt.params.sessionId);
    expect(contract.permission.params.sessionId).toBe(contract.created.result.sessionId);
    expect(contract.prompt.params.prompt[0].text).toContain('茶\r\nline\u2028separator\u2029paragraph');
    expect(contract.prompt.id).toBe(contract.completed.id);
    expect(contract.cancelled.result.stopReason).toBe('cancelled');
    expect(contract.modelControl).toContain('session/set_config_option');
    expect(contract.unknownOutcome).toContain('never replay');
  });
  it('cannot infer approval from first option, label or persistent permission', () => {
    expect(contract.permission.params.options[0].kind).toBe('allow_always');
    expect(contract.permission.params.options.find(option => option.kind === 'allow_once')?.optionId).toBe('once');
    expect(contract.permissionPolicy).toContain('unique option ID');
    expect(contract.settlement).toContain('cancel notification do not');
  });
});
