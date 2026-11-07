import assert from 'node:assert/strict';
import { test } from 'node:test';
import path from 'node:path';
import { fixtureReady, fixtureStopped, managedArtifact, webviewFinished } from './host-probe.mjs';

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

test('requires the fixture ready marker and public ready session before launching a WebView', () => {
  const value = { marker: 'M1_DESKTOP_HOST_FIXTURE_READY', endpoint: 'synthetic-private-endpoint', session: { id: 'host-commands', state: { state: 'ready' } } };
  assert.deepEqual(fixtureReady(JSON.stringify(value) + '\n'), value);
  for (const wrong of [{ ...value, marker: 'other' }, { ...value, endpoint: '' }, { ...value, session: { id: 'other', state: { state: 'ready' } } }, { ...value, session: { id: 'host-commands', state: { state: 'running' } } }]) {
    assert.throws(() => fixtureReady(JSON.stringify(wrong)));
  }
  assert.throws(() => fixtureReady('not json'));
});
