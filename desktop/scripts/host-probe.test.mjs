import assert from 'node:assert/strict';
import { test } from 'node:test';
import path from 'node:path';
import { fixtureReady, managedArtifact } from './host-probe.mjs';

test('selects only the native managed test artifact and rejects missing or ambiguous paths', () => {
  const artifact = executable => ({ reason: 'compiler-artifact', target: { name: 'managed' }, profile: { test: true }, executable });
  const executable = path.resolve('target/debug/deps/managed-example');
  const text = [JSON.stringify({ reason: 'build-finished', success: true }), JSON.stringify(artifact(executable))].join('\n');
  assert.equal(managedArtifact(text), executable);
  assert.throws(() => managedArtifact('{}'));
  assert.throws(() => managedArtifact(JSON.stringify(artifact('relative'))));
  assert.throws(() => managedArtifact([artifact(executable), artifact(executable + '-other')].map(item => JSON.stringify(item)).join('\n')));
});

test('requires the fixture ready marker and public ready session before launching a WebView', () => {
  const value = { marker: 'M1_DESKTOP_HOST_FIXTURE_READY', endpoint: 'synthetic-private-endpoint', session: { id: 'host-commands', state: { state: 'ready' } } };
  assert.deepEqual(fixtureReady(JSON.stringify(value) + '\n'), value);
  for (const wrong of [{ ...value, marker: 'other' }, { ...value, endpoint: '' }, { ...value, session: { id: 'other', state: { state: 'ready' } } }, { ...value, session: { id: 'host-commands', state: { state: 'running' } } }]) {
    assert.throws(() => fixtureReady(JSON.stringify(wrong)));
  }
  assert.throws(() => fixtureReady('not json'));
});
