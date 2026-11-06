import { DaemonClient, type DaemonView } from '../state/DaemonClient';
import { desktopProbeSessionId, desktopProbePrompt, desktopProbeCancelPrompt, desktopProbeExpectedText } from '../protocol-fixtures.generated';

function ensure(value: unknown): asserts value {
  if (!value) throw new Error('M1_HOST_PROBE_ASSERTION_FAILED');
}

function waitFor(client: DaemonClient, predicate: (view: DaemonView) => boolean) {
  return new Promise<void>((resolve, reject) => {
    let unsubscribe = () => {};
    const finish = (error?: Error) => { clearTimeout(timer); unsubscribe(); if (error) reject(error); else resolve(); };
    const timer = setTimeout(() => finish(new Error('M1_HOST_PROBE_TIMEOUT')), 10000);
    const check = () => {
      const view = client.getSnapshot();
      if (view.error || view.session?.error || view.connection === 'disconnected' || view.connection === 'version_mismatch') finish(new Error('M1_HOST_PROBE_DISCONNECTED'));
      else if (predicate(view)) finish();
    };
    unsubscribe = client.subscribe(check);
    check();
  });
}

function completeText(view: DaemonView) {
  return view.session?.detail?.view.messages.some(message => message.role === 'assistant' && message.status === 'complete'
    && message.parts.some(part => part.kind === 'text' && part.data.text === desktopProbeExpectedText));
}

/** Runs only in the explicitly enabled debug WebView. Unknown outcomes fail, never retry. */
export async function runHostProbe(client = new DaemonClient()) {
  await client.connect();
  await waitFor(client, view => view.connection === 'connected');
  const initial = client.getSnapshot();
  ensure(initial.runtimes?.length === 0 && initial.sessions?.length === 1);
  ensure(initial.sessions[0].id === desktopProbeSessionId);
  await client.openSession(desktopProbeSessionId);
  const session = client.getSnapshot().session?.detail?.view.session;
  ensure(session?.state.state === 'ready' && !session.activeRunId && session.streamId);
  ensure(session.capabilities.toolApproval === 'never_asks' && !session.capabilities.approvalForSession);
  const fences = { sessionId: session.id, expectedStreamId: session.streamId, expectedCwdFence: session.cwdFence };
  const prompted = await client.call({ method: 'prompt', params: { ...fences, content: desktopProbePrompt, delivery: 'steer' } });
  ensure(prompted.status === 'dispatched');
  await waitFor(client, view => !!completeText(view) && view.session?.detail?.view.session.state.state === 'ready'
    && !view.session.detail.view.session.activeRunId);
  await client.openSession(session.id);
  const settled = client.getSnapshot();
  ensure(completeText(settled) && settled.session?.detail?.view.session.state.state === 'ready'
    && !settled.session.detail.view.session.activeRunId);
  const busyPrompt = await client.call({ method: 'prompt', params: { ...fences, content: desktopProbeCancelPrompt, delivery: 'steer' } });
  ensure(busyPrompt.status === 'dispatched');
  await waitFor(client, view => view.session?.detail?.view.session.state.state === 'running'
    && !!view.session.detail.view.session.activeRunId);
  const runId = client.getSnapshot().session?.detail?.view.session.activeRunId;
  ensure(runId);
  await client.openSession(session.id);
  const running = client.getSnapshot().session?.detail?.view.session;
  ensure(running?.state.state === 'running' && running.activeRunId === runId);
  const cancelled = await client.call({ method: 'cancel', params: { ...fences, runId } });
  ensure(cancelled.status === 'dispatched');
  await waitFor(client, view => view.session?.detail?.view.session.state.state === 'ready'
    && !view.session.detail.view.session.activeRunId);
  await client.openSession(session.id);
  const cancelledView = client.getSnapshot();
  ensure(completeText(cancelledView) && cancelledView.session?.detail?.view.session.state.state === 'ready'
    && !cancelledView.session.detail.view.session.activeRunId);
  const closed = await client.call({ method: 'close_session', params: fences });
  ensure(closed.status === 'dispatched');
  await waitFor(client, view => view.session?.detail?.view.session.state.state === 'stopped');
  await client.openSession(session.id);
  const final = client.getSnapshot();
  ensure(completeText(final) && final.session?.detail?.view.session.stopReason === 'SESSION_CLOSED'
    && final.session.detail.view.session.state.state === 'stopped' && !final.session.detail.view.session.activeRunId);
  // Keep the native connection alive until m1_finish inspects its authoritative final snapshot.
}
