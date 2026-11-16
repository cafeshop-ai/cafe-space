import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { test } from 'node:test';
import path from 'node:path';
import { failureCleanup, failureCode, failureStage, fixtureReady, fixtureStopped, managedArtifact, probe, ready, safeCode, stop, webviewFinished } from './host-probe.mjs';

test('requires native final-ACK proof in addition to generic success and a clean exit', () => {
  const result = { code: 0, signal: null };
  const markers = ['M1_HOST_WEBVIEW_OK', 'M1_HOST_WEBVIEW_FINAL_ACK_OK'];
  assert.deepEqual(webviewFinished(result, markers.join('\r\n') + '\r\n'), markers);
  for (const output of ['', markers[0], markers[1], markers[0] + '\n' + markers[1] + '_FAKE', markers.join('\n') + '\nM1_HOST_WEBVIEW_FAILED']) {
    assert.throws(() => webviewFinished(result, output));
  }
  for (const exit of [{ code: 1, signal: null }, { code: null, signal: 'SIGTERM' }]) {
    assert.throws(() => webviewFinished(exit, markers.join('\n')));
  }
});

test('reports allowlisted stage and code without exposing the original error', () => {
  const error = new Error('private path and native error', {
    cause: { stage: 'webview_exit', code: 'M1_WEBVIEW_TIMEOUT', detail: '/private/path' },
  });
  assert.equal(failureStage(error), 'webview_exit');
  assert.equal(failureCode(error), 'M1_WEBVIEW_TIMEOUT');
  const unexpected = new Error('secret', { cause: { stage: 'artifact', code: '/private/path' } });
  assert.equal(failureStage(unexpected), 'artifact');
  assert.equal(failureCode(unexpected), 'UNEXPECTED_PROBE_ERROR');
  const cleanup = new Error('primary', { cause: { stage: 'webview_exit', code: 'M1_WEBVIEW_TIMEOUT', cleanup: [
    { stage: 'stop_app', code: 'PROBE_KILL_TIMEOUT', detail: '/private/path' },
    { stage: 'cleanup', code: 'UNEXPECTED_PROBE_ERROR', detail: '/private/path' },
    { stage: 'cleanup', code: '/private/path' },
  ] } });
  assert.deepEqual(failureCleanup(cleanup), [
    { stage: 'stop_app', code: 'PROBE_KILL_TIMEOUT' },
    { stage: 'cleanup', code: 'UNEXPECTED_PROBE_ERROR' },
  ]);
  assert.equal(failureStage(new Error('secret')), 'unknown');
  assert.equal(failureCode(new Error('secret')), 'UNEXPECTED_PROBE_ERROR');
});

test('keeps fixture validation errors inside the diagnostic allowlist', () => {
  const code = action => {
    try { action(); } catch (error) { return safeCode(error); }
    assert.fail('expected validation to fail');
  };
  const readyValue = { marker: 'M1_DESKTOP_HOST_FIXTURE_READY', endpoint: 'private', session: { id: 'host-commands', state: { state: 'ready' } } };
  const stoppedValue = { marker: 'M1_DESKTOP_HOST_FIXTURE_STOPPED', promptCount: 4, nativeCommands: [] };
  assert.equal(code(() => managedArtifact('{}')), 'MANAGED_ARTIFACT_REQUIRED');
  assert.equal(code(() => managedArtifact(JSON.stringify({ reason: 'compiler-artifact', target: { name: 'managed' }, profile: { test: true }, executable: 'relative' }))), 'MANAGED_ARTIFACT_MUST_BE_ABSOLUTE');
  assert.equal(code(() => fixtureReady('not json')), 'FIXTURE_READY_REQUIRED');
  assert.equal(code(() => fixtureReady('{"marker":"M1_DESKTOP_HOST_FIXTURE_READY"')), 'FIXTURE_READY_INVALID_JSON');
  assert.equal(code(() => fixtureReady(JSON.stringify({ ...readyValue, endpoint: '' }))), 'FIXTURE_READY_ENDPOINT_INVALID');
  assert.equal(code(() => fixtureStopped('{"marker":"M1_DESKTOP_HOST_FIXTURE_STOPPED"')), 'FIXTURE_STOPPED_INVALID_JSON');
  assert.equal(code(() => fixtureStopped(JSON.stringify(stoppedValue))), 'NATIVE_COMMAND_ORDER_OR_REPLAY');
});

test('rejects an unapproved probe run with a stable diagnostic', async () => {
  const previous = process.env.CAFE_SPACE_TEST_DESKTOP;
  delete process.env.CAFE_SPACE_TEST_DESKTOP;
  try {
    await assert.rejects(probe(), error => {
      assert.equal(error.message, 'M1_HOST_WEBVIEW_PROBE_FAILED');
      assert.equal(failureStage(error), 'authorization');
      assert.equal(failureCode(error), 'M1_TEST_OPT_IN_REQUIRED');
      return true;
    });
  } finally {
    if (previous === undefined) delete process.env.CAFE_SPACE_TEST_DESKTOP;
    else process.env.CAFE_SPACE_TEST_DESKTOP = previous;
  }
});

test('ignores non-JSON Cargo notes while selecting the exact managed artifact', () => {
  const artifact = executable => ({ reason: 'compiler-artifact', target: { name: 'managed' }, profile: { test: true }, executable });
  const executable = path.resolve('target/debug/deps/managed-example');
  assert.equal(managedArtifact(['cargo note', JSON.stringify(artifact(executable))].join('\n')), executable);
});

