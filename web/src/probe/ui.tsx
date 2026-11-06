import { createRoot } from 'react-dom/client';
import { App } from '../app/App';
import { DaemonClient, type DaemonView } from '../state/DaemonClient';
import type { Request, RequestResult } from '../protocol';
import { desktopProbeSessionId, desktopProbePrompt, desktopProbeCancelPrompt, desktopProbeConfirmationPrompt, desktopProbeConfirmationSummary, desktopProbeExpectedText } from '../protocol-fixtures.generated';

function ensure(value: unknown): asserts value {
  if (!value) throw new Error('M1_HOST_UI_PROBE_ASSERTION_FAILED');
}

/** The existing debug-only native probe, driven through the normal session page's controls. */
export async function runHostUiProbe(element: HTMLElement, client = new DaemonClient()) {
  const controls: { request: Request; result: RequestResult }[] = [];
  const methods = new Set<Request['method']>(['prompt', 'cancel', 'respond_to_approval', 'close_session']);
  const call = client.call.bind(client);
  client.call = async request => {
    const result = await call(request);
    if (methods.has(request.method)) controls.push({ request, result });
    return result;
  };
  let connected = false;
  async function wait(predicate: (view: DaemonView) => unknown) {
    const deadline = Date.now() + 10000;
    while (Date.now() < deadline) {
      const view = client.getSnapshot();
      if (view.error || view.session?.error || (connected && view.connection !== 'connected')) throw new Error('M1_HOST_UI_PROBE_DISCONNECTED');
      connected ||= view.connection === 'connected';
      if (predicate(view)) return;
      await new Promise(resolve => setTimeout(resolve, 10));
    }
    throw new Error('M1_HOST_UI_PROBE_TIMEOUT');
  }
  const button = (label: string) => Array.from(element.querySelectorAll<HTMLButtonElement>('button'))
    .find(item => item.textContent?.trim() === label && !item.disabled);
  async function click(label: string) {
    await wait(() => button(label));
    button(label)!.click();
  }
  async function control(method: Request['method'], label: string) {
    const before = controls.length;
    await click(label);
    await wait(() => controls.length > before);
    ensure(controls.length === before + 1 && controls[before].request.method === method
      && controls[before].result.status === 'dispatched');
    return controls[before].request;
  }
  const view = () => client.getSnapshot().session!.detail!.view;
  const ready = (state: DaemonView) => state.session?.detail?.view.session.state.state === 'ready'
    && !state.session.detail.view.session.activeRunId && state.session.detail.view.approvals.length === 0 && !state.session.detail.expired?.length;
  const complete = () => view().messages.some(message => message.role === 'assistant' && message.status === 'complete'
    && message.parts.some(part => part.kind === 'text' && part.data.text === desktopProbeExpectedText));
  async function readReady() {
    await wait(ready);
    await client.openSession(desktopProbeSessionId);
    ensure(ready(client.getSnapshot()) && complete());
    await wait(() => element.querySelector('.transcript')?.textContent?.includes(desktopProbeExpectedText));
  }
  async function prompt(content: string) {
    await wait(() => element.querySelector<HTMLTextAreaElement>('#composer-input'));
    const input = element.querySelector<HTMLTextAreaElement>('#composer-input')!;
    const set = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set;
    ensure(set);
    set.call(input, content);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    const request = await control('prompt', '发送');
    ensure(request.method === 'prompt' && request.params.content === content);
  }
  window.location.hash = `#/sessions/${desktopProbeSessionId}`;
  const root = createRoot(element);
  root.render(<App client={client} nativeAvailable />);
  try {
    await wait(state => state.connection === 'connected' && ready(state) && element.querySelector('#composer-input'));
    const initial = client.getSnapshot();
    ensure(initial.runtimes?.length === 0 && initial.sessions?.length === 1 && initial.sessions[0].id === desktopProbeSessionId);
    ensure(view().session.capabilities.toolApproval === 'never_asks' && !view().session.capabilities.approvalForSession);
    await prompt(desktopProbePrompt);
    await readReady();
    await prompt(desktopProbeCancelPrompt);
    await wait(() => view().session.state.state === 'running' && view().session.activeRunId);
    const runId = view().session.activeRunId;
    await client.openSession(desktopProbeSessionId);
    ensure(view().session.state.state === 'running' && view().session.activeRunId === runId);
    const cancel = await control('cancel', '停止');
    ensure(cancel.method === 'cancel' && cancel.params.runId === runId);
    await readReady();
    for (const [decision, label] of [['allow_once', '允许一次'], ['deny', '拒绝']] as const) {
      await prompt(desktopProbeConfirmationPrompt);
      await wait(() => view().approvals.length === 1 && view().session.state.state === 'running' && view().session.activeRunId);
      const offered = view().approvals[0], runId = view().session.activeRunId;
      ensure(offered.summary === desktopProbeConfirmationSummary);
      await client.openSession(desktopProbeSessionId);
      ensure(view().approvals.length === 1 && view().approvals[0].id === offered.id && view().approvals[0].summary === offered.summary
        && view().session.state.state === 'running' && view().session.activeRunId === runId);
      const selected = await control('respond_to_approval', label);
      ensure(selected.method === 'respond_to_approval' && selected.params.approvalId === offered.id && selected.params.decision === decision);
      await readReady();
      ensure(!client.getSnapshot().session?.detail?.expired?.length);
    }
    await click('关闭会话');
    await wait(() => element.querySelector('.inline-confirm'));
    await control('close_session', '关闭会话');
    await wait(() => view().session.state.state === 'stopped');
    await client.openSession(desktopProbeSessionId);
    ensure(view().session.state.state === 'stopped' && view().session.stopReason === 'SESSION_CLOSED'
      && !view().session.activeRunId && view().approvals.length === 0 && complete());
    await wait(() => element.querySelector('.stop-reason') && element.querySelector('.transcript')?.textContent?.includes(desktopProbeExpectedText));
    ensure(controls.map(item => item.request.method).join(',') === 'prompt,prompt,cancel,prompt,respond_to_approval,prompt,respond_to_approval,close_session');
    // The native final-proof check still needs this connection; the runner exits the isolated window.
    return () => { root.unmount(); client.call = call; };
  } catch (error) {
    root.unmount();
    client.call = call;
    throw error;
  }
}
