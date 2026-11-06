// Opt-in native IPC check using an existing managed test artifact; never builds or launches Pi.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { readFile } from 'node:fs/promises';
import net from 'node:net';
import { setTimeout as delay } from 'node:timers/promises';
import { fixtureReady, managedArtifact } from './host-probe.mjs';

assert.equal(process.env.CAFE_SPACE_TEST_DESKTOP, '1');
const artifact = managedArtifact(await readFile(new URL('../../target/desktop-host-artifacts.json', import.meta.url), 'utf8'));
const contract = JSON.parse(await readFile(new URL('../../fixtures/pi-rpc/desktop-host-v5.json', import.meta.url), 'utf8'));

async function* frames(socket) {
  let buffer = Buffer.alloc(0);
  for await (const chunk of socket) {
    buffer = Buffer.concat([buffer, chunk]);
    while (buffer.length >= 4) {
      const length = buffer.readUInt32BE(0);
      assert(length > 0 && length <= 262144);
      if (buffer.length < length + 4) break;
      yield JSON.parse(buffer.subarray(4, length + 4).toString('utf8'));
      buffer = buffer.subarray(length + 4);
    }
    assert(buffer.length <= 262148);
  }
  assert.equal(buffer.length, 0);
}

for (const scenario of ['busy', 'approval']) {
  const child = spawn(artifact, ['--desktop-host-fixture'], { stdio: ['ignore', 'pipe', 'pipe'] });
  const exited = new Promise(resolve => {
    child.once('error', () => resolve([null, 'SPAWN_ERROR']));
    child.once('close', (code, signal) => resolve([code, signal]));
  });
  let output = '', socket;
  child.stdout.setEncoding('utf8');
  child.stdout.on('data', chunk => { output = (output + chunk).slice(-65536); });
  child.stderr.resume();
  const deadline = setTimeout(() => { socket?.destroy(); child.kill(); }, 15000);
  try {
    while (!output.includes('\n')) {
      assert.equal(child.exitCode, null);
      const alive = await Promise.race([once(child.stdout, 'data').then(() => true), exited.then(() => false)]);
      assert(alive, 'FIXTURE_EARLY_EXIT');
    }
    const ready = fixtureReady(output);
    socket = net.createConnection(ready.endpoint);
    await once(socket, 'connect');
    const incoming = frames(socket);
    const receive = async () => {
      const next = await incoming.next();
      assert(!next.done, 'FIXTURE_DISCONNECTED');
      return next.value;
    };
    const send = frame => {
      const body = Buffer.from(JSON.stringify(frame)), header = Buffer.alloc(4);
      assert(body.length <= 262144);
      header.writeUInt32BE(body.length);
      socket.write(Buffer.concat([header, body]));
    };
    send({ type: 'hello', supportedProtocolVersions: [5], clientVersion: 'active-disconnect-check' });
    assert.equal((await receive()).type, 'welcome');
    let counter = 0;
    const request = async request => {
      const requestId = `disconnect-${++counter}`;
      send({ type: 'request', protocolVersion: 5, requestId, request });
      const response = await receive();
      assert.equal(response.type, 'response');
      assert.equal(response.requestId, requestId);
      return response.result;
    };
    const runtimes = await request({ method: 'list_runtimes' });
    assert.equal(runtimes.status, 'applied');
    assert.equal(runtimes.data.result.kind, 'runtimes');
    assert.deepEqual(runtimes.data.result.data.items, []);
    const session = ready.session;
    const prompted = await request({ method: 'prompt', params: { sessionId: session.id,
      expectedStreamId: session.streamId, expectedCwdFence: session.cwdFence,
      content: scenario === 'busy' ? contract.cancelPrompt : contract.confirmationPrompt, delivery: 'steer' } });
    assert.equal(prompted.status, 'dispatched');
    let active;
    for (let attempt = 0; attempt < 200; attempt++) {
      const result = await request({ method: 'get_session', params: { sessionId: session.id } });
      assert.equal(result.status, 'applied');
      assert.equal(result.data.result.kind, 'session');
      active = result.data.result.data.session;
      if (active.session.state.state === 'running' && active.session.activeRunId
        && active.approvals.length === (scenario === 'approval' ? 1 : 0)) break;
      await delay(5);
    }
    assert.equal(active.session.id, session.id);
    assert.equal(active.session.state.state, 'running');
    assert(active.session.activeRunId);
    assert.equal(active.approvals.length, scenario === 'approval' ? 1 : 0);
    if (scenario === 'approval') assert.equal(active.approvals[0].summary, contract.confirmationSummary);
    socket.destroy();
    assert.deepEqual(await exited, [0, null]);
    const stopped = JSON.parse(output.trim().split('\n').at(-1));
    assert.equal(stopped.marker, 'M1_DESKTOP_HOST_FIXTURE_STOPPED');
    assert.equal(stopped.promptCount, 1);
    assert.deepEqual(stopped.nativeCommands, ['get_state', 'prompt']);
    console.log(JSON.stringify({ scenario, nativeIpc: true, activeBeforeDisconnect: true,
      pendingDialogs: active.approvals.length, fixtureExited: true, promptCount: 1,
      noExtraNativeCommands: true, modelCalls: 0, productionDisabled: true }));
  } finally {
    clearTimeout(deadline);
    socket?.destroy();
    if (child.exitCode === null && child.signalCode === null) child.kill();
    await exited;
  }
}