test('selects only the native managed test artifact and rejects missing or ambiguous paths', () => {
  const artifact = executable => ({ reason: 'compiler-artifact', target: { name: 'managed' }, profile: { test: true }, executable });
  const executable = path.resolve('target/debug/deps/managed-example');
  const text = [JSON.stringify({ reason: 'build-finished', success: true }), JSON.stringify(artifact(executable))].join('\n');
  assert.equal(managedArtifact(text), executable);
  assert.throws(() => managedArtifact('{}'));
  assert.throws(() => managedArtifact(JSON.stringify(artifact('relative'))));
  assert.throws(() => managedArtifact([artifact(executable), artifact(executable + '-other')].map(item => JSON.stringify(item)).join('\n')));
});

test('requires four prompts, ordered cancel and one allow/deny selection each without replay', () => {
  const value = { marker: 'M1_DESKTOP_HOST_FIXTURE_STOPPED', promptCount: 4, nativeCommands: ['get_state', 'prompt', 'prompt', 'clear_queue', 'abort', 'prompt', 'extension_ui_response', 'confirmed=true', 'prompt', 'extension_ui_response', 'confirmed=false'] };
  assert.deepEqual(fixtureStopped('{}\n' + JSON.stringify(value) + '\n'), value);
  for (const wrong of [
    { ...value, marker: 'other' }, { ...value, promptCount: 1 },
    { ...value, nativeCommands: [...value.nativeCommands.slice(0, 3), 'abort', 'clear_queue', ...value.nativeCommands.slice(5)] },
    { ...value, nativeCommands: value.nativeCommands.filter(command => command !== 'abort') },
    { ...value, nativeCommands: [...value.nativeCommands, 'abort'] },
    { ...value, nativeCommands: ['get_state', 'prompt', ...value.nativeCommands.slice(1)] },
    { ...value, nativeCommands: value.nativeCommands.map(command => command === 'confirmed=false' ? 'confirmed=true' : command) },
    { ...value, nativeCommands: value.nativeCommands.map(command => command === 'confirmed=true' ? 'confirmed=false' : command) },
    { ...value, nativeCommands: value.nativeCommands.filter(command => command !== 'extension_ui_response') },
    { ...value, nativeCommands: [...value.nativeCommands, 'extension_ui_response'] },
  ]) assert.throws(() => fixtureStopped(JSON.stringify(wrong)));
});

test('does not try to kill a child whose spawn failed', async () => {
  const child = new EventEmitter();
  child.exitCode = null;
  child.signalCode = null;
  child.kill = () => assert.fail('spawned child expected');
  await stop({
    child, done: Promise.resolve({ code: null, signal: 'SPAWN_ERROR' }),
    started: Promise.resolve(false), spawned: () => false,
  });
  assert.equal(safeCode(new Error('PROBE_STOP_TIMEOUT')), 'PROBE_STOP_TIMEOUT');
});

test('cleans readiness listeners when the fixture exits early', async () => {
  const stdout = new EventEmitter();
  const child = new EventEmitter();
  child.stdout = stdout;
  const host = { child, output: () => '' };
  const readyPromise = ready(host);
  child.emit('close', 1, null);
  await assert.rejects(readyPromise, error => error.message === 'FIXTURE_EARLY_EXIT');
  assert.equal(stdout.listenerCount('data'), 0);
  assert.equal(child.listenerCount('close'), 0);
  assert.equal(child.listenerCount('error'), 0);
});

test('waits through banner lines for a ready record without requiring a trailing newline', async () => {
  const stdout = new EventEmitter();
  const child = new EventEmitter();
  child.stdout = stdout;
  let output = '';
  const host = { child, output: () => output };
  const readyPromise = ready(host);
  output = 'runner banner\r\nM1_DESKTOP_HOST_FIXTURE_READY is starting\r\n';
  stdout.emit('data');
  output += JSON.stringify({ marker: 'M1_DESKTOP_HOST_FIXTURE_READY', endpoint: 'synthetic-private-endpoint', session: { id: 'host-commands', state: { state: 'ready' } } });
  stdout.emit('data');
  assert.equal((await readyPromise).marker, 'M1_DESKTOP_HOST_FIXTURE_READY');
  assert.equal(stdout.listenerCount('data'), 0);
  assert.equal(child.listenerCount('close'), 0);
  assert.equal(child.listenerCount('error'), 0);
});

test('requires the fixture ready marker and public ready session before launching a WebView', () => {
  const value = { marker: 'M1_DESKTOP_HOST_FIXTURE_READY', endpoint: 'synthetic-private-endpoint', session: { id: 'host-commands', state: { state: 'ready' } } };
  assert.deepEqual(fixtureReady('runner banner\r\nM1_DESKTOP_HOST_FIXTURE_READY is starting\r\n' + JSON.stringify(value) + '\r\n'), value);
  for (const wrong of [{ ...value, marker: 'other' }, { ...value, endpoint: '' }, { ...value, session: { id: 'other', state: { state: 'ready' } } }, { ...value, session: { id: 'host-commands', state: { state: 'running' } } }]) {
    assert.throws(() => fixtureReady(JSON.stringify(wrong)));
  }
  assert.throws(() => fixtureReady('not json'));
});
